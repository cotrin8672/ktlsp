//! Goto-definition resolution. Pure; operates on a parsed file plus the cross-file index.
//!
//! Strategy (in order):
//!  0. If the cursor is *on a declaration's own name*, return that location (never fall through to
//!     a homonym elsewhere).
//!  1. **Local scope walk** — from the usage, climb ancestor scopes (block, lambda, function,
//!     class, file …) and return the nearest visible binding. Innermost scope wins (shadowing),
//!     and block-locals must be declared before use.
//!  2. **Cross-file lookup** — by name in the index, filtered by the usage's *kind* (type vs call
//!     vs value), then ranked by `as`-alias / explicit import / same package / wildcard import.
//!     Unimported other-package symbols do NOT match.
//!
//! Member access (`receiver.member`) is best-effort: the receiver's type is unknown in v1, so a
//! selector resolves only if its name is unique in the index — an editor prefers no result over
//! several wrong ones.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::ControlFlow;

use tree_sitter::{Node, Tree};

use crate::index::{Entry, InferenceIndex};
use crate::infer;
use crate::knowledge::Knowledge;
use crate::members::{self, MemberUse};
use crate::parser::{
    child_of_kind, class_kind, first_ident, identifier_at, imports_of, name_field, node_text,
    package_of, Import,
};
use crate::project_model::{self, ProjectPackageScope};
use crate::symbol::{Def, SymbolKind};
use crate::types::{Type, TypeRef};

/// Coarse index/source completeness facts used only for negative diagnostics. These are deliberately
/// separate from the symbol index: an empty lookup proves absence only when the caller knows the
/// relevant source worlds have been indexed cleanly.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompletenessFacts {
    pub project_scan_complete: bool,
    pub project_packages_complete: BTreeSet<String>,
    pub project_scoped_packages_complete: BTreeSet<ProjectPackageScope>,
    pub project_packages_with_unknown_scope: BTreeSet<String>,
    pub project_package_modules: BTreeMap<String, BTreeSet<String>>,
    pub library_index_complete: bool,
    pub jdk_index_complete: bool,
    /// No unknown compiler producer may add types or members to this source world.
    pub compiler_producers_complete: bool,
    /// Accepted annotations may augment their owner even when authored source is clean.
    pub member_surfaces_unknown: BTreeSet<Def>,
}

impl CompletenessFacts {
    /// Explicit closed-source fixtures/callers also certify compiler-producer coverage.
    pub fn complete() -> Self {
        Self {
            project_scan_complete: true,
            project_packages_complete: BTreeSet::new(),
            project_scoped_packages_complete: BTreeSet::new(),
            project_packages_with_unknown_scope: BTreeSet::new(),
            project_package_modules: BTreeMap::new(),
            library_index_complete: true,
            jdk_index_complete: true,
            compiler_producers_complete: true,
            member_surfaces_unknown: BTreeSet::new(),
        }
    }

    pub fn compiler_surface_status(&self, owner: Option<&Def>) -> ResolutionStatus<()> {
        let mut reasons = Vec::new();
        if !self.compiler_producers_complete {
            reasons.push(IncompletenessReason::CompilerProducerIncomplete);
        }
        if let Some(owner) = owner.filter(|owner| self.member_surfaces_unknown.contains(*owner)) {
            reasons.push(IncompletenessReason::MemberSurfaceIncomplete(owner.clone()));
        }
        if reasons.is_empty() {
            ResolutionStatus::Found(())
        } else {
            ResolutionStatus::Unknown(reasons)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IncompletenessReason {
    ProjectPackageIncomplete(String),
    ProjectSourceSetPackageIncomplete(ProjectPackageScope),
    LibraryPackageIncomplete(String),
    JdkPackageIncomplete(String),
    NotSimpleTypeName,
    NotSimpleName,
    NotTypePosition,
    NotReferencePosition,
    UnknownReceiverType,
    AmbiguousReceiverTypePackage(String),
    ReceiverTypeNotIndexed(String),
    QueryScopeIncomplete,
    CompilerProducerIncomplete,
    MemberSurfaceIncomplete(Def),
}

pub type ResolutionStatus<T> = Knowledge<T, IncompletenessReason>;

impl IncompletenessReason {
    pub fn label(&self) -> String {
        match self {
            IncompletenessReason::ProjectPackageIncomplete(pkg) => {
                format!("project-package-incomplete:{pkg}")
            }
            IncompletenessReason::ProjectSourceSetPackageIncomplete(scope) => {
                format!("project-source-set-package-incomplete:{}", scope.label())
            }
            IncompletenessReason::LibraryPackageIncomplete(pkg) => {
                format!("library-package-incomplete:{pkg}")
            }
            IncompletenessReason::JdkPackageIncomplete(pkg) => {
                format!("jdk-package-incomplete:{pkg}")
            }
            IncompletenessReason::NotSimpleTypeName => "not-simple-type-name".to_string(),
            IncompletenessReason::NotSimpleName => "not-simple-name".to_string(),
            IncompletenessReason::NotTypePosition => "not-type-position".to_string(),
            IncompletenessReason::NotReferencePosition => "not-reference-position".to_string(),
            IncompletenessReason::UnknownReceiverType => "unknown-receiver-type".to_string(),
            IncompletenessReason::AmbiguousReceiverTypePackage(name) => {
                format!("ambiguous-receiver-type-package:{name}")
            }
            IncompletenessReason::ReceiverTypeNotIndexed(name) => {
                format!("receiver-type-not-indexed:{name}")
            }
            IncompletenessReason::QueryScopeIncomplete => "query-scope-incomplete".to_string(),
            IncompletenessReason::CompilerProducerIncomplete => {
                "compiler-producer-incomplete".to_string()
            }
            IncompletenessReason::MemberSurfaceIncomplete(definition) => format!(
                "member-surface-incomplete:{}:{}:{}",
                definition.file, definition.start_byte, definition.end_byte
            ),
        }
    }
}

/// Where an identifier sits syntactically — determines which symbol kinds may resolve it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UseKind {
    /// In type position: `val x: Foo`, `fun f(): Foo`.
    Type,
    /// The callee of a call: `Foo(...)`.
    Call,
    /// The selector of `receiver.member`.
    MemberSelector,
    /// Anything else (plain value reference, navigation receiver, …).
    Value,
}

pub fn use_kind(usage: Node) -> UseKind {
    use_kind_in(usage, usage.parent())
}

/// [`use_kind`] with the parent supplied by the caller (e.g. from an ancestor slice), avoiding
/// a root-redescending `Node::parent` call per query.
pub(crate) fn use_kind_in(usage: Node, parent: Option<Node>) -> UseKind {
    if let Some(parent) = parent {
        match parent.kind() {
            "infix_expression" => return UseKind::Call,
            "navigation_expression" => {
                // First named child is the receiver; anything after is a selector.
                if parent.named_child(0) != Some(usage) {
                    return UseKind::MemberSelector;
                }
                return UseKind::Value;
            }
            "user_type" => return UseKind::Type,
            "call_expression" => {
                if parent.named_child(0) == Some(usage) {
                    return UseKind::Call;
                }
                return UseKind::Value;
            }
            _ => {}
        }
    }
    UseKind::Value
}

pub fn reference_status(
    index: &dyn InferenceIndex,
    file: &str,
    tree: &Tree,
    src: &str,
    usage: Node,
    facts: &CompletenessFacts,
) -> ResolutionStatus<()> {
    let ctx = infer::FileCtx::from_tree_in_file(tree, src, file);
    reference_status_with_ctx(index, file, tree, src, usage, facts, &ctx)
}

pub fn reference_status_with_ctx(
    index: &dyn InferenceIndex,
    file: &str,
    tree: &Tree,
    src: &str,
    usage: Node,
    facts: &CompletenessFacts,
    ctx: &infer::FileCtx,
) -> ResolutionStatus<()> {
    let _ = tree;
    let ancestors = crate::parser::ancestors_of(usage);
    reference_status_node(index, file, src, usage, &ancestors, facts, ctx)
}

/// [`reference_status_with_ctx`] with the usage node's ancestor chain supplied root-first by
/// the caller. Batch traversals that already visit every node pass the chain down so
/// classification never redescends the tree.
pub(crate) fn reference_status_node(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    usage: Node,
    ancestors: &[Node],
    facts: &CompletenessFacts,
    ctx: &infer::FileCtx,
) -> ResolutionStatus<()> {
    if usage.kind() != "identifier" {
        return ResolutionStatus::Unknown(vec![IncompletenessReason::NotSimpleName]);
    }
    let parent = ancestors.last().copied();
    if is_non_reference_identifier_in(usage, parent, src) {
        return ResolutionStatus::Unknown(vec![IncompletenessReason::NotReferencePosition]);
    }
    if let Some(import) = ancestor_of_kind_a(ancestors, "import") {
        let Some(path) = child_of_kind(import, "qualified_identifier") else {
            return ResolutionStatus::Unknown(vec![IncompletenessReason::NotReferencePosition]);
        };
        if direct_identifier_parts(path, src).last().map(|part| part.0) != Some(usage) {
            return ResolutionStatus::Unknown(vec![IncompletenessReason::NotReferencePosition]);
        }
        let path = node_text(path, src);
        let Some(import) = ctx.imports.iter().find(|import| import.path == path) else {
            return ResolutionStatus::Unknown(vec![IncompletenessReason::NotReferencePosition]);
        };
        return import_target_status(index, file, import, facts, ctx);
    }
    if is_declaration_identifier_in(usage, parent)
        || has_ancestor_kind_a(ancestors, &["package_header"])
    {
        return ResolutionStatus::Unknown(vec![IncompletenessReason::NotReferencePosition]);
    }
    if !goto_node_with_ctx(index, file, src, usage, ancestors, ctx).is_empty() {
        return ResolutionStatus::Found(());
    }
    if !index.scope_is_complete() {
        return ResolutionStatus::Unknown(vec![IncompletenessReason::QueryScopeIncomplete]);
    }
    if node_text(usage, src) == "it" {
        if has_ancestor_kind_a(ancestors, &["lambda_literal"]) {
            return ResolutionStatus::Found(());
        }
        if infer::infer(index, usage, src, ctx).name().is_some() {
            return ResolutionStatus::Found(());
        }
    }
    let uk = use_kind_in(usage, parent);
    match uk {
        UseKind::Type => {
            simple_type_name_status_with_ctx(index, file, src, usage, parent, ancestors, facts, ctx)
        }
        UseKind::Call | UseKind::Value => {
            if is_navigation_receiver_in(usage, parent) {
                return ResolutionStatus::Unknown(vec![IncompletenessReason::NotReferencePosition]);
            }
            if !is_simple_identifier(usage) {
                return ResolutionStatus::Unknown(vec![IncompletenessReason::NotSimpleName]);
            }
            if !infer::implicit_receiver_types_with_ancestors(index, ancestors, src, ctx, 0)
                .is_empty()
            {
                return ResolutionStatus::Unknown(vec![IncompletenessReason::UnknownReceiverType]);
            }
            simple_name_status_a(index, file, src, usage, ancestors, uk, facts, ctx)
        }
        UseKind::MemberSelector => {
            member_name_status_a(index, file, src, usage, parent, facts, ctx)
        }
    }
}

/// Certify whether a simple type-name usage is definitely absent from the current file's visible
/// type scope. This is intentionally narrower than goto-definition:
/// - only simple `user_type` names are considered;
/// - absence is proved against an explicit visibility/completeness model rather than a best-effort
///   empty goto result;
/// - every visible package world involved in the lookup must be marked complete before absence is
///   reported.
///
/// Callers should use this for diagnostics, not navigation. Navigation can return "no result" when
/// unsure; diagnostics need a proof boundary for the negative case.
pub fn simple_type_name_status(
    index: &dyn InferenceIndex,
    file: &str,
    tree: &Tree,
    src: &str,
    usage: Node,
    facts: &CompletenessFacts,
) -> ResolutionStatus<()> {
    let parent = usage.parent();
    if use_kind_in(usage, parent) != UseKind::Type {
        return ResolutionStatus::Unknown(vec![IncompletenessReason::NotTypePosition]);
    }
    if !is_simple_user_type_identifier_in(usage, parent, src) {
        return ResolutionStatus::Unknown(vec![IncompletenessReason::NotSimpleTypeName]);
    }
    let ctx = infer::FileCtx::from_tree_in_file(tree, src, file);
    let ancestors = crate::parser::ancestors_of(usage);
    simple_type_name_status_with_ctx(index, file, src, usage, parent, &ancestors, facts, &ctx)
}

fn simple_type_name_status_with_ctx(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    usage: Node,
    parent: Option<Node>,
    ancestors: &[Node],
    facts: &CompletenessFacts,
    ctx: &infer::FileCtx,
) -> ResolutionStatus<()> {
    if use_kind_in(usage, parent) != UseKind::Type {
        return ResolutionStatus::Unknown(vec![IncompletenessReason::NotTypePosition]);
    }
    if !is_simple_user_type_identifier_in(usage, parent, src) {
        return ResolutionStatus::Unknown(vec![IncompletenessReason::NotSimpleTypeName]);
    }
    simple_name_status_a(
        index,
        file,
        src,
        usage,
        ancestors,
        UseKind::Type,
        facts,
        ctx,
    )
}

fn is_simple_user_type_identifier_in(usage: Node, parent: Option<Node>, src: &str) -> bool {
    let Some(parent) = parent else {
        return false;
    };
    if parent.kind() != "user_type" {
        return false;
    }
    let parts = direct_identifier_parts(parent, src);
    parts.len() == 1 && parts[0].0 == usage
}

fn is_simple_identifier(usage: Node) -> bool {
    usage.kind() == "identifier"
}

fn is_navigation_receiver_in(usage: Node, parent: Option<Node>) -> bool {
    parent.is_some_and(|parent| {
        parent.kind() == "navigation_expression" && parent.named_child(0) == Some(usage)
    })
}

fn is_declaration_identifier_in(node: Node<'_>, parent: Option<Node<'_>>) -> bool {
    let Some(parent) = parent else {
        return false;
    };
    match parent.kind() {
        "variable_declaration"
        | "parameter"
        | "class_parameter"
        | "type_parameter"
        | "enum_entry" => true,
        "class_declaration" | "object_declaration" | "function_declaration" => {
            parent.child_by_field_name("name").is_some_and(|name| {
                name.start_byte() == node.start_byte() && name.end_byte() == node.end_byte()
            })
        }
        "type_alias" => parent.child_by_field_name("type") == Some(node),
        _ => false,
    }
}

fn is_non_reference_identifier_in(node: Node<'_>, parent: Option<Node<'_>>, src: &str) -> bool {
    if is_named_argument_label_in(node, parent) {
        return true;
    }
    matches!(
        node_text(node, src),
        "true" | "false" | "null" | "break" | "continue"
    )
}

fn is_named_argument_label_in(node: Node<'_>, parent: Option<Node<'_>>) -> bool {
    let Some(parent) = parent else {
        return false;
    };
    parent.kind() == "value_argument"
        && parent.named_child_count() >= 2
        && parent.named_child(0) == Some(node)
}

fn has_ancestor_kind_a(ancestors: &[Node<'_>], kinds: &[&str]) -> bool {
    ancestors.iter().any(|n| kinds.contains(&n.kind()))
}

fn simple_name_status_a(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    usage: Node,
    ancestors: &[Node],
    uk: UseKind,
    facts: &CompletenessFacts,
    ctx: &infer::FileCtx,
) -> ResolutionStatus<()> {
    let name = node_text(usage, src);
    if local_decl_a(usage, ancestors, name, uk, src).is_some() {
        return ResolutionStatus::Found(());
    }
    if has_visible_cross_file(index, name, uk, &ctx.imports, &ctx.package) {
        return ResolutionStatus::Found(());
    }
    if !index.scope_is_complete() {
        return ResolutionStatus::Unknown(vec![IncompletenessReason::QueryScopeIncomplete]);
    }

    if uk == UseKind::Type && infer::is_intrinsic_type_ref(index, &TypeRef::simple(name), ctx) {
        return ResolutionStatus::Found(());
    }
    let owner = ancestors.iter().rev().find_map(|ancestor| {
        if !matches!(ancestor.kind(), "class_declaration" | "object_declaration") {
            return None;
        }
        let name = name_field(*ancestor)?;
        Some(Def {
            file: file.into(),
            start_byte: name.start_byte(),
            end_byte: name.end_byte(),
        })
    });
    let mut reasons =
        visible_package_reasons_with_ctx(file, name, facts, &ctx.package, &ctx.imports);
    if let ResolutionStatus::Unknown(unknown) = facts.compiler_surface_status(owner.as_ref()) {
        reasons.extend(unknown);
    }
    dedup_reasons(&mut reasons);
    if reasons.is_empty() {
        for import in ctx
            .imports
            .iter()
            .filter(|import| !import.wildcard && import.local_name() == Some(name))
        {
            match import_target_status(index, file, import, facts, ctx) {
                ResolutionStatus::DefinitelyAbsent => {}
                ResolutionStatus::Unknown(reasons) => return ResolutionStatus::Unknown(reasons),
                ResolutionStatus::Found(()) => {
                    return ResolutionStatus::Unknown(vec![
                        IncompletenessReason::UnknownReceiverType,
                    ]);
                }
            }
        }
        if visible_durable_candidate_with_ctx(index, name, uk, &ctx.package, &ctx.imports) {
            return ResolutionStatus::Unknown(vec![IncompletenessReason::UnknownReceiverType]);
        }
        ResolutionStatus::DefinitelyAbsent
    } else {
        ResolutionStatus::Unknown(reasons)
    }
}

fn member_name_status_a(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    usage: Node,
    parent: Option<Node>,
    facts: &CompletenessFacts,
    ctx: &infer::FileCtx,
) -> ResolutionStatus<()> {
    let name = node_text(usage, src);
    let Some(nav) = parent else {
        return ResolutionStatus::Unknown(vec![IncompletenessReason::UnknownReceiverType]);
    };
    let Some(recv) = nav.named_child(0) else {
        return ResolutionStatus::Unknown(vec![IncompletenessReason::UnknownReceiverType]);
    };
    if recv.kind() == "super_expression" {
        return if members::super_member_candidates(
            index,
            infer::super_receivers(index, recv, src, ctx),
            name,
            ctx,
        )
        .is_some()
        {
            ResolutionStatus::Found(())
        } else {
            ResolutionStatus::Unknown(vec![IncompletenessReason::UnknownReceiverType])
        };
    }
    let recv_ty = infer::member_receiver_type(recv, infer::infer(index, recv, src, ctx));
    if ctx.local_extensions.is_some() {
        let pool = members::candidates_at(index, &recv_ty, name, ctx, usage.start_byte());
        if !pool.members.is_empty()
            || pool.known_extensions().next().is_some()
            || pool.known_local_extensions().next().is_some()
        {
            return ResolutionStatus::Found(());
        }
        if pool.has_local_extensions() || pool.has_uncertain_receivers() {
            return ResolutionStatus::Unknown(vec![IncompletenessReason::UnknownReceiverType]);
        }
    }
    member_name_status_for_receiver(index, file, &recv_ty, name, facts, ctx, usage.start_byte())
}

/// Shared negative member proof after a language adapter has inferred the receiver. A known
/// declaration is useful even when the rest of the producer/hierarchy surface remains unknown.
pub fn member_name_status_for_receiver(
    index: &dyn InferenceIndex,
    file: &str,
    recv_ty: &Type,
    name: &str,
    facts: &CompletenessFacts,
    ctx: &infer::FileCtx,
    position: usize,
) -> ResolutionStatus<()> {
    let candidates = members::candidates_at(index, recv_ty, name, ctx, position);
    if !candidates.members.is_empty() || candidates.known_extensions().next().is_some() {
        return ResolutionStatus::Found(());
    }
    if candidates.has_uncertain_receivers() {
        return ResolutionStatus::Unknown(vec![IncompletenessReason::UnknownReceiverType]);
    }
    if !index.scope_is_complete() {
        return ResolutionStatus::Unknown(vec![IncompletenessReason::QueryScopeIncomplete]);
    }
    let Some(root) = recv_ty.name() else {
        return ResolutionStatus::Unknown(vec![IncompletenessReason::UnknownReceiverType]);
    };
    let Some(root_pkg) = recv_ty.package() else {
        return ResolutionStatus::Unknown(vec![
            IncompletenessReason::AmbiguousReceiverTypePackage(root.to_string()),
        ]);
    };

    if file.ends_with(".java") {
        if let Some(status) = java_binary_member_status(index, recv_ty, name, facts) {
            return status;
        }
    }

    let mut reasons = hierarchy_package_reasons(index, recv_ty, facts);
    reasons.extend(visible_package_reasons_with_ctx(
        file,
        name,
        facts,
        &ctx.package,
        &ctx.imports,
    ));
    if !facts.compiler_producers_complete
        && source_hierarchy_member_surface_complete(index, recv_ty, name, facts)
    {
        clear_certified_source_hierarchy_reasons(index, &mut reasons);
    }
    dedup_reasons(&mut reasons);
    if reasons.is_empty() {
        if receiver_world_is_library_or_jdk(index, root, root_pkg) {
            return ResolutionStatus::Unknown(vec![IncompletenessReason::UnknownReceiverType]);
        }
        if [("Object", "java.lang"), ("Any", "kotlin")]
            .iter()
            .any(|(owner, package)| {
                index
                    .members_of(owner)
                    .iter()
                    .any(|entry| entry.sym.package == *package && entry.sym.name == name)
            })
        {
            // Implicit JVM roots are not yet part of the typed supertype walk.
            return ResolutionStatus::Unknown(vec![IncompletenessReason::UnknownReceiverType]);
        }
        if receiver_is_enum_like(index, root, root_pkg) && matches!(name, "name" | "ordinal") {
            return ResolutionStatus::Unknown(vec![IncompletenessReason::UnknownReceiverType]);
        }
        if recv_ty.container().is_none() && package_has_member_named(index, root_pkg, name) {
            return ResolutionStatus::Unknown(vec![IncompletenessReason::UnknownReceiverType]);
        }
        if !index.scope_is_complete() {
            return ResolutionStatus::Unknown(vec![IncompletenessReason::QueryScopeIncomplete]);
        }
        ResolutionStatus::DefinitelyAbsent
    } else {
        ResolutionStatus::Unknown(reasons)
    }
}

/// Close only this queried member's source hierarchy, including the selected SDK's implicit root.
/// An unknown owner, missing edge, annotation producer, or binary-only blocker keeps the proof open.
pub fn source_hierarchy_member_surface_complete(
    index: &dyn InferenceIndex,
    receiver: &Type,
    name: &str,
    facts: &CompletenessFacts,
) -> bool {
    if !index.scope_is_complete() || !facts.project_scan_complete || !facts.jdk_index_complete {
        return false;
    }
    let mut complete = true;
    let _ = infer::walk_supertypes(index, receiver, 32, |current| {
        let Some(name) = current.name() else {
            complete = false;
            return ControlFlow::Break(());
        };
        let Some(package) = current.package() else {
            complete = false;
            return ControlFlow::Break(());
        };
        if name == "Object" && package == "java.lang" && current.container().is_none() {
            // The selected SDK inventory below certifies this explicit root edge.
            return ControlFlow::Continue(());
        }
        let mut owners = index.lookup_type(name).into_iter().filter(|owner| {
            owner.sym.package == package && owner.sym.container.as_deref() == current.container()
        });
        let Some(owner) = owners.next() else {
            complete = false;
            return ControlFlow::Break(());
        };
        if owners.next().is_some()
            || owner.tier != crate::index::Tier::Volatile
            || facts.member_surfaces_unknown.contains(&to_def(owner))
            || !index.source_members_complete(owner)
        {
            complete = false;
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    });
    complete
        && index.scope_is_complete()
        && java_binary_member_status(
            index,
            &Type::class("Object", Some("java.lang".into())),
            name,
            facts,
        )
        .is_some_and(|status| status.is_definitely_absent())
}

fn clear_certified_source_hierarchy_reasons(
    index: &dyn InferenceIndex,
    reasons: &mut Vec<IncompletenessReason>,
) {
    let object = binary_owner(index, "java/lang/Object").map(to_def);
    reasons.retain(|reason| match reason {
        IncompletenessReason::CompilerProducerIncomplete => false,
        IncompletenessReason::MemberSurfaceIncomplete(def) => object.as_ref() != Some(def),
        _ => true,
    });
}

fn java_binary_member_status(
    index: &dyn InferenceIndex,
    receiver: &Type,
    member: &str,
    facts: &CompletenessFacts,
) -> Option<ResolutionStatus<()>> {
    let mut root = receiver.package()?.replace('.', "/");
    if !root.is_empty() {
        root.push('/');
    }
    if let Some(container) = receiver.container() {
        root.push_str(&container.replace('.', "$"));
        root.push('$');
    }
    root.push_str(receiver.name()?);
    index.java_class_inventory(binary_owner(index, &root)?)?;

    let mut pending = vec![(root, false, 0)];
    let mut active = BTreeSet::new();
    let mut complete = BTreeSet::new();
    while let Some((internal, leaving, depth)) = pending.pop() {
        if leaving {
            active.remove(&internal);
            complete.insert(internal);
            continue;
        }
        if complete.contains(&internal) {
            continue;
        }
        let absent = (|| {
            if depth >= 32
                || complete.len() + active.len() >= 128
                || !active.insert(internal.clone())
            {
                return None;
            }
            let owner = binary_owner(index, &internal)?;
            let class = index.java_class_inventory(owner)?;
            let origin = index.origin_facts()?.file_origin(&owner.path)?;
            if class.identity.origin_id != origin.origin_id
                || class.identity.internal_name != internal
                || !matches!(
                    class.identity.input,
                    crate::jvm::InputIdentity::Binary { .. }
                )
                || facts.member_surfaces_unknown.contains(&to_def(owner))
                || (class.super_name.is_none() && internal != "java/lang/Object")
                || class
                    .fields
                    .iter()
                    .chain(&class.methods)
                    .any(|item| item.name.as_ref() == member)
            {
                return None;
            }
            pending.push((internal.clone(), true, depth));
            for parent in class.super_name.iter().chain(&class.interfaces) {
                pending.push((parent.to_string(), false, depth + 1));
            }
            Some(())
        })();
        if absent.is_none() {
            // A raw name can suppress an error without claiming source callability.
            return Some(ResolutionStatus::Unknown(vec![
                IncompletenessReason::UnknownReceiverType,
            ]));
        }
    }
    Some(if index.scope_is_complete() {
        ResolutionStatus::DefinitelyAbsent
    } else {
        ResolutionStatus::Unknown(vec![IncompletenessReason::QueryScopeIncomplete])
    })
}

fn binary_owner<'a>(index: &'a dyn InferenceIndex, internal: &str) -> Option<&'a Entry> {
    let (package, owner) = internal.rsplit_once('/').unwrap_or(("", internal));
    let package = package.replace('/', ".");
    let (container, name) = owner
        .rsplit_once('$')
        .map_or((None, owner), |(container, name)| {
            (Some(container.replace('$', ".")), name)
        });
    let entries = index.lookup_type(name);
    let mut selected = entries.into_iter().filter(|entry| {
        entry.sym.package == package && entry.sym.container.as_deref() == container.as_deref()
    });
    let owner = selected.next()?;
    selected.next().is_none().then_some(owner)
}

fn receiver_world_is_library_or_jdk(
    index: &dyn InferenceIndex,
    root: &str,
    root_pkg: &str,
) -> bool {
    if matches!(root_pkg, "java.lang")
        || root_pkg.starts_with("java.")
        || root_pkg.starts_with("javax.")
        || root_pkg.starts_with("jdk.")
        || root_pkg.starts_with("kotlin.")
        || root_pkg == "kotlin"
    {
        return true;
    }
    let matches = index
        .lookup_type(root)
        .into_iter()
        .filter(|entry| entry.sym.package == root_pkg)
        .collect::<Vec<_>>();
    !matches.is_empty()
        && matches
            .iter()
            .all(|entry| entry.tier == crate::index::Tier::Durable)
}

fn package_has_member_named(index: &dyn InferenceIndex, package: &str, name: &str) -> bool {
    index.lookup_by_name(name).iter().any(|entry| {
        entry.sym.package == package
            && entry.sym.container.is_some()
            && matches!(entry.sym.kind, SymbolKind::Property | SymbolKind::Function)
    })
}

fn receiver_is_enum_like(index: &dyn InferenceIndex, root: &str, root_pkg: &str) -> bool {
    index
        .lookup_type(root)
        .into_iter()
        .any(|entry| entry.sym.package == root_pkg && entry.sym.kind == SymbolKind::EnumClass)
}

fn visible_durable_candidate_with_ctx(
    index: &dyn InferenceIndex,
    name: &str,
    uk: UseKind,
    current_pkg: &str,
    imports: &[Import],
) -> bool {
    let candidates = index
        .lookup_by_name(name)
        .iter()
        .filter(|entry| kind_ok(uk, entry.sym.kind))
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return false;
    }
    if imports.iter().any(|imp| {
        !imp.wildcard
            && imp.local_name() == Some(name)
            && candidates.iter().any(|entry| {
                entry.tier == crate::index::Tier::Durable && import_path_matches_entry(imp, entry)
            })
    }) {
        return true;
    }
    if candidates
        .iter()
        .any(|entry| entry.tier == crate::index::Tier::Durable && entry.sym.package == current_pkg)
    {
        return true;
    }
    let star_pkgs: Vec<String> = imports
        .iter()
        .filter(|imp| imp.wildcard)
        .map(|imp| imp.package())
        .chain(DEFAULT_IMPORT_PACKAGES.iter().map(|pkg| (*pkg).to_string()))
        .collect();
    candidates.iter().any(|entry| {
        entry.tier == crate::index::Tier::Durable && star_pkgs.contains(&entry.sym.package)
    })
}

/// Resolve an explicit import's full target before certifying its absence. Import aliases do not
/// change that target, and imported members require an owner surface we may not have indexed.
pub fn import_target_status(
    index: &dyn InferenceIndex,
    file: &str,
    import: &Import,
    facts: &CompletenessFacts,
    ctx: &infer::FileCtx,
) -> ResolutionStatus<()> {
    if import.wildcard || import.path.is_empty() {
        return ResolutionStatus::Unknown(vec![IncompletenessReason::NotReferencePosition]);
    }
    let parts: Vec<String> = import.path.split('.').map(str::to_string).collect();
    if !resolve_absolute_path(index, &parts, |_| true).is_empty() {
        return ResolutionStatus::Found(());
    }
    if !index.scope_is_complete() {
        return ResolutionStatus::Unknown(vec![IncompletenessReason::QueryScopeIncomplete]);
    }
    let package = import.package();
    let mut reasons = Vec::new();
    if !facts.project_scan_complete {
        reasons.push(IncompletenessReason::ProjectPackageIncomplete(
            package.clone(),
        ));
    }
    if let ResolutionStatus::Unknown(unknown) = facts.compiler_surface_status(None) {
        reasons.extend(unknown);
    }
    package_world_reasons(&package, None, file, &ctx.package, facts, &mut reasons);
    if !facts.library_index_complete {
        reasons.push(IncompletenessReason::LibraryPackageIncomplete(package));
    }
    if !facts.jdk_index_complete {
        reasons.push(IncompletenessReason::JdkPackageIncomplete(
            "java.lang".into(),
        ));
    }
    dedup_reasons(&mut reasons);
    if !reasons.is_empty() {
        return ResolutionStatus::Unknown(reasons);
    }
    for end in (1..parts.len()).rev() {
        if !resolve_absolute_path(index, &parts[..end], |_| true).is_empty() {
            // Source declarations alone do not close compiler-generated nested/member surfaces.
            return ResolutionStatus::Unknown(vec![IncompletenessReason::UnknownReceiverType]);
        }
    }
    if !index.scope_is_complete() {
        return ResolutionStatus::Unknown(vec![IncompletenessReason::QueryScopeIncomplete]);
    }
    ResolutionStatus::DefinitelyAbsent
}

fn hierarchy_package_reasons(
    index: &dyn InferenceIndex,
    receiver: &Type,
    facts: &CompletenessFacts,
) -> Vec<IncompletenessReason> {
    let mut reasons = Vec::new();
    if let ResolutionStatus::Unknown(unknown) = facts.compiler_surface_status(None) {
        reasons.extend(unknown);
    }
    let root_pkg = receiver.package().unwrap_or_default();
    let _ = infer::walk_supertypes(index, receiver, 32, |current| {
        let Some(name) = current.name() else {
            reasons.push(IncompletenessReason::UnknownReceiverType);
            return ControlFlow::Continue(());
        };
        let Some(pkg) = current.package() else {
            reasons.push(IncompletenessReason::AmbiguousReceiverTypePackage(
                name.to_string(),
            ));
            return ControlFlow::Continue(());
        };
        let entries: Vec<&Entry> = index
            .lookup_type(name)
            .into_iter()
            .filter(|e| e.sym.package == pkg && e.sym.container.as_deref() == current.container())
            .collect();
        if entries.is_empty() {
            reasons.push(IncompletenessReason::ReceiverTypeNotIndexed(format!(
                "{pkg}.{}",
                infer::receiver_root_key(index, current).unwrap_or_else(|| name.to_string())
            )));
            return ControlFlow::Continue(());
        }
        for entry in &entries {
            if entry.tier == crate::index::Tier::Durable {
                // Source-only dependency ancestors do not close compiler-generated members.
                reasons.push(IncompletenessReason::MemberSurfaceIncomplete(to_def(entry)));
            }
            if let ResolutionStatus::Unknown(unknown) =
                facts.compiler_surface_status(Some(&to_def(entry)))
            {
                reasons.extend(unknown);
            }
            package_world_reasons(
                pkg,
                Some(*entry),
                entry.path.as_ref(),
                root_pkg,
                facts,
                &mut reasons,
            );
        }
        ControlFlow::Continue(())
    });
    dedup_reasons(&mut reasons);
    reasons
}

/// A callable's absence of further overloads also depends on its selected declaration owner.
pub fn declaration_surface_status(
    index: &dyn InferenceIndex,
    entry: &Entry,
    facts: &CompletenessFacts,
) -> ResolutionStatus<()> {
    if !index.scope_is_complete() {
        return ResolutionStatus::Unknown(vec![IncompletenessReason::QueryScopeIncomplete]);
    }
    if entry.sym.kind.is_type_like() {
        return facts.compiler_surface_status(Some(&to_def(entry)));
    }
    let Some(container) = entry.sym.container.as_deref() else {
        return facts.compiler_surface_status(None);
    };
    let (parent, name) = container
        .rsplit_once('.')
        .map_or((None, container), |(parent, name)| (Some(parent), name));
    let owners: Vec<_> = index
        .lookup_type(name)
        .into_iter()
        .filter(|owner| {
            owner.sym.package == entry.sym.package && owner.sym.container.as_deref() == parent
        })
        .collect();
    if owners.is_empty() {
        return ResolutionStatus::Unknown(vec![IncompletenessReason::ReceiverTypeNotIndexed(
            container.into(),
        )]);
    }
    let unique_owner = owners.len() == 1;
    for owner in owners {
        if let ResolutionStatus::Unknown(mut reasons) =
            facts.compiler_surface_status(Some(&to_def(owner)))
        {
            if !unique_owner
                || owner.path != entry.path
                || !reasons
                    .iter()
                    .all(|reason| *reason == IncompletenessReason::CompilerProducerIncomplete)
                || !source_hierarchy_member_surface_complete(
                    index,
                    &Type::class(&owner.sym.name, Some(owner.sym.package.clone())),
                    &entry.sym.name,
                    facts,
                )
            {
                return ResolutionStatus::Unknown(reasons);
            }
            reasons.extend(hierarchy_package_reasons(
                index,
                &Type::class(&owner.sym.name, Some(owner.sym.package.clone())),
                facts,
            ));
            for package in [owner.sym.package.as_str(), "java.lang"] {
                package_world_reasons(
                    package,
                    (package == owner.sym.package).then_some(owner),
                    &owner.path,
                    &owner.sym.package,
                    facts,
                    &mut reasons,
                );
            }
            // Only the certified source hierarchy and exact SDK Object surface are closed.
            clear_certified_source_hierarchy_reasons(index, &mut reasons);
            dedup_reasons(&mut reasons);
            if !reasons.is_empty() {
                return ResolutionStatus::Unknown(reasons);
            }
        }
    }
    if !index.scope_is_complete() {
        return ResolutionStatus::Unknown(vec![IncompletenessReason::QueryScopeIncomplete]);
    }
    ResolutionStatus::Found(())
}

fn visible_package_reasons(
    file: &str,
    tree: &Tree,
    src: &str,
    local_name: &str,
    facts: &CompletenessFacts,
) -> Vec<IncompletenessReason> {
    let imports = imports_of(tree, src);
    let current_pkg = package_of(tree, src);
    visible_package_reasons_with_ctx(file, local_name, facts, &current_pkg, &imports)
}

fn visible_package_reasons_with_ctx(
    file: &str,
    local_name: &str,
    facts: &CompletenessFacts,
    current_pkg: &str,
    imports: &[Import],
) -> Vec<IncompletenessReason> {
    let mut reasons = Vec::new();
    if let ResolutionStatus::Unknown(unknown) = facts.compiler_surface_status(None) {
        reasons.extend(unknown);
    }
    let mut visible_pkgs = Vec::new();
    push_visible_package(&mut visible_pkgs, current_pkg.to_string());
    for imp in imports {
        if !imp.wildcard && imp.local_name() == Some(local_name) {
            push_visible_package(&mut visible_pkgs, imp.package());
        }
    }
    for imp in imports {
        if imp.wildcard {
            push_visible_package(&mut visible_pkgs, imp.package());
        }
    }
    for pkg in DEFAULT_IMPORT_PACKAGES {
        push_visible_package(&mut visible_pkgs, (*pkg).to_string());
    }
    for pkg in visible_pkgs {
        package_world_reasons(&pkg, None, file, current_pkg, facts, &mut reasons);
    }
    dedup_reasons(&mut reasons);
    reasons
}

pub fn top_level_call_completeness_reasons(
    file: &str,
    tree: &Tree,
    src: &str,
    local_name: &str,
    facts: &CompletenessFacts,
) -> Vec<IncompletenessReason> {
    visible_package_reasons(file, tree, src, local_name, facts)
}

pub fn top_level_call_completeness_reasons_with_ctx(
    file: &str,
    local_name: &str,
    facts: &CompletenessFacts,
    ctx: &infer::FileCtx,
) -> Vec<IncompletenessReason> {
    visible_package_reasons_with_ctx(file, local_name, facts, &ctx.package, &ctx.imports)
}

pub fn member_call_completeness_reasons(
    index: &dyn InferenceIndex,
    file: &str,
    tree: &Tree,
    src: &str,
    usage: Node,
    facts: &CompletenessFacts,
) -> Vec<IncompletenessReason> {
    let name = node_text(usage, src);
    let Some(nav) = usage.parent() else {
        return vec![IncompletenessReason::UnknownReceiverType];
    };
    let Some(recv) = nav.named_child(0) else {
        return vec![IncompletenessReason::UnknownReceiverType];
    };
    let ctx = infer::FileCtx::from_tree_in_file(tree, src, file);
    let recv_ty = infer::infer(index, recv, src, &ctx);
    let Some(root) = recv_ty.name() else {
        return vec![IncompletenessReason::UnknownReceiverType];
    };
    let Some(_) = recv_ty.package() else {
        return vec![IncompletenessReason::AmbiguousReceiverTypePackage(
            root.to_string(),
        )];
    };

    let mut reasons = hierarchy_package_reasons(index, &recv_ty, facts);
    reasons.extend(visible_package_reasons(file, tree, src, name, facts));
    if source_hierarchy_member_surface_complete(index, &recv_ty, name, facts) {
        clear_certified_source_hierarchy_reasons(index, &mut reasons);
    }
    dedup_reasons(&mut reasons);
    reasons
}

pub fn member_call_completeness_reasons_with_ctx(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    usage: Node,
    facts: &CompletenessFacts,
    ctx: &infer::FileCtx,
) -> Vec<IncompletenessReason> {
    member_call_completeness_reasons_in(
        index,
        file,
        src,
        node_text(usage, src),
        usage.parent(),
        facts,
        ctx,
    )
}

/// [`member_call_completeness_reasons_with_ctx`] with the member name and its
/// navigation-expression parent supplied by the caller.
pub(crate) fn member_call_completeness_reasons_in(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    name: &str,
    nav: Option<Node>,
    facts: &CompletenessFacts,
    ctx: &infer::FileCtx,
) -> Vec<IncompletenessReason> {
    let Some(nav) = nav else {
        return vec![IncompletenessReason::UnknownReceiverType];
    };
    let Some(recv) = nav.named_child(0) else {
        return vec![IncompletenessReason::UnknownReceiverType];
    };
    let recv_ty = infer::infer(index, recv, src, ctx);
    let Some(root) = recv_ty.name() else {
        return vec![IncompletenessReason::UnknownReceiverType];
    };
    let Some(_) = recv_ty.package() else {
        return vec![IncompletenessReason::AmbiguousReceiverTypePackage(
            root.to_string(),
        )];
    };

    let mut reasons = hierarchy_package_reasons(index, &recv_ty, facts);
    reasons.extend(visible_package_reasons_with_ctx(
        file,
        name,
        facts,
        &ctx.package,
        &ctx.imports,
    ));
    if source_hierarchy_member_surface_complete(index, &recv_ty, name, facts) {
        clear_certified_source_hierarchy_reasons(index, &mut reasons);
    }
    dedup_reasons(&mut reasons);
    reasons
}

fn push_visible_package(out: &mut Vec<String>, package: String) {
    if !out.iter().any(|p| p == &package) {
        out.push(package);
    }
}

fn package_world_reasons(
    package: &str,
    entry: Option<&Entry>,
    current_file: &str,
    current_file_pkg: &str,
    facts: &CompletenessFacts,
    out: &mut Vec<IncompletenessReason>,
) {
    let from_project = entry
        .map(|e| e.tier == crate::index::Tier::Volatile)
        .unwrap_or(package == current_file_pkg);
    let from_jdk = package == "java.lang"
        || package.starts_with("java.")
        || package.starts_with("javax.")
        || package.starts_with("jdk.");
    let from_library = entry
        .map(|e| e.tier == crate::index::Tier::Durable && !from_jdk)
        .unwrap_or(!from_project && !from_jdk);

    if from_project && !facts.project_scan_complete {
        if entry.is_none() && package == current_file_pkg {
            if let Some(reasons) = source_set_package_reasons(package, current_file, facts) {
                out.extend(reasons);
                return;
            }
        }
        if !facts.project_packages_complete.contains(package) {
            out.push(IncompletenessReason::ProjectPackageIncomplete(
                package.to_string(),
            ));
        }
    }
    if from_library && !facts.library_index_complete {
        out.push(IncompletenessReason::LibraryPackageIncomplete(
            package.to_string(),
        ));
    }
    if from_jdk && !facts.jdk_index_complete {
        out.push(IncompletenessReason::JdkPackageIncomplete(
            package.to_string(),
        ));
    }
}

fn source_set_package_reasons(
    package: &str,
    current_file: &str,
    facts: &CompletenessFacts,
) -> Option<Vec<IncompletenessReason>> {
    let current_scope = project_model::project_scope_for_path(current_file)?;
    if facts.project_packages_with_unknown_scope.contains(package) {
        return None;
    }
    let package_modules = facts.project_package_modules.get(package)?;
    if package_modules.len() != 1 || !package_modules.contains(&current_scope.module) {
        return None;
    }
    let missing = project_model::related_package_scopes(&current_scope, package)
        .into_iter()
        .filter(|scope| !facts.project_scoped_packages_complete.contains(scope))
        .map(IncompletenessReason::ProjectSourceSetPackageIncomplete)
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Some(Vec::new())
    } else {
        Some(missing)
    }
}

fn dedup_reasons(reasons: &mut Vec<IncompletenessReason>) {
    let mut seen = std::collections::HashSet::new();
    reasons.retain(|reason| seen.insert(reason.label()));
}

fn kind_ok(uk: UseKind, kind: SymbolKind) -> bool {
    match uk {
        UseKind::Type => kind.is_type_like(),
        UseKind::Call => kind.is_callable_like(),
        UseKind::Value => kind.is_value_like(),
        UseKind::MemberSelector => true,
    }
}

/// Resolve goto-definition for the identifier at `offset` in `file` (whose parsed `tree`/`src` are
/// given). `file` is the canonical key used for results in the current file.
pub fn goto(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    offset: usize,
) -> Vec<Def> {
    let ctx = infer::FileCtx::from_tree_in_file(tree, src, file);
    goto_with_ctx(index, file, src, tree, offset, &ctx)
}

pub fn goto_with_ctx(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    offset: usize,
    ctx: &infer::FileCtx,
) -> Vec<Def> {
    if let Some(binding) = index.semantic_binding_at(file, offset) {
        return vec![index.semantic_definition(binding.target.definition())];
    }
    let ident = match identifier_at(tree, offset) {
        Some(n) => n,
        None => return Vec::new(),
    };
    let ancestors = crate::parser::ancestors_of(ident);
    goto_node_with_ctx(index, file, src, ident, &ancestors, ctx)
}

/// [`goto_with_ctx`] with the identifier node and its ancestor chain (root-first) supplied by
/// the caller, skipping both the `identifier_at` root descent and per-helper parent lookups.
pub(crate) fn goto_node_with_ctx(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    ident: Node,
    ancestors: &[Node],
    ctx: &infer::FileCtx,
) -> Vec<Def> {
    goto_authored_node_with_ctx(index, file, src, ident, ancestors, ctx)
        .into_iter()
        .map(|definition| index.semantic_definition(&definition))
        .collect()
}

pub fn goto_authored(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    tree: &Tree,
    offset: usize,
) -> Vec<Def> {
    let Some(ident) = identifier_at(tree, offset) else {
        return Vec::new();
    };
    let ctx = infer::FileCtx::from_tree_in_file(tree, src, file);
    goto_authored_node_with_ctx(
        index,
        file,
        src,
        ident,
        &crate::parser::ancestors_of(ident),
        &ctx,
    )
}

fn goto_authored_node_with_ctx(
    index: &dyn InferenceIndex,
    file: &str,
    src: &str,
    ident: Node,
    ancestors: &[Node],
    ctx: &infer::FileCtx,
) -> Vec<Def> {
    let name = node_text(ident, src);
    if name.is_empty() {
        return Vec::new();
    }
    let parent = ancestors.last().copied();
    let uk = use_kind_in(ident, parent);
    let named_usage = call_for_use(ident)
        .and_then(|call| infer::named_call_arguments(index, call, src, ctx))
        .map(MemberUse::Call);

    // Import declarations are neither value nor type positions. Resolve the imported target (or
    // alias) directly by its qualified path.
    if let Some(defs) = resolve_import_target_a(index, ident, ancestors, src) {
        return defs;
    }

    // Fully-qualified top-level names (`pkg.Type`, `pkg.func()`) bypass imports. Handle them before
    // the member path so a package-qualified selector is not mistaken for `receiver.member`.
    if let Some(defs) = resolve_qualified_a(
        index,
        ident,
        ancestors,
        name,
        uk,
        src,
        named_usage.as_ref(),
        ctx,
    ) {
        if !defs.is_empty() || named_usage.is_some() {
            return defs;
        }
    }

    // Member access: infer the receiver's type and filter members by it (S6), else best-effort.
    if uk == UseKind::MemberSelector {
        return resolve_member_with_ctx(index, ident, parent, name, src, ctx, named_usage.as_ref());
    }
    if let Some(def) = definition_self_in(ident, parent, file) {
        return vec![def];
    }
    if let Some(def) = resolve_local_a(ident, ancestors, name, uk, src, file) {
        if ctx.local_entry(&def).is_some() && uk == UseKind::Call {
            return resolve_implicit_receiver_member_with_ctx_a(
                index,
                ident,
                ancestors,
                name,
                src,
                ctx,
                named_usage.as_ref(),
            )
            .unwrap_or_default();
        }
        // Class members are also found by the lexical walk. Select the same overload/property
        // partition as an explicit receiver instead of committing to the first declaration.
        if index.entries_for_file(file).iter().any(|entry| {
            entry.sym.start_byte == def.start_byte
                && entry.sym.end_byte == def.end_byte
                && matches!(entry.sym.kind, SymbolKind::Function | SymbolKind::Property)
                && (entry.sym.container.is_some() || uk == UseKind::Call)
        }) {
            if let Some(defs) = resolve_implicit_receiver_member_with_ctx_a(
                index,
                ident,
                ancestors,
                name,
                src,
                ctx,
                named_usage.as_ref(),
            ) {
                return defs;
            }
        }
        if let Some(usage) = &named_usage {
            let entries = index.lookup_by_name(name);
            if entries.iter().any(|entry| {
                entry.path.as_ref() == file
                    && entry.sym.start_byte == def.start_byte
                    && entry.sym.end_byte == def.end_byte
                    && entry.sym.kind == SymbolKind::Function
                    && entry.sym.container.is_none()
                    && entry.sym.ext_receiver.is_none()
            }) {
                let group = entries
                    .iter()
                    .filter(|entry| {
                        entry.path.as_ref() == file
                            && entry.sym.kind == SymbolKind::Function
                            && entry.sym.container.is_none()
                            && entry.sym.ext_receiver.is_none()
                    })
                    .collect();
                return call_entries_to_defs(index, group, Some(usage), ctx);
            }
        }
        return vec![def];
    }
    if let Some(defs) = resolve_implicit_receiver_member_with_ctx_a(
        index,
        ident,
        ancestors,
        name,
        src,
        ctx,
        named_usage.as_ref(),
    ) {
        // An uncertain implicit receiver is not permission to select unrelated global homonyms.
        return defs;
    }
    if let Some(defs) = resolve_nested_type_in(index, src, ident, parent, ctx) {
        if !defs.is_empty() {
            return defs;
        }
    }
    resolve_cross_file(index, name, uk, ctx, named_usage.as_ref())
}

/// If the cursor is on the defining identifier of a declaration, return its own location.
fn definition_self_in(usage: Node, parent: Option<Node>, file: &str) -> Option<Def> {
    let parent = parent?;
    let is_self = match parent.kind() {
        "class_declaration" | "object_declaration" | "function_declaration" => {
            name_field(parent) == Some(usage)
        }
        "type_alias" => parent.child_by_field_name("type") == Some(usage),
        "variable_declaration"
        | "parameter"
        | "class_parameter"
        | "type_parameter"
        | "enum_entry" => first_ident(parent) == Some(usage),
        _ => false,
    };
    is_self.then(|| def_here(file, usage))
}

fn def_here(file: &str, node: Node) -> Def {
    Def {
        file: file.to_string(),
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
    }
}

// ---------------------------------------------------------------------------------------------
// Local resolution
// ---------------------------------------------------------------------------------------------

fn resolve_local_a(
    usage: Node,
    ancestors: &[Node],
    name: &str,
    uk: UseKind,
    src: &str,
    file: &str,
) -> Option<Def> {
    local_decl_a(usage, ancestors, name, uk, src).map(|(node, _kind)| def_here(file, node))
}

fn resolve_implicit_receiver_member_with_ctx_a(
    index: &dyn InferenceIndex,
    usage: Node,
    ancestors: &[Node],
    name: &str,
    src: &str,
    ctx: &infer::FileCtx,
    normalized: Option<&MemberUse>,
) -> Option<Vec<Def>> {
    let member_use = normalized
        .map(std::borrow::Cow::Borrowed)
        .unwrap_or_else(|| std::borrow::Cow::Owned(member_use(index, usage, src, ctx)));
    let receivers = infer::implicit_receiver_types_with_ancestors(index, ancestors, src, &ctx, 0);
    crate::members::select_implicit_at(index, receivers, name, &member_use, ctx, usage.start_byte())
        .map(|(_, selection)| entries_to_defs(selection.entries))
}

/// Walk ancestor scopes from `usage` and return the nearest declaration's name node + kind.
/// `pub(crate)` so inference can resolve an identifier to its local/param declaration.
///
/// One-shot callers use this direct parent walk (no allocation); batch traversals that already
/// hold the ancestor chain use [`local_decl_a`] instead.
pub(crate) fn local_decl<'t>(
    usage: Node<'t>,
    name: &str,
    uk: UseKind,
    src: &str,
) -> Option<(Node<'t>, SymbolKind)> {
    let mut scope = usage.parent();
    while let Some(s) = scope {
        if let Some(hit) = decl_in_scope(s, name, usage, uk, src) {
            return Some(hit);
        }
        scope = s.parent();
    }
    None
}

/// [`local_decl`] over a caller-supplied ancestor chain (root-first): the walk order —
/// innermost scope first — is exactly the parent-chain order of the node-based variant.
pub(crate) fn local_decl_a<'t>(
    usage: Node<'t>,
    ancestors: &[Node<'t>],
    name: &str,
    uk: UseKind,
    src: &str,
) -> Option<(Node<'t>, SymbolKind)> {
    for scope in ancestors.iter().rev() {
        if let Some(hit) = decl_in_scope(*scope, name, usage, uk, src) {
            return Some(hit);
        }
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Member resolution (S6): type-directed where the receiver's type is inferable, else unique-only.
// ---------------------------------------------------------------------------------------------

fn resolve_member_with_ctx(
    index: &dyn InferenceIndex,
    selector: Node,
    parent: Option<Node>,
    name: &str,
    src: &str,
    ctx: &infer::FileCtx,
    normalized: Option<&MemberUse>,
) -> Vec<Def> {
    // `selector` is the `.member` identifier; its parent is the navigation_expression.
    if let Some(nav) = parent {
        if nav.kind() == "navigation_expression" {
            if let Some(receiver) = nav.named_child(0) {
                if receiver.kind() == "super_expression" {
                    let computed = normalized
                        .is_none()
                        .then(|| member_use(index, selector, src, ctx));
                    return members::super_member_candidates(
                        index,
                        infer::super_receivers(index, receiver, src, ctx),
                        name,
                        ctx,
                    )
                    .map(|(ty, candidates)| {
                        entries_to_defs(
                            candidates
                                .select(
                                    index,
                                    &ty,
                                    normalized.unwrap_or_else(|| computed.as_ref().unwrap()),
                                    ctx,
                                )
                                .entries,
                        )
                    })
                    .unwrap_or_default();
                }
                // Infer the receiver's type (package-qualified) and resolve the member on it —
                // own members, inherited (supertype walk), and extensions; package-filtered so a
                // same-named type in another package can't be picked.
                let ty =
                    infer::member_receiver_type(receiver, infer::infer(index, receiver, src, &ctx));
                if ty.is_member_receiver() {
                    let computed = normalized
                        .is_none()
                        .then(|| member_use(index, selector, src, ctx));
                    return member_defs_for_use_at(
                        index,
                        &ty,
                        name,
                        normalized.unwrap_or_else(|| computed.as_ref().unwrap()),
                        ctx,
                        selector.start_byte(),
                    );
                }
                // A written receiver does not belong to a globally unique, unrelated member.
                return Vec::new();
            }
        }
    }
    // Fallback: commit only if the member name is globally unique (best-effort, no type info).
    let candidates = index.lookup_by_name(name);
    if index.permits_unique_member_fallback() && candidates.len() == 1 {
        vec![to_def(&candidates[0])]
    } else {
        Vec::new()
    }
}

/// Normalize whether a Kotlin selector/implicit member is used as a value or as a call's callee.
/// This is independent of the receiver route (`UseKind::MemberSelector`).
pub fn member_use(
    index: &dyn InferenceIndex,
    usage: Node,
    src: &str,
    ctx: &infer::FileCtx,
) -> MemberUse {
    if let Some(call) = call_for_use(usage) {
        return MemberUse::Call(infer::call_arguments(index, call, src, ctx));
    }
    let callee = callee_for_use(usage);
    // With an unfinished argument list the parser can keep a navigation expression and place
    // `(` in an ERROR sibling. Preserve the call's possible candidates without inventing args.
    if src
        .get(callee.end_byte()..)
        .is_some_and(|suffix| suffix.trim_start_matches([' ', '\t']).starts_with('('))
    {
        return MemberUse::Call(crate::members::CallArguments {
            count: None,
            types: Vec::new(),
            names: None,
            has_spread: false,
            trailing_lambda: false,
        });
    }
    MemberUse::Value
}

fn callee_for_use(usage: Node) -> Node {
    usage
        .parent()
        .filter(|parent| {
            parent.kind() == "navigation_expression" && parent.named_child(1) == Some(usage)
        })
        .unwrap_or(usage)
}

fn call_for_use(usage: Node) -> Option<Node> {
    let callee = callee_for_use(usage);
    let mut call = callee.parent().filter(|parent| {
        parent.kind() == "call_expression" && parent.named_child(0) == Some(callee)
    })?;
    while let Some(outer) = call.parent().filter(|parent| {
        parent.kind() == "call_expression"
            && parent.named_child(0) == Some(call)
            && infer::has_trailing_lambda(*parent)
    }) {
        call = outer;
    }
    Some(call)
}

/// Resolve the shared typed member candidates for a normalized call/value use.
pub fn member_defs_for_use(
    index: &dyn InferenceIndex,
    ty: &Type,
    name: &str,
    usage: &MemberUse,
    ctx: &infer::FileCtx,
) -> Vec<Def> {
    let candidates = if matches!(usage, MemberUse::Value) {
        members::value_candidates(index, ty, name, ctx, None)
    } else {
        members::candidates(index, ty, name, ctx)
    };
    entries_to_defs(candidates.select(index, ty, usage, ctx).entries)
}

pub fn member_defs_for_use_at(
    index: &dyn InferenceIndex,
    ty: &Type,
    name: &str,
    usage: &MemberUse,
    ctx: &infer::FileCtx,
    position: usize,
) -> Vec<Def> {
    let candidates = if matches!(usage, MemberUse::Value) {
        members::value_candidates(index, ty, name, ctx, Some(position))
    } else {
        members::candidates_at(index, ty, name, ctx, position)
    };
    entries_to_defs(candidates.select(index, ty, usage, ctx).entries)
}

/// Generic-receiver extensions visible under the written canonical or aliased call-site name.
pub fn generic_receiver_extensions<'a>(
    index: &'a dyn InferenceIndex,
    name: &str,
    imports: &[Import],
    current_pkg: &str,
) -> Vec<&'a Entry> {
    std::iter::once(name)
        .chain(
            imports
                .iter()
                .filter(|imp| imp.alias.is_some() && imp.local_name() == Some(name))
                .map(Import::simple_name)
                .filter(|real_name| *real_name != name),
        )
        .flat_map(|lookup_name| index.lookup_by_name(lookup_name))
        .filter(|e| crate::index::is_generic_receiver_extension(e))
        .filter(|e| visible_extension_names(e, imports, current_pkg).any(|visible| visible == name))
        .collect()
}

/// Call-site names for an extension, preserving its authored declaration and definition range.
/// An alias hides the canonical name from package/wildcard visibility, but an explicit unaliased
/// import restores it. Aliases follow the canonical name in import order.
pub fn visible_extension_names<'a>(
    entry: &'a Entry,
    imports: &'a [Import],
    current_pkg: &str,
) -> impl Iterator<Item = &'a str> {
    let matching_imports = imports
        .iter()
        .filter(move |imp| import_path_matches_entry(imp, entry));
    let canonical = if matching_imports.clone().any(|imp| imp.alias.is_none()) {
        true
    } else if matching_imports.clone().any(|imp| imp.alias.is_some()) {
        false
    } else {
        extension_is_visible(entry, imports, current_pkg)
    };
    canonical
        .then_some(entry.sym.name.as_str())
        .into_iter()
        .chain(matching_imports.filter_map(|imp| imp.alias.as_deref()))
}

pub(crate) fn extension_is_visible(e: &Entry, imports: &[Import], current_pkg: &str) -> bool {
    if e.sym.package == current_pkg {
        return true;
    }
    if imports
        .iter()
        .any(|imp| !imp.wildcard && imp.alias.is_none() && import_path_matches_entry(imp, e))
    {
        return true;
    }
    imports
        .iter()
        .filter(|imp| imp.wildcard)
        .map(|imp| imp.package())
        .chain(DEFAULT_IMPORT_PACKAGES.iter().map(|pkg| (*pkg).to_string()))
        .any(|pkg| pkg == e.sym.package)
}

/// The name of the class/object enclosing `node`. `pub(crate)` so inference can type `this`.
/// Direct parent walk: one-shot callers avoid the ancestor-Vec allocation of the batch path.
pub(crate) fn enclosing_type_name(node: Node, src: &str) -> Option<String> {
    let mut cur = node.parent();
    while let Some(n) = cur {
        if matches!(n.kind(), "class_declaration" | "object_declaration") {
            return name_field(n).map(|nn| node_text(nn, src).to_string());
        }
        cur = n.parent();
    }
    None
}

fn decl_in_scope<'t>(
    scope: Node<'t>,
    name: &str,
    usage: Node<'t>,
    uk: UseKind,
    src: &str,
) -> Option<(Node<'t>, SymbolKind)> {
    match scope.kind() {
        // Blocks and lambda bodies hold ordered locals (must precede the use).
        "block" | "lambda_literal" => scan_block(scope, name, usage, src),
        "function_declaration" | "secondary_constructor" => {
            if let Some(params) = child_of_kind(scope, "function_value_parameters") {
                if let Some(n) = scan_params(params, "parameter", name, src) {
                    return Some((n, SymbolKind::Parameter));
                }
            }
            if let Some(tp) = child_of_kind(scope, "type_parameters") {
                if let Some(n) = scan_params(tp, "type_parameter", name, src) {
                    return Some((n, SymbolKind::TypeParameter));
                }
            }
            None
        }
        "class_declaration" => {
            if let Some(pc) = child_of_kind(scope, "primary_constructor") {
                if let Some(cp) = child_of_kind(pc, "class_parameters") {
                    if let Some(n) = scan_params(cp, "class_parameter", name, src) {
                        return Some((n, SymbolKind::Parameter));
                    }
                }
            }
            if let Some(tp) = child_of_kind(scope, "type_parameters") {
                if let Some(n) = scan_params(tp, "type_parameter", name, src) {
                    return Some((n, SymbolKind::TypeParameter));
                }
            }
            None
        }
        "class_body" | "enum_class_body" | "source_file" => scan_members(scope, name, uk, src),
        "for_statement" => scan_var_decls(scope, name, src).map(|n| (n, SymbolKind::LocalVariable)),
        "when_expression" => child_of_kind(scope, "when_subject")
            .and_then(|ws| scan_var_decls(ws, name, src))
            .map(|n| (n, SymbolKind::LocalVariable)),
        _ => None,
    }
}

/// Locals in a block/lambda body that are declared *before* the usage; nearest one wins.
fn scan_block<'t>(
    scope: Node<'t>,
    name: &str,
    usage: Node<'t>,
    src: &str,
) -> Option<(Node<'t>, SymbolKind)> {
    let u = usage.start_byte();
    let mut cands: Vec<(Node<'t>, SymbolKind)> = Vec::new();
    let mut cursor = scope.walk();
    for st in scope.named_children(&mut cursor) {
        match st.kind() {
            // A block local cannot shadow an outer callable inside its own initializer.
            "property_declaration" if st.end_byte() <= u => {
                collect_var_names(st, name, SymbolKind::LocalVariable, src, &mut cands)
            }
            "lambda_parameters" => {
                collect_var_names(st, name, SymbolKind::Parameter, src, &mut cands)
            }
            "function_declaration" => {
                if let Some(nn) = name_field(st) {
                    if node_text(nn, src) == name {
                        cands.push((nn, SymbolKind::Function));
                    }
                }
            }
            "class_declaration" => {
                if let Some(nn) = name_field(st) {
                    if node_text(nn, src) == name {
                        cands.push((nn, class_kind(st)));
                    }
                }
            }
            "object_declaration" => {
                if let Some(nn) = name_field(st) {
                    if node_text(nn, src) == name {
                        cands.push((nn, SymbolKind::Object));
                    }
                }
            }
            _ => {}
        }
    }
    cands
        .into_iter()
        .filter(|(n, _)| n.start_byte() < u)
        .max_by_key(|(n, _)| n.start_byte())
}

/// Members of a class body / file top-level matching `name` and compatible with the use kind.
/// Recurses into companion objects so companion members resolve for unqualified use.
fn scan_members<'t>(
    scope: Node<'t>,
    name: &str,
    uk: UseKind,
    src: &str,
) -> Option<(Node<'t>, SymbolKind)> {
    let mut cursor = scope.walk();
    for m in scope.named_children(&mut cursor) {
        let hit: Option<(Node<'t>, SymbolKind)> = match m.kind() {
            "function_declaration" => named(m, name, SymbolKind::Function, src),
            "class_declaration" => named(m, name, class_kind(m), src),
            "object_declaration" => named(m, name, SymbolKind::Object, src),
            "property_declaration" => {
                let mut v = Vec::new();
                collect_var_names(m, name, SymbolKind::Property, src, &mut v);
                v.into_iter().next()
            }
            "enum_entry" => first_ident(m)
                .filter(|id| node_text(*id, src) == name)
                .map(|id| (id, SymbolKind::EnumEntry)),
            "companion_object" => {
                child_of_kind(m, "class_body").and_then(|b| scan_members(b, name, uk, src))
            }
            _ => None,
        };
        if let Some((node, kind)) = hit {
            if kind_ok(uk, kind) {
                return Some((node, kind));
            }
        }
    }
    None
}

fn named<'t>(
    decl: Node<'t>,
    name: &str,
    kind: SymbolKind,
    src: &str,
) -> Option<(Node<'t>, SymbolKind)> {
    name_field(decl)
        .filter(|nn| node_text(*nn, src) == name)
        .map(|nn| (nn, kind))
}

/// Match `name` against the first identifier of each `child_kind` child of `parent`.
fn scan_params<'t>(parent: Node<'t>, child_kind: &str, name: &str, src: &str) -> Option<Node<'t>> {
    let mut cursor = parent.walk();
    for p in parent.named_children(&mut cursor) {
        if p.kind() == child_kind {
            if let Some(id) = first_ident(p) {
                if node_text(id, src) == name {
                    return Some(id);
                }
            }
        }
    }
    None
}

/// First `variable_declaration` (direct child) whose name matches — used for `for`/`when` binders.
fn scan_var_decls<'t>(parent: Node<'t>, name: &str, src: &str) -> Option<Node<'t>> {
    let mut cursor = parent.walk();
    for vd in parent.named_children(&mut cursor) {
        if vd.kind() == "variable_declaration" {
            if let Some(id) = first_ident(vd) {
                if node_text(id, src) == name {
                    return Some(id);
                }
            }
        }
    }
    None
}

/// Collect matching identifiers from a node holding `variable_declaration` /
/// `multi_variable_declaration` children (property declarations, lambda params).
fn collect_var_names<'t>(
    node: Node<'t>,
    name: &str,
    kind: SymbolKind,
    src: &str,
    out: &mut Vec<(Node<'t>, SymbolKind)>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "variable_declaration" => {
                if let Some(id) = first_ident(child) {
                    if node_text(id, src) == name {
                        out.push((id, kind));
                    }
                }
            }
            "multi_variable_declaration" => {
                let mut c2 = child.walk();
                for vd in child.named_children(&mut c2) {
                    if vd.kind() == "variable_declaration" {
                        if let Some(id) = first_ident(vd) {
                            if node_text(id, src) == name {
                                out.push((id, kind));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Cross-file resolution
// ---------------------------------------------------------------------------------------------

fn to_def(e: &Entry) -> Def {
    Def {
        file: e.path.to_string(),
        start_byte: e.sym.start_byte,
        end_byte: e.sym.end_byte,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourceSetSpecificity {
    Generic,
    Specific,
}

/// Named free calls share member applicability. Unknown competing declarations remain possible,
/// but navigation must not turn their written/index order into a selected overload.
fn call_entries_to_defs(
    index: &dyn InferenceIndex,
    entries: Vec<&Entry>,
    usage: Option<&MemberUse>,
    ctx: &infer::FileCtx,
) -> Vec<Def> {
    if let Some(usage) = usage {
        if entries
            .iter()
            .any(|entry| entry.sym.kind == SymbolKind::Function)
        {
            if entries.iter().any(|entry| {
                entry.sym.kind != SymbolKind::Function || entry.sym.ext_receiver.is_some()
            }) {
                return Vec::new();
            }
            let selection = members::select_functions(index, entries, usage, ctx);
            if selection.applicability == members::Applicability::Unknown
                && selection.entries.len() != 1
            {
                return Vec::new();
            }
            let defs = entries_to_defs(selection.entries);
            return if defs.len() == 1 { defs } else { Vec::new() };
        }
    }
    entries_to_defs(entries)
}

fn entries_to_defs(entries: Vec<&Entry>) -> Vec<Def> {
    narrow_source_set_entries(entries)
        .into_iter()
        .map(to_def)
        .collect()
}

pub(crate) fn narrow_source_set_entries(entries: Vec<&Entry>) -> Vec<&Entry> {
    if entries.len() <= 1 {
        return entries;
    }

    let mut classified = Vec::with_capacity(entries.len());
    for entry in &entries {
        let Some(kind) = source_set_specificity(&entry.path) else {
            return entries;
        };
        classified.push((*entry, kind));
    }

    if classified
        .iter()
        .any(|(_, kind)| *kind == SourceSetSpecificity::Specific)
    {
        classified
            .into_iter()
            .filter(|(_, kind)| *kind == SourceSetSpecificity::Specific)
            .map(|(entry, _)| entry)
            .collect()
    } else {
        entries
    }
}

fn source_set_specificity(path: &str) -> Option<SourceSetSpecificity> {
    let segments: Vec<&str> = path
        .split(|c| c == '/' || c == '\\')
        .filter(|segment| !segment.is_empty())
        .collect();

    for pair in segments.windows(2) {
        if pair[0] == "src" {
            if let Some(kind) = classify_source_set(pair[1]) {
                return Some(kind);
            }
        }
    }

    // Some sources jars preserve source-set roots directly (`commonMain/...`, `jvmMain/...`)
    // instead of the Gradle project layout (`src/commonMain/kotlin/...`).
    segments.iter().find_map(|segment| match *segment {
        "commonMain" => Some(SourceSetSpecificity::Generic),
        source_set if is_specific_main_source_set(source_set) => {
            Some(SourceSetSpecificity::Specific)
        }
        _ => None,
    })
}

fn classify_source_set(source_set: &str) -> Option<SourceSetSpecificity> {
    match source_set {
        "main" | "commonMain" => Some(SourceSetSpecificity::Generic),
        s if is_specific_main_source_set(s) => Some(SourceSetSpecificity::Specific),
        _ => None,
    }
}

fn is_specific_main_source_set(source_set: &str) -> bool {
    source_set.ends_with("Main") && source_set != "Main" && source_set != "commonMain"
}

fn resolve_import_target_a(
    index: &dyn InferenceIndex,
    usage: Node,
    ancestors: &[Node],
    src: &str,
) -> Option<Vec<Def>> {
    let import = ancestor_of_kind_a(ancestors, "import")?;
    let qid = child_of_kind(import, "qualified_identifier")?;
    let parts = direct_identifier_parts(qid, src);
    if parts.is_empty() {
        return Some(Vec::new());
    }

    let on_imported_name = parts.last().is_some_and(|(node, _)| *node == usage);
    let on_alias = ancestors.last().copied() == Some(import);
    if !on_imported_name && !on_alias {
        return Some(Vec::new());
    }

    Some(resolve_absolute_path(index, &part_names(&parts), |_| true))
}

fn resolve_qualified_a(
    index: &dyn InferenceIndex,
    usage: Node,
    ancestors: &[Node],
    name: &str,
    uk: UseKind,
    src: &str,
    named_usage: Option<&MemberUse>,
    ctx: &infer::FileCtx,
) -> Option<Vec<Def>> {
    let (parts, qkind) = qualified_path_and_kind_a(index, usage, ancestors, uk, src, ctx)?;
    if !parts.last().is_some_and(|part| part == name) {
        return Some(Vec::new());
    }
    let entries = absolute_path_entries(index, &parts, |kind| kind_ok(qkind, kind));
    if named_usage.is_some() && entries.is_empty() {
        // An unrecognized path may still be an inherited or otherwise typed member access.
        return None;
    }
    Some(call_entries_to_defs(index, entries, named_usage, ctx))
}

fn resolve_nested_type_in(
    index: &dyn InferenceIndex,
    src: &str,
    usage: Node,
    parent: Option<Node>,
    ctx: &infer::FileCtx,
) -> Option<Vec<Def>> {
    let parent = parent?;
    if parent.kind() != "user_type" {
        return None;
    }
    let parts = direct_identifier_parts(parent, src);
    if parts.len() < 2 || !parts.last().is_some_and(|(node, _)| *node == usage) {
        return None;
    }
    let names = part_names(&parts);

    // Package-qualified nested type: `pkg.Outer.Inner`.
    if names.len() > 2 {
        let absolute = resolve_absolute_path(index, &names, |kind| kind.is_type_like());
        if !absolute.is_empty() {
            return Some(absolute);
        }
    }

    // Visible nested type: `Outer.Inner`, where `Outer` may be same-package, explicitly imported,
    // alias-imported, wildcard-imported, or default-imported.
    if names.len() == 2 {
        let outer = &names[0];
        let inner = &names[1];
        let visible_outers = visible_outer_types(index, outer, ctx);
        let hits: Vec<&Entry> = index
            .lookup_by_name(inner)
            .iter()
            .filter(|e| e.sym.kind.is_type_like())
            .filter(|e| {
                visible_outers.iter().any(|(real_outer, pkg)| {
                    e.sym.container.as_deref() == Some(real_outer.as_str()) && &e.sym.package == pkg
                })
            })
            .collect();
        return Some(entries_to_defs(hits));
    }

    None
}

fn qualified_path_and_kind_a(
    index: &dyn InferenceIndex,
    usage: Node,
    ancestors: &[Node],
    uk: UseKind,
    src: &str,
    ctx: &infer::FileCtx,
) -> Option<(Vec<String>, UseKind)> {
    let parent = ancestors.last().copied()?;
    if parent.kind() == "user_type" {
        let parts = direct_identifier_parts(parent, src);
        if parts.len() > 1 && parts.last().is_some_and(|(node, _)| *node == usage) {
            return Some((part_names(&parts), UseKind::Type));
        }
    }
    if parent.kind() == "navigation_expression" {
        let parts = unshadowed_navigation_parts(index, parent, src, ctx)?;
        if parts.last().is_some_and(|(n, _)| *n == usage) {
            let qkind = match ancestors
                .get(ancestors.len().wrapping_sub(2))
                .map(|p| p.kind())
            {
                Some("call_expression") => UseKind::Call,
                _ => {
                    if uk == UseKind::MemberSelector {
                        UseKind::Value
                    } else {
                        uk
                    }
                }
            };
            return Some((part_names(&parts), qkind));
        }
    }
    None
}

/// Recognize an exact package-qualified top-level callable before treating its prefix as a value.
/// Empty or unsupported paths leave the existing typed-member route in charge.
pub(crate) fn qualified_top_level_function_entries<'a>(
    index: &'a dyn InferenceIndex,
    callee: Node,
    src: &str,
    ctx: &infer::FileCtx,
) -> Option<Vec<&'a Entry>> {
    let parts = unshadowed_navigation_parts(index, callee, src, ctx)?;
    let mut entries = absolute_path_entries(index, &part_names(&parts), |kind| {
        kind == SymbolKind::Function
    });
    entries.retain(|entry| entry.sym.container.is_none() && entry.sym.ext_receiver.is_none());
    (!entries.is_empty()).then_some(entries)
}

/// A qualified classifier is a receiver only after ruling out a value at its root.
pub(crate) fn qualified_type_receiver(
    index: &dyn InferenceIndex,
    node: Node,
    src: &str,
    ctx: &infer::FileCtx,
) -> Option<Type> {
    let parts = navigation_parts(node, src)?;
    let names = part_names(&parts);
    let entries = absolute_path_entries(index, &names, |kind| {
        matches!(
            kind,
            SymbolKind::Class | SymbolKind::Interface | SymbolKind::Object | SymbolKind::EnumClass
        )
    });
    let [entry] = entries.as_slice() else {
        return None;
    };
    if !absolute_path_entries(index, &names, |kind| {
        matches!(kind, SymbolKind::Property | SymbolKind::EnumEntry)
    })
    .is_empty()
    {
        return None;
    }
    unshadowed_navigation_parts(index, node, src, ctx)?;
    Some(Type::Class {
        name: entry.sym.name.clone(),
        package: Some(entry.sym.package.clone()),
        container: entry.sym.container.clone(),
        nullable: false,
        args: Vec::new(),
        owner: None,
    })
}

fn unshadowed_navigation_parts<'t>(
    index: &dyn InferenceIndex,
    node: Node<'t>,
    src: &str,
    ctx: &infer::FileCtx,
) -> Option<Vec<(Node<'t>, String)>> {
    let parts = navigation_parts(node, src)?;
    if parts.len() < 2 {
        return None;
    }
    let (first_node, first_name) = &parts[0];
    if !index.scope_is_complete()
        || local_decl(*first_node, first_name, UseKind::Value, src).is_some()
        || qualified_root_has_binding(index, first_name, ctx)
    {
        return None;
    }
    for receiver in infer::implicit_receiver_types(index, node, src, ctx, 0) {
        let mut complete = true;
        let _ = infer::walk_supertypes(index, &receiver, infer::MAX_DEPTH, |current| {
            let Some(root) = infer::receiver_root_key(index, current) else {
                complete = false;
                return ControlFlow::Break(());
            };
            let owners: Vec<_> = index
                .lookup_type(current.name().unwrap_or_default())
                .into_iter()
                .filter(|owner| {
                    Some(owner.sym.package.as_str()) == current.package()
                        && owner.sym.container.as_deref() == current.container()
                })
                .collect();
            if owners.len() != 1
                || !index.source_value_namespace_complete(owners[0])
                || index.members_of(&root).iter().any(|entry| {
                    entry.sym.name == *first_name
                        && (entry.sym.kind.is_value_like() || entry.sym.kind.is_type_like())
                        && Some(entry.sym.package.as_str()) == current.package()
                        && entry.sym.container.as_deref() == Some(root.as_str())
                })
            {
                complete = false;
                return ControlFlow::Break(());
            }
            ControlFlow::Continue(())
        });
        if !complete {
            return None;
        }
    }
    Some(parts)
}

fn qualified_root_has_binding(
    index: &dyn InferenceIndex,
    name: &str,
    ctx: &infer::FileCtx,
) -> bool {
    let is_root_binding = |entry: &Entry| {
        entry.sym.container.is_none()
            && entry.sym.ext_receiver.is_none()
            && (entry.sym.kind.is_value_like() || entry.sym.kind.is_type_like())
    };
    if ctx.imports.iter().any(|import| {
        if import.wildcard || import.local_name() != Some(name) {
            return false;
        }
        let entries: Vec<_> = index
            .lookup_by_name(import.simple_name())
            .into_iter()
            .filter(|entry| import_path_matches_entry(import, entry))
            .collect();
        entries.is_empty()
            || entries
                .iter()
                .any(|entry| entry.sym.kind.is_value_like() || entry.sym.kind.is_type_like())
    }) {
        return true;
    }
    if ctx
        .imports
        .iter()
        .filter(|import| import.wildcard)
        .any(|import| {
            absolute_path_entries(
                index,
                &import
                    .path
                    .split('.')
                    .map(str::to_string)
                    .collect::<Vec<_>>(),
                SymbolKind::is_type_like,
            )
            .iter()
            .any(|owner| !index.source_value_namespace_complete(owner))
        })
    {
        return true;
    }
    index.lookup_by_name(name).iter().any(|entry| {
        (is_root_binding(entry)
            && (entry.sym.package == ctx.package
                || DEFAULT_IMPORT_PACKAGES.contains(&entry.sym.package.as_str())
                || ctx
                    .imports
                    .iter()
                    .any(|import| import.wildcard && import.package() == entry.sym.package)))
            || (entry.sym.kind.is_value_like() || entry.sym.kind.is_type_like())
                && ctx.imports.iter().any(|import| {
                    import.wildcard
                        && absolute_path_matches(
                            entry,
                            &import
                                .path
                                .split('.')
                                .map(str::to_string)
                                .collect::<Vec<_>>(),
                        )
                })
    })
}


fn ancestor_of_kind_a<'t>(ancestors: &[Node<'t>], kind: &str) -> Option<Node<'t>> {
    ancestors.iter().rev().find(|n| n.kind() == kind).copied()
}

fn direct_identifier_parts<'t>(node: Node<'t>, src: &str) -> Vec<(Node<'t>, String)> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "identifier" {
            out.push((child, node_text(child, src).to_string()));
        }
    }
    out
}

fn part_names(parts: &[(Node, String)]) -> Vec<String> {
    parts.iter().map(|(_, name)| name.clone()).collect()
}

fn navigation_parts<'t>(node: Node<'t>, src: &str) -> Option<Vec<(Node<'t>, String)>> {
    match node.kind() {
        "identifier" => Some(vec![(node, node_text(node, src).to_string())]),
        "navigation_expression" => {
            let mut cursor = node.walk();
            if !node
                .children(&mut cursor)
                .any(|child| !child.is_named() && child.kind() == ".")
            {
                return None;
            }
            let mut out = navigation_parts(node.named_child(0)?, src)?;
            let selector = node.named_child(1)?;
            if selector.kind() != "identifier" {
                return None;
            }
            out.push((selector, node_text(selector, src).to_string()));
            Some(out)
        }
        _ => None,
    }
}

fn resolve_absolute_path(
    index: &dyn InferenceIndex,
    parts: &[String],
    kind_ok: impl Fn(SymbolKind) -> bool,
) -> Vec<Def> {
    entries_to_defs(absolute_path_entries(index, parts, kind_ok))
}

fn absolute_path_entries<'a>(
    index: &'a dyn InferenceIndex,
    parts: &[String],
    kind_ok: impl Fn(SymbolKind) -> bool,
) -> Vec<&'a Entry> {
    let Some(name) = parts.last() else {
        return Vec::new();
    };
    let prefix = &parts[..parts.len() - 1];
    index
        .lookup_by_name(name)
        .iter()
        .filter(|entry| kind_ok(entry.sym.kind) && absolute_path_matches(entry, prefix))
        .collect()
}

fn import_path_matches_entry(import: &Import, entry: &Entry) -> bool {
    if import.wildcard || import.simple_name() != entry.sym.name {
        return false;
    }
    let parts: Vec<String> = import.path.split('.').map(str::to_string).collect();
    let prefix = &parts[..parts.len().saturating_sub(1)];
    absolute_path_matches(entry, prefix)
}

fn absolute_path_matches(entry: &Entry, prefix: &[String]) -> bool {
    if let Some(container) = &entry.sym.container {
        let Some((last, package_parts)) = prefix.split_last() else {
            return false;
        };
        if container == last && entry.sym.package == package_parts.join(".") {
            return true;
        }
        if last == "Companion" {
            let Some((enclosing, package_parts)) = package_parts.split_last() else {
                return false;
            };
            return container == enclosing && entry.sym.package == package_parts.join(".");
        }
        false
    } else {
        entry.sym.package == prefix.join(".")
    }
}

fn visible_outer_types(
    index: &dyn InferenceIndex,
    local_name: &str,
    ctx: &infer::FileCtx,
) -> Vec<(String, String)> {
    let mut out = Vec::new();

    for imp in &ctx.imports {
        if imp.alias.as_deref() == Some(local_name) {
            push_visible_outer(index, &mut out, imp.simple_name(), &imp.package());
        }
    }
    for imp in &ctx.imports {
        if !imp.wildcard && imp.alias.is_none() && imp.simple_name() == local_name {
            push_visible_outer(index, &mut out, local_name, &imp.package());
        }
    }

    push_visible_outer(index, &mut out, local_name, &ctx.package);

    let star_pkgs = ctx
        .imports
        .iter()
        .filter(|i| i.wildcard)
        .map(|i| i.package())
        .chain(DEFAULT_IMPORT_PACKAGES.iter().map(|s| s.to_string()));
    for pkg in star_pkgs {
        push_visible_outer(index, &mut out, local_name, &pkg);
    }

    out
}

fn push_visible_outer(
    index: &dyn InferenceIndex,
    out: &mut Vec<(String, String)>,
    name: &str,
    package: &str,
) {
    if index
        .lookup_by_name(name)
        .iter()
        .any(|e| e.sym.kind.is_type_like() && e.sym.container.is_none() && e.sym.package == package)
    {
        let item = (name.to_string(), package.to_string());
        if !out.contains(&item) {
            out.push(item);
        }
    }
}

fn has_visible_cross_file(
    index: &dyn InferenceIndex,
    name: &str,
    uk: UseKind,
    imports: &[Import],
    current_pkg: &str,
) -> bool {
    for imp in imports {
        if imp.alias.as_deref() == Some(name) {
            let real = imp.simple_name();
            return index
                .lookup_by_name(real)
                .iter()
                .any(|e| kind_ok(uk, e.sym.kind) && import_path_matches_entry(imp, e));
        }
    }

    let candidates = index
        .lookup_by_name(name)
        .iter()
        .filter(|e| kind_ok(uk, e.sym.kind))
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return false;
    }

    let explicit_imports: Vec<&Import> = imports
        .iter()
        .filter(|i| !i.wildcard && i.local_name() == Some(name))
        .collect();
    if candidates.iter().any(|e| {
        explicit_imports
            .iter()
            .any(|imp| import_path_matches_entry(imp, e))
    }) {
        return true;
    }
    if candidates.iter().any(|e| e.sym.package == current_pkg) {
        return true;
    }

    let star_pkgs: Vec<String> = imports
        .iter()
        .filter(|i| i.wildcard)
        .map(|i| i.package())
        .chain(DEFAULT_IMPORT_PACKAGES.iter().map(|s| s.to_string()))
        .collect();
    candidates
        .iter()
        .any(|e| star_pkgs.contains(&e.sym.package))
}

fn resolve_cross_file(
    index: &dyn InferenceIndex,
    name: &str,
    uk: UseKind,
    ctx: &infer::FileCtx,
    named_usage: Option<&MemberUse>,
) -> Vec<Def> {
    // `as`-alias: the usage name is an import alias; resolve the *real* name in that package.
    for imp in &ctx.imports {
        if imp.alias.as_deref() == Some(name) {
            let real = imp.simple_name();
            let hits: Vec<&Entry> = index
                .lookup_by_name(real)
                .iter()
                .filter(|e| kind_ok(uk, e.sym.kind) && import_path_matches_entry(imp, e))
                .collect();
            return call_entries_to_defs(index, hits, named_usage, ctx);
        }
    }

    let candidates: Vec<Entry> = index
        .lookup_by_name(name)
        .iter()
        .filter(|e| kind_ok(uk, e.sym.kind))
        .cloned()
        .collect();
    if candidates.is_empty() {
        return Vec::new();
    }

    // Explicit (non-wildcard) import of this exact name.
    let explicit_imports: Vec<&Import> = ctx
        .imports
        .iter()
        .filter(|i| !i.wildcard && i.local_name() == Some(name))
        .collect();
    if let Some(hits) = pick(&candidates, |e| {
        explicit_imports
            .iter()
            .any(|imp| import_path_matches_entry(imp, e))
    }) {
        return call_entries_to_defs(index, hits, named_usage, ctx);
    }

    // Same package as the current file (Kotlin needs no import for these).
    if let Some(hits) = pick(&candidates, |e| e.sym.package == ctx.package) {
        return call_entries_to_defs(index, hits, named_usage, ctx);
    }

    // Wildcard-imported packages in declaration order, then Kotlin's implicit default imports in
    // their documented precedence (kotlin.* before java.lang.*): the first package binding the
    // name wins, so resolution never depends on index insertion order (which shifts when library
    // shards land at different times).
    let star_pkgs: Vec<String> = ctx
        .imports
        .iter()
        .filter(|i| i.wildcard)
        .map(|i| i.package())
        .chain(DEFAULT_IMPORT_PACKAGES.iter().map(|s| s.to_string()))
        .collect();
    for pkg in &star_pkgs {
        if let Some(hits) = pick(&candidates, |e| &e.sym.package == pkg) {
            return call_entries_to_defs(index, hits, named_usage, ctx);
        }
    }

    // Not visible from here (different package, not imported): no guess.
    Vec::new()
}

/// Packages Kotlin imports implicitly into every file (JVM target). Symbols in these resolve
/// without an explicit `import`.
pub(crate) const DEFAULT_IMPORT_PACKAGES: &[&str] = &[
    "kotlin",
    "kotlin.annotation",
    "kotlin.collections",
    "kotlin.comparisons",
    "kotlin.io",
    "kotlin.ranges",
    "kotlin.sequences",
    "kotlin.text",
    "kotlin.jvm",
    "java.lang",
];

/// Whether `pkg` is one of Kotlin's implicit default-import packages (symbols in it resolve
/// without an explicit `import`). A thin predicate over `DEFAULT_IMPORT_PACKAGES` so callers
/// (e.g. completion) reuse the exact same set without leaking its representation.
pub fn is_default_import_pkg(pkg: &str) -> bool {
    DEFAULT_IMPORT_PACKAGES.contains(&pkg)
}

fn pick<'a>(candidates: &'a [Entry], keep: impl Fn(&Entry) -> bool) -> Option<Vec<&'a Entry>> {
    let hits: Vec<&Entry> = candidates.iter().filter(|e| keep(e)).collect();
    (!hits.is_empty()).then_some(hits)
}
