//! Host adapter for shared contributions. Framework policy lives in registered consumers.

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use ktcore::extensions::{ContributionSet, ExtensionStore, IncompleteReason};
use ktcore::index::{Index, InferenceIndex};
use ktcore::knowledge::Knowledge;
use tree_sitter::Tree;

mod ipc;
mod source_hydration;
pub(crate) use source_hydration::{prepare_sources, PreparedSource, SourceRequest};

pub use ktcore::extensions::AnalysisProvider;

#[derive(Default)]
pub(crate) struct AnalysisExtensions {
    providers: BTreeMap<String, Arc<dyn AnalysisProvider>>,
    ipc_providers: BTreeMap<String, ipc::Slot>,
    ipc_evaluations: BTreeMap<u64, crate::extension_ipc::Evaluation>,
    inputs: BTreeMap<String, SourceInput>,
    retain_all_sources: bool,
    compiled_enabled: bool,
    compiled_receipt: Option<Arc<crate::compile::SuccessfulCompileReceipt>>,
    compiler_api_sources: BTreeSet<String>,
    store: ExtensionStore,
    changed: BTreeSet<String>,
    suspended: bool,
    pending_index: bool,
    published_revision: u64,
    source_epoch: u64,
    library_sources: BTreeMap<
        String,
        (
            ktcore::project_model::FileOrigin,
            Arc<crate::deps::LibrarySource>,
        ),
    >,
    attempted_sources: BTreeMap<String, source_hydration::SourceStamp>,
    hydrated_sources: BTreeMap<String, source_hydration::SourceStamp>,
    source_producers: BTreeMap<String, source_hydration::SourceProducer>,
}

impl AnalysisExtensions {
    pub fn new(providers: Vec<Arc<dyn AnalysisProvider>>) -> Self {
        let retain_all_sources = !providers.is_empty();
        let mut providers = providers;
        providers.push(Arc::new(CompiledJavaProvider));
        let mut result = Self {
            retain_all_sources,
            compiled_enabled: true,
            ..Self::default()
        };
        for provider in providers {
            let id = provider.id().to_string();
            assert!(
                !id.is_empty() && !result.providers.contains_key(&id),
                "duplicate or empty analysis provider id"
            );
            result.store.configure_provider(&id, true, Arc::from("{}"));
            result.providers.insert(id, provider);
        }
        result
    }

    pub fn is_empty(&self) -> bool {
        !self.retain_all_sources
            && self.compiled_receipt.is_none()
            && self.compiler_api_sources.is_empty()
    }

    pub fn has_framework_providers(&self) -> bool {
        self.retain_all_sources
    }

    pub fn configure(
        &mut self,
        index: &dyn InferenceIndex,
        id: &str,
        enabled: bool,
        config: Arc<str>,
    ) -> bool {
        if !self.providers.contains_key(id) && !self.ipc_providers.contains_key(id) {
            return false;
        }
        self.configure_ipc(id, enabled, &config);
        if id == COMPILED_JAVA_PROVIDER {
            self.clear_compiled();
            self.clear_compiler_api(index);
            self.compiled_enabled = enabled;
        }
        self.changed.extend(
            self.store
                .contribution_sets()
                .filter(|(provider, _, _)| *provider == id)
                .map(|(_, owner, _)| owner.to_string()),
        );
        self.store.configure_provider(id, enabled, config);
        self.evaluate(index);
        true
    }

    pub fn replace_sources(
        &mut self,
        index: &dyn InferenceIndex,
        sources: Vec<(String, Arc<str>, Tree)>,
    ) {
        if self.is_empty() || sources.is_empty() {
            return;
        }
        if sources.iter().any(|(key, text, tree)| {
            self.compiler_api_sources.contains(key)
                && self
                    .inputs
                    .get(key)
                    .is_none_or(|input| !input.compiler_source_matches(text, tree))
        }) {
            self.clear_compiler_api(index);
        }
        if self.compiled_receipt.as_ref().is_some_and(|receipt| {
            sources.iter().any(|(key, text, _)| {
                receipt.observes_path(key) && !receipt.source_matches(key, text)
            })
        }) {
            if let Some(receipt) = &self.compiled_receipt {
                receipt
                    .request
                    .invalid
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            self.clear_compiled();
        }
        let namespace = self.store.namespace_revision();
        let keys: Vec<_> = sources.iter().map(|(key, _, _)| key.clone()).collect();
        for (key, text, tree) in sources {
            self.hydrated_sources.remove(&key);
            self.forget_source_producer(&key);
            self.changed.insert(key.clone());
            // A host source replacement cannot transfer a prior build's binary authority.
            let previous = self
                .inputs
                .remove(&key)
                .filter(|input| input.compiler_source_matches(&text, &tree));
            let (compiled, compiler_classes) = previous
                .map(|input| (input.compiled, input.compiler_classes))
                .unwrap_or_default();
            self.inputs.insert(
                key,
                SourceInput {
                    text,
                    tree,
                    compiled,
                    compiler_classes,
                    prepared_references: None,
                },
            );
        }
        for key in keys {
            let _ = self.normalize(index, &key);
        }
        // Type visibility changes can change an otherwise untouched source signature.
        if self.store.namespace_revision() != namespace {
            self.normalize_all(index);
        }
        self.evaluate(index);
    }

    pub fn remove_source(&mut self, index: &dyn InferenceIndex, key: &str) {
        if self.compiler_api_sources.contains(key) {
            self.clear_compiler_api(index);
        }
        if self
            .compiled_receipt
            .as_ref()
            .is_some_and(|receipt| receipt.observes_path(key))
        {
            if let Some(receipt) = &self.compiled_receipt {
                receipt
                    .request
                    .invalid
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            self.clear_compiled();
        }
        self.inputs.remove(key);
        self.hydrated_sources.remove(key);
        self.attempted_sources.remove(key);
        self.library_sources.remove(key);
        self.forget_source_producer(key);
        let namespace = self.store.namespace_revision();
        self.store.remove_source(key);
        self.changed.insert(key.into());
        if self.store.namespace_revision() != namespace {
            self.normalize_all(index);
        }
        self.evaluate(index);
    }

    pub fn suspend(&mut self) {
        if self.is_empty() {
            return;
        }
        self.mark_published();
        self.suspended = true;
        self.invalidate_hydrated_sources();
        self.clear_compiled();
        self.compiler_api_sources.clear();
        self.store.invalidate_model();
    }

    pub fn model_changed(&mut self, index: &dyn InferenceIndex) {
        if self.is_empty() {
            return;
        }
        self.mark_published();
        self.invalidate_hydrated_sources();
        self.clear_compiled();
        self.clear_compiler_api(index);
        self.store.invalidate_model();
        self.normalize_all(index);
        self.evaluate(index);
    }

    pub fn mark_index_changed(&mut self) {
        if !self.is_empty() && !self.pending_index {
            self.mark_published();
            self.clear_compiled();
            self.store.invalidate_model();
            self.pending_index = true;
        }
    }

    pub fn flush_index_changed(&mut self, index: &dyn InferenceIndex) {
        if self.pending_index {
            self.pending_index = false;
            self.normalize_all(index);
            self.evaluate(index);
        }
    }

    pub fn resume(&mut self, index: &dyn InferenceIndex) {
        self.suspended = false;
        self.normalize_all(index);
        self.evaluate(index);
    }

    /// Retain the existing input allocation across a cancellable request, without copying text.
    pub(crate) fn source_input_identity(&self, key: &str) -> Option<Arc<str>> {
        self.inputs.get(key).map(|input| input.text.clone())
    }

    /// Native callbacks only wake the existing scheduler; publication and requeue
    /// happen under the ordinary host lock with atomic liveness checks only.
    pub(crate) fn refresh_host_inputs(&mut self)->bool {
        let changed=self.store.refresh_host_inputs();
        let any=!changed.is_empty();
        for(provider,owner) in changed {
            self.changed.insert(owner.clone());
            if let Some(slot)=self.ipc_providers.get_mut(&provider).filter(|slot|slot.enabled) {
                slot.queued.insert(owner);
            }
        }
        self.requeue_stale_ipc();any
    }

    pub fn revision(&self) -> u64 {
        self.store.publication_revision()
    }

    pub fn publish(&mut self, index: &mut Index) -> bool {
        self.refresh_host_inputs();
        if self.published_revision == self.revision() {
            return false;
        }
        let mut sets: BTreeMap<String, Vec<ContributionSet>> = BTreeMap::new();
        for (_, owner, set) in self.store.contribution_sets() {
            sets.entry(owner.into()).or_default().push(set.clone());
        }
        index.replace_contributions(sets);
        self.published_revision = self.revision();
        true
    }

    pub fn sets(&self) -> impl Iterator<Item = (&str, &ContributionSet)> {
        self.store
            .contribution_sets()
            .map(|(_, owner, set)| (owner, set))
    }

    pub fn take_changed(&mut self) -> Vec<String> {
        std::mem::take(&mut self.changed).into_iter().collect()
    }

    fn mark_published(&mut self) {
        self.changed.extend(
            self.store
                .contribution_sets()
                .map(|(_, owner, _)| owner.to_string()),
        );
    }

    fn normalize(&mut self, index: &dyn InferenceIndex, key: &str) -> Result<(), String> {
        let Some(input) = self.inputs.get(key) else {
            return Ok(());
        };
        let facts = crate::language::extension_source_facts(index, key, &input.tree, &input.text);
        let coverage = Knowledge::Unknown(vec![if crate::language::SourceLanguage::for_project_path(std::path::Path::new(key)).is_some() { IncompleteReason::IncompleteTarget } else { IncompleteReason::UnsupportedSource }]);
        let prepared = input.prepared_references.clone().unwrap_or_else(|| {
            ktcore::extensions::source::prepare_source_references(
                index,
                key,
                input.text.clone(),
                &input.tree,
            )
        });
        let replaced = self
            .store
            .replace_prepared_source_declarations(index, prepared, coverage, facts);
        if let Err(error) = replaced {
            // Accepted source/index disagreement must remove old results, never reuse their spans.
            self.store.remove_source(key);
            tracing::warn!(file = key, ?error, "invalid normalized extension source");
            return Err(format!("invalid normalized extension source: {error:?}"));
        }
        let result = self.attach_compiled(index, key);
        if let Err(error) = &result {
            tracing::warn!(file = key, %error, "invalid compiled source owner");
        }
        result
    }

    fn normalize_all(&mut self, index: &dyn InferenceIndex) {
        if self.is_empty() {
            return;
        }
        for key in self.inputs.keys().cloned().collect::<Vec<_>>() {
            let _ = self.normalize(index, &key);
        }
    }

    fn evaluate(&mut self, index: &dyn InferenceIndex) {
        if self.suspended {
            return;
        }
        self.requeue_stale_ipc();
        for (provider, owner) in self.store.take_dirty() {
            if let Some(slot) = self.ipc_providers.get_mut(&provider) {
                if slot.enabled {
                    slot.queued.insert(owner);
                }
                continue;
            }
            if provider == COMPILED_JAVA_PROVIDER
                && !self.inputs.get(&owner).is_some_and(|input| {
                    input.compiled.is_some() || !input.compiler_classes.is_empty()
                })
            {
                continue;
            }
            let Some(provider) = self.providers.get(&provider) else {
                continue;
            };
            let Ok(mut read) = self.store.begin(index, provider.id(), &owner) else {
                continue;
            };
            let result = catch_unwind(AssertUnwindSafe(|| provider.analyze(&mut read)));
            let pending = read.finish();
            if matches!(result, Ok(Ok(()))) {
                if let Err(error) = self.store.commit(pending) {
                    tracing::warn!(
                        provider = provider.id(),
                        file = owner,
                        ?error,
                        "stale extension evaluation"
                    );
                }
            } else {
                let _ = self.store.fail(pending);
                tracing::warn!(
                    provider = provider.id(),
                    file = owner,
                    "analysis provider failed"
                );
            }
            self.changed.insert(owner);
        }
    }
}

pub(crate) const COMPILED_JAVA_PROVIDER: &str = "ktlsp.compiled-java";

struct CompiledJavaProvider;

impl AnalysisProvider for CompiledJavaProvider {
    fn id(&self) -> &str {
        COMPILED_JAVA_PROVIDER
    }

    fn analyze(
        &self,
        read: &mut ktcore::extensions::ReadSession<'_>,
    ) -> Result<(), ktcore::extensions::ContractError> {
        for declaration in read.declarations().to_vec() {
            if declaration.owner.is_some() || declaration.member.is_some() {
                continue;
            }
            let def = &declaration.definition;
            let Some(name) = read.source().get(def.start_byte..def.end_byte) else {
                continue;
            };
            let ty = ktcore::types::TypeRef::simple(name);
            let Knowledge::Found(owner) = read.resolve_type_ref(&ty) else {
                continue;
            };
            if owner.definition() != def {
                continue;
            }
            if let Knowledge::Found(class) = read.compiled_class(&owner)? {
                for method in read.compiled_methods(&class)? {
                    // The shared core validates language surface and descriptor; this provider cannot
                    // create a signature, source position or a framework-specific completion item.
                    let _ = read.admit_java_method(&method)?;
                }
            }
            for class in read.compiler_classes(&owner)? {
                read.admit_compiler_class(&class)?;
            }
        }
        Ok(())
    }
}

struct SourceInput {
    text: Arc<str>,
    tree: Tree,
    compiled: Option<Arc<ktcore::classfile::ClassFileFacts>>,
    compiler_classes: Vec<ktcore::extensions::CompilerClassInput>,
    prepared_references: Option<ktcore::extensions::source::PreparedSourceReferences>,
}

impl SourceInput {
    fn compiler_source_matches(&self, text: &str, tree: &Tree) -> bool {
        self.text.as_ref() == text
            || !self.compiler_classes.is_empty()
                && self.compiler_classes.iter().all(|input| {
                    input.value_namespace && input.kotlin_methods.is_empty() && !input.companion
                })
                && crate::java::declaration_header_text(&self.tree, &self.text).is_some_and(
                    |header| {
                        crate::java::declaration_header_text(tree, text).as_ref() == Some(&header)
                    },
                )
    }
}

#[cfg(test)]
mod compiler_source_tests {
    use super::*;

    #[test]
    fn valid_class_declarations_survive_an_unfinished_sibling_function() {
        let file = "/compiler-api/Model.kt";
        let source = include_str!("../../ktcore/tests/fixtures/classfile/serialization/Model.kt");
        for extension in [
            "",
            "fun Model.topLevelExtension() = this\nclass Owner { fun Model.memberExtension() = this }\n",
        ] {
            let edited = format!("{source}\n{extension}fun incompleteSameFile() = Model.\n");
            let mut workspace = crate::workspace::Workspace::new();
            workspace.open(file, source.into());
            workspace.change(file, edited.clone());
            let tree = crate::language::LanguageParsers::new().parse_for_key(file, &edited);
            let facts = crate::language::extension_source_facts(&workspace.index, file, &tree, &edited);
            assert!(facts.iter().any(|fact| !fact.annotations.is_empty()));
            for name in ["incompleteSameFile", "topLevelExtension", "memberExtension"] {
                if name != "incompleteSameFile" && extension.is_empty() {
                    continue;
                }
                assert!(facts.iter().any(|fact| {
                    edited.get(fact.definition.start_byte..fact.definition.end_byte) == Some(name)
                        && fact.member.is_none()
                }), "authored {name} declaration must survive without a class-member claim");
            }
            let mut store = ktcore::extensions::ExtensionStore::default();
            let result = store.replace_source_declarations(
                &workspace.index,
                file,
                Arc::from(edited),
                Knowledge::Unknown(vec![IncompleteReason::IncompleteTarget]),
                facts.clone(),
            );
            assert!(result.is_ok(), "{result:?}; {facts:?}");
        }
    }

    #[test]
    fn normalized_reference_eligibility_reuses_current_tree_after_index_and_source_changes() {
        struct Probe;
        impl AnalysisProvider for Probe {
            fn id(&self) -> &str {
                "reference-tree"
            }
            fn analyze(
                &self,
                _: &mut ktcore::extensions::ReadSession<'_>,
            ) -> Result<(), ktcore::extensions::ContractError> {
                Ok(())
            }
        }
        for (file, plain, generic) in [
            (
                "/tree/Plain.java",
                "package tree; class Plain {}",
                "package tree; class Plain<T> {}",
            ),
            (
                "/tree/Plain.kt",
                "package tree\nclass Plain",
                "package tree\nclass Plain<T>",
            ),
        ] {
            let mut index = Index::new();
            let mut parser = crate::language::LanguageParsers::new();
            let mut extensions = AnalysisExtensions::new(vec![Arc::new(Probe)]);
            for (text, expected) in [(plain, true), (generic, false), (plain, true)] {
                let tree = parser.parse_for_key(file, text);
                let language = crate::language::SourceLanguage::for_key(file);
                let facts = crate::language::symbol_facts(language, &tree, text);
                index.replace_file(file, facts.symbols, ktcore::index::Tier::Volatile);
                extensions.replace_sources(&index, vec![(file.into(), Arc::from(text), tree)]);
                // A dependency/index flush must preserve current source eligibility.
                extensions.mark_index_changed();
                extensions.flush_index_changed(&index);
                let mut read = extensions
                    .store
                    .begin(&index, "reference-tree", file)
                    .unwrap();
                let Knowledge::Found(owner) = read.resolve_type("tree.Plain") else {
                    panic!("current source identity");
                };
                assert_eq!(
                    read.reference_type(&owner).unwrap().is_found(),
                    expected,
                    "{file}: {text}"
                );
            }
        }
    }

    fn input(text: &str, value_namespace: bool) -> SourceInput {
        SourceInput {
            text: Arc::from(text),
            tree: crate::java::JavaParser::new().parse(text),
            compiled: None,
            compiler_classes: vec![ktcore::extensions::CompilerClassInput {
                class: Arc::new(
                    ktcore::classfile::decode(
                        "test",
                        include_bytes!(
                            "../../ktcore/tests/fixtures/classfile/serialization/Model.class"
                        ),
                    )
                    .unwrap(),
                ),
                kotlin_methods: Default::default(),
                companion: false,
                value_namespace,
            }],
            prepared_references: None,
        }
    }

    #[test]
    fn only_independently_bounded_namespace_observations_reuse_consumer_bodies() {
        let text = "package sample; class Use { void run() { Model.Companion.serializer(); } }";
        let partial = "package sample; class Use { void run() { Model.Companion.seri } }";
        let tree = crate::java::JavaParser::new().parse(partial);
        assert!(!input(text, false).compiler_source_matches(partial, &tree));
        assert!(input(text, true).compiler_source_matches(partial, &tree));
        for changed in [
            "package sample; class Use { Object Model; void run() { Model.Companion.seri } }",
            "package sample; class Use extends Base { void run() { Model.Companion.seri } }",
            "package other; class Use { void run() { Model.Companion.seri } }",
            "package sample; class Use { @Generated void run() { Model.Companion.seri } }",
        ] {
            assert!(
                !input(text, true).compiler_source_matches(
                    changed,
                    &crate::java::JavaParser::new().parse(changed)
                ),
                "{changed}"
            );
        }
        let mut generated = input(text, true);
        generated.compiler_classes[0]
            .kotlin_methods
            .insert("generated()V".into());
        assert!(!generated.compiler_source_matches(partial, &tree));
    }
}

impl AnalysisExtensions {
    /// Called before Arc/tree copies. The builtin retains only accepted compiled source owners;
    /// explicitly registered external providers retain the same source inputs as before.
    pub fn needs_source(&self, key: &str) -> bool {
        self.retain_all_sources || self.inputs.contains_key(key)
    }

    pub fn compiled_enabled(&self) -> bool {
        self.compiled_enabled
    }

    pub fn compiled_receipt(&self) -> Option<&Arc<crate::compile::SuccessfulCompileReceipt>> {
        self.compiled_receipt.as_ref()
    }

    pub fn invalidate_compiled(&mut self, index: &dyn InferenceIndex) {
        if self.clear_compiled() {
            self.evaluate(index);
        }
    }

    fn clear_compiled(&mut self) -> bool {
        if self.compiled_receipt.take().is_none() {
            return false;
        }
        self.store.clear_compiled_inputs();
        let compiled_keys: Vec<_> = self
            .inputs
            .iter_mut()
            .filter_map(|(key, input)| input.compiled.take().map(|_| key.clone()))
            .collect();
        for key in compiled_keys {
            self.changed.insert(key.clone());
            if !self.retain_all_sources {
                self.inputs.remove(&key);
                self.store.remove_source(&key);
            }
        }
        true
    }

    pub fn accept_compiled(
        &mut self,
        index: &dyn InferenceIndex,
        receipt: Arc<crate::compile::SuccessfulCompileReceipt>,
        owners: Vec<crate::compile::CompiledOwnerInput>,
    ) -> Result<(), String> {
        if !self.compiled_enabled || self.suspended {
            return Err("provider-disabled-or-suspended".into());
        }
        receipt
            .request
            .check_live()
            .map_err(|error| error.to_string())?;
        self.invalidate_compiled(index);
        self.compiled_receipt = Some(receipt);
        for owner in owners {
            let receipt = self.compiled_receipt.as_ref().unwrap();
            if let Err(error) = receipt.verify_compiled_output(
                &owner.class_file,
                owner.class_file_bytes,
                &owner.class,
            ) {
                self.invalidate_compiled(index);
                return Err(error.to_string());
            }
            let expected = receipt
                .request
                .owners
                .iter()
                .find(|expected| expected.definition.file == owner.file);
            let valid = expected.is_some_and(|expected| {
                owner.class.identity.origin_id == expected.origin_id
                    && owner.class.identity.internal_name
                        == expected.qualified_name.replace('.', "/")
                    && receipt.source_matches(&owner.file, &owner.text)
            });
            if !valid {
                self.invalidate_compiled(index);
                return Err("compiled-owner-or-source-mismatch".into());
            }
            self.changed.insert(owner.file.clone());
            self.inputs.insert(
                owner.file,
                SourceInput {
                    text: owner.text,
                    tree: owner.tree,
                    compiled: Some(owner.class),
                    compiler_classes: Vec::new(),
                    prepared_references: None,
                },
            );
        }
        // No intermediate publication: failure removes the whole attempted compiled set. Other
        // providers keep their independent validated contributions and configuration.
        let result = (|| {
            for key in self
                .inputs
                .iter()
                .filter(|(_, input)| input.compiled.is_some())
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>()
            {
                self.normalize(index, &key)?;
            }
            Ok(())
        })();
        if result.is_err() {
            self.invalidate_compiled(index);
        }
        self.evaluate(index);
        result
    }

    fn attach_compiled(&mut self, index: &dyn InferenceIndex, key: &str) -> Result<(), String> {
        if let Some(input) = self
            .inputs
            .get(key)
            .filter(|input| !input.compiler_classes.is_empty())
        {
            let mut read = self
                .store
                .begin(index, COMPILED_JAVA_PROVIDER, key)
                .map_err(|error| format!("{error:?}"))?;
            let roots: Vec<_> = input
                .compiler_classes
                .iter()
                .filter(|class| !class.class.identity.internal_name.contains('$'))
                .collect();
            let [root] = roots.as_slice() else {
                return Err("compiler-api-root-unknown".into());
            };
            let owner = read.resolve_type(&root.class.identity.internal_name.replace('/', "."));
            let pending = read.finish();
            self.store
                .commit(pending)
                .map_err(|error| format!("{error:?}"))?;
            let Knowledge::Found(owner) = owner else {
                return Err("compiler-api-owner-unknown".into());
            };
            self.store
                .replace_compiler_classes(index, &owner, input.compiler_classes.clone())
                .map_err(|error| format!("{error:?}"))?;
        }
        let Some(class) = self
            .inputs
            .get(key)
            .and_then(|input| input.compiled.clone())
        else {
            return Ok(());
        };
        let mut read = self
            .store
            .begin(index, COMPILED_JAVA_PROVIDER, key)
            .map_err(|error| format!("{error:?}"))?;
        let qualified = class.identity.internal_name.replace('/', ".");
        let resolved = read.resolve_type(&qualified);
        let pending = read.finish();
        self.store
            .commit(pending)
            .map_err(|error| format!("{error:?}"))?;
        let Knowledge::Found(owner) = resolved else {
            return Err("current-source-owner-unknown".into());
        };
        self.store
            .replace_compiled_input(index, &owner, Some(class))
            .map_err(|error| format!("{error:?}"))
    }
}

impl AnalysisExtensions {
    pub fn clear_compiler_api(&mut self, index: &dyn InferenceIndex) {
        if self.compiler_api_sources.is_empty() {
            return;
        }
        self.store.clear_compiled_inputs();
        for key in std::mem::take(&mut self.compiler_api_sources) {
            self.changed.insert(key.clone());
            if self.retain_all_sources {
                if let Some(input) = self.inputs.get_mut(&key) {
                    input.compiler_classes.clear();
                    input.compiled = None;
                }
            } else {
                self.inputs.remove(&key);
                self.store.remove_source(&key);
            }
        }
        self.evaluate(index);
    }

    pub fn accept_compiler_api(
        &mut self,
        index: &dyn InferenceIndex,
        snapshot: crate::compile::CompilerApiSnapshot,
    ) -> Result<(), String> {
        if !self.compiled_enabled || self.suspended {
            return Err("compiler-api-provider-disabled".into());
        }
        let mut parsers = crate::language::LanguageParsers::new();
        let result = (|| {
            for (key, mut classes) in snapshot.classes {
                if !classes
                    .iter()
                    .any(|class| !class.kotlin_methods.is_empty() || class.value_namespace)
                {
                    continue;
                }
                let text = snapshot
                    .sources
                    .get(&key)
                    .ok_or("compiler-api-source-missing")?
                    .clone();
                let tree = parsers.parse_for_key(&key, &text);
                let origin = index
                    .origin_facts()
                    .and_then(|facts| facts.file_origin(&key))
                    .ok_or("compiler-api-origin-unknown")?;
                for class in &mut classes {
                    Arc::make_mut(&mut class.class).identity.origin_id = origin.origin_id.clone();
                }
                self.compiler_api_sources.insert(key.clone());
                self.changed.insert(key.clone());
                self.inputs.insert(
                    key.clone(),
                    SourceInput {
                        text,
                        tree,
                        compiled: None,
                        compiler_classes: classes,
                        prepared_references: None,
                    },
                );
                self.normalize(index, &key)?;
            }
            Ok(())
        })();
        if result.is_err() {
            self.clear_compiler_api(index);
        }
        self.evaluate(index);
        result
    }

    pub(crate) fn withdraw_project_outputs(
        &mut self,
        index: &dyn InferenceIndex,
        keys: impl IntoIterator<Item = String>,
    ) {
        for key in keys {
            let Some(input) = self.inputs.get_mut(&key) else { continue };
            if input.compiled.take().is_none() { continue; }
            self.compiler_api_sources.remove(&key);
            self.changed.insert(key.clone());
            self.store.remove_source(&key);
            if self.retain_all_sources {
                let _ = self.normalize(index, &key);
            } else {
                self.inputs.remove(&key);
            }
        }
        self.evaluate(index);
    }

    /// Selected externally built binary APIs are positive facts, not compiler-clean authority.
    /// The host binds source/model/output identities before reaching this shared provider.
    pub(crate) fn accept_project_outputs(
        &mut self,
        index: &dyn InferenceIndex,
        owners: Vec<(String, Arc<str>, Tree, Arc<ktcore::classfile::ClassFileFacts>)>,
    ) -> Result<(), String> {
        if !self.compiled_enabled || self.suspended {
            return Err("project-output-provider-disabled".into());
        }
        let mut keys = Vec::new();
        for (key, text, tree, class) in owners {
            self.compiler_api_sources.insert(key.clone());
            self.changed.insert(key.clone());
            keys.push(key.clone());
            self.inputs.insert(key, SourceInput {
                text, tree, compiled: Some(class), compiler_classes: Vec::new(), prepared_references: None,
            });
        }
        for key in keys {
            if let Err(error) = self.normalize(index, &key) {
                self.clear_compiler_api(index);
                return Err(error);
            }
        }
        self.evaluate(index);
        Ok(())
    }
}
