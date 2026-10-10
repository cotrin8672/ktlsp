//! JVM-visible callable facts. Authored declarations keep their source names and signatures.
use serde::{Deserialize, Serialize};

use crate::index::{Entry, InferenceIndex, Tier};
use crate::indexer::{self, TypeScope};
use crate::infer::{self, FileCtx};
use crate::parser::{child_of_kind, imports_of, name_field, node_text};
use crate::symbol::{Def, IndexedSymbol, SymbolKind};
use crate::types::{Type, TypeRef};
use tree_sitter::{Node, Tree};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum InputIdentity {
    Source { file: String, revision: u64 },
    Binary { content_sha256: [u8; 32] },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ClassIdentity {
    pub origin_id: String,
    pub internal_name: String,
    pub input: InputIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MemberIdentity {
    pub class: ClassIdentity,
    pub name: String,
    pub descriptor: String,
}

/// A plain annotation literal has a verified editable subrange. Escapes/interpolation are unknown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixedName {
    pub definition: Def,
    pub annotation_range: std::ops::Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallableFacts {
    pub name: String,
    pub params: Vec<TypeRef>,
    pub return_type: TypeRef,
    pub type_params: Vec<String>,
    pub definition: Option<Def>,
    pub fixed_name: Option<FixedName>,
    pub jvm_name_annotation: Option<TypeRef>,
    pub is_static: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceFileFacts {
    pub input_hash: u64,
    pub package: String,
    pub owner: String,
    pub owner_annotation: Option<TypeRef>,
    pub callables: Vec<CallableFacts>,
}

/// Accepted source facts are immutable and shared with inference snapshots.
#[derive(Clone, Debug)]
pub struct SourceFile {
    pub file: String,
    pub revision: u64,
    pub tier: Tier,
    pub facts: SourceFileFacts,
}

/// An admitted language surface borrows either accepted source facts or exact compiled metadata.
/// Raw public flags alone do not establish that a compiled member is callable from a language.
#[derive(Clone, Copy)]
pub enum JvmOwner<'a> {
    Source(&'a SourceFile),
    Compiled {
        class: &'a ClassIdentity,
        descriptor: &'a str,
        tier: Tier,
    },
}

impl JvmOwner<'_> {
    pub fn tier(self) -> Tier {
        match self {
            Self::Source(source) => source.tier,
            Self::Compiled { tier, .. } => tier,
        }
    }
}

#[derive(Clone, Copy)]
pub enum CallableCandidate<'a> {
    Authored(&'a Entry),
    Jvm {
        owner: JvmOwner<'a>,
        facts: &'a CallableFacts,
    },
}

impl<'a> CallableCandidate<'a> {
    pub fn name(self) -> &'a str {
        match self {
            Self::Authored(e) => &e.sym.name,
            Self::Jvm { facts, .. } => &facts.name,
        }
    }
    pub fn params(self) -> &'a [TypeRef] {
        match self {
            Self::Authored(e) => &e.sym.params,
            Self::Jvm { facts, .. } => &facts.params,
        }
    }
    pub fn definition(self) -> Option<Def> {
        match self {
            Self::Authored(e) => Some(Def {
                file: e.path.to_string(),
                start_byte: e.sym.start_byte,
                end_byte: e.sym.end_byte,
            }),
            Self::Jvm { facts, .. } => facts.definition.clone(),
        }
    }
    pub fn arity(self) -> Option<u8> {
        match self {
            Self::Authored(e) => e.sym.arity,
            Self::Jvm { facts, .. } => facts.params.len().try_into().ok(),
        }
    }
    pub fn min_arity(self) -> Option<u8> {
        match self {
            Self::Authored(e) => e.sym.min_arity,
            Self::Jvm { .. } => self.arity(),
        }
    }
    pub fn accepts_count(self, count: Option<usize>) -> bool {
        let Some(count) = count else {
            return true;
        };
        !self.min_arity().is_some_and(|min| count < min as usize)
            && !(self.arity().is_some_and(|max| count > max as usize)
                && !matches!(self, Self::Authored(e) if e.sym.has_vararg))
    }
    pub fn return_type(self) -> Option<&'a TypeRef> {
        match self {
            Self::Authored(e) => e.sym.return_type.as_ref(),
            Self::Jvm { facts, .. } => Some(&facts.return_type),
        }
    }
    pub fn identity(self, index: &dyn InferenceIndex) -> Option<MemberIdentity> {
        let Self::Jvm { owner, facts } = self else {
            return None;
        };
        let source = match owner {
            JvmOwner::Source(source) => source,
            JvmOwner::Compiled {
                class, descriptor, ..
            } => {
                return Some(MemberIdentity {
                    class: class.clone(),
                    name: facts.name.clone(),
                    descriptor: descriptor.into(),
                })
            }
        };
        if !owner_annotation_known(index, source)
            || !annotation_known(
                index,
                facts.jvm_name_annotation.as_ref(),
                &source.facts.package,
            )
        {
            return None;
        }
        let ctx = FileCtx::new(source.facts.package.clone(), Vec::new());
        let mut descriptor = "(".to_string();
        for ty in &facts.params {
            descriptor.push_str(&erased_descriptor(index, ty, &ctx)?);
        }
        descriptor.push(')');
        descriptor.push_str(&erased_descriptor(index, &facts.return_type, &ctx)?);
        let origin_id = index
            .origin_facts()
            .and_then(|origins| origins.file_origin(&source.file))
            .map(|origin| origin.origin_id.clone())
            .unwrap_or_else(|| format!("source:{}", source.file));
        Some(MemberIdentity {
            class: ClassIdentity {
                origin_id,
                internal_name: source.internal_name(),
                input: InputIdentity::Source {
                    file: source.file.clone(),
                    revision: source.revision,
                },
            },
            name: facts.name.clone(),
            descriptor,
        })
    }
}

impl SourceFile {
    pub fn internal_name(&self) -> String {
        if self.facts.package.is_empty() {
            self.facts.owner.clone()
        } else {
            format!(
                "{}/{}",
                self.facts.package.replace('.', "/"),
                self.facts.owner
            )
        }
    }
}

/// Never invent Object erasure for unresolved source types. Suspend lowering inserts its verified
/// Object return and Continuation parameter explicitly in the source producer.
fn erased_descriptor(index: &dyn InferenceIndex, ty: &TypeRef, ctx: &FileCtx) -> Option<String> {
    if ty.qualification.as_deref() == Some("java.lang") && ty.name == "Object" {
        return Some("Ljava/lang/Object;".into());
    }
    if ty.qualification.as_deref() == Some("kotlin.coroutines") && ty.name == "Continuation" {
        return Some("Lkotlin/coroutines/Continuation;".into());
    }
    let Type::Class {
        name,
        package: Some(package),
        container,
        ..
    } = infer::declared_type(index, ty, ctx)
    else {
        return None;
    };
    Some(format!(
        "L{}{name};",
        if let Some(container) = container {
            format!(
                "{}/{}$",
                package.replace('.', "/"),
                container.replace('.', "$")
            )
        } else if package.is_empty() {
            String::new()
        } else {
            format!("{}/", package.replace('.', "/"))
        }
    ))
}

/// JVM owners are language projections, never authored type declarations.
pub fn visible_owner(index: &dyn InferenceIndex, name: &str, ctx: &FileCtx) -> Option<Type> {
    let sources: Vec<_> = index
        .jvm_sources(name)
        .into_iter()
        .filter(|source| owner_annotation_known(index, source))
        .collect();
    let explicit: Vec<_> = ctx
        .imports
        .iter()
        .filter(|i| !i.wildcard && i.local_name() == Some(name))
        .collect();
    let mut packages: Vec<_> = sources
        .into_iter()
        .filter(|source| {
            let package = &source.facts.package;
            if !explicit.is_empty() {
                explicit
                    .iter()
                    .any(|i| i.path == format!("{package}.{name}"))
            } else {
                package == &ctx.package
                    || ctx
                        .imports
                        .iter()
                        .any(|i| i.wildcard && i.package() == *package)
            }
        })
        .map(|source| source.facts.package.clone())
        .collect();
    packages.sort();
    packages.dedup();
    let [package] = packages.as_slice() else {
        return None;
    };
    Some(Type::class(name, Some(package.clone())))
}

/// One borrowed candidate query for Java calls and their JVM projections.
pub fn member_candidates<'a>(
    index: &'a dyn InferenceIndex,
    receiver: &Type,
    prefix: &str,
) -> Vec<CallableCandidate<'a>> {
    let mut out: Vec<_> = crate::members::matching_candidates(
        index,
        receiver,
        crate::members::MemberName::Prefix(prefix),
        &FileCtx::new(
            receiver.package().unwrap_or_default().to_string(),
            Vec::new(),
        ),
    )
    .members
    .into_iter()
    .filter(|entry| entry.sym.kind == SymbolKind::Function)
    .map(CallableCandidate::Authored)
    .collect();
    if let (Some(name), Some(package)) = (receiver.name(), receiver.package()) {
        for source in index
            .jvm_sources(name)
            .into_iter()
            .filter(|s| s.facts.package == package && owner_annotation_known(index, s))
        {
            out.extend(
                source
                    .facts
                    .callables
                    .iter()
                    .filter(|facts| {
                        facts.name.starts_with(prefix)
                            && annotation_known(
                                index,
                                facts.jvm_name_annotation.as_ref(),
                                &source.facts.package,
                            )
                    })
                    .map(|facts| CallableCandidate::Jvm {
                        owner: JvmOwner::Source(source),
                        facts,
                    }),
            );
        }
    }
    out
}

pub struct CallSelection<'a> {
    pub candidates: Vec<CallableCandidate<'a>>,
    pub applicability: crate::members::Applicability,
}

impl<'a> CallSelection<'a> {
    pub fn navigation_candidates(&self) -> &[CallableCandidate<'a>] {
        if self.applicability == crate::members::Applicability::Rejected
            && self.candidates.len() > 1
        {
            &[]
        } else {
            &self.candidates
        }
    }
}

pub fn select_call<'a>(
    index: &dyn InferenceIndex,
    receiver: &Type,
    candidates: Vec<CallableCandidate<'a>>,
    name: &str,
    args: &crate::members::CallArguments,
    ctx: &FileCtx,
) -> CallSelection<'a> {
    use crate::members::{self, Applicability, CallMode};
    let named: Vec<_> = candidates
        .into_iter()
        .filter(|candidate| candidate.name() == name)
        .collect();
    let mut accepted = Vec::new();
    let mut uncertain = false;
    for candidate in &named {
        let applicability = match *candidate {
            CallableCandidate::Authored(entry) => {
                members::function_applicability(index, receiver, entry, args, ctx, CallMode::Java)
            }
            CallableCandidate::Jvm { owner, facts } => {
                if !candidate.accepts_count(args.count) {
                    Applicability::Rejected
                } else {
                    let package = match owner {
                        JvmOwner::Source(source) => source.facts.package.clone(),
                        JvmOwner::Compiled { class, .. } => class
                            .internal_name
                            .rsplit_once('/')
                            .map(|(package, _)| package.replace('/', "."))
                            .unwrap_or_default(),
                    };
                    let declaration_ctx = FileCtx::new(package, Vec::new());
                    members::arguments_applicability(
                        index,
                        args,
                        |position| {
                            let Some(ty) = facts.params.get(position) else {
                                return Type::Unknown;
                            };
                            if ty.qualification.is_none() && facts.type_params.contains(&ty.name) {
                                Type::Unknown
                            } else {
                                infer::declared_type(index, ty, &declaration_ctx)
                            }
                        },
                        CallMode::Java,
                        args.count.is_none() || args.trailing_lambda,
                    )
                }
            }
        };
        if applicability != Applicability::Rejected {
            accepted.push(*candidate);
            uncertain |= applicability == Applicability::Unknown;
        }
    }
    let applicability = if accepted.is_empty() {
        // Keep rejected signatures for diagnostics; navigation requires a unique declaration.
        accepted = named;
        Applicability::Rejected
    } else if uncertain {
        Applicability::Unknown
    } else {
        Applicability::Applicable
    };
    CallSelection {
        candidates: accepted,
        applicability,
    }
}

pub fn projected_return_type(candidate: CallableCandidate<'_>) -> Option<Type> {
    let CallableCandidate::Jvm { facts, .. } = candidate else {
        return None;
    };
    (facts.return_type.name == "Object"
        && facts.return_type.qualification.as_deref() == Some("java.lang"))
    .then(|| Type::class("Object", Some("java.lang".into())))
}

#[derive(Clone, Debug)]
pub struct ResolvedMember {
    pub identity: Option<MemberIdentity>,
    pub name: String,
    pub definition: Option<Def>,
    pub fixed_name: Option<FixedName>,
    pub tier: Tier,
}

impl CallableCandidate<'_> {
    pub fn resolved(self, index: &dyn InferenceIndex) -> Option<ResolvedMember> {
        let Self::Jvm { owner, facts } = self else {
            return None;
        };
        Some(ResolvedMember {
            identity: self.identity(index),
            name: facts.name.clone(),
            definition: facts.definition.clone(),
            fixed_name: facts.fixed_name.clone(),
            tier: owner.tier(),
        })
    }
}

/// The Kotlin-to-Java syntax boundary supplies facts once per accepted source input. No facade
/// class declaration or synthetic source span is inserted into the authored symbol index.
pub fn source_facts(
    file: &str,
    tree: &Tree,
    text: &str,
    symbols: &[IndexedSymbol],
) -> Option<SourceFileFacts> {
    if tree.root_node().kind() != "source_file" {
        return None;
    }
    let package = crate::parser::package_of(tree, text);
    let scope = TypeScope::with_default_imports(
        &package,
        imports_of(tree, text),
        crate::defaults::DEFAULT_IMPORT_PACKAGES,
    );
    let mut owner = std::path::Path::new(file)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| format!("{s}Kt"))
        .unwrap_or_default();
    let mut owner_annotation = None;
    let mut cursor = tree.root_node().walk();
    for node in tree.root_node().named_children(&mut cursor) {
        if node.kind() == "file_annotation" {
            if let Some((value, _)) = jvm_name(node, text) {
                owner = value;
                owner_annotation = annotation_type(node, text, &scope);
            } else if node_text(node, text).contains("JvmName") {
                return None;
            }
        }
    }
    let mut callables = Vec::new();
    for symbol in symbols
        .iter()
        .filter(|s| s.kind == SymbolKind::Function && s.container.is_none())
    {
        let Some(mut node) = tree
            .root_node()
            .descendant_for_byte_range(symbol.start_byte, symbol.end_byte)
        else {
            continue;
        };
        while node.kind() != "function_declaration" {
            let Some(parent) = node.parent() else {
                break;
            };
            node = parent;
        }
        if node.kind() != "function_declaration" || name_field(node).is_none() {
            continue;
        }
        let modifiers = child_of_kind(node, "modifiers");
        let words = modifiers.map(|m| node_text(m, text)).unwrap_or("");
        // shortcut: only verified public suspend lowering; expand when other JVM transforms have compiler controls.
        if !words.split_whitespace().any(|w| w == "suspend")
            || words
                .split_whitespace()
                .any(|w| matches!(w, "private" | "internal" | "external" | "expect"))
        {
            continue;
        }
        let header_clean = node
            .children(&mut node.walk())
            .filter(|n| n.kind() != "function_body")
            .all(|n| !n.has_error() && !n.is_missing());
        if !header_clean
            || symbol.has_vararg
            || symbol.arity != Some(symbol.params.len().min(255) as u8)
        {
            continue;
        }
        let mut name = symbol.name.clone();
        let mut fixed_name = None;
        let mut jvm_name_annotation = None;
        if let Some(modifiers) = modifiers {
            for annotation in modifiers
                .named_children(&mut modifiers.walk())
                .filter(|n| n.kind() == "annotation")
            {
                if let Some((value, range)) = jvm_name(annotation, text) {
                    name = value;
                    jvm_name_annotation = annotation_type(annotation, text, &scope);
                    fixed_name = Some(FixedName {
                        definition: Def {
                            file: file.into(),
                            start_byte: range.start,
                            end_byte: range.end,
                        },
                        annotation_range: annotation.byte_range(),
                    });
                } else if node_text(annotation, text).contains("JvmName") {
                    name.clear();
                    break;
                }
            }
        }
        if name.is_empty() {
            continue;
        }
        let mut params = Vec::new();
        if symbol.ext_receiver.is_some() {
            let mut c = node.walk();
            let receiver = node
                .named_children(&mut c)
                .take_while(|n| Some(*n) != name_field(node))
                .find_map(|n| indexer::type_ref_from(n, text, Some(&scope)));
            let Some(receiver) = receiver else {
                continue;
            };
            params.push(receiver);
        }
        params.extend(symbol.params.clone());
        let result = match &symbol.return_type {
            Some(result) => result.clone(),
            None if child_of_kind(node, "function_body")
                .is_some_and(|body| node_text(body, text).trim_start().starts_with('{')) =>
            {
                scope.type_ref("kotlin.Unit", false, Vec::new())
            }
            None => continue,
        };
        params.push(scope.type_ref("kotlin.coroutines.Continuation", false, vec![result]));
        callables.push(CallableFacts {
            name,
            params,
            return_type: scope.type_ref("java.lang.Object", false, Vec::new()),
            type_params: symbol.type_params.clone(),
            definition: Some(Def {
                file: file.into(),
                start_byte: symbol.start_byte,
                end_byte: symbol.end_byte,
            }),
            fixed_name,
            jvm_name_annotation,
            is_static: true,
        });
    }
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hash);
    (!callables.is_empty()).then_some(SourceFileFacts {
        input_hash: hash.finish(),
        package,
        owner,
        owner_annotation,
        callables,
    })
}

/// Accept only a whole, plain JVM-name annotation; spelling substrings are not semantic names.
fn jvm_name(node: Node<'_>, text: &str) -> Option<(String, std::ops::Range<usize>)> {
    let spelling = node_text(node, text).trim();
    let tail = spelling
        .strip_prefix("@file:")
        .or_else(|| spelling.strip_prefix('@'))?;
    let open = tail.find('(')?;
    if !matches!(tail[..open].trim(), "JvmName" | "kotlin.jvm.JvmName")
        || !tail.ends_with(')')
        || node.has_error()
    {
        return None;
    }
    let mut strings = Vec::new();
    fn collect(node: Node<'_>, out: &mut Vec<std::ops::Range<usize>>) {
        if node.kind() == "string_literal" {
            out.push(node.byte_range());
            return;
        }
        for child in node.named_children(&mut node.walk()) {
            collect(child, out);
        }
    }
    collect(node, &mut strings);
    let [range] = strings.as_slice() else {
        return None;
    };
    let literal = text.get(range.clone())?;
    let value = literal.strip_prefix('"')?.strip_suffix('"')?;
    if !value
        .chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        || !value
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
        || tail[open + 1..tail.len() - 1].trim() != literal
    {
        return None;
    }
    Some((value.into(), range.start + 1..range.end - 1))
}

fn annotation_type(node: Node<'_>, text: &str, scope: &TypeScope) -> Option<TypeRef> {
    let spelling = node_text(node, text).trim();
    let name = spelling
        .strip_prefix("@file:")
        .or_else(|| spelling.strip_prefix('@'))?
        .split('(')
        .next()?
        .trim();
    Some(scope.type_ref(name, false, Vec::new()))
}

fn owner_annotation_known(index: &dyn InferenceIndex, source: &SourceFile) -> bool {
    annotation_known(
        index,
        source.facts.owner_annotation.as_ref(),
        &source.facts.package,
    )
}

fn annotation_known(
    index: &dyn InferenceIndex,
    annotation: Option<&TypeRef>,
    package: &str,
) -> bool {
    let Some(annotation) = annotation else {
        return true;
    };
    if let Some(qualification) = &annotation.qualification {
        return qualification == "kotlin.jvm";
    }
    !index.lookup_type(&annotation.name).iter().any(|entry| {
        entry.sym.package != "kotlin.jvm"
            && (entry.sym.package == package
                || annotation.package_candidates.contains(&entry.sym.package))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoded_member_uses_the_same_candidate_without_a_source_location() {
        let class = crate::classfile::decode(
            "project:plugin-fixture:main",
            include_bytes!("../tests/fixtures/classfile/pluginfixture/Person.class"),
        )
        .unwrap();
        let member = class
            .methods
            .iter()
            .find(|member| member.name.as_ref() == "getName")
            .unwrap();
        assert_eq!(member.descriptor.as_ref(), "()Ljava/lang/String;");
        // This control admits only the independently compiled Java getter surface.
        let facts = CallableFacts {
            name: member.name.to_string(),
            params: Vec::new(),
            return_type: TypeRef {
                name: "String".into(),
                qualification: Some("java.lang".into()),
                ..TypeRef::default()
            },
            type_params: Vec::new(),
            definition: None,
            fixed_name: None,
            jvm_name_annotation: None,
            is_static: false,
        };
        let candidate = CallableCandidate::Jvm {
            owner: JvmOwner::Compiled {
                class: &class.identity,
                descriptor: &member.descriptor,
                tier: Tier::Durable,
            },
            facts: &facts,
        };
        let index = crate::index::Index::new();
        let identity = candidate.identity(&index).unwrap();
        assert_eq!(identity.class, class.identity);
        assert_eq!(identity.descriptor, member.descriptor.as_ref());
        assert_eq!(candidate.definition(), None);
        assert_eq!(candidate.resolved(&index).unwrap().tier, Tier::Durable);
        assert_eq!(
            crate::signature::signature_for_callable(candidate).label,
            "getName(): String"
        );
        assert_eq!(
            select_call(
                &index,
                &Type::Unknown,
                vec![candidate],
                "getName",
                &crate::members::CallArguments {
                    count: Some(0),
                    types: Vec::new(),
                    trailing_lambda: false
                },
                &FileCtx::new(String::new(), Vec::new())
            )
            .candidates
            .len(),
            1
        );
        assert!(index.lookup_by_name("getName").is_empty());
        assert!(index.lookup_type("Person").is_empty());
    }
}
