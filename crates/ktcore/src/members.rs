//! Receiver member candidates and use-site selection, shared by resolution and inference.

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
    pub types: Vec<Type>,
    pub trailing_lambda: bool,
}

#[derive(Clone, Debug)]
pub enum MemberUse {
    Value,
    Call(CallArguments),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applicability {
    Applicable,
    Unknown,
    Rejected,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CallMode {
    Kotlin,
    Java,
}

pub struct Selection<'a> {
    pub entries: Vec<&'a Entry>,
    pub applicability: Applicability,
}

pub struct Candidates<'a> {
    pub members: Vec<&'a Entry>,
    pub extensions: Vec<&'a Entry>,
    local_scopes: Vec<Vec<&'a Entry>>,
    uncertain_receivers: BTreeSet<(&'a str, usize, usize)>,
    hierarchy_complete: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum MemberName<'a> {
    Exact(&'a str),
    Prefix(&'a str),
}

impl MemberName<'_> {
    fn matches(self, name: &str) -> bool {
        match self {
            Self::Exact(written) => name == written,
            Self::Prefix(written) => name.starts_with(written),
        }
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
            extensions: Vec::new(),
            local_scopes: Vec::new(),
            uncertain_receivers: BTreeSet::new(),
            hierarchy_complete: !selected.incomplete,
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
        let pool = match position {
            Some(position) => candidates_at(index, &receiver, name, ctx, position),
            None => candidates(index, &receiver, name, ctx),
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
    matching_candidates(index, receiver, MemberName::Exact(name), ctx)
}

pub fn candidates_at<'a>(
    index: &'a dyn InferenceIndex,
    receiver: &Type,
    name: &str,
    ctx: &'a FileCtx,
    position: usize,
) -> Candidates<'a> {
    matching_candidates_at(index, receiver, MemberName::Exact(name), ctx, position)
}

pub fn completion_candidates_at<'a>(
    index: &'a dyn InferenceIndex,
    receiver: &Type,
    prefix: &str,
    ctx: &'a FileCtx,
    position: usize,
) -> Candidates<'a> {
    matching_candidates_at(index, receiver, MemberName::Prefix(prefix), ctx, position)
}

fn matching_candidates_at<'a>(
    index: &'a dyn InferenceIndex,
    receiver: &Type,
    name: MemberName<'_>,
    ctx: &'a FileCtx,
    position: usize,
) -> Candidates<'a> {
    let mut result = matching_candidates(index, receiver, name, ctx);
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
) -> Candidates<'a> {
    let mut result = collect_candidates(index, receiver, name, ctx, true);
    remove_overridden_members(index, receiver, &mut result.members, ctx, false);
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
    let _ = infer::walk_supertypes(index, receiver, 32, |current| {
        let Some(root) = infer::receiver_root_key(index, current) else {
            hierarchy_complete = false;
            return ControlFlow::Continue(());
        };
        members.extend(index.members_of(&root).into_iter().filter(|entry| {
            name.matches(&entry.sym.name)
                && current.package().is_none_or(|pkg| entry.sym.package == pkg)
        }));
        if include_extensions {
            extensions.extend(index.extensions_for(&root).into_iter().filter(|entry| {
                resolve::visible_extension_names(entry, &ctx.imports, &ctx.package)
                    .any(|visible_name| name.matches(visible_name))
            }));
        }
        ControlFlow::Continue(())
    });
    if let (true, MemberName::Exact(name)) = (include_extensions, name) {
        extensions.extend(resolve::generic_receiver_extensions(
            index,
            name,
            &ctx.imports,
            &ctx.package,
        ));
    }
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
        extensions,
        local_scopes: Vec::new(),
        uncertain_receivers,
        hierarchy_complete,
    }
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
            let mut relation = if source_super
                && nearer.sym.kind == crate::symbol::SymbolKind::Property
                && inherited.sym.kind == crate::symbol::SymbolKind::Property
            {
                value_inheritance_relation(index, nearer, inherited)
            } else {
                signature_inheritance_relation(
                    index,
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
                infer::declared_member_type(index, receiver, entry, parameter, ctx),
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
        child,
        parent,
        &method_signature(index, receiver, child, ctx),
        &method_signature(index, receiver, parent, ctx),
    )
}

fn signature_inheritance_relation(
    index: &dyn InferenceIndex,
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
        && child_signature.is_some()
        && parent_signature.is_some()
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

fn parameter_identity(index: &dyn InferenceIndex, ty: Type) -> Option<Type> {
    if !infer::type_head_is_known(index, &ty) {
        return None;
    }
    let Type::Class {
        name,
        package,
        container,
        args,
        ..
    } = ty
    else {
        return None;
    };
    let builtin = container.is_none()
        && package.as_deref().is_none_or(|package| {
            package == "kotlin" || package == "java.lang" && name == "String"
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
    pub fn known_extensions(&self) -> impl Iterator<Item = &'a Entry> + '_ {
        self.extensions
            .iter()
            .copied()
            .filter(|entry| self.receiver_is_known(entry))
    }

    pub fn has_uncertain_receivers(&self) -> bool {
        !self.uncertain_receivers.is_empty()
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
        self.members.is_empty() && self.extensions.is_empty() && self.local_scopes.is_empty()
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
        match usage {
            MemberUse::Value => {
                for entries in self.groups() {
                    let values: Vec<_> = entries
                        .iter()
                        .copied()
                        .filter(|entry| entry.sym.kind.is_value_like())
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
            applicability: Applicability::Rejected,
        }
    }
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

pub(crate) fn function_applicability(
    index: &dyn InferenceIndex,
    receiver: &Type,
    entry: &Entry,
    args: &CallArguments,
    ctx: &FileCtx,
    mode: CallMode,
) -> Applicability {
    let mut uncertain = args.count.is_none()
        || args.trailing_lambda
        || extension_receiver_applicability(index, receiver, entry) == Applicability::Unknown;
    if let Some(count) = args.count {
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
    parameters_applicability(
        index,
        receiver,
        entry,
        &entry.sym.params,
        args,
        ctx,
        mode,
        uncertain,
    )
}

fn parameters_applicability(
    index: &dyn InferenceIndex,
    receiver: &Type,
    entry: &Entry,
    params: &[TypeRef],
    args: &CallArguments,
    ctx: &FileCtx,
    mode: CallMode,
    uncertain: bool,
) -> Applicability {
    if let Some(application) = infer::projected_member_application(index, receiver, entry, args) {
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
            infer::declared_member_type(index, receiver, entry, formal, ctx)
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
    if ty
        .qualification
        .as_deref()
        .is_some_and(|qualification| qualification != "kotlin")
        || (ty.qualification.as_deref() != Some("kotlin")
            && (ty.package_candidates.len() != 1 || ty.package_candidates[0] != "kotlin"))
        || ty.args.len() != arity + 1
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
            params,
            args,
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
