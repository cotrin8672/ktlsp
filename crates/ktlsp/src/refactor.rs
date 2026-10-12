//! Small, local refactor code actions that produce byte edits without mutating files.

use tree_sitter::{Node, Tree};

use crate::actions::{Action, ActionKind};
use crate::edit::TextEdit;
use crate::parser::node_text;

/// A cursor directly in a current, top-level concrete class body. Keep method bodies, interfaces,
/// nested/local declarations and parse-broken source out of this first implementation slice.
pub(crate) fn implementation_class<'t>(
    index: &dyn crate::index::InferenceIndex,
    file: &str,
    text: &str,
    tree: &'t Tree,
    offset: usize,
    current_source: impl Fn(&str) -> bool,
) -> Option<(crate::index::Entry, Node<'t>)> {
    if tree.root_node().has_error() {
        return None;
    }
    let mut body = tree
        .root_node()
        .named_descendant_for_byte_range(offset, offset)?;
    if matches!(body.kind(), "line_comment" | "block_comment") {
        body = body.parent()?;
    }
    if body.kind() != "class_body" || offset <= body.start_byte() || offset >= body.end_byte() {
        return None;
    }
    let declaration = body.parent()?;
    if declaration.kind() != "class_declaration"
        || crate::parser::class_kind(declaration) != crate::symbol::SymbolKind::Class
        || declaration.parent()? != tree.root_node()
    {
        return None;
    }
    if let Some(modifiers) = child_of_kind(declaration, "modifiers") {
        let mut walk = modifiers.walk();
        if modifiers.children(&mut walk).any(|modifier| {
            !matches!(
                node_text(modifier, text),
                "public" | "final" | "open" | "internal"
            )
        }) {
            return None;
        }
    }
    if let Some(specifiers) = child_of_kind(declaration, "delegation_specifiers") {
        let mut walk = specifiers.walk();
        if specifiers.named_children(&mut walk).any(|specifier| {
            specifier.named_child_count() != 1
                || specifier
                    .named_child(0)
                    .is_none_or(|child| child.kind() != "user_type")
        }) {
            return None;
        }
    }
    let name = crate::parser::name_field(declaration)
        .or_else(|| crate::parser::first_ident(declaration))?;
    let mut owners = index
        .lookup_type(node_text(name, text))
        .into_iter()
        .filter(|entry| {
            entry.path.as_ref() == file
                && entry.sym.kind == crate::symbol::SymbolKind::Class
                && entry.sym.start_byte == name.start_byte()
                && entry.sym.end_byte == name.end_byte()
        });
    let owner = owners.next()?.clone();
    let language = crate::language::SourceLanguage::for_key(file);
    let prefix = if language.is_java() { "java" } else { "kotlin" };
    let visibility = crate::language::NameVisibility::for_file(language, tree, text);
    if owners.next().is_some()
        || visibility.is_import_visible("", prefix)
        || index.lookup_by_name(prefix).iter().any(|entry| {
            // Kotlin extensions are not Java type names in the generated qualified type contexts.
            (!language.is_java() || entry.sym.extension_receiver.is_none())
                && (entry.path.as_ref() == file || visibility.is_symbol_visible(&entry.sym))
                && (entry.tier != crate::index::Tier::Durable && !current_source(&entry.path)
                    || prefix_may_shadow(index, &owner, entry, &current_source))
        })
    {
        return None;
    }
    Some((owner, body))
}

fn prefix_may_shadow(
    index: &dyn crate::index::InferenceIndex,
    owner: &crate::index::Entry,
    entry: &crate::index::Entry,
    current_source: &impl Fn(&str) -> bool,
) -> bool {
    use crate::symbol::SymbolKind;
    let Some(receiver) = &entry.sym.extension_receiver else {
        return true;
    };
    if receiver.has_type_constraints
        || receiver.ty.qualification.is_none() && entry.sym.type_params.contains(&receiver.ty.name)
        || receiver.ty.qualification.is_none() && receiver.ty.package_candidates.is_empty()
    {
        return true;
    }
    // Current indexed receiver facts already expand imports and retain declaration-file candidates.
    let ctx = crate::infer::FileCtx::new(entry.sym.package.clone(), Vec::new());
    let formal = crate::infer::declared_type(index, &receiver.ty, &ctx);
    // Implicit JVM roots and unresolved/aliased heads cannot prove receiver rejection.
    if matches!(
        (formal.package(), formal.name()),
        (Some("kotlin"), Some("Any")) | (Some("java.lang"), Some("Object"))
    ) {
        return true;
    }
    let Some(name) = formal.name() else {
        return true;
    };
    let mut heads = index.lookup_type(name).into_iter().filter(|candidate| {
        Some(candidate.sym.package.as_str()) == formal.package()
            && candidate.sym.container.as_deref() == formal.container()
    });
    if heads.next().is_none_or(|head| {
        !matches!(
            head.sym.kind,
            SymbolKind::Class | SymbolKind::Interface | SymbolKind::Object | SymbolKind::EnumClass
        ) || head.tier != crate::index::Tier::Durable && !current_source(&head.path)
    }) || heads.next().is_some()
    {
        return true;
    }
    let actual = crate::types::Type::class(&owner.sym.name, Some(owner.sym.package.clone()));
    let mut incomplete = false;
    let found = crate::infer::walk_supertypes(index, &actual, 32, |current| {
        incomplete |= current.name().is_none_or(|name| {
            index
                .lookup_type(name)
                .into_iter()
                .filter(|candidate| {
                    Some(candidate.sym.package.as_str()) == current.package()
                        && candidate.sym.container.as_deref() == current.container()
                })
                .count()
                != 1
        });
        if current.name() == formal.name()
            && current.package() == formal.package()
            && current.container() == formal.container()
        {
            std::ops::ControlFlow::Break(())
        } else {
            std::ops::ControlFlow::Continue(())
        }
    })
    .is_break();
    found || incomplete
}

pub(crate) fn implement_missing_methods(
    index: &dyn crate::index::InferenceIndex,
    file: &str,
    text: &str,
    owner: &crate::index::Entry,
    body: Node<'_>,
    object: &ktcore::classfile::ClassFileFacts,
    facts: &crate::resolve::CompletenessFacts,
    source_owner_known: &dyn Fn(&crate::index::Entry) -> bool,
) -> Option<Action> {
    let crate::resolve::ResolutionStatus::Found(methods) =
        ktcore::members::required_interface_methods(
            index,
            owner,
            object,
            facts,
            source_owner_known,
        )
    else {
        return None;
    };
    if methods.is_empty() {
        return None;
    }
    let java = crate::language::SourceLanguage::for_key(file).is_java();
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let line_start = text[..body.start_byte()].rfind('\n').map_or(0, |at| at + 1);
    let indentation: String = text[line_start..body.start_byte()]
        .chars()
        .take_while(|character| matches!(character, ' ' | '\t'))
        .collect();
    let indent = format!("{indentation}    ");
    let mut generated = String::new();
    for (method, ty) in methods {
        let name = &method.sym.name;
        if !(if java {
            crate::java::is_valid_identifier(name)
        } else {
            crate::rename::is_valid_identifier(name)
        }) {
            return None;
        }
        use ktcore::extensions::JvmType;
        let (java_result, kotlin_result) = match ty {
            JvmType::Int => ("int", "Int"),
            JvmType::Long => ("long", "Long"),
            JvmType::Short => ("short", "Short"),
            JvmType::Byte => ("byte", "Byte"),
            JvmType::Float => ("float", "Float"),
            JvmType::Double => ("double", "Double"),
            JvmType::Boolean => ("boolean", "Boolean"),
            JvmType::Char => ("char", "Char"),
            JvmType::Void => ("void", "Unit"),
            JvmType::String => ("java.lang.String", "String"),
            _ => return None,
        };
        if java {
            generated.push_str(&format!("{newline}{indent}@java.lang.Override{newline}{indent}public {java_result} {name}() {{{newline}{indent}    throw new java.lang.UnsupportedOperationException(\"Not implemented\");{newline}{indent}}}{newline}"));
        } else {
            generated.push_str(&format!("{newline}{indent}override fun {name}(): kotlin.{kotlin_result} {{{newline}{indent}    throw kotlin.NotImplementedError(\"Not implemented\"){newline}{indent}}}{newline}"));
        }
    }
    let insert = body.end_byte().checked_sub(1)?;
    (text.get(insert..insert + 1)? == "}").then(|| Action {
        title: "Implement missing methods".into(),
        kind: ActionKind::QuickFix,
        edits: vec![TextEdit::new(file, insert, insert, generated)],
        is_preferred: false,
    })
}

pub fn function_rewrite_actions(file: &str, text: &str, tree: &Tree, offset: usize) -> Vec<Action> {
    let Some(function) = enclosing_function(tree, offset) else {
        return Vec::new();
    };
    let Some(body) = child_of_kind(function, "function_body") else {
        return Vec::new();
    };
    let Some(child) = body.named_child(0) else {
        return Vec::new();
    };

    if child.kind() == "block" {
        block_to_expression_action(file, text, child)
            .into_iter()
            .collect()
    } else {
        expression_to_block_action(file, text, function, child)
            .into_iter()
            .collect()
    }
}

fn expression_to_block_action(
    file: &str,
    text: &str,
    function: Node<'_>,
    expression: Node<'_>,
) -> Option<Action> {
    let eq = text[function.start_byte()..expression.start_byte()].rfind('=')?;
    let start = function.start_byte() + eq;
    let expr = node_text(expression, text).trim();
    if expr.is_empty() {
        return None;
    }
    Some(Action {
        title: "Convert expression body to block body".to_string(),
        kind: ActionKind::RefactorRewrite,
        edits: vec![TextEdit::new(
            file,
            start,
            expression.end_byte(),
            format!("{{ return {expr} }}"),
        )],
        is_preferred: false,
    })
}

fn block_to_expression_action(file: &str, text: &str, block: Node<'_>) -> Option<Action> {
    let mut named = Vec::new();
    let mut cursor = block.walk();
    for child in block.named_children(&mut cursor) {
        named.push(child);
    }
    let [return_expr] = named.as_slice() else {
        return None;
    };
    if return_expr.kind() != "return_expression" {
        return None;
    }
    let expr = return_expr.named_child(0)?;
    let expr_text = node_text(expr, text).trim();
    if expr_text.is_empty() {
        return None;
    }
    Some(Action {
        title: "Convert block body to expression body".to_string(),
        kind: ActionKind::RefactorRewrite,
        edits: vec![TextEdit::new(
            file,
            block.start_byte(),
            block.end_byte(),
            format!("= {expr_text}"),
        )],
        is_preferred: false,
    })
}

fn enclosing_function(tree: &Tree, offset: usize) -> Option<Node<'_>> {
    let mut node = tree
        .root_node()
        .named_descendant_for_byte_range(offset, offset)?;
    loop {
        if node.kind() == "function_declaration" {
            return Some(node);
        }
        node = node.parent()?;
    }
}

fn child_of_kind<'t>(node: Node<'t>, kind: &str) -> Option<Node<'t>> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == kind {
            return Some(child);
        }
    }
    None
}
