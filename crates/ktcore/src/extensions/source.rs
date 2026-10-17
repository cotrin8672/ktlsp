//! Authored declaration facts from the accepted source tree and shared index. These facts are a
//! sidecar: they do not change cached symbols or pretend source properties are generated methods.

use std::ops::Range;

use tree_sitter::{Node, Tree};

use super::{
    DeclaredMemberFacts, Dispatch, Fact, IncompleteReason, JvmType, MemberSignature, Visibility,
};
use crate::index::InferenceIndex;
use crate::indexer::TypeScope;
use crate::infer::{self, FileCtx};
use crate::knowledge::Knowledge;
use crate::parser::{child_of_kind, node_text};
use crate::symbol::{Def, IndexedSymbol, SymbolKind};
use crate::types::{Type, TypeRef};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceDeclaration {
    pub definition: Def,
    pub owner: Option<Def>,
    pub annotations: Vec<SourceAnnotation>,
    pub member: Option<DeclaredMemberFacts>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceAnnotation {
    pub ty: TypeRef,
    pub use_site_target: Option<String>,
    pub arguments: Vec<AnnotationArgument>,
    pub range: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnnotationArgument {
    pub name: Option<String>,
    pub range: Range<usize>,
    pub value: Fact<AnnotationValue>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnnotationValue {
    Class(TypeRef),
    String {
        value: String,
        content_range: Range<usize>,
    },
    Boolean(bool),
    Array(Vec<AnnotationArgument>),
}

/// The grammar is identified by its root kind; the host supplies the same declaration visibility
/// context used by its language indexer. Unknown expressions/types retain their source ranges.
pub fn extract(
    index: &dyn InferenceIndex,
    file: &str,
    tree: &Tree,
    text: &str,
    scope: &TypeScope,
) -> Vec<SourceDeclaration> {
    extract_declarations(index, file, tree, text, scope, true)
}

/// Declaration annotations may introduce members even with no framework provider registered.
/// This uses the same accepted identities and annotation parser without normalizing signatures.
pub fn annotated_type_owners(
    index: &dyn InferenceIndex,
    file: &str,
    tree: &Tree,
    text: &str,
    scope: &TypeScope,
) -> Vec<Def> {
    let entries = index.entries_for_file(file);
    let mut owners: Vec<_> = extract_declarations(index, file, tree, text, scope, false)
        .into_iter()
        .filter(|declaration| !declaration.annotations.is_empty())
        .filter_map(|declaration| {
            if entries.iter().any(|entry| {
                definition(file, &entry.sym) == declaration.definition
                    && entry.sym.kind.is_type_like()
            }) {
                Some(declaration.definition)
            } else {
                declaration.owner
            }
        })
        .collect();
    owners.sort();
    owners.dedup();
    owners
}

fn extract_declarations(
    index: &dyn InferenceIndex,
    file: &str,
    tree: &Tree,
    text: &str,
    scope: &TypeScope,
    include_members: bool,
) -> Vec<SourceDeclaration> {
    let java = match tree.root_node().kind() {
        "program" => true,
        "source_file" => false,
        _ => return Vec::new(),
    };
    let entries = index.entries_for_file(file);
    let mut result = Vec::new();
    for entry in &entries {
        let symbol = &entry.sym;
        let Some(node) = declaration_node(tree, symbol, java) else {
            continue;
        };
        let owner = entries
            .iter()
            .find(|candidate| {
                candidate.sym.kind.is_type_like()
                    && symbol.container.as_deref() == Some(container_name(&candidate.sym).as_str())
                    && candidate.sym.package == symbol.package
            })
            .map(|entry| definition(file, &entry.sym));
        let annotations = annotations(node, text, scope, java);
        let member = (include_members
            && matches!(symbol.kind, SymbolKind::Function | SymbolKind::Property))
        .then(|| member_facts(index, file, text, symbol, node, scope, java, &annotations));
        result.push(SourceDeclaration {
            definition: definition(file, symbol),
            owner,
            annotations,
            member,
        });
    }
    result
}

fn definition(file: &str, symbol: &IndexedSymbol) -> Def {
    Def {
        file: file.into(),
        start_byte: symbol.start_byte,
        end_byte: symbol.end_byte,
    }
}

fn container_name(symbol: &IndexedSymbol) -> String {
    let name = symbol.name.trim_matches('`');
    symbol
        .container
        .as_ref()
        .map_or_else(|| name.into(), |parent| format!("{parent}.{name}"))
}

fn declaration_node<'a>(tree: &'a Tree, symbol: &IndexedSymbol, java: bool) -> Option<Node<'a>> {
    let mut node = tree
        .root_node()
        .descendant_for_byte_range(symbol.start_byte, symbol.end_byte)?;
    loop {
        let is_declaration = if java {
            match symbol.kind {
                SymbolKind::Function => matches!(
                    node.kind(),
                    "method_declaration" | "constructor_declaration"
                ),
                SymbolKind::Property => node.kind() == "field_declaration",
                _ if symbol.kind.is_type_like() => matches!(
                    node.kind(),
                    "class_declaration"
                        | "interface_declaration"
                        | "enum_declaration"
                        | "record_declaration"
                        | "annotation_type_declaration"
                ),
                _ => return None,
            }
        } else {
            match symbol.kind {
                SymbolKind::Function => node.kind() == "function_declaration",
                SymbolKind::Property => node.kind() == "property_declaration",
                _ if symbol.kind.is_type_like() => matches!(
                    node.kind(),
                    "class_declaration" | "object_declaration" | "type_alias"
                ),
                _ => return None,
            }
        };
        if is_declaration {
            return Some(node);
        }
        node = node.parent()?;
    }
}

fn annotations(node: Node<'_>, text: &str, scope: &TypeScope, java: bool) -> Vec<SourceAnnotation> {
    let Some(modifiers) = child_of_kind(node, "modifiers") else {
        return Vec::new();
    };
    let mut cursor = modifiers.walk();
    modifiers
        .named_children(&mut cursor)
        .filter_map(|node| {
            if !matches!(node.kind(), "annotation" | "marker_annotation") {
                return None;
            }
            let invocation = child_of_kind(node, "constructor_invocation").unwrap_or(node);
            let name = if java {
                node.child_by_field_name("name")
            } else {
                child_of_kind(invocation, "user_type")
            }?;
            let name_text = node_text(name, text);
            // Type arguments and malformed annotation names are not type identities.
            if !name_text.split('.').all(|part| {
                !part.is_empty()
                    && part
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
            }) {
                return None;
            }
            let arguments = node
                .child_by_field_name("arguments")
                .or_else(|| child_of_kind(invocation, "value_arguments"));
            let arguments = arguments
                .map(|args| {
                    let mut cursor = args.walk();
                    args.named_children(&mut cursor)
                        .map(|arg| annotation_argument(arg, text, scope, java))
                        .collect()
                })
                .unwrap_or_default();
            Some(SourceAnnotation {
                ty: scope.type_ref(name_text, false, Vec::new()),
                use_site_target: child_of_kind(node, "use_site_target")
                    .map(|n| node_text(n, text).trim_end_matches(':').into()),
                arguments,
                range: node.byte_range(),
            })
        })
        .collect()
}

fn annotation_argument(
    node: Node<'_>,
    text: &str,
    scope: &TypeScope,
    java: bool,
) -> AnnotationArgument {
    let (name, value) = if node.kind() == "element_value_pair" {
        (
            node.child_by_field_name("key")
                .map(|n| node_text(n, text).into()),
            node.child_by_field_name("value"),
        )
    } else if node.kind() == "value_argument" {
        let mut cursor = node.walk();
        let children: Vec<_> = node.named_children(&mut cursor).collect();
        if children.len() == 2 && node_text(node, text).contains('=') {
            (
                Some(node_text(children[0], text).into()),
                children.last().copied(),
            )
        } else {
            (None, children.first().copied())
        }
    } else {
        (None, Some(node))
    };
    let Some(value) = value else {
        return AnnotationArgument {
            name,
            range: node.byte_range(),
            value: unsupported(),
        };
    };
    AnnotationArgument {
        name,
        range: value.byte_range(),
        value: annotation_value(value, text, scope, java),
    }
}

fn annotation_value(
    node: Node<'_>,
    text: &str,
    scope: &TypeScope,
    java: bool,
) -> Fact<AnnotationValue> {
    if node.has_error() || node.is_missing() {
        return unsupported();
    }
    let spelling = node_text(node, text);
    if spelling == "true" || spelling == "false" {
        return Knowledge::Found(AnnotationValue::Boolean(spelling == "true"));
    }
    if node.kind() == "string_literal"
        && spelling.len() >= 2
        && !node.has_error()
        && spelling.starts_with('"')
        && spelling.ends_with('"')
        && !spelling.starts_with("\"\"\"")
    {
        let value = &spelling[1..spelling.len() - 1];
        // Escapes/interpolation need a source-to-value map before their subranges are editable.
        let interpolation = !java && {
            let mut cursor = node.walk();
            let interpolated = node
                .named_children(&mut cursor)
                .any(|child| !matches!(child.kind(), "string_content"));
            interpolated
        };
        if value.contains('\\') || interpolation {
            return unsupported();
        }
        return Knowledge::Found(AnnotationValue::String {
            value: value.into(),
            content_range: node.start_byte() + 1..node.end_byte() - 1,
        });
    }
    if matches!(
        node.kind(),
        "element_value_array_initializer" | "collection_literal"
    ) {
        let mut cursor = node.walk();
        return Knowledge::Found(AnnotationValue::Array(
            node.named_children(&mut cursor)
                .map(|child| annotation_argument(child, text, scope, java))
                .collect(),
        ));
    }
    let class_name = if java && node.kind() == "class_literal" {
        spelling.strip_suffix(".class")
    } else if !java && node.kind() == "navigation_expression" {
        spelling.strip_suffix("::class")
    } else {
        None
    };
    if let Some(name) = class_name.filter(|name| {
        name.split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_alphanumeric() || c == '_'))
    }) {
        return Knowledge::Found(AnnotationValue::Class(scope.type_ref(
            name,
            false,
            Vec::new(),
        )));
    }
    unsupported()
}

fn member_facts(
    index: &dyn InferenceIndex,
    file: &str,
    text: &str,
    symbol: &IndexedSymbol,
    node: Node<'_>,
    scope: &TypeScope,
    java: bool,
    annotations: &[SourceAnnotation],
) -> DeclaredMemberFacts {
    let modifiers = child_of_kind(node, "modifiers");
    let mut words = Vec::new();
    if let Some(modifiers) = modifiers.filter(|_| symbol.kind != SymbolKind::Function) {
        let mut cursor = modifiers.walk();
        for modifier in modifiers.children(&mut cursor) {
            if !matches!(modifier.kind(), "annotation" | "marker_annotation") {
                words.push(node_text(modifier, text));
            }
        }
    }
    let interface = node
        .parent()
        .is_some_and(|parent| parent.kind() == "interface_body");
    let method = symbol.kind == SymbolKind::Function;
    let flags = symbol.source_member;
    let visibility = if method {
        flags.visibility().unwrap_or(Visibility::Public)
    } else if words.contains(&"private") {
        Visibility::Private
    } else if words.contains(&"protected") {
        Visibility::Protected
    } else if words.contains(&"public") || !java || interface {
        Visibility::Public
    } else {
        Visibility::Package
    };
    let unsupported_jvm = annotations.iter().any(|annotation| {
        matches!(
            annotation.ty.qualification.as_deref(),
            None | Some("kotlin.jvm")
        ) && matches!(
            annotation.ty.name.as_str(),
            "JvmName" | "JvmStatic" | "JvmField" | "JvmSynthetic" | "JvmOverloads"
        )
    }) || flags.jvm_restricted()
        || words.iter().any(|word| {
            matches!(
                *word,
                "suspend" | "external" | "expect" | "inline" | "internal"
            )
        });
    let plain_owner = java || symbol.container.is_some() && !unsupported_kotlin_owner(node, text);
    // A Kotlin public property denotes accessors as well as a backing field. Only a private
    // authored property has a field identity in this slice; generated accessor identity is separate.
    let field_known = java
        || symbol.kind != SymbolKind::Property
        || visibility == Visibility::Private
            && !words.contains(&"abstract")
            && !["getter", "setter", "property_delegate"]
                .iter()
                .any(|kind| child_of_kind(node, kind).is_some())
            && (words.contains(&"lateinit")
                || node
                    .children(&mut node.walk())
                    .any(|child| child.kind() == "="));
    let mut cursor = node.walk();
    let header_clean = node
        .children(&mut cursor)
        .filter(|child| {
            !matches!(
                child.kind(),
                "block" | "function_body" | "class_body" | "interface_body"
            )
        })
        .all(|child| !child.has_error() && !child.is_missing());
    let known = header_clean
        && !unsupported_jvm
        && plain_owner
        && field_known
        && (!method
            || matches!(
                flags.implementation(),
                Some(
                    crate::symbol::MemberImplementation::Concrete
                        | crate::symbol::MemberImplementation::Abstract
                )
            ) && flags.dispatch().is_some()
                && flags.visibility().is_some())
        && symbol.ext_receiver.is_none();
    let signature = if known {
        signature(index, file, symbol, scope, java, node, text)
    } else {
        unsupported()
    };
    DeclaredMemberFacts {
        definition: definition(file, symbol),
        signature,
        dispatch: if known {
            Knowledge::Found(if method {
                flags.dispatch().unwrap()
            } else if words.contains(&"static")
                || java && interface && symbol.kind == SymbolKind::Property
            {
                Dispatch::Static
            } else {
                Dispatch::Instance
            })
        } else {
            unsupported()
        },
        visibility: if known {
            Knowledge::Found(visibility)
        } else {
            unsupported()
        },
        synthetic: if known {
            Knowledge::Found(false)
        } else {
            unsupported()
        },
    }
}

fn unsupported_kotlin_owner(mut node: Node<'_>, text: &str) -> bool {
    while let Some(parent) = node.parent() {
        if matches!(parent.kind(), "object_declaration" | "companion_object") {
            return true;
        }
        if parent.kind() == "class_declaration"
            && child_of_kind(parent, "modifiers").is_some_and(|modifiers| {
                modifiers
                    .named_children(&mut modifiers.walk())
                    .any(|modifier| node_text(modifier, text) == "value")
            })
        {
            return true;
        }
        node = parent;
    }
    false
}

fn signature(
    index: &dyn InferenceIndex,
    file: &str,
    symbol: &IndexedSymbol,
    scope: &TypeScope,
    java: bool,
    node: Node<'_>,
    text: &str,
) -> Fact<MemberSignature> {
    // Post-name dimensions remain outside the source-ABI slice, including fields/parameters
    // whose writer still stores their base type. Keep their descriptors unavailable.
    if java && has_post_name_dimensions(node) {
        return unsupported();
    }
    let ctx = FileCtx::new(symbol.package.clone(), Vec::new());
    let type_parameters = member_type_parameters(index, file, symbol);
    let mut unsupported_types = Vec::new();
    let mut convert = |ty: &TypeRef| {
        let value = jvm_type(index, ty, &ctx, scope, java, &type_parameters);
        if value.is_none() {
            unsupported_types.push(IncompleteReason::UnsupportedType(ty.name.clone()));
        }
        value
    };
    let result = if symbol.kind == SymbolKind::Property {
        symbol
            .value_type
            .as_ref()
            .and_then(&mut convert)
            .and_then(|ty| MemberSignature::field(&ty).ok())
    } else {
        let params: Option<Vec<_>> = symbol.params.iter().map(&mut convert).collect();
        let returns = match &symbol.return_type {
            Some(ty) => convert(ty),
            None if !java
                && child_of_kind(node, "function_body")
                    .is_some_and(|body| node_text(body, text).trim_start().starts_with('{')) =>
            {
                Some(JvmType::Void)
            }
            None => None,
        };
        // Missing/unsupported parameter facts may not shorten the JVM descriptor.
        match (params, returns) {
            (Some(params), Some(returns))
                if symbol.arity == Some(params.len().min(u8::MAX as usize) as u8)
                    && !symbol.has_vararg =>
            {
                MemberSignature::method(&params, &returns).ok()
            }
            _ => None,
        }
    };
    result.map(Knowledge::Found).unwrap_or_else(|| {
        if unsupported_types.is_empty() {
            unsupported()
        } else {
            Knowledge::Unknown(unsupported_types)
        }
    })
}

pub(super) fn member_type_parameters(
    index: &dyn InferenceIndex,
    file: &str,
    symbol: &IndexedSymbol,
) -> Vec<String> {
    let mut parameters = symbol.type_params.clone();
    if let Some(container) = &symbol.container {
        for entry in index.entries_for_file(file) {
            let owner = container_name(&entry.sym);
            if entry.sym.kind.is_type_like()
                && entry.sym.package == symbol.package
                && (container == &owner || container.starts_with(&format!("{owner}.")))
            {
                parameters.extend(entry.sym.type_params.iter().cloned());
            }
        }
    }
    parameters
}

#[cfg(test)]
thread_local! {
    static REFERENCE_PARSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(super) fn reference_parse_count() -> usize {
    REFERENCE_PARSES.with(std::cell::Cell::get)
}

/// Eligibility prepared with a verified source tree off the editor lock. The text and file stay
/// attached so the host cannot accidentally apply its ranges to a different accepted input.
#[derive(Clone)]
pub struct PreparedSourceReferences {
    pub(super) file: String,
    pub(super) text: std::sync::Arc<str>,
    pub(super) ranges: std::collections::BTreeSet<(usize, usize)>,
}

pub fn prepare_source_references(
    index: &dyn InferenceIndex,
    file: &str,
    text: std::sync::Arc<str>,
    tree: &Tree,
) -> PreparedSourceReferences {
    let ranges = index
        .entries_for_file(file)
        .into_iter()
        .filter(|entry| {
            matches!(
                entry.sym.kind,
                SymbolKind::Class
                    | SymbolKind::Interface
                    | SymbolKind::EnumClass
                    | SymbolKind::Object
            ) && ordinary_reference_type(entry, &text, tree)
        })
        .map(|entry| (entry.sym.start_byte, entry.sym.end_byte))
        .collect();
    PreparedSourceReferences {
        file: file.into(),
        text,
        ranges,
    }
}

pub(super) fn ordinary_reference_types(
    index: &dyn InferenceIndex,
    file: &str,
    text: &str,
) -> std::collections::BTreeSet<(usize, usize)> {
    let entries: Vec<_> = index
        .entries_for_file(file)
        .into_iter()
        .filter(|entry| {
            matches!(
                entry.sym.kind,
                SymbolKind::Class
                    | SymbolKind::Interface
                    | SymbolKind::EnumClass
                    | SymbolKind::Object
            ) && entry.sym.type_params.is_empty()
        })
        .collect();
    if entries.is_empty() {
        return std::collections::BTreeSet::new();
    }
    #[cfg(test)]
    REFERENCE_PARSES.with(|count| count.set(count.get() + 1));
    let tree = if file.ends_with(".java") {
        let mut parser = tree_sitter::Parser::new();
        if parser
            .set_language(&tree_sitter_java::LANGUAGE.into())
            .is_err()
        {
            return std::collections::BTreeSet::new();
        }
        let Some(tree) = parser.parse(text, None) else {
            return std::collections::BTreeSet::new();
        };
        tree
    } else {
        crate::parser::KotlinParser::new().parse(text)
    };
    entries
        .into_iter()
        .filter(|entry| ordinary_reference_type(entry, text, &tree))
        .map(|entry| (entry.sym.start_byte, entry.sym.end_byte))
        .collect()
}

fn ordinary_reference_type(entry: &crate::index::Entry, text: &str, tree: &Tree) -> bool {
    if entry
        .sym
        .name
        .trim_matches('`')
        .contains(['.', '/', ';', '[', ']', '<', '>'])
        || !entry.sym.type_params.is_empty()
    {
        return false;
    }
    let java = entry.path.ends_with(".java");
    let Some(node) = declaration_node(tree, &entry.sym, java) else {
        return false;
    };
    if !matches!(
        node.kind(),
        "class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "record_declaration"
            | "object_declaration"
    ) {
        return false;
    }
    if child_of_kind(node, "modifiers").is_some_and(|modifiers| {
        node_text(modifiers, text)
            .split_whitespace()
            .any(|word| matches!(word, "value" | "inline" | "expect"))
    }) {
        return false;
    }
    let mut ancestor = node.parent();
    while let Some(parent) = ancestor {
        if child_of_kind(parent, "type_parameters").is_some() {
            return false;
        }
        if matches!(
            parent.kind(),
            "block"
                | "function_body"
                | "function_declaration"
                | "method_declaration"
                | "lambda_literal"
        ) {
            return false;
        }
        ancestor = parent.parent();
    }
    let clean = node
        .children(&mut node.walk())
        .filter(|child| {
            !matches!(
                child.kind(),
                "class_body" | "interface_body" | "enum_body" | "enum_class_body"
            )
        })
        .all(|child| !child.has_error() && !child.is_missing());
    clean
}

pub(crate) fn has_post_name_dimensions(node: Node<'_>) -> bool {
    if child_of_kind(node, "dimensions").is_some() {
        return true;
    }
    let mut cursor = node.walk();
    let found = node
        .named_children(&mut cursor)
        .filter(|child| {
            matches!(
                child.kind(),
                "formal_parameters" | "formal_parameter" | "variable_declarator"
            )
        })
        .any(has_post_name_dimensions);
    found
}

fn jvm_type(
    index: &dyn InferenceIndex,
    ty: &TypeRef,
    ctx: &FileCtx,
    scope: &TypeScope,
    java: bool,
    type_parameters: &[String],
) -> Option<JvmType> {
    if ty.nullable || ty.qualification.is_none() && type_parameters.contains(&ty.name) {
        return None;
    }
    if java && ty.name.ends_with("[]") {
        let mut element = ty.clone();
        element.name = ty.name.strip_suffix("[]")?.to_string();
        return jvm_type(index, &element, ctx, scope, java, type_parameters)
            .map(|ty| JvmType::Array(Box::new(ty)));
    }
    source_scalar_jvm_type(index, ty, ctx, java, type_parameters)
}

/// Shared source ABI identity for the ordinary scalar signature slice. Java primitive tokens
/// and Kotlin builtins are distinct spellings; unresolved lookalikes must not be interchanged.
pub(crate) fn source_scalar_jvm_type(
    index: &dyn InferenceIndex,
    ty: &TypeRef,
    ctx: &FileCtx,
    java: bool,
    type_parameters: &[String],
) -> Option<JvmType> {
    if ty.nullable || ty.qualification.is_none() && type_parameters.contains(&ty.name) {
        return None;
    }
    if !ty.args.is_empty() {
        return None;
    }
    if ty.name == "String" && ty.qualification.as_deref() == Some("java.lang") {
        return Some(JvmType::String);
    }
    if java && ty.qualification.is_none() {
        let primitive = match ty.name.as_str() {
            "boolean" => Some(JvmType::Boolean),
            "byte" => Some(JvmType::Byte),
            "char" => Some(JvmType::Char),
            "short" => Some(JvmType::Short),
            "int" => Some(JvmType::Int),
            "long" => Some(JvmType::Long),
            "float" => Some(JvmType::Float),
            "double" => Some(JvmType::Double),
            "void" => Some(JvmType::Void),
            _ => None,
        };
        if primitive.is_some() {
            return primitive;
        }
    }
    let resolved = infer::declared_type(index, ty, ctx);
    let Type::Class {
        name,
        package: Some(package),
        container: None,
        ..
    } = resolved
    else {
        return None;
    };
    if name == "String" && matches!(package.as_str(), "kotlin" | "java.lang") {
        return Some(JvmType::String);
    }
    if java || package != "kotlin" {
        return None;
    }
    Some(match name.as_str() {
        "Boolean" => JvmType::Boolean,
        "Byte" => JvmType::Byte,
        "Char" => JvmType::Char,
        "Short" => JvmType::Short,
        "Int" => JvmType::Int,
        "Long" => JvmType::Long,
        "Float" => JvmType::Float,
        "Double" => JvmType::Double,
        "Unit" => JvmType::Void,
        _ => return None,
    })
}

fn unsupported<T>() -> Fact<T> {
    Knowledge::Unknown(vec![IncompleteReason::UnsupportedSource])
}
