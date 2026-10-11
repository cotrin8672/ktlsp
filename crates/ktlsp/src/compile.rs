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

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::diagnostics::Severity;

/// The compile task the spike runs. Overridable later; module-aware routing is deferred.
pub const DEFAULT_COMPILE_TASK: &str = "compileKotlin";

/// Execution deadline; verified OS cleanup/EOF failures retain the same worker in quarantine.
/// This does not promise bounded shutdown when process termination itself fails.
pub(crate) const COMPILE_TIMEOUT: Duration = Duration::from_secs(180);

/// Maximum retained diagnostic bytes per stream; classpath stdout is spooled to disk.
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
    run_gradle_compile_native(root, task, &[], None).outcome
}

#[derive(Default)]
struct NativeCompile {
    outcome: CompileOutcome,
    successful: bool,
}

fn run_gradle_compile_native(
    root: &Path,
    task: &str,
    extra: &[String],
    cancelled: Option<&std::sync::atomic::AtomicBool>,
) -> NativeCompile {
    if cancelled.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire)) {
        return NativeCompile::default();
    }
    if !crate::deps::is_gradle_project(root) {
        return NativeCompile::default();
    }
    let program = match resolve_gradle(root) {
        Some(p) => p,
        None => {
            tracing::warn!(
                "no gradle wrapper or `gradle` on PATH for {}",
                root.display()
            );
            return NativeCompile::default();
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
        .args(extra)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    let result = OwnedNativeCompile::spawn(&mut command, MAX_OUTPUT_BYTES)
        .map(|owned| owned.wait(COMPILE_TIMEOUT, cancelled));
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
            return NativeCompile::default();
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
    NativeCompile {
        outcome,
        // The legacy Unix public runner still returns diagnostics; it cannot mint a receipt.
        successful: cfg!(windows) && status.success() && !interrupted && complete,
    }
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
        Self::spawn_with_stdout(command, |pipe| drain_capped(pipe, stdout_limit))
    }

    fn spawn_to_file(command: &mut Command, stdout: std::fs::File) -> std::io::Result<Self> {
        Self::spawn_with_stdout(command, |pipe| drain_to_file(pipe, stdout))
    }

    fn spawn_with_stdout(
        command: &mut Command,
        capture: impl FnOnce(
            std::process::ChildStdout,
        ) -> std::io::Result<std::thread::JoinHandle<(String, bool)>>,
    ) -> std::io::Result<Self> {
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
            owned.out = Some(capture(pipe)?);
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

/// File-backed stdout uses the same ownership/EOF rules; its returned string contains IO failures.
pub(crate) fn run_with_stdout_file(
    command: &mut Command,
    stdout: std::fs::File,
) -> std::io::Result<(std::process::ExitStatus, bool, String, bool, String, bool)> {
    #[cfg(windows)]
    {
        Ok(OwnedNativeCompile::spawn_to_file(command, stdout)?.wait(COMPILE_TIMEOUT, None))
    }
    #[cfg(not(windows))]
    {
        // shortcut: Unix retains wrapper-only ownership, add process-group cleanup after native proof.
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let mut out = None;
        let mut err = None;
        let readers = (|| -> std::io::Result<()> {
            out = child
                .stdout
                .take()
                .map(|pipe| drain_to_file(pipe, stdout))
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
    }
}

/// A failed sink must still drain the pipe, so its producer cannot block before owned cleanup.
fn drain_to_file<R: Read + Send + 'static, W: Write + Send + 'static>(
    mut reader: R,
    writer: W,
) -> std::io::Result<std::thread::JoinHandle<(String, bool)>> {
    std::thread::Builder::new()
        .name("ktlsp-compile-spool".into())
        .spawn(move || {
            let mut writer = std::io::BufWriter::new(writer);
            let mut failure = String::new();
            let mut chunk = [0; 65536];
            loop {
                let n = match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        failure.push_str(&format!("stdout read: {error}; "));
                        break;
                    }
                };
                if failure.is_empty() {
                    if let Err(error) = writer.write_all(&chunk[..n]) {
                        failure.push_str(&format!("stdout write: {error}; "));
                    }
                }
            }
            if let Err(error) = writer.flush() {
                failure.push_str(&format!("stdout flush: {error}; "));
            }
            let complete = failure.is_empty();
            (failure, complete)
        })
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
    let matches = if task.starts_with(':') {
        task_path == task
    } else {
        task_path.rsplit(':').next()? == task
    };
    matches.then_some(status)
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

    // Job termination is asynchronous; verify the retained descendant handle actually signals.
    const PROCESS_EXIT_WAIT_MS: u32 = 5000;

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
            assert!([
                "exit",
                "large-classpath",
                "oversized-classpath",
                "truncated-classpath",
                "failed-classpath",
                "stderr-overflow-classpath",
            ]
            .contains(&mode));
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
        if mode.ends_with("-classpath") {
            let size: usize = match mode.as_str() {
                "large-classpath" => 11 * 1024 * 1024,
                "oversized-classpath" => 65 * 1024 * 1024,
                _ => 1024,
            };
            let mut output = std::io::BufWriter::new(std::io::stdout().lock());
            writeln!(output, "\nPROJECT\t:head\t{}", dir.display()).unwrap();
            let entry = format!("CP\tC:/classpath/{}.jar\n", "x".repeat(1024));
            for _ in 0..size.div_ceil(entry.len()) {
                output.write_all(entry.as_bytes()).unwrap();
            }
            writeln!(output, "END\t:head").unwrap();
            writeln!(output, "PROJECT\t:tail\t{}", dir.display()).unwrap();
            writeln!(output, "CP\tC:/classpath/tail.jar").unwrap();
            if mode != "truncated-classpath" {
                writeln!(output, "END\t:tail").unwrap();
            }
            output.flush().unwrap();
            if mode == "failed-classpath" {
                std::process::exit(7);
            }
            if mode == "stderr-overflow-classpath" {
                let chunk = [b'x'; 8192];
                for _ in 0..MAX_OUTPUT_BYTES / chunk.len() + 1 {
                    std::io::stderr().write_all(&chunk).unwrap();
                }
                std::io::stderr().flush().unwrap();
            }
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
            unsafe { WaitForSingleObject(leaf.as_raw_handle(), PROCESS_EXIT_WAIT_MS) },
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
    fn classpath_accepts_complete_dump_above_old_memory_cap_with_tail_module() {
        let fixture = Fixture::new();
        let entry = PathBuf::from(format!("C:/classpath/{}.jar", "x".repeat(1024)));
        for (mode, expected) in [("large-classpath", 11038), ("oversized-classpath", 65223)] {
            fixture.write_gradle_wrapper(mode);
            let model = crate::classpath::load_snapshot(&fixture.0, true).unwrap();
            assert_eq!(model.modules.len(), 2);
            assert_eq!(model.modules[0].module, ":head");
            assert_eq!(model.modules[0].entries.len(), expected);
            assert!(model.modules[0].entries.iter().all(|found| found == &entry));
            assert_eq!(model.modules[1].module, ":tail");
            assert_eq!(
                model.modules[1].entries,
                vec![PathBuf::from("C:/classpath/tail.jar")]
            );
        }
    }

    #[test]
    fn classpath_rejects_partial_blocks_failed_native_exit_and_stderr_overflow() {
        for (mode, reason) in [
            ("truncated-classpath", "unfinished PROJECT :tail"),
            ("failed-classpath", "classpath dump failed"),
            ("stderr-overflow-classpath", "stderr capture failed"),
        ] {
            let fixture = Fixture::new();
            fixture.write_gradle_wrapper(mode);
            let error = crate::classpath::load_snapshot(&fixture.0, true).unwrap_err();
            assert!(error.to_string().contains(reason), "{mode}: {error}");
            assert!(fixture.0.join("fixture-started").is_file());
            assert!(crate::classpath::snapshot(&fixture.0).is_none());
        }
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
                    unsafe { WaitForSingleObject(leaf.as_raw_handle(), PROCESS_EXIT_WAIT_MS) },
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
                unsafe { WaitForSingleObject(leaf.as_raw_handle(), PROCESS_EXIT_WAIT_MS) },
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
            unsafe { WaitForSingleObject(leaf.as_raw_handle(), PROCESS_EXIT_WAIT_MS) },
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

    #[test]
    fn spooled_reader_preserves_bytes_and_drains_after_write_or_flush_failure() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Mutex,
        };
        struct Counted {
            bytes: std::io::Cursor<Vec<u8>>,
            count: Arc<AtomicUsize>,
            interrupted: bool,
        }
        impl Read for Counted {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if !self.interrupted {
                    self.interrupted = true;
                    return Err(std::io::ErrorKind::Interrupted.into());
                }
                let n = self.bytes.read(buffer)?;
                self.count.fetch_add(n, Ordering::Relaxed);
                Ok(n)
            }
        }
        struct Sink {
            bytes: Arc<Mutex<Vec<u8>>>,
            fail_write: bool,
            fail_flush: bool,
        }
        impl Write for Sink {
            fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
                if self.fail_write {
                    return Err(std::io::Error::other("fixture write failure"));
                }
                self.bytes.lock().unwrap().extend_from_slice(buffer);
                Ok(buffer.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                if self.fail_flush {
                    Err(std::io::Error::other("fixture flush failure"))
                } else {
                    Ok(())
                }
            }
        }
        let mut expected = vec![b'x'; 150_000];
        expected.extend_from_slice(b"\xff\0tail\n");
        for (fail_write, fail_flush, reason) in [
            (false, false, ""),
            (true, false, "stdout write: fixture write failure"),
            (false, true, "stdout flush: fixture flush failure"),
        ] {
            let bytes = Arc::new(Mutex::new(Vec::new()));
            let count = Arc::new(AtomicUsize::new(0));
            let result = drain_to_file(
                Counted {
                    bytes: std::io::Cursor::new(expected.clone()),
                    count: count.clone(),
                    interrupted: false,
                },
                Sink {
                    bytes: bytes.clone(),
                    fail_write,
                    fail_flush,
                },
            )
            .unwrap()
            .join()
            .unwrap();
            assert_eq!(result.1, !fail_write && !fail_flush);
            if reason.is_empty() {
                assert!(result.0.is_empty());
            } else {
                assert!(result.0.contains(reason), "{result:?}");
            }
            assert_eq!(count.load(Ordering::Relaxed), expected.len());
            if !fail_write {
                assert_eq!(*bytes.lock().unwrap(), expected);
            }
        }
    }

    #[test]
    fn spooled_reader_reports_read_failure() {
        struct FailsAfterPrefix(bool);
        impl Read for FailsAfterPrefix {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if self.0 {
                    return Err(std::io::Error::other("fixture read failure"));
                }
                self.0 = true;
                buffer[..4].copy_from_slice(b"kept");
                Ok(4)
            }
        }
        let result = drain_to_file(FailsAfterPrefix(false), std::io::sink())
            .unwrap()
            .join()
            .unwrap();
        assert!(!result.1);
        assert!(result.0.contains("stdout read: fixture read failure"));
    }
}

// Private observed-build path. CompileOutcome and the sidecar wire request are unchanged.
pub(crate) struct CompileOwner {
    pub definition: crate::symbol::Def,
    pub qualified_name: String,
    pub origin_id: String,
}

pub(crate) struct PreparedCompilePaths {
    output_roots: Vec<String>,
    watch_paths: Vec<String>,
    environment_sha256: String,
}

pub(crate) struct ObservedCompileRequest {
    pub root: PathBuf,
    pub project_dir: PathBuf,
    pub scope: crate::project_model::ProjectScope,
    pub source_set: String,
    pub task: crate::classpath::CompilerTask,
    pub compiler_tasks: Vec<crate::classpath::CompilerTask>,
    pub project_path: String,
    pub source_roots: Vec<String>,
    pub classpath: Vec<String>,
    pub cache_dir: PathBuf,
    pub owners: Vec<CompileOwner>,
    pub nonce: String,
    pub invalid: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub prepared_paths: Option<PreparedCompilePaths>,
}

pub(crate) struct CompiledOwnerInput {
    pub file: String,
    pub class_file: PathBuf,
    pub class_file_bytes: u64,
    pub text: std::sync::Arc<str>,
    pub tree: tree_sitter::Tree,
    pub class: std::sync::Arc<ktcore::classfile::ClassFileFacts>,
}

/// An observed positive snapshot, not a certificate of arbitrary AP dependency closure. Only
/// run_observed_compile constructs it after native/task/input/output verification. Never persist
/// it as a disk cache or promote compiler_producers_complete from its existence.
pub(crate) struct SuccessfulCompileReceipt {
    pub request: std::sync::Arc<ObservedCompileRequest>,
    inputs: Vec<serde_json::Value>,
    outputs: Vec<serde_json::Value>,
    environment_sha256: String,
}

pub(crate) struct ObservedCompileResult {
    pub outcome: CompileOutcome,
    pub receipt: Option<std::sync::Arc<SuccessfulCompileReceipt>>,
    pub owners: Vec<CompiledOwnerInput>,
    pub reason: String,
}

impl ObservedCompileRequest {
    pub fn check_live(&self) -> anyhow::Result<()> {
        use std::sync::atomic::Ordering;
        anyhow::ensure!(
            !self.invalid.load(Ordering::Acquire),
            "compiler-input-invalidated"
        );
        anyhow::ensure!(
            !self.cancelled.load(Ordering::Acquire),
            "compiler-job-cancelled"
        );
        Ok(())
    }

    pub fn is_known_output(&self, key: &str) -> bool {
        self.checked_output_roots()
            .iter()
            .any(|root| path_under(key, root))
    }

    fn checked_output_roots(&self) -> std::borrow::Cow<'_, [String]> {
        if let Some(paths) = &self.prepared_paths {
            return std::borrow::Cow::Borrowed(&paths.output_roots);
        }
        // Unpublished/test requests still require current role validation before deferral.
        std::borrow::Cow::Owned(if self.validate_roles().is_ok() {
            self.output_roots()
        } else {
            Vec::new()
        })
    }

    fn output_roots(&self) -> Vec<String> {
        self.compiler_tasks
            .iter()
            .flat_map(|task| &task.declared_output_roots)
            .map(|root| crate::paths::file_key(root))
            .collect()
    }

    pub fn prepare_paths(&mut self) -> anyhow::Result<()> {
        // Seal the input plan while exclusively owned, before publishing the request Arc.
        self.prepared_paths = None;
        self.check_live()?;
        self.validate_roles()?;
        let environment_sha256 = environment_hash();
        let output_roots = self.output_roots();
        let watch_paths = self.watch_paths_with_outputs(&output_roots);
        anyhow::ensure!(
            environment_sha256 == environment_hash(),
            "compiler environment changed while preparing paths"
        );
        self.check_live()?;
        self.prepared_paths = Some(PreparedCompilePaths {
            output_roots,
            watch_paths,
            environment_sha256,
        });
        Ok(())
    }

    fn verify_prepared_environment(&self, current: &str) -> anyhow::Result<()> {
        let paths = self
            .prepared_paths
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("compiler input paths were not prepared"))?;
        anyhow::ensure!(
            paths.environment_sha256 == current,
            "compiler environment changed after path preparation"
        );
        Ok(())
    }

    pub fn validate_roles(&self) -> anyhow::Result<()> {
        use crate::classpath::CompilerLanguage;
        anyhow::ensure!(
            self.task.language == CompilerLanguage::Java
                && self.task.complete
                && !self.task.source_files.is_empty()
                && !self.task.output_roots.is_empty()
                && !self.task.declared_output_roots.is_empty(),
            "selected Java input/output metadata unavailable"
        );
        let mut tasks = std::collections::BTreeSet::new();
        for task in &self.compiler_tasks {
            anyhow::ensure!(
                task.complete && task.problem.is_none() && tasks.insert(&task.task),
                "ambiguous compiler task metadata"
            );
        }
        anyhow::ensure!(
            self.compiler_tasks
                .iter()
                .filter(|task| **task == self.task)
                .count()
                == 1,
            "selected compiler task metadata mismatch"
        );
        anyhow::ensure!(
            self.compiler_tasks.iter().all(|task| task
                .source_files
                .iter()
                .chain(&task.classpath)
                .chain(&task.declared_input_files)
                .chain(&task.declared_output_roots)
                .chain(&task.annotation_processor_path)
                .chain(&task.output_roots)
                .all(|path| path.is_absolute())),
            "relative compiler resource path"
        );
        anyhow::ensure!(
            self.source_roots
                .iter()
                .chain(&self.classpath)
                .all(|path| Path::new(path).is_absolute())
                && self.compiler_tasks.iter().all(|task| task
                    .java_home
                    .as_ref()
                    .is_none_or(|home| home.is_absolute())
                    && task
                        .arguments
                        .iter()
                        .filter_map(|argument| argument.strip_prefix("-Xplugin="))
                        .flat_map(|plugins| plugins.split(','))
                        .all(|path| Path::new(path).is_absolute())),
            "relative configured compiler resource path"
        );
        let own_outputs: Vec<_> = self
            .task
            .declared_output_roots
            .iter()
            .map(|p| crate::paths::file_key(p))
            .collect();
        let overlaps = |path: &Path| {
            let key = crate::paths::file_key(path);
            own_outputs.iter().any(|output| paths_overlap(&key, output))
        };
        for output in &self.task.output_roots {
            let key = crate::paths::file_key(output);
            anyhow::ensure!(
                own_outputs.iter().any(|root| path_under(&key, root)),
                "class destination is not a declared task output"
            );
        }
        let processor_inputs = || {
            self.compiler_tasks
                .iter()
                .flat_map(|task| task.annotation_processor_path.iter().cloned())
                .chain(
                    self.compiler_tasks
                        .iter()
                        .flat_map(|task| task.arguments.iter())
                        .filter_map(|argument| argument.strip_prefix("-Xplugin="))
                        .flat_map(|plugins| plugins.split(',').map(PathBuf::from)),
                )
        };
        for input in self
            .task
            .source_files
            .iter()
            .cloned()
            .chain(self.task.classpath.iter().cloned())
            .chain(
                self.compiler_tasks
                    .iter()
                    .flat_map(|task| task.declared_input_files.iter().cloned()),
            )
            .chain(processor_inputs())
        {
            anyhow::ensure!(
                !overlaps(&input),
                "compiler input overlaps selected own output"
            );
        }
        // shortcut: reject reads of deferred outputs; admit them after phase-aware read/ABA proof.
        // Endpoint hashes cannot detect a binary A->B->A during a selected action. Registered
        // source roots remain coverage only; actual modeled read roles must stay native-protected.
        for input in self
            .compiler_tasks
            .iter()
            .flat_map(|task| {
                task.source_files
                    .iter()
                    .chain(&task.classpath)
                    .chain(&task.declared_input_files)
                    .cloned()
            })
            .chain(processor_inputs())
        {
            let key = crate::paths::file_key(&input);
            anyhow::ensure!(
                !self
                    .compiler_tasks
                    .iter()
                    .flat_map(|task| &task.declared_output_roots)
                    .any(|output| paths_overlap(&key, &crate::paths::file_key(output))),
                "compiler input overlaps deferred output without temporal read proof"
            );
        }
        for (i, task) in self.compiler_tasks.iter().enumerate() {
            for other in &self.compiler_tasks[i + 1..] {
                anyhow::ensure!(
                    !task.declared_output_roots.iter().any(|a| other
                        .declared_output_roots
                        .iter()
                        .any(|b| paths_overlap(
                            &crate::paths::file_key(a),
                            &crate::paths::file_key(b)
                        ))),
                    "ambiguous compiler output ownership"
                );
            }
        }
        Ok(())
    }

    fn planned_reads(&self) -> Vec<PathBuf> {
        self.planned_reads_with_outputs(&self.checked_output_roots())
    }

    fn planned_reads_with_outputs(&self, outputs: &[String]) -> Vec<PathBuf> {
        let is_output = |key: &str| outputs.iter().any(|root| path_under(key, root));
        let mut paths: std::collections::BTreeSet<_> = self
            .source_roots
            .iter()
            .filter(|root| !is_output(root))
            .map(PathBuf::from)
            .collect();
        for directory in [&self.root, &self.project_dir] {
            for name in [
                "build.gradle",
                "build.gradle.kts",
                "settings.gradle",
                "settings.gradle.kts",
                "gradle.properties",
                "gradlew",
                "gradlew.bat",
                "gradle/wrapper/gradle-wrapper.properties",
                "gradle/wrapper/gradle-wrapper.jar",
                "gradle/libs.versions.toml",
            ] {
                paths.insert(directory.join(name));
            }
        }
        if let Some(home) = std::env::var_os("GRADLE_USER_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join(".gradle")))
        {
            paths.extend([
                home.join("gradle.properties"),
                home.join("init.gradle"),
                home.join("init.gradle.kts"),
                home.join("init.d"),
            ]);
        }
        for task in &self.compiler_tasks {
            paths.extend(task.annotation_processor_path.iter().cloned());
            for input in &task.declared_input_files {
                let key = crate::paths::file_key(input);
                if !is_output(&key) {
                    paths.insert(input.clone());
                }
            }
            for argument in &task.arguments {
                if let Some(plugins) = argument.strip_prefix("-Xplugin=") {
                    paths.extend(plugins.split(',').map(PathBuf::from));
                }
            }
            if let Some(home) = &task.java_home {
                for name in [
                    "release",
                    "bin/java",
                    "bin/java.exe",
                    "bin/javac",
                    "bin/javac.exe",
                    "lib/modules",
                ] {
                    paths.insert(home.join(name));
                }
            }
        }
        for entry in &self.classpath {
            if !is_output(entry) {
                paths.insert(PathBuf::from(entry));
            }
        }
        for owner in &self.owners {
            let mut directory = Path::new(&owner.definition.file).parent();
            while let Some(parent) = directory {
                paths.insert(parent.join("lombok.config"));
                directory = parent.parent();
            }
        }
        paths.into_iter().collect()
    }

    pub fn initial_watch_paths(&self) -> std::borrow::Cow<'_, [String]> {
        if let Some(paths) = &self.prepared_paths {
            return std::borrow::Cow::Borrowed(&paths.watch_paths);
        }
        std::borrow::Cow::Owned(self.watch_paths_with_outputs(&self.checked_output_roots()))
    }

    fn watch_paths_with_outputs(&self, outputs: &[String]) -> Vec<String> {
        let mut paths: Vec<_> = self
            .planned_reads_with_outputs(outputs)
            .iter()
            .map(|p| crate::paths::file_key(p))
            .collect();
        paths.extend(
            self.compiler_tasks
                .iter()
                .flat_map(|task| &task.declared_output_roots)
                .map(|p| crate::paths::file_key(p)),
        );
        paths.sort();
        paths.dedup();
        paths
    }

    pub fn is_declared_directory(&self, key: &str) -> bool {
        self.source_roots.iter().any(|root| root == key)
            || self
                .compiler_tasks
                .iter()
                .flat_map(|task| &task.output_roots)
                .any(|root| crate::paths::file_key(root) == key)
    }

    pub fn observes_path(&self, key: &str) -> bool {
        self.initial_watch_paths()
            .iter()
            .any(|path| path_under(key, path))
    }
}

impl SuccessfulCompileReceipt {
    pub fn observes_path(&self, key: &str) -> bool {
        self.request.observes_path(key)
            || self.inputs.iter().chain(&self.outputs).any(|record| {
                record["path"]
                    .as_str()
                    .is_some_and(|path| path_under(key, path))
            })
    }

    pub fn covers_output(&self, key: &str) -> bool {
        self.outputs.iter().any(|record| {
            record["path"]
                .as_str()
                .is_some_and(|path| path_under(key, path))
        })
    }

    pub fn source_matches(&self, key: &str, text: &str) -> bool {
        use sha2::{Digest, Sha256};
        let sha = format!("{:x}", Sha256::digest(text.as_bytes()));
        self.inputs
            .iter()
            .flat_map(|record| {
                std::iter::once(record).chain(record["entries"].as_array().into_iter().flatten())
            })
            .any(|record| {
                record["path"].as_str() == Some(key)
                    && record["sha256"].as_str() == Some(sha.as_str())
            })
    }

    pub fn verify_current(&self) -> anyhow::Result<()> {
        self.request.check_live()?;
        anyhow::ensure!(
            environment_hash() == self.environment_sha256,
            "compiler environment changed"
        );
        for input in self.inputs.iter().chain(&self.outputs) {
            let path = input
                .get("path")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("receipt inventory has no path"))?;
            anyhow::ensure!(
                inventory(Path::new(path))? == *input,
                "receipt input/output changed: {path}"
            );
        }
        self.request.check_live()?;
        Ok(())
    }

    /// Bind the exact decoded bytes to the selected Java task's captured file, not a
    /// later disk hash or an upstream class with the same internal name.
    pub(crate) fn verify_compiled_output(
        &self,
        class_file: &Path,
        byte_len: u64,
        class: &ktcore::classfile::ClassFileFacts,
    ) -> anyhow::Result<()> {
        self.request.check_live()?;
        anyhow::ensure!(
            byte_len <= ktcore::classfile::MAX_CLASS_FILE_BYTES as u64,
            "compiled output byte cap exceeded"
        );
        let ktcore::jvm::InputIdentity::Binary { content_sha256 } = &class.identity.input else {
            anyhow::bail!("compiled output has no binary byte identity");
        };
        let normalized = |path: &Path| -> anyhow::Result<String> {
            anyhow::ensure!(path.is_absolute(), "compiled output path is relative");
            let value = crate::paths::file_key(&std::path::absolute(path)?);
            Ok(if cfg!(windows) {
                value.to_lowercase()
            } else {
                value
            })
        };
        let file_key = normalized(class_file)?;
        let relative = format!("{}.class", class.identity.internal_name);
        let mut selected = false;
        for root in &self.request.task.output_roots {
            if path_under(&file_key, &normalized(root)?)
                && normalized(&root.join(&relative))? == file_key
            {
                selected = true;
            }
        }
        anyhow::ensure!(
            selected,
            "compiled output is not the exact selected Java owner path"
        );
        let sha: String = content_sha256
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let mut found = false;
        for record in self.outputs.iter().flat_map(|record| {
            std::iter::once(record).chain(record["entries"].as_array().into_iter().flatten())
        }) {
            let Some(path) = record["path"].as_str() else {
                continue;
            };
            if normalized(Path::new(path))? != file_key {
                continue;
            }
            anyhow::ensure!(
                record["kind"].as_str() == Some("file")
                    && record["bytes"].as_u64() == Some(byte_len)
                    && record["sha256"].as_str() == Some(sha.as_str()),
                "compiled output bytes differ from the successful selected-task receipt"
            );
            // Identical duplicate inventories are harmless; any conflicting one rejects above.
            found = true;
        }
        anyhow::ensure!(
            found,
            "compiled output file missing from the selected-task receipt"
        );
        self.request.check_live()
    }

    pub fn watch_paths(&self) -> Vec<String> {
        let mut paths = self.request.initial_watch_paths().into_owned();
        paths.extend(self.inputs.iter().chain(&self.outputs).filter_map(|value| {
            value
                .get("path")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        }));
        paths.sort();
        paths.dedup();
        paths
    }
}

pub(crate) fn run_observed_compile(
    request: std::sync::Arc<ObservedCompileRequest>,
    prearmed: &crate::compiled_watch::WatchPlan,
) -> ObservedCompileResult {
    let mut outcome = CompileOutcome::default();
    let result = (|| -> anyhow::Result<_> {
        use serde_json::Value;
        use std::sync::Arc;
        request.check_live()?;
        request.validate_roles()?;
        request.verify_prepared_environment(&environment_hash())?;
        // shortcut: native tree completion is verified on Windows; admit other hosts after equivalent native tests.
        anyhow::ensure!(
            cfg!(windows),
            "native tree completion proof unavailable on this host"
        );
        let directory = request.cache_dir.clone();
        // Never consume an earlier invocation's observer files or a partial stale output receipt.
        std::fs::create_dir_all(
            directory
                .parent()
                .ok_or_else(|| anyhow::anyhow!("missing cache parent"))?,
        )?;
        std::fs::create_dir(&directory)?;
        let events_directory = directory.join("events");
        std::fs::create_dir(&events_directory)?;
        let script = directory.join("observer.init.gradle.kts");
        std::fs::write(
            &script,
            include_bytes!("../../../scripts/classpath-dump.init.gradle.kts"),
        )?;
        let planned_reads = request.planned_reads();
        anyhow::ensure!(planned_reads.len() <= 4096, "host input path cap exceeded");
        let mut before = Vec::new();
        let mut inventory_budget = MAX_RECEIPT_INVENTORY_ENTRIES;
        for path in &planned_reads {
            let record = inventory(path)?;
            let records = 1 + record["entries"].as_array().map_or(0, Vec::len);
            inventory_budget = inventory_budget
                .checked_sub(records)
                .ok_or_else(|| anyhow::anyhow!("aggregate host inventory cap exceeded"))?;
            before.push(record);
        }
        let environment_sha256 = environment_hash();
        request.verify_prepared_environment(&environment_sha256)?;
        let extra = vec![
            "--rerun-tasks".to_string(),
            "--no-build-cache".into(),
            "--no-configuration-cache".into(),
            "--max-workers=1".into(),
            "--init-script".into(),
            script.to_string_lossy().into_owned(),
            format!("-Dktlsp.observer.dir={}", events_directory.display()),
            format!("-Dktlsp.observer.root={}", request.root.display()),
            format!("-Dktlsp.observer.task={}", request.task.task),
            format!("-Dktlsp.observer.nonce={}", request.nonce),
        ];
        let native = run_gradle_compile_native(
            &request.root,
            &request.task.task,
            &extra,
            Some(&request.cancelled),
        );
        outcome = native.outcome;
        anyhow::ensure!(
            native.successful,
            "native compile failed, timed out or logs were incomplete"
        );
        let mut files = Vec::new();
        for file in std::fs::read_dir(&events_directory)? {
            anyhow::ensure!(files.len() < 4096, "observer event cap exceeded");
            files.push(file?);
        }
        files.sort_by_key(|file| file.file_name());
        let mut events = Vec::new();
        let mut bytes_total = 0;
        for file in files {
            anyhow::ensure!(
                file.metadata()?.len() <= 16 * 1024 * 1024,
                "observer event byte cap exceeded"
            );
            let mut bytes = Vec::new();
            std::fs::File::open(file.path())?
                .take(16 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            anyhow::ensure!(
                bytes.len() <= 16 * 1024 * 1024,
                "observer event byte cap exceeded"
            );
            bytes_total += bytes.len();
            anyhow::ensure!(
                bytes_total <= 64 * 1024 * 1024,
                "observer byte cap exceeded"
            );
            let event: Value = serde_json::from_slice(&bytes)?;
            anyhow::ensure!(
                event["nonce"].as_str() == Some(request.nonce.as_str())
                    && event["sequence"].as_u64() == Some(events.len() as u64),
                "foreign/gapped observer event"
            );
            events.push(event);
        }
        let one = |name: &str| -> anyhow::Result<&Value> {
            let matched: Vec<_> = events
                .iter()
                .filter(|event| event["event"] == name)
                .collect();
            anyhow::ensure!(matched.len() == 1, "missing/duplicate {name}");
            Ok(matched[0])
        };
        let graph = one("graph")?;
        let start = one("selected-action-start")?;
        let action_end = one("selected-action-end")?;
        let finished = one("build-finished")?;
        let selected_ends: Vec<_> = events
            .iter()
            .filter(|event| {
                event["event"] == "task-end"
                    && event["task"].as_str() == Some(request.task.task.as_str())
            })
            .collect();
        anyhow::ensure!(
            selected_ends.len() == 1,
            "missing/duplicate selected task end"
        );
        let end = selected_ends[0];
        anyhow::ensure!(
            graph["selected_task"] == request.task.task
                && graph["project"] == request.project_path
                && graph["source_set"] == request.source_set
                && graph["root"]
                    .as_str()
                    .is_some_and(|root| crate::paths::file_key(Path::new(root))
                        == crate::paths::file_key(&request.root))
                && graph["project_dir"]
                    .as_str()
                    .is_some_and(|path| crate::paths::file_key(Path::new(path))
                        == crate::paths::file_key(&request.project_dir)),
            "observed task/module/build/source-set mismatch"
        );
        anyhow::ensure!(
            successful_task_state(&end["state"])
                && successful_task_state(&finished["selected_state"])
                && finished["failure"].is_null()
                && finished["selected_started"] == true,
            "selected task/whole invocation did not succeed"
        );
        anyhow::ensure!(
            start["sequence"].as_u64() < action_end["sequence"].as_u64()
                && action_end["sequence"].as_u64() < end["sequence"].as_u64()
                && end["sequence"].as_u64() < finished["sequence"].as_u64(),
            "invalid observer action order"
        );
        anyhow::ensure!(
            start["inputs"] == end["inputs_after"],
            "selected task inputs changed during action"
        );
        let inputs = &start["inputs"];
        let configuration: crate::classpath::CompilerTask =
            serde_json::from_value(inputs["configuration"].clone())?;
        anyhow::ensure!(
            configuration == request.task && configuration.complete,
            "selected configured task changed"
        );
        let configurations: Vec<crate::classpath::CompilerTask> =
            serde_json::from_value(inputs["compiler_configurations"].clone())?;
        anyhow::ensure!(
            !configurations.is_empty()
                && configurations.iter().all(|task| task.complete
                    && request
                        .compiler_tasks
                        .iter()
                        .any(|expected| expected == task)),
            "unknown upstream compiler task/options"
        );
        for compiler in &configurations {
            if compiler.task == request.task.task {
                continue;
            }
            let ends: Vec<_> = events
                .iter()
                .filter(|event| event["event"] == "task-end" && event["task"] == compiler.task)
                .collect();
            anyhow::ensure!(
                ends.len() == 1
                    && ends[0]["state"]["failure"].is_null()
                    && ends[0]["sequence"].as_u64() < start["sequence"].as_u64(),
                "upstream compiler did not finish before Java action"
            );
        }
        for prior in &before {
            let path = prior["path"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("invalid host input"))?;
            anyhow::ensure!(
                inventory(Path::new(path))? == *prior,
                "host source/config/AP/toolchain input changed"
            );
        }
        anyhow::ensure!(
            environment_sha256 == environment_hash(),
            "compiler environment changed"
        );
        // Validate categories before deduplication can erase a read/write overlap.
        validate_observed_roles(&request, inputs)?;
        validate_prearmed_inputs(prearmed, inputs)?;
        let mut reads: std::collections::BTreeMap<String, Value> = before
            .into_iter()
            .map(|value| (value["path"].as_str().unwrap().to_string(), value))
            .collect();
        for field in OBSERVED_INPUT_FIELDS {
            let records = inputs[field]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("missing readset {field}"))?;
            for record in records {
                let path = record["path"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("invalid readset path"))?;
                anyhow::ensure!(
                    inventory(Path::new(path))? == *record,
                    "observed execution-boundary input changed"
                );
                if let Some(prior) = reads.insert(path.to_string(), record.clone()) {
                    anyhow::ensure!(prior == *record, "conflicting input stamps");
                }
            }
        }
        // Upstream compiler execution-boundary inputs belong to the same receipt. Do not confuse
        // unchanged Java owner's text with unchanged Kotlin output or AP/compiler configuration.
        for compiler in &configurations {
            let starts: Vec<_> = events
                .iter()
                .filter(|event| event["event"] == "task-start" && event["task"] == compiler.task)
                .collect();
            anyhow::ensure!(
                starts.len() == 1 && starts[0]["configuration"] == serde_json::to_value(compiler)?,
                "upstream compiler input boundary missing"
            );
            for record in starts[0]["declared_inputs"]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("missing upstream inputs"))?
            {
                ensure_prearmed_read(prearmed, record)?;
                let path = record["path"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("invalid upstream input"))?;
                // An upstream compiler may consume an earlier task's output. A value that changed
                // later cannot be silently replaced by the final inventory and still be certified.
                anyhow::ensure!(
                    inventory(Path::new(path))? == *record,
                    "upstream compiler input changed"
                );
                if let Some(prior) = reads.insert(path.to_string(), record.clone()) {
                    anyhow::ensure!(prior == *record, "conflicting upstream input stamps");
                }
            }
        }
        anyhow::ensure!(
            action_end["outputs"] == end["outputs"]
                && end["outputs"] == finished["selected_outputs"],
            "selected outputs changed before build completion"
        );
        let mut outputs = finished["selected_outputs"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing outputs"))?
            .clone();
        for compiler in &configurations {
            if compiler.task == request.task.task {
                continue;
            }
            let end = events
                .iter()
                .find(|event| event["event"] == "task-end" && event["task"] == compiler.task)
                .ok_or_else(|| anyhow::anyhow!("missing upstream output boundary"))?;
            outputs.extend(
                end["outputs"]
                    .as_array()
                    .ok_or_else(|| anyhow::anyhow!("missing upstream outputs"))?
                    .clone(),
            );
        }
        let receipt = Arc::new(SuccessfulCompileReceipt {
            request: request.clone(),
            inputs: reads.into_values().collect(),
            outputs,
            environment_sha256,
        });
        receipt.verify_current()?;
        // Decode only actual selected Java task output roots. Kotlin upstream classes do not count
        // as evidence of a selected Java member, even if build/classes as a whole is non-empty.
        let java_sources: std::collections::BTreeSet<_> = inputs["java_task_sources"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|value| value["path"].as_str())
            .map(|path| crate::paths::file_key(Path::new(path)))
            .collect();
        let mut owners = Vec::new();
        let mut parser = crate::java::JavaParser::new();
        for owner in &request.owners {
            if !java_sources.contains(&owner.definition.file) {
                continue;
            }
            let internal_name = owner.qualified_name.replace('.', "/");
            let relative = PathBuf::from(format!("{internal_name}.class"));
            let classes: Vec<_> = request
                .task
                .output_roots
                .iter()
                .map(|root| root.join(&relative))
                .filter(|file| file.is_file())
                .collect();
            if classes.len() != 1 {
                continue;
            }
            let limit = ktcore::classfile::MAX_CLASS_FILE_BYTES as u64;
            let mut bytes = Vec::new();
            std::fs::File::open(&classes[0])?
                .take(limit + 1)
                .read_to_end(&mut bytes)?;
            anyhow::ensure!(bytes.len() as u64 <= limit, "classfile byte cap exceeded");
            let class = Arc::new(ktcore::classfile::decode(&owner.origin_id, &bytes)?);
            receipt.verify_compiled_output(&classes[0], bytes.len() as u64, &class)?;
            anyhow::ensure!(
                class.identity.internal_name == internal_name,
                "compiled owner mismatch"
            );
            if !class.methods.iter().any(|method| {
                method.descriptor.as_ref() == "()Ljava/lang/String;"
                    && method.access_flags & 0x0001 != 0
                    && method.access_flags & (0x0008 | 0x0040 | 0x1000) == 0
            }) {
                continue;
            }
            let text: Arc<str> = Arc::from(std::fs::read_to_string(&owner.definition.file)?);
            let tree = parser.parse(&text);
            anyhow::ensure!(
                !tree.root_node().has_error(),
                "owner source parse is incomplete"
            );
            owners.push(CompiledOwnerInput {
                file: owner.definition.file.clone(),
                class_file: classes[0].clone(),
                class_file_bytes: bytes.len() as u64,
                text,
                tree,
                class,
            });
        }
        receipt.verify_current()?;
        anyhow::ensure!(!owners.is_empty(), "no supported compiled owner");
        Ok((receipt, owners))
    })();
    match result {
        Ok((receipt, owners)) => ObservedCompileResult {
            outcome,
            receipt: Some(receipt),
            owners,
            reason: "observed-success".into(),
        },
        Err(error) => ObservedCompileResult {
            outcome,
            receipt: None,
            owners: Vec::new(),
            reason: error.to_string(),
        },
    }
}

const OBSERVED_INPUT_FIELDS: [&str; 9] = [
    "module_sources",
    "java_task_sources",
    "ordered_classpath",
    "annotation_processor_path",
    "plugin_artifacts",
    "declared_task_input_files",
    "build_configuration_files",
    "ancestor_lombok_configs",
    "compiler_runtime_files",
];

fn validate_observed_roles(
    request: &ObservedCompileRequest,
    inputs: &serde_json::Value,
) -> anyhow::Result<()> {
    let outputs: Vec<_> = request
        .compiler_tasks
        .iter()
        .flat_map(|task| &task.declared_output_roots)
        .map(|p| crate::paths::file_key(p))
        .collect();
    for field in OBSERVED_INPUT_FIELDS {
        let records = inputs[field]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing input role {field}"))?;
        for record in records {
            let path = record["path"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("invalid input role path"))?;
            anyhow::ensure!(
                Path::new(path).is_absolute(),
                "relative observed input path"
            );
            let key = crate::paths::file_key(Path::new(path));
            anyhow::ensure!(
                !outputs.iter().any(|root| paths_overlap(&key, root)),
                "observed {field} input overlaps deferred output without temporal read proof"
            );
        }
    }
    Ok(())
}

fn validate_prearmed_inputs(
    plan: &crate::compiled_watch::WatchPlan,
    inputs: &serde_json::Value,
) -> anyhow::Result<()> {
    for field in OBSERVED_INPUT_FIELDS {
        for record in inputs[field]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing readset {field}"))?
        {
            ensure_prearmed_read(plan, record)?;
        }
    }
    Ok(())
}

fn ensure_prearmed_read(
    plan: &crate::compiled_watch::WatchPlan,
    record: &serde_json::Value,
) -> anyhow::Result<()> {
    let path = record["path"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("invalid readset path"))?;
    let kind = record["kind"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("invalid readset kind"))?;
    anyhow::ensure!(
        Path::new(path).is_absolute() && matches!(kind, "file" | "directory" | "absent"),
        "unsupported observed read record"
    );
    // Extending watches after a build cannot establish what changed during its action.
    anyhow::ensure!(
        plan.covers_read(path, kind == "directory"),
        "observed input lacked prearmed read coverage: {path}"
    );
    Ok(())
}

fn paths_overlap(a: &str, b: &str) -> bool {
    #[cfg(windows)]
    {
        let a =
            crate::paths::file_key(&std::path::absolute(a).unwrap_or_else(|_| PathBuf::from(a)))
                .to_lowercase();
        let b =
            crate::paths::file_key(&std::path::absolute(b).unwrap_or_else(|_| PathBuf::from(b)))
                .to_lowercase();
        return path_under(&a, &b) || path_under(&b, &a);
    }
    #[cfg(not(windows))]
    {
        path_under(a, b) || path_under(b, a)
    }
}

fn successful_task_state(state: &serde_json::Value) -> bool {
    state["executed"] == true
        && state["did_work"] == true
        && state["failure"].is_null()
        && state["skipped"] == false
        && state["up_to_date"] == false
        && state["no_source"] == false
}

pub(crate) fn path_under(key: &str, root: &str) -> bool {
    let root = root.trim_end_matches('/');
    key == root
        || key
            .strip_prefix(root)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn environment_hash() -> String {
    use sha2::{Digest, Sha256};
    // Hash values rather than retaining/logging potentially sensitive environment strings.
    let variables: std::collections::BTreeMap<_, _> = std::env::vars_os()
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect();
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&variables).unwrap_or_default())
    )
}

const MAX_RECEIPT_INVENTORY_ENTRIES: usize = 20_000;

fn inventory(path: &Path) -> anyhow::Result<serde_json::Value> {
    use sha2::{Digest, Sha256};
    let key = crate::paths::file_key(path);
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(serde_json::json!({"path":key,"kind":"absent"}))
        }
        Err(error) => return Err(error.into()),
    };
    anyhow::ensure!(
        !metadata.file_type().is_symlink(),
        "unsupported input symlink"
    );
    if metadata.is_file() {
        let mut hash = Sha256::new();
        let mut file = std::fs::File::open(path)?;
        let mut buffer = [0; 65536];
        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
        }
        return Ok(
            serde_json::json!({"path":key,"kind":"file","bytes":metadata.len(),"sha256":format!("{:x}",hash.finalize())}),
        );
    }
    anyhow::ensure!(metadata.is_dir(), "unsupported non-regular input");
    let mut entries = std::collections::BTreeMap::new();
    for entry in walkdir::WalkDir::new(path).follow_links(false) {
        let entry = entry?;
        anyhow::ensure!(
            !entry.file_type().is_symlink(),
            "unsupported directory symlink"
        );
        if entry.file_type().is_dir() {
            continue;
        }
        anyhow::ensure!(
            entries.len() < MAX_RECEIPT_INVENTORY_ENTRIES,
            "receipt inventory cap exceeded"
        );
        entries.insert(
            crate::paths::file_key(entry.path()),
            inventory(entry.path())?,
        );
    }
    Ok(
        serde_json::json!({"path":key,"kind":"directory","entries":entries.into_values().collect::<Vec<_>>()}),
    )
}

#[cfg(test)]
pub(crate) fn observed_test_request(root: &Path) -> std::sync::Arc<ObservedCompileRequest> {
    // Lifecycle fixtures only; this cannot mint a production receipt or generated source Def.
    use std::sync::{atomic::AtomicBool, Arc};
    let task = crate::classpath::CompilerTask {
        task: ":compileJava".into(),
        language: crate::classpath::CompilerLanguage::Java,
        task_type: "org.gradle.api.tasks.compile.JavaCompile".into(),
        arguments: Vec::new(),
        annotation_processor_path: Vec::new(),
        java_home: None,
        options: Default::default(),
        source_files: vec![root.join("src/Person.java")],
        classpath: Vec::new(),
        declared_input_files: vec![root.join("src/Person.java")],
        declared_output_roots: vec![root.join("build/classes/java/main")],
        output_roots: vec![root.join("build/classes/java/main")],
        complete: true,
        problem: None,
    };
    Arc::new(ObservedCompileRequest {
        root: root.into(),
        project_dir: root.into(),
        scope: crate::project_model::ProjectScope {
            module: ":".into(),
            source_set: "main".into(),
        },
        source_set: "main".into(),
        task: task.clone(),
        compiler_tasks: vec![task],
        project_path: ":".into(),
        source_roots: vec![crate::paths::file_key(&root.join("src"))],
        classpath: Vec::new(),
        cache_dir: root.join("unused-test-cache"),
        owners: Vec::new(),
        nonce: "lifecycle-fixture-only".into(),
        invalid: Arc::new(AtomicBool::new(false)),
        cancelled: Arc::new(AtomicBool::new(false)),
        prepared_paths: None,
    })
}

#[cfg(test)]
pub(crate) fn observed_test_receipt(
    request: std::sync::Arc<ObservedCompileRequest>,
) -> std::sync::Arc<SuccessfulCompileReceipt> {
    // Empty lifecycle state, never positive classfile admission or a Gradle-success oracle.
    std::sync::Arc::new(SuccessfulCompileReceipt {
        request,
        inputs: Vec::new(),
        outputs: Vec::new(),
        environment_sha256: environment_hash(),
    })
}

#[cfg(test)]
mod observed_role_tests {
    use super::*;
    use std::sync::Arc;

    fn fixture_request() -> Arc<ObservedCompileRequest> {
        observed_test_request(&std::env::temp_dir().join("ktlsp-observed-role-only"))
    }

    fn sync_selected(request: &mut ObservedCompileRequest) {
        request.compiler_tasks[0] = request.task.clone();
    }

    #[test]
    fn qualified_task_status_requires_the_selected_module() {
        assert!(parse_output("> Task :app:compileJava\n", ":app:compileJava").executed);
        assert!(!parse_output("> Task :other:compileJava\n", ":app:compileJava").executed);
        assert!(!parse_output("> Task :app:compileJava UP-TO-DATE\n", ":app:compileJava").executed);
        assert!(
            parse_output("> Task :app:compileJava\n", "compileJava").executed,
            "legacy unqualified task names retain their behavior"
        );
    }

    #[test]
    fn registered_generated_root_is_coverage_but_actual_read_overlap_is_rejected() {
        let mut owned = fixture_request();
        let request = Arc::get_mut(&mut owned).unwrap();
        let output = request.root.join("build/generated/annotationProcessor");
        request.source_roots.push(crate::paths::file_key(&output));
        request.task.declared_output_roots.push(output.clone());
        sync_selected(request);
        assert!(
            request.validate_roles().is_ok(),
            "registration alone does not prove a read"
        );
        assert!(request.is_known_output(&crate::paths::file_key(&output.join("Generated.java"))));
        assert!(!request.planned_reads().contains(&output));

        for role in ["source", "classpath", "declared-input", "AP", "plugin"] {
            let mut owned = fixture_request();
            let candidate = Arc::get_mut(&mut owned).unwrap();
            let input = candidate.task.output_roots[0].join("read-again.jar");
            match role {
                "source" => candidate.task.source_files.push(input),
                "classpath" => candidate.task.classpath.push(input),
                "declared-input" => candidate.task.declared_input_files.push(input),
                "AP" => candidate.task.annotation_processor_path.push(input),
                "plugin" => candidate
                    .task
                    .arguments
                    .push(format!("-Xplugin={}", input.display())),
                _ => unreachable!(),
            }
            sync_selected(candidate);
            assert!(
                candidate.validate_roles().is_err(),
                "{role} was lost before role validation"
            );
        }
    }

    #[test]
    fn upstream_owner_is_distinct_but_ambiguous_or_missing_metadata_never_proves_roles() {
        let mut owned = fixture_request();
        let candidate = Arc::get_mut(&mut owned).unwrap();
        let mut upstream = candidate.task.clone();
        upstream.task = ":compileKotlin".into();
        upstream.language = crate::classpath::CompilerLanguage::Kotlin;
        upstream.source_files = vec![candidate.root.join("src/Upstream.kt")];
        upstream.declared_input_files = upstream.source_files.clone();
        upstream.output_roots = vec![candidate.root.join("build/classes/kotlin/main")];
        upstream.declared_output_roots = upstream.output_roots.clone();
        candidate.task.classpath = vec![candidate.root.join("dependencies/external.jar")];
        sync_selected(candidate);
        candidate.compiler_tasks.push(upstream);
        assert!(
            candidate.validate_roles().is_ok(),
            "distinct output ownership alone is supported"
        );
        candidate.task.classpath = candidate.compiler_tasks[1].output_roots.clone();
        sync_selected(candidate);
        assert!(candidate.validate_roles().is_err(),
            "legitimate upstream Java classpath remains an OPEN useful-API gap until temporal read proof");
        candidate.task.classpath.clear();
        candidate.task.annotation_processor_path = candidate.compiler_tasks[1].output_roots.clone();
        sync_selected(candidate);
        assert!(
            candidate.validate_roles().is_err(),
            "an AP input cannot be hidden by output delegation"
        );
        candidate.task.annotation_processor_path.clear();
        sync_selected(candidate);
        candidate.compiler_tasks[1]
            .declared_output_roots
            .push(candidate.task.output_roots[0].clone());
        assert!(
            candidate.validate_roles().is_err(),
            "two output owners must not be merged"
        );

        for missing in [
            "source",
            "class-destination",
            "declared-output",
            "relative-input",
        ] {
            let mut owned = fixture_request();
            let candidate = Arc::get_mut(&mut owned).unwrap();
            match missing {
                "source" => candidate.task.source_files.clear(),
                "class-destination" => candidate.task.output_roots.clear(),
                "declared-output" => candidate.task.declared_output_roots.clear(),
                "relative-input" => candidate
                    .task
                    .declared_input_files
                    .push(PathBuf::from("relative.jar")),
                _ => unreachable!(),
            }
            sync_selected(candidate);
            assert!(
                candidate.validate_roles().is_err(),
                "complete=true cannot prove {missing}"
            );
        }
    }

    #[test]
    fn observed_upstream_binary_a_b_a_cannot_be_certified_by_endpoint_stamps() {
        use sha2::Digest;
        let mut owned = fixture_request();
        let request = Arc::get_mut(&mut owned).unwrap();
        let mut upstream = request.task.clone();
        upstream.task = ":compileKotlin".into();
        upstream.language = crate::classpath::CompilerLanguage::Kotlin;
        upstream.output_roots = vec![request.root.join("build/classes/kotlin/main")];
        upstream.declared_output_roots = upstream.output_roots.clone();
        let binary = upstream.output_roots[0].join("Upstream.class");
        request.compiler_tasks.push(upstream);
        // Model roles have no overlap here: the actual receipt reveals an additional read.
        assert!(request.validate_roles().is_ok());
        let mut inputs = serde_json::Value::Object(
            OBSERVED_INPUT_FIELDS
                .iter()
                .map(|field| ((*field).into(), serde_json::json!([])))
                .collect(),
        );
        inputs["ordered_classpath"] = serde_json::json!([{
            "path": crate::paths::file_key(&binary), "kind": "file", "bytes":1,
            "sha256":format!("{:x}", sha2::Sha256::digest(b"A"))
        }]);
        let identical_endpoints = inputs.clone();
        assert_eq!(
            inputs, identical_endpoints,
            "equal endpoints say nothing about an intermediate B"
        );
        assert!(
            validate_observed_roles(request, &inputs).is_err(),
            "a deferred binary read must not be admitted even with equal endpoint stamps"
        );
    }

    #[test]
    fn selected_and_upstream_reads_cannot_acquire_coverage_after_the_action() {
        let request = fixture_request();
        let plan =
            crate::compiled_watch::WatchPlan::build(&request, &request.initial_watch_paths())
                .unwrap();
        let source = crate::paths::file_key(&request.task.source_files[0]);
        let config = crate::paths::file_key(&request.root.join("gradle.properties"));
        let external_config = crate::paths::file_key(
            &request
                .root
                .parent()
                .unwrap()
                .join("unplanned-source-parent/lombok.config"),
        );
        let known =
            serde_json::json!({"path":source,"kind":"file","sha256":"same-at-both-endpoints"});
        let negative = serde_json::json!({"path":config,"kind":"absent"});
        let unprotected = serde_json::json!({"path":external_config,"kind":"absent"});
        for field in OBSERVED_INPUT_FIELDS {
            let mut inputs = serde_json::json!({});
            for other in OBSERVED_INPUT_FIELDS {
                inputs[other] = serde_json::json!([]);
            }
            inputs[field] = serde_json::json!([known.clone(), negative.clone()]);
            assert!(
                validate_prearmed_inputs(&plan, &inputs).is_ok(),
                "{field} has initial source/negative coverage"
            );
            inputs[field] = serde_json::json!([unprotected.clone()]);
            assert!(
                validate_prearmed_inputs(&plan, &inputs).is_err(),
                "{field} cannot certify A->B->A solely from matching endpoint/late watches"
            );
        }
        // Upstream task-start.declared_inputs use the same rule; a parent guard is not an
        // arbitrary sibling read watch, even when a final inventory reports absence again.
        assert!(ensure_prearmed_read(&plan, &known).is_ok());
        assert!(ensure_prearmed_read(&plan, &unprotected).is_err());
        let later = crate::compiled_watch::WatchPlan::build(&request, &[external_config]).unwrap();
        assert!(ensure_prearmed_read(&later, &unprotected).is_ok());
        assert!(
            ensure_prearmed_read(&plan, &unprotected).is_err(),
            "a late plan does not change the actual initial plan"
        );
    }

    #[test]
    fn observed_input_categories_are_checked_before_path_deduplication() {
        let request = fixture_request();
        let empty = serde_json::Value::Object(
            OBSERVED_INPUT_FIELDS
                .iter()
                .map(|field| ((*field).into(), serde_json::json!([])))
                .collect(),
        );
        assert!(validate_observed_roles(&request, &empty).is_ok());
        for field in OBSERVED_INPUT_FIELDS {
            let mut inputs = empty.clone();
            inputs[field] = serde_json::json!([{
                "path": crate::paths::file_key(&request.task.output_roots[0].join("collision.class")),
                "kind": "absent"
            }]);
            assert!(
                validate_observed_roles(&request, &inputs).is_err(),
                "{field}"
            );
        }
        assert!(validate_observed_roles(&request, &serde_json::json!({})).is_err());
    }
}

#[cfg(test)]
pub(crate) fn observed_test_receipt_with_source(
    request: std::sync::Arc<ObservedCompileRequest>,
    key: &str,
    text: &str,
    class_file: &Path,
    class_bytes: &[u8],
) -> std::sync::Arc<SuccessfulCompileReceipt> {
    // Host invalidation fixture only: deliberately no native/task/freshness proof. The private
    // production receipt constructor remains exclusively in run_observed_compile.
    use sha2::{Digest, Sha256};
    std::sync::Arc::new(SuccessfulCompileReceipt {
        request,
        inputs: vec![
            serde_json::json!({"path":key,"kind":"file","bytes":text.len(),
            "sha256":format!("{:x}", Sha256::digest(text.as_bytes()))}),
        ],
        outputs: vec![
            serde_json::json!({"path":crate::paths::file_key(class_file),"kind":"file",
            "bytes":class_bytes.len(),"sha256":format!("{:x}",Sha256::digest(class_bytes))}),
        ],
        environment_sha256: environment_hash(),
    })
}

#[cfg(test)]
mod compiled_output_binding_tests {
    use super::*;
    use std::sync::Arc;

    const CLASS: &[u8] =
        include_bytes!("../../ktcore/tests/fixtures/classfile/pluginfixture/Person.class");

    fn fixture() -> (
        Arc<SuccessfulCompileReceipt>,
        PathBuf,
        ktcore::classfile::ClassFileFacts,
    ) {
        let request =
            observed_test_request(&std::env::temp_dir().join("ktlsp-output-binding-only"));
        let path = request.task.output_roots[0].join("pluginfixture/Person.class");
        let receipt = observed_test_receipt_with_source(request, "unused-source", "", &path, CLASS);
        (
            receipt,
            path,
            ktcore::classfile::decode("fixture-origin", CLASS).unwrap(),
        )
    }

    #[test]
    fn different_decoded_bytes_cannot_use_an_unchanged_output_receipt() {
        let (receipt, path, original) = fixture();
        assert!(receipt
            .verify_compiled_output(&path, CLASS.len() as u64, &original)
            .is_ok());
        // Constant-pool-only mutation is a distinct decodable class with the same owner;
        // it models the B bytes retained between A endpoint inventories, not a compiler run.
        let mut different = CLASS.to_vec();
        let offset = different
            .windows(7)
            .position(|value| value == b"getName")
            .unwrap();
        different[offset..offset + 7].copy_from_slice(b"getElse");
        let different = ktcore::classfile::decode("fixture-origin", &different).unwrap();
        assert_eq!(
            original.identity.internal_name,
            different.identity.internal_name
        );
        assert_ne!(original.identity.input, different.identity.input);
        assert!(receipt
            .verify_compiled_output(&path, CLASS.len() as u64, &different)
            .is_err());
    }

    #[test]
    fn output_binding_requires_selected_exact_path_and_complete_consistent_file_stamp() {
        let (receipt, path, class) = fixture();
        let upstream = receipt
            .request
            .root
            .join("build/classes/kotlin/main/pluginfixture/Person.class");
        assert!(receipt
            .verify_compiled_output(&upstream, CLASS.len() as u64, &class)
            .is_err());
        let wrong_name = path.with_file_name("Other.class");
        assert!(receipt
            .verify_compiled_output(&wrong_name, CLASS.len() as u64, &class)
            .is_err());
        assert!(receipt
            .verify_compiled_output(
                &path,
                ktcore::classfile::MAX_CLASS_FILE_BYTES as u64 + 1,
                &class
            )
            .is_err());
        let file = receipt.outputs[0].clone();
        for change in [
            "missing",
            "upstream-only",
            "kind",
            "length",
            "hash",
            "conflict",
        ] {
            let mut outputs = vec![file.clone()];
            match change {
                "missing" => outputs.clear(),
                "upstream-only" => {
                    outputs[0]["path"] = serde_json::json!(crate::paths::file_key(&upstream))
                }
                "kind" => outputs[0]["kind"] = serde_json::json!("absent"),
                "length" => outputs[0]["bytes"] = serde_json::json!(CLASS.len() + 1),
                "hash" => outputs[0]["sha256"] = serde_json::json!("00".repeat(32)),
                "conflict" => {
                    let mut other = file.clone();
                    other["sha256"] = serde_json::json!("00".repeat(32));
                    outputs.push(other);
                }
                _ => unreachable!(),
            }
            let candidate = SuccessfulCompileReceipt {
                request: receipt.request.clone(),
                inputs: Vec::new(),
                outputs,
                environment_sha256: environment_hash(),
            };
            assert!(
                candidate
                    .verify_compiled_output(&path, CLASS.len() as u64, &class)
                    .is_err(),
                "{change}"
            );
        }
        let mut positive = SuccessfulCompileReceipt {
            request: receipt.request.clone(),
            inputs: Vec::new(),
            outputs: vec![
                serde_json::json!({"path":crate::paths::file_key(&receipt.request.task.output_roots[0]),
                "kind":"directory","entries":[file.clone()]}),
            ],
            environment_sha256: environment_hash(),
        };
        assert!(positive
            .verify_compiled_output(&path, CLASS.len() as u64, &class)
            .is_ok());
        positive.outputs.push(file);
        assert!(
            positive
                .verify_compiled_output(&path, CLASS.len() as u64, &class)
                .is_ok(),
            "identical stamp duplicates"
        );
    }
}

#[cfg(test)]
mod observed_path_cache_tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn prepared_paths_preserve_input_and_output_boundaries() {
        let root = std::env::temp_dir().join("ktlsp-prepared-path-boundaries");
        let mut owned = observed_test_request(&root);
        let request = Arc::get_mut(&mut owned).unwrap();
        let ap = root.join("ap/processor.jar");
        let plugin = root.join("plugins/compiler.jar");
        let external = root.join("dependencies/api.jar");
        let options = root.join("ap/options.json");
        request.task.annotation_processor_path.push(ap.clone());
        request
            .task
            .arguments
            .push(format!("-Xplugin={}", plugin.display()));
        request.task.declared_input_files.push(options.clone());
        request.task.java_home = Some(root.join("jdk"));
        request.compiler_tasks[0] = request.task.clone();
        request.classpath.push(crate::paths::file_key(&external));
        let unprepared = request.initial_watch_paths().into_owned();
        request.prepare_paths().unwrap();
        let first = request.initial_watch_paths();
        assert_eq!(first.as_ref(), unprepared.as_slice());
        for input in [
            root.join("src/Person.java"),
            root.join("gradle.properties"),
            root.join("jdk/lib/modules"),
            ap,
            plugin,
            external,
            options,
        ] {
            let key = crate::paths::file_key(&input);
            assert!(request.observes_path(&key), "{key}");
            assert!(!request.is_known_output(&key), "{key}");
        }
        let output = &request.task.declared_output_roots[0];
        assert!(request.is_known_output(&crate::paths::file_key(&output.join("Person.class"))));
        assert!(
            !request.is_known_output(&crate::paths::file_key(&output.with_extension("sibling")))
        );
        assert!(!request.observes_path(&crate::paths::file_key(
            &root.join("unplanned/options.json")
        )));
    }

    #[test]
    fn overlapping_read_cannot_prepare_output_deferral_or_reuse_an_old_plan() {
        let root = std::env::temp_dir().join("ktlsp-prepared-path-overlap");
        let mut owned = observed_test_request(&root);
        let request = Arc::get_mut(&mut owned).unwrap();
        request.prepare_paths().unwrap();
        let output = request.task.declared_output_roots[0].join("processor.jar");
        request.task.annotation_processor_path.push(output.clone());
        request.compiler_tasks[0] = request.task.clone();
        assert!(request.prepare_paths().is_err());
        assert!(request.prepared_paths.is_none());
        assert!(!request.is_known_output(&crate::paths::file_key(&output)));
    }

    #[test]
    fn prepared_paths_do_not_turn_negative_parent_guards_into_read_coverage() {
        let root = std::env::temp_dir().join("ktlsp-prepared-negative-coverage");
        let mut owned = observed_test_request(&root);
        let request = Arc::get_mut(&mut owned).unwrap();
        request.prepare_paths().unwrap();
        let plan = crate::compiled_watch::WatchPlan::build(request, &request.initial_watch_paths())
            .unwrap();
        let known = serde_json::json!({
            "path":crate::paths::file_key(&root.join("gradle.properties")), "kind":"absent"
        });
        let unprotected = serde_json::json!({
            "path":crate::paths::file_key(&root.parent().unwrap().join("unplanned-source-parent/lombok.config")),
            "kind":"absent"
        });
        for field in OBSERVED_INPUT_FIELDS {
            let mut inputs = serde_json::json!({});
            for other in OBSERVED_INPUT_FIELDS {
                inputs[other] = serde_json::json!([]);
            }
            inputs[field] = serde_json::json!([known.clone()]);
            assert!(validate_prearmed_inputs(&plan, &inputs).is_ok(), "{field}");
            inputs[field] = serde_json::json!([unprotected.clone()]);
            assert!(validate_prearmed_inputs(&plan, &inputs).is_err(), "{field}");
        }
        assert!(ensure_prearmed_read(&plan, &unprotected).is_err());
        assert!(request
            .verify_prepared_environment(&environment_hash())
            .is_ok());
        assert!(request
            .verify_prepared_environment("different-environment")
            .is_err());
    }
}
