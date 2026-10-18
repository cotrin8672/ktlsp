//! Fixed KSP -> stubs -> active KAPT execution, not a general task scheduler.
//! These leases are in-memory capabilities. A JSON receipt or saved build output cannot mint one.
use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// Immutable invocation facts captured on the same accepted project model as the request.
/// No filesystem work or API authority is established in the workspace request factory.
#[derive(Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Plan {
    root: PathBuf,
    owner: crate::classpath::ModuleClasspath,
    primary: CompilerTask,
    java: CompilerTask,
    tasks: Tasks,
    processor_modules: Vec<crate::classpath::ModuleClasspath>,
    module_dirs: Vec<PathBuf>,
    unselected_source_roots: Vec<PathBuf>,
}

impl Plan {
    pub(crate) fn capture(
        owner: &crate::classpath::ModuleClasspath,
        source: &crate::classpath::SourceSetClasspath,
        primary: &CompilerTask,
        java: &CompilerTask,
        model: &crate::classpath::ProjectSnapshot,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            source.name == "main"
                && !source.classpath_incomplete
                && !source.compiler_metadata_incomplete
                && source.unresolved.is_empty()
                && source.compiler_tasks.len() == 5
                && java.java_compiler_recipe().is_some(),
            "processor-source-set-contract-incomplete"
        );
        let exactly_one = |kind: &str| -> anyhow::Result<CompilerTask> {
            let matches: Vec<_> = source
                .compiler_tasks
                .iter()
                .filter(|t| t.task_type == kind)
                .collect();
            anyhow::ensure!(matches.len() == 1, "processor-task-type-ambiguous");
            Ok(matches[0].clone())
        };
        let tasks = Tasks {
            ksp: exactly_one("com.google.devtools.ksp.gradle.KspTaskJvm_Decorated")?,
            stubs: exactly_one(
                "org.jetbrains.kotlin.gradle.internal.KaptGenerateStubsTask_Decorated",
            )?,
            kapt: exactly_one(
                "org.jetbrains.kotlin.gradle.internal.KaptWithoutKotlincTask_Decorated",
            )?,
        };
        tasks.validate(primary, java)?;
        let root = owner
            .build
            .strip_prefix("legacy:")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .ok_or_else(|| anyhow::anyhow!("processor-build-root-unmapped"))?;
        anyhow::ensure!(
            owner.project_dir.starts_with(&root),
            "processor-module-outside-build"
        );
        let processor_paths = array(
            &tasks.ksp.options["kspProcessorInputs"],
            "processorClasspath",
        )?
        .iter()
        .map(|p| {
            p.as_str()
                .map(PathBuf::from)
                .ok_or_else(|| anyhow::anyhow!("processor-classpath-path-unavailable"))
        })
        .chain(tasks.kapt.annotation_processor_path.iter().cloned().map(Ok))
        .collect::<anyhow::Result<Vec<_>>>()?;
        let mut selected = BTreeMap::new();
        for path in processor_paths {
            let owners: Vec<_> = model
                .modules
                .iter()
                .flat_map(|module| {
                    module
                        .source_sets
                        .iter()
                        .filter(|set| set.provided_artifacts.iter().any(|p| same_path(p, &path)))
                        .map(move |set| (module, set))
                })
                .collect();
            if owners.is_empty() {
                anyhow::ensure!(
                    !path.starts_with(&root),
                    "processor-local-artifact-producer-unmapped"
                );
                continue; // exact external artifact bytes are ordinary declared read inputs
            }
            anyhow::ensure!(owners.len() == 1, "processor-artifact-producer-ambiguous");
            let (module, set) = owners[0];
            validate_bootstrap_owner(module, set, owner, &root)?;
            selected.insert(
                (module.build.clone(), module.module.clone()),
                module.clone(),
            );
        }
        // Runtime processor paths omit compileOnly producers. Follow the modeled main-task
        // declared reads to a fixed point before selecting unsaved overlays or a reuse key.
        let mut visited = BTreeSet::new();
        while let Some(key) = selected.keys().find(|key| !visited.contains(*key)).cloned() {
            visited.insert(key.clone());
            let module = selected[&key].clone();
            let set = module
                .source_sets
                .iter()
                .find(|s| s.name == "main")
                .unwrap();
            let reads = set
                .entries
                .iter()
                .chain(set.compiler_tasks.iter().flat_map(|task| {
                    task.source_files
                        .iter()
                        .chain(&task.classpath)
                        .chain(&task.declared_input_files)
                        .chain(&task.annotation_processor_path)
                }));
            for read in reads {
                if let Some((dependency, source)) = bootstrap_read_owner(model, read, &root)? {
                    validate_bootstrap_owner(dependency, source, owner, &root)?;
                    selected
                        .entry((dependency.build.clone(), dependency.module.clone()))
                        .or_insert_with(|| dependency.clone());
                }
            }
        }
        let module_dirs = std::iter::once(root.clone())
            .chain(
                model
                    .modules
                    .iter()
                    .filter(|m| m.build == owner.build)
                    .map(|m| m.project_dir.clone()),
            )
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let unselected_source_roots = model
            .modules
            .iter()
            .filter(|m| {
                m.build == owner.build
                    && m.module != owner.module
                    && !selected.contains_key(&(m.build.clone(), m.module.clone()))
            })
            .flat_map(|m| &m.source_sets)
            .flat_map(|s| &s.source_roots)
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        Ok(Self {
            root,
            owner: owner.clone(),
            primary: primary.clone(),
            java: java.clone(),
            tasks,
            processor_modules: selected.into_values().collect(),
            module_dirs,
            unselected_source_roots,
        })
    }

    /// Include unsaved processor buffers even when their source set is NOT a semantic compile
    /// dependency of the consumer. New/custom-filtered source selection needs its own proof.
    pub(crate) fn selects_processor_source(&self, path: &Path) -> bool {
        self.processor_modules
            .iter()
            .flat_map(|m| &m.source_sets)
            .filter(|s| s.name == "main")
            .flat_map(|s| &s.compiler_tasks)
            .flat_map(|t| &t.source_files)
            .any(|source| same_path(source, path))
    }

    pub(crate) fn affects_processor_source(&self, path: &Path) -> bool {
        self.selects_processor_source(path)
            || self
                .processor_modules
                .iter()
                .flat_map(|module| &module.source_sets)
                .filter(|set| set.name == "main")
                .flat_map(|set| &set.source_roots)
                .any(|root| {
                    path_under(
                        &kotlin_path_key(&crate::paths::file_key(path)),
                        &kotlin_path_key(&crate::paths::file_key(root)),
                    )
                })
    }

    /// An execution-boundary bootstrap read cannot silently introduce a project whose unsaved
    /// inputs were absent from the request. Unknown noncompiler task edges stay unavailable.
    fn owns_bootstrap_read(&self, path: &Path, directory: bool) -> bool {
        if !directory
            && matches!(
                path.extension().and_then(|s| s.to_str()),
                Some("kt" | "java")
            )
        {
            let own = self.primary.source_files.iter().any(|p| same_path(p, path))
                || self
                    .owner
                    .source_sets
                    .iter()
                    .filter(|s| s.name == "main")
                    .flat_map(|s| &s.source_roots)
                    .any(|root| path.starts_with(root));
            if !own && !self.affects_processor_source(path) {
                return false;
            }
        }
        if directory
            && std::iter::once(&self.owner)
                .chain(&self.processor_modules)
                .flat_map(|m| &m.source_sets)
                .filter(|s| s.name != "main")
                .flat_map(|s| &s.source_roots)
                .any(|root| root.starts_with(path))
        {
            return false;
        }
        let selected = |module: &Path| {
            same_path(module, &self.owner.project_dir)
                || self
                    .processor_modules
                    .iter()
                    .any(|m| same_path(module, &m.project_dir))
        };
        if directory
            && self.module_dirs.iter().any(|m| {
                !same_path(m, path)
                    && m.starts_with(path)
                    && !selected(m)
                    && !same_path(m, &self.root)
            })
        {
            return false;
        }
        if let Some(module) = self
            .module_dirs
            .iter()
            .filter(|m| path.starts_with(m))
            .max_by_key(|m| m.components().count())
        {
            if !same_path(module, &self.root) || selected(module) {
                return selected(module);
            }
        }
        self.affects_processor_source(path)
            || (path.parent() == Some(self.root.as_path())
                && matches!(
                    path.file_name().and_then(|s| s.to_str()),
                    Some(
                        "settings.gradle"
                            | "settings.gradle.kts"
                            | "build.gradle"
                            | "build.gradle.kts"
                            | "gradle.properties"
                    )
                ))
            || (path.starts_with(self.root.join("gradle"))
                && !matches!(
                    path.extension().and_then(|s| s.to_str()),
                    Some("kt" | "java")
                ))
    }

    pub(crate) fn watch_inputs(&self) -> Vec<PathBuf> {
        let mut inputs: Vec<_> = self
            .processor_modules
            .iter()
            .flat_map(|module| {
                module
                    .source_sets
                    .iter()
                    .filter(|source| source.name == "main")
            })
            .flat_map(|source| source.source_roots.iter().cloned())
            .collect();
        for module in &self.processor_modules {
            inputs.push(module.project_dir.join("src"));
        }
        for root in std::iter::once(&self.root)
            .chain(std::iter::once(&self.owner.project_dir))
            .chain(
                self.processor_modules
                    .iter()
                    .map(|module| &module.project_dir),
            )
        {
            for name in [
                "settings.gradle",
                "settings.gradle.kts",
                "build.gradle",
                "build.gradle.kts",
                "gradle.properties",
            ] {
                inputs.push(root.join(name));
            }
        }
        inputs.push(self.root.join("gradle"));
        inputs.push(self.root.join("buildSrc"));
        inputs.push(self.owner.project_dir.join("src"));
        if let Some(home) = std::env::var_os("GRADLE_USER_HOME")
            .map(PathBuf::from)
            .or_else(|| crate::paths::user_home().map(|h| h.join(".gradle")))
        {
            inputs.extend([
                home.join("gradle.properties"),
                home.join("init.gradle"),
                home.join("init.gradle.kts"),
                home.join("init.d"),
            ]);
        }
        for task in [
            &self.primary,
            &self.java,
            &self.tasks.ksp,
            &self.tasks.stubs,
            &self.tasks.kapt,
        ]
        .into_iter()
        .chain(
            self.processor_modules
                .iter()
                .flat_map(|m| &m.source_sets)
                .filter(|s| s.name == "main")
                .flat_map(|s| &s.compiler_tasks),
        ) {
            inputs.extend(task.declared_input_files.iter().cloned());
            inputs.extend(task.classpath.iter().cloned());
            inputs.extend(task.annotation_processor_path.iter().cloned());
            if let Some(runtime) = &task.kotlin_compiler {
                inputs.extend(runtime.default_classpath.iter().cloned());
                inputs.extend(runtime.current_jvm_tools_jar.iter().cloned());
            }
        }
        // The configured KSP processor path is appended by its stock transformer, so it need
        // not occur in the compiler argument/classpath lists above.
        if let Some(paths) = self
            .tasks
            .ksp
            .options
            .get("kspProcessorInputs")
            .and_then(|inputs| inputs.get("processorClasspath"))
            .and_then(Value::as_array)
        {
            inputs.extend(paths.iter().filter_map(Value::as_str).map(PathBuf::from));
        }
        inputs.sort();
        inputs.dedup();
        inputs
    }
}

fn validate_bootstrap_owner(
    module: &crate::classpath::ModuleClasspath,
    set: &crate::classpath::SourceSetClasspath,
    owner: &crate::classpath::ModuleClasspath,
    root: &Path,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        module.build == owner.build
            && module.module != owner.module
            && module.project_dir.starts_with(root)
            && set.name == "main"
            && !set.classpath_incomplete
            && !set.compiler_metadata_incomplete
            && set.unresolved.is_empty()
            && !set.compiler_tasks.is_empty()
            && set.compiler_tasks.iter().all(|task| task.complete
                && task.problem.is_none()
                && matches!(
                    task.task_type.as_str(),
                    "org.gradle.api.tasks.compile.JavaCompile_Decorated"
                        | "org.jetbrains.kotlin.gradle.tasks.KotlinCompile_Decorated"
                )),
        "processor-artifact-current-input-contract-unproved"
    );
    Ok(())
}

fn bootstrap_read_owner<'a>(
    model: &'a crate::classpath::ProjectSnapshot,
    path: &Path,
    root: &Path,
) -> anyhow::Result<
    Option<(
        &'a crate::classpath::ModuleClasspath,
        &'a crate::classpath::SourceSetClasspath,
    )>,
> {
    if !path.starts_with(root) {
        return Ok(None);
    }
    let sets: Vec<_> = model
        .modules
        .iter()
        .flat_map(|module| module.source_sets.iter().map(move |set| (module, set)))
        .collect();
    // An artifact/output association is stronger than a containing project directory.
    let outputs: Vec<_> = sets
        .iter()
        .copied()
        .filter(|(_, set)| {
            set.provided_artifacts
                .iter()
                .chain(&set.output_roots)
                .chain(
                    set.compiler_tasks
                        .iter()
                        .flat_map(|t| &t.declared_output_roots),
                )
                .any(|output| same_path(output, path) || path.starts_with(output))
        })
        .collect();
    let sources: Vec<_> = if outputs.is_empty() {
        sets.iter()
            .copied()
            .filter(|(_, set)| {
                set.source_roots
                    .iter()
                    .any(|source| path.starts_with(source) || source.starts_with(path))
                    || set
                        .compiler_tasks
                        .iter()
                        .any(|t| t.source_files.iter().any(|source| same_path(source, path)))
            })
            .collect()
    } else {
        Vec::new()
    };
    let matches = if outputs.is_empty() { sources } else { outputs };
    if !matches.is_empty() {
        anyhow::ensure!(
            matches.len() == 1,
            "processor-bootstrap-read-owner-ambiguous"
        );
        return Ok(Some(matches[0]));
    }
    // Root Gradle configuration is already captured/watched, not a JVM source producer.
    if path.parent() == Some(root)
        && matches!(
            path.file_name().and_then(|s| s.to_str()),
            Some(
                "settings.gradle"
                    | "settings.gradle.kts"
                    | "build.gradle"
                    | "build.gradle.kts"
                    | "gradle.properties"
            )
        )
        || path.starts_with(root.join("gradle"))
    {
        return Ok(None);
    }
    let depth = model
        .modules
        .iter()
        .filter(|m| path.starts_with(&m.project_dir))
        .map(|m| m.project_dir.components().count())
        .max();
    let closest: Vec<_> = sets
        .into_iter()
        .filter(|(m, s)| {
            s.name == "main"
                && path.starts_with(&m.project_dir)
                && Some(m.project_dir.components().count()) == depth
        })
        .collect();
    anyhow::ensure!(
        closest.len() == 1,
        "processor-bootstrap-local-read-owner-unmapped"
    );
    Ok(Some(closest[0]))
}

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct Tasks {
    pub(super) ksp: CompilerTask,
    pub(super) stubs: CompilerTask,
    pub(super) kapt: CompilerTask,
}

impl Tasks {
    /// Configuration is only an invocation profile, never completion/API authority.
    pub(super) fn validate(
        &self,
        primary: &CompilerTask,
        java: &CompilerTask,
    ) -> anyhow::Result<()> {
        let runtime = primary
            .kotlin_compiler
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("processor-runtime-absent"))?;
        anyhow::ensure!(
            runtime.compiler_version == "1.8.21"
                && runtime.gradle_plugin_version == "1.8.21"
                && runtime.gradle_java_home
                    == *primary
                        .java_home
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("processor-sdk-absent"))?
                && java.java_home == primary.java_home,
            "processor-runtime-profile-unproved"
        );
        let prefix = primary
            .task
            .strip_suffix("compileKotlin")
            .ok_or_else(|| anyhow::anyhow!("processor-main-task-unproved"))?;
        anyhow::ensure!(
            self.ksp.task == format!("{prefix}kspKotlin")
                && self.stubs.task == format!("{prefix}kaptGenerateStubsKotlin")
                && self.kapt.task == format!("{prefix}kaptKotlin"),
            "processor-task-association-unproved"
        );
        // KaptWithoutKotlincTask has no compiler java_home. Its stock worker selects
        // the build JVM (NONE) or the configured executable (PROCESS), independently.
        let worker = self
            .kapt
            .options
            .get("kaptWorkerJvm")
            .ok_or_else(|| anyhow::anyhow!("kapt-worker-jvm-absent"))?;
        let executable = Path::new(value_string(worker, "javaExecutable")?);
        let build_home = Path::new(value_string(worker, "buildJavaHome")?);
        let expected_executable = runtime.gradle_java_home.join("bin").join(if cfg!(windows) {
            "java.exe"
        } else {
            "java"
        });
        anyhow::ensure!(
            worker["adapter"] == "kgp-1.8.21-kapt-worker-jvm-v1"
                && executable.is_absolute()
                && build_home.is_absolute()
                && same_path(executable, &expected_executable)
                && same_path(build_home, &runtime.gradle_java_home)
                && worker["jvmArgs"] == serde_json::json!([]),
            "kapt-worker-jvm-unproved"
        );
        for (task, kind, declaring, action, kapt) in [
            (
                &self.ksp,
                "com.google.devtools.ksp.gradle.KspTaskJvm_Decorated",
                "org.jetbrains.kotlin.gradle.tasks.AbstractKotlinCompile",
                "execute",
                false,
            ),
            (
                &self.stubs,
                "org.jetbrains.kotlin.gradle.internal.KaptGenerateStubsTask_Decorated",
                "org.jetbrains.kotlin.gradle.tasks.AbstractKotlinCompile",
                "execute",
                false,
            ),
            (
                &self.kapt,
                "org.jetbrains.kotlin.gradle.internal.KaptWithoutKotlincTask_Decorated",
                "org.jetbrains.kotlin.gradle.internal.KaptWithoutKotlincTask",
                "compile",
                true,
            ),
        ] {
            anyhow::ensure!(
                task.complete
                    && task.problem.is_none()
                    && task.task_type == kind
                    && (kapt || task.java_home == primary.java_home)
                    && stock_task_action(task, declaring, action)
                    && stock_only_if(task, kapt)
                    && task.options["taskActionInventory"]["adapter"]
                        == "gradle-7.4.2-stock-actions-v1"
                    && task.options["taskOnlyIfInventory"]["adapter"]
                        == "gradle-7.4.2-stock-onlyif-v1",
                "processor-stock-task-contract-unproved"
            );
        }
        let inputs = self
            .ksp
            .options
            .get("kspProcessorInputs")
            .ok_or_else(|| anyhow::anyhow!("ksp-configured-inputs-absent"))?;
        anyhow::ensure!(
            self.ksp.options.get("kspProcessorInputsProblem") == Some(&Value::Null)
                && inputs["adapter"] == "gradle-7.4.2-ksp-1.8.21-1.0.11-configured-inputs-v1"
                && inputs["basis"] == "stock-callCompilerAsync-transformer-fields"
                && inputs["commandLineArgumentProviderCount"] == 0
                && inputs["producerCompletion"] == false
                && inputs["apiAuthority"] == false,
            "ksp-configured-inputs-unproved"
        );
        let pins = (
            value_string(inputs, "taskClassSha256")?,
            value_string(inputs, "transformerClassSha256")?,
        );
        anyhow::ensure!(
            matches!(
                pins,
                (
                    "17f978d4c95c090f1441aa1bd9ef95778743193c2777953525d006b92a84552f",
                    "37746fc33ef72b4972f51e72dc579f697e1d084fd4f4e4f3b3bf6b3392236dab"
                ) | (
                    "2f242870186225d3a6bff2ae134213ac6363423a3114efcb109c26c3e13c5c29",
                    "daf4803ac34409a1b641f49447ba0fd96acb5b5e2d3cdfd3d2851ea79995983d"
                )
            ),
            "ksp-stock-implementation-unproved"
        );
        anyhow::ensure!(
            !array(inputs, "processorClasspath")?.is_empty()
                && array(inputs, "processorClasspath")?
                    .iter()
                    .all(|p| p.as_str().is_some_and(|s| Path::new(s).is_absolute()))
                && self
                    .kapt
                    .options
                    .get("annotationProcessorOptionProviderCount")
                    == Some(&Value::from(0))
                && self.kapt.options.get("providerArguments") == Some(&serde_json::json!([]))
                && !self.kapt.annotation_processor_path.is_empty(),
            "processor-classpath-or-provider-unproved"
        );
        // Artifact contents are dynamic inputs, deliberately NOT an implementation allowlist.
        Ok(())
    }
}

/// A completed output is watched before its first read by the following phase. Its invalidation
/// token belongs to the lease, not to the report: retiring private files after publication must
/// not invalidate an otherwise current source report.
struct SealedOutput {
    roots: Vec<PathBuf>,
    facts: Vec<Value>,
    invalid: Arc<AtomicBool>,
    _watch: Arc<crate::compiled_watch::NativeInputWatch>,
}

impl SealedOutput {
    fn seal(roots: Vec<PathBuf>) -> anyhow::Result<Self> {
        let invalid = Arc::new(AtomicBool::new(false));
        let plan = crate::compiled_watch::WatchPlan::read_inputs(&roots)?;
        let (wake, _receiver) = tokio::sync::mpsc::channel(1);
        let watch = crate::compiled_watch::NativeInputWatch::new(invalid.clone(), plan, wake)?;
        // Coverage precedes the first inventory; absence and membership are reads too.
        let facts = roots
            .iter()
            .map(|p| inventory(p))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let lease = Self {
            roots,
            facts,
            invalid,
            _watch: watch,
        };
        lease.check_live()?;
        Ok(lease)
    }

    fn check_live(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.invalid.load(Ordering::Acquire),
            "processor-sealed-output-changed"
        );
        for (path, expected) in self.roots.iter().zip(&self.facts) {
            anyhow::ensure!(
                inventory(path)? == *expected,
                "processor-sealed-output-inventory-changed"
            );
        }
        anyhow::ensure!(
            !self.invalid.load(Ordering::Acquire),
            "processor-sealed-output-changed"
        );
        Ok(())
    }
}

/// Only the fixed executor below can construct a completed generated-source capability.
/// Keep it alive through Kotlin API preparation and the complete selected Javac task.
pub(crate) struct GeneratedSources {
    generation: u64,
    selected_inputs: String,
    sealed: Vec<SealedOutput>,
    sources: BTreeMap<PathBuf, String>,
}

fn selected_inputs(request: &KotlinDiagnosticRequest) -> anyhow::Result<String> {
    let overlays: Vec<_> = request
        .overlays
        .iter()
        .chain(&request.dependency_overlays)
        .map(|s| (&s.key, &s.text_sha256))
        .collect();
    Ok(sha(&serde_json::to_vec(&(
        request.generation,
        &request.task,
        &request.java_task,
        &request.sources,
        &request.source_roots,
        overlays,
        &request.visible_origins,
        &request.java_editor_sources,
    ))?))
}

impl GeneratedSources {
    pub(super) fn check_live(&self, request: &KotlinDiagnosticRequest) -> anyhow::Result<()> {
        request.check_live()?;
        anyhow::ensure!(
            self.generation == request.generation
                && self.selected_inputs == selected_inputs(request)?,
            "processor-source-generation-changed"
        );
        for sealed in &self.sealed {
            sealed.check_live()?;
        }
        for (path, digest) in &self.sources {
            anyhow::ensure!(
                sha(&std::fs::read(path)?) == *digest,
                "processor-generated-source-changed"
            );
        }
        Ok(())
    }

    pub(super) fn source_digest(&self, path: &Path) -> Option<&str> {
        self.sources
            .iter()
            .find(|(p, _)| same_path(p, path))
            .map(|(_, s)| s.as_str())
    }
}

#[cfg(test)]
pub(super) fn current_generated_sources_for_test(
    request: &KotlinDiagnosticRequest,
    paths: &[PathBuf],
) -> anyhow::Result<GeneratedSources> {
    // Tests exercise real file watches and bytes; this constructor is absent from production.
    Ok(GeneratedSources {
        generation: request.generation,
        selected_inputs: selected_inputs(request)?,
        sealed: vec![SealedOutput::seal(paths.to_vec())?],
        sources: paths
            .iter()
            .map(|p| Ok((p.clone(), sha(&std::fs::read(p)?))))
            .collect::<anyhow::Result<_>>()?,
    })
}

/// A native phase completion cannot be read from JSON. In particular, root0 alone, an earlier
/// build, cancelled execution, a surviving pipe holder, or an unclosed daemon cannot mint it.
fn execute_phase(
    command: &mut Command,
    request: &KotlinDiagnosticRequest,
    environment: &KotlinDiagnosticEnvironment,
) -> anyhow::Result<()> {
    let (exit, _) = native(command, request, environment)?;
    anyhow::ensure!(exit == 0, "processor-native-phase-failed");
    Ok(())
}

fn successful_state(value: &Value) -> anyhow::Result<()> {
    for (key, expected) in [
        ("executed", true),
        ("skipped", false),
        ("up_to_date", false),
        ("no_source", false),
        ("did_work", true),
    ] {
        require_bool(value, key, expected)?;
    }
    anyhow::ensure!(
        value.get("failure") == Some(&Value::Null),
        "processor-task-failed"
    );
    Ok(())
}

/// Observer files are descriptive. Validate them only after OUR native invocation closed.
fn phase_events(
    directory: &Path,
    nonce: &str,
    selected: &CompilerTask,
) -> anyhow::Result<Vec<Value>> {
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        anyhow::ensure!(
            entry.file_type()?.is_file() && paths.len() < 256,
            "processor-event-membership-unproved"
        );
        paths.push(entry.path());
    }
    paths.sort();
    let mut events = Vec::new();
    let mut bytes = 0usize;
    for (sequence, path) in paths.iter().enumerate() {
        bytes = bytes
            .checked_add(std::fs::metadata(path)?.len() as usize)
            .ok_or_else(|| anyhow::anyhow!("processor-event-overflow"))?;
        anyhow::ensure!(bytes <= 16 * 1024 * 1024, "processor-event-cap");
        let event = read_json(path)?;
        anyhow::ensure!(
            event["nonce"] == nonce && event["sequence"] == sequence as u64,
            "processor-event-owner-or-order-differs"
        );
        events.push(event);
    }
    let starts: Vec<_> = events
        .iter()
        .filter(|e| e["event"] == "processor-start" && e["task"] == selected.task)
        .collect();
    let ends: Vec<_> = events
        .iter()
        .filter(|e| e["event"] == "processor-task-end" && e["task"] == selected.task)
        .collect();
    anyhow::ensure!(
        starts.len() == 1
            && ends.len() == 1
            && starts[0]["sequence"].as_u64() < ends[0]["sequence"].as_u64(),
        "processor-selected-boundaries-unproved"
    );

    successful_state(&ends[0]["state"])?;
    let complete: Vec<_> = events
        .iter()
        .filter(|e| e["event"] == "processor-build-finished")
        .collect();
    anyhow::ensure!(
        complete.len() == 1 && complete[0].get("failure") == Some(&Value::Null),
        "processor-build-unclosed"
    );
    Ok(events)
}

#[path = "processor_snapshot.rs"]
mod snapshot;
pub(crate) use snapshot::CachedArtifacts;
pub(super) const OBSERVER: &str =
    include_str!("../../../../scripts/processor-observer.init.fragment.gradle.kts");
pub(crate) fn run(
    request: &KotlinDiagnosticRequest,
    environment: &KotlinDiagnosticEnvironment,
    plan: &Plan,
) -> anyhow::Result<KotlinDiagnosticOutcome> {
    snapshot::run(request, environment, plan)
}
