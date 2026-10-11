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
    #[serde(default, with = "arc_candidates")]
    pub package_candidates: Arc<Vec<String>>,
    #[serde(default)]
    pub container_candidates: Vec<String>,
}

impl TypeRef {
    pub fn simple(name: impl Into<String>) -> Self {
        TypeRef {
            name: name.into(),
            projection: Projection::Invariant,
            qualification: None,
            nullable: false,
            args: Vec::new(),
            package_candidates: Arc::new(Vec::new()),
            container_candidates: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    Class {
        name: String,
        package: Option<String>,
        container: Option<String>,
        nullable: bool,
        args: Vec<Type>,
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

    pub fn class(name: impl Into<String>, package: Option<String>) -> Self {
        Type::Class {
            name: name.into(),
            package,
            container: None,
            nullable: false,
            args: Vec::new(),
        }
    }

    pub fn name(&self) -> Option<&str> {
        match self {
            Type::Class { name, .. } => Some(name),
            Type::Unknown | Type::Projected { .. } | Type::Star => None,
        }
    }

    pub fn package(&self) -> Option<&str> {
        match self {
            Type::Class { package, .. } => package.as_deref(),
            Type::Unknown | Type::Projected { .. } | Type::Star => None,
        }
    }

    pub fn container(&self) -> Option<&str> {
        match self {
            Type::Class { container, .. } => container.as_deref(),
            Type::Unknown | Type::Projected { .. } | Type::Star => None,
        }
    }

    pub fn is_nullable(&self) -> bool {
        matches!(self, Type::Class { nullable: true, .. })
    }

    pub fn args(&self) -> &[Type] {
        match self {
            Type::Class { args, .. } => args,
            Type::Unknown | Type::Projected { .. } | Type::Star => &[],
        }
    }

    pub fn into_non_null(self) -> Type {
        match self {
            Type::Class {
                name,
                package,
                container,
                args,
                ..
            } => Type::Class {
                name,
                package,
                container,
                nullable: false,
                args,
            },
            other => other,
        }
    }
}
