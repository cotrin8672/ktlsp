//! Receiver member candidates and use-site selection, shared by resolution and inference.

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

/// A competing supertype can be excluded only when its known hierarchy supplies no matching
/// declaration. Extensions are not super implementations; multiple providers stay ambiguous.
pub(crate) fn unique_super_receiver(
    index: &dyn InferenceIndex,
    receivers: Vec<Type>,
    name: MemberName<'_>,
    ctx: &FileCtx,
) -> Type {
    let mut result = None;
    for receiver in receivers {
        let candidates = matching_candidates(index, &receiver, name, ctx);
        if candidates.members.is_empty() {
            if !candidates.hierarchy_complete {
                return Type::Unknown;
            }
        } else if result.is_some() {
            return Type::Unknown;
        } else {
            result = Some(receiver);
        }
    }
    result.unwrap_or(Type::Unknown)
}

/// Select a bare member/extension call through the same innermost-first receiver stack used by
/// navigation. Rejected inner declarations cannot hide an applicable outer receiver, but remain
/// available for useful invalid-call navigation when no receiver supplies a viable declaration.
pub fn select_implicit<'a>(
    index: &'a dyn InferenceIndex,
    receivers: impl IntoIterator<Item = Type>,
    name: &str,
    usage: &MemberUse,
    ctx: &FileCtx,
) -> Option<(Type, Selection<'a>)> {
    let mut rejected = None;
    for receiver in receivers {
        let selection =
            candidates(index, &receiver, name, ctx).select(index, &receiver, usage, ctx);
        if selection.entries.is_empty() {
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

pub(crate) fn matching_candidates<'a>(
    index: &'a dyn InferenceIndex,
    receiver: &Type,
    name: MemberName<'_>,
    ctx: &FileCtx,
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
        extensions.extend(index.extensions_for(&root).into_iter().filter(|entry| {
            name.matches(&entry.sym.name)
                && resolve::extension_is_visible(entry, &ctx.imports, &ctx.package)
        }));
        ControlFlow::Continue(())
    });
    if let MemberName::Exact(name) = name {
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
    remove_overridden_members(index, receiver, &mut members, ctx);
    Candidates {
        members,
        extensions,
        hierarchy_complete,
    }
}

// shortcut: static/visibility validity needs declaration metadata before invalid overrides can be diagnosed.
fn remove_overridden_members(
    index: &dyn InferenceIndex,
    receiver: &Type,
    members: &mut Vec<&Entry>,
    ctx: &FileCtx,
) {
    if members.len() < 2 {
        return;
    }
    let signatures: Vec<Option<Vec<Type>>> = members
        .iter()
        .map(|entry| {
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
    for (position, inherited) in members.iter().enumerate() {
        keep[position] = !members.iter().enumerate().any(|(other, nearer)| {
            other != position
                && signature_inheritance_relation(
                    index,
                    nearer,
                    inherited,
                    &signatures[other],
                    &signatures[position],
                )
                .is_found()
        });
    }
    let mut position = 0;
    members.retain(|_| {
        let retain = keep[position];
        position += 1;
        retain
    });
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
                for entries in [&self.members, &self.extensions] {
                    let values: Vec<_> = entries
                        .iter()
                        .copied()
                        .filter(|entry| entry.sym.kind.is_value_like())
                        .collect();
                    if !values.is_empty() {
                        return Selection {
                            entries: values,
                            applicability: Applicability::Applicable,
                        };
                    }
                }
            }
            MemberUse::Call(args) => {
                let mut possible = Vec::new();
                let mut rejected = Vec::new();
                for functions in [true, false] {
                    for entries in [&self.members, &self.extensions] {
                        let mut viable = Vec::new();
                        let mut uncertain = false;
                        for entry in entries.iter().copied() {
                            let applicability = if functions && entry.sym.kind.is_callable_like() {
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
                                return Selection {
                                    entries: viable,
                                    applicability: Applicability::Applicable,
                                };
                            }
                            possible.extend(viable);
                        }
                    }
                }
                if !possible.is_empty() {
                    return Selection {
                        entries: possible,
                        applicability: Applicability::Unknown,
                    };
                }
                // An invalid call still names a unique declaration. Navigation can show it while
                // inference uses the rejected status rather than inventing a successful result.
                if rejected.len() == 1 {
                    return Selection {
                        entries: rejected,
                        applicability: Applicability::Rejected,
                    };
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
    let mut uncertain = args.count.is_none() || args.trailing_lambda;
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
        uncertain |=
            !infer::type_head_is_known(index, actual) || !infer::type_head_is_known(index, &formal);
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
