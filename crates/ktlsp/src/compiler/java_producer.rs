//! Replays the selected Gradle Java task; an unsupported standalone javac recipe stays unsupported.
use super::*;
use serde_json::{json, Value};
use std::collections::BTreeSet;

const SCRIPT: &str = include_str!("../../../../scripts/java-producer-overlay.init.gradle");

#[derive(Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Plan {
    root: PathBuf,
    module_dirs: Vec<PathBuf>,
    java_roots: Vec<PathBuf>,
    included_dirs: Vec<PathBuf>,
}

impl Plan {
    pub(crate) fn capture(
        owner: &crate::classpath::ModuleClasspath,
        source: &crate::classpath::SourceSetClasspath,
        model: &crate::classpath::ProjectSnapshot,
    ) -> Option<Self> {
        use crate::classpath::CompilerLanguage;
        let [task] = source.compiler_tasks.as_slice() else {
            return None;
        };
        let policy = task.options.get("sdkSourcePolicy")?;
        if policy["adapter"] != "gradle-source-tree-stock-patterns-v1"
            || policy["language"] != "java"
        {
            return None;
        }
        let java_roots: Vec<PathBuf> =
            serde_json::from_value(policy["authored_roots"].clone()).ok()?;
        if owner.build != ":"
            || source.name != "main"
            || task.language != CompilerLanguage::Java
            || !task.complete
            || task.problem.is_some()
            || task.java_home.is_none()
            || !utf8_sources(task)
            || task.output_roots.len() != 1
            || task.source_files.is_empty()
            || !task.task.ends_with(":compileJava")
            || source
                .source_roots
                .iter()
                .any(|root| !root.starts_with(&owner.project_dir))
            || java_roots.is_empty()
            || java_roots
                .iter()
                .any(|root| !root.starts_with(&owner.project_dir))
            || task
                .source_files
                .iter()
                .any(|path| !java_roots.iter().any(|root| path.starts_with(root)))
            || !matches!(
                task.task_type.as_str(),
                "org.gradle.api.tasks.compile.JavaCompile"
                    | "org.gradle.api.tasks.compile.JavaCompile_Decorated"
            )
            || task
                .source_files
                .iter()
                .any(|path| path.extension().and_then(|s| s.to_str()) != Some("java"))
        {
            return None;
        }
        let module_dirs: Vec<_> = model
            .modules
            .iter()
            .filter(|module| module.build == owner.build)
            .map(|module| module.project_dir.clone())
            .collect();
        let root = owner
            .project_dir
            .ancestors()
            .find(|root| {
                module_dirs
                    .iter()
                    .all(|directory| directory.starts_with(root))
            })?
            .to_owned();
        let included_dirs = model
            .modules
            .iter()
            .filter(|module| module.build != owner.build)
            .map(|module| module.project_dir.clone())
            .collect();
        let module_dirs = model
            .modules
            .iter()
            .map(|module| module.project_dir.clone())
            .collect();
        Some(Self {
            root,
            module_dirs,
            java_roots,
            included_dirs,
        })
    }

    fn configuration_roots(&self) -> anyhow::Result<Vec<PathBuf>> {
        let mut roots = BTreeSet::new();
        for directory in &self.included_dirs {
            let build = directory
                .ancestors()
                .find(|root| {
                    root.join("settings.gradle").is_file()
                        || root.join("settings.gradle.kts").is_file()
                })
                .ok_or_else(|| anyhow::anyhow!("java-producer-included-build-root-unknown"))?;
            roots.insert(build.to_owned());
        }
        roots.insert(self.root.join("buildSrc"));
        // Models may omit configuration-only included builds; verify discovered roots again in Gradle.
        for (visited, entry) in walkdir::WalkDir::new(&self.root)
            .max_depth(4)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                !matches!(
                    entry.file_name().to_str(),
                    Some("build" | ".gradle" | ".git" | "src" | "node_modules" | "target")
                )
            })
            .enumerate()
        {
            anyhow::ensure!(
                visited < 100_000,
                "java-producer-configuration-discovery-cap"
            );
            let entry = entry?;
            anyhow::ensure!(
                !entry.file_type().is_symlink(),
                "java-producer-config-symlink"
            );
            if entry.file_type().is_file()
                && matches!(
                    entry.file_name().to_str(),
                    Some("settings.gradle" | "settings.gradle.kts")
                )
            {
                let directory = entry.path().parent().unwrap();
                if directory != self.root {
                    roots.insert(directory.to_owned());
                }
            }
            anyhow::ensure!(roots.len() <= 256, "java-producer-build-root-cap");
        }
        let nested: Vec<_> = roots.iter().map(|root| root.join("buildSrc")).collect();
        roots.extend(nested);
        Ok(roots.into_iter().collect())
    }

    pub(super) fn watch_inputs(
        &self,
        source: &crate::classpath::SourceSetClasspath,
        project_dir: &Path,
    ) -> anyhow::Result<Vec<PathBuf>> {
        let task = &source.compiler_tasks[0];
        let home = task
            .java_home
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("java-producer-sdk-unknown"))?;
        let mut inputs: BTreeSet<PathBuf> = task
            .source_files
            .iter()
            .chain(&task.classpath)
            .chain(&task.annotation_processor_path)
            .chain(&task.declared_input_files)
            .chain(&source.source_roots)
            .cloned()
            .collect();
        inputs.extend([
            home.join("release"),
            home.join("bin"),
            home.join("conf"),
            home.join("lib/ct.sym"),
            home.join("lib/jrt-fs.jar"),
            home.join("lib/jvm.cfg"),
        ]);
        if home.join("lib/modules").is_file() {
            inputs.insert(home.join("lib/modules"));
        } else {
            anyhow::ensure!(
                home.join("jre/lib/rt.jar").is_file(),
                "java-producer-sdk-layout-unknown"
            );
            inputs.extend([home.join("jre/lib"), home.join("lib/tools.jar")]);
        }
        for directory in self.module_dirs.iter().chain(std::iter::once(&self.root)) {
            inputs.extend([
                directory.join("build.gradle"),
                directory.join("build.gradle.kts"),
                directory.join("gradle.properties"),
            ]);
        }
        inputs.extend([
            self.root.join("settings.gradle"),
            self.root.join("settings.gradle.kts"),
            self.root.join("gradlew.bat"),
            self.root.join("gradle"),
            gradle_home()?.join("gradle.properties"),
            gradle_home()?.join("init.d"),
            project_dir.join("src/main/resources"),
        ]);
        for directory in self.configuration_roots()? {
            match std::fs::symlink_metadata(&directory) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    // A later buildSrc creation changes configuration without a settings edit.
                    inputs.insert(directory.clone());
                    anyhow::ensure!(inputs.len() <= 4096, "java-producer-readset-cap");
                    continue;
                }
                Err(error) => return Err(error.into()),
                Ok(_) => {}
            }
            for entry in walkdir::WalkDir::new(&directory)
                .follow_links(false)
                .into_iter()
                .filter_entry(|entry| {
                    !matches!(
                        entry.file_name().to_str(),
                        Some("build" | ".gradle" | ".git")
                    )
                })
            {
                let entry = entry?;
                anyhow::ensure!(
                    !entry.file_type().is_symlink(),
                    "java-producer-config-symlink"
                );
                if entry.file_type().is_file() || entry.file_name() == "src" {
                    inputs.insert(entry.path().to_owned());
                }
                if entry.file_type().is_file()
                    && matches!(
                        entry.file_name().to_str(),
                        Some("build.gradle" | "build.gradle.kts")
                    )
                {
                    let libraries = entry.path().parent().unwrap().join("build/libs");
                    if libraries.is_dir() {
                        for artifact in std::fs::read_dir(libraries)? {
                            let artifact = artifact?;
                            if artifact
                                .path()
                                .extension()
                                .and_then(|extension| extension.to_str())
                                == Some("jar")
                            {
                                inputs.insert(artifact.path());
                            }
                        }
                    }
                }
                anyhow::ensure!(inputs.len() <= 4096, "java-producer-readset-cap");
            }
        }
        Ok(inputs.into_iter().collect())
    }
}

fn utf8_sources(task: &crate::classpath::CompilerTask) -> bool {
    match task.options.get("encoding") {
        Some(Value::String(encoding)) => {
            encoding.eq_ignore_ascii_case("UTF-8") || encoding.eq_ignore_ascii_case("UTF8")
        }
        None | Some(Value::Null) => task.java_compiler_recipe().is_some_and(|recipe| {
            recipe.effective_charset.eq_ignore_ascii_case("UTF-8")
                || recipe.effective_charset.eq_ignore_ascii_case("UTF8")
        }),
        _ => false,
    }
}

fn gradle_home() -> anyhow::Result<PathBuf> {
    std::env::var_os("GRADLE_USER_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".gradle")))
        .ok_or_else(|| anyhow::anyhow!("java-producer-gradle-home-unknown"))
}

fn sha(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn expected_recipe(
    task: &crate::classpath::CompilerTask,
    sources: &[Value],
) -> anyhow::Result<Value> {
    let options = &task.options;
    let processors = task
        .annotation_processor_path
        .iter()
        .map(|path| {
            Ok(json!({"path": path,
        "sha256": sha(&std::fs::read(path)?)}))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(
        json!({"javaHome": task.java_home, "sourceCompatibility": options.get("sourceCompatibility"),
        "targetCompatibility": options.get("targetCompatibility"), "release": options.get("release"),
        "encoding": options.get("encoding"), "fork": options.get("fork"),
        "forkJavaHome": options.get("forkJavaHome"), "forkExecutable": options.get("forkExecutable"),
        "forkJvmArgs": options.get("forkJvmArgs"), "compilerArguments": task.arguments,
        "annotationProcessors": processors,
        "sources": sources.iter().map(|source| json!({"path": source["original"], "sha256": source["originalSha256"]})).collect::<Vec<_>>(),
        "classpath": task.classpath, "destination": task.output_roots[0]}),
    )
}

fn events(path: &Path, nonce: &str) -> anyhow::Result<Vec<Value>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= 8 * 1024 * 1024,
        "java-producer-event-byte-cap"
    );
    let text = std::str::from_utf8(&bytes)?;
    let mut events = Vec::new();
    for line in text.lines() {
        let event: Value = serde_json::from_str(line)?;
        anyhow::ensure!(
            events.len() < 1024
                && event["schema"] == 1
                && event["nonce"] == nonce
                && event["sequence"] == events.len() as u64,
            "java-producer-event-owner-or-order"
        );
        events.push(event);
    }
    Ok(events)
}

fn one<'a>(events: &'a [Value], name: &str) -> anyhow::Result<&'a Value> {
    let matches: Vec<_> = events
        .iter()
        .filter(|event| event["event"] == name)
        .collect();
    anyhow::ensure!(matches.len() == 1, "java-producer-boundary-{name}");
    Ok(matches[0])
}

fn diagnostics(
    events: &[Value],
    overlay: &KotlinEditorOverlay,
) -> anyhow::Result<Vec<CompileDiagnostic>> {
    let mut diagnostics = Vec::new();
    for event in events.iter().filter(|event| event["event"] == "diagnostic") {
        let severity = match event["severity"].as_str() {
            Some("ERROR") => Severity::Error,
            Some("WARNING") => Severity::Warning,
            Some("ADVICE") => continue,
            _ => anyhow::bail!("java-producer-diagnostic-severity-unknown"),
        };
        let message = event["message"]
            .as_str()
            .filter(|text| !text.is_empty())
            .ok_or_else(|| anyhow::anyhow!("java-producer-diagnostic-message-unknown"))?;
        let locations = event["locations"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("java-producer-diagnostic-location-unknown"))?;
        let mut valid_locations = 0;
        let mut mapped_locations = 0;
        let mut distinct_valid_mapped = BTreeSet::new();
        let mut location_types = BTreeSet::new();
        for location in locations {
            if let Some(kind) = location["type"].as_str().filter(|kind| kind.len() <= 256) {
                if location_types.len() < 8 {
                    location_types.insert(kind);
                }
            }
            let position = location["line"]
                .as_u64()
                .zip(location["column"].as_u64())
                .filter(|(line, column)| {
                    *line > 0
                        && *column > 0
                        && *line <= u32::MAX as u64
                        && *column <= u32::MAX as u64
                });
            valid_locations += usize::from(position.is_some());
            if let Some(owner) = location["path"]
                .as_str()
                .and_then(|path| overlay.paths.get(&kotlin_path_key(path)))
            {
                mapped_locations += 1;
                if let Some((line, column)) = position {
                    distinct_valid_mapped.insert((owner, line, column));
                }
            }
        }
        crate::trace::span(
            "compiler.java_producer_diagnostic_locations",
            "compiler",
            std::time::Instant::now(),
            json!({"locations":locations.len(),"valid_positions":valid_locations,
                "mapped_locations":mapped_locations,"distinct_valid_mapped":distinct_valid_mapped.len(),
                "unmapped_locations":locations.len()-mapped_locations,"location_types":location_types,
                "origin_file_locations":event["originFileLocations"],
                "raw_selected_locations":event["rawSelectedLocations"],
                "normalized_selected_locations":event["selectedLocations"],
                "origin_location_types":event["originLocationTypes"]}),
        );
        anyhow::ensure!(
            locations.len() == 1,
            "java-producer-diagnostic-location-ambiguous"
        );
        let location = &locations[0];
        let key = kotlin_path_key(
            location["path"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("java-producer-diagnostic-path-unknown"))?,
        );
        let original = overlay
            .paths
            .get(&key)
            .ok_or_else(|| anyhow::anyhow!("java-producer-diagnostic-path-unmapped"))?;
        let coordinate = |name: &str| -> anyhow::Result<u32> {
            Ok(u32::try_from(
                location[name]
                    .as_u64()
                    .filter(|value| *value > 0)
                    .ok_or_else(|| anyhow::anyhow!("java-producer-diagnostic-position-unknown"))?,
            )?)
        };
        diagnostics.push(CompileDiagnostic {
            path: original.clone(),
            line: coordinate("line")?,
            col: coordinate("column")?,
            severity,
            message: message.to_owned(),
        });
    }
    Ok(diagnostics)
}

pub(super) fn run(
    consumer: &KotlinDiagnosticRequest,
    environment: &KotlinDiagnosticEnvironment,
    plan: &KotlinDependencyCompilation,
    replay: &Plan,
) -> anyhow::Result<Result<CompiledKotlinDependency, KotlinDiagnosticOutcome>> {
    let java = &plan.source.compiler_tasks[0];
    let selected: BTreeSet<_> = java
        .source_files
        .iter()
        .map(|path| crate::paths::file_key(path))
        .collect();
    let mut producer = consumer.clone();
    producer.project_dir = plan.project_dir.clone();
    producer.scope = plan.scope.clone();
    producer.task = java.clone();
    producer.java_task = None;
    producer.java_dependencies.clear();
    producer.java_editor_sources.clear();
    producer.processor_plan = None;
    producer.dependency_compilation = None;
    producer.source_roots = plan.source.source_roots.clone();
    producer.sources = java.source_files.clone();
    (producer.overlays, producer.dependency_overlays) = consumer
        .dependency_overlays
        .iter()
        .cloned()
        .partition(|source| selected.contains(&source.key));
    let overlay = KotlinEditorOverlay::create(&producer)?;
    let mut sources = Vec::new();
    for (original, private) in producer.sources.iter().zip(&overlay.sources) {
        sources.push(json!({"original": original, "private": private, "originalSha256": sha(&std::fs::read(original)?), "privateSha256": sha(&std::fs::read(private)?)}));
    }
    sources.sort_by(|a, b| a["original"].as_str().cmp(&b["original"].as_str()));
    let recipe = expected_recipe(java, &sources)?;
    anyhow::ensure!(
        recipe["annotationProcessors"].is_array(),
        "java-producer-processor-input-unreadable"
    );
    let source_roots: Vec<_> = replay.java_roots.iter().map(|root| Ok(json!({"original": root,
        "private": overlay.directory.join("sources/project").join(root.strip_prefix(&producer.project_dir)?)})))
        .collect::<anyhow::Result<_>>()?;
    let archive_task = format!(
        "{}:jar",
        java.task
            .strip_suffix(":compileJava")
            .ok_or_else(|| anyhow::anyhow!("java-producer-task-role-unknown"))?
    );
    let event_file = overlay.directory.join("events.jsonl");
    let nonce = format!("{}:{}", std::process::id(), overlay.directory.display());
    let input = overlay.directory.join("request.json");
    let script = overlay.directory.join("producer.init.gradle");
    std::fs::write(&script, SCRIPT)?;
    std::fs::write(
        &input,
        serde_json::to_vec(&json!({"schema": 1, "nonce": nonce, "root": replay.root,
        "projectDir": plan.project_dir, "task": java.task, "events": event_file, "expectedRecipe": recipe,
        "sources": sources, "classes": overlay.directory.join("classes"), "generated": overlay.directory.join("generated"),
        "headers": overlay.directory.join("headers"), "gradleUserHome": gradle_home()?,
        "sourceRoots": source_roots, "metadataRoot": overlay.directory.join("metadata"),
        "jarDirectory": overlay.directory.join("archive"), "resourcesRoot": overlay.directory.join("resources"),
        "includedBuildRoots": replay.configuration_roots()?}))?,
    )?;
    producer.check_live()?;
    environment.check_live()?;
    let wrapper = resolve_gradle(&replay.root)
        .ok_or_else(|| anyhow::anyhow!("java-producer-wrapper-unknown"))?;
    let mut command = Command::new(wrapper);
    command
        .current_dir(&replay.root)
        .env("JAVA_HOME", &environment.key.recipe.gradle_java_home)
        .arg(&archive_task)
        .args([
            "--no-daemon",
            "--console=plain",
            "--no-configuration-cache",
            "--no-scan",
            "--init-script",
        ])
        .arg(&script)
        .arg(format!("-Dktlsp.java.producer.request={}", input.display()));
    #[cfg(windows)]
    let exit = {
        let (status, interrupted, _, out_complete, _, err_complete) =
            OwnedNativeCompile::spawn(&mut command, MAX_OUTPUT_BYTES)?
                .wait(COMPILE_TIMEOUT, Some(consumer.invalid.as_ref()));
        anyhow::ensure!(
            !interrupted && out_complete && err_complete,
            "java-producer-native-incomplete"
        );
        status.code()
    };
    #[cfg(not(windows))]
    let exit: Option<i32> = {
        let _ = command;
        anyhow::bail!("java-producer-native-proof-unavailable")
    };
    producer.check_live()?;
    environment.check_live()?;
    anyhow::ensure!(
        std::fs::read(&script)? == SCRIPT.as_bytes(),
        "java-producer-adapter-changed"
    );
    for source in &sources {
        anyhow::ensure!(
            sha(&std::fs::read(source["original"].as_str().unwrap())?) == source["originalSha256"]
                && sha(&std::fs::read(source["private"].as_str().unwrap())?)
                    == source["privateSha256"],
            "java-producer-source-changed"
        );
    }
    let events = events(&event_file, &nonce)?;
    let start = one(&events, "compiler-start")?;
    let end = one(&events, "task-end")?;
    let finished = one(&events, "build-end")?;
    anyhow::ensure!(
        start["sequence"].as_u64() < end["sequence"].as_u64()
            && end["sequence"].as_u64() < finished["sequence"].as_u64()
            && end["executed"] == true
            && end["didWork"] == true
            && end["skipped"] == false
            && end["noSource"] == false
            && end["upToDate"] == false,
        "java-producer-task-not-executed"
    );
    let severity_counts = events
        .iter()
        .filter(|event| event["event"] == "diagnostic")
        .fold([0usize; 4], |mut counts, event| {
            counts[match event["severity"].as_str() {
                Some("ERROR") => 0,
                Some("WARNING") => 1,
                Some("ADVICE") => 2,
                _ => 3,
            }] += 1;
            counts
        });
    crate::trace::span(
        "compiler.java_producer_finished",
        "compiler",
        std::time::Instant::now(),
        json!({"exit":exit,"events":events.len(),"task_failed":!end["failure"].is_null(),
            "build_failed":!finished["failure"].is_null(),"error_events":severity_counts[0],
            "warning_events":severity_counts[1],"advice_events":severity_counts[2],
            "unknown_severity_events":severity_counts[3]}),
    );
    let diagnostics = diagnostics(&events, &overlay)?;
    if exit == Some(1) {
        anyhow::ensure!(
            !end["failure"].is_null()
                && !finished["failure"].is_null()
                && diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.severity == Severity::Error),
            "java-producer-failure-unlocated"
        );
        return Ok(Err(KotlinDiagnosticOutcome::Verified {
            diagnostics,
            clean: false,
        }));
    }
    anyhow::ensure!(
        exit == Some(0)
            && end["failure"].is_null()
            && finished["failure"].is_null()
            && diagnostics
                .iter()
                .all(|diagnostic| diagnostic.severity != Severity::Error),
        "java-producer-compile-failed"
    );
    let completed = one(&events, "compiler-finish")?;
    anyhow::ensure!(
        start["recipe"] == completed["recipe"],
        "java-producer-recipe-changed"
    );
    let archive_start = one(&events, "archive-start")?;
    let archive_finish = one(&events, "archive-finish")?;
    anyhow::ensure!(
        completed["sequence"].as_u64() < archive_start["sequence"].as_u64()
            && archive_start["sequence"].as_u64() < archive_finish["sequence"].as_u64()
            && archive_finish["sequence"].as_u64() < finished["sequence"].as_u64()
            && archive_start["sources"] == archive_finish["sources"],
        "java-producer-archive-boundary-order-or-inputs"
    );
    for metadata in events
        .iter()
        .filter(|event| event["event"] == "metadata-start")
    {
        let ends: Vec<_> = events
            .iter()
            .filter(|event| {
                event["event"] == "metadata-finish" && event["taskPath"] == metadata["taskPath"]
            })
            .collect();
        anyhow::ensure!(
            ends.len() == 1
                && metadata["sequence"].as_u64() < ends[0]["sequence"].as_u64()
                && ends[0]["sequence"].as_u64() < archive_start["sequence"].as_u64(),
            "java-producer-metadata-boundary-order"
        );
        if metadata["type"] == "gradlebuild.kotlin.tasks.ParameterNamesIndex_Decorated" {
            anyhow::ensure!(
                completed["sequence"].as_u64() < metadata["sequence"].as_u64(),
                "java-producer-metadata-before-classes"
            );
        }
    }
    let artifact = overlay.directory.join("archive/producer.jar");
    let sealed = java_diagnostics::processor::SealedOutput::seal(vec![artifact.clone()])?;
    anyhow::ensure!(
        archive_finish["destination"].as_str().is_some_and(
            |path| kotlin_path_key(path) == kotlin_path_key(&crate::paths::file_key(&artifact))
        ) && sha(&std::fs::read(&artifact)?) == archive_finish["sha256"],
        "java-producer-archive-output-changed"
    );
    let outputs = vec![artifact];
    let output_inventory = outputs
        .iter()
        .map(|path| inventory(path))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let compiled = CompiledKotlinDependency {
        _owner: overlay,
        producer,
        environment: None,
        artifact: plan.artifact.clone(),
        outputs,
        output_inventory,
        sealed: vec![sealed],
    };
    compiled.check_live()?;
    Ok(Ok(compiled))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlay() -> KotlinEditorOverlay {
        let root = std::env::temp_dir().join(format!(
            "ktlsp-java-producer-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        KotlinEditorOverlay {
            directory: root,
            paths: Default::default(),
            sources: Vec::new(),
        }
    }

    #[test]
    fn captured_source_levels_are_preserved_instead_of_selecting_a_fixture_version() {
        use crate::classpath::{
            CompilerLanguage, ModuleClasspath, ProjectSnapshot, SourceSetClasspath,
        };
        let owner = overlay();
        let root = &owner.directory;
        for level in ["6", "8", "17", "21"] {
            let mut task = kotlin_test_request(root).task;
            task.language = CompilerLanguage::Java;
            task.task = ":producer:compileJava".into();
            task.task_type = "org.gradle.api.tasks.compile.JavaCompile_Decorated".into();
            task.source_files = vec![root.join("src/main/java/Owner.java")];
            task.options
                .insert("sourceCompatibility".into(), json!(level));
            task.options
                .insert("targetCompatibility".into(), json!(level));
            task.options.insert("encoding".into(), json!("UTF-8"));
            task.options.insert(
                "sdkSourcePolicy".into(),
                json!({"adapter":"gradle-source-tree-stock-patterns-v1",
                "language":"java", "authored_roots":[root.join("src/main/java")]}),
            );
            let source = SourceSetClasspath {
                name: "main".into(),
                source_roots: vec![root.join("src/main/java")],
                compiler_tasks: vec![task.clone()],
                ..Default::default()
            };
            let module = ModuleClasspath {
                module: ":producer".into(),
                project_dir: root.clone(),
                build: ":".into(),
                source_sets: vec![source.clone()],
                entries: Vec::new(),
                unresolved: Vec::new(),
            };
            let model = ProjectSnapshot::new(vec![module.clone()]);
            assert!(Plan::capture(&module, &source, &model).is_some(), "{level}");
            let recipe = expected_recipe(&task, &[]).unwrap();
            assert_eq!(recipe["sourceCompatibility"], level);
            assert_eq!(recipe["targetCompatibility"], level);
        }
    }

    #[test]
    fn editor_utf8_never_guesses_an_unknown_or_different_java_source_charset() {
        let owner = overlay();
        let mut task = kotlin_test_request(&owner.directory).task;
        assert!(!utf8_sources(&task));
        for encoding in ["UTF-8", "utf-8", "UTF8"] {
            task.options.insert("encoding".into(), json!(encoding));
            assert!(utf8_sources(&task), "{encoding}");
        }
        for encoding in [
            json!("MS932"),
            json!("UTF-16"),
            json!("ISO-8859-1"),
            Value::Null,
        ] {
            task.options.insert("encoding".into(), encoding.clone());
            assert!(!utf8_sources(&task), "{encoding}");
        }
        task.language = crate::classpath::CompilerLanguage::Java;
        let recipe = crate::classpath::JavaCompilerRecipe {
            adapter: "gradle-7.4.2-current-jdk17-javac-api-v1".into(),
            compiler_type: "org.gradle.jvm.toolchain.internal.DefaultToolchainJavaCompiler".into(),
            java_runtime_version: "17.0.7+8".into(),
            language_version: "17".into(),
            gradle_java_version: "17.0.7".into(),
            gradle_file_encoding: "UTF-8".into(),
            effective_charset: "UTF-8".into(),
            locale: "ja-JP".into(),
            api_options: vec!["-source".into(), "17".into()],
        };
        task.options.insert(
            "javaCompilerRecipe".into(),
            serde_json::to_value(recipe).unwrap(),
        );
        assert!(
            utf8_sources(&task),
            "a verified effective default is reusable"
        );
        task.options.insert("encoding".into(), json!("MS932"));
        assert!(
            !utf8_sources(&task),
            "explicit source encoding wins over the JVM default"
        );
    }

    #[test]
    fn configuration_builds_come_from_settings_and_standard_build_src() {
        let owner = overlay();
        let included = owner.directory.join("arbitrary-plugin-build");
        std::fs::create_dir(&included).unwrap();
        std::fs::write(
            included.join("settings.gradle.kts"),
            "rootProject.name = \"configuration\"",
        )
        .unwrap();
        let build_src = owner.directory.join("buildSrc");
        std::fs::create_dir(&build_src).unwrap();
        let plan = Plan {
            root: owner.directory.clone(),
            module_dirs: Vec::new(),
            java_roots: Vec::new(),
            included_dirs: Vec::new(),
        };
        let roots = plan.configuration_roots().unwrap();
        assert!(roots.contains(&included));
        assert!(roots.contains(&build_src));
        assert!(roots.contains(&included.join("buildSrc")));
        assert!(!included.join("buildSrc").exists());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn missing_build_src_read_invalidates_consumer_when_created() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        let owner = overlay();
        let root = &owner.directory;
        std::fs::create_dir_all(root.join("jdk/lib")).unwrap();
        std::fs::write(root.join("jdk/lib/modules"), b"test runtime").unwrap();
        let included = root.join("configuration");
        std::fs::create_dir_all(included.join("build")).unwrap();
        std::fs::write(included.join("settings.gradle.kts"), "").unwrap();
        let plan = Plan {
            root: root.clone(),
            module_dirs: Vec::new(),
            java_roots: Vec::new(),
            included_dirs: Vec::new(),
        };
        let source = crate::classpath::SourceSetClasspath {
            compiler_tasks: vec![kotlin_test_request(root).task],
            ..Default::default()
        };
        let inputs = plan.watch_inputs(&source, root).unwrap();
        let build_src = root.join("buildSrc");
        assert!(inputs.contains(&build_src));
        assert!(!build_src.exists());
        assert!(inputs.contains(&included.join("buildSrc")));
        assert!(
            !inputs.contains(&included),
            "existing build output remains excluded"
        );
        // Keep this native probe inside the disposable fixture, excluding the user's Gradle home.
        let local_inputs: Vec<_> = inputs
            .into_iter()
            .filter(|input| input.starts_with(root))
            .collect();
        let invalid = Arc::new(AtomicBool::new(false));
        let (wake, mut events) = tokio::sync::mpsc::channel(1);
        let watch = crate::compiled_watch::NativeInputWatch::new(
            invalid.clone(),
            crate::compiled_watch::WatchPlan::read_inputs(&local_inputs).unwrap(),
            wake,
        )
        .unwrap();
        std::fs::write(included.join("build/generated.class"), b"private output").unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(!invalid.load(Ordering::Acquire));
        std::fs::create_dir(&build_src).unwrap();
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
                .await
                .unwrap(),
            Some(())
        );
        assert!(invalid.load(Ordering::Acquire));
        watch.close();
    }

    #[test]
    fn structured_errors_keep_owner_and_utf16_position_and_reject_unknown_locations() {
        let mut owner = overlay();
        let private = owner.directory.join("Problem.java");
        let key = kotlin_path_key(&crate::paths::file_key(&private));
        owner.paths.insert(key, "C:/project/Problem.java".into());
        let event = json!({"event":"diagnostic", "severity":"ERROR",
            "code":"compiler.err.feature.not.supported.in.source.plural",
            "message":"default methods are not supported in -source 6",
            "locations":[{"path":private,"line":36,"column":3}]});
        let actual = diagnostics(&[event.clone()], &owner).unwrap();
        assert_eq!(actual.len(), 1);
        assert_eq!(actual[0].path, "C:/project/Problem.java");
        assert_eq!((actual[0].line, actual[0].col), (36, 3));
        assert_eq!(actual[0].severity, Severity::Error);
        let mut slash_variant = event.clone();
        slash_variant["locations"][0]["path"] = json!(private.to_string_lossy().replace('\\', "/"));
        assert_eq!(
            diagnostics(&[slash_variant], &owner).unwrap()[0].path,
            actual[0].path
        );
        let mut warning = event.clone();
        warning["severity"] = json!("WARNING");
        assert_eq!(
            diagnostics(&[warning], &owner).unwrap()[0].severity,
            Severity::Warning
        );
        let mut advice = event.clone();
        advice["severity"] = json!("ADVICE");
        assert!(diagnostics(&[advice], &owner).unwrap().is_empty());
        // Gradle's display strings are not enum identities and must never silently erase errors.
        for severity in [
            json!("Error"),
            json!("Warning"),
            json!("fatal"),
            Value::Null,
        ] {
            let mut changed = event.clone();
            changed["severity"] = severity;
            assert!(diagnostics(&[changed], &owner).is_err());
        }
        for mutation in ["missing", "ambiguous", "foreign", "zero", "overflow"] {
            let mut changed = event.clone();
            match mutation {
                "missing" => changed["locations"] = json!([]),
                "ambiguous" => {
                    changed["locations"] = json!([
                        changed["locations"][0].clone(),
                        changed["locations"][0].clone()
                    ])
                }
                "foreign" => changed["locations"][0]["path"] = json!("C:/other/Problem.java"),
                "zero" => changed["locations"][0]["column"] = json!(0),
                "overflow" => changed["locations"][0]["line"] = json!(u64::MAX),
                _ => unreachable!(),
            }
            assert!(diagnostics(&[changed], &owner).is_err(), "{mutation}");
        }
    }

    #[test]
    fn sidecars_require_current_nonce_contiguous_order_and_unique_native_boundaries() {
        let owner = overlay();
        let file = owner.directory.join("events.jsonl");
        for event in [
            json!({"schema":1,"nonce":"old","sequence":0}),
            json!({"schema":1,"nonce":"current","sequence":1}),
        ] {
            std::fs::write(&file, format!("{event}\n")).unwrap();
            assert!(events(&file, "current").is_err());
        }
        let event = json!({"schema":1,"nonce":"current","sequence":0,"event":"compiler-start"});
        std::fs::write(&file, format!("{event}\n")).unwrap();
        let actual = events(&file, "current").unwrap();
        assert!(one(&actual, "compiler-start").is_ok());
        assert!(one(&actual, "compiler-finish").is_err());
        assert!(one(&[event.clone(), event], "compiler-start").is_err());
    }
}
