use ktcore::classfile::{
    decode, is_requires_only_module, ClassFileFacts, MemberFacts, MAX_CLASS_FILE_BYTES,
};
use ktcore::jvm::InputIdentity;

const PERSON: &[u8] = include_bytes!("fixtures/classfile/pluginfixture/Person.class");
const MODEL: &[u8] = include_bytes!("fixtures/classfile/pluginfixture/Model.class");
const COMPANION: &[u8] = include_bytes!("fixtures/classfile/pluginfixture/Model$Companion.class");
const SERIALIZER: &[u8] =
    include_bytes!("fixtures/classfile/pluginfixture/Model$$serializer.class");
const NOARG_OFF: &[u8] = include_bytes!("fixtures/classfile/noarg-off/NeedsLabel.class");
const NOARG_ON: &[u8] = include_bytes!("fixtures/classfile/noarg-on/NeedsLabel.class");
const UNICODE: &[u8] = include_bytes!("fixtures/classfile/unicodefixture/名前.class");

fn method<'a>(class: &'a ClassFileFacts, name: &str, descriptor: &str) -> &'a MemberFacts {
    class
        .methods
        .iter()
        .find(|m| m.name.as_ref() == name && m.descriptor.as_ref() == descriptor)
        .unwrap()
}

#[test]
fn nested_signature_normalization_requires_whole_class_tokens() {
    use ktcore::classfile::{method_types_with_inner_classes, signature_with_inner_classes, InnerClassFacts};
    let rows = [InnerClassFacts { inner: "a$Entry".into(), outer: Some("a".into()),
        name: Some("Entry".into()), access_flags: 0x0009 }];
    for (input, expected) in [
        ("(ILa$Entry;)V", "(ILa.Entry;)V"),
        ("<K:Ljava/lang/Object;>(ILa$Entry<TK;>;)V", "<K:Ljava/lang/Object;>(ILa.Entry<TK;>;)V"),
        ("<La$Entry:Ljava/lang/Object;>(TLa$Entry;La$Entry<TLa$Entry;>;)V",
            "<La$Entry:Ljava/lang/Object;>(TLa$Entry;La.Entry<TLa$Entry;>;)V"),
    ] {
        assert_eq!(signature_with_inner_classes(input, &rows).as_deref(), Some(expected));
    }
    assert_eq!(method_types_with_inner_classes("(ILa$Entry;)V", &rows).unwrap().0.len(), 2);
    let generic = signature_with_inner_classes("<K:Ljava/lang/Object;>(ILa$Entry<TK;>;)V", &rows).unwrap();
    assert!(ktcore::classfile::signature::method_signature(&generic, "(ILa$Entry;)V", &[]).is_some());
    for input in ["(LPrefixLa$Entry;)V", "(Lliteral$Entry;)V",
        "(LPrefixLa$Entry<Ljava/lang/String;>;)V", "(TLa$Entry;)V"] {
        assert_eq!(signature_with_inner_classes(input, &rows).as_deref(), Some(input),
            "a row for a$Entry cannot rewrite another class or type variable");
    }
    assert!(method_types_with_inner_classes("(LPrefixLa$Entry;)V", &rows).is_none());
    assert!(method_types_with_inner_classes("(Lliteral$Entry;)V", &rows).is_none());
    let chain = [rows[0].clone(), InnerClassFacts { inner: "a$Entry$Deep".into(),
        outer: Some("a$Entry".into()), name: Some("Deep".into()), access_flags: 0x4019 }];
    assert_eq!(signature_with_inner_classes("(ILa$Entry$Deep;)V", &chain).as_deref(), Some("(ILa.Entry.Deep;)V"));
    assert!(method_types_with_inner_classes("(ILa$Entry$Deep;)V", &chain).is_some());
    assert!(signature_with_inner_classes("(La$Entry$Deep;)V", &chain[1..]).is_none());
    for flags in [0x0001, 0x000a] {
        let mut invalid = chain.clone(); invalid[0].access_flags = flags;
        assert!(signature_with_inner_classes("(La$Entry$Deep;)V", &invalid).is_none());
    }
    let mut duplicate = chain.to_vec(); duplicate.push(chain[0].clone());
    assert!(signature_with_inner_classes("(La$Entry$Deep;)V", &duplicate).is_none());
    let package = [InnerClassFacts { inner: "pkg/Outer$Hidden".into(), outer: Some("pkg/Outer".into()),
        name: Some("Hidden".into()), access_flags: 0x0408 }];
    assert!(signature_with_inner_classes("Lpkg/Outer$Hidden;", &package).is_none());
    assert!(ktcore::classfile::signature_with_inner_classes_in_package("Lpkg/Outer$Hidden;", &package, "other").is_none());
    let normalized = ktcore::classfile::signature_with_inner_classes_in_package("Lpkg/Outer$Hidden;", &package, "pkg").unwrap();
    assert_eq!(normalized, "Lpkg/Outer.Hidden;");
    assert!(ktcore::classfile::signature::field_signature(&normalized, "Lpkg/Outer$Hidden;", &[]).is_some());
}

#[test]
fn actual_lombok_getter_has_binary_origin_and_descriptor() {
    let class = decode("project:plugin-fixture:main", PERSON).unwrap();
    assert_eq!(class.identity.origin_id, "project:plugin-fixture:main");
    assert_eq!(class.identity.internal_name, "pluginfixture/Person");
    let InputIdentity::Binary { content_sha256 } = class.identity.input else {
        panic!("binary identity required");
    };
    assert_eq!(
        content_sha256,
        [
            0x71, 0x03, 0x82, 0x37, 0x0e, 0x23, 0xcc, 0xa9, 0x62, 0x4d, 0xcb, 0x70, 0x60, 0x67,
            0xa8, 0x88, 0xa9, 0xeb, 0xb8, 0x83, 0x90, 0xf1, 0xe9, 0xa2, 0x58, 0xcb, 0x65, 0x1f,
            0x5d, 0x59, 0xa5, 0x9d
        ]
    );
    assert_eq!((class.major_version, class.minor_version), (61, 0));
    assert_eq!(class.access_flags, 0x0031);
    assert_eq!(class.super_name.as_deref(), Some("java/lang/Object"));
    assert_eq!(class.source_file.as_deref(), Some("Person.java"));
    assert_eq!(
        method(&class, "getName", "()Ljava/lang/String;").access_flags,
        0x0001
    );
    assert_eq!(
        method(&class, "<init>", "(Ljava/lang/String;)V").access_flags,
        0x0001
    );
    assert_eq!(class.fields[0].name.as_ref(), "name");
    assert_eq!(class.fields[0].access_flags, 0x0002);
}

#[test]
fn actual_serializer_owner_generic_signature_and_bridge_are_preserved() {
    let model = decode("project:plugin-fixture:main", MODEL).unwrap();
    let companion = decode("project:plugin-fixture:main", COMPANION).unwrap();
    let serializer = decode("project:plugin-fixture:main", SERIALIZER).unwrap();
    assert!(!model
        .methods
        .iter()
        .any(|m| m.name.as_ref() == "serializer"));
    let field = model
        .fields
        .iter()
        .find(|f| f.name.as_ref() == "Companion")
        .unwrap();
    assert_eq!(field.descriptor.as_ref(), "Lpluginfixture/Model$Companion;");
    assert_eq!(field.access_flags, 0x0019);
    assert_eq!(
        companion.identity.internal_name,
        "pluginfixture/Model$Companion"
    );
    let callable = method(
        &companion,
        "serializer",
        "()Lkotlinx/serialization/KSerializer;",
    );
    assert_eq!(callable.access_flags, 0x0011);
    assert_eq!(
        callable.signature.as_deref(),
        Some("()Lkotlinx/serialization/KSerializer<Lpluginfixture/Model;>;")
    );
    assert_eq!(
        serializer.signature.as_deref(),
        Some(
            "Ljava/lang/Object;Lkotlinx/serialization/internal/GeneratedSerializer<Lpluginfixture/Model;>;"
        )
    );
    assert_eq!(
        serializer.interfaces[0].as_ref(),
        "kotlinx/serialization/internal/GeneratedSerializer"
    );
    assert_eq!(
        method(
            &serializer,
            "deserialize",
            "(Lkotlinx/serialization/encoding/Decoder;)Ljava/lang/Object;"
        )
        .access_flags,
        0x1041
    );
    assert_eq!(
        method(
            &serializer,
            "deserialize",
            "(Lkotlinx/serialization/encoding/Decoder;)Lpluginfixture/Model;"
        )
        .access_flags,
        0x0001
    );
    assert_eq!(
        method(
            &serializer,
            "childSerializers",
            "()[Lkotlinx/serialization/KSerializer;"
        )
        .signature
        .as_deref(),
        Some("()[Lkotlinx/serialization/KSerializer<*>;")
    );
    assert!(serializer
        .attribute_names
        .iter()
        .any(|a| a.as_ref() == "RuntimeVisibleAnnotations"));
}

#[test]
fn actual_noarg_public_constructor_does_not_prove_kotlin_callability() {
    let off = decode("project:noarg:main", NOARG_OFF).unwrap();
    let on = decode("project:noarg:main", NOARG_ON).unwrap();
    assert!(!off
        .methods
        .iter()
        .any(|m| m.name.as_ref() == "<init>" && m.descriptor.as_ref() == "()V"));
    assert_eq!(method(&on, "<init>", "()V").access_flags, 0x0001);
    assert_ne!(off.identity, on.identity);
    // The independent compiler controls accept this constructor in Java and reject it in Kotlin.
    // This inventory deliberately has no source-callability or source-location field to guess.
}

#[test]
fn actual_modified_utf8_preserves_unicode_declarations() {
    let class = decode("project:unicode:main", UNICODE).unwrap();
    assert_eq!(class.identity.internal_name, "unicodefixture/名前");
    assert_eq!(
        method(
            &class,
            "変換",
            "(Lunicodefixture/名前;)Lunicodefixture/名前;"
        )
        .access_flags,
        0x0001
    );
}

#[test]
fn identity_changes_with_selected_origin_and_exact_contents() {
    let first = decode("origin-one", PERSON).unwrap();
    let second = decode("origin-two", PERSON).unwrap();
    assert_ne!(first.identity, second.identity);
    let mut bytes = PERSON.to_vec();
    let hint = bytes
        .windows(b"Person.java".len())
        .position(|b| b == b"Person.java")
        .unwrap();
    bytes[hint] = b'Q';
    let changed = decode("origin-one", &bytes).unwrap();
    assert_eq!(first.methods, changed.methods);
    assert_ne!(first.identity, changed.identity);
}

#[test]
fn repeated_constant_pool_names_share_storage() {
    let class = decode("origin", MODEL).unwrap();
    let constructors: Vec<_> = class
        .methods
        .iter()
        .filter(|m| m.name.as_ref() == "<init>")
        .collect();
    assert!(constructors.len() >= 2);
    assert!(std::sync::Arc::ptr_eq(
        &constructors[0].name,
        &constructors[1].name
    ));
}

#[test]
fn repeated_large_pool_references_exhaust_a_bounded_text_budget() {
    let name = vec![b'x'; 60_000];
    let (mut bytes, start) = metadata_class(&name, b"()V", None);
    let method = bytes[start + 12..start + 20].to_vec();
    bytes.truncate(start + 12);
    bytes[start + 10..start + 12].copy_from_slice(&512u16.to_be_bytes());
    for _ in 0..512 {
        bytes.extend_from_slice(&method);
    }
    bytes.extend_from_slice(&[0, 0]);
    let error = decode("origin", &bytes).unwrap_err().to_string();
    assert!(error.contains("metadata text budget"), "{error}");
}

#[test]
fn every_compiler_fixture_truncation_is_rejected() {
    for bytes in [
        PERSON, MODEL, COMPANION, SERIALIZER, NOARG_OFF, NOARG_ON, UNICODE, ANNOTATED,
    ] {
        for cut in 0..bytes.len() {
            assert!(
                decode("origin", &bytes[..cut]).is_err(),
                "accepted prefix {cut}/{}",
                bytes.len()
            );
        }
    }
}

// A flat metadata-only class with one abstract method. Payloads test reader boundaries rather than
// JVM executable validity. The compiler-produced fixtures above supply the independent oracle.
fn metadata_class(
    name: &[u8],
    descriptor: &[u8],
    attribute: Option<(&[u8], &[u8])>,
) -> (Vec<u8>, usize) {
    let mut bytes = vec![0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 61, 0, 8];
    for (index, value) in [
        b"fixture/C".as_slice(),
        b"",
        b"java/lang/Object",
        b"",
        name,
        descriptor,
        attribute
            .map(|(name, _)| name)
            .unwrap_or(b"FutureAttribute"),
    ]
    .into_iter()
    .enumerate()
    {
        if index == 1 || index == 3 {
            bytes.extend_from_slice(&[7, 0, index as u8]);
        } else {
            bytes.push(1);
            bytes.extend_from_slice(&(value.len() as u16).to_be_bytes());
            bytes.extend_from_slice(value);
        }
    }
    let class_start = bytes.len();
    bytes.extend_from_slice(&[0x04, 0x21, 0, 2, 0, 4, 0, 0, 0, 0, 0, 1]);
    bytes.extend_from_slice(&[0x04, 0x01, 0, 5, 0, 6, 0, u8::from(attribute.is_some())]);
    if let Some((_, payload)) = attribute {
        bytes.extend_from_slice(&[0, 7]);
        bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        bytes.extend_from_slice(payload);
    }
    bytes.extend_from_slice(&[0, 0]);
    (bytes, class_start)
}

#[test]
fn invalid_constant_pool_tag_index_and_type_are_errors() {
    let (bytes, start) = metadata_class(b"read", b"()V", None);
    assert!(decode("origin", &bytes).is_ok());
    for (at, value) in [
        (10, 255),
        (start + 3, 255),
        (start + 3, 5),
        (start + 15, 0),
        (start + 17, 2),
    ] {
        let mut corrupt = bytes.clone();
        corrupt[at] = value;
        assert!(
            decode("origin", &corrupt).is_err(),
            "bad CP reference at {at}"
        );
    }
}

#[test]
fn invalid_descriptor_shapes_and_internal_names_are_errors() {
    for descriptor in [
        b"(V)V".as_slice(),
        b"()",
        b"()Vx",
        b"()[V",
        b"(L;)V",
        b"()Lfoo.bar;",
        b"()L/foo;",
        b"()Lfoo//bar;",
    ] {
        let (bytes, _) = metadata_class(b"read", descriptor, None);
        assert!(decode("origin", &bytes).is_err(), "accepted {descriptor:?}");
    }
    let (mut bytes, _) = metadata_class(b"read", b"()V", None);
    bytes[13 + b"fixture".len()] = b'.';
    assert!(decode("origin", &bytes).is_err());
    let (bytes, _) = metadata_class(b"<init>", b"()I", None);
    assert!(decode("origin", &bytes).is_err());
}

#[test]
fn selected_signature_requires_exact_length_and_utf8_pool_type() {
    for payload in [b"\0".as_slice(), b"\0\x02", b"\0\0", b"\0\x05\0"] {
        let (bytes, _) = metadata_class(b"read", b"()V", Some((b"Signature", payload)));
        assert!(decode("origin", &bytes).is_err());
    }
}

#[test]
fn future_and_code_payloads_remain_opaque() {
    for name in [
        b"FutureAttribute".as_slice(),
        b"Code", // Declaration metadata never interprets executable bytecode.
    ] {
        let (bytes, _) = metadata_class(b"read", b"()V", Some((name, &[255; 32])));
        let class = decode("origin", &bytes).unwrap();
        assert_eq!(class.methods[0].attribute_names[0].as_bytes(), name);
    }
}

#[test]
fn synthetic_attribute_does_not_rewrite_raw_flags() {
    let (bytes, _) = metadata_class(b"read", b"()V", Some((b"Synthetic", b"")));
    let class = decode("origin", &bytes).unwrap();
    assert_eq!(class.methods[0].access_flags, 0x0401);
    assert_eq!(class.methods[0].attribute_names[0].as_ref(), "Synthetic");
    let (bytes, _) = metadata_class(b"read", b"()V", Some((b"Synthetic", b"x")));
    assert!(decode("origin", &bytes).is_err());
}

#[test]
fn invalid_and_unrepresentable_modified_utf8_never_becomes_lossy_identity() {
    for name in [b"\xed\xa0\x80".as_slice(), b"\xf0\x9f\x98\x80", b"\0"] {
        let (bytes, _) = metadata_class(name, b"()V", None);
        assert!(decode("origin", &bytes).is_err());
    }
}

#[test]
fn forged_counts_lengths_oversize_and_trailing_bytes_are_errors() {
    let (bytes, start) = metadata_class(b"read", b"()V", Some((b"FutureAttribute", b"")));
    let mut corrupt = bytes.clone();
    corrupt[start + 22..start + 26].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(decode("origin", &corrupt).is_err());
    let mut corrupt = bytes[..10].to_vec();
    corrupt[8..10].copy_from_slice(&u16::MAX.to_be_bytes());
    assert!(decode("origin", &corrupt).is_err());
    let mut corrupt = bytes;
    corrupt.push(0);
    assert!(decode("origin", &corrupt).is_err());
    assert!(decode("origin", &vec![0; MAX_CLASS_FILE_BYTES + 1]).is_err());
    assert!(decode("", PERSON).is_err());
}

#[test]
fn corruptions_of_compiler_metadata_return_without_panicking() {
    for bytes in [PERSON, MODEL, COMPANION, SERIALIZER, UNICODE] {
        for at in (0..bytes.len()).step_by(31) {
            for value in [0, 255] {
                let mut corrupt = bytes.to_vec();
                corrupt[at] = value;
                let _ = decode("origin", &corrupt);
            }
        }
    }
}

#[test]
fn requires_only_module_matches_independent_jdk17_oracle() {
    macro_rules! case {
        ($file:literal, $expected:expr) => {
            (
                $file,
                include_bytes!(concat!("fixtures/classfile/module-only/", $file)).as_slice(),
                $expected,
            )
        };
    }
    for (name, bytes, expected) in [
        case!("kotlin-stdlib-jdk7-1.8.21.class", true),
        case!("kotlin-stdlib-jdk8-1.8.21.class", true),
        case!("kotlin-stdlib-jdk7-2.4.0.class", true),
        case!("kotlin-stdlib-jdk8-2.4.0.class", true),
        case!("kotlinx-coroutines-jdk8-1.11.0.class", true),
        case!("javac-empty-module.class", true),
        case!("zero-requires-version.class", true),
        case!("ordinary-as-module-info.class", false),
        case!("zero-member-interface.class", false),
        case!("wrong-class-flags.class", false),
        case!("extra-class-flags.class", false),
        case!("old-class-version.class", false),
        case!("invalid-module-name-index.class", false),
        case!("wrong-module-name-tag.class", false),
        case!("forged-module-class-name.class", false),
        case!("invalid-empty-module-payload.class", false),
        case!("invalid-module-payload-trailing.class", false),
        case!("duplicate-module-attribute.class", false),
        case!("truncated-module.class", false),
    ] {
        let result = is_requires_only_module(bytes);
        assert_eq!(matches!(result, Ok(true)), expected, "{name}: {result:?}");
    }
    let bytes = include_bytes!("fixtures/classfile/module-only/javac-empty-module.class");
    assert_eq!(bytes.len(), 160);
    for end in 0..bytes.len() {
        assert!(
            !matches!(is_requires_only_module(&bytes[..end]), Ok(true)),
            "prefix={end}"
        );
    }
}

// Flat trust-boundary inputs supplement the compiler/JDK fixtures; they are not an oracle.
fn module_class(payload: &[u16], attributes: &[(u16, &[u8])]) -> Vec<u8> {
    let mut bytes = vec![0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 61, 0, 17];
    for (index, text) in [
        "module-info",
        "",
        "Module",
        "oracle.empty",
        "",
        "java.base",
        "",
        "SourceFile",
        "module-info.java",
        "RuntimeVisibleAnnotations",
        "Lfixture/Marker;",
        "RuntimeInvisibleAnnotations",
        "FutureAttribute",
        "other.module",
        "",
        "",
    ]
    .into_iter()
    .enumerate()
    {
        match index + 1 {
            2 => bytes.extend_from_slice(&[7, 0, 1]),
            5 => bytes.extend_from_slice(&[19, 0, 4]),
            7 | 16 => bytes.extend_from_slice(&[19, 0, 6]),
            15 => bytes.extend_from_slice(&[19, 0, 14]),
            _ => {
                bytes.push(1);
                bytes.extend_from_slice(&(text.len() as u16).to_be_bytes());
                bytes.extend_from_slice(text.as_bytes());
            }
        }
    }
    bytes.extend_from_slice(&[0x80, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0]);
    bytes.extend_from_slice(&(1 + attributes.len() as u16).to_be_bytes());
    bytes.extend_from_slice(&[0, 3]);
    bytes.extend_from_slice(&(payload.len() as u32 * 2).to_be_bytes());
    for word in payload {
        bytes.extend_from_slice(&word.to_be_bytes());
    }
    for (name, payload) in attributes {
        bytes.extend_from_slice(&name.to_be_bytes());
        bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        bytes.extend_from_slice(payload);
    }
    bytes
}

const REQUIRES_BASE: &[u16] = &[5, 0, 0, 1, 7, 0x8000, 0, 0, 0, 0, 0];

#[test]
fn module_tables_indexes_versions_and_flags_must_be_proven() {
    assert!(is_requires_only_module(&module_class(REQUIRES_BASE, &[])).unwrap());
    for (word, value) in [
        (0, 0),
        (0, 2),
        (1, 1),
        (2, 2),
        (3, u16::MAX),
        (4, 0),
        (4, 2),
        (4, 5),
        (4, 15),
        (5, 0),
        (5, 0x0020),
        (6, 2),
        (7, 1),
        (8, 1),
        (9, 1),
        (10, 1),
    ] {
        let mut words = REQUIRES_BASE.to_vec();
        words[word] = value;
        let result = is_requires_only_module(&module_class(&words, &[]));
        assert!(
            !matches!(result, Ok(true)),
            "word {word}={value}: {result:?}"
        );
    }
    for words in [
        vec![5, 0, 0, 0, 0, 0, 0, 0], // missing java.base
        vec![5, 0, 0, 2, 7, 0x8000, 0, 16, 0x8000, 0, 0, 0, 0, 0], // aliases, same name
        vec![5, 0, 0, 2, 7, 0x8000, 0, 15, 0x0040, 0, 0, 0, 0, 0], // unsupported flags
    ] {
        assert!(!matches!(
            is_requires_only_module(&module_class(&words, &[])),
            Ok(true)
        ));
    }
    let versions = [5, 0, 9, 2, 7, 0x8000, 9, 15, 0x0020, 0, 0, 0, 0, 0];
    assert!(is_requires_only_module(&module_class(&versions, &[])).unwrap());
    let bytes = module_class(REQUIRES_BASE, &[]);
    for (offset, value) in [(4, 255), (7, 62)] {
        // preview/future versions
        let mut changed = bytes.clone();
        changed[offset] = value;
        assert!(!matches!(is_requires_only_module(&changed), Ok(true)));
    }
}

#[test]
fn module_known_attributes_require_exact_typed_marker_payloads() {
    assert!(is_requires_only_module(&module_class(
        REQUIRES_BASE,
        &[
            (8, &[0, 9]),
            (10, &[0, 1, 0, 11, 0, 0]),
            (12, &[0, 1, 0, 11, 0, 0]),
        ]
    ))
    .unwrap());
    for (name, payload) in [
        (8, &[0][..]),
        (8, &[0, 0]),
        (8, &[0, 2]),
        (8, &[0, 9, 0]),
        (10, &[255, 255]),
        (10, &[0, 1, 0, 11, 0]),
        (10, &[0, 1, 0, 11, 0, 0, 0]),
        (10, &[0, 1, 0, 11, 0, 1]),
        (10, &[0, 1, 0, 0, 0, 0]),
        (10, &[0, 1, 0, 2, 0, 0]),
        (10, &[0, 1, 0, 9, 0, 0]),
        (13, &[]),
    ] {
        let result = is_requires_only_module(&module_class(REQUIRES_BASE, &[(name, payload)]));
        assert!(
            !matches!(result, Ok(true)),
            "attribute {name} {payload:?}: {result:?}"
        );
    }
    assert!(!matches!(
        is_requires_only_module(&module_class(REQUIRES_BASE, &[(8, &[0, 9]), (8, &[0, 9])])),
        Ok(true)
    ));
}

#[test]
fn module_unused_pool_references_and_input_bounds_cannot_prove_emptiness() {
    let bytes = module_class(REQUIRES_BASE, &[]);
    let unused = bytes.windows(3).rposition(|b| b == [19, 0, 6]).unwrap();
    for (offset, value) in [(unused, 20), (unused + 2, 255)] {
        let mut changed = bytes.clone();
        changed[offset] = value;
        assert!(!matches!(is_requires_only_module(&changed), Ok(true)));
    }
    let mut changed = bytes.clone();
    let utf8 = changed
        .windows(b"Lfixture/Marker;".len())
        .position(|b| b == b"Lfixture/Marker;")
        .unwrap();
    changed[utf8..utf8 + 3].copy_from_slice(&[0xed, 0xa0, 0x80]);
    assert!(is_requires_only_module(&changed).is_err());
    let mut forged = bytes[..10].to_vec();
    forged[8..10].copy_from_slice(&u16::MAX.to_be_bytes());
    assert!(is_requires_only_module(&forged).is_err());
    let mut trailing = bytes;
    trailing.push(0);
    assert!(is_requires_only_module(&trailing).is_err());
    assert!(is_requires_only_module(&vec![0; MAX_CLASS_FILE_BYTES + 1]).is_err());

    let mut repeated = module_class(REQUIRES_BASE, &[]);
    let at = repeated
        .windows(b"oracle.empty".len())
        .position(|b| b == b"oracle.empty")
        .unwrap();
    repeated[at - 2..at].copy_from_slice(&60_000u16.to_be_bytes());
    repeated.splice(at..at + b"oracle.empty".len(), vec![b'x'; 60_000]);
    let pool_end = repeated.windows(3).rposition(|b| b == [19, 0, 6]).unwrap() + 3;
    repeated.splice(pool_end..pool_end, (0..512).flat_map(|_| [19, 0, 4]));
    repeated[8..10].copy_from_slice(&529u16.to_be_bytes());
    let error = is_requires_only_module(&repeated).unwrap_err().to_string();
    assert!(error.contains("metadata text budget"), "{error}");
}

#[test]
fn erased_method_descriptor_types_keep_java_array_rank_and_qualified_component_identity() {
    use ktcore::classfile::method_types;
    let (parameters, returns) =
        method_types("(I[[Ljava/lang/String;)[[Larrayfixture/Item;").unwrap();
    assert_eq!(parameters[0].name, "int");
    assert_eq!(parameters[0].qualification, None);
    assert_eq!(parameters[1].name, "String[][]");
    assert_eq!(parameters[1].qualification.as_deref(), Some("java.lang"));
    let returns = returns.unwrap();
    assert_eq!(returns.name, "Item[][]");
    assert_eq!(returns.qualification.as_deref(), Some("arrayfixture"));
    assert!(returns.args.is_empty());
    for (descriptor, name) in [
        ("()[Z", "boolean[]"),
        ("()[B", "byte[]"),
        ("()[S", "short[]"),
        ("()[I", "int[]"),
        ("()[J", "long[]"),
        ("()[F", "float[]"),
        ("()[D", "double[]"),
        ("()[C", "char[]"),
    ] {
        let result = method_types(descriptor).unwrap().1.unwrap();
        assert_eq!(result.name, name);
        assert_eq!(result.qualification, None);
    }
    let default_package = method_types("()[LItem;").unwrap().1.unwrap();
    assert_eq!(default_package.qualification.as_deref(), Some(""));
    assert_eq!(method_types("()V").unwrap(), (Vec::new(), None));
    for unsupported in [
        "()",
        "()[V",
        "()Igarbage",
        "()[Lmissing//Item;",
        "()[Lpkg/Outer$Inner;",
    ] {
        assert!(method_types(unsupported).is_none(), "{unsupported}");
    }
}

#[test]
fn method_parameter_count_keeps_nested_binary_identity_opaque() {
    use ktcore::classfile::{method_parameter_count, method_types};
    let class = ktcore::classfile::decode(
        "fixture:nested-return",
        include_bytes!("fixtures/classfile/shadowfixture/NestedBoundMid.class"),
    )
    .unwrap();
    let method = class
        .methods
        .iter()
        .find(|method| method.name.as_ref() == "f" && method.access_flags & 0x1040 == 0)
        .unwrap();
    assert!(method_types(&method.descriptor).is_none());
    assert_eq!(method_parameter_count(&method.descriptor), Some(0));
    assert_eq!(method_parameter_count("(J[[Lpkg/Outer$Inner;D)V"), Some(3));
    for malformed in ["()", "()[V", "()Igarbage", "(Lpkg//Nested;)V", "(V)V"] {
        assert_eq!(method_parameter_count(malformed), None, "{malformed}");
    }
}

#[test]
fn erased_field_descriptor_types_preserve_nominal_identity_and_array_rank() {
    use ktcore::classfile::field_type;
    for (descriptor, name, package) in [
        ("I", "int", None),
        ("[[I", "int[][]", None),
        ("Ljava/lang/String;", "String", Some("java.lang")),
        ("[[Larrayfixture/Item;", "Item[][]", Some("arrayfixture")),
        ("LItem;", "Item", Some("")),
    ] {
        let ty = field_type(descriptor).unwrap();
        assert_eq!(ty.name, name);
        assert_eq!(ty.qualification.as_deref(), package);
    }
    for descriptor in [
        "",
        "V",
        "[V",
        "Igarbage",
        "Lpkg/Outer$Inner;",
        "Lmissing//Item;",
    ] {
        assert!(field_type(descriptor).is_none(), "{descriptor}");
    }
}

const ANNOTATED: &[u8] = include_bytes!("fixtures/classfile/annotationfixture/CustomPoint.class");

#[test]
fn javac_declaration_annotations_preserve_typed_values_and_order() {
    use ktcore::classfile::{Annotation, AnnotationElement, AnnotationValue as V, BinaryAnnotation};
    let annotation = |descriptor: &str, elements: Vec<(&str, V)>| Annotation {
        descriptor: descriptor.into(),
        elements: elements.into_iter().map(|(name, value)|AnnotationElement {name:name.into(),value}).collect(),
    };
    let class = decode("compile-only:annotation-fixture", ANNOTATED).unwrap();
    assert_eq!(class.super_name.as_deref(), Some("annotationfixture/InjectionPoint"));
    assert_eq!(class.signature.as_deref(), Some("<T:Ljava/lang/Number;>Lannotationfixture/InjectionPoint;"));
    assert_eq!(class.annotations, vec![
        BinaryAnnotation {runtime_visible:true,annotation:annotation("Lannotationfixture/AtCode;",vec![
            ("value", V::String("CUSTOM".into())), ("namespace", V::String("example".into())),
        ])},
        BinaryAnnotation {runtime_visible:false,annotation:annotation("Lannotationfixture/Invisible;",vec![
            ("value", V::String("classInvisible".into())),
        ])},
    ]);
    assert_eq!(class.fields[0].annotations, vec![BinaryAnnotation {
        runtime_visible:false,annotation:annotation("Lannotationfixture/Invisible;",vec![
            ("value", V::String("fieldInvisible".into())),
        ]),
    }]);
    let nested=annotation("Lannotationfixture/Nested;",vec![("value",V::String("nestedValue".into()))]);
    assert_eq!(method(&class,"probe","()V").annotations,vec![
        BinaryAnnotation {runtime_visible:true,annotation:annotation("Lannotationfixture/Payload;",vec![
            ("b",V::Byte(-7)), ("c",V::Char(0xd800)), ("d",V::DoubleBits(0x8000000000000000)),
            ("f",V::FloatBits(0x7f800000)), ("i",V::Int(-17)), ("j",V::Long(i64::MAX)),
            ("s",V::Short(-123)), ("z",V::Boolean(true)), ("text",V::String("nul\0😃".into())),
            ("utf16",V::StringUtf16(vec![0xd800,120,0xdc00,0xd83d,0xde03,0].into())),
            ("choice",V::Enum {descriptor:"Lannotationfixture/Choice;".into(),name:"SECOND".into()}),
            ("type",V::Class("[[I".into())), ("nested",V::Annotation(Box::new(nested))),
            ("array",V::Array(vec![V::Int(1),V::Int(-2),V::Int(3)])),
        ])},
        BinaryAnnotation {runtime_visible:false,annotation:annotation("Lannotationfixture/Invisible;",vec![
            ("value",V::String("methodInvisible".into())),
        ])},
    ]);
    assert!(method(&class,"<init>","()V").annotations.is_empty());
    assert!(class.annotations.iter().map(|value|value.annotation.heap_size()).sum::<usize>()>0);
}

// Extend the existing metadata fixture's constant pool; this constructs bytes without parsing them.
fn annotation_class(payload: &[u8]) -> Vec<u8> {
    let (mut bytes,start)=metadata_class(b"read",b"()V",Some((b"RuntimeVisibleAnnotations",payload)));
    let mut pool=Vec::new();
    for text in ["Lfixture/Mark;","value"] { // 8, 9
        pool.push(1);pool.extend_from_slice(&(text.len() as u16).to_be_bytes());pool.extend_from_slice(text.as_bytes());
    }
    pool.push(3);pool.extend_from_slice(&(-7i32).to_be_bytes()); // 10
    for text in ["literal","I","V","FIRST","bad"] { // 11..15
        pool.push(1);pool.extend_from_slice(&(text.len() as u16).to_be_bytes());pool.extend_from_slice(text.as_bytes());
    }
    pool.push(3);pool.extend_from_slice(&65536i32.to_be_bytes()); // 16, outside byte/char/short ranges
    pool.push(3);pool.extend_from_slice(&2i32.to_be_bytes()); // 17
    pool.push(5);pool.extend_from_slice(&i64::MIN.to_be_bytes()); // 18, reserved 19
    pool.push(4);pool.extend_from_slice(&0x7fc12345u32.to_be_bytes()); // 20, noncanonical NaN
    pool.push(6);pool.extend_from_slice(&0x7ff8000000001234u64.to_be_bytes()); // 21, reserved 22
    bytes[8..10].copy_from_slice(&23u16.to_be_bytes());
    bytes.splice(start..start,pool);
    bytes
}

fn annotation_payload(tag: u8, index: u16) -> Vec<u8> {
    let mut result=vec![0,1,0,8,0,1,0,9,tag];
    result.extend_from_slice(&index.to_be_bytes());result
}

#[test]
fn annotation_strings_preserve_and_intern_unpaired_utf16_without_loss() {
    use ktcore::classfile::AnnotationValue as V;
    let mut payload=vec![0,1,0,8,0,1,0,9,b'[',0,2];
    payload.extend_from_slice(&[b's',0,11,b's',0,11]);
    for (raw,expected) in [
        (b"\xed\xa0\x80".as_slice(),vec![0xd800]),
        (b"\xed\xb0\x80".as_slice(),vec![0xdc00]),
        (b"\xed\xa0\x80x\xed\xb0\x80\xed\xa0\xbd\xed\xb8\x83\xc0\x80".as_slice(),vec![0xd800,120,0xdc00,0xd83d,0xde03,0]),
    ] {
        let mut bytes=annotation_class(&payload);
        let at=bytes.windows(b"literal".len()).position(|value|value==b"literal").unwrap();
        bytes[at-2..at].copy_from_slice(&(raw.len() as u16).to_be_bytes());
        bytes.splice(at..at+7,raw.iter().copied());
        let facts=decode("origin",&bytes).unwrap();
        let V::Array(values)=&facts.methods[0].annotations[0].annotation.elements[0].value else {panic!("array expected")};
        let (V::StringUtf16(first),V::StringUtf16(second))=(&values[0],&values[1]) else {panic!("exact UTF16 expected")};
        assert_eq!(first.as_ref(),expected.as_slice());
        assert!(std::sync::Arc::ptr_eq(first,second),"pool references share one UTF16 allocation");
    }
}

#[test]
fn declaration_annotation_rejects_malformed_payloads_instead_of_dropping_values() {
    for name in [b"RuntimeVisibleAnnotations".as_slice(),b"RuntimeInvisibleAnnotations"] {
        let (bytes,_)=metadata_class(b"read",b"()V",Some((name,&[255;32])));
        assert!(decode("origin",&bytes).is_err());
    }
    let valid=annotation_payload(b'I',10);
    assert!(decode("origin",&annotation_class(&valid)).is_ok());
    for cut in 0..valid.len() {
        assert!(decode("origin",&annotation_class(&valid[..cut])).is_err(),"accepted cut {cut}");
    }
    let mut invalid=Vec::new();
    let mut trailing=valid.clone();trailing.push(0);invalid.push(trailing);
    for (offset,value) in [(1,255),(3,0),(3,2),(3,12),(3,15),(7,0),(7,2),(7,8),(8,255),(10,0),(10,2),(10,11),(10,255)] {
        let mut changed=valid.clone();changed[offset]=value;invalid.push(changed);
    }
    // Duplicate element names, even when the first value is otherwise valid.
    let mut duplicate=valid.clone();duplicate[5]=2;duplicate.extend_from_slice(&valid[6..]);invalid.push(duplicate);
    for (tag,index) in [(b'B',16),(b'C',10),(b'C',16),(b'S',16),(b'Z',17),(b'J',10),(b'J',19),(b'F',10),(b'D',10),(b's',10),(b'c',15),(b'c',6)] {
        invalid.push(annotation_payload(tag,index));
    }
    let mut bad_enum=annotation_payload(b'e',12);bad_enum.extend_from_slice(&14u16.to_be_bytes());invalid.push(bad_enum);
    let mut bad_enum=annotation_payload(b'e',8);bad_enum.extend_from_slice(&2u16.to_be_bytes());invalid.push(bad_enum);
    invalid.push(vec![0,1,0,8,0,1,0,9,b'@',0,12,0,0]);
    invalid.push(vec![0,1,0,8,0,1,0,9,b'[',255,255]);
    let mut deep=vec![0,1,0,8,0,1,0,9];
    for _ in 0..65 {deep.extend_from_slice(&[b'[',0,1]);}
    deep.extend_from_slice(&[b'I',0,10]);invalid.push(deep);
    for (number,payload) in invalid.iter().enumerate() {
        assert!(decode("origin",&annotation_class(payload)).is_err(),"accepted mutation {number}: {payload:?}");
    }
    // Both primitive and void class literals are legal; CONSTANT_Class is never the element value.
    for index in [12,13] {assert!(decode("origin",&annotation_class(&annotation_payload(b'c',index))).is_ok());}
    // Nested arrays/annotations are structural values, including empty arrays.
    for value in [vec![b'[',0,0],vec![b'@',0,8,0,0]] {
        let mut payload=vec![0,1,0,8,0,1,0,9];payload.extend(value);
        assert!(decode("origin",&annotation_class(&payload)).is_ok());
    }
    let mut bytes=annotation_class(&valid);
    let last_attribute=bytes.len()-valid.len()-8;
    bytes[last_attribute-1]=2; // method attributes_count, leaving the trailing class attributes_count intact.
    let copy=bytes[last_attribute..bytes.len()-2].to_vec();
    bytes.splice(bytes.len()-2..bytes.len()-2,copy);
    assert!(decode("origin",&bytes).unwrap_err().to_string().contains("duplicate annotation attribute"));
}

#[test]
fn annotation_floating_nan_bits_and_signed_long_remain_exact() {
    use ktcore::classfile::AnnotationValue as V;
    for (tag,index,expected) in [
        (b'J',18,V::Long(i64::MIN)),
        (b'F',20,V::FloatBits(0x7fc12345)),
        (b'D',21,V::DoubleBits(0x7ff8000000001234)),
    ] {
        let facts=decode("origin",&annotation_class(&annotation_payload(tag,index))).unwrap();
        assert_eq!(facts.methods[0].annotations[0].annotation.elements[0].value,expected);
    }
}

#[test]
fn annotation_node_and_text_work_are_bounded_without_partial_results() {
    // A small pool can be referenced repeatedly by legal annotation payloads.
    let mut payload=vec![0,1,0,8,0,1,0,9,b'[',255,255];
    for _ in 0..65535 {payload.extend_from_slice(&[b'I',0,10]);}
    assert!(decode("origin",&annotation_class(&payload)).unwrap_err().to_string().contains("annotation node limit"));
    let mut payload=vec![0,1,0,8,0,1,0,9,b'[',3,232];
    for _ in 0..1000 {payload.extend_from_slice(&[b's',0,11]);}
    for raw in [vec![b'x';4096],[b"\xed\xa0\x80".as_slice(),&vec![b'x';4093]].concat()] {
        let mut bytes=annotation_class(&payload);
        let at=bytes.windows(b"literal".len()).position(|value|value==b"literal").unwrap();
        bytes[at-2..at].copy_from_slice(&(raw.len() as u16).to_be_bytes());
        bytes.splice(at..at+7,raw);
        assert!(decode("origin",&bytes).unwrap_err().to_string().contains("metadata text budget"));
    }
}
