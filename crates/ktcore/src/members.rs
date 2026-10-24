//! Receiver member candidates and use-site selection, shared by resolution and inference.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::ControlFlow;

use crate::index::{Entry, InferenceIndex};
use crate::infer::{self, FileCtx};
use crate::resolve;
use crate::types::{Type, TypeRef};

#[derive(Clone, Debug)]
pub struct CallArguments {
    /// Missing/broken argument syntax cannot establish an arity mismatch.
    pub count: Option<usize>,
    /// Written argument order until `bind_arguments` establishes declaration order.
    pub types: Vec<Type>,
    /// Only named Kotlin calls allocate labels; None is the ordinary positional path.
    pub names: Option<Vec<Option<String>>>,
    pub has_spread: bool,
    pub trailing_lambda: bool,
    pub trailing_lambda_arguments: Option<TrailingLambdaArguments>,
}

#[derive(Clone, Debug)]
pub enum LambdaParameters {
    Implicit,
    /// Unannotated parameters have a contextual type; annotations must match that context.
    Explicit(Vec<Option<Type>>),
    Unknown,
}

#[derive(Clone, Debug)]
pub struct TrailingLambdaArguments {
    pub parameters: LambdaParameters,
    pub result: Type,
}

#[derive(Clone, Debug)]
pub enum MemberUse {
    Value,
    Call(CallArguments),
}

/// JLS 10.7 array members have no authored declaration span or importable owner.
pub const JAVA_ARRAY_MEMBERS: [(&str, crate::symbol::SymbolKind, Option<u8>); 2] = [
    ("length", crate::symbol::SymbolKind::Property, None),
    ("clone", crate::symbol::SymbolKind::Function, Some(0)),
];

pub fn java_array_member_type(receiver: &Type, name: &str, usage: &MemberUse) -> Option<Type> {
    if !matches!(receiver, Type::Array { .. }) {
        return None;
    }
    match (name, usage) {
        ("length", MemberUse::Value) => Some(Type::class("int", None)),
        ("clone", MemberUse::Call(arguments))
            if arguments.count == Some(0)
                && arguments.types.is_empty()
                && arguments.names.as_ref().is_none_or(Vec::is_empty)
                && !arguments.has_spread
                && !arguments.trailing_lambda =>
        {
            Some(receiver.clone())
        }
        _ => None,
    }
}

/// Kotlin's finite read surface on a structurally known Java array.
/// These intrinsics have no authored span; other Kotlin Array APIs remain unmodeled.
pub const KOTLIN_JAVA_ARRAY_MEMBERS: [(&str, crate::symbol::SymbolKind, Option<u8>); 3] = [
    ("size", crate::symbol::SymbolKind::Property, None),
    ("get", crate::symbol::SymbolKind::Function, Some(1)),
    ("clone", crate::symbol::SymbolKind::Function, Some(0)),
];

pub fn kotlin_java_array_member_type(
    receiver: &Type,
    name: &str,
    usage: &MemberUse,
) -> Option<Type> {
    let Type::Array {
        component,
        nullable: false,
    } = receiver
    else {
        return None;
    };
    match (name, usage) {
        ("size", MemberUse::Value) => Some(Type::class("Int", Some("kotlin".into()))),
        ("clone", MemberUse::Call(_)) => java_array_member_type(receiver, name, usage),
        ("get", MemberUse::Call(arguments))
            if arguments.count == Some(1)
                && arguments.types.len() == 1
                && matches!(&arguments.types[0], Type::Class {
                    name, package: Some(package), container: None, nullable: false, args,
                    owner: None,
                } if name == "Int" && package == "kotlin" && args.is_empty())
                && arguments.names.as_ref().is_none_or(Vec::is_empty)
                && !arguments.has_spread
                && !arguments.trailing_lambda =>
        {
            // A Java primitive array reads as the corresponding Kotlin scalar, never a homonym.
            if let Type::Class {
                name,
                package: None,
                container: None,
                nullable: false,
                args,
                ..
            } = &**component
            {
                let scalar = infer::primitive_type_name(name);
                if args.is_empty() && scalar != name.as_str() {
                    return Some(Type::class(scalar, Some("kotlin".into())));
                }
            }
            Some((**component).clone())
        }
        _ => None,
    }
}

/// Kotlin exposes exactly these selected java.lang.Enum accessors as properties.
/// Keep the original declaration entry so navigation and refactoring retain the real SDK span.
pub fn kotlin_enum_property_type_ref<'a>(
    index: &dyn InferenceIndex,
    receiver: &Type,
    entry: &'a Entry,
) -> Option<&'a TypeRef> {
    use crate::extensions::{Dispatch, Visibility};
    use crate::symbol::SymbolKind;
    let sym = &entry.sym;
    if !matches!(sym.name.as_str(), "name" | "ordinal")
        || sym.kind != SymbolKind::Function
        || sym.package != "java.lang"
        || sym.container.as_deref() != Some("Enum")
        || sym.arity != Some(0)
        || !sym.params.is_empty()
        || sym.source_member.dispatch() != Some(Dispatch::Instance)
        || sym.source_member.visibility() != Some(Visibility::Public)
    {
        return None;
    }
    let origin = index.origin_facts()?.file_origin(&entry.path)?;
    if !origin.origin_id.starts_with("jdk-api:") && !origin.origin_id.starts_with("jdk:") {
        return None;
    }
    let owners = index.lookup_type(receiver.name()?);
    let mut owners = owners.into_iter().filter(|owner| {
        receiver.package() == Some(owner.sym.package.as_str())
            && receiver.container() == owner.sym.container.as_deref()
    });
    let owner = owners.next()?;
    if owners.next().is_some() || owner.sym.kind != SymbolKind::EnumClass {
        return None;
    }
    let mut reaches_sdk_enum = false;
    let _ = infer::walk_supertypes(index, receiver, 32, |current| {
        if current.name() == Some("Enum")
            && current.package() == Some("java.lang")
            && current.container().is_none()
        {
            let declarations = index.lookup_type("Enum");
            let mut declarations = declarations
                .into_iter()
                .filter(|d| d.sym.package == "java.lang" && d.sym.container.is_none());
            reaches_sdk_enum = declarations.next().is_some_and(|d| d.path == entry.path)
                && declarations.next().is_none();
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    });
    if !reaches_sdk_enum {
        return None;
    }
    let result = sym.return_type.as_ref()?;
    let expected = match sym.name.as_str() {
        "name" => result.name == "String" && result.qualification.as_deref() == Some("java.lang"),
        "ordinal" => result.name == "int" && result.qualification.is_none(),
        _ => false,
    };
    (expected && !result.nullable && result.args.is_empty()).then_some(result)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applicability {
    Applicable,
    Unknown,
    Rejected,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CallMode {
    Kotlin,
    Java,
}

pub struct Selection<'a> {
    pub entries: Vec<&'a Entry>,
    pub applicability: Applicability,
}

/// A deliberately small Java read-property projection. Keep the getter Entry/Def intact.
/// shortcut: acronyms and boolean `is` accessors need verified naming rules before admission.
pub fn kotlin_java_getter_property_name(entry: &Entry) -> Option<String> {
    use crate::extensions::{Dispatch, Visibility};
    let sym = &entry.sym;
    let suffix = sym.name.strip_prefix("get")?;
    let first = *suffix.as_bytes().first()?;
    if !entry.path.ends_with(".java")
        || sym.kind != crate::symbol::SymbolKind::Function
        || sym.arity != Some(0)
        || !sym.params.is_empty()
        || !sym.type_params.is_empty()
        || sym.has_vararg
        || sym.extension_receiver.is_some()
        || sym.source_member.dispatch() != Some(Dispatch::Instance)
        || sym.source_member.visibility() != Some(Visibility::Public)
        || !first.is_ascii_uppercase()
        || suffix.as_bytes().get(1).is_some_and(u8::is_ascii_uppercase)
        || sym.return_type.as_ref().is_none_or(|ty| ty.name == "void")
    {
        return None;
    }
    Some(format!(
        "{}{}",
        (first as char).to_ascii_lowercase(),
        &suffix[1..]
    ))
}

pub struct Candidates<'a> {
    pub members: Vec<&'a Entry>,
    pub property_getters: Vec<&'a Entry>,
    pub extensions: Vec<&'a Entry>,
    local_scopes: Vec<Vec<&'a Entry>>,
    uncertain_receivers: BTreeSet<(&'a str, usize, usize)>,
    hierarchy_complete: bool,
    unavailable_members: Vec<(&'a str, crate::symbol::SymbolKind, Option<u8>)>,
}

#[derive(Clone, Copy)]
pub(crate) enum MemberName<'a> {
    Exact(&'a str),
    Prefix(&'a str),
    /// Java callable lookup also retains source properties under their derived getter name.
    JvmPrefix(&'a str),
}

impl MemberName<'_> {
    fn matches(self, name: &str) -> bool {
        match self {
            Self::Exact(written) => name == written,
            Self::Prefix(written) | Self::JvmPrefix(written) => name.starts_with(written),
        }
    }

    fn matches_member(self, name: &str, kind: crate::symbol::SymbolKind) -> bool {
        self.matches(name)
            || matches!(self, Self::JvmPrefix(_))
                && kind == crate::symbol::SymbolKind::Property
                && (crate::jvm::property_getter_name(name)
                    .is_some_and(|getter| self.matches(&getter))
                    || crate::jvm::property_setter_name(name)
                        .is_some_and(|setter| self.matches(&setter)))
    }
}

pub struct SuperCandidates<'a> {
    /// Each entry retains its instantiated direct-super receiver for generic substitution.
    pub providers: Vec<(Type, Vec<&'a Entry>)>,
    pub incomplete: bool,
}

/// Effective members callable through a Kotlin source `super` form.
pub fn super_candidates<'a>(
    index: &'a dyn InferenceIndex,
    receivers: Vec<Type>,
    prefix: &str,
    ctx: &FileCtx,
) -> SuperCandidates<'a> {
    super_matching_candidates(index, receivers, MemberName::Prefix(prefix), ctx)
}

// Missing ancestors above a concrete class declaration cannot override that declaration.
// shortcut: exact calls require a single fixed zero-argument declaration until overload proof exists.
fn partial_class_super_candidates<'a>(
    index: &'a dyn InferenceIndex,
    receivers: &[Type],
    name: MemberName<'_>,
    ctx: &FileCtx,
) -> Option<(Type, Vec<&'a Entry>)> {
    use crate::symbol::SymbolKind;
    let owner = |ty: &Type| {
        if !ty.args().is_empty() || ty.container().is_some() {
            return None;
        }
        let matches: Vec<_> = index
            .lookup_type(&infer::receiver_root_key(index, ty)?)
            .into_iter()
            .filter(|entry| {
                ty.package()
                    .is_none_or(|package| package == entry.sym.package)
                    && entry.sym.container.is_none()
                    && entry.sym.type_params.is_empty()
            })
            .collect();
        match matches.as_slice() {
            [entry] => Some(*entry),
            _ => None,
        }
    };
    let mut class = None;
    let mut interfaces = Vec::new();
    for receiver in receivers {
        match owner(receiver)?.sym.kind {
            SymbolKind::Class if class.is_none() => class = Some(receiver),
            SymbolKind::Interface => interfaces.push(receiver),
            _ => return None,
        }
    }
    let class = class?;
    let raw = collect_candidates(index, class, name, ctx, false);
    let absent_in_interface = |interface: &Type, name| {
        let pool = collect_candidates(index, interface, MemberName::Exact(name), ctx, false);
        pool.hierarchy_complete && pool.members.is_empty()
    };
    let mut members = Vec::new();
    for entry in &raw.members {
        if entry.sym.kind != SymbolKind::Function
            || !entry.sym.type_params.is_empty()
            || entry.sym.has_vararg
            || super_eligibility(entry, ctx) != Some(true)
            || matches!(name, MemberName::Exact(_))
                && (entry.sym.arity != Some(0) || !entry.sym.params.is_empty())
            || !interfaces
                .iter()
                .all(|interface| absent_in_interface(interface, &entry.sym.name))
        {
            continue;
        }
        let mut current = class.clone();
        for _ in 0..32 {
            let Some(declaration) = owner(&current) else {
                break;
            };
            if declaration
                .sym
                .unavailable_members
                .as_ref()
                .is_some_and(|members| members.iter().any(|(name, _, _)| *name == entry.sym.name))
            {
                break;
            }
            if entry.sym.package == declaration.sym.package
                && entry.sym.container.as_deref() == Some(declaration.sym.name.as_str())
            {
                if !matches!(name, MemberName::Exact(_))
                    || raw
                        .members
                        .iter()
                        .filter(|other| {
                            other.sym.name == entry.sym.name
                                && other.sym.package == declaration.sym.package
                                && other.sym.container.as_deref()
                                    == Some(declaration.sym.name.as_str())
                        })
                        .count()
                        == 1
                {
                    members.push(*entry);
                }
                break;
            }
            // A nearer declaration (including abstract or unsupported overrides) owns this name.
            if raw.members.iter().any(|other| {
                other.sym.name == entry.sym.name
                    && other.sym.package == declaration.sym.package
                    && other.sym.container.as_deref() == Some(declaration.sym.name.as_str())
            }) {
                break;
            }
            let mut next = None;
            let known = infer::direct_supertypes(index, &current)
                .into_iter()
                .all(|parent| match owner(&parent).map(|owner| owner.sym.kind) {
                    Some(SymbolKind::Class) if next.is_none() => {
                        next = Some(parent);
                        true
                    }
                    Some(SymbolKind::Interface) => absent_in_interface(&parent, &entry.sym.name),
                    _ => false,
                });
            let Some(parent) = next.filter(|_| known) else {
                break;
            };
            current = parent;
        }
    }
    (!members.is_empty()).then(|| (class.clone(), members))
}

/// Keep the effective source-super pool through later call/value selection. Returning only its
/// receiver and collecting ordinary members again would reintroduce private/abstract declarations.
pub fn super_member_candidates<'a>(
    index: &'a dyn InferenceIndex,
    receivers: Vec<Type>,
    name: &str,
    ctx: &FileCtx,
) -> Option<(Type, Candidates<'a>)> {
    let mut selected = super_matching_candidates(index, receivers, MemberName::Exact(name), ctx);
    if selected.providers.len() != 1 {
        return None;
    }
    let (receiver, members) = selected.providers.pop()?;
    Some((
        receiver,
        Candidates {
            members,
            property_getters: Vec::new(),
            extensions: Vec::new(),
            local_scopes: Vec::new(),
            uncertain_receivers: BTreeSet::new(),
            hierarchy_complete: !selected.incomplete,
            unavailable_members: Vec::new(),
        },
    ))
}

/// Provider choice is per exact name, after effective override and implementation selection.
pub(crate) fn unique_super_receiver(
    index: &dyn InferenceIndex,
    receivers: Vec<Type>,
    name: MemberName<'_>,
    ctx: &FileCtx,
) -> Type {
    let mut providers = super_matching_candidates(index, receivers, name, ctx).providers;
    if providers.len() == 1 {
        providers.pop().unwrap().0
    } else {
        Type::Unknown
    }
}

fn super_matching_candidates<'a>(
    index: &'a dyn InferenceIndex,
    receivers: Vec<Type>,
    name: MemberName<'_>,
    ctx: &FileCtx,
) -> SuperCandidates<'a> {
    let mut selected = resolved_super_matching_candidates(index, receivers.clone(), name, ctx);
    if selected.incomplete && selected.providers.is_empty() && receivers.len() > 1 {
        if let Some(provider) = partial_class_super_candidates(index, &receivers, name, ctx) {
            selected.providers.push(provider);
        }
    }
    selected
}

fn resolved_super_matching_candidates<'a>(
    index: &'a dyn InferenceIndex,
    receivers: Vec<Type>,
    name: MemberName<'_>,
    ctx: &FileCtx,
) -> SuperCandidates<'a> {
    let mut providers = Vec::new();
    let mut blocked = BTreeSet::new();
    let mut incomplete = false;
    let multiple = receivers.len() > 1;
    for receiver in receivers {
        let raw = collect_candidates(index, &receiver, name, ctx, false);
        if !raw.hierarchy_complete {
            incomplete = true;
            if multiple {
                // An unresolved direct provider could compete with any otherwise useful name.
                return SuperCandidates {
                    providers: Vec::new(),
                    incomplete,
                };
            }
        }
        let mut names: BTreeMap<&str, Vec<&Entry>> = BTreeMap::new();
        for entry in raw.members {
            names.entry(&entry.sym.name).or_default().push(entry);
        }
        let mut members = Vec::new();
        for (name, mut entries) in names {
            // Keep abstract/unknown overrides until dominance has hidden their ancestors.
            let mut uncertain =
                remove_overridden_members(index, &receiver, &mut entries, ctx, true);
            entries.retain(|entry| match super_eligibility(entry, ctx) {
                Some(eligible) => eligible,
                None => {
                    uncertain = true;
                    false
                }
            });
            if uncertain {
                blocked.insert(name);
                incomplete = true;
            } else {
                members.extend(entries);
            }
        }
        providers.push((receiver, members));
    }
    let mut owners = BTreeMap::new();
    for (provider, (_, entries)) in providers.iter().enumerate() {
        for entry in entries {
            if owners
                .insert(entry.sym.name.as_str(), provider)
                .is_some_and(|previous| previous != provider)
            {
                blocked.insert(entry.sym.name.as_str());
                incomplete = true;
            }
        }
    }
    for (_, entries) in &mut providers {
        entries.retain(|entry| !blocked.contains(entry.sym.name.as_str()));
    }
    providers.retain(|(_, entries)| !entries.is_empty());
    SuperCandidates {
        providers,
        incomplete,
    }
}

fn super_eligibility(entry: &Entry, ctx: &FileCtx) -> Option<bool> {
    use crate::extensions::{Dispatch, Visibility};
    use crate::symbol::MemberImplementation;
    let flags = entry.sym.source_member;
    if matches!(
        flags.implementation(),
        Some(MemberImplementation::Abstract | MemberImplementation::Constructor)
    ) || flags.dispatch() == Some(Dispatch::Static)
        || flags.visibility() == Some(Visibility::Private)
    {
        return Some(false);
    }
    if flags.implementation() != Some(MemberImplementation::Concrete)
        || flags.dispatch() != Some(Dispatch::Instance)
    {
        return None;
    }
    match flags.visibility() {
        Some(Visibility::Public | Visibility::Protected) => Some(true),
        // Kotlin2.4 permits same-package access even through a foreign-package
        // intermediate superclass; javac uses different inheritance visibility.
        Some(Visibility::Package) => Some(ctx.package == entry.sym.package),
        Some(Visibility::Private) => Some(false),
        // shortcut: source internal is known, but module/friend visibility is not certified here.
        None => None,
    }
}

/// Select a bare member/extension call through the same innermost-first receiver stack used by
/// navigation. Rejected inner declarations cannot hide an applicable outer receiver, but remain
/// available for useful invalid-call navigation when no receiver supplies a viable declaration.
pub fn select_implicit<'a>(
    index: &'a dyn InferenceIndex,
    receivers: impl IntoIterator<Item = Type>,
    name: &str,
    usage: &MemberUse,
    ctx: &'a FileCtx,
) -> Option<(Type, Selection<'a>)> {
    select_implicit_in(index, receivers, name, usage, ctx, None)
}

pub fn select_implicit_at<'a>(
    index: &'a dyn InferenceIndex,
    receivers: impl IntoIterator<Item = Type>,
    name: &str,
    usage: &MemberUse,
    ctx: &'a FileCtx,
    position: usize,
) -> Option<(Type, Selection<'a>)> {
    select_implicit_in(index, receivers, name, usage, ctx, Some(position))
}

fn select_implicit_in<'a>(
    index: &'a dyn InferenceIndex,
    receivers: impl IntoIterator<Item = Type>,
    name: &str,
    usage: &MemberUse,
    ctx: &'a FileCtx,
    position: Option<usize>,
) -> Option<(Type, Selection<'a>)> {
    let mut rejected = None;
    for receiver in receivers {
        let pool = if matches!(usage, MemberUse::Value) {
            value_candidates(index, &receiver, name, ctx, position)
        } else {
            match position {
                Some(position) => candidates_at(index, &receiver, name, ctx, position),
                None => candidates(index, &receiver, name, ctx),
            }
        };
        let selection = pool.select(index, &receiver, usage, ctx);
        if selection.entries.is_empty() && selection.applicability != Applicability::Unknown {
            continue;
        }
        if selection.applicability != Applicability::Rejected {
            return Some((receiver, selection));
        }
        if rejected.is_none() {
            rejected = Some((receiver, selection));
        }
    }
    rejected
}

/// One typed traversal for own/inherited members and visible extensions. Artifact ownership is
/// provided by the borrowed index view; no language or project-specific fallback belongs here.
pub fn candidates<'a>(
    index: &'a dyn InferenceIndex,
    receiver: &Type,
    name: &str,
    ctx: &FileCtx,
) -> Candidates<'a> {
    matching_candidates(index, receiver, MemberName::Exact(name), ctx, true)
}

pub fn candidates_at<'a>(
    index: &'a dyn InferenceIndex,
    receiver: &Type,
    name: &str,
    ctx: &'a FileCtx,
    position: usize,
) -> Candidates<'a> {
    matching_candidates_at(
        index,
        receiver,
        MemberName::Exact(name),
        ctx,
        position,
        true,
    )
}

pub fn completion_candidates_at<'a>(
    index: &'a dyn InferenceIndex,
    receiver: &Type,
    prefix: &str,
    ctx: &'a FileCtx,
    position: usize,
    include_extensions: bool,
) -> Candidates<'a> {
    let mut result = matching_candidates_at(
        index,
        receiver,
        MemberName::Prefix(prefix),
        ctx,
        position,
        include_extensions,
    );
    add_property_getters(
        index,
        receiver,
        MemberName::Prefix(prefix),
        ctx,
        Some(position),
        &mut result,
    );
    result
}

fn matching_candidates_at<'a>(
    index: &'a dyn InferenceIndex,
    receiver: &Type,
    name: MemberName<'_>,
    ctx: &'a FileCtx,
    position: usize,
    include_extensions: bool,
) -> Candidates<'a> {
    let mut result = matching_candidates(index, receiver, name, ctx, include_extensions);
    result
        .members
        .retain(|entry| member_accessible_at(index, receiver, entry, ctx, position));
    result
        .property_getters
        .retain(|entry| member_accessible_at(index, receiver, entry, ctx, position));
    if !include_extensions {
        return result;
    }
    let mut scopes = BTreeMap::new();
    for local in ctx.local_extensions.iter().flat_map(|locals| locals.iter()) {
        if !local.scope.contains(&position)
            || local.entry.sym.end_byte > position
            || !name.matches(&local.entry.sym.name)
        {
            continue;
        }
        let mut applicability = local_receiver_applicability(index, receiver, local, ctx);
        if applicability != Applicability::Rejected && local.entry.sym.extension_receiver.is_some()
        {
            applicability = extension_receiver_applicability(index, receiver, &local.entry);
        }
        if applicability == Applicability::Rejected {
            continue;
        }
        if result
            .unavailable_members
            .iter()
            .any(|(name, kind, _)| *name == local.entry.sym.name && *kind == local.entry.sym.kind)
        {
            continue;
        }
        if applicability == Applicability::Unknown {
            result.uncertain_receivers.insert((
                local.entry.path.as_ref(),
                local.entry.sym.start_byte,
                local.entry.sym.end_byte,
            ));
        }
        scopes
            .entry((local.scope.len(), local.scope.start))
            .or_insert_with(Vec::new)
            .push(&local.entry);
    }
    result.local_scopes = scopes.into_values().collect();
    result
}

fn local_receiver_applicability(
    index: &dyn InferenceIndex,
    receiver: &Type,
    local: &crate::indexer::LocalExtension,
    ctx: &FileCtx,
) -> Applicability {
    if local.has_type_constraints {
        // shortcut: where clauses and projected receiver constraints remain unmodeled.
        return Applicability::Unknown;
    }
    if local.receiver.qualification.is_none()
        && local.entry.sym.type_params.contains(&local.receiver.name)
    {
        return if infer::type_head_is_known(index, receiver) {
            Applicability::Applicable
        } else {
            Applicability::Unknown
        };
    }
    let formal = infer::declared_type(index, &local.receiver, ctx);
    if !infer::type_head_is_known(index, &formal) || receiver.name().is_none() {
        return Applicability::Unknown;
    }
    if receiver.is_nullable() && !formal.is_nullable() {
        return Applicability::Rejected;
    }
    let mut uncertain = false;
    let found = infer::walk_supertypes(index, receiver, 32, |current| {
        uncertain |= !infer::type_head_is_known(index, current);
        if current.name() == formal.name()
            && current.package() == formal.package()
            && current.container() == formal.container()
        {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    })
    .is_break();
    if found {
        // Type-argument constraints/variance are not a proof of receiver applicability yet.
        if local.receiver.args.is_empty() {
            Applicability::Applicable
        } else {
            Applicability::Unknown
        }
    } else if uncertain {
        Applicability::Unknown
    } else {
        Applicability::Rejected
    }
}

/// Structured receiver proof shared by inference, navigation and editor completion. A failed exact
/// template match cannot prove rejection while declaration-site variance remains unmodeled.
pub fn extension_receiver_applicability(
    index: &dyn InferenceIndex,
    receiver: &Type,
    entry: &Entry,
) -> Applicability {
    // shortcut: bounded receivers need extension-constraint proof before exposing extensions.
    if matches!(receiver, Type::Parameter { .. }) {
        return Applicability::Unknown;
    }
    if entry.sym.extension_receiver.is_none()
        || infer::receiver_type_bindings(index, receiver, entry).is_some()
    {
        Applicability::Applicable
    } else {
        Applicability::Unknown
    }
}

pub(crate) fn matching_candidates<'a>(
    index: &'a dyn InferenceIndex,
    receiver: &Type,
    name: MemberName<'_>,
    ctx: &FileCtx,
    include_extensions: bool,
) -> Candidates<'a> {
    let mut result = collect_candidates(index, receiver, name, ctx, include_extensions);
    remove_overridden_members(index, receiver, &mut result.members, ctx, false);
    result
}

fn add_property_getters<'a>(
    index: &'a dyn InferenceIndex,
    receiver: &Type,
    name: MemberName<'_>,
    ctx: &FileCtx,
    position: Option<usize>,
    result: &mut Candidates<'a>,
) {
    let written = match name {
        MemberName::Exact(name) | MemberName::Prefix(name) | MemberName::JvmPrefix(name) => name,
    };
    let getter_name = if written.is_empty() {
        Some("get".to_string())
    } else if written.as_bytes()[0].is_ascii_lowercase() {
        Some(format!(
            "get{}{}",
            (written.as_bytes()[0] as char).to_ascii_uppercase(),
            &written[1..]
        ))
    } else {
        None
    };
    if let Some(getter_name) = getter_name {
        let getter_query = match name {
            MemberName::Exact(_) => MemberName::Exact(&getter_name),
            MemberName::Prefix(_) | MemberName::JvmPrefix(_) => MemberName::Prefix(&getter_name),
        };
        let mut getters = collect_candidates(index, receiver, getter_query, ctx, false);
        remove_overridden_members(index, receiver, &mut getters.members, ctx, false);
        result.hierarchy_complete &= getters.hierarchy_complete;
        result
            .unavailable_members
            .extend(getters.unavailable_members);
        result.property_getters = getters
            .members
            .into_iter()
            .filter(|entry| {
                kotlin_java_getter_property_name(entry)
                    .is_some_and(|property| name.matches(&property))
            })
            .collect();
        let mut counts = BTreeMap::new();
        for entry in &result.property_getters {
            *counts
                .entry(kotlin_java_getter_property_name(entry).unwrap())
                .or_insert(0usize) += 1;
        }
        result
            .property_getters
            .retain(|entry| counts[&kotlin_java_getter_property_name(entry).unwrap()] == 1);
    }
    if let Some(position) = position {
        result
            .property_getters
            .retain(|entry| member_accessible_at(index, receiver, entry, ctx, position));
    }
}

/// Value queries discover getter aliases only when the call site uses property syntax.
pub fn value_candidates<'a>(
    index: &'a dyn InferenceIndex,
    receiver: &Type,
    name: &str,
    ctx: &'a FileCtx,
    position: Option<usize>,
) -> Candidates<'a> {
    let mut result = match position {
        Some(position) => candidates_at(index, receiver, name, ctx, position),
        None => candidates(index, receiver, name, ctx),
    };
    // An accessible ordinary value wins before any getter projection.
    if !result
        .members
        .iter()
        .any(|entry| entry.sym.kind.is_value_like())
    {
        add_property_getters(
            index,
            receiver,
            MemberName::Exact(name),
            ctx,
            position,
            &mut result,
        );
    }
    result
}

fn collect_candidates<'a>(
    index: &'a dyn InferenceIndex,
    receiver: &Type,
    name: MemberName<'_>,
    ctx: &FileCtx,
    include_extensions: bool,
) -> Candidates<'a> {
    let mut members = Vec::new();
    let mut extensions = Vec::new();
    let mut hierarchy_complete = true;
    let mut blockers = Vec::new();
    let mut unavailable_members = Vec::new();
    let _ = infer::walk_supertypes(index, receiver, 32, |current| {
        let Some(root) = infer::receiver_root_key(index, current) else {
            hierarchy_complete = false;
            return ControlFlow::Continue(());
        };
        for owner in index.lookup_type(&root).into_iter().filter(|owner| {
            current.package().is_none_or(|pkg| owner.sym.package == pkg)
                && current.container() == owner.sym.container.as_deref()
        }) {
            if let Some(names) = owner.sym.unavailable_members.as_deref() {
                if names
                    .iter()
                    .any(|(written, kind, _)| name.matches_member(written, *kind))
                {
                    blockers.push((current.clone(), names));
                    unavailable_members.extend(
                        names
                            .iter()
                            .filter(|(written, kind, _)| name.matches_member(written, *kind))
                            .map(|(written, kind, arity)| (written.as_str(), *kind, *arity)),
                    );
                    hierarchy_complete = false;
                }
            }
        }
        members.extend(index.members_of(&root).into_iter().filter(|entry| {
            name.matches_member(&entry.sym.name, entry.sym.kind)
                && current.package().is_none_or(|pkg| entry.sym.package == pkg)
                && receiver_member_accessible(receiver, entry)
        }));
        if include_extensions {
            extensions.extend(index.extensions_for(&root).into_iter().filter(|entry| {
                resolve::visible_extension_names(entry, &ctx.imports, &ctx.package)
                    .any(|visible_name| name.matches(visible_name))
            }));
        }
        ControlFlow::Continue(())
    });
    for (owner, names) in blockers {
        let mut shadowed_owners: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let _ = infer::walk_supertypes(index, &owner, 32, |current| {
            if let (Some(package), Some(root)) =
                (current.package(), infer::receiver_root_key(index, current))
            {
                shadowed_owners
                    .entry(package.to_string())
                    .or_default()
                    .insert(root);
            }
            ControlFlow::Continue(())
        });
        // shortcut: same-arity unknown overloads block ancestors until exact signature facts exist.
        members.retain(|entry| {
            !names.iter().any(|(written, kind, arity)| {
                *written == entry.sym.name
                    && *kind == entry.sym.kind
                    && (*kind != crate::symbol::SymbolKind::Function
                        || arity.is_none()
                        || entry.sym.arity.is_none()
                        || entry.sym.arity == *arity)
            }) || !entry.sym.container.as_ref().is_some_and(|container| {
                shadowed_owners
                    .get(entry.sym.package.as_str())
                    .is_some_and(|owners| owners.contains(container))
            })
        });
    }
    if let (true, MemberName::Exact(name)) = (include_extensions, name) {
        extensions.extend(resolve::generic_receiver_extensions(
            index,
            name,
            &ctx.imports,
            &ctx.package,
        ));
    }
    if let (true, MemberName::Prefix(prefix)) = (include_extensions, name) {
        extensions.extend(
            index
                .generic_receiver_extensions(prefix)
                .into_iter()
                .chain(
                    ctx.imports
                        .iter()
                        .filter(|import| {
                            import
                                .alias
                                .as_deref()
                                .is_some_and(|alias| alias.starts_with(prefix))
                        })
                        .flat_map(|import| index.lookup_by_name(import.simple_name())),
                )
                .filter(|entry| crate::index::is_generic_receiver_extension(entry))
                .filter(|entry| {
                    resolve::visible_extension_names(entry, &ctx.imports, &ctx.package)
                        .any(|visible| visible.starts_with(prefix))
                }),
        );
    }
    extensions.retain(|entry| {
        !unavailable_members.iter().any(|(name, kind, _)| {
            *kind == entry.sym.kind
                && resolve::visible_extension_names(entry, &ctx.imports, &ctx.package)
                    .any(|visible| visible == *name)
        })
    });
    // A diamond or a generic extension reached through several supertypes is still one declaration.
    for entries in [&mut members, &mut extensions] {
        entries.sort_by_key(|entry| {
            (
                entry.path.as_ref(),
                entry.sym.start_byte,
                entry.sym.end_byte,
            )
        });
        entries.dedup_by(|a, b| {
            a.path == b.path
                && a.sym.start_byte == b.sym.start_byte
                && a.sym.end_byte == b.sym.end_byte
        });
    }
    let uncertain_receivers = extensions
        .iter()
        .filter(|entry| {
            extension_receiver_applicability(index, receiver, entry) == Applicability::Unknown
        })
        .map(|entry| {
            (
                entry.path.as_ref(),
                entry.sym.start_byte,
                entry.sym.end_byte,
            )
        })
        .collect();
    Candidates {
        members,
        property_getters: Vec::new(),
        extensions,
        local_scopes: Vec::new(),
        uncertain_receivers,
        hierarchy_complete,
        unavailable_members,
    }
}

/// Bound surfaces must not borrow inaccessible declarations from their nominal upper bounds.
pub fn receiver_member_accessible(receiver: &Type, entry: &Entry) -> bool {
    use crate::extensions::{Dispatch, Visibility};
    // shortcut: bound receivers expose public instance members until access and static dispatch are modeled.
    !matches!(receiver, Type::Parameter { .. })
        || entry.sym.source_member.visibility() == Some(Visibility::Public)
            && entry.sym.source_member.dispatch() == Some(Dispatch::Instance)
            && entry.sym.source_member.implementation()
                != Some(crate::symbol::MemberImplementation::Constructor)
            && entry.sym.ext_receiver.is_none()
            && entry.sym.extension_receiver.is_none()
}

/// Shared caller access for normalized source and classfile-backed declarations.
pub fn member_accessible_at(
    index: &dyn InferenceIndex,
    receiver: &Type,
    entry: &Entry,
    ctx: &FileCtx,
    position: usize,
) -> bool {
    use crate::extensions::{Dispatch, Visibility};
    let flags = entry.sym.source_member;
    let visibility = flags.visibility();
    if visibility == Some(Visibility::Public) || visibility.is_none() {
        return true;
    }
    if visibility == Some(Visibility::Package) {
        return ctx.package == entry.sym.package;
    }
    if visibility == Some(Visibility::Protected)
        && flags.protected_package_access()
        && ctx.package == entry.sym.package
    {
        return true;
    }
    let Some(caller) = ctx.caller_type(index, position) else {
        return false;
    };
    let owner_matches = |ty: &Type| {
        ty.package() == Some(entry.sym.package.as_str())
            && infer::receiver_root_key(index, ty).as_deref() == entry.sym.container.as_deref()
    };
    if visibility == Some(Visibility::Private) {
        return owner_matches(&caller);
    }
    let subclass = infer::walk_supertypes(index, &caller, 32, |current| {
        if owner_matches(current) {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    })
    .is_break();
    if !subclass {
        return false;
    }
    if flags.dispatch() == Some(Dispatch::Static) {
        return true;
    }
    if ctx.super_access_at(position)
        && infer::direct_supertypes(index, &caller)
            .iter()
            .any(|parent| parent == receiver)
    {
        return true;
    }
    infer::walk_supertypes(index, receiver, 32, |current| {
        if current.name() == caller.name()
            && current.package() == caller.package()
            && current.container() == caller.container()
        {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    })
    .is_break()
}

// shortcut: static/visibility validity needs declaration metadata before invalid overrides can be diagnosed.
fn remove_overridden_members(
    index: &dyn InferenceIndex,
    receiver: &Type,
    members: &mut Vec<&Entry>,
    ctx: &FileCtx,
    source_super: bool,
) -> bool {
    if members.len() < 2 {
        return false;
    }
    let signatures: Vec<Option<Vec<Type>>> = members
        .iter()
        .map(|entry| {
            if entry.sym.kind != crate::symbol::SymbolKind::Function {
                return None;
            }
            if !members.iter().any(|other| {
                other.sym.name == entry.sym.name
                    && other.sym.arity == entry.sym.arity
                    && (other.sym.container != entry.sym.container
                        || other.sym.package != entry.sym.package)
            }) {
                return None;
            }
            method_signature(index, receiver, entry, ctx)
        })
        .collect();
    let mut keep = vec![true; members.len()];
    let mut uncertain = false;
    for (position, inherited) in members.iter().enumerate() {
        keep[position] = !members.iter().enumerate().any(|(other, nearer)| {
            if other == position {
                return false;
            }
            let metadata = source_super.then(|| super_override_relation(nearer, inherited));
            if metadata
                .as_ref()
                .is_some_and(|relation| relation.is_definitely_absent())
            {
                return false;
            }
            let mut relation = if nearer.sym.kind == crate::symbol::SymbolKind::Property
                && inherited.sym.kind == crate::symbol::SymbolKind::Property
            {
                value_inheritance_relation(index, nearer, inherited)
            } else {
                signature_inheritance_relation(
                    index,
                    receiver,
                    nearer,
                    inherited,
                    &signatures[other],
                    &signatures[position],
                )
            };
            if relation.is_found() {
                if let Some(metadata) = metadata {
                    relation = metadata;
                }
            }
            uncertain |= source_super && !relation.is_found() && !relation.is_definitely_absent();
            relation.is_found()
        });
    }
    let mut position = 0;
    members.retain(|_| {
        let retain = keep[position];
        position += 1;
        retain
    });
    uncertain
}

fn value_inheritance_relation(
    index: &dyn InferenceIndex,
    child: &Entry,
    parent: &Entry,
) -> resolve::ResolutionStatus<()> {
    use crate::knowledge::Knowledge;
    if child.sym.name != parent.sym.name
        || child.sym.ext_receiver.is_some()
        || parent.sym.ext_receiver.is_some()
    {
        return Knowledge::DefinitelyAbsent;
    }
    let owner = owner_inheritance(index, child, parent);
    if owner.is_definitely_absent() {
        return owner;
    }
    if owner.is_found() && owner_inheritance(index, parent, child).is_definitely_absent() {
        return Knowledge::Found(());
    }
    Knowledge::Unknown(vec![resolve::IncompletenessReason::UnknownReceiverType])
}

fn super_override_relation(child: &Entry, parent: &Entry) -> resolve::ResolutionStatus<()> {
    use crate::extensions::{Dispatch, Visibility};
    use crate::knowledge::Knowledge;
    use crate::symbol::MemberImplementation;
    let child_flags = child.sym.source_member;
    let parent_flags = parent.sym.source_member;
    if [child_flags, parent_flags]
        .iter()
        .any(|flags| flags.implementation() == Some(MemberImplementation::Constructor))
        || parent_flags.visibility() == Some(Visibility::Private)
        || parent_flags.visibility() == Some(Visibility::Package)
            && child.sym.package != parent.sym.package
    {
        return Knowledge::DefinitelyAbsent;
    }
    let access_valid = match (child_flags.visibility(), parent_flags.visibility()) {
        (Some(Visibility::Public), Some(_)) => true,
        (Some(Visibility::Protected), Some(Visibility::Protected | Visibility::Package)) => true,
        (Some(Visibility::Package), Some(Visibility::Package)) => true,
        _ => false,
    };
    if child_flags.implementation().is_some()
        && parent_flags.implementation().is_some()
        && matches!(
            (child_flags.dispatch(), parent_flags.dispatch()),
            (Some(Dispatch::Instance), Some(Dispatch::Instance))
                | (Some(Dispatch::Static), Some(Dispatch::Static))
        )
        && access_valid
    {
        Knowledge::Found(())
    } else {
        Knowledge::Unknown(vec![
            resolve::IncompletenessReason::MemberSurfaceIncomplete(crate::symbol::Def {
                file: child.path.to_string(),
                start_byte: child.sym.start_byte,
                end_byte: child.sym.end_byte,
            }),
        ])
    }
}

/// Known parameter identities after substituting the receiver's owning-class arguments.
/// Unbound type variables and incomplete parameter facts cannot prove an override relation.
pub fn method_signature(
    index: &dyn InferenceIndex,
    receiver: &Type,
    entry: &Entry,
    ctx: &FileCtx,
) -> Option<Vec<Type>> {
    if entry.sym.kind != crate::symbol::SymbolKind::Function
        || !entry.sym.type_params.is_empty()
        || entry.sym.arity.map(usize::from) != Some(entry.sym.params.len())
    {
        return None;
    }
    entry
        .sym
        .params
        .iter()
        .map(|parameter| {
            parameter_identity(
                index,
                // Override identities use the JVM declaration view, independent of caller language.
                infer::declared_member_type(index, receiver, entry, parameter, CallMode::Java, ctx),
            )
        })
        .collect()
}

/// Signature and strict declaring-owner relationship, shared by lookup and host refactoring.
/// Dispatch/access must be checked against accepted source facts before treating this as an override.
pub fn inherited_method_relation(
    index: &dyn InferenceIndex,
    receiver: &Type,
    child: &Entry,
    parent: &Entry,
    ctx: &FileCtx,
) -> resolve::ResolutionStatus<()> {
    signature_inheritance_relation(
        index,
        receiver,
        child,
        parent,
        &method_signature(index, receiver, child, ctx),
        &method_signature(index, receiver, parent, ctx),
    )
}

/// Missing ordinary interface methods for one concrete source class. Unsupported signatures,
/// provider conflicts and incomplete owners stay unknown rather than authorizing a guessed stub.
/// The host supplies the complete Object inventory from the caller's selected target SDK.
pub fn required_interface_methods<'a>(
    index: &'a dyn InferenceIndex,
    owner: &Entry,
    object: &crate::classfile::ClassFileFacts,
    facts: &resolve::CompletenessFacts,
    source_owner_known: &dyn Fn(&Entry) -> bool,
) -> resolve::ResolutionStatus<Vec<(&'a Entry, crate::extensions::JvmType)>> {
    use crate::extensions::{Dispatch, Visibility};
    use crate::symbol::{MemberImplementation, SymbolKind};
    use resolve::{IncompletenessReason, ResolutionStatus};

    let unknown = || {
        ResolutionStatus::Unknown(vec![IncompletenessReason::MemberSurfaceIncomplete(
            crate::symbol::Def {
                file: owner.path.to_string(),
                start_byte: owner.sym.start_byte,
                end_byte: owner.sym.end_byte,
            },
        )])
    };
    if !index.scope_is_complete()
        || owner.sym.kind != SymbolKind::Class
        || owner.sym.container.is_some()
        || !owner.sym.type_params.is_empty()
        || object.identity.internal_name != "java/lang/Object"
        || !matches!(
            object.identity.input,
            crate::jvm::InputIdentity::Binary { .. }
        )
        || object.super_name.is_some()
        || !object.interfaces.is_empty()
        || object.signature.is_some()
        || object.access_flags & 0x0001 == 0
        || object.access_flags & 0xe600 != 0
    {
        return unknown();
    }
    let receiver = Type::class(&owner.sym.name, Some(owner.sym.package.clone()));
    let ctx = FileCtx::new(owner.sym.package.clone(), Vec::new());
    let mut declarations = Vec::new();
    let mut incomplete = false;
    let _ = infer::walk_supertypes(index, &receiver, 32, |current| {
        let Some(name) = current.name() else {
            incomplete = true;
            return ControlFlow::Break(());
        };
        let mut matching = index.lookup_type(name).into_iter().filter(|entry| {
            current.package() == Some(entry.sym.package.as_str())
                && current.container() == entry.sym.container.as_deref()
        });
        let Some(declaration) = matching.next() else {
            incomplete = true;
            return ControlFlow::Break(());
        };
        if matching.next().is_some()
            || current.container().is_some()
            || !current.args().is_empty()
            || !declaration.sym.type_params.is_empty()
            || !index.contributions_for_file(&declaration.path).is_empty()
            || facts.member_surfaces_unknown.contains(&crate::symbol::Def {
                file: declaration.path.to_string(),
                start_byte: declaration.sym.start_byte,
                end_byte: declaration.sym.end_byte,
            })
            || !source_owner_known(declaration)
            || if current == &receiver {
                declaration.path != owner.path
                    || declaration.sym.start_byte != owner.sym.start_byte
                    || declaration.sym.end_byte != owner.sym.end_byte
            } else {
                declaration.sym.kind != SymbolKind::Interface
            }
        {
            incomplete = true;
            return ControlFlow::Break(());
        }
        declarations.push(declaration);
        ControlFlow::Continue(())
    });
    if incomplete {
        return unknown();
    }
    let mut methods: BTreeMap<&str, Vec<(&Entry, crate::extensions::JvmType)>> = BTreeMap::new();
    for declaration in declarations {
        for entry in index.members_of(&declaration.sym.name) {
            if entry.path != declaration.path
                || entry.sym.package != declaration.sym.package
                || entry.sym.container.as_deref() != Some(declaration.sym.name.as_str())
                || entry.sym.kind.is_type_like()
                || entry.sym.source_member.implementation()
                    == Some(MemberImplementation::Constructor)
            {
                continue;
            }
            let flags = entry.sym.source_member;
            let inherited = declaration.sym.kind == SymbolKind::Interface;
            if inherited
                && !entry.path.ends_with(".java")
                && flags.implementation() == Some(MemberImplementation::Concrete)
            {
                // Kotlin JVM-default producer modes are not yet part of this source query.
                return unknown();
            }
            if inherited
                && (flags.dispatch() == Some(Dispatch::Static)
                    || flags.visibility() == Some(Visibility::Private))
            {
                continue;
            }
            if entry.sym.kind != SymbolKind::Function
                || entry.sym.arity != Some(0)
                || !entry.sym.params.is_empty()
                || entry.sym.has_vararg
                || !entry.sym.type_params.is_empty()
                || entry.sym.ext_receiver.is_some()
                || entry.sym.extension_receiver.is_some()
                || flags.jvm_restricted()
                || flags.dispatch() != Some(Dispatch::Instance)
                || flags.visibility() != Some(Visibility::Public)
                || flags.implementation().is_none()
            {
                // This slice implements the whole required surface, not a misleading subset.
                return unknown();
            }
            let Some(written_return) = &entry.sym.return_type else {
                return unknown();
            };
            let Some(ty) = crate::extensions::source::source_scalar_jvm_type(
                index,
                written_return,
                &ctx,
                entry.path.ends_with(".java"),
                &entry.sym.type_params,
            ) else {
                // Nullable/boxed, nominal/generic and projected returns stay unsupported.
                return unknown();
            };
            methods
                .entry(&entry.sym.name)
                .or_default()
                .push((entry, ty));
        }
    }
    let mut required = Vec::new();
    for (name, group) in methods {
        let abstract_ = group.iter().any(|(entry, _)| {
            entry.sym.source_member.implementation() == Some(MemberImplementation::Abstract)
        });
        let ty = &group[0].1;
        if group.iter().any(|(_, other)| other != ty)
            || object
                .methods
                .iter()
                .any(|method| method.name.as_ref() == name && method.descriptor.starts_with("()"))
        {
            // Includes final Object methods, inherited concrete methods and incompatible returns.
            return unknown();
        }
        let local: Vec<_> = group
            .iter()
            .filter(|(entry, _)| {
                entry.path == owner.path
                    && entry.sym.container.as_deref() == Some(owner.sym.name.as_str())
            })
            .collect();
        if !local.is_empty() {
            if local.len() != 1
                || local[0].0.sym.source_member.implementation()
                    != Some(MemberImplementation::Concrete)
            {
                return unknown();
            }
            continue;
        }
        let mut effective: Vec<_> = group.iter().map(|(entry, _)| *entry).collect();
        if remove_overridden_members(index, &receiver, &mut effective, &ctx, true) {
            return unknown();
        }
        let concrete = effective
            .iter()
            .filter(|entry| {
                entry.sym.source_member.implementation() == Some(MemberImplementation::Concrete)
            })
            .count();
        if concrete != 0 {
            if effective.len() != 1 {
                return unknown();
            }
            continue;
        }
        let Some(entry) = effective.first() else {
            return unknown();
        };
        if abstract_ {
            required.push((*entry, ty.clone()));
        }
    }
    ResolutionStatus::Found(required)
}

fn signature_inheritance_relation(
    index: &dyn InferenceIndex,
    receiver: &Type,
    child: &Entry,
    parent: &Entry,
    child_signature: &Option<Vec<Type>>,
    parent_signature: &Option<Vec<Type>>,
) -> resolve::ResolutionStatus<()> {
    use crate::knowledge::Knowledge;
    if child.sym.kind != crate::symbol::SymbolKind::Function
        || parent.sym.kind != crate::symbol::SymbolKind::Function
        || child.sym.name != parent.sym.name
        || child.sym.has_vararg != parent.sym.has_vararg
        || child
            .sym
            .arity
            .zip(parent.sym.arity)
            .is_some_and(|(child, parent)| child != parent)
        || child.sym.container == parent.sym.container && child.sym.package == parent.sym.package
        || matches!((child_signature, parent_signature), (Some(child), Some(parent)) if child != parent)
    {
        return Knowledge::DefinitelyAbsent;
    }
    let owner = owner_inheritance(index, child, parent);
    if owner.is_definitely_absent() {
        return owner;
    }
    if owner.is_found()
        && !owner_inheritance(index, parent, child).is_found()
        && (child_signature.is_some() && parent_signature.is_some()
            || generic_override_signature_equal(index, receiver, child, parent))
    {
        return Knowledge::Found(());
    }
    Knowledge::Unknown(vec![
        resolve::IncompletenessReason::MemberSurfaceIncomplete(crate::symbol::Def {
            file: child.path.to_string(),
            start_byte: child.sym.start_byte,
            end_byte: child.sym.end_byte,
        }),
    ])
}

// Generic overrides compare declaration binders, not invocation substitutions or erasures.
// Equal erasures alone would also admit incompatible bounds and nested type arguments.
fn generic_override_signature_equal(
    index: &dyn InferenceIndex,
    receiver: &Type,
    left: &Entry,
    right: &Entry,
) -> bool {
    if left.sym.type_params.is_empty()
        || left.sym.type_params.len() != right.sym.type_params.len()
        || !left.sym.type_parameter_facts.known_invariant()
        || !right.sym.type_parameter_facts.known_invariant()
        || left.sym.arity.map(usize::from) != Some(left.sym.params.len())
        || right.sym.arity.map(usize::from) != Some(right.sym.params.len())
        || left.sym.params.len() != right.sym.params.len()
    {
        return false;
    }
    let same = |a: &TypeRef, b: &TypeRef| {
        generic_signature_type_equal(index, receiver, left, a, right, b, 0)
    };
    let bounds = |entry: &Entry| {
        entry.sym.method_type_bounds.as_deref().cloned().or_else(|| {
            entry
                .sym
                .type_parameter_facts
                .known_unbounded()
                .then(Vec::new)
        })
    };
    let (Some(a), Some(b)) = (bounds(left), bounds(right)) else {
        return false;
    };
    a.len() == b.len()
        && a.iter().zip(&b).all(|((an, a), (bn, b))| {
            left.sym.type_params.iter().position(|name| name == an).is_some_and(|slot| {
                right.sym.type_params.iter().position(|name| name == bn) == Some(slot)
            })
                && same(a, b)
        })
        && left
            .sym
            .params
            .iter()
            .zip(&right.sym.params)
            .all(|(a, b)| same(a, b))
}

fn generic_signature_type_equal(
    index: &dyn InferenceIndex,
    receiver: &Type,
    left: &Entry,
    a: &TypeRef,
    right: &Entry,
    b: &TypeRef,
    depth: usize,
) -> bool {
    if depth > infer::MAX_DEPTH
        || a.nullable != b.nullable
        || a.projection != b.projection
        || a.owner.is_some()
        || b.owner.is_some()
    {
        return false;
    }
    let variable = |entry: &Entry, ty: &TypeRef| {
        ty.qualification
            .is_none()
            .then(|| {
                entry
                    .sym
                    .type_params
                    .iter()
                    .position(|name| name == &ty.name)
            })
            .flatten()
    };
    match (variable(left, a), variable(right, b)) {
        (Some(a_slot), Some(b_slot)) => {
            return a_slot == b_slot && a.args.is_empty() && b.args.is_empty();
        }
        (None, None) => {}
        _ => return false,
    }
    if a.projection == crate::types::Projection::Star {
        return true;
    }
    let head = |entry: &Entry, ty: &TypeRef| {
        let mut head = ty.clone();
        head.args.clear();
        head.projection = crate::types::Projection::Invariant;
        parameter_identity(
            index,
            infer::declared_member_type(
                index,
                receiver,
                entry,
                &head,
                CallMode::Java,
                &FileCtx::new(entry.sym.package.clone(), Vec::new()),
            ),
        )
    };
    let (Some(a_head), Some(b_head)) = (head(left, a), head(right, b)) else {
        return false;
    };
    a_head == b_head
        && a.args.len() == b.args.len()
        && a.args.iter().zip(&b.args).all(|(a, b)| {
            generic_signature_type_equal(index, receiver, left, a, right, b, depth + 1)
        })
}

fn parameter_identity(index: &dyn InferenceIndex, ty: Type) -> Option<Type> {
    if !infer::type_head_is_known(index, &ty) {
        return None;
    }
    let Type::Class {
        name,
        package,
        container,
        args,
        owner,
        ..
    } = ty
    else {
        return None;
    };
    let builtin = container.is_none()
        && package.as_deref().is_none_or(|package| {
            package == "kotlin"
                || package == "java.lang" && crate::defaults::is_jvm_reference_builtin(&name)
        })
        && crate::defaults::is_builtin_type(infer::primitive_type_name(&name));
    Some(Type::Class {
        name: if builtin {
            infer::primitive_type_name(&name).to_string()
        } else {
            name
        },
        package: if builtin {
            Some("kotlin".into())
        } else {
            package
        },
        container,
        nullable: false,
        args: args
            .into_iter()
            .map(|argument| parameter_identity(index, argument))
            .collect::<Option<_>>()?,

        owner: match owner {
            Some(owner) => Some(Box::new(parameter_identity(index, *owner)?)),
            None => None,
        },
    })
}

fn owner_inheritance(
    index: &dyn InferenceIndex,
    child: &Entry,
    parent: &Entry,
) -> resolve::ResolutionStatus<()> {
    use crate::knowledge::Knowledge;
    let (Some(child_owner), Some(parent_owner)) = (&child.sym.container, &parent.sym.container)
    else {
        return Knowledge::DefinitelyAbsent;
    };
    if child_owner == parent_owner && child.sym.package == parent.sym.package {
        return Knowledge::DefinitelyAbsent;
    }
    let (container, name) = child_owner
        .rsplit_once('.')
        .map_or((None, child_owner.as_str()), |(container, name)| {
            (Some(container.to_string()), name)
        });
    let owner = Type::Class {
        name: name.to_string(),
        package: Some(child.sym.package.clone()),
        container,
        nullable: false,
        args: Vec::new(),

        owner: None,
    };
    let mut incomplete = false;
    let found = infer::walk_supertypes(index, &owner, 32, |current| {
        incomplete |= current.name().is_none();
        if current.package() == Some(parent.sym.package.as_str())
            && infer::receiver_root_key(index, current).as_ref() == Some(parent_owner)
        {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    })
    .is_break();
    if found {
        Knowledge::Found(())
    } else if incomplete {
        Knowledge::Unknown(vec![resolve::IncompletenessReason::UnknownReceiverType])
    } else {
        Knowledge::DefinitelyAbsent
    }
}

impl<'a> Candidates<'a> {
    pub fn member_name_is_unavailable(&self, name: &str, kind: crate::symbol::SymbolKind) -> bool {
        self.unavailable_members
            .iter()
            .any(|(written, unavailable_kind, _)| *written == name && *unavailable_kind == kind)
    }

    pub fn has_unavailable_call(&self, count: Option<usize>) -> bool {
        self.unavailable_members.iter().any(|(_, kind, arity)| {
            *kind == crate::symbol::SymbolKind::Function
                && (arity.is_none() || count.is_none() || arity.map(usize::from) == count)
        })
    }

    pub fn known_extensions(&self) -> impl Iterator<Item = &'a Entry> + '_ {
        self.extensions
            .iter()
            .copied()
            .filter(|entry| self.receiver_is_known(entry))
    }

    pub fn has_uncertain_receivers(&self) -> bool {
        !self.uncertain_receivers.is_empty() || !self.unavailable_members.is_empty()
    }

    pub fn has_local_extensions(&self) -> bool {
        !self.local_scopes.is_empty()
    }

    pub fn known_local_extensions(&self) -> impl Iterator<Item = &'a Entry> + '_ {
        self.local_scopes
            .iter()
            .flat_map(|scope| scope.iter().copied())
            .filter(|entry| self.receiver_is_known(entry))
    }

    fn receiver_is_known(&self, entry: &Entry) -> bool {
        !self.uncertain_receivers.contains(&(
            entry.path.as_ref(),
            entry.sym.start_byte,
            entry.sym.end_byte,
        ))
    }

    fn selection(&self, entries: Vec<&'a Entry>, applicability: Applicability) -> Selection<'a> {
        if entries.iter().any(|entry| !self.receiver_is_known(entry)) {
            return Selection {
                entries: Vec::new(),
                applicability: Applicability::Unknown,
            };
        }
        Selection {
            entries,
            applicability,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
            && self.property_getters.is_empty()
            && self.extensions.is_empty()
            && self.local_scopes.is_empty()
    }

    fn groups(&self) -> impl Iterator<Item = &[&'a Entry]> {
        std::iter::once(self.members.as_slice())
            .chain(self.local_scopes.iter().map(Vec::as_slice))
            .chain(std::iter::once(self.extensions.as_slice()))
    }

    /// Applicable functions have priority over a value's `invoke`. A known arity/type mismatch
    /// cannot hide a callable property; unknown syntax/types retain possible candidates.
    pub fn select(
        &self,
        index: &dyn InferenceIndex,
        receiver: &Type,
        usage: &MemberUse,
        ctx: &FileCtx,
    ) -> Selection<'a> {
        if matches!(usage, MemberUse::Call(args) if self.has_unavailable_call(args.count)) {
            return Selection {
                entries: Vec::new(),
                applicability: Applicability::Unknown,
            };
        }
        match usage {
            MemberUse::Value => {
                for (entries, getter) in std::iter::once((self.members.as_slice(), false))
                    .chain(std::iter::once((self.property_getters.as_slice(), true)))
                    .chain(
                        self.local_scopes
                            .iter()
                            .map(|entries| (entries.as_slice(), false)),
                    )
                    .chain(std::iter::once((self.extensions.as_slice(), false)))
                {
                    let values: Vec<_> = entries
                        .iter()
                        .copied()
                        .filter(|entry| {
                            entry.sym.kind.is_value_like()
                                || kotlin_enum_property_type_ref(index, receiver, entry).is_some()
                                || getter && kotlin_java_getter_property_name(entry).is_some()
                        })
                        .collect();
                    if !values.is_empty() {
                        return self.selection(values, Applicability::Applicable);
                    }
                }
            }
            MemberUse::Call(args) => {
                let mut possible = Vec::new();
                let mut rejected = Vec::new();
                for functions in [true, false] {
                    for entries in self.groups() {
                        let mut viable = Vec::new();
                        let mut uncertain = false;
                        for entry in entries.iter().copied() {
                            let mut applicability =
                                if functions && entry.sym.kind.is_callable_like() {
                                    function_applicability(
                                        index,
                                        receiver,
                                        entry,
                                        args,
                                        ctx,
                                        CallMode::Kotlin,
                                    )
                                } else if !functions && entry.sym.kind.is_value_like() {
                                    callable_value_applicability(index, receiver, entry, args, ctx)
                                } else {
                                    continue;
                                };
                            if applicability == Applicability::Applicable
                                && !self.receiver_is_known(entry)
                            {
                                applicability = Applicability::Unknown;
                            }
                            if applicability == Applicability::Rejected {
                                // A scalar field is not a rejected callable declaration.
                                if functions
                                    || entry
                                        .sym
                                        .value_type
                                        .as_ref()
                                        .is_some_and(|ty| function_type_signature(ty).is_some())
                                {
                                    rejected.push(entry);
                                }
                            } else {
                                uncertain |= applicability == Applicability::Unknown;
                                viable.push(entry);
                            }
                        }
                        if !viable.is_empty() {
                            if possible.is_empty()
                                && functions
                                && (viable.len() > 1 || uncertain)
                                && viable.iter().all(|entry| self.receiver_is_known(entry))
                            {
                                if let Some(selected) = most_specific_reference_call(
                                    index,
                                    receiver,
                                    &viable,
                                    args,
                                    CallMode::Kotlin,
                                ) {
                                    return self.selection(
                                        vec![viable[selected]],
                                        Applicability::Applicable,
                                    );
                                }
                            }
                            if possible.is_empty() && !uncertain {
                                return self.selection(viable, Applicability::Applicable);
                            }
                            possible.extend(viable);
                        }
                    }
                }
                if !possible.is_empty() {
                    return self.selection(possible, Applicability::Unknown);
                }
                // An invalid call still names a unique declaration. Navigation can show it while
                // inference uses the rejected status rather than inventing a successful result.
                if rejected.len() == 1 {
                    return self.selection(rejected, Applicability::Rejected);
                }
            }
        }
        Selection {
            entries: Vec::new(),
            applicability: if self.unavailable_members.is_empty() {
                Applicability::Rejected
            } else {
                Applicability::Unknown
            },
        }
    }
}

/// Free-function callers use the same applicability and uncertainty policy as member calls.
/// The caller supplies its already-visible declaration group; this does not change scope lookup.
pub(crate) fn select_functions<'a>(
    index: &dyn InferenceIndex,
    entries: Vec<&'a Entry>,
    usage: &MemberUse,
    ctx: &FileCtx,
) -> Selection<'a> {
    Candidates {
        members: entries,
        property_getters: Vec::new(),
        extensions: Vec::new(),
        local_scopes: Vec::new(),
        uncertain_receivers: BTreeSet::new(),
        hierarchy_complete: true,
        unavailable_members: Vec::new(),
    }
    .select(index, &Type::Unknown, usage, ctx)
}

/// Supplied actuals and their corresponding formal types always have the same order.
/// Ordinary positional calls retain their existing borrowed facts and formal slice.
pub(crate) struct BoundArguments<'a> {
    pub mode: CallMode,
    pub arguments: Cow<'a, CallArguments>,
    pub params: Cow<'a, [TypeRef]>,
}

/// Rebind one candidate, never a name-wide overload group. Unsupported shapes remain possible.
pub(crate) fn bind_arguments<'a>(
    index: &dyn InferenceIndex,
    receiver: &Type,
    entry: &'a Entry,
    args: &'a CallArguments,
    mode: CallMode,
    ctx: &FileCtx,
) -> Result<BoundArguments<'a>, Applicability> {
    bind_arguments_with_lambda_context(index, receiver, entry, args, mode, ctx, false)
}

pub(crate) fn bind_arguments_for_lambda_context<'a>(
    index: &dyn InferenceIndex,
    receiver: &Type,
    entry: &'a Entry,
    args: &'a CallArguments,
    ctx: &FileCtx,
) -> Result<BoundArguments<'a>, Applicability> {
    bind_arguments_with_lambda_context(index, receiver, entry, args, CallMode::Kotlin, ctx, true)
}

fn bind_arguments_with_lambda_context<'a>(
    index: &dyn InferenceIndex,
    receiver: &Type,
    entry: &'a Entry,
    args: &'a CallArguments,
    mode: CallMode,
    ctx: &FileCtx,
    only_context: bool,
) -> Result<BoundArguments<'a>, Applicability> {
    let Some(names) = &args.names else {
        return Ok(BoundArguments {
            mode,
            arguments: Cow::Borrowed(args),
            params: Cow::Borrowed(&entry.sym.params),
        });
    };
    if mode != CallMode::Kotlin
        || args.count != Some(args.types.len() + usize::from(args.trailing_lambda))
        || names.len() != args.types.len()
        || args.has_spread
        || entry.sym.has_vararg
    {
        return Err(Applicability::Unknown);
    }
    let contextual_lambda = args.trailing_lambda;
    let Some(formals) = entry.sym.kotlin_parameters.as_deref() else {
        return Err(Applicability::Unknown);
    };
    if formals.len() != entry.sym.params.len()
        || entry.sym.arity.map(usize::from) != Some(formals.len())
        || formals.iter().any(|formal| formal.name.is_empty())
        || formals.iter().enumerate().any(|(position, formal)| {
            formals[..position]
                .iter()
                .any(|previous| previous.name == formal.name)
        })
    {
        return Err(Applicability::Unknown);
    }
    let supplied = args.types.len() + usize::from(contextual_lambda);
    if supplied > formals.len() {
        return Err(Applicability::Rejected);
    }
    let mut positions = vec![None; formals.len()];
    let mut saw_named = false;
    for (written, name) in names.iter().enumerate() {
        let position = if let Some(name) = name {
            saw_named = true;
            let Some(position) = formals.iter().position(|formal| &formal.name == name) else {
                return Err(Applicability::Rejected);
            };
            position
        } else {
            if saw_named {
                return Err(Applicability::Unknown);
            }
            written
        };
        if positions[position].replace(written).is_some() {
            return Err(Applicability::Rejected);
        }
    }
    if contextual_lambda && positions.last().is_some_and(Option::is_some) {
        return Err(Applicability::Rejected);
    }
    if positions
        .iter()
        .zip(formals.iter())
        .enumerate()
        .any(|(slot, (position, formal))| {
            position.is_none() && !formal.has_default
                && !(contextual_lambda && slot + 1 == formals.len())
        })
    {
        // shortcut: missing defaults stay unknown until inherited/actual defaults are modeled.
        return Err(Applicability::Unknown);
    }
    let params = if args.types.len() == formals.len() {
        Cow::Borrowed(entry.sym.params.as_slice())
    } else {
        Cow::Owned(
            entry
                .sym
                .params
                .iter()
                .zip(&positions)
                .filter(|(_, position)| position.is_some())
                .map(|(formal, _)| formal.clone())
                .collect(),
        )
    };
    let types = positions
        .into_iter()
        .flatten()
        .map(|position| args.types[position].clone())
        .collect();
    let bound = BoundArguments {
        mode,
        arguments: Cow::Owned(CallArguments {
            count: Some(args.types.len()),
            types,
            names: None,
            has_spread: false,
            trailing_lambda: false,
            trailing_lambda_arguments: None,
        }),
        params,
    };
    if contextual_lambda {
        let Some(parameter) = entry.sym.params.last() else {
            return Err(Applicability::Unknown);
        };
        if infer::trailing_lambda_receiver_ref(index, entry, ctx).is_some()
            || function_type_signature(parameter).is_none()
        {
            return Err(Applicability::Unknown);
        }
        // Ordinary arguments establish method variables before the callback is checked.
        let ty = infer::declared_member_bound_type(index, receiver, entry, parameter, &bound, ctx);
        let Type::Class { name, package: Some(package), container: None, args: signature, owner: None, nullable: false } = &ty else {
            return Err(Applicability::Unknown);
        };
        let Some((result, parameters)) = signature.split_last() else {
            return Err(Applicability::Unknown);
        };
        if name != &format!("Function{}", parameters.len())
            || !matches!(package.as_str(), "kotlin" | "kotlin.jvm.functions")
            || parameters.len() > 1
            || signature.iter().any(|ty| !infer::constraint_type_is_known(index, ty, 0))
        {
            return Err(Applicability::Unknown);
        }
        let Some(lambda) = &args.trailing_lambda_arguments else {
            return Err(Applicability::Unknown);
        };
        match &lambda.parameters {
            LambdaParameters::Implicit if parameters.len() <= 1 => {},
            LambdaParameters::Explicit(actual) if actual.len() == parameters.len()
                && actual.iter().zip(parameters).all(|(actual, formal)| actual.as_ref().is_none_or(|actual| {
                    infer::constraint_type_is_known(index, actual, 0)
                        && infer::receiver_satisfies_upper_bound(index, formal, actual)
                })) => {},
            _ => return Err(Applicability::Unknown),
        }
        // Context does not depend on the body. Only proven Unit coercion may discard its result.
        if !only_context && result != &Type::class("Unit", Some("kotlin".into()))
            && (!infer::constraint_type_is_known(index, &lambda.result, 0)
                || !infer::receiver_satisfies_upper_bound(index, &lambda.result, result))
        {
            return Err(Applicability::Unknown);
        }
    }
    Ok(bound)
}

pub fn function_accepts(
    index: &dyn InferenceIndex,
    receiver: &Type,
    entry: &Entry,
    args: &CallArguments,
    ctx: &FileCtx,
) -> bool {
    function_applicability(index, receiver, entry, args, ctx, CallMode::Kotlin)
        != Applicability::Rejected
}

// shortcut: exact positional reference overloads only; add conversion/variance phases with compiler controls.
pub(crate) fn most_specific_reference_call(
    index: &dyn InferenceIndex,
    receiver: &Type,
    entries: &[&Entry],
    args: &CallArguments,
    mode: CallMode,
) -> Option<usize> {
    if entries.is_empty()
        || args.count != Some(args.types.len())
        || args.names.is_some()
        || args.has_spread
        || args.trailing_lambda
        || !infer::type_head_is_known(index, receiver)
        || !args
            .types
            .iter()
            .all(|ty| reference_shape_known(index, ty, false, 0))
    {
        return None;
    }
    if mode == CallMode::Java {
        if let Some(winner) = root_generic_specificity(index, receiver, entries, args) {
            return Some(winner);
        }
    }
    let formals = entries
        .iter()
        .map(|entry| {
            if entry.sym.kind != crate::symbol::SymbolKind::Function
                || entry.sym.has_vararg
                || entry.sym.ext_receiver.is_some()
                || entry.sym.extension_receiver.is_some()
                || entry.sym.arity.map(usize::from) != args.count
                || entry.sym.min_arity.map(usize::from) != args.count
                || entry.sym.params.len() != args.types.len()
            {
                return None;
            }
            let ctx = FileCtx::new(entry.sym.package.clone(), Vec::new());
            let generic = !entry.sym.type_params.is_empty();
            if generic {
                let bound = bind_arguments(index, receiver, entry, args, mode, &ctx).ok()?;
                if infer::projected_member_application(index, receiver, entry, &bound)?.status
                    != crate::solve::ConstraintStatus::Matched
                {
                    return None;
                }
            }
            entry
                .sym
                .params
                .iter()
                .zip(&args.types)
                .map(|(formal, actual)| {
                    let formal = if generic {
                        infer::declared_member_call_type(index, receiver, entry, formal, args, mode, &ctx)
                    } else {
                        infer::declared_member_type(index, receiver, entry, formal, mode, &ctx)
                    };
                    reference_subtype(index, actual, &formal, false).then_some(formal)
                })
                .collect::<Option<Vec<_>>>()
        })
        .collect::<Option<Vec<_>>>()?;
    let declarations: Vec<_> = entries
        .iter()
        .zip(&formals)
        .map(|(entry, formals)| {
            if entry.sym.type_params.is_empty() {
                formals.iter().cloned().map(Some).collect()
            } else {
                generic_declaration_formals(index, receiver, entry, mode)
            }
        })
        .collect();
    let more_specific = |left: usize, right: usize| {
        // A generic competitor needs declaration inference, not an invocation's inferred arguments.
        if !entries[right].sym.type_params.is_empty() {
            return false;
        }
        let generic = !entries[left].sym.type_params.is_empty();
        let left_formals = &declarations[left];
        left_formals.iter().zip(&formals[right]).all(|(a, b)| {
            a.as_ref()
                .is_some_and(|a| reference_subtype(index, a, b, generic))
        }) && left_formals.iter().zip(&formals[right]).any(|(a, b)| {
            a.as_ref()
                .is_some_and(|a| !reference_subtype(index, b, a, generic))
        })
    };
    let mut winner = None;
    for candidate in 0..entries.len() {
        if (0..entries.len()).all(|other| candidate == other || more_specific(candidate, other)) {
            if winner.replace(candidate).is_some() {
                return None;
            }
        }
    }
    if winner.is_some() || mode != CallMode::Kotlin || entries.len() != 2 {
        return winner;
    }
    let generic = !entries[0].sym.type_params.is_empty();
    if generic == !entries[1].sym.type_params.is_empty() {
        return None;
    }
    let forward = |left: usize, right: usize| {
        let mut proof = DeclarationForward::Proven;
        for (position, (a, b)) in declarations[left]
            .iter()
            .zip(&declarations[right])
            .enumerate()
        {
            let (Some(a), Some(b)) = (a, b) else {
                proof = DeclarationForward::Unknown;
                continue;
            };
            let rigid = entries[left].sym.type_params.is_empty()
                || entries[left].sym.type_parameter_facts.known_unbounded();
            match reference_forward(index, a, b, true, rigid) {
                DeclarationForward::Disproven => return DeclarationForward::Disproven,
                DeclarationForward::Proven
                    if !contains_method_parameter(
                        &entries[right].sym.params[position],
                        &entries[right].sym.type_params,
                    ) => {}
                _ => proof = DeclarationForward::Unknown,
            }
        }
        proof
    };
    // Kotlin's nongeneric tie-break follows declaration forwarding, never call-site substitutions.
    match (forward(0, 1), forward(1, 0)) {
        (DeclarationForward::Proven, DeclarationForward::Proven)
        | (DeclarationForward::Disproven, DeclarationForward::Disproven) => {
            Some(usize::from(generic))
        }
        _ => None,
    }
}

/// A declaration-level slice of generic specificity: a nominal reference parameter can
/// forward to an otherwise unconstrained method variable occurring once. Invocation-inferred
/// bindings cannot prove this relation (they make both overloads look equally specific).
fn root_generic_specificity(
    index: &dyn InferenceIndex,
    receiver: &Type,
    entries: &[&Entry],
    args: &CallArguments,
) -> Option<usize> {
    let unbounded = |entry: &Entry| {
        entry.sym.type_params.len() == 1
            && entry.sym.type_parameter_facts.known_invariant()
            && (entry.sym.type_parameter_facts.known_unbounded()
                // The declaration producer normalizes explicit Java Object bounds away.
                || entry.sym.method_type_bounds.as_deref().is_some_and(|bounds| bounds.is_empty()))
    };
    if entries.len() < 2
        || !entries.iter().all(|entry| {
            unbounded(entry)
                && !entry.sym.has_vararg
                && entry.sym.extension_receiver.is_none()
                && entry.sym.ext_receiver.is_none()
                && entry.sym.arity.map(usize::from) == args.count
                && entry.sym.min_arity.map(usize::from) == args.count
                && entry.sym.params.len() == args.types.len()
                && bind_arguments(index, receiver, entry, args, CallMode::Java, &FileCtx::new(entry.sym.package.clone(), Vec::new()))
                    .ok()
                    .and_then(|bound| {
                        infer::projected_member_application(index, receiver, entry, &bound)
                    })
                    .is_some_and(|application| {
                        application.status == crate::solve::ConstraintStatus::Matched
                    })
        })
    {
        return None;
    }
    let forwards = |left: &Entry, right: &Entry| {
        let slots: Vec<_> = right
            .sym
            .params
            .iter()
            .enumerate()
            .filter(|(_, ty)| contains_method_parameter(ty, &right.sym.type_params))
            .collect();
        let [(slot, variable)] = slots.as_slice() else {
            return false;
        };
        if variable.qualification.is_some()
            || variable.name != right.sym.type_params[0]
            || variable.projection != crate::types::Projection::Invariant
            || variable.nullable
            || variable.owner.is_some()
            || !variable.args.is_empty()
        {
            return false;
        }
        left.sym
            .params
            .iter()
            .zip(&right.sym.params)
            .enumerate()
            .all(|(position, (a, b))| {
                if position != *slot {
                    return generic_signature_type_equal(index, receiver, left, a, right, b, 0);
                }
                if a.qualification.is_none() && left.sym.type_params.contains(&a.name)
                    || a.projection != crate::types::Projection::Invariant
                    || a.nullable
                    || a.owner.is_some()
                {
                    return false;
                }
                let ty = infer::declared_member_type(
                    index,
                    receiver,
                    left,
                    a,
                    CallMode::Java,
                    &FileCtx::new(left.sym.package.clone(), Vec::new()),
                );
                matches!(ty, Type::Class { .. }) && infer::type_head_is_known(index, &ty)
            })
    };
    let mut winners = (0..entries.len()).filter(|&left| {
        (0..entries.len()).all(|right| left == right || forwards(entries[left], entries[right]))
    });
    let winner = winners.next()?;
    winners.next().is_none().then_some(winner)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeclarationForward {
    Proven,
    Disproven,
    Unknown,
}

fn contains_method_parameter(ty: &TypeRef, parameters: &[String]) -> bool {
    ty.qualification.is_none() && parameters.contains(&ty.name)
        || ty
            .owner
            .as_deref()
            .is_some_and(|owner| contains_method_parameter(owner, parameters))
        || ty
            .args
            .iter()
            .any(|argument| contains_method_parameter(argument, parameters))
}

fn generic_declaration_formals(
    index: &dyn InferenceIndex,
    receiver: &Type,
    entry: &Entry,
    mode: CallMode,
) -> Vec<Option<Type>> {
    let ctx = FileCtx::new(entry.sym.package.clone(), Vec::new());
    entry
        .sym
        .params
        .iter()
        .map(|parameter| {
            if parameter.owner.is_some()
                || parameter.qualification.is_none()
                    && entry.sym.type_params.contains(&parameter.name)
            {
                return None;
            }
            let mut declared = parameter.clone();
            for argument in &mut declared.args {
                if argument.qualification.is_none()
                    && entry.sym.type_params.contains(&argument.name)
                    && argument.projection == crate::types::Projection::Invariant
                    && argument.args.is_empty()
                    && argument.owner.is_none()
                    && !argument.nullable
                {
                    // Universal binder slots can fit an explicit wildcard, never a concrete argument.
                    argument.projection = crate::types::Projection::Star;
                } else if contains_method_parameter(argument, &entry.sym.type_params) {
                    return None;
                }
            }
            let declared = infer::declared_member_type(index, receiver, entry, &declared, mode, &ctx);
            reference_shape_known(index, &declared, true, 0).then_some(declared)
        })
        .collect()
}

fn reference_shape_known(index: &dyn InferenceIndex, ty: &Type, star: bool, depth: usize) -> bool {
    if depth > infer::MAX_DEPTH {
        return false;
    }
    if star && matches!(ty, Type::Star) {
        return true;
    }
    let Type::Class {
        name,
        package: Some(package),
        container,
        args,
        owner: None,
        ..
    } = ty
    else {
        return false;
    };
    let declarations = index.lookup_type(name);
    let mut matching = declarations
        .iter()
        .filter(|entry| entry.sym.package == *package && entry.sym.container == *container);
    let Some(first) = matching.next() else {
        return false;
    };
    let valid = |entry: &Entry| {
        matches!(
            entry.sym.kind,
            crate::symbol::SymbolKind::Class
                | crate::symbol::SymbolKind::Interface
                | crate::symbol::SymbolKind::EnumClass
        ) && entry.sym.type_params.len() == args.len()
            && entry.sym.type_parameter_facts.known_invariant()
            && entry.sym.type_parameter_facts.known_unbounded()
            && entry.sym.kind == first.sym.kind
            && entry.sym.type_params == first.sym.type_params
    };
    valid(first)
        && matching.all(|entry| valid(entry))
        && args
            .iter()
            .all(|argument| reference_shape_known(index, argument, star, depth + 1))
}

fn reference_subtype(
    index: &dyn InferenceIndex,
    actual: &Type,
    formal: &Type,
    universal: bool,
) -> bool {
    reference_forward(index, actual, formal, universal, false) == DeclarationForward::Proven
}

fn reference_identity_equal(index: &dyn InferenceIndex, actual: &Type, formal: &Type) -> bool {
    actual == formal
        || actual.is_nullable() == formal.is_nullable()
            && actual.args().len() == formal.args().len()
            && actual
                .args()
                .iter()
                .zip(formal.args())
                .all(|(a, b)| reference_identity_equal(index, a, b))
            && parameter_identity(index, actual.clone())
                .zip(parameter_identity(index, formal.clone()))
                .is_some_and(|(a, b)| a == b)
}

fn reference_forward(
    index: &dyn InferenceIndex,
    actual: &Type,
    formal: &Type,
    universal: bool,
    rigid_arguments: bool,
) -> DeclarationForward {
    if !reference_shape_known(index, actual, universal, 0)
        || !reference_shape_known(index, formal, true, 0)
    {
        return DeclarationForward::Unknown;
    }
    if actual.is_nullable() && !formal.is_nullable() {
        return DeclarationForward::Disproven;
    }
    // Identity and the language root do not depend on unrelated inherited interfaces.
    if reference_identity_equal(index, actual, formal) || infer::is_reference_root(formal) {
        return DeclarationForward::Proven;
    }
    let mut witnessed = false;
    let mut conflicting = false;
    let mut incomplete = false;
    let mut unsupported = false;
    let mut matching = None;
    let _ = infer::walk_supertypes(index, actual, infer::MAX_DEPTH, |current| {
        incomplete |= matches!(current, Type::Unknown);
        unsupported |= !reference_shape_known(index, current, universal, 0);
        if current.name() == formal.name()
            && current.package() == formal.package()
            && current.container() == formal.container()
        {
            if matching
                .as_ref()
                .is_some_and(|previous| previous != current)
            {
                incomplete = true;
            }
            matching = Some(current.clone());
            let compatible =
                reference_shape_known(index, current, universal, 0)
                    && current.owner() == formal.owner()
                    && current.args().len() == formal.args().len()
                    && current.args().iter().zip(formal.args()).all(|(a, b)| {
                        matches!(b, Type::Star) || reference_identity_equal(index, a, b)
                    });
            witnessed |= compatible;
            conflicting |= !compatible;
        }
        ControlFlow::Continue(())
    });
    if incomplete || witnessed && conflicting {
        DeclarationForward::Unknown
    } else if witnessed {
        DeclarationForward::Proven
    } else if unsupported || conflicting && !rigid_arguments {
        DeclarationForward::Unknown
    } else {
        DeclarationForward::Disproven
    }
}

#[cfg(test)]
#[test]
fn reference_forward_preserves_mapped_argument_nullability_and_foreign_identity() {
    let text = "package proof\nclass List<T>\nclass String\n";
    let tree = crate::parser::KotlinParser::new().parse(text);
    let mut index = crate::index::Index::new();
    index.replace_file(
        "Proof.kt",
        crate::indexer::extract_symbols(&tree, text, "proof"),
        crate::index::Tier::Volatile,
    );
    for (file, package) in [("Kotlin.kt", "kotlin"), ("Java.kt", "java.lang")] {
        let source = format!("package {package}\nclass String\n");
        let tree = crate::parser::KotlinParser::new().parse(&source);
        index.replace_file(
            file,
            crate::indexer::extract_symbols(&tree, &source, package),
            crate::index::Tier::Volatile,
        );
    }
    let list = |argument| {
        let mut list = Type::class("List", Some("proof".into()));
        let Type::Class { args, .. } = &mut list else {
            unreachable!()
        };
        args.push(argument);
        list
    };
    let kotlin = Type::class("String", Some("kotlin".into()));
    let java = Type::class("String", Some("java.lang".into()));
    assert!(reference_subtype(
        &index,
        &list(kotlin.clone()),
        &list(java.clone()),
        false
    ));
    assert!(!reference_subtype(
        &index,
        &list(kotlin.with_nullability(true)),
        &list(java.clone()),
        false
    ));
    assert!(!reference_subtype(
        &index,
        &Type::class("String", Some("proof".into())),
        &java,
        false
    ));
    assert!(!reference_subtype(
        &index,
        &Type::Unknown,
        &Type::Unknown,
        false
    ));
}

pub(crate) fn function_applicability(
    index: &dyn InferenceIndex,
    receiver: &Type,
    entry: &Entry,
    args: &CallArguments,
    ctx: &FileCtx,
    mode: CallMode,
) -> Applicability {
    let bound = match bind_arguments(index, receiver, entry, args, mode, ctx) {
        Ok(bound) => bound,
        Err(status) => return status,
    };
    let named = args.names.is_some();
    let args = bound.arguments.as_ref();
    let mut uncertain = args.count.is_none()
        || args.trailing_lambda
        || extension_receiver_applicability(index, receiver, entry) == Applicability::Unknown;
    // Named binding has checked each missing slot against its authored default fact.
    // Positional min_arity describes a required prefix, not the number of named arguments.
    if let Some(count) = args.count.filter(|_| !named) {
        let min = if args.trailing_lambda {
            infer::trailing_lambda_min_arity(index, entry, ctx).or(entry.sym.min_arity)
        } else {
            entry.sym.min_arity
        };
        if min.is_some_and(|min| count < min as usize)
            || (!entry.sym.has_vararg && entry.sym.arity.is_some_and(|max| count > max as usize))
        {
            return Applicability::Rejected;
        }
        uncertain |= min.is_none() || entry.sym.arity.is_none();
    }
    parameters_applicability(index, receiver, entry, &bound, ctx, mode, uncertain)
}

fn parameters_applicability(
    index: &dyn InferenceIndex,
    receiver: &Type,
    entry: &Entry,
    bound: &BoundArguments<'_>,
    ctx: &FileCtx,
    mode: CallMode,
    mut uncertain: bool,
) -> Applicability {
    let platform =
        infer::java_platform_null_arguments(entry, &bound.params, &bound.arguments.types).map(
            |types| CallArguments {
                types,
                ..bound.arguments.as_ref().clone()
            },
        );
    let platform_bound = platform.as_ref().map(|arguments| BoundArguments {
        mode: bound.mode,
        arguments: Cow::Borrowed(arguments),
        params: Cow::Borrowed(&bound.params),
    });
    let bound = platform_bound.as_ref().unwrap_or(bound);
    let args = bound.arguments.as_ref();
    let params = bound.params.as_ref();
    uncertain |= params.len() != entry.sym.params.len()
        && !infer::call_parameter_templates_known(index, entry);
    if let Some(application) = infer::projected_member_application(index, receiver, entry, bound) {
        if application.status == crate::solve::ConstraintStatus::Unknown
            && !entry.sym.has_vararg
            && !args.has_spread
            && !args.trailing_lambda
            && args.count == Some(args.types.len())
            && params.len() == args.types.len()
            && params.len() == entry.sym.params.len()
            && params.iter().zip(&args.types).any(|(parameter, actual)| {
                // Preserve the existing unbounded nominal-head proof for Class<T> even
                // when T is unresolved. A variable itself has no independent nominal head;
                // bounded projected templates still need their application proof.
                if contains_method_parameter(parameter, &entry.sym.type_params)
                    && (!entry.sym.type_parameter_facts.known_unbounded()
                        || parameter.qualification.is_none()
                            && entry.sym.type_params.contains(&parameter.name))
                {
                    return false;
                }
                let formal = infer::declared_member_type(index, receiver, entry, parameter, mode, ctx);
                infer::nominal_heads_incompatible(index, actual, &formal)
            })
        {
            return Applicability::Rejected;
        }
        return match application.status {
            crate::solve::ConstraintStatus::Rejected => Applicability::Rejected,
            crate::solve::ConstraintStatus::Unknown => Applicability::Unknown,
            crate::solve::ConstraintStatus::Matched if !uncertain => Applicability::Applicable,
            _ => Applicability::Unknown,
        };
    }
    arguments_applicability(
        index,
        args,
        |position| {
            let formal = params
                .get(position)
                .or_else(|| entry.sym.has_vararg.then(|| params.last()).flatten());
            let Some(formal) = formal else {
                return Type::Unknown;
            };
            if entry.sym.has_vararg
                && position >= params.len().saturating_sub(1)
                && formal.name.ends_with("[]")
            {
                // A legacy array reference has not retained a normalized component type.
                return Type::Unknown;
            }
            infer::declared_member_type(index, receiver, entry, formal, mode, ctx)
        },
        mode,
        uncertain,
    )
}

fn argument_shape_is_known(index: &dyn InferenceIndex, ty: &Type) -> bool {
    infer::type_head_is_known(index, ty)
        && ty.projection() == crate::types::Projection::Invariant
        && ty
            .args()
            .iter()
            .all(|arg| argument_shape_is_known(index, arg))
}

pub(crate) fn arguments_applicability(
    index: &dyn InferenceIndex,
    args: &CallArguments,
    mut formal_type: impl FnMut(usize) -> Type,
    mode: CallMode,
    mut uncertain: bool,
) -> Applicability {
    if args.names.is_some() {
        return Applicability::Unknown;
    }
    uncertain |= args.count.is_some_and(|count| args.types.len() < count);
    for (position, actual) in args.types.iter().enumerate() {
        let formal = formal_type(position);
        if !infer::types_consistent(index, actual, &formal) {
            if mode == CallMode::Java && java_conversion_unmodeled(actual, &formal) {
                // shortcut: preserve Java widening/boxing until phased overload specificity is modeled.
                uncertain = true;
                continue;
            }
            return Applicability::Rejected;
        }
        uncertain |= !infer::type_head_is_known(index, actual)
            || !infer::type_head_is_known(index, &formal)
            || !argument_shape_is_known(index, actual)
            || !argument_shape_is_known(index, &formal);
    }
    if uncertain {
        Applicability::Unknown
    } else {
        Applicability::Applicable
    }
}

fn java_conversion_unmodeled(actual: &Type, formal: &Type) -> bool {
    let builtin = |ty: &Type| {
        ty.container().is_none()
            && ty
                .package()
                .is_none_or(|package| matches!(package, "kotlin" | "java.lang"))
    };
    let numeric = |ty: &Type| {
        builtin(ty)
            && matches!(
                ty.name(),
                Some(
                    "byte"
                        | "Byte"
                        | "short"
                        | "Short"
                        | "char"
                        | "Char"
                        | "Character"
                        | "int"
                        | "Int"
                        | "Integer"
                        | "long"
                        | "Long"
                        | "float"
                        | "Float"
                        | "double"
                        | "Double"
                )
            )
    };
    let scalar = |ty: &Type| {
        numeric(ty) || (builtin(ty) && matches!(ty.name(), Some("boolean" | "Boolean")))
    };
    (formal.name() == Some("Object")
        && formal.package() == Some("java.lang")
        && formal.container().is_none())
        || (numeric(actual) && numeric(formal))
        || (scalar(actual)
            && !scalar(formal)
            && formal.container().is_none()
            && formal
                .package()
                .is_some_and(|package| package.starts_with("java."))
            && formal.name() != Some("String"))
}

/// Kotlin function types are normalized into the existing `FunctionN<P..., R>` type representation.
/// Custom callable classes and unmodeled signatures remain possible, never definitely absent.
pub fn function_type_signature(ty: &TypeRef) -> Option<(&[TypeRef], &TypeRef)> {
    let arity = ty.name.strip_prefix("Function")?.parse::<usize>().ok()?;
    let package = ty
        .qualification
        .as_deref()
        .or_else(|| (ty.package_candidates.len() == 1).then(|| ty.package_candidates[0].as_str()));
    if ty.name != format!("Function{arity}")
        || !matches!(package, Some("kotlin" | "kotlin.jvm.functions"))
        || arity.checked_add(1)? != ty.args.len()
        || ty.owner.is_some()
    {
        return None;
    }
    Some((&ty.args[..arity], &ty.args[arity]))
}

fn callable_value_applicability(
    index: &dyn InferenceIndex,
    receiver: &Type,
    entry: &Entry,
    args: &CallArguments,
    ctx: &FileCtx,
) -> Applicability {
    if args.names.is_some() {
        return Applicability::Unknown;
    }
    let Some(ty) = &entry.sym.value_type else {
        return Applicability::Unknown;
    };
    if let Some((params, _)) = function_type_signature(ty) {
        if args.count.is_some_and(|count| count != params.len()) {
            return Applicability::Rejected;
        }
        return parameters_applicability(
            index,
            receiver,
            entry,
            &BoundArguments {
                mode: CallMode::Kotlin,
                arguments: Cow::Borrowed(args),
                params: Cow::Borrowed(params),
            },
            ctx,
            CallMode::Kotlin,
            args.count.is_none() || args.trailing_lambda || ty.nullable,
        );
    }
    // These final value types have no invoke operator. A custom class may have an operator that
    // has not been indexed; its absence is not a proof that the value cannot be called.
    if matches!(
        ty.name.as_str(),
        "Int"
            | "Long"
            | "Short"
            | "Byte"
            | "Float"
            | "Double"
            | "Boolean"
            | "Char"
            | "String"
            | "Unit"
            | "int"
            | "long"
            | "short"
            | "byte"
            | "float"
            | "double"
            | "boolean"
            | "char"
            | "void"
    ) {
        Applicability::Rejected
    } else {
        Applicability::Unknown
    }
}
