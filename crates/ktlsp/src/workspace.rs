//! Owns the cross-file index + open (dirty) document text + a parser, and exposes the operations
//! the LSP layer drives. All keys are the caller's canonical identity string (a path or URI
//! string); we never re-derive identity from the filesystem at query time.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
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
}

fn is_java_path(key: &str) -> bool {
    SourceLanguage::for_key(key).is_java()
}

pub struct Workspace {
    pub index: Index,
    /// Open buffers, keyed by canonical identity. Take precedence over disk.
    open_docs: HashMap<String, DocState>,
    parsers: LanguageParsers,
    salsa: SalsaState,
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
    annotated_type_owners: HashMap<String, BTreeSet<Def>>,
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
            index: Index::new(),
            open_docs: HashMap::new(),
            parsers: LanguageParsers::new(),
            salsa: SalsaState::new(),
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
            annotated_type_owners: HashMap::new(),
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
        self.index.replace_scope_visibilities(
            model.scope_visibilities(&self.global_origins.iter().cloned().collect::<Vec<_>>()),
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
        let changed_origins: BTreeSet<_> = previous_revisions
            .iter()
            .filter(|(origin, revision)| {
                active.contains(*origin) && revisions.get(*origin) != Some(*revision)
            })
            .map(|(origin, _)| origin.clone())
            .collect();
        self.live_dependency_origins = active.clone();
        let removed: Vec<_> = self
            .index
            .file_origins()
            .filter(|(key, origin)| {
                is_dependency_origin(&origin.origin_id)
                    && !origin.origin_id.starts_with("jdk:")
                    && (!active.contains(&origin.origin_id)
                        || changed_origins.contains(&origin.origin_id))
                    && !self.open_docs.contains_key(*key)
            })
            .map(|(key, _)| key.to_string())
            .collect();
        for key in removed {
            self.index.remove_file(&key);
            self.analysis_extensions.remove_source(&self.index, &key);
            self.annotated_type_owners.remove(&key);
            self.salsa.forget(&key);
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
                    package: batch.package,
                    clean: batch.clean,
                    scope: self.project_scope_for_file(&batch.key),
                },
            );
            self.index
                .replace_file(&batch.key, batch.symbols, Tier::Volatile);
            self.index.replace_file_refs(&batch.key, batch.usages);
            self.salsa.forget(&batch.key);
            self.record_annotated_owners(&batch.key, &batch.source.0, &batch.source.1);
            if !self.analysis_extensions.is_empty() {
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
                self.salsa.forget(key);
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
                    && !origin.origin_id.starts_with("jdk:")
                    && !self.live_dependency_origins.contains(&origin.origin_id)
            })
        {
            return false;
        }
        let mut metadata_changed = false;
        if let Some(model) = &self.project_model {
            if model.modules.iter().any(|m| !m.source_sets.is_empty()) {
                if let Some(origin) = batch.origin {
                    if origin.origin_id.starts_with("jdk:")
                        && self.global_origins.insert(origin.origin_id.clone())
                    {
                        metadata_changed |=
                            self.index
                                .replace_scope_visibilities(model.scope_visibilities(
                                    &self.global_origins.iter().cloned().collect::<Vec<_>>(),
                                ));
                    }
                    // Late ownership metadata also belongs to authoritative open source buffers.
                    metadata_changed |= self.index.set_file_origin(&batch.file, origin);
                }
            }
        }
        if metadata_changed {
            self.analysis_extensions.mark_index_changed();
        }
        self.index_dependency_file(&batch.file, batch.symbols)
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
        self.analysis_extensions.mark_index_changed();
        self.index.replace_file(key, symbols, Tier::Durable);
        true
    }

    pub(crate) fn remove_dependency_file(&mut self, key: &str) {
        if self.open_docs.contains_key(key) {
            return;
        }
        self.index.remove_file(key);
        self.analysis_extensions.remove_source(&self.index, key);
        self.analysis_extensions.mark_index_changed();
        self.annotated_type_owners.remove(key);
        self.salsa.forget(key);
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
        let revision = self.analysis_extensions.revision();
        let registered =
            self.analysis_extensions
                .configure(&self.index, provider, enabled, configuration);
        if self.analysis_extensions.revision() != revision {
            self.bump_index_revision();
        }
        registered
    }

    pub fn take_extension_diagnostic_changes(&mut self) -> Vec<String> {
        self.analysis_extensions.take_changed()
    }

    fn replace_extension_source(&mut self, key: &str, text: Arc<str>, tree: Tree) {
        self.record_annotated_owners(key, &text, &tree);
        if !self.analysis_extensions.is_empty() {
            self.analysis_extensions
                .replace_sources(&self.index, vec![(key.into(), text, tree)]);
            if self.analysis_extensions.publish(&mut self.index) {
                self.index_revision = self.index_revision.wrapping_add(1).max(1);
            }
        }
    }

    fn record_annotated_owners(&mut self, key: &str, text: &str, tree: &Tree) {
        self.annotated_type_owners.insert(
            key.into(),
            language::annotated_type_owners(&self.index, key, tree, text)
                .into_iter()
                .collect(),
        );
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

    /// Index a project file from an already-parsed tree: its declarations (volatile tier) and its
    /// identifier usages (reverse-reference index).
    fn index_from_tree(&mut self, key: &str, text: &str, tree: &Tree) {
        let facts = language::file_facts(SourceLanguage::for_key(key), tree, text);
        self.apply_file_facts(key, facts);
        self.replace_extension_source(key, Arc::from(text), tree.clone());
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
        self.index.replace_file_refs(key, facts.usages);
        self.bump_index_revision();
        self.record_project_file_meta(key, facts.package, facts.clean);
    }

    /// Parse `text` from scratch and (re)index the file. Used for non-open files (scan/close),
    /// where there is no cached tree to reuse.
    pub fn reindex(&mut self, key: &str, text: &str) {
        let facts = self.salsa.file_facts(key, text, Durability::LOW);
        self.apply_file_facts(key, facts);
        let tree = self.salsa.parsed_tree(key, text, Durability::LOW);
        self.replace_extension_source(key, Arc::from(text), tree);
    }

    /// `textDocument/didOpen`.
    pub fn open(&mut self, key: impl Into<String>, text: String) {
        let key = key.into();
        let tree = self.salsa.parsed_tree(&key, &text, Durability::LOW);
        let facts = self.salsa.file_facts(&key, &text, Durability::LOW);
        self.apply_file_facts(&key, facts);
        self.replace_extension_source(&key, Arc::from(text.as_str()), tree.clone());
        self.open_docs.insert(key, DocState { text, tree });
    }

    /// `textDocument/didChange` (FULL sync: `text` is the whole new document). Reparses
    /// incrementally by diffing against the cached buffer, then re-indexes from the new tree.
    pub fn change(&mut self, key: &str, text: String) {
        let tree = match self.open_docs.remove(key) {
            Some(mut old) => {
                let edit = compute_edit(&old.text, &text);
                old.tree.edit(&edit);
                self.reparse_text(key, &text, &old.tree)
            }
            None => self.parse_text(key, &text),
        };
        self.salsa.record_text(key, &text, Durability::LOW);
        self.index_from_tree(key, &text, &tree);
        self.open_docs
            .insert(key.to_string(), DocState { text, tree });
    }

    /// `textDocument/didClose`: drop the dirty buffer; re-sync the index from disk (or drop it).
    pub fn close(&mut self, key: &str) {
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
            self.salsa.forget(key);
            self.project_file_meta.remove(key);
            self.bump_index_revision();
            self.recompute_project_completeness();
            return;
        }
        match std::fs::read_to_string(key) {
            Ok(text) => self.reindex(key, &text),
            Err(_) => {
                self.index.remove_file(key);
                self.analysis_extensions.remove_source(&self.index, key);
                self.annotated_type_owners.remove(key);
                self.bump_index_revision();
                self.salsa.forget(key);
                if self.project_scan_initialized {
                    self.project_file_meta.remove(key);
                    self.recompute_project_completeness();
                }
            }
        }
    }

    fn parsed_closed_doc(&mut self, key: &str) -> Option<(String, Tree)> {
        let text = self.doc_text(key)?;
        let tree = self.salsa.parsed_tree(key, &text, Durability::LOW);
        Some((text, tree))
    }

    /// Index every project `.kt`/`.kts`/`.java` under `root`, skipping build output except Gradle
    /// generated source roots and any files currently open (their dirty buffers are authoritative).
    /// Returns the count indexed.
    pub fn scan(&mut self, root: &Path) -> usize {
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
            self.index.replace_file_refs(&batch.key, batch.usages);
            self.record_annotated_owners(&batch.key, &batch.source.0, &batch.source.1);
            if !self.analysis_extensions.is_empty() {
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
    /// current text of `key`). Open buffers use their cached tree (no parse on the hot path);
    /// a non-open file is read from disk and parsed once.
    pub fn goto_definition(&mut self, key: &str, offset: usize) -> Vec<Def> {
        if let Some(doc) = self.open_docs.get(key) {
            let defs = language::goto_definition(
                &self.index.scoped_for_file(key),
                SourceLanguage::for_key(key),
                key,
                &doc.text,
                &doc.tree,
                offset,
            );
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
        let defs = language::goto_definition(
            &self.index.scoped_for_file(key),
            language,
            key,
            &text,
            &tree,
            offset,
        );
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
        language::goto_definition(
            &self.index.scoped_for_file(key),
            SourceLanguage::for_key(key),
            key,
            text,
            tree,
            offset,
        )
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
        let ctx = infer::FileCtx::from_tree(tree, text);
        let receiver_type = self.salsa.expr_type(
            key,
            text,
            Durability::LOW,
            &self.index,
            self.index_revision,
            ExprKey::from_node(receiver),
        );
        let member_use = resolve::member_use(&self.index.scoped_for_file(key), ident, text, &ctx);
        let hits = resolve::member_defs_for_use(
            &self.index.scoped_for_file(key),
            &receiver_type,
            name,
            &member_use,
            &ctx,
        );
        if receiver_type.name().is_some() || !hits.is_empty() {
            return Some(hits);
        }
        let index = self.index.scoped_for_file(key);
        let candidates = index.lookup_by_name(name);
        if index.permits_unique_member_fallback() && candidates.len() == 1 {
            let entry = &candidates[0];
            return Some(vec![Def {
                file: entry.path.to_string(),
                start_byte: entry.sym.start_byte,
                end_byte: entry.sym.end_byte,
            }]);
        }
        Some(Vec::new())
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
        if let Some(doc) = self.open_docs.get(key) {
            return semantic_query::resolved_symbol_query(
                &self.index.scoped_for_file(key),
                key,
                &doc.tree,
                &doc.text,
                offset,
                &self.effective_completeness(),
            );
        }
        let (text, tree) = self.parsed_closed_doc(key)?;
        semantic_query::resolved_symbol_query(
            &self.index.scoped_for_file(key),
            key,
            &tree,
            &text,
            offset,
            &self.effective_completeness(),
        )
    }

    pub fn after_dot_query(
        &mut self,
        key: &str,
        offset: usize,
    ) -> Option<semantic_query::CompletionQuery> {
        if is_java_path(key) {
            return self
                .completion_query(key, offset)
                .filter(|query| query.context == complete::CompletionContext::AfterDot);
        }
        let text = self.doc_text(key)?;
        semantic_query::after_dot_query(
            &self.index.scoped_for_file(key),
            self.parsers.kotlin_mut(),
            &text,
            offset,
            MAX_COMPLETIONS,
        )
    }

    pub fn completion_query(
        &mut self,
        key: &str,
        offset: usize,
    ) -> Option<semantic_query::CompletionQuery> {
        if is_java_path(key) {
            if let Some(doc) = self.open_docs.get(key) {
                return java::completion_query(
                    &self.index.scoped_for_file(key),
                    key,
                    &doc.text,
                    &doc.tree,
                    offset,
                    MAX_COMPLETIONS,
                );
            }
            let (text, tree) = self.parsed_closed_doc(key)?;
            return java::completion_query(
                &self.index.scoped_for_file(key),
                key,
                &text,
                &tree,
                offset,
                MAX_COMPLETIONS,
            );
        }
        if !self.open_docs.contains_key(key) {
            if let Some(query) = self.closed_completion_query(key, offset) {
                return Some(query);
            }
        }
        let text = self.doc_text(key)?;
        semantic_query::completion_query(
            &self.index.scoped_for_file(key),
            self.parsers.kotlin_mut(),
            key,
            &text,
            offset,
            MAX_COMPLETIONS,
        )
    }

    fn closed_completion_query(
        &mut self,
        key: &str,
        offset: usize,
    ) -> Option<semantic_query::CompletionQuery> {
        let text = self.doc_text(key)?;
        let tree = self.salsa.parsed_tree(key, &text, Durability::LOW);
        if complete::completion_context(&tree, &text, offset)
            != complete::CompletionContext::AfterDot
        {
            return None;
        }
        let (prefix, synthetic, syn_offset) = complete::dot_recovery(&text, offset)?;
        let synthetic_key = format!("{key}#completion");
        let synthetic_tree = self
            .salsa
            .parsed_tree(&synthetic_key, &synthetic, Durability::LOW);
        let receiver = complete::navigation_receiver_at(&synthetic_tree, syn_offset)?;
        let receiver_type = self.salsa.expr_type_in_file(
            &synthetic_key,
            key,
            &synthetic,
            Durability::LOW,
            &self.index,
            self.index_revision,
            ExprKey::from_node(receiver),
        );
        semantic_query::member_completion_query_for_type(
            &self.index.scoped_for_file(key),
            SourceLanguage::Kotlin,
            &synthetic_tree,
            &synthetic,
            receiver_type,
            prefix,
            MAX_COMPLETIONS,
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
    /// Open buffers reuse their cached tree (no parse on the hot path); a non-open file is read from
    /// disk and parsed once, exactly like `goto_definition`.
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
        self.resolved_symbol_query(key, offset)?.symbol_summary()
    }

    /// Project/library symbols matching `query`, capped and ordered for workspace/symbol.
    pub fn workspace_symbols(&self, query: &str) -> Vec<SymbolSummary> {
        const CAP: usize = 200;
        let mut out: Vec<SymbolSummary> = self
            .index
            .all_entries()
            .iter()
            .map(SymbolSummary::from_entry)
            .filter(|s| s.matches_query(query))
            .collect();
        out.sort_by(|a, b| {
            tier_rank(a.tier)
                .cmp(&tier_rank(b.tier))
                .then(a.name.len().cmp(&b.name.len()))
                .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then(a.package.cmp(&b.package))
                .then(a.file.cmp(&b.file))
                .then(a.start_byte.cmp(&b.start_byte))
        });
        out.truncate(CAP);
        out
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
        if let Some(doc) = self.open_docs.get(key) {
            return language::inlay_hints(
                SourceLanguage::for_key(key),
                &self.index.scoped_for_file(key),
                &doc.tree,
                &doc.text,
                start_byte,
                end_byte,
            );
        }
        let (text, tree) = match self.parsed_closed_doc(key) {
            Some(parsed) => parsed,
            None => return Vec::new(),
        };
        language::inlay_hints(
            SourceLanguage::for_key(key),
            &self.index.scoped_for_file(key),
            &tree,
            &text,
            start_byte,
            end_byte,
        )
    }

    /// `textDocument/prepareRename`: exact range + current spelling for project/local symbols.
    pub fn prepare_rename(
        &mut self,
        key: &str,
        offset: usize,
    ) -> Option<crate::rename::PreparedRename> {
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
        let target = self.rename_target(key, offset)?;
        let text = self.doc_text(&target.file)?;
        let placeholder = text.get(target.start_byte..target.end_byte)?.to_string();
        Some(crate::rename::PreparedRename {
            range: target,
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
        if !language::is_valid_identifier(SourceLanguage::for_key(key), new_name) {
            return None;
        }
        if let Some(subject) = self.extension_rename_subject(key, offset) {
            return self.rename_contribution(key, offset, subject, new_name);
        }
        let target = self.rename_target(key, offset)?;
        let name = self.name_at(key, offset)?;
        if self.index.lookup_refs(&name).len() > RENAME_REF_CAP {
            return None;
        }
        let refs = self.references(key, offset, true);
        if refs.is_empty() || !refs.iter().any(|r| *r == target) {
            return None;
        }
        let edits = crate::rename::edits_for_refs(refs, new_name);
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
            RenameTarget::Member(_) => self.references(key, offset, true),
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
        let target = self.goto_definition(key, offset).into_iter().next()?;
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
        if let Some(doc) = self.open_docs.get(key) {
            return language::type_definition(
                SourceLanguage::for_key(key),
                &self.index.scoped_for_file(key),
                key,
                &doc.tree,
                &doc.text,
                offset,
            );
        }
        let (text, tree) = match self.parsed_closed_doc(key) {
            Some(parsed) => parsed,
            None => return Vec::new(),
        };
        language::type_definition(
            SourceLanguage::for_key(key),
            &self.index.scoped_for_file(key),
            key,
            &tree,
            &text,
            offset,
        )
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
            return language::outgoing_calls(
                SourceLanguage::for_key(&item.file),
                &self.index.scoped_for_file(&item.file),
                &item.file,
                &doc.tree,
                &doc.text,
                item,
            );
        }
        let (text, tree) = match self.parsed_closed_doc(&item.file) {
            Some(parsed) => parsed,
            None => return Vec::new(),
        };
        language::outgoing_calls(
            SourceLanguage::for_key(&item.file),
            &self.index.scoped_for_file(&item.file),
            &item.file,
            &tree,
            &text,
            item,
        )
    }

    pub fn signature_help(
        &mut self,
        key: &str,
        offset: usize,
    ) -> Option<crate::signature::SignatureHelp> {
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
        } = language::signature_entries(
            &self.index.scoped_for_file(key),
            language,
            key,
            doc_text,
            tree,
            offset,
        )?;
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
        crate::signature::signatures_for_entries(entries, active_parameter)
    }

    /// `textDocument/publishDiagnostics` source: name-based, high-confidence diagnostics for `key`,
    /// over the cached tree for an open buffer (or a one-off parse from disk). Byte-range based; the
    /// LSP layer converts to positions and severities.
    pub fn diagnostics(&mut self, key: &str) -> Vec<crate::diagnostics::Diagnostic> {
        let mut extra = self
            .analysis_extensions
            .sets()
            .filter(|(owner, _)| *owner == key)
            .flat_map(|(_, set)| set.diagnostics().iter().cloned())
            .collect::<Vec<_>>();
        if let Some(doc) = self.open_docs.get(key) {
            extra.extend(self.diagnostics_for_tree(key, &doc.text, &doc.tree));
            return extra;
        }
        let (text, tree) = match self.parsed_closed_doc(key) {
            Some(parsed) => parsed,
            None => return Vec::new(),
        };
        extra.extend(self.diagnostics_for_tree(key, &text, &tree));
        extra
    }

    fn diagnostics_for_tree(
        &self,
        key: &str,
        text: &str,
        tree: &Tree,
    ) -> Vec<crate::diagnostics::Diagnostic> {
        language::diagnostics(
            &self.index.scoped_for_file(key),
            SourceLanguage::for_key(key),
            key,
            text,
            tree,
            &self.effective_completeness(),
        )
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
        let start = std::time::Instant::now();
        let target = match self.goto_definition(key, offset).into_iter().next() {
            Some(d) => d,
            None => return Vec::new(),
        };
        let mut names = self.reference_names(&target);
        if names.is_empty() {
            if let Some(name) = self.name_at(key, offset) {
                names.insert(name);
            }
        }
        let mut candidates: Vec<RefEntry> = names
            .iter()
            .flat_map(|name| self.index.lookup_refs(name).iter().cloned())
            .collect();
        // Backstop against a pathologically common name in a very large project.
        const MAX_CANDIDATES: usize = 5000;
        if candidates.len() > MAX_CANDIDATES {
            tracing::warn!(
                "references({names:?}): {} candidates, capping at {MAX_CANDIDATES}",
                candidates.len()
            );
            candidates.truncate(MAX_CANDIDATES);
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
        out
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
fn is_excluded(entry: &DirEntry) -> bool {
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
    origin.starts_with("maven:") || origin.starts_with("file:") || origin.starts_with("jdk:")
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
            "*.kt",
            "*.kts",
            "*.java",
        ])
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
    fn model_refresh_removes_inactive_libraries_and_rejects_stale_batches_and_readiness() {
        use crate::project_model::{FileOrigin, SourceRepresentation};
        let root = unique_tmp("model-refresh-library");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("api.jar"), b"first binary").unwrap();
        let model = |version| {
            Arc::new(crate::classpath::ProjectSnapshot::new(crate::classpath::parse_dump(&format!(
            "PROJECT\t:app\t{}\nSOURCE\tmain\t{}\nSCP\tmain\t{}\nARTIFACT\tmain\t{}\tMODULE\tvendor\tapi\t{version}\nEND\t:app\n",
            root.display(), root.join("src").display(), root.join("api.jar").display(), root.join("api.jar").display()))))
        };
        let mut ws = Workspace::new();
        ws.set_project_model(model("1"));
        let old = ws.begin_analysis_generation();
        let mut parsers = LanguageParsers::new();
        let batch = |parsers: &mut LanguageParsers, key: &str, text: &str| {
            let tree = parsers.parse_for_key(key, text);
            crate::deps::FileSymbols {
                file: key.to_string(),
                symbols: language::file_facts(SourceLanguage::for_key(key), &tree, text).symbols,
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
