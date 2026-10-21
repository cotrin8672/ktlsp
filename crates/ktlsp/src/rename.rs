//! Conservative rename support built on goto-grade reference filtering.

use crate::edit::TextEdit;
use crate::symbol::Def;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedRename {
    pub range: Def,
    pub placeholder: String,
}

pub fn is_valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_alphabetic()) {
        return false;
    }
    if !chars.all(|c| c == '_' || c.is_alphanumeric()) {
        return false;
    }
    !KOTLIN_KEYWORDS.contains(&name)
}

pub fn edits_for_refs(refs: Vec<Def>, new_name: &str) -> Vec<TextEdit> {
    refs.into_iter()
        .map(|r| TextEdit::new(r.file, r.start_byte, r.end_byte, new_name))
        .collect()
}

/// A Java accessor cursor still renames the authored Kotlin property.
pub fn property_edit_spelling(
    property: &str,
    current: &str,
    new_name: &str,
    java: bool,
) -> Option<String> {
    if !java {
        return (current == property).then(|| new_name.to_string());
    }
    if current == ktcore::jvm::property_getter_name(property)? {
        ktcore::jvm::property_getter_name(new_name)
    } else if current == ktcore::jvm::property_setter_name(property)? {
        ktcore::jvm::property_setter_name(new_name)
    } else {
        None
    }
}

/// Labels belong to the constructor callee, never to an unrelated argument expression.
pub fn constructor_argument_labels(
    tree: &tree_sitter::Tree,
    site: &Def,
    property: &str,
    text: &str,
) -> Vec<std::ops::Range<usize>> {
    let Some(mut node) = tree
        .root_node()
        .named_descendant_for_byte_range(site.start_byte, site.end_byte)
    else {
        return Vec::new();
    };
    loop {
        if matches!(node.kind(), "call_expression" | "constructor_invocation") {
            let Some(callee) = node.named_child(0) else {
                return Vec::new();
            };
            if !(callee.start_byte() <= site.start_byte && site.end_byte <= callee.end_byte()) {
                return Vec::new();
            }
            let Some(arguments) = crate::parser::child_of_kind(node, "value_arguments") else {
                return Vec::new();
            };
            return arguments
                .named_children(&mut arguments.walk())
                .filter_map(|argument| {
                    let label = argument.named_child(0)?;
                    (argument.kind() == "value_argument"
                        && argument.named_child_count() > 1
                        && label.kind() == "identifier"
                        && crate::parser::node_text(label, text) == property)
                        .then(|| label.byte_range())
                })
                .collect();
        }
        let Some(parent) = node.parent() else {
            return Vec::new();
        };
        node = parent;
    }
}

const KOTLIN_KEYWORDS: &[&str] = &[
    "as",
    "break",
    "class",
    "continue",
    "do",
    "else",
    "false",
    "for",
    "fun",
    "if",
    "in",
    "interface",
    "is",
    "null",
    "object",
    "package",
    "return",
    "super",
    "this",
    "throw",
    "true",
    "try",
    "typealias",
    "typeof",
    "val",
    "var",
    "when",
    "while",
    "import",
    "by",
    "catch",
    "constructor",
    "delegate",
    "dynamic",
    "field",
    "file",
    "finally",
    "get",
    "init",
    "param",
    "property",
    "receiver",
    "set",
    "setparam",
    "where",
];
