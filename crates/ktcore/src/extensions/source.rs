//! Authored declaration facts from the accepted source tree and shared index. These facts are a
//! sidecar: they do not change cached symbols or pretend source properties are generated methods.

use std::ops::Range;

use tree_sitter::{Node, Tree};

use super::{
    DeclaredMemberFacts, Dispatch, Fact, IncompleteReason, JvmType, MemberSignature, Visibility,
};
use crate::index::{InferenceIndex, ScopedIndex};
use crate::indexer::TypeScope;
use crate::infer::{self, FileCtx};
use crate::knowledge::Knowledge;
use crate::parser::{child_of_kind, node_text};
use crate::symbol::{Def, IndexedSymbol, SymbolKind};
use crate::types::{Type, TypeRef};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceDeclaration {
    pub definition: Def,
    pub kind: SymbolKind,
    pub range: Range<usize>,
    pub header_range: Range<usize>,
    pub name_range: Range<usize>,
    pub body_range: Option<Range<usize>>,
    pub modifiers: SourceModifiers,
    pub owner: Option<Def>,
    pub annotations: Vec<SourceAnnotation>,
    pub member: Option<DeclaredMemberFacts>,
    /// Current authored property lowering; not a complete method inventory.
    pub jvm_property: Option<Fact<JvmPropertyAccess>>,
}

/// JVM value namespace facts are distinct from generated accessor/callable inventory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JvmPropertyAccess {
    /// No Java-exposed/inherited field; a private JVM backing field may still exist.
    GetterOnly,
    Field,
}

/// Derived from one accepted tree, retained with the host's existing per-file metadata.
/// It is never serialized or admitted as compiler output.
#[derive(Clone, Debug)]
pub struct KotlinValueNamespaceFacts {
    pub input_sha256: [u8; 32],
    pub qualifier_packages: std::sync::Arc<Vec<String>>,
    pub owners: std::collections::BTreeSet<Def>,
    pub properties: std::collections::BTreeMap<Def, Fact<JvmPropertyAccess>>,
}

impl KotlinValueNamespaceFacts {
    pub fn matches_source(&self, text: &str) -> bool {
        use sha2::Digest;
        self.input_sha256 == <[u8; 32]>::from(sha2::Sha256::digest(text.as_bytes()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceAnnotation {
    pub ty: TypeRef,
    pub use_site_target: Option<String>,
    pub arguments: Vec<AnnotationArgument>,
    pub range: Range<usize>,
    pub name_range: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnnotationArgument {
    pub name: Option<String>,
    pub range: Range<usize>,
    pub value: Fact<AnnotationValue>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnnotationValue {
    Class(TypeRef),
    String {
        value: String,
        content_range: Range<usize>,
    },
    Boolean(bool),
    Integer { value: i64, ty: AnnotationIntegerType },
    Enum { ty: TypeRef, name: String },
    Annotation(Box<SourceAnnotation>),
    MappedString { value: String, segments: Vec<StringSourceSegment> },
    Array(Vec<AnnotationArgument>),
}

/// The grammar is identified by its root kind; the host supplies the same declaration visibility
/// context used by its language indexer. Unknown expressions/types retain their source ranges.
pub fn extract(
    index: &dyn InferenceIndex,
    file: &str,
    tree: &Tree,
    text: &str,
    scope: &TypeScope,
) -> Vec<SourceDeclaration> {
    let index = ScopedIndex::new(index, file);
    extract_declarations(&index, file, tree, text, scope, true)
}

/// Declaration annotations may introduce members even with no framework provider registered.
/// This uses the same accepted identities and annotation parser without normalizing signatures.
pub fn annotated_type_owners(
    index: &dyn InferenceIndex,
    file: &str,
    tree: &Tree,
    text: &str,
    scope: &TypeScope,
) -> Vec<Def> {
    let entries = index.entries_for_file(file);
    let mut owners: Vec<_> = extract_declarations(index, file, tree, text, scope, false)
        .into_iter()
        .filter(|declaration| !declaration.annotations.is_empty())
        .filter_map(|declaration| {
            if entries.iter().any(|entry| {
                definition(file, &entry.sym) == declaration.definition
                    && entry.sym.kind.is_type_like()
            }) {
                Some(declaration.definition)
            } else {
                declaration.owner
            }
        })
        .collect();
    owners.sort();
    owners.dedup();
    owners
}

fn extract_declarations(
    index: &dyn InferenceIndex,
    file: &str,
    tree: &Tree,
    text: &str,
    scope: &TypeScope,
    include_members: bool,
) -> Vec<SourceDeclaration> {
    let java = match tree.root_node().kind() {
        "program" => true,
        "source_file" => false,
        _ => return Vec::new(),
    };
    let entries = index.entries_for_file(file);
    let mut result = Vec::new();
    for entry in &entries {
        let symbol = &entry.sym;
        let Some(node) = declaration_node(tree, symbol, java) else {
            continue;
        };
        let owner = entries
            .iter()
            .find(|candidate| {
                candidate.sym.kind.is_type_like()
                    && symbol.container.as_deref() == Some(container_name(&candidate.sym).as_str())
                    && candidate.sym.package == symbol.package
            })
            .map(|entry| definition(file, &entry.sym));
        let ctx = FileCtx::new(symbol.package.clone(), Vec::new()).with_file(file);
        let annotations = annotations(index, &ctx, node, text, scope, java);
        let jvm_property = (include_members && !java && symbol.kind == SymbolKind::Property)
            .then(|| kotlin_property_access(index, symbol, node, text, &annotations));
        let member = (include_members
            && symbol.container.is_some()
            && symbol.ext_receiver.is_none()
            && matches!(symbol.kind, SymbolKind::Function | SymbolKind::Property))
        .then(|| {
            member_facts(
                index,
                file,
                text,
                symbol,
                node,
                scope,
                java,
                &annotations,
                jvm_property.as_ref(),
            )
        });
        let (range, header_range, name_range, body_range) = declaration_ranges(node, symbol);
        let modifiers = applied_modifiers(node, text, symbol, java, member.as_ref());
        result.push(SourceDeclaration {
            definition: definition(file, symbol),
            kind: symbol.kind,
            range, header_range, name_range, body_range, modifiers,
            owner,
            annotations,
            member,
            jvm_property,
        });
    }
    result
}

fn definition(file: &str, symbol: &IndexedSymbol) -> Def {
    Def {
        file: file.into(),
        start_byte: symbol.start_byte,
        end_byte: symbol.end_byte,
    }
}

fn container_name(symbol: &IndexedSymbol) -> String {
    let name = symbol.name.trim_matches('`');
    symbol
        .container
        .as_ref()
        .map_or_else(|| name.into(), |parent| format!("{parent}.{name}"))
}

pub(super) fn declaration_node<'a>(tree: &'a Tree, symbol: &IndexedSymbol, java: bool) -> Option<Node<'a>> {
    let mut node = tree
        .root_node()
        .descendant_for_byte_range(symbol.start_byte, symbol.end_byte)?;
    loop {
        let is_declaration = if java {
            match symbol.kind {
                SymbolKind::Function => matches!(
                    node.kind(),
                    "method_declaration" | "constructor_declaration"
                ),
                SymbolKind::Property => node.kind() == "field_declaration",
                _ if symbol.kind.is_type_like() => matches!(
                    node.kind(),
                    "class_declaration"
                        | "interface_declaration"
                        | "enum_declaration"
                        | "record_declaration"
                        | "annotation_type_declaration"
                ),
                _ => return None,
            }
        } else {
            match symbol.kind {
                SymbolKind::Function => node.kind() == "function_declaration",
                SymbolKind::Property => node.kind() == "property_declaration",
                _ if symbol.kind.is_type_like() => matches!(
                    node.kind(),
                    "class_declaration" | "object_declaration" | "type_alias"
                ),
                _ => return None,
            }
        };
        if is_declaration {
            return Some(node);
        }
        node = node.parent()?;
    }
}

/// Certify only the small, compiler-controlled ordinary property slice. The host still
/// has to prove current source identity and absence of unknown compiler transformations.
fn kotlin_property_access(
    index: &dyn InferenceIndex,
    symbol: &IndexedSymbol,
    node: Node<'_>,
    text: &str,
    annotations: &[SourceAnnotation],
) -> Fact<JvmPropertyAccess> {
    if node.kind() != "property_declaration"
        || node.has_error()
        || symbol.container.is_none()
        || symbol.ext_receiver.is_some()
        || symbol.source_member.dispatch() != Some(Dispatch::Instance)
        || !matches!(
            symbol.source_member.visibility(),
            Some(Visibility::Public | Visibility::Protected | Visibility::Private)
        )
        || symbol.source_member.jvm_restricted()
        || !node
            .children(&mut node.walk())
            .any(|child| node_text(child, text) == "val")
        || ["getter", "setter", "property_delegate"]
            .iter()
            .any(|kind| child_of_kind(node, kind).is_some())
    {
        return unsupported();
    }
    if let Some(modifiers) = child_of_kind(node, "modifiers") {
        if modifiers.named_children(&mut modifiers.walk()).any(|word| {
            !matches!(word.kind(), "annotation" | "marker_annotation")
                && !matches!(
                    node_text(word, text),
                    "public" | "protected" | "private" | "final"
                )
        }) {
            return unsupported();
        }
        let count = modifiers
            .named_children(&mut modifiers.walk())
            .filter(|child| matches!(child.kind(), "annotation" | "marker_annotation"))
            .count();
        if count != annotations.len() {
            return unsupported();
        }
    }
    let mut ancestor = node.parent();
    let mut class = false;
    while let Some(parent) = ancestor {
        if parent.kind() == "class_declaration" {
            if class
                || child_of_kind(parent, "type_parameters").is_some()
                || crate::parser::class_kind(parent) != SymbolKind::Class
            {
                return unsupported();
            }
            class = true;
            if child_of_kind(parent, "modifiers").is_some_and(|mods| {
                mods.named_children(&mut mods.walk()).any(|modifier| {
                    matches!(modifier.kind(), "annotation" | "marker_annotation")
                        || !matches!(node_text(modifier, text), "public" | "open" | "final")
                })
            }) {
                return unsupported();
            }
        } else if parent.kind() == "source_file"
            && parent
                .named_children(&mut parent.walk())
                .any(|child| child.kind() == "file_annotation")
        {
            return unsupported();
        } else if matches!(
            parent.kind(),
            "object_declaration"
                | "companion_object"
                | "function_declaration"
                | "lambda_literal"
                | "block"
                | "anonymous_initializer"
                | "init_block"
        ) {
            return unsupported();
        }
        ancestor = parent.parent();
    }
    if !class {
        return unsupported();
    }
    match annotations {
        [] => Knowledge::Found(JvmPropertyAccess::GetterOnly),
        [annotation]
            if matches!(symbol.source_member.visibility(), Some(Visibility::Public | Visibility::Protected))
            && annotation.ty.name == "JvmField"
            && annotation.ty.qualification.as_deref() == Some("kotlin.jvm")
            && kotlin_jvm_field_qualifier_unshadowed(index, symbol, &annotation.ty.package_candidates)
            && annotation.ty.args.is_empty()
            && annotation.arguments.is_empty()
            && annotation.use_site_target.is_none()
            // The fully qualified compiler intrinsic cannot be replaced by an authored
            // declaration in the forbidden kotlin package under the supported plain profile.
            && !index.lookup_type("JvmField").iter().any(|entry| {
                entry.sym.package == "kotlin.jvm" && entry.tier == crate::index::Tier::Volatile
            }) =>
        {
            Knowledge::Found(JvmPropertyAccess::Field)
        }
        _ => unsupported(),
    }
}

/// A qualified type path may start with an in-scope type rather than the root
/// package. Recheck this namespace dependency against the current index as well
/// as at source normalization; a later source edit can introduce the qualifier.
pub fn kotlin_jvm_field_qualifier_unshadowed(
    index: &dyn InferenceIndex,
    property: &IndexedSymbol,
    packages: &[String],
) -> bool {
    use std::ops::ControlFlow;
    let qualifiers = index.lookup_type("kotlin");
    if qualifiers
        .iter()
        .any(|entry| entry.sym.container.is_none() && packages.contains(&entry.sym.package))
    {
        return false;
    }
    let Some(owner) = property.container.as_deref() else {
        return false;
    };
    let receiver = Type::class(owner, Some(property.package.clone()));
    let mut unshadowed = true;
    let _ = infer::walk_supertypes(index, &receiver, 64, |current| {
        let Some(root) = infer::receiver_root_key(index, current) else {
            unshadowed = false;
            return ControlFlow::Break(());
        };
        if qualifiers.iter().any(|entry| {
            Some(entry.sym.package.as_str()) == current.package()
                && entry.sym.container.as_deref() == Some(root.as_str())
        }) {
            unshadowed = false;
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    });
    unshadowed
}

/// Normalize once per accepted source publication, using the same SourceDeclaration facts
/// consumed by extensions. Only the value namespace is closed; accessors remain independent.
pub fn kotlin_value_namespace(
    index: &dyn InferenceIndex,
    file: &str,
    tree: &Tree,
    text: &str,
    scope: &TypeScope,
) -> Option<KotlinValueNamespaceFacts> {
    use sha2::Digest;
    if tree.root_node().kind() != "source_file" {
        return None;
    }
    let declarations = extract(index, file, tree, text, scope);
    let properties = declarations
        .iter()
        .filter_map(|decl| Some((decl.definition.clone(), decl.jvm_property.clone()?)))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut owners = std::collections::BTreeSet::new();
    for entry in index.entries_for_file(file) {
        if entry.sym.kind != SymbolKind::Class
            || entry.sym.container.is_some()
            || !entry.sym.type_params.is_empty()
        {
            continue;
        }
        let Some(node) = declaration_node(tree, &entry.sym, false) else {
            continue;
        };
        let mut pending = vec![node];
        let mut complete = !tree
            .root_node()
            .named_children(&mut tree.root_node().walk())
            .any(|n| n.kind() == "file_annotation");
        while let Some(child) = pending.pop() {
            // Executable bodies contain locals, not declarations in this owner's
            // JVM value namespace. Inspect member headers and transforms only.
            if matches!(
                child.kind(),
                "function_body"
                    | "anonymous_initializer"
                    | "init_block"
                    | "block"
                    | "lambda_literal"
            ) {
                continue;
            }
            if child.is_error() || child.is_missing() {
                complete = false;
                break;
            }
            if child.kind() == "property_declaration" {
                let Some(name) = crate::parser::name_field(child).or_else(|| {
                    child_of_kind(child, "variable_declaration").and_then(|var| {
                        crate::parser::name_field(var).or_else(|| var.named_child(0))
                    })
                }) else {
                    complete = false;
                    break;
                };
                if !matches!(
                    properties.get(&Def {
                        file: file.into(),
                        start_byte: name.start_byte(),
                        end_byte: name.end_byte()
                    }),
                    Some(Knowledge::Found(_))
                ) {
                    complete = false;
                    break;
                }
                continue;
            }
            if matches!(
                child.kind(),
                "annotation"
                    | "marker_annotation"
                    | "object_declaration"
                    | "companion_object"
                    | "class_parameter"
                    | "type_parameters"
            ) || child.kind() == "class_declaration" && child.id() != node.id()
                || matches!(
                    child.kind(),
                    "data"
                        | "enum"
                        | "annotation"
                        | "value"
                        | "expect"
                        | "external"
                        | "inline"
                        | "suspend"
                        | "by"
                        | "context"
                )
            {
                complete = false;
                break;
            }
            pending.extend(child.children(&mut child.walk()));
        }
        if complete {
            owners.insert(definition(file, &entry.sym));
        }
    }
    Some(KotlinValueNamespaceFacts {
        input_sha256: sha2::Sha256::digest(text.as_bytes()).into(),
        qualifier_packages: scope
            .type_ref("kotlin", false, Vec::new())
            .package_candidates,
        owners,
        properties,
    })
}

fn annotations(index: &dyn InferenceIndex, ctx: &FileCtx, node: Node<'_>, text: &str,
    scope: &TypeScope, java: bool) -> Vec<SourceAnnotation> {
    let Some(modifiers) = child_of_kind(node, "modifiers") else {return Vec::new();};
    modifiers.named_children(&mut modifiers.walk())
        .filter(|node|matches!(node.kind(),"annotation"|"marker_annotation"))
        .filter_map(|node| {let mut remaining=MAX_SOURCE_ANNOTATION_NODES;
            source_annotation(index,ctx,node,text,scope,java,false,0,&mut remaining)}).collect()
}

fn source_annotation(index:&dyn InferenceIndex,ctx:&FileCtx,node:Node<'_>,text:&str,
    scope:&TypeScope,java:bool,constructor:bool,depth:usize,remaining:&mut usize)->Option<SourceAnnotation> {
    if depth>=MAX_SOURCE_ANNOTATION_DEPTH || *remaining==0 || node.byte_range().len()>MAX_SOURCE_ANNOTATION_BYTES
        || node.has_error() || node.is_missing() {return None;}
    *remaining-=1;
    let invocation=child_of_kind(node,"constructor_invocation").unwrap_or(node);
    let name=annotation_name_node(node,java)?;
    let name_text=node_text(name,text);
    if !name_text.split('.').all(|part|!part.is_empty()
        && part.chars().all(|c|c.is_alphanumeric()||c=='_'||c=='$')) {return None;}
    let ty=scope.type_ref(name_text,false,Vec::new());
    if constructor && !java && !resolved_type_entry(index,&ty,ctx).is_some_and(|entry|entry.sym.is_annotation) {
        return None;
    }
    let arguments=node.child_by_field_name("arguments")
        .or_else(||child_of_kind(invocation,"value_arguments"))
        .or_else(||child_of_kind(invocation,"call_suffix").and_then(|suffix|child_of_kind(suffix,"value_arguments")));
    if arguments.is_some_and(|args|args.named_child_count()>*remaining) {return None;}
    let arguments=arguments.map(|args|args.named_children(&mut args.walk())
        .filter(|n|!n.is_extra()).map(|arg|annotation_argument(index,ctx,arg,text,scope,java,depth+1,remaining)).collect())
        .unwrap_or_default();
    if *remaining==0 {return None;}
    Some(SourceAnnotation {ty,use_site_target:child_of_kind(node,"use_site_target")
        .map(|n|node_text(n,text).trim_end_matches(':').into()),arguments,
        range:node.byte_range(),name_range:name.byte_range()})
}

pub(super) fn annotation_name_node(node:Node<'_>,java:bool)->Option<Node<'_>> {
    if java {node.child_by_field_name("name")}
    else if node.kind()=="call_expression" {node.named_child(0)}
    else {child_of_kind(child_of_kind(node,"constructor_invocation").unwrap_or(node),"user_type")}
}

fn annotation_argument(index:&dyn InferenceIndex,ctx:&FileCtx,node:Node<'_>,text:&str,
    scope:&TypeScope,java:bool,depth:usize,remaining:&mut usize)->AnnotationArgument {
    let (name,value)=if node.kind()=="element_value_pair" {
        (node.child_by_field_name("key").map(|n|node_text(n,text).into()),node.child_by_field_name("value"))
    } else if node.kind()=="value_argument" {
        let children:Vec<_>=node.named_children(&mut node.walk()).filter(|n|!n.is_extra()).collect();
        if children.len()==2 && node.children(&mut node.walk()).any(|child|child.kind()=="=") {
            (Some(node_text(children[0],text).into()),children.last().copied())
        } else {(None,children.first().copied())}
    } else {(None,Some(node))};
    let Some(value)=value else {return AnnotationArgument {name,range:node.byte_range(),value:unsupported()};};
    AnnotationArgument {name,range:value.byte_range(),value:annotation_value(index,ctx,value,text,scope,java,depth,remaining)}
}

fn annotation_value(index:&dyn InferenceIndex,ctx:&FileCtx,node:Node<'_>,text:&str,
    scope:&TypeScope,java:bool,depth:usize,remaining:&mut usize)->Fact<AnnotationValue> {
    if depth>=MAX_SOURCE_ANNOTATION_DEPTH || *remaining==0 || node.has_error() || node.is_missing() {return unsupported();}
    *remaining-=1;
    let spelling=node_text(node,text);
    if spelling=="true" || spelling=="false" {return Knowledge::Found(AnnotationValue::Boolean(spelling=="true"));}
    if let Some(value)=decode_string_expression(node,text,java).or_else(||annotation_integer(node,text,java)) {
        return Knowledge::Found(value);
    }
    if matches!(node.kind(),"element_value_array_initializer"|"collection_literal") {
        if node.named_child_count()>*remaining {return unsupported();}
        return Knowledge::Found(AnnotationValue::Array(node.named_children(&mut node.walk())
            .filter(|n|!n.is_extra()).map(|child|annotation_argument(index,ctx,child,text,scope,java,depth+1,remaining)).collect()));
    }
    if java && matches!(node.kind(),"annotation"|"marker_annotation")
        || !java && matches!(node.kind(),"call_expression"|"constructor_invocation") {
        return source_annotation(index,ctx,node,text,scope,java,true,depth+1,remaining)
            .map(|annotation|Knowledge::Found(AnnotationValue::Annotation(Box::new(annotation))))
            .unwrap_or_else(unsupported);
    }
    let class_name=if java && node.kind()=="class_literal" {spelling.strip_suffix(".class")}
        else if !java && node.kind()=="navigation_expression" {spelling.strip_suffix("::class")} else {None};
    if let Some(name)=class_name.filter(|name|name.split('.').all(|part|!part.is_empty()
        && part.chars().all(|c|c.is_alphanumeric()||c=='_'))) {
        return Knowledge::Found(AnnotationValue::Class(scope.type_ref(name,false,Vec::new())));
    }
    annotation_enum(index,node,text,scope,ctx).map(Knowledge::Found).unwrap_or_else(unsupported)
}

fn member_facts(
    index: &dyn InferenceIndex,
    file: &str,
    text: &str,
    symbol: &IndexedSymbol,
    node: Node<'_>,
    scope: &TypeScope,
    java: bool,
    annotations: &[SourceAnnotation],
    property_access: Option<&Fact<JvmPropertyAccess>>,
) -> DeclaredMemberFacts {
    let modifiers = child_of_kind(node, "modifiers");
    let mut words = Vec::new();
    if let Some(modifiers) = modifiers.filter(|_| symbol.kind != SymbolKind::Function) {
        let mut cursor = modifiers.walk();
        for modifier in modifiers.children(&mut cursor) {
            if !matches!(modifier.kind(), "annotation" | "marker_annotation") {
                words.push(node_text(modifier, text));
            }
        }
    }
    let interface = node
        .parent()
        .is_some_and(|parent| parent.kind() == "interface_body");
    let method = symbol.kind == SymbolKind::Function;
    let flags = symbol.source_member;
    let visibility = if method {
        flags.visibility().unwrap_or(Visibility::Public)
    } else if words.contains(&"private") {
        Visibility::Private
    } else if words.contains(&"protected") {
        Visibility::Protected
    } else if words.contains(&"public") || !java || interface {
        Visibility::Public
    } else {
        Visibility::Package
    };
    let jvm_field = matches!(
        property_access,
        Some(Knowledge::Found(JvmPropertyAccess::Field))
    );
    let unsupported_jvm = annotations.iter().any(|annotation| {
        matches!(
            annotation.ty.qualification.as_deref(),
            None | Some("kotlin.jvm")
        ) && !(jvm_field && annotation.ty.name == "JvmField")
            && matches!(
                annotation.ty.name.as_str(),
                "JvmName" | "JvmStatic" | "JvmField" | "JvmSynthetic" | "JvmOverloads"
            )
    }) || flags.jvm_restricted()
        || words.iter().any(|word| {
            matches!(
                *word,
                "suspend" | "external" | "expect" | "inline" | "internal"
            )
        });
    let plain_owner = java || symbol.container.is_some() && !unsupported_kotlin_owner(node, text);
    // A Kotlin public property denotes accessors as well as a backing field. Only a private
    // authored property has a field identity in this slice; generated accessor identity is separate.
    let field_known = java
        || jvm_field
        || symbol.kind != SymbolKind::Property
        || visibility == Visibility::Private
            && !words.contains(&"abstract")
            && !["getter", "setter", "property_delegate"]
                .iter()
                .any(|kind| child_of_kind(node, kind).is_some())
            && (words.contains(&"lateinit")
                || node
                    .children(&mut node.walk())
                    .any(|child| child.kind() == "="));
    let mut cursor = node.walk();
    let header_clean = node
        .children(&mut cursor)
        .filter(|child| {
            !matches!(
                child.kind(),
                "block" | "function_body" | "class_body" | "interface_body"
            )
        })
        .all(|child| !child.has_error() && !child.is_missing());
    let known = header_clean
        && !unsupported_jvm
        && plain_owner
        && field_known
        && (!method
            || matches!(
                flags.implementation(),
                Some(
                    crate::symbol::MemberImplementation::Concrete
                        | crate::symbol::MemberImplementation::Abstract
                )
            ) && flags.dispatch().is_some()
                && flags.visibility().is_some())
        && symbol.ext_receiver.is_none();
    let signature = if known {
        signature(index, file, symbol, scope, java, node, text)
    } else {
        unsupported()
    };
    DeclaredMemberFacts {
        definition: definition(file, symbol),
        signature,
        dispatch: if known {
            Knowledge::Found(if method {
                flags.dispatch().unwrap()
            } else if words.contains(&"static")
                || java && interface && symbol.kind == SymbolKind::Property
            {
                Dispatch::Static
            } else {
                Dispatch::Instance
            })
        } else {
            unsupported()
        },
        visibility: if known {
            Knowledge::Found(visibility)
        } else {
            unsupported()
        },
        synthetic: if known {
            Knowledge::Found(false)
        } else {
            unsupported()
        },
    }
}

fn unsupported_kotlin_owner(mut node: Node<'_>, text: &str) -> bool {
    while let Some(parent) = node.parent() {
        if matches!(parent.kind(), "object_declaration" | "companion_object") {
            return true;
        }
        if parent.kind() == "class_declaration"
            && child_of_kind(parent, "modifiers").is_some_and(|modifiers| {
                modifiers
                    .named_children(&mut modifiers.walk())
                    .any(|modifier| node_text(modifier, text) == "value")
            })
        {
            return true;
        }
        node = parent;
    }
    false
}

fn signature(
    index: &dyn InferenceIndex,
    file: &str,
    symbol: &IndexedSymbol,
    scope: &TypeScope,
    java: bool,
    node: Node<'_>,
    text: &str,
) -> Fact<MemberSignature> {
    // Post-name dimensions remain outside the source-ABI slice, including fields/parameters
    // whose writer still stores their base type. Keep their descriptors unavailable.
    if java && has_post_name_dimensions(node) {
        return unsupported();
    }
    let ctx = FileCtx::new(symbol.package.clone(), Vec::new());
    let type_parameters = member_type_parameters(index, file, symbol);
    let mut unsupported_types = Vec::new();
    let mut convert = |ty: &TypeRef| {
        let value = jvm_type(index, ty, &ctx, scope, java, &type_parameters);
        if value.is_none() {
            unsupported_types.push(IncompleteReason::UnsupportedType(ty.name.clone()));
        }
        value
    };
    let result = if symbol.kind == SymbolKind::Property {
        symbol
            .value_type
            .as_ref()
            .and_then(&mut convert)
            .and_then(|ty| MemberSignature::field(&ty).ok())
    } else {
        let params: Option<Vec<_>> = symbol.params.iter().map(&mut convert).collect();
        let returns = match &symbol.return_type {
            Some(ty) => convert(ty),
            None if !java
                && child_of_kind(node, "function_body")
                    .is_some_and(|body| node_text(body, text).trim_start().starts_with('{')) =>
            {
                Some(JvmType::Void)
            }
            None => None,
        };
        // Missing/unsupported parameter facts may not shorten the JVM descriptor.
        match (params, returns) {
            (Some(params), Some(returns))
                if symbol.arity == Some(params.len().min(u8::MAX as usize) as u8)
                    && !symbol.has_vararg =>
            {
                MemberSignature::method(&params, &returns).ok()
            }
            _ => None,
        }
    };
    result.map(Knowledge::Found).unwrap_or_else(|| {
        if unsupported_types.is_empty() {
            unsupported()
        } else {
            Knowledge::Unknown(unsupported_types)
        }
    })
}

pub(super) fn member_type_parameters(
    index: &dyn InferenceIndex,
    file: &str,
    symbol: &IndexedSymbol,
) -> Vec<String> {
    let mut parameters = symbol.type_params.clone();
    if let Some(container) = &symbol.container {
        for entry in index.entries_for_file(file) {
            let owner = container_name(&entry.sym);
            if entry.sym.kind.is_type_like()
                && entry.sym.package == symbol.package
                && (container == &owner || container.starts_with(&format!("{owner}.")))
            {
                parameters.extend(entry.sym.type_params.iter().cloned());
            }
        }
    }
    parameters
}

#[cfg(test)]
thread_local! {
    static REFERENCE_PARSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(super) fn reference_parse_count() -> usize {
    REFERENCE_PARSES.with(std::cell::Cell::get)
}

/// Eligibility prepared with a verified source tree off the editor lock. The text and file stay
/// attached so the host cannot accidentally apply its ranges to a different accepted input.
#[derive(Clone)]
pub struct PreparedSourceReferences {
    pub(super) file: String,
    pub(super) text: std::sync::Arc<str>,
    pub(super) tree: Tree,
    pub(super) ranges: std::collections::BTreeSet<(usize, usize)>,
}

pub fn prepare_source_references(
    index: &dyn InferenceIndex,
    file: &str,
    text: std::sync::Arc<str>,
    tree: &Tree,
) -> PreparedSourceReferences {
    let ranges = index
        .entries_for_file(file)
        .into_iter()
        .filter(|entry| {
            matches!(
                entry.sym.kind,
                SymbolKind::Class
                    | SymbolKind::Interface
                    | SymbolKind::EnumClass
                    | SymbolKind::Object
            ) && ordinary_reference_type(entry, &text, tree)
        })
        .map(|entry| (entry.sym.start_byte, entry.sym.end_byte))
        .collect();
    PreparedSourceReferences {
        file: file.into(),
        text,
        tree: tree.clone(),
        ranges,
    }
}

pub(super) fn ordinary_reference_types(
    index: &dyn InferenceIndex,
    file: &str,
    text: &str,
) -> std::collections::BTreeSet<(usize, usize)> {
    let entries: Vec<_> = index
        .entries_for_file(file)
        .into_iter()
        .filter(|entry| {
            matches!(
                entry.sym.kind,
                SymbolKind::Class
                    | SymbolKind::Interface
                    | SymbolKind::EnumClass
                    | SymbolKind::Object
            ) && entry.sym.type_params.is_empty()
        })
        .collect();
    if entries.is_empty() {
        return std::collections::BTreeSet::new();
    }
    #[cfg(test)]
    REFERENCE_PARSES.with(|count| count.set(count.get() + 1));
    let tree = if file.ends_with(".java") {
        let mut parser = tree_sitter::Parser::new();
        if parser
            .set_language(&tree_sitter_java::LANGUAGE.into())
            .is_err()
        {
            return std::collections::BTreeSet::new();
        }
        let Some(tree) = parser.parse(text, None) else {
            return std::collections::BTreeSet::new();
        };
        tree
    } else {
        crate::parser::KotlinParser::new().parse(text)
    };
    entries
        .into_iter()
        .filter(|entry| ordinary_reference_type(entry, text, &tree))
        .map(|entry| (entry.sym.start_byte, entry.sym.end_byte))
        .collect()
}

fn ordinary_reference_type(entry: &crate::index::Entry, text: &str, tree: &Tree) -> bool {
    if entry
        .sym
        .name
        .trim_matches('`')
        .contains(['.', '/', ';', '[', ']', '<', '>'])
        || !entry.sym.type_params.is_empty()
    {
        return false;
    }
    let java = entry.path.ends_with(".java");
    let Some(node) = declaration_node(tree, &entry.sym, java) else {
        return false;
    };
    if !matches!(
        node.kind(),
        "class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "record_declaration"
            | "object_declaration"
    ) {
        return false;
    }
    if child_of_kind(node, "modifiers").is_some_and(|modifiers| {
        node_text(modifiers, text)
            .split_whitespace()
            .any(|word| matches!(word, "value" | "inline" | "expect"))
    }) {
        return false;
    }
    let mut ancestor = node.parent();
    while let Some(parent) = ancestor {
        if child_of_kind(parent, "type_parameters").is_some() {
            return false;
        }
        if matches!(
            parent.kind(),
            "block"
                | "function_body"
                | "function_declaration"
                | "method_declaration"
                | "lambda_literal"
        ) {
            return false;
        }
        ancestor = parent.parent();
    }
    let clean = node
        .children(&mut node.walk())
        .filter(|child| {
            !matches!(
                child.kind(),
                "class_body" | "interface_body" | "enum_body" | "enum_class_body"
            )
        })
        .all(|child| !child.has_error() && !child.is_missing());
    clean
}

pub(crate) fn has_post_name_dimensions(node: Node<'_>) -> bool {
    if child_of_kind(node, "dimensions").is_some() {
        return true;
    }
    let mut cursor = node.walk();
    let found = node
        .named_children(&mut cursor)
        .filter(|child| {
            matches!(
                child.kind(),
                "formal_parameters" | "formal_parameter" | "variable_declarator"
            )
        })
        .any(has_post_name_dimensions);
    found
}

fn jvm_type(
    index: &dyn InferenceIndex,
    ty: &TypeRef,
    ctx: &FileCtx,
    scope: &TypeScope,
    java: bool,
    type_parameters: &[String],
) -> Option<JvmType> {
    if ty.nullable || ty.qualification.is_none() && type_parameters.contains(&ty.name) {
        return None;
    }
    if java && ty.name.ends_with("[]") {
        let mut element = ty.clone();
        element.name = ty.name.strip_suffix("[]")?.to_string();
        return jvm_type(index, &element, ctx, scope, java, type_parameters)
            .map(|ty| JvmType::Array(Box::new(ty)));
    }
    source_scalar_jvm_type(index, ty, ctx, java, type_parameters)
}

/// Shared source ABI identity for the ordinary scalar signature slice. Java primitive tokens
/// and Kotlin builtins are distinct spellings; unresolved lookalikes must not be interchanged.
pub(crate) fn source_scalar_jvm_type(
    index: &dyn InferenceIndex,
    ty: &TypeRef,
    ctx: &FileCtx,
    java: bool,
    type_parameters: &[String],
) -> Option<JvmType> {
    if ty.nullable || ty.qualification.is_none() && type_parameters.contains(&ty.name) {
        return None;
    }
    if !ty.args.is_empty() {
        return None;
    }
    if ty.name == "String" && ty.qualification.as_deref() == Some("java.lang") {
        return Some(JvmType::String);
    }
    if java && ty.qualification.is_none() {
        let primitive = match ty.name.as_str() {
            "boolean" => Some(JvmType::Boolean),
            "byte" => Some(JvmType::Byte),
            "char" => Some(JvmType::Char),
            "short" => Some(JvmType::Short),
            "int" => Some(JvmType::Int),
            "long" => Some(JvmType::Long),
            "float" => Some(JvmType::Float),
            "double" => Some(JvmType::Double),
            "void" => Some(JvmType::Void),
            _ => None,
        };
        if primitive.is_some() {
            return primitive;
        }
    }
    let resolved = infer::declared_type(index, ty, ctx);
    let Type::Class {
        name,
        package: Some(package),
        container: None,
        ..
    } = resolved
    else {
        return None;
    };
    if name == "String" && matches!(package.as_str(), "kotlin" | "java.lang") {
        return Some(JvmType::String);
    }
    if java || package != "kotlin" {
        return None;
    }
    Some(match name.as_str() {
        "Boolean" => JvmType::Boolean,
        "Byte" => JvmType::Byte,
        "Char" => JvmType::Char,
        "Short" => JvmType::Short,
        "Int" => JvmType::Int,
        "Long" => JvmType::Long,
        "Float" => JvmType::Float,
        "Double" => JvmType::Double,
        "Unit" => JvmType::Void,
        _ => return None,
    })
}

fn unsupported<T>() -> Fact<T> {
    Knowledge::Unknown(vec![IncompleteReason::UnsupportedSource])
}

pub(super) const MAX_SOURCE_ANNOTATION_DEPTH:usize=128;
pub(super) const MAX_SOURCE_ANNOTATION_NODES:usize=4096;
pub(super) const MAX_SOURCE_ANNOTATION_BYTES:usize=64*1024;
const MAX_DECODED_STRING_SCALARS:usize=16_384;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceModifiers {
    pub visibility: Fact<Visibility>,
    pub is_static: Fact<bool>,
    pub is_final: Fact<bool>,
    pub is_abstract: Fact<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnnotationIntegerType { Int, Long }

/// Each segment is one decoded Unicode scalar (UTF-16 offsets), including a whole escape.
/// A supplementary scalar's surrogate pair cannot be edited as two independent characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StringSourceSegment {
    pub decoded: Range<usize>,
    pub source: Range<usize>,
}

pub(super) fn declaration_ranges(node: Node<'_>, symbol: &IndexedSymbol)
    -> (Range<usize>, Range<usize>, Range<usize>, Option<Range<usize>>)
{
    let body = node.children(&mut node.walk()).find(|child| matches!(child.kind(),
        "block" | "function_body" | "class_body" | "interface_body" | "enum_body"
        | "enum_class_body" | "annotation_type_body" | "constructor_body"))
        .map(|body| body.byte_range());
    (node.byte_range(), node.start_byte()..body.as_ref().map_or(node.end_byte(), |r|r.start),
        symbol.start_byte..symbol.end_byte, body)
}

fn applied_modifiers(node: Node<'_>, text: &str, symbol: &IndexedSymbol, java: bool,
    member: Option<&DeclaredMemberFacts>) -> SourceModifiers
{
    let mut words = Vec::new();
    if let Some(modifiers) = child_of_kind(node, "modifiers") {
        let mut pending: Vec<_> = modifiers.children(&mut modifiers.walk()).collect();
        while let Some(child) = pending.pop() {
            if matches!(child.kind(), "annotation" | "marker_annotation") { continue; }
            if child.child_count() == 0 { words.push(node_text(child, text)); }
            else { pending.extend(child.children(&mut child.walk())); }
        }
    }
    let clean = !child_of_kind(node,"modifiers").is_some_and(|mods|mods.has_error());
    let has = |word| words.contains(&word);
    let interface = node.parent().is_some_and(|body|body.kind()=="interface_body"
        || body.kind()=="annotation_type_body");
    let visibility = if !clean || has("internal") { unsupported() }
        else if let Some(member) = member.filter(|member|member.visibility.is_found()) { member.visibility.clone() }
        else if let Some(value) = symbol.source_member.visibility() { Knowledge::Found(value) }
        else { Knowledge::Found(if has("private") {Visibility::Private}
            else if has("protected") {Visibility::Protected}
            else if has("public") || !java || interface {Visibility::Public} else {Visibility::Package}) };
    let is_static = if !clean { unsupported() }
        else if let Some(member) = member.filter(|member|member.dispatch.is_found()) {
            member.dispatch.clone().map_found(|dispatch|dispatch==Dispatch::Static)
        } else if java && symbol.kind.is_type_like() && symbol.container.is_some() {
            if has("static") {Knowledge::Found(true)} else {symbol.source_member.dispatch()
                .map(|dispatch|Knowledge::Found(dispatch==Dispatch::Static)).unwrap_or_else(unsupported)}
        } else if java { Knowledge::Found(has("static") || interface && symbol.kind==SymbolKind::Property) }
        else if symbol.kind==SymbolKind::Function && !symbol.source_member.jvm_restricted() {
            symbol.source_member.dispatch().map(|dispatch|Knowledge::Found(dispatch==Dispatch::Static)).unwrap_or_else(unsupported)
        } else { unsupported() };
    let is_final = if !clean { unsupported() }
        else if has("final") || java && interface && symbol.kind==SymbolKind::Property { Knowledge::Found(true) }
        else if has("open") || has("abstract") || symbol.kind==SymbolKind::Interface { Knowledge::Found(false) }
        else if java && symbol.kind!=SymbolKind::EnumClass && node.kind()!="record_declaration" { Knowledge::Found(false) }
        // shortcut: implicit Kotlin finality and compiler projections require a lowering witness.
        else { unsupported() };
    let is_abstract = if !clean { unsupported() }
        else if has("abstract") || symbol.kind==SymbolKind::Interface { Knowledge::Found(true) }
        else if let Some(implementation)=symbol.source_member.implementation() {
            Knowledge::Found(implementation==crate::symbol::MemberImplementation::Abstract)
        } else if java && symbol.kind!=SymbolKind::EnumClass { Knowledge::Found(false) } else { unsupported() };
    SourceModifiers {visibility,is_static,is_final,is_abstract}
}

/// Shared by extraction and the source acceptance boundary; no client-supplied map is trusted.
pub(super) fn decode_string_expression(node: Node<'_>, text: &str, java: bool) -> Option<AnnotationValue> {
    if node.byte_range().len()>MAX_SOURCE_ANNOTATION_BYTES {return None;}
    let spelling=node_text(node,text);
    if node.kind()=="string_literal" && !node.has_error() && !node.is_missing() && spelling.len()>=2
        && spelling.starts_with('"') && spelling.ends_with('"') && !spelling.starts_with("\"\"\"")
        && !spelling.contains('\\') && !spelling[1..spelling.len()-1].contains(['"','\r','\n'])
        && (java || !node.named_children(&mut node.walk()).any(|child|child.kind()=="interpolation")) {
        return Some(AnnotationValue::String {value:spelling[1..spelling.len()-1].into(),
            content_range:node.start_byte()+1..node.end_byte()-1});
    }
    fn append(node: Node<'_>, text: &str, java: bool, value: &mut String,
        segments: &mut Vec<StringSourceSegment>, decoded: &mut usize, depth: usize,remaining:&mut usize) -> Option<()> {
        if depth>MAX_SOURCE_ANNOTATION_DEPTH || *remaining==0 || node.has_error() || node.is_missing() { return None; }
        *remaining-=1;
        if matches!(node.kind(),"binary_expression"|"additive_expression") {
            let children: Vec<_>=node.named_children(&mut node.walk()).filter(|n|!n.is_extra()).collect();
            if children.len()!=2 || !node.children(&mut node.walk()).any(|n|n.kind()=="+") {return None;}
            append(children[0],text,java,value,segments,decoded,depth+1,remaining)?;
            return append(children[1],text,java,value,segments,decoded,depth+1,remaining);
        }
        if node.kind()=="parenthesized_expression" {
            let children: Vec<_>=node.named_children(&mut node.walk()).filter(|n|!n.is_extra()).collect();
            if children.len()!=1 {return None;}
            return append(children[0],text,java,value,segments,decoded,depth+1,remaining);
        }
        let spelling=node_text(node,text);
        if node.kind()!="string_literal" || !spelling.starts_with('"') || !spelling.ends_with('"')
            || spelling.starts_with("\"\"\"") || spelling.len()<2 {return None;}
        if !java && node.named_children(&mut node.walk()).any(|child|child.kind()=="interpolation") {return None;}
        let end=node.end_byte()-1;
        let mut position=node.start_byte()+1;
        while position<end {
            if segments.len()>=MAX_DECODED_STRING_SCALARS {return None;}
            let start=position;
            let mut ch=text.get(position..end)?.chars().next()?;
            position+=ch.len_utf8();
            if ch=='\\' {
                let escaped=text.get(position..end)?.chars().next()?;
                position+=escaped.len_utf8();
                ch=match escaped {
                    'b'=>'\u{8}', 't'=>'\t','n'=>'\n','r'=>'\r','\''=>'\'','"'=>'"','\\'=>'\\',
                    '$' if !java=>'$', 'f' if java=>'\u{c}', 's' if java=>' ',
                    'u'=> {
                        if java { while text.get(position..end)?.starts_with('u') {position+=1;} }
                        let unit=u16::from_str_radix(text.get(position..position.checked_add(4)?)?,16).ok()?;
                        position+=4;
                        if position>end {return None;}
                        if (0xd800..=0xdbff).contains(&unit) {
                            if !text.get(position..end)?.starts_with("\\u") {return None;}
                            position+=2;
                            if java {while text.get(position..end)?.starts_with('u') {position+=1;}}
                            let low=u16::from_str_radix(text.get(position..position.checked_add(4)?)?,16).ok()?;
                            position+=4;
                            if position>end || !(0xdc00..=0xdfff).contains(&low) {return None;}
                            char::from_u32(0x10000+((u32::from(unit)-0xd800)<<10)+(u32::from(low)-0xdc00))?
                        } else {
                            let scalar=char::from_u32(u32::from(unit))?;
                            // Java Unicode escapes run before tokenization; these change its token stream.
                            if java && matches!(scalar,'"'|'\\'|'\n'|'\r') {return None;}
                            scalar
                        }
                    },
                    '0'..='7' if java=> {
                        let mut octal=escaped.to_digit(8)?;
                        for _ in 1..if escaped<='3' {3} else {2} {
                            let Some(next)=text.get(position..end)?.chars().next().and_then(|n|n.to_digit(8)) else {break;};
                            octal=octal*8+next;position+=1;
                        }
                        char::from_u32(octal)?
                    },
                    _=>return None,
                };
            } else if matches!(ch,'"'|'\r'|'\n') {
                return None;
            }
            let next=decoded.checked_add(ch.len_utf16())?;
            segments.push(StringSourceSegment {decoded:*decoded..next,source:start..position});
            value.push(ch);*decoded=next;
        }
        Some(())
    }
    let mut value=String::new();let mut segments=Vec::new();let mut decoded=0;
    let mut remaining=MAX_SOURCE_ANNOTATION_NODES;
    append(node,text,java,&mut value,&mut segments,&mut decoded,0,&mut remaining)?;
    Some(AnnotationValue::MappedString {value,segments})
}

fn annotation_integer(node:Node<'_>,text:&str,java:bool)->Option<AnnotationValue> {
    let mut literal=node;let mut negative=false;
    if matches!(node.kind(),"unary_expression"|"prefix_expression") {
        let children:Vec<_>=node.named_children(&mut node.walk()).filter(|n|!n.is_extra()).collect();
        if children.len()!=1 {return None;}
        let operator=text.get(node.start_byte()..children[0].start_byte())?.trim();
        if !matches!(operator,"-"|"+") {return None;} negative=operator=="-";literal=children[0];
    }
    if !matches!(literal.kind(),"number_literal"|"integer_literal"|"long_literal"|"decimal_integer_literal"
        |"hex_integer_literal"|"octal_integer_literal"|"binary_integer_literal") {return None;}
    let spelling=node_text(literal,text).replace('_',"");
    let long=spelling.ends_with('L') || java && spelling.ends_with('l');
    let digits=if long {&spelling[..spelling.len()-1]} else {&spelling};
    let (radix,digits)=if let Some(rest)=digits.strip_prefix("0x").or_else(||digits.strip_prefix("0X")) {(16,rest)}
        else if let Some(rest)=digits.strip_prefix("0b").or_else(||digits.strip_prefix("0B")) {(2,rest)}
        else if java && digits.len()>1 && digits.starts_with('0') {(8,&digits[1..])} else {(10,digits)};
    let magnitude=i128::from_str_radix(digits,radix).ok()?;
    let ty=if long || !java && (magnitude>i128::from(i32::MAX)+i128::from(negative)) {
        AnnotationIntegerType::Long
    } else {AnnotationIntegerType::Int};
    let bits=if ty==AnnotationIntegerType::Long {64} else {32};
    let mut value=if java && radix!=10 {
        if magnitude >= 1i128<<bits {return None;}
        if magnitude>=1i128<<(bits-1) {magnitude-(1i128<<bits)} else {magnitude}
    } else {magnitude};
    if negative { value=if java && radix!=10 && value==-(1i128<<(bits-1)) {value} else {-value}; }
    if value < -(1i128<<(bits-1)) || value >= 1i128<<(bits-1) {return None;}
    Some(AnnotationValue::Integer {value:i64::try_from(value).ok()?,ty})
}

fn resolved_type_entry<'a>(index:&'a dyn InferenceIndex,ty:&TypeRef,ctx:&FileCtx)->Option<&'a crate::index::Entry> {
    let Type::Class {name,package:Some(package),container,..}=infer::declared_type(index,ty,ctx) else {return None;};
    let candidates:Vec<_>=index.lookup_type(&name).into_iter().filter(|entry|
        entry.sym.package==package && entry.sym.container==container).collect();
    match candidates.as_slice() {[only]=>Some(*only),_=>None}
}

fn annotation_enum(index:&dyn InferenceIndex,node:Node<'_>,text:&str,scope:&TypeScope,ctx:&FileCtx)->Option<AnnotationValue> {
    if !matches!(node.kind(),"field_access"|"navigation_expression"|"identifier"|"simple_identifier") {return None;}
    let spelling=node_text(node,text);
    let (owner,name)=spelling.rsplit_once('.').map_or((None,spelling),|(owner,name)|(Some(owner),name));
    if !name.chars().all(|c|c.is_alphanumeric()||c=='_'||c=='$') || name.is_empty() {return None;}
    let (ty,entry_name)=if let Some(owner)=owner {(scope.type_ref(owner,false,Vec::new()),name.to_string())}
        else {
            let imported=scope.type_ref(name,false,Vec::new());
            (scope.type_ref(imported.qualification.as_deref()?,false,Vec::new()),imported.name)
        };
    let owner=resolved_type_entry(index,&ty,ctx)?;
    if owner.sym.kind!=SymbolKind::EnumClass {return None;}
    let owner_name=container_name(&owner.sym);
    let entries:Vec<_>=index.members_of(&owner_name).iter().filter(|entry|entry.sym.name==entry_name
        && entry.sym.kind==SymbolKind::EnumEntry && entry.sym.package==owner.sym.package
        && entry.sym.container.as_deref()==Some(owner_name.as_str())).collect();
    if entries.len()!=1 {return None;}
    Some(AnnotationValue::Enum {ty,name:entry_name})
}
