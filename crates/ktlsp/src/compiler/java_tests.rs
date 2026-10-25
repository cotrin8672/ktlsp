use super::*;
use crate::classpath::{ModuleClasspath, ProjectSnapshot, SourceSetClasspath};
use crate::workspace::Workspace;
use serde_json::json;
use std::sync::{atomic::AtomicBool, Arc};

const JAVA: &str = "package fixture;\nimport generated.KspKotlin;\nimport generated.KspJava;\nimport generated.KaptJava;\nclass Main { void use(KspKotlin a, KspJava b, KaptJava c) { new Helper().editorUnsaved(); } }\n";
const KOTLIN: &str = "package fixture\nclass Helper { fun editorUnsaved(): Int = 1 }\n";

pub(super) fn request(root: &Path) -> KotlinDiagnosticRequest {
    let mut request = kotlin_test_request(root);
    let recipe = request.task.kotlin_compiler.as_mut().unwrap();
    recipe.compiler_version = "1.8.21".into();
    recipe.gradle_plugin_version = "1.8.21".into();
    recipe.run_via_build_tools_api = false;
    let mut kotlin_actions = action_fact("org.jetbrains.kotlin.gradle.tasks.AbstractKotlinCompile");
    kotlin_actions["actions"][0]["method"] = json!("execute");
    request
        .task
        .options
        .insert("taskActionInventory".into(), kotlin_actions);
    request
        .task
        .options
        .insert("taskOnlyIfInventory".into(), only_if_fact(false));
    let output = root.join("build/classes/java/main");
    let header = root.join("build/headers/java/main");
    let generated = root.join("build/generated/java/main");
    let classpath = vec![
        root.join("libraries/target.jar"),
        request.task.output_roots[0].clone(),
    ];
    let options = vec![
        "-source".into(),
        "17".into(),
        "-target".into(),
        "17".into(),
        "-d".into(),
        path_text(&output).unwrap(),
        "-h".into(),
        path_text(&header).unwrap(),
        "-g".into(),
        "-sourcepath".into(),
        "".into(),
        "-proc:none".into(),
        "-s".into(),
        path_text(&generated).unwrap(),
        "-XDuseUnsharedTable=true".into(),
        "-classpath".into(),
        joined_classpath(&classpath).unwrap(),
        "-proc:none".into(),
    ];
    request.java_task = Some(serde_json::from_value(json!({
        "task": ":compileJava", "language": "java", "task_type": "org.gradle.api.tasks.compile.JavaCompile_Decorated",
        "arguments": ["-proc:none"], "annotation_processor_path": [],
        "source_files": [root.join("src/Main.java")], "classpath": classpath,
        "declared_input_files": [root.join("src/Main.java")], "declared_output_roots": [output, header, generated],
        "java_home": root.join("jdk"), "output_roots": [output], "complete": true, "problem": null,
        "options": {"fork": false, "forkJavaHome": null, "forkExecutable": null, "forkJvmArgs": [],
            "bootstrapClasspath": null, "release": null, "encoding": null, "javaCompilerRecipeProblem": null,
            "taskActionInventory": action_fact("org.gradle.api.tasks.compile.JavaCompile"),
            "taskOnlyIfInventory": only_if_fact(false),
            "javaCompilerRecipe": {"adapter": "gradle-7.4.2-current-jdk17-javac-api-v1",
                "compiler_type": "org.gradle.jvm.toolchain.internal.DefaultToolchainJavaCompiler",
                "java_runtime_version": "17.0.7+8-LTS-224", "language_version": "17", "gradle_java_version": "17.0.7",
                "gradle_file_encoding": "MS932", "effective_charset": "UTF-8", "locale": "ja-JP", "api_options": options}}
    })).unwrap());
    request.overlays = request
        .sources
        .iter()
        .map(|path| {
            KotlinSourceOverlay::new(
                crate::paths::file_key(path),
                if path.extension().unwrap() == "java" {
                    JAVA.into()
                } else {
                    KOTLIN.into()
                },
                1,
            )
        })
        .collect();
    request
}

fn fixture() -> (KotlinEditorOverlay, KotlinDiagnosticRequest) {
    let parent = std::env::temp_dir().join("ktlsp-java-diagnostic-boundaries");
    let owner = KotlinEditorOverlay::empty_in(
        &kotlin_test_request(&parent.join("unused-project")),
        &parent.join("runs"),
    )
    .unwrap();
    let request = request(&owner.directory.join("project"));
    (owner, request)
}

fn set_recipe(request: &mut KotlinDiagnosticRequest, recipe: JavaCompilerRecipe) {
    request.java_task.as_mut().unwrap().options.insert(
        "javaCompilerRecipe".into(),
        serde_json::to_value(recipe).unwrap(),
    );
}

// Shapes are the actual pinned Moshi7.4.2 and OkHttp7.5 JavaCompilerArgumentsBuilder captures.
fn selected_captured_legacy_profile(request: &mut KotlinDiagnosticRequest, release8: bool) {
    let mut recipe = request
        .java_task
        .as_ref()
        .unwrap()
        .java_compiler_recipe()
        .unwrap();
    let task = request.java_task.as_mut().unwrap();
    task.options
        .insert("sourceCompatibility".into(), json!("1.8"));
    task.options
        .insert("targetCompatibility".into(), json!("1.8"));
    if release8 {
        recipe.adapter = "gradle-7.4.2-current-jdk17-release8-utf8-javac-api-v1".into();
        recipe.api_options = [
            vec!["--release".into(), "8".into()],
            recipe.api_options[4..17].to_vec(),
        ]
        .concat();
        task.arguments.clear();
        task.options.insert("release".into(), json!(8));
    } else {
        recipe.adapter = "gradle-7.5-current-jdk17-utf8-javac-api-v1".into();
        recipe.api_options[1] = "1.8".into();
        recipe.api_options[3] = "1.8".into();
        recipe
            .api_options
            .splice(6..6, ["-nowarn", "-encoding", "UTF-8"].map(String::from));
        task.options.insert("encoding".into(), json!("UTF-8"));
        for selected in [&mut request.task, task] {
            selected.options.get_mut("taskActionInventory").unwrap()["adapter"] =
                json!("gradle-7.5-stock-actions-v1");
            selected.options.get_mut("taskOnlyIfInventory").unwrap()["adapter"] =
                json!("gradle-7.5-stock-onlyif-v1");
        }
    }
    set_recipe(request, recipe);
}

fn captured_gradle75_zero_java_options(task: &mut CompilerTask) {
    task.arguments.clear();
    task.source_files.clear();
    let mut recipe = task.java_compiler_recipe().unwrap();
    assert_eq!(recipe.api_options.remove(6), "-nowarn");
    assert_eq!(recipe.api_options.pop().unwrap(), "-proc:none");
    task.options.insert("javaCompilerRecipe".into(), serde_json::to_value(recipe).unwrap());
}

#[test]
fn gradle75_zero_java_options_preserve_strict_roles_and_processor_boundary() {
    let (_owner, mut selected) = fixture();
    selected_captured_legacy_profile(&mut selected, false);
    let task = selected.java_task.as_mut().unwrap();
    captured_gradle75_zero_java_options(task);
    let recipe = task.java_compiler_recipe().unwrap();
    assert_eq!(recipe.api_options.len(), 19);
    let slots = task.java_api_option_slots(&recipe).unwrap();
    assert_eq!([slots.destination, slots.headers, slots.generated, slots.classpath], [5, 9, 15, 18]);
    assert!(!slots.explicit_processor_argument);
    assert!(task.annotation_processing_disabled());
    for mutation in ["missing-proc", "proc-mode", "unknown-flag", "ordering", "raw-arguments"] {
        let mut changed = task.clone();
        let mut recipe = recipe.clone();
        match mutation {
            "missing-proc" => { recipe.api_options.remove(13); }
            "proc-mode" => recipe.api_options[13] = "-proc:only".into(),
            "unknown-flag" => recipe.api_options.push("-Xplugin:opaque".into()),
            "ordering" => recipe.api_options.swap(6, 8),
            "raw-arguments" => changed.arguments.push("-Xlint".into()),
            _ => unreachable!(),
        }
        changed.options.insert("javaCompilerRecipe".into(), serde_json::to_value(&recipe).unwrap());
        assert!(changed.java_api_option_slots(&recipe).is_err(), "{mutation}");
        assert!(!changed.annotation_processing_disabled(), "{mutation}");
    }
}

#[test]
#[ignore = "requires an explicitly selected captured Gradle model; read-only admission oracle"]
fn captured_model_kotlin_producer_closure_admission() {
    let path = std::env::var_os("KTLSP_CAPTURED_PRODUCER_MODEL").expect("KTLSP_CAPTURED_PRODUCER_MODEL");
    let java_key = std::env::var("KTLSP_CAPTURED_PRODUCER_JAVA").expect("KTLSP_CAPTURED_PRODUCER_JAVA");
    let value: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let modules = serde_json::from_value(value["modules"].clone()).unwrap();
    let model = Arc::new(ProjectSnapshot::new(modules));
    let key = crate::paths::file_key(Path::new(&java_key));
    let (owner, source) = model.owner_for_key(&key).expect("actual source owner");
    let source = source.expect("actual source set");
    let primary = source.compiler_tasks.iter().find(|task|
        Some(task.task.as_str()) == source.kotlin_compile_task.as_deref()).unwrap();
    let java = model.plain_java_task(&key).expect("actual supported plain Java selection");
    assert_eq!(java.source_files.len(), 31);
    let predicates: Vec<_> = model.modules.iter().flat_map(|module| &module.source_sets)
        .filter(|source| source.name == "main")
        .filter(|source| source.compiler_tasks.iter().any(|task|
            [primary.task.as_str(), ":okhttp:compileKotlin", ":okhttp-tls:compileKotlin", ":mockwebserver:compileKotlin"]
                .contains(&task.task.as_str())))
        .filter_map(|source| {
            let kt = source.compiler_tasks.iter().find(|t| Some(t.task.as_str()) == source.kotlin_compile_task.as_deref())?;
            let java = source.compiler_tasks.iter().find(|t| t.language == crate::classpath::CompilerLanguage::Java)?;
            Some(json!({"task":kt.task,"selected_supported":selected_tasks_supported(kt,java,&source.compiler_tasks),
                "selected_denial":selected_tasks_supported_reason(kt,java,&source.compiler_tasks).err(),
                "java_processing_disabled":java.annotation_processing_disabled(),
                "kt_sdk_complete":crate::classpath::sdk_producer_roots(kt).map(|(_,c)|c),
                "java_sdk_complete":crate::classpath::sdk_producer_roots(java).map(|(_,c)|c),
                "kt_sources":kt.source_files.len(),"java_sources":java.source_files.len()}))
        }).collect();
    eprintln!("captured_admission_predicates={}", json!(predicates));
    let plain = crate::compile::kotlin_producers::capture(&model,source,primary,java);
    assert!(plain.is_none(), "active KAPT cannot receive plain compiler authority");
    assert_eq!(selected_tasks_supported_reason(primary,java,&source.compiler_tasks),
        Err("active-annotation-processor-path"));
    let mut ws = Workspace::new();
    ws.set_project_model(model);
    ws.open(key.clone(), std::fs::read_to_string(&key).unwrap());
    let request = ws.begin_kotlin_diagnostics(&key).unwrap();
    assert!(request.java_task.is_none(), "active AP cannot borrow unverified built API authority");
    assert!(!request.java_body_eligible, "configuration is not completed current generation authority");
    assert!(request.kotlin_producers.is_empty());
}

#[test]
fn captured_release8_and_gradle75_options_preserve_flags_and_proved_replay_roles() {
    for release8 in [true, false] {
        let (_owner, mut request) = fixture();
        selected_captured_legacy_profile(&mut request, release8);
        let recipe = configured_recipe(&request).unwrap();
        let task = request.java_task.as_ref().unwrap();
        assert!(selected_tasks_supported(
            &request.task,
            task,
            &[request.task.clone(), task.clone()]
        ));
        assert_eq!(recipe.api_options.len(), if release8 { 15 } else { 21 });
        let roles = if release8 {
            [3, 5, 11, 14]
        } else {
            [5, 10, 16, 19]
        };
        let private = request.project_dir.join("private-captured-profile");
        let replayed =
            replay_options(task, &recipe, &request.task.output_roots[0], &private).unwrap();
        for (index, original) in recipe.api_options.iter().enumerate() {
            if !roles.contains(&index) {
                assert_eq!(&replayed[index], original);
            }
        }
        assert_eq!(
            replayed[roles[0]],
            path_text(&private.join("java-classes")).unwrap()
        );
        assert_eq!(
            replayed[roles[1]],
            path_text(&private.join("java-headers")).unwrap()
        );
        assert_eq!(
            replayed[roles[2]],
            path_text(&private.join("java-generated")).unwrap()
        );
        assert_eq!(
            replayed[roles[3]],
            joined_classpath(&[task.classpath[0].clone(), private.join("lease")]).unwrap()
        );
        if !release8 {
            assert_eq!(&replayed[6..9], &["-nowarn", "-encoding", "UTF-8"]);
        }
    }
}

#[test]
fn captured_legacy_profiles_reject_unproved_sdk_release_encoding_flags_and_processors() {
    for release8 in [true, false] {
        for kind in ["sdk", "release", "encoding", "flags", "processor"] {
            let (_owner, mut request) = fixture();
            selected_captured_legacy_profile(&mut request, release8);
            match kind {
                "sdk" => {
                    request.java_task.as_mut().unwrap().java_home =
                        Some(request.project_dir.join("other-sdk"))
                }
                "release" => {
                    request
                        .java_task
                        .as_mut()
                        .unwrap()
                        .options
                        .insert("release".into(), json!(11));
                }
                "encoding" => {
                    request
                        .java_task
                        .as_mut()
                        .unwrap()
                        .options
                        .insert("encoding".into(), json!("UTF-16"));
                }
                "processor" => request
                    .java_task
                    .as_mut()
                    .unwrap()
                    .annotation_processor_path
                    .push(request.project_dir.join("processor.jar")),
                _ => {
                    let mut recipe = request
                        .java_task
                        .as_ref()
                        .unwrap()
                        .java_compiler_recipe()
                        .unwrap();
                    recipe.api_options.remove(if release8 { 0 } else { 6 });
                    set_recipe(&mut request, recipe);
                }
            }
            assert!(configured_recipe(&request).is_err(), "{release8}/{kind}");
        }
    }
    let (_owner, mut request) = fixture();
    selected_captured_legacy_profile(&mut request, false);
    let java = request.java_task.as_ref().unwrap();
    let mut auxiliary = inactive_kapt_tasks(&request);
    for task in &mut auxiliary {
        task.options.get_mut("taskActionInventory").unwrap()["adapter"] =
            json!("gradle-7.5-stock-actions-v1");
        task.options.get_mut("taskOnlyIfInventory").unwrap()["adapter"] =
            json!("gradle-7.5-stock-onlyif-v1");
    }
    let mut tasks = vec![request.task.clone(), java.clone()];
    tasks.extend(auxiliary.clone());
    assert!(selected_tasks_supported(&request.task, java, &tasks));
    let mut resolved_processors = tasks.clone();
    resolved_processors[2].annotation_processor_path.push(request.project_dir.join("moshi-kotlin-codegen.jar"));
    assert_eq!(selected_tasks_supported_reason(&request.task, java, &resolved_processors),
        Err("active-annotation-processor-path"),
        "empty explicit processor names do not disable a nonempty discovered processor path");
    auxiliary[0]
        .options
        .insert("processors".into(), json!(["active.Processor"]));
    let mut active = vec![request.task.clone(), java.clone()];
    active.extend(auxiliary);
    assert!(!selected_tasks_supported(&request.task, java, &active));
    tasks[1].options.get_mut("taskActionInventory").unwrap()["actions"][0]["class"] =
        json!("custom.Action");
    assert!(!selected_tasks_supported(&request.task, &tasks[1], &tasks));
}

#[test]
fn actual_option_order_and_output_roles_survive_partial_header_replay() {
    let (_owner, request) = fixture();
    let task = request.java_task.as_ref().unwrap();
    let recipe = configured_recipe(&request).unwrap();
    let output = request.project_dir.join("private");
    let replayed = replay_options(task, &recipe, &request.task.output_roots[0], &output).unwrap();
    for (position, original) in recipe.api_options.iter().enumerate() {
        if ![5, 7, 13, 16].contains(&position) {
            assert_eq!(&replayed[position], original);
        }
    }
    assert_eq!(replayed[10], "");
    assert_eq!(replayed[11], replayed[17]);
    assert_eq!(
        replayed[16],
        joined_classpath(&[task.classpath[0].clone(), output.join("lease")]).unwrap()
    );
    assert!(!replayed[16].contains(&path_text(&request.task.output_roots[0]).unwrap()));
}

#[test]
fn unavailable_java_keeps_kotlin_errors_and_cannot_publish_canonical_java_diagnostics() {
    let kotlin = CompileDiagnostic {
        path: "Use.kt".into(),
        line: 1,
        col: 1,
        severity: Severity::Error,
        message: "canonical error".into(),
    };
    let canonical_java = CompileDiagnostic {
        path: "Use.java".into(),
        message: "not selected by Javac".into(),
        ..kotlin.clone()
    };
    let verified_java = CompileDiagnostic {
        path: "Use.java".into(),
        message: "checked import".into(),
        ..kotlin.clone()
    };
    let mut unavailable = vec![kotlin.clone(), canonical_java.clone()];
    append_positives(&mut unavailable, Vec::new());
    assert_eq!(unavailable, vec![kotlin.clone()]);
    let mut available = vec![kotlin.clone(), canonical_java];
    append_positives(&mut available, vec![verified_java.clone()]);
    assert_eq!(available, vec![kotlin, verified_java]);
}

#[test]
fn opaque_options_new_sources_sdk_and_classpath_overlap_are_unavailable() {
    let (_owner, original) = fixture();
    for kind in [
        "opaque",
        "new-source",
        "sdk",
        "charset",
        "duplicate-slot",
        "parent-slot",
        "ap",
        "boot",
        "missing-boot",
    ] {
        let mut request = request(&original.project_dir);
        let mut recipe = request
            .java_task
            .as_ref()
            .unwrap()
            .java_compiler_recipe()
            .unwrap();
        match kind {
            "opaque" => recipe.api_options.push("@unproven.args".into()),
            "new-source" => request.sources.push(request.project_dir.join("src/New.kt")),
            "sdk" => {
                request.java_task.as_mut().unwrap().java_home =
                    Some(request.project_dir.join("other-jdk"))
            }
            "charset" => recipe.effective_charset = "windows-31j".into(),
            "duplicate-slot" | "parent-slot" => {
                let path = if kind == "duplicate-slot" {
                    request.task.output_roots[0].clone()
                } else {
                    request.task.output_roots[0].parent().unwrap().to_owned()
                };
                request.java_task.as_mut().unwrap().classpath.push(path);
                recipe.api_options[16] =
                    joined_classpath(&request.java_task.as_ref().unwrap().classpath).unwrap();
            }
            "ap" => request.java_task.as_mut().unwrap().arguments.clear(),
            "boot" => {
                request
                    .java_task
                    .as_mut()
                    .unwrap()
                    .options
                    .insert("bootstrapClasspath".into(), json!([]));
            }
            "missing-boot" => {
                request
                    .java_task
                    .as_mut()
                    .unwrap()
                    .options
                    .remove("bootstrapClasspath");
            }
            _ => unreachable!(),
        }
        set_recipe(&mut request, recipe);
        assert!(configured_recipe(&request).is_err(), "{kind}");
    }
}

#[test]
fn whole_package_domain_includes_sources_whose_api_is_unknown() {
    let expected = BTreeSet::from(["/source.kt".into(), "/consumer.kt".into()]);
    let mut receipt = json!({"unknownSources": {"/consumer.kt": ["inferred-error-type"]}, "packageDomainProof": {
        "stageCompleted": true, "allPackageDomainsKnown": true, "nameDomainIsWholePackage": true, "globalIssues": [],
        "sourceDomains": [{"source": "/source.kt", "package": "fixture", "status": "KnownWholePackage", "issues": []},
            {"source": "/consumer.kt", "package": "generated", "status": "KnownWholePackage", "issues": []}]}});
    let packages: BTreeSet<_> = known_packages(&receipt, &expected)
        .unwrap()
        .into_values()
        .collect();
    assert!(!disjoint_import("generated.KspKotlin", &packages));
    assert!(!disjoint_import("fixture.Outer.Nested", &packages));
    assert!(disjoint_import("other.Generated", &packages));
    assert!(!disjoint_import("other.*", &packages));
    receipt["packageDomainProof"]["sourceDomains"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(known_packages(&receipt, &expected).is_err());
    receipt["packageDomainProof"]["sourceDomains"] = json!([
        {"source": "/source.kt", "package": "fixture", "status": "Unknown", "issues": ["synthetic-hook"]},
        {"source": "/consumer.kt", "package": "generated", "status": "KnownWholePackage", "issues": []}]);
    assert!(known_packages(&receipt, &expected).is_err());
}

#[test]
fn incomplete_or_backend_error_headers_cannot_publish_a_lease() {
    let (owner, request) = fixture();
    let overlay =
        KotlinEditorOverlay::create_in(&request, &owner.directory.join("overlays")).unwrap();
    let expected = current_sources(&overlay).unwrap();
    let mut receipt = json!({"schema": 1, "compilerVersion": "1.8.21", "callbackCount": 1,
        "stageCompleted": true, "factoryDone": true, "bindingIdentityPreserved": true,
        "bindingDiagnosticsPreserved": true, "allSelectedSourcesIncluded": true,
        "irDiagnosticErrors": false, "incompleteHierarchyCount": 0, "badJavaSignatureCount": 0,
        "extraJvmDiagnostics": ["CONFLICTING_JVM_DECLARATIONS:ERROR"], "irExtensions": [],
        "classBuilderInterceptors": [], "classFileFinalizers": [], "analysisHandlers": ["ktlsp.headers.HeaderExtension"]});
    assert!(validated_lease(&request, &overlay, &receipt, &expected, &[], false).is_err());
    assert!(!overlay.directory.join("lease").exists());
    receipt["extraJvmDiagnostics"] = json!([]);
    receipt["stageCompleted"] = json!(false);
    assert!(validated_lease(&request, &overlay, &receipt, &expected, &[], false).is_err());
    assert!(!overlay.directory.join("lease").exists());
}

fn java_receipt(path: &Path, text: &str, recipe: &JavaCompilerRecipe) -> Value {
    let uri = format!("test:{}", path.display());
    let mut diagnostics = Vec::new();
    let mut imports = Vec::new();
    for name in [
        "generated.KspKotlin",
        "generated.KspJava",
        "generated.KaptJava",
    ] {
        let byte_start = text.find(name).unwrap();
        let start = text[..byte_start].encode_utf16().count();
        let end = start + name.encode_utf16().count();
        // Exact selected JDK17 receipt: JCFieldAccess position is the dot before the class name.
        let position = start + name[..name.find('.').unwrap()].encode_utf16().count();
        imports.push(json!({"sourceUri": uri, "static": false, "name": name, "start": start - 7, "end": end + 1,
            "qualifiedStart": start, "qualifiedEnd": end}));
        diagnostics.push(
            json!({"kind": "ERROR", "code": "compiler.err.doesnt.exist", "sourceUri": uri,
            "position": position, "start": start, "end": end, "line": -1, "column": 999,
            "message": "パッケージgeneratedは存在しません"}),
        );
    }
    for name in ["KspKotlin a", "KspJava b", "KaptJava c"] {
        diagnostics.push(
            json!({"kind": "ERROR", "code": "compiler.err.cant.resolve.location", "sourceUri": uri,
            "position": text[..text.find(name).unwrap()].encode_utf16().count(), "message": "シンボルを見つけられません"}),
        );
    }
    let source =
        json!({"path": path, "uri": uri, "bytes": text.len(), "sha256": sha(text.as_bytes())});
    json!({"compileCompleted": true, "failure": null, "success": false, "options": recipe.api_options,
        "gradleFileEncoding": recipe.gradle_file_encoding,
        "gradleConfiguredEncodingNull": !explicit_utf8_recipe(recipe),
        "gradleConfiguredEncoding": if explicit_utf8_recipe(recipe) { json!("UTF-8") } else { Value::Null },
        "gradleEffectiveDefaultCharset": recipe.effective_charset,
        "gradleEffectiveDefaultLocale": recipe.locale, "explicitFileManagerCharset": recipe.effective_charset,
        "explicitDiagnosticLocale": recipe.locale, "runtime": {"javaVersion": recipe.gradle_java_version,
            "javaRuntimeVersion": recipe.java_runtime_version, "defaultCharset": "windows-31j"},
        "compilerClass": "com.sun.tools.javac.api.JavacTool", "fileManagerClass": "com.sun.tools.javac.file.JavacFileManager",
        "sourcesBefore": [source], "sourcesAfter": [source], "parsedUnits": [uri], "imports": imports,
        "diagnostics": diagnostics, "compilerOutput": ""})
}

#[test]
fn japanese_import_messages_use_original_task_ast_and_keep_cascades_in_the_stream() {
    let (owner, request) = fixture();
    let overlay =
        KotlinEditorOverlay::create_in(&request, &owner.directory.join("overlays")).unwrap();
    let path = overlay
        .sources
        .iter()
        .find(|path| path.extension().unwrap() == "java")
        .unwrap();
    let recipe = configured_recipe(&request).unwrap();
    let receipt = java_receipt(path, JAVA, &recipe);
    let expected = vec![(path.clone(), sha(JAVA.as_bytes()))];
    let packages = BTreeSet::from(["fixture".into()]);
    let errors = import_diagnostics(
        &receipt,
        &recipe.api_options,
        &recipe,
        &expected,
        &overlay,
        &packages,
        1,
        false,
    )
    .unwrap();
    assert_eq!(receipt["diagnostics"].as_array().unwrap().len(), 6);
    assert_eq!(errors.len(), 3);
    assert_eq!(errors[0].line, 2);
    assert_eq!(errors[0].col, 18);
    assert_eq!(
        errors[0].path,
        crate::paths::file_key(&request.project_dir.join("src/Main.java"))
    );
    assert_eq!(errors[0].message, "パッケージgeneratedは存在しません");
    let mut wrong = receipt.clone();
    wrong["imports"][0]["static"] = json!(true);
    assert_eq!(
        import_diagnostics(
            &wrong,
            &recipe.api_options,
            &recipe,
            &expected,
            &overlay,
            &packages,
            1,
            false,
        )
        .unwrap()
        .len(),
        2
    );
    wrong = receipt.clone();
    wrong["parsedUnits"] = json!([]);
    assert!(import_diagnostics(
        &wrong,
        &recipe.api_options,
        &recipe,
        &expected,
        &overlay,
        &packages,
        1,
        false,
    )
    .is_err());
    wrong = receipt.clone();
    wrong["diagnostics"][0]["position"] = json!(JAVA.find("generated.KspKotlin").unwrap());
    assert_eq!(
        import_diagnostics(
            &wrong,
            &recipe.api_options,
            &recipe,
            &expected,
            &overlay,
            &packages,
            1,
            false,
        )
        .unwrap()
        .len(),
        2
    );
    let overlapping = BTreeSet::from(["fixture".into(), "generated".into()]);
    assert!(import_diagnostics(
        &receipt,
        &recipe.api_options,
        &recipe,
        &expected,
        &overlay,
        &overlapping,
        1,
        false,
    )
    .unwrap()
    .is_empty());
    std::fs::write(path, format!("{JAVA}// changed")).unwrap();
    assert!(import_diagnostics(
        &receipt,
        &recipe.api_options,
        &recipe,
        &expected,
        &overlay,
        &packages,
        1,
        false,
    )
    .is_err());
}

#[test]
fn javac_absolute_utf16_offsets_require_exact_utf8_boundaries() {
    let text = "a😀日\r\n\t終";
    for (byte, _) in text.char_indices() {
        assert_eq!(
            utf16_byte_offset(text, text[..byte].encode_utf16().count()),
            Some(byte)
        );
    }
    assert_eq!(
        utf16_byte_offset(text, 2),
        None,
        "middle of the surrogate pair"
    );
    assert_eq!(
        utf16_byte_offset(text, text.encode_utf16().count()),
        Some(text.len())
    );
    assert_eq!(
        utf16_byte_offset(text, text.encode_utf16().count() + 1),
        None
    );
    assert_eq!(utf16_byte_offset(text, usize::MAX), None);
    assert_eq!(utf16_byte_offset("", 0), Some(0));
}

#[test]
fn java_import_positions_bind_unicode_current_text_and_utf16_ast_ranges() {
    let (owner, mut request) = fixture();
    let text = format!(
        "// 前置😀\r\n{}",
        JAVA.replace("\nimport", "\n\t/* 日本語😀 */ import")
    );
    let source = request
        .overlays
        .iter_mut()
        .find(|source| source.key.ends_with(".java"))
        .unwrap();
    *source = KotlinSourceOverlay::new(source.key.clone(), text.clone(), 2);
    let overlay =
        KotlinEditorOverlay::create_in(&request, &owner.directory.join("overlays")).unwrap();
    let path = overlay
        .sources
        .iter()
        .find(|path| path.extension().unwrap() == "java")
        .unwrap();
    let recipe = configured_recipe(&request).unwrap();
    let receipt = java_receipt(path, &text, &recipe);
    let expected = vec![(path.clone(), sha(text.as_bytes()))];
    let packages = BTreeSet::from(["fixture".into()]);
    let check = |receipt: &Value| {
        import_diagnostics(
            receipt,
            &recipe.api_options,
            &recipe,
            &expected,
            &overlay,
            &packages,
            1,
            false,
        )
    };
    let errors = check(&receipt).unwrap();
    assert_eq!(errors.len(), 3);
    for (i, error) in errors.iter().enumerate() {
        assert_eq!(error.line, i as u32 + 3);
        assert_eq!(
            error.col, 31,
            "UTF-16 column, including tab and astral comment"
        );
        assert_eq!(
            error.path,
            crate::paths::file_key(&request.project_dir.join("src/Main.java"))
        );
    }
    let midpoint = text[..text.find('😀').unwrap()].encode_utf16().count() + 1;
    for field in ["qualifiedStart", "qualifiedEnd"] {
        let mut wrong = receipt.clone();
        wrong["imports"][0][field] = json!(midpoint);
        assert_eq!(
            check(&wrong).unwrap().len(),
            2,
            "reject {field} inside surrogate pair"
        );
    }
    let mut wrong = receipt.clone();
    wrong["diagnostics"][0]["position"] = json!(usize::MAX);
    assert_eq!(check(&wrong).unwrap().len(), 2);
    std::fs::write(path, text.replace("日本語", "別入力")).unwrap();
    assert!(
        check(&receipt).is_err(),
        "current source bytes must still match the owned receipt"
    );
}

#[test]
fn java_positions_use_utf16_offsets_instead_of_javac_visual_tab_columns() {
    let (owner, mut request) = fixture();
    let text = JAVA.replace("\nimport", "\n\timport");
    let source = request
        .overlays
        .iter_mut()
        .find(|source| source.key.ends_with(".java"))
        .unwrap();
    *source = KotlinSourceOverlay::new(source.key.clone(), text.clone(), 2);
    let overlay =
        KotlinEditorOverlay::create_in(&request, &owner.directory.join("overlays")).unwrap();
    let path = overlay
        .sources
        .iter()
        .find(|path| path.extension().unwrap() == "java")
        .unwrap();
    let recipe = configured_recipe(&request).unwrap();
    let receipt = java_receipt(path, &text, &recipe);
    let expected = vec![(path.clone(), sha(text.as_bytes()))];
    let errors = import_diagnostics(
        &receipt,
        &recipe.api_options,
        &recipe,
        &expected,
        &overlay,
        &BTreeSet::from(["fixture".into()]),
        1,
        false,
    )
    .unwrap();
    assert_eq!(errors[0].col, 19);
    assert_eq!(errors.len(), 3);
}

fn inactive_kapt_tasks(request: &KotlinDiagnosticRequest) -> Vec<CompilerTask> {
    let mut apt = request.task.clone();
    apt.task = ":kaptKotlin".into();
    apt.task_type = "org.jetbrains.kotlin.gradle.internal.KaptWithoutKotlincTask_Decorated".into();
    apt.arguments.clear();
    apt.source_files.clear();
    apt.kotlin_compiler = None;
    apt.annotation_processor_path.clear();
    apt.options = serde_json::from_value(json!({"processors": [], "providerArguments": [], "includeCompileClasspath": true,
        "annotationProcessorOptionProviderCount": 0,
        "taskActionInventory": action_fact("org.jetbrains.kotlin.gradle.internal.KaptWithoutKotlincTask"),
        "taskOnlyIfInventory": only_if_fact(true)})).unwrap();
    let mut stubs = request.task.clone();
    stubs.task = ":kaptGenerateStubsKotlin".into();
    stubs.task_type = "org.jetbrains.kotlin.gradle.internal.KaptGenerateStubsTask_Decorated".into();
    stubs.annotation_processor_path.clear();
    stubs.output_roots = vec![request
        .project_dir
        .join("build/tmp/kapt3/incrementalData/main")];
    stubs.declared_output_roots = vec![
        stubs.output_roots[0].clone(),
        request.project_dir.join("build/tmp/kapt3/stubs/main"),
    ];
    vec![apt, stubs]
}

#[test]
fn unknown_or_active_auxiliary_producers_cannot_authorize_import_absence() {
    let (_owner, request) = fixture();
    let java = request.java_task.as_ref().unwrap();
    let tasks = |auxiliary: Vec<CompilerTask>| {
        let mut tasks = vec![request.task.clone(), java.clone()];
        tasks.extend(auxiliary);
        tasks
    };
    assert!(selected_tasks_supported(
        &request.task,
        java,
        &tasks(Vec::new())
    ));
    assert!(selected_tasks_supported(
        &request.task,
        java,
        &tasks(inactive_kapt_tasks(&request))
    ));
    for kind in [
        "processor",
        "ap-path",
        "missing-processors",
        "provider",
        "custom",
        "missing-pair",
        "extra-ksp",
        "stub-source",
        "stub-runtime",
        "stub-output",
    ] {
        let mut auxiliary = inactive_kapt_tasks(&request);
        match kind {
            "processor" => {
                auxiliary[0]
                    .options
                    .insert("processors".into(), json!(["fixture.Processor"]));
            }
            "ap-path" => auxiliary[0]
                .annotation_processor_path
                .push(request.project_dir.join("processor.jar")),
            "missing-processors" => {
                auxiliary[0].options.remove("processors");
            }
            "provider" => {
                auxiliary[0]
                    .options
                    .insert("providerArguments".into(), json!(["-Aenabled=true"]));
            }
            "custom" => auxiliary[0].task_type = "fixture.CustomKaptWithoutKotlincTask".into(),
            "missing-pair" => {
                auxiliary.pop();
            }
            "extra-ksp" => {
                let mut task = request.task.clone();
                task.task = ":kspKotlin".into();
                auxiliary.push(task);
            }
            "stub-source" => auxiliary[1]
                .source_files
                .push(request.project_dir.join("generated/Foreign.kt")),
            "stub-runtime" => {
                auxiliary[1]
                    .kotlin_compiler
                    .as_mut()
                    .unwrap()
                    .compiler_version = "1.9.0".into()
            }
            "stub-output" => auxiliary[1]
                .declared_output_roots
                .push(java.classpath[0].clone()),
            _ => unreachable!(),
        }
        assert!(
            !selected_tasks_supported(&request.task, java, &tasks(auxiliary)),
            "{kind}"
        );
    }
}

fn workspace(
    request: &KotlinDiagnosticRequest,
    providers: Vec<Arc<dyn crate::analysis_extensions::AnalysisProvider>>,
) -> Workspace {
    let mut ws = Workspace::with_analysis_providers(providers);
    let mut tasks = vec![request.task.clone(), request.java_task.clone().unwrap()];
    if request.positive_diagnostics_only {
        tasks.extend(inactive_kapt_tasks(request));
    }
    ws.set_project_model(Arc::new(ProjectSnapshot::new(vec![ModuleClasspath {
        module: ":".into(),
        project_dir: request.project_dir.clone(),
        build: ":".into(),
        entries: Vec::new(),
        unresolved: Vec::new(),
        source_sets: vec![SourceSetClasspath {
            name: "main".into(),
            source_roots: request.source_roots.clone(),
            kotlin_compile_task: Some(request.task.task.clone()),
            compiler_tasks: tasks,
            ..Default::default()
        }],
    }])));
    for source in &request.overlays {
        ws.open(source.key.clone(), source.text.clone());
    }
    ws
}

#[test]
fn active_processors_are_build_inputs_not_editor_generation_jobs() {
    let (_owner, mut selected) = fixture();
    selected_captured_legacy_profile(&mut selected, false);
    selected.positive_diagnostics_only = true;
    let mut ws = workspace(&selected, Vec::new());
    let mut modules = ws.project_snapshot().unwrap().modules.clone();
    let processor = selected.project_dir.join("processor/src/Processor.java");
    let tasks = &mut modules[0].source_sets[0].compiler_tasks;
    tasks.iter_mut().find(|task| task.task_type.contains("KaptWithoutKotlincTask"))
        .unwrap().annotation_processor_path.push(selected.project_dir.join("processor.jar"));
    ws.set_project_model(Arc::new(ProjectSnapshot::new(modules)));
    ws.open(crate::paths::file_key(&processor), "class Processor { int unsaved; }".into());
    let key = crate::paths::file_key(&selected.project_dir.join("src/Main.java"));
    let current = ws.begin_kotlin_diagnostics(&key).unwrap();
    assert!(current.java_task.is_none(), "active processors cannot certify absent Java API");
    assert!(!current.java_body_eligible);
    assert!(current.dependency_compilation.is_none());
    assert!(current.kotlin_producers.is_empty());
    assert!(!current.dependency_overlays.iter().any(|source|
        source.key == crate::paths::file_key(&processor)),
        "editing a processor does not schedule its execution as an analysis dependency");
    assert_eq!(current.sources, selected.sources);
}

fn positive(java: &str) -> KotlinDiagnosticOutcome {
    KotlinDiagnosticOutcome::Verified {
        java_clean: None,
        clean: true,
        diagnostics: vec![CompileDiagnostic {
            path: java.into(),
            line: 2,
            col: 18,
            severity: Severity::Error,
            message: "パッケージgeneratedは存在しません".into(),
        }],
    }
}

#[test]
fn java_positive_reports_reuse_same_generation_and_never_get_kotlin_clean_authority() {
    let (_owner, request) = fixture();
    let mut ws = workspace(&request, Vec::new());
    let java = crate::paths::file_key(&request.project_dir.join("src/Main.java"));
    let kt = crate::paths::file_key(&request.project_dir.join("src/Helper.kt"));
    let target = ws.kotlin_diagnostic_targets().into_iter().next().unwrap().1;
    assert_eq!(
        target, kt,
        "the existing queue selects an open Kotlin source"
    );
    let pending = ws.begin_kotlin_diagnostics(&target).unwrap();
    assert!(pending.java_task.is_some());
    let invalid = Arc::new(AtomicBool::new(false));
    let changed = ws.finish_kotlin_diagnostics(&pending, positive(&java), invalid.clone());
    assert!(changed.contains(&java));
    assert_eq!(
        ws.kotlin_diagnostic_trace(&java).unwrap()["clean"],
        json!(false)
    );
    let accepted = ws.kotlin_diagnostic_result_id(&java).unwrap();
    // A retry on a source not previously reported may be Kotlin-clean but cannot clear Java.
    ws.change(&kt, format!("{KOTLIN}// new body\n"));
    assert!(ws.kotlin_diagnostic_result_id(&java).is_none());
    assert!(ws
        .finish_kotlin_diagnostics(&pending, positive(&java), invalid.clone())
        .is_empty());
    let newer = ws.begin_kotlin_diagnostics(&java).unwrap();
    let changed = ws.finish_kotlin_diagnostics(
        &newer,
        KotlinDiagnosticOutcome::Verified {
            java_clean: None,
            clean: true,
            diagnostics: Vec::new(),
        },
        invalid,
    );
    assert!(!changed.contains(&java));
    assert!(ws.kotlin_diagnostic_result_id(&java).is_none());
    assert!(ws.kotlin_diagnostic_result_id(&kt).is_some());
    assert!(!accepted.is_empty());
}

struct Provider;
impl crate::analysis_extensions::AnalysisProvider for Provider {
    fn id(&self) -> &str {
        "java-virtual-target-control"
    }
    fn analyze(
        &self,
        _: &mut ktcore::extensions::ReadSession<'_>,
    ) -> Result<(), ktcore::extensions::ContractError> {
        Ok(())
    }
}

#[test]
fn extension_provider_blocks_plain_java_negatives_without_disabling_kotlin_worker() {
    let (_owner, request) = fixture();
    let java = crate::paths::file_key(&request.project_dir.join("src/Main.java"));
    let mut ws = workspace(&request, vec![Arc::new(Provider)]);
    let pending = ws.begin_kotlin_diagnostics(&java).unwrap();
    assert!(pending.java_task.is_none());
    assert!(pending.task.kotlin_compiler.is_some());
    let changed =
        ws.finish_kotlin_diagnostics(&pending, positive(&java), Arc::new(AtomicBool::new(false)));
    assert!(!changed.contains(&java));
    assert!(ws.kotlin_diagnostic_result_id(&java).is_none());
}

#[test]
fn auxiliary_producer_kotlin_clean_does_not_discard_an_independent_java_positive() {
    let (_owner, mut request) = fixture();
    request.positive_diagnostics_only = true;
    let java = crate::paths::file_key(&request.project_dir.join("src/Main.java"));
    let kt = crate::paths::file_key(&request.project_dir.join("src/Helper.kt"));
    let mut ws = workspace(&request, Vec::new());
    let pending = ws.begin_kotlin_diagnostics(&java).unwrap();
    assert!(pending.positive_diagnostics_only);
    assert!(pending.java_task.is_some());
    let changed =
        ws.finish_kotlin_diagnostics(&pending, positive(&java), Arc::new(AtomicBool::new(false)));
    assert_eq!(changed, vec![java.clone()]);
    assert_eq!(
        ws.kotlin_diagnostic_trace(&java).unwrap()["clean"],
        json!(false)
    );
    assert!(ws.kotlin_diagnostic_result_id(&kt).is_none());
}

fn action_fact(declaring: &str) -> Value {
    json!({"adapter":"gradle-7.4.2-stock-actions-v1", "actions":[{
        "class":"org.gradle.api.internal.project.taskfactory.IncrementalInputsTaskAction",
        "declaringClass":declaring,"method":"compile","parameterTypes":["org.gradle.work.InputChanges"]}]})
}

fn only_if_fact(kapt: bool) -> Value {
    let mut predicates = vec![json!({"class":"org.gradle.api.internal.AbstractTask$8",
        "stockEnabledPredicate":true,"stockKaptPredicate":false})];
    if kapt {
        predicates.push(json!({"class":"org.jetbrains.kotlin.gradle.tasks.configuration.KaptConfigKt$sam$org_gradle_api_specs_Spec$0",
        "stockEnabledPredicate":false,"stockKaptPredicate":true}));
    }
    json!({"adapter":"gradle-7.4.2-stock-onlyif-v1", "predicates":predicates})
}

#[test]
fn inactive_kapt_requires_actual_stock_action_and_absent_raw_providers() {
    let (_owner, request) = fixture();
    let java = request.java_task.as_ref().unwrap();
    for case in [
        "missing-fact",
        "null-fact",
        "doLast",
        "actions-add",
        "wrong-method",
        "wrong-owner",
        "wrong-parameters",
        "unknown-action-adapter",
        "missing-provider-count",
        "empty-provider",
        "nested-empty-provider",
        "missing-onlyif",
        "custom-onlyif",
        "replacement-onlyif",
        "same-wrapper-custom-function",
        "unknown-onlyif-adapter",
    ] {
        let mut auxiliary = inactive_kapt_tasks(&request);
        let apt = &mut auxiliary[0];
        match case {
            "missing-fact" => {
                apt.options.remove("taskActionInventory");
            }
            "null-fact" => {
                apt.options
                    .insert("taskActionInventory".into(), Value::Null);
            }
            "doLast" | "actions-add" => {
                apt.options.get_mut("taskActionInventory").unwrap()["actions"]
                    .as_array_mut()
                    .unwrap()
                    .push(
                        json!({"class":"org.gradle.api.internal.AbstractTask$TaskActionWrapper"}),
                    );
            }
            "wrong-method" => {
                apt.options.get_mut("taskActionInventory").unwrap()["actions"][0]["method"] =
                    json!("other");
            }
            "wrong-owner" => {
                apt.options.get_mut("taskActionInventory").unwrap()["actions"][0]
                    ["declaringClass"] = json!("fixture.CustomProducer");
            }
            "wrong-parameters" => {
                apt.options.get_mut("taskActionInventory").unwrap()["actions"][0]
                    ["parameterTypes"] = json!([]);
            }
            "unknown-action-adapter" => {
                apt.options.get_mut("taskActionInventory").unwrap()["adapter"] = json!("unknown");
            }
            "missing-provider-count" => {
                apt.options.remove("annotationProcessorOptionProviderCount");
            }
            "empty-provider" | "nested-empty-provider" => {
                apt.options
                    .insert("annotationProcessorOptionProviderCount".into(), json!(1));
            }
            "missing-onlyif" => {
                apt.options.remove("taskOnlyIfInventory");
            }
            "custom-onlyif" => {
                apt.options.get_mut("taskOnlyIfInventory").unwrap()["predicates"].as_array_mut().unwrap()
                .push(json!({"class":"fixture.CustomPredicate","stockEnabledPredicate":false,"stockKaptPredicate":false}));
            }
            "replacement-onlyif" => {
                apt.options.get_mut("taskOnlyIfInventory").unwrap()["predicates"] = json!([]);
            }
            "same-wrapper-custom-function" => {
                apt.options.get_mut("taskOnlyIfInventory").unwrap()["predicates"][1]
                    ["stockKaptPredicate"] = json!(false);
            }
            "unknown-onlyif-adapter" => {
                apt.options.get_mut("taskOnlyIfInventory").unwrap()["adapter"] = json!("unknown");
            }
            _ => unreachable!(),
        }
        let mut tasks = vec![request.task.clone(), java.clone()];
        tasks.extend(auxiliary);
        assert!(
            !selected_tasks_supported(&request.task, java, &tasks),
            "{case}"
        );
    }
}

fn foreign_java_source(request: &KotlinDiagnosticRequest) -> SourceSetClasspath {
    let root = request.project_dir.join("dependency");
    let mut task = request.java_task.clone().unwrap();
    task.task = ":dependency:compileJava".into();
    task.source_files = vec![root.join("src/Dep.java")];
    task.declared_input_files = task.source_files.clone();
    task.classpath.clear();
    task.arguments.clear();
    task.output_roots = vec![root.join("build/classes/java/main")];
    task.declared_output_roots = vec![
        task.output_roots[0].clone(),
        root.join("build/headers"),
        root.join("build/generated"),
    ];
    task.options.insert(
        "taskActionInventory".into(),
        action_fact("org.gradle.api.tasks.compile.JavaCompile"),
    );
    task.options
        .insert("taskOnlyIfInventory".into(), only_if_fact(false));
    let mut recipe = task.java_compiler_recipe().unwrap();
    recipe.api_options.pop(); // Actual Gradle default -proc:none remains; no explicit raw override.
    for (at, path) in [
        (5, &task.declared_output_roots[0]),
        (7, &task.declared_output_roots[1]),
        (13, &task.declared_output_roots[2]),
    ] {
        recipe.api_options[at] = path_text(path).unwrap();
    }
    recipe.api_options[16] = String::new();
    task.options.insert(
        "javaCompilerRecipe".into(),
        serde_json::to_value(recipe).unwrap(),
    );
    SourceSetClasspath {
        name: "main".into(),
        source_roots: vec![root.join("src")],
        output_roots: task.output_roots.clone(),
        provided_artifacts: vec![root.join("build/libs/dependency.jar")],
        compiler_tasks: vec![task],
        ..Default::default()
    }
}

fn dependency_model(
    request: &KotlinDiagnosticRequest,
    foreign: SourceSetClasspath,
    read: Option<PathBuf>,
) -> ProjectSnapshot {
    let mut main = request.task.clone();
    let mut java = request.java_task.clone().unwrap();
    if let Some(read) = read {
        main.classpath.push(read.clone());
        java.classpath.push(read);
    }
    let mut recipe = java.java_compiler_recipe().unwrap();
    recipe.api_options[16] = joined_classpath(&java.classpath).unwrap();
    java.options.insert(
        "javaCompilerRecipe".into(),
        serde_json::to_value(recipe).unwrap(),
    );
    ProjectSnapshot::new(vec![
        ModuleClasspath {
            module: ":app".into(),
            project_dir: request.project_dir.clone(),
            build: ":".into(),
            entries: Vec::new(),
            unresolved: Vec::new(),
            source_sets: vec![SourceSetClasspath {
                name: "main".into(),
                source_roots: request.source_roots.clone(),
                entries: java.classpath.clone(),
                output_roots: main.output_roots.clone(),
                kotlin_compile_task: Some(main.task.clone()),
                compiler_tasks: vec![main, java],
                ..Default::default()
            }],
        },
        ModuleClasspath {
            module: ":dependency".into(),
            project_dir: request.project_dir.join("dependency"),
            build: ":".into(),
            entries: Vec::new(),
            unresolved: Vec::new(),
            source_sets: vec![foreign],
        },
    ])
}

fn participating_dependencies(
    request: &KotlinDiagnosticRequest,
    model: &ProjectSnapshot,
) -> Option<Vec<SourceSetClasspath>> {
    let key = crate::paths::file_key(&request.task.source_files[0]);
    let (_, source) = model.owner_for_key(&key).unwrap();
    let source = source.unwrap();
    let primary = source
        .compiler_tasks
        .iter()
        .find(|task| task.language == crate::classpath::CompilerLanguage::Kotlin)
        .unwrap();
    let java = source
        .compiler_tasks
        .iter()
        .find(|task| task.language == crate::classpath::CompilerLanguage::Java)
        .unwrap();
    let scope = model
        .file_origin(Path::new(&key))
        .unwrap()
        .project_scope
        .unwrap();
    let visibilities = model.scope_visibilities(&[]);
    let visible = visibilities
        .iter()
        .find(|v| v.scope == scope)
        .unwrap()
        .ordered_origins
        .iter()
        .cloned()
        .collect();
    import_dependencies(primary, java, source, model, &visibilities, &visible)
}

#[test]
fn participating_active_or_unknown_producers_deny_even_if_the_output_directory_exists() {
    let (_owner, request) = fixture();
    let source = foreign_java_source(&request);
    let read = source.output_roots[0].clone();
    let model = dependency_model(&request, source.clone(), Some(read.clone()));
    assert_eq!(
        participating_dependencies(&request, &model).unwrap().len(),
        1
    );
    for case in [
        "ksp",
        "kapt",
        "unknown-task",
        "incomplete",
        "unresolved",
        "missing-tasks",
        "missing-task-outputs",
        "missing-source-outputs",
        "custom-action",
        "provider",
        "active-ap",
        "missing-recipe",
        "custom-onlyif",
        "missing-onlyif",
    ] {
        let mut foreign = source.clone();
        match case {
            "ksp" | "kapt" | "missing-source-outputs" => {
                let mut producer = request.task.clone();
                producer.task = format!(":dependency:{case}");
                producer.task_type = format!("fixture.{case}Producer");
                producer.output_roots = vec![read.clone()];
                producer.declared_output_roots = vec![read.clone()];
                foreign.compiler_tasks.push(producer);
                if case == "missing-source-outputs" {
                    foreign.output_roots.clear();
                    foreign.provided_artifacts.clear();
                }
            }
            "unknown-task" => {
                foreign.compiler_tasks[0].task_type = "fixture.UnknownProducer".into();
            }
            "incomplete" => {
                foreign.compiler_metadata_incomplete = true;
            }
            "unresolved" => {
                foreign.unresolved.push("unknown".into());
            }
            "missing-tasks" => {
                foreign.compiler_tasks.clear();
            }
            "missing-task-outputs" => {
                foreign.compiler_tasks[0].output_roots.clear();
                foreign.compiler_tasks[0].declared_output_roots.clear();
            }
            "custom-action" => {
                foreign.compiler_tasks[0]
                    .options
                    .remove("taskActionInventory");
            }
            "provider" => {
                foreign.compiler_tasks[0].options.insert(
                    "javaCompilerRecipeProblem".into(),
                    json!("argument provider unavailable"),
                );
            }
            "active-ap" => {
                foreign.compiler_tasks[0]
                    .annotation_processor_path
                    .push(request.project_dir.join("processor.jar"));
            }
            "missing-recipe" => {
                foreign.compiler_tasks[0]
                    .options
                    .remove("javaCompilerRecipe");
            }
            "custom-onlyif" => {
                foreign.compiler_tasks[0].options.get_mut("taskOnlyIfInventory").unwrap()["predicates"].as_array_mut().unwrap()
                .push(json!({"class":"fixture.CustomPredicate","stockEnabledPredicate":false,"stockKaptPredicate":false}));
            }
            "missing-onlyif" => {
                foreign.compiler_tasks[0]
                    .options
                    .remove("taskOnlyIfInventory");
            }
            _ => unreachable!(),
        }
        let model = dependency_model(&request, foreign, Some(read.clone()));
        assert!(
            participating_dependencies(&request, &model).is_none(),
            "{case}/absent"
        );
        std::fs::create_dir_all(&read).unwrap();
        assert!(
            participating_dependencies(&request, &model).is_none(),
            "{case}/present"
        );
        std::fs::remove_dir(&read).unwrap();
    }
}

#[test]
fn provided_jar_and_unknown_outputs_cannot_hide_a_producer_but_unrelated_modules_are_ignored() {
    let (_owner, request) = fixture();
    let mut source = foreign_java_source(&request);
    let jar = source.provided_artifacts[0].clone();
    source.compiler_tasks.clear(); // Missing task metadata falls back to source-set artifact ownership.
    let model = dependency_model(&request, source.clone(), Some(jar.clone()));
    assert!(participating_dependencies(&request, &model).is_none());
    source.compiler_tasks.push(request.task.clone());
    let model = dependency_model(&request, source.clone(), None);
    // This clone owns the app Kotlin CP slot, so it is an intersecting unknown producer.
    assert!(participating_dependencies(&request, &model).is_none());
    source = foreign_java_source(&request);
    let model = dependency_model(&request, source, Some(jar));
    assert!(
        participating_dependencies(&request, &model).is_none(),
        "Jar/resource naming has no current proof yet"
    );
}

#[test]
fn genuinely_unrelated_unknown_producers_do_not_disable_owner_import_analysis() {
    let (_owner, request) = fixture();
    let mut source = foreign_java_source(&request);
    source.compiler_tasks[0].task_type = "example.UnknownProducer".into();
    source.compiler_tasks[0].complete = false;
    // Keep its actual foreign output paths and do not add them to either owner read classpath.
    let model = dependency_model(&request, source, None);
    assert!(participating_dependencies(&request, &model)
        .unwrap()
        .is_empty());
}

#[test]
fn current_foreign_source_packages_block_missing_imports_after_new_saved_source_and_package_move() {
    let (_owner, mut request) = fixture();
    let source = foreign_java_source(&request);
    let path = &source.compiler_tasks[0].source_files[0];
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "package processor; class Dep {}\n").unwrap();
    request.java_dependencies = vec![source.clone()];
    let (packages, _) = dependency_domains(&request).unwrap();
    assert!(disjoint_import("generated.NewType", &packages));
    let saved_new = source.source_roots[0].join("NewType.java");
    assert!(!source.compiler_tasks[0].source_files.contains(&saved_new));
    std::fs::write(&saved_new, "package generated; class NewType {}\n").unwrap();
    let (packages, receipt) = dependency_domains(&request).unwrap();
    assert!(!disjoint_import("generated.NewType", &packages));
    assert_eq!(receipt["sourceDomains"].as_array().unwrap().len(), 2);
    std::fs::remove_file(&saved_new).unwrap();
    std::fs::write(path, "package generated; class Dep {}\n").unwrap();
    let (packages, _) = dependency_domains(&request).unwrap();
    assert!(!disjoint_import("generated.NewType", &packages));
    let bytes = std::fs::read(path).unwrap();
    request.dependency_overlays.push(KotlinSourceOverlay::new(
        crate::paths::file_key(path),
        "package third; class Dep {}\n".into(),
        2,
    ));
    assert!(
        KotlinEditorOverlay::create_in(&request, &request.project_dir.join("private-overlay"))
            .is_err(),
        "existing dirty-visible-dependency boundary rejects before any compiler/Java absence"
    );
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn foreign_package_proof_rejects_unknown_syntax_unicode_escapes_modules_and_decorations() {
    let mut parser = crate::java::JavaParser::new();
    assert_eq!(
        java_source_package(&mut parser, "package processor; class Dep {}\n").unwrap(),
        "processor"
    );
    for text in [
        "package generated.; class Dep {}",
        "package \\u0067enerated; class Dep {}",
        "module foo {}",
        "@Unknown package generated;",
        "package generated; class Dep {",
        "package 日本; class Dep {}",
        "class Dep {}",
    ] {
        assert!(java_source_package(&mut parser, text).is_err(), "{text}");
    }
}

#[test]
fn selected_foreign_source_outside_exported_roots_and_new_siblings_are_not_omitted() {
    let (_owner, mut request) = fixture();
    let mut source = foreign_java_source(&request);
    let inside = source.compiler_tasks[0].source_files[0].clone();
    let outside = request.project_dir.join("custom-sources/Outside.java");
    for file in [&inside, &outside] {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, "package processor; class Dep {}\n").unwrap();
    }
    source.compiler_tasks[0].source_files.push(outside.clone());
    request.java_dependencies = vec![source];
    let (packages, first) = dependency_domains(&request).unwrap();
    assert!(disjoint_import("generated.NewType", &packages));
    assert_eq!(first["sourceDomains"].as_array().unwrap().len(), 2);
    let inputs = dependency_inputs(&request).unwrap();
    assert!(inputs.contains(outside.parent().unwrap()));
    let plan =
        crate::compiled_watch::WatchPlan::read_inputs(&inputs.into_iter().collect::<Vec<_>>())
            .unwrap();
    assert!(plan.includes_directory_watch(outside.parent().unwrap()));
    std::fs::write(
        outside.parent().unwrap().join("NewType.java"),
        "package generated; class NewType {}\n",
    )
    .unwrap();
    let (packages, second) = dependency_domains(&request).unwrap();
    assert!(!disjoint_import("generated.NewType", &packages));
    assert_eq!(second["sourceDomains"].as_array().unwrap().len(), 3);
    std::fs::remove_file(outside).unwrap();
    assert!(
        dependency_domains(&request).is_err(),
        "a missing selected input cannot become an empty domain"
    );
}

#[test]
fn unavailable_foreign_producer_retains_the_canonical_kotlin_worker_and_no_java_authority() {
    let (_owner, request) = fixture();
    let mut foreign = foreign_java_source(&request);
    let read = foreign.output_roots[0].clone();
    foreign.compiler_metadata_incomplete = true;
    let mut ws = Workspace::new();
    ws.set_project_model(Arc::new(dependency_model(&request, foreign, Some(read))));
    for source in &request.overlays {
        ws.open(source.key.clone(), source.text.clone());
    }
    let kt = crate::paths::file_key(&request.project_dir.join("src/Helper.kt"));
    let java = crate::paths::file_key(&request.project_dir.join("src/Main.java"));
    let pending = ws.begin_kotlin_diagnostics(&kt).unwrap();
    assert!(pending.java_task.is_none() && pending.java_dependencies.is_empty());
    assert!(pending.task.kotlin_compiler.is_some());
    let diagnostic = CompileDiagnostic {
        path: kt.clone(),
        line: 2,
        col: 1,
        severity: Severity::Error,
        message: "canonical Kotlin".into(),
    };
    let changed = ws.finish_kotlin_diagnostics(
        &pending,
        KotlinDiagnosticOutcome::Verified {
            java_clean: None,
            clean: false,
            diagnostics: vec![diagnostic],
        },
        Arc::new(AtomicBool::new(false)),
    );
    assert!(changed.contains(&kt));
    assert!(ws.kotlin_diagnostic_result_id(&kt).is_some());
    assert!(ws.kotlin_diagnostic_result_id(&java).is_none());
}

#[test]
fn selected_java_custom_actions_or_predicates_cannot_authorize_missing_imports() {
    let (_owner, request) = fixture();
    for case in [
        "missing-actions",
        "custom-action",
        "missing-onlyif",
        "custom-onlyif",
    ] {
        let mut java = request.java_task.clone().unwrap();
        match case {
            "missing-actions" => {
                java.options.remove("taskActionInventory");
            }
            "custom-action" => {
                java.options.get_mut("taskActionInventory").unwrap()["actions"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"class":"fixture.ProducerAction"}));
            }
            "missing-onlyif" => {
                java.options.remove("taskOnlyIfInventory");
            }
            "custom-onlyif" => {
                java.options.get_mut("taskOnlyIfInventory").unwrap()["predicates"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"stockEnabledPredicate":false,"stockKaptPredicate":false}));
            }
            _ => unreachable!(),
        }
        assert!(
            !selected_tasks_supported(&request.task, &java, &[request.task.clone(), java.clone()]),
            "{case}"
        );
    }
}

#[test]
fn selected_kotlin_and_stubs_custom_producers_cannot_be_replaced_by_a_partial_header_lease() {
    let (_owner, request) = fixture();
    let java = request.java_task.as_ref().unwrap();
    for which in ["primary", "stubs"] {
        for case in [
            "missing-actions",
            "custom-action",
            "missing-onlyif",
            "custom-onlyif",
            "wrong-method",
        ] {
            let mut primary = request.task.clone();
            let mut auxiliary = inactive_kapt_tasks(&request);
            let task = if which == "primary" {
                &mut primary
            } else {
                &mut auxiliary[1]
            };
            match case {
                "missing-actions" => {
                    task.options.remove("taskActionInventory");
                }
                "custom-action" => {
                    task.options.get_mut("taskActionInventory").unwrap()["actions"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!({"class":"fixture.WritesUndeclaredApiToReadSlot"}));
                }
                "missing-onlyif" => {
                    task.options.remove("taskOnlyIfInventory");
                }
                "custom-onlyif" => {
                    task.options.get_mut("taskOnlyIfInventory").unwrap()["predicates"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!({"stockEnabledPredicate":false,"stockKaptPredicate":false}));
                }
                "wrong-method" => {
                    task.options.get_mut("taskActionInventory").unwrap()["actions"][0]["method"] =
                        json!("other");
                }
                _ => unreachable!(),
            }
            let mut tasks = vec![primary.clone(), java.clone()];
            tasks.extend(auxiliary);
            assert!(
                !selected_tasks_supported(&primary, java, &tasks),
                "{which}/{case}"
            );
        }
    }
}

#[test]
fn closed_current_java_source_is_included_and_a_missing_closed_input_is_unavailable() {
    let (owner, mut request) = fixture();
    for source in &request.overlays {
        let path = Path::new(&source.key);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, &source.text).unwrap();
    }
    let closed = request.source_roots[0].join("Current.java");
    let text = "package generated; class Current {}\n";
    std::fs::write(&closed, text).unwrap();
    request.sources.push(closed.clone());
    request.task.source_files.push(closed.clone());
    request.task.declared_input_files.push(closed.clone());
    let java = request.java_task.as_mut().unwrap();
    java.source_files.push(closed.clone());
    java.declared_input_files.push(closed.clone());
    assert!(!request
        .overlays
        .iter()
        .any(|source| same_path(Path::new(&source.key), &closed)));
    assert!(configured_recipe(&request).is_ok());
    let plan =
        crate::compiled_watch::WatchPlan::read_inputs(&owned_membership_inputs(&request).unwrap())
            .unwrap();
    let receipt = owned_source_membership(&request, &plan).unwrap();
    assert!(receipt["diskSources"]
        .as_array()
        .unwrap()
        .contains(&json!(header_key(&closed))));
    let overlay =
        KotlinEditorOverlay::create_in(&request, &owner.directory.join("closed-source-overlay"))
            .unwrap();
    let copied = overlay
        .sources
        .iter()
        .find(|path| {
            overlay
                .paths
                .get(&kotlin_path_key(&crate::paths::file_key(path)))
                .is_some_and(|key| same_path(Path::new(key), &closed))
        })
        .unwrap();
    assert_eq!(std::fs::read_to_string(copied).unwrap(), text);
    std::fs::remove_file(&closed).unwrap();
    assert!(
        owned_source_membership(&request, &plan).is_err(),
        "closed selected source cannot silently disappear"
    );
}

#[test]
fn saved_closed_java_and_kotlin_arrivals_cannot_authorize_missing_imports_from_an_old_task_list() {
    for (name, text) in [
        ("NewType.java", "package generated; class NewType {}\n"),
        ("NewType.kt", "package generated\nclass NewType\n"),
    ] {
        let (owner, request) = fixture();
        for source in &request.overlays {
            let path = Path::new(&source.key);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, &source.text).unwrap();
        }
        let plan = crate::compiled_watch::WatchPlan::read_inputs(
            &owned_membership_inputs(&request).unwrap(),
        )
        .unwrap();
        assert!(owned_source_membership(&request, &plan).is_ok());
        let added = request.source_roots[0].join(name);
        std::fs::write(&added, text).unwrap();
        assert!(!request.task.source_files.contains(&added));
        assert!(!request
            .overlays
            .iter()
            .any(|source| same_path(Path::new(&source.key), &added)));
        assert!(
            configured_recipe(&request).is_ok(),
            "old exported task membership alone misses this current source"
        );
        assert!(owned_source_membership(&request, &plan).is_err(), "{name}");
        let overlay =
            KotlinEditorOverlay::create_in(&request, &owner.directory.join("canonical-inputs"))
                .unwrap();
        assert_eq!(
            overlay.sources.len(),
            request.sources.len(),
            "auxiliary failure does not exclude canonical inputs"
        );
        assert_eq!(std::fs::read_to_string(&added).unwrap(), text);
        std::fs::remove_file(&added).unwrap();
        assert!(
            owned_source_membership(&request, &plan).is_ok(),
            "membership is current, not a permanent model denial"
        );
    }
}

#[test]
fn closed_source_edits_before_owner_watch_cannot_authorize_import_absence_but_wire_bodies_can() {
    for (extension, original_text, body_edit) in [
        (
            "java",
            JAVA,
            JAVA.replace("editorUnsaved()", "otherUnsaved()"),
        ),
        ("kt", KOTLIN, KOTLIN.replace("= 1", "= 2")),
    ] {
        let (owner, mut request) = fixture();
        let original = request
            .sources
            .iter()
            .find(|path| path.extension().unwrap() == extension)
            .unwrap()
            .clone();
        let key = crate::paths::file_key(&original);
        request.overlays.retain(|source| source.key != key);
        std::fs::create_dir_all(original.parent().unwrap()).unwrap();
        std::fs::write(&original, original_text).unwrap();
        let overlay = KotlinEditorOverlay::create_in(
            &request,
            &owner.directory.join("canonical-closed-copy"),
        )
        .unwrap();
        let expected = current_sources(&overlay).unwrap();
        assert!(check_closed_sources(&request, &overlay, &expected).is_ok());
        for changed in [original_text.replace("fixture", "generated"), body_edit] {
            // This is the concrete pre-watch window: canonical already copied the old body.
            std::fs::write(&original, &changed).unwrap();
            let plan = crate::compiled_watch::WatchPlan::read_inputs(
                &owned_membership_inputs(&request).unwrap(),
            )
            .unwrap();
            assert!(
                owned_source_membership(&request, &plan).is_ok(),
                "same paths conceal the changed {extension} body"
            );
            assert!(check_closed_sources(&request, &overlay, &expected).is_err());
            assert_eq!(
                current_sources(&overlay).unwrap(),
                expected,
                "auxiliary denial preserves canonical private inputs"
            );
        }
        std::fs::write(&original, original_text).unwrap();
        assert!(
            check_closed_sources(&request, &overlay, &expected).is_ok(),
            "closed current bytes recover"
        );
        request.overlays.push(KotlinSourceOverlay::new(
            key.clone(),
            original_text.into(),
            2,
        ));
        std::fs::write(&original, original_text.replace("fixture", "diskonly")).unwrap();
        assert!(
            check_closed_sources(&request, &overlay, &expected).is_ok(),
            "wire body owns the canonical input despite disk divergence"
        );
        std::fs::remove_file(&original).unwrap();
        assert!(
            check_closed_sources(&request, &overlay, &expected).is_ok(),
            "wire input need not have a disk file"
        );
        let private = overlay
            .sources
            .iter()
            .find(|path| path.extension().unwrap() == extension)
            .unwrap();
        std::fs::write(private, original_text.replace("fixture", "tampered")).unwrap();
        let tampered = current_sources(&overlay).unwrap();
        assert!(
            check_closed_sources(&request, &overlay, &tampered).is_err(),
            "private wire bytes must match the exact current request body"
        );
        std::fs::write(private, original_text).unwrap();
        assert!(check_closed_sources(&request, &overlay, &expected).is_ok());
        request.overlays.retain(|source| source.key != key);
        assert!(
            check_closed_sources(&request, &overlay, &expected).is_err(),
            "unreadable closed input is unavailable"
        );
    }
}

fn selected_k2(request: &mut KotlinDiagnosticRequest) {
    let runtime = request.task.kotlin_compiler.as_mut().unwrap();
    runtime.compiler_version = "2.4.0".into();
    runtime.gradle_plugin_version = "2.4.0".into();
    runtime.run_via_build_tools_api = true;
    runtime.execution_strategy = crate::classpath::KotlinCompilerExecutionStrategy::InProcess;
    for task in [&mut request.task, request.java_task.as_mut().unwrap()] {
        task.options.get_mut("taskActionInventory").unwrap()["adapter"] =
            json!("gradle-9.6.1-stock-actions-v1");
        task.options.get_mut("taskActionInventory").unwrap()["actions"][0]["class"] =
            json!("org.gradle.api.internal.project.taskfactory.IncrementalTaskAction");
        task.options.get_mut("taskOnlyIfInventory").unwrap()["adapter"] =
            json!("gradle-9.6.1-stock-onlyif-v1");
    }
    let mut recipe = request
        .java_task
        .as_ref()
        .unwrap()
        .java_compiler_recipe()
        .unwrap();
    recipe.adapter = "gradle-9.6.1-current-jdk17-javac-api-v1".into();
    recipe.api_options.pop();
    set_recipe(request, recipe);
    request.java_task.as_mut().unwrap().arguments.clear();
}

#[test]
fn k2_normal_profile_reuses_exact_options_and_rejects_auxiliary_and_mixed_profiles() {
    let (_owner, mut request) = fixture();
    selected_k2(&mut request);
    let java = request.java_task.as_ref().unwrap();
    let recipe = configured_recipe(&request).unwrap();
    let private = request.project_dir.join("private");
    let replayed = replay_options(java, &recipe, &request.task.output_roots[0], &private).unwrap();
    assert_eq!(replayed.len(), 17);
    for (position, original) in recipe.api_options.iter().enumerate() {
        if ![5, 7, 13, 16].contains(&position) {
            assert_eq!(&replayed[position], original);
        }
    }
    let mut tasks = vec![request.task.clone(), java.clone()];
    assert!(selected_tasks_supported(&request.task, java, &tasks));
    tasks.push(java.clone());
    tasks[2].task = ":unprovedProducer".into();
    assert!(!selected_tasks_supported(&request.task, java, &tasks));
    let mut mixed = java.clone();
    mixed.options.get_mut("taskOnlyIfInventory").unwrap()["adapter"] =
        json!("gradle-7.4.2-stock-onlyif-v1");
    assert!(!selected_tasks_supported(
        &request.task,
        &mixed,
        &[request.task.clone(), mixed.clone()]
    ));
    request
        .task
        .kotlin_compiler
        .as_mut()
        .unwrap()
        .run_via_build_tools_api = false;
    assert!(configured_recipe(&request).is_err());
}

#[test]
fn k2_complete_receipt_cannot_borrow_k1_binding_or_error_header_authority() {
    let (owner, mut request) = fixture();
    selected_k2(&mut request);
    let receipt = json!({"schema": 2, "component": "k2-complete-output", "compilerVersion": "2.4.0",
        "callbackCount": 1, "stageCompleted": true, "factoryDone": true, "allSelectedSourcesIncluded": true,
        "packageDomainProof": {"schema": 2, "basis": "original-fir-complete-output"}});
    assert!(validate_header_stage(&request, &receipt, &[], true).unwrap());
    assert!(validate_header_stage(&request, &receipt, &[], false).is_err());
    let error = CompileDiagnostic {
        path: "Use.kt".into(),
        line: 1,
        col: 1,
        severity: Severity::Error,
        message: "canonical body error".into(),
    };
    assert!(validate_header_stage(&request, &receipt, &[error], true).is_err());
    for (field, value) in [
        ("schema", json!(1)),
        ("component", json!("tool-owned-compiler-core-header-probe")),
        ("compilerVersion", json!("2.4.1")),
        ("callbackCount", json!(2)),
        ("bindingIdentityPreserved", json!(true)),
        ("bindingDiagnosticsPreserved", json!(true)),
        ("failure", json!("observer failed")),
        ("stageCompleted", json!(false)),
    ] {
        let mut invalid = receipt.clone();
        invalid[field] = value;
        assert!(
            validate_header_stage(&request, &invalid, &[], true).is_err(),
            "{field}"
        );
    }
    let overlay =
        KotlinEditorOverlay::create_in(&request, &owner.directory.join("overlays")).unwrap();
    let expected = current_sources(&overlay).unwrap();
    // Stage completion without source/domain/output accounting never creates a lease.
    assert!(validated_lease(&request, &overlay, &receipt, &expected, &[], true).is_err());
    assert!(!overlay.directory.join("lease").exists());
    let (_legacy_owner, legacy) = fixture();
    assert!(validate_header_stage(&legacy, &receipt, &[], true).is_err());
}

#[test]
fn k2_complete_outputs_require_all_canonical_bytes_and_stable_baseline() {
    let (owner, _) = fixture();
    let canonical = owner.directory.join("canonical");
    let observed = owner.directory.join("observed");
    for root in [&canonical, &observed] {
        std::fs::create_dir_all(root.join("fixture")).unwrap();
        std::fs::create_dir_all(root.join("META-INF")).unwrap();
        std::fs::write(root.join("fixture/Helper.class"), b"synthetic class bytes").unwrap();
        std::fs::write(
            root.join("META-INF/main.kotlin_module"),
            b"synthetic module bytes",
        )
        .unwrap();
    }
    let before = complete_output_inventory(&canonical).unwrap();
    complete_outputs_match(&canonical, &observed, &before).unwrap();
    std::fs::write(
        observed.join("META-INF/main.kotlin_module"),
        b"different module bytes",
    )
    .unwrap();
    assert!(complete_outputs_match(&canonical, &observed, &before).is_err());
    std::fs::write(
        observed.join("META-INF/main.kotlin_module"),
        b"synthetic module bytes",
    )
    .unwrap();
    std::fs::write(observed.join("fixture/Extra.class"), b"extra output").unwrap();
    assert!(complete_outputs_match(&canonical, &observed, &before).is_err());
    std::fs::remove_file(observed.join("fixture/Extra.class")).unwrap();
    std::fs::remove_file(observed.join("fixture/Helper.class")).unwrap();
    assert!(complete_outputs_match(&canonical, &observed, &before).is_err());
    std::fs::write(observed.join("fixture/Helper.class"), b"changed canonical").unwrap();
    std::fs::write(canonical.join("fixture/Helper.class"), b"changed canonical").unwrap();
    assert!(complete_outputs_match(&canonical, &observed, &before).is_err());
}

#[test]
fn k2_original_fir_domains_require_current_source_hash_and_identity() {
    let (owner, mut request) = fixture();
    selected_k2(&mut request);
    let overlay =
        KotlinEditorOverlay::create_in(&request, &owner.directory.join("overlays")).unwrap();
    let expected = current_sources(&overlay).unwrap();
    let inventory: Vec<_> = overlay
        .sources
        .iter()
        .map(|path| {
            json!({
                "kind": if path.extension().unwrap() == "kt" { "kotlin" } else { "java" },
                "path": path, "sha256": expected[&header_key(path)]
            })
        })
        .collect();
    let source = overlay
        .sources
        .iter()
        .find(|path| path.extension().unwrap() == "kt")
        .unwrap();
    let source = header_key(source);
    let receipt = json!({"schema": 2, "component": "k2-complete-output", "compilerVersion": "2.4.0",
        "callbackCount": 1, "stageCompleted": true, "factoryDone": true, "allSelectedSourcesIncluded": true,
        "sourceInventory": inventory,
        "packageDomainProof": {"schema": 2, "basis": "original-fir-complete-output", "stageCompleted": true,
            "allPackageDomainsKnown": true, "nameDomainIsWholePackage": true, "globalIssues": [],
            "sourceDomains": [{"source": source, "package": "fixture", "status": "KnownWholePackage",
                "issues": [], "originalFir": true, "sourceSha256": expected[&source]}]}});
    for (field, value, reason) in [
        ("originalFir", json!(false), "java-receipt-flag-originalFir"),
        (
            "sourceSha256",
            json!("stale-source"),
            "java-k2-original-fir-source-identity-differs",
        ),
        (
            "source",
            json!("/unselected.kt"),
            "java-domain-source-membership-differs",
        ),
    ] {
        let mut invalid = receipt.clone();
        invalid["packageDomainProof"]["sourceDomains"][0][field] = value;
        let error =
            validated_lease(&request, &overlay, &invalid, &expected, &[], true).unwrap_err();
        assert!(error.to_string().contains(reason), "{field}: {error}");
        assert!(!overlay.directory.join("lease").exists());
    }
}

#[test]
fn k2_metadata_and_empty_source_lease_enforce_origins_roles_and_current_fir() {
    for case in [
        "empty",
        "kotlin-origin",
        "named",
        "changed-hash",
        "unselected-origin",
        "java-origin",
        "unknown-path",
        "unknown-name",
        "extra-metadata",
    ] {
        let (owner, mut request) = fixture();
        selected_k2(&mut request);
        if case != "named" {
            for source in &mut request.overlays {
                if source.key.ends_with(".kt") {
                    *source =
                        KotlinSourceOverlay::new(source.key.clone(), "package fixture\n".into(), 1);
                }
            }
        }
        let overlay =
            KotlinEditorOverlay::create_in(&request, &owner.directory.join("overlays")).unwrap();
        let expected = current_sources(&overlay).unwrap();
        let inventory: Vec<_> = overlay
            .sources
            .iter()
            .map(|path| {
                json!({
                    "kind": if path.extension().unwrap() == "kt" { "kotlin" } else { "java" },
                    "path": path, "sha256": expected[&header_key(path)]
                })
            })
            .collect();
        let source = header_key(
            overlay
                .sources
                .iter()
                .find(|path| path.extension().unwrap() == "kt")
                .unwrap(),
        );
        let raw = overlay.directory.join("raw-headers/META-INF");
        std::fs::create_dir_all(&raw).unwrap();
        let metadata = b"synthetic module metadata for lease validation";
        std::fs::write(raw.join("main.kotlin_module"), metadata).unwrap();
        let mut receipt = json!({"schema": 2, "component": "k2-complete-output", "compilerVersion": "2.4.0",
            "callbackCount": 1, "stageCompleted": true, "factoryDone": true, "allSelectedSourcesIncluded": true,
            "sourceInventory": inventory,
            "outputs": [{"relativePath": "META-INF/main.kotlin_module", "bytes": metadata.len(), "sha256": sha(metadata),
                "sources": [], "role": "k2-module-metadata", "apiStatus": "CompleteCompilation"}],
            "packageDomainProof": {"schema": 2, "basis": "original-fir-complete-output", "stageCompleted": true,
                "allPackageDomainsKnown": true, "nameDomainIsWholePackage": true, "globalIssues": [], "rawClasses": [],
                "sourceDomains": [{"source": source, "package": "fixture", "status": "KnownWholePackage", "issues": [],
                    "originalFir": true, "declarationCount": if case == "named" { 1 } else { 0 },
                    "sourceSha256": if case == "changed-hash" { "stale".to_owned() } else { expected[&source].clone() }}]}});
        match case {
            "kotlin-origin" | "named" => receipt["outputs"][0]["sources"] = json!([source]),
            "unselected-origin" => receipt["outputs"][0]["sources"] = json!(["/unselected.kt"]),
            "java-origin" => {
                let java = overlay
                    .sources
                    .iter()
                    .find(|path| path.extension().unwrap() == "java")
                    .unwrap();
                receipt["outputs"][0]["sources"] = json!([header_key(java)]);
            }
            "unknown-path" | "unknown-name" => {
                let relative = if case == "unknown-path" {
                    "other/main.kotlin_module"
                } else {
                    "META-INF/unproved name.kotlin_module"
                };
                let path = overlay.directory.join("raw-headers").join(relative);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, metadata).unwrap();
                receipt["outputs"][0]["relativePath"] = json!(relative);
            }
            "extra-metadata" => {
                std::fs::write(raw.join("extra.kotlin_module"), metadata).unwrap();
                let mut extra = receipt["outputs"][0].clone();
                extra["relativePath"] = json!("META-INF/extra.kotlin_module");
                receipt["outputs"].as_array_mut().unwrap().push(extra);
            }
            _ => {}
        }
        let result = validated_lease(&request, &overlay, &receipt, &expected, &[], true);
        if matches!(case, "empty" | "kotlin-origin") {
            assert_eq!(result.unwrap(), BTreeSet::from(["fixture".into()]));
            assert_eq!(
                std::fs::read_dir(overlay.directory.join("lease"))
                    .unwrap()
                    .count(),
                0
            );
        } else {
            let reason = match case {
                "named" => "java-raw-header-source-coverage-differs",
                "changed-hash" => "java-k2-original-fir-source-identity-differs",
                "extra-metadata" => "java-k2-metadata-inventory-unhandled",
                _ => "java-header-output-source-unaccounted",
            };
            assert!(result.unwrap_err().to_string().contains(reason), "{case}");
        }
    }
}

fn selected_k2_jdk21(request: &mut KotlinDiagnosticRequest) {
    selected_k2(request);
    request
        .task
        .kotlin_compiler
        .as_mut()
        .unwrap()
        .execution_strategy = crate::classpath::KotlinCompilerExecutionStrategy::Daemon;
    let mut recipe = request
        .java_task
        .as_ref()
        .unwrap()
        .java_compiler_recipe()
        .unwrap();
    recipe.adapter = "gradle-9.6.1-current-jdk21-utf8-javac-api-v1".into();
    recipe.language_version = "21".into();
    recipe.gradle_java_version = "21.0.3".into();
    recipe.java_runtime_version = "21.0.3+7-LTS-152".into();
    recipe.gradle_file_encoding = "UTF-8".into();
    recipe.api_options[1] = "21".into();
    recipe.api_options[3] = "21".into();
    recipe
        .api_options
        .splice(6..6, ["-encoding".into(), "UTF-8".into()]);
    set_recipe(request, recipe);
    let task = request.java_task.as_mut().unwrap();
    task.options.insert("encoding".into(), json!("UTF-8"));
    task.options
        .insert("sourceCompatibility".into(), json!("21"));
    task.options
        .insert("targetCompatibility".into(), json!("21"));
}

// Actual unchanged generic fixture getter: SDK21, target17, default UTF8, seventeen API options.
fn selected_captured_k2_jdk21_target17(request: &mut KotlinDiagnosticRequest) {
    selected_k2_jdk21(request);
    request
        .task
        .kotlin_compiler
        .as_mut()
        .unwrap()
        .execution_strategy = crate::classpath::KotlinCompilerExecutionStrategy::InProcess;
    let mut recipe = request
        .java_task
        .as_ref()
        .unwrap()
        .java_compiler_recipe()
        .unwrap();
    recipe.adapter = "gradle-9.6.1-current-jdk21-target17-default-utf8-javac-api-v1".into();
    recipe.api_options[1] = "17".into();
    recipe.api_options[3] = "17".into();
    recipe.api_options.drain(6..8);
    set_recipe(request, recipe);
    let task = request.java_task.as_mut().unwrap();
    task.options.insert("encoding".into(), Value::Null);
    task.options
        .insert("sourceCompatibility".into(), json!("17"));
    task.options
        .insert("targetCompatibility".into(), json!("17"));
}

#[test]
fn captured_gradle961_jdk21_target17_default_utf8_preserves_order_and_output_roles() {
    let (_owner, mut request) = fixture();
    selected_captured_k2_jdk21_target17(&mut request);
    let task = request.java_task.as_ref().unwrap();
    let recipe = configured_recipe(&request).unwrap();
    assert_eq!(recipe.language_version, "21");
    assert_eq!(recipe.api_options.len(), 17);
    assert!(!recipe
        .api_options
        .iter()
        .any(|arg| arg == "-encoding" || arg == "--release"));
    assert!(task.arguments.is_empty());
    assert!(selected_tasks_supported(
        &request.task,
        task,
        &[request.task.clone(), task.clone()]
    ));
    let private = request.project_dir.join("private-captured-target17");
    let replayed = replay_options(task, &recipe, &request.task.output_roots[0], &private).unwrap();
    for (index, original) in recipe.api_options.iter().enumerate() {
        if ![5, 7, 13, 16].contains(&index) {
            assert_eq!(&replayed[index], original);
        }
    }
    assert_eq!(&replayed[..5], &["-source", "17", "-target", "17", "-d"]);
    assert_eq!(
        &replayed[8..13],
        &["-g", "-sourcepath", "", "-proc:none", "-s"]
    );
    assert_eq!(replayed[14], "-XDuseUnsharedTable=true");
    assert_eq!(replayed[15], "-classpath");
    for (index, directory) in [
        (5, "java-classes"),
        (7, "java-headers"),
        (13, "java-generated"),
    ] {
        assert_eq!(
            replayed[index],
            path_text(&private.join(directory)).unwrap()
        );
    }
    assert_eq!(
        replayed[16],
        joined_classpath(&[task.classpath[0].clone(), private.join("lease")]).unwrap()
    );
}

#[test]
fn captured_gradle961_jdk21_target17_default_utf8_rejects_unproved_policies() {
    for kind in [
        "encoding",
        "source",
        "target",
        "release",
        "runtime",
        "charset",
        "api-order",
        "api-target",
        "processor",
        "strategy",
    ] {
        let (_owner, mut request) = fixture();
        selected_captured_k2_jdk21_target17(&mut request);
        match kind {
            "encoding" => {
                request
                    .java_task
                    .as_mut()
                    .unwrap()
                    .options
                    .insert("encoding".into(), json!("UTF-8"));
            }
            "source" => {
                request
                    .java_task
                    .as_mut()
                    .unwrap()
                    .options
                    .insert("sourceCompatibility".into(), json!("21"));
            }
            "target" => {
                request
                    .java_task
                    .as_mut()
                    .unwrap()
                    .options
                    .insert("targetCompatibility".into(), json!("21"));
            }
            "release" => {
                request
                    .java_task
                    .as_mut()
                    .unwrap()
                    .options
                    .insert("release".into(), json!(17));
            }
            "processor" => request
                .java_task
                .as_mut()
                .unwrap()
                .annotation_processor_path
                .push(request.project_dir.join("processor.jar")),
            "strategy" => {
                request
                    .task
                    .kotlin_compiler
                    .as_mut()
                    .unwrap()
                    .execution_strategy =
                    crate::classpath::KotlinCompilerExecutionStrategy::OutOfProcess
            }
            _ => {
                let mut recipe = request
                    .java_task
                    .as_ref()
                    .unwrap()
                    .java_compiler_recipe()
                    .unwrap();
                match kind {
                    "runtime" => recipe.java_runtime_version = "17.0.7".into(),
                    "charset" => recipe.effective_charset = "windows-31j".into(),
                    "api-order" => recipe.api_options[0] = "-target".into(),
                    "api-target" => recipe.api_options[3] = "21".into(),
                    _ => unreachable!(),
                }
                set_recipe(&mut request, recipe);
            }
        }
        assert!(configured_recipe(&request).is_err(), "{kind}");
    }
}

#[test]
fn captured_target17_retains_daemon_but_rejects_opaque_jvm_options_for_both_strategies() {
    for strategy in [
        crate::classpath::KotlinCompilerExecutionStrategy::InProcess,
        crate::classpath::KotlinCompilerExecutionStrategy::Daemon,
    ] {
        let (_owner, mut request) = fixture();
        selected_captured_k2_jdk21_target17(&mut request);
        request
            .task
            .kotlin_compiler
            .as_mut()
            .unwrap()
            .execution_strategy = strategy;
        assert!(configured_recipe(&request).is_ok());
        request
            .task
            .kotlin_compiler
            .as_mut()
            .unwrap()
            .normalized_daemon_jvm_arguments
            .push("-Dopaque=true".into());
        assert!(configured_recipe(&request).is_err());
    }
}

#[test]
fn k2_jdk21_daemon_recipe_preserves_ordered_explicit_utf8_and_output_roles() {
    let (_owner, mut request) = fixture();
    selected_k2_jdk21(&mut request);
    let task = request.java_task.as_ref().unwrap();
    let recipe = configured_recipe(&request).unwrap();
    let private = request.project_dir.join("private");
    let replayed = replay_options(task, &recipe, &request.task.output_roots[0], &private).unwrap();
    assert_eq!(replayed.len(), 19);
    assert_eq!(&replayed[6..8], &["-encoding", "UTF-8"]);
    assert_eq!(replayed[1], "21");
    assert_eq!(replayed[3], "21");
    for (position, original) in recipe.api_options.iter().enumerate() {
        if ![5, 9, 15, 18].contains(&position) {
            assert_eq!(&replayed[position], original);
        }
    }
    assert_eq!(
        replayed[5],
        path_text(&private.join("java-classes")).unwrap()
    );
    assert_eq!(
        replayed[9],
        path_text(&private.join("java-headers")).unwrap()
    );
    assert_eq!(
        replayed[15],
        path_text(&private.join("java-generated")).unwrap()
    );
    assert_eq!(
        replayed[18],
        joined_classpath(&[task.classpath[0].clone(), private.join("lease")]).unwrap()
    );
    assert!(selected_tasks_supported(
        &request.task,
        task,
        &[request.task.clone(), task.clone()]
    ));
}

#[test]
fn k2_jdk21_rejects_unproved_strategy_encoding_runtime_and_options() {
    let (_owner, original) = fixture();
    for kind in [
        "in-process",
        "out-of-process",
        "daemon-args",
        "encoding-null",
        "encoding-other",
        "runtime17",
        "gradle17",
        "source17",
        "target17",
        "missing-encoding",
        "option-encoding",
        "adapter",
        "ap",
        "release",
        "charset",
    ] {
        let mut request = request(&original.project_dir);
        selected_k2_jdk21(&mut request);
        let mut recipe = request
            .java_task
            .as_ref()
            .unwrap()
            .java_compiler_recipe()
            .unwrap();
        match kind {
            "in-process" => {
                request
                    .task
                    .kotlin_compiler
                    .as_mut()
                    .unwrap()
                    .execution_strategy =
                    crate::classpath::KotlinCompilerExecutionStrategy::InProcess
            }
            "out-of-process" => {
                request
                    .task
                    .kotlin_compiler
                    .as_mut()
                    .unwrap()
                    .execution_strategy =
                    crate::classpath::KotlinCompilerExecutionStrategy::OutOfProcess
            }
            "daemon-args" => request
                .task
                .kotlin_compiler
                .as_mut()
                .unwrap()
                .normalized_daemon_jvm_arguments
                .push("-Dopaque=true".into()),
            "encoding-null" => {
                request
                    .java_task
                    .as_mut()
                    .unwrap()
                    .options
                    .insert("encoding".into(), Value::Null);
            }
            "encoding-other" => {
                request
                    .java_task
                    .as_mut()
                    .unwrap()
                    .options
                    .insert("encoding".into(), json!("MS932"));
            }
            "runtime17" => recipe.java_runtime_version = "17.0.7+8-LTS-224".into(),
            "gradle17" => recipe.gradle_java_version = "17.0.7".into(),
            "source17" => recipe.api_options[1] = "17".into(),
            "target17" => recipe.api_options[3] = "17".into(),
            "missing-encoding" => {
                recipe.api_options.drain(6..8);
            }
            "option-encoding" => recipe.api_options[7] = "MS932".into(),
            "adapter" => recipe.adapter = "gradle-9.6.2-current-jdk21-utf8-javac-api-v1".into(),
            "ap" => request
                .java_task
                .as_mut()
                .unwrap()
                .annotation_processor_path
                .push(request.project_dir.join("processor.jar")),
            "release" => {
                request
                    .java_task
                    .as_mut()
                    .unwrap()
                    .options
                    .insert("release".into(), json!(21));
            }
            "charset" => recipe.effective_charset = "windows-31j".into(),
            _ => unreachable!(),
        }
        set_recipe(&mut request, recipe);
        assert!(configured_recipe(&request).is_err(), "{kind}");
    }
}

#[test]
fn k2_jdk17_retains_in_process_and_null_encoding_policy() {
    let (_owner, mut request) = fixture();
    selected_k2(&mut request);
    assert!(configured_recipe(&request).is_ok());
    request
        .task
        .kotlin_compiler
        .as_mut()
        .unwrap()
        .execution_strategy = crate::classpath::KotlinCompilerExecutionStrategy::Daemon;
    assert!(configured_recipe(&request).is_err());
    request
        .task
        .kotlin_compiler
        .as_mut()
        .unwrap()
        .execution_strategy = crate::classpath::KotlinCompilerExecutionStrategy::InProcess;
    request
        .java_task
        .as_mut()
        .unwrap()
        .options
        .insert("encoding".into(), json!("UTF-8"));
    assert!(configured_recipe(&request).is_err());
}

#[test]
fn javac_structured_encoding_flag_must_match_selected_null_or_explicit_policy() {
    for profile in 0..5 {
        let sdk21 = matches!(profile, 1 | 4);
        let explicit_utf8 = matches!(profile, 1 | 3);
        let (owner, mut request) = fixture();
        if profile == 4 {
            selected_captured_k2_jdk21_target17(&mut request);
        } else if sdk21 {
            selected_k2_jdk21(&mut request);
        } else if profile >= 2 {
            selected_captured_legacy_profile(&mut request, profile == 2);
        }
        let overlay =
            KotlinEditorOverlay::create_in(&request, &owner.directory.join("overlays")).unwrap();
        let path = overlay
            .sources
            .iter()
            .find(|p| p.extension().unwrap() == "java")
            .unwrap();
        let recipe = configured_recipe(&request).unwrap();
        let receipt = java_receipt(path, JAVA, &recipe);
        let expected = vec![(path.clone(), sha(JAVA.as_bytes()))];
        let packages = BTreeSet::from(["fixture".into()]);
        assert_eq!(
            import_diagnostics(
                &receipt,
                &recipe.api_options,
                &recipe,
                &expected,
                &overlay,
                &packages,
                1,
                false,
            )
            .unwrap()
            .len(),
            3
        );
        for field in [
            "gradleConfiguredEncodingNull",
            "gradleConfiguredEncoding",
            "gradleEffectiveDefaultCharset",
        ] {
            let mut wrong = receipt.clone();
            wrong[field] = match field {
                "gradleConfiguredEncodingNull" => json!(explicit_utf8),
                "gradleConfiguredEncoding" => {
                    if explicit_utf8 {
                        Value::Null
                    } else {
                        json!("UTF-8")
                    }
                }
                _ => json!("windows-31j"),
            };
            assert!(
                import_diagnostics(
                    &wrong,
                    &recipe.api_options,
                    &recipe,
                    &expected,
                    &overlay,
                    &packages,
                    1,
                    false,
                )
                .is_err(),
                "sdk21={sdk21} {field}"
            );
        }
    }
}

#[test]
fn canonical_k2_lease_requires_exact_current_outputs_before_and_after_javac() {
    let (owner, _) = fixture();
    let canonical = owner.directory.join("canonical-fast");
    std::fs::create_dir_all(canonical.join("fixture")).unwrap();
    std::fs::create_dir_all(canonical.join("META-INF")).unwrap();
    std::fs::write(
        canonical.join("fixture/Helper.class"),
        b"synthetic class bytes",
    )
    .unwrap();
    std::fs::write(
        canonical.join("META-INF/main.kotlin_module"),
        b"module bytes",
    )
    .unwrap();
    let before = complete_output_inventory(&canonical).unwrap();
    let lease = owner.directory.join("canonical-lease");
    complete_output_lease(&canonical, &lease, &before).unwrap();
    complete_outputs_match(&canonical, &lease, &before).unwrap();
    for relative in before.keys() {
        assert_eq!(
            std::fs::read(canonical.join(relative)).unwrap(),
            std::fs::read(lease.join(relative)).unwrap()
        );
    }
    // Neither replacing both trees nor changing only Javac's read copy preserves this lease.
    std::fs::write(lease.join("fixture/Helper.class"), b"changed").unwrap();
    assert!(complete_outputs_match(&canonical, &lease, &before).is_err());
    std::fs::write(canonical.join("fixture/Helper.class"), b"changed").unwrap();
    assert!(complete_outputs_match(&canonical, &lease, &before).is_err());
    assert!(
        complete_output_lease(&canonical, &owner.directory.join("stale-lease"), &before).is_err()
    );
    assert!(!owner.directory.join("stale-lease").exists());
    std::fs::remove_file(canonical.join("fixture/Helper.class")).unwrap();
    assert!(
        complete_output_lease(&canonical, &owner.directory.join("missing-lease"), &before).is_err()
    );
    assert!(!owner.directory.join("missing-lease").exists());
    std::fs::write(
        canonical.join("fixture/Helper.class"),
        b"synthetic class bytes",
    )
    .unwrap();
    std::fs::write(canonical.join("fixture/Extra.class"), b"extra").unwrap();
    assert!(
        complete_output_lease(&canonical, &owner.directory.join("extra-lease"), &before).is_err()
    );
    assert!(!owner.directory.join("extra-lease").exists());
    std::fs::remove_dir_all(&canonical).unwrap();
    assert!(
        complete_output_lease(&canonical, &owner.directory.join("removed-lease"), &before).is_err()
    );
}

#[test]
fn closed_canonical_output_uses_actual_javac_imports_without_package_absence_projection() {
    let (owner, mut request) = fixture();
    selected_k2_jdk21(&mut request);
    let overlay =
        KotlinEditorOverlay::create_in(&request, &owner.directory.join("overlays")).unwrap();
    let path = overlay
        .sources
        .iter()
        .find(|p| p.extension().unwrap() == "java")
        .unwrap();
    let recipe = configured_recipe(&request).unwrap();
    let receipt = java_receipt(path, JAVA, &recipe);
    let expected = vec![(path.clone(), sha(JAVA.as_bytes()))];
    let packages = BTreeSet::from(["generated".into()]);
    assert!(import_diagnostics(
        &receipt,
        &recipe.api_options,
        &recipe,
        &expected,
        &overlay,
        &packages,
        1,
        false
    )
    .unwrap()
    .is_empty());
    let actual = import_diagnostics(
        &receipt,
        &recipe.api_options,
        &recipe,
        &expected,
        &overlay,
        &packages,
        1,
        true,
    )
    .unwrap();
    assert_eq!(actual.len(), 3);
    // Full output does not relax byte, runtime, AST range, static-import or error-code checks.
    let mut wrong = receipt.clone();
    wrong["imports"][0]["static"] = json!(true);
    assert_eq!(
        import_diagnostics(
            &wrong,
            &recipe.api_options,
            &recipe,
            &expected,
            &overlay,
            &packages,
            1,
            true
        )
        .unwrap()
        .len(),
        2
    );
    wrong = receipt.clone();
    wrong["diagnostics"][0]["position"] = json!(JAVA.find("generated.KspKotlin").unwrap());
    assert_eq!(
        import_diagnostics(
            &wrong,
            &recipe.api_options,
            &recipe,
            &expected,
            &overlay,
            &packages,
            1,
            true
        )
        .unwrap()
        .len(),
        2
    );
    wrong = receipt.clone();
    wrong["diagnostics"][0]["code"] = json!("compiler.err.cant.resolve.location");
    assert_eq!(
        import_diagnostics(
            &wrong,
            &recipe.api_options,
            &recipe,
            &expected,
            &overlay,
            &packages,
            1,
            true
        )
        .unwrap()
        .len(),
        2
    );
    wrong = receipt.clone();
    wrong["runtime"]["javaVersion"] = json!("17");
    assert!(import_diagnostics(
        &wrong,
        &recipe.api_options,
        &recipe,
        &expected,
        &overlay,
        &packages,
        1,
        true
    )
    .is_err());
    std::fs::write(path, format!("{JAVA}// changed")).unwrap();
    assert!(import_diagnostics(
        &receipt,
        &recipe.api_options,
        &recipe,
        &expected,
        &overlay,
        &packages,
        1,
        true
    )
    .is_err());
}

fn editor_membership_fixture() -> (KotlinEditorOverlay, KotlinDiagnosticRequest) {
    let (owner, mut request) = fixture();
    let roots = request.source_roots.clone();
    for task in [&mut request.task, request.java_task.as_mut().unwrap()] {
        task.options.insert(
            "sdkSourcePolicy".into(),
            json!({
                "adapter": "gradle-source-tree-stock-patterns-v1",
                "task": task.task, "language": task.language,
                "authored_roots": roots, "future_roots": roots, "generated_roots": [],
                "default_excludes": crate::classpath::tests::sdk_source_default_excludes(),
            }),
        );
        task.options.insert(
            "sdkProducerClaims".into(),
            json!({
                "adapter": "gradle-producing-source-trees-v1",
                "task": task.task, "language": task.language, "roots": roots, "complete": true,
            }),
        );
    }
    (owner, request)
}

#[test]
fn new_editor_java_and_kotlin_sources_use_stock_membership_and_distinct_cache() {
    for first_java in [false, true] {
        let (_owner, mut request) = editor_membership_fixture();
        if first_java {
            request
                .sources
                .retain(|path| path.extension().and_then(|s| s.to_str()) != Some("java"));
            request.task.source_files = request.sources.clone();
            request
                .task
                .declared_input_files
                .retain(|path| path.extension().and_then(|s| s.to_str()) != Some("java"));
            request
                .overlays
                .retain(|source| !source.key.ends_with(".java"));
            let task = request.java_task.as_mut().unwrap();
            task.source_files.clear();
            task.declared_input_files
                .retain(|path| path.extension().and_then(|s| s.to_str()) != Some("java"));
        }
        let mut ws = workspace(&request, Vec::new());
        let java = crate::paths::file_key(&request.source_roots[0].join("Editor.java"));
        let kotlin = crate::paths::file_key(&request.source_roots[0].join("Editor.kt"));
        let java_text =
            "package fixture; class Editor { void call(Helper h) { h.editorUnsaved(); } }";
        let kotlin_text = "package fixture\nfun edit(helper: Helper) { helper.editorUnsaved() }";
        ws.open(&java, java_text.into());
        ws.open(&kotlin, kotlin_text.into());
        let first = ws.begin_kotlin_diagnostics(&kotlin).unwrap();
        assert!(
            configured_recipe(&first).is_ok(),
            "stock future policy admits both new wire inputs; first Java={first_java}"
        );
        assert!(!first
            .task
            .source_files
            .iter()
            .any(|path| same_path(path, Path::new(&java))));
        assert!(!first
            .java_task
            .as_ref()
            .unwrap()
            .source_files
            .iter()
            .any(|path| same_path(path, Path::new(&java))));
        let overlay = KotlinEditorOverlay::create(&first).unwrap();
        for (key, text) in [(&java, java_text), (&kotlin, kotlin_text)] {
            let private = overlay
                .sources
                .iter()
                .find(|path| {
                    overlay
                        .paths
                        .get(&kotlin_path_key(&crate::paths::file_key(path)))
                        == Some(key)
                })
                .unwrap();
            assert_eq!(std::fs::read_to_string(private).unwrap(), text);
        }
        let first_key = KotlinEnvironmentKey::new(&first).unwrap();
        ws.close(&kotlin);
        let helper = crate::paths::file_key(&request.project_dir.join("src/Helper.kt"));
        let next = ws.begin_kotlin_diagnostics(&helper).unwrap();
        assert!(configured_recipe(&next).is_ok());
        assert!(
            first_key != KotlinEnvironmentKey::new(&next).unwrap(),
            "new source membership cannot reuse an old Java tools/domain cache"
        );
    }
}

#[test]
fn new_editor_sources_keep_unknown_filtered_generated_and_excluded_policy_unavailable() {
    for kind in [
        "unknown",
        "empty-java-no-policy",
        "filtered",
        "default-excludes",
        "generated",
        "excluded",
        "unmodeled-root",
    ] {
        let (_owner, mut request) = editor_membership_fixture();
        let relative = if kind == "excluded" {
            ".git/Editor.java"
        } else {
            "Editor.java"
        };
        let java = crate::paths::file_key(&request.source_roots[0].join(relative));
        match kind {
            "empty-java-no-policy" => {
                let task = request.java_task.as_mut().unwrap();
                task.source_files.clear();
                task.options.remove("sdkSourcePolicy");
            }
            "unknown" => {
                request
                    .java_task
                    .as_mut()
                    .unwrap()
                    .options
                    .remove("sdkSourcePolicy");
            }
            "filtered" => {
                request
                    .java_task
                    .as_mut()
                    .unwrap()
                    .options
                    .get_mut("sdkSourcePolicy")
                    .unwrap()["future_roots"] = json!([]);
            }
            "default-excludes" => {
                request
                    .java_task
                    .as_mut()
                    .unwrap()
                    .options
                    .get_mut("sdkSourcePolicy")
                    .unwrap()["default_excludes"] = Value::Null;
            }
            "generated" => {
                request
                    .java_task
                    .as_mut()
                    .unwrap()
                    .options
                    .get_mut("sdkSourcePolicy")
                    .unwrap()["generated_roots"] =
                    json!([request.source_roots[0].join("Editor.java")]);
            }
            "unmodeled-root" => {
                request
                    .java_task
                    .as_mut()
                    .unwrap()
                    .options
                    .get_mut("sdkSourcePolicy")
                    .unwrap()["authored_roots"] = json!([request.project_dir.join("foreign")]);
            }
            _ => {}
        }
        let mut ws = workspace(&request, Vec::new());
        ws.open(&java, "package fixture; class Editor {}".into());
        let helper = crate::paths::file_key(&request.project_dir.join("src/Helper.kt"));
        let pending = ws.begin_kotlin_diagnostics(&helper).unwrap();
        assert!(configured_recipe(&pending).is_err(), "{kind}");
    }
}

#[test]
fn same_javac_platform_body_errors_require_current_receiver_attribution() {
    for prefix in ["", "// \u{65e5}\u{672c}\u{8a9e} \u{1f600}\n\t"] {
        let (owner, mut request) = fixture();
        selected_k2_jdk21(&mut request);
        let variable_method = "missing\u{30e1}\u{30bd}\u{30c3}\u{30c9}";
        let body = format!(
            r#"class Body {{ void call(String receiver) {{ receiver.{variable_method}(); new String("x").missingConstructedMethod(); }} }}"#
        );
        let text = format!("{prefix}{JAVA}{body}");
        for source in &mut request.overlays {
            if source.key.ends_with(".java") {
                *source = KotlinSourceOverlay::new(source.key.clone(), text.clone(), 1);
            }
        }
        let overlay =
            KotlinEditorOverlay::create_in(&request, &owner.directory.join("platform-overlay"))
                .unwrap();
        let path = overlay
            .sources
            .iter()
            .find(|p| p.extension().unwrap() == "java")
            .unwrap();
        let recipe = configured_recipe(&request).unwrap();
        let mut receipt = java_receipt(path, &text, &recipe);
        let uri = receipt["sourcesBefore"][0]["uri"].clone();
        receipt["imports"] = json!([]);
        receipt["diagnostics"] = json!([]);
        receipt["platformCalls"] = json!([]);
        for (receiver, method, kind) in [
            ("receiver", variable_method, "variable"),
            (
                r#"new String("x")"#,
                "missingConstructedMethod",
                "construction",
            ),
        ] {
            let expression = format!("{receiver}.{method}()");
            let byte_start = text.find(&expression).unwrap();
            let start = text[..byte_start].encode_utf16().count();
            let receiver_end = start + receiver.encode_utf16().count();
            let end = receiver_end + 1 + method.encode_utf16().count();
            let ordinal = receipt["diagnostics"].as_array().unwrap().len();
            receipt["platformCalls"].as_array_mut().unwrap().push(json!({
            "sourceUri": uri, "diagnosticOrdinal": ordinal, "receiverText": receiver, "receiverKind": kind,
            "receiverBinaryName": "java.lang.String", "originUri": "jrt:/java.base/java/lang/String.class",
            "method": method, "receiverStart": start, "receiverEnd": receiver_end,
            "selectStart": start, "selectEnd": end, "callStart": start, "callEnd": end + 2,
        }));
            receipt["diagnostics"].as_array_mut().unwrap().push(json!({
            "ordinal": ordinal, "kind": "ERROR", "code": "compiler.err.cant.resolve.location.args", "sourceUri": uri,
            "start": start, "end": end, "position": receiver_end,
            "message": format!("Missing method {method}"),
        }));
        }
        let expected = vec![(path.clone(), sha(text.as_bytes()))];
        let decode = |receipt: &Value, complete| {
            import_diagnostics(
                receipt,
                &recipe.api_options,
                &recipe,
                &expected,
                &overlay,
                &BTreeSet::new(),
                1,
                complete,
            )
        };
        let actual = decode(&receipt, true).unwrap();
        assert_eq!(actual.len(), 2);
        assert!(actual.iter().all(|error| error.path
            == crate::paths::file_key(&request.project_dir.join("src/Main.java"))));
        for (error, method) in actual
            .iter()
            .zip([variable_method, "missingConstructedMethod"])
        {
            let start =
                crate::text::LineIndex::new(&text).offset(&text, error.line - 1, error.col - 1);
            assert_eq!(&text[start..start + method.len()], method);
        }
        assert!(
            decode(&receipt, false).unwrap().is_empty(),
            "K1 partial headers cannot gain body authority"
        );
        for (field, value) in [
            ("diagnosticOrdinal", json!(999)),
            ("receiverKind", json!("static")),
            ("originUri", json!("file:/unwatched/String.class")),
            (
                "originUri",
                json!("jrt:/other.module/java/lang/String.class"),
            ),
            ("receiverBinaryName", json!("other.String")),
            ("receiverText", json!("other")),
            ("method", json!("otherMethod")),
            ("sourceUri", json!("other-source")),
            ("callEnd", json!(0)),
            ("receiverStart", json!(0)),
        ] {
            let mut wrong = receipt.clone();
            for row in wrong["platformCalls"].as_array_mut().unwrap() {
                row[field] = value.clone();
            }
            assert!(decode(&wrong, true).unwrap().is_empty(), "{field}={value}");
        }
        for field in ["start", "end", "position"] {
            let mut wrong = receipt.clone();
            for row in wrong["diagnostics"].as_array_mut().unwrap() {
                row[field] = json!(0);
            }
            assert!(decode(&wrong, true).unwrap().is_empty(), "{field}");
        }
        let mut wrong = receipt.clone();
        wrong["platformCalls"] = json!([]);
        assert!(
            decode(&wrong, true).unwrap().is_empty(),
            "raw diagnostic code alone is insufficient"
        );
        wrong = receipt.clone();
        for row in wrong["diagnostics"].as_array_mut().unwrap() {
            row["code"] = json!("compiler.err.cant.resolve.location");
        }
        assert!(decode(&wrong, true).unwrap().is_empty());
        // Coherent malformed raw ranges must not overflow before boundary validation.
        let mut malformed = receipt.clone();
        for i in 0..malformed["platformCalls"].as_array().unwrap().len() {
            let start = usize::MAX
                - malformed["platformCalls"][i]["receiverText"]
                    .as_str()
                    .unwrap()
                    .encode_utf16()
                    .count();
            for field in ["callStart", "receiverStart", "selectStart"] {
                malformed["platformCalls"][i][field] = json!(start);
            }
            for field in ["receiverEnd", "selectEnd", "callEnd"] {
                malformed["platformCalls"][i][field] = json!(usize::MAX);
            }
            malformed["diagnostics"][i]["start"] = json!(start);
            malformed["diagnostics"][i]["end"] = json!(usize::MAX);
            malformed["diagnostics"][i]["position"] = json!(usize::MAX);
        }
        assert!(decode(&malformed, true).unwrap().is_empty());
        std::fs::write(path, format!("{text}//changed")).unwrap();
        assert!(
            decode(&receipt, true).is_err(),
            "current private bytes are required"
        );
    }
}

#[test]
fn k2_platform_origin_rejects_release_and_patched_system_modules() {
    for flags in [
        vec!["--release", "17"],
        vec!["--patch-module", "java.base=unwatched"],
        vec!["--system", "unwatched-sdk"],
        vec!["--upgrade-module-path", "unwatched"],
    ] {
        let (_owner, mut request) = fixture();
        selected_k2_jdk21(&mut request);
        assert!(configured_recipe(&request).is_ok());
        let mut recipe = request
            .java_task
            .as_ref()
            .unwrap()
            .java_compiler_recipe()
            .unwrap();
        recipe
            .api_options
            .extend(flags.iter().map(|flag| (*flag).into()));
        set_recipe(&mut request, recipe);
        assert!(configured_recipe(&request).is_err(), "{flags:?}");
    }
    let (_owner, mut request) = fixture();
    selected_k2_jdk21(&mut request);
    request
        .java_task
        .as_mut()
        .unwrap()
        .options
        .insert("release".into(), json!(17));
    assert!(configured_recipe(&request).is_err());
}

// Native, baseline-compilable regression: the current helper compiles/runs but has no
// methodCalls attribution facts. Explicitly run with the captured SDK17, never auto-skip.
#[test]
fn three_kotlin_producers_bind_both_languages_and_track_unsaved_inputs() {
    for stock75_producers in [false, true] {
    let (owner, _) = fixture();
    let mut requests = Vec::new();
    let mut modules = Vec::new();
    // Deliberately not dependency order: the planner must place base before both readers.
    for name in ["mock", "base", "tls", "consumer"] {
        let root = owner.directory.join(name);
        let mut selected = request(&root);
        if stock75_producers { selected_captured_legacy_profile(&mut selected, false); }
        selected.scope.module = format!(":{name}");
        selected.task.task = format!(":{name}:compileKotlin");
        selected.task.classpath.clear();
        let java = selected.java_task.as_mut().unwrap();
        java.task = format!(":{name}:compileJava");
        java.classpath = selected.task.output_roots.clone();
        if name != "consumer" {
            selected.task.source_files.retain(|path| path.extension().and_then(|e| e.to_str()) == Some("kt"));
            java.source_files.clear();
            if stock75_producers { captured_gradle75_zero_java_options(java); }
            selected.sources = selected.task.source_files.clone();
            selected.overlays.retain(|source| source.key.ends_with(".kt"));
        }
        for task in [&mut selected.task, java] {
            task.options.insert("sdkProducerClaims".into(), json!({
                "adapter":"gradle-producing-source-trees-v1", "complete":true,
                "language":if task.language == crate::classpath::CompilerLanguage::Java { "java" } else { "kotlin" },
                "task":task.task,"roots":[root.join("src")],
            }));
        }
        std::fs::create_dir_all(root.join("src")).unwrap();
        for source in &selected.overlays { std::fs::write(&source.key, &source.text).unwrap(); }
        requests.push(selected);
    }
    let slots = |selected: &KotlinDiagnosticRequest| {
        selected.task.output_roots.iter().chain(&selected.java_task.as_ref().unwrap().output_roots)
            .cloned().collect::<Vec<_>>()
    };
    let base_slots = slots(&requests[1]);
    let all_slots: Vec<_> = requests[..3].iter().flat_map(&slots).collect();
    for slot in &all_slots { std::fs::create_dir_all(slot).unwrap(); }
    for (i, selected) in requests.iter_mut().enumerate() {
        let dependencies = match i { 0 | 2 => base_slots.clone(), 3 => all_slots.clone(), _ => Vec::new() };
        selected.task.classpath = dependencies.clone();
        let java = selected.java_task.as_mut().unwrap();
        java.classpath = dependencies;
        java.classpath.extend(selected.task.output_roots.iter().cloned());
        let mut recipe = java.java_compiler_recipe().unwrap();
        let cp = recipe.api_options.iter().position(|arg| arg == "-classpath").unwrap() + 1;
        recipe.api_options[cp] = joined_classpath(&java.classpath).unwrap();
        java.options.insert("javaCompilerRecipe".into(), serde_json::to_value(recipe).unwrap());
        if !selected.task.classpath.is_empty() {
            selected.task.arguments.extend(["-classpath".into(), joined_classpath(&selected.task.classpath).unwrap()]);
        }
        let compiler_tasks = vec![selected.task.clone(), java.clone()];
        let selected_slots = slots(selected);
        modules.push(ModuleClasspath {
            module:selected.scope.module.clone(), project_dir:selected.project_dir.clone(), build:"producer-test".into(),
            entries:Vec::new(), unresolved:Vec::new(), source_sets:vec![SourceSetClasspath {
                name:"main".into(), source_roots:selected.source_roots.clone(),
                output_roots:selected_slots.clone(), provided_artifacts:selected_slots,
                kotlin_compile_task:Some(selected.task.task.clone()),
                compiler_tasks, ..Default::default()
            }],
        });
    }
    let model = ProjectSnapshot::new(modules.clone());
    let consumer_source = &model.modules[3].source_sets[0];
    let mut consumer = requests[3].clone();
    consumer.kotlin_producers = crate::compile::kotlin_producers::capture(
        &model, consumer_source, &consumer.task, consumer.java_task.as_ref().unwrap()).unwrap();
    assert_eq!(consumer.kotlin_producers.len(), 3);
    assert_eq!(consumer.kotlin_producers[0].scope.module, "gradle:producer-test|:base");
    consumer.dependency_overlays = requests[..3].iter().flat_map(|r| r.overlays.clone()).collect();
    let identity = crate::compile::kotlin_producers::identity(&consumer).unwrap();
    consumer.dependency_overlays[0].text.push_str("\n// unsaved API edit");
    let source = &mut consumer.dependency_overlays[0];
    *source = KotlinSourceOverlay::new(source.key.clone(), source.text.clone(), 2);
    assert_ne!(crate::compile::kotlin_producers::identity(&consumer).unwrap(), identity);
    let mut bindings = BTreeMap::new();
    for (i, slot) in all_slots.iter().enumerate() {
        let output = owner.directory.join(format!("private-output-{i}"));
        std::fs::create_dir(&output).unwrap();
        bindings.insert(kotlin_path_key(&crate::paths::file_key(slot)), output);
    }
    let covered = consumer.dependency_overlays.iter().map(|s| s.key.clone()).collect();
    let effective = crate::compile::kotlin_producers::effective(&consumer, &bindings, &covered).unwrap();
    for slot in &all_slots {
        assert!(!effective.task.classpath.contains(slot));
        assert!(!effective.java_task.as_ref().unwrap().classpath.contains(slot));
    }
    assert_eq!(effective.task.classpath.len(), 6);
    let kotlin_cp = effective.task.arguments.iter().position(|arg| arg == "-classpath").unwrap() + 1;
    assert_eq!(effective.task.arguments[kotlin_cp], joined_classpath(&effective.task.classpath).unwrap());
    assert!(effective.dependency_overlays.is_empty());
    let java = effective.java_task.as_ref().unwrap();
    let recipe = java.java_compiler_recipe().unwrap();
    let cp = java.java_api_option_slots(&recipe).unwrap().classpath;
    assert_eq!(recipe.api_options[cp], joined_classpath(&java.classpath).unwrap());
    // The primary compiler must not bootstrap its current ABI from its own saved output.
    for slot in slots(&requests[1]) {
        let mut self_read = modules.clone();
        self_read[1].source_sets[0].compiler_tasks[0].classpath.push(slot);
        let self_read = ProjectSnapshot::new(self_read);
        assert!(crate::compile::kotlin_producers::capture(&self_read, &self_read.modules[3].source_sets[0],
            &consumer.task, consumer.java_task.as_ref().unwrap()).is_none());
    }
    // A modeled cycle or an absent complete-zero claim must never admit a clean closure.
    modules[1].source_sets[0].compiler_tasks[0].classpath = slots(&requests[0]);
    let cyclic = ProjectSnapshot::new(modules.clone());
    assert!(crate::compile::kotlin_producers::capture(&cyclic, &cyclic.modules[3].source_sets[0],
        &consumer.task, consumer.java_task.as_ref().unwrap()).is_none());
    modules[1].source_sets[0].compiler_tasks[0].classpath.clear();
    modules[1].source_sets[0].compiler_tasks[1].options.remove("sdkProducerClaims");
    let unknown = ProjectSnapshot::new(modules);
    assert!(crate::compile::kotlin_producers::capture(&unknown, &unknown.modules[3].source_sets[0],
        &consumer.task, consumer.java_task.as_ref().unwrap()).is_none());
    }
}

#[test]
#[ignore = "requires KTLSP_PROCESSOR_TEST_JAVA_HOME: SDK17 ordinary Javac built-generated API control"]
fn built_generated_method_capture_paired_current_api_receiver_and_ranges() {
    assert!(
        cfg!(windows),
        "native ownership qualification is Windows-only"
    );
    let sdk =
        PathBuf::from(std::env::var_os("KTLSP_PROCESSOR_TEST_JAVA_HOME").expect("SDK17 required"));
    let (owner, request) = fixture();
    let root = owner.directory.join("method-capture");
    std::fs::create_dir(&root).unwrap();
    let tools = root.join("tools");
    std::fs::create_dir(&tools).unwrap();
    let helper = root.join("JavacDiagnostics.java");
    std::fs::write(&helper, JAVA_SOURCE).unwrap();
    let owned = |command: &mut Command, expected: i32| {
        #[cfg(windows)]
        {
            for name in [
                "JAVA_TOOL_OPTIONS",
                "_JAVA_OPTIONS",
                "JDK_JAVA_OPTIONS",
                "JAVA_OPTS",
            ] {
                command.env_remove(name);
            }
            let (status, interrupted, out, out_complete, err, err_complete) =
                OwnedNativeCompile::spawn(command, MAX_OUTPUT_BYTES)
                    .unwrap()
                    .wait(COMPILE_TIMEOUT, Some(request.invalid.as_ref()));
            assert!(!interrupted && out_complete && err_complete, "{out}\n{err}");
            assert_eq!(status.code(), Some(expected), "{out}\n{err}");
        }
        #[cfg(not(windows))]
        {
            let _ = (command, expected);
            unreachable!();
        }
    };
    owned(
        Command::new(sdk.join("bin/javac.exe"))
            .args(["-encoding", "UTF-8", "-d"])
            .arg(&tools)
            .arg(&helper),
        0,
    );
    for (api_name, receiver, current, absent) in [
        ("Api", "api", "versionOne", "versionTwo"),
        ("Api", "api", "versionTwo", "versionOne"),
        ("生成API", "対象", "現在", "削除済"),
    ] {
        let phase = root.join(current);
        std::fs::create_dir(&phase).unwrap();
        let api = phase.join(format!("{api_name}.java"));
        std::fs::write(
            &api,
            format!(
                "package generated; public class {api_name} {{ public int {current}() {{ return 1; }} }}"
            ),
        )
        .unwrap();
        let classes = phase.join("lease");
        std::fs::create_dir(&classes).unwrap();
        owned(
            Command::new(sdk.join("bin/javac.exe"))
                .args(["-proc:none", "-encoding", "UTF-8", "-d"])
                .arg(&classes)
                .arg(&api),
            0,
        );
        let consumer = phase.join("Consumer.java");
        let text = format!("// 前置😀\nclass Consumer {{ void use(generated.{api_name} {receiver}) {{ {receiver}.{absent}(); {receiver}.{current}(); }} }}\n");
        std::fs::write(&consumer, &text).unwrap();
        let options = vec![
            "-proc:none".to_owned(),
            "-sourcepath".into(),
            "".into(),
            "-classpath".into(),
            path_text(&classes).unwrap(),
            "-d".into(),
            path_text(&phase.join("out")).unwrap(),
        ];
        let mut spec = BTreeMap::from([
            ("stage".into(), "built-generated-method-capture-regression".into()),
            ("expected.javaHome".into(), path_text(&sdk).unwrap()),
            ("expected.javaVersion".into(), "17.0.7".into()),
            (
                "expected.javaRuntimeVersion".into(),
                "17.0.7+8-LTS-224".into(),
            ),
            ("gradle.configuredEncodingNull".into(), "true".into()),
            ("effective.charset".into(), "UTF-8".into()),
            ("effective.locale".into(), "ja-JP".into()),
            ("gradle.fileEncoding".into(), "UTF-8".into()),
            ("option.count".into(), options.len().to_string()),
            ("source.count".into(), "1".into()),
            ("source.0.path".into(), path_text(&consumer).unwrap()),
            ("source.0.sha256".into(), sha(text.as_bytes())),
        ]);
        for (i, option) in options.iter().enumerate() {
            spec.insert(format!("option.{i}"), option.clone());
        }
        let properties_file = phase.join("java.properties");
        std::fs::write(&properties_file, properties(&spec)).unwrap();
        let output = phase.join("java.json");
        owned(
            Command::new(sdk.join("bin/java.exe"))
                .arg("-cp")
                .arg(&tools)
                .arg("JavacDiagnostics")
                .arg(&properties_file)
                .arg(&output),
            1,
        );
        let value = read_json(&output).unwrap();
        require_bool(&value, "compileCompleted", true).unwrap();
        let errors: Vec<_> = array(&value, "diagnostics")
            .unwrap()
            .iter()
            .filter(|d| d["kind"] == "ERROR")
            .collect();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["code"], "compiler.err.cant.resolve.location.args");
        let calls = array(&value, "methodCalls").expect("same-task attribution is required");
        assert_eq!(calls.len(), 2);
        for call in calls {
            assert_eq!(call["receiverType"], format!("generated.{api_name}"));
            assert_eq!(call["receiver"], receiver);
            assert_eq!(call["simpleOwnedApi"], true);
            assert_eq!(call["originKind"], "class");
            assert!(same_path(
                Path::new(value_string(call, "originPath").unwrap()),
                &classes.join(format!("generated/{api_name}.class"))
            ));
            assert_eq!(
                value_string(call, "originSha256").unwrap(),
                sha(&std::fs::read(classes.join(format!("generated/{api_name}.class"))).unwrap())
            );
            assert_eq!(call["publicInstanceZeroArgumentMethods"], json!([current]));
        }
        let missing = calls.iter().find(|c| c["method"] == absent).unwrap();
        let start = missing["selectStart"].as_u64().unwrap() as usize;
        let end = missing["selectEnd"].as_u64().unwrap() as usize;
        assert_eq!(
            &text[utf16_byte_offset(&text, start).unwrap()..utf16_byte_offset(&text, end).unwrap()],
            format!("{receiver}.{absent}")
        );
        assert_eq!(errors[0]["start"], missing["selectStart"]);
        assert_eq!(errors[0]["position"], missing["receiverEnd"]);
        let bytes = std::fs::read(classes.join(format!("generated/{api_name}.class"))).unwrap();
        let class = ktcore::classfile::decode("current-method-capture", &bytes).unwrap();
        assert_eq!(
            class.identity.internal_name.as_ref(),
            format!("generated/{api_name}")
        );
        assert!(class
            .methods
            .iter()
            .any(|method| method.name.as_ref() == current));
        assert!(!class
            .methods
            .iter()
            .any(|method| method.name.as_ref() == absent));
    }
}
