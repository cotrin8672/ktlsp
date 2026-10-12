use ktlsp::hints::InlayHintKind;
use ktlsp::workspace::Workspace;

const JAVA_CONTEXT: &str =
    include_str!("../../../dev/corpus/fixtures/java-inlay-hints/HintUse.java");
const KOTLIN_CONTEXT: &str = include_str!("../../../dev/corpus/fixtures/java-inlay-hints/Hints.kt");

#[test]
fn java_inlay_hints_follow_contextual_types_and_unsaved_kotlin_edits() {
    let mut ws = Workspace::new();
    let java = "mem:///HintUse.java";
    let kotlin = "mem:///Hints.kt";
    ws.open(kotlin, KOTLIN_CONTEXT.to_string());
    ws.open(java, JAVA_CONTEXT.to_string());
    ws.open(
        "mem:///UnrelatedString.java",
        "package unrelated; class String {}".to_string(),
    );
    for (step, kotlin_type) in ["Payload", "Updated", "Payload"].into_iter().enumerate() {
        let changed = KOTLIN_CONTEXT.replace(
            "supply(): Payload = Payload()",
            &format!("supply(): {kotlin_type} = {kotlin_type}()"),
        );
        if step > 0 {
            ws.change(kotlin, changed);
        }
        let expected: Vec<_> = [
            ("longNumber", "long"),
            ("floatNumber", "float"),
            ("boxed", "Box<String>"),
            ("selected", "Body"),
            ("overloaded", "Box<String>"),
            ("covariant", "Box<? extends Body>"),
            ("contravariant", "Box<? super Body>"),
            ("wildcard", "Box<?>"),
            ("alias", "Body"),
            ("generic", "Body"),
            ("fromKotlin", kotlin_type),
            ("resource", "Body"),
            ("resourceAlias", "Body"),
        ]
        .into_iter()
        .map(|(name, ty)| {
            let marker = format!("var {name} =");
            (
                JAVA_CONTEXT.find(&marker).unwrap() + 4 + name.len(),
                format!(": {ty}"),
            )
        })
        .collect();
        let actual: Vec<_> = ws
            .inlay_hints(java, 0, JAVA_CONTEXT.len())
            .into_iter()
            .map(|hint| (hint.position_byte, hint.label))
            .collect();
        assert_eq!(actual, expected, "Kotlin return {kotlin_type}");
    }
}

#[test]
fn java_inlay_hints_keep_invalid_initializers_unknown() {
    let mut ws = Workspace::new();
    let source = JAVA_CONTEXT.replace(
        "Body explicit = body;",
        "Body explicit = body;\n        var missing = missingCall();\n        var self = self;\n        var rejected = factory.make(new Other());",
    );
    let java = "mem:///HintUse.java";
    ws.open("mem:///Hints.kt", KOTLIN_CONTEXT.to_string());
    ws.open(java, source.clone());
    let start = source.find("Body explicit").unwrap();
    let hints = ws.inlay_hints(java, start, source.len());
    assert!(
        hints.is_empty(),
        "unrelated global methods, self references and rejected overloads cannot supply a type: {hints:?}"
    );
    let call = source.rfind("missingCall()").unwrap();
    assert!(ws.goto_definition(java, call).is_empty());
}

#[test]
fn java_method_hint_fallback_respects_lexical_and_static_imported_owners() {
    let factory = "package helpers; public class Factory extends Base {} class Base { public static hintcase.Payload make() { return null; } public hintcase.Updated instanceOnly() { return null; } }";
    let decoy = "package hintcase; class Decoy { Payload make() { return null; } }";
    for (imports, call, expected) in [
        (
            "import static helpers.Factory.make;",
            "make()",
            Some(": Payload"),
        ),
        (
            "import static helpers.Factory.*;",
            "make()",
            Some(": Payload"),
        ),
        ("", "make()", None),
        ("import helpers.Factory;", "make()", None),
        (
            "import static helpers.Factory.instanceOnly;",
            "instanceOnly()",
            None,
        ),
    ] {
        let mut ws = Workspace::new();
        ws.open("mem:///Hints.kt", KOTLIN_CONTEXT.to_string());
        ws.open("mem:///Factory.java", factory.to_string());
        ws.open("mem:///Decoy.java", decoy.to_string());
        let java = "mem:///Use.java";
        let source = format!(
            "package hintcase; {imports} class Use {{ void run() {{ var result = {call}; }} }}"
        );
        ws.open(java, source.clone());
        let actual = ws.inlay_hints(java, 0, source.len());
        assert_eq!(
            actual.first().map(|hint| hint.label.as_str()),
            expected,
            "{source}: {actual:?}"
        );
        let definitions = ws.goto_definition(java, source.rfind(call).unwrap());
        if expected.is_some() {
            assert_eq!(definitions.len(), 1, "{source}: {definitions:?}");
            let definition = &definitions[0];
            assert_eq!(definition.file, "mem:///Factory.java");
            assert_eq!(&factory[definition.start_byte..definition.end_byte], "make");
        } else {
            assert!(definitions.is_empty(), "{source}: {definitions:?}");
        }
    }
    let mut ws = Workspace::new();
    ws.open("mem:///Hints.kt", KOTLIN_CONTEXT.to_string());
    let source = "package hintcase; class Outer { Payload make() { return null; } class Inner { void run() { var result = make(); } } }";
    ws.open("mem:///Outer.java", source.to_string());
    let hints = ws.inlay_hints("mem:///Outer.java", 0, source.len());
    assert_eq!(
        hints.first().map(|hint| hint.label.as_str()),
        Some(": Payload"),
        "{hints:?}"
    );
}

#[test]
fn inlay_hint_ranges_include_the_endpoint_in_both_languages() {
    for (key, source, marker) in [
        (
            "mem:///Range.kt",
            "fun run() { val first = 1; val last = 2 }",
            "val last",
        ),
        (
            "mem:///Range.java",
            "class Range { void run() { var first = 1; var last = 2; } }",
            "var last",
        ),
    ] {
        let mut ws = Workspace::new();
        ws.open(key, source.to_string());
        let position = source.find(marker).unwrap() + marker.len();
        let hints = ws.inlay_hints(key, position, position);
        assert_eq!(hints.len(), 1, "{key}: {hints:?}");
        assert_eq!(hints[0].position_byte, position);
        assert!(ws.inlay_hints(key, position + 1, position).is_empty());
        assert!(ws.inlay_hints(key, source.len() + 1, usize::MAX).is_empty());
    }
}

#[test]
fn local_extension_inlay_hints_use_local_call_results_over_a_global_decoy() {
    let mut ws = Workspace::new();
    let key = "mem:///Local.kt";
    let src = "package app\nclass Versioned\nclass Payload\nfun Versioned.oldApi(): Versioned = this\nfun use(v: Versioned) {\n    fun Versioned.oldApi(): Payload = Payload()\n    val result = v.oldApi()\n    fun inferred() = v.oldApi()\n}\n";
    ws.open(key, src.to_string());
    let hints = ws.inlay_hints(key, 0, src.len());
    for (marker, length) in [
        ("val result", "val result".len()),
        ("inferred()", "inferred()".len()),
    ] {
        let position = src.find(marker).unwrap() + length;
        assert!(
            hints
                .iter()
                .any(|hint| hint.position_byte == position && hint.label == ": Payload"),
            "{marker}: {hints:?}"
        );
        assert!(
            !hints
                .iter()
                .any(|hint| hint.position_byte == position && hint.label == ": Versioned"),
            "{hints:?}"
        );
    }
}

#[test]
fn inlay_hints_include_local_types_and_expression_body_returns() {
    let mut ws = Workspace::new();
    let key = "mem:///Main.kt".to_string();
    let src = "class Box\n\
               fun make(): Box = Box()\n\
               fun inferred() = Box()\n\
               fun explicit(): Box = Box()\n\
               fun main() {\n\
               \x20\x20\x20\x20val box = make()\n\
               \x20\x20\x20\x20val n = 1\n\
               \x20\x20\x20\x20val named: Box = make()\n\
               }\n";
    ws.open(key.clone(), src.to_string());

    let hints = ws.inlay_hints(&key, 0, src.len());

    assert!(
        hints.iter().any(|hint| hint.kind == InlayHintKind::Type
            && hint.label == ": Box"
            && hint.position_byte == src.find("inferred()").unwrap() + "inferred()".len()),
        "expression-body return hint should infer Box: {hints:?}"
    );
    assert!(
        hints.iter().any(|hint| hint.label == ": Box"
            && hint.position_byte == src.find("val box").unwrap() + "val box".len()),
        "local box hint should infer Box: {hints:?}"
    );
    assert!(
        hints.iter().any(|hint| hint.label == ": Int"
            && hint.position_byte == src.find("val n").unwrap() + "val n".len()),
        "local n hint should infer Int: {hints:?}"
    );
    assert!(
        !hints.iter().any(|hint| {
            hint.position_byte == src.find("explicit()").unwrap() + "explicit()".len()
                || hint.position_byte == src.find("val named").unwrap() + "val named".len()
        }),
        "explicit return/property types should not get hints: {hints:?}"
    );
}

#[test]
fn inlay_hints_respect_requested_byte_range() {
    let mut ws = Workspace::new();
    let key = "mem:///Main.kt".to_string();
    let src = "fun main() {\n    val a = 1\n    val b = 2\n}\n";
    ws.open(key.clone(), src.to_string());
    let start = src.find("val b").unwrap();

    let hints = ws.inlay_hints(&key, start, src.len());

    assert!(
        hints.iter().all(|hint| hint.position_byte >= start),
        "hints must stay within the requested range: {hints:?}"
    );
    assert!(
        hints.iter().any(|hint| hint.label == ": Int"
            && hint.position_byte == src.find("val b").unwrap() + "val b".len()),
        "range should include b's hint: {hints:?}"
    );
    assert!(
        !hints
            .iter()
            .any(|hint| hint.position_byte == src.find("val a").unwrap() + "val a".len()),
        "range should exclude a's hint: {hints:?}"
    );
}

#[test]
fn java_inlay_hints_include_var_local_types() {
    let mut ws = Workspace::new();
    let key = "mem:///Main.java".to_string();
    let src = "package app;\n\
               class Helper {\n\
               \x20\x20\x20\x20String name() { return \"x\"; }\n\
               \x20\x20\x20\x20void run() {\n\
               \x20\x20\x20\x20\x20\x20\x20\x20var text = \"x\";\n\
               \x20\x20\x20\x20\x20\x20\x20\x20var count = 1;\n\
               \x20\x20\x20\x20\x20\x20\x20\x20var helper = new Helper();\n\
               \x20\x20\x20\x20\x20\x20\x20\x20var named = name();\n\
               \x20\x20\x20\x20\x20\x20\x20\x20String explicit = \"y\";\n\
               \x20\x20\x20\x20}\n\
               }\n";
    ws.open(key.clone(), src.to_string());

    let hints = ws.inlay_hints(&key, 0, src.len());

    assert!(
        hints.iter().any(|hint| hint.kind == InlayHintKind::Type
            && hint.label == ": String"
            && hint.position_byte == src.find("var text").unwrap() + "var text".len()),
        "text hint should infer String: {hints:?}"
    );
    assert!(
        hints.iter().any(|hint| hint.label == ": int"
            && hint.position_byte == src.find("var count").unwrap() + "var count".len()),
        "count hint should infer int: {hints:?}"
    );
    assert!(
        hints.iter().any(|hint| hint.label == ": Helper"
            && hint.position_byte == src.find("var helper").unwrap() + "var helper".len()),
        "helper hint should infer Helper: {hints:?}"
    );
    assert!(
        hints.iter().any(|hint| hint.label == ": String"
            && hint.position_byte == src.find("var named").unwrap() + "var named".len()),
        "method return hint should infer String: {hints:?}"
    );
    assert!(
        !hints
            .iter()
            .any(|hint| hint.position_byte == src.find("explicit").unwrap() + "explicit".len()),
        "explicit Java types should not get hints: {hints:?}"
    );
}
