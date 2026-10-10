//! Host adapter for shared contributions. Framework policy lives in registered consumers.

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use ktcore::extensions::{ContributionSet, ExtensionStore, IncompleteReason};
use ktcore::index::{Index, InferenceIndex};
use ktcore::knowledge::Knowledge;
use tree_sitter::Tree;

pub use ktcore::extensions::AnalysisProvider;

#[derive(Default)]
pub(crate) struct AnalysisExtensions {
    providers: BTreeMap<String, Arc<dyn AnalysisProvider>>,
    inputs: BTreeMap<String, (Arc<str>, Tree)>,
    store: ExtensionStore,
    changed: BTreeSet<String>,
    suspended: bool,
    pending_index: bool,
    published_revision: u64,
}

impl AnalysisExtensions {
    pub fn new(providers: Vec<Arc<dyn AnalysisProvider>>) -> Self {
        let mut result = Self::default();
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
        self.providers.is_empty()
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
        let namespace = self.store.namespace_revision();
        let keys: Vec<_> = sources.iter().map(|(key, _, _)| key.clone()).collect();
        for (key, text, tree) in sources {
            self.changed.insert(key.clone());
            self.inputs.insert(key, (text, tree));
        }
        for key in keys {
            self.normalize(index, &key);
        }
        // Type visibility changes can change an otherwise untouched source signature.
        if self.store.namespace_revision() != namespace {
            self.normalize_all(index);
        }
        self.evaluate(index);
    }

    pub fn remove_source(&mut self, index: &dyn InferenceIndex, key: &str) {
        self.inputs.remove(key);
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
        self.store.invalidate_model();
    }

    pub fn model_changed(&mut self, index: &dyn InferenceIndex) {
        if self.is_empty() {
            return;
        }
        self.mark_published();
        self.store.invalidate_model();
        self.normalize_all(index);
        self.evaluate(index);
    }

    pub fn mark_index_changed(&mut self) {
        if !self.is_empty() && !self.pending_index {
            self.mark_published();
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

    fn normalize(&mut self, index: &dyn InferenceIndex, key: &str) {
        let Some((text, tree)) = self.inputs.get(key) else {
            return;
        };
        let facts = crate::language::extension_source_facts(index, key, tree, text);
        if let Err(error) = self.store.replace_source_declarations(
            index,
            key,
            text.clone(),
            Knowledge::Unknown(vec![IncompleteReason::IncompleteTarget]),
            facts,
        ) {
            // Accepted source/index disagreement must remove old results, never reuse their spans.
            self.store.remove_source(key);
            tracing::warn!(file = key, ?error, "invalid normalized extension source");
        }
    }

    fn normalize_all(&mut self, index: &dyn InferenceIndex) {
        if self.is_empty() {
            return;
        }
        for key in self.inputs.keys().cloned().collect::<Vec<_>>() {
            self.normalize(index, &key);
        }
    }

    fn evaluate(&mut self, index: &dyn InferenceIndex) {
        if self.suspended {
            return;
        }
        for (provider, owner) in self.store.take_dirty() {
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
