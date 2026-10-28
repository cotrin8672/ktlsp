//! Resolve a Gradle project's per-module `compileClasspath` for the Kotlin compile-daemon backend.
//!
//! The compiler sidecar needs the full classpath; Gradle is the only thing that knows it. We run a
//! one-shot init script (embedded below, also committed at `scripts/classpath-dump.init.gradle.kts`)
//! that prints a line protocol, parse it, and cache the result keyed by the build files' mtimes —
//! the classpath is stable across `.kt` edits, so this runs rarely.
//!
//! Pure parsing (`parse_dump`) is unit-tested; `resolve` does the gradle process IO.

use std::collections::{BTreeMap, BTreeSet};
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
const MODEL_VERSION: u32 = 15;

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

pub(crate) struct JavaApiOptionSlots {
    pub destination: usize,
    pub headers: usize,
    pub generated: usize,
    pub classpath: usize,
    pub explicit_processor_argument: bool,
}

pub(crate) fn same_java_api_path(a: &Path, b: &Path) -> bool {
    let normalize = |path: &Path| {
        let key = crate::paths::file_key(path);
        if cfg!(windows) {
            key.to_lowercase()
        } else {
            key
        }
    };
    normalize(a) == normalize(b)
}

pub(crate) fn java_api_path_text(path: &Path) -> anyhow::Result<String> {
    let text = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("java-path-encoding"))?;
    anyhow::ensure!(
        path.is_absolute() && !text.chars().any(|c| matches!(c, '\0' | '\r' | '\n' | '\t')),
        "java-path-unhandled"
    );
    Ok(text.to_owned())
}

pub(crate) fn joined_java_api_classpath(paths: &[PathBuf]) -> anyhow::Result<String> {
    for path in paths {
        let text = java_api_path_text(path)?;
        anyhow::ensure!(
            !text.contains(if cfg!(windows) { ';' } else { ':' }),
            "java-classpath-separator-in-path"
        );
    }
    std::env::join_paths(paths)?
        .into_string()
        .map_err(|_| anyhow::anyhow!("java-classpath-encoding"))
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
        (matches!(
            (recipe.adapter.as_str(), recipe.language_version.as_str()),
            (
                "gradle-7.4.2-current-jdk17-javac-api-v1"
                    | "gradle-7.4.2-current-jdk17-release8-utf8-javac-api-v1"
                    | "gradle-7.5-current-jdk17-utf8-javac-api-v1"
                    | "gradle-8.10.2-current-jdk17-release17-javac-api-with-processors-v1"
                    | "gradle-9.6.1-current-jdk17-javac-api-v1",
                "17"
            ) | ("gradle-9.6.1-current-jdk21-utf8-javac-api-v1", "21")
                | (
                    "gradle-9.6.1-current-jdk21-target17-default-utf8-javac-api-v1",
                    "21"
                )
        ) && recipe.compiler_type
            == "org.gradle.jvm.toolchain.internal.DefaultToolchainJavaCompiler"
            && (recipe.language_version != "21" || recipe.effective_charset == "UTF-8")
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

    /// Validate the captured Gradle arguments once for replay and source namespace authority.
    pub(crate) fn java_api_option_slots(
        &self,
        recipe: &JavaCompilerRecipe,
    ) -> anyhow::Result<JavaApiOptionSlots> {
        let task = self;
        let options = &recipe.api_options;
        let (levels, decoration, raw_proc): (&[&str], &[&str], bool) = match recipe.adapter.as_str() {
            "gradle-7.4.2-current-jdk17-javac-api-v1" => {
                (&["-source", "17", "-target", "17"], &[], true)
            }
            "gradle-7.4.2-current-jdk17-release8-utf8-javac-api-v1" => {
                (&["--release", "8"], &[], false)
            }
            "gradle-7.5-current-jdk17-utf8-javac-api-v1" => match task.arguments.as_slice() {
                // Captured stock producer: warnings enabled and no user compiler arguments.
                [] => (&["-source", "1.8", "-target", "1.8"], &["-encoding", "UTF-8"], false),
                [argument] if argument == "-proc:none" => (
                    &["-source", "1.8", "-target", "1.8"],
                    &["-nowarn", "-encoding", "UTF-8"], true,
                ),
                _ => anyhow::bail!("java-api-raw-arguments-unhandled"),
            },
            "gradle-9.6.1-current-jdk17-javac-api-v1"
            | "gradle-9.6.1-current-jdk21-target17-default-utf8-javac-api-v1" => {
                (&["-source", "17", "-target", "17"], &[], false)
            }
            "gradle-9.6.1-current-jdk21-utf8-javac-api-v1" => (
                &["-source", "21", "-target", "21"],
                &["-encoding", "UTF-8"],
                false,
            ),
            _ => anyhow::bail!("java-api-options-adapter-unhandled"),
        };
        let destination = levels.len() + 1;
        let header = levels.len() + 2 + decoration.len();
        let classpath = header + 10;
        anyhow::ensure!(
            options.len() == header + 11 + usize::from(raw_proc),
            "java-api-options-shape-unhandled"
        );
        for (index, expected) in levels.iter().enumerate() {
            anyhow::ensure!(
                options[index] == *expected,
                "java-api-options-unhandled-at-{index}"
            );
        }
        anyhow::ensure!(
            options[levels.len()] == "-d",
            "java-api-destination-option-differs"
        );
        for (index, expected) in decoration.iter().enumerate() {
            anyhow::ensure!(
                options[levels.len() + 2 + index] == *expected,
                "java-api-encoding-warning-options-differ"
            );
        }
        for (offset, expected) in [
            (0, "-h"),
            (2, "-g"),
            (3, "-sourcepath"),
            (4, ""),
            (5, "-proc:none"),
            (6, "-s"),
            (8, "-XDuseUnsharedTable=true"),
            (9, "-classpath"),
        ] {
            anyhow::ensure!(
                options[header + offset] == expected,
                "java-api-options-unhandled-at-{}",
                header + offset
            );
        }
        anyhow::ensure!(
            !raw_proc || options[classpath + 1] == "-proc:none",
            "java-api-raw-processor-argument-differs"
        );
        anyhow::ensure!(
            task.output_roots.len() == 1
                && same_java_api_path(Path::new(&options[destination]), &task.output_roots[0]),
            "java-output-role-unknown"
        );
        for position in [header + 1, header + 7] {
            anyhow::ensure!(
                task.declared_output_roots
                    .iter()
                    .any(|root| same_java_api_path(root, Path::new(&options[position]))),
                "java-secondary-output-role-unknown"
            );
        }
        anyhow::ensure!(
            options[classpath] == joined_java_api_classpath(&task.classpath)?,
            "java-captured-classpath-differs"
        );
        Ok(JavaApiOptionSlots {
            destination,
            headers: header + 1,
            generated: header + 7,
            classpath,
            explicit_processor_argument: raw_proc,
        })
    }

    pub(crate) fn annotation_processing_disabled(&self) -> bool {
        if self.arguments.len() == 1 && self.arguments[0] == "-proc:none" {
            return true;
        }
        if !self.arguments.is_empty() || !self.annotation_processor_path.is_empty() {
            return false;
        }
        self.java_compiler_recipe().is_some_and(|recipe| {
            self.java_api_option_slots(&recipe)
                .is_ok_and(|slots| !slots.explicit_processor_argument)
        })
    }


}

/// Explicit Gradle runtime inputs; absence in a legacy model is unknown authority.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeClassInputs {
    pub adapter: String,
    pub classpath: Vec<PathBuf>,
    pub class_outputs: Vec<PathBuf>,
    pub complete: bool,
    #[serde(default)]
    pub problem: Option<String>,
}

impl RuntimeClassInputs {
    pub(crate) fn usable(&self) -> bool {
        self.adapter == "gradle-runtime-class-inputs-v1"
            && self.complete && self.problem.is_none()
            && self.classpath.len() <= 16_384 && self.class_outputs.len() <= 1024
            && self.classpath.iter().chain(&self.class_outputs).all(|path|
                path.is_absolute() && !path.components().any(|part|
                    matches!(part, std::path::Component::ParentDir)))
            && self.classpath.iter().map(|path|crate::paths::file_key(path))
                .collect::<std::collections::BTreeSet<_>>().len() == self.classpath.len()
            && self.class_outputs.iter().map(|path|crate::paths::file_key(path))
                .collect::<std::collections::BTreeSet<_>>().len() == self.class_outputs.len()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSetClasspath {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<crate::resource_model::ResourceClaims>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeClassInputs>,
    pub name: String,
    pub source_roots: Vec<PathBuf>,
    pub output_roots: Vec<PathBuf>,
    pub provided_artifacts: Vec<PathBuf>,
    pub entries: Vec<PathBuf>,
    pub artifacts: Vec<ResolvedArtifact>,
    /// Exact Gradle source selections; absent until the background source query completes.
    #[serde(default)]
    pub source_artifacts: Option<Vec<ResolvedArtifact>>,
    #[serde(default)]
    pub source_artifacts_incomplete: bool,
    #[serde(default)]
    pub source_poms: Option<Vec<ResolvedArtifact>>,
    #[serde(default)]
    pub source_poms_incomplete: bool,
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SdkApiSelection {
    pub home: PathBuf,
    /// Navigation sources do not establish the API surface of a lower --release.
    pub source_zip: Option<PathBuf>,
    pub release: Option<u16>,
    pub archive: PathBuf,
    pub archive_sha256: String,
    pub archive_revision: String,
    pub entries: Arc<BTreeMap<String, Option<String>>>,
}

impl SdkApiSelection {
    pub(crate) fn identity(&self) -> String {
        format!(
            "jdk-api:{}#release={}#{}",
            path_key(&self.home),
            self.release
                .map_or_else(|| "full".into(), |release| release.to_string()),
            self.archive_sha256
        )
    }

    pub(crate) fn origin(&self) -> String {
        match self.release {
            Some(_) => self.identity(),
            None => jdk_origin_id(self.source_zip.as_ref().expect("full SDK source selection")),
        }
    }
}

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
    sdk_selections: BTreeMap<ProjectScope, BTreeMap<String, SdkApiSelection>>,
    sdk_caller_views: Vec<ktcore::index::CallerScopeVisibility>,
    compiler_source_views: Vec<ktcore::index::CallerScopeVisibility>,
}

pub(crate) struct ExtensionProjectContext<'a> {
    pub module: &'a str,
    pub scope: ProjectScope,
    pub source: &'a SourceSetClasspath,
    pub sdk: Option<&'a SdkApiSelection>,
    pub classpath_complete: bool,
}

impl ProjectSnapshot {
    /// Borrow the accepted selection; extensions do not rediscover roots or choose an SDK.
    pub(crate) fn extension_project_context(&self, file: &Path) -> Option<ExtensionProjectContext<'_>> {
        let Some((module, Some(source))) = self.owner_for(file)
            .filter(|_| crate::language::SourceLanguage::for_project_path(file).is_some()) else {
            let ktcore::knowledge::Knowledge::Found(owner) = self.resource_owner_context(file) else { return None; };
            let source = self.source_set_for_scope(&owner.scope)?;
            let module = self.modules.iter().find(|module|module.source_sets.iter().any(|candidate|
                std::ptr::eq(candidate,source)))?;
            return Some(ExtensionProjectContext {module:&module.module,scope:owner.scope.clone(),source,
                sdk:self.sdk_api_for_scope(&owner.scope),
                classpath_complete:!source.classpath_incomplete && source.unresolved.is_empty() && module.unresolved.is_empty()});
        };
        Some(ExtensionProjectContext {
            module: &module.module,
            scope: project_scope(module, source),
            source,
            sdk: self.sdk_api_for_file(file),
            classpath_complete: !source.classpath_incomplete
                && source.unresolved.is_empty() && module.unresolved.is_empty(),
        })
    }

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
            sdk_selections: BTreeMap::new(),
            sdk_caller_views: Vec::new(),
            compiler_source_views: Vec::new(),
        };
        for entry in model
            .modules
            .iter()
            .flat_map(|m| &m.source_sets)
            .flat_map(|s| {
                s.entries.iter().chain(
                    s.source_artifacts
                        .iter()
                        .flatten()
                        .chain(s.source_poms.iter().flatten())
                        .map(|artifact| &artifact.path),
                )
            })
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
        let mut sdk_features = BTreeMap::new();
        let mut sdk_digests = BTreeMap::new();
        for (visibility, source) in model
            .visibilities
            .iter_mut()
            .zip(model.modules.iter().flat_map(|module| &module.source_sets))
        {
            if let Some(source_zip) = source_set_jdk_source(source, &mut sdk_features) {
                visibility.ordered_origins.push(jdk_origin_id(&source_zip));
            }
            let selections = model
                .sdk_selections
                .entry(visibility.scope.clone())
                .or_default();
            for (language, suffixes) in [
                (CompilerLanguage::Java, &["java"][..]),
                (CompilerLanguage::Kotlin, &["kt"][..]),
            ] {
                if let Some(selection) =
                    source_set_sdk_api(source, language, &mut sdk_features, &mut sdk_digests)
                {
                    for suffix in suffixes {
                        selections.insert((*suffix).into(), selection.clone());
                    }
                }
            }
        }
        for selection in model.sdk_selections.values().flat_map(BTreeMap::values) {
            for input in [&selection.archive, &selection.home.join("release")] {
                model
                    .file_origins
                    .insert(path_key(input), file_origin_id(input));
            }
        }
        for source_zip in model.selected_jdk_sources() {
            let home = crate::deps::jdk_home_for_source_archive(&source_zip)
                .expect("selected SDK source archive home");
            for input in [
                &source_zip,
                &home.join("release"),
                &home.join("jmods/java.base.jmod"),
                &home.join("jre/lib/rt.jar"),
            ] {
                model
                    .file_origins
                    .insert(path_key(input), file_origin_id(input));
            }
        }
        model.sdk_caller_views = model.build_caller_scope_visibilities(true);
        model.compiler_source_views = model.build_caller_scope_visibilities(false);
        for view in &mut model.compiler_source_views {
            // New inputs require future-root policy, not another task's exact selected file.
            view.source_files.clear();
        }
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
        self.plain_java_task_for_editor(key, &BTreeSet::new())
    }

    pub(crate) fn plain_java_task_for_editor(
        &self,
        key: &str,
        editor_sources: &BTreeSet<String>,
    ) -> Option<&CompilerTask> {
        let (module, source) = self.owner_for_key(key)?;
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
        let scope = project_scope(module, source);
        let has_new_java = editor_sources.iter().any(|file| {
            let mut views = self
                .compiler_source_views
                .iter()
                .filter(|view| view.suffix == "java" && view.matches_file(file));
            views
                .next()
                .is_some_and(|view| view.visibility.scope == scope)
                && views.next().is_none()
        });
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
            || (task.source_files.is_empty() && !has_new_java)
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

    /// Positive metadata from an existing output; this never selects a task to execute.
    pub(crate) fn java_output_task(&self, key: &str) -> Option<&CompilerTask> {
        let (module, source) = self.owner_for_key(key)?;
        let source = source?;
        if source.compiler_metadata_incomplete || source.classpath_incomplete || !source.unresolved.is_empty() {
            return None;
        }
        let mut selected = source.compiler_tasks.iter().filter(|task| {
            task.language == CompilerLanguage::Java
                && task.source_files.iter().any(|path| crate::paths::file_key(path) == key)
        });
        let task = selected.next()?;
        if selected.next().is_some() || !task.complete || task.problem.is_some() || task.output_roots.len() != 1 {
            return None;
        }
        let root = &task.output_roots[0];
        if !root.is_absolute() || !task.declared_output_roots.iter().any(|path| path_key(path) == path_key(root)) {
            return None;
        }
        let owners = &self.output_owners.get(&path_key(root))?.source_sets;
        let &(mi, si) = owners.first()?;
        // Gradle may expose one output both as classes and a provided artifact.
        // Repeated declarations of the same owner do not make ownership ambiguous.
        if owners.iter().any(|owner| *owner != (mi, si)) {
            return None;
        }
        (std::ptr::eq(&self.modules[mi], module)
            && std::ptr::eq(&self.modules[mi].source_sets[si], source)).then_some(task)
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

    pub(crate) fn source_set_for_scope(&self, scope: &ProjectScope) -> Option<&SourceSetClasspath> {
        let mut found=None;
        for module in &self.modules {
            for source in &module.source_sets {
                if &project_scope(module,source)!=scope { continue; }
                if found.is_some() { return None; }
                found=Some(source);
            }
        }
        found
    }

    /// External editor documents require one modeled source owner, including current task inputs.
    pub(crate) fn unique_source_origin(&self, file: &Path) -> Option<FileOrigin> {
        let selected = self.file_origin(file)?;
        let mut found = None;
        for module in &self.modules {
            for source in &module.source_sets {
                if !source.source_roots.iter().any(|root|file.starts_with(root))
                    && !source.compiler_tasks.iter().any(|task|task.source_files.iter().any(|input|path_key(input)==path_key(file))) { continue; }
                let scope = project_scope(module, source);
                if found.as_ref().is_some_and(|previous|previous!=&scope) { return None; }
                found = Some(scope);
            }
        }
        (found.as_ref()==selected.project_scope.as_ref()).then_some(selected)
    }

    /// Model-only ownership check. A resource URI is never an arbitrary filesystem grant.
    pub(crate) fn resource_owner_context(&self, file: &Path) -> ktcore::extensions::Fact<crate::resource_model::OwnedResourceContext> {
        use ktcore::knowledge::Knowledge;
        let mut found = None;
        for module in &self.modules {
            for source in &module.source_sets {
                let Some(claims) = source.resources.as_ref().filter(|claims| claims.valid()) else { continue; };
                let mut matches = Vec::new();
                for selected in &claims.files {
                    if crate::paths::file_key(&selected.path)==crate::paths::file_key(file) {
                        matches.push(("source",selected.relative_path.clone()));
                    }
                }
                for tree in &claims.trees {
                    let Ok(relative) = file.strip_prefix(&tree.root) else { continue; };
                    let relative = crate::paths::file_key(relative);
                    if claims.complete && claims.selects(tree, &relative) == Some(true) { matches.push(("source",relative)); }
                }
                for root in &claims.output_roots {
                    if let Ok(relative) = file.strip_prefix(root) {
                        let relative = crate::paths::file_key(relative);
                        if crate::resource_model::safe_relative(&relative) { matches.push(("output",relative)); }
                    }
                }
                for input in &claims.configuration_inputs {
                    if crate::paths::file_key(input)==crate::paths::file_key(file) {
                        if let Some(name)=file.file_name().and_then(|name|name.to_str()) { matches.push(("configuration",name.into())); }
                    }
                }
                if matches.is_empty() { continue; }
                matches.sort();matches.dedup();
                if matches.len()!=1 { return Knowledge::Unknown(vec![ktcore::extensions::IncompleteReason::IncompleteScope]); }
                let scope=project_scope(module,source);
                let context=crate::resource_model::OwnedResourceContext {
                    origin:FileOrigin { origin_id:project_origin_id(&scope),representation:SourceRepresentation::Source,project_scope:Some(scope.clone()) },
                    scope,path:file.into(),relative_path:matches[0].1.clone(),association:matches[0].0,
                };
                if found.as_ref().is_some_and(|previous: &crate::resource_model::OwnedResourceContext| previous.scope!=context.scope) {
                    return Knowledge::Unknown(vec![ktcore::extensions::IncompleteReason::IncompleteScope]);
                }
                found=Some(context);
            }
        }
        found.map(Knowledge::Found).unwrap_or_else(||Knowledge::Unknown(vec![ktcore::extensions::IncompleteReason::MissingFacts]))
    }

    /// A raw resource has no Java/Kotlin filename suffix from which to select an SDK.
    /// Require every configured language in its unique scope to agree exactly.
    pub(crate) fn sdk_api_for_scope(&self, scope: &ProjectScope) -> Option<&SdkApiSelection> {
        let source = self.source_set_for_scope(scope)?;
        let selected = self.sdk_selections.get(scope)?;
        let mut common = None;
        for (language, suffix) in [(CompilerLanguage::Java, "java"), (CompilerLanguage::Kotlin, "kt")] {
            if !source.compiler_tasks.iter().any(|task| task.language == language) { continue; }
            let sdk = selected.get(suffix)?;
            if common.is_some_and(|known| known != sdk) { return None; }
            common = Some(sdk);
        }
        common
    }


    /// The ordinary model cache already records present and negative configuration
    /// stamps. Reuse them off-lock before arming resource facts from that selection.
    pub(crate) fn resource_configuration_current(&self,inputs:&[PathBuf])->bool {
        if inputs.is_empty() || self.mtimes.is_empty() {return false;}
        inputs.iter().all(|path| {
            let Some(recorded)=self.mtimes.get(&path_key(path)) else {return false;};
            match std::fs::symlink_metadata(path) {
                Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink()=>metadata.modified().ok()
                    .and_then(|time|time.duration_since(std::time::UNIX_EPOCH).ok())
                    .is_some_and(|time|time.as_nanos()==*recorded),
                Err(error) if error.kind()==std::io::ErrorKind::NotFound=>*recorded==0,
                _=>false,
            }
        })
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
            visibility
                .ordered_origins
                .retain(|origin| !is_sdk_origin(origin));
            for origin in global_origins {
                // Model-owned SDKs are scoped above; a host SDK must never leak into another target.
                if is_sdk_origin(origin) {
                    continue;
                }
                if !visibility.ordered_origins.contains(origin) {
                    visibility.ordered_origins.push(origin.clone());
                }
            }
        }
        visibilities
    }

    pub(crate) fn selected_jdk_sources(&self) -> Vec<PathBuf> {
        self.sdk_selections
            .values()
            .flat_map(BTreeMap::values)
            .filter(|selection| selection.release.is_none())
            .filter_map(|selection| selection.source_zip.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub(crate) fn selected_jdk_source_for_file(&self, file: &Path) -> Option<PathBuf> {
        let selection = self.sdk_api_for_file(file)?;
        // API8 declarations come from selected signatures. SDK17 sources cannot seed API8 facts.
        selection
            .release
            .is_none()
            .then(|| selection.source_zip.clone())
            .flatten()
    }

    pub(crate) fn sdk_api_for_file(&self, file: &Path) -> Option<&SdkApiSelection> {
        let (module, source) = self.owner_for(file)?;
        let scope = project_scope(module, source?);
        let key = crate::paths::file_key(file);
        let mut views = self
            .sdk_caller_views
            .iter()
            .filter(|view| view.visibility.scope == scope && view.matches_file(&key));
        let selected = views.next()?;
        if views.next().is_some() {
            return None;
        }
        self.sdk_selections.get(&scope)?.get(&selected.suffix)
    }

    pub(crate) fn selected_release_apis(&self) -> Vec<SdkApiSelection> {
        let mut seen = std::collections::BTreeSet::new();
        self.sdk_selections
            .values()
            .flat_map(BTreeMap::values)
            .filter(|selection| selection.release.is_some() && seen.insert(selection.identity()))
            .cloned()
            .collect()
    }

    pub(crate) fn caller_scope_visibilities(
        &self,
        global_origins: &[String],
    ) -> Vec<ktcore::index::CallerScopeVisibility> {
        self.sdk_caller_views
            .iter()
            .cloned()
            .map(|mut view| {
                for origin in global_origins
                    .iter()
                    .filter(|origin| !is_sdk_origin(origin))
                {
                    if !view.visibility.ordered_origins.contains(origin) {
                        view.visibility.ordered_origins.push(origin.clone());
                    }
                }
                view
            })
            .collect()
    }

    /// Reuse stock pattern and producer-overlap checks without requiring SDK source indexing.
    pub(crate) fn compiler_source_views(&self) -> &[ktcore::index::CallerScopeVisibility] {
        &self.compiler_source_views
    }

    fn build_caller_scope_visibilities(
        &self,
        require_sdk: bool,
    ) -> Vec<ktcore::index::CallerScopeVisibility> {
        let mut result = Vec::new();
        // Producer claims only veto a view. SDK or pattern-policy failure cannot erase them.
        let mut claimed = Vec::new();
        for (visibility, source) in self
            .scope_visibilities(&[])
            .into_iter()
            .zip(self.modules.iter().flat_map(|module| &module.source_sets))
        {
            for (language, suffix) in [
                (CompilerLanguage::Java, "java"),
                (CompilerLanguage::Kotlin, "kt"),
            ] {
                for task in source.compiler_tasks.iter().filter(|task| {
                    task.language == language
                        && (language == CompilerLanguage::Java
                            || source.kotlin_compile_task.as_deref() == Some(task.task.as_str()))
                }) {
                    let files = task
                        .source_files
                        .iter()
                        .filter(|file| {
                            file.extension().and_then(|value| value.to_str()) == Some(suffix)
                        })
                        .map(|file| crate::paths::file_key(file))
                        .collect::<std::collections::BTreeSet<_>>();
                    let roots = sdk_producer_roots(task).or_else(|| {
                        sdk_source_policy(task, source, suffix)
                            .map(|policy| (policy.future_roots, false))
                    });
                    let inputs_proved = task.complete
                        && task.problem.is_none()
                        && roots.as_ref().is_some_and(|(_, complete)| *complete);
                    claimed.push((
                        visibility.scope.clone(),
                        suffix,
                        files,
                        roots,
                        inputs_proved,
                    ));
                }
                let selection = self
                    .sdk_selections
                    .get(&visibility.scope)
                    .and_then(|views| views.get(suffix));
                if require_sdk && selection.is_none() {
                    continue;
                }
                let task = if require_sdk {
                    producing_task(source, &language)
                } else {
                    sdk_producing_task(source, &language)
                };
                let Some(task) = task else {
                    continue;
                };
                let Some(policy) = sdk_source_policy(task, source, suffix) else {
                    continue;
                };
                let mut caller = visibility.clone();
                if let Some(selection) = selection {
                    caller.ordered_origins.push(selection.origin());
                }
                result.push(ktcore::index::CallerScopeVisibility {
                    visibility: caller,
                    suffix: suffix.into(),
                    source_files: task
                        .source_files
                        .iter()
                        .filter(|file| {
                            file.extension().and_then(|value| value.to_str()) == Some(suffix)
                                && policy
                                    .authored_roots
                                    .iter()
                                    .any(|root| path_under_key(&path_key(file), root))
                        })
                        .map(|file| crate::paths::file_key(file))
                        .collect(),
                    authored_roots: policy.future_roots,
                    generated_roots: policy.generated_roots,
                    excluded_segments: policy.excluded_segments,
                });
            }
        }
        // Equal or nested producing roots in different scopes cannot borrow the first owner.
        for view in &mut result {
            for (scope, suffix, files, roots, inputs_complete) in &claimed {
                if *suffix != view.suffix || *scope == view.visibility.scope {
                    continue;
                }
                let (roots, roots_complete) = roots
                    .as_ref()
                    .map(|(roots, complete)| (roots.as_slice(), *complete))
                    .unwrap_or((&[][..], false));
                if !roots_complete && scope.module == view.visibility.scope.module {
                    // Current exact inputs cannot prove absence of a future local producer.
                    // Do not turn an unresolved source-set capture into a global SDK stall.
                    view.authored_roots.clear();
                    if !inputs_complete {
                        view.source_files.clear();
                    }
                }
                for file in &view.source_files {
                    if files.contains(file) || roots.iter().any(|root| path_under_key(file, root)) {
                        view.generated_roots.push(file.clone());
                    }
                }
                for file in files {
                    if view
                        .authored_roots
                        .iter()
                        .any(|root| path_under_key(file, root))
                    {
                        view.generated_roots.push(file.clone());
                    }
                }
                let mut unresolved_roots = Vec::new();
                for own_root in &view.authored_roots {
                    if !roots_complete && files.iter().any(|file| path_under_key(file, own_root)) {
                        unresolved_roots.push(own_root.clone());
                    }
                    for other_root in roots {
                        if own_root == other_root || path_under_key(own_root, other_root) {
                            view.generated_roots.push(own_root.clone());
                        } else if path_under_key(other_root, own_root) {
                            view.generated_roots.push(other_root.clone());
                        }
                    }
                }
                view.authored_roots
                    .retain(|root| !unresolved_roots.contains(root));
            }
            view.generated_roots.sort();
            view.generated_roots.dedup();
        }
        result
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

    pub(crate) fn coordinate_sources(
        &self,
        coord: &Coordinate,
    ) -> anyhow::Result<Option<Vec<(PathBuf, String)>>> {
        self.coordinate_source_artifacts(coord, false)
    }

    pub(crate) fn coordinate_source_poms(
        &self,
        coord: &Coordinate,
    ) -> anyhow::Result<Option<Vec<(PathBuf, String)>>> {
        self.coordinate_source_artifacts(coord, true)
    }

    fn coordinate_source_artifacts(
        &self,
        coord: &Coordinate,
        poms: bool,
    ) -> anyhow::Result<Option<Vec<(PathBuf, String)>>> {
        let selected = |component: &ComponentId| {
            matches!(component, ComponentId::Module { group, name, version }
                if group == &coord.group && name == &coord.artifact && version == &coord.version)
        };
        let mut queried = false;
        let mut paths = BTreeMap::new();
        for source in self.modules.iter().flat_map(|module| &module.source_sets) {
            if !source
                .artifacts
                .iter()
                .any(|artifact| selected(&artifact.component))
                && !(poms
                    && source
                        .source_poms
                        .iter()
                        .flatten()
                        .any(|artifact| selected(&artifact.component)))
            {
                continue;
            }
            let (artifacts, incomplete) = if poms {
                (&source.source_poms, source.source_poms_incomplete)
            } else {
                (&source.source_artifacts, source.source_artifacts_incomplete)
            };
            let Some(artifacts) = artifacts else {
                anyhow::ensure!(!incomplete, "Gradle source metadata records are invalid");
                return Ok(None);
            };
            queried = true;
            let selected_artifacts = artifacts
                .iter()
                .filter(|artifact| selected(&artifact.component))
                .collect::<Vec<_>>();
            anyhow::ensure!(
                !incomplete || !selected_artifacts.is_empty(),
                "Gradle source metadata selection is incomplete for {}",
                coord.label()
            );
            for artifact in selected_artifacts {
                let origin = self.artifact_origin(&artifact.path).ok_or_else(|| {
                    anyhow::anyhow!("Gradle source artifact identity is unavailable")
                })?;
                paths.insert(artifact.path.clone(), origin.to_string());
            }
        }
        // Different repository/variant selections cannot share one coordinate's source facts.
        anyhow::ensure!(
            paths.len() <= 1,
            "ambiguous Gradle sources for {}",
            coord.label()
        );
        Ok(queried.then(|| paths.into_iter().collect()))
    }

    pub(crate) fn source_enrichment_needed(&self) -> bool {
        self.modules
            .iter()
            .flat_map(|module| &module.source_sets)
            .any(|source| {
                source.source_artifacts.is_none()
                    || source.source_artifacts_incomplete
                    || source.source_poms.is_none()
                    || source.source_poms_incomplete
            })
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
            .chain(
                self.sdk_selections
                    .values()
                    .flat_map(BTreeMap::values)
                    .map(SdkApiSelection::origin),
            )
            .chain(
                self.selected_jdk_sources()
                    .iter()
                    .map(|source| jdk_origin_id(source)),
            )
            .collect()
    }

    pub fn artifact_revisions(&self) -> BTreeMap<String, std::collections::BTreeSet<String>> {
        let mut revisions: BTreeMap<String, std::collections::BTreeSet<String>> = BTreeMap::new();
        for source in self.modules.iter().flat_map(|module| &module.source_sets) {
            let pom_revisions = source
                .source_poms
                .iter()
                .flatten()
                .filter_map(|artifact| self.file_origins.get(&path_key(&artifact.path)).cloned())
                .collect::<std::collections::BTreeSet<_>>();
            for artifact in source
                .artifacts
                .iter()
                .chain(source.source_artifacts.iter().flatten())
                .chain(source.source_poms.iter().flatten())
            {
                if !self.file_origins.contains_key(&path_key(&artifact.path)) {
                    continue;
                }
                for origin in self.origins_for_entry(&artifact.path, Some(&artifact.component)) {
                    if let Some(revision) = self.file_origins.get(&path_key(&artifact.path)) {
                        let versions = revisions.entry(origin).or_default();
                        versions.insert(revision.clone());
                        // A declared parent POM can change a child's source decoding policy.
                        if matches!(artifact.component, ComponentId::Module { .. }) {
                            versions.extend(pom_revisions.iter().cloned());
                        }
                    }
                }
            }
        }
        for source_zip in self.selected_jdk_sources() {
            let home = crate::deps::jdk_home_for_source_archive(&source_zip)
                .expect("selected SDK source archive home");
            let inputs = [
                &source_zip,
                &home.join("release"),
                &home.join("jmods/java.base.jmod"),
                &home.join("jre/lib/rt.jar"),
            ];
            revisions
                .entry(jdk_origin_id(&source_zip))
                .or_default()
                .extend(
                    inputs
                        .into_iter()
                        .filter_map(|input| self.file_origins.get(&path_key(input)).cloned()),
                );
        }
        for selection in self.sdk_selections.values().flat_map(BTreeMap::values) {
            let inputs = revisions.entry(selection.origin()).or_default();
            inputs.insert(selection.identity());
            for input in [&selection.archive, &selection.home.join("release")] {
                if let Some(revision) = self.file_origins.get(&path_key(input)) {
                    inputs.insert(revision.clone());
                }
            }
        }
        revisions
    }

    pub(crate) fn binary_artifact_revisions(
        &self,
    ) -> BTreeMap<String, std::collections::BTreeSet<String>> {
        let mut revisions: BTreeMap<String, std::collections::BTreeSet<String>> = BTreeMap::new();
        for artifact in self
            .modules
            .iter()
            .flat_map(|module| &module.source_sets)
            .flat_map(|source| &source.artifacts)
        {
            if let Some(revision) = self.artifact_origin(&artifact.path) {
                for origin in self.origins_for_entry(&artifact.path, Some(&artifact.component)) {
                    revisions
                        .entry(origin)
                        .or_default()
                        .insert(revision.to_string());
                }
            }
        }
        revisions
    }
}

pub(crate) fn sdk_feature_version(release: &str) -> Option<u16> {
    let mut versions = release
        .lines()
        .filter_map(|line| line.trim().strip_prefix("JAVA_VERSION="));
    let value = versions.next()?.strip_prefix('"')?.strip_suffix('"')?;
    if versions.next().is_some()
        || value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
    {
        return None;
    }
    if value.ends_with(['.', '_', '+', '-']) || value.contains("..") {
        return None;
    }
    if value.split(['+', '-']).any(str::is_empty)
        || value
            .split(['+', '-'])
            .next()?
            .split(['.', '_'])
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    let mut numbers = value.split(['.', '_', '+', '-']);
    let first = numbers.next()?.parse::<u16>().ok()?;
    let feature = if first == 1 {
        let legacy = numbers.next()?.parse::<u16>().ok()?;
        if legacy != 8 {
            return None;
        }
        legacy
    } else {
        first
    };
    (feature >= 8).then_some(feature)
}

// Kotlin's selected compiler accepts the legacy spelling only for Java 8.
pub(crate) fn kotlin_jdk_release_feature(value: &str) -> Option<u16> {
    if value == "1.8" {
        Some(8)
    } else {
        value.parse().ok()
    }
}

fn source_set_jdk_source(
    source: &SourceSetClasspath,
    features: &mut BTreeMap<String, Option<u16>>,
) -> Option<PathBuf> {
    crate::deps::src_zip_for_home(source_set_jdk_home(source, features)?)
}

fn source_set_jdk_home(
    source: &SourceSetClasspath,
    features: &mut BTreeMap<String, Option<u16>>,
) -> Option<PathBuf> {
    if source.compiler_metadata_incomplete
        || source.kotlin_compile_task.is_none()
            && source
                .compiler_tasks
                .iter()
                .any(|task| task.language == CompilerLanguage::Kotlin)
    {
        return None;
    }
    let mut home = None;
    let mut selected_kotlin = 0;
    for task in &source.compiler_tasks {
        if task.language == CompilerLanguage::Kotlin
            && source.kotlin_compile_task.as_deref() != Some(task.task.as_str())
        {
            continue;
        }
        if !task.complete || task.problem.is_some() {
            return None;
        }
        if task.language == CompilerLanguage::Kotlin {
            selected_kotlin += 1;
        }
        let selected = validated_sdk_task_home(
            task,
            task.java_home.as_deref()?,
            &task.arguments,
            &task.options,
            features,
        )?;
        if home
            .as_ref()
            .is_some_and(|previous: &PathBuf| path_key(previous) != path_key(&selected))
        {
            return None;
        }
        home = Some(selected);
    }
    if source.kotlin_compile_task.is_some() && selected_kotlin != 1 {
        return None;
    }
    home
}

fn validated_sdk_task_home(
    task: &CompilerTask,
    selected: &Path,
    arguments: &[String],
    options: &BTreeMap<String, serde_json::Value>,
    features: &mut BTreeMap<String, Option<u16>>,
) -> Option<PathBuf> {
    if !selected.is_absolute() {
        return None;
    }
    let feature = *features.entry(path_key(selected)).or_insert_with(|| {
        let bytes = std::fs::read(selected.join("release")).ok()?;
        (bytes.len() <= 64 * 1024).then_some(())?;
        sdk_feature_version(std::str::from_utf8(&bytes).ok()?)
    });
    let feature = feature?;
    match task.language {
        CompilerLanguage::Java => {
            if !matches!(
                task.task_type.as_str(),
                "org.gradle.api.tasks.compile.JavaCompile"
                    | "org.gradle.api.tasks.compile.JavaCompile_Decorated"
            ) || options.get("bootstrapClasspath") != Some(&serde_json::Value::Null)
            {
                return None;
            }
            if options.get("forkJavaHome").is_some_and(|home| {
                !home.is_null()
                    && home
                        .as_str()
                        .is_none_or(|home| path_key(Path::new(home)) != path_key(selected))
            }) || options
                .get("forkExecutable")
                .is_some_and(|value| !value.is_null())
            {
                return None;
            }
            match options.get("release") {
                Some(serde_json::Value::Null) => {}
                Some(serde_json::Value::Number(release))
                    if release.as_u64() == Some(u64::from(feature)) => {}
                _ => return None,
            }
            if options
                .get("javaCompilerRecipe")
                .is_some_and(|recipe| !recipe.is_null())
            {
                let recorded = options
                    .get("javaCompilerRecipe")?
                    .get("language_version")?
                    .as_str()?
                    .parse::<u16>()
                    .ok()?;
                if recorded != feature {
                    return None;
                }
                let options: Vec<String> = serde_json::from_value(
                    options
                        .get("javaCompilerRecipe")?
                        .get("api_options")?
                        .clone(),
                )
                .ok()?;
                if !sdk_java_arguments_match(&options, feature) {
                    return None;
                }
            }
            if !sdk_java_arguments_match(arguments, feature) {
                return None;
            }
        }
        CompilerLanguage::Kotlin => {
            if !matches!(
                task.task_type.as_str(),
                "org.jetbrains.kotlin.gradle.tasks.KotlinCompile"
                    | "org.jetbrains.kotlin.gradle.tasks.KotlinCompile_Decorated"
            ) {
                return None;
            }
            let mut arguments = arguments.iter();
            while let Some(argument) = arguments.next() {
                if argument == "-no-jdk"
                    || argument.starts_with('@')
                    || argument.starts_with("-Xbuild-file")
                    || argument == "-module"
                    || argument.starts_with("-module=")
                {
                    return None;
                }
                if argument == "-jdk-home"
                    && path_key(Path::new(arguments.next()?)) != path_key(selected)
                {
                    return None;
                }
                if let Some(home) = argument.strip_prefix("-jdk-home=") {
                    if path_key(Path::new(home)) != path_key(selected) {
                        return None;
                    }
                }
                if let Some(release) = argument.strip_prefix("-Xjdk-release=") {
                    if kotlin_jdk_release_feature(release) != Some(feature) {
                        return None;
                    }
                } else if argument.starts_with("-Xjdk-release") {
                    return None;
                }
            }
        }
    }
    Some(selected.to_path_buf())
}

struct SdkInputs<'a> {
    home: std::borrow::Cow<'a, Path>,
    arguments: std::borrow::Cow<'a, [String]>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KotlinSdkSelection {
    adapter: String,
    task: String,
    language: CompilerLanguage,
    java_home: PathBuf,
    arguments: Vec<String>,
}

fn sdk_inputs(task: &CompilerTask) -> Option<SdkInputs<'_>> {
    if task.complete && task.problem.is_none() {
        return Some(SdkInputs {
            home: std::borrow::Cow::Borrowed(task.java_home.as_deref()?),
            arguments: std::borrow::Cow::Borrowed(&task.arguments),
        });
    }
    let selection: KotlinSdkSelection =
        serde_json::from_value(task.options.get("sdkSelection")?.clone()).ok()?;
    let (plugin_version, maximum_target) = match selection.adapter.as_str() {
        "kgp-1.8.21-jvm-sdk-options-v1" => ("1.8.21", 19),
        "kgp-1.9.25-jvm-sdk-options-v1" => ("1.9.25", 21),
        _ => return None,
    };
    if task.language != CompilerLanguage::Kotlin
        || selection.language != task.language
        || selection.task != task.task
        || !selection.java_home.is_absolute()
        || task
            .kotlin_compiler
            .as_ref()
            .is_some_and(|compiler| compiler.gradle_plugin_version != plugin_version)
        || !matches!(selection.arguments.as_slice(), [flag, target, ..]
            if flag == "-jvm-target" && kotlin_jdk_release_feature(target).is_some_and(|target| (8..=maximum_target).contains(&target)))
    {
        return None;
    }
    // Only SDK inputs are projected. The compiler task and its incomplete execution recipe stay intact.
    Some(SdkInputs {
        home: std::borrow::Cow::Owned(selection.java_home),
        arguments: std::borrow::Cow::Owned(selection.arguments),
    })
}

fn sdk_producing_task<'a>(
    source: &'a SourceSetClasspath,
    language: &CompilerLanguage,
) -> Option<&'a CompilerTask> {
    let task = producing_task(source, language)?;
    (task.complete && task.problem.is_none()).then_some(task)
}

fn producing_task<'a>(
    source: &'a SourceSetClasspath,
    language: &CompilerLanguage,
) -> Option<&'a CompilerTask> {
    let selected = if *language == CompilerLanguage::Kotlin {
        Some(source.kotlin_compile_task.as_deref()?)
    } else {
        None
    };
    let mut tasks = source.compiler_tasks.iter().filter(|task| {
        &task.language == language && selected.is_none_or(|selected| task.task == selected)
    });
    let task = tasks.next()?;
    tasks.next().is_none().then_some(task)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SdkProducerClaims {
    adapter: String,
    language: CompilerLanguage,
    task: String,
    roots: Vec<String>,
    complete: bool,
}

pub(crate) fn sdk_producer_roots(task: &CompilerTask) -> Option<(Vec<String>, bool)> {
    let claims: SdkProducerClaims =
        serde_json::from_value(task.options.get("sdkProducerClaims")?.clone()).ok()?;
    if claims.adapter != "gradle-producing-source-trees-v1"
        || claims.language != task.language
        || claims.task != task.task
        || claims.roots.len() > 1024
        || claims.roots.iter().any(|root| {
            !Path::new(root).is_absolute() || root.split(['/', '\\']).any(|part| part == "..")
        })
    {
        return None;
    }
    let mut roots = claims
        .roots
        .iter()
        .map(|root| path_key(Path::new(root)))
        .collect::<Vec<_>>();
    roots.sort();
    roots.dedup();
    let suffix = if task.language == CompilerLanguage::Java {
        "java"
    } else {
        "kt"
    };
    if claims.complete
        && task
            .source_files
            .iter()
            .filter(|file| file.extension().and_then(|value| value.to_str()) == Some(suffix))
            .any(|file| {
                let file = crate::paths::file_key(file);
                !roots
                    .iter()
                    .any(|root| root == &file || path_under_key(&file, root))
            })
    {
        return None;
    }
    Some((roots, claims.complete))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SdkSourcePolicy {
    adapter: String,
    language: CompilerLanguage,
    task: String,
    authored_roots: Vec<String>,
    future_roots: Vec<String>,
    #[serde(default)]
    default_excludes: Option<Vec<String>>,
    #[serde(skip)]
    excluded_segments: Vec<String>,
    generated_roots: Vec<String>,
}

fn path_under_key(file: &str, root: &str) -> bool {
    file.strip_prefix(root.trim_end_matches('/'))
        .is_some_and(|tail| tail.starts_with('/'))
}

fn sdk_source_policy(
    task: &CompilerTask,
    source: &SourceSetClasspath,
    suffix: &str,
) -> Option<SdkSourcePolicy> {
    let mut policy: SdkSourcePolicy =
        serde_json::from_value(task.options.get("sdkSourcePolicy")?.clone()).ok()?;
    if policy.adapter != "gradle-source-tree-stock-patterns-v1"
        || policy.language != task.language
        || policy.task != task.task
        || policy.authored_roots.len() > 256
        || policy.future_roots.len() > 256
        || policy.generated_roots.len() > 1024
        || suffix
            != match task.language {
                CompilerLanguage::Java => "java",
                CompilerLanguage::Kotlin => "kt",
            }
    {
        return None;
    }
    for roots in [
        &mut policy.authored_roots,
        &mut policy.future_roots,
        &mut policy.generated_roots,
    ] {
        if roots.iter().any(|root| {
            !Path::new(root).is_absolute() || root.split(['/', '\\']).any(|part| part == "..")
        }) {
            return None;
        }
        *roots = roots.iter().map(|root| path_key(Path::new(root))).collect();
        roots.sort();
        roots.dedup();
    }
    if policy.authored_roots.iter().any(|root| {
        !source
            .source_roots
            .iter()
            .any(|declared| path_key(declared) == *root)
    }) || policy
        .future_roots
        .iter()
        .any(|root| !policy.authored_roots.contains(root))
    {
        return None;
    }
    match policy
        .default_excludes
        .as_deref()
        .and_then(ktcore::index::ant_default_exclude_segments)
    {
        Some(excludes) => policy.excluded_segments = excludes,
        None => policy.future_roots.clear(),
    }
    policy
        .generated_roots
        .extend(source.output_roots.iter().map(|root| path_key(root)));
    policy.generated_roots.extend(
        source
            .compiler_tasks
            .iter()
            .flat_map(|task| task.declared_output_roots.iter().chain(&task.output_roots))
            .map(|root| path_key(root)),
    );
    policy.generated_roots.sort();
    policy.generated_roots.dedup();
    Some(policy)
}

fn source_set_sdk_api(
    source: &SourceSetClasspath,
    language: CompilerLanguage,
    features: &mut BTreeMap<String, Option<u16>>,
    digests: &mut BTreeMap<String, Option<SdkApiSelection>>,
) -> Option<SdkApiSelection> {
    use sha2::{Digest, Sha256};
    let task = producing_task(source, &language)?;
    let inputs = sdk_inputs(task)?;
    // Another language's failed compiler classpath cannot revoke this proved SDK producer.
    if source.compiler_metadata_incomplete
        && (!sdk_producer_roots(task).is_some_and(|(_, complete)| complete)
            || sdk_source_policy(
                task,
                source,
                if language == CompilerLanguage::Java {
                    "java"
                } else {
                    "kt"
                },
            )
            .is_none())
    {
        return None;
    }
    let home = inputs.home.as_ref();
    if !home.is_absolute() {
        return None;
    }
    let feature = *features.entry(path_key(home)).or_insert_with(|| {
        let bytes = std::fs::read(home.join("release")).ok()?;
        (bytes.len() <= 64 * 1024).then_some(())?;
        sdk_feature_version(std::str::from_utf8(&bytes).ok()?)
    });
    let feature = feature?;
    let mut normalized_arguments = inputs.arguments.into_owned();
    let mut normalized_options = task.options.clone();
    let release = match language {
        CompilerLanguage::Java => {
            let release = match task.options.get("release")? {
                serde_json::Value::Null => None,
                serde_json::Value::Number(value) => Some(u16::try_from(value.as_u64()?).ok()?),
                _ => return None,
            };
            let target = release.unwrap_or(feature);
            if !sdk_java_arguments_match(&normalized_arguments, target) {
                return None;
            }
            normalize_java_release(&mut normalized_arguments, feature)?;
            if let Some(recipe) = normalized_options
                .get_mut("javaCompilerRecipe")
                .filter(|recipe| !recipe.is_null())
            {
                let mut arguments: Vec<String> =
                    serde_json::from_value(recipe.get("api_options")?.clone()).ok()?;
                if !sdk_java_arguments_match(&arguments, target) {
                    return None;
                }
                normalize_java_release(&mut arguments, feature)?;
                recipe["api_options"] = serde_json::to_value(arguments).ok()?;
            }
            normalized_options.insert("release".into(), serde_json::json!(feature));
            release
        }
        CompilerLanguage::Kotlin => {
            let mut target = None;
            let mut arguments = normalized_arguments.iter();
            while let Some(argument) = arguments.next() {
                let value = if argument == "-jvm-target" {
                    Some(arguments.next()?.as_str())
                } else {
                    argument.strip_prefix("-jvm-target=")
                };
                if let Some(value) = value {
                    if target.is_some() {
                        return None;
                    }
                    target = Some(kotlin_jdk_release_feature(value)?);
                }
            }
            let mut release = None;
            for argument in &mut normalized_arguments {
                if let Some(value) = argument.strip_prefix("-Xjdk-release=") {
                    let selected = kotlin_jdk_release_feature(value)?;
                    if release.is_some_and(|previous| previous != selected) {
                        return None;
                    }
                    release = Some(selected);
                    *argument = format!("-Xjdk-release={feature}");
                } else if argument.starts_with("-Xjdk-release") {
                    return None;
                }
            }
            // Validate the authored flags before normalizing the release for SDK-home checks.
            if release
                .zip(target)
                .is_some_and(|(release, target)| release != target)
            {
                return None;
            }
            release
        }
    };
    if release.is_some_and(|release| release < 8 || release > feature) {
        return None;
    }
    // Reuse the same SDK guards with normalized SDK-only options, never a promoted compiler task.
    if validated_sdk_task_home(
        task,
        home,
        &normalized_arguments,
        &normalized_options,
        features,
    )
    .as_deref()
        != Some(home)
    {
        return None;
    }
    let source_zip = crate::deps::src_zip_for_home(home.to_path_buf());
    let release = release.filter(|release| *release != feature);
    let archive = match release {
        Some(_) => home.join("lib/ct.sym"),
        None => source_zip.clone()?,
    };
    digests
        .entry(format!(
            "{}#{}",
            path_key(&archive),
            release.map_or_else(|| "full".into(), |release| release.to_string())
        ))
        .or_insert_with(|| {
            let before = std::fs::metadata(&archive).ok()?;
            if before.len() > 256 * 1024 * 1024 {
                return None;
            }
            let bytes = std::fs::read(&archive).ok()?;
            let after = std::fs::metadata(&archive).ok()?;
            (before.len() == bytes.len() as u64
                && after.len() == before.len()
                && after.modified().ok()? == before.modified().ok()?)
            .then_some(())?;
            let entries = match release {
                Some(release) => crate::sdk_api::release_entries(&bytes, release).ok()?,
                None => BTreeMap::new(),
            };
            Some(SdkApiSelection {
                home: home.to_path_buf(),
                source_zip,
                release,
                archive: archive.clone(),
                archive_sha256: format!("{:x}", Sha256::digest(bytes)),
                archive_revision: file_origin_id(&archive),
                entries: Arc::new(entries),
            })
        })
        .clone()
}

fn normalize_java_release(arguments: &mut [String], feature: u16) -> Option<()> {
    let mut arguments = arguments.iter_mut();
    while let Some(argument) = arguments.next() {
        if argument == "--release" {
            *arguments.next()? = feature.to_string();
        } else if argument.starts_with("--release=") {
            *argument = format!("--release={feature}");
        }
    }
    Some(())
}

fn sdk_java_arguments_match(arguments: &[String], feature: u16) -> bool {
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        if argument == "--release" {
            if arguments
                .next()
                .and_then(|release| release.parse::<u16>().ok())
                != Some(feature)
            {
                return false;
            }
        } else if let Some(release) = argument.strip_prefix("--release=") {
            if release.parse::<u16>().ok() != Some(feature) {
                return false;
            }
        } else if argument.starts_with('@')
            || argument.starts_with("-J")
            || argument.starts_with("-Xbootclasspath")
            || [
                "--system",
                "--patch-module",
                "--upgrade-module-path",
                "--module-path",
                "--limit-modules",
                "-bootclasspath",
                "--boot-class-path",
                "-endorseddirs",
                "-extdirs",
                "-profile",
            ]
            .iter()
            .any(|flag| {
                argument == flag
                    || argument.starts_with(&format!("{flag}="))
                    || argument.starts_with(&format!("{flag}:"))
            })
        {
            return false;
        }
    }
    true
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

pub(crate) fn is_sdk_origin(origin: &str) -> bool {
    origin.starts_with("jdk:") || origin.starts_with("jdk-api:")
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
    let mut resource_input_sources = std::collections::BTreeSet::new();
    let mut runtime_input_sources = std::collections::BTreeSet::new();
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
                            | "RUNTIME_INPUTS"
                            | "RESOURCE_INPUTS"
                            | "SOURCE"
                            | "OUTPUT"
                            | "PROVIDES"
                            | "SCP"
                            | "ARTIFACT"
                            | "SOURCEARTIFACT"
                            | "SOURCEQUERY"
                            | "POMARTIFACT"
                            | "POMQUERY"
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
                resource_input_sources.clear();
                runtime_input_sources.clear();
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
                record @ ("RUNTIME_INPUTS"
                | "RESOURCE_INPUTS"
                | "SOURCESET"
                | "SOURCE"
                | "OUTPUT"
                | "PROVIDES"
                | "SCP"
                | "ARTIFACT"
                | "SOURCEARTIFACT"
                | "SOURCEQUERY"
                | "POMARTIFACT"
                | "POMQUERY"
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
                    if record == "SOURCEQUERY" || record == "POMQUERY" {
                        let (artifacts, incomplete) = if record == "POMQUERY" {
                            (&mut source.source_poms, &mut source.source_poms_incomplete)
                        } else {
                            (
                                &mut source.source_artifacts,
                                &mut source.source_artifacts_incomplete,
                            )
                        };
                        if artifacts.is_some() || *incomplete {
                            *artifacts = None;
                            *incomplete = true;
                        } else {
                            *artifacts = Some(Vec::new());
                            *incomplete = parts.next() != Some("complete");
                        }
                        continue;
                    }
                    if record == "RESOURCE_INPUTS" {
                        let parsed = parts.next().and_then(|json|
                            serde_json::from_str::<crate::resource_model::ResourceClaims>(json).ok());
                        source.resources = if resource_input_sources.insert(name.to_owned()) && parts.next().is_none() {
                            parsed.filter(crate::resource_model::ResourceClaims::valid)
                        } else { None };
                        continue;
                    }
                    if record == "RUNTIME_INPUTS" {
                        let runtime = parts.next().and_then(|json|
                            serde_json::from_str::<RuntimeClassInputs>(json).ok());
                        source.runtime = if runtime_input_sources.insert(name.to_owned()) && parts.next().is_none() {
                            runtime
                        } else { None };
                        continue;
                    }
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
                            "ARTIFACT" | "SOURCEARTIFACT" | "POMARTIFACT" => {
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
                                    let artifact = ResolvedArtifact { path, component };
                                    if record == "SOURCEARTIFACT" || record == "POMARTIFACT" {
                                        let (artifacts, incomplete) = if record == "POMARTIFACT" {
                                            (
                                                &mut source.source_poms,
                                                &mut source.source_poms_incomplete,
                                            )
                                        } else {
                                            (
                                                &mut source.source_artifacts,
                                                &mut source.source_artifacts_incomplete,
                                            )
                                        };
                                        if !matches!(
                                            &artifact.component,
                                            ComponentId::Module { .. }
                                        ) {
                                            *incomplete = true;
                                            *artifacts = None;
                                        } else if let Some(artifacts) = artifacts {
                                            artifacts.push(artifact);
                                        } else {
                                            *incomplete = true;
                                        }
                                    } else {
                                        source.artifacts.push(artifact);
                                    }
                                } else if record == "SOURCEARTIFACT" {
                                    source.source_artifacts_incomplete = true;
                                    source.source_artifacts = None;
                                } else if record == "POMARTIFACT" {
                                    source.source_poms_incomplete = true;
                                    source.source_poms = None;
                                }
                            }
                            _ => {}
                        }
                    } else if record == "SOURCEARTIFACT" {
                        source.source_artifacts_incomplete = true;
                        source.source_artifacts = None;
                    } else if record == "POMARTIFACT" {
                        source.source_poms_incomplete = true;
                        source.source_poms = None;
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

/// Build configuration and declared Gradle transform mtimes that invalidate the cache.
fn build_file_mtimes(root: &Path, modules: &[ModuleClasspath]) -> BTreeMap<String, u128> {
    build_file_mtimes_with_presence(root, modules).0
}

fn model_inputs_current(
    root: &Path,
    modules: &[ModuleClasspath],
    recorded: &BTreeMap<String, u128>,
) -> bool {
    let (current, required_inputs_present) = build_file_mtimes_with_presence(root, modules);
    required_inputs_present && recorded == &current
}

fn build_file_mtimes_with_presence(
    root: &Path,
    modules: &[ModuleClasspath],
) -> (BTreeMap<String, u128>, bool) {
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
    // Gradle may retire a declared transform without changing any build file.
    let inputs: BTreeSet<_> = modules
        .iter()
        .flat_map(|module| &module.source_sets)
        .flat_map(|source| &source.compiler_tasks)
        .flat_map(|task| &task.declared_input_files)
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with("-snapshot.bin"))
                && path.parent().is_some_and(|parent| {
                    parent.file_name().is_some_and(|name| name == "transformed")
                        && parent
                            .parent()
                            .and_then(Path::parent)
                            .and_then(Path::file_name)
                            .is_some_and(|name| name == "transforms")
                })
        })
        .collect();
    let mut required_inputs_present = true;
    for path in inputs {
        let mtime = std::fs::metadata(path)
            .ok()
            .filter(|metadata| metadata.is_file())
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok());
        required_inputs_present &= mtime.is_some();
        map.insert(path_key(path), mtime.map_or(0, |value| value.as_nanos()));
    }
    (map, required_inputs_present)
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

fn model_cache_hash(root: &Path, module: Option<&str>, script: &str) -> u64 {
    let key = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut hash: u64 = 1469598103934665603; // FNV-1a
                                             // Compiler recipe enrichment changes with the embedded adapter, even when build files do not.
    for b in key
        .to_string_lossy()
        .bytes()
        .chain([0])
        .chain(module.unwrap_or("").bytes())
        .chain([0])
        .chain(script.bytes())
    {
        hash ^= b as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    hash
}

fn cache_path(root: &Path) -> PathBuf {
    let hash = model_cache_hash(root, None, INIT_SCRIPT);
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
    load_snapshot_cancellable(root, force, None)
}

pub(crate) fn load_snapshot_cancellable(
    root: &Path,
    force: bool,
    cancelled: Option<&std::sync::atomic::AtomicBool>,
) -> anyhow::Result<Arc<ProjectSnapshot>> {
    load_snapshot_with_cancellable(root, force, cancelled, || {
        dump_with_fallback_cancellable(root, "ktlspDumpClasspath", cancelled)
    })
}

/// Enrich an accepted model after project sources are usable; source downloads do not delay it.
pub(crate) fn load_source_snapshot(
    root: &Path,
    accepted: &Arc<ProjectSnapshot>,
    cancelled: &std::sync::atomic::AtomicBool,
) -> anyhow::Result<Arc<ProjectSnapshot>> {
    if !accepted.source_enrichment_needed() {
        return Ok(accepted.clone());
    }
    let start = std::time::Instant::now();
    let result = run_dump_cancellable(root, "ktlspDumpSources", false, Some(cancelled));
    crate::trace::span(
        "classpath.source_query",
        "classpath",
        start,
        serde_json::json!({"root": root.to_string_lossy(), "ok": result.is_ok()}),
    );
    source_snapshot(root, accepted, result?)
}

fn source_snapshot(
    root: &Path,
    accepted: &ProjectSnapshot,
    modules: Vec<ModuleClasspath>,
) -> anyhow::Result<Arc<ProjectSnapshot>> {
    let without_sources = |modules: &[ModuleClasspath]| {
        let mut by_identity = BTreeMap::new();
        for mut module in modules.to_vec() {
            for source in &mut module.source_sets {
                source.source_artifacts = None;
                source.source_artifacts_incomplete = false;
                source.source_poms = None;
                source.source_poms_incomplete = false;
            }
            // Parallel Gradle tasks may emit complete modules in a different order. A project
            // path belongs to its build; preserve every other fact and every inner ordering.
            let identity = (module.build.clone(), module.module.clone());
            anyhow::ensure!(
                by_identity.insert(identity, module).is_none(),
                "Duplicate Gradle module identity during source resolution"
            );
        }
        Ok::<_, anyhow::Error>(by_identity)
    };
    anyhow::ensure!(
        without_sources(&modules)? == without_sources(&accepted.modules)?,
        "Gradle binary model changed during source resolution"
    );
    anyhow::ensure!(
        model_inputs_current(root, &accepted.modules, &accepted.mtimes),
        "Gradle build inputs changed during source resolution"
    );
    let mut enriched = ProjectSnapshot::new(modules);
    enriched.mtimes = accepted.mtimes.clone();
    Ok(Arc::new(enriched))
}

fn load_snapshot_with(
    root: &Path,
    force: bool,
    dump: impl FnOnce() -> anyhow::Result<Vec<ModuleClasspath>>,
) -> anyhow::Result<Arc<ProjectSnapshot>> {
    load_snapshot_with_cancellable(root, force, None, dump)
}

fn load_snapshot_with_cancellable(
    root: &Path,
    force: bool,
    cancelled: Option<&std::sync::atomic::AtomicBool>,
    dump: impl FnOnce() -> anyhow::Result<Vec<ModuleClasspath>>,
) -> anyhow::Result<Arc<ProjectSnapshot>> {
    let check_cancelled = || -> anyhow::Result<()> {
        anyhow::ensure!(
            !cancelled.is_some_and(|flag| flag.load(AtomicOrdering::Acquire)),
            "Gradle model resolution cancelled"
        );
        Ok(())
    };
    let lock_start = std::time::Instant::now();
    let _guard = loop {
        check_cancelled()?;
        match RESOLUTION_LOCK.try_lock() {
            Ok(guard) => break guard,
            Err(std::sync::TryLockError::Poisoned(error)) => break error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                std::thread::sleep(std::time::Duration::from_millis(25))
            }
        }
    };
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
        if model_inputs_current(root, &model.modules, &model.mtimes) {
            trace_cache("memory");
            check_cancelled()?;
            return Ok(model);
        }
    }
    let cache = cache_path(root);
    if !force {
        if let Some(entry) = read_cache_entry(&cache) {
            if entry.model_version == MODEL_VERSION
                && model_inputs_current(root, &entry.modules, &entry.mtimes)
            {
                let mut model = ProjectSnapshot::new(entry.modules);
                model.mtimes = entry.mtimes;
                trace_cache("disk");
                check_cancelled()?;
                return Ok(Arc::new(model));
            }
        }
    }
    trace_cache("miss");
    let modules = dump()?;
    check_cancelled()?;
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
        if model_inputs_current(root, &model.modules, &model.mtimes) {
            if let Some(module) = model.modules.iter().find(|m| m.module == module) {
                return Ok(module.clone());
            }
        }
    }
    let cache = module_cache_path(root, module);
    if let Some(entry) = read_cache_entry(&cache) {
        if entry.model_version == MODEL_VERSION
            && model_inputs_current(root, &entry.modules, &entry.mtimes)
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
    let hash = model_cache_hash(root, Some(module), INIT_SCRIPT);
    crate::deps::cache_home()
        .join("classpath")
        .join(format!("mod-{hash:016x}.json"))
}

/// Offline-first dump: with a populated Gradle cache the result is a pure function of local
/// state, immune to repository flakiness. An empty or failed offline dump means the cache isn't
/// populated (fresh machine) — fall back to the online run for completeness.
fn dump_with_fallback(root: &Path, task: &str) -> anyhow::Result<Vec<ModuleClasspath>> {
    dump_with_fallback_cancellable(root, task, None)
}

fn dump_with_fallback_cancellable(
    root: &Path,
    task: &str,
    cancelled: Option<&std::sync::atomic::AtomicBool>,
) -> anyhow::Result<Vec<ModuleClasspath>> {
    let run = |offline| {
        let start = std::time::Instant::now();
        let result = run_dump_cancellable(root, task, offline, cancelled);
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
        Err(_) => {
            anyhow::ensure!(
                !cancelled.is_some_and(|flag| flag.load(AtomicOrdering::Acquire)),
                "Gradle model resolution cancelled"
            );
            tracing::info!("offline classpath dump failed; retrying online");
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

fn create_dump_script() -> anyhow::Result<PathBuf> {
    cached_dump_script(&crate::deps::cache_home().join("classpath"), INIT_SCRIPT)
}

fn cached_dump_script(directory: &Path, contents: &str) -> anyhow::Result<PathBuf> {
    use sha2::{Digest, Sha256};
    let directory = directory.join(format!("init-{:x}", Sha256::digest(contents.as_bytes())));
    std::fs::create_dir_all(&directory)?;
    let path = directory.join("ktlsp-classpath.init.gradle.kts");
    match std::fs::read(&path) {
        Ok(bytes) => {
            anyhow::ensure!(
                bytes == contents.as_bytes(),
                "cached Gradle init script differs"
            );
            return Ok(path);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let sequence = CACHE_WRITE_SEQUENCE.fetch_add(1, AtomicOrdering::Relaxed);
    let tmp = path.with_extension(format!("kts.{}.{sequence}.tmp", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)?;
    let write_result = file.write_all(contents.as_bytes());
    drop(file);
    let _cleanup = TempFile(&tmp);
    write_result?;
    // Windows may have a concurrent complete winner; never truncate a published script.
    let published = std::fs::rename(&tmp, &path);
    anyhow::ensure!(
        std::fs::read(&path)? == contents.as_bytes(),
        "cached Gradle init script publication differs: {published:?}"
    );
    Ok(path)
}

/// Run the init script via the project's gradle wrapper and parse the dump. Inherits the parent's
/// environment (so repo-specific flags like SKIP_* are honored when ktlsp is launched with them).
/// `offline` passes `--offline`: resolution then depends only on the local Gradle cache, which is
/// stable across runs — online runs drop entries nondeterministically under repository flakiness.
fn run_dump(root: &Path, task: &str, offline: bool) -> anyhow::Result<Vec<ModuleClasspath>> {
    run_dump_cancellable(root, task, offline, None)
}

fn run_dump_cancellable(
    root: &Path,
    task: &str,
    offline: bool,
    cancelled: Option<&std::sync::atomic::AtomicBool>,
) -> anyhow::Result<Vec<ModuleClasspath>> {
    anyhow::ensure!(
        !cancelled.is_some_and(|flag| flag.load(AtomicOrdering::Acquire)),
        "Gradle resolution cancelled before launch"
    );
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
    // Keep the content-addressed script for Gradle Kotlin DSL compilation reuse.
    let script = create_dump_script()?;

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
        crate::compile::run_model_with_stdout_file_cancellable(
            &mut command,
            stdout.try_clone()?,
            std::time::Duration::from_secs(60 * 60),
            cancelled,
        )?;
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
pub(crate) mod tests {
    #[test]
    fn resource_scope_sdk_requires_all_configured_languages_to_agree() {
        let root = std::env::temp_dir().join("ktlsp_resource_sdk_scope_only");
        let java = CompilerTask { task:":compileJava".into(), language:CompilerLanguage::Java,
            task_type:"JavaCompile".into(), arguments:Vec::new(), annotation_processor_path:Vec::new(),
            source_files:Vec::new(), classpath:Vec::new(), declared_input_files:Vec::new(),
            declared_output_roots:Vec::new(), java_home:None, kotlin_compiler:None,
            options:BTreeMap::new(), output_roots:Vec::new(), complete:true, problem:None };
        let mut kotlin = java.clone(); kotlin.task=":compileKotlin".into(); kotlin.language=CompilerLanguage::Kotlin;
        let source = SourceSetClasspath { name:"main".into(), compiler_tasks:vec![java,kotlin], ..Default::default() };
        let module = ModuleClasspath {module:":app".into(), project_dir:root.clone(), build:String::new(),
            source_sets:vec![source], entries:Vec::new(), unresolved:Vec::new()};
        let scope = project_scope(&module,&module.source_sets[0]);
        let mut model = ProjectSnapshot::new(vec![module]);
        model.sdk_selections.clear();
        assert!(model.sdk_api_for_scope(&scope).is_none());
        let sdk = SdkApiSelection {home:root.clone(), source_zip:Some(root.join("lib/src.zip")), release:None,
            archive:root.join("lib/src.zip"), archive_sha256:"fixture-model-only".into(),
            archive_revision:"fixture-model-only".into(), entries:std::sync::Arc::new(BTreeMap::new())};
        model.sdk_selections.insert(scope.clone(), BTreeMap::from([("java".into(),sdk.clone())]));
        assert!(model.sdk_api_for_scope(&scope).is_none(), "unknown Kotlin selection cannot borrow Java's SDK");
        model.sdk_selections.get_mut(&scope).unwrap().insert("kt".into(),sdk.clone());
        assert_eq!(model.sdk_api_for_scope(&scope),Some(&sdk));
        model.sdk_selections.get_mut(&scope).unwrap().get_mut("kt").unwrap().release=Some(11);
        assert!(model.sdk_api_for_scope(&scope).is_none(), "different targets cannot silently choose one SDK");
    }


    #[test]
    fn resource_runtime_metadata_preserves_selected_owners_and_unknown_authority() {
        use crate::resource_model::{ResourceClaims, ResourceFile, ResourcePatterns, ResourceTree};
        use ktcore::knowledge::Knowledge;
        let root = std::env::current_dir().unwrap().join("resource-model-no-io-fixture");
        let resources = root.join("src/main/resources");
        let selected = resources.join("META-INF/😀.json");
        let mut claims = ResourceClaims {
            adapter: "gradle-source-set-resources-v1".into(), roots: vec![resources.clone()],
            trees: vec![ResourceTree {root:resources.clone(), patterns:vec![ResourcePatterns {
                includes:vec!["META-INF/**".into()], excludes:vec!["**/excluded.json".into()]
            }],future_complete:true}],
            files:vec![ResourceFile {path:selected.clone(),relative_path:"META-INF/😀.json".into()}],
            output_roots:vec![root.join("build/resources/main")], configuration_inputs:vec![root.join("gradle.properties")],
            default_excludes:vec!["**/.git/**".into()], complete:true, problem:None,
        };
        let runtime=RuntimeClassInputs {adapter:"gradle-runtime-class-inputs-v1".into(),
            classpath:vec![root.join("build/classes/java/main"),root.join("dep.jar")],
            class_outputs:vec![root.join("build/classes/java/main")],complete:true,problem:None};
        let dump=|claims:&ResourceClaims,tail:&str|format!(
            "PROJECT\t:app\t{}\nSOURCESET\tmain\nRESOURCE_INPUTS\tmain\t{}\nRUNTIME_INPUTS\tmain\t{}\n{tail}END\t:app\n",
            root.display(),serde_json::to_string(claims).unwrap(),serde_json::to_string(&runtime).unwrap());
        let model=ProjectSnapshot::new(parse_dump_reader(dump(&claims,"").as_bytes(),true).unwrap());
        let source=&model.modules[0].source_sets[0];
        assert_eq!(source.resources.as_ref(),Some(&claims));
        assert_eq!(source.runtime.as_ref().unwrap().classpath,runtime.classpath);
        assert!(source.runtime.as_ref().unwrap().usable());
        assert!(!source.compiler_metadata_incomplete);
        let Knowledge::Found(owner)=model.resource_owner_context(&selected) else {panic!("selected resource")};
        assert_eq!(owner.scope.source_set,"main"); assert_eq!(owner.association,"source");
        assert_eq!(owner.relative_path,"META-INF/😀.json");
        assert!(matches!(model.resource_owner_context(&resources.join("META-INF/new.aw")),Knowledge::Found(_)));
        for denied in [resources.join("other.json"), resources.join("META-INF/excluded.json"), root.join("unselected.json")] {
            assert!(matches!(model.resource_owner_context(&denied),Knowledge::Unknown(_)));
        }
        // A custom future predicate never erases an actual selected file, and never
        // authorizes a new buffer merely because its path is under a resource root.
        claims.complete=false;claims.problem=Some("opaque-filter".into());claims.trees[0].future_complete=false;
        let model=ProjectSnapshot::new(parse_dump_reader(dump(&claims,"").as_bytes(),true).unwrap());
        assert!(matches!(model.resource_owner_context(&selected),Knowledge::Found(_)));
        assert!(matches!(model.resource_owner_context(&resources.join("META-INF/new.aw")),Knowledge::Unknown(_)));
        let duplicate=format!("RESOURCE_INPUTS\tmain\t{}\nRUNTIME_INPUTS\tmain\t{}\n",
            serde_json::to_string(&claims).unwrap(),serde_json::to_string(&runtime).unwrap());
        let invalid=parse_dump_reader(dump(&claims,&duplicate).as_bytes(),true).unwrap();
        assert!(invalid[0].source_sets[0].resources.is_none()); assert!(invalid[0].source_sets[0].runtime.is_none());
        assert!(!invalid[0].source_sets[0].compiler_metadata_incomplete);
        let legacy=ProjectSnapshot::new(parse_dump_reader(format!("PROJECT\t:app\t{}\nSOURCESET\tmain\nEND\t:app\n",root.display()).as_bytes(),true).unwrap());
        assert!(legacy.modules[0].source_sets[0].resources.is_none());assert!(legacy.modules[0].source_sets[0].runtime.is_none());
        assert!(matches!(legacy.resource_owner_context(&selected),Knowledge::Unknown(_)));
    }

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
    fn selected_sdk_release_features_use_the_standard_bounded_version_key() {
        for (text, expected) in [
            ("JAVA_VERSION=\"17.0.7\"", Some(17)),
            ("JAVA_VERSION=\"21.0.3+9-LTS\"", Some(21)),
            ("JAVA_VERSION=\"1.8.0_402\"", Some(8)),
            ("JAVA_VERSION=\"25-ea\"", Some(25)),
            ("JAVA_VERSION=\"17foo\"", None),
            ("JAVA_VERSION=\"17.foo\"", None),
            ("JAVA_VERSION=\"17..0\"", None),
            ("JAVA_VERSION=17", None),
            ("JAVA_VERSION=\"17\"\nJAVA_VERSION=\"21\"", None),
            ("JAVA_VERSION_DATE=\"2023-01-01\"", None),
        ] {
            assert_eq!(sdk_feature_version(text), expected, "{text}");
        }
    }

    #[test]
    fn caller_sdk_future_paths_require_captured_default_exclusions() {
        let (root, original) = sdk_selection_fixture();
        let source_root = original[0].source_sets[0].source_roots[0].clone();
        assert!(ProjectSnapshot::new(original.clone())
            .sdk_api_for_file(&source_root.join("New.java"))
            .is_some());
        for failure in [
            "missing",
            "null",
            "unsupported",
            "unpaired-subtree",
            "oversized",
        ] {
            let mut modules = original.clone();
            for task in &mut modules[0].source_sets[0].compiler_tasks {
                let policy = task.options.get_mut("sdkSourcePolicy").unwrap();
                match failure {
                    "missing" => {
                        policy.as_object_mut().unwrap().remove("default_excludes");
                    }
                    "null" => policy["default_excludes"] = serde_json::Value::Null,
                    "unsupported" => {
                        policy["default_excludes"] = serde_json::json!(["**/unknown/*/tree"])
                    }
                    "unpaired-subtree" => {
                        policy["default_excludes"] = serde_json::json!(["**/Excluded.java/**"])
                    }
                    "oversized" => {
                        policy["default_excludes"] = serde_json::json!(vec!["**/excluded"; 129])
                    }
                    _ => unreachable!(),
                }
            }
            let model = ProjectSnapshot::new(modules);
            for suffix in ["java", "kt"] {
                assert!(
                    model
                        .sdk_api_for_file(&source_root.join(format!("Use.{suffix}")))
                        .is_some(),
                    "{failure}: exact compiler input"
                );
                assert!(
                    model
                        .sdk_api_for_file(&source_root.join(format!("New.{suffix}")))
                        .is_none(),
                    "{failure}: future input remains unknown"
                );
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    pub(crate) fn sdk_source_default_excludes() -> serde_json::Value {
        serde_json::json!([
            "**/%*%",
            "**/.git/**",
            "**/SCCS",
            "**/.bzr",
            "**/.hg/**",
            "**/.bzrignore",
            "**/.git",
            "**/SCCS/**",
            "**/.hg",
            "**/.#*",
            "**/vssver.scc",
            "**/.bzr/**",
            "**/._*",
            "**/#*#",
            "**/*~",
            "**/CVS",
            "**/.hgtags",
            "**/.svn/**",
            "**/.hgignore",
            "**/.svn",
            "**/.gitignore",
            "**/.gitmodules",
            "**/.hgsubstate",
            "**/.gitattributes",
            "**/CVS/**",
            "**/.hgsub",
            "**/.DS_Store",
            "**/.cvsignore"
        ])
    }

    pub(crate) fn sdk_selection_fixture() -> (PathBuf, Vec<ModuleClasspath>) {
        // Concurrent fixtures must not share cleanup ownership when the clock repeats a tick.
        let root = std::env::temp_dir().join(format!(
            "ktlsp-sdk-selection-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            CACHE_WRITE_SEQUENCE.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        let mut modules = Vec::new();
        for feature in [17, 21] {
            let home = root.join(format!("jdk{feature}"));
            std::fs::create_dir_all(home.join("lib")).unwrap();
            std::fs::write(home.join("lib/src.zip"), "explicit SDK source stamp").unwrap();
            std::fs::write(
                home.join("release"),
                format!("JAVA_VERSION=\"{feature}.0.1\"\n"),
            )
            .unwrap();
            let mut java = plain_java_configuration();
            java["java_home"] = serde_json::json!(home);
            java["options"]["release"] = if feature == 17 {
                serde_json::Value::Null
            } else {
                serde_json::json!(21)
            };
            java["options"]["bootstrapClasspath"] = serde_json::Value::Null;
            java["options"]["javaCompilerRecipe"] = selected_java_recipe();
            java["options"]["javaCompilerRecipe"]["language_version"] = feature.to_string().into();
            let authored_root = root.join(format!("module{feature}/src"));
            java["source_files"] = serde_json::json!([authored_root.join("Use.java")]);
            java["options"]["sdkSourcePolicy"] = serde_json::json!({
                "adapter": "gradle-source-tree-stock-patterns-v1", "task": java["task"], "language": "java",
                "authored_roots": [authored_root], "future_roots": [authored_root], "generated_roots": [],
                "default_excludes": sdk_source_default_excludes(),
            });
            java["options"]["sdkProducerClaims"] = serde_json::json!({
                "adapter": "gradle-producing-source-trees-v1", "task": java["task"],
                "language": "java", "roots": [authored_root], "complete": true,
            });
            let mut kotlin = java.clone();
            kotlin["task"] = serde_json::json!(format!(":sdk{feature}:compileKotlin"));
            kotlin["language"] = serde_json::json!("kotlin");
            kotlin["task_type"] =
                serde_json::json!("org.jetbrains.kotlin.gradle.tasks.KotlinCompile");
            kotlin["source_files"] =
                serde_json::json!([authored_root.join("Use.kt"), authored_root.join("Use.java")]);
            kotlin["arguments"] = serde_json::json!([]);
            kotlin["options"]["sdkSourcePolicy"]["task"] = kotlin["task"].clone();
            kotlin["options"]["sdkSourcePolicy"]["language"] = serde_json::json!("kotlin");
            kotlin["options"]["sdkProducerClaims"]["task"] = kotlin["task"].clone();
            kotlin["options"]["sdkProducerClaims"]["language"] = serde_json::json!("kotlin");
            let kotlin_task = kotlin["task"].as_str().unwrap().to_owned();
            modules.push(ModuleClasspath {
                module: format!(":sdk{feature}"),
                project_dir: root.join(format!("module{feature}")),
                build: String::new(),
                entries: Vec::new(),
                unresolved: Vec::new(),
                source_sets: vec![SourceSetClasspath {
                    name: "main".into(),
                    source_roots: vec![root.join(format!("module{feature}/src"))],
                    compiler_tasks: vec![
                        serde_json::from_value(java).unwrap(),
                        serde_json::from_value(kotlin).unwrap(),
                    ],
                    kotlin_compile_task: Some(kotlin_task),
                    ..Default::default()
                }],
            });
        }
        (root, modules)
    }

    #[test]
    fn kotlin_sdk_witness_is_narrow_and_never_promotes_the_compiler_recipe() {
        let (root, modules) = sdk_selection_fixture();
        let mut original = modules[0].clone();
        let source = &mut original.source_sets[0];
        source.compiler_metadata_incomplete = true;
        let caller = source.source_roots[0].join("Use.kt");
        let task = source
            .compiler_tasks
            .iter_mut()
            .find(|task| task.language == CompilerLanguage::Kotlin)
            .unwrap();
        let home = task.java_home.take().unwrap();
        task.complete = false;
        task.problem = Some("compiler classpath is unavailable".into());
        task.arguments = vec!["original incomplete compiler arguments".into()];
        let witness = serde_json::json!({
            "adapter": "kgp-1.9.25-jvm-sdk-options-v1", "task": task.task,
            "language": "kotlin", "java_home": home,
            "arguments": ["-jvm-target", "17"],
        });
        for fault in [
            "valid",
            "valid-1.8.21",
            "valid-1.8.21-target19",
            "1.8.21-target20",
            "missing",
            "adapter",
            "task",
            "language",
            "home",
            "target",
            "no-jdk",
            "foreign-jdk",
            "opaque",
            "release",
            "extra",
            "type",
            "claims",
            "policy",
        ] {
            let mut module = original.clone();
            let task = module.source_sets[0]
                .compiler_tasks
                .iter_mut()
                .find(|task| task.language == CompilerLanguage::Kotlin)
                .unwrap();
            let mut proof = witness.clone();
            match fault {
                "valid-1.8.21" | "valid-1.8.21-target19" | "1.8.21-target20" => {
                    proof["adapter"] = "kgp-1.8.21-jvm-sdk-options-v1".into();
                    if fault != "valid-1.8.21" {
                        proof["arguments"] = serde_json::json!([
                            "-jvm-target",
                            if fault == "valid-1.8.21-target19" {
                                "19"
                            } else {
                                "20"
                            }
                        ]);
                    }
                }
                "adapter" => proof["adapter"] = "unverified".into(),
                "task" => proof["task"] = ":different:compileKotlin".into(),
                "language" => proof["language"] = "java".into(),
                "home" => proof["java_home"] = "relative-sdk".into(),
                "target" => proof["arguments"] = serde_json::json!(["-jvm-target", "999"]),
                "no-jdk" => proof["arguments"] = serde_json::json!(["-jvm-target", "17", "-no-jdk"]),
                "foreign-jdk" => {
                    proof["arguments"] =
                        serde_json::json!(["-jvm-target", "17", "-jdk-home", root.join("jdk21")])
                }
                "opaque" => {
                    proof["arguments"] = serde_json::json!(["-jvm-target", "17", "@opaque.args"])
                }
                "release" => {
                    proof["arguments"] = serde_json::json!(["-jvm-target", "17", "-Xjdk-release=1.8"])
                }
                "extra" => proof["unverified_override"] = true.into(),
                "type" => task.task_type = "custom.KotlinCompiler".into(),
                "claims" => {
                    task.options.get_mut("sdkProducerClaims").unwrap()["complete"] = false.into()
                }
                "policy" => {
                    task.options
                        .insert("sdkSourcePolicy".into(), serde_json::Value::Null);
                }
                _ => {}
            }
            if fault != "missing" {
                task.options.insert("sdkSelection".into(), proof);
            }
            let model = ProjectSnapshot::new(vec![module]);
            assert_eq!(
                model.sdk_api_for_file(&caller).is_some(),
                fault.starts_with("valid"),
                "{fault}"
            );
            assert!(
                !model
                    .compiler_source_views()
                    .iter()
                    .any(|view| view.suffix == "kt"),
                "SDK inputs do not authorize compiler execution"
            );
            let task = model.modules[0].source_sets[0]
                .compiler_tasks
                .iter()
                .find(|task| task.language == CompilerLanguage::Kotlin)
                .unwrap();
            assert!(!task.complete && task.problem.is_some() && task.java_home.is_none());
            assert_eq!(task.arguments, ["original incomplete compiler arguments"]);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn caller_sdk_failure_in_another_language_keeps_proved_sdk_inputs() {
        let (root, modules) = sdk_selection_fixture();
        for (language, suffix) in [
            (CompilerLanguage::Java, "java"),
            (CompilerLanguage::Kotlin, "kt"),
        ] {
            let mut module = modules[0].clone();
            let source = &mut module.source_sets[0];
            source.compiler_metadata_incomplete = true;
            let failed = source
                .compiler_tasks
                .iter_mut()
                .find(|task| task.language != language)
                .unwrap();
            failed.complete = false;
            failed.problem = Some("unrelated compiler classpath resolution failed".into());
            failed.java_home = None;
            let caller = source.source_roots[0].join(format!("Use.{suffix}"));
            let future = source.source_roots[0].join(format!("New.{suffix}"));
            let model = ProjectSnapshot::new(vec![module.clone()]);
            assert!(
                model.sdk_api_for_file(&caller).is_some(),
                "{suffix}: the other language's missing classpath is not this caller's SDK"
            );
            assert!(model.sdk_api_for_file(&future).is_some());
            for fault in ["claim", "policy", "own-task", "duplicate"] {
                let mut changed = module.clone();
                let source = &mut changed.source_sets[0];
                let task = source
                    .compiler_tasks
                    .iter_mut()
                    .find(|task| task.language == language)
                    .unwrap();
                match fault {
                    "claim" => {
                        task.options.get_mut("sdkProducerClaims").unwrap()["complete"] =
                            false.into();
                    }
                    "policy" => {
                        task.options
                            .insert("sdkSourcePolicy".into(), serde_json::Value::Null);
                    }
                    "own-task" => task.complete = false,
                    "duplicate" => {
                        let duplicate = task.clone();
                        source.compiler_tasks.push(duplicate);
                    }
                    _ => unreachable!(),
                }
                let model = ProjectSnapshot::new(vec![changed]);
                assert!(
                    model.sdk_api_for_file(&caller).is_none(),
                    "{suffix}/{fault}"
                );
                assert!(
                    model.sdk_api_for_file(&future).is_none(),
                    "{suffix}/{fault}"
                );
            }
            assert!(model
                .sdk_api_for_file(&root.join(format!("unclaimed/Use.{suffix}")))
                .is_none());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "requires the unmodified captured Gradle module and its selected SDK"]
    fn captured_gradle_complete_kotlin_task_keeps_its_atomic_sdk() {
        let captured = PathBuf::from(std::env::var_os("KTLSP_SDK_CAPTURED_MODULE").unwrap());
        let module: ModuleClasspath =
            serde_json::from_slice(&std::fs::read(captured).unwrap()).unwrap();
        let caller = PathBuf::from(std::env::var_os("KTLSP_SDK_CAPTURED_FILE").unwrap());
        let source = module
            .source_sets
            .iter()
            .find(|source| source.name == "main")
            .unwrap();
        assert!(source.compiler_metadata_incomplete);
        let kotlin = sdk_producing_task(source, &CompilerLanguage::Kotlin).unwrap();
        assert!(kotlin
            .arguments
            .iter()
            .any(|argument| argument == "-Xjdk-release=1.8"));
        assert!(source
            .compiler_tasks
            .iter()
            .any(|task| task.language == CompilerLanguage::Java
                && !task.complete
                && task.problem.is_some()));
        let model = ProjectSnapshot::new(vec![module]);
        let selection = model
            .sdk_api_for_file(&caller)
            .expect("complete Kotlin producing task must retain its independently proved SDK API");
        assert_eq!(selection.release, Some(8));
        assert!(selection
            .entries
            .get("java/util/concurrent/atomic/AtomicInteger")
            .is_some_and(Option::is_some));
    }

    #[test]
    fn caller_sdk_overlap_keeps_ineligible_producer_claims() {
        let (root, modules) = sdk_selection_fixture();
        let first = modules[0].clone();
        let source_root = first.source_sets[0].source_roots[0].clone();
        for (language, suffix) in [
            (CompilerLanguage::Java, "java"),
            (CompilerLanguage::Kotlin, "kt"),
        ] {
            let existing = source_root.join(format!("Use.{suffix}"));
            let future = source_root.join(format!("New.{suffix}"));
            let other_suffix = if suffix == "java" { "kt" } else { "java" };
            for failure in [
                "unsupported-sdk",
                "null-policy",
                "incomplete-roots",
                "incomplete-compiler",
                "ambiguous-producers",
            ] {
                let mut second = first.clone();
                second.module.push_str("-other-producer");
                second.source_sets[0]
                    .compiler_tasks
                    .retain(|task| task.language == language);
                let task = &mut second.source_sets[0].compiler_tasks[0];
                match failure {
                    "unsupported-sdk" if language == CompilerLanguage::Java => {
                        task.options
                            .insert("release".into(), serde_json::json!(999));
                    }
                    "unsupported-sdk" => task.arguments.push("-Xjdk-release=999".into()),
                    "null-policy" => {
                        task.options
                            .insert("sdkSourcePolicy".into(), serde_json::Value::Null);
                    }
                    "incomplete-roots" => {
                        task.options
                            .insert("sdkProducerClaims".into(), serde_json::Value::Null);
                    }
                    "incomplete-compiler" => task.complete = false,
                    "ambiguous-producers" => {}
                    _ => unreachable!(),
                }
                if failure == "ambiguous-producers" {
                    let duplicate = task.clone();
                    second.source_sets[0].compiler_tasks.push(duplicate);
                }
                let model = ProjectSnapshot::new(vec![first.clone(), second]);
                assert!(
                    model.sdk_api_for_file(&existing).is_none(),
                    "{suffix}/{failure}: current exact overlap"
                );
                assert!(
                    model.sdk_api_for_file(&future).is_none(),
                    "{suffix}/{failure}: future overlap"
                );
                assert!(
                    model
                        .sdk_api_for_file(&source_root.join(format!("New.{other_suffix}")))
                        .is_some(),
                    "{suffix}/{failure}: another language is not this producer"
                );
            }
        }
        for (language, suffix) in [
            (CompilerLanguage::Java, "java"),
            (CompilerLanguage::Kotlin, "kt"),
        ] {
            let mut same_source_set = first.clone();
            let mut unsupported = same_source_set.source_sets[0]
                .compiler_tasks
                .iter()
                .find(|task| task.language == language)
                .unwrap()
                .clone();
            if language == CompilerLanguage::Java {
                unsupported.task.push_str("Other");
                unsupported
                    .options
                    .insert("release".into(), serde_json::json!(999));
            } else {
                // This is a duplicate of the publicly selected producing task, not a KAPT consumer.
                unsupported.arguments.push("-Xjdk-release=999".into());
            }
            same_source_set.source_sets[0]
                .compiler_tasks
                .push(unsupported);
            let model = ProjectSnapshot::new(vec![same_source_set]);
            for name in [format!("Use.{suffix}"), format!("New.{suffix}")] {
                assert!(
                    model.sdk_api_for_file(&source_root.join(name)).is_none(),
                    "same SourceSet must reject multiple actual producers"
                );
            }
            let other = if suffix == "java" { "kt" } else { "java" };
            assert!(model
                .sdk_api_for_file(&source_root.join(format!("New.{other}")))
                .is_some());
        }
        let with_auxiliary = first.clone();
        let mut auxiliary = with_auxiliary.source_sets[0].compiler_tasks[1].clone();
        auxiliary.task = ":kaptGenerateStubsKotlin".into();
        auxiliary.task_type = "org.jetbrains.kotlin.gradle.internal.KaptGenerateStubsTask".into();
        auxiliary
            .options
            .insert("sdkSourcePolicy".into(), serde_json::Value::Null);
        auxiliary
            .options
            .insert("sdkProducerClaims".into(), serde_json::Value::Null);
        let mut other = first.clone();
        other.module.push_str("-nonproducer");
        other.source_sets[0].compiler_tasks = vec![auxiliary];
        other.source_sets[0].kotlin_compile_task = None;
        let model = ProjectSnapshot::new(vec![with_auxiliary, other]);
        for suffix in ["java", "kt"] {
            assert!(
                model
                    .sdk_api_for_file(&source_root.join(format!("New.{suffix}")))
                    .is_some(),
                "unselected KAPT task and Kotlin-read Java headers cannot veto {suffix}"
            );
        }
        let mut headers = first.clone();
        headers.module.push_str("-kotlin-header-consumer");
        headers.source_sets[0]
            .compiler_tasks
            .retain(|task| task.language == CompilerLanguage::Kotlin);
        let task = &mut headers.source_sets[0].compiler_tasks[0];
        task.source_files = vec![source_root.join("Use.java")];
        task.options
            .insert("sdkSourcePolicy".into(), serde_json::Value::Null);
        let headers = ProjectSnapshot::new(vec![first, headers]);
        for file in ["Use.java", "New.java"] {
            assert!(
                headers.sdk_api_for_file(&source_root.join(file)).is_some(),
                "a Kotlin Java-header input cannot claim Java production"
            );
        }
        for unrelated in [false, true] {
            for proof in ["known-empty", "missing", "opaque", "incomplete-compiler"] {
                let first = modules[0].clone();
                let mut unknown = first.source_sets[0].clone();
                unknown.name = "unproved-inputs".into();
                unknown.source_roots = vec![root.join("other-declared-root")];
                unknown
                    .compiler_tasks
                    .retain(|task| task.language == CompilerLanguage::Java);
                unknown.kotlin_compile_task = None;
                let task = &mut unknown.compiler_tasks[0];
                task.task = ":compileUnknownJava".into();
                task.source_files.clear();
                task.options
                    .insert("sdkProducerClaims".into(), serde_json::Value::Null);
                task.options
                    .insert("sdkSourcePolicy".into(), serde_json::Value::Null);
                if proof == "known-empty" || proof == "opaque" {
                    task.options.insert(
                        "sdkProducerClaims".into(),
                        serde_json::json!({
                            "adapter":"gradle-producing-source-trees-v1", "task":task.task,
                            "language":task.language, "roots":[], "complete":proof == "known-empty",
                        }),
                    );
                }
                if proof == "incomplete-compiler" {
                    task.complete = false;
                }
                let model = if unrelated {
                    let mut other = first.clone();
                    other.module.push_str("-unrelated");
                    other.source_sets = vec![unknown];
                    ProjectSnapshot::new(vec![first, other])
                } else {
                    let mut local = first;
                    local.source_sets.push(unknown);
                    ProjectSnapshot::new(vec![local])
                };
                assert_eq!(
                    model
                        .sdk_api_for_file(&source_root.join("Use.java"))
                        .is_some(),
                    unrelated || proof == "known-empty",
                    "absent source_files is not an independently complete empty inventory: {proof}"
                );
                assert_eq!(model.sdk_api_for_file(&source_root.join("New.java")).is_some(), unrelated || proof == "known-empty",
                "empty current inputs do not prove disjoint future ownership within the same modeled module");
                assert!(
                    model
                        .sdk_api_for_file(&source_root.join("New.kt"))
                        .is_some(),
                    "Java producer uncertainty does not veto Kotlin"
                );
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selected_sdk_legacy_root_source_archive_uses_its_own_home_and_revisions() {
        let (root, modules) = sdk_selection_fixture();
        let home = root.join("jdk8");
        std::fs::create_dir_all(home.join("jre/lib")).unwrap();
        let source = home.join("src.zip");
        std::fs::write(&source, "selected legacy SDK archive").unwrap();
        std::fs::write(home.join("release"), "JAVA_VERSION=\"1.8.0_352\"\n").unwrap();
        std::fs::write(home.join("jre/lib/rt.jar"), "selected legacy runtime").unwrap();
        let mut module = modules[0].clone();
        module.source_sets[0].compiler_tasks.truncate(1);
        module.source_sets[0].kotlin_compile_task = None;
        let task = &mut module.source_sets[0].compiler_tasks[0];
        task.java_home = Some(home.clone());
        task.options.get_mut("javaCompilerRecipe").unwrap()["language_version"] = "8".into();
        let model = ProjectSnapshot::new(vec![module.clone()]);
        assert_eq!(model.selected_jdk_sources(), [source.clone()]);
        assert_eq!(
            crate::deps::jdk_home_for_source_archive(&source),
            Some(home.as_path())
        );
        assert_eq!(
            model.selected_jdk_source_for_file(
                &module.source_sets[0].source_roots[0].join("Use.java")
            ),
            Some(source.clone())
        );
        let origin = jdk_origin_id(&source);
        let revisions = &model.artifact_revisions()[&origin];
        assert!(revisions.contains(&file_origin_id(&home.join("release"))));
        assert!(revisions.contains(&file_origin_id(&home.join("jre/lib/rt.jar"))));
        std::fs::write(home.join("release"), "JAVA_VERSION=\"1.8.0_353\"\n").unwrap();
        let updated = ProjectSnapshot::new(vec![module]);
        assert_ne!(
            model.artifact_revisions()[&origin],
            updated.artifact_revisions()[&origin]
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selected_sdk_sources_are_scoped_and_same_feature_release_is_accepted() {
        let (root, modules) = sdk_selection_fixture();
        let model = ProjectSnapshot::new(modules.clone());
        assert_eq!(model.selected_jdk_sources().len(), 2);
        let visibilities = model.scope_visibilities(&["jdk:/unrelated/host/lib/src.zip".into()]);
        for (module, visibility) in modules.iter().zip(visibilities) {
            let source = module.source_sets[0].compiler_tasks[0]
                .java_home
                .as_ref()
                .unwrap()
                .join("lib/src.zip");
            let sdks = visibility
                .ordered_origins
                .iter()
                .filter(|origin| origin.starts_with("jdk:"))
                .collect::<Vec<_>>();
            assert!(
                sdks.is_empty(),
                "raw source-set scope cannot choose a caller API"
            );
            let callers =
                model.caller_scope_visibilities(&["jdk:/unrelated/host/lib/src.zip".into()]);
            let own_scope = project_scope(module, &module.source_sets[0]);
            let selected: Vec<_> = callers
                .iter()
                .filter(|view| view.visibility.scope == own_scope)
                .collect();
            assert_eq!(
                selected.len(),
                2,
                "validated Java and Kotlin producing tasks"
            );
            for view in selected {
                let sdks: Vec<_> = view
                    .visibility
                    .ordered_origins
                    .iter()
                    .filter(|origin| is_sdk_origin(origin))
                    .collect();
                assert_eq!(sdks, [&jdk_origin_id(&source)]);
            }
            for name in ["Unsaved.java", "Unsaved.kt"] {
                assert_eq!(
                    model.selected_jdk_source_for_file(
                        &module.source_sets[0].source_roots[0].join(name)
                    ),
                    Some(source.clone())
                );
            }
        }
        let mut cem = modules[1].clone();
        cem.source_sets[0].compiler_tasks[0]
            .options
            .insert("release".into(), serde_json::Value::Null);
        assert_eq!(
            ProjectSnapshot::new(vec![cem]).selected_jdk_sources().len(),
            1,
            "CEM21 null release remains selected"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selected_sdk_sources_refuse_cross_feature_ambiguous_or_unverified_inputs() {
        let (root, modules) = sdk_selection_fixture();
        let mut source = modules[1].source_sets[0].clone();
        // This case adds its own selected Kotlin producer below.
        source.compiler_tasks.truncate(1);
        source.kotlin_compile_task = None;
        for field in ["release", "javaCompilerRecipe"] {
            let mut changed = source.clone();
            if field == "release" {
                changed.compiler_tasks[0]
                    .options
                    .insert(field.into(), serde_json::json!(17));
            } else {
                changed.compiler_tasks[0].options.get_mut(field).unwrap()["language_version"] =
                    "17".into();
            }
            assert!(
                source_set_jdk_source(&changed, &mut BTreeMap::new()).is_none(),
                "{field}"
            );
        }
        source.compiler_tasks[0].arguments = vec!["--release=17".into()];
        assert!(source_set_jdk_source(&source, &mut BTreeMap::new()).is_none());
        source.compiler_tasks[0].arguments = vec!["--release=21".into()];
        assert!(source_set_jdk_source(&source, &mut BTreeMap::new()).is_some());
        for argument in [
            "-Xbootclasspath/p:/foreign-sdk",
            "--system=/foreign-sdk",
            "@opaque.args",
        ] {
            source.compiler_tasks[0].arguments = vec![argument.into()];
            assert!(source_set_jdk_source(&source, &mut BTreeMap::new()).is_none());
        }
        source.compiler_tasks[0].arguments = vec!["--release=21".into()];
        for options in [
            serde_json::json!(["--system", "/foreign-sdk"]),
            serde_json::json!(["--release=17"]),
            serde_json::json!(["-Xbootclasspath/p:/foreign-sdk"]),
            serde_json::json!({}),
        ] {
            let mut changed = source.clone();
            changed.compiler_tasks[0]
                .options
                .get_mut("javaCompilerRecipe")
                .unwrap()["api_options"] = options;
            assert!(
                source_set_jdk_source(&changed, &mut BTreeMap::new()).is_none(),
                "effective compiler options cannot select foreign SDK APIs"
            );
        }
        let mut dangling = source.clone();
        dangling.compiler_tasks[0].arguments = vec!["--release".into()];
        dangling.compiler_tasks[0]
            .options
            .get_mut("javaCompilerRecipe")
            .unwrap()["api_options"] = serde_json::json!(["21"]);
        assert!(
            source_set_jdk_source(&dangling, &mut BTreeMap::new()).is_none(),
            "independent argument streams cannot complete each other's dangling options"
        );
        let mut kotlin = source.compiler_tasks[0].clone();
        kotlin.language = CompilerLanguage::Kotlin;
        kotlin.task = ":compileKotlin".into();
        kotlin.task_type = "org.jetbrains.kotlin.gradle.tasks.KotlinCompile".into();
        kotlin.arguments.clear();
        kotlin.java_home = modules[0].source_sets[0].compiler_tasks[0]
            .java_home
            .clone();
        source.kotlin_compile_task = Some(kotlin.task.clone());
        source.compiler_tasks.push(kotlin);
        assert!(
            source_set_jdk_source(&source, &mut BTreeMap::new()).is_none(),
            "two homes in one source set cannot use a first winner"
        );
        source.compiler_tasks[1].java_home = source.compiler_tasks[0].java_home.clone();
        assert!(source_set_jdk_source(&source, &mut BTreeMap::new()).is_some());
        for arguments in [
            vec!["@opaque.args".into()],
            vec!["-Xbuild-file=opaque.xml".into()],
            vec!["-module=opaque.xml".into()],
        ] {
            source.compiler_tasks[1].arguments = arguments;
            assert!(source_set_jdk_source(&source, &mut BTreeMap::new()).is_none());
        }
        source.compiler_tasks.pop();
        source.kotlin_compile_task = None;
        source.compiler_metadata_incomplete = true;
        assert!(source_set_jdk_source(&source, &mut BTreeMap::new()).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn caller_sdk_api_separates_release8_java_from_target8_kotlin() {
        use crate::workspace::Workspace;
        use std::io::Write;
        let (root, modules) = sdk_selection_fixture();
        let mut module = modules[0].clone();
        let source_root = module.source_sets[0].source_roots[0].clone();
        let java_path = source_root.join("Api8.java");
        let kotlin_path = source_root.join("Api17.kt");
        let java_key = crate::paths::file_key(&java_path);
        let kotlin_key = crate::paths::file_key(&kotlin_path);
        let source = &mut module.source_sets[0];
        source.compiler_tasks.truncate(1);
        let java = &mut source.compiler_tasks[0];
        let home = java.java_home.clone().unwrap();
        java.source_files = vec![java_path.clone()];
        java.classpath.clear();
        java.arguments = vec!["-proc:none".into()];
        java.options.insert("release".into(), serde_json::json!(8));
        java.options.get_mut("javaCompilerRecipe").unwrap()["api_options"] =
            serde_json::json!(["--release", "8", "-proc:none"]);
        let mut kotlin = java.clone();
        kotlin.language = CompilerLanguage::Kotlin;
        kotlin.task = ":compileKotlin".into();
        kotlin.task_type = "org.jetbrains.kotlin.gradle.tasks.KotlinCompile".into();
        // Kotlin reads Java headers too; that does not make it the Java producing task.
        kotlin.source_files = vec![kotlin_path.clone(), java_path.clone()];
        kotlin.arguments.clear();
        kotlin
            .options
            .insert("jvmTarget".into(), serde_json::json!("1.8"));
        source.kotlin_compile_task = Some(kotlin.task.clone());
        source.compiler_tasks.push(kotlin);
        for task in &mut source.compiler_tasks {
            task.options.insert("sdkSourcePolicy".into(), serde_json::json!({
                "adapter": "gradle-source-tree-stock-patterns-v1", "task": task.task,
                "language": task.language, "authored_roots": [source_root],
                "future_roots": [source_root], "generated_roots": [source_root.join("generated")],
                "default_excludes": sdk_source_default_excludes(),
            }));
            task.options.insert(
                "sdkProducerClaims".into(),
                serde_json::json!({
                    "adapter": "gradle-producing-source-trees-v1", "task": task.task,
                    "language": task.language, "roots": [source_root], "complete": true,
                }),
            );
        }
        let fixtures: &[(&str, &[u8])] = &[
            (
                "java/lang/Object",
                include_bytes!("../../ktcore/tests/fixtures/classfile/sdk17-release8/Object.sig"),
            ),
            (
                "java/lang/Number",
                include_bytes!("../../ktcore/tests/fixtures/classfile/sdk17-release8/Number.sig"),
            ),
            (
                "java/lang/String",
                include_bytes!("../../ktcore/tests/fixtures/classfile/sdk17-release8/String.sig"),
            ),
            (
                "java/util/concurrent/atomic/AtomicInteger",
                include_bytes!(
                    "../../ktcore/tests/fixtures/classfile/sdk17-release8/AtomicInteger.sig"
                ),
            ),
            (
                "java/io/Serializable",
                include_bytes!(
                    "../../ktcore/tests/fixtures/classfile/sdk17-release8/Serializable.sig"
                ),
            ),
            (
                "java/lang/Comparable",
                include_bytes!(
                    "../../ktcore/tests/fixtures/classfile/sdk17-release8/Comparable.sig"
                ),
            ),
            (
                "java/lang/CharSequence",
                include_bytes!(
                    "../../ktcore/tests/fixtures/classfile/sdk17-release8/CharSequence.sig"
                ),
            ),
        ];
        let mut signatures =
            zip::ZipWriter::new(std::fs::File::create(home.join("lib/ct.sym")).unwrap());
        for (owner, bytes) in fixtures {
            signatures
                .start_file(
                    format!("8/java.base/{owner}.sig"),
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            signatures.write_all(bytes).unwrap();
        }
        signatures.finish().unwrap();
        let mut sources =
            zip::ZipWriter::new(std::fs::File::create(home.join("lib/src.zip")).unwrap());
        let string_source = "package java.lang; public final class String { public String trim(){return this;} public String strip(){return this;} public int length(){return 0;} }";
        sources
            .start_file(
                "java.base/java/lang/String.java",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        sources.write_all(string_source.as_bytes()).unwrap();
        sources.finish().unwrap();
        let model = Arc::new(ProjectSnapshot::new(vec![module.clone()]));
        let java_api = model.sdk_api_for_file(&java_path).unwrap();
        let kotlin_api = model.sdk_api_for_file(&kotlin_path).unwrap();
        assert!(std::ptr::eq(
            model.extension_project_context(&java_path).unwrap().sdk.unwrap(),
            java_api
        ));
        assert!(std::ptr::eq(
            model.extension_project_context(&kotlin_path).unwrap().sdk.unwrap(),
            kotlin_api
        ));
        assert_eq!(kotlin_api.release, None);
        assert_eq!(java_api.release, Some(8));
        let new_java_path = source_root.join("NewApi8.java");
        let new_kotlin_path = source_root.join("NewApi17.kt");
        assert_eq!(
            model.sdk_api_for_file(&new_java_path).unwrap().release,
            Some(8)
        );
        assert_eq!(
            model.sdk_api_for_file(&new_kotlin_path).unwrap().release,
            None
        );
        for unavailable in [
            source_root.join("generated/Kapt.java"),
            source_root.join("generated/Ksp.kt"),
            source_root.join("Script.kts"),
        ] {
            assert!(
                model.sdk_api_for_file(&unavailable).is_none(),
                "{unavailable:?}"
            );
        }
        assert!(
            model.selected_jdk_source_for_file(&java_path).is_none(),
            "SDK17 source cannot seed Java API8"
        );
        assert_eq!(
            model.selected_jdk_source_for_file(&kotlin_path),
            kotlin_api.source_zip
        );
        assert_eq!(
            kotlin_api.release, None,
            "jvmTarget alone does not restrict SDK APIs"
        );
        assert_eq!(java_api.source_zip, kotlin_api.source_zip);
        assert_ne!(java_api.origin(), kotlin_api.origin());
        assert!(model.artifact_revisions()[&java_api.origin()].contains(&java_api.identity()));
        let mut ws = Workspace::new();
        ws.set_project_model(model.clone());
        let indexed = crate::sdk_api::index(java_api, &root.join("extract")).unwrap();
        assert!(
            indexed.complete,
            "selected portable ct.sym subset must have useful declaration coverage"
        );
        assert!(!indexed.batches.is_empty());
        for batch in indexed.batches {
            assert!(ws.index_dependency_batch(batch));
        }
        let full_sources = crate::deps::resolve_jdk_sources(
            &home.join("lib/src.zip"),
            &root.join("extract"),
            &mut crate::parser::KotlinParser::new(),
            &mut crate::java::JavaParser::new(),
        );
        for batch in full_sources {
            assert!(ws.index_dependency_batch(batch));
        }
        let java_text = "class Api8 { int value(java.util.concurrent.atomic.AtomicInteger i, String s) { i.incrementAndGet(); s.trim(); s.strip(); return 0; } }";
        let kotlin_text = "fun value(i: java.util.concurrent.atomic.AtomicInteger, s: java.lang.String): String = s.strip()";
        ws.open(&java_key, java_text.into());
        ws.open(&kotlin_key, kotlin_text.into());
        let new_java_key = crate::paths::file_key(&new_java_path);
        let new_kotlin_key = crate::paths::file_key(&new_kotlin_path);
        // Neither new unsaved file occurs in model.source_files; root policy must suffice.
        ws.open(&new_java_key, java_text.into());
        ws.open(&new_kotlin_key, kotlin_text.into());
        ws.assume_index_complete_for_tests();
        for (file, text, prefix, expected, stub) in [
            (&java_key, java_text, "i.inc", "incrementAndGet", true),
            (&java_key, java_text, "s.tr", "trim", true),
            (&kotlin_key, kotlin_text, "s.str", "strip", false),
            (&new_java_key, java_text, "i.inc", "incrementAndGet", true),
            (&new_java_key, java_text, "s.tr", "trim", true),
            (&new_kotlin_key, kotlin_text, "s.str", "strip", false),
        ] {
            let offset = text.find(prefix).unwrap() + prefix.len();
            let completion = ws
                .complete(file, offset, true)
                .expect("useful SDK completion");
            assert!(
                completion.items.iter().any(|item| item.label == expected),
                "{file}: {completion:?}"
            );
            let start = text.find(&format!("{expected}(")).unwrap();
            let definitions = ws.goto_definition(file, start);
            assert_eq!(definitions.len(), 1, "{file}: {definitions:?}");
            assert_eq!(
                definitions[0].file.replace('\\', "/").contains("/jdk-api/"),
                stub,
                "{definitions:?}"
            );
            // SDK API declarations are eagerly written; only archive-backed sources are lazy.
            if !stub {
                assert!(crate::deps::materialize_source_file(Path::new(
                    &definitions[0].file
                )));
            }
            assert!(std::fs::read_to_string(&definitions[0].file).unwrap()
                [definitions[0].start_byte..definitions[0].end_byte]
                .starts_with(expected));
        }
        let strip_offset = java_text.find("s.strip").unwrap() + "s.str".len();
        assert!(!ws
            .complete(&java_key, strip_offset, true)
            .is_some_and(|result| result.items.iter().any(|item| item.label == "strip")));
        assert!(
            ws.diagnostics(&java_key)
                .iter()
                .any(|diagnostic| diagnostic.message.contains("strip")),
            "Java release8 must reject strip"
        );
        assert!(
            ws.diagnostics(&kotlin_key).is_empty(),
            "SDK17 Kotlin strip is valid"
        );
        let mut restricted = module.clone();
        restricted.source_sets[0].compiler_tasks[1]
            .arguments
            .push("-Xjdk-release=8".into());
        ws.set_project_model(Arc::new(ProjectSnapshot::new(vec![restricted])));
        let offset = kotlin_text.find("s.str").unwrap() + "s.str".len();
        assert!(!ws
            .complete(&kotlin_key, offset, true)
            .is_some_and(|result| result.items.iter().any(|item| item.label == "strip")));
        ws.set_project_model(model);
        assert!(
            !ws.complete(&kotlin_key, offset, true)
                .is_some_and(|result| result.items.iter().any(|item| item.label == "strip")),
            "restoring model metadata cannot resurrect retired SDK source facts"
        );
        let generation = ws.analysis_generation();
        let restored = crate::deps::resolve_jdk_sources(
            &home.join("lib/src.zip"),
            &root.join("extract"),
            &mut crate::parser::KotlinParser::new(),
            &mut crate::java::JavaParser::new(),
        );
        assert!(
            !restored.is_empty(),
            "selected SDK source inventory must be republished"
        );
        for batch in restored {
            assert_eq!(
                batch.origin.as_ref().unwrap().origin_id,
                jdk_origin_id(&home.join("lib/src.zip"))
            );
            assert!(ws.index_dependency_batch_for_generation(generation, batch));
        }
        assert!(ws
            .complete(&kotlin_key, offset, true)
            .unwrap()
            .items
            .iter()
            .any(|item| item.label == "strip"));
        let mut ambiguous = module.clone();
        let second_java = ambiguous.source_sets[0].compiler_tasks[0].clone();
        ambiguous.source_sets[0].compiler_tasks.push(second_java);
        assert!(ProjectSnapshot::new(vec![ambiguous])
            .sdk_api_for_file(&java_path)
            .is_none());
        let mut overlapping = module.clone();
        overlapping.module.push_str("-other-owner");
        let overlapping = ProjectSnapshot::new(vec![module.clone(), overlapping]);
        assert!(overlapping.sdk_api_for_file(&java_path).is_none());
        assert!(overlapping.sdk_api_for_file(&new_java_path).is_none());
        let mut opaque = module.clone();
        opaque.source_sets[0].compiler_tasks[0]
            .options
            .get_mut("sdkSourcePolicy")
            .unwrap()["future_roots"] = serde_json::json!([]);
        let opaque = ProjectSnapshot::new(vec![opaque]);
        assert!(
            opaque.sdk_api_for_file(&java_path).is_some(),
            "exact existing claim survives opaque future policy"
        );
        assert!(
            opaque.sdk_api_for_file(&new_java_path).is_none(),
            "opaque task filters must not admit new Java files"
        );
        // Actual Gradle 8.14.2 / KGP 2.0.21 metadata uses release=1.8 and target=1.8.
        // The selected compiler independently emits identical AtomicKt bytes for release 8/1.8.
        let atomic_text = "fun atomic(i: java.util.concurrent.atomic.AtomicInteger, s: java.lang.String): Int { s.strip(); s.incrementAndGet(); return i.incrementAndGet() }";
        // The retained Kotlin 2.0.21 / SDK17 oracle also accepts two numeric 8 flags
        // with -Werror and emits the identical AtomicKt class. Keep authored order/count.
        for (release, release_arguments) in [
            ("8", vec!["-Xjdk-release=8"]),
            ("1.8", vec!["-Xjdk-release=1.8"]),
            ("duplicate8", vec!["-Xjdk-release=8", "-Xjdk-release=8"]),
        ] {
            let mut legacy = module.clone();
            let arguments: Vec<String> = ["-jvm-target", "1.8"]
                .into_iter()
                .chain(release_arguments)
                .map(str::to_owned)
                .collect();
            legacy.source_sets[0].compiler_tasks[1].arguments = arguments.clone();
            let legacy_model = Arc::new(ProjectSnapshot::new(vec![legacy.clone()]));
            let selection = legacy_model
                .sdk_api_for_file(&kotlin_path)
                .unwrap_or_else(|| {
                    panic!("valid Kotlin release {release} needs its API inventory")
                });
            assert_eq!(selection.release, Some(8));
            assert_eq!(
                legacy.source_sets[0].compiler_tasks[1].arguments, arguments,
                "API selection must not rewrite the captured invocation"
            );
            assert!(selection
                .entries
                .get("java/util/concurrent/atomic/AtomicInteger")
                .is_some_and(Option::is_some));
            let projected = crate::sdk_api::index(selection, &root.join("extract")).unwrap();
            ws.set_project_model(legacy_model);
            for batch in projected.batches {
                assert!(ws.index_dependency_batch(batch));
            }
            ws.open(&kotlin_key, atomic_text.into());
            let offset = atomic_text.find("i.inc").unwrap() + "i.inc".len();
            assert!(
                ws.complete(&kotlin_key, offset, true)
                    .is_some_and(|result| result
                        .items
                        .iter()
                        .any(|item| item.label == "incrementAndGet")),
                "release {release}: actual AtomicInteger API must remain useful"
            );
            let definition =
                ws.goto_definition(&kotlin_key, atomic_text.rfind("incrementAndGet").unwrap());
            assert_eq!(definition.len(), 1, "release {release}: {definition:?}");
            assert!(definition[0].file.replace('\\', "/").contains("/jdk-api/"));
            for prefix in ["s.str", "s.inc"] {
                let offset = atomic_text.find(prefix).unwrap() + prefix.len();
                assert!(
                    !ws.complete(&kotlin_key, offset, true)
                        .is_some_and(|result| result.items.iter().any(|item| item.label
                            == if prefix == "s.str" {
                                "strip"
                            } else {
                                "incrementAndGet"
                            })),
                    "release {release}: {prefix}"
                );
            }
            legacy.source_sets[0].compiler_tasks[1]
                .options
                .insert("sdkSourcePolicy".into(), serde_json::Value::Null);
            assert!(
                ProjectSnapshot::new(vec![legacy])
                    .sdk_api_for_file(&kotlin_path)
                    .is_none(),
                "release normalization must not invent the actual96 missing source policy"
            );
        }
        for arguments in [
            vec!["-Xjdk-release=bogus"],
            vec!["-Xjdk-release=1.9"],
            vec!["-Xjdk-release=999"],
            vec!["-Xjdk-release=8", "-Xjdk-release=bogus"],
            vec!["-Xjdk-release=999", "-Xjdk-release=999"],
            vec!["-Xjdk-release=8", "-Xjdk-release"],
            vec!["-Xjdk-release=8", "-Xjdk-release=17"],
            vec!["-jvm-target", "17", "-Xjdk-release=8", "-Xjdk-release=8"],
            vec!["-jvm-target", "17", "-Xjdk-release=8"],
        ] {
            let mut unsupported = module.clone();
            unsupported.source_sets[0].compiler_tasks[1].arguments = arguments
                .iter()
                .map(|argument| (*argument).into())
                .collect();
            assert!(
                ProjectSnapshot::new(vec![unsupported])
                    .sdk_api_for_file(&kotlin_path)
                    .is_none(),
                "unsupported/ambiguous Kotlin API policy: {arguments:?}"
            );
        }
        let mut unsupported = module;
        unsupported.source_sets[0].compiler_tasks[0]
            .options
            .insert("release".into(), serde_json::json!(999));
        assert!(ProjectSnapshot::new(vec![unsupported])
            .sdk_api_for_file(&java_path)
            .is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "actual captured SourceSet/SDK oracle; requires KTLSP_SDK_CAPTURED_SOURCE_SET"]
    fn captured_gradle_kotlin_legacy_release_selects_atomic_signature() {
        use sha2::Digest;
        use std::io::Read;
        let captured = PathBuf::from(std::env::var_os("KTLSP_SDK_CAPTURED_SOURCE_SET").unwrap());
        let source: SourceSetClasspath =
            serde_json::from_slice(&std::fs::read(captured).unwrap()).unwrap();
        let task = sdk_producing_task(&source, &CompilerLanguage::Kotlin).unwrap();
        assert!(task
            .arguments
            .iter()
            .any(|argument| argument == "-Xjdk-release=1.8"));
        assert_eq!(task.source_files.len(), 12);
        assert!(task.options.get("sdkSourcePolicy").unwrap().is_null());
        assert!(
            sdk_source_policy(task, &source, "kt").is_none(),
            "the actual96 null policy is a separate caller-admission gap"
        );
        let selection = source_set_sdk_api(
            &source,
            CompilerLanguage::Kotlin,
            &mut BTreeMap::new(),
            &mut BTreeMap::new(),
        )
        .expect("compiler-valid actual release1.8 needs the selected API8 inventory");
        assert_eq!(selection.release, Some(8));
        let name = selection
            .entries
            .get("java/util/concurrent/atomic/AtomicInteger")
            .and_then(Option::as_ref)
            .expect("AtomicInteger belongs to Java8");
        let bytes = std::fs::read(&selection.archive).unwrap();
        assert_eq!(
            format!("{:x}", sha2::Sha256::digest(&bytes)),
            selection.archive_sha256
        );
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut entry = archive.by_name(name).unwrap();
        assert!(entry.size() <= ktcore::classfile::MAX_CLASS_FILE_BYTES as u64);
        let mut signature = Vec::new();
        entry.read_to_end(&mut signature).unwrap();
        let facts = ktcore::classfile::decode(&selection.origin(), &signature).unwrap();
        assert_eq!(
            facts.identity.internal_name.as_str(),
            "java/util/concurrent/atomic/AtomicInteger"
        );
        assert!(facts
            .methods
            .iter()
            .any(|method| method.name.as_ref() == "incrementAndGet"
                && method.descriptor.as_ref() == "()I"
                && method.access_flags & 1 == 1));
        let (declaration, _) =
            crate::jar::selected_sdk_class_stub_facts(&selection, &facts).unwrap();
        assert!(declaration.contains("incrementAndGet("));
    }

    #[test]
    fn java_recipe_jdk21_utf8_requires_exact_adapter_language_and_charset() {
        let mut java = plain_java_configuration();
        let mut recipe = selected_java_recipe();
        recipe["adapter"] = serde_json::json!("gradle-9.6.1-current-jdk21-utf8-javac-api-v1");
        recipe["language_version"] = serde_json::json!("21");
        recipe["java_runtime_version"] = serde_json::json!("21.0.3+7-LTS-152");
        recipe["gradle_java_version"] = serde_json::json!("21.0.3");
        recipe["api_options"] =
            serde_json::json!(["-source", "21", "-target", "21", "-encoding", "UTF-8"]);
        java["options"]["javaCompilerRecipeProblem"] = serde_json::Value::Null;
        java["options"]["javaCompilerRecipe"] = recipe.clone();
        let task: CompilerTask = serde_json::from_value(java.clone()).unwrap();
        assert_eq!(task.java_compiler_recipe().unwrap().language_version, "21");
        for (field, value) in [
            ("adapter", "gradle-9.6.2-current-jdk21-utf8-javac-api-v1"),
            ("adapter", "gradle-9.6.1-current-jdk17-javac-api-v1"),
            ("language_version", "17"),
            ("language_version", "22"),
            ("effective_charset", "MS932"),
        ] {
            let mut wrong = recipe.clone();
            wrong[field] = serde_json::json!(value);
            java["options"]["javaCompilerRecipe"] = wrong;
            let task: CompilerTask = serde_json::from_value(java.clone()).unwrap();
            assert!(task.java_compiler_recipe().is_none(), "{field}={value}");
        }
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
    fn java_recipe_gradle_9_6_1_preserves_selected_defaults_and_effective_option_order() {
        let mut recipe = selected_java_recipe();
        recipe["adapter"] = serde_json::json!("gradle-9.6.1-current-jdk17-javac-api-v1");
        recipe["gradle_file_encoding"] = serde_json::json!("UTF-8");
        recipe["api_options"] = serde_json::json!([
            "-source",
            "17",
            "-target",
            "17",
            "-d",
            "/repo/classes",
            "-h",
            "/repo/headers",
            "-g",
            "-sourcepath",
            "",
            "-proc:none",
            "-s",
            "/repo/generated",
            "-XDuseUnsharedTable=true",
            "-classpath",
            "/repo/z.jar;/repo/a.jar"
        ]);
        let mut java = plain_java_configuration();
        java["options"]["javaCompilerRecipe"] = recipe.clone();
        java["options"]["javaCompilerRecipeProblem"] = serde_json::Value::Null;
        let model = plain_java_snapshot(&[java]);
        let entry = CacheEntry {
            model_version: MODEL_VERSION,
            mtimes: BTreeMap::new(),
            modules: model.modules,
        };
        let decoded: CacheEntry =
            serde_json::from_slice(&serde_json::to_vec(&entry).unwrap()).unwrap();
        let selected = decoded.modules[0].source_sets[0].compiler_tasks[0]
            .java_compiler_recipe()
            .unwrap();
        assert_eq!(serde_json::to_value(selected).unwrap(), recipe);
    }

    #[test]
    fn java_recipe_rejects_unknown_adapter_compiler_type_and_language_version() {
        for adapter in [
            "gradle-7.4.2-current-jdk17-javac-api-v1",
            "gradle-9.6.1-current-jdk17-javac-api-v1",
        ] {
            for (field, value) in [
                ("adapter", "gradle-9.6.2-current-jdk17-javac-api-v1"),
                ("compiler_type", "custom.JavaCompiler"),
                ("language_version", "21"),
            ] {
                let mut recipe = selected_java_recipe();
                recipe["adapter"] = serde_json::json!(adapter);
                recipe[field] = serde_json::json!(value);
                let mut java = plain_java_configuration();
                java["options"]["javaCompilerRecipe"] = recipe;
                let model = plain_java_snapshot(&[java]);
                let source = &model.modules[0].source_sets[0];
                assert!(!source.compiler_metadata_incomplete);
                assert!(source.compiler_tasks[0].java_compiler_recipe().is_none());
                assert!(model
                    .plain_java_task("/repo/app/src/main/java/Use.java")
                    .is_some());
            }
        }
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
    fn jvm_compilation_source_sets_preserve_mixed_ownership_and_dependency_visibility() {
        let kotlin = serde_json::json!({
            "task": ":mixed:compileKotlinJvm", "language": "kotlin", "task_type": "KotlinCompile",
            "arguments": [], "annotation_processor_path": [], "java_home": null,
            "source_files": ["/repo/mixed/src/commonMain/kotlin/Shared.kt", "/repo/mixed/src/jvmMain/kotlin/Jvm.kt", "/repo/mixed/src/jvmMain/java/Bridge.java"],
            "classpath": ["/repo/library.jar"], "options": {}, "output_roots": ["/repo/mixed/build/classes/kotlin/jvm/main"],
            "complete": false, "problem": "Compiler recipe is unavailable"
        });
        let java = serde_json::json!({
            "task": ":mixed:compileJvmMainJava", "language": "java", "task_type": "JavaCompile",
            "arguments": [], "annotation_processor_path": [], "java_home": null,
            "source_files": ["/repo/mixed/src/jvmMain/java/Bridge.java"], "classpath": ["/repo/library.jar"],
            "options": {}, "output_roots": ["/repo/mixed/build/classes/java/jvmMain"], "complete": true, "problem": null
        });
        let dump = format!(
            "PROJECT\t:consumer\t/repo/consumer\nBUILD\t:\nSOURCE\tmain\t/repo/consumer/src/main/java\n\
             SCP\tmain\t/repo/mixed/build/libs/mixed-jvm.jar\nARTIFACT\tmain\t/repo/mixed/build/libs/mixed-jvm.jar\tPROJECT\t:\t:mixed\nEND\t:consumer\n\
             PROJECT\t:mixed\t/repo/mixed\nBUILD\t:\nSOURCESET\tjvmMain\n\
             SOURCE\tjvmMain\t/repo/mixed/src/commonMain/kotlin\nSOURCE\tjvmMain\t/repo/mixed/src/jvmMain/kotlin\nSOURCE\tjvmMain\t/repo/mixed/src/jvmMain/java\n\
             OUTPUT\tjvmMain\t/repo/mixed/build/classes/kotlin/jvm/main\nOUTPUT\tjvmMain\t/repo/mixed/build/classes/java/jvmMain\nPROVIDES\tjvmMain\t/repo/mixed/build/libs/mixed-jvm.jar\n\
             KOTLIN_COMPILE_TASK\tjvmMain\t\":mixed:compileKotlinJvm\"\nCOMPILER\tjvmMain\t{java}\nCOMPILER\tjvmMain\t{kotlin}\nSCP\tjvmMain\t/repo/library.jar\n\
             SOURCESET\tjvmTest\nSOURCE\tjvmTest\t/repo/mixed/src/jvmTest/kotlin\nSCP\tjvmTest\t/repo/mixed/build/classes/kotlin/jvm/main\nEND\t:mixed\n"
        );
        let modules = parse_dump_reader(dump.as_bytes(), true).unwrap();
        let model = ProjectSnapshot::new(modules);
        for source in ["commonMain/kotlin/Shared.kt", "jvmMain/kotlin/Jvm.kt", "jvmMain/java/Bridge.java"] {
            let path = PathBuf::from(format!("/repo/mixed/src/{source}"));
            let (module, owner) = model.owner_for(&path).unwrap();
            assert_eq!(module.module, ":mixed");
            let owner = owner.unwrap();
            assert_eq!(owner.name, "jvmMain");
            assert_eq!(owner.kotlin_compile_task.as_deref(), Some(":mixed:compileKotlinJvm"));
            assert_eq!(owner.compiler_tasks.len(), 2);
            assert_eq!(owner.entries, [PathBuf::from("/repo/library.jar")]);
            assert!(!owner.compiler_tasks[1].complete, "unavailable compiler authority must remain explicit");
        }
        assert_eq!(model.file_origin(Path::new("/repo/mixed/src/jvmTest/kotlin/Test.kt")).unwrap().project_scope.unwrap().source_set, "jvmTest");
        let dependency = model.file_origin(Path::new("/repo/mixed/src/jvmMain/java/Bridge.java")).unwrap();
        let consumer = model.file_origin(Path::new("/repo/consumer/src/main/java/Use.java")).unwrap();
        assert!(model.scope_visibilities(&[]).iter().any(|view|
            Some(&view.scope) == consumer.project_scope.as_ref() && view.ordered_origins.contains(&dependency.origin_id)));
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
    fn source_queries_share_complete_content_addressed_init_scripts() {
        let directory = std::env::temp_dir().join(format!(
            "ktlsp-init-cache-{}-{}",
            std::process::id(),
            CACHE_WRITE_SEQUENCE.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let directory = directory.clone();
                std::thread::spawn(move || cached_dump_script(&directory, INIT_SCRIPT))
            })
            .collect();
        let scripts: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap().unwrap())
            .collect();
        assert_eq!(scripts.iter().collect::<BTreeSet<_>>().len(), 1);
        assert_eq!(std::fs::read(&scripts[0]).unwrap(), INIT_SCRIPT.as_bytes());
        assert_eq!(
            cached_dump_script(&directory, INIT_SCRIPT).unwrap(),
            scripts[0]
        );
        let changed = cached_dump_script(&directory, "different current init script").unwrap();
        assert_ne!(changed, scripts[0]);
        assert_eq!(
            std::fs::read(&changed).unwrap(),
            b"different current init script"
        );
        std::fs::write(&scripts[0], "corrupt").unwrap();
        assert!(cached_dump_script(&directory, INIT_SCRIPT).is_err());
        assert_eq!(std::fs::read(&scripts[0]).unwrap(), b"corrupt");
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 2);
        for version in std::fs::read_dir(&directory).unwrap() {
            let version = version.unwrap();
            assert!(version.file_type().unwrap().is_dir());
            assert_eq!(std::fs::read_dir(version.path()).unwrap().count(), 1);
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn source_cache_written_after_config_change_cannot_restore_stale_selection() {
        let root =
            std::env::temp_dir().join(format!("ktlsp-stale-source-cache-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let build = root.join("build.gradle");
        std::fs::write(&build, "plugins { id 'java' }\n").unwrap();
        let old_time = std::fs::metadata(&build).unwrap().modified().unwrap();
        let modules = parse_dump(&format!(
            "PROJECT\t:\t{}\nSOURCESET\tmain\nEND\t:\n",
            root.display()
        ));
        let mut accepted = ProjectSnapshot::new(modules.clone());
        accepted.mtimes = build_file_mtimes(&root, &accepted.modules);
        accepted.modules[0].source_sets[0].source_artifacts = Some(Vec::new());
        std::fs::write(&build, "plugins { id 'java-library' }\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&build)
            .unwrap()
            .set_modified(old_time + std::time::Duration::from_secs(2))
            .unwrap();
        // Simulate the generation changing between the worker's check and its disk write.
        store_snapshot(&root, &accepted);
        let reloaded = std::cell::Cell::new(false);
        let fresh = load_snapshot_with(&root, false, || {
            reloaded.set(true);
            Ok(modules)
        })
        .unwrap();
        assert!(reloaded.get());
        assert!(fresh.modules[0].source_sets[0].source_artifacts.is_none());
        std::fs::remove_file(cache_path(&root)).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_model_cancellation_does_not_wait_for_another_gradle_resolution_lock() {
        let guard = RESOLUTION_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let (started, entered) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            started.send(()).unwrap();
            load_snapshot_with_cancellable(
                Path::new("/cancelled-model-lock"),
                true,
                Some(&worker_cancelled),
                || panic!("cancelled model import must not launch Gradle"),
            )
        });
        entered.recv().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        cancelled.store(true, AtomicOrdering::Release);
        let started = std::time::Instant::now();
        while !worker.is_finished() && started.elapsed() < std::time::Duration::from_secs(2) {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let cancelled_while_locked = worker.is_finished();
        drop(guard);
        assert!(worker.join().unwrap().is_err());
        assert!(
            cancelled_while_locked,
            "shutdown must not wait for another workspace's Gradle process"
        );
    }

    #[test]
    fn source_query_keeps_resolved_artifacts_and_parent_poms_when_another_module_fails() {
        let root =
            std::env::temp_dir().join(format!("ktlsp-partial-source-query-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let binary = root.join("api.jar");
        let sources = root.join("api-sources.jar");
        let pom = root.join("api.pom");
        let parent = root.join("parent.pom");
        for path in [&binary, &sources, &pom, &parent] {
            std::fs::write(path, b"v1").unwrap();
        }
        let modules = parse_dump(&format!(
            "PROJECT\t:app\t{}\nSOURCESET\tmain\nSCP\tmain\t{}\nARTIFACT\tmain\t{}\tMODULE\tvendor\tapi\t1\nARTIFACT\tmain\t{}\tMODULE\tvendor\tmissing\t1\nSOURCEQUERY\tmain\tfailed\nSOURCEARTIFACT\tmain\t{}\tMODULE\tvendor\tapi\t1\nPOMQUERY\tmain\tfailed\nPOMARTIFACT\tmain\t{}\tMODULE\tvendor\tapi\t1\nPOMARTIFACT\tmain\t{}\tMODULE\tvendor\tparent\t1\nEND\t:app\n",
            root.display(),
            binary.display(),
            binary.display(),
            root.join("missing.jar").display(),
            sources.display(),
            pom.display(),
            parent.display()
        ));
        let selected = ProjectSnapshot::new(modules.clone());
        let api = Coordinate::parse("vendor:api:1").unwrap();
        let missing = Coordinate::parse("vendor:missing:1").unwrap();
        let parent_coord = Coordinate::parse("vendor:parent:1").unwrap();
        assert_eq!(
            selected.coordinate_sources(&api).unwrap().unwrap()[0].0,
            sources
        );
        assert!(selected.coordinate_sources(&missing).is_err());
        assert_eq!(
            selected.coordinate_source_poms(&api).unwrap().unwrap()[0].0,
            pom
        );
        assert_eq!(
            selected
                .coordinate_source_poms(&parent_coord)
                .unwrap()
                .unwrap()[0]
                .0,
            parent
        );
        assert!(selected.coordinate_source_poms(&missing).is_err());
        assert!(selected.source_enrichment_needed());
        std::fs::write(&parent, b"changed parent encoding").unwrap();
        let changed = ProjectSnapshot::new(modules);
        let child = maven_origin_id(&api);
        assert_ne!(
            selected.artifact_revisions()[&child],
            changed.artifact_revisions()[&child],
            "a parent POM edit must invalidate the selected child's source policy"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_query_preserves_exact_selection_absence_failure_and_ambiguity() {
        let coord = Coordinate::parse("vendor:api:1").unwrap();
        let prefix = "PROJECT\t:app\t/repo/app\nSOURCESET\tmain\nARTIFACT\tmain\t/repo/api.jar\tMODULE\tvendor\tapi\t1\n";
        let model =
            |extra: &str| ProjectSnapshot::new(parse_dump(&format!("{prefix}{extra}END\t:app\n")));
        assert_eq!(model("").coordinate_sources(&coord).unwrap(), None);
        assert_eq!(
            model("SOURCEQUERY\tmain\tcomplete\n")
                .coordinate_sources(&coord)
                .unwrap(),
            Some(Vec::new())
        );
        assert!(model("SOURCEQUERY\tmain\tfailed\n")
            .coordinate_sources(&coord)
            .is_err());
        let selected = "SOURCEQUERY\tmain\tcomplete\nSOURCEARTIFACT\tmain\t/repo/settings/api-1-sources.jar\tMODULE\tvendor\tapi\t1\n";
        let snapshot = model(selected);
        assert_ne!(
            snapshot.artifact_revisions(),
            model("").artifact_revisions(),
            "source selection changes must invalidate completed coordinate facts"
        );
        let sources = snapshot.coordinate_sources(&coord).unwrap().unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(
            sources[0].0,
            PathBuf::from("/repo/settings/api-1-sources.jar")
        );
        assert_eq!(
            sources[0].1,
            snapshot.artifact_origin(&sources[0].0).unwrap()
        );
        for invalid in [
            "SOURCEQUERY\tmain\tcomplete\n",
            "SOURCEARTIFACT\tmain\t/repo/project/api-1-sources.jar\tMODULE\tvendor\tapi\t1\n",
            "SOURCEARTIFACT\tmain\t/repo/bad.jar\tPROJECT\t:\t:other\n",
            "SOURCEARTIFACT\tmain\n",
        ] {
            assert!(model(&format!("{selected}{invalid}"))
                .coordinate_sources(&coord)
                .is_err());
        }
        let other_source_set = "SOURCESET\ttest\nARTIFACT\ttest\t/repo/other-api.jar\tMODULE\tvendor\tapi\t1\nSOURCEQUERY\ttest\tcomplete\nSOURCEARTIFACT\ttest\t/repo/project/api-1-sources.jar\tMODULE\tvendor\tapi\t1\n";
        assert!(model(&format!("{selected}{other_source_set}"))
            .coordinate_sources(&coord)
            .is_err());
        let serialized = serde_json::to_value(&snapshot.modules[0].source_sets[0]).unwrap();
        let mut previous = serialized.as_object().unwrap().clone();
        previous.remove("source_artifacts");
        previous.remove("source_artifacts_incomplete");
        let previous: SourceSetClasspath = serde_json::from_value(previous.into()).unwrap();
        assert_eq!(
            previous.source_artifacts, None,
            "MODEL14 remains valid before enrichment"
        );
        assert!(!previous.source_artifacts_incomplete);
    }

    fn source_snapshot_order_fixture() -> (PathBuf, ProjectSnapshot, Vec<ModuleClasspath>) {
        let root = std::env::temp_dir().join(format!(
            "ktlsp-source-order-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let compiler = serde_json::json!({
            "task": ":app:compileJava", "language": "java",
            "task_type": "org.gradle.api.tasks.compile.JavaCompile",
            "arguments": ["-g", "-parameters"], "annotation_processor_path": [],
            "java_home": "/jdk", "options": {"release": 8},
            "output_roots": [], "complete": true, "problem": null
        });
        let dump = format!(
            "PROJECT\t:app\t/repo/app\nCP\t/jars/a.jar\nCP\t/jars/b.jar\n\
             SOURCESET\tmain\nSCP\tmain\t/jars/a.jar\nSCP\tmain\t/jars/b.jar\n\
             ARTIFACT\tmain\t/jars/api.jar\tMODULE\tvendor\tapi\t1\n\
             COMPILER\tmain\t{compiler}\nSOURCESET\ttest\nEND\t:app\n\
             PROJECT\t:lib\t/repo/lib\nSOURCESET\tmain\nEND\t:lib\n"
        );
        let modules = parse_dump_reader(dump.as_bytes(), true).unwrap();
        assert_eq!(modules.len(), 2);
        let mut accepted = ProjectSnapshot::new(modules.clone());
        accepted.mtimes = build_file_mtimes(&root, &accepted.modules);
        let mut queried = modules;
        for source in queried
            .iter_mut()
            .flat_map(|module| &mut module.source_sets)
        {
            source.source_artifacts = Some(Vec::new());
            source.source_poms = Some(Vec::new());
        }
        queried[0].source_sets[0].source_artifacts = Some(vec![ResolvedArtifact {
            path: PathBuf::from("/sources/api-sources.jar"),
            component: ComponentId::Module {
                group: "vendor".into(),
                name: "api".into(),
                version: "1".into(),
            },
        }]);
        (root, accepted, queried)
    }

    #[test]
    fn source_snapshot_accepts_unique_reordered_modules_and_retains_sources() {
        let (root, accepted, mut queried) = source_snapshot_order_fixture();
        queried.reverse();
        let enriched = source_snapshot(&root, &accepted, queried.clone()).unwrap();
        assert!(accepted.source_enrichment_needed());
        assert!(!enriched.source_enrichment_needed());
        assert_eq!(
            enriched.modules, queried,
            "keep returned order and exact enriched facts"
        );
        assert_eq!(enriched.modules[0].module, ":lib");
        assert_eq!(
            enriched.modules[1].source_sets[0]
                .source_artifacts
                .as_ref()
                .unwrap()[0]
                .path,
            PathBuf::from("/sources/api-sources.jar")
        );

        // A project path is scoped to its Gradle build; these identities are distinct.
        let mut modules = accepted.modules.clone();
        modules[1].module = modules[0].module.clone();
        modules[1].build = ":included".into();
        let mut accepted = ProjectSnapshot::new(modules.clone());
        accepted.mtimes = build_file_mtimes(&root, &accepted.modules);
        modules.reverse();
        assert!(source_snapshot(&root, &accepted, modules).is_ok());
    }

    #[test]
    fn source_snapshot_reordered_modules_reject_binary_drift_and_duplicate_identities() {
        let (root, accepted, queried) = source_snapshot_order_fixture();
        let changes: [(&str, fn(&mut [ModuleClasspath])); 7] = [
            ("module directory", |modules| {
                modules[0].project_dir = PathBuf::from("/changed")
            }),
            ("module identity", |modules| {
                modules[0].build = ":other".into()
            }),
            ("compiler arguments order", |modules| {
                modules[0].source_sets[0].compiler_tasks[0]
                    .arguments
                    .swap(0, 1)
            }),
            ("compiler options", |modules| {
                modules[0].source_sets[0].compiler_tasks[0]
                    .options
                    .insert("release".into(), serde_json::json!(9));
            }),
            ("source classpath order", |modules| {
                modules[0].source_sets[0].entries.swap(0, 1)
            }),
            ("module classpath order", |modules| {
                modules[0].entries.swap(0, 1)
            }),
            ("source set order", |modules| {
                modules[0].source_sets.swap(0, 1)
            }),
        ];
        for (label, change) in changes {
            let mut changed = queried.clone();
            change(&mut changed);
            changed.reverse();
            assert!(
                source_snapshot(&root, &accepted, changed).is_err(),
                "{label}"
            );
        }

        // Both sides duplicated identically would be silently accepted by collect-to-map.
        for same_directory in [true, false] {
            let mut duplicated = vec![queried[0].clone(), queried[0].clone()];
            if !same_directory {
                duplicated[1].project_dir = PathBuf::from("/repo/duplicate-app");
            }
            let mut duplicate_accepted = ProjectSnapshot::new(duplicated.clone());
            duplicate_accepted.mtimes = build_file_mtimes(&root, &duplicate_accepted.modules);
            assert!(source_snapshot(&root, &duplicate_accepted, duplicated.clone()).is_err());
            assert!(source_snapshot(&root, &accepted, duplicated).is_err());
        }
    }

    #[test]
    fn source_snapshot_reordered_modules_reject_changed_build_file_mtime() {
        let (root, mut accepted, mut queried) = source_snapshot_order_fixture();
        std::fs::create_dir(&root).unwrap();
        let build = root.join("build.gradle.kts");
        std::fs::write(&build, "// source snapshot build input\n").unwrap();
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&build)
            .unwrap();
        let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        file.set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        accepted.mtimes = build_file_mtimes(&root, &accepted.modules);
        queried.reverse();
        assert!(source_snapshot(&root, &accepted, queried.clone()).is_ok());
        file.set_times(
            std::fs::FileTimes::new().set_modified(modified + std::time::Duration::from_secs(1)),
        )
        .unwrap();
        assert!(source_snapshot(&root, &accepted, queried).is_err());
        drop(file);
        std::fs::remove_file(build).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    #[ignore = "local real paired Gradle protocol replay; requires KTLSP_CLOSED_GRADLE_INITIAL_PROTOCOL, KTLSP_CLOSED_GRADLE_SOURCES_PROTOCOL and KTLSP_CLOSED_GRADLE_ROOT"]
    fn retained_gradle_8142_source_query_reordering_preserves_binary_model() {
        let parse = |name| {
            let file = std::fs::File::open(std::env::var_os(name).expect("retained protocol path"))
                .unwrap();
            parse_dump_reader(BufReader::new(file), true).unwrap()
        };
        let initial = parse("KTLSP_CLOSED_GRADLE_INITIAL_PROTOCOL");
        let queried = parse("KTLSP_CLOSED_GRADLE_SOURCES_PROTOCOL");
        assert_eq!(initial.len(), 194);
        assert_eq!(queried.len(), 194);
        let identities = |modules: &[ModuleClasspath]| {
            modules
                .iter()
                .map(|module| (module.build.clone(), module.module.clone()))
                .collect::<Vec<_>>()
        };
        assert_ne!(
            identities(&initial),
            identities(&queried),
            "retain actual completion-order change"
        );
        let root = PathBuf::from(
            std::env::var_os("KTLSP_CLOSED_GRADLE_ROOT").expect("actual pinned root"),
        );
        let mut accepted = ProjectSnapshot::new(initial);
        accepted.mtimes = build_file_mtimes(&root, &accepted.modules);
        let enriched = source_snapshot(&root, &accepted, queried)
            .expect("only outer order and source/POM facts differ");
        let sources = enriched
            .modules
            .iter()
            .flat_map(|module| &module.source_sets)
            .collect::<Vec<_>>();
        assert_eq!(
            sources
                .iter()
                .map(|source| source.source_artifacts.as_ref().unwrap().len())
                .sum::<usize>(),
            49_390
        );
        assert_eq!(
            sources
                .iter()
                .map(|source| source.source_poms.as_ref().unwrap().len())
                .sum::<usize>(),
            72_307
        );
    }

    #[test]
    fn source_snapshot_rejects_changed_binary_model_or_build_inputs() {
        let root =
            std::env::temp_dir().join(format!("ktlsp-source-snapshot-{}", std::process::id()));
        let modules = parse_dump(
            "PROJECT\t:app\t/repo/app\nSOURCESET\tmain\nARTIFACT\tmain\t/repo/api.jar\tMODULE\tvendor\tapi\t1\nEND\t:app\n",
        );
        let mut accepted = ProjectSnapshot::new(modules.clone());
        accepted.mtimes = build_file_mtimes(&root, &accepted.modules);
        let mut enriched = modules;
        enriched[0].source_sets[0].source_artifacts = Some(Vec::new());
        assert!(source_snapshot(&root, &accepted, enriched.clone()).is_ok());
        let mut changed = enriched.clone();
        changed[0].source_sets[0].artifacts[0].path = PathBuf::from("/repo/changed.jar");
        assert!(source_snapshot(&root, &accepted, changed).is_err());
        accepted.mtimes.insert("changed-build-input".into(), 1);
        assert!(source_snapshot(&root, &accepted, enriched).is_err());
    }
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
    #[ignore = "local real Gradle protocol replay; requires KTLSP_CLOSED_GRADLE_PROTOCOL artifact"]
    fn retained_gradle_8142_closed_output_passes_the_normal_strict_parser() {
        let path = std::env::var_os("KTLSP_CLOSED_GRADLE_PROTOCOL")
            .expect("actual retained protocol path");
        let file = std::fs::File::open(path).unwrap();
        let modules = parse_dump_reader(BufReader::new(file), true).unwrap();
        assert_eq!(modules.len(), 194);
        let problems = modules
            .iter()
            .find(|module| module.module == ":problems")
            .unwrap();
        let main = problems
            .source_sets
            .iter()
            .find(|source_set| source_set.name == "main")
            .unwrap();
        assert!(main
            .compiler_tasks
            .iter()
            .any(|task| task.task == ":problems:compileJava" && task.complete));
        assert!(main
            .compiler_tasks
            .iter()
            .any(|task| task.task == ":problems:compileKotlin" && task.complete));
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
    fn embedded_adapter_content_invalidates_root_and_module_cache_keys() {
        let root = Path::new("/cache-key-fixture");
        for module in [None, Some(":app")] {
            assert_eq!(
                model_cache_hash(root, module, "same adapter"),
                model_cache_hash(root, module, "same adapter")
            );
            assert_ne!(
                model_cache_hash(root, module, "old adapter"),
                model_cache_hash(root, module, "new adapter")
            );
        }
        assert_ne!(
            model_cache_hash(root, None, INIT_SCRIPT),
            model_cache_hash(root, Some(":app"), INIT_SCRIPT)
        );
    }

    #[test]
    fn old_adapter_cache_requires_a_normal_dump_then_reuses_the_new_result() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp-adapter-key-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let make_module = |name: &str| ModuleClasspath {
            module: name.into(),
            project_dir: root.clone(),
            build: ":".into(),
            source_sets: Vec::new(),
            entries: Vec::new(),
            unresolved: Vec::new(),
        };
        let old_hash = model_cache_hash(&root, None, "previous embedded compiler adapter");
        let old_path = cache_path(&root).with_file_name(format!("{old_hash:016x}.json"));
        assert_ne!(old_path, cache_path(&root));
        write_cache_entry(
            &old_path,
            &CacheEntry {
                model_version: MODEL_VERSION,
                mtimes: BTreeMap::new(),
                modules: vec![make_module(":old-unsupported-recipe")],
            },
        );
        let called = std::cell::Cell::new(false);
        let fresh = load_snapshot_with(&root, false, || {
            called.set(true);
            Ok(vec![make_module(":fresh-adapter")])
        })
        .unwrap();
        assert!(
            called.get(),
            "different adapter must perform a normal dump instead of reusing old compiler metadata"
        );
        assert_eq!(fresh.modules[0].module, ":fresh-adapter");
        store_snapshot(&root, &fresh);
        let warm = load_snapshot_with(&root, false, || {
            anyhow::bail!("unchanged adapter should reuse its normal dump")
        })
        .unwrap();
        assert_eq!(warm.modules[0].module, ":fresh-adapter");
        std::fs::remove_file(old_path).unwrap();
        std::fs::remove_file(cache_path(&root)).unwrap();
        std::fs::remove_dir(root).unwrap();
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
            root.join("mapped-module").display(),
            root.join("custom-kotlin").display(),
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
            serde_json::to_vec(&CacheEntry {
                // The embedded script previously left Gradle 9.6.1 recipes unsupported.
                model_version: 11,
                mtimes: model.mtimes.clone(),
                modules: model.modules.clone(),
            })
            .unwrap(),
            serde_json::to_vec(&CacheEntry {
                // MODEL12 cached the selected JDK21/explicit UTF-8 recipe as unsupported.
                model_version: 12,
                mtimes: model.mtimes.clone(),
                modules: model.modules.clone(),
            })
            .unwrap(),
            serde_json::to_vec(&CacheEntry {
                // MODEL13 has no typed KGP classpath-snapshot input role.
                model_version: 13,
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
    fn extension_project_context_borrows_selected_scope_and_preserves_classpath_order() {
        let model = ProjectSnapshot::new(parse_dump(
            "PROJECT\t:editing\t/actual/module\nBUILD\t:included\n\
             SOURCE\tmain\t/custom/java\nSOURCE\tmain\t/custom/kotlin\n\
             OUTPUT\tmain\t/generated/classes\nSCP\tmain\t/libs/z.jar\n\
             SCP\tmain\t/libs/a.jar\nSCP\tmain\t/libs/z.jar\nINCOMPLETE\tmain\n\
             SOURCE\tintegration\t/selected/tests\nSCP\tintegration\t/libs/test.jar\nEND\t:editing\n",
        ));
        for file in ["/custom/java/Use.java", "/custom/kotlin/Use.kt"] {
            let context = model.extension_project_context(Path::new(file)).unwrap();
            assert_eq!(context.module, ":editing");
            assert_eq!(context.scope.module, "gradle::included|:editing");
            assert_eq!(context.scope.source_set, "main");
            assert!(std::ptr::eq(
                context.source,
                &model.modules[0].source_sets[0]
            ));
            assert_eq!(
                context.source.entries,
                vec![
                    PathBuf::from("/libs/z.jar"),
                    PathBuf::from("/libs/a.jar"),
                    PathBuf::from("/libs/z.jar")
                ]
            );
            assert_eq!(
                context.source.output_roots,
                vec![PathBuf::from("/generated/classes")]
            );
            assert!(!context.classpath_complete);
            assert!(context.sdk.is_none());
            assert!(model.artifact_origin(&context.source.entries[0]).is_some());
            assert_eq!(
                model.artifact_origin(&context.source.entries[0]),
                model.artifact_origin(&context.source.entries[2])
            );
        }
        let context = model
            .extension_project_context(Path::new("/selected/tests/Test.kt"))
            .unwrap();
        assert_eq!(context.scope.source_set, "integration");
        assert!(context.classpath_complete);
        assert!(
            model
                .extension_project_context(Path::new("/actual/module/Unregistered.kt"))
                .is_none()
        );
        assert!(
            model
                .extension_project_context(Path::new("/unrelated/Use.java"))
                .is_none()
        );
        let mut modules = model.modules.clone();
        modules[0]
            .unresolved
            .push("unresolved module dependency".into());
        let incomplete = ProjectSnapshot::new(modules);
        assert!(
            !incomplete
                .extension_project_context(Path::new("/selected/tests/Test.kt"))
                .unwrap()
                .classpath_complete
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
            ]
        );
        assert_eq!(
            visibilities[1].ordered_origins,
            vec![
                "project:gradle::|:app#test",
                "project:gradle::|:app#main",
                "maven:vendor:api:1",
            ]
        );
        assert!(visibilities.iter().all(|scope| {
            !scope
                .ordered_origins
                .iter()
                .any(|origin| origin == "jdk:chosen")
        }));
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
    fn transform_snapshot_cache_refreshes_missing_inputs_but_reuses_source_saves() {
        for memory in [false, true] {
            let root = std::env::temp_dir().join(format!(
                "ktlsp-transform-cache-{}-{}-{memory}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let snapshot_file = root.join("cache/transforms/id/transformed/api-snapshot.bin");
            let source = root.join("Use.java");
            let kotlin_source = root.join("Use.kt");
            std::fs::create_dir_all(snapshot_file.parent().unwrap()).unwrap();
            std::fs::write(root.join("build.gradle"), "plugins { id 'java' }").unwrap();
            let wrapper = root.join(if cfg!(windows) {
                "gradlew.bat"
            } else {
                "gradlew"
            });
            std::fs::write(
                &wrapper,
                if cfg!(windows) {
                    "@echo off\r\necho invoked> \"%~dp0gradle-invoked\"\r\nexit /b 23\r\n"
                } else {
                    "#!/bin/sh\n: > \"$(dirname \"$0\")/gradle-invoked\"\nexit 23\n"
                },
            )
            .unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            std::fs::write(&source, "class Use {}").unwrap();
            std::fs::write(&kotlin_source, "class KotlinUse").unwrap();
            std::fs::write(&snapshot_file, "snapshot").unwrap();
            let set_snapshot_time = |seconds| {
                std::fs::File::options()
                    .write(true)
                    .open(&snapshot_file)
                    .unwrap()
                    .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds))
                    .unwrap();
            };
            set_snapshot_time(0);
            let mut compiler = plain_java_configuration();
            compiler["source_files"] = serde_json::json!([source]);
            compiler["declared_input_files"] =
                serde_json::json!([source, kotlin_source, snapshot_file, snapshot_file]);
            let modules = parse_dump(&format!(
                "PROJECT\t:app\t{}\nSOURCESET\tmain\nCOMPILER\tmain\t{compiler}\nEND\t:app\n",
                root.display()
            ));
            let dumps = std::cell::Cell::new(0);
            let load = || {
                load_snapshot_with(&root, false, || {
                    dumps.set(dumps.get() + 1);
                    Ok(modules.clone())
                })
                .unwrap()
            };
            let retain = |model: &Arc<ProjectSnapshot>| {
                store_snapshot(&root, model);
                if memory {
                    publish_snapshot(&root, model.clone());
                }
            };
            let first = load();
            retain(&first);
            let warm = load();
            assert_eq!(dumps.get(), 1, "an existing epoch-zero input is reusable");
            if memory {
                assert!(Arc::ptr_eq(&first, &warm));
            }
            std::fs::write(&source, "class Use { int changed; }").unwrap();
            std::fs::write(&kotlin_source, "class KotlinUse { val changed = 1 }").unwrap();
            load();
            assert_eq!(dumps.get(), 1, "ordinary source saves reuse the classpath");

            set_snapshot_time(2);
            let changed = load();
            assert_eq!(dumps.get(), 2, "a changed transform invalidates the cache");
            retain(&changed);
            load();
            assert_eq!(dumps.get(), 2, "unchanged transform metadata stays warm");

            std::fs::remove_file(&snapshot_file).unwrap();
            let missing = load();
            assert_eq!(dumps.get(), 3, "a deleted transform requires a normal dump");
            retain(&missing);
            load();
            assert_eq!(dumps.get(), 4, "a missing zero baseline is never fresh");

            std::fs::write(&snapshot_file, "restored").unwrap();
            set_snapshot_time(4);
            let restored = load();
            retain(&restored);
            assert_eq!(dumps.get(), 5);
            write_cache_entry(
                &module_cache_path(&root, ":app"),
                &CacheEntryRef {
                    model_version: MODEL_VERSION,
                    mtimes: &restored.mtimes,
                    modules: &restored.modules,
                },
            );
            assert_eq!(resolve_module(&root, ":app").unwrap().module, ":app");
            assert!(!root.join("gradle-invoked").exists());
            std::fs::remove_file(&snapshot_file).unwrap();
            let failure = resolve_module(&root, ":app")
                .expect_err("neither memory nor module cache may hide the missing input");
            assert!(root.join("gradle-invoked").is_file());
            assert!(format!("{failure:#}").contains("gradle classpath dump failed"));
            std::fs::remove_file(cache_path(&root)).unwrap();
            std::fs::remove_file(module_cache_path(&root, ":app")).unwrap();
            std::fs::remove_dir_all(root).unwrap();
        }
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
                root.join("modules/editing").display(),
                root.join("custom-generated").display(),
                root.join("api.jar").display(),
                root.join("api.jar").display()
            ))
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

    #[test]
    #[ignore = "local real Gradle runtime mutation admission; requires KTLSP_COMPILER_RUNTIME_PROBE_ROOT"]
    fn normal_dump_rejects_changed_runtime_after_complete_model_output() {
        let root = PathBuf::from(
            std::env::var_os("KTLSP_COMPILER_RUNTIME_PROBE_ROOT")
                .expect("prepared disposable classpath-config-cache harness project"),
        );
        // A complete protocol by itself is valid. The actual stock Gradle service's close
        // must fail, and the normal importer must not accept those already-printed rows.
        let complete = format!(
            "PROJECT\t:mutation\t{}\nBUILD\t:\nSOURCESET\tmain\nEND\t:mutation\n",
            root.display()
        );
        assert_eq!(
            parse_dump_reader(std::io::Cursor::new(complete), true)
                .unwrap()
                .len(),
            1
        );
        let failure = run_dump(&root, "ktlspCompilerRuntimeMutationProbe", true)
            .expect_err("changed runtime must not become an accepted model");
        let message = format!("{failure:#}");
        assert!(
            message.contains("gradle classpath dump failed"),
            "{message}"
        );
        assert!(
            message.contains("Selected Kotlin compiler runtime changed during export"),
            "{message}"
        );
    }
}
