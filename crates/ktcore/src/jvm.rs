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
    pub source_property: Option<SourceProperty>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceProperty {
    pub owner: String,
    pub name: String,
    pub type_params: Vec<String>,
    pub requires_binary_witness: bool,
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
                });
            }
        };
        if (facts.source_property.is_none() && !owner_annotation_known(index, source))
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
            descriptor.push_str(&source_value_descriptor(index, facts, ty, &ctx)?);
        }
        descriptor.push(')');
        if property_setter(facts) {
            descriptor.push('V');
        } else {
            descriptor.push_str(&source_value_descriptor(
                index,
                facts,
                &facts.return_type,
                &ctx,
            )?);
        }
        let origin_id = index
            .origin_facts()
            .and_then(|origins| origins.file_origin(&source.file))
            .map(|origin| origin.origin_id.clone())
            .unwrap_or_else(|| format!("source:{}", source.file));
        Some(MemberIdentity {
            class: ClassIdentity {
                origin_id,
                internal_name: facts
                    .source_property
                    .as_ref()
                    .map(|property| {
                        format!(
                            "{}{}",
                            if source.facts.package.is_empty() {
                                String::new()
                            } else {
                                format!("{}/", source.facts.package.replace('.', "/"))
                            },
                            property.owner
                        )
                    })
                    .unwrap_or_else(|| source.internal_name()),
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
    pub fn owners(&self) -> Vec<&str> {
        let mut owners = vec![self.facts.owner.as_str()];
        for property in self
            .facts
            .callables
            .iter()
            .filter_map(|facts| facts.source_property.as_ref())
        {
            if !owners.contains(&property.owner.as_str()) {
                owners.push(&property.owner);
            }
        }
        owners
    }
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

fn source_property_entry<'a>(
    index: &'a dyn InferenceIndex,
    source: &SourceFile,
    facts: &CallableFacts,
) -> Option<&'a Entry> {
    let property = facts.source_property.as_ref()?;
    let definition = facts.definition.as_ref()?;
    let entries: Vec<_> = index
        .lookup_by_name(&property.name)
        .into_iter()
        .filter(|entry| {
            entry.path.as_ref() == source.file
                && entry.sym.kind == SymbolKind::Property
                && entry.sym.container.as_deref() == Some(&property.owner)
                && entry.sym.start_byte == definition.start_byte
                && entry.sym.end_byte == definition.end_byte
        })
        .collect();
    let [entry] = entries.as_slice() else {
        return None;
    };
    Some(*entry)
}

fn source_value_descriptor(
    index: &dyn InferenceIndex,
    facts: &CallableFacts,
    ty: &TypeRef,
    ctx: &FileCtx,
) -> Option<String> {
    if facts.source_property.as_ref().is_some_and(|property| {
        ty.qualification.is_none() && property.type_params.contains(&ty.name)
    }) {
        // The source producer admits only unbounded class parameters here.
        return Some("Ljava/lang/Object;".into());
    }
    if facts.source_property.is_some() {
        let resolved = infer::declared_type(index, ty, ctx);
        let mut non_nullable = ty.clone();
        non_nullable.nullable = false;
        let scalar_abi = infer::is_intrinsic_type_ref(index, ty, ctx)
            && crate::extensions::source::source_scalar_jvm_type(
                index,
                &non_nullable,
                ctx,
                false,
                &facts.type_params,
            )
            .is_some();
        if !scalar_abi
            && index.lookup_type(resolved.name()?).iter().any(|entry| {
                Some(entry.sym.package.as_str()) == resolved.package()
                    && entry.sym.container.as_deref() == resolved.container()
                    && entry.sym.source_member.jvm_restricted()
            })
        {
            return None;
        }
    }
    erased_descriptor(index, ty, ctx, &facts.type_params)
}

fn property_setter(facts: &CallableFacts) -> bool {
    facts.source_property.is_some()
        && facts.params.len() == 1
        && facts.return_type.name == "Unit"
        && facts.return_type.qualification.as_deref() == Some("kotlin")
        && !facts.return_type.nullable
        && facts.return_type.args.is_empty()
}

fn property_accessor_is_admitted(
    index: &dyn InferenceIndex,
    source: &SourceFile,
    facts: &CallableFacts,
    receiver: &Type,
    ctx: &FileCtx,
    position: usize,
) -> bool {
    let Some(property) = &facts.source_property else {
        return false;
    };
    let Some(entry) = source_property_entry(index, source, facts) else {
        return false;
    };
    if !crate::members::member_accessible_at(index, receiver, entry, ctx, position) {
        return false;
    }
    let owners: Vec<_> = index
        .lookup_type(&property.owner)
        .into_iter()
        .filter(|owner| owner.sym.package == source.facts.package && owner.sym.container.is_none())
        .collect();
    let [owner] = owners.as_slice() else {
        return false;
    };
    if owner.path.as_ref() != source.file {
        return false;
    }
    let Some(identity) = (CallableCandidate::Jvm {
        owner: JvmOwner::Source(source),
        facts,
    })
    .identity(index) else {
        return false;
    };
    let Some(class) = index.java_class_inventory(owner) else {
        return !property.requires_binary_witness;
    };
    if class.identity.internal_name != identity.class.internal_name {
        return false;
    }
    let methods: Vec<_> = class
        .methods
        .iter()
        .filter(|method| {
            method.name.as_ref() == facts.name
                && method.access_flags & (0x0001 | 0x0008 | 0x0040 | 0x1000) == 0x0001
        })
        .collect();
    let [method] = methods.as_slice() else {
        return false;
    };
    if method.descriptor.as_ref() != identity.descriptor {
        return false;
    }
    if property.type_params.is_empty() {
        return method.signature.is_none();
    }
    let Some(signature) = crate::classfile::signature::class_signature(&class) else {
        return false;
    };
    if signature
        .type_parameters
        .iter()
        .map(|parameter| &parameter.name)
        .ne(property.type_params.iter())
        || signature.type_parameters.iter().any(|parameter| {
            !matches!(parameter.bounds.as_slice(), [bound]
            if bound.name == "Object" && bound.qualification.as_deref() == Some("java.lang"))
        })
    {
        return false;
    }
    let Some(projected) = method.signature.as_deref().and_then(|signature_text| {
        crate::classfile::signature::method_signature(
            signature_text,
            &method.descriptor,
            &signature.type_parameters,
        )
    }) else {
        return false;
    };
    let same_type = |left: &TypeRef, right: &TypeRef| {
        left.name == right.name
            && left.qualification == right.qualification
            && left.args == right.args
    };
    projected.type_parameters.is_empty()
        && projected.parameters.len() == facts.params.len()
        && projected
            .parameters
            .iter()
            .zip(&facts.params)
            .all(|(left, right)| same_type(left, right))
        && if property_setter(facts) {
            projected.returns.is_none()
        } else {
            projected
                .returns
                .as_ref()
                .is_some_and(|ty| same_type(ty, &facts.return_type))
        }
}

/// Never invent Object erasure for unresolved source types. Suspend lowering inserts its verified
/// Object return and Continuation parameter explicitly in the source producer.
fn erased_descriptor(
    index: &dyn InferenceIndex,
    ty: &TypeRef,
    ctx: &FileCtx,
    type_parameters: &[String],
) -> Option<String> {
    if ty.qualification.is_none() && type_parameters.contains(&ty.name) {
        return None;
    }
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
    if container.is_none() && package == "kotlin" {
        // shortcut: generic array erasure remains Unknown until its element/boxing rules are covered.
        if name == "Array" {
            return None;
        }
        if crate::defaults::is_builtin_type(&name) {
            let mut non_nullable = ty.clone();
            non_nullable.nullable = false;
            let scalar = crate::extensions::source::source_scalar_jvm_type(
                index,
                &non_nullable,
                ctx,
                false,
                type_parameters,
            )?;
            use crate::extensions::{JvmType, MemberSignature};
            if scalar == JvmType::Void {
                // Unit is a value parameter here; only ordinary method returns lower it to void.
                return Some("Lkotlin/Unit;".into());
            }
            if ty.nullable && scalar != JvmType::String {
                let wrapper = match scalar {
                    JvmType::Boolean => "Boolean",
                    JvmType::Byte => "Byte",
                    JvmType::Char => "Character",
                    JvmType::Short => "Short",
                    JvmType::Int => "Integer",
                    JvmType::Long => "Long",
                    JvmType::Float => "Float",
                    JvmType::Double => "Double",
                    _ => return None,
                };
                return Some(format!("Ljava/lang/{wrapper};"));
            }
            return Some(
                MemberSignature::field(&scalar)
                    .ok()?
                    .descriptor()
                    .to_string(),
            );
        }
    }
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
        .filter(|source| source.facts.owner == name && owner_annotation_known(index, source))
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
    member_candidates_at(
        index,
        receiver,
        prefix,
        &FileCtx::new(String::new(), Vec::new()),
        0,
    )
}

pub fn member_candidates_at<'a>(
    index: &'a dyn InferenceIndex,
    receiver: &Type,
    prefix: &str,
    ctx: &FileCtx,
    position: usize,
) -> Vec<CallableCandidate<'a>> {
    if matches!(receiver, Type::Array { .. }) {
        use crate::extensions::{Dispatch, Visibility};
        let owners: Vec<_> = index
            .lookup_type("Object")
            .into_iter()
            .filter(|entry| entry.sym.package == "java.lang" && entry.sym.container.is_none())
            .collect();
        let [owner] = owners.as_slice() else {
            return Vec::new();
        };
        if owner.sym.kind != SymbolKind::Class {
            return Vec::new();
        }
        let object = Type::class("Object", Some("java.lang".into()));
        // JLS 10.7 inherits only public Object methods; clone retains its array intrinsic.
        return crate::members::matching_candidates(
            index,
            &object,
            crate::members::MemberName::Prefix(prefix),
            &FileCtx::new("java.lang".into(), Vec::new()),
            false,
        )
        .members
        .into_iter()
        .filter(|entry| {
            entry.path == owner.path
                && entry.sym.package == "java.lang"
                && entry.sym.container.as_deref() == Some("Object")
                && entry.sym.kind == SymbolKind::Function
                && entry.sym.source_member.visibility() == Some(Visibility::Public)
                && entry.sym.source_member.dispatch() == Some(Dispatch::Instance)
                && entry.sym.source_member.implementation()
                    != Some(crate::symbol::MemberImplementation::Constructor)
                && !entry.sym.source_member.jvm_restricted()
                && entry.sym.ext_receiver.is_none()
                && entry.sym.extension_receiver.is_none()
        })
        .map(CallableCandidate::Authored)
        .collect();
    }
    let source_members = crate::members::matching_candidates(
        index,
        receiver,
        crate::members::MemberName::JvmPrefix(prefix),
        ctx,
        false,
    )
    .members;
    member_candidates_from(index, receiver, prefix, ctx, position, &source_members)
}

/// Add JVM projections to an already collected source pool without walking its hierarchy again.
pub(crate) fn member_candidates_from<'a>(
    index: &'a dyn InferenceIndex,
    receiver: &Type,
    prefix: &str,
    ctx: &FileCtx,
    position: usize,
    source_members: &[&'a Entry],
) -> Vec<CallableCandidate<'a>> {
    let mut out: Vec<_> = source_members
        .iter()
        .copied()
        .filter(|entry| {
            entry.sym.kind == SymbolKind::Function
                && crate::members::member_accessible_at(index, receiver, entry, ctx, position)
        })
        .map(CallableCandidate::Authored)
        .collect();
    if let (Some(name), Some(package)) = (receiver.name(), receiver.package()) {
        for source in index
            .jvm_sources(name)
            .into_iter()
            .filter(|s| s.facts.package == package)
        {
            out.extend(
                source
                    .facts
                    .callables
                    .iter()
                    .filter(|facts| {
                        facts.name.starts_with(prefix)
                            && facts.source_property.is_none()
                            && source.facts.owner == name
                            && owner_annotation_known(index, source)
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
    // The pool has already applied visibility, unavailable names and nearest-property overrides.
    // Project only its surviving exact source definitions, including inherited properties.
    for entry in source_members
        .iter()
        .filter(|entry| entry.sym.kind == SymbolKind::Property)
    {
        let Some(owner) = entry.sym.container.as_deref() else {
            continue;
        };
        for source in index.jvm_sources(owner).into_iter().filter(|source| {
            source.file == entry.path.as_ref() && source.facts.package == entry.sym.package
        }) {
            out.extend(
                source
                    .facts
                    .callables
                    .iter()
                    .filter(|facts| {
                        facts.name.starts_with(prefix)
                            && facts.source_property.as_ref().is_some_and(|property| {
                                property.owner == owner && property.name == entry.sym.name
                            })
                            && facts.definition.as_ref().is_some_and(|definition| {
                                definition.start_byte == entry.sym.start_byte
                                    && definition.end_byte == entry.sym.end_byte
                            })
                            && property_accessor_is_admitted(
                                index, source, facts, receiver, ctx, position,
                            )
                    })
                    .map(|facts| CallableCandidate::Jvm {
                        owner: JvmOwner::Source(source),
                        facts,
                    }),
            );
        }
    }
    // Only the uniquely selected scoped authored owner can expose an admitted binary surface.
    // The generated callable has its real ClassIdentity but deliberately has no source Def.
    if index.may_have_compiled_callables() {
        if let Type::Class {
            name,
            package: Some(package),
            container,
            ..
        } = receiver
        {
            let owners: Vec<_> = index
                .lookup_type(
                    container
                        .as_deref()
                        .and_then(|container| container.split('.').next())
                        .unwrap_or(name),
                )
                .into_iter()
                .filter(|entry| {
                    entry.sym.package == *package
                        && if container.is_some() {
                            entry.sym.container.is_none()
                        } else {
                            entry.sym.container.as_ref() == container.as_ref()
                        }
                })
                .collect();
            if let [owner] = owners.as_slice() {
                if index.scope_is_complete() {
                    let definition = Def {
                        file: owner.path.to_string(),
                        start_byte: owner.sym.start_byte,
                        end_byte: owner.sym.end_byte,
                    };
                    for set in index.contributions_for_file(&definition.file) {
                        for (source_owner, class, descriptor, facts) in set.compiled_callables() {
                            if source_owner == &definition
                                && facts.name.starts_with(prefix)
                                && class.identity.internal_name
                                    == receiver_internal_name(receiver).unwrap_or_default()
                            {
                                out.push(CallableCandidate::Jvm {
                                    owner: JvmOwner::Compiled {
                                        class: &class.identity,
                                        descriptor,
                                        tier: owner.tier,
                                    },
                                    facts,
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    normalize_compiled_candidates(index, receiver, &mut out);
    out
}

fn receiver_internal_name(receiver: &Type) -> Option<String> {
    let Type::Class {
        name,
        package: Some(package),
        container,
        ..
    } = receiver
    else {
        return None;
    };
    Some(format!(
        "{}{name}",
        match container {
            Some(container) => format!(
                "{}/{}$",
                package.replace('.', "/"),
                container.replace('.', "$")
            ),
            None if package.is_empty() => String::new(),
            None => format!("{}/", package.replace('.', "/")),
        }
    ))
}

/// Static final object fields from current compiler observations retain their binary nested identity.
pub fn compiler_field_type(
    index: &dyn InferenceIndex,
    receiver: &Type,
    name: &str,
) -> Option<Type> {
    compiler_object_field_type(index, receiver, Some(name))
}

pub fn compiler_companion_type(index: &dyn InferenceIndex, receiver: &Type) -> Option<Type> {
    compiler_object_field_type(index, receiver, None)
}

pub fn compiler_object_value(index: &dyn InferenceIndex, receiver: &Type) -> bool {
    let Type::Class {
        package: Some(package),
        container: Some(container),
        ..
    } = receiver
    else {
        return false;
    };
    let root = Type::class(container, Some(package.clone()));
    compiler_companion_type(index, &root).as_ref() == Some(receiver)
}

/// This source anchor has a current complete, field-free Object-rooted Javac namespace.
/// It proves no method inventory or project-wide producer closure.
pub fn compiler_value_namespace(index: &dyn InferenceIndex, owner: &Entry) -> bool {
    if !owner.path.ends_with(".java")
        || owner.sym.container.is_some()
        || !owner.sym.type_params.is_empty()
        || !owner.sym.supertypes.is_empty()
        || !owner.sym.supertype_refs.is_empty()
    {
        return false;
    }
    let definition = Def {
        file: owner.path.to_string(),
        start_byte: owner.sym.start_byte,
        end_byte: owner.sym.end_byte,
    };
    index.contributions_for_file(&owner.path).iter().any(|set| {
        set.compiled_value_namespace(&definition)
            .is_some_and(|class| {
                class.fields.is_empty()
                    && class.super_name.as_deref() == Some("java/lang/Object")
                    && class.interfaces.is_empty()
                    && class.signature.is_none()
                    && class
                        .inner_classes
                        .iter()
                        .all(|inner| inner.inner.as_ref() != class.identity.internal_name)
                    && class
                        .attribute_names
                        .iter()
                        .all(|name| name.as_ref() != "EnclosingMethod")
            })
    })
}

fn compiler_object_field_type(
    index: &dyn InferenceIndex,
    receiver: &Type,
    name: Option<&str>,
) -> Option<Type> {
    if !index.may_have_compiled_callables() || !index.scope_is_complete() {
        return None;
    }
    let internal = receiver_internal_name(receiver)?;
    let Type::Class {
        name: owner_name,
        package: Some(package),
        container,
        ..
    } = receiver
    else {
        return None;
    };
    let root_name = container
        .as_deref()
        .and_then(|container| container.split('.').next())
        .unwrap_or(owner_name);
    let owners: Vec<_> = index
        .lookup_type(root_name)
        .into_iter()
        .filter(|entry| entry.sym.package == *package && entry.sym.container.is_none())
        .collect();
    let [owner] = owners.as_slice() else {
        return None;
    };
    let mut result = None;
    for set in index.contributions_for_file(&owner.path) {
        for (anchor, class, field, _, companion) in set.compiled_fields() {
            if anchor.start_byte != owner.sym.start_byte
                || class.identity.internal_name != internal
                || match name {
                    Some(name) => field.name.as_ref() != name,
                    None => !companion,
                }
            {
                continue;
            }
            let target = field.descriptor.strip_prefix('L')?.strip_suffix(';')?;
            let (package, binary) = target.rsplit_once('/')?;
            let (container, name) = binary.rsplit_once('$')?;
            let ty = Type::Class {
                name: name.into(),
                package: Some(package.replace('/', ".")),
                container: Some(container.replace('$', ".")),
                nullable: false,
                args: Vec::new(),
                owner: None,
            };
            if result.as_ref().is_some_and(|previous| previous != &ty) {
                return None;
            }
            result = Some(ty);
        }
    }
    result
}

fn compiled_key<'a>(
    candidate: CallableCandidate<'a>,
) -> Option<(&'a ClassIdentity, &'a str, &'a str)> {
    match candidate {
        CallableCandidate::Jvm {
            owner: JvmOwner::Compiled {
                class, descriptor, ..
            },
            facts,
        } => Some((class, facts.name.as_str(), descriptor)),
        _ => None,
    }
}

fn normalize_compiled_candidates(
    index: &dyn InferenceIndex,
    receiver: &Type,
    candidates: &mut Vec<CallableCandidate<'_>>,
) {
    if !candidates
        .iter()
        .any(|candidate| compiled_key(*candidate).is_some())
    {
        return;
    }
    // Provider ownership stays independent. Equal accepted semantic facts are one callable.
    let mut seen: crate::fxhash::FxHashMap<_, Vec<&CallableFacts>> = Default::default();
    candidates.retain(|candidate| {
        let Some(key) = compiled_key(*candidate) else {
            return true;
        };
        let CallableCandidate::Jvm { facts, .. } = *candidate else {
            unreachable!()
        };
        let existing = seen.entry(key).or_default();
        if existing.contains(&facts) {
            return false;
        }
        existing.push(facts);
        true
    });
    let compiled: Vec<_> = candidates
        .iter()
        .copied()
        .filter(|candidate| compiled_key(*candidate).is_some_and(|key| seen[&key].len() == 1))
        .collect();
    candidates.retain(|candidate| match *candidate {
        CallableCandidate::Authored(entry) => !compiled
            .iter()
            .any(|compiled| compiled_overrides_authored(index, receiver, *compiled, entry)),
        _ => true,
    });
}

fn conflicting_compiled_facts(candidates: &[CallableCandidate<'_>]) -> bool {
    let mut seen: crate::fxhash::FxHashMap<_, &CallableFacts> = Default::default();
    for candidate in candidates {
        let Some(key) = compiled_key(*candidate) else {
            continue;
        };
        let CallableCandidate::Jvm { facts, .. } = *candidate else {
            unreachable!()
        };
        if let Some(previous) = seen.insert(key, facts) {
            if previous != facts {
                return true;
            }
        }
    }
    false
}

/// Whether an admitted compiled callable overrides this selected authored member.
/// Callers must select candidates in the receiver's source-set scope first. False does not
/// prove the absence of other binary surfaces outside the currently admitted getter subset.
pub fn compiled_overrides_authored(
    index: &dyn InferenceIndex,
    receiver: &Type,
    candidate: CallableCandidate<'_>,
    inherited: &Entry,
) -> bool {
    use crate::extensions::{Dispatch, Visibility};
    use std::ops::ControlFlow;

    let CallableCandidate::Jvm {
        owner: JvmOwner::Compiled {
            class, descriptor, ..
        },
        facts,
    } = candidate
    else {
        return false;
    };
    let Type::Class {
        name,
        package: Some(package),
        container: None,
        ..
    } = receiver
    else {
        return false;
    };
    let owner = if package.is_empty() {
        name.clone()
    } else {
        format!("{}/{}", package.replace('.', "/"), name)
    };
    if class.internal_name != owner
        || descriptor != "()Ljava/lang/String;"
        || facts.is_static
        || !facts.params.is_empty()
        || !facts.type_params.is_empty()
        || facts.name != inherited.sym.name
        || inherited.sym.has_vararg
        || inherited.sym.ext_receiver.is_some()
        || inherited.sym.source_member.dispatch() != Some(Dispatch::Instance)
        || inherited.sym.source_member.jvm_restricted()
        || !matches!(
            inherited.sym.source_member.visibility(),
            Some(Visibility::Public | Visibility::Protected)
        ) && !(inherited.sym.source_member.visibility() == Some(Visibility::Package)
            && inherited.sym.package == *package)
    {
        return false;
    }
    let ctx = FileCtx::new(package.clone(), Vec::new());
    if crate::members::method_signature(index, receiver, inherited, &ctx) != Some(Vec::new()) {
        return false;
    }
    let Some(parent_owner) = inherited.sym.container.as_deref() else {
        return false;
    };
    let child_owner = infer::receiver_root_key(index, receiver);
    if child_owner.as_deref() == Some(parent_owner) && inherited.sym.package == *package {
        return false;
    }
    let found = infer::walk_supertypes(index, receiver, 32, |current| {
        if current.package() == Some(inherited.sym.package.as_str())
            && infer::receiver_root_key(index, current).as_deref() == Some(parent_owner)
        {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    })
    .is_break();
    if !found {
        return false;
    }
    let (container, simple) = parent_owner
        .rsplit_once('.')
        .map_or((None, parent_owner), |(container, simple)| {
            (Some(container.to_string()), simple)
        });
    let parent = Type::Class {
        name: simple.to_string(),
        package: Some(inherited.sym.package.clone()),
        container,
        nullable: false,
        args: Vec::new(),

        owner: None,
    };
    !infer::walk_supertypes(index, &parent, 32, |current| {
        if current.package() == Some(package.as_str())
            && infer::receiver_root_key(index, current) == child_owner
        {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    })
    .is_break()
}

/// A language adapter may prove a value receiver. A type spelling alone does not prove
/// absence of a shadowing value, especially when compiler-generated fields are unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiverKind {
    Type,
    Value,
    Unknown,
}

/// Compiled instance methods and constructor getters require a proven value receiver.
/// Existing facade lowering keeps its static dispatch policy.
pub fn select_call_for_receiver_kind<'a>(
    index: &dyn InferenceIndex,
    receiver: &Type,
    mut candidates: Vec<CallableCandidate<'a>>,
    name: &str,
    args: &crate::members::CallArguments,
    ctx: &FileCtx,
    receiver_kind: ReceiverKind,
) -> CallSelection<'a> {
    use crate::members::Applicability;
    if receiver_kind == ReceiverKind::Value
        || !candidates.iter().any(|candidate| {
            candidate.name() == name
                && matches!(candidate,
            CallableCandidate::Jvm { facts, .. } if !facts.is_static)
        })
    {
        return select_call(index, receiver, candidates, name, args, ctx);
    }
    candidates.retain(|candidate| candidate.name() == name);
    let conflicting = conflicting_compiled_facts(&candidates);
    let (mut blocked, remaining): (Vec<_>, Vec<_>) =
        candidates.into_iter().partition(|candidate| {
            candidate.name() == name
                && matches!(candidate,
            CallableCandidate::Jvm { facts, .. } if !facts.is_static)
        });
    let remaining = select_call(index, receiver, remaining, name, args, ctx);
    // A proven type cannot call the instance surface, but can still select a real static
    // overload. Unknown receiver syntax does not establish that the instance is ineligible.
    if !conflicting
        && receiver_kind == ReceiverKind::Type
        && remaining.applicability != Applicability::Rejected
        && !remaining.candidates.is_empty()
        && remaining
            .candidates
            .iter()
            .all(|candidate| match candidate {
                CallableCandidate::Authored(entry) => {
                    entry.sym.source_member.dispatch() == Some(crate::extensions::Dispatch::Static)
                }
                CallableCandidate::Jvm { facts, .. } => facts.is_static,
            })
    {
        return remaining;
    }
    let applicability = if !conflicting
        && receiver_kind == ReceiverKind::Type
        && remaining.applicability == Applicability::Rejected
    {
        Applicability::Rejected
    } else {
        Applicability::Unknown
    };
    // Keep provenance and genuine unknown competitors. Consumers must not turn removal of
    // a blocked compiled callable into a second authored/global lookup.
    blocked.extend(remaining.candidates);
    CallSelection {
        candidates: blocked,
        applicability,
        compiled_dispatch_blocked: true,
    }
}

pub struct CallSelection<'a> {
    pub candidates: Vec<CallableCandidate<'a>>,
    pub applicability: crate::members::Applicability,
    compiled_dispatch_blocked: bool,
}

impl<'a> CallSelection<'a> {
    /// An unsupported dispatch keeps provenance but cannot expose a signature or source target.
    pub fn consumable_candidates(&self) -> &[CallableCandidate<'a>] {
        if self.compiled_dispatch_blocked {
            &[]
        } else {
            &self.candidates
        }
    }

    pub fn navigation_candidates(&self) -> &[CallableCandidate<'a>] {
        if self.compiled_dispatch_blocked {
            return &[];
        }
        if (self.applicability == crate::members::Applicability::Rejected
            && self.candidates.len() > 1)
            || ((self.applicability == crate::members::Applicability::Unknown
                || self.candidates.len() > 1)
                && self.candidates.iter().any(|candidate| {
                    matches!(
                        candidate,
                        CallableCandidate::Jvm {
                            owner: JvmOwner::Compiled { .. },
                            ..
                        }
                    )
                }))
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
    if members::candidates(index, receiver, name, ctx).has_unavailable_call(args.count) {
        return CallSelection {
            candidates: Vec::new(),
            applicability: Applicability::Unknown,
            compiled_dispatch_blocked: false,
        };
    }
    let named: Vec<_> = candidates
        .into_iter()
        .filter(|candidate| candidate.name() == name)
        .collect();
    let mut accepted = Vec::new();
    let conflicting = conflicting_compiled_facts(&named);
    let mut uncertain = conflicting;
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
                            if let JvmOwner::Source(source) = owner {
                                if let Some(entry) = source_property_entry(index, source, facts) {
                                    return infer::declared_member_type(
                                        index,
                                        receiver,
                                        entry,
                                        ty,
                                        &declaration_ctx,
                                    );
                                }
                            }
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
        if conflicting {
            Applicability::Unknown
        } else {
            Applicability::Rejected
        }
    } else if uncertain {
        Applicability::Unknown
    } else {
        Applicability::Applicable
    };
    CallSelection {
        candidates: accepted,
        applicability,
        compiled_dispatch_blocked: false,
    }
}

/// Present the same JVM String through the caller's language, preserving its nullability.
/// Other nominal identities and generic arguments retain their declaration facts.
pub fn string_result_for_call_mode(
    index: &dyn InferenceIndex,
    result: Type,
    ctx: &FileCtx,
    mode: crate::members::CallMode,
) -> Type {
    let Type::Class {
        name,
        package: Some(package),
        container: None,
        args,
        ..
    } = &result
    else {
        return result;
    };
    if name != "String" || !args.is_empty() {
        return result;
    }
    let target = match (mode, package.as_str()) {
        (crate::members::CallMode::Kotlin, "java.lang") => "kotlin",
        (crate::members::CallMode::Java, "kotlin") => "java.lang",
        _ => return result,
    };
    infer::declared_type(
        index,
        &TypeRef {
            name: "String".into(),
            qualification: Some(target.into()),
            package_candidates: std::sync::Arc::new(vec![target.into()]),
            nullable: result.is_nullable(),
            ..TypeRef::default()
        },
        ctx,
    )
}

pub fn projected_return_type(candidate: CallableCandidate<'_>) -> Option<Type> {
    let CallableCandidate::Jvm { facts, .. } = candidate else {
        return None;
    };
    (facts.return_type.name == "Object"
        && facts.return_type.qualification.as_deref() == Some("java.lang"))
    .then(|| Type::class("Object", Some("java.lang".into())))
}

/// Preserve the source-JVM lowering behavior; derive admitted Java results through the current
/// scoped type resolver. Missing/ambiguous java.lang.String remains Unknown, never guessed Object.
pub fn projected_return_type_in_context(
    index: &dyn InferenceIndex,
    receiver: &Type,
    candidate: CallableCandidate<'_>,
    ctx: &FileCtx,
) -> Option<Type> {
    if let CallableCandidate::Jvm {
        owner: JvmOwner::Source(source),
        facts,
    } = candidate
    {
        if let Some(entry) = source_property_entry(index, source, facts) {
            let result =
                infer::declared_member_type(index, receiver, entry, &facts.return_type, ctx);
            return (!matches!(result, Type::Unknown)).then_some(result);
        }
    }
    if let CallableCandidate::Jvm {
        owner: JvmOwner::Compiled { .. },
        facts,
    } = candidate
    {
        let component = facts.return_type.name.trim_end_matches("[]");
        if facts.return_type.qualification.is_none()
            && component != facts.return_type.name
            && infer::primitive_type_name(component) != component
        {
            // Exact Java primitive descriptors cannot bind a Kotlin class with the same spelling.
            let rank = (facts.return_type.name.len() - component.len()) / 2;
            let mut result = Type::class(component, None);
            for _ in 0..rank {
                result = Type::array(result, false);
            }
            return Some(result);
        }
        let result = infer::declared_type(index, &facts.return_type, ctx);
        return (!matches!(result, Type::Unknown)).then_some(result);
    }
    projected_return_type(candidate)
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
            source_property: None,
        });
    }
    for symbol in symbols
        .iter()
        .filter(|symbol| symbol.kind == SymbolKind::Property)
    {
        let Some(mut node) = tree
            .root_node()
            .descendant_for_byte_range(symbol.start_byte, symbol.end_byte)
        else {
            continue;
        };
        while !matches!(node.kind(), "class_parameter" | "property_declaration")
            && node.parent().is_some()
        {
            node = node.parent().unwrap();
        }
        if !matches!(node.kind(), "class_parameter" | "property_declaration")
            || node.kind() == "class_parameter" && node.has_error()
            || symbol.source_member.visibility() != Some(crate::extensions::Visibility::Public)
            || symbol.source_member.dispatch() != Some(crate::extensions::Dispatch::Instance)
            || symbol.source_member.jvm_restricted()
            || symbol.ext_receiver.is_some()
            || symbol.extension_receiver.is_some()
        {
            continue;
        }
        let modifiers = child_of_kind(node, "modifiers");
        if modifiers.is_some_and(|m| {
            m.named_children(&mut m.walk())
                .any(|n| n.kind() == "annotation")
                || node_text(m, text)
                    .split_whitespace()
                    .any(|w| !matches!(w, "public" | "final" | "open" | "override" | "abstract"))
        }) {
            continue;
        }
        // An accessor annotation/unknown modifier can change the emitted method independently
        // of the property's spelling. Getter bodies do not change an explicit property type.
        if child_of_kind(node, "getter").is_some_and(|getter| {
            child_of_kind(getter, "modifiers").is_some_and(|modifiers| {
                modifiers
                    .named_children(&mut modifiers.walk())
                    .any(|n| n.kind() == "annotation")
                    || node_text(modifiers, text)
                        .split_whitespace()
                        .any(|word| word != "public")
            })
        }) {
            continue;
        }
        let class = if node.kind() == "class_parameter" {
            node.parent()
                .and_then(|n| n.parent())
                .and_then(|n| n.parent())
        } else {
            node.parent()
                .filter(|parent| parent.kind() == "class_body")
                .and_then(|body| body.parent())
        };
        let Some(class) = class else { continue };
        if class.kind() != "class_declaration"
            || class
                .parent()
                .is_none_or(|parent| parent.kind() != "source_file")
            || child_of_kind(class, "type_constraints").is_some()
        {
            continue;
        }
        let Some(owner) = symbols.iter().find(|owner| {
            owner.kind == SymbolKind::Class
                && owner.container.is_none()
                && Some(owner.name.as_str()) == symbol.container.as_deref()
        }) else {
            continue;
        };
        let class_modifiers = child_of_kind(class, "modifiers");
        let mut requires_binary_witness = false;
        if let Some(modifiers) = class_modifiers {
            let mut supported = true;
            for modifier in modifiers.named_children(&mut modifiers.walk()) {
                if modifier.kind() == "annotation" {
                    requires_binary_witness = true;
                } else if node_text(modifier, text)
                    .split_whitespace()
                    .any(|word| !matches!(word, "public" | "final" | "open" | "data" | "abstract"))
                {
                    supported = false;
                }
            }
            if !supported {
                continue;
            }
        }
        // Bounds and value-class mangling need their own lowering proof.
        if child_of_kind(class, "type_parameters").is_some_and(|params| {
            params
                .named_children(&mut params.walk())
                .any(|parameter| node_text(parameter, text).contains([':', '@']))
        }) {
            continue;
        }
        let Some(return_type) = symbol.value_type.clone() else {
            continue;
        };
        let Some(name) = property_getter_name(&symbol.name) else {
            continue;
        };
        let getter = CallableFacts {
            name,
            params: Vec::new(),
            return_type,
            type_params: Vec::new(),
            definition: Some(Def {
                file: file.into(),
                start_byte: symbol.start_byte,
                end_byte: symbol.end_byte,
            }),
            fixed_name: None,
            jvm_name_annotation: None,
            is_static: false,
            source_property: Some(SourceProperty {
                owner: owner.name.clone(),
                name: symbol.name.clone(),
                type_params: owner.type_params.clone(),
                requires_binary_witness,
            }),
        };
        let mutable = node
            .children(&mut node.walk())
            .any(|child| child.kind() == "var");
        let public_setter = child_of_kind(node, "setter").is_none_or(|setter| {
            !setter
                .children(&mut setter.walk())
                .filter(|child| child.kind() != "function_body")
                .any(|child| child.has_error() || child.is_missing())
                && child_of_kind(setter, "modifiers").is_none_or(|modifiers| {
                    !modifiers
                        .named_children(&mut modifiers.walk())
                        .any(|child| child.kind() == "annotation")
                        && node_text(modifiers, text)
                            .split_whitespace()
                            .all(|word| word == "public")
                })
        });
        if mutable && public_setter {
            let mut setter = getter.clone();
            setter.name = property_setter_name(&symbol.name).unwrap();
            setter.params.push(setter.return_type.clone());
            setter.return_type = scope.type_ref("kotlin.Unit", false, Vec::new());
            callables.push(setter);
        }
        callables.push(getter);
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

/// The supported Kotlin property spelling, shared by source facts and Java candidate lookup.
pub(crate) fn property_getter_name(name: &str) -> Option<String> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    if name.starts_with("is") && name.chars().nth(2).is_some_and(|c| !c.is_ascii_lowercase()) {
        Some(name.into())
    } else {
        let mut chars = name.chars();
        Some(format!(
            "get{}{}",
            chars.next()?.to_ascii_uppercase(),
            chars.as_str()
        ))
    }
}

pub(crate) fn property_setter_name(name: &str) -> Option<String> {
    let getter = property_getter_name(name)?;
    Some(format!(
        "set{}",
        if getter == name {
            &getter[2..]
        } else {
            &getter[3..]
        }
    ))
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
    fn constructor_property_getters_share_candidates_and_source_definitions() {
        let file = "/getter/Boxes.kt";
        let text = "package getter\nclass Box<T>(val value: T)\nclass Plain(val count: Int)\n";
        let mut parser = crate::parser::KotlinParser::new();
        let tree = parser.parse(text);
        let symbols = indexer::extract_symbols(&tree, text, "getter");
        let facts = source_facts(file, &tree, text, &symbols).expect("constructor JVM getters");
        let mut index = crate::index::Index::new();
        index.replace_file(file, symbols, Tier::Volatile);
        index.replace_jvm_source(file, Some(facts), Tier::Volatile);
        let receiver = Type::class("Box", Some("getter".into()));
        let candidates = member_candidates(&index, &receiver, "getValue");
        let [candidate] = candidates.as_slice() else {
            panic!(
                "one getter for the selected class, got {}",
                candidates.len()
            );
        };
        assert_eq!(candidate.params().len(), 0);
        let definition = candidate.definition().unwrap();
        assert_eq!(&text[definition.start_byte..definition.end_byte], "value");
        let identity = candidate.identity(&index).unwrap();
        assert_eq!(identity.class.internal_name, "getter/Box");
        assert_eq!(identity.descriptor, "()Ljava/lang/Object;");
        assert!(member_candidates(&index, &receiver, "getCount").is_empty());
        let snapshot = index.inference_snapshot();
        index.replace_jvm_source(file, None, Tier::Volatile);
        assert!(member_candidates(&index, &receiver, "getValue").is_empty());
        assert_eq!(member_candidates(&snapshot, &receiver, "getValue").len(), 1);
    }

    #[test]
    fn inherited_constructor_getters_respect_nearer_unavailable_properties() {
        let file = "/getter/Inherited.kt";
        let text =
            "package getter\nopen class Base<T>(val value: T)\nclass Child : Base<String>()\n";
        let tree = crate::parser::KotlinParser::new().parse(text);
        let symbols = indexer::extract_symbols(&tree, text, "getter");
        let facts = source_facts(file, &tree, text, &symbols).unwrap();
        let mut index = crate::index::Index::new();
        index.replace_file(file, symbols.clone(), Tier::Volatile);
        index.replace_jvm_source(file, Some(facts), Tier::Volatile);
        let receiver = Type::class("Child", Some("getter".into()));
        assert_eq!(member_candidates(&index, &receiver, "getValue").len(), 1);
        let mut unavailable = symbols;
        unavailable
            .iter_mut()
            .find(|symbol| symbol.name == "Child")
            .unwrap()
            .unavailable_members =
            Some(Box::new(vec![("value".into(), SymbolKind::Property, None)]));
        index.replace_file(file, unavailable, Tier::Volatile);
        assert!(
            member_candidates(&index, &receiver, "getValue").is_empty(),
            "an unknown nearer property must block the ancestor getter"
        );
    }

    #[test]
    fn constructor_property_getter_names_follow_kotlin_is_prefix_rules() {
        let text = "package names\nclass Names(val is_foo: String, val is0: String, val isfoo: String, val isFoo: String)\n";
        let mut parser = crate::parser::KotlinParser::new();
        let tree = parser.parse(text);
        let symbols = indexer::extract_symbols(&tree, text, "names");
        let facts = source_facts("/names/Names.kt", &tree, text, &symbols).unwrap();
        assert_eq!(
            facts
                .callables
                .iter()
                .map(|facts| facts.name.as_str())
                .collect::<Vec<_>>(),
            vec!["is_foo", "is0", "getIsfoo", "isFoo"]
        );
    }

    #[test]
    fn constructor_property_getters_require_exact_annotated_owner_inventory() {
        use crate::index::{Index, LookupEntries};
        use std::sync::Arc;
        struct Inventory {
            index: Index,
            class: Option<Arc<crate::classfile::ClassFileFacts>>,
        }
        impl InferenceIndex for Inventory {
            fn lookup_by_name(&self, name: &str) -> LookupEntries<'_> {
                LookupEntries::All(self.index.lookup_by_name(name))
            }
            fn members_of(&self, name: &str) -> LookupEntries<'_> {
                LookupEntries::All(self.index.members_of(name))
            }
            fn extensions_for(&self, name: &str) -> LookupEntries<'_> {
                LookupEntries::All(self.index.extensions_for(name))
            }
            fn jvm_sources(&self, owner: &str) -> Vec<&SourceFile> {
                self.index.jvm_sources(owner)
            }
            fn java_class_inventory(
                &self,
                _: &Entry,
            ) -> Option<Arc<crate::classfile::ClassFileFacts>> {
                self.class.clone()
            }
        }
        let file = "/stdlib/Tuples.kt";
        let text = include_str!("../tests/fixtures/classfile/kotlin240/Tuples.kt");
        let mut parser = crate::parser::KotlinParser::new();
        let tree = parser.parse(text);
        let symbols = indexer::extract_symbols(&tree, text, "kotlin");
        let facts = source_facts(file, &tree, text, &symbols).unwrap();
        let mut index = Index::new();
        index.replace_file(file, symbols, Tier::Durable);
        index.replace_jvm_source(file, Some(facts), Tier::Durable);
        let pair = Type::Class {
            name: "Pair".into(),
            package: Some("kotlin".into()),
            container: None,
            args: vec![
                Type::class("String", Some("kotlin".into())),
                Type::class("Int", Some("kotlin".into())),
            ],
            nullable: false,
            owner: None,
        };
        let actual = crate::classfile::decode(
            "maven:org.jetbrains.kotlin:kotlin-stdlib:2.4.0",
            include_bytes!("../tests/fixtures/classfile/kotlin240/Pair.class"),
        )
        .unwrap();
        let mut inventory = Inventory {
            index,
            class: Some(Arc::new(actual.clone())),
        };
        let candidates = member_candidates(&inventory, &pair, "getFirst");
        assert_eq!(candidates.len(), 1, "actual Pair.class getter");
        let candidate = candidates[0];
        assert_eq!(
            candidate.identity(&inventory).unwrap().class.internal_name,
            "kotlin/Pair"
        );
        assert_eq!(
            &text[candidate.definition().unwrap().start_byte
                ..candidate.definition().unwrap().end_byte],
            "first"
        );
        assert_eq!(
            projected_return_type_in_context(
                &inventory,
                &pair,
                candidate,
                &FileCtx::new("kotlin".into(), Vec::new())
            )
            .unwrap()
            .name(),
            Some("String")
        );
        for change in 0..7 {
            let mut changed = actual.clone();
            let method = changed
                .methods
                .iter_mut()
                .find(|m| m.name.as_ref() == "getFirst")
                .unwrap();
            match change {
                0 => method.access_flags |= 0x0008,
                1 => method.access_flags |= 0x1000,
                2 => method.access_flags = 0x0002,
                3 => method.signature = Some("()TB;".into()),
                4 => changed.signature = Some("<A:Ljava/lang/String;B:Ljava/lang/Object;>Ljava/lang/Object;Ljava/io/Serializable;".into()),
                5 => changed.identity.internal_name = "kotlin/Other".into(),
                _ => method.name = "renamed".into(),
            }
            inventory.class = Some(Arc::new(changed));
            assert!(
                member_candidates(&inventory, &pair, "getFirst").is_empty(),
                "invalid witness {change}"
            );
        }
        inventory.class = None;
        assert!(
            member_candidates(&inventory, &pair, "getFirst").is_empty(),
            "unknown class annotation needs a witness"
        );

        let text = include_str!("../tests/fixtures/classfile/kotlin240/VarProperties.kt");
        let file = "/setter/Properties.kt";
        let tree = parser.parse(text);
        let symbols = indexer::extract_symbols(&tree, text, "settercontrol");
        let facts = source_facts(file, &tree, text, &symbols).unwrap();
        inventory.index = Index::new();
        inventory.index.replace_file(file, symbols, Tier::Durable);
        inventory
            .index
            .replace_jvm_source(file, Some(facts), Tier::Durable);
        let actual = crate::classfile::decode(
            "fixture:kotlin-2.4.0",
            include_bytes!("../tests/fixtures/classfile/kotlin240/VarBox.class"),
        )
        .unwrap();
        inventory.class = Some(Arc::new(actual.clone()));
        let receiver = Type::class("Box", Some("settercontrol".into()));
        let candidates = member_candidates(&inventory, &receiver, "setValue");
        let [setter] = candidates.as_slice() else {
            panic!("actual generic Box setter witness")
        };
        let identity = setter.identity(&inventory).unwrap();
        assert_eq!(identity.class.internal_name, "settercontrol/Box");
        assert_eq!(identity.descriptor, "(Ljava/lang/Object;)V");
        for change in 0..4 {
            let mut changed = actual.clone();
            let setter = changed
                .methods
                .iter_mut()
                .find(|method| method.name.as_ref() == "setValue")
                .unwrap();
            match change {
                0 => setter.access_flags |= 0x0008,
                1 => setter.access_flags = 0x0002,
                2 => setter.signature = Some("(Ljava/lang/String;)V".into()),
                _ => setter.descriptor = "(Ljava/lang/Object;)Ljava/lang/Object;".into(),
            }
            inventory.class = Some(Arc::new(changed));
            assert!(
                member_candidates(&inventory, &receiver, "setValue").is_empty(),
                "invalid setter witness {change}"
            );
        }
    }

    #[test]
    fn constructor_property_getters_do_not_guess_annotation_or_visibility_lowering() {
        for declaration in [
            "class Box(@JvmField val value: Int)",
            "class Box(@get:JvmName(\"other\") val value: Int)",
            "class Box(private val value: Int)",
            "class Box(internal val value: Int)",
            "@Unknown class Box(val value: Int)",
            "value class Box(val value: Int)",
            "class Box<T: Number>(val value: T)",
            "@JvmInline value class Token(val raw: Int)\nclass Box(val value: Token)",
            "class Box { @JvmField val value: Int = 1 }",
            "class Box { @get:JvmName(\"other\") val value: Int = 1 }",
            "class Box { private val value: Int = 1 }",
            "class Box { internal val value: Int = 1 }",
            "@Unknown class Box { val value: Int = 1 }",
            "class Box { val String.value: Int get() = 1 }",
            "class Box { val value = 1 }",
            "@JvmInline value class Token(val raw: Int)\nclass Box { val value: Token = Token(1) }",
            "@JvmInline value class String(val raw: Int)\nclass Box(var value: String)",
        ] {
            let text = format!("package getter\n{declaration}\n");
            let mut parser = crate::parser::KotlinParser::new();
            let tree = parser.parse(&text);
            let symbols = indexer::extract_symbols(&tree, &text, "getter");
            let facts = source_facts("/getter/Box.kt", &tree, &text, &symbols);
            let mut index = crate::index::Index::new();
            index.replace_file("/getter/Box.kt", symbols, Tier::Volatile);
            index.replace_jvm_source("/getter/Box.kt", facts, Tier::Volatile);
            assert!(
                member_candidates(
                    &index,
                    &Type::class("Box", Some("getter".into())),
                    "getValue"
                )
                .is_empty(),
                "{declaration}"
            );
        }
    }

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
                projection: Default::default(),
                qualification: Some("java.lang".into()),
                ..TypeRef::default()
            },
            type_params: Vec::new(),
            definition: None,
            fixed_name: None,
            jvm_name_annotation: None,
            is_static: false,
            source_property: None,
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
                    names: None,
                    has_spread: false,
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

    #[test]
    fn compiled_same_identity_with_different_facts_is_unknown() {
        let index = crate::index::Index::new();
        let class = ClassIdentity {
            origin_id: "selected-project".into(),
            internal_name: "oracle/Person".into(),
            input: InputIdentity::Binary {
                content_sha256: [7; 32],
            },
        };
        let facts = CallableFacts {
            name: "getName".into(),
            params: Vec::new(),
            return_type: TypeRef {
                name: "String".into(),
                projection: Default::default(),
                qualification: Some("java.lang".into()),
                ..TypeRef::default()
            },
            type_params: Vec::new(),
            definition: None,
            fixed_name: None,
            jvm_name_annotation: None,
            is_static: false,
            source_property: None,
        };
        let mut conflicting = facts.clone();
        conflicting.is_static = true;
        let candidate = CallableCandidate::Jvm {
            owner: JvmOwner::Compiled {
                class: &class,
                descriptor: "()Ljava/lang/String;",
                tier: Tier::Volatile,
            },
            facts: &facts,
        };
        let conflicting_candidate = CallableCandidate::Jvm {
            owner: JvmOwner::Compiled {
                class: &class,
                descriptor: "()Ljava/lang/String;",
                tier: Tier::Volatile,
            },
            facts: &conflicting,
        };
        let selection = select_call(
            &index,
            &Type::class("Person", Some("oracle".into())),
            vec![candidate, conflicting_candidate],
            "getName",
            &crate::members::CallArguments {
                count: Some(0),
                types: Vec::new(),
                names: None,
                has_spread: false,
                trailing_lambda: false,
            },
            &FileCtx::new("oracle".into(), Vec::new()),
        );
        assert_eq!(
            selection.applicability,
            crate::members::Applicability::Unknown
        );
        assert_eq!(selection.candidates.len(), 2);
        assert!(selection.navigation_candidates().is_empty());
        let mut rejected_shape = facts.clone();
        rejected_shape.params.push(TypeRef::simple("Int"));
        let conflicting_candidate = CallableCandidate::Jvm {
            owner: JvmOwner::Compiled {
                class: &class,
                descriptor: "()Ljava/lang/String;",
                tier: Tier::Volatile,
            },
            facts: &rejected_shape,
        };
        let selection = select_call(
            &index,
            &Type::class("Person", Some("oracle".into())),
            vec![candidate, conflicting_candidate],
            "getName",
            &crate::members::CallArguments {
                count: Some(0),
                types: Vec::new(),
                names: None,
                has_spread: false,
                trailing_lambda: false,
            },
            &FileCtx::new("oracle".into(), Vec::new()),
        );
        assert_eq!(
            selection.applicability,
            crate::members::Applicability::Unknown
        );
        assert!(selection.navigation_candidates().is_empty());
    }

    #[test]
    fn compiled_instance_dispatch_requires_a_proven_value_without_changing_authored_static_calls() {
        use crate::extensions::{Dispatch, Visibility};
        use crate::members::{Applicability, CallArguments};
        use crate::symbol::{MemberImplementation, SourceMemberFlags};
        use std::sync::Arc;

        let index = crate::index::Index::new();
        let class = ClassIdentity {
            origin_id: "selected-project".into(),
            internal_name: "oracle/Person".into(),
            input: InputIdentity::Binary {
                content_sha256: [7; 32],
            },
        };
        let facts = CallableFacts {
            name: "getName".into(),
            params: Vec::new(),
            return_type: TypeRef {
                name: "String".into(),
                projection: Default::default(),
                qualification: Some("java.lang".into()),
                ..TypeRef::default()
            },
            type_params: Vec::new(),
            definition: None,
            fixed_name: None,
            jvm_name_annotation: None,
            is_static: false,
            source_property: None,
        };
        let candidate = CallableCandidate::Jvm {
            owner: JvmOwner::Compiled {
                class: &class,
                descriptor: "()Ljava/lang/String;",
                tier: Tier::Volatile,
            },
            facts: &facts,
        };
        let receiver = Type::class("Person", Some("oracle".into()));
        let args = CallArguments {
            count: Some(0),
            types: Vec::new(),
            names: None,
            has_spread: false,
            trailing_lambda: false,
        };
        let ctx = FileCtx::new("oracle".into(), Vec::new());
        for (kind, expected, usable) in [
            (ReceiverKind::Value, Applicability::Applicable, 1),
            (ReceiverKind::Type, Applicability::Rejected, 0),
            (ReceiverKind::Unknown, Applicability::Unknown, 0),
        ] {
            let selected = select_call_for_receiver_kind(
                &index,
                &receiver,
                vec![candidate],
                "getName",
                &args,
                &ctx,
                kind,
            );
            assert_eq!(selected.applicability, expected, "{kind:?}");
            assert_eq!(selected.candidates.len(), 1, "retain provenance");
            assert_eq!(selected.consumable_candidates().len(), usable, "{kind:?}");
            assert_eq!(selected.navigation_candidates().len(), usable, "{kind:?}");
            assert!(selected.candidates[0].definition().is_none());
        }
        let mut conflicting = facts.clone();
        conflicting.is_static = true;
        let conflict = CallableCandidate::Jvm {
            owner: JvmOwner::Compiled {
                class: &class,
                descriptor: "()Ljava/lang/String;",
                tier: Tier::Volatile,
            },
            facts: &conflicting,
        };
        let selected = select_call_for_receiver_kind(
            &index,
            &receiver,
            vec![candidate, conflict],
            "getName",
            &args,
            &ctx,
            ReceiverKind::Type,
        );
        assert_eq!(selected.applicability, Applicability::Unknown);
        assert_eq!(selected.candidates.len(), 2);
        assert!(selected.consumable_candidates().is_empty());

        let mut symbol = IndexedSymbol::new(
            "getName",
            SymbolKind::Function,
            "oracle",
            Some("Person".into()),
            10,
            17,
        );
        symbol.arity = Some(0);
        symbol.min_arity = Some(0);
        symbol.source_member = SourceMemberFlags::new(
            MemberImplementation::Concrete,
            Dispatch::Static,
            Visibility::Public,
            false,
            false,
        );
        let entry = Entry {
            path: Arc::from("/authored/Person.java"),
            tier: Tier::Volatile,
            sym: Arc::new(symbol),
        };
        for kind in [ReceiverKind::Type, ReceiverKind::Unknown] {
            let authored = CallableCandidate::Authored(&entry);
            let old = select_call(&index, &receiver, vec![authored], "getName", &args, &ctx);
            let new = select_call_for_receiver_kind(
                &index,
                &receiver,
                vec![authored],
                "getName",
                &args,
                &ctx,
                kind,
            );
            assert_eq!(new.applicability, old.applicability);
            assert_eq!(
                new.navigation_candidates()[0].definition(),
                old.navigation_candidates()[0].definition()
            );
            assert_eq!(new.consumable_candidates().len(), 1);
        }
        let mut static_facts = facts.clone();
        static_facts.is_static = true;
        let source = SourceFile {
            file: "/source/Api.kt".into(),
            revision: 1,
            tier: Tier::Volatile,
            facts: SourceFileFacts {
                input_hash: 1,
                package: "oracle".into(),
                owner: "ApiKt".into(),
                owner_annotation: None,
                callables: vec![static_facts],
            },
        };
        let projected = CallableCandidate::Jvm {
            owner: JvmOwner::Source(&source),
            facts: &source.facts.callables[0],
        };
        let selected = select_call_for_receiver_kind(
            &index,
            &receiver,
            vec![projected],
            "getName",
            &args,
            &ctx,
            ReceiverKind::Unknown,
        );
        assert_eq!(selected.applicability, Applicability::Applicable);
        assert_eq!(selected.consumable_candidates().len(), 1);
    }

    fn suspend_source_fixture(text: &str) -> (crate::index::Index, SourceFile) {
        let file = "/scalar/Api.kt";
        let mut parser = crate::parser::KotlinParser::new();
        let tree = parser.parse(text);
        let symbols = crate::indexer::extract_symbols(&tree, text, "scalar");
        let facts = source_facts(file, &tree, text, &symbols).expect("real suspend source facts");
        let mut index = crate::index::Index::new();
        index.replace_file(file, symbols, Tier::Volatile);
        for (name, wrapper) in [
            ("Boolean", "Boolean"),
            ("Byte", "Byte"),
            ("Char", "Character"),
            ("Short", "Short"),
            ("Int", "Integer"),
            ("Long", "Long"),
            ("Float", "Float"),
            ("Double", "Double"),
        ] {
            for source_set in ["common", "jvm"] {
                index.replace_file(
                    &format!("/stdlib/{source_set}/{name}.kt"),
                    vec![IndexedSymbol::new(
                        name,
                        SymbolKind::Class,
                        "kotlin",
                        None,
                        0,
                        name.len(),
                    )],
                    Tier::Durable,
                );
            }
            index.replace_file(
                &format!("/sdk/{wrapper}.java"),
                vec![IndexedSymbol::new(
                    wrapper,
                    SymbolKind::Class,
                    "java.lang",
                    None,
                    0,
                    wrapper.len(),
                )],
                Tier::Durable,
            );
        }
        for name in ["String", "Unit"] {
            index.replace_file(
                &format!("/stdlib/{name}.kt"),
                vec![IndexedSymbol::new(
                    name,
                    SymbolKind::Class,
                    "kotlin",
                    None,
                    0,
                    name.len(),
                )],
                Tier::Durable,
            );
        }
        (
            index,
            SourceFile {
                file: file.into(),
                revision: 1,
                tier: Tier::Volatile,
                facts,
            },
        )
    }

    #[test]
    fn suspend_source_scalar_identities_use_actual_primitive_and_wrapper_descriptors() {
        let mut text = "package scalar\nimport java.lang.Long as JLong\n".to_string();
        let mut expected = Vec::new();
        for (name, primitive, wrapper) in [
            ("Long", "J", "Long"),
            ("Short", "S", "Short"),
            ("Byte", "B", "Byte"),
            ("Float", "F", "Float"),
            ("Double", "D", "Double"),
            ("Boolean", "Z", "Boolean"),
            ("Char", "C", "Character"),
            ("Int", "I", "Integer"),
        ] {
            text.push_str(&format!(
                "suspend fun p{name}(value: {name}): {name} = value\n"
            ));
            text.push_str(&format!(
                "suspend fun n{name}(value: {name}?): {name}? = value\n"
            ));
            expected.push((format!("p{name}"), primitive.to_string()));
            expected.push((format!("n{name}"), format!("Ljava/lang/{wrapper};")));
        }
        for (name, written, descriptor) in [
            ("boxed", "java.lang.Long", "Ljava/lang/Long;"),
            ("alias", "JLong", "Ljava/lang/Long;"),
            ("string", "String", "Ljava/lang/String;"),
            ("nullableString", "String?", "Ljava/lang/String;"),
            ("unit", "Unit", "Lkotlin/Unit;"),
            ("nullableUnit", "Unit?", "Lkotlin/Unit;"),
        ] {
            text.push_str(&format!(
                "suspend fun {name}(value: {written}): {written} = value\n"
            ));
            expected.push((name.to_string(), descriptor.to_string()));
        }
        let (index, source) = suspend_source_fixture(&text);
        assert_eq!(source.facts.callables.len(), expected.len());
        for (name, parameter) in expected {
            let facts = source
                .facts
                .callables
                .iter()
                .find(|facts| facts.name == name)
                .unwrap();
            let identity = CallableCandidate::Jvm {
                owner: JvmOwner::Source(&source),
                facts,
            }
            .identity(&index)
            .unwrap();
            assert_eq!(identity.class.internal_name, "scalar/ApiKt");
            assert_eq!(
                identity.descriptor,
                format!("({parameter}Lkotlin/coroutines/Continuation;)Ljava/lang/Object;"),
                "{name}"
            );
            assert!(
                !identity.descriptor.contains("Lkotlin/Long;"),
                "{name}: {identity:?}"
            );
        }
    }

    #[test]
    fn suspend_source_unknown_generic_and_array_erasure_never_fabricates_identity() {
        let text = "package scalar\nsuspend fun <Long> generic(value: Long): Long = value\nsuspend fun missing(value: Missing): Missing = value\nsuspend fun <T> array(value: Array<T>): Array<T> = value\nsuspend fun concreteArray(value: Array<Long>): Array<Long> = value\nsuspend fun invalidScalar(value: Long<String>): Long<String> = value\n";
        let (mut index, source) = suspend_source_fixture(text);
        index.replace_file(
            "/stdlib/Array.kt",
            vec![IndexedSymbol::new(
                "Array",
                SymbolKind::Class,
                "kotlin",
                None,
                0,
                5,
            )],
            Tier::Durable,
        );
        assert_eq!(source.facts.callables.len(), 5);
        let generic = source
            .facts
            .callables
            .iter()
            .find(|facts| facts.name == "generic")
            .unwrap();
        assert_eq!(generic.type_params, ["Long"]);
        assert!(generic.params[0].qualification.is_none());
        for facts in &source.facts.callables {
            let identity = CallableCandidate::Jvm {
                owner: JvmOwner::Source(&source),
                facts,
            }
            .identity(&index);
            assert!(
                identity.is_none(),
                "{} must remain unsupported: {identity:?}",
                facts.name
            );
        }
    }

    #[test]
    fn suspend_source_nominal_nested_and_generic_owner_erasure_is_preserved() {
        let text = "package scalar\nclass Long\nclass Outer { class Inner }\nclass Box<T>\nsuspend fun user(value: Long): Long = value\nsuspend fun nested(value: Outer.Inner): Outer.Inner = value\nsuspend fun <T> boxedGeneric(value: Box<T>): Box<T> = value\n";
        let (index, source) = suspend_source_fixture(text);
        assert_eq!(source.facts.callables.len(), 3);
        for (name, parameter) in [
            ("user", "Lscalar/Long;"),
            ("nested", "Lscalar/Outer$Inner;"),
            ("boxedGeneric", "Lscalar/Box;"),
        ] {
            let facts = source
                .facts
                .callables
                .iter()
                .find(|facts| facts.name == name)
                .unwrap();
            let identity = CallableCandidate::Jvm {
                owner: JvmOwner::Source(&source),
                facts,
            }
            .identity(&index)
            .unwrap();
            assert_eq!(
                identity.descriptor,
                format!("({parameter}Lkotlin/coroutines/Continuation;)Ljava/lang/Object;"),
                "{name}"
            );
        }
    }
}
