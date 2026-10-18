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
const MAX_INPUT_PATHS: usize = 4096;
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
    exact: BTreeSet<String>,
    trees: BTreeSet<String>,
    ancestors: BTreeSet<String>,
    memberships: BTreeSet<String>,
    delegated_sources: BTreeSet<String>,
    delegated_outputs: BTreeSet<String>,
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
        plan.guard_parents(&request.root)?;
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
            plan.guard_parents(directory)?;
        }
        Ok(plan)
    }

    /// Concrete nonrecursive directory registrations in this immutable plan. After extend
    /// succeeds, a current read traversal may not rely on a directory omitted during planning.
    pub(crate) fn includes_directory_watch(&self, path: &Path) -> bool {
        self.directories.contains(path)
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
        self.trees.iter().any(|root| path_under(&path, root))
            || (!directory && (self.exact.contains(&path) || self.is_delegated_source(&path)))
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
        self.guard_parents(path)?;
        if metadata.as_ref().is_some_and(|m| m.is_dir()) {
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
                if entry.file_type().is_dir() {
                    self.add_directory(entry.path())?;
                }
            }
        }
        Ok(())
    }

    fn guard_parents(&mut self, path: &Path) -> anyhow::Result<()> {
        // Also watch each existing parent nonrecursively: parent removal/rename can otherwise
        // make a child watch silently disappear. Missing-ancestor arrival invalidates the receipt.
        for parent in path.ancestors().skip(1) {
            self.ancestors.insert(key(parent));
            match std::fs::symlink_metadata(parent) {
                Ok(metadata) => {
                    anyhow::ensure!(
                        !metadata.file_type().is_symlink(),
                        "compiler-watch-parent-symlink"
                    );
                    anyhow::ensure!(metadata.is_dir(), "compiler-watch-parent-not-directory");
                    self.add_directory(parent)?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    fn add_directory(&mut self, path: &Path) -> anyhow::Result<()> {
        self.directories.insert(path.to_path_buf());
        anyhow::ensure!(
            self.directories.len() <= MAX_WATCH_DIRS,
            "compiler-native-watch-cap"
        );
        Ok(())
    }

    fn merge(&mut self, next: Self) -> anyhow::Result<()> {
        self.exact.extend(next.exact);
        self.trees.extend(next.trees);
        self.ancestors.extend(next.ancestors);
        self.memberships.extend(next.memberships);
        self.delegated_sources.extend(next.delegated_sources);
        self.delegated_outputs.extend(next.delegated_outputs);
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
            self.trees.iter().any(|root| path_under(&key, root))
                || (ancestor_change && self.ancestors.contains(&key))
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
}

impl Signal {
    fn invalidate(&self) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        self.invalid.store(true, Ordering::Release);
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
            self.invalidate();
        }
    }
}

pub(crate) struct NativeInputWatch {
    watcher: Mutex<Option<RecommendedWatcher>>,
    plan: Arc<RwLock<WatchPlan>>,
    signal: Arc<Signal>,
}

impl NativeInputWatch {
    pub(crate) fn new(
        invalid: Arc<AtomicBool>,
        plan: WatchPlan,
        wake: mpsc::Sender<()>,
    ) -> anyhow::Result<Arc<Self>> {
        let signal = Arc::new(Signal {
            invalid,
            closed: AtomicBool::new(false),
            wake,
        });
        let shared = Arc::new(RwLock::new(plan.clone()));
        let callback_signal = signal.clone();
        let callback_plan = shared.clone();
        let mut watcher = match notify::recommended_watcher(move |event| {
            callback_signal.event(&callback_plan, event)
        }) {
            Ok(watcher) => watcher,
            Err(error) => {
                signal.invalidate();
                return Err(error.into());
            }
        };
        for directory in &plan.directories {
            if let Err(error) = watcher.watch(directory, RecursiveMode::NonRecursive) {
                signal.invalidate();
                return Err(error.into());
            }
        }
        Ok(Arc::new(Self {
            watcher: Mutex::new(Some(watcher)),
            plan: shared,
            signal,
        }))
    }

    pub(crate) fn extend(&self, next: WatchPlan) -> anyhow::Result<()> {
        let previous = self.plan.read().directories.clone();
        let mut merged = self.plan.read().clone();
        merged.merge(next)?;
        *self.plan.write() = merged.clone();
        let mut guard = self.watcher.lock();
        let watcher = guard
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("compiler-watch-closed"))?;
        for directory in merged.directories.difference(&previous) {
            if let Err(error) = watcher.watch(directory, RecursiveMode::NonRecursive) {
                self.signal.invalidate();
                return Err(error.into());
            }
        }
        Ok(())
    }

    pub(crate) fn close(&self) {
        // Removing native coverage cannot leave its accepted owner current.
        self.signal.invalidate();
        self.signal.closed.store(true, Ordering::Release);
        let watcher = self.watcher.lock().take();
        // Never join the notify thread from its callback or while holding Workspace's mutex.
        drop(watcher);
    }
}

impl Drop for NativeInputWatch {
    fn drop(&mut self) {
        self.close();
    }
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

    #[test]
    fn finite_external_directory_plan_has_only_immediate_native_watches() {
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
        assert!(plan.directories.contains(&root.join("input/a/b")));
        assert!(plan.directories.len() <= MAX_WATCH_DIRS);
        assert!(plan.affected(
            &Event::new(EventKind::Create(CreateKind::Folder)).add_path(root.join("input/new"))
        ));
        // Planner never enumerates siblings of a readset root, even when their parent is watched.
        assert!(!plan.directories.contains(&root.join("sibling")));
        assert!(!plan.directories.contains(&root.join("sibling/deep/child")));
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
        assert!(plan.directories.contains(&root));
        assert!(plan.directories.contains(&root.join("src/pkg")));
        assert!(!plan
            .directories
            .contains(&root.join("build/classes/java/main")));
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
mod selected_kotlin_read_watch_tests {
    use super::*;
    use notify::event::{DataChange, ModifyKind};

    #[test]
    fn current_directory_membership_cannot_rely_on_a_directory_omitted_during_planning() {
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
            "logical read-tree coverage alone is insufficient for nonrecursive registrations"
        );
        assert!(!plan.includes_directory_watch(&arrived));
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
