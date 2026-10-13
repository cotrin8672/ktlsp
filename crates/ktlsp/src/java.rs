//! Java source support via tree-sitter-java.
//!
//! Supports both library `.java` files as goto targets and project `.java` files as editable
//! documents. Node kinds verified via `examples/dump_java.rs`:
//! - `class_declaration` / `interface_declaration` / `enum_declaration` / `record_declaration` /
//!   `annotation_type_declaration` expose a `name:` field and a `body:` child
//! - `method_declaration` / `constructor_declaration` expose `name:`
//! - `field_declaration` -> `variable_declarator name:`
//! - `enum_constant` exposes `name:`
//! - `package_declaration` -> `scoped_identifier` (its text is the dotted package)

use std::collections::{BTreeMap, HashSet};
use std::ops::ControlFlow;

use tree_sitter::{Node, Parser, Tree};

use crate::complete::{self, ScopeCompletion, ShapedCompletions};
use crate::hierarchy::{self, HierarchyItem, IncomingCall, OutgoingCall};
use crate::imports::{self, ImportStyle};
use crate::index::{Entry, InferenceIndex, Tier, Usage};
use crate::indexer::TypeScope;
use crate::infer::{self, FileCtx};
use crate::language;
use crate::parser::{child_of_kind, name_field, node_text};
use crate::semantic_query::{
    self, CompletionIncompletenessReason, CompletionQuery, CompletionStatus,
};
use crate::symbol::{Def, IndexedSymbol, SymbolKind};
use crate::types::{Type, TypeParameterBounds, TypeRef};

/// A reusable Java parser.
pub struct JavaParser {
    inner: Parser,
}

impl JavaParser {
    pub fn new() -> Self {
        let mut inner = Parser::new();
        let lang: tree_sitter::Language = tree_sitter_java::LANGUAGE.into();
        inner
            .set_language(&lang)
            .expect("failed to load tree-sitter-java grammar");
        JavaParser { inner }
    }

    pub fn parse(&mut self, text: &str) -> Tree {
        self.inner
            .parse(text, None)
            .expect("java parse unexpectedly returned None")
    }

    /// Incrementally reparse `text`, reusing `old_tree` (which must already have had the matching
    /// `InputEdit` applied via `Tree::edit`). Only the changed region is re-parsed.
    pub fn reparse(&mut self, text: &str, old_tree: &Tree) -> Tree {
        self.inner
            .parse(text, Some(old_tree))
            .expect("java reparse unexpectedly returned None")
    }
}

impl Default for JavaParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract top-level & member declarations (skips method/constructor bodies, so locals don't leak).
pub fn extract_symbols(tree: &Tree, src: &str) -> Vec<IndexedSymbol> {
    let package = package_of(tree, src);
    let scope = type_scope(tree, src);
    // Same sizing heuristic as `extract_usages`, scaled for declaration density: dense files grow
    // once instead of walking the doubling sequence that dominates cold-index profiles.
    let mut out = Vec::with_capacity(src.len() / 128);
    let mut stack = vec![(tree.root_node(), None::<String>)];
    while let Some((node, container)) = stack.pop() {
        walk_declaration_container(
            node,
            src,
            &package,
            container.as_deref(),
            &scope,
            &mut out,
            &mut stack,
        );
    }
    out
}

/// Collect every `identifier` / `type_identifier` occurrence as a usage site, for the
/// reverse-reference index. Declarations are included so find-references can return the decl too.
pub fn extract_usages(tree: &Tree, src: &str) -> Vec<Usage> {
    // Match the Kotlin indexer's conservative real-project sizing heuristic. Dense files grow
    // once; typical files avoid the long sequence of small Vec reallocations.
    let mut out = Vec::with_capacity(src.len() / 48);
    let mut cursor = tree.walk();
    let mut interner = ktcore::indexer::UsageInterner::default();
    collect_usages(&mut cursor, src, &mut out, &mut interner);
    out
}

fn collect_usages<'a>(
    cursor: &mut tree_sitter::TreeCursor,
    src: &'a str,
    out: &mut Vec<Usage>,
    interner: &mut ktcore::indexer::UsageInterner<'a>,
) {
    let mut stack = vec![cursor.node()];
    while let Some(node) = stack.pop() {
        if is_identifier(node) {
            out.push(Usage {
                name: interner.intern(node_text(node, src)),
                start_byte: node.start_byte(),
                end_byte: node.end_byte(),
            });
        }
        let mut child_cursor = node.walk();
        // Java identifiers and type identifiers are named nodes, so anonymous punctuation and
        // keyword leaves cannot contribute a usage.
        for child in node.named_children(&mut child_cursor) {
            stack.push(child);
        }
    }
}

/// Whether `node` is an identifier-like node in Java (`identifier`, `type_identifier`, or the
/// reserved-identifier alias that wraps them).
fn is_identifier(node: Node) -> bool {
    matches!(
        node.kind(),
        "identifier" | "type_identifier" | "_reserved_identifier"
    )
}

/// The `identifier`/`type_identifier` node at a byte offset, if the cursor sits on one.
///
/// Probes `[off, off]` then `[off-1, off]` so a cursor at the *end* of an identifier still
/// resolves.
pub fn identifier_at(tree: &Tree, offset: usize) -> Option<Node<'_>> {
    let root = tree.root_node();
    let mut node = root.descendant_for_byte_range(offset, offset)?;
    if is_identifier(node) {
        return Some(node);
    }
    // Cursor right after an identifier (e.g. before a `(`) lands on the following node.
    node = root.descendant_for_byte_range(offset.saturating_sub(1), offset)?;
    if is_identifier(node) {
        return Some(node);
    }
    None
}

/// The file's package (dotted), or `""` if none.
pub fn package_of(tree: &Tree, src: &str) -> String {
    let root = tree.root_node();
    if let Some(decl) = child_of_kind(root, "package_declaration") {
        let mut cursor = decl.walk();
        for child in decl.named_children(&mut cursor) {
            if matches!(child.kind(), "scoped_identifier" | "identifier") {
                return node_text(child, src).to_string();
            }
        }
    }
    String::new()
}

fn push(
    out: &mut Vec<IndexedSymbol>,
    name_node: Node,
    src: &str,
    kind: SymbolKind,
    package: &str,
    container: Option<&str>,
) {
    out.push(indexed_symbol(name_node, src, kind, package, container));
}

fn indexed_symbol(
    name_node: Node,
    src: &str,
    kind: SymbolKind,
    package: &str,
    container: Option<&str>,
) -> IndexedSymbol {
    IndexedSymbol::new(
        node_text(name_node, src),
        kind,
        package,
        container.map(str::to_string),
        name_node.start_byte(),
        name_node.end_byte(),
    )
}

fn type_decl<'tree>(
    node: Node<'tree>,
    kind: SymbolKind,
    src: &str,
    package: &str,
    container: Option<&str>,
    scope: &TypeScope,
    out: &mut Vec<IndexedSymbol>,
    stack: &mut Vec<(Node<'tree>, Option<String>)>,
) {
    let Some(name) = name_field(node) else {
        return;
    };
    let mut sym = indexed_symbol(name, src, kind, package, container);
    let declaration_scope = java_declaration_scope(node, src, scope);
    let scope = declaration_scope.as_ref().unwrap_or(scope);
    sym.supertype_refs = supertype_refs(node, src, scope);
    sym.supertypes = sym
        .supertype_refs
        .iter()
        .map(|ty| ty.name.clone())
        .collect();
    sym.type_params = type_parameter_names(node, src);
    sym.type_parameter_facts = java_type_parameter_facts(node);
    out.push(sym);
    let cname = match container {
        Some(parent) => format!("{parent}.{}", node_text(name, src)),
        None => node_text(name, src).to_string(),
    };
    if node.kind() == "record_declaration" {
        record_component_accessors(node, src, package, &cname, scope, out);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(
            child.kind(),
            "class_body" | "interface_body" | "enum_body" | "annotation_type_body"
        ) {
            stack.push((child, Some(cname.clone())));
        }
    }
}

/// Java records expose a zero-argument accessor for every component. Tree-sitter represents the
/// components as constructor-style parameters, so synthesize their otherwise implicit methods.
fn record_component_accessors(
    node: Node<'_>,
    src: &str,
    package: &str,
    container: &str,
    scope: &TypeScope,
    out: &mut Vec<IndexedSymbol>,
) {
    let Some(params) = node.child_by_field_name("parameters") else {
        return;
    };
    let mut cursor = params.walk();
    for param in params.named_children(&mut cursor) {
        if param.kind() != "formal_parameter" {
            continue;
        }
        let Some(name) = name_field(param) else {
            continue;
        };
        let mut accessor =
            indexed_symbol(name, src, SymbolKind::Function, package, Some(container));
        accessor.arity = Some(0);
        accessor.min_arity = Some(0);
        accessor.return_type = param
            .child_by_field_name("type")
            .and_then(|ty| type_ref_from_node(ty, src, scope));
        out.push(accessor);
    }
}

fn supertype_refs(node: Node<'_>, src: &str, scope: &TypeScope) -> Vec<TypeRef> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(
            child.kind(),
            "superclass" | "super_interfaces" | "extends_interfaces"
        ) {
            collect_supertype_refs(child, src, scope, &mut out);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup();
    out
}

fn collect_supertype_refs(node: Node<'_>, src: &str, scope: &TypeScope, out: &mut Vec<TypeRef>) {
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        if matches!(
            node.kind(),
            "type_identifier" | "scoped_type_identifier" | "generic_type"
        ) {
            if let Some(ty) = type_ref_from_node(node, src, scope) {
                out.push(ty);
            }
            continue;
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            stack.push(child);
        }
    }
}

fn walk_declaration_container<'tree>(
    node: Node<'tree>,
    src: &str,
    package: &str,
    container: Option<&str>,
    scope: &TypeScope,
    out: &mut Vec<IndexedSymbol>,
    stack: &mut Vec<(Node<'tree>, Option<String>)>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "class_declaration" | "record_declaration" => type_decl(
                child,
                SymbolKind::Class,
                src,
                package,
                container,
                scope,
                out,
                stack,
            ),
            "interface_declaration" | "annotation_type_declaration" => type_decl(
                child,
                SymbolKind::Interface,
                src,
                package,
                container,
                scope,
                out,
                stack,
            ),
            "enum_declaration" => type_decl(
                child,
                SymbolKind::EnumClass,
                src,
                package,
                container,
                scope,
                out,
                stack,
            ),
            "method_declaration" | "constructor_declaration" => {
                if let Some(name) = name_field(child) {
                    let mut sym =
                        indexed_symbol(name, src, SymbolKind::Function, package, container);
                    sym.source_member = crate::indexer::source_member_flags(child, src, true);
                    let declaration_scope = java_declaration_scope(child, src, scope);
                    let scope = declaration_scope.as_ref().unwrap_or(scope);
                    sym.params = parameter_types(child, src, scope);
                    sym.type_params = type_parameter_names(child, src);
                    sym.type_parameter_facts = java_type_parameter_facts(child);
                    sym.arity = Some(sym.params.len().min(u8::MAX as usize) as u8);
                    sym.min_arity = sym.arity;
                    sym.has_vararg = has_vararg_parameter(child);
                    if child.kind() == "method_declaration" {
                        sym.return_type = child.child_by_field_name("type").and_then(|ty| {
                            with_array_dimensions(
                                type_ref_from_node(ty, src, scope)?,
                                child.child_by_field_name("dimensions"),
                            )
                        });
                    } else if let Some(container) = container {
                        sym.return_type = Some(TypeRef::simple(container));
                    }
                    out.push(sym);
                }
                // Do NOT recurse into the body (only locals live there).
            }
            "field_declaration" => {
                let flags = crate::indexer::source_member_flags(child, src, true);
                let value_type = child
                    .child_by_field_name("type")
                    .and_then(|ty| type_ref_from_node(ty, src, scope));
                let mut c2 = child.walk();
                for vd in child.named_children(&mut c2) {
                    if vd.kind() == "variable_declarator" {
                        if let Some(name) = name_field(vd) {
                            let mut sym =
                                indexed_symbol(name, src, SymbolKind::Property, package, container);
                            sym.source_member = flags;
                            sym.value_type = value_type.clone();
                            out.push(sym);
                        }
                    }
                }
            }
            "enum_constant" => {
                if let Some(name) = name_field(child) {
                    push(out, name, src, SymbolKind::EnumEntry, package, container);
                }
            }
            // Recurse only through declaration-bearing wrappers. Avoid walking arbitrary
            // expressions/annotations from generated dependency sources; those can be extremely
            // deep and do not contribute project/library symbols.
            "enum_body_declarations" | "block" | "declaration" => {
                stack.push((child, container.map(str::to_string)));
            }
            _ => {}
        }
    }
}

fn parameter_types(node: Node<'_>, src: &str, scope: &TypeScope) -> Vec<TypeRef> {
    let Some(params) = node.child_by_field_name("parameters") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = params.walk();
    for child in params.named_children(&mut cursor) {
        if matches!(child.kind(), "formal_parameter" | "spread_parameter") {
            out.push(
                child
                    .child_by_field_name("type")
                    .and_then(|ty| type_ref_from_node(ty, src, scope))
                    .unwrap_or_default(),
            );
        }
    }
    out
}

fn has_vararg_parameter(node: Node<'_>) -> bool {
    let Some(params) = node.child_by_field_name("parameters") else {
        return false;
    };
    let mut cursor = params.walk();
    let has_vararg = params
        .named_children(&mut cursor)
        .any(|child| child.kind() == "spread_parameter");
    has_vararg
}

fn type_ref_from_node(node: Node<'_>, src: &str, scope: &TypeScope) -> Option<TypeRef> {
    let text = node_text(node, src).trim();
    if text.is_empty() {
        return None;
    }
    match node.kind() {
        "generic_type" => {
            let head = node.named_child(0)?;
            let mut args = Vec::new();
            if let Some(arguments) = child_of_kind(node, "type_arguments") {
                let mut cursor = arguments.walk();
                for argument in arguments.named_children(&mut cursor) {
                    // Unknown projections keep their position instead of shifting later bindings.
                    args.push(type_ref_from_node(argument, src, scope).unwrap_or_default());
                }
            }
            Some(scope.type_ref(&type_path(head, src)?, false, args))
        }
        "type_identifier" | "scoped_type_identifier" => {
            Some(scope.type_ref(&type_path(node, src)?, false, Vec::new()))
        }
        "array_type" => {
            if node.has_error() {
                return None;
            }
            with_array_dimensions(
                type_ref_from_node(node.child_by_field_name("element")?, src, scope)?,
                Some(node.child_by_field_name("dimensions")?),
            )
        }
        "wildcard" => {
            use crate::types::Projection;
            let mut projection = Projection::Star;
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                match child.kind() {
                    "extends" => projection = Projection::Out,
                    "super" => projection = Projection::In,
                    _ => {}
                }
            }
            let mut cursor = node.walk();
            let mut bound = node
                .named_children(&mut cursor)
                .filter(|child| {
                    matches!(
                        child.kind(),
                        "type_identifier"
                            | "scoped_type_identifier"
                            | "generic_type"
                            | "annotated_type"
                    )
                })
                .find_map(|child| type_ref_from_node(child, src, scope))
                .unwrap_or_default();
            if node.has_error() {
                bound = TypeRef::default();
            }
            bound.projection = projection;
            Some(bound)
        }
        "annotated_type" => {
            let mut cursor = node.walk();
            let ty = node
                .named_children(&mut cursor)
                .filter(|child| child.kind() != "annotation" && child.kind() != "marker_annotation")
                .find_map(|child| type_ref_from_node(child, src, scope));
            ty
        }
        _ => Some(TypeRef::simple(text.replace("...", "[]"))),
    }
}

// Array shape stays in the existing TypeRef spelling. Resolving its rank-one component uses
// the original declaration's imports, generic arguments, and lexical substitutions.
fn array_dimensions_rank(dimensions: Node<'_>) -> Option<usize> {
    if dimensions.has_error() || !matches!(dimensions.kind(), "dimensions" | "dimensions_expr") {
        return None;
    }
    let mut cursor = dimensions.walk();
    let rank = dimensions
        .children(&mut cursor)
        .filter(|child| child.kind() == "[")
        .count();
    (rank > 0).then_some(rank)
}

fn with_array_dimensions(mut ty: TypeRef, dimensions: Option<Node<'_>>) -> Option<TypeRef> {
    if let Some(dimensions) = dimensions {
        for _ in 0..array_dimensions_rank(dimensions)? {
            ty.name.push_str("[]");
        }
    }
    Some(ty)
}

/// A type's qualified identity without generic arguments. Generic outer owners remain in the
/// path (`Outer<T>.Inner<U>` -> `Outer.Inner`); argument identifiers are never owner segments.
fn type_path(node: Node<'_>, src: &str) -> Option<String> {
    match node.kind() {
        "type_identifier" | "identifier" => Some(node_text(node, src).to_string()),
        "generic_type" => type_path(node.named_child(0)?, src),
        "scoped_type_identifier" => Some(format!(
            "{}.{}",
            type_path(node.named_child(0)?, src)?,
            type_path(
                node.named_child(node.named_child_count().checked_sub(1)?)?,
                src
            )?
        )),
        _ => None,
    }
}

fn java_declaration_scope(node: Node<'_>, src: &str, scope: &TypeScope) -> Option<TypeScope> {
    let mut names = Vec::new();
    let mut declaration = Some(node);
    while let Some(current) = declaration {
        names.extend(type_parameter_names(current, src));
        declaration = current.parent();
    }
    scope.without_shadowed_imports(&names)
}

fn java_type_parameter_facts(node: Node<'_>) -> crate::symbol::TypeParameterFacts {
    let mut header_cursor = node.walk();
    if node.named_children(&mut header_cursor).any(|child| {
        !matches!(
            child.kind(),
            "block" | "class_body" | "interface_body" | "enum_body" | "annotation_type_body"
        ) && child.has_error()
    }) {
        return Default::default();
    }
    let Some(parameters) = node.child_by_field_name("type_parameters") else {
        return crate::symbol::TypeParameterFacts::new(true, true);
    };
    if parameters.has_error() {
        return Default::default();
    }
    let mut cursor = parameters.walk();
    let unbounded = parameters
        .named_children(&mut cursor)
        .all(|parameter| child_of_kind(parameter, "type_bound").is_none());
    // Java constructors are invariant even when a parameter has an explicit bound.
    crate::symbol::TypeParameterFacts::new(unbounded, true)
}

fn type_parameter_names(node: Node<'_>, src: &str) -> Vec<String> {
    let Some(parameters) = node.child_by_field_name("type_parameters") else {
        return Vec::new();
    };
    let mut cursor = parameters.walk();
    parameters
        .named_children(&mut cursor)
        .filter_map(|parameter| {
            let mut cursor = parameter.walk();
            let name = parameter
                .named_children(&mut cursor)
                .find(|child| child.kind() == "type_identifier")
                .map(|name| node_text(name, src).to_string());
            name
        })
        .collect()
}

fn type_parameters_in_scope(node: Node<'_>, src: &str, tree: &Tree) -> Vec<TypeParameterBounds> {
    let file_scope = type_scope(tree, src);
    let mut out = Vec::new();
    let mut current = Some(node);
    let mut accessible = true;
    while let Some(decl) = current {
        if let Some(parameters) = decl.child_by_field_name("type_parameters") {
            let scoped = java_declaration_scope(decl, src, &file_scope);
            let scope = scoped.as_ref().unwrap_or(&file_scope);
            let mut cursor = decl.walk();
            let valid_header = accessible
                && !parameters.has_error()
                && !decl.named_children(&mut cursor).any(|child| {
                    !matches!(child.kind(), "block" | "class_body" | "interface_body")
                        && child.has_error()
                });
            let mut own = Vec::new();
            let mut cursor = parameters.walk();
            for parameter in parameters.named_children(&mut cursor) {
                let mut cursor = parameter.walk();
                let Some(name) = parameter
                    .named_children(&mut cursor)
                    .find(|child| child.kind() == "type_identifier")
                else {
                    continue;
                };
                let mut upper_bounds = Vec::new();
                let mut complete = valid_header && !parameter.has_error();
                if let Some(bounds) = child_of_kind(parameter, "type_bound") {
                    let mut cursor = bounds.walk();
                    for bound in bounds.named_children(&mut cursor) {
                        match type_ref_from_node(bound, src, scope) {
                            Some(bound) => upper_bounds.push(bound),
                            None => complete = false,
                        }
                    }
                    complete &= !upper_bounds.is_empty();
                }
                own.push(TypeParameterBounds {
                    name: node_text(name, src).to_string(),
                    start_byte: name.start_byte(),
                    end_byte: name.end_byte(),
                    scope_start: decl.start_byte(),
                    scope_end: decl.end_byte(),
                    upper_bounds,
                    complete,
                });
            }
            let duplicate = own.iter().enumerate().any(|(i, parameter)| {
                own[..i]
                    .iter()
                    .any(|earlier| earlier.name == parameter.name)
            });
            for parameter in &mut own {
                parameter.complete &= !duplicate;
            }
            out.extend(own);
        }
        let is_static = child_of_kind(decl, "modifiers").is_some_and(|modifiers| {
            let mut cursor = modifiers.walk();
            let found = modifiers
                .children(&mut cursor)
                .any(|modifier| modifier.kind() == "static");
            found
        });
        accessible &= !is_static
            && !matches!(
                decl.kind(),
                "interface_declaration"
                    | "annotation_type_declaration"
                    | "enum_declaration"
                    | "record_declaration"
                    | "static_initializer"
            );
        current = decl.parent();
    }
    out
}

fn inference_imports(tree: &Tree, src: &str) -> Vec<crate::parser::Import> {
    imports_of(tree, src)
        .into_iter()
        .map(|import| crate::parser::Import {
            path: match import.name {
                Some(name) if !import.package.is_empty() => format!("{}.{name}", import.package),
                Some(name) => name,
                None => import.package,
            },
            alias: None,
            wildcard: import.wildcard,
        })
        .collect()
}

pub(crate) fn type_scope(tree: &Tree, src: &str) -> TypeScope {
    TypeScope::with_default_imports(
        &package_of(tree, src),
        inference_imports(tree, src),
        &["java.lang"],
    )
}

/// A parsed Java import statement. Static imports are ignored for now.
#[derive(Clone, Debug)]
pub(crate) struct Import {
    /// Package path for wildcard imports, or declaring package for explicit imports.
    package: String,
    /// Simple imported name for explicit imports, or `None` for wildcard imports.
    name: Option<String>,
    wildcard: bool,
}

struct DiagnosticImport {
    name: String,
    start_byte: usize,
    end_byte: usize,
}

struct JavaDiagnosticContext {
    package: String,
    imports: Vec<Import>,
    visibility: language::NameVisibility,
    static_imports: StaticImports,
    diagnostic_imports: Vec<DiagnosticImport>,
    diagnostic_import_names: HashSet<String>,
    names_used_outside_imports: HashSet<String>,
}

impl JavaDiagnosticContext {
    fn new(tree: &Tree, src: &str) -> Self {
        let root = tree.root_node();
        let diagnostic_imports = diagnostic_imports(tree, src);
        let diagnostic_import_names = diagnostic_imports
            .iter()
            .map(|import| import.name.clone())
            .collect();
        let package = package_of(tree, src);
        let imports = imports_of(tree, src);
        let visibility = language::NameVisibility::for_java_imports(&package, &imports);
        let mut ctx = Self {
            package,
            imports,
            visibility,
            static_imports: static_imports_of(tree, src),
            diagnostic_imports,
            diagnostic_import_names,
            names_used_outside_imports: HashSet::new(),
        };
        collect_java_diagnostic_context(root, src, DiagnosticWalkState::default(), &mut ctx);
        ctx
    }

    fn name_used_outside_imports(&self, name: &str) -> bool {
        self.names_used_outside_imports.contains(name)
    }
}

#[derive(Clone, Copy, Default)]
struct DiagnosticWalkState {
    in_import: bool,
    in_package_declaration: bool,
    in_package_name: bool,
}

fn collect_java_diagnostic_context(
    node: Node<'_>,
    src: &str,
    state: DiagnosticWalkState,
    ctx: &mut JavaDiagnosticContext,
) {
    let kind = node.kind();
    let state = DiagnosticWalkState {
        in_import: state.in_import || kind == "import_declaration",
        in_package_declaration: state.in_package_declaration || kind == "package_declaration",
        in_package_name: state.in_package_name
            || matches!(kind, "scoped_identifier" | "identifier")
                && node
                    .parent()
                    .is_some_and(|parent| parent.kind() == "package_declaration"),
    };

    if is_identifier(node) {
        let name = node_text(node, src);
        if !state.in_import
            && (!state.in_package_declaration || !state.in_package_name)
            && ctx.diagnostic_import_names.contains(name)
        {
            ctx.names_used_outside_imports.insert(name.to_string());
        }
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_java_diagnostic_context(child, src, state, ctx);
    }
}

impl Import {
    pub(crate) fn package(&self) -> String {
        self.package.clone()
    }

    fn simple_name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub(crate) fn local_name(&self) -> Option<&str> {
        if self.wildcard {
            None
        } else {
            self.simple_name()
        }
    }

    pub(crate) fn is_wildcard(&self) -> bool {
        self.wildcard
    }
}

pub fn unused_import_actions(
    file: &str,
    text: &str,
    tree: &Tree,
    diagnostics: &[crate::diagnostics::Diagnostic],
    range_start: usize,
    range_end: usize,
) -> Vec<crate::actions::Action> {
    crate::actions::unused_import_actions_with_style(
        ImportStyle::Java,
        file,
        text,
        tree,
        diagnostics,
        range_start,
        range_end,
    )
}

pub fn organize_imports_action(
    file: &str,
    text: &str,
    tree: &Tree,
) -> Option<crate::actions::Action> {
    crate::actions::organize_imports_action_with_style(ImportStyle::Java, file, text, tree)
}

pub fn add_import_action(
    file: &str,
    text: &str,
    tree: &Tree,
    name: &str,
    fqn: &str,
) -> Option<crate::actions::Action> {
    crate::actions::add_import_action_with_style(ImportStyle::Java, file, text, tree, name, fqn)
}

pub fn diagnostics(
    index: &dyn InferenceIndex,
    file: &str,
    tree: &Tree,
    src: &str,
    facts: &crate::resolve::CompletenessFacts,
) -> Vec<crate::diagnostics::Diagnostic> {
    diagnostics_with_options(index, file, tree, src, facts, true)
}

pub fn diagnostics_with_options(
    index: &dyn InferenceIndex,
    file: &str,
    tree: &Tree,
    src: &str,
    facts: &crate::resolve::CompletenessFacts,
    include_call_shape: bool,
) -> Vec<crate::diagnostics::Diagnostic> {
    let mut out = Vec::new();
    let ctx = JavaDiagnosticContext::new(tree, src);
    let inference_ctx = FileCtx::new(ctx.package.clone(), inference_imports(tree, src));
    for import in &ctx.diagnostic_imports {
        if !ctx.name_used_outside_imports(&import.name) {
            out.push(crate::diagnostics::unused_import_diagnostic(
                import.start_byte,
                import.end_byte,
                &import.name,
            ));
        }
    }
    if facts.project_scan_complete && durable_indexes_complete(facts) && index.scope_is_complete() {
        collect_unresolved_imports(index, file, tree, src, facts, &inference_ctx, &mut out);
    }
    if facts.project_scan_complete && index.scope_is_complete() {
        collect_unresolved_references(
            index,
            file,
            src,
            tree,
            tree.root_node(),
            &ctx,
            &inference_ctx,
            facts,
            &mut out,
        );
    }
    if facts.project_scan_complete
        && include_call_shape
        && index.scope_is_complete()
        && (facts.compiler_producers_complete || durable_indexes_complete(facts))
    {
        collect_call_shape_mismatches(
            index,
            file,
            src,
            tree,
            tree.root_node(),
            &ctx.package,
            &ctx.imports,
            facts,
            &mut out,
        );
    }
    out
}

fn collect_unresolved_imports(
    index: &dyn InferenceIndex,
    file: &str,
    tree: &Tree,
    src: &str,
    facts: &crate::resolve::CompletenessFacts,
    inference_ctx: &FileCtx,
    out: &mut Vec<crate::diagnostics::Diagnostic>,
) {
    let root = tree.root_node();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() != "import_declaration" {
            continue;
        }
        if child
            .children(&mut child.walk())
            .any(|c| c.kind() == "static")
        {
            continue;
        }
        let Some(import) = parse_import(child, src) else {
            continue;
        };
        if import.wildcard {
            continue;
        }
        let Some(name) = import.name.as_deref() else {
            continue;
        };
        if import_resolves_to_type(index, &import.package, name) {
            continue;
        }
        let target = crate::parser::Import {
            path: if import.package.is_empty() {
                name.into()
            } else {
                format!("{}.{name}", import.package)
            },
            alias: None,
            wildcard: false,
        };
        if !crate::resolve::import_target_status(index, file, &target, facts, inference_ctx)
            .is_definitely_absent()
        {
            continue;
        }
        out.push(crate::diagnostics::Diagnostic {
            start_byte: child.start_byte(),
            end_byte: child.end_byte(),
            severity: crate::diagnostics::Severity::Error,
            code: Some(crate::diagnostics::DiagnosticCode::UnresolvedReference),
            message: format!("Unresolved import: {}.{name}", import.package),
        });
    }
}

fn import_resolves_to_type(index: &dyn InferenceIndex, package: &str, name: &str) -> bool {
    if index.lookup_by_name(name).iter().any(|entry| {
        entry.sym.name == name && entry.sym.package == package && entry.sym.kind.is_type_like()
    }) {
        return true;
    }

    let Some((outer_package, container)) = package.rsplit_once('.') else {
        return false;
    };
    index.lookup_by_name(name).iter().any(|entry| {
        entry.sym.name == name
            && entry.sym.container.as_deref() == Some(container)
            && (entry.sym.package == outer_package
                || outer_package
                    .strip_prefix(&format!("{}.", entry.sym.package))
                    .is_some())
            && entry.sym.kind.is_type_like()
    })
}

#[allow(clippy::too_many_arguments)]
fn collect_unresolved_references(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    node: Node<'_>,
    ctx: &JavaDiagnosticContext,
    inference_ctx: &FileCtx,
    facts: &crate::resolve::CompletenessFacts,
    out: &mut Vec<crate::diagnostics::Diagnostic>,
) {
    if node.kind() == "method_invocation" {
        if let Some(name_node) = node.child_by_field_name("name") {
            let name = node_text(name_node, src);
            let absent = if let Some(object) = node.child_by_field_name("object") {
                !java_synthetic_member_resolves(name, name_node, src)
                    && receiver_type(index, file, src, tree, object, 0).is_some_and(|receiver| {
                        crate::resolve::member_name_status_for_receiver(
                            index,
                            file,
                            &receiver,
                            name,
                            facts,
                            inference_ctx,
                        )
                        .is_definitely_absent()
                    })
            } else {
                !is_java_builtin_type(name)
                    && facts.compiler_surface_status(None).is_found()
                    && !ctx.static_imports.may_resolve(name)
                    && !java_synthetic_member_resolves(name, name_node, src)
                    && missing_java_reference_is_project_local_proof(name_node, &ctx.static_imports)
                    && !java_reference_resolves_with_ctx(index, file, src, tree, name_node, ctx)
                    && enclosing_type_name(name_node, src).is_some_and(|owner| {
                        let entries: Vec<_> = index
                            .lookup_type(&owner)
                            .into_iter()
                            .filter(|entry| entry.path.as_ref() == file)
                            .collect();
                        !entries.is_empty()
                            && entries.iter().all(|entry| {
                                crate::resolve::declaration_surface_status(index, entry, facts)
                                    .is_found()
                            })
                    })
            };
            if !name.is_empty() && absent {
                out.push(crate::diagnostics::Diagnostic {
                    start_byte: name_node.start_byte(),
                    end_byte: name_node.end_byte(),
                    severity: crate::diagnostics::Severity::Error,
                    code: Some(crate::diagnostics::DiagnosticCode::UnresolvedReference),
                    message: format!("Unresolved reference: {name}"),
                });
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_unresolved_references(
            index,
            file,
            src,
            tree,
            child,
            ctx,
            inference_ctx,
            facts,
            out,
        );
    }
}

fn durable_indexes_complete(facts: &crate::resolve::CompletenessFacts) -> bool {
    facts.library_index_complete && facts.jdk_index_complete
}

fn java_synthetic_member_resolves(name: &str, node: Node<'_>, src: &str) -> bool {
    match name {
        "getClass" => is_method_call_name(node),
        "values" | "valueOf" => {
            is_method_call_name(node) && enclosing_type_kind(node) == Some("enum_declaration")
        }
        "name" | "ordinal" => is_method_call_name(node) && receiver_is_enum_expression(node, src),
        _ => false,
    }
}

fn is_method_call_name(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    parent.kind() == "method_invocation" && parent.child_by_field_name("name") == Some(node)
}

fn enclosing_type_kind(node: Node<'_>) -> Option<&'static str> {
    let mut current = node.parent();
    while let Some(parent) = current {
        match parent.kind() {
            "class_declaration" => return Some("class_declaration"),
            "record_declaration" => return Some("record_declaration"),
            "interface_declaration" => return Some("interface_declaration"),
            "enum_declaration" => return Some("enum_declaration"),
            "annotation_type_declaration" => return Some("annotation_type_declaration"),
            _ => current = parent.parent(),
        }
    }
    None
}

fn receiver_is_enum_expression(node: Node<'_>, src: &str) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    let Some(object) = parent.child_by_field_name("object") else {
        return enclosing_type_kind(node) == Some("enum_declaration");
    };
    node_text(object, src)
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase())
}

fn missing_java_reference_is_project_local_proof(
    node: Node<'_>,
    static_imports: &StaticImports,
) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    if parent.kind() != "method_invocation"
        || !parent
            .child_by_field_name("name")
            .is_some_and(|name| same_span(name, node))
        || parent.child_by_field_name("object").is_some()
    {
        return false;
    }
    static_imports.is_empty() && !enclosing_type_has_explicit_supertype(node)
}

fn enclosing_type_has_explicit_supertype(node: Node<'_>) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        if matches!(
            parent.kind(),
            "class_declaration" | "record_declaration" | "interface_declaration"
        ) {
            let mut cursor = parent.walk();
            return parent.named_children(&mut cursor).any(|child| {
                matches!(
                    child.kind(),
                    "superclass" | "super_interfaces" | "extends_interfaces"
                )
            });
        }
        current = parent.parent();
    }
    false
}

fn collect_call_shape_mismatches(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    node: Node<'_>,
    package: &str,
    imports: &[Import],
    facts: &crate::resolve::CompletenessFacts,
    out: &mut Vec<crate::diagnostics::Diagnostic>,
) {
    if matches!(
        node.kind(),
        "method_invocation" | "object_creation_expression"
    ) {
        if let Some(query) =
            java_call_shape_query(index, file, src, tree, node, package, imports, facts)
        {
            out.push(crate::diagnostics::Diagnostic {
                start_byte: node.start_byte(),
                end_byte: node.end_byte(),
                severity: crate::diagnostics::Severity::Error,
                code: Some(crate::diagnostics::DiagnosticCode::CallShapeMismatch),
                message: query.diagnostic_message(),
            });
            return;
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_call_shape_mismatches(index, file, src, tree, child, package, imports, facts, out);
    }
}

struct JavaCallShapeQuery {
    symbol: String,
    arg_count: usize,
    argument_types: Option<Vec<String>>,
}

#[derive(Default)]
struct StaticImports {
    paths: Vec<String>,
}

impl StaticImports {
    fn may_resolve(&self, name: &str) -> bool {
        self.owners(name).next().is_some()
    }

    fn owners<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> {
        self.paths.iter().filter_map(move |path| {
            let (owner, member) = path.rsplit_once('.')?;
            (member == name || member == "*").then_some(owner)
        })
    }

    fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

impl JavaCallShapeQuery {
    fn diagnostic_message(&self) -> String {
        if let Some(types) = &self.argument_types {
            return format!(
                "No overload of {} accepts argument type{} ({})",
                self.symbol,
                if types.len() == 1 { "" } else { "s" },
                types.join(", ")
            );
        }
        format!(
            "No overload of {} accepts {} argument{}",
            self.symbol,
            self.arg_count,
            if self.arg_count == 1 { "" } else { "s" }
        )
    }
}

fn java_call_shape_query(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    call: Node<'_>,
    package: &str,
    imports: &[Import],
    facts: &crate::resolve::CompletenessFacts,
) -> Option<JavaCallShapeQuery> {
    let name = java_call_name(call)?;
    let symbol = node_text(name, src).to_string();
    if symbol.is_empty() {
        return None;
    }
    let proven_receiver = if facts.compiler_producers_complete {
        None
    } else {
        // shortcut: selected-source proof is method-only, widen after constructor proof and coverage.
        if call.kind() != "method_invocation" {
            return None;
        }
        let (receiver, kind) = java_member_receiver(index, file, src, tree, call, 0)?;
        if kind != ktcore::jvm::ReceiverKind::Value || receiver.container().is_some() {
            return None;
        }
        let owner_complete = {
            let package = receiver.package()?;
            let mut owners = index
                .lookup_type(receiver.name()?)
                .into_iter()
                .filter(|owner| owner.sym.package == package && owner.sym.container.is_none());
            let owner = owners.next()?;
            owners.next().is_none() && index.java_source_members_complete(owner)
        };
        if !owner_complete {
            return None;
        }
        Some(receiver)
    };
    let entries = java_call_entries_with_imports(
        index, file, src, tree, call, name, &symbol, package, imports,
    );
    // A certified base declaration cannot close overloads on an unproven child receiver.
    if proven_receiver.is_some_and(|receiver| {
        entries.iter().any(|entry| {
            receiver.name() != entry.sym.container.as_deref()
                || receiver.package() != Some(entry.sym.package.as_str())
        })
    }) {
        return None;
    }
    if entries
        .iter()
        .any(|entry| !crate::resolve::declaration_surface_status(index, entry, facts).is_found())
    {
        return None;
    }
    call_shape_from_java_entries(index, file, src, tree, call, symbol, entries)
}

fn java_call_name(call: Node<'_>) -> Option<Node<'_>> {
    match call.kind() {
        "method_invocation" => call.child_by_field_name("name"),
        "object_creation_expression" => {
            let ty = call.child_by_field_name("type")?;
            simple_type_identifier_node(ty)
        }
        _ => None,
    }
}

fn simple_type_identifier_node(node: Node<'_>) -> Option<Node<'_>> {
    if node.kind() == "type_identifier" {
        return Some(node);
    }
    let mut cursor = node.walk();
    let mut last = None;
    for child in node.named_children(&mut cursor) {
        if child.kind() == "type_identifier" {
            last = Some(child);
        } else if let Some(found) = simple_type_identifier_node(child) {
            last = Some(found);
        }
    }
    last
}

pub(crate) fn java_call_entries(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    call: Node<'_>,
    name: Node<'_>,
    symbol: &str,
) -> Vec<Entry> {
    let package = package_of(tree, src);
    let imports = imports_of(tree, src);
    java_call_entries_with_imports(
        index, file, src, tree, call, name, symbol, &package, &imports,
    )
}

fn java_call_entries_with_imports(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    call: Node<'_>,
    name: Node<'_>,
    symbol: &str,
    package: &str,
    imports: &[Import],
) -> Vec<Entry> {
    let arguments = java_call_arguments(index, file, src, tree, call, 0);
    java_call_entries_with_arguments(
        index, file, src, tree, call, name, symbol, package, imports, &arguments, 0,
    )
}

fn java_call_entries_with_arguments(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    call: Node<'_>,
    name: Node<'_>,
    symbol: &str,
    package: &str,
    imports: &[Import],
    arguments: &ktcore::members::CallArguments,
    initializer_depth: usize,
) -> Vec<Entry> {
    let explicit_call =
        call.kind() == "method_invocation" && call.child_by_field_name("object").is_some();
    let mut arguments = arguments.clone();
    // Keep alternative arities for signature help and shape diagnostics while typing arguments.
    arguments.count = None;
    let member_call = java_member_call_candidates_with_arguments(
        index,
        file,
        src,
        tree,
        call,
        &arguments,
        initializer_depth,
    );
    let resolved_member_call = member_call.is_some();
    let defs = if let Some(selection) = member_call {
        selection
            .consumable_candidates()
            .iter()
            .copied()
            .filter_map(|candidate| match candidate {
                ktcore::jvm::CallableCandidate::Authored(_) => candidate.definition(),
                _ => None,
            })
            .collect()
    } else if explicit_call {
        return Vec::new();
    } else {
        goto_definition_with_imports(
            index,
            file,
            src,
            tree,
            name.start_byte(),
            package,
            imports,
            Some(&arguments),
            initializer_depth,
        )
    };
    let seeds = defs
        .into_iter()
        .filter_map(|def| {
            hierarchy::entry_for_name_range(index, &def.file, def.start_byte, def.end_byte)
        })
        .collect::<Vec<_>>();
    let mut entries = if call.kind() == "object_creation_expression" {
        constructor_entries_for_call(index, symbol, seeds)
    } else {
        seeds
            .into_iter()
            .filter(|entry| entry.sym.kind == SymbolKind::Function)
            .collect::<Vec<_>>()
    };
    if !resolved_member_call {
        expand_java_overloads(index, symbol, &mut entries);
    }
    entries.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then(a.sym.package.cmp(&b.sym.package))
            .then(a.sym.container.cmp(&b.sym.container))
            .then(a.sym.start_byte.cmp(&b.sym.start_byte))
            .then(a.sym.end_byte.cmp(&b.sym.end_byte))
    });
    entries.dedup_by(|a, b| {
        a.path == b.path
            && a.sym.package == b.sym.package
            && a.sym.container == b.sym.container
            && a.sym.start_byte == b.sym.start_byte
            && a.sym.end_byte == b.sym.end_byte
    });
    entries
}

fn constructor_entries_for_call(
    index: &dyn InferenceIndex,
    symbol: &str,
    seeds: Vec<Entry>,
) -> Vec<Entry> {
    let mut entries = Vec::new();
    for seed in seeds {
        if seed.sym.kind == SymbolKind::Function {
            entries.push(seed);
            continue;
        }
        if !seed.sym.kind.is_type_like() {
            continue;
        }
        entries.extend(index.lookup_by_name(symbol).iter().filter_map(|entry| {
            (entry.sym.kind == SymbolKind::Function
                && entry.sym.package == seed.sym.package
                && entry.sym.container.as_deref() == Some(seed.sym.name.as_str()))
            .then_some(entry.clone())
        }));
    }
    entries
}

fn expand_java_overloads(index: &dyn InferenceIndex, symbol: &str, entries: &mut Vec<Entry>) {
    let seeds = entries.clone();
    for seed in seeds {
        for entry in index.lookup_by_name(symbol) {
            if entry.sym.kind != SymbolKind::Function {
                continue;
            }
            if entry.sym.package != seed.sym.package || entry.sym.container != seed.sym.container {
                continue;
            }
            entries.push(entry.clone());
        }
    }
}

fn call_shape_from_java_entries(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    call: Node<'_>,
    symbol: String,
    entries: Vec<Entry>,
) -> Option<JavaCallShapeQuery> {
    if entries.is_empty() {
        return None;
    }
    if entries.iter().any(|entry| entry.tier != Tier::Volatile) {
        return None;
    }
    if entries.iter().any(|entry| {
        entry.sym.arity.is_none() || entry.sym.min_arity.is_none() || entry.sym.has_vararg
    }) {
        return None;
    }
    let args = java_argument_nodes(src, call)?;
    let arg_count = args.len();
    let arity_compatible = entries
        .iter()
        .filter(|entry| {
            let min = entry.sym.min_arity.expect("guarded above") as usize;
            let max = entry.sym.arity.expect("guarded above") as usize;
            (min..=max).contains(&arg_count)
        })
        .collect::<Vec<_>>();
    if arity_compatible.is_empty() {
        return Some(JavaCallShapeQuery {
            symbol,
            arg_count,
            argument_types: None,
        });
    }
    java_argument_type_mismatch_query(
        index,
        file,
        src,
        tree,
        args,
        symbol,
        arg_count,
        arity_compatible,
    )
}

fn java_argument_type_mismatch_query(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    args: Vec<Node<'_>>,
    symbol: String,
    arg_count: usize,
    entries: Vec<&Entry>,
) -> Option<JavaCallShapeQuery> {
    let mut arg_types = Vec::with_capacity(args.len());
    let mut labels = Vec::with_capacity(args.len());
    for arg in args {
        let ty = java_expression_type(index, file, src, tree, arg, 0)?;
        labels.push(ty.name.clone());
        arg_types.push(ty);
    }
    if entries.iter().any(|entry| {
        let min = entry.sym.min_arity.expect("guarded above") as usize;
        let max = entry.sym.arity.expect("guarded above") as usize;
        (min..=max).contains(&arg_count)
            && java_params_accept_args(index, &entry.sym.params, &arg_types)
    }) {
        return None;
    }
    Some(JavaCallShapeQuery {
        symbol,
        arg_count,
        argument_types: Some(labels),
    })
}

fn java_argument_nodes<'tree>(src: &str, call: Node<'tree>) -> Option<Vec<Node<'tree>>> {
    let args = call.child_by_field_name("arguments")?;
    let mut out = Vec::new();
    let mut cursor = args.walk();
    for child in args.named_children(&mut cursor) {
        if !matches!(child.kind(), "line_comment" | "block_comment")
            && !node_text(child, src).trim().is_empty()
        {
            out.push(child);
        }
    }
    Some(out)
}

#[derive(Clone, Debug)]
struct JavaExprType {
    name: String,
    package: Option<String>,
}

fn java_expression_type(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    expr: Node<'_>,
    initializer_depth: usize,
) -> Option<JavaExprType> {
    match expr.kind() {
        "string_literal" => Some(JavaExprType {
            name: "String".to_string(),
            package: Some("java.lang".to_string()),
        }),
        "character_literal" => Some(JavaExprType {
            name: "char".to_string(),
            package: None,
        }),
        "true" | "false" => Some(JavaExprType {
            name: "boolean".to_string(),
            package: None,
        }),
        "decimal_floating_point_literal" | "hex_floating_point_literal" => Some(JavaExprType {
            name: java_floating_literal_type(node_text(expr, src)).to_string(),
            package: None,
        }),
        "decimal_integer_literal"
        | "hex_integer_literal"
        | "octal_integer_literal"
        | "binary_integer_literal" => Some(JavaExprType {
            name: java_integer_literal_type(node_text(expr, src)).to_string(),
            package: None,
        }),
        "object_creation_expression" => {
            let ty = expr.child_by_field_name("type")?;
            let receiver = receiver_type_from_type_node(index, file, src, tree, ty)?;
            Some(JavaExprType {
                name: receiver.name()?.to_string(),
                package: receiver.package().map(str::to_string),
            })
        }
        "identifier" => {
            let decl = local_declaration_node(src, expr)
                .or_else(|| same_file_field_declaration(src, expr))?;
            let receiver =
                type_of_value_declaration(index, file, src, tree, decl, initializer_depth)?;
            Some(JavaExprType {
                name: receiver.name()?.to_string(),
                package: receiver.package().map(str::to_string),
            })
        }
        _ => None,
    }
}

fn java_integer_literal_type(text: &str) -> &'static str {
    if text.ends_with('l') || text.ends_with('L') {
        "long"
    } else {
        "int"
    }
}

fn java_floating_literal_type(text: &str) -> &'static str {
    if text.ends_with('f') || text.ends_with('F') {
        "float"
    } else {
        "double"
    }
}

fn java_params_accept_args(
    index: &dyn InferenceIndex,
    params: &[TypeRef],
    args: &[JavaExprType],
) -> bool {
    if params.len() != args.len() {
        return false;
    }
    params
        .iter()
        .zip(args)
        .all(|(param, arg)| java_type_accepts(index, param, arg))
}

fn java_type_accepts(index: &dyn InferenceIndex, param: &TypeRef, arg: &JavaExprType) -> bool {
    let formal = simple_java_type_name(&param.name);
    let actual = simple_java_type_name(&arg.name);
    if formal == actual {
        return true;
    }
    if is_java_type_variable(&formal) || is_java_type_variable(&actual) {
        return true;
    }
    if formal == "Object" {
        return true;
    }
    if java_boxing_accepts(&formal, &actual) {
        return true;
    }
    if java_primitive_widening_accepts(&formal, &actual) {
        return true;
    }
    if formal == "Throwable" && matches!(actual.as_str(), "Exception" | "RuntimeException") {
        return true;
    }
    if is_java_primitive_type(&formal) || is_java_primitive_type(&actual) {
        return false;
    }
    java_is_subtype(index, &actual, arg.package.as_deref(), &formal)
}

fn java_is_subtype(
    index: &dyn InferenceIndex,
    actual: &str,
    package: Option<&str>,
    formal: &str,
) -> bool {
    let mut visited = std::collections::HashSet::new();
    let mut frontier = vec![(actual.to_string(), package.map(str::to_string))];
    while let Some((current, current_package)) = frontier.pop() {
        if !visited.insert((current.clone(), current_package.clone())) {
            continue;
        }
        for supertype in index.supertypes_of_in(&current, current_package.as_deref()) {
            if supertype == formal {
                return true;
            }
            let super_package =
                java_same_pkg_supertype(index, &supertype, current_package.as_deref());
            frontier.push((supertype, super_package));
        }
    }
    false
}

fn is_java_type_variable(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_uppercase() && chars.all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit())
}

fn java_boxing_accepts(formal: &str, actual: &str) -> bool {
    java_boxed_type(actual) == Some(formal)
        || java_boxed_type(formal) == Some(actual)
        || java_boxed_type(actual)
            .is_some_and(|boxed| java_primitive_widening_accepts(formal, boxed))
        || java_boxed_type(formal)
            .is_some_and(|boxed| java_primitive_widening_accepts(boxed, actual))
}

fn java_boxed_type(primitive: &str) -> Option<&'static str> {
    match primitive {
        "boolean" => Some("Boolean"),
        "byte" => Some("Byte"),
        "char" => Some("Character"),
        "double" => Some("Double"),
        "float" => Some("Float"),
        "int" => Some("Integer"),
        "long" => Some("Long"),
        "short" => Some("Short"),
        _ => None,
    }
}

fn java_primitive_widening_accepts(formal: &str, actual: &str) -> bool {
    matches!(
        (formal, actual),
        ("short", "byte")
            | ("int", "byte" | "short" | "char")
            | ("long", "byte" | "short" | "char" | "int")
            | ("float", "byte" | "short" | "char" | "int" | "long")
            | (
                "double",
                "byte" | "short" | "char" | "int" | "long" | "float"
            )
    )
}

fn simple_java_type_name(name: &str) -> String {
    name.split('<')
        .next()
        .unwrap_or(name)
        .trim_end_matches("[]")
        .rsplit('.')
        .next()
        .unwrap_or(name)
        .trim()
        .to_string()
}

fn is_java_primitive_type(name: &str) -> bool {
    matches!(
        name,
        "boolean" | "byte" | "char" | "double" | "float" | "int" | "long" | "short"
    )
}

fn same_span(a: Node<'_>, b: Node<'_>) -> bool {
    a.start_byte() == b.start_byte() && a.end_byte() == b.end_byte()
}

fn is_java_builtin_type(name: &str) -> bool {
    matches!(
        name,
        "boolean"
            | "byte"
            | "char"
            | "double"
            | "float"
            | "int"
            | "long"
            | "short"
            | "void"
            | "String"
            | "Object"
            | "Integer"
            | "Long"
            | "var"
            | "Double"
            | "Float"
            | "Boolean"
            | "Character"
            | "Byte"
            | "Short"
    )
}

fn diagnostic_imports(tree: &Tree, src: &str) -> Vec<DiagnosticImport> {
    let mut out = Vec::new();
    let root = tree.root_node();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() != "import_declaration" {
            continue;
        }
        if child
            .children(&mut child.walk())
            .any(|c| c.kind() == "static")
        {
            continue;
        }
        let parsed = parse_import(child, src);
        let Some(import) = parsed else {
            continue;
        };
        if import.wildcard {
            continue;
        }
        let Some(name) = import.name else {
            continue;
        };
        out.push(DiagnosticImport {
            name,
            start_byte: child.start_byte(),
            end_byte: child.end_byte(),
        });
    }
    out
}

/// All regular (non-static) import statements in the file.
pub(crate) fn imports_of(tree: &Tree, src: &str) -> Vec<Import> {
    let mut out = Vec::new();
    let root = tree.root_node();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() != "import_declaration" {
            continue;
        }
        if child
            .children(&mut child.walk())
            .any(|c| c.kind() == "static")
        {
            continue;
        }
        let mut package = String::new();
        let mut name = None;
        let mut qualifier = None;
        let mut wildcard = false;
        for c in child.children(&mut child.walk()) {
            match c.kind() {
                "identifier" | "type_identifier" => {
                    name = Some(node_text(c, src).to_string());
                }
                "scoped_identifier" | "scoped_type_identifier" => {
                    qualifier = Some(node_text(c, src).to_string());
                }
                "asterisk" => wildcard = true,
                _ => {}
            }
        }
        if wildcard {
            // `import pkg.*`: package is the full qualifier, or the single identifier if there is
            // no qualifier (`import pkg.*`).
            package = qualifier.unwrap_or_else(|| name.take().unwrap_or_default());
            name = None;
        } else if let Some(q) = qualifier {
            if let Some(pos) = q.rfind('.') {
                // `import pkg.Type`: package is the qualifier, name is the last segment.
                package = q[..pos].to_string();
                name = Some(q[pos + 1..].to_string());
            } else {
                // Single-segment qualifier: default-package import.
                name = Some(q);
            }
        }
        out.push(Import {
            package,
            name,
            wildcard,
        });
    }
    out
}

fn static_imports_of(tree: &Tree, src: &str) -> StaticImports {
    let mut out = StaticImports::default();
    let root = tree.root_node();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() != "import_declaration" {
            continue;
        }
        let text = node_text(child, src).trim();
        let Some(rest) = text.strip_prefix("import static ") else {
            continue;
        };
        let path = rest.trim().trim_end_matches(';').trim();
        if path
            .rsplit_once('.')
            .is_some_and(|(_, name)| !name.is_empty())
        {
            out.paths.push(path.to_string());
        }
    }
    out
}

/// Explicit regular imports as fully-qualified names, excluding wildcard and static imports.
pub fn explicit_import_fqns(tree: &Tree, src: &str) -> Vec<String> {
    imports_of(tree, src)
        .into_iter()
        .filter_map(|imp| imp.name.map(|name| format!("{}.{}", imp.package, name)))
        .collect()
}

/// Default-import JDK source candidates mentioned by simple Java type references.
/// Loading a real source file does not change lexical or import resolution precedence.
pub(crate) fn referenced_java_lang_fqns(tree: &Tree, src: &str) -> Vec<String> {
    // shortcut: expression-only class names wait for background indexing until value/type contexts are distinguished.
    let mut names = HashSet::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.kind() == "type_identifier"
            && node.parent().is_some_and(|parent| {
                !matches!(parent.kind(), "scoped_type_identifier" | "type_parameter")
            })
        {
            let name = node_text(node, src);
            if !name.is_empty() && name != "var" {
                names.insert(format!("java.lang.{name}"));
            }
        }
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
    }
    let mut names = names.into_iter().collect::<Vec<_>>();
    names.sort();
    names
}

/// Explicit regular import containing `offset`, as a fully-qualified name.
pub fn explicit_import_fqn_at(tree: &Tree, src: &str, offset: usize) -> Option<String> {
    let root = tree.root_node();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() != "import_declaration" {
            continue;
        }
        if offset < child.start_byte() || offset > child.end_byte() {
            continue;
        }
        let imp = parse_import(child, src)?;
        if imp.wildcard {
            return None;
        }
        let name = imp.name?;
        return Some(format!("{}.{}", imp.package, name));
    }
    None
}

/// Goto-definition for an identifier inside a Java source file. This is intentionally minimal:
/// local variables/parameters, `this.*`, and cross-file/same-package resolution work; broader
/// member access and static imports are still best-effort or not yet implemented.
pub fn goto_definition(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    offset: usize,
) -> Vec<Def> {
    if let Some(binding) = index.semantic_binding_at(file, offset) {
        return vec![index.semantic_definition(binding.target.definition())];
    }
    authored_goto_definition(index, file, src, tree, offset)
        .into_iter()
        .map(|definition| index.semantic_definition(&definition))
        .collect()
}

pub fn authored_goto_definition(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    offset: usize,
) -> Vec<Def> {
    let package = package_of(tree, src);
    let imports = imports_of(tree, src);
    goto_definition_with_imports(index, file, src, tree, offset, &package, &imports, None, 0)
}

fn fallback_method_entry_visibility(
    index: &dyn InferenceIndex,
    src: &str,
    tree: &Tree,
    usage: Node<'_>,
    entry: &Entry,
    static_imports: &StaticImports,
) -> Option<bool> {
    let Some(call) = usage.parent().filter(|parent| {
        parent.kind() == "method_invocation" && parent.child_by_field_name("name") == Some(usage)
    }) else {
        return None;
    };
    if call.child_by_field_name("object").is_some() || entry.sym.kind != SymbolKind::Function {
        return Some(false);
    }
    let package = package_of(tree, src);
    let mut ancestor = call.parent();
    while let Some(node) = ancestor {
        if matches!(
            node.kind(),
            "class_declaration"
                | "interface_declaration"
                | "enum_declaration"
                | "record_declaration"
        ) && entry.sym.package == package
            && name_field(node)
                .is_some_and(|name| entry.sym.container.as_deref() == Some(node_text(name, src)))
        {
            return Some(true);
        }
        ancestor = node.parent();
    }
    let name = node_text(usage, src);
    if !static_imports.may_resolve(name) {
        return Some(false);
    }
    let ctx = FileCtx::new(package, inference_imports(tree, src));
    let scope = type_scope(tree, src);
    Some(static_imports.owners(name).any(|owner| {
        let receiver = infer::declared_type(index, &scope.type_ref(owner, false, Vec::new()), &ctx);
        ktcore::jvm::member_candidates(index, &receiver, name)
            .iter()
            .any(|candidate| {
                let is_static = match candidate {
                    ktcore::jvm::CallableCandidate::Authored(member) => {
                        member.sym.source_member.dispatch()
                            == Some(ktcore::extensions::Dispatch::Static)
                    }
                    ktcore::jvm::CallableCandidate::Jvm { facts, .. } => facts.is_static,
                };
                is_static
                    && candidate.definition().is_some_and(|def| {
                        def.file == entry.path.as_ref()
                            && def.start_byte == entry.sym.start_byte
                            && def.end_byte == entry.sym.end_byte
                    })
            })
    }))
}

fn goto_definition_with_imports(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    offset: usize,
    package: &str,
    imports: &[Import],
    arguments: Option<&ktcore::members::CallArguments>,
    initializer_depth: usize,
) -> Vec<Def> {
    let usage = match identifier_at(tree, offset) {
        Some(node) => node,
        None => return Vec::new(),
    };
    if is_declaration_name(usage) {
        return vec![Def {
            file: file.to_string(),
            start_byte: usage.start_byte(),
            end_byte: usage.end_byte(),
        }];
    }
    if let Some(defs) = qualified_type_defs(index, src, tree, usage) {
        return defs;
    }
    if usage.kind() == "type_identifier" {
        if let Some(definitions) = lexical_member_type_definitions(index, file, src, tree, usage) {
            return definitions;
        }
        let Some(ty) = receiver_type_from_type_node(index, file, src, tree, usage) else {
            return Vec::new();
        };
        return match ty {
            Type::Parameter { definition, .. } => vec![definition],
            ty => type_defs_for_receiver(index, &ty),
        };
    }
    if let Some(local) = local_definition(file, src, usage) {
        return vec![local];
    }
    if usage.kind() == "identifier"
        && usage.parent().is_some_and(|parent| {
            matches!(parent.kind(), "method_invocation" | "field_access")
                && parent.child_by_field_name("object") == Some(usage)
        })
    {
        if let Some(field) = same_file_field_declaration(src, usage) {
            return vec![Def {
                file: file.to_string(),
                start_byte: field.start_byte(),
                end_byte: field.end_byte(),
            }];
        }
    }
    if let Some(defs) =
        receiver_member_defs(index, file, src, tree, usage, arguments, initializer_depth)
    {
        return defs;
    }
    if let Some(defs) = qualified_member_defs(index, src, tree, usage) {
        return defs;
    }
    let name = node_text(usage, src);
    let static_imports = static_imports_of(tree, src);
    let mut candidates: Vec<Def> = Vec::new();
    for entry in index.lookup_by_name(name) {
        let sym = &entry.sym;
        if sym.name != name {
            continue;
        }
        if is_type_name_position(usage) && !sym.kind.is_type_like() {
            continue;
        }
        if fallback_method_entry_visibility(index, src, tree, usage, entry, &static_imports)
            .unwrap_or_else(|| is_visible(&package, &imports, sym))
        {
            candidates.push(Def {
                file: entry.path.to_string(),
                start_byte: sym.start_byte,
                end_byte: sym.end_byte,
            });
        }
    }
    candidates.sort();
    candidates.dedup();
    candidates
}

fn java_reference_resolves_with_ctx(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    usage: Node<'_>,
    ctx: &JavaDiagnosticContext,
) -> bool {
    if is_declaration_name(usage) || local_declaration_node(src, usage).is_some() {
        return true;
    }
    if type_parameter_in_scope(usage, src) {
        return true;
    }
    if receiver_member_defs(index, file, src, tree, usage, None, 0)
        .is_some_and(|defs| !defs.is_empty())
    {
        return true;
    }
    if qualified_member_defs(index, src, tree, usage).is_some_and(|defs| !defs.is_empty()) {
        return true;
    }
    if let Some(defs) = qualified_type_defs(index, src, tree, usage) {
        return !defs.is_empty();
    }
    let name = node_text(usage, src);
    index.lookup_by_name(name).iter().any(|entry| {
        let sym = &entry.sym;
        sym.name == name
            && (!is_type_name_position(usage) || sym.kind.is_type_like())
            && fallback_method_entry_visibility(index, src, tree, usage, entry, &ctx.static_imports)
                .unwrap_or_else(|| {
                    is_visible_with_facts(&ctx.package, &ctx.imports, &ctx.visibility, sym)
                })
    })
}

fn qualified_type_defs(
    index: &dyn InferenceIndex,
    src: &str,
    tree: &Tree,
    usage: Node<'_>,
) -> Option<Vec<Def>> {
    if usage.kind() != "type_identifier" {
        return None;
    }
    let mut head = usage;
    let mut qualified = false;
    while let Some(parent) = head.parent() {
        match parent.kind() {
            "generic_type" if parent.named_child(0) == Some(head) => head = parent,
            "scoped_type_identifier" => {
                qualified = true;
                if parent.named_child(parent.named_child_count().saturating_sub(1)) != Some(head) {
                    break;
                }
                head = parent;
            }
            _ => break,
        }
    }
    if !qualified {
        return None;
    }
    let name = type_path(head, src)?;
    Some(type_defs_for_name(index, "", src, tree, &name))
}

fn is_type_name_position(node: Node<'_>) -> bool {
    if node.kind() == "type_identifier" {
        return true;
    }
    let Some(parent) = node.parent() else {
        return false;
    };
    if parent.kind() == "scoped_identifier"
        && parent.child_by_field_name("name") == Some(node)
        && parent
            .parent()
            .is_some_and(|grandparent| grandparent.kind() == "import_declaration")
    {
        return true;
    }
    false
}

pub fn type_definition(
    index: &dyn InferenceIndex,
    file: &str,
    tree: &Tree,
    src: &str,
    offset: usize,
) -> Vec<Def> {
    let Some(usage) = identifier_at(tree, offset) else {
        return Vec::new();
    };
    if usage.kind() == "type_identifier" {
        if let Some(defs) = qualified_type_defs(index, src, tree, usage) {
            return defs;
        }
        if let Some(definitions) = lexical_member_type_definitions(index, file, src, tree, usage) {
            return definitions;
        }
        return receiver_type_from_type_node(index, file, src, tree, usage)
            .map(|ty| type_defs_for_receiver(index, &ty))
            .unwrap_or_default();
    }
    if let Some(decl) = if is_declaration_name(usage) {
        Some(usage)
    } else {
        local_declaration_node(src, usage).or_else(|| same_file_field_declaration(src, usage))
    } {
        if let Some(ty) = type_of_value_declaration(index, file, src, tree, decl, 0) {
            return type_defs_for_receiver(index, &ty);
        }
    }
    Vec::new()
}

fn lexical_member_type_definitions(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    usage: Node<'_>,
) -> Option<Vec<Def>> {
    let name = node_text(usage, src);
    let mut current = usage.parent();
    while let Some(node) = current {
        // A nearer type parameter keeps the existing declaration-scoped resolver.
        if type_parameter_names(node, src)
            .iter()
            .any(|parameter| parameter == name)
        {
            return None;
        }
        if matches!(
            node.kind(),
            "class_body" | "interface_body" | "enum_body_declarations" | "annotation_type_body"
        ) {
            let mut cursor = node.walk();
            let declarations = node
                .named_children(&mut cursor)
                .filter(|child| {
                    matches!(
                        child.kind(),
                        "class_declaration"
                            | "interface_declaration"
                            | "enum_declaration"
                            | "record_declaration"
                            | "annotation_type_declaration"
                    )
                })
                .filter_map(|child| name_field(child).filter(|decl| node_text(*decl, src) == name))
                .collect::<Vec<_>>();
            match declarations.as_slice() {
                [] => {}
                [declaration] => {
                    return Some(vec![Def {
                        file: file.to_string(),
                        start_byte: declaration.start_byte(),
                        end_byte: declaration.end_byte(),
                    }])
                }
                _ => return Some(Vec::new()),
            }
            let mut owner = node.parent()?;
            if owner.kind() == "enum_body" {
                owner = owner.parent()?;
            }
            // This class's parameters precede inherited names; an outer class's do not.
            if type_parameter_names(owner, src)
                .iter()
                .any(|parameter| parameter == name)
            {
                return None;
            }
            if let Some(definitions) =
                inherited_member_type_definitions(index, file, src, tree, owner, name)
            {
                return Some(definitions);
            }
        }
        current = node.parent();
    }
    None
}

fn current_java_type_declaration<'tree>(
    file: &str,
    src: &str,
    tree: &'tree Tree,
    entry: &Entry,
) -> Option<Node<'tree>> {
    if entry.path.as_ref() != file || !entry.sym.kind.is_type_like() {
        return None;
    }
    let name = tree
        .root_node()
        .named_descendant_for_byte_range(entry.sym.start_byte, entry.sym.end_byte)?;
    let declaration = name.parent()?;
    (name.start_byte() == entry.sym.start_byte
        && name.end_byte() == entry.sym.end_byte
        && node_text(name, src) == entry.sym.name
        && name_field(declaration) == Some(name)
        && matches!(
            declaration.kind(),
            "class_declaration"
                | "interface_declaration"
                | "enum_declaration"
                | "record_declaration"
                | "annotation_type_declaration"
        )
        && java_type_parameter_facts(declaration).known_invariant())
    .then_some(declaration)
}

fn inherited_member_type_definitions(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    owner: Node<'_>,
    name: &str,
) -> Option<Vec<Def>> {
    let anonymous = owner.kind() == "object_creation_expression";
    let mut cursor = owner.walk();
    if !anonymous
        && !owner.named_children(&mut cursor).any(|child| {
            matches!(
                child.kind(),
                "superclass" | "super_interfaces" | "extends_interfaces"
            )
        })
    {
        return None;
    }
    let scope = type_scope(tree, src);
    let written_scope = TypeScope::with_default_imports("", Vec::new(), &[]);
    let receiver = if anonymous {
        // Method-local type declarations are not indexed; do not guess the created base through one.
        let mut ancestor = owner.parent();
        while let Some(node) = ancestor {
            if matches!(
                node.kind(),
                "class_body" | "interface_body" | "enum_body_declarations"
            ) {
                break;
            }
            if matches!(node.kind(), "block" | "constructor_body" | "switch_block") {
                return Some(Vec::new());
            }
            ancestor = node.parent();
        }
        let Some(parent) = owner.child_by_field_name("type") else {
            return Some(Vec::new());
        };
        let scoped = java_declaration_scope(owner, src, &scope);
        let Some(written) = type_ref_from_node(parent, src, &written_scope) else {
            return Some(Vec::new());
        };
        let Some(expanded) = type_ref_from_node(parent, src, scoped.as_ref().unwrap_or(&scope))
        else {
            return Some(Vec::new());
        };
        if parent_type_import_shadowed(index, file, src, owner, &[written], &[expanded]) {
            return Some(Vec::new());
        }
        let Some(ty) = receiver_type_from_type_node(index, file, src, tree, parent) else {
            return Some(Vec::new());
        };
        ty
    } else {
        let Some(declaration) = name_field(owner) else {
            return Some(Vec::new());
        };
        let Some(entry) = hierarchy::entry_for_name_range(
            index,
            file,
            declaration.start_byte(),
            declaration.end_byte(),
        ) else {
            return Some(Vec::new());
        };
        if current_java_type_declaration(file, src, tree, &entry) != Some(owner) {
            return Some(Vec::new());
        }
        let scoped = java_declaration_scope(owner, src, &scope);
        if entry.sym.supertype_refs != supertype_refs(owner, src, scoped.as_ref().unwrap_or(&scope))
        {
            return Some(Vec::new());
        }
        Type::Class {
            name: entry.sym.name.clone(),
            package: Some(entry.sym.package.clone()),
            container: entry.sym.container.clone(),
            nullable: false,
            args: vec![Type::Unknown; entry.sym.type_params.len()],
        }
    };
    let package = package_of(tree, src);
    let mut definitions = Vec::new();
    let mut incomplete = false;
    let mut package_member = false;
    let mut foreign_package = false;
    let _ = infer::walk_supertypes(index, &receiver, JAVA_SUPERTYPE_WALK_CAP, |current| {
        let mut heads = index
            .lookup_type(current.name().unwrap_or_default())
            .into_iter()
            .filter(|entry| {
                current.package() == Some(entry.sym.package.as_str())
                    && current.container() == entry.sym.container.as_deref()
            });
        let Some(head) = heads.next() else {
            incomplete = true;
            return ControlFlow::Break(());
        };
        // These existing facts also reject a malformed ancestor header with a dropped parent.
        if heads.next().is_some()
            || (!head.sym.type_parameter_facts.known_unbounded()
                && !head.sym.type_parameter_facts.known_invariant())
            || (anonymous
                && current == &receiver
                && index
                    .lookup_type(&head.sym.name)
                    .iter()
                    .filter(|entry| entry.sym.package == head.sym.package)
                    .count()
                    != 1)
        {
            incomplete = true;
            return ControlFlow::Break(());
        }
        if head.path.as_ref() == file {
            let Some(declaration) = current_java_type_declaration(file, src, tree, head) else {
                incomplete = true;
                return ControlFlow::Break(());
            };
            let scoped = java_declaration_scope(declaration, src, &scope);
            if head.sym.package != package
                || head.sym.supertype_refs
                    != supertype_refs(declaration, src, scoped.as_ref().unwrap_or(&scope))
            {
                incomplete = true;
                return ControlFlow::Break(());
            }
            if parent_type_import_shadowed(
                index,
                file,
                src,
                declaration,
                &supertype_refs(declaration, src, &written_scope),
                &head.sym.supertype_refs,
            ) {
                incomplete = true;
                return ControlFlow::Break(());
            }
        } else {
            // shortcut: foreign nested Kotlin parents stay Unknown until lexical alias binding is available.
            if (head.path.ends_with(".kt") || head.path.ends_with(".kts"))
                && head.sym.container.is_some()
                && (!head.sym.supertype_refs.is_empty() || !head.sym.supertypes.is_empty())
            {
                incomplete = true;
                return ControlFlow::Break(());
            }
            // Without this ancestor's raw header, a lexical member may have been import-expanded.
            for parent in &head.sym.supertype_refs {
                let mut container = head.sym.container.as_deref();
                while let Some(outer) = container {
                    if index.members_of(outer).iter().any(|member| {
                        member.path == head.path
                            && member.sym.kind.is_type_like()
                            && (member.sym.name == parent.name
                                || parent.qualification.as_deref().is_some_and(|prefix| {
                                    prefix.split('.').any(|segment| member.sym.name == segment)
                                }))
                            && member.sym.package == head.sym.package
                            && member.sym.container.as_deref() == Some(outer)
                    }) {
                        incomplete = true;
                        return ControlFlow::Break(());
                    }
                    container = outer.rsplit_once('.').map(|(outer, _)| outer);
                }
            }
        }
        // The existing parent TypeRef cannot choose a lexical member head from same-package homonyms.
        if head.sym.supertype_refs.iter().any(|parent| {
            parent.qualification.is_none()
                && parent.container_candidates.is_empty()
                && parent
                    .package_candidates
                    .iter()
                    .find_map(|package| {
                        let count = index
                            .lookup_type(&parent.name)
                            .iter()
                            .filter(|entry| entry.sym.package == *package)
                            .count();
                        (count != 0).then_some(count > 1)
                    })
                    .unwrap_or(false)
        }) {
            incomplete = true;
            return ControlFlow::Break(());
        }
        foreign_package |= head.sym.package != package;
        if !anonymous && current == &receiver {
            return ControlFlow::Continue(());
        }
        let Some(root) = infer::receiver_root_key(index, current) else {
            incomplete = true;
            return ControlFlow::Break(());
        };
        for member in index.members_of(&root).into_iter().filter(|entry| {
            entry.sym.kind.is_type_like()
                && entry.sym.name == name
                && entry.sym.package == head.sym.package
                && entry.sym.container.as_deref() == Some(root.as_str())
        }) {
            let Some(declaration) = current_java_type_declaration(file, src, tree, member) else {
                incomplete = true;
                return ControlFlow::Break(());
            };
            let mut access = Vec::new();
            if let Some(modifiers) = child_of_kind(declaration, "modifiers") {
                let mut cursor = modifiers.walk();
                access.extend(modifiers.children(&mut cursor).filter_map(|modifier| {
                    match modifier.kind() {
                        "public" | "protected" | "private" => Some(modifier.kind()),
                        _ => None,
                    }
                }));
            }
            match access.as_slice() {
                [] => package_member = true,
                ["public"] | ["protected"] => {}
                _ => {
                    incomplete = true;
                    return ControlFlow::Break(());
                }
            }
            definitions.push(Def {
                file: member.path.to_string(),
                start_byte: member.sym.start_byte,
                end_byte: member.sym.end_byte,
            });
        }
        ControlFlow::Continue(())
    });
    definitions.sort();
    definitions.dedup();
    if incomplete || package_member && foreign_package || definitions.len() > 1 {
        Some(Vec::new())
    } else if definitions.is_empty() {
        None
    } else {
        Some(definitions)
    }
}

fn parent_type_import_shadowed(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    owner: Node<'_>,
    written: &[TypeRef],
    expanded: &[TypeRef],
) -> bool {
    let ctx = FileCtx::new(String::new(), Vec::new()).with_file(file);
    for parent in written {
        let first = parent
            .qualification
            .as_deref()
            .and_then(|prefix| prefix.split('.').next())
            .unwrap_or(&parent.name);
        let mut current = owner.parent();
        while let Some(node) = current {
            if matches!(
                node.kind(),
                "class_body" | "interface_body" | "enum_body_declarations" | "annotation_type_body"
            ) {
                let mut cursor = node.walk();
                let declarations = node
                    .named_children(&mut cursor)
                    .filter(|child| {
                        matches!(
                            child.kind(),
                            "class_declaration"
                                | "interface_declaration"
                                | "enum_declaration"
                                | "record_declaration"
                                | "annotation_type_declaration"
                        )
                    })
                    .filter_map(|child| {
                        name_field(child).filter(|name| node_text(*name, src) == first)
                    })
                    .collect::<Vec<_>>();
                if !declarations.is_empty() {
                    // A qualified lexical root requires member-path binding the stamped import cannot prove.
                    if declarations.len() != 1 || parent.qualification.is_some() {
                        return true;
                    }
                    let targets = expanded
                        .iter()
                        .filter(|target| target.name == parent.name)
                        .collect::<Vec<_>>();
                    let [target] = targets.as_slice() else {
                        return true;
                    };
                    let expected = Def {
                        file: file.to_string(),
                        start_byte: declarations[0].start_byte(),
                        end_byte: declarations[0].end_byte(),
                    };
                    if type_defs_for_receiver(index, &infer::declared_type(index, target, &ctx))
                        != vec![expected]
                    {
                        return true;
                    }
                    break;
                }
            }
            current = node.parent();
        }
    }
    false
}

fn type_defs_for_receiver(index: &dyn InferenceIndex, ty: &Type) -> Vec<Def> {
    let Some(name) = ty.name() else {
        return Vec::new();
    };
    let mut defs = index
        .lookup_type(name)
        .into_iter()
        .filter(|entry| {
            ty.package().is_none_or(|pkg| entry.sym.package == pkg)
                && ty.container() == entry.sym.container.as_deref()
        })
        .map(|entry| Def {
            file: entry.path.to_string(),
            start_byte: entry.sym.start_byte,
            end_byte: entry.sym.end_byte,
        })
        .collect::<Vec<_>>();
    defs.sort();
    defs.dedup();
    defs
}

fn type_defs_for_name(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    name: &str,
) -> Vec<Def> {
    let _ = file;
    let ty =
        type_scope(tree, src).type_ref(name.split('<').next().unwrap_or(name), false, Vec::new());
    type_defs_for_receiver(
        index,
        &infer::declared_type(
            index,
            &ty,
            &FileCtx::new(package_of(tree, src), inference_imports(tree, src)),
        ),
    )
}

/// Java call site under `offset`, for signature help. Handles ordinary method calls
/// (`greet(a, b)`, `receiver.greet(a, b)`) and constructor calls (`new Helper(a, b)`).
pub fn call_at<'tree>(
    tree: &'tree Tree,
    src: &str,
    offset: usize,
) -> Option<(Node<'tree>, Node<'tree>, String, u32)> {
    let mut node = tree
        .root_node()
        .named_descendant_for_byte_range(offset, offset)?;
    loop {
        match node.kind() {
            "method_invocation" => {
                let name = node.child_by_field_name("name")?;
                if offset < name.end_byte() {
                    return None;
                }
                let active = active_parameter(src, name.end_byte(), offset);
                return Some((node, name, node_text(name, src).to_string(), active));
            }
            "object_creation_expression" => {
                let ty = node.child_by_field_name("type")?;
                let callee = simple_type_identifier_node(ty)?;
                if offset < ty.end_byte() {
                    return None;
                }
                let name = simple_type_name(ty, src)?;
                let active = active_parameter(src, ty.end_byte(), offset);
                return Some((node, callee, name, active));
            }
            _ => node = node.parent()?,
        }
    }
}

fn simple_type_name(node: Node<'_>, src: &str) -> Option<String> {
    let text = node_text(node, src).trim();
    let without_args = text.split('<').next().unwrap_or(text);
    without_args
        .rsplit('.')
        .next()
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

fn active_parameter(text: &str, callee_end: usize, offset: usize) -> u32 {
    let mut depth = 0_i32;
    let mut active = 0_u32;
    let start = complete::floor_boundary(text, callee_end);
    let end = complete::floor_boundary(text, offset);
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

pub fn enclosing_callable_item(
    index: &dyn InferenceIndex,
    file: &str,
    tree: &Tree,
    text: &str,
    offset: usize,
) -> Option<HierarchyItem> {
    let mut node = tree
        .root_node()
        .named_descendant_for_byte_range(offset, offset)?;
    loop {
        match node.kind() {
            "method_declaration" | "constructor_declaration" => {
                let name = name_field(node)?;
                return hierarchy::entry_for_name_range(
                    index,
                    file,
                    name.start_byte(),
                    name.end_byte(),
                )
                .map(|entry| hierarchy::item_from_entry(&entry))
                .or_else(|| {
                    Some(HierarchyItem {
                        name: node_text(name, text).to_string(),
                        kind: SymbolKind::Function,
                        package: package_of(tree, text),
                        file: file.to_string(),
                        start_byte: name.start_byte(),
                        end_byte: name.end_byte(),
                    })
                });
            }
            _ => node = node.parent()?,
        }
    }
}

pub fn incoming_calls<F>(
    index: &dyn InferenceIndex,
    target: &HierarchyItem,
    refs: Vec<Def>,
    mut parse_file: F,
) -> Vec<IncomingCall>
where
    F: FnMut(&str) -> Option<(String, Tree)>,
{
    let mut grouped: BTreeMap<(String, usize, usize), (HierarchyItem, Vec<Def>)> = BTreeMap::new();
    for r in refs {
        if r.file == target.file
            && r.start_byte == target.start_byte
            && r.end_byte == target.end_byte
        {
            continue;
        }
        let Some((text, tree)) = parse_file(&r.file) else {
            continue;
        };
        let caller = if is_java_file(&r.file) {
            enclosing_callable_item(index, &r.file, &tree, &text, r.start_byte)
        } else {
            hierarchy::enclosing_callable_item(index, &r.file, &tree, &text, r.start_byte)
        };
        let Some(caller) = caller else {
            continue;
        };
        grouped
            .entry((caller.file.clone(), caller.start_byte, caller.end_byte))
            .or_insert_with(|| (caller, Vec::new()))
            .1
            .push(r);
    }
    grouped
        .into_values()
        .map(|(from, ranges)| IncomingCall { from, ranges })
        .collect()
}

pub fn outgoing_calls(
    index: &dyn InferenceIndex,
    file: &str,
    tree: &Tree,
    text: &str,
    callable: &HierarchyItem,
) -> Vec<OutgoingCall> {
    let Some(decl) = java_declaration_node_for_range(tree, callable.start_byte, callable.end_byte)
    else {
        return Vec::new();
    };
    let mut grouped: BTreeMap<(String, usize, usize), (HierarchyItem, Vec<Def>)> = BTreeMap::new();
    visit_java_identifiers(decl, &mut |ident| {
        if ident.start_byte() == callable.start_byte && ident.end_byte() == callable.end_byte {
            return;
        }
        if !is_java_call_name(ident) {
            return;
        }
        for def in goto_definition(index, file, text, tree, ident.start_byte()) {
            let Some(entry) =
                hierarchy::entry_for_name_range(index, &def.file, def.start_byte, def.end_byte)
            else {
                continue;
            };
            if entry.sym.kind != SymbolKind::Function {
                continue;
            }
            let item = hierarchy::item_from_entry(&entry);
            grouped
                .entry((item.file.clone(), item.start_byte, item.end_byte))
                .or_insert_with(|| (item, Vec::new()))
                .1
                .push(Def {
                    file: file.to_string(),
                    start_byte: ident.start_byte(),
                    end_byte: ident.end_byte(),
                });
        }
    });
    grouped
        .into_values()
        .map(|(to, ranges)| OutgoingCall { to, ranges })
        .collect()
}

fn java_declaration_node_for_range(tree: &Tree, start: usize, end: usize) -> Option<Node<'_>> {
    let mut node = tree
        .root_node()
        .named_descendant_for_byte_range(start, end)?;
    loop {
        if matches!(
            node.kind(),
            "method_declaration" | "constructor_declaration"
        ) {
            return Some(node);
        }
        node = node.parent()?;
    }
}

fn visit_java_identifiers(node: Node<'_>, f: &mut impl FnMut(Node<'_>)) {
    if is_identifier(node) {
        f(node);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        visit_java_identifiers(child, f);
    }
}

fn is_java_call_name(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    parent.kind() == "method_invocation"
        && parent.child_by_field_name("name").is_some_and(|name| {
            name.start_byte() == node.start_byte() && name.end_byte() == node.end_byte()
        })
}

fn is_java_file(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        == Some("java")
}

fn local_definition(file: &str, src: &str, usage: Node<'_>) -> Option<Def> {
    let decl = local_declaration_node(src, usage)?;
    Some(Def {
        file: file.to_string(),
        start_byte: decl.start_byte(),
        end_byte: decl.end_byte(),
    })
}

fn local_declaration_node<'a>(src: &str, usage: Node<'a>) -> Option<Node<'a>> {
    if usage.kind() != "identifier"
        || is_java_call_name(usage)
        || usage.parent().is_some_and(|parent| {
            parent.kind() == "field_access" && parent.child_by_field_name("field") == Some(usage)
                || parent.kind() == "method_reference"
                    && parent.named_child(parent.named_child_count().saturating_sub(1))
                        == Some(usage)
        })
    {
        return None;
    }
    let name = node_text(usage, src);
    let mut current = usage;
    while let Some(parent) = current.parent() {
        if let Some(def) = local_declaration_from_parent(src, parent, current, name) {
            return Some(def);
        }
        current = parent;
    }
    None
}

fn type_parameter_in_scope(usage: Node<'_>, src: &str) -> bool {
    if usage.kind() != "type_identifier" {
        return false;
    }
    let name = node_text(usage, src);
    let mut current = usage.parent();
    while let Some(parent) = current {
        let mut cursor = parent.walk();
        for child in parent.named_children(&mut cursor) {
            if child.kind() != "type_parameters" || child.start_byte() > usage.start_byte() {
                continue;
            }
            if type_parameters_contain(child, src, name) {
                return true;
            }
        }
        current = parent.parent();
    }
    false
}

fn type_parameters_contain(node: Node<'_>, src: &str, name: &str) -> bool {
    if node.kind() == "type_identifier" && node_text(node, src) == name {
        return true;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if type_parameters_contain(child, src, name) {
            return true;
        }
    }
    false
}

fn local_declaration_from_parent<'a>(
    src: &str,
    parent: Node<'a>,
    current: Node<'a>,
    name: &str,
) -> Option<Node<'a>> {
    match parent.kind() {
        "variable_declarator"
            if parent
                .parent()
                .is_some_and(|declaration| declaration.kind() == "local_variable_declaration")
                && parent.child_by_field_name("value") == Some(current) =>
        {
            // Local scope includes its own initializer; definite assignment is separate.
            name_field(parent).filter(|declaration| node_text(*declaration, src) == name)
        }
        "block" => {
            let mut cursor = parent.walk();
            let siblings = parent.named_children(&mut cursor).collect::<Vec<_>>();
            let current_ix = siblings.iter().position(|child| *child == current)?;
            for sibling in siblings[..current_ix].iter().rev() {
                if let Some(def) = declaration_in_statement(src, *sibling, name) {
                    return Some(def);
                }
            }
            None
        }
        "method_declaration" | "constructor_declaration" => {
            if parent.child_by_field_name("body") == Some(current) {
                return declaration_in_parameters(src, parent, name);
            }
            None
        }
        "lambda_expression" => {
            if parent.child_by_field_name("body") == Some(current) {
                return declaration_in_lambda_parameters(src, parent, name);
            }
            None
        }
        "catch_clause" => {
            if parent.child_by_field_name("body") == Some(current) {
                return declaration_in_catch_parameter(src, parent, name);
            }
            None
        }
        "try_with_resources_statement" => {
            if parent.child_by_field_name("body") == Some(current) {
                return declaration_in_resources_before(
                    src,
                    parent.child_by_field_name("resources")?,
                    name,
                    current.start_byte(),
                );
            }
            None
        }
        "resource_specification" => {
            declaration_in_resources_before(src, parent, name, current.start_byte())
        }
        "resource" => {
            // A resource is in scope in its own initializer; definite assignment is separate.
            if parent.child_by_field_name("value") == Some(current) {
                return declaration_in_statement(src, parent, name);
            }
            None
        }
        "for_statement" => {
            if let Some(body) = parent.child_by_field_name("body") {
                if body == current || body.start_byte() <= current.start_byte() {
                    return declaration_in_for_init(src, parent, name);
                }
            }
            None
        }
        "enhanced_for_statement" => {
            if let Some(body) = parent.child_by_field_name("body") {
                if body == current || body.start_byte() <= current.start_byte() {
                    return name_field(parent).filter(|decl| node_text(*decl, src) == name);
                }
            }
            None
        }
        _ => None,
    }
}

fn declaration_in_resources_before<'a>(
    src: &str,
    resources: Node<'a>,
    name: &str,
    before: usize,
) -> Option<Node<'a>> {
    let mut found = None;
    let mut cursor = resources.walk();
    for resource in resources.named_children(&mut cursor) {
        if resource.start_byte() >= before {
            break;
        }
        if let Some(declaration) = declaration_in_statement(src, resource, name) {
            found = Some(declaration);
        }
    }
    found
}

fn declaration_in_statement<'a>(src: &str, node: Node<'a>, name: &str) -> Option<Node<'a>> {
    match node.kind() {
        "local_variable_declaration" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() != "variable_declarator" {
                    continue;
                }
                let decl = name_field(child)?;
                if node_text(decl, src) == name {
                    return Some(decl);
                }
            }
            None
        }
        "resource" => {
            let decl = name_field(node)?;
            (node_text(decl, src) == name).then_some(decl)
        }
        _ => None,
    }
}

fn declaration_in_parameters<'a>(src: &str, node: Node<'a>, name: &str) -> Option<Node<'a>> {
    let params = node.child_by_field_name("parameters")?;
    let mut cursor = params.walk();
    for child in params.named_children(&mut cursor) {
        if child.kind() != "formal_parameter" {
            continue;
        }
        let decl = name_field(child)?;
        if node_text(decl, src) == name {
            return Some(decl);
        }
    }
    None
}

fn declaration_in_lambda_parameters<'a>(src: &str, node: Node<'a>, name: &str) -> Option<Node<'a>> {
    let params = node.child_by_field_name("parameters")?;
    let mut cursor = params.walk();
    for child in params.named_children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                if node_text(child, src) == name {
                    return Some(child);
                }
            }
            "formal_parameter" => {
                let decl = name_field(child)?;
                if node_text(decl, src) == name {
                    return Some(decl);
                }
            }
            _ => {}
        }
    }
    None
}

fn declaration_in_catch_parameter<'a>(src: &str, node: Node<'a>, name: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "catch_formal_parameter" {
            continue;
        }
        let decl = name_field(child)?;
        if node_text(decl, src) == name {
            return Some(decl);
        }
    }
    None
}

fn declaration_in_for_init<'a>(src: &str, node: Node<'a>, name: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "local_variable_declaration" {
            continue;
        }
        return declaration_in_statement(src, child, name);
    }
    None
}

fn receiver_member_defs(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    usage: Node<'_>,
    arguments: Option<&ktcore::members::CallArguments>,
    initializer_depth: usize,
) -> Option<Vec<Def>> {
    let start = std::time::Instant::now();
    let parent = usage.parent()?;
    let kind = parent.kind();
    if kind == "method_invocation" && parent.child_by_field_name("name") == Some(usage) {
        let selection = match arguments {
            Some(arguments) => java_member_call_candidates_with_arguments(
                index,
                file,
                src,
                tree,
                parent,
                arguments,
                initializer_depth,
            ),
            None => java_member_call_candidates(index, file, src, tree, parent, initializer_depth),
        };
        if let Some(selection) = selection {
            return Some(
                selection
                    .navigation_candidates()
                    .iter()
                    .filter_map(|candidate| candidate.definition())
                    .collect(),
            );
        }
        return parent.child_by_field_name("object").map(|_| Vec::new());
    }
    let object = match kind {
        "field_access" if parent.child_by_field_name("field") == Some(usage) => {
            parent.child_by_field_name("object")
        }
        _ => None,
    }?;
    let name = node_text(usage, src);
    let Some(recv) = receiver_type(index, file, src, tree, object, initializer_depth) else {
        return Some(Vec::new());
    };
    let members = java_member_entries(index, &recv, name, SymbolKind::Property);
    let mut defs = members
        .iter()
        .map(|entry| Def {
            file: entry.path.to_string(),
            start_byte: entry.sym.start_byte,
            end_byte: entry.sym.end_byte,
        })
        .collect::<Vec<_>>();
    defs.sort();
    defs.dedup();
    if crate::trace::enabled() {
        crate::trace::span(
            "java.receiver_member",
            "java",
            start,
            serde_json::json!({
                "file": file,
                "receiver": recv.name(),
                "receiverPackage": recv.package(),
                "member": name,
                "kind": kind,
                "candidateMembers": members.len(),
                "count": defs.len(),
            }),
        );
    }
    Some(defs)
}

const JAVA_SUPERTYPE_WALK_CAP: usize = 16;

fn java_member_entries(
    index: &dyn InferenceIndex,
    recv: &Type,
    name: &str,
    expected_kind: SymbolKind,
) -> Vec<Entry> {
    let mut out = Vec::new();
    let _ = infer::walk_supertypes(index, recv, JAVA_SUPERTYPE_WALK_CAP, |current| {
        let Some(cur) =
            infer::receiver_root_key(index, current).or_else(|| current.name().map(str::to_string))
        else {
            return ControlFlow::Continue(());
        };
        let cur_pkg = current.package();
        out.extend(
            index
                .members_of(&cur)
                .iter()
                .filter(|entry| entry.sym.container.as_deref() == Some(cur.as_str()))
                .filter(|entry| entry.sym.name == name)
                .filter(|entry| java_member_kind_matches(entry.sym.kind, expected_kind))
                .filter(|entry| cur_pkg.is_none_or(|pkg| entry.sym.package == pkg))
                .filter(|entry| ktcore::members::receiver_member_accessible(recv, entry))
                .cloned(),
        );
        ControlFlow::Continue(())
    });
    out.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then(a.sym.package.cmp(&b.sym.package))
            .then(a.sym.container.cmp(&b.sym.container))
            .then(a.sym.start_byte.cmp(&b.sym.start_byte))
            .then(a.sym.end_byte.cmp(&b.sym.end_byte))
    });
    out.dedup_by(|a, b| {
        a.path == b.path
            && a.sym.package == b.sym.package
            && a.sym.container == b.sym.container
            && a.sym.start_byte == b.sym.start_byte
            && a.sym.end_byte == b.sym.end_byte
    });
    out
}

fn narrow_java_entries_by_call_arity(src: &str, call: Node<'_>, entries: Vec<Entry>) -> Vec<Entry> {
    let Some(args) = java_argument_nodes(src, call) else {
        return entries;
    };
    if entries
        .iter()
        .any(|entry| entry.sym.arity.is_none() || entry.sym.min_arity.is_none())
    {
        return entries;
    }
    let arg_count = args.len();
    let compatible = entries
        .iter()
        .filter(|entry| java_entry_accepts_arg_count(entry, arg_count))
        .cloned()
        .collect::<Vec<_>>();
    if compatible.is_empty() {
        entries
    } else {
        compatible
    }
}

fn java_entry_accepts_arg_count(entry: &Entry, arg_count: usize) -> bool {
    let (Some(min), Some(max)) = (entry.sym.min_arity, entry.sym.arity) else {
        return false;
    };
    let min = min as usize;
    let max = max as usize;
    if entry.sym.has_vararg {
        arg_count >= max.saturating_sub(1)
    } else {
        (min..=max).contains(&arg_count)
    }
}

fn java_member_kind_matches(actual: SymbolKind, expected: SymbolKind) -> bool {
    actual == expected || expected == SymbolKind::Property && actual == SymbolKind::EnumEntry
}

fn java_same_pkg_supertype(
    index: &dyn InferenceIndex,
    sup: &str,
    cur_pkg: Option<&str>,
) -> Option<String> {
    match cur_pkg {
        Some(pkg)
            if index
                .lookup_type(sup)
                .iter()
                .any(|entry| entry.sym.package == pkg) =>
        {
            Some(pkg.to_string())
        }
        _ => None,
    }
}

fn receiver_type(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    object: Node<'_>,
    initializer_depth: usize,
) -> Option<Type> {
    if initializer_depth > infer::MAX_DEPTH {
        return None;
    }
    match object.kind() {
        "this" | "super" => {
            let receiver = infer::declared_type(
                index,
                &type_scope(tree, src).type_ref(
                    &enclosing_type_name(object, src)?,
                    false,
                    Vec::new(),
                ),
                &FileCtx::new(package_of(tree, src), inference_imports(tree, src)),
            );
            if object.kind() == "this" {
                return Some(receiver);
            }
            let mut supers = infer::direct_supertypes(index, &receiver);
            if supers.iter().any(|ty| ty.name().is_none()) {
                return None;
            }
            // Java's unqualified super dispatches through its class superclass, not an
            // implemented interface. Keep the normalized generic arguments from the core walk.
            supers.retain(|ty| {
                index.lookup_type(ty.name().unwrap()).iter().any(|entry| {
                    entry.sym.kind == SymbolKind::Class
                        && ty
                            .package()
                            .is_none_or(|package| package == entry.sym.package)
                        && ty.container() == entry.sym.container.as_deref()
                })
            });
            (supers.len() == 1).then(|| supers.pop().unwrap())
        }
        "identifier" => {
            if let Some(decl) = local_declaration_node(src, object)
                .or_else(|| same_file_field_declaration(src, object))
            {
                return type_of_value_declaration(index, file, src, tree, decl, initializer_depth);
            }
            receiver_type_from_type_identifier(index, file, src, tree, object)
        }
        "object_creation_expression" => {
            let ty = object.child_by_field_name("type")?;
            receiver_type_from_type_node(index, file, src, tree, ty)
        }
        "parenthesized_expression" => java_argument_type(
            index,
            file,
            src,
            tree,
            object.named_child(0)?,
            initializer_depth + 1,
        ),
        "array_creation_expression" => {
            if object.has_error() {
                return None;
            }
            let mut cursor = object.walk();
            let rank = object
                .children_by_field_name("dimensions", &mut cursor)
                .try_fold(0usize, |rank, dimensions| {
                    Some(rank + array_dimensions_rank(dimensions)?)
                })?;
            if rank == 0 || rank > infer::MAX_DEPTH {
                return None;
            }
            let mut ty = receiver_type_from_type_node(
                index,
                file,
                src,
                tree,
                object.child_by_field_name("type")?,
            )?;
            for _ in 0..rank {
                ty = Type::array(ty, false);
            }
            Some(ty)
        }
        "array_access" => {
            if object.has_error() {
                return None;
            }
            let subscript = java_argument_type(
                index,
                file,
                src,
                tree,
                object.child_by_field_name("index")?,
                initializer_depth + 1,
            )?;
            if subscript.package().is_some()
                || !matches!(subscript.name(), Some("int" | "byte" | "short" | "char"))
            {
                // shortcut: boxed or unknown subscripts need verified unboxing/integral inference.
                return None;
            }
            match receiver_type(
                index,
                file,
                src,
                tree,
                object.child_by_field_name("array")?,
                initializer_depth + 1,
            )? {
                Type::Array { component, .. } => Some(*component),
                _ => None,
            }
        }
        "field_access" => {
            let receiver = receiver_type(
                index,
                file,
                src,
                tree,
                object.child_by_field_name("object")?,
                initializer_depth + 1,
            )?;
            ktcore::members::java_array_member_type(
                &receiver,
                node_text(object.child_by_field_name("field")?, src),
                &ktcore::members::MemberUse::Value,
            )
        }
        "method_invocation" => {
            java_method_invocation_return_type(index, file, src, tree, object, initializer_depth)
        }
        _ => None,
    }
}

fn receiver_type_from_type_identifier(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    ident: Node<'_>,
) -> Option<Type> {
    let name = node_text(ident, src);
    if name.is_empty() {
        return None;
    }
    if let Some(package) = same_file_type_package(index, file, name)
        .or_else(|| visible_type_package(index, src, tree, name))
    {
        return Some(Type::class(name, Some(package)));
    }
    ktcore::jvm::visible_owner(
        index,
        name,
        &FileCtx::new(package_of(tree, src), inference_imports(tree, src)),
    )
}

/// Classify the call receiver once. Known values win over identically named types.
/// Missing authored fields are not proof that compiler-generated fields are absent.
fn java_member_receiver(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    call: Node<'_>,
    initializer_depth: usize,
) -> Option<(Type, ktcore::jvm::ReceiverKind)> {
    use ktcore::jvm::ReceiverKind;
    let Some(object) = call.child_by_field_name("object") else {
        let receiver = Type::class(enclosing_type_name(call, src)?, Some(package_of(tree, src)));
        return Some((receiver, implicit_receiver_kind(call, src)));
    };
    match object.kind() {
        "identifier" => {
            if let Some(declaration) = local_declaration_node(src, object) {
                return Some((
                    type_of_value_declaration(
                        index,
                        file,
                        src,
                        tree,
                        declaration,
                        initializer_depth,
                    )
                    .unwrap_or(Type::Unknown),
                    ReceiverKind::Value,
                ));
            }
            if let Some(declaration) = same_file_field_declaration(src, object) {
                let mut field = declaration;
                while field.kind() != "field_declaration" {
                    let Some(parent) = field.parent() else {
                        break;
                    };
                    field = parent;
                }
                let dispatch = crate::indexer::source_member_flags(field, src, true).dispatch();
                let kind = if dispatch == Some(ktcore::extensions::Dispatch::Static)
                    || implicit_receiver_kind(call, src) == ReceiverKind::Value
                {
                    ReceiverKind::Value
                } else {
                    ReceiverKind::Unknown
                };
                return Some((
                    type_of_value_declaration(
                        index,
                        file,
                        src,
                        tree,
                        declaration,
                        initializer_depth,
                    )
                    .unwrap_or(Type::Unknown),
                    kind,
                ));
            }
            let ctx = FileCtx::new(package_of(tree, src), inference_imports(tree, src));
            if let Some(owner) = enclosing_type_name(object, src) {
                let enclosing = infer::declared_type(
                    index,
                    &type_scope(tree, src).type_ref(&owner, false, Vec::new()),
                    &ctx,
                );
                let fields =
                    ktcore::members::candidates(index, &enclosing, node_text(object, src), &ctx)
                        .members
                        .into_iter()
                        .filter(|entry| entry.sym.kind == SymbolKind::Property)
                        .collect::<Vec<_>>();
                let mut complete = index.scope_is_complete();
                let _ =
                    infer::walk_supertypes(index, &enclosing, JAVA_SUPERTYPE_WALK_CAP, |current| {
                        complete &= current.name().is_some() && current.package().is_some();
                        ControlFlow::Continue(())
                    });
                if let [field] = fields.as_slice() {
                    let visible = matches!(
                        field.sym.source_member.visibility(),
                        Some(
                            ktcore::extensions::Visibility::Public
                                | ktcore::extensions::Visibility::Protected
                        )
                    ) || field.sym.source_member.visibility()
                        == Some(ktcore::extensions::Visibility::Package)
                        && field.sym.package == ctx.package;
                    let dispatch = field.sym.source_member.dispatch();
                    if complete
                        && visible
                        && !static_imports_of(tree, src).may_resolve(node_text(object, src))
                        && (dispatch == Some(ktcore::extensions::Dispatch::Static)
                            || dispatch == Some(ktcore::extensions::Dispatch::Instance)
                                && implicit_receiver_kind(call, src) == ReceiverKind::Value)
                    {
                        if let Some(value) = &field.sym.value_type {
                            let ty =
                                infer::declared_member_type(index, &enclosing, field, value, &ctx);
                            return Some((ty, ReceiverKind::Value));
                        }
                    }
                }
            }
            // No compiler/value-namespace closure is available here. Even a known class name
            // could denote an unresolved imported or generated field; do not prove Type.
            receiver_type_from_type_identifier(index, file, src, tree, object)
                .map(|receiver| (receiver, ReceiverKind::Unknown))
        }
        "object_creation_expression" | "method_invocation" => {
            receiver_type(index, file, src, tree, object, initializer_depth)
                .map(|receiver| (receiver, ReceiverKind::Value))
        }
        "this" | "super" => receiver_type(index, file, src, tree, object, initializer_depth)
            .map(|receiver| (receiver, implicit_receiver_kind(call, src))),
        _ => receiver_type(index, file, src, tree, object, initializer_depth)
            .map(|receiver| (receiver, ReceiverKind::Unknown)),
    }
}

fn implicit_receiver_kind(node: Node<'_>, src: &str) -> ktcore::jvm::ReceiverKind {
    let mut current = node.parent();
    while let Some(parent) = current {
        if matches!(
            parent.kind(),
            "method_declaration" | "constructor_declaration"
        ) {
            return if crate::indexer::source_member_flags(parent, src, true).dispatch()
                == Some(ktcore::extensions::Dispatch::Instance)
            {
                ktcore::jvm::ReceiverKind::Value
            } else {
                ktcore::jvm::ReceiverKind::Unknown
            };
        }
        if matches!(
            parent.kind(),
            "class_declaration"
                | "interface_declaration"
                | "record_declaration"
                | "enum_declaration"
                | "annotation_type_declaration"
        ) {
            break;
        }
        current = parent.parent();
    }
    ktcore::jvm::ReceiverKind::Unknown
}

fn java_member_call_candidates<'a>(
    index: &'a dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    call: Node<'_>,
    initializer_depth: usize,
) -> Option<ktcore::jvm::CallSelection<'a>> {
    java_member_call_candidates_with_arguments(
        index,
        file,
        src,
        tree,
        call,
        &java_call_arguments(index, file, src, tree, call, initializer_depth),
        initializer_depth,
    )
}

fn java_member_call_candidates_with_arguments<'a>(
    index: &'a dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    call: Node<'_>,
    arguments: &ktcore::members::CallArguments,
    initializer_depth: usize,
) -> Option<ktcore::jvm::CallSelection<'a>> {
    if call.kind() != "method_invocation" {
        return None;
    }
    let name = node_text(call.child_by_field_name("name")?, src);
    let object = call.child_by_field_name("object");
    let receiver = match object {
        Some(object) => receiver_type(index, file, src, tree, object, initializer_depth),
        None => enclosing_type_name(call, src)
            .map(|name| Type::class(name, Some(package_of(tree, src)))),
    };
    let receiver = receiver.or_else(|| {
        java_member_receiver(index, file, src, tree, call, initializer_depth).map(|(ty, _)| ty)
    })?;
    let mut candidates = ktcore::jvm::member_candidates(index, &receiver, name);
    let ctx = FileCtx::new(package_of(tree, src), inference_imports(tree, src));
    let selection = if candidates.iter().any(|candidate| {
        matches!(
            candidate,
            ktcore::jvm::CallableCandidate::Jvm {
                owner: ktcore::jvm::JvmOwner::Compiled { .. },
                ..
            }
        )
    }) {
        let (verified_receiver, kind) =
            java_member_receiver(index, file, src, tree, call, initializer_depth)?;
        if verified_receiver != receiver {
            candidates = ktcore::jvm::member_candidates(index, &verified_receiver, name);
        }
        ktcore::jvm::select_call_for_receiver_kind(
            index,
            &verified_receiver,
            candidates,
            name,
            arguments,
            &ctx,
            kind,
        )
    } else {
        ktcore::jvm::select_call(index, &receiver, candidates, name, arguments, &ctx)
    };
    // An absent implicit member leaves the existing import/name lookup available.
    (object.is_some() || !selection.candidates.is_empty()).then_some(selection)
}

fn java_call_arguments(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    call: Node<'_>,
    initializer_depth: usize,
) -> ktcore::members::CallArguments {
    let nodes = (!call.has_error())
        .then(|| java_argument_nodes(src, call))
        .flatten();
    ktcore::members::CallArguments {
        count: nodes.as_ref().map(Vec::len),
        types: nodes
            .into_iter()
            .flatten()
            .map(|node| {
                java_argument_type(index, file, src, tree, node, initializer_depth)
                    .unwrap_or(Type::Unknown)
            })
            .collect(),
        names: None,
        has_spread: false,
        trailing_lambda: false,
    }
}

fn java_argument_type(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    node: Node<'_>,
    initializer_depth: usize,
) -> Option<Type> {
    if initializer_depth > 0 && node.kind() == "identifier" {
        let declaration =
            local_declaration_node(src, node).or_else(|| same_file_field_declaration(src, node))?;
        return type_of_value_declaration(index, file, src, tree, declaration, initializer_depth);
    }
    if initializer_depth > 0
        && node.kind() == "object_creation_expression"
        && child_of_kind(node, "class_body").is_some()
    {
        // shortcut: anonymous initializer types stay unknown until their own members are indexed.
        return None;
    }
    if let Some(ty) = receiver_type(index, file, src, tree, node, initializer_depth) {
        return Some(ty);
    }
    if node.kind() == "parenthesized_expression" {
        let child = node.named_child(0)?;
        return java_argument_type(index, file, src, tree, child, initializer_depth);
    }
    if matches!(
        node.kind(),
        "string_literal"
            | "character_literal"
            | "true"
            | "false"
            | "decimal_floating_point_literal"
            | "hex_floating_point_literal"
            | "decimal_integer_literal"
            | "hex_integer_literal"
            | "octal_integer_literal"
            | "binary_integer_literal"
    ) {
        let ty = java_expression_type(index, file, src, tree, node, initializer_depth)?;
        return Some(Type::class(ty.name, ty.package));
    }
    None
}

pub(crate) fn jvm_call_selection_at<'a>(
    index: &'a dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    offset: usize,
) -> Option<ktcore::jvm::CallSelection<'a>> {
    let usage = identifier_at(tree, offset)?;
    let call = usage.parent()?;
    if call.kind() != "method_invocation" || call.child_by_field_name("name") != Some(usage) {
        return None;
    }
    java_member_call_candidates(index, file, src, tree, call, 0)
}

pub fn jvm_member_at(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    offset: usize,
) -> Option<ktcore::jvm::ResolvedMember> {
    let candidates = jvm_call_selection_at(index, file, src, tree, offset)?;
    let [candidate] = candidates.navigation_candidates() else {
        return None;
    };
    candidate.resolved(index)
}

/// Return projected signatures and whether unqualified authored/global fallback is allowed.
pub fn jvm_call_signatures(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    call: Node<'_>,
) -> (Vec<crate::signature::Signature>, bool) {
    let unqualified =
        call.kind() == "method_invocation" && call.child_by_field_name("object").is_none();
    let mut arguments = java_call_arguments(index, file, src, tree, call, 0);
    arguments.count = None;
    let Some(selection) =
        java_member_call_candidates_with_arguments(index, file, src, tree, call, &arguments, 0)
    else {
        return (Vec::new(), unqualified);
    };
    let allow_fallback = unqualified
        && !(selection.consumable_candidates().is_empty()
            && selection.candidates.iter().any(|candidate| {
                matches!(
                    candidate,
                    ktcore::jvm::CallableCandidate::Jvm {
                        owner: ktcore::jvm::JvmOwner::Compiled { .. },
                        ..
                    }
                )
            }));
    let signatures = selection
        .consumable_candidates()
        .iter()
        .copied()
        .filter(|candidate| matches!(candidate, ktcore::jvm::CallableCandidate::Jvm { .. }))
        .map(crate::signature::signature_for_callable)
        .collect();
    (signatures, allow_fallback)
}

fn java_method_invocation_return_type(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    call: Node<'_>,
    initializer_depth: usize,
) -> Option<Type> {
    let name = call.child_by_field_name("name")?;
    let symbol = node_text(name, src);
    if symbol.is_empty() {
        return None;
    }
    let arguments = java_call_arguments(index, file, src, tree, call, initializer_depth);
    if symbol == "clone" && call.child_by_field_name("type_arguments").is_none() {
        if let Some(object) = call.child_by_field_name("object") {
            if let Some(receiver) =
                receiver_type(index, file, src, tree, object, initializer_depth + 1)
            {
                if let Some(result) = ktcore::members::java_array_member_type(
                    &receiver,
                    symbol,
                    &ktcore::members::MemberUse::Call(arguments.clone()),
                ) {
                    return Some(result);
                }
            }
        }
    }
    if let Some(candidates) = java_member_call_candidates_with_arguments(
        index,
        file,
        src,
        tree,
        call,
        &arguments,
        initializer_depth,
    ) {
        if candidates.applicability == ktcore::members::Applicability::Rejected {
            return None;
        }
        if candidates
            .candidates
            .iter()
            .any(|candidate| matches!(candidate, ktcore::jvm::CallableCandidate::Jvm { .. }))
        {
            let [candidate] = candidates.candidates.as_slice() else {
                return None;
            };
            if !candidate.accepts_count(java_argument_nodes(src, call).map(|args| args.len())) {
                return None;
            }
            if matches!(
                candidate,
                ktcore::jvm::CallableCandidate::Jvm {
                    owner: ktcore::jvm::JvmOwner::Compiled { .. },
                    ..
                }
            ) && candidates.applicability != ktcore::members::Applicability::Applicable
            {
                return None;
            }
            // Compiled/lowered array mappings are outside the currently admitted JVM slice.
            return ktcore::jvm::projected_return_type_in_context(
                index,
                *candidate,
                &FileCtx::new(package_of(tree, src), inference_imports(tree, src)),
            )
            .filter(|ty| !matches!(ty, Type::Array { .. }));
        }
    }
    let package = package_of(tree, src);
    let imports = imports_of(tree, src);
    let entries = java_call_entries_with_arguments(
        index,
        file,
        src,
        tree,
        call,
        name,
        symbol,
        &package,
        &imports,
        &arguments,
        initializer_depth,
    );
    if entries.is_empty() {
        return None;
    }
    if entries.iter().any(|entry| {
        entry.sym.arity.is_none()
            || entry.sym.min_arity.is_none()
            || entry.sym.has_vararg
            || entry.sym.return_type.is_none()
    }) {
        return None;
    }
    let args = java_argument_nodes(src, call)?;
    let arg_count = args.len();
    let receiver = match call.child_by_field_name("object") {
        Some(object) => receiver_type(index, file, src, tree, object, initializer_depth)
            .unwrap_or(Type::Unknown),
        None => enclosing_type_name(call, src)
            .map(|name| Type::class(name, Some(package.clone())))
            .unwrap_or(Type::Unknown),
    };
    let ctx = FileCtx::new(package, inference_imports(tree, src));
    let mut result: Option<Type> = None;
    for entry in entries {
        let min = entry.sym.min_arity.expect("guarded above") as usize;
        let max = entry.sym.arity.expect("guarded above") as usize;
        if !(min..=max).contains(&arg_count) {
            continue;
        }
        let declared = entry.sym.return_type.as_ref()?;
        let ty =
            infer::declared_member_call_type(index, &receiver, &entry, declared, &arguments, &ctx);
        if !matches!(ty, Type::Array { .. })
            && ty
                .name()
                .is_none_or(|name| name == "void" || is_java_primitive_type(name))
        {
            return None;
        }
        match &result {
            None => result = Some(ty.clone()),
            Some(prev) if *prev == ty => {}
            Some(_) => return None,
        }
    }
    result
}

fn same_file_field_declaration<'a>(src: &str, usage: Node<'a>) -> Option<Node<'a>> {
    let name = node_text(usage, src);
    let type_name = enclosing_type_name(usage, src)?;
    let mut current = usage.parent();
    while let Some(parent) = current {
        let matches_type = matches!(
            parent.kind(),
            "class_declaration"
                | "record_declaration"
                | "interface_declaration"
                | "enum_declaration"
                | "annotation_type_declaration"
        ) && name_field(parent)
            .is_some_and(|decl| node_text(decl, src) == type_name);
        if matches_type {
            let mut cursor = parent.walk();
            for child in parent.named_children(&mut cursor) {
                if parent.kind() == "record_declaration" && child.kind() == "formal_parameters" {
                    let mut c2 = child.walk();
                    for param in child.named_children(&mut c2) {
                        if param.kind() != "formal_parameter" {
                            continue;
                        }
                        let id = name_field(param)?;
                        if node_text(id, src) == name {
                            return Some(id);
                        }
                    }
                    continue;
                }
                if child.kind() != "class_body"
                    && child.kind() != "interface_body"
                    && child.kind() != "enum_body"
                    && child.kind() != "annotation_type_body"
                {
                    continue;
                }
                let mut c2 = child.walk();
                for member in child.named_children(&mut c2) {
                    if member.kind() != "field_declaration" {
                        continue;
                    }
                    let mut c3 = member.walk();
                    for decl in member.named_children(&mut c3) {
                        if decl.kind() != "variable_declarator" {
                            continue;
                        }
                        let id = name_field(decl)?;
                        if node_text(id, src) == name {
                            return Some(id);
                        }
                    }
                }
            }
        }
        current = parent.parent();
    }
    None
}

pub(crate) fn type_of_value_declaration(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    decl: Node<'_>,
    initializer_depth: usize,
) -> Option<Type> {
    let parent = decl.parent()?;
    let type_node = value_declaration_type_node(decl)?;
    if type_node.kind() == "type_identifier" && node_text(type_node, src) == "var" {
        if initializer_depth >= infer::MAX_DEPTH {
            // shortcut: valid deeper var aliases stay Unknown until initializer expansion is iterative.
            return None;
        }
        if parent.kind() == "enhanced_for_statement" {
            if parent.child_by_field_name("dimensions").is_some() {
                return None;
            }
            let value = parent.child_by_field_name("value")?;
            if value.has_error() {
                return None;
            }
            return enhanced_for_element_type(index, file, src, tree, value, initializer_depth + 1);
        }
        let local = parent.kind() == "resource"
            || parent.kind() == "variable_declarator"
                && parent
                    .parent()
                    .is_some_and(|declaration| declaration.kind() == "local_variable_declaration");
        if !local || parent.child_by_field_name("dimensions").is_some() {
            return None;
        }
        let initializer = parent.child_by_field_name("value")?;
        if initializer.has_error() || initializer_uses_declaration(src, initializer, decl) {
            return None;
        }
        return java_argument_type(index, file, src, tree, initializer, initializer_depth + 1);
    }
    let mut ty = receiver_type_from_type_node(index, file, src, tree, type_node)?;
    if let Some(dimensions) = parent.child_by_field_name("dimensions") {
        let rank = array_dimensions_rank(dimensions)?;
        if rank > infer::MAX_DEPTH {
            return None;
        }
        for _ in 0..rank {
            ty = Type::array(ty, false);
        }
    }
    Some(ty)
}

fn value_declaration_type_node(declaration: Node<'_>) -> Option<Node<'_>> {
    let parent = declaration.parent()?;
    match parent.kind() {
        "formal_parameter" | "catch_formal_parameter" | "resource" | "enhanced_for_statement" => {
            parent.child_by_field_name("type")
        }
        "variable_declarator" => parent.parent()?.child_by_field_name("type"),
        _ => None,
    }
}

fn enhanced_for_element_type(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    mut value: Node<'_>,
    initializer_depth: usize,
) -> Option<Type> {
    while value.kind() == "parenthesized_expression" {
        value = value.named_child(0)?;
    }
    if value.has_error() {
        return None;
    }
    let receiver = java_argument_type(index, file, src, tree, value, initializer_depth)?;
    if let Type::Array { component, .. } = receiver {
        return Some(*component);
    }
    let mut result: Option<Type> = None;
    let mut incomplete = false;
    let _ = infer::walk_supertypes(index, &receiver, JAVA_SUPERTYPE_WALK_CAP, |current| {
        if !current.is_member_receiver() {
            incomplete = true;
            return ControlFlow::Break(());
        }
        if current.name() == Some("Iterable")
            && current.package() == Some("java.lang")
            && current.container().is_none()
        {
            match current.args() {
                [element]
                    if element.projection() == crate::types::Projection::Invariant
                        && element.is_member_receiver() =>
                {
                    if result.as_ref().is_some_and(|previous| previous != element) {
                        incomplete = true;
                        return ControlFlow::Break(());
                    }
                    result = Some(element.clone());
                }
                // shortcut: raw and captured iterable elements stay Unknown until capture types are represented.
                _ => {
                    incomplete = true;
                    return ControlFlow::Break(());
                }
            }
        }
        ControlFlow::Continue(())
    });
    if incomplete {
        None
    } else {
        result
    }
}

fn initializer_uses_declaration(src: &str, initializer: Node<'_>, declaration: Node<'_>) -> bool {
    let mut cursor = initializer.walk();
    loop {
        let node = cursor.node();
        if node.kind() == "identifier"
            && node_text(node, src) == node_text(declaration, src)
            && local_declaration_node(src, node) == Some(declaration)
        {
            return true;
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return false;
            }
        }
    }
}

fn receiver_type_from_type_node(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    type_node: Node<'_>,
) -> Option<Type> {
    let file_scope = type_scope(tree, src);
    let scoped = java_declaration_scope(type_node, src, &file_scope);
    let ty = type_ref_from_node(type_node, src, scoped.as_ref().unwrap_or(&file_scope))?;
    Some(infer::declared_type_in_scope(
        index,
        &ty,
        &FileCtx::new(package_of(tree, src), inference_imports(tree, src)).with_file(file),
        &type_parameters_in_scope(type_node, src, tree),
    ))
}

fn same_file_type_package(
    index: &dyn InferenceIndex,
    file: &str,
    type_name: &str,
) -> Option<String> {
    index.entries_for_file(file).into_iter().find_map(|entry| {
        (entry.sym.name == type_name && entry.sym.kind.is_type_like())
            .then(|| entry.sym.package.clone())
    })
}

fn visible_type_package(
    index: &dyn InferenceIndex,
    src: &str,
    tree: &Tree,
    type_name: &str,
) -> Option<String> {
    let package = package_of(tree, src);
    let imports = imports_of(tree, src);
    index
        .lookup_type(type_name)
        .into_iter()
        .find(|entry| is_visible(&package, &imports, &entry.sym))
        .map(|entry| entry.sym.package.clone())
}

fn enclosing_type_name(node: Node<'_>, src: &str) -> Option<String> {
    let mut current = node.parent();
    while let Some(parent) = current {
        match parent.kind() {
            "class_declaration"
            | "record_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "annotation_type_declaration" => {
                let name = name_field(parent)?;
                return Some(node_text(name, src).to_string());
            }
            _ => current = parent.parent(),
        }
    }
    None
}

fn qualified_member_defs(
    index: &dyn InferenceIndex,
    src: &str,
    tree: &Tree,
    usage: Node<'_>,
) -> Option<Vec<Def>> {
    let parent = usage.parent()?;
    if parent.kind() != "field_access" || parent.child_by_field_name("field") != Some(usage) {
        return None;
    }
    let object = parent.child_by_field_name("object")?;
    if !is_identifier(object) {
        return Some(Vec::new());
    }

    let field_name = node_text(usage, src);
    let owner_name = node_text(object, src);
    let package = package_of(tree, src);
    let imports = imports_of(tree, src);
    let mut owners = index
        .lookup_by_name(owner_name)
        .iter()
        .filter(|entry| entry.sym.kind.is_type_like() && is_visible(&package, &imports, &entry.sym))
        .collect::<Vec<_>>();
    owners.sort_by(|a, b| {
        a.sym
            .package
            .cmp(&b.sym.package)
            .then(a.path.cmp(&b.path))
            .then(a.sym.start_byte.cmp(&b.sym.start_byte))
    });
    owners.dedup_by(|a, b| {
        a.sym.package == b.sym.package
            && a.sym.name == b.sym.name
            && a.sym.start_byte == b.sym.start_byte
            && a.path == b.path
    });

    let mut defs = Vec::new();
    for owner in owners {
        let members = index
            .members_of(&owner.sym.name)
            .iter()
            .filter(|member| member.sym.name == field_name)
            .filter(|member| member.sym.container.as_deref() == Some(owner.sym.name.as_str()))
            .filter(|member| member.sym.package == owner.sym.package)
            .cloned()
            .collect::<Vec<_>>();
        let members = narrow_java_entries_by_field_access_arity(src, parent, members);
        for member in members {
            defs.push(Def {
                file: member.path.to_string(),
                start_byte: member.sym.start_byte,
                end_byte: member.sym.end_byte,
            });
        }
    }
    defs.sort();
    defs.dedup();
    Some(defs)
}

fn narrow_java_entries_by_field_access_arity(
    src: &str,
    field_access: Node<'_>,
    entries: Vec<Entry>,
) -> Vec<Entry> {
    let Some(parent) = field_access.parent() else {
        return entries;
    };
    if parent.kind() != "method_invocation"
        || parent.child_by_field_name("object") != Some(field_access)
    {
        return entries;
    }
    narrow_java_entries_by_call_arity(src, parent, entries)
}

/// Whether `entry` is visible from `file_package` given the file's explicit imports.
pub(crate) fn is_visible(file_package: &str, imports: &[Import], sym: &IndexedSymbol) -> bool {
    let visibility = language::NameVisibility::for_java_imports(file_package, imports);
    is_visible_with_facts(file_package, imports, &visibility, sym)
}

fn is_visible_with_facts(
    file_package: &str,
    imports: &[Import],
    visibility: &language::NameVisibility,
    sym: &IndexedSymbol,
) -> bool {
    // Same package is always visible.
    if sym.package == file_package && !sym.package.is_empty() {
        return true;
    }
    // java.lang is implicitly imported.
    if sym.package == "java.lang" {
        return true;
    }
    if visibility.is_exact_import_visible(&sym.package, &sym.name)
        || visibility.is_star_import_visible(&sym.package)
    {
        return true;
    }
    // Imported nested types: `import pkg.Outer.Inner` makes `Inner` visible.
    for imp in imports {
        if !imp.wildcard
            && sym.container.as_deref().is_some()
            && Some(sym.name.as_str()) == imp.simple_name()
            && sym
                .container
                .as_ref()
                .is_some_and(|container| imp.package.ends_with(&format!(".{container}")))
        {
            let owner_package_len = imp.package.len() - sym.container.as_ref().unwrap().len() - 1;
            if sym.package == imp.package[..owner_package_len] {
                return true;
            }
        }
    }
    false
}

/// Whether `node` is the `name` field of a declaration, i.e. the cursor is on its own definition.
fn is_declaration_name(node: Node) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    let declaration_kinds = [
        "class_declaration",
        "interface_declaration",
        "enum_declaration",
        "record_declaration",
        "annotation_type_declaration",
        "method_declaration",
        "annotation_type_element_declaration",
        "constructor_declaration",
        "variable_declarator",
        "enum_constant",
        "formal_parameter",
        "resource",
    ];
    if !declaration_kinds.contains(&parent.kind()) {
        return false;
    }
    name_field(parent) == Some(node)
}

/// Java keywords and reserved literals that cannot be used as identifiers.
const JAVA_KEYWORDS: &[&str] = &[
    "abstract",
    "assert",
    "boolean",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "class",
    "const",
    "continue",
    "default",
    "do",
    "double",
    "else",
    "enum",
    "extends",
    "final",
    "finally",
    "float",
    "for",
    "goto",
    "if",
    "implements",
    "import",
    "instanceof",
    "int",
    "interface",
    "long",
    "native",
    "new",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "short",
    "static",
    "strictfp",
    "super",
    "switch",
    "synchronized",
    "this",
    "throw",
    "throws",
    "transient",
    "try",
    "void",
    "volatile",
    "while",
    "true",
    "false",
    "null",
    "var",
    "record",
];

/// Whether `name` is a legal Java identifier and not a keyword or reserved literal.
pub(crate) fn is_valid_identifier(name: &str) -> bool {
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
    !JAVA_KEYWORDS.contains(&name)
}

/// `textDocument/completion` for callers that only need shaped Java results.
pub fn completion(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    offset: usize,
    snippets_supported: bool,
) -> Option<ShapedCompletions> {
    let query = completion_query(index, file, src, tree, offset, complete::RESULT_CAP)?;
    let mut shaped = complete::shape(
        query.context,
        &query.prefix,
        query.candidates,
        snippets_supported,
    );
    if let Some((sorted_imports, anchor)) = query.layout.as_ref() {
        complete::resolve_auto_import_lines(&mut shaped, sorted_imports, *anchor);
    }
    (!shaped.items.is_empty()).then_some(shaped)
}

/// Normalize Java syntax and receiver types for the shared completion candidate and shaping flow.
pub fn completion_query(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    offset: usize,
    max_candidates: usize,
) -> Option<CompletionQuery> {
    let context = completion_context(tree, src, offset);
    if context == complete::CompletionContext::AfterDot {
        let (prefix, mut synthetic, syn_offset) = complete::dot_recovery(src, offset)?;
        let selector_end = syn_offset
            + synthetic[syn_offset..]
                .chars()
                .take_while(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '$'))
                .map(char::len_utf8)
                .sum::<usize>();
        // Preserve an authored call, including a selector suffix to the right of the cursor.
        let force_call = !synthetic[selector_end..].trim_start().starts_with('(');
        if force_call {
            synthetic.insert_str(selector_end, "()");
        }
        let mut synthetic_tree = JavaParser::new().parse(&synthetic);
        let call_end = if force_call {
            Some(selector_end + 2)
        } else {
            completion_receiver_at(&synthetic_tree, syn_offset)
                .and_then(|receiver| receiver.parent())
                .and_then(|call| call.child_by_field_name("arguments"))
                .filter(|arguments| !arguments.has_error())
                .map(|arguments| arguments.end_byte())
        };
        let mut restored = None;
        let mut truncated = false;
        if let Some(call_end) =
            call_end.filter(|_| completion_needs_scope(&synthetic_tree, syn_offset))
        {
            if let Some(delimiters) = completion_delimiter_stack(&synthetic_tree, call_end) {
                if let Some(closers) = completion_call_closers(&synthetic, call_end, &delimiters) {
                    restored = Some((call_end, closers.len()));
                    synthetic.insert_str(call_end, &closers);
                    synthetic_tree = JavaParser::new().parse(&synthetic);
                } else if let Some((prefix_source, prefix_tree)) = completion_prefix_source(
                    &synthetic_tree,
                    &synthetic,
                    syn_offset,
                    call_end,
                    &delimiters,
                ) {
                    synthetic = prefix_source;
                    synthetic_tree = prefix_tree;
                    truncated = true;
                }
            }
        }
        let mut source = crate::index::Index::new();
        let recovered = if tree.root_node().has_error() && !truncated {
            let mut symbols = extract_symbols(&synthetic_tree, &synthetic);
            let (call_end, restored_closers) = restored.unwrap_or((selector_end, 0));
            // Keep semantic identities at authored offsets; discard names touching synthetic text.
            symbols.retain_mut(|symbol| {
                for (at, len) in [
                    (call_end, restored_closers),
                    (selector_end, usize::from(force_call) * 2),
                    (offset, complete::DOT_PLACEHOLDER.len()),
                ] {
                    if len == 0 {
                        continue;
                    }
                    if symbol.start_byte >= at + len {
                        symbol.start_byte -= len;
                        symbol.end_byte -= len;
                    } else if symbol.end_byte > at {
                        return false;
                    }
                }
                true
            });
            source.replace_file(file, symbols, crate::index::Tier::Volatile);
            Some(semantic_query::RecoveredTypeIndex::replacing_file(
                index, &source, file,
            ))
        } else {
            None
        };
        let index: &dyn InferenceIndex = recovered.as_ref().map_or(index, |recovered| recovered);
        let receiver = completion_receiver_at(&synthetic_tree, syn_offset);
        let (ty, receiver_kind) = if index.may_have_compiled_callables() {
            receiver
                .and_then(|receiver| {
                    java_member_receiver(
                        index,
                        file,
                        &synthetic,
                        &synthetic_tree,
                        receiver.parent()?,
                        0,
                    )
                })
                .unwrap_or((Type::Unknown, ktcore::jvm::ReceiverKind::Unknown))
        } else {
            (
                receiver
                    .and_then(|receiver| {
                        receiver_type(index, file, &synthetic, &synthetic_tree, receiver, 0)
                    })
                    .unwrap_or(Type::Unknown),
                ktcore::jvm::ReceiverKind::Unknown,
            )
        };
        return semantic_query::member_completion_query_for_type(
            index,
            language::SourceLanguage::Java,
            &synthetic_tree,
            &synthetic,
            ty,
            receiver_kind,
            prefix,
            max_candidates,
        );
    }
    let (prefix, candidates, status) = if context == complete::CompletionContext::ScopeName {
        let (prefix, _) = prefix_at(tree, src, offset);
        let candidates = scope_candidates(index, file, src, tree, &prefix);
        let status = if candidates.is_empty() {
            CompletionStatus::DefinitelyAbsent
        } else {
            CompletionStatus::Found(())
        };
        (prefix, candidates, status)
    } else {
        (
            String::new(),
            Vec::new(),
            CompletionStatus::Unknown(vec![CompletionIncompletenessReason::NonCompletablePosition]),
        )
    };
    Some(CompletionQuery {
        context,
        prefix,
        candidates,
        layout: imports::import_layout_with_style(ImportStyle::Java, tree, src),
        status,
    })
}

/// Restore call delimiters only in a completion view whose parser lost the enclosing block.
/// Balanced source and delimiters below the active brace remain untouched.
fn completion_needs_scope(tree: &Tree, selector_offset: usize) -> bool {
    if !tree.root_node().has_error() {
        return false;
    }
    let Some(receiver) = completion_receiver_at(tree, selector_offset) else {
        return false;
    };
    let mut scope = Some(receiver);
    while let Some(node) = scope {
        // A malformed call can swallow later declarations even when its block survives.
        if node.kind() == "ERROR" {
            return true;
        }
        if node.kind() == "block" {
            return false;
        }
        scope = node.parent();
    }
    true
}

fn completion_call_closers(
    src: &str,
    insert_at: usize,
    delimiters: &[(u8, bool)],
) -> Option<String> {
    // Do not close an authored suffix or turn a following statement into a call argument.
    let suffix = src.get(insert_at..)?.trim_start();
    if !suffix.is_empty() && !suffix.starts_with('}') {
        return None;
    }
    let brace = delimiters.iter().rposition(|(open, _)| *open == b'{')?;
    let unmatched = &delimiters[brace + 1..];
    if unmatched.is_empty() || unmatched.iter().any(|(_, semicolon)| *semicolon) {
        return None;
    }
    Some(
        unmatched
            .iter()
            .rev()
            .map(|(open, _)| match *open {
                b'(' => ')',
                b'[' => ']',
                _ => unreachable!("only call/index delimiters follow the innermost brace"),
            })
            .collect(),
    )
}

/// A later incomplete statement can erase an earlier receiver's block. Recover only a prefix
/// that proves the same identifier is a preserved local, parameter or resource; fields and types
/// may depend on declarations in the discarded suffix, so their full view must remain available.
fn completion_prefix_source(
    tree: &Tree,
    src: &str,
    selector_offset: usize,
    end: usize,
    delimiters: &[(u8, bool)],
) -> Option<(String, Tree)> {
    let original = completion_receiver_at(tree, selector_offset)?;
    if original.kind() != "identifier" {
        return None;
    }
    if delimiters.iter().any(|(_, semicolon)| *semicolon) {
        return None;
    }
    let brace = delimiters.iter().rposition(|(open, _)| *open == b'{')?;
    let close = |open| match open {
        b'(' => ')',
        b'[' => ']',
        b'{' => '}',
        _ => unreachable!("only structural delimiters are recorded"),
    };
    let mut prefix = String::from(src.get(..end)?);
    for (open, _) in delimiters[brace + 1..].iter().rev() {
        prefix.push(close(*open));
    }
    prefix.push(';');
    for (index, (open, _)) in delimiters[..=brace].iter().enumerate().rev() {
        prefix.push(close(*open));
        if index > 0 && delimiters[index - 1].0 == b'{' {
            prefix.push(';');
        }
    }
    let prefix_tree = JavaParser::new().parse(&prefix);
    let recovered = completion_receiver_at(&prefix_tree, selector_offset)?;
    if recovered.kind() != "identifier"
        || recovered.start_byte() != original.start_byte()
        || recovered.end_byte() != original.end_byte()
        || node_text(recovered, &prefix) != node_text(original, src)
    {
        return None;
    }
    local_declaration_node(&prefix, recovered)?;
    Some((prefix, prefix_tree))
}

fn completion_delimiter_stack(tree: &Tree, insert_at: usize) -> Option<Vec<(u8, bool)>> {
    let mut delimiters: Vec<(u8, bool)> = Vec::new();
    let mut cursor = tree.walk();
    'walk: loop {
        let node = cursor.node();
        if node.start_byte() >= insert_at {
            break;
        }
        if node.child_count() > 0 && cursor.goto_first_child() {
            continue;
        }
        // Terminal kinds, rather than source characters, exclude quoted/comment delimiters.
        if !node.is_missing() {
            match node.kind() {
                "(" => delimiters.push((b'(', false)),
                "[" => delimiters.push((b'[', false)),
                "{" => delimiters.push((b'{', false)),
                ")" | "]" | "}" => {
                    let expected = match node.kind() {
                        ")" => b'(',
                        "]" => b'[',
                        _ => b'{',
                    };
                    let (open, semicolon) = delimiters.pop()?;
                    if open != expected {
                        return None;
                    }
                    // A prior balanced for (...; ...; ...) must not poison a later call.
                    // Semicolons in nested brace bodies do not belong to enclosing arguments.
                    if semicolon && open != b'{' {
                        if let Some((parent, invalid)) = delimiters.last_mut() {
                            if *parent != b'{' {
                                *invalid = true;
                            }
                        }
                    }
                }
                ";" => {
                    if let Some((open, invalid)) = delimiters.last_mut() {
                        if *open != b'{' {
                            *invalid = true;
                        }
                    }
                }
                _ => {}
            }
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                break 'walk;
            }
        }
    }
    Some(delimiters)
}

fn completion_receiver_at(tree: &Tree, offset: usize) -> Option<Node<'_>> {
    for probe in [offset, offset.saturating_sub(1)] {
        let mut current = tree
            .root_node()
            .named_descendant_for_byte_range(probe, probe);
        while let Some(node) = current {
            if matches!(node.kind(), "field_access" | "method_invocation") {
                return node.child_by_field_name("object");
            }
            current = node.parent();
        }
    }
    None
}

fn completion_context(tree: &Tree, src: &str, offset: usize) -> complete::CompletionContext {
    let root = tree.root_node();
    for probe in [offset, offset.saturating_sub(1)] {
        let mut node = root.named_descendant_for_byte_range(probe, probe);
        while let Some(n) = node {
            match n.kind() {
                "string_literal"
                | "string_fragment"
                | "line_comment"
                | "block_comment"
                | "character_literal"
                | "decimal_integer_literal"
                | "hex_integer_literal"
                | "octal_integer_literal"
                | "binary_integer_literal"
                | "decimal_floating_point_literal"
                | "hex_floating_point_literal" => return complete::CompletionContext::None,
                "import_declaration" | "package_declaration" => {
                    return complete::CompletionContext::None
                }
                _ => {}
            }
            node = n.parent();
        }
    }
    // Incomplete Java member access also needs a raw-text check because it may parse as a type.
    let mut i = complete::floor_boundary(src, offset);
    while let Some((start, ch)) = complete::prev_char(src, i) {
        if ch.is_alphanumeric() || ch == '_' {
            i = start;
        } else {
            break;
        }
    }
    while let Some((start, ch)) = complete::prev_char(src, i) {
        if ch == ' ' || ch == '\t' {
            i = start;
        } else {
            break;
        }
    }
    if matches!(complete::prev_char(src, i), Some((_, '.'))) {
        return complete::CompletionContext::AfterDot;
    }
    complete::CompletionContext::ScopeName
}

fn prefix_at<'a>(tree: &'a Tree, src: &str, offset: usize) -> (String, Option<Node<'a>>) {
    if let Some(ident) = identifier_at(tree, offset) {
        let start = ident.start_byte();
        let end = offset.max(start).min(src.len());
        return (src[start..end].to_string(), Some(ident));
    }
    (String::new(), None)
}

fn scope_candidates(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    prefix: &str,
) -> Vec<ScopeCompletion> {
    let package = package_of(tree, src);
    let imports = imports_of(tree, src);
    let mut out: Vec<ScopeCompletion> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Same-file members and top-level declarations are always in scope.
    for entry in index.entries_for_file(file) {
        if !seen.insert(entry.sym.name.clone()) {
            continue;
        }
        let mut c = ScopeCompletion::new(entry.sym.name.clone(), entry.sym.kind);
        c.tier = entry.tier;
        c.arity = entry.sym.arity;
        c.package = entry.sym.package.clone();
        c.container = entry.sym.container.clone();
        out.push(c);
    }

    // Cross-file top-level symbols. Visible ones are offered directly; non-visible ones are
    // offered with an auto-import edit (default-package symbols cannot be imported, so they are
    // skipped).
    for mut candidate in complete::index_scope_candidates(
        index,
        file,
        prefix,
        complete::IndexScopeCandidateConfig {
            include_contained: false,
            include_default_package: false,
        },
        |entry| is_visible(&package, &imports, &entry.sym),
    ) {
        if !seen.insert(candidate.label.clone()) {
            continue;
        }
        candidate.container = None;
        out.push(candidate);
    }

    // Java keywords.
    for kw in JAVA_KEYWORDS {
        if kw.starts_with(prefix) && seen.insert(kw.to_string()) {
            out.push(ScopeCompletion::keyword(*kw));
        }
    }

    out
}

fn parse_import(node: Node, src: &str) -> Option<Import> {
    if node.kind() != "import_declaration" {
        return None;
    }
    if node
        .children(&mut node.walk())
        .any(|c| c.kind() == "static")
    {
        return None;
    }
    let mut package = String::new();
    let mut name = None;
    let mut qualifier = None;
    let mut wildcard = false;
    for c in node.children(&mut node.walk()) {
        match c.kind() {
            "identifier" | "type_identifier" => {
                name = Some(node_text(c, src).to_string());
            }
            "scoped_identifier" | "scoped_type_identifier" => {
                qualifier = Some(node_text(c, src).to_string());
            }
            "asterisk" => wildcard = true,
            _ => {}
        }
    }
    if wildcard {
        // `import pkg.*`: package is the full qualifier, or the single identifier if there is no
        // qualifier (`import pkg.*`).
        package = qualifier.unwrap_or_else(|| name.take().unwrap_or_default());
        name = None;
    } else if let Some(q) = qualifier {
        if let Some(pos) = q.rfind('.') {
            package = q[..pos].to_string();
            name = Some(q[pos + 1..].to_string());
        } else {
            name = Some(q);
        }
    }
    Some(Import {
        package,
        name,
        wildcard,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::Index;

    fn index(src: &str) -> Vec<IndexedSymbol> {
        let tree = JavaParser::new().parse(src);
        extract_symbols(&tree, src)
    }

    fn usages(src: &str) -> Vec<Usage> {
        let tree = JavaParser::new().parse(src);
        extract_usages(&tree, src)
    }

    #[test]
    fn extracts_types_methods_fields_enums() {
        let src = r#"
package com.example.app;

public class Greeter {
    private final String name;
    public static final String DEFAULT = "world";
    public Greeter(String name) { this.name = name; }
    public String greet() { return "hi " + name; }
    interface Named { String label(); }
    enum Color { RED, GREEN, BLUE }
    static class Inner { int counter; void bump() { counter++; } }
}
"#;
        let syms = index(src);
        let by_name = |n: &str| syms.iter().find(|s| s.name == n);

        let greeter = by_name("Greeter").unwrap();
        assert_eq!(greeter.kind, SymbolKind::Class);
        assert_eq!(greeter.package, "com.example.app");
        assert_eq!(greeter.container, None);

        assert_eq!(by_name("greet").unwrap().kind, SymbolKind::Function);
        assert_eq!(
            by_name("greet").unwrap().container.as_deref(),
            Some("Greeter")
        );
        assert_eq!(by_name("DEFAULT").unwrap().kind, SymbolKind::Property);
        assert_eq!(by_name("Named").unwrap().kind, SymbolKind::Interface);
        assert_eq!(
            by_name("label").unwrap().container.as_deref(),
            Some("Greeter.Named")
        );
        assert_eq!(by_name("Color").unwrap().kind, SymbolKind::EnumClass);
        assert_eq!(by_name("RED").unwrap().kind, SymbolKind::EnumEntry);
        assert_eq!(by_name("Inner").unwrap().kind, SymbolKind::Class);
        assert_eq!(by_name("counter").unwrap().kind, SymbolKind::Property);
        assert_eq!(
            by_name("bump").unwrap().container.as_deref(),
            Some("Greeter.Inner")
        );

        // A method-body local must NOT be indexed.
        assert!(by_name("name").is_some()); // the field `name`
        assert_eq!(by_name("name").unwrap().kind, SymbolKind::Property);
    }

    #[test]
    fn jdk_seed_selects_simple_type_references_without_value_or_qualified_names() {
        let src = r#"package seed;
import java.util.List;
import static java.lang.Math.max;
class Seed<OnlyDeclared> {
    Iterable<String> values;
    java.util.Map<Long, java.time.Instant> qualified;
    void run() { var value = 1; int OnlyValue = 0; }
}"#;
        let tree = JavaParser::new().parse(src);
        assert_eq!(
            referenced_java_lang_fqns(&tree, src),
            ["java.lang.Iterable", "java.lang.Long", "java.lang.String"]
        );
    }

    #[test]
    fn extracts_identifier_and_type_identifier_usages() {
        let src = r#"
package demo;
public class App {
    public void run(User user) { user.greet(); }
}
"#;
        let u = usages(src);
        let names: Vec<&str> = u.iter().map(|x| x.name.as_ref()).collect();
        assert!(
            names.contains(&"App"),
            "type identifier App should be a usage"
        );
        assert!(
            names.contains(&"User"),
            "type identifier User should be a usage"
        );
        assert!(names.contains(&"user"), "identifier user should be a usage");
        assert!(
            names.contains(&"greet"),
            "identifier greet should be a usage"
        );
    }

    #[test]
    fn identifier_at_finds_type_and_value_names() {
        let src = "package demo;\npublic class App { User user; }";
        let tree = JavaParser::new().parse(src);
        let offset = src.find("User").unwrap();
        let id = identifier_at(&tree, offset).unwrap();
        assert_eq!(node_text(id, src), "User");
        let offset = src.find("user").unwrap();
        let id = identifier_at(&tree, offset).unwrap();
        assert_eq!(node_text(id, src), "user");
    }

    #[test]
    fn parses_explicit_and_wildcard_imports() {
        let src = r#"
package demo;
import java.util.List;
import java.io.*;
import static java.lang.Math.max;
class App {}
"#;
        let tree = JavaParser::new().parse(src);
        let imps = imports_of(&tree, src);
        assert_eq!(imps.len(), 2, "static imports are ignored for now");
        let list = imps
            .iter()
            .find(|i| i.simple_name() == Some("List"))
            .unwrap();
        assert_eq!(list.package, "java.util");
        let wildcard = imps.iter().find(|i| i.wildcard).unwrap();
        assert_eq!(wildcard.package, "java.io");
    }

    #[test]
    fn diagnostics_resolve_nested_type_imports() {
        let lib_src = r#"
package lib;
public class Outer {
    public static class Inner {}
    public static class Middle {
        public static class Deep {}
    }
}
"#;
        let app_src = r#"
package app;
import lib.Outer.Inner;
import lib.Outer.Middle.Deep;
public class App { Inner value; Deep deep; }
"#;
        let lib_tree = JavaParser::new().parse(lib_src);
        let app_tree = JavaParser::new().parse(app_src);
        let mut index = Index::new();
        index.replace_file(
            "/lib/Outer.java",
            extract_symbols(&lib_tree, lib_src),
            crate::index::Tier::Volatile,
        );
        index.replace_file(
            "/app/App.java",
            extract_symbols(&app_tree, app_src),
            crate::index::Tier::Volatile,
        );

        let facts = crate::resolve::CompletenessFacts {
            project_scan_complete: true,
            ..Default::default()
        };
        let diags = diagnostics(&index, "/app/App.java", &app_tree, app_src, &facts);
        assert!(
            !diags
                .iter()
                .any(|diag| diag.message == "Unresolved import: lib.Outer.Inner"),
            "diagnostics were {diags:?}"
        );
        assert!(
            !diags
                .iter()
                .any(|diag| diag.message == "Unresolved import: lib.Outer.Middle.Deep"),
            "diagnostics were {diags:?}"
        );
    }

    #[test]
    fn diagnostics_report_only_project_local_unqualified_missing_methods() {
        let src = r#"
package demo;
class App {
    void present() {}
    void run(App other) {
        missing();
        present();
        other.missingQualified();
        getClass();
    }
}
"#;
        let tree = JavaParser::new().parse(src);
        let mut index = Index::new();
        index.replace_file(
            "/demo/App.java",
            extract_symbols(&tree, src),
            crate::index::Tier::Volatile,
        );
        let facts = crate::resolve::CompletenessFacts {
            project_scan_complete: true,
            // This authored-only fixture certifies producers; durable coverage stays unknown.
            compiler_producers_complete: true,
            ..Default::default()
        };

        let diags = diagnostics_with_options(&index, "/demo/App.java", &tree, src, &facts, false);
        let unresolved = diags
            .iter()
            .filter(|diag| {
                diag.code == Some(crate::diagnostics::DiagnosticCode::UnresolvedReference)
            })
            .collect::<Vec<_>>();

        assert_eq!(unresolved.len(), 1, "diagnostics were {diags:?}");
        assert_eq!(unresolved[0].message, "Unresolved reference: missing");
        assert_eq!(unresolved[0].start_byte, src.find("missing();").unwrap());
    }

    #[test]
    fn diagnostics_suppress_unqualified_missing_methods_with_inheritance_or_static_imports() {
        let inherited_src = r#"
package demo;
class Child extends Base {
    void run() { inheritedMaybe(); }
}
"#;
        let inherited_tree = JavaParser::new().parse(inherited_src);
        let static_src = r#"
package demo;
import static helpers.Maybe.staticMaybe;
class App {
    void run() { staticMaybe(); }
}
"#;
        let static_tree = JavaParser::new().parse(static_src);
        let facts = crate::resolve::CompletenessFacts {
            project_scan_complete: true,
            ..Default::default()
        };

        for (file, tree, src) in [
            ("/demo/Child.java", &inherited_tree, inherited_src),
            ("/demo/App.java", &static_tree, static_src),
        ] {
            let diags = diagnostics_with_options(&Index::new(), file, tree, src, &facts, false);
            assert!(
                !diags.iter().any(|diag| {
                    diag.code == Some(crate::diagnostics::DiagnosticCode::UnresolvedReference)
                }),
                "diagnostics for {file} were {diags:?}"
            );
        }
    }

    #[test]
    fn finds_explicit_import_fqn_at_cursor() {
        let src = r#"
package demo;
import com.alibaba.cloud.nacos.NacosConfigManager;
import java.io.*;
class App {}
"#;
        let tree = JavaParser::new().parse(src);
        let offset = src.find("NacosConfigManager").unwrap();
        assert_eq!(
            explicit_import_fqn_at(&tree, src, offset).as_deref(),
            Some("com.alibaba.cloud.nacos.NacosConfigManager")
        );
        let wildcard_offset = src.find("java.io").unwrap();
        assert_eq!(explicit_import_fqn_at(&tree, src, wildcard_offset), None);
    }

    #[test]
    fn goto_definition_respects_same_package_and_imports() {
        let src = r#"
package demo;
import other.Helper;
public class App {
    public void run() {
        Helper h = new Helper();
        h.help();
    }
}
"#;
        let tree = JavaParser::new().parse(src);
        let mut index = Index::new();
        let helper = IndexedSymbol::new("Helper", SymbolKind::Class, "other", None, 0, 6);
        let app = IndexedSymbol::new("App", SymbolKind::Class, "demo", None, 0, 3);
        index.replace_file(
            "/other/Helper.java",
            vec![helper],
            crate::index::Tier::Volatile,
        );
        index.replace_file("/demo/App.java", vec![app], crate::index::Tier::Volatile);

        let help_offset = src.find("h.help").unwrap() + 2; // cursor on "help"
        let defs = goto_definition(&index, "/demo/App.java", src, &tree, help_offset);
        // `help` is not indexed, so no result yet.
        assert!(defs.is_empty());

        let helper_offset = src.find("new Helper").unwrap() + 8; // cursor on "Helper"
        let defs = goto_definition(&index, "/demo/App.java", src, &tree, helper_offset);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].file, "/other/Helper.java");
    }

    #[test]
    fn goto_definition_resolves_java_parameters_and_locals() {
        let src = r#"
package demo;
public class App {
    public boolean run(String key) {
        RLock lock = factory(key);
        return lock.tryLock(key.length(), java.util.concurrent.TimeUnit.SECONDS);
    }
}
"#;
        let tree = JavaParser::new().parse(src);
        let index = Index::new();

        let key_usage = src.find("factory(key)").unwrap() + "factory(".len();
        let defs = goto_definition(&index, "/demo/App.java", src, &tree, key_usage);
        assert_eq!(defs.len(), 1);
        let key_decl = src.find("key)").unwrap();
        assert_eq!(defs[0].file, "/demo/App.java");
        assert_eq!(defs[0].start_byte, key_decl);

        let lock_usage = src.find("return lock").unwrap() + "return ".len();
        let defs = goto_definition(&index, "/demo/App.java", src, &tree, lock_usage);
        assert_eq!(defs.len(), 1);
        let lock_decl = src.find("lock =").unwrap();
        assert_eq!(defs[0].file, "/demo/App.java");
        assert_eq!(defs[0].start_byte, lock_decl);
    }

    #[test]
    fn goto_definition_prefers_nearest_visible_java_local() {
        let src = r#"
package demo;
public class App {
    public int run(int value) {
        int result = value;
        {
            int result = value + 1;
            return result;
        }
    }
}
"#;
        let tree = JavaParser::new().parse(src);
        let index = Index::new();

        let result_usage = src.rfind("result;").unwrap();
        let defs = goto_definition(&index, "/demo/App.java", src, &tree, result_usage);
        assert_eq!(defs.len(), 1);
        let decls = src.match_indices("result =").collect::<Vec<_>>();
        assert_eq!(decls.len(), 2);
        assert_eq!(defs[0].start_byte, decls[1].0);
    }

    #[test]
    fn goto_definition_resolves_this_field_and_method_members() {
        let src = r#"
package demo;
public class App {
    private int count;

    public void bump() {
        this.count = this.count + 1;
        this.bump();
    }
}
"#;
        let tree = JavaParser::new().parse(src);
        let syms = extract_symbols(&tree, src);
        let mut index = Index::new();
        index.replace_file("/demo/App.java", syms, crate::index::Tier::Volatile);

        let count_usage = src.rfind("this.count").unwrap() + "this.".len();
        let defs = goto_definition(&index, "/demo/App.java", src, &tree, count_usage);
        assert_eq!(defs.len(), 1);
        let count_decl = src.find("count;").unwrap();
        assert_eq!(defs[0].file, "/demo/App.java");
        assert_eq!(defs[0].start_byte, count_decl);

        let method_usage = src.rfind("this.bump").unwrap() + "this.".len();
        let defs = goto_definition(&index, "/demo/App.java", src, &tree, method_usage);
        assert_eq!(defs.len(), 1);
        let method_decl = src.find("bump()").unwrap();
        assert_eq!(defs[0].file, "/demo/App.java");
        assert_eq!(defs[0].start_byte, method_decl);
    }

    #[test]
    fn goto_definition_resolves_identifier_receiver_methods() {
        let lib_src = r#"
package java.util.concurrent;
public interface ExecutorService {
    boolean awaitTermination(long timeout, TimeUnit unit);
    java.util.List shutdownNow();
}
"#;
        let app_src = r#"
package demo;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.TimeUnit;
public class App {
    void stop(ExecutorService ex) throws Exception {
        ex.awaitTermination(1, TimeUnit.SECONDS);
        ex.shutdownNow();
    }
}
"#;
        let lib_tree = JavaParser::new().parse(lib_src);
        let app_tree = JavaParser::new().parse(app_src);
        let mut index = Index::new();
        index.replace_file(
            "/java/util/concurrent/ExecutorService.java",
            extract_symbols(&lib_tree, lib_src),
            crate::index::Tier::Durable,
        );
        index.replace_file(
            "/demo/App.java",
            extract_symbols(&app_tree, app_src),
            crate::index::Tier::Volatile,
        );

        let await_usage = app_src.find("awaitTermination").unwrap();
        let defs = goto_definition(&index, "/demo/App.java", app_src, &app_tree, await_usage);
        assert_eq!(defs.len(), 1);
        let await_decl = lib_src.find("awaitTermination").unwrap();
        assert_eq!(defs[0].file, "/java/util/concurrent/ExecutorService.java");
        assert_eq!(defs[0].start_byte, await_decl);

        let shutdown_usage = app_src.find("shutdownNow").unwrap();
        let defs = goto_definition(&index, "/demo/App.java", app_src, &app_tree, shutdown_usage);
        assert_eq!(defs.len(), 1);
        let shutdown_decl = lib_src.find("shutdownNow").unwrap();
        assert_eq!(defs[0].start_byte, shutdown_decl);
    }

    #[test]
    fn goto_definition_resolves_static_method_on_imported_type() {
        let lib_src = r#"
package java.time;
public final class Duration {
    public static Duration ofSeconds(long seconds) { return null; }
    public static Duration ofSeconds(long seconds, long nanoAdjustment) { return null; }
}
"#;
        let app_src = r#"
package demo;
import java.time.Duration;
public class App {
    private static final Duration HTTP_TIMEOUT = Duration.ofSeconds(5);
}
"#;
        let lib_tree = JavaParser::new().parse(lib_src);
        let app_tree = JavaParser::new().parse(app_src);
        let mut index = Index::new();
        index.replace_file(
            "/java/time/Duration.java",
            extract_symbols(&lib_tree, lib_src),
            crate::index::Tier::Durable,
        );
        index.replace_file(
            "/demo/App.java",
            extract_symbols(&app_tree, app_src),
            crate::index::Tier::Volatile,
        );

        let usage = app_src.find("ofSeconds").unwrap();
        let defs = goto_definition(&index, "/demo/App.java", app_src, &app_tree, usage);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].file, "/java/time/Duration.java");
        let method_decl = lib_src.find("ofSeconds(long seconds)").unwrap();
        assert_eq!(defs[0].start_byte, method_decl);
    }

    #[test]
    fn goto_definition_prefers_local_receiver_over_type_receiver() {
        let lib_src = r#"
package java.time;
public final class Duration {
    public static Duration ofSeconds(long seconds) { return null; }
}
"#;
        let local_src = r#"
package demo;
public class LocalDuration {
    public void ofSeconds(long seconds) {}
}
"#;
        let app_src = r#"
package demo;
import java.time.Duration;
public class App {
    void run(LocalDuration Duration) {
        Duration.ofSeconds(5);
    }
}
"#;
        let lib_tree = JavaParser::new().parse(lib_src);
        let local_tree = JavaParser::new().parse(local_src);
        let app_tree = JavaParser::new().parse(app_src);
        let mut index = Index::new();
        index.replace_file(
            "/java/time/Duration.java",
            extract_symbols(&lib_tree, lib_src),
            crate::index::Tier::Durable,
        );
        index.replace_file(
            "/demo/LocalDuration.java",
            extract_symbols(&local_tree, local_src),
            crate::index::Tier::Volatile,
        );
        index.replace_file(
            "/demo/App.java",
            extract_symbols(&app_tree, app_src),
            crate::index::Tier::Volatile,
        );

        let usage = app_src.find("ofSeconds").unwrap();
        let defs = goto_definition(&index, "/demo/App.java", app_src, &app_tree, usage);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].file, "/demo/LocalDuration.java");
        let method_decl = local_src.find("ofSeconds").unwrap();
        assert_eq!(defs[0].start_byte, method_decl);
    }

    #[test]
    fn goto_definition_resolves_qualified_enum_constants() {
        let enum_src = r#"
package other;
public enum Visibility {
    PUBLIC,
    PRIVATE
}
"#;
        let app_src = r#"
package demo;
import other.Visibility;
public class App {
    boolean ok() {
        return Visibility.PUBLIC == Visibility.PRIVATE;
    }
}
"#;
        let enum_tree = JavaParser::new().parse(enum_src);
        let app_tree = JavaParser::new().parse(app_src);
        let mut index = Index::new();
        index.replace_file(
            "/other/Visibility.java",
            extract_symbols(&enum_tree, enum_src),
            crate::index::Tier::Volatile,
        );
        index.replace_file(
            "/demo/App.java",
            extract_symbols(&app_tree, app_src),
            crate::index::Tier::Volatile,
        );

        let public_offset = app_src.find("PUBLIC").unwrap();
        let defs = goto_definition(&index, "/demo/App.java", app_src, &app_tree, public_offset);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].file, "/other/Visibility.java");
        let public_decl = enum_src.find("PUBLIC").unwrap();
        assert_eq!(defs[0].start_byte, public_decl);

        let private_offset = app_src.find("PRIVATE").unwrap();
        let defs = goto_definition(&index, "/demo/App.java", app_src, &app_tree, private_offset);
        assert_eq!(defs.len(), 1);
        let private_decl = enum_src.find("PRIVATE").unwrap();
        assert_eq!(defs[0].start_byte, private_decl);
    }

    #[test]
    fn call_shape_accepts_java_hierarchy_boxing_record_and_inherited_members() {
        let collections_src = r#"
package collections;
interface Collection {}
interface Set extends Collection {}
class HashSet implements Set {}
class Api { void accept(Collection value) {} void acceptObject(Object value) {} }
"#;
        let base_src = r#"
package demo;
class Base { void send(String value) {} }
"#;
        let app_src = r#"
package demo;
import collections.Api;
import collections.HashSet;
record Result(boolean success) { static Result success(String message) { return null; } }
class Child extends Base {
    void send(int value) {}
    void run(Result result) {
        new Api().accept(new HashSet());
        new Api().acceptObject(0);
        send("inherited");
        result.success();
    }
}
"#;
        let collections_tree = JavaParser::new().parse(collections_src);
        let base_tree = JavaParser::new().parse(base_src);
        let app_tree = JavaParser::new().parse(app_src);
        let mut index = Index::new();
        index.replace_file(
            "/collections/Types.java",
            extract_symbols(&collections_tree, collections_src),
            crate::index::Tier::Volatile,
        );
        index.replace_file(
            "/demo/Base.java",
            extract_symbols(&base_tree, base_src),
            crate::index::Tier::Volatile,
        );
        index.replace_file(
            "/demo/Child.java",
            extract_symbols(&app_tree, app_src),
            crate::index::Tier::Volatile,
        );

        let facts = crate::resolve::CompletenessFacts {
            project_scan_complete: true,
            ..Default::default()
        };
        let diags = diagnostics(&index, "/demo/Child.java", &app_tree, app_src, &facts);
        assert!(
            !diags.iter().any(|diag| {
                diag.code == Some(crate::diagnostics::DiagnosticCode::CallShapeMismatch)
            }),
            "diagnostics were {diags:?}"
        );
    }

    #[test]
    fn nested_java_argument_inference_shares_the_leaf_lookup() {
        use crate::index::{InferenceIndex, LookupEntries};
        use std::cell::Cell;

        struct CountIndex {
            index: Index,
            leaf_queries: Cell<usize>,
        }
        impl InferenceIndex for CountIndex {
            fn may_have_compiled_callables(&self) -> bool {
                InferenceIndex::may_have_compiled_callables(&self.index)
            }
            fn lookup_by_name(&self, name: &str) -> LookupEntries<'_> {
                InferenceIndex::lookup_by_name(&self.index, name)
            }
            fn members_of(&self, name: &str) -> LookupEntries<'_> {
                if name == "LeafOwner" {
                    self.leaf_queries.set(self.leaf_queries.get() + 1);
                }
                InferenceIndex::members_of(&self.index, name)
            }
            fn extensions_for(&self, name: &str) -> LookupEntries<'_> {
                InferenceIndex::extensions_for(&self.index, name)
            }
            fn entries_for_file(&self, file: &str) -> Vec<Entry> {
                InferenceIndex::entries_for_file(&self.index, file)
            }
        }

        let mut counts = Vec::new();
        for depth in [1, 8] {
            let mut expression = "leaf.leaf()".to_string();
            for _ in 0..depth {
                expression = format!("result.id({expression})");
            }
            let source = format!("package demo; class Result {{ public Result id(Result value) {{ return value; }} }} class LeafOwner {{ public Result leaf() {{ return null; }} }} class Use {{ void use(Result result,LeafOwner leaf) {{ {expression}; }} }}");
            let tree = JavaParser::new().parse(&source);
            let mut raw = Index::new();
            raw.replace_file("Use.java", extract_symbols(&tree, &source), Tier::Volatile);
            let index = CountIndex {
                index: raw,
                leaf_queries: Cell::new(0),
            };
            let start = source.find(&expression).unwrap();
            let call = tree
                .root_node()
                .descendant_for_byte_range(start, start + expression.len())
                .unwrap();
            assert_eq!(call.kind(), "method_invocation");
            assert_eq!(node_text(call, &source), expression);
            assert_eq!(
                java_method_invocation_return_type(&index, "Use.java", &source, &tree, call, 0),
                Some(Type::class("Result", Some("demo".into()))),
                "the complete nested call must retain its useful result at depth {depth}"
            );
            counts.push(index.leaf_queries.get());
        }
        assert_eq!(
            counts[0], counts[1],
            "deeper argument nesting must not repeat the leaf inference"
        );
        assert!(
            (1..=4).contains(&counts[1]),
            "leaf work must be bounded independently of nesting: {counts:?}"
        );

        let mut generic_counts = Vec::new();
        for depth in [1, 8, 20, 32] {
            let mut expression = "leaf.leaf()".to_string();
            for _ in 0..depth {
                expression = format!("identity.id({expression})");
            }
            let source = format!("package demo; class Result {{}} class Identity {{ public <T> T id(T value) {{ return value; }} }} class LeafOwner {{ public Result leaf() {{ return null; }} }} class Use {{ void use(Identity identity,LeafOwner leaf) {{ {expression}; }} }}");
            let tree = JavaParser::new().parse(&source);
            let mut raw = Index::new();
            raw.replace_file("Use.java", extract_symbols(&tree, &source), Tier::Volatile);
            let index = CountIndex {
                index: raw,
                leaf_queries: Cell::new(0),
            };
            let start = source.find(&expression).unwrap();
            let call = tree
                .root_node()
                .descendant_for_byte_range(start, start + expression.len())
                .unwrap();
            assert_eq!(call.kind(), "method_invocation");
            assert_eq!(
                java_method_invocation_return_type(&index, "Use.java", &source, &tree, call, 0),
                Some(Type::class("Result", Some("demo".into()))),
                "generic identity must retain the computed leaf type at ordinary call depth {depth}",
            );
            generic_counts.push(index.leaf_queries.get());
        }
        assert!(
            generic_counts
                .iter()
                .all(|count| *count == generic_counts[0]),
            "ordinary call depth must not repeat or skip the leaf: {generic_counts:?}"
        );
        assert!(
            (1..=4).contains(&generic_counts[0]),
            "the useful deep result requires bounded, nonzero leaf inference: {generic_counts:?}"
        );

        let mut unknown_counts = Vec::new();
        for depth in [1, 8] {
            let mut expression = "leaf.leaf()".to_string();
            for _ in 0..depth {
                expression = format!("missing({expression})");
            }
            let source = format!("package demo; class Result {{}} class LeafOwner {{ public Result leaf() {{ return null; }} }} class Use {{ void use(LeafOwner leaf) {{ var value = {expression}; }} }}");
            let tree = JavaParser::new().parse(&source);
            let mut raw = Index::new();
            raw.replace_file("Use.java", extract_symbols(&tree, &source), Tier::Volatile);
            let index = CountIndex {
                index: raw,
                leaf_queries: Cell::new(0),
            };
            let start = source.find(&expression).unwrap();
            let call = tree
                .root_node()
                .descendant_for_byte_range(start, start + expression.len())
                .unwrap();
            assert_eq!(call.kind(), "method_invocation");
            assert_eq!(
                java_method_invocation_return_type(&index, "Use.java", &source, &tree, call, 0),
                None
            );
            unknown_counts.push(index.leaf_queries.get());
        }
        assert_eq!(
            unknown_counts[0], unknown_counts[1],
            "unresolved implicit calls must share their argument inference: {unknown_counts:?}"
        );
        assert!(
            (1..=4).contains(&unknown_counts[1]),
            "unknown fallback must not repeat the leaf inference: {unknown_counts:?}"
        );
    }

    #[test]
    fn java_resource_bindings_follow_the_specification_and_try_body_only() {
        let source = r#"class Use {
 void run() {
  try (First first = open(); Second second = first.next()) {
   first.inside(); second.inside();
  } catch (Exception e) { first.caught(); } finally { first.finished(); }
  first.after();
  try (First before = later.forward(); Second later = open()) {}
  try (First self = self.make()) {}
  First existing = open();
  try (existing) { existing.reused(); }
 }
}"#;
        let tree = JavaParser::new().parse(source);
        assert!(!tree.root_node().has_error());
        for (usage, declaration) in [
            ("first.next", Some("first =")),
            ("first.inside", Some("first =")),
            ("second.inside", Some("second =")),
            ("first.caught", None),
            ("first.finished", None),
            ("first.after", None),
            ("later.forward", None),
            ("self.make", Some("self =")),
            ("existing.reused", Some("existing =")),
        ] {
            let offset = source.find(usage).unwrap();
            let node = identifier_at(&tree, offset).unwrap();
            let found = local_declaration_node(source, node);
            assert_eq!(
                found.map(|decl| decl.start_byte()),
                declaration.map(|text| source.find(text).unwrap()),
                "{usage}"
            );
        }
        let declaration = source.find("first =").unwrap();
        let definitions =
            authored_goto_definition(&Index::new(), "Use.java", source, &tree, declaration);
        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0].start_byte, declaration);
    }

    #[test]
    fn java_incomplete_nested_invocation_keeps_the_completion_receiver() {
        let type_source = "package app; class Body { public String string() { return null; } } class body { public static void enqueue() {} }";
        let type_tree = JavaParser::new().parse(type_source);
        let mut index = Index::new();
        index.replace_file(
            "Types.java",
            extract_symbols(&type_tree, type_source),
            Tier::Volatile,
        );
        for (case, source) in [
            ("bare", "package app; class Use { void run(Body body) {\n body.\n } }"),
            ("balanced", "package app; class Use { void run(Body body) {\n System.out.println(body.);\n } }"),
            ("unfinished", "package app; class Use { void run(Body body) {\n System.out.println(body.\n } }"),
            ("nested calls", "package app; class Use { void run(Body body) {\n System.out.println(wrap(body.\n } }"),
            ("quoted and commented delimiters", r#"package app; class Use { void run(Body body) {
 String marker = ") } ; [ ("; char close = ')';
 /* ) } ; [ ( */
 // ) } ; [ (
 System.out.println(body.
 } }"#),
            ("prior classic for and anonymous callback", r#"package app; class Use {
 void enqueue(Callback callback) {}
 void run() { enqueue(new Callback() {
  public void onResponse(Body body, Headers responseHeaders) {
   for (int i = 0, size = responseHeaders.size(); i < size; i++) {
    System.out.println(responseHeaders.name(i));
   }
   System.out.println(body.
  }
 }); }
}"#),
        ] {
            let offset = source.find("body.").unwrap() + "body.".len();
            let (prefix, mut synthetic, synthetic_offset) =
                complete::dot_recovery(source, offset).unwrap();
            assert!(prefix.is_empty());
            synthetic.insert_str(synthetic_offset, "()");
            let tree = JavaParser::new().parse(&synthetic);
            let receiver =
                completion_receiver_at(&tree, synthetic_offset).expect("inner call receiver");
            assert_eq!(node_text(receiver, &synthetic), "body", "{case}");
            let source_tree = JavaParser::new().parse(source);
            let query = completion_query(&index, "Use.java", source, &source_tree, offset, 100).unwrap();
            let labels: HashSet<_> = query.candidates.iter().map(|item| item.label.as_str()).collect();
            assert!(labels.contains("string"), "{case}: {labels:?}");
            assert!(!labels.contains("enqueue"), "{case}: {labels:?}");
        }
        for source in [
            "package app; class Use { void run(Body body) { String text = \"body.\"; } }",
            "package app; class Use { void run(Body body) { /* body. */ } }",
            "package app; class Use { void run(Body body) { // body.\n } }",
        ] {
            let offset = source.find("body.").unwrap() + "body.".len();
            let tree = JavaParser::new().parse(source);
            let query = completion_query(&index, "Use.java", source, &tree, offset, 100).unwrap();
            assert_ne!(query.context, complete::CompletionContext::AfterDot);
            assert!(query.candidates.is_empty());
        }
    }

    #[test]
    fn java_completion_recovers_current_same_file_factories_and_authored_call_suffixes() {
        let types = "package app; class Body { String string() { return null; } }";
        let mut index = Index::new();
        let tree = JavaParser::new().parse(types);
        index.replace_file("Body.java", extract_symbols(&tree, types), Tier::Volatile);
        for (source, cursor) in [
            ("package app; class Use { Body make() { return null; } void run() { var body = make(); System.out.println(body. } }", "body."),
            ("package app; class Use { void run() { var body = make(); System.out.println(body. } Body make() { return null; } }", "body."),
            ("package app; class Use { void run(Body body) { body.string(); } }", "body."),
            ("package app; class Use { void run(Body body) { body.string(); } }", "body.str"),
            ("package app; class Use { void run(Body body) { body.string; } }", "body.str"),
        ] {
            let tree = JavaParser::new().parse(source);
            let original = extract_symbols(&tree, source);
            index.replace_file("Use.java", original.clone(), Tier::Volatile);
            let offset = source.find(cursor).unwrap() + cursor.len();
            let query = completion_query(&index, "Use.java", source, &tree, offset, 100).unwrap();
            assert!(query.candidates.iter().any(|candidate| candidate.label == "string"),
                "{cursor} in {source}: {:?}", query.candidates);
            assert_eq!(index.entries_for_file("Use.java").len(), original.len());
        }
    }

    #[test]
    fn java_completion_recovers_same_file_generic_calls_in_unfinished_statements() {
        let types = "package app; class Body { String string() { return null; } }";
        let mut index = Index::new();
        let tree = JavaParser::new().parse(types);
        index.replace_file("Body.java", extract_symbols(&tree, types), Tier::Volatile);
        for depth in [1, 8, 32] {
            let mut expression = "supply".to_string();
            for _ in 0..depth {
                expression = format!("identity({expression})");
            }
            let source = format!("package app; class Use {{ <T> T identity(T value) {{ return value; }} void run(Body supply) {{ {expression}. }} }}");
            let tree = JavaParser::new().parse(&source);
            index.replace_file("Use.java", extract_symbols(&tree, &source), Tier::Volatile);
            let offset = source.find(&format!("{expression}.")).unwrap() + expression.len() + 1;
            let query = completion_query(&index, "Use.java", &source, &tree, offset, 100).unwrap();
            assert!(
                query
                    .candidates
                    .iter()
                    .any(|candidate| candidate.label == "string"),
                "depth {depth}: {:?}; original declarations: {:?}",
                query.candidates,
                index
                    .entries_for_file("Use.java")
                    .iter()
                    .map(|entry| &entry.sym.name)
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn java_receiver_field_goto_preserves_identifier_roles_and_local_shadowing() {
        let type_source = "package app; class Body { public void string() {} } class body {} class Other { Body body; }";
        let source = r#"package app; class Use {
 Body body;
 void body() {}
 void run(Other other) {
  /*field*/ body.string();
  /*method*/ body();
  /*type*/ body value = null;
  /*selector*/ other.body.string();
 }
 void shadow(Body body) { /*local*/ body.string(); }
}"#;
        let tree = JavaParser::new().parse(source);
        let type_tree = JavaParser::new().parse(type_source);
        let mut index = Index::new();
        index.replace_file("Use.java", extract_symbols(&tree, source), Tier::Volatile);
        index.replace_file(
            "Types.java",
            extract_symbols(&type_tree, type_source),
            Tier::Volatile,
        );
        for (marker, file, declaration) in [
            (
                "field",
                "Use.java",
                source.find("Body body;").unwrap() + "Body ".len(),
            ),
            (
                "method",
                "Use.java",
                source.find("void body()").unwrap() + "void ".len(),
            ),
            (
                "type",
                "Types.java",
                type_source.find("class body").unwrap() + "class ".len(),
            ),
            (
                "selector",
                "Types.java",
                type_source.find("class Other { Body body;").unwrap() + "class Other { Body ".len(),
            ),
            (
                "local",
                "Use.java",
                source.find("shadow(Body body)").unwrap() + "shadow(Body ".len(),
            ),
        ] {
            let start = source.find(&format!("/*{marker}*/")).unwrap();
            let offset = start + source[start..].find("body").unwrap();
            let definitions = goto_definition(&index, "Use.java", source, &tree, offset);
            assert_eq!(
                definitions,
                vec![Def {
                    file: file.to_string(),
                    start_byte: declaration,
                    end_byte: declaration + "body".len(),
                }],
                "{marker}"
            );
        }
    }
    #[test]
    fn java_completion_prefix_recovery_preserves_bound_names_only() {
        let type_source = "package app; class Body { public String string() { return null; } } class body { public static void enqueue() {} }";
        let type_tree = JavaParser::new().parse(type_source);
        let mut index = Index::new();
        index.replace_file(
            "Types.java",
            extract_symbols(&type_tree, type_source),
            Tier::Volatile,
        );
        for (case, source, cursor, expected_type) in [
            ("parameter before broken resource", "package app; class Use { void run(Body supply) {\n supply.string();\n try (Missing body = supply) {\n System.out.println(body.\n } } }", "supply.", Some("Body")),
            ("local before broken statement", "package app; class Use { void run(Body supply) { Body value = supply;\n value.string();\n System.out.println(value.\n } }", "value.", Some("Body")),
            ("resource before broken statement", "package app; class Use { void run(Body supply) { try (Body body = supply) {\n body.string();\n System.out.println(body.\n } } }", "body.", Some("Body")),
            ("explicit missing is never initializer inference", "package app; class Use { void run(Body supply) { try (Missing body = supply) {\n body.string();\n System.out.println(body.\n } } }", "body.", Some("Missing")),
            ("later field cannot disappear", "package app; class Use { void run() {\n body.string();\n System.out.println(body.\n } Body body; }", "body.", None),
            ("later local is not yet in scope", "package app; class Use { void run(Body supply) {\n body.string(); Body body = supply;\n System.out.println(body.\n } }", "body.", None),
            ("type receiver retains full view", "package app; class Use { void run() {\n Body.string();\n System.out.println(body.\n } }", "Body.", None),
            ("computed receiver retains full view", "package app; class Use { void run() {\n make().string();\n System.out.println(body.\n } Body make() { return null; } }", "make().", None),
        ] {
            let offset = source.find(cursor).unwrap() + cursor.len();
            let (_, mut synthetic, selector_offset) = complete::dot_recovery(source, offset).unwrap();
            synthetic.insert_str(selector_offset, "()");
            let tree = JavaParser::new().parse(&synthetic);
            let delimiters = completion_delimiter_stack(&tree, selector_offset + 2).unwrap();
            let recovered = completion_prefix_source(&tree, &synthetic, selector_offset, selector_offset + 2, &delimiters);
            if let Some(expected) = expected_type {
                let (prefix, prefix_tree) = recovered.unwrap_or_else(|| panic!("{case}"));
                let receiver = completion_receiver_at(&prefix_tree, selector_offset).unwrap();
                let declaration = local_declaration_node(&prefix, receiver).unwrap();
                let ty = type_of_value_declaration(&index, "Use.java", &prefix, &prefix_tree, declaration, 0).unwrap();
                assert_eq!(ty.name(), Some(expected), "{case}");
                let source_tree = JavaParser::new().parse(source);
                let query = completion_query(&index, "Use.java", source, &source_tree, offset, 100).unwrap();
                let labels: HashSet<_> = query.candidates.iter().map(|item| item.label.as_str()).collect();
                if expected == "Body" {
                    assert!(labels.contains("string"), "{case}: {labels:?}");
                } else {
                    assert!(!labels.contains("string"), "{case}: {labels:?}");
                }
                assert!(!labels.contains("enqueue"), "{case}: {labels:?}");
            } else {
                assert!(recovered.is_none(), "{case}");
            }
        }
    }

    #[test]
    fn java_var_initializer_bindings_respect_value_and_selector_names() {
        for (initializer, owns_value) in [
            ("/*probe*/body", true),
            ("make(/*probe*/body)", true),
            ("/*probe*/body.make()", true),
            ("new Body(/*probe*/body)", true),
            ("/*probe*/body()", false),
            ("factory./*probe*/body()", false),
            ("factory./*probe*/body", false),
            ("new /*probe*/body()", false),
            ("factory::/*probe*/body", false),
            ("/*probe*/body::make", true),
        ] {
            let source = format!("class Use {{ Body body; Body body() {{ return null; }} void run(Factory factory) {{ var body = {initializer}; }} }}");
            let tree = JavaParser::new().parse(&source);
            assert!(
                !tree.root_node().has_error(),
                "{initializer}: {}",
                tree.root_node().to_sexp()
            );
            let declaration =
                identifier_at(&tree, source.find("var body =").unwrap() + "var ".len()).unwrap();
            let usage = identifier_at(&tree, source.find("/*probe*/").unwrap() + "/*probe*/".len())
                .unwrap();
            assert_eq!(
                local_declaration_node(&source, usage),
                owns_value.then_some(declaration),
                "{initializer}: role {} must retain its lexical binding",
                usage.kind(),
            );
        }
    }
}
