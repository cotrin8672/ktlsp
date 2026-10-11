//! Authoritative compile diagnostics from an external Kotlin build (pure-core parser + gradle
//! runner). Unlike `diagnostics.rs` (name-based, "emit only when provably wrong"), this source is
//! the real compiler, so its output is trusted as-is.
//!
//! Split mirrors the rest of the core: `parse_output` is a pure function over the build's text (no
//! process, no LSP types — unit-tested in milliseconds); `run_gradle_compile` (Unit 2) does the
//! process IO but still carries no LSP types, like `artifacts.rs`. The `lsp.rs` layer converts the
//! resulting `CompileDiagnostic`s into `ls_types::Diagnostic`.
//!
//! `run_gradle_compile` is the single Option-1 swap point: replacing its body with classpath
//! resolution + `kotlinc`/compile-daemon must preserve the `(root, task) -> CompileOutcome`
//! signature so the merge store and publish plumbing stay untouched.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::diagnostics::Severity;

/// The compile task the spike runs. Overridable later; module-aware routing is deferred.
pub const DEFAULT_COMPILE_TASK: &str = "compileKotlin";

/// Execution deadline; verified OS cleanup/EOF failures retain the same worker in quarantine.
/// This does not promise bounded shutdown when process termination itself fails.
pub(crate) const COMPILE_TIMEOUT: Duration = Duration::from_secs(180);

/// Maximum retained diagnostic bytes per stream; classpath data has its own capture budget.
const MAX_OUTPUT_BYTES: usize = 10 * 1024 * 1024;

/// One compiler diagnostic over a file at a 1-based (line, column), exactly as the compiler reports
/// it. Converted to an `ls_types::Diagnostic` (and 0-based UTF-16 positions) at the LSP boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileDiagnostic {
    pub path: String,
    pub line: u32,
    pub col: u32,
    pub severity: Severity,
    pub message: String,
}

/// The result of one build invocation: the diagnostics it reported, plus whether the compile task
/// actually executed. `executed` is the R8 signal — an `UP-TO-DATE`/`NO-SOURCE`/`FROM-CACHE` run
/// carries no information about current errors, so the merge store must not clear on it.
/// The runner also refuses to clear on interrupted or incomplete output.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CompileOutcome {
    pub diagnostics: Vec<CompileDiagnostic>,
    pub executed: bool,
}

/// Parse raw gradle/kotlinc stdout+stderr into a `CompileOutcome`. `task` is the compile task whose
/// execution status drives `executed` (e.g. `compileKotlin`).
pub fn parse_output(output: &str, task: &str) -> CompileOutcome {
    let cleaned = strip_ansi(output);
    let mut diagnostics = Vec::new();
    let mut executed = false;

    for line in cleaned.lines() {
        let trimmed = line.trim_start();
        if let Some(status) = compile_task_status(trimmed, task) {
            if status_means_executed(status) {
                executed = true;
            }
            continue;
        }
        if let Some(d) = parse_diagnostic_line(trimmed) {
            diagnostics.push(d);
        }
    }

    CompileOutcome {
        diagnostics,
        executed,
    }
}

/// Run the project's gradle compile `task` in `root` and parse the result. The single Option-1 swap
/// point — preserve the `(root, task) -> CompileOutcome` signature. Degrades to an empty,
/// `executed:false` outcome on any failure; never panics, never fabricates a diagnostic.
///
/// Blocking (process IO); the caller runs it under `spawn_blocking`. On Windows, a retained Job
/// owns the tree before it can execute. OS cleanup failures quarantine this worker.
pub fn run_gradle_compile(root: &Path, task: &str) -> CompileOutcome {
    if !crate::deps::is_gradle_project(root) {
        return CompileOutcome::default();
    }
    let program = match resolve_gradle(root) {
        Some(p) => p,
        None => {
            tracing::warn!(
                "no gradle wrapper or `gradle` on PATH for {}",
                root.display()
            );
            return CompileOutcome::default();
        }
    };
    tracing::info!(
        "compile: {} {task} in {}",
        program.display(),
        root.display()
    );

    let mut command = Command::new(&program);
    command
        .arg(task)
        .arg("--console=plain")
        .arg("--continue")
        .arg("--no-daemon")
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    let result = OwnedNativeCompile::spawn(&mut command, MAX_OUTPUT_BYTES)
        .map(|owned| owned.wait(COMPILE_TIMEOUT, None));
    #[cfg(not(windows))]
    let result = (|| -> std::io::Result<_> {
        // shortcut: Unix still owns only the wrapper, not an escaped inherited pipe writer.
        // A process-group repair needs independent Unix runtime proof before producer use.
        let mut child = command.spawn()?;
        let mut out = None;
        let mut err = None;
        let readers = (|| -> std::io::Result<()> {
            out = child
                .stdout
                .take()
                .map(|pipe| drain_capped(pipe, MAX_OUTPUT_BYTES))
                .transpose()?;
            err = child
                .stderr
                .take()
                .map(|pipe| drain_capped(pipe, MAX_OUTPUT_BYTES))
                .transpose()?;
            Ok(())
        })();
        if let Err(error) = readers {
            let _ = child.kill();
            let _ = child.wait();
            let _ = join_capped(&mut out);
            let _ = join_capped(&mut err);
            return Err(error);
        }
        let interrupted = wait_with_timeout(&mut child, COMPILE_TIMEOUT);
        let status = child.wait();
        let (out, out_complete) = join_capped(&mut out);
        let (err, err_complete) = join_capped(&mut err);
        Ok((status?, interrupted, out, out_complete, err, err_complete))
    })();
    let (status, interrupted, mut text, out_complete, err, err_complete) = match result {
        Ok(result) => result,
        Err(error) => {
            tracing::warn!(%error, "failed to run owned gradle compile");
            return CompileOutcome::default();
        }
    };
    text.push('\n');
    text.push_str(&err);
    let complete = out_complete && err_complete;
    let outcome = parse_completed_output(&text, task, interrupted, complete);
    if interrupted || !complete {
        tracing::warn!(
            interrupted,
            complete,
            "compile output cannot clear prior diagnostics"
        );
    }
    if !status.success()
        && !interrupted
        && !outcome
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    {
        tracing::warn!(
            "gradle exited non-zero with no compile errors parsed for {} — likely a build-script or \
             configuration failure, not a code error",
            root.display()
        );
    }
    outcome
}

fn parse_completed_output(
    text: &str,
    task: &str,
    interrupted: bool,
    complete: bool,
) -> CompileOutcome {
    let mut outcome = parse_output(text, task);
    outcome.executed &= !interrupted && complete;
    outcome
}

/// Prefer the in-repo wrapper; fall back to `gradle` on PATH only if the resolved binary is OUTSIDE
/// the workspace (a wrapper-shaped binary inside the repo would be attacker-controlled).
fn resolve_gradle(root: &Path) -> Option<PathBuf> {
    let wrapper = root.join(if cfg!(windows) {
        "gradlew.bat"
    } else {
        "gradlew"
    });
    if wrapper.is_file() {
        return Some(wrapper);
    }
    let on_path = find_on_path(if cfg!(windows) {
        "gradle.bat"
    } else {
        "gradle"
    })?;
    let resolved = on_path.canonicalize().ok()?;
    let canon_root = root.canonicalize().ok()?;
    if resolved.starts_with(&canon_root) {
        tracing::warn!(
            "ignoring `gradle` resolved inside the workspace: {}",
            resolved.display()
        );
        return None;
    }
    Some(resolved)
}

/// First `name` found in a `PATH` directory (a plain lookup; we don't invoke through a shell).
fn find_on_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// Own the exact Windows tree and both readers on the same blocking worker.
#[cfg(windows)]
pub(crate) struct OwnedNativeCompile {
    tree: process_tree::ProcessTree,
    out: Option<std::thread::JoinHandle<(String, bool)>>,
    err: Option<std::thread::JoinHandle<(String, bool)>>,
}

#[cfg(windows)]
impl OwnedNativeCompile {
    pub(crate) fn spawn(command: &mut Command, stdout_limit: usize) -> std::io::Result<Self> {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // The guard exists before either reader: even a second thread-spawn failure must clean up.
        let mut owned = Self {
            tree: process_tree::ProcessTree::spawn(command)?,
            out: None,
            err: None,
        };
        if let Some(pipe) = owned.tree.child_mut().stdout.take() {
            owned.out = Some(drain_capped(pipe, stdout_limit)?);
        }
        if let Some(pipe) = owned.tree.child_mut().stderr.take() {
            owned.err = Some(drain_capped(pipe, MAX_OUTPUT_BYTES)?);
        }
        Ok(owned)
    }

    pub(crate) fn wait(
        mut self,
        timeout: Duration,
        cancelled: Option<&std::sync::atomic::AtomicBool>,
    ) -> (std::process::ExitStatus, bool, String, bool, String, bool) {
        // A successful Gradle wrapper may precede a short single-use daemon shutdown tail.
        const NATURAL_TREE_EXIT_GRACE: Duration = Duration::from_secs(2);
        let deadline = Instant::now() + timeout;
        let is_cancelled =
            || cancelled.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire));
        let interrupted = loop {
            if Instant::now() >= deadline || is_cancelled() {
                break true;
            }
            match self.tree.root_has_exited() {
                Ok(true) => break false,
                Ok(false) => std::thread::sleep(Duration::from_millis(50)),
                Err(error) => {
                    tracing::warn!(%error, "compile: could not observe owned root exit");
                    break true;
                }
            }
        };
        let grace = (!interrupted).then(|| deadline.min(Instant::now() + NATURAL_TREE_EXIT_GRACE));
        // Wrapper exit alone is insufficient. All assigned processes must be terminal before EOF.
        let (status, forced) = self.tree.cleanup(grace);
        let (out, out_complete) = join_capped(&mut self.out);
        let (err, err_complete) = join_capped(&mut self.err);
        let interrupted = interrupted || forced || is_cancelled() || Instant::now() >= deadline;
        (status, interrupted, out, out_complete, err, err_complete)
    }
}

#[cfg(windows)]
impl Drop for OwnedNativeCompile {
    fn drop(&mut self) {
        // Early return/unwind retains the worker until tree verification and both joins finish.
        let _ = self.tree.cleanup(None);
        let _ = join_capped(&mut self.out);
        let _ = join_capped(&mut self.err);
    }
}

/// Drain to EOF with bounded capture; truncation/read failure cannot clear errors.
fn drain_capped<R: Read + Send + 'static>(
    mut reader: R,
    limit: usize,
) -> std::io::Result<std::thread::JoinHandle<(String, bool)>> {
    std::thread::Builder::new()
        .name("ktlsp-compile-output".into())
        .spawn(move || {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            let mut complete = true;
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        complete = false;
                        break;
                    }
                    Ok(n) => {
                        let keep = n.min(limit.saturating_sub(buf.len()));
                        buf.extend_from_slice(&chunk[..keep]);
                        complete &= keep == n;
                    }
                }
            }
            let text = String::from_utf8(buf)
                .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned());
            (text, complete)
        })
}

fn join_capped(reader: &mut Option<std::thread::JoinHandle<(String, bool)>>) -> (String, bool) {
    // No inherited parent writer + zero assigned live processes proves EOF. If the OS violates
    // that proof, keep this worker and its readers quarantined; never detach or claim shutdown.
    let started = Instant::now();
    let mut reported = false;
    while reader.as_ref().is_some_and(|handle| !handle.is_finished()) {
        if !reported && started.elapsed() >= Duration::from_secs(2) {
            tracing::error!("compile: output EOF unverified; worker quarantined");
            reported = true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    reader
        .take()
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default()
}

#[cfg(not(windows))]
/// Legacy wrapper-only timeout. Unix descendant ownership is not proved by this helper.
fn wait_with_timeout(child: &mut std::process::Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return false,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    return true;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return true,
        }
    }
}

#[cfg(windows)]
mod process_tree {
    use super::*;
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::os::windows::process::CommandExt;
    use std::process::{Child, ExitStatus};
    use windows_sys::Win32::Foundation::{ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
        JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
        TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        GetProcessId, GetProcessIdOfThread, OpenThread, ResumeThread, CREATE_NO_WINDOW,
        CREATE_SUSPENDED, THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
    };

    pub(super) struct ProcessTree {
        child: Child,
        job: OwnedHandle,
        status: Option<ExitStatus>,
        forced: bool,
        terminal: bool,
    }

    impl ProcessTree {
        pub(super) fn spawn(command: &mut Command) -> io::Result<Self> {
            Self::spawn_before_resume(command, |_| Ok(()))
        }

        pub(super) fn spawn_before_resume(
            command: &mut Command,
            before_resume: impl FnOnce(&Child) -> io::Result<()>,
        ) -> io::Result<Self> {
            // Null security attributes: non-inheritable job. Breakaway is never enabled.
            let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if raw.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = unsafe { OwnedHandle::from_raw_handle(raw) };
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if unsafe {
                SetInformationJobObject(
                    job.as_raw_handle(),
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of_val(&limits) as u32,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            command.creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW);
            let owned = Self {
                child: command.spawn()?,
                job,
                status: None,
                forced: false,
                terminal: false,
            };
            if unsafe { GetProcessId(owned.child.as_raw_handle()) } != owned.child.id() {
                return Err(io::Error::other("retained child identity mismatch"));
            }
            if unsafe {
                AssignProcessToJobObject(owned.job.as_raw_handle(), owned.child.as_raw_handle())
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            before_resume(&owned.child)?;
            resume_primary_thread(&owned.child)?;
            Ok(owned)
        }

        pub(super) fn child_mut(&mut self) -> &mut Child {
            &mut self.child
        }

        pub(super) fn root_has_exited(&mut self) -> io::Result<bool> {
            if self.status.is_none() {
                self.status = self.child.try_wait()?;
            }
            Ok(self.status.is_some())
        }

        #[cfg(test)]
        pub(super) fn contains(
            &self,
            process: std::os::windows::io::RawHandle,
        ) -> io::Result<bool> {
            let mut member = 0;
            if unsafe {
                windows_sys::Win32::System::JobObjects::IsProcessInJob(
                    process,
                    self.job.as_raw_handle(),
                    &mut member,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(member != 0)
        }

        fn active(&self) -> io::Result<u32> {
            let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
            if unsafe {
                QueryInformationJobObject(
                    self.job.as_raw_handle(),
                    JobObjectBasicAccountingInformation,
                    (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                    std::mem::size_of_val(&info) as u32,
                    std::ptr::null_mut(),
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(info.ActiveProcesses)
        }

        pub(super) fn cleanup(&mut self, natural_deadline: Option<Instant>) -> (ExitStatus, bool) {
            if self.terminal {
                return (self.status.expect("terminal root status"), self.forced);
            }
            let mut stopping_since = None;
            let mut reported = false;
            loop {
                let root_done = self.root_has_exited().unwrap_or(false);
                match self.active() {
                    Ok(0) if root_done => {
                        self.terminal = true;
                        return (self.status.expect("observed root status"), self.forced);
                    }
                    Ok(active)
                        if active > 0
                            && root_done
                            && natural_deadline
                                .is_some_and(|deadline| Instant::now() < deadline) =>
                    {
                        std::thread::sleep(Duration::from_millis(50));
                        continue;
                    }
                    Ok(0) => {}
                    _ => {
                        if unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) } != 0 {
                            self.forced = true;
                        }
                    }
                }
                // Assignment/resume can fail. Kill only the exact retained (still suspended) root.
                if !root_done && self.child.kill().is_ok() {
                    self.forced = true;
                }
                let started = stopping_since.get_or_insert_with(Instant::now);
                if !reported && started.elapsed() >= Duration::from_secs(2) {
                    tracing::error!(
                        "compile: job/root terminal state unverified; worker quarantined"
                    );
                    reported = true;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }

    impl Drop for ProcessTree {
        fn drop(&mut self) {
            let _ = self.cleanup(None);
        }
    }

    fn resume_primary_thread(child: &Child) -> io::Result<()> {
        // Stable std closes the initial thread handle. Reopen only the unique suspended thread
        // of the retained process; no nightly spawn attributes or undocumented NtResumeProcess.
        let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut entry = THREADENTRY32::default();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut next = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) };
        let mut id = None;
        while next != 0 {
            if entry.th32OwnerProcessID == child.id() {
                if id.replace(entry.th32ThreadID).is_some() {
                    return Err(io::Error::other("suspended child has multiple threads"));
                }
            }
            entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            next = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) };
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_NO_MORE_FILES as i32) {
            return Err(error);
        }
        let id =
            id.ok_or_else(|| io::Error::other("suspended child's primary thread not found"))?;
        let raw = unsafe {
            OpenThread(
                THREAD_SUSPEND_RESUME | THREAD_QUERY_LIMITED_INFORMATION,
                0,
                id,
            )
        };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        let thread = unsafe { OwnedHandle::from_raw_handle(raw) };
        if unsafe { GetProcessIdOfThread(thread.as_raw_handle()) } != child.id() {
            return Err(io::Error::other("primary thread owner mismatch"));
        }
        let count = unsafe { ResumeThread(thread.as_raw_handle()) };
        if count == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        if count != 1 {
            return Err(io::Error::other("unexpected primary thread suspend count"));
        }
        Ok(())
    }
}

/// If `line` is a gradle task-status line for our compile `task` (e.g. `> Task :app:compileKotlin`
/// or `> Task :compileKotlin UP-TO-DATE`), return the status suffix (`""` when the task executed).
fn compile_task_status<'a>(line: &'a str, task: &str) -> Option<&'a str> {
    let rest = line.strip_prefix("> Task ")?;
    let (task_path, status) = match rest.split_once(char::is_whitespace) {
        Some((p, s)) => (p, s.trim()),
        None => (rest, ""),
    };
    let last_segment = task_path.rsplit(':').next()?;
    (last_segment == task).then_some(status)
}

/// A gradle task with no status suffix executed; the named statuses mean it was skipped.
fn status_means_executed(status: &str) -> bool {
    !matches!(
        status,
        "UP-TO-DATE" | "NO-SOURCE" | "FROM-CACHE" | "SKIPPED"
    )
}

/// Parse one `e:`/`w:` compiler line into a diagnostic, or `None` if it isn't a located diagnostic
/// (a prefixed line without a parseable location, like `w: Kotlin language version ...`, is skipped).
fn parse_diagnostic_line(line: &str) -> Option<CompileDiagnostic> {
    let (severity, rest) = if let Some(r) = line.strip_prefix("e:") {
        (Severity::Error, r.trim_start())
    } else if let Some(r) = line.strip_prefix("w:") {
        (Severity::Warning, r.trim_start())
    } else {
        return None;
    };

    parse_legacy(rest, severity).or_else(|| parse_modern(rest, severity))
}

/// Legacy form: `path: (line, col): message`.
fn parse_legacy(rest: &str, severity: Severity) -> Option<CompileDiagnostic> {
    let open = rest.find(": (")?;
    let path = &rest[..open];
    let after = &rest[open + 3..];
    let close = after.find(')')?;
    let (line, col) = parse_line_col(&after[..close])?;
    let message = after[close + 1..]
        .trim_start_matches(':')
        .trim()
        .to_string();
    Some(CompileDiagnostic {
        path: path.to_string(),
        line,
        col,
        severity,
        message,
    })
}

/// Modern form: `file:///abs/Foo.kt:line:col message` (scheme optional; Windows drive aware). Strips
/// the `file://` scheme *before* locating the `:line:col` boundary, so a path containing spaces isn't
/// truncated by a naive whitespace split.
fn parse_modern(rest: &str, severity: Severity) -> Option<CompileDiagnostic> {
    let s = strip_file_scheme(rest);
    let (path, line, col, after) = find_line_col(s)?;
    let message = s[after..].trim_start_matches(':').trim().to_string();
    Some(CompileDiagnostic {
        path: path.to_string(),
        line,
        col,
        severity,
        message,
    })
}

/// Locate the first `:<line>:<col>` boundary (two colon-separated integer runs, ending at
/// whitespace / `:` / end). On a unix path there are no colons before it; a Windows `C:` drive's
/// colon isn't followed by digits, so it's skipped. Returns `(path, line, col, byte-after-col)`.
fn find_line_col(s: &str) -> Option<(&str, u32, u32, usize)> {
    let b = s.as_bytes();
    let digits = |mut k: usize| {
        let start = k;
        while k < b.len() && b[k].is_ascii_digit() {
            k += 1;
        }
        (start, k)
    };
    for i in 0..b.len() {
        if b[i] != b':' {
            continue;
        }
        let (ls, le) = digits(i + 1);
        if le == ls || le >= b.len() || b[le] != b':' {
            continue;
        }
        let (cs, ce) = digits(le + 1);
        if ce == cs {
            continue;
        }
        if ce == b.len() || b[ce] == b' ' || b[ce] == b'\t' || b[ce] == b':' {
            let line = s[ls..le].parse().ok()?;
            let col = s[cs..ce].parse().ok()?;
            return Some((&s[..i], line, col, ce));
        }
    }
    None
}

/// Strip a leading `file://` scheme, and the slash a `file:///C:/...` URL puts before a drive letter.
fn strip_file_scheme(loc: &str) -> &str {
    let loc = loc.strip_prefix("file://").unwrap_or(loc);
    let bytes = loc.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
        &loc[1..]
    } else {
        loc
    }
}

/// Parse `"12, 5"` (legacy) into `(line, col)`.
fn parse_line_col(s: &str) -> Option<(u32, u32)> {
    let (l, c) = s.split_once(',')?;
    Some((l.trim().parse().ok()?, c.trim().parse().ok()?))
}

/// Remove ANSI escape sequences (CSI `\x1b[ … <final>` and OSC `\x1b] … BEL|ST`) so colored build
/// output parses cleanly and no escape bytes leak into diagnostic messages.
fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.clone().next() {
            Some('[') => {
                chars.next();
                for f in chars.by_ref() {
                    if ('@'..='~').contains(&f) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                while let Some(f) = chars.next() {
                    if f == '\x07' {
                        break;
                    }
                    if f == '\x1b' {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diags(output: &str) -> Vec<CompileDiagnostic> {
        parse_output(output, "compileKotlin").diagnostics
    }

    #[test]
    fn modern_error_line() {
        let d = diags("e: file:///abs/path/Foo.kt:12:5 Unresolved reference: bar\n");
        assert_eq!(
            d,
            vec![CompileDiagnostic {
                path: "/abs/path/Foo.kt".into(),
                line: 12,
                col: 5,
                severity: Severity::Error,
                message: "Unresolved reference: bar".into(),
            }]
        );
    }

    #[test]
    fn legacy_error_line() {
        let d = diags("e: /abs/path/Foo.kt: (12, 5): Unresolved reference: bar\n");
        assert_eq!(
            d,
            vec![CompileDiagnostic {
                path: "/abs/path/Foo.kt".into(),
                line: 12,
                col: 5,
                severity: Severity::Error,
                message: "Unresolved reference: bar".into(),
            }]
        );
    }

    #[test]
    fn warning_severity() {
        let d = diags("w: file:///a/B.kt:1:1 Variable 'x' is never used\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].severity, Severity::Warning);
    }

    #[test]
    fn executed_from_task_status() {
        assert!(parse_output("> Task :app:compileKotlin\n", "compileKotlin").executed);
        assert!(!parse_output("> Task :app:compileKotlin UP-TO-DATE\n", "compileKotlin").executed);
        assert!(!parse_output("> Task :compileKotlin NO-SOURCE\n", "compileKotlin").executed);
        assert!(!parse_output("> Task :compileKotlin FROM-CACHE\n", "compileKotlin").executed);
        assert!(!parse_output("> Task :compileKotlin SKIPPED\n", "compileKotlin").executed);
    }

    #[test]
    fn path_with_spaces_preserved() {
        let d = diags("e: file:///my path/A.kt:2:3 oops: x\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].path, "/my path/A.kt");
        assert_eq!(d[0].line, 2);
        assert_eq!(d[0].col, 3);
        assert_eq!(d[0].message, "oops: x");
    }

    #[test]
    fn osc_sequence_stripped() {
        let d = diags("\x1b]0;window title\x07e: file:///a/A.kt:1:1 boom\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].message, "boom");
    }

    #[test]
    fn multiple_files() {
        let out = "e: file:///a/A.kt:1:1 oops\ne: file:///b/B.kt:2:3 nope\n";
        let d = diags(out);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].path, "/a/A.kt");
        assert_eq!(d[1].path, "/b/B.kt");
    }

    #[test]
    fn lifecycle_noise_ignored() {
        let out = "> Configure project :app\nBUILD FAILED in 2s\n3 actionable tasks\n";
        assert!(diags(out).is_empty());
    }

    #[test]
    fn empty_output() {
        let outcome = parse_output("", "compileKotlin");
        assert!(outcome.diagnostics.is_empty());
        assert!(!outcome.executed);
    }

    #[test]
    fn message_colon_preserved() {
        let d = diags("e: file:///a/A.kt:1:1 Unresolved reference: foo: bar\n");
        assert_eq!(d[0].message, "Unresolved reference: foo: bar");
    }

    #[test]
    fn windows_drive_path() {
        let d = diags("e: file:///C:/src/Foo.kt:12:5 boom\n");
        assert_eq!(d[0].path, "C:/src/Foo.kt");
        assert_eq!(d[0].line, 12);
        assert_eq!(d[0].col, 5);
    }

    #[test]
    fn ansi_stripped() {
        let d = diags("\x1b[31me: file:///a/A.kt:1:1 boom\x1b[0m\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].message, "boom");
        assert!(!d[0].message.contains('\x1b'));
    }

    #[test]
    fn malformed_location_skipped() {
        assert!(diags("e: something went wrong with no location\n").is_empty());
        assert!(diags("w: Kotlin language version 1.9 is deprecated\n").is_empty());
    }

    #[test]
    fn trailing_colon_after_location() {
        let d = diags("e: file:///a/A.kt:3:7: type mismatch\n");
        assert_eq!(d[0].line, 3);
        assert_eq!(d[0].col, 7);
        assert_eq!(d[0].message, "type mismatch");
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ktlsp_compile_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn resolve_prefers_wrapper() {
        let dir = scratch("wrapper");
        let wrapper = dir.join(if cfg!(windows) {
            "gradlew.bat"
        } else {
            "gradlew"
        });
        std::fs::write(&wrapper, "#!/bin/sh\n").unwrap();
        assert_eq!(resolve_gradle(&dir), Some(wrapper));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn non_gradle_dir_yields_empty_outcome() {
        let dir = scratch("nongradle");
        let outcome = run_gradle_compile(&dir, DEFAULT_COMPILE_TASK);
        assert_eq!(outcome, CompileOutcome::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Real gradle invocation against the sample project. Ignored by default: requires a gradle
    /// wrapper or `gradle` on PATH, which the unit environment lacks. Run manually with
    /// `cargo test -- --ignored gradle_sample`.
    #[test]
    #[ignore]
    fn gradle_sample_integration() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dev/gradle-sample");
        let outcome = run_gradle_compile(&root, DEFAULT_COMPILE_TASK);
        assert!(
            outcome.executed,
            "compile task should have run against the sample"
        );
    }
}

// This subprocess fixture needs no Gradle/Java installation or user project.
#[cfg(all(test, windows))]
mod native_compile_ownership_tests {
    use super::*;
    use std::io::Write;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        GetProcessId, OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_SYNCHRONIZE,
    };

    fn fixture_command(directory: &Path, mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "compile::native_compile_ownership_tests::native_fixture",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("KTLSP_NATIVE_COMPILE_TEST_DIR", directory)
            .env("KTLSP_NATIVE_COMPILE_TEST_MODE", mode)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn await_file(directory: &Path, name: &str) -> Vec<u8> {
        let start = Instant::now();
        loop {
            if let Ok(bytes) = std::fs::read(directory.join(name)) {
                return bytes;
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "fixture did not reach {name}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let seq = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "ktlsp-native-compile-{}-{nonce}-{seq}",
                std::process::id()
            ));
            std::fs::create_dir(&path).unwrap(); // Never adopt/delete a preexisting directory.
            Self(path)
        }
        fn hold_leaf(&self) -> OwnedHandle {
            let pid: u32 = String::from_utf8(await_file(&self.0, "leaf.pid"))
                .unwrap()
                .parse()
                .unwrap();
            let raw = unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                    0,
                    pid,
                )
            };
            assert!(!raw.is_null(), "fixture leaf must still be alive");
            let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
            assert_eq!(unsafe { GetProcessId(handle.as_raw_handle()) }, pid);
            assert_eq!(
                unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) },
                WAIT_TIMEOUT
            );
            handle
        }
        fn write_gradle_wrapper(&self, mode: &str) {
            assert!(["exit", "large-classpath", "oversized-classpath"].contains(&mode));
            fn batch_path(path: &Path) -> String {
                let path = path.to_str().unwrap();
                assert!(!path.contains(['\r', '\n', '"']));
                path.replace('%', "%%")
            }
            let script = format!(
                "@echo off\r\nsetlocal DisableDelayedExpansion\r\nset \"KTLSP_NATIVE_COMPILE_TEST_DIR={}\"\r\nset \"KTLSP_NATIVE_COMPILE_TEST_MODE={mode}\"\r\n\"{}\" --exact compile::native_compile_ownership_tests::native_fixture --ignored --nocapture --test-threads=1\r\nexit /b %errorlevel%\r\n",
                batch_path(&self.0), batch_path(&std::env::current_exe().unwrap()),
            );
            std::fs::write(
                self.0.join("build.gradle.kts"),
                b"// owned fake wrapper fixture\n",
            )
            .unwrap();
            std::fs::write(self.0.join("gradlew.bat"), script).unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            // Only known files in the directory successfully created by this fixture.
            for file in [
                "leaf.pid",
                "leaf.pid.tmp",
                "output-ready",
                "leaf.exit",
                "fixture-started",
                "build.gradle.kts",
                "gradlew.bat",
            ] {
                let _ = std::fs::remove_file(self.0.join(file));
            }
            let _ = std::fs::remove_dir(&self.0);
        }
    }

    #[test]
    #[ignore = "subprocess fixture; only owning tests supply its Temp directory and mode"]
    fn native_fixture() {
        let Ok(mode) = std::env::var("KTLSP_NATIVE_COMPILE_TEST_MODE") else {
            return;
        };
        let dir = PathBuf::from(std::env::var_os("KTLSP_NATIVE_COMPILE_TEST_DIR").unwrap());
        assert!(dir.is_dir());
        std::fs::write(dir.join("fixture-started"), b"resumed").unwrap();
        if mode == "large-classpath" || mode == "oversized-classpath" {
            let size = if mode == "large-classpath" {
                11 * 1024 * 1024
            } else {
                crate::classpath::MAX_DUMP_OUTPUT_BYTES + 8192
            };
            let mut output = std::io::BufWriter::new(std::io::stdout().lock());
            writeln!(output, "\nPROJECT\t:head\t{}", dir.display()).unwrap();
            let entry = format!("CP\tC:/classpath/{}.jar\n", "x".repeat(1024));
            for _ in 0..size.div_ceil(entry.len()) {
                output.write_all(entry.as_bytes()).unwrap();
            }
            writeln!(output, "END\t:head").unwrap();
            writeln!(output, "PROJECT\t:tail\t{}", dir.display()).unwrap();
            writeln!(output, "CP\tC:/classpath/tail.jar\nEND\t:tail").unwrap();
            output.flush().unwrap();
            return;
        }
        if mode == "leaf" || mode == "leaf-natural" {
            std::fs::write(dir.join("leaf.pid.tmp"), std::process::id().to_string()).unwrap();
            std::fs::rename(dir.join("leaf.pid.tmp"), dir.join("leaf.pid")).unwrap();
            let started = Instant::now();
            while started.elapsed() < Duration::from_secs(20) {
                if mode == "leaf-natural" && dir.join("leaf.exit").is_file() {
                    std::thread::sleep(Duration::from_millis(100));
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            return;
        }
        println!("\n> Task :compileKotlin");
        eprintln!("\nw: file:///C:/owned/Fixture.kt:1:1 fixture retained diagnostic");
        std::io::stdout().flush().unwrap();
        std::io::stderr().flush().unwrap();
        if mode == "plain" {
            return;
        }
        if mode == "error" {
            eprintln!("e: file:///C:/owned/Fixture.kt:2:3 genuine compiler error");
            std::process::exit(7);
        }
        assert!(
            ["exit", "cancel", "timeout", "nonzero", "noisy", "natural"].contains(&mode.as_str())
        );
        let mut command = fixture_command(
            &dir,
            if mode == "natural" {
                "leaf-natural"
            } else {
                "leaf"
            },
        );
        command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
        let mut leaf = command.spawn().unwrap();
        await_file(&dir, "leaf.pid");
        if mode == "noisy" {
            let chunk = [b'x'; 8192];
            for _ in 0..MAX_OUTPUT_BYTES / chunk.len() + 16 {
                std::io::stdout().write_all(&chunk).unwrap();
                std::io::stderr().write_all(&chunk).unwrap();
            }
            std::fs::write(dir.join("output-ready"), b"ready").unwrap();
        }
        if mode == "nonzero" {
            std::process::exit(7);
        }
        if mode == "cancel" || mode == "timeout" {
            let _ = leaf.wait();
        }
        // exit/noisy/natural return while the descendant still holds both inherited writers.
    }

    #[test]
    fn public_runner_rejects_live_descendant_after_successful_wrapper() {
        let fixture = Fixture::new();
        fixture.write_gradle_wrapper("exit");
        let root = fixture.0.clone();
        let started = Instant::now();
        let worker = std::thread::spawn(move || run_gradle_compile(&root, DEFAULT_COMPILE_TASK));
        // This proves the wrapper really ran: a spawn/configuration failure cannot pass vacuously.
        let leaf = fixture.hold_leaf();
        let outcome = worker.join().unwrap();
        eprintln!(
            "public runner returned after {:?}: {outcome:?}",
            started.elapsed()
        );
        assert_eq!(
            unsafe { WaitForSingleObject(leaf.as_raw_handle(), 0) },
            WAIT_OBJECT_0
        );
        assert_eq!(outcome.diagnostics.len(), 1);
        assert_eq!(
            outcome.diagnostics[0].message,
            "fixture retained diagnostic"
        );
        assert!(
            started.elapsed() < Duration::from_secs(6),
            "silent pipe holder delayed public return after {:?}: {outcome:?}",
            started.elapsed()
        );
        assert!(
            !outcome.executed,
            "forced cleanup must not clear previously valid diagnostics"
        );
    }

    #[test]
    fn classpath_accepts_large_complete_dump_but_rejects_oversized_model() {
        let fixture = Fixture::new();
        fixture.write_gradle_wrapper("large-classpath");
        let model = crate::classpath::load_snapshot(&fixture.0, true).unwrap();
        assert_eq!(model.modules.len(), 2);
        assert_eq!(model.modules[0].module, ":head");
        assert_eq!(model.modules[0].entries.len(), 11038);
        let entry = PathBuf::from(format!("C:/classpath/{}.jar", "x".repeat(1024)));
        assert!(model.modules[0].entries.iter().all(|found| found == &entry));
        assert_eq!(model.modules[1].module, ":tail");
        assert_eq!(
            model.modules[1].entries,
            vec![PathBuf::from("C:/classpath/tail.jar")]
        );
        fixture.write_gradle_wrapper("oversized-classpath");
        let error = crate::classpath::load_snapshot(&fixture.0, true).unwrap_err();
        assert!(error.to_string().contains("interrupted or incomplete"));
    }

    #[test]
    fn native_normal_exit_preserves_both_streams_and_genuine_compiler_error() {
        for mode in ["plain", "error", "natural"] {
            let fixture = Fixture::new();
            let mut command = fixture_command(&fixture.0, mode);
            command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
            let mut owned = OwnedNativeCompile::spawn(&mut command, MAX_OUTPUT_BYTES).unwrap();
            let leaf = if mode == "natural" {
                let leaf = fixture.hold_leaf();
                assert!(owned.tree.contains(leaf.as_raw_handle()).unwrap());
                let started = Instant::now();
                while !owned.tree.root_has_exited().unwrap() {
                    assert!(started.elapsed() < Duration::from_secs(5));
                    std::thread::sleep(Duration::from_millis(10));
                }
                // Root is known terminal while leaf is still alive. Its own normal exit follows.
                std::fs::write(fixture.0.join("leaf.exit"), b"exit naturally").unwrap();
                Some(leaf)
            } else {
                None
            };
            let (status, interrupted, mut out, out_complete, err, err_complete) =
                owned.wait(Duration::from_secs(5), None);
            assert!(!interrupted && out_complete && err_complete, "{mode}");
            out.push('\n');
            out.push_str(&err);
            let outcome = parse_completed_output(
                &out,
                DEFAULT_COMPILE_TASK,
                interrupted,
                out_complete && err_complete,
            );
            assert!(outcome.executed, "{mode}");
            assert!(err.contains("fixture retained diagnostic"));
            assert_eq!(status.success(), mode != "error");
            if mode == "error" {
                assert_eq!(status.code(), Some(7));
                assert!(outcome
                    .diagnostics
                    .iter()
                    .any(|d| d.severity == Severity::Error));
            }
            if let Some(leaf) = leaf {
                assert_eq!(
                    unsafe { WaitForSingleObject(leaf.as_raw_handle(), 0) },
                    WAIT_OBJECT_0
                );
            }
        }
    }

    #[test]
    fn native_wrapper_exit_cancel_timeout_and_output_cap_keep_the_whole_tree_owned() {
        for mode in ["exit", "cancel", "timeout", "nonzero", "noisy"] {
            let fixture = Fixture::new();
            let owned =
                OwnedNativeCompile::spawn(&mut fixture_command(&fixture.0, mode), MAX_OUTPUT_BYTES)
                    .unwrap();
            let leaf = fixture.hold_leaf();
            assert!(owned.tree.contains(leaf.as_raw_handle()).unwrap());
            if mode == "noisy" {
                await_file(&fixture.0, "output-ready");
            }
            let cancel = std::sync::atomic::AtomicBool::new(mode == "cancel");
            let timeout = if mode == "timeout" {
                Duration::from_millis(50)
            } else {
                Duration::from_secs(5)
            };
            let started = Instant::now();
            let (status, interrupted, out, out_complete, err, err_complete) =
                owned.wait(timeout, Some(&cancel));
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "{mode}: silent pipe holder stranded join"
            );
            assert_eq!(
                unsafe { WaitForSingleObject(leaf.as_raw_handle(), 0) },
                WAIT_OBJECT_0,
                "{mode}"
            );
            assert!(
                interrupted,
                "{mode}: forced tree cleanup must reject success"
            );
            assert_eq!(
                status.success(),
                mode == "exit" || mode == "noisy",
                "{mode}"
            );
            assert_eq!(out_complete && err_complete, mode != "noisy", "{mode}");
            if mode == "nonzero" {
                assert_eq!(status.code(), Some(7));
            }
            if mode == "noisy" {
                assert!(out.len() <= MAX_OUTPUT_BYTES && err.len() <= MAX_OUTPUT_BYTES);
            }
            assert!(
                !parse_completed_output(
                    &format!("{out}\n{err}"),
                    DEFAULT_COMPILE_TASK,
                    interrupted,
                    out_complete && err_complete
                )
                .executed
            );
        }
    }

    #[test]
    fn native_owner_unwind_stops_tree_and_joins_before_returning() {
        let fixture = Fixture::new();
        let owned =
            OwnedNativeCompile::spawn(&mut fixture_command(&fixture.0, "cancel"), MAX_OUTPUT_BYTES)
                .unwrap();
        let leaf = fixture.hold_leaf();
        assert!(owned.tree.contains(leaf.as_raw_handle()).unwrap());
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _owned = owned;
            panic!("fixture owner unwind");
        }));
        assert!(caught.is_err());
        assert_eq!(
            unsafe { WaitForSingleObject(leaf.as_raw_handle(), 0) },
            WAIT_OBJECT_0
        );
    }

    #[test]
    fn native_before_resume_failure_stops_exact_suspended_child() {
        use std::os::windows::io::AsHandle;
        let fixture = Fixture::new();
        let mut held = None;
        let result = process_tree::ProcessTree::spawn_before_resume(
            &mut fixture_command(&fixture.0, "plain"),
            |child| {
                held = Some(child.as_handle().try_clone_to_owned()?);
                Err(std::io::Error::other("injected before-resume failure"))
            },
        );
        assert!(result.is_err());
        let held = held.expect("retained exact suspended root");
        assert_eq!(
            unsafe { WaitForSingleObject(held.as_raw_handle(), 0) },
            WAIT_OBJECT_0
        );
        assert!(
            !fixture.0.join("fixture-started").exists(),
            "unassigned/unverified code must never run"
        );
    }
}

#[cfg(test)]
mod completed_output_tests {
    use super::*;

    #[test]
    fn interrupted_or_partial_task_output_preserves_diagnostics_without_clearing() {
        let text = "> Task :compileKotlin\ne: file:///C:/owned/A.kt:2:3 real diagnostic\n";
        let parsed = parse_output(text, DEFAULT_COMPILE_TASK);
        assert!(parsed.executed);
        for (interrupted, complete) in [(false, true), (true, true), (false, false), (true, false)]
        {
            let result = parse_completed_output(text, DEFAULT_COMPILE_TASK, interrupted, complete);
            assert_eq!(result.executed, !interrupted && complete);
            assert_eq!(result.diagnostics, parsed.diagnostics);
        }
    }

    #[test]
    fn capped_reader_drains_tail_and_reports_read_errors() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        struct Counted(std::io::Cursor<Vec<u8>>, Arc<AtomicUsize>);
        impl Read for Counted {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                let n = self.0.read(buffer)?;
                self.1.fetch_add(n, Ordering::Relaxed);
                Ok(n)
            }
        }
        let bytes = vec![b'x'; MAX_OUTPUT_BYTES + 8192];
        let count = Arc::new(AtomicUsize::new(0));
        let reader = Counted(std::io::Cursor::new(bytes), count.clone());
        let read = drain_capped(reader, MAX_OUTPUT_BYTES)
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(read.0.len(), MAX_OUTPUT_BYTES);
        assert!(!read.1);
        assert_eq!(count.load(Ordering::Relaxed), MAX_OUTPUT_BYTES + 8192);
        struct FailsAfterPrefix(bool);
        impl Read for FailsAfterPrefix {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if self.0 {
                    return Err(std::io::Error::other("fixture read error"));
                }
                self.0 = true;
                buffer[..4].copy_from_slice(b"kept");
                Ok(4)
            }
        }
        let read = drain_capped(FailsAfterPrefix(false), MAX_OUTPUT_BYTES)
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(read, ("kept".into(), false));
    }
}
