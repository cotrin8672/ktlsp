//! Signature-help extraction over indexed callable declarations.

use tree_sitter::{Node, Tree};

use crate::index::Entry;
use crate::parser::node_text;
use crate::symbol::SymbolKind;
use crate::types::TypeRef;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureHelp {
    pub signatures: Vec<Signature>,
    pub active_parameter: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    pub label: String,
    pub parameters: Vec<String>,
}

pub fn call_at<'tree>(
    tree: &'tree Tree,
    text: &str,
    offset: usize,
) -> Option<(Node<'tree>, String, u32)> {
    let mut node = tree
        .root_node()
        .named_descendant_for_byte_range(offset, offset)?;
    loop {
        if node.kind() == "call_expression" {
            let callee = node.named_child(0)?;
            if offset < callee.end_byte() {
                return None;
            }
            let name = callable_name(callee, text)?;
            let active = active_parameter(text, callee.end_byte(), offset);
            return Some((callee, name, active));
        }
        node = node.parent()?;
    }
}

pub fn signatures_for_entries(entries: Vec<Entry>, active_parameter: u32) -> Option<SignatureHelp> {
    signatures_for_entries_with_extra(entries, Vec::new(), active_parameter)
}

pub fn signatures_for_entries_with_extra(
    entries: Vec<Entry>,
    extra: Vec<Signature>,
    active_parameter: u32,
) -> Option<SignatureHelp> {
    let mut signatures = entries
        .into_iter()
        .filter(|entry| {
            entry.sym.kind == SymbolKind::Function
                || entry.sym.kind.is_type_like()
                || (entry.sym.kind.is_value_like()
                    && entry
                        .sym
                        .value_type
                        .as_ref()
                        .and_then(crate::members::function_type_signature)
                        .is_some())
        })
        .map(|entry| signature_for_entry(&entry))
        .collect::<Vec<_>>();
    signatures.extend(extra);
    signatures.sort_by(|a, b| a.label.cmp(&b.label));
    signatures.dedup_by(|a, b| a.label == b.label);
    if signatures.is_empty() {
        return None;
    }
    Some(SignatureHelp {
        signatures,
        active_parameter: Some(active_parameter),
    })
}

pub fn signature_for_entry(entry: &Entry) -> Signature {
    let (declared_params, return_type) = entry
        .sym
        .value_type
        .as_ref()
        .filter(|_| entry.sym.kind.is_value_like())
        .and_then(crate::members::function_type_signature)
        .map_or(
            (entry.sym.params.as_slice(), entry.sym.return_type.as_ref()),
            |(params, result)| (params, Some(result)),
        );
    signature(&entry.sym.name, declared_params, return_type)
}

pub fn signature_for_callable(candidate: crate::jvm::CallableCandidate<'_>) -> Signature {
    signature(
        candidate.name(),
        candidate.params(),
        candidate.return_type(),
    )
}

pub fn signature_for_callable_in_context(
    index: &dyn crate::index::InferenceIndex,
    candidate: crate::jvm::CallableCandidate<'_>,
) -> Option<Signature> {
    let (params, returns) = crate::jvm::resolved_signature_types(index, candidate)?;
    Some(signature(candidate.name(), &params, returns.as_ref()))
}

fn signature(name: &str, declared_params: &[TypeRef], return_type: Option<&TypeRef>) -> Signature {
    let params = declared_params
        .iter()
        .enumerate()
        .map(|(idx, ty)| format!("p{}: {}", idx + 1, type_ref_label(ty)))
        .collect::<Vec<_>>();
    let mut label = String::new();
    label.push_str(name);
    label.push('(');
    label.push_str(&params.join(", "));
    label.push(')');
    if let Some(ret) = return_type {
        if !ret.name.is_empty() {
            label.push_str(": ");
            label.push_str(&type_ref_label(ret));
        }
    }
    Signature {
        label,
        parameters: params,
    }
}

fn callable_name(node: Node<'_>, text: &str) -> Option<String> {
    match node.kind() {
        "identifier" => Some(node_text(node, text).to_string()),
        "navigation_expression" => {
            let selector = node.named_child(1)?;
            (selector.kind() == "identifier").then(|| node_text(selector, text).to_string())
        }
        _ => None,
    }
}

fn active_parameter(text: &str, callee_end: usize, offset: usize) -> u32 {
    let mut depth = 0_i32;
    let mut active = 0_u32;
    let start = floor_char_boundary(text, callee_end);
    let end = floor_char_boundary(text, offset);
    if start >= end {
        return active;
    }
    for ch in text[start..end].chars() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 1 => active += 1,
            _ => {}
        }
    }
    active
}

fn floor_char_boundary(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn type_ref_label(ty: &TypeRef) -> String {
    match ty.projection {
        crate::types::Projection::Star => return "*".to_string(),
        crate::types::Projection::Out | crate::types::Projection::In => {
            let prefix = if ty.projection == crate::types::Projection::Out {
                "out "
            } else {
                "in "
            };
            let mut bound = ty.clone();
            bound.projection = crate::types::Projection::Invariant;
            return format!("{prefix}{}", type_ref_label(&bound));
        }
        crate::types::Projection::Invariant => {}
    }
    if ty.name.is_empty() {
        return "_".to_string();
    }
    let mut out = ty.owner.as_deref().map_or_else(
        || ty.name.clone(),
        |owner| format!("{}.{}", type_ref_label(owner), ty.name),
    );
    if !ty.args.is_empty() {
        out.push('<');
        out.push_str(
            &ty.args
                .iter()
                .map(type_ref_label)
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push('>');
    }
    if ty.nullable {
        out.push('?');
    }
    out
}
