use ktcore::extensions::source::{AnnotationValue, SourceDeclaration};
use ktcore::extensions::{Dispatch, Visibility};
use ktcore::knowledge::Knowledge;
use ktlsp::index::{Index, Tier};
use ktlsp::language::{self, LanguageParsers, SourceLanguage};

fn facts(file: &str, source: &str) -> Vec<SourceDeclaration> {
    facts_with_types(file, source, &[])
}

fn facts_with_types(file: &str, source: &str, types: &[(&str, &str)]) -> Vec<SourceDeclaration> {
    let mut index = Index::new();
    let mut parsers = LanguageParsers::new();
    for &(file, source) in types {
        let language = SourceLanguage::for_key(file);
        let tree = parsers.parse(language, source);
        let file_facts = language::file_facts(language, &tree, source);
        index.replace_file(file, file_facts.symbols, Tier::Volatile);
    }
    let language = SourceLanguage::for_key(file);
    let tree = parsers.parse(language, source);
    let file_facts = language::file_facts(language, &tree, source);
    index.replace_file(file, file_facts.symbols, Tier::Volatile);
    language::extension_source_facts(&index, file, &tree, source)
}

fn named<'a>(facts: &'a [SourceDeclaration], source: &str, name: &str) -> &'a SourceDeclaration {
    facts
        .iter()
        .find(|fact| &source[fact.definition.start_byte..fact.definition.end_byte] == name)
        .unwrap()
}

fn descriptor(fact: &SourceDeclaration) -> &str {
    match &fact.member.as_ref().unwrap().signature {
        Knowledge::Found(signature) => signature.descriptor(),
        other => panic!("expected a known JVM signature: {other:?}"),
    }
}

#[test]
fn java_authored_identity_flags_and_exact_jvm_signatures() {
    let source = r#"package oracle;
import java.lang.String;
public class Target {
    private static int secret;
    public int twice(int value) { return value * 2; }
    public String twice(String value) { return value; }
    public static String[] copy(int[] input, String[] names) { return names; }
}"#;
    let facts = facts("Target.java", source);
    let secret = named(&facts, source, "secret");
    assert_eq!(descriptor(secret), "I");
    let member = secret.member.as_ref().unwrap();
    assert_eq!(member.dispatch, Knowledge::Found(Dispatch::Static));
    assert_eq!(member.visibility, Knowledge::Found(Visibility::Private));
    assert_eq!(member.synthetic, Knowledge::Found(false));
    let target = named(&facts, source, "Target");
    assert_eq!(secret.owner.as_ref(), Some(&target.definition));
    let overloads: Vec<_> = facts
        .iter()
        .filter(|f| &source[f.definition.start_byte..f.definition.end_byte] == "twice")
        .map(descriptor)
        .collect();
    assert_eq!(
        overloads,
        ["(I)I", "(Ljava/lang/String;)Ljava/lang/String;"]
    );
    assert_eq!(
        descriptor(named(&facts, source, "copy")),
        "([I[Ljava/lang/String;)[Ljava/lang/String;"
    );
}

#[test]
fn kotlin_annotations_keep_class_aliases_field_target_and_exact_editable_strings() {
    let source = r#"package oracle.mixin
import oracle.Target as ActualTarget
import org.spongepowered.asm.mixin.Mixin
import org.spongepowered.asm.mixin.Shadow
@Mixin(value = [ActualTarget::class], remap = false)
abstract class CaseMixin {
    @Shadow(prefix = "shadow$") abstract fun `shadow$twice`(value: Int): Int
    @field:Shadow private lateinit var label: String
}
"#;
    let facts = facts("CaseMixin.kt", source);
    let mixin = named(&facts, source, "CaseMixin");
    let annotation = &mixin.annotations[0];
    assert_eq!(
        annotation.ty.qualification.as_deref(),
        Some("org.spongepowered.asm.mixin")
    );
    assert_eq!(annotation.ty.name, "Mixin");
    let Knowledge::Found(AnnotationValue::Array(values)) = &annotation.arguments[0].value else {
        panic!("class array")
    };
    let Knowledge::Found(AnnotationValue::Class(target)) = &values[0].value else {
        panic!("class literal")
    };
    assert_eq!(target.qualification.as_deref(), Some("oracle"));
    assert_eq!(target.name, "Target");
    assert_eq!(
        annotation.arguments[1].value,
        Knowledge::Found(AnnotationValue::Boolean(false))
    );
    let shadow = named(&facts, source, "`shadow$twice`");
    assert_eq!(shadow.owner.as_ref(), Some(&mixin.definition));
    assert_eq!(descriptor(shadow), "(I)I");
    let Knowledge::Found(AnnotationValue::String {
        value,
        content_range,
    }) = &shadow.annotations[0].arguments[0].value
    else {
        panic!("prefix string")
    };
    assert_eq!(value, "shadow$");
    assert_eq!(&source[content_range.clone()], "shadow$");
    let field = named(&facts, source, "label");
    assert_eq!(
        field.annotations[0].use_site_target.as_deref(),
        Some("field")
    );
    assert_eq!(descriptor(field), "Ljava/lang/String;");
    assert_eq!(
        field.member.as_ref().unwrap().visibility,
        Knowledge::Found(Visibility::Private)
    );
}

#[test]
fn java_annotation_arrays_preserve_selector_descriptor_and_alias_content_ranges() {
    let source = r#"package oracle.mixin;
import oracle.Target;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.Shadow;
@Mixin(value = Target.class, remap = false)
abstract class CaseMixin {
    @Shadow(aliases = {"hidden", "other"}) private int local(int value) { return 0; }
    @Select(method = {"twice(I)I", "twice(Ljava/lang/String;)Ljava/lang/String;"}) void probe() {}
}"#;
    let facts = facts("CaseMixin.java", source);
    let mixin = named(&facts, source, "CaseMixin");
    let Knowledge::Found(AnnotationValue::Class(target)) = &mixin.annotations[0].arguments[0].value
    else {
        panic!("class literal")
    };
    assert_eq!(target.qualification.as_deref(), Some("oracle"));
    let local = named(&facts, source, "local");
    let Knowledge::Found(AnnotationValue::Array(aliases)) =
        &local.annotations[0].arguments[0].value
    else {
        panic!("aliases")
    };
    assert_eq!(aliases.len(), 2);
    let Knowledge::Found(AnnotationValue::String { content_range, .. }) = &aliases[0].value else {
        panic!("alias")
    };
    assert_eq!(&source[content_range.clone()], "hidden");
    let probe = named(&facts, source, "probe");
    let Knowledge::Found(AnnotationValue::Array(selectors)) =
        &probe.annotations[0].arguments[0].value
    else {
        panic!("selectors")
    };
    let Knowledge::Found(AnnotationValue::String { content_range, .. }) = &selectors[0].value
    else {
        panic!("selector")
    };
    assert_eq!(&source[content_range.clone()], "twice(I)I");
}

#[test]
fn unsupported_or_unresolved_source_never_becomes_a_primitive_or_void_signature() {
    for (file, source, members) in [
        ("Unknown.java", "package oracle; class Unknown<T> { T generic(T value) { return value; } Missing unknown() { return null; } String unresolved(String s) { return s; } }", vec!["generic", "unknown"]),
        ("Unknown.kt", "package oracle\nclass Int\nclass Unknown<T> {\n fun wrong(value: Int): Int = value\n fun generic(value: T): T = value\n fun unknown(): Missing {}\n fun boxed(value: kotlin.Int?): kotlin.Int? = value\n val generatedGetter: kotlin.Int = 1\n}\n", vec!["wrong", "generic", "unknown", "boxed", "generatedGetter"]),
    ] {
        let facts = facts(file, source);
        for member in members {
            assert!(matches!(named(&facts, source, member).member.as_ref().unwrap().signature, Knowledge::Unknown(_)), "{file}: {member}");
        }
        if file == "Unknown.java" {
            assert_eq!(descriptor(named(&facts, source, "unresolved")), "(Ljava/lang/String;)Ljava/lang/String;");
        }
    }
}

#[test]
fn interpolation_and_escapes_remain_unknown_instead_of_issuing_false_edit_ranges() {
    for (file, source) in [
        (
            "Use.java",
            r#"class Use { @Select(method = "twice\u0028I)I") void probe() {} }"#,
        ),
        (
            "Use.kt",
            r#"class Use { @Select(method = "${name}(I)I") fun probe() {} }"#,
        ),
    ] {
        let facts = facts(file, source);
        assert!(matches!(
            named(&facts, source, "probe").annotations[0].arguments[0].value,
            Knowledge::Unknown(_)
        ));
    }
}

#[test]
fn incomplete_method_body_does_not_erase_a_known_authored_header() {
    let source =
        "package oracle; class Target { public int twice(int value) { return value + ; } }";
    let facts = facts("Target.java", source);
    assert_eq!(descriptor(named(&facts, source, "twice")), "(I)I");
}

#[test]
fn source_properties_or_extension_receivers_do_not_invent_jvm_fields_or_descriptors() {
    let source = "package oracle\nclass Owner {\n private val computed: Int get() = 42\n private val delegated: Int by lazy { 42 }\n fun String.extension(value: Int): Int = value\n internal fun mangled(value: Int): Int = value\n @JvmName(\"other\") fun renamed(value: Int): Int = value\n}\n";
    let facts = facts("Owner.kt", source);
    for name in ["computed", "delegated", "extension", "mangled", "renamed"] {
        assert!(
            matches!(
                named(&facts, source, name)
                    .member
                    .as_ref()
                    .unwrap()
                    .signature,
                Knowledge::Unknown(_)
            ),
            "{name}"
        );
    }
}

#[test]
fn java_post_name_dimensions_do_not_become_scalar_signatures() {
    let source = "package oracle; class Target { private int array[]; int parameter(int values[]) { return 1; } int returns()[] { return null; } }";
    let facts = facts("Target.java", source);
    for name in ["array", "parameter", "returns"] {
        assert!(
            matches!(
                named(&facts, source, name)
                    .member
                    .as_ref()
                    .unwrap()
                    .signature,
                Knowledge::Unknown(_)
            ),
            "{name}"
        );
    }
}

// The real JDK 17 and Kotlin 1.8.21 source reduction has these two top-level identities.
const STRING_TYPES: &[(&str, &str)] = &[
    ("KotlinString.kt", "package kotlin\nclass String\n"),
    (
        "JavaString.java",
        "package java.lang; public final class String {}",
    ),
];

#[test]
fn mixed_jdk_kotlin_string_signatures_are_known_in_both_languages() {
    for (file, source) in [
        ("Target.kt", "package oracle\nclass Target {\n fun twice(value: Int): Int = value * 2\n fun twice(value: String): String = value\n}\n"),
        ("Target.java", "package oracle; class Target { int twice(int value) { return value * 2; } String twice(String value) { return value; } }"),
    ] {
        let facts = facts_with_types(file, source, STRING_TYPES);
        let signatures: Vec<_> = facts.iter()
            .filter(|fact| &source[fact.definition.start_byte..fact.definition.end_byte] == "twice")
            .map(descriptor)
            .collect();
        assert_eq!(signatures, ["(I)I", "(Ljava/lang/String;)Ljava/lang/String;"], "{file}");
    }
}

#[test]
fn user_string_declarations_and_imports_do_not_become_jvm_builtins() {
    for (file, source) in [
        ("Target.kt", "package oracle\nclass String\nclass Target { fun probe(value: String): String = value }\n"),
        ("Target.java", "package oracle; class String {} class Target { String probe(String value) { return value; } }"),
        ("Target.kt", "package oracle\nimport other.String\nclass Target { fun probe(value: String): String = value }\n"),
        ("Target.java", "package oracle; import other.String; class Target { String probe(String value) { return value; } }"),
        ("Target.kt", "package oracle\nimport other.String as Text\nclass Target { fun probe(value: Text): Text = value }\n"),
        ("Target.kt", "package oracle\nimport other.*\nclass Target { fun probe(value: String): String = value }\n"),
        ("Target.kt", "package oracle\nclass Target { class String\n fun probe(value: String): String = value }\n"),
        ("Target.java", "package oracle; class Target { class String {} String probe(String value) { return value; } }"),
    ] {
        let mut types = STRING_TYPES.to_vec();
        types.push(("OtherString.kt", "package other\nclass String\n"));
        let facts = facts_with_types(file, source, &types);
        assert!(matches!(named(&facts, source, "probe").member.as_ref().unwrap().signature, Knowledge::Unknown(_)), "{file}: {source}");
    }
}

#[test]
fn nested_java_lang_string_is_not_the_top_level_jvm_builtin() {
    let types = &[
        ("KotlinString.kt", "package kotlin\nclass String\n"),
        (
            "JavaString.java",
            "package java.lang; public class Outer { public class String {} }",
        ),
    ];
    for (file, source) in [
        ("Target.kt", "package oracle\nimport java.lang.Outer.String\nclass Target { fun probe(value: String): String = value }\n"),
        ("Target.java", "package oracle; import java.lang.Outer.String; class Target { String probe(String value) { return value; } }"),
    ] {
        let facts = facts_with_types(file, source, types);
        assert!(matches!(named(&facts, source, "probe").member.as_ref().unwrap().signature, Knowledge::Unknown(_)), "{file}");
    }
}

#[test]
fn source_member_availability_stays_independent_of_jvm_descriptor_knowledge() {
    use ktlsp::symbol::{IndexedSymbol, MemberImplementation, SourceMemberFlags};
    assert_eq!(std::mem::size_of::<SourceMemberFlags>(), 1);
    eprintln!(
        "SOURCE_METHOD_LAYOUT flags={} symbol={}",
        std::mem::size_of::<SourceMemberFlags>(),
        std::mem::size_of::<IndexedSymbol>()
    );
    for (file, source, concrete, abstract_) in [
        ("Owner.kt", "package oracle\nabstract class Owner {\n external open fun externalOnly(): Int\n internal fun internalOnly(): Int = 1\n suspend fun suspendOnly(): Int = 2\n inline fun inlineOnly(): Int = 3\n abstract fun abstractOnly(): Int\n}\n", vec!["externalOnly", "internalOnly", "suspendOnly", "inlineOnly"], "abstractOnly"),
        ("Owner.java", "package oracle; public abstract class Owner { public native int nativeOnly(); public abstract int abstractOnly(); }", vec!["nativeOnly"], "abstractOnly"),
    ] {
        let mut parsers = LanguageParsers::new();
        let language = SourceLanguage::for_key(file);
        let tree = parsers.parse(language, source);
        let symbols = language::symbol_facts(language, &tree, source).symbols;
        for name in concrete {
            let flags = symbols.iter().find(|symbol| symbol.name == name).unwrap().source_member;
            assert_eq!(flags.implementation(), Some(MemberImplementation::Concrete), "{name}");
            assert_eq!(flags.dispatch(), Some(Dispatch::Instance), "{name}");
            if name == "internalOnly" { assert!(flags.is_internal()); assert_eq!(flags.visibility(), None); }
        }
        assert_eq!(symbols.iter().find(|symbol| symbol.name == abstract_).unwrap().source_member.implementation(), Some(MemberImplementation::Abstract));
        let extracted = facts(file, source);
        for name in ["externalOnly", "internalOnly", "suspendOnly", "inlineOnly"].into_iter().filter(|name| source.contains(name)) {
            let member = named(&extracted, source, name).member.as_ref().unwrap();
            assert!(matches!(member.signature, Knowledge::Unknown(_)), "JVM descriptor promoted: {name}");
            assert!(matches!(member.dispatch, Knowledge::Unknown(_)), "old extension knowledge promoted: {name}");
        }
    }
}

#[test]
fn source_member_headers_distinguish_abstract_default_invalid_and_unfinished_body() {
    use ktlsp::symbol::MemberImplementation;
    for (file, source, known) in [
        ("Cases.java", "package oracle; interface Cases { int abstractOnly(); default int defaultOnly() { return 1; } public int invalidDefault() { return 2; } }", vec![("abstractOnly", Some(MemberImplementation::Abstract)), ("defaultOnly", Some(MemberImplementation::Concrete)), ("invalidDefault", None)]),
        ("Cases.java", "package oracle; class Cases { default int invalidClassDefault() { return 1; } }", vec![("invalidClassDefault", None)]),
        ("Cases.kt", "package oracle\ninterface Cases {\n fun abstractOnly(): Int\n fun defaultOnly(): Int = 1\n}\nopen class Partial { open fun unfinished(): Int { return } }\n", vec![("abstractOnly", Some(MemberImplementation::Abstract)), ("defaultOnly", Some(MemberImplementation::Concrete)), ("unfinished", Some(MemberImplementation::Concrete))]),
        ("Cases.kt", "package oracle\nclass Cases {\n expect fun expected(): Int\n private abstract fun invalid(): Int\n open fun missingBody(): Int\n}\n", vec![("expected", None), ("invalid", None), ("missingBody", None)]),
    ] {
        let language = SourceLanguage::for_key(file);
        let mut parsers = LanguageParsers::new();
        let tree = parsers.parse(language, source);
        let symbols = language::symbol_facts(language, &tree, source).symbols;
        for (name, expected) in known {
            let symbol = symbols.iter().find(|symbol| symbol.name == name).unwrap_or_else(|| panic!("{file}: missing {name}"));
            assert_eq!(symbol.source_member.implementation(), expected, "{file}: {name}");
        }
    }
}

#[test]
fn java_constructor_is_not_an_ordinary_method_descriptor_with_the_owner_name() {
    use ktlsp::symbol::MemberImplementation;
    let source = "package oracle; public class Owner { public Owner(int value) {} public int Owner(int value) { return value; } }";
    let extracted = facts("Owner.java", source);
    let constructor_start = source.find("Owner(int value)").unwrap();
    let method_start = source.rfind("Owner(int value)").unwrap();
    let constructor = extracted
        .iter()
        .find(|fact| fact.definition.start_byte == constructor_start)
        .unwrap();
    let method = extracted
        .iter()
        .find(|fact| fact.definition.start_byte == method_start)
        .unwrap();
    assert!(matches!(
        constructor.member.as_ref().unwrap().signature,
        Knowledge::Unknown(_)
    ));
    assert_eq!(descriptor(method), "(I)I");
    let mut parsers = LanguageParsers::new();
    let tree = parsers.parse(SourceLanguage::Java, source);
    let symbols = language::symbol_facts(SourceLanguage::Java, &tree, source).symbols;
    assert_eq!(
        symbols
            .iter()
            .find(|symbol| symbol.start_byte == constructor_start)
            .unwrap()
            .source_member
            .implementation(),
        Some(MemberImplementation::Constructor)
    );
    assert_eq!(
        symbols
            .iter()
            .find(|symbol| symbol.start_byte == method_start)
            .unwrap()
            .source_member
            .implementation(),
        Some(MemberImplementation::Concrete)
    );
}

#[test]
fn java_array_signature_components_keep_qualification_and_generic_arguments() {
    let source = r#"package oracle;
public class Target {
    public java.lang.String[] known(int[] values) { return null; }
    public other.String[] foreign() { return null; }
    public other.Box<java.lang.String>[] generic() { return null; }
}
"#;
    let facts = facts_with_types(
        "Target.java",
        source,
        &[
            (
                "ForeignString.java",
                "package other; public class String {}",
            ),
            ("ForeignBox.java", "package other; public class Box<T> {}"),
        ],
    );
    assert_eq!(
        descriptor(named(&facts, source, "known")),
        "([I)[Ljava/lang/String;"
    );
    for name in ["foreign", "generic"] {
        assert!(
            matches!(
                named(&facts, source, name)
                    .member
                    .as_ref()
                    .unwrap()
                    .signature,
                Knowledge::Unknown(_)
            ),
            "{name} must not erase an unrelated declaration to java.lang.String"
        );
    }
}
