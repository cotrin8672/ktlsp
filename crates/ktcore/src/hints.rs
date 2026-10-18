//! Compiler-free inlay hints. Pure core: byte positions and labels only.

use tree_sitter::{Node, Tree};

use crate::index::InferenceIndex;
use crate::infer::{self, FileCtx};
use crate::parser::{child_of_kind, first_ident, name_field, node_text};
use crate::types::Type;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InlayHintKind {
    Type,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InlayHint {
    pub position_byte: usize,
    pub label: String,
    pub kind: InlayHintKind,
}

pub fn inlay_hints(
    index: &dyn InferenceIndex,
    tree: &Tree,
    text: &str,
    start_byte: usize,
    end_byte: usize,
) -> Vec<InlayHint> {
    let ctx = FileCtx::from_tree(tree, text);
    inlay_hints_with_ctx(index, tree, text, start_byte, end_byte, &ctx)
}

pub fn inlay_hints_with_ctx(
    index: &dyn InferenceIndex,
    tree: &Tree,
    text: &str,
    start_byte: usize,
    end_byte: usize,
    ctx: &FileCtx,
) -> Vec<InlayHint> {
    let mut out = Vec::new();
    collect(
        index,
        tree.root_node(),
        text,
        ctx,
        start_byte,
        end_byte,
        &mut out,
    );
    out.sort_by(|a, b| {
        a.position_byte
            .cmp(&b.position_byte)
            .then(a.label.cmp(&b.label))
    });
    out
}

/// Collect Java syntax positions using contextual types supplied by the language adapter.
pub fn java_inlay_hints<'tree>(
    tree: &'tree Tree,
    text: &str,
    start_byte: usize,
    end_byte: usize,
    mut type_of: impl FnMut(Node<'tree>) -> Option<Type>,
) -> Vec<InlayHint> {
    let mut out = Vec::new();
    collect_java(
        tree.root_node(),
        text,
        start_byte,
        end_byte,
        &mut type_of,
        &mut out,
    );
    out.sort_by(|a, b| {
        a.position_byte
            .cmp(&b.position_byte)
            .then(a.label.cmp(&b.label))
    });
    out
}

fn collect(
    index: &dyn InferenceIndex,
    node: Node<'_>,
    text: &str,
    ctx: &FileCtx,
    start_byte: usize,
    end_byte: usize,
    out: &mut Vec<InlayHint>,
) {
    if node.end_byte() < start_byte || node.start_byte() > end_byte {
        return;
    }
    match node.kind() {
        "property_declaration" => {
            collect_property_hint(index, node, text, ctx, start_byte, end_byte, out);
        }
        "function_declaration" => {
            collect_function_return_hint(index, node, text, ctx, start_byte, end_byte, out);
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect(index, child, text, ctx, start_byte, end_byte, out);
    }
}

fn collect_java<'tree>(
    node: Node<'tree>,
    text: &str,
    start_byte: usize,
    end_byte: usize,
    type_of: &mut impl FnMut(Node<'tree>) -> Option<Type>,
    out: &mut Vec<InlayHint>,
) {
    if node.end_byte() < start_byte || node.start_byte() > end_byte {
        return;
    }
    if matches!(node.kind(), "variable_declarator" | "resource") {
        collect_java_var_hint(node, text, start_byte, end_byte, type_of, out);
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_java(child, text, start_byte, end_byte, type_of, out);
    }
}

fn collect_java_var_hint<'tree>(
    variable: Node<'tree>,
    text: &str,
    start_byte: usize,
    end_byte: usize,
    type_of: &mut impl FnMut(Node<'tree>) -> Option<Type>,
    out: &mut Vec<InlayHint>,
) {
    let declaration = match variable.kind() {
        "resource" => variable,
        "variable_declarator" => match variable.parent() {
            Some(parent) if parent.kind() == "local_variable_declaration" => parent,
            _ => return,
        },
        _ => return,
    };
    let Some(type_node) = declaration.child_by_field_name("type") else {
        return;
    };
    if node_text(type_node, text) != "var" {
        return;
    }
    let Some(name) = name_field(variable) else {
        return;
    };
    let position = name.end_byte();
    if !position_in_range(position, start_byte, end_byte) {
        return;
    }
    let Some(label) = type_of(name).and_then(|ty| type_label(ty, TypeSyntax::Java)) else {
        return;
    };
    out.push(InlayHint {
        position_byte: position,
        label,
        kind: InlayHintKind::Type,
    });
}

fn collect_property_hint(
    index: &dyn InferenceIndex,
    property: Node<'_>,
    text: &str,
    ctx: &FileCtx,
    start_byte: usize,
    end_byte: usize,
    out: &mut Vec<InlayHint>,
) {
    if !is_local_property(property) {
        return;
    }
    let Some(var_decl) = child_of_kind(property, "variable_declaration") else {
        return;
    };
    if crate::indexer::value_type_of(var_decl, text).is_some() {
        return;
    }
    let Some(name) = first_ident(var_decl) else {
        return;
    };
    let position = name.end_byte();
    if !position_in_range(position, start_byte, end_byte) {
        return;
    }
    let Some(expr) = initializer_expression(property, var_decl) else {
        return;
    };
    let Some(label) = type_label(infer::infer(index, expr, text, ctx), TypeSyntax::Kotlin) else {
        return;
    };
    out.push(InlayHint {
        position_byte: position,
        label,
        kind: InlayHintKind::Type,
    });
}

fn collect_function_return_hint(
    index: &dyn InferenceIndex,
    function: Node<'_>,
    text: &str,
    ctx: &FileCtx,
    start_byte: usize,
    end_byte: usize,
    out: &mut Vec<InlayHint>,
) {
    if has_explicit_return_type(function) {
        return;
    }
    let Some(params) = child_of_kind(function, "function_value_parameters") else {
        return;
    };
    let position = params.end_byte();
    if !position_in_range(position, start_byte, end_byte) {
        return;
    }
    let Some(body) = child_of_kind(function, "function_body") else {
        return;
    };
    let Some(expr) = body.named_child(0) else {
        return;
    };
    if expr.kind() == "block" {
        return;
    }
    let Some(label) = type_label(infer::infer(index, expr, text, ctx), TypeSyntax::Kotlin) else {
        return;
    };
    out.push(InlayHint {
        position_byte: position,
        label,
        kind: InlayHintKind::Type,
    });
}

fn initializer_expression<'t>(property: Node<'t>, var_decl: Node<'t>) -> Option<Node<'t>> {
    let mut cursor = property.walk();
    let found = property.named_children(&mut cursor).find(|child| {
        child.start_byte() >= var_decl.end_byte() && child.kind() != "property_delegate"
    });
    found
}

fn has_explicit_return_type(function: Node<'_>) -> bool {
    let mut after_params = false;
    let mut cursor = function.walk();
    for child in function.named_children(&mut cursor) {
        if child.kind() == "function_value_parameters" {
            after_params = true;
            continue;
        }
        if after_params {
            return matches!(child.kind(), "user_type" | "nullable_type");
        }
    }
    false
}

fn is_local_property(property: Node<'_>) -> bool {
    let mut current = property.parent();
    while let Some(parent) = current {
        match parent.kind() {
            "block" | "lambda_literal" => return true,
            "class_body" | "enum_class_body" | "source_file" => return false,
            _ => current = parent.parent(),
        }
    }
    false
}

#[derive(Clone, Copy)]
enum TypeSyntax {
    Kotlin,
    Java,
}

fn type_label(ty: Type, syntax: TypeSyntax) -> Option<String> {
    format_type(&ty, syntax).map(|name| format!(": {name}"))
}

fn format_type(ty: &Type, syntax: TypeSyntax) -> Option<String> {
    match ty {
        Type::Parameter { name, nullable, .. } => {
            Some(format!("{name}{}", if *nullable { "?" } else { "" }))
        }
        Type::Projected { projection, bound } => {
            let prefix = match projection {
                crate::types::Projection::Out => match syntax {
                    TypeSyntax::Kotlin => "out ",
                    TypeSyntax::Java => "? extends ",
                },
                crate::types::Projection::In => match syntax {
                    TypeSyntax::Kotlin => "in ",
                    TypeSyntax::Java => "? super ",
                },
                _ => return None,
            };
            format_type(bound, syntax).map(|bound| format!("{prefix}{bound}"))
        }
        Type::Star => Some(
            match syntax {
                TypeSyntax::Kotlin => "*",
                TypeSyntax::Java => "?",
            }
            .to_string(),
        ),
        Type::Unknown | Type::Array { .. } => None,
        Type::Class {
            name,
            nullable,
            args,
            ..
        } => {
            if name.is_empty() {
                return None;
            }
            let mut out = match ty.owner() {
                Some(owner) => format!("{}.{}", format_type(owner, syntax)?, name),
                None => name.clone(),
            };
            if !args.is_empty() {
                let formatted_args = args
                    .iter()
                    .map(|arg| format_type(arg, syntax))
                    .collect::<Option<Vec<_>>>()?;
                out.push('<');
                out.push_str(&formatted_args.join(", "));
                out.push('>');
            }
            if *nullable {
                out.push('?');
            }
            Some(out)
        }
    }
}

fn position_in_range(position: usize, start_byte: usize, end_byte: usize) -> bool {
    position >= start_byte && position <= end_byte
}
