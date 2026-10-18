//! Per-application generic substitution and projected argument constraints.
//! Ordinary inference keeps its local structural unifier. Projection-aware callers use the
//! bounded equality/lower/upper slice below; there is no cross-statement propagation or global
//! solver. Unsupported constraints retain Unknown and possible overloads.

use std::collections::{HashMap, HashSet};

use crate::types::{Type, TypeRef};

/// Match a formal parameter type `formal` against an actual argument type `actual`, binding any formal
/// type-variable names (those in `tparams`) into `subst`. Match-don't-propagate:
/// - `formal` is a type variable -> bind it to `actual` (first binding wins; never overwrite).
/// - heads match (`List` vs `List`) -> recurse positionally into type arguments (`List<T>` vs
///   `List<Foo>` binds `T := Foo`).
/// - heads differ -> bind nothing (stay silent).
///
/// No occurs-check is needed because nothing is ever propagated (we only read `actual`, never a
/// partially-built binding), but a self-referential bind is refused defensively.
pub fn unify_into(
    formal: &TypeRef,
    actual: &Type,
    tparams: &HashSet<String>,
    subst: &mut HashMap<String, Type>,
) {
    if formal.projection != crate::types::Projection::Invariant
        || actual.projection() != crate::types::Projection::Invariant
    {
        return;
    }
    if tparams.contains(&formal.name) {
        // Refuse to bind a variable to itself (e.g. an unresolved actual carrying the same name).
        if actual.name() == Some(formal.name.as_str()) {
            return;
        }
        subst
            .entry(formal.name.clone())
            .or_insert_with(|| actual.clone());
        return;
    }
    if Some(formal.name.as_str()) == actual.name() {
        for (f, a) in formal.args.iter().zip(actual.args()) {
            unify_into(f, a, tparams, subst);
        }
    }
}

/// Result of a single projected application; unknown constraints never remove an overload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstraintStatus {
    Matched,
    Rejected,
    Unknown,
}

pub struct Application {
    pub status: ConstraintStatus,
    pub bindings: HashMap<String, Type>,
}

pub fn has_projection(ty: &TypeRef) -> bool {
    ty.projection != crate::types::Projection::Invariant || ty.args.iter().any(has_projection)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Relation {
    Equal,
    Lower,
    Upper,
}

struct Constraint<'a> {
    parameter: &'a str,
    relation: Relation,
    actual: &'a Type,
}

/// A bounded local slice, not a global solver: equality plus one identical lower candidate.
/// All collected constraints are rechecked after choosing a candidate. Upper-only variables,
/// capture conversion and different lower candidates cannot manufacture an exact type.
pub fn constrain_application(
    formals: &[TypeRef],
    actuals: &[Type],
    parameters: &HashSet<String>,
    initial: &HashMap<String, Type>,
    known_unbounded: bool,
    mut resolve: impl FnMut(&TypeRef) -> Type,
    mut known: impl FnMut(&Type) -> bool,
    mut invariant: impl FnMut(&Type) -> bool,
    mut subtype: impl FnMut(&Type, &Type) -> bool,
    mut distinct: impl FnMut(&Type, &Type) -> bool,
) -> Application {
    use crate::types::Projection;
    fn combine(a: ConstraintStatus, b: ConstraintStatus) -> ConstraintStatus {
        match (a, b) {
            (ConstraintStatus::Rejected, _) | (_, ConstraintStatus::Rejected) => {
                ConstraintStatus::Rejected
            }
            (ConstraintStatus::Unknown, _) | (_, ConstraintStatus::Unknown) => {
                ConstraintStatus::Unknown
            }
            _ => ConstraintStatus::Matched,
        }
    }
    fn collect<'a>(
        formal: &'a TypeRef,
        actual: &'a Type,
        parameters: &HashSet<String>,
        relation: Relation,
        projection_allowed: bool,
        constraints: &mut Vec<Constraint<'a>>,
        resolve: &mut impl FnMut(&TypeRef) -> Type,
        known: &mut impl FnMut(&Type) -> bool,
        invariant: &mut impl FnMut(&Type) -> bool,
        subtype: &mut impl FnMut(&Type, &Type) -> bool,
        distinct: &mut impl FnMut(&Type, &Type) -> bool,
        depth: usize,
    ) -> ConstraintStatus {
        if depth > crate::infer::MAX_DEPTH {
            return ConstraintStatus::Unknown;
        }
        // Wildcard containment is allowed at a subtype argument boundary, never through
        // an invariant equality slot (Pair<Class<X>> is not Pair<Class<? extends X>>).
        if formal.projection == Projection::Star
            || formal.projection != Projection::Invariant && !projection_allowed
        {
            return ConstraintStatus::Unknown;
        }
        let actual = match actual {
            Type::Projected {
                projection: Projection::Out,
                bound,
            } if formal.projection == Projection::Out && projection_allowed => bound.as_ref(),
            actual if actual.projection() == Projection::Invariant => actual,
            _ => return ConstraintStatus::Unknown,
        };
        let relation = match formal.projection {
            Projection::Out => Relation::Lower,
            Projection::In => Relation::Upper,
            _ => relation,
        };
        if formal.name.is_empty() {
            return ConstraintStatus::Unknown;
        }
        if formal.qualification.is_none() && parameters.contains(&formal.name) {
            if formal.nullable
                || !formal.args.is_empty()
                || !known(actual)
                || actual.is_nullable() && !matches!(actual, Type::Parameter { .. })
            {
                return ConstraintStatus::Unknown;
            }
            constraints.push(Constraint {
                parameter: &formal.name,
                relation,
                actual,
            });
            return ConstraintStatus::Matched;
        }
        let resolved = resolve(formal);
        if resolved.name().is_none() || actual.name().is_none() {
            return ConstraintStatus::Unknown;
        }
        let same_head = resolved.name() == actual.name()
            && resolved.package() == actual.package()
            && resolved.container() == actual.container();
        if !same_head {
            if known(&resolved) && known(actual) {
                if relation == Relation::Equal && distinct(actual, &resolved) {
                    return ConstraintStatus::Rejected;
                }
                if (relation == Relation::Lower && subtype(actual, &resolved))
                    || (relation == Relation::Upper && subtype(&resolved, actual))
                {
                    return ConstraintStatus::Matched;
                }
            }
            return ConstraintStatus::Unknown;
        }
        if actual.is_nullable() && !formal.nullable
            || relation == Relation::Equal && actual.is_nullable() != formal.nullable
        {
            return ConstraintStatus::Unknown;
        }
        if formal.args.len() != actual.args().len() {
            return ConstraintStatus::Unknown;
        }
        if formal.args.is_empty() && (!known(&resolved) || !known(actual)) {
            return ConstraintStatus::Unknown;
        }
        if !formal.args.is_empty() && !invariant(actual) {
            return ConstraintStatus::Unknown;
        }
        formal
            .args
            .iter()
            .zip(actual.args())
            .fold(ConstraintStatus::Matched, |status, (f, a)| {
                combine(
                    status,
                    collect(
                        f,
                        a,
                        parameters,
                        Relation::Equal,
                        relation == Relation::Lower,
                        constraints,
                        resolve,
                        known,
                        invariant,
                        subtype,
                        distinct,
                        depth + 1,
                    ),
                )
            })
    }
    let mut application = Application {
        status: ConstraintStatus::Unknown,
        bindings: initial.clone(),
    };
    if !known_unbounded || formals.len() != actuals.len() {
        return application;
    }
    let mut constraints = Vec::new();
    let mut status = ConstraintStatus::Matched;
    // Extension receiver variables are already fixed by receiver equality. Value arguments
    // may validate those bindings, but must not overwrite them with a different lower bound.
    for (parameter, actual) in initial
        .iter()
        .filter(|(parameter, _)| parameters.contains(*parameter))
    {
        if known(actual) && (!actual.is_nullable() || matches!(actual, Type::Parameter { .. })) {
            constraints.push(Constraint {
                parameter,
                relation: Relation::Equal,
                actual,
            });
        } else {
            status = combine(status, ConstraintStatus::Unknown);
        }
    }
    for (formal, actual) in formals.iter().zip(actuals) {
        status = combine(
            status,
            collect(
                formal,
                actual,
                parameters,
                Relation::Lower,
                false,
                &mut constraints,
                &mut resolve,
                &mut known,
                &mut invariant,
                &mut subtype,
                &mut distinct,
                0,
            ),
        );
    }
    for parameter in parameters {
        let mut equal: Option<&Type> = None;
        let mut lower: Option<&Type> = None;
        let mut different_lowers = false;
        for constraint in constraints
            .iter()
            .filter(|c| c.parameter == parameter.as_str())
        {
            match constraint.relation {
                Relation::Equal => match equal {
                    Some(previous) if previous != constraint.actual => {
                        status = combine(
                            status,
                            if distinct(previous, constraint.actual) {
                                ConstraintStatus::Rejected
                            } else {
                                ConstraintStatus::Unknown
                            },
                        )
                    }
                    None => equal = Some(constraint.actual),
                    _ => {}
                },
                Relation::Lower => match lower {
                    Some(previous) if previous != constraint.actual => different_lowers = true,
                    None => lower = Some(constraint.actual),
                    _ => {}
                },
                Relation::Upper => {}
            }
        }
        let candidate = equal.or_else(|| (!different_lowers).then_some(lower).flatten());
        if let Some(candidate) = candidate {
            for constraint in constraints
                .iter()
                .filter(|c| c.parameter == parameter.as_str())
            {
                let proved = match constraint.relation {
                    Relation::Equal => candidate == constraint.actual,
                    Relation::Lower => {
                        candidate == constraint.actual || subtype(constraint.actual, candidate)
                    }
                    Relation::Upper => {
                        candidate == constraint.actual || subtype(candidate, constraint.actual)
                    }
                };
                if !proved {
                    status = combine(status, ConstraintStatus::Unknown);
                }
            }
            application
                .bindings
                .insert(parameter.clone(), candidate.clone());
        } else if constraints
            .iter()
            .any(|c| c.parameter == parameter.as_str())
        {
            status = combine(status, ConstraintStatus::Unknown);
        }
    }
    // A partially understood argument cannot leave a seemingly precise projected-call result.
    if status != ConstraintStatus::Matched {
        for parameter in parameters {
            application.bindings.remove(parameter);
        }
    }
    application.status = status;
    application
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tparams(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn binds_a_bare_type_variable() {
        let mut subst = HashMap::new();
        unify_into(
            &TypeRef::simple("T"),
            &Type::class("Foo", Some("app".into())),
            &tparams(&["T"]),
            &mut subst,
        );
        assert_eq!(subst.get("T").and_then(|t| t.name()), Some("Foo"));
    }

    #[test]
    fn binds_through_matching_head() {
        // formal List<T> vs actual List<Foo> binds T := Foo.
        let formal = TypeRef {
            name: "List".into(),
            projection: Default::default(),
            qualification: None,
            nullable: false,
            args: vec![TypeRef::simple("T")],
            package_candidates: std::sync::Arc::new(Vec::new()),
            container_candidates: Vec::new(),
        };
        let actual = Type::Class {
            name: "List".into(),
            package: None,
            container: None,
            nullable: false,
            args: vec![Type::class("Foo", None)],
        };
        let mut subst = HashMap::new();
        unify_into(&formal, &actual, &tparams(&["T"]), &mut subst);
        assert_eq!(subst.get("T").and_then(|t| t.name()), Some("Foo"));
    }

    #[test]
    fn differing_heads_bind_nothing() {
        let mut subst = HashMap::new();
        unify_into(
            &TypeRef::simple("List"),
            &Type::class("Set", None),
            &tparams(&["T"]),
            &mut subst,
        );
        assert!(subst.is_empty());
    }

    #[test]
    fn first_binding_wins() {
        let mut subst = HashMap::new();
        let tp = tparams(&["T"]);
        unify_into(
            &TypeRef::simple("T"),
            &Type::class("A", None),
            &tp,
            &mut subst,
        );
        unify_into(
            &TypeRef::simple("T"),
            &Type::class("B", None),
            &tp,
            &mut subst,
        );
        assert_eq!(subst.get("T").and_then(|t| t.name()), Some("A"));
    }
}
