//! Shared semantic contributions, independent of LSP and framework policy.
//!
//! The host supplies normalized facts from the same accepted inputs as its index. A provider
//! borrows a scoped read session and describes requirements and source sites; it cannot issue
//! target identities, supply revision stamps, or publish feature-specific answers. Host adapters
//! must update file/model inputs together with the corresponding index changes.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::diagnostics::Diagnostic;
use crate::index::{Entry, InferenceIndex, ScopedIndex};
use crate::knowledge::Knowledge;
use crate::symbol::{Def, SymbolKind};

pub mod source;

/// Compile-time registration is intentionally independent of framework discovery and LSP.
pub trait AnalysisProvider: Send + Sync {
    fn id(&self) -> &str;
    fn analyze(&self, session: &mut ReadSession<'_>) -> Result<(), ContractError>;
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IncompleteReason {
    MissingSource,
    MissingFacts,
    IncompleteScope,
    IncompleteTarget,
    IncompleteSelector,
    UnsupportedType(String),
    AmbiguousType,
    AmbiguousMember,
    UnsupportedSource,
}

pub type Fact<T> = Knowledge<T, IncompleteReason>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberKind {
    Method,
    Field,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dispatch {
    Instance,
    Static,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Protected,
    Package,
    Private,
}

/// Only these JVM types have a normalized representation in this first contract. An unresolved
/// class or type parameter must remain Unknown; in particular, it must not be erased to Object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JvmType {
    Boolean,
    Byte,
    Char,
    Short,
    Int,
    Long,
    Float,
    Double,
    String,
    Array(Box<JvmType>),
    Void,
}

impl JvmType {
    fn valid_value(&self) -> bool {
        match self {
            Self::Void => false,
            Self::Array(element) => element.valid_value(),
            _ => true,
        }
    }

    fn descriptor(&self, out: &mut String) {
        match self {
            Self::Boolean => out.push('Z'),
            Self::Byte => out.push('B'),
            Self::Char => out.push('C'),
            Self::Short => out.push('S'),
            Self::Int => out.push('I'),
            Self::Long => out.push('J'),
            Self::Float => out.push('F'),
            Self::Double => out.push('D'),
            Self::String => out.push_str("Ljava/lang/String;"),
            Self::Array(element) => {
                out.push('[');
                element.descriptor(out);
            }
            Self::Void => out.push('V'),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberSignature {
    kind: MemberKind,
    descriptor: String,
}

impl MemberSignature {
    pub fn method(parameters: &[JvmType], returns: &JvmType) -> Result<Self, ContractError> {
        if parameters.iter().any(|parameter| !parameter.valid_value())
            || matches!(returns, JvmType::Array(_)) && !returns.valid_value()
        {
            return Err(ContractError::InvalidSignature);
        }
        let mut descriptor = "(".to_string();
        for parameter in parameters {
            parameter.descriptor(&mut descriptor);
        }
        descriptor.push(')');
        returns.descriptor(&mut descriptor);
        Ok(Self {
            kind: MemberKind::Method,
            descriptor,
        })
    }

    pub fn field(value: &JvmType) -> Result<Self, ContractError> {
        if !value.valid_value() {
            return Err(ContractError::InvalidSignature);
        }
        let mut descriptor = String::new();
        value.descriptor(&mut descriptor);
        Ok(Self {
            kind: MemberKind::Field,
            descriptor,
        })
    }

    pub fn kind(&self) -> MemberKind {
        self.kind
    }

    pub fn descriptor(&self) -> &str {
        &self.descriptor
    }
}

/// Host-owned normalized facts attached to a real indexed declaration. Missing metadata is
/// Unknown, never an implicit public/instance/non-synthetic declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclaredMemberFacts {
    pub definition: Def,
    pub signature: Fact<MemberSignature>,
    pub dispatch: Fact<Dispatch>,
    pub visibility: Fact<Visibility>,
    pub synthetic: Fact<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclarationId {
    host: u64,
    generation: u64,
    definition: Def,
}

impl DeclarationId {
    pub fn definition(&self) -> &Def {
        &self.definition
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeId {
    declaration: DeclarationId,
    qualified_name: String,
    package: String,
    container: String,
    model_generation: u64,
}

/// Exact binary input attached by the host to a current authored source owner. Providers cannot
/// supply a binary identity, signature or source range through this session-bound handle.
#[derive(Clone)]
pub struct CompiledClassHandle {
    session: u64,
    owner: TypeId,
    revision: u64,
    class: Arc<crate::classfile::ClassFileFacts>,
}

#[derive(Clone)]
pub struct CompiledMethodHandle {
    class: CompiledClassHandle,
    method: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CompiledCallable {
    owner: TypeId,
    class: Arc<crate::classfile::ClassFileFacts>,
    method: usize,
    facts: crate::jvm::CallableFacts,
}

impl TypeId {
    pub fn definition(&self) -> &Def {
        self.declaration.definition()
    }

    pub fn qualified_name(&self) -> &str {
        &self.qualified_name
    }
}

/// A target identity is distinct from the authored declaration that requests a binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberId {
    declaration: DeclarationId,
    owner: TypeId,
    signature: Option<MemberSignature>,
}

impl MemberId {
    pub fn definition(&self) -> &Def {
        self.declaration.definition()
    }

    pub fn owner(&self) -> &TypeId {
        &self.owner
    }

    pub fn signature(&self) -> Option<&MemberSignature> {
        self.signature.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemberNames {
    /// Names are tried in order, allowing an extension to express a primary name and aliases.
    Exact(Vec<String>),
    Prefix(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameLink {
    Direct,
    Prefixed(String),
    IndependentAlias,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberRequirement {
    pub target: Fact<TypeId>,
    pub authored: Option<DeclarationId>,
    pub names: MemberNames,
    pub kind: MemberKind,
    pub signature: Fact<MemberSignature>,
    pub dispatch: Fact<Dispatch>,
    pub name_link: NameLink,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemberMismatch {
    Kind,
    Signature,
    Dispatch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberCandidate {
    pub id: MemberId,
    pub name: String,
    pub kind: MemberKind,
    pub signature: Fact<MemberSignature>,
    pub dispatch: Fact<Dispatch>,
    pub visibility: Fact<Visibility>,
    pub synthetic: Fact<bool>,
    pub mismatches: Vec<MemberMismatch>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberBinding {
    pub target: MemberId,
    pub authored: Option<DeclarationId>,
    pub name_link: NameLink,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedRequirement {
    pub requirement: MemberRequirement,
    pub binding: Fact<MemberBinding>,
    pub candidates: Vec<MemberCandidate>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequirementId {
    session: u64,
    offset: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenameSubject {
    Target,
    Authored,
}

/// Kept outside the editable name. This includes a method prefix, Kotlin backticks, or a selector
/// descriptor. Rename edits replace only `editable_name`, preserving these exact source bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameProjection {
    pub prefix: String,
    pub suffix: String,
}

impl NameProjection {
    pub fn apply(&self, new_name: &str) -> String {
        format!("{}{new_name}{}", self.prefix, self.suffix)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundSite {
    pub full_range: Range<usize>,
    pub editable_name: Range<usize>,
    pub rename_subject: RenameSubject,
    pub projection: NameProjection,
    requirement: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replacement {
    pub range: Range<usize>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenameTarget {
    Member(MemberId),
    Declaration(DeclarationId),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContributionSet {
    requirements: Vec<ResolvedRequirement>,
    sites: Vec<BoundSite>,
    diagnostics: Vec<Diagnostic>,
    compiled: Vec<CompiledCallable>,
}

impl ContributionSet {
    pub(crate) fn compiled_callables(
        &self,
    ) -> impl Iterator<
        Item = (
            &Def,
            &crate::classfile::ClassFileFacts,
            &str,
            &crate::jvm::CallableFacts,
        ),
    > {
        self.compiled.iter().map(|callable| {
            (
                callable.owner.definition(),
                callable.class.as_ref(),
                callable.class.methods[callable.method].descriptor.as_ref(),
                &callable.facts,
            )
        })
    }

    pub fn sites_at(
        &self,
        byte: usize,
    ) -> impl Iterator<Item = (&BoundSite, &ResolvedRequirement)> {
        self.sites
            .iter()
            .filter(move |site| site.full_range.start <= byte && byte <= site.full_range.end)
            .map(|site| (site, &self.requirements[site.requirement]))
    }

    pub fn requirements(&self) -> &[ResolvedRequirement] {
        &self.requirements
    }

    pub fn sites(&self) -> &[BoundSite] {
        &self.sites
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn binding_at(&self, byte: usize) -> Option<&MemberBinding> {
        self.sites.iter().find_map(|site| {
            if !site.full_range.contains(&byte) {
                return None;
            }
            match &self.requirements[site.requirement].binding {
                Knowledge::Found(binding) => Some(binding),
                _ => None,
            }
        })
    }

    pub fn references(&self, target: &MemberId) -> Vec<&BoundSite> {
        self.sites
            .iter()
            .filter(|site| {
                matches!(&self.requirements[site.requirement].binding,
                Knowledge::Found(binding) if &binding.target == target)
            })
            .collect()
    }

    pub fn rename_subject_at(&self, byte: usize) -> Option<RenameTarget> {
        self.sites.iter().find_map(|site| {
            if !site.full_range.contains(&byte) {
                return None;
            }
            let Knowledge::Found(binding) = &self.requirements[site.requirement].binding else {
                return None;
            };
            match site.rename_subject {
                RenameSubject::Target => Some(RenameTarget::Member(binding.target.clone())),
                RenameSubject::Authored => binding.authored.clone().map(RenameTarget::Declaration),
            }
        })
    }

    pub fn rename_edits(
        &self,
        target: &RenameTarget,
        new_name: &str,
    ) -> Result<Vec<Replacement>, ContractError> {
        // The language adapter still validates keywords and language-specific escaped names.
        // This boundary accepts plain identifier text, never literal/descriptor delimiters.
        let mut characters = new_name.chars();
        if !characters
            .next()
            .is_some_and(|c| c.is_alphabetic() || matches!(c, '_' | '$'))
            || characters.any(|c| !c.is_alphanumeric() && !matches!(c, '_' | '$'))
        {
            return Err(ContractError::InvalidName);
        }
        let mut edits: Vec<Replacement> = Vec::new();
        for site in &self.sites {
            let Knowledge::Found(binding) = &self.requirements[site.requirement].binding else {
                continue;
            };
            let matches = match (target, site.rename_subject) {
                (RenameTarget::Member(target), RenameSubject::Target) => target == &binding.target,
                (RenameTarget::Declaration(target), RenameSubject::Authored) => {
                    binding.authored.as_ref() == Some(target)
                }
                _ => false,
            };
            if matches && !edits.iter().any(|edit| edit.range == site.editable_name) {
                edits.push(Replacement {
                    range: site.editable_name.clone(),
                    text: new_name.into(),
                });
            }
        }
        edits.sort_by_key(|edit| edit.range.start);
        if edits
            .windows(2)
            .any(|pair| pair[0].range.end > pair[1].range.start)
        {
            return Err(ContractError::OverlappingEdits);
        }
        Ok(edits)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContractError {
    InvalidRange,
    ForeignFile,
    NotIndexedDeclaration,
    MissingSource,
    InvalidSignature,
    InvalidName,
    ForeignIdentity,
    ForeignRequirement,
    InvalidNameLink,
    DisabledProvider,
    Stale,
    OverlappingEdits,
}

struct SourceFacts {
    generation: u64,
    text: Arc<str>,
    coverage: Fact<()>,
    members: BTreeMap<(usize, usize), DeclaredMemberFacts>,
    types: Vec<String>,
    declarations: Vec<source::SourceDeclaration>,
    compiled_revision: u64,
    compiled: Option<(TypeId, Arc<crate::classfile::ClassFileFacts>)>,
}

#[derive(Clone)]
struct ProviderState {
    generation: u64,
    enabled: bool,
    config: Arc<str>,
}

#[derive(Clone, Debug)]
struct ReadSet {
    model: u64,
    namespace: Option<u64>,
    files: BTreeMap<String, u64>,
    compiled: BTreeMap<String, u64>,
}

pub struct PendingContributions {
    host: u64,
    provider: String,
    owner: String,
    provider_generation: u64,
    evaluation: u64,
    reads: ReadSet,
    set: ContributionSet,
}

/// This store owns input stamps and atomic provider/file sets. It does not clone the semantic
/// index or define a loading mechanism. Publication revisions are separate from input revisions.
pub struct ExtensionStore {
    host: u64,
    next_generation: u64,
    model_generation: u64,
    namespace_generation: u64,
    namespace_coverage: Fact<()>,
    publication_revision: u64,
    sources: BTreeMap<String, SourceFacts>,
    providers: BTreeMap<String, ProviderState>,
    evaluations: BTreeMap<(String, String), u64>,
    sets: BTreeMap<(String, String), PendingContributions>,
    dirty: BTreeSet<(String, String)>,
}

impl Default for ExtensionStore {
    fn default() -> Self {
        Self {
            host: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            next_generation: 0,
            model_generation: 0,
            namespace_generation: 0,
            namespace_coverage: unknown(IncompleteReason::IncompleteScope),
            publication_revision: 0,
            sources: BTreeMap::new(),
            providers: BTreeMap::new(),
            evaluations: BTreeMap::new(),
            sets: BTreeMap::new(),
            dirty: BTreeSet::new(),
        }
    }
}

impl ExtensionStore {
    pub fn clear_compiled_inputs(&mut self) {
        let mut changed = Vec::new();
        for (file, source) in &mut self.sources {
            if source.compiled.take().is_some() {
                self.next_generation += 1;
                source.compiled_revision = self.next_generation;
                changed.push(file.clone());
            }
        }
        self.remove_stale_sets();
        for (provider, state) in &self.providers {
            if state.enabled {
                self.dirty
                    .extend(changed.iter().map(|file| (provider.clone(), file.clone())));
            }
        }
    }

    /// The trusted host must first verify its actual successful invocation receipt and input CAS.
    /// This validates semantic ownership, not Gradle success or filesystem freshness. Attaching an
    /// output does not change the authored generation that issued `owner`.
    pub fn replace_compiled_input(
        &mut self,
        index: &dyn InferenceIndex,
        owner: &TypeId,
        class: Option<Arc<crate::classfile::ClassFileFacts>>,
    ) -> Result<(), ContractError> {
        if owner.declaration.host != self.host {
            return Err(ContractError::ForeignIdentity);
        }
        let file = &owner.definition().file;
        let source = self.sources.get(file).ok_or(ContractError::MissingSource)?;
        if source.generation != owner.declaration.generation
            || owner.model_generation != self.model_generation
        {
            return Err(ContractError::Stale);
        }
        let scoped = ScopedIndex::new(index, file);
        let entries = scoped.entries_for_file(file);
        let Some(entry) = entries
            .iter()
            .find(|entry| definition(entry) == *owner.definition())
        else {
            return Err(ContractError::NotIndexedDeclaration);
        };
        // First surface: one top-level authored Java class, exact project origin. SourceFile is
        // only a hint; it does not issue a generated method Def or a Kotlin callable permission.
        if !file.ends_with(".java")
            || entry.sym.kind != SymbolKind::Class
            || entry.sym.container.is_some()
            || !entry.sym.type_params.is_empty()
            || entries
                .iter()
                .filter(|entry| entry.sym.kind.is_type_like())
                .count()
                != 1
            || qualified_type_name(entry) != owner.qualified_name
        {
            return Err(ContractError::NotIndexedDeclaration);
        }
        let origin = scoped
            .origin_facts()
            .and_then(|facts| facts.file_origin(file))
            .filter(|origin| origin.project_scope.is_some())
            .ok_or(ContractError::ForeignIdentity)?;
        if let Some(class) = &class {
            if class.identity.internal_name != owner.qualified_name.replace('.', "/")
                || class.identity.origin_id != origin.origin_id
                || !matches!(
                    class.identity.input,
                    crate::jvm::InputIdentity::Binary { .. }
                )
            {
                return Err(ContractError::ForeignIdentity);
            }
        }
        let replacement = class.map(|class| (owner.clone(), class));
        if source.compiled == replacement {
            return Ok(());
        }
        self.next_generation += 1;
        let source = self.sources.get_mut(file).unwrap();
        source.compiled_revision = self.next_generation;
        source.compiled = replacement;
        self.remove_stale_sets();
        for (provider, state) in &self.providers {
            if state.enabled {
                self.dirty.insert((provider.clone(), file.clone()));
            }
        }
        Ok(())
    }

    pub fn publication_revision(&self) -> u64 {
        self.publication_revision
    }

    pub fn namespace_revision(&self) -> u64 {
        self.namespace_generation
    }

    pub fn source_declarations(&self, file: &str) -> Option<&[source::SourceDeclaration]> {
        self.sources
            .get(file)
            .map(|source| source.declarations.as_slice())
    }

    pub fn contribution_sets(&self) -> impl Iterator<Item = (&str, &str, &ContributionSet)> {
        self.sets
            .iter()
            .map(|((provider, owner), pending)| (provider.as_str(), owner.as_str(), &pending.set))
    }

    /// Invalidated evaluations include negative reads; removed owners and disabled providers
    /// cannot be evaluated again until the host supplies their inputs.
    pub fn take_dirty(&mut self) -> Vec<(String, String)> {
        let dirty = std::mem::take(&mut self.dirty);
        dirty
            .into_iter()
            .filter(|(provider, owner)| {
                self.sources.contains_key(owner)
                    && self
                        .providers
                        .get(provider)
                        .is_some_and(|provider| provider.enabled)
            })
            .collect()
    }

    pub fn replace_source_declarations(
        &mut self,
        index: &dyn InferenceIndex,
        file: &str,
        text: Arc<str>,
        coverage: Fact<()>,
        declarations: Vec<source::SourceDeclaration>,
    ) -> Result<(), ContractError> {
        let entries = index.entries_for_file(file);
        let mut seen = BTreeSet::new();
        for declaration in &declarations {
            if declaration.definition.file != file {
                return Err(ContractError::ForeignFile);
            }
            let entry = entries
                .iter()
                .find(|entry| definition(entry) == declaration.definition)
                .ok_or(ContractError::NotIndexedDeclaration)?;
            validate_entry(&text, entry)?;
            if !seen.insert((
                declaration.definition.start_byte,
                declaration.definition.end_byte,
            )) {
                return Err(ContractError::NotIndexedDeclaration);
            }
            if let Some(owner) = &declaration.owner {
                let owner_entry = entries
                    .iter()
                    .find(|entry| definition(entry) == *owner && entry.sym.kind.is_type_like())
                    .ok_or(ContractError::NotIndexedDeclaration)?;
                let owner_name = owner_entry.sym.container.as_ref().map_or_else(
                    || unquote(&owner_entry.sym.name).to_string(),
                    |parent| format!("{parent}.{}", unquote(&owner_entry.sym.name)),
                );
                if entry.sym.container.as_deref() != Some(owner_name.as_str()) {
                    return Err(ContractError::NotIndexedDeclaration);
                }
            }
            if declaration
                .member
                .as_ref()
                .is_some_and(|member| member.definition != declaration.definition)
            {
                return Err(ContractError::NotIndexedDeclaration);
            }
            for annotation in &declaration.annotations {
                validate_range(&text, &annotation.range)?;
                for argument in &annotation.arguments {
                    validate_argument(&text, argument, &annotation.range)?;
                }
            }
        }
        let members = declarations
            .iter()
            .filter_map(|declaration| declaration.member.clone())
            .collect();
        self.replace_source(index, file, text, coverage, members)?;
        self.sources.get_mut(file).unwrap().declarations = declarations;
        Ok(())
    }

    /// Call with the accepted index and source together. Coverage certifies completeness of the
    /// normalized target, including generated members; a partial parse/plugin model is Unknown.
    pub fn replace_source(
        &mut self,
        index: &dyn InferenceIndex,
        file: &str,
        text: Arc<str>,
        coverage: Fact<()>,
        members: Vec<DeclaredMemberFacts>,
    ) -> Result<(), ContractError> {
        let entries = index.entries_for_file(file);
        let mut types: Vec<_> = entries
            .iter()
            .filter(|entry| entry.sym.kind.is_type_like())
            .map(qualified_type_name)
            .collect();
        types.sort();
        types.dedup();
        let namespace_changed = self
            .sources
            .get(file)
            .is_none_or(|source| source.types != types);
        let mut normalized = BTreeMap::new();
        for member in members {
            if member.definition.file != file {
                return Err(ContractError::ForeignFile);
            }
            let entry = entries
                .iter()
                .find(|entry| {
                    entry.sym.start_byte == member.definition.start_byte
                        && entry.sym.end_byte == member.definition.end_byte
                })
                .ok_or(ContractError::NotIndexedDeclaration)?;
            validate_entry(&text, entry)?;
            let kind = member_kind(entry).ok_or(ContractError::NotIndexedDeclaration)?;
            if matches!(&member.signature, Knowledge::Found(signature) if signature.kind != kind) {
                return Err(ContractError::InvalidSignature);
            }
            if normalized
                .insert(
                    (member.definition.start_byte, member.definition.end_byte),
                    member,
                )
                .is_some()
            {
                return Err(ContractError::NotIndexedDeclaration);
            }
        }
        self.next_generation += 1;
        self.sources.insert(
            file.into(),
            SourceFacts {
                generation: self.next_generation,
                text,
                coverage,
                members: normalized,
                types,
                declarations: Vec::new(),
                compiled_revision: self.next_generation,
                compiled: None,
            },
        );
        if namespace_changed {
            self.namespace_generation += 1;
        }
        self.remove_stale_sets();
        for (provider, state) in &self.providers {
            if state.enabled {
                self.dirty.insert((provider.clone(), file.into()));
            }
        }
        Ok(())
    }

    pub fn remove_source(&mut self, file: &str) {
        if let Some(source) = self.sources.remove(file) {
            if !source.types.is_empty() {
                self.namespace_generation += 1;
            }
            self.remove_stale_sets();
        }
    }

    /// An accepted classpath/project-model change invalidates scoped and negative reads.
    pub fn invalidate_model(&mut self) {
        self.model_generation += 1;
        for source in self.sources.values_mut() {
            self.next_generation += 1;
            source.compiled_revision = self.next_generation;
            source.compiled = None;
        }
        self.namespace_coverage = unknown(IncompleteReason::IncompleteScope);
        self.remove_stale_sets();
        for (provider, state) in &self.providers {
            if state.enabled {
                for owner in self.sources.keys() {
                    self.dirty.insert((provider.clone(), owner.clone()));
                }
            }
        }
    }

    /// Certifies whether a missing scoped type is really absent from the accepted project model.
    /// A source scan finishing does not by itself certify generated outputs or the classpath.
    pub fn set_namespace_coverage(&mut self, coverage: Fact<()>) {
        if self.namespace_coverage != coverage {
            self.namespace_coverage = coverage;
            self.namespace_generation += 1;
            self.remove_stale_sets();
        }
    }

    pub fn configure_provider(&mut self, provider: &str, enabled: bool, config: Arc<str>) {
        if self
            .providers
            .get(provider)
            .is_some_and(|old| old.enabled == enabled && old.config == config)
        {
            return;
        }
        self.next_generation += 1;
        self.providers.insert(
            provider.into(),
            ProviderState {
                generation: self.next_generation,
                enabled,
                config,
            },
        );
        self.remove_stale_sets();
        if enabled {
            for owner in self.sources.keys() {
                self.dirty.insert((provider.into(), owner.clone()));
            }
        }
    }

    /// A current failed evaluation removes only its own previous set. A delayed failure cannot
    /// remove a newer successful evaluation, nor can its token be published after cleanup.
    pub fn fail(&mut self, pending: PendingContributions) -> Result<(), ContractError> {
        self.validate_pending(&pending)?;
        let key = (pending.provider, pending.owner);
        self.evaluations.remove(&key);
        if self.sets.remove(&key).is_some() {
            self.publication_revision += 1;
        }
        Ok(())
    }

    pub fn begin<'a>(
        &'a mut self,
        index: &'a dyn InferenceIndex,
        provider: &str,
        owner: &'a str,
    ) -> Result<ReadSession<'a>, ContractError> {
        let provider_state = self
            .providers
            .get(provider)
            .filter(|provider| provider.enabled)
            .ok_or(ContractError::DisabledProvider)?
            .clone();
        let source = self
            .sources
            .get(owner)
            .ok_or(ContractError::MissingSource)?;
        let owner_generation = source.generation;
        self.next_generation += 1;
        let evaluation = self.next_generation;
        self.evaluations
            .insert((provider.into(), owner.into()), evaluation);
        Ok(ReadSession {
            store: self,
            index: ScopedIndex::new(index, owner),
            owner,
            provider: provider.into(),
            provider_generation: provider_state.generation,
            evaluation,
            session: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            reads: ReadSet {
                model: self.model_generation,
                namespace: None,
                files: BTreeMap::from([(owner.into(), owner_generation)]),
                compiled: BTreeMap::new(),
            },
            set: ContributionSet::default(),
        })
    }

    pub fn commit(&mut self, pending: PendingContributions) -> Result<(), ContractError> {
        self.validate_pending(&pending)?;
        self.evaluations
            .remove(&(pending.provider.clone(), pending.owner.clone()));
        self.sets
            .insert((pending.provider.clone(), pending.owner.clone()), pending);
        self.publication_revision += 1;
        Ok(())
    }

    pub fn contributions(&self, provider: &str, owner: &str) -> Option<&ContributionSet> {
        self.sets
            .get(&(provider.into(), owner.into()))
            .map(|pending| &pending.set)
    }

    fn is_current(&self, pending: &PendingContributions) -> bool {
        self.providers
            .get(&pending.provider)
            .is_some_and(|provider| {
                provider.enabled && provider.generation == pending.provider_generation
            })
            && pending.reads.model == self.model_generation
            && pending
                .reads
                .namespace
                .is_none_or(|generation| generation == self.namespace_generation)
            && pending.reads.files.iter().all(|(file, generation)| {
                self.sources
                    .get(file)
                    .is_some_and(|source| source.generation == *generation)
            })
            && pending.reads.compiled.iter().all(|(file, revision)| {
                self.sources
                    .get(file)
                    .is_some_and(|source| source.compiled_revision == *revision)
            })
    }

    fn validate_pending(&self, pending: &PendingContributions) -> Result<(), ContractError> {
        if pending.host != self.host {
            return Err(ContractError::ForeignIdentity);
        }
        if !self.is_current(pending)
            || self
                .evaluations
                .get(&(pending.provider.clone(), pending.owner.clone()))
                != Some(&pending.evaluation)
        {
            return Err(ContractError::Stale);
        }
        Ok(())
    }

    fn remove_stale_sets(&mut self) {
        let stale: Vec<_> = self
            .sets
            .iter()
            .filter(|(_, pending)| !self.is_current(pending))
            .map(|(key, _)| key.clone())
            .collect();
        if !stale.is_empty() {
            for key in stale {
                self.sets.remove(&key);
                self.dirty.insert(key);
            }
            self.publication_revision += 1;
        }
    }
}

pub struct ReadSession<'a> {
    store: &'a ExtensionStore,
    index: ScopedIndex<'a>,
    owner: &'a str,
    provider: String,
    provider_generation: u64,
    evaluation: u64,
    session: u64,
    reads: ReadSet,
    set: ContributionSet,
}

impl ReadSession<'_> {
    pub fn compiled_class(
        &mut self,
        owner: &TypeId,
    ) -> Result<Fact<CompiledClassHandle>, ContractError> {
        self.check_type(owner)?;
        let source = &self.store.sources[&owner.definition().file];
        self.reads
            .compiled
            .insert(owner.definition().file.clone(), source.compiled_revision);
        Ok(match &source.compiled {
            Some((attached_owner, class)) if attached_owner == owner => {
                Knowledge::Found(CompiledClassHandle {
                    session: self.session,
                    owner: owner.clone(),
                    revision: source.compiled_revision,
                    class: class.clone(),
                })
            }
            _ => unknown(IncompleteReason::MissingFacts),
        })
    }

    pub fn compiled_methods(
        &mut self,
        class: &CompiledClassHandle,
    ) -> Result<Vec<CompiledMethodHandle>, ContractError> {
        self.check_compiled_class(class)?;
        Ok((0..class.class.methods.len())
            .map(|method| CompiledMethodHandle {
                class: class.clone(),
                method,
            })
            .collect())
    }

    /// Admit one narrow Java language surface from host-owned raw facts. Unsupported flags,
    /// descriptors and Kotlin lowering remain Unknown. This does not certify owner completeness.
    pub fn admit_java_method(
        &mut self,
        handle: &CompiledMethodHandle,
    ) -> Result<Fact<()>, ContractError> {
        self.check_compiled_class(&handle.class)?;
        let class = &handle.class.class;
        let method = class
            .methods
            .get(handle.method)
            .ok_or(ContractError::ForeignIdentity)?;
        const PUBLIC: u16 = 0x0001;
        const UNSUPPORTED: u16 = 0x0002 | 0x0004 | 0x0008 | 0x0040 | 0x0080 | 0x0400 | 0x1000;
        if class.access_flags & PUBLIC == 0
            || class.signature.is_some()
            || method.access_flags & PUBLIC == 0
            || method.access_flags & UNSUPPORTED != 0
            || method.name.starts_with('<')
            || method.descriptor.as_ref() != "()Ljava/lang/String;"
            || method.signature.is_some()
        {
            return Ok(unknown(IncompleteReason::UnsupportedSource));
        }
        let signature = MemberSignature::method(&[], &JvmType::String)?;
        let (authored, _) = self.declared_members(&handle.class.owner)?;
        // A known same source declaration is already represented by the ordinary authored path.
        // Unknown homonyms remain in the shared candidate pool rather than being erased.
        if authored.iter().any(|candidate| {
            candidate.name == method.name.as_ref()
                && candidate.signature == Knowledge::Found(signature.clone())
                && candidate.dispatch == Knowledge::Found(Dispatch::Instance)
        }) {
            return Ok(Knowledge::Found(()));
        }
        if self.set.compiled.iter().any(|existing| {
            existing.owner == handle.class.owner
                && existing.class.identity == class.identity
                && existing.method == handle.method
        }) {
            return Ok(Knowledge::Found(()));
        }
        self.set.compiled.push(CompiledCallable {
            owner: handle.class.owner.clone(),
            class: class.clone(),
            method: handle.method,
            facts: crate::jvm::CallableFacts {
                name: method.name.to_string(),
                params: Vec::new(),
                return_type: {
                    let mut ty = crate::types::TypeRef::simple("String");
                    ty.qualification = Some("java.lang".into());
                    ty
                },
                type_params: Vec::new(),
                definition: None,
                fixed_name: None,
                jvm_name_annotation: None,
                is_static: false,
            },
        });
        Ok(Knowledge::Found(()))
    }

    fn check_compiled_class(&mut self, class: &CompiledClassHandle) -> Result<(), ContractError> {
        if class.session != self.session {
            return Err(ContractError::ForeignIdentity);
        }
        self.check_type(&class.owner)?;
        let source = &self.store.sources[&class.owner.definition().file];
        if source.compiled_revision != class.revision
            || !source.compiled.as_ref().is_some_and(|(owner, current)| {
                owner == &class.owner && current.identity == class.class.identity
            })
        {
            return Err(ContractError::Stale);
        }
        self.reads
            .compiled
            .insert(class.owner.definition().file.clone(), class.revision);
        Ok(())
    }

    pub fn declarations(&self) -> &[source::SourceDeclaration] {
        &self.store.sources[self.owner].declarations
    }

    pub fn source(&self) -> &str {
        &self.store.sources[self.owner].text
    }

    pub fn configuration(&self) -> &str {
        &self.store.providers[&self.provider].config
    }

    pub fn declaration(&mut self, range: Range<usize>) -> Result<DeclarationId, ContractError> {
        let entry = self
            .index
            .entries_for_file(self.owner)
            .into_iter()
            .find(|entry| entry.sym.start_byte == range.start && entry.sym.end_byte == range.end)
            .ok_or(ContractError::NotIndexedDeclaration)?;
        self.issue_declaration(&entry)
    }

    pub fn resolve_type(&mut self, qualified_name: &str) -> Fact<TypeId> {
        self.reads.namespace = Some(self.store.namespace_generation);
        let name = qualified_name.rsplit('.').next().unwrap_or(qualified_name);
        let entries: Vec<_> = self
            .index
            .lookup_type(name)
            .into_iter()
            .filter(|entry| qualified_type_name(entry) == qualified_name)
            .cloned()
            .collect();
        if !self.index.scope_is_complete() {
            return unknown(IncompleteReason::IncompleteScope);
        }
        if entries.len() > 1 {
            return unknown(IncompleteReason::AmbiguousType);
        }
        let Some(entry) = entries.first() else {
            return match &self.store.namespace_coverage {
                Knowledge::Found(()) => Knowledge::DefinitelyAbsent,
                Knowledge::Unknown(reasons) if !reasons.is_empty() => {
                    Knowledge::Unknown(reasons.clone())
                }
                _ => unknown(IncompleteReason::IncompleteScope),
            };
        };
        if !matches!(
            entry.sym.kind,
            SymbolKind::Class | SymbolKind::Interface | SymbolKind::Object | SymbolKind::EnumClass
        ) {
            return unknown(IncompleteReason::IncompleteTarget);
        }
        let declaration = match self.issue_declaration(entry) {
            Ok(declaration) => declaration,
            Err(_) => return unknown(IncompleteReason::MissingSource),
        };
        let container = entry.sym.container.as_ref().map_or_else(
            || unquote(&entry.sym.name).to_string(),
            |container| format!("{container}.{}", unquote(&entry.sym.name)),
        );
        Knowledge::Found(TypeId {
            declaration,
            qualified_name: qualified_name.into(),
            package: entry.sym.package.clone(),
            container,
            model_generation: self.store.model_generation,
        })
    }

    pub fn resolve_type_ref(&mut self, ty: &crate::types::TypeRef) -> Fact<TypeId> {
        self.reads.namespace = Some(self.store.namespace_generation);
        let package = self
            .index
            .entries_for_file(self.owner)
            .first()
            .map(|entry| entry.sym.package.clone())
            .unwrap_or_default();
        let context = crate::infer::FileCtx::new(package, Vec::new());
        match crate::infer::declared_type(&self.index, ty, &context) {
            crate::types::Type::Class {
                name,
                package,
                container,
                ..
            } => {
                let qualified = [
                    package.unwrap_or_default(),
                    container.unwrap_or_default(),
                    name,
                ]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(".");
                self.resolve_type(&qualified)
            }
            crate::types::Type::Unknown => {
                unknown(IncompleteReason::UnsupportedType(ty.name.clone()))
            }
        }
    }

    pub fn declared_members(
        &mut self,
        target: &TypeId,
    ) -> Result<(Vec<MemberCandidate>, Fact<()>), ContractError> {
        self.check_type(target)?;
        let source = &self.store.sources[&target.definition().file];
        self.reads
            .files
            .insert(target.definition().file.clone(), source.generation);
        let mut coverage = source.coverage.clone();
        let mut members = Vec::new();
        for entry in self.index.members_of(&target.container) {
            if entry.path.as_ref() != target.definition().file
                || entry.sym.package != target.package
                || entry.sym.container.as_deref() != Some(target.container.as_str())
            {
                continue;
            }
            let Some(kind) = member_kind(entry) else {
                continue;
            };
            if validate_entry(&source.text, entry).is_err() {
                coverage = unknown(IncompleteReason::MissingSource);
                continue;
            }
            let definition = definition(entry);
            let facts = source
                .members
                .get(&(definition.start_byte, definition.end_byte));
            let signature = facts.map_or_else(
                || unknown(IncompleteReason::MissingFacts),
                |facts| facts.signature.clone(),
            );
            let declaration = DeclarationId {
                host: self.store.host,
                generation: source.generation,
                definition,
            };
            members.push(MemberCandidate {
                id: MemberId {
                    declaration,
                    owner: target.clone(),
                    signature: match &signature {
                        Knowledge::Found(signature) => Some(signature.clone()),
                        _ => None,
                    },
                },
                name: unquote(&entry.sym.name).into(),
                kind,
                signature,
                dispatch: facts.map_or_else(
                    || unknown(IncompleteReason::MissingFacts),
                    |facts| facts.dispatch.clone(),
                ),
                visibility: facts.map_or_else(
                    || unknown(IncompleteReason::MissingFacts),
                    |facts| facts.visibility.clone(),
                ),
                synthetic: facts.map_or_else(
                    || unknown(IncompleteReason::MissingFacts),
                    |facts| facts.synthetic.clone(),
                ),
                mismatches: Vec::new(),
            });
        }
        if !self.index.scope_is_complete() {
            coverage = unknown(IncompleteReason::IncompleteScope);
        }
        Ok((members, coverage))
    }

    /// The sole solver used to produce binding status and candidates for every consuming feature.
    pub fn require(
        &mut self,
        requirement: MemberRequirement,
    ) -> Result<RequirementId, ContractError> {
        self.reads.namespace = Some(self.store.namespace_generation);
        if let Some(authored) = &requirement.authored {
            self.check_declaration(authored)?;
            if authored.definition.file != self.owner {
                return Err(ContractError::ForeignFile);
            }
        }
        if matches!(&requirement.names, MemberNames::Exact(names) if names.is_empty() || names.iter().any(|name| name.is_empty()))
        {
            return Err(ContractError::InvalidName);
        }
        if matches!(&requirement.signature, Knowledge::Found(signature) if signature.kind != requirement.kind)
        {
            return Err(ContractError::InvalidSignature);
        }
        let (mut candidates, coverage) = match &requirement.target {
            Knowledge::Found(target) => self.declared_members(target)?,
            Knowledge::DefinitelyAbsent => (Vec::new(), Knowledge::Found(())),
            Knowledge::Unknown(reasons) => (Vec::new(), Knowledge::Unknown(reasons.clone())),
        };
        candidates.retain(|candidate| match &requirement.names {
            MemberNames::Exact(names) => names.contains(&candidate.name),
            MemberNames::Prefix(prefix) => candidate.name.starts_with(prefix),
        });
        let mut reasons = Vec::new();
        if matches!(requirement.names, MemberNames::Prefix(_)) {
            reasons.push(IncompleteReason::IncompleteSelector);
        }
        add_unknown(&requirement.signature, &mut reasons);
        add_unknown(&requirement.dispatch, &mut reasons);
        for candidate in &mut candidates {
            if candidate.kind != requirement.kind {
                candidate.mismatches.push(MemberMismatch::Kind);
            }
            match (&requirement.signature, &candidate.signature) {
                (Knowledge::Found(expected), Knowledge::Found(actual)) if expected != actual => {
                    candidate.mismatches.push(MemberMismatch::Signature)
                }
                _ => {}
            }
            match (&requirement.dispatch, &candidate.dispatch) {
                (Knowledge::Found(expected), Knowledge::Found(actual)) if expected != actual => {
                    candidate.mismatches.push(MemberMismatch::Dispatch)
                }
                _ => {}
            }
        }
        // Unknown metadata on an already rejected candidate cannot make a known different overload
        // applicable. Collect uncertainty only from candidates without a proved mismatch.
        let mut eligible: Vec<_> = candidates
            .iter()
            .filter(|candidate| candidate.mismatches.is_empty())
            .collect();
        if let MemberNames::Exact(names) = &requirement.names {
            if let Some(first) = names
                .iter()
                .find(|name| eligible.iter().any(|candidate| &candidate.name == *name))
            {
                eligible.retain(|candidate| &candidate.name == first);
            }
        }
        for candidate in &eligible {
            add_unknown(&candidate.signature, &mut reasons);
            add_unknown(&candidate.dispatch, &mut reasons);
        }
        // A verified authored JVM member remains a positive fact even when the owner may also
        // contain generated members. Coverage is needed to prove absence, not this exact identity.
        if eligible.len() != 1 || !reasons.is_empty() {
            add_unknown(&coverage, &mut reasons);
        }
        let binding = if !reasons.is_empty() {
            Knowledge::Unknown(reasons)
        } else if eligible.len() > 1 {
            unknown(IncompleteReason::AmbiguousMember)
        } else if let Some(candidate) = eligible.first() {
            self.check_name_link(&requirement, &candidate.name)?;
            Knowledge::Found(MemberBinding {
                target: candidate.id.clone(),
                authored: requirement.authored.clone(),
                name_link: requirement.name_link.clone(),
            })
        } else {
            Knowledge::DefinitelyAbsent
        };
        let id = RequirementId {
            session: self.session,
            offset: self.set.requirements.len(),
        };
        self.set.requirements.push(ResolvedRequirement {
            requirement,
            binding,
            candidates,
        });
        Ok(id)
    }

    pub fn resolution(&self, id: RequirementId) -> Result<&ResolvedRequirement, ContractError> {
        if id.session != self.session {
            return Err(ContractError::ForeignRequirement);
        }
        self.set
            .requirements
            .get(id.offset)
            .ok_or(ContractError::ForeignRequirement)
    }

    pub fn bind_site(
        &mut self,
        id: RequirementId,
        full_range: Range<usize>,
        editable_name: Range<usize>,
        rename_subject: RenameSubject,
    ) -> Result<(), ContractError> {
        let resolution = self.resolution(id)?;
        validate_range(self.source(), &full_range)?;
        validate_range(self.source(), &editable_name)?;
        if editable_name.is_empty()
            || editable_name.start < full_range.start
            || editable_name.end > full_range.end
        {
            return Err(ContractError::InvalidRange);
        }
        let spelling = &self.source()[editable_name.clone()];
        let full_spelling = unquote(&self.source()[full_range.clone()]);
        match rename_subject {
            RenameSubject::Target => {
                let matches = match &resolution.requirement.names {
                    MemberNames::Exact(names) => names.iter().any(|name| name == spelling),
                    MemberNames::Prefix(prefix) => spelling == prefix,
                };
                let bound_name = match &resolution.binding {
                    Knowledge::Found(binding) => resolution
                        .candidates
                        .iter()
                        .find(|candidate| candidate.id == binding.target)
                        .map(|candidate| candidate.name.as_str()),
                    _ => None,
                };
                if !matches
                    || bound_name.is_some_and(|name| name != spelling)
                    || matches!(resolution.requirement.name_link, NameLink::IndependentAlias)
                        && resolution
                            .requirement
                            .authored
                            .as_ref()
                            .is_some_and(|authored| {
                                editable_name.start >= authored.definition.start_byte
                                    && editable_name.end <= authored.definition.end_byte
                            })
                {
                    return Err(ContractError::InvalidNameLink);
                }
                if resolution.binding.is_found() {
                    let identifier = match &resolution.requirement.name_link {
                        NameLink::Prefixed(prefix) => {
                            full_spelling == format!("{prefix}{spelling}")
                        }
                        _ => full_spelling == spelling,
                    };
                    let selector = match &resolution.requirement.signature {
                        Knowledge::Found(signature) if signature.kind == MemberKind::Method => {
                            full_spelling == format!("{spelling}{}", signature.descriptor)
                        }
                        _ => false,
                    };
                    if !identifier && !selector && full_spelling != spelling {
                        return Err(ContractError::InvalidNameLink);
                    }
                }
            }
            RenameSubject::Authored => {
                let authored = resolution
                    .requirement
                    .authored
                    .as_ref()
                    .ok_or(ContractError::InvalidNameLink)?;
                let expected = unquote(
                    &self.source()[authored.definition.start_byte..authored.definition.end_byte],
                );
                if spelling != expected
                    || full_spelling != expected
                    || !matches!(resolution.requirement.name_link, NameLink::IndependentAlias)
                {
                    return Err(ContractError::InvalidNameLink);
                }
            }
        }
        let projection = NameProjection {
            prefix: self.source()[full_range.start..editable_name.start].into(),
            suffix: self.source()[editable_name.end..full_range.end].into(),
        };
        if let NameLink::Prefixed(prefix) = &resolution.requirement.name_link {
            if rename_subject == RenameSubject::Target
                && full_range.start
                    == resolution
                        .requirement
                        .authored
                        .as_ref()
                        .map_or(usize::MAX, |authored| authored.definition.start_byte)
                && !projection
                    .prefix
                    .trim_start_matches('`')
                    .starts_with(prefix)
            {
                return Err(ContractError::InvalidNameLink);
            }
        }
        if self.set.sites.iter().any(|site| {
            site.full_range.start < full_range.end && full_range.start < site.full_range.end
        }) {
            return Err(ContractError::OverlappingEdits);
        }
        self.set.sites.push(BoundSite {
            full_range,
            editable_name,
            rename_subject,
            projection,
            requirement: id.offset,
        });
        Ok(())
    }

    pub fn diagnostic(&mut self, diagnostic: Diagnostic) -> Result<(), ContractError> {
        validate_range(self.source(), &(diagnostic.start_byte..diagnostic.end_byte))?;
        self.set.diagnostics.push(diagnostic);
        Ok(())
    }

    pub fn finish(self) -> PendingContributions {
        PendingContributions {
            host: self.store.host,
            provider: self.provider,
            owner: self.owner.into(),
            provider_generation: self.provider_generation,
            evaluation: self.evaluation,
            reads: self.reads,
            set: self.set,
        }
    }

    fn issue_declaration(&mut self, entry: &Entry) -> Result<DeclarationId, ContractError> {
        let source = self
            .store
            .sources
            .get(entry.path.as_ref())
            .ok_or(ContractError::MissingSource)?;
        validate_entry(&source.text, entry)?;
        self.reads
            .files
            .insert(entry.path.to_string(), source.generation);
        Ok(DeclarationId {
            host: self.store.host,
            generation: source.generation,
            definition: definition(entry),
        })
    }

    fn check_declaration(&mut self, declaration: &DeclarationId) -> Result<(), ContractError> {
        let source = self
            .store
            .sources
            .get(&declaration.definition.file)
            .ok_or(ContractError::Stale)?;
        if declaration.host != self.store.host {
            return Err(ContractError::ForeignIdentity);
        }
        if source.generation != declaration.generation {
            return Err(ContractError::Stale);
        }
        self.reads
            .files
            .insert(declaration.definition.file.clone(), source.generation);
        Ok(())
    }

    fn check_type(&mut self, target: &TypeId) -> Result<(), ContractError> {
        self.check_declaration(&target.declaration)?;
        if target.model_generation != self.store.model_generation {
            return Err(ContractError::Stale);
        }
        self.reads.namespace = Some(self.store.namespace_generation);
        if !self
            .index
            .lookup_type(target.container.rsplit('.').next().unwrap())
            .iter()
            .any(|entry| {
                definition(entry) == *target.definition()
                    && qualified_type_name(entry) == target.qualified_name
            })
        {
            return Err(ContractError::ForeignIdentity);
        }
        Ok(())
    }

    fn check_name_link(
        &self,
        requirement: &MemberRequirement,
        target_name: &str,
    ) -> Result<(), ContractError> {
        let Some(authored) = &requirement.authored else {
            return Ok(());
        };
        let authored_name =
            unquote(&self.source()[authored.definition.start_byte..authored.definition.end_byte]);
        let valid = match &requirement.name_link {
            NameLink::Direct => authored_name == target_name,
            NameLink::Prefixed(prefix) => authored_name.strip_prefix(prefix) == Some(target_name),
            NameLink::IndependentAlias => true,
        };
        if valid {
            Ok(())
        } else {
            Err(ContractError::InvalidNameLink)
        }
    }
}

fn unknown<T>(reason: IncompleteReason) -> Fact<T> {
    Knowledge::Unknown(vec![reason])
}

fn add_unknown<T>(fact: &Fact<T>, reasons: &mut Vec<IncompleteReason>) {
    match fact {
        Knowledge::Unknown(more) if more.is_empty() => reasons.push(IncompleteReason::MissingFacts),
        Knowledge::Unknown(more) => reasons.extend(more.iter().cloned()),
        Knowledge::DefinitelyAbsent => reasons.push(IncompleteReason::MissingFacts),
        Knowledge::Found(_) => {}
    }
}

fn definition(entry: &Entry) -> Def {
    Def {
        file: entry.path.to_string(),
        start_byte: entry.sym.start_byte,
        end_byte: entry.sym.end_byte,
    }
}

fn qualified_type_name(entry: &Entry) -> String {
    [
        &entry.sym.package,
        entry.sym.container.as_deref().unwrap_or(""),
        unquote(&entry.sym.name),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(".")
}

fn member_kind(entry: &Entry) -> Option<MemberKind> {
    match entry.sym.kind {
        SymbolKind::Function
            if entry.sym.container.is_some() && entry.sym.ext_receiver.is_none() =>
        {
            Some(MemberKind::Method)
        }
        SymbolKind::Property
            if entry.sym.container.is_some() && entry.sym.ext_receiver.is_none() =>
        {
            Some(MemberKind::Field)
        }
        _ => None,
    }
}

fn unquote(name: &str) -> &str {
    name.strip_prefix('`')
        .and_then(|name| name.strip_suffix('`'))
        .unwrap_or(name)
}

fn validate_range(text: &str, range: &Range<usize>) -> Result<(), ContractError> {
    if range.start > range.end || text.get(range.clone()).is_none() {
        Err(ContractError::InvalidRange)
    } else {
        Ok(())
    }
}

fn validate_argument(
    text: &str,
    argument: &source::AnnotationArgument,
    parent: &Range<usize>,
) -> Result<(), ContractError> {
    validate_range(text, &argument.range)?;
    if argument.range.start < parent.start || argument.range.end > parent.end {
        return Err(ContractError::InvalidRange);
    }
    match &argument.value {
        Knowledge::Found(source::AnnotationValue::String {
            value,
            content_range,
        }) => {
            validate_range(text, content_range)?;
            if content_range.start < argument.range.start
                || content_range.end > argument.range.end
                || text.get(content_range.clone()) != Some(value.as_str())
            {
                return Err(ContractError::InvalidRange);
            }
        }
        Knowledge::Found(source::AnnotationValue::Array(values)) => {
            for value in values {
                validate_argument(text, value, &argument.range)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_entry(text: &str, entry: &Entry) -> Result<(), ContractError> {
    let range = entry.sym.start_byte..entry.sym.end_byte;
    validate_range(text, &range)?;
    if range.is_empty() || unquote(&text[range]) != unquote(&entry.sym.name) {
        Err(ContractError::NotIndexedDeclaration)
    } else {
        Ok(())
    }
}
