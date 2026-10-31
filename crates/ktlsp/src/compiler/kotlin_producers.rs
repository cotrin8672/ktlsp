//! Current normal outputs for the bounded plain Kotlin producer closure.
use super::*;
use crate::classpath::{CompilerLanguage, CompilerTask, ProjectSnapshot, SourceSetClasspath};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use sha2::{Digest, Sha256};

const MAX_PRODUCERS: usize = 16;

fn key(path: &Path) -> String { kotlin_path_key(&crate::paths::file_key(path)) }

fn rejected<T>(task: &str, reason: &'static str) -> Option<T> {
    crate::trace::span("compiler.kotlin.producer-rejected", "compiler", Instant::now(),
        serde_json::json!({"task":task,"reason":reason}));
    None
}

fn tasks(source: &SourceSetClasspath) -> Option<(&CompilerTask, &CompilerTask)> {
    if source.compiler_metadata_incomplete || source.classpath_incomplete
        || !source.unresolved.is_empty() || source.compiler_tasks.len() != 2 {
        return rejected(source.kotlin_compile_task.as_deref().unwrap_or("unknown"), "producer-model-incomplete");
    }
    let kotlin = source.compiler_tasks.iter().find(|t| t.language == CompilerLanguage::Kotlin
        && Some(t.task.as_str()) == source.kotlin_compile_task.as_deref())?;
    let java = source.compiler_tasks.iter().find(|t| t.language == CompilerLanguage::Java)?;
    let selected_java: BTreeSet<_> = kotlin.source_files.iter()
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("java"))
        .map(|path| key(path)).collect();
    let java_sources: BTreeSet<_> = java.source_files.iter().map(|path| key(path)).collect();
    let mixed = !java.source_files.is_empty();
    let source_roles = if mixed {
        java.java_compiler_recipe().is_some_and(|recipe|
            recipe.adapter == "gradle-9.8.2-current-jdk21-release8-default-utf8-javac-api-v1"
                && java.java_api_option_slots(&recipe).is_ok())
            && selected_java == java_sources && java_sources.len() == java.source_files.len()
            && java.source_files.iter().all(|path| path.extension().and_then(|e| e.to_str()) == Some("java"))
            && kotlin.source_files.iter().any(|path| path.extension().and_then(|e| e.to_str()) == Some("kt"))
            && kotlin.source_files.iter().all(|path| matches!(path.extension().and_then(|e| e.to_str()), Some("kt" | "java")))
    } else {
        kotlin.source_files.iter().all(|path| path.extension().and_then(|e| e.to_str()) == Some("kt"))
    };
    // A mixed producer must replay the checked Java body; only a proved zero inventory gets an empty slot.
    if !kotlin.complete || kotlin.problem.is_some() || !java.complete || java.problem.is_some()
        || kotlin.source_files.is_empty() || !source_roles
        || kotlin.source_files.iter().map(|path| key(path)).collect::<BTreeSet<_>>().len() != kotlin.source_files.len()
        || !crate::classpath::sdk_producer_roots(kotlin).is_some_and(|(_, complete)| complete)
        || !crate::classpath::sdk_producer_roots(java).is_some_and(|(_, complete)| complete)
        || kotlin.output_roots.len() != 1 || java.output_roots.len() != 1
        || !java_diagnostics::selected_tasks_supported(kotlin, java, &source.compiler_tasks)
        || !(java.annotation_processing_disabled()
            || (java.java_compiler_recipe().is_none() && java.arguments.is_empty()))
        || !java.annotation_processor_path.is_empty()
    { return rejected(&kotlin.task, "producer-recipe-or-inventory-unhandled"); }
    let outputs = [&kotlin.output_roots[0], &java.output_roots[0]];
    if outputs.iter().any(|p| !p.is_absolute())
        || paths_overlap(&key(outputs[0]), &key(outputs[1])) {
        return rejected(&kotlin.task, "producer-normal-output-roles-unhandled");
    }
    Some((kotlin, java))
}

/// Match every modeled producer read, including both language output slots. A shared or
/// overlapping artifact cannot be resolved by taking the first owner.
pub(crate) fn capture(
    model: &ProjectSnapshot,
    owner: &SourceSetClasspath,
    primary: &CompilerTask,
    java: &CompilerTask,
) -> Option<Vec<KotlinDependencyCompilation>> {
    if let Err(reason) = java_diagnostics::selected_tasks_supported_reason(primary, java, &owner.compiler_tasks) {
        return rejected(&primary.task, reason);
    }
    capture_dependencies(model, owner, primary, java)
}

fn capture_dependencies(
    model: &ProjectSnapshot, owner: &SourceSetClasspath,
    primary: &CompilerTask, java: &CompilerTask,
) -> Option<Vec<KotlinDependencyCompilation>> {
    let sets: Vec<_> = model.modules.iter().flat_map(|m| m.source_sets.iter().map(move |s| (m, s))).collect();
    let dependencies = |primary: &CompilerTask, java: &CompilerTask, own: &SourceSetClasspath| -> Option<Vec<usize>> {
        let mut found = BTreeSet::new();
        for (read, primary_read) in primary.classpath.iter().map(|read| (read, true))
            .chain(java.classpath.iter().map(|read| (read, false))) {
            let read_key = key(read);
            let owners: Vec<_> = sets.iter().enumerate().filter(|(_, (_, source))| {
                source.output_roots.iter().chain(&source.provided_artifacts)
                    .chain(source.compiler_tasks.iter().flat_map(|t| t.output_roots.iter().chain(&t.declared_output_roots)))
                    .any(|output| paths_overlap(&read_key, &key(output)))
            }).collect();
            match owners.as_slice() {
                [] => {
                    // A project-local read with no modeled owner cannot become an external
                    // binary merely because its producer was absent from the captured model.
                    if model.modules.iter().any(|m| read.starts_with(&m.project_dir)) {
                        return rejected(&primary.task, "unmodeled-project-read");
                    }
                }
                [(_, (_, source))] if std::ptr::eq(*source, own) => {
                    if primary_read && primary.output_roots.iter().chain(&java.output_roots).chain(&source.provided_artifacts)
                        .any(|output| paths_overlap(&read_key, &key(output))) {
                        return rejected(&primary.task, "primary-reads-own-normal-output");
                    }
                }
                [(index, (_, source))] => {
                    let (kt, j) = tasks(source)?;
                    let provided_jar = source.provided_artifacts.iter().any(|p|
                        p.extension().and_then(|e| e.to_str()) == Some("jar") && key(p) == read_key);
                    if !provided_jar && !kt.output_roots.iter().chain(&j.output_roots).any(|p| key(p) == read_key) {
                        return rejected(&primary.task, "producer-read-not-normal-output");
                    }
                    found.insert(*index);
                }
                _ => return rejected(&primary.task, "ambiguous-producer-read"),
            }
        }
        Some(found.into_iter().collect())
    };
    let seeds = dependencies(primary, java, owner)?;
    if seeds.is_empty() { return Some(Vec::new()); }
    let mut order = Vec::new();
    let mut states = BTreeMap::new();
    fn visit(
        node: usize, states: &mut BTreeMap<usize, u8>, order: &mut Vec<usize>,
        sets: &[(&crate::classpath::ModuleClasspath, &SourceSetClasspath)],
        dependencies: &impl Fn(&CompilerTask, &CompilerTask, &SourceSetClasspath) -> Option<Vec<usize>>,
    ) -> Option<()> {
        match states.get(&node) { Some(1) => return None, Some(2) => return Some(()), _ => {} }
        if states.len() >= MAX_PRODUCERS { return None; }
        states.insert(node, 1);
        let (kt, java) = tasks(sets[node].1)?;
        for dependency in dependencies(kt, java, sets[node].1)? {
            visit(dependency, states, order, sets, dependencies)?;
        }
        states.insert(node, 2);
        order.push(node);
        Some(())
    }
    for seed in seeds { visit(seed, &mut states, &mut order, &sets, &dependencies)?; }
    order.into_iter().map(|index| {
        let (module, source) = sets[index];
        let (kt, _) = tasks(source)?;
        let scope = model.file_origin(&kt.source_files[0]).and_then(|origin| origin.project_scope)
            .or_else(|| rejected(&kt.task, "producer-source-scope-unknown"))?;
        Some(KotlinDependencyCompilation {
            project_dir: module.project_dir.clone(), scope, source: source.clone(),
            artifact: kt.output_roots[0].clone(),
        })
    }).collect()
}

fn rewrite(task: &mut CompilerTask, bindings: &BTreeMap<String, Vec<PathBuf>>) -> anyhow::Result<()> {
    let java_recipe = if task.language == CompilerLanguage::Java {
        if let Some(recipe) = task.java_compiler_recipe() {
            let slots = task.java_api_option_slots(&recipe)?;
            Some((recipe, slots.classpath))
        } else {
            anyhow::ensure!(task.complete && task.problem.is_none()
                && task.source_files.is_empty() && task.arguments.is_empty()
                && task.annotation_processor_path.is_empty()
                && crate::classpath::sdk_producer_roots(task).is_some_and(|(_, complete)| complete),
                "kotlin-producer-java-recipe-unknown");
            None
        }
    } else { None };
    anyhow::ensure!(bindings.values().all(|outputs| !outputs.is_empty()
        && outputs.iter().all(|path| path.is_absolute())), "kotlin-producer-output-binding-unhandled");
    let replace = |p: &PathBuf| bindings.get(&key(p)).cloned().unwrap_or_else(|| vec![p.clone()]);
    let before: BTreeSet<_> = task.classpath.iter().map(|p| key(p)).collect();
    anyhow::ensure!(before.len() == task.classpath.len(), "kotlin-producer-duplicate-classpath-slot");
    task.classpath = task.classpath.iter().flat_map(replace).collect();
    task.declared_input_files = task.declared_input_files.iter().flat_map(replace).collect();
    let existing: Vec<_> = task.classpath.iter().filter(|p| p.exists()).collect();
    let joined = std::env::join_paths(existing)?.to_str()
        .ok_or_else(|| anyhow::anyhow!("kotlin-producer-classpath-encoding"))?.to_owned();
    for i in 0..task.arguments.len().saturating_sub(1) {
        if matches!(task.arguments[i].as_str(), "-classpath" | "-cp") {
            task.arguments[i + 1] = joined.clone();
        }
    }
    if let Some((mut recipe, classpath)) = java_recipe {
        recipe.api_options[classpath] = crate::classpath::joined_java_api_classpath(&task.classpath)?;
        task.options.insert("javaCompilerRecipe".into(), serde_json::to_value(recipe)?);
    }
    Ok(())
}

pub(super) fn effective(
    original: &KotlinDiagnosticRequest, bindings: &BTreeMap<String, Vec<PathBuf>>,
    covered: &BTreeSet<String>,
) -> anyhow::Result<KotlinDiagnosticRequest> {
    // Validate before replacing any selected slots; rewriting must not normalize away an
    // unsupported argument or mismatched captured classpath.
    kotlin_editor_arguments(original, Path::new("unused-private-output"), &[])?;
    let mut result = original.clone();
    rewrite(&mut result.task, bindings)?;
    if let Some(java) = &mut result.java_task { rewrite(java, bindings)?; }
    result.dependency_overlays.retain(|s| !covered.contains(&s.key));
    result.kotlin_producers.clear();
    Ok(result)
}

struct CompiledProducer {
    environment: Arc<KotlinDiagnosticEnvironment>,
    outputs: sealed_output::SealedOutput,
    _owner: KotlinEditorOverlay,
    bindings: Vec<(String, Vec<PathBuf>)>,
    source_keys: BTreeSet<String>,
}

pub(super) struct CompiledProducers {
    identity: Vec<u8>,
    nodes: Vec<CompiledProducer>,
    consumer_environment: std::sync::Mutex<Option<Arc<KotlinDiagnosticEnvironment>>>,
}

impl CompiledProducers {
    fn consumer_environment(
        &self, request: &KotlinDiagnosticRequest, parent: &KotlinDiagnosticEnvironment,
    ) -> anyhow::Result<Arc<KotlinDiagnosticEnvironment>> {
        let mut retained = self.consumer_environment.lock()
            .map_err(|_| anyhow::anyhow!("kotlin-producer-consumer-cache-poisoned"))?;
        if let Some(environment) = retained.as_ref() {
            anyhow::ensure!(environment.matches(request), "kotlin-producer-consumer-inputs-changed");
            environment.check_live()?;
            return Ok(environment.clone());
        }
        let environment = KotlinDiagnosticEnvironment::prepare_with_invalidation(
            request, parent.wake.clone(), parent.invalid.clone())?;
        // Dropping this watch on a Kotlin-only return would cancel the parent's shared input epoch.
        *retained = Some(environment.clone());
        Ok(environment)
    }

    pub(super) fn epochs_live(&self) -> bool {
        self.nodes.iter().all(|node|
            !node.environment.invalid.load(std::sync::atomic::Ordering::Acquire)
                && node.outputs.epoch_live())
    }

    pub(super) fn check_live(&self) -> anyhow::Result<()> {
        for node in &self.nodes {
            node.environment.check_live()?;
            node.outputs.check_live()?;
        }
        Ok(())
    }

    fn bindings(&self) -> BTreeMap<String, Vec<PathBuf>> {
        self.nodes.iter().flat_map(|node| node.bindings.iter().cloned()).collect()
    }

    fn covered(&self) -> BTreeSet<String> {
        self.nodes.iter().flat_map(|node| node.source_keys.iter().cloned()).collect()
    }

    pub(super) fn trace(&self) -> serde_json::Value {
        serde_json::json!({"basis":"current-normal-kotlin-producer-outputs",
            "producer_count":self.nodes.len(), "identity_sha256":format!("{:x}", Sha256::digest(&self.identity)),
            "output_slots":self.nodes.iter().map(|n| n.bindings.len()).sum::<usize>()})
    }
}

impl Drop for CompiledProducers {
    fn drop(&mut self) {
        if let Some(environment) = self.consumer_environment.get_mut()
            .unwrap_or_else(|error| error.into_inner()).as_ref() {
            environment.close();
        }
        // All dependent watches retire before any earlier producer's private output is deleted.
        for node in &self.nodes { node.environment.close(); }
    }
}

pub(super) fn identity(request: &KotlinDiagnosticRequest) -> anyhow::Result<Vec<u8>> {
    let mut sources = BTreeMap::new();
    for plan in &request.kotlin_producers {
        let producer = plan.request(request)?;
        for path in &producer.sources {
            request.check_live()?;
            let source_key = crate::paths::file_key(path);
            let digest: [u8; 32] = if let Some(source) = producer.overlays.iter().find(|s| s.key == source_key) {
                source.text_sha256
            } else {
                Sha256::digest(std::fs::read(path)?).into()
            };
            anyhow::ensure!(sources.insert(source_key, digest).is_none(), "kotlin-producer-shared-source-owner");
        }
    }
    Ok(serde_json::to_vec(&(&request.kotlin_producers, sources))?)
}

pub(super) fn output_bindings(
    request: &KotlinDiagnosticRequest, plan: &KotlinDependencyCompilation,
    kotlin_output: &Path, java_output: &Path,
) -> anyhow::Result<Vec<(String, Vec<PathBuf>)>> {
    let (kt, java) = tasks(&plan.source).ok_or_else(|| anyhow::anyhow!("kotlin-producer-recipe-unhandled"))?;
    let mut bindings = vec![(key(&kt.output_roots[0]), vec![kotlin_output.to_path_buf()]),
        (key(&java.output_roots[0]), vec![java_output.to_path_buf()])];
    // Only artifact slots actually selected by this closed plan are rebound. Capture
    // already proved their unique source owner; sources/artifact metadata stay unchanged.
    let reads: BTreeSet<_> = request.task.classpath.iter()
        .chain(request.java_task.iter().flat_map(|t| &t.classpath))
        .chain(request.kotlin_producers.iter().flat_map(|p|
            p.source.compiler_tasks.iter().flat_map(|t| &t.classpath)))
        .map(|p| key(p)).collect();
    bindings.extend(plan.source.provided_artifacts.iter()
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jar") && reads.contains(&key(p)))
        .map(|p| (key(p), vec![kotlin_output.to_path_buf(), java_output.to_path_buf()])));
    Ok(bindings)
}

fn compile(
    request: &KotlinDiagnosticRequest, environment: &KotlinDiagnosticEnvironment,
    expected_identity: Vec<u8>,
) -> anyhow::Result<Arc<CompiledProducers>> {
    let mut compiled = CompiledProducers { identity: expected_identity, nodes: Vec::new(),
        consumer_environment: std::sync::Mutex::new(None) };
    for plan in &request.kotlin_producers {
        request.check_live()?;
        environment.check_live()?;
        compiled.check_live()?;
        let producer = effective(&plan.request(request)?, &compiled.bindings(), &compiled.covered())?;
        let (kt, java) = tasks(&plan.source).ok_or_else(|| anyhow::anyhow!("kotlin-producer-recipe-unhandled"))?;
        let mixed = !java.source_files.is_empty();
        let producer_environment = KotlinDiagnosticEnvironment::prepare_with_invalidation(
            &producer, environment.wake.clone(), environment.invalid.clone())?;
        let roots = java_diagnostics::owned_membership_inputs(&producer)?;
        let read_plan = crate::compiled_watch::WatchPlan::read_inputs(&roots)?;
        producer_environment.watch.extend(read_plan.clone())?;
        java_diagnostics::owned_source_membership(&producer, &read_plan)?;
        // Mixed sources use the existing required-clean Javac replay over normal Kotlin outputs.
        let mut execution = producer.clone();
        if !mixed { execution.java_task = None; }
        let started = Instant::now();
        let (outcome, owner) = run_owned_kotlin_overlay(&execution, &producer_environment, mixed)?;
        anyhow::ensure!(matches!(outcome, KotlinDiagnosticOutcome::Verified { clean:true, ref diagnostics, .. }
            if diagnostics.iter().all(|d| d.severity != Severity::Error)), "kotlin-producer-compilation-not-clean");
        java_diagnostics::owned_source_membership(&producer, &read_plan)?;
        let kotlin_output = owner.directory.join("classes");
        anyhow::ensure!(kotlin_output.is_dir(), "kotlin-producer-normal-output-missing");
        let java_output = owner.directory.join("java-classes");
        if mixed {
            let entries = walkdir::WalkDir::new(&java_output).into_iter().collect::<Result<Vec<_>, _>>()?;
            anyhow::ensure!(java_output.is_dir() && entries.iter().any(|entry| entry.file_type().is_file()
                    && entry.path().extension().and_then(|extension| extension.to_str()) == Some("class")),
                "kotlin-producer-java-normal-output-missing");
        } else {
            std::fs::create_dir(&java_output)?;
        }
        let outputs = sealed_output::SealedOutput::seal(vec![kotlin_output.clone(), java_output.clone()])?;
        let bindings = output_bindings(request, plan, &kotlin_output, &java_output)?;
        compiled.nodes.push(CompiledProducer {
            _owner: owner, environment: producer_environment, outputs,
            bindings,
            source_keys: producer.sources.iter().map(|p| crate::paths::file_key(p)).collect(),
        });
        crate::trace::span("compiler.kotlin.producer-compiled", "compiler", started,
            serde_json::json!({"task":kt.task,"source_count":producer.sources.len(),
                "output_slots":2,"normal_output":true,"java_complete_zero":!mixed,
                "java_required_clean":mixed,"generation":request.generation}));
    }
    anyhow::ensure!(identity(request)? == compiled.identity, "kotlin-producer-source-changed-during-compile");
    compiled.check_live()?;
    Ok(Arc::new(compiled))
}

pub(super) fn run(
    request: &KotlinDiagnosticRequest, environment: &KotlinDiagnosticEnvironment,
) -> anyhow::Result<KotlinDiagnosticOutcome> {
    anyhow::ensure!(!request.kotlin_producers.is_empty() && request.kotlin_producers.len() <= MAX_PRODUCERS
        && request.dependency_compilation.is_none(), "kotlin-producer-closure-role-unhandled");
    let current = identity(request)?;
    let mut cache = environment.kotlin_producer_cache.lock()
        .map_err(|_| anyhow::anyhow!("kotlin-producer-cache-poisoned"))?;
    let producers = if let Some(cached) = cache.as_ref() {
        if cached.identity != current || cached.check_live().is_err() {
            // Replacing child watches inside this epoch would cancel its newly compiled replacement.
            environment.invalid.store(true, std::sync::atomic::Ordering::Release);
            let _ = environment.wake.try_send(());
            anyhow::bail!("kotlin-producer-inputs-changed");
        }
        cached.clone()
    } else {
        let compiled = compile(request, environment, current)?;
        *cache = Some(compiled.clone());
        compiled
    };
    let effective = effective(request, &producers.bindings(), &producers.covered())?;
    anyhow::ensure!(effective.dependency_overlays.iter().all(|s|
        std::fs::read_to_string(&s.key).ok().as_deref() == Some(s.text.as_str())),
        "kotlin-producer-unclosed-wire-dependency");
    let consumer_environment = producers.consumer_environment(&effective, environment)?;
    let (mut outcome, _owner) = run_owned_kotlin_overlay(&effective, &consumer_environment, false)?;
    request.check_live()?;
    environment.check_live()?;
    producers.check_live()?;
    anyhow::ensure!(identity(request)? == producers.identity, "kotlin-producer-source-changed-during-consumer");
    if let KotlinDiagnosticOutcome::Verified { java_clean: Some(proof), .. } = &mut outcome {
        proof.input_epoch = environment.invalid.clone();
        proof.body_environment = Some(consumer_environment);
        proof.producer_outputs = Some(producers.clone());
    }
    Ok(outcome)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn producer_consumer_watch_survives_kotlin_only_return_and_reuses_same_inputs() {
        let base = std::env::temp_dir().join("ktlsp-producer-consumer-watch");
        let owner = KotlinEditorOverlay::empty_in(&kotlin_test_request(&base.join("unused")), &base.join("runs")).unwrap();
        let mut request = kotlin_test_request(&owner.directory.join("project"));
        request.task.arguments.retain(|argument| !argument.starts_with("-Xplugin="));
        let runtime = request.task.kotlin_compiler.as_ref().unwrap();
        let sdk = runtime.gradle_java_home.clone();
        // No JVM runs: these are only watched input bytes for the existing environment boundary.
        for directory in [sdk.join("bin"), sdk.join("conf")] { std::fs::create_dir_all(directory).unwrap(); }
        let inputs = runtime.default_classpath.iter().chain(&request.task.classpath).cloned()
            .chain([sdk.join("release"), sdk.join("lib/modules"), sdk.join("lib/jvm.cfg"), sdk.join("lib/jrt-fs.jar")]);
        for input in inputs {
            std::fs::create_dir_all(input.parent().unwrap()).unwrap();
            std::fs::write(input, b"watched input").unwrap();
        }
        let selected_library = request.task.classpath[0].clone();
        let (wake, mut events) = tokio::sync::mpsc::channel(1);
        let parent = KotlinDiagnosticEnvironment::prepare(&request, wake).unwrap();
        let compiled = CompiledProducers { identity:Vec::new(), nodes:Vec::new(),
            consumer_environment:std::sync::Mutex::new(None) };
        let first = compiled.consumer_environment(&request, &parent).unwrap();
        let identity = Arc::as_ptr(&first);
        drop(first);
        assert!(!parent.invalid.load(Ordering::Acquire), "returning a Kotlin-only result must retain its input watch");
        assert!(matches!(events.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty)),
            "normal return must not enqueue a new diagnostic generation");
        let second = compiled.consumer_environment(&request, &parent).unwrap();
        assert_eq!(Arc::as_ptr(&second), identity, "same inputs reuse the retained consumer watch");
        drop(second);
        std::fs::write(selected_library, b"actual selected library edit").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut woken = false;
        while !woken && Instant::now() < deadline {
            woken = events.try_recv().is_ok();
            if !woken { std::thread::sleep(Duration::from_millis(10)); }
        }
        assert!(woken, "real selected input changes must still wake the diagnostic owner");
        assert!(parent.invalid.load(Ordering::Acquire), "real selected input changes still invalidate the shared epoch");
        assert!(parent.check_live().is_err());
        parent.close();
        drop(compiled);
    }
}
