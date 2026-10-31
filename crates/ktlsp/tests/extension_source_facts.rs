use ktcore::extensions::source::{AnnotationValue, SourceDeclaration};
use ktcore::extensions::{Dispatch, Visibility};
use ktcore::knowledge::Knowledge;
use ktlsp::index::{Index, InferenceIndex, Tier};
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
fn callable_parameters_preserve_authored_order_ranges_types_and_annotation_values() {
    let java=r#"package parameters; // UTF-8 😃
@interface Nest { String value(); }
@interface Local { boolean argsOnly(); int ordinal(); String[] name(); Nest at(); }
@interface Share { String value(); }
@interface Cancellable {}
class LocalRef<T> {}
class T {}
class CallbackInfo {}
class Use {
 void h(@Local(argsOnly=true,ordinal=0,name={"text"},at=@Nest("inside")) String text,
        @Share("slot") LocalRef<String> slot,@Cancellable CallbackInfo ci) {}
 void none() {}
 void receiver(Use this) {}
 void spread(String... values) {}
 void projected(LocalRef<? extends String> values) {}
 void post(String values[]) {}
 <T> void shadow(T value) {}
}"#;
    let kotlin=r#"package parameters // UTF-8 😃
annotation class Nest(val value: String)
annotation class Local(val argsOnly: Boolean, val ordinal: Int, val name: Array<String>, val at: Nest)
annotation class Share(val value: String)
annotation class Cancellable
class LocalRef<T>
class T
class CallbackInfo
class Use {
 fun h(@Local(argsOnly=true,ordinal=0,name=["text"],at=Nest("inside")) text: String,
       @Share("slot") slot: LocalRef<String>,@Cancellable ci: CallbackInfo) {}
 fun none() {}
 fun escaped(`odd name`: Int) {}
 fun defaulted(value: Int = 1) {}
 suspend fun suspended(value: Int) {}
 context(ctx: String) fun contextual(value: Int) {}
 fun String.receiver(value: Int) {}
 fun spread(vararg values: String) {}
 fun projected(values: LocalRef<out String>) {}
 fun <T> shadow(value: T) {}
}
"#;
    for (file,text,unsupported) in [
        ("Parameters.java",java,&["receiver","spread","projected","post","shadow"][..]),
        ("Parameters.kt",kotlin,&["defaulted","suspended","contextual","receiver","spread","projected","shadow"][..]),
    ] {
        let declarations=facts(file,text);
        let Knowledge::Found(parameters)=&named(&declarations,text,"h").parameters else {panic!("{file}: parameters");};
        assert_eq!(parameters.iter().map(|parameter|parameter.name.as_str()).collect::<Vec<_>>(),["text","slot","ci"]);
        for parameter in parameters {
            assert_eq!(&text[parameter.name_range.clone()],parameter.name);
            assert!(parameter.range.start<=parameter.name_range.start && parameter.name_range.end<=parameter.range.end);
            assert!(text[parameter.range.clone()].starts_with('@'));
            assert_eq!(parameter.annotations.len(),1);
            assert!(parameter.annotations[0].range.start>=parameter.range.start);
            assert!(parameter.annotations[0].range.end<=parameter.name_range.start);
        }
        assert_eq!(parameters[1].ty.name,"LocalRef");
        assert_eq!(parameters[1].ty.args.len(),1);
        assert_eq!(parameters[1].ty.args[0].name,"String");
        let local=&parameters[0].annotations[0];
        assert_eq!(local.ty.name,"Local");
        assert_eq!(local.arguments[0].value,Knowledge::Found(AnnotationValue::Boolean(true)));
        assert!(matches!(&local.arguments[1].value,Knowledge::Found(AnnotationValue::Integer {value:0,..})));
        let Knowledge::Found(AnnotationValue::Array(names))=&local.arguments[2].value else {panic!("parameter names");};
        assert!(matches!(&names[0].value,Knowledge::Found(AnnotationValue::String {value,..}) if value=="text"));
        let Knowledge::Found(AnnotationValue::Annotation(nested))=&local.arguments[3].value else {panic!("nested parameter annotation");};
        assert_eq!(nested.ty.name,"Nest");
        assert!(matches!(&nested.arguments[0].value,Knowledge::Found(AnnotationValue::String {value,..}) if value=="inside"));
        assert_eq!(named(&declarations,text,"none").parameters,Knowledge::Found(Vec::new()));
        if file.ends_with(".kt") {
            let Knowledge::Found(parameters)=&named(&declarations,text,"escaped").parameters else {panic!("backticked parameter");};
            assert_eq!(parameters[0].name,"odd name");
            assert_eq!(&text[parameters[0].name_range.clone()],"`odd name`");
        }
        assert!(matches!(named(&declarations,text,"Use").parameters,Knowledge::Unknown(_)));
        for name in unsupported {assert!(matches!(named(&declarations,text,name).parameters,Knowledge::Unknown(_)),"{file}: {name}");}
    }
}

#[test]
fn callable_type_parameters_never_resolve_as_the_shadowed_import() {
    for (file,text) in [
        ("Shadow.java","package parameters; import unrelated.T; class Use { <T> void h(T value) {} } class Box<T> { void owner(T value) {} void qualified(unrelated.T value) {} }"),
        ("Shadow.kt","package parameters\nimport unrelated.T\nclass Use { fun <T> h(value: T) {} }\nclass Box<T> { fun owner(value: T) {}\n fun qualified(value: unrelated.T) {} }"),
    ] {
        let declarations=facts_with_types(file,text,&[("T.java","package unrelated; public class T {}")]);
        for name in ["h","owner"] {assert!(matches!(named(&declarations,text,name).parameters,Knowledge::Unknown(_)),"{file}: {name}");}
        let Knowledge::Found(parameters)=&named(&declarations,text,"qualified").parameters else {panic!("{file}: qualified nominal type");};
        assert_eq!(parameters.len(),1);
        assert_eq!(parameters[0].ty.name,"T");assert_eq!(parameters[0].ty.qualification.as_deref(),Some("unrelated"));
    }
}

#[test]
fn incomplete_parameter_headers_never_claim_known_empty_parameters() {
    for (file,text) in [("Broken.java","class Use { void broken(String value, ) {} }"),
        ("Broken.kt","class Use { fun broken(value: ) {} }")] {
        let mut index=Index::new();let mut parsers=LanguageParsers::new();
        let language=SourceLanguage::for_key(file);let tree=parsers.parse(language,text);
        let symbols=language::file_facts(language,&tree,text).symbols;
        let indexed=symbols.iter().any(|symbol|symbol.name=="broken");
        index.replace_file(file,symbols,Tier::Volatile);
        let declarations=language::extension_source_facts(&index,file,&tree,text);
        match declarations.iter().find(|declaration|&text[declaration.name_range.clone()]=="broken") {
            Some(declaration)=>assert!(matches!(declaration.parameters,Knowledge::Unknown(_)),"{file}: incomplete parameters must be unknown"),
            None=>assert!(!indexed || tree.root_node().has_error(),"{file}: only an unindexed or unsupported parse may omit the incomplete callable"),
        }
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
fn interpolation_and_unsupported_escapes_remain_unknown_instead_of_issuing_false_edit_ranges() {
    for (file, source) in [
        (
            "Use.java",
            r#"class Use { @Select(method = "twice\q(I)I") void probe() {} }"#,
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
    let source = "package oracle\nclass Owner {\n private val computed: Int get() = 42\n private val delegated: Int by lazy { 42 }\n @Marker(\"receiver\") fun String.extension(value: Int): Int = value\n internal fun mangled(value: Int): Int = value\n @JvmName(\"other\") fun renamed(value: Int): Int = value\n}\n";
    let facts = facts("Owner.kt", source);
    let extension = named(&facts, source, "extension");
    assert_eq!(
        extension.owner.as_ref(),
        Some(&named(&facts, source, "Owner").definition)
    );
    assert!(
        extension.member.is_none(),
        "extension receivers are not ordinary class members"
    );
    assert!(extension.jvm_property.is_none());
    assert_eq!(extension.annotations.len(), 1);
    let annotation = &extension.annotations[0];
    assert_eq!(annotation.ty.name, "Marker");
    let Knowledge::Found(AnnotationValue::String {
        value,
        content_range,
    }) = &annotation.arguments[0].value
    else {
        panic!("extension annotation must retain its authored argument")
    };
    assert_eq!(value, "receiver");
    assert_eq!(&source[content_range.clone()], "receiver");
    for name in ["computed", "delegated", "mangled", "renamed"] {
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
fn java_constructor_visibility_reuses_authored_access_without_implicit_defaults() {
    use ktlsp::symbol::{MemberImplementation, SymbolKind};
    for (modifier, visibility) in [("public", Visibility::Public),
        ("protected", Visibility::Protected), ("private", Visibility::Private),
        ("", Visibility::Package)] {
        let source = format!("package oracle; class Owner {{ {modifier} Owner() {{}} }}");
        let mut parsers = LanguageParsers::new();
        let tree = parsers.parse(SourceLanguage::Java, &source);
        let symbols = language::symbol_facts(SourceLanguage::Java, &tree, &source).symbols;
        let flags = symbols.iter().find(|symbol| symbol.name == "Owner"
            && symbol.kind == SymbolKind::Function).unwrap().source_member;
        assert_eq!(flags.implementation(), Some(MemberImplementation::Constructor));
        assert_eq!(flags.dispatch(), Some(Dispatch::Instance));
        assert_eq!(flags.visibility(), Some(visibility));
        assert_eq!(flags.protected_package_access(), visibility == Visibility::Protected);
    }
    let mut parsers = LanguageParsers::new();
    let source = "package oracle; class Owner {}";
    let tree = parsers.parse(SourceLanguage::Java, source);
    assert!(!language::symbol_facts(SourceLanguage::Java, &tree, source).symbols.iter()
        .any(|symbol| symbol.name == "Owner" && symbol.kind == SymbolKind::Function));
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

#[test]
fn source_jvm_property_access_distinguishes_getter_only_and_real_field() {
    use ktcore::extensions::source::JvmPropertyAccess;
    for (declaration, expected) in [
        ("public val field: Int = 1", JvmPropertyAccess::GetterOnly),
        ("public val field = 1", JvmPropertyAccess::GetterOnly),
        ("protected val field = 1", JvmPropertyAccess::GetterOnly),
        ("public val field: Int", JvmPropertyAccess::GetterOnly),
        (
            "protected val field: Int = 1",
            JvmPropertyAccess::GetterOnly,
        ),
        (
            "@kotlin.jvm.JvmField public val field: Int = 1",
            JvmPropertyAccess::Field,
        ),
        (
            "@kotlin.jvm.JvmField protected val field: Int = 1",
            JvmPropertyAccess::Field,
        ),
    ] {
        let text = format!("package oracle\nopen class Parent {{ {declaration} }}\n");
        let declarations = facts("Parent.kt", &text);
        let property = named(&declarations, &text, "field");
        assert_eq!(
            property.jvm_property,
            Some(Knowledge::Found(expected)),
            "{declaration}"
        );
        let member = property.member.as_ref().unwrap();
        if expected == JvmPropertyAccess::Field {
            assert_eq!(descriptor(property), "I");
            assert_eq!(member.dispatch, Knowledge::Found(Dispatch::Instance));
        } else {
            assert!(
                matches!(member.signature, Knowledge::Unknown(_)),
                "a generated getter is not a JVM field signature: {member:?}"
            );
        }
    }
}

#[test]
fn source_jvm_property_access_withholds_unsupported_transforms_and_shapes() {
    for text in [
        "package oracle\nclass Parent { @other.JvmField val field: Int = 1 }",
        "package oracle\nclass Parent { @JvmField val field: Int = 1 }",
        "package oracle\nclass Parent { @kotlin.jvm.JvmField(1) val field: Int = 1 }",
        "package oracle\nclass Parent { @field:kotlin.jvm.JvmField val field: Int = 1 }",
        "package oracle\nclass Parent { val field: Int get() = 1 }",
        "package oracle\nclass Parent { var field: Int = 1 }",
        "package oracle\nclass Parent { @kotlin.jvm.JvmField private val field: Int = 1 }",
        "package oracle\nopen class Parent { open val field: Int = 1 }",
        "package oracle\nclass Parent<T> { val field: Int = 1 }",
        "package oracle\n@other.Transform class Parent { val field: Int = 1 }",
        "@file:other.Transform\npackage oracle\nclass Parent { val field: Int = 1 }",
        "package oracle\nobject Parent { val field: Int = 1 }",
    ] {
        let declarations = facts("Parent.kt", text);
        let property = named(&declarations, text, "field");
        assert!(
            matches!(property.jvm_property, Some(Knowledge::Unknown(_))),
            "unsupported lowering must stay unknown: {text}, {property:?}"
        );
    }
    let private = "package oracle\nclass Parent { private val field = 1 }";
    let declarations = facts("Parent.kt", private);
    assert_eq!(
        named(&declarations, private, "field").jvm_property,
        Some(Knowledge::Found(
            ktcore::extensions::source::JvmPropertyAccess::GetterOnly
        ))
    );
    let text = "package oracle\nclass Parent { @kotlin.jvm.JvmField val field: Int = 1 }";
    let declarations = facts_with_types(
        "Parent.kt",
        text,
        &[("Bad.kt", "package kotlin.jvm\nannotation class JvmField")],
    );
    assert!(
        matches!(
            named(&declarations, text, "field").jvm_property,
            Some(Knowledge::Unknown(_))
        ),
        "an authored replacement cannot certify the intrinsic"
    );
}

#[test]
fn source_jvm_property_value_namespace_ignores_executable_locals_and_contextual_identifiers() {
    let text = r#"package oracle
open class Parent {
    private val state = 0
    public val key = 1
    fun data(): Int { val data = 2; val by = data + 1; return by }
    init { val by = 3; val data = by + 1 }
    fun use(callback: () -> Int = { val data = 4; data }): Int = callback()
}
"#;
    let mut index = Index::new();
    let mut parsers = LanguageParsers::new();
    let tree = parsers.parse(SourceLanguage::Kotlin, text);
    assert!(
        !tree.root_node().has_error(),
        "valid ordinary executable bodies: {}",
        tree.root_node().to_sexp()
    );
    let file = "Parent.kt";
    let symbols = language::file_facts(SourceLanguage::Kotlin, &tree, text).symbols;
    index.replace_file(file, symbols, Tier::Volatile);
    let scope = ktcore::indexer::TypeScope::with_default_imports(
        "oracle",
        Vec::new(),
        ktcore::defaults::DEFAULT_IMPORT_PACKAGES,
    );
    let facts =
        ktcore::extensions::source::kotlin_value_namespace(&index, file, &tree, text, &scope)
            .unwrap();
    let owner = index.lookup_type("Parent").into_iter().next().unwrap();
    assert!(
        facts.owners.contains(&ktcore::symbol::Def {
            file: file.into(),
            start_byte: owner.sym.start_byte,
            end_byte: owner.sym.end_byte
        }),
        "locals and contextual identifiers cannot hide an ordinary owner namespace: {facts:?}"
    );
    assert_eq!(facts.properties.values().filter(|fact| matches!(fact, Knowledge::Found(_))).count(), 2, "private backing state and inferred public property both have value facts; locals are not fields");
    let transformed = text.replace("fun data()", "@other.Transform fun data()");
    let tree = parsers.parse(SourceLanguage::Kotlin, &transformed);
    index.replace_file(
        file,
        language::file_facts(SourceLanguage::Kotlin, &tree, &transformed).symbols,
        Tier::Volatile,
    );
    let facts = ktcore::extensions::source::kotlin_value_namespace(
        &index,
        file,
        &tree,
        &transformed,
        &scope,
    )
    .unwrap();
    assert!(
        facts.owners.is_empty(),
        "skipping bodies must retain declaration transform guards"
    );
}

#[test]
fn source_jvm_property_qualified_intrinsic_withholds_a_shadowed_package_prefix() {
    let names = "package consumer\nclass kotlin { class jvm { annotation class JvmField } }\n";
    let text =
        "package consumer\nopen class Parent { @kotlin.jvm.JvmField public val field: Int = 1 }\n";
    let declarations = facts_with_types("Parent.kt", text, &[("Names.kt", names)]);
    assert!(
        matches!(
            named(&declarations, text, "field").jvm_property,
            Some(Knowledge::Unknown(_))
        ),
        "the normalized type path is not proof that its first qualifier denotes a package"
    );
    // Explicit aliases already expand through TypeScope; keep that path conservative too.
    let alias = "package consumer\nimport foreign.Type as kotlin\nclass Parent { @kotlin.jvm.JvmField public val field: Int = 1 }\n";
    assert!(matches!(
        named(&facts("Parent.kt", alias), alias, "field").jvm_property,
        Some(Knowledge::Unknown(_))
    ));
}

#[test]
fn source_v2_keeps_whole_field_declarations_and_applied_explicit_modifiers() {
    let source="class Owner { @Mark private static final int first=1, second=2; public final void run() {} }";
    let declarations=facts("Owner.java",source);
    let first=named(&declarations,source,"first");let second=named(&declarations,source,"second");
    assert_eq!(first.range,second.range);
    assert_eq!(&source[first.range.clone()],"@Mark private static final int first=1, second=2;");
    assert_eq!(first.header_range,first.range);assert!(first.body_range.is_none());
    for declaration in [first,second] {
        assert_eq!(declaration.modifiers.visibility,Knowledge::Found(Visibility::Private));
        assert_eq!(declaration.modifiers.is_static,Knowledge::Found(true));
        assert_eq!(declaration.modifiers.is_final,Knowledge::Found(true));
        assert_eq!(declaration.name_range,declaration.definition.start_byte..declaration.definition.end_byte);
    }
    let run=named(&declarations,source,"run");
    assert_eq!(&source[run.header_range.clone()],"public final void run() ");
    assert_eq!(&source[run.body_range.clone().unwrap()],"{}");
    let source="class Owner { final fun explicit() {}\n fun implicit() {} }";
    let declarations=facts("Owner.kt",source);
    assert_eq!(named(&declarations,source,"explicit").modifiers.is_final,Knowledge::Found(true));
    assert!(matches!(named(&declarations,source,"implicit").modifiers.is_final,Knowledge::Unknown(_)));
}

#[test]
fn source_v2_nested_annotations_preserve_exact_names_ranges_and_typed_ordinal() {
    use ktcore::extensions::source::AnnotationIntegerType;
    for (file,source,types) in [
        ("Use.java",r#"package oracle; class Use { @Example(method="tick()V", at=@At(value="INVOKE", target="Lpkg/C;m()V", ordinal=1)) void probe() {} }"#,
            vec![("At.java","package oracle; public @interface At { String value(); String target(); int ordinal(); }")]),
        ("Use.kt",r#"package oracle
class Use { @Example(method="tick()V", at=At(value="INVOKE", target="Lpkg/C;m()V", ordinal=1)) fun probe() {} }"#,
            vec![("At.kt","package oracle\nannotation class At(val value: String, val target: String, val ordinal: Int)")]),
    ] {
        let declarations=facts_with_types(file,source,&types);
        let annotation=&named(&declarations,source,"probe").annotations[0];
        assert_eq!(&source[annotation.name_range.clone()],"Example");
        let argument=annotation.arguments.iter().find(|arg|arg.name.as_deref()==Some("at")).unwrap();
        let Knowledge::Found(AnnotationValue::Annotation(nested))=&argument.value else {panic!("{file}: {argument:?}");};
        assert_eq!(nested.ty.name,"At");
        assert_eq!(&source[nested.name_range.clone()],"At");
        assert_eq!(nested.range,argument.range);
        assert_eq!(nested.arguments.len(),3);
        assert_eq!(nested.arguments[2].value,Knowledge::Found(AnnotationValue::Integer {value:1,ty:AnnotationIntegerType::Int}));
        assert!(matches!(&nested.arguments[1].value,Knowledge::Found(AnnotationValue::String {value,..}) if value=="Lpkg/C;m()V"));
    }
    let source="package oracle\nclass Use { @Example(at=At(1)) fun probe() {} }";
    for types in [vec![],vec![("At.kt","package oracle\nclass At(val ordinal: Int)")]] {
        let declarations=facts_with_types("Use.kt",source,&types);
        assert!(matches!(named(&declarations,source,"probe").annotations[0].arguments[0].value,Knowledge::Unknown(_)));
    }
}

#[test]
fn source_v2_enum_entries_require_one_resolved_enum_and_entry() {
    for (file,source,types) in [
        ("Use.java","package oracle; class Use { @Select(mode=Mode.INVOKE) void probe() {} }",
            vec![("Mode.java","package oracle; enum Mode { INVOKE, HEAD }")]),
        ("Use.kt","package oracle\nclass Use { @Select(mode=Mode.INVOKE) fun probe() {} }",
            vec![("Mode.kt","package oracle\nenum class Mode { INVOKE, HEAD }")]),
    ] {
        let declarations=facts_with_types(file,source,&types);
        assert!(matches!(&named(&declarations,source,"probe").annotations[0].arguments[0].value,
            Knowledge::Found(AnnotationValue::Enum {ty,name}) if ty.name=="Mode" && name=="INVOKE"));
        let declarations=facts(file,source);
        assert!(matches!(named(&declarations,source,"probe").annotations[0].arguments[0].value,Knowledge::Unknown(_)));
        let absent=source.replace("Mode.INVOKE","Mode.MISSING");
        let declarations=facts_with_types(file,&absent,&types);
        assert!(matches!(named(&declarations,&absent,"probe").annotations[0].arguments[0].value,Knowledge::Unknown(_)));
    }
    let source="package consumer\nimport one.*\nimport two.*\nclass Use { @Select(Mode.INVOKE) fun probe() {} }";
    let declarations=facts_with_types("Use.kt",source,&[("One.kt","package one\nenum class Mode { INVOKE }"),
        ("Two.kt","package two\nenum class Mode { INVOKE }")]);
    assert!(matches!(named(&declarations,source,"probe").annotations[0].arguments[0].value,Knowledge::Unknown(_)));
    // Selected binary enum stubs re-enter the shared index through this same nested Java shape.
    for (file,method) in [("Use.java","void probe() {}"),("Use.kt","fun probe() {}") ] {
        let source=format!("package oracle{}class Use {{ @Select(At.Shift.AFTER) {method} }}",if file.ends_with("java") {"; "} else {"\n"});
        let declarations=facts_with_types(file,&source,&[("At.java","package oracle; public class At { public enum Shift { BEFORE, AFTER } }")]);
        assert!(matches!(&named(&declarations,&source,"probe").annotations[0].arguments[0].value,
            Knowledge::Found(AnnotationValue::Enum {ty,name}) if ty.name=="Shift" && name=="AFTER"));
    }
}

#[test]
fn source_v2_nested_annotation_arrays_recurse_without_constant_interpretation() {
    for (file,source,types) in [
        ("Use.java",r#"class Use { @Example(at={@At(value="INVOKE",ordinal=1)}) void probe() {} }"#,
            vec![("At.java","public @interface At { String value(); int ordinal(); }")]),
        ("Use.kt",r#"class Use { @Example(at=[At(value="INVOKE",ordinal=1)]) fun probe() {} }"#,
            vec![("At.kt","annotation class At(val value: String, val ordinal: Int)")]),
    ] {
        let declarations=facts_with_types(file,source,&types);
        let argument=&named(&declarations,source,"probe").annotations[0].arguments[0];
        let Knowledge::Found(AnnotationValue::Array(values))=&argument.value else {panic!("{file}: {argument:?}");};
        assert_eq!(values.len(),1);
        assert!(matches!(values[0].value,Knowledge::Found(AnnotationValue::Annotation(_))));
    }
}

#[test]
fn source_v2_decodes_escapes_and_folds_only_literal_strings_with_scalar_maps() {
    for (file,source) in [
        ("Use.java",r#"class Use { @Select(method="ti\u0063k" + "(\uD83D\uDE00)V") void probe() {} }"#),
        ("Use.kt",r#"class Use { @Select(method="ti\u0063k" + "(\uD83D\uDE00)V") fun probe() {} }"#),
    ] {
        let declarations=facts(file,source);
        let argument=&named(&declarations,source,"probe").annotations[0].arguments[0];
        let Knowledge::Found(AnnotationValue::MappedString {value,segments})=&argument.value else {panic!("{file}: {argument:?}");};
        assert_eq!(value,"tick(😀)V");
        let mut cursor=0;
        for (scalar,segment) in value.chars().zip(segments) {
            assert_eq!(segment.decoded,cursor..cursor+scalar.len_utf16());cursor+=scalar.len_utf16();
            assert!(argument.range.start<segment.source.start && segment.source.end<argument.range.end);
        }
        assert_eq!(segments.len(),value.chars().count());
        assert_eq!(&source[segments[2].source.clone()],r"\u0063");
        assert_eq!(segments[5].decoded,5..7);
        assert_eq!(&source[segments[5].source.clone()],r"\uD83D\uDE00");
        assert_eq!(cursor,value.encode_utf16().count());
    }
    for (file,source) in [
        ("Use.java",r#"class Use { @Select("tick" + NAME) void probe() {} }"#),
        ("Use.kt",r#"class Use { @Select("tick" + name) fun probe() {} }"#),
        ("Use.java",r#"class Use { @Select("\u005cn") void probe() {} }"#),
        ("Use.kt",r#"class Use { @Select("\uD800") fun probe() {} }"#),
    ] {
        let declarations=facts(file,source);
        assert!(matches!(named(&declarations,source,"probe").annotations[0].arguments[0].value,Knowledge::Unknown(_)),"{source}");
    }
}

#[test]
fn source_v2_integer_widths_and_unsupported_values_are_honest() {
    use ktcore::extensions::source::AnnotationIntegerType::{Int,Long};
    for (literal,expected) in [("1",Some((1,Int))),("1L",Some((1,Long))),("-1",Some((-1,Int))),
        ("2147483647",Some((2147483647,Int))),("9223372036854775808L",None),("1+2",None),("1.5",None)] {
        for (file,method) in [("Use.java","void probe() {}"),("Use.kt","fun probe() {}") ] {
            let source=format!("class Use {{ @Select({literal}) {method} }}");
            let declarations=facts(file,&source);
            let actual=&named(&declarations,&source,"probe").annotations[0].arguments[0].value;
            if let Some((value,ty))=expected {assert_eq!(actual,&Knowledge::Found(AnnotationValue::Integer {value,ty}),"{file}: {literal}");}
            else {assert!(matches!(actual,Knowledge::Unknown(_)),"{file}: {literal}: {actual:?}");}
        }
    }
}

#[test]
fn source_v2_bounds_literal_concatenation_depth_and_decoded_scalar_work() {
    let deep=std::iter::repeat_n("\"x\"",200).collect::<Vec<_>>().join(" + ");
    let large=format!("\"{}\" + \"x\"","x".repeat(16_384));
    for expression in [deep,large] {
        let source=format!("class Use {{ @Select({expression}) void probe() {{}} }}");
        let declarations=facts("Use.java",&source);
        assert!(matches!(named(&declarations,&source,"probe").annotations[0].arguments[0].value,Knowledge::Unknown(_)));
    }
}

#[test]
fn source_v2_normalization_uses_the_authored_files_visible_enum_origins() {
    use ktcore::project_model::{FileOrigin,ProjectScope,ScopeVisibility,SourceRepresentation};
    let file="/consumer/Use.kt";
    let source="package consumer\nimport one.*\nimport two.*\nclass Use { @Select(Mode.INVOKE) fun probe() {} }";
    let mut index=Index::new();let mut parsers=LanguageParsers::new();
    let scope=ProjectScope {module:"consumer".into(),source_set:"main".into()};
    for (key,text,origin) in [(file,source,"consumer"),("/one/Mode.kt","package one\nenum class Mode { INVOKE }","one"),
        ("/two/Mode.kt","package two\nenum class Mode { INVOKE }","two")] {
        let tree=parsers.parse(SourceLanguage::Kotlin,text);
        index.replace_file(key,language::file_facts(SourceLanguage::Kotlin,&tree,text).symbols,Tier::Volatile);
        index.set_file_origin(key,FileOrigin {origin_id:origin.into(),representation:SourceRepresentation::Source,
            project_scope:Some(if key==file {scope.clone()} else {ProjectScope {module:origin.into(),source_set:"main".into()}})});
    }
    index.replace_scope_visibilities(vec![ScopeVisibility {scope:scope.clone(),ordered_origins:vec!["consumer".into(),"one".into()]}]);
    let tree=parsers.parse(SourceLanguage::Kotlin,source);
    let declarations=language::extension_source_facts(&index,file,&tree,source);
    assert!(matches!(named(&declarations,source,"probe").annotations[0].arguments[0].value,
        Knowledge::Found(AnnotationValue::Enum {..})),"an unrelated source-set enum cannot introduce ambiguity");
    index.replace_scope_visibilities(vec![ScopeVisibility {scope,ordered_origins:vec!["consumer".into(),"one".into(),"two".into()]}]);
    let declarations=language::extension_source_facts(&index,file,&tree,source);
    assert!(matches!(named(&declarations,source,"probe").annotations[0].arguments[0].value,Knowledge::Unknown(_)),
        "two visible wildcard candidates remain genuinely ambiguous");
}
