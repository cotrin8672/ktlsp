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

pub struct Selection<'a> {
    pub entries: Vec<&'a Entry>,
    pub applicability: Applicability,
}

pub struct Candidates<'a> {
    pub members: Vec<&'a Entry>,
    pub extensions: Vec<&'a Entry>,
}

/// One typed traversal for own/inherited members and visible extensions. Artifact ownership is
/// provided by the borrowed index view; no language or project-specific fallback belongs here.
pub fn candidates<'a>(
    index: &'a dyn InferenceIndex,
    receiver: &Type,
    name: &str,
    ctx: &FileCtx,
) -> Candidates<'a> {
    let mut members = Vec::new();
    let mut extensions = Vec::new();
    let _ = infer::walk_supertypes(index, receiver, 32, |current| {
        let Some(root) = infer::receiver_root_key(index, current) else {
            return ControlFlow::Continue(());
        };
        members.extend(index.members_of(&root).into_iter().filter(|entry| {
            entry.sym.name == name && current.package().is_none_or(|pkg| entry.sym.package == pkg)
        }));
        extensions.extend(index.extensions_for(&root).into_iter().filter(|entry| {
            entry.sym.name == name
                && resolve::extension_is_visible(entry, &ctx.imports, &ctx.package)
        }));
        ControlFlow::Continue(())
    });
    extensions.extend(resolve::generic_receiver_extensions(
        index,
        name,
        &ctx.imports,
        &ctx.package,
    ));
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
    Candidates {
        members,
        extensions,
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
                                function_applicability(index, receiver, entry, args, ctx)
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
    function_applicability(index, receiver, entry, args, ctx) != Applicability::Rejected
}

fn function_applicability(
    index: &dyn InferenceIndex,
    receiver: &Type,
    entry: &Entry,
    args: &CallArguments,
    ctx: &FileCtx,
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
    mut uncertain: bool,
) -> Applicability {
    uncertain |= args.count.is_some_and(|count| args.types.len() < count);
    for (position, actual) in args.types.iter().enumerate() {
        let formal = params
            .get(position)
            .or_else(|| entry.sym.has_vararg.then(|| params.last()).flatten());
        let Some(formal) = formal else {
            uncertain = true;
            continue;
        };
        if entry.sym.has_vararg
            && position >= params.len().saturating_sub(1)
            && formal.name.ends_with("[]")
        {
            // A legacy array reference has not retained a normalized component type.
            uncertain = true;
            continue;
        }
        let formal = infer::declared_member_type(index, receiver, entry, formal, ctx);
        if !infer::types_consistent(index, actual, &formal) {
            return Applicability::Rejected;
        }
        uncertain |= matches!(actual, Type::Unknown) || matches!(formal, Type::Unknown);
    }
    if uncertain {
        Applicability::Unknown
    } else {
        Applicability::Applicable
    }
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
