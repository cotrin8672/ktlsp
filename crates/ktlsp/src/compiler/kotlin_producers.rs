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
    // Nonempty Java producer bodies require their own replay. This slice proves the actual
    // complete zero-Java inventory instead of inheriting any saved Java output directory.
    if !kotlin.complete || kotlin.problem.is_some() || !java.complete || java.problem.is_some()
        || kotlin.source_files.is_empty() || !java.source_files.is_empty()
        || kotlin.source_files.iter().any(|p| p.extension().and_then(|e| e.to_str()) != Some("kt"))
        || !crate::classpath::sdk_producer_roots(kotlin).is_some_and(|(_, complete)| complete)
        || !crate::classpath::sdk_producer_roots(java).is_some_and(|(_, complete)| complete)
        || kotlin.output_roots.len() != 1 || java.output_roots.len() != 1
        || !java_diagnostics::selected_tasks_supported(kotlin, java, &source.compiler_tasks)
        || !java.annotation_processing_disabled() || !java.annotation_processor_path.is_empty()
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
                    if primary_read && primary.output_roots.iter().chain(&java.output_roots)
                        .any(|output| paths_overlap(&read_key, &key(output))) {
                        return rejected(&primary.task, "primary-reads-own-normal-output");
                    }
                }
                [(index, (_, source))] => {
                    let (kt, j) = tasks(source)?;
                    if !kt.output_roots.iter().chain(&j.output_roots).any(|p| key(p) == read_key) {
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

fn rewrite(task: &mut CompilerTask, bindings: &BTreeMap<String, PathBuf>) -> anyhow::Result<()> {
    let java_recipe = if task.language == CompilerLanguage::Java {
        let recipe = task.java_compiler_recipe().ok_or_else(|| anyhow::anyhow!("kotlin-producer-java-recipe-unknown"))?;
        let slots = task.java_api_option_slots(&recipe)?;
        Some((recipe, slots.classpath))
    } else { None };
    let replace = |p: &PathBuf| bindings.get(&key(p)).cloned().unwrap_or_else(|| p.clone());
    let before: BTreeSet<_> = task.classpath.iter().map(|p| key(p)).collect();
    anyhow::ensure!(before.len() == task.classpath.len(), "kotlin-producer-duplicate-classpath-slot");
    task.classpath = task.classpath.iter().map(replace).collect();
    task.declared_input_files = task.declared_input_files.iter().map(replace).collect();
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
    original: &KotlinDiagnosticRequest, bindings: &BTreeMap<String, PathBuf>,
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
    _owner: KotlinEditorOverlay,
    environment: Arc<KotlinDiagnosticEnvironment>,
    outputs: sealed_output::SealedOutput,
    bindings: Vec<(String, PathBuf)>,
    source_keys: BTreeSet<String>,
}

pub(super) struct CompiledProducers {
    identity: Vec<u8>,
    nodes: Vec<CompiledProducer>,
}

impl CompiledProducers {
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

    fn bindings(&self) -> BTreeMap<String, PathBuf> {
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

fn compile(
    request: &KotlinDiagnosticRequest, environment: &KotlinDiagnosticEnvironment,
    expected_identity: Vec<u8>,
) -> anyhow::Result<Arc<CompiledProducers>> {
    let mut compiled = CompiledProducers { identity: expected_identity, nodes: Vec::new() };
    for plan in &request.kotlin_producers {
        request.check_live()?;
        environment.check_live()?;
        compiled.check_live()?;
        let producer = effective(&plan.request(request)?, &compiled.bindings(), &compiled.covered())?;
        // Recheck complete capture before treating an empty Java slot as empty API authority.
        let (kt, java) = tasks(&plan.source).ok_or_else(|| anyhow::anyhow!("kotlin-producer-recipe-unhandled"))?;
        let producer_environment = KotlinDiagnosticEnvironment::prepare_with_invalidation(
            &producer, environment.wake.clone(), environment.invalid.clone())?;
        let roots = java_diagnostics::owned_membership_inputs(&producer)?;
        let read_plan = crate::compiled_watch::WatchPlan::read_inputs(&roots)?;
        producer_environment.watch.extend(read_plan.clone())?;
        java_diagnostics::owned_source_membership(&producer, &read_plan)?;
        // The normal compiler, not signature generation, supplies the current Kotlin ABI.
        // All current selected sources are Kotlin; the independently checked Java inventory
        // above is empty, so no no-source javac invocation is used as a cleanliness shortcut.
        let mut kotlin_only = producer.clone();
        kotlin_only.java_task = None;
        let started = Instant::now();
        let (outcome, owner) = run_owned_kotlin_overlay(&kotlin_only, &producer_environment, false)?;
        anyhow::ensure!(matches!(outcome, KotlinDiagnosticOutcome::Verified { clean:true, ref diagnostics, .. }
            if diagnostics.iter().all(|d| d.severity != Severity::Error)), "kotlin-producer-compilation-not-clean");
        java_diagnostics::owned_source_membership(&producer, &read_plan)?;
        let kotlin_output = owner.directory.join("classes");
        anyhow::ensure!(kotlin_output.is_dir(), "kotlin-producer-normal-output-missing");
        let java_output = owner.directory.join("java-classes");
        std::fs::create_dir(&java_output)?;
        let outputs = sealed_output::SealedOutput::seal(vec![kotlin_output.clone(), java_output.clone()])?;
        compiled.nodes.push(CompiledProducer {
            _owner: owner, environment: producer_environment, outputs,
            bindings: vec![(key(&kt.output_roots[0]), kotlin_output), (key(&java.output_roots[0]), java_output)],
            source_keys: producer.sources.iter().map(|p| crate::paths::file_key(p)).collect(),
        });
        crate::trace::span("compiler.kotlin.producer-compiled", "compiler", started,
            serde_json::json!({"task":kt.task,"source_count":producer.sources.len(),
                "output_slots":2,"normal_output":true,"java_complete_zero":true,"generation":request.generation}));
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
    let cached = cache.as_ref().filter(|c| c.identity == current && c.check_live().is_ok()).cloned();
    let producers = if let Some(cached) = cached { cached }
        else { compile(request, environment, current)? };
    let effective = effective(request, &producers.bindings(), &producers.covered())?;
    anyhow::ensure!(effective.dependency_overlays.iter().all(|s|
        std::fs::read_to_string(&s.key).ok().as_deref() == Some(s.text.as_str())),
        "kotlin-producer-unclosed-wire-dependency");
    let consumer_environment = KotlinDiagnosticEnvironment::prepare_with_invalidation(
        &effective, environment.wake.clone(), environment.invalid.clone())?;
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
    *cache = Some(producers);
    Ok(outcome)
}
