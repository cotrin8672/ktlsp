//! Demand-driven, compiler-free type inference.
//!
//! `infer(index, node, src, ctx)` answers "what is the type of this expression?" well enough to
//! drive member completion and member goto. It is **best-effort**: any expression it cannot resolve
//! becomes [`Type::Unknown`], which yields no members — honouring the silent-omission contract
//! (never a wrong completion, show nothing when unsure). There is no constraint solver and no
//! kotlinc; everything is structural over the tree-sitter AST plus the symbol index.
//!
//! The single entry point is called from BOTH the goto member path (`resolve::goto`) and the
//! completion after-dot path (`workspace::assemble_after_dot`), so the two can never disagree about
//! a receiver's type. Package resolution (which `Foo` a simple name means, given the file's imports)
//! lives here — it is why a `Type` carries a resolved `package`, replacing the old post-hoc
//! `resolve_type_package` band-aid.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;
use std::sync::Arc;

use crate::fxhash::FxHashMap;

use tree_sitter::{Node, Tree};

use crate::index::{Entry, InferenceIndex};
use crate::members::{self, CallArguments, MemberUse};
use crate::parser::{
    child_of_kind, first_ident, imports_of, name_field, node_text, package_of, Import,
};
use crate::resolve::{self, UseKind};
use crate::solve;
use crate::symbol::SymbolKind;
use crate::types::{Projection, Type, TypeRef};

/// Recursion guard for the inference walk (chained calls, initializer cycles like `val x = x`).
pub const MAX_DEPTH: usize = 16;
/// Cap on the supertype walk when looking up a member's type (mirrors the completion/goto caps).
const SUPERTYPE_CAP: usize = 32;

/// The file-level facts inference needs to resolve a simple type name to a package: the file's own
/// package and its imports. Built once per request from the (synthetic or real) tree.
#[derive(Clone, Debug)]
pub struct FileCtx {
    pub package: String,
    pub imports: Vec<Import>,
    pub local_extensions: Option<Arc<Vec<crate::indexer::LocalExtension>>>,
    inference_cache: RefCell<FxHashMap<(usize, usize, u16), Type>>,
    /// Per-node implicit-receiver contributions, keyed by the boundary node's position + kind +
    /// query depth. Receiver contributions are a pure function of the boundary node within one
    /// file's analysis, so computing them once per node — rather than once per identifier under
    /// that node — is what makes per-identifier receiver queries cheap.
    receiver_cache: RefCell<FxHashMap<(usize, usize, u16, usize), Option<Type>>>,
}

impl FileCtx {
    pub fn new(package: String, imports: Vec<Import>) -> Self {
        FileCtx {
            package,
            imports,
            local_extensions: None,
            inference_cache: RefCell::new(FxHashMap::default()),
            receiver_cache: RefCell::new(FxHashMap::default()),
        }
    }

    pub fn from_tree(tree: &Tree, src: &str) -> Self {
        FileCtx::new(package_of(tree, src), imports_of(tree, src))
    }

    pub fn from_tree_in_file(tree: &Tree, src: &str, file: &str) -> Self {
        let mut ctx = Self::from_tree(tree, src);
        let locals =
            crate::indexer::local_extensions(tree, src, file, &ctx.package, ctx.imports.clone());
        if !locals.is_empty() {
            ctx.local_extensions = Some(Arc::new(locals));
        }
        ctx
    }

    /// Look up an authored local definition without publishing it to the cross-file index.
    pub fn local_entry(&self, definition: &crate::symbol::Def) -> Option<&Entry> {
        self.local_extensions.as_ref()?.iter().find_map(|local| {
            (local.entry.path.as_ref() == definition.file
                && local.entry.sym.start_byte == definition.start_byte
                && local.entry.sym.end_byte == definition.end_byte)
                .then_some(&local.entry)
        })
    }

    pub(crate) fn local_extension_at(&self, start: usize, end: usize) -> bool {
        self.local_extensions.as_ref().is_some_and(|locals| {
            locals
                .iter()
                .any(|local| local.entry.sym.start_byte == start && local.entry.sym.end_byte == end)
        })
    }
}

/// Infer the type of expression `node`. Returns [`Type::Unknown`] when it can't be determined
/// (never an error).
pub fn infer(index: &dyn InferenceIndex, node: Node, src: &str, ctx: &FileCtx) -> Type {
    let cache_key = (node.start_byte(), node.end_byte(), node.kind_id());
    if let Some(ty) = ctx.inference_cache.borrow().get(&cache_key).cloned() {
        return ty;
    }
    let mut child_infer = LocalChildInfer;
    let ty = infer_depth_with(index, node, src, ctx, 0, &mut child_infer);
    ctx.inference_cache
        .borrow_mut()
        .insert(cache_key, ty.clone());
    ty
}

/// Boundary for recursive expression inference. The default implementation recurses locally; Salsa
/// provides an implementation that re-enters by stable expression key for selected child nodes.
pub trait ChildInfer {
    fn infer_child(
        &mut self,
        index: &dyn InferenceIndex,
        node: Node<'_>,
        src: &str,
        ctx: &FileCtx,
        depth: usize,
    ) -> Type;
}

struct LocalChildInfer;

impl ChildInfer for LocalChildInfer {
    fn infer_child(
        &mut self,
        index: &dyn InferenceIndex,
        node: Node<'_>,
        src: &str,
        ctx: &FileCtx,
        depth: usize,
    ) -> Type {
        infer_depth_with(index, node, src, ctx, depth, self)
    }
}

pub fn infer_with_child_infer<R: ChildInfer>(
    index: &dyn InferenceIndex,
    node: Node,
    src: &str,
    ctx: &FileCtx,
    depth: usize,
    child_infer: &mut R,
) -> Type {
    let cache_key = (node.start_byte(), node.end_byte(), node.kind_id());
    if let Some(ty) = ctx.inference_cache.borrow().get(&cache_key).cloned() {
        return ty;
    }
    let ty = infer_depth_with(index, node, src, ctx, depth, child_infer);
    ctx.inference_cache
        .borrow_mut()
        .insert(cache_key, ty.clone());
    ty
}

fn infer_depth_with<R: ChildInfer>(
    index: &dyn InferenceIndex,
    node: Node,
    src: &str,
    ctx: &FileCtx,
    depth: usize,
    child_infer: &mut R,
) -> Type {
    if depth > MAX_DEPTH {
        return Type::Unknown;
    }
    match node.kind() {
        "string_literal" => intrinsic_type(index, "String"),
        "character_literal" => intrinsic_type(index, "Char"),
        "number_literal" => {
            let t = node_text(node, src);
            let name = if t.ends_with('L') || t.ends_with('l') {
                "Long"
            } else {
                "Int"
            };
            intrinsic_type(index, name)
        }
        "float_literal" => {
            let t = node_text(node, src);
            let name = if t.ends_with('f') || t.ends_with('F') {
                "Float"
            } else {
                "Double"
            };
            intrinsic_type(index, name)
        }
        // Defensive: should the grammar ever expose a boolean_literal kind.
        "boolean_literal" => intrinsic_type(index, "Boolean"),
        "identifier" => infer_identifier(index, node, src, ctx, depth, child_infer),
        "this_expression" if node_text(node, src) == "this" => {
            implicit_receiver_types(index, node, src, ctx, depth)
                .into_iter()
                .next()
                .unwrap_or(Type::Unknown)
        }
        "this_expression" => {
            if let Some(t) = lambda_receiver_type(index, node, src, ctx, depth, child_infer) {
                t
            } else {
                match resolve::enclosing_type_name(node, src) {
                    Some(name) => resolve_type_name(index, &name, ctx, false),
                    None => Type::Unknown,
                }
            }
        }
        "super_expression" => super_receiver_type(index, node, src, ctx, None),
        "parenthesized_expression" => match node.named_child(0) {
            Some(inner) => child_infer.infer_child(index, inner, src, ctx, depth + 1),
            None => Type::Unknown,
        },
        // Elvis `a ?: b`: the non-null type of the left operand (best-effort; we don't unify with
        // the right). `?:` is an anonymous operator token, so detect it by scanning all children.
        // Other binary operators don't yield a useful receiver type — leave them Unknown.
        "binary_expression" if has_child_token(node, "?:") => match node.named_child(0) {
            Some(left) => child_infer
                .infer_child(index, left, src, ctx, depth + 1)
                .into_non_null(),
            None => Type::Unknown,
        },
        // Unary, incl. the non-null assertion `a!!`: the argument's type with nullability stripped.
        // (`-n`/`!b` keep the argument's type too, which is correct for Int/Boolean.)
        "unary_expression" => match node.named_child(0) {
            Some(arg) => child_infer
                .infer_child(index, arg, src, ctx, depth + 1)
                .into_non_null(),
            None => Type::Unknown,
        },
        // A cast `x as T` (or `x as? T`) has the cast-target type T.
        "as_expression" => match node.child_by_field_name("right") {
            Some(ty) => resolve_narrowed_type(index, ty, src, ctx).unwrap_or(Type::Unknown),
            None => Type::Unknown,
        },
        "call_expression" => infer_call(index, node, src, ctx, depth, child_infer),
        "callable_reference" => infer_class_literal(index, node, src, ctx),
        "navigation_expression" if has_child_token(node, "::") => {
            infer_class_literal(index, node, src, ctx)
        }
        "navigation_expression" => infer_navigation(index, node, src, ctx, depth, child_infer),
        _ => Type::Unknown,
    }
}

fn infer_class_literal(index: &dyn InferenceIndex, node: Node, src: &str, ctx: &FileCtx) -> Type {
    if node.has_error() || !has_child_token(node, "::") {
        return Type::Unknown;
    }
    let class_keyword = match node.kind() {
        "callable_reference" => has_child_token(node, "class"),
        // The pinned grammar prefers navigation_expression over callable_reference here.
        "navigation_expression" => node.named_child(1).is_some_and(|selector| {
            selector.kind() == "identifier" && node_text(selector, src) == "class"
        }),
        _ => false,
    };
    if !class_keyword {
        return Type::Unknown;
    }
    let Some(receiver) = node.named_child(0) else {
        return Type::Unknown;
    };
    let scope = crate::indexer::TypeScope::with_default_imports(
        &ctx.package,
        ctx.imports.clone(),
        crate::defaults::DEFAULT_IMPORT_PACKAGES,
    );
    let Some((first, reference)) = named_class_literal_receiver(receiver, src, &scope) else {
        return Type::Unknown;
    };
    if let Some((_, kind)) = resolve::local_decl(first, node_text(first, src), UseKind::Value, src)
    {
        if matches!(
            kind,
            SymbolKind::Parameter
                | SymbolKind::LocalVariable
                | SymbolKind::Property
                | SymbolKind::TypeParameter
        ) {
            // Bound expression and reified type-parameter literals need their own type semantics.
            return Type::Unknown;
        }
    }
    let target = resolve_type_ref(index, &reference, ctx, 0);
    let Some(declaration) = type_declaration_shape(index, &target) else {
        return Type::Unknown;
    };
    if !matches!(
        declaration.sym.kind,
        SymbolKind::Class | SymbolKind::Interface | SymbolKind::Object | SymbolKind::EnumClass
    ) {
        return Type::Unknown;
    }
    if !declaration.sym.type_params.is_empty() {
        // Generic named literals need star-projection semantics, not an invented raw argument list.
        return Type::Unknown;
    }
    Type::Class {
        name: "KClass".into(),
        package: Some("kotlin.reflect".into()),
        container: None,
        nullable: false,
        args: vec![target],
    }
}

fn named_class_literal_receiver<'t>(
    mut receiver: Node<'t>,
    src: &str,
    scope: &crate::indexer::TypeScope,
) -> Option<(Node<'t>, TypeRef)> {
    if receiver.kind() == "user_type" {
        if child_of_kind(receiver, "type_arguments").is_some() {
            return None;
        }
        return Some((
            first_ident(receiver)?,
            crate::indexer::type_ref_from(receiver, src, Some(scope))?,
        ));
    }
    let mut names = Vec::new();
    while receiver.kind() == "navigation_expression" {
        if !has_child_token(receiver, ".") || receiver.named_child_count() != 2 {
            return None;
        }
        let selector = receiver.named_child(1)?;
        if selector.kind() != "identifier" {
            return None;
        }
        names.push(node_text(selector, src));
        receiver = receiver.named_child(0)?;
    }
    if receiver.kind() != "identifier" {
        return None;
    }
    names.push(node_text(receiver, src));
    names.reverse();
    Some((
        receiver,
        scope.type_ref(&names.join("."), false, Vec::new()),
    ))
}

// ---------------------------------------------------------------------------------------------
// Flow typing (Stage 4): smart casts (`is` in if/when, `as`) and `it`-based scope functions.
// ---------------------------------------------------------------------------------------------

/// The smart-cast narrowed type for identifier `name` at `ident`, from an enclosing `if (name is T)`
/// then-branch or `when(name){ is T -> }` entry that contains `ident`. Negated checks (`!is`) and
/// the else-branch are never narrowed (they'd be a wrong type).
fn narrowed_type(
    index: &dyn InferenceIndex,
    ident: Node,
    name: &str,
    src: &str,
    ctx: &FileCtx,
) -> Option<Type> {
    let mut cur = ident.parent();
    while let Some(n) = cur {
        match n.kind() {
            "if_expression" => {
                if let Some(t) = if_narrowing(index, n, ident, name, src, ctx) {
                    return Some(t);
                }
            }
            "when_entry" => {
                if let Some(t) = when_entry_narrowing(index, n, ident, name, src, ctx) {
                    return Some(t);
                }
            }
            // `x is T && <…x…>`: the right operand of `&&` runs only when the left conjunct is true.
            "binary_expression" => {
                if let Some(t) = and_short_circuit_narrowing(index, n, ident, name, src, ctx) {
                    return Some(t);
                }
            }
            _ => {}
        }
        cur = n.parent();
    }
    // Fallback: a preceding `if (name !is T) <return/throw/break/continue>` narrows the rest of the
    // block to T. Ancestor (if/when/&&) narrowing above takes precedence when both apply.
    early_return_narrowing(index, ident, name, src, ctx)
}

/// `if (name !is T) <terminating>` earlier in the same block narrows `name` to `T` for every
/// statement after it (the only way to reach them is if the `!is` was false). Bounded
/// preceding-sibling scan — no CFG. Stability-gated from the guard position.
fn early_return_narrowing(
    index: &dyn InferenceIndex,
    ident: Node,
    name: &str,
    src: &str,
    ctx: &FileCtx,
) -> Option<Type> {
    let (block, stmt) = enclosing_block_and_stmt(ident)?;
    let mut cursor = block.walk();
    let mut best: Option<(Type, usize)> = None; // (narrowed type, guard start byte); latest wins
    for child in block.named_children(&mut cursor) {
        if child.start_byte() >= stmt.start_byte() {
            break; // only statements strictly before the one containing the use
        }
        if child.kind() == "if_expression" {
            if let Some(t) = terminating_guard_narrows(index, child, name, src, ctx) {
                best = Some((t, child.start_byte()));
            }
        }
    }
    let (narrowed, guard_start) = best?;
    if !is_stable_for_narrowing(ident, name, guard_start, src) {
        return None;
    }
    Some(narrowed)
}

/// The (`block`, direct-child statement) pair enclosing `ident` — the statement is the block child
/// the use sits within, so preceding siblings can be scanned.
fn enclosing_block_and_stmt<'t>(ident: Node<'t>) -> Option<(Node<'t>, Node<'t>)> {
    let mut node = ident;
    while let Some(parent) = node.parent() {
        if parent.kind() == "block" {
            return Some((parent, node));
        }
        node = parent;
    }
    None
}

/// An `if_expression` of the form `if (name !is T) <terminating>` → narrows the fallthrough to `T`.
fn terminating_guard_narrows(
    index: &dyn InferenceIndex,
    if_expr: Node,
    name: &str,
    src: &str,
    ctx: &FileCtx,
) -> Option<Type> {
    let cond = if_expr.child_by_field_name("condition")?;
    let then_branch = if_expr.named_child(1)?;
    if !is_terminating(then_branch, src) {
        return None;
    }
    negated_guard_type(index, cond, name, src, ctx)
}

/// A statement that exits the enclosing block: `return`/`throw`, `break`/`continue`, a known
/// `Nothing`-returning call like `exitProcess`, or a block whose final statement does one of those.
fn is_terminating(node: Node, src: &str) -> bool {
    match node.kind() {
        "return_expression" | "throw_expression" => true,
        "identifier" => matches!(node_text(node, src), "break" | "continue"),
        "call_expression" => terminating_call_name(node, src)
            .is_some_and(|name| matches!(name, "exitProcess" | "error" | "TODO" | "todo" | "fail")),
        "block" => node
            .named_child(node.named_child_count().saturating_sub(1))
            .is_some_and(|c| is_terminating(c, src)),
        _ => false,
    }
}

fn terminating_call_name<'t>(node: Node<'t>, src: &'t str) -> Option<&'t str> {
    let callee = node.named_child(0)?;
    match callee.kind() {
        "identifier" => Some(node_text(callee, src)),
        "navigation_expression" => callee
            .named_child(1)
            .filter(|child| child.kind() == "identifier")
            .map(|child| node_text(child, src)),
        _ => None,
    }
}

/// The type `name` is narrowed to when the *negation* of `guard` holds — i.e. `name !is T` (a
/// negated `is_expression`) → `T`. For fallthrough after `if (guard) <terminating>`, a disjunction
/// such as `a !is T || b !is U` also narrows each name on the fallthrough path.
fn negated_guard_type(
    index: &dyn InferenceIndex,
    guard: Node,
    name: &str,
    src: &str,
    ctx: &FileCtx,
) -> Option<Type> {
    match guard.kind() {
        "is_expression" => {
            if !has_child_token(guard, "!is") {
                return None;
            }
            let left = guard.child_by_field_name("left")?;
            if node_text(left, src) != name {
                return None;
            }
            resolve_narrowed_type(index, guard.child_by_field_name("right")?, src, ctx)
        }
        "binary_expression" if has_child_token(guard, "||") => guard
            .child_by_field_name("left")
            .and_then(|left| negated_guard_type(index, left, name, src, ctx))
            .or_else(|| {
                guard
                    .child_by_field_name("right")
                    .and_then(|right| negated_guard_type(index, right, name, src, ctx))
            }),
        "parenthesized_expression" => guard
            .named_child(0)
            .and_then(|node| negated_guard_type(index, node, name, src, ctx)),
        _ => None,
    }
}

/// Narrowing from `if (<guard on name>) <then>`: the guard may be a bare `name is T` OR a `&&`
/// compound containing one (`if (name is T && cond)` / `if (cond && name is T)`). Only the then-branch
/// is narrowed, only for the positive `is`, only for a stable binding.
fn if_narrowing(
    index: &dyn InferenceIndex,
    if_expr: Node,
    ident: Node,
    name: &str,
    src: &str,
    ctx: &FileCtx,
) -> Option<Type> {
    let cond = if_expr.child_by_field_name("condition")?;
    let narrowed = guard_type(index, cond, name, src, ctx)?;
    // The then-branch is the named child right after the condition (index 1); the else (if any) is
    // index 2. Only narrow when `ident` is within the then-branch.
    let then_branch = if_expr.named_child(1)?;
    if ident.start_byte() < then_branch.start_byte() || ident.end_byte() > then_branch.end_byte() {
        return None;
    }
    if !is_stable_for_narrowing(ident, name, cond.start_byte(), src) {
        return None;
    }
    Some(narrowed)
}

/// If boolean `guard` proves `name is T` — directly (`name is T`), through `&&` conjuncts
/// (`a && name is T && b`), or through parentheses — return the narrowed simple type name. Positive
/// `is` only; `!is` and other operators do not narrow here.
fn guard_type(
    index: &dyn InferenceIndex,
    guard: Node,
    name: &str,
    src: &str,
    ctx: &FileCtx,
) -> Option<Type> {
    match guard.kind() {
        "is_expression" => {
            if has_child_token(guard, "!is") {
                return None;
            }
            let left = guard.child_by_field_name("left")?;
            if node_text(left, src) != name {
                return None;
            }
            resolve_narrowed_type(index, guard.child_by_field_name("right")?, src, ctx)
        }
        "binary_expression" if has_child_token(guard, "&&") => guard
            .child_by_field_name("left")
            .and_then(|l| guard_type(index, l, name, src, ctx))
            .or_else(|| {
                guard
                    .child_by_field_name("right")
                    .and_then(|r| guard_type(index, r, name, src, ctx))
            }),
        "parenthesized_expression" => guard
            .named_child(0)
            .and_then(|n| guard_type(index, n, name, src, ctx)),
        _ => None,
    }
}

/// `<guard on name> && <…name…>`: within the right operand of `&&`, narrow `name` when the left
/// operand proves `name is T` (the left conjunct is true before the right runs). Stability-gated.
fn and_short_circuit_narrowing(
    index: &dyn InferenceIndex,
    be: Node,
    ident: Node,
    name: &str,
    src: &str,
    ctx: &FileCtx,
) -> Option<Type> {
    if !has_child_token(be, "&&") {
        return None;
    }
    let left = be.child_by_field_name("left")?;
    let right = be.child_by_field_name("right")?;
    if ident.start_byte() < right.start_byte() || ident.end_byte() > right.end_byte() {
        return None;
    }
    let narrowed = guard_type(index, left, name, src, ctx)?;
    if !is_stable_for_narrowing(ident, name, be.start_byte(), src) {
        return None;
    }
    Some(narrowed)
}

/// Narrowing from a `when` entry:
/// - `when(name) { is T -> <body> }`
/// - `when { name is T && ... -> <body> }`
///
/// Only applies when `ident` is in the entry body (not inside the condition itself).
fn when_entry_narrowing(
    index: &dyn InferenceIndex,
    entry: Node,
    ident: Node,
    name: &str,
    src: &str,
    ctx: &FileCtx,
) -> Option<Type> {
    let cond = entry.child_by_field_name("condition")?;
    // `ident` must be in the body, not inside the condition.
    if ident.start_byte() >= cond.start_byte() && ident.end_byte() <= cond.end_byte() {
        return None;
    }
    let when_expr = entry.parent()?;
    if when_expr.kind() != "when_expression" {
        return None;
    }
    if let Some(subject) = child_of_kind(when_expr, "when_subject") {
        if cond.kind() != "type_test" {
            return None;
        }
        let subject_id = first_ident(subject)?;
        if node_text(subject_id, src) != name {
            return None;
        }
        let ut = child_of_kind(cond, "user_type")?;
        if !is_stable_for_narrowing(ident, name, cond.start_byte(), src) {
            return None;
        }
        return resolve_narrowed_type(index, ut, src, ctx);
    }

    let narrowed = guard_type(index, cond, name, src, ctx)?;
    if !is_stable_for_narrowing(ident, name, cond.start_byte(), src) {
        return None;
    }
    Some(narrowed)
}

/// Whether `name` (used at `ident`) is stable enough to smart-cast, given the narrowing check began at
/// byte `check_start`. Kotlin only smart-casts stable values; narrowing an unstable one is unsound (a
/// wrong type → wrong members). Rules: a parameter is always stable (params are immutable); a `val`
/// local is stable; a `var` local is stable only if it is not reassigned textually between the check
/// and the use; anything we can't resolve to a local/param (a member/property that might have a custom
/// getter) is treated as not stable. Conservative by design — refusing to narrow falls back to the
/// declared type, never a wrong one.
fn is_stable_for_narrowing(ident: Node, name: &str, check_start: usize, src: &str) -> bool {
    let Some((decl, kind)) = resolve::local_decl(ident, name, UseKind::Value, src) else {
        return false;
    };
    match kind {
        SymbolKind::Parameter => true,
        SymbolKind::LocalVariable => {
            // `val` vs `var`: decl is the binder identifier; go up to variable_declaration, then to
            // property_declaration, and test the anonymous `val` token child.
            let is_val = decl
                .parent()
                .and_then(|vd| vd.parent())
                .filter(|p| p.kind() == "property_declaration")
                .is_some_and(|p| has_child_token(p, "val"));
            if is_val {
                return true;
            }
            // `var`: stable only if not reassigned textually between the check and the use.
            !reassigned_between(ident, name, check_start, ident.start_byte(), src)
        }
        _ => false,
    }
}

/// Whether `name` is the target of an `assignment` textually in the byte window `(lo, hi)` anywhere in
/// the nearest enclosing function body / file. A conservative over-approximation of "reassigned
/// between the check and the use" (no CFG): any textual reassignment in the window refuses narrowing.
fn reassigned_between(ident: Node, name: &str, lo: usize, hi: usize, src: &str) -> bool {
    let mut scope = ident;
    while let Some(p) = scope.parent() {
        scope = p;
        if matches!(
            scope.kind(),
            "function_body" | "function_declaration" | "source_file"
        ) {
            break;
        }
    }
    has_assignment_to(scope, ident, name, lo, hi, src)
}

fn has_assignment_to(node: Node, ident: Node, name: &str, lo: usize, hi: usize, src: &str) -> bool {
    if node.kind() == "assignment" {
        if let Some(left) = node.child_by_field_name("left") {
            let s = node.start_byte();
            let use_is_in_rhs = node.child_by_field_name("right").is_some_and(|right| {
                ident.start_byte() >= right.start_byte() && ident.end_byte() <= right.end_byte()
            });
            if left.kind() == "identifier"
                && node_text(left, src) == name
                && s > lo
                && s < hi
                && !use_is_in_rhs
            {
                return true;
            }
        }
    }
    let mut cursor = node.walk();
    for ch in node.named_children(&mut cursor) {
        if has_assignment_to(ch, ident, name, lo, hi, src) {
            return true;
        }
    }
    false
}

fn resolve_narrowed_type(
    index: &dyn InferenceIndex,
    node: Node,
    src: &str,
    ctx: &FileCtx,
) -> Option<Type> {
    let ut = match node.kind() {
        "user_type" => node,
        "nullable_type" => child_of_kind(node, "user_type")?,
        _ => return None,
    };
    let names = user_type_identifiers(ut, src);
    let (simple, prefix) = names.split_last()?;
    let package = resolve_narrowed_package(index, prefix, simple, ctx);
    Some(Type::Class {
        name: (*simple).to_string(),
        package,
        container: qualified_container_prefix(prefix),
        nullable: node.kind() == "nullable_type",
        args: Vec::new(),
    })
}

fn user_type_identifiers<'a>(ut: Node, src: &'a str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut cursor = ut.walk();
    for child in ut.named_children(&mut cursor) {
        if child.kind() == "identifier" {
            out.push(node_text(child, src));
        }
    }
    out
}

fn qualified_container_prefix(parts: &[&str]) -> Option<String> {
    let first_type_segment = parts.iter().position(|part| {
        part.chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_uppercase())
    })?;
    Some(parts[first_type_segment..].join("."))
}

fn qualified_type_key(name: &str, container: Option<&str>) -> String {
    match container {
        Some(container) => format!("{container}.{name}"),
        None => name.to_string(),
    }
}

pub fn receiver_root_key(index: &dyn InferenceIndex, recv_ty: &Type) -> Option<String> {
    let root = recv_ty.name()?;
    if let Some(container) = recv_ty.container() {
        return Some(qualified_type_key(root, Some(container)));
    }
    let pkg = recv_ty.package()?;
    let matches = index
        .lookup_type(root)
        .into_iter()
        .filter(|entry| entry.sym.package == pkg)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [only] => Some(qualified_type_key(root, only.sym.container.as_deref())),
        _ => Some(root.to_string()),
    }
}

fn resolve_narrowed_package(
    index: &dyn InferenceIndex,
    prefix: &[&str],
    simple: &str,
    ctx: &FileCtx,
) -> Option<String> {
    if prefix.is_empty() {
        return resolve_package(index, simple, ctx);
    }
    if prefix.len() == 1 {
        let outer = prefix[0];
        if let Some(pkg) = resolve_package(index, outer, ctx) {
            let has_nested = index.lookup_by_name(simple).iter().any(|entry| {
                entry.sym.kind.is_type_like()
                    && entry.sym.container.as_deref() == Some(outer)
                    && entry.sym.package == pkg
            });
            if has_nested {
                return Some(pkg);
            }
        }
    }
    resolve_package(index, simple, ctx)
}

/// Collection transform/query operations whose lambda binds `it` to the receiver's ELEMENT type
/// (`Iterable<T>.fn(… (T) -> …)`). For these, `it` is the receiver's single type argument. (`Map`
/// ops bind `it` to an entry, not a single type arg — they're excluded by the single-arg guard.)
const ELEMENT_LAMBDA_OPS: &[&str] = &[
    "map",
    "mapNotNull",
    "mapIndexed",
    "filter",
    "filterNot",
    "filterNotNull",
    "forEach",
    "onEach",
    "flatMap",
    "any",
    "all",
    "none",
    "find",
    "firstOrNull",
    "first",
    "last",
    "lastOrNull",
    "count",
    "sumOf",
    "maxByOrNull",
    "minByOrNull",
    "sortedBy",
    "sortedByDescending",
    "groupBy",
    "associateBy",
    "associateWith",
    "takeWhile",
    "dropWhile",
    "partition",
    "indexOfFirst",
    "single",
    "singleOrNull",
    "collect",
    "collectIndexed",
    "forEachIndexed",
    "fold",
    "reduce",
    "maxOf",
    "minOf",
    "withIndex",
];

/// Scope-style operations whose trailing lambda binds `this` to the receiver.
const RECEIVER_LAMBDA_OPS: &[&str] = &["apply", "run", "runCatching"];

/// Operations whose generic return type is determined by the trailing lambda's result.
const RETURN_BINDING_LAMBDA_OPS: &[&str] = &["let", "map", "mapCatching", "run", "runCatching"];

/// The type of `this` inside a receiver-style scope lambda such as `recv.apply { this }`
/// or `recv.run { this }`.
fn lambda_receiver_type(
    index: &dyn InferenceIndex,
    this_expr: Node,
    src: &str,
    ctx: &FileCtx,
    depth: usize,
    child_infer: &mut impl ChildInfer,
) -> Option<Type> {
    let mut cur = this_expr.parent();
    while let Some(n) = cur {
        if n.kind() == "lambda_literal" {
            let ancestors = crate::parser::ancestors_of(n);
            return lambda_receiver_for_this(index, n, &ancestors, src, ctx, depth, child_infer);
        }
        cur = n.parent();
    }
    None
}

/// The implicit receiver stack visible at `node`, ordered innermost-first.
///
/// This includes receiver-style trailing lambdas (`recv.apply { ... }`), the receiver of an
/// enclosing extension function, and lexical class/object receivers so bare member references can
/// resolve through every Kotlin implicit-receiver layer.
pub fn implicit_receiver_types(
    index: &dyn InferenceIndex,
    node: Node,
    src: &str,
    ctx: &FileCtx,
    depth: usize,
) -> Vec<Type> {
    let ancestors = crate::parser::ancestors_of(node);
    implicit_receiver_types_with_ancestors(index, &ancestors, src, ctx, depth)
}

/// Ancestor-slice variant of [`implicit_receiver_types`]: `ancestors` is the query node's
/// ancestor chain, root-first (its `last()` is the node's parent). Per-boundary contributions
/// are memoized in [`FileCtx`], so repeated queries for identifiers under the same lambda,
/// function, or class pay one slice walk instead of full receiver recomputation.
pub(crate) fn implicit_receiver_types_with_ancestors(
    index: &dyn InferenceIndex,
    ancestors: &[Node],
    src: &str,
    ctx: &FileCtx,
    depth: usize,
) -> Vec<Type> {
    let mut out = Vec::new();
    let mut child_infer = LocalChildInfer;
    for (i, n) in ancestors.iter().enumerate().rev() {
        let ty = match n.kind() {
            "lambda_literal" => cached_receiver_contribution(
                index,
                *n,
                &ancestors[..i],
                src,
                ctx,
                depth,
                &mut child_infer,
            ),
            "function_declaration" => cached_fn_receiver_contribution(index, *n, src, ctx, depth),
            "class_declaration" | "object_declaration" => {
                cached_class_receiver_contribution(index, *n, src, ctx)
            }
            _ => None,
        };
        if let Some(ty) = ty {
            out.push(ty);
        }
    }
    out
}

fn receiver_cache_lookup(ctx: &FileCtx, node: Node, depth: usize) -> Option<Option<Type>> {
    ctx.receiver_cache
        .borrow()
        .get(&(node.start_byte(), node.end_byte(), node.kind_id(), depth))
        .cloned()
}

fn receiver_cache_store(ctx: &FileCtx, node: Node, depth: usize, ty: Option<Type>) -> Option<Type> {
    ctx.receiver_cache.borrow_mut().insert(
        (node.start_byte(), node.end_byte(), node.kind_id(), depth),
        ty.clone(),
    );
    ty
}

fn cached_receiver_contribution(
    index: &dyn InferenceIndex,
    lambda: Node,
    lambda_ancestors: &[Node],
    src: &str,
    ctx: &FileCtx,
    depth: usize,
    child_infer: &mut impl ChildInfer,
) -> Option<Type> {
    if let Some(hit) = receiver_cache_lookup(ctx, lambda, depth) {
        return hit;
    }
    let ty = lambda_receiver_for_this(
        index,
        lambda,
        lambda_ancestors,
        src,
        ctx,
        depth,
        child_infer,
    );
    receiver_cache_store(ctx, lambda, depth, ty)
}

fn cached_fn_receiver_contribution(
    index: &dyn InferenceIndex,
    declaration: Node,
    src: &str,
    ctx: &FileCtx,
    depth: usize,
) -> Option<Type> {
    if let Some(hit) = receiver_cache_lookup(ctx, declaration, depth) {
        return hit;
    }
    let ty = extension_function_receiver_ref(declaration, src)
        .map(|receiver| resolve_type_ref(index, &receiver, ctx, depth + 1));
    receiver_cache_store(ctx, declaration, depth, ty)
}

fn cached_class_receiver_contribution(
    index: &dyn InferenceIndex,
    declaration: Node,
    src: &str,
    ctx: &FileCtx,
) -> Option<Type> {
    if let Some(hit) = receiver_cache_lookup(ctx, declaration, 0) {
        return hit;
    }
    let ty = name_field(declaration)
        .map(|name_node| node_text(name_node, src))
        .map(|name| resolve_type_name(index, name, ctx, false));
    receiver_cache_store(ctx, declaration, 0, ty)
}

/// `super` starts at the enclosing declaration's direct supertypes, never its own overrides.
/// A written qualifier/outer label must identify an actual direct supertype. With several direct
/// supertypes, a selector must identify a unique member provider. Completion passes the real prefix
/// separately so recovery placeholders never become part of semantic lookup.
pub fn super_receiver_type(
    index: &dyn InferenceIndex,
    node: Node,
    src: &str,
    ctx: &FileCtx,
    member_prefix: Option<&str>,
) -> Type {
    let mut supers = super_receivers(index, node, src, ctx);
    let name = if let Some(prefix) = member_prefix {
        members::MemberName::Prefix(prefix)
    } else if let Some(selector) = node
        .parent()
        .filter(|parent| parent.kind() == "navigation_expression")
        .and_then(|navigation| navigation.named_child(1))
    {
        members::MemberName::Exact(node_text(selector, src))
    } else {
        return if supers.len() == 1 && type_head_is_known(index, &supers[0]) {
            supers.pop().unwrap()
        } else {
            Type::Unknown
        };
    };
    members::unique_super_receiver(index, supers, name, ctx)
}

/// The actual immediate supertypes, including generic arguments and explicit qualifier/label.
pub fn super_receivers(
    index: &dyn InferenceIndex,
    node: Node,
    src: &str,
    ctx: &FileCtx,
) -> Vec<Type> {
    let label = child_of_kind(node, "identifier").map(|label| node_text(label, src));
    let mut enclosing = node.parent();
    let declaration = loop {
        let Some(current) = enclosing else {
            return Vec::new();
        };
        if matches!(current.kind(), "class_declaration" | "object_declaration")
            && label.is_none_or(|label| {
                name_field(current).is_some_and(|name| node_text(name, src) == label)
            })
        {
            break current;
        }
        enclosing = current.parent();
    };
    let Some(receiver) = cached_class_receiver_contribution(index, declaration, src, ctx) else {
        return Vec::new();
    };
    let qualifier = child_of_kind(node, "user_type")
        .and_then(|ty| crate::indexer::syntax_type_ref(ty, src))
        .map(|ty| resolve_type_ref(index, &ty, ctx, 0));
    let mut supers = direct_supertypes(index, &receiver);
    if let Some(qualifier) = qualifier {
        if qualifier.name().is_none() {
            return Vec::new();
        }
        supers.retain(|ty| {
            ty.name() == qualifier.name()
                && ty.package() == qualifier.package()
                && ty.container() == qualifier.container()
        });
    }
    supers.dedup();
    supers
}

/// Return the receiver written before an enclosing function's name (`fun Route.setup()`). The
/// return type and parameter types occur after the name boundary and must not be mistaken for a
/// lexical receiver.
fn extension_function_receiver_ref(declaration: Node, src: &str) -> Option<TypeRef> {
    let name = name_field(declaration)?;
    let mut cursor = declaration.walk();
    for child in declaration.named_children(&mut cursor) {
        if child == name {
            return None;
        }
        if matches!(child.kind(), "user_type" | "nullable_type") {
            return crate::indexer::syntax_type_ref(child, src);
        }
    }
    None
}

/// The type of a lambda parameter `name` (implicit `it` OR a named param like `{ user -> … }`) inside
/// the trailing lambda of a scope/collection call:
/// - `recv.let { … }` / `recv.also { … }` → the parameter is `recv`'s type.
/// - `recv.map { … }` / `filter`/`forEach`/… (see `ELEMENT_LAMBDA_OPS`) → the parameter is `recv`'s
///   single element type (`List<Foo>` → `Foo`), when the receiver has exactly one type argument.
///
/// Implicit `it` binds to the innermost lambda; a named parameter binds to whichever enclosing lambda
/// declares it (so we walk outward until that lambda is found).
fn lambda_param_type(
    index: &dyn InferenceIndex,
    ident: Node,
    name: &str,
    src: &str,
    ctx: &FileCtx,
    depth: usize,
    child_infer: &mut impl ChildInfer,
) -> Option<Type> {
    let implicit = name == "it";
    let mut cur = ident.parent();
    while let Some(n) = cur {
        if n.kind() == "lambda_literal" {
            let is_this_lambdas_param = implicit || lambda_declares_param(n, name, src);
            if is_this_lambdas_param {
                return lambda_receiver_or_element(index, n, src, ctx, depth, child_infer);
            }
            if implicit {
                return None; // `it` belongs to the innermost lambda; if it's not the op, no type
            }
            // a named parameter not declared here — keep walking outward to its declaring lambda
        }
        cur = n.parent();
    }
    None
}

/// Whether `lambda` declares a value parameter named `name` (in its `lambda_parameters`).
fn lambda_declares_param(lambda: Node, name: &str, src: &str) -> bool {
    let Some(lp) = child_of_kind(lambda, "lambda_parameters") else {
        return false;
    };
    let mut cursor = lp.walk();
    for c in lp.named_children(&mut cursor) {
        let matched = match c.kind() {
            "identifier" => node_text(c, src) == name,
            // a destructured/annotated param wraps the binder in a variable_declaration
            "variable_declaration" => first_ident(c).is_some_and(|id| node_text(id, src) == name),
            _ => false,
        };
        if matched {
            return true;
        }
    }
    false
}

/// Given a `lambda_literal` that is the trailing lambda of a `recv.op { … }` call, the type its
/// parameter takes: the receiver type for `let`/`also`, or the receiver's single element type for an
/// `ELEMENT_LAMBDA_OPS` op (`map`/`filter`/…). `None` for any other shape.
fn lambda_receiver_or_element(
    index: &dyn InferenceIndex,
    lambda: Node,
    src: &str,
    ctx: &FileCtx,
    depth: usize,
    child_infer: &mut impl ChildInfer,
) -> Option<Type> {
    let call = lambda.parent()?.parent()?; // lambda_literal -> annotated_lambda -> call_expression
    if call.kind() != "call_expression" {
        return None;
    }
    let callee = call.named_child(0)?;
    if callee.kind() != "navigation_expression" {
        return None;
    }
    if let Some((receiver, selection)) =
        local_lambda_call_selection(index, call, callee, src, ctx, depth, child_infer)
    {
        let arguments = normalized_call_arguments(
            call,
            src,
            synth_arg_types(index, call, src, ctx, depth, child_infer),
        );
        let mut result = None;
        for entry in selection.entries {
            if trailing_lambda_receiver_ref(index, entry, ctx).is_some() {
                return None;
            }
            let (params, _) = members::function_type_signature(entry.sym.params.last()?)?;
            let [parameter] = params else {
                return None;
            };
            let ty = resolve_trailing_lambda_receiver_candidate(
                index,
                entry,
                parameter,
                Some(&receiver),
                &arguments,
                ctx,
                depth,
            )?;
            if result.as_ref().is_some_and(|previous| previous != &ty) {
                return None;
            }
            result = Some(ty);
        }
        return result;
    }
    let sel = node_text(callee.named_child(1)?, src);
    let recv = callee.named_child(0)?;
    let recv_ty = child_infer.infer_child(index, recv, src, ctx, depth + 1);
    if sel == "let" || sel == "also" {
        recv_ty.name().is_some().then_some(recv_ty)
    } else if ELEMENT_LAMBDA_OPS.contains(&sel) {
        match recv_ty.args() {
            [elem] if elem.name().is_some() => Some(elem.clone()),
            _ => None,
        }
    } else {
        None
    }
}

/// Given a `lambda_literal` that is the trailing lambda of a receiver-style scope call, return the
/// type that `this` refers to inside the lambda body.
fn lambda_receiver_for_this(
    index: &dyn InferenceIndex,
    _lambda: Node,
    lambda_ancestors: &[Node],
    src: &str,
    ctx: &FileCtx,
    depth: usize,
    child_infer: &mut impl ChildInfer,
) -> Option<Type> {
    let call = lambda_enclosing_call(lambda_ancestors)?;
    let call_ancestors = &lambda_ancestors[..lambda_ancestors.len() - 2];
    let (call, call_ancestors) = outer_trailing_lambda_call_with_ancestors(call, call_ancestors);
    let (callee, callee_ancestors) = callable_callee_with_path(call, call_ancestors)?;
    if let Some((receiver, selection)) =
        local_lambda_call_selection(index, call, callee, src, ctx, depth, child_infer)
    {
        return select_trailing_lambda_receiver_type(
            index,
            selection.entries,
            Some(&receiver),
            call,
            src,
            ctx,
            depth,
            child_infer,
        );
    }
    if let Some(recv_ty) =
        hardcoded_receiver_lambda_type(index, callee, src, ctx, depth, child_infer)
    {
        return Some(recv_ty);
    }
    generic_lambda_receiver_type(
        index,
        call,
        callee,
        &callee_ancestors,
        src,
        ctx,
        depth,
        child_infer,
    )
}

fn local_lambda_call_selection<'a>(
    index: &'a dyn InferenceIndex,
    call: Node,
    callee: Node,
    src: &str,
    ctx: &'a FileCtx,
    depth: usize,
    child_infer: &mut impl ChildInfer,
) -> Option<(Type, members::Selection<'a>)> {
    if ctx.local_extensions.is_none() || callee.kind() != "navigation_expression" {
        return None;
    }
    let receiver = callee.named_child(0)?;
    let name = node_text(callee.named_child(1)?, src);
    let receiver = member_receiver_type(
        receiver,
        child_infer.infer_child(index, receiver, src, ctx, depth + 1),
    );
    let candidates = members::candidates_at(index, &receiver, name, ctx, callee.start_byte());
    if !candidates.has_local_extensions() {
        return None;
    }
    let usage = MemberUse::Call(normalized_call_arguments(
        call,
        src,
        synth_arg_types(index, call, src, ctx, depth, child_infer),
    ));
    let selection = candidates.select(index, &receiver, &usage, ctx);
    (selection.applicability != members::Applicability::Rejected).then_some((receiver, selection))
}

/// The call expression a lambda literal trails: `lambda_literal -> annotated_lambda ->
/// call_expression`, taken from the ancestor slice (`last()` is the lambda's parent).
fn lambda_enclosing_call<'t>(lambda_ancestors: &[Node<'t>]) -> Option<Node<'t>> {
    let call = *lambda_ancestors.get(lambda_ancestors.len().checked_sub(2)?)?;
    if call.kind() != "call_expression" {
        return None;
    }
    Some(call)
}

/// Slice variant of the trailing-lambda climb: walks `call`'s ancestors while they are outer
/// calls whose first child is the current call and which take a trailing lambda. Returns the
/// outermost call together with its remaining ancestor slice.
pub(crate) fn outer_trailing_lambda_call_with_ancestors<'a, 't>(
    mut call: Node<'t>,
    mut ancestors: &'a [Node<'t>],
) -> (Node<'t>, &'a [Node<'t>]) {
    while let Some((&parent, rest)) = ancestors.split_last() {
        if parent.kind() == "call_expression"
            && parent.named_child(0) == Some(call)
            && has_trailing_lambda(parent)
        {
            call = parent;
            ancestors = rest;
        } else {
            break;
        }
    }
    (call, ancestors)
}

/// Descend nested `foo()()` callee chains to the innermost callee, recording the callee's
/// ancestor chain (`call_ancestors` plus each call descended through) for receiver queries.
pub(crate) fn callable_callee_with_path<'t>(
    call: Node<'t>,
    call_ancestors: &[Node<'t>],
) -> Option<(Node<'t>, Vec<Node<'t>>)> {
    let mut ancestors = call_ancestors.to_vec();
    let mut current = call;
    loop {
        ancestors.push(current);
        let callee = current.named_child(0)?;
        if callee.kind() == "call_expression" {
            current = callee;
        } else {
            return Some((callee, ancestors));
        }
    }
}

pub(crate) fn has_trailing_lambda(call: Node) -> bool {
    child_of_kind(call, "annotated_lambda").is_some()
        || child_of_kind(call, "lambda_literal").is_some()
}

fn hardcoded_receiver_lambda_type(
    index: &dyn InferenceIndex,
    callee: Node,
    src: &str,
    ctx: &FileCtx,
    depth: usize,
    child_infer: &mut impl ChildInfer,
) -> Option<Type> {
    match callee.kind() {
        "navigation_expression" => {
            let recv = callee.named_child(0)?;
            let sel = node_text(callee.named_child(1)?, src);
            if !RECEIVER_LAMBDA_OPS.contains(&sel) {
                return None;
            }
            let recv_ty = child_infer.infer_child(index, recv, src, ctx, depth + 1);
            recv_ty.name().is_some().then_some(recv_ty)
        }
        _ => None,
    }
}

fn generic_lambda_receiver_type(
    index: &dyn InferenceIndex,
    call: Node,
    callee: Node,
    callee_ancestors: &[Node],
    src: &str,
    ctx: &FileCtx,
    depth: usize,
    child_infer: &mut impl ChildInfer,
) -> Option<Type> {
    match callee.kind() {
        "identifier" => {
            let name = node_text(callee, src);
            // A bare callable inside a receiver lambda may itself be a member/extension on an
            // enclosing implicit receiver (`post { ok { ... } }`). Follow the same innermost-first
            // precedence as goto before considering ordinary visible top-level functions.
            for implicit_receiver in
                implicit_receiver_types_with_ancestors(index, callee_ancestors, src, ctx, depth + 1)
            {
                let candidates = members::candidates_at(
                    index,
                    &implicit_receiver,
                    name,
                    ctx,
                    callee.start_byte(),
                );
                let (entries, local_selected) = callable_member_entries(
                    candidates,
                    index,
                    &implicit_receiver,
                    call,
                    src,
                    ctx,
                    depth,
                    child_infer,
                );
                let receiver = select_trailing_lambda_receiver_type(
                    index,
                    entries,
                    Some(&implicit_receiver),
                    call,
                    src,
                    ctx,
                    depth,
                    child_infer,
                );
                if local_selected || receiver.is_some() {
                    return receiver;
                }
            }
            let entries = visible_top_level_entries(index, name, ctx, SymbolKind::Function);
            select_trailing_lambda_receiver_type(
                index,
                entries,
                None,
                call,
                src,
                ctx,
                depth,
                child_infer,
            )
        }
        "navigation_expression" => {
            let recv = callee.named_child(0)?;
            let sel = callee.named_child(1)?;
            let (recv_ty, candidates, _) = navigation_member_candidates(
                index,
                recv,
                node_text(sel, src),
                src,
                ctx,
                depth,
                child_infer,
            )?;
            if recv_ty == Type::Unknown {
                if let Some(entries) =
                    resolve::qualified_top_level_function_entries(index, callee, src, ctx)
                {
                    return select_trailing_lambda_receiver_type(
                        index,
                        entries,
                        None,
                        call,
                        src,
                        ctx,
                        depth,
                        child_infer,
                    );
                }
            }
            let (entries, _) = callable_member_entries(
                candidates,
                index,
                &recv_ty,
                call,
                src,
                ctx,
                depth,
                child_infer,
            );
            select_trailing_lambda_receiver_type(
                index,
                entries,
                Some(&recv_ty),
                call,
                src,
                ctx,
                depth,
                child_infer,
            )
        }
        _ => None,
    }
}

/// Whether `node` has a direct (possibly anonymous) child token equal to `token`. Operators like
/// `?:` and `!!` are exposed by tree-sitter as UNNAMED children, invisible to `named_children`, so
/// detecting them requires iterating all children.
fn has_child_token(node: Node, token: &str) -> bool {
    let mut cursor = node.walk();
    let mut found = false;
    for c in node.children(&mut cursor) {
        if !c.is_named() && c.kind() == token {
            found = true;
            break;
        }
    }
    found
}

/// A bare identifier: a local/param (read its declared/initialized type), the boolean literals
/// `true`/`false` (which parse as plain identifiers), a name that IS a type (`Foo.`/`Color.` static
/// access), or a visible cross-file top-level property.
fn infer_identifier(
    index: &dyn InferenceIndex,
    ident: Node,
    src: &str,
    ctx: &FileCtx,
    depth: usize,
    child_infer: &mut impl ChildInfer,
) -> Type {
    let name = node_text(ident, src);
    if name == "true" || name == "false" {
        return intrinsic_type(index, "Boolean");
    }
    // A lambda parameter (implicit `it` OR a named param) of a scope/collection call takes the
    // receiver's (or element's) type.
    if let Some(t) = lambda_param_type(index, ident, name, src, ctx, depth, child_infer) {
        return t;
    }
    // A smart cast from an enclosing `if (name is T)` / `when(name){ is T -> }` overrides the
    // declared type within the narrowed region.
    if let Some(narrowed) = narrowed_type(index, ident, name, src, ctx) {
        return narrowed;
    }
    // 1. A local val/var/parameter in scope wins (so `val Dog = Dog(); Dog.` is the instance).
    if let Some((decl, _)) = resolve::local_decl(ident, name, UseKind::Value, src) {
        let t = decl_type(index, decl, src, ctx, depth, child_infer);
        if t.name().is_some() {
            return t;
        }
    }
    // 2. A bare identifier naming a known type -> that type (companion / enum-entry / static access).
    if !index.lookup_type(name).is_empty() {
        return resolve_type_name(index, name, ctx, false);
    }
    // 3. A visible cross-file top-level property of that name.
    if let Some(tr) = top_level_property_type(index, name, ctx) {
        return resolve_type_ref(index, &tr, ctx, depth);
    }
    Type::Unknown
}

/// The type a declaration's name node binds: an explicit annotation, or (recursively) the inferred
/// type of its initializer (`val x = foo()` -> foo's return type).
fn decl_type(
    index: &dyn InferenceIndex,
    decl: Node,
    src: &str,
    ctx: &FileCtx,
    depth: usize,
    child_infer: &mut impl ChildInfer,
) -> Type {
    let Some(parent) = decl.parent() else {
        return Type::Unknown;
    };
    match parent.kind() {
        "variable_declaration" => {
            if let Some(t) = destructured_decl_type(index, decl, src, ctx, depth, child_infer) {
                return t;
            }
            // Explicit annotation (`val x: T`, `val x: T?`, `val x: List<T>`).
            if let Some(tr) = crate::indexer::value_type_of(parent, src) {
                return resolve_type_ref(index, &tr, ctx, depth);
            }
            // Initializer: `val x = <expr>` — the expression is a sibling under property_declaration.
            if let Some(prop) = parent.parent() {
                if prop.kind() == "property_declaration" {
                    if let Some(init) = initializer_expr(prop, parent) {
                        return child_infer.infer_child(index, init, src, ctx, depth + 1);
                    }
                }
            }
            Type::Unknown
        }
        "parameter" | "class_parameter" => crate::indexer::value_type_of(parent, src)
            .map(|tr| resolve_type_ref(index, &tr, ctx, depth))
            .unwrap_or(Type::Unknown),
        _ => Type::Unknown,
    }
}

fn destructured_decl_type(
    index: &dyn InferenceIndex,
    decl: Node,
    src: &str,
    ctx: &FileCtx,
    depth: usize,
    child_infer: &mut impl ChildInfer,
) -> Option<Type> {
    let multi = decl.parent()?.parent()?;
    if multi.kind() != "multi_variable_declaration" {
        return None;
    }
    let prop = multi.parent()?;
    if prop.kind() != "property_declaration" {
        return None;
    }

    let mut ordinal = None;
    let mut cursor = multi.walk();
    let mut seen = 0usize;
    for child in multi.named_children(&mut cursor) {
        if child.kind() != "variable_declaration" {
            continue;
        }
        if child == decl.parent()? {
            ordinal = Some(seen);
            break;
        }
        seen += 1;
    }
    let ordinal = ordinal?;
    let init = initializer_expr(prop, multi)?;
    let init_ty = child_infer.infer_child(index, init, src, ctx, depth + 1);
    init_ty.args().get(ordinal).cloned()
}

/// The initializer expression of a `property_declaration` (`val x = EXPR`): the first named child
/// after the `variable_declaration` that is an actual expression (not a getter/setter/delegate).
fn initializer_expr<'t>(prop: Node<'t>, binder: Node<'t>) -> Option<Node<'t>> {
    let mut cursor = prop.walk();
    let mut after = false;
    for child in prop.named_children(&mut cursor) {
        if after {
            match child.kind() {
                "getter" | "setter" | "property_delegate" | "modifiers" | "annotation"
                | "type_constraints" => continue,
                _ => return Some(child),
            }
        }
        if child == binder {
            after = true;
        }
    }
    None
}

/// A call expression: a constructor call (`Foo(...)` -> `Foo`), a visible free function call
/// (`foo()` -> foo's return type), or a method call (`recv.method(...)` -> method's return type).
fn infer_call(
    index: &dyn InferenceIndex,
    node: Node,
    src: &str,
    ctx: &FileCtx,
    depth: usize,
    child_infer: &mut impl ChildInfer,
) -> Type {
    let Some(callee) = node.named_child(0) else {
        return Type::Unknown;
    };
    match callee.kind() {
        "identifier" => {
            let cname = node_text(callee, src);
            // Locals/parameters are lexically stronger than every implicit receiver. Their
            // unmodeled callable signatures must not accidentally select a same-named extension.
            if let Some((binder, _)) = resolve::local_decl(callee, cname, UseKind::Call, src) {
                let owner = binder.parent().and_then(|decl| {
                    if decl.kind() == "variable_declaration" {
                        decl.parent().and_then(|property| property.parent())
                    } else {
                        decl.parent()
                    }
                });
                if !ctx.local_extension_at(binder.start_byte(), binder.end_byte())
                    && !owner.is_some_and(|owner| {
                        matches!(
                            owner.kind(),
                            "source_file" | "class_body" | "enum_class_body"
                        )
                    })
                {
                    return Type::Unknown;
                }
            }
            // Constructor: a known type name -> that type (verified before treating it as a function).
            if !index.lookup_type(cname).is_empty() {
                return resolve_type_name(index, cname, ctx, false);
            }
            let arg_types = synth_arg_types(index, node, src, ctx, depth, child_infer);
            let usage = MemberUse::Call(normalized_call_arguments(node, src, arg_types.clone()));
            if let Some((receiver, selection)) = members::select_implicit_at(
                index,
                implicit_receiver_types(index, callee, src, ctx, depth + 1),
                cname,
                &usage,
                ctx,
                callee.start_byte(),
            ) {
                return selected_member_type(
                    index,
                    &receiver,
                    cname,
                    true,
                    selection,
                    Some(node),
                    match &usage {
                        MemberUse::Call(arguments) => Some(arguments),
                        _ => None,
                    },
                    src,
                    ctx,
                    depth,
                    child_infer,
                );
            }
            // An ordinary visible free function is considered after implicit receivers.
            let entries = visible_top_level_entries(index, cname, ctx, SymbolKind::Function);
            free_function_call_type(
                index,
                entries,
                cname,
                node,
                &arg_types,
                &usage,
                src,
                ctx,
                depth,
                child_infer,
            )
        }
        // Method call `recv.method(...)`: infer recv, then method's return type on that type.
        "navigation_expression" => {
            let (Some(recv), Some(sel)) = (callee.named_child(0), callee.named_child(1)) else {
                return Type::Unknown;
            };
            let Some((recv_ty, candidates, nullable_result)) = navigation_member_candidates(
                index,
                recv,
                node_text(sel, src),
                src,
                ctx,
                depth,
                child_infer,
            ) else {
                return Type::Unknown;
            };
            let arg_types = synth_arg_types(index, node, src, ctx, depth, child_infer);
            if recv_ty == Type::Unknown {
                if let Some(entries) =
                    resolve::qualified_top_level_function_entries(index, callee, src, ctx)
                {
                    let usage =
                        MemberUse::Call(normalized_call_arguments(node, src, arg_types.clone()));
                    return free_function_call_type(
                        index,
                        entries,
                        node_text(sel, src),
                        node,
                        &arg_types,
                        &usage,
                        src,
                        ctx,
                        depth,
                        child_infer,
                    );
                }
            }
            let result = member_type(
                index,
                &recv_ty,
                candidates,
                node_text(sel, src),
                true,
                Some(value_arg_count(node)),
                Some(&arg_types),
                Some(node),
                src,
                ctx,
                depth,
                child_infer,
            );
            navigation_result_type(result, nullable_result)
        }
        _ => Type::Unknown,
    }
}

/// Both visible and package-qualified free calls use the same selected signature and bindings.
fn free_function_call_type(
    index: &dyn InferenceIndex,
    entries: Vec<&Entry>,
    cname: &str,
    node: Node,
    arg_types: &[Type],
    usage: &MemberUse,
    src: &str,
    ctx: &FileCtx,
    depth: usize,
    child_infer: &mut impl ChildInfer,
) -> Type {
    let Some((ret_tr, type_params, params, parameter_facts, rebound)) = free_function_sig(
        index,
        entries,
        ctx,
        value_arg_count(node),
        arg_types,
        usage,
        depth,
    ) else {
        return Type::Unknown;
    };
    let was_named = rebound.is_some();
    let arg_types = rebound.as_deref().unwrap_or(arg_types);
    if was_named && !parameter_facts.known_unbounded() {
        return Type::Unknown;
    }
    if params.iter().any(solve::has_projection) {
        let MemberUse::Call(normalized) = usage else {
            return Type::Unknown;
        };
        if normalized.count != Some(params.len())
            || normalized.count != Some(arg_types.len())
            || normalized.trailing_lambda
        {
            return Type::Unknown;
        }
        let application = projected_application(
            index,
            &params,
            arg_types,
            &type_params,
            parameter_facts.known_unbounded(),
            ctx,
            &HashMap::new(),
        );
        if application.status != solve::ConstraintStatus::Matched {
            return Type::Unknown;
        }
        let tset = type_params.into_iter().collect();
        return resolve_with_subst(index, &ret_tr, ctx, &tset, &application.bindings, depth);
    }
    if type_params.is_empty() {
        return resolve_type_ref(index, &ret_tr, ctx, depth);
    }
    // Generic free function (`fun <T> listOf(vararg e: T): List<T>`): unify the formal
    // parameter types against the synthesized argument types, then substitute the bindings
    // through the declared return type (`List<T>` with `T := Foo` -> `List<Foo>`).
    let tset: HashSet<String> = type_params.into_iter().collect();
    let mut subst: HashMap<String, Type> = HashMap::new();
    for (i, p) in params.iter().enumerate() {
        if let Some(a) = arg_types.get(i) {
            if was_named {
                let mut bindings = HashMap::new();
                solve::unify_into(p, a, &tset, &mut bindings);
                merge_type_bindings(&mut subst, bindings);
            } else {
                solve::unify_into(p, a, &tset, &mut subst);
            }
        }
    }
    // Vararg / extra args: unify the remaining args against the last formal param.
    if let Some(last) = params.last() {
        for a in arg_types.iter().skip(params.len()) {
            solve::unify_into(last, a, &tset, &mut subst);
        }
    }
    bind_lambda_return_type(
        index,
        cname,
        node,
        src,
        ctx,
        &ret_tr,
        &tset,
        &mut subst,
        depth,
        child_infer,
    );
    resolve_with_subst(index, &ret_tr, ctx, &tset, &subst, depth)
}

/// Select a free-call signature from an already resolved visible or package-qualified candidate set.
/// Overloads use the same shared applicability and argument binding before return-type inference.
fn free_function_sig(
    index: &dyn InferenceIndex,
    entries: Vec<&Entry>,
    ctx: &FileCtx,
    arg_count: usize,
    arg_types: &[Type],
    usage: &MemberUse,
    depth: usize,
) -> Option<(
    TypeRef,
    Vec<String>,
    Vec<TypeRef>,
    crate::symbol::TypeParameterFacts,
    Option<Vec<Type>>,
)> {
    if let MemberUse::Call(arguments) = usage {
        if arguments.names.is_some() {
            let selection = members::select_functions(index, entries, usage, ctx);
            if selection.applicability == members::Applicability::Rejected
                || selection.entries.len() != 1
            {
                return None;
            }
            let entry = selection.entries[0];
            let bound =
                members::bind_arguments(entry, arguments, members::CallMode::Kotlin).ok()?;
            if (selection.applicability == members::Applicability::Unknown
                && entry
                    .sym
                    .params
                    .iter()
                    .any(|p| solve::has_projection(p) || !p.args.is_empty()))
                || bound
                    .arguments
                    .types
                    .iter()
                    .any(|ty| !constraint_type_is_known(index, ty, 0))
                || entry.sym.params.iter().any(|p| {
                    !(p.qualification.is_none() && entry.sym.type_params.contains(&p.name))
                        && p.args.is_empty()
                        && !constraint_type_is_known(
                            index,
                            &resolve_type_ref(index, p, ctx, depth),
                            0,
                        )
                })
            {
                return None;
            }
            return Some((
                entry.sym.return_type.clone()?,
                entry.sym.type_params.clone(),
                bound.params.into_owned(),
                entry.sym.type_parameter_facts,
                Some(bound.arguments.into_owned().types),
            ));
        }
    }
    let mut group: Vec<&Entry> = entries
        .into_iter()
        .filter(|e| e.sym.return_type.is_some())
        .collect();
    if group.is_empty() {
        return None;
    }
    let exact: Vec<&Entry> = group
        .iter()
        .copied()
        .filter(|e| e.sym.arity == Some(arg_count.min(u8::MAX as usize) as u8))
        .collect();
    if !exact.is_empty() {
        group = exact;
    }
    let consistent: Vec<&Entry> = group
        .iter()
        .copied()
        .filter(|e| {
            args_consistent(
                index,
                &e.sym.params,
                &e.sym.type_params,
                arg_types,
                ctx,
                depth,
            )
        })
        .collect();
    if !consistent.is_empty() {
        group = consistent;
    }
    let first = group.first()?;
    if group.len() > 1
        && !group.iter().all(|e| {
            opt_type_ref_same_shape(e.sym.return_type.as_ref(), first.sym.return_type.as_ref())
                && e.sym.type_params == first.sym.type_params
                && e.sym.type_parameter_facts == first.sym.type_parameter_facts
                && type_ref_list_same_shape(&e.sym.params, &first.sym.params)
        })
    {
        return None;
    }
    Some((
        first.sym.return_type.clone().unwrap(),
        first.sym.type_params.clone(),
        first.sym.params.clone(),
        first.sym.type_parameter_facts,
        None,
    ))
}

fn top_level_property_type(
    index: &dyn InferenceIndex,
    local_name: &str,
    ctx: &FileCtx,
) -> Option<TypeRef> {
    let group: Vec<&Entry> =
        visible_top_level_entries(index, local_name, ctx, SymbolKind::Property)
            .into_iter()
            .filter(|e| e.sym.value_type.is_some())
            .collect();
    let first = group.first()?;
    if group.len() > 1
        && !group.iter().all(|e| {
            opt_type_ref_same_shape(e.sym.value_type.as_ref(), first.sym.value_type.as_ref())
        })
    {
        return None;
    }
    first.sym.value_type.clone()
}

fn opt_type_ref_same_shape(a: Option<&TypeRef>, b: Option<&TypeRef>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => type_ref_same_shape(a, b),
        (None, None) => true,
        _ => false,
    }
}

fn type_ref_list_same_shape(a: &[TypeRef], b: &[TypeRef]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| type_ref_same_shape(a, b))
}

fn type_ref_same_shape(a: &TypeRef, b: &TypeRef) -> bool {
    a.name == b.name
        && a.projection == b.projection
        && a.qualification == b.qualification
        && a.nullable == b.nullable
        && a.args.len() == b.args.len()
        && a.args
            .iter()
            .zip(&b.args)
            .all(|(a, b)| type_ref_same_shape(a, b))
}

fn visible_top_level_entries<'a>(
    index: &'a dyn InferenceIndex,
    local_name: &str,
    ctx: &FileCtx,
    kind: SymbolKind,
) -> Vec<&'a Entry> {
    for imp in &ctx.imports {
        if !imp.wildcard && imp.alias.as_deref() == Some(local_name) {
            let hits = top_level_entries_exact(index, imp.simple_name(), &imp.package(), kind);
            if !hits.is_empty() {
                return hits;
            }
        }
    }
    for imp in &ctx.imports {
        if !imp.wildcard && imp.alias.is_none() && imp.simple_name() == local_name {
            let hits = top_level_entries_exact(index, local_name, &imp.package(), kind);
            if !hits.is_empty() {
                return hits;
            }
        }
    }
    let same_pkg = top_level_entries_exact(index, local_name, &ctx.package, kind);
    if !same_pkg.is_empty() {
        return same_pkg;
    }
    let star_pkgs: Vec<String> = ctx
        .imports
        .iter()
        .filter(|i| i.wildcard)
        .map(|i| i.package())
        .chain(
            resolve::DEFAULT_IMPORT_PACKAGES
                .iter()
                .map(|s| s.to_string()),
        )
        .collect();
    index
        .lookup_by_name(local_name)
        .iter()
        .filter(|e| e.sym.kind == kind && e.sym.container.is_none() && e.sym.ext_receiver.is_none())
        .filter(|e| star_pkgs.contains(&e.sym.package))
        .collect()
}

fn top_level_entries_exact<'a>(
    index: &'a dyn InferenceIndex,
    name: &str,
    package: &str,
    kind: SymbolKind,
) -> Vec<&'a Entry> {
    index
        .lookup_by_name(name)
        .iter()
        .filter(|e| {
            e.sym.kind == kind
                && e.sym.container.is_none()
                && e.sym.ext_receiver.is_none()
                && e.sym.package == package
        })
        .collect()
}

fn callable_member_entries<'a>(
    candidates: members::Candidates<'a>,
    index: &dyn InferenceIndex,
    receiver: &Type,
    call: Node,
    src: &str,
    ctx: &FileCtx,
    depth: usize,
    child_infer: &mut impl ChildInfer,
) -> (Vec<&'a Entry>, bool) {
    if candidates.has_local_extensions() {
        let usage = MemberUse::Call(normalized_call_arguments(
            call,
            src,
            synth_arg_types(index, call, src, ctx, depth, child_infer),
        ));
        let selection = candidates.select(index, receiver, &usage, ctx);
        return (
            selection
                .entries
                .into_iter()
                .filter(|entry| entry.sym.kind == SymbolKind::Function)
                .collect(),
            selection.applicability != members::Applicability::Rejected,
        );
    }
    let own: Vec<_> = candidates
        .members
        .iter()
        .copied()
        .filter(|entry| entry.sym.kind == SymbolKind::Function)
        .collect();
    if !own.is_empty() {
        (own, false)
    } else {
        (
            candidates
                .extensions
                .iter()
                .copied()
                .filter(|entry| entry.sym.kind == SymbolKind::Function)
                .collect(),
            false,
        )
    }
}

/// The synthesized (bottom-up) types of a call's value arguments, in order. Each `value_argument`'s
/// value expression is its last named child (`name = expr` -> `expr`; bare `expr` -> `expr`).
fn synth_arg_types(
    index: &dyn InferenceIndex,
    call: Node,
    src: &str,
    ctx: &FileCtx,
    depth: usize,
    child_infer: &mut impl ChildInfer,
) -> Vec<Type> {
    let mut out = Vec::new();
    let Some(va) = child_of_kind(call, "value_arguments") else {
        return call
            .named_child(0)
            .filter(|child| child.kind() == "call_expression" && has_trailing_lambda(call))
            .map(|inner| synth_arg_types(index, inner, src, ctx, depth, child_infer))
            .unwrap_or(out);
    };
    let mut cursor = va.walk();
    for arg in va.named_children(&mut cursor) {
        if arg.kind() != "value_argument" {
            continue;
        }
        let n = arg.named_child_count();
        let t = (n > 0)
            .then(|| arg.named_child(n - 1))
            .flatten()
            .map_or(Type::Unknown, |e| {
                child_infer.infer_child(index, e, src, ctx, depth + 1)
            });
        out.push(t);
    }
    out
}

/// Kotlin syntax adapter for the normalized shared member-use query. Argument expressions are
/// inferred bottom-up by inference callers; direct resolution can use the same facts here.
pub fn call_arguments(
    index: &dyn InferenceIndex,
    call: Node,
    src: &str,
    ctx: &FileCtx,
) -> CallArguments {
    let mut child_infer = LocalChildInfer;
    let types = synth_arg_types(index, call, src, ctx, 0, &mut child_infer);
    normalized_call_arguments(call, src, types)
}

/// Resolution needs argument types only when authored labels require candidate-specific binding.
pub(crate) fn named_call_arguments(
    index: &dyn InferenceIndex,
    call: Node,
    src: &str,
    ctx: &FileCtx,
) -> Option<CallArguments> {
    let mut arguments = normalized_call_arguments(call, src, Vec::new());
    arguments.names.as_ref()?;
    arguments.types = synth_arg_types(index, call, src, ctx, 0, &mut LocalChildInfer);
    Some(arguments)
}

fn normalized_call_arguments(call: Node, src: &str, types: Vec<Type>) -> CallArguments {
    let mut arguments = call;
    while child_of_kind(arguments, "value_arguments").is_none() && has_trailing_lambda(arguments) {
        let Some(inner) = arguments
            .named_child(0)
            .filter(|child| child.kind() == "call_expression")
        else {
            break;
        };
        arguments = inner;
    }
    let value_arguments = child_of_kind(arguments, "value_arguments");
    let named = value_arguments.is_some_and(|args| {
        args.named_children(&mut args.walk())
            .any(|arg| arg.kind() == "value_argument" && named_argument_label(arg).is_some())
    });
    let names = if named {
        value_arguments.map(|args| {
            args.named_children(&mut args.walk())
                .filter(|arg| arg.kind() == "value_argument")
                .map(|arg| {
                    named_argument_label(arg)
                        .map(|label| node_text(label, src).trim_matches('`').to_string())
                })
                .collect()
        })
    } else {
        None
    };
    let has_spread = value_arguments.is_some_and(|args| {
        args.named_children(&mut args.walk()).any(|arg| {
            arg.kind() == "value_argument"
                && arg
                    .children(&mut arg.walk())
                    .any(|child| child.kind() == "*")
        })
    });
    CallArguments {
        count: (!call.has_error()).then(|| value_arg_count(call)),
        types,
        names,
        has_spread,
        trailing_lambda: has_trailing_lambda(call),
    }
}

fn named_argument_label(arg: Node) -> Option<Node> {
    (arg.named_child_count() > 1)
        .then(|| arg.named_child(0))
        .flatten()
        .filter(|first| first.kind() == "identifier")
}

fn select_trailing_lambda_receiver_type(
    index: &dyn InferenceIndex,
    entries: Vec<&Entry>,
    recv_ty: Option<&Type>,
    call: Node,
    src: &str,
    ctx: &FileCtx,
    depth: usize,
    child_infer: &mut impl ChildInfer,
) -> Option<Type> {
    let arg_count = value_arg_count(call);
    let arg_types = synth_arg_types(index, call, src, ctx, depth, child_infer);
    if std::env::var("KTDBG").is_ok() {
        eprintln!(
            "DBG select: arg_count={} arg_types={:?}",
            arg_count, arg_types
        );
    }
    let mut group: Vec<(&Entry, TypeRef)> = entries
        .into_iter()
        .filter_map(|entry| {
            trailing_lambda_receiver_ref(index, entry, ctx).map(|receiver| (entry, receiver))
        })
        .collect();
    if std::env::var("KTDBG").is_ok() {
        eprintln!("DBG group: {}", group.len());
    }
    if group.is_empty() {
        return None;
    }
    let args = normalized_call_arguments(call, src, arg_types.clone());
    group.retain(|(entry, _)| {
        members::function_accepts(index, recv_ty.unwrap_or(&Type::Unknown), entry, &args, ctx)
    });
    let mut resolved = Vec::with_capacity(group.len());
    for (entry, receiver_ref) in group {
        resolved.push(resolve_trailing_lambda_receiver_candidate(
            index,
            entry,
            &receiver_ref,
            recv_ty,
            &args,
            ctx,
            depth,
        )?);
    }
    let first = resolved.first()?.clone();
    resolved
        .iter()
        .all(|receiver| *receiver == first)
        .then_some(first)
}

fn resolve_trailing_lambda_receiver_candidate(
    index: &dyn InferenceIndex,
    entry: &Entry,
    receiver_ref: &TypeRef,
    recv_ty: Option<&Type>,
    arguments: &CallArguments,
    ctx: &FileCtx,
    depth: usize,
) -> Option<Type> {
    let tset: HashSet<String> = entry.sym.type_params.iter().cloned().collect();
    let arguments = members::bind_arguments(entry, arguments, members::CallMode::Kotlin).ok()?;
    let arg_types = arguments.arguments.types.as_slice();
    let params = arguments.params.as_ref();
    let mut subst = match recv_ty {
        Some(receiver) if entry.sym.extension_receiver.is_some() => {
            receiver_type_bindings(index, receiver, entry)?
        }
        _ => HashMap::new(),
    };
    if params.iter().any(solve::has_projection) {
        let declaration_ctx = FileCtx::new(entry.sym.package.clone(), Vec::new());
        let application = projected_application(
            index,
            params,
            arg_types,
            &entry.sym.type_params,
            entry.sym.type_parameter_facts.known_unbounded(),
            &declaration_ctx,
            &subst,
        );
        if application.status != solve::ConstraintStatus::Matched {
            return None;
        }
        subst = application.bindings;
    } else {
        for (i, p) in params.iter().enumerate() {
            if let Some(a) = arg_types.get(i) {
                let mut argument_bindings = HashMap::new();
                solve::unify_into(p, a, &tset, &mut argument_bindings);
                merge_type_bindings(&mut subst, argument_bindings);
            }
        }
    }
    if let (None, Some(recv_name), Some(actual_recv)) = (
        entry.sym.extension_receiver.as_ref(),
        entry.sym.ext_receiver.as_deref(),
        recv_ty,
    ) {
        if tset.contains(recv_name) {
            subst
                .entry(recv_name.to_string())
                .or_insert_with(|| actual_recv.clone());
        }
    }
    if !extension_bounds_hold(index, entry, &subst) {
        return None;
    }
    // Only reject when the receiver HEAD is itself an unbound type parameter (the class to look
    // up is unknown). Unbound type-parameter ARGUMENTS resolve to `Unknown` (sound — never a
    // wrong guess) and the head class still drives member lookup, so a `flow { emit(x) }` builder
    // without explicit type arguments resolves its receiver as `FlowCollector<Unknown>` and
    // `emit` is found instead of being falsely reported absent.
    if tset.contains(receiver_ref.name.as_str()) && !subst.contains_key(&receiver_ref.name) {
        return None;
    }
    Some(resolve_with_subst(
        index,
        receiver_ref,
        ctx,
        &tset,
        &subst,
        depth,
    ))
}

/// The receiver contributed by an entry's trailing lambda. Direct receiver-function parameters
/// are stored on the function itself. Alias-backed parameters are expanded lazily because their
/// declaration can live in a different indexed file (as Ktor's `RoutingHandler` does).
pub fn trailing_lambda_receiver_ref(
    index: &dyn InferenceIndex,
    entry: &Entry,
    ctx: &FileCtx,
) -> Option<TypeRef> {
    if let Some(receiver) = entry.sym.trailing_lambda_receiver_type.as_deref() {
        return Some(receiver.clone());
    }
    let parameter_type = entry.sym.params.last()?;
    if parameter_type.name.is_empty() {
        return None;
    }
    let resolved_package = type_ref_identity(index, parameter_type, ctx)?.0;
    let aliases: Vec<&Entry> = index
        .lookup_by_name(&parameter_type.name)
        .iter()
        .filter(|candidate| candidate.sym.kind == SymbolKind::TypeAlias)
        .filter(|candidate| {
            resolved_package
                .as_deref()
                .map_or(true, |package| candidate.sym.package == package)
        })
        .filter(|candidate| candidate.sym.function_type_receiver.is_some())
        .collect();
    let first = *aliases.first()?;
    if !aliases.iter().all(|candidate| {
        candidate.sym.type_params == first.sym.type_params
            && candidate.sym.type_parameter_facts == first.sym.type_parameter_facts
            && opt_type_ref_same_shape(
                candidate.sym.function_type_receiver.as_deref(),
                first.sym.function_type_receiver.as_deref(),
            )
    }) {
        return None;
    }
    if first.sym.type_params.len() != parameter_type.args.len() {
        return None;
    }
    let bindings: HashMap<&str, &TypeRef> = first
        .sym
        .type_params
        .iter()
        .map(String::as_str)
        .zip(parameter_type.args.iter())
        .collect();
    substitute_alias_type_ref(first.sym.function_type_receiver.as_deref()?, &bindings)
}

/// Minimum accepted argument count for a syntactic trailing-lambda call after proving the final
/// parameter is either a direct function type or a receiver-function alias.
pub fn trailing_lambda_min_arity(
    index: &dyn InferenceIndex,
    entry: &Entry,
    ctx: &FileCtx,
) -> Option<u8> {
    entry.sym.trailing_lambda_min_arity.or_else(|| {
        trailing_lambda_receiver_ref(index, entry, ctx)
            .is_some()
            .then_some(entry.sym.last_parameter_min_arity)
            .flatten()
    })
}

fn substitute_alias_type_ref(
    template: &TypeRef,
    bindings: &HashMap<&str, &TypeRef>,
) -> Option<TypeRef> {
    if let Some(actual) = bindings.get(template.name.as_str()) {
        let mut actual = (*actual).clone();
        actual.nullable |= template.nullable;
        if template.projection != Projection::Invariant {
            if actual.projection != Projection::Invariant {
                actual.name.clear();
                actual.args.clear();
            }
            actual.projection = template.projection;
        }
        return Some(actual);
    }
    let mut resolved = template.clone();
    resolved.args = template
        .args
        .iter()
        .map(|arg| substitute_alias_type_ref(arg, bindings))
        .collect::<Option<Vec<_>>>()?;
    Some(resolved)
}

/// Like `resolve_type_ref`, but a name in `tparams` resolves to its binding in `subst` (or `Unknown`
/// when unbound — never a wrong guess) instead of being looked up as a concrete type.
fn resolve_with_subst(
    index: &dyn InferenceIndex,
    tr: &TypeRef,
    ctx: &FileCtx,
    tparams: &HashSet<String>,
    subst: &HashMap<String, Type>,
    depth: usize,
) -> Type {
    if tr.projection == Projection::Star {
        return Type::Star;
    }
    Type::projected(
        tr.projection,
        resolve_with_subst_unprojected(index, tr, ctx, tparams, subst, depth),
    )
}

fn resolve_with_subst_unprojected(
    index: &dyn InferenceIndex,
    tr: &TypeRef,
    ctx: &FileCtx,
    tparams: &HashSet<String>,
    subst: &HashMap<String, Type>,
    depth: usize,
) -> Type {
    if tr.name.is_empty() {
        return Type::Unknown;
    }
    if tr.qualification.is_none() {
        if let Some(bound) = subst.get(&tr.name) {
            if !type_head_is_known(index, bound) {
                return Type::Unknown;
            }
            let mut bound = bound.clone();
            if let Type::Class { nullable, .. } = &mut bound {
                *nullable |= tr.nullable;
            }
            return bound;
        }
        if tparams.contains(&tr.name) {
            return Type::Unknown;
        }
    }
    let Some((package, container)) = type_ref_identity(index, tr, ctx) else {
        return Type::Unknown;
    };
    let args = if depth > MAX_DEPTH {
        Vec::new()
    } else {
        tr.args
            .iter()
            .map(|a| resolve_with_subst(index, a, ctx, tparams, subst, depth + 1))
            .collect()
    };
    Type::Class {
        name: tr.name.clone(),
        package,
        container,
        nullable: tr.nullable,
        args,
    }
}

/// The number of value arguments of a call: the `value_argument` children of its `value_arguments`
/// node, plus one for a trailing `annotated_lambda` (`f(a) { … }` / `xs.map { … }`). Used for
/// arity-based overload disambiguation.
fn value_arg_count(call: Node) -> usize {
    let mut n = 0;
    if let Some(va) = child_of_kind(call, "value_arguments") {
        let mut cursor = va.walk();
        n += va
            .named_children(&mut cursor)
            .filter(|x| x.kind() == "value_argument")
            .count();
    } else if has_trailing_lambda(call) {
        if let Some(inner) = call
            .named_child(0)
            .filter(|child| child.kind() == "call_expression")
        {
            n += value_arg_count(inner);
        }
    }
    if child_of_kind(call, "annotated_lambda").is_some() {
        n += 1;
    }
    n
}

/// A standalone `recv.prop` navigation (no call): infer recv, then the type of property `prop`.
fn infer_navigation(
    index: &dyn InferenceIndex,
    node: Node,
    src: &str,
    ctx: &FileCtx,
    depth: usize,
    child_infer: &mut impl ChildInfer,
) -> Type {
    let (Some(recv), Some(sel)) = (node.named_child(0), node.named_child(1)) else {
        return Type::Unknown;
    };
    let Some((recv_ty, candidates, nullable_result)) = navigation_member_candidates(
        index,
        recv,
        node_text(sel, src),
        src,
        ctx,
        depth,
        child_infer,
    ) else {
        return Type::Unknown;
    };
    let result = member_type(
        index,
        &recv_ty,
        candidates,
        node_text(sel, src),
        false,
        None,
        None,
        None,
        src,
        ctx,
        depth,
        child_infer,
    );
    navigation_result_type(result, nullable_result)
}

fn navigation_member_candidates<'a>(
    index: &'a dyn InferenceIndex,
    receiver: Node,
    name: &str,
    src: &str,
    ctx: &'a FileCtx,
    depth: usize,
    child_infer: &mut impl ChildInfer,
) -> Option<(Type, members::Candidates<'a>, bool)> {
    if receiver.kind() == "super_expression" {
        return members::super_member_candidates(
            index,
            super_receivers(index, receiver, src, ctx),
            name,
            ctx,
        )
        .map(|(receiver, candidates)| (receiver, candidates, false));
    }
    let original = child_infer.infer_child(index, receiver, src, ctx, depth + 1);
    let nullable_result = original.is_nullable() && is_safe_navigation_receiver(receiver);
    let ty = member_receiver_type(receiver, original);
    let candidates = members::candidates_at(index, &ty, name, ctx, receiver.end_byte());
    Some((ty, candidates, nullable_result))
}

fn is_safe_navigation_receiver(receiver: Node) -> bool {
    receiver
        .parent()
        .is_some_and(|nav| nav.kind() == "navigation_expression" && has_child_token(nav, "?."))
}

fn navigation_result_type(mut result: Type, nullable_result: bool) -> Type {
    if nullable_result {
        if let Type::Class { nullable, .. } = &mut result {
            *nullable = true;
        }
    }
    result
}

/// A safe navigation tests the non-null receiver before selecting its member.
pub fn member_receiver_type(receiver: Node, mut ty: Type) -> Type {
    if is_safe_navigation_receiver(receiver) {
        if let Type::Class { nullable, .. } = &mut ty {
            *nullable = false;
        }
    }
    ty
}

/// Infer the result of the same call/value member selection used by navigation and references.
/// Every surviving candidate must agree; missing callable signatures retain `Unknown`.
fn member_type(
    index: &dyn InferenceIndex,
    recv_ty: &Type,
    candidates: members::Candidates<'_>,
    name: &str,
    want_function: bool,
    arg_count: Option<usize>,
    arg_types: Option<&[Type]>,
    call: Option<Node>,
    src: &str,
    ctx: &FileCtx,
    depth: usize,
    child_infer: &mut impl ChildInfer,
) -> Type {
    let Some(_) = recv_ty.name() else {
        return Type::Unknown;
    };
    let usage = if want_function {
        let types = arg_types.unwrap_or_default().to_vec();
        MemberUse::Call(match call {
            Some(call) => normalized_call_arguments(call, src, types),
            None => CallArguments {
                count: arg_count,
                types,
                names: None,
                has_spread: false,
                trailing_lambda: false,
            },
        })
    } else {
        MemberUse::Value
    };
    let group = candidates.select(index, recv_ty, &usage, ctx);
    selected_member_type(
        index,
        recv_ty,
        name,
        want_function,
        group,
        call,
        match &usage {
            MemberUse::Call(arguments) => Some(arguments),
            _ => None,
        },
        src,
        ctx,
        depth,
        child_infer,
    )
}

fn selected_member_type(
    index: &dyn InferenceIndex,
    recv_ty: &Type,
    name: &str,
    want_function: bool,
    group: members::Selection<'_>,
    call: Option<Node>,
    arguments: Option<&CallArguments>,
    src: &str,
    ctx: &FileCtx,
    depth: usize,
    child_infer: &mut impl ChildInfer,
) -> Type {
    if group.applicability == members::Applicability::Rejected || group.entries.is_empty() {
        return Type::Unknown;
    }
    // Resolve each candidate's type; agree -> that type, disagree -> Unknown (ambiguous, stay silent).
    let mut result: Option<Type> = None;
    for e in group.entries {
        let definition = crate::symbol::Def {
            file: e.path.to_string(),
            start_byte: e.sym.start_byte,
            end_byte: e.sym.end_byte,
        };
        let semantic_entry = index.semantic_entry(&definition);
        let e = semantic_entry.unwrap_or(e);
        let tr = match member_type_ref(e, want_function) {
            Some(tr) => tr,
            None => return Type::Unknown,
        };
        let t = if semantic_entry.is_some() {
            if let Some(arguments) = arguments.filter(|arguments| arguments.names.is_some()) {
                if members::bind_arguments(e, arguments, members::CallMode::Kotlin).is_err() {
                    return Type::Unknown;
                }
            }
            declared_type(index, tr, &FileCtx::new(e.sym.package.clone(), Vec::new()))
        } else {
            resolve_member_type_with_subst(
                index,
                recv_ty,
                e,
                name,
                tr,
                call,
                arguments,
                src,
                ctx,
                depth + 1,
                child_infer,
            )
        };
        match &result {
            None => result = Some(t),
            Some(prev) if *prev == t => {}
            Some(_) => return Type::Unknown,
        }
    }
    result.unwrap_or(Type::Unknown)
}

fn resolve_member_type_with_subst(
    index: &dyn InferenceIndex,
    recv_ty: &Type,
    entry: &Entry,
    op_name: &str,
    tr: &TypeRef,
    call: Option<Node>,
    arguments: Option<&CallArguments>,
    src: &str,
    ctx: &FileCtx,
    depth: usize,
    child_infer: &mut impl ChildInfer,
) -> Type {
    let tparams = member_type_parameters(index, entry);
    let projected = entry.sym.params.iter().any(solve::has_projection);
    let bound = match arguments {
        Some(arguments) => {
            match members::bind_arguments(entry, arguments, members::CallMode::Kotlin) {
                Ok(bound) => Some(bound),
                Err(_) => return Type::Unknown,
            }
        }
        None => None,
    };
    if bound.as_ref().is_some_and(|bound| {
        bound.params.len() != entry.sym.params.len()
            && !call_parameter_templates_known(index, entry)
    }) {
        return Type::Unknown;
    }
    if projected {
        if let Some(bound) = &bound {
            return declared_member_bound_type(index, recv_ty, entry, tr, bound, ctx);
        }
    }
    if tparams.is_empty() && entry.sym.extension_receiver.is_none() {
        return resolve_type_ref(index, tr, ctx, depth + 1);
    }
    let receiver_bindings = receiver_type_bindings(index, recv_ty, entry);
    if entry.sym.extension_receiver.is_some() && receiver_bindings.is_none() {
        return Type::Unknown;
    }
    if tparams.is_empty() {
        return resolve_type_ref(index, tr, ctx, depth + 1);
    }
    let mut subst = receiver_bindings.unwrap_or_default();
    if let (Some(call), Some(bound)) = (call, bound.as_ref()) {
        // Arguments can bind only method variables; receiver variables retain their equalities.
        let method_params = entry.sym.type_params.iter().cloned().collect();
        for (formal, actual) in bound.params.iter().zip(&bound.arguments.types) {
            let mut argument_bindings = HashMap::new();
            solve::unify_into(formal, actual, &method_params, &mut argument_bindings);
            merge_type_bindings(&mut subst, argument_bindings);
        }
        bind_lambda_return_type(
            index,
            op_name,
            call,
            src,
            ctx,
            tr,
            &tparams,
            &mut subst,
            depth,
            child_infer,
        );
    }
    if !extension_bounds_hold(index, entry, &subst) {
        return Type::Unknown;
    }
    resolve_with_subst(index, tr, ctx, &tparams, &subst, depth + 1)
}

/// Resolve normalized member facts using the receiver's instantiated superclass path.
/// Syntax adapters do not need to pass their language's AST through Kotlin inference.
pub fn declared_member_type(
    index: &dyn InferenceIndex,
    recv_ty: &Type,
    entry: &Entry,
    tr: &TypeRef,
    ctx: &FileCtx,
) -> Type {
    let tparams = member_type_parameters(index, entry);
    if tparams.is_empty() && entry.sym.extension_receiver.is_none() {
        return resolve_type_ref(index, tr, ctx, 0);
    }
    let receiver_bindings = receiver_type_bindings(index, recv_ty, entry);
    if entry.sym.extension_receiver.is_some() && receiver_bindings.is_none() {
        return Type::Unknown;
    }
    if tparams.is_empty() {
        return resolve_type_ref(index, tr, ctx, 0);
    }
    let subst = receiver_bindings.unwrap_or_default();
    resolve_with_subst(index, tr, ctx, &tparams, &subst, 0)
}

// Closed type values used in projected constraints must not smuggle unmodeled aliases or bounds.
fn constraint_type_is_known(index: &dyn InferenceIndex, ty: &Type, depth: usize) -> bool {
    if depth > MAX_DEPTH || !receiver_arguments_known(index, ty, depth) {
        return false;
    }
    if let Some(declaration) = type_declaration_shape(index, ty) {
        if declaration.sym.kind == SymbolKind::TypeAlias {
            return false;
        }
        if !ty.args().is_empty()
            && !index
                .lookup_type(ty.name().unwrap_or_default())
                .iter()
                .filter(|entry| {
                    entry.sym.package == ty.package().unwrap_or_default()
                        && entry.sym.container.as_deref() == ty.container()
                })
                .all(|entry| entry.sym.type_parameter_facts.known_unbounded())
        {
            return false;
        }
    }
    ty.args()
        .iter()
        .all(|arg| constraint_type_is_known(index, arg, depth + 1))
}

// Equality can reject distinct ordinary nominal identities; JVM mapped types and aliases need
// their own identity normalization. Gradual compatibility is never used as an absence proof.
fn constraint_types_distinct(index: &dyn InferenceIndex, a: &Type, b: &Type, depth: usize) -> bool {
    if depth > MAX_DEPTH
        || a == b
        || !constraint_type_is_known(index, a, depth)
        || !constraint_type_is_known(index, b, depth)
    {
        return false;
    }
    let same_head =
        a.name() == b.name() && a.package() == b.package() && a.container() == b.container();
    if same_head {
        return a.args().len() == b.args().len()
            && a.args()
                .iter()
                .zip(b.args())
                .any(|(a, b)| constraint_types_distinct(index, a, b, depth + 1));
    }
    let ordinary = |ty: &Type| {
        ty.package().is_some_and(|package| {
            package != "kotlin"
                && !package.starts_with("kotlin.")
                && package != "java"
                && !package.starts_with("java.")
        })
    };
    ordinary(a) && ordinary(b)
}

fn projected_application(
    index: &dyn InferenceIndex,
    params: &[TypeRef],
    actuals: &[Type],
    type_params: &[String],
    known_unbounded: bool,
    ctx: &FileCtx,
    initial: &HashMap<String, Type>,
) -> solve::Application {
    let parameters = type_params.iter().cloned().collect();
    solve::constrain_application(
        params,
        actuals,
        &parameters,
        initial,
        known_unbounded,
        |formal| resolve_with_subst_unprojected(index, formal, ctx, &parameters, initial, 0),
        |actual| constraint_type_is_known(index, actual, 0),
        |actual| {
            type_declaration_shape(index, actual).is_some()
                && index
                    .lookup_type(actual.name().unwrap_or_default())
                    .iter()
                    .filter(|entry| {
                        entry.sym.package == actual.package().unwrap_or_default()
                            && entry.sym.container.as_deref() == actual.container()
                    })
                    .all(|entry| {
                        entry.sym.type_parameter_facts.known_invariant()
                            && entry.sym.type_parameter_facts.known_unbounded()
                    })
        },
        |actual, bound| receiver_satisfies_upper_bound(index, actual, bound),
        |actual, other| constraint_types_distinct(index, actual, other, 0),
    )
}

/// A defaulted slot is not a synthesized argument. It must still have modeled declaration types
/// before removing that slot can turn an uncertain overload into a proven applicable one.
pub(crate) fn call_parameter_templates_known(index: &dyn InferenceIndex, entry: &Entry) -> bool {
    if !entry.sym.type_parameter_facts.known_unbounded() {
        return false;
    }
    fn known(
        index: &dyn InferenceIndex,
        formal: &TypeRef,
        ctx: &FileCtx,
        parameters: &HashSet<String>,
        depth: usize,
    ) -> bool {
        if depth > MAX_DEPTH || formal.projection == Projection::Star {
            return false;
        }
        if formal.qualification.is_none() && parameters.contains(&formal.name) {
            return formal.args.is_empty();
        }
        let resolved =
            resolve_with_subst_unprojected(index, formal, ctx, parameters, &HashMap::new(), depth);
        if !type_head_is_known(index, &resolved) || !receiver_shape_is_known(index, &resolved) {
            return false;
        }
        if formal.args.is_empty() {
            return constraint_type_is_known(index, &resolved, depth);
        }
        if type_declaration_shape(index, &resolved).is_none()
            || !index
                .lookup_type(resolved.name().unwrap_or_default())
                .iter()
                .filter(|entry| {
                    entry.sym.package == resolved.package().unwrap_or_default()
                        && entry.sym.container.as_deref() == resolved.container()
                })
                .all(|entry| {
                    entry.sym.kind != SymbolKind::TypeAlias
                        && entry.sym.type_parameter_facts.known_unbounded()
                        && entry.sym.type_parameter_facts.known_invariant()
                })
        {
            return false;
        }
        formal
            .args
            .iter()
            .all(|arg| known(index, arg, ctx, parameters, depth + 1))
    }
    let ctx = FileCtx::new(entry.sym.package.clone(), Vec::new());
    let parameters = member_type_parameters(index, entry);
    entry
        .sym
        .params
        .iter()
        .all(|formal| known(index, formal, &ctx, &parameters, 0))
}

/// Both call selection and return substitution consume the same bound formal/actual pairs.
pub(crate) fn projected_member_application(
    index: &dyn InferenceIndex,
    receiver: &Type,
    entry: &Entry,
    bound: &members::BoundArguments<'_>,
) -> Option<solve::Application> {
    if !entry
        .sym
        .params
        .iter()
        .any(|ty| solve::has_projection(ty) || !ty.args.is_empty())
    {
        return None;
    }
    let arguments = bound.arguments.as_ref();
    let ctx = FileCtx::new(entry.sym.package.clone(), Vec::new());
    let receiver_bindings = receiver_type_bindings(index, receiver, entry);
    let missing_receiver = entry.sym.extension_receiver.is_some() && receiver_bindings.is_none();
    let initial = receiver_bindings.unwrap_or_default();
    let mut application = projected_application(
        index,
        &bound.params,
        &arguments.types,
        &entry.sym.type_params,
        entry.sym.type_parameter_facts.known_unbounded(),
        &ctx,
        &initial,
    );
    if entry.sym.has_vararg
        || arguments.count != Some(arguments.types.len())
        || arguments.trailing_lambda
        || missing_receiver
        || (bound.params.len() != entry.sym.params.len()
            && !call_parameter_templates_known(index, entry))
    {
        application.status = solve::ConstraintStatus::Unknown;
    }
    Some(application)
}

/// A language adapter supplies already synthesized arguments, without using another grammar's AST.
pub fn declared_member_call_type(
    index: &dyn InferenceIndex,
    receiver: &Type,
    entry: &Entry,
    tr: &TypeRef,
    arguments: &CallArguments,
    ctx: &FileCtx,
) -> Type {
    let bound = match members::bind_arguments(entry, arguments, members::CallMode::Kotlin) {
        Ok(bound) => bound,
        Err(_) => return Type::Unknown,
    };
    declared_member_bound_type(index, receiver, entry, tr, &bound, ctx)
}

fn declared_member_bound_type(
    index: &dyn InferenceIndex,
    receiver: &Type,
    entry: &Entry,
    tr: &TypeRef,
    bound: &members::BoundArguments<'_>,
    ctx: &FileCtx,
) -> Type {
    if bound.params.len() != entry.sym.params.len() && !call_parameter_templates_known(index, entry)
    {
        return Type::Unknown;
    }
    if entry.sym.type_params.is_empty() && !entry.sym.params.iter().any(solve::has_projection) {
        return declared_member_type(index, receiver, entry, tr, ctx);
    }
    let Some(application) = projected_member_application(index, receiver, entry, bound) else {
        return declared_member_type(index, receiver, entry, tr, ctx);
    };
    if application.status != solve::ConstraintStatus::Matched {
        return Type::Unknown;
    }
    let parameters = member_type_parameters(index, entry);
    resolve_with_subst(index, tr, ctx, &parameters, &application.bindings, 0)
}

fn merge_type_bindings(subst: &mut HashMap<String, Type>, bindings: HashMap<String, Type>) {
    for (parameter, actual) in bindings {
        subst
            .entry(parameter)
            .and_modify(|bound| {
                if *bound != actual {
                    // shortcut: conflicting constraints stay Unknown until join inference is available.
                    *bound = Type::Unknown;
                }
            })
            .or_insert(actual);
    }
}

fn member_type_parameters(index: &dyn InferenceIndex, entry: &Entry) -> HashSet<String> {
    let mut params: HashSet<_> = entry.sym.type_params.iter().cloned().collect();
    if let Some(container) = entry.sym.container.as_deref() {
        if let Some(owner) = named_type_params(index, container, &entry.sym.package) {
            params.extend(owner);
        }
    }
    params
}

/// Whether a candidate's parameter types are each consistent with the corresponding argument type.
/// Gradual: an `Unknown` argument or param, or a param that is one of the candidate's own type
/// variables, never eliminates the candidate; a known subtype is consistent. Only the positional value
/// args are checked (a trailing lambda / extra args beyond the param list are ignored).

/// Whether `name` is declared as a type parameter of some indexed type/function. Used to tell
/// an unbound class-level type variable from a concrete type name.
fn is_class_level_type_variable(index: &dyn InferenceIndex, name: &str) -> bool {
    index
        .lookup_by_name(name)
        .iter()
        .any(|e| e.sym.type_params.iter().any(|tp| tp == name))
}

fn args_consistent(
    index: &dyn InferenceIndex,
    params: &[TypeRef],
    type_params: &[String],
    args: &[Type],
    ctx: &FileCtx,
    depth: usize,
) -> bool {
    let tvars: HashSet<&str> = type_params.iter().map(String::as_str).collect();
    for (p, a) in params.iter().zip(args) {
        if tvars.contains(p.name.as_str()) {
            continue; // a type-variable parameter accepts anything
        }
        if p.args.is_empty() && is_class_level_type_variable(index, &p.name) {
            // A class-level type variable we could not bind (the `E` of `MutableList<E>.add`,
            // unbound when the receiver's type arguments are unknown). Treating it as a
            // concrete type would reject every argument — accept. Other names use the shared
            // gradual consistency check, including intrinsic Int/String without library sources.
            continue;
        }
        let pt = resolve_type_ref(index, p, ctx, depth + 1);
        if !types_consistent(index, a, &pt) {
            return false;
        }
    }
    true
}

pub fn argument_types_consistent(
    index: &dyn InferenceIndex,
    params: &[TypeRef],
    type_params: &[String],
    args: &[Type],
    ctx: &FileCtx,
) -> bool {
    args_consistent(index, params, type_params, args, ctx, 0)
}

/// Gradual consistency of an actual argument type with a formal parameter type: an `Unknown` on either
/// side or an unindexed symbolic head stays possible; an exact head match or a known subtype is
/// consistent; otherwise inconsistent.
pub(crate) fn types_consistent(index: &dyn InferenceIndex, actual: &Type, formal: &Type) -> bool {
    let (Some(an), Some(fname)) = (actual.name(), formal.name()) else {
        return true; // Unknown on either side -> compatible (don't eliminate)
    };
    if primitive_type_name(an) == primitive_type_name(fname) {
        let builtin = |ty: &Type| {
            matches!(
                primitive_type_name(ty.name().unwrap_or_default()),
                "Int"
                    | "Long"
                    | "Short"
                    | "Byte"
                    | "Float"
                    | "Double"
                    | "Boolean"
                    | "Char"
                    | "String"
            ) && ty.container().is_none()
                && ty
                    .package()
                    .is_none_or(|package| package == "kotlin" || package == "java.lang")
        };
        if builtin(actual) && builtin(formal) {
            return true;
        }
        return actual
            .package()
            .zip(formal.package())
            .is_none_or(|(a, f)| a == f)
            && actual.container() == formal.container();
    }
    if !type_head_is_known(index, actual) || !type_head_is_known(index, formal) {
        return true;
    }
    is_subtype(index, an, actual.package(), fname)
}

/// A symbolic name retained from an incomplete declaration is not evidence of a type mismatch.
/// Fundamental language types have identities without source spans; other heads need an exact
/// declaration in the current scoped index before overload selection can reject them.
pub(crate) fn type_head_is_known(index: &dyn InferenceIndex, ty: &Type) -> bool {
    let Some(name) = ty.name() else {
        return false;
    };
    if ty.container().is_none()
        && crate::defaults::is_builtin_type(primitive_type_name(name))
        && ty
            .package()
            .is_none_or(|package| package == "kotlin" || package == "java.lang")
    {
        return true;
    }
    index.lookup_type(name).iter().any(|entry| {
        entry.sym.package == ty.package().unwrap_or_default()
            && entry.sym.container.as_deref() == ty.container()
    })
}

pub(crate) fn primitive_type_name(name: &str) -> &str {
    match name {
        "int" => "Int",
        "long" => "Long",
        "short" => "Short",
        "byte" => "Byte",
        "float" => "Float",
        "double" => "Double",
        "boolean" => "Boolean",
        "char" => "Char",
        _ => name,
    }
}

/// Whether the type named `name` (in package `pkg` when known) is `target` or a subtype of it,
/// walking the supertype closure (bounded).
fn is_subtype(index: &dyn InferenceIndex, name: &str, pkg: Option<&str>, target: &str) -> bool {
    let mut visited: HashSet<String> = HashSet::new();
    let mut frontier: Vec<(String, Option<String>, usize)> =
        vec![(name.to_string(), pkg.map(str::to_string), 0)];
    while let Some((cur, cur_pkg, d)) = frontier.pop() {
        if !visited.insert(cur.clone()) || d > SUPERTYPE_CAP {
            continue;
        }
        if cur == target {
            return true;
        }
        for sup in index.supertypes_of_in(&cur, cur_pkg.as_deref()) {
            let sup_pkg = same_pkg_supertype(index, &sup, cur_pkg.as_deref());
            frontier.push((sup, sup_pkg, d + 1));
        }
    }
    false
}

/// Bind the declaring type's parameters through instantiated superclass/interface paths.
/// Multiple paths must agree; missing arguments and cycles never select arbitrary bindings.
pub(crate) fn receiver_type_bindings(
    index: &dyn InferenceIndex,
    recv_ty: &Type,
    entry: &Entry,
) -> Option<HashMap<String, Type>> {
    if let Some(receiver) = entry.sym.extension_receiver.as_deref() {
        if receiver.has_type_constraints || !type_head_is_known(index, recv_ty) {
            return None;
        }
        // Each variable records whether its explicit upper bound proves it non-null.
        let mut params: HashMap<_, _> = entry
            .sym
            .type_params
            .iter()
            .cloned()
            .map(|name| (name, false))
            .collect();
        let ctx = FileCtx::new(entry.sym.package.clone(), Vec::new());
        for (parameter, bound) in &receiver.upper_bounds {
            let resolved = resolve_type_ref(index, bound, &ctx, 0);
            if !resolved.is_nullable()
                && receiver_arguments_known(index, &resolved, 0)
                && !receiver_bound_mentions_parameter(bound, &entry.sym.type_params, 0)
            {
                if let Some(non_null) = params.get_mut(parameter) {
                    *non_null = true;
                }
            }
        }
        let bind = |actual: &Type| {
            let mut bindings = HashMap::new();
            if !bind_receiver_template(index, &receiver.ty, actual, &ctx, &params, &mut bindings, 0)
            {
                return None;
            }
            extension_bounds_hold(index, entry, &bindings).then_some(bindings)
        };
        if receiver.ty.qualification.is_none() && params.contains_key(&receiver.ty.name) {
            return bind(recv_ty);
        }
        let formal = resolve_type_ref(index, &receiver.ty, &ctx, 0);
        if !type_head_is_known(index, &formal) || (recv_ty.is_nullable() && !formal.is_nullable()) {
            return None;
        }
        let same_head = |actual: &Type| {
            actual.name() == formal.name()
                && actual.package() == formal.package()
                && actual.container() == formal.container()
        };
        // A direct match needs no assumptions about unrelated, possibly unavailable ancestors.
        if same_head(recv_ty) {
            return bind(recv_ty);
        }
        let mut result = None;
        let mut incomplete = false;
        let walked = walk_supertypes(index, recv_ty, SUPERTYPE_CAP, |current| {
            incomplete |= !type_head_is_known(index, current);
            if same_head(current) {
                let Some(bindings) = bind(current) else {
                    return ControlFlow::Break(());
                };
                match &result {
                    Some(previous) if previous != &bindings => return ControlFlow::Break(()),
                    None => result = Some(bindings),
                    _ => {}
                }
            }
            ControlFlow::Continue(())
        });
        return if walked.is_break() || incomplete {
            None
        } else {
            result
        };
    }
    if let Some(recv) = entry.sym.ext_receiver.as_deref() {
        if entry.sym.type_params.iter().any(|tp| tp == recv) {
            let mut subst = HashMap::new();
            subst.insert(recv.to_string(), recv_ty.clone());
            return Some(subst);
        }
    }
    if let Some(container) = entry.sym.container.as_deref() {
        let mut result = None;
        let walked = walk_supertypes(index, recv_ty, SUPERTYPE_CAP, |current| {
            if receiver_root_key(index, current).as_deref() == Some(container)
                && current.package().is_none_or(|pkg| pkg == entry.sym.package)
            {
                let Some(declaration) = type_declaration_shape(index, current) else {
                    return ControlFlow::Break(());
                };
                if declaration.sym.type_params.len() != current.args().len() {
                    return ControlFlow::Break(());
                }
                let mut bindings: HashMap<_, _> = declaration
                    .sym
                    .type_params
                    .iter()
                    .cloned()
                    .zip(current.args().iter().cloned())
                    .collect();
                // A method's own T shadows the owning class's T.
                bindings.retain(|name, _| !entry.sym.type_params.contains(name));
                match &result {
                    Some(previous) if previous != &bindings => return ControlFlow::Break(()),
                    None => result = Some(bindings),
                    _ => {}
                }
            }
            ControlFlow::Continue(())
        });
        if walked.is_break() {
            return None;
        }
        return result;
    }
    None
}

/// Receiver matching may leave a function's own variables unbound. Check their known bindings
/// again after value arguments have been unified; unbound variables still substitute to Unknown.
fn extension_bounds_hold(
    index: &dyn InferenceIndex,
    entry: &Entry,
    bindings: &HashMap<String, Type>,
) -> bool {
    let Some(receiver) = entry.sym.extension_receiver.as_deref() else {
        return true;
    };
    if receiver.has_type_constraints {
        return false;
    }
    let ctx = FileCtx::new(entry.sym.package.clone(), Vec::new());
    receiver.upper_bounds.iter().all(|(parameter, bound)| {
        // F-bounds and relationships between variables still require a constraint solver.
        if receiver_bound_mentions_parameter(bound, &entry.sym.type_params, 0) {
            return false;
        }
        let bound = resolve_type_ref(index, bound, &ctx, 0);
        if !receiver_arguments_known(index, &bound, 0) {
            return false;
        }
        let Some(actual) = bindings
            .get(parameter)
            .filter(|actual| actual.name().is_some())
        else {
            // A function's own T may come from its value arguments, independently of the receiver.
            // An unbound property variable cannot establish an applicable receiver.
            return entry.sym.kind == SymbolKind::Function;
        };
        receiver_satisfies_upper_bound(index, actual, &bound)
    })
}

fn receiver_bound_mentions_parameter(bound: &TypeRef, params: &[String], depth: usize) -> bool {
    depth > MAX_DEPTH
        || (bound.qualification.is_none() && params.contains(&bound.name))
        || bound
            .args
            .iter()
            .any(|arg| receiver_bound_mentions_parameter(arg, params, depth + 1))
}

/// Prove a nominal upper bound without gradual compatibility or name-only subtype guesses.
/// Instantiated arguments remain exact; incomplete or conflicting inheritance paths stay unknown.
fn receiver_satisfies_upper_bound(index: &dyn InferenceIndex, actual: &Type, bound: &Type) -> bool {
    if !receiver_arguments_known(index, actual, 0)
        || !receiver_arguments_known(index, bound, 0)
        || (actual.is_nullable() && !bound.is_nullable())
    {
        return false;
    }
    // Every known Kotlin/JVM type satisfies canonical Any once nullability has been checked.
    // A user type also named Any retains its ordinary nominal identity.
    if bound.name() == Some("Any")
        && bound.package() == Some("kotlin")
        && bound.container().is_none()
        && bound.args().is_empty()
    {
        return true;
    }
    let same_head = |current: &Type| {
        current.name() == bound.name()
            && current.package() == bound.package()
            && current.container() == bound.container()
    };
    if same_head(actual) {
        return actual.args() == bound.args();
    }
    let mut found = false;
    let mut incomplete = false;
    let walked = walk_supertypes(index, actual, SUPERTYPE_CAP, |current| {
        incomplete |= !receiver_arguments_known(index, current, 0);
        if same_head(current) {
            if current.args() != bound.args() {
                return ControlFlow::Break(());
            }
            found = true;
        }
        ControlFlow::Continue(())
    });
    found && !incomplete && !walked.is_break()
}

/// Exact receiver-template matching, separate from gradual argument compatibility. Positions come
/// from the declaration, and nominal heads must retain package and containing-type identity.
fn bind_receiver_template(
    index: &dyn InferenceIndex,
    formal: &TypeRef,
    actual: &Type,
    ctx: &FileCtx,
    params: &HashMap<String, bool>,
    bindings: &mut HashMap<String, Type>,
    depth: usize,
) -> bool {
    if depth > MAX_DEPTH
        || formal.name.is_empty()
        || formal.projection != Projection::Invariant
        || !type_head_is_known(index, actual)
    {
        return false;
    }
    if formal.qualification.is_none() && params.contains_key(&formal.name) {
        // T? determines T only when an explicit non-null upper bound removes the ambiguity.
        // Inside invariant argument slots, a non-null actual cannot equal the nullable template.
        if formal.nullable && (!params[&formal.name] || (depth > 0 && !actual.is_nullable())) {
            return false;
        }
        let non_nullable = formal.nullable.then(|| actual.clone().into_non_null());
        let actual = non_nullable.as_ref().unwrap_or(actual);
        if !formal.args.is_empty() || !receiver_arguments_known(index, actual, depth) {
            return false;
        }
        return match bindings.get(&formal.name) {
            Some(previous) => previous == actual,
            None => {
                bindings.insert(formal.name.clone(), actual.clone());
                true
            }
        };
    }
    if !receiver_shape_is_known(index, actual) {
        return false;
    }
    let Some((package, container)) = type_ref_identity(index, formal, ctx) else {
        return false;
    };
    if actual.name() != Some(formal.name.as_str())
        || actual.package() != package.as_deref()
        || actual.container() != container.as_deref()
        || (actual.is_nullable() && !formal.nullable)
        || (depth > 0 && actual.is_nullable() != formal.nullable)
        || actual.args().len() != formal.args.len()
    {
        return false;
    }
    formal
        .args
        .iter()
        .zip(actual.args())
        .all(|(formal, actual)| {
            bind_receiver_template(index, formal, actual, ctx, params, bindings, depth + 1)
        })
}

fn receiver_arguments_known(index: &dyn InferenceIndex, actual: &Type, depth: usize) -> bool {
    depth <= MAX_DEPTH
        && type_head_is_known(index, actual)
        && receiver_shape_is_known(index, actual)
        && actual
            .args()
            .iter()
            .all(|argument| receiver_arguments_known(index, argument, depth + 1))
}

// Callers already prove the nominal head; this checks its declaration shape and argument count.
fn receiver_shape_is_known(index: &dyn InferenceIndex, actual: &Type) -> bool {
    if let Some(declaration) = type_declaration_shape(index, actual) {
        return declaration.sym.type_params.len() == actual.args().len();
    }
    // A missing fundamental type needs no source declaration. Conflicting declarations are not
    // missing: they must never fall through to the zero-argument builtin rule.
    actual.container().is_none()
        && actual.args().is_empty()
        && actual.name().is_some_and(|name| {
            crate::defaults::is_builtin_type(primitive_type_name(name))
                && actual
                    .package()
                    .is_none_or(|package| package == "kotlin" || package == "java.lang")
                && !index.lookup_type(name).iter().any(|entry| {
                    actual.package().is_none_or(|pkg| pkg == entry.sym.package)
                        && actual.container() == entry.sym.container.as_deref()
                })
        })
}

/// Exact argument positions need only an agreed nominal shape. For example, a library can contain
/// both expect and actual declarations with the same parameters but different parent lists.
fn type_declaration_shape<'a>(index: &'a dyn InferenceIndex, ty: &Type) -> Option<&'a Entry> {
    let mut result: Option<&Entry> = None;
    for entry in index.lookup_type(ty.name()?) {
        if ty.package().is_some_and(|pkg| pkg != entry.sym.package)
            || ty.container() != entry.sym.container.as_deref()
        {
            continue;
        }
        if let Some(previous) = result {
            if previous.sym.package != entry.sym.package
                || previous.sym.kind != entry.sym.kind
                || previous.sym.type_params != entry.sym.type_params
            {
                return None;
            }
        }
        result = Some(entry);
    }
    result
}

/// Parent traversal additionally requires all declarations of the nominal identity to agree on
/// their instantiated hierarchy; shape agreement alone cannot prove a subtype relationship.
fn type_declaration<'a>(index: &'a dyn InferenceIndex, ty: &Type) -> Option<&'a Entry> {
    let mut result: Option<&Entry> = None;
    for entry in index.lookup_type(ty.name()?) {
        if ty.package().is_some_and(|pkg| pkg != entry.sym.package)
            || ty.container() != entry.sym.container.as_deref()
        {
            continue;
        }
        if let Some(previous) = result {
            if previous.sym.package != entry.sym.package
                || previous.sym.kind != entry.sym.kind
                || previous.sym.type_params != entry.sym.type_params
                || previous.sym.supertype_refs != entry.sym.supertype_refs
                || previous.sym.supertypes != entry.sym.supertypes
            {
                return None;
            }
        }
        result = Some(entry);
    }
    result
}

/// Visit a receiver and its instantiated supertypes, stopping when `visit` breaks.
/// Unresolved supertypes visit `Unknown` so consumers cannot mistake an incomplete hierarchy
/// for a closed source world when proving absence.
/// Declaration identity is path-local: cycles cannot grow arguments indefinitely, while
/// separate inheritance paths may still carry conflicting bindings to the same declaration.
pub fn walk_supertypes(
    index: &dyn InferenceIndex,
    receiver: &Type,
    max_depth: usize,
    mut visit: impl FnMut(&Type) -> ControlFlow<()>,
) -> ControlFlow<()> {
    let mut visited = HashSet::new();
    let mut frontier = vec![(receiver.clone(), Vec::new())];
    while let Some((current, mut path)) = frontier.pop() {
        let Some(name) = current.name() else {
            if visited.insert(current.clone()) {
                visit(&current)?;
            }
            continue;
        };
        let identity = (
            name.to_string(),
            current.package().map(str::to_string),
            current.container().map(str::to_string),
        );
        if path.len() > max_depth || path.contains(&identity) {
            if visited.insert(Type::Unknown) {
                visit(&Type::Unknown)?;
            }
            continue;
        }
        if !visited.insert(current.clone()) {
            continue;
        }
        visit(&current)?;
        path.push(identity);
        for supertype in direct_supertypes(index, &current) {
            frontier.push((supertype, path.clone()));
        }
    }
    ControlFlow::Continue(())
}

/// Instantiate a type's direct superclasses/interfaces using its actual type arguments.
pub fn direct_supertypes(index: &dyn InferenceIndex, ty: &Type) -> Vec<Type> {
    let Some(declaration) = type_declaration(index, ty) else {
        return vec![Type::Unknown];
    };
    let params: HashSet<_> = declaration.sym.type_params.iter().cloned().collect();
    let subst: HashMap<_, _> = if declaration.sym.type_params.len() == ty.args().len() {
        declaration
            .sym
            .type_params
            .iter()
            .cloned()
            .zip(ty.args().iter().cloned())
            .collect()
    } else {
        HashMap::new()
    };
    let ctx = FileCtx::new(declaration.sym.package.clone(), Vec::new());
    if declaration.sym.supertype_refs.is_empty() {
        return declaration
            .sym
            .supertypes
            .iter()
            .map(|name| resolve_with_subst(index, &TypeRef::simple(name), &ctx, &params, &subst, 0))
            .collect();
    }
    declaration
        .sym
        .supertype_refs
        .iter()
        .map(|tr| resolve_with_subst(index, tr, &ctx, &params, &subst, 0))
        .collect()
}

fn bind_lambda_return_type(
    index: &dyn InferenceIndex,
    op_name: &str,
    call: Node,
    src: &str,
    ctx: &FileCtx,
    ret_tr: &TypeRef,
    tparams: &HashSet<String>,
    subst: &mut HashMap<String, Type>,
    depth: usize,
    child_infer: &mut impl ChildInfer,
) {
    if !RETURN_BINDING_LAMBDA_OPS.contains(&op_name) {
        return;
    }
    let Some(lambda_ty) = trailing_lambda_result_type(index, call, src, ctx, depth, child_infer)
    else {
        return;
    };
    if lambda_ty.name().is_none() {
        return;
    }
    let mut needed = Vec::new();
    collect_unbound_return_type_params(ret_tr, tparams, subst, &mut needed);
    needed.sort();
    needed.dedup();
    if needed.len() == 1 {
        subst.entry(needed[0].clone()).or_insert(lambda_ty);
    }
}

fn collect_unbound_return_type_params(
    tr: &TypeRef,
    tparams: &HashSet<String>,
    subst: &HashMap<String, Type>,
    out: &mut Vec<String>,
) {
    if tparams.contains(&tr.name) && !subst.contains_key(&tr.name) {
        out.push(tr.name.clone());
    }
    for arg in &tr.args {
        collect_unbound_return_type_params(arg, tparams, subst, out);
    }
}

fn trailing_lambda_result_type(
    index: &dyn InferenceIndex,
    call: Node,
    src: &str,
    ctx: &FileCtx,
    depth: usize,
    child_infer: &mut impl ChildInfer,
) -> Option<Type> {
    let annotated = child_of_kind(call, "annotated_lambda")?;
    let lambda = child_of_kind(annotated, "lambda_literal")?;
    let expr = lambda_tail_expr(lambda)?;
    Some(child_infer.infer_child(index, expr, src, ctx, depth + 1))
}

fn lambda_tail_expr(lambda: Node) -> Option<Node> {
    let mut cursor = lambda.walk();
    let mut tail = None;
    for child in lambda.named_children(&mut cursor) {
        if child.kind() == "lambda_parameters" {
            continue;
        }
        tail = Some(child);
    }
    let tail = tail?;
    if tail.kind() == "statements" {
        let mut cursor = tail.walk();
        let mut stmt = None;
        for child in tail.named_children(&mut cursor) {
            stmt = Some(child);
        }
        stmt
    } else {
        Some(tail)
    }
}

fn named_type_params(
    index: &dyn InferenceIndex,
    name: &str,
    preferred_pkg: &str,
) -> Option<Vec<String>> {
    let (container, simple) = match name.rsplit_once('.') {
        Some((container, simple)) => (Some(container.to_string()), simple),
        None => (None, name),
    };
    type_declaration_shape(
        index,
        &Type::Class {
            name: simple.to_string(),
            package: Some(preferred_pkg.to_string()),
            container,
            nullable: false,
            args: Vec::new(),
        },
    )
    .map(|entry| entry.sym.type_params.clone())
}

/// The declared type ref to read from a member entry: a function's `return_type` (when
/// `want_function`) or a property's `value_type`.
fn member_type_ref(e: &crate::index::Entry, want_function: bool) -> Option<&TypeRef> {
    if want_function {
        if e.sym.kind == SymbolKind::Function {
            e.sym.return_type.as_ref()
        } else {
            e.sym
                .value_type
                .as_ref()
                .filter(|ty| !ty.nullable)
                .and_then(members::function_type_signature)
                .map(|(_, result)| result)
        }
    } else {
        (e.sym.kind == SymbolKind::Property)
            .then(|| e.sym.value_type.as_ref())
            .flatten()
    }
}

/// Resolve a supertype's package: prefer the same package as the subtype (the common case), else
/// leave unfiltered (`None`) rather than guess. Mirrors the completion supertype-walk rule.
fn same_pkg_supertype(
    index: &dyn InferenceIndex,
    sup: &str,
    cur_pkg: Option<&str>,
) -> Option<String> {
    match cur_pkg {
        Some(p) if index.lookup_type(sup).iter().any(|e| e.sym.package == p) => Some(p.to_string()),
        _ => None,
    }
}

/// Resolve a simple type NAME to a [`Type`], picking its package from the use-site context.
fn resolve_type_name(
    index: &dyn InferenceIndex,
    name: &str,
    ctx: &FileCtx,
    nullable: bool,
) -> Type {
    Type::Class {
        name: name.to_string(),
        package: resolve_package(index, name, ctx),
        container: resolve_container(index, name, ctx),
        nullable,
        args: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::named_type_params;
    use crate::index::{Index, Tier};
    use crate::symbol::{IndexedSymbol, SymbolKind};

    #[test]
    fn supertype_walk_reports_cycles_and_depth_limits_as_incomplete() {
        use crate::types::{Type, TypeRef};
        use std::ops::ControlFlow;
        let mut index = Index::new();
        let mut a = IndexedSymbol::new("A", SymbolKind::Class, "demo", None, 0, 1);
        a.supertype_refs = vec![TypeRef::simple("B")];
        let mut b = IndexedSymbol::new("B", SymbolKind::Class, "demo", None, 2, 3);
        b.supertype_refs = vec![TypeRef::simple("A")];
        index.replace_file("Cycle.kt", vec![a, b], Tier::Volatile);
        for (limit, expected) in [
            (0, vec![Some("A"), None]),
            (32, vec![Some("A"), Some("B"), None]),
        ] {
            let mut visited = Vec::new();
            let result = super::walk_supertypes(
                &index,
                &Type::class("A", Some("demo".into())),
                limit,
                |ty| {
                    visited.push(ty.name().map(str::to_string));
                    ControlFlow::Continue(())
                },
            );
            assert_eq!(result, ControlFlow::Continue(()));
            assert_eq!(
                visited
                    .iter()
                    .map(|name| name.as_deref())
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }

    #[test]
    fn inference_stops_a_generic_cycle_before_expanding_the_same_declaration() {
        use crate::index::{InferenceIndex, LookupEntries};
        use crate::parser::KotlinParser;
        use std::cell::Cell;

        struct CountIndex {
            index: Index,
            cyclic_member_queries: Cell<usize>,
        }
        impl InferenceIndex for CountIndex {
            fn lookup_by_name(&self, name: &str) -> LookupEntries<'_> {
                LookupEntries::All(self.index.lookup_by_name(name))
            }
            fn members_of(&self, name: &str) -> LookupEntries<'_> {
                if name == "A" || name == "B" {
                    self.cyclic_member_queries
                        .set(self.cyclic_member_queries.get() + 1);
                }
                LookupEntries::All(self.index.members_of(name))
            }
            fn extensions_for(&self, name: &str) -> LookupEntries<'_> {
                LookupEntries::All(self.index.extensions_for(name))
            }
        }

        let source = "package app\nfun run(value: A<Item>) { value.missing() }\n";
        for (declarations, expected_queries) in [
            ("package app\nclass Box<T>\nopen class A<T> : A<Box<T>>()\nclass Item\n", 1),
            ("package app\nclass Pair<L, R>\nopen class A<T> : A<Pair<T, T>>()\nclass Item\n", 1),
            ("package app\nclass Pair<L, R>\nopen class A<T> : B<Pair<T, T>>()\nopen class B<T> : A<Pair<T, T>>()\nclass Item\n", 2),
        ] {
            let mut parser = KotlinParser::new();
            let mut raw_index = Index::new();
            raw_index.replace_file(
                "Types.kt",
                crate::indexer::extract_symbols(&parser.parse(declarations), declarations, "app"),
                Tier::Volatile,
            );
            let index = CountIndex {
                index: raw_index,
                cyclic_member_queries: Cell::new(0),
            };
            let tree = parser.parse(source);
            let expression = "value.missing()";
            let start = source.find(expression).unwrap();
            let node = tree
                .root_node()
                .descendant_for_byte_range(start, start + expression.len())
                .unwrap();
            assert_eq!(
                super::infer(&index, node, source, &super::FileCtx::from_tree(&tree, source)),
                crate::types::Type::Unknown
            );
            assert_eq!(
                index.cyclic_member_queries.get(),
                expected_queries,
                "different type arguments must not allow revisiting the same declaration: {declarations}"
            );
        }
    }

    #[test]
    fn nested_named_member_arguments_are_inferred_linearly() {
        use crate::index::InferenceIndex;
        use crate::parser::KotlinParser;
        use crate::types::Type;
        use tree_sitter::Node;

        struct CountInfer {
            calls: usize,
        }
        impl super::ChildInfer for CountInfer {
            fn infer_child(
                &mut self,
                index: &dyn InferenceIndex,
                node: Node<'_>,
                src: &str,
                ctx: &super::FileCtx,
                depth: usize,
            ) -> Type {
                self.calls += 1;
                // Use the default local recursion so a child cache cannot hide repeated traversal.
                super::infer_depth_with(index, node, src, ctx, depth, self)
            }
        }

        let mut counts = Vec::new();
        for depth in [1usize, 8] {
            let mut expression = "box".to_string();
            for _ in 0..depth {
                expression = format!("box.id(value={expression})");
            }
            let source = format!(
                "package app\nclass Box {{ fun id(value: Box): Box = value }}\nfun use(box: Box) {{ val result = {expression} }}\n"
            );
            let mut parser = KotlinParser::new();
            let tree = parser.parse(&source);
            let mut index = Index::new();
            index.replace_file(
                "Named.kt",
                crate::indexer::extract_symbols(&tree, &source, "app"),
                Tier::Volatile,
            );
            let start = source.find(&expression).unwrap();
            let node = tree
                .root_node()
                .descendant_for_byte_range(start, start + expression.len())
                .unwrap();
            assert_eq!(node.kind(), "call_expression");
            assert_eq!(super::node_text(node, &source), expression);
            let ctx = super::FileCtx::from_tree(&tree, &source);
            let mut child_infer = CountInfer { calls: 0 };
            assert_eq!(
                super::infer_with_child_infer(&index, node, &source, &ctx, 0, &mut child_infer),
                Type::class("Box", Some("app".into())),
                "named calls must retain a useful result at depth {depth}"
            );
            assert!(child_infer.calls > 0);
            counts.push(child_infer.calls);
        }
        eprintln!("nested named inference callback counts at depths 1/8: {counts:?}");
        assert!(
            counts[1] <= 8 * counts[0],
            "named argument inference must scale with syntax depth: {counts:?}"
        );
    }

    #[test]
    fn named_type_params_returns_none_for_empty_lookup() {
        let index = Index::new();
        assert_eq!(named_type_params(&index, "Missing", "demo"), None);
    }

    #[test]
    fn named_type_params_prefers_matching_package() {
        let mut index = Index::new();

        let mut demo = IndexedSymbol::new("Box", SymbolKind::Class, "demo", None, 0, 3);
        demo.type_params = vec!["T".to_string()];
        index.replace_file("demo.kt", vec![demo], Tier::Volatile);

        let mut other = IndexedSymbol::new("Box", SymbolKind::Class, "other", None, 0, 3);
        other.type_params = vec!["U".to_string()];
        index.replace_file("other.kt", vec![other], Tier::Volatile);

        assert_eq!(
            named_type_params(&index, "Box", "demo"),
            Some(vec!["T".to_string()])
        );
    }
}

/// Resolve a stored [`TypeRef`] (name + nullability + args) to a [`Type`] at the use site.
fn resolve_type_ref(index: &dyn InferenceIndex, tr: &TypeRef, ctx: &FileCtx, depth: usize) -> Type {
    if tr.projection == Projection::Star {
        return Type::Star;
    }
    Type::projected(
        tr.projection,
        resolve_type_ref_unprojected(index, tr, ctx, depth),
    )
}

fn resolve_type_ref_unprojected(
    index: &dyn InferenceIndex,
    tr: &TypeRef,
    ctx: &FileCtx,
    depth: usize,
) -> Type {
    if tr.name.is_empty() {
        return Type::Unknown;
    }
    let Some((package, container)) = type_ref_identity(index, tr, ctx) else {
        return Type::Unknown;
    };
    let args = if depth > MAX_DEPTH {
        Vec::new()
    } else {
        tr.args
            .iter()
            .map(|a| resolve_type_ref(index, a, ctx, depth + 1))
            .collect()
    };
    Type::Class {
        name: tr.name.clone(),
        package,
        container,
        nullable: tr.nullable,
        args,
    }
}

/// Resolve normalized declaration facts without requiring a particular source grammar.
pub fn declared_type(index: &dyn InferenceIndex, tr: &TypeRef, ctx: &FileCtx) -> Type {
    resolve_type_ref(index, tr, ctx, 0)
}

fn type_ref_identity(
    index: &dyn InferenceIndex,
    tr: &TypeRef,
    ctx: &FileCtx,
) -> Option<(Option<String>, Option<String>)> {
    if is_intrinsic_type_ref(index, tr, ctx) {
        return Some((Some(intrinsic_package(index, &tr.name).to_string()), None));
    }
    if tr.qualification.is_some() {
        qualified_type_identity(index, tr, ctx)
            .map(|(package, container)| (Some(package), container))
    } else {
        let package = if tr.package_candidates.is_empty() {
            resolve_package(index, &tr.name, ctx)
        } else {
            let package = package_from_decl_candidates(index, tr);
            if package.is_none() && !index.lookup_type(&tr.name).is_empty() {
                return None;
            }
            package
        };
        Some((package, resolve_type_ref_container(index, tr, ctx)))
    }
}

fn intrinsic_type(index: &dyn InferenceIndex, name: &str) -> Type {
    Type::class(name, Some(intrinsic_package(index, name).to_string()))
}

fn intrinsic_package(index: &dyn InferenceIndex, name: &str) -> &'static str {
    // Kotlin's JVM String is java.lang.String. Keep its actual source members usable when only
    // JDK sources are available, while never binding literals to a user class named String.
    if name == "String" {
        let candidates = index.lookup_type(name);
        let has_top_level = |package: &str| {
            candidates
                .iter()
                .any(|entry| entry.sym.package == package && entry.sym.container.is_none())
        };
        if !has_top_level("kotlin") && has_top_level("java.lang") {
            return "java.lang";
        }
    }
    "kotlin"
}

/// Default-imported fundamental types do not require an indexed source span. Explicit imports and
/// visible user declarations still bind their own identities; unrelated global homonyms do not.
pub(crate) fn is_intrinsic_type_ref(
    index: &dyn InferenceIndex,
    tr: &TypeRef,
    ctx: &FileCtx,
) -> bool {
    if !crate::defaults::is_builtin_type(&tr.name) {
        return false;
    }
    if let Some(qualification) = &tr.qualification {
        return qualification == "kotlin";
    }
    if tr.package_candidates.is_empty() {
        if let Some(import) = ctx
            .imports
            .iter()
            .find(|import| !import.wildcard && import.local_name() == Some(tr.name.as_str()))
        {
            return import.path == format!("kotlin.{}", tr.name);
        }
    } else if !tr
        .package_candidates
        .iter()
        .any(|package| package == "kotlin")
    {
        return false;
    }
    !index.lookup_type(&tr.name).iter().any(|entry| {
        entry.sym.package != "kotlin"
            // The top-level JVM String represents Kotlin String, not a competing user type.
            && !(tr.name == "String"
                && entry.sym.package == "java.lang"
                && entry.sym.container.is_none())
            && if tr.package_candidates.is_empty() {
                entry.sym.package == ctx.package
                    || ctx
                        .imports
                        .iter()
                        .any(|import| import.wildcard && import.package() == entry.sym.package)
                    || resolve::is_default_import_pkg(&entry.sym.package)
            } else {
                tr.package_candidates.contains(&entry.sym.package)
            }
    })
}

fn qualified_type_identity(
    index: &dyn InferenceIndex,
    tr: &TypeRef,
    ctx: &FileCtx,
) -> Option<(String, Option<String>)> {
    let qualification = tr.qualification.as_deref()?;
    let expanded;
    let qualification = if tr.package_candidates.is_empty() {
        let first = qualification.split('.').next().unwrap_or(qualification);
        if let Some(import) = ctx
            .imports
            .iter()
            .find(|import| import.local_name() == Some(first))
        {
            expanded = format!("{}{}", import.path, &qualification[first.len()..]);
            expanded.as_str()
        } else {
            qualification
        }
    } else {
        qualification
    };
    let candidates = index.lookup_type(&tr.name);
    let matching = |prefix: &str| {
        let mut identities = HashSet::new();
        for entry in &candidates {
            let declaring_prefix = match &entry.sym.container {
                Some(container) if entry.sym.package.is_empty() => container.clone(),
                Some(container) => format!("{}.{container}", entry.sym.package),
                None => entry.sym.package.clone(),
            };
            if declaring_prefix == prefix {
                identities.insert((entry.sym.package.clone(), entry.sym.container.clone()));
            }
        }
        identities
    };
    let exact = matching(qualification);
    if !exact.is_empty() {
        return (exact.len() == 1).then(|| exact.into_iter().next().unwrap());
    }
    let contextual;
    let packages = if tr.package_candidates.is_empty() {
        contextual = std::iter::once(ctx.package.clone())
            .chain(
                ctx.imports
                    .iter()
                    .filter(|import| import.wildcard)
                    .map(Import::package),
            )
            .chain(
                resolve::DEFAULT_IMPORT_PACKAGES
                    .iter()
                    .map(|package| (*package).to_string()),
            )
            .collect::<Vec<_>>();
        contextual.as_slice()
    } else {
        tr.package_candidates.as_slice()
    };
    let mut identities = HashSet::new();
    for (position, package) in packages.iter().enumerate() {
        let prefix = if package.is_empty() {
            qualification.to_string()
        } else {
            format!("{package}.{qualification}")
        };
        let matches = matching(&prefix);
        if position == 0 && !matches.is_empty() {
            return (matches.len() == 1).then(|| matches.into_iter().next().unwrap());
        }
        identities.extend(matches);
    }
    (identities.len() == 1).then(|| identities.into_iter().next().unwrap())
}

/// Resolve a stored container candidate, or the current file's visible nested type.
fn resolve_type_ref_container(
    index: &dyn InferenceIndex,
    tr: &TypeRef,
    ctx: &FileCtx,
) -> Option<String> {
    container_from_decl_candidates(index, tr).or_else(|| resolve_container(index, &tr.name, ctx))
}

fn package_from_decl_candidates(index: &dyn InferenceIndex, tr: &TypeRef) -> Option<String> {
    let candidates = index.lookup_type(&tr.name);
    if candidates.is_empty() {
        return None;
    }
    let mut pkgs = tr.package_candidates.iter();
    let primary = pkgs.next()?;
    if candidates.iter().any(|e| &e.sym.package == primary) {
        return Some(primary.clone());
    }
    let mut rest_hits: Vec<String> = Vec::new();
    for pkg in pkgs {
        if candidates.iter().any(|e| &e.sym.package == pkg) && !rest_hits.iter().any(|p| p == pkg) {
            rest_hits.push(pkg.clone());
        }
    }
    match rest_hits.as_slice() {
        [one] => Some(one.clone()),
        _ => None,
    }
}

fn container_from_decl_candidates(index: &dyn InferenceIndex, tr: &TypeRef) -> Option<String> {
    let candidates = index.lookup_type(&tr.name);
    if candidates.is_empty() || tr.container_candidates.is_empty() {
        return None;
    }
    let mut containers = tr.container_candidates.iter();
    let primary = containers.next()?;
    if candidates
        .iter()
        .any(|e| e.sym.container.as_deref() == Some(primary.as_str()))
    {
        return Some(primary.clone());
    }
    let mut rest_hits = Vec::new();
    for container in containers {
        if candidates
            .iter()
            .any(|e| e.sym.container.as_deref() == Some(container.as_str()))
            && !rest_hits.iter().any(|seen| seen == container)
        {
            rest_hits.push(container.clone());
        }
    }
    match rest_hits.as_slice() {
        [one] => Some(one.clone()),
        _ => None,
    }
}

/// Resolve a simple type name to the package of the type it refers to in this file's context (the
/// old `resolve_type_package`, now keyed off [`FileCtx`]). Precedence: a single candidate wins
/// outright; otherwise alias/explicit import > same package > a single wildcard-imported match > a
/// single Kotlin default-import match. `None` when genuinely ambiguous (callers then don't
/// package-filter — best-effort over dropping results).
fn resolve_package(index: &dyn InferenceIndex, name: &str, ctx: &FileCtx) -> Option<String> {
    let candidates = index.lookup_type(name);
    match candidates.as_slice() {
        [] => None,
        [only] => Some(only.sym.package.clone()),
        _ => {
            for imp in &ctx.imports {
                let binds = imp.alias.as_deref() == Some(name)
                    || (!imp.wildcard && imp.local_name() == Some(name));
                if binds {
                    let pkg = imp.package();
                    if candidates.iter().any(|e| e.sym.package == pkg) {
                        return Some(pkg);
                    }
                }
            }
            if candidates.iter().any(|e| e.sym.package == ctx.package) {
                return Some(ctx.package.clone());
            }
            let star: Vec<String> = ctx
                .imports
                .iter()
                .filter(|i| i.wildcard)
                .map(|i| i.package())
                .collect();
            let starred: Vec<_> = candidates
                .iter()
                .filter(|e| star.contains(&e.sym.package))
                .collect();
            if let [one] = starred.as_slice() {
                return Some(one.sym.package.clone());
            }
            let defaulted: Vec<_> = candidates
                .iter()
                .filter(|e| resolve::is_default_import_pkg(&e.sym.package))
                .collect();
            if let [one] = defaulted.as_slice() {
                return Some(one.sym.package.clone());
            }
            None
        }
    }
}

fn resolve_container(index: &dyn InferenceIndex, name: &str, ctx: &FileCtx) -> Option<String> {
    let candidates = visible_type_candidates(index, name, ctx);
    match candidates.as_slice() {
        [] => None,
        [only] => only.sym.container.clone(),
        _ => None,
    }
}

fn visible_type_candidates<'a>(
    index: &'a dyn InferenceIndex,
    name: &str,
    ctx: &FileCtx,
) -> Vec<&'a Entry> {
    let candidates = index.lookup_type(name);
    if candidates.len() <= 1 {
        return candidates;
    }
    for imp in &ctx.imports {
        let binds =
            imp.alias.as_deref() == Some(name) || (!imp.wildcard && imp.local_name() == Some(name));
        if binds {
            let pkg = imp.package();
            let hits: Vec<_> = candidates
                .iter()
                .copied()
                .filter(|e| e.sym.package == pkg)
                .collect();
            if !hits.is_empty() {
                return hits;
            }
        }
    }
    let same_pkg: Vec<_> = candidates
        .iter()
        .copied()
        .filter(|e| e.sym.package == ctx.package)
        .collect();
    if !same_pkg.is_empty() {
        return same_pkg;
    }
    let star: Vec<String> = ctx
        .imports
        .iter()
        .filter(|i| i.wildcard)
        .map(|i| i.package())
        .collect();
    let starred: Vec<_> = candidates
        .iter()
        .copied()
        .filter(|e| star.contains(&e.sym.package))
        .collect();
    if !starred.is_empty() {
        return starred;
    }
    candidates
        .into_iter()
        .filter(|e| resolve::is_default_import_pkg(&e.sym.package))
        .collect()
}
