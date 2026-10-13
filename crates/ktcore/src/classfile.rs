//! Source-less JVM declaration metadata. Reading a class never loads or executes it.
//!
//! These are positive binary facts, not a verified callable surface: bytecode and annotation
//! payloads are opaque, and generic signatures remain unparsed. A consumer must establish output
//! freshness, selected origin, language callability, and any real source mapping separately. In
//! particular, a public JVM constructor need not be callable from Kotlin source.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{ensure, Context, Result};
use noak::descriptor::{BaseType, MethodDescriptor, TypeDescriptor};
use noak::reader::{cpool, Attribute, Class, DecodeMany};
use sha2::{Digest, Sha256};

use crate::jvm::{ClassIdentity, InputIdentity};

/// An oversized input is unsupported, never evidence that a declaration is absent.
pub const MAX_CLASS_FILE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassFileFacts {
    pub identity: ClassIdentity,
    pub major_version: u16,
    pub minor_version: u16,
    pub access_flags: u16,
    pub super_name: Option<Arc<str>>,
    pub interfaces: Vec<Arc<str>>,
    pub signature: Option<Arc<str>>,
    /// A debug filename hint, not an editable source location or identity.
    pub source_file: Option<Arc<str>>,
    pub attribute_names: Vec<Arc<str>>,
    pub fields: Vec<MemberFacts>,
    pub methods: Vec<MemberFacts>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberFacts {
    pub name: Arc<str>,
    pub descriptor: Arc<str>,
    /// Raw JVM flags; synthetic/bridge members are retained without exposing them as source APIs.
    pub access_flags: u16,
    pub signature: Option<Arc<str>>,
    /// Includes annotation presence, without interpreting annotation or Code payloads.
    pub attribute_names: Vec<Arc<str>>,
}

/// Decode declaration metadata for one selected artifact origin and exact classfile contents.
///
/// This is intentionally not a JVM verifier. Unsupported or malformed declaration metadata is an
/// error; success says nothing about compiler-producer closure or dirty-source compatibility.
/// Text processing is limited to sixteen times the input length, including repeated pool uses.
pub fn decode(origin_id: &str, bytes: &[u8]) -> Result<ClassFileFacts> {
    ensure!(!origin_id.is_empty(), "classfile origin is empty");
    let class = read_class(bytes)?;
    let mut strings = Strings {
        pool: class.pool(),
        values: HashMap::new(),
        remaining_text_work: bytes.len() * 16,
    };
    let internal_name = strings.class_name(class.this_class())?;
    let super_name = class
        .super_class()
        .map(|index| strings.class_name(index))
        .transpose()?;
    let interfaces = class
        .interfaces()
        .into_iter()
        .map(|index| strings.class_name(index?))
        .collect::<Result<_>>()?;
    let mut fields = Vec::new();
    for field in class.fields() {
        let field = field?;
        let name = strings.get(field.name())?;
        let descriptor = strings.get(field.descriptor())?;
        strings.charge(name.len() + descriptor.len())?;
        ensure!(unqualified_name(&name), "invalid field name");
        let ty = TypeDescriptor::parse(class.pool().retrieve(field.descriptor())?)?;
        validate_type_name(&ty)?;
        let attributes = attributes(&mut strings, field.attributes(), false)?;
        fields.push(MemberFacts {
            name,
            descriptor,
            access_flags: field.access_flags().bits(),
            signature: attributes.signature,
            attribute_names: attributes.names,
        });
    }
    let mut methods = Vec::new();
    for method in class.methods() {
        let method = method?;
        let name = strings.get(method.name())?;
        let descriptor = strings.get(method.descriptor())?;
        strings.charge(name.len() + descriptor.len())?;
        ensure!(
            name.as_ref() == "<init>"
                || name.as_ref() == "<clinit>"
                || (unqualified_name(&name) && !name.contains(['<', '>'])),
            "invalid method name"
        );
        let shape = MethodDescriptor::parse(class.pool().retrieve(method.descriptor())?)?;
        for ty in shape.parameters().chain(shape.return_type()) {
            validate_type_name(&ty)?;
        }
        ensure!(
            name.as_ref() != "<init>" || shape.return_type().is_none(),
            "constructor returns a value"
        );
        ensure!(
            name.as_ref() != "<clinit>"
                || (descriptor.as_ref() == "()V" && method.access_flags().bits() & 0x0008 != 0),
            "invalid class initializer"
        );
        let attributes = attributes(&mut strings, method.attributes(), false)?;
        methods.push(MemberFacts {
            name,
            descriptor,
            access_flags: method.access_flags().bits(),
            signature: attributes.signature,
            attribute_names: attributes.names,
        });
    }
    for members in [&fields, &methods] {
        let mut keys = HashSet::new();
        for member in members {
            ensure!(
                keys.insert((&member.name, &member.descriptor)),
                "duplicate JVM member"
            );
        }
    }
    let attributes = attributes(&mut strings, class.attributes(), true)?;
    Ok(ClassFileFacts {
        identity: ClassIdentity {
            origin_id: origin_id.to_owned(),
            internal_name: internal_name.to_string(),
            input: InputIdentity::Binary {
                content_sha256: Sha256::digest(bytes).into(),
            },
        },
        major_version: class.version().major,
        minor_version: class.version().minor,
        access_flags: class.access_flags().bits(),
        super_name,
        interfaces,
        signature: attributes.signature,
        source_file: attributes.source_file,
        attribute_names: attributes.names,
        fields,
        methods,
    })
}

/// Normalize an erased method descriptor; this establishes neither source callability nor
/// generic arguments. Nested binary names need InnerClasses identity and remain unsupported.
pub fn method_types(
    descriptor: &str,
) -> Option<(Vec<crate::types::TypeRef>, Option<crate::types::TypeRef>)> {
    fn type_ref(ty: TypeDescriptor<'_>) -> Option<crate::types::TypeRef> {
        let mut result = match ty.base {
            BaseType::Object(name) => {
                let internal = name
                    .chars()
                    .collect::<std::result::Result<String, _>>()
                    .ok()?;
                if !internal_name(&internal) || internal.contains('$') {
                    return None;
                }
                let (package, simple) = internal.rsplit_once('/').unwrap_or(("", &internal));
                let mut result = crate::types::TypeRef::simple(simple);
                // Even the default package is explicit; the consumer's package cannot rebind it.
                result.qualification = Some(package.replace('/', "."));
                result
            }
            base => crate::types::TypeRef::simple(match base {
                BaseType::Boolean => "boolean",
                BaseType::Byte => "byte",
                BaseType::Short => "short",
                BaseType::Integer => "int",
                BaseType::Long => "long",
                BaseType::Float => "float",
                BaseType::Double => "double",
                BaseType::Char => "char",
                BaseType::Object(_) => unreachable!(),
            }),
        };
        result
            .name
            .push_str(&"[]".repeat(usize::from(ty.dimensions)));
        Some(result)
    }
    let encoded = noak::MString::from(descriptor);
    let method = MethodDescriptor::parse(&encoded).ok()?;
    Some((
        method
            .parameters()
            .map(type_ref)
            .collect::<Option<Vec<_>>>()?,
        match method.return_type() {
            Some(ty) => Some(type_ref(ty)?),
            None => None,
        },
    ))
}

/// Prove only a narrow, requires-only module descriptor has no ordinary declarations.
///
/// `false` or an error is inconclusive. This is not a module graph reader or compiler-producer
/// closure proof. Unsupported attributes, tables, versions, and constant-pool tags stay unknown.
pub fn is_requires_only_module(bytes: &[u8]) -> Result<bool> {
    let class = read_class(bytes)?;
    if class.access_flags().bits() != 0x8000
        || !(53..=61).contains(&class.version().major)
        || class.version().minor != 0
        || class.super_class().is_some()
        || class.interfaces().into_iter().next().is_some()
        || class.fields().into_iter().next().is_some()
        || class.methods().into_iter().next().is_some()
    {
        return Ok(false);
    }
    let mut strings = Strings {
        pool: class.pool(),
        values: HashMap::new(),
        remaining_text_work: bytes.len() * 16,
    };
    if strings.class_name(class.this_class())?.as_ref() != "module-info" {
        return Ok(false);
    }
    // Validate even unused references; accepting the header alone is not an emptiness proof.
    for (index, item) in class.pool().iter_indices() {
        match item {
            cpool::Item::Utf8(_) => {
                let text = strings.get(cpool::Index::new(index.as_u16())?)?;
                strings.charge(text.len())?;
            }
            cpool::Item::Class(_) => {
                strings.class_name(cpool::Index::new(index.as_u16())?)?;
            }
            cpool::Item::Module(_) => {
                strings.module_name(index.as_u16())?;
            }
            _ => return Ok(false),
        }
    }
    let mut names = HashSet::new();
    let mut module = false;
    for attribute in class.attributes() {
        let attribute = attribute?;
        let name = strings.get(attribute.name())?;
        strings.charge(name.len())?;
        if !names.insert(name.clone()) {
            return Ok(false);
        }
        match name.as_ref() {
            "Module" => {
                if !requires_only(&mut strings, attribute.content())? {
                    return Ok(false);
                }
                module = true;
            }
            "SourceFile" => {
                let hint = strings.attribute_string(attribute.content())?;
                strings.charge(hint.len())?;
            }
            "RuntimeVisibleAnnotations" | "RuntimeInvisibleAnnotations" => {
                if !marker_annotations(&mut strings, attribute.content())? {
                    return Ok(false);
                }
            }
            _ => return Ok(false),
        }
    }
    Ok(module)
}

fn read_class(bytes: &[u8]) -> Result<Class<'_>> {
    ensure!(
        bytes.len() <= MAX_CLASS_FILE_BYTES,
        "classfile exceeds metadata size limit"
    );
    ensure!(bytes.len() >= 10, "truncated classfile header");
    // Check before Noak allocates its u16-sized pool from an untrusted header.
    let pool_count = u16::from_be_bytes([bytes[8], bytes[9]]) as usize;
    ensure!(
        pool_count.saturating_sub(1) <= (bytes.len() - 10) / 3,
        "truncated constant pool"
    );
    let class = Class::new(bytes).context("classfile structure")?;
    ensure!(
        class.buffer_size() == bytes.len(),
        "trailing classfile bytes"
    );
    Ok(class)
}

fn take_u16(input: &mut &[u8]) -> Result<u16> {
    let (word, rest) = input
        .split_first_chunk::<2>()
        .context("truncated module metadata")?;
    *input = rest;
    Ok(u16::from_be_bytes(*word))
}

fn requires_only(strings: &mut Strings<'_, '_>, mut input: &[u8]) -> Result<bool> {
    let length = input.len();
    let name = strings.module_name(take_u16(&mut input)?)?;
    if name.as_ref() == "java.base" || take_u16(&mut input)? != 0 {
        return Ok(false);
    }
    strings.optional_utf8(take_u16(&mut input)?)?;
    let count = take_u16(&mut input)? as usize;
    ensure!(
        length == 16 + 6 * count,
        "invalid requires-only Module length"
    );
    let mut required = HashSet::new();
    let mut base = false;
    for _ in 0..count {
        let dependency = strings.module_name(take_u16(&mut input)?)?;
        let flags = take_u16(&mut input)?;
        strings.optional_utf8(take_u16(&mut input)?)?;
        if dependency == name || !required.insert(dependency.clone()) {
            return Ok(false);
        }
        if dependency.as_ref() == "java.base" {
            if flags != 0x8000 {
                return Ok(false);
            }
            base = true;
        } else if !matches!(flags, 0 | 0x0020) {
            return Ok(false);
        }
    }
    // Noak's typed Require rejects legal version index 0 and does not enforce exact consumption.
    for _ in 0..4 {
        if take_u16(&mut input)? != 0 {
            return Ok(false);
        }
    }
    Ok(base && input.is_empty())
}

fn marker_annotations(strings: &mut Strings<'_, '_>, mut input: &[u8]) -> Result<bool> {
    let count = take_u16(&mut input)? as usize;
    ensure!(input.len() == count * 4, "invalid marker annotation length");
    for _ in 0..count {
        let index = cpool::Index::new(take_u16(&mut input)?)?;
        let descriptor = strings.get(index)?;
        strings.charge(descriptor.len())?;
        let ty = TypeDescriptor::parse(strings.pool.retrieve(index)?)?;
        validate_type_name(&ty)?;
        if ty.dimensions != 0
            || !matches!(ty.base, BaseType::Object(_))
            || take_u16(&mut input)? != 0
        {
            return Ok(false);
        }
    }
    Ok(input.is_empty())
}

// Intern by constant-pool index: repeated references to a large UTF8 item must not multiply its
// allocation by the number of members/attributes in an otherwise small classfile.
struct Strings<'pool, 'input> {
    pool: &'pool cpool::ConstantPool<'input>,
    values: HashMap<u16, Arc<str>>,
    remaining_text_work: usize,
}

impl<'input> Strings<'_, 'input> {
    fn get(&mut self, index: cpool::Index<cpool::Utf8<'input>>) -> Result<Arc<str>> {
        if let Some(value) = self.values.get(&index.as_u16()) {
            return Ok(value.clone());
        }
        let raw = self.pool.retrieve(index)?;
        let value = raw
            .chars()
            .collect::<std::result::Result<String, _>>()
            .map_err(|surrogate| {
                anyhow::anyhow!("unpaired surrogate in JVM metadata: {surrogate:x}")
            })?;
        let value: Arc<str> = value.into();
        self.values.insert(index.as_u16(), value.clone());
        Ok(value)
    }

    fn class_name(&mut self, index: cpool::Index<cpool::Class<'input>>) -> Result<Arc<str>> {
        let name = self.get(self.pool.get(index)?.name)?;
        self.charge(name.len())?;
        ensure!(internal_name(&name), "invalid JVM internal class name");
        Ok(name)
    }

    fn module_name(&mut self, index: u16) -> Result<Arc<str>> {
        let module = self
            .pool
            .get(cpool::Index::<cpool::Module<'input>>::new(index)?)?;
        let name = self.get(module.name)?;
        self.charge(name.len())?;
        // shortcut: escaped/non-ASCII module names need a full module-name reader before admission.
        ensure!(
            name.split('.').all(|part| {
                let mut chars = part.chars();
                chars
                    .next()
                    .is_some_and(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphabetic())
                    && chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
            }),
            "unsupported module name"
        );
        Ok(name)
    }

    fn optional_utf8(&mut self, index: u16) -> Result<()> {
        if index != 0 {
            let value = self.get(cpool::Index::new(index)?)?;
            self.charge(value.len())?;
        }
        Ok(())
    }

    fn charge(&mut self, bytes: usize) -> Result<()> {
        self.remaining_text_work = self
            .remaining_text_work
            .checked_sub(bytes)
            .context("classfile metadata text budget exceeded")?;
        Ok(())
    }

    fn attribute_string(&mut self, bytes: &[u8]) -> Result<Arc<str>> {
        ensure!(bytes.len() == 2, "invalid UTF8 attribute length");
        self.get(cpool::Index::new(u16::from_be_bytes([bytes[0], bytes[1]]))?)
    }
}

#[derive(Default)]
struct Attributes {
    names: Vec<Arc<str>>,
    signature: Option<Arc<str>>,
    source_file: Option<Arc<str>>,
}

fn attributes<'input>(
    strings: &mut Strings<'_, 'input>,
    input: DecodeMany<'input, Attribute<'input>, u16>,
    class: bool,
) -> Result<Attributes> {
    let mut result = Attributes::default();
    let mut synthetic = false;
    for attribute in input {
        let attribute = attribute?;
        let name = strings.get(attribute.name())?;
        match name.as_ref() {
            "Signature" => {
                ensure!(result.signature.is_none(), "duplicate Signature attribute");
                result.signature = Some(strings.attribute_string(attribute.content())?);
            }
            "SourceFile" if class => {
                ensure!(
                    result.source_file.is_none(),
                    "duplicate SourceFile attribute"
                );
                result.source_file = Some(strings.attribute_string(attribute.content())?);
            }
            "Synthetic" => {
                ensure!(
                    !synthetic && attribute.content().is_empty(),
                    "invalid Synthetic attribute"
                );
                synthetic = true;
            }
            _ => {} // Unknown attributes and nested payloads stay opaque; no recursive decoding.
        }
        result.names.push(name);
    }
    Ok(result)
}

fn unqualified_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['.', ';', '[', '/'])
}

fn internal_name(name: &str) -> bool {
    name.split('/').all(unqualified_name)
}

fn validate_type_name(ty: &TypeDescriptor<'_>) -> Result<()> {
    if let BaseType::Object(name) = &ty.base {
        let name = name
            .chars()
            .collect::<std::result::Result<String, _>>()
            .map_err(|_| anyhow::anyhow!("unpaired surrogate in descriptor"))?;
        ensure!(internal_name(&name), "invalid class name in descriptor");
    }
    Ok(())
}
