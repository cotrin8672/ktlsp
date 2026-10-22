//! Shared semantic queries built on top of the proof-bounded resolution core.
//!
//! This module is the first feature-facing layer of the gradual semantic engine: callers ask a
//! semantic question ("what do we know about this reference?") and get back a structured,
//! proof-bounded answer they can format for navigation, explainability, or diagnostics.

use std::collections::HashSet;
use std::ops::ControlFlow;

use tree_sitter::{Node, Tree};

use crate::complete::{self, ScopeCompletion};
use crate::hierarchy;
use crate::imports::{self, ImportLayout, ImportStyle};
use crate::index::{Entry, Index, InferenceIndex, LookupEntries, OriginFacts, ScopedIndex, Tier};
use crate::infer;
use crate::knowledge::Knowledge;
use crate::language::{self, SourceLanguage};
use crate::parser::{child_of_kind, identifier_at, node_text, KotlinParser};
use crate::resolve::{self, CompletenessFacts, ResolutionStatus, UseKind};
use crate::symbol::{Def, SymbolKind};
use crate::symbols::SymbolSummary;
use crate::types::Type;

type Visibility = language::NameVisibility;

pub struct ReferenceQuery {
    kind: UseKind,
    symbol: Option<String>,
    status: ResolutionStatus<()>,
}

impl ReferenceQuery {
    pub fn kind_label(&self) -> &'static str {
        kind_label(self.kind)
    }

    pub fn symbol(&self) -> Option<&str> {
        self.symbol.as_deref()
    }

    pub fn status_label(&self) -> &'static str {
        status_label(&self.status)
    }

    pub fn is_definitely_absent(&self) -> bool {
        self.status.is_definitely_absent()
    }

    pub fn reason_labels(&self) -> Vec<String> {
        reason_labels(&self.status)
    }
}

pub struct ResolvedSymbolQuery {
    reference: ReferenceQuery,
    pub targets: Vec<Def>,
    pub entry: Option<Entry>,
    compiled_signature: Option<crate::signature::Signature>,
}

impl ResolvedSymbolQuery {
    pub fn reference(&self) -> &ReferenceQuery {
        &self.reference
    }

    pub fn symbol_summary(&self) -> Option<SymbolSummary> {
        self.entry.as_ref().map(SymbolSummary::from_entry)
    }

    pub fn hover_markdown(&self) -> Option<String> {
        self.symbol_summary()
            .map(|summary| summary.hover_markdown())
            .or_else(|| {
                self.compiled_signature
                    .as_ref()
                    .map(|signature| format!("```java\n{}\n```", signature.label))
            })
    }
}

impl CompletionQuery {
    pub fn context_label(&self) -> &'static str {
        match self.context {
            complete::CompletionContext::ScopeName => "scope-name",
            complete::CompletionContext::AfterDot => "member",
            complete::CompletionContext::Import => "import",
            complete::CompletionContext::None => "none",
        }
    }

    pub fn status_label(&self) -> &'static str {
        completion_status_label(&self.status)
    }

    pub fn reason_labels(&self) -> Vec<String> {
        completion_reason_labels(&self.status)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompletionIncompletenessReason {
    ImportContext,
    NonCompletablePosition,
    UnknownReceiverType,
    UnknownReceiverKind,
    QueryScopeIncomplete,
}

impl CompletionIncompletenessReason {
    pub fn label(&self) -> &'static str {
        match self {
            CompletionIncompletenessReason::ImportContext => "import-context",
            CompletionIncompletenessReason::NonCompletablePosition => "non-completable-position",
            CompletionIncompletenessReason::UnknownReceiverType => "unknown-receiver-type",
            CompletionIncompletenessReason::UnknownReceiverKind => "unknown-receiver-kind",
            CompletionIncompletenessReason::QueryScopeIncomplete => "query-scope-incomplete",
        }
    }
}

pub type CompletionStatus = Knowledge<(), CompletionIncompletenessReason>;

pub struct CompletionQuery {
    pub context: complete::CompletionContext,
    pub prefix: String,
    pub candidates: Vec<ScopeCompletion>,
    pub layout: ImportLayout,
    pub status: CompletionStatus,
}

pub struct CallShapeQuery {
    pub symbol: String,
    pub arg_count: usize,
    pub arities: Vec<u8>,
    pub argument_types: Option<Vec<String>>,
}

impl CallShapeQuery {
    pub fn diagnostic_message(&self) -> String {
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

fn kind_label(kind: UseKind) -> &'static str {
    match kind {
        UseKind::Type => "type",
        UseKind::Call => "call",
        UseKind::MemberSelector => "member",
        UseKind::Value => "value",
    }
}

fn status_label(status: &ResolutionStatus<()>) -> &'static str {
    match status {
        ResolutionStatus::Found(()) => "ok",
        ResolutionStatus::DefinitelyAbsent => "definitely-absent",
        ResolutionStatus::Unknown(_) => "unknown",
    }
}

fn reason_labels(status: &ResolutionStatus<()>) -> Vec<String> {
    match status {
        ResolutionStatus::Unknown(reasons) => reasons.iter().map(|reason| reason.label()).collect(),
        _ => Vec::new(),
    }
}

fn completion_status_label(status: &CompletionStatus) -> &'static str {
    match status {
        CompletionStatus::Found(()) => "ok",
        CompletionStatus::DefinitelyAbsent => "empty",
        CompletionStatus::Unknown(_) => "unknown",
    }
}

fn completion_reason_labels(status: &CompletionStatus) -> Vec<String> {
    match status {
        CompletionStatus::Unknown(reasons) => reasons
            .iter()
            .map(|reason| reason.label().to_string())
            .collect(),
        _ => Vec::new(),
    }
}

pub fn reference_query(
    index: &dyn InferenceIndex,
    file: &str,
    tree: &Tree,
    src: &str,
    usage: Node,
    facts: &CompletenessFacts,
) -> Option<ReferenceQuery> {
    if usage.kind() != "identifier" {
        return None;
    }
    let symbol = Some(node_text(usage, src).to_string()).filter(|s| !s.is_empty());
    let kind = resolve::use_kind(usage);
    let status = resolve::reference_status(index, file, tree, src, usage, facts);
    Some(ReferenceQuery {
        kind,
        symbol,
        status,
    })
}

pub fn resolved_symbol_query(
    index: &dyn InferenceIndex,
    file: &str,
    tree: &Tree,
    src: &str,
    offset: usize,
    facts: &CompletenessFacts,
) -> Option<ResolvedSymbolQuery> {
    if SourceLanguage::for_key(file) == SourceLanguage::Java && index.may_have_compiled_callables()
    {
        if let Some(selection) = crate::java::jvm_call_selection_at(index, file, src, tree, offset)
        {
            if selection.candidates.iter().any(|candidate| {
                matches!(
                    candidate,
                    ktcore::jvm::CallableCandidate::Jvm {
                        owner: ktcore::jvm::JvmOwner::Compiled { .. },
                        ..
                    }
                )
            }) {
                if selection.applicability == ktcore::members::Applicability::Applicable {
                    if let [candidate @ ktcore::jvm::CallableCandidate::Jvm {
                        owner: ktcore::jvm::JvmOwner::Compiled { .. },
                        ..
                    }] = selection.consumable_candidates()
                    {
                        return Some(ResolvedSymbolQuery {
                            reference: ReferenceQuery {
                                kind: UseKind::MemberSelector,
                                symbol: Some(candidate.name().into()),
                                status: ResolutionStatus::Found(()),
                            },
                            targets: Vec::new(),
                            entry: None,
                            compiled_signature: Some(crate::signature::signature_for_callable(
                                *candidate,
                            )),
                        });
                    }
                }
                // Preserve the selected compiled provenance; a rejected or ambiguous call
                // cannot become an unrelated authored/global name through a second lookup.
                return Some(ResolvedSymbolQuery {
                    reference: ReferenceQuery {
                        kind: UseKind::MemberSelector,
                        symbol: selection
                            .candidates
                            .first()
                            .map(|candidate| candidate.name().into()),
                        status: ResolutionStatus::Unknown(vec![
                            resolve::IncompletenessReason::CompilerProducerIncomplete,
                        ]),
                    },
                    targets: Vec::new(),
                    entry: None,
                    compiled_signature: None,
                });
            }
        }
    }
    let usage = identifier_at(tree, offset)?;
    let symbol = Some(node_text(usage, src).to_string()).filter(|s| !s.is_empty());
    let kind = resolve::use_kind(usage);
    let ctx = infer::FileCtx::from_tree_in_file(tree, src, file);
    let targets = resolve::goto_with_ctx(index, file, src, tree, offset, &ctx);
    let entry = targets.first().and_then(|target| {
        hierarchy::entry_for_name_range(index, &target.file, target.start_byte, target.end_byte)
            .or_else(|| ctx.local_entry(target).cloned())
    });
    let status = if entry.is_some() {
        ResolutionStatus::Found(())
    } else {
        resolve::reference_status(index, file, tree, src, usage, facts)
    };
    Some(ResolvedSymbolQuery {
        reference: ReferenceQuery {
            kind,
            symbol,
            status,
        },
        targets,
        entry,
        compiled_signature: None,
    })
}

pub fn completion_query(
    index: &dyn InferenceIndex,
    parser: &mut KotlinParser,
    file: &str,
    src: &str,
    offset: usize,
    max_candidates: usize,
) -> Option<CompletionQuery> {
    let tree = parser.parse(src);
    let recovered = complete::incomplete_template_source(&tree, src, offset);
    let src = recovered.as_deref().unwrap_or(src);
    let tree = if recovered.is_some() {
        parser.parse(src)
    } else {
        tree
    };
    let context = complete::completion_context(&tree, src, offset);
    if context != complete::CompletionContext::None {
        let package = crate::parser::package_of(&tree, src);
        let imports = crate::parser::imports_of(&tree, src);
        let visibility = Visibility::new(&package, &imports);
        let scope = language::source_type_scope(file, &tree, src);
        let ctx = infer::FileCtx::from_tree_in_file(&tree, src, file);
        if let Some((prefix, mut candidates)) = complete::annotation_candidates(
            &ScopedIndex::new(index, file),
            file,
            src,
            &tree,
            offset,
            &scope,
            &ctx,
            |entry| visibility.is_symbol_visible(&entry.sym),
        ) {
            candidates.truncate(max_candidates);
            return Some(CompletionQuery {
                context: complete::CompletionContext::ScopeName,
                prefix,
                status: if candidates.is_empty() {
                    CompletionStatus::Unknown(vec![
                        CompletionIncompletenessReason::QueryScopeIncomplete,
                    ])
                } else {
                    CompletionStatus::Found(())
                },
                candidates,
                layout: imports::import_layout(&tree, src),
            });
        }
    }
    match context {
        complete::CompletionContext::ScopeName => scope_name_query(
            &ScopedIndex::new(index, file),
            file,
            &tree,
            src,
            offset,
            max_candidates,
        ),
        complete::CompletionContext::AfterDot => {
            after_dot_query(index, parser, file, src, offset, max_candidates)
        }
        complete::CompletionContext::Import => Some(CompletionQuery {
            context,
            prefix: String::new(),
            candidates: Vec::new(),
            layout: None,
            status: CompletionStatus::Unknown(vec![CompletionIncompletenessReason::ImportContext]),
        }),
        complete::CompletionContext::None => Some(CompletionQuery {
            context,
            prefix: String::new(),
            candidates: Vec::new(),
            layout: None,
            status: CompletionStatus::Unknown(vec![
                CompletionIncompletenessReason::NonCompletablePosition,
            ]),
        }),
    }
}

pub fn after_dot_query(
    index: &dyn InferenceIndex,
    parser: &mut KotlinParser,
    file: &str,
    src: &str,
    offset: usize,
    max_candidates: usize,
) -> Option<CompletionQuery> {
    let (prefix, synthetic, syn_offset) = complete::dot_recovery(src, offset)?;
    let tree = parser.parse(&synthetic);
    let receiver = complete::navigation_receiver_at(&tree, syn_offset)?;
    let inserted_at = syn_offset - complete::DOT_PLACEHOLDER.len();
    let mut symbols = language::symbol_facts(SourceLanguage::Kotlin, &tree, &synthetic).symbols;
    // Keep provider relations tied to authored declarations, never placeholder-shifted names.
    symbols.retain_mut(|symbol| {
        if symbol.end_byte <= inserted_at {
            true
        } else if symbol.start_byte >= syn_offset {
            symbol.start_byte -= complete::DOT_PLACEHOLDER.len();
            symbol.end_byte -= complete::DOT_PLACEHOLDER.len();
            true
        } else {
            false
        }
    });
    let mut source = Index::new();
    source.replace_file(file, symbols, Tier::Volatile);
    let recovered = RecoveredTypeIndex {
        index,
        source: &source,
        replaced_file: Some(file),
        source_member_proofs: false,
    };
    let ctx = infer::FileCtx::from_tree_in_file(&tree, &synthetic, file);
    member_completion_query_with_ctx(
        &ScopedIndex::new(&recovered, file),
        &tree,
        &synthetic,
        receiver,
        prefix,
        max_candidates,
        &ctx,
    )
}

pub fn member_completion_query(
    index: &dyn InferenceIndex,
    tree: &Tree,
    src: &str,
    receiver: Node,
    prefix: String,
    max_candidates: usize,
) -> Option<CompletionQuery> {
    let ctx = infer::FileCtx::from_tree(tree, src);
    member_completion_query_with_ctx(index, tree, src, receiver, prefix, max_candidates, &ctx)
}

fn member_completion_query_with_ctx(
    index: &dyn InferenceIndex,
    tree: &Tree,
    src: &str,
    receiver: Node,
    prefix: String,
    max_candidates: usize,
    ctx: &infer::FileCtx,
) -> Option<CompletionQuery> {
    if receiver.kind() == "super_expression" {
        let receivers = infer::super_receivers(index, receiver, src, &ctx);
        let selected = ktcore::members::super_candidates(index, receivers, &prefix, &ctx);
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();
        for (_, entries) in selected.providers {
            for entry in entries {
                push_member_candidate(
                    &mut candidates,
                    &mut seen,
                    entry,
                    &entry.sym.name,
                    &prefix,
                    None,
                );
            }
        }
        candidates.truncate(max_candidates);
        let status = if selected.incomplete || candidates.is_empty() {
            // Source hierarchy/implementation facts do not certify generated-API absence.
            CompletionStatus::Unknown(vec![CompletionIncompletenessReason::QueryScopeIncomplete])
        } else {
            CompletionStatus::Found(())
        };
        return Some(CompletionQuery {
            context: complete::CompletionContext::AfterDot,
            prefix,
            candidates,
            status,
            layout: imports::import_layout_with_style(ImportStyle::Kotlin, tree, src),
        });
    }
    let ty = infer::member_receiver_type(receiver, infer::infer(index, receiver, src, &ctx));
    let receiver_kind =
        if ktcore::jvm::compiler_object_value(index, &ty) || receiver.kind() == "call_expression" {
            ktcore::jvm::ReceiverKind::Value
        } else {
            ktcore::jvm::ReceiverKind::Unknown
        };
    member_completion_query_for_type_with_ctx(
        index,
        SourceLanguage::Kotlin,
        tree,
        src,
        ty,
        receiver_kind,
        prefix,
        max_candidates,
        Some((ctx, receiver.end_byte())),
    )
}

pub fn member_completion_query_for_type(
    index: &dyn InferenceIndex,
    language: SourceLanguage,
    tree: &Tree,
    src: &str,
    ty: Type,
    receiver_kind: ktcore::jvm::ReceiverKind,
    prefix: String,
    max_candidates: usize,
) -> Option<CompletionQuery> {
    let ctx = (language == SourceLanguage::Kotlin).then(|| infer::FileCtx::from_tree(tree, src));
    member_completion_query_for_type_with_ctx(
        index,
        language,
        tree,
        src,
        ty,
        receiver_kind,
        prefix,
        max_candidates,
        ctx.as_ref().map(|ctx| (ctx, 0)),
    )
}

pub(crate) fn member_completion_query_for_type_with_ctx(
    index: &dyn InferenceIndex,
    language: SourceLanguage,
    tree: &Tree,
    src: &str,
    ty: Type,
    receiver_kind: ktcore::jvm::ReceiverKind,
    prefix: String,
    max_candidates: usize,
    file_ctx: Option<(&infer::FileCtx, usize)>,
) -> Option<CompletionQuery> {
    let style = match language {
        SourceLanguage::Kotlin => ImportStyle::Kotlin,
        SourceLanguage::Java => ImportStyle::Java,
    };
    let layout = imports::import_layout_with_style(style, tree, src);
    if !ty.is_member_receiver() {
        return Some(CompletionQuery {
            context: complete::CompletionContext::AfterDot,
            prefix,
            candidates: Vec::new(),
            layout,
            status: CompletionStatus::Unknown(vec![
                CompletionIncompletenessReason::UnknownReceiverType,
            ]),
        });
    }
    // Kotlin recovery already supplies scoped file facts before receiver inference.
    let source_index = (language == SourceLanguage::Java && !matches!(ty, Type::Array { .. }))
        .then(|| {
            let mut source = Index::new();
            source.replace_file(
                file_ctx
                    .and_then(|(ctx, _)| ctx.file())
                    .unwrap_or("<completion>"),
                language::symbol_facts(language, tree, src).symbols,
                Tier::Volatile,
            );
            source
        });
    let (candidates, withheld_compiled) = member_candidates(
        index,
        source_index.as_ref(),
        language,
        &ty,
        receiver_kind,
        &prefix,
        file_ctx,
        max_candidates,
    );
    Some(CompletionQuery {
        context: complete::CompletionContext::AfterDot,
        prefix,
        status: if withheld_compiled {
            CompletionStatus::Unknown(vec![CompletionIncompletenessReason::UnknownReceiverKind])
        } else if candidates.is_empty() && ty.name().is_none() {
            CompletionStatus::Unknown(vec![CompletionIncompletenessReason::UnknownReceiverType])
        } else if candidates.is_empty() && !index.scope_is_complete() {
            CompletionStatus::Unknown(vec![CompletionIncompletenessReason::QueryScopeIncomplete])
        } else if candidates.is_empty() {
            CompletionStatus::DefinitelyAbsent
        } else {
            CompletionStatus::Found(())
        },
        candidates,
        layout,
    })
}

pub fn call_shape_query(
    index: &dyn InferenceIndex,
    file: &str,
    tree: &Tree,
    src: &str,
    call: Node,
    facts: &CompletenessFacts,
) -> Option<CallShapeQuery> {
    if call.kind() != "call_expression" || !index.scope_is_complete() {
        return None;
    }
    let normalized = outer_trailing_lambda_call(call);
    if normalized != call {
        return None;
    }
    if uses_named_arguments(normalized) {
        return None;
    }
    let callee = callable_callee(normalized)?;
    match callee.kind() {
        "identifier" => {
            top_level_call_shape_query(index, file, tree, src, normalized, callee, facts)
        }
        "navigation_expression" => {
            member_call_shape_query(index, file, tree, src, normalized, callee, facts)
        }
        _ => None,
    }
}

fn top_level_call_shape_query(
    index: &dyn InferenceIndex,
    file: &str,
    tree: &Tree,
    src: &str,
    call: Node,
    ident: Node,
    facts: &CompletenessFacts,
) -> Option<CallShapeQuery> {
    let symbol = node_text(ident, src).to_string();
    if symbol.is_empty() {
        return None;
    }
    if !resolve::top_level_call_completeness_reasons(file, tree, src, &symbol, facts).is_empty() {
        return None;
    }
    let ctx = infer::FileCtx::from_tree_in_file(tree, src, file);
    let defs = resolve::goto_with_ctx(index, file, src, tree, ident.start_byte(), &ctx);
    let local_selected = defs.iter().any(|def| ctx.local_entry(def).is_some());
    let mut entries = defs
        .into_iter()
        .filter_map(|def| {
            hierarchy::entry_for_name_range(index, &def.file, def.start_byte, def.end_byte)
                .or_else(|| ctx.local_entry(&def).cloned())
        })
        .collect::<Vec<_>>();
    if !local_selected {
        entries = expand_same_file_function_overloads(index, file, &symbol, entries);
    }
    if entries.is_empty() {
        return None;
    }
    if entries.iter().any(|entry| {
        entry.sym.kind != SymbolKind::Function
            || entry.sym.arity.is_none()
            || entry.sym.min_arity.is_none()
            || entry.sym.has_vararg
    }) {
        return None;
    }
    let arg_count = value_arg_count(call);
    let uses_trailing_lambda = has_trailing_lambda(call);
    let mut arities = entries
        .iter()
        .filter_map(|entry| entry.sym.arity)
        .collect::<Vec<_>>();
    arities.sort_unstable();
    arities.dedup();
    if entries
        .iter()
        .any(|entry| call_accepts_arg_count(index, entry, arg_count, uses_trailing_lambda, &ctx))
    {
        let arity_compatible: Vec<&Entry> = entries
            .iter()
            .filter(|entry| {
                call_accepts_arg_count(index, entry, arg_count, uses_trailing_lambda, &ctx)
            })
            .collect();
        return argument_type_mismatch_query(
            index,
            &ctx,
            src,
            call,
            symbol,
            arg_count,
            arity_compatible,
        );
    }
    Some(CallShapeQuery {
        symbol,
        arg_count,
        arities,
        argument_types: None,
    })
}

fn member_call_shape_query(
    index: &dyn InferenceIndex,
    file: &str,
    tree: &Tree,
    src: &str,
    call: Node,
    callee: Node,
    facts: &CompletenessFacts,
) -> Option<CallShapeQuery> {
    let recv = callee.named_child(0)?;
    let ident = callee.named_child(1)?;
    if ident.kind() != "identifier" {
        return None;
    }
    if !resolve::member_call_completeness_reasons(index, file, tree, src, ident, facts).is_empty() {
        return None;
    }
    let symbol = node_text(ident, src).to_string();
    if symbol.is_empty() {
        return None;
    }
    let ctx = infer::FileCtx::from_tree_in_file(tree, src, file);
    let (_recv_ty, entries) = if recv.kind() == "super_expression" {
        let (ty, candidates) = ktcore::members::super_member_candidates(
            index,
            infer::super_receivers(index, recv, src, &ctx),
            &symbol,
            &ctx,
        )?;
        let entries = candidates
            .members
            .into_iter()
            .filter(|entry| entry.sym.kind == SymbolKind::Function)
            .collect::<Vec<_>>();
        (ty, entries)
    } else {
        let ty = infer::member_receiver_type(recv, infer::infer(index, recv, src, &ctx));
        let candidates =
            ktcore::members::candidates_at(index, &ty, &symbol, &ctx, ident.start_byte());
        let usage = ktcore::members::MemberUse::Call(infer::call_arguments(index, call, src, &ctx));
        let entries = candidates
            .select(index, &ty, &usage, &ctx)
            .entries
            .into_iter()
            .filter(|entry| entry.sym.kind == SymbolKind::Function)
            .collect::<Vec<_>>();
        (ty, entries)
    };
    if entries.is_empty() {
        return None;
    }
    if entries.iter().any(|entry| {
        entry.sym.kind != SymbolKind::Function
            || entry.sym.arity.is_none()
            || entry.sym.min_arity.is_none()
            || entry.sym.has_vararg
    }) {
        return None;
    }
    let arg_count = value_arg_count(call);
    let uses_trailing_lambda = has_trailing_lambda(call);
    let mut arities = entries
        .iter()
        .filter_map(|entry| entry.sym.arity)
        .collect::<Vec<_>>();
    arities.sort_unstable();
    arities.dedup();
    if entries
        .iter()
        .any(|entry| call_accepts_arg_count(index, entry, arg_count, uses_trailing_lambda, &ctx))
    {
        let arity_compatible: Vec<&Entry> = entries
            .iter()
            .copied()
            .filter(|entry| {
                call_accepts_arg_count(index, entry, arg_count, uses_trailing_lambda, &ctx)
            })
            .collect();
        return argument_type_mismatch_query(
            index,
            &ctx,
            src,
            call,
            symbol,
            arg_count,
            arity_compatible,
        );
    }
    Some(CallShapeQuery {
        symbol,
        arg_count,
        arities,
        argument_types: None,
    })
}

pub fn scope_name_query(
    index: &dyn InferenceIndex,
    file: &str,
    tree: &Tree,
    src: &str,
    offset: usize,
    max_candidates: usize,
) -> Option<CompletionQuery> {
    let (prefix, anchor, mut items) = {
        let (prefix, anchor) = complete::prefix_at(tree, src, offset);
        let items = complete::complete_scope(tree, src, offset, &prefix);
        (prefix, anchor, items)
    };
    let anchor = anchor?;
    let pkg = crate::parser::package_of(tree, src);
    let imports = crate::parser::imports_of(tree, src);
    let layout = imports::import_layout(tree, src);
    let vis = Visibility::new(&pkg, &imports);
    let file_ctx = infer::FileCtx::from_tree_in_file(tree, src, file);
    for recv_ty in infer::implicit_receiver_types(index, anchor, src, &file_ctx, 0) {
        for candidate in member_candidates(
            index,
            None,
            SourceLanguage::Kotlin,
            &recv_ty,
            ktcore::jvm::ReceiverKind::Unknown,
            &prefix,
            Some((&file_ctx, anchor.start_byte())),
            max_candidates,
        )
        .0
        {
            if !items.iter().any(|existing| {
                existing.label == candidate.label && existing.kind == candidate.kind
            }) {
                items.push(candidate);
            }
        }
    }
    let mut index_items: Vec<(ScopeCompletion, u8)> = Vec::new();
    for candidate in complete::index_scope_candidates(
        index,
        file,
        &prefix,
        complete::IndexScopeCandidateConfig {
            include_contained: true,
            include_default_package: true,
        },
        |entry| Some(vis.is_symbol_visible(&entry.sym)),
    ) {
        let rank = match candidate.tier {
            crate::index::Tier::Volatile => 0,
            crate::index::Tier::Durable => 1,
        };
        index_items.push((candidate, rank));
    }
    for imp in &imports {
        if let Some(alias) = imp.alias.as_deref() {
            if alias.starts_with(&prefix) {
                index_items.push((
                    ScopeCompletion::new(alias.to_string(), SymbolKind::Object),
                    0,
                ));
            }
        }
    }
    for kw in KOTLIN_KEYWORDS {
        if kw.starts_with(&prefix) {
            index_items.push((ScopeCompletion::keyword(*kw), 0));
        }
    }
    index_items.sort_by(|a, b| a.0.label.cmp(&b.0.label).then(a.1.cmp(&b.1)));
    let mut seen: HashSet<String> = items.iter().map(|c| c.label.clone()).collect();
    for (c, _) in index_items {
        if seen.insert(c.label.clone()) {
            items.push(c);
        }
    }
    items.truncate(max_candidates);
    Some(CompletionQuery {
        context: complete::CompletionContext::ScopeName,
        prefix,
        status: if items.is_empty() && !index.scope_is_complete() {
            CompletionStatus::Unknown(vec![CompletionIncompletenessReason::QueryScopeIncomplete])
        } else if items.is_empty() {
            CompletionStatus::DefinitelyAbsent
        } else {
            CompletionStatus::Found(())
        },
        candidates: items,
        layout,
    })
}

// Query recovery replaces only its file; member presentation preserves other packages' homonyms.
pub(crate) struct RecoveredTypeIndex<'a> {
    index: &'a dyn InferenceIndex,
    source: &'a Index,
    replaced_file: Option<&'a str>,
    source_member_proofs: bool,
}

impl<'scope> RecoveredTypeIndex<'scope> {
    pub(crate) fn replacing_file(
        index: &'scope dyn InferenceIndex,
        source: &'scope Index,
        file: &'scope str,
    ) -> Self {
        Self {
            index,
            source,
            replaced_file: Some(file),
            source_member_proofs: false,
        }
    }

    pub(crate) fn with_source_member_proofs(mut self, proven_headers: bool) -> Self {
        self.source_member_proofs = proven_headers;
        self
    }

    fn replace_file_entries<'a>(
        &self,
        original: LookupEntries<'a>,
        recovered: &'a [Entry],
    ) -> LookupEntries<'a> {
        let Some(file) = self.replaced_file else {
            return original;
        };
        LookupEntries::Selected(
            original
                .into_iter()
                .filter(|entry| entry.path.as_ref() != file)
                .chain(recovered)
                .collect(),
        )
    }
}

impl InferenceIndex for RecoveredTypeIndex<'_> {
    fn source_declaration_type(&self, entry: &crate::index::Entry) -> Option<Type> {
        // Reuse only an unchanged authored declaration; its provider still validates current source.
        ((self.replaced_file != Some(entry.path.as_ref())
            || self
                .source
                .entries_for_file(&entry.path)
                .iter()
                .any(|recovered| recovered.sym == entry.sym))
            && self
                .index
                .entries_for_file(&entry.path)
                .iter()
                .any(|original| original.sym == entry.sym))
        .then(|| self.index.source_declaration_type(entry))
        .flatten()
    }
    fn source_value_namespace_complete(&self, owner: &Entry) -> bool {
        if self.replaced_file == Some(owner.path.as_ref()) {
            // Body recovery can restore an owner absent from the erroneous original tree.
            return self.source_members_complete(owner)
                || self.source_member_proofs
                    && self
                        .index
                        .entries_for_file(&owner.path)
                        .iter()
                        .any(|original| original.sym == owner.sym)
                    && self.index.source_value_namespace_complete(owner);
        }
        self.index
            .entries_for_file(&owner.path)
            .iter()
            .any(|original| original.sym == owner.sym)
            && self.index.source_value_namespace_complete(owner)
    }

    fn source_property_access(
        &self,
        property: &Entry,
    ) -> Option<ktcore::extensions::source::JvmPropertyAccess> {
        // Never transfer another accepted source's lowering identity to a synthetic
        // declaration. Foreign unchanged properties retain their host proof.
        (self.replaced_file != Some(property.path.as_ref())
            && self
                .index
                .entries_for_file(&property.path)
                .iter()
                .any(|original| original.sym == property.sym))
        .then(|| self.index.source_property_access(property))
        .flatten()
    }

    fn java_class_inventory(
        &self,
        owner: &Entry,
    ) -> Option<std::sync::Arc<ktcore::classfile::ClassFileFacts>> {
        (self.replaced_file != Some(owner.path.as_ref()))
            .then(|| self.index.java_class_inventory(owner))
            .flatten()
    }

    fn source_members_complete(&self, owner: &Entry) -> bool {
        (self.replaced_file != Some(owner.path.as_ref()) || self.source_member_proofs)
            && self.index.source_members_complete(owner)
    }

    fn may_have_compiled_callables(&self) -> bool {
        self.index.may_have_compiled_callables()
    }

    fn jvm_sources(&self, owner: &str) -> Vec<&ktcore::jvm::SourceFile> {
        self.index.jvm_sources(owner)
    }

    fn contributions_for_file(&self, file: &str) -> &[ktcore::extensions::ContributionSet] {
        self.index.contributions_for_file(file)
    }

    fn lookup_by_name(&self, name: &str) -> LookupEntries<'_> {
        self.replace_file_entries(
            self.index.lookup_by_name(name),
            self.source.lookup_by_name(name),
        )
    }
    fn entries_for_file(&self, file: &str) -> Vec<Entry> {
        let mut entries = if self.replaced_file == Some(file) {
            Vec::new()
        } else {
            self.index.entries_for_file(file)
        };
        if self.replaced_file.is_some() {
            entries.extend(self.source.entries_for_file(file));
        }
        entries
    }
    fn members_of(&self, name: &str) -> LookupEntries<'_> {
        if self.replaced_file.is_some() {
            return self
                .replace_file_entries(self.index.members_of(name), self.source.members_of(name));
        }
        let (container, simple) = name
            .rsplit_once('.')
            .map_or((None, name), |(container, simple)| {
                (Some(container), simple)
            });
        let owners = self.source.lookup_type(simple);
        let matches_owner = |entry: &&Entry| {
            owners.iter().any(|owner| {
                owner.sym.container.as_deref() == container
                    && owner.sym.package == entry.sym.package
            })
        };
        let mut entries: Vec<_> = self
            .index
            .members_of(name)
            .into_iter()
            .filter(|entry| !matches_owner(entry))
            .collect();
        entries.extend(self.source.members_of(name).iter());
        LookupEntries::Selected(entries)
    }
    fn extensions_for(&self, name: &str) -> LookupEntries<'_> {
        self.replace_file_entries(
            self.index.extensions_for(name),
            self.source.extensions_for(name),
        )
    }
    fn generic_receiver_extensions(&self, prefix: &str) -> LookupEntries<'_> {
        let original = self.index.generic_receiver_extensions(prefix);
        match self.replaced_file {
            Some(file) => LookupEntries::Selected(
                original
                    .into_iter()
                    .filter(move |entry| entry.path.as_ref() != file)
                    .chain(self.source.generic_receiver_extensions(prefix))
                    .collect(),
            ),
            None => original,
        }
    }
    fn origin_facts(&self) -> Option<&OriginFacts> {
        self.index.origin_facts()
    }
    fn scope_is_complete(&self) -> bool {
        self.index.scope_is_complete()
    }
    fn permits_unique_member_fallback(&self) -> bool {
        self.index.permits_unique_member_fallback()
    }
    fn lookup_type(&self, name: &str) -> Vec<&Entry> {
        if self.replaced_file.is_some() {
            return self
                .lookup_by_name(name)
                .into_iter()
                .filter(|entry| entry.sym.kind.is_type_like())
                .collect();
        }
        let recovered = self.source.lookup_type(name);
        let mut entries = self.index.lookup_type(name);
        entries.retain(|entry| {
            !recovered.iter().any(|recovered| {
                recovered.sym.package == entry.sym.package
                    && recovered.sym.container == entry.sym.container
            })
        });
        entries.extend(recovered);
        entries
    }

    fn entries_with_prefix<'a>(
        &'a self,
        prefix: &'a str,
        top_level_only: bool,
    ) -> Box<dyn Iterator<Item = &'a Entry> + 'a> {
        let original = self.index.entries_with_prefix(prefix, top_level_only);
        match self.replaced_file {
            Some(file) => Box::new(
                original
                    .filter(move |entry| entry.path.as_ref() != file)
                    .chain(self.source.entries_with_prefix(prefix, top_level_only)),
            ),
            None => original,
        }
    }
}

fn member_candidates(
    index: &dyn InferenceIndex,
    source_index: Option<&Index>,
    language: SourceLanguage,
    ty: &Type,
    receiver_kind: ktcore::jvm::ReceiverKind,
    prefix: &str,
    file_ctx: Option<(&infer::FileCtx, usize)>,
    max_candidates: usize,
) -> (Vec<ScopeCompletion>, bool) {
    if language == SourceLanguage::Kotlin
        && matches!(
            ty,
            Type::Array {
                nullable: false,
                ..
            }
        )
    {
        let out = ktcore::members::KOTLIN_JAVA_ARRAY_MEMBERS
            .iter()
            .filter(|(name, _, _)| name.starts_with(prefix))
            .take(max_candidates)
            .map(|(name, kind, arity)| {
                let mut item = ScopeCompletion::new(*name, *kind);
                item.arity = *arity;
                item
            })
            .collect();
        // shortcut: Kotlin array extensions, writes and general Array<T> projection need separate proof.
        return (out, false);
    }
    if language == SourceLanguage::Java && matches!(ty, Type::Array { .. }) {
        let mut out: Vec<_> = ktcore::members::JAVA_ARRAY_MEMBERS
            .iter()
            .filter(|(name, _, _)| name.starts_with(prefix))
            .take(max_candidates)
            .map(|(name, kind, arity)| {
                let mut item = ScopeCompletion::new(*name, *kind);
                item.arity = *arity;
                item
            })
            .collect();
        let mut seen = out
            .iter()
            .map(|item| (item.label.clone(), item.kind))
            .collect();
        for candidate in ktcore::jvm::member_candidates(index, ty, prefix) {
            if let ktcore::jvm::CallableCandidate::Authored(entry) = candidate {
                push_member_candidate(&mut out, &mut seen, entry, &entry.sym.name, prefix, None);
            }
        }
        out.truncate(max_candidates);
        // shortcut: array subtyping and Kotlin array presentation remain unmodeled.
        return (out, false);
    }
    let mut out: Vec<ScopeCompletion> = Vec::new();
    let mut seen: HashSet<(String, SymbolKind)> = HashSet::new();
    let mut extensions = Vec::new();
    let recovered = source_index.map(|source| RecoveredTypeIndex {
        index,
        source,
        replaced_file: None,
        source_member_proofs: false,
    });
    let inference_index: &dyn InferenceIndex = match &recovered {
        Some(recovered) => recovered,
        None => index,
    };
    let fallback_ctx = infer::FileCtx::new(String::new(), Vec::new());
    let (member_ctx, position) = file_ctx.unwrap_or((&fallback_ctx, 0));
    let shared_members = ktcore::members::completion_candidates_at(
        inference_index,
        ty,
        prefix,
        member_ctx,
        position,
        language == SourceLanguage::Kotlin,
    );
    // Prefer the accepted generated declaration's presentation to an inherited homonym.
    // Recovery still supplies authored members; exact call selection remains in ktcore::jvm.
    let mut java_callables = (language == SourceLanguage::Java
        || index.may_have_compiled_callables())
    .then(|| ktcore::jvm::member_candidates_at(inference_index, ty, prefix, member_ctx, position))
    .unwrap_or_default();
    let mut withheld_compiled = false;
    java_callables.retain(|candidate| {
        let blocked = matches!(candidate, ktcore::jvm::CallableCandidate::Jvm {
            facts, ..
        } if !facts.is_static && receiver_kind != ktcore::jvm::ReceiverKind::Value);
        withheld_compiled |= blocked;
        !blocked
    });
    for candidate in &java_callables {
        if let ktcore::jvm::CallableCandidate::Jvm {
            owner: ktcore::jvm::JvmOwner::Compiled { class, tier, .. },
            facts,
        } = candidate
        {
            if seen.insert((facts.name.clone(), SymbolKind::Function)) {
                let mut item = ScopeCompletion::new(facts.name.clone(), SymbolKind::Function);
                item.arity = candidate.arity();
                item.tier = *tier;
                let (package, name) = class
                    .internal_name
                    .rsplit_once('/')
                    .unwrap_or(("", &class.internal_name));
                item.package = package.replace('/', ".");
                item.container = Some(name.to_string());
                out.push(item);
            }
        }
    }
    if language == SourceLanguage::Kotlin {
        for entry in &shared_members.property_getters {
            let Some(name) = ktcore::members::kotlin_java_getter_property_name(entry)
                .filter(|name| name.starts_with(prefix))
            else {
                continue;
            };
            if seen.insert((name.clone(), SymbolKind::Property)) {
                let mut item = ScopeCompletion::new(name, SymbolKind::Property);
                item.package = entry.sym.package.clone();
                item.container = entry.sym.container.clone();
                item.tier = entry.tier;
                out.push(item);
            }
        }
    }
    for entry in &shared_members.members {
        if language == SourceLanguage::Kotlin
            && entry.sym.name.starts_with(prefix)
            && ktcore::members::kotlin_enum_property_type_ref(inference_index, ty, entry).is_some()
        {
            if seen.insert((entry.sym.name.clone(), SymbolKind::Property)) {
                let mut item = ScopeCompletion::new(entry.sym.name.clone(), SymbolKind::Property);
                item.package = entry.sym.package.clone();
                item.container = entry.sym.container.clone();
                item.tier = entry.tier;
                out.push(item);
            }
        } else {
            push_member_candidate(&mut out, &mut seen, entry, &entry.sym.name, prefix, None);
        }
    }
    let _ = infer::walk_supertypes(inference_index, ty, 32, |current| {
        let Some(cur) = infer::receiver_root_key(inference_index, current) else {
            return ControlFlow::Continue(());
        };
        if language == SourceLanguage::Kotlin && file_ctx.is_some() {
            extensions.extend(index.extensions_for(&cur));
        }
        ControlFlow::Continue(())
    });
    if let Some((ctx, _)) = file_ctx.filter(|_| language == SourceLanguage::Kotlin) {
        extensions.extend(shared_members.extensions.iter().copied());
        for entry in shared_members.known_local_extensions() {
            push_member_candidate(&mut out, &mut seen, entry, &entry.sym.name, prefix, None);
        }
        for entry in extensions {
            if ktcore::members::extension_receiver_applicability(inference_index, ty, entry)
                != ktcore::members::Applicability::Applicable
            {
                continue;
            }
            let mut visible = false;
            for name in resolve::visible_extension_names(entry, &ctx.imports, &ctx.package) {
                visible = true;
                if !shared_members.member_name_is_unavailable(name, entry.sym.kind) {
                    push_member_candidate(&mut out, &mut seen, entry, name, prefix, None);
                }
            }
            if !visible
                && !shared_members.member_name_is_unavailable(&entry.sym.name, entry.sym.kind)
            {
                push_member_candidate(
                    &mut out,
                    &mut seen,
                    entry,
                    &entry.sym.name,
                    prefix,
                    Some(fqn(&entry.sym.package, &entry.sym.name)),
                );
            }
        }
        for alias in ctx
            .imports
            .iter()
            .filter_map(|import| import.alias.as_deref())
        {
            if !alias.starts_with(prefix) {
                continue;
            }
            for entry in
                resolve::generic_receiver_extensions(index, alias, &ctx.imports, &ctx.package)
            {
                if shared_members.member_name_is_unavailable(alias, entry.sym.kind) {
                    continue;
                }
                if ktcore::members::extension_receiver_applicability(inference_index, ty, entry)
                    != ktcore::members::Applicability::Applicable
                {
                    continue;
                }
                push_member_candidate(&mut out, &mut seen, entry, alias, prefix, None);
            }
        }
    }
    if language == SourceLanguage::Java {
        for candidate in java_callables {
            if let ktcore::jvm::CallableCandidate::Jvm { owner, facts } = candidate {
                if seen.insert((facts.name.clone(), SymbolKind::Function)) {
                    let mut item = ScopeCompletion::new(facts.name.clone(), SymbolKind::Function);
                    item.arity = candidate.arity();
                    item.tier = owner.tier();
                    let (package, container) = match owner {
                        ktcore::jvm::JvmOwner::Source(source) => (
                            source.facts.package.clone(),
                            facts
                                .source_property
                                .as_ref()
                                .map(|property| property.owner.clone())
                                .unwrap_or_else(|| source.facts.owner.clone()),
                        ),
                        ktcore::jvm::JvmOwner::Compiled { class, .. } => {
                            let (package, name) = class
                                .internal_name
                                .rsplit_once('/')
                                .unwrap_or(("", &class.internal_name));
                            (package.replace('/', "."), name.to_string())
                        }
                    };
                    item.package = package;
                    item.container = Some(container);
                    out.push(item);
                }
            }
        }
    }
    out.truncate(max_candidates);
    (
        out,
        withheld_compiled || shared_members.has_uncertain_receivers(),
    )
}

fn argument_type_mismatch_query(
    index: &dyn InferenceIndex,
    ctx: &infer::FileCtx,
    src: &str,
    call: Node,
    symbol: String,
    arg_count: usize,
    entries: Vec<&Entry>,
) -> Option<CallShapeQuery> {
    if entries.is_empty() {
        return None;
    }
    let arg_types = synth_arg_types(index, call, src, ctx);
    let mut labels = Vec::with_capacity(arg_types.len());
    for ty in &arg_types {
        labels.push(type_label(ty)?);
    }
    if entries
        .iter()
        .any(|entry| infer::argument_types_consistent(index, entry, &arg_types, ctx))
    {
        return None;
    }
    Some(CallShapeQuery {
        symbol,
        arg_count,
        arities: Vec::new(),
        argument_types: Some(labels),
    })
}

fn expand_same_file_function_overloads(
    index: &dyn InferenceIndex,
    file: &str,
    symbol: &str,
    entries: Vec<Entry>,
) -> Vec<Entry> {
    let Some(seed) = entries
        .iter()
        .find(|entry| entry.path.as_ref() == file && entry.sym.kind == SymbolKind::Function)
    else {
        return entries;
    };
    let mut expanded = index
        .lookup_by_name(symbol)
        .iter()
        .filter(|entry| {
            entry.path.as_ref() == file
                && entry.sym.kind == SymbolKind::Function
                && entry.sym.container == seed.sym.container
        })
        .cloned()
        .collect::<Vec<_>>();
    if expanded.is_empty() {
        entries
    } else {
        expanded.sort_by(|a, b| {
            a.sym
                .container
                .cmp(&b.sym.container)
                .then(a.sym.start_byte.cmp(&b.sym.start_byte))
                .then(a.sym.end_byte.cmp(&b.sym.end_byte))
        });
        expanded.dedup_by(|a, b| {
            a.path == b.path
                && a.sym.start_byte == b.sym.start_byte
                && a.sym.end_byte == b.sym.end_byte
        });
        expanded
    }
}

fn synth_arg_types(
    index: &dyn InferenceIndex,
    call: Node,
    src: &str,
    ctx: &infer::FileCtx,
) -> Vec<Type> {
    let mut out = Vec::new();
    let va = if let Some(va) = call.child_by_field_name("valueArguments") {
        Some(va)
    } else {
        let mut cursor = call.walk();
        let found = call
            .named_children(&mut cursor)
            .find(|child| child.kind() == "value_arguments");
        found
    };
    let Some(va) = va else {
        return out;
    };
    let mut cursor = va.walk();
    for arg in va.named_children(&mut cursor) {
        if arg.kind() != "value_argument" {
            continue;
        }
        let n = arg.named_child_count();
        let ty = (n > 0)
            .then(|| arg.named_child(n - 1))
            .flatten()
            .map_or(Type::Unknown, |expr| infer::infer(index, expr, src, ctx));
        out.push(ty);
    }
    out
}

fn type_label(ty: &Type) -> Option<String> {
    match ty {
        Type::Parameter { name, nullable, .. } => {
            Some(format!("{name}{}", if *nullable { "?" } else { "" }))
        }
        Type::Class {
            name,
            nullable,
            args,
            ..
        } => {
            let mut out = name.clone();
            if !args.is_empty() {
                let inner = args.iter().map(type_label).collect::<Option<Vec<_>>>()?;
                out.push('<');
                out.push_str(&inner.join(", "));
                out.push('>');
            }
            if *nullable {
                out.push('?');
            }
            Some(out)
        }
        Type::Projected { projection, bound } => {
            let prefix = match projection {
                crate::types::Projection::Out => "out ",
                crate::types::Projection::In => "in ",
                _ => return None,
            };
            type_label(bound).map(|bound| format!("{prefix}{bound}"))
        }
        Type::Star => Some("*".to_string()),
        Type::Unknown | Type::Array { .. } => None,
    }
}

fn push_member_candidate(
    out: &mut Vec<ScopeCompletion>,
    seen: &mut HashSet<(String, SymbolKind)>,
    e: &Entry,
    name: &str,
    prefix: &str,
    import_path: Option<String>,
) {
    if !name.starts_with(prefix) {
        return;
    }
    if !seen.insert((name.to_string(), e.sym.kind)) {
        return;
    }
    let mut c = ScopeCompletion::new(name.to_string(), e.sym.kind);
    c.tier = e.tier;
    c.arity = e.sym.arity;
    c.package = e.sym.package.clone();
    c.container = e.sym.container.clone();
    c.import_path = import_path;
    out.push(c);
}

fn fqn(package: &str, name: &str) -> String {
    if package.is_empty() {
        name.to_string()
    } else {
        format!("{package}.{name}")
    }
}

fn value_arg_count(call: Node) -> usize {
    let mut n = 0;
    if let Some(va) = call.child_by_field_name("valueArguments") {
        let mut cursor = va.walk();
        n += va
            .named_children(&mut cursor)
            .filter(|x| x.kind() == "value_argument")
            .count();
    } else {
        let mut cursor = call.walk();
        for child in call.named_children(&mut cursor) {
            if child.kind() == "value_arguments" {
                let mut args_cursor = child.walk();
                n += child
                    .named_children(&mut args_cursor)
                    .filter(|x| x.kind() == "value_argument")
                    .count();
            } else if child.kind() == "call_expression" && n == 0 {
                n += value_arg_count(child);
            }
        }
    }
    if has_trailing_lambda(call) {
        n += 1;
    }
    n
}

fn has_trailing_lambda(call: Node) -> bool {
    child_of_kind(call, "annotated_lambda").is_some()
        || child_of_kind(call, "lambda_literal").is_some()
}

fn call_accepts_arg_count(
    index: &dyn InferenceIndex,
    entry: &Entry,
    arg_count: usize,
    uses_trailing_lambda: bool,
    ctx: &infer::FileCtx,
) -> bool {
    let min = if uses_trailing_lambda {
        infer::trailing_lambda_min_arity(index, entry, ctx)
            .unwrap_or_else(|| entry.sym.min_arity.expect("guarded above"))
    } else {
        entry.sym.min_arity.expect("guarded above")
    } as usize;
    let max = entry.sym.arity.expect("guarded above") as usize;
    (min..=max).contains(&arg_count)
}

fn outer_trailing_lambda_call(mut call: Node) -> Node {
    while let Some(parent) = call.parent() {
        if parent.kind() == "call_expression"
            && parent.named_child(0) == Some(call)
            && has_trailing_lambda(parent)
        {
            call = parent;
        } else {
            break;
        }
    }
    call
}

fn callable_callee(mut call: Node) -> Option<Node> {
    loop {
        let callee = call.named_child(0)?;
        if callee.kind() == "call_expression" {
            call = callee;
        } else {
            return Some(callee);
        }
    }
}

fn uses_named_arguments(call: Node) -> bool {
    if let Some(args) = child_of_kind(call, "value_arguments") {
        let mut cursor = args.walk();
        for arg in args.named_children(&mut cursor) {
            if arg.kind() != "value_argument" {
                continue;
            }
            let count = arg.named_child_count();
            if count >= 2
                && arg
                    .named_child(0)
                    .is_some_and(|child| child.kind() == "identifier")
            {
                return true;
            }
        }
        return false;
    }
    call.named_child(0)
        .filter(|child| child.kind() == "call_expression")
        .is_some_and(uses_named_arguments)
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
    "private",
    "public",
    "protected",
    "internal",
    "abstract",
    "final",
    "open",
    "override",
    "sealed",
    "data",
    "enum",
    "companion",
    "lateinit",
    "inline",
    "suspend",
    "const",
];

#[cfg(test)]
mod tests {
    use crate::parser::{identifier_at, KotlinParser};
    use crate::workspace::Workspace;

    use super::*;

    #[test]
    fn sdk_selection_keeps_factory_overloads_scoped_when_compiler_inputs_fail() {
        let (root, mut original) = crate::classpath::tests::sdk_selection_fixture();
        original.truncate(1);
        let source = &mut original[0].source_sets[0];
        source.compiler_metadata_incomplete = true;
        let kotlin = source
            .compiler_tasks
            .iter_mut()
            .find(|task| task.language == crate::classpath::CompilerLanguage::Kotlin)
            .unwrap();
        let home = kotlin.java_home.take().unwrap();
        kotlin.complete = false;
        kotlin.problem = Some("unavailable dependency snapshot".into());
        let selection = serde_json::json!({
            "adapter": "kgp-1.9.25-jvm-sdk-options-v1", "task": kotlin.task,
            "language": "kotlin", "java_home": home, "arguments": ["-jvm-target", "17"],
        });
        let authored = original[0].source_sets[0].source_roots[0].clone();
        let file = crate::paths::file_key(&authored.join("Use.kt"));
        for witnessed in [false, true] {
            let mut modules = original.clone();
            if witnessed {
                modules[0].source_sets[0]
                    .compiler_tasks
                    .iter_mut()
                    .find(|task| task.language == crate::classpath::CompilerLanguage::Kotlin)
                    .unwrap()
                    .options
                    .insert("sdkSelection".into(), selection.clone());
            }
            let model = std::sync::Arc::new(crate::classpath::ProjectSnapshot::new(modules));
            assert_eq!(
                model
                    .sdk_api_for_file(std::path::Path::new(&file))
                    .is_some(),
                witnessed
            );
            // This SDK-only witness must not certify current compiler or processor execution.
            let task = model.modules[0].source_sets[0]
                .compiler_tasks
                .iter()
                .find(|task| task.language == crate::classpath::CompilerLanguage::Kotlin)
                .unwrap();
            assert!(!task.complete && task.problem.is_some() && task.java_home.is_none());
            let mut ws = Workspace::new();
            ws.set_project_model(model);
            for (name, text) in [
                ("BeanFactory.java", include_str!("../../../dev/corpus/fixtures/spring-provider/BeanFactory.java")),
                ("ObjectProvider.java", include_str!("../../../dev/corpus/fixtures/spring-provider/ObjectProvider.java")),
                ("ResolvableType.java", include_str!("../../../dev/corpus/fixtures/spring-provider/ResolvableType.java")),
                ("ParameterizedTypeReference.java", include_str!("../../../dev/corpus/fixtures/spring-provider/ParameterizedTypeReference.java")),
                ("Use.kt", include_str!("../../../dev/corpus/fixtures/spring-provider/BeanFactoryExtensions.kt")),
            ] {
                ws.open(crate::paths::file_key(&authored.join(name)), text.into());
            }
            let class = crate::paths::file_key(&root.join("sdk/Class.java"));
            ws.open(
                &class,
                "package java.lang; public final class Class<T> {}".into(),
            );
            ws.index.set_file_origin(
                &class,
                ktcore::project_model::FileOrigin {
                    origin_id: crate::classpath::jdk_origin_id(&home.join("lib/src.zip")),
                    representation: ktcore::project_model::SourceRepresentation::Source,
                    project_scope: None,
                },
            );
            let src = ws.doc_text(&file).unwrap();
            let outer = src.find("getBeanProvider(ResolvableType.forType(").unwrap();
            let actual = ws.goto_definition(&file, outer);
            assert_eq!(
                actual.len(),
                if witnessed { 1 } else { 2 },
                "{witnessed}: {actual:?}"
            );
            if witnessed {
                let entries = ws.index.entries_for_file(&actual[0].file);
                let target = &entries
                    .iter()
                    .find(|entry| entry.sym.start_byte == actual[0].start_byte)
                    .unwrap()
                    .sym;
                assert_eq!(target.params[0].name, "ResolvableType");
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn annotation_completion_uses_exact_types_and_qualified_owners_across_languages() {
        let mut ws = Workspace::new();
        ws.open(
            "Marks.java",
            "package marks; public @interface OverrideMark {} class OverrideClass {}".into(),
        );
        ws.open("Outer.java", "package marks; public class Outer { public @interface OverrideNested {} private @interface OverrideHidden {} }".into());
        ws.open("Marks.kt", "package marks\nannotation class DeprecatedMark\nclass DeprecatedClass\nfun DeprecatedFunction() {}".into());
        for (file, src, needle, expected) in [
            (
                "Use.java",
                "class Use { @Over public String toString() { return \"\"; } }",
                "@Over",
                "OverrideMark",
            ),
            (
                "Use.java",
                "class Use { @marks.Over public String toString() { return \"\"; } }",
                "@marks.Over",
                "OverrideMark",
            ),
            (
                "Use.java",
                "@marks.Depre class Use {}",
                "@marks.Depre",
                "DeprecatedMark",
            ),
            (
                "Use.java",
                "@marks.Outer.Over class Use {}",
                "@marks.Outer.Over",
                "OverrideNested",
            ),
            (
                "Use.java",
                "import marks.Outer; @Outer.Over class Use {}",
                "@Outer.Over",
                "OverrideNested",
            ),
            ("Use.kt", "@Depre class Use", "@Depre", "DeprecatedMark"),
            (
                "Use.kt",
                "import marks.DeprecatedMark as RenamedMark\n@Rena class Use",
                "@Rena",
                "RenamedMark",
            ),
            (
                "Use.kt",
                "@marks.Depre class Use",
                "@marks.Depre",
                "DeprecatedMark",
            ),
            (
                "Use.kt",
                "@marks.Over class Use",
                "@marks.Over",
                "OverrideMark",
            ),
            (
                "Use.kt",
                "import marks.Outer as Alias\n@Alias.Over class Use",
                "@Alias.Over",
                "OverrideNested",
            ),
            (
                "Use.kt",
                "@get:marks.Depre val value = 1",
                "@get:marks.Depre",
                "DeprecatedMark",
            ),
        ] {
            ws.open(file, src.into());
            let offset = src.find(needle).unwrap() + needle.len();
            let result = ws
                .complete(file, offset, false)
                .unwrap_or_else(|| panic!("empty {file}: {src}"));
            assert!(
                result.items.iter().any(|item| item.label == expected),
                "{file}: {src}: {result:?}"
            );
            assert!(result.items.iter().all(|item| !matches!(
                item.label.as_str(),
                "OverrideClass" | "OverrideHidden" | "DeprecatedClass" | "DeprecatedFunction"
            )));
            if needle.contains('.') || expected == "RenamedMark" {
                assert!(result.items.iter().all(|item| item.auto_import.is_none()));
            }
        }
        for (file, src, needle) in [
            ("Use.java", "@missing.Over class Use {}", "@missing.Over"),
            ("Use.kt", "@missing.Depre class Use", "@missing.Depre"),
            (
                "Use.java",
                "@marks.Outer.OverrideH class Use {}",
                "@marks.Outer.OverrideH",
            ),
            (
                "Use.kt",
                "@marks.Outer.OverrideH class Use",
                "@marks.Outer.OverrideH",
            ),
            (
                "Use.java",
                "class Use { String s = \"@marks.Over\"; }",
                "@marks.Over",
            ),
            ("Use.kt", "val s = \"@marks.Depre\"", "@marks.Depre"),
            ("Use.java", "// @marks.Over", "@marks.Over"),
            ("Use.kt", "// @marks.Depre", "@marks.Depre"),
            (
                "Use.kt",
                "@marks.DeprecatedMark(value = other.Depre) class Use",
                "other.Depre",
            ),
        ] {
            ws.open(file, src.into());
            assert!(
                ws.complete(file, src.find(needle).unwrap() + needle.len(), false)
                    .is_none(),
                "{file}: {src}"
            );
        }
        let src = "package marks; public class Outer { private @interface OverrideHidden {} } @Outer.OverrideH class Other {}";
        ws.open("Outer.java", src.into());
        assert!(
            ws.complete(
                "Outer.java",
                src.find("@Outer.OverrideH").unwrap() + "@Outer.OverrideH".len(),
                false
            )
            .is_none(),
            "same file does not grant a different top-level nest private access"
        );
    }

    #[test]
    fn annotation_completion_retracts_unsaved_java_and_kotlin_type_changes() {
        for (producer, annotation, ordinary, consumer) in [
            (
                "Marks.java",
                "package marks; public @interface OverrideMark {}",
                "package marks; public interface OverrideMark {}",
                "@marks.Over class Use",
            ),
            (
                "Marks.kt",
                "package marks\nannotation class OverrideMark",
                "package marks\nclass OverrideMark",
                "@marks.Over class Use",
            ),
        ] {
            let mut ws = Workspace::new();
            for file in ["Use.java", "Use.kt"] {
                ws.open(file, consumer.into());
            }
            for source in [annotation, ordinary, annotation] {
                ws.open(producer, source.into());
                for file in ["Use.java", "Use.kt"] {
                    let result = ws.complete(file, "@marks.Over".len(), false);
                    assert_eq!(
                        result.is_some(),
                        source == annotation,
                        "{producer} -> {file}: {source}"
                    );
                }
            }
        }
    }

    #[test]
    fn annotation_completion_preserves_language_specific_private_nests() {
        for (file, src, needle, available) in [
            (
                "Outer.java",
                "public class Outer { static class Inner { private @interface Hidden {} } @Inner.Hid static class Other {} }",
                "@Inner.Hid",
                true,
            ),
            (
                "Outer.java",
                "public class Outer { private @interface Hidden {} } @Outer.Hid class Other {}",
                "@Outer.Hid",
                false,
            ),
            (
                "Outer.java",
                "public class Outer { static class Inner { public @interface Hidden {} } static class Other { static class Inner {} @Inner.Hid void run() {} } }",
                "@Inner.Hid",
                false,
            ),
            (
                "Outer.kt",
                "class Outer { private annotation class Hidden\n @Hid class Inner }",
                "@Hid",
                true,
            ),
            (
                "Outer.kt",
                "class Outer { class Inner { private annotation class Hidden }\n @Inner.Hid class Other }",
                "@Inner.Hid",
                false,
            ),
            (
                "Outer.kt",
                "private annotation class Hidden\n@Hid class Other",
                "@Hid",
                true,
            ),
        ] {
            let mut ws = Workspace::new();
            ws.open(file, src.into());
            let result = ws.complete(file, src.find(needle).unwrap() + needle.len(), false);
            assert_eq!(
                result
                    .as_ref()
                    .is_some_and(|result| result.items.iter().any(|item| item.label == "Hidden")),
                available,
                "{file}: {src}: {result:?}"
            );
        }
    }

    #[test]
    fn annotation_completion_imports_nested_types_only_when_legal_and_needed() {
        let mut ws = Workspace::new();
        ws.open("Outer.java", "package marks; public class Outer { public @interface NestedMark {} private @interface PrivateMark {} static class Inner { private @interface HiddenMark {} } @Hid void use() {} }".into());
        for (file, src, import) in [
            ("Use.java", "package marks; @Nes class Use {}", true),
            ("Use.kt", "package marks\n@Nes class Use", true),
            (
                "Use.java",
                "package marks; import marks.Outer.NestedMark; @Nes class Use {}",
                false,
            ),
            (
                "Use.kt",
                "package marks\nimport marks.Outer.NestedMark\n@Nes class Use",
                false,
            ),
            (
                "Use.java",
                "package marks; import marks.Outer.*; @Nes class Use {}",
                false,
            ),
        ] {
            ws.open(file, src.into());
            let result = ws
                .complete(file, src.find("@Nes").unwrap() + "@Nes".len(), false)
                .expect(src);
            let item = result
                .items
                .iter()
                .find(|item| item.label == "NestedMark")
                .unwrap();
            assert_eq!(
                item.auto_import.is_some(),
                import,
                "{file}: {src}: {item:?}"
            );
            if import {
                assert!(item
                    .auto_import
                    .as_ref()
                    .unwrap()
                    .text
                    .contains("marks.Outer.NestedMark"));
            }
        }
        let src = "package marks; public class Outer { public @interface NestedMark {} @Nes void use() {} }";
        ws.open("Outer.java", src.into());
        let result = ws
            .complete(
                "Outer.java",
                src.find("@Nes").unwrap() + "@Nes".len(),
                false,
            )
            .unwrap();
        assert!(result
            .items
            .iter()
            .find(|item| item.label == "NestedMark")
            .unwrap()
            .auto_import
            .is_none());
        let src = "public class Outer { static class Inner { private @interface HiddenMark {} } @Hid void use() {} }";
        ws.open("Outer.java", src.into());
        assert!(
            ws.complete(
                "Outer.java",
                src.find("@Hid").unwrap() + "@Hid".len(),
                false
            )
            .is_none(),
            "private import cannot make an out-of-scope nested annotation usable"
        );
    }

    #[test]
    fn annotation_completion_and_ordinary_types_apply_language_correct_imports() {
        let mut ws = Workspace::new();
        ws.open(
            "Types.kt",
            "package marks\nannotation class Marker\nclass Other".into(),
        );
        for (file, src, prefix, label, expected) in [
            (
                "Use.java",
                "package use;\n\n@Mar class Use {}\n",
                "Mar",
                "Marker",
                "package use;\nimport marks.Marker;\n\n@Marker class Use {}\n",
            ),
            (
                "Use.kt",
                "package use\n\n@Mar class Use\n",
                "Mar",
                "Marker",
                "package use\nimport marks.Marker\n\n@Marker class Use\n",
            ),
            (
                "Use.java",
                "package use;\n\nclass Use { Oth value; }\n",
                "Oth",
                "Other",
                "package use;\nimport marks.Other;\n\nclass Use { Other value; }\n",
            ),
            (
                "Use.kt",
                "package use\n\nval value: Oth? = null\n",
                "Oth",
                "Other",
                "package use\nimport marks.Other\n\nval value: Other? = null\n",
            ),
        ] {
            ws.open(file, src.into());
            let start = src.find(prefix).unwrap();
            let result = ws.complete(file, start + prefix.len(), false).expect(src);
            let item = result
                .items
                .iter()
                .find(|item| item.label == label)
                .unwrap();
            let import = item
                .auto_import
                .as_ref()
                .expect("out-of-package type needs import");
            let offset = crate::imports::import_insert_offset(src, import.line);
            let applied = crate::edit::apply_to_text(
                file,
                src,
                &[
                    crate::edit::TextEdit::new(
                        file,
                        start,
                        start + prefix.len(),
                        &item.insert_text,
                    ),
                    crate::edit::TextEdit::new(file, offset, offset, format!("{}\n", import.text)),
                ],
            )
            .unwrap();
            assert_eq!(applied, expected, "{file}: {src}");
        }
    }

    fn call_at<'t>(tree: &'t Tree, src: &str, needle: &str) -> Node<'t> {
        tree.root_node()
            .named_descendant_for_byte_range(src.find(needle).unwrap(), src.find(needle).unwrap())
            .and_then(|node| {
                let mut cur = Some(node);
                while let Some(n) = cur {
                    if n.kind() == "call_expression" {
                        return Some(n);
                    }
                    cur = n.parent();
                }
                None
            })
            .expect("call expression")
    }

    #[test]
    fn query_reports_definitely_absent_for_closed_missing_call() {
        let mut ws = Workspace::new();
        ws.assume_index_complete_for_tests();
        let src = "fun main() { missingCall() }\n";
        ws.open("Main.kt", src.to_string());

        let mut parser = KotlinParser::new();
        let tree = parser.parse(src);
        let offset = src.find("missingCall").unwrap();
        let ident = identifier_at(&tree, offset).unwrap();
        let query = reference_query(
            &ws.index,
            "Main.kt",
            &tree,
            src,
            ident,
            &resolve::CompletenessFacts::complete(),
        )
        .unwrap();

        assert_eq!(query.kind_label(), "call");
        assert_eq!(query.symbol(), Some("missingCall"));
        assert_eq!(query.status_label(), "definitely-absent");
        assert!(query.is_definitely_absent());
        assert!(query.reason_labels().is_empty());
    }

    #[test]
    fn resolved_symbol_query_returns_indexed_target() {
        let src = "/** docs */\nfun helper() {}\nfun main() { helper() }\n";
        let mut ws = Workspace::new();
        ws.assume_index_complete_for_tests();
        ws.open("Main.kt", src.to_string());

        let mut parser = KotlinParser::new();
        let tree = parser.parse(src);
        let query = resolved_symbol_query(
            &ws.index,
            "Main.kt",
            &tree,
            src,
            src.rfind("helper").unwrap(),
            &resolve::CompletenessFacts::complete(),
        )
        .expect("resolved symbol query");

        assert_eq!(query.reference().status_label(), "ok");
        assert_eq!(query.reference().symbol(), Some("helper"));
        assert_eq!(query.targets.len(), 1);
        assert_eq!(
            query.symbol_summary().map(|s| s.name),
            Some("helper".to_string())
        );
    }

    #[test]
    fn after_dot_query_reports_members_and_extension_imports() {
        let mut ws = Workspace::new();
        ws.open(
            "Main.kt",
            "package app\nclass Dog { fun bark() {} }\nfun lib.fetch(d: Dog) {}\nfun main(d: Dog) { d.fe }\n"
                .to_string(),
        );
        ws.index.replace_file(
            "Lib.kt",
            vec![crate::symbol::IndexedSymbol {
                name: "fetch".into(),
                kind: SymbolKind::Function,
                package: "lib".into(),
                container: None,
                start_byte: 0,
                end_byte: 5,
                documentation: None,
                supertypes: Vec::new(),
                supertype_refs: Vec::new(),
                unavailable_members: None,
                is_annotation: false,
                is_expect: false,
                ext_receiver: Some("Dog".into()),
                extension_receiver: None,
                arity: Some(1),
                min_arity: Some(1),
                has_vararg: false,
                trailing_lambda_min_arity: None,
                last_parameter_min_arity: None,
                trailing_lambda_receiver_type: None,
                function_type_receiver: None,
                return_type: None,
                value_type: None,
                kotlin_parameters: None,
                params: Vec::new(),
                type_params: Vec::new(),
                type_parameter_facts: Default::default(),
                source_member: Default::default(),
                method_type_bounds: None,
            }],
            crate::index::Tier::Durable,
        );

        let src = ws.doc_text("Main.kt").unwrap();
        let offset = src.find("fe }").unwrap() + 2;
        let mut parser = KotlinParser::new();
        let query = after_dot_query(&ws.index, &mut parser, "Main.kt", &src, offset, 1000)
            .expect("after-dot query");

        assert_eq!(query.context_label(), "member");
        assert_eq!(query.status_label(), "ok");
        assert!(matches!(query.status, CompletionStatus::Found(())));
        assert!(query.reason_labels().is_empty());
        assert_eq!(query.prefix, "fe");
        assert!(query.candidates.iter().any(|c| c.label == "fetch"));
        assert!(
            query
                .candidates
                .iter()
                .any(|c| { c.label == "fetch" && c.import_path.as_deref() == Some("lib.fetch") }),
            "{:?}",
            query
                .candidates
                .iter()
                .map(|c| (&c.label, &c.import_path))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn scope_name_query_reports_auto_import_candidates_and_keywords() {
        let mut ws = Workspace::new();
        ws.open(
            "Other.kt",
            "package other\nfun widgetXyz() {}\n".to_string(),
        );
        let src = "package app\nfun main() { wi }\n";
        ws.open("Main.kt", src.to_string());

        let mut parser = KotlinParser::new();
        let tree = parser.parse(src);
        let offset = src.find("wi }").unwrap() + 2;
        let query =
            scope_name_query(&ws.index, "Main.kt", &tree, src, offset, 1000).expect("scope query");

        assert_eq!(query.context_label(), "scope-name");
        assert_eq!(query.status_label(), "ok");
        assert!(query.candidates.iter().any(|c| {
            c.label == "widgetXyz" && c.import_path.as_deref() == Some("other.widgetXyz")
        }));

        let kw_src = "fun main() { wh }\n";
        let kw_tree = parser.parse(kw_src);
        let kw_offset = kw_src.find("wh }").unwrap() + 2;
        let kw_query = scope_name_query(&ws.index, "Main.kt", &kw_tree, kw_src, kw_offset, 1000)
            .expect("keyword query");
        assert!(kw_query.candidates.iter().any(|c| c.label == "while"));
        assert!(kw_query.candidates.iter().any(|c| c.label == "when"));
    }

    #[test]
    fn completion_query_reports_unknown_in_import_context() {
        let mut ws = Workspace::new();
        let src = "import ko\n";
        ws.open("Main.kt", src.to_string());

        let mut parser = KotlinParser::new();
        let offset = src.find("ko").unwrap() + 1;
        let query = completion_query(&ws.index, &mut parser, "Main.kt", src, offset, 1000)
            .expect("completion query");

        assert_eq!(query.context_label(), "import");
        assert_eq!(query.status_label(), "unknown");
        assert_eq!(query.reason_labels(), vec!["import-context".to_string()]);
    }

    #[test]
    fn member_completion_query_reports_unknown_receiver_type() {
        let mut ws = Workspace::new();
        let src = "fun main() { unknown.member }\n";
        ws.open("Main.kt", src.to_string());

        let mut parser = KotlinParser::new();
        let tree = parser.parse(src);
        let receiver = tree
            .root_node()
            .named_descendant_for_byte_range(
                src.find("unknown").unwrap(),
                src.find("unknown").unwrap(),
            )
            .expect("receiver")
            .parent()
            .expect("navigation")
            .named_child(0)
            .expect("receiver child");
        let query =
            member_completion_query(&ws.index, &tree, src, receiver, "me".to_string(), 1000)
                .expect("member query");

        assert_eq!(query.context_label(), "member");
        assert_eq!(query.status_label(), "unknown");
        assert_eq!(
            query.reason_labels(),
            vec!["unknown-receiver-type".to_string()]
        );
        assert!(query.candidates.is_empty());
    }

    #[test]
    fn scope_name_query_reports_empty_when_no_candidates_exist() {
        let mut ws = Workspace::new();
        let src = "fun main() { zzzz }\n";
        ws.open("Main.kt", src.to_string());

        let mut parser = KotlinParser::new();
        let tree = parser.parse(src);
        let offset = src.find("zzzz").unwrap() + 4;
        let query =
            scope_name_query(&ws.index, "Main.kt", &tree, src, offset, 1000).expect("scope query");

        assert_eq!(query.context_label(), "scope-name");
        assert_eq!(query.status_label(), "empty");
        assert!(query.reason_labels().is_empty());
        assert!(query.candidates.is_empty());
    }

    #[test]
    fn call_shape_query_reports_wrong_arity_when_every_target_is_known() {
        let src = "fun ping(a: Int) {}\nfun main() { ping() }\n";
        let mut ws = Workspace::new();
        ws.assume_index_complete_for_tests();
        ws.open("Main.kt", src.to_string());

        let mut parser = KotlinParser::new();
        let tree = parser.parse(src);
        let call = call_at(&tree, src, "ping()");

        let query = call_shape_query(
            &ws.index,
            "Main.kt",
            &tree,
            src,
            call,
            &resolve::CompletenessFacts::complete(),
        )
        .expect("call-shape query");
        assert_eq!(query.symbol, "ping");
        assert_eq!(query.arg_count, 0);
        assert_eq!(query.arities, vec![1]);
    }

    #[test]
    fn call_shape_query_allows_trailing_default_arguments() {
        let src =
            "fun manifest(value: Int, indent: String = \"  \") {}\nfun main() { manifest(1) }\n";
        let mut ws = Workspace::new();
        ws.assume_index_complete_for_tests();
        ws.open("Main.kt", src.to_string());

        let mut parser = KotlinParser::new();
        let tree = parser.parse(src);
        let call = call_at(&tree, src, "manifest(1)");

        assert!(call_shape_query(
            &ws.index,
            "Main.kt",
            &tree,
            src,
            call,
            &resolve::CompletenessFacts::complete()
        )
        .is_none());
    }

    #[test]
    fn call_shape_query_allows_defaults_before_trailing_lambda() {
        let src = "fun span(name: String = \"x\", block: () -> Unit) {}\nfun main() { span { } }\n";
        let mut ws = Workspace::new();
        ws.assume_index_complete_for_tests();
        ws.open("Main.kt", src.to_string());

        let mut parser = KotlinParser::new();
        let tree = parser.parse(src);
        let call = call_at(&tree, src, "span { }");

        assert!(call_shape_query(
            &ws.index,
            "Main.kt",
            &tree,
            src,
            call,
            &resolve::CompletenessFacts::complete()
        )
        .is_none());
    }

    #[test]
    fn call_shape_query_allows_trailing_lambda_on_wrapped_call_expression() {
        let src = "fun throttleDelay(a: Int, b: Int, c: Int, d: Int, block: () -> Unit) {}\nfun main() { throttleDelay(1, 2, 3, 4) { } }\n";
        let mut ws = Workspace::new();
        ws.assume_index_complete_for_tests();
        ws.open("Main.kt", src.to_string());

        let mut parser = KotlinParser::new();
        let tree = parser.parse(src);
        let call = call_at(&tree, src, "throttleDelay(1, 2, 3, 4) { }");

        assert!(call_shape_query(
            &ws.index,
            "Main.kt",
            &tree,
            src,
            call,
            &resolve::CompletenessFacts::complete()
        )
        .is_none());
    }

    #[test]
    fn call_shape_query_declines_named_argument_calls() {
        let src = "fun buildNote(noteId: String, parentId: String? = null, modifiedId: String? = null, modifiedParentId: String? = null) {}\nfun main() { buildNote(noteId = \"a\", modifiedId = \"b\") }\n";
        let mut ws = Workspace::new();
        ws.assume_index_complete_for_tests();
        ws.open("Main.kt", src.to_string());

        let mut parser = KotlinParser::new();
        let tree = parser.parse(src);
        let call = call_at(&tree, src, "buildNote(noteId = \"a\", modifiedId = \"b\")");

        assert!(call_shape_query(
            &ws.index,
            "Main.kt",
            &tree,
            src,
            call,
            &resolve::CompletenessFacts::complete()
        )
        .is_none());
    }

    #[test]
    fn call_shape_query_uses_same_file_overload_set_for_members() {
        let src = "class Service {\n    fun executeWebhook(info: String, isNew: Boolean) {}\n    private fun executeWebhook(info: Int) {}\n    fun run() { executeWebhook(1) }\n}\n";
        let mut ws = Workspace::new();
        ws.assume_index_complete_for_tests();
        ws.open("Main.kt", src.to_string());

        let mut parser = KotlinParser::new();
        let tree = parser.parse(src);
        let call = call_at(&tree, src, "executeWebhook(1)");

        assert!(call_shape_query(
            &ws.index,
            "Main.kt",
            &tree,
            src,
            call,
            &resolve::CompletenessFacts::complete()
        )
        .is_none());
    }

    #[test]
    fn call_shape_query_declines_vararg_targets() {
        let src =
            "fun collect(prefix: String, vararg names: String) {}\nfun main() { collect() }\n";
        let mut ws = Workspace::new();
        ws.assume_index_complete_for_tests();
        ws.open("Main.kt", src.to_string());

        let mut parser = KotlinParser::new();
        let tree = parser.parse(src);
        let call = call_at(&tree, src, "collect()");

        assert!(call_shape_query(
            &ws.index,
            "Main.kt",
            &tree,
            src,
            call,
            &resolve::CompletenessFacts::complete()
        )
        .is_none());
    }

    #[test]
    fn call_shape_query_declines_member_calls_backed_only_by_free_function_fallback() {
        let src = "class Items\nclass Bag(val items: Items)\nfun map(a: Int, b: Int, c: Int) {}\nfun main(b: Bag) { b.items.map() }\n";
        let mut ws = Workspace::new();
        ws.assume_index_complete_for_tests();
        ws.open("Main.kt", src.to_string());

        let mut parser = KotlinParser::new();
        let tree = parser.parse(src);
        let call = call_at(&tree, src, "map()");

        assert!(call_shape_query(
            &ws.index,
            "Main.kt",
            &tree,
            src,
            call,
            &resolve::CompletenessFacts::complete()
        )
        .is_none());
    }

    #[test]
    fn call_shape_query_reports_wrong_arity_for_known_member_call() {
        let src = "class Greeter { fun ping() {} }\nfun main(g: Greeter) { g.ping(1) }\n";
        let mut ws = Workspace::new();
        ws.assume_index_complete_for_tests();
        ws.open("Main.kt", src.to_string());

        let mut parser = KotlinParser::new();
        let tree = parser.parse(src);
        let call = call_at(&tree, src, "ping(1)");

        let query = call_shape_query(
            &ws.index,
            "Main.kt",
            &tree,
            src,
            call,
            &resolve::CompletenessFacts::complete(),
        )
        .expect("call-shape query");
        assert_eq!(query.symbol, "ping");
        assert_eq!(query.arg_count, 1);
        assert_eq!(query.arities, vec![0]);
    }

    #[test]
    fn call_shape_query_keeps_applicable_import_alias_extension() {
        let src = "package app\nimport lib.Value\nimport lib.compat as oldApi\nfun use(v: Value): Int = v.oldApi()\n";
        let mut ws = Workspace::new();
        ws.assume_index_complete_for_tests();
        ws.open("Api.kt", "package lib\nclass Value { fun oldApi(s: String): String = s }\nfun Value.compat(): Int = 1\n".to_string());
        ws.open("Main.kt", src.to_string());
        let mut parser = KotlinParser::new();
        let tree = parser.parse(src);
        let call = call_at(&tree, src, "oldApi()");
        assert!(call_shape_query(
            &ws.index,
            "Main.kt",
            &tree,
            src,
            call,
            &resolve::CompletenessFacts::complete(),
        )
        .is_none());
    }

    #[test]
    fn call_shape_query_reports_wrong_arity_for_generic_receiver_extension() {
        let src = r#"
class Throwable
class Result<T>
class Account
fun account(): Account = Account()
fun <R> runCatching(block: () -> R): Result<R> = TODO()
fun <T> Result<T>.getOrThrow(): T = TODO()
fun main() { runCatching { account() }.getOrThrow(1) }
"#;
        let mut ws = Workspace::new();
        ws.assume_index_complete_for_tests();
        ws.open("Main.kt", src.to_string());

        let mut parser = KotlinParser::new();
        let tree = parser.parse(src);
        let call = call_at(&tree, src, "getOrThrow(1)");

        let query = call_shape_query(
            &ws.index,
            "Main.kt",
            &tree,
            src,
            call,
            &resolve::CompletenessFacts::complete(),
        )
        .expect("call-shape query");
        assert_eq!(query.symbol, "getOrThrow");
        assert_eq!(query.arg_count, 1);
        assert_eq!(query.arities, vec![0]);
    }

    #[test]
    fn call_shape_query_reports_argument_type_mismatch_for_top_level_call() {
        let src = r#"
class Cat
class Dog
fun adopt(cat: Cat) {}
fun main() { adopt(Dog()) }
"#;
        let mut ws = Workspace::new();
        ws.assume_index_complete_for_tests();
        ws.open("Main.kt", src.to_string());

        let mut parser = KotlinParser::new();
        let tree = parser.parse(src);
        let call = call_at(&tree, src, "adopt(Dog())");

        let query = call_shape_query(
            &ws.index,
            "Main.kt",
            &tree,
            src,
            call,
            &resolve::CompletenessFacts::complete(),
        )
        .expect("call-shape query");
        assert_eq!(query.symbol, "adopt");
        assert_eq!(query.arg_count, 1);
        assert_eq!(query.argument_types, Some(vec!["Dog".to_string()]));
    }

    #[test]
    fn call_shape_query_reports_argument_type_mismatch_for_known_member_call() {
        let src = r#"
class Cat
class Dog
class Shelter {
    fun adopt(cat: Cat) {}
}
fun main(s: Shelter) { s.adopt(Dog()) }
"#;
        let mut ws = Workspace::new();
        ws.assume_index_complete_for_tests();
        ws.open("Main.kt", src.to_string());

        let mut parser = KotlinParser::new();
        let tree = parser.parse(src);
        let call = call_at(&tree, src, "adopt(Dog())");

        let query = call_shape_query(
            &ws.index,
            "Main.kt",
            &tree,
            src,
            call,
            &resolve::CompletenessFacts::complete(),
        )
        .expect("call-shape query");
        assert_eq!(query.symbol, "adopt");
        assert_eq!(query.arg_count, 1);
        assert_eq!(query.argument_types, Some(vec!["Dog".to_string()]));
    }

    #[test]
    fn call_shape_query_reports_argument_type_mismatch_for_generic_receiver_extension() {
        let src = r#"
class Throwable
class Result<T>(val value: T)
class Account
fun account(): Account = Account()
fun <R> runCatching(block: () -> R): Result<R> = Result(block())
fun <T> Result<T>.report(error: Throwable) {}
fun main() { runCatching { account() }.report(account()) }
"#;
        let mut ws = Workspace::new();
        ws.assume_index_complete_for_tests();
        ws.open("Main.kt", src.to_string());

        let mut parser = KotlinParser::new();
        let tree = parser.parse(src);
        let call = call_at(&tree, src, "report(account())");

        let query = call_shape_query(
            &ws.index,
            "Main.kt",
            &tree,
            src,
            call,
            &resolve::CompletenessFacts::complete(),
        )
        .expect("call-shape query");
        assert_eq!(query.symbol, "report");
        assert_eq!(query.arg_count, 1);
        assert_eq!(query.argument_types, Some(vec!["Account".to_string()]));
    }

    #[test]
    fn call_shape_query_declines_top_level_calls_when_visible_package_world_is_incomplete() {
        let lib = "package kotlin.collections\nclass List<T>\nfun <T> listOf(element: T): List<T> = TODO()\n";
        let src = "fun main() { listOf() }\n";
        let mut ws = Workspace::new();
        ws.open("Lib.kt", lib.to_string());
        ws.open("Main.kt", src.to_string());

        let mut parser = KotlinParser::new();
        let tree = parser.parse(src);
        let call = call_at(&tree, src, "listOf()");

        assert!(call_shape_query(
            &ws.index,
            "Main.kt",
            &tree,
            src,
            call,
            &resolve::CompletenessFacts::default()
        )
        .is_none());
    }

    #[test]
    fn call_shape_query_declines_member_mismatch_when_generic_receiver_extension_infers_concrete_type(
    ) {
        let src = r#"
class UserId
class Pagination(val zedToken: String)
open class Consistency
class AtLeastAsFreshAs(val token: String) : Consistency()
class ForceConsistency(val inner: AtLeastAsFreshAs) : Consistency()
class Repo {
    fun queryParticipantDocumentsWithShareLink(
        userId: UserId,
        limit: Int,
        pagination: Pagination?,
        consistencyOverride: Consistency?
    ) {}
}
fun <T, R> T.let(block: (T) -> R): R = block(this)
fun main(repo: Repo, userId: UserId, pagination: Pagination?) {
    val consistencyOverride = pagination?.zedToken?.let { ForceConsistency(AtLeastAsFreshAs(it)) }
    repo.queryParticipantDocumentsWithShareLink(userId, 1, pagination, consistencyOverride)
}
"#;
        let mut ws = Workspace::new();
        ws.assume_index_complete_for_tests();
        ws.open("Main.kt", src.to_string());

        let mut parser = KotlinParser::new();
        let tree = parser.parse(src);
        let call = call_at(
            &tree,
            src,
            "queryParticipantDocumentsWithShareLink(userId, 1, pagination, consistencyOverride)",
        );

        assert!(call_shape_query(
            &ws.index,
            "Main.kt",
            &tree,
            src,
            call,
            &resolve::CompletenessFacts::complete()
        )
        .is_none());
    }

    const COMPILED_OWNER: &str = "/compiled/a/Person.java";

    #[test]
    fn compiler_companion_api_uses_binary_identity_for_java_kotlin_and_generic_results() {
        use ktcore::extensions::{CompilerClassInput, ExtensionStore};
        use ktcore::project_model::{
            FileOrigin, ProjectScope, ScopeVisibility, SourceRepresentation,
        };
        use std::{collections::BTreeMap, sync::Arc};
        let owner_file = "/compiler-api/Model.kt";
        let kotlin_file = "/compiler-api/Use.kt";
        let java_file = "/compiler-api/Use.java";
        let source = include_str!("../../ktcore/tests/fixtures/classfile/serialization/Model.kt");
        let mut ws = Workspace::new();
        let kotlin = "package pluginfixture\nfun use() { Model.serializer() }";
        let java =
            "package pluginfixture; class Use { void use() { Model.Companion.serializer(); } }";
        for (file, text) in [
            (owner_file, source),
            (kotlin_file, kotlin),
            (java_file, java),
            (
                "/compiler-api/KSerializer.kt",
                "package kotlinx.serialization\ninterface KSerializer<T>",
            ),
        ] {
            ws.open(file, text.into());
            ws.index.set_file_origin(
                file,
                FileOrigin {
                    origin_id: "selected".into(),
                    representation: SourceRepresentation::Source,
                    project_scope: Some(ProjectScope {
                        module: "app".into(),
                        source_set: "main".into(),
                    }),
                },
            );
        }
        ws.index.replace_scope_visibilities(vec![ScopeVisibility {
            scope: ProjectScope {
                module: "app".into(),
                source_set: "main".into(),
            },
            ordered_origins: vec!["selected".into()],
        }]);
        ws.assume_index_complete_for_tests();
        let mut parser = KotlinParser::new();
        let tree = parser.parse(source);
        let mut store = ExtensionStore::default();
        store.configure_provider("current-compiler", true, Arc::from("{}"));
        store.set_namespace_coverage(Knowledge::Found(()));
        store
            .replace_source_declarations(
                &ws.index,
                owner_file,
                Arc::from(source),
                Knowledge::Found(()),
                crate::language::extension_source_facts(
                    &ws.index.scoped_for_file(owner_file),
                    owner_file,
                    &tree,
                    source,
                ),
            )
            .unwrap();
        let owner = {
            let mut read = store
                .begin(&ws.index, "current-compiler", owner_file)
                .unwrap();
            let Knowledge::Found(owner) = read.resolve_type("pluginfixture.Model") else {
                panic!("source owner")
            };
            owner
        };
        let classes = [
            (
                include_bytes!("../../ktcore/tests/fixtures/classfile/serialization/Model.class")
                    .as_slice(),
                false,
            ),
            (
                include_bytes!(
                    "../../ktcore/tests/fixtures/classfile/serialization/Model$Companion.class"
                )
                .as_slice(),
                true,
            ),
        ]
        .into_iter()
        .map(|(bytes, companion)| CompilerClassInput {
            class: Arc::new(ktcore::classfile::decode("selected", bytes).unwrap()),
            companion,
            value_namespace: false,
            kotlin_methods: if companion {
                ["serializer()Lkotlinx/serialization/KSerializer;".into()]
                    .into_iter()
                    .collect()
            } else {
                Default::default()
            },
        })
        .collect();
        store
            .replace_compiler_classes(&ws.index, &owner, classes)
            .unwrap();
        let mut read = store
            .begin(&ws.index, "current-compiler", owner_file)
            .unwrap();
        for class in read.compiler_classes(&owner).unwrap() {
            read.admit_compiler_class(&class).unwrap();
        }
        let pending = read.finish();
        store.commit(pending).unwrap();
        ws.index.replace_contributions(BTreeMap::from([(
            owner_file.into(),
            vec![store
                .contributions("current-compiler", owner_file)
                .unwrap()
                .clone()],
        )]));
        let mut identity = None;
        for (file, text) in [(java_file, java), (kotlin_file, kotlin)] {
            let scoped = ws.index.scoped_for_file(file);
            let parent = Type::class("Model", Some("pluginfixture".into()));
            let receiver = ktcore::jvm::compiler_field_type(&scoped, &parent, "Companion").unwrap();
            assert_eq!(
                ktcore::jvm::compiler_companion_type(&scoped, &parent),
                Some(receiver.clone())
            );
            let candidates = ktcore::jvm::member_candidates(&scoped, &receiver, "serializer");
            let [candidate] = candidates.as_slice() else {
                panic!("one generated method")
            };
            let current = candidate.identity(&scoped).unwrap();
            assert!(matches!(
                current.class.input,
                ktcore::jvm::InputIdentity::Binary { .. }
            ));
            assert_eq!(current.class.internal_name, "pluginfixture/Model$Companion");
            assert_eq!(candidate.definition(), None);
            if let Some(identity) = &identity {
                assert_eq!(identity, &current);
            }
            identity = Some(current);
            let ty = ktcore::jvm::projected_return_type_in_context(
                &scoped,
                &receiver,
                *candidate,
                &infer::FileCtx::new("pluginfixture".into(), Vec::new()),
            )
            .unwrap();
            assert_eq!(ty.name(), Some("KSerializer"));
            assert_eq!(ty.args()[0].name(), Some("Model"));
            let offset = text.find("serializer").unwrap() + "ser".len();
            let result = if file.ends_with(".java") {
                let tree = crate::java::JavaParser::new().parse(text);
                let index = crate::binary_diagnostics::BinaryDiagnostics {
                    index: &scoped,
                    inventory: &|_| None,
                    source_members: &|owner| {
                        owner.path.as_ref() == java_file
                            && crate::java::declaration_headers_complete(&tree, text)
                    },
                    value_namespace: &|owner| owner.path.as_ref() == java_file,
                    property_access: &|_| None,
                };
                crate::java::completion_query(&index, file, text, &tree, offset, 100).unwrap()
            } else {
                let tree = parser.parse(text);
                let mut pending = vec![tree.root_node()];
                let call = loop {
                    let node = pending.pop().expect("Kotlin serializer call");
                    if node.kind() == "call_expression"
                        && node_text(node, text) == "Model.serializer()"
                    {
                        break node;
                    }
                    let mut cursor = node.walk();
                    pending.extend(node.named_children(&mut cursor));
                };
                let inferred = infer::infer(
                    &scoped,
                    call,
                    text,
                    &infer::FileCtx::from_tree_in_file(&tree, text, file),
                );
                assert_eq!(inferred, ty);
                completion_query(&scoped, &mut parser, file, text, offset, 100).unwrap()
            };
            assert_eq!(
                result
                    .candidates
                    .iter()
                    .filter(|candidate| candidate.label == "serializer")
                    .count(),
                1,
                "{file}: {:?}",
                result
                    .candidates
                    .iter()
                    .map(|item| &item.label)
                    .collect::<Vec<_>>()
            );
        }
        for java in [
            "package pluginfixture; class Use { void use() { Object Model = null; Model.Companion.serializer(); } }",
            "package pluginfixture; class Use { void use() { class Model {} Model.Companion.serializer(); } }",
            "package pluginfixture; class Use { void use(Object Model) { Model.Companion.serializer(); } }",
            "package pluginfixture; class Use { void use() { Object Model = ; Model.Companion.serializer(); } }",
        ] {
            ws.open(java_file, java.into());
            let tree = crate::java::JavaParser::new().parse(java);
            let scoped = ws.index.scoped_for_file(java_file);
            let index = crate::binary_diagnostics::BinaryDiagnostics {
                index: &scoped,
                inventory: &|_| None,
                source_members: &|owner| owner.path.as_ref() == java_file && owner.sym.name == "Use",
                value_namespace: &|owner| owner.path.as_ref() == java_file && owner.sym.name == "Use",
                property_access: &|_| None,
            };
            let result = crate::java::completion_query(
                &index, java_file, java, &tree,
                java.find("serializer").unwrap() + "ser".len(), 100,
            );
            assert!(result.is_none_or(|query| query.candidates.iter().all(|candidate| candidate.label != "serializer")), "{java}");
        }
        // Exercise the host snapshot path too: a Java source must retain a Java tree, and
        // replacing only its body must not erase the independent Kotlin producer facts.
        let full_java_file = "/compiler-api/JavaUse.java";
        let full_java =
            include_str!("../../ktcore/tests/fixtures/classfile/serialization/JavaUse.java");
        ws.open(full_java_file, full_java.into());
        ws.index.set_file_origin(
            full_java_file,
            FileOrigin {
                origin_id: "selected".into(),
                representation: SourceRepresentation::Source,
                project_scope: Some(ProjectScope {
                    module: "app".into(),
                    source_set: "main".into(),
                }),
            },
        );
        let full_classes: Vec<_> = [
            (
                include_bytes!(
                    "../../ktcore/tests/fixtures/classfile/serialization/ModelFull.class"
                )
                .as_slice(),
                false,
            ),
            (
                include_bytes!(
                    "../../ktcore/tests/fixtures/classfile/serialization/CompanionFull.class"
                )
                .as_slice(),
                true,
            ),
        ]
        .into_iter()
        .map(|(bytes, companion)| CompilerClassInput {
            class: Arc::new(ktcore::classfile::decode("selected", bytes).unwrap()),
            companion,
            value_namespace: false,
            kotlin_methods: if companion {
                ["serializer()Lkotlinx/serialization/KSerializer;".into()]
                    .into_iter()
                    .collect()
            } else {
                Default::default()
            },
        })
        .collect();
        let mut host = crate::analysis_extensions::AnalysisExtensions::new(Vec::new());
        host.accept_compiler_api(&ws.index, crate::compile::CompilerApiSnapshot {
            sources: BTreeMap::from([(owner_file.into(), Arc::from(source)), (full_java_file.into(), Arc::from(full_java))]),
            classes: BTreeMap::from([
                (owner_file.into(), full_classes),
                (full_java_file.into(), vec![CompilerClassInput {
                    class: Arc::new(ktcore::classfile::decode("selected", include_bytes!("../../ktcore/tests/fixtures/classfile/serialization/JavaUse.class")).unwrap()),
                    companion: false, value_namespace: true, kotlin_methods: Default::default(),
                }]),
            ]),
        }).unwrap();
        host.publish(&mut ws.index);
        let java_owner = ws
            .index
            .lookup_type("JavaUse")
            .into_iter()
            .find(|entry| entry.path.as_ref() == full_java_file)
            .unwrap();
        assert!(
            ktcore::jvm::compiler_value_namespace(
                &ws.index.scoped_for_file(full_java_file),
                &java_owner
            ),
            "actual Javac namespace must survive host normalization"
        );
        for expression in ["Model.Companion.seri", "Model.Companion."] {
            let edited = full_java.replace("Model.Companion.serializer()", expression);
            let tree = crate::java::JavaParser::new().parse(&edited);
            ws.open(full_java_file, edited.clone());
            host.replace_sources(
                &ws.index,
                vec![(
                    full_java_file.into(),
                    Arc::from(edited.as_str()),
                    tree.clone(),
                )],
            );
            host.publish(&mut ws.index);
            let kotlin = ws.index.scoped_for_file(kotlin_file);
            let companion = ktcore::jvm::compiler_companion_type(
                &kotlin,
                &Type::class("Model", Some("pluginfixture".into())),
            )
            .expect("consumer body edit keeps current producer");
            assert_eq!(
                ktcore::jvm::member_candidates(&kotlin, &companion, "serializer").len(),
                1
            );
            let scoped = ws.index.scoped_for_file(full_java_file);
            let offset = edited.find(expression).unwrap() + expression.len();
            for proven_namespace in [true, false] {
                let index = crate::binary_diagnostics::BinaryDiagnostics {
                    index: &scoped,
                    inventory: &|_| None,
                    source_members: &|_| false,
                    value_namespace: &|owner| {
                        proven_namespace && ktcore::jvm::compiler_value_namespace(&scoped, owner)
                    },
                    property_access: &|_| None,
                };
                let result = crate::java::completion_query(
                    &index,
                    full_java_file,
                    &edited,
                    &tree,
                    offset,
                    100,
                )
                .unwrap();
                assert_eq!(
                    result
                        .candidates
                        .iter()
                        .any(|candidate| candidate.label == "serializer"),
                    proven_namespace,
                    "{expression}: namespace={proven_namespace}"
                );
            }
        }
        store.clear_compiled_inputs();
        assert!(store
            .contributions("current-compiler", owner_file)
            .is_none());
    }
    const COMPILED_BASE: &str = "/compiled/common/Base.java";
    const COMPILED_USE: &str = "/compiled/a/Use.java";
    const COMPILED_OTHER_SCOPE: &str = "/compiled/b/Use.java";

    // Pure normalized-fact controls, not a Gradle task-success or classfile-freshness oracle.
    // A later native compiler control must independently establish the generated override bytes.
    fn compiled_override_workspace(providers: &[&str]) -> Workspace {
        compiled_override_workspace_with_owner(
            providers,
            "package oracle; public class Person extends Base { private String name; }",
        )
    }

    fn compiled_override_workspace_with_owner(providers: &[&str], owner_source: &str) -> Workspace {
        use ktcore::classfile::{ClassFileFacts, MemberFacts};
        use ktcore::extensions::ExtensionStore;
        use ktcore::jvm::{ClassIdentity, InputIdentity};
        use ktcore::project_model::{
            FileOrigin, ProjectScope, ScopeVisibility, SourceRepresentation,
        };
        use std::collections::BTreeMap;
        use std::sync::Arc;

        let mut ws = Workspace::new();
        let sources = [
            (
                COMPILED_BASE,
                "package oracle; public class Base { public String getName() { return null; } public String getName(int n) { return null; } public String otherName() { return null; } }",
                "common",
            ),
            (COMPILED_OWNER, owner_source, "a"),
            (
                "/compiled/a/Other.java",
                "package oracle; public class Other { public String getName() { return null; } }",
                "a",
            ),
            (
                COMPILED_USE,
                "package oracle; class Use { void run(Person p, Base b, Other o) { p.getName(); p.getName(1); p.otherName(); b.getName(); o.getName(); p.getName().length(); } }",
                "a",
            ),
            (
                "/compiled/b/Person.java",
                "package oracle; public class Person extends Base {}",
                "b",
            ),
            (
                COMPILED_OTHER_SCOPE,
                "package oracle; class Use { void run(Person p) { p.getName(); } }",
                "b",
            ),
            // Minimal parsed JDK declaration model isolates result-type lookup from indexing.
            // These are existing String declarations, not synthesized members on Person.
            (
                "/compiled/jdk/String.java",
                "package java.lang; public final class String { public int length() { return 0; } }",
                "jdk",
            ),
        ];
        for (file, source, _) in sources {
            ws.open(file, source.into());
        }
        let scope = |module: &str| ProjectScope {
            module: module.into(),
            source_set: "main".into(),
        };
        for (file, _, module) in sources {
            ws.index.set_file_origin(
                file,
                FileOrigin {
                    origin_id: module.into(),
                    representation: SourceRepresentation::Source,
                    project_scope: (module != "jdk").then(|| scope(module)),
                },
            );
        }
        ws.index.replace_scope_visibilities(
            ["a", "b", "common"]
                .into_iter()
                .map(|module| ScopeVisibility {
                    scope: scope(module),
                    ordered_origins: vec![module.into(), "common".into(), "jdk".into()],
                })
                .collect(),
        );
        ws.assume_index_complete_for_tests();
        let source = ws.doc_text(COMPILED_OWNER).unwrap();
        let tree = crate::java::JavaParser::new().parse(&source);
        let declarations = crate::language::extension_source_facts(
            &ws.index.scoped_for_file(COMPILED_OWNER),
            COMPILED_OWNER,
            &tree,
            &source,
        );
        let mut store = ExtensionStore::default();
        store
            .replace_source_declarations(
                &ws.index,
                COMPILED_OWNER,
                Arc::from(source),
                Knowledge::Found(()),
                declarations,
            )
            .unwrap();
        store.set_namespace_coverage(Knowledge::Found(()));
        for provider in providers {
            store.configure_provider(provider, true, Arc::from("{}"));
        }
        let owner = {
            let mut read = store
                .begin(&ws.index, providers[0], COMPILED_OWNER)
                .unwrap();
            let Knowledge::Found(owner) = read.resolve_type("oracle.Person") else {
                panic!("selected owner")
            };
            owner
        };
        let class = Arc::new(ClassFileFacts {
            identity: ClassIdentity {
                origin_id: "a".into(),
                internal_name: "oracle/Person".into(),
                input: InputIdentity::Binary {
                    content_sha256: [7; 32],
                },
            },
            major_version: 61,
            minor_version: 0,
            access_flags: 0x0021,
            super_name: Some(Arc::from("oracle/Base")),
            interfaces: Vec::new(),
            signature: None,
            source_file: Some(Arc::from("Person.java")),
            attribute_names: Vec::new(),
            inner_classes: Vec::new(),
            fields: Vec::new(),
            methods: vec![MemberFacts {
                name: Arc::from("getName"),
                descriptor: Arc::from("()Ljava/lang/String;"),
                access_flags: 0x0001,
                signature: None,
                attribute_names: Vec::new(),
            }],
        });
        store
            .replace_compiled_input(&ws.index, &owner, Some(class))
            .unwrap();
        for provider in providers {
            let mut read = store.begin(&ws.index, provider, COMPILED_OWNER).unwrap();
            let Knowledge::Found(class) = read.compiled_class(&owner).unwrap() else {
                panic!("accepted compiled input")
            };
            for method in read.compiled_methods(&class).unwrap() {
                assert!(read.admit_java_method(&method).unwrap().is_found());
            }
            let pending = read.finish();
            store.commit(pending).unwrap();
        }
        let mut sets = BTreeMap::new();
        sets.insert(
            COMPILED_OWNER.into(),
            providers
                .iter()
                .map(|provider| {
                    store
                        .contributions(provider, COMPILED_OWNER)
                        .unwrap()
                        .clone()
                })
                .collect(),
        );
        ws.index.replace_contributions(sets);
        ws
    }

    fn compiled_usage(ws: &mut Workspace, file: &str, text: &str) -> usize {
        let source = ws.doc_text(file).unwrap();
        source.find(text).unwrap() + text.find('.').unwrap() + 1
    }

    fn compiled_declaration(ws: &Workspace, file: &str, name: &str, arity: u8) -> Def {
        let entries = ws.index.lookup_by_name(name);
        let entry = entries
            .iter()
            .find(|entry| entry.path.as_ref() == file && entry.sym.arity == Some(arity))
            .unwrap();
        Def {
            file: file.into(),
            start_byte: entry.sym.start_byte,
            end_byte: entry.sym.end_byte,
        }
    }

    #[test]
    fn compiled_override_has_one_completion_known_result_and_no_fake_source_target() {
        let mut ws = compiled_override_workspace(&["compiled-one"]);
        let offset = compiled_usage(&mut ws, COMPILED_USE, "p.getName()");
        assert!(ws.goto_definition(COMPILED_USE, offset).is_empty());
        assert!(ws.references(COMPILED_USE, offset, true).is_empty());
        assert!(ws.prepare_rename(COMPILED_USE, offset).is_none());
        assert!(ws.rename(COMPILED_USE, offset, "changed").is_none());
        let resolved = ws.resolved_symbol_query(COMPILED_USE, offset).unwrap();
        assert_eq!(resolved.reference().status_label(), "ok");
        assert_eq!(
            resolved.hover_markdown().as_deref(),
            Some("```java\ngetName(): String\n```")
        );
        assert!(resolved.targets.is_empty());
        assert!(resolved.entry.is_none());

        let completion = ws
            .completion_query(COMPILED_USE, offset + "getName".len())
            .unwrap();
        let items: Vec<_> = completion
            .candidates
            .iter()
            .filter(|item| item.label == "getName")
            .collect();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].arity, Some(0));
        assert_eq!(items[0].container.as_deref(), Some("Person"));
        let source = ws.doc_text(COMPILED_USE).unwrap();
        let chain = source.find("p.getName().length()").unwrap() + "p.getName().le".len();
        let result_completion = ws.completion_query(COMPILED_USE, chain).unwrap();
        assert_eq!(result_completion.status_label(), "ok");
        assert!(result_completion
            .candidates
            .iter()
            .any(|item| item.label == "length"));
    }

    #[test]
    fn compiled_override_preserves_authored_overloads_owners_scopes_and_applied_rename() {
        let mut ws = compiled_override_workspace(&["compiled-one"]);
        {
            let offset = compiled_usage(&mut ws, COMPILED_USE, "p.getName(1)");
            let source = ws.doc_text(COMPILED_USE).unwrap();
            let tree = crate::java::JavaParser::new().parse(&source);
            let scoped = ws.index.scoped_for_file(COMPILED_USE);
            let selected =
                crate::java::jvm_call_selection_at(&scoped, COMPILED_USE, &source, &tree, offset)
                    .unwrap();
            assert_eq!(
                selected.applicability,
                ktcore::members::Applicability::Applicable
            );
            assert!(
                matches!(
                    selected.candidates.as_slice(),
                    [ktcore::jvm::CallableCandidate::Authored(_)]
                ),
                "arity-rejected compiled getter leaves the legitimate authored overload alone"
            );
            assert_eq!(selected.navigation_candidates().len(), 1);
        }
        for (file, needle, target_file, name, arity) in [
            (COMPILED_USE, "p.getName(1)", COMPILED_BASE, "getName", 1),
            (COMPILED_USE, "p.otherName()", COMPILED_BASE, "otherName", 0),
            (COMPILED_USE, "b.getName()", COMPILED_BASE, "getName", 0),
            (
                COMPILED_USE,
                "o.getName()",
                "/compiled/a/Other.java",
                "getName",
                0,
            ),
            (
                COMPILED_OTHER_SCOPE,
                "p.getName()",
                COMPILED_BASE,
                "getName",
                0,
            ),
        ] {
            let offset = compiled_usage(&mut ws, file, needle);
            let target = compiled_declaration(&ws, target_file, name, arity);
            assert_eq!(
                ws.goto_definition(file, offset),
                vec![target],
                "{file}: {needle}"
            );
        }
        let base = compiled_declaration(&ws, COMPILED_BASE, "getName", 0);
        let generated_use = compiled_usage(&mut ws, COMPILED_USE, "p.getName()");
        let references = ws.references(COMPILED_BASE, base.start_byte, true);
        assert!(!references
            .iter()
            .any(|site| site.file == COMPILED_USE && site.start_byte == generated_use));
        // Base b = new Person() dispatches to the generated getter. Renaming only Base
        // and its authored references would silently change that behavior, so refuse both APIs.
        let base_use = compiled_usage(&mut ws, COMPILED_USE, "b.getName()");
        let other_scope_use = compiled_usage(&mut ws, COMPILED_OTHER_SCOPE, "p.getName()");
        for (file, offset) in [
            (COMPILED_BASE, base.start_byte),
            (COMPILED_USE, base_use),
            (COMPILED_OTHER_SCOPE, other_scope_use),
        ] {
            assert!(ws.prepare_rename(file, offset).is_none(), "{file}");
            assert!(ws.rename(file, offset, "support").is_none(), "{file}");
        }
        // A different overload/name/owner still has a fully editable family. Apply every edit
        // and compare whole buffers, including the generated owner's unchanged source.
        for (target_file, name, arity, old_decl, new_decl, old_use, new_use) in [
            (
                COMPILED_BASE,
                "getName",
                1,
                "getName(int n)",
                "support(int n)",
                "p.getName(1)",
                "p.support(1)",
            ),
            (
                COMPILED_BASE,
                "otherName",
                0,
                "otherName()",
                "support()",
                "p.otherName()",
                "p.support()",
            ),
            (
                "/compiled/a/Other.java",
                "getName",
                0,
                "getName()",
                "support()",
                "o.getName()",
                "o.support()",
            ),
        ] {
            let target = compiled_declaration(&ws, target_file, name, arity);
            assert!(ws.prepare_rename(target_file, target.start_byte).is_some());
            let edits = ws
                .rename(target_file, target.start_byte, "support")
                .unwrap();
            assert!(edits.iter().all(|edit| edit.file != COMPILED_OWNER));
            for file in [
                COMPILED_BASE,
                COMPILED_USE,
                COMPILED_OTHER_SCOPE,
                COMPILED_OWNER,
                "/compiled/a/Other.java",
            ] {
                let before = ws.doc_text(file).unwrap();
                let expected = if file == target_file {
                    before.replacen(old_decl, new_decl, 1)
                } else if file == COMPILED_USE {
                    before.replace(old_use, new_use)
                } else {
                    before.clone()
                };
                let actual = crate::edit::apply_to_text(file, &before, &edits).unwrap();
                assert_eq!(actual, expected, "whole buffer {file}: {name}/{arity}");
            }
        }
    }

    #[test]
    fn compiled_override_rename_preserves_a_different_selected_parent_origin() {
        use ktcore::project_model::{FileOrigin, ProjectScope, SourceRepresentation};
        let mut ws = compiled_override_workspace(&["compiled-one"]);
        let file = "/compiled/b/Base.java";
        let source =
            "package oracle; public class Base { public String getName() { return null; } }";
        ws.open(file, source.into());
        ws.index.set_file_origin(
            file,
            FileOrigin {
                origin_id: "b".into(),
                representation: SourceRepresentation::Source,
                project_scope: Some(ProjectScope {
                    module: "b".into(),
                    source_set: "main".into(),
                }),
            },
        );
        let target = compiled_declaration(&ws, file, "getName", 0);
        let usage = compiled_usage(&mut ws, COMPILED_OTHER_SCOPE, "p.getName()");
        assert_eq!(
            ws.goto_definition(COMPILED_OTHER_SCOPE, usage),
            vec![target.clone()]
        );
        let generated = compiled_usage(&mut ws, COMPILED_USE, "p.getName()");
        assert!(ws.goto_definition(COMPILED_USE, generated).is_empty());
        assert!(ws.prepare_rename(file, target.start_byte).is_some());
        let edits = ws.rename(file, target.start_byte, "support").unwrap();
        for check in [
            file,
            COMPILED_OTHER_SCOPE,
            COMPILED_BASE,
            COMPILED_OWNER,
            COMPILED_USE,
        ] {
            let before = ws.doc_text(check).unwrap();
            let expected = if check == file {
                before.replacen("getName()", "support()", 1)
            } else if check == COMPILED_OTHER_SCOPE {
                before.replace("p.getName()", "p.support()")
            } else {
                before.clone()
            };
            let actual = crate::edit::apply_to_text(check, &before, &edits).unwrap();
            assert_eq!(actual, expected, "whole buffer {check}");
        }
    }

    #[test]
    fn compiled_override_rename_preserves_static_and_private_authored_members() {
        let mut ws = compiled_override_workspace(&["compiled-one"]);
        for (file, source) in [
            (
                "/compiled/a/StaticControl.java",
                "package oracle; class StaticControl { public static String getName() { return null; } void use() { getName(); } }",
            ),
            (
                "/compiled/a/PrivateControl.java",
                "package oracle; class PrivateControl { private String getName() { return null; } void use() { getName(); } }",
            ),
        ] {
            open_compiled_dispatch_source(&mut ws, file, source);
            let target = compiled_declaration(&ws, file, "getName", 0);
            assert!(ws.prepare_rename(file, target.start_byte).is_some());
            let edits = ws.rename(file, target.start_byte, "support").unwrap();
            for check in [
                file,
                COMPILED_BASE,
                COMPILED_OWNER,
                COMPILED_USE,
                COMPILED_OTHER_SCOPE,
            ] {
                let before = ws.doc_text(check).unwrap();
                let expected = if check == file {
                    before.replace("getName", "support")
                } else {
                    before.clone()
                };
                let actual = crate::edit::apply_to_text(check, &before, &edits).unwrap();
                assert_eq!(actual, expected, "whole buffer {check}");
            }
        }
    }

    #[test]
    fn compiled_same_member_admitted_by_two_providers_is_one_callable() {
        let mut ws = compiled_override_workspace(&["compiled-one", "compiled-two"]);
        let offset = compiled_usage(&mut ws, COMPILED_USE, "p.getName()");
        let source = ws.doc_text(COMPILED_USE).unwrap();
        let tree = crate::java::JavaParser::new().parse(&source);
        let scoped = ws.index.scoped_for_file(COMPILED_USE);
        let selection =
            crate::java::jvm_call_selection_at(&scoped, COMPILED_USE, &source, &tree, offset)
                .unwrap();
        assert_eq!(
            selection.applicability,
            ktcore::members::Applicability::Applicable
        );
        assert_eq!(selection.candidates.len(), 1);
        assert!(selection.candidates[0].definition().is_none());
        assert!(ktcore::jvm::projected_return_type_in_context(
            &scoped,
            &Type::class("Person", Some("oracle".into())),
            selection.candidates[0],
            &infer::FileCtx::new("oracle".into(), Vec::new())
        )
        .is_some());
        let resolved = ws.resolved_symbol_query(COMPILED_USE, offset).unwrap();
        assert_eq!(resolved.reference().status_label(), "ok");
        assert!(resolved.targets.is_empty());
        assert!(ws.rename(COMPILED_USE, offset, "changed").is_none());
        assert_eq!(ws.index.contributions_for_file(COMPILED_OWNER).len(), 2);
    }

    #[test]
    fn compiled_unknown_authored_competitor_is_retained_without_base_fallback() {
        let mut ws = compiled_override_workspace(&["compiled-one"]);
        let base = compiled_declaration(&ws, COMPILED_BASE, "getName", 0);
        let mut symbols: Vec<_> = ws
            .index
            .entries_for_file(COMPILED_BASE)
            .iter()
            .map(|entry| (*entry.sym).clone())
            .collect();
        symbols
            .iter_mut()
            .find(|symbol| symbol.start_byte == base.start_byte)
            .unwrap()
            .arity = None;
        ws.index
            .replace_file(COMPILED_BASE, symbols, Tier::Volatile);
        let offset = compiled_usage(&mut ws, COMPILED_USE, "p.getName()");
        let source = ws.doc_text(COMPILED_USE).unwrap();
        let tree = crate::java::JavaParser::new().parse(&source);
        let scoped = ws.index.scoped_for_file(COMPILED_USE);
        let selection =
            crate::java::jvm_call_selection_at(&scoped, COMPILED_USE, &source, &tree, offset)
                .unwrap();
        assert_eq!(
            selection.applicability,
            ktcore::members::Applicability::Unknown
        );
        assert_eq!(selection.candidates.len(), 2);
        assert!(selection.navigation_candidates().is_empty());
        assert!(ws.goto_definition(COMPILED_USE, offset).is_empty());
        assert!(ws.references(COMPILED_USE, offset, true).is_empty());
        assert!(ws.prepare_rename(COMPILED_USE, offset).is_none());
        assert!(ws.rename(COMPILED_USE, offset, "changed").is_none());
        let resolved = ws.resolved_symbol_query(COMPILED_USE, offset).unwrap();
        assert_eq!(resolved.reference().status_label(), "unknown");
        assert!(resolved.targets.is_empty());
        assert!(resolved.hover_markdown().is_none());
    }

    fn open_compiled_dispatch_source(ws: &mut Workspace, file: &str, source: &str) {
        use ktcore::project_model::{FileOrigin, ProjectScope, SourceRepresentation};
        ws.open(file, source.into());
        ws.index.set_file_origin(
            file,
            FileOrigin {
                origin_id: "a".into(),
                representation: SourceRepresentation::Source,
                project_scope: Some(ProjectScope {
                    module: "a".into(),
                    source_set: "main".into(),
                }),
            },
        );
    }

    fn assert_compiled_dispatch_value(ws: &mut Workspace, file: &str, offset: usize) {
        let resolved = ws.resolved_symbol_query(file, offset).unwrap();
        assert_eq!(resolved.reference().status_label(), "ok", "{file}:{offset}");
        assert_eq!(
            resolved.hover_markdown().as_deref(),
            Some("```java\ngetName(): String\n```")
        );
        assert!(resolved.targets.is_empty());
        assert!(ws.goto_definition(file, offset).is_empty());
        assert!(ws.references(file, offset, true).is_empty());
        assert!(ws.prepare_rename(file, offset).is_none());
        assert!(ws.rename(file, offset, "changed").is_none());
        let signature = ws.signature_help(file, offset + "getName(".len()).unwrap();
        let mut labels: Vec<_> = signature
            .signatures
            .iter()
            .map(|item| item.label.as_str())
            .collect();
        labels.sort();
        assert_eq!(
            labels,
            vec!["getName(): String", "getName(p1: int): String"]
        );
        let source = ws.doc_text(file).unwrap();
        let length = source[offset..].find("length").unwrap() + offset;
        let completion = ws.completion_query(file, length + 2).unwrap();
        assert!(
            completion
                .candidates
                .iter()
                .any(|item| item.label == "length"),
            "{file}"
        );
    }

    fn assert_compiled_dispatch_unknown(ws: &mut Workspace, file: &str, offset: usize) {
        let source = ws.doc_text(file).unwrap();
        let tree = crate::java::JavaParser::new().parse(&source);
        let scoped = ws.index.scoped_for_file(file);
        let selected =
            crate::java::jvm_call_selection_at(&scoped, file, &source, &tree, offset).unwrap();
        assert_eq!(
            selected.applicability,
            ktcore::members::Applicability::Unknown,
            "parsed syntax lacks a value-namespace closure"
        );
        assert!(selected.candidates.iter().any(|candidate| matches!(
            candidate,
            ktcore::jvm::CallableCandidate::Jvm {
                owner: ktcore::jvm::JvmOwner::Compiled { .. },
                ..
            }
        )));
        assert!(selected.consumable_candidates().is_empty());
        assert!(selected.navigation_candidates().is_empty());
        let resolved = ws.resolved_symbol_query(file, offset).unwrap();
        assert_eq!(
            resolved.reference().status_label(),
            "unknown",
            "{file}:{offset}"
        );
        assert!(resolved.targets.is_empty());
        assert!(resolved.hover_markdown().is_none());
        assert!(ws.goto_definition(file, offset).is_empty());
        assert!(ws.references(file, offset, true).is_empty());
        assert!(ws.prepare_rename(file, offset).is_none());
        assert!(ws.rename(file, offset, "changed").is_none());
        assert!(ws.signature_help(file, offset + "getName(".len()).is_none());
        let length = source[offset..].find("length").unwrap() + offset;
        assert!(
            !ws.completion_query(file, length + 2)
                .is_some_and(|query| query.candidates.iter().any(|item| item.label == "length")),
            "no useful fabricated return on {file}"
        );
    }

    #[test]
    fn compiled_dispatch_parsed_parameter_local_own_inherited_and_new_values_keep_getter() {
        let mut ws = compiled_override_workspace(&["compiled-one"]);
        let sources = [
            (
                "/compiled/a/ParameterUse.java",
                "package oracle; class ParameterUse { void run(Person p) { p.getName().length(); } }",
            ),
            (
                "/compiled/a/NewUse.java",
                "package oracle; class NewUse { void run() { new Person().getName().length(); } }",
            ),
            (
                "/compiled/a/LocalUse.java",
                "package oracle; class LocalUse { void run() { Person Person = new Person(); Person.getName().length(); } }",
            ),
            (
                "/compiled/a/OwnFieldUse.java",
                "package oracle; class OwnFieldUse { Person Person; void run() { Person.getName().length(); } }",
            ),
            (
                "/compiled/a/Holder.java",
                "package oracle; public class Holder { public Person Person; }",
            ),
            (
                "/compiled/a/InheritedUse.java",
                "package oracle; class InheritedUse extends Holder { void run() { Person.getName().length(); } }",
            ),
        ];
        for (file, source) in sources {
            open_compiled_dispatch_source(&mut ws, file, source);
        }
        for (file, source) in sources
            .into_iter()
            .filter(|(_, source)| source.contains("getName"))
        {
            assert_compiled_dispatch_value(&mut ws, file, source.find("getName").unwrap());
        }
    }

    #[test]
    fn compiled_dispatch_type_spelling_unknown_import_and_incomplete_parent_do_not_invent_value() {
        let mut ws = compiled_override_workspace(&["compiled-one"]);
        for (file, source) in [
            (
                "/compiled/a/TypeUse.java",
                "package oracle; class TypeUse { void run() { Person.getName().length(); } }",
            ),
            (
                "/compiled/a/ImportedUse.java",
                "package oracle; import static missing.Values.Person; class ImportedUse { void run() { Person.getName().length(); } }",
            ),
            (
                "/compiled/a/UncertainUse.java",
                "package oracle; class UncertainUse extends Missing { void run() { Person.getName().length(); } }",
            ),
        ] {
            open_compiled_dispatch_source(&mut ws, file, source);
            assert_compiled_dispatch_unknown(&mut ws, file, source.find("getName").unwrap());
        }
        // Authored static calls retain their existing selection policy, independent of case.
        let file = "/compiled/a/StaticUse.java";
        let source = "package oracle; class staticApi { public static String stable() { return null; } } class StaticUse { void run() { staticApi.stable(); } }";
        open_compiled_dispatch_source(&mut ws, file, source);
        let offset = source.rfind("stable").unwrap();
        let target = compiled_declaration(&ws, file, "stable", 0);
        assert_eq!(ws.goto_definition(file, offset), vec![target]);
        assert!(ws.signature_help(file, offset + "stable(".len()).is_some());
    }

    #[test]
    fn compiled_dispatch_implicit_instance_and_static_context_use_one_gate_including_signature_fallback(
    ) {
        // Normalized host facts deliberately contain only the admitted generated getter;
        // this test does not prove these source bytes match a successful compiler output.
        let source = "package oracle; public class Person extends Base { private String name; void instanceProbe() { getName().length(); } static void staticProbe() { getName().length(); } }";
        let mut ws = compiled_override_workspace_with_owner(&["compiled-one"], source);
        open_compiled_dispatch_source(
            &mut ws,
            "/compiled/a/Free.kt",
            "package oracle\nfun getName(): String = \"decoy\"\n",
        );
        assert!(
            ws.index
                .lookup_by_name("getName")
                .iter()
                .any(|entry| entry.sym.container.is_none()),
            "real unrelated top-level candidate"
        );
        let instance = source.find("instanceProbe").unwrap();
        let instance = instance + source[instance..].find("getName").unwrap();
        let static_call = source.find("staticProbe").unwrap();
        let static_call = static_call + source[static_call..].find("getName").unwrap();
        assert_compiled_dispatch_value(&mut ws, COMPILED_OWNER, instance);
        assert_compiled_dispatch_unknown(&mut ws, COMPILED_OWNER, static_call);
    }

    #[test]
    fn compiled_incomplete_completion_keeps_proven_values_including_inherited_fields() {
        let mut ws = compiled_override_workspace_with_owner(
            &["compiled-one"],
            "package oracle; public class Person { private String name; }",
        );
        open_compiled_dispatch_source(
            &mut ws,
            "/compiled/a/Holder.java",
            "package oracle; public class Holder { public Person person; }",
        );
        for (file, source) in [
            (
                "/compiled/a/ParameterCompletion.java",
                "package oracle; class ParameterCompletion { void run(Person p) { p.ge } }",
            ),
            (
                "/compiled/a/LocalCompletion.java",
                "package oracle; class LocalCompletion { void run() { Person Person = new Person(); Person.ge } }",
            ),
            (
                "/compiled/a/OwnCompletion.java",
                "package oracle; class OwnCompletion { Person person; void run() { person.ge } }",
            ),
            (
                "/compiled/a/InheritedCompletion.java",
                "package oracle; class InheritedCompletion extends Holder { void run() { person.ge } }",
            ),
            (
                "/compiled/a/NewCompletion.java",
                "package oracle; class NewCompletion { void run() { new Person().ge } }",
            ),
        ] {
            open_compiled_dispatch_source(&mut ws, file, source);
            let query = ws
                .completion_query(file, source.rfind(".ge").unwrap() + 3)
                .unwrap();
            assert_eq!(query.status_label(), "ok", "{file}");
            assert_eq!(
                query
                    .candidates
                    .iter()
                    .filter(|item| item.label == "getName")
                    .count(),
                1,
                "{file}"
            );
        }
    }

    #[test]
    fn compiled_incomplete_completion_withholds_unproven_type_and_import_receivers() {
        let mut ws = compiled_override_workspace_with_owner(
            &["compiled-one"],
            "package oracle; public class Person { private String name; }",
        );
        for (file, source) in [
            (
                "/compiled/a/TypeCompletion.java",
                "package oracle; class TypeCompletion { void run() { Person.ge } }",
            ),
            (
                "/compiled/a/ImportCompletion.java",
                "package oracle; import static missing.Values.Person; class ImportCompletion { void run() { Person.ge } }",
            ),
            (
                "/compiled/a/ParentCompletion.java",
                "package oracle; class ParentCompletion extends Missing { void run() { Person.ge } }",
            ),
        ] {
            open_compiled_dispatch_source(&mut ws, file, source);
            let query = ws
                .completion_query(file, source.rfind(".ge").unwrap() + 3)
                .unwrap();
            assert_eq!(query.status_label(), "unknown", "{file}");
            assert!(
                !query.candidates.iter().any(|item| item.label == "getName"),
                "{file}"
            );
        }
    }

    #[test]
    fn compiled_rejected_call_does_not_resolve_to_a_top_level_property() {
        let mut ws = compiled_override_workspace(&["compiled-one"]);
        ws.open(COMPILED_BASE, "package oracle; public class Base {}".into());
        let free = "/compiled/a/RejectedFree.kt";
        open_compiled_dispatch_source(&mut ws, free, "val getName: String = \"decoy\"\n");
        assert!(
            ws.index.lookup_by_name("getName").iter().any(|entry| {
                entry.path.as_ref() == free
                    && entry.sym.kind == SymbolKind::Property
                    && entry.sym.container.is_none()
            }),
            "real parsed top-level value, not a fake candidate"
        );

        let offset = compiled_usage(&mut ws, COMPILED_USE, "p.getName(1)");
        let source = ws.doc_text(COMPILED_USE).unwrap();
        let tree = crate::java::JavaParser::new().parse(&source);
        let scoped = ws.index.scoped_for_file(COMPILED_USE);
        let selected =
            crate::java::jvm_call_selection_at(&scoped, COMPILED_USE, &source, &tree, offset)
                .unwrap();
        assert_eq!(
            selected.applicability,
            ktcore::members::Applicability::Rejected
        );
        assert_eq!(selected.candidates.len(), 1);
        assert!(matches!(
            selected.candidates[0],
            ktcore::jvm::CallableCandidate::Jvm {
                owner: ktcore::jvm::JvmOwner::Compiled { .. },
                ..
            }
        ));
        assert!(ws.goto_definition(COMPILED_USE, offset).is_empty());

        let resolved = ws.resolved_symbol_query(COMPILED_USE, offset).unwrap();
        assert_eq!(resolved.reference().symbol(), Some("getName"));
        assert_eq!(resolved.reference().status_label(), "unknown");
        assert_eq!(
            resolved.reference().reason_labels(),
            vec!["compiler-producer-incomplete"]
        );
        assert!(resolved.targets.is_empty());
        assert!(resolved.entry.is_none());
        assert!(resolved.hover_markdown().is_none());

        // A genuine zero-argument compiled call still exposes its signature without a fake Def.
        let valid = compiled_usage(&mut ws, COMPILED_USE, "p.getName()");
        let resolved = ws.resolved_symbol_query(COMPILED_USE, valid).unwrap();
        assert_eq!(resolved.reference().status_label(), "ok");
        assert_eq!(
            resolved.hover_markdown().as_deref(),
            Some("```java\ngetName(): String\n```")
        );
        assert!(resolved.targets.is_empty());
        assert!(resolved.entry.is_none());
    }

    #[test]
    fn compiled_private_parent_does_not_become_a_generated_call_source_target() {
        let mut ws = compiled_override_workspace(&["compiled-one"]);
        ws.open(
            COMPILED_BASE,
            "package oracle; public class Base { private String getName() { return null; } }"
                .into(),
        );
        assert!(ws
            .index
            .entries_for_file(COMPILED_BASE)
            .iter()
            .any(|entry| {
                entry.sym.name == "getName"
                    && entry.sym.source_member.visibility()
                        == Some(ktcore::extensions::Visibility::Private)
            }));
        let offset = compiled_usage(&mut ws, COMPILED_USE, "p.getName()");
        let targets = {
            let source = ws.doc_text(COMPILED_USE).unwrap();
            let tree = crate::java::JavaParser::new().parse(&source);
            let scoped = ws.index.scoped_for_file(COMPILED_USE);
            let selected =
                crate::java::jvm_call_selection_at(&scoped, COMPILED_USE, &source, &tree, offset)
                    .unwrap();
            assert_eq!(
                selected.applicability,
                ktcore::members::Applicability::Applicable
            );
            assert!(matches!(
                selected.candidates.as_slice(),
                [ktcore::jvm::CallableCandidate::Jvm {
                    owner: ktcore::jvm::JvmOwner::Compiled { .. },
                    ..
                }]
            ));
            selected
                .navigation_candidates()
                .iter()
                .filter_map(|candidate| candidate.definition())
                .collect::<Vec<_>>()
        };
        assert!(
            ws.goto_definition(COMPILED_USE, offset).is_empty(),
            "an inaccessible private ancestor is not the generated method source"
        );
        assert!(targets.is_empty());
        assert!(ws.references(COMPILED_USE, offset, true).is_empty());
        assert!(ws.prepare_rename(COMPILED_USE, offset).is_none());
        assert!(ws.rename(COMPILED_USE, offset, "changed").is_none());
        let resolved = ws.resolved_symbol_query(COMPILED_USE, offset).unwrap();
        assert_eq!(resolved.reference().status_label(), "ok");
        assert!(resolved.targets.is_empty());
        assert!(resolved.entry.is_none());
        assert_eq!(
            resolved.hover_markdown().as_deref(),
            Some("```java\ngetName(): String\n```")
        );
        let completion = ws
            .completion_query(COMPILED_USE, offset + "getName".len())
            .unwrap();
        let items: Vec<_> = completion
            .candidates
            .iter()
            .filter(|item| item.label == "getName")
            .collect();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].container.as_deref(), Some("Person"));
        let source = ws.doc_text(COMPILED_USE).unwrap();
        let chain = source.find("p.getName().length()").unwrap() + "p.getName().le".len();
        let result_completion = ws.completion_query(COMPILED_USE, chain).unwrap();
        assert_eq!(result_completion.status_label(), "ok");
        assert!(result_completion
            .candidates
            .iter()
            .any(|item| item.label == "length"));
    }

    #[test]
    fn authored_private_parent_single_call_navigation_and_applied_rename_are_preserved() {
        let mut ws = Workspace::new();
        let file = "/compiled/plain/Base.java";
        let source = "class Base { private String getName() { return null; } String readName() { return getName(); } }";
        ws.open(file, source.into());
        ws.assume_index_complete_for_tests();
        assert!(!ws.index.may_have_compiled_callables());
        let target = compiled_declaration(&ws, file, "getName", 0);
        let offset = source.rfind("getName").unwrap();
        assert_eq!(ws.goto_definition(file, offset), vec![target.clone()]);
        let refs = ws.references(file, offset, true);
        assert_eq!(refs.len(), 2);
        assert!(refs.contains(&target));
        assert!(refs
            .iter()
            .any(|site| site.file == file && site.start_byte == offset));
        assert!(ws.prepare_rename(file, offset).is_some());
        let edits = ws.rename(file, offset, "support").unwrap();
        assert!(edits.iter().all(|edit| edit.file == file));
        assert_eq!(
            crate::edit::apply_to_text(file, source, &edits).unwrap(),
            source.replace("getName", "support"),
        );
    }

    #[test]
    fn compiled_unknown_and_ambiguous_mixed_calls_do_not_restart_authored_name_lookup() {
        for unknown_overload in [true, false] {
            let mut ws = compiled_override_workspace(&["compiled-one"]);
            if unknown_overload {
                let declaration = compiled_declaration(&ws, COMPILED_BASE, "getName", 0);
                let mut symbols: Vec<_> = ws
                    .index
                    .entries_for_file(COMPILED_BASE)
                    .iter()
                    .map(|entry| (*entry.sym).clone())
                    .collect();
                symbols
                    .iter_mut()
                    .find(|symbol| symbol.start_byte == declaration.start_byte)
                    .unwrap()
                    .arity = None;
                ws.index
                    .replace_file(COMPILED_BASE, symbols, Tier::Volatile);
            } else {
                // Deliberately mixed host facts: an authored static candidate cannot be erased
                // by an admitted instance callable. This is not a successful compiler oracle.
                ws.open(COMPILED_BASE,
                    "package oracle; public class Base { public static String getName() { return null; } }".into());
            }
            let free = "/compiled/a/MixedFree.kt";
            let free_source = "val getName: String = \"decoy\"\nfun readName(): String = getName\n";
            open_compiled_dispatch_source(&mut ws, free, free_source);
            let declaration = ws
                .index
                .lookup_by_name("getName")
                .into_iter()
                .find(|entry| entry.path.as_ref() == free && entry.sym.container.is_none())
                .unwrap();
            let decoy = Def {
                file: free.into(),
                start_byte: declaration.sym.start_byte,
                end_byte: declaration.sym.end_byte,
            };
            // Ordinary authored Kotlin resolution remains useful and names the real property.
            let authored = ws
                .resolved_symbol_query(free, free_source.rfind("getName").unwrap())
                .unwrap();
            assert_eq!(authored.reference().status_label(), "ok");
            assert_eq!(authored.targets, vec![decoy]);
            assert!(authored.hover_markdown().is_some());

            let offset = compiled_usage(&mut ws, COMPILED_USE, "p.getName()");
            let source = ws.doc_text(COMPILED_USE).unwrap();
            let tree = crate::java::JavaParser::new().parse(&source);
            let scoped = ws.index.scoped_for_file(COMPILED_USE);
            let selected =
                crate::java::jvm_call_selection_at(&scoped, COMPILED_USE, &source, &tree, offset)
                    .unwrap();
            assert_eq!(
                selected.applicability,
                if unknown_overload {
                    ktcore::members::Applicability::Unknown
                } else {
                    ktcore::members::Applicability::Applicable
                }
            );
            assert_eq!(
                selected.candidates.len(),
                2,
                "a genuine competing candidate is retained"
            );
            assert!(selected
                .candidates
                .iter()
                .any(|candidate| matches!(candidate, ktcore::jvm::CallableCandidate::Authored(_))));
            assert!(selected.candidates.iter().any(|candidate| matches!(
                candidate,
                ktcore::jvm::CallableCandidate::Jvm {
                    owner: ktcore::jvm::JvmOwner::Compiled { .. },
                    ..
                }
            )));

            assert!(
                ws.goto_definition(COMPILED_USE, offset).is_empty(),
                "mixed pool must not collapse to its sole authored Def: unknown={unknown_overload}"
            );
            assert!(ws.references(COMPILED_USE, offset, true).is_empty());
            assert!(ws.prepare_rename(COMPILED_USE, offset).is_none());
            assert!(ws.rename(COMPILED_USE, offset, "changed").is_none());

            let resolved = ws.resolved_symbol_query(COMPILED_USE, offset).unwrap();
            assert_eq!(
                resolved.reference().status_label(),
                "unknown",
                "unknown={unknown_overload}"
            );
            assert_eq!(
                resolved.reference().reason_labels(),
                vec!["compiler-producer-incomplete"]
            );
            assert!(resolved.targets.is_empty());
            assert!(resolved.entry.is_none());
            assert!(resolved.hover_markdown().is_none());
        }
    }
}

#[cfg(test)]
mod recovered_property_proof_tests {
    use super::*;

    #[test]
    fn recovered_file_lookup_preserves_replacement_and_scope_without_global_scan() {
        use crate::index::Tier;
        use crate::symbol::IndexedSymbol;
        use std::cell::Cell;

        struct FileScope {
            index: Index,
            global_scans: Cell<usize>,
        }
        impl InferenceIndex for FileScope {
            fn lookup_by_name(&self, name: &str) -> LookupEntries<'_> {
                InferenceIndex::lookup_by_name(&self.index, name)
            }
            fn members_of(&self, name: &str) -> LookupEntries<'_> {
                InferenceIndex::members_of(&self.index, name)
            }
            fn extensions_for(&self, name: &str) -> LookupEntries<'_> {
                InferenceIndex::extensions_for(&self.index, name)
            }
            fn entries_for_file(&self, file: &str) -> Vec<Entry> {
                if file == "Hidden.java" {
                    Vec::new()
                } else {
                    self.index.entries_for_file(file)
                }
            }
            fn entries_with_prefix<'a>(
                &'a self,
                prefix: &'a str,
                top_level_only: bool,
            ) -> Box<dyn Iterator<Item = &'a Entry> + 'a> {
                if prefix.is_empty() {
                    self.global_scans.set(self.global_scans.get() + 1);
                }
                Box::new(
                    self.index
                        .entries_with_prefix(prefix, top_level_only)
                        .filter(|entry| entry.path.as_ref() != "Hidden.java"),
                )
            }
            fn scope_is_complete(&self) -> bool {
                false
            }
        }
        let class =
            |name: &str| IndexedSymbol::new(name, SymbolKind::Class, "demo", None, 0, name.len());
        let mut original = Index::new();
        for (file, name) in [
            ("Use.java", "OldOwner"),
            ("Visible.java", "Visible"),
            ("Hidden.java", "Hidden"),
        ] {
            original.replace_file(file, vec![class(name)], Tier::Volatile);
        }
        let scoped = FileScope {
            index: original,
            global_scans: Cell::new(0),
        };
        let mut source = Index::new();
        source.replace_file("Use.java", vec![class("NewOwner")], Tier::Volatile);
        source.replace_file("Extra.java", vec![class("Extra")], Tier::Volatile);
        for replaced_file in [Some("Use.java"), None] {
            let recovered = RecoveredTypeIndex {
                index: &scoped,
                source: &source,
                replaced_file,
                source_member_proofs: false,
            };
            let selected = recovered.entries_for_file("Use.java");
            assert_eq!(selected.len(), 1);
            assert_eq!(
                selected[0].sym.name,
                if replaced_file.is_some() {
                    "NewOwner"
                } else {
                    "OldOwner"
                }
            );
            assert!(!recovered.source_value_namespace_complete(&selected[0]));
            assert!(!recovered.scope_is_complete());
            assert_eq!(
                recovered.entries_for_file("Visible.java")[0].sym.name,
                "Visible"
            );
            assert!(recovered.entries_for_file("Hidden.java").is_empty());
            assert!(recovered.entries_for_file("Missing.java").is_empty());
            assert_eq!(
                recovered.entries_for_file("Extra.java").len(),
                usize::from(replaced_file.is_some())
            );
        }
        assert_eq!(
            scoped.global_scans.get(),
            0,
            "file lookup must not enumerate unrelated workspace symbols"
        );
    }

    #[test]
    fn recovered_source_index_forwards_only_unchanged_foreign_property_proofs() {
        use ktcore::extensions::source::JvmPropertyAccess;
        let mut original = Index::new();
        let owner =
            crate::symbol::IndexedSymbol::new("Parent", SymbolKind::Class, "foreign", None, 1, 7);
        let property = crate::symbol::IndexedSymbol::new(
            "key",
            SymbolKind::Property,
            "foreign",
            Some("Parent".into()),
            20,
            23,
        );
        original.replace_file(
            "Parent.kt",
            vec![owner, property],
            crate::index::Tier::Volatile,
        );
        let owner = original.lookup_type("Parent").into_iter().next().unwrap();
        let property = original.members_of("Parent").into_iter().next().unwrap();
        let declaration = |_: &Entry| Some(Type::class("Int", Some("kotlin".into())));
        let scoped = original
            .scoped_for_file("Use.java")
            .with_declaration_types(&declaration);
        let host = crate::binary_diagnostics::BinaryDiagnostics {
            index: &scoped,
            inventory: &|_| None,
            source_members: &|_| false,
            value_namespace: &|entry| entry.sym.kind == SymbolKind::Class,
            property_access: &|entry| {
                (entry.sym.kind == SymbolKind::Property).then_some(JvmPropertyAccess::Field)
            },
        };
        let replacement = Index::new();
        let recovered = RecoveredTypeIndex::replacing_file(&host, &replacement, "Use.java")
            .with_source_member_proofs(true);
        assert!(recovered.source_value_namespace_complete(owner));
        assert!(
            !recovered.source_members_complete(owner),
            "value proof cannot become method inventory proof through recovery"
        );
        assert_eq!(
            recovered.source_property_access(property),
            Some(JvmPropertyAccess::Field)
        );
        assert_eq!(
            recovered.source_declaration_type(property),
            Some(Type::class("Int", Some("kotlin".into())))
        );
        let replaced = RecoveredTypeIndex::replacing_file(&host, &replacement, "Parent.kt");
        assert!(!replaced.source_value_namespace_complete(owner));
        assert_eq!(replaced.source_property_access(property), None);
        assert_eq!(replaced.source_declaration_type(property), None);
        let mut unchanged = Index::new();
        unchanged.replace_file(
            "Parent.kt",
            vec![property.sym.as_ref().clone()],
            crate::index::Tier::Volatile,
        );
        let unchanged = RecoveredTypeIndex::replacing_file(&host, &unchanged, "Parent.kt");
        assert_eq!(
            unchanged.source_declaration_type(property),
            Some(Type::class("Int", Some("kotlin".into())))
        );
        assert_eq!(unchanged.source_property_access(property), None);
        assert!(!unchanged.source_value_namespace_complete(owner));
        let mut stale = property.clone();
        let mut symbol = stale.sym.as_ref().clone();
        symbol.start_byte += 1;
        stale.sym = std::sync::Arc::new(symbol);
        assert_eq!(recovered.source_declaration_type(&stale), None);
        assert_eq!(unchanged.source_declaration_type(&stale), None);
        let mut changed = Index::new();
        changed.replace_file(
            "Parent.kt",
            vec![stale.sym.as_ref().clone()],
            crate::index::Tier::Volatile,
        );
        let changed = RecoveredTypeIndex::replacing_file(&host, &changed, "Parent.kt");
        assert_eq!(changed.source_declaration_type(&stale), None);
        assert_eq!(
            recovered.source_property_access(&stale),
            None,
            "a synthetic or stale declaration cannot reuse an accepted foreign field proof"
        );
    }
}
