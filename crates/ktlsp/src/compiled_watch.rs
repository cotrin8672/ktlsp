//! Bounded receipt inputs; model source/output events reuse the existing LSP lifecycle.
use crate::compile::{path_under, ObservedCompileRequest};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::{Mutex, RwLock};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::mpsc;

const MAX_WATCH_DIRS: usize = 512;
pub(crate) const MAX_INPUT_PATHS: usize = 4096;
const MAX_DIRECTORY_ENTRIES: usize = 20_000;

fn key(path: &Path) -> String {
    let key = crate::paths::file_key(path);
    if cfg!(windows) {
        key.to_lowercase()
    } else {
        key
    }
}

#[derive(Clone, Default)]
pub(crate) struct WatchPlan {
    directories: BTreeSet<PathBuf>,
    recursive: BTreeSet<PathBuf>,
    exact: BTreeSet<String>,
    trees: BTreeSet<String>,
    ancestors: BTreeSet<String>,
    memberships: BTreeSet<String>,
    delegated_sources: BTreeSet<String>,
    delegated_outputs: BTreeSet<String>,
    filtered_roots: Vec<(String, BTreeSet<String>)>,
}

impl WatchPlan {
    pub(crate) fn build(
        request: &ObservedCompileRequest,
        inputs: &[String],
    ) -> anyhow::Result<Self> {
        request.validate_roles()?;
        let mut plan = Self::default();
        let project = key(&request.root);
        plan.delegated_sources.extend(
            request
                .source_roots
                .iter()
                .map(|p| key(Path::new(p)))
                .filter(|p| path_under(p, &project)),
        );
        // The producer checked the exact ACKed client watcher options for this model.
        // A registered generated-source root is coverage, not proof of an actual read role.
        // AP/plugin read overlaps were rejected before these output paths could be delegated.
        plan.delegated_outputs.extend(
            request
                .compiler_tasks
                .iter()
                .flat_map(|task| &task.declared_output_roots)
                .map(|p| key(p))
                .filter(|p| path_under(p, &project)),
        );
        // The parent guards replacement/deletion of the project root, not its contents.
        plan.guard_parents(&request.root, false)?;
        plan.ancestors.insert(project.clone());
        anyhow::ensure!(inputs.len() <= MAX_INPUT_PATHS, "compiler-native-input-cap");
        for input in inputs {
            let input = Path::new(input);
            let input_key = key(input);
            let directory =
                request.is_declared_directory(&crate::paths::file_key(input)) || input.is_dir();
            if plan.delegated_outputs.contains(&input_key) {
                continue;
            }
            if !directory && plan.is_delegated_source(&input_key) {
                continue;
            }
            // A registered project glob alone does not prove coverage of config/AP input files.
            plan.add_input(input, directory)?;
        }
        Ok(plan)
    }

    /// Explicit compiler read inputs. Build once per selected recipe, then retain its native watch.
    pub(crate) fn read_inputs(inputs: &[PathBuf]) -> anyhow::Result<Self> {
        anyhow::ensure!(inputs.len() <= MAX_INPUT_PATHS, "compiler-native-input-cap");
        let mut plan = Self::default();
        for input in inputs {
            plan.add_input(input, input.is_dir())?;
        }
        Ok(plan)
    }

    /// Arm root coverage without enumerating before the lease exists. The caller then
    /// performs its bounded filtered walk; unsupported native recursion stays Unknown.
    pub(crate) fn filtered_root(root: &Path, excluded: &BTreeSet<String>) -> anyhow::Result<Self> {
        anyhow::ensure!(root.is_absolute() && !root.components().any(|part|
            matches!(part,std::path::Component::ParentDir|std::path::Component::CurDir)), "root-watch-relative-path");
        anyhow::ensure!(excluded.len()<=128 && excluded.iter().all(|name|
            !name.is_empty() && name.len()<=256 && name!="." && name!=".."
                && !name.chars().any(|ch|ch.is_control() || matches!(ch,'/'|'\\'|':'|'*'|'?'|'['|']'))),
            "root-watch-exclusion-unhandled");
        let metadata = std::fs::symlink_metadata(root)?;
        anyhow::ensure!(metadata.is_dir() && !metadata.file_type().is_symlink(), "root-watch-unknown-root");
        #[cfg(not(windows))]
        anyhow::bail!("bounded-root-recursion-unavailable");
        #[cfg(windows)]
        {
            let mut plan = Self::default();
            plan.trees.insert(key(root));
            plan.guard_parents(root, true)?;
            plan.add_recursive_directory(root)?;
            plan.filtered_roots.push((key(root), excluded.iter().map(|name|name.to_lowercase()).collect()));
            Ok(plan)
        }
    }

    /// A bounded copied inventory: exact file reads plus direct-child membership changes.
    pub(crate) fn read_source_inventory(
        files: &[PathBuf],
        directories: &[PathBuf],
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            files.len() + directories.len() <= MAX_INPUT_PATHS,
            "compiler-native-input-cap"
        );
        let mut plan = Self::read_inputs(files)?;
        for directory in directories {
            let metadata = std::fs::symlink_metadata(directory)?;
            anyhow::ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "compiler-watch-inventory-directory-unknown"
            );
            plan.memberships.insert(key(directory));
            plan.add_directory(directory)?;
            plan.guard_parents(directory, true)?;
        }
        Ok(plan)
    }

    /// Concrete nonrecursive directory registrations in this immutable plan. After extend
    /// succeeds, a current read traversal may not rely on a directory omitted during planning.
    pub(crate) fn includes_directory_watch(&self, path: &Path) -> bool {
        self.directories.contains(path)
            || self
                .recursive
                .iter()
                .any(|root| path_under(&key(path), &key(root)))
    }

    pub(crate) fn covers_read(&self, path: &str, directory: bool) -> bool {
        let path = key(Path::new(path));
        // The callback ignores delegated output writes; a containing read tree cannot prove them.
        if self
            .delegated_outputs
            .iter()
            .any(|output| path_under(&path, output) || (directory && path_under(output, &path)))
        {
            return false;
        }
        self.trees.iter().any(|root| path_under(&path, root) && !self.excluded_descendant(root,&path)
            && !(directory && self.filtered_roots.iter().any(|(filtered,excluded)|filtered==root && !excluded.is_empty())))
            || (!directory && (self.exact.contains(&path) || self.is_delegated_source(&path)))
    }

    fn excluded_descendant(&self, root: &str, path: &str) -> bool {
        self.filtered_roots.iter().any(|(filtered,excluded)|filtered==root
            && path.strip_prefix(root).and_then(|relative|relative.strip_prefix('/'))
                .and_then(|relative|relative.rsplit_once('/')).is_some_and(|(parents,_)|
                    parents.split('/').any(|segment|excluded.contains(segment))))
    }

    fn is_delegated_source(&self, path: &str) -> bool {
        crate::language::SourceLanguage::for_project_path(Path::new(path)).is_some()
            && self
                .delegated_sources
                .iter()
                .any(|root| path_under(path, root))
    }

    fn add_input(&mut self, path: &Path, declared_directory: bool) -> anyhow::Result<()> {
        anyhow::ensure!(path.is_absolute(), "compiler-watch-relative-input");
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        anyhow::ensure!(
            !metadata
                .as_ref()
                .is_some_and(|m| m.file_type().is_symlink()),
            "compiler-watch-symlink"
        );
        let directory = declared_directory || metadata.as_ref().is_some_and(|m| m.is_dir());
        if directory {
            self.trees.insert(key(path));
        } else {
            self.exact.insert(key(path));
        }
        self.guard_parents(path, metadata.as_ref().is_some_and(|m| m.is_dir()))?;
        if metadata.as_ref().is_some_and(|m| m.is_dir()) {
            // Windows recursion uses one native handle; retain other backends' per-directory bound.
            #[cfg(windows)]
            self.add_recursive_directory(path)?;
            let outputs = self.delegated_outputs.clone();
            for (n, entry) in walkdir::WalkDir::new(path)
                .follow_links(false)
                .into_iter()
                .filter_entry(|entry| !outputs.contains(&key(entry.path())))
                .enumerate()
            {
                anyhow::ensure!(n < MAX_DIRECTORY_ENTRIES, "compiler-watch-directory-cap");
                let entry = entry?;
                anyhow::ensure!(!entry.file_type().is_symlink(), "compiler-watch-symlink");
                #[cfg(not(windows))]
                if entry.file_type().is_dir() {
                    self.add_directory(entry.path())?;
                }
            }
        }
        Ok(())
    }

    fn guard_parents(&mut self, path: &Path, native_at_path: bool) -> anyhow::Result<()> {
        // Windows cannot rename higher ancestors while a watched child directory is open.
        // Keep the closest watch and its own rename guard; still validate every logical ancestor.
        let guard_limit = if cfg!(windows) {
            if native_at_path {
                1
            } else {
                2
            }
        } else {
            usize::MAX
        };
        let mut guarded = 0;
        for parent in path.ancestors().skip(1) {
            self.ancestors.insert(key(parent));
            match std::fs::symlink_metadata(parent) {
                Ok(metadata) => {
                    anyhow::ensure!(
                        !metadata.file_type().is_symlink(),
                        "compiler-watch-parent-symlink"
                    );
                    anyhow::ensure!(metadata.is_dir(), "compiler-watch-parent-not-directory");
                    if guarded < guard_limit {
                        self.add_directory(parent)?;
                        guarded += 1;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    fn add_directory(&mut self, path: &Path) -> anyhow::Result<()> {
        if self
            .recursive
            .iter()
            .any(|root| path_under(&key(path), &key(root)))
        {
            return Ok(());
        }
        self.directories.insert(path.to_path_buf());
        anyhow::ensure!(
            self.directories.len() <= MAX_WATCH_DIRS,
            "compiler-native-watch-cap"
        );
        Ok(())
    }

    fn add_recursive_directory(&mut self, path: &Path) -> anyhow::Result<()> {
        if self
            .recursive
            .iter()
            .any(|root| path_under(&key(path), &key(root)))
        {
            return Ok(());
        }
        self.directories
            .retain(|directory| !path_under(&key(directory), &key(path)));
        self.recursive
            .retain(|directory| !path_under(&key(directory), &key(path)));
        self.directories.insert(path.to_owned());
        self.recursive.insert(path.to_owned());
        anyhow::ensure!(
            self.directories.len() <= MAX_WATCH_DIRS,
            "compiler-native-watch-cap"
        );
        Ok(())
    }

    fn merge(&mut self, next: Self) -> anyhow::Result<()> {
        // A root-file domain owns its lease; another domain must not inherit its exclusions.
        anyhow::ensure!(self.filtered_roots.is_empty() && next.filtered_roots.is_empty(),
            "filtered-root-watch-not-composable");
        self.exact.extend(next.exact);
        self.trees.extend(next.trees);
        self.ancestors.extend(next.ancestors);
        self.memberships.extend(next.memberships);
        self.delegated_sources.extend(next.delegated_sources);
        self.delegated_outputs.extend(next.delegated_outputs);
        for directory in &next.recursive {
            self.add_recursive_directory(directory)?;
        }
        for directory in next.directories {
            self.add_directory(&directory)?;
        }
        anyhow::ensure!(
            self.exact.len() + self.trees.len() + self.memberships.len() <= MAX_INPUT_PATHS,
            "compiler-native-input-cap"
        );
        Ok(())
    }

    fn affected(&self, event: &Event) -> bool {
        use notify::event::{CreateKind,ModifyKind,RemoveKind};
        if event.need_rescan() {
            return true;
        }
        if event.kind.is_access()
            && !matches!(
                event.kind,
                EventKind::Access(notify::event::AccessKind::Close(
                    notify::event::AccessMode::Write
                ))
            )
        {
            return false;
        }
        if event.paths.is_empty() || event.paths.len() > 64 {
            return true;
        }
        let ancestor_change = matches!(
            event.kind,
            EventKind::Any
                | EventKind::Other
                | EventKind::Create(_)
                | EventKind::Remove(_)
                | EventKind::Modify(notify::event::ModifyKind::Name(_))
        );
        let filterable = matches!(event.kind,
            EventKind::Create(CreateKind::File|CreateKind::Folder)
                | EventKind::Remove(RemoveKind::File|RemoveKind::Folder)
                | EventKind::Modify(ModifyKind::Data(_)|ModifyKind::Metadata(_))
                | EventKind::Access(_));
        event.paths.iter().any(|path| {
            let parent = path.parent().map(key);
            let key = key(path);
            // Explicit non-source input files always win, even inside an output container.
            if self.exact.contains(&key) {
                return true;
            }
            let explicit_output_input = self.trees.iter().any(|root| {
                path_under(&key, root)
                    && self
                        .delegated_outputs
                        .iter()
                        .any(|output| path_under(root, output))
            });
            if !explicit_output_input
                && self
                    .delegated_outputs
                    .iter()
                    .any(|root| path_under(&key, root))
            {
                return false;
            }
            if self.is_delegated_source(&key)
                && !self.ancestors.contains(&key)
                && !self.trees.contains(&key)
            {
                return false;
            }
            self.trees.iter().any(|root| path_under(&key, root)
                && !(filterable && self.excluded_descendant(root,&key)))
                || ((ancestor_change || !self.filtered_roots.is_empty()) && self.ancestors.contains(&key))
                || (ancestor_change
                    && parent
                        .as_ref()
                        .is_some_and(|parent| self.memberships.contains(parent)))
        })
    }
}

struct Signal {
    invalid: Arc<AtomicBool>,
    closed: AtomicBool,
    wake: mpsc::Sender<()>,
    callback: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl Signal {
    fn disarm(&self) {
        self.closed.store(true, Ordering::Release);
        self.invalid.store(true, Ordering::Release);
        // Callback only schedules existing work and cannot take Workspace inline.
        // Taking this slot also waits for an already scheduled callback to return.
        self.callback.lock().take();
    }

    fn invalidate(&self) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        let first = !self.invalid.swap(true, Ordering::AcqRel);
        if first {
            let callback = self.callback.lock();
            if !self.closed.load(Ordering::Acquire) {
                if let Some(callback) = &*callback { callback(); }
            }
        }
        // One monotone bit + one bounded wake, never an unbounded path/event queue.
        // Full means an invalidation is already queued for this same owner.
        let _ = self.wake.try_send(());
    }

    fn event(&self, plan: &RwLock<WatchPlan>, event: notify::Result<Event>) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        if event
            .as_ref()
            .map_or(true, |event| plan.read().affected(event))
        {
            tracing::debug!(?event, "compiler receipt invalidated by native watch");
            if !self.invalid.load(Ordering::Acquire) {
                let (kind, rescan, path_count, paths, error) = match &event {
                    Ok(event) => (
                        Some(format!("{:?}", event.kind)),
                        event.need_rescan(),
                        event.paths.len(),
                        event
                            .paths
                            .iter()
                            .take(4)
                            .map(|path| {
                                path.to_string_lossy().chars().take(512).collect::<String>()
                            })
                            .collect::<Vec<_>>(),
                        None,
                    ),
                    Err(error) => (
                        None,
                        false,
                        0,
                        Vec::new(),
                        Some(error.to_string().chars().take(512).collect::<String>()),
                    ),
                };
                let read_hint = {
                    let plan = plan.read();
                    plan.exact
                        .iter()
                        .next()
                        .or_else(|| plan.trees.iter().next())
                        .map(|path| path.chars().take(512).collect::<String>())
                };
                crate::trace::span(
                    "compiler.native_watch_invalidated",
                    "compiler",
                    std::time::Instant::now(),
                    serde_json::json!({"read_hint":read_hint,"kind":kind,"rescan":rescan,
                        "path_count":path_count,"paths":paths,"error":error}),
                );
            }
            self.invalidate();
        }
    }
}

pub(crate) struct NativeInputWatch {
    watcher: Mutex<Option<RecommendedWatcher>>,
    recursive_watcher: Mutex<Option<RecommendedWatcher>>,
    plan: Arc<RwLock<WatchPlan>>,
    signal: Arc<Signal>,
}

impl NativeInputWatch {
    fn watcher(
        signal: &Arc<Signal>,
        plan: &Arc<RwLock<WatchPlan>>,
    ) -> anyhow::Result<RecommendedWatcher> {
        let signal = signal.clone();
        let plan = plan.clone();
        Ok(notify::recommended_watcher(move |event| {
            signal.event(&plan, event)
        })?)
    }
    pub(crate) fn new(
        invalid: Arc<AtomicBool>,
        plan: WatchPlan,
        wake: mpsc::Sender<()>,
    ) -> anyhow::Result<Arc<Self>> {
        Self::new_with_callback(invalid, plan, wake, None)
    }

    pub(crate) fn new_with_callback(
        invalid: Arc<AtomicBool>,
        plan: WatchPlan,
        wake: mpsc::Sender<()>,
        callback: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> anyhow::Result<Arc<Self>> {
        let signal = Arc::new(Signal {
            invalid,
            closed: AtomicBool::new(false),
            wake,
            callback: Mutex::new(callback),
        });
        let shared = Arc::new(RwLock::new(plan.clone()));
        let mut watcher = match Self::watcher(&signal, &shared) {
            Ok(watcher) => watcher,
            Err(error) => {
                signal.invalidate();
                return Err(error.into());
            }
        };
        let mut recursive_watcher = if plan.recursive.is_empty() {
            None
        } else {
            match Self::watcher(&signal, &shared) {
                Ok(watcher) => Some(watcher),
                Err(error) => {
                    signal.invalidate();
                    return Err(error);
                }
            }
        };
        for directory in &plan.directories {
            let result = if plan.recursive.contains(directory) {
                recursive_watcher
                    .as_mut()
                    .unwrap()
                    .watch(directory, RecursiveMode::Recursive)
            } else {
                watcher.watch(directory, RecursiveMode::NonRecursive)
            };
            if let Err(error) = result {
                signal.invalidate();
                return Err(error.into());
            }
        }
        // A root/ancestor can be replaced between planning and native arming; no walk has run yet.
        let root_ready = (|| -> anyhow::Result<()> {
            for (root, _) in &plan.filtered_roots {
                for path in Path::new(root).ancestors() {
                    let metadata = std::fs::symlink_metadata(path)?;
                    anyhow::ensure!(metadata.is_dir() && !metadata.file_type().is_symlink(),
                        "root-watch-changed-during-arm");
                }
            }
            Ok(())
        })();
        if let Err(error) = root_ready {
            signal.invalidate();
            return Err(error);
        }
        crate::trace::span(
            "compiler.native_watch_plan",
            "compiler",
            std::time::Instant::now(),
            serde_json::json!({"directories":plan.directories.len(), "recursive_read_trees":plan.recursive.len(), "limit":MAX_WATCH_DIRS}),
        );
        Ok(Arc::new(Self {
            watcher: Mutex::new(Some(watcher)),
            recursive_watcher: Mutex::new(recursive_watcher),
            plan: shared,
            signal,
        }))
    }

    pub(crate) fn extend(&self, next: WatchPlan) -> anyhow::Result<()> {
        let previous = self.plan.read().clone();
        let mut merged = self.plan.read().clone();
        merged.merge(next)?;
        let new_recursive: Vec<_> = merged
            .recursive
            .difference(&previous.recursive)
            .cloned()
            .collect();
        let new_shallow: Vec<_> = merged
            .directories
            .iter()
            .filter(|directory| {
                !merged.recursive.contains(*directory) && !previous.directories.contains(*directory)
            })
            .cloned()
            .collect();
        // Arm replacement coverage before retiring old handles; never cross the native resource cap.
        if previous.directories.len() + new_recursive.len() + new_shallow.len() > MAX_WATCH_DIRS {
            self.signal.invalidate();
            anyhow::bail!("compiler-native-watch-cap");
        }
        *self.plan.write() = merged.clone();
        let mut guard = self.watcher.lock();
        let watcher = guard
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("compiler-watch-closed"))?;
        let mut recursive_guard = self.recursive_watcher.lock();
        if !new_recursive.is_empty() && recursive_guard.is_none() {
            match Self::watcher(&self.signal, &self.plan) {
                Ok(watcher) => *recursive_guard = Some(watcher),
                Err(error) => {
                    self.signal.invalidate();
                    return Err(error);
                }
            }
        }
        for directory in &new_recursive {
            if let Err(error) = recursive_guard
                .as_mut()
                .unwrap()
                .watch(directory, RecursiveMode::Recursive)
            {
                self.signal.invalidate();
                return Err(error.into());
            }
        }
        for directory in &new_shallow {
            if let Err(error) = watcher.watch(directory, RecursiveMode::NonRecursive) {
                self.signal.invalidate();
                return Err(error.into());
            }
        }
        for directory in &previous.directories {
            if !merged.directories.contains(directory)
                || (!previous.recursive.contains(directory) && merged.recursive.contains(directory))
            {
                let result = if previous.recursive.contains(directory) {
                    recursive_guard.as_mut().unwrap().unwatch(directory)
                } else {
                    watcher.unwatch(directory)
                };
                if let Err(error) = result {
                    self.signal.invalidate();
                    return Err(error.into());
                }
            }
        }
        crate::trace::span(
            "compiler.native_watch_plan",
            "compiler",
            std::time::Instant::now(),
            serde_json::json!({"directories":merged.directories.len(), "recursive_read_trees":merged.recursive.len(), "limit":MAX_WATCH_DIRS}),
        );
        Ok(())
    }

    pub(crate) fn close(&self) {
        // Removing native coverage cannot leave its accepted owner current.
        self.signal.invalidate();
        self.signal.closed.store(true, Ordering::Release);
        let watcher = self.watcher.lock().take();
        let recursive_watcher = self.recursive_watcher.lock().take();
        // Never join the notify thread from its callback or while holding Workspace's mutex.
        drop(watcher);
        drop(recursive_watcher);
    }
}

impl Drop for NativeInputWatch {
    fn drop(&mut self) {
        self.close();
    }
}

/// One host input lease is shared by a scope's opaque handles and final read set.
/// The callback only schedules existing extension work; it must not lock Workspace inline.
pub(crate) struct HostInputLease {
    invalid: Arc<AtomicBool>,
    watch: Option<Arc<NativeInputWatch>>,
}

impl std::fmt::Debug for HostInputLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostInputLease").field("invalid", &self.invalid.load(Ordering::Acquire)).finish()
    }
}

impl HostInputLease {
    pub(crate) fn new(plan: WatchPlan, wake: Arc<dyn Fn() + Send + Sync>) -> anyhow::Result<Arc<Self>> {
        let invalid = Arc::new(AtomicBool::new(false));
        let (sender, _receiver) = mpsc::channel(1);
        let watch = NativeInputWatch::new_with_callback(invalid.clone(), plan, sender, Some(wake))?;
        Ok(Arc::new(Self { invalid, watch: Some(watch) }))
    }
}

impl Drop for HostInputLease {
    fn drop(&mut self) {
        if let Some(watch) = self.watch.take() {
            // Like Worker::drop, final retirement may happen under Workspace access.
            // Disarm synchronously; native notify destruction/join runs off that lock.
            watch.signal.disarm();
            std::thread::spawn(move || drop(watch));
        }
    }
}

impl ktcore::extensions::HostInputGuard for HostInputLease {
    fn is_current(&self) -> bool { !self.invalid.load(Ordering::Acquire) }
}

/// Observed class-output receipts retain their existing owner and lifecycle.
pub(crate) struct NativeCompileWatch {
    pub(crate) owner: Arc<ObservedCompileRequest>,
    input: Arc<NativeInputWatch>,
}

impl NativeCompileWatch {
    pub(crate) fn new(
        owner: Arc<ObservedCompileRequest>,
        plan: WatchPlan,
        wake: mpsc::Sender<()>,
    ) -> anyhow::Result<Arc<Self>> {
        let input = NativeInputWatch::new(owner.invalid.clone(), plan, wake)?;
        Ok(Arc::new(Self { owner, input }))
    }

    pub(crate) fn extend(&self, next: WatchPlan) -> anyhow::Result<()> {
        self.input.extend(next)
    }

    pub(crate) fn close(&self) {
        self.input.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, ModifyKind, RemoveKind, RenameMode};

    #[test]
    fn host_input_callback_wakes_once_and_never_after_close() {
        use std::sync::atomic::AtomicUsize;
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let (wake, _receiver) = mpsc::channel(1);
        let signal = Signal { invalid: Arc::new(AtomicBool::new(false)),
            closed: AtomicBool::new(false), wake,
            callback: Mutex::new(Some(Arc::new(move || { counted.fetch_add(1, Ordering::AcqRel); }))) };
        signal.invalidate(); signal.invalidate();
        assert!(signal.invalid.load(Ordering::Acquire));
        assert_eq!(calls.load(Ordering::Acquire), 1);
        signal.disarm();
        assert!(signal.callback.lock().is_none());
        signal.invalidate();
        assert_eq!(calls.load(Ordering::Acquire), 1);
    }


    #[test]
    fn read_coverage_never_promotes_parent_guards_outputs_or_file_slots_to_trees() {
        let base = std::env::temp_dir().join("ktlsp-initial-read-coverage-only");
        let mut plan = WatchPlan::default();
        let external = base.join("external");
        plan.ancestors.insert(key(&external));
        plan.directories.insert(external.clone());
        let exact = external.join("lombok.config");
        plan.exact.insert(key(&exact));
        let tree = external.join("sources");
        plan.trees.insert(key(&tree));
        let delegated = base.join("project/src");
        plan.delegated_sources.insert(key(&delegated));
        let output = base.join("project/build/classes");
        plan.delegated_outputs.insert(key(&output));
        assert!(plan.covers_read(&key(&exact), false));
        assert!(
            !plan.covers_read(&key(&exact), true),
            "an exact absent/file watch does not cover a later directory inventory"
        );
        assert!(!plan.covers_read(&key(&external.join("other.config")), false));
        assert!(
            !plan.covers_read(&key(&external), true),
            "armed parent directory alone guards replacement, not arbitrary read contents"
        );
        assert!(plan.covers_read(&key(&tree.join("nested/Input.java")), false));
        assert!(plan.covers_read(&key(&tree.join("nested")), true));
        assert!(plan.covers_read(&key(&delegated.join("Input.java")), false));
        assert!(!plan.covers_read(&key(&delegated.join("settings.xml")), false));
        assert!(!plan.covers_read(&key(&delegated.join("Fake.java")), true));
        assert!(!plan.covers_read(&key(&output.join("Person.class")), false));
        plan.trees.insert(key(&base.join("project")));
        assert!(
            !plan.covers_read(&key(&output.join("Person.class")), false),
            "a broad read tree cannot certify writes ignored by output delegation"
        );
        assert!(
            !plan.covers_read(&key(&base.join("project")), true),
            "a whole directory inventory includes the ignored output subtree"
        );
    }

    fn event(kind: EventKind, path: &str) -> Event {
        Event::new(kind).add_path(PathBuf::from(path))
    }

    fn plan() -> WatchPlan {
        WatchPlan {
            exact: BTreeSet::from([key(Path::new("/receipt/ancestor/lombok.config"))]),
            ancestors: BTreeSet::from([
                key(Path::new("/receipt")),
                key(Path::new("/receipt/ancestor")),
            ]),
            ..WatchPlan::default()
        }
    }

    #[test]
    fn external_negative_ancestor_arrival_replace_and_parent_rename_invalidate() {
        let plan = plan();
        for event in [
            event(
                EventKind::Create(CreateKind::File),
                "/receipt/ancestor/lombok.config",
            ),
            event(
                EventKind::Modify(ModifyKind::Name(RenameMode::To)),
                "/receipt/ancestor/lombok.config",
            ),
            event(EventKind::Create(CreateKind::Folder), "/receipt/ancestor"),
            event(EventKind::Remove(RemoveKind::Folder), "/receipt/ancestor"),
            event(
                EventKind::Modify(ModifyKind::Name(RenameMode::From)),
                "/receipt",
            ),
        ] {
            assert!(plan.affected(&event));
        }
        assert!(!plan.affected(&event(
            EventKind::Create(CreateKind::File),
            "/receipt/unrelated.txt"
        )));
    }

    #[tokio::test]
    async fn overflow_error_and_a_b_a_are_monotone_owner_latches() {
        let (tx, mut rx) = mpsc::channel(1);
        let invalid = Arc::new(AtomicBool::new(false));
        let signal = Signal {
            invalid: invalid.clone(),
            closed: AtomicBool::new(false),
            wake: tx,
            callback: Mutex::new(None),
        };
        let plan = RwLock::new(plan());
        signal.event(
            &plan,
            Ok(event(
                EventKind::Create(CreateKind::File),
                "/receipt/ancestor/lombok.config",
            )),
        );
        signal.event(
            &plan,
            Ok(event(
                EventKind::Remove(RemoveKind::File),
                "/receipt/ancestor/lombok.config",
            )),
        );
        assert!(invalid.load(Ordering::Acquire));
        assert_eq!(rx.recv().await, Some(()));
        assert!(rx.try_recv().is_err());
        signal.event(&plan, Err(notify::Error::generic("native error")));
        assert!(invalid.load(Ordering::Acquire));
        let rescan = Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan);
        assert!(plan.read().affected(&rescan));
        assert!(
            plan.read().affected(&Event::new(EventKind::Other)),
            "pathless callback is not a usable coverage report"
        );
        signal.closed.store(true, Ordering::Release);
        assert!(invalid.load(Ordering::Acquire));
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires genuine KTLSP_WATCH_INPUT_SNAPSHOT recipe inputs"]
    fn selected_compiler_physical_registration_counts() {
        let snapshot: serde_json::Value = serde_json::from_slice(
            &std::fs::read(std::env::var_os("KTLSP_WATCH_INPUT_SNAPSHOT").unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(snapshot["schema"], 1);
        for (name, recipe) in snapshot["recipes"].as_object().unwrap() {
            let inputs: Vec<PathBuf> = serde_json::from_value(recipe["inputs"].clone()).unwrap();
            let plan = WatchPlan::read_inputs(&inputs).unwrap();
            let mut previous = plan.clone();
            for input in &inputs {
                assert!(plan.covers_read(&crate::paths::file_key(input), input.is_dir()));
                for parent in input.ancestors().skip(1) {
                    assert!(plan.ancestors.contains(&key(parent)));
                    if parent.is_dir() {
                        previous.add_directory(parent).unwrap();
                    }
                }
            }
            assert_eq!(
                previous.directories.len() as u64,
                recipe["old_native_directories"].as_u64().unwrap()
            );
            assert!(plan.directories.len() < previous.directories.len());
            eprintln!(
                "native_watch_registration_measurement {}",
                serde_json::json!({
                "recipe":name,"inputs":inputs.len(),"before":previous.directories.len(),
                "after":plan.directories.len(),"recursive_read_trees":plan.recursive.len(),
                "logical_readset_preserved":true})
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn physical_parent_guards_preserve_negative_reads_and_root_replacements() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp-parent-guards-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let parent = root.join("a");
        let directory = parent.join("b");
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("Input.java");
        std::fs::write(&file, b"input").unwrap();
        let missing = directory.join("missing/input.json");
        for input in [&file, &missing] {
            let plan = WatchPlan::read_inputs(&[input.clone()]).unwrap();
            assert_eq!(
                plan.directories,
                BTreeSet::from([directory.clone(), parent.clone()])
            );
            assert_eq!(plan.ancestors, input.ancestors().skip(1).map(key).collect());
            let arriving = if input == &missing {
                directory.join("missing")
            } else {
                file.clone()
            };
            assert!(
                plan.affected(&Event::new(EventKind::Create(CreateKind::Any)).add_path(arriving))
            );
            assert!(plan.affected(
                &Event::new(EventKind::Modify(ModifyKind::Name(
                    notify::event::RenameMode::From
                )))
                .add_path(directory.clone())
            ));
        }
        let tree = WatchPlan::read_inputs(&[directory.clone()]).unwrap();
        assert_eq!(
            tree.directories,
            BTreeSet::from([directory.clone(), parent.clone()])
        );
        assert!(tree.covers_read(
            &crate::paths::file_key(&directory.join("new/Child.java")),
            false
        ));
        let request = crate::compile::observed_test_request(&directory);
        let guard_only = WatchPlan::build(&request, &[]).unwrap();
        assert_eq!(
            guard_only.directories,
            BTreeSet::from([parent, root.clone()])
        );
        assert!(guard_only.affected(
            &Event::new(EventKind::Modify(ModifyKind::Name(
                notify::event::RenameMode::From
            )))
            .add_path(directory)
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn finite_external_directory_plan_compacts_only_the_declared_read_tree() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp-compiled-watch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("input/a/b")).unwrap();
        std::fs::create_dir_all(root.join("sibling/deep/child")).unwrap();
        std::fs::write(root.join("input/a/b/member.class"), b"class").unwrap();
        let mut plan = WatchPlan::default();
        plan.add_input(&root.join("input"), true).unwrap();
        assert_eq!(
            plan.directories.contains(&root.join("input/a/b")),
            !cfg!(windows)
        );
        assert!(plan.includes_directory_watch(&root.join("input/a/b")));
        if cfg!(windows) {
            assert_eq!(plan.recursive, BTreeSet::from([root.join("input")]));
        }
        assert!(plan.directories.len() <= MAX_WATCH_DIRS);
        assert!(plan.affected(
            &Event::new(EventKind::Create(CreateKind::Folder)).add_path(root.join("input/new"))
        ));
        // Planner never enumerates siblings of a readset root, even when their parent is watched.
        assert!(!plan.directories.contains(&root.join("sibling")));
        assert!(!plan.directories.contains(&root.join("sibling/deep/child")));
        assert!(!plan.affected(
            &Event::new(EventKind::Create(CreateKind::File))
                .add_path(root.join("sibling/deep/child/generated.class"))
        ));
        let mut nested = WatchPlan::default();
        nested.add_input(&root.join("input/a"), true).unwrap();
        plan.merge(nested).unwrap();
        if cfg!(windows) {
            assert_eq!(plan.recursive, BTreeSet::from([root.join("input")]));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn native_read_tree_upgrade_keeps_nested_edits_read_tree_rename_and_output_filtering() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp-recursive-input-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("original/input/a/b")).unwrap();
        std::fs::create_dir_all(root.join("generated")).unwrap();
        let input = root.join("original/input");
        let file = input.join("a/b/Input.java");
        std::fs::write(&file, b"A").unwrap();
        for operation in 0..2 {
            let invalid = Arc::new(AtomicBool::new(false));
            let (wake, mut events) = mpsc::channel(1);
            let watch = NativeInputWatch::new(
                invalid.clone(),
                WatchPlan::read_inputs(&[file.clone()]).unwrap(),
                wake,
            )
            .unwrap();
            watch
                .extend(WatchPlan::read_inputs(&[input.clone()]).unwrap())
                .unwrap();
            assert!(watch.plan.read().recursive.contains(&input));
            assert!(!watch.plan.read().directories.contains(&input.join("a/b")));
            std::fs::write(root.join("generated/Output.class"), b"unrelated").unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            assert!(!invalid.load(Ordering::Acquire));
            if operation == 0 {
                std::fs::create_dir_all(input.join("new/nested")).unwrap();
                std::fs::write(input.join("new/nested/New.java"), b"B").unwrap();
            } else {
                // This also proves the old deeper directory handles were retired by extend.
                std::fs::rename(&input, root.join("original/renamed-input")).unwrap();
            }
            assert_eq!(
                tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
                    .await
                    .unwrap(),
                Some(())
            );
            assert!(invalid.load(Ordering::Acquire));
            watch.close();
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rescan_error_and_directory_cap_reject_independently() {
        let (wake, _events) = mpsc::channel(1);
        let invalid = Arc::new(AtomicBool::new(false));
        let signal = Signal {
            invalid: invalid.clone(),
            closed: AtomicBool::new(false),
            wake,
            callback: Mutex::new(None),
        };
        signal.event(
            &RwLock::new(plan()),
            Err(notify::Error::generic("watch failed")),
        );
        assert!(invalid.load(Ordering::Acquire));
        let (wake, _events) = mpsc::channel(1);
        let invalid = Arc::new(AtomicBool::new(false));
        let signal = Signal {
            invalid: invalid.clone(),
            closed: AtomicBool::new(false),
            wake,
            callback: Mutex::new(None),
        };
        signal.event(
            &RwLock::new(plan()),
            Ok(Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan)),
        );
        assert!(invalid.load(Ordering::Acquire));
        let mut capped = WatchPlan::default();
        for n in 0..MAX_WATCH_DIRS {
            capped
                .add_directory(Path::new(&format!("watch-{n}")))
                .unwrap();
        }
        assert!(capped.add_directory(Path::new("one-too-many")).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn deep_read_tree_keeps_directory_cap_without_losing_descendant_coverage() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp-watch-tree-cap-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let input = root.join("input");
        for n in 0..MAX_WATCH_DIRS + 1 {
            std::fs::create_dir_all(input.join(format!("child-{n}"))).unwrap();
        }
        let plan = WatchPlan::read_inputs(&[input.clone()]).unwrap();
        assert_eq!(plan.recursive, BTreeSet::from([input.clone()]));
        assert!(plan.directories.len() <= MAX_WATCH_DIRS);
        assert!(plan.includes_directory_watch(&input.join("child-512")));
        assert!(plan.affected(
            &Event::new(EventKind::Create(CreateKind::File))
                .add_path(input.join("child-512/New.java"))
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn separate_compiler_recipe_watches_invalidate_one_consumer_and_release_both() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp-recipe-watches-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("consumer")).unwrap();
        std::fs::create_dir_all(root.join("producer")).unwrap();
        let consumer_input = root.join("consumer/input.jar");
        let producer_input = root.join("producer/processor.jar");
        std::fs::write(&consumer_input, b"consumer").unwrap();
        std::fs::write(&producer_input, b"producer").unwrap();
        let invalid = Arc::new(AtomicBool::new(false));
        let (wake, mut events) = mpsc::channel(1);
        let consumer = NativeInputWatch::new(
            invalid.clone(),
            WatchPlan::read_inputs(&[consumer_input]).unwrap(),
            wake.clone(),
        )
        .unwrap();
        let producer = NativeInputWatch::new(
            invalid.clone(),
            WatchPlan::read_inputs(&[producer_input.clone()]).unwrap(),
            wake,
        )
        .unwrap();
        std::fs::write(root.join("producer/unrelated-output.class"), b"output").unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(!invalid.load(Ordering::Acquire));
        std::fs::write(&producer_input, b"changed processor").unwrap();
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
                .await
                .unwrap(),
            Some(())
        );
        assert!(consumer.signal.invalid.load(Ordering::Acquire));
        consumer.close();
        producer.close();
        for watch in [consumer, producer] {
            assert!(watch.watcher.lock().is_none());
            assert!(watch.recursive_watcher.lock().is_none());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn native_negative_ancestor_file_replace_and_parent_rename_are_observed() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp-native-watch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("project")).unwrap();
        std::fs::create_dir_all(root.join("external")).unwrap();
        for operation in 0..3 {
            let request = crate::compile::observed_test_request(&root.join("project"));
            let input = root.join(format!("external/case-{operation}/lombok.config"));
            if operation != 0 {
                std::fs::create_dir_all(input.parent().unwrap()).unwrap();
            }
            if operation == 1 {
                std::fs::write(&input, b"A").unwrap();
            }
            let plan = WatchPlan::build(&request, &[crate::paths::file_key(&input)]).unwrap();
            let (wake, mut events) = mpsc::channel(1);
            let watch = NativeCompileWatch::new(request.clone(), plan, wake).unwrap();
            std::fs::write(root.join("external/unrelated-sibling.txt"), b"unrelated").unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            assert!(
                !request.invalid.load(Ordering::Acquire),
                "unrelated sibling must not invalidate"
            );
            match operation {
                0 => std::fs::create_dir_all(input.parent().unwrap()).unwrap(),
                1 => {
                    std::fs::remove_file(&input).unwrap();
                    std::fs::write(&input, b"B").unwrap();
                }
                _ => std::fs::rename(
                    input.parent().unwrap(),
                    root.join("external/renamed-parent"),
                )
                .unwrap(),
            }
            assert_eq!(
                tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
                    .await
                    .unwrap(),
                Some(())
            );
            assert!(request.invalid.load(Ordering::Acquire));
            watch.close();
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn project_config_and_declared_ap_inputs_are_native_but_source_output_events_are_delegated() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp-project-input-plan-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("src/pkg")).unwrap();
        std::fs::create_dir_all(root.join("ap")).unwrap();
        std::fs::create_dir_all(root.join("build/classes/java/main")).unwrap();
        let request = crate::compile::observed_test_request(&root);
        let inputs = [
            root.join("src"),
            root.join("lombok.config"),
            root.join("src/pkg/lombok.config"),
            root.join("ap/options.json"),
            root.join("ap/options.xml"),
            root.join("build/classes/java/main"),
        ]
        .map(|path| crate::paths::file_key(&path));
        let plan = WatchPlan::build(&request, &inputs).unwrap();
        for path in [
            "lombok.config",
            "src/pkg/lombok.config",
            "ap/options.json",
            "ap/options.xml",
        ] {
            assert!(
                plan.affected(
                    &Event::new(EventKind::Create(CreateKind::File)).add_path(root.join(path))
                ),
                "{path}"
            );
        }
        for path in [
            "src/pkg/Consumer.java",
            "src/pkg/Consumer.kt",
            "build/classes/java/main/Person.class",
        ] {
            assert!(
                !plan.affected(
                    &Event::new(EventKind::Modify(ModifyKind::Data(
                        notify::event::DataChange::Any
                    )))
                    .add_path(root.join(path))
                ),
                "model source/output must use existing LSP path: {path}"
            );
        }
        assert!(!plan.affected(
            &Event::new(EventKind::Create(CreateKind::File)).add_path(root.join("unrelated.json"))
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn native_project_lombok_config_and_declared_json_xml_changes_invalidate() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp-native-project-input-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("src/pkg")).unwrap();
        std::fs::create_dir_all(root.join("ap")).unwrap();
        std::fs::create_dir_all(root.join("build/classes/java/main")).unwrap();
        std::fs::write(root.join("src/pkg/Consumer.java"), b"class Consumer {} ").unwrap();
        std::fs::write(
            root.join("build/classes/java/main/Person.class"),
            b"old-output",
        )
        .unwrap();
        for input in ["lombok.config", "ap/options.json", "ap/options.xml"] {
            std::fs::write(root.join(input), b"A").unwrap();
        }
        for (operation, input) in [
            "lombok.config",
            "src/pkg/lombok.config",
            "ap/options.json",
            "ap/options.xml",
        ]
        .into_iter()
        .enumerate()
        {
            let request = crate::compile::observed_test_request(&root);
            let inputs = [
                root.join("src"),
                root.join(input),
                root.join("build/classes/java/main"),
            ]
            .map(|path| crate::paths::file_key(&path));
            let (wake, mut events) = mpsc::channel(1);
            let watch = NativeCompileWatch::new(
                request.clone(),
                WatchPlan::build(&request, &inputs).unwrap(),
                wake,
            )
            .unwrap();
            std::fs::write(
                root.join("src/pkg/Consumer.java"),
                b"class Consumer { int edited; }",
            )
            .unwrap();
            std::fs::write(
                root.join("build/classes/java/main/Person.class"),
                b"new-output",
            )
            .unwrap();
            std::fs::write(root.join("unrelated.json"), b"ignored sibling").unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            assert!(
                !request.invalid.load(Ordering::Acquire),
                "source/output/sibling must not be native invalidations"
            );
            if operation == 2 {
                std::fs::remove_file(root.join(input)).unwrap();
            }
            std::fs::write(root.join(input), b"B").unwrap();
            assert_eq!(
                tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
                    .await
                    .unwrap(),
                Some(())
            );
            assert!(
                request.invalid.load(Ordering::Acquire),
                "project input {input} must invalidate"
            );
            watch.close();
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn generated_registration_is_not_an_ap_read_and_does_not_mask_actual_input_overlap() {
        let root = std::env::temp_dir().join("ktlsp-generated-role-watch-plan");
        let mut request = crate::compile::observed_test_request(&root);
        let input = Arc::get_mut(&mut request).unwrap();
        let generated = root.join("build/generated/annotationProcessor");
        input.source_roots.push(crate::paths::file_key(&generated));
        input.task.declared_output_roots.push(generated.clone());
        input.compiler_tasks[0] = input.task.clone();
        let paths = [crate::paths::file_key(&generated)];
        let plan = WatchPlan::build(&request, &paths).unwrap();
        assert!(
            !plan.affected(
                &Event::new(EventKind::Create(CreateKind::File))
                    .add_path(generated.join("Generated.java"))
            ),
            "self output is checked by the observed task receipt and LSP path"
        );
        let input = Arc::get_mut(&mut request).unwrap();
        input.task.annotation_processor_path.push(generated);
        input.compiler_tasks[0] = input.task.clone();
        assert!(
            WatchPlan::build(&request, &paths).is_err(),
            "an actual AP read must never be masked as output"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn partial_native_arm_failure_cannot_leave_a_current_owner() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp-partial-watch-arm-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("aa-valid")).unwrap();
        let request = crate::compile::observed_test_request(&root);
        let plan = WatchPlan {
            directories: BTreeSet::from([root.join("aa-valid"), root.join("zz-missing")]),
            ..WatchPlan::default()
        };
        let (wake, mut events) = mpsc::channel(1);
        let result = NativeCompileWatch::new(request.clone(), plan, wake);
        assert!(result.is_err());
        assert!(request.invalid.load(Ordering::Acquire));
        assert_eq!(events.recv().await, Some(()));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn external_model_source_output_are_not_delegated_without_client_coverage_proof() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp-external-model-input-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for path in ["project/src", "external-src", "external-output"] {
            std::fs::create_dir_all(root.join(path)).unwrap();
        }
        let mut request = crate::compile::observed_test_request(&root.join("project"));
        let input = Arc::get_mut(&mut request).unwrap();
        input
            .source_roots
            .push(crate::paths::file_key(&root.join("external-src")));
        input.task.output_roots.push(root.join("external-output"));
        input
            .task
            .declared_output_roots
            .push(root.join("external-output"));
        input.compiler_tasks[0] = input.task.clone();
        let paths = [
            root.join("project/src"),
            root.join("external-src"),
            root.join("external-output"),
        ]
        .map(|path| crate::paths::file_key(&path));
        let plan = WatchPlan::build(&request, &paths).unwrap();
        for path in ["external-src/Consumer.java", "external-output/Person.class"] {
            assert!(
                plan.affected(
                    &Event::new(EventKind::Modify(ModifyKind::Data(
                        notify::event::DataChange::Any
                    )))
                    .add_path(root.join(path))
                ),
                "unproven external LSP watch must retain native invalidation: {path}"
            );
        }
        assert!(!plan.affected(
            &Event::new(EventKind::Modify(ModifyKind::Data(
                notify::event::DataChange::Any
            )))
            .add_path(root.join("project/src/Consumer.java"))
        ));
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod filtered_root_watch_tests {
    use super::*;
    #[cfg(windows)]
    use ktcore::extensions::HostInputGuard;
    use notify::event::{CreateKind,DataChange,MetadataKind,ModifyKind,RemoveKind,RenameMode};

    fn root_fixture() -> PathBuf {
        let root=std::env::temp_dir().join(format!("ktlsp-filtered-root-{}-{}",std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&root).unwrap();root
    }
    fn plan(root: &Path) -> WatchPlan {
        WatchPlan {trees:BTreeSet::from([key(root)]),
            ancestors:root.ancestors().skip(1).map(key).collect(),
            filtered_roots:vec![(key(root),BTreeSet::from(["vendor".into()]))],
            ..WatchPlan::default()}
    }

    #[test]
    fn filtered_root_events_keep_ambiguity_boundaries_and_read_roles_conservative() {
        let root=std::env::temp_dir().join("ktlsp-filtered-root-plan-only");let plan=plan(&root);
        let excluded=root.join("vendor/nested/config.json");
        for kind in [EventKind::Create(CreateKind::File),EventKind::Remove(RemoveKind::File),
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),EventKind::Create(CreateKind::Folder)] {
            assert!(!plan.affected(&Event::new(kind).add_path(excluded.clone())),"{kind:?}");
        }
        for kind in [EventKind::Any,EventKind::Other,EventKind::Create(CreateKind::Any),
            EventKind::Remove(RemoveKind::Any),EventKind::Modify(ModifyKind::Any),
            EventKind::Modify(ModifyKind::Other),EventKind::Modify(ModifyKind::Name(RenameMode::From)),
            EventKind::Modify(ModifyKind::Name(RenameMode::To)),EventKind::Modify(ModifyKind::Name(RenameMode::Both))] {
            assert!(plan.affected(&Event::new(kind).add_path(excluded.clone())),"{kind:?}");
        }
        assert!(plan.affected(&Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Content)))
            .add_path(excluded.clone()).set_flag(notify::event::Flag::Rescan)));
        assert!(plan.affected(&Event::new(EventKind::Other)));
        for path in [root.clone(),root.join("vendor"),root.parent().unwrap().to_path_buf()] {
            assert!(plan.affected(&Event::new(EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any))).add_path(path)));
        }
        for path in [root.join("src/config.json"),root.join("build/config.json"),root.join(".gradle/config.json"),root.join("other/vendor-copy/config.json")] {
            assert!(plan.affected(&Event::new(EventKind::Create(CreateKind::File)).add_path(path)));
        }
        assert!(plan.affected(&Event::new(EventKind::Create(CreateKind::File))
            .add_path(excluded.clone()).add_path(root.join("included.json"))));
        assert!(plan.covers_read(&key(&root.join("included.json")),false));
        assert!(!plan.covers_read(&key(&excluded),false));
        assert!(!plan.covers_read(&key(&root),true),"filtered coverage is not an unfiltered directory read proof");
        assert!(!plan.covers_read(&key(&root.join("included")),true));
    }

    #[test]
    fn filtered_root_plans_reject_composition_in_both_directions() {
        let root=std::env::temp_dir().join("ktlsp-filtered-root-merge-only");
        let plain=WatchPlan {trees:BTreeSet::from([key(&root)]),..WatchPlan::default()};
        let mut filtered=plan(&root);
        assert!(filtered.merge(plain.clone()).is_err());
        let mut unfiltered=plain.clone();assert!(unfiltered.merge(plan(&root)).is_err());
        let mut other=plan(&root);other.filtered_roots[0].1.clear();
        assert!(filtered.merge(other).is_err());
        assert!(!filtered.affected(&Event::new(EventKind::Create(CreateKind::File)).add_path(root.join("vendor/file.json"))),
            "rejected composition must leave the original domain intact");
        assert!(unfiltered.affected(&Event::new(EventKind::Create(CreateKind::File)).add_path(root.join("vendor/file.json"))));
        unfiltered.merge(plain).unwrap();
    }

    #[test]
    fn filtered_root_constructor_bounds_policy_and_native_registrations() {
        let base=root_fixture();let root=base.join("project");
        std::fs::create_dir_all(root.join("vendor/nested")).unwrap();
        std::fs::write(root.join("vendor/nested/config.json"),b"excluded").unwrap();
        for excluded in [BTreeSet::from(["..".into()]),BTreeSet::from(["a/b".into()]),
            BTreeSet::from(["a\\b".into()]),BTreeSet::from(["*".into()]),
            (0..129).map(|index|format!("dir{index}")).collect()] {
            assert!(WatchPlan::filtered_root(&root,&excluded).is_err());
        }
        assert!(WatchPlan::filtered_root(Path::new("relative"),&BTreeSet::new()).is_err());
        assert!(WatchPlan::filtered_root(&base.join("absent"),&BTreeSet::new()).is_err());
        let result=WatchPlan::filtered_root(&root,&BTreeSet::from(["VENDOR".into()]));
        #[cfg(windows)] {
            let plan=result.unwrap();
            assert_eq!(plan.recursive,BTreeSet::from([root.clone()]));
            assert_eq!(plan.directories,BTreeSet::from([root.clone(),base.clone()]));
            assert!(plan.exact.is_empty() && plan.memberships.is_empty());
            assert!(plan.excluded_descendant(&key(&root),&key(&root.join("vendor/nested/config.json"))));
        }
        #[cfg(not(windows))]
        assert!(result.err().unwrap().to_string().contains("bounded-root-recursion-unavailable"));
        std::fs::remove_dir_all(base).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn filtered_root_native_lease_precedes_walk_and_observes_add_change_delete_rename_and_replacement() {
        for operation in 0..5 {
            let base=root_fixture();let root=base.join("project");
            std::fs::create_dir_all(root.join("vendor/nested")).unwrap();
            std::fs::create_dir(root.join("included")).unwrap();
            let file=root.join("included/config.json");std::fs::write(&file,b"before").unwrap();
            let excluded=root.join("vendor/nested/cache.json");std::fs::write(&excluded,b"before").unwrap();
            let plan=WatchPlan::filtered_root(&root,&BTreeSet::from(["vendor".into()])).unwrap();
            let (observed,events)=std::sync::mpsc::channel();
            let lease=HostInputLease::new(plan,Arc::new(move || {let _=observed.send(());})).unwrap();
            assert!(lease.is_current());
            // The first traversal happens after this mutation, with the lease already armed.
            match operation {
                0=>std::fs::write(root.join("included/new.json"),b"new").unwrap(),
                1=>std::fs::write(&file,b"changed").unwrap(),
                2=>std::fs::remove_file(&file).unwrap(),
                3=>std::fs::rename(&file,root.join("included/renamed.json")).unwrap(),
                _=>{std::fs::rename(&root,base.join("old-project")).unwrap();std::fs::create_dir(&root).unwrap();}
            }
            let _walked=walkdir::WalkDir::new(&root).follow_links(false).into_iter().collect::<Result<Vec<_>,_>>().unwrap();
            events.recv_timeout(std::time::Duration::from_secs(5)).expect("relevant edit before traversal must retire the lease");
            assert!(!lease.is_current());
            lease.watch.as_ref().unwrap().close();drop(lease);
            std::fs::remove_dir_all(base).unwrap();
        }
    }

    #[cfg(windows)]
    #[test]
    fn filtered_root_native_ambiguous_excluded_write_still_retires_the_lease() {
        let base=root_fixture();let root=base.join("project");
        std::fs::create_dir_all(root.join("vendor/nested")).unwrap();
        let excluded=root.join("vendor/nested/cache.json");std::fs::write(&excluded,b"before").unwrap();
        let plan=WatchPlan::filtered_root(&root,&BTreeSet::from(["vendor".into()])).unwrap();
        let (observed,events)=std::sync::mpsc::channel();
        let lease=HostInputLease::new(plan,Arc::new(move || {let _=observed.send(());})).unwrap();
        // The vendored Windows backend emits Modify(Any), not a proven data-only notification.
        std::fs::write(&excluded,b"changed").unwrap();
        events.recv_timeout(std::time::Duration::from_secs(5)).expect("ambiguous native events must stay conservative");
        assert!(!lease.is_current());lease.watch.as_ref().unwrap().close();drop(lease);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn filtered_root_replacement_between_plan_and_arm_cannot_mint_a_current_lease() {
        let base=root_fixture();let root=base.join("project");std::fs::create_dir(&root).unwrap();
        let plan=WatchPlan::filtered_root(&root,&BTreeSet::new()).unwrap();
        std::fs::remove_dir(&root).unwrap();std::fs::write(&root,b"now a file").unwrap();
        let (observed,events)=std::sync::mpsc::channel();
        assert!(HostInputLease::new(plan,Arc::new(move || {let _=observed.send(());})).is_err());
        events.recv_timeout(std::time::Duration::from_secs(5)).expect("failed arm must invalidate its owner");
        std::fs::remove_dir_all(base).unwrap();
    }
}

#[cfg(test)]
mod selected_kotlin_read_watch_tests {
    use super::*;
    use notify::event::{DataChange, ModifyKind};

    #[test]
    fn new_directory_coverage_accounts_for_native_recursive_support() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp-java-domain-watch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = root.join("sources");
        let existing = source.join("existing");
        std::fs::create_dir_all(&existing).unwrap();
        let plan = WatchPlan::read_inputs(&[source.clone()]).unwrap();
        assert!(plan.includes_directory_watch(&source));
        assert!(plan.includes_directory_watch(&existing));
        let arrived = existing.join("new/nested");
        std::fs::create_dir_all(&arrived).unwrap();
        assert!(
            plan.covers_read(&crate::paths::file_key(&arrived), true),
            "new descendants remain in the logical input read-tree"
        );
        assert_eq!(plan.includes_directory_watch(&arrived), cfg!(windows));
        assert!(plan.affected(
            &Event::new(EventKind::Create(notify::event::CreateKind::Folder))
                .add_path(existing.join("new"))
        ));
        let current = WatchPlan::read_inputs(&[source]).unwrap();
        assert!(current.includes_directory_watch(&arrived));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retained_binary_read_plan_does_not_restore_authority_after_a_b_a() {
        let root = std::env::temp_dir().join("ktlsp-kotlin-read-plan-only");
        let runtime = root.join("runtime/compiler.jar");
        let plugin = root.join("plugin/scripting.jar");
        let sdk = root.join("jdk/lib/modules");
        let plan = WatchPlan::read_inputs(&[runtime.clone(), plugin.clone(), sdk.clone()]).unwrap();
        assert!(plan.covers_read(&crate::paths::file_key(&runtime), false));
        assert!(plan.covers_read(&crate::paths::file_key(&plugin), false));
        assert!(plan.covers_read(&crate::paths::file_key(&sdk), false));
        assert!(!plan.covers_read(&crate::paths::file_key(&root.join("unread.jar")), false));
        let (wake, mut events) = mpsc::channel(1);
        let invalid = Arc::new(AtomicBool::new(false));
        let signal = Signal {
            invalid: invalid.clone(),
            closed: AtomicBool::new(false),
            wake,
            callback: Mutex::new(None),
        };
        let shared = RwLock::new(plan);
        for path in [runtime.clone(), runtime, plugin, sdk] {
            signal.event(
                &shared,
                Ok(
                    Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Content)))
                        .add_path(path),
                ),
            );
            assert!(
                invalid.load(Ordering::Acquire),
                "restored bytes/mtime cannot reverse an input event"
            );
        }
        assert!(events.try_recv().is_ok());
        assert!(
            events.try_recv().is_err(),
            "one bounded wake survives repeated replacement events"
        );
    }
}
