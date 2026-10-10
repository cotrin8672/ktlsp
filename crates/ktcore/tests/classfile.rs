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
    assert_eq!(serializer.signature.as_deref(), Some("Ljava/lang/Object;Lkotlinx/serialization/internal/GeneratedSerializer<Lpluginfixture/Model;>;"));
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
        PERSON, MODEL, COMPANION, SERIALIZER, NOARG_OFF, NOARG_ON, UNICODE,
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
fn future_code_and_annotation_payloads_are_opaque() {
    for name in [
        b"FutureAttribute".as_slice(),
        b"Code",
        b"RuntimeVisibleAnnotations",
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
