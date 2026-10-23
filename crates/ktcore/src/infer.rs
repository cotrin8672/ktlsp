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
use crate::types::{Projection, Type, TypeParameterBounds, TypeRef};

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
    file: Option<Arc<str>>,
    lexical_types: Vec<(std::ops::Range<usize>, String, usize, usize)>,
    super_accesses: Vec<std::ops::Range<usize>>,
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
            file: None,
            lexical_types: Vec::new(),
            super_accesses: Vec::new(),
            local_extensions: None,
            inference_cache: RefCell::new(FxHashMap::default()),
            receiver_cache: RefCell::new(FxHashMap::default()),
        }
    }

    pub fn from_tree(tree: &Tree, src: &str) -> Self {
        let mut ctx = FileCtx::new(package_of(tree, src), imports_of(tree, src));
        fn collect(node: Node<'_>, src: &str, ctx: &mut FileCtx) {
            if matches!(
                node.kind(),
                "class_declaration"
                    | "object_declaration"
                    | "interface_declaration"
                    | "enum_declaration"
                    | "record_declaration"
                    | "annotation_type_declaration"
            ) {
                if let Some(name) = name_field(node).filter(|name| !name.is_missing()) {
                    ctx.lexical_types.push((
                        node.byte_range(),
                        node_text(name, src).into(),
                        name.start_byte(),
                        name.end_byte(),
                    ));
                }
            }
            let access = match node.kind() {
                "navigation_expression" => node.named_child(0).zip(node.named_child(1)),
                "method_invocation" => node
                    .child_by_field_name("object")
                    .zip(node.child_by_field_name("name")),
                "field_access" => node
                    .child_by_field_name("object")
                    .zip(node.child_by_field_name("field")),
                _ => None,
            };
            if let Some((receiver, selector)) = access
                .filter(|(receiver, _)| matches!(receiver.kind(), "super" | "super_expression"))
            {
                // Arguments cannot inherit their outer call's super access.
                ctx.super_accesses
                    .push(receiver.start_byte()..selector.end_byte());
            }
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect(child, src, ctx);
            }
        }
        collect(tree.root_node(), src, &mut ctx);
        ctx
    }

    /// Bind access to the exact current lexical declaration, never a same-spelled class.
    pub fn caller_type(&self, index: &dyn InferenceIndex, position: usize) -> Option<Type> {
        let (_, name, start, end) = self
            .lexical_types
            .iter()
            .filter(|(range, _, _, _)| range.contains(&position))
            .min_by_key(|(range, _, _, _)| range.len())?;
        let file = self.file.as_deref()?;
        let owners: Vec<_> = index
            .lookup_type(name)
            .into_iter()
            .filter(|entry| {
                entry.path.as_ref() == file
                    && entry.sym.package == self.package
                    && entry.sym.start_byte == *start
                    && entry.sym.end_byte == *end
            })
            .collect();
        let [owner] = owners.as_slice() else {
            return None;
        };
        Some(Type::Class {
            name: owner.sym.name.clone(),
            package: Some(owner.sym.package.clone()),
            container: owner.sym.container.clone(),
            nullable: false,
            args: Vec::new(),

            owner: None,
        })
    }

    pub(crate) fn super_access_at(&self, position: usize) -> bool {
        self.super_accesses
            .iter()
            .any(|range| range.contains(&position))
    }

    pub fn with_file(mut self, file: &str) -> Self {
        self.file = (!file.is_empty()).then(|| Arc::from(file));
        self.inference_cache.get_mut().clear();
        self.receiver_cache.get_mut().clear();
        self
    }

    pub fn file(&self) -> Option<&str> {
        self.file.as_deref()
    }

    pub fn from_tree_in_file(tree: &Tree, src: &str, file: &str) -> Self {
        let mut ctx = Self::from_tree(tree, src).with_file(file);
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
        "null_literal" => intrinsic_type(index, "Nothing").with_nullability(true),
        "identifier" => infer_identifier(index, node, src, ctx, depth, child_infer),
        "lambda_literal" => {
            infer_lambda(index, node, src, ctx, depth, child_infer).unwrap_or(Type::Unknown)
        }
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
        "index_expression" if !node.has_error() && node.named_child_count() == 2 => {
            let receiver =
                child_infer.infer_child(index, node.named_child(0).unwrap(), src, ctx, depth + 1);
            let argument =
                child_infer.infer_child(index, node.named_child(1).unwrap(), src, ctx, depth + 1);
            if matches!(&receiver, Type::Class {
                name, package: Some(package), container: None, nullable: false, args,
                owner: None,
            } if package == "kotlin" && (name == "Array" && args.len() == 1
                || matches!(name.as_str(), "BooleanArray" | "ByteArray" | "CharArray"
                    | "ShortArray" | "IntArray" | "LongArray" | "FloatArray" | "DoubleArray")
                    && args.is_empty()))
            {
                return member_type(
                    index,
                    &receiver,
                    members::candidates_at(index, &receiver, "get", ctx, node.start_byte()),
                    "get",
                    true,
                    Some(1),
                    Some(&[argument]),
                    None,
                    src,
                    ctx,
                    depth,
                    child_infer,
                );
            }
            members::kotlin_java_array_member_type(
                &receiver,
                "get",
                &MemberUse::Call(CallArguments {
                    count: Some(1),
                    types: vec![argument],
                    names: None,
                    has_spread: false,
                    trailing_lambda: false,
                    trailing_lambda_omits_or_has_zero_parameters: false,
                }),
            )
            .unwrap_or(Type::Unknown)
        }
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

        owner: None,
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
    if !is_stable_for_narrowing(index, ident, name, guard_start, src, ctx) {
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
    if !is_stable_for_narrowing(index, ident, name, cond.start_byte(), src, ctx) {
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
    if !is_stable_for_narrowing(index, ident, name, be.start_byte(), src, ctx) {
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
        if !is_stable_for_narrowing(index, ident, name, cond.start_byte(), src, ctx) {
            return None;
        }
        return resolve_narrowed_type(index, ut, src, ctx);
    }

    let narrowed = guard_type(index, cond, name, src, ctx)?;
    if !is_stable_for_narrowing(index, ident, name, cond.start_byte(), src, ctx) {
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
fn is_stable_for_narrowing(
    index: &dyn InferenceIndex,
    ident: Node,
    name: &str,
    check_start: usize,
    src: &str,
    ctx: &FileCtx,
) -> bool {
    if name == "it" {
        let mut scope = ident.parent();
        while let Some(lambda) = scope {
            if lambda.kind() == "lambda_literal" {
                if child_of_kind(lambda, "lambda_parameters").is_none()
                    && property_lambda_parameter_type(index, lambda, ident, name, src, ctx)
                        .is_some()
                {
                    // An implicit parameter shadows outer `it` binders and their guards.
                    return lambda.start_byte() < check_start;
                }
                break;
            }
            scope = lambda.parent();
        }
    }
    let Some((decl, kind)) = resolve::local_decl(ident, name, UseKind::Value, src) else {
        return false;
    };
    if decl.start_byte() > check_start {
        return false;
    }
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

        owner: None,
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
            "object_literal" => Some(Type::Unknown),
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
    let ty = extension_function_receiver_ref(declaration, src).map(|receiver| {
        if depth >= MAX_DEPTH {
            return Type::Unknown;
        }
        declared_type_in_scope(
            index,
            &receiver,
            ctx,
            &crate::indexer::type_parameters_in_scope(declaration, src, &ctx.package, &ctx.imports),
        )
    });
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
    let ty = Some(name_field(declaration).map_or(Type::Unknown, |name_node| {
        if ctx.file.is_some() {
            ctx.caller_type(index, name_node.start_byte())
                .unwrap_or(Type::Unknown)
        } else {
            resolve_type_name(index, node_text(name_node, src), ctx, false)
        }
    }));
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
            let is_this_lambdas_param = (implicit
                && child_of_kind(n, "lambda_parameters").is_none())
                || lambda_declares_param(n, name, src);
            if is_this_lambdas_param {
                if let Some(parameters) = child_of_kind(n, "lambda_parameters") {
                    let mut cursor = parameters.walk();
                    for parameter in parameters.named_children(&mut cursor) {
                        if first_ident(parameter).is_some_and(|id| node_text(id, src) == name) {
                            if let Some(reference) =
                                crate::indexer::value_type_of_scoped(parameter, src, None)
                            {
                                let bounds = crate::indexer::type_parameters_in_scope(
                                    parameter,
                                    src,
                                    &ctx.package,
                                    &ctx.imports,
                                );
                                return Some(declared_type_in_scope(
                                    index, &reference, ctx, &bounds,
                                ));
                            }
                        }
                    }
                }
                if let Some(ty) = property_lambda_parameter_type(index, n, ident, name, src, ctx) {
                    return Some(narrowed_type(index, ident, name, src, ctx).unwrap_or(ty));
                }
                return lambda_receiver_or_element(index, n, src, ctx, depth, child_infer);
            }
            // a named parameter not declared here — keep walking outward to its declaring lambda
        }
        cur = n.parent();
    }
    None
}

/// A direct property initializer has an authored expected type without overload or call inference.
fn property_lambda_parameter_type(
    index: &dyn InferenceIndex,
    lambda: Node,
    usage: Node,
    name: &str,
    src: &str,
    ctx: &FileCtx,
) -> Option<Type> {
    let mut expression = lambda;
    while let Some(parent) = expression
        .parent()
        .filter(|parent| parent.kind() == "parenthesized_expression")
    {
        expression = parent;
    }
    let property = expression
        .parent()
        .filter(|parent| parent.kind() == "property_declaration")?;
    let declaration = child_of_kind(property, "variable_declaration")?;
    let reference = crate::indexer::value_type_of_scoped(declaration, src, None)?;
    let (formals, _) = members::function_type_signature(&reference)?;
    let mut annotation = declaration;
    let function_type = loop {
        let mut cursor = annotation.walk();
        annotation = annotation.named_children(&mut cursor).find(|child| {
            matches!(
                child.kind(),
                "function_type" | "parenthesized_type" | "nullable_type"
            )
        })?;
        if annotation.kind() == "function_type" {
            break annotation;
        }
    };
    fn contains_suspend(node: Node) -> bool {
        let mut cursor = node.walk();
        node.kind() == "suspend" || node.children(&mut cursor).any(contains_suspend)
    }
    let source_parameters = child_of_kind(function_type, "function_type_parameters")?;
    let mut cursor = function_type.walk();
    if contains_suspend(declaration)
        || function_type.named_children(&mut cursor).any(|child| {
            child.end_byte() <= source_parameters.start_byte()
                && matches!(
                    child.kind(),
                    "user_type" | "nullable_type" | "parenthesized_type" | "function_type"
                )
        })
    {
        // Receiver/suspend function types need their source parameter roles, not flattened ABI slots.
        return None;
    }
    let parameters = child_of_kind(lambda, "lambda_parameters");
    if let Some((binder, _)) = resolve::local_decl(usage, name, UseKind::Value, src) {
        if binder.start_byte() > lambda.start_byte()
            && binder.end_byte() < lambda.end_byte()
            && !parameters.is_some_and(|parameters| {
                binder.start_byte() >= parameters.start_byte()
                    && binder.end_byte() <= parameters.end_byte()
            })
        {
            return None;
        }
    }
    let position = if let Some(parameters) = parameters {
        let mut cursor = parameters.walk();
        let parameters: Vec<_> = parameters.named_children(&mut cursor).collect();
        if parameters.len() != formals.len()
            || parameters
                .iter()
                .any(|parameter| !matches!(parameter.kind(), "identifier" | "variable_declaration"))
        {
            return None;
        }
        parameters.iter().position(|parameter| {
            first_ident(*parameter).is_some_and(|identifier| node_text(identifier, src) == name)
        })?
    } else {
        (name == "it" && formals.len() == 1).then_some(0)?
    };
    let bounds =
        crate::indexer::type_parameters_in_scope(declaration, src, &ctx.package, &ctx.imports);
    Some(declared_type_in_scope(
        index,
        &formals[position],
        ctx,
        &bounds,
    ))
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
        recv_ty.is_member_receiver().then_some(recv_ty)
    } else if ELEMENT_LAMBDA_OPS.contains(&sel) {
        match recv_ty.args() {
            [elem] if elem.is_member_receiver() => Some(elem.clone()),
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
    lambda: Node,
    lambda_ancestors: &[Node],
    src: &str,
    ctx: &FileCtx,
    depth: usize,
    child_infer: &mut impl ChildInfer,
) -> Option<Type> {
    if let Some(receiver) = property_lambda_receiver_type(index, lambda, src, ctx) {
        return Some(receiver);
    }
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

fn property_lambda_receiver_type(
    index: &dyn InferenceIndex,
    lambda: Node,
    src: &str,
    ctx: &FileCtx,
) -> Option<Type> {
    let mut initializer = lambda;
    while let Some(parent) = initializer
        .parent()
        .filter(|parent| parent.kind() == "parenthesized_expression")
    {
        initializer = parent;
    }
    let property = initializer
        .parent()
        .filter(|parent| parent.kind() == "property_declaration")?;
    let declaration = child_of_kind(property, "variable_declaration")?;
    let function_type = (|| {
        let mut annotation = declaration;
        loop {
            let mut cursor = annotation.walk();
            annotation = annotation.named_children(&mut cursor).find(|child| {
                matches!(
                    child.kind(),
                    "function_type" | "parenthesized_type" | "nullable_type"
                )
            })?;
            if annotation.kind() == "function_type" {
                return Some(annotation);
            }
        }
    })();
    let reference = if let Some(function_type) = function_type {
        crate::indexer::receiver_type_of_function_type(function_type, src, None)?
    } else {
        let annotation = crate::indexer::value_type_of_scoped(declaration, src, None)?;
        type_alias_function_receiver_ref(index, &annotation, ctx)?
    };
    let bounds =
        crate::indexer::type_parameters_in_scope(declaration, src, &ctx.package, &ctx.imports);
    Some(declared_type_in_scope(index, &reference, ctx, &bounds))
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
            Some(child_infer.infer_child(index, recv, src, ctx, depth + 1))
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
                if entries.is_empty() && !local_selected {
                    continue;
                }
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
                false,
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
    if name == "null" {
        return intrinsic_type(index, "Nothing").with_nullability(true);
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
        return decl_type(index, decl, src, ctx, depth, child_infer);
    }
    if let Some((receiver, selection)) = members::select_implicit_at(
        index,
        implicit_receiver_types(index, ident, src, ctx, depth + 1),
        name,
        &MemberUse::Value,
        ctx,
        ident.start_byte(),
    ) {
        return selected_member_type(
            index,
            &receiver,
            name,
            false,
            selection,
            None,
            None,
            src,
            ctx,
            depth,
            child_infer,
        );
    }
    // 2. A bare identifier naming a known type -> that type (companion / enum-entry / static access).
    if !index.lookup_type(name).is_empty() {
        let ty = resolve_type_name(index, name, ctx, false);
        if ident
            .parent()
            .filter(|parent| parent.kind() == "navigation_expression")
            .and_then(|parent| parent.named_child(1))
            .is_some_and(|selector| {
                crate::jvm::compiler_field_type(index, &ty, node_text(selector, src)).is_some()
            })
        {
            return ty;
        }
        return crate::jvm::compiler_companion_type(index, &ty).unwrap_or(ty);
    }
    // 3. A visible cross-file top-level property of that name.
    if let Some(tr) = top_level_property_type(index, name, ctx) {
        return resolve_type_ref(index, &tr, ctx, depth);
    }
    Type::Unknown
}

/// The type a declaration's name node binds, including inferred initializer/function results.
pub fn decl_type(
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
    if parent.kind() == "function_declaration" {
        let parameters =
            crate::indexer::type_parameters_in_scope(parent, src, &ctx.package, &ctx.imports);
        let scope = crate::indexer::TypeScope::with_default_imports(
            &ctx.package,
            ctx.imports.clone(),
            crate::defaults::DEFAULT_IMPORT_PACKAGES,
        );
        let names = parameters
            .iter()
            .map(|p| p.name.clone())
            .collect::<Vec<_>>();
        let scope = scope.without_shadowed_imports(&names).unwrap_or(scope);
        let mut after_params = false;
        let mut cursor = parent.walk();
        for child in parent.named_children(&mut cursor) {
            if child.kind() == "function_value_parameters" {
                after_params = true;
            } else if after_params {
                match child.kind() {
                    "user_type" | "nullable_type" | "function_type" | "parenthesized_type" => {
                        return crate::indexer::type_ref_from(child, src, Some(&scope))
                            .map(|tr| declared_type_in_scope(index, &tr, ctx, &parameters))
                            .unwrap_or(Type::Unknown);
                    }
                    "function_body" => {
                        if parent.has_error() {
                            return Type::Unknown;
                        }
                        let Some(body) = child.named_child(0) else {
                            return Type::Unknown;
                        };
                        // Kotlin block bodies without an annotation return Unit, not their last value.
                        return if body.kind() == "block" {
                            Type::class("Unit", Some("kotlin".into()))
                        } else {
                            child_infer.infer_child(index, body, src, ctx, depth + 1)
                        };
                    }
                    _ => {}
                }
            }
        }
        return Type::Unknown;
    }
    if parent.kind() == "variable_declaration" {
        if let Some(t) = destructured_decl_type(index, decl, src, ctx, depth, child_infer) {
            return t;
        }
    }
    if matches!(
        parent.kind(),
        "variable_declaration" | "parameter" | "class_parameter"
    ) {
        let parameters =
            crate::indexer::type_parameters_in_scope(parent, src, &ctx.package, &ctx.imports);
        let scope = crate::indexer::TypeScope::with_default_imports(
            &ctx.package,
            ctx.imports.clone(),
            crate::defaults::DEFAULT_IMPORT_PACKAGES,
        );
        let names = parameters
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect::<Vec<_>>();
        let scope = scope.without_shadowed_imports(&names).unwrap_or(scope);
        if let Some(tr) = crate::indexer::value_type_of_scoped(parent, src, Some(&scope)) {
            return declared_type_in_scope(index, &tr, ctx, &parameters);
        }
    }
    // Initializers are siblings of their variable declarations.
    if parent.kind() == "variable_declaration" {
        if let Some(prop) = parent
            .parent()
            .filter(|prop| prop.kind() == "property_declaration")
        {
            if let Some(init) = initializer_expr(prop, parent) {
                return child_infer.infer_child(index, init, src, ctx, depth + 1);
            }
        }
    }
    Type::Unknown
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
    let Some(mut callee) = node.named_child(0) else {
        return Type::Unknown;
    };
    if has_trailing_lambda(node)
        && child_of_kind(node, "value_arguments").is_none()
        && callee.kind() == "call_expression"
        && child_of_kind(callee, "value_arguments").is_some()
    {
        let Some(inner) = callee.named_child(0).filter(|inner| inner.kind() != "call_expression") else {
            return Type::Unknown;
        };
        callee = inner;
    }
    match callee.kind() {
        "identifier" => {
            let cname = node_text(callee, src);
            // Locals/parameters are lexically stronger than every implicit receiver. Their
            // unmodeled callable signatures must not accidentally select a same-named extension.
            if let Some((binder, kind)) = resolve::local_decl(callee, cname, UseKind::Call, src) {
                if matches!(kind, SymbolKind::LocalVariable | SymbolKind::Parameter) {
                    let callable = decl_type(index, binder, src, ctx, depth + 1, child_infer);
                    let arguments = normalized_call_arguments(
                        node,
                        src,
                        synth_arg_types(index, node, src, ctx, depth, child_infer),
                    );
                    return function_value_call_type(
                        index,
                        &callable,
                        &arguments,
                        members::CallMode::Kotlin,
                    )
                    .unwrap_or(Type::Unknown);
                }
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
                if selection.entries.len() == 1
                    && selection.entries[0].sym.kind == SymbolKind::Property
                    && selection.applicability != members::Applicability::Rejected
                {
                    let callable = child_infer.infer_child(index, callee, src, ctx, depth + 1);
                    if let MemberUse::Call(arguments) = &usage {
                        if let Some(result) = function_value_call_type(
                            index,
                            &callable,
                            arguments,
                            members::CallMode::Kotlin,
                        ) {
                            return result;
                        }
                    }
                }
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
            if entries.is_empty() {
                let callable = child_infer.infer_child(index, callee, src, ctx, depth + 1);
                if let MemberUse::Call(arguments) = &usage {
                    if let Some(result) = function_value_call_type(
                        index,
                        &callable,
                        arguments,
                        members::CallMode::Kotlin,
                    ) {
                        return result;
                    }
                }
            }
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
            if node_text(sel, src) == "invoke" {
                let callable = child_infer.infer_child(index, recv, src, ctx, depth + 1);
                let arguments = normalized_call_arguments(
                    node,
                    src,
                    synth_arg_types(index, node, src, ctx, depth, child_infer),
                );
                if let Some(result) = function_value_call_type(
                    index,
                    &callable,
                    &arguments,
                    members::CallMode::Kotlin,
                ) {
                    return navigation_result_type(result, has_child_token(callee, "?."));
                }
            }
            let Some((recv_ty, candidates, nullable_result)) = navigation_member_candidates(
                index,
                recv,
                node_text(sel, src),
                src,
                ctx,
                depth,
                false,
                child_infer,
            ) else {
                return Type::Unknown;
            };
            let arg_types = synth_arg_types(index, node, src, ctx, depth, child_infer);
            let arguments = normalized_call_arguments(node, src, arg_types.clone());
            let selection =
                candidates.select(index, &recv_ty, &MemberUse::Call(arguments.clone()), ctx);
            if selection.entries.len() == 1
                && selection.entries[0].sym.kind == SymbolKind::Property
                && selection.applicability != members::Applicability::Rejected
            {
                let callable = child_infer.infer_child(index, callee, src, ctx, depth + 1);
                if let Some(result) = function_value_call_type(
                    index,
                    &callable,
                    &arguments,
                    members::CallMode::Kotlin,
                ) {
                    return navigation_result_type(result, nullable_result);
                }
            }
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

/// Invoke a normalized Kotlin function value using the same argument checks as ordinary members.
pub fn function_value_call_type(
    index: &dyn InferenceIndex,
    callable: &Type,
    arguments: &CallArguments,
    mode: members::CallMode,
) -> Option<Type> {
    let Type::Class {
        name,
        package: Some(package),
        container: None,
        nullable,
        args,
        owner: None,
    } = callable
    else {
        return None;
    };
    if !matches!(package.as_str(), "kotlin" | "kotlin.jvm.functions") {
        return None;
    }
    let arity = name.strip_prefix("Function")?.parse::<usize>().ok()?;
    if name != &format!("Function{arity}") || arity.checked_add(1)? != args.len() {
        return None;
    }
    if *nullable
        || arguments.count != Some(arity)
        || arguments.names.is_some()
        || arguments.has_spread
        || arguments.trailing_lambda
        || members::arguments_applicability(
            index,
            arguments,
            |position| args.get(position).cloned().unwrap_or(Type::Unknown),
            mode,
            false,
        ) != members::Applicability::Applicable
    {
        return Some(Type::Unknown);
    }
    Some(args[arity].clone())
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
            members::CallMode::Kotlin,
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
                members::bind_arguments(index, &Type::Unknown, entry, arguments, members::CallMode::Kotlin, ctx).ok()?;
            if (selection.applicability == members::Applicability::Unknown
                && entry
                    .sym
                    .params
                    .iter()
                    .any(|p| solve::has_projection(p) || !p.args.is_empty() || p.owner.is_some()))
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
                function_return_type_ref(index, entry)?,
                entry.sym.type_params.clone(),
                bound.params.into_owned(),
                entry.sym.type_parameter_facts,
                Some(bound.arguments.into_owned().types),
            ));
        }
    }
    let mut group = entries;
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
            opt_type_ref_same_shape(
                function_return_type_ref(index, e).as_ref(),
                function_return_type_ref(index, first).as_ref(),
            ) && e.sym.type_params == first.sym.type_params
                && e.sym.type_parameter_facts == first.sym.type_parameter_facts
                && type_ref_list_same_shape(&e.sym.params, &first.sym.params)
        })
    {
        return None;
    }
    Some((
        function_return_type_ref(index, first)?,
        first.sym.type_params.clone(),
        first.sym.params.clone(),
        first.sym.type_parameter_facts,
        None,
    ))
}

/// Read an inferred source return through the same current-declaration query as property types.
pub fn function_return_type_ref(index: &dyn InferenceIndex, entry: &Entry) -> Option<TypeRef> {
    entry
        .sym
        .return_type
        .clone()
        .or_else(|| crate::jvm::inferred_type_ref(&index.source_declaration_type(entry)?))
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
        && opt_type_ref_same_shape(a.owner.as_deref(), b.owner.as_deref())
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
        trailing_lambda_omits_or_has_zero_parameters: child_of_kind(call, "annotated_lambda")
            .and_then(|annotated| child_of_kind(annotated, "lambda_literal"))
            .or_else(|| child_of_kind(call, "lambda_literal"))
            .is_some_and(|lambda| {
                child_of_kind(lambda, "lambda_parameters")
                    .is_none_or(|parameters| parameters.named_child_count() == 0)
            }),
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
    let args = normalized_call_arguments(call, src, arg_types);
    let mut entries = entries;
    entries.retain(|entry| {
        members::function_accepts(index, recv_ty.unwrap_or(&Type::Unknown), entry, &args, ctx)
    });
    let mut unknown_role = entries.is_empty();
    let mut group = Vec::new();
    for entry in entries {
        if let Some(receiver) = trailing_lambda_receiver_ref(index, entry, ctx) {
            group.push((entry, receiver));
        } else if entry.sym.trailing_lambda_min_arity.is_none()
            || entry
                .sym
                .params
                .last()
                .and_then(members::function_type_signature)
                .is_none()
        {
            unknown_role = true;
        }
    }
    if unknown_role {
        return Some(Type::Unknown);
    }
    if std::env::var("KTDBG").is_ok() {
        eprintln!("DBG group: {}", group.len());
    }
    if group.is_empty() {
        return None;
    }
    let mut resolved = Vec::with_capacity(group.len());
    for (entry, receiver_ref) in group {
        let Some(receiver) = resolve_trailing_lambda_receiver_candidate(
            index,
            entry,
            &receiver_ref,
            recv_ty,
            &args,
            ctx,
            depth,
        ) else {
            return Some(Type::Unknown);
        };
        resolved.push(receiver);
    }
    let Some(first) = resolved.first().cloned() else {
        return Some(Type::Unknown);
    };
    Some(if resolved.iter().all(|receiver| *receiver == first) {
        first
    } else {
        Type::Unknown
    })
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
    let arguments = members::bind_arguments(index, recv_ty.unwrap_or(&Type::Unknown), entry, arguments, members::CallMode::Kotlin, ctx).ok()?;
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
            members::CallMode::Kotlin,
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
    type_alias_function_receiver_ref(index, parameter_type, ctx)
}

fn type_alias_function_receiver_ref(
    index: &dyn InferenceIndex,
    parameter_type: &TypeRef,
    ctx: &FileCtx,
) -> Option<TypeRef> {
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
    resolved.owner = match template.owner.as_deref() {
        Some(owner) => Some(Box::new(substitute_alias_type_ref(owner, bindings)?)),
        None => None,
    };
    resolved.args = template
        .args
        .iter()
        .map(|arg| substitute_alias_type_ref(arg, bindings))
        .collect::<Option<Vec<_>>>()?;
    Some(resolved)
}

// Normalized [] facts describe Java arrays, whose unqualified primitive components are keywords.
// Resolve only those components without nominal lookup; ordinary Kotlin classes named int still bind.
fn java_primitive_array_component(component: &TypeRef) -> Option<Type> {
    (component.qualification.is_none()
        && component.args.is_empty()
        && component.container_candidates.is_empty()
        && primitive_type_name(&component.name) != component.name)
        .then(|| Type::class(component.name.clone(), None))
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
    if tr.name.ends_with("[]") {
        let Some(component) = tr.array_component().filter(|_| depth < MAX_DEPTH) else {
            return Type::Unknown;
        };
        return Type::array(
            java_primitive_array_component(&component).unwrap_or_else(|| {
                resolve_with_subst(index, &component, ctx, tparams, subst, depth + 1)
            }),
            tr.nullable,
        );
    }
    if tr.qualification.is_none() {
        if let Some(bound) = subst.get(&tr.name) {
            if !matches!(bound, Type::Star)
                && !type_head_is_known(index, bound)
                && !unbounded_scoped_parameter(bound)
            {
                return Type::Unknown;
            }
            return bound
                .clone()
                .with_nullability(bound.is_nullable() || tr.nullable);
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
    let ty = Type::Class {
        name: tr.name.clone(),
        package,
        container,
        nullable: tr.nullable,
        args,

        owner: None,
    };
    let ty = match tr.owner.as_deref() {
        Some(owner) if depth < MAX_DEPTH => with_instantiated_owner(
            index,
            ty,
            resolve_with_subst(index, owner, ctx, tparams, subst, depth + 1),
        ),
        Some(_) => Type::Unknown,
        None => ty,
    };
    java_omitted_type(index, ty, tr.java_raw_eligible, depth)
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
    if let Some(receiver) = resolve::qualified_type_receiver(index, node, src, ctx) {
        return receiver;
    }
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
        true,
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
    want_value: bool,
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
    let candidates = if want_value {
        members::value_candidates(index, &ty, name, ctx, Some(receiver.end_byte()))
    } else {
        members::candidates_at(index, &ty, name, ctx, receiver.end_byte())
    };
    Some((ty, candidates, nullable_result))
}

fn is_safe_navigation_receiver(receiver: Node) -> bool {
    receiver
        .parent()
        .is_some_and(|nav| nav.kind() == "navigation_expression" && has_child_token(nav, "?."))
}

fn navigation_result_type(mut result: Type, nullable_result: bool) -> Type {
    if nullable_result {
        result = result.with_nullability(true);
    }
    result
}

/// A safe navigation tests the non-null receiver before selecting its member.
pub fn member_receiver_type(receiver: Node, mut ty: Type) -> Type {
    if is_safe_navigation_receiver(receiver) {
        ty = ty.into_non_null();
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
    if !recv_ty.is_member_receiver() {
        return Type::Unknown;
    }
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
                trailing_lambda_omits_or_has_zero_parameters: false,
            },
        })
    } else {
        MemberUse::Value
    };
    if let Some(result) = members::kotlin_java_array_member_type(recv_ty, name, &usage) {
        return result;
    }
    if !want_function {
        if let Some(ty) = crate::jvm::compiler_field_type(index, recv_ty, name) {
            return ty;
        }
    } else if let MemberUse::Call(arguments) = &usage {
        let jvm_candidates =
            crate::jvm::member_candidates_from(index, recv_ty, name, ctx, 0, &candidates.members);
        if jvm_candidates.iter().any(|candidate| {
            matches!(
                candidate,
                crate::jvm::CallableCandidate::Jvm {
                    owner: crate::jvm::JvmOwner::Compiled { .. },
                    ..
                }
            )
        }) {
            let selected =
                crate::jvm::select_call(index, recv_ty, jvm_candidates, name, arguments, ctx);
            if let [candidate] = selected.consumable_candidates() {
                return crate::jvm::projected_return_type_in_call_mode(
                    index, recv_ty, *candidate, members::CallMode::Kotlin, ctx,
                )
                .unwrap_or(Type::Unknown);
            }
            return Type::Unknown;
        }
    }
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
        let inferred;
        let tr = match member_type_ref(index, recv_ty, e, want_function) {
            Some(tr) => tr,
            None if (!want_function && e.sym.kind == SymbolKind::Property)
                || (want_function && e.sym.kind == SymbolKind::Function) =>
            {
                inferred = index
                    .source_declaration_type(e)
                    .as_ref()
                    .and_then(|ty| crate::jvm::inferred_member_type_ref(index, e, ty));
                let Some(tr) = inferred.as_ref() else {
                    return Type::Unknown;
                };
                tr
            }
            None => return Type::Unknown,
        };
        let scalar = primitive_type_name(&tr.name);
        let normalized;
        let tr = if scalar != tr.name
            && tr.qualification.is_none()
            && tr.projection == Projection::Invariant
            && tr.args.is_empty()
            && tr.package_candidates.is_empty()
            && tr.container_candidates.is_empty()
        {
            // Java keyword facts have no nominal lookup scope; Kotlin user types do.
            normalized = TypeRef {
                name: scalar.into(),
                qualification: Some("kotlin".into()),
                nullable: tr.nullable,
                ..TypeRef::default()
            };
            &normalized
        } else {
            tr
        };
        let t = if semantic_entry.is_some() {
            if let Some(arguments) = arguments.filter(|arguments| arguments.names.is_some()) {
                if members::bind_arguments(index, recv_ty, e, arguments, members::CallMode::Kotlin, ctx).is_err() {
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
        let t = crate::jvm::string_result_for_call_mode(index, t, ctx, members::CallMode::Kotlin);
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
            match members::bind_arguments(index, recv_ty, entry, arguments, members::CallMode::Kotlin, ctx) {
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
    if let Some(signature) = raw_member_signature(index, recv_ty, entry, tr, members::CallMode::Kotlin) {
        return match &bound {
            Some(bound) if raw_member_application(index, entry, bound, &signature).status
                != solve::ConstraintStatus::Matched => Type::Unknown,
            _ => signature.result,
        };
    }
    if projected
        || (entry.sym.extension_receiver.is_none()
            && !entry.sym.type_params.is_empty()
            && !entry.sym.type_parameter_facts.known_unbounded())
    {
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
    if tr.qualification.is_none() && tr.args.is_empty() && tr.owner.is_none()
        && !entry.sym.type_params.contains(&tr.name)
        && matches!(subst.get(&tr.name), Some(Type::Star))
    {
        return declared_member_type(index, recv_ty, entry, tr, members::CallMode::Kotlin, ctx);
    }
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
    mode: members::CallMode,
    ctx: &FileCtx,
) -> Type {
    if let Some(signature) = raw_member_signature(index, recv_ty, entry, tr, mode) {
        return signature.result;
    }
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
    if tr.qualification.is_none() && tr.args.is_empty() && tr.owner.is_none()
        && !entry.sym.type_params.contains(&tr.name)
        && matches!(subst.get(&tr.name), Some(Type::Star))
    {
        let mut read = None;
        let mut conflict = false;
        let _ = walk_supertypes(index, recv_ty, SUPERTYPE_CAP, |current| {
            if receiver_root_key(index, current).as_deref() != entry.sym.container.as_deref()
                || current.package() != Some(entry.sym.package.as_str()) { return ControlFlow::Continue(()); }
            let mut scope = Some(current);
            while let Some(ty) = scope {
                if let Some(declaration) = type_declaration_shape(index, ty) {
                    if let Some(parameters) = type_declaration_parameters(index, ty) {
                        if parameters.iter().any(|p| p.name == tr.name) {
                            let ctx = FileCtx::new(declaration.sym.package.clone(), Vec::new()).with_file(&declaration.path);
                            let candidate = declared_type_in_scope(index, tr, &ctx, parameters);
                            if read.as_ref().is_some_and(|previous| previous != &candidate) { conflict = true; return ControlFlow::Break(()); }
                            read = Some(candidate);
                            break;
                        }
                    }
                }
                scope = ty.owner();
            }
            ControlFlow::Continue(())
        });
        if conflict { return Type::Unknown; }
        if let Some(read) = read { return read; }
    }
    resolve_with_subst(index, tr, ctx, &tparams, &subst, 0)
}

// A complete unbounded Kotlin parameter has an implicit Any? bound, but remains an opaque
// source identity. Unresolved and recursive bounds retain Unknown and cannot enter the solver.
fn unbounded_scoped_parameter(ty: &Type) -> bool {
    matches!(ty, Type::Parameter { definition, upper_bounds, .. }
        if definition.file.ends_with(".kt") && matches!(upper_bounds.as_slice(),
            [Type::Class { name, package, container: None, nullable: true, args, owner: None }]
                if name == "Any" && package.as_deref() == Some("kotlin") && args.is_empty()))
}

// Closed type values retain declaration shape; call bounds are validated after substitution.
fn constraint_type_is_known(index: &dyn InferenceIndex, ty: &Type, depth: usize) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    if unbounded_scoped_parameter(ty) {
        return true;
    }
    if !type_head_is_known(index, ty) || !receiver_shape_is_known(index, ty) {
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
                .all(|entry| {
                    entry.sym.type_parameter_facts.known_unbounded()
                        || entry.sym.type_parameter_facts.known_invariant()
                })
        {
            return false;
        }
    }
    ty.owner()
        .is_none_or(|owner| constraint_type_is_known(index, owner, depth + 1))
        && ty
            .args()
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
        if let (Some(a), Some(b)) = (a.owner(), b.owner()) {
            if constraint_types_distinct(index, a, b, depth + 1) {
                return true;
            }
        }
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
    mode: members::CallMode,
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
        |actual| match actual {
            Type::Raw(nominal) => {
                matches!(nominal.as_ref(), Type::Class { args, .. } if args.is_empty())
                    && type_head_is_known(index, nominal)
                    && type_declaration_parameters(index, nominal).is_some()
            }
            _ => constraint_type_is_known(index, actual, 0),
        },
        |actual, formals| {
            if type_declaration_shape(index, actual).is_none() {
                return false;
            }
            let declarations = index.lookup_type(actual.name().unwrap_or_default());
            let mut invariant = true;
            let mut unbounded = true;
            for entry in declarations.iter().filter(|entry| {
                entry.sym.package == actual.package().unwrap_or_default()
                    && entry.sym.container.as_deref() == actual.container()
            }) {
                invariant &= entry.sym.type_parameter_facts.known_invariant();
                unbounded &= entry.sym.type_parameter_facts.known_unbounded();
            }
            if invariant {
                return true;
            }
            if !unbounded {
                return false;
            }
            // The JVM signature exposes declaration-site Kotlin variance as an ordinary class.
            if mode == members::CallMode::Java {
                return true;
            }
            // Plain equality slots still require an invariant constructor in this local solver.
            type_declaration_parameters(index, actual).is_some_and(|parameters| {
                parameters.len() == formals.len()
                    && parameters.iter().zip(formals).all(|(parameter, formal)| {
                        matches!(
                            (parameter.variance, formal.projection),
                            (Some(Projection::Out), Projection::Out)
                                | (Some(Projection::In), Projection::In)
                                | (
                                    Some(Projection::Invariant),
                                    Projection::Out | Projection::In
                                )
                        )
                    })
            })
        },
        |actual, bound| receiver_satisfies_upper_bound(index, actual, bound),
        |actual, other| constraint_types_distinct(index, actual, other, 0),
    )
}

/// A defaulted slot is not a synthesized argument. It must still have modeled declaration types
/// before removing that slot can turn an uncertain overload into a proven applicable one.
pub(crate) fn call_parameter_templates_known(index: &dyn InferenceIndex, entry: &Entry) -> bool {
    if !callable_bounds_known(entry) {
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
        if let Some((arguments, result)) = members::function_type_signature(formal) {
            return arguments.iter().chain(std::iter::once(result))
                .all(|argument| known(index, argument, ctx, parameters, depth + 1));
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

/// Project only through a complete hierarchy with one instantiated formal head.
fn argument_at_formal_head(
    index: &dyn InferenceIndex,
    actual: &Type,
    formal: &TypeRef,
    ctx: &FileCtx,
    parameters: &HashSet<String>,
    initial: &HashMap<String, Type>,
) -> Type {
    if formal.args.is_empty() || actual.is_nullable() && !formal.nullable {
        return actual.clone();
    }
    let resolved = resolve_with_subst_unprojected(index, formal, ctx, parameters, initial, 0);
    let same_head = |current: &Type| {
        current.name() == resolved.name()
            && current.package() == resolved.package()
            && current.container() == resolved.container()
    };
    if resolved.name().is_none() || same_head(actual) {
        return actual.clone();
    }
    let mut projected = None;
    let mut complete = true;
    let _ = walk_supertypes(index, actual, SUPERTYPE_CAP, |current| {
        complete &= receiver_arguments_known(index, current, 0);
        if same_head(current) {
            let current = current.clone().with_nullability(actual.is_nullable());
            if projected
                .as_ref()
                .is_some_and(|previous| previous != &current)
            {
                complete = false;
            }
            projected = Some(current);
        }
        ControlFlow::Continue(())
    });
    if complete {
        projected.unwrap_or_else(|| actual.clone())
    } else {
        actual.clone()
    }
}

/// Both call selection and return substitution consume the same bound formal/actual pairs.
pub(crate) fn projected_member_application(
    index: &dyn InferenceIndex,
    receiver: &Type,
    entry: &Entry,
    bound: &members::BoundArguments<'_>,
) -> Option<solve::Application> {
    if let Some(signature) = raw_member_signature(
        index,
        receiver,
        entry,
        entry.sym.return_type.as_ref().unwrap_or(&TypeRef::default()),
        bound.mode,
    ) {
        return Some(raw_member_application(index, entry, bound, &signature));
    }
    if entry.sym.type_params.is_empty()
        && !entry
            .sym
            .params
            .iter()
            .any(|ty| solve::has_projection(ty) || !ty.args.is_empty() || ty.owner.is_some())
    {
        return None;
    }
    let arguments = bound.arguments.as_ref();
    let ctx = FileCtx::new(entry.sym.package.clone(), Vec::new());
    let receiver_bindings = receiver_type_bindings(index, receiver, entry);
    let missing_receiver = entry.sym.extension_receiver.is_some() && receiver_bindings.is_none();
    let initial = receiver_bindings.unwrap_or_default();
    let parameters: HashSet<String> = entry.sym.type_params.iter().cloned().collect();
    let actuals: Vec<Type> = arguments
        .types
        .iter()
        .enumerate()
        .map(|(position, actual)| {
            bound.params.get(position).map_or_else(
                || actual.clone(),
                |formal| {
                    argument_at_formal_head(index, actual, formal, &ctx, &parameters, &initial)
                },
            )
        })
        .collect();
    let mut application = projected_application(
        index,
        &bound.params,
        &actuals,
        &entry.sym.type_params,
        callable_bounds_known(entry),
        bound.mode,
        &ctx,
        &initial,
    );
    if application.status == solve::ConstraintStatus::Matched {
        let declared_parameters = member_type_parameters(index, entry);
        for (parameter, bound) in entry
            .sym
            .method_type_bounds
            .as_deref()
            .into_iter()
            .flatten()
        {
            let scoped_relation = bound.qualification.is_none()
                && declared_parameters.contains(&bound.name)
                && bound.args.is_empty()
                && bound.owner.is_none();
            let bound = resolve_with_subst_unprojected(
                index,
                bound,
                &ctx,
                &declared_parameters,
                &application.bindings,
                0,
            );
            let actual = application.bindings.get(parameter);
            let satisfies =
                actual.is_some_and(|actual| receiver_satisfies_upper_bound(index, actual, &bound));
            if !satisfies {
                application.status = if scoped_relation
                    && actual
                        .is_some_and(|actual| nominal_heads_incompatible(index, actual, &bound))
                {
                    solve::ConstraintStatus::Rejected
                } else {
                    solve::ConstraintStatus::Unknown
                };
                break;
            }
        }
    }
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

fn callable_bounds_known(entry: &Entry) -> bool {
    entry.sym.type_parameter_facts.known_unbounded()
        || entry.sym.type_parameter_facts.known_invariant()
            && entry.sym.method_type_bounds.is_some()
}

/// A language adapter supplies already synthesized arguments, without using another grammar's AST.
pub fn declared_member_call_type(
    index: &dyn InferenceIndex,
    receiver: &Type,
    entry: &Entry,
    tr: &TypeRef,
    arguments: &CallArguments,
    mode: members::CallMode,
    ctx: &FileCtx,
) -> Type {
    let bound = match members::bind_arguments(index, receiver, entry, arguments, mode, ctx) {
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
    if let Some(signature) = raw_member_signature(index, receiver, entry, tr, bound.mode) {
        return if raw_member_application(index, entry, bound, &signature).status
            == solve::ConstraintStatus::Matched
        {
            signature.result
        } else {
            Type::Unknown
        };
    }
    if bound.params.len() != entry.sym.params.len() && !call_parameter_templates_known(index, entry)
    {
        return Type::Unknown;
    }
    if entry.sym.type_params.is_empty() && !entry.sym.params.iter().any(solve::has_projection) {
        return declared_member_type(index, receiver, entry, tr, bound.mode, ctx);
    }
    let Some(application) = projected_member_application(index, receiver, entry, bound) else {
        return declared_member_type(index, receiver, entry, tr, bound.mode, ctx);
    };
    if application.status == solve::ConstraintStatus::Unknown
        && member_type_parameters(index, entry).is_empty()
        && entry.sym.extension_receiver.is_none()
        && bound.arguments.count == Some(bound.arguments.types.len())
        && !bound.arguments.has_spread
        && !bound.arguments.trailing_lambda
        && !entry.sym.has_vararg
        && entry.sym.arity == Some(entry.sym.params.len().min(u8::MAX as usize) as u8)
        && entry.sym.min_arity.is_some()
        && tr.args.is_empty()
        && args_consistent(
            index,
            &bound.params,
            &entry.sym.type_params,
            &bound.arguments.types,
            &FileCtx::new(entry.sym.package.clone(), Vec::new()),
            0,
        )
    {
        // An uncertain parameter binding does not change a closed, fixed return type.
        let declared = declared_member_type(index, receiver, entry, tr, bound.mode, ctx);
        if constraint_type_is_known(index, &declared, 0) {
            return declared;
        }
    }
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
        params.extend(lexical_type_parameter_names(
            index,
            container,
            &entry.sym.package,
        ));
    }
    params
}

fn lexical_type_parameter_names(
    index: &dyn InferenceIndex,
    container: &str,
    package: &str,
) -> HashSet<String> {
    let mut params = HashSet::new();
    let mut current = Some(container);
    for _ in 0..SUPERTYPE_CAP {
        let Some(identity) = current else {
            break;
        };
        let (parent, name) = identity
            .rsplit_once('.')
            .map_or((None, identity), |(parent, name)| (Some(parent), name));
        let ty = Type::Class {
            name: name.to_string(),
            package: Some(package.to_string()),
            container: parent.map(str::to_string),
            nullable: false,
            args: Vec::new(),
            owner: None,
        };
        let Some(declaration) = type_declaration_shape(index, &ty) else {
            break;
        };
        params.extend(declaration.sym.type_params.iter().cloned());
        current = if declaration.sym.source_member.dispatch()
            == Some(crate::extensions::Dispatch::Instance)
        {
            parent
        } else {
            None
        };
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
    entry: &Entry,
    args: &[Type],
    ctx: &FileCtx,
) -> bool {
    let platform = java_platform_null_arguments(entry, &entry.sym.params, args);
    args_consistent(
        index,
        &entry.sym.params,
        &entry.sym.type_params,
        platform.as_deref().unwrap_or(args),
        ctx,
        0,
    )
}

pub(crate) fn java_platform_null_arguments(
    entry: &Entry,
    params: &[TypeRef],
    args: &[Type],
) -> Option<Vec<Type>> {
    if !entry.path.ends_with(".java") || entry.sym.kind != SymbolKind::Function {
        return None;
    }
    unproven_reference_null_arguments(params, args, entry.sym.has_vararg)
}

pub(crate) fn unproven_reference_null_arguments(
    params: &[TypeRef],
    args: &[Type],
    has_vararg: bool,
) -> Option<Vec<Type>> {
    let mut platform = None;
    for (position, actual) in args.iter().enumerate() {
        let Some(formal) = params
            .get(position)
            .or_else(|| has_vararg.then(|| params.last()).flatten())
        else {
            continue;
        };
        if is_kotlin_null_type(actual)
            && !formal.name.is_empty()
            && !matches!(
                formal.name.as_str(),
                "boolean"
                    | "byte"
                    | "char"
                    | "short"
                    | "int"
                    | "long"
                    | "float"
                    | "double"
                    | "void"
            )
        {
            // shortcut: Java parameter nullability annotations need normalized facts before proof.
            platform.get_or_insert_with(|| args.to_vec())[position] = Type::Unknown;
        }
    }
    platform
}

fn is_kotlin_null_type(ty: &Type) -> bool {
    matches!(ty, Type::Class {
        name, package: Some(package), container: None, nullable: true, args, owner: None,
    } if name == "Nothing" && package == "kotlin" && args.is_empty())
}

/// Gradual consistency of an actual argument type with a formal parameter type: an `Unknown` on either
/// side or an unindexed symbolic head stays possible; an exact head match or a known subtype is
/// consistent; otherwise inconsistent.
pub(crate) fn types_consistent(index: &dyn InferenceIndex, actual: &Type, formal: &Type) -> bool {
    if is_kotlin_null_type(actual) {
        return match formal {
            Type::Class { .. } | Type::Array { .. } | Type::Parameter { .. } => {
                formal.is_nullable()
            }
            _ => true,
        };
    }
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
    if is_reference_root(formal)
        && matches!(actual, Type::Class { .. } | Type::Raw(_))
        && (!actual.is_nullable() || formal.is_nullable())
        && !(formal.package() == Some("java.lang")
            && actual.container().is_none()
            && actual
                .package()
                .is_none_or(|package| matches!(package, "kotlin" | "java.lang"))
            && matches!(
                primitive_type_name(an),
                "Int" | "Long" | "Short" | "Byte" | "Float" | "Double" | "Boolean" | "Char"
            ))
    {
        return true;
    }
    is_subtype(index, an, actual.package(), fname)
}

pub(crate) fn is_reference_root(ty: &Type) -> bool {
    ty.container().is_none()
        && ty.owner().is_none()
        && ty.args().is_empty()
        && matches!(
            (ty.package(), ty.name()),
            (Some("kotlin"), Some("Any")) | (Some("java.lang"), Some("Object"))
        )
}

/// A closed nominal hierarchy can exclude a head without solving its type arguments.
pub(crate) fn nominal_heads_incompatible(
    index: &dyn InferenceIndex,
    actual: &Type,
    formal: &Type,
) -> bool {
    if !matches!((actual, formal), (Type::Class { .. }, Type::Class { .. })) {
        return false;
    }
    if is_reference_root(formal) || actual.name() == formal.name() {
        return false;
    }
    let modeled = |ty: &Type| {
        type_declaration(index, ty).is_some_and(|declaration| {
            declaration.sym.kind != SymbolKind::TypeAlias
                && declaration.sym.type_parameter_facts.known_unbounded()
                && declaration.sym.type_parameter_facts.known_invariant()
        })
    };
    if !modeled(formal) {
        return false;
    }
    if !modeled(actual) {
        return false;
    }
    let mut uncertain = false;
    let mut found = false;
    let walked = walk_supertypes(index, actual, SUPERTYPE_CAP, |current| {
        uncertain |= !modeled(current);
        if current.name() == formal.name() {
            if current.package() == formal.package() && current.container() == formal.container() {
                found = true;
            } else {
                // Equal names in distinct packages may be a JVM mapped identity.
                uncertain = true;
            }
        }
        let kotlin_java = |left: Option<&str>, right: Option<&str>| {
            left.is_some_and(|package| package == "kotlin" || package.starts_with("kotlin."))
                && right.is_some_and(|package| package.starts_with("java."))
        };
        uncertain |= kotlin_java(current.package(), formal.package())
            || kotlin_java(formal.package(), current.package());
        std::ops::ControlFlow::Continue(())
    });
    walked.is_continue() && !uncertain && !found
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

// Only the Java adapter can certify omitted type arguments. A missing Kotlin argument list
// and an explicit Java diamond never acquire raw semantics from their nominal shape.
fn java_omitted_type(
    index: &dyn InferenceIndex,
    mut ty: Type,
    eligible: bool,
    depth: usize,
) -> Type {
    if !eligible || depth > MAX_DEPTH || !matches!(ty, Type::Class { .. }) {
        return ty;
    }
    let Some(declaration) = type_declaration_shape(index, &ty) else {
        return ty;
    };
    if !ty.args().is_empty() {
        return ty;
    }
    if declaration.sym.source_member.dispatch() == Some(crate::extensions::Dispatch::Instance)
        && ty.owner().is_none()
    {
        if let Some(container) = ty.container() {
            let (parent, name) = container
                .rsplit_once('.')
                .map_or((None, container), |(p, n)| (Some(p.to_string()), n));
            let owner = java_omitted_type(
                index,
                Type::Class {
                    name: name.to_string(),
                    package: ty.package().map(str::to_string),
                    container: parent,
                    nullable: false,
                    args: Vec::new(),
                    owner: None,
                },
                true,
                depth + 1,
            );
            if matches!(owner, Type::Raw(_)) {
                ty = with_instantiated_owner(index, ty, owner);
            }
        }
    }
    let raw_owner = ty
        .owner()
        .is_some_and(|owner| matches!(owner, Type::Raw(_)));
    if matches!(ty, Type::Raw(_)) {
        return if type_declaration_parameters(index, &ty).is_some() {
            ty
        } else {
            Type::Unknown
        };
    }
    if !declaration.sym.type_params.is_empty() || raw_owner {
        if type_declaration_parameters(index, &ty).is_none() {
            return Type::Unknown;
        }
        Type::Raw(Box::new(ty))
    } else {
        ty
    }
}

struct ErasureScope<'a> {
    parameters: &'a [TypeParameterBounds],
    ctx: FileCtx,
    mode: members::CallMode,
}

// Method variables shadow class variables; an inherited bound is resolved from its own
// declaration's layer, so a method cannot change the meaning of a class variable's bound.
fn raw_erasure_scopes<'a>(
    index: &'a dyn InferenceIndex,
    receiver: &Type,
    method: Option<&'a Entry>,
    mode: members::CallMode,
) -> Option<Vec<ErasureScope<'a>>> {
    let mut scopes = Vec::new();
    if let Some(method) = method {
        if !method.sym.type_params.is_empty() {
            let parameters = method.sym.type_parameter_bounds.as_deref()?;
            if parameters.len() != method.sym.type_params.len()
                || !parameters
                    .iter()
                    .zip(&method.sym.type_params)
                    .all(|(p, n)| {
                        p.name == *n
                            && p.complete
                            && p.variance == Some(Projection::Invariant)
                            && p.erasure.is_some()
                    })
            {
                return None;
            }
            scopes.push(ErasureScope {
                parameters,
                mode,
                ctx: FileCtx::new(method.sym.package.clone(), Vec::new()).with_file(&method.path),
            });
        }
    }
    let mut current = Some(receiver);
    let mut depth = 0;
    while let Some(ty) = current {
        if depth > MAX_DEPTH {
            return None;
        }
        let parameters = type_declaration_parameters(index, ty)?;
        if parameters
            .iter()
            .any(|parameter| parameter.erasure.is_none())
        {
            return None;
        }
        let declaration = type_declaration_shape(index, ty)?;
        scopes.push(ErasureScope {
            parameters,
            mode,
            ctx: FileCtx::new(ty.package().unwrap_or_default().to_string(), Vec::new())
                .with_file(&declaration.path),
        });
        current = ty.owner();
        depth += 1;
    }
    Some(scopes)
}

fn erase_parameter_bound(
    index: &dyn InferenceIndex,
    parameter: &TypeParameterBounds,
    scopes: &[ErasureScope<'_>],
    layer: usize,
    path: &mut Vec<(usize, usize)>,
    depth: usize,
) -> Type {
    use crate::types::ParameterErasure;
    let Some(policy) = parameter.erasure else {
        return Type::Unknown;
    };
    if parameter.upper_bounds.is_empty() {
        if policy == ParameterErasure::ClassFirst && scopes[layer].mode == members::CallMode::Kotlin
        {
            return Type::class("Any", Some("kotlin".into())).with_nullability(true);
        }
        return Type::class("Object", Some("java.lang".into()));
    }
    if policy == ParameterErasure::FirstBound {
        return erase_type_ref(
            index,
            &parameter.upper_bounds[0],
            scopes,
            layer,
            path,
            depth + 1,
        );
    }
    // An intersection involving another type variable has no supported JVM class-order proof.
    if parameter.upper_bounds.len() > 1
        && parameter.upper_bounds.iter().any(|bound| {
            bound.qualification.is_none()
                && scopes
                    .iter()
                    .skip(layer)
                    .any(|scope| scope.parameters.iter().any(|p| p.name == bound.name))
        })
    {
        return Type::Unknown;
    }
    let mut first_interface = None;
    let mut first_metadata_bound = None;
    let mut class = None;
    for bound in &parameter.upper_bounds {
        let bound = erase_type_ref(index, bound, scopes, layer, path, depth + 1);
        if first_metadata_bound.is_none() {
            first_metadata_bound = Some(bound.clone());
        }
        if is_reference_root(&bound) {
            if class.is_some() {
                return Type::Unknown;
            }
            class = Some(Type::class("Object", Some("java.lang".into())));
            continue;
        }
        let Some(declaration) = type_declaration_shape(index, &bound) else {
            return Type::Unknown;
        };
        match declaration.sym.kind {
            SymbolKind::Interface => {
                first_interface.get_or_insert(bound);
            }
            SymbolKind::Class | SymbolKind::EnumClass | SymbolKind::Object => {
                if class.is_some() {
                    return Type::Unknown;
                }
                class = Some(bound);
            }
            _ => return Type::Unknown,
        }
    }
    if scopes[layer].mode == members::CallMode::Kotlin {
        first_metadata_bound.unwrap_or(Type::Unknown)
    } else {
        class.or(first_interface).unwrap_or(Type::Unknown)
    }
}

fn erase_type_ref(
    index: &dyn InferenceIndex,
    tr: &TypeRef,
    scopes: &[ErasureScope<'_>],
    from: usize,
    path: &mut Vec<(usize, usize)>,
    depth: usize,
) -> Type {
    if depth > MAX_DEPTH
        || from >= scopes.len()
        || tr.name.is_empty()
        || tr.projection != Projection::Invariant
    {
        return Type::Unknown;
    }
    if tr.name.ends_with("[]") {
        return tr.array_component().map_or(Type::Unknown, |component| {
            Type::array(
                java_primitive_array_component(&component).unwrap_or_else(|| {
                    erase_type_ref(index, &component, scopes, from, path, depth + 1)
                }),
                tr.nullable,
            )
        });
    }
    if tr.qualification.is_none() && tr.args.is_empty() && tr.owner.is_none() {
        for (layer, scope) in scopes.iter().enumerate().skip(from) {
            if let Some((position, parameter)) = scope
                .parameters
                .iter()
                .enumerate()
                .find(|(_, p)| p.name == tr.name)
            {
                if !parameter.complete || path.contains(&(layer, position)) {
                    return Type::Unknown;
                }
                path.push((layer, position));
                let result =
                    erase_parameter_bound(index, parameter, scopes, layer, path, depth + 1);
                path.pop();
                let nullable = tr.nullable
                    || (scopes[layer].mode == members::CallMode::Kotlin
                        && parameter.erasure == Some(crate::types::ParameterErasure::ClassFirst)
                        && result.is_nullable());
                return result.with_nullability(nullable);
            }
        }
    }
    let mut erased = tr.clone();
    erased.args.clear();
    erased.owner = None;
    erased.java_raw_eligible = true;
    let mut result = resolve_type_ref(index, &erased, &scopes[from].ctx, depth + 1);
    if let Some(owner) = tr.owner.as_deref() {
        let owner = erase_type_ref(index, owner, scopes, from, path, depth + 1);
        let nominal = match result {
            Type::Raw(inner) => *inner,
            other => other,
        };
        result = with_instantiated_owner(index, nominal, owner);
        result = java_omitted_type(index, result, true, depth + 1);
    }
    result
}

struct RawMemberSignature {
    params: Vec<Type>,
    result: Type,
}

fn raw_member_signature(
    index: &dyn InferenceIndex,
    receiver: &Type,
    entry: &Entry,
    result: &TypeRef,
    mode: members::CallMode,
) -> Option<RawMemberSignature> {
    if entry.sym.source_member.dispatch() != Some(crate::extensions::Dispatch::Instance)
        || entry.sym.extension_receiver.is_some()
    {
        return None;
    }
    let container = entry.sym.container.as_deref()?;
    let mut declaring = None;
    let mut conflict = false;
    let mut raw_path = false;
    let _ = walk_supertypes(index, receiver, SUPERTYPE_CAP, |current| {
        if receiver_root_key(index, current).as_deref() == Some(container)
            && current.package() == Some(entry.sym.package.as_str())
        {
            raw_path |= matches!(current, Type::Raw(_));
            if declaring
                .as_ref()
                .is_some_and(|previous| previous != current)
            {
                conflict = true;
            }
            declaring = Some(current.clone());
        }
        ControlFlow::Continue(())
    });
    let declaring = declaring?;
    if !raw_path {
        return None;
    }
    let scopes = (!conflict)
        .then(|| raw_erasure_scopes(index, &declaring, Some(entry), mode))
        .flatten();
    Some(match scopes {
        Some(scopes) => RawMemberSignature {
            params: entry
                .sym
                .params
                .iter()
                .map(|tr| erase_type_ref(index, tr, &scopes, 0, &mut Vec::new(), 0))
                .collect(),
            result: erase_type_ref(index, result, &scopes, 0, &mut Vec::new(), 0),
        },
        None => RawMemberSignature {
            params: vec![Type::Unknown; entry.sym.params.len()],
            result: Type::Unknown,
        },
    })
}

fn raw_member_application(
    index: &dyn InferenceIndex,
    entry: &Entry,
    bound: &members::BoundArguments<'_>,
    signature: &RawMemberSignature,
) -> solve::Application {
    let uncertain = bound.arguments.count != Some(bound.arguments.types.len())
        || bound.arguments.trailing_lambda
        || bound.arguments.has_spread
        || entry.sym.has_vararg
        || signature.params.len() != bound.arguments.types.len();
    let applicability = members::arguments_applicability(
        index,
        bound.arguments.as_ref(),
        |position| {
            signature
                .params
                .get(position)
                .cloned()
                .unwrap_or(Type::Unknown)
        },
        bound.mode,
        uncertain,
    );
    let status = match applicability {
        members::Applicability::Applicable => solve::ConstraintStatus::Matched,
        members::Applicability::Rejected => solve::ConstraintStatus::Rejected,
        members::Applicability::Unknown => solve::ConstraintStatus::Unknown,
    };
    solve::Application {
        status,
        bindings: HashMap::new(),
    }
}

/// A qualified name alone never proves capture of an outer instance or its arguments.
fn with_instantiated_owner(index: &dyn InferenceIndex, mut ty: Type, owner: Type) -> Type {
    let Some(declaration) = type_declaration_shape(index, &ty) else {
        return Type::Unknown;
    };
    if !unique_type_definition(index, &owner)
        || !unique_type_definition(index, &ty)
        || !type_head_is_known(index, &owner)
        || !receiver_shape_is_known(index, &owner)
        || owner.is_nullable()
        || declaration.sym.source_member.dispatch() != Some(crate::extensions::Dispatch::Instance)
        || owner.package() != ty.package()
        || receiver_root_key(index, &owner).as_deref() != ty.container()
    {
        return Type::Unknown;
    }
    if let Type::Class { owner: slot, .. } = &mut ty {
        *slot = Some(Box::new(owner));
    } else {
        return Type::Unknown;
    }
    if ty.owner().is_some_and(|owner| matches!(owner, Type::Raw(_))) {
        if !ty.args().is_empty() { return Type::Unknown; }
        Type::Raw(Box::new(ty))
    } else {
        ty
    }
}

fn unique_type_definition(index: &dyn InferenceIndex, ty: &Type) -> bool {
    let entries = index.lookup_type(ty.name().unwrap_or_default());
    let mut definition = None;
    for entry in entries.iter().filter(|entry| {
        ty.package() == Some(entry.sym.package.as_str())
            && ty.container() == entry.sym.container.as_deref()
    }) {
        let current = (
            entry.path.as_ref(),
            entry.sym.start_byte,
            entry.sym.end_byte,
        );
        if definition.is_some_and(|previous| previous != current) {
            return false;
        }
        definition = Some(current);
    }
    definition.is_some()
}

fn lexical_receiver_bindings(
    index: &dyn InferenceIndex,
    ty: &Type,
    depth: usize,
) -> Option<HashMap<String, Type>> {
    if depth > SUPERTYPE_CAP {
        return None;
    }
    let declaration = type_declaration_shape(index, ty)?;
    if declaration.sym.type_params.len() != ty.args().len() {
        return None;
    }
    let mut bindings = match ty.owner() {
        Some(owner) => lexical_receiver_bindings(index, owner, depth + 1)?,
        None => HashMap::new(),
    };
    // Nearer class parameters shadow lexical owners; methods shadow these below.
    bindings.extend(
        declaration
            .sym
            .type_params
            .iter()
            .cloned()
            .zip(ty.args().iter().cloned()),
    );
    Some(bindings)
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
                let Some(mut bindings) = lexical_receiver_bindings(index, current, 0) else {
                    return ControlFlow::Break(());
                };
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
            .owner
            .as_deref()
            .is_some_and(|owner| receiver_bound_mentions_parameter(owner, params, depth + 1))
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
        return actual.args() == bound.args() && actual.owner() == bound.owner();
    }
    let mut found = false;
    let mut incomplete = false;
    let walked = walk_supertypes(index, actual, SUPERTYPE_CAP, |current| {
        incomplete |= !receiver_arguments_known(index, current, 0);
        if same_head(current) {
            if current.args() != bound.args() || current.owner() != bound.owner() {
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
    let owners_match = match (formal.owner.as_deref(), actual.owner()) {
        (None, None) => true,
        (Some(formal), Some(actual)) => {
            bind_receiver_template(index, formal, actual, ctx, params, bindings, depth + 1)
        }
        _ => false,
    };
    owners_match
        && formal
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
            .owner()
            .is_none_or(|owner| receiver_arguments_known(index, owner, depth + 1))
        && actual
            .args()
            .iter()
            .all(|argument| receiver_arguments_known(index, argument, depth + 1))
}

// Callers already prove the nominal head; this checks its declaration shape and argument count.
fn receiver_shape_is_known(index: &dyn InferenceIndex, actual: &Type) -> bool {
    if let Type::Raw(nominal) = actual {
        return matches!(nominal.as_ref(), Type::Class { args, .. } if args.is_empty())
            && type_declaration_parameters(index, nominal).is_some()
            && nominal.owner().is_none_or(|owner| receiver_shape_is_known(index, owner));
    }
    if let Some(declaration) = type_declaration_shape(index, actual) {
        return declaration.sym.type_params.len() == actual.args().len()
            && actual
                .owner()
                .is_none_or(|owner| receiver_shape_is_known(index, owner));
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
                || !crate::symbol::type_parameter_bounds_agree(
                    previous
                        .sym
                        .type_parameter_bounds
                        .as_deref()
                        .map(Vec::as_slice),
                    entry
                        .sym
                        .type_parameter_bounds
                        .as_deref()
                        .map(Vec::as_slice),
                )
                || previous.sym.source_member.dispatch() != entry.sym.source_member.dispatch()
            {
                return None;
            }
        }
        result = Some(entry);
    }
    result
}

pub(crate) fn type_declaration_parameters<'a>(
    index: &'a dyn InferenceIndex,
    ty: &Type,
) -> Option<&'a [crate::types::TypeParameterBounds]> {
    let declaration = type_declaration_shape(index, ty)?;
    if declaration.sym.type_params.is_empty()
        && declaration.sym.type_parameter_facts.known_invariant()
    {
        return Some(&[]);
    }
    let parameters = declaration.sym.type_parameter_bounds.as_deref()?;
    (parameters.len() == declaration.sym.type_params.len()
        && parameters
            .iter()
            .zip(&declaration.sym.type_params)
            .all(|(parameter, name)| {
                parameter.name == *name
                    && parameter.complete
                    && matches!(
                        parameter.variance,
                        Some(
                            crate::types::Projection::Invariant
                                | crate::types::Projection::Out
                                | crate::types::Projection::In
                        )
                    )
            }))
    .then_some(parameters.as_slice())
}

/// Parent traversal additionally requires all declarations of the nominal identity to agree on
/// their instantiated hierarchy; shape agreement alone cannot prove a subtype relationship.
fn supertype_refs_agree(
    index: &dyn InferenceIndex,
    left: &[TypeRef],
    right: &[TypeRef],
    package: &str,
) -> bool {
    fn nongeneric_view(index: &dyn InferenceIndex, ty: &Type, depth: usize) -> bool {
        if depth > SUPERTYPE_CAP || !matches!(ty, Type::Class { .. }) || !ty.args().is_empty() {
            return false;
        }
        let Some(declaration) = type_declaration_shape(index, ty) else {
            return false;
        };
        if declaration.sym.kind == SymbolKind::TypeAlias {
            return false;
        }
        if !index
            .lookup_type(ty.name().unwrap_or_default())
            .iter()
            .filter(|entry| {
                entry.sym.package == declaration.sym.package
                    && entry.sym.container == declaration.sym.container
            })
            .all(|entry| {
                entry.sym.type_params.is_empty() && entry.sym.type_parameter_facts.known_invariant()
            })
        {
            return false;
        }
        if let Some(owner) = ty.owner() {
            return matches!(owner, Type::Class { .. })
                && receiver_arguments_known(index, owner, depth + 1);
        }
        let Some(container) = ty.container() else {
            return true;
        };
        match declaration.sym.source_member.dispatch() {
            Some(crate::extensions::Dispatch::Static) => true,
            Some(crate::extensions::Dispatch::Instance) => {
                let (outer, name) = container
                    .rsplit_once('.')
                    .map_or((None, container), |(outer, name)| {
                        (Some(outer.to_string()), name)
                    });
                nongeneric_view(
                    index,
                    &Type::Class {
                        name: name.to_string(),
                        package: ty.package().map(str::to_string),
                        container: outer,
                        nullable: false,
                        args: Vec::new(),
                        owner: None,
                    },
                    depth + 1,
                )
            }
            None => false,
        }
    }
    fn same(
        index: &dyn InferenceIndex,
        a: &TypeRef,
        b: &TypeRef,
        ctx: &FileCtx,
        depth: usize,
    ) -> bool {
        if a == b {
            return true;
        }
        if depth > MAX_DEPTH || a.args.len() != b.args.len() {
            return false;
        }
        let head = |ty: &TypeRef| {
            let mut head = ty.clone();
            head.args.clear();
            head.owner = None;
            head.java_raw_eligible = false;
            head
        };
        if head(a) != head(b) {
            return false;
        }
        if a.java_raw_eligible != b.java_raw_eligible {
            let a_type = resolve_type_ref(index, a, ctx, depth);
            let b_type = resolve_type_ref(index, b, ctx, depth);
            // Omission provenance is immaterial only when both views retain a proven nongeneric
            // identity. A nongeneric inner class may still acquire an implicit raw outer owner.
            if a_type != b_type || !nongeneric_view(index, &a_type, depth) {
                return false;
            }
        }
        a.args
            .iter()
            .zip(&b.args)
            .all(|(a, b)| same(index, a, b, ctx, depth + 1))
            && match (&a.owner, &b.owner) {
                (Some(a), Some(b)) => same(index, a, b, ctx, depth + 1),
                (None, None) => true,
                _ => false,
            }
    }
    left == right
        || left.len() == right.len() && {
            let ctx = FileCtx::new(package.to_string(), Vec::new());
            left.iter()
                .zip(right)
                .all(|(a, b)| same(index, a, b, &ctx, 0))
        }
}

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
                || !crate::symbol::type_parameter_bounds_agree(
                    previous
                        .sym
                        .type_parameter_bounds
                        .as_deref()
                        .map(Vec::as_slice),
                    entry
                        .sym
                        .type_parameter_bounds
                        .as_deref()
                        .map(Vec::as_slice),
                )
                || !supertype_refs_agree(
                    index,
                    &previous.sym.supertype_refs,
                    &entry.sym.supertype_refs,
                    &entry.sym.package,
                )
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
    let mut frontier = vec![(receiver.clone(), Vec::new(), 0)];
    while let Some((current, mut path, depth)) = frontier.pop() {
        if depth > max_depth {
            if visited.insert(Type::Unknown) {
                visit(&Type::Unknown)?;
            }
            continue;
        }
        if let Type::Parameter { upper_bounds, .. } = &current {
            if visited.insert(current.clone()) {
                for bound in upper_bounds {
                    frontier.push((bound.clone(), path.clone(), depth + 1));
                }
            }
            continue;
        }
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
        if path.contains(&identity) {
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
            frontier.push((supertype, path.clone(), depth + 1));
        }
    }
    ControlFlow::Continue(())
}

/// Instantiate a type's direct superclasses/interfaces using its actual type arguments.
pub fn direct_supertypes(index: &dyn InferenceIndex, ty: &Type) -> Vec<Type> {
    if matches!(ty, Type::Raw(_)) {
        let Some(declaration) = type_declaration(index, ty) else {
            return vec![Type::Unknown];
        };
        let Some(scopes) = raw_erasure_scopes(index, ty, None, members::CallMode::Java) else {
            return vec![Type::Unknown];
        };
        return declaration.sym.supertype_refs.iter().map(|tr| {
            erase_type_ref(index, tr, &scopes, 0, &mut Vec::new(), 0)
        }).collect();
    }
    if let Type::Parameter { upper_bounds, .. } = ty {
        return upper_bounds.clone();
    }
    if let Type::Class {
        name,
        package: Some(package),
        container: None,
        args,
        owner: None,
        ..
    } = ty
    {
        if package == "kotlin" && type_declaration(index, ty).is_none() {
            if let Some(arity) = name
                .strip_prefix("Function")
                .and_then(|arity| arity.parse::<usize>().ok())
            {
                if arity <= 22 && name == &format!("Function{arity}") && args.len() == arity + 1 {
                    let mut runtime = ty.clone();
                    if let Type::Class { package, .. } = &mut runtime {
                        *package = Some("kotlin.jvm.functions".into());
                    }
                    if type_declaration(index, &runtime).is_some_and(|declaration| {
                        declaration.sym.kind == SymbolKind::Interface
                            && declaration.sym.type_params.len() == args.len()
                    }) {
                        return vec![runtime];
                    }
                }
            }
        }
    }
    if ty.package().is_none()
        && ty.container().is_none()
        && ty
            .name()
            .is_some_and(|name| primitive_type_name(name) != name)
    {
        // A primitive has no nominal superclass, even when a Kotlin homonym is indexed.
        return Vec::new();
    }
    let Some(declaration) = type_declaration(index, ty) else {
        return vec![Type::Unknown];
    };
    let subst = lexical_receiver_bindings(index, ty, 0).unwrap_or_default();
    let params = lexical_type_parameter_names(
        index,
        receiver_root_key(index, ty).as_deref().unwrap_or_default(),
        &declaration.sym.package,
    );
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
    if let Some(owner) = tr.owner.as_deref() {
        collect_unbound_return_type_params(owner, tparams, subst, out);
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

fn infer_lambda(
    index: &dyn InferenceIndex,
    lambda: Node,
    src: &str,
    ctx: &FileCtx,
    depth: usize,
    child_infer: &mut impl ChildInfer,
) -> Option<Type> {
    if lambda.has_error() {
        return None;
    }
    let parameters = child_of_kind(lambda, "lambda_parameters");
    if parameters.is_none() {
        let mut expression = lambda;
        while let Some(parent) = expression
            .parent()
            .filter(|parent| parent.kind() == "parenthesized_expression")
        {
            expression = parent;
        }
        let initializer = expression
            .parent()
            .filter(|parent| parent.kind() == "property_declaration")?;
        let declaration = child_of_kind(initializer, "variable_declaration")?;
        // shortcut: contextual arity stays Unknown until expected-type parameter/receiver binding.
        if crate::indexer::value_type_of_scoped(declaration, src, None).is_some() {
            return None;
        }
    }
    let mut args = Vec::new();
    if let Some(parameters) = parameters {
        let mut cursor = parameters.walk();
        for parameter in parameters.named_children(&mut cursor) {
            // shortcut: contextual/destructured parameters stay Unknown until expected-type binding.
            let reference = crate::indexer::value_type_of_scoped(parameter, src, None)?;
            let bounds = crate::indexer::type_parameters_in_scope(
                parameter,
                src,
                &ctx.package,
                &ctx.imports,
            );
            let ty = declared_type_in_scope(index, &reference, ctx, &bounds);
            if ty == Type::Unknown {
                return None;
            }
            args.push(ty);
        }
    }
    let arity = args.len();
    let result = match lambda_tail_expr(lambda) {
        Some(tail) => child_infer.infer_child(index, tail, src, ctx, depth + 1),
        None => intrinsic_type(index, "Unit"),
    };
    if result == Type::Unknown {
        return None;
    }
    args.push(result);
    Some(Type::Class {
        name: format!("Function{arity}"),
        package: Some("kotlin".into()),
        container: None,
        nullable: false,
        args,
        owner: None,
    })
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

            owner: None,
        },
    )
    .map(|entry| entry.sym.type_params.clone())
}

/// The declared type ref to read from a member entry: a function's `return_type` (when
/// `want_function`) or a property's `value_type`.
fn member_type_ref<'a>(
    index: &dyn InferenceIndex,
    receiver: &Type,
    e: &'a crate::index::Entry,
    want_function: bool,
) -> Option<&'a TypeRef> {
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
        matches!(e.sym.kind, SymbolKind::Property | SymbolKind::EnumEntry)
            .then(|| e.sym.value_type.as_ref())
            .flatten()
            .or_else(|| members::kotlin_enum_property_type_ref(index, receiver, e))
            .or_else(|| {
                members::kotlin_java_getter_property_name(e).and(e.sym.return_type.as_ref())
            })
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

        owner: None,
    }
}

#[cfg(test)]
mod tests {
    use super::named_type_params;
    use crate::index::{Index, Tier};
    use crate::symbol::{IndexedSymbol, SymbolKind};

    #[test]
    fn receiver_lambda_roles_preserve_unknown_and_ordinary_lambda_absence() {
        use crate::types::Type;
        let declarations = "package consumer\nclass Known\nfun unknownScope(block: Missing.() -> Unit) {}\nfun <T> genericScope(block: T.() -> Unit) {}\nfun parenthesizedScope(block: (Missing).() -> Unit) {}\nfun plain(block: () -> Unit) {}\nfun run(block: () -> Unit) {}\nfun Known.let(block: (Known) -> Unit) {}\ntypealias UnknownHandler = Missing.() -> Unit\ntypealias KnownHandler = Known.() -> Unit\ntypealias PlainHandler = () -> Unit\nfun choose(a: Int, b: Int) {}\nfun choose(block: Known.() -> Unit) {}\n";
        let mut parser = crate::parser::KotlinParser::new();
        let mut index = Index::new();
        index.replace_file(
            "Declarations.kt",
            crate::indexer::extract_symbols(&parser.parse(declarations), declarations, "consumer"),
            Tier::Volatile,
        );
        let target = "package probe\nclass Owner\n";
        index.replace_file(
            "Owner.kt",
            crate::indexer::extract_symbols(&parser.parse(target), target, "probe"),
            Tier::Volatile,
        );
        for (body, expected) in [
            (
                "fun use() { missing.apply { probe.Owner.AXIS } }",
                vec![Type::Unknown],
            ),
            (
                "fun use() { missing.run { probe.Owner.AXIS } }",
                vec![Type::Unknown],
            ),
            (
                "fun use() { unknownScope { probe.Owner.AXIS } }",
                vec![Type::class("Missing", None)],
            ),
            (
                "fun use() { genericScope { probe.Owner.AXIS } }",
                vec![Type::Unknown],
            ),
            (
                "fun use() { parenthesizedScope { probe.Owner.AXIS } }",
                vec![Type::class("Missing", None)],
            ),
            ("fun use() { plain { probe.Owner.AXIS } }", vec![]),
            ("fun use() { run { probe.Owner.AXIS } }", vec![]),
            ("fun use() { Known().let { probe.Owner.AXIS } }", vec![]),
            (
                "val handler: Missing.() -> Unit = { probe.Owner.AXIS }",
                vec![Type::class("Missing", None)],
            ),
            (
                "val handler: Known.() -> Unit = { probe.Owner.AXIS }",
                vec![Type::class("Known", Some("consumer".into()))],
            ),
            ("val handler: () -> Unit = { probe.Owner.AXIS }", vec![]),
            ("val handler: (() -> Unit)? = { probe.Owner.AXIS }", vec![]),
            (
                "val handler: UnknownHandler = { probe.Owner.AXIS }",
                vec![Type::class("Missing", None)],
            ),
            (
                "val handler: KnownHandler = { probe.Owner.AXIS }",
                vec![Type::class("Known", Some("consumer".into()))],
            ),
            ("val handler: PlainHandler = { probe.Owner.AXIS }", vec![]),
            (
                "fun use() { choose { probe.Owner.AXIS } }",
                vec![Type::class("Known", Some("consumer".into()))],
            ),
        ] {
            let source = format!("package consumer\n{body}\n");
            let tree = parser.parse(&source);
            assert!(!tree.root_node().has_error(), "{source}");
            let start = source.find("probe.Owner.AXIS").unwrap();
            let node = tree
                .root_node()
                .descendant_for_byte_range(start, start + "probe.Owner.AXIS".len())
                .unwrap();
            let ctx = super::FileCtx::from_tree_in_file(&tree, &source, "Use.kt");
            assert_eq!(
                super::implicit_receiver_types(&index, node, &source, &ctx, 0),
                expected,
                "{source}"
            );
            if expected.is_empty()
                || expected.iter().any(|receiver| {
                    receiver == &Type::Unknown || receiver.name() == Some("Missing")
                })
            {
                let qualifier = tree
                    .root_node()
                    .descendant_for_byte_range(start, start + "probe.Owner".len())
                    .unwrap();
                assert_eq!(&source[qualifier.byte_range()], "probe.Owner");
                assert_eq!(
                    crate::resolve::qualified_type_receiver(&index, qualifier, &source, &ctx)
                        .is_some(),
                    expected.is_empty(),
                    "{source}"
                );
            }
        }
    }

    #[test]
    fn unmodeled_implicit_receivers_remain_unknown_in_the_receiver_stack() {
        for source in [
            "package consumer\nclass MissingOwner { fun use() { probe.Owner.AXIS } }\n",
            "package consumer\nfun use() { missing.apply { probe.Owner.AXIS } }\n",
            "package consumer\nfun use() { val owner = object:Missing() { fun use() { probe.Owner.AXIS } } }\n",
        ] {
            let mut parser = crate::parser::KotlinParser::new();
            let tree = parser.parse(source);
            let start = source.find("probe.Owner.AXIS").unwrap();
            let node = tree.root_node().descendant_for_byte_range(start, start + "probe.Owner.AXIS".len()).unwrap();
            let ctx = super::FileCtx::from_tree_in_file(&tree, source, "Use.kt");
            let receivers = super::implicit_receiver_types(&Index::new(), node, source, &ctx, 0);
            assert_eq!(receivers, vec![crate::types::Type::Unknown], "{source}");
        }
    }

    #[test]
    fn java_primitive_arrays_never_bind_kotlin_keyword_homonyms_or_their_ancestors() {
        use crate::types::{Type, TypeParameterBounds, TypeRef};
        use std::collections::{HashMap, HashSet};
        let mut index = Index::new();
        let mut ancestor =
            IndexedSymbol::new("Ancestor", SymbolKind::Class, "consumer", None, 0, 8);
        ancestor.supertypes = Vec::new();
        let member = IndexedSymbol::new(
            "poisoned",
            SymbolKind::Function,
            "consumer",
            Some("Ancestor".into()),
            10,
            18,
        );
        index.replace_file("Ancestor.kt", vec![ancestor, member], Tier::Volatile);
        let ctx = super::FileCtx::new("consumer".into(), Vec::new()).with_file("Use.java");
        for name in [
            "boolean", "byte", "char", "short", "int", "long", "float", "double",
        ] {
            let mut head =
                IndexedSymbol::new(name, SymbolKind::Class, "consumer", None, 0, name.len());
            let mut parent = TypeRef::simple("Ancestor");
            parent.qualification = Some("consumer".into());
            head.supertype_refs = vec![parent];
            index.replace_file(&format!("{name}.kt"), vec![head], Tier::Volatile);
            let nominal = Type::class(name, Some("consumer".into()));
            // Ordinary Kotlin int and an explicitly qualified object-array component stay nominal.
            assert_eq!(
                super::declared_type(&index, &TypeRef::simple(name), &ctx),
                nominal
            );
            let mut qualified = TypeRef::simple(format!("{name}[]"));
            qualified.qualification = Some("consumer".into());
            assert_eq!(
                super::declared_type(&index, &qualified, &ctx),
                Type::array(nominal.clone(), false)
            );
            assert_eq!(
                super::direct_supertypes(&index, &nominal),
                vec![Type::class("Ancestor", Some("consumer".into()))]
            );
            assert!(!crate::jvm::member_candidates(&index, &nominal, "poisoned").is_empty());
            let primitive = Type::class(name, None);
            for rank in [1, 2] {
                let reference = TypeRef::simple(format!("{name}{}", "[]".repeat(rank)));
                let mut expected = primitive.clone();
                for _ in 0..rank {
                    expected = Type::array(expected, false);
                }
                assert_eq!(super::declared_type(&index, &reference, &ctx), expected);
                assert_eq!(
                    super::resolve_with_subst(
                        &index,
                        &reference,
                        &ctx,
                        &HashSet::new(),
                        &HashMap::new(),
                        0
                    ),
                    expected
                );
                let parameters = [TypeParameterBounds {
                    variance: None,
                    erasure: None,
                    name: "T".into(),
                    start_byte: 0,
                    end_byte: 1,
                    scope_start: 0,
                    scope_end: 100,
                    upper_bounds: Vec::new(),
                    complete: true,
                }];
                assert_eq!(
                    super::declared_type_in_scope(&index, &reference, &ctx, &parameters),
                    expected
                );
            }
            // Peeling an array or iterating it must not restart lookup at the homonym's parent.
            assert!(super::direct_supertypes(&index, &primitive).is_empty());
            assert!(crate::jvm::member_candidates(&index, &primitive, "poisoned").is_empty());
            assert_eq!(index.lookup_type(name).len(), 1);
        }
    }

    #[test]
    fn kotlin_nominal_array_index_uses_shared_get_selection() {
        use crate::types::Type;
        let mut index = Index::new();
        let declarations = "package kotlin\nclass Array<T> { operator fun get(index: Int): T = throw Exception() }\nclass IntArray { operator fun get(index: Int): Int = throw Exception() }\n";
        let mut parser = crate::parser::KotlinParser::new();
        let declarations_tree = parser.parse(declarations);
        index.replace_file(
            "/stdlib/Arrays.kt",
            crate::indexer::extract_symbols(&declarations_tree, declarations, "kotlin"),
            Tier::Durable,
        );
        let src = "package consumer\nfun read(values: Array<String>, nullable: Array<String>?, ints: IntArray) {\n values[0]\n values[true]\n nullable[0]\n ints[0]\n}\n";
        let tree = parser.parse(src);
        let ctx = super::FileCtx::from_tree_in_file(&tree, src, "Use.kt");
        for (expression, expected) in [
            ("values[0]", Type::class("String", Some("kotlin".into()))),
            ("values[true]", Type::Unknown),
            ("nullable[0]", Type::Unknown),
            ("ints[0]", Type::class("Int", Some("kotlin".into()))),
        ] {
            let start = src.find(expression).unwrap();
            let node = tree
                .root_node()
                .descendant_for_byte_range(start, start + expression.len())
                .unwrap();
            assert_eq!(&src[node.byte_range()], expression);
            assert_eq!(
                super::infer(&index, node, src, &ctx),
                expected,
                "{expression}"
            );
        }
    }

    #[test]
    fn kotlin_java_array_read_inference_preserves_rank_nullable_and_primitive_identity() {
        use crate::types::{Type, TypeRef};
        let mut index = Index::new();
        for (name, package) in [
            ("Item", "library"),
            ("Factory", "consumer"),
            ("int", "consumer"),
        ] {
            index.replace_file(
                &format!("{name}.kt"),
                vec![IndexedSymbol::new(
                    name,
                    SymbolKind::Class,
                    package,
                    None,
                    0,
                    name.len(),
                )],
                Tier::Volatile,
            );
        }
        // Normalized Java return facts are also exercised through real Java sources in interop.
        let scalars = [
            ("booleans", "boolean"),
            ("bytes", "byte"),
            ("chars", "char"),
            ("shorts", "short"),
            ("numbers", "int"),
            ("longs", "long"),
            ("floats", "float"),
            ("doubles", "double"),
        ];
        let mut returns = Vec::new();
        for (name, nullable) in [("matrix", false), ("nullable", true)] {
            let mut reference = TypeRef::simple("Item[][]");
            reference.qualification = Some("library".into());
            reference.nullable = nullable;
            returns.push((name, reference));
        }
        let mut nominal = TypeRef::simple("int[]");
        nominal.qualification = Some("consumer".into());
        returns.push(("nominal", nominal));
        for (name, primitive) in scalars {
            returns.push((name, TypeRef::simple(format!("{primitive}[]"))));
        }
        let methods = returns
            .into_iter()
            .enumerate()
            .map(|(i, (name, reference))| {
                let mut method = IndexedSymbol::new(
                    name,
                    SymbolKind::Function,
                    "consumer",
                    Some("Factory".into()),
                    100 + i * 20,
                    100 + i * 20 + name.len(),
                );
                method.return_type = Some(reference);
                method.arity = Some(0);
                method.min_arity = Some(0);
                method
            })
            .collect();
        index.replace_file("FactoryMethods.java", methods, Tier::Volatile);
        let mut src = "package consumer\nfun read(factory: Factory) {\n factory.matrix().size\n factory.matrix()[0]\n factory.matrix().get(0)\n factory.matrix()[0][0]\n factory.matrix()[true]\n factory.nullable().size\n factory.nullable()?.size\n factory.nominal()[0]\n".to_string();
        for (name, _) in scalars {
            src.push_str(&format!(" factory.{name}()[0]\n"));
        }
        src.push_str(" factory.matrix().clone()\n factory.matrix().clone()[0][0]\n factory.matrix().clone(0)\n factory.matrix().clone { 0 }\n factory.nullable().clone()\n factory.nullable()?.clone()\n factory.nominal().clone()[0]\n");
        for (name, _) in scalars {
            src.push_str(&format!(" factory.{name}().clone()[0]\n"));
        }
        src.push_str("}\n");
        let tree = crate::parser::KotlinParser::new().parse(&src);
        let ctx = super::FileCtx::from_tree_in_file(&tree, &src, "Use.kt");
        let inferred = |expr: &str| {
            let start = src.find(expr).unwrap();
            let node = tree
                .root_node()
                .descendant_for_byte_range(start, start + expr.len())
                .unwrap();
            assert_eq!(&src[node.byte_range()], expr);
            super::infer(&index, node, &src, &ctx)
        };
        let item = Type::class("Item", Some("library".into()));
        let int = Type::class("Int", Some("kotlin".into()));
        assert_eq!(inferred("factory.matrix().size"), int);
        assert_eq!(
            inferred("factory.matrix()[0]"),
            Type::array(item.clone(), false)
        );
        assert_eq!(
            inferred("factory.matrix().get(0)"),
            Type::array(item.clone(), false)
        );
        assert_eq!(inferred("factory.matrix()[0][0]"), item.clone());
        assert_eq!(inferred("factory.matrix()[true]"), Type::Unknown);
        assert_eq!(inferred("factory.nullable().size"), Type::Unknown);
        assert_eq!(
            inferred("factory.nullable()?.size"),
            int.with_nullability(true)
        );
        assert_eq!(
            inferred("factory.nominal()[0]"),
            Type::class("int", Some("consumer".into()))
        );
        assert_eq!(
            inferred("factory.matrix().clone()"),
            Type::array(Type::array(item.clone(), false), false)
        );
        assert_eq!(inferred("factory.matrix().clone()[0][0]"), item);
        assert_eq!(inferred("factory.matrix().clone(0)"), Type::Unknown);
        assert_eq!(inferred("factory.matrix().clone { 0 }"), Type::Unknown);
        assert_eq!(inferred("factory.nullable().clone()"), Type::Unknown);
        assert_eq!(
            inferred("factory.nullable()?.clone()"),
            Type::array(
                Type::array(Type::class("Item", Some("library".into())), false),
                true
            )
        );
        assert_eq!(
            inferred("factory.nominal().clone()[0]"),
            Type::class("int", Some("consumer".into()))
        );
        for (name, primitive) in scalars {
            assert_eq!(
                inferred(&format!("factory.{name}()[0]")),
                Type::class(super::primitive_type_name(primitive), Some("kotlin".into()))
            );
            assert_eq!(
                inferred(&format!("factory.{name}().clone()[0]")),
                Type::class(super::primitive_type_name(primitive), Some("kotlin".into()))
            );
        }
    }

    #[test]
    fn java_array_shapes_keep_components_and_only_prove_intrinsic_use_types() {
        use crate::members::{java_array_member_type, CallArguments, MemberUse};
        use crate::types::{Type, TypeParameterBounds, TypeRef};
        use std::collections::{HashMap, HashSet};
        use std::sync::Arc;
        let mut index = Index::new();
        index.replace_file(
            "Item.kt",
            vec![IndexedSymbol::new(
                "Item",
                SymbolKind::Class,
                "library",
                None,
                0,
                4,
            )],
            Tier::Volatile,
        );
        let ctx = super::FileCtx::new("consumer".into(), Vec::new());
        let item = Type::class("Item", Some("library".into()));
        let expected = Type::array(Type::array(item.clone(), false), true);
        let mut reference = TypeRef::simple("Item[][]");
        reference.qualification = Some("library".into());
        reference.package_candidates = Arc::new(vec!["library".into()]);
        reference.nullable = true;
        assert_eq!(super::declared_type(&index, &reference, &ctx), expected);
        let component = reference.array_component().unwrap();
        assert_eq!(component.name, "Item[]");
        assert_eq!(component.qualification, reference.qualification);
        assert_eq!(component.package_candidates, reference.package_candidates);
        assert!(!component.nullable);
        let mut substituted = TypeRef::simple("T[][]");
        substituted.nullable = true;
        assert_eq!(
            super::resolve_with_subst(
                &index,
                &substituted,
                &ctx,
                &HashSet::from(["T".into()]),
                &HashMap::from([("T".into(), item)]),
                0
            ),
            expected
        );
        let parameters = [TypeParameterBounds {
            variance: None,
            erasure: None,
            name: "Item".into(),
            start_byte: 10,
            end_byte: 14,
            scope_start: 0,
            scope_end: 100,
            upper_bounds: Vec::new(),
            complete: true,
        }];
        let scoped = super::declared_type_in_scope(
            &index,
            &TypeRef::simple("Item[][]"),
            &ctx.clone().with_file("Use.java"),
            &parameters,
        );
        let Type::Array { component, .. } = scoped else {
            panic!("missing outer dimension")
        };
        let Type::Array { component, .. } = *component else {
            panic!("missing inner dimension")
        };
        let Type::Parameter {
            definition,
            upper_bounds,
            ..
        } = *component
        else {
            panic!("global Item leaked")
        };
        assert_eq!(definition.file, "Use.java");
        assert_eq!(definition.start_byte, 10);
        assert_eq!(upper_bounds, [Type::Unknown]);
        let empty = CallArguments {
            count: Some(0),
            types: Vec::new(),
            names: None,
            has_spread: false,
            trailing_lambda: false,
            trailing_lambda_omits_or_has_zero_parameters: false,
        };
        assert_eq!(
            java_array_member_type(&expected, "length", &MemberUse::Value),
            Some(Type::class("int", None))
        );
        assert_eq!(
            java_array_member_type(&expected, "clone", &MemberUse::Call(empty.clone())),
            Some(expected.clone())
        );
        assert_eq!(
            java_array_member_type(&expected, "length", &MemberUse::Call(empty.clone())),
            None
        );
        for count in [None, Some(1)] {
            assert_eq!(
                java_array_member_type(
                    &expected,
                    "clone",
                    &MemberUse::Call(CallArguments {
                        count,
                        ..empty.clone()
                    })
                ),
                None
            );
        }
        assert_eq!(
            java_array_member_type(&Type::Unknown, "clone", &MemberUse::Call(empty)),
            None
        );
        assert!(!super::type_head_is_known(&index, &expected));
        assert_eq!(super::direct_supertypes(&index, &expected), [Type::Unknown]);
        for malformed in ["[]", "Item[[]"] {
            assert_eq!(
                super::declared_type(&index, &TypeRef::simple(malformed), &ctx),
                Type::Unknown
            );
        }
    }

    #[test]
    fn java_string_identity_survives_partial_indexes_without_hiding_shadowing_or_ambiguity() {
        use crate::types::{Type, TypeRef};
        use std::sync::Arc;

        for (packages, visible, expected) in [
            (vec![], vec!["app", "java.lang"], Some("java.lang")),
            (
                vec!["unrelated"],
                vec!["app", "java.lang"],
                Some("java.lang"),
            ),
            (
                vec!["unrelated", "java.lang"],
                vec!["app", "java.lang"],
                Some("java.lang"),
            ),
            (
                vec!["app", "java.lang"],
                vec!["app", "java.lang"],
                Some("app"),
            ),
            (vec!["other"], vec!["app", "other", "java.lang"], None),
            (
                vec!["other", "java.lang"],
                vec!["app", "other", "java.lang"],
                None,
            ),
            (vec!["other", "java.lang"], vec!["other"], Some("other")),
            (vec!["unrelated"], vec!["missing"], None),
            (
                vec!["other"],
                vec!["app", "other", "kotlin", "java.lang"],
                Some("other"),
            ),
        ] {
            let mut index = Index::new();
            for package in &packages {
                index.replace_file(
                    package,
                    vec![IndexedSymbol::new(
                        "String",
                        SymbolKind::Class,
                        *package,
                        None,
                        0,
                        6,
                    )],
                    Tier::Volatile,
                );
            }
            let mut reference = TypeRef::simple("String");
            reference.package_candidates =
                Arc::new(visible.iter().map(|p| p.to_string()).collect());
            let ctx = super::FileCtx::new("app".into(), Vec::new());
            let actual = super::declared_type(&index, &reference, &ctx);
            let expected = expected.map_or(Type::Unknown, |package| {
                Type::class("String", Some(package.into()))
            });
            assert_eq!(
                actual, expected,
                "indexed {packages:?}, visible {visible:?}"
            );
        }
    }

    #[test]
    fn kotlin_primitive_identity_survives_indexed_java_wrappers() {
        use crate::parser::KotlinParser;
        use crate::types::Type;

        for name in ["Byte", "Short", "Long", "Float", "Double", "Boolean"] {
            let mut index = Index::new();
            for package in ["kotlin", "java.lang"] {
                index.replace_file(
                    package,
                    vec![IndexedSymbol::new(
                        name,
                        SymbolKind::Class,
                        package,
                        None,
                        0,
                        name.len(),
                    )],
                    Tier::Durable,
                );
            }
            let mut parser = KotlinParser::new();
            for (package, imports, written, expected) in [
                ("app", String::new(), name, "kotlin"),
                ("app", "import java.lang.*\n".into(), name, "kotlin"),
                (
                    "app",
                    "import kotlin.*\nimport java.lang.*\n".into(),
                    name,
                    "kotlin",
                ),
                (
                    "app",
                    "import java.lang.*\nimport kotlin.*\n".into(),
                    name,
                    "kotlin",
                ),
                (
                    "app",
                    format!("import java.lang.{name}\n"),
                    name,
                    "java.lang",
                ),
                (
                    "app",
                    format!("import java.lang.{name} as JValue\n"),
                    "JValue",
                    "java.lang",
                ),
                ("java.lang", String::new(), name, "java.lang"),
            ] {
                let source = format!(
                    "package {package}\n{imports}fun echo(value: {written}): {written} = value\n"
                );
                let symbols =
                    crate::indexer::extract_symbols(&parser.parse(&source), &source, package);
                let function = symbols.iter().find(|symbol| symbol.name == "echo").unwrap();
                let reference = function.return_type.as_ref().unwrap();
                // Shared declaration facts keep their source package even in another consumer context.
                for consumer in ["app", "java.lang"] {
                    assert_eq!(
                        super::declared_type(
                            &index,
                            reference,
                            &super::FileCtx::new(consumer.into(), Vec::new())
                        ),
                        Type::class(name, Some(expected.into())),
                        "{source}, consumer {consumer}"
                    );
                }
            }
            index.replace_file(
                "Shadow.kt",
                vec![IndexedSymbol::new(
                    name,
                    SymbolKind::Class,
                    "app",
                    None,
                    0,
                    name.len(),
                )],
                Tier::Volatile,
            );
            let source = format!("package app\nfun echo(value: {name}): {name} = value\n");
            let symbols = crate::indexer::extract_symbols(&parser.parse(&source), &source, "app");
            let reference = symbols
                .iter()
                .find(|symbol| symbol.name == "echo")
                .unwrap()
                .return_type
                .as_ref()
                .unwrap();
            assert_eq!(
                super::declared_type(
                    &index,
                    reference,
                    &super::FileCtx::new("consumer".into(), Vec::new())
                ),
                Type::class(name, Some("app".into()))
            );
            assert_eq!(
                super::declared_type(
                    &index,
                    &crate::types::TypeRef::simple(name),
                    &super::FileCtx::new("java.lang".into(), Vec::new())
                ),
                Type::class(name, Some("java.lang".into())),
                "legacy context without stamped candidates"
            );
        }
    }

    #[test]
    fn implicit_reference_root_consistency_preserves_nullability_and_primitive_phases() {
        use crate::types::Type;
        let ty = |name: &str, package: &str| Type::class(name, Some(package.into()));
        let mut index = Index::new();
        index.replace_file(
            "Roots.kt",
            [
                ("Actual", "probe"),
                ("Object", "probe"),
                ("Object", "java.lang"),
            ]
            .into_iter()
            .map(|(name, package)| {
                IndexedSymbol::new(name, SymbolKind::Class, package, None, 0, name.len())
            })
            .collect(),
            Tier::Volatile,
        );
        let actual = ty("Actual", "probe");
        let object = ty("Object", "java.lang");
        assert!(super::types_consistent(&index, &actual, &object));
        assert!(!super::types_consistent(
            &index,
            &actual.clone().with_nullability(true),
            &object
        ));
        assert!(super::types_consistent(
            &index,
            &actual.clone().with_nullability(true),
            &object.with_nullability(true)
        ));
        assert!(!super::types_consistent(
            &index,
            &actual,
            &ty("Object", "probe")
        ));
        assert!(!super::types_consistent(
            &index,
            &ty("Int", "kotlin"),
            &ty("Object", "java.lang")
        ));
        assert!(!super::types_consistent(
            &index,
            &Type::class("int", None),
            &ty("Object", "java.lang")
        ));
        assert!(super::types_consistent(
            &index,
            &ty("Int", "kotlin"),
            &ty("Any", "kotlin")
        ));
    }

    #[test]
    fn nominal_head_exclusion_requires_closed_unaliased_invariant_hierarchies() {
        use crate::symbol::TypeParameterFacts;
        use crate::types::{Type, TypeRef};
        let ty = |name: &str| Type::class(name, Some("probe".into()));
        let declaration = |name: &str| {
            let mut symbol =
                IndexedSymbol::new(name, SymbolKind::Class, "probe", None, 0, name.len());
            symbol.type_parameter_facts = TypeParameterFacts::new(true, true);
            symbol
        };
        let mut head = declaration("Head");
        head.type_params = vec!["T".into()];
        let mut formal = ty("Head");
        if let Type::Class { args, .. } = &mut formal {
            *args = vec![Type::Unknown];
        }
        for (case, expected) in [
            ("closed", true),
            ("missing", false),
            ("cycle", false),
            ("alias", false),
            ("variance", false),
            ("bounds", false),
            ("subtype", false),
        ] {
            let mut actual = declaration("Actual");
            match case {
                "missing" => actual.supertype_refs = vec![TypeRef::simple("Missing")],
                "cycle" => actual.supertype_refs = vec![TypeRef::simple("Actual")],
                "alias" => actual.kind = SymbolKind::TypeAlias,
                "variance" => actual.type_parameter_facts = TypeParameterFacts::new(true, false),
                "bounds" => actual.type_parameter_facts = TypeParameterFacts::new(false, true),
                "subtype" => {
                    let mut parent = TypeRef::simple("Head");
                    parent.args = vec![TypeRef::simple("String")];
                    actual.supertype_refs = vec![parent];
                }
                _ => {}
            }
            let mut index = Index::new();
            index.replace_file("Heads.kt", vec![head.clone(), actual], Tier::Volatile);
            assert_eq!(
                super::nominal_heads_incompatible(&index, &ty("Actual"), &formal),
                expected,
                "{case}"
            );
            for universal in [
                Type::class("Any", Some("kotlin".into())),
                Type::class("Object", Some("java.lang".into())),
            ] {
                assert!(
                    !super::nominal_heads_incompatible(&index, &ty("Actual"), &universal),
                    "implicit universal superclass: {case}"
                );
            }
            let mut alias = declaration("Alias");
            alias.kind = SymbolKind::TypeAlias;
            index.replace_file("Alias.kt", vec![alias], Tier::Volatile);
            assert!(!super::nominal_heads_incompatible(
                &index,
                &ty("Actual"),
                &ty("Alias")
            ));
        }
        let mut index = Index::new();
        index.replace_file("Head.kt", vec![head], Tier::Volatile);
        assert!(!super::nominal_heads_incompatible(
            &index,
            &Type::class("Boolean", Some("kotlin".into())),
            &formal
        ));
        for package in ["java.lang", "kotlin"] {
            assert!(!super::nominal_heads_incompatible(
                &index,
                &Type::Unknown,
                &Type::class("Object", Some(package.into()))
            ));
        }
    }

    #[test]
    fn plain_method_variable_result_uses_current_normalized_argument_facts() {
        use crate::members::CallArguments;
        use crate::parser::KotlinParser;
        use crate::types::Type;

        let source = "package app\nclass Item\nclass Other\nclass Identity {\n fun <T> id(value: T): T = value\n}\n";
        let mut parser = KotlinParser::new();
        let mut index = Index::new();
        index.replace_file(
            "Types.kt",
            crate::indexer::extract_symbols(&parser.parse(source), source, "app"),
            Tier::Volatile,
        );
        let entry = index.lookup_by_name("id").first().unwrap();
        let receiver = Type::class("Identity", Some("app".into()));
        let ctx = super::FileCtx::new("app".into(), Vec::new());
        for actual in [
            Type::class("Item", Some("app".into())),
            Type::class("Other", Some("app".into())),
            Type::Unknown,
        ] {
            let arguments = CallArguments {
                count: Some(1),
                types: vec![actual.clone()],
                names: None,
                has_spread: false,
                trailing_lambda: false,
                trailing_lambda_omits_or_has_zero_parameters: false,
            };
            assert_eq!(
                super::declared_member_call_type(
                    &index,
                    &receiver,
                    entry,
                    entry.sym.return_type.as_ref().unwrap(),
                    &arguments,
                    crate::members::CallMode::Kotlin,
                    &ctx,
                ),
                actual,
            );
        }
    }

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

        let receiver = Type::class("Use", Some("demo".into()));
        let mut parent = IndexedSymbol::new("Parent", SymbolKind::Class, "demo", None, 0, 1);
        parent.type_parameter_facts = crate::symbol::TypeParameterFacts::new(true, true);
        index.replace_file("Parent.kt", vec![parent.clone()], Tier::Volatile);
        let mut java = IndexedSymbol::new("Use", SymbolKind::Class, "demo", None, 0, 1);
        java.supertype_refs = vec![TypeRef::simple("Parent")];
        java.supertype_refs[0].java_raw_eligible = true;
        let mut kotlin = java.clone();
        kotlin.supertype_refs[0].java_raw_eligible = false;
        index.replace_file("Use.java", vec![java.clone()], Tier::Volatile);
        index.replace_file("Use.kt", vec![kotlin.clone()], Tier::Volatile);
        assert_eq!(
            super::direct_supertypes(&index, &receiver),
            vec![Type::class("Parent", Some("demo".into()))]
        );

        // An actual generic, an unavailable producer, and an unknown header cannot borrow the
        // nongeneric agreement proof. The two omission views must remain distinguishable.
        let declarations = "package demo\nopen class Parent<T>";
        let mut parser = crate::parser::KotlinParser::new();
        index.replace_file(
            "Parent.kt",
            crate::indexer::extract_symbols(&parser.parse(declarations), declarations, "demo"),
            Tier::Volatile,
        );
        let ctx = super::FileCtx::new("demo".into(), Vec::new());
        assert!(matches!(
            super::declared_type(&index, &java.supertype_refs[0], &ctx),
            Type::Raw(_)
        ));
        assert!(matches!(
            super::declared_type(&index, &kotlin.supertype_refs[0], &ctx),
            Type::Class { .. }
        ));
        assert_eq!(
            super::direct_supertypes(&index, &receiver),
            vec![Type::Unknown]
        );
        index.replace_file("Parent.kt", Vec::new(), Tier::Volatile);
        assert_eq!(
            super::direct_supertypes(&index, &receiver),
            vec![Type::Unknown]
        );
        parent.type_params.clear();
        parent.type_parameter_facts = Default::default();
        index.replace_file("Parent.kt", vec![parent], Tier::Volatile);
        assert_eq!(
            super::direct_supertypes(&index, &receiver),
            vec![Type::Unknown]
        );

        for (declarations, accepted) in [
            ("package demo\nclass Outer<T> { inner class Inner }", false),
            ("package demo\nclass Outer { inner class Inner }", true),
        ] {
            let mut parser = crate::parser::KotlinParser::new();
            index.replace_file(
                "Outer.kt",
                crate::indexer::extract_symbols(&parser.parse(declarations), declarations, "demo"),
                Tier::Volatile,
            );
            for symbol in [&mut java, &mut kotlin] {
                symbol.supertype_refs[0].name = "Inner".into();
                symbol.supertype_refs[0].qualification = Some("demo.Outer".into());
            }
            index.replace_file("Use.java", vec![java.clone()], Tier::Volatile);
            index.replace_file("Use.kt", vec![kotlin.clone()], Tier::Volatile);
            let supers = super::direct_supertypes(&index, &receiver);
            assert_eq!(
                supers[0] != Type::Unknown,
                accepted,
                "{declarations}: {supers:?}"
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
            (
                "package app\nclass Box<T>\nopen class A<T> : A<Box<T>>()\nclass Item\n",
                1,
            ),
            (
                "package app\nclass Pair<L, R>\nopen class A<T> : A<Pair<T, T>>()\nclass Item\n",
                1,
            ),
            (
                "package app\nclass Pair<L, R>\nopen class A<T> : B<Pair<T, T>>()\nopen class B<T> : A<Pair<T, T>>()\nclass Item\n",
                2,
            ),
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
                super::infer(
                    &index,
                    node,
                    source,
                    &super::FileCtx::from_tree(&tree, source)
                ),
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
    if tr.name.ends_with("[]") {
        let Some(component) = tr.array_component().filter(|_| depth < MAX_DEPTH) else {
            return Type::Unknown;
        };
        return Type::array(
            java_primitive_array_component(&component)
                .unwrap_or_else(|| resolve_type_ref(index, &component, ctx, depth + 1)),
            tr.nullable,
        );
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
    let ty = Type::Class {
        name: tr.name.clone(),
        package,
        container,
        nullable: tr.nullable,
        args,

        owner: None,
    };
    let ty = match tr.owner.as_deref() {
        Some(owner) if depth < MAX_DEPTH => {
            with_instantiated_owner(index, ty, resolve_type_ref(index, owner, ctx, depth + 1))
        }
        Some(_) => Type::Unknown,
        None => ty,
    };
    java_omitted_type(index, ty, tr.java_raw_eligible, depth)
}

/// Resolve normalized declaration facts without requiring a particular source grammar.
pub fn declared_type(index: &dyn InferenceIndex, tr: &TypeRef, ctx: &FileCtx) -> Type {
    resolve_type_ref(index, tr, ctx, 0)
}

/// Resolve lexical parameters before nominal names, using the declaring source's visibility.
/// Bounds expose members; they do not solve bounded generic applications or certify absence.
pub fn declared_type_in_scope(
    index: &dyn InferenceIndex,
    tr: &TypeRef,
    ctx: &FileCtx,
    parameters: &[TypeParameterBounds],
) -> Type {
    fn bound_known(index: &dyn InferenceIndex, ty: &Type) -> bool {
        match ty {
            Type::Parameter { upper_bounds, .. } => {
                !upper_bounds.is_empty()
                    && upper_bounds.iter().all(|bound| bound_known(index, bound))
            }
            Type::Class { .. } => receiver_arguments_known(index, ty, 0),
            _ => false,
        }
    }
    fn resolve(
        index: &dyn InferenceIndex,
        tr: &TypeRef,
        ctx: &FileCtx,
        parameters: &[TypeParameterBounds],
        path: &mut Vec<usize>,
        depth: usize,
        position: Option<usize>,
        remaining: &mut usize,
    ) -> Type {
        if depth > MAX_DEPTH || tr.name.is_empty() || *remaining == 0 {
            return Type::Unknown;
        }
        *remaining -= 1;
        if tr.projection == Projection::Star {
            return Type::Star;
        }
        if tr.name.ends_with("[]") {
            let Some(component) = tr.array_component().filter(|_| depth < MAX_DEPTH) else {
                return Type::Unknown;
            };
            return Type::projected(
                tr.projection,
                Type::array(
                    java_primitive_array_component(&component).unwrap_or_else(|| {
                        resolve(
                            index,
                            &component,
                            ctx,
                            parameters,
                            path,
                            depth + 1,
                            position,
                            remaining,
                        )
                    }),
                    tr.nullable,
                ),
            );
        }
        let parameter = tr
            .qualification
            .is_none()
            .then(|| {
                parameters.iter().find(|parameter| {
                    parameter.name == tr.name
                        && position.is_none_or(|at| {
                            parameter.scope_start <= at && at < parameter.scope_end
                        })
                })
            })
            .flatten();
        let resolved = if let Some(parameter) = parameter {
            let Some(file) = &ctx.file else {
                return Type::Unknown;
            };
            if path.contains(&parameter.start_byte) {
                return Type::Unknown;
            }
            path.push(parameter.start_byte);
            let mut upper_bounds = if parameter.complete && tr.args.is_empty() {
                parameter
                    .upper_bounds
                    .iter()
                    .map(|bound| {
                        resolve(
                            index,
                            bound,
                            ctx,
                            parameters,
                            path,
                            depth + 1,
                            Some(parameter.start_byte),
                            remaining,
                        )
                    })
                    .collect::<Vec<_>>()
            } else {
                vec![Type::Unknown]
            };
            path.pop();
            if upper_bounds.is_empty() && file.ends_with(".kt") {
                upper_bounds.push(Type::class("Any", Some("kotlin".into())).with_nullability(true));
            }
            // shortcut: Java implicit roots and recursive bounds stay incomplete until separately covered.
            if upper_bounds.is_empty()
                || !upper_bounds.iter().all(|bound| bound_known(index, bound))
            {
                upper_bounds = vec![Type::Unknown];
            }
            let nullable = tr.nullable
                || upper_bounds.iter().all(|bound| {
                    !bound.is_member_receiver()
                        || bound.is_nullable()
                        || matches!(bound, Type::Class { .. }) && !type_head_is_known(index, bound)
                });
            Type::Parameter {
                definition: crate::symbol::Def {
                    file: file.to_string(),
                    start_byte: parameter.start_byte,
                    end_byte: parameter.end_byte,
                },
                name: parameter.name.clone(),
                nullable,
                nullability: if tr.nullable {
                    crate::types::ParameterNullability::Nullable
                } else {
                    crate::types::ParameterNullability::Inherit
                },
                upper_bounds,
            }
        } else {
            let Some((package, container)) = type_ref_identity(index, tr, ctx) else {
                return Type::Unknown;
            };
            let ty = Type::Class {
                name: tr.name.clone(),
                package,
                container,
                nullable: tr.nullable,
                args: tr
                    .args
                    .iter()
                    .map(|arg| {
                        resolve(
                            index,
                            arg,
                            ctx,
                            parameters,
                            path,
                            depth + 1,
                            position,
                            remaining,
                        )
                    })
                    .collect(),

                owner: None,
            };
            let ty = match tr.owner.as_deref() {
                Some(owner) => with_instantiated_owner(
                    index,
                    ty,
                    resolve(
                        index,
                        owner,
                        ctx,
                        parameters,
                        path,
                        depth + 1,
                        position,
                        remaining,
                    ),
                ),
                None => ty,
            };
            java_omitted_type(index, ty, tr.java_raw_eligible, depth)
        };
        Type::projected(tr.projection, resolved)
    }
    if parameters.is_empty() {
        return declared_type(index, tr, ctx);
    }
    // shortcut: cap total bound expansion until repeated parameter dependencies are memoized.
    let mut remaining = MAX_DEPTH * MAX_DEPTH;
    resolve(
        index,
        tr,
        ctx,
        parameters,
        &mut Vec::new(),
        0,
        None,
        &mut remaining,
    )
}

fn type_ref_identity(
    index: &dyn InferenceIndex,
    tr: &TypeRef,
    ctx: &FileCtx,
) -> Option<(Option<String>, Option<String>)> {
    if members::function_type_signature(tr).is_some() {
        let package = tr
            .qualification
            .as_deref()
            .unwrap_or_else(|| &tr.package_candidates[0]);
        return Some((Some(package.to_string()), None));
    }
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
        if package.is_none()
            && primitive_type_name(&tr.name) != tr.name
            && tr
                .package_candidates
                .iter()
                .any(|package| package == "kotlin")
        {
            return None;
        }
        Some((package, resolve_type_ref_container(index, tr, ctx)))
    }
}

fn intrinsic_type(index: &dyn InferenceIndex, name: &str) -> Type {
    Type::class(name, Some(intrinsic_package(index, name).to_string()))
}

fn intrinsic_package(index: &dyn InferenceIndex, name: &str) -> &'static str {
    // Mapped references use JDK source members when Kotlin builtin declarations are unavailable.
    if crate::defaults::is_jvm_reference_builtin(name) {
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
            // Default/wildcard JVM wrappers do not compete with Kotlin's primitive builtins.
            // A declaration in java.lang still binds its own package; explicit imports were
            // already expanded into qualification, above. Prefer declaration context to use-site ctx.
            && !(entry.sym.package == "java.lang"
                && entry.sym.container.is_none()
                && (crate::defaults::is_jvm_reference_builtin(&tr.name)
                    || matches!(tr.name.as_str(), "Byte" | "Short" | "Long" | "Float" | "Double" | "Boolean")
                        && tr.package_candidates.first().map_or(ctx.package.as_str(), String::as_str)
                            != "java.lang"))
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
    // Java's implicit String exists before JDK indexing, even when unrelated homonyms arrive first.
    let java_string = tr.name == "String" && !tr.package_candidates.iter().any(|p| p == "kotlin");
    if candidates.is_empty() && !java_string {
        return None;
    }
    let has_type = |package: &str| {
        java_string && package == "java.lang"
            || candidates.iter().any(|entry| entry.sym.package == package)
    };
    let mut pkgs = tr.package_candidates.iter();
    let primary = pkgs.next()?;
    if has_type(primary) {
        return Some(primary.clone());
    }
    let mut rest_hits: Vec<String> = Vec::new();
    for pkg in pkgs {
        if has_type(pkg) && !rest_hits.iter().any(|p| p == pkg) {
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
