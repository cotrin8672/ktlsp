//! Current K1 plugin API positives. This never certifies absent members or producer completeness.
use super::*;

const SOURCE: &str = include_str!("PositiveHeaders.kt");

pub(crate) struct Snapshot {
    pub sources: BTreeMap<String, std::sync::Arc<str>>,
    pub classes: BTreeMap<String, Vec<ktcore::extensions::CompilerClassInput>>,
}

const LOMBOK_1_18_32_SHA256: &str =
    "97574674e2a25f567a313736ace00df8787d443de316407d57fc877d9f19a65d";

fn bounded_lombok(task: &CompilerTask) -> anyhow::Result<()> {
    let [processor] = task.annotation_processor_path.as_slice() else {
        anyhow::bail!("positive-java-processor-profile-unknown");
    };
    anyhow::ensure!(
        processor.file_name().and_then(|name| name.to_str()) == Some("lombok-1.18.32.jar")
            && sha(&std::fs::read(processor)?) == LOMBOK_1_18_32_SHA256,
        "positive-java-processor-profile-unknown"
    );
    Ok(())
}

fn stock_java_task(task: &CompilerTask) -> bool {
    task.complete
        && task.problem.is_none()
        && matches!(
            task.task_type.as_str(),
            "org.gradle.api.tasks.compile.JavaCompile"
                | "org.gradle.api.tasks.compile.JavaCompile_Decorated"
        )
        && task
            .options
            .get("taskActionInventory")
            .and_then(|fact| fact.get("adapter"))
            .and_then(Value::as_str)
            == Some("gradle-8.10.2-stock-actions-v1")
        && task
            .options
            .get("taskOnlyIfInventory")
            .and_then(|fact| fact.get("adapter"))
            .and_then(Value::as_str)
            == Some("gradle-8.10.2-stock-onlyif-v1")
        && stock_task_action(task, "org.gradle.api.tasks.compile.JavaCompile", "compile")
        && stock_only_if(task, false)
        && task.options.get("fork") == Some(&Value::Bool(false))
        && [
            "forkJavaHome",
            "forkExecutable",
            "bootstrapClasspath",
            "encoding",
        ]
        .iter()
        .all(|key| task.options.get(*key) == Some(&Value::Null))
        && task.options.get("forkJvmArgs") == Some(&serde_json::json!([]))
        && task.options.get("release") == Some(&serde_json::json!(17))
        && ["sourceCompatibility", "targetCompatibility"]
            .iter()
            .all(|key| task.options.get(*key).and_then(Value::as_str) == Some("17"))
}

#[cfg(test)]
mod profile_tests {
    use super::*;

    #[tokio::test]
    async fn private_lombok_watch_ends_before_overlay_cleanup_but_original_watch_survives() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        let root = std::env::temp_dir().join(format!(
            "ktlsp-private-lombok-watch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let original = root.join("project/src");
        let private = root.join("overlay/sources");
        std::fs::create_dir_all(&original).unwrap();
        std::fs::create_dir_all(&private).unwrap();
        let invalid = Arc::new(AtomicBool::new(false));
        let (wake, mut events) = tokio::sync::mpsc::channel(1);
        let environment =
            crate::compiled_watch::NativeInputWatch::new(invalid.clone(), Default::default(), wake)
                .unwrap();
        let sources = [original.join("Input.java")];
        let copied = [private.join("Input.java")];
        let (private_invalid, watch) =
            watch_lombok_configurations(&sources, &copied, &environment).unwrap();
        std::fs::write(private.join("lombok.config"), "config.stopBubbling = true").unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !private_invalid.load(Ordering::Acquire) {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        assert!(
            !invalid.load(Ordering::Acquire),
            "private config changes invalidate only their Java invocation"
        );
        drop(watch);
        std::fs::remove_dir_all(root.join("overlay")).unwrap();
        assert!(
            !invalid.load(Ordering::Acquire),
            "owned cleanup must not supersede current Kotlin API"
        );
        std::fs::write(original.join("lombok.config"), "config.stopBubbling = true").unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            invalid.load(Ordering::Acquire),
            "original config changes still supersede the environment"
        );
        assert!(
            watch_lombok_configurations(&sources, &[], &environment).is_err(),
            "existing configs remain unmodeled"
        );
        drop(environment);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scripting_or_renamed_unknown_plugins_do_not_add_an_initial_compile_barrier() {
        fn archive(title: &str, registrar: &str) -> Vec<u8> {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            for (name, content) in [
                ("META-INF/MANIFEST.MF", format!("Implementation-Title: {title}\r\nImplementation-Version: 1.9.23-release-779\r\n")),
                ("META-INF/services/org.jetbrains.kotlin.compiler.plugin.CompilerPluginRegistrar", registrar.into()),
                ("org/jetbrains/kotlinx/serialization/compiler/extensions/SerializationComponentRegistrar.class", "archive classification only; no API authority".into()),
            ] {
                writer.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
                writer.write_all(content.as_bytes()).unwrap();
            }
            writer.finish().unwrap().into_inner()
        }
        let title = "kotlinx-serialization-compiler-plugin.embeddable";
        let registrar = "org.jetbrains.kotlinx.serialization.compiler.extensions.SerializationComponentRegistrar";
        assert!(producer_archive(std::io::Cursor::new(archive(title, registrar))).unwrap());
        for (title, registrar) in [
            ("kotlin-scripting-compiler.embeddable", registrar),
            (title, "custom.RenamedPlugin"),
        ] {
            assert!(!producer_archive(std::io::Cursor::new(archive(title, registrar))).unwrap());
        }
        let mut task = kotlin_test_request(Path::new("/profile-test")).task;
        task.kotlin_compiler.as_mut().unwrap().compiler_version = "1.9.23".into();
        task.kotlin_compiler.as_mut().unwrap().gradle_plugin_version = "1.9.23".into();
        task.arguments = vec![
            "-Xplugin=/unknown/kotlin-serialization-compiler-plugin-embeddable-1.9.23.jar".into(),
        ];
        assert!(!supported(&task));
        task.arguments.clear();
        assert!(!supported(&task));
    }

    #[test]
    fn custom_task_actions_and_predicates_cannot_certify_a_value_namespace() {
        let mut task = kotlin_test_request(Path::new("/profile-test")).task;
        task.complete = true;
        task.problem = None;
        task.task_type = "org.gradle.api.tasks.compile.JavaCompile_Decorated".into();
        let options = serde_json::json!({
            "taskActionInventory": {"adapter":"gradle-8.10.2-stock-actions-v1", "actions":[{
                "class":"org.gradle.api.internal.project.taskfactory.IncrementalTaskAction",
                "declaringClass":"org.gradle.api.tasks.compile.JavaCompile", "method":"compile",
                "parameterTypes":["org.gradle.work.InputChanges"]}]},
            "taskOnlyIfInventory":{"adapter":"gradle-8.10.2-stock-onlyif-v1", "predicates":[{
                "class":"org.gradle.api.internal.tasks.execution.SelfDescribingSpec",
                "stockEnabledPredicate":true,"stockKaptPredicate":false}]},
            "fork":false, "forkJavaHome":null, "forkExecutable":null, "forkJvmArgs":[],
            "bootstrapClasspath":null, "encoding":null, "release":17,
            "sourceCompatibility":"17", "targetCompatibility":"17"
        });
        task.options = serde_json::from_value(options).unwrap();
        assert!(stock_java_task(&task));
        for (field, replacement) in [
            (
                "taskActionInventory",
                serde_json::json!({"adapter":"gradle-8.10.2-stock-actions-v1", "actions":[]}),
            ),
            (
                "taskActionInventory",
                serde_json::json!({"unproved":true,"actions":[{"class":"custom.BodyFieldAction"}]}),
            ),
            (
                "taskOnlyIfInventory",
                serde_json::json!({"adapter":"gradle-8.10.2-stock-onlyif-v1", "predicates":[]}),
            ),
            (
                "taskOnlyIfInventory",
                serde_json::json!({"unproved":true,"predicates":[{"stockEnabledPredicate":true,"stockKaptPredicate":false}]}),
            ),
            ("fork", Value::Bool(true)),
            ("release", serde_json::json!(21)),
        ] {
            let mut changed = task.clone();
            changed.options.insert(field.into(), replacement);
            assert!(!stock_java_task(&changed), "{field}");
        }
        let mut changed = task.clone();
        changed.options.get_mut("taskActionInventory").unwrap()["actions"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"class":"custom.BodyFieldAction"}));
        assert!(!stock_java_task(&changed));
        changed = task;
        changed.options.get_mut("taskOnlyIfInventory").unwrap()["predicates"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"class":"custom.Predicate"}));
        assert!(!stock_java_task(&changed));
    }

    #[test]
    fn captured_processor_recipe_does_not_admit_the_absence_observer() {
        let mut request = kotlin_test_request(Path::new("/profile-test"));
        let mut java = request.task.clone();
        java.language = crate::classpath::CompilerLanguage::Java;
        java.complete = true;
        java.problem = None;
        java.annotation_processor_path = vec![PathBuf::from("/processor/lombok-1.18.32.jar")];
        let recipe = crate::classpath::JavaCompilerRecipe {
            adapter: "gradle-8.10.2-current-jdk17-release17-javac-api-with-processors-v1".into(),
            compiler_type: "org.gradle.jvm.toolchain.internal.DefaultToolchainJavaCompiler".into(),
            java_runtime_version: "17.0.7+8-LTS-224".into(),
            language_version: "17".into(),
            gradle_java_version: "17.0.7".into(),
            gradle_file_encoding: "UTF-8".into(),
            effective_charset: "UTF-8".into(),
            locale: "ja-JP".into(),
            api_options: vec!["--release".into(), "17".into()],
        };
        java.options.insert(
            "javaCompilerRecipe".into(),
            serde_json::to_value(recipe).unwrap(),
        );
        java.options
            .insert("javaCompilerRecipeProblem".into(), serde_json::Value::Null);
        assert!(java.java_compiler_recipe().is_some());
        request.java_task = Some(java);
        for version in ["1.8.21", "1.9.23", "2.4.0"] {
            let compiler = request.task.kotlin_compiler.as_mut().unwrap();
            compiler.compiler_version = version.into();
            compiler.gradle_plugin_version = version.into();
            assert!(configured_recipe(&request).is_err(), "{version}");
        }
    }

    #[test]
    fn arbitrary_and_body_sensitive_processors_do_not_gain_namespace_reuse() {
        let mut task = kotlin_test_request(Path::new("/profile-test")).task;
        for processors in [
            Vec::new(),
            vec![PathBuf::from("/processor/body-sensitive.jar")],
            vec![
                PathBuf::from("/processor/lombok-1.18.32.jar"),
                PathBuf::from("/processor/extra.jar"),
            ],
        ] {
            task.annotation_processor_path = processors;
            assert!(bounded_lombok(&task).is_err());
        }
        let directory = std::env::temp_dir().join(format!(
            "ktlsp-positive-profile-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let disguised = directory.join("lombok-1.18.32.jar");
        std::fs::write(&disguised, b"untrusted processor bytes").unwrap();
        task.annotation_processor_path = vec![disguised.clone()];
        assert!(bounded_lombok(&task).is_err());
        std::fs::remove_file(disguised).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}

fn lombok_configurations(sources: &[PathBuf]) -> Vec<PathBuf> {
    sources
        .iter()
        .flat_map(|source| {
            source
                .ancestors()
                .skip(1)
                .map(|parent| parent.join("lombok.config"))
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn watch_lombok_configurations(
    originals: &[PathBuf],
    private: &[PathBuf],
    environment: &crate::compiled_watch::NativeInputWatch,
) -> anyhow::Result<(
    std::sync::Arc<std::sync::atomic::AtomicBool>,
    std::sync::Arc<crate::compiled_watch::NativeInputWatch>,
)> {
    let originals = lombok_configurations(originals);
    let private = lombok_configurations(private);
    environment.extend(crate::compiled_watch::WatchPlan::read_inputs(&originals)?)?;
    // Private config reads end with this invocation, before its owned overlay is removed.
    let invalid = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (wake, _events) = tokio::sync::mpsc::channel(1);
    let watch = crate::compiled_watch::NativeInputWatch::new(
        invalid.clone(),
        crate::compiled_watch::WatchPlan::read_inputs(&private)?,
        wake,
    )?;
    anyhow::ensure!(
        originals
            .iter()
            .chain(&private)
            .all(|path| matches!(path.try_exists(), Ok(false))),
        "positive-java-lombok-config-unmodeled"
    );
    Ok((invalid, watch))
}

fn java_namespace(
    request: &KotlinDiagnosticRequest,
    environment: &KotlinDiagnosticEnvironment,
    overlay: &KotlinEditorOverlay,
    headers: &Path,
    snapshot: &mut Snapshot,
) -> anyhow::Result<()> {
    let task = request
        .java_task
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("positive-java-task-unknown"))?;
    anyhow::ensure!(
        stock_java_task(task),
        "positive-java-task-actions-unmodeled"
    );
    anyhow::ensure!(
        std::env::var_os("JDK_JAVAC_OPTIONS")
            .filter(|value| !value.is_empty())
            .is_none(),
        "positive-java-ambient-javac-options-unmodeled"
    );
    bounded_lombok(task)?;
    let recipe = task
        .java_compiler_recipe()
        .ok_or_else(|| anyhow::anyhow!("positive-java-recipe-unknown"))?;
    anyhow::ensure!(
        recipe.adapter == "gradle-8.10.2-current-jdk17-release17-javac-api-with-processors-v1"
            && recipe.effective_charset == "UTF-8"
            && recipe.gradle_java_version.starts_with("17.")
            && recipe.java_runtime_version.starts_with("17.")
            && task.arguments.is_empty()
            && task
                .java_home
                .as_ref()
                .is_some_and(|home| same_path(home, &environment.key.recipe.gradle_java_home)),
        "positive-java-runtime-profile-unknown"
    );
    let (private_configuration_invalid, _configuration_watch) =
        watch_lombok_configurations(&task.source_files, &overlay.sources, &environment.watch)?;
    let java_sources: Vec<_> = overlay
        .sources
        .iter()
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("java"))
        .collect();
    let originals: BTreeSet<_> = java_sources
        .iter()
        .map(|path| {
            overlay
                .paths
                .get(&kotlin_path_key(&crate::paths::file_key(path)))
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("positive-java-source-unknown"))
        })
        .collect::<anyhow::Result<_>>()?;
    anyhow::ensure!(
        originals
            == task
                .source_files
                .iter()
                .map(|path| crate::paths::file_key(path))
                .collect(),
        "positive-java-source-inventory-differs"
    );
    let mut classpath = task.classpath.clone();
    let slots: Vec<_> = classpath
        .iter()
        .enumerate()
        .filter(|(_, path)| {
            request
                .task
                .output_roots
                .iter()
                .any(|output| same_path(path, output))
        })
        .map(|(slot, _)| slot)
        .collect();
    let [slot] = slots.as_slice() else {
        anyhow::bail!("positive-java-kotlin-slot-unknown");
    };
    classpath[*slot] = headers.into();
    let classes = overlay.directory.join("java-positive-classes");
    let generated = overlay.directory.join("java-positive-generated");
    let native_headers = overlay.directory.join("java-positive-native-headers");
    for directory in [&classes, &generated, &native_headers] {
        std::fs::create_dir(directory)?;
    }
    let mut arguments = recipe.api_options.clone();
    let mut seen = BTreeSet::new();
    let mut index = 0;
    while index < arguments.len() {
        let flag = arguments[index].clone();
        if matches!(flag.as_str(), "-g" | "-XDuseUnsharedTable=true") {
            index += 1;
            continue;
        }
        anyhow::ensure!(
            seen.insert(flag.clone()) && index + 1 < arguments.len(),
            "positive-java-option-shape-unknown"
        );
        let value = &arguments[index + 1];
        let replacement = match flag.as_str() {
            "--release" if value == "17" => value.clone(),
            "-sourcepath" if value.is_empty() => value.clone(),
            "-processorpath" if *value == joined_classpath(&task.annotation_processor_path)? => {
                value.clone()
            }
            "-classpath" if *value == joined_classpath(&task.classpath)? => {
                joined_classpath(&classpath)?
            }
            "-d" if task.output_roots.len() == 1
                && same_path(Path::new(value), &task.output_roots[0]) =>
            {
                path_text(&classes)?
            }
            "-h" | "-s"
                if task
                    .declared_output_roots
                    .iter()
                    .any(|root| same_path(Path::new(value), root)) =>
            {
                path_text(if flag == "-h" {
                    &native_headers
                } else {
                    &generated
                })?
            }
            _ => anyhow::bail!("positive-java-option-unhandled: {flag}"),
        };
        arguments[index + 1] = replacement;
        index += 2;
    }
    anyhow::ensure!(
        seen == [
            "--release",
            "-sourcepath",
            "-processorpath",
            "-classpath",
            "-d",
            "-h",
            "-s"
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        "positive-java-options-incomplete"
    );
    arguments.extend(
        java_sources
            .iter()
            .map(|source| path_text(source))
            .collect::<anyhow::Result<Vec<_>>>()?,
    );
    let argument_file = overlay.directory.join("java-positive.args");
    write_new(&argument_file, kotlin_argument_file(&arguments)?.as_bytes())?;
    let mut command = Command::new(environment.key.recipe.gradle_java_home.join(
        if cfg!(windows) {
            "bin/javac.exe"
        } else {
            "bin/javac"
        },
    ));
    command
        .current_dir(&request.project_dir)
        .arg("-J-Dfile.encoding=UTF-8")
        .arg(format!("@{}", argument_file.display()));
    let (exit, text) = native(&mut command, request, environment)?;
    anyhow::ensure!(
        exit == 0,
        "positive-java-current-compilation-failed: {text}"
    );
    for source in java_sources {
        let text = std::fs::read_to_string(source)?;
        let tree = crate::java::JavaParser::new().parse(&text);
        if crate::java::declaration_header_text(&tree, &text).is_none() {
            continue;
        }
        let symbols = crate::java::extract_symbols(&tree, &text);
        let roots: Vec<_> = symbols
            .iter()
            .filter(|symbol| symbol.kind.is_type_like() && symbol.container.is_none())
            .collect();
        let [owner] = roots.as_slice() else {
            continue;
        };
        if owner.kind != ktcore::symbol::SymbolKind::Class
            || owner.container.is_some()
            || !owner.type_params.is_empty()
            || !owner.supertypes.is_empty()
            || !owner.supertype_refs.is_empty()
        {
            continue;
        }
        let internal = if owner.package.is_empty() {
            owner.name.clone()
        } else {
            format!("{}/{}", owner.package.replace('.', "/"), owner.name)
        };
        let class = ktcore::classfile::decode(
            "current-compiler",
            &std::fs::read(classes.join(format!("{internal}.class")))?,
        )?;
        if class.identity.internal_name != internal
            || !class.fields.is_empty()
            || class.super_name.as_deref() != Some("java/lang/Object")
            || !class.interfaces.is_empty()
            || class.signature.is_some()
            || class
                .inner_classes
                .iter()
                .any(|inner| inner.inner.as_ref() == internal)
            || class
                .attribute_names
                .iter()
                .any(|name| name.as_ref() == "EnclosingMethod")
        {
            continue;
        }
        // shortcut: body reuse is limited to pinned Lombok's unannotated, field-free Object roots.
        let header = crate::java::declaration_header_text(&tree, &text).unwrap();
        if header.contains('@')
            || symbols
                .iter()
                .any(|symbol| symbol.kind == ktcore::symbol::SymbolKind::Property)
        {
            continue;
        }
        let original = overlay
            .paths
            .get(&kotlin_path_key(&crate::paths::file_key(source)))
            .ok_or_else(|| anyhow::anyhow!("positive-java-source-unknown"))?;
        snapshot
            .sources
            .insert(original.clone(), std::sync::Arc::from(text));
        snapshot.classes.insert(
            original.clone(),
            vec![ktcore::extensions::CompilerClassInput {
                class: std::sync::Arc::new(class),
                kotlin_methods: BTreeSet::new(),
                companion: false,
                value_namespace: true,
            }],
        );
    }
    anyhow::ensure!(
        !snapshot.classes.is_empty(),
        "positive-java-no-bounded-source-owner"
    );
    check_closed_sources(request, overlay, &current_sources(overlay)?)?;
    anyhow::ensure!(
        !private_configuration_invalid.load(std::sync::atomic::Ordering::Acquire),
        "positive-java-private-configuration-changed"
    );
    Ok(())
}

pub(crate) struct Tools {
    owner: KotlinEditorOverlay,
    jar: PathBuf,
}

pub(crate) fn candidate(task: &CompilerTask) -> bool {
    task.kotlin_compiler.as_ref().is_some_and(|compiler| {
        compiler.compiler_version == "1.9.23" && compiler.gradle_plugin_version == "1.9.23"
    }) && task
        .arguments
        .iter()
        .any(|argument| argument.starts_with("-Xplugin="))
}

fn producer_archive<R: std::io::Read + std::io::Seek>(reader: R) -> anyhow::Result<bool> {
    let mut archive = zip::ZipArchive::new(reader)?;
    let mut small_entry = |name: &str| -> anyhow::Result<String> {
        let mut entry = archive.by_name(name)?;
        anyhow::ensure!(entry.size() <= 8192, "positive-producer-entry-too-large");
        let mut text = String::new();
        entry.read_to_string(&mut text)?;
        Ok(text)
    };
    let manifest = small_entry("META-INF/MANIFEST.MF")?;
    let title = manifest.lines().any(|line| {
        line == "Implementation-Title: kotlinx-serialization-compiler-plugin.embeddable"
    });
    let version = manifest.lines().any(|line| {
        line.strip_prefix("Implementation-Version: ")
            .is_some_and(|version| version.starts_with("1.9.23-"))
    });
    let registrar = small_entry(
        "META-INF/services/org.jetbrains.kotlin.compiler.plugin.CompilerPluginRegistrar",
    )?;
    let registrars: Vec<_> = registrar
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    Ok(title && version && registrars == ["org.jetbrains.kotlinx.serialization.compiler.extensions.SerializationComponentRegistrar"]
        && archive.by_name("org/jetbrains/kotlinx/serialization/compiler/extensions/SerializationComponentRegistrar.class").is_ok())
}

/// Used for producer scheduling/environment initialization, never completion queries.
pub(crate) fn supported(task: &CompilerTask) -> bool {
    candidate(task)
        && kotlin_plugin_paths(&task.arguments).is_ok_and(|plugins| {
            plugins.iter().any(|path| {
                std::fs::File::open(path)
                    .ok()
                    .is_some_and(|file| producer_archive(file).unwrap_or(false))
            })
        })
}

impl Tools {
    fn build(
        request: &KotlinDiagnosticRequest,
        environment: &KotlinDiagnosticEnvironment,
    ) -> anyhow::Result<Self> {
        let owner = KotlinEditorOverlay::empty_in(
            request,
            &crate::deps::cache_home().join("positive-header-tools"),
        )?;
        let source = owner.directory.join("PositiveHeaders.kt");
        write_new(&source, SOURCE.as_bytes())?;
        let classes = owner.directory.join("observer-classes");
        std::fs::create_dir(&classes)?;
        let arguments = vec![
            "-no-stdlib".into(),
            "-no-reflect".into(),
            "-jvm-target".into(),
            "17".into(),
            "-jdk-home".into(),
            path_text(&environment.key.recipe.gradle_java_home)?,
            "-classpath".into(),
            joined_classpath(&environment.key.recipe.default_classpath)?,
            "-d".into(),
            path_text(&classes)?,
            "-version".into(),
            path_text(&source)?,
        ];
        let (exit, text) = kotlin(
            &arguments,
            &owner.directory.join("build.args"),
            request,
            environment,
        )?;
        anyhow::ensure!(exit == 0, "positive-header-tool-build: {text}");
        let jar = owner.directory.join("observer.jar");
        let mut archive = zip::ZipWriter::new(
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&jar)?,
        );
        let mut names = BTreeSet::new();
        for entry in walkdir::WalkDir::new(&classes).follow_links(false) {
            let entry = entry?;
            anyhow::ensure!(!entry.file_type().is_symlink(), "positive-tool-symlink");
            if !entry.file_type().is_file() {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(&classes)?
                .to_string_lossy()
                .replace('\\', "/");
            anyhow::ensure!(
                relative.starts_with("ktlsp/positive/") || relative.starts_with("META-INF/"),
                "positive-tool-foreign-class"
            );
            archive.start_file(&relative, zip::write::SimpleFileOptions::default())?;
            archive.write_all(&std::fs::read(entry.path())?)?;
            names.insert(relative);
        }
        anyhow::ensure!(
            names.contains("ktlsp/positive/PositiveRegistrar.class")
                && names.contains("ktlsp/positive/PositiveOptions.class"),
            "positive-tool-registrar-missing"
        );
        for (service, class) in [
            ("ComponentRegistrar", "PositiveRegistrar"),
            ("CommandLineProcessor", "PositiveOptions"),
        ] {
            archive.start_file(
                format!("META-INF/services/org.jetbrains.kotlin.compiler.plugin.{service}"),
                zip::write::SimpleFileOptions::default(),
            )?;
            archive.write_all(format!("ktlsp.positive.{class}\n").as_bytes())?;
        }
        archive.finish()?.sync_all()?;
        Ok(Self { owner, jar })
    }
}

pub(crate) fn run(
    request: &KotlinDiagnosticRequest,
    environment: &KotlinDiagnosticEnvironment,
) -> anyhow::Result<KotlinDiagnosticOutcome> {
    anyhow::ensure!(
        environment.positive_supported,
        "positive-compiler-unsupported"
    );
    let mut reads = request.sources.clone();
    if let Some(java) = &request.java_task {
        reads.extend(java.annotation_processor_path.iter().cloned());
        reads.extend(
            java.classpath
                .iter()
                .filter(|path| {
                    !request
                        .task
                        .output_roots
                        .iter()
                        .any(|output| same_path(path, output))
                })
                .cloned(),
        );
    }
    environment
        .watch
        .extend(crate::compiled_watch::WatchPlan::read_inputs(&reads)?)?;
    let overlay = KotlinEditorOverlay::create(request)?;
    let expected = current_sources(&overlay)?;
    let mut cached = environment
        .positive_tools
        .lock()
        .map_err(|_| anyhow::anyhow!("positive-tool-cache-poisoned"))?;
    if cached.is_none() {
        *cached = Some(Tools::build(request, environment)?);
    }
    let tools = cached.as_ref().unwrap();
    anyhow::ensure!(
        std::fs::read(tools.owner.directory.join("PositiveHeaders.kt"))? == SOURCE.as_bytes(),
        "positive-tool-source-changed"
    );
    let inventory = overlay.directory.join("sources.tsv");
    let inventory_text: String = overlay
        .sources
        .iter()
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("kt"))
        .map(|path| {
            Ok(format!(
                "{}\t{}\n",
                expected[&header_key(path)],
                path_text(path)?
            ))
        })
        .collect::<anyhow::Result<_>>()?;
    write_new(&inventory, inventory_text.as_bytes())?;
    let output = overlay.directory.join("positive-headers");
    std::fs::create_dir(&output)?;
    let receipt = overlay.directory.join("positive.json");
    let mut arguments =
        kotlin_editor_arguments(request, &overlay.directory.join("unused-output"), &[])?;
    arguments.push(format!("-Xplugin={}", path_text(&tools.jar)?));
    for (key, value) in [
        ("output", output.clone()),
        ("receipt", receipt.clone()),
        ("inventory", inventory),
    ] {
        arguments.extend([
            "-P".into(),
            format!("plugin:ktlsp.positive-headers:{key}={}", path_text(&value)?),
        ]);
    }
    arguments.extend(
        overlay
            .sources
            .iter()
            .map(|source| path_text(source))
            .collect::<anyhow::Result<Vec<_>>>()?,
    );
    let (exit, text) = kotlin(
        &arguments,
        &overlay.directory.join("compiler.args"),
        request,
        environment,
    )?;
    let outcome = selected_kotlin_output(&text, "1.9.23", Some(exit), false, true, &overlay.paths);
    if !matches!(outcome, KotlinDiagnosticOutcome::Verified { .. }) {
        return Ok(outcome);
    }
    let facts = read_json(&receipt)?;
    anyhow::ensure!(
        facts.get("failure").is_none()
            && facts["schema"] == 1
            && facts["compilerVersion"] == "1.9.23"
            && facts["callbackCount"] == 1,
        "positive-header-observation-failed: {facts}"
    );
    for flag in [
        "positiveOnly",
        "stageCompleted",
        "bindingDiagnosticsPreserved",
    ] {
        require_bool(&facts, flag, true)?;
    }
    check_sources(&overlay, &expected)?;
    check_closed_sources(request, &overlay, &expected)?;
    let mut sources = BTreeMap::new();
    let mut classes: BTreeMap<String, Vec<ktcore::extensions::CompilerClassInput>> =
        BTreeMap::new();
    let rows = array(&facts, "classes")?;
    for record in array(&facts, "outputs")? {
        let relative = value_string(record, "relativePath")?;
        if !relative.ends_with(".class") {
            continue;
        }
        let path = output.join(relative);
        anyhow::ensure!(
            path.starts_with(&output) && !relative.contains(".."),
            "positive-output-path-escape"
        );
        let bytes = std::fs::read(&path)?;
        anyhow::ensure!(
            sha(&bytes) == value_string(record, "sha256")?,
            "positive-output-changed"
        );
        let raw = ktcore::classfile::decode("current-compiler", &bytes)?;
        anyhow::ensure!(
            relative == format!("{}.class", raw.identity.internal_name),
            "positive-output-owner-mismatch"
        );
        let qualified = raw
            .identity
            .internal_name
            .replace('/', ".")
            .replace('$', ".");
        let matching: Vec<_> = rows
            .iter()
            .filter(|row| row["owner"] == qualified)
            .collect();
        let [row] = matching.as_slice() else {
            continue;
        };
        let source = value_string(row, "source")?;
        anyhow::ensure!(
            expected.contains_key(source)
                && array(record, "sources")?
                    .iter()
                    .any(|path| path.as_str() == Some(source)),
            "positive-output-source-mismatch"
        );
        let original = overlay
            .paths
            .get(&kotlin_path_key(source))
            .ok_or_else(|| anyhow::anyhow!("positive-source-origin-unknown"))?;
        let private = overlay
            .sources
            .iter()
            .find(|path| header_key(path) == source)
            .ok_or_else(|| anyhow::anyhow!("positive-private-source-unknown"))?;
        sources.insert(
            original.clone(),
            std::sync::Arc::from(std::fs::read_to_string(private)?),
        );
        let methods = array(row, "methods")?
            .iter()
            .map(|method| {
                method
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| anyhow::anyhow!("positive-method-unknown"))
            })
            .collect::<anyhow::Result<_>>()?;
        classes
            .entry(original.clone())
            .or_default()
            .push(ktcore::extensions::CompilerClassInput {
                class: std::sync::Arc::new(raw),
                kotlin_methods: methods,
                companion: row["companion"]
                    .as_bool()
                    .ok_or_else(|| anyhow::anyhow!("positive-companion-unknown"))?,
                value_namespace: false,
            });
    }
    let mut snapshot = Snapshot { sources, classes };
    let java_started = Instant::now();
    let mut java_snapshot = Snapshot {
        sources: BTreeMap::new(),
        classes: BTreeMap::new(),
    };
    let java = java_namespace(request, environment, &overlay, &output, &mut java_snapshot)
        .map_err(|error| error.to_string());
    if java.is_ok() {
        snapshot.sources.extend(java_snapshot.sources);
        snapshot.classes.extend(java_snapshot.classes);
    }
    crate::trace::span(
        "compiler.api.java-namespace",
        "compiler",
        java_started,
        serde_json::json!({"accepted": java.is_ok(), "error": java.err()}),
    );
    environment.check_live()?;
    request.check_live()?;
    *environment
        .compiler_api
        .lock()
        .map_err(|_| anyhow::anyhow!("positive-snapshot-poisoned"))? = Some(snapshot);
    Ok(outcome)
}
