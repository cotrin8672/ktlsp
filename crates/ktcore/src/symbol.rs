//! Core, LSP-independent data types: symbol kinds, indexed symbols, and definition results.

use serde::{Deserialize, Serialize};

use crate::types::TypeRef;

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
    #[serde(default)]
    pub trailing_lambda_receiver_type: Option<TypeRef>,
    /// Receiver type of a type alias whose target is a receiver function type, for example
    /// `typealias Handler = suspend RoutingContext.() -> Unit`.
    #[serde(default)]
    pub function_type_receiver: Option<TypeRef>,
    #[serde(default)]
    pub return_type: Option<TypeRef>,
    #[serde(default)]
    pub value_type: Option<TypeRef>,
    #[serde(default)]
    pub params: Vec<TypeRef>,
    #[serde(default)]
    pub type_params: Vec<String>,
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
            type_params: Vec::new(),
        }
    }
}

/// Restore declaration-context sharing lost when a cache decodes each `Arc` by value.
pub fn reshare_file_type_contexts(
    symbols: &mut [IndexedSymbol],
    jvm: Option<&mut crate::jvm::SourceFileFacts>,
) {
    use std::collections::HashSet;
    use std::sync::Arc;

    fn visit(ty: &mut TypeRef, pool: &mut HashSet<Arc<Vec<String>>>) {
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

    let mut pool = HashSet::new();
    for symbol in symbols {
        for ty in symbol
            .supertype_refs
            .iter_mut()
            .chain(symbol.trailing_lambda_receiver_type.iter_mut())
            .chain(symbol.function_type_receiver.iter_mut())
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

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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
        symbol.supertype_refs = vec![parent];
        symbol.trailing_lambda_receiver_type = Some(ty());
        symbol.function_type_receiver = Some(ty());
        symbol.return_type = Some(TypeRef {
            name: "Qualified".into(),
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
            symbol.trailing_lambda_receiver_type.as_ref().unwrap(),
            symbol.function_type_receiver.as_ref().unwrap(),
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
