//! Owns the cross-file index + open (dirty) document text + a parser, and exposes the operations
//! the LSP layer drives. All keys are the caller's canonical identity string (a path or URI
//! string); we never re-derive identity from the filesystem at query time.

use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::Path;
use std::process::Command;
use std::sync::{mpsc, Arc, Mutex};

use salsa::Durability;
use tree_sitter::{Node, Tree};
use walkdir::{DirEntry, WalkDir};

use crate::actions::{self, Action};
use crate::complete::{self, ShapedCompletions};
use crate::hierarchy::{self, HierarchyItem, IncomingCall, OutgoingCall};
use crate::index::{Index, InferenceIndex, RefEntry, Tier, Usage};
use crate::infer;
use crate::java;
use crate::language::{self, LanguageParsers, SourceLanguage};
use crate::parser::{compute_edit, identifier_at, node_text};
use crate::project_model::{self, ProjectScope};
use crate::ranges::{self, FoldRange, SelectionRange};
use crate::resolve;
use crate::salsa_support::{ExprKey, SalsaState};
use crate::semantic;
use crate::semantic_query;
use crate::symbol::{Def, IndexedSymbol, SymbolKind};
use crate::symbols::SymbolSummary;
use ktcore::extensions::{NameLink, RenameTarget};
use ktcore::knowledge::Knowledge;

/// An open buffer: its current text plus the parsed tree, kept in sync so goto-definition reads an
/// already-current tree instead of re-parsing on every request.
struct DocState {
    text: String,
    tree: Tree,
}

struct ProjectFileIndex {
    key: String,
    package: String,
    symbols: Vec<IndexedSymbol>,
    jvm: Option<ktcore::jvm::SourceFileFacts>,
    usages: Vec<Usage>,
    clean: bool,
    stamp: Option<SourceStamp>,
    source: (Arc<str>, Tree),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct SourceStamp {
    modified: std::time::SystemTime,
    size: u64,
}

pub(crate) struct ModelSourceUpdate {
    current: HashSet<String>,
    batches: Vec<ProjectFileIndex>,
    complete: bool,
}

#[derive(Clone)]
struct ProjectFileMeta {
    package: String,
    clean: bool,
    scope: Option<ProjectScope>,
    kotlin_values: Option<Arc<ktcore::extensions::source::KotlinValueNamespaceFacts>>,
}

fn is_java_path(key: &str) -> bool {
    SourceLanguage::for_key(key).is_java()
}

pub struct Workspace {
    _drop_start_marker: WorkspaceDropMarker,
    pub index: Index,
    _index_drop_marker: WorkspaceDropMarker,
    /// Open buffers, keyed by canonical identity. Take precedence over disk.
    open_docs: HashMap<String, DocState>,
    parsers: LanguageParsers,
    salsa: RefCell<SalsaState>,
    index_revision: u64,
    completeness: resolve::CompletenessFacts,
    project_file_meta: HashMap<String, ProjectFileMeta>,
    project_scan_initialized: bool,
    project_model: Option<Arc<crate::classpath::ProjectSnapshot>>,
    global_origins: BTreeSet<String>,
    source_stamps: HashMap<String, SourceStamp>,
    completed_dependency_origins: BTreeSet<String>,
    analysis_generation: u64,
    live_dependency_origins: BTreeSet<String>,
    project_refresh_pending: bool,
    analysis_extensions: crate::analysis_extensions::AnalysisExtensions,
    extension_sources_running: bool,
    annotated_type_owners: HashMap<String, BTreeSet<Def>>,
    observed_compile: Option<ObservedCompileTicket>,
    kotlin_diagnostic_tickets: BTreeMap<ProjectScope, KotlinDiagnosticTicket>,
    kotlin_diagnostic_reports: HashMap<String, KotlinFileDiagnostics>,
    kotlin_source_generations: BTreeMap<ProjectScope, u64>,
    kotlin_diagnostic_store_epoch: u64,
    kotlin_diagnostic_changes: BTreeSet<String>,
    compiler_api_environments:
        BTreeMap<ProjectScope, Arc<crate::compile::KotlinDiagnosticEnvironment>>,
    class_inventory: RefCell<crate::binary_diagnostics::ClassInventoryCache>,
    // Keep this last: its destructor observes completion of all preceding field destruction.
    _drop_marker: WorkspaceDropMarker,
}

struct WorkspaceDropMarker(&'static str);

impl Drop for WorkspaceDropMarker {
    fn drop(&mut self) {
        if crate::trace::enabled() {
            crate::trace::span(
                self.0,
                "workspace",
                std::time::Instant::now(),
                serde_json::json!({}),
            );
        }
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

impl Workspace {
    pub fn new() -> Self {
        Self::with_analysis_providers(Vec::new())
    }

    pub fn with_analysis_providers(
        providers: Vec<Arc<dyn crate::analysis_extensions::AnalysisProvider>>,
    ) -> Self {
        Workspace {
            _drop_start_marker: WorkspaceDropMarker("workspace.drop.start"),
            index: Index::new(),
            _index_drop_marker: WorkspaceDropMarker("workspace.index.dropped"),
            open_docs: HashMap::new(),
            parsers: LanguageParsers::new(),
            salsa: RefCell::new(SalsaState::new()),
            index_revision: 0,
            completeness: resolve::CompletenessFacts::default(),
            project_file_meta: HashMap::new(),
            project_scan_initialized: false,
            project_model: None,
            global_origins: BTreeSet::new(),
            source_stamps: HashMap::new(),
            completed_dependency_origins: BTreeSet::new(),
            analysis_generation: 0,
            live_dependency_origins: BTreeSet::new(),
            project_refresh_pending: false,
            analysis_extensions: crate::analysis_extensions::AnalysisExtensions::new(providers),
            extension_sources_running: false,
            annotated_type_owners: HashMap::new(),
            observed_compile: None,
            kotlin_diagnostic_tickets: BTreeMap::new(),
            kotlin_diagnostic_reports: HashMap::new(),
            kotlin_source_generations: BTreeMap::new(),
            kotlin_diagnostic_store_epoch: 0,
            kotlin_diagnostic_changes: BTreeSet::new(),
            compiler_api_environments: BTreeMap::new(),
            class_inventory: RefCell::default(),
            _drop_marker: WorkspaceDropMarker("workspace.fields.dropped"),
        }
    }

    /// Parse `text` for `key` using the appropriate grammar (Kotlin or Java).
    fn parse_text(&mut self, key: &str, text: &str) -> Tree {
        self.parsers.parse_for_key(key, text)
    }

    /// Incrementally reparse `text` for `key`, reusing `old_tree`.
    fn reparse_text(&mut self, key: &str, text: &str, old_tree: &Tree) -> Tree {
        self.parsers.reparse_for_key(key, text, old_tree)
    }

    /// Find the identifier-like node at `offset` in `tree`, using Java node kinds when the key
    /// is a `.java` file.
    fn identifier_at_key<'a>(&self, key: &str, tree: &'a Tree, offset: usize) -> Option<Node<'a>> {
        language::identifier_at(SourceLanguage::for_key(key), tree, offset)
    }

    pub fn set_project_scan_complete(&mut self, complete: bool) {
        self.completeness.project_scan_complete = complete;
    }

    pub fn set_library_index_complete(&mut self, complete: bool) {
        self.completeness.library_index_complete = complete;
    }

    pub fn set_jdk_index_complete(&mut self, complete: bool) {
        self.completeness.jdk_index_complete = complete;
    }

    pub fn open_doc_keys(&self) -> Vec<String> {
        self.open_docs.keys().cloned().collect()
    }

    pub fn analysis_generation(&self) -> u64 {
        self.analysis_generation
    }

    pub fn begin_analysis_generation(&mut self) -> u64 {
        self.invalidate_kotlin_diagnostics();
        self.class_inventory.get_mut().clear();
        self.invalidate_observed_compile();
        self.analysis_generation = self.analysis_generation.wrapping_add(1);
        self.completeness.project_scan_complete = false;
        self.completeness.library_index_complete = false;
        self.project_refresh_pending = true;
        self.analysis_extensions.suspend();
        self.bump_index_revision();
        self.analysis_generation
    }

    pub fn is_current_generation(&self, generation: u64) -> bool {
        self.analysis_generation == generation
    }

    pub fn project_snapshot(&self) -> Option<Arc<crate::classpath::ProjectSnapshot>> {
        self.project_model.clone()
    }

    pub(crate) fn finish_analysis_generation(
        &mut self,
        generation: u64,
        libraries: bool,
        jdk: bool,
    ) -> bool {
        if !self.is_current_generation(generation) {
            return false;
        }
        self.set_library_index_complete(libraries);
        self.set_jdk_index_complete(jdk);
        true
    }

    pub(crate) fn source_stamps(&self) -> HashMap<String, SourceStamp> {
        self.source_stamps.clone()
    }

    pub(crate) fn dependency_origin_complete(&self, origin: &str) -> bool {
        self.completed_dependency_origins.contains(origin)
    }

    pub(crate) fn complete_dependency_origin(&mut self, origin: String) {
        self.completed_dependency_origins.insert(origin);
    }

    pub fn set_project_model(&mut self, model: Arc<crate::classpath::ProjectSnapshot>) {
        self.invalidate_kotlin_diagnostics();
        self.class_inventory.get_mut().clear();
        self.invalidate_observed_compile();
        self.index.replace_scope_visibilities(
            model.scope_visibilities(&self.global_origins.iter().cloned().collect::<Vec<_>>()),
        );
        self.index.replace_caller_scope_visibilities(
            model.caller_scope_visibilities(
                &self.global_origins.iter().cloned().collect::<Vec<_>>(),
            ),
        );
        for key in self.project_file_meta.keys().chain(self.open_docs.keys()) {
            if let Some(origin) = model.file_origin(Path::new(key)) {
                self.index.set_file_origin(key, origin);
            } else if self
                .index
                .file_origin(key)
                .is_some_and(|origin| origin.project_scope.is_some())
            {
                self.index.remove_file_origin(key);
            }
        }
        for (key, meta) in &mut self.project_file_meta {
            meta.scope = model
                .file_origin(Path::new(key))
                .and_then(|origin| origin.project_scope);
        }
        let active = model.live_origins();
        let previous_revisions = self
            .project_model
            .as_ref()
            .map(|model| model.artifact_revisions())
            .unwrap_or_default();
        let revisions = model.artifact_revisions();
        self.global_origins.retain(|origin| {
            !crate::classpath::is_sdk_origin(origin)
                || active.contains(origin)
                    && (self.project_model.is_none()
                        || previous_revisions.get(origin) == revisions.get(origin))
        });
        let changed_origins: BTreeSet<_> = previous_revisions
            .iter()
            .filter(|(origin, revision)| {
                active.contains(*origin) && revisions.get(*origin) != Some(*revision)
            })
            .map(|(origin, _)| origin.clone())
            .collect();
        let previous_binaries = self
            .project_model
            .as_ref()
            .map(|model| model.binary_artifact_revisions())
            .unwrap_or_default();
        let binaries = model.binary_artifact_revisions();
        let unchanged_binaries: BTreeSet<_> = changed_origins
            .iter()
            .filter(|origin| {
                binaries.contains_key(*origin)
                    && previous_binaries.get(*origin) == binaries.get(*origin)
            })
            .cloned()
            .collect();
        self.live_dependency_origins = active.clone();
        let removed: Vec<_> = self
            .index
            .file_origins()
            .filter(|(key, origin)| {
                is_dependency_origin(&origin.origin_id)
                    && (!active.contains(&origin.origin_id)
                        || changed_origins.contains(&origin.origin_id)
                            && !(origin.representation
                                == crate::project_model::SourceRepresentation::Stub
                                && unchanged_binaries.contains(&origin.origin_id)))
                    && !self.open_docs.contains_key(*key)
            })
            .map(|(key, _)| key.to_string())
            .collect();
        for key in removed {
            // Artifact retirement does not prove an authored type was deleted.
            self.index.remove_file_origin(&key);
            self.index.remove_file(&key);
            self.analysis_extensions.remove_source(&self.index, &key);
            self.annotated_type_owners.remove(&key);
            self.salsa.get_mut().forget(&key);
            self.project_file_meta.remove(&key);
        }
        self.completed_dependency_origins.retain(|origin| {
            self.global_origins.contains(origin)
                || (active.contains(origin)
                    && previous_revisions.get(origin) == revisions.get(origin))
        });
        self.project_model = Some(model);
        self.analysis_extensions.model_changed(&self.index);
        self.bump_index_revision();
        self.recompute_project_completeness();
    }

    /// Reconcile source ownership without reparsing unchanged sources from the initial scan.
    pub fn scan_model_sources(&mut self) -> usize {
        let Some(model) = &self.project_model else {
            return 0;
        };
        let update = prepare_model_sources(model, &self.source_stamps);
        self.apply_model_sources(update)
    }

    pub(crate) fn apply_model_sources(&mut self, update: ModelSourceUpdate) -> usize {
        self.invalidate_observed_compile();
        self.project_refresh_pending = false;
        self.remove_obsolete_project_files(&update.current);
        self.source_stamps
            .retain(|key, _| update.current.contains(key));
        let mut count = 0;
        let mut extension_sources = Vec::new();
        for batch in update.batches {
            if let Some(stamp) = batch.stamp {
                self.source_stamps.insert(batch.key.clone(), stamp);
            }
            if self.open_docs.contains_key(&batch.key) {
                continue;
            }
            count += 1;
            if let Some(origin) = self
                .project_model
                .as_ref()
                .and_then(|m| m.file_origin(Path::new(&batch.key)))
            {
                self.index.set_file_origin(&batch.key, origin);
            }
            self.project_file_meta.insert(
                batch.key.clone(),
                ProjectFileMeta {
                    kotlin_values: None,
                    package: batch.package,
                    clean: batch.clean,
                    scope: self.project_scope_for_file(&batch.key),
                },
            );
            self.index
                .replace_file(&batch.key, batch.symbols, Tier::Volatile);
            self.index
                .replace_jvm_source(&batch.key, batch.jvm, Tier::Volatile);
            self.index.replace_file_refs(&batch.key, batch.usages);
            if self
                .index
                .jvm_source_for_file(&batch.key)
                .is_some_and(|source| {
                    source.facts.callables.iter().any(|callable| {
                        callable
                            .source_property
                            .as_ref()
                            .is_some_and(|p| p.deferred.is_some())
                    })
                })
            {
                self.salsa
                    .get_mut()
                    .record_text(&batch.key, &batch.source.0, Durability::LOW);
            } else {
                self.salsa.get_mut().forget(&batch.key);
            }
            self.record_annotated_owners(&batch.key, &batch.source.0, &batch.source.1);
            if self.analysis_extensions.needs_source(&batch.key) {
                extension_sources.push((batch.key, batch.source.0, batch.source.1));
            }
        }
        let extension_revision = self.analysis_extensions.revision();
        self.analysis_extensions
            .replace_sources(&self.index, extension_sources);
        self.analysis_extensions.resume(&self.index);
        if count > 0 || self.analysis_extensions.revision() != extension_revision {
            self.bump_index_revision();
        }
        self.project_scan_initialized = true;
        for (key, doc) in &self.open_docs {
            if self
                .project_model
                .as_ref()
                .and_then(|m| m.file_origin(Path::new(key)))
                .is_some()
            {
                let facts =
                    language::file_facts(SourceLanguage::for_key(key), &doc.tree, &doc.text);
                self.project_file_meta.insert(
                    key.clone(),
                    ProjectFileMeta {
                        kotlin_values: self.kotlin_value_facts(key, &doc.text, &doc.tree),
                        package: facts.package,
                        clean: facts.clean,
                        scope: self.project_scope_for_file(key),
                    },
                );
            }
        }
        self.recompute_project_completeness();
        if !update.complete {
            self.completeness.project_scan_complete = false;
        }
        if self.index.prune_origin_history() {
            self.bump_index_revision();
        }
        count
    }

    pub(crate) fn install_project_update(
        &mut self,
        generation: u64,
        model: Arc<crate::classpath::ProjectSnapshot>,
        update: ModelSourceUpdate,
    ) -> Option<usize> {
        if !self.is_current_generation(generation) {
            return None;
        }
        self.set_project_model(model);
        Some(self.apply_model_sources(update))
    }

    fn remove_obsolete_project_files(&mut self, current: &HashSet<String>) {
        let obsolete: Vec<_> = self
            .project_file_meta
            .keys()
            .filter(|key| !current.contains(*key) && !self.open_docs.contains_key(*key))
            .cloned()
            .collect();
        for key in &obsolete {
            // Initial checkout facts have no origin; dependency facts belong to another owner.
            if self
                .index
                .file_origin(key)
                .is_none_or(|origin| origin.project_scope.is_some())
            {
                self.index.remove_file(key);
                self.analysis_extensions.remove_source(&self.index, key);
                self.annotated_type_owners.remove(key);
                self.salsa.get_mut().forget(key);
            }
            self.project_file_meta.remove(key);
            self.source_stamps.remove(key);
        }
        if !obsolete.is_empty() {
            self.bump_index_revision();
        }
        if let Some(model) = &self.project_model {
            if model.modules.iter().any(|m| !m.source_sets.is_empty()) {
                self.project_file_meta
                    .retain(|key, _| model.file_origin(Path::new(key)).is_some());
            }
        }
    }

    pub fn project_scope_for_file(&self, key: &str) -> Option<ProjectScope> {
        match &self.project_model {
            Some(model) => model
                .file_origin(Path::new(key))
                .and_then(|origin| origin.project_scope),
            None => project_model::project_scope_for_path(key),
        }
    }

    pub fn index_dependency_batch(&mut self, batch: crate::deps::FileSymbols) -> bool {
        if self.project_model.is_some()
            && batch.origin.as_ref().is_some_and(|origin| {
                is_dependency_origin(&origin.origin_id)
                    && !self.live_dependency_origins.contains(&origin.origin_id)
            })
        {
            return false;
        }
        let mut metadata_changed = false;
        if self.project_model.is_none() {
            if let Some(origin) = batch
                .origin
                .as_ref()
                .filter(|origin| crate::classpath::is_sdk_origin(&origin.origin_id))
            {
                self.global_origins.insert(origin.origin_id.clone());
                metadata_changed |= self.index.set_file_origin(&batch.file, origin.clone());
            }
        }
        if let Some(model) = &self.project_model {
            if model.modules.iter().any(|m| !m.source_sets.is_empty()) {
                if let Some(origin) = batch.origin.clone() {
                    if crate::classpath::is_sdk_origin(&origin.origin_id)
                        && self.global_origins.insert(origin.origin_id.clone())
                    {
                        metadata_changed |=
                            self.index
                                .replace_scope_visibilities(model.scope_visibilities(
                                    &self.global_origins.iter().cloned().collect::<Vec<_>>(),
                                ));
                        metadata_changed |= self.index.replace_caller_scope_visibilities(
                            model.caller_scope_visibilities(
                                &self.global_origins.iter().cloned().collect::<Vec<_>>(),
                            ),
                        );
                    }
                    // Late ownership metadata also belongs to authoritative open source buffers.
                    metadata_changed |= self.index.set_file_origin(&batch.file, origin);
                }
            }
        }
        // Archive hydration also needs exact ownership in projects without an imported model.
        if self.project_model.is_none()
            && self.index.file_origin(&batch.file).is_none()
            && batch.library_source.is_some()
        {
            if let Some(origin) = &batch.origin {
                metadata_changed |= self.index.set_file_origin(&batch.file, origin.clone());
            }
        }
        if metadata_changed {
            self.invalidate_observed_compile();
            self.analysis_extensions.mark_index_changed();
        }
        let indexed = self.index_dependency_file(&batch.file, batch.symbols);
        self.analysis_extensions.select_library_source(
            &self.index,
            &batch.file,
            batch.origin.clone(),
            batch.library_source,
        );
        if indexed {
            self.index
                .replace_jvm_source(&batch.file, batch.jvm, Tier::Durable);
            true
        } else {
            false
        }
    }

    pub fn index_dependency_batch_for_generation(
        &mut self,
        generation: u64,
        batch: crate::deps::FileSymbols,
    ) -> bool {
        self.is_current_generation(generation) && self.index_dependency_batch(batch)
    }

    /// Insert closed dependency facts under a normalized key, preserving authoritative open text.
    /// Callers bump the revision once for their whole batch.
    pub fn index_dependency_file(&mut self, key: &str, symbols: Vec<IndexedSymbol>) -> bool {
        if self.open_docs.contains_key(key) {
            return false;
        }
        self.invalidate_observed_compile();
        self.analysis_extensions.mark_index_changed();
        self.index.replace_file(key, symbols, Tier::Durable);
        true
    }

    pub(crate) fn remove_dependency_file(&mut self, key: &str) {
        if self.open_docs.contains_key(key) {
            return;
        }
        self.invalidate_observed_compile();
        self.index.remove_file(key);
        self.analysis_extensions.remove_source(&self.index, key);
        self.analysis_extensions.mark_index_changed();
        self.annotated_type_owners.remove(key);
        self.salsa.get_mut().forget(key);
    }

    pub fn project_doc_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.project_file_meta.keys().cloned().collect();
        keys.sort();
        keys
    }

    /// Test helper for fixtures that intentionally model a closed source world.
    pub fn assume_index_complete_for_tests(&mut self) {
        self.completeness = resolve::CompletenessFacts::complete();
        self.project_scan_initialized = false;
        self.project_file_meta.clear();
    }

    pub fn bump_index_revision(&mut self) {
        self.analysis_extensions.flush_index_changed(&self.index);
        self.analysis_extensions.publish(&mut self.index);
        self.index_revision = self.index_revision.wrapping_add(1).max(1);
    }

    pub fn configure_analysis_provider(
        &mut self,
        provider: &str,
        enabled: bool,
        configuration: Arc<str>,
    ) -> bool {
        self.invalidate_observed_compile();
        let revision = self.analysis_extensions.revision();
        let registered =
            self.analysis_extensions
                .configure(&self.index, provider, enabled, configuration);
        if self.analysis_extensions.revision() != revision {
            self.bump_index_revision();
        }
        registered
    }

    pub(crate) fn extension_source_input_identity(&self, owner: &str) -> Option<Arc<str>> {
        self.analysis_extensions.source_input_identity(owner)
    }

    pub(crate) fn extension_source_wait(
        &mut self,
        owner: &str,
    ) -> Option<tokio::sync::watch::Receiver<bool>> {
        self.analysis_extensions.source_wait(&self.index, owner)
    }

    pub(crate) fn begin_extension_source_batch(
        &mut self,
    ) -> Vec<crate::analysis_extensions::SourceRequest> {
        if self.extension_sources_running {
            return Vec::new();
        }
        let requests = self.analysis_extensions.take_source_requests(&self.index);
        self.extension_sources_running = !requests.is_empty();
        requests
    }

    pub(crate) fn finish_extension_source_batch(
        &mut self,
        prepared: Vec<crate::analysis_extensions::PreparedSource>,
    ) -> Vec<String> {
        if self
            .analysis_extensions
            .accept_sources(&self.index, prepared)
        {
            self.bump_index_revision();
        }
        self.extension_sources_running = false;
        let mut changed = self.take_extension_diagnostic_changes();
        // Closed archive facts do not need editor diagnostics or a second closed-document parse.
        changed.retain(|key| {
            self.open_docs.contains_key(key) || self.project_file_meta.contains_key(key)
        });
        changed
    }

    pub fn take_extension_diagnostic_changes(&mut self) -> Vec<String> {
        let mut keys = self.analysis_extensions.take_changed();
        keys.extend(std::mem::take(&mut self.kotlin_diagnostic_changes));
        keys.sort();
        keys.dedup();
        keys
    }

    fn replace_extension_source(&mut self, key: &str, text: &str, tree: &Tree) {
        self.record_annotated_owners(key, text, tree);
        if self.analysis_extensions.needs_source(key) {
            self.analysis_extensions.replace_sources(
                &self.index,
                vec![(key.into(), Arc::from(text), tree.clone())],
            );
            if self.analysis_extensions.publish(&mut self.index) {
                self.index_revision = self.index_revision.wrapping_add(1).max(1);
            }
        }
    }

    fn record_annotated_owners(&mut self, key: &str, text: &str, tree: &Tree) {
        let values = self.kotlin_value_facts(key, text, tree);
        if let Some(meta) = self.project_file_meta.get_mut(key) {
            meta.kotlin_values = values;
        }
        self.annotated_type_owners.insert(
            key.into(),
            language::annotated_type_owners(&self.index, key, tree, text)
                .into_iter()
                .collect(),
        );
    }

    fn kotlin_value_facts(
        &self,
        key: &str,
        text: &str,
        tree: &Tree,
    ) -> Option<Arc<ktcore::extensions::source::KotlinValueNamespaceFacts>> {
        if is_java_path(key) {
            return None;
        }
        let scope = language::source_type_scope(key, tree, text);
        ktcore::extensions::source::kotlin_value_namespace(&self.index, key, tree, text, &scope)
            .map(Arc::new)
    }

    fn current_kotlin_value_facts(
        &self,
        key: &str,
    ) -> Option<&ktcore::extensions::source::KotlinValueNamespaceFacts> {
        if is_java_path(key) {
            return None;
        }
        let facts = self.project_file_meta.get(key)?.kotlin_values.as_deref()?;
        // Positive authored field/accessor identity may survive a pending scan; it never
        // closes that generation's namespace or certifies compiler-generated methods.
        if !self.source_only_profile_in_source(key, None)
            || self.analysis_extensions.needs_source(key)
        {
            return None;
        }
        if self
            .index
            .lookup_type("JvmField")
            .iter()
            .any(|entry| entry.sym.package == "kotlin.jvm" && entry.tier == Tier::Volatile)
        {
            return None;
        }
        // A qualifier type can be added in another source after this file was
        // normalized. Revalidate only actual field facts, once per file/query.
        if facts.properties.values().any(|access| {
            matches!(
                access,
                Knowledge::Found(ktcore::extensions::source::JvmPropertyAccess::Field)
            )
        }) {
            let entries = self.index.entries_for_file(key);
            for (definition, access) in &facts.properties {
                if !matches!(
                    access,
                    Knowledge::Found(ktcore::extensions::source::JvmPropertyAccess::Field)
                ) {
                    continue;
                }
                let property = entries
                    .iter()
                    .find(|entry| rename_definition(entry) == *definition)?;
                if !ktcore::extensions::source::kotlin_jvm_field_qualifier_unshadowed(
                    &self.index,
                    &property.sym,
                    &facts.qualifier_packages,
                ) {
                    return None;
                }
            }
        }
        if let Some(doc) = self.open_docs.get(key) {
            return facts.matches_source(&doc.text).then_some(facts);
        }
        let stamp = std::fs::metadata(key).ok().and_then(|meta| {
            Some(SourceStamp {
                modified: meta.modified().ok()?,
                size: meta.len(),
            })
        })?;
        if self.source_stamps.get(key) != Some(&stamp) {
            return None;
        }
        let text = std::fs::read_to_string(key).ok()?;
        facts.matches_source(&text).then_some(facts)
    }

    /// Current text for a key: the open buffer if present, else the file on disk.
    pub fn doc_text(&self, key: &str) -> Option<String> {
        if let Some(doc) = self.open_docs.get(key) {
            return Some(doc.text.clone());
        }
        std::fs::read_to_string(key).ok().or_else(|| {
            crate::deps::materialize_source_file(Path::new(key))
                .then(|| std::fs::read_to_string(key).ok())
                .flatten()
        })
    }

    /// Source seeding reuses the open buffer's parsed tree without a second whole-file parse.
    pub(crate) fn open_java_source(&self, key: &str) -> Option<(String, Tree)> {
        if !is_java_path(key) {
            return None;
        }
        let doc = self.open_docs.get(key)?;
        Some((doc.text.clone(), doc.tree.clone()))
    }

    /// Index a project file from an already-parsed tree: its declarations (volatile tier) and its
    /// identifier usages (reverse-reference index).
    fn index_from_tree(&mut self, key: &str, text: &str, tree: &Tree) {
        let facts = language::file_facts(SourceLanguage::for_key(key), tree, text);
        self.apply_file_facts(key, facts);
        self.replace_extension_source(key, text, tree);
    }

    fn apply_file_facts(&mut self, key: &str, facts: language::FileFacts) {
        if let Some(origin) = self
            .project_model
            .as_ref()
            .and_then(|m| m.file_origin(Path::new(key)))
        {
            self.index.set_file_origin(key, origin);
        }
        self.index.replace_file(key, facts.symbols, Tier::Volatile);
        self.index
            .replace_jvm_source(key, facts.jvm, Tier::Volatile);
        self.index.replace_file_refs(key, facts.usages);
        self.bump_index_revision();
        self.record_project_file_meta(key, facts.package, facts.clean);
    }

    /// Parse `text` from scratch and (re)index the file. Used for non-open files (scan/close),
    /// where there is no cached tree to reuse.
    pub fn reindex(&mut self, key: &str, text: &str) {
        self.observe_compiled_source(key, Some(text), false);
        let facts = self.salsa.get_mut().file_facts(key, text, Durability::LOW);
        self.apply_file_facts(key, facts);
        let tree = self.salsa.get_mut().parsed_tree(key, text, Durability::LOW);
        self.replace_extension_source(key, text, &tree);
    }

    /// `textDocument/didOpen`.
    pub fn open(&mut self, key: impl Into<String>, text: String) {
        let key = key.into();
        self.invalidate_kotlin_source(&key);
        self.observe_compiled_source(&key, Some(&text), false);
        let tree = self
            .salsa
            .get_mut()
            .parsed_tree(&key, &text, Durability::LOW);
        let facts = self
            .salsa
            .get_mut()
            .file_facts(&key, &text, Durability::LOW);
        self.apply_file_facts(&key, facts);
        self.replace_extension_source(&key, &text, &tree);
        self.open_docs.insert(key, DocState { text, tree });
    }

    /// `textDocument/didChange` (FULL sync: `text` is the whole new document). Reparses
    /// incrementally by diffing against the cached buffer, then re-indexes from the new tree.
    pub fn change(&mut self, key: &str, text: String) {
        self.invalidate_kotlin_source(key);
        self.observe_compiled_source(key, Some(&text), true);
        let tree = match self.open_docs.remove(key) {
            Some(mut old) => {
                let edit = compute_edit(&old.text, &text);
                old.tree.edit(&edit);
                self.reparse_text(key, &text, &old.tree)
            }
            None => self.parse_text(key, &text),
        };
        self.salsa
            .get_mut()
            .record_text(key, &text, Durability::LOW);
        self.index_from_tree(key, &text, &tree);
        self.open_docs
            .insert(key.to_string(), DocState { text, tree });
    }

    /// `textDocument/didClose`: drop the dirty buffer; re-sync the index from disk (or drop it).
    pub fn close(&mut self, key: &str) {
        self.invalidate_kotlin_source(key);
        self.kotlin_diagnostic_reports.remove(key);
        let disk = std::fs::read_to_string(key).ok();
        self.observe_compiled_source(key, disk.as_deref(), false);
        self.open_docs.remove(key);
        let inactive_dependency = self.index.file_origin(key).is_some_and(|origin| {
            is_dependency_origin(&origin.origin_id)
                && !origin.origin_id.starts_with("jdk:")
                && self.project_model.is_some()
                && !self.live_dependency_origins.contains(&origin.origin_id)
        });
        if self.project_model.as_ref().is_some_and(|model| {
            model.modules.iter().any(|m| !m.source_sets.is_empty())
                && model.file_origin(Path::new(key)).is_none()
        }) && self.index.file_origin(key).is_none()
            || inactive_dependency
        {
            self.index.remove_file(key);
            self.analysis_extensions.remove_source(&self.index, key);
            self.annotated_type_owners.remove(key);
            self.salsa.get_mut().forget(key);
            self.project_file_meta.remove(key);
            self.bump_index_revision();
            self.recompute_project_completeness();
            return;
        }
        match disk.or_else(|| self.doc_text(key)) {
            Some(text) => self.reindex(key, &text),
            None => {
                self.index.remove_file(key);
                self.analysis_extensions.remove_source(&self.index, key);
                self.annotated_type_owners.remove(key);
                self.bump_index_revision();
                self.salsa.get_mut().forget(key);
                if self.project_scan_initialized {
                    self.project_file_meta.remove(key);
                    self.recompute_project_completeness();
                }
            }
        }
    }

    fn parsed_closed_doc(&mut self, key: &str) -> Option<(String, Tree)> {
        let text = self.doc_text(key)?;
        let tree = self
            .salsa
            .get_mut()
            .parsed_tree(key, &text, Durability::LOW);
        Some((text, tree))
    }

    /// Index every project `.kt`/`.kts`/`.java` under `root`, skipping build output except Gradle
    /// generated source roots and any files currently open (their dirty buffers are authoritative).
    /// Returns the count indexed.
    pub fn scan(&mut self, root: &Path) -> usize {
        self.invalidate_observed_compile();
        self.project_refresh_pending = false;
        let open_keys = self
            .open_docs
            .keys()
            .map(|key| crate::paths::file_key(Path::new(key)))
            .collect::<HashSet<_>>();
        let source_paths = project_source_files_with_model(root, self.project_model.as_deref());
        let current: HashSet<_> = source_paths
            .iter()
            .map(|path| crate::paths::file_key(path))
            .collect();
        self.remove_obsolete_project_files(&current);
        let mut paths = Vec::new();
        for path in source_paths {
            let key = crate::paths::file_key(&path);
            if open_keys.contains(&key) {
                continue;
            }
            paths.push(path);
        }
        let batches = parse_project_files(paths);
        let n = batches.len();
        self.project_scan_initialized = true;
        self.project_file_meta.clear();
        let mut extension_sources = Vec::new();
        for batch in batches {
            if let Some(stamp) = batch.stamp {
                self.source_stamps.insert(batch.key.clone(), stamp);
            }
            self.project_file_meta.insert(
                batch.key.clone(),
                ProjectFileMeta {
                    kotlin_values: None,
                    package: batch.package.clone(),
                    clean: batch.clean,
                    scope: self.project_scope_for_file(&batch.key),
                },
            );
            if let Some(origin) = self
                .project_model
                .as_ref()
                .and_then(|m| m.file_origin(Path::new(&batch.key)))
            {
                self.index.set_file_origin(&batch.key, origin);
            }
            self.index
                .replace_file(&batch.key, batch.symbols, Tier::Volatile);
            self.index
                .replace_jvm_source(&batch.key, batch.jvm, Tier::Volatile);
            self.index.replace_file_refs(&batch.key, batch.usages);
            self.record_annotated_owners(&batch.key, &batch.source.0, &batch.source.1);
            if self.analysis_extensions.needs_source(&batch.key) {
                extension_sources.push((batch.key, batch.source.0, batch.source.1));
            }
        }
        self.analysis_extensions
            .replace_sources(&self.index, extension_sources);
        self.analysis_extensions.resume(&self.index);
        self.bump_index_revision();
        for (key, doc) in &self.open_docs {
            if self.project_model.as_ref().is_some_and(|model| {
                model.modules.iter().any(|m| !m.source_sets.is_empty())
                    && model.file_origin(Path::new(key)).is_none()
            }) {
                continue;
            }
            let facts = language::file_facts(SourceLanguage::for_key(key), &doc.tree, &doc.text);
            self.project_file_meta.insert(
                key.clone(),
                ProjectFileMeta {
                    kotlin_values: self.kotlin_value_facts(key, &doc.text, &doc.tree),
                    package: facts.package,
                    clean: facts.clean,
                    scope: self.project_scope_for_file(key),
                },
            );
        }
        self.recompute_project_completeness();
        n
    }

    /// `textDocument/definition`: resolve the identifier at `offset` (a byte offset into the
    /// current text of `key`). Open buffers reuse their cached tree; unfinished receivers may
    /// need query-local recovery. Closed source authority can require fresh parsing.
    pub fn goto_definition(&mut self, key: &str, offset: usize) -> Vec<Def> {
        self.refresh_compiled_freshness();
        if let Some(doc) = self.open_docs.get(key) {
            let defs = self.goto_definition_for_tree(key, &doc.text, &doc.tree, offset);
            materialize_definition_files(&defs);
            return defs;
        }
        let (text, tree) = match self.parsed_closed_doc(key) {
            Some(parsed) => parsed,
            None => return Vec::new(),
        };
        let language = SourceLanguage::for_key(key);
        if language == SourceLanguage::Kotlin {
            if let Some(defs) = self.closed_member_goto_definition(key, &text, &tree, offset) {
                materialize_definition_files(&defs);
                return defs;
            }
        }
        let defs = self.goto_definition_for_tree(key, &text, &tree, offset);
        materialize_definition_files(&defs);
        defs
    }

    fn goto_definition_for_tree(
        &self,
        key: &str,
        text: &str,
        tree: &Tree,
        offset: usize,
    ) -> Vec<Def> {
        let recovery = is_java_path(key)
            .then(|| java::definition_source(key, text, tree, offset))
            .flatten();
        self.with_java_member_proofs_in_source(key, recovery.as_ref(), |index| {
            if let Some(recovery) = &recovery {
                if let Some(recovered) = recovery.recovered_index(index, key) {
                    return language::goto_definition(
                        &recovered,
                        SourceLanguage::Java,
                        key,
                        &recovery.text,
                        &recovery.tree,
                        offset,
                    );
                }
            }
            language::goto_definition(index, SourceLanguage::for_key(key), key, text, tree, offset)
        })
    }

    fn closed_member_goto_definition(
        &mut self,
        key: &str,
        text: &str,
        tree: &Tree,
        offset: usize,
    ) -> Option<Vec<Def>> {
        let ident = identifier_at(tree, offset)?;
        if resolve::use_kind(ident) != resolve::UseKind::MemberSelector {
            return None;
        }
        let name = node_text(ident, text);
        if name.is_empty() {
            return Some(Vec::new());
        }
        let nav = ident.parent()?;
        if nav.kind() != "navigation_expression" {
            return None;
        }
        let receiver = nav.named_child(0)?;
        if receiver.kind() == "super_expression" {
            return None;
        }
        let ctx = infer::FileCtx::from_tree_in_file(tree, text, key);
        let receiver_type = self.salsa.get_mut().expr_type(
            key,
            text,
            Durability::LOW,
            &self.index,
            self.index_revision,
            ExprKey::from_node(receiver),
        );
        let receiver_type = infer::member_receiver_type(receiver, receiver_type);
        let member_use = resolve::member_use(&self.index.scoped_for_file(key), ident, text, &ctx);
        let hits = resolve::member_defs_for_use_at(
            &self.index.scoped_for_file(key),
            &receiver_type,
            name,
            &member_use,
            &ctx,
            ident.start_byte(),
        );
        Some(hits)
    }

    pub fn explain_resolution(
        &mut self,
        key: &str,
        offset: usize,
    ) -> Option<crate::commands::ResolutionExplanation> {
        let query = self.resolved_symbol_query(key, offset)?;
        let reference = query.reference();
        let targets = query
            .targets
            .iter()
            .map(|d| format!("{}:{}..{}", d.file, d.start_byte, d.end_byte))
            .collect::<Vec<_>>();
        Some(crate::commands::ResolutionExplanation {
            status: reference.status_label(),
            kind: reference.kind_label(),
            symbol: reference.symbol().map(str::to_string),
            targets,
            reasons: reference.reason_labels(),
        })
    }

    pub fn explain_completion(
        &mut self,
        key: &str,
        offset: usize,
    ) -> Option<crate::commands::CompletionExplanation> {
        let query = self.completion_query(key, offset)?;
        let reasons = query.reason_labels();
        let candidate_count = query.candidates.len();
        Some(crate::commands::CompletionExplanation {
            status: query.status_label(),
            context: query.context_label(),
            prefix: query.prefix,
            candidate_count,
            reasons,
            candidates: query
                .candidates
                .into_iter()
                .take(20)
                .map(|candidate| candidate.label)
                .collect(),
        })
    }

    pub fn resolved_symbol_query(
        &mut self,
        key: &str,
        offset: usize,
    ) -> Option<semantic_query::ResolvedSymbolQuery> {
        self.refresh_compiled_freshness();
        if let Some(doc) = self.open_docs.get(key) {
            return self.with_java_member_proofs(key, |index| {
                semantic_query::resolved_symbol_query(
                    index,
                    key,
                    &doc.tree,
                    &doc.text,
                    offset,
                    &self.effective_completeness(),
                )
            });
        }
        let (text, tree) = self.parsed_closed_doc(key)?;
        self.with_java_member_proofs(key, |index| {
            semantic_query::resolved_symbol_query(
                index,
                key,
                &tree,
                &text,
                offset,
                &self.effective_completeness(),
            )
        })
    }

    pub fn after_dot_query(
        &mut self,
        key: &str,
        offset: usize,
    ) -> Option<semantic_query::CompletionQuery> {
        self.refresh_compiled_freshness();
        if is_java_path(key) {
            return self
                .completion_query(key, offset)
                .filter(|query| query.context == complete::CompletionContext::AfterDot);
        }
        let text = self.doc_text(key)?;
        let mut parser = crate::parser::KotlinParser::new();
        self.with_java_member_proofs(key, |index| {
            semantic_query::after_dot_query(index, &mut parser, key, &text, offset, MAX_COMPLETIONS)
        })
    }

    pub fn completion_query(
        &mut self,
        key: &str,
        offset: usize,
    ) -> Option<semantic_query::CompletionQuery> {
        self.refresh_compiled_freshness();
        if is_java_path(key) {
            if let Some(doc) = self.open_docs.get(key) {
                let recovery = java::completion_source(key, &doc.text, &doc.tree, offset);
                return self.with_java_member_proofs_in_source(key, recovery.as_ref(), |index| {
                    java::completion_query_with_source(
                        index,
                        key,
                        &doc.text,
                        &doc.tree,
                        offset,
                        MAX_COMPLETIONS,
                        recovery.as_ref(),
                    )
                });
            }
            let (text, tree) = self.parsed_closed_doc(key)?;
            let recovery = java::completion_source(key, &text, &tree, offset);
            return self.with_java_member_proofs_in_source(key, recovery.as_ref(), |index| {
                java::completion_query_with_source(
                    index,
                    key,
                    &text,
                    &tree,
                    offset,
                    MAX_COMPLETIONS,
                    recovery.as_ref(),
                )
            });
        }
        if !self.open_docs.contains_key(key) {
            if let Some(query) = self.closed_completion_query(key, offset) {
                return Some(query);
            }
        }
        let text = self.doc_text(key)?;
        let mut parser = crate::parser::KotlinParser::new();
        self.with_java_member_proofs(key, |index| {
            semantic_query::completion_query(
                index,
                &mut parser,
                key,
                &text,
                offset,
                MAX_COMPLETIONS,
            )
        })
    }

    fn closed_completion_query(
        &mut self,
        key: &str,
        offset: usize,
    ) -> Option<semantic_query::CompletionQuery> {
        let text = self.doc_text(key)?;
        let tree = self
            .salsa
            .get_mut()
            .parsed_tree(key, &text, Durability::LOW);
        if tree.root_node().has_error()
            || complete::completion_context(&tree, &text, offset)
                != complete::CompletionContext::AfterDot
        {
            return None;
        }
        let (prefix, synthetic, syn_offset) = complete::dot_recovery(&text, offset)?;
        let synthetic_key = format!("{key}#completion");
        let synthetic_tree =
            self.salsa
                .get_mut()
                .parsed_tree(&synthetic_key, &synthetic, Durability::LOW);
        if synthetic_tree.root_node().has_error() {
            return None;
        }
        let receiver = complete::navigation_receiver_at(&synthetic_tree, syn_offset)?;
        let mut prepared = HashSet::new();
        let receiver_type = loop {
            let result = self.salsa.get_mut().expr_query_in_file(
                &synthetic_key,
                key,
                &synthetic,
                Durability::LOW,
                &self.index,
                self.index_revision,
                ExprKey::from_node(receiver),
            );
            let missing = result
                .missing_sources
                .iter()
                .filter(|file| prepared.insert((*file).clone()))
                .cloned()
                .collect::<Vec<_>>();
            if missing.is_empty() {
                break result.value;
            }
            for file in missing {
                if let Some(text) = self.doc_text(&file) {
                    self.salsa.get_mut().prepare_declaration(
                        &file,
                        &text,
                        &self.index,
                        self.index_revision,
                    );
                }
            }
        };
        let receiver_type = infer::member_receiver_type(receiver, receiver_type);
        let ctx = infer::FileCtx::from_tree_in_file(&synthetic_tree, &synthetic, key);
        semantic_query::member_completion_query_for_type_with_ctx(
            &self.index.scoped_for_file(key),
            SourceLanguage::Kotlin,
            &synthetic_tree,
            &synthetic,
            receiver_type,
            ktcore::jvm::ReceiverKind::Unknown,
            prefix,
            MAX_COMPLETIONS,
            Some((&ctx, receiver.end_byte())),
        )
    }

    /// `textDocument/completion`. Returns visible completion candidates at the cursor, or `None`
    /// for a silent-omission position (string/comment/number/import).
    ///
    /// Two branches, keyed by the shared context detector:
    /// - **`ScopeName`** (Stage A): in-scope names — locals/params/type-params, same-file members +
    ///   top-level, cross-file/imported/default-import top-level names, import aliases, keywords.
    /// - **`AfterDot`** (Stage B): the member set of the receiver's inferred type — own members,
    ///   inherited members (supertype walk), and applicable extensions. Silent omission (`None`)
    ///   when the receiver type can't be inferred.
    ///
    /// Java open buffers reuse their cached tree. Kotlin queries parse their source and, for dot
    /// recovery, synthetic source. Closed buffers read the current text from disk.
    pub fn complete(
        &mut self,
        key: &str,
        offset: usize,
        snippets_supported: bool,
    ) -> Option<ShapedCompletions> {
        if let Some(completions) = self.contributed_completion(key, offset) {
            return Some(completions);
        }
        let query = self.completion_query(key, offset)?;
        Self::shape_completion_query(key, query, snippets_supported)
    }

    /// Keep the LSP trace summary and rendered items from one ordinary query.
    /// Preserve the LSP's query-before-provider order; `complete` keeps its public behavior.
    pub(crate) fn complete_with_semantic(
        &mut self,
        key: &str,
        offset: usize,
        snippets_supported: bool,
    ) -> (
        Option<ShapedCompletions>,
        Option<(&'static str, &'static str, Vec<String>)>,
    ) {
        let query = self.completion_query(key, offset);
        let semantic = query.as_ref().map(|query| {
            (
                query.context_label(),
                query.status_label(),
                query.reason_labels(),
            )
        });
        let shaped = self.contributed_completion(key, offset).or_else(|| {
            query.and_then(|query| Self::shape_completion_query(key, query, snippets_supported))
        });
        (shaped, semantic)
    }

    fn shape_completion_query(
        key: &str,
        query: semantic_query::CompletionQuery,
        snippets_supported: bool,
    ) -> Option<ShapedCompletions> {
        if matches!(
            query.context,
            complete::CompletionContext::Import | complete::CompletionContext::None
        ) {
            return None;
        }

        let mut shaped = complete::shape(
            query.context,
            &query.prefix,
            query.candidates,
            snippets_supported,
            match language::SourceLanguage::for_key(key) {
                language::SourceLanguage::Java => crate::imports::ImportStyle::Java,
                language::SourceLanguage::Kotlin => crate::imports::ImportStyle::Kotlin,
            },
        );
        // Resolve each surviving item's auto-import line from the file's import layout. `shape`
        // leaves `ImportEdit.line` at 0 (the text is set); the line depends on the live tree, so it
        // is resolved here (where the layout is known).
        if let Some((sorted_imports, anchor)) = query.layout.as_ref() {
            complete::resolve_auto_import_lines(&mut shaped, sorted_imports, *anchor);
        }
        // Silent omission: an empty result is never a "success".
        (!shaped.items.is_empty()).then_some(shaped)
    }

    fn contributed_completion(&self, key: &str, offset: usize) -> Option<ShapedCompletions> {
        let index = self.index.scoped_for_file(key);
        let mut items = Vec::new();
        let mut site_present = false;
        for set in index.contributions_for_file(key) {
            for (site, resolution) in set.sites_at(offset) {
                if !(site.editable_name.start <= offset && offset <= site.editable_name.end) {
                    continue;
                }
                site_present = true;
                if !resolution.requirement.target.is_found() {
                    continue;
                }
                for candidate in &resolution.candidates {
                    if !candidate.mismatches.is_empty()
                        || !candidate.signature.is_found()
                        || !candidate.dispatch.is_found()
                    {
                        continue;
                    }
                    if !index.lookup_by_name(&candidate.name).iter().any(|entry| {
                        entry.path.as_ref() == candidate.id.definition().file
                            && entry.sym.start_byte == candidate.id.definition().start_byte
                            && entry.sym.end_byte == candidate.id.definition().end_byte
                    }) {
                        continue;
                    }
                    let label = site.projection.apply(&candidate.name);
                    items.push(complete::ShapedItem {
                        label: label.clone(),
                        sort_text: label.clone(),
                        filter_text: label,
                        kind: match candidate.kind {
                            ktcore::extensions::MemberKind::Method => SymbolKind::Function,
                            ktcore::extensions::MemberKind::Field => SymbolKind::Property,
                        },
                        is_keyword: false,
                        insert_text: candidate.name.clone(),
                        is_snippet: false,
                        detail: Some(format!(
                            "{} {}",
                            candidate.id.owner().qualified_name(),
                            candidate.id.signature().unwrap().descriptor()
                        )),
                        auto_import: None,
                        replace_range: Some(site.editable_name.clone()),
                    });
                }
            }
        }
        items.sort_by(|a, b| a.label.cmp(&b.label));
        items.dedup_by(|a, b| a.label == b.label && a.replace_range == b.replace_range);
        site_present.then_some(ShapedCompletions {
            items,
            is_incomplete: true,
        })
    }

    /// Declarations for `textDocument/documentSymbol` and future passive symbol features. Results
    /// are flat, source-ordered summaries over the current authoritative text for `key`.
    pub fn document_symbols(&self, key: &str) -> Vec<SymbolSummary> {
        self.index
            .entries_for_file(key)
            .iter()
            .map(SymbolSummary::from_entry)
            .collect()
    }

    /// The indexed symbol resolved at `offset`, for hover and future symbol-aware features. Local
    /// declarations are intentionally omitted for now because they are not in the cross-file index.
    pub fn symbol_at(&mut self, key: &str, offset: usize) -> Option<SymbolSummary> {
        self.refresh_compiled_freshness();
        self.resolved_symbol_query(key, offset)?.symbol_summary()
    }

    /// Project/library symbols matching `query`, capped and ordered for workspace/symbol.
    pub fn workspace_symbols(&self, query: &str) -> Vec<SymbolSummary> {
        const CAP: usize = 200;
        let query = query.to_lowercase();
        let mut out: Vec<_> = self
            .index
            .entries_with_prefix("", false)
            .filter(|entry| {
                crate::symbols::matches_lowercase_query(
                    &entry.sym.name,
                    &entry.sym.package,
                    entry.sym.container.as_deref(),
                    &query,
                )
            })
            .collect();
        out.sort_by(|a, b| {
            tier_rank(a.tier)
                .cmp(&tier_rank(b.tier))
                .then(a.sym.name.len().cmp(&b.sym.name.len()))
                .then(a.sym.name.to_lowercase().cmp(&b.sym.name.to_lowercase()))
                .then(a.sym.package.cmp(&b.sym.package))
                .then(a.path.cmp(&b.path))
                .then(a.sym.start_byte.cmp(&b.sym.start_byte))
                // Preserve the former all_entries order when the feature's ranking ties.
                .then(a.sym.end_byte.cmp(&b.sym.end_byte))
                .then(a.sym.name.cmp(&b.sym.name))
        });
        out.truncate(CAP);
        out.into_iter().map(SymbolSummary::from_entry).collect()
    }

    /// Same-file highlights for the exact symbol at `offset`, using goto-grade reference filtering.
    pub fn document_highlights(&mut self, key: &str, offset: usize) -> Vec<Def> {
        self.references(key, offset, true)
            .into_iter()
            .filter(|d| d.file == key)
            .collect()
    }

    /// `textDocument/foldingRange`: AST-only folds over the current authoritative document.
    pub fn folding_ranges(&mut self, key: &str) -> Vec<FoldRange> {
        if let Some(doc) = self.open_docs.get(key) {
            return ranges::folding_ranges(&doc.tree, &doc.text);
        }
        let (text, tree) = match self.parsed_closed_doc(key) {
            Some(parsed) => parsed,
            None => return Vec::new(),
        };
        ranges::folding_ranges(&tree, &text)
    }

    /// `textDocument/selectionRange`: one parent chain for each requested byte offset.
    pub fn selection_ranges(
        &mut self,
        key: &str,
        offsets: &[usize],
    ) -> Vec<Option<SelectionRange>> {
        if let Some(doc) = self.open_docs.get(key) {
            return offsets
                .iter()
                .map(|offset| ranges::selection_range(&doc.tree, &doc.text, *offset))
                .collect();
        }
        let (text, tree) = match self.parsed_closed_doc(key) {
            Some(parsed) => parsed,
            None => return Vec::new(),
        };
        offsets
            .iter()
            .map(|offset| ranges::selection_range(&tree, &text, *offset))
            .collect()
    }

    /// `textDocument/semanticTokens/full`: parser-only semantic classifications.
    pub fn semantic_tokens(&mut self, key: &str) -> Vec<semantic::SemanticToken> {
        if let Some(doc) = self.open_docs.get(key) {
            return language::semantic_tokens(SourceLanguage::for_key(key), &doc.tree, &doc.text);
        }
        let (text, tree) = match self.parsed_closed_doc(key) {
            Some(parsed) => parsed,
            None => return Vec::new(),
        };
        language::semantic_tokens(SourceLanguage::for_key(key), &tree, &text)
    }

    /// `textDocument/inlayHint`: conservative type hints within the requested byte range.
    pub fn inlay_hints(
        &mut self,
        key: &str,
        start_byte: usize,
        end_byte: usize,
    ) -> Vec<crate::hints::InlayHint> {
        self.refresh_compiled_freshness();
        if let Some(doc) = self.open_docs.get(key) {
            return self.with_java_member_proofs(key, |index| {
                language::inlay_hints(
                    SourceLanguage::for_key(key),
                    index,
                    key,
                    &doc.tree,
                    &doc.text,
                    start_byte,
                    end_byte,
                )
            });
        }
        let (text, tree) = match self.parsed_closed_doc(key) {
            Some(parsed) => parsed,
            None => return Vec::new(),
        };
        self.with_java_member_proofs(key, |index| {
            language::inlay_hints(
                SourceLanguage::for_key(key),
                index,
                key,
                &tree,
                &text,
                start_byte,
                end_byte,
            )
        })
    }

    /// `textDocument/prepareRename`: exact range + current spelling for project/local symbols.
    pub fn prepare_rename(
        &mut self,
        key: &str,
        offset: usize,
    ) -> Option<crate::rename::PreparedRename> {
        self.refresh_compiled_freshness();
        if let Some(subject) = self.extension_rename_subject(key, offset) {
            let range = self.extension_rename_range_at(key, offset, &subject)?;
            let text = self.doc_text(key)?;
            return Some(crate::rename::PreparedRename {
                placeholder: text.get(range.clone())?.to_string(),
                range: Def {
                    file: key.into(),
                    start_byte: range.start,
                    end_byte: range.end,
                },
            });
        }
        if let Some(member) = self.jvm_rename_subject(key, offset) {
            let fixed = member.fixed_name.as_ref()?;
            member.identity.as_ref()?;
            if member.tier == Tier::Durable {
                return None;
            }
            let range = if fixed.definition.file == key
                && fixed.definition.start_byte <= offset
                && offset <= fixed.definition.end_byte
            {
                fixed.definition.clone()
            } else {
                let site = self.index.lookup_refs(&member.name).iter().find(|site| {
                    site.path.as_ref() == key
                        && site.start_byte <= offset
                        && offset <= site.end_byte
                })?;
                Def {
                    file: key.into(),
                    start_byte: site.start_byte,
                    end_byte: site.end_byte,
                }
            };
            return Some(crate::rename::PreparedRename {
                range,
                placeholder: member.name,
            });
        }
        let target = self.rename_target(key, offset)?;
        self.rename_member_family(&target)?;
        let text = self.doc_text(&target.file)?;
        let placeholder = text.get(target.start_byte..target.end_byte)?.to_string();
        let range = if let Some(doc) = self.open_docs.get(key) {
            self.identifier_at_key(key, &doc.tree, offset)?.byte_range()
        } else {
            let (_, tree) = self.parsed_closed_doc(key)?;
            self.identifier_at_key(key, &tree, offset)?.byte_range()
        };
        Some(crate::rename::PreparedRename {
            range: Def {
                file: key.to_string(),
                start_byte: range.start,
                end_byte: range.end,
            },
            placeholder,
        })
    }

    /// `textDocument/rename`: exact reference edits for project/local symbols.
    pub fn rename(
        &mut self,
        key: &str,
        offset: usize,
        new_name: &str,
    ) -> Option<Vec<crate::edit::TextEdit>> {
        self.refresh_compiled_freshness();
        let valid_input = language::is_valid_identifier(SourceLanguage::for_key(key), new_name);
        if let Some(subject) = self.extension_rename_subject(key, offset) {
            if !valid_input {
                return None;
            }
            return self.rename_contribution(key, offset, subject, new_name);
        }
        if let Some(member) = self.jvm_rename_subject(key, offset) {
            if !valid_input {
                return None;
            }
            return self.rename_jvm_alias(key, offset, member, new_name);
        }
        let target = self.rename_target(key, offset)?;
        let target_language = if !SourceLanguage::for_key(&target.file).is_java()
            && hierarchy::entry_for_name_range(
                &self.index,
                &target.file,
                target.start_byte,
                target.end_byte,
            )
            .is_some_and(|entry| entry.sym.kind == SymbolKind::Property)
        {
            SourceLanguage::Kotlin
        } else {
            SourceLanguage::for_key(key)
        };
        if !language::is_valid_identifier(target_language, new_name) {
            return None;
        }
        let name = self.name_at(key, offset)?;
        if self.index.lookup_refs(&name).len() > RENAME_REF_CAP {
            return None;
        }
        let family_start = std::time::Instant::now();
        let family = self.rename_member_family(&target);
        if crate::trace::enabled() {
            crate::trace::span(
                "workspace.rename.member_family",
                "workspace",
                family_start,
                serde_json::json!({
                    "file": key,
                    "targetFile": target.file,
                    "symbol": name,
                    "found": family.is_some(),
                    "members": family.as_ref().map_or(0, Vec::len),
                }),
            );
        }
        let family = family?;
        if family.len() > 1 {
            let candidates = self.index.lookup_refs(&name).to_vec();
            for candidate in candidates {
                let definitions = self.goto_definition(&candidate.path, candidate.start_byte);
                if definitions.len() > 1
                    && definitions
                        .iter()
                        .any(|definition| family.contains(definition))
                {
                    return None;
                }
            }
        }
        let mut refs = BTreeSet::new();
        for member in family {
            for site in self.references_with_policy(&member.file, member.start_byte, true, true)? {
                // A fixed JVM export name is independent of the authored Kotlin identifier.
                if self
                    .jvm_member_at(&site.file, site.start_byte)
                    .is_none_or(|member| member.fixed_name.is_none())
                {
                    refs.insert(site);
                }
            }
        }
        if refs.is_empty() || refs.len() > RENAME_REF_CAP || !refs.iter().any(|r| *r == target) {
            return None;
        }
        let property = self
            .index
            .jvm_source_for_file(&target.file)
            .is_some_and(|source| {
                source.facts.callables.iter().any(|facts| {
                    facts.source_property.is_some() && facts.definition.as_ref() == Some(&target)
                })
            });
        if hierarchy::entry_for_name_range(
            &self.index,
            &target.file,
            target.start_byte,
            target.end_byte,
        )
        .is_some_and(|entry| ktcore::members::kotlin_java_getter_property_name(&entry).is_some())
            && refs.iter().any(|site| {
                self.doc_text(&site.file).is_none_or(|text| {
                    text.get(site.start_byte..site.end_byte) != Some(name.as_str())
                })
            })
        {
            return None;
        }
        let edits = if property {
            let text = self.doc_text(&target.file)?;
            let property_name = text.get(target.start_byte..target.end_byte)?.to_string();
            let entry = hierarchy::entry_for_name_range(
                &self.index,
                &target.file,
                target.start_byte,
                target.end_byte,
            )?;
            self.rename_property_family_isolated(&entry, new_name)?;
            self.rename_property_argument_labels(&target, &property_name, &mut refs)?;
            refs.into_iter()
                .map(|site| {
                    let text = self.doc_text(&site.file)?;
                    let current = text.get(site.start_byte..site.end_byte)?;
                    let spelling = crate::rename::property_edit_spelling(
                        &property_name,
                        current,
                        new_name,
                        SourceLanguage::for_key(&site.file).is_java(),
                    )?;
                    Some(crate::edit::TextEdit::new(
                        site.file,
                        site.start_byte,
                        site.end_byte,
                        spelling,
                    ))
                })
                .collect::<Option<Vec<_>>>()?
        } else {
            crate::rename::edits_for_refs(refs.into_iter().collect(), new_name)
        };
        crate::edit::validate_non_overlapping(&edits).ok()?;
        for file in edits
            .iter()
            .map(|edit| edit.file.clone())
            .collect::<BTreeSet<_>>()
        {
            if self
                .index
                .entries_for_file(&file)
                .iter()
                .any(|entry| entry.tier == Tier::Durable)
                || edits.iter().filter(|edit| edit.file == file).any(|edit| {
                    !language::is_valid_identifier(SourceLanguage::for_key(&file), &edit.new_text)
                })
            {
                return None;
            }
            crate::edit::apply_to_text(&file, &self.doc_text(&file)?, &edits).ok()?;
        }
        Some(edits)
    }

    fn rename_property_argument_labels(
        &mut self,
        target: &Def,
        property: &str,
        refs: &mut BTreeSet<Def>,
    ) -> Option<()> {
        let entry = hierarchy::entry_for_name_range(
            &self.index,
            &target.file,
            target.start_byte,
            target.end_byte,
        )?;
        let tree = if let Some(doc) = self.open_docs.get(&target.file) {
            doc.tree.clone()
        } else {
            self.parsed_closed_doc(&target.file)?.1
        };
        let mut node = tree
            .root_node()
            .named_descendant_for_byte_range(target.start_byte, target.end_byte)?;
        while !matches!(node.kind(), "class_parameter" | "property_declaration") {
            node = node.parent()?;
        }
        if node.kind() != "class_parameter" {
            return Some(());
        }
        let owner = self.rename_member_owner(&entry)?;
        while node.kind() != "class_declaration" {
            node = node.parent()?;
        }
        let mut pending = vec![node];
        while let Some(node) = pending.pop() {
            // A class identity alone cannot select one of several constructor parameter lists.
            if node.kind() == "secondary_constructor" {
                return None;
            }
            pending.extend(node.named_children(&mut node.walk()));
        }
        if self
            .index
            .lookup_by_name(&owner.sym.name)
            .iter()
            .any(|candidate| {
                candidate.sym.kind == SymbolKind::Function
                    && candidate.sym.container.is_none()
                    && candidate.sym.package == owner.sym.package
            })
        {
            return None;
        }
        let owner_def = rename_definition(&owner);
        let names = self.reference_names(&owner_def);
        let (candidates, _) = self.reference_candidates(&owner_def, &names, true)?;
        if candidates.len() + refs.len() > RENAME_REF_CAP {
            return None;
        }
        for site in candidates {
            if SourceLanguage::for_key(&site.path).is_java() {
                continue;
            }
            if self.goto_definition(&site.path, site.start_byte).as_slice() != [owner_def.clone()] {
                continue;
            }
            let (text, tree) = if let Some(doc) = self.open_docs.get(site.path.as_ref()) {
                (doc.text.clone(), doc.tree.clone())
            } else {
                self.parsed_closed_doc(&site.path)?
            };
            let definition = Def {
                file: site.path.to_string(),
                start_byte: site.start_byte,
                end_byte: site.end_byte,
            };
            for range in
                crate::rename::constructor_argument_labels(&tree, &definition, property, &text)
            {
                refs.insert(Def {
                    file: site.path.to_string(),
                    start_byte: range.start,
                    end_byte: range.end,
                });
                if refs.len() > RENAME_REF_CAP {
                    return None;
                }
            }
        }
        Some(())
    }

    fn rename_property_family_isolated(
        &mut self,
        property: &crate::index::Entry,
        spelling: &str,
    ) -> Option<()> {
        if property.sym.source_member.visibility() == Some(ktcore::extensions::Visibility::Private)
        {
            return Some(());
        }
        let owner = self.rename_member_owner(property)?;
        // Override families need a shared accessor-family identity before atomic rename.
        if !owner.sym.supertypes.is_empty() {
            return None;
        }
        let names = [
            spelling.to_string(),
            ktcore::jvm::property_getter_name(spelling)?,
            ktcore::jvm::property_setter_name(spelling)?,
        ];
        let candidates: Vec<_> = names
            .iter()
            .flat_map(|name| self.index.lookup_by_name(name))
            .cloned()
            .collect();
        if candidates.len() > RENAME_REF_CAP {
            return None;
        }
        let mut pending = VecDeque::from([hierarchy::item_from_entry(&owner)]);
        let mut seen = BTreeSet::new();
        while let Some(owner) = pending.pop_front() {
            if !seen.insert((owner.file.clone(), owner.start_byte, owner.end_byte)) {
                continue;
            }
            if seen.len() > RENAME_REF_CAP {
                return None;
            }
            let owner_entry = hierarchy::entry_for_name_range(
                &self.index,
                &owner.file,
                owner.start_byte,
                owner.end_byte,
            )?;
            if owner_entry.tier == Tier::Durable {
                return None;
            }
            let owner_name = owner_entry.sym.container.as_ref().map_or_else(
                || owner.name.clone(),
                |parent| format!("{parent}.{}", owner.name),
            );
            if candidates.iter().any(|candidate| {
                rename_definition(candidate) != rename_definition(property)
                    && candidate.path.as_ref() == owner.file
                    && candidate.sym.package == owner.package
                    && candidate.sym.container.as_deref() == Some(owner_name.as_str())
                    && (candidate.sym.kind == SymbolKind::Property
                        || candidate.sym.kind == SymbolKind::Function
                            && (candidate.sym.name == names[1]
                                && candidate.sym.arity.is_none_or(|arity| arity == 0)
                                || candidate.sym.name == names[2]
                                    && candidate.sym.arity.is_none_or(|arity| arity == 1)))
            }) {
                return None;
            }
            pending.extend(hierarchy::subtypes(&self.index, &owner));
        }
        Some(())
    }

    fn rename_member_family(&mut self, target: &Def) -> Option<Vec<Def>> {
        use ktcore::extensions::{Dispatch, Visibility};
        let Some(entry) = hierarchy::entry_for_name_range(
            &self.index,
            &target.file,
            target.start_byte,
            target.end_byte,
        ) else {
            return Some(vec![target.clone()]);
        };
        if entry.sym.kind == SymbolKind::Property
            && entry.sym.container.is_some()
            && !SourceLanguage::for_key(&entry.path).is_java()
        {
            self.rename_property_family_isolated(&entry, &entry.sym.name)?;
        }
        if entry.sym.kind != SymbolKind::Function || entry.sym.container.is_none() {
            return Some(vec![target.clone()]);
        }
        let mut source_facts = BTreeMap::new();
        let flags = self
            .rename_member_facts(&entry, &mut source_facts)
            .and_then(|facts| facts.member);
        if flags.is_some_and(|facts| {
            matches!(facts.dispatch, Knowledge::Found(Dispatch::Static))
                || matches!(facts.visibility, Knowledge::Found(Visibility::Private))
        }) {
            return Some(vec![target.clone()]);
        }
        let candidates: Vec<_> = self
            .index
            .lookup_by_name(&entry.sym.name)
            .into_iter()
            .filter(|candidate| {
                candidate.sym.kind == SymbolKind::Function && candidate.sym.container.is_some()
            })
            .cloned()
            .collect();
        if candidates.len() > RENAME_REF_CAP {
            return None;
        }
        let mut family = vec![entry];
        let mut definitions = BTreeSet::from([target.clone()]);
        let mut next = 0;
        while next < family.len() {
            let current = family[next].clone();
            next += 1;
            for candidate in &candidates {
                let definition = rename_definition(candidate);
                if definitions.contains(&definition) {
                    continue;
                }
                let mut connected = false;
                for (child, parent) in [(candidate, &current), (&current, candidate)] {
                    let Some(owner) = self.rename_member_owner(child) else {
                        return None;
                    };
                    let receiver = rename_type(&owner);
                    let scoped = self.index.scoped_for_file(&child.path);
                    if !scoped
                        .lookup_by_name(&parent.sym.name)
                        .iter()
                        .any(|visible| rename_definition(visible) == rename_definition(parent))
                    {
                        continue;
                    }
                    let ctx = infer::FileCtx::new(child.sym.package.clone(), Vec::new());
                    let relation = ktcore::members::inherited_method_relation(
                        &scoped, &receiver, child, parent, &ctx,
                    );
                    if relation.is_definitely_absent() {
                        continue;
                    }
                    let child_flags = self.rename_member_facts(child, &mut source_facts)?.member?;
                    let parent_flags = self
                        .rename_member_facts(parent, &mut source_facts)?
                        .member?;
                    let (
                        Knowledge::Found(child_dispatch),
                        Knowledge::Found(parent_dispatch),
                        Knowledge::Found(child_visibility),
                        Knowledge::Found(parent_visibility),
                    ) = (
                        child_flags.dispatch,
                        parent_flags.dispatch,
                        child_flags.visibility,
                        parent_flags.visibility,
                    )
                    else {
                        return None;
                    };
                    if child_dispatch != Dispatch::Instance
                        || parent_dispatch != Dispatch::Instance
                        || child_visibility == Visibility::Private
                        || parent_visibility == Visibility::Private
                        || parent_visibility == Visibility::Package
                            && child.sym.package != parent.sym.package
                    {
                        continue;
                    }
                    if !relation.is_found()
                        || child.tier == Tier::Durable
                        || parent.tier == Tier::Durable
                    {
                        return None;
                    }
                    connected = true;
                }
                if connected {
                    definitions.insert(definition);
                    family.push(candidate.clone());
                }
            }
        }
        // An inherited implementation can satisfy an unrelated interface owner. Until that edge
        // has a shared identity, refuse a partial rename whenever a common descendant exposes it.
        let mut owners = VecDeque::new();
        for member in &family {
            owners.push_back(hierarchy::item_from_entry(
                &self.rename_member_owner(member)?,
            ));
        }
        let mut visited = BTreeSet::new();
        while let Some(owner) = owners.pop_front() {
            if !visited.insert((owner.file.clone(), owner.start_byte, owner.end_byte)) {
                continue;
            }
            if visited.len() > RENAME_REF_CAP {
                return None;
            }
            owners.extend(hierarchy::subtypes(&self.index, &owner));
            let entry = hierarchy::entry_for_name_range(
                &self.index,
                &owner.file,
                owner.start_byte,
                owner.end_byte,
            )?;
            let receiver = rename_type(&entry);
            let scoped = self.index.scoped_for_file(&owner.file);
            let ctx = infer::FileCtx::new(owner.package.clone(), Vec::new());
            let members: Vec<_> =
                ktcore::members::candidates(&scoped, &receiver, &family[0].sym.name, &ctx)
                    .members
                    .into_iter()
                    .cloned()
                    .collect();
            // Renaming only the authored ancestor would change dispatch through that ancestor
            // while leaving a generated override unchanged. It has no editable source range.
            if scoped.may_have_compiled_callables()
                && ktcore::jvm::member_candidates(&scoped, &receiver, &family[0].sym.name)
                    .into_iter()
                    .any(|compiled| {
                        compiled.definition().is_none()
                            && members.iter().any(|member| {
                                definitions.contains(&rename_definition(member))
                                    && ktcore::jvm::compiled_overrides_authored(
                                        &scoped, &receiver, compiled, member,
                                    )
                            })
                    })
            {
                return None;
            }
            let Some(implementation) = members
                .iter()
                .find(|member| definitions.contains(&rename_definition(member)))
            else {
                continue;
            };
            let signature =
                ktcore::members::method_signature(&scoped, &receiver, implementation, &ctx);
            let competing: Vec<_> = members
                .iter()
                .filter(|member| {
                    !definitions.contains(&rename_definition(member))
                        && member.sym.kind == SymbolKind::Function
                })
                .map(|candidate| {
                    (
                        candidate.clone(),
                        ktcore::members::method_signature(&scoped, &receiver, candidate, &ctx),
                    )
                })
                .collect();
            for (candidate, candidate_signature) in &competing {
                if implementation
                    .sym
                    .arity
                    .zip(candidate.sym.arity)
                    .is_some_and(|(own, other)| own != other)
                    || matches!((&signature, candidate_signature), (Some(own), Some(other)) if own != other)
                {
                    continue;
                }
                let flags = self
                    .rename_member_facts(candidate, &mut source_facts)?
                    .member?;
                match (flags.dispatch, flags.visibility) {
                    (Knowledge::Found(Dispatch::Static), _)
                    | (_, Knowledge::Found(Visibility::Private)) => continue,
                    (
                        Knowledge::Found(Dispatch::Instance),
                        Knowledge::Found(Visibility::Package),
                    ) if candidate.sym.package != owner.package => continue,
                    _ => return None,
                }
            }
        }
        Some(definitions.into_iter().collect())
    }

    fn rename_member_facts(
        &mut self,
        entry: &crate::index::Entry,
        files: &mut BTreeMap<String, Vec<ktcore::extensions::source::SourceDeclaration>>,
    ) -> Option<ktcore::extensions::source::SourceDeclaration> {
        if !files.contains_key(entry.path.as_ref()) {
            let (text, tree) = if let Some(doc) = self.open_docs.get(entry.path.as_ref()) {
                (doc.text.clone(), doc.tree.clone())
            } else {
                self.parsed_closed_doc(&entry.path)?
            };
            let scoped = self.index.scoped_for_file(&entry.path);
            files.insert(
                entry.path.to_string(),
                language::extension_source_facts(&scoped, &entry.path, &tree, &text),
            );
        }
        files
            .get(entry.path.as_ref())?
            .iter()
            .find(|fact| fact.definition == rename_definition(entry))
            .cloned()
    }

    fn rename_member_owner(&self, member: &crate::index::Entry) -> Option<crate::index::Entry> {
        let owner = member.sym.container.as_deref()?;
        let (container, name) = owner
            .rsplit_once('.')
            .map_or((None, owner), |(container, name)| (Some(container), name));
        let scoped = self.index.scoped_for_file(&member.path);
        let candidates: Vec<_> = scoped
            .lookup_type(name)
            .into_iter()
            .filter(|entry| {
                entry.path == member.path
                    && entry.sym.package == member.sym.package
                    && entry.sym.container.as_deref() == container
            })
            .collect();
        let [entry] = candidates.as_slice() else {
            return None;
        };
        Some((*entry).clone())
    }

    fn jvm_member_at(&mut self, key: &str, offset: usize) -> Option<ktcore::jvm::ResolvedMember> {
        if let Some(doc) = self.open_docs.get(key) {
            return self.with_java_member_proofs(key, |index| {
                language::jvm_member_at(index, key, &doc.text, &doc.tree, offset)
            });
        }
        let (text, tree) = self.parsed_closed_doc(key)?;
        self.with_java_member_proofs(key, |index| {
            language::jvm_member_at(index, key, &text, &tree, offset)
        })
    }

    fn jvm_rename_subject(
        &mut self,
        key: &str,
        offset: usize,
    ) -> Option<ktcore::jvm::ResolvedMember> {
        if let Some(member) = self
            .jvm_member_at(key, offset)
            .filter(|member| member.fixed_name.is_some())
        {
            return Some(member);
        }
        let source = self.index.jvm_source_for_file(key)?;
        let facts = source.facts.callables.iter().find(|facts| {
            facts.fixed_name.as_ref().is_some_and(|fixed| {
                fixed.definition.start_byte <= offset && offset <= fixed.definition.end_byte
            })
        })?;
        self.with_java_member_proofs(key, |index| {
            ktcore::jvm::CallableCandidate::Jvm {
                owner: ktcore::jvm::JvmOwner::Source(source),
                facts,
            }
            .resolved(index)
        })
    }

    fn rename_jvm_alias(
        &mut self,
        key: &str,
        offset: usize,
        member: ktcore::jvm::ResolvedMember,
        new_name: &str,
    ) -> Option<Vec<crate::edit::TextEdit>> {
        if member.tier == Tier::Durable {
            return None;
        }
        let identity = member.identity.as_ref()?;
        let fixed = member.fixed_name.as_ref()?;
        let target = member.definition.as_ref()?;
        let source = self.doc_text(&fixed.definition.file)?;
        // Validate the complete currently accepted annotation, not merely an equal substring.
        let tree =
            self.salsa
                .get_mut()
                .parsed_tree(&fixed.definition.file, &source, Durability::LOW);
        let symbols = language::symbol_facts(SourceLanguage::Kotlin, &tree, &source).symbols;
        let verified = ktcore::jvm::source_facts(&fixed.definition.file, &tree, &source, &symbols)?;
        if !verified.callables.iter().any(|facts| {
            facts.definition.as_ref() == Some(target)
                && facts.name == member.name
                && facts.fixed_name.as_ref() == Some(fixed)
        }) {
            return None;
        }
        let mut sites =
            self.references_with_policy(&target.file, target.start_byte, false, true)?;
        sites.retain(|site| {
            self.jvm_member_at(&site.file, site.start_byte)
                .is_some_and(|candidate| candidate.identity.as_ref() == Some(identity))
        });
        // Renaming directly on the annotation has no identifier/goto usage at the cursor.
        if key != fixed.definition.file
            && !sites.iter().any(|site| {
                site.file == key && site.start_byte <= offset && offset <= site.end_byte
            })
        {
            return None;
        }
        sites.push(fixed.definition.clone());
        let edits = crate::rename::edits_for_refs(sites, new_name);
        crate::edit::validate_non_overlapping(&edits).ok()?;
        Some(edits)
    }

    fn authored_goto(&mut self, key: &str, offset: usize) -> Vec<Def> {
        if let Some(doc) = self.open_docs.get(key) {
            return language::authored_goto_definition(
                &self.index.scoped_for_file(key),
                SourceLanguage::for_key(key),
                key,
                &doc.text,
                &doc.tree,
                offset,
            );
        }
        let Some((text, tree)) = self.parsed_closed_doc(key) else {
            return Vec::new();
        };
        language::authored_goto_definition(
            &self.index.scoped_for_file(key),
            SourceLanguage::for_key(key),
            key,
            &text,
            &tree,
            offset,
        )
    }

    fn extension_rename_subject(&mut self, key: &str, offset: usize) -> Option<RenameTarget> {
        let scoped = self.index.scoped_for_file(key);
        if scoped.semantic_binding_at(key, offset).is_some() {
            let mut selected = None;
            for set in scoped.contributions_for_file(key) {
                if let Some(subject) = set.rename_subject_at(offset) {
                    if selected.as_ref().is_some_and(|old| old != &subject) {
                        return None;
                    }
                    selected = Some(subject);
                }
            }
            if selected.is_some() {
                return selected;
            }
        }
        let authored = self.authored_goto(key, offset);
        let [authored] = authored.as_slice() else {
            return None;
        };
        if let Some(binding) = self.index.scoped_for_file(key).semantic_binding(authored) {
            return match binding.name_link {
                NameLink::IndependentAlias => {
                    binding.authored.clone().map(RenameTarget::Declaration)
                }
                _ => Some(RenameTarget::Member(binding.target.clone())),
            };
        }
        let mut selected = None;
        for (owner, set) in self.analysis_extensions.sets() {
            for resolution in set.requirements() {
                let Knowledge::Found(binding) = &resolution.binding else {
                    continue;
                };
                if binding.target.definition() != authored {
                    continue;
                }
                if let Some(source) = &binding.authored {
                    if self
                        .index
                        .scoped_for_file(owner)
                        .semantic_binding(source.definition())
                        != Some(binding)
                    {
                        continue;
                    }
                }
                let subject = RenameTarget::Member(binding.target.clone());
                if selected.as_ref().is_some_and(|old| old != &subject) {
                    return None;
                }
                selected = Some(subject);
            }
        }
        selected
    }

    fn extension_rename_range_at(
        &mut self,
        key: &str,
        offset: usize,
        subject: &RenameTarget,
    ) -> Option<std::ops::Range<usize>> {
        for set in self.index.contributions_for_file(key) {
            if set.rename_subject_at(offset).as_ref() == Some(subject) {
                return set
                    .sites_at(offset)
                    .next()
                    .map(|(site, _)| site.editable_name.clone());
            }
        }
        let parsed;
        let tree = if let Some(doc) = self.open_docs.get(key) {
            &doc.tree
        } else {
            parsed = self.parsed_closed_doc(key)?;
            &parsed.1
        };
        let ident = self.identifier_at_key(key, tree, offset)?;
        let site = Def {
            file: key.into(),
            start_byte: ident.start_byte(),
            end_byte: ident.end_byte(),
        };
        self.extension_usage_range(&site, subject)
    }

    fn extension_usage_range(
        &mut self,
        site: &Def,
        subject: &RenameTarget,
    ) -> Option<std::ops::Range<usize>> {
        let defs = self.authored_goto(&site.file, site.start_byte);
        let [definition] = defs.as_slice() else {
            return None;
        };
        let prefix = match subject {
            RenameTarget::Declaration(authored) if authored.definition() == definition => {
                String::new()
            }
            RenameTarget::Member(target) if target.definition() == definition => String::new(),
            RenameTarget::Member(target) => {
                let scoped = self.index.scoped_for_file(&site.file);
                let binding = scoped.semantic_binding(definition)?;
                if &binding.target != target {
                    return None;
                }
                match &binding.name_link {
                    NameLink::Direct => String::new(),
                    NameLink::Prefixed(prefix) => prefix.clone(),
                    NameLink::IndependentAlias => return None,
                }
            }
            _ => return None,
        };
        let text = self.doc_text(&site.file)?;
        let spelling = text.get(site.start_byte..site.end_byte)?;
        let escaped = spelling.starts_with('`') && spelling.ends_with('`');
        let start = site.start_byte + usize::from(escaped);
        let end = site.end_byte - usize::from(escaped);
        text.get(start..end)?.strip_prefix(&prefix)?;
        Some(start + prefix.len()..end)
    }

    fn rename_contribution(
        &mut self,
        key: &str,
        offset: usize,
        subject: RenameTarget,
        new_name: &str,
    ) -> Option<Vec<crate::edit::TextEdit>> {
        let definition = match &subject {
            RenameTarget::Member(id) => id.definition(),
            RenameTarget::Declaration(id) => id.definition(),
        }
        .clone();
        if self.is_library_def(&definition) {
            return None;
        }
        let names = self.reference_names(&definition);
        if names
            .iter()
            .map(|name| self.index.lookup_refs(name).len())
            .sum::<usize>()
            > RENAME_REF_CAP
        {
            return None;
        }
        let mut edits = Vec::new();
        let mut explicit = Vec::new();
        for (owner, set) in self.analysis_extensions.sets() {
            for site in set.sites() {
                explicit.push((owner.to_string(), site.full_range.clone()));
            }
            for replacement in set.rename_edits(&subject, new_name).ok()? {
                let site = set
                    .sites()
                    .iter()
                    .find(|site| site.editable_name == replacement.range)?;
                if self
                    .index
                    .scoped_for_file(owner)
                    .semantic_binding_at(owner, site.full_range.start)
                    .is_some()
                {
                    edits.push(crate::edit::TextEdit::new(
                        owner,
                        replacement.range.start,
                        replacement.range.end,
                        replacement.text,
                    ));
                }
            }
        }
        let mut refs = match &subject {
            RenameTarget::Member(_) => self.references_with_policy(key, offset, true, true)?,
            RenameTarget::Declaration(_) => {
                let mut out = Vec::new();
                for name in &names {
                    let candidates = self.index.lookup_refs(name).to_vec();
                    let mut by_path: HashMap<Arc<str>, Vec<RefEntry>> = HashMap::new();
                    for candidate in candidates {
                        by_path
                            .entry(candidate.path.clone())
                            .or_default()
                            .push(candidate);
                    }
                    for (path, candidates) in by_path {
                        self.collect_refs_in_file(
                            &path,
                            &candidates,
                            &definition,
                            true,
                            true,
                            &mut out,
                        );
                    }
                }
                out
            }
        };
        refs.push(definition);
        refs.sort();
        refs.dedup();
        for site in refs {
            if explicit.iter().any(|(file, range)| {
                file == &site.file && range.start < site.end_byte && site.start_byte < range.end
            }) {
                continue;
            }
            if let Some(range) = self.extension_usage_range(&site, &subject) {
                edits.push(crate::edit::TextEdit::new(
                    site.file,
                    range.start,
                    range.end,
                    new_name,
                ));
            }
        }
        edits.sort_by(|a, b| {
            (&a.file, a.start_byte, a.end_byte, &a.new_text).cmp(&(
                &b.file,
                b.start_byte,
                b.end_byte,
                &b.new_text,
            ))
        });
        edits.dedup();
        if edits.is_empty() {
            return None;
        }
        crate::edit::validate_non_overlapping(&edits).ok()?;
        let files = edits
            .iter()
            .map(|edit| edit.file.clone())
            .collect::<BTreeSet<_>>();
        for file in files {
            if !language::is_valid_identifier(SourceLanguage::for_key(&file), new_name) {
                return None;
            }
            crate::edit::apply_to_text(&file, &self.doc_text(&file)?, &edits).ok()?;
        }
        Some(edits)
    }

    fn rename_target(&mut self, key: &str, offset: usize) -> Option<Def> {
        let definitions = self.goto_definition(key, offset);
        let [target] = definitions.as_slice() else {
            return None;
        };
        let target = target.clone();
        if self.is_library_def(&target) {
            return None;
        }
        let parsed;
        let (doc_text, tree): (&str, &Tree) = if let Some(doc) = self.open_docs.get(key) {
            (&doc.text, &doc.tree)
        } else {
            parsed = self.parsed_closed_doc(key)?;
            (&parsed.0, &parsed.1)
        };
        let ident = self.identifier_at_key(key, tree, offset)?;
        let language = SourceLanguage::for_key(key);
        if language::is_import_or_package_position(language, ident)
            || node_text(ident, doc_text).is_empty()
        {
            return None;
        }
        if let Some(entry) = hierarchy::entry_for_name_range(
            &self.index,
            &target.file,
            target.start_byte,
            target.end_byte,
        ) {
            if let Some(property) = ktcore::members::kotlin_java_getter_property_name(&entry) {
                let sites = self.index.lookup_refs(&property);
                if sites.len() > RENAME_REF_CAP {
                    return None;
                }
                let sites = sites.to_vec();
                for site in sites {
                    if SourceLanguage::for_key(&site.path).is_java() {
                        continue;
                    }
                    if self
                        .goto_definition(&site.path, site.start_byte)
                        .contains(&target)
                    {
                        // Getter/property spellings need different edits; never offer a partial rename.
                        return None;
                    }
                }
            }
        }
        Some(target)
    }

    fn is_library_def(&self, def: &Def) -> bool {
        self.index
            .entries_for_file(&def.file)
            .into_iter()
            .find(|entry| {
                entry.sym.start_byte == def.start_byte && entry.sym.end_byte == def.end_byte
            })
            .is_some_and(|entry| entry.tier == Tier::Durable)
    }

    pub fn implementation(&mut self, key: &str, offset: usize) -> Vec<Def> {
        let Some(target) = self.goto_definition(key, offset).into_iter().next() else {
            return Vec::new();
        };
        let Some(entry) = hierarchy::entry_for_name_range(
            &self.index,
            &target.file,
            target.start_byte,
            target.end_byte,
        ) else {
            return Vec::new();
        };
        hierarchy::implementations(&self.index, &entry)
    }

    pub fn type_definition(&mut self, key: &str, offset: usize) -> Vec<Def> {
        self.refresh_compiled_freshness();
        if let Some(doc) = self.open_docs.get(key) {
            return self.with_java_member_proofs(key, |index| {
                language::type_definition(
                    SourceLanguage::for_key(key),
                    index,
                    key,
                    &doc.tree,
                    &doc.text,
                    offset,
                )
            });
        }
        let (text, tree) = match self.parsed_closed_doc(key) {
            Some(parsed) => parsed,
            None => return Vec::new(),
        };
        self.with_java_member_proofs(key, |index| {
            language::type_definition(
                SourceLanguage::for_key(key),
                index,
                key,
                &tree,
                &text,
                offset,
            )
        })
    }

    pub fn hierarchy_item_at(&mut self, key: &str, offset: usize) -> Option<HierarchyItem> {
        let target = self.goto_definition(key, offset).into_iter().next()?;
        hierarchy::entry_for_name_range(
            &self.index,
            &target.file,
            target.start_byte,
            target.end_byte,
        )
        .map(|entry| hierarchy::item_from_entry(&entry))
    }

    pub fn type_supertypes(&self, item: &HierarchyItem) -> Vec<HierarchyItem> {
        hierarchy::supertypes(&self.index.scoped_for_file(&item.file), item)
    }

    pub fn type_subtypes(&self, item: &HierarchyItem) -> Vec<HierarchyItem> {
        hierarchy::subtypes(&self.index, item)
    }

    pub fn incoming_calls(&mut self, item: &HierarchyItem) -> Vec<IncomingCall> {
        let refs = self.references(&item.file, item.start_byte, true);
        let mut parsers = LanguageParsers::new();
        language::incoming_calls(
            SourceLanguage::for_key(&item.file),
            &self.index,
            item,
            refs,
            |path| {
                let text = self.doc_text(path)?;
                let tree = parsers.parse_for_key(path, &text);
                Some((text, tree))
            },
        )
    }

    pub fn outgoing_calls(&mut self, item: &HierarchyItem) -> Vec<OutgoingCall> {
        if let Some(doc) = self.open_docs.get(&item.file) {
            return self.with_java_member_proofs(&item.file, |index| {
                language::outgoing_calls(
                    SourceLanguage::for_key(&item.file),
                    index,
                    &item.file,
                    &doc.tree,
                    &doc.text,
                    item,
                )
            });
        }
        let (text, tree) = match self.parsed_closed_doc(&item.file) {
            Some(parsed) => parsed,
            None => return Vec::new(),
        };
        self.with_java_member_proofs(&item.file, |index| {
            language::outgoing_calls(
                SourceLanguage::for_key(&item.file),
                index,
                &item.file,
                &tree,
                &text,
                item,
            )
        })
    }

    pub fn signature_help(
        &mut self,
        key: &str,
        offset: usize,
    ) -> Option<crate::signature::SignatureHelp> {
        self.refresh_compiled_freshness();
        let parsed;
        let (doc_text, tree): (&str, &Tree) = if let Some(doc) = self.open_docs.get(key) {
            (&doc.text, &doc.tree)
        } else {
            parsed = self.parsed_closed_doc(key)?;
            (&parsed.0, &parsed.1)
        };
        let language = SourceLanguage::for_key(key);
        let language::SignatureEntries {
            name,
            active_parameter,
            mut entries,
            unqualified,
            projected_signatures,
        } = self.with_java_member_proofs(key, |index| {
            language::signature_entries(index, language, key, doc_text, tree, offset)
        })?;
        if entries.is_empty() && unqualified {
            entries = self
                .index
                .scoped_for_file(key)
                .lookup_by_name(&name)
                .iter()
                .filter(|entry| {
                    entry.sym.kind == SymbolKind::Function && entry.sym.container.is_none()
                })
                .cloned()
                .collect();
        }
        crate::signature::signatures_for_entries_with_extra(
            entries,
            projected_signatures,
            active_parameter,
        )
    }

    /// `textDocument/publishDiagnostics` source: name-based, high-confidence diagnostics for `key`,
    /// over the cached tree for an open buffer (or a one-off parse from disk). Byte-range based; the
    /// LSP layer converts to positions and severities.
    pub fn diagnostics(&mut self, key: &str) -> Vec<crate::diagnostics::Diagnostic> {
        self.refresh_compiled_freshness();
        let mut extra = self
            .analysis_extensions
            .sets()
            .filter(|(owner, _)| *owner == key)
            .flat_map(|(_, set)| set.diagnostics().iter().cloned())
            .collect::<Vec<_>>();
        let canonical = self.current_kotlin_diagnostics(key);
        let fast = if let Some(doc) = self.open_docs.get(key) {
            self.diagnostics_for_tree(key, &doc.text, &doc.tree)
        } else {
            let (text, tree) = match self.parsed_closed_doc(key) {
                Some(parsed) => parsed,
                None => return Vec::new(),
            };
            self.diagnostics_for_tree(key, &text, &tree)
        };
        let fast = Self::reconcile_fast_diagnostics(&canonical, fast);
        extra.extend(canonical);
        extra.extend(fast);
        extra
    }

    fn reconcile_fast_diagnostics(
        canonical: &[crate::diagnostics::Diagnostic],
        mut fast: Vec<crate::diagnostics::Diagnostic>,
    ) -> Vec<crate::diagnostics::Diagnostic> {
        use crate::diagnostics::{DiagnosticCode, Severity};
        if canonical.is_empty() || fast.is_empty() {
            return fast;
        }
        let current_errors: HashSet<_> = canonical
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Error)
            .map(|diagnostic| (diagnostic.start_byte, diagnostic.end_byte))
            .collect();
        fast.retain(|diagnostic| {
            diagnostic.code != Some(DiagnosticCode::UnresolvedReference)
                || diagnostic.severity != Severity::Error
                || !current_errors.contains(&(diagnostic.start_byte, diagnostic.end_byte))
        });
        fast
    }

    fn diagnostics_for_tree(
        &self,
        key: &str,
        text: &str,
        tree: &Tree,
    ) -> Vec<crate::diagnostics::Diagnostic> {
        self.with_java_member_proofs(key, |index| {
            language::diagnostics(
                index,
                SourceLanguage::for_key(key),
                key,
                text,
                tree,
                &self.effective_completeness(),
            )
        })
    }

    fn with_java_member_proofs<T>(
        &self,
        key: &str,
        query: impl FnOnce(&dyn crate::index::InferenceIndex) -> T,
    ) -> T {
        self.with_java_member_proofs_in_source(key, None, query)
    }

    fn with_java_member_proofs_in_source<T>(
        &self,
        key: &str,
        recovery: Option<&java::CompletionSource>,
        query: impl FnOnce(&dyn crate::index::InferenceIndex) -> T,
    ) -> T {
        let prepared_sources = RefCell::new(HashSet::new());
        let declaration = |entry: &crate::index::Entry| {
            let mut pending = vec![entry.path.to_string()];
            loop {
                for file in pending {
                    if !prepared_sources.borrow_mut().insert(file.clone()) {
                        continue;
                    }
                    if let Some(text) = self.doc_text(&file) {
                        self.salsa.borrow_mut().prepare_declaration(
                            &file,
                            &text,
                            &self.index,
                            self.index_revision,
                        );
                    }
                }
                // No input writes or mutable borrow during tracked/nested inference.
                let result = self.salsa.borrow().declaration_query(entry);
                pending = result
                    .missing_sources
                    .iter()
                    .cloned()
                    .filter(|file| !prepared_sources.borrow().contains(file))
                    .collect();
                if pending.is_empty() {
                    return result.into_type();
                }
            }
        };
        let scoped = self
            .index
            .scoped_for_file(key)
            .with_declaration_types(&declaration);
        let model = self
            .project_model
            .as_deref()
            .filter(|_| !self.project_refresh_pending);
        let caller = model
            .and_then(|model| model.plain_java_task(key))
            .filter(|_| is_java_path(key));
        let services = OnceCell::new();
        let source_only_policy = OnceCell::new();
        let source_only = || {
            *source_only_policy
                .get_or_init(|| model.is_none() && self.source_only_caller_in_source(key, recovery))
        };
        let inventory = |owner: &crate::index::Entry| {
            if source_only() {
                let origin = self.index.file_origin(&owner.path)?;
                if owner.tier == Tier::Durable
                    && !self.open_docs.contains_key(owner.path.as_ref())
                    && owner.sym.package == "java.lang"
                    && owner.sym.name == "Object"
                    && owner.sym.container.is_none()
                    && self.global_origins.contains(&origin.origin_id)
                    && self
                        .global_origins
                        .iter()
                        .filter(|origin| origin.starts_with("jdk:"))
                        .count()
                        == 1
                {
                    return self
                        .class_inventory
                        .borrow_mut()
                        .object_for_source_origin(&origin.origin_id);
                }
                return None;
            }
            let (model, task) = (model?, caller?);
            if owner.tier != Tier::Durable
                || self.open_docs.contains_key(owner.path.as_ref())
                || !services.get_or_init(|| self.class_inventory.borrow_mut().plain_caller(task))
            {
                return None;
            }
            let origin = self.index.file_origin(&owner.path)?;
            self.class_inventory
                .borrow_mut()
                .inventory(model, task, key, owner, &origin.origin_id)
        };
        let source_proofs = RefCell::new(BTreeMap::new());
        let source_members = |owner: &crate::index::Entry| {
            let identity = (owner.path.clone(), owner.sym.start_byte);
            if let Some(complete) = source_proofs.borrow().get(&identity) {
                return *complete;
            }
            let complete = if source_only() {
                self.standalone_source_members_complete_in_source(
                    owner,
                    (owner.path.as_ref() == key).then_some(recovery).flatten(),
                )
            } else {
                match (model, caller) {
                    (Some(model), Some(task)) => {
                        self.plain_java_source_members_complete_in_source(
                            model,
                            task,
                            key,
                            owner,
                            (owner.path.as_ref() == key).then_some(recovery).flatten(),
                        ) && *services
                            .get_or_init(|| self.class_inventory.borrow_mut().plain_caller(task))
                    }
                    _ => false,
                }
            };
            source_proofs.borrow_mut().insert(identity, complete);
            complete
        };
        // One freshness check per foreign file per query, never a new parse. Facts
        // are refreshed with existing accepted source metadata on edit/scan/close.
        let kotlin_proofs = RefCell::new(BTreeMap::new());
        let kotlin_values = |path: &str| {
            *kotlin_proofs
                .borrow_mut()
                .entry(path.to_string())
                .or_insert_with(|| self.current_kotlin_value_facts(path))
        };
        let value_namespace = |owner: &crate::index::Entry| {
            ktcore::jvm::compiler_value_namespace(&scoped, owner)
                || source_only()
                    && owner.tier == Tier::Volatile
                    && kotlin_values(&owner.path)
                        .is_some_and(|facts| facts.owners.contains(&rename_definition(owner)))
                || source_members(owner)
        };
        let property_access = |property: &crate::index::Entry| {
            if property.tier != Tier::Volatile {
                return None;
            }
            match kotlin_values(&property.path)?
                .properties
                .get(&rename_definition(property))?
            {
                Knowledge::Found(access) => Some(*access),
                _ => None,
            }
        };
        let index = crate::binary_diagnostics::BinaryDiagnostics {
            index: &scoped,
            inventory: &inventory,
            source_members: &source_members,
            value_namespace: &value_namespace,
            property_access: &property_access,
        };
        query(&index)
    }

    fn effective_completeness(&self) -> resolve::CompletenessFacts {
        let mut facts = self.completeness.clone();
        // shortcut: Gradle metadata does not certify generated API closure; lift after producer/binary inventory proof.
        if self.project_model.is_some() || self.project_refresh_pending {
            facts.compiler_producers_complete = false;
        }
        facts
            .member_surfaces_unknown
            .extend(self.annotated_type_owners.values().flatten().cloned());
        facts
    }

    fn source_only_caller_in_source(
        &self,
        key: &str,
        recovery: Option<&java::CompletionSource>,
    ) -> bool {
        !self.project_refresh_pending && self.source_only_profile_in_source(key, recovery)
    }

    fn source_only_profile_in_source(
        &self,
        key: &str,
        recovery: Option<&java::CompletionSource>,
    ) -> bool {
        if self.project_model.is_some()
            || !self.project_scan_initialized
            || !self.project_file_meta.get(key).is_some_and(|meta| {
                (meta.clean
                    || recovery.is_some_and(|view| view.headers_complete)
                    || is_java_path(key)
                        && self.open_docs.get(key).is_some_and(|doc| {
                            java::declaration_headers_complete(&doc.tree, &doc.text)
                        }))
                    && meta.scope.is_none()
            })
        {
            return false;
        }
        let path = Path::new(key);
        path.is_absolute()
            && path.ancestors().skip(1).all(|ancestor| {
                [
                    "build.gradle",
                    "build.gradle.kts",
                    "settings.gradle",
                    "settings.gradle.kts",
                    "gradlew",
                    "gradlew.bat",
                    "pom.xml",
                ]
                .iter()
                .all(|name| matches!(ancestor.join(name).try_exists(), Ok(false)))
            })
    }

    fn standalone_source_members_complete_in_source(
        &self,
        owner: &crate::index::Entry,
        recovery: Option<&java::CompletionSource>,
    ) -> bool {
        let recovery = recovery.filter(|view| view.proves_owner(owner));
        if owner.tier != Tier::Volatile
            || owner.sym.container.is_some()
            || !matches!(owner.sym.kind, SymbolKind::Class | SymbolKind::Interface)
            || !owner.sym.type_params.is_empty()
            || !self.source_only_caller_in_source(&owner.path, recovery)
            || self.analysis_extensions.needs_source(&owner.path)
            || !self
                .annotated_type_owners
                .get(owner.path.as_ref())
                .is_some_and(|owners| !owners.contains(&rename_definition(owner)))
        {
            return false;
        }
        let closed;
        let (text, tree) = if let Some(view) = recovery {
            (view.text.as_str(), &view.tree)
        } else if let Some(doc) = self.open_docs.get(owner.path.as_ref()) {
            (doc.text.as_str(), &doc.tree)
        } else {
            let Some(mut batch) =
                parse_project_file(Path::new(owner.path.as_ref()), &mut LanguageParsers::new())
            else {
                return false;
            };
            if self.source_stamps.get(owner.path.as_ref()) != batch.stamp.as_ref()
                || !batch.clean
                || batch.package != owner.sym.package
            {
                return false;
            }
            batch.symbols.sort_by(|a, b| {
                a.start_byte
                    .cmp(&b.start_byte)
                    .then(a.end_byte.cmp(&b.end_byte))
                    .then(a.name.cmp(&b.name))
            });
            if !self
                .index
                .entries_for_file(&owner.path)
                .iter()
                .map(|entry| entry.sym.as_ref())
                .eq(batch.symbols.iter())
            {
                return false;
            }
            closed = batch.source;
            (closed.0.as_ref(), &closed.1)
        };
        if tree.root_node().has_error()
            && !(is_java_path(&owner.path) && java::declaration_headers_complete(tree, text))
        {
            return false;
        }
        let Some(name) = self.identifier_at_key(&owner.path, tree, owner.sym.start_byte) else {
            return false;
        };
        if name.start_byte() != owner.sym.start_byte
            || name.end_byte() != owner.sym.end_byte
            || &text[name.byte_range()] != owner.sym.name
        {
            return false;
        }
        let Some(declaration) = name.parent() else {
            return false;
        };
        if !matches!(
            declaration.kind(),
            "class_declaration" | "interface_declaration"
        ) {
            return false;
        }
        let kotlin = !is_java_path(&owner.path);
        if kotlin
            && self
                .index
                .entries_for_file(&owner.path)
                .iter()
                .any(|entry| {
                    entry.sym.container.as_deref() == Some(owner.sym.name.as_str())
                        && entry.sym.ext_receiver.is_some()
                })
        {
            return false;
        }
        let mut pending = vec![declaration];
        while let Some(node) = pending.pop() {
            if matches!(node.kind(), "annotation" | "marker_annotation")
                || kotlin
                    && (matches!(
                        node.kind(),
                        "property_declaration"
                            | "object_declaration"
                            | "companion_object"
                            | "class_parameter"
                    ) || matches!(
                        &text[node.byte_range()],
                        "data"
                            | "enum"
                            | "annotation"
                            | "value"
                            | "expect"
                            | "external"
                            | "inline"
                            | "suspend"
                            | "by"
                            | "context"
                    ))
            {
                return false;
            }
            let mut cursor = node.walk();
            pending.extend(node.children(&mut cursor));
        }
        true
    }

    /// A current Java owner under the caller's selected javac task. This does not close project
    /// producers or Kotlin output. Closed owners are freshly parsed against the indexed stamp
    /// and declaration facts; query-local recovery can prove preserved current caller headers.
    fn plain_java_source_members_complete(
        &self,
        model: &crate::classpath::ProjectSnapshot,
        task: &crate::classpath::CompilerTask,
        caller: &str,
        owner: &crate::index::Entry,
    ) -> bool {
        self.plain_java_source_members_complete_in_source(model, task, caller, owner, None)
    }

    fn plain_java_source_members_complete_in_source(
        &self,
        model: &crate::classpath::ProjectSnapshot,
        task: &crate::classpath::CompilerTask,
        caller: &str,
        owner: &crate::index::Entry,
        recovery: Option<&java::CompletionSource>,
    ) -> bool {
        let recovery = recovery.filter(|view| view.proves_owner(owner));
        if self.project_refresh_pending
            || owner.tier != Tier::Volatile
            || !is_java_path(&owner.path)
            || owner.sym.kind != SymbolKind::Class
            || owner.sym.container.is_some()
            || !owner.sym.type_params.is_empty()
            || !owner.sym.supertypes.is_empty()
            || !owner.sym.supertype_refs.is_empty()
            || self.analysis_extensions.needs_source(&owner.path)
            || !self
                .annotated_type_owners
                .get(owner.path.as_ref())
                .is_some_and(|owners| !owners.contains(&rename_definition(owner)))
            || !std::ptr::eq(
                task,
                match model.plain_java_task(&owner.path) {
                    Some(selected) => selected,
                    None => return false,
                },
            )
            || !task
                .source_files
                .iter()
                .any(|path| crate::paths::file_key(path) == owner.path.as_ref())
        {
            return false;
        }
        let Some(meta) = self.project_file_meta.get(owner.path.as_ref()) else {
            return false;
        };
        if !(meta.clean
            || recovery.is_some()
            || self
                .open_docs
                .get(owner.path.as_ref())
                .is_some_and(|doc| java::declaration_headers_complete(&doc.tree, &doc.text)))
            || meta.package != owner.sym.package
            || meta.scope.is_none()
            || meta.scope != self.project_scope_for_file(caller)
            || !self.index.file_origin(&owner.path).is_some_and(|origin| {
                origin.representation == crate::project_model::SourceRepresentation::Source
                    && origin.project_scope == meta.scope
            })
        {
            return false;
        }
        let closed;
        let (text, tree) = if let Some(view) = recovery {
            (view.text.as_str(), &view.tree)
        } else if let Some(doc) = self.open_docs.get(owner.path.as_ref()) {
            (doc.text.as_str(), &doc.tree)
        } else {
            let Some(stamp) = self.source_stamps.get(owner.path.as_ref()) else {
                return false;
            };
            let Some(mut batch) =
                parse_project_file(Path::new(owner.path.as_ref()), &mut LanguageParsers::new())
            else {
                return false;
            };
            if batch.stamp != Some(*stamp) || !batch.clean || batch.package != meta.package {
                return false;
            }
            batch.symbols.sort_by(|a, b| {
                a.start_byte
                    .cmp(&b.start_byte)
                    .then(a.end_byte.cmp(&b.end_byte))
                    .then(a.name.cmp(&b.name))
            });
            let indexed = self.index.entries_for_file(&owner.path);
            if !indexed
                .iter()
                .map(|entry| entry.sym.as_ref())
                .eq(batch.symbols.iter())
                || language::annotated_type_owners(
                    &self.index,
                    &owner.path,
                    &batch.source.1,
                    &batch.source.0,
                )
                .contains(&rename_definition(owner))
            {
                return false;
            }
            closed = batch.source;
            (closed.0.as_ref(), &closed.1)
        };
        let root = tree.root_node();
        if !java::declaration_headers_complete(tree, text) {
            return false;
        }
        if recovery.is_some() {
            let mut pending = vec![root];
            while let Some(node) = pending.pop() {
                if matches!(node.kind(), "annotation" | "marker_annotation") {
                    return false;
                }
                let mut cursor = node.walk();
                pending.extend(node.children(&mut cursor));
            }
        }
        let is_type = |node: &tree_sitter::Node<'_>| {
            matches!(
                node.kind(),
                "class_declaration"
                    | "interface_declaration"
                    | "enum_declaration"
                    | "record_declaration"
                    | "annotation_type_declaration"
            )
        };
        let mut cursor = root.walk();
        let mut declarations = root.named_children(&mut cursor).filter(is_type);
        let Some(declaration) = declarations.next() else {
            return false;
        };
        if declaration.kind() != "class_declaration" || declarations.next().is_some() {
            return false;
        }
        if !declaration.child_by_field_name("name").is_some_and(|name| {
            name.start_byte() == owner.sym.start_byte
                && name.end_byte() == owner.sym.end_byte
                && &text[name.byte_range()] == owner.sym.name
        }) {
            return false;
        }
        let Some(body) = declaration.child_by_field_name("body") else {
            return false;
        };
        let mut cursor = body.walk();
        let nested = body.named_children(&mut cursor).any(|node| is_type(&node));
        !nested
    }

    fn record_project_file_meta(&mut self, key: &str, package: String, clean: bool) {
        if !self.project_scan_initialized {
            return;
        }
        if self.project_model.as_ref().is_some_and(|model| {
            model.modules.iter().any(|m| !m.source_sets.is_empty())
                && model.file_origin(Path::new(key)).is_none()
        }) {
            self.project_file_meta.remove(key);
            self.recompute_project_completeness();
            return;
        }
        self.project_file_meta.insert(
            key.to_string(),
            ProjectFileMeta {
                kotlin_values: None,
                package,
                clean,
                scope: self.project_scope_for_file(key),
            },
        );
        self.recompute_project_completeness();
    }

    fn recompute_project_completeness(&mut self) {
        if !self.project_scan_initialized {
            return;
        }
        self.completeness.project_scan_complete = !self.project_refresh_pending
            && self
                .project_model
                .as_ref()
                .is_none_or(|model| model.modules.iter().any(|m| !m.source_sets.is_empty()))
            && self
                .project_file_meta
                .values()
                .all(|meta| meta.clean && (self.project_model.is_none() || meta.scope.is_some()));
        let mut clean_packages = BTreeSet::new();
        let mut dirty_packages = BTreeSet::new();
        let mut clean_scoped_packages = BTreeSet::new();
        let mut dirty_scoped_packages = BTreeSet::new();
        let mut packages_with_unknown_scope = BTreeSet::new();
        let mut package_modules = HashMap::<String, BTreeSet<String>>::new();
        for meta in self.project_file_meta.values() {
            if meta.clean {
                clean_packages.insert(meta.package.clone());
            } else {
                dirty_packages.insert(meta.package.clone());
            }
            match &meta.scope {
                Some(scope) => {
                    package_modules
                        .entry(meta.package.clone())
                        .or_default()
                        .insert(scope.module.clone());
                    let scoped = scope.package_scope(meta.package.clone());
                    if meta.clean {
                        clean_scoped_packages.insert(scoped);
                    } else {
                        dirty_scoped_packages.insert(scoped);
                    }
                }
                None => {
                    packages_with_unknown_scope.insert(meta.package.clone());
                }
            }
        }
        clean_packages.retain(|pkg| !dirty_packages.contains(pkg));
        clean_scoped_packages.retain(|scope| !dirty_scoped_packages.contains(scope));
        self.completeness.project_packages_complete = clean_packages;
        self.completeness.project_scoped_packages_complete = clean_scoped_packages;
        self.completeness.project_packages_with_unknown_scope = packages_with_unknown_scope;
        self.completeness.project_package_modules = package_modules.into_iter().collect();
    }

    /// `textDocument/codeAction`: conservative import actions over the current document.
    pub fn code_actions(
        &mut self,
        key: &str,
        range_start: usize,
        range_end: usize,
        cursor_offset: usize,
    ) -> Vec<Action> {
        let unresolved = self.goto_definition(key, cursor_offset).is_empty();
        if let Some(doc) = self.open_docs.get(key) {
            return self.code_actions_for_tree(
                key,
                &doc.text,
                &doc.tree,
                range_start,
                range_end,
                cursor_offset,
                unresolved,
            );
        }
        let (text, tree) = match self.parsed_closed_doc(key) {
            Some(parsed) => parsed,
            None => return Vec::new(),
        };
        self.code_actions_for_tree(
            key,
            &text,
            &tree,
            range_start,
            range_end,
            cursor_offset,
            unresolved,
        )
    }

    fn code_actions_for_tree(
        &self,
        key: &str,
        text: &str,
        tree: &Tree,
        range_start: usize,
        range_end: usize,
        cursor_offset: usize,
        unresolved: bool,
    ) -> Vec<Action> {
        if tree.root_node().has_error() {
            return Vec::new();
        }
        if is_java_path(key) {
            let diagnostics = java::diagnostics(
                &self.index.scoped_for_file(key),
                key,
                tree,
                text,
                &self.effective_completeness(),
            );
            let mut out =
                java::unused_import_actions(key, text, tree, &diagnostics, range_start, range_end);
            if let Some(action) =
                self.interface_implementation_action(key, text, tree, cursor_offset)
            {
                out.push(action);
            }
            if let Some(action) = java::organize_imports_action(key, text, tree) {
                out.push(action);
            }
            if unresolved {
                if let Some((name, fqn)) =
                    self.unambiguous_import_candidate(key, text, tree, cursor_offset)
                {
                    if let Some(action) = java::add_import_action(key, text, tree, &name, &fqn) {
                        out.push(action);
                    }
                }
            }
            return out;
        }
        let diagnostics = crate::diagnostics::compute(text, tree);
        let mut out =
            actions::unused_import_actions(key, text, tree, &diagnostics, range_start, range_end);
        if let Some(action) = self.interface_implementation_action(key, text, tree, cursor_offset) {
            out.push(action);
        }
        if let Some(action) = actions::organize_imports_action(key, text, tree) {
            out.push(action);
        }
        out.extend(crate::refactor::function_rewrite_actions(
            key,
            text,
            tree,
            cursor_offset,
        ));
        if unresolved {
            if let Some((name, fqn)) =
                self.unambiguous_import_candidate(key, text, tree, cursor_offset)
            {
                if let Some(action) = actions::add_import_action(key, text, tree, &name, &fqn) {
                    out.push(action);
                }
            }
        }
        out
    }

    fn interface_implementation_action(
        &self,
        key: &str,
        text: &str,
        tree: &Tree,
        offset: usize,
    ) -> Option<Action> {
        let model = self
            .project_model
            .as_deref()
            .filter(|_| !self.project_refresh_pending)?;
        let scoped = self.index.scoped_for_file(key);
        let (owner, body) =
            crate::refactor::implementation_class(&scoped, key, text, tree, offset, |file| {
                self.open_docs
                    .get(file)
                    .is_some_and(|doc| !doc.tree.root_node().has_error())
                    && !self.analysis_extensions.needs_source(file)
            })?;
        if owner.sym.supertypes.is_empty() && owner.sym.supertype_refs.is_empty() {
            return None;
        }
        let object = self
            .class_inventory
            .borrow_mut()
            .object_for_caller(model, key)?;
        let source_owner_known = |declaration: &crate::index::Entry| {
            if declaration.tier != Tier::Volatile
                || self.analysis_extensions.needs_source(&declaration.path)
                || !self
                    .annotated_type_owners
                    .get(declaration.path.as_ref())
                    .is_some_and(|owners| !owners.contains(&rename_definition(declaration)))
            {
                return false;
            }
            if let Some(doc) = self.open_docs.get(declaration.path.as_ref()) {
                if doc.tree.root_node().has_error() {
                    return false;
                }
            } else {
                let Some(meta) = self.project_file_meta.get(declaration.path.as_ref()) else {
                    return false;
                };
                let Some(stamp) = self.source_stamps.get(declaration.path.as_ref()) else {
                    return false;
                };
                let Some(mut batch) = parse_project_file(
                    Path::new(declaration.path.as_ref()),
                    &mut LanguageParsers::new(),
                ) else {
                    return false;
                };
                if !meta.clean
                    || !batch.clean
                    || batch.stamp != Some(*stamp)
                    || batch.package != meta.package
                {
                    return false;
                }
                batch.symbols.sort_by(|a, b| {
                    a.start_byte
                        .cmp(&b.start_byte)
                        .then(a.end_byte.cmp(&b.end_byte))
                        .then(a.name.cmp(&b.name))
                });
                if !self
                    .index
                    .entries_for_file(&declaration.path)
                    .iter()
                    .map(|entry| entry.sym.as_ref())
                    .eq(batch.symbols.iter())
                    || language::annotated_type_owners(
                        &self.index,
                        &declaration.path,
                        &batch.source.1,
                        &batch.source.0,
                    )
                    .contains(&rename_definition(declaration))
                {
                    return false;
                }
            }
            self.class_inventory
                .borrow_mut()
                .object_for_caller(model, &declaration.path)
                .is_some_and(|selected| selected.identity == object.identity)
        };
        crate::refactor::implement_missing_methods(
            &scoped,
            key,
            text,
            &owner,
            body,
            &object,
            &self.effective_completeness(),
            &source_owner_known,
        )
    }

    fn unambiguous_import_candidate(
        &self,
        key: &str,
        text: &str,
        tree: &Tree,
        offset: usize,
    ) -> Option<(String, String)> {
        let language = SourceLanguage::for_key(key);
        let ident = language::identifier_at(language, tree, offset)?;
        let name = language::importable_reference_name(language, ident, text)?;
        let visibility = language::NameVisibility::for_file(language, tree, text);
        let index = self.index.scoped_for_file(key);
        let mut candidates = index
            .lookup_by_name(name)
            .into_iter()
            .filter(|entry| {
                entry.sym.name == name
                    && entry.sym.container.is_none()
                    && !entry.sym.package.is_empty()
                    && !visibility.is_visible(&entry.sym.package, &entry.sym.name)
            })
            .map(|entry| fqn(&entry.sym.package, &entry.sym.name))
            .collect::<Vec<_>>();
        candidates.sort();
        candidates.dedup();
        match candidates.as_slice() {
            [fqn] => Some((name.to_string(), fqn.clone())),
            _ => None,
        }
    }

    /// `textDocument/references`: all usage sites (as `Def` locations) of the symbol at `offset`.
    /// Goto-grade precision: every candidate usage of the name is re-resolved and kept only if it
    /// resolves to the same definition as the cursor. Optionally includes the declaration site.
    pub fn references(&mut self, key: &str, offset: usize, include_declaration: bool) -> Vec<Def> {
        self.references_with_policy(key, offset, include_declaration, false)
            .unwrap_or_default()
    }

    fn references_with_policy(
        &mut self,
        key: &str,
        offset: usize,
        include_declaration: bool,
        for_rename: bool,
    ) -> Option<Vec<Def>> {
        self.refresh_compiled_freshness();
        let start = std::time::Instant::now();
        let target = match self.goto_definition(key, offset).into_iter().next() {
            Some(d) => d,
            None => return None,
        };
        let mut names = self.reference_names(&target);
        if names.is_empty() {
            if let Some(name) = self.name_at(key, offset) {
                names.insert(name);
            }
        }
        let (mut candidates, alias_sites) =
            self.reference_candidates(&target, &names, for_rename)?;
        // Backstop against a pathologically common name in a very large project.
        const MAX_CANDIDATES: usize = 5000;
        if candidates.len() > MAX_CANDIDATES {
            tracing::warn!(
                "references({names:?}): {} candidates, capping at {MAX_CANDIDATES}",
                candidates.len()
            );
            candidates.truncate(MAX_CANDIDATES);
        }
        if for_rename {
            // Import aliases keep their own spelling when the authored declaration is renamed.
            candidates.retain(|site| {
                !alias_sites.contains(&Def {
                    file: site.path.to_string(),
                    start_byte: site.start_byte,
                    end_byte: site.end_byte,
                })
            });
        }
        // Group by file so each file is parsed at most once (not once per usage within it).
        let mut by_path: HashMap<Arc<str>, Vec<RefEntry>> = HashMap::new();
        for c in candidates {
            by_path.entry(c.path.clone()).or_default().push(c);
        }
        let candidate_count = by_path.values().map(Vec::len).sum::<usize>();
        let file_count = by_path.len();
        let mut out: Vec<Def> = Vec::new();
        for (path, refs) in by_path {
            self.collect_refs_in_file(&path, &refs, &target, include_declaration, false, &mut out);
        }
        if include_declaration {
            out.push(target.clone());
        }
        for (owner, set) in self.analysis_extensions.sets() {
            for site in set.sites() {
                let scoped = self.index.scoped_for_file(owner);
                let Some(binding) = scoped.semantic_binding_at(owner, site.full_range.start) else {
                    continue;
                };
                if binding.target.definition() != &target {
                    continue;
                }
                let location = Def {
                    file: owner.into(),
                    start_byte: site.full_range.start,
                    end_byte: site.full_range.end,
                };
                if include_declaration
                    || binding
                        .authored
                        .as_ref()
                        .is_none_or(|authored| authored.definition() != &location)
                {
                    out.push(location);
                }
            }
        }
        out.sort();
        out.dedup();
        if for_rename && out.len() > RENAME_REF_CAP {
            return None;
        }
        crate::trace::span(
            "workspace.references.resolve_candidates",
            "workspace",
            start,
            serde_json::json!({
                "file": key,
                "symbols": names,
                "candidateRefs": candidate_count,
                "candidateFiles": file_count,
                "count": out.len(),
            }),
        );
        Some(out)
    }

    fn reference_candidates(
        &mut self,
        target: &Def,
        names: &BTreeSet<String>,
        for_rename: bool,
    ) -> Option<(Vec<RefEntry>, BTreeSet<Def>)> {
        let mut candidates: Vec<_> = names
            .iter()
            .flat_map(|name| self.index.lookup_refs(name).iter().cloned())
            .collect();
        let mut alias_sites = BTreeSet::new();
        if candidates.len() > RENAME_REF_CAP {
            return (!for_rename).then_some((candidates, alias_sites));
        }
        // The real imported terminal is already indexed. Only its candidate files need parsing.
        let paths = candidates
            .iter()
            .filter(|site| !SourceLanguage::for_key(&site.path).is_java())
            .map(|site| site.path.clone())
            .collect::<BTreeSet<_>>();
        let mut seen = candidates
            .iter()
            .map(|site| (site.path.clone(), site.start_byte, site.end_byte))
            .collect::<BTreeSet<_>>();
        for path in paths {
            let aliases = if let Some(doc) = self.open_docs.get(path.as_ref()) {
                self.matching_import_aliases(&path, &doc.text, &doc.tree, target, names)
            } else {
                let Some((text, tree)) = self.parsed_closed_doc(&path) else {
                    if for_rename {
                        return None;
                    }
                    continue;
                };
                self.matching_import_aliases(&path, &text, &tree, target, names)
            };
            for (alias, imported_name, unambiguous) in aliases {
                // shortcut: same-spelling aliases need qualified-use projection before rename.
                if for_rename && (!unambiguous || alias == imported_name) {
                    return None;
                }
                for site in self
                    .index
                    .lookup_refs(&alias)
                    .iter()
                    .filter(|site| site.path == path)
                {
                    if for_rename {
                        alias_sites.insert(Def {
                            file: path.to_string(),
                            start_byte: site.start_byte,
                            end_byte: site.end_byte,
                        });
                    }
                    if seen.insert((site.path.clone(), site.start_byte, site.end_byte)) {
                        candidates.push(site.clone());
                    }
                    if candidates.len() > RENAME_REF_CAP {
                        return (!for_rename).then_some((candidates, alias_sites));
                    }
                }
            }
        }
        Some((candidates, alias_sites))
    }

    /// (Local alias spelling, real import spelling, unique imported declaration).
    fn matching_import_aliases(
        &self,
        key: &str,
        text: &str,
        tree: &Tree,
        target: &Def,
        names: &BTreeSet<String>,
    ) -> Vec<(String, String, bool)> {
        let mut aliases = Vec::new();
        let mut cursor = tree.root_node().walk();
        for import in tree.root_node().named_children(&mut cursor) {
            if import.kind() != "import" {
                continue;
            }
            let Some(alias) = crate::parser::child_of_kind(import, "identifier") else {
                continue;
            };
            let Some(path) = crate::parser::child_of_kind(import, "qualified_identifier") else {
                continue;
            };
            let Some(imported) = path.named_child(path.named_child_count().saturating_sub(1))
            else {
                continue;
            };
            let imported_name = node_text(imported, text);
            if !names.contains(imported_name) {
                continue;
            }
            let definitions = self.goto_definition_for_tree(key, text, tree, imported.start_byte());
            if definitions.contains(target) {
                aliases.push((
                    node_text(alias, text).to_string(),
                    imported_name.to_string(),
                    definitions.len() == 1,
                ));
            }
        }
        aliases
    }

    fn reference_names(&self, target: &Def) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        if let Some(entry) = hierarchy::entry_for_name_range(
            &self.index,
            &target.file,
            target.start_byte,
            target.end_byte,
        ) {
            names.insert(entry.sym.name.clone());
            if let Some(property) = ktcore::members::kotlin_java_getter_property_name(&entry) {
                names.insert(property);
            }
        }
        if let Some(source) = self.index.jvm_source_for_file(&target.file) {
            for callable in &source.facts.callables {
                if callable.definition.as_ref() == Some(target) {
                    names.insert(callable.name.clone());
                }
            }
        }
        for (owner, set) in self.analysis_extensions.sets() {
            for resolution in set.requirements() {
                let Knowledge::Found(binding) = &resolution.binding else {
                    continue;
                };
                if binding.target.definition() != target {
                    continue;
                }
                let Some(authored) = &binding.authored else {
                    continue;
                };
                if self
                    .index
                    .scoped_for_file(owner)
                    .semantic_binding(authored.definition())
                    != Some(binding)
                {
                    continue;
                }
                let definition = authored.definition();
                if let Some(entry) = hierarchy::entry_for_name_range(
                    &self.index,
                    &definition.file,
                    definition.start_byte,
                    definition.end_byte,
                ) {
                    names.insert(entry.sym.name.clone());
                }
            }
        }
        names
    }

    /// Resolve all candidate usages in one file against `target`, parsing the file at most once
    /// (reusing the cached tree for open buffers).
    fn collect_refs_in_file(
        &mut self,
        path: &str,
        refs: &[RefEntry],
        target: &Def,
        include_declaration: bool,
        authored: bool,
        out: &mut Vec<Def>,
    ) {
        if let Some(doc) = self.open_docs.get(path) {
            for r in refs {
                if let Some(s) = self.resolve_usage(
                    path,
                    &doc.text,
                    &doc.tree,
                    r,
                    target,
                    include_declaration,
                    authored,
                ) {
                    out.push(s);
                }
            }
        } else {
            let (text, tree) = match self.parsed_closed_doc(path) {
                Some(parsed) => parsed,
                None => return,
            };
            for r in refs {
                if let Some(s) =
                    self.resolve_usage(path, &text, &tree, r, target, include_declaration, authored)
                {
                    out.push(s);
                }
            }
        }
    }

    /// Whether a single usage site references `target` (re-resolving against the given tree).
    fn resolve_usage(
        &self,
        path: &str,
        text: &str,
        tree: &Tree,
        r: &RefEntry,
        target: &Def,
        include_declaration: bool,
        authored: bool,
    ) -> Option<Def> {
        let site = Def {
            file: path.to_string(),
            start_byte: r.start_byte,
            end_byte: r.end_byte,
        };
        if site == *target {
            // The declaration's own name identifier.
            return include_declaration.then_some(site);
        }
        if !authored
            && self
                .index
                .scoped_for_file(path)
                .semantic_binding(&site)
                .is_some_and(|binding| binding.target.definition() == target)
        {
            return include_declaration.then_some(site);
        }
        let defs = if authored {
            language::authored_goto_definition(
                &self.index.scoped_for_file(path),
                SourceLanguage::for_key(path),
                path,
                text,
                tree,
                r.start_byte,
            )
        } else {
            self.goto_definition_for_tree(path, text, tree, r.start_byte)
        };
        matches!(defs.as_slice(), [definition] if definition == target).then_some(site)
    }

    /// The identifier text at `offset` in `key`, using the cached tree for open buffers.
    fn name_at(&mut self, key: &str, offset: usize) -> Option<String> {
        if let Some(doc) = self.open_docs.get(key) {
            let id = self.identifier_at_key(key, &doc.tree, offset)?;
            return Some(node_text(id, &doc.text).to_string());
        }
        let (text, tree) = self.parsed_closed_doc(key)?;
        let id = self.identifier_at_key(key, &tree, offset)?;
        Some(node_text(id, &text).to_string())
    }
}

fn materialize_definition_files(defs: &[Def]) {
    for def in defs {
        let path = Path::new(&def.file);
        if !path.is_file() {
            crate::deps::materialize_source_file(path);
        }
    }
}

/// Cap on the number of completion candidates assembled before shaping (UX contract: ~1000). High
/// enough that a common prefix rarely truncates useful names; editors re-request as the prefix
/// narrows. Equals `complete::RESULT_CAP` (the post-ranking cap), so assembly never starves shaping.
const MAX_COMPLETIONS: usize = complete::RESULT_CAP;

const RENAME_REF_CAP: usize = 5000;

fn rename_definition(entry: &crate::index::Entry) -> Def {
    Def {
        file: entry.path.to_string(),
        start_byte: entry.sym.start_byte,
        end_byte: entry.sym.end_byte,
    }
}

fn rename_type(entry: &crate::index::Entry) -> crate::types::Type {
    crate::types::Type::Class {
        name: entry.sym.name.clone(),
        package: Some(entry.sym.package.clone()),
        container: entry.sym.container.clone(),
        nullable: false,
        args: entry
            .sym
            .type_params
            .iter()
            .map(|_| crate::types::Type::Unknown)
            .collect(),

        owner: None,
    }
}

/// A symbol's fully-qualified name (`package.name`), or the bare `name` when the package is empty.
fn fqn(package: &str, name: &str) -> String {
    if package.is_empty() {
        name.to_string()
    } else {
        format!("{package}.{name}")
    }
}

fn tier_rank(tier: Tier) -> u8 {
    match tier {
        Tier::Volatile => 0,
        Tier::Durable => 1,
    }
}

/// Prune build output, common generated dirs, and dot directories from the scan.
pub(crate) fn is_excluded(entry: &DirEntry) -> bool {
    if !entry.file_type().is_dir() {
        return false;
    }
    match entry.file_name().to_str() {
        Some(name) => is_excluded_dir_name(name),
        None => false,
    }
}

fn is_excluded_dir_name(name: &str) -> bool {
    matches!(
        name,
        "build" | "out" | "target" | "node_modules" | ".gradle"
    ) || (name.starts_with('.') && name.len() > 1)
}

fn relative_path_has_excluded_dir(path: &str) -> bool {
    path.split('/').any(is_excluded_dir_name)
}

#[cfg(test)]
fn project_source_files(root: &Path) -> Vec<std::path::PathBuf> {
    project_source_files_with_model(root, None)
}

fn project_source_files_with_model(
    root: &Path,
    model: Option<&crate::classpath::ProjectSnapshot>,
) -> Vec<std::path::PathBuf> {
    if let Some(model) = model {
        return registered_source_files(model);
    }
    let mut out = git_source_files(root).unwrap_or_else(|| walk_source_files(root));
    out.extend(generated_source_files(root));
    out.sort();
    out.dedup();
    out
}

fn registered_source_files(model: &crate::classpath::ProjectSnapshot) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for root in model
        .modules
        .iter()
        .flat_map(|m| &m.source_sets)
        .flat_map(|s| &s.source_roots)
    {
        for entry in WalkDir::new(root).into_iter().filter_map(Result::ok) {
            if entry.file_type().is_file()
                && SourceLanguage::for_project_path(entry.path()).is_some()
                && model.file_origin(entry.path()).is_some()
            {
                files.push(entry.into_path());
            }
        }
    }
    files.sort_by_key(|path| crate::paths::file_key(path));
    files.dedup_by_key(|path| crate::paths::file_key(path));
    files
}

fn parse_project_files(paths: Vec<std::path::PathBuf>) -> Vec<ProjectFileIndex> {
    if paths.is_empty() {
        return Vec::new();
    }
    let workers = project_index_threads().min(paths.len());
    if workers <= 1 {
        let mut parsers = LanguageParsers::new();
        let mut out: Vec<_> = paths
            .into_iter()
            .filter_map(|path| parse_project_file(&path, &mut parsers))
            .collect();
        out.sort_by(|a, b| a.key.cmp(&b.key));
        return out;
    }

    let queue = Arc::new(Mutex::new(VecDeque::from(paths)));
    let (tx, rx) = mpsc::channel();
    let mut handles = Vec::new();
    for _ in 0..workers {
        let queue = queue.clone();
        let tx = tx.clone();
        handles.push(std::thread::spawn(move || {
            let mut parsers = LanguageParsers::new();
            loop {
                let path = {
                    let mut guard = queue.lock().unwrap();
                    guard.pop_front()
                };
                let Some(path) = path else {
                    break;
                };
                if let Some(batch) = parse_project_file(&path, &mut parsers) {
                    let _ = tx.send(batch);
                }
            }
        }));
    }
    drop(tx);
    let mut out: Vec<_> = rx.into_iter().collect();
    for handle in handles {
        let _ = handle.join();
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

fn parse_project_file(path: &Path, parsers: &mut LanguageParsers) -> Option<ProjectFileIndex> {
    let stamp = source_stamp(path)?;
    let text = std::fs::read_to_string(path).ok()?;
    let language = SourceLanguage::for_project_path(path)?;
    let tree = parsers.parse(language, &text);
    let facts = language::file_facts(language, &tree, &text);
    if source_stamp(path) != Some(stamp) {
        return None;
    }
    Some(ProjectFileIndex {
        key: crate::paths::file_key(path),
        package: facts.package,
        symbols: facts.symbols,
        jvm: facts.jvm,
        usages: facts.usages,
        clean: facts.clean,
        stamp: Some(stamp),
        source: (Arc::from(text), tree),
    })
}

fn source_stamp(path: &Path) -> Option<SourceStamp> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(SourceStamp {
        modified: metadata.modified().ok()?,
        size: metadata.len(),
    })
}

pub(crate) fn prepare_model_sources(
    model: &crate::classpath::ProjectSnapshot,
    known: &HashMap<String, SourceStamp>,
) -> ModelSourceUpdate {
    let paths = registered_source_files(model);
    let current = paths
        .iter()
        .map(|path| crate::paths::file_key(path))
        .collect();
    let changed: Vec<_> = paths
        .into_iter()
        .filter(|path| {
            let key = crate::paths::file_key(path);
            source_stamp(path).is_none_or(|stamp| known.get(&key) != Some(&stamp))
        })
        .collect();
    let expected = changed.len();
    let batches = parse_project_files(changed);
    ModelSourceUpdate {
        current,
        complete: batches.len() == expected,
        batches,
    }
}

fn is_dependency_origin(origin: &str) -> bool {
    origin.starts_with("maven:")
        || origin.starts_with("file:")
        || crate::classpath::is_sdk_origin(origin)
}

fn project_index_threads() -> usize {
    std::env::var("KTLSP_PROJECT_INDEX_THREADS")
        .ok()
        .or_else(|| std::env::var("KTLSP_INDEX_THREADS").ok())
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|threads| *threads > 0)
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|threads| threads.get())
                .unwrap_or(1)
                .min(8)
        })
}

fn git_source_files(root: &Path) -> Option<Vec<std::path::PathBuf>> {
    git_project_files(root, &["*.kt", "*.kts", "*.java"])
}

pub(crate) fn git_project_files(root: &Path, patterns: &[&str]) -> Option<Vec<std::path::PathBuf>> {
    let top = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !top.status.success() {
        return None;
    }
    let top = String::from_utf8(top.stdout).ok()?;
    let top = std::path::PathBuf::from(top.trim());
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--full-name",
            "--",
        ])
        .args(patterns)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let canonical_top = top.canonicalize().ok()?;
    let relative_root = root.strip_prefix(&canonical_top).ok()?;
    let mut out = Vec::new();
    for rel in output.stdout.split(|byte| *byte == 0) {
        if rel.is_empty() {
            continue;
        }
        let rel = String::from_utf8_lossy(rel);
        if relative_path_has_excluded_dir(&rel) {
            continue;
        }
        if Path::new(rel.as_ref()).starts_with(relative_root) {
            out.push(top.join(rel.as_ref()));
        }
    }
    out.sort();
    Some(out)
}

fn walk_source_files(root: &Path) -> Vec<std::path::PathBuf> {
    let walker = WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !is_excluded(e));
    let mut out = Vec::new();
    for entry in walker.filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("kt") | Some("kts") | Some("java")
        ) {
            out.push(path.to_path_buf());
        }
    }
    out
}

fn generated_source_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let walker = WalkDir::new(root).into_iter().filter_entry(|entry| {
        if !entry.file_type().is_dir() {
            return true;
        }
        if path_has_build_dir(root, entry.path()) {
            generated_source_walk_dir(root, entry.path())
        } else {
            !is_excluded(entry)
        }
    });
    for entry in walker.filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if !is_generated_source_file(root, path) {
            continue;
        }
        if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("kt") | Some("kts") | Some("java")
        ) {
            out.push(path.to_path_buf());
        }
    }
    out
}

fn path_has_build_dir(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    rel.components()
        .any(|c| c.as_os_str().to_string_lossy() == "build")
}

fn generated_source_walk_dir(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    let parts: Vec<_> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect();
    let Some(build_pos) = parts.iter().rposition(|part| part == "build") else {
        return false;
    };
    let inside_build = &parts[build_pos + 1..];
    match inside_build {
        [] => true,
        [a] => a == "generated",
        [a, b] => a == "generated" && b == "source",
        [a, b, ..] => a == "generated" && b == "source",
    }
}

fn generated_source_dir(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .windows(3)
        .any(|w| w[0] == "build" && w[1] == "generated" && w[2] == "source")
}

fn is_generated_source_file(root: &Path, path: &Path) -> bool {
    path.parent()
        .is_some_and(|parent| generated_source_dir(root, parent))
}

#[cfg(test)]
mod tests {
    use super::*;

    const IMPLEMENT_INTERFACE_ACTION_TITLE: &str = "Implement missing methods";

    // Explicit unit-test inventory, not a claim that source-only fixtures have a selected SDK.
    // Native/independent compiler probes use the actual selected java.base archive.
    fn interface_sdk_fixture(ws: &mut Workspace, files: &[&str]) -> std::path::PathBuf {
        use crate::classpath::{
            CompilerTask, ModuleClasspath, ProjectSnapshot, SourceSetClasspath,
        };
        let root = unique_tmp("interface-sdk");
        let home = root.join("jdk");
        std::fs::create_dir_all(home.join("jmods")).unwrap();
        std::fs::write(
            home.join("jmods/java.base.jmod"),
            b"explicit inventory stamp",
        )
        .unwrap();
        let mut kotlin = crate::compile::kotlin_test_request(&root).task;
        kotlin.source_files = files
            .iter()
            .filter(|file| file.ends_with(".kt"))
            .map(|file| std::path::PathBuf::from(*file))
            .collect();
        kotlin.classpath.clear();
        kotlin.arguments = vec!["-jvm-target".into(), "17".into()];
        let source_root = if files[0].starts_with("mem:///") {
            Path::new("mem:")
        } else {
            Path::new(files[0]).parent().unwrap()
        };
        let java: CompilerTask = serde_json::from_value(serde_json::json!({
            "task": ":compileJava", "language": "java",
            "task_type": "org.gradle.api.tasks.compile.JavaCompile",
            "arguments": [], "annotation_processor_path": [],
            "source_files": files.iter().filter(|file| file.ends_with(".java")).collect::<Vec<_>>(),
            "classpath": [], "java_home": home,
            "options": {"fork": false, "forkJavaHome": null, "forkExecutable": null,
                "forkJvmArgs": [], "release": null, "bootstrapClasspath": null},
            "output_roots": [], "complete": true, "problem": null,
        }))
        .unwrap();
        ws.set_project_model(Arc::new(ProjectSnapshot::new(vec![ModuleClasspath {
            module: ":".into(),
            project_dir: source_root.into(),
            build: String::new(),
            entries: Vec::new(),
            unresolved: Vec::new(),
            source_sets: vec![SourceSetClasspath {
                name: "main".into(),
                source_roots: vec![source_root.into()],
                kotlin_compile_task: Some(kotlin.task.clone()),
                compiler_tasks: vec![java, kotlin],
                ..Default::default()
            }],
        }])));
        ws.class_inventory.get_mut().inject_object_for_tests(
            &home,
            ktcore::classfile::ClassFileFacts {
                identity: ktcore::jvm::ClassIdentity {
                    origin_id: crate::classpath::jdk_origin_id(&home.join("lib/src.zip")),
                    internal_name: "java/lang/Object".into(),
                    input: ktcore::jvm::InputIdentity::Binary {
                        content_sha256: [1; 32],
                    },
                },
                major_version: 61,
                minor_version: 0,
                access_flags: 1,
                super_name: None,
                interfaces: Vec::new(),
                signature: None,
                source_file: None,
                attribute_names: Vec::new(),
                inner_classes: Vec::new(),
                fields: Vec::new(),
                methods: Vec::new(),
            },
        );
        root
    }

    #[test]
    fn canonical_fast_merge_preserves_other_fast_meanings_ranges_and_severities() {
        use crate::diagnostics::{Diagnostic, DiagnosticCode, Severity};
        let canonical = Diagnostic {
            start_byte: 10,
            end_byte: 20,
            severity: Severity::Error,
            code: None,
            message: "compiler positive".into(),
        };
        let unresolved = Diagnostic {
            code: Some(DiagnosticCode::UnresolvedReference),
            message: "fast unresolved".into(),
            ..canonical.clone()
        };
        let fast = vec![
            Diagnostic {
                start_byte: 11,
                ..unresolved.clone()
            },
            Diagnostic {
                severity: Severity::Warning,
                ..unresolved.clone()
            },
            Diagnostic {
                code: Some(DiagnosticCode::CallShapeMismatch),
                ..unresolved.clone()
            },
        ];
        assert_eq!(
            Workspace::reconcile_fast_diagnostics(&[canonical.clone()], fast.clone()),
            fast
        );
        let warning = Diagnostic {
            severity: Severity::Warning,
            ..canonical
        };
        assert_eq!(
            Workspace::reconcile_fast_diagnostics(&[warning], vec![unresolved.clone()]),
            vec![unresolved]
        );
    }

    fn required_interface_actions_at(ws: &mut Workspace, key: &str, cursor: usize) -> Vec<Action> {
        let actions = ws.code_actions(key, cursor, cursor, cursor);
        eprintln!("required-interface query {key}:{cursor}, all_code_actions = {actions:#?}");
        actions
            .into_iter()
            .filter(|action| action.title == IMPLEMENT_INTERFACE_ACTION_TITLE)
            .collect()
    }

    fn required_interface_actions(ws: &mut Workspace, key: &str, src: &str) -> Vec<Action> {
        let cursor = src.find("// HERE").unwrap() + 3;
        required_interface_actions_at(ws, key, cursor)
    }

    fn preview_required_interface_action(
        ws: &mut Workspace,
        key: &str,
        src: &str,
        expected_return: &str,
    ) -> String {
        let actions = required_interface_actions(ws, key, src);
        assert_eq!(actions.len(), 1, "one implementation action: {actions:?}");
        assert_eq!(actions[0].kind, crate::actions::ActionKind::QuickFix);
        let edits = &actions[0].edits;
        let body_start = src.find('{').unwrap() + 1;
        let body_end = body_start + src[body_start..].find('}').unwrap();
        assert!(!edits.is_empty());
        assert!(edits.iter().all(|edit| edit.file == key
            && edit.start_byte >= body_start
            && edit.end_byte <= body_end));
        let applied = crate::edit::apply_to_text(key, src, edits).unwrap();
        assert_eq!(
            ws.doc_text(key).as_deref(),
            Some(src),
            "query cannot apply edits"
        );
        let language = SourceLanguage::for_key(key);
        let tree = LanguageParsers::new().parse(language, &applied);
        assert!(!tree.root_node().has_error(), "{applied}");
        let facts = language::symbol_facts(language, &tree, &applied);
        let methods: Vec<_> = facts
            .symbols
            .iter()
            .filter(|symbol| {
                symbol.name == "required" && symbol.container.as_deref() == Some("Impl")
            })
            .collect();
        assert_eq!(methods.len(), 1, "{applied}");
        let method = methods[0];
        assert_eq!(method.arity, Some(0));
        assert!(method.params.is_empty());
        assert_eq!(method.return_type.as_ref().unwrap().name, expected_return);
        assert_eq!(
            method.source_member.implementation(),
            Some(ktcore::symbol::MemberImplementation::Concrete)
        );
        assert_eq!(
            method.source_member.dispatch(),
            Some(ktcore::extensions::Dispatch::Instance)
        );
        assert_eq!(
            method.source_member.visibility(),
            Some(ktcore::extensions::Visibility::Public)
        );
        if language.is_java() {
            assert!(applied.contains("@Override") || applied.contains("@java.lang.Override"));
        } else {
            let words = applied.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(words.contains("override fun required"), "{applied}");
        }
        applied
    }

    fn assert_cross_language_interface_implementation(java_caller: bool) {
        let (
            owner_key,
            caller_key,
            clash_key,
            duplicate_key,
            owner,
            clash,
            caller,
            old_return,
            new_return,
        ) = if java_caller {
            (
                "mem:///Contract.kt",
                "mem:///Impl.java",
                "mem:///Clash.kt",
                "mem:///Duplicate.kt",
                "package generated\ninterface Contract { fun required(): Int }\n",
                "package generated\ninterface Clash { fun required(): Long }\n",
                "package generated;\npublic class Impl implements Contract {\n    // HERE\n}\nclass Other {\n    // OTHER\n}\nclass Calls { static long use(Impl value) { return value.required(); } }\n",
                "int",
                "long",
            )
        } else {
            (
                "mem:///Contract.java",
                "mem:///Impl.kt",
                "mem:///Clash.java",
                "mem:///Duplicate.java",
                "package generated;\npublic interface Contract { int required(); }\n",
                "package generated;\npublic interface Clash { long required(); }\n",
                "package generated\nclass Impl : Contract {\n    // HERE\n}\nclass Other {\n    // OTHER\n}\nfun use(value: Impl) = value.required()\n",
                "Int",
                "Long",
            )
        };
        let mut ws = Workspace::new();
        ws.open(owner_key, owner.into());
        ws.open(clash_key, clash.into());
        ws.open(caller_key, caller.into());
        assert!(
            required_interface_actions(&mut ws, caller_key, caller).is_empty(),
            "without a selected SDK inventory the source-only fixture is unavailable"
        );
        let sdk =
            interface_sdk_fixture(&mut ws, &[owner_key, clash_key, caller_key, duplicate_key]);
        let parent_start = owner.find("required").unwrap();
        let call_start = caller.rfind("required").unwrap();
        assert_eq!(
            ws.goto_definition(caller_key, call_start),
            vec![Def {
                file: owner_key.into(),
                start_byte: parent_start,
                end_byte: parent_start + "required".len(),
            }],
            "the current inherited method is already indexed and resolves"
        );
        let parent = ws
            .index
            .lookup_by_name("required")
            .into_iter()
            .find(|entry| entry.path.as_ref() == owner_key)
            .unwrap();
        assert_eq!(
            parent.sym.source_member.implementation(),
            Some(ktcore::symbol::MemberImplementation::Abstract)
        );
        assert_eq!(
            parent.sym.source_member.dispatch(),
            Some(ktcore::extensions::Dispatch::Instance)
        );
        assert_eq!(
            parent.sym.source_member.visibility(),
            Some(ktcore::extensions::Visibility::Public)
        );
        for cursor in [caller.find("// OTHER").unwrap() + 3, call_start] {
            assert!(
                required_interface_actions_at(&mut ws, caller_key, cursor).is_empty(),
                "do not generate for an unrelated class or method-body cursor"
            );
        }
        let interface_cursor = owner.find('{').unwrap() + 1;
        assert!(
            required_interface_actions_at(&mut ws, owner_key, interface_cursor).is_empty(),
            "the requested containing declaration must be a class"
        );
        let changed = if java_caller {
            owner.replace(": Int", ": Long")
        } else {
            owner.replace("int required", "long required")
        };
        let unknown = if java_caller {
            owner.replace(": Int", ": Missing")
        } else {
            owner.replace("int required", "Missing required")
        };
        ws.change(owner_key, unknown);
        assert!(
            required_interface_actions(&mut ws, caller_key, caller).is_empty(),
            "unresolved return types cannot produce a signature"
        );
        ws.change(owner_key, owner.into());
        ws.open(duplicate_key, changed.clone());
        assert!(
            required_interface_actions(&mut ws, caller_key, caller).is_empty(),
            "two current owners disagreeing on the same signature are ambiguous"
        );
        ws.change(duplicate_key, changed.replace("Contract", "OtherContract"));
        let conflicted = if java_caller {
            caller.replace("implements Contract", "implements Contract, Clash")
        } else {
            caller.replace(": Contract", ": Contract, Clash")
        };
        ws.change(caller_key, conflicted.clone());
        assert!(
            required_interface_actions(&mut ws, caller_key, &conflicted).is_empty(),
            "incompatible inherited return types cannot be repaired by one method"
        );
        ws.change(caller_key, caller.into());
        preview_required_interface_action(&mut ws, caller_key, caller, old_return);
        ws.change(owner_key, changed.clone());
        let applied = preview_required_interface_action(&mut ws, caller_key, caller, new_return);
        ws.change(caller_key, applied.clone());
        assert_eq!(ws.doc_text(owner_key).as_deref(), Some(changed.as_str()));
        assert!(
            required_interface_actions(&mut ws, caller_key, &applied).is_empty(),
            "an applied implementation must not be proposed again"
        );
        let implemented = applied.find("required").unwrap();
        let use_start = applied.rfind("required").unwrap();
        assert_eq!(
            ws.goto_definition(caller_key, use_start),
            vec![Def {
                file: caller_key.into(),
                start_byte: implemented,
                end_byte: implemented + "required".len(),
            }],
            "navigation must observe the applied current declaration"
        );
        ws.change(caller_key, caller.into());
        let empty_owner = if java_caller {
            "package generated\ninterface Contract {}\n"
        } else {
            "package generated;\npublic interface Contract {}\n"
        };
        ws.change(owner_key, empty_owner.into());
        assert!(
            required_interface_actions(&mut ws, caller_key, caller).is_empty(),
            "removing the inherited requirement invalidates the proposal"
        );
        ws.change(owner_key, owner.into());
        preview_required_interface_action(&mut ws, caller_key, caller, old_return);
        std::fs::remove_dir_all(sdk).unwrap();
    }

    #[test]
    fn implement_java_interface_in_kotlin_uses_current_signature_and_applied_edit() {
        assert_cross_language_interface_implementation(false);
    }

    #[test]
    fn implement_kotlin_interface_in_java_uses_current_signature_and_applied_edit() {
        assert_cross_language_interface_implementation(true);
    }

    #[test]
    fn interface_implementation_kotlin_long_survives_indexed_java_wrapper() {
        let kotlin_key = "mem:///Main.kt";
        let owner_key = "mem:///PlainOwner.java";
        let caller_key = "mem:///PlainUse.java";
        let kotlin = "package app\ninterface Contract { fun required(): Int }\nclass Impl : PlainOwner {\n    // HERE_KOTLIN\n}\nclass OtherKotlin {\n    // OTHER_KOTLIN\n}\nfun useRequired(value: Impl) = value.required()\n";
        let owner = "package app;\npublic interface PlainOwner { int required(); }\n";
        let caller = "package app;\npublic class PlainUse implements Contract {\n    // HERE_JAVA\n}\nclass OtherJava {\n    // OTHER_JAVA\n}\nclass Calls { static long use(PlainUse value) { return value.required(); } }\n";
        let preview = |ws: &mut Workspace, expected: &str| {
            let actions =
                required_interface_actions_at(ws, caller_key, caller.find("// HERE_JAVA").unwrap());
            assert_eq!(actions.len(), 1, "one implementation action: {actions:?}");
            let edits = &actions[0].edits;
            assert_eq!(edits.len(), 1);
            assert_eq!(
                (
                    edits[0].file.as_str(),
                    edits[0].start_byte,
                    edits[0].end_byte
                ),
                (
                    caller_key,
                    caller.find("\n}").unwrap() + 1,
                    caller.find("\n}").unwrap() + 1
                )
            );
            let applied = crate::edit::apply_to_text(caller_key, caller, edits).unwrap();
            assert_eq!(ws.doc_text(caller_key).as_deref(), Some(caller));
            let tree = LanguageParsers::new().parse(SourceLanguage::Java, &applied);
            assert!(!tree.root_node().has_error());
            let facts = language::symbol_facts(SourceLanguage::Java, &tree, &applied);
            let methods: Vec<_> = facts
                .symbols
                .iter()
                .filter(|symbol| {
                    symbol.name == "required" && symbol.container.as_deref() == Some("PlainUse")
                })
                .collect();
            assert_eq!(methods.len(), 1);
            assert_eq!(methods[0].return_type.as_ref().unwrap().name, expected);
            applied
        };
        let mut ws = Workspace::new();
        ws.open(kotlin_key, kotlin.into());
        ws.open(owner_key, owner.into());
        ws.open(caller_key, caller.into());
        // The actual selected stdlib has expect/actual Long; the selected JDK adds java.lang.Long.
        ws.open(
            "mem:///CommonPrimitives.kt",
            "package kotlin\npublic expect class Long\n".into(),
        );
        ws.open(
            "mem:///JvmPrimitives.kt",
            "package kotlin\npublic actual class Long private constructor()\n".into(),
        );
        ws.open(
            "mem:///Long.java",
            "package java.lang; public final class Long {}".into(),
        );
        let sdk = interface_sdk_fixture(
            &mut ws,
            &[
                kotlin_key,
                owner_key,
                caller_key,
                "mem:///CommonPrimitives.kt",
                "mem:///JvmPrimitives.kt",
                "mem:///Long.java",
                "mem:///Shadow.kt",
            ],
        );
        assert_eq!(ws.index.lookup_type("Long").len(), 3);
        preview(&mut ws, "int");
        let changed = kotlin.replace(": Int", ": Long");
        ws.change(kotlin_key, changed.clone());
        ws.change(owner_key, owner.replace("int required", "long required"));
        ws.change(caller_key, caller.into());
        let parent = ws
            .index
            .lookup_by_name("required")
            .into_iter()
            .find(|entry| entry.path.as_ref() == kotlin_key)
            .unwrap();
        let written = parent.sym.return_type.as_ref().unwrap();
        assert_eq!(written.name, "Long");
        assert!(written.qualification.is_none());
        let scoped = ws.index.scoped_for_file(caller_key);
        let resolved = crate::infer::declared_type(
            &scoped,
            written,
            &crate::infer::FileCtx::new("app".into(), Vec::new()),
        );
        eprintln!(
            "actual header Long candidates: {:?}; written: {written:?}; resolved: {resolved:?}",
            ws.index.lookup_type("Long")
        );
        let applied = preview(&mut ws, "long");
        assert_eq!(
            resolved,
            crate::types::Type::class("Long", Some("kotlin".into()))
        );
        for imported in [
            "import java.lang.*",
            "import kotlin.*\nimport java.lang.*",
            "import java.lang.*\nimport kotlin.*",
        ] {
            ws.change(
                kotlin_key,
                changed.replacen("package app\n", &format!("package app\n{imported}\n"), 1),
            );
            preview(&mut ws, "long");
        }
        for imported in ["import java.lang.Long", "import java.lang.Long as Long"] {
            ws.change(
                kotlin_key,
                changed.replacen("package app\n", &format!("package app\n{imported}\n"), 1),
            );
            assert!(
                required_interface_actions(&mut ws, caller_key, caller).is_empty(),
                "an explicitly imported boxed return cannot authorize primitive long: {imported}"
            );
        }
        ws.change(kotlin_key, changed.clone());
        ws.open("mem:///Shadow.kt", "package app\nclass Long\n".into());
        assert!(
            required_interface_actions(&mut ws, caller_key, caller).is_empty(),
            "a same-package nominal return cannot authorize primitive long"
        );
        ws.change("mem:///Shadow.kt", "package app\n".into());
        ws.change(caller_key, applied.clone());
        assert!(
            required_interface_actions(&mut ws, caller_key, &applied).is_empty(),
            "the actual applied Long method is not duplicated"
        );
        ws.change(caller_key, caller.into());
        ws.change(kotlin_key, kotlin.into());
        ws.change(owner_key, owner.into());
        preview(&mut ws, "int");
        std::fs::remove_dir_all(sdk).unwrap();
    }

    #[test]
    fn interface_implementation_ignores_unrelated_stdlib_extension_prefixes() {
        // The selected 2.2.20 JvmClassMapping.kt declares both public extension properties.
        // Index their authored receiver/header facts; the explicit SDK fixture supplies Object.
        let mappings = "package kotlin.jvm\nimport kotlin.reflect.KClass\npublic val <T> KClass<T>.java: Class<T> get() = throw UnsupportedOperationException()\npublic val <T : Any> Class<T>.kotlin: KClass<T> get() = throw UnsupportedOperationException()\n";
        for java_caller in [false, true] {
            let (owner_key, caller_key, owner, caller, result) = if java_caller {
                (
                    "mem:///Contract.kt",
                    "mem:///Impl.java",
                    "package generated\ninterface Contract { fun required(): Int }\n",
                    "package generated;\npublic class Impl implements Contract {\n    // HERE\n}\n",
                    "int",
                )
            } else {
                (
                    "mem:///Contract.java",
                    "mem:///Impl.kt",
                    "package generated;\npublic interface Contract { int required(); }\n",
                    "package generated\nclass Impl : Contract {\n    // HERE\n}\n",
                    "Int",
                )
            };
            let mut ws = Workspace::new();
            ws.open(owner_key, owner.into());
            ws.open(caller_key, caller.into());
            ws.open("mem:///JvmClassMapping.kt", mappings.into());
            ws.open(
                "mem:///Class.java",
                "package java.lang; public final class Class<T> {}".into(),
            );
            ws.open(
                "mem:///KClass.kt",
                "package kotlin.reflect\npublic interface KClass<T> {}".into(),
            );
            ws.open(
                "mem:///Other.kt",
                "package unrelated\nclass Other {}".into(),
            );
            let sdk = interface_sdk_fixture(
                &mut ws,
                &[
                    owner_key,
                    caller_key,
                    "mem:///JvmClassMapping.kt",
                    "mem:///Class.java",
                    "mem:///KClass.kt",
                    "mem:///Other.kt",
                    "mem:///Shadow.kt",
                    "mem:///Shadow.java",
                ],
            );
            for prefix in ["java", "kotlin"] {
                let entries = ws.index.lookup_by_name(prefix);
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].sym.kind, SymbolKind::Property);
                assert!(entries[0].sym.extension_receiver.is_some());
            }
            let cursor = caller.find("// HERE").unwrap();
            assert_eq!(
                required_interface_actions_at(&mut ws, caller_key, cursor).len(),
                1,
                "the native first-slash cursor must produce the action with both stdlib properties indexed"
            );
            preview_required_interface_action(&mut ws, caller_key, caller, result);
            let prefix = if java_caller { "java" } else { "kotlin" };
            ws.open(
                "mem:///Shadow.java",
                format!("package generated; class {prefix} {{}}"),
            );
            assert!(
                required_interface_actions_at(&mut ws, caller_key, cursor).is_empty(),
                "a visible type can obscure the generated qualified prefix"
            );
            ws.change("mem:///Shadow.java", "package generated;".into());
            if !java_caller {
                for shadow in [
                    "val kotlin = 0",
                    "val Any.kotlin: Int get() = 0",
                    "val Impl.kotlin: Int get() = 0",
                    "val Missing.kotlin: Int get() = 0",
                    "typealias Alias = Impl\nval Alias.kotlin: Int get() = 0",
                    "import generated.Impl as Other\nval Other.kotlin: Int get() = 0",
                    "val <T> T.kotlin: Int get() = 0",
                ] {
                    ws.open("mem:///Shadow.kt", format!("package generated\n{shadow}\n"));
                    if shadow.starts_with("import") {
                        let receiver = ws
                            .index
                            .lookup_by_name("kotlin")
                            .iter()
                            .find(|entry| entry.path.as_ref() == "mem:///Shadow.kt")
                            .unwrap()
                            .sym
                            .extension_receiver
                            .as_ref()
                            .unwrap();
                        assert_eq!(
                            (
                                receiver.ty.name.as_str(),
                                receiver.ty.qualification.as_deref()
                            ),
                            ("Impl", Some("generated")),
                            "import aliases retain their authored target"
                        );
                    }
                    assert!(
                        required_interface_actions_at(&mut ws, caller_key, cursor).is_empty(),
                        "an applicable or unproven prefix value remains a blocker: {shadow}"
                    );
                }
                ws.change("mem:///Shadow.kt", "package generated\n".into());
                preview_required_interface_action(&mut ws, caller_key, caller, result);
            }
            std::fs::remove_dir_all(sdk).unwrap();
        }
    }

    #[test]
    fn interface_implementation_java_prefix_ignores_unresolved_extension_receivers() {
        let owner_key = "mem:///Contract.kt";
        let caller_key = "mem:///Impl.java";
        let caller =
            "package generated;\npublic class Impl implements Contract {\n    // HERE\n}\n";
        let mut ws = Workspace::new();
        ws.open(
            owner_key,
            "package generated\ninterface Contract { fun required(): Int }\n".into(),
        );
        ws.open(caller_key, caller.into());
        ws.open("mem:///JvmClassMapping.kt",
            "package kotlin.jvm\nimport kotlin.reflect.KClass\npublic val <T> KClass<T>.java: Class<T> get() = throw UnsupportedOperationException()\n".into());
        let sdk = interface_sdk_fixture(
            &mut ws,
            &[
                owner_key,
                caller_key,
                "mem:///JvmClassMapping.kt",
                "mem:///CommonKClass.kt",
                "mem:///JvmKClass.kt",
                "mem:///Shadow.java",
            ],
        );
        assert!(ws.index.lookup_type("KClass").is_empty());
        let entries = ws.index.lookup_by_name("java");
        assert_eq!(entries.len(), 1);
        assert!(entries[0].sym.extension_receiver.is_some());
        let cursor = caller.find("// HERE").unwrap();
        assert_eq!(
            required_interface_actions_at(&mut ws, caller_key, cursor).len(),
            1,
            "Kotlin extension receiver availability cannot obscure a Java type name"
        );
        // The actual selected stdlib source archive contains both expect/actual KClass heads.
        ws.open(
            "mem:///CommonKClass.kt",
            "package kotlin.reflect\npublic expect interface KClass<T : Any> : KClassifier {}\n"
                .into(),
        );
        ws.open("mem:///JvmKClass.kt",
            "package kotlin.reflect\npublic actual interface KClass<T : Any> : KDeclarationContainer, KAnnotatedElement, KClassifier {}\n".into());
        assert_eq!(ws.index.lookup_type("KClass").len(), 2);
        assert_eq!(
            required_interface_actions_at(&mut ws, caller_key, cursor).len(),
            1,
            "ambiguous Kotlin extension receiver heads cannot obscure a Java type name"
        );
        preview_required_interface_action(&mut ws, caller_key, caller, "int");
        ws.open(
            "mem:///Shadow.java",
            "package generated; class java {}".into(),
        );
        assert!(
            required_interface_actions_at(&mut ws, caller_key, cursor).is_empty(),
            "a real visible Java type still obscures the generated qualified type names"
        );
        std::fs::remove_dir_all(sdk).unwrap();
    }

    #[test]
    fn interface_implementation_withholds_unusable_or_stale_selected_sdk() {
        use crate::classpath::{CompilerLanguage, ProjectSnapshot};
        for java_caller in [false, true] {
            let (contract_key, caller_key, contract, caller) = if java_caller {
                (
                    "mem:///Contract.kt",
                    "mem:///Impl.java",
                    "package generated\ninterface Contract { fun required(): Int }\n",
                    "package generated; public class Impl implements Contract { // HERE\n}",
                )
            } else {
                (
                    "mem:///Contract.java",
                    "mem:///Impl.kt",
                    "package generated; public interface Contract { int required(); }",
                    "package generated\nclass Impl : Contract { // HERE\n}",
                )
            };
            let mut ws = Workspace::new();
            ws.open(contract_key, contract.into());
            ws.open(caller_key, caller.into());
            let sdk = interface_sdk_fixture(&mut ws, &[contract_key, caller_key]);
            assert_eq!(
                required_interface_actions(&mut ws, caller_key, caller).len(),
                1
            );
            let model = ws.project_model.clone().unwrap();
            for fault in [
                "SDK",
                "KotlinSDK",
                "relativeSDK",
                "bootstrap-absent",
                "bootstrap-custom",
                "release",
                "no-jdk",
                "jdk-release",
                "argfile",
                "plugin",
                "primary",
                "ambiguous",
                "custom",
                "source",
                "incomplete",
            ] {
                let mut modules = model.modules.clone();
                let source = &mut modules[0].source_sets[0];
                match fault {
                    "SDK" => source.compiler_tasks[0].java_home = None,
                    "KotlinSDK" => source.compiler_tasks[1].java_home = None,
                    "relativeSDK" => {
                        source.compiler_tasks[0].java_home = Some("relative-sdk".into())
                    }
                    "bootstrap-absent" => {
                        source.compiler_tasks[0]
                            .options
                            .remove("bootstrapClasspath");
                    }
                    "bootstrap-custom" => {
                        source.compiler_tasks[0].options.insert(
                            "bootstrapClasspath".into(),
                            serde_json::json!(["custom-boot.jar"]),
                        );
                    }
                    "release" => {
                        source.compiler_tasks[0]
                            .options
                            .insert("release".into(), serde_json::json!(8));
                    }
                    "no-jdk" => source.compiler_tasks[1].arguments.push("-no-jdk".into()),
                    "jdk-release" => source.compiler_tasks[1]
                        .arguments
                        .push("-Xjdk-release=8".into()),
                    "argfile" => source.compiler_tasks[1]
                        .arguments
                        .push("@opaque.args".into()),
                    "plugin" => source.compiler_tasks[1]
                        .arguments
                        .push("-Xplugin=opaque.jar".into()),
                    "primary" => source.kotlin_compile_task = None,
                    "ambiguous" => source.compiler_tasks.push(source.compiler_tasks[1].clone()),
                    "custom" => source.compiler_tasks[1].task_type = "custom.Compiler".into(),
                    "source" => source.compiler_tasks[1].source_files.clear(),
                    "incomplete" => source.compiler_metadata_incomplete = true,
                    _ => unreachable!(),
                }
                // Keep the same injected archive; only the accepted immutable model changes.
                ws.project_model = Some(Arc::new(ProjectSnapshot::new(modules)));
                assert!(
                    required_interface_actions(&mut ws, caller_key, caller).is_empty(),
                    "{fault}"
                );
            }
            ws.project_model = Some(model);
            ws.change(contract_key, format!("{contract}\nclass Broken {{"));
            assert!(required_interface_actions(&mut ws, caller_key, caller).is_empty());
            ws.change(contract_key, contract.into());
            let prefix = if java_caller { "java" } else { "kotlin" };
            ws.change(caller_key, format!("{caller}\nclass {prefix} {{}}"));
            assert!(required_interface_actions(&mut ws, caller_key, caller).is_empty());
            ws.change(caller_key, caller.into());
            ws.project_refresh_pending = true;
            assert!(required_interface_actions(&mut ws, caller_key, caller).is_empty());
            ws.project_refresh_pending = false;
            assert_eq!(
                required_interface_actions(&mut ws, caller_key, caller).len(),
                1
            );
            let task = ws.project_model.as_ref().unwrap().modules[0].source_sets[0]
                .compiler_tasks
                .iter()
                .find(|task| task.language == CompilerLanguage::Java)
                .unwrap();
            std::fs::write(
                task.java_home
                    .as_ref()
                    .unwrap()
                    .join("jmods/java.base.jmod"),
                b"changed SDK inventory must be reread",
            )
            .unwrap();
            assert!(required_interface_actions(&mut ws, caller_key, caller).is_empty());
            std::fs::remove_dir_all(sdk).unwrap();
        }
    }

    #[test]
    fn interface_implementation_rechecks_closed_owner_before_watched_refresh() {
        for java_caller in [false, true] {
            let root = unique_tmp("interface-current-owner");
            std::fs::create_dir_all(&root).unwrap();
            let (contract_file, caller_file, contract, caller, old, new) = if java_caller {
                (
                    "Contract.kt",
                    "Impl.java",
                    "package generated\ninterface Contract { fun required(): Int }",
                    "package generated; public class Impl implements Contract { // HERE\n}",
                    "required(): Int",
                    "required(): Long",
                )
            } else {
                (
                    "Contract.java",
                    "Impl.kt",
                    "package generated; public interface Contract { int required(); }",
                    "package generated\nclass Impl : Contract { // HERE\n}",
                    "int required()",
                    "long required()",
                )
            };
            let contract_key = crate::paths::file_key(&root.join(contract_file));
            let caller_key = crate::paths::file_key(&root.join(caller_file));
            std::fs::write(&contract_key, contract).unwrap();
            std::fs::write(&caller_key, caller).unwrap();
            let mut ws = Workspace::new();
            ws.open(&caller_key, caller.into());
            let sdk = interface_sdk_fixture(&mut ws, &[&contract_key, &caller_key]);
            ws.scan(&root);
            assert_eq!(
                required_interface_actions(&mut ws, &caller_key, caller).len(),
                1
            );
            std::fs::write(&contract_key, contract.replace(old, new)).unwrap();
            assert!(
                required_interface_actions(&mut ws, &caller_key, caller).is_empty(),
                "disk changed before watched refresh"
            );
            ws.scan(&root);
            let actions = required_interface_actions(&mut ws, &caller_key, caller);
            assert_eq!(actions.len(), 1);
            assert!(actions[0].edits[0].new_text.contains(if java_caller {
                "long required"
            } else {
                "kotlin.Long"
            }));
            if !java_caller {
                let shadow_key = crate::paths::file_key(&root.join("Shadow.kt"));
                let shadow = "package generated\nclass Box {}\nval Box.kotlin: Int get() = 0\n";
                std::fs::write(&shadow_key, shadow).unwrap();
                ws.scan(&root);
                assert!(!ws.open_docs.contains_key(&shadow_key));
                assert!(
                    ws.index
                        .lookup_by_name("kotlin")
                        .iter()
                        .any(|entry| entry.path.as_ref() == shadow_key
                            && entry.tier == Tier::Volatile)
                );
                std::fs::write(&shadow_key, shadow.replace("Box.kotlin", "Impl.kotlin")).unwrap();
                assert!(
                    required_interface_actions(&mut ws, &caller_key, caller).is_empty(),
                    "closed prefix receiver facts may change before watched refresh"
                );
                ws.open(&shadow_key, shadow.into());
                assert_eq!(
                    required_interface_actions(&mut ws, &caller_key, caller).len(),
                    1,
                    "the current clean unrelated extension receiver can be rejected"
                );
                let head_key = crate::paths::file_key(&root.join("Head.kt"));
                let head = "package generated\nclass ClosedBox {}\n";
                std::fs::write(&head_key, head).unwrap();
                ws.scan(&root);
                ws.change(
                    &shadow_key,
                    "package generated\nval ClosedBox.kotlin: Int get() = 0\n".into(),
                );
                assert!(!ws.open_docs.contains_key(&head_key));
                assert!(ws
                    .index
                    .lookup_type("ClosedBox")
                    .iter()
                    .any(|entry| entry.path.as_ref() == head_key && entry.tier == Tier::Volatile));
                std::fs::write(&head_key, "package generated\ntypealias ClosedBox = Impl\n")
                    .unwrap();
                assert!(
                    required_interface_actions(&mut ws, &caller_key, caller).is_empty(),
                    "closed formal receiver heads may become applicable aliases before refresh"
                );
                ws.open(&head_key, head.into());
                assert_eq!(
                    required_interface_actions(&mut ws, &caller_key, caller).len(),
                    1,
                    "a current unrelated nominal receiver head can prove rejection"
                );
                ws.change(&shadow_key, shadow.into());
                ws.change(&shadow_key, format!("{shadow}\nclass Broken {{"));
                assert!(ws.open_docs[&shadow_key].tree.root_node().has_error());
                assert!(
                    required_interface_actions(&mut ws, &caller_key, caller).is_empty(),
                    "a broken prefix source cannot prove receiver rejection"
                );
                ws.change(&shadow_key, shadow.into());
                struct Provider;
                impl crate::analysis_extensions::AnalysisProvider for Provider {
                    fn id(&self) -> &str {
                        "prefix-source-test"
                    }
                    fn analyze(
                        &self,
                        _: &mut ktcore::extensions::ReadSession<'_>,
                    ) -> Result<(), ktcore::extensions::ContractError> {
                        Ok(())
                    }
                }
                ws.analysis_extensions =
                    crate::analysis_extensions::AnalysisExtensions::new(vec![Arc::new(Provider)]);
                assert!(ws.analysis_extensions.needs_source(&shadow_key));
                assert!(
                    required_interface_actions(&mut ws, &caller_key, caller).is_empty(),
                    "synthetic source providers keep the ordinary action unavailable"
                );
            }
            std::fs::remove_dir_all(sdk).unwrap();
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn workspace_symbols_preserve_payload_duplicates_and_stable_tier_order() {
        use crate::types::TypeRef;

        let mut first = IndexedSymbol::new(
            "Helper",
            SymbolKind::Function,
            "app",
            Some("Owner".into()),
            20,
            26,
        );
        let mut list = TypeRef::simple("List");
        list.args.push(TypeRef::simple("T"));
        first.documentation = Some("first overload".into());
        first.ext_receiver = Some("Scope".into());
        first.arity = Some(2);
        first.trailing_lambda_receiver_type = Some(Box::new(TypeRef::simple("Scope")));
        first.return_type = Some(TypeRef::simple("T"));
        first.params = vec![list.clone(), TypeRef::simple("Function1")];
        first.type_params = vec!["T".into()];
        let mut duplicate = first.clone();
        duplicate.documentation = Some("duplicate".into());
        let mut lower = first.clone();
        lower.name = "helper".into();
        lower.documentation = Some("lowercase".into());
        let mut later = first.clone();
        later.end_byte = 29;
        later.documentation = Some("later range".into());

        let mut ws = Workspace::new();
        ws.index.replace_file(
            "mem:///Same.kt",
            vec![later, lower, first, duplicate],
            Tier::Volatile,
        );
        ws.index.replace_file(
            "mem:///Long.kt",
            vec![IndexedSymbol::new(
                "HelpLong",
                SymbolKind::Class,
                "app",
                None,
                10,
                18,
            )],
            Tier::Volatile,
        );
        ws.index.replace_file(
            "lib:///Short.kt",
            vec![IndexedSymbol::new(
                "Help",
                SymbolKind::Class,
                "lib",
                None,
                0,
                4,
            )],
            Tier::Durable,
        );

        let symbols = ws.workspace_symbols("HELP");
        let ordered = symbols
            .iter()
            .map(|s| {
                (
                    s.file.as_str(),
                    s.name.as_str(),
                    s.end_byte,
                    s.documentation.as_deref(),
                    s.tier,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            ordered,
            vec![
                (
                    "mem:///Same.kt",
                    "Helper",
                    26,
                    Some("first overload"),
                    Tier::Volatile
                ),
                (
                    "mem:///Same.kt",
                    "Helper",
                    26,
                    Some("duplicate"),
                    Tier::Volatile
                ),
                (
                    "mem:///Same.kt",
                    "helper",
                    26,
                    Some("lowercase"),
                    Tier::Volatile
                ),
                (
                    "mem:///Same.kt",
                    "Helper",
                    29,
                    Some("later range"),
                    Tier::Volatile
                ),
                ("mem:///Long.kt", "HelpLong", 18, None, Tier::Volatile),
                ("lib:///Short.kt", "Help", 4, None, Tier::Durable),
            ]
        );
        assert_eq!(
            symbols[0],
            SymbolSummary {
                file: "mem:///Same.kt".into(),
                name: "Helper".into(),
                kind: SymbolKind::Function,
                package: "app".into(),
                container: Some("Owner".into()),
                start_byte: 20,
                end_byte: 26,
                documentation: Some("first overload".into()),
                tier: Tier::Volatile,
                supertypes: vec![],
                ext_receiver: Some("Scope".into()),
                arity: Some(2),
                trailing_lambda_receiver_type: Some(TypeRef::simple("Scope")),
                return_type: Some(TypeRef::simple("T")),
                value_type: None,
                params: vec![list, TypeRef::simple("Function1")],
                type_params: vec!["T".into()],
            }
        );
        let before = ws.workspace_symbols("HELP");
        let same_file = ws
            .index
            .entries_for_file("mem:///Same.kt")
            .into_iter()
            .map(|entry| (*entry.sym).clone())
            .collect();
        ws.index.remove_file("mem:///Same.kt");
        ws.index
            .replace_file("mem:///Same.kt", same_file, Tier::Volatile);
        assert_eq!(ws.workspace_symbols("HELP"), before);
    }

    #[test]
    fn workspace_symbols_cap_after_filter_and_rank_including_empty_query() {
        let mut ws = Workspace::new();
        let symbols = (0..205)
            .map(|i| {
                let start = (204 - i) * 10;
                let mut symbol = IndexedSymbol::new(
                    format!("Hit{i:03}"),
                    SymbolKind::Function,
                    "app",
                    None,
                    start,
                    start + 6,
                );
                symbol.documentation = Some(format!("payload {i}"));
                symbol
            })
            .collect();
        ws.index
            .replace_file("mem:///Bulk.kt", symbols, Tier::Volatile);
        ws.index.replace_file(
            "lib:///Bulk.kt",
            vec![IndexedSymbol::new(
                "Hit000",
                SymbolKind::Function,
                "lib",
                None,
                0,
                6,
            )],
            Tier::Durable,
        );

        let symbols = ws.workspace_symbols("");
        assert_eq!(symbols.len(), 200);
        let payloads = symbols
            .iter()
            .map(|s| {
                (
                    s.name.clone(),
                    s.documentation.clone(),
                    s.start_byte,
                    s.tier,
                )
            })
            .collect::<Vec<_>>();
        let expected = (0..200)
            .map(|i| {
                (
                    format!("Hit{i:03}"),
                    Some(format!("payload {i}")),
                    (204 - i) * 10,
                    Tier::Volatile,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(payloads, expected);
        let selective = ws.workspace_symbols("HiT000");
        assert_eq!(
            selective
                .iter()
                .map(|s| (s.file.as_str(), s.tier))
                .collect::<Vec<_>>(),
            vec![
                ("mem:///Bulk.kt", Tier::Volatile),
                ("lib:///Bulk.kt", Tier::Durable),
            ]
        );
        assert!(ws.workspace_symbols("MissingSymbol").is_empty());
    }

    #[test]
    fn workspace_symbols_match_unicode_and_joined_package_container() {
        let key = "mem:///Unicode.kt";
        let mut ws = Workspace::new();
        ws.index.replace_file(
            key,
            [
                ("İSTANBUL", "möd.api", Some("Ångström")),
                ("Straße", "", None),
                ("Plain", "möd.api", None),
                ("RootMember", "", Some("Owner")),
                ("Rootless", "", None),
            ]
            .into_iter()
            .enumerate()
            .map(|(i, (name, package, container))| {
                IndexedSymbol::new(
                    name,
                    SymbolKind::Class,
                    package,
                    container.map(str::to_owned),
                    i * 20,
                    i * 20 + name.len(),
                )
            })
            .collect(),
            Tier::Volatile,
        );
        let cases: &[(&str, &[&str])] = &[
            ("İST", &["İSTANBUL"]),
            ("i\u{307}st", &["İSTANBUL"]),
            ("istanbul", &[]),
            ("MÖD.API.ÅNG", &["İSTANBUL"]),
            ("api.Å", &["İSTANBUL"]),
            ("apiÅ", &[]),
            ("möd.api", &["Plain", "İSTANBUL"]),
            ("möd.api.", &["İSTANBUL"]),
            ("OWNER", &["RootMember"]),
            ("STRAßE", &["Straße"]),
            ("STRASSE", &[]),
            (
                "",
                &["Plain", "Straße", "Rootless", "İSTANBUL", "RootMember"],
            ),
        ];
        let summaries = ws.document_symbols(key);
        for &(query, expected) in cases {
            let actual = ws.workspace_symbols(query);
            assert_eq!(
                actual.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
                expected,
                "{query}"
            );
            for summary in &summaries {
                assert_eq!(
                    summary.matches_query(query),
                    expected.contains(&summary.name.as_str()),
                    "{}: {query}",
                    summary.name
                );
            }
        }
    }

    #[test]
    fn annotation_owner_metadata_survives_close_and_cleans_up_deleted_and_reopened_sources() {
        for (file, annotated, plain) in [
            (
                "Person.java",
                "package oracle; @Marker class Person { @Marker int value; }",
                "package oracle; class Person { int value; }",
            ),
            (
                "Person.kt",
                "package oracle\n@Marker class Person { @Marker val value: Int = 1 }",
                "package oracle\nclass Person { val value: Int = 1 }",
            ),
        ] {
            let root = unique_tmp("annotation-lifecycle");
            std::fs::create_dir_all(&root).unwrap();
            let path = root.join(file);
            std::fs::write(&path, annotated).unwrap();
            let key = crate::paths::file_key(&path);
            let mut ws = Workspace::new();
            ws.scan(&root);
            ws.assume_index_complete_for_tests();
            let initial = ws.effective_completeness();
            assert!(initial.compiler_producers_complete);
            assert_eq!(initial.member_surfaces_unknown.len(), 1);
            ws.open(&key, plain.into());
            assert!(ws
                .effective_completeness()
                .member_surfaces_unknown
                .is_empty());
            ws.close(&key);
            assert_eq!(ws.effective_completeness().member_surfaces_unknown.len(), 1);
            std::fs::remove_file(&path).unwrap();
            ws.close(&key);
            assert!(ws
                .effective_completeness()
                .member_surfaces_unknown
                .is_empty());
            assert!(!ws.annotated_type_owners.contains_key(&key));
            std::fs::write(&path, annotated).unwrap();
            ws.scan(&root);
            assert_eq!(ws.effective_completeness().member_surfaces_unknown.len(), 1);
            std::fs::remove_file(&path).unwrap();
            ws.scan(&root);
            assert!(ws
                .effective_completeness()
                .member_surfaces_unknown
                .is_empty());
            assert!(!ws.annotated_type_owners.contains_key(&key));
        }
    }

    #[test]
    fn model_source_refresh_with_no_reparsed_files_republishes_provider_results() {
        struct Provider;
        impl crate::analysis_extensions::AnalysisProvider for Provider {
            fn id(&self) -> &str {
                "refresh-test"
            }
            fn analyze(
                &self,
                read: &mut ktcore::extensions::ReadSession<'_>,
            ) -> Result<(), ktcore::extensions::ContractError> {
                read.diagnostic(crate::diagnostics::Diagnostic {
                    start_byte: 0,
                    end_byte: 1,
                    severity: crate::diagnostics::Severity::Hint,
                    code: None,
                    message: "accepted".into(),
                })
            }
        }
        let mut ws = Workspace::with_analysis_providers(vec![Arc::new(Provider)]);
        ws.open("Current.kt", "class Current".into());
        assert_eq!(ws.index.contributions_for_file("Current.kt").len(), 1);
        ws.begin_analysis_generation();
        assert!(ws.index.contributions_for_file("Current.kt").is_empty());
        assert_eq!(
            ws.apply_model_sources(ModelSourceUpdate {
                current: HashSet::from(["Current.kt".into()]),
                batches: Vec::new(),
                complete: true,
            }),
            0
        );
        assert_eq!(ws.index.contributions_for_file("Current.kt").len(), 1);
    }

    #[test]
    fn closed_import_alias_references_and_rename_refresh_after_unsaved_import_close() {
        let root = unique_tmp("closed-import-alias");
        std::fs::create_dir_all(&root).unwrap();
        let api_path = root.join("Api.kt");
        let alias_path = root.join("Alias.kt");
        let other_path = root.join("Other.kt");
        let api = "package lib\nclass Value\nfun Value.compat(): Int = 1\n";
        let alias = "package app\nimport lib.Value\nimport lib.compat as oldApi\nfun use(v: Value): Int = v.oldApi()\n";
        let other = "package other\nimport lib.Value\nfun Value.compat(): Int = 2\n";
        for (path, text) in [(&api_path, api), (&alias_path, alias), (&other_path, other)] {
            std::fs::write(path, text).unwrap();
        }
        let api_key = crate::paths::file_key(&api_path);
        let alias_key = crate::paths::file_key(&alias_path);
        let other_key = crate::paths::file_key(&other_path);
        let declaration = Def {
            file: api_key.clone(),
            start_byte: api.find("compat").unwrap(),
            end_byte: api.find("compat").unwrap() + "compat".len(),
        };
        let mut expected = vec![declaration.clone()];
        for name in ["compat", "oldApi"] {
            for (start, _) in alias.match_indices(name) {
                expected.push(Def {
                    file: alias_key.clone(),
                    start_byte: start,
                    end_byte: start + name.len(),
                });
            }
        }
        expected.sort();
        let mut ws = Workspace::new();
        assert_eq!(ws.scan(&root), 3);
        assert!(
            ws.open_docs.is_empty(),
            "all initial queries use closed-file parsing"
        );
        assert_eq!(
            ws.references(&api_key, declaration.start_byte, true),
            expected
        );
        let edits = ws
            .rename(&api_key, declaration.start_byte, "freshCompat")
            .unwrap();
        assert_eq!(edits.len(), 2, "{edits:?}");
        let expected_api = api.replace("compat", "freshCompat");
        let expected_alias = alias.replace("lib.compat as oldApi", "lib.freshCompat as oldApi");
        for (key, original, after) in [
            (&api_key, api, expected_api.as_str()),
            (&alias_key, alias, expected_alias.as_str()),
            (&other_key, other, other),
        ] {
            assert_eq!(
                crate::edit::apply_to_text(key, original, &edits).unwrap(),
                after
            );
            assert_eq!(std::fs::read_to_string(key).unwrap(), original);
        }
        let dirty = alias
            .replace("lib.compat as oldApi", "other.compat as dirtyApi")
            .replace("v.oldApi()", "v.dirtyApi()");
        ws.open(alias_key.clone(), dirty.clone());
        assert_eq!(
            ws.goto_definition(&alias_key, dirty.rfind("dirtyApi").unwrap()),
            vec![Def {
                file: other_key,
                start_byte: other.find("compat").unwrap(),
                end_byte: other.find("compat").unwrap() + "compat".len(),
            }]
        );
        assert_eq!(
            ws.references(&api_key, declaration.start_byte, true),
            vec![declaration.clone()]
        );
        ws.close(&alias_key);
        assert!(ws.open_docs.is_empty());
        assert_eq!(
            ws.references(&api_key, declaration.start_byte, true),
            expected
        );
        let restored_edits = ws
            .rename(&api_key, declaration.start_byte, "freshCompat")
            .unwrap();
        assert_eq!(restored_edits.len(), 2, "{restored_edits:?}");
        assert_eq!(
            crate::edit::apply_to_text(&alias_key, alias, &restored_edits).unwrap(),
            expected_alias
        );
        assert_eq!(std::fs::read_to_string(&alias_path).unwrap(), alias);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_ready_standalone_java_reports_authored_call_errors() {
        let root = unique_tmp("runtime-java-diagnostics");
        std::fs::create_dir_all(&root).unwrap();
        let helper = "package app; interface Worker { void assist(); } public class Helper extends Base implements Worker { public void assist() {} public void combine(String name, int count) {} public void adopt(Cat cat) {} } class Base {} class Cat {} class Dog {}";
        let main = "package app; public class Main { void run(Helper helper) { helper.combine(\"Ada\"); helper.adopt(new Dog()); missingCall(); } }";
        std::fs::write(root.join("Helper.java"), helper).unwrap();
        std::fs::write(root.join("Main.java"), main).unwrap();
        let mut ws = Workspace::new();
        ws.scan(&root);
        runtime_ready_source_diagnostic_fixture(&mut ws, &root);
        assert!(ws.completeness.project_scan_complete);
        assert!(!ws.effective_completeness().compiler_producers_complete);
        let key = crate::paths::file_key(&root.join("Main.java"));
        ws.open(&key, main.into());
        let diagnostics = ws.diagnostics(&key);
        assert_eq!(diagnostics.len(), 3, "{diagnostics:?}");
        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message == "Unresolved reference: missingCall"));
        assert_eq!(
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code
                    == Some(crate::diagnostics::DiagnosticCode::CallShapeMismatch))
                .count(),
            2
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selected_sdk_model_switch_retires_foreign_symbols_and_rejects_late_batches() {
        let root = unique_tmp("sdk-model-switch");
        let project = root.join("project");
        std::fs::create_dir_all(project.join("src")).unwrap();
        let key = crate::paths::file_key(&project.join("src/Use.java"));
        let source = "package app; class Use { void use(Thread thread) { thread.th; } }";
        std::fs::write(&key, source).unwrap();
        for feature in [17, 21] {
            let home = root.join(format!("jdk{feature}"));
            std::fs::create_dir_all(home.join("lib")).unwrap();
            std::fs::write(home.join("lib/src.zip"), "explicit SDK source stamp").unwrap();
            std::fs::write(
                home.join("release"),
                format!("JAVA_VERSION=\"{feature}.0.1\"\n"),
            )
            .unwrap();
        }
        let model = |feature| {
            let task = serde_json::from_value(serde_json::json!({
                "task": ":compileJava", "language": "java", "task_type": "org.gradle.api.tasks.compile.JavaCompile",
                "java_home": root.join(format!("jdk{feature}")), "arguments": [], "annotation_processor_path": [],
                "source_files": [&key], "classpath": [], "output_roots": [], "complete": true, "problem": null,
                "options": {
                    "release": null, "bootstrapClasspath": null, "fork": false, "forkJavaHome": null, "forkExecutable": null, "forkJvmArgs": [],
                    "sdkSourcePolicy": {
                        "adapter":"gradle-source-tree-stock-patterns-v1", "task":":compileJava", "language":"java",
                        "authored_roots":[project.join("src")], "future_roots":[project.join("src")], "generated_roots":[],
                    },
                    "sdkProducerClaims": {
                        "adapter":"gradle-producing-source-trees-v1", "task":":compileJava", "language":"java",
                        "roots":[project.join("src")], "complete":true,
                    },
                }
            })).unwrap();
            Arc::new(crate::classpath::ProjectSnapshot::new(vec![
                crate::classpath::ModuleClasspath {
                    module: ":".into(),
                    project_dir: project.clone(),
                    build: String::new(),
                    entries: Vec::new(),
                    unresolved: Vec::new(),
                    source_sets: vec![crate::classpath::SourceSetClasspath {
                        name: "main".into(),
                        source_roots: vec![project.join("src")],
                        compiler_tasks: vec![task],
                        ..Default::default()
                    }],
                },
            ]))
        };
        let batch = |feature| {
            let text = if feature == 21 {
                "package java.lang; public class Thread { public long threadId() { return 1; } public String getName() { return null; } }"
            } else {
                "package java.lang; public class Thread { public String getName() { return null; } }"
            };
            let path = root.join(format!("cache{feature}/Thread.java"));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            let mut parsers = LanguageParsers::new();
            let tree = parsers.parse(SourceLanguage::Java, text);
            crate::deps::FileSymbols {
                file: crate::paths::file_key(&path),
                symbols: language::file_facts(SourceLanguage::Java, &tree, text).symbols,
                jvm: None,
                library_source: None,
                origin: Some(crate::project_model::FileOrigin {
                    origin_id: crate::classpath::jdk_origin_id(
                        &root.join(format!("jdk{feature}/lib/src.zip")),
                    ),
                    representation: crate::project_model::SourceRepresentation::Source,
                    project_scope: None,
                }),
            }
        };
        let mut ws = Workspace::new();
        ws.scan(&project);
        ws.set_project_model(model(21));
        ws.open(&key, source.into());
        assert!(ws.index_dependency_batch(batch(21)));
        let offset = source.find("thread.th").unwrap() + "thread.th".len();
        assert!(
            completion_labels(&ws.completion_query(&key, offset).unwrap())
                .contains(&"threadId".into())
        );
        ws.set_project_model(model(17));
        assert!(
            ws.index
                .scoped_for_file(&key)
                .lookup_type("Thread")
                .is_empty(),
            "old SDK is retired before replacement, without host fallback"
        );
        assert!(
            !ws.index_dependency_batch(batch(21)),
            "a late foreign SDK batch cannot restore stale APIs"
        );
        assert!(ws.index_dependency_batch(batch(17)));
        assert!(
            !completion_labels(&ws.completion_query(&key, offset).unwrap())
                .contains(&"threadId".into())
        );
        let current = source.replace("thread.th", "thread.getName()");
        ws.open(&key, current.clone());
        let definitions = ws.goto_definition(&key, current.find("getName").unwrap());
        assert_eq!(definitions.len(), 1);
        assert_eq!(
            definitions[0].file,
            crate::paths::file_key(&root.join("cache17/Thread.java"))
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn runtime_ready_standalone_superclass_diagnostics_follow_unsaved_imports() {
        let root = unique_tmp("runtime-super-diagnostics");
        std::fs::create_dir_all(root.join("support")).unwrap();
        std::fs::create_dir_all(root.join("app")).unwrap();
        let java_child = "package app; import support.ImportedBase; public class ImportedChild extends ImportedBase {}";
        let kotlin_child =
            "package app\nimport support.ImportedBase\nclass KotlinImportedChild : ImportedBase()";
        let java_use = "package app; class SuperUse { void run(ImportedChild child, KotlinImportedChild kotlin) { child.accept(\"ok\"); kotlin.accept(\"ok\"); } }";
        let kotlin_use = "package app\nfun use(child: ImportedChild, kotlin: KotlinImportedChild) { child.accept(\"ok\"); kotlin.accept(\"ok\") }";
        for (path, text) in [
            (
                "support/ImportedBase.java",
                "package support; public class ImportedBase { public void accept(String value) {} }",
            ),
            (
                "app/ImportedBase.java",
                "package app; public class ImportedBase { public void accept(int value) {} }",
            ),
            ("app/ImportedChild.java", java_child),
            ("app/KotlinImportedChild.kt", kotlin_child),
            ("app/SuperUse.java", java_use),
            ("app/SuperUse.kt", kotlin_use),
        ] {
            std::fs::write(root.join(path), text).unwrap();
        }
        let mut ws = Workspace::new();
        ws.scan(&root);
        runtime_ready_source_diagnostic_fixture(&mut ws, &root);
        assert!(!ws.effective_completeness().compiler_producers_complete);
        for (path, text) in [
            ("app/SuperUse.java", java_use),
            ("app/SuperUse.kt", kotlin_use),
        ] {
            let key = crate::paths::file_key(&root.join(path));
            ws.open(&key, text.into());
            assert!(ws.diagnostics(&key).is_empty(), "valid {path}");
        }
        for (path, text) in [
            ("app/ImportedChild.java", java_child),
            ("app/KotlinImportedChild.kt", kotlin_child),
        ] {
            ws.open(
                &crate::paths::file_key(&root.join(path)),
                text.replace("support.ImportedBase", "app.ImportedBase"),
            );
        }
        for path in ["app/SuperUse.java", "app/SuperUse.kt"] {
            let diagnostics = ws.diagnostics(&crate::paths::file_key(&root.join(path)));
            assert_eq!(diagnostics.len(), 2, "{path}: {diagnostics:?}");
            assert!(
                diagnostics.iter().all(|diagnostic| diagnostic.code
                    == Some(crate::diagnostics::DiagnosticCode::CallShapeMismatch)),
                "{diagnostics:?}"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn runtime_ready_standalone_source_proofs_refuse_uncertain_owners() {
        let root = unique_tmp("runtime-source-proof-guards");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("Owners.java"),
            "package app; class Plain {} @Marker class Generated {} @interface Marker {}",
        )
        .unwrap();
        std::fs::write(root.join("Shapes.kt"), "package app\nclass KotlinPlain\nclass PropertyOwner(val value: Int)\ndata class DataOwner(val value: Int)").unwrap();
        let source = "package app; class Use { void run(Plain plain, Generated generated, KotlinPlain kotlin, PropertyOwner property, DataOwner data) { plain.absent(); generated.absent(); kotlin.absent(); property.absent(); data.absent(); } }";
        std::fs::write(root.join("Use.java"), source).unwrap();
        let kotlin_source = "package app\nfun use(plain: Plain, generated: Generated, kotlin: KotlinPlain, property: PropertyOwner, data: DataOwner) { plain.absent(); generated.absent(); kotlin.absent(); property.absent(); data.absent() }";
        std::fs::write(root.join("Use.kt"), kotlin_source).unwrap();
        let mut ws = Workspace::new();
        ws.scan(&root);
        runtime_ready_source_diagnostic_fixture(&mut ws, &root);
        let keys = [
            crate::paths::file_key(&root.join("Use.java")),
            crate::paths::file_key(&root.join("Use.kt")),
        ];
        for (key, text) in keys.iter().zip([source, kotlin_source]) {
            ws.open(key, text.into());
        }
        let check = |ws: &mut Workspace, count| {
            for key in &keys {
                let diagnostics = ws.diagnostics(key);
                assert_eq!(diagnostics.len(), count, "{key}: {diagnostics:?}");
                assert!(diagnostics.iter().all(|diagnostic| diagnostic.code
                    == Some(crate::diagnostics::DiagnosticCode::UnresolvedReference)));
            }
        };
        check(&mut ws, 2);
        ws.set_jdk_index_complete(false);
        check(&mut ws, 0);
        ws.set_jdk_index_complete(true);
        std::fs::write(root.join("build.gradle.kts"), "plugins { java }").unwrap();
        check(&mut ws, 0);
        std::fs::remove_file(root.join("build.gradle.kts")).unwrap();
        check(&mut ws, 2);
        let owners = crate::paths::file_key(&root.join("Owners.java"));
        ws.open(
            &owners,
            "package app; @Marker class Plain {} @Marker class Generated {} @interface Marker {}"
                .into(),
        );
        check(&mut ws, 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn runtime_ready_source_diagnostic_fixture(ws: &mut Workspace, root: &Path) {
        // Model the normal source-indexed SDK, without closing compiler producers.
        let home = root.join("sdk");
        std::fs::create_dir_all(home.join("jmods")).unwrap();
        std::fs::create_dir_all(home.join("lib")).unwrap();
        std::fs::write(
            home.join("jmods/java.base.jmod"),
            "explicit SDK inventory stamp",
        )
        .unwrap();
        let origin = crate::classpath::jdk_origin_id(&home.join("lib/src.zip"));
        for (key, text) in [
            (
                "sdk/java/lang/Object.java",
                "package java.lang; public class Object { public String toString() { return null; } }",
            ),
            (
                "sdk/java/lang/String.java",
                "package java.lang; public final class String {}",
            ),
        ] {
            let mut parsers = LanguageParsers::new();
            let tree = parsers.parse(SourceLanguage::Java, text);
            assert!(ws.index_dependency_batch(crate::deps::FileSymbols {
                file: key.into(),
                symbols: language::file_facts(SourceLanguage::Java, &tree, text).symbols,
                jvm: None,
                library_source: None,
                origin: Some(crate::project_model::FileOrigin {
                    origin_id: origin.clone(),
                    representation: crate::project_model::SourceRepresentation::Source,
                    project_scope: None,
                }),
            }));
            assert_eq!(ws.index.file_origin(key).unwrap().origin_id, origin);
            assert!(ws.global_origins.contains(&origin));
        }
        ws.class_inventory.get_mut().inject_object_for_tests(
            &home,
            ktcore::classfile::ClassFileFacts {
                identity: ktcore::jvm::ClassIdentity {
                    origin_id: origin,
                    internal_name: "java/lang/Object".into(),
                    input: ktcore::jvm::InputIdentity::Binary {
                        content_sha256: [1; 32],
                    },
                },
                major_version: 61,
                minor_version: 0,
                access_flags: 1,
                super_name: None,
                interfaces: Vec::new(),
                signature: None,
                source_file: None,
                attribute_names: Vec::new(),
                inner_classes: Vec::new(),
                fields: Vec::new(),
                methods: Vec::new(),
            },
        );
        ws.set_library_index_complete(true);
        ws.set_jdk_index_complete(true);
    }

    fn unique_tmp(prefix: &str) -> std::path::PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("ktlsp-{prefix}-{stamp}"))
    }

    fn index_durable_kotlin(ws: &mut Workspace, key: &str, src: &str) {
        let mut parsers = LanguageParsers::new();
        let tree = parsers.parse(SourceLanguage::Kotlin, src);
        let facts = language::file_facts(SourceLanguage::Kotlin, &tree, src);
        ws.index.replace_file(key, facts.symbols, Tier::Durable);
        ws.bump_index_revision();
    }

    fn completion_labels(query: &semantic_query::CompletionQuery) -> Vec<String> {
        query
            .candidates
            .iter()
            .map(|candidate| candidate.label.clone())
            .collect()
    }

    #[test]
    fn late_dependency_batches_preserve_open_java_and_kotlin_facts() {
        for (key, original, changed, empty) in [
            (
                "lib/Helper.java",
                "package lib;\npublic class Helper {\n public void clearOriginal() {}\n}\n",
                "package lib;\npublic class Helper {\n public void clearChanged() {}\n}\n",
                "package lib;\npublic class Helper {}\n",
            ),
            (
                "lib/Helper.kt",
                "package lib\nclass Helper {\n fun clearOriginal() {}\n}\n",
                "package lib\nclass Helper {\n fun clearChanged() {}\n}\n",
                "package lib\nclass Helper {}\n",
            ),
        ] {
            let language = SourceLanguage::for_key(key);
            let mut parsers = LanguageParsers::new();
            let facts = |parsers: &mut LanguageParsers, text: &str| {
                let tree = parsers.parse(language, text);
                language::symbol_facts(language, &tree, text).symbols
            };
            let mut ws = Workspace::new();
            ws.assume_index_complete_for_tests();
            assert!(ws.index_dependency_file(key, facts(&mut parsers, original)));
            ws.open(key, original.to_string());
            ws.change(key, changed.to_string());
            let main =
                "package app\nimport lib.Helper\nfun use(helper: Helper) {\n helper.clear\n}\n";
            let offset = main.find("helper.clear").unwrap() + "helper.clear".len();
            ws.open("Main.kt", main.to_string());

            let accepted = ws.index_dependency_file(key, facts(&mut parsers, original));
            ws.bump_index_revision();
            let labels = completion_labels(&ws.completion_query("Main.kt", offset).unwrap());
            assert_eq!(
                labels,
                vec!["clearChanged"],
                "late closed facts replaced {key}: {labels:?}"
            );
            assert!(!accepted, "open dependency batches should be skipped");
            assert_eq!(ws.doc_text(key).as_deref(), Some(changed));

            ws.change(key, empty.to_string());
            assert!(!ws.index_dependency_file(key, facts(&mut parsers, original)));
            ws.bump_index_revision();
            assert!(ws
                .completion_query("Main.kt", offset)
                .unwrap()
                .candidates
                .is_empty());

            let other = "package lib\nclass OtherClosed\n";
            let tree = parsers.parse(SourceLanguage::Kotlin, other);
            let symbols = language::symbol_facts(SourceLanguage::Kotlin, &tree, other).symbols;
            assert!(ws.index_dependency_file("lib/OtherClosed.kt", symbols));
            assert_eq!(ws.index.lookup_type("OtherClosed").len(), 1);

            ws.close(key);
            assert!(ws.index_dependency_file(key, facts(&mut parsers, original)));
            ws.bump_index_revision();
            assert_eq!(
                completion_labels(&ws.completion_query("Main.kt", offset).unwrap()),
                vec!["clearOriginal"]
            );
        }
    }

    #[test]
    fn git_source_discovery_handles_nested_and_canonical_roots() {
        let root = unique_tmp("git-source-discovery");
        std::fs::create_dir_all(&root).unwrap();
        assert!(Command::new("git")
            .args(["init", "-q"])
            .arg(&root)
            .status()
            .unwrap()
            .success());
        for relative in [
            "Root.kt",
            "nested/Helper.java",
            "nested/More.kt",
            "sibling/Sibling.java",
            "nested/build/Output.kt",
            "nested/.hidden/Hidden.kt",
            "nested/ignored/Ignored.kt",
            "build/generated/source/ksp/main/kotlin/Generated.kt",
        ] {
            let file = root.join(relative);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, "class Example {}\n").unwrap();
        }
        std::fs::write(root.join("nested/.gitignore"), "ignored/\n").unwrap();
        assert!(Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["add", "--", "Root.kt", "nested/Helper.java"])
            .status()
            .unwrap()
            .success());

        let expected = [
            "Root.kt",
            "nested/Helper.java",
            "nested/More.kt",
            "sibling/Sibling.java",
        ]
        .into_iter()
        .map(|path| root.join(path).canonicalize().unwrap())
        .collect::<BTreeSet<_>>();
        let actual = git_source_files(&root)
            .unwrap()
            .into_iter()
            .map(|path| path.canonicalize().unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(actual, expected);
        let mut expected_project = expected;
        expected_project.insert(
            root.join("build/generated/source/ksp/main/kotlin/Generated.kt")
                .canonicalize()
                .unwrap(),
        );
        for scan_root in [root.clone(), root.canonicalize().unwrap()] {
            let actual = project_source_files(&scan_root)
                .into_iter()
                .map(|path| path.canonicalize().unwrap())
                .collect::<BTreeSet<_>>();
            assert_eq!(actual, expected_project);
        }
        let nested = root.join("nested");
        let expected = ["Helper.java", "More.kt"]
            .into_iter()
            .map(|path| nested.join(path).canonicalize().unwrap())
            .collect::<BTreeSet<_>>();
        for scan_root in [nested.clone(), nested.canonicalize().unwrap()] {
            let actual = git_source_files(&scan_root)
                .unwrap()
                .into_iter()
                .map(|path| path.canonicalize().unwrap())
                .collect::<BTreeSet<_>>();
            assert_eq!(actual, expected);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn git_scan_keeps_uri_open_buffers_authoritative() {
        let root = unique_tmp("git-open-buffer");
        std::fs::create_dir_all(&root).unwrap();
        assert!(Command::new("git")
            .args(["init", "-q"])
            .arg(&root)
            .status()
            .unwrap()
            .success());
        let file = root.join("Main.kt");
        std::fs::write(&file, "class DiskState\n").unwrap();
        let generated = root.join("build/generated/source/ksp/main/kotlin/Generated.kt");
        std::fs::create_dir_all(generated.parent().unwrap()).unwrap();
        std::fs::write(&generated, "class GeneratedDiskState\n").unwrap();
        let native_key = file.to_string_lossy().to_string();
        let uri_key = crate::paths::file_key(&file);
        for key in [native_key, uri_key] {
            let mut ws = Workspace::new();
            ws.open(key.clone(), "class BufferState\n".to_string());
            ws.open(
                crate::paths::file_key(&generated),
                "class GeneratedBufferState\n".to_string(),
            );
            assert_eq!(ws.scan(&root), 0);
            assert_eq!(ws.scan(&root.canonicalize().unwrap()), 0);
            assert!(ws.index.lookup_by_name("DiskState").is_empty());
            assert!(ws.index.lookup_by_name("GeneratedDiskState").is_empty());
            assert_eq!(ws.index.lookup_by_name("BufferState").len(), 1);
            assert_eq!(ws.index.lookup_by_name("GeneratedBufferState").len(), 1);
            assert_eq!(ws.doc_text(&key).as_deref(), Some("class BufferState\n"));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn gradle_live_project_completion_updates_after_model_sources_arrive() {
        let root = unique_tmp("gradle-live-completion");
        let source = root.join("src/main/kotlin/com/example/fixture");
        std::fs::create_dir_all(&source).unwrap();
        let probe = include_str!(
            "../../../dev/gradle-sample/src/main/kotlin/com/example/fixture/_Probe.kt"
        );
        for (name, text) in [
            ("_Probe.kt", probe),
            (
                "Greetings.kt",
                include_str!(
                    "../../../dev/gradle-sample/src/main/kotlin/com/example/fixture/Greetings.kt"
                ),
            ),
            (
                "Model.kt",
                include_str!(
                    "../../../dev/gradle-sample/src/main/kotlin/com/example/fixture/Model.kt"
                ),
            ),
        ] {
            std::fs::write(source.join(name), text).unwrap();
        }
        let key = crate::paths::file_key(&source.join("_Probe.kt"));
        let cases = [
            ("    g.gr", "greet"),
            ("    greeterFor(\"en\").gr", "greet"),
            ("    BasicGreeter.def", "default"),
            ("    BasicGreeter.default().sal", "salutation"),
        ];
        let mut ws = Workspace::new();
        ws.open(key.clone(), probe.to_string());
        let generation = ws.begin_analysis_generation();
        for (needle, _) in cases {
            let offset = probe.find(needle).unwrap() + needle.len();
            assert!(ws.complete(&key, offset, true).is_none(), "{needle}");
        }
        let model = Arc::new(crate::classpath::ProjectSnapshot::new(
            crate::classpath::parse_dump(&format!(
                "PROJECT\t:app\t{}\nSOURCE\tmain\t{}\nEND\t:app\n",
                root.display(),
                root.join("src/main/kotlin").display()
            )),
        ));
        let update = prepare_model_sources(&model, &ws.source_stamps());
        assert_eq!(
            ws.install_project_update(generation, model, update),
            Some(2)
        );
        for (needle, expected) in cases {
            let offset = probe.find(needle).unwrap() + needle.len();
            let result = ws.complete(&key, offset, true).expect(needle);
            assert!(
                result.items.iter().any(|item| item.label == expected),
                "{needle}: expected {expected}, got {result:?}"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn model_refresh_updates_changed_closed_sources_and_preserves_dirty_buffers() {
        let root = unique_tmp("model-refresh-sources");
        let source = root.join("custom-generated");
        std::fs::create_dir_all(&source).unwrap();
        let java = source.join("Generated.java");
        let kotlin = source.join("Generated.kt");
        std::fs::write(&java, "class BeforeGenerated { Before reference; }").unwrap();
        std::fs::write(&kotlin, "class KotlinDisk\n").unwrap();
        let model = Arc::new(crate::classpath::ProjectSnapshot::new(
            crate::classpath::parse_dump(&format!(
                "PROJECT\t:app\t{}\nSOURCE\tmain\t{}\nEND\t:app\n",
                root.display(),
                source.display()
            )),
        ));
        let mut ws = Workspace::new();
        ws.set_project_model(model.clone());
        assert_eq!(ws.scan_model_sources(), 2);
        assert_eq!(
            ws.scan_model_sources(),
            0,
            "unchanged files must not be reparsed"
        );
        let java_key = crate::paths::file_key(&java);
        let kotlin_key = crate::paths::file_key(&kotlin);
        assert!(ws
            .index
            .lookup_refs("Before")
            .iter()
            .any(|entry| entry.path.as_ref() == java_key));
        std::fs::write(&java, "class AfterGeneratedUpdate { After reference; }").unwrap();
        std::fs::write(&kotlin, "class KotlinDiskChanged\n").unwrap();
        let update = prepare_model_sources(&model, &ws.source_stamps());
        assert_eq!(update.batches.len(), 2);
        // Opening a file after staging must still win over the background disk facts.
        ws.open(kotlin_key.clone(), "class KotlinDirty\n".to_string());
        let generation = ws.begin_analysis_generation();
        assert_eq!(
            ws.install_project_update(generation, model, update),
            Some(1)
        );
        assert!(ws.index.lookup_type("BeforeGenerated").is_empty());
        assert_eq!(ws.index.lookup_type("AfterGeneratedUpdate").len(), 1);
        assert!(!ws
            .index
            .lookup_refs("Before")
            .iter()
            .any(|entry| entry.path.as_ref() == java_key));
        assert_eq!(ws.index.lookup_type("KotlinDirty").len(), 1);
        assert!(ws.index.lookup_type("KotlinDiskChanged").is_empty());
        assert_eq!(ws.scan_model_sources(), 0);
        std::fs::remove_file(&java).unwrap();
        assert_eq!(ws.scan_model_sources(), 0);
        assert!(ws.index.lookup_type("AfterGeneratedUpdate").is_empty());
        assert!(!ws
            .index
            .lookup_refs("After")
            .iter()
            .any(|entry| entry.path.as_ref() == java_key));
        ws.close(&kotlin_key);
        assert!(ws.index.lookup_type("KotlinDirty").is_empty());
        assert_eq!(ws.index.lookup_type("KotlinDiskChanged").len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_enrichment_preserves_unchanged_binary_completion_until_sources_replace_it() {
        use crate::project_model::{FileOrigin, SourceRepresentation};
        use std::io::Write;
        let root = unique_tmp("source-enrichment-continuity");
        std::fs::create_dir_all(root.join("src")).unwrap();
        let binary = root.join("api.jar");
        let mut archive = zip::ZipWriter::new(std::fs::File::create(&binary).unwrap());
        archive
            .start_file(
                "fieldfixture/FieldOnly.class",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive
            .write_all(include_bytes!(
                "../../ktcore/tests/fixtures/classfile/fieldfixture/FieldOnly.class"
            ))
            .unwrap();
        archive.finish().unwrap();
        let source_jar = root.join("api-sources.jar");
        let pom = root.join("api.pom");
        std::fs::write(&source_jar, b"source revision").unwrap();
        std::fs::write(&pom, b"encoding revision").unwrap();
        let model = |extra: &str| {
            Arc::new(crate::classpath::ProjectSnapshot::new(
                crate::classpath::parse_dump(&format!(
                    "PROJECT\t:app\t{}\nSOURCESET\tmain\nSOURCE\tmain\t{}\nSCP\tmain\t{}\nARTIFACT\tmain\t{}\tMODULE\tfieldfixture\tapi\t1\n{extra}END\t:app\n",
                    root.display(),
                    root.join("src").display(),
                    binary.display(),
                    binary.display()
                )),
            ))
        };
        let accepted = model("");
        let coord = crate::coords::Coordinate::parse("fieldfixture:api:1").unwrap();
        let origin = crate::classpath::maven_origin_id(&coord);
        let mut ws = Workspace::new();
        ws.set_project_model(accepted.clone());
        let mut kotlin = crate::parser::KotlinParser::new();
        let mut java = crate::java::JavaParser::new();
        for batch in crate::deps::resolve_module_binary_stubs(
            &coord,
            &accepted.coordinate_binaries(&coord).unwrap(),
            &root.join("extract"),
            &mut kotlin,
            &mut java,
        )
        .unwrap()
        {
            ws.index_dependency_batch(batch);
        }
        let mut parsers = LanguageParsers::new();
        let source_batch = |parsers: &mut LanguageParsers, path: &Path, text: &str| {
            let key = crate::paths::file_key(path);
            let tree = parsers.parse_for_key(&key, text);
            crate::deps::FileSymbols {
                jvm: None,
                file: key,
                symbols: language::file_facts(SourceLanguage::Java, &tree, text).symbols,
                library_source: None,
                origin: Some(FileOrigin {
                    origin_id: origin.clone(),
                    representation: SourceRepresentation::Source,
                    project_scope: None,
                }),
            }
        };
        ws.index_dependency_batch(source_batch(
            &mut parsers,
            &root.join("Obsolete.java"),
            "package fieldfixture; public class Obsolete {}",
        ));
        ws.complete_dependency_origin(origin.clone());
        let useful = |ws: &mut Workspace| {
            for (name, text) in [
                (
                    "Use.java",
                    "package fieldfixture; class Use { void f(FieldOnly value) { value.co; } }",
                ),
                (
                    "Use.kt",
                    "package fieldfixture\nfun f(value: FieldOnly) { value.co }\n",
                ),
            ] {
                let key = crate::paths::file_key(&root.join("src").join(name));
                ws.open(key.clone(), text.to_string());
                let query = ws
                    .completion_query(&key, text.find("value.co").unwrap() + "value.co".len())
                    .unwrap();
                assert_eq!(
                    query
                        .candidates
                        .iter()
                        .filter(|item| item.label == "count")
                        .count(),
                    1,
                    "{name}: {} {:?}",
                    query.status_label(),
                    query.reason_labels()
                );
            }
        };
        useful(&mut ws);
        ws.set_project_model(model(&format!(
            "SOURCEQUERY\tmain\tcomplete\nSOURCEARTIFACT\tmain\t{}\tMODULE\tfieldfixture\tapi\t1\nPOMQUERY\tmain\tcomplete\nPOMARTIFACT\tmain\t{}\tMODULE\tfieldfixture\tapi\t1\n",
            source_jar.display(), pom.display())));
        assert!(
            ws.index.lookup_type("Obsolete").is_empty(),
            "old source facts must retire"
        );
        assert!(!ws.dependency_origin_complete(&origin));
        useful(&mut ws); // Accepted enrichment has published, but source parsing has not finished.
        let source = root.join("FieldOnly.java");
        let text = include_str!(
            "../../ktcore/tests/fixtures/classfile/sources/fieldfixture/FieldOnly.java"
        );
        std::fs::write(&source, text).unwrap();
        ws.index_dependency_batch(source_batch(&mut parsers, &source, text));
        ws.bump_index_revision();
        useful(&mut ws); // Existing canonical source preference must avoid duplicate ambiguity.
        let navigation_key = crate::paths::file_key(&root.join("src/Navigate.java"));
        let navigation_text = "package fieldfixture; class Navigate { int f(FieldOnly value) { return value.count; } }";
        ws.open(navigation_key.clone(), navigation_text.to_string());
        let navigation_offset = navigation_text.find("count").unwrap();
        let source_key = crate::paths::file_key(&source);
        assert!(ws
            .goto_definition(&navigation_key, navigation_offset)
            .iter()
            .any(|definition| definition.file == source_key));
        std::fs::write(&pom, b"changed selected encoding metadata").unwrap();
        ws.set_project_model(model(&format!(
            "SOURCEQUERY\tmain\tcomplete\nSOURCEARTIFACT\tmain\t{}\tMODULE\tfieldfixture\tapi\t1\nPOMQUERY\tmain\tcomplete\nPOMARTIFACT\tmain\t{}\tMODULE\tfieldfixture\tapi\t1\n",
            source_jar.display(), pom.display())));
        assert!(
            ws.goto_definition(&navigation_key, navigation_offset)
                .iter()
                .all(|definition| definition.file != source_key),
            "an invalidated source declaration must retire"
        );
        useful(&mut ws); // Metadata invalidation cannot hide the unchanged selected binary.
        std::fs::write(&binary, b"different selected binary").unwrap();
        ws.set_project_model(model(""));
        assert!(
            ws.index.lookup_type("FieldOnly").is_empty(),
            "changed binaries must retire their stubs too"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn model_refresh_removes_inactive_libraries_and_rejects_stale_batches_and_readiness() {
        use crate::project_model::{FileOrigin, SourceRepresentation};
        let root = unique_tmp("model-refresh-library");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("api.jar"), b"first binary").unwrap();
        let model = |version| {
            Arc::new(crate::classpath::ProjectSnapshot::new(
                crate::classpath::parse_dump(&format!(
                    "PROJECT\t:app\t{}\nSOURCE\tmain\t{}\nSCP\tmain\t{}\nARTIFACT\tmain\t{}\tMODULE\tvendor\tapi\t{version}\nEND\t:app\n",
                    root.display(),
                    root.join("src").display(),
                    root.join("api.jar").display(),
                    root.join("api.jar").display()
                )),
            ))
        };
        let mut ws = Workspace::new();
        ws.set_project_model(model("1"));
        let old = ws.begin_analysis_generation();
        let mut parsers = LanguageParsers::new();
        let batch = |parsers: &mut LanguageParsers, key: &str, text: &str| {
            let tree = parsers.parse_for_key(key, text);
            crate::deps::FileSymbols {
                jvm: None,
                file: key.to_string(),
                symbols: language::file_facts(SourceLanguage::for_key(key), &tree, text).symbols,
                library_source: None,
                origin: Some(FileOrigin {
                    origin_id: "maven:vendor:api:1".to_string(),
                    representation: SourceRepresentation::Source,
                    project_scope: None,
                }),
            }
        };
        let closed = crate::paths::file_key(&root.join("cache/Closed.kt"));
        let opened = crate::paths::file_key(&root.join("cache/Open.kt"));
        std::fs::create_dir_all(root.join("cache")).unwrap();
        std::fs::write(&opened, "package api\nclass OpenDisk\n").unwrap();
        assert!(ws.index_dependency_batch_for_generation(
            old,
            batch(&mut parsers, &closed, "package api\nclass ClosedOld\n")
        ));
        assert!(ws.index_dependency_batch_for_generation(
            old,
            batch(&mut parsers, &opened, "package api\nclass OpenDisk\n")
        ));
        ws.open(opened.clone(), "package api\nclass OpenDirty\n".to_string());
        ws.complete_dependency_origin("maven:vendor:api:1".to_string());
        std::fs::write(
            root.join("api.jar"),
            b"updated binary with the same coordinate",
        )
        .unwrap();
        ws.set_project_model(model("1"));
        assert!(
            ws.index.lookup_type("ClosedOld").is_empty(),
            "changed binary kept its old source projection"
        );
        assert!(!ws.dependency_origin_complete("maven:vendor:api:1"));
        assert_eq!(ws.index.lookup_type("OpenDirty").len(), 1);
        assert!(ws.index_dependency_batch_for_generation(
            old,
            batch(&mut parsers, &closed, "package api\nclass ClosedOld\n")
        ));
        ws.complete_dependency_origin("maven:vendor:api:1".to_string());
        let next = model("2");
        let staged = prepare_model_sources(&next, &ws.source_stamps());
        let generation = ws.begin_analysis_generation();
        assert!(ws
            .install_project_update(old, next.clone(), staged)
            .is_none());
        assert!(!ws.finish_analysis_generation(old, true, true));
        assert!(!ws.completeness.library_index_complete);
        assert!(
            ws.index.lookup_type("ClosedOld").len() == 1,
            "a rejected model changed the index"
        );
        let staged = prepare_model_sources(&next, &ws.source_stamps());
        assert_eq!(ws.install_project_update(generation, next, staged), Some(0));
        assert!(ws.index.lookup_type("ClosedOld").is_empty());
        assert!(ws.index.file_origin(&closed).is_none());
        assert_eq!(
            ws.doc_text(&opened).as_deref(),
            Some("package api\nclass OpenDirty\n")
        );
        assert_eq!(ws.index.lookup_type("OpenDirty").len(), 1);
        assert!(!ws.dependency_origin_complete("maven:vendor:api:1"));
        assert!(!ws.index_dependency_batch_for_generation(
            old,
            batch(&mut parsers, &closed, "package api\nclass ClosedOld\n")
        ));
        assert!(
            !ws.index_dependency_batch_for_generation(
                generation,
                batch(&mut parsers, &closed, "package api\nclass ClosedOld\n")
            ),
            "a same-generation lazy result used an inactive snapshot"
        );
        ws.close(&opened);
        assert!(ws.index.lookup_type("OpenDirty").is_empty());
        assert!(
            ws.index.lookup_type("OpenDisk").is_empty(),
            "closing an inactive library resurrected old disk facts"
        );
        assert!(ws.finish_analysis_generation(generation, true, false));
        assert!(ws.completeness.library_index_complete);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn authoritative_model_adopts_only_registered_sources_and_preserves_open_and_library_facts() {
        use crate::project_model::{FileOrigin, SourceRepresentation};
        let root = unique_tmp("model-source-ownership");
        std::fs::create_dir_all(&root).unwrap();
        assert!(Command::new("git")
            .args(["init", "-q"])
            .arg(&root)
            .status()
            .unwrap()
            .success());
        for (relative, text) in [
            (
                "modules/editing/src/main/java/app/Main.java",
                "package app; class DiskMain { Helper helper; }",
            ),
            (
                "modules/editing/src/main/java/app/Helper.java",
                "package app; class Helper {}",
            ),
            (
                "build/custom-java/app/Generated.java",
                "package app; class Generated { Helper helper; }",
            ),
            (
                "testData/private/Private.java",
                "package app; class PrivateFixture {}",
            ),
            (
                "testData/private/Closed.java",
                "package app; class ClosedFixture { Helper helper; }",
            ),
            (
                "testData/extracted/Library.java",
                "package library; class Library {}",
            ),
        ] {
            let file = root.join(relative);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
        assert!(Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["add", "--", "modules", "testData"])
            .status()
            .unwrap()
            .success());
        let main =
            crate::paths::file_key(&root.join("modules/editing/src/main/java/app/Main.java"));
        let private = crate::paths::file_key(&root.join("testData/private/Private.java"));
        let library = crate::paths::file_key(&root.join("testData/extracted/Library.java"));
        let generated = root.join("build/custom-java/app/Generated.java");
        let generated_key = crate::paths::file_key(&generated);
        let mut ws = Workspace::new();
        ws.scan(&root);
        assert_eq!(ws.index.lookup_type("PrivateFixture").len(), 1);
        assert_eq!(ws.index.lookup_type("ClosedFixture").len(), 1);
        ws.open(
            main.clone(),
            "package app; class BufferMain { Helper helper; }".to_string(),
        );
        ws.open(
            private.clone(),
            "package app; class PrivateBuffer { Helper helper; }".to_string(),
        );
        ws.index.set_file_origin(
            &library,
            FileOrigin {
                origin_id: "maven:vendor:api:1".to_string(),
                representation: SourceRepresentation::Source,
                project_scope: None,
            },
        );
        let model = Arc::new(crate::classpath::ProjectSnapshot::new(
            crate::classpath::parse_dump(&format!(
                "PROJECT\t:app\t{}\nSOURCE\tmain\t{}\nSOURCE\tmain\t{}\nSCP\tmain\t{}\nARTIFACT\tmain\t{}\tMODULE\tvendor\tapi\t1\nEND\t:app\n",
                root.join("modules/editing").display(),
                root.join("modules/editing/src/main/java").display(),
                root.join("build/custom-java").display(),
                root.join("api.jar").display(),
                root.join("api.jar").display(),
            )),
        ));
        ws.set_project_model(model);
        assert_eq!(ws.scan_model_sources(), 1);
        assert!(ws.completeness.project_scan_complete);
        assert!(ws.index.lookup_type("DiskMain").is_empty());
        assert_eq!(ws.index.lookup_type("BufferMain").len(), 1);
        assert_eq!(ws.index.lookup_type("PrivateBuffer").len(), 1);
        assert_eq!(ws.index.lookup_type("Generated").len(), 1);
        assert_eq!(ws.index.lookup_type("Library").len(), 1);
        assert!(ws.index.lookup_type("ClosedFixture").is_empty());
        assert!(ws.index.file_origin(&private).is_none());
        assert!(!ws.project_file_meta.contains_key(&private));
        assert!(!ws.project_file_meta.contains_key(&library));
        let private_text = ws.doc_text(&private).unwrap();
        assert!(ws
            .goto_definition(&private, private_text.find("Helper").unwrap())
            .is_empty());
        assert!(ws
            .index
            .lookup_refs("Helper")
            .iter()
            .any(|entry| entry.path.as_ref() == generated_key));
        std::fs::remove_file(&generated).unwrap();
        assert_eq!(ws.scan_model_sources(), 0);
        assert!(ws.index.lookup_type("Generated").is_empty());
        assert!(!ws
            .index
            .lookup_refs("Helper")
            .iter()
            .any(|entry| entry.path.as_ref() == generated_key));
        assert!(ws.index.file_origin(&generated_key).is_none());
        assert!(ws.completeness.project_scan_complete);
        ws.close(&private);
        assert!(ws.index.lookup_type("PrivateBuffer").is_empty());
        assert!(ws.index.lookup_type("PrivateFixture").is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn removed_source_root_keeps_dirty_buffer_but_drops_its_scope() {
        let root = unique_tmp("model-removed-root");
        let source_root = root.join("custom");
        std::fs::create_dir_all(&source_root).unwrap();
        let file = source_root.join("Generated.java");
        std::fs::write(&file, "class DiskGenerated {}").unwrap();
        let key = crate::paths::file_key(&file);
        let model = |source: &Path| {
            Arc::new(crate::classpath::ProjectSnapshot::new(
                crate::classpath::parse_dump(&format!(
                    "PROJECT\t:app\t{}\nSOURCE\tmain\t{}\nEND\t:app\n",
                    root.display(),
                    source.display(),
                )),
            ))
        };
        let mut ws = Workspace::new();
        ws.set_project_model(model(&source_root));
        assert_eq!(ws.scan_model_sources(), 1);
        ws.open(key.clone(), "class DirtyGenerated {}".to_string());
        assert!(ws.index.file_origin(&key).is_some());
        ws.set_project_model(model(&root.join("src/main/java")));
        ws.scan_model_sources();
        assert!(ws.index.file_origin(&key).is_none());
        assert_eq!(
            ws.doc_text(&key).as_deref(),
            Some("class DirtyGenerated {}")
        );
        assert_eq!(ws.index.lookup_type("DirtyGenerated").len(), 1);
        assert!(!ws.project_file_meta.contains_key(&key));
        assert!(ws.completeness.project_scan_complete);
        ws.close(&key);
        assert!(ws.index.lookup_type("DirtyGenerated").is_empty());
        assert!(ws.index.lookup_type("DiskGenerated").is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pending_project_refresh_withdraws_source_closure_until_real_rescan() {
        let root = unique_tmp("pending-source-namespace");
        std::fs::create_dir_all(&root).unwrap();
        let caller_path = root.join("Caller.java");
        let owner_path = root.join("Owner.java");
        let caller_text =
            "package app; import api.Owner; class Caller { int read() { return Owner.value; } }";
        std::fs::write(&caller_path, caller_text).unwrap();
        std::fs::write(
            &owner_path,
            "package api; public class Owner { public static int value; }",
        )
        .unwrap();
        let caller = crate::paths::file_key(&caller_path);
        let owner = crate::paths::file_key(&owner_path);
        let mut ws = Workspace::new();
        assert_eq!(ws.scan(&root), 2);
        let overlay = caller_text.replace("read()", "currentRead()");
        ws.open(&caller, overlay.clone());
        let proof = |ws: &Workspace| {
            ws.with_java_member_proofs(&caller, |index| {
                let caller_owner = index
                    .lookup_type("Caller")
                    .into_iter()
                    .find(|entry| entry.path.as_ref() == caller.as_str())
                    .unwrap();
                let target_owner = index
                    .lookup_type("Owner")
                    .into_iter()
                    .find(|entry| entry.path.as_ref() == owner.as_str())
                    .unwrap();
                (
                    index.scope_is_complete(),
                    index.source_members_complete(caller_owner),
                    index.source_members_complete(target_owner),
                )
            })
        };
        assert_eq!(
            proof(&ws),
            (true, true, true),
            "qualify the real scanned source namespace first"
        );
        let previous = ws.analysis_generation();
        let generation = ws.begin_analysis_generation();
        assert!(!ws.is_current_generation(previous));
        assert!(ws.is_current_generation(generation));
        assert!(ws.project_refresh_pending);
        assert!(ws.project_scan_initialized);
        assert_eq!(
            ws.project_file_meta.len(),
            2,
            "pending work must preserve source ownership"
        );
        assert_eq!(proof(&ws), (true, false, false),
            "scope visibility remains known, but the pending producer cannot certify source closure");
        assert!(!ws.completeness.project_scan_complete);
        let offset = overlay.find("Owner.value").unwrap() + "Owner.".len();
        let _ = ws.completion_query(&caller, offset);
        let _ = ws.goto_definition(&caller, offset);
        assert!(
            ws.project_refresh_pending,
            "a query must not claim the pending scan finished"
        );
        assert_eq!(
            proof(&ws),
            (true, false, false),
            "queries cannot restore unproduced proof"
        );
        assert!(!ws.completeness.project_scan_complete);
        assert_eq!(
            ws.scan(&root),
            1,
            "only the closed owner is read from disk again"
        );
        assert!(!ws.project_refresh_pending);
        assert!(ws.completeness.project_scan_complete);
        assert_eq!(
            proof(&ws),
            (true, true, true),
            "real publication restores closure"
        );
        assert_eq!(ws.doc_text(&caller).as_deref(), Some(overlay.as_str()));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unsupported_gradle_source_model_stays_unknown_instead_of_using_global_facts() {
        let root = unique_tmp("model-unsupported-sources");
        std::fs::create_dir_all(&root).unwrap();
        let main = root.join("Main.java");
        let text = "package app;\nimport api.Helper;\nclass Main { Helper helper; }\n";
        std::fs::write(&main, text).unwrap();
        std::fs::write(
            root.join("Helper.java"),
            "package api;\npublic class Helper {}\n",
        )
        .unwrap();
        let key = crate::paths::file_key(&main);
        let mut ws = Workspace::new();
        ws.scan(&root);
        assert_eq!(
            ws.goto_definition(&key, text.find("Helper").unwrap()).len(),
            1
        );
        ws.set_project_model(Arc::new(crate::classpath::ProjectSnapshot::new(
            crate::classpath::parse_dump(&format!(
                "PROJECT\t:app\t{}\nEND\t:app\n",
                root.display(),
            )),
        )));
        assert!(ws
            .goto_definition(&key, text.find("Helper").unwrap())
            .is_empty());
        assert!(!ws.completeness.project_scan_complete);
        assert_eq!(ws.scan_model_sources(), 0);
        assert!(!ws.completeness.project_scan_complete);
        assert_eq!(ws.scan(&root), 0);
        assert!(ws.index.lookup_type("Helper").is_empty());
        assert!(!ws.completeness.project_scan_complete);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scan_records_open_java_package_with_java_parser() {
        let root = unique_tmp("open-java-package");
        std::fs::create_dir_all(&root).unwrap();
        let java_file = root.join("src/main/java/app/Open.java");
        let key = java_file.to_string_lossy().to_string();

        let mut ws = Workspace::new();
        ws.open(
            key.clone(),
            "package app;\npublic class Open {}\n".to_string(),
        );
        ws.scan(&root);

        let meta = ws
            .project_file_meta
            .get(&key)
            .expect("open Java buffer should be tracked after scan");
        assert_eq!(meta.package, "app");
        assert!(meta.clean);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn closed_after_dot_completion_uses_salsa_inference() {
        let root = unique_tmp("closed-salsa-completion");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("Main.kt");
        let key = file.to_string_lossy().to_string();
        let src = "class Box {\n    fun unbox() {}\n}\nfun main(box: Box) { box. }\n";
        std::fs::write(&file, src).unwrap();

        let mut ws = Workspace::new();
        ws.reindex(&key, src);

        let offset = src.find("box. }").unwrap() + "box.".len();
        let query = ws
            .completion_query(&key, offset)
            .expect("closed after-dot completion");

        assert_eq!(query.context, complete::CompletionContext::AfterDot);
        assert!(
            query
                .candidates
                .iter()
                .any(|candidate| candidate.label == "unbox"),
            "status={} reasons={:?} candidates={:?} members={:?}",
            query.status_label(),
            query.reason_labels(),
            query
                .candidates
                .iter()
                .map(|candidate| &candidate.label)
                .collect::<Vec<_>>(),
            ws.index
                .members_of("Box")
                .iter()
                .map(|entry| entry.sym.name.as_str())
                .collect::<Vec<_>>()
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn closed_after_dot_completion_uses_compositional_salsa_chain_inference() {
        let root = unique_tmp("closed-salsa-chain-completion");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("Main.kt");
        let key = file.to_string_lossy().to_string();
        let src = "class Leaf {\n    fun target() {}\n}\nclass Mid {\n    fun leaf(): Leaf = Leaf()\n}\nclass Root {\n    val mid: Mid = Mid()\n}\nfun main(root: Root) { root.mid.leaf(). }\n";
        std::fs::write(&file, src).unwrap();

        let mut ws = Workspace::new();
        ws.reindex(&key, src);

        let offset = src.find("leaf(). }").unwrap() + "leaf().".len();
        let query = ws
            .completion_query(&key, offset)
            .expect("closed chained after-dot completion");

        assert_eq!(query.context, complete::CompletionContext::AfterDot);
        assert!(
            query
                .candidates
                .iter()
                .any(|candidate| candidate.label == "target"),
            "status={} reasons={:?} candidates={:?}",
            query.status_label(),
            query.reason_labels(),
            query
                .candidates
                .iter()
                .map(|candidate| &candidate.label)
                .collect::<Vec<_>>()
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn salsa_inferred_completion_invalidates_after_reindex() {
        let root = unique_tmp("closed-salsa-invalidation");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("Main.kt");
        let key = file.to_string_lossy().to_string();
        let first = "class Box {\n    fun before() {}\n}\nfun main(box: Box) { box. }\n";
        std::fs::write(&file, first).unwrap();

        let mut ws = Workspace::new();
        ws.reindex(&key, first);
        let offset = first.find("box. }").unwrap() + "box.".len();

        let first_query = ws
            .completion_query(&key, offset)
            .expect("first closed after-dot completion");
        assert!(
            first_query
                .candidates
                .iter()
                .any(|candidate| candidate.label == "before"),
            "status={} reasons={:?} candidates={:?}",
            first_query.status_label(),
            first_query.reason_labels(),
            first_query
                .candidates
                .iter()
                .map(|candidate| &candidate.label)
                .collect::<Vec<_>>()
        );

        let second = "class Box {\n    fun after() {}\n}\nfun main(box: Box) { box. }\n";
        std::fs::write(&file, second).unwrap();
        ws.reindex(&key, second);

        let second_query = ws
            .completion_query(&key, offset)
            .expect("second closed after-dot completion");
        assert!(second_query
            .candidates
            .iter()
            .any(|candidate| candidate.label == "after"));
        assert!(!second_query
            .candidates
            .iter()
            .any(|candidate| candidate.label == "before"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn salsa_inferred_completion_invalidates_after_durable_index_update() {
        let root = unique_tmp("closed-salsa-durable-invalidation");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("Main.kt");
        let key = file.to_string_lossy().to_string();
        let src = "package app\nimport lib.Box\nfun main(box: Box) { box. }\n";
        std::fs::write(&file, src).unwrap();

        let mut ws = Workspace::new();
        ws.reindex(&key, src);
        index_durable_kotlin(
            &mut ws,
            "lib://Box.kt",
            "package lib\nclass Box {\n    fun before() {}\n}\n",
        );

        let offset = src.find("box. }").unwrap() + "box.".len();
        let first_query = ws
            .completion_query(&key, offset)
            .expect("first closed after-dot completion");
        let first_labels = completion_labels(&first_query);
        assert!(first_labels.iter().any(|label| label == "before"));
        assert!(!first_labels.iter().any(|label| label == "after"));

        index_durable_kotlin(
            &mut ws,
            "lib://Box.kt",
            "package lib\nclass Box {\n    fun after() {}\n}\n",
        );

        let second_query = ws
            .completion_query(&key, offset)
            .expect("second closed after-dot completion");
        let second_labels = completion_labels(&second_query);
        assert!(second_labels.iter().any(|label| label == "after"));
        assert!(!second_labels.iter().any(|label| label == "before"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn compositional_salsa_inference_invalidates_when_receiver_annotation_changes() {
        let root = unique_tmp("closed-salsa-receiver-annotation-invalidation");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("Main.kt");
        let key = file.to_string_lossy().to_string();
        let first = "class First {\n    fun onlyFirst() {}\n}\nclass Second {\n    fun onlySecond() {}\n}\nfun main() { val receiver: First = First(); receiver. }\n";
        std::fs::write(&file, first).unwrap();

        let mut ws = Workspace::new();
        ws.reindex(&key, first);
        let first_offset = first.find("receiver. }").unwrap() + "receiver.".len();

        let first_query = ws
            .completion_query(&key, first_offset)
            .expect("first receiver completion");
        assert!(first_query
            .candidates
            .iter()
            .any(|candidate| candidate.label == "onlyFirst"));
        assert!(!first_query
            .candidates
            .iter()
            .any(|candidate| candidate.label == "onlySecond"));

        let second = "class First {\n    fun onlyFirst() {}\n}\nclass Second {\n    fun onlySecond() {}\n}\nfun main() { val receiver: Second = Second(); receiver. }\n";
        std::fs::write(&file, second).unwrap();
        ws.reindex(&key, second);
        let second_offset = second.find("receiver. }").unwrap() + "receiver.".len();

        let second_query = ws
            .completion_query(&key, second_offset)
            .expect("second receiver completion");
        assert!(second_query
            .candidates
            .iter()
            .any(|candidate| candidate.label == "onlySecond"));
        assert!(!second_query
            .candidates
            .iter()
            .any(|candidate| candidate.label == "onlyFirst"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn compositional_salsa_inference_invalidates_when_function_return_type_changes() {
        let root = unique_tmp("closed-salsa-function-return-invalidation");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("Main.kt");
        let key = file.to_string_lossy().to_string();
        let first = "class First {\n    fun onlyFirst() {}\n}\nclass Second {\n    fun onlySecond() {}\n}\nfun make(): First = First()\nfun main() { make(). }\n";
        std::fs::write(&file, first).unwrap();

        let mut ws = Workspace::new();
        ws.reindex(&key, first);
        let first_offset = first.find("make(). }").unwrap() + "make().".len();

        let first_query = ws
            .completion_query(&key, first_offset)
            .expect("first return completion");
        assert!(first_query
            .candidates
            .iter()
            .any(|candidate| candidate.label == "onlyFirst"));
        assert!(!first_query
            .candidates
            .iter()
            .any(|candidate| candidate.label == "onlySecond"));

        let second = "class First {\n    fun onlyFirst() {}\n}\nclass Second {\n    fun onlySecond() {}\n}\nfun make(): Second = Second()\nfun main() { make(). }\n";
        std::fs::write(&file, second).unwrap();
        ws.reindex(&key, second);
        let second_offset = second.find("make(). }").unwrap() + "make().".len();

        let second_query = ws
            .completion_query(&key, second_offset)
            .expect("second return completion");
        assert!(second_query
            .candidates
            .iter()
            .any(|candidate| candidate.label == "onlySecond"));
        assert!(!second_query
            .candidates
            .iter()
            .any(|candidate| candidate.label == "onlyFirst"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn closed_completion_hydrates_current_inferred_function_return_sources() {
        let root = unique_tmp("closed-inferred-function-source");
        std::fs::create_dir_all(&root).unwrap();
        let model_file = root.join("Model.kt");
        let use_file = root.join("Use.kt");
        let model_key = model_file.to_string_lossy().to_string();
        let use_key = use_file.to_string_lossy().to_string();
        let model = "class Item { fun ping() {} }\nclass Other { fun pong() {} }\nclass Model { private fun helper(): Item = Item()\n fun make() = helper() }\n";
        let source = "fun use(model: Model) { model.make().ping() }\n";
        std::fs::write(&model_file, model).unwrap();
        std::fs::write(&use_file, source).unwrap();
        let mut ws = Workspace::new();
        ws.reindex(&model_key, model);
        ws.reindex(&use_key, source);
        let offset = source.find("make().").unwrap() + "make().".len();
        let first = ws.completion_query(&use_key, offset).unwrap();
        assert!(first.candidates.iter().any(|item| item.label == "ping"));
        let defs = ws.goto_definition(&use_key, offset);
        assert_eq!(defs.len(), 1, "{defs:?}");
        assert_eq!(defs[0].file, model_key);
        ws.open(
            &model_key,
            model.replace("helper(): Item = Item()", "helper(): Other = Other()"),
        );
        let second = ws.completion_query(&use_key, offset).unwrap();
        assert!(second.candidates.iter().any(|item| item.label == "pong"));
        assert!(!second.candidates.iter().any(|item| item.label == "ping"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn open_buffer_member_completion_and_goto_stay_on_incremental_path() {
        let root = unique_tmp("open-incremental-member-path");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("Main.kt");
        let key = file.to_string_lossy().to_string();
        let src = "class Leaf {\n    fun target() {}\n}\nclass Mid {\n    fun leaf(): Leaf = Leaf()\n}\nclass Root {\n    val mid: Mid = Mid()\n}\nfun main(root: Root) { root.mid.leaf().target() }\n";

        let mut ws = Workspace::new();
        ws.open(key.clone(), src.to_string());

        let completion_offset = src.find("leaf().target").unwrap() + "leaf().".len();
        let query = ws
            .completion_query(&key, completion_offset)
            .expect("open chained after-dot completion");
        assert!(query
            .candidates
            .iter()
            .any(|candidate| candidate.label == "target"));

        let goto_offset = src.rfind("target").unwrap();
        let defs = ws.goto_definition(&key, goto_offset);
        assert_eq!(defs.len(), 1);
        assert_eq!(&src[defs[0].start_byte..defs[0].end_byte], "target");
        assert!(defs[0].start_byte < src.find("class Mid").unwrap());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn open_buffer_member_completion_updates_after_change() {
        let key = "buffer:///Main.kt";
        let first = "class Box {\n    fun before() {}\n}\nfun main(box: Box) { box. }\n";

        let mut ws = Workspace::new();
        ws.open(key, first.to_string());

        let first_offset = first.find("box. }").unwrap() + "box.".len();
        let first_query = ws
            .completion_query(key, first_offset)
            .expect("first open after-dot completion");
        let first_labels = completion_labels(&first_query);
        assert!(first_labels.iter().any(|label| label == "before"));
        assert!(!first_labels.iter().any(|label| label == "after"));

        let second = "class Box {\n    fun after() {}\n}\nfun main(box: Box) { box. }\n";
        ws.change(key, second.to_string());

        let second_offset = second.find("box. }").unwrap() + "box.".len();
        let second_query = ws
            .completion_query(key, second_offset)
            .expect("second open after-dot completion");
        let second_labels = completion_labels(&second_query);
        assert!(second_labels.iter().any(|label| label == "after"));
        assert!(!second_labels.iter().any(|label| label == "before"));
    }

    #[test]
    fn closed_member_goto_uses_salsa_receiver_inference() {
        let root = unique_tmp("closed-salsa-goto");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("Main.kt");
        let key = file.to_string_lossy().to_string();
        let src = "class Box {\n    fun unbox() {}\n}\nfun main(box: Box) { box.unbox() }\n";
        std::fs::write(&file, src).unwrap();

        let mut ws = Workspace::new();
        ws.reindex(&key, src);

        let offset = src.rfind("unbox").unwrap();
        let defs = ws.goto_definition(&key, offset);

        assert_eq!(defs.len(), 1);
        assert_eq!(&src[defs[0].start_byte..defs[0].end_byte], "unbox");
        assert!(defs[0].start_byte < src.find("main").unwrap());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn salsa_inferred_member_goto_invalidates_after_reindex() {
        let root = unique_tmp("closed-salsa-goto-invalidation");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("Main.kt");
        let key = file.to_string_lossy().to_string();
        let first = "class First {\n    fun target() {}\n}\nclass Second {\n    fun target() {}\n}\nfun main(box: First) { box.target() }\n";
        std::fs::write(&file, first).unwrap();

        let mut ws = Workspace::new();
        ws.reindex(&key, first);
        let offset = first.rfind("target").unwrap();

        let first_defs = ws.goto_definition(&key, offset);
        assert_eq!(first_defs.len(), 1);
        assert!(first_defs[0].start_byte < first.find("class Second").unwrap());

        let second = "class First {\n    fun target() {}\n}\nclass Second {\n    fun target() {}\n}\nfun main(box: Second) { box.target() }\n";
        std::fs::write(&file, second).unwrap();
        ws.reindex(&key, second);

        let second_offset = second.rfind("target").unwrap();
        let second_defs = ws.goto_definition(&key, second_offset);
        assert_eq!(second_defs.len(), 1);
        assert!(second_defs[0].start_byte > second.find("class Second").unwrap());
        assert!(second_defs[0].start_byte < second.find("main").unwrap());

        let _ = std::fs::remove_dir_all(root);
    }
}

struct ObservedCompileTicket {
    request: Arc<crate::compile::ObservedCompileRequest>,
    model: Arc<crate::classpath::ProjectSnapshot>,
    generation: u64,
    revision: u64,
    deferred_outputs: BTreeSet<String>,
    valid: bool,
}

impl Workspace {
    pub(crate) fn invalidate_observed_compile(&mut self) {
        if let Some(receipt) = self.analysis_extensions.compiled_receipt() {
            receipt
                .request
                .invalid
                .store(true, std::sync::atomic::Ordering::Release);
        }
        if let Some(ticket) = &mut self.observed_compile {
            ticket.valid = false;
            ticket
                .request
                .invalid
                .store(true, std::sync::atomic::Ordering::Release);
        }
        self.analysis_extensions.invalidate_compiled(&self.index);
        if self.analysis_extensions.publish(&mut self.index) {
            self.index_revision = self.index_revision.wrapping_add(1).max(1);
        }
    }

    // Source-set ownership for dirty edits is wider than the selected task's read closure.
    // Model replacement already invalidates both active tickets and accepted receipts.
    fn compiled_module_source(
        &self,
        key: &str,
        request: &crate::compile::ObservedCompileRequest,
    ) -> bool {
        self.project_model
            .as_ref()
            .and_then(|model| model.file_origin(Path::new(key)))
            .and_then(|origin| origin.project_scope)
            .is_some_and(|scope| scope.module == request.scope.module)
    }

    /// `true` only defers a known output event inside this exact active compile. Inputs,
    /// configuration, AP/ancestor/dependency files invalidate before any source-retention filter.
    pub(crate) fn observe_compiled_resource(&mut self, key: &str) -> bool {
        self.invalidate_kotlin_source(key);
        let module_source = self
            .observed_compile
            .as_ref()
            .is_some_and(|ticket| self.compiled_module_source(key, &ticket.request));
        if let Some(ticket) = &mut self.observed_compile {
            if ticket.valid && ticket.request.is_known_output(key) {
                ticket.deferred_outputs.insert(key.into());
                return true;
            }
            if module_source || ticket.request.observes_path(key) {
                tracing::debug!(
                    path = key,
                    "compiler receipt input changed through client watch"
                );
                self.invalidate_observed_compile();
            }
        }
        if self
            .analysis_extensions
            .compiled_receipt()
            .is_some_and(|receipt| {
                self.compiled_module_source(key, &receipt.request) || receipt.observes_path(key)
            })
        {
            // A late self-output notification is conservative invalidation, never a grace period.
            tracing::debug!(
                path = key,
                "published compiler receipt resource changed through client watch"
            );
            self.invalidate_observed_compile();
        }
        false
    }

    fn observe_compiled_source(&mut self, key: &str, text: Option<&str>, force: bool) {
        if self.observed_compile.as_ref().is_some_and(|ticket| {
            self.compiled_module_source(key, &ticket.request) || ticket.request.observes_path(key)
        }) || self
            .analysis_extensions
            .compiled_receipt()
            .is_some_and(|receipt| {
                (self.compiled_module_source(key, &receipt.request) || receipt.observes_path(key))
                    && (force || text.is_none_or(|text| !receipt.source_matches(key, text)))
            })
        {
            tracing::debug!(path = key, force, "compiler receipt source changed");
            self.invalidate_observed_compile();
        }
    }

    pub(crate) fn begin_observed_compile(
        &mut self,
        root: &Path,
        key: &str,
    ) -> Result<
        (
            Arc<crate::compile::ObservedCompileRequest>,
            Arc<crate::classpath::ProjectSnapshot>,
        ),
        &'static str,
    > {
        use crate::classpath::CompilerLanguage;
        if !cfg!(windows) {
            return Err("native-tree-proof-unavailable");
        }
        if self.observed_compile.is_some() {
            return Err("compile-already-running");
        }
        if !self.analysis_extensions.compiled_enabled() {
            return Err("provider-disabled");
        }
        if self.project_refresh_pending {
            return Err("project-model-pending");
        }
        let model = self.project_model.clone().ok_or("project-model-unknown")?;
        let (module, source) = model.owner_for_key(key).ok_or("source-owner-unknown")?;
        let source = source.ok_or("source-set-unknown")?;
        // The command runs the current build's actual model task, never a fixture task name.
        if module.build != ":" && module.build != format!("legacy:{}", crate::paths::file_key(root))
        {
            return Err("included-build-selection-unknown");
        }
        if source.compiler_metadata_incomplete || source.classpath_incomplete {
            return Err("compiler-model-incomplete");
        }
        let java: Vec<_> = source
            .compiler_tasks
            .iter()
            .filter(|task| task.language == CompilerLanguage::Java && task.complete)
            .collect();
        let [task] = java.as_slice() else {
            return Err("selected-java-task-unknown");
        };
        let scope = model
            .file_origin(Path::new(key))
            .and_then(|origin| origin.project_scope)
            .ok_or("source-scope-unknown")?;
        let roots: Vec<_> = module
            .source_sets
            .iter()
            .flat_map(|source| &source.source_roots)
            .map(|root| crate::paths::file_key(root))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if self.open_docs.iter().any(|(key, doc)| {
            roots
                .iter()
                .any(|root| crate::compile::path_under(key, root))
                && std::fs::read_to_string(key).ok().as_deref() != Some(doc.text.as_str())
        }) {
            return Err("unsaved-module-source");
        }
        let mut owners = Vec::new();
        for key in self
            .project_file_meta
            .keys()
            .chain(self.open_docs.keys())
            .collect::<BTreeSet<_>>()
        {
            if !key.ends_with(".java")
                || !source
                    .source_roots
                    .iter()
                    .any(|root| crate::compile::path_under(key, &crate::paths::file_key(root)))
            {
                continue;
            }
            let entries = self.index.entries_for_file(key);
            let types: Vec<_> = entries
                .iter()
                .filter(|entry| entry.sym.kind.is_type_like())
                .collect();
            let [owner] = types.as_slice() else {
                continue;
            };
            if owner.sym.kind != SymbolKind::Class
                || owner.sym.container.is_some()
                || !owner.sym.type_params.is_empty()
            {
                continue;
            }
            let Some(origin) = self
                .index
                .file_origin(key)
                .filter(|origin| origin.project_scope.as_ref() == Some(&scope))
            else {
                continue;
            };
            owners.push(crate::compile::CompileOwner {
                definition: Def {
                    file: key.clone(),
                    start_byte: owner.sym.start_byte,
                    end_byte: owner.sym.end_byte,
                },
                qualified_name: if owner.sym.package.is_empty() {
                    owner.sym.name.clone()
                } else {
                    format!("{}.{}", owner.sym.package, owner.sym.name)
                },
                origin_id: origin.origin_id.clone(),
            });
        }
        if owners.is_empty() {
            return Err("supported-authored-owner-unknown");
        }
        self.invalidate_observed_compile();
        let nonce = format!(
            "{}-{}-{}",
            std::process::id(),
            self.index_revision,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| "clock-unknown")?
                .as_nanos()
        );
        let mut request = crate::compile::ObservedCompileRequest {
            root: root.to_path_buf(),
            project_dir: module.project_dir.clone(),
            scope,
            source_set: source.name.clone(),
            task: (**task).clone(),
            compiler_tasks: source.compiler_tasks.clone(),
            project_path: module.module.clone(),
            // Registered roots of other source sets are dirty-edit coverage, not reads of
            // this selected compiler task (notably their absent generated output directories).
            source_roots: source
                .source_roots
                .iter()
                .map(|root| crate::paths::file_key(root))
                .collect(),
            classpath: source
                .entries
                .iter()
                .map(|path| crate::paths::file_key(path))
                .collect(),
            cache_dir: crate::deps::cache_home()
                .join("observed-compile")
                .join(&nonce),
            owners,
            nonce,
            invalid: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            prepared_paths: None,
        };
        request
            .prepare_paths()
            .map_err(|_| "compiler-input-output-overlap-or-unknown")?;
        let request = Arc::new(request);
        self.observed_compile = Some(ObservedCompileTicket {
            request: request.clone(),
            model: model.clone(),
            generation: self.analysis_generation,
            revision: self.index_revision,
            deferred_outputs: BTreeSet::new(),
            valid: true,
        });
        Ok((request, model))
    }

    pub(crate) fn finish_observed_compile(
        &mut self,
        request: &Arc<crate::compile::ObservedCompileRequest>,
        result: crate::compile::ObservedCompileResult,
    ) -> (&'static str, usize) {
        let Some(ticket) = &self.observed_compile else {
            return ("stale-inputs", 0);
        };
        if !Arc::ptr_eq(&ticket.request, request) {
            return ("foreign-compile", 0);
        }
        if !ticket.valid
            || request.check_live().is_err()
            || ticket.generation != self.analysis_generation
            || ticket.revision != self.index_revision
            || !self
                .project_model
                .as_ref()
                .is_some_and(|model| Arc::ptr_eq(&ticket.model, model))
        {
            self.observed_compile = None;
            return ("stale-inputs", 0);
        }
        let deferred = ticket.deferred_outputs.len();
        if let Some(receipt) = &result.receipt {
            if !ticket
                .deferred_outputs
                .iter()
                .all(|key| receipt.covers_output(key))
            {
                self.invalidate_observed_compile();
                self.observed_compile = None;
                return ("unverified-deferred-output", deferred);
            }
        }
        self.observed_compile = None;
        let Some(receipt) = result.receipt else {
            return ("compile-unverified", deferred);
        };
        let status = if self
            .analysis_extensions
            .accept_compiled(&self.index, receipt, result.owners)
            .is_ok()
        {
            "observed-success"
        } else {
            "source-owner-unknown"
        };
        self.bump_index_revision();
        (status, deferred)
    }

    pub(crate) fn invalidate_owned_compile(
        &mut self,
        request: &Arc<crate::compile::ObservedCompileRequest>,
    ) -> bool {
        let owned = self
            .observed_compile
            .as_ref()
            .is_some_and(|ticket| Arc::ptr_eq(&ticket.request, request))
            || self
                .analysis_extensions
                .compiled_receipt()
                .is_some_and(|receipt| Arc::ptr_eq(&receipt.request, request));
        if owned {
            self.invalidate_observed_compile();
        }
        owned
    }

    fn refresh_compiled_freshness(&mut self) {
        if self.compiler_api_environments.values().any(|environment| {
            environment
                .invalid
                .load(std::sync::atomic::Ordering::Acquire)
        }) {
            self.compiler_api_environments.clear();
            self.analysis_extensions.clear_compiler_api(&self.index);
            if self.analysis_extensions.publish(&mut self.index) {
                self.index_revision = self.index_revision.wrapping_add(1).max(1);
            }
        }
        let invalid = self
            .analysis_extensions
            .compiled_receipt()
            .is_some_and(|receipt| receipt.request.check_live().is_err())
            || self
                .observed_compile
                .as_ref()
                .is_some_and(|ticket| ticket.request.check_live().is_err());
        if invalid {
            self.invalidate_observed_compile();
        }
    }

    pub(crate) fn stop_observed_compile(&mut self) {
        if let Some(ticket) = &self.observed_compile {
            ticket
                .request
                .cancelled
                .store(true, std::sync::atomic::Ordering::Release);
        }
        self.invalidate_observed_compile();
    }

    pub(crate) fn cancel_observed_compile(
        &mut self,
        request: &Arc<crate::compile::ObservedCompileRequest>,
    ) {
        if self
            .observed_compile
            .as_ref()
            .is_some_and(|ticket| Arc::ptr_eq(&ticket.request, request))
        {
            self.observed_compile = None;
        }
    }
}

#[cfg(test)]
mod observed_compile_lifecycle_tests {
    use super::*;
    use crate::compile::{observed_test_receipt, observed_test_request, ObservedCompileResult};
    use std::sync::atomic::Ordering;

    fn ticket(ws: &mut Workspace, request: Arc<crate::compile::ObservedCompileRequest>) {
        let model = Arc::new(crate::classpath::ProjectSnapshot::new(Vec::new()));
        ws.project_model = Some(model.clone());
        ws.observed_compile = Some(ObservedCompileTicket {
            request,
            model,
            generation: ws.analysis_generation,
            revision: ws.index_revision,
            deferred_outputs: BTreeSet::new(),
            valid: true,
        });
    }

    fn failed(
        receipt: Option<Arc<crate::compile::SuccessfulCompileReceipt>>,
    ) -> ObservedCompileResult {
        ObservedCompileResult {
            outcome: Default::default(),
            receipt,
            owners: Vec::new(),
            reason: "lifecycle-fixture-only".into(),
        }
    }

    // Synthetic host lifecycle controls; these do not certify native Gradle success.
    fn selected_and_test_model(
        root: &Path,
        task: &crate::classpath::CompilerTask,
    ) -> Arc<crate::classpath::ProjectSnapshot> {
        use crate::classpath::{ModuleClasspath, ProjectSnapshot, SourceSetClasspath};
        let main = SourceSetClasspath {
            name: "main".into(),
            source_roots: vec![root.join("src")],
            compiler_tasks: vec![task.clone()],
            ..Default::default()
        };
        let test = SourceSetClasspath {
            name: "test".into(),
            source_roots: vec![
                root.join("test"),
                root.join("build/generated/sources/annotationProcessor/java/test"),
                root.with_extension("external-test"),
            ],
            ..Default::default()
        };
        let module =
            |directory: std::path::PathBuf, build: &str, name: &str, source_sets| ModuleClasspath {
                module: name.into(),
                project_dir: directory,
                build: build.into(),
                source_sets,
                entries: Vec::new(),
                unresolved: Vec::new(),
            };
        Arc::new(ProjectSnapshot::new(vec![
            module(root.into(), ":", ":", vec![main, test]),
            module(
                root.join("foreign"),
                ":",
                ":foreign",
                vec![SourceSetClasspath {
                    name: "main".into(),
                    source_roots: vec![root.join("foreign/src")],
                    ..Default::default()
                }],
            ),
            module(
                root.join("included"),
                ":included",
                ":",
                vec![SourceSetClasspath {
                    name: "main".into(),
                    source_roots: vec![root.join("included/src")],
                    ..Default::default()
                }],
            ),
        ]))
    }

    fn module_request(
        root: &Path,
    ) -> (
        Arc<crate::compile::ObservedCompileRequest>,
        Arc<crate::classpath::ProjectSnapshot>,
    ) {
        let mut request = observed_test_request(root);
        let model = selected_and_test_model(root, &request.task);
        Arc::get_mut(&mut request).unwrap().scope = model
            .file_origin(&root.join("src/Person.java"))
            .unwrap()
            .project_scope
            .unwrap();
        (request, model)
    }

    #[cfg(windows)]
    #[test]
    fn selected_compile_reads_exclude_unrelated_absent_test_generated_root() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp-selected-roots-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("src")).unwrap();
        let owner = crate::paths::file_key(&root.join("src/Person.java"));
        let text = "public class Person { public String authored() { return \"name\"; } }";
        std::fs::write(&owner, text).unwrap();
        let request = observed_test_request(&root);
        let model = selected_and_test_model(&root, &request.task);
        let mut ws = Workspace::new();
        ws.set_project_model(model);
        ws.open(owner.clone(), text.into());
        let (request, _) = ws.begin_observed_compile(&root, &owner).unwrap();
        assert_eq!(
            request.source_roots,
            vec![crate::paths::file_key(&root.join("src"))]
        );
        let excluded = crate::paths::file_key(
            &root.join("build/generated/sources/annotationProcessor/java/test"),
        );
        assert!(!request.initial_watch_paths().contains(&excluded));
        let plan =
            crate::compiled_watch::WatchPlan::build(&request, &request.initial_watch_paths())
                .unwrap();
        assert!(
            !plan.covers_read(&excluded, true),
            "test registration is not selected task read coverage"
        );
        assert!(
            !plan.covers_read(&format!("{excluded}/Surprise.java"), false),
            "a later unexpected upstream/test read must remain uncovered and reject"
        );
        ws.cancel_observed_compile(&request);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pending_main_test_and_external_source_aba_latch_without_foreign_module_aliasing() {
        let root = std::env::temp_dir().join("ktlsp-module-source-aba-pending");
        for (path, same_module) in [
            (root.join("src/Consumer.java"), true),
            (root.join("test/TestConsumer.java"), true),
            (
                root.with_extension("external-test")
                    .join("ExternalTest.java"),
                true,
            ),
            (root.join("foreign/src/Foreign.java"), false),
            (root.join("included/src/Included.java"), false),
        ] {
            let (request, model) = module_request(&root);
            let mut ws = Workspace::new();
            ws.set_project_model(model.clone());
            let key = crate::paths::file_key(&path);
            ws.open(key.clone(), "class Consumer {}".into());
            assert!(!ws.analysis_extensions.needs_source(&key));
            ws.observed_compile = Some(ObservedCompileTicket {
                request: request.clone(),
                model,
                generation: ws.analysis_generation,
                revision: ws.index_revision,
                deferred_outputs: BTreeSet::new(),
                valid: true,
            });
            ws.change(&key, "class Consumer { broken }".into());
            ws.change(&key, "class Consumer {}".into());
            assert_eq!(
                request.invalid.load(Ordering::Acquire),
                same_module,
                "{key}"
            );
            if same_module {
                assert!(!ws.observed_compile.as_ref().unwrap().valid);
            }
        }
    }

    #[test]
    fn accepted_same_module_client_source_changes_retract_and_stay_invalid_after_aba() {
        let root = std::env::temp_dir().join("ktlsp-module-source-aba-accepted");
        for (path, same_module) in [
            (root.join("src/Consumer.java"), true),
            (root.join("test/TestConsumer.java"), true),
            (
                root.with_extension("external-test")
                    .join("ExternalTest.java"),
                true,
            ),
            (root.join("foreign/src/Foreign.java"), false),
            (root.join("included/src/Included.java"), false),
        ] {
            let (request, model) = module_request(&root);
            let mut ws = Workspace::new();
            ws.set_project_model(model);
            ws.analysis_extensions
                .accept_compiled(
                    &ws.index,
                    observed_test_receipt(request.clone()),
                    Vec::new(),
                )
                .unwrap();
            let key = crate::paths::file_key(&path);
            assert!(!ws.observe_compiled_resource(&key));
            assert!(!ws.observe_compiled_resource(&key));
            assert_eq!(
                request.invalid.load(Ordering::Acquire),
                same_module,
                "{key}"
            );
            assert_eq!(
                ws.analysis_extensions.compiled_receipt().is_none(),
                same_module,
                "{key}"
            );
        }
    }

    #[test]
    fn failed_and_unverified_deferred_outcomes_retire_the_ticket() {
        let root = std::env::temp_dir().join("ktlsp-observed-ticket-lifecycle");
        let request = observed_test_request(&root);
        let mut ws = Workspace::new();
        ticket(&mut ws, request.clone());
        assert_eq!(
            ws.finish_observed_compile(&request, failed(None)).0,
            "compile-unverified"
        );
        assert!(ws.observed_compile.is_none());
        ticket(&mut ws, request.clone());
        ws.observed_compile
            .as_mut()
            .unwrap()
            .deferred_outputs
            .insert(crate::paths::file_key(&root.join("build/other")));
        assert_eq!(
            ws.finish_observed_compile(
                &request,
                failed(Some(observed_test_receipt(request.clone())))
            )
            .0,
            "unverified-deferred-output"
        );
        assert!(request.invalid.load(Ordering::Acquire));
        assert!(ws.observed_compile.is_none());
    }

    #[test]
    fn dirty_nonretained_consumer_a_b_a_rejects_pending_publication() {
        let root = std::env::temp_dir().join("ktlsp-observed-dirty-consumer");
        let consumer = crate::paths::file_key(&root.join("src/Consumer.java"));
        let mut ws = Workspace::new();
        ws.open(consumer.clone(), "class Consumer {}".into());
        assert!(!ws.analysis_extensions.needs_source(&consumer));
        let request = observed_test_request(&root);
        ticket(&mut ws, request.clone());
        ws.change(&consumer, "class Consumer { broken }".into());
        ws.change(&consumer, "class Consumer {}".into());
        assert!(request.invalid.load(Ordering::Acquire));
        assert_eq!(
            ws.finish_observed_compile(&request, failed(None)).0,
            "stale-inputs"
        );
        assert!(ws.observed_compile.is_none());
    }

    #[test]
    fn old_callback_and_retire_do_not_touch_a_new_owner() {
        let root = std::env::temp_dir().join("ktlsp-observed-owner");
        let old = observed_test_request(&root);
        let current = observed_test_request(&root);
        let mut ws = Workspace::new();
        ticket(&mut ws, current.clone());
        assert!(!ws.invalidate_owned_compile(&old));
        ws.cancel_observed_compile(&old);
        assert!(Arc::ptr_eq(
            &ws.observed_compile.as_ref().unwrap().request,
            &current
        ));
        assert!(ws.observed_compile.as_ref().unwrap().valid);
        assert!(!current.invalid.load(Ordering::Acquire));
        assert!(ws.invalidate_owned_compile(&current));
        assert!(!ws.observed_compile.as_ref().unwrap().valid);
    }

    #[test]
    fn model_generation_or_index_replacement_rejects_the_old_publication() {
        let root = std::env::temp_dir().join("ktlsp-observed-publication-cas");
        for replacement in ["model-arc", "generation", "index-revision"] {
            let request = observed_test_request(&root);
            let mut ws = Workspace::new();
            ticket(&mut ws, request.clone());
            match replacement {
                "model-arc" => {
                    ws.project_model =
                        Some(Arc::new(crate::classpath::ProjectSnapshot::new(Vec::new())))
                }
                "generation" => ws.analysis_generation = ws.analysis_generation.wrapping_add(1),
                "index-revision" => ws.index_revision = ws.index_revision.wrapping_add(1),
                _ => unreachable!(),
            }
            // A test-only host receipt exercises publication CAS, not native success.
            let result = failed(Some(observed_test_receipt(request.clone())));
            assert_eq!(
                ws.finish_observed_compile(&request, result).0,
                "stale-inputs",
                "{replacement}"
            );
            assert!(ws.observed_compile.is_none());
            assert!(ws.analysis_extensions.compiled_receipt().is_none());
        }
    }

    // This is a host-publication/invalidation control, not a successful compiler invocation.
    // Person.class is the existing independently compiled fixture; the receipt is test-only.
    fn getter_owner_fixture() -> (
        Workspace,
        Arc<crate::compile::ObservedCompileRequest>,
        String,
        crate::compile::CompiledOwnerInput,
    ) {
        use crate::project_model::{FileOrigin, ScopeVisibility, SourceRepresentation};
        const SOURCE: &str =
            include_str!("../../ktcore/tests/fixtures/classfile/sources/pluginfixture/Person.java");
        const CLASS: &[u8] =
            include_bytes!("../../ktcore/tests/fixtures/classfile/pluginfixture/Person.class");
        let root = std::env::temp_dir().join("ktlsp-admitted-getter-lifecycle");
        let key = crate::paths::file_key(&root.join("src/Person.java"));
        let mut request = observed_test_request(&root);
        let mut ws = Workspace::new();
        ws.open(key.clone(), SOURCE.into());
        let input = Arc::get_mut(&mut request).unwrap();
        input
            .task
            .declared_input_files
            .push(root.join("ap/options.json"));
        input.compiler_tasks[0] = input.task.clone();
        let owner = ws
            .index
            .entries_for_file(&key)
            .into_iter()
            .find(|entry| entry.sym.kind == SymbolKind::Class && entry.sym.name == "Person")
            .unwrap();
        input.owners.push(crate::compile::CompileOwner {
            definition: Def {
                file: key.clone(),
                start_byte: owner.sym.start_byte,
                end_byte: owner.sym.end_byte,
            },
            qualified_name: "pluginfixture.Person".into(),
            origin_id: "lifecycle-owner".into(),
        });
        ws.index.set_file_origin(
            &key,
            FileOrigin {
                origin_id: "lifecycle-owner".into(),
                representation: SourceRepresentation::Source,
                project_scope: Some(input.scope.clone()),
            },
        );
        ws.index.replace_scope_visibilities(vec![ScopeVisibility {
            scope: input.scope.clone(),
            ordered_origins: vec!["lifecycle-owner".into()],
        }]);
        let owner = crate::compile::CompiledOwnerInput {
            file: key.clone(),
            text: Arc::from(SOURCE),
            tree: crate::java::JavaParser::new().parse(SOURCE),
            class_file: input.task.output_roots[0].join("pluginfixture/Person.class"),
            class_file_bytes: CLASS.len() as u64,
            class: Arc::new(ktcore::classfile::decode("lifecycle-owner", CLASS).unwrap()),
        };
        (ws, request, key, owner)
    }

    #[test]
    fn prepared_paths_still_retract_the_actual_admitted_getter_on_config_aba() {
        const CLASS: &[u8] =
            include_bytes!("../../ktcore/tests/fixtures/classfile/pluginfixture/Person.class");
        let (mut ws, mut request, key, owner) = getter_owner_fixture();
        Arc::get_mut(&mut request).unwrap().prepare_paths().unwrap();
        let receipt = crate::compile::observed_test_receipt_with_source(
            request.clone(),
            &key,
            &owner.text,
            &owner.class_file,
            CLASS,
        );
        ws.analysis_extensions
            .accept_compiled(&ws.index, receipt, vec![owner])
            .unwrap();
        ws.bump_index_revision();
        let count = |ws: &Workspace| {
            ktcore::jvm::member_candidates(
                &ws.index.scoped_for_file(&key),
                &ktcore::types::Type::class("Person", Some("pluginfixture".into())),
                "getName",
            )
            .len()
        };
        assert_eq!(count(&ws), 1);
        let config = crate::paths::file_key(&request.root.join("gradle.properties"));
        assert!(!ws.observe_compiled_resource(&config));
        assert!(!ws.observe_compiled_resource(&config));
        assert!(request.invalid.load(Ordering::Acquire));
        assert!(ws.analysis_extensions.compiled_receipt().is_none());
        assert_eq!(count(&ws), 0);
    }

    fn admitted_getter_fixture() -> (
        Workspace,
        Arc<crate::compile::ObservedCompileRequest>,
        String,
    ) {
        const CLASS: &[u8] =
            include_bytes!("../../ktcore/tests/fixtures/classfile/pluginfixture/Person.class");
        let (mut ws, request, key, owner) = getter_owner_fixture();
        let receipt = crate::compile::observed_test_receipt_with_source(
            request.clone(),
            &key,
            &owner.text,
            &owner.class_file,
            CLASS,
        );
        ws.analysis_extensions
            .accept_compiled(&ws.index, receipt, vec![owner])
            .unwrap();
        ws.bump_index_revision();
        (ws, request, key)
    }

    #[test]
    fn admission_rechecks_class_bytes_against_the_selected_successful_output() {
        const CLASS: &[u8] =
            include_bytes!("../../ktcore/tests/fixtures/classfile/pluginfixture/Person.class");
        let (mut ws, request, key, mut owner) = getter_owner_fixture();
        let receipt = crate::compile::observed_test_receipt_with_source(
            request,
            &key,
            &owner.text,
            &owner.class_file,
            CLASS,
        );
        let mut different = CLASS.to_vec();
        let offset = different
            .windows(7)
            .position(|value| value == b"getName")
            .unwrap();
        different[offset..offset + 7].copy_from_slice(b"getElse");
        owner.class = Arc::new(ktcore::classfile::decode("lifecycle-owner", &different).unwrap());
        assert!(ws
            .analysis_extensions
            .accept_compiled(&ws.index, receipt, vec![owner])
            .is_err());
        assert!(ws.analysis_extensions.compiled_receipt().is_none());
        let scoped = ws.index.scoped_for_file(&key);
        for name in ["getName", "getElse"] {
            assert!(
                ktcore::jvm::member_candidates(
                    &scoped,
                    &ktcore::types::Type::class("Person", Some("pluginfixture".into())),
                    name
                )
                .is_empty(),
                "rejected output contributed {name}"
            );
        }
    }

    #[test]
    fn diagnostics_retracts_invalid_compiled_members_before_any_other_query() {
        let (mut ws, request, key) = admitted_getter_fixture();
        let count = |ws: &Workspace| {
            ktcore::jvm::member_candidates(
                &ws.index.scoped_for_file(&key),
                &ktcore::types::Type::class("Person", Some("pluginfixture".into())),
                "getName",
            )
            .len()
        };
        assert_eq!(count(&ws), 1);
        let _ = ws.take_extension_diagnostic_changes();
        request.invalid.store(true, Ordering::Release);
        let _ = ws.diagnostics(&key);
        assert!(ws.analysis_extensions.compiled_receipt().is_none());
        assert_eq!(count(&ws), 0);
        assert!(ws.take_extension_diagnostic_changes().contains(&key));
    }

    #[test]
    fn source_config_binary_aba_and_error_latch_retract_the_actual_admitted_getter() {
        const SOURCE: &str =
            include_str!("../../ktcore/tests/fixtures/classfile/sources/pluginfixture/Person.java");
        for mutation in ["source-aba", "config-aba", "binary-aba", "callback-error"] {
            let (mut ws, request, key) = admitted_getter_fixture();
            let count = |ws: &Workspace| {
                let scoped = ws.index.scoped_for_file(&key);
                let entries = ktcore::jvm::member_candidates(
                    &scoped,
                    &ktcore::types::Type::class("Person", Some("pluginfixture".into())),
                    "getName",
                );
                for entry in &entries {
                    assert!(entry.definition().is_none(), "no invented source target");
                }
                entries.len()
            };
            assert_eq!(
                count(&ws),
                1,
                "independently compiled getter was admitted before {mutation}"
            );
            let _ = ws.take_extension_diagnostic_changes();
            match mutation {
                "source-aba" => {
                    ws.change(&key, format!("{SOURCE}\n// edited"));
                    ws.change(&key, SOURCE.into());
                }
                "config-aba" => {
                    let config = crate::paths::file_key(&request.root.join("ap/options.json"));
                    assert!(!ws.observe_compiled_resource(&config));
                    assert!(!ws.observe_compiled_resource(&config));
                }
                "binary-aba" => {
                    let binary = crate::paths::file_key(
                        &request.task.output_roots[0].join("pluginfixture/Person.class"),
                    );
                    assert!(!ws.observe_compiled_resource(&binary));
                    assert!(!ws.observe_compiled_resource(&binary));
                }
                "callback-error" => {
                    request.invalid.store(true, Ordering::Release);
                    ws.refresh_compiled_freshness();
                }
                _ => unreachable!(),
            }
            assert!(
                request.invalid.load(Ordering::Acquire),
                "ABA cannot restore the receipt"
            );
            assert!(ws.analysis_extensions.compiled_receipt().is_none());
            assert_eq!(count(&ws), 0, "stale callable survived {mutation}");
            assert!(
                ws.take_extension_diagnostic_changes().contains(&key),
                "affected diagnostics must be queued"
            );
        }
    }

    #[test]
    fn late_output_and_callback_latch_invalidate_an_accepted_owner() {
        let root = std::env::temp_dir().join("ktlsp-observed-late-output");
        let request = observed_test_request(&root);
        let mut ws = Workspace::new();
        ws.analysis_extensions
            .accept_compiled(
                &ws.index,
                observed_test_receipt(request.clone()),
                Vec::new(),
            )
            .unwrap();
        assert!(ws.analysis_extensions.compiled_receipt().is_some());
        assert!(!ws.observe_compiled_resource(&crate::paths::file_key(
            &root.join("build/classes/java/main/Member.class")
        )));
        assert!(ws.analysis_extensions.compiled_receipt().is_none());
        let request = observed_test_request(&root);
        ws.analysis_extensions
            .accept_compiled(
                &ws.index,
                observed_test_receipt(request.clone()),
                Vec::new(),
            )
            .unwrap();
        request.invalid.store(true, Ordering::Release);
        // Same pre-feature check used by ordinary completion/hover/diagnostics, no per-key rehash.
        ws.refresh_compiled_freshness();
        assert!(ws.analysis_extensions.compiled_receipt().is_none());
    }
}

struct KotlinDiagnosticTicket {
    request: Arc<crate::compile::KotlinDiagnosticRequest>,
    model: Arc<crate::classpath::ProjectSnapshot>,
    analysis_generation: u64,
}

struct KotlinFileDiagnostics {
    request: Arc<crate::compile::KotlinDiagnosticRequest>,
    model: Arc<crate::classpath::ProjectSnapshot>,
    environment_invalid: Arc<std::sync::atomic::AtomicBool>,
    diagnostics: Vec<crate::diagnostics::Diagnostic>,
    store_epoch: u64,
    clean: bool,
}

impl Workspace {
    pub(crate) fn kotlin_diagnostic_targets(&self) -> Vec<(ProjectScope, String)> {
        let mut scopes = BTreeMap::new();
        let keys: BTreeSet<_> = self
            .open_docs
            .keys()
            .filter(|key| key.ends_with(".kt"))
            .collect();
        for key in keys {
            if let Some(scope) = self.project_scope_for_file(key) {
                scopes.entry(scope).or_insert_with(|| key.clone());
            }
        }
        scopes.into_iter().collect()
    }

    fn kotlin_source_affects(
        &self,
        request: &crate::compile::KotlinDiagnosticRequest,
        key: &str,
    ) -> bool {
        if self
            .project_model
            .as_ref()
            .and_then(|model| model.file_origin(Path::new(key)))
            .is_some_and(|origin| {
                origin.project_scope.as_ref() == Some(&request.scope)
                    || request.visible_origins.contains(&origin.origin_id)
            })
        {
            return true;
        }
        if request
            .processor_plan
            .as_ref()
            .is_some_and(|plan| plan.affects_processor_source(Path::new(key)))
        {
            return true;
        }
        // Keep selected external sources and files without a current model origin protected.
        request
            .sources
            .iter()
            .any(|path| crate::paths::file_key(path) == key)
    }

    fn invalidate_kotlin_source(&mut self, key: &str) {
        let mut affected = BTreeSet::new();
        if key.ends_with(".kt") || key.ends_with(".java") {
            if let Some(scope) = self.project_scope_for_file(key) {
                affected.insert(scope);
            }
        }
        let mut seen = BTreeSet::new();
        for request in self
            .kotlin_diagnostic_tickets
            .values()
            .map(|ticket| &ticket.request)
            .chain(
                self.kotlin_diagnostic_reports
                    .values()
                    .map(|report| &report.request),
            )
        {
            // Many file reports share one request. Distinct old/current requests must still
            // check their selected-source fallback until their scope is known affected.
            if affected.contains(&request.scope) || !seen.insert(Arc::as_ptr(request)) {
                continue;
            }
            if self.kotlin_source_affects(request, key) {
                affected.insert(request.scope.clone());
            }
        }
        for scope in affected {
            self.invalidate_kotlin_scope(&scope);
        }
    }

    pub(crate) fn invalidate_kotlin_scope(&mut self, scope: &ProjectScope) {
        for request in self
            .kotlin_diagnostic_tickets
            .values()
            .map(|ticket| &ticket.request)
            .chain(
                self.kotlin_diagnostic_reports
                    .values()
                    .map(|report| &report.request),
            )
        {
            if &request.scope == scope {
                request
                    .invalid
                    .store(true, std::sync::atomic::Ordering::Release);
            }
        }
        let changes = &mut self.kotlin_diagnostic_changes;
        self.kotlin_diagnostic_reports.retain(|key, report| {
            if &report.request.scope == scope {
                changes.insert(key.clone());
                false
            } else {
                true
            }
        });
        self.kotlin_diagnostic_tickets.remove(scope);
        let generation = self
            .kotlin_source_generations
            .entry(scope.clone())
            .or_default();
        *generation = generation.saturating_add(1);
    }

    pub(crate) fn invalidate_kotlin_diagnostics(&mut self) {
        let scopes: BTreeSet<_> = self
            .kotlin_diagnostic_tickets
            .keys()
            .cloned()
            .chain(
                self.kotlin_diagnostic_reports
                    .values()
                    .map(|report| report.request.scope.clone()),
            )
            .collect();
        for scope in scopes {
            self.invalidate_kotlin_scope(&scope);
        }
    }

    pub(crate) fn begin_kotlin_diagnostics(
        &mut self,
        key: &str,
    ) -> Result<Arc<crate::compile::KotlinDiagnosticRequest>, &'static str> {
        self.begin_kotlin_diagnostics_for_sources(key, false)
    }

    pub(crate) fn begin_kotlin_compiler_api(
        &mut self,
        key: &str,
    ) -> Result<Arc<crate::compile::KotlinDiagnosticRequest>, &'static str> {
        self.begin_kotlin_diagnostics_for_sources(key, true)
    }

    fn begin_kotlin_diagnostics_for_sources(
        &mut self,
        key: &str,
        allow_closed: bool,
    ) -> Result<Arc<crate::compile::KotlinDiagnosticRequest>, &'static str> {
        use crate::classpath::CompilerLanguage;
        if self.project_refresh_pending {
            return Err("kotlin-project-model-pending");
        }
        let model = self
            .project_model
            .clone()
            .ok_or("kotlin-project-model-unknown")?;
        let (module, source) = model
            .owner_for_key(key)
            .ok_or("kotlin-source-owner-unknown")?;
        let source = source.ok_or("kotlin-source-set-unknown")?;
        if source.compiler_metadata_incomplete
            || source.classpath_incomplete
            || !source.unresolved.is_empty()
        {
            return Err("kotlin-compiler-model-incomplete");
        }
        let selected_task = source
            .kotlin_compile_task
            .as_deref()
            .ok_or("selected-kotlin-compilation-unknown")?;
        let mut tasks = source
            .compiler_tasks
            .iter()
            .filter(|task| task.language == CompilerLanguage::Kotlin && task.task == selected_task);
        let task = tasks.next().ok_or("selected-kotlin-task-unknown")?;
        if tasks.next().is_some()
            || !task.complete
            || task.problem.is_some()
            || task.kotlin_compiler.is_none()
        {
            return Err("selected-kotlin-task-incomplete");
        }
        let scope = model
            .file_origin(Path::new(key))
            .and_then(|origin| origin.project_scope)
            .ok_or("kotlin-source-scope-unknown")?;
        let visibilities = model.scope_visibilities(&[]);
        let visible_origins: BTreeSet<_> = visibilities
            .iter()
            .find(|visibility| visibility.scope == scope)
            .map(|visibility| visibility.ordered_origins.iter().cloned().collect())
            .unwrap_or_default();
        let mut sources: BTreeMap<_, _> = task
            .source_files
            .iter()
            .map(|path| (crate::paths::file_key(path), path.clone()))
            .collect();
        let source_views = model.compiler_source_views();
        let mut java_editor_sources = BTreeSet::new();
        let mut processor_plan = if self.analysis_extensions.is_empty() {
            model.plain_java_task(key).and_then(|java| {
                crate::compile::ProcessorPlan::capture(module, source, task, java, &model)
                    .inspect_err(|error| {
                        tracing::debug!(task = %task.task, %error,
                            "selected processor execution plan unavailable");
                    })
                    .ok()
            })
        } else {
            None
        };
        let mut overlays = Vec::new();
        let mut dependency_overlays = Vec::new();
        for (open_key, doc) in &self.open_docs {
            let origin = model.file_origin(Path::new(open_key));
            // Selected generated inputs can be outside the SourceSet's registered roots.
            let own = sources.contains_key(open_key)
                || origin
                    .as_ref()
                    .is_some_and(|origin| origin.project_scope.as_ref() == Some(&scope));
            let dependency = origin
                .as_ref()
                .is_some_and(|origin| visible_origins.contains(&origin.origin_id));
            let processor = processor_plan
                .as_ref()
                .is_some_and(|plan| plan.affects_processor_source(Path::new(open_key)));
            if !own && !dependency && !processor {
                continue;
            }
            let revision = self
                .index
                .jvm_source_for_file(open_key)
                .map_or(self.index_revision, |source| source.revision);
            if own {
                if open_key.ends_with(".kt") || open_key.ends_with(".java") {
                    // Preserve the canonical editor policy; Java needs stock task membership too.
                    if !sources.contains_key(open_key) {
                        let mut views = source_views
                            .iter()
                            .filter(|view| view.matches_file(open_key));
                        if views
                            .next()
                            .is_some_and(|view| view.visibility.scope == scope)
                            && views.next().is_none()
                        {
                            java_editor_sources.insert(open_key.clone());
                        }
                    }
                    sources.insert(open_key.clone(), std::path::PathBuf::from(open_key));
                    overlays.push(crate::compile::KotlinSourceOverlay::new(
                        open_key.clone(),
                        doc.text.clone(),
                        revision,
                    ));
                }
            } else {
                dependency_overlays.push(crate::compile::KotlinSourceOverlay::new(
                    open_key.clone(),
                    doc.text.clone(),
                    revision,
                ));
            }
        }
        if !allow_closed && !overlays.iter().any(|source| source.key.ends_with(".kt")) {
            return Err("no-open-owned-kotlin-source");
        }
        overlays.sort_by(|a, b| a.key.cmp(&b.key));
        dependency_overlays.sort_by(|a, b| a.key.cmp(&b.key));
        // Plain javac cannot reject virtual targets contributed by LSPExtension providers.
        // Until Java compiler positives reconcile with those facts, keep this surface unavailable.
        let compiler_sources_supported = self.analysis_extensions.is_empty()
            && sources
                .keys()
                .chain(dependency_overlays.iter().map(|source| &source.key))
                .all(|file| self.index.contributions_for_file(file).is_empty());
        let java_inputs = if compiler_sources_supported {
            model
                .plain_java_task_for_editor(key, &java_editor_sources)
                .and_then(|java| {
                    java.java_compiler_recipe()?;
                    crate::compile::java_import_dependencies(
                        task,
                        java,
                        source,
                        &model,
                        &visibilities,
                        &visible_origins,
                    )
                    .map(|dependencies| (java.clone(), dependencies))
                    .or_else(|| processor_plan.as_ref().map(|_| (java.clone(), Vec::new())))
                })
        } else {
            None
        };
        if java_inputs.is_none() {
            processor_plan = None;
        }
        let (java_task, java_dependencies) = match java_inputs {
            Some((task, dependencies)) => (Some(task), dependencies),
            None if crate::compile::compiler_api_candidate(task) => {
                let java: Vec<_> = source
                    .compiler_tasks
                    .iter()
                    .filter(|task| {
                        task.language == CompilerLanguage::Java
                            && task.complete
                            && task.problem.is_none()
                    })
                    .collect();
                (
                    match java.as_slice() {
                        [task] => Some((*task).clone()),
                        _ => None,
                    },
                    Vec::new(),
                )
            }
            None => (None, Vec::new()),
        };
        let generation = *self.kotlin_source_generations.get(&scope).unwrap_or(&0);
        if self
            .kotlin_diagnostic_reports
            .get(key)
            .is_some_and(|report| {
                !report
                    .environment_invalid
                    .load(std::sync::atomic::Ordering::Acquire)
                    && report.request.check_live().is_ok()
                    && Arc::ptr_eq(&report.model, &model)
                    && report.request.generation == generation
            })
        {
            return Err("kotlin-diagnostics-current");
        }
        if let Some(previous) = self.kotlin_diagnostic_tickets.remove(&scope) {
            previous
                .request
                .invalid
                .store(true, std::sync::atomic::Ordering::Release);
        }
        let request = Arc::new(crate::compile::KotlinDiagnosticRequest {
            project_dir: module.project_dir.clone(),
            scope: scope.clone(),
            task: task.clone(),
            java_task,
            java_dependencies,
            java_editor_sources,
            processor_plan,
            positive_diagnostics_only: source.compiler_tasks.iter().any(|candidate| {
                candidate.language == CompilerLanguage::Kotlin && candidate.task != task.task
            }),
            source_roots: source.source_roots.clone(),
            sources: sources.into_values().collect(),
            overlays,
            dependency_overlays,
            visible_origins,
            generation,
            invalid: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        self.kotlin_diagnostic_tickets.insert(
            scope,
            KotlinDiagnosticTicket {
                request: request.clone(),
                model,
                analysis_generation: self.analysis_generation,
            },
        );
        Ok(request)
    }

    pub(crate) fn compiler_api_targets(&self) -> Vec<String> {
        let mut targets = BTreeMap::new();
        let mut visited = BTreeSet::new();
        if let Some(model) = &self.project_model {
            for key in self
                .project_file_meta
                .keys()
                .filter(|key| key.ends_with(".kt"))
            {
                let Some((_, Some(source))) = model.owner_for_key(key) else {
                    continue;
                };
                let Some(scope) = self.project_scope_for_file(key) else {
                    continue;
                };
                if !visited.insert(scope.clone()) {
                    continue;
                }
                if source
                    .compiler_tasks
                    .iter()
                    .any(crate::compile::compiler_api_supported)
                {
                    targets.entry(scope).or_insert_with(|| key.clone());
                }
            }
        }
        targets.into_values().collect()
    }

    pub(crate) fn compiler_api_environment(
        &self,
        request: &crate::compile::KotlinDiagnosticRequest,
    ) -> Option<Arc<crate::compile::KotlinDiagnosticEnvironment>> {
        self.compiler_api_environments
            .get(&request.scope)
            .filter(|environment| environment.matches(request))
            .cloned()
    }

    pub(crate) fn accept_kotlin_compiler_api(
        &mut self,
        request: &Arc<crate::compile::KotlinDiagnosticRequest>,
        environment: Arc<crate::compile::KotlinDiagnosticEnvironment>,
        snapshot: crate::compile::CompilerApiSnapshot,
    ) -> Result<(), String> {
        if request.check_live().is_err()
            || environment
                .invalid
                .load(std::sync::atomic::Ordering::Acquire)
            || !self.kotlin_overlay_matches(request)
            || !self
                .kotlin_diagnostic_tickets
                .get(&request.scope)
                .is_some_and(|ticket| {
                    Arc::ptr_eq(&ticket.request, request)
                        && ticket.analysis_generation == self.analysis_generation
                        && self
                            .project_model
                            .as_ref()
                            .is_some_and(|model| Arc::ptr_eq(model, &ticket.model))
                })
        {
            return Err("compiler-api-inputs-superseded".into());
        }
        if self
            .compiler_api_environments
            .get(&request.scope)
            .is_some_and(|previous| !Arc::ptr_eq(previous, &environment))
        {
            self.analysis_extensions.clear_compiler_api(&self.index);
        }
        self.analysis_extensions
            .accept_compiler_api(&self.index, snapshot)?;
        self.compiler_api_environments
            .insert(request.scope.clone(), environment);
        if self.analysis_extensions.publish(&mut self.index) {
            self.index_revision = self.index_revision.wrapping_add(1).max(1);
        }
        Ok(())
    }

    fn kotlin_overlay_matches(&self, request: &crate::compile::KotlinDiagnosticRequest) -> bool {
        let Some(model) = &self.project_model else {
            return false;
        };
        let selected: BTreeSet<_> = request
            .task
            .source_files
            .iter()
            .map(|path| crate::paths::file_key(path))
            .collect();
        let mut actual = BTreeSet::new();
        for (key, doc) in &self.open_docs {
            let origin = model.file_origin(Path::new(key));
            let own = (selected.contains(key)
                || origin
                    .as_ref()
                    .is_some_and(|origin| origin.project_scope.as_ref() == Some(&request.scope)))
                && (key.ends_with(".kt") || key.ends_with(".java"));
            if !own
                && !request
                    .processor_plan
                    .as_ref()
                    .is_some_and(|plan| plan.affects_processor_source(Path::new(key)))
                && origin
                    .as_ref()
                    .is_none_or(|origin| !request.visible_origins.contains(&origin.origin_id))
            {
                continue;
            }
            let captured = if own {
                &request.overlays
            } else {
                &request.dependency_overlays
            };
            let Some(source) = captured.iter().find(|source| &source.key == key) else {
                return false;
            };
            // JVM declaration revisions can remain equal after a body-only edit; exact text matters.
            let same_identity = match &source.identity {
                ktcore::jvm::InputIdentity::Source { file, revision } => {
                    file == key
                        && self
                            .index
                            .jvm_source_for_file(key)
                            .is_none_or(|current| current.revision == *revision)
                }
                _ => false,
            };
            if !same_identity || doc.text != source.text {
                return false;
            }
            actual.insert(key.clone());
        }
        actual.len() == request.overlays.len() + request.dependency_overlays.len()
    }

    pub(crate) fn finish_kotlin_diagnostics(
        &mut self,
        request: &Arc<crate::compile::KotlinDiagnosticRequest>,
        outcome: crate::compile::KotlinDiagnosticOutcome,
        environment_invalid: Arc<std::sync::atomic::AtomicBool>,
    ) -> Vec<String> {
        let current = self
            .kotlin_diagnostic_tickets
            .get(&request.scope)
            .is_some_and(|ticket| {
                Arc::ptr_eq(&ticket.request, request)
                    && self.analysis_generation == ticket.analysis_generation
                    && self
                        .project_model
                        .as_ref()
                        .is_some_and(|model| Arc::ptr_eq(model, &ticket.model))
                    && !self.project_refresh_pending
                    && request.generation
                        == *self
                            .kotlin_source_generations
                            .get(&request.scope)
                            .unwrap_or(&0)
            });
        if !current
            || request.check_live().is_err()
            || environment_invalid.load(std::sync::atomic::Ordering::Acquire)
            || !self.kotlin_overlay_matches(request)
        {
            return Vec::new();
        }
        let ticket = self
            .kotlin_diagnostic_tickets
            .remove(&request.scope)
            .unwrap();
        let crate::compile::KotlinDiagnosticOutcome::Verified { diagnostics, clean } = outcome
        else {
            // A failure is not an empty successful compilation and cannot replace accepted reports.
            return Vec::new();
        };
        // A Kotlin clean result never gives Java clean authority, or auxiliary producer closure.
        let kotlin_clean = clean && !request.positive_diagnostics_only;
        let mut by_file: BTreeMap<String, Vec<crate::diagnostics::Diagnostic>> = BTreeMap::new();
        for diagnostic in diagnostics {
            if clean && request.positive_diagnostics_only && diagnostic.path.ends_with(".kt") {
                continue;
            }
            let Some(source) = request.overlays.iter().find(|source| {
                source.key == diagnostic.path
                    && (source.key.ends_with(".kt")
                        || (source.key.ends_with(".java") && request.java_task.is_some()))
            }) else {
                continue;
            };
            let index = crate::text::LineIndex::new(&source.text);
            let Some(line) = diagnostic.line.checked_sub(1) else {
                return Vec::new();
            };
            let Some(column) = diagnostic.col.checked_sub(1) else {
                return Vec::new();
            };
            let start = index.offset(&source.text, line, column);
            if index.position(&source.text, start) != (line, column) {
                return Vec::new();
            }
            let tail = &source.text[start..];
            let mut length = 0;
            for ch in tail
                .chars()
                .take_while(|ch| ch.is_alphanumeric() || *ch == '_' || *ch == '$')
            {
                length += ch.len_utf8();
            }
            if length == 0 {
                length = tail.chars().next().map_or(0, char::len_utf8);
            }
            by_file
                .entry(source.key.clone())
                .or_default()
                .push(crate::diagnostics::Diagnostic {
                    start_byte: start,
                    end_byte: start + length,
                    severity: diagnostic.severity,
                    code: None,
                    message: diagnostic.message,
                });
        }
        let keys: Vec<_> = request
            .overlays
            .iter()
            .filter(|source| {
                by_file.contains_key(&source.key) || (source.key.ends_with(".kt") && kotlin_clean)
            })
            .map(|source| source.key.clone())
            .collect();
        self.kotlin_diagnostic_store_epoch = self.kotlin_diagnostic_store_epoch.saturating_add(1);
        for key in &keys {
            self.kotlin_diagnostic_reports.insert(
                key.clone(),
                KotlinFileDiagnostics {
                    request: request.clone(),
                    model: ticket.model.clone(),
                    environment_invalid: environment_invalid.clone(),
                    diagnostics: by_file.remove(key).unwrap_or_default(),
                    store_epoch: self.kotlin_diagnostic_store_epoch,
                    clean: clean && key.ends_with(".kt"),
                },
            );
        }
        keys
    }

    fn current_kotlin_report(&self, key: &str) -> Option<&KotlinFileDiagnostics> {
        let report = self.kotlin_diagnostic_reports.get(key)?;
        if self.project_refresh_pending
            || report.request.check_live().is_err()
            || report
                .environment_invalid
                .load(std::sync::atomic::Ordering::Acquire)
            || !self
                .project_model
                .as_ref()
                .is_some_and(|model| Arc::ptr_eq(model, &report.model))
            || !self.open_docs.get(key).is_some_and(|doc| {
                report
                    .request
                    .overlays
                    .iter()
                    .any(|source| source.key == key && doc.text == source.text)
            })
        {
            return None;
        }
        Some(report)
    }

    fn current_kotlin_diagnostics(&self, key: &str) -> Vec<crate::diagnostics::Diagnostic> {
        self.current_kotlin_report(key)
            .map_or_else(Vec::new, |report| report.diagnostics.clone())
    }

    pub(crate) fn kotlin_diagnostic_result_id(&self, key: &str) -> Option<String> {
        let report = self.current_kotlin_report(key)?;
        Some(format!(
            "kotlin:{}:{}:{}:{}",
            self.analysis_generation,
            report.request.generation,
            report.store_epoch,
            self.index_revision
        ))
    }

    pub(crate) fn kotlin_diagnostic_trace(&self, key: &str) -> Option<serde_json::Value> {
        let report = self.current_kotlin_report(key)?;
        let source = report
            .request
            .overlays
            .iter()
            .find(|source| source.key == key)?;
        let source_hash: String = source
            .text_sha256
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Some(serde_json::json!({
            "path": key, "result_id": self.kotlin_diagnostic_result_id(key),
            "store_epoch": report.store_epoch, "source_generation": report.request.generation,
            "source_sha256": source_hash, "count": report.diagnostics.len(), "clean": report.clean,
            "compiler_version": report.request.task.kotlin_compiler.as_ref().map(|recipe| &recipe.compiler_version),
        }))
    }
}

#[cfg(test)]
mod kotlin_diagnostic_lifecycle_tests {
    use super::*;
    use crate::compile::{CompileDiagnostic, KotlinDiagnosticOutcome};
    use std::sync::atomic::{AtomicBool, Ordering};

    fn fixture() -> (Workspace, String, String, String) {
        use crate::classpath::{ModuleClasspath, ProjectSnapshot, SourceSetClasspath};
        let root = std::env::temp_dir().join("ktlsp-kotlin-diagnostic-host-only");
        let task = crate::compile::kotlin_test_request(&root).task;
        let source = SourceSetClasspath {
            name: "main".into(),
            source_roots: vec![root.join("src")],
            kotlin_compile_task: Some(task.task.clone()),
            compiler_tasks: vec![task],
            ..Default::default()
        };
        let model = Arc::new(ProjectSnapshot::new(vec![ModuleClasspath {
            module: ":".into(),
            project_dir: root.clone(),
            build: ":".into(),
            source_sets: vec![
                source,
                SourceSetClasspath {
                    name: "test".into(),
                    source_roots: vec![root.join("test")],
                    ..Default::default()
                },
            ],
            entries: Vec::new(),
            unresolved: Vec::new(),
        }]));
        let mut ws = Workspace::new();
        ws.set_project_model(model);
        let helper = crate::paths::file_key(&root.join("src/Helper.kt"));
        let java = crate::paths::file_key(&root.join("src/Main.java"));
        let editor = crate::paths::file_key(&root.join("src/Editor.kt"));
        ws.open(
            helper.clone(),
            "class Helper { fun value(): Int = 1 }".into(),
        );
        ws.open(java.clone(), "class Main { String unsaved; }".into());
        ws.open(
            editor.clone(),
            "val emoji = \"😀\"; fun use() = Versioned.oldApi()".into(),
        );
        (ws, editor, helper, java)
    }

    fn reported(
        request: &Arc<crate::compile::KotlinDiagnosticRequest>,
        key: &str,
        token: &str,
    ) -> KotlinDiagnosticOutcome {
        let source = request
            .overlays
            .iter()
            .find(|source| source.key == key)
            .unwrap();
        let (line, column) = crate::text::LineIndex::new(&source.text)
            .position(&source.text, source.text.find(token).unwrap());
        KotlinDiagnosticOutcome::Verified {
            clean: false,
            diagnostics: vec![CompileDiagnostic {
                path: key.into(),
                line: line + 1,
                col: column + 1,
                severity: crate::diagnostics::Severity::Error,
                message: format!("Unresolved reference '{token}'."),
            }],
        }
    }

    #[test]
    fn primary_compilation_selection_ignores_auxiliary_order_and_rejects_unknown_identity() {
        let (mut ws, editor, _, _) = fixture();
        let mut modules = request_model(&ws);
        let source = &mut modules[0].source_sets[0];
        let mut selected = source.compiler_tasks[0].clone();
        selected.task = ":app:customPrimaryJvmTask".into();
        let mut auxiliary = selected.clone();
        auxiliary.task = ":app:auxiliaryProducer".into();
        auxiliary.arguments = vec!["-P".into(), "opaque-producer-input=private-path".into()];
        source.kotlin_compile_task = Some(selected.task.clone());
        source.compiler_tasks = vec![auxiliary, selected.clone()];
        ws.set_project_model(Arc::new(crate::classpath::ProjectSnapshot::new(modules)));
        let request = ws.begin_kotlin_diagnostics(&editor).unwrap();
        assert_eq!(request.task, selected);
        assert!(request.positive_diagnostics_only);
        assert!(!request
            .task
            .arguments
            .iter()
            .any(|argument| argument == "-P"));

        for mutation in [
            "missing",
            "unlisted",
            "duplicate",
            "selected-incomplete",
            "metadata-incomplete",
        ] {
            let (mut ws, editor, _, _) = fixture();
            let mut modules = request_model(&ws);
            let source = &mut modules[0].source_sets[0];
            match mutation {
                "missing" => source.kotlin_compile_task = None,
                "unlisted" => source.kotlin_compile_task = Some(":app:uncapturedPrimary".into()),
                "duplicate" => source.compiler_tasks.push(source.compiler_tasks[0].clone()),
                "selected-incomplete" => source.compiler_tasks[0].complete = false,
                "metadata-incomplete" => source.compiler_metadata_incomplete = true,
                _ => unreachable!(),
            }
            ws.set_project_model(Arc::new(crate::classpath::ProjectSnapshot::new(modules)));
            assert!(ws.begin_kotlin_diagnostics(&editor).is_err(), "{mutation}");
            assert!(
                ws.kotlin_diagnostic_result_id(&editor).is_none(),
                "{mutation}"
            );
        }
    }

    #[test]
    fn producer_scope_accepts_located_errors_but_never_publishes_canonical_empty() {
        let (mut ws, editor, helper, _) = fixture();
        let mut modules = request_model(&ws);
        let source = &mut modules[0].source_sets[0];
        let mut producer = source.compiler_tasks[0].clone();
        producer.task = ":app:auxiliaryProducer".into();
        source.compiler_tasks.push(producer);
        ws.set_project_model(Arc::new(crate::classpath::ProjectSnapshot::new(modules)));
        let clean = ws.begin_kotlin_diagnostics(&editor).unwrap();
        assert!(clean.positive_diagnostics_only);
        assert!(ws
            .finish_kotlin_diagnostics(
                &clean,
                KotlinDiagnosticOutcome::Verified {
                    diagnostics: Vec::new(),
                    clean: true
                },
                Arc::new(AtomicBool::new(false)),
            )
            .is_empty());
        assert!(
            ws.kotlin_diagnostic_result_id(&editor).is_none(),
            "exit 0 does not certify producer closure"
        );
        let error = ws.begin_kotlin_diagnostics(&editor).unwrap();
        let outcome = reported(&error, &editor, "oldApi");
        assert_eq!(
            ws.finish_kotlin_diagnostics(&error, outcome, Arc::new(AtomicBool::new(false))),
            vec![editor.clone()]
        );
        let accepted = ws.kotlin_diagnostic_result_id(&editor).unwrap();
        let clean = ws.begin_kotlin_diagnostics(&helper).unwrap();
        assert!(ws
            .finish_kotlin_diagnostics(
                &clean,
                KotlinDiagnosticOutcome::Verified {
                    diagnostics: Vec::new(),
                    clean: true
                },
                Arc::new(AtomicBool::new(false)),
            )
            .is_empty());
        assert_eq!(ws.kotlin_diagnostic_result_id(&editor).unwrap(), accepted);
        assert_eq!(ws.current_kotlin_diagnostics(&editor).len(), 1);
        assert!(ws.kotlin_diagnostic_result_id(&helper).is_none());
    }

    #[test]
    fn selected_generated_wire_outside_source_set_roots_is_captured_and_checked_before_publish() {
        for change in [
            "body-without-notification",
            "disk-event",
            "external-disk-event",
            "close",
        ] {
            let (mut ws, editor, _, _) = fixture();
            let mut modules = request_model(&ws);
            let generated = if change == "external-disk-event" {
                modules[0]
                    .project_dir
                    .parent()
                    .unwrap()
                    .join("ktlsp-kotlin-diagnostic-external/Generated.kt")
            } else {
                modules[0]
                    .project_dir
                    .join("build/generated/source/kapt/main/Generated.kt")
            };
            let generated_key = crate::paths::file_key(&generated);
            let task = &mut modules[0].source_sets[0].compiler_tasks[0];
            task.source_files.push(generated.clone());
            task.declared_input_files.push(generated.clone());
            ws.set_project_model(Arc::new(crate::classpath::ProjectSnapshot::new(modules)));
            assert!(ws.project_scope_for_file(&generated_key).is_none());
            ws.open(
                generated_key.clone(),
                "class Generated { val unsaved = 1 }".into(),
            );
            let request = ws.begin_kotlin_diagnostics(&editor).unwrap();
            assert!(!request
                .source_roots
                .iter()
                .any(|root| generated.starts_with(root)));
            assert!(request
                .overlays
                .iter()
                .any(|source| source.key == generated_key
                    && source.text == "class Generated { val unsaved = 1 }"));
            assert!(ws.kotlin_overlay_matches(&request));
            let outcome = reported(&request, &editor, "oldApi");
            match change {
                "body-without-notification" => ws
                    .open_docs
                    .get_mut(&generated_key)
                    .unwrap()
                    .text
                    .push_str(" // changed after capture"),
                "disk-event" | "external-disk-event" => {
                    ws.observe_compiled_resource(&generated_key);
                    assert!(request.invalid.load(Ordering::Acquire));
                }
                "close" => ws.close(&generated_key),
                _ => unreachable!(),
            }
            assert!(
                ws.finish_kotlin_diagnostics(&request, outcome, Arc::new(AtomicBool::new(false)))
                    .is_empty(),
                "{change}"
            );
            assert!(
                ws.kotlin_diagnostic_result_id(&editor).is_none(),
                "{change}"
            );
        }
    }

    #[test]
    fn new_owned_buffer_and_unsaved_java_kotlin_peers_share_one_selected_request() {
        let (mut ws, editor, helper, java) = fixture();
        let test = editor.replace("/src/Editor.kt", "/test/Excluded.kt");
        ws.open(test.clone(), "class Excluded".into());
        let request = ws.begin_kotlin_diagnostics(&editor).unwrap();
        assert!(!request
            .task
            .source_files
            .iter()
            .any(|path| crate::paths::file_key(path) == editor));
        assert!(
            request
                .sources
                .iter()
                .any(|path| crate::paths::file_key(path) == editor),
            "owned new buffer policy"
        );
        assert_eq!(
            request
                .overlays
                .iter()
                .map(|source| source.key.clone())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([editor.clone(), helper, java])
        );
        assert!(!request
            .sources
            .iter()
            .any(|path| crate::paths::file_key(path) == test));
        let outcome = reported(&request, &editor, "oldApi");
        assert_eq!(
            ws.finish_kotlin_diagnostics(&request, outcome, Arc::new(AtomicBool::new(false))),
            vec![editor.clone()]
        );
        let diagnostic = ws.current_kotlin_diagnostics(&editor).pop().unwrap();
        let source = request
            .overlays
            .iter()
            .find(|source| source.key == editor)
            .unwrap();
        assert_eq!(
            &source.text[diagnostic.start_byte..diagnostic.end_byte],
            "oldApi"
        );
        assert!(ws
            .diagnostics(&editor)
            .iter()
            .any(|diagnostic| diagnostic.message.contains("oldApi")));
        assert!(ws.kotlin_diagnostic_result_id(&editor).is_some());
        let proof = ws.kotlin_diagnostic_trace(&editor).unwrap();
        assert_eq!(proof["source_generation"], request.generation);
        assert_eq!(proof["source_sha256"].as_str().unwrap().len(), 64);
        assert!(proof.get("message").is_none() && proof.get("text").is_none());
    }

    #[test]
    fn source_aba_model_environment_and_membership_changes_reject_late_publication() {
        for mutation in [
            "body",
            "source-aba",
            "disk-event-aba",
            "environment",
            "model",
            "new-buffer",
            "close",
            "analysis",
        ] {
            let (mut ws, editor, helper, _) = fixture();
            let request = ws.begin_kotlin_diagnostics(&editor).unwrap();
            let invalid = Arc::new(AtomicBool::new(false));
            let outcome = reported(&request, &editor, "oldApi");
            match mutation {
                // Deliberately bypass change hooks/index revisions to exercise exact-text CAS itself.
                "body" => ws
                    .open_docs
                    .get_mut(&helper)
                    .unwrap()
                    .text
                    .push_str(" // body-only"),
                "source-aba" => {
                    let original = ws.doc_text(&helper).unwrap();
                    ws.change(&helper, format!("{original} // B"));
                    ws.change(&helper, original);
                    assert!(request.invalid.load(Ordering::Acquire));
                    assert!(
                        *ws.kotlin_source_generations.get(&request.scope).unwrap()
                            > request.generation
                    );
                }
                "disk-event-aba" => {
                    ws.observe_compiled_resource(&helper);
                    ws.observe_compiled_resource(&helper);
                }
                "environment" => invalid.store(true, Ordering::Release),
                "model" => ws.set_project_model(Arc::new(crate::classpath::ProjectSnapshot::new(
                    request_model(&ws),
                ))),
                "new-buffer" => ws.open(
                    editor.replace("Editor.kt", "Later.kt"),
                    "class Later".into(),
                ),
                "close" => ws.close(&helper),
                "analysis" => {
                    ws.begin_analysis_generation();
                }
                _ => unreachable!(),
            }
            assert!(
                ws.finish_kotlin_diagnostics(&request, outcome, invalid)
                    .is_empty(),
                "{mutation}"
            );
            assert!(
                ws.current_kotlin_diagnostics(&editor).is_empty(),
                "{mutation}"
            );
            assert!(
                ws.kotlin_diagnostic_result_id(&editor).is_none(),
                "{mutation}"
            );
        }
    }

    fn request_model(ws: &Workspace) -> Vec<crate::classpath::ModuleClasspath> {
        ws.project_model.as_ref().unwrap().modules.clone()
    }

    #[test]
    fn external_selected_source_invalidates_distinct_old_reports_and_current_ticket_once() {
        let (mut ws, editor, helper, _) = fixture();
        let current = ws.begin_kotlin_diagnostics(&editor).unwrap();
        let external = crate::paths::file_key(&current.project_dir.join("external/Selected.kt"));
        assert!(ws
            .project_model
            .as_ref()
            .unwrap()
            .file_origin(Path::new(&external))
            .is_none());
        let mut older = crate::compile::kotlin_test_request(&current.project_dir);
        older.scope = current.scope.clone();
        older.sources = current.sources.clone();
        older.sources.push(std::path::PathBuf::from(&external));
        older.overlays = current.overlays.clone();
        older.generation = current.generation;
        let older = Arc::new(older);
        let model = ws.project_model.clone().unwrap();
        // Several accepted file reports can share an older request with a different source list.
        for key in [editor, helper] {
            ws.kotlin_diagnostic_reports.insert(
                key,
                KotlinFileDiagnostics {
                    request: older.clone(),
                    model: model.clone(),
                    environment_invalid: Arc::new(AtomicBool::new(false)),
                    diagnostics: Vec::new(),
                    store_epoch: 1,
                    clean: true,
                },
            );
        }
        ws.invalidate_kotlin_source(&external);
        assert!(
            older.invalid.load(Ordering::Acquire),
            "fallback protects externally selected old sources"
        );
        assert!(
            current.invalid.load(Ordering::Acquire),
            "all request generations in the affected scope retire"
        );
        assert!(ws.kotlin_diagnostic_reports.is_empty());
        assert!(!ws.kotlin_diagnostic_tickets.contains_key(&current.scope));
        assert_eq!(
            *ws.kotlin_source_generations.get(&current.scope).unwrap(),
            current.generation + 1
        );
    }

    #[test]
    fn failure_and_partial_error_result_cannot_clear_an_accepted_same_epoch_error() {
        let (mut ws, editor, helper, java) = fixture();
        let request = ws.begin_kotlin_diagnostics(&editor).unwrap();
        let outcome = reported(&request, &editor, "oldApi");
        ws.finish_kotlin_diagnostics(&request, outcome, Arc::new(AtomicBool::new(false)));
        let accepted = ws.kotlin_diagnostic_result_id(&editor).unwrap();
        let retry = ws.begin_kotlin_diagnostics(&helper).unwrap();
        assert!(ws
            .finish_kotlin_diagnostics(
                &retry,
                KotlinDiagnosticOutcome::Unverified("internal failure".into()),
                Arc::new(AtomicBool::new(false))
            )
            .is_empty());
        assert_eq!(ws.kotlin_diagnostic_result_id(&editor).unwrap(), accepted);
        assert_eq!(ws.current_kotlin_diagnostics(&editor).len(), 1);
        let partial = ws.begin_kotlin_diagnostics(&helper).unwrap();
        let outcome = reported(&partial, &helper, "value");
        ws.finish_kotlin_diagnostics(&partial, outcome, Arc::new(AtomicBool::new(false)));
        assert_eq!(
            ws.kotlin_diagnostic_result_id(&editor).unwrap(),
            accepted,
            "exit 1 only replaces files actually reported"
        );
        let clean = ws.begin_kotlin_diagnostics(&java).unwrap();
        ws.finish_kotlin_diagnostics(
            &clean,
            KotlinDiagnosticOutcome::Verified {
                diagnostics: Vec::new(),
                clean: true,
            },
            Arc::new(AtomicBool::new(false)),
        );
        assert!(ws.current_kotlin_diagnostics(&editor).is_empty());
        assert_ne!(
            ws.kotlin_diagnostic_result_id(&editor).unwrap(),
            accepted,
            "exit 0 advances the canonical accepted store"
        );
    }
}

#[cfg(test)]
mod java_source_authority_tests {
    use super::*;

    fn fixture(
        generated: bool,
        extension: &str,
        source: &str,
    ) -> (
        Workspace,
        Arc<crate::classpath::ProjectSnapshot>,
        String,
        String,
    ) {
        use crate::classpath::{
            CompilerTask, ModuleClasspath, ProjectSnapshot, SourceSetClasspath,
        };
        let root = std::env::temp_dir().join(format!(
            "ktlsp-plain-java-source-authority-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        let source_root = root.join(if generated {
            "build/generated/java/main"
        } else {
            "src/main/java"
        });
        let owner = crate::paths::file_key(&source_root.join(format!("Current.{extension}")));
        let caller = crate::paths::file_key(&source_root.join("Use.java"));
        let java: CompilerTask = serde_json::from_value(serde_json::json!({
            "task": ":compileJava", "language": "java",
            "task_type": "org.gradle.api.tasks.compile.JavaCompile_Decorated",
            "arguments": ["-proc:none"], "annotation_processor_path": [],
            "source_files": [&owner, &caller], "classpath": [],
            "java_home": root.join("jdk"),
            "options": {"fork": false, "forkJavaHome": null, "forkExecutable": null, "forkJvmArgs": [], "release": null},
            "output_roots": [], "complete": true, "problem": null
        })).unwrap();
        let kotlin: CompilerTask = serde_json::from_value(serde_json::json!({
            "task": ":compileKotlin", "language": "kotlin", "task_type": "KotlinCompile",
            "arguments": ["-Xplugin=/plugins/transform.jar"], "annotation_processor_path": [],
            "java_home": null, "options": {}, "output_roots": [], "complete": true, "problem": null
        }))
        .unwrap();
        let model = Arc::new(ProjectSnapshot::new(vec![ModuleClasspath {
            module: ":".into(),
            project_dir: root,
            build: String::new(),
            entries: Vec::new(),
            unresolved: Vec::new(),
            source_sets: vec![SourceSetClasspath {
                name: "main".into(),
                source_roots: vec![source_root],
                compiler_tasks: vec![java, kotlin],
                ..Default::default()
            }],
        }]));
        let mut ws = Workspace::new();
        ws.set_project_model(model.clone());
        ws.project_scan_initialized = true;
        ws.open(&owner, source.into());
        ws.open(&caller, "package app; class Use { void run(Current value) { value.current(); value.removed(); } }".into());
        (ws, model, owner, caller)
    }

    fn proof(
        ws: &Workspace,
        model: &crate::classpath::ProjectSnapshot,
        owner: &str,
        caller: &str,
    ) -> bool {
        let Some(task) = model.plain_java_task(caller) else {
            return false;
        };
        let mut entries = ws
            .index
            .lookup_type("Current")
            .into_iter()
            .filter(|entry| entry.path.as_ref() == owner);
        let Some(entry) = entries.next() else {
            return false;
        };
        entries.next().is_none()
            && ws.plain_java_source_members_complete(model, task, caller, entry)
    }

    #[test]
    fn current_java_owner_authority_is_independent_of_kotlin_tasks_and_generated_origin() {
        let source = "package app; public class Current { public int current() { return 2; } }";
        for generated in [false, true] {
            let (ws, model, owner, caller) = fixture(generated, "java", source);
            assert!(proof(&ws, &model, &owner, &caller));
            assert!(!ws.effective_completeness().compiler_producers_complete);
            assert_eq!(model.modules[0].source_sets[0].compiler_tasks.len(), 2);
        }
    }

    #[test]
    fn current_java_owner_authority_retracts_with_unsaved_shape_source_and_model_uncertainty() {
        let source = "package app; public class Current { public int current() { return 2; } }";
        let (mut ws, model, owner, caller) = fixture(false, "java", source);
        assert!(proof(&ws, &model, &owner, &caller));
        for changed in [
            "package app; public class Current {",
            "package app; @Unknown public class Current { public int current() { return 2; } }",
            "package app; public record Current(int component) {}",
            "package app; public class Current<T> {}",
            "package app; public class Current extends Missing {}",
            "package app; public class Current { class Nested {} }",
            "package app; public class Current {} class Other {}",
        ] {
            ws.change(&owner, changed.into());
            assert!(!proof(&ws, &model, &owner, &caller), "{changed}");
            ws.change(&owner, source.into());
            assert!(
                proof(&ws, &model, &owner, &caller),
                "restoration after {changed}"
            );
        }
        ws.project_refresh_pending = true;
        assert!(!proof(&ws, &model, &owner, &caller));
        ws.project_refresh_pending = false;
        ws.annotated_type_owners.remove(&owner);
        assert!(!proof(&ws, &model, &owner, &caller));
        ws.change(&owner, source.into());
        assert!(proof(&ws, &model, &owner, &caller));
        ws.project_file_meta.get_mut(&owner).unwrap().scope = None;
        assert!(!proof(&ws, &model, &owner, &caller));
        ws.change(&owner, source.into());
        assert!(proof(&ws, &model, &owner, &caller));
        for gap in ["inventory", "jdk", "opaque"] {
            let (mut ws, model, owner, caller) = fixture(false, "java", source);
            let mut modules = model.modules.clone();
            let java = &mut modules[0].source_sets[0].compiler_tasks[0];
            match gap {
                "inventory" => java.source_files = vec![caller.clone().into()],
                "jdk" => java.java_home = None,
                "opaque" => java.arguments.push("-Xplugin:transform".into()),
                _ => unreachable!(),
            }
            let changed = Arc::new(crate::classpath::ProjectSnapshot::new(modules));
            ws.set_project_model(changed.clone());
            assert!(!proof(&ws, &changed, &owner, &caller), "{gap}");
        }
        ws.open_docs.remove(&owner);
        assert!(!proof(&ws, &model, &owner, &caller));
        let (ws, model, owner, caller) = fixture(
            false,
            "kt",
            "package app\nclass Current { fun current() = 2 }",
        );
        assert!(!proof(&ws, &model, &owner, &caller));
    }

    #[test]
    fn closed_java_owner_authority_requires_fresh_matching_declarations_and_restores_after_edits() {
        let source = "package app; public class Current { public int current() { return 2; } }";
        let (mut ws, model, owner, caller) = fixture(false, "java", source);
        std::fs::create_dir_all(Path::new(&owner).parent().unwrap()).unwrap();
        std::fs::write(&owner, source).unwrap();
        ws.source_stamps
            .insert(owner.clone(), source_stamp(Path::new(&owner)).unwrap());
        ws.close(&owner);
        assert!(
            !ws.open_docs.contains_key(&owner),
            "the target oracle stays closed"
        );
        assert!(proof(&ws, &model, &owner, &caller));

        let changed = "package app; public class Current { public int removed() { return 2; } }";
        std::fs::write(&owner, changed).unwrap();
        // Even a misleading matching timestamp cannot certify an old declaration inventory.
        ws.source_stamps
            .insert(owner.clone(), source_stamp(Path::new(&owner)).unwrap());
        assert!(!proof(&ws, &model, &owner, &caller));
        ws.reindex(&owner, changed);
        assert!(proof(&ws, &model, &owner, &caller));
        std::fs::write(&owner, source).unwrap();
        ws.source_stamps
            .insert(owner.clone(), source_stamp(Path::new(&owner)).unwrap());
        assert!(!proof(&ws, &model, &owner, &caller));
        ws.reindex(&owner, source);
        assert!(proof(&ws, &model, &owner, &caller));

        ws.open(&owner, changed.into());
        assert!(
            proof(&ws, &model, &owner, &caller),
            "the unsaved declaration wins over disk"
        );
        ws.close(&owner);
        assert!(
            proof(&ws, &model, &owner, &caller),
            "closing restores the disk declaration"
        );
        ws.source_stamps.remove(&owner);
        assert!(!proof(&ws, &model, &owner, &caller));
        ws.source_stamps
            .insert(owner.clone(), source_stamp(Path::new(&owner)).unwrap());
        std::fs::remove_file(&owner).unwrap();
        assert!(!proof(&ws, &model, &owner, &caller));
        std::fs::remove_dir_all(&model.modules[0].project_dir).unwrap();
    }
}

#[cfg(test)]
mod kotlin_property_value_tests {
    use super::*;
    use ktcore::extensions::source::JvmPropertyAccess;

    #[test]
    fn source_jvm_property_proof_tracks_edits_pending_and_plain_profile() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp-property-value-proof-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut ws = Workspace::new();
        assert_eq!(ws.scan(&root), 0);
        let parent = crate::paths::file_key(&root.join("Parent.kt"));
        let caller = crate::paths::file_key(&root.join("Use.java"));
        ws.open(
            &parent,
            "package sample\nopen class Parent { public val key: Int = 1 }\n".into(),
        );
        ws.open(
            &caller,
            "package sample; class Use extends Parent { void use() {} }".into(),
        );
        let owner = ws
            .index
            .lookup_type("Parent")
            .into_iter()
            .find(|entry| entry.path.as_ref() == parent)
            .unwrap()
            .clone();
        let property = ws
            .index
            .members_of("Parent")
            .into_iter()
            .find(|entry| entry.sym.name == "key")
            .unwrap()
            .clone();
        ws.with_java_member_proofs(&caller, |index| {
            assert!(index.source_value_namespace_complete(&owner));
            assert!(
                !index.source_members_complete(&owner),
                "value proof must not invent a generated getter inventory"
            );
            assert_eq!(
                index.source_property_access(&property),
                Some(JvmPropertyAccess::GetterOnly)
            );
        });
        ws.begin_analysis_generation();
        ws.with_java_member_proofs(&caller, |index| {
            assert!(
                !index.source_value_namespace_complete(&owner),
                "pending producer proof is still withdrawn"
            );
            assert_eq!(
                index.source_property_access(&property),
                Some(JvmPropertyAccess::GetterOnly)
            );
        });
        ws.change(
            &parent,
            "package sample\nopen class Parent { @kotlin.jvm.JvmField public val key: Int = 1 }\n"
                .into(),
        );
        let property = ws
            .index
            .members_of("Parent")
            .into_iter()
            .find(|entry| entry.sym.name == "key")
            .unwrap()
            .clone();
        ws.with_java_member_proofs(&caller, |index| {
            assert!(!index.source_value_namespace_complete(&owner));
            assert_eq!(
                index.source_property_access(&property),
                Some(JvmPropertyAccess::Field),
                "unsaved lowering replacement cannot reuse getter-only facts"
            );
        });
        ws.scan(&root);
        ws.with_java_member_proofs(&caller, |index| {
            assert!(index.source_value_namespace_complete(&owner));
            assert!(!index.source_members_complete(&owner));
        });
        std::fs::write(root.join("build.gradle.kts"), "plugins {}\n").unwrap();
        ws.with_java_member_proofs(&caller, |index| {
            assert!(!index.source_value_namespace_complete(&owner));
            assert_eq!(
                index.source_property_access(&property),
                None,
                "a build/compiler profile needs its own transformation proof"
            );
        });
        std::fs::remove_dir_all(root).unwrap();
    }
}
