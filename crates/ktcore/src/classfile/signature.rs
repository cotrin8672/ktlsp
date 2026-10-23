//! JVMS 4.7.9.1 declaration signatures projected without inventing erased type arguments.

use super::ClassFileFacts;
use crate::types::{Projection, TypeRef};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureParameter {
    pub name: String,
    pub bounds: Vec<TypeRef>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_class() -> ClassFileFacts {
        crate::classfile::decode(
            "javac:genericfixture",
            include_bytes!("../../tests/fixtures/classfile/genericfixture/Box.class"),
        )
        .unwrap()
    }

    #[test]
    fn javac_generics_preserve_bounds_arrays_variance_and_method_shadowing() {
        let class = box_class();
        let signature = class_signature(&class).unwrap();
        let scope = &signature.type_parameters;
        assert_eq!(scope[0].name, "T");
        assert_eq!(scope[0].bounds[0].name, "Item");
        for field in &class.fields {
            let actual = field_signature(
                field.signature.as_deref().unwrap(),
                &field.descriptor,
                scope,
            );
            if field.name.as_ref() == "entry" {
                assert!(actual.is_none(), "unproved dollar-named nested owner");
                continue;
            }
            let ty = actual.unwrap();
            match field.name.as_ref() {
                "value" => assert_eq!(ty.name, "T"),
                "producers" => {
                    assert_eq!(ty.args[0].projection, Projection::Out);
                    assert_eq!(ty.args[0].name, "T");
                }
                "consumers" => {
                    assert_eq!(ty.args[0].projection, Projection::In);
                    assert_eq!(ty.args[0].name, "T");
                }
                "stars" => assert_eq!(ty.args[0].projection, Projection::Star),
                _ => panic!("unexpected fixture field"),
            }
        }
        for method in class
            .methods
            .iter()
            .filter(|method| method.signature.is_some())
        {
            let actual = method_signature(
                method.signature.as_deref().unwrap(),
                &method.descriptor,
                scope,
            )
            .unwrap();
            match method.name.as_ref() {
                "get" => assert_eq!(actual.returns.unwrap().name, "T"),
                "array" => assert_eq!(actual.returns.unwrap().name, "T[]"),
                "same" => assert_eq!(actual.type_parameters[0].bounds[0].name, "Item"),
                "bounded" => {
                    let bound = &actual.type_parameters[0].bounds[0];
                    assert_eq!(actual.type_parameters[0].name, "V");
                    assert_eq!(bound.name, "Comparable");
                    assert_eq!(bound.qualification.as_deref(), Some("java.lang"));
                    assert_eq!(bound.args[0].name, "V");
                    assert_eq!(actual.returns.unwrap().name, "V");
                }
                "shadow" => {
                    assert_eq!(actual.type_parameters[0].name, "T");
                    assert_eq!(actual.type_parameters[0].bounds[0].name, "Other");
                }
                _ => panic!("unexpected fixture method"),
            }
        }
        let child = crate::classfile::decode(
            "javac:genericfixture",
            include_bytes!("../../tests/fixtures/classfile/genericfixture/Child.class"),
        )
        .unwrap();
        let child = class_signature(&child).unwrap();
        assert_eq!(child.superclass.name, "Box");
        assert_eq!(child.superclass.args[0].name, "Item");
    }

    #[test]
    fn signature_projection_rejects_malformed_unknown_and_mismatched_erasure() {
        let class = box_class();
        let parameters = class_signature(&class).unwrap().type_parameters;
        for signature in [
            "",
            "TT",
            "TU;",
            "TT;;",
            "I",
            "Ljava/util/List<>;",
            "Ljava/util/List<I>;",
            "Ljava/util/List<+*>;",
            "Lpkg/Outer<TT;>.Inner<Ljava/lang/String;>;",
            "Lpkg/Outer$Inner<TT;>;",
        ] {
            assert!(
                field_signature(signature, "Lgenericfixture/Item;", &parameters).is_none(),
                "{signature}"
            );
        }
        assert!(field_signature("TT;", "Ljava/lang/Object;", &parameters).is_none());
        assert!(field_signature("[TT;", "[Lgenericfixture/Item;", &parameters).is_some());
        assert!(method_signature(
            "<T:Lgenericfixture/Other;>(TT;)TT;",
            "(Lgenericfixture/Other;)Lgenericfixture/Other;",
            &parameters
        )
        .is_some());
        assert!(method_signature(
            "<T:Lgenericfixture/Other;>(TT;)TT;",
            "(Lgenericfixture/Item;)Lgenericfixture/Item;",
            &parameters
        )
        .is_none());
        for signature in [
            "<T:TU;U:TT;>()TT;",
            "<T:Ljava/lang/Object;T:Ljava/lang/Object;>()TT;",
            "()TU;",
            "(V)V",
            "()V^I",
            "()VV",
        ] {
            assert!(
                method_signature(signature, "()Ljava/lang/Object;", &parameters).is_none(),
                "{signature}"
            );
        }
        assert!(method_signature(
            "<T::Ljava/lang/Comparable<TT;>;>(TT;)TT;",
            "(Ljava/lang/Comparable;)Ljava/lang/Comparable;",
            &[]
        )
        .is_some());
        assert!(method_signature("<E:Ljava/lang/Exception;>()V^TE;", "()V", &[]).is_some());
        for signature in [
            "<T:TU;U:TT;>Ljava/lang/Object;",
            "<T:Ljava/lang/Object;>Ljava/lang/String;",
            "Ljava/lang/Object;Ljava/lang/Runnable;",
            "Ljava/lang/Object;;",
        ] {
            let mut altered = class.clone();
            altered.signature = Some(signature.into());
            assert!(class_signature(&altered).is_none(), "{signature}");
        }
        let nested = format!("{}TT;", "[".repeat(64));
        assert!(field_signature(&nested, "Ljava/lang/Object;", &parameters).is_none());
        let huge = format!("Ljava/util/List<{}>;", "*".repeat(5000));
        assert!(field_signature(&huge, "Ljava/util/List;", &parameters).is_none());
    }

    #[test]
    fn class_bound_erasure_keeps_its_declaration_scope_under_method_shadowing() {
        let class = crate::classfile::decode(
            "javac:genericfixture",
            include_bytes!("../../tests/fixtures/classfile/genericfixture/Lexical.class"),
        )
        .unwrap();
        let class_signature = class_signature(&class).unwrap();
        let method = class
            .methods
            .iter()
            .find(|method| method.name.as_ref() == "lexical")
            .unwrap();
        assert_eq!(method.descriptor.as_ref(), "()Ljava/lang/Object;");
        let projected = method_signature(
            method.signature.as_deref().unwrap(),
            &method.descriptor,
            &class_signature.type_parameters,
        )
        .unwrap();
        assert_eq!(projected.returns.unwrap().name, "T");
        assert_eq!(projected.type_parameters[0].name, "U");
        assert_eq!(projected.type_parameters[0].bounds[0].name, "Other");
        assert!(method_signature(
            method.signature.as_deref().unwrap(),
            "()Lgenericfixture/Other;",
            &class_signature.type_parameters
        )
        .is_none());
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassSignature {
    pub type_parameters: Vec<SignatureParameter>,
    pub superclass: TypeRef,
    pub interfaces: Vec<TypeRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MethodSignature {
    pub type_parameters: Vec<SignatureParameter>,
    pub parameters: Vec<TypeRef>,
    pub returns: Option<TypeRef>,
}

/// Invalid or unsupported signatures are unknown; callers must not fall back to erasure.
pub fn class_signature(class: &ClassFileFacts) -> Option<ClassSignature> {
    class_signature_scoped(class, &[])
}

/// Enclosing scopes must come from a proved nonstatic InnerClasses chain, nearest first.
pub fn class_signature_scoped(
    class: &ClassFileFacts,
    enclosing: &[&[SignatureParameter]],
) -> Option<ClassSignature> {
    let mut parser = Parser::new(class.signature.as_deref()?)?;
    let type_parameters = parser.formals()?;
    let superclass = parser.class_type(0)?;
    let mut interfaces = Vec::new();
    while !parser.rest.is_empty() {
        interfaces.push(parser.class_type(0)?);
    }
    let layers: Vec<_> = std::iter::once(type_parameters.as_slice())
        .chain(enclosing.iter().copied())
        .collect();
    let scope = Scope { layers: &layers };
    validate_parameters(&type_parameters, enclosing)?;
    if erase(&superclass, scope, 0)? != format!("L{};", class.super_name.as_deref()?)
        || interfaces.len() != class.interfaces.len()
    {
        return None;
    }
    for (ty, internal) in interfaces.iter().zip(&class.interfaces) {
        validate_type(ty, scope)?;
        if erase(ty, scope, 0)? != format!("L{internal};") {
            return None;
        }
    }
    validate_type(&superclass, scope)?;
    Some(ClassSignature {
        type_parameters,
        superclass,
        interfaces,
    })
}

pub fn field_signature(
    signature: &str,
    descriptor: &str,
    class_parameters: &[SignatureParameter],
) -> Option<TypeRef> {
    field_signature_scoped(signature, descriptor, class_parameters, &[])
}

pub fn field_signature_scoped(
    signature: &str,
    descriptor: &str,
    class_parameters: &[SignatureParameter],
    enclosing: &[&[SignatureParameter]],
) -> Option<TypeRef> {
    let mut parser = Parser::new(signature)?;
    let ty = parser.reference(0)?;
    if !parser.rest.is_empty() {
        return None;
    }
    let layers: Vec<_> = std::iter::once(class_parameters)
        .chain(enclosing.iter().copied())
        .collect();
    let scope = Scope { layers: &layers };
    validate_type(&ty, scope)?;
    (erase(&ty, scope, 0)? == descriptor).then_some(ty)
}

pub fn method_signature(
    signature: &str,
    descriptor: &str,
    class_parameters: &[SignatureParameter],
) -> Option<MethodSignature> {
    method_signature_scoped(signature, descriptor, class_parameters, &[])
}

pub fn method_signature_scoped(
    signature: &str,
    descriptor: &str,
    class_parameters: &[SignatureParameter],
    enclosing: &[&[SignatureParameter]],
) -> Option<MethodSignature> {
    let mut parser = Parser::new(signature)?;
    let type_parameters = parser.formals()?;
    let class_layers: Vec<_> = std::iter::once(class_parameters)
        .chain(enclosing.iter().copied())
        .collect();
    validate_parameters(&type_parameters, &class_layers)?;
    // Nearest method parameters shadow class parameters, including in their own bounds.
    let layers: Vec<_> = std::iter::once(type_parameters.as_slice())
        .chain(class_layers.iter().copied())
        .collect();
    let scope = Scope { layers: &layers };
    parser.take('(')?;
    let mut parameters = Vec::new();
    while !parser.eat(')') {
        parameters.push(parser.ty(0)?);
    }
    let returns = if parser.eat('V') {
        None
    } else {
        Some(parser.ty(0)?)
    };
    let mut thrown = Vec::new();
    while parser.eat('^') {
        if !matches!(parser.rest.as_bytes().first(), Some(b'L' | b'T')) {
            return None;
        }
        thrown.push(parser.reference(0)?);
    }
    if !parser.rest.is_empty() {
        return None;
    }
    let mut erased = "(".to_string();
    for ty in &parameters {
        validate_type(ty, scope)?;
        erased.push_str(&erase(ty, scope, 0)?);
    }
    erased.push(')');
    if let Some(ty) = &returns {
        validate_type(ty, scope)?;
        erased.push_str(&erase(ty, scope, 0)?);
    } else {
        erased.push('V');
    }
    for ty in &thrown {
        validate_type(ty, scope)?;
    }
    (erased == descriptor).then_some(MethodSignature {
        type_parameters,
        parameters,
        returns,
    })
}

fn validate_parameters(
    parameters: &[SignatureParameter],
    enclosing: &[&[SignatureParameter]],
) -> Option<()> {
    let mut names = std::collections::HashSet::new();
    let layers: Vec<_> = std::iter::once(parameters)
        .chain(enclosing.iter().copied())
        .collect();
    let scope = Scope { layers: &layers };
    for parameter in parameters {
        if !names.insert(&parameter.name) {
            return None;
        }
        for bound in &parameter.bounds {
            validate_type(bound, scope)?;
        }
        erase(&TypeRef::simple(&parameter.name), scope, 0)?;
    }
    Some(())
}

#[derive(Clone, Copy)]
struct Scope<'a> {
    layers: &'a [&'a [SignatureParameter]],
}

impl<'a> Scope<'a> {
    fn parameter(self, name: &str) -> Option<(&'a SignatureParameter, Self)> {
        for (index, layer) in self.layers.iter().enumerate() {
            if let Some(parameter) = layer.iter().find(|p| p.name == name) {
                // Its bounds retain the declaration's lexical scopes under later shadowing.
                return Some((
                    parameter,
                    Self {
                        layers: &self.layers[index..],
                    },
                ));
            }
        }
        None
    }
}

fn validate_type(ty: &TypeRef, scope: Scope<'_>) -> Option<()> {
    let base = ty.name.trim_end_matches("[]");
    if ty.projection != Projection::Star
        && ty.qualification.is_none()
        && !matches!(
            base,
            "boolean" | "byte" | "char" | "short" | "int" | "long" | "float" | "double"
        )
        && scope.parameter(base).is_none()
    {
        return None;
    }
    if let Some(owner) = ty.owner.as_deref() {
        if owner.name.ends_with("[]") || owner.projection != Projection::Invariant {
            return None;
        }
        validate_type(owner, scope)?;
    }
    for argument in &ty.args {
        validate_type(argument, scope)?;
    }
    Some(())
}

fn erase(ty: &TypeRef, scope: Scope<'_>, depth: usize) -> Option<String> {
    if depth >= 64 || ty.projection != Projection::Invariant {
        return None;
    }
    let base = ty.name.trim_end_matches("[]");
    let rank = (ty.name.len() - base.len()) / 2;
    let erased = if let Some(owner) = ty.owner.as_deref() {
        let owner = erase(owner, scope, depth + 1)?;
        let internal = owner.strip_prefix('L')?.strip_suffix(';')?;
        format!("L{internal}${base};")
    } else if let Some(package) = &ty.qualification {
        format!(
            "L{}{base};",
            if package.is_empty() {
                String::new()
            } else {
                format!("{}/", package.replace('.', "/"))
            }
        )
    } else {
        match base {
            "boolean" => "Z".to_string(),
            "byte" => "B".to_string(),
            "char" => "C".to_string(),
            "short" => "S".to_string(),
            "int" => "I".to_string(),
            "long" => "J".to_string(),
            "float" => "F".to_string(),
            "double" => "D".to_string(),
            variable => {
                let (parameter, declaration_scope) = scope.parameter(variable)?;
                match parameter.bounds.first() {
                    Some(bound) => erase(bound, declaration_scope, depth + 1)?,
                    None => "Ljava/lang/Object;".to_string(),
                }
            }
        }
    };
    Some(format!("{}{erased}", "[".repeat(rank)))
}

struct Parser<'a> {
    rest: &'a str,
    nodes: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Option<Self> {
        (!input.is_empty() && input.len() <= u16::MAX as usize).then_some(Self {
            rest: input,
            nodes: 0,
        })
    }
    fn eat(&mut self, token: char) -> bool {
        if let Some(rest) = self.rest.strip_prefix(token) {
            self.rest = rest;
            true
        } else {
            false
        }
    }
    fn take(&mut self, token: char) -> Option<()> {
        self.eat(token).then_some(())
    }
    fn identifier(&mut self) -> Option<&'a str> {
        let end = self
            .rest
            .find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '$')
            .unwrap_or(self.rest.len());
        let name = &self.rest[..end];
        if name.is_empty() || name.as_bytes()[0].is_ascii_digit() {
            return None;
        }
        self.rest = &self.rest[end..];
        Some(name)
    }
    fn formals(&mut self) -> Option<Vec<SignatureParameter>> {
        let mut parameters = Vec::new();
        if !self.eat('<') {
            return Some(parameters);
        }
        loop {
            let name = self.identifier()?.to_string();
            self.take(':')?;
            let mut bounds = Vec::new();
            if matches!(self.rest.as_bytes().first(), Some(b'L' | b'T' | b'[')) {
                bounds.push(self.reference(0)?);
            }
            while self.eat(':') {
                bounds.push(self.reference(0)?);
            }
            parameters.push(SignatureParameter { name, bounds });
            if self.eat('>') {
                return Some(parameters);
            }
        }
    }
    fn ty(&mut self, depth: usize) -> Option<TypeRef> {
        if let Some(name) = match self.rest.as_bytes().first()? {
            b'Z' => Some("boolean"),
            b'B' => Some("byte"),
            b'C' => Some("char"),
            b'S' => Some("short"),
            b'I' => Some("int"),
            b'J' => Some("long"),
            b'F' => Some("float"),
            b'D' => Some("double"),
            _ => None,
        } {
            self.rest = &self.rest[1..];
            Some(TypeRef::simple(name))
        } else {
            self.reference(depth)
        }
    }
    fn reference(&mut self, depth: usize) -> Option<TypeRef> {
        self.nodes += 1;
        if depth >= 64 || self.nodes > 4096 {
            return None;
        }
        if self.eat('[') {
            let mut ty = self.ty(depth + 1)?;
            ty.name.push_str("[]");
            Some(ty)
        } else if self.eat('T') {
            let name = self.identifier()?;
            self.take(';')?;
            Some(TypeRef::simple(name))
        } else {
            self.class_type(depth)
        }
    }
    fn class_type(&mut self, depth: usize) -> Option<TypeRef> {
        self.nodes += 1;
        if depth >= 64 || self.nodes > 4096 {
            return None;
        }
        self.take('L')?;
        let mut segments = vec![self.identifier()?.to_string()];
        while self.eat('/') {
            segments.push(self.identifier()?.to_string());
        }
        let name = segments.pop()?;
        if name.contains('$') {
            return None;
        }
        let mut ty = TypeRef::simple(name);
        ty.qualification = Some(segments.join("."));
        ty.args = self.arguments(depth)?;
        let mut owner_depth = depth;
        while self.eat('.') {
            owner_depth += 1;
            self.nodes += 1;
            if owner_depth >= 64 || self.nodes > 4096 {
                return None;
            }
            let name = self.identifier()?.to_string();
            if name.contains('$') {
                return None;
            }
            let qualification = match ty.qualification.as_deref() {
                Some("") | None => ty.name.clone(),
                Some(package) => format!("{package}.{}", ty.name),
            };
            let mut inner = TypeRef::simple(name);
            inner.qualification = Some(qualification);
            inner.args = self.arguments(owner_depth)?;
            inner.owner = Some(Box::new(ty));
            ty = inner;
        }
        self.take(';')?;
        Some(ty)
    }
    fn arguments(&mut self, depth: usize) -> Option<Vec<TypeRef>> {
        let mut arguments = Vec::new();
        if self.eat('<') {
            loop {
                self.nodes += 1;
                if self.nodes > 4096 {
                    return None;
                }
                let argument = if self.eat('*') {
                    TypeRef {
                        projection: Projection::Star,
                        ..TypeRef::default()
                    }
                } else {
                    let projection = if self.eat('+') {
                        Projection::Out
                    } else if self.eat('-') {
                        Projection::In
                    } else {
                        Projection::Invariant
                    };
                    let mut argument = self.reference(depth + 1)?;
                    argument.projection = projection;
                    argument
                };
                arguments.push(argument);
                if self.eat('>') {
                    break;
                }
            }
        }
        Some(arguments)
    }
}

#[cfg(test)]
mod binary_outer_regression_tests {
    use super::*;

    #[test]
    fn binary_outer_factory_preserves_owner_and_inner_instantiations() {
        let factory = crate::classfile::decode(
            "javac:outerfixture",
            include_bytes!("../../tests/fixtures/classfile/outerfixture/Factory.class"),
        )
        .unwrap();
        let field = factory
            .fields
            .iter()
            .find(|field| field.name.as_ref() == "value")
            .unwrap();
        let projected =
            field_signature(field.signature.as_deref().unwrap(), &field.descriptor, &[])
                .expect("valid javac nonstatic owner Signature must remain useful");
        assert_eq!(projected.name, "Inner");
        assert_eq!(projected.args[0].name, "Other");
        let owner = projected
            .owner
            .as_deref()
            .expect("instantiated lexical owner");
        assert_eq!(owner.name, "Outer");
        assert_eq!(owner.args[0].name, "SpecialItem");
        let method = factory
            .methods
            .iter()
            .find(|method| method.name.as_ref() == "make")
            .unwrap();
        let projected_method = method_signature(
            method.signature.as_deref().unwrap(),
            &method.descriptor,
            &[],
        )
        .expect("the same actual owner type must survive method return projection");
        assert_eq!(projected_method.returns.as_ref(), Some(&projected));
        assert!(field_signature(
            field.signature.as_deref().unwrap(),
            "Louterfixture/Outer$Shadow;",
            &[]
        )
        .is_none());
    }
}
