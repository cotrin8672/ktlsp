//! Core, LSP-independent data types: symbol kinds, indexed symbols, and definition results.

use serde::{Deserialize, Serialize};

use crate::types::TypeRef;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberImplementation {
    Concrete,
    Abstract,
    Constructor,
}

/// Source-call facts are independent of whether a JVM descriptor can be certified.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceMemberFlags(u8);

impl SourceMemberFlags {
    pub(crate) fn new(
        implementation: MemberImplementation,
        dispatch: crate::extensions::Dispatch,
        visibility: crate::extensions::Visibility,
        internal: bool,
        jvm_restricted: bool,
    ) -> Self {
        use crate::extensions::{Dispatch, Visibility};
        let implementation = match implementation {
            MemberImplementation::Concrete => 1,
            MemberImplementation::Abstract => 2,
            MemberImplementation::Constructor => 3,
        };
        let dispatch = match dispatch {
            Dispatch::Instance => 1,
            Dispatch::Static => 2,
        };
        let access = if internal {
            5
        } else {
            match visibility {
                Visibility::Public => 1,
                Visibility::Protected => 2,
                Visibility::Package => 3,
                Visibility::Private => 4,
            }
        };
        Self(implementation | dispatch << 2 | access << 4 | u8::from(jvm_restricted) << 7)
    }

    pub fn implementation(self) -> Option<MemberImplementation> {
        match self.0 & 3 {
            1 => Some(MemberImplementation::Concrete),
            2 => Some(MemberImplementation::Abstract),
            3 => Some(MemberImplementation::Constructor),
            _ => None,
        }
    }

    pub fn dispatch(self) -> Option<crate::extensions::Dispatch> {
        use crate::extensions::Dispatch;
        match self.0 >> 2 & 3 {
            1 => Some(Dispatch::Instance),
            2 => Some(Dispatch::Static),
            _ => None,
        }
    }

    pub fn visibility(self) -> Option<crate::extensions::Visibility> {
        use crate::extensions::Visibility;
        match self.0 >> 4 & 7 {
            1 => Some(Visibility::Public),
            2 => Some(Visibility::Protected),
            3 => Some(Visibility::Package),
            4 => Some(Visibility::Private),
            _ => None,
        }
    }

    pub fn is_internal(self) -> bool {
        self.0 >> 4 & 7 == 5
    }
    pub fn jvm_restricted(self) -> bool {
        self.0 & 128 != 0
    }
}

/// Authored Kotlin call syntax, distinct from names in a lowered JVM signature.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KotlinValueParameter {
    pub name: String,
    pub has_default: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SymbolKind {
    Class,
    Interface,
    Object,
    EnumClass,
    TypeAlias,
    EnumEntry,
    Function,
    Property,
    Parameter,
    TypeParameter,
    LocalVariable,
}

impl SymbolKind {
    pub fn is_type_like(self) -> bool {
        use SymbolKind::*;
        matches!(
            self,
            Class | Interface | Object | EnumClass | TypeAlias | TypeParameter
        )
    }

    pub fn is_callable_like(self) -> bool {
        use SymbolKind::*;
        matches!(self, Function | Class | Object | EnumClass)
    }

    pub fn is_value_like(self) -> bool {
        use SymbolKind::*;
        matches!(
            self,
            Property | Object | EnumEntry | Parameter | LocalVariable
        )
    }
}

/// Independent declaration facts: method bounds and constructor variance are not interchangeable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TypeParameterFacts(u8);

impl TypeParameterFacts {
    pub fn new(unbounded: bool, invariant: bool) -> Self {
        Self(u8::from(unbounded) | (u8::from(invariant) << 1))
    }
    pub fn known_unbounded(self) -> bool {
        self.0 & 1 != 0
    }
    pub fn known_invariant(self) -> bool {
        self.0 & 2 != 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionReceiverFacts {
    pub ty: TypeRef,
    /// Inline upper bounds retain the declaring file's type identities.
    #[serde(default)]
    pub upper_bounds: Vec<(String, TypeRef)>,
    /// Unmodeled constraints, including where clauses and projected receiver templates.
    pub has_type_constraints: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexedSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub package: String,
    pub container: Option<String>,
    pub start_byte: usize,
    pub end_byte: usize,
    #[serde(default)]
    pub documentation: Option<String>,
    #[serde(default)]
    pub supertypes: Vec<String>,
    /// Declared superclass/interface types, including arguments and declaration-file visibility.
    #[serde(default)]
    pub supertype_refs: Vec<TypeRef>,
    #[serde(default)]
    pub ext_receiver: Option<String>,
    /// The declaration-scoped receiver template; keep absent facts out of line.
    #[serde(default)]
    pub extension_receiver: Option<Box<ExtensionReceiverFacts>>,
    #[serde(default)]
    pub arity: Option<u8>,
    #[serde(default)]
    pub min_arity: Option<u8>,
    #[serde(default)]
    pub has_vararg: bool,
    #[serde(default)]
    pub trailing_lambda_min_arity: Option<u8>,
    /// Minimum arity when the final non-vararg parameter is supplied separately from preceding
    /// defaulted parameters. Used only after inference proves that final parameter is callable.
    #[serde(default)]
    pub last_parameter_min_arity: Option<u8>,
    /// Keep specialized receiver facts out of line so absent slots cost only a pointer.
    #[serde(default)]
    pub trailing_lambda_receiver_type: Option<Box<TypeRef>>,
    /// Receiver type of a type alias whose target is a receiver function type, for example
    /// `typealias Handler = suspend RoutingContext.() -> Unit`.
    #[serde(default)]
    pub function_type_receiver: Option<Box<TypeRef>>,
    #[serde(default)]
    pub return_type: Option<TypeRef>,
    #[serde(default)]
    pub value_type: Option<TypeRef>,
    #[serde(default)]
    pub params: Vec<TypeRef>,
    /// Verified authored Kotlin names and default positions, in the same order as `params`.
    /// Missing facts do not authorize names/defaults on Java/JVM or incomplete declarations.
    #[serde(default)]
    pub kotlin_parameters: Option<Box<Vec<KotlinValueParameter>>>,
    #[serde(default)]
    pub type_params: Vec<String>,
    #[serde(default)]
    pub type_parameter_facts: TypeParameterFacts,
    #[serde(default)]
    pub source_member: SourceMemberFlags,
}

impl IndexedSymbol {
    pub fn new(
        name: impl Into<String>,
        kind: SymbolKind,
        package: impl Into<String>,
        container: Option<String>,
        start_byte: usize,
        end_byte: usize,
    ) -> Self {
        IndexedSymbol {
            name: name.into(),
            kind,
            package: package.into(),
            container,
            start_byte,
            end_byte,
            documentation: None,
            supertypes: Vec::new(),
            supertype_refs: Vec::new(),
            ext_receiver: None,
            extension_receiver: None,
            arity: None,
            min_arity: None,
            has_vararg: false,
            trailing_lambda_min_arity: None,
            last_parameter_min_arity: None,
            trailing_lambda_receiver_type: None,
            function_type_receiver: None,
            return_type: None,
            value_type: None,
            params: Vec::new(),
            kotlin_parameters: None,
            type_params: Vec::new(),
            type_parameter_facts: TypeParameterFacts::default(),
            source_member: SourceMemberFlags::default(),
        }
    }
}

/// Restore declaration-context sharing lost when a cache decodes each `Arc` by value.
pub fn reshare_file_type_contexts(
    symbols: &mut [IndexedSymbol],
    jvm: Option<&mut crate::jvm::SourceFileFacts>,
) {
    use crate::fxhash::FxBuildHasher;
    use std::collections::HashSet;
    use std::sync::Arc;

    fn visit(ty: &mut TypeRef, pool: &mut HashSet<Arc<Vec<String>>, FxBuildHasher>) {
        if let Some(shared) = pool.get(&ty.package_candidates) {
            if !Arc::ptr_eq(shared, &ty.package_candidates) {
                ty.package_candidates = Arc::clone(shared);
            }
        } else {
            pool.insert(Arc::clone(&ty.package_candidates));
        }
        for arg in &mut ty.args {
            visit(arg, pool);
        }
    }

    let mut pool = HashSet::with_hasher(FxBuildHasher::default());
    for symbol in symbols {
        if let Some(receiver) = symbol.extension_receiver.as_deref_mut() {
            visit(&mut receiver.ty, &mut pool);
            for (_, bound) in &mut receiver.upper_bounds {
                visit(bound, &mut pool);
            }
        }
        for ty in symbol
            .supertype_refs
            .iter_mut()
            .chain(symbol.trailing_lambda_receiver_type.as_deref_mut())
            .chain(symbol.function_type_receiver.as_deref_mut())
            .chain(symbol.return_type.iter_mut())
            .chain(symbol.value_type.iter_mut())
            .chain(symbol.params.iter_mut())
        {
            visit(ty, &mut pool);
        }
    }
    if let Some(jvm) = jvm {
        if let Some(ty) = &mut jvm.owner_annotation {
            visit(ty, &mut pool);
        }
        for callable in &mut jvm.callables {
            for ty in callable
                .params
                .iter_mut()
                .chain(std::iter::once(&mut callable.return_type))
                .chain(callable.jvm_name_annotation.iter_mut())
            {
                visit(ty, &mut pool);
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Def {
    pub file: String,
    pub start_byte: usize,
    pub end_byte: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn reshare_file_type_contexts_preserves_values_and_distinct_ordered_scopes() {
        fn scoped(name: &str, packages: &[&str]) -> TypeRef {
            TypeRef {
                package_candidates: Arc::new(packages.iter().map(|p| (*p).to_string()).collect()),
                ..TypeRef::simple(name)
            }
        }
        let packages = ["lib", "wildcard", "kotlin"];
        let ty = || scoped("Payload", &packages);
        let mut symbol = IndexedSymbol::new("method", SymbolKind::Function, "lib", None, 1, 2);
        let mut parent = ty();
        parent.args.push(ty());
        symbol.supertype_refs = vec![parent.clone()];
        symbol.extension_receiver = Some(Box::new(ExtensionReceiverFacts {
            ty: ty(),
            upper_bounds: vec![("T".into(), parent)],
            has_type_constraints: false,
        }));
        symbol.trailing_lambda_receiver_type = Some(Box::new(ty()));
        symbol.function_type_receiver = Some(Box::new(ty()));
        symbol.return_type = Some(TypeRef {
            name: "Qualified".into(),
            projection: Default::default(),
            qualification: Some("other".into()),
            nullable: true,
            ..ty()
        });
        symbol.value_type = Some(TypeRef {
            container_candidates: vec!["Outer".into()],
            ..ty()
        });
        symbol.params = vec![
            ty(),
            scoped("Reversed", &["kotlin", "wildcard", "lib"]),
            scoped("Duplicated", &["lib", "lib"]),
            scoped("Imported", &["lib"]),
            scoped("Function1", &["kotlin"]),
            scoped("Empty", &[]),
            scoped("OtherEmpty", &[]),
        ];
        let mut symbols = vec![symbol];
        let mut jvm = crate::jvm::SourceFileFacts {
            input_hash: 42,
            package: "lib".into(),
            owner: "Facade".into(),
            owner_annotation: Some(ty()),
            callables: vec![crate::jvm::CallableFacts {
                name: "lowered".into(),
                params: vec![ty()],
                return_type: ty(),
                type_params: Vec::new(),
                definition: None,
                fixed_name: None,
                jvm_name_annotation: Some(ty()),
                is_static: true,
            }],
        };
        let before = (symbols.clone(), jvm.clone());
        reshare_file_type_contexts(&mut symbols, Some(&mut jvm));
        assert_eq!((&symbols, &jvm), (&before.0, &before.1));
        let symbol = &symbols[0];
        let shared = Arc::clone(&symbol.supertype_refs[0].package_candidates);
        for ty in [
            &symbol.supertype_refs[0].args[0],
            &symbol.extension_receiver.as_deref().unwrap().ty,
            &symbol.extension_receiver.as_deref().unwrap().upper_bounds[0].1,
            &symbol.extension_receiver.as_deref().unwrap().upper_bounds[0]
                .1
                .args[0],
            symbol.trailing_lambda_receiver_type.as_deref().unwrap(),
            symbol.function_type_receiver.as_deref().unwrap(),
            symbol.return_type.as_ref().unwrap(),
            symbol.value_type.as_ref().unwrap(),
            &symbol.params[0],
            jvm.owner_annotation.as_ref().unwrap(),
            &jvm.callables[0].params[0],
            &jvm.callables[0].return_type,
            jvm.callables[0].jvm_name_annotation.as_ref().unwrap(),
        ] {
            assert!(Arc::ptr_eq(&shared, &ty.package_candidates));
        }
        for ty in &symbol.params[1..] {
            assert!(!Arc::ptr_eq(&shared, &ty.package_candidates));
        }
        assert!(Arc::ptr_eq(
            &symbol.params[5].package_candidates,
            &symbol.params[6].package_candidates,
        ));
        reshare_file_type_contexts(&mut symbols, Some(&mut jvm));
        assert!(Arc::ptr_eq(
            &shared,
            &symbols[0].supertype_refs[0].package_candidates
        ));
        assert_eq!((symbols, jvm), before);
    }
}
