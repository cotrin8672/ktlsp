//! Resolve a Gradle project's per-module `compileClasspath` for the Kotlin compile-daemon backend.
//!
//! The compiler sidecar needs the full classpath; Gradle is the only thing that knows it. We run a
//! one-shot init script (embedded below, also committed at `scripts/classpath-dump.init.gradle.kts`)
//! that prints a line protocol, parse it, and cache the result keyed by the build files' mtimes —
//! the classpath is stable across `.kt` edits, so this runs rarely.
//!
//! Pure parsing (`parse_dump`) is unit-tested; `resolve` does the gradle process IO.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, BufWriter, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use walkdir::{DirEntry, WalkDir};

use crate::coords::Coordinate;
use crate::project_model::{FileOrigin, ProjectScope, ScopeVisibility, SourceRepresentation};

/// The init script, single-sourced with the committed copy so a manual run and the embedded run
/// can't drift.
const INIT_SCRIPT: &str = include_str!("../../../scripts/classpath-dump.init.gradle.kts");

/// Gradle classpath discovery uses one process-global temporary init-script path. More
/// importantly, a cache miss is expensive enough that concurrent LSP requests must share it
/// instead of each launching Gradle before the first result reaches the disk cache.
static RESOLUTION_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static SNAPSHOTS: LazyLock<Mutex<BTreeMap<String, Arc<ProjectSnapshot>>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));
const MODEL_VERSION: u32 = 11;

/// Gradle's resolved identity, independent of where an artifact happens to be cached.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ComponentId {
    Module {
        group: String,
        name: String,
        version: String,
    },
    Project {
        build: String,
        project: String,
    },
    File,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedArtifact {
    pub path: PathBuf,
    pub component: ComponentId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompilerLanguage {
    Java,
    Kotlin,
}

/// Gradle's configured Kotlin/JVM launch policy, not proof of an executed compiler process.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum KotlinCompilerExecutionStrategy {
    Daemon,
    InProcess,
    OutOfProcess,
}

/// Selected compiler inputs captured from the task, separate from the target's library classpath.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KotlinCompilerRecipe {
    /// Exact task defaultCompilerClasspath order; append current_jvm_tools_jar once when launching.
    pub default_classpath: Vec<PathBuf>,
    pub current_jvm_tools_jar: Option<PathBuf>,
    /// Read from this compiler runtime's KotlinCompilerVersion, independently of the KGP version.
    pub compiler_version: String,
    /// Actual KGP version, used only to verify the legacy runtime adapter; not compiler identity.
    pub gradle_plugin_version: String,
    /// Gradle's host JVM; this does not certify the JVM of an already-running Kotlin daemon.
    pub gradle_java_home: PathBuf,
    /// Task toolchain before an explicit -jdk-home or -no-jdk changes the effective target SDK.
    pub toolchain_java_home: PathBuf,
    pub run_via_build_tools_api: bool,
    pub execution_strategy: KotlinCompilerExecutionStrategy,
    pub normalized_daemon_jvm_arguments: Vec<String>,
}

/// Configured plain javac API defaults, not an execution receipt or API/producer closure proof.
/// Source files, classpath, processors, target SDK and outputs remain on CompilerTask.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JavaCompilerRecipe {
    pub adapter: String,
    pub compiler_type: String,
    pub java_runtime_version: String,
    pub language_version: String,
    pub gradle_java_version: String,
    /// A property observation; this may differ from the already cached effective_charset.
    pub gradle_file_encoding: String,
    pub effective_charset: String,
    pub locale: String,
    /// Complete JavaCompilerArgumentsBuilder output: retain order, duplicates and empty values.
    pub api_options: Vec<String>,
}

/// Configured compiler inputs, not proof that a build produced current class files.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompilerTask {
    pub task: String,
    pub language: CompilerLanguage,
    pub task_type: String,
    pub arguments: Vec<String>,
    pub annotation_processor_path: Vec<PathBuf>,
    #[serde(default)]
    pub source_files: Vec<PathBuf>,
    #[serde(default)]
    pub classpath: Vec<PathBuf>,
    #[serde(default)]
    pub declared_input_files: Vec<PathBuf>,
    #[serde(default)]
    pub declared_output_roots: Vec<PathBuf>,
    /// Effective target SDK for Kotlin, including -jdk-home and -no-jdk overrides.
    pub java_home: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kotlin_compiler: Option<KotlinCompilerRecipe>,
    pub options: BTreeMap<String, serde_json::Value>,
    pub output_roots: Vec<PathBuf>,
    pub complete: bool,
    pub problem: Option<String>,
}

impl CompilerTask {
    /// A separate optional surface in options avoids duplicating the task's existing input fields.
    /// A worker must still validate argument/input roles and current runtime bytes before launch.
    pub(crate) fn java_compiler_recipe(&self) -> Option<JavaCompilerRecipe> {
        if self.language != CompilerLanguage::Java
            || !self.complete
            || self.problem.is_some()
            || self
                .options
                .get("javaCompilerRecipeProblem")
                .is_some_and(|problem| !problem.is_null())
        {
            return None;
        }
        let recipe: JavaCompilerRecipe =
            serde_json::from_value(self.options.get("javaCompilerRecipe")?.clone()).ok()?;
        (recipe.adapter == "gradle-7.4.2-current-jdk17-javac-api-v1"
            && recipe.compiler_type
                == "org.gradle.jvm.toolchain.internal.DefaultToolchainJavaCompiler"
            && recipe.language_version == "17"
            && !recipe.api_options.is_empty()
            && [
                &recipe.java_runtime_version,
                &recipe.gradle_java_version,
                &recipe.gradle_file_encoding,
                &recipe.effective_charset,
                &recipe.locale,
            ]
            .iter()
            .all(|value| !value.is_empty()))
        .then_some(recipe)
    }

    pub(crate) fn annotation_processing_disabled(&self) -> bool {
        self.arguments.len() == 1 && self.arguments[0] == "-proc:none"
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSetClasspath {
    pub name: String,
    pub source_roots: Vec<PathBuf>,
    pub output_roots: Vec<PathBuf>,
    pub provided_artifacts: Vec<PathBuf>,
    pub entries: Vec<PathBuf>,
    pub artifacts: Vec<ResolvedArtifact>,
    pub unresolved: Vec<String>,
    pub classpath_incomplete: bool,
    #[serde(default)]
    pub compiler_tasks: Vec<CompilerTask>,
    /// Public KotlinCompilation.compileTaskProvider for this exact Java SourceSet.
    /// Auxiliary KAPT/KSP tasks stay in compiler_tasks; absent/ambiguous ownership stays unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kotlin_compile_task: Option<String>,
    #[serde(default)]
    pub compiler_metadata_incomplete: bool,
}

/// One module's resolved compile classpath.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleClasspath {
    /// Gradle project path, e.g. `:Web:api`.
    pub module: String,
    /// Absolute module directory.
    pub project_dir: PathBuf,
    #[serde(default)]
    pub build: String,
    #[serde(default)]
    pub source_sets: Vec<SourceSetClasspath>,
    /// Classpath entries (jars and class dirs), in declared order.
    pub entries: Vec<PathBuf>,
    /// Dependencies the lenient dump could not resolve (`selector: problem`). Empty on a fully
    /// resolved run; reported so silent under-indexing is observable.
    #[serde(default)]
    pub unresolved: Vec<String>,
}

#[derive(Debug, Default)]
struct OutputOwners {
    source_sets: Vec<(usize, usize)>,
    output_root: bool,
    exact_root: bool,
}

/// A shared immutable Gradle model. Ownership queries do no filesystem or Gradle work.
#[derive(Debug)]
pub struct ProjectSnapshot {
    pub modules: Vec<ModuleClasspath>,
    owners: BTreeMap<String, (usize, Option<usize>)>,
    output_owners: BTreeMap<String, OutputOwners>,
    file_origins: BTreeMap<String, String>,
    local_source_keys: std::collections::BTreeSet<String>,
    selected_kotlin_source_keys: std::collections::BTreeSet<String>,
    mtimes: BTreeMap<String, u128>,
    visibilities: Vec<ScopeVisibility>,
}

impl ProjectSnapshot {
    pub fn new(modules: Vec<ModuleClasspath>) -> Self {
        let mut owners = BTreeMap::new();
        let mut output_owners: BTreeMap<String, OutputOwners> = BTreeMap::new();
        let mut selected_kotlin_source_keys = std::collections::BTreeSet::new();
        for (module_idx, module) in modules.iter().enumerate() {
            owners
                .entry(path_key(&module.project_dir))
                .or_insert((module_idx, None));
            for (source_idx, source) in module.source_sets.iter().enumerate() {
                selected_kotlin_source_keys.extend(
                    source
                        .compiler_tasks
                        .iter()
                        .filter(|task| {
                            task.language == CompilerLanguage::Kotlin
                                && source.kotlin_compile_task.as_deref() == Some(task.task.as_str())
                        })
                        .flat_map(|task| &task.source_files)
                        .map(|path| crate::paths::file_key(path)),
                );
                for (path, is_output) in source
                    .output_roots
                    .iter()
                    .map(|path| (path, true))
                    .chain(source.provided_artifacts.iter().map(|path| (path, false)))
                {
                    let raw_key = crate::paths::file_key(path);
                    let output = output_owners
                        .entry(raw_key.trim_end_matches('/').to_string())
                        .or_default();
                    output.source_sets.push((module_idx, source_idx));
                    if is_output {
                        output.output_root = true;
                        // Preserve the current exact-match behavior of trailing-slash roots.
                        output.exact_root |= !raw_key.ends_with('/');
                    }
                }
                for root in &source.source_roots {
                    for key in std::iter::once(path_key(root)).chain(
                        root.canonicalize()
                            .ok()
                            .map(|canonical| path_key(&canonical)),
                    ) {
                        let owner = owners.entry(key).or_insert((module_idx, Some(source_idx)));
                        if owner.1.is_none() {
                            *owner = (module_idx, Some(source_idx));
                        }
                    }
                }
            }
        }
        let mut model = Self {
            modules,
            owners,
            output_owners,
            file_origins: BTreeMap::new(),
            local_source_keys: std::collections::BTreeSet::new(),
            selected_kotlin_source_keys,
            mtimes: BTreeMap::new(),
            visibilities: Vec::new(),
        };
        for entry in model
            .modules
            .iter()
            .flat_map(|m| &m.source_sets)
            .flat_map(|s| &s.entries)
        {
            model
                .file_origins
                .entry(path_key(entry))
                .or_insert_with(|| file_origin_id(entry));
        }
        model.visibilities = model
            .modules
            .iter()
            .flat_map(|module| {
                module.source_sets.iter().map(|source| {
                    let scope = project_scope(module, source);
                    let mut ordered_origins = vec![project_origin_id(&scope)];
                    let mut seen = std::collections::BTreeSet::from([ordered_origins[0].clone()]);
                    let artifacts: BTreeMap<_, _> = source
                        .artifacts
                        .iter()
                        .map(|a| (path_key(&a.path), &a.component))
                        .collect();
                    for entry in &source.entries {
                        for origin in
                            model.origins_for_entry(entry, artifacts.get(&path_key(entry)).copied())
                        {
                            if seen.insert(origin.clone()) {
                                ordered_origins.push(origin);
                            }
                        }
                    }
                    ScopeVisibility {
                        scope,
                        ordered_origins,
                    }
                })
            })
            .collect();
        model.local_source_keys = model
            .local_jars()
            .iter()
            .filter_map(|jar| adjacent_sources_jar(jar))
            .map(|path| crate::paths::file_key(&path))
            .collect();
        model
    }

    pub fn owner_for(
        &self,
        file: &Path,
    ) -> Option<(&ModuleClasspath, Option<&SourceSetClasspath>)> {
        self.owner_for_key(&path_key(file))
    }

    pub(crate) fn owner_for_key(
        &self,
        key: &str,
    ) -> Option<(&ModuleClasspath, Option<&SourceSetClasspath>)> {
        let (module, source) =
            find_ancestor(key.trim_end_matches('/'), |root| self.owners.get(root))?;
        let module = &self.modules[*module];
        Some((module, source.map(|idx| &module.source_sets[idx])))
    }

    /// Configured Java inputs only: this does not prove compiler service absence, binary
    /// freshness, or generated API closure. Kotlin compiler plugins are a separate surface.
    pub(crate) fn plain_java_task(&self, key: &str) -> Option<&CompilerTask> {
        let (_, source) = self.owner_for_key(key)?;
        let source = source?;
        if source.compiler_metadata_incomplete
            || source.classpath_incomplete
            || !source.unresolved.is_empty()
        {
            return None;
        }
        let mut java = source
            .compiler_tasks
            .iter()
            .filter(|task| task.language == CompilerLanguage::Java);
        let task = java.next()?;
        let fork_java_home_matches = match task.options.get("forkJavaHome") {
            Some(serde_json::Value::Null) => true,
            Some(serde_json::Value::String(home)) => task
                .java_home
                .as_ref()
                .is_some_and(|selected| path_key(Path::new(home)) == path_key(selected)),
            _ => false,
        };
        if java.next().is_some()
            || !task.complete
            || task.problem.is_some()
            || !matches!(
                task.task_type.as_str(),
                "org.gradle.api.tasks.compile.JavaCompile"
                    | "org.gradle.api.tasks.compile.JavaCompile_Decorated"
            )
            || task.source_files.is_empty()
            || (!task.arguments.is_empty() && !task.annotation_processing_disabled())
            || !task.annotation_processor_path.is_empty()
            || task.java_home.is_none()
            || task.options.get("fork") != Some(&serde_json::Value::Bool(false))
            || !fork_java_home_matches
            || task.options.get("forkExecutable") != Some(&serde_json::Value::Null)
        {
            return None;
        }
        match task.options.get("forkJvmArgs") {
            Some(serde_json::Value::Null) => Some(task),
            Some(serde_json::Value::Array(args)) if args.is_empty() => Some(task),
            _ => None,
        }
    }

    pub(crate) fn is_output_key(&self, key: &str) -> bool {
        find_ancestor(key, |root| {
            self.output_owners.get(root).filter(|output| {
                output.output_root && (output.exact_root || root.len() < key.len())
            })
        })
        .is_some()
    }

    pub(crate) fn has_dependency_key(&self, key: &str) -> bool {
        self.file_origins.contains_key(key.trim_end_matches('/'))
            || self.local_source_keys.contains(key)
    }

    /// Exact selected compiler inputs also need watch events, without assigning file ownership.
    pub(crate) fn has_selected_kotlin_source_key(&self, key: &str) -> bool {
        self.selected_kotlin_source_keys.contains(key)
    }

    pub fn file_origin(&self, file: &Path) -> Option<FileOrigin> {
        let (module, source) = self.owner_for(file)?;
        let scope = project_scope(module, source?);
        Some(FileOrigin {
            origin_id: project_origin_id(&scope),
            representation: SourceRepresentation::Source,
            project_scope: Some(scope),
        })
    }

    pub fn scope_visibilities(&self, global_origins: &[String]) -> Vec<ScopeVisibility> {
        let mut visibilities = self.visibilities.clone();
        for visibility in &mut visibilities {
            for origin in global_origins {
                if !visibility.ordered_origins.contains(origin) {
                    visibility.ordered_origins.push(origin.clone());
                }
            }
        }
        visibilities
    }

    fn origins_for_entry(&self, entry: &Path, component: Option<&ComponentId>) -> Vec<String> {
        if let Some(ComponentId::Module {
            group,
            name,
            version,
        }) = component
        {
            return vec![format!("maven:{group}:{name}:{version}")];
        }
        let key = path_key(entry);
        let mut origins = Vec::new();
        for &(module_idx, source_idx) in self
            .output_owners
            .get(&key)
            .into_iter()
            .flat_map(|output| &output.source_sets)
        {
            let module = &self.modules[module_idx];
            if let Some(ComponentId::Project { build, project }) = component {
                if module.build != *build || module.module != *project {
                    continue;
                }
            }
            origins.push(project_origin_id(&project_scope(
                module,
                &module.source_sets[source_idx],
            )));
        }
        if origins.is_empty() {
            // An unmapped project output stays unavailable instead of borrowing another module.
            origins.push(self.file_origins[&key].clone());
        }
        origins
    }

    pub fn artifact_origin(&self, path: &Path) -> Option<&str> {
        self.file_origins.get(&path_key(path)).map(String::as_str)
    }

    pub fn coordinate_binaries(&self, coord: &Coordinate) -> Option<Vec<(PathBuf, String)>> {
        self.modules
            .iter()
            .flat_map(|module| &module.source_sets)
            .flat_map(|source| &source.artifacts)
            .filter(|artifact| {
                matches!(&artifact.component, ComponentId::Module { group, name, version }
                    if group == &coord.group && name == &coord.artifact && version == &coord.version)
            })
            .map(|artifact| {
                Some((artifact.path.clone(), self.artifact_origin(&artifact.path)?.to_string()))
            })
            .collect::<Option<BTreeMap<_, _>>>()
            .map(|paths| paths.into_iter().collect())
    }

    pub fn coordinates_for_file(&self, file: &Path) -> Option<Vec<Coordinate>> {
        let (_, source) = self.owner_for(file)?;
        Some(
            source?
                .artifacts
                .iter()
                .filter_map(|artifact| {
                    if let ComponentId::Module {
                        group,
                        name,
                        version,
                    } = &artifact.component
                    {
                        Coordinate::parse(&format!("{group}:{name}:{version}"))
                    } else {
                        None
                    }
                })
                .collect(),
        )
    }

    pub fn local_jars_for_file(&self, file: &Path) -> Option<Vec<PathBuf>> {
        let (_, source) = self.owner_for(file)?;
        let source = source?;
        let known: std::collections::BTreeSet<_> = source
            .artifacts
            .iter()
            .filter(|a| !matches!(a.component, ComponentId::File))
            .map(|a| path_key(&a.path))
            .collect();
        Some(
            source
                .entries
                .iter()
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jar"))
                .filter(|p| !known.contains(&path_key(p)))
                .cloned()
                .collect(),
        )
    }

    pub fn coordinates(&self) -> Vec<Coordinate> {
        self.modules
            .iter()
            .filter(|module| !module.source_sets.is_empty())
            .flat_map(module_coordinates)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn local_jars(&self) -> Vec<PathBuf> {
        self.modules
            .iter()
            .filter(|module| !module.source_sets.is_empty())
            .flat_map(module_local_jars)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn live_origins(&self) -> std::collections::BTreeSet<String> {
        self.visibilities
            .iter()
            .flat_map(|visibility| visibility.ordered_origins.iter().cloned())
            .collect()
    }

    pub fn artifact_revisions(&self) -> BTreeMap<String, std::collections::BTreeSet<String>> {
        let mut revisions: BTreeMap<String, std::collections::BTreeSet<String>> = BTreeMap::new();
        for source in self.modules.iter().flat_map(|module| &module.source_sets) {
            for artifact in &source.artifacts {
                if !self.file_origins.contains_key(&path_key(&artifact.path)) {
                    continue;
                }
                for origin in self.origins_for_entry(&artifact.path, Some(&artifact.component)) {
                    if let Some(revision) = self.file_origins.get(&path_key(&artifact.path)) {
                        revisions
                            .entry(origin)
                            .or_default()
                            .insert(revision.clone());
                    }
                }
            }
        }
        revisions
    }
}

fn project_scope(module: &ModuleClasspath, source: &SourceSetClasspath) -> ProjectScope {
    ProjectScope {
        module: format!("gradle:{}|{}", module.build, module.module),
        source_set: source.name.clone(),
    }
}

fn project_origin_id(scope: &ProjectScope) -> String {
    format!("project:{}#{}", scope.module, scope.source_set)
}

pub fn maven_origin_id(coordinate: &Coordinate) -> String {
    format!("maven:{}", coordinate.label())
}

pub fn file_origin_id(path: &Path) -> String {
    let mut origin = format!("file:{}", file_revision(path));
    if let Some(sources) = adjacent_sources_jar(path).filter(|sources| sources.is_file()) {
        origin.push_str("|source:");
        origin.push_str(&file_revision(&sources));
    }
    origin
}

/// The exact sibling is watched even before a build creates it.
pub fn adjacent_sources_jar(path: &Path) -> Option<PathBuf> {
    if path.extension()?.to_str()? != "jar" {
        return None;
    }
    let mut name = path.file_stem()?.to_os_string();
    name.push("-sources.jar");
    Some(path.with_file_name(name))
}

fn file_revision(path: &Path) -> String {
    let metadata = std::fs::metadata(path).ok();
    let size = metadata.as_ref().map_or(0, |m| m.len());
    let mtime = metadata
        .and_then(|m| m.modified().ok())
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    format!("{}#{mtime}:{size}", path_key(path))
}

pub fn jdk_origin_id(path: &Path) -> String {
    format!("jdk:{}", path_key(path))
}

fn find_ancestor<T>(mut key: &str, mut lookup: impl FnMut(&str) -> Option<T>) -> Option<T> {
    loop {
        if let Some(value) = lookup(key) {
            return Some(value);
        }
        key = &key[..key.rfind('/')?];
    }
}

fn path_key(path: &Path) -> String {
    crate::paths::file_key(path)
        .trim_end_matches('/')
        .to_string()
}

/// Only returns an already-published model; safe on an interactive request path.
pub fn snapshot(root: &Path) -> Option<Arc<ProjectSnapshot>> {
    SNAPSHOTS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&path_key(root))
        .cloned()
}

fn publish(
    root: &Path,
    modules: Vec<ModuleClasspath>,
    mtimes: BTreeMap<String, u128>,
) -> Arc<ProjectSnapshot> {
    let mut model = ProjectSnapshot::new(modules);
    model.mtimes = mtimes;
    let model = Arc::new(model);
    publish_snapshot(root, model.clone());
    model
}

/// Called with the workspace generation accepted; resolving alone never publishes a model.
pub fn publish_snapshot(root: &Path, model: Arc<ProjectSnapshot>) {
    let mut snapshots = SNAPSHOTS.lock().unwrap_or_else(|p| p.into_inner());
    snapshots.insert(path_key(root), model.clone());
    if let Ok(canonical) = root.canonicalize() {
        snapshots.insert(path_key(&canonical), model.clone());
    }
}

/// Parse the init script's line protocol:
///   PROJECT	<gradle path>	<absolute projectDir>
///   CP	<absolute entry>            (repeated)
///   UNRESOLVED	<selector>	<problem> (lenient-resolution drops, repeated)
///   END	<gradle path>
/// This best-effort helper retains unfinished blocks for callers inspecting partial text. Native
/// discovery uses the strict streaming parser and never accepts an unfinished or mismatched block.
///
/// `UNRESOLVED` lines are produced from `ResolutionResult` (a CC-compatible `Provider`) by
/// iterating dependency edges for `UnresolvedDependencyResult` entries.
pub fn parse_dump(output: &str) -> Vec<ModuleClasspath> {
    parse_dump_reader(output.as_bytes(), false).unwrap_or_default()
}

fn parse_dump_reader(
    mut reader: impl BufRead,
    require_complete: bool,
) -> anyhow::Result<Vec<ModuleClasspath>> {
    let mut out = Vec::new();
    let mut current: Option<ModuleClasspath> = None;
    let mut kotlin_compile_task_sources = std::collections::BTreeSet::new();
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let mut parts = line.trim_end_matches(['\r', '\n']).split('\t');
        let record = parts.next();
        anyhow::ensure!(
            !require_complete
                || current.is_some()
                || !matches!(
                    record,
                    Some(
                        "BUILD"
                            | "SOURCESET"
                            | "SOURCE"
                            | "OUTPUT"
                            | "PROVIDES"
                            | "SCP"
                            | "ARTIFACT"
                            | "SUNRESOLVED"
                            | "INCOMPLETE"
                            | "COMPILER"
                            | "KOTLIN_COMPILE_TASK"
                            | "CP"
                            | "UNRESOLVED"
                    )
                ),
            "record outside PROJECT in classpath dump"
        );
        match record {
            Some("PROJECT") => {
                kotlin_compile_task_sources.clear();
                anyhow::ensure!(
                    !require_complete || current.is_none(),
                    "nested PROJECT before matching END in classpath dump"
                );
                if let Some(done) = current.take() {
                    out.push(done);
                }
                let module = parts.next().unwrap_or("").to_string();
                let dir = parts.next().unwrap_or("");
                anyhow::ensure!(
                    !require_complete
                        || (!module.is_empty() && !dir.is_empty() && parts.next().is_none()),
                    "invalid PROJECT record in classpath dump"
                );
                if !module.is_empty() {
                    current = Some(ModuleClasspath {
                        module,
                        project_dir: PathBuf::from(dir),
                        build: ":".to_string(),
                        source_sets: Vec::new(),
                        entries: Vec::new(),
                        unresolved: Vec::new(),
                    });
                }
            }
            Some("BUILD") => {
                if let (Some(cur), Some(build)) = (current.as_mut(), parts.next()) {
                    cur.build = build.to_string();
                }
            }
            Some(
                record @ ("SOURCESET"
                | "SOURCE"
                | "OUTPUT"
                | "PROVIDES"
                | "SCP"
                | "ARTIFACT"
                | "SUNRESOLVED"
                | "INCOMPLETE"
                | "COMPILER"
                | "KOTLIN_COMPILE_TASK"),
            ) => {
                if let (Some(cur), Some(name)) = (current.as_mut(), parts.next()) {
                    let idx = cur
                        .source_sets
                        .iter()
                        .position(|s| s.name == name)
                        .unwrap_or_else(|| {
                            cur.source_sets.push(SourceSetClasspath {
                                name: name.to_string(),
                                ..Default::default()
                            });
                            cur.source_sets.len() - 1
                        });
                    let source = &mut cur.source_sets[idx];
                    if record == "KOTLIN_COMPILE_TASK" {
                        let selection = parts
                            .next()
                            .and_then(|json| {
                                serde_json::from_str::<Option<String>>(json).ok().flatten()
                            })
                            .filter(|task| !task.is_empty() && !task.chars().any(char::is_control));
                        source.kotlin_compile_task = if kotlin_compile_task_sources
                            .insert(name.to_owned())
                            && parts.next().is_none()
                        {
                            selection
                        } else {
                            None
                        };
                        continue;
                    }
                    if record == "COMPILER" {
                        let compiler = parts
                            .next()
                            .and_then(|json| serde_json::from_str::<CompilerTask>(json).ok());
                        match compiler {
                            Some(compiler)
                                if !compiler.task.is_empty() && !compiler.task_type.is_empty() =>
                            {
                                source.compiler_metadata_incomplete |= !compiler.complete;
                                source.compiler_tasks.push(compiler);
                            }
                            _ => {
                                source.compiler_metadata_incomplete = true;
                                tracing::warn!(
                                    "invalid compiler configuration for {}:{}",
                                    cur.module,
                                    name
                                );
                            }
                        }
                        continue;
                    }
                    if record == "INCOMPLETE" {
                        source.classpath_incomplete = true;
                    }
                    if let Some(path) = parts.next().filter(|p| !p.is_empty()) {
                        if record == "SUNRESOLVED" {
                            source
                                .unresolved
                                .push(format!("{path}: {}", parts.next().unwrap_or("")));
                            continue;
                        }
                        let path = PathBuf::from(path);
                        match record {
                            "SOURCE" => source.source_roots.push(path),
                            "OUTPUT" => source.output_roots.push(path),
                            "PROVIDES" => source.provided_artifacts.push(path),
                            "SCP" => source.entries.push(path),
                            "ARTIFACT" => {
                                let component = match parts.next() {
                                    Some("MODULE") => {
                                        match (parts.next(), parts.next(), parts.next()) {
                                            (Some(group), Some(name), Some(version)) => {
                                                Some(ComponentId::Module {
                                                    group: group.to_string(),
                                                    name: name.to_string(),
                                                    version: version.to_string(),
                                                })
                                            }
                                            _ => None,
                                        }
                                    }
                                    Some("PROJECT") => match (parts.next(), parts.next()) {
                                        (Some(build), Some(project)) => {
                                            Some(ComponentId::Project {
                                                build: build.to_string(),
                                                project: project.to_string(),
                                            })
                                        }
                                        _ => None,
                                    },
                                    Some("FILE") => Some(ComponentId::File),
                                    _ => None,
                                };
                                if let Some(component) = component {
                                    source.artifacts.push(ResolvedArtifact { path, component });
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            Some("CP") => {
                if let (Some(cur), Some(path)) = (current.as_mut(), parts.next()) {
                    if !path.is_empty() {
                        cur.entries.push(PathBuf::from(path));
                    }
                }
            }
            Some("UNRESOLVED") => {
                if let (Some(cur), Some(selector)) = (current.as_mut(), parts.next()) {
                    let problem = parts.next().unwrap_or("");
                    cur.unresolved.push(format!("{selector}: {problem}"));
                }
            }
            Some("END") => {
                if require_complete {
                    anyhow::ensure!(
                        current.as_ref().is_some_and(|module| {
                            parts.next() == Some(module.module.as_str()) && parts.next().is_none()
                        }),
                        "END does not match PROJECT in classpath dump"
                    );
                    anyhow::ensure!(line.ends_with('\n'), "unterminated END in classpath dump");
                }
                if let Some(done) = current.take() {
                    out.push(done);
                }
            }
            _ => {}
        }
    }
    if let Some(done) = current.take() {
        anyhow::ensure!(
            !require_complete,
            "unfinished PROJECT {} in classpath dump",
            done.module
        );
        out.push(done);
    }
    // shortcut: omitted whole blocks are undetectable, add a dump marker when subset discovery must be ruled out.
    Ok(out)
}

/// Build files whose mtimes invalidate the cache.
fn build_file_mtimes(root: &Path, modules: &[ModuleClasspath]) -> BTreeMap<String, u128> {
    const NAMES: &[&str] = &[
        "settings.gradle.kts",
        "settings.gradle",
        "build.gradle.kts",
        "build.gradle",
        "gradle/libs.versions.toml",
        "gradle.properties",
    ];
    let gradle = crate::deps::gradle_root(root).unwrap_or_else(|| root.to_path_buf());
    let mut map = BTreeMap::new();
    for dir in
        std::iter::once(gradle.as_path()).chain(modules.iter().map(|m| m.project_dir.as_path()))
    {
        for name in NAMES {
            let p = dir.join(name);
            let mtime = std::fs::metadata(&p)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            map.insert(path_key(&p), mtime);
        }
    }
    map
}

#[derive(Serialize, Deserialize)]
struct CacheEntry {
    #[serde(default)]
    model_version: u32,
    mtimes: BTreeMap<String, u128>,
    modules: Vec<ModuleClasspath>,
}

#[derive(Serialize)]
struct CacheEntryRef<'a> {
    model_version: u32,
    mtimes: &'a BTreeMap<String, u128>,
    modules: &'a [ModuleClasspath],
}

static CACHE_WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn read_cache_entry(path: &Path) -> Option<CacheEntry> {
    let file = std::fs::File::open(path).ok()?;
    serde_json::from_reader(BufReader::new(file)).ok()
}

fn cache_path(root: &Path) -> PathBuf {
    // One cache file per root, named by a hash of the canonical root path.
    let key = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut hash: u64 = 1469598103934665603; // FNV-1a
    for b in key.to_string_lossy().bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    crate::deps::cache_home()
        .join("classpath")
        .join(format!("{hash:016x}.json"))
}

/// Resolve every module's compile classpath, using the cache when build files are unchanged.
/// Note: on a build with configuration-on-demand this may only return the configured subset; prefer
/// `resolve_module` when the target module is known.
pub fn resolve(root: &Path) -> anyhow::Result<Vec<ModuleClasspath>> {
    Ok(resolve_snapshot(root)?.modules.clone())
}

pub fn resolve_snapshot(root: &Path) -> anyhow::Result<Arc<ProjectSnapshot>> {
    let previous = snapshot(root);
    let model = load_snapshot(root, false)?;
    if previous
        .as_ref()
        .is_none_or(|previous| !Arc::ptr_eq(previous, &model))
    {
        publish_snapshot(root, model.clone());
        store_snapshot(root, &model);
    }
    Ok(model)
}

/// Background-only preparation. Build/output events force Gradle even when metadata is unchanged.
pub fn load_snapshot(root: &Path, force: bool) -> anyhow::Result<Arc<ProjectSnapshot>> {
    load_snapshot_with(root, force, || {
        dump_with_fallback(root, "ktlspDumpClasspath")
    })
}

fn load_snapshot_with(
    root: &Path,
    force: bool,
    dump: impl FnOnce() -> anyhow::Result<Vec<ModuleClasspath>>,
) -> anyhow::Result<Arc<ProjectSnapshot>> {
    let lock_start = std::time::Instant::now();
    let _guard = RESOLUTION_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    crate::trace::span(
        "classpath.model_lock",
        "classpath",
        lock_start,
        serde_json::json!({"root": root.to_string_lossy(), "forcedModel": force}),
    );
    let cache_start = std::time::Instant::now();
    let trace_cache = |source: &str| {
        crate::trace::span(
            "classpath.model_cache",
            "classpath",
            cache_start,
            serde_json::json!({"root": root.to_string_lossy(), "forcedModel": force, "source": source}),
        )
    };
    if let Some(model) = snapshot(root).filter(|_| !force) {
        if model.mtimes == build_file_mtimes(root, &model.modules) {
            trace_cache("memory");
            return Ok(model);
        }
    }
    let cache = cache_path(root);
    if !force {
        if let Some(entry) = read_cache_entry(&cache) {
            if entry.model_version == MODEL_VERSION
                && entry.mtimes == build_file_mtimes(root, &entry.modules)
            {
                let mut model = ProjectSnapshot::new(entry.modules);
                model.mtimes = entry.mtimes;
                trace_cache("disk");
                return Ok(Arc::new(model));
            }
        }
    }
    trace_cache("miss");
    let modules = dump()?;
    let mtimes = build_file_mtimes(root, &modules);
    log_unresolved(&modules);
    let mut model = ProjectSnapshot::new(modules);
    model.mtimes = mtimes;
    Ok(Arc::new(model))
}

/// Persist an accepted snapshot off the workspace lock.
pub fn store_snapshot(root: &Path, model: &ProjectSnapshot) {
    write_cache_entry(
        &cache_path(root),
        &CacheEntryRef {
            model_version: MODEL_VERSION,
            mtimes: &model.mtimes,
            modules: &model.modules,
        },
    );
}

/// Resolve one module's classpath by gradle path (e.g. `:Web:api`), running only that module's dump
/// task. This configures just the module + its dependencies — essential on large repos with
/// configuration-on-demand, where an unqualified task wouldn't reach the module. Cached per module.
pub fn resolve_module(root: &Path, module: &str) -> anyhow::Result<ModuleClasspath> {
    let _guard = RESOLUTION_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(model) = snapshot(root) {
        if model.mtimes == build_file_mtimes(root, &model.modules) {
            if let Some(module) = model.modules.iter().find(|m| m.module == module) {
                return Ok(module.clone());
            }
        }
    }
    let cache = module_cache_path(root, module);
    if let Some(entry) = read_cache_entry(&cache) {
        if entry.model_version == MODEL_VERSION
            && entry.mtimes == build_file_mtimes(root, &entry.modules)
        {
            if let Some(found) = entry.modules.iter().find(|m| m.module == module).cloned() {
                publish_modules(root, entry.modules);
                return Ok(found);
            }
        }
    }
    let task = format!("{}:ktlspDumpClasspath", module.trim_end_matches(':'));
    let modules = dump_with_fallback(root, &task)?;
    log_unresolved(&modules);
    let found = modules
        .iter()
        .find(|m| m.module == module)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("module {module} not found in dump"))?;
    let mtimes = build_file_mtimes(root, &modules);
    write_cache_entry(
        &cache,
        &CacheEntryRef {
            model_version: MODEL_VERSION,
            mtimes: &mtimes,
            modules: &modules,
        },
    );
    publish_modules(root, modules);
    Ok(found)
}

fn publish_modules(root: &Path, modules: Vec<ModuleClasspath>) {
    let mut all = snapshot(root)
        .map(|m| m.modules.clone())
        .unwrap_or_default();
    for module in modules {
        all.retain(|m| m.module != module.module || m.build != module.build);
        all.push(module);
    }
    let mtimes = build_file_mtimes(root, &all);
    publish(root, all, mtimes);
}

fn write_cache_entry(path: &Path, entry: &impl Serialize) {
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let sequence = CACHE_WRITE_SEQUENCE.fetch_add(1, AtomicOrdering::Relaxed);
    let tmp = path.with_extension(format!("json.{}.{sequence}.tmp", std::process::id()));
    let Ok(file) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
    else {
        return;
    };
    let _cleanup = TempFile(&tmp);
    let mut writer = BufWriter::new(file);
    if serde_json::to_writer(&mut writer, entry).is_err() || writer.flush().is_err() {
        return;
    }
    drop(writer);
    let _ = std::fs::rename(&tmp, path);
}

fn module_cache_path(root: &Path, module: &str) -> PathBuf {
    let key = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut hash: u64 = 1469598103934665603;
    for b in key.to_string_lossy().bytes().chain(module.bytes()) {
        hash ^= b as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    crate::deps::cache_home()
        .join("classpath")
        .join(format!("mod-{hash:016x}.json"))
}

/// Offline-first dump: with a populated Gradle cache the result is a pure function of local
/// state, immune to repository flakiness. An empty or failed offline dump means the cache isn't
/// populated (fresh machine) — fall back to the online run for completeness.
fn dump_with_fallback(root: &Path, task: &str) -> anyhow::Result<Vec<ModuleClasspath>> {
    let run = |offline| {
        let start = std::time::Instant::now();
        let result = run_dump(root, task, offline);
        crate::trace::span(
            "classpath.gradle_dump",
            "classpath",
            start,
            serde_json::json!({"root": root.to_string_lossy(), "task": task, "offline": offline,
                "ok": result.is_ok(), "modules": result.as_ref().ok().map(Vec::len)}),
        );
        result
    };
    match run(true) {
        Ok(modules) if modules.iter().any(|m| !m.entries.is_empty()) => Ok(modules),
        Ok(_) => {
            tracing::info!("offline classpath dump resolved no entries; retrying online");
            run(false)
        }
        Err(offline_err) => {
            tracing::info!("offline classpath dump failed ({offline_err:#}); retrying online");
            run(false)
        }
    }
}

fn log_unresolved(modules: &[ModuleClasspath]) {
    let total: usize = modules.iter().map(|m| m.unresolved.len()).sum();
    if total == 0 {
        return;
    }
    let sample: Vec<String> = modules
        .iter()
        .flat_map(|m| {
            m.unresolved
                .iter()
                .take(2)
                .map(move |u| format!("{}: {u}", m.module))
        })
        .take(6)
        .collect();
    tracing::warn!(
        "classpath dump left {total} dependencies unresolved; their symbols stay missing until a Gradle build populates the cache: {}",
        sample.join("; ")
    );
}

/// Run the init script via the project's gradle wrapper and parse the dump. Inherits the parent's
/// environment (so repo-specific flags like SKIP_* are honored when ktlsp is launched with them).
/// `offline` passes `--offline`: resolution then depends only on the local Gradle cache, which is
/// stable across runs — online runs drop entries nondeterministically under repository flakiness.
fn run_dump(root: &Path, task: &str, offline: bool) -> anyhow::Result<Vec<ModuleClasspath>> {
    let gradle = crate::deps::gradle_root(root)
        .ok_or_else(|| anyhow::anyhow!("{} is not a Gradle project", root.display()))?;
    // Absolute wrapper path: with current_dir(root) set, a relative program path resolves against
    // the child's cwd and fails to spawn.
    let gradlew = std::fs::canonicalize(gradle.join(if cfg!(windows) {
        "gradlew.bat"
    } else {
        "gradlew"
    }))
    .map_err(|_| anyhow::anyhow!("no gradle wrapper under {}", gradle.display()))?;
    // Write the embedded init script to a temp file (avoids depending on ktlsp's install layout).
    let script = std::env::temp_dir().join(format!(
        "ktlsp-classpath-{}.init.gradle.kts",
        std::process::id()
    ));
    std::fs::write(&script, INIT_SCRIPT)?;
    let _guard = TempFile(&script);

    let mut command = Command::new(&gradlew);
    command
        .current_dir(&gradle)
        .arg("-I")
        .arg(&script)
        .arg(task)
        .arg("-q")
        // Cached provider values must not replay stale compiler recipes or source membership.
        .arg("--no-configuration-cache")
        .arg("-Dscan.capture-resource-usage=false")
        .arg("--console=plain");
    if offline {
        command.arg("--offline");
    }
    #[cfg(windows)]
    command.arg("--no-daemon");
    let sequence = CACHE_WRITE_SEQUENCE.fetch_add(1, AtomicOrdering::Relaxed);
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let stdout_path = std::env::temp_dir().join(format!(
        "ktlsp-classpath-{}-{nonce}-{sequence}.stdout",
        std::process::id()
    ));
    let mut stdout = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&stdout_path)?;
    let _stdout_guard = TempFile(&stdout_path);
    let (status, interrupted, stdout_failure, stdout_complete, stderr, stderr_complete) =
        crate::compile::run_with_stdout_file(&mut command, stdout.try_clone()?)?;
    if interrupted {
        anyhow::bail!(
            "gradle classpath dump interrupted by deadline or owned-tree cleanup:\n{stderr}"
        );
    }
    if !stdout_complete {
        let stdout_failure = if stdout_failure.is_empty() {
            "stdout reader did not complete"
        } else {
            &stdout_failure
        };
        anyhow::bail!("gradle classpath stdout capture failed: {stdout_failure}");
    }
    if !stderr_complete {
        anyhow::bail!("gradle classpath stderr capture failed or exceeded diagnostic limit");
    }
    if !status.success() {
        anyhow::bail!("gradle classpath dump failed ({}):\n{}", status, stderr);
    }
    stdout.rewind()?;
    let modules = parse_dump_reader(BufReader::new(stdout), true)
        .map_err(|error| anyhow::anyhow!("invalid gradle classpath dump: {error}"))?;
    if modules.is_empty() {
        anyhow::bail!(
            "classpath dump produced no modules (is this a gradle project with compileClasspath?)"
        );
    }
    Ok(modules)
}

struct TempFile<'a>(&'a Path);
impl Drop for TempFile<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0);
    }
}

/// Best-effort Maven coordinates derived from the resolved Gradle `compileClasspath`.
/// This is the fallback dependency-source index for Gradle projects without a version catalog:
/// each jar's path under the Gradle cache (`modules-2/files-2.1/{group}/{artifact}/{version}/...`)
/// reveals its coordinate, and ktlsp can then locate/download the matching `-sources.jar`.
pub fn coordinates_from_classpath(root: &Path) -> Vec<Coordinate> {
    let mut out = std::collections::BTreeSet::new();
    if let Ok(model) = resolve_snapshot(root) {
        for module in &model.modules {
            out.extend(module_coordinates(module));
        }
    }
    out.into_iter().collect()
}

pub fn coordinates_from_module_classpath(root: &Path, module: &str) -> Vec<Coordinate> {
    resolve_module(root, module)
        .map(|classpath| module_coordinates(&classpath).into_iter().collect())
        .unwrap_or_default()
}

/// Local binary jars from the resolved compile classpath that are not identifiable Maven cache
/// artifacts. These usually come from `files("libs/foo.jar")`; they have no coordinate/source jar,
/// so dependency navigation falls back to class stubs.
pub fn local_jars_from_classpath(root: &Path) -> Vec<PathBuf> {
    let mut out = std::collections::BTreeSet::new();
    let mut authoritative = false;
    if let Ok(model) = resolve_snapshot(root) {
        authoritative = model.modules.iter().any(|m| !m.source_sets.is_empty());
        for module in &model.modules {
            out.extend(module_local_jars(module));
        }
    }
    if !authoritative {
        out.extend(local_jars_from_build_files(root));
    }
    out.into_iter().collect()
}

pub fn local_jars_from_module_classpath(root: &Path, module: &str) -> Vec<PathBuf> {
    let mut out = std::collections::BTreeSet::new();
    match resolve_module(root, module) {
        Ok(classpath) => {
            out.extend(module_local_jars(&classpath));
            if classpath.source_sets.is_empty() {
                out.extend(local_jars_from_build_files(&classpath.project_dir));
            }
        }
        Err(_) => {
            if let Some(module_dir) = module_dir_for_default_layout(root, module) {
                out.extend(local_jars_from_build_files(&module_dir));
            }
        }
    }
    out.into_iter().collect()
}

/// Fallback for local file dependencies such as `api(files("libs/vendor.jar"))`.
///
/// This intentionally avoids Gradle execution, so it still works when a repo's Java toolchain is
/// unavailable during editor startup.
pub fn local_jars_from_build_files(root: &Path) -> Vec<PathBuf> {
    let mut out = std::collections::BTreeSet::new();
    let build_files = git_gradle_build_files(root).unwrap_or_else(|| {
        WalkDir::new(root)
            .into_iter()
            .filter_entry(|entry| is_gradle_scan_entry(entry))
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| entry.into_path())
            .filter(|path| {
                matches!(
                    path.file_name().and_then(|name| name.to_str()),
                    Some("build.gradle" | "build.gradle.kts")
                )
            })
            .collect()
    });
    for path in build_files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let base = path.parent().unwrap_or(root);
        for rel in quoted_files_jar_paths(&text) {
            let jar = base.join(rel);
            if jar.is_file() {
                out.insert(jar);
            }
        }
    }
    out.into_iter().collect()
}

/// Ask Git for build files when possible so ignored generated trees do not turn this small lookup
/// into a full checkout walk. Include non-ignored untracked files to preserve editor behavior for
/// newly created modules; non-Git projects retain the filesystem fallback above.
fn git_gradle_build_files(root: &Path) -> Option<Vec<PathBuf>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            ":(glob)**/build.gradle",
            ":(glob)**/build.gradle.kts",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut paths = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|rel| !rel.is_empty())
        .map(|rel| root.join(String::from_utf8_lossy(rel).as_ref()))
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    Some(paths)
}

fn coordinates_from_entries(entries: Vec<PathBuf>) -> std::collections::BTreeSet<Coordinate> {
    entries
        .into_iter()
        .filter_map(|entry| coordinate_from_gradle_cache_path(&entry))
        .collect()
}

fn module_coordinates(module: &ModuleClasspath) -> std::collections::BTreeSet<Coordinate> {
    if module.source_sets.is_empty() {
        return coordinates_from_entries(module.entries.clone());
    }
    module
        .source_sets
        .iter()
        .flat_map(|source| &source.artifacts)
        .filter_map(|artifact| {
            if let ComponentId::Module {
                group,
                name,
                version,
            } = &artifact.component
            {
                Coordinate::parse(&format!("{group}:{name}:{version}"))
            } else {
                None
            }
        })
        .collect()
}

fn module_local_jars(module: &ModuleClasspath) -> std::collections::BTreeSet<PathBuf> {
    if module.source_sets.is_empty() {
        return local_jars_from_entries(module.entries.clone());
    }
    let known: std::collections::BTreeSet<_> = module
        .source_sets
        .iter()
        .flat_map(|source| &source.artifacts)
        .filter(|artifact| !matches!(artifact.component, ComponentId::File))
        .map(|artifact| path_key(&artifact.path))
        .collect();
    module
        .source_sets
        .iter()
        .flat_map(|source| &source.entries)
        .filter(|entry| entry.extension().and_then(|ext| ext.to_str()) == Some("jar"))
        .filter(|entry| !known.contains(&path_key(entry)))
        .cloned()
        .collect()
}

fn local_jars_from_entries(entries: Vec<PathBuf>) -> std::collections::BTreeSet<PathBuf> {
    entries
        .into_iter()
        .filter(|entry| entry.extension().and_then(|ext| ext.to_str()) == Some("jar"))
        .filter(|entry| coordinate_from_gradle_cache_path(entry).is_none())
        .collect()
}

fn module_dir_for_default_layout(root: &Path, module: &str) -> Option<PathBuf> {
    let trimmed = module.trim_matches(':');
    if trimmed.is_empty() {
        return Some(root.to_path_buf());
    }
    Some(root.join(trimmed.replace(':', "/")))
}

fn quoted_files_jar_paths(text: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(idx) = rest.find("files(") {
        rest = &rest[idx + "files(".len()..];
        let end = rest.find(')').unwrap_or(rest.len());
        let args = &rest[..end];
        let mut chars = args.char_indices();
        while let Some((start, ch)) = chars.next() {
            if ch != '"' && ch != '\'' {
                continue;
            }
            let quote = ch;
            let value_start = start + ch.len_utf8();
            let mut value_end = None;
            for (idx, c) in chars.by_ref() {
                if c == quote {
                    value_end = Some(idx);
                    break;
                }
            }
            let Some(value_end) = value_end else {
                break;
            };
            let value = &args[value_start..value_end];
            if value.ends_with(".jar") && !value.contains('$') {
                out.push(PathBuf::from(value));
            }
        }
        rest = &rest[end.min(rest.len())..];
    }
    out
}

fn is_gradle_scan_entry(entry: &DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    if !entry.file_type().is_dir() {
        return true;
    }
    !matches!(
        name.as_ref(),
        ".git" | ".gradle" | "build" | "out" | "target" | "node_modules"
    )
}

/// Parse a single Gradle cache jar path back into a `group:artifact:version` coordinate.
/// Returns `None` for non-cache paths (e.g. project `build/classes/java/main`) or jars whose
/// file name does not match the expected `{artifact}-{version}` prefix.
fn coordinate_from_gradle_cache_path(path: &Path) -> Option<Coordinate> {
    let components: Vec<_> = path.components().collect();
    let idx = components
        .iter()
        .position(|c| c.as_os_str() == "files-2.1")?;
    if components.len() < idx + 5 {
        return None;
    }
    let group = components[idx + 1].as_os_str().to_str()?;
    let artifact = components[idx + 2].as_os_str().to_str()?;
    let version = components[idx + 3].as_os_str().to_str()?;
    let stem = path.file_stem()?.to_str()?;
    let expected_prefix = format!("{artifact}-{version}");
    if !stem.starts_with(&expected_prefix) {
        return None;
    }
    Coordinate::parse(&format!("{group}:{artifact}:{version}"))
}

#[cfg(test)]
mod tests {
    fn selected_kotlin_recipe() -> serde_json::Value {
        serde_json::json!({
            "default_classpath": ["/compiler/z-runtime.jar", "/compiler/a-runtime.jar"],
            "current_jvm_tools_jar": null,
            "compiler_version": "2.4.0",
            "gradle_plugin_version": "2.4.0",
            "gradle_java_home": "/jdks/host-17",
            "toolchain_java_home": "/jdks/toolchain-8",
            "run_via_build_tools_api": true,
            "execution_strategy": "IN_PROCESS",
            "normalized_daemon_jvm_arguments": ["-Xmx768m", "-Dopaque=one\ttwo\n三"]
        })
    }

    fn plain_java_configuration() -> serde_json::Value {
        serde_json::json!({
            "task": ":app:compileJava", "language": "java",
            "task_type": "org.gradle.api.tasks.compile.JavaCompile_Decorated",
            "arguments": [], "annotation_processor_path": [],
            "source_files": ["/repo/app/src/main/java/Use.java"],
            "classpath": ["/repo/vendor.jar"], "java_home": "/jdk",
            "options": {
                "fork": false, "forkJavaHome": null,
                "forkExecutable": null, "forkJvmArgs": []
            },
            "output_roots": ["/repo/app/build/classes/java/main"],
            "complete": true, "problem": null
        })
    }

    fn selected_java_recipe() -> serde_json::Value {
        serde_json::json!({
            "adapter": "gradle-7.4.2-current-jdk17-javac-api-v1",
            "compiler_type": "org.gradle.jvm.toolchain.internal.DefaultToolchainJavaCompiler",
            "java_runtime_version": "17.0.7+8-LTS-224", "language_version": "17",
            "gradle_java_version": "17.0.7", "gradle_file_encoding": "MS932",
            "effective_charset": "UTF-8", "locale": "ja-JP",
            "api_options": ["-source", "17", "-target", "17", "-sourcepath", "",
                "-proc:none", "-XDuseUnsharedTable=true", "-classpath", "/repo/z.jar;/repo/a.jar", "-proc:none"]
        })
    }

    #[test]
    fn java_recipe_preserves_effective_defaults_and_full_option_order_in_model_cache() {
        let mut java = plain_java_configuration();
        java["options"]["javaCompilerRecipe"] = selected_java_recipe();
        java["options"]["javaCompilerRecipeProblem"] = serde_json::Value::Null;
        java["source_files"] = serde_json::json!([
            "/repo/app/src/main/java/Z.java",
            "/repo/app/src/main/java/Use.java"
        ]);
        java["classpath"] = serde_json::json!(["/repo/z.jar", "/repo/a.jar"]);
        let model = plain_java_snapshot(&[java.clone()]);
        let task = &model.modules[0].source_sets[0].compiler_tasks[0];
        let recipe = task.java_compiler_recipe().unwrap();
        assert_eq!(recipe.gradle_file_encoding, "MS932");
        assert_eq!(recipe.effective_charset, "UTF-8");
        assert_eq!(recipe.locale, "ja-JP");
        assert_eq!(recipe.api_options[5], "");
        assert_eq!(
            recipe
                .api_options
                .iter()
                .filter(|option| *option == "-proc:none")
                .count(),
            2
        );
        let serialized = serde_json::to_value(task).unwrap();
        assert_eq!(serialized["options"], java["options"]);
        assert_eq!(serialized["source_files"], java["source_files"]);
        assert_eq!(serialized["classpath"], java["classpath"]);
        let entry = CacheEntry {
            model_version: MODEL_VERSION,
            mtimes: BTreeMap::new(),
            modules: model.modules,
        };
        let decoded: CacheEntry =
            serde_json::from_slice(&serde_json::to_vec(&entry).unwrap()).unwrap();
        let cached_task = &decoded.modules[0].source_sets[0].compiler_tasks[0];
        assert_eq!(cached_task.java_compiler_recipe(), Some(recipe));
        assert_eq!(serde_json::to_value(cached_task).unwrap(), serialized);
    }

    #[test]
    fn java_recipe_unknown_or_malformed_retains_other_compiler_metadata_and_source_ownership() {
        let mut missing_options = selected_java_recipe();
        missing_options
            .as_object_mut()
            .unwrap()
            .remove("api_options");
        let mut future_adapter = selected_java_recipe();
        future_adapter["adapter"] = serde_json::json!("gradle-9.6.1");
        let mut wrong_options = selected_java_recipe();
        wrong_options["api_options"] = serde_json::json!(["-g", 19]);
        let mut wrong_runtime = selected_java_recipe();
        wrong_runtime["language_version"] = serde_json::json!("21");
        let mut empty_charset = selected_java_recipe();
        empty_charset["effective_charset"] = serde_json::json!("");
        let mut unknown_field = selected_java_recipe();
        unknown_field["source_files"] = serde_json::json!([]);
        for recipe in [
            serde_json::Value::Null,
            missing_options,
            future_adapter,
            wrong_options,
            wrong_runtime,
            empty_charset,
            unknown_field,
        ] {
            let mut java = plain_java_configuration();
            java["options"]["javaCompilerRecipe"] = recipe;
            java["options"]["javaCompilerRecipeProblem"] = serde_json::Value::Null;
            let model = plain_java_snapshot(&[java]);
            let source = &model.modules[0].source_sets[0];
            assert!(!source.compiler_metadata_incomplete);
            assert_eq!(source.compiler_tasks.len(), 1);
            assert!(source.compiler_tasks[0].java_compiler_recipe().is_none());
            assert!(model
                .plain_java_task("/repo/app/src/main/java/Use.java")
                .is_some());
            assert!(model
                .file_origin(Path::new("/repo/app/src/main/java/New.java"))
                .is_some());
        }
        let mut unavailable = plain_java_configuration();
        unavailable["options"]["javaCompilerRecipe"] = selected_java_recipe();
        unavailable["options"]["javaCompilerRecipeProblem"] =
            serde_json::json!("Java compiler providers are unhandled");
        let model = plain_java_snapshot(&[unavailable]);
        assert!(model.modules[0].source_sets[0].compiler_tasks[0]
            .java_compiler_recipe()
            .is_none());
        assert!(!model.modules[0].source_sets[0].compiler_metadata_incomplete);
        assert!(
            plain_java_snapshot(&[plain_java_configuration()]).modules[0].source_sets[0]
                .compiler_tasks[0]
                .java_compiler_recipe()
                .is_none()
        );
    }

    fn plain_java_snapshot(compilers: &[serde_json::Value]) -> ProjectSnapshot {
        let records = compilers
            .iter()
            .map(|compiler| format!("COMPILER\tmain\t{compiler}\n"))
            .collect::<String>();
        ProjectSnapshot::new(parse_dump(&format!(
            "PROJECT\t:app\t/repo/app\nSOURCE\tmain\t/repo/app/src/main/java\n{records}END\t:app\n"
        )))
    }

    #[test]
    fn plain_java_task_selects_the_caller_despite_kotlin_plugins() {
        let kotlin = serde_json::json!({
            "task": ":app:compileKotlin", "language": "kotlin", "task_type": "KotlinCompile",
            "arguments": ["-Xplugin=/plugins/transform.jar", "-P", "plugin:transform:enabled=true"],
            "annotation_processor_path": ["/plugins/kapt.jar"], "java_home": null,
            "options": {}, "output_roots": [], "complete": true, "problem": null
        });
        for task_type in [
            "org.gradle.api.tasks.compile.JavaCompile",
            "org.gradle.api.tasks.compile.JavaCompile_Decorated",
        ] {
            for jvm_args in [serde_json::Value::Null, serde_json::json!([])] {
                let mut java = plain_java_configuration();
                java["task_type"] = task_type.into();
                java["options"]["forkJvmArgs"] = jvm_args;
                let model = plain_java_snapshot(&[kotlin.clone(), java]);
                let task = model
                    .plain_java_task("/repo/app/src/main/java/Use.java")
                    .expect("the selected Java task has no configured extra transformations");
                assert_eq!(task.task, ":app:compileJava");
                assert_eq!(task.language, CompilerLanguage::Java);
                assert!(
                    model
                        .plain_java_task("/repo/app/src/main/java/New.java")
                        .is_some(),
                    "a new unsaved file uses its selected source set's compiler context"
                );
            }
        }
        #[cfg(windows)]
        {
            let mut java = plain_java_configuration();
            java["source_files"] = serde_json::json!([r"\repo\app\src\main\java\Use.java"]);
            assert!(plain_java_snapshot(&[java])
                .plain_java_task("/repo/app/src/main/java/Use.java")
                .is_some());
        }
    }

    #[test]
    fn plain_java_task_rejects_unproved_compiler_inputs() {
        let caller = "/repo/app/src/main/java/Use.java";
        for (label, field, value) in [
            (
                "Java plugin",
                "arguments",
                serde_json::json!(["-Xplugin:transform"]),
            ),
            (
                "argument file",
                "arguments",
                serde_json::json!(["@compiler.args"]),
            ),
            (
                "unclassified argument",
                "arguments",
                serde_json::json!(["-parameters"]),
            ),
            (
                "processor path",
                "annotation_processor_path",
                serde_json::json!(["/plugins/ap.jar"]),
            ),
            (
                "custom compiler task",
                "task_type",
                serde_json::json!("custom.JavaCompile"),
            ),
            ("incomplete capture", "complete", serde_json::json!(false)),
            (
                "capture problem",
                "problem",
                serde_json::json!("provider unavailable"),
            ),
            ("unknown JDK", "java_home", serde_json::Value::Null),
            (
                "empty source inventory",
                "source_files",
                serde_json::json!([]),
            ),
        ] {
            let mut java = plain_java_configuration();
            java[field] = value;
            assert!(
                plain_java_snapshot(&[java])
                    .plain_java_task(caller)
                    .is_none(),
                "{label}"
            );
        }
        for (option, value) in [
            ("fork", serde_json::json!(true)),
            ("fork", serde_json::json!("false")),
            ("fork", serde_json::Value::Null),
            ("forkJavaHome", serde_json::json!("/other-jdk")),
            ("forkJavaHome", serde_json::json!(false)),
            ("forkExecutable", serde_json::json!("/custom/javac")),
            ("forkExecutable", serde_json::json!(false)),
            (
                "forkJvmArgs",
                serde_json::json!(["-javaagent:transform.jar"]),
            ),
            ("forkJvmArgs", serde_json::json!("")),
            ("forkJvmArgs", serde_json::json!({})),
        ] {
            let mut java = plain_java_configuration();
            java["options"][option] = value;
            assert!(
                plain_java_snapshot(&[java])
                    .plain_java_task(caller)
                    .is_none(),
                "{option}"
            );
        }
        for option in ["fork", "forkJavaHome", "forkExecutable", "forkJvmArgs"] {
            let mut java = plain_java_configuration();
            java["options"].as_object_mut().unwrap().remove(option);
            assert!(
                plain_java_snapshot(&[java])
                    .plain_java_task(caller)
                    .is_none(),
                "missing {option}"
            );
        }
        let mut legacy = plain_java_configuration();
        legacy.as_object_mut().unwrap().remove("source_files");
        assert!(plain_java_snapshot(&[legacy])
            .plain_java_task(caller)
            .is_none());
    }

    #[test]
    fn plain_java_task_accepts_only_the_explicit_standalone_ap_disable_flag() {
        let caller = "/repo/app/src/main/java/Use.java";
        let mut java = plain_java_configuration();
        java["arguments"] = serde_json::json!(["-proc:none"]);
        let model = plain_java_snapshot(&[java.clone()]);
        assert!(model
            .plain_java_task(caller)
            .unwrap()
            .annotation_processing_disabled());
        for args in [
            serde_json::json!(["-proc:none", "-Xplugin:transform"]),
            serde_json::json!(["-proc:none", "@compiler.args"]),
            serde_json::json!(["-proc:none", "-parameters"]),
            serde_json::json!(["-proc:none", "-proc:only"]),
        ] {
            java["arguments"] = args;
            assert!(plain_java_snapshot(&[java.clone()])
                .plain_java_task(caller)
                .is_none());
        }
        java["arguments"] = serde_json::json!(["-proc:none"]);
        java["java_home"] = serde_json::Value::Null;
        assert!(plain_java_snapshot(&[java])
            .plain_java_task(caller)
            .is_none());
    }

    #[test]
    fn plain_java_task_accepts_an_applied_toolchain_home_only_when_it_matches_the_selected_jdk() {
        let caller = "/repo/app/src/main/java/Use.java";
        for args in [serde_json::json!([]), serde_json::json!(["-proc:none"])] {
            let mut java = plain_java_configuration();
            java["arguments"] = args;
            java["options"]["forkJavaHome"] = java["java_home"].clone();
            assert!(plain_java_snapshot(&[java.clone()])
                .plain_java_task(caller)
                .is_some());
            java["options"]["forkJavaHome"] = serde_json::json!("/other-jdk");
            assert!(plain_java_snapshot(&[java.clone()])
                .plain_java_task(caller)
                .is_none());
            java["options"]["forkJavaHome"] = serde_json::json!("/jdk");
            java["java_home"] = serde_json::Value::Null;
            assert!(plain_java_snapshot(&[java])
                .plain_java_task(caller)
                .is_none());
        }
    }

    #[test]
    fn plain_java_task_requires_one_complete_selected_source_set() {
        let caller = "/repo/app/src/main/java/Use.java";
        assert!(plain_java_snapshot(&[]).plain_java_task(caller).is_none());
        let java = plain_java_configuration();
        let mut duplicate = java.clone();
        duplicate["task"] = ":app:anotherJavaCompile".into();
        assert!(plain_java_snapshot(&[java.clone(), duplicate])
            .plain_java_task(caller)
            .is_none());
        for reason in ["compiler metadata", "classpath", "unresolved dependency"] {
            let mut model = plain_java_snapshot(&[java.clone()]);
            let source = &mut model.modules[0].source_sets[0];
            match reason {
                "compiler metadata" => source.compiler_metadata_incomplete = true,
                "classpath" => source.classpath_incomplete = true,
                _ => source.unresolved.push("vendor:missing:1".into()),
            }
            assert!(model.plain_java_task(caller).is_none(), "{reason}");
        }
        let model = plain_java_snapshot(&[java]);
        for outside in [
            "/repo/app/Use.java",
            "/repo/app/src/main/java2/Use.java",
            "/other/Use.java",
        ] {
            assert!(model.plain_java_task(outside).is_none(), "{outside}");
        }
        let java = plain_java_configuration();
        let model = ProjectSnapshot::new(parse_dump(&format!(
            "PROJECT\t:app\t/repo/app\nSOURCE\tmain\t/repo/app/src/main/java\nCOMPILER\tmain\t{java}\nSOURCE\ttest\t/repo/app/src/test/java\nEND\t:app\n"
        )));
        assert!(model
            .plain_java_task("/repo/app/src/main/java/New.java")
            .is_some());
        assert!(
            model
                .plain_java_task("/repo/app/src/test/java/New.java")
                .is_none(),
            "another source set cannot borrow the main Java task"
        );
    }

    #[test]
    fn compiler_configuration_preserves_task_identity_and_opaque_options() {
        let mut compiler = serde_json::json!({
            "task": ":app:compileKotlin", "language": "kotlin", "task_type": "KotlinCompile",
            "arguments": ["-Xplugin=C:\\plugins\\serializer.jar", "-P", "annotation=one\ttwo\n三", "-jdk-home", "/jdks/explicit-21"],
            "annotation_processor_path": [], "java_home": "/jdks/explicit-21",
            "kotlin_compiler": selected_kotlin_recipe(),
            "source_files": ["/app/src/First.kt", "/app/src/Second.kt"],
            "classpath": ["/app/first.jar", "/app/second.jar"],
            "declared_input_files": ["/app/src/First.kt", "/app/first.jar"],
            "declared_output_roots": ["/app/classes/kotlin", "/app/task-state.bin"],
            "options": {"jvmTarget": "17"}, "output_roots": ["/app/classes/kotlin"],
            "complete": true, "problem": null
        });
        let mut legacy_recipe = selected_kotlin_recipe();
        legacy_recipe["current_jvm_tools_jar"] = serde_json::json!("/jdks/host-8/lib/tools.jar");
        legacy_recipe["compiler_version"] = serde_json::json!("1.9.23");
        legacy_recipe["gradle_plugin_version"] = serde_json::json!("1.9.23");
        legacy_recipe["gradle_java_home"] = serde_json::json!("/jdks/host-8");
        legacy_recipe["run_via_build_tools_api"] = serde_json::json!(false);
        legacy_recipe["execution_strategy"] = serde_json::json!("DAEMON");
        let mut legacy18_recipe = legacy_recipe.clone();
        legacy18_recipe["compiler_version"] = serde_json::json!("1.8.21");
        legacy18_recipe["gradle_plugin_version"] = serde_json::json!("1.8.21");
        for recipe in [selected_kotlin_recipe(), legacy_recipe, legacy18_recipe] {
            compiler["kotlin_compiler"] = recipe;
            let dump = format!(
                "PROJECT\t:app\t/app\nSOURCESET\tmain\nCOMPILER\tmain\t{compiler}\nEND\t:app\n"
            );
            let modules = parse_dump_reader(dump.as_bytes(), true).unwrap();
            let value = serde_json::to_value(&modules[0]).unwrap();
            assert_eq!(value["source_sets"][0]["compiler_tasks"][0], compiler);
        }
    }

    #[test]
    fn primary_kotlin_compilation_is_explicit_scoped_and_round_trips() {
        let selected = ":app:customPrimaryJvmTask";
        let dump = format!(
            "PROJECT\t:app\t/repo/app\nSOURCE\tmain\t/repo/app/src/main\nKOTLIN_COMPILE_TASK\tmain\t{selected:?}\nKOTLIN_COMPILE_TASK\ttest\tnull\nEND\t:app\nPROJECT\t:other\t/repo/other\nKOTLIN_COMPILE_TASK\tmain\t\":other:primary\"\nEND\t:other\n"
        );
        let modules = parse_dump_reader(dump.as_bytes(), true).unwrap();
        assert_eq!(
            modules[0].source_sets[0].kotlin_compile_task.as_deref(),
            Some(selected)
        );
        assert!(modules[0].source_sets[1].kotlin_compile_task.is_none());
        assert_eq!(
            modules[1].source_sets[0].kotlin_compile_task.as_deref(),
            Some(":other:primary")
        );
        let bytes = serde_json::to_vec(&modules).unwrap();
        assert_eq!(
            serde_json::from_slice::<Vec<ModuleClasspath>>(&bytes).unwrap(),
            modules
        );
    }

    #[test]
    fn missing_malformed_or_duplicate_primary_kotlin_compilation_remains_unknown() {
        for records in [
            String::new(),
            "KOTLIN_COMPILE_TASK\tmain\tnull\n".into(),
            "KOTLIN_COMPILE_TASK\tmain\t42\n".into(),
            "KOTLIN_COMPILE_TASK\tmain\t\"\"\n".into(),
            "KOTLIN_COMPILE_TASK\tmain\t\"bad\\npath\"\n".into(),
            "KOTLIN_COMPILE_TASK\tmain\t\":app:selected\"\textra\n".into(),
            "KOTLIN_COMPILE_TASK\tmain\t\":app:first\"\nKOTLIN_COMPILE_TASK\tmain\t\":app:second\"\nKOTLIN_COMPILE_TASK\tmain\t\":app:first\"\n".into(),
        ] {
            let dump = format!(
                "PROJECT\t:app\t/repo/app\nSOURCE\tmain\t/repo/app/src/main\n{records}END\t:app\n"
            );
            let modules = parse_dump_reader(dump.as_bytes(), true).unwrap();
            let source = &modules[0].source_sets[0];
            assert!(source.kotlin_compile_task.is_none(), "{records}");
            assert_eq!(source.source_roots, [PathBuf::from("/repo/app/src/main")]);
        }
    }

    #[test]
    fn malformed_kotlin_compiler_recipe_retains_source_ownership() {
        for (field, replacement) in [
            ("default_classpath", None),
            ("gradle_plugin_version", None),
            (
                "execution_strategy",
                Some(serde_json::json!("FUTURE_STRATEGY")),
            ),
            (
                "normalized_daemon_jvm_arguments",
                Some(serde_json::json!([19])),
            ),
        ] {
            let mut recipe = selected_kotlin_recipe();
            if let Some(value) = replacement {
                recipe[field] = value;
            } else {
                recipe.as_object_mut().unwrap().remove(field);
            }
            let compiler = serde_json::json!({
                "task": ":app:compileKotlin", "language": "kotlin", "task_type": "KotlinCompile",
                "arguments": [], "annotation_processor_path": [], "java_home": null,
                "kotlin_compiler": recipe, "options": {}, "output_roots": [],
                "complete": true, "problem": null
            });
            let dump = format!(
                "PROJECT\t:app\t/repo/app\nSOURCE\tmain\t/repo/app/src\nCOMPILER\tmain\t{compiler}\nSCP\tmain\t/repo/dependency.jar\nEND\t:app\n"
            );
            let modules = parse_dump_reader(dump.as_bytes(), true).unwrap();
            let source = &modules[0].source_sets[0];
            assert!(source.compiler_metadata_incomplete, "{field}");
            assert!(source.compiler_tasks.is_empty(), "{field}");
            assert_eq!(source.source_roots, [PathBuf::from("/repo/app/src")]);
            assert_eq!(source.entries, [PathBuf::from("/repo/dependency.jar")]);
            let model = ProjectSnapshot::new(modules);
            assert!(model
                .file_origin(Path::new("/repo/app/src/New.kt"))
                .is_some());
        }
    }

    #[test]
    fn compiler_metadata_failure_cannot_erase_source_ownership_or_claim_completeness() {
        let compiler = serde_json::json!({
            "task": ":compileKotlin", "language": "kotlin", "task_type": "UnsupportedKotlinCompile",
            "arguments": [], "annotation_processor_path": [], "java_home": null,
            "options": {}, "output_roots": [], "complete": false,
            "problem": "Serialized compiler arguments are unavailable"
        });
        let modules = parse_dump(&format!(
            "PROJECT\t:\t/repo\nSOURCE\tmain\t/repo/src\nCOMPILER\tmain\t{compiler}\n\
             COMPILER\tmain\t{{malformed\nSCP\tmain\t/repo/api.jar\nEND\t:\n"
        ));
        let source = &modules[0].source_sets[0];
        assert_eq!(source.source_roots, [PathBuf::from("/repo/src")]);
        assert_eq!(source.entries, [PathBuf::from("/repo/api.jar")]);
        assert!(source.compiler_metadata_incomplete);
        assert_eq!(source.compiler_tasks.len(), 1);
        assert!(!source.compiler_tasks[0].complete);
        let model = ProjectSnapshot::new(modules);
        assert!(model.file_origin(Path::new("/repo/src/Use.kt")).is_some());
    }

    use super::*;
    #[test]
    fn parse_single_module() {
        let dump = "PROJECT\t:lib\t/abs/lib\nCP\t/jars/a.jar\nCP\t/jars/b.jar\nEND\t:lib\n";
        let m = parse_dump(dump);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].module, ":lib");
        assert_eq!(m[0].project_dir, PathBuf::from("/abs/lib"));
        assert_eq!(
            m[0].entries,
            vec![PathBuf::from("/jars/a.jar"), PathBuf::from("/jars/b.jar")]
        );
    }

    #[test]
    fn parse_multiple_modules_with_chatter() {
        let dump = "\
> Configure project :app\n\
PROJECT\t:app\t/abs/app\n\
CP\t/jars/lib.jar\n\
CP\t/jars/stdlib.jar\n\
END\t:app\n\
some gradle log line\n\
PROJECT\t:lib\t/abs/lib\n\
CP\t/jars/stdlib.jar\n\
END\t:lib\n";
        let m = parse_dump(dump);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].module, ":app");
        assert_eq!(m[0].entries.len(), 2);
        assert_eq!(m[1].module, ":lib");
        assert_eq!(m[1].entries, vec![PathBuf::from("/jars/stdlib.jar")]);
    }

    #[test]
    fn parse_empty_is_empty() {
        assert!(parse_dump("no records here\njust gradle output\n").is_empty());
    }

    #[test]
    fn parse_unresolved_lines_are_attributed() {
        let dump = "PROJECT\t:lib\t/abs/lib\nCP\t/jars/a.jar\nUNRESOLVED\tcom.acme:lib:1.0\tCould not resolve\nEND\t:lib\n";
        let m = parse_dump(dump);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].entries, vec![PathBuf::from("/jars/a.jar")]);
        assert_eq!(
            m[0].unresolved,
            vec!["com.acme:lib:1.0: Could not resolve".to_string()]
        );
    }

    #[test]
    fn cache_entry_without_unresolved_field_deserializes() {
        // Legacy records remain readable for tooling but cannot satisfy the new model cache.
        let json = r#"{"mtimes":{},"modules":[{"module":":lib","project_dir":"/abs/lib","entries":["/jars/a.jar"]}]}"#;
        let entry: CacheEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.modules[0].module, ":lib");
        assert!(entry.modules[0].unresolved.is_empty());
        assert_ne!(entry.model_version, MODEL_VERSION);
    }

    #[test]
    fn best_effort_parse_block_without_end_is_flushed() {
        let dump = "PROJECT\t:lib\t/abs/lib\nCP\t/jars/a.jar\n";
        let m = parse_dump(dump);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].entries, vec![PathBuf::from("/jars/a.jar")]);
    }

    #[test]
    fn strict_streaming_parser_rejects_partial_or_mismatched_blocks_after_complete_prefix() {
        let head = "PROJECT\t:head\t/head\nCP\t/head.jar\nEND\t:head\n";
        for tail in [
            "PROJECT\t:tail\t/tail\nCP\t/tail.jar\n",
            "PROJECT\t:tail\t/tail\nEND\t:other\n",
            "PROJECT\t:tail\t/tail\nPROJECT\t:nested\t/nested\nEND\t:nested\n",
            "END\t:head\n",
            "CP\t/orphan.jar\n",
            "PROJECT\t\t/tail\nEND\t\n",
            "PROJECT\t:tail\t\nEND\t:tail\n",
            "PROJECT\t:tail\t/tail\nEND\t:tail\textra\n",
            "PROJECT\t:tail\t/tail\nEND\t:tail",
        ] {
            let text = format!("{head}{tail}");
            let reader = BufReader::with_capacity(3, text.as_bytes());
            assert!(parse_dump_reader(reader, true).is_err(), "{tail:?}");
        }
        let reader = BufReader::with_capacity(3, head.as_bytes());
        let modules = parse_dump_reader(reader, true).unwrap();
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].entries, [PathBuf::from("/head.jar")]);
    }

    #[test]
    fn strict_streaming_parser_preserves_large_compiler_records_across_buffers() {
        let mut compiler = plain_java_configuration();
        compiler["declared_input_files"] = serde_json::json!([]);
        compiler["declared_output_roots"] = serde_json::json!([]);
        compiler["arguments"] = serde_json::json!(["-P", "路\t\n".repeat(150_000)]);
        let dump = format!(
            "chatter\r\nPROJECT\t:app\t/app\r\nCOMPILER\tmain\t{compiler}\r\nEND\t:app\r\n"
        );
        let reader = BufReader::with_capacity(17, dump.as_bytes());
        let modules = parse_dump_reader(reader, true).unwrap();
        assert_eq!(modules.len(), 1);
        let task = &modules[0].source_sets[0].compiler_tasks[0];
        assert_eq!(serde_json::to_value(task).unwrap(), compiler);
    }

    #[test]
    fn strict_streaming_parser_reports_read_and_utf8_errors_after_complete_prefix() {
        use std::io::Read;
        struct FailsAtEof(std::io::Cursor<&'static [u8]>);
        impl Read for FailsAtEof {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                match self.0.read(buffer)? {
                    0 => Err(std::io::Error::other("fixture read failure")),
                    n => Ok(n),
                }
            }
        }
        let head = b"PROJECT\t:head\t/head\nEND\t:head\n";
        let reader = BufReader::with_capacity(3, FailsAtEof(std::io::Cursor::new(head)));
        let error = parse_dump_reader(reader, true).unwrap_err();
        assert!(error.to_string().contains("fixture read failure"));
        let mut invalid = head.to_vec();
        invalid.extend_from_slice(b"\xff\n");
        assert!(parse_dump_reader(BufReader::new(invalid.as_slice()), true).is_err());
    }

    #[test]
    fn embedded_script_registers_late_configurations_with_supported_resolution_provider() {
        assert!(INIT_SCRIPT.contains("ktlspDumpClasspath"));
        assert!(INIT_SCRIPT.contains("compileClasspath"));
        assert!(INIT_SCRIPT.contains("configurations.matching"));
        assert!(INIT_SCRIPT.contains("resolutionResult.rootComponent"));
        assert!(INIT_SCRIPT.contains("UnresolvedDependencyResult"));
        assert!(!INIT_SCRIPT.contains("afterEvaluate {"));
        assert!(!INIT_SCRIPT.contains("resolvedConfiguration"));
    }

    #[test]
    fn coordinate_from_gradle_cache_path_parses_maven_jar() {
        let p = PathBuf::from(
            "/Users/me/.gradle/caches/modules-2/files-2.1/org.springframework.boot/spring-boot/3.3.2/72a257d/spring-boot-3.3.2.jar",
        );
        let c = super::coordinate_from_gradle_cache_path(&p).unwrap();
        assert_eq!(c.group, "org.springframework.boot");
        assert_eq!(c.artifact, "spring-boot");
        assert_eq!(c.version, "3.3.2");
    }

    #[test]
    fn coordinate_from_gradle_cache_path_rejects_non_cache_paths() {
        assert!(
            super::coordinate_from_gradle_cache_path(&PathBuf::from("/jars/lib.jar")).is_none()
        );
        assert!(super::coordinate_from_gradle_cache_path(&PathBuf::from(
            "/build/classes/java/main"
        ))
        .is_none());
    }

    #[test]
    fn coordinate_from_gradle_cache_path_rejects_mismatched_jar_name() {
        // The file name must start with `{artifact}-{version}`.
        let p = PathBuf::from(
            "/Users/me/.gradle/caches/modules-2/files-2.1/org.example/lib/1.0/abc/other-1.0.jar",
        );
        assert!(super::coordinate_from_gradle_cache_path(&p).is_none());
    }

    #[test]
    fn quoted_files_jar_paths_finds_literal_files_dependencies() {
        let text = r#"
dependencies {
    api(files("libs/datatester-java-sdk-2.0.24.jar"))
    implementation(files('libs/other.jar', "libs/not-a-jar.txt"))
    api(files("$generated.jar"))
}
"#;
        assert_eq!(
            super::quoted_files_jar_paths(text),
            vec![
                PathBuf::from("libs/datatester-java-sdk-2.0.24.jar"),
                PathBuf::from("libs/other.jar")
            ]
        );
    }

    #[test]
    fn local_jars_from_build_files_scans_gradle_file_dependencies() {
        let root =
            std::env::temp_dir().join(format!("ktlsp_classpath_local_jars_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let module = root.join("shared-common");
        std::fs::create_dir_all(module.join("libs")).unwrap();
        std::fs::write(module.join("libs/datatester-java-sdk-2.0.24.jar"), "").unwrap();
        std::fs::write(
            module.join("build.gradle.kts"),
            r#"dependencies { api(files("libs/datatester-java-sdk-2.0.24.jar")) }"#,
        )
        .unwrap();

        let jars = super::local_jars_from_build_files(&root);
        assert_eq!(
            jars,
            vec![module.join("libs/datatester-java-sdk-2.0.24.jar")]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cache_entry_write_is_atomic_and_readable() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp_classpath_cache_write_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let path = root.join("classpath.json");
        let entry = CacheEntry {
            model_version: MODEL_VERSION,
            mtimes: BTreeMap::from([("build.gradle.kts".to_string(), 42)]),
            modules: vec![ModuleClasspath {
                module: ":app".to_string(),
                project_dir: PathBuf::from("/project/app"),
                build: ":".to_string(),
                source_sets: Vec::new(),
                entries: vec![PathBuf::from("/jars/app.jar")],
                unresolved: Vec::new(),
            }],
        };

        write_cache_entry(&path, &entry);

        let stored: CacheEntry =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(stored.modules[0].module, ":app");
        assert!(std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .all(|entry| entry.path().extension().and_then(|ext| ext.to_str()) != Some("tmp")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cache_stream_failure_keeps_valid_cache_and_removes_partial_temp() {
        struct PrefixThenError<'a> {
            root: &'a Path,
            reached_disk: &'a std::cell::Cell<bool>,
        }
        impl Serialize for PrefixThenError<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                use serde::ser::SerializeMap;
                let mut map = serializer.serialize_map(Some(2))?;
                let capacity = std::io::BufWriter::new(std::io::sink()).capacity();
                map.serialize_entry("partial", &"x".repeat(capacity * 2 + 1024))?;
                self.reached_disk
                    .set(
                        std::fs::read_dir(self.root)
                            .unwrap()
                            .flatten()
                            .any(|entry| {
                                entry.path().extension().is_some_and(|ext| ext == "tmp")
                                    && std::fs::read(entry.path())
                                        .is_ok_and(|bytes| !bytes.is_empty())
                            }),
                    );
                Err(serde::ser::Error::custom("failure after streaming prefix"))
            }
        }

        let root =
            std::env::temp_dir().join(format!("ktlsp_cache_stream_failure_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("classpath.json");
        let original = serde_json::to_vec(&CacheEntry {
            model_version: MODEL_VERSION,
            mtimes: BTreeMap::new(),
            modules: Vec::new(),
        })
        .unwrap();
        std::fs::write(&path, &original).unwrap();
        let reached_disk = std::cell::Cell::new(false);
        write_cache_entry(
            &path,
            &PrefixThenError {
                root: &root,
                reached_disk: &reached_disk,
            },
        );

        assert_eq!(std::fs::read(&path).unwrap(), original);
        let stored: CacheEntry = serde_json::from_slice(&original).unwrap();
        assert_eq!(stored.model_version, MODEL_VERSION);
        assert!(std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .all(|entry| { entry.path().extension().is_none_or(|ext| ext != "tmp") }));
        assert!(
            reached_disk.get(),
            "failure must occur after a real partial disk write"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cache_concurrent_writers_publish_one_whole_json_and_remove_owned_temps() {
        #[derive(Serialize)]
        struct Payload {
            writer: &'static str,
            payload: String,
            complete: bool,
        }
        struct Paused<'a> {
            value: &'a Payload,
            ready: &'a std::sync::Barrier,
            release: &'a std::sync::Barrier,
        }
        impl Serialize for Paused<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                use serde::ser::SerializeStruct;
                let mut record = serializer.serialize_struct("Payload", 3)?;
                record.serialize_field("writer", self.value.writer)?;
                record.serialize_field("payload", &self.value.payload)?;
                self.ready.wait();
                self.release.wait();
                record.serialize_field("complete", &self.value.complete)?;
                record.end()
            }
        }

        let root =
            std::env::temp_dir().join(format!("ktlsp_cache_concurrent_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("classpath.json");
        let capacity = std::io::BufWriter::new(std::io::sink()).capacity();
        let values = [
            Payload {
                writer: "first",
                payload: "a".repeat(capacity * 2 + 1024),
                complete: true,
            },
            Payload {
                writer: "second",
                payload: "b".repeat(capacity * 3 + 1024),
                complete: true,
            },
        ];
        let expected = values
            .iter()
            .map(|value| serde_json::to_vec(value).unwrap())
            .collect::<Vec<_>>();
        let ready = std::sync::Barrier::new(3);
        let release = std::sync::Barrier::new(3);
        std::thread::scope(|scope| {
            let cache_path = &path;
            let ready_ref = &ready;
            let release_ref = &release;
            let jobs = values
                .iter()
                .map(|value| {
                    scope.spawn(move || {
                        write_cache_entry(
                            cache_path,
                            &Paused {
                                value,
                                ready: ready_ref,
                                release: release_ref,
                            },
                        );
                    })
                })
                .collect::<Vec<_>>();
            ready.wait();
            let temps = std::fs::read_dir(&root)
                .unwrap()
                .flatten()
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "tmp"))
                .collect::<Vec<_>>();
            let partial_writes = temps
                .iter()
                .all(|entry| !std::fs::read(entry.path()).unwrap().is_empty());
            release.wait();
            for job in jobs {
                job.join().unwrap();
            }
            assert_eq!(
                temps.len(),
                2,
                "each concurrent writer must own a distinct temp file"
            );
            assert!(partial_writes);
        });
        let stored = std::fs::read(&path).unwrap();
        assert!(
            expected.contains(&stored),
            "cache must be one complete writer's JSON"
        );
        assert!(std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .all(|entry| { entry.path().extension().is_none_or(|ext| ext != "tmp") }));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cache_snapshot_round_trip_preserves_wire_format_and_does_not_publish() {
        let root =
            std::env::temp_dir().join(format!("ktlsp_cache_snapshot_{}", std::process::id()));
        std::fs::create_dir_all(root.join("mapped-module")).unwrap();
        std::fs::write(root.join("build.gradle.kts"), "plugins { java }").unwrap();
        let compiler = serde_json::json!({
            "task": ":app:compileKotlin", "language": "kotlin", "task_type": "KotlinCompile",
            "arguments": ["-P", "annotation=one\ttwo\n三"], "annotation_processor_path": [],
            "source_files": ["/app/Use.kt", "/app/Main.java", "/app/Script.kts"],
            "classpath": ["/app/z-first.jar", "/app/a-second.jar"],
            "java_home": "/jdks/selected-21", "options": {"jvmTarget": "17"}, "output_roots": [],
            "kotlin_compiler": selected_kotlin_recipe(),
            "complete": false, "problem": "opaque producer"
        });
        let modules = parse_dump(&format!(
            "PROJECT\t:app\t{}\nSOURCE\tmain\t{}\nOUTPUT\tmain\t{}\nKOTLIN_COMPILE_TASK\tmain\t\":app:compileKotlin\"\nCOMPILER\tmain\t{compiler}\nEND\t:app\n",
            root.join("mapped-module").display(), root.join("custom-kotlin").display(),
            root.join("custom-output").display()
        ));
        let mut model = ProjectSnapshot::new(modules);
        model.mtimes = build_file_mtimes(&root, &model.modules);
        let original = serde_json::to_vec(&CacheEntry {
            model_version: MODEL_VERSION,
            mtimes: model.mtimes.clone(),
            modules: model.modules.clone(),
        })
        .unwrap();
        store_snapshot(&root, &model);
        let cache = cache_path(&root);
        assert_eq!(std::fs::read(&cache).unwrap(), original);
        let loaded = load_snapshot_with(&root, false, || {
            panic!("valid disk cache must avoid Gradle")
        })
        .unwrap();
        assert_eq!(loaded.modules, model.modules);
        assert_eq!(loaded.mtimes, model.mtimes);
        assert!(snapshot(&root).is_none(), "loading must remain unpublished");

        let mut legacy_modules = model.modules.clone();
        legacy_modules[0].source_sets[0].compiler_tasks[0].kotlin_compiler = None;
        for invalid in [
            original[..original.len() - 1].to_vec(),
            [original.as_slice(), b" trailing"].concat(),
            serde_json::to_vec(&CacheEntry {
                model_version: 6,
                mtimes: model.mtimes.clone(),
                modules: legacy_modules,
            })
            .unwrap(),
            serde_json::to_vec(&CacheEntry {
                model_version: 7,
                mtimes: model.mtimes.clone(),
                modules: model.modules.clone(),
            })
            .unwrap(),
            serde_json::to_vec(&CacheEntry {
                model_version: 8,
                mtimes: model.mtimes.clone(),
                modules: model.modules.clone(),
            })
            .unwrap(),
            serde_json::to_vec(&CacheEntry {
                // Public bootstrap metadata alone predates the selected Java API recipe.
                model_version: 10,
                mtimes: model.mtimes.clone(),
                modules: model.modules.clone(),
            })
            .unwrap(),
        ] {
            std::fs::write(&cache, invalid).unwrap();
            let calls = std::cell::Cell::new(0);
            let loaded = load_snapshot_with(&root, false, || {
                calls.set(calls.get() + 1);
                Ok(model.modules.clone())
            })
            .unwrap();
            assert_eq!(
                calls.get(),
                1,
                "invalid or obsolete cache must use the existing dump fallback"
            );
            assert_eq!(loaded.modules, model.modules);
            assert!(snapshot(&root).is_none());
        }
        std::fs::remove_file(cache).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn actual_components_distinguish_maven_project_and_local_jars() {
        let dump = "PROJECT\t:app\t/repo/modules/editing\nBUILD\t:\nSOURCESET\tmain\n\
SOURCE\tmain\t/repo/generated/custom\nOUTPUT\tmain\t/repo/classes\n\
SCP\tmain\t/repo/home/.m2/repository/vendor/api/2/api-2.jar\n\
SCP\tmain\t/repo/libs/local.jar\nSCP\tmain\t/repo/modules/library/build/api.jar\n\
ARTIFACT\tmain\t/repo/home/.m2/repository/vendor/api/2/api-2.jar\tMODULE\tvendor\tapi\t2\n\
ARTIFACT\tmain\t/repo/modules/library/build/api.jar\tPROJECT\t:included\t:library\nEND\t:app\n";
        let modules = parse_dump(dump);
        let module = &modules[0];
        assert_eq!(
            module.source_sets[0].source_roots,
            vec![PathBuf::from("/repo/generated/custom")]
        );
        assert_eq!(
            module.source_sets[0].output_roots,
            vec![PathBuf::from("/repo/classes")]
        );
        assert_eq!(
            module_coordinates(module).into_iter().collect::<Vec<_>>(),
            vec![Coordinate::parse("vendor:api:2").unwrap()]
        );
        assert_eq!(
            module_local_jars(module).into_iter().collect::<Vec<_>>(),
            vec![PathBuf::from("/repo/libs/local.jar")]
        );
        assert_eq!(
            module.source_sets[0].artifacts[1].component,
            ComponentId::Project {
                build: ":included".to_string(),
                project: ":library".to_string(),
            }
        );
        let mut modules = modules;
        modules.extend(parse_dump(
            "PROJECT\t:unsupported\t/repo/unsupported\n\
CP\t/cache/modules-2/files-2.1/guessed/library/9/hash/library-9.jar\n\
CP\t/repo/unsupported/library.jar\nEND\t:unsupported\n",
        ));
        let model = ProjectSnapshot::new(modules);
        assert_eq!(
            model.coordinates(),
            vec![Coordinate::parse("vendor:api:2").unwrap()]
        );
        assert_eq!(
            model.local_jars(),
            vec![PathBuf::from("/repo/libs/local.jar")]
        );
    }

    #[test]
    fn actual_module_versions_with_plus_remain_source_coordinates() {
        let coordinate = Coordinate {
            group: "net.createmod.ponder".into(),
            artifact: "ponder-neoforge".into(),
            version: "1.0.87+mc1.21.1".into(),
        };
        let jar = "/cache/modules-2/files-2.1/net.createmod.ponder/ponder-neoforge/1.0.87+mc1.21.1/hash/ponder-neoforge-1.0.87+mc1.21.1.jar";
        let modules = parse_dump(&format!(
            "PROJECT\t:app\t/repo/app\nSOURCESET\tmain\nSCP\tmain\t{jar}\nARTIFACT\tmain\t{jar}\tMODULE\tnet.createmod.ponder\tponder-neoforge\t1.0.87+mc1.21.1\nEND\t:app\n"
        ));
        let model = ProjectSnapshot::new(modules);
        assert_eq!(model.coordinates(), vec![coordinate.clone()]);
        assert!(model.local_jars().is_empty());
        assert_eq!(
            coordinate_from_gradle_cache_path(Path::new(jar)),
            Some(coordinate)
        );
    }

    #[test]
    fn ancestor_lookup_work_is_bounded_by_path_depth_not_root_count() {
        use std::cell::Cell;
        for count in [8, 5063, 10_000] {
            let mut roots: BTreeMap<String, usize> = (0..count)
                .map(|n| (format!("/workspace/sources/module-{n}/src"), n))
                .collect();
            let key = "/workspace/docs/edit.txt";
            let probes = Cell::new(0);
            assert_eq!(
                find_ancestor(key, |ancestor| {
                    probes.set(probes.get() + 1);
                    roots.get(ancestor).copied()
                }),
                None
            );
            assert_eq!(
                probes.get(),
                key.bytes().filter(|byte| *byte == b'/').count() + 1
            );
            roots.insert("/workspace/docs".to_string(), count);
            probes.set(0);
            assert_eq!(
                find_ancestor(key, |ancestor| {
                    probes.set(probes.get() + 1);
                    roots.get(ancestor).copied()
                }),
                Some(count)
            );
            assert_eq!(probes.get(), 2);
        }
    }

    #[test]
    fn normalized_owner_lookup_preserves_longest_source_and_first_owner_rules() {
        let model = ProjectSnapshot::new(parse_dump(
            "PROJECT\t:module\t/scope/shared\nEND\t:module\n\
             PROJECT\t:first\t/owner/first\nSOURCE\tmain\t/scope/shared\nEND\t:first\n\
             PROJECT\t:second\t/owner/second\nSOURCE\ttest\t/scope/shared\nEND\t:second\n\
             PROJECT\t:deep\t/scope/shared/deep\nEND\t:deep\n\
             PROJECT\t:first-module\t/scope/duplicates\nEND\t:first-module\n\
             PROJECT\t:second-module\t/scope/duplicates\nEND\t:second-module\n",
        ));
        let (module, source) = model.owner_for_key("/scope/shared/Unsaved.kt").unwrap();
        assert_eq!(module.module, ":first");
        assert_eq!(source.unwrap().name, "main");
        let (module, source) = model
            .owner_for_key("/scope/shared/deep/Unregistered.kt")
            .unwrap();
        assert_eq!(module.module, ":deep");
        assert!(
            source.is_none(),
            "a longer module root still wins over a shorter source root"
        );
        let (module, source) = model.owner_for_key("/scope/duplicates/Main.kt").unwrap();
        assert_eq!(module.module, ":first-module");
        assert!(source.is_none());
        assert!(model
            .owner_for_key("/scope/shared-other/Unrelated.kt")
            .is_none());
    }

    #[test]
    fn registered_source_root_keeps_its_actual_canonical_alias() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp_snapshot_source_alias_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("alias")).unwrap();
        std::fs::create_dir_all(root.join("sources")).unwrap();
        let registered = root.join("alias/../sources");
        let canonical = registered.canonicalize().unwrap();
        let model = ProjectSnapshot::new(parse_dump(&format!(
            "PROJECT\t:app\t{}\nSOURCE\tmain\t{}\nEND\t:app\n",
            root.display(),
            registered.display()
        )));
        for path in [registered.join("Unsaved.kt"), canonical.join("Unsaved.kt")] {
            let (module, source) = model.owner_for(&path).unwrap();
            assert_eq!(module.module, ":app");
            assert_eq!(source.unwrap().name, "main");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn provided_artifact_prefixes_do_not_become_output_directories() {
        let model = ProjectSnapshot::new(parse_dump(
            "PROJECT\t:app\t/workspace\nSOURCE\tmain\t/workspace/src\n\
             OUTPUT\tmain\t/external/classes\nSCP\tmain\t/external/export/api.jar\nEND\t:app\n\
             PROJECT\t:lib\t/library\nPROVIDES\tmain\t/external/export/api.jar\nEND\t:lib\n",
        ));
        assert!(model.is_output_key("/external/classes/New.class"));
        assert!(!model.is_output_key("/external/classes-other/Unrelated.class"));
        assert!(!model.is_output_key("/external/export/api.jar/Invented.class"));
        assert!(model.has_dependency_key("/external/export/api.jar"));
        assert!(!model.has_dependency_key("/external/export/api.jar/Invented.class"));
        assert!(!model.has_dependency_key("/external/export/api-other.jar"));
    }

    #[test]
    fn selected_local_source_keys_are_frozen_without_existence_checks_or_gav_guessing() {
        let model = ProjectSnapshot::new(parse_dump(
            "PROJECT\t:app\t/workspace\nSOURCE\tmain\t/workspace/src\n\
             SCP\tmain\t/external/local.jar\nSCP\tmain\t/external/maven.jar\n\
             ARTIFACT\tmain\t/external/maven.jar\tMODULE\tvendor\tapi\t1\n\
             SCP\ttest\t/external/cross-known.jar\n\
             ARTIFACT\tmain\t/external/cross-known.jar\tMODULE\tvendor\tother\t1\nEND\t:app\n\
             PROJECT\t:unsupported\t/unsupported\nCP\t/external/unsupported.jar\nEND\t:unsupported\n",
        ));
        assert!(model.has_dependency_key("/external/local.jar"));
        assert!(model.has_dependency_key("/external/local-sources.jar"));
        assert!(model.has_dependency_key("/external/maven.jar"));
        assert!(!model.has_dependency_key("/external/maven-sources.jar"));
        assert!(!model.has_dependency_key("/external/cross-known-sources.jar"));
        assert!(!model.has_dependency_key("/external/unsupported-sources.jar"));
        assert!(!model.has_dependency_key("/external/local-other-sources.jar"));
        let replacement = ProjectSnapshot::new(parse_dump(
            "PROJECT\t:app\t/workspace\nSOURCE\tmain\t/workspace/src\nEND\t:app\n",
        ));
        assert!(!replacement.has_dependency_key("/external/local-sources.jar"));
        assert!(
            model.has_dependency_key("/external/local-sources.jar"),
            "old Arc view is immutable"
        );
    }

    #[test]
    fn snapshot_uses_actual_project_dirs_and_longest_registered_source_root() {
        let model = ProjectSnapshot::new(parse_dump(
            "\
PROJECT\t:\t/repo\nEND\t:\n\
PROJECT\t:app\t/repo/modules/editing\nSOURCESET\tmain\n\
SOURCE\tmain\t/repo/modules/editing/src/main/java\n\
SOURCE\tmain\t/repo/build/custom-generated\nEND\t:app\n\
PROJECT\t:tests\t/repo/modules/editing/tests\nSOURCESET\tintegration\n\
SOURCE\tintegration\t/repo/modules/editing/tests/custom\nEND\t:tests\n",
        ));
        for file in [
            "/repo/modules/editing/src/main/java/Main.java",
            "/repo/build/custom-generated/New.java",
        ] {
            let (module, source) = model.owner_for(Path::new(file)).unwrap();
            assert_eq!(module.module, ":app");
            assert_eq!(source.unwrap().name, "main");
        }
        assert_eq!(
            model
                .owner_for(Path::new("/repo/modules/editing/tests/custom/T.kt"))
                .unwrap()
                .0
                .module,
            ":tests"
        );
        assert!(model.owner_for(Path::new("/repository/Other.kt")).is_none());
    }

    #[test]
    fn source_set_visibility_preserves_order_and_actual_project_artifact_aliases() {
        let model = ProjectSnapshot::new(parse_dump(
            "PROJECT\t:app\t/repo/modules/editing\n\
SOURCE\tmain\t/repo/modules/editing/src/main/java\n\
OUTPUT\tmain\t/repo/app-classes\n\
SCP\tmain\t/repo/m2/api-2.jar\n\
ARTIFACT\tmain\t/repo/m2/api-2.jar\tMODULE\tvendor\tapi\t2\n\
SCP\tmain\t/repo/lib-custom.jar\n\
ARTIFACT\tmain\t/repo/lib-custom.jar\tPROJECT\t:\t:lib\n\
SCP\tmain\t/repo/libs/local.jar\n\
SOURCE\ttest\t/repo/modules/editing/src/test/java\n\
SCP\ttest\t/repo/app-classes\n\
SCP\ttest\t/repo/m2/api-1.jar\n\
ARTIFACT\ttest\t/repo/m2/api-1.jar\tMODULE\tvendor\tapi\t1\n\
INCOMPLETE\ttest\nSUNRESOLVED\ttest\tvendor:missing:1\tabsent\nEND\t:app\n\
PROJECT\t:lib\t/repo/modules/library\n\
SOURCE\tmain\t/repo/modules/library/src/main/java\n\
PROVIDES\tmain\t/repo/lib-custom.jar\nEND\t:lib\n",
        ));
        let visibilities = model.scope_visibilities(&["jdk:chosen".to_string()]);
        assert_eq!(
            visibilities[0].ordered_origins,
            vec![
                "project:gradle::|:app#main",
                "maven:vendor:api:2",
                "project:gradle::|:lib#main",
                model
                    .artifact_origin(Path::new("/repo/libs/local.jar"))
                    .unwrap(),
                "jdk:chosen",
            ]
        );
        assert_eq!(
            visibilities[1].ordered_origins,
            vec![
                "project:gradle::|:app#test",
                "project:gradle::|:app#main",
                "maven:vendor:api:1",
                "jdk:chosen",
            ]
        );
        assert_eq!(
            model
                .coordinates_for_file(Path::new("/repo/modules/editing/src/test/java/Use.java"))
                .unwrap(),
            vec![Coordinate::parse("vendor:api:1").unwrap()]
        );
        assert_eq!(
            model
                .local_jars_for_file(Path::new("/repo/modules/editing/src/main/java/Use.java"))
                .unwrap(),
            vec![PathBuf::from("/repo/libs/local.jar")]
        );
        assert!(model.modules[0].source_sets[1].classpath_incomplete);
        assert_eq!(
            model.modules[0].source_sets[1].unresolved,
            vec!["vendor:missing:1: absent"]
        );
        assert!(model
            .file_origin(Path::new("/repo/modules/editing/testData/Private.java"))
            .is_none());
    }

    #[test]
    fn artifact_identity_is_frozen_with_the_snapshot() {
        let root =
            std::env::temp_dir().join(format!("ktlsp_snapshot_artifact_{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let jar = root.join("local.jar");
        std::fs::write(&jar, b"first").unwrap();
        let model = ProjectSnapshot::new(parse_dump(&format!(
            "PROJECT\t:app\t{}\nSOURCESET\tmain\nSCP\tmain\t{}\nEND\t:app\n",
            root.display(),
            jar.display()
        )));
        let captured = model.artifact_origin(&jar).unwrap().to_string();
        std::fs::write(&jar, b"changed binary").unwrap();
        assert_ne!(captured, file_origin_id(&jar));
        assert_eq!(model.artifact_origin(&jar), Some(captured.as_str()));
        assert_eq!(
            model.scope_visibilities(&[])[0].ordered_origins[1],
            captured
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn published_model_reuses_actual_module_without_another_gradle_invocation() {
        let root = std::env::temp_dir().join(format!("ktlsp_shared_model_{}", std::process::id()));
        std::fs::create_dir_all(root.join("modules/editing")).unwrap();
        std::fs::write(root.join("settings.gradle.kts"), "include(\":app\")").unwrap();
        std::fs::write(
            root.join("modules/editing/build.gradle.kts"),
            "plugins { java }",
        )
        .unwrap();
        let modules = parse_dump(&format!(
            "PROJECT\t:app\t{}\nSOURCESET\tmain\nSOURCE\tmain\t{}\nEND\t:app\n",
            root.join("modules/editing").display(),
            root.join("build/custom-generated").display()
        ));
        let mtimes = build_file_mtimes(&root, &modules);
        let model = publish(&root, modules, mtimes.clone());
        for _ in 0..100 {
            assert!(Arc::ptr_eq(&model, &resolve_snapshot(&root).unwrap()));
            assert_eq!(
                resolve_module(&root, ":app").unwrap().project_dir,
                root.join("modules/editing")
            );
            assert_eq!(
                crate::daemon::module_path_for(&root, &root.join("build/custom-generated/New.kt")),
                Some(":app".to_string())
            );
        }
        std::fs::write(
            root.join("modules/editing/gradle.properties"),
            "changed=true",
        )
        .unwrap();
        assert_ne!(mtimes, build_file_mtimes(&root, &model.modules));
        let canonical = root.canonicalize().unwrap();
        assert!(Arc::ptr_eq(&model, &snapshot(&canonical).unwrap()));
        assert_eq!(
            model
                .owner_for(&canonical.join("modules/editing/src/main/kotlin/Unsaved.kt"))
                .unwrap()
                .0
                .module,
            ":app"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn forced_model_load_bypasses_unchanged_metadata_without_publishing() {
        let root = std::env::temp_dir().join(format!("ktlsp_forced_model_{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("build.gradle"), "plugins { id 'java' }").unwrap();
        let modules = |version| {
            parse_dump(&format!(
            "PROJECT\t:app\t{}\nSOURCE\tmain\t{}\nSCP\tmain\t{}\nARTIFACT\tmain\t{}\tMODULE\tvendor\tapi\t{version}\nEND\t:app\n",
            root.join("modules/editing").display(), root.join("custom-generated").display(),
            root.join("api.jar").display(), root.join("api.jar").display()))
        };
        let original = modules("1");
        let old = publish(&root, original.clone(), build_file_mtimes(&root, &original));
        let reused = load_snapshot_with(&root, false, || {
            panic!("unchanged metadata must reuse the model")
        })
        .unwrap();
        assert!(Arc::ptr_eq(&old, &reused));
        let calls = std::cell::Cell::new(0);
        let prepared = load_snapshot_with(&root, true, || {
            calls.set(calls.get() + 1);
            Ok(modules("2"))
        })
        .unwrap();
        assert_eq!(calls.get(), 1);
        assert!(
            Arc::ptr_eq(&old, &snapshot(&root).unwrap()),
            "preparation leaked into interactive queries"
        );
        assert_eq!(prepared.coordinates()[0].label(), "vendor:api:2");
        assert_eq!(
            prepared
                .owner_for(&root.join("custom-generated/New.kt"))
                .unwrap()
                .0
                .module,
            ":app"
        );
        publish_snapshot(&root, prepared.clone());
        assert!(Arc::ptr_eq(
            &prepared,
            &snapshot(&root.canonicalize().unwrap()).unwrap()
        ));
        std::fs::remove_dir_all(root).unwrap();
    }
}
