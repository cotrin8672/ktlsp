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

/// Normalized scalar types and host-issued nominal identities. An unresolved class or type
/// parameter must remain Unknown; in particular, it must not be erased to Object.
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
    Reference(JvmReference),
    Array(Box<JvmType>),
    Void,
}

/// A scoped nominal JVM identity issued by the host; providers cannot construct owner names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JvmReference {
    owner: TypeId,
    internal_name: String,
}

impl JvmType {
    fn valid_value(&self) -> bool {
        match self {
            Self::Void => false,
            Self::Array(element) => element.valid_value(),
            _ => true,
        }
    }

    fn references(&self, out: &mut Vec<TypeId>) {
        match self {
            Self::Reference(reference) if !out.contains(&reference.owner) => {
                out.push(reference.owner.clone())
            }
            Self::Array(element) => element.references(out),
            _ => {}
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
            Self::Reference(reference) => {
                out.push('L');
                out.push_str(&reference.internal_name);
                out.push(';');
            }
            Self::Array(element) => {
                out.push('[');
                element.descriptor(out);
            }
            Self::Void => out.push('V'),
        }
    }
}

#[derive(Clone, Debug, Eq)]
pub struct MemberSignature {
    kind: MemberKind,
    descriptor: String,
    references: Vec<TypeId>,
}

impl PartialEq for MemberSignature {
    fn eq(&self, other: &Self) -> bool {
        // Host handles validate provenance at require(); they do not change JVM signature identity.
        self.kind == other.kind && self.descriptor == other.descriptor
    }
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
        let mut references = Vec::new();
        for ty in parameters.iter().chain(std::iter::once(returns)) {
            ty.references(&mut references);
        }
        Ok(Self {
            kind: MemberKind::Method,
            descriptor,
            references,
        })
    }

    pub fn field(value: &JvmType) -> Result<Self, ContractError> {
        if !value.valid_value() {
            return Err(ContractError::InvalidSignature);
        }
        let mut descriptor = String::new();
        value.descriptor(&mut descriptor);
        let mut references = Vec::new();
        value.references(&mut references);
        Ok(Self {
            kind: MemberKind::Field,
            descriptor,
            references,
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
pub struct JvmBinaryName {
    pub internal_name: String,
    pub descriptor: String,
}

/// Editing members and positive raw JVM metadata have independent completeness.
#[derive(Clone, Debug)]
pub struct TypeJvmFacts {
    pub binary_name: Fact<JvmBinaryName>,
    pub class_flags: Fact<u16>,
    pub declared_members: Vec<MemberCandidate>,
    pub declared_members_coverage: Fact<()>,
    pub abi: Fact<Arc<crate::classfile::ClassFileFacts>>,
    pub jvm_abi_coverage: Fact<()>,
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
    kotlin_methods: Option<BTreeSet<String>>,
    value_namespace: bool,
}

/// Current compiler observations, anchored to an authored owner by the host. A frontend
/// descriptor proves a non-null Kotlin callable; classfile visibility alone only proves the JVM surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompilerClassInput {
    pub class: Arc<crate::classfile::ClassFileFacts>,
    pub kotlin_methods: BTreeSet<String>,
    pub companion: bool,
    /// A fresh complete Javac observation under a host-verified body-insensitive profile.
    /// This closes only the value namespace of this source owner, never its method inventory.
    pub value_namespace: bool,
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct CompiledField {
    owner: TypeId,
    class: Arc<crate::classfile::ClassFileFacts>,
    field: usize,
    ty: crate::types::TypeRef,
    companion: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CompiledValueNamespace {
    owner: TypeId,
    class: Arc<crate::classfile::ClassFileFacts>,
}

impl TypeId {
    pub fn definition(&self) -> &Def {
        self.declaration.definition()
    }

    pub fn qualified_name(&self) -> &str {
        &self.qualified_name
    }

    pub fn package(&self) -> &str {
        &self.package
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
    compiled_fields: Vec<CompiledField>,
    compiled_value_namespaces: Vec<CompiledValueNamespace>,
}

impl ContributionSet {
    pub fn compiled_value_namespace(
        &self,
        owner: &Def,
    ) -> Option<&crate::classfile::ClassFileFacts> {
        self.compiled_value_namespaces
            .iter()
            .find(|namespace| namespace.owner.definition() == owner)
            .map(|namespace| namespace.class.as_ref())
    }
    pub(crate) fn compiled_fields(
        &self,
    ) -> impl Iterator<
        Item = (
            &Def,
            &crate::classfile::ClassFileFacts,
            &crate::classfile::MemberFacts,
            &crate::types::TypeRef,
            bool,
        ),
    > {
        self.compiled_fields.iter().map(|field| {
            (
                field.owner.definition(),
                field.class.as_ref(),
                &field.class.fields[field.field],
                &field.ty,
                field.companion,
            )
        })
    }
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
    reference_types: BTreeSet<(usize, usize)>,
    declarations: Vec<source::SourceDeclaration>,
    compiled_revision: u64,
    compiled: Option<(TypeId, Arc<crate::classfile::ClassFileFacts>)>,
    compiler_classes: Vec<(TypeId, CompilerClassInput)>,
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
    missing_sources: BTreeSet<String>,
}

pub struct PendingContributions {
    host: u64,
    provider: String,
    owner: String,
    provider_generation: u64,
    evaluation: u64,
    session: u64,
    reads: ReadSet,
    set: ContributionSet,
}

impl PendingContributions {
    /// Source producers requested by staged reads, without publishing their semantic results.
    pub fn requested_source_files(&self) -> &BTreeSet<String> {
        &self.reads.missing_sources
    }
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
    /// Only unresolved source inputs from current, enabled provider evaluations are requested.
    pub fn requested_source_files(&self) -> BTreeSet<String> {
        self.sets
            .values()
            .filter(|pending| self.is_current(pending))
            .flat_map(|pending| pending.reads.missing_sources.iter().cloned())
            .collect()
    }

    /// Dependencies of this source's current enabled providers, without waiting for other files.
    pub fn requested_source_files_for(&self, owner: &str) -> BTreeSet<String> {
        self.providers
            .keys()
            .filter_map(|provider| self.sets.get(&(provider.clone(), owner.into())))
            .filter(|pending| self.is_current(pending))
            .flat_map(|pending| pending.reads.missing_sources.iter().cloned())
            .collect()
    }

    pub fn clear_compiled_inputs(&mut self) {
        let mut changed = Vec::new();
        for (file, source) in &mut self.sources {
            if source.compiled.take().is_some() || !source.compiler_classes.is_empty() {
                source.compiler_classes.clear();
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

    /// Positive compiler API observations share source/model/read-set generations with compiled
    /// Java inputs. Only reciprocal public static nested identities can extend a source owner.
    pub fn replace_compiler_classes(
        &mut self,
        index: &dyn InferenceIndex,
        owner: &TypeId,
        classes: Vec<CompilerClassInput>,
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
        let entry = entries
            .iter()
            .find(|entry| definition(entry) == *owner.definition())
            .ok_or(ContractError::NotIndexedDeclaration)?;
        let origin = scoped
            .origin_facts()
            .and_then(|facts| facts.file_origin(file))
            .filter(|origin| origin.project_scope.is_some())
            .ok_or(ContractError::ForeignIdentity)?;
        if !(file.ends_with(".kt") || file.ends_with(".java"))
            || entry.sym.container.is_some()
            || !entry.sym.type_params.is_empty()
            || qualified_type_name(entry) != owner.qualified_name
        {
            return Err(ContractError::NotIndexedDeclaration);
        }
        let root = owner.qualified_name.replace('.', "/");
        let Some(parent) = classes
            .iter()
            .find(|input| input.class.identity.internal_name == root)
        else {
            return Err(ContractError::MissingSource);
        };
        let mut names = BTreeSet::new();
        for input in &classes {
            let class = &input.class;
            if !names.insert(&class.identity.internal_name)
                || class.identity.origin_id != origin.origin_id
                || !matches!(
                    class.identity.input,
                    crate::jvm::InputIdentity::Binary { .. }
                )
            {
                return Err(ContractError::ForeignIdentity);
            }
            if class.identity.internal_name != root {
                let Some(inner) = class
                    .inner_classes
                    .iter()
                    .find(|inner| inner.inner.as_ref() == class.identity.internal_name)
                else {
                    return Err(ContractError::ForeignIdentity);
                };
                if inner.outer.as_deref() != Some(root.as_str())
                    || inner.access_flags & 0x0009 != 0x0009
                    || !parent.class.inner_classes.contains(inner)
                    || inner
                        .name
                        .as_ref()
                        .is_none_or(|name| format!("{root}${name}") != class.identity.internal_name)
                {
                    return Err(ContractError::ForeignIdentity);
                }
            }
        }
        let replacement: Vec<_> = classes
            .into_iter()
            .map(|input| (owner.clone(), input))
            .collect();
        if source.compiler_classes == replacement {
            return Ok(());
        }
        self.next_generation += 1;
        let source = self.sources.get_mut(file).unwrap();
        source.compiled_revision = self.next_generation;
        source.compiler_classes = replacement;
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
        self.replace_source_declarations_impl(index, file, text, coverage, declarations, None)
    }

    /// A background host parser may supply the same source eligibility without parsing again
    /// while applying the accepted source/index batch. Other validation remains identical.
    pub fn replace_prepared_source_declarations(
        &mut self,
        index: &dyn InferenceIndex,
        prepared: source::PreparedSourceReferences,
        coverage: Fact<()>,
        declarations: Vec<source::SourceDeclaration>,
    ) -> Result<(), ContractError> {
        let file = prepared.file.clone();
        let text = prepared.text.clone();
        self.replace_source_declarations_impl(
            index,
            &file,
            text,
            coverage,
            declarations,
            Some(prepared),
        )
    }

    fn replace_source_declarations_impl(
        &mut self,
        index: &dyn InferenceIndex,
        file: &str,
        text: Arc<str>,
        coverage: Fact<()>,
        declarations: Vec<source::SourceDeclaration>,
        prepared: Option<source::PreparedSourceReferences>,
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
        self.replace_source_impl(index, file, text, coverage, members, prepared)?;
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
        self.replace_source_impl(index, file, text, coverage, members, None)
    }

    fn replace_source_impl(
        &mut self,
        index: &dyn InferenceIndex,
        file: &str,
        text: Arc<str>,
        coverage: Fact<()>,
        members: Vec<DeclaredMemberFacts>,
        prepared: Option<source::PreparedSourceReferences>,
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
        let reference_types = match prepared {
            Some(prepared) if prepared.file != file => return Err(ContractError::ForeignFile),
            Some(prepared) if prepared.text != text => return Err(ContractError::InvalidRange),
            Some(prepared) => prepared.ranges,
            None => source::ordinary_reference_types(index, file, &text),
        };
        self.next_generation += 1;
        self.sources.insert(
            file.into(),
            SourceFacts {
                generation: self.next_generation,
                text,
                coverage,
                members: normalized,
                types,
                reference_types,
                declarations: Vec::new(),
                compiled_revision: self.next_generation,
                compiled: None,
                compiler_classes: Vec::new(),
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
            source.compiler_classes.clear();
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
                missing_sources: BTreeSet::new(),
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

    /// Reborrow a detached evaluation only while all previously consumed inputs are current.
    pub fn resume<'a>(
        &'a self,
        index: &'a dyn InferenceIndex,
        owner: &'a str,
        pending: PendingContributions,
    ) -> Result<ReadSession<'a>, ContractError> {
        self.validate_pending(&pending)?;
        if pending.owner != owner {
            return Err(ContractError::ForeignFile);
        }
        Ok(ReadSession {
            store: self,
            index: ScopedIndex::new(index, owner),
            owner,
            provider: pending.provider,
            provider_generation: pending.provider_generation,
            evaluation: pending.evaluation,
            session: pending.session,
            reads: pending.reads,
            set: pending.set,
        })
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

    pub fn validate_pending(&self, pending: &PendingContributions) -> Result<(), ContractError> {
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
    pub fn compiler_classes(
        &mut self,
        owner: &TypeId,
    ) -> Result<Vec<CompiledClassHandle>, ContractError> {
        self.check_type(owner)?;
        let source = &self.store.sources[&owner.definition().file];
        self.reads
            .compiled
            .insert(owner.definition().file.clone(), source.compiled_revision);
        Ok(source
            .compiler_classes
            .iter()
            .filter(|(anchor, _)| anchor == owner)
            .map(|(_, input)| CompiledClassHandle {
                session: self.session,
                owner: owner.clone(),
                revision: source.compiled_revision,
                class: input.class.clone(),
                kotlin_methods: Some(input.kotlin_methods.clone()),
                value_namespace: input.value_namespace,
            })
            .collect())
    }

    /// The host's current frontend descriptor and the classfile's checked Signature must agree.
    pub fn admit_compiler_class(
        &mut self,
        handle: &CompiledClassHandle,
    ) -> Result<(), ContractError> {
        self.check_compiled_class(handle)?;
        let Some(surface) = &handle.kotlin_methods else {
            return Err(ContractError::ForeignIdentity);
        };
        let class = &handle.class;
        if handle.value_namespace {
            self.set
                .compiled_value_namespaces
                .push(CompiledValueNamespace {
                    owner: handle.owner.clone(),
                    class: class.clone(),
                });
        }
        if class.access_flags & 0x0001 == 0 || class.signature.is_some() {
            return Ok(());
        }
        for (method, raw) in class.methods.iter().enumerate() {
            if raw.access_flags & 0x0001 == 0
                || raw.access_flags & (0x0040 | 0x0080 | 0x1000) != 0
                || raw.name.starts_with('<')
                || !surface.contains(&format!("{}{}", raw.name, raw.descriptor))
            {
                continue;
            }
            let types = match &raw.signature {
                Some(signature) => {
                    crate::classfile::signature::method_signature(signature, &raw.descriptor, &[])
                        .filter(|signature| signature.type_parameters.is_empty())
                        .map(|signature| (signature.parameters, signature.returns))
                }
                None => crate::classfile::method_types(&raw.descriptor),
            };
            let Some((params, Some(return_type))) = types else {
                continue;
            };
            self.set.compiled.push(CompiledCallable {
                owner: handle.owner.clone(),
                class: class.clone(),
                method,
                facts: crate::jvm::CallableFacts {
                    name: raw.name.to_string(),
                    params,
                    return_type: Some(return_type),
                    type_params: Vec::new(),
                    definition: None,
                    fixed_name: None,
                    jvm_name_annotation: None,
                    is_static: raw.access_flags & 0x0008 != 0,
                    source_property: None,
                },
            });
        }
        for (field, raw) in class.fields.iter().enumerate() {
            if raw.access_flags & 0x0019 != 0x0019
                || raw.access_flags & 0x1000 != 0
                || raw.signature.is_some()
            {
                continue;
            }
            // Nested descriptors are usable only through the reciprocal class inventory above.
            let target = raw
                .descriptor
                .strip_prefix('L')
                .and_then(|d| d.strip_suffix(';'));
            let Some(target) = target else {
                continue;
            };
            let source = &self.store.sources[&handle.owner.definition().file];
            if !source
                .compiler_classes
                .iter()
                .any(|(_, input)| input.class.identity.internal_name == target)
            {
                continue;
            }
            let Some((package, binary)) = target.rsplit_once('/') else {
                continue;
            };
            let Some((container, name)) = binary.rsplit_once('$') else {
                continue;
            };
            self.set.compiled_fields.push(CompiledField {
                owner: handle.owner.clone(),
                class: class.clone(),
                field,
                companion: source.compiler_classes.iter().any(|(_, input)| {
                    input.class.identity.internal_name == target && input.companion
                }),
                ty: crate::types::TypeRef {
                    name: name.into(),
                    qualification: Some(format!(
                        "{}.{}",
                        package.replace('/', "."),
                        container.replace('$', ".")
                    )),
                    ..Default::default()
                },
            });
        }
        Ok(())
    }

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
                    kotlin_methods: None,
                    value_namespace: false,
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
            || method.signature.is_some()
        {
            return Ok(unknown(IncompleteReason::UnsupportedSource));
        }
        let Some((parameters, Some(return_type))) =
            crate::classfile::method_types(&method.descriptor)
        else {
            return Ok(unknown(IncompleteReason::UnsupportedSource));
        };
        let array = return_type.name.ends_with("[]");
        if !parameters.is_empty()
            || !array && method.descriptor.as_ref() != "()Ljava/lang/String;"
            || array
                && class
                    .attribute_names
                    .iter()
                    .chain(&method.attribute_names)
                    .any(|name| name.starts_with("Runtime") && name.ends_with("Annotations"))
        {
            return Ok(unknown(IncompleteReason::UnsupportedSource));
        }
        // The raw descriptor was validated above. Nominal array components do not become JvmType::String.
        let signature = MemberSignature {
            kind: MemberKind::Method,
            descriptor: method.descriptor.to_string(),
            references: Vec::new(),
        };
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
                return_type: Some(return_type),
                type_params: Vec::new(),
                definition: None,
                fixed_name: None,
                jvm_name_annotation: None,
                is_static: false,
                source_property: None,
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
            || !(source.compiled.as_ref().is_some_and(|(owner, current)| {
                owner == &class.owner && current.identity == class.class.identity
            }) || source.compiler_classes.iter().any(|(owner, input)| {
                owner == &class.owner
                    && input.class.identity == class.class.identity
                    && class.kotlin_methods.as_ref() == Some(&input.kotlin_methods)
                    && class.value_namespace == input.value_namespace
            }))
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

    /// The exact class/interface/object/enum declaration's own type, without name resolution.
    /// Members do not yield their value/return type or containing owner. Handles retain the
    /// existing host/source lifetime checks and must remain visible in this session's scope.
    pub fn declaration_type(
        &mut self,
        declaration: &DeclarationId,
    ) -> Result<Fact<TypeId>, ContractError> {
        self.check_declaration(declaration)?;
        self.reads.namespace = Some(self.store.namespace_generation);
        let entry = self
            .index
            .entries_for_file(&declaration.definition.file)
            .into_iter()
            .find(|entry| definition(entry) == declaration.definition);
        if !self.index.scope_is_complete() {
            return Ok(unknown(IncompleteReason::IncompleteScope));
        }
        let entry = entry.ok_or(ContractError::ForeignIdentity)?;
        if !matches!(
            entry.sym.kind,
            SymbolKind::Class | SymbolKind::Interface | SymbolKind::Object | SymbolKind::EnumClass
        ) {
            return Ok(unknown(IncompleteReason::UnsupportedType(
                entry.sym.name.clone(),
            )));
        }
        self.issue_type(&entry).map(Knowledge::Found)
    }

    /// The authored declaration's exact member, including its host-normalized signature.
    /// Same-name overloads and members of other owners never supply a correspondence.
    pub fn declaration_member(
        &mut self,
        declaration: &DeclarationId,
    ) -> Result<Fact<MemberCandidate>, ContractError> {
        self.check_declaration(declaration)?;
        self.reads.namespace = Some(self.store.namespace_generation);
        if !self.index.scope_is_complete() {
            return Ok(unknown(IncompleteReason::IncompleteScope));
        }
        let entries = self.index.entries_for_file(&declaration.definition.file);
        let exact: Vec<_> = entries
            .iter()
            .filter(|entry| definition(entry) == declaration.definition)
            .collect();
        let entry = match exact.as_slice() {
            [entry] => *entry,
            [] => return Err(ContractError::ForeignIdentity),
            _ => return Ok(unknown(IncompleteReason::AmbiguousMember)),
        };
        let Some(container) = entry
            .sym
            .container
            .as_deref()
            .filter(|_| member_kind(entry).is_some())
        else {
            return Ok(unknown(IncompleteReason::MissingFacts));
        };
        let owners: Vec<_> = entries
            .iter()
            .filter(|owner| {
                matches!(
                    owner.sym.kind,
                    SymbolKind::Class
                        | SymbolKind::Interface
                        | SymbolKind::Object
                        | SymbolKind::EnumClass
                ) && owner.sym.package == entry.sym.package
                    && owner.sym.container.as_ref().map_or_else(
                        || unquote(&owner.sym.name).to_string(),
                        |parent| format!("{parent}.{}", unquote(&owner.sym.name)),
                    ) == container
            })
            .collect();
        let owner = match owners.as_slice() {
            [owner] => self.issue_type(owner)?,
            [] => return Ok(unknown(IncompleteReason::MissingFacts)),
            _ => return Ok(unknown(IncompleteReason::AmbiguousType)),
        };
        let (members, coverage) = self.declared_members(&owner)?;
        let mut exact = members
            .into_iter()
            .filter(|member| member.id.definition() == &declaration.definition);
        match (exact.next(), exact.next()) {
            (Some(member), None) => {
                let mut reasons = Vec::new();
                add_unknown(&member.signature, &mut reasons);
                add_unknown(&member.dispatch, &mut reasons);
                add_unknown(&member.visibility, &mut reasons);
                add_unknown(&member.synthetic, &mut reasons);
                if reasons.is_empty() {
                    Ok(Knowledge::Found(member))
                } else {
                    Ok(Knowledge::Unknown(reasons))
                }
            }
            (Some(_), Some(_)) => Ok(unknown(IncompleteReason::AmbiguousMember)),
            _ => Ok(match coverage {
                Knowledge::Unknown(reasons) => Knowledge::Unknown(reasons),
                _ => unknown(IncompleteReason::MissingFacts),
            }),
        }
    }

    pub fn resolve_type(&mut self, qualified_name: &str) -> Fact<TypeId> {
        self.reads.namespace = Some(self.store.namespace_generation);
        let name = qualified_name.rsplit('.').next().unwrap_or(qualified_name);
        let entries = self
            .index
            .lookup_type(name)
            .into_iter()
            .filter(|entry| qualified_type_name(entry) == qualified_name)
            .collect();
        let entries: Vec<_> = narrow_archive_jvm_types(&self.index, entries)
            .into_iter()
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
        match self.issue_type(entry) {
            Ok(owner) => Knowledge::Found(owner),
            Err(ContractError::MissingSource) => {
                self.reads.missing_sources.insert(entry.path.to_string());
                unknown(IncompleteReason::MissingSource)
            }
            Err(_) => unknown(IncompleteReason::MissingSource),
        }
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
            crate::types::Type::Unknown
            | crate::types::Type::Raw(_)
            | crate::types::Type::Array { .. }
            | crate::types::Type::Parameter { .. }
            | crate::types::Type::Projected { .. }
            | crate::types::Type::Star => {
                unknown(IncompleteReason::UnsupportedType(ty.name.clone()))
            }
        }
    }

    /// Obtain an opaque JVM reference from one current, unambiguous host-issued source identity.
    pub fn reference_type(&mut self, owner: &TypeId) -> Result<Fact<JvmType>, ContractError> {
        self.check_type(owner)?;
        match self.resolve_type(owner.qualified_name()) {
            Knowledge::Found(current) if current == *owner => {}
            Knowledge::Unknown(reasons) => return Ok(Knowledge::Unknown(reasons)),
            _ => return Ok(unknown(IncompleteReason::IncompleteTarget)),
        }
        let source = &self.store.sources[&owner.definition().file];
        if !source
            .reference_types
            .contains(&(owner.definition().start_byte, owner.definition().end_byte))
        {
            return Ok(unknown(IncompleteReason::UnsupportedType(
                owner.qualified_name.clone(),
            )));
        }
        let name = owner.container.replace('.', "$");
        let internal_name = if owner.package == "kotlin" {
            match owner.container.as_str() {
                "Any" => "java/lang/Object".into(),
                "String" => "java/lang/String".into(),
                // shortcut: other builtin lowering/boxing needs signature context before issuance.
                _ => {
                    return Ok(unknown(IncompleteReason::UnsupportedType(
                        owner.qualified_name.clone(),
                    )));
                }
            }
        } else if owner.package.is_empty() {
            name
        } else {
            format!("{}/{}", owner.package.replace('.', "/"), name)
        };
        Ok(Knowledge::Found(JvmType::Reference(JvmReference {
            owner: owner.clone(),
            internal_name,
        })))
    }

    pub fn type_jvm(&mut self, owner: &TypeId) -> Result<TypeJvmFacts, ContractError> {
        let binary_name = match self.reference_type(owner)? {
            Knowledge::Found(JvmType::Reference(reference)) => Knowledge::Found(JvmBinaryName {
                descriptor: format!("L{};", reference.internal_name),
                internal_name: reference.internal_name,
            }),
            Knowledge::Unknown(reasons) => Knowledge::Unknown(reasons),
            _ => unknown(IncompleteReason::MissingFacts),
        };
        let (declared_members, declared_members_coverage) = self.declared_members(owner)?;
        let mut inputs = self.compiler_classes(owner)?;
        if let Knowledge::Found(class) = self.compiled_class(owner)? {
            inputs.push(class);
        }
        let mut abi = unknown(IncompleteReason::MissingFacts);
        if let Knowledge::Found(name) = &binary_name {
            for input in inputs {
                self.check_compiled_class(&input)?;
                if input.class.identity.internal_name != name.internal_name
                    || !matches!(input.class.identity.input, crate::jvm::InputIdentity::Binary { .. })
                {
                    continue;
                }
                if matches!(&abi, Knowledge::Found(previous) if previous != &input.class) {
                    abi = unknown(IncompleteReason::AmbiguousType);
                    break;
                }
                abi = Knowledge::Found(input.class);
            }
        }
        let class_flags = match &abi {
            Knowledge::Found(class) => Knowledge::Found(class.access_flags),
            Knowledge::Unknown(reasons) => Knowledge::Unknown(reasons.clone()),
            Knowledge::DefinitelyAbsent => unknown(IncompleteReason::MissingFacts),
        };
        Ok(TypeJvmFacts {
            binary_name,
            class_flags,
            declared_members,
            declared_members_coverage,
            abi,
            // Existing inputs attest declarations, not full runtime ABI versus headers/SDK stubs.
            jvm_abi_coverage: unknown(IncompleteReason::MissingFacts),
        })
    }

    fn source_jvm_type(
        &mut self,
        ty: &crate::types::TypeRef,
        context: &crate::infer::FileCtx,
        java: bool,
        parameters: &[String],
        container: Option<&str>,
    ) -> Option<JvmType> {
        if ty.nullable || ty.qualification.is_none() && parameters.contains(&ty.name) {
            return None;
        }
        if ty.qualification.is_none() {
            let mut lexical = container;
            while let Some(owner) = lexical {
                if self.index.lookup_type(&ty.name).iter().any(|entry| {
                    entry.sym.package == context.package
                        && entry.sym.container.as_deref() == Some(owner)
                }) {
                    let mut qualified = ty.clone();
                    qualified.qualification = Some(if context.package.is_empty() {
                        owner.into()
                    } else {
                        format!("{}.{owner}", context.package)
                    });
                    return self.source_jvm_type(&qualified, context, java, parameters, None);
                }
                lexical = owner.rsplit_once('.').map(|(outer, _)| outer);
            }
        }
        if java && ty.name.ends_with("[]") {
            let mut component = ty.clone();
            component.name = ty.name.strip_suffix("[]")?.to_string();
            return self
                .source_jvm_type(&component, context, java, parameters, container)
                .map(|element| JvmType::Array(Box::new(element)));
        }
        if !java
            && ty.name == "Array"
            && ty.args.len() == 1
            && (ty.qualification.as_deref() == Some("kotlin")
                || ty.qualification.is_none()
                    && ty
                        .package_candidates
                        .iter()
                        .any(|package| package == "kotlin")
                    && !self.index.lookup_type("Array").iter().any(|entry| {
                        entry.sym.container.is_none()
                            && entry.sym.package != "kotlin"
                            && (ty.package_candidates.contains(&entry.sym.package)
                                || entry.sym.package == context.package)
                    }))
        {
            let element =
                self.source_jvm_type(&ty.args[0], context, java, parameters, container)?;
            // Kotlin Array<Int> boxes its elements; do not pretend it is an int[].
            return matches!(
                element,
                JvmType::Reference(_) | JvmType::String | JvmType::Array(_)
            )
            .then(|| JvmType::Array(Box::new(element)));
        }
        if !ty.args.is_empty() {
            return None;
        }
        if let Some(scalar) =
            source::source_scalar_jvm_type(&self.index, ty, context, java, parameters)
        {
            return Some(scalar);
        }
        let crate::types::Type::Class {
            name,
            package,
            container,
            ..
        } = crate::infer::declared_type(&self.index, ty, context)
        else {
            return None;
        };
        let qualified = [
            package.unwrap_or_default(),
            container.unwrap_or_default(),
            name,
        ]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(".");
        let Knowledge::Found(owner) = self.resolve_type(&qualified) else {
            return None;
        };
        match self.reference_type(&owner).ok()? {
            Knowledge::Found(value) => Some(value),
            _ => None,
        }
    }

    fn source_member_signature(
        &mut self,
        entry: &Entry,
        facts: &DeclaredMemberFacts,
    ) -> Fact<MemberSignature> {
        if !matches!(&facts.signature, Knowledge::Unknown(reasons) if !reasons.is_empty()
            && reasons.iter().all(|reason| matches!(reason, IncompleteReason::UnsupportedType(_))))
        {
            return facts.signature.clone();
        }
        let source = &self.store.sources[entry.path.as_ref()];
        // Only the host's accepted source adapter can request this normalization. Other Unknown facts stay Unknown.
        if !source.declarations.iter().any(|declaration| {
            declaration.definition == facts.definition && declaration.member.as_ref() == Some(facts)
        }) || !matches!(facts.dispatch, Knowledge::Found(_))
            || !matches!(facts.visibility, Knowledge::Found(_))
            || facts.synthetic != Knowledge::Found(false)
        {
            return facts.signature.clone();
        }
        let context = crate::infer::FileCtx::new(entry.sym.package.clone(), Vec::new());
        let parameters = source::member_type_parameters(&self.index, &entry.path, &entry.sym);
        let signature = if entry.sym.kind == SymbolKind::Property {
            entry
                .sym
                .value_type
                .as_ref()
                .and_then(|ty| {
                    self.source_jvm_type(
                        ty,
                        &context,
                        entry.path.ends_with(".java"),
                        &parameters,
                        entry.sym.container.as_deref(),
                    )
                })
                .and_then(|ty| MemberSignature::field(&ty).ok())
        } else if !entry.sym.has_vararg
            && entry.sym.arity == Some(entry.sym.params.len().min(u8::MAX as usize) as u8)
        {
            let converted: Option<Vec<_>> = entry
                .sym
                .params
                .iter()
                .map(|ty| {
                    self.source_jvm_type(
                        ty,
                        &context,
                        entry.path.ends_with(".java"),
                        &parameters,
                        entry.sym.container.as_deref(),
                    )
                })
                .collect();
            let returns = entry.sym.return_type.as_ref().and_then(|ty| {
                self.source_jvm_type(
                    ty,
                    &context,
                    entry.path.ends_with(".java"),
                    &parameters,
                    entry.sym.container.as_deref(),
                )
            });
            match (converted, returns) {
                (Some(parameters), Some(returns)) => {
                    MemberSignature::method(&parameters, &returns).ok()
                }
                _ => None,
            }
        } else {
            None
        };
        signature
            .map(Knowledge::Found)
            .unwrap_or_else(|| facts.signature.clone())
    }

    pub fn declared_members(
        &mut self,
        target: &TypeId,
    ) -> Result<(Vec<MemberCandidate>, Fact<()>), ContractError> {
        self.check_type(target)?;
        let store = self.store;
        let source = &store.sources[&target.definition().file];
        self.reads
            .files
            .insert(target.definition().file.clone(), source.generation);
        let mut coverage = source.coverage.clone();
        let mut members = Vec::new();
        let entries: Vec<_> = self
            .index
            .members_of(&target.container)
            .into_iter()
            .cloned()
            .collect();
        for entry in &entries {
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
                |facts| self.source_member_signature(entry, facts),
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
        if let Knowledge::Found(signature) = &requirement.signature {
            for reference in &signature.references {
                self.check_type(reference)?;
            }
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
            session: self.session,
            reads: self.reads,
            set: self.set,
        }
    }

    fn issue_type(&mut self, entry: &Entry) -> Result<TypeId, ContractError> {
        Ok(TypeId {
            declaration: self.issue_declaration(entry)?,
            qualified_name: qualified_type_name(entry),
            package: entry.sym.package.clone(),
            container: entry.sym.container.as_ref().map_or_else(
                || unquote(&entry.sym.name).to_string(),
                |container| format!("{container}.{}", unquote(&entry.sym.name)),
            ),
            model_generation: self.store.model_generation,
        })
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

// A JVM source archive can carry common expect declarations beside its actual declarations.
// Only one verified archive view may narrow; unrelated or unauthoritative duplicates stay ambiguous.
fn narrow_archive_jvm_types<'a>(
    index: &dyn InferenceIndex,
    entries: Vec<&'a Entry>,
) -> Vec<&'a Entry> {
    if entries.len() < 2 {
        return entries;
    }
    let Some(origins) = index.origin_facts() else {
        return entries;
    };
    let first = entries[0];
    let Some(origin) = origins.file_origin(&first.path) else {
        return entries;
    };
    if origin.project_scope.is_some()
        || origin.representation != crate::project_model::SourceRepresentation::Source
    {
        return entries;
    }
    let archive_root = |path: &str| {
        let path = path.replace('\\', "/");
        ["/commonMain/", "/jvmMain/"]
            .into_iter()
            .find_map(|view| path.split_once(view).map(|(root, _)| root.to_string()))
    };
    let Some(root) = archive_root(&first.path) else {
        return entries;
    };
    if entries.iter().any(|entry| {
        origins.file_origin(&entry.path) != Some(origin)
            || archive_root(&entry.path).as_ref() != Some(&root)
            || entry.sym.kind != first.sym.kind
            || entry.sym.type_params != first.sym.type_params
    }) {
        return entries;
    }
    crate::resolve::narrow_source_set_entries(entries)
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

#[cfg(test)]
mod nominal_reference_tests {
    use super::*;
    use crate::{index::Index, index::Tier, indexer, parser::KotlinParser};

    #[test]
    fn prepared_reference_eligibility_applies_without_reparsing_and_keeps_generic_unknown() {
        const FILE: &str = "/background/Types.kt";
        let text: Arc<str> = Arc::from("package background\nclass Plain\nclass Box<T>\n");
        let tree = KotlinParser::new().parse(&text);
        assert!(!tree.root_node().has_error());
        let mut index = Index::new();
        index.replace_file(
            FILE,
            indexer::extract_symbols(&tree, &text, "background"),
            Tier::Durable,
        );
        let scope = indexer::TypeScope::with_default_imports("background", Vec::new(), &["kotlin"]);
        let declarations = source::extract(&index, FILE, &tree, &text, &scope);
        let prepared = source::prepare_source_references(&index, FILE, text, &tree);
        let mut store = ExtensionStore::default();
        store.configure_provider("prepared", true, Arc::from("{}"));
        let before = source::reference_parse_count();
        store
            .replace_prepared_source_declarations(
                &index,
                prepared,
                Knowledge::Found(()),
                declarations,
            )
            .unwrap();
        assert_eq!(
            source::reference_parse_count(),
            before,
            "background tree is reused during acceptance"
        );
        let mut read = store.begin(&index, "prepared", FILE).unwrap();
        let Knowledge::Found(plain) = read.resolve_type("background.Plain") else {
            panic!("plain type");
        };
        assert!(read.reference_type(&plain).unwrap().is_found());
        let Knowledge::Found(generic) = read.resolve_type("background.Box") else {
            panic!("generic type identity");
        };
        assert!(!read.reference_type(&generic).unwrap().is_found());
    }

    #[test]
    fn repeated_self_reference_reads_do_not_parse_and_source_edits_retract_eligibility() {
        const FILE: &str = "/perf/Self.kt";
        const PROVIDER: &str = "self-reference";
        let mut index = Index::new();
        let mut store = ExtensionStore::default();
        store.configure_provider(PROVIDER, true, Arc::from("{}"));
        let scope = indexer::TypeScope::with_default_imports("perf", Vec::new(), &["kotlin"]);
        let update = |text: &str, index: &mut Index, store: &mut ExtensionStore| {
            let tree = KotlinParser::new().parse(text);
            assert!(!tree.root_node().has_error());
            index.replace_file(
                FILE,
                indexer::extract_symbols(&tree, text, "perf"),
                Tier::Volatile,
            );
            let declarations = source::extract(index, FILE, &tree, text, &scope);
            store
                .replace_source_declarations(
                    index,
                    FILE,
                    Arc::from(text),
                    Knowledge::Found(()),
                    declarations,
                )
                .unwrap();
        };
        let methods = (0..16)
            .map(|number| format!("fun same{number}(a: Self, b: Self): Self = a\n"))
            .collect::<String>();
        let text = format!("package perf\nclass Self {{\n{methods}}}\n");
        let start = source::reference_parse_count();
        update(&text, &mut index, &mut store);
        let after_upsert = source::reference_parse_count();
        for _ in 0..4 {
            let mut read = store.begin(&index, PROVIDER, FILE).unwrap();
            let Knowledge::Found(owner) = read.resolve_type("perf.Self") else {
                panic!("Self identity")
            };
            let (members, _) = read.declared_members(&owner).unwrap();
            assert_eq!(members.len(), 16);
            assert!(members.iter().all(|member| matches!(&member.signature, Knowledge::Found(signature) if signature.descriptor() == "(Lperf/Self;Lperf/Self;)Lperf/Self;")));
        }
        assert_eq!(
            source::reference_parse_count() - after_upsert,
            0,
            "four reads of 16 self-referencing methods; upsert parses={}",
            after_upsert - start
        );
        assert_eq!(
            after_upsert - start,
            1,
            "one additional eligibility parse per source update"
        );
        update(
            "package perf\n@JvmInline value class Self(val value: Int)\n",
            &mut index,
            &mut store,
        );
        let after_edit = source::reference_parse_count();
        assert_eq!(after_edit - after_upsert, 1);
        let mut read = store.begin(&index, PROVIDER, FILE).unwrap();
        let Knowledge::Found(owner) = read.resolve_type("perf.Self") else {
            panic!("edited identity")
        };
        assert!(matches!(
            read.reference_type(&owner).unwrap(),
            Knowledge::Unknown(_)
        ));
        assert_eq!(source::reference_parse_count(), after_edit);
    }
}
