//! Positive Java import errors on the existing selected Kotlin diagnostic worker.
//! shortcut: only verified Kotlin 1.8.21/Gradle 7.4.2/JDK 17 plain tasks and explicit imports;
//! expand after independent compiler, namespace and editor controls for the new case pass.
use super::*;
use crate::classpath::{CompilerTask, JavaCompilerRecipe};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const HEADER_SOURCE: &str = include_str!("ToolOwnedHeaders.kt");
const DOMAIN_SOURCE: &str = include_str!("DomainProof.kt");
const K2_HEADER_SOURCE: &str = include_str!("K2CompleteHeaders.kt");
const JAVA_SOURCE: &str = include_str!("JavacDiagnostics.java");

pub(super) fn assets_sha256() -> String {
    let mut hash = Sha256::new();
    for source in [HEADER_SOURCE, DOMAIN_SOURCE, K2_HEADER_SOURCE, JAVA_SOURCE] {
        hash.update((source.len() as u64).to_be_bytes());
        hash.update(source.as_bytes());
    }
    format!("{:x}", hash.finalize())
}

pub(super) fn append_positives(
    canonical: &mut Vec<CompileDiagnostic>,
    java: Vec<CompileDiagnostic>,
) {
    // Kotlin's mapped stream can mention Java files too. Preserve its existing .kt publish boundary;
    // only the independently checked Javac selector may add Java diagnostics to this outcome.
    canonical.retain(|diagnostic| diagnostic.path.ends_with(".kt"));
    canonical.extend(java);
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn value_string<'a>(value: &'a Value, name: &str) -> anyhow::Result<&'a str> {
    value
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("java-receipt-string-{name}"))
}

fn array<'a>(value: &'a Value, name: &str) -> anyhow::Result<&'a Vec<Value>> {
    value
        .get(name)
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("java-receipt-array-{name}"))
}

fn require_bool(value: &Value, name: &str, expected: bool) -> anyhow::Result<()> {
    anyhow::ensure!(
        value.get(name).and_then(Value::as_bool) == Some(expected),
        "java-receipt-flag-{name}"
    );
    Ok(())
}

fn header_key(path: &Path) -> String {
    // The pinned compiler component normalizes its own canonical source paths this way.
    crate::paths::file_key(path)
        .replace('\\', "/")
        .to_lowercase()
}

fn same_path(a: &Path, b: &Path) -> bool {
    kotlin_path_key(&crate::paths::file_key(a)) == kotlin_path_key(&crate::paths::file_key(b))
}

fn path_text(path: &Path) -> anyhow::Result<String> {
    let text = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("java-path-encoding"))?;
    anyhow::ensure!(
        path.is_absolute() && !text.chars().any(|c| matches!(c, '\0' | '\r' | '\n' | '\t')),
        "java-path-unhandled"
    );
    Ok(text.to_owned())
}

fn joined_classpath(paths: &[PathBuf]) -> anyhow::Result<String> {
    for path in paths {
        let text = path_text(path)?;
        anyhow::ensure!(
            !text.contains(if cfg!(windows) { ';' } else { ':' }),
            "java-classpath-separator-in-path"
        );
    }
    std::env::join_paths(paths)?
        .into_string()
        .map_err(|_| anyhow::anyhow!("java-classpath-encoding"))
}

fn k2_normal(request: &KotlinDiagnosticRequest) -> bool {
    request
        .task
        .kotlin_compiler
        .as_ref()
        .is_some_and(|runtime| {
            runtime.compiler_version == "2.4.0" && runtime.gradle_plugin_version == "2.4.0"
        })
}

fn header_assets(request: &KotlinDiagnosticRequest) -> Vec<(&'static str, &'static str)> {
    if k2_normal(request) {
        vec![
            ("K2CompleteHeaders.kt", K2_HEADER_SOURCE),
            ("JavacDiagnostics.java", JAVA_SOURCE),
        ]
    } else {
        vec![
            ("ToolOwnedHeaders.kt", HEADER_SOURCE),
            ("DomainProof.kt", DOMAIN_SOURCE),
            ("JavacDiagnostics.java", JAVA_SOURCE),
        ]
    }
}

fn stock_task_action(task: &CompilerTask, declaring_class: &str, method_name: &str) -> bool {
    let Some(fact) = task.options.get("taskActionInventory") else {
        return false;
    };
    let action = match fact.get("adapter").and_then(Value::as_str) {
        Some("gradle-7.4.2-stock-actions-v1") => {
            "org.gradle.api.internal.project.taskfactory.IncrementalInputsTaskAction"
        }
        Some("gradle-9.6.1-stock-actions-v1") => {
            "org.gradle.api.internal.project.taskfactory.IncrementalTaskAction"
        }
        _ => return false,
    };
    fact.get("actions")
        .and_then(Value::as_array)
        .is_some_and(|actions| {
            actions.len() == 1
                && actions[0].get("class").and_then(Value::as_str) == Some(action)
                && actions[0].get("declaringClass").and_then(Value::as_str) == Some(declaring_class)
                && actions[0].get("method").and_then(Value::as_str) == Some(method_name)
                && actions[0].get("parameterTypes")
                    == Some(&serde_json::json!(["org.gradle.work.InputChanges"]))
        })
}

fn stock_only_if(task: &CompilerTask, kapt: bool) -> bool {
    let Some(fact) = task.options.get("taskOnlyIfInventory") else {
        return false;
    };
    match fact.get("adapter").and_then(Value::as_str) {
        Some("gradle-7.4.2-stock-onlyif-v1") => {}
        Some("gradle-9.6.1-stock-onlyif-v1") if !kapt => {}
        _ => return false,
    }
    let Some(predicates) = fact.get("predicates").and_then(Value::as_array) else {
        return false;
    };
    predicates.len() == (if kapt { 2 } else { 1 })
        && predicates[0]
            .get("stockEnabledPredicate")
            .and_then(Value::as_bool)
            == Some(true)
        && predicates[0]
            .get("stockKaptPredicate")
            .and_then(Value::as_bool)
            == Some(false)
        && (!kapt
            || (predicates[1]
                .get("stockEnabledPredicate")
                .and_then(Value::as_bool)
                == Some(false)
                && predicates[1]
                    .get("stockKaptPredicate")
                    .and_then(Value::as_bool)
                    == Some(true)))
}

/// Only source sets which can feed this compiler read set participate in absence checks.
/// A materialized directory is never proof that a dependent source API is current.
pub(super) fn import_dependencies(
    primary: &CompilerTask,
    java: &CompilerTask,
    owner: &crate::classpath::SourceSetClasspath,
    model: &crate::classpath::ProjectSnapshot,
    visibilities: &[crate::project_model::ScopeVisibility],
    visible_origins: &BTreeSet<String>,
) -> Option<Vec<crate::classpath::SourceSetClasspath>> {
    if !selected_tasks_supported(primary, java, &owner.compiler_tasks) {
        return None;
    }
    let sets: Vec<_> = model
        .modules
        .iter()
        .flat_map(|module| &module.source_sets)
        .collect();
    if sets.len() != visibilities.len() {
        return None;
    }
    // The supported complete Java recipe fixes -sourcepath="". Kotlin analysis also reads
    // its own compile CP, so a producer visible there cannot silently evade the boundary.
    let reads: Vec<_> = primary
        .classpath
        .iter()
        .chain(java.classpath.iter())
        .map(|p| header_key(p))
        .collect();
    let mut dependencies = Vec::new();
    for (source, visibility) in sets.into_iter().zip(visibilities) {
        if std::ptr::eq(source, owner) {
            continue;
        }
        let outputs: Vec<_> = source
            .output_roots
            .iter()
            .chain(&source.provided_artifacts)
            .chain(
                source
                    .compiler_tasks
                    .iter()
                    .flat_map(|task| task.output_roots.iter().chain(&task.declared_output_roots)),
            )
            .collect();
        let intersecting: Vec<_> = reads
            .iter()
            .filter(|read| {
                outputs
                    .iter()
                    .any(|output| paths_overlap(read, &header_key(output)))
            })
            .collect();
        let visible = visibility
            .ordered_origins
            .first()
            .is_some_and(|origin| visible_origins.contains(origin));
        if !visible && intersecting.is_empty() {
            continue;
        }
        if source.compiler_metadata_incomplete
            || source.classpath_incomplete
            || !source.unresolved.is_empty()
            || source.kotlin_compile_task.is_some()
            || source.compiler_tasks.len() != 1
            || source.source_roots.is_empty()
            || intersecting.is_empty()
        {
            return None;
        }
        let task = &source.compiler_tasks[0];
        let Some(first) = task.source_files.first() else {
            return None;
        };
        let candidate = model.plain_java_task(&crate::paths::file_key(first))?;
        let recipe = task.java_compiler_recipe()?;
        if !std::ptr::eq(task, candidate)
            || !task.annotation_processor_path.is_empty()
            || task.kotlin_compiler.is_some()
            || !stock_task_action(task, "org.gradle.api.tasks.compile.JavaCompile", "compile")
            || !stock_only_if(task, false)
            || task
                .options
                .get("taskActionInventory")
                .and_then(|fact| fact.get("adapter"))
                != java
                    .options
                    .get("taskActionInventory")
                    .and_then(|fact| fact.get("adapter"))
            || task
                .options
                .get("taskOnlyIfInventory")
                .and_then(|fact| fact.get("adapter"))
                != java
                    .options
                    .get("taskOnlyIfInventory")
                    .and_then(|fact| fact.get("adapter"))
            || task
                .java_home
                .as_ref()
                .zip(java.java_home.as_ref())
                .is_none_or(|(a, b)| !same_path(a, b))
            || task.options.get("bootstrapClasspath") != Some(&Value::Null)
            || task.options.get("release") != Some(&Value::Null)
            || if recipe.adapter == "gradle-9.6.1-current-jdk21-utf8-javac-api-v1" {
                task.options.get("encoding").and_then(Value::as_str) != Some("UTF-8")
            } else {
                task.options.get("encoding") != Some(&Value::Null)
            }
            || task.output_roots.len() != 1
            || intersecting
                .iter()
                .any(|read| *read != &header_key(&task.output_roots[0]))
        {
            // Jar/resources/other producer outputs need their own naming proof. Initial support
            // covers only the exact plain Java classes directory; unrelated source sets are ignored.
            return None;
        }
        if recipe.effective_charset != "UTF-8"
            || !recipe.api_options.iter().any(|arg| arg == "-proc:none")
            || recipe
                .api_options
                .windows(2)
                .filter(|pair| pair[0] == "-sourcepath")
                .count()
                != 1
            || !recipe
                .api_options
                .windows(2)
                .any(|pair| pair[0] == "-sourcepath" && pair[1].is_empty())
        {
            return None;
        }
        dependencies.push(source.clone());
    }
    Some(dependencies)
}

fn dependency_inputs(request: &KotlinDiagnosticRequest) -> anyhow::Result<BTreeSet<PathBuf>> {
    let mut roots = BTreeSet::new();
    for source in &request.java_dependencies {
        anyhow::ensure!(
            source.compiler_tasks.len() == 1,
            "java-dependency-task-count-unknown"
        );
        roots.extend(source.source_roots.iter().cloned());
        for file in &source.compiler_tasks[0].source_files {
            roots.insert(file.clone());
            // Explicit source files outside the exported roots are also parsed and watched.
            // Watching their parent overapproximates later arrivals instead of dropping them.
            if !source
                .source_roots
                .iter()
                .any(|root| path_under(&header_key(file), &header_key(root)))
            {
                roots.insert(
                    file.parent()
                        .ok_or_else(|| anyhow::anyhow!("java-dependency-source-parent-unknown"))?
                        .to_path_buf(),
                );
            }
        }
    }
    for root in &roots {
        path_text(root)?;
    }
    Ok(roots)
}

/// Plain javac without AP/plugins/custom actions cannot emit a class outside its source package.
/// Parse an overapproximation of current sources, including files saved after model capture.
fn java_source_package(parser: &mut crate::java::JavaParser, text: &str) -> anyhow::Result<String> {
    anyhow::ensure!(
        text.is_ascii() && !text.contains("\\u"),
        "java-dependency-encoding-or-unicode-escape-unhandled"
    );
    let tree = parser.parse(text);
    let root = tree.root_node();
    anyhow::ensure!(!root.has_error(), "java-dependency-syntax-unknown");
    let mut cursor = root.walk();
    let children: Vec<_> = root.named_children(&mut cursor).collect();
    anyhow::ensure!(
        !children
            .iter()
            .any(|node| node.kind() == "module_declaration"),
        "java-dependency-module-unhandled"
    );
    let declarations: Vec<_> = children
        .iter()
        .filter(|node| node.kind() == "package_declaration")
        .collect();
    anyhow::ensure!(declarations.len() <= 1, "java-dependency-package-ambiguous");
    let package = crate::java::package_of(&tree, text);
    anyhow::ensure!(
        !package.is_empty(),
        "java-dependency-unnamed-package-unhandled"
    );
    if let Some(declaration) = declarations.first() {
        let mut cursor = declaration.walk();
        anyhow::ensure!(
            declaration
                .named_children(&mut cursor)
                .all(|node| matches!(node.kind(), "identifier" | "scoped_identifier")),
            "java-dependency-package-decoration-unhandled"
        );
        anyhow::ensure!(
            !package.is_empty()
                && package.split('.').all(|part| !part.is_empty()
                    && part
                        .chars()
                        .enumerate()
                        .all(|(i, ch)| ch.is_ascii_alphabetic()
                            || ch == '_'
                            || ch == '$'
                            || (i > 0 && ch.is_ascii_digit()))),
            "java-dependency-package-name-unhandled"
        );
    }
    Ok(package)
}

fn dependency_domains(
    request: &KotlinDiagnosticRequest,
) -> anyhow::Result<(BTreeSet<String>, Value)> {
    let mut files = BTreeSet::new();
    let mut directories = BTreeSet::new();
    for input in dependency_inputs(request)? {
        let metadata = match std::fs::symlink_metadata(&input) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // Missing roots remain watched for arrival. An exact selected source may not disappear.
                anyhow::ensure!(
                    !request
                        .java_dependencies
                        .iter()
                        .flat_map(|source| &source.compiler_tasks[0].source_files)
                        .any(|source| same_path(source, &input)),
                    "java-dependency-selected-source-missing"
                );
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "java-dependency-source-symlink"
        );
        if metadata.is_file() {
            files.insert(input);
            continue;
        }
        anyhow::ensure!(metadata.is_dir(), "java-dependency-source-role-unknown");
        for (count, entry) in walkdir::WalkDir::new(&input)
            .follow_links(false)
            .into_iter()
            .enumerate()
        {
            anyhow::ensure!(
                count < MAX_RECEIPT_INVENTORY_ENTRIES,
                "java-dependency-source-inventory-cap"
            );
            let entry = entry?;
            anyhow::ensure!(
                !entry.file_type().is_symlink(),
                "java-dependency-source-symlink"
            );
            if entry.file_type().is_dir() {
                directories.insert(entry.path().to_path_buf());
            }
            if entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("java"))
            {
                files.insert(entry.path().to_path_buf());
            }
        }
    }
    anyhow::ensure!(
        files.len() <= MAX_RECEIPT_INVENTORY_ENTRIES,
        "java-dependency-source-count-cap"
    );
    let mut parser = crate::java::JavaParser::new();
    let mut packages = BTreeSet::new();
    let mut rows = Vec::new();
    for file in files {
        anyhow::ensure!(
            file.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("java"))
                && std::fs::metadata(&file)?.len() <= MAX_OUTPUT_BYTES as u64,
            "java-dependency-source-role-or-size-unhandled"
        );
        let text = std::fs::read_to_string(&file)?;
        let package = java_source_package(&mut parser, &text)?;
        packages.insert(package.clone());
        rows.push(
            serde_json::json!({"path":file, "sha256":sha(text.as_bytes()), "package":package,
            "nameDomainIsWholePackage":true, "apiAuthority":false}),
        );
    }
    Ok((
        packages,
        serde_json::json!({"sourceDomains":rows, "directories":directories, "producerClosure":false, "apiAuthority":false}),
    ))
}
// Source membership can change without a build-file change or a new Gradle task snapshot.
// A closed newly saved file must not be omitted from a negative import proof.
fn owned_membership_inputs(request: &KotlinDiagnosticRequest) -> anyhow::Result<Vec<PathBuf>> {
    anyhow::ensure!(
        !request.source_roots.is_empty(),
        "java-owned-source-roots-unknown"
    );
    for root in &request.source_roots {
        path_text(root)?;
        anyhow::ensure!(
            !path_under(&header_key(&request.project_dir), &header_key(root)),
            "java-owned-source-root-unbounded"
        );
        anyhow::ensure!(
            !request
                .task
                .output_roots
                .iter()
                .chain(request.java_task.iter().flat_map(|task| &task.output_roots))
                .any(|output| paths_overlap(&header_key(root), &header_key(output))),
            "java-owned-source-root-overlaps-classes"
        );
    }
    Ok(request.source_roots.clone())
}

fn owned_source_membership(
    request: &KotlinDiagnosticRequest,
    read_plan: &crate::compiled_watch::WatchPlan,
) -> anyhow::Result<Value> {
    let mut disk = BTreeSet::new();
    let mut directories = BTreeSet::new();
    let mut visited = 0;
    for root in owned_membership_inputs(request)? {
        let metadata = match std::fs::symlink_metadata(&root) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        anyhow::ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "java-owned-source-root-role-unknown"
        );
        for entry in walkdir::WalkDir::new(root).follow_links(false) {
            visited += 1;
            anyhow::ensure!(
                visited <= MAX_RECEIPT_INVENTORY_ENTRIES,
                "java-owned-source-inventory-cap"
            );
            let entry = entry?;
            anyhow::ensure!(!entry.file_type().is_symlink(), "java-owned-source-symlink");
            if entry.file_type().is_dir() {
                directories.insert(entry.path().to_path_buf());
            }
            if entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| {
                        ext.eq_ignore_ascii_case("kt") || ext.eq_ignore_ascii_case("java")
                    })
            {
                disk.insert(header_key(entry.path()));
            }
        }
    }
    for directory in &directories {
        anyhow::ensure!(
            read_plan.includes_directory_watch(directory),
            "java-owned-directory-arrived-before-watch"
        );
    }
    let selected: BTreeSet<_> = request
        .sources
        .iter()
        .map(|path| header_key(path))
        .collect();
    let wire: BTreeSet<_> = request
        .overlays
        .iter()
        .map(|source| header_key(Path::new(&source.key)))
        .collect();
    anyhow::ensure!(
        wire.is_subset(&selected),
        "java-owned-wire-membership-unknown"
    );
    let current: BTreeSet<_> = disk.union(&wire).cloned().collect();
    anyhow::ensure!(
        current == selected,
        "java-owned-current-source-membership-differs"
    );
    Ok(
        serde_json::json!({"stageCompleted":true, "currentSourceMembershipKnown":true,
        "selectedSources":selected, "diskSources":disk, "wireSources":wire,
        "directories":directories, "producerClosure":false, "apiAuthority":false}),
    )
}

pub(super) fn selected_tasks_supported(
    primary: &CompilerTask,
    java: &CompilerTask,
    tasks: &[CompilerTask],
) -> bool {
    let Some(runtime) = primary.kotlin_compiler.as_ref() else {
        return false;
    };
    let legacy = runtime.compiler_version == "1.8.21" && runtime.gradle_plugin_version == "1.8.21";
    let complete_k2 =
        runtime.compiler_version == "2.4.0" && runtime.gradle_plugin_version == "2.4.0";
    let adapter = if legacy {
        "gradle-7.4.2-stock-actions-v1"
    } else {
        "gradle-9.6.1-stock-actions-v1"
    };
    if (!legacy && !complete_k2)
        || [primary, java].iter().any(|task| {
            task.options
                .get("taskActionInventory")
                .and_then(|fact| fact.get("adapter"))
                .and_then(Value::as_str)
                != Some(adapter)
                || task
                    .options
                    .get("taskOnlyIfInventory")
                    .and_then(|fact| fact.get("adapter"))
                    .and_then(Value::as_str)
                    != Some(if legacy {
                        "gradle-7.4.2-stock-onlyif-v1"
                    } else {
                        "gradle-9.6.1-stock-onlyif-v1"
                    })
        })
        || !matches!(
            primary.task_type.as_str(),
            "org.jetbrains.kotlin.gradle.tasks.KotlinCompile"
                | "org.jetbrains.kotlin.gradle.tasks.KotlinCompile_Decorated"
        )
        || !stock_task_action(
            primary,
            "org.jetbrains.kotlin.gradle.tasks.AbstractKotlinCompile",
            "execute",
        )
        || !stock_only_if(primary, false)
        || !stock_task_action(java, "org.gradle.api.tasks.compile.JavaCompile", "compile")
        || !stock_only_if(java, false)
    {
        return false;
    }
    let auxiliary: Vec<_> = tasks
        .iter()
        .filter(|task| task.task != primary.task && task.task != java.task)
        .collect();
    if auxiliary.is_empty() {
        return true;
    }
    if complete_k2 {
        return false;
    }
    // The only auxiliary path admitted here is KGP 1.8.21's inactive KAPT pair.
    // KaptWithoutKotlincTask.compile returns before submitWork when both fields are empty,
    // even if includeCompileClasspath=true. An empty AP path alone is insufficient.
    if auxiliary.len() != 2
        || auxiliary.iter().any(|task| {
            !task.complete
                || task.problem.is_some()
                || task
                    .options
                    .get("taskActionInventory")
                    .and_then(|fact| fact.get("adapter"))
                    .and_then(Value::as_str)
                    != Some("gradle-7.4.2-stock-actions-v1")
                || task
                    .options
                    .get("taskOnlyIfInventory")
                    .and_then(|fact| fact.get("adapter"))
                    .and_then(Value::as_str)
                    != Some("gradle-7.4.2-stock-onlyif-v1")
                || task.language != crate::classpath::CompilerLanguage::Kotlin
                || !task.annotation_processor_path.is_empty()
        })
    {
        return false;
    }
    let Some(apt) = auxiliary.iter().find(|task| {
        matches!(
            task.task_type.as_str(),
            "org.jetbrains.kotlin.gradle.internal.KaptWithoutKotlincTask"
                | "org.jetbrains.kotlin.gradle.internal.KaptWithoutKotlincTask_Decorated"
        )
    }) else {
        return false;
    };
    let Some(stubs) = auxiliary.iter().find(|task| {
        matches!(
            task.task_type.as_str(),
            "org.jetbrains.kotlin.gradle.internal.KaptGenerateStubsTask"
                | "org.jetbrains.kotlin.gradle.internal.KaptGenerateStubsTask_Decorated"
        )
    }) else {
        return false;
    };
    let empty_array = |name| {
        apt.options
            .get(name)
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
    };
    if !apt.arguments.is_empty()
        || !empty_array("processors")
        || !empty_array("providerArguments")
        || apt
            .options
            .get("annotationProcessorOptionProviderCount")
            .and_then(Value::as_u64)
            != Some(0)
        || !stock_task_action(
            apt,
            "org.jetbrains.kotlin.gradle.internal.KaptWithoutKotlincTask",
            "compile",
        )
        || !stock_only_if(apt, true)
        || !stock_task_action(
            stubs,
            "org.jetbrains.kotlin.gradle.tasks.AbstractKotlinCompile",
            "execute",
        )
        || !stock_only_if(stubs, false)
        || stubs.kotlin_compiler.as_ref() != Some(runtime)
        || stubs.java_home != primary.java_home
    {
        return false;
    }
    let main_sources: BTreeSet<_> = primary
        .source_files
        .iter()
        .map(|path| header_key(path))
        .collect();
    let stub_sources: BTreeSet<_> = stubs
        .source_files
        .iter()
        .map(|path| header_key(path))
        .collect();
    if main_sources != stub_sources
        || stub_sources.len() != stubs.source_files.len()
        || stubs.output_roots.is_empty()
        || stubs.declared_output_roots.is_empty()
    {
        return false;
    }
    // Stubs are not a Java API lease. Their configured outputs must not supply any read slot;
    // custom/KSP/active AP producers need a separate current-input proof before admission.
    let reads = primary
        .source_files
        .iter()
        .chain(java.source_files.iter())
        .chain(primary.classpath.iter())
        .chain(java.classpath.iter());
    let reads: Vec<_> = reads.map(|path| header_key(path)).collect();
    stubs.output_roots.iter().all(|root| {
        stubs
            .declared_output_roots
            .iter()
            .any(|declared| same_path(root, declared))
    }) && stubs.declared_output_roots.iter().all(|root| {
        root.is_absolute()
            && reads
                .iter()
                .all(|read| !paths_overlap(&header_key(root), read))
    })
}

/// The complete captured list is retained. Only its three owned outputs and one Kotlin CP slot move.
fn replay_options(
    task: &CompilerTask,
    recipe: &JavaCompilerRecipe,
    kotlin_output: &Path,
    private: &Path,
) -> anyhow::Result<Vec<String>> {
    let options = &recipe.api_options;
    let explicit_utf8_jdk21 = recipe.adapter == "gradle-9.6.1-current-jdk21-utf8-javac-api-v1";
    let complete_k2 =
        explicit_utf8_jdk21 || recipe.adapter == "gradle-9.6.1-current-jdk17-javac-api-v1";
    let encoding_slots = if explicit_utf8_jdk21 { 2 } else { 0 };
    let level = if explicit_utf8_jdk21 { "21" } else { "17" };
    anyhow::ensure!(
        options.len()
            == if explicit_utf8_jdk21 {
                19
            } else if complete_k2 {
                17
            } else {
                18
            },
        "java-api-options-shape-unhandled"
    );
    for (index, expected) in [
        (0, "-source"),
        (1, level),
        (2, "-target"),
        (3, level),
        (4, "-d"),
        (6, "-h"),
        (8, "-g"),
        (9, "-sourcepath"),
        (10, ""),
        (11, "-proc:none"),
        (12, "-s"),
        (14, "-XDuseUnsharedTable=true"),
        (15, "-classpath"),
    ] {
        let index = if index >= 6 {
            index + encoding_slots
        } else {
            index
        };
        anyhow::ensure!(
            options[index] == expected,
            "java-api-options-unhandled-at-{index}"
        );
    }
    anyhow::ensure!(
        !explicit_utf8_jdk21 || (options[6] == "-encoding" && options[7] == "UTF-8"),
        "java-api-encoding-options-differ"
    );
    anyhow::ensure!(
        complete_k2 || options[17] == "-proc:none",
        "java-api-options-unhandled-at-17"
    );
    anyhow::ensure!(
        task.output_roots.len() == 1 && same_path(Path::new(&options[5]), &task.output_roots[0]),
        "java-output-role-unknown"
    );
    for position in [7 + encoding_slots, 13 + encoding_slots] {
        anyhow::ensure!(
            task.declared_output_roots
                .iter()
                .any(|root| same_path(root, Path::new(&options[position]))),
            "java-secondary-output-role-unknown"
        );
    }
    anyhow::ensure!(
        options[16 + encoding_slots] == joined_classpath(&task.classpath)?,
        "java-captured-classpath-differs"
    );
    let slots: Vec<_> = task
        .classpath
        .iter()
        .enumerate()
        .filter(|(_, path)| same_path(path, kotlin_output))
        .map(|(index, _)| index)
        .collect();
    anyhow::ensure!(slots.len() == 1, "java-kotlin-output-slot-unknown");
    for (index, path) in task.classpath.iter().enumerate() {
        anyhow::ensure!(
            index == slots[0]
                || !paths_overlap(
                    &kotlin_path_key(&crate::paths::file_key(path)),
                    &kotlin_path_key(&crate::paths::file_key(kotlin_output))
                ),
            "java-other-classpath-overlaps-kotlin-output"
        );
    }
    let mut classpath = task.classpath.clone();
    classpath[slots[0]] = private.join("lease");
    let mut result = options.clone();
    for (index, directory) in [
        (5, "java-classes"),
        (7 + encoding_slots, "java-headers"),
        (13 + encoding_slots, "java-generated"),
    ] {
        result[index] = path_text(&private.join(directory))?;
    }
    result[16 + encoding_slots] = joined_classpath(&classpath)?;
    Ok(result)
}

pub(super) fn configured_recipe(
    request: &KotlinDiagnosticRequest,
) -> anyhow::Result<JavaCompilerRecipe> {
    anyhow::ensure!(
        std::env::var_os("JDK_JAVAC_OPTIONS")
            .filter(|value| !value.is_empty())
            .is_none(),
        "java-ambient-javac-options-unmodeled"
    );
    let task = request
        .java_task
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("java-selected-task-unavailable"))?;
    let recipe = task
        .java_compiler_recipe()
        .ok_or_else(|| anyhow::anyhow!("java-selected-recipe-unavailable"))?;
    let kotlin = request
        .task
        .kotlin_compiler
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("kotlin-recipe-unavailable"))?;
    let explicit_utf8_jdk21 = recipe.adapter == "gradle-9.6.1-current-jdk21-utf8-javac-api-v1";
    anyhow::ensure!(
        ((kotlin.compiler_version == "1.8.21"
            && kotlin.gradle_plugin_version == "1.8.21"
            && !kotlin.run_via_build_tools_api
            && recipe.adapter == "gradle-7.4.2-current-jdk17-javac-api-v1")
            || (k2_normal(request)
                && kotlin.run_via_build_tools_api
                && matches!(
                    (recipe.adapter.as_str(), &kotlin.execution_strategy),
                    (
                        "gradle-9.6.1-current-jdk17-javac-api-v1",
                        crate::classpath::KotlinCompilerExecutionStrategy::InProcess
                    ) | (
                        "gradle-9.6.1-current-jdk21-utf8-javac-api-v1",
                        crate::classpath::KotlinCompilerExecutionStrategy::Daemon
                    )
                )))
            && kotlin.normalized_daemon_jvm_arguments.is_empty(),
        "java-header-compiler-version-unhandled"
    );
    let sdk = task
        .java_home
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("java-sdk-unknown"))?;
    anyhow::ensure!(
        request
            .task
            .java_home
            .as_ref()
            .is_some_and(|home| same_path(home, sdk))
            && same_path(&kotlin.gradle_java_home, sdk),
        "java-kotlin-sdk-selection-differs"
    );
    let version_prefix = if explicit_utf8_jdk21 { "21." } else { "17." };
    anyhow::ensure!(
        recipe.gradle_java_version.starts_with(version_prefix)
            && recipe.java_runtime_version.starts_with(version_prefix)
            && recipe.effective_charset == "UTF-8",
        "java-runtime-or-charset-unhandled"
    );
    anyhow::ensure!(
        (task.annotation_processing_disabled()
            || (k2_normal(request) && task.arguments.is_empty()))
            && task.annotation_processor_path.is_empty()
            && task.options.get("bootstrapClasspath") == Some(&Value::Null)
            && task.options.get("release") == Some(&Value::Null)
            && if explicit_utf8_jdk21 {
                task.options.get("encoding").and_then(Value::as_str) == Some("UTF-8")
            } else {
                task.options.get("encoding") == Some(&Value::Null)
            },
        "java-custom-compiler-options-unhandled"
    );
    anyhow::ensure!(
        request.task.output_roots.len() == 1,
        "java-kotlin-output-unknown"
    );
    let declared: BTreeSet<_> = request
        .task
        .source_files
        .iter()
        .map(|p| header_key(p))
        .collect();
    let selected: BTreeSet<_> = request.sources.iter().map(|p| header_key(p)).collect();
    anyhow::ensure!(
        declared == selected
            && declared.len() == request.task.source_files.len()
            && selected.len() == request.sources.len(),
        "java-aux-new-or-duplicate-source-unhandled"
    );
    let declared_java: BTreeSet<_> = request
        .sources
        .iter()
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("java"))
        .map(|p| header_key(p))
        .collect();
    let selected_java: BTreeSet<_> = task.source_files.iter().map(|p| header_key(p)).collect();
    anyhow::ensure!(
        !selected_java.is_empty()
            && selected_java == declared_java
            && selected_java.len() == task.source_files.len(),
        "java-selected-source-membership-differs"
    );
    anyhow::ensure!(
        request
            .overlays
            .iter()
            .any(|source| source.key.ends_with(".java")
                && selected_java.contains(&header_key(Path::new(&source.key)))),
        "java-no-open-selected-source"
    );
    replay_options(
        task,
        &recipe,
        &request.task.output_roots[0],
        &request.project_dir,
    )?;
    Ok(recipe)
}

fn java_inputs(request: &KotlinDiagnosticRequest) -> anyhow::Result<Vec<PathBuf>> {
    let task = request.java_task.as_ref().unwrap();
    let kotlin_output = &request.task.output_roots[0];
    let sources: BTreeSet<_> = request.sources.iter().map(|p| header_key(p)).collect();
    let under_kotlin_output =
        |path: &Path| path_under(&header_key(path), &header_key(kotlin_output));
    let mut inputs: BTreeSet<_> = task
        .classpath
        .iter()
        .filter(|path| !same_path(path, kotlin_output))
        .cloned()
        .collect();
    for input in &task.declared_input_files {
        if !sources.contains(&header_key(input)) && !under_kotlin_output(input) {
            inputs.insert(input.clone());
        }
    }
    inputs.extend(dependency_inputs(request)?);
    let sdk = task.java_home.as_ref().unwrap();
    anyhow::ensure!(
        sdk.join("lib/modules").is_file(),
        "java-sdk-layout-unhandled"
    );
    inputs.extend([
        sdk.join("release"),
        sdk.join("bin"),
        sdk.join("lib/modules"),
        sdk.join("lib/jvm.cfg"),
        sdk.join("lib/jrt-fs.jar"),
        sdk.join("conf"),
    ]);
    for input in &inputs {
        path_text(input)?;
        anyhow::ensure!(
            !request
                .source_roots
                .iter()
                .any(|root| paths_overlap(&header_key(input), &header_key(root)))
                && !task
                    .output_roots
                    .iter()
                    .any(|root| paths_overlap(&header_key(input), &header_key(root)))
                && !paths_overlap(&header_key(input), &header_key(kotlin_output)),
            "java-read-input-overlaps-owned-role"
        );
        anyhow::ensure!(
            !path_under(&header_key(&request.project_dir), &header_key(input)),
            "java-unbounded-project-input"
        );
    }
    Ok(inputs.into_iter().collect())
}

fn write_new(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.flush()?;
    Ok(())
}

fn read_json(path: &Path) -> anyhow::Result<Value> {
    let metadata = std::fs::symlink_metadata(path)?;
    anyhow::ensure!(
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && metadata.len() <= MAX_OUTPUT_BYTES as u64,
        "java-structured-receipt-unavailable"
    );
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

fn native(
    command: &mut Command,
    request: &KotlinDiagnosticRequest,
    environment: &KotlinDiagnosticEnvironment,
) -> anyhow::Result<(i32, String)> {
    request.check_live()?;
    environment.check_live()?;
    #[cfg(windows)]
    {
        let (status, interrupted, mut out, out_complete, err, err_complete) =
            OwnedNativeCompile::spawn(command, MAX_OUTPUT_BYTES)?
                .wait(COMPILE_TIMEOUT, Some(request.invalid.as_ref()));
        request.check_live()?;
        environment.check_live()?;
        anyhow::ensure!(
            !interrupted && out_complete && err_complete,
            "java-native-incomplete"
        );
        out.push('\n');
        out.push_str(&err);
        let code = status
            .code()
            .ok_or_else(|| anyhow::anyhow!("java-native-exit-unknown"))?;
        Ok((code, out))
    }
    #[cfg(not(windows))]
    {
        let _ = command;
        anyhow::bail!("native-tree-proof-unavailable")
    }
}

fn kotlin(
    arguments: &[String],
    file: &Path,
    request: &KotlinDiagnosticRequest,
    environment: &KotlinDiagnosticEnvironment,
) -> anyhow::Result<(i32, String)> {
    write_new(file, kotlin_argument_file(arguments)?.as_bytes())?;
    let recipe = &environment.key.recipe;
    let runtime = std::env::join_paths(
        recipe
            .default_classpath
            .iter()
            .chain(recipe.current_jvm_tools_jar.iter()),
    )?;
    anyhow::ensure!(
        runtime.to_string_lossy().encode_utf16().count() < 24_000,
        "kotlin-runtime-command-line-unhandled"
    );
    let mut command = Command::new(recipe.gradle_java_home.join(if cfg!(windows) {
        "bin/java.exe"
    } else {
        "bin/java"
    }));
    command
        .current_dir(&request.project_dir)
        .arg("-Dfile.encoding=UTF-8")
        .arg("-Dorg.jetbrains.kotlin.cliMessageRenderer=GradleStyle")
        .arg("-cp")
        .arg(runtime)
        .arg("org.jetbrains.kotlin.cli.jvm.K2JVMCompiler")
        .arg(format!("@{}", file.display()));
    native(&mut command, request, environment)
}

pub(super) struct Tools {
    owner: KotlinEditorOverlay,
    stamps: Vec<(PathBuf, KotlinInputStamp)>,
    input_sha256: String,
    dependency_packages: BTreeSet<String>,
    dependency_domains: Value,
}

impl Tools {
    fn check_live(&self) -> anyhow::Result<()> {
        for (path, stamp) in &self.stamps {
            anyhow::ensure!(
                kotlin_input_stamp(path)? == *stamp,
                "java-helper-or-input-stamp-changed"
            );
        }
        Ok(())
    }

    fn prepare(request: &KotlinDiagnosticRequest) -> anyhow::Result<Self> {
        configured_recipe(request)?;
        let owner = KotlinEditorOverlay::empty_in(
            request,
            &crate::deps::cache_home().join("java-editor-tools"),
        )?;
        Ok(Self {
            owner,
            stamps: Vec::new(),
            input_sha256: String::new(),
            dependency_packages: BTreeSet::new(),
            dependency_domains: Value::Null,
        })
    }

    fn capture_inputs(
        &mut self,
        request: &KotlinDiagnosticRequest,
        environment: &KotlinDiagnosticEnvironment,
    ) -> anyhow::Result<()> {
        let owner = &self.owner;
        let source_root = owner.directory.join("assets");
        std::fs::create_dir(&source_root)?;
        let mut inputs = java_inputs(request)?;
        for (name, text) in header_assets(request) {
            let path = source_root.join(name);
            write_new(&path, text.as_bytes())?;
            inputs.push(path);
        }
        // Reuse the main environment's immutable CP/JDK inventory instead of hashing it twice.
        inputs.retain(|path| {
            !environment
                .stamps
                .iter()
                .any(|(original, _)| same_path(original, path))
        });
        let mut pins = BTreeSet::new();
        pins.extend(environment.key.recipe.default_classpath.iter().cloned());
        pins.extend(environment.key.recipe.current_jvm_tools_jar.iter().cloned());
        pins.extend(kotlin_plugin_paths(&request.task.arguments)?);
        let mut pin_text = String::new();
        for path in pins {
            pin_text.push_str(&format!(
                "{}\t{}\n",
                sha(&std::fs::read(&path)?),
                path_text(&path)?
            ));
        }
        let pins_path = owner.directory.join("plugins.tsv");
        write_new(&pins_path, pin_text.as_bytes())?;
        inputs.push(pins_path);
        let watch_inputs: Vec<_> = inputs
            .iter()
            .cloned()
            .chain(owned_membership_inputs(request)?)
            .collect();
        let read_plan = crate::compiled_watch::WatchPlan::read_inputs(&watch_inputs)?;
        environment.watch.extend(read_plan.clone())?;
        let mut hash = environment
            .key
            .input_hasher(&environment.environment_sha256)?;
        hash.update(assets_sha256().as_bytes());
        let ktcore::jvm::InputIdentity::Binary { content_sha256 } = &environment.input_identity
        else {
            anyhow::bail!("java-kotlin-environment-identity-unavailable");
        };
        hash.update(content_sha256);
        let mut stamps = Vec::new();
        for input in inputs {
            let before = kotlin_input_stamp(&input)?;
            hash.update(serde_json::to_vec(&inventory(&input)?)?);
            anyhow::ensure!(
                before == kotlin_input_stamp(&input)?,
                "java-input-changed-during-inventory"
            );
            stamps.push((input, before));
        }
        for (name, text) in header_assets(request) {
            anyhow::ensure!(
                std::fs::read(source_root.join(name))? == text.as_bytes(),
                "java-helper-asset-identity-differs"
            );
        }
        anyhow::ensure!(
            std::fs::read(owner.directory.join("plugins.tsv"))? == pin_text.as_bytes(),
            "java-helper-plugin-inventory-differs"
        );
        // Watches are installed above, before enumeration. Membership and exact bytes of
        // new saved sources therefore participate in the same monotone invalidation boundary.
        let owned_membership = owned_source_membership(request, &read_plan)?;
        let (dependency_packages, mut dependency_domains) = dependency_domains(request)?;
        dependency_domains
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("java-domain-receipt-unknown"))?
            .insert("ownedSourceMembership".into(), owned_membership);
        for directory in array(&dependency_domains, "directories")? {
            let directory = Path::new(
                directory
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("java-domain-directory-unknown"))?,
            );
            anyhow::ensure!(
                read_plan.includes_directory_watch(directory),
                "java-domain-directory-arrived-before-watch"
            );
        }
        hash.update(serde_json::to_vec(&dependency_domains)?);
        for (path, stamp) in &stamps {
            anyhow::ensure!(
                kotlin_input_stamp(path)? == *stamp,
                "java-dependency-changed-during-domain"
            );
        }
        request.check_live()?;
        environment.check_live()?;
        self.dependency_packages = dependency_packages;
        self.dependency_domains = dependency_domains;
        self.stamps = stamps;
        self.input_sha256 = format!("{:x}", hash.finalize());
        Ok(())
    }

    fn compile(
        &mut self,
        request: &KotlinDiagnosticRequest,
        environment: &KotlinDiagnosticEnvironment,
    ) -> anyhow::Result<()> {
        self.capture_inputs(request, environment)?;
        let source_root = self.owner.directory.join("assets");
        let plugin_classes = self.owner.directory.join("classes/headers");
        std::fs::create_dir(&plugin_classes)?;
        let recipe = &environment.key.recipe;
        let mut arguments = vec![
            "-no-stdlib".into(),
            "-no-reflect".into(),
            "-jvm-target".into(),
            "17".into(),
            "-jdk-home".into(),
            path_text(
                request
                    .java_task
                    .as_ref()
                    .unwrap()
                    .java_home
                    .as_ref()
                    .unwrap(),
            )?,
            "-classpath".into(),
            joined_classpath(&recipe.default_classpath)?,
            "-d".into(),
            path_text(&plugin_classes)?,
            "-version".into(),
        ];
        if k2_normal(request) {
            arguments.push("-Xcontext-parameters".into());
        }
        let header_names: Vec<_> = header_assets(request)
            .into_iter()
            .map(|(name, _)| name)
            .filter(|name| name.ends_with(".kt"))
            .collect();
        for name in &header_names {
            arguments.push(path_text(&source_root.join(name))?);
        }
        let (exit, text) = kotlin(
            &arguments,
            &self.owner.directory.join("build-headers.args"),
            request,
            environment,
        )?;
        anyhow::ensure!(exit == 0, "java-header-tool-compilation-failed: {text}");
        let tool_paths: BTreeMap<_, _> = header_names
            .into_iter()
            .map(|name| {
                let path = source_root.join(name);
                (
                    kotlin_path_key(&crate::paths::file_key(&path)),
                    crate::paths::file_key(&path),
                )
            })
            .collect();
        anyhow::ensure!(
            matches!(
                selected_kotlin_output(
                    &text,
                    &recipe.compiler_version,
                    Some(exit),
                    false,
                    true,
                    &tool_paths
                ),
                KotlinDiagnosticOutcome::Verified { clean: true, .. }
            ),
            "java-header-tool-runtime-unverified"
        );
        let plugin = self.owner.directory.join("headers.jar");
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&plugin)?;
        let mut archive = zip::ZipWriter::new(file);
        let mut classes = BTreeSet::new();
        for entry in walkdir::WalkDir::new(&plugin_classes).follow_links(false) {
            let entry = entry?;
            anyhow::ensure!(
                !entry.file_type().is_symlink(),
                "java-helper-output-symlink"
            );
            if !entry.file_type().is_file() {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(&plugin_classes)?
                .to_string_lossy()
                .replace('\\', "/");
            anyhow::ensure!(
                relative.starts_with("ktlsp/headers/") || relative.starts_with("META-INF/"),
                "java-helper-unexpected-class"
            );
            archive.start_file(&relative, zip::write::SimpleFileOptions::default())?;
            archive.write_all(&std::fs::read(entry.path())?)?;
            classes.insert(relative);
        }
        anyhow::ensure!(
            classes.contains("ktlsp/headers/HeaderRegistrar.class")
                && classes.contains("ktlsp/headers/HeaderOptions.class"),
            "java-helper-registrar-absent"
        );
        for (name, body) in [
            (
                if k2_normal(request) {
                    "META-INF/services/org.jetbrains.kotlin.compiler.plugin.CompilerPluginRegistrar"
                } else {
                    "META-INF/services/org.jetbrains.kotlin.compiler.plugin.ComponentRegistrar"
                },
                "ktlsp.headers.HeaderRegistrar\n",
            ),
            (
                "META-INF/services/org.jetbrains.kotlin.compiler.plugin.CommandLineProcessor",
                "ktlsp.headers.HeaderOptions\n",
            ),
        ] {
            archive.start_file(name, zip::write::SimpleFileOptions::default())?;
            archive.write_all(body.as_bytes())?;
        }
        archive.finish()?.sync_all()?;
        let java_classes = self.owner.directory.join("classes/java");
        std::fs::create_dir(&java_classes)?;
        let sdk = request
            .java_task
            .as_ref()
            .unwrap()
            .java_home
            .as_ref()
            .unwrap();
        let mut command = Command::new(sdk.join(if cfg!(windows) {
            "bin/javac.exe"
        } else {
            "bin/javac"
        }));
        command
            .current_dir(&self.owner.directory)
            .args([
                "-proc:none",
                "-encoding",
                "UTF-8",
                "-source",
                "17",
                "-target",
                "17",
                "-classpath",
                "",
                "-sourcepath",
                "",
                "-d",
            ])
            .arg(&java_classes)
            .arg(source_root.join("JavacDiagnostics.java"));
        let (exit, text) = native(&mut command, request, environment)?;
        anyhow::ensure!(
            exit == 0 && java_classes.join("JavacDiagnostics.class").is_file(),
            "java-diagnostic-tool-compilation-failed: {text}"
        );
        let mut outputs = vec![plugin];
        for entry in walkdir::WalkDir::new(&java_classes).follow_links(false) {
            let entry = entry?;
            anyhow::ensure!(
                !entry.file_type().is_symlink(),
                "java-helper-output-symlink"
            );
            if entry.file_type().is_file() {
                outputs.push(entry.into_path());
            }
        }
        environment
            .watch
            .extend(crate::compiled_watch::WatchPlan::read_inputs(&outputs)?)?;
        let mut hash = Sha256::new();
        hash.update(self.input_sha256.as_bytes());
        for output in outputs {
            let before = kotlin_input_stamp(&output)?;
            hash.update(serde_json::to_vec(&inventory(&output)?)?);
            anyhow::ensure!(
                before == kotlin_input_stamp(&output)?,
                "java-helper-changed-during-inventory"
            );
            self.stamps.push((output, before));
        }
        self.input_sha256 = format!("{:x}", hash.finalize());
        self.check_live()?;
        request.check_live()?;
        environment.check_live()?;
        tracing::debug!(identity = %self.input_sha256, "selected Java tools compiled once for Kotlin environment");
        Ok(())
    }
}

fn properties(values: &BTreeMap<String, String>) -> String {
    fn escape(value: &str) -> String {
        let mut out = String::new();
        for ch in value.chars() {
            match ch {
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\0' => out.push_str("\\u0000"),
                '=' | ':' | '#' | '!' | ' ' => {
                    out.push('\\');
                    out.push(ch);
                }
                _ => out.push(ch),
            }
        }
        out
    }
    values
        .iter()
        .map(|(key, value)| format!("{key}={}\n", escape(value)))
        .collect()
}

fn current_sources(overlay: &KotlinEditorOverlay) -> anyhow::Result<BTreeMap<String, String>> {
    overlay
        .sources
        .iter()
        .map(|path| Ok((header_key(path), sha(&std::fs::read(path)?))))
        .collect()
}

fn check_sources(
    overlay: &KotlinEditorOverlay,
    expected: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        &current_sources(overlay)? == expected,
        "java-current-overlay-changed"
    );
    Ok(())
}

fn check_closed_sources(
    request: &KotlinDiagnosticRequest,
    overlay: &KotlinEditorOverlay,
    expected: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    for private in &overlay.sources {
        let original = overlay
            .paths
            .get(&kotlin_path_key(&crate::paths::file_key(private)))
            .ok_or_else(|| anyhow::anyhow!("java-closed-source-origin-unknown"))?;
        // Match the canonical overlay's exact key policy; a current wire body owns its input.
        if request
            .overlays
            .iter()
            .any(|source| source.key == *original)
        {
            continue;
        }
        anyhow::ensure!(
            expected.get(&header_key(private)) == Some(&sha(&std::fs::read(original)?)),
            "java-closed-source-changed-since-overlay"
        );
    }
    Ok(())
}

fn known_packages(
    receipt: &Value,
    expected_kotlin: &BTreeSet<String>,
) -> anyhow::Result<BTreeMap<String, String>> {
    let proof = receipt
        .get("packageDomainProof")
        .ok_or_else(|| anyhow::anyhow!("java-domain-proof-absent"))?;
    for name in [
        "stageCompleted",
        "allPackageDomainsKnown",
        "nameDomainIsWholePackage",
    ] {
        require_bool(proof, name, true)?;
    }
    anyhow::ensure!(
        array(proof, "globalIssues")?.is_empty(),
        "java-domain-global-issues"
    );
    let mut packages = BTreeMap::new();
    for source in array(proof, "sourceDomains")? {
        let path = value_string(source, "source")?.to_owned();
        let package = value_string(source, "package")?.to_owned();
        anyhow::ensure!(
            !package.is_empty()
                && value_string(source, "status")? == "KnownWholePackage"
                && array(source, "issues")?.is_empty()
                && packages.insert(path, package).is_none(),
            "java-domain-source-unknown"
        );
    }
    anyhow::ensure!(
        packages.keys().cloned().collect::<BTreeSet<_>>() == *expected_kotlin,
        "java-domain-source-membership-differs"
    );
    Ok(packages)
}

// Only fresh owned compiler outputs enter this inventory, never arbitrary project/cache trees.
fn complete_output_inventory(root: &Path) -> anyhow::Result<BTreeMap<String, (u64, String)>> {
    let metadata = std::fs::symlink_metadata(root)?;
    anyhow::ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "java-k2-output-root-unverified"
    );
    let mut files = BTreeMap::new();
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        anyhow::ensure!(!entry.file_type().is_symlink(), "java-k2-output-symlink");
        if !entry.file_type().is_file() {
            continue;
        }
        anyhow::ensure!(
            files.len() < MAX_RECEIPT_INVENTORY_ENTRIES,
            "java-k2-output-inventory-too-large"
        );
        let relative = entry
            .path()
            .strip_prefix(root)?
            .to_string_lossy()
            .replace('\\', "/");
        let body = std::fs::read(entry.path())?;
        anyhow::ensure!(
            files
                .insert(relative, (body.len() as u64, sha(&body)))
                .is_none(),
            "java-k2-output-duplicate"
        );
    }
    anyhow::ensure!(!files.is_empty(), "java-k2-canonical-output-empty");
    Ok(files)
}

fn complete_outputs_match(
    canonical: &Path,
    observed: &Path,
    before: &BTreeMap<String, (u64, String)>,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        complete_output_inventory(canonical)? == *before,
        "java-k2-canonical-output-changed"
    );
    anyhow::ensure!(
        complete_output_inventory(observed)? == *before,
        "java-k2-observer-output-differs"
    );
    for relative in before.keys() {
        anyhow::ensure!(
            std::fs::read(canonical.join(relative))? == std::fs::read(observed.join(relative))?,
            "java-k2-observer-output-bytes-differ"
        );
    }
    Ok(())
}

fn validate_header_stage(
    request: &KotlinDiagnosticRequest,
    receipt: &Value,
    canonical: &[CompileDiagnostic],
    complete_success: bool,
) -> anyhow::Result<bool> {
    let complete_k2 = k2_normal(request);
    if complete_k2 {
        anyhow::ensure!(
            complete_success && canonical.iter().all(|d| d.severity != Severity::Error),
            "java-k2-normal-compilation-required"
        );
        anyhow::ensure!(
            receipt.get("failure").is_none()
                && receipt.get("schema").and_then(Value::as_u64) == Some(2)
                && value_string(receipt, "component")? == "k2-complete-output"
                && value_string(receipt, "compilerVersion")? == "2.4.0"
                && receipt.get("callbackCount").and_then(Value::as_u64) == Some(1),
            "java-k2-complete-stage-unverified"
        );
        for name in [
            "stageCompleted",
            "factoryDone",
            "allSelectedSourcesIncluded",
        ] {
            require_bool(receipt, name, true)?;
        }
        anyhow::ensure!(
            receipt.get("bindingIdentityPreserved").is_none()
                && receipt.get("bindingDiagnosticsPreserved").is_none(),
            "java-k2-fabricated-k1-proof"
        );
        let proof = &receipt["packageDomainProof"];
        anyhow::ensure!(
            proof.get("schema").and_then(Value::as_u64) == Some(2)
                && value_string(proof, "basis")? == "original-fir-complete-output",
            "java-k2-domain-certificate-unavailable"
        );
    } else {
        anyhow::ensure!(
            receipt.get("failure").is_none()
                && receipt.get("schema").and_then(Value::as_u64) == Some(1)
                && value_string(receipt, "compilerVersion")? == "1.8.21"
                && receipt.get("callbackCount").and_then(Value::as_u64) == Some(1),
            "java-header-stage-unverified"
        );
        for name in [
            "stageCompleted",
            "factoryDone",
            "bindingIdentityPreserved",
            "bindingDiagnosticsPreserved",
            "allSelectedSourcesIncluded",
        ] {
            require_bool(receipt, name, true)?;
        }
        require_bool(receipt, "irDiagnosticErrors", false)?;
        anyhow::ensure!(
            receipt
                .get("incompleteHierarchyCount")
                .and_then(Value::as_u64)
                == Some(0)
                && receipt.get("badJavaSignatureCount").and_then(Value::as_u64) == Some(0)
                && array(receipt, "extraJvmDiagnostics")?
                    .iter()
                    .all(|d| d.as_str().is_some_and(|d| !d.ends_with(":ERROR")))
                && array(receipt, "irExtensions")?.is_empty()
                && array(receipt, "classBuilderInterceptors")?.is_empty()
                && array(receipt, "classFileFinalizers")?.is_empty()
                && array(receipt, "analysisHandlers")?
                    == &vec![Value::String("ktlsp.headers.HeaderExtension".into())],
            "java-header-plugin-or-backend-unverified"
        );
    }
    Ok(complete_k2)
}

fn k2_empty_sources(
    receipt: &Value,
    sources: &BTreeMap<String, String>,
) -> anyhow::Result<BTreeSet<String>> {
    let mut empty = BTreeSet::new();
    for source in array(&receipt["packageDomainProof"], "sourceDomains")? {
        require_bool(source, "originalFir", true)?;
        let path = value_string(source, "source")?;
        anyhow::ensure!(
            sources
                .get(path)
                .is_some_and(
                    |digest| source.get("sourceSha256").and_then(Value::as_str) == Some(digest)
                ),
            "java-k2-original-fir-source-identity-differs"
        );
        let declarations = source
            .get("declarationCount")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("java-k2-original-fir-declaration-count-unavailable"))?;
        if declarations == 0 {
            empty.insert(path.to_owned());
        }
    }
    Ok(empty)
}

fn validated_lease(
    request: &KotlinDiagnosticRequest,
    overlay: &KotlinEditorOverlay,
    receipt: &Value,
    expected: &BTreeMap<String, String>,
    canonical: &[CompileDiagnostic],
    complete_success: bool,
) -> anyhow::Result<BTreeSet<String>> {
    let complete_k2 = validate_header_stage(request, receipt, canonical, complete_success)?;
    let mut sources = BTreeMap::new();
    let mut expected_kotlin = BTreeSet::new();
    for row in array(receipt, "sourceInventory")? {
        let path = Path::new(value_string(row, "path")?);
        let key = header_key(path);
        let kind = if path.extension().and_then(|s| s.to_str()) == Some("kt") {
            "kotlin"
        } else {
            "java"
        };
        anyhow::ensure!(
            value_string(row, "kind")? == kind
                && sources
                    .insert(key.clone(), value_string(row, "sha256")?.to_owned())
                    .is_none(),
            "java-header-source-kind-or-duplicate"
        );
        if kind == "kotlin" {
            expected_kotlin.insert(key);
        }
    }
    anyhow::ensure!(&sources == expected, "java-header-source-inventory-differs");
    let packages = known_packages(receipt, &expected_kotlin)?;
    let empty_sources = if complete_k2 {
        k2_empty_sources(receipt, &sources)?
    } else {
        BTreeSet::new()
    };
    let empty_unknown = serde_json::Map::new();
    let unknown = if complete_k2 {
        &empty_unknown
    } else {
        receipt
            .get("unknownSources")
            .and_then(Value::as_object)
            .ok_or_else(|| anyhow::anyhow!("java-api-unknown-source-inventory-absent"))?
    };
    anyhow::ensure!(
        unknown
            .keys()
            .all(|source| expected_kotlin.contains(source)),
        "java-unaccounted-unknown-api"
    );
    if !complete_k2 {
        require_bool(receipt, "allApiTypesKnown", unknown.is_empty())?;
    }
    let error_sources: BTreeSet<_> = canonical
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| header_key(Path::new(&d.path)))
        .collect();
    let raw = overlay.directory.join("raw-headers");
    let lease = overlay.directory.join("lease");
    std::fs::create_dir(&lease)?;
    let mut recorded = BTreeSet::new();
    let mut class_records = BTreeMap::new();
    let mut represented = BTreeSet::new();
    let mut metadata_count = 0;
    for output in array(receipt, "outputs")? {
        anyhow::ensure!(
            recorded.len() < MAX_RECEIPT_INVENTORY_ENTRIES,
            "java-header-output-inventory-too-large"
        );
        let relative = value_string(output, "relativePath")?;
        let relative_path = Path::new(relative);
        anyhow::ensure!(
            !relative.is_empty()
                && !relative.contains('\\')
                && !relative_path.is_absolute()
                && relative_path
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_)))
                && recorded.insert(relative.to_owned()),
            "java-header-output-path-invalid"
        );
        let path = raw.join(relative_path);
        let metadata = std::fs::symlink_metadata(&path)?;
        let body = std::fs::read(&path)?;
        anyhow::ensure!(
            metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.len() == body.len() as u64
                && output.get("bytes").and_then(Value::as_u64) == Some(body.len() as u64)
                && value_string(output, "sha256")? == sha(&body),
            "java-header-output-bytes-differ"
        );
        let source_keys: BTreeSet<_> = array(output, "sources")?
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| anyhow::anyhow!("java-header-output-source-invalid"))
            })
            .collect::<anyhow::Result<_>>()?;
        let metadata_only = complete_k2 && !relative.ends_with(".class");
        if metadata_only {
            metadata_count += 1;
            anyhow::ensure!(metadata_count == 1, "java-k2-metadata-inventory-unhandled");
        }
        anyhow::ensure!(
            if metadata_only {
                value_string(output, "role")? == "k2-module-metadata"
                    && source_keys.is_subset(&expected_kotlin)
                    && relative
                        .strip_prefix("META-INF/")
                        .and_then(|name| name.strip_suffix(".kotlin_module"))
                        .is_some_and(|name| {
                            !name.is_empty()
                                && name.bytes().all(|byte| {
                                    byte.is_ascii_alphanumeric()
                                        || matches!(byte, b'.' | b'_' | b'-')
                                })
                        })
            } else {
                !source_keys.is_empty()
                    && source_keys
                        .iter()
                        .all(|source| expected_kotlin.contains(source))
            },
            "java-header-output-source-unaccounted"
        );
        let status = if complete_k2 {
            "CompleteCompilation"
        } else if source_keys
            .iter()
            .any(|source| unknown.contains_key(source))
        {
            "Unknown"
        } else {
            "SignatureChecked"
        };
        anyhow::ensure!(
            value_string(output, "apiStatus")? == status,
            "java-header-api-status-differs"
        );
        if relative.ends_with(".class") {
            anyhow::ensure!(
                body.starts_with(&[0xca, 0xfe, 0xba, 0xbe]),
                "java-header-class-invalid"
            );
            represented.extend(source_keys.iter().cloned());
            class_records.insert(relative.to_owned(), (sha(&body), source_keys.clone()));
            // API status and canonical body errors differ: neither can leak an uncertain class.
            let eligible = (status == "SignatureChecked" || complete_k2)
                && source_keys.iter().all(|source| {
                    overlay
                        .paths
                        .get(&kotlin_path_key(source))
                        .is_some_and(|original| {
                            !error_sources.contains(&header_key(Path::new(original)))
                        })
                });
            if eligible {
                let target = lease.join(relative_path);
                std::fs::create_dir_all(target.parent().unwrap())?;
                write_new(&target, &body)?;
                anyhow::ensure!(
                    sha(&std::fs::read(target)?) == sha(&body),
                    "java-lease-copy-differs"
                );
            }
        }
    }
    anyhow::ensure!(
        represented.is_subset(&expected_kotlin)
            && expected_kotlin
                .difference(&represented)
                .all(|source| empty_sources.contains(source)),
        "java-raw-header-source-coverage-differs"
    );
    let mut actual = BTreeSet::new();
    for entry in walkdir::WalkDir::new(&raw).follow_links(false) {
        let entry = entry?;
        anyhow::ensure!(!entry.file_type().is_symlink(), "java-raw-output-symlink");
        if entry.file_type().is_file() {
            anyhow::ensure!(
                actual.len() < MAX_RECEIPT_INVENTORY_ENTRIES,
                "java-raw-output-inventory-too-large"
            );
            actual.insert(
                entry
                    .path()
                    .strip_prefix(&raw)?
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    anyhow::ensure!(actual == recorded, "java-raw-header-inventory-differs");
    let proof = &receipt["packageDomainProof"];
    let mut raw_classes = BTreeSet::new();
    for class in array(proof, "rawClasses")? {
        let relative = value_string(class, "relativePath")?;
        let (digest, owners) = class_records
            .get(relative)
            .ok_or_else(|| anyhow::anyhow!("java-domain-class-unaccounted"))?;
        let class_owners: BTreeSet<_> = array(class, "sources")?
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| anyhow::anyhow!("java-domain-class-source"))
            })
            .collect::<anyhow::Result<_>>()?;
        let package = value_string(class, "package")?;
        anyhow::ensure!(
            raw_classes.insert(relative.to_owned())
                && &class_owners == owners
                && owners.len() == 1
                && value_string(class, "sha256")? == digest
                && array(class, "issues")?.is_empty()
                && format!("{}.class", value_string(class, "internalName")?) == relative
                && owners
                    .iter()
                    .all(|owner| packages.get(owner).is_some_and(|p| p == package)),
            "java-domain-raw-class-differs"
        );
    }
    anyhow::ensure!(
        raw_classes == class_records.keys().cloned().collect(),
        "java-domain-class-inventory-differs"
    );
    check_sources(overlay, expected)?;
    request.check_live()?;
    Ok(packages.into_values().collect())
}

/// A nested class import can have any preceding name prefix as its actual JVM package.
fn disjoint_import(name: &str, packages: &BTreeSet<String>) -> bool {
    let mut prefix = String::new();
    let mut count = 0;
    for part in name.split('.') {
        if part.is_empty()
            || !part.chars().enumerate().all(|(i, ch)| {
                ch.is_ascii_alphabetic() || ch == '_' || ch == '$' || (i > 0 && ch.is_ascii_digit())
            })
        {
            return false;
        }
        if !prefix.is_empty() {
            prefix.push('.');
        }
        prefix.push_str(part);
        if packages.contains(&prefix) {
            return false;
        }
        count += 1;
    }
    count >= 2
}

fn import_diagnostics(
    value: &Value,
    options: &[String],
    recipe: &JavaCompilerRecipe,
    expected_sources: &[(PathBuf, String)],
    overlay: &KotlinEditorOverlay,
    packages: &BTreeSet<String>,
    exit: i32,
) -> anyhow::Result<Vec<CompileDiagnostic>> {
    require_bool(value, "compileCompleted", true)?;
    let explicit_utf8_jdk21 = recipe.adapter == "gradle-9.6.1-current-jdk21-utf8-javac-api-v1";
    require_bool(value, "gradleConfiguredEncodingNull", !explicit_utf8_jdk21)?;
    anyhow::ensure!(
        if explicit_utf8_jdk21 {
            value
                .get("gradleConfiguredEncoding")
                .and_then(Value::as_str)
                == Some("UTF-8")
        } else {
            value.get("gradleConfiguredEncoding") == Some(&Value::Null)
        },
        "java-structured-encoding-differs"
    );
    anyhow::ensure!(
        value.get("failure") == Some(&Value::Null)
            && value.get("options") == Some(&serde_json::to_value(options)?)
            && value_string(value, "explicitFileManagerCharset")? == recipe.effective_charset
            && value_string(value, "explicitDiagnosticLocale")? == recipe.locale
            && value_string(value, "gradleFileEncoding")? == recipe.gradle_file_encoding
            && value_string(value, "gradleEffectiveDefaultCharset")? == recipe.effective_charset
            && value_string(value, "gradleEffectiveDefaultLocale")? == recipe.locale,
        "java-structured-input-options-differ"
    );
    let runtime = value
        .get("runtime")
        .ok_or_else(|| anyhow::anyhow!("java-runtime-receipt-absent"))?;
    anyhow::ensure!(
        value_string(runtime, "javaVersion")? == recipe.gradle_java_version
            && value_string(runtime, "javaRuntimeVersion")? == recipe.java_runtime_version
            && value_string(value, "compilerClass")? == "com.sun.tools.javac.api.JavacTool"
            && value_string(value, "fileManagerClass")?
                == "com.sun.tools.javac.file.JavacFileManager",
        "java-selected-runtime-differs"
    );
    let before = array(value, "sourcesBefore")?;
    anyhow::ensure!(
        before == array(value, "sourcesAfter")? && before.len() == expected_sources.len(),
        "java-source-before-after-differs"
    );
    let mut by_uri = BTreeMap::new();
    for (row, (path, digest)) in before.iter().zip(expected_sources) {
        anyhow::ensure!(
            same_path(Path::new(value_string(row, "path")?), path)
                && value_string(row, "sha256")? == digest
                && sha(&std::fs::read(path)?) == *digest,
            "java-selected-source-order-or-bytes-differ"
        );
        let uri = value_string(row, "uri")?.to_owned();
        anyhow::ensure!(
            by_uri.insert(uri, path).is_none(),
            "java-duplicate-source-uri"
        );
    }
    let parsed: Vec<_> = array(value, "parsedUnits")?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow::anyhow!("java-parse-uri-unavailable"))
        })
        .collect::<anyhow::Result<_>>()?;
    anyhow::ensure!(
        parsed.len() == by_uri.len()
            && parsed.into_iter().collect::<BTreeSet<_>>() == by_uri.keys().cloned().collect(),
        "java-parsed-unit-coverage-differs"
    );
    let diagnostics = array(value, "diagnostics")?;
    let errors = diagnostics
        .iter()
        .filter(|d| d.get("kind").and_then(Value::as_str) == Some("ERROR"))
        .count();
    anyhow::ensure!(
        value.get("success").and_then(Value::as_bool) == Some(exit == 0)
            && ((exit == 0 && errors == 0) || (exit == 1 && errors > 0)),
        "java-exit-diagnostic-stream-differs"
    );
    tracing::debug!(diagnostics = %value["diagnostics"], compiler_output = %value["compilerOutput"],
        runtime = %value["runtime"], "selected Java complete structured diagnostic stream");
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    for diagnostic in diagnostics {
        if diagnostic.get("kind").and_then(Value::as_str) != Some("ERROR")
            || diagnostic.get("code").and_then(Value::as_str) != Some("compiler.err.doesnt.exist")
        {
            continue;
        }
        let Some(uri) = diagnostic.get("sourceUri").and_then(Value::as_str) else {
            continue;
        };
        let Some(path) = by_uri.get(uri) else {
            continue;
        };
        let text = std::fs::read_to_string(path)?;
        anyhow::ensure!(
            text.is_ascii(),
            "java-nonascii-position-adapter-unavailable"
        );
        let Some(position) = diagnostic
            .get("position")
            .and_then(Value::as_u64)
            .and_then(|p| usize::try_from(p).ok())
        else {
            continue;
        };
        for imported in array(value, "imports")? {
            if imported.get("sourceUri").and_then(Value::as_str) != Some(uri)
                || imported.get("static").and_then(Value::as_bool) != Some(false)
            {
                continue;
            }
            let name = value_string(imported, "name")?;
            if !disjoint_import(name, packages) {
                continue;
            }
            let Some(start) = imported
                .get("qualifiedStart")
                .and_then(Value::as_u64)
                .and_then(|p| usize::try_from(p).ok())
            else {
                continue;
            };
            let Some(end) = imported
                .get("qualifiedEnd")
                .and_then(Value::as_u64)
                .and_then(|p| usize::try_from(p).ok())
            else {
                continue;
            };
            let Some(dot) = name.rfind('.') else {
                continue;
            };
            // Pinned Javac's missing-package diagnostic points at the select's dot, not its name.
            // Bind that raw range to the original task AST, then publish the class-name token.
            if start >= end
                || end > text.len()
                || &text[start..end] != name
                || diagnostic.get("start").and_then(Value::as_u64) != Some(start as u64)
                || diagnostic.get("end").and_then(Value::as_u64) != Some(end as u64)
                || position != start + dot
            {
                continue;
            }
            let selector = position + 1;
            let original = overlay
                .paths
                .get(&kotlin_path_key(&crate::paths::file_key(path)))
                .ok_or_else(|| anyhow::anyhow!("java-diagnostic-source-unmapped"))?;
            if !seen.insert((original.clone(), selector)) {
                continue;
            }
            let (line, col) = crate::text::LineIndex::new(&text).position(&text, selector);
            result.push(CompileDiagnostic {
                path: original.clone(),
                line: line + 1,
                col: col + 1,
                severity: Severity::Error,
                message: value_string(diagnostic, "message")?.to_owned(),
            });
        }
    }
    Ok(result)
}

pub(super) fn run(
    request: &KotlinDiagnosticRequest,
    environment: &KotlinDiagnosticEnvironment,
    overlay: &KotlinEditorOverlay,
    canonical: &[CompileDiagnostic],
    clean: bool,
) -> anyhow::Result<Vec<CompileDiagnostic>> {
    let started = Instant::now();
    let recipe = configured_recipe(request)?;
    let complete_k2 = k2_normal(request);
    anyhow::ensure!(!complete_k2 || clean, "java-k2-normal-compilation-required");
    // run_kotlin_editor_compile creates this fresh output and calls us only after owned/EOF
    // selected_kotlin_output; clean=true means exit0 with no reported canonical errors.
    let canonical_outputs = if complete_k2 {
        Some(complete_output_inventory(
            &overlay.directory.join("classes"),
        )?)
    } else {
        None
    };
    let mut cached = environment
        .java_tools
        .lock()
        .map_err(|_| anyhow::anyhow!("java-tool-cache-poisoned"))?;
    if cached.is_none() {
        let mut tools = Tools::prepare(request)?;
        let compiled = tools
            .compile(request, environment)
            .map_err(|error| error.to_string());
        // Keep failed-build assets alive, so deleting a watched helper does not invalidate Kotlin.
        // A source cancellation instead lets the next current request retry the build.
        if request.check_live().is_ok() && environment.check_live().is_ok() {
            *cached = Some((tools, compiled));
        } else {
            anyhow::bail!("java-tool-build-invalidated");
        }
    }
    let (tools, compiled) = cached.as_ref().unwrap();
    compiled
        .as_ref()
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    tools.check_live()?;
    let expected = current_sources(overlay)?;
    // Owner watches are armed by Tools::capture_inputs. Close the earlier canonical copy-to-watch
    // window on every run, including cached tools, before using any closed-source package proof.
    check_closed_sources(request, overlay, &expected)?;
    let mut source_inventory = String::new();
    for path in &overlay.sources {
        let kind = if path.extension().and_then(|e| e.to_str()) == Some("kt") {
            "kotlin"
        } else {
            "java"
        };
        source_inventory.push_str(&format!(
            "{kind}\t{}\t{}\n",
            expected[&header_key(path)],
            path_text(path)?
        ));
    }
    let inventory_path = overlay.directory.join("header-sources.tsv");
    write_new(&inventory_path, source_inventory.as_bytes())?;
    let raw = overlay.directory.join("raw-headers");
    std::fs::create_dir(&raw)?;
    let header_receipt = overlay.directory.join("headers.json");
    let mut arguments = kotlin_editor_arguments(
        request,
        &overlay.directory.join(if complete_k2 {
            "raw-headers"
        } else {
            "unused-header-output"
        }),
        &[],
    )?;
    let cp_position = arguments
        .iter()
        .position(|arg| arg == "-classpath" || arg == "-cp")
        .ok_or_else(|| anyhow::anyhow!("java-header-effective-classpath-unknown"))?;
    let cp_file = overlay.directory.join("header-classpath.txt");
    let separator = if cfg!(windows) { ';' } else { ':' };
    write_new(
        &cp_file,
        (arguments[cp_position + 1]
            .split(separator)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n")
            .as_bytes(),
    )?;
    arguments.push(format!(
        "-Xplugin={}",
        tools.owner.directory.join("headers.jar").display()
    ));
    for (key, path) in [
        ("output", raw),
        ("receipt", header_receipt.clone()),
        ("inventory", inventory_path),
        ("plugins", tools.owner.directory.join("plugins.tsv")),
        ("classpath", cp_file),
    ] {
        arguments.push("-P".into());
        arguments.push(format!(
            "plugin:ktlsp.current-api-headers:{key}={}",
            path_text(&path)?
        ));
    }
    arguments.extend(
        overlay
            .sources
            .iter()
            .map(|path| path_text(path))
            .collect::<anyhow::Result<Vec<_>>>()?,
    );
    let (exit, text) = kotlin(
        &arguments,
        &overlay.directory.join("headers.args"),
        request,
        environment,
    )?;
    let KotlinDiagnosticOutcome::Verified {
        diagnostics,
        clean: header_clean,
    } = selected_kotlin_output(
        &text,
        &environment.key.recipe.compiler_version,
        Some(exit),
        false,
        true,
        &overlay.paths,
    )
    else {
        anyhow::bail!("java-header-canonical-diagnostics-unverified");
    };
    anyhow::ensure!(
        diagnostics.as_slice() == canonical && header_clean == clean,
        "java-header-changed-canonical-diagnostics"
    );
    if let Some(before) = &canonical_outputs {
        complete_outputs_match(
            &overlay.directory.join("classes"),
            &overlay.directory.join("raw-headers"),
            before,
        )?;
    }
    let receipt = read_json(&header_receipt)?;
    let mut packages = validated_lease(
        request,
        overlay,
        &receipt,
        &expected,
        canonical,
        clean && header_clean && exit == 0,
    )?;
    packages.extend(tools.dependency_packages.iter().cloned());
    tools.check_live()?;
    let task = request.java_task.as_ref().unwrap();
    let options = replay_options(
        task,
        &recipe,
        &request.task.output_roots[0],
        &overlay.directory,
    )?;
    for name in ["java-classes", "java-headers", "java-generated"] {
        std::fs::create_dir(overlay.directory.join(name))?;
    }
    let mut java_sources = Vec::new();
    for original in &task.source_files {
        let private = overlay
            .sources
            .iter()
            .find(|path| {
                overlay
                    .paths
                    .get(&kotlin_path_key(&crate::paths::file_key(path)))
                    .is_some_and(|key| same_path(Path::new(key), original))
            })
            .ok_or_else(|| anyhow::anyhow!("java-selected-source-not-in-overlay"))?;
        anyhow::ensure!(
            std::fs::read_to_string(private)?.is_ascii(),
            "java-nonascii-position-adapter-unavailable"
        );
        java_sources.push((private.clone(), expected[&header_key(private)].clone()));
    }
    let spec = overlay.directory.join("java.properties");
    let mut values = BTreeMap::from([
        ("stage".into(), "current-editor-explicit-imports".into()),
        (
            "expected.javaHome".into(),
            path_text(task.java_home.as_ref().unwrap())?,
        ),
        (
            "expected.javaVersion".into(),
            recipe.gradle_java_version.clone(),
        ),
        (
            "expected.javaRuntimeVersion".into(),
            recipe.java_runtime_version.clone(),
        ),
        (
            "gradle.configuredEncodingNull".into(),
            (task.options.get("encoding") == Some(&Value::Null)).to_string(),
        ),
        ("effective.charset".into(), recipe.effective_charset.clone()),
        ("effective.locale".into(), recipe.locale.clone()),
        (
            "gradle.fileEncoding".into(),
            recipe.gradle_file_encoding.clone(),
        ),
        ("option.count".into(), options.len().to_string()),
        ("source.count".into(), java_sources.len().to_string()),
    ]);
    for (index, option) in options.iter().enumerate() {
        values.insert(format!("option.{index}"), option.clone());
    }
    for (index, (path, digest)) in java_sources.iter().enumerate() {
        values.insert(format!("source.{index}.path"), path_text(path)?);
        values.insert(format!("source.{index}.sha256"), digest.clone());
    }
    write_new(&spec, properties(&values).as_bytes())?;
    let output = overlay.directory.join("java.json");
    let mut command = Command::new(task.java_home.as_ref().unwrap().join(if cfg!(windows) {
        "bin/java.exe"
    } else {
        "bin/java"
    }));
    command
        .current_dir(&request.project_dir)
        .arg("-cp")
        .arg(tools.owner.directory.join("classes/java"))
        .arg("JavacDiagnostics")
        .arg(&spec)
        .arg(&output);
    let (exit, _) = native(&mut command, request, environment)?;
    anyhow::ensure!(matches!(exit, 0 | 1), "java-helper-failed");
    let value = read_json(&output)?;
    // Keep all actual errors, including non-admitted parameter cascades, in the existing trace.
    if crate::trace::enabled() {
        crate::trace::span(
            "compiler.java.editor",
            "compiler",
            started,
            serde_json::json!({
                "module": request.scope.module, "source_set": request.scope.source_set, "generation": request.generation,
                "helper_input_sha256": tools.input_sha256, "header_receipt": receipt, "java_receipt": value,
                "dependent_source_domains": tools.dependency_domains,
                "producer_closure": false, "whole_api_authority": false,
                "k2_normal_output_matches_canonical": complete_k2.then_some(true),
            }),
        );
    }
    anyhow::ensure!(
        value_string(&value, "inputSpecSha256")? == sha(&std::fs::read(&spec)?),
        "java-helper-spec-identity-differs"
    );
    let result = import_diagnostics(
        &value,
        &options,
        &recipe,
        &java_sources,
        overlay,
        &packages,
        exit,
    )?;
    check_sources(overlay, &expected)?;
    tools.check_live()?;
    request.check_live()?;
    environment.check_live()?;
    tracing::debug!(identity = %tools.input_sha256, errors = result.len(), "selected Java explicit import positive diagnostics");
    Ok(result)
}

#[cfg(test)]
#[path = "java_tests.rs"]
mod tests;
