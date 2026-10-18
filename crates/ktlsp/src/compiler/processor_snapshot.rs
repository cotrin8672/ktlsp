//! Private current-input materialization for the fixed stock processor chain.
use super::*;

const MAX_FILES: usize = 8192;
const MAX_BYTES: usize = 64 * 1024 * 1024;

struct Snapshot {
    owner: KotlinEditorOverlay,
    root: PathBuf,
    authored: BTreeMap<PathBuf, String>,
    authored_lease: SealedOutput,
    original_root: PathBuf,
    module_dirs: Vec<PathBuf>,
    original_content: BTreeMap<PathBuf, Option<String>>,
    original_directories: BTreeSet<PathBuf>,
    _original_watch: Arc<crate::compiled_watch::NativeInputWatch>,
    artifact_key: String,
}

pub(crate) struct CachedArtifacts {
    _owner: KotlinEditorOverlay,
    root: PathBuf,
    key: String,
    outputs: SealedOutput,
    excluded_tasks: Vec<String>,
    authored_reads: Vec<(PathBuf, bool, BTreeMap<PathBuf, String>)>,
}

fn project_path(path: &Path, original: &Path, private: &Path) -> anyhow::Result<PathBuf> {
    if path.starts_with(original) {
        let relative = path.strip_prefix(original)?;
        anyhow::ensure!(
            relative
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
            "processor-private-path-escape"
        );
        Ok(private.join(relative))
    } else {
        Ok(path.to_owned())
    }
}

fn rebase<T: serde::Serialize + serde::de::DeserializeOwned>(
    value: &T,
    original: &Path,
    private: &Path,
) -> anyhow::Result<T> {
    fn rewrite(value: &mut Value, from: &str, to: &str) {
        match value {
            Value::String(text) => {
                *text = text
                    .replace(from, to)
                    .replace(&from.replace('\\', "/"), &to.replace('\\', "/"));
            }
            Value::Array(values) => values.iter_mut().for_each(|v| rewrite(v, from, to)),
            Value::Object(values) => values.values_mut().for_each(|v| rewrite(v, from, to)),
            _ => {}
        }
    }
    let mut value = serde_json::to_value(value)?;
    rewrite(&mut value, &path_text(original)?, &path_text(private)?);
    Ok(serde_json::from_value(value)?)
}

fn regular(path: &Path) -> anyhow::Result<std::fs::Metadata> {
    let metadata = std::fs::symlink_metadata(path)?;
    anyhow::ensure!(
        !metadata.file_type().is_symlink(),
        "processor-snapshot-symlink-unhandled"
    );
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        anyhow::ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "processor-snapshot-reparse-unhandled"
        );
    }
    Ok(metadata)
}

fn authored_paths(
    root: &Path,
    module_dirs: &[PathBuf],
) -> anyhow::Result<(BTreeSet<PathBuf>, BTreeSet<PathBuf>)> {
    let mut files = BTreeSet::new();
    let mut directories = BTreeSet::new();
    let iterator = walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            entry.path() == root
                || (!matches!(
                    entry.file_name().to_str().unwrap_or(""),
                    ".git" | ".gradle" | ".idea" | ".kotlin" | "node_modules"
                ) && !module_dirs
                    .iter()
                    .any(|module| entry.path() == module.join("build")))
        });
    for entry in iterator {
        let entry = entry?;
        let metadata = regular(entry.path())?;
        if metadata.is_dir() {
            directories.insert(entry.path().to_owned());
        } else {
            anyhow::ensure!(metadata.is_file(), "processor-snapshot-file-kind");
            files.insert(entry.path().to_owned());
        }
        anyhow::ensure!(
            files.len() + directories.len() <= MAX_FILES,
            "processor-snapshot-file-cap"
        );
    }
    Ok((files, directories))
}

impl Snapshot {
    fn create(
        plan: &Plan,
        request: &KotlinDiagnosticRequest,
        environment: &KotlinDiagnosticEnvironment,
    ) -> anyhow::Result<Self> {
        Self::create_with_watch(
            plan,
            request,
            &crate::deps::cache_home().join("processor-editor-compile"),
            environment.watch.clone(),
        )
    }

    #[cfg(test)]
    fn create_in(
        plan: &Plan,
        request: &KotlinDiagnosticRequest,
        parent: &Path,
    ) -> anyhow::Result<Self> {
        let (wake, _receiver) = tokio::sync::mpsc::channel(1);
        let watch = crate::compiled_watch::NativeInputWatch::new(
            request.invalid.clone(),
            crate::compiled_watch::WatchPlan::default(),
            wake,
        )?;
        Self::create_with_watch(plan, request, parent, watch)
    }

    fn create_with_watch(
        plan: &Plan,
        request: &KotlinDiagnosticRequest,
        parent: &Path,
        source_watch: Arc<crate::compiled_watch::NativeInputWatch>,
    ) -> anyhow::Result<Self> {
        let mut owner_request = request.clone();
        owner_request.dependency_overlays.clear();
        let owner = KotlinEditorOverlay::empty_in(&owner_request, parent)?;
        let root = owner.directory.join("project");
        std::fs::create_dir(&root)?;
        let (files, original_directories) = authored_paths(&plan.root, &plan.module_dirs)?;
        let mut watched_files: BTreeSet<_> = files.clone();
        watched_files.extend(
            request
                .overlays
                .iter()
                .chain(&request.dependency_overlays)
                .map(|source| PathBuf::from(&source.key)),
        );
        source_watch.extend(crate::compiled_watch::WatchPlan::read_source_inventory(
            &watched_files.into_iter().collect::<Vec<_>>(),
            &original_directories.iter().cloned().collect::<Vec<_>>(),
        )?)?;
        anyhow::ensure!(
            authored_paths(&plan.root, &plan.module_dirs)?
                == (files.clone(), original_directories.clone()),
            "processor-original-membership-changed-during-watch"
        );
        let mut authored = BTreeMap::new();
        let mut original_content = BTreeMap::new();
        let mut bytes = 0usize;
        for original in &files {
            request.check_live()?;
            anyhow::ensure!(regular(original)?.is_file(), "processor-snapshot-file-kind");
            let original_key = crate::paths::file_key(original);
            let overlay = request
                .overlays
                .iter()
                .chain(&request.dependency_overlays)
                .find(|source| kotlin_path_key(&source.key) == kotlin_path_key(&original_key));
            let content = if let Some(overlay) = overlay {
                use sha2::Digest;
                anyhow::ensure!(
                    <[u8; 32]>::from(Sha256::digest(overlay.text.as_bytes()))
                        == overlay.text_sha256,
                    "processor-overlay-content-identity-differs"
                );
                overlay.text.as_bytes().to_vec()
            } else {
                std::fs::read(original)?
            };
            bytes = bytes
                .checked_add(content.len())
                .ok_or_else(|| anyhow::anyhow!("processor-snapshot-byte-overflow"))?;
            anyhow::ensure!(bytes <= MAX_BYTES, "processor-snapshot-byte-cap");
            let target = root.join(original.strip_prefix(&plan.root)?);
            std::fs::create_dir_all(target.parent().unwrap())?;
            write_new(&target, &content)?;
            original_content.insert(original.clone(), overlay.is_none().then(|| sha(&content)));
            authored.insert(target, sha(&content));
        }
        // Include current new unsaved files in the disposable tree. The actual stock Gradle task
        // resolves its source selection here; a model root alone never certifies their API.
        for overlay in request.overlays.iter().chain(&request.dependency_overlays) {
            let original = Path::new(&overlay.key);
            anyhow::ensure!(
                original.starts_with(&plan.root),
                "processor-overlay-outside-current-build"
            );
            let target = project_path(original, &plan.root, &root)?;
            if authored.contains_key(&target) {
                continue;
            }
            anyhow::ensure!(
                request.sources.iter().any(|s| same_path(s, original))
                    || plan.affects_processor_source(original),
                "processor-unmodeled-dependency-overlay"
            );
            anyhow::ensure!(authored.len() < MAX_FILES, "processor-snapshot-file-cap");
            bytes = bytes
                .checked_add(overlay.text.len())
                .ok_or_else(|| anyhow::anyhow!("processor-snapshot-byte-overflow"))?;
            anyhow::ensure!(bytes <= MAX_BYTES, "processor-snapshot-byte-cap");
            std::fs::create_dir_all(target.parent().unwrap())?;
            write_new(&target, overlay.text.as_bytes())?;
            authored.insert(target, sha(overlay.text.as_bytes()));
        }
        let artifact_key = artifact_key(plan, &root, &authored, &environment_hash())?;
        let authored_lease = SealedOutput::seal(authored.keys().cloned().collect())?;
        let snapshot = Self {
            owner,
            root,
            authored,
            authored_lease,
            original_root: plan.root.clone(),
            module_dirs: plan.module_dirs.clone(),
            original_content,
            original_directories,
            _original_watch: source_watch,
            artifact_key,
        };
        snapshot.check_sources(request)?;
        Ok(snapshot)
    }

    fn check_sources(&self, request: &KotlinDiagnosticRequest) -> anyhow::Result<()> {
        request.check_live()?;
        let (files, directories) = authored_paths(&self.original_root, &self.module_dirs)?;
        anyhow::ensure!(
            files == self.original_content.keys().cloned().collect()
                && directories == self.original_directories,
            "processor-original-source-membership-changed"
        );
        for (path, digest) in &self.original_content {
            if let Some(digest) = digest {
                anyhow::ensure!(
                    sha(&std::fs::read(path)?) == *digest,
                    "processor-original-source-content-changed"
                );
            }
        }
        self.authored_lease.check_live()?;
        for (path, digest) in &self.authored {
            anyhow::ensure!(
                sha(&std::fs::read(path)?) == *digest,
                "processor-current-snapshot-written-by-producer"
            );
        }
        Ok(())
    }
}

fn artifact_key(
    plan: &Plan,
    root: &Path,
    authored: &BTreeMap<PathBuf, String>,
    environment: &str,
) -> anyhow::Result<String> {
    let mut artifact_inputs = Vec::new();
    let consumer_sources = plan.owner.project_dir.join("src");
    for (path, digest) in authored {
        let relative = path.strip_prefix(root)?;
        let original = plan.root.join(relative);
        if original.starts_with(&consumer_sources) {
            continue;
        }
        if matches!(
            original.extension().and_then(|s| s.to_str()),
            Some("kt" | "java")
        ) && !plan.affects_processor_source(&original)
            && plan
                .unselected_source_roots
                .iter()
                .any(|r| original.starts_with(r))
        {
            continue;
        }
        artifact_inputs.push((relative.to_owned(), digest.clone()));
    }
    // Jar/service/source/resource/config identities are dynamic: valid changed processors
    // produce a new key and are built again, rather than rejected by implementation hashes.
    Ok(sha(&serde_json::to_vec(&(
        &plan.processor_modules,
        artifact_inputs,
        environment,
    ))?))
}

impl CachedArtifacts {
    fn matches_current_authored_reads(&self, snapshot: &Snapshot) -> bool {
        self.authored_reads.iter().all(|(path, tree, expected)| {
            authored_read(&snapshot.root, &snapshot.authored, path, *tree) == *expected
        })
    }
    fn copy_to(&self, snapshot: &Snapshot) -> anyhow::Result<SealedOutput> {
        self.outputs.check_live()?;
        let mut roots = Vec::new();
        for (source, fact) in self.outputs.roots.iter().zip(&self.outputs.facts) {
            let target = snapshot.root.join(source.strip_prefix(&self.root)?);
            match fact["kind"].as_str() {
                Some("absent") => {
                    anyhow::ensure!(!target.exists(), "processor-cached-absence-overwritten")
                }
                Some("file") => {
                    std::fs::create_dir_all(target.parent().unwrap())?;
                    write_new(&target, &std::fs::read(source)?)?;
                }
                Some("directory") => {
                    std::fs::create_dir_all(&target)?;
                    for entry in array(fact, "entries")? {
                        let path = Path::new(value_string(entry, "path")?);
                        let destination = target.join(path.strip_prefix(source)?);
                        std::fs::create_dir_all(destination.parent().unwrap())?;
                        write_new(&destination, &std::fs::read(path)?)?;
                    }
                }
                _ => anyhow::bail!("processor-cached-output-kind-unproved"),
            }
            roots.push(target);
        }
        self.outputs.check_live()?;
        SealedOutput::seal(roots)
    }
}

fn authored_read(
    root: &Path,
    authored: &BTreeMap<PathBuf, String>,
    relative: &Path,
    tree: bool,
) -> BTreeMap<PathBuf, String> {
    let selected = root.join(relative);
    authored
        .iter()
        .filter(|(p, _)| same_path(p, &selected) || (tree && p.starts_with(&selected)))
        .map(|(p, digest)| (p.strip_prefix(root).unwrap().to_owned(), digest.clone()))
        .collect()
}

fn bootstrap_authored_reads(
    snapshot: &Snapshot,
    rows: &[Value],
) -> anyhow::Result<Vec<(PathBuf, bool, BTreeMap<PathBuf, String>)>> {
    let mut reads = BTreeMap::new();
    for start in rows
        .iter()
        .filter(|e| e["event"] == "processor-bootstrap-start")
    {
        for read in array(start, "inputs")? {
            let path = Path::new(value_string(read, "path")?);
            if let Ok(relative) = path.strip_prefix(&snapshot.root) {
                // Absence can denote a future file OR directory; retain its membership veto.
                let tree = read["kind"] != "file";
                reads.insert(
                    (relative.to_owned(), tree),
                    authored_read(&snapshot.root, &snapshot.authored, relative, tree),
                );
            }
        }
    }
    Ok(reads
        .into_iter()
        .map(|((path, tree), facts)| (path, tree, facts))
        .collect())
}

fn generated_input(path: &Path, tasks: &Tasks) -> bool {
    [&tasks.ksp, &tasks.stubs, &tasks.kapt]
        .into_iter()
        .flat_map(|t| &t.declared_output_roots)
        .any(|root| path.starts_with(root))
}

/// Preserve all compiler-affecting options and ordered classpaths. Membership is checked against
/// the actual private current source selection and owned generated outputs, not the old saved tree.
fn same_recipe(observed: &CompilerTask, expected: &CompilerTask) -> anyhow::Result<()> {
    let mut actual = observed.clone();
    let mut selected = expected.clone();
    // Pinned KGP 1.8's classpath contributor filters absent declared entries. Private current
    // producer class slots may be absent/empty instead of their previous saved-build state.
    if selected.kotlin_compiler.is_some() {
        if let Some(index) = selected
            .arguments
            .iter()
            .position(|a| a == "-classpath" || a == "-cp")
        {
            anyhow::ensure!(
                index + 1 < selected.arguments.len(),
                "processor-classpath-value-absent"
            );
            selected.arguments[index + 1] = joined_classpath(
                &selected
                    .classpath
                    .iter()
                    .filter(|p| p.exists())
                    .cloned()
                    .collect::<Vec<_>>(),
            )?;
        }
    }
    actual.source_files.clear();
    selected.source_files.clear();
    actual.declared_input_files.clear();
    selected.declared_input_files.clear();
    // Source-policy/claim inventories describe the source membership just resolved by Gradle.
    // They are not effective compiler arguments; validate selected sources separately below.
    for task in [&mut actual, &mut selected] {
        task.options.remove("sdkProducerClaims");
        task.options.remove("sdkSourcePolicy");
    }
    anyhow::ensure!(
        actual == selected,
        "processor-selected-compiler-options-differ"
    );
    Ok(())
}

fn validate_current_inputs(
    snapshot: &Snapshot,
    observed: &CompilerTask,
    expected: &CompilerTask,
    tasks: &Tasks,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        observed.complete && observed.problem.is_none(),
        "processor-current-input-contract-incomplete"
    );
    let selected: BTreeSet<_> = observed
        .source_files
        .iter()
        .map(|p| header_key(p))
        .collect();
    anyhow::ensure!(
        selected.len() == observed.source_files.len(),
        "processor-selected-source-duplicate"
    );
    for path in &expected.source_files {
        if !generated_input(path, tasks) {
            anyhow::ensure!(
                selected.contains(&header_key(path)),
                "processor-authored-source-deselected"
            );
        }
    }
    for path in &observed.source_files {
        if generated_input(path, tasks) {
            continue;
        }
        let digest = snapshot
            .authored
            .get(path)
            .ok_or_else(|| anyhow::anyhow!("processor-selected-source-not-in-current-snapshot"))?;
        anyhow::ensure!(
            sha(&std::fs::read(path)?) == *digest,
            "processor-selected-source-differs"
        );
    }
    Ok(())
}

fn validate_graph(
    rows: &[Value],
    root: &Path,
    tasks: &Tasks,
    primary: &CompilerTask,
    java: &CompilerTask,
) -> anyhow::Result<()> {
    let graphs: Vec<_> = rows
        .iter()
        .filter(|e| e["event"] == "processor-graph")
        .collect();
    anyhow::ensure!(graphs.len() == 1, "processor-graph-ambiguous");
    let graph = graphs[0];
    anyhow::ensure!(
        same_path(Path::new(value_string(graph, "root")?), root)
            && graph["gradleVersion"] == "7.4.2"
            && primary
                .java_home
                .as_ref()
                .is_some_and(|sdk| graph["javaHome"]
                    .as_str()
                    .is_some_and(|p| same_path(Path::new(p), sdk)))
            && graph["requestedTasks"]
                == serde_json::json!([tasks.ksp.task, tasks.stubs.task, tasks.kapt.task])
            && graph["producerCompletion"] == false
            && graph["apiAuthority"] == false,
        "processor-graph-invocation-differs"
    );
    let selected = array(graph, "tasks")?;
    let contracts = array(graph, "contracts")?;
    let names: BTreeSet<_> = selected
        .iter()
        .map(|v| {
            v.as_str()
                .ok_or_else(|| anyhow::anyhow!("processor-graph-task-name"))
        })
        .collect::<anyhow::Result<_>>()?;
    anyhow::ensure!(
        names.len() == selected.len()
            && contracts.len() == selected.len()
            && !names.contains(primary.task.as_str())
            && !names.contains(java.task.as_str()),
        "processor-graph-consumer-or-duplicate-task"
    );
    for (name, contract) in selected.iter().zip(contracts) {
        anyhow::ensure!(
            contract["task"] == *name
                && Path::new(value_string(contract, "projectDir")?).starts_with(root),
            "processor-graph-task-owner-differs"
        );
        for output in array(contract, "declaredOutputPaths")? {
            let path = Path::new(
                output
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("processor-output-path-unavailable"))?,
            );
            anyhow::ensure!(
                path != root
                    && path.starts_with(root)
                    && !path
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir)),
                "processor-declared-output-escapes-current-generation"
            );
        }
    }
    Ok(())
}

/// Current private reads must be authored bytes or the exact closed output of an earlier stock
/// task in THIS invocation. External reads must already be covered by the retained input watch.
fn validate_reads(
    snapshot: &Snapshot,
    plan: &Plan,
    rows: &[Value],
    start: &Value,
    environment: &KotlinDiagnosticEnvironment,
    cached: Option<&SealedOutput>,
) -> anyhow::Result<()> {
    let before = start["sequence"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("processor-read-order-absent"))?;
    let mut produced = BTreeMap::new();
    if let Some(cached) = cached {
        cached.check_live()?;
        for output in &cached.facts {
            let files = if output["kind"] == "file" {
                vec![output]
            } else {
                output
                    .get("entries")
                    .and_then(Value::as_array)
                    .map(|v| v.iter().collect())
                    .unwrap_or_default()
            };
            for file in files {
                produced.insert(
                    header_key(Path::new(value_string(file, "path")?)),
                    value_string(file, "sha256")?.to_owned(),
                );
            }
        }
    }
    for end in rows.iter().filter(|e| {
        e["event"] == "processor-task-end" && e["sequence"].as_u64().is_some_and(|s| s < before)
    }) {
        anyhow::ensure!(
            end["state"]["failure"] == Value::Null,
            "processor-predecessor-failed"
        );
        for output in array(end, "outputs")? {
            let files = if output["kind"] == "file" {
                vec![output]
            } else {
                output
                    .get("entries")
                    .and_then(Value::as_array)
                    .map(|v| v.iter().collect())
                    .unwrap_or_default()
            };
            for file in files {
                produced.insert(
                    header_key(Path::new(value_string(file, "path")?)),
                    value_string(file, "sha256")?.to_owned(),
                );
            }
        }
    }
    for read in array(start, "inputs")? {
        let path = Path::new(value_string(read, "path")?);
        anyhow::ensure!(
            inventory(path)? == *read,
            "processor-execution-boundary-read-changed"
        );
        if !path.starts_with(&snapshot.root) {
            anyhow::ensure!(
                !path.starts_with(&plan.root),
                "processor-current-build-read-escaped-private-snapshot"
            );
            anyhow::ensure!(
                environment
                    .stamps
                    .iter()
                    .any(|(watched, _)| same_path(watched, path)
                        || (watched.is_dir() && path.starts_with(watched))),
                "processor-external-read-not-prearmed"
            );
            continue;
        }
        let original = project_path(path, &snapshot.root, &plan.root)?;
        anyhow::ensure!(
            plan.owns_bootstrap_read(&original, read["kind"] != "file"),
            "processor-bootstrap-read-outside-current-overlay-closure"
        );
        let files = if read["kind"] == "file" {
            vec![read]
        } else {
            read.get("entries")
                .and_then(Value::as_array)
                .map(|v| v.iter().collect())
                .unwrap_or_default()
        };
        for file in files {
            let path = Path::new(value_string(file, "path")?);
            let original = project_path(path, &snapshot.root, &plan.root)?;
            anyhow::ensure!(
                plan.owns_bootstrap_read(&original, false),
                "processor-bootstrap-file-outside-current-overlay-closure"
            );
            let digest = value_string(file, "sha256")?;
            anyhow::ensure!(
                snapshot.authored.get(path).is_some_and(|s| s == digest)
                    || produced.get(&header_key(path)).is_some_and(|s| s == digest),
                "processor-private-read-has-no-current-owner"
            );
        }
    }
    Ok(())
}

fn current_request(
    original: &KotlinDiagnosticRequest,
    plan: &Plan,
    private: &Path,
    primary: CompilerTask,
    java: CompilerTask,
) -> anyhow::Result<KotlinDiagnosticRequest> {
    let mut request = original.clone();
    request.processor_plan = None;
    // The owned stock DAG has now selected the complete current private input set.
    request.java_editor_sources.clear();
    request.project_dir = project_path(&original.project_dir, &plan.root, private)?;
    request.source_roots = original
        .source_roots
        .iter()
        .map(|p| project_path(p, &plan.root, private))
        .collect::<anyhow::Result<_>>()?;
    request.sources = primary.source_files.clone();
    request.task = primary;
    request.java_task = Some(java);
    // Stock tasks selected these current inputs. Keep wire ownership for the Java diagnostic
    // admission check, rebased to the private bytes instead of discarding every open Java file.
    request.overlays = original
        .overlays
        .iter()
        .map(|source| {
            let ktcore::jvm::InputIdentity::Source { revision, .. } = &source.identity else {
                anyhow::bail!("processor-overlay-source-identity-unknown");
            };
            Ok(KotlinSourceOverlay::new(
                crate::paths::file_key(&project_path(Path::new(&source.key), &plan.root, private)?),
                source.text.clone(),
                *revision,
            ))
        })
        .collect::<anyhow::Result<_>>()?;
    request.dependency_overlays.clear();
    request.java_dependencies = plan
        .processor_modules
        .iter()
        .flat_map(|m| &m.source_sets)
        .filter(|s| {
            s.provided_artifacts
                .iter()
                .any(|p| plan.java.classpath.iter().any(|cp| same_path(p, cp)))
        })
        .map(|s| rebase(s, &plan.root, private))
        .collect::<anyhow::Result<_>>()?;
    Ok(request)
}

pub(super) fn run(
    original: &KotlinDiagnosticRequest,
    environment: &KotlinDiagnosticEnvironment,
    plan: &Plan,
) -> anyhow::Result<KotlinDiagnosticOutcome> {
    original.check_live()?;
    environment.check_live()?;
    anyhow::ensure!(
        environment.matches(original),
        "processor-original-recipe-stale"
    );
    plan.tasks.validate(&plan.primary, &plan.java)?;
    let snapshot = Snapshot::create(plan, original, environment)?;
    let private_tasks: Tasks = rebase(&plan.tasks, &plan.root, &snapshot.root)?;
    let expected_primary: CompilerTask = rebase(&plan.primary, &plan.root, &snapshot.root)?;
    let expected_java: CompilerTask = rebase(&plan.java, &plan.root, &snapshot.root)?;
    let mut cache = environment
        .processor_artifacts
        .lock()
        .map_err(|_| anyhow::anyhow!("processor-artifact-cache-poisoned"))?;
    let (excluded, copied_artifacts) = if let Some(cached) = cache.as_ref().filter(|cached| {
        cached.key == snapshot.artifact_key
            && cached.matches_current_authored_reads(&snapshot)
            && cached.outputs.check_live().is_ok()
    }) {
        (
            cached.excluded_tasks.clone(),
            Some(cached.copy_to(&snapshot)?),
        )
    } else {
        (Vec::new(), None)
    };
    let events = snapshot.owner.directory.join("events");
    std::fs::create_dir(&events)?;
    let nonce = sha(format!(
        "{}:{}:{}",
        snapshot.owner.directory.display(),
        original.generation,
        std::process::id()
    )
    .as_bytes());
    let observer = snapshot.owner.directory.join("observer.init.gradle.kts");
    let script = format!(
        "{}\n{}",
        include_str!("../../../../scripts/classpath-dump.init.gradle.kts"),
        OBSERVER
    );
    write_new(&observer, script.as_bytes())?;
    let gradle = resolve_gradle(&snapshot.root)
        .ok_or_else(|| anyhow::anyhow!("processor-wrapper-unavailable"))?;
    let mut command = Command::new(gradle);
    command
        .current_dir(&snapshot.root)
        .args([
            &private_tasks.ksp.task,
            &private_tasks.stubs.task,
            &private_tasks.kapt.task,
        ])
        .args([
            "--console=plain",
            "--no-daemon",
            "--no-build-cache",
            "--no-configuration-cache",
            "--rerun-tasks",
            "--max-workers=1",
        ])
        .arg("--init-script")
        .arg(&observer)
        .arg(format!("-Dktlsp.processor.proof.dir={}", events.display()))
        .arg(format!("-Dktlsp.processor.proof.nonce={nonce}"))
        .arg(format!("-Dktlsp.processor.owner={}", expected_primary.task));
    for task in &excluded {
        command.arg("-x").arg(task);
    }
    // One owned stock DAG, not three Gradle startups. No consumer compileJava/compileKotlin.
    execute_phase(&mut command, original, environment)?;
    if let Some(copied) = &copied_artifacts {
        copied.check_live()?;
    }
    snapshot.check_sources(original)?;
    let rows = phase_events(&events, &nonce, &private_tasks.ksp)?;
    validate_graph(
        &rows,
        &snapshot.root,
        &private_tasks,
        &expected_primary,
        &expected_java,
    )?;
    for start in rows
        .iter()
        .filter(|e| e["event"] == "processor-bootstrap-start")
    {
        validate_reads(
            &snapshot,
            plan,
            &rows,
            start,
            environment,
            copied_artifacts.as_ref(),
        )?;
    }
    for task in [&private_tasks.stubs, &private_tasks.kapt] {
        phase_events(&events, &nonce, task)?;
    }
    let mut last_end = None;
    let mut output_roots = Vec::new();
    let mut generated = BTreeMap::new();
    for task in [
        &private_tasks.ksp,
        &private_tasks.stubs,
        &private_tasks.kapt,
    ] {
        let start = rows
            .iter()
            .find(|e| e["event"] == "processor-start" && e["task"] == task.task)
            .unwrap();
        let end = rows
            .iter()
            .find(|e| e["event"] == "processor-task-end" && e["task"] == task.task)
            .unwrap();
        anyhow::ensure!(
            last_end.is_none_or(|n| start["sequence"].as_u64().is_some_and(|s| s > n)),
            "processor-stock-task-order-differs"
        );
        last_end = end["sequence"].as_u64();
        let current: CompilerTask = serde_json::from_value(start["configuration"].clone())?;
        same_recipe(&current, task)?;
        validate_current_inputs(&snapshot, &current, task, &private_tasks)?;
        validate_reads(
            &snapshot,
            plan,
            &rows,
            start,
            environment,
            copied_artifacts.as_ref(),
        )?;
        // Fresh generation: previous generated sources/classes cannot certify the current API.
        anyhow::ensure!(
            array(start, "outputsBefore")?
                .iter()
                .all(|r| r["kind"] == "absent"),
            "processor-fresh-output-precondition-failed"
        );
        for output in array(end, "outputs")? {
            let path = PathBuf::from(value_string(output, "path")?);
            anyhow::ensure!(
                path.starts_with(&snapshot.root),
                "processor-output-outside-private-generation"
            );
            anyhow::ensure!(
                inventory(&path)? == *output,
                "processor-closed-generated-output-differs"
            );
            output_roots.push(path);
            for entry in output
                .get("entries")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let path = PathBuf::from(value_string(entry, "path")?);
                if matches!(
                    path.extension().and_then(|x| x.to_str()),
                    Some("kt" | "java")
                ) {
                    let digest = value_string(entry, "sha256")?.to_owned();
                    if let Some(previous) = generated.insert(path, digest.clone()) {
                        anyhow::ensure!(
                            previous == digest,
                            "processor-generated-source-owner-differs"
                        );
                    }
                }
            }
        }
    }
    let sealed = SealedOutput::seal(output_roots)?;
    let finished = rows
        .iter()
        .find(|e| e["event"] == "processor-build-finished")
        .unwrap();
    let configurations = array(finished, "selectedCompilerConfigurations")?;
    let select = |name: &str| -> anyhow::Result<CompilerTask> {
        let values: Vec<_> = configurations
            .iter()
            .filter(|v| v["task"] == name)
            .collect();
        anyhow::ensure!(
            values.len() == 1,
            "processor-current-compiler-task-ambiguous"
        );
        Ok(serde_json::from_value(values[0].clone())?)
    };
    let primary = select(&expected_primary.task)?;
    let java = select(&expected_java.task)?;
    same_recipe(&primary, &expected_primary)?;
    same_recipe(&java, &expected_java)?;
    validate_current_inputs(&snapshot, &primary, &expected_primary, &private_tasks)?;
    validate_current_inputs(&snapshot, &java, &expected_java, &private_tasks)?;
    for selected in &original.sources {
        if generated_input(
            &project_path(selected, &plan.root, &snapshot.root)?,
            &private_tasks,
        ) {
            continue;
        }
        let private = project_path(selected, &plan.root, &snapshot.root)?;
        anyhow::ensure!(
            primary.source_files.iter().any(|p| same_path(p, &private)),
            "processor-current-editor-source-not-selected"
        );
    }
    let request = current_request(original, plan, &snapshot.root, primary, java)?;
    let produced = GeneratedSources {
        generation: request.generation,
        selected_inputs: selected_inputs(&request)?,
        sealed: vec![sealed],
        sources: generated,
    };
    produced.check_live(&request)?;
    let (wake, _receiver) = tokio::sync::mpsc::channel(1);
    let private_environment = KotlinDiagnosticEnvironment::prepare(&request, wake)?;
    let mut outcome =
        run_selected_kotlin_editor_compile(&request, &private_environment, Some(&produced));
    produced.check_live(&request)?;
    snapshot.check_sources(original)?;
    environment.check_live()?;
    if let KotlinDiagnosticOutcome::Verified { diagnostics, .. } = &mut outcome {
        for diagnostic in diagnostics {
            let path = Path::new(&diagnostic.path);
            anyhow::ensure!(
                path.starts_with(&snapshot.root),
                "processor-diagnostic-source-outside-current-generation"
            );
            diagnostic.path =
                crate::paths::file_key(&plan.root.join(path.strip_prefix(&snapshot.root)?));
        }
    }
    // Cache only closed owned processor artifacts. The consumer's generated outputs are never
    // copied into a subsequent generation, even when the source text returns to old bytes.
    let mut artifact_roots: Vec<PathBuf> = Vec::new();
    for module in &plan.processor_modules {
        for source in &module.source_sets {
            if source.name != "main" {
                continue;
            }
            for path in &source.provided_artifacts {
                let private = project_path(path, &plan.root, &snapshot.root)?;
                if !artifact_roots.iter().any(|p| same_path(p, &private)) {
                    artifact_roots.push(private);
                }
            }
        }
    }
    let artifacts = SealedOutput::seal(artifact_roots)?;
    let module_prefixes: Vec<_> = plan
        .processor_modules
        .iter()
        .map(|m| format!("{}:", m.module))
        .collect();
    let excluded_tasks: Vec<_> = rows
        .iter()
        .filter(|e| e["event"] == "processor-task-end")
        .filter_map(|e| e["task"].as_str())
        .filter(|task| module_prefixes.iter().any(|p| task.starts_with(p)))
        .map(str::to_owned)
        .collect();
    if !plan.processor_modules.is_empty() && !excluded_tasks.is_empty() {
        // The private environment/tools must be retired before the project owner can become cache
        // storage. Original source/config watches are retained by the original environment.
        drop(private_environment);
        *cache = Some(CachedArtifacts {
            authored_reads: if excluded.is_empty() {
                bootstrap_authored_reads(&snapshot, &rows)?
            } else {
                cache.as_ref().unwrap().authored_reads.clone()
            },
            root: snapshot.root,
            key: snapshot.artifact_key,
            outputs: artifacts,
            excluded_tasks,
            _owner: snapshot.owner,
        });
    }
    original.check_live()?;
    environment.check_live()?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (KotlinEditorOverlay, Plan, KotlinDiagnosticRequest) {
        let parent = std::env::temp_dir().join("ktlsp-current-processor-snapshot-tests");
        let owner = KotlinEditorOverlay::empty_in(
            &kotlin_test_request(&parent.join("unused")),
            &parent.join("runs"),
        )
        .unwrap();
        let root = owner.directory.join("original");
        let app = root.join("app");
        let processor = root.join("processor");
        for (relative, text) in [
            ("app/src/main/kotlin/Source.kt", "class Source { fun current() = 1 }"),
            ("processor/src/main/java/P.java", "class P { int current = 1; }"),
            ("processor/src/main/resources/META-INF/services/javax.annotation.processing.Processor", "P"),
            ("settings.gradle.kts", "include(\":app\", \":processor\")"),
            ("app/build.gradle.kts", "// generation=2"),
            ("app/build/generated/ksp/main/kotlin/Stale.kt", "class Stale"),
            ("processor/build/libs/processor.jar", "saved-artifact-is-not-a-lease"),
        ] {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let mut request = kotlin_test_request(&app);
        request.sources = vec![app.join("src/main/kotlin/Source.kt")];
        request.task.source_files = request.sources.clone();
        request.overlays = vec![KotlinSourceOverlay::new(
            crate::paths::file_key(&request.sources[0]),
            "class Source { fun current() = 2 }".into(),
            2,
        )];
        request.dependency_overlays = vec![KotlinSourceOverlay::new(
            crate::paths::file_key(&processor.join("src/main/java/P.java")),
            "class P { int current = 2; }".into(),
            2,
        )];
        let module = |name: &str, dir: &Path| crate::classpath::ModuleClasspath {
            module: name.into(),
            project_dir: dir.into(),
            build: format!("legacy:{}", crate::paths::file_key(&root)),
            entries: Vec::new(),
            unresolved: Vec::new(),
            source_sets: vec![crate::classpath::SourceSetClasspath {
                name: "main".into(),
                source_roots: vec![dir.join("src/main")],
                ..Default::default()
            }],
        };
        let plan = Plan {
            root: root.clone(),
            owner: module(":app", &app),
            primary: request.task.clone(),
            java: request.task.clone(),
            tasks: Tasks {
                ksp: request.task.clone(),
                stubs: request.task.clone(),
                kapt: request.task.clone(),
            },
            processor_modules: vec![module(":processor", &processor)],
            module_dirs: vec![root, app, processor],
            unselected_source_roots: Vec::new(),
        };
        (owner, plan, request)
    }

    #[test]
    fn processor_private_request_preserves_open_java_after_stock_selection() {
        for first_java in [false, true] {
            let (_owner, mut plan, _) = setup();
            let mut original = super::super::super::tests::request(&plan.owner.project_dir);
            if first_java {
                original
                    .sources
                    .retain(|path| path.extension().and_then(|s| s.to_str()) != Some("java"));
                original.task.source_files = original.sources.clone();
                original
                    .overlays
                    .retain(|source| !source.key.ends_with(".java"));
                original.java_task.as_mut().unwrap().source_files.clear();
            }
            let editor = original.project_dir.join("src/Editor.java");
            let key = crate::paths::file_key(&editor);
            original.sources.push(editor);
            original.java_editor_sources.insert(key.clone());
            original.overlays.push(KotlinSourceOverlay::new(
                key,
                "class Editor { void call(Helper helper) { helper.missing(); } }".into(),
                7,
            ));
            assert!(configured_recipe(&original).is_ok());
            plan.primary = original.task.clone();
            plan.java = original.java_task.clone().unwrap();
            let private = plan.root.parent().unwrap().join("current-private");
            let mut primary: CompilerTask = rebase(&plan.primary, &plan.root, &private).unwrap();
            primary.source_files = original
                .sources
                .iter()
                .map(|path| project_path(path, &plan.root, &private).unwrap())
                .collect();
            let mut java: CompilerTask = rebase(&plan.java, &plan.root, &private).unwrap();
            java.source_files = primary
                .source_files
                .iter()
                .filter(|path| path.extension().and_then(|s| s.to_str()) == Some("java"))
                .cloned()
                .collect();
            let current = current_request(&original, &plan, &private, primary, java).unwrap();
            assert!(
                configured_recipe(&current).is_ok(),
                "selected current Java must reach Javac; first_java={first_java}: {:?}",
                configured_recipe(&current).err(),
            );
            assert!(current.java_editor_sources.is_empty());
            assert_eq!(current.overlays.len(), original.overlays.len());
            for (source, rebased) in original.overlays.iter().zip(&current.overlays) {
                assert_eq!(rebased.text, source.text);
                assert_eq!(rebased.text_sha256, source.text_sha256);
                assert_eq!(
                    rebased.key,
                    crate::paths::file_key(
                        &project_path(Path::new(&source.key), &plan.root, &private).unwrap()
                    ),
                );
                match (&source.identity, &rebased.identity) {
                    (
                        ktcore::jvm::InputIdentity::Source { revision: old, .. },
                        ktcore::jvm::InputIdentity::Source {
                            file,
                            revision: new,
                        },
                    ) => {
                        assert_eq!(old, new);
                        assert_eq!(file, &rebased.key);
                    }
                    _ => panic!("wire overlay must keep its source identity"),
                }
            }
        }
    }

    #[test]
    fn processor_snapshot_rejects_original_config_changes_and_membership() {
        for change in ["content", "membership", "delete", "restored-epoch"] {
            let (owner, plan, request) = setup();
            let config = plan.owner.project_dir.join("config/api.json");
            std::fs::create_dir_all(config.parent().unwrap()).unwrap();
            std::fs::write(&config, br#"{"api":"one"}"#).unwrap();
            let snapshot =
                Snapshot::create_in(&plan, &request, &owner.directory.join("private")).unwrap();
            assert!(snapshot.check_sources(&request).is_ok());
            match change {
                "membership" => {
                    std::fs::write(config.parent().unwrap().join("extra.json"), "{}").unwrap();
                }
                "delete" => {
                    std::fs::remove_file(&config).unwrap();
                }
                _ => {
                    std::fs::write(&config, br#"{"api":"two"}"#).unwrap();
                }
            }
            if change == "restored-epoch" {
                std::fs::write(&config, br#"{"api":"one"}"#).unwrap();
                let until = std::time::Instant::now() + std::time::Duration::from_secs(1);
                while !request.invalid.load(Ordering::Acquire) && std::time::Instant::now() < until
                {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                assert!(
                    request.invalid.load(Ordering::Acquire),
                    "restored bytes cannot restore a retired original source epoch"
                );
            }
            assert!(
                snapshot.check_sources(&request).is_err(),
                "private unchanged bytes must not hide original {change}"
            );
        }
    }

    #[test]
    fn processor_root_project_bootstrap_keeps_original_watch_reads_bounded() {
        let (_owner, mut plan, request) = setup();
        let producer = &mut plan.processor_modules[0];
        producer.module = ":".into();
        producer.project_dir = plan.root.clone();
        producer.source_sets[0].source_roots = vec![plan.root.join("src/main")];
        let inputs = plan.watch_inputs();
        assert!(
            inputs
                .iter()
                .all(|input| !request.project_dir.starts_with(input)),
            "a root processor must not force the existing unbounded-read veto"
        );
        assert!(inputs.contains(&plan.root.join("src/main")));
        assert!(inputs.contains(&plan.root.join("src")));
        assert!(inputs.contains(&plan.root.join("build.gradle.kts")));
    }

    #[test]
    fn processor_snapshot_uses_current_overlays_and_never_saved_generated_outputs() {
        let (owner, plan, request) = setup();
        let snapshot =
            Snapshot::create_in(&plan, &request, &owner.directory.join("private")).unwrap();
        assert!(
            std::fs::read_to_string(snapshot.root.join("app/src/main/kotlin/Source.kt"))
                .unwrap()
                .contains("current() = 2")
        );
        assert!(
            std::fs::read_to_string(snapshot.root.join("processor/src/main/java/P.java"))
                .unwrap()
                .contains("current = 2")
        );
        assert!(!snapshot.root.join("app/build").exists());
        assert!(!snapshot.root.join("processor/build").exists());
        assert!(
            std::fs::read_to_string(plan.root.join("processor/src/main/java/P.java"))
                .unwrap()
                .contains("current = 1")
        );
        snapshot.check_sources(&request).unwrap();
    }

    #[test]
    fn processor_artifact_key_reuses_consumer_body_but_changes_with_processor_resource_options_or_environment(
    ) {
        let (owner, plan, request) = setup();
        let snapshot =
            Snapshot::create_in(&plan, &request, &owner.directory.join("private")).unwrap();
        let key = artifact_key(&plan, &snapshot.root, &snapshot.authored, "environment").unwrap();
        let mut files = snapshot.authored.clone();
        files.insert(
            snapshot.root.join("app/src/main/kotlin/Source.kt"),
            sha(b"another consumer edit"),
        );
        assert_eq!(
            key,
            artifact_key(&plan, &snapshot.root, &files, "environment").unwrap()
        );
        // A processor may legitimately declare consumer files as bootstrap inputs. Such a
        // captured read overrides the broad consumer-body cache exclusion above.
        let consumer = Path::new("app/src/main/kotlin/Source.kt");
        assert_ne!(
            authored_read(&snapshot.root, &snapshot.authored, consumer, false),
            authored_read(&snapshot.root, &files, consumer, false)
        );
        let mut added = snapshot.authored.clone();
        added.insert(
            snapshot.root.join("app/src/main/kotlin/New.kt"),
            sha(b"new declared input"),
        );
        assert_ne!(
            authored_read(
                &snapshot.root,
                &snapshot.authored,
                Path::new("app/src"),
                true
            ),
            authored_read(&snapshot.root, &added, Path::new("app/src"), true)
        );
        for relative in [
            "processor/src/main/java/P.java",
            "processor/src/main/resources/META-INF/services/javax.annotation.processing.Processor",
            "app/build.gradle.kts",
            "settings.gradle.kts",
        ] {
            let mut changed = files.clone();
            changed.insert(
                snapshot.root.join(relative),
                sha(b"valid changed current input"),
            );
            assert_ne!(
                key,
                artifact_key(&plan, &snapshot.root, &changed, "environment").unwrap(),
                "{relative}"
            );
        }
        assert_ne!(
            key,
            artifact_key(&plan, &snapshot.root, &files, "different environment").unwrap()
        );
    }

    #[test]
    fn processor_recipe_preserves_flags_and_ordered_classpath() {
        let (_owner, _plan, request) = setup();
        same_recipe(&request.task, &request.task).unwrap();
        for kind in ["flag", "classpath", "sdk", "output"] {
            let mut changed = request.task.clone();
            match kind {
                "flag" => changed.arguments.push("-Xjsr305=strict".into()),
                "classpath" => changed
                    .classpath
                    .push(request.project_dir.join("other.jar")),
                "sdk" => changed.java_home = Some(request.project_dir.join("different-jdk")),
                "output" => changed
                    .output_roots
                    .push(request.project_dir.join("other-output")),
                _ => unreachable!(),
            }
            assert!(same_recipe(&changed, &request.task).is_err(), "{kind}");
        }
        let mut expected = request.task.clone();
        expected.classpath = vec![
            request.project_dir.join("one.jar"),
            request.project_dir.join("two.jar"),
        ];
        let mut reversed = expected.clone();
        reversed.classpath.reverse();
        assert!(same_recipe(&reversed, &expected).is_err());
    }

    #[test]
    fn processor_current_membership_rejects_dropped_and_unknown_authored_sources() {
        let (owner, plan, request) = setup();
        let snapshot =
            Snapshot::create_in(&plan, &request, &owner.directory.join("private")).unwrap();
        let expected: CompilerTask = rebase(&request.task, &plan.root, &snapshot.root).unwrap();
        let tasks: Tasks = rebase(&plan.tasks, &plan.root, &snapshot.root).unwrap();
        validate_current_inputs(&snapshot, &expected, &expected, &tasks).unwrap();
        let mut dropped = expected.clone();
        dropped.source_files.clear();
        assert!(validate_current_inputs(&snapshot, &dropped, &expected, &tasks).is_err());
        let mut unknown = expected.clone();
        unknown
            .source_files
            .push(snapshot.root.join("app/src/Unmodeled.kt"));
        assert!(validate_current_inputs(&snapshot, &unknown, &expected, &tasks).is_err());
        let mut duplicate = expected.clone();
        duplicate.source_files.extend(expected.source_files.clone());
        assert!(validate_current_inputs(&snapshot, &duplicate, &expected, &tasks).is_err());
    }

    #[test]
    fn processor_generation_capability_rejects_other_current_request() {
        let (_owner, _plan, mut request) = setup();
        let generated = GeneratedSources {
            generation: request.generation,
            selected_inputs: selected_inputs(&request).unwrap(),
            sealed: Vec::new(),
            sources: BTreeMap::new(),
        };
        generated.check_live(&request).unwrap();
        request.overlays[0] = KotlinSourceOverlay::new(
            request.overlays[0].key.clone(),
            "different dirty input".into(),
            3,
        );
        assert!(generated.check_live(&request).is_err());
        request.generation += 1;
        assert!(generated.check_live(&request).is_err());
    }

    #[test]
    fn processor_graph_rejects_external_output_sdk_or_consumer_task() {
        let (_owner, plan, request) = setup();
        let mut tasks = plan.tasks.clone();
        tasks.ksp.task = ":app:kspKotlin".into();
        tasks.stubs.task = ":app:kaptGenerateStubsKotlin".into();
        tasks.kapt.task = ":app:kaptKotlin".into();
        let mut java = request.task.clone();
        java.task = ":app:compileJava".into();
        let names = vec![
            tasks.ksp.task.clone(),
            tasks.stubs.task.clone(),
            tasks.kapt.task.clone(),
        ];
        let contracts: Vec<_> = names
            .iter()
            .map(|name| {
                serde_json::json!({"task":name, "projectDir":plan.owner.project_dir,
            "declaredOutputPaths":[plan.root.join("app/build/generated")]})
            })
            .collect();
        let graph = serde_json::json!({"event":"processor-graph", "root":plan.root, "gradleVersion":"7.4.2",
            "javaHome":request.task.java_home, "requestedTasks":names,"tasks":names,"contracts":contracts,
            "producerCompletion":false,"apiAuthority":false});
        validate_graph(&[graph.clone()], &plan.root, &tasks, &request.task, &java).unwrap();
        for kind in ["output", "sdk", "consumer", "duplicate"] {
            let mut changed = graph.clone();
            match kind {
                "output" => {
                    changed["contracts"][0]["declaredOutputPaths"] =
                        serde_json::json!([plan.root.parent().unwrap().join("user-project/build")])
                }
                "sdk" => changed["javaHome"] = serde_json::json!(plan.root.join("other-jdk")),
                "consumer" => changed["tasks"][0] = serde_json::json!(java.task),
                "duplicate" => changed["tasks"][1] = changed["tasks"][0].clone(),
                _ => unreachable!(),
            }
            assert!(
                validate_graph(&[changed], &plan.root, &tasks, &request.task, &java).is_err(),
                "{kind}"
            );
        }
    }

    #[test]
    fn processor_unmodeled_bootstrap_read_is_unknown_instead_of_stale_authored_disk() {
        let (owner, mut plan, mut request) = setup();
        let support_dir = plan.root.join("support");
        let unrelated_dir = plan.root.join("unrelated");
        plan.module_dirs
            .extend([support_dir.clone(), unrelated_dir.clone()]);
        plan.unselected_source_roots
            .push(unrelated_dir.join("src/main/java"));
        let support_file = support_dir.join("src/main/java/Constants.java");
        assert!(!plan.owns_bootstrap_read(&support_file, false));
        let mut support = plan.processor_modules[0].clone();
        support.module = ":support".into();
        support.project_dir = support_dir.clone();
        support.source_sets[0].source_roots = vec![support_dir.join("src/main/java")];
        plan.processor_modules.push(support);
        assert!(plan.owns_bootstrap_read(&support_file, false));
        assert!(plan.owns_bootstrap_read(&support_dir, true));
        std::fs::create_dir_all(support_file.parent().unwrap()).unwrap();
        std::fs::write(
            &support_file,
            "class Constants { static final int CURRENT = 1; }",
        )
        .unwrap();
        let saved = Snapshot::create_in(&plan, &request, &owner.directory.join("saved")).unwrap();
        request.dependency_overlays.push(KotlinSourceOverlay::new(
            crate::paths::file_key(&support_file),
            "class Constants { static final int CURRENT = 2; }".into(),
            2,
        ));
        let current =
            Snapshot::create_in(&plan, &request, &owner.directory.join("current")).unwrap();
        assert!(
            std::fs::read_to_string(current.root.join("support/src/main/java/Constants.java"))
                .unwrap()
                .contains("CURRENT = 2")
        );
        assert_ne!(
            saved.artifact_key, current.artifact_key,
            "dirty compileOnly support changes the processor artifact reuse key"
        );
        let key = artifact_key(&plan, &current.root, &current.authored, "environment").unwrap();
        let mut unrelated = current.authored.clone();
        unrelated.insert(
            current.root.join("unrelated/src/main/java/Unrelated.java"),
            sha(b"unrelated source edit"),
        );
        assert_eq!(
            key,
            artifact_key(&plan, &current.root, &unrelated, "environment").unwrap()
        );
        assert!(std::fs::read_to_string(&support_file)
            .unwrap()
            .contains("CURRENT = 1"));
        assert!(
            !plan.owns_bootstrap_read(&unrelated_dir.join("src/main/java/Unrelated.java"), false)
        );
        assert!(
            !plan.owns_bootstrap_read(&plan.root, true),
            "a broad read must not silently capture every unrelated module"
        );
    }
}

#[cfg(test)]
mod current_recipe_metadata_tests {
    use super::*;

    #[test]
    fn processor_selected_recipe_keeps_classpath_snapshot_flags_strict() {
        let mut expected: CompilerTask = serde_json::from_value(serde_json::json!({
            "task": ":compileKotlin", "language": "kotlin",
            "task_type": "org.jetbrains.kotlin.gradle.tasks.KotlinCompile",
            "arguments": [], "annotation_processor_path": [], "java_home": null,
            "options": {}, "output_roots": [], "complete": true, "problem": null,
        }))
        .unwrap();
        expected
            .options
            .insert("kotlinClasspathSnapshotInputs".into(), Value::Null);
        let mut observed = expected.clone();
        assert!(same_recipe(&observed, &expected).is_ok());
        observed.options.insert(
            "kotlinClasspathSnapshotInputs".into(),
            serde_json::json!({"adapter": "future", "files": ["different.bin"]}),
        );
        assert!(same_recipe(&observed, &expected).is_err());
        assert!(same_recipe(&expected, &observed).is_err());
    }
}
