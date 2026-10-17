//! Host adapter for shared contributions. Framework policy lives in registered consumers.

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use ktcore::extensions::{ContributionSet, ExtensionStore, IncompleteReason};
use ktcore::index::{Index, InferenceIndex};
use ktcore::knowledge::Knowledge;
use tree_sitter::Tree;

mod source_hydration;
pub(crate) use source_hydration::{prepare_sources, PreparedSource, SourceRequest};

pub use ktcore::extensions::AnalysisProvider;

#[derive(Default)]
pub(crate) struct AnalysisExtensions {
    providers: BTreeMap<String, Arc<dyn AnalysisProvider>>,
    inputs: BTreeMap<String, SourceInput>,
    retain_all_sources: bool,
    compiled_enabled: bool,
    compiled_receipt: Option<Arc<crate::compile::SuccessfulCompileReceipt>>,
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
        !self.retain_all_sources && self.compiled_receipt.is_none()
    }

    pub fn configure(
        &mut self,
        index: &dyn InferenceIndex,
        id: &str,
        enabled: bool,
        config: Arc<str>,
    ) -> bool {
        if !self.providers.contains_key(id) {
            return false;
        }
        if id == COMPILED_JAVA_PROVIDER {
            self.clear_compiled();
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
            self.changed.insert(key.clone());
            // A host source replacement cannot transfer a prior build's binary authority.
            let compiled = self
                .inputs
                .remove(&key)
                .and_then(|input| (input.text == text).then_some(input.compiled).flatten());
            self.inputs.insert(
                key,
                SourceInput {
                    text,
                    tree,
                    compiled,
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
        self.store.invalidate_model();
    }

    pub fn model_changed(&mut self, index: &dyn InferenceIndex) {
        if self.is_empty() {
            return;
        }
        self.mark_published();
        self.invalidate_hydrated_sources();
        self.clear_compiled();
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

    pub fn revision(&self) -> u64 {
        self.store.publication_revision()
    }

    pub fn publish(&mut self, index: &mut Index) -> bool {
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
        let coverage = Knowledge::Unknown(vec![IncompleteReason::IncompleteTarget]);
        let replaced = match input.prepared_references.clone() {
            Some(prepared) => self
                .store
                .replace_prepared_source_declarations(index, prepared, coverage, facts),
            None => self.store.replace_source_declarations(
                index,
                key,
                input.text.clone(),
                coverage,
                facts,
            ),
        };
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
        for (provider, owner) in self.store.take_dirty() {
            if provider == COMPILED_JAVA_PROVIDER
                && !self
                    .inputs
                    .get(&owner)
                    .is_some_and(|input| input.compiled.is_some())
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
            let Knowledge::Found(class) = read.compiled_class(&owner)? else {
                continue;
            };
            for method in read.compiled_methods(&class)? {
                // The shared core validates language surface and descriptor; this provider cannot
                // create a signature, source position or a framework-specific completion item.
                let _ = read.admit_java_method(&method)?;
            }
        }
        Ok(())
    }
}

struct SourceInput {
    text: Arc<str>,
    tree: Tree,
    compiled: Option<Arc<ktcore::classfile::ClassFileFacts>>,
    prepared_references: Option<ktcore::extensions::source::PreparedSourceReferences>,
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
