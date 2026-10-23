//! Lightweight type values for compiler-free (no-kotlinc) inference.
//!
//! Two representations, on purpose:
//! - [`TypeRef`] is what the *index* stores for a declaration's return/property/parameter type: a
//!   simple name, nullability, raw type-args, plus declaration-context package/container
//!   candidates. The candidates keep `fun f(): Bar` tied to the imports/package of the file that
//!   declared `f`; qualified or ambiguous references are checked against indexed identities.
//!   `TypeRef` is serialized into the symcache.
//! - [`Type`] is a type *resolved at a use site*: a simple name plus the package/container we
//!   resolved it to.
//!   [`Type::Unknown`] is a first-class, non-failing outcome that drives the silent-omission
//!   contract (no members → show nothing). `Type` is runtime-only (never serialized).

use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// `package_candidates` is shared, not owned per `TypeRef`: for a given declaration file the
/// ordinary candidate list (package + wildcard imports + default imports) is shared across
/// unresolved references. Explicit imports and synthetic types may have different lists. The serde shim
/// keeps the bincode symcache wire format identical to the previous `Vec<String>` encoding.
mod arc_candidates {
    use super::Arc;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(
        value: &Arc<Vec<String>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.as_ref().serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Arc<Vec<String>>, D::Error> {
        Vec::<String>::deserialize(deserializer).map(Arc::new)
    }
}

/// Use-site projection. The bound stays in the same TypeRef, preserving declaration context.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
#[repr(u8)]
pub enum Projection {
    #[default]
    Invariant = 0,
    Out = 1,
    In = 2,
    Star = 3,
}

impl From<Projection> for u8 {
    fn from(value: Projection) -> Self {
        value as u8
    }
}

impl TryFrom<u8> for Projection {
    type Error = &'static str;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Invariant),
            1 => Ok(Self::Out),
            2 => Ok(Self::In),
            3 => Ok(Self::Star),
            _ => Err("invalid type projection"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TypeRef {
    pub name: String,
    #[serde(default)]
    pub projection: Projection,
    /// The syntactic qualifier before the simple name, with explicit imports expanded.
    /// Resolution matches it against indexed package/container identities; casing is irrelevant.
    #[serde(default)]
    pub qualification: Option<String>,
    #[serde(default)]
    pub nullable: bool,
    #[serde(default)]
    pub args: Vec<TypeRef>,
    /// Instantiated lexical owner of a nonstatic nested type; arguments stay on each segment.
    #[serde(default)]
    pub owner: Option<Box<TypeRef>>,
    #[serde(default, with = "arc_candidates")]
    pub package_candidates: Arc<Vec<String>>,
    #[serde(default)]
    pub container_candidates: Vec<String>,
    /// Java syntax omitted type arguments; resolution must still prove a generic declaration.
    #[serde(default)]
    pub java_raw_eligible: bool,
}

impl TypeRef {
    /// Peel one normalized Java dimension, retaining the component's lexical provenance.
    pub(crate) fn array_component(&self) -> Option<Self> {
        let name = self.name.strip_suffix("[]")?;
        let head = name.trim_end_matches("[]");
        if head.is_empty() || head.contains(['[', ']']) {
            return None;
        }
        let mut component = self.clone();
        component.name = name.to_string();
        component.nullable = false;
        component.projection = Projection::Invariant;
        Some(component)
    }

    pub fn simple(name: impl Into<String>) -> Self {
        TypeRef {
            name: name.into(),
            projection: Projection::Invariant,
            qualification: None,
            nullable: false,
            args: Vec::new(),
            package_candidates: Arc::new(Vec::new()),
            container_candidates: Vec::new(),

            owner: None,
            java_raw_eligible: false,
        }
    }
}

/// Lexical declaration facts supplied by a syntax adapter, nearest scope first.
/// Source-property caches may persist this declaration proof; inferred runtime types stay separate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeParameterBounds {
    pub name: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub scope_start: usize,
    pub scope_end: usize,
    pub upper_bounds: Vec<TypeRef>,
    pub complete: bool,
    /// Declared variance, independently of use-site projections. Legacy facts remain unknown.
    #[serde(default)]
    pub variance: Option<Projection>,
    /// JVM bound selection supplied by the producer; absent legacy facts cannot prove erasure.
    #[serde(default)]
    pub erasure: Option<ParameterErasure>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParameterErasure {
    FirstBound,
    ClassFirst,
}

/// Keep a declaration's own facts while retaining lexical blockers from static/nested scopes.
pub fn own_parameter_bounds(
    parameters: Vec<TypeParameterBounds>,
    names: &[String],
    start: usize,
    end: usize,
) -> Option<Box<Vec<TypeParameterBounds>>> {
    fn accessible(reference: &TypeRef, parameters: &[TypeParameterBounds]) -> bool {
        (reference.qualification.is_some()
            || parameters
                .iter()
                .find(|parameter| parameter.name == reference.name)
                .is_none_or(|parameter| parameter.complete))
            && reference
                .owner
                .as_deref()
                .is_none_or(|owner| accessible(owner, parameters))
            && reference
                .args
                .iter()
                .all(|argument| accessible(argument, parameters))
    }
    let own: Vec<_> = parameters
        .iter()
        .filter(|parameter| parameter.scope_start == start && parameter.scope_end == end)
        .map(|parameter| {
            let mut own = parameter.clone();
            own.complete &= own
                .upper_bounds
                .iter()
                .all(|bound| accessible(bound, &parameters));
            own
        })
        .collect();
    (!names.is_empty()
        && own.len() == names.len()
        && own
            .iter()
            .zip(names)
            .all(|(parameter, name)| parameter.name == *name))
    .then(|| Box::new(own))
}

/// Keep `T` distinct from `T?` even when an implicit `Any?` bound makes both nullable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParameterNullability {
    Inherit,
    Nullable,
    NonNull,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    /// Proven Java raw use. Keep it opaque to ordinary generic equality and inference.
    Raw(Box<Type>),
    /// A Java array has structural rank and a component, without an indexed nominal owner.
    Array {
        component: Box<Type>,
        nullable: bool,
    },
    Class {
        name: String,
        package: Option<String>,
        container: Option<String>,
        nullable: bool,
        args: Vec<Type>,
        owner: Option<Box<Type>>,
    },
    Parameter {
        definition: crate::symbol::Def,
        name: String,
        nullable: bool,
        nullability: ParameterNullability,
        upper_bounds: Vec<Type>,
    },
    /// Only projected arguments allocate a bound; ordinary Class values keep their layout.
    Projected {
        projection: Projection,
        bound: Box<Type>,
    },
    Star,
    Unknown,
}

impl Type {
    pub fn projected(projection: Projection, bound: Type) -> Type {
        match projection {
            Projection::Invariant => bound,
            Projection::Star => Type::Star,
            Projection::Out | Projection::In => Type::Projected {
                projection,
                bound: Box::new(bound),
            },
        }
    }

    pub fn projection(&self) -> Projection {
        match self {
            Type::Projected { projection, .. } => *projection,
            Type::Star => Projection::Star,
            _ => Projection::Invariant,
        }
    }

    pub fn array(component: Type, nullable: bool) -> Self {
        Type::Array {
            component: Box::new(component),
            nullable,
        }
    }

    pub fn class(name: impl Into<String>, package: Option<String>) -> Self {
        Type::Class {
            name: name.into(),
            package,
            container: None,
            nullable: false,
            args: Vec::new(),

            owner: None,
        }
    }

    pub fn name(&self) -> Option<&str> {
        match self {
            Type::Raw(inner) => inner.name(),
            Type::Class { name, .. } => Some(name),
            Type::Unknown
            | Type::Array { .. }
            | Type::Parameter { .. }
            | Type::Projected { .. }
            | Type::Star => None,
        }
    }

    pub fn package(&self) -> Option<&str> {
        match self {
            Type::Raw(inner) => inner.package(),
            Type::Class { package, .. } => package.as_deref(),
            Type::Unknown
            | Type::Array { .. }
            | Type::Parameter { .. }
            | Type::Projected { .. }
            | Type::Star => None,
        }
    }

    pub fn container(&self) -> Option<&str> {
        match self {
            Type::Raw(inner) => inner.container(),
            Type::Class { container, .. } => container.as_deref(),
            Type::Unknown
            | Type::Array { .. }
            | Type::Parameter { .. }
            | Type::Projected { .. }
            | Type::Star => None,
        }
    }

    pub fn is_nullable(&self) -> bool {
        if let Type::Raw(inner) = self {
            return inner.is_nullable();
        }
        matches!(
            self,
            Type::Class { nullable: true, .. }
                | Type::Parameter { nullable: true, .. }
                | Type::Array { nullable: true, .. }
        )
    }

    /// Parameters and arrays expose useful members without a nominal head.
    pub fn is_member_receiver(&self) -> bool {
        matches!(
            self,
            Type::Class { .. } | Type::Parameter { .. } | Type::Array { .. } | Type::Raw(_)
        )
    }

    pub fn with_nullability(mut self, value: bool) -> Self {
        match &mut self {
            Type::Raw(inner) => **inner = inner.clone().with_nullability(value),
            Type::Parameter {
                nullable,
                nullability,
                ..
            } => {
                *nullable = value;
                *nullability = if value {
                    ParameterNullability::Nullable
                } else {
                    ParameterNullability::NonNull
                };
            }
            Type::Class { nullable, .. } | Type::Array { nullable, .. } => *nullable = value,
            _ => {}
        }
        self
    }

    pub fn owner(&self) -> Option<&Type> {
        match self {
            Type::Class { owner, .. } => owner.as_deref(),
            Type::Projected { bound, .. } | Type::Raw(bound) => bound.owner(),
            _ => None,
        }
    }

    pub fn args(&self) -> &[Type] {
        match self {
            Type::Class { args, .. } => args,
            Type::Raw(inner) => inner.args(),
            Type::Unknown
            | Type::Array { .. }
            | Type::Parameter { .. }
            | Type::Projected { .. }
            | Type::Star => &[],
        }
    }

    pub fn into_non_null(self) -> Type {
        self.with_nullability(false)
    }
}
