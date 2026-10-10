//! In-memory cross-file symbol index, split into two tiers.
//!
//! Symbols are tagged `Volatile` (project files & open buffers — churned on every edit) or
//! `Durable` (library dependency symbols — written once, never touched by an edit). A single
//! shared by-name map keeps lookups O(1) and clone-free; `files` records each file's tier for
//! whole-file replace/remove. The tier split makes "a keystroke can't disturb library symbols"
//! structural, and marks exactly the set that the persistent symbol cache serializes.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ops::Index as SliceIndex;
use std::sync::Arc;

use crate::fxhash::FxHashMap;
use crate::project_model::{FileOrigin, ProjectScope, ScopeVisibility, SourceRepresentation};
use crate::symbol::{IndexedSymbol, SymbolKind};
use crate::types::TypeRef;

/// Which tier a file's symbols belong to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// Project files and open buffers — re-indexed on edits.
    Volatile,
    /// Library dependency symbols — written once, never disturbed by project edits.
    Durable,
}

/// An indexed symbol together with the file that declared it and its tier.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Canonical file key (a path or URI string — the caller picks one identity scheme).
    pub path: Arc<str>,
    pub tier: Tier,
    pub sym: Arc<IndexedSymbol>,
}

/// An identifier *usage* produced by the reverse-reference pass (carries its own name for removal).
///
/// `name` is interned per file at extraction: a project shares most identifier spellings across
/// thousands of usages, so an `Arc<str>` turns per-usage string clones into refcount bumps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Usage {
    pub name: Arc<str>,
    pub start_byte: usize,
    pub end_byte: usize,
}

/// A usage site keyed by name in the reverse index.
#[derive(Clone, Debug)]
pub struct RefEntry {
    pub path: Arc<str>,
    pub start_byte: usize,
    pub end_byte: usize,
}

/// Push an entry into a name->entries bucket, cloning the key only when the bucket is new.
fn push_entry(map: &mut FxHashMap<String, Vec<Entry>>, key: &str, entry: Entry) {
    if let Some(entries) = map.get_mut(key) {
        entries.push(entry);
    } else {
        map.insert(key.to_string(), vec![entry]);
    }
}

#[derive(Clone, Default)]
pub struct Index {
    /// Source of truth: each file's tier + symbols (for whole-file replace/remove).
    files: FxHashMap<String, (Tier, Vec<Arc<IndexedSymbol>>)>,
    /// Derived lookup: simple name -> entries (both tiers merged).
    by_name: FxHashMap<String, Vec<Entry>>,
    /// Derived lookup for member completion: container type simple-name -> member entries (the
    /// hot path for `receiver.` completion, so it gets a dedicated maintained map). Keyed off
    /// `sym.container`; top-level symbols (no container) are not recorded here.
    members_by_container: FxHashMap<String, Vec<Entry>>,
    /// Derived lookup for member completion: extension-receiver type simple-name -> extension
    /// entries (`fun T.f` / `val T.p`). Keyed off `sym.ext_receiver`.
    ext_by_receiver: FxHashMap<String, Vec<Entry>>,
    /// Reverse index source of truth: each file's identifier usages (project files only).
    ref_files: FxHashMap<String, Vec<Usage>>,
    /// Derived reverse lookup: simple name -> usage sites.
    refs_by_name: FxHashMap<Arc<str>, Vec<RefEntry>>,
    origins: OriginFacts,
    contributions: Arc<BTreeMap<String, Vec<crate::extensions::ContributionSet>>>,
}

#[derive(Clone, Default)]
pub struct InferenceIndexSnapshot {
    by_name: FxHashMap<String, Vec<Entry>>,
    members_by_container: FxHashMap<String, Vec<Entry>>,
    ext_by_receiver: FxHashMap<String, Vec<Entry>>,
    origins: OriginFacts,
    contributions: Arc<BTreeMap<String, Vec<crate::extensions::ContributionSet>>>,
}

/// Project ownership is separate from serialized symbols and per-symbol entries.
#[derive(Clone, Default)]
pub struct OriginFacts {
    files: Arc<FxHashMap<String, FileOrigin>>,
    visibilities: Arc<BTreeMap<ProjectScope, ScopeVisibility>>,
    // Only types actually removed from authoritative source are stale. A
    // source-less binary class may instead be generated and is not excluded.
    removed_source_types: Arc<BTreeSet<(String, String, String)>>,
    configured: bool,
}

/// A query borrows the original lookup bucket; a scoped query selects references
/// from that bucket without cloning symbols or the index.
#[derive(Clone)]
pub enum LookupEntries<'a> {
    All(&'a [Entry]),
    Selected(Vec<&'a Entry>),
}

impl<'a> LookupEntries<'a> {
    pub fn iter(&self) -> LookupIter<'_, 'a> {
        match self {
            Self::All(entries) => LookupIter::All(entries.iter()),
            Self::Selected(entries) => LookupIter::Selected(entries.iter()),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Self::All(entries) => entries.len(),
            Self::Selected(entries) => entries.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn first(&self) -> Option<&'a Entry> {
        self.get(0)
    }

    pub fn get(&self, index: usize) -> Option<&'a Entry> {
        match self {
            Self::All(entries) => entries.get(index),
            Self::Selected(entries) => entries.get(index).copied(),
        }
    }
}

impl SliceIndex<usize> for LookupEntries<'_> {
    type Output = Entry;

    fn index(&self, index: usize) -> &Entry {
        self.get(index).expect("lookup entry index")
    }
}

pub enum LookupIter<'s, 'a> {
    All(std::slice::Iter<'a, Entry>),
    Selected(std::slice::Iter<'s, &'a Entry>),
}

impl<'a> Iterator for LookupIter<'_, 'a> {
    type Item = &'a Entry;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::All(entries) => entries.next(),
            Self::Selected(entries) => entries.next().copied(),
        }
    }
}

pub enum LookupIntoIter<'a> {
    All(std::slice::Iter<'a, Entry>),
    Selected(std::vec::IntoIter<&'a Entry>),
}

impl<'a> Iterator for LookupIntoIter<'a> {
    type Item = &'a Entry;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::All(entries) => entries.next(),
            Self::Selected(entries) => entries.next(),
        }
    }
}

impl<'a> IntoIterator for LookupEntries<'a> {
    type Item = &'a Entry;
    type IntoIter = LookupIntoIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        match self {
            Self::All(entries) => LookupIntoIter::All(entries.iter()),
            Self::Selected(entries) => LookupIntoIter::Selected(entries.into_iter()),
        }
    }
}

impl<'s, 'a> IntoIterator for &'s LookupEntries<'a> {
    type Item = &'a Entry;
    type IntoIter = LookupIter<'s, 'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

pub trait InferenceIndex {
    fn lookup_by_name(&self, name: &str) -> LookupEntries<'_>;
    fn members_of(&self, type_name: &str) -> LookupEntries<'_>;
    fn extensions_for(&self, receiver_type: &str) -> LookupEntries<'_>;

    fn contributions_for_file(&self, _file: &str) -> &[crate::extensions::ContributionSet] {
        &[]
    }

    fn semantic_binding_at(
        &self,
        file: &str,
        byte: usize,
    ) -> Option<&crate::extensions::MemberBinding> {
        let mut selected = None;
        for set in self.contributions_for_file(file) {
            if let Some(binding) = set.binding_at(byte) {
                let candidate = set
                    .requirements()
                    .iter()
                    .flat_map(|resolution| &resolution.candidates)
                    .find(|candidate| candidate.id == binding.target)?;
                if !self.scope_is_complete()
                    || !self.lookup_by_name(&candidate.name).iter().any(|entry| {
                        entry.path.as_ref() == binding.target.definition().file
                            && entry.sym.start_byte == binding.target.definition().start_byte
                            && entry.sym.end_byte == binding.target.definition().end_byte
                    })
                {
                    return None;
                }
                if selected.is_some_and(|old| old != binding) {
                    return None;
                }
                selected = Some(binding);
            }
        }
        selected
    }

    /// A relation preserves authored identity and is usable only while its real target remains
    /// visible in this scoped view. Conflicting providers cannot silently choose a target.
    fn semantic_binding(
        &self,
        declaration: &crate::symbol::Def,
    ) -> Option<&crate::extensions::MemberBinding> {
        let mut selected = None;
        for set in self.contributions_for_file(&declaration.file) {
            for resolution in set.requirements() {
                let crate::knowledge::Knowledge::Found(binding) = &resolution.binding else {
                    continue;
                };
                if binding
                    .authored
                    .as_ref()
                    .is_none_or(|authored| authored.definition() != declaration)
                {
                    continue;
                }
                let candidate = resolution
                    .candidates
                    .iter()
                    .find(|candidate| candidate.id == binding.target)?;
                if !self.lookup_by_name(&candidate.name).iter().any(|entry| {
                    entry.path.as_ref() == binding.target.definition().file
                        && entry.sym.start_byte == binding.target.definition().start_byte
                        && entry.sym.end_byte == binding.target.definition().end_byte
                }) || !self.scope_is_complete()
                {
                    return None;
                }
                if selected.is_some_and(|old| old != binding) {
                    return None;
                }
                selected = Some(binding);
            }
        }
        selected
    }

    fn semantic_entry(&self, declaration: &crate::symbol::Def) -> Option<&Entry> {
        let binding = self.semantic_binding(declaration)?;
        for set in self.contributions_for_file(&declaration.file) {
            for resolution in set.requirements() {
                if let Some(candidate) = resolution
                    .candidates
                    .iter()
                    .find(|candidate| candidate.id == binding.target)
                {
                    return self
                        .lookup_by_name(&candidate.name)
                        .into_iter()
                        .find(|entry| {
                            entry.path.as_ref() == binding.target.definition().file
                                && entry.sym.start_byte == binding.target.definition().start_byte
                                && entry.sym.end_byte == binding.target.definition().end_byte
                        });
                }
            }
        }
        None
    }

    fn semantic_definition(&self, declaration: &crate::symbol::Def) -> crate::symbol::Def {
        let mut current = declaration.clone();
        let mut seen = BTreeSet::new();
        for _ in 0..16 {
            if !seen.insert(current.clone()) {
                return declaration.clone();
            }
            let Some(binding) = self.semantic_binding(&current) else {
                return current;
            };
            current = binding.target.definition().clone();
        }
        declaration.clone()
    }

    fn origin_facts(&self) -> Option<&OriginFacts> {
        None
    }

    fn scope_is_complete(&self) -> bool {
        true
    }

    fn permits_unique_member_fallback(&self) -> bool {
        true
    }

    fn entries_with_prefix<'a>(
        &'a self,
        _prefix: &'a str,
        _top_level_only: bool,
    ) -> Box<dyn Iterator<Item = &'a Entry> + 'a> {
        Box::new(std::iter::empty())
    }

    fn entries_for_file(&self, file: &str) -> Vec<Entry> {
        self.entries_with_prefix("", false)
            .filter(|entry| entry.path.as_ref() == file)
            .cloned()
            .collect()
    }

    fn all_entries(&self) -> Vec<Entry> {
        self.entries_with_prefix("", false).cloned().collect()
    }

    fn lookup_type(&self, name: &str) -> Vec<&Entry> {
        self.lookup_by_name(name)
            .iter()
            .filter(|e| e.sym.kind.is_type_like())
            .collect()
    }

    fn supertypes_of_in(&self, type_name: &str, package: Option<&str>) -> Vec<String> {
        let mut out = Vec::new();
        for entry in self
            .lookup_by_name(type_name)
            .iter()
            .filter(|e| e.sym.kind.is_type_like() && package.map_or(true, |p| e.sym.package == p))
        {
            for sup in &entry.sym.supertypes {
                if !out.iter().any(|seen| seen == sup) {
                    out.push(sup.clone());
                }
            }
        }
        out
    }

    fn supertypes_of(&self, type_name: &str) -> Vec<String> {
        self.supertypes_of_in(type_name, None)
    }
}

impl Index {
    pub fn new() -> Self {
        Self::default()
    }

    /// Immutable inference-only view for Salsa queries.
    ///
    /// Reverse-reference maps are excluded. Immutable ownership maps are shared,
    /// so an ordinary source edit does not copy every project's origin strings.
    pub fn inference_snapshot(&self) -> InferenceIndexSnapshot {
        InferenceIndexSnapshot {
            by_name: self.by_name.clone(),
            members_by_container: self.members_by_container.clone(),
            ext_by_receiver: self.ext_by_receiver.clone(),
            origins: self.origins.clone(),
            contributions: self.contributions.clone(),
        }
    }

    pub fn replace_contributions(
        &mut self,
        contributions: BTreeMap<String, Vec<crate::extensions::ContributionSet>>,
    ) {
        self.contributions = Arc::new(contributions);
    }

    pub fn set_file_origin(&mut self, path: &str, origin: FileOrigin) -> bool {
        if self.origins.files.get(path) == Some(&origin) {
            return false;
        }
        Arc::make_mut(&mut self.origins.files).insert(path.to_string(), origin);
        true
    }

    pub fn file_origin(&self, path: &str) -> Option<&FileOrigin> {
        self.origins.files.get(path)
    }

    pub fn file_origins(&self) -> impl Iterator<Item = (&str, &FileOrigin)> {
        self.origins
            .files
            .iter()
            .map(|(path, origin)| (path.as_str(), origin))
    }

    pub fn remove_file_origin(&mut self, path: &str) -> bool {
        if !self.origins.files.contains_key(path) {
            return false;
        }
        Arc::make_mut(&mut self.origins.files).remove(path);
        true
    }

    pub fn replace_scope_visibilities(&mut self, visibilities: Vec<ScopeVisibility>) -> bool {
        let next: BTreeMap<_, _> = visibilities
            .into_iter()
            .map(|visibility| (visibility.scope.clone(), visibility))
            .collect();
        if self.origins.configured && *self.origins.visibilities == next {
            return false;
        }
        self.origins.visibilities = Arc::new(next);
        self.origins.configured = true;
        true
    }

    pub fn scope_visibility(&self, scope: &ProjectScope) -> Option<&ScopeVisibility> {
        self.origins.visibilities.get(scope)
    }

    /// Forget stale-source history after an ownership batch removes an origin.
    /// Called once per model update, rather than scanning all files per removal.
    pub fn prune_origin_history(&mut self) -> bool {
        if self.origins.removed_source_types.is_empty() {
            return false;
        }
        let active: HashSet<&str> =
            self.origins
                .files
                .values()
                .map(|origin| origin.origin_id.as_str())
                .chain(
                    self.origins.visibilities.values().flat_map(|visibility| {
                        visibility.ordered_origins.iter().map(String::as_str)
                    }),
                )
                .collect();
        if self
            .origins
            .removed_source_types
            .iter()
            .all(|(origin, _, _)| active.contains(origin.as_str()))
        {
            return false;
        }
        Arc::make_mut(&mut self.origins.removed_source_types)
            .retain(|(origin, _, _)| active.contains(origin.as_str()));
        true
    }

    pub fn scoped_for_file<'a>(&'a self, file: &'a str) -> ScopedIndex<'a> {
        ScopedIndex::new(self, file)
    }

    /// Replace all symbols contributed by `path`, recording them in the given tier.
    pub fn replace_file(&mut self, path: &str, symbols: Vec<IndexedSymbol>, tier: Tier) {
        self.record_source_type_changes(path, &symbols);
        self.remove_symbols(path);
        let symbols: Vec<_> = symbols.into_iter().map(Arc::new).collect();
        let shared_path: Arc<str> = Arc::from(path);
        for sym in &symbols {
            let entry = Entry {
                path: Arc::clone(&shared_path),
                tier,
                sym: Arc::clone(sym),
            };
            push_entry(&mut self.by_name, &sym.name, entry.clone());
            // Member-by-container map: only symbols that ARE members (Some container).
            if let Some(container) = &sym.container {
                push_entry(&mut self.members_by_container, container, entry.clone());
            }
            // Extension-by-receiver map: only extension functions/properties (Some ext_receiver).
            if let Some(recv) = &sym.ext_receiver {
                push_entry(&mut self.ext_by_receiver, recv, entry);
            }
        }
        self.files.insert(path.to_string(), (tier, symbols));
    }

    /// Replace all identifier usages contributed by `path` (the reverse-reference index).
    pub fn replace_file_refs(&mut self, path: &str, usages: Vec<Usage>) {
        self.remove_refs(path);
        let shared_path: Arc<str> = Arc::from(path);
        for u in &usages {
            let entry = RefEntry {
                path: Arc::clone(&shared_path),
                start_byte: u.start_byte,
                end_byte: u.end_byte,
            };
            if let Some(entries) = self.refs_by_name.get_mut(u.name.as_ref()) {
                entries.push(entry);
            } else {
                self.refs_by_name.insert(Arc::clone(&u.name), vec![entry]);
            }
        }
        self.ref_files.insert(path.to_string(), usages);
    }

    /// Drop everything (symbols + usages) contributed by `path`.
    pub fn remove_file(&mut self, path: &str) {
        self.record_source_type_changes(path, &[]);
        self.remove_symbols(path);
        self.remove_refs(path);
        self.remove_file_origin(path);
    }

    fn record_source_type_changes(&mut self, path: &str, next: &[IndexedSymbol]) {
        let Some(origin) = self
            .file_origin(path)
            .filter(|origin| origin.representation == SourceRepresentation::Source)
        else {
            return;
        };
        let key = |symbol: &IndexedSymbol| {
            (
                origin.origin_id.clone(),
                symbol.package.clone(),
                symbol.name.clone(),
            )
        };
        let removed: Vec<_> = self
            .files
            .get(path)
            .into_iter()
            .flat_map(|(_, symbols)| symbols)
            .filter(|symbol| symbol.kind.is_type_like() && symbol.container.is_none())
            .filter(|symbol| {
                !next.iter().any(|next| {
                    next.kind.is_type_like()
                        && next.container.is_none()
                        && next.name == symbol.name
                        && next.package == symbol.package
                })
            })
            .map(|symbol| key(symbol))
            .collect();
        let restored: Vec<_> = next
            .iter()
            .filter(|symbol| symbol.kind.is_type_like() && symbol.container.is_none())
            .map(key)
            .filter(|key| self.origins.removed_source_types.contains(key))
            .collect();
        if removed.is_empty() && restored.is_empty() {
            return;
        }
        let stale = Arc::make_mut(&mut self.origins.removed_source_types);
        stale.extend(removed);
        for key in restored {
            stale.remove(&key);
        }
    }

    fn remove_symbols(&mut self, path: &str) {
        if let Some((_, old)) = self.files.remove(path) {
            for sym in old {
                Self::drop_from(&mut self.by_name, &sym.name, path);
                if let Some(container) = &sym.container {
                    Self::drop_from(&mut self.members_by_container, container, path);
                }
                if let Some(recv) = &sym.ext_receiver {
                    Self::drop_from(&mut self.ext_by_receiver, recv, path);
                }
            }
        }
    }

    /// Remove every entry contributed by `path` from one bucket of a name->entries map, pruning
    /// the now-empty bucket.
    fn drop_from(map: &mut FxHashMap<String, Vec<Entry>>, key: &str, path: &str) {
        if let Some(entries) = map.get_mut(key) {
            entries.retain(|e| e.path.as_ref() != path);
            if entries.is_empty() {
                map.remove(key);
            }
        }
    }

    fn remove_refs(&mut self, path: &str) {
        if let Some(old) = self.ref_files.remove(path) {
            for u in old {
                if let Some(entries) = self.refs_by_name.get_mut(u.name.as_ref()) {
                    entries.retain(|e| e.path.as_ref() != path);
                    if entries.is_empty() {
                        self.refs_by_name.remove(u.name.as_ref());
                    }
                }
            }
        }
    }

    /// All entries with the given simple name (borrowed; callers clone only what they keep).
    pub fn lookup_by_name(&self, name: &str) -> &[Entry] {
        self.by_name.get(name).map(Vec::as_slice).unwrap_or(&[])
    }

    /// All entries in deterministic file/start order. Returns owned `Entry`s so callers cannot
    /// mutate the index internals.
    pub fn all_entries(&self) -> Vec<Entry> {
        let mut out = Vec::new();
        for (path, (tier, symbols)) in &self.files {
            let shared_path: Arc<str> = Arc::from(path.as_str());
            for sym in symbols {
                out.push(Entry {
                    path: Arc::clone(&shared_path),
                    tier: *tier,
                    sym: Arc::clone(sym),
                });
            }
        }
        out.sort_by(|a, b| {
            a.path
                .cmp(&b.path)
                .then(a.sym.start_byte.cmp(&b.sym.start_byte))
                .then(a.sym.end_byte.cmp(&b.sym.end_byte))
                .then(a.sym.name.cmp(&b.sym.name))
        });
        out
    }

    /// Entries contributed by a single file in source order.
    pub fn entries_for_file(&self, path: &str) -> Vec<Entry> {
        let Some((tier, symbols)) = self.files.get(path) else {
            return Vec::new();
        };
        let shared_path: Arc<str> = Arc::from(path);
        let mut out: Vec<Entry> = symbols
            .iter()
            .map(|sym| Entry {
                path: Arc::clone(&shared_path),
                tier: *tier,
                sym: Arc::clone(sym),
            })
            .collect();
        out.sort_by(|a, b| {
            a.sym
                .start_byte
                .cmp(&b.sym.start_byte)
                .then(a.sym.end_byte.cmp(&b.sym.end_byte))
                .then(a.sym.name.cmp(&b.sym.name))
        });
        out
    }

    /// All entries whose `container == type_name` — members declared directly on the type (instance
    /// members, companion members, enum entries; both tiers merged). Borrowed.
    pub fn members_of(&self, type_name: &str) -> &[Entry] {
        self.members_by_container
            .get(type_name)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// All extension functions/properties whose receiver type simple-name == `receiver_type`.
    /// Borrowed.
    pub fn extensions_for(&self, receiver_type: &str) -> &[Entry] {
        self.ext_by_receiver
            .get(receiver_type)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Type-like entries with simple name `name` (a thin `is_type_like` filter over the by-name
    /// map). NOT a separate maintained index — used by inference to resolve a simple type name's
    /// package.
    pub fn lookup_type(&self, name: &str) -> Vec<&Entry> {
        self.by_name
            .get(name)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .filter(|e| e.sym.kind.is_type_like())
            .collect()
    }

    /// The declared return type of a function named `name`, optionally scoped by `container` and
    /// `package`. Reads the `return_type` stamped at index time; the first scoped match with a
    /// recorded return type wins. `None` when no such function has an explicit return annotation.
    pub fn return_type_of(
        &self,
        name: &str,
        container: Option<&str>,
        package: Option<&str>,
    ) -> Option<TypeRef> {
        self.by_name
            .get(name)?
            .iter()
            .filter(|e| e.sym.kind == SymbolKind::Function)
            .filter(|e| container.map_or(true, |c| e.sym.container.as_deref() == Some(c)))
            .filter(|e| package.map_or(true, |p| e.sym.package == p))
            .find_map(|e| e.sym.return_type.clone())
    }

    /// The declared type of a property named `name`, optionally scoped by `container` and `package`.
    /// Reads the `value_type` stamped at index time. `None` when no such property is annotated.
    pub fn property_type_of(
        &self,
        name: &str,
        container: Option<&str>,
        package: Option<&str>,
    ) -> Option<TypeRef> {
        self.by_name
            .get(name)?
            .iter()
            .filter(|e| e.sym.kind == SymbolKind::Property)
            .filter(|e| container.map_or(true, |c| e.sym.container.as_deref() == Some(c)))
            .filter(|e| package.map_or(true, |p| e.sym.package == p))
            .find_map(|e| e.sym.value_type.clone())
    }

    /// The direct supertype simple-names of `type_name`, across both tiers. Reads the declaring
    /// type's `sym.supertypes` (the first `is_type_like` entry wins — simple names rarely collide).
    /// Returns empty if the type is unknown or has no supertypes.
    pub fn supertypes_of(&self, type_name: &str) -> Vec<String> {
        self.supertypes_of_in(type_name, None)
    }

    /// Like `supertypes_of`, but for the type named `type_name` in a specific `package` (when
    /// known) — so two same-named types in different packages don't share a supertype list.
    /// With `package == None`, unions supertypes across every matching type-like entry.
    pub fn supertypes_of_in(&self, type_name: &str, package: Option<&str>) -> Vec<String> {
        let mut out = Vec::new();
        let Some(entries) = self.by_name.get(type_name) else {
            return out;
        };
        for entry in entries
            .iter()
            .filter(|e| e.sym.kind.is_type_like() && package.map_or(true, |p| e.sym.package == p))
        {
            for sup in &entry.sym.supertypes {
                if !out.iter().any(|seen| seen == sup) {
                    out.push(sup.clone());
                }
            }
        }
        out
    }

    /// Iterate all entries whose symbol name starts with `prefix`. When `top_level_only` is true,
    /// yields only entries with `sym.container.is_none()` (so a common prefix does not surface
    /// thousands of stdlib member symbols). Linear scan of `by_name`; fine at project+stdlib scale,
    /// bounded by the caller's cap. An empty prefix yields everything (capped by the caller).
    pub fn entries_with_prefix<'a>(
        &'a self,
        prefix: &'a str,
        top_level_only: bool,
    ) -> impl Iterator<Item = &'a Entry> + 'a {
        self.by_name
            .iter()
            .filter(move |(name, _)| name.starts_with(prefix))
            .flat_map(|(_, entries)| entries.iter())
            .filter(move |e| !top_level_only || e.sym.container.is_none())
    }

    /// All usage sites of the given simple name (the reverse-reference index).
    pub fn lookup_refs(&self, name: &str) -> &[RefEntry] {
        self.refs_by_name
            .get(name)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

impl InferenceIndex for Index {
    fn contributions_for_file(&self, file: &str) -> &[crate::extensions::ContributionSet] {
        self.contributions
            .get(file)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn lookup_by_name(&self, name: &str) -> LookupEntries<'_> {
        LookupEntries::All(Index::lookup_by_name(self, name))
    }

    fn members_of(&self, type_name: &str) -> LookupEntries<'_> {
        LookupEntries::All(Index::members_of(self, type_name))
    }

    fn extensions_for(&self, receiver_type: &str) -> LookupEntries<'_> {
        LookupEntries::All(Index::extensions_for(self, receiver_type))
    }

    fn origin_facts(&self) -> Option<&OriginFacts> {
        Some(&self.origins)
    }

    fn entries_with_prefix<'a>(
        &'a self,
        prefix: &'a str,
        top_level_only: bool,
    ) -> Box<dyn Iterator<Item = &'a Entry> + 'a> {
        Box::new(Index::entries_with_prefix(self, prefix, top_level_only))
    }

    fn entries_for_file(&self, file: &str) -> Vec<Entry> {
        Index::entries_for_file(self, file)
    }

    fn all_entries(&self) -> Vec<Entry> {
        Index::all_entries(self)
    }
}

impl InferenceIndex for InferenceIndexSnapshot {
    fn contributions_for_file(&self, file: &str) -> &[crate::extensions::ContributionSet] {
        self.contributions
            .get(file)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn lookup_by_name(&self, name: &str) -> LookupEntries<'_> {
        LookupEntries::All(self.by_name.get(name).map(Vec::as_slice).unwrap_or(&[]))
    }

    fn members_of(&self, type_name: &str) -> LookupEntries<'_> {
        LookupEntries::All(
            self.members_by_container
                .get(type_name)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        )
    }

    fn extensions_for(&self, receiver_type: &str) -> LookupEntries<'_> {
        LookupEntries::All(
            self.ext_by_receiver
                .get(receiver_type)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        )
    }

    fn origin_facts(&self) -> Option<&OriginFacts> {
        Some(&self.origins)
    }

    fn entries_with_prefix<'a>(
        &'a self,
        prefix: &'a str,
        top_level_only: bool,
    ) -> Box<dyn Iterator<Item = &'a Entry> + 'a> {
        Box::new(
            self.by_name
                .iter()
                .filter(move |(name, _)| name.starts_with(prefix))
                .flat_map(|(_, entries)| entries.iter())
                .filter(move |entry| !top_level_only || entry.sym.container.is_none()),
        )
    }
}

enum QueryScope<'a> {
    Unscoped,
    Project(&'a ScopeVisibility),
    // A library file can be shared by modules with different classpaths. Its own
    // origin is certain; another type is usable only when every associated scope
    // selects the same owner.
    Library {
        own_origin: &'a str,
        contexts: Vec<&'a ScopeVisibility>,
    },
    Unknown(Option<&'a str>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct SelectedOwner<'a> {
    origin_id: &'a str,
    representation: SourceRepresentation,
    source_missing: bool,
}

/// A file-relative semantic view. Tier describes storage/editing lifetime;
/// actual project/artifact ownership determines visibility and classpath priority.
pub struct ScopedIndex<'a> {
    index: &'a dyn InferenceIndex,
    file: &'a str,
    facts: Option<&'a OriginFacts>,
    scope: QueryScope<'a>,
    owners: RefCell<FxHashMap<(String, String), Option<SelectedOwner<'a>>>>,
    missing_ownership: Cell<bool>,
}

impl<'a> ScopedIndex<'a> {
    pub fn new(index: &'a dyn InferenceIndex, file: &'a str) -> Self {
        let facts = index.origin_facts();
        let scope = match facts.filter(|facts| facts.configured) {
            None => QueryScope::Unscoped,
            Some(facts) => match facts.files.get(file) {
                Some(origin) => match &origin.project_scope {
                    Some(scope) => facts
                        .visibilities
                        .get(scope)
                        .map(QueryScope::Project)
                        .unwrap_or(QueryScope::Unknown(Some(&origin.origin_id))),
                    None => QueryScope::Library {
                        own_origin: &origin.origin_id,
                        contexts: facts
                            .visibilities
                            .values()
                            .filter(|context| context.ordered_origins.contains(&origin.origin_id))
                            .collect(),
                    },
                },
                None => QueryScope::Unknown(None),
            },
        };
        Self {
            index,
            file,
            facts,
            scope,
            owners: RefCell::new(FxHashMap::default()),
            missing_ownership: Cell::new(false),
        }
    }

    fn file_origin(&self, file: &str) -> Option<&'a FileOrigin> {
        self.facts?.files.get(file)
    }

    fn origin_is_visible(&self, origin: &str) -> bool {
        match &self.scope {
            QueryScope::Unscoped => true,
            QueryScope::Project(context) => context.ordered_origins.iter().any(|id| id == origin),
            QueryScope::Library {
                own_origin,
                contexts,
            } => {
                *own_origin == origin
                    || (!contexts.is_empty()
                        && contexts
                            .iter()
                            .all(|context| context.ordered_origins.iter().any(|id| id == origin)))
            }
            QueryScope::Unknown(own_origin) => *own_origin == Some(origin),
        }
    }

    fn owner_in_context(
        &self,
        package: &str,
        root: &str,
        context: Option<&ScopeVisibility>,
        own_origin: Option<&str>,
    ) -> Option<SelectedOwner<'a>> {
        let mut result: Option<(usize, SelectedOwner<'a>)> = None;
        for entry in self.index.lookup_type(root) {
            if entry.sym.package != package || entry.sym.container.is_some() {
                continue;
            }
            let Some(origin) = self.file_origin(&entry.path) else {
                self.missing_ownership.set(true);
                continue;
            };
            let rank = if own_origin == Some(origin.origin_id.as_str()) {
                Some(0)
            } else {
                context.and_then(|context| {
                    context
                        .ordered_origins
                        .iter()
                        .position(|id| id == &origin.origin_id)
                        .map(|rank| rank + 1)
                })
            };
            let Some(rank) = rank else {
                continue;
            };
            let source_missing = origin.representation == SourceRepresentation::Stub
                && self.facts.is_some_and(|facts| {
                    facts.removed_source_types.contains(&(
                        origin.origin_id.clone(),
                        package.to_string(),
                        root.to_string(),
                    ))
                });
            let owner = SelectedOwner {
                origin_id: &origin.origin_id,
                representation: if source_missing {
                    SourceRepresentation::Source
                } else {
                    origin.representation
                },
                source_missing,
            };
            if result.is_none_or(|(previous_rank, previous)| {
                rank < previous_rank
                    || (rank == previous_rank
                        && previous.representation == SourceRepresentation::Stub
                        && owner.representation == SourceRepresentation::Source)
                    || (rank == previous_rank && previous.source_missing && !owner.source_missing)
            }) {
                result = Some((rank, owner));
            }
        }
        result.map(|(_, owner)| owner)
    }

    fn selected_owner(&self, package: &str, root: &str) -> Option<SelectedOwner<'a>> {
        let key = (package.to_string(), root.to_string());
        if let Some(owner) = self.owners.borrow().get(&key) {
            return *owner;
        }
        let owner = match &self.scope {
            QueryScope::Unscoped => None,
            QueryScope::Project(context) => {
                self.owner_in_context(package, root, Some(context), None)
            }
            QueryScope::Library {
                own_origin,
                contexts,
            } => {
                if contexts.is_empty() {
                    self.owner_in_context(package, root, None, Some(own_origin))
                } else {
                    let first =
                        self.owner_in_context(package, root, Some(contexts[0]), Some(own_origin));
                    if contexts.iter().skip(1).all(|context| {
                        self.owner_in_context(package, root, Some(context), Some(own_origin))
                            == first
                    }) {
                        first
                    } else {
                        self.missing_ownership.set(true);
                        None
                    }
                }
            }
            QueryScope::Unknown(own_origin) => {
                self.owner_in_context(package, root, None, *own_origin)
            }
        };
        self.owners.borrow_mut().insert(key, owner);
        owner
    }

    fn entry_is_visible(&self, entry: &Entry) -> bool {
        if matches!(self.scope, QueryScope::Unscoped) {
            return true;
        }
        let Some(origin) = self.file_origin(&entry.path) else {
            if entry.path.as_ref() == self.file {
                return true;
            }
            self.missing_ownership.set(true);
            return false;
        };
        if !self.origin_is_visible(&origin.origin_id) {
            return false;
        }
        let root = match entry.sym.container.as_deref() {
            Some(container) => container.split('.').next(),
            None if entry.sym.kind.is_type_like() => Some(entry.sym.name.as_str()),
            // Unrelated artifacts can legitimately declare top-level overloads.
            None => return true,
        };
        let Some(owner) = root.and_then(|root| self.selected_owner(&entry.sym.package, root))
        else {
            self.missing_ownership.set(true);
            return false;
        };
        if owner.source_missing {
            self.missing_ownership.set(true);
            return false;
        }
        if origin.origin_id != owner.origin_id {
            return false;
        }
        if origin.representation != owner.representation {
            // An extra binary fact can be stale source or a compiler projection.
            // Only a source counterpart certifies that this is mere duplication.
            let duplicate = self
                .index
                .lookup_by_name(&entry.sym.name)
                .iter()
                .any(|source| {
                    source.sym.package == entry.sym.package
                        && source.sym.container == entry.sym.container
                        && (source.sym.kind == entry.sym.kind
                            || (source.sym.kind.is_type_like() && entry.sym.kind.is_type_like()))
                        && self.file_origin(&source.path).is_some_and(|source_origin| {
                            source_origin.origin_id == owner.origin_id
                                && source_origin.representation == owner.representation
                        })
                });
            if !duplicate {
                self.missing_ownership.set(true);
            }
            return false;
        }
        true
    }

    fn select<'b>(&self, entries: LookupEntries<'b>) -> LookupEntries<'b> {
        if matches!(self.scope, QueryScope::Unscoped) {
            entries
        } else {
            LookupEntries::Selected(
                entries
                    .into_iter()
                    .filter(|entry| self.entry_is_visible(entry))
                    .collect(),
            )
        }
    }
}

impl InferenceIndex for ScopedIndex<'_> {
    fn contributions_for_file(&self, file: &str) -> &[crate::extensions::ContributionSet] {
        self.index.contributions_for_file(file)
    }

    fn lookup_by_name(&self, name: &str) -> LookupEntries<'_> {
        self.select(self.index.lookup_by_name(name))
    }

    fn members_of(&self, type_name: &str) -> LookupEntries<'_> {
        self.select(self.index.members_of(type_name))
    }

    fn extensions_for(&self, receiver_type: &str) -> LookupEntries<'_> {
        self.select(self.index.extensions_for(receiver_type))
    }

    fn origin_facts(&self) -> Option<&OriginFacts> {
        self.facts
    }

    fn scope_is_complete(&self) -> bool {
        if self.missing_ownership.get() {
            return false;
        }
        match &self.scope {
            QueryScope::Unscoped | QueryScope::Project(_) => true,
            QueryScope::Library { contexts, .. } => contexts.first().is_some_and(|first| {
                contexts
                    .iter()
                    .all(|context| context.ordered_origins == first.ordered_origins)
            }),
            QueryScope::Unknown(_) => false,
        }
    }

    fn permits_unique_member_fallback(&self) -> bool {
        matches!(self.scope, QueryScope::Unscoped)
    }

    fn entries_with_prefix<'a>(
        &'a self,
        prefix: &'a str,
        top_level_only: bool,
    ) -> Box<dyn Iterator<Item = &'a Entry> + 'a> {
        Box::new(
            self.index
                .entries_with_prefix(prefix, top_level_only)
                .filter(|entry| self.entry_is_visible(entry)),
        )
    }

    fn entries_for_file(&self, file: &str) -> Vec<Entry> {
        self.index
            .entries_for_file(file)
            .into_iter()
            .filter(|entry| self.entry_is_visible(entry))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbol::SymbolKind;

    fn sym(name: &str) -> IndexedSymbol {
        IndexedSymbol::new(name, SymbolKind::Function, "p", None, 0, name.len())
    }

    fn member(name: &str, container: &str) -> IndexedSymbol {
        IndexedSymbol {
            container: Some(container.to_string()),
            ..sym(name)
        }
    }

    fn origin(
        id: &str,
        scope: Option<ProjectScope>,
        representation: SourceRepresentation,
    ) -> FileOrigin {
        FileOrigin {
            origin_id: id.to_string(),
            project_scope: scope,
            representation,
        }
    }

    fn scope(module: &str) -> ProjectScope {
        ProjectScope {
            module: module.to_string(),
            source_set: "main".to_string(),
        }
    }

    fn visibility(module: &str, origins: &[&str]) -> ScopeVisibility {
        ScopeVisibility {
            scope: scope(module),
            ordered_origins: origins.iter().map(|id| (*id).to_string()).collect(),
        }
    }

    fn scoped_types() -> Index {
        let mut index = Index::new();
        for (file, id, api) in [
            ("project/Reader.java", "project-api", "currentOnly"),
            ("library/Reader.java", "library-v1", "oldOnly"),
        ] {
            index.replace_file(
                file,
                vec![
                    IndexedSymbol::new("Reader", SymbolKind::Class, "api", None, 0, 6),
                    IndexedSymbol::new(
                        "Factory",
                        SymbolKind::Class,
                        "api",
                        Some("Reader".to_string()),
                        10,
                        17,
                    ),
                    IndexedSymbol::new(
                        api,
                        SymbolKind::Function,
                        "api",
                        Some("Reader".to_string()),
                        20,
                        20 + api.len(),
                    ),
                ],
                if id == "project-api" {
                    Tier::Volatile
                } else {
                    Tier::Durable
                },
            );
            index.set_file_origin(file, origin(id, None, SourceRepresentation::Source));
        }
        for module in ["a", "b"] {
            index.set_file_origin(
                module,
                origin(module, Some(scope(module)), SourceRepresentation::Source),
            );
        }
        index.replace_scope_visibilities(vec![
            visibility("a", &["a", "project-api", "library-v1"]),
            visibility("b", &["b", "library-v1"]),
        ]);
        index
    }

    #[test]
    fn scoped_class_owner_controls_nested_types_and_every_member_name() {
        let index = scoped_types();
        for (file, expected_origin, useful, excluded) in [
            ("a", "project/Reader.java", "currentOnly", "oldOnly"),
            ("b", "library/Reader.java", "oldOnly", "currentOnly"),
        ] {
            let view = index.scoped_for_file(file);
            for name in ["Reader", "Factory", useful] {
                let entries = view.lookup_by_name(name);
                assert_eq!(entries.len(), 1, "{file} {name}");
                assert_eq!(entries[0].path.as_ref(), expected_origin);
            }
            assert!(
                view.lookup_by_name(excluded).is_empty(),
                "{file}: missing member must not fall back to another origin"
            );
            let members = view.members_of("Reader");
            assert!(members.iter().any(|entry| entry.sym.name == useful));
            assert!(!members.iter().any(|entry| entry.sym.name == excluded));
            assert!(view.scope_is_complete());
        }
    }

    #[test]
    fn deleted_nested_type_cannot_resurrect_from_another_visible_version() {
        let mut index = scoped_types();
        let symbols = index
            .entries_for_file("project/Reader.java")
            .into_iter()
            .filter(|entry| entry.sym.name != "Factory")
            .map(|entry| (*entry.sym).clone())
            .collect();
        index.replace_file("project/Reader.java", symbols, Tier::Volatile);
        assert!(index
            .scoped_for_file("a")
            .lookup_by_name("Factory")
            .is_empty());
        assert_eq!(
            index.scoped_for_file("b").lookup_by_name("Factory").len(),
            1
        );
    }

    #[test]
    fn shallow_stub_does_not_duplicate_source_or_restore_a_removed_member() {
        let mut index = scoped_types();
        index.replace_file(
            "stub/Reader.class",
            vec![
                IndexedSymbol::new("Reader", SymbolKind::Class, "api", None, 0, 6),
                IndexedSymbol::new(
                    "removed",
                    SymbolKind::Function,
                    "api",
                    Some("Reader".to_string()),
                    10,
                    17,
                ),
            ],
            Tier::Durable,
        );
        index.set_file_origin(
            "stub/Reader.class",
            origin("project-api", None, SourceRepresentation::Stub),
        );
        let view = index.scoped_for_file("a");
        assert_eq!(view.lookup_type("Reader").len(), 1);
        assert_eq!(
            view.lookup_type("Reader")[0].path.as_ref(),
            "project/Reader.java"
        );
        assert!(view.lookup_by_name("removed").is_empty());
        assert!(!view
            .members_of("Reader")
            .iter()
            .any(|entry| entry.sym.name == "removed"));
        assert!(
            !view.scope_is_complete(),
            "extra binary callables need projection provenance and freshness"
        );
    }

    #[test]
    fn top_level_source_removal_cannot_restore_stale_shallow_class_stubs() {
        let mut index = scoped_types();
        index.replace_file(
            "stub/Reader.class",
            vec![IndexedSymbol::new(
                "Reader",
                SymbolKind::Class,
                "api",
                None,
                0,
                6,
            )],
            Tier::Durable,
        );
        index.set_file_origin(
            "stub/Reader.class",
            origin("project-api", None, SourceRepresentation::Stub),
        );
        index.replace_file(
            "stub/Generated.class",
            vec![IndexedSymbol::new(
                "Generated",
                SymbolKind::Class,
                "api",
                None,
                0,
                9,
            )],
            Tier::Durable,
        );
        index.set_file_origin(
            "stub/Generated.class",
            origin("project-api", None, SourceRepresentation::Stub),
        );
        index.replace_scope_visibilities(vec![visibility("a", &["a", "project-api"])]);
        assert_eq!(
            index.scoped_for_file("a").lookup_type("Reader")[0]
                .path
                .as_ref(),
            "project/Reader.java"
        );
        index.replace_file(
            "project/Reader.java",
            vec![IndexedSymbol::new(
                "RenamedReader",
                SymbolKind::Class,
                "api",
                None,
                0,
                13,
            )],
            Tier::Volatile,
        );
        let view = index.scoped_for_file("a");
        assert!(
            view.lookup_type("Reader").is_empty(),
            "renamed source must not resurrect its compiled class"
        );
        assert!(
            !view.scope_is_complete(),
            "stale output cannot establish source absence"
        );
        assert_eq!(
            view.lookup_type("Generated").len(),
            1,
            "a source-less class is not proven stale"
        );
        assert_eq!(view.lookup_type("RenamedReader").len(), 1);
        drop(view);
        index.replace_file(
            "project/Reader.java",
            vec![IndexedSymbol::new(
                "Reader",
                SymbolKind::Class,
                "api",
                None,
                0,
                6,
            )],
            Tier::Volatile,
        );
        assert_eq!(
            index.scoped_for_file("a").lookup_type("Reader")[0]
                .path
                .as_ref(),
            "project/Reader.java"
        );
        index.remove_file("project/Reader.java");
        assert!(
            index.scoped_for_file("a").lookup_type("Reader").is_empty(),
            "deleted source must not resurrect its compiled class"
        );
    }

    #[test]
    fn shallow_nested_binary_class_needs_projection_freshness_before_proving_absence() {
        let mut index = scoped_types();
        index.replace_file(
            "stub/Reader.class",
            vec![
                IndexedSymbol::new("Reader", SymbolKind::Class, "api", None, 0, 6),
                IndexedSymbol::new(
                    "GeneratedNested",
                    SymbolKind::Class,
                    "api",
                    Some("Reader".to_string()),
                    10,
                    25,
                ),
            ],
            Tier::Durable,
        );
        index.set_file_origin(
            "stub/Reader.class",
            origin("project-api", None, SourceRepresentation::Stub),
        );
        let view = index.scoped_for_file("a");
        assert_eq!(view.lookup_type("Reader").len(), 1);
        assert!(
            view.scope_is_complete(),
            "duplicate class-name stubs alone are not unknown facts"
        );
        assert!(view.lookup_type("GeneratedNested").is_empty());
        assert!(
            !view.scope_is_complete(),
            "a binary-only nested type may be generated or stale"
        );
    }

    #[test]
    fn ownership_snapshots_share_maps_until_an_actual_metadata_change() {
        let mut index = scoped_types();
        let snapshot = index.inference_snapshot();
        assert!(Arc::ptr_eq(&index.origins.files, &snapshot.origins.files));
        assert!(Arc::ptr_eq(
            &index.origins.visibilities,
            &snapshot.origins.visibilities
        ));
        let same = index.file_origin("project/Reader.java").cloned().unwrap();
        assert!(!index.set_file_origin("project/Reader.java", same));
        let same = index.origins.visibilities.values().cloned().collect();
        assert!(!index.replace_scope_visibilities(same));
        index.replace_file(
            "project/Reader.java",
            vec![IndexedSymbol::new(
                "Reader",
                SymbolKind::Class,
                "api",
                None,
                0,
                6,
            )],
            Tier::Volatile,
        );
        assert!(
            Arc::ptr_eq(&index.origins.files, &snapshot.origins.files),
            "ordinary source edits must not copy all origin paths"
        );
        assert!(index.set_file_origin("new1", origin("new", None, SourceRepresentation::Source)));
        assert!(!Arc::ptr_eq(&index.origins.files, &snapshot.origins.files));
        let batch_map = Arc::as_ptr(&index.origins.files);
        assert!(index.set_file_origin("new2", origin("new", None, SourceRepresentation::Source)));
        assert_eq!(
            Arc::as_ptr(&index.origins.files),
            batch_map,
            "one background batch must detach at most once"
        );
        assert!(!snapshot.origins.files.contains_key("new1"));
        assert!(Arc::ptr_eq(
            &index.origins.visibilities,
            &snapshot.origins.visibilities
        ));
        assert!(index.replace_scope_visibilities(vec![visibility("a", &["a"])]));
        assert!(!Arc::ptr_eq(
            &index.origins.visibilities,
            &snapshot.origins.visibilities
        ));
        assert_eq!(snapshot.origins.visibilities.len(), 2);
    }

    #[test]
    fn source_history_is_pruned_only_after_its_origin_leaves_the_model() {
        let mut index = scoped_types();
        let stub = IndexedSymbol::new("Reader", SymbolKind::Class, "api", None, 0, 6);
        index.replace_file("stub/Reader.class", vec![stub.clone()], Tier::Durable);
        index.set_file_origin(
            "stub/Reader.class",
            origin("project-api", None, SourceRepresentation::Stub),
        );
        index.replace_file("project/Reader.java", Vec::new(), Tier::Volatile);
        assert!(!index.origins.removed_source_types.is_empty());
        assert!(
            !index.prune_origin_history(),
            "dirty source still owns this origin"
        );
        index.remove_file("project/Reader.java");
        assert!(
            !index.prune_origin_history(),
            "a visible origin can still receive late binary batches"
        );
        let snapshot = index.inference_snapshot();
        index.replace_scope_visibilities(vec![
            visibility("a", &["a", "library-v1"]),
            visibility("b", &["b", "library-v1"]),
        ]);
        assert!(
            !index.prune_origin_history(),
            "a remaining binary file still owns this origin"
        );
        index.remove_file("stub/Reader.class");
        assert!(index.prune_origin_history());
        assert!(index.origins.removed_source_types.is_empty());
        assert!(!index.prune_origin_history());
        assert!(
            ScopedIndex::new(&snapshot, "a")
                .lookup_type("Reader")
                .is_empty(),
            "pruning cannot alter an older query snapshot"
        );
        index.replace_file("stub/Reader.class", vec![stub], Tier::Durable);
        index.set_file_origin(
            "stub/Reader.class",
            origin("project-api", None, SourceRepresentation::Stub),
        );
        index.replace_scope_visibilities(vec![visibility("a", &["a", "project-api"])]);
        assert_eq!(
            index.scoped_for_file("a").lookup_type("Reader")[0]
                .path
                .as_ref(),
            "stub/Reader.class",
            "a fully removed and reintroduced origin has no stale source history"
        );
    }

    #[test]
    fn unrelated_visible_top_level_overloads_are_preserved() {
        let mut index = scoped_types();
        for (file, id) in [
            ("project/Functions.kt", "project-api"),
            ("library/Functions.kt", "library-v1"),
        ] {
            index.replace_file(file, vec![sym("overloaded")], Tier::Durable);
            index.set_file_origin(file, origin(id, None, SourceRepresentation::Source));
        }
        assert_eq!(
            index
                .scoped_for_file("a")
                .lookup_by_name("overloaded")
                .len(),
            2
        );
    }

    #[test]
    fn dependency_source_prefers_own_origin_and_requires_agreement_for_other_types() {
        let mut index = scoped_types();
        for (file, id) in [
            ("library/Shared.java", "shared"),
            ("dep1/Choice.java", "choice1"),
            ("dep2/Choice.java", "choice2"),
        ] {
            let name = if id == "shared" { "Shared" } else { "Choice" };
            index.replace_file(
                file,
                vec![IndexedSymbol::new(
                    name,
                    SymbolKind::Class,
                    "api",
                    None,
                    0,
                    name.len(),
                )],
                Tier::Durable,
            );
            index.set_file_origin(file, origin(id, None, SourceRepresentation::Source));
        }
        index.replace_scope_visibilities(vec![
            visibility("a", &["a", "project-api", "shared", "choice2"]),
            visibility("b", &["b", "library-v1", "shared", "choice1"]),
        ]);
        let dependency = index.scoped_for_file("library/Reader.java");
        assert_eq!(
            dependency.lookup_type("Reader")[0].path.as_ref(),
            "library/Reader.java"
        );
        let shared = index.scoped_for_file("library/Shared.java");
        assert_eq!(shared.lookup_type("Shared").len(), 1);
        assert!(shared.lookup_type("Choice").is_empty());
        assert!(!shared.scope_is_complete());
        assert_eq!(
            index.scoped_for_file("a").lookup_type("Choice")[0]
                .path
                .as_ref(),
            "dep2/Choice.java"
        );
        assert_eq!(
            index.scoped_for_file("b").lookup_type("Choice")[0]
                .path
                .as_ref(),
            "dep1/Choice.java"
        );
    }

    #[test]
    fn origin_lifetime_and_inference_snapshot_follow_file_replacement() {
        let mut index = scoped_types();
        let before = index.file_origin("project/Reader.java").cloned().unwrap();
        index.replace_file(
            "project/Reader.java",
            vec![IndexedSymbol::new(
                "Changed",
                SymbolKind::Class,
                "api",
                None,
                0,
                7,
            )],
            Tier::Volatile,
        );
        assert_eq!(index.file_origin("project/Reader.java"), Some(&before));
        let snapshot = index.inference_snapshot();
        assert_eq!(
            ScopedIndex::new(&snapshot, "a")
                .lookup_type("Changed")
                .len(),
            1
        );
        index.remove_file("project/Reader.java");
        assert!(index.file_origin("project/Reader.java").is_none());
        assert_eq!(
            ScopedIndex::new(&snapshot, "a")
                .lookup_type("Changed")
                .len(),
            1,
            "snapshot must stay immutable"
        );
    }

    #[test]
    fn configured_model_rejects_unowned_global_fallbacks_but_legacy_is_borrowed() {
        let mut index = Index::new();
        index.replace_file("unknown.kt", vec![sym("unique")], Tier::Durable);
        assert!(matches!(
            index.scoped_for_file("caller.kt").lookup_by_name("unique"),
            LookupEntries::All(_)
        ));
        index.replace_scope_visibilities(vec![visibility("a", &["a"])]);
        let view = index.scoped_for_file("caller.kt");
        assert!(view.lookup_by_name("unique").is_empty());
        assert!(!view.scope_is_complete());
        assert!(!view.permits_unique_member_fallback());
    }

    fn extension(name: &str, receiver: &str) -> IndexedSymbol {
        IndexedSymbol {
            ext_receiver: Some(receiver.to_string()),
            ..sym(name)
        }
    }

    fn typ(name: &str, supertypes: &[&str]) -> IndexedSymbol {
        IndexedSymbol {
            kind: SymbolKind::Class,
            supertypes: supertypes.iter().map(|s| s.to_string()).collect(),
            ..sym(name)
        }
    }

    #[test]
    fn replace_is_idempotent_per_file() {
        let mut idx = Index::new();
        idx.replace_file("a.kt", vec![sym("foo"), sym("bar")], Tier::Volatile);
        idx.replace_file("b.kt", vec![sym("foo")], Tier::Durable);
        assert_eq!(idx.lookup_by_name("foo").len(), 2);

        // Re-indexing a.kt with fewer symbols must drop the stale ones.
        idx.replace_file("a.kt", vec![sym("baz")], Tier::Volatile);
        assert_eq!(idx.lookup_by_name("foo").len(), 1); // only b.kt remains
        assert_eq!(idx.lookup_by_name("bar").len(), 0);
        assert_eq!(idx.lookup_by_name("baz").len(), 1);

        // Tiers are recorded on entries and merged in lookup.
        assert_eq!(idx.lookup_by_name("foo")[0].tier, Tier::Durable);

        idx.remove_file("b.kt");
        assert_eq!(idx.lookup_by_name("foo").len(), 0);
    }

    #[test]
    fn member_extension_supertype_maps_idempotent() {
        let mut idx = Index::new();
        // A file contributing a type with supertypes, a member, and an extension.
        idx.replace_file(
            "a.kt",
            vec![
                typ("Dog", &["Base"]),
                member("bark", "Dog"),
                extension("fetch", "Dog"),
            ],
            Tier::Volatile,
        );
        assert_eq!(idx.members_of("Dog").len(), 1);
        assert_eq!(idx.members_of("Dog")[0].sym.name, "bark");
        assert_eq!(idx.extensions_for("Dog").len(), 1);
        assert_eq!(idx.extensions_for("Dog")[0].sym.name, "fetch");
        assert_eq!(idx.supertypes_of("Dog"), vec!["Base".to_string()]);

        // Re-indexing the same file must not duplicate entries in any derived map.
        idx.replace_file(
            "a.kt",
            vec![
                typ("Dog", &["Base"]),
                member("bark", "Dog"),
                extension("fetch", "Dog"),
            ],
            Tier::Volatile,
        );
        assert_eq!(
            idx.members_of("Dog").len(),
            1,
            "members must not duplicate on re-index"
        );
        assert_eq!(
            idx.extensions_for("Dog").len(),
            1,
            "extensions must not duplicate"
        );

        // Re-indexing with the member/extension gone must prune them from the derived maps.
        idx.replace_file("a.kt", vec![typ("Dog", &[])], Tier::Volatile);
        assert!(
            idx.members_of("Dog").is_empty(),
            "stale member must be pruned"
        );
        assert!(
            idx.extensions_for("Dog").is_empty(),
            "stale extension must be pruned"
        );
        assert!(
            idx.supertypes_of("Dog").is_empty(),
            "supertypes updated on re-index"
        );

        // Removing the file clears everything.
        idx.remove_file("a.kt");
        assert!(idx.members_of("Dog").is_empty());
        assert!(idx.supertypes_of("Dog").is_empty());
    }

    fn names_with_prefix(idx: &Index, prefix: &str, top_level_only: bool) -> Vec<String> {
        let mut got: Vec<String> = idx
            .entries_with_prefix(prefix, top_level_only)
            .map(|e| e.sym.name.clone())
            .collect();
        got.sort();
        got
    }

    #[test]
    fn inference_snapshot_does_not_observe_later_mutations() {
        let mut idx = Index::new();
        idx.replace_file("a.kt", vec![member("before", "Box")], Tier::Volatile);
        idx.replace_file_refs(
            "a.kt",
            vec![Usage {
                name: Arc::from("before"),
                start_byte: 10,
                end_byte: 16,
            }],
        );

        let snapshot = idx.inference_snapshot();

        idx.replace_file("a.kt", vec![member("after", "Box")], Tier::Volatile);
        idx.replace_file_refs(
            "a.kt",
            vec![Usage {
                name: Arc::from("after"),
                start_byte: 20,
                end_byte: 25,
            }],
        );

        assert_eq!(snapshot.members_of("Box")[0].sym.name, "before");
        assert!(snapshot.lookup_by_name("after").is_empty());

        assert_eq!(idx.members_of("Box")[0].sym.name, "after");
        assert!(idx.lookup_by_name("before").is_empty());
        assert!(idx.lookup_refs("before").is_empty());
        assert_eq!(idx.lookup_refs("after").len(), 1);
    }

    #[test]
    fn entries_with_prefix_filters_by_name_and_container() {
        let mut idx = Index::new();
        idx.replace_file(
            "a.kt",
            vec![
                sym("listOf"),
                sym("listOfNotNull"),
                sym("mapOf"),
                member("size", "List"),
            ],
            Tier::Durable,
        );

        // Prefix match across top-level + member names.
        assert_eq!(
            names_with_prefix(&idx, "list", false),
            vec!["listOf".to_string(), "listOfNotNull".to_string()]
        );

        // `top_level_only` drops members; `size` has a container so it is excluded for the "s" prefix.
        assert_eq!(names_with_prefix(&idx, "s", true), Vec::<String>::new());
        assert_eq!(
            names_with_prefix(&idx, "s", false),
            vec!["size".to_string()]
        );

        // Empty prefix yields everything (top-level only here drops the member `size`).
        assert_eq!(
            names_with_prefix(&idx, "", true),
            vec![
                "listOf".to_string(),
                "listOfNotNull".to_string(),
                "mapOf".to_string()
            ]
        );

        // No match -> empty.
        assert_eq!(names_with_prefix(&idx, "zzz", false), Vec::<String>::new());
    }
}
