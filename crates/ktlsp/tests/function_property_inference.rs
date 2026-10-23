use ktlsp::parser::KotlinParser;
use ktlsp::salsa_support::{ExprKey, SalsaState};
use ktlsp::types::Type;
use ktlsp::workspace::Workspace;

const FILE: &str = "/functions/Callbacks.kt";
const SOURCE: &str = r#"package callbacks
class Callbacks {
    val callback = { value: String -> value.length }
    val explicit: (String) -> Int = { value -> value.length }
}
fun use(callbacks: Callbacks) {
    val local = { value: String -> value.length }
    local("text")
    local.invoke("text")
    callbacks.callback("text")
    callbacks.callback.invoke("text")
    callbacks.explicit.invoke("text")
    callbacks.callback(1).resultMarker()
    callbacks.callback.invoke(1)
    local.invoke()
}
"#;

#[test]
fn dangling_same_file_function_property_completion_tracks_unsaved_signature() {
    let mut ws = workspace();
    ws.open(
        "/functions/Function1.java",
        "package kotlin.jvm.functions; public interface Function1<P, R> { R invoke(P value); }"
            .into(),
    );
    let source = "package callbacks\nclass Callbacks { val callback = { value: String -> value.length } }\nfun use(callbacks: Callbacks) { callbacks.callback. }\n";
    assert!(KotlinParser::new().parse(source).root_node().has_error());
    ws.change(FILE, source.into());
    let offset = source.find("callbacks.callback.").unwrap() + "callbacks.callback.".len();
    let completion = ws
        .complete(FILE, offset, true)
        .expect("dangling inferred function value");
    assert!(completion.items.iter().any(|item| item.label == "invoke"));
    let source = source.replace("callbacks.callback. }", "callbacks.callback(1). }");
    ws.change(FILE, source.clone());
    let offset = source.find("callbacks.callback(1).").unwrap() + "callbacks.callback(1).".len();
    assert!(ws.complete(FILE, offset, true).is_none());
    let updated = source.replace("value: String -> value.length", "value: Int -> value");
    let offset = updated.find("callbacks.callback(1).").unwrap() + "callbacks.callback(1).".len();
    ws.change(FILE, updated);
    let completion = ws
        .complete(FILE, offset, true)
        .expect("updated dangling function result");
    assert!(completion
        .items
        .iter()
        .any(|item| item.label == "resultMarker"));
}

fn workspace() -> Workspace {
    let mut ws = Workspace::new();
    ws.open("/functions/Builtins.kt", "package kotlin\nclass String { val length: Int = 0 }\nclass Int { fun resultMarker() {} }\n".into());
    ws.open(FILE, SOURCE.into());
    ws.assume_index_complete_for_tests();
    ws
}

fn expression_type(ws: &Workspace, expression: &str) -> Type {
    let source = ws.doc_text(FILE).unwrap();
    let tree = KotlinParser::new().parse(&source);
    let start = source.find(expression).unwrap();
    let node = tree
        .root_node()
        .descendant_for_byte_range(start, start + expression.len())
        .unwrap();
    assert_eq!(&source[node.byte_range()], expression);
    SalsaState::new().expr_type(
        FILE,
        &source,
        salsa::Durability::LOW,
        &ws.index,
        1,
        ExprKey::from_node(node),
    )
}

#[test]
fn inferred_function_properties_preserve_signature_and_validate_invocations() {
    let ws = workspace();
    let callback = expression_type(&ws, "{ value: String -> value.length }");
    assert_eq!(callback.name(), Some("Function1"));
    assert_eq!(callback.package(), Some("kotlin"));
    assert_eq!(
        callback.args().iter().map(Type::name).collect::<Vec<_>>(),
        [Some("String"), Some("Int")]
    );
    for expression in [
        "local(\"text\")",
        "local.invoke(\"text\")",
        "callbacks.callback(\"text\")",
        "callbacks.callback.invoke(\"text\")",
        "callbacks.explicit.invoke(\"text\")",
    ] {
        assert_eq!(
            expression_type(&ws, expression).name(),
            Some("Int"),
            "{expression}"
        );
    }
    for expression in [
        "callbacks.callback(1)",
        "callbacks.callback.invoke(1)",
        "local.invoke()",
    ] {
        assert_eq!(
            expression_type(&ws, expression),
            Type::Unknown,
            "{expression}"
        );
    }
}

#[test]
fn function_property_unsaved_signature_edits_and_foreign_function_names_do_not_keep_results() {
    let mut ws = workspace();
    let prefix = "callbacks.callback(1).";
    let offset = SOURCE.find(prefix).unwrap() + prefix.len();
    assert!(ws.complete(FILE, offset, true).is_none());
    ws.change(
        FILE,
        SOURCE.replace("value: String -> value.length", "value: Int -> value"),
    );
    let source = ws.doc_text(FILE).unwrap();
    let offset = source.find(prefix).unwrap() + prefix.len();
    let completion = ws.complete(FILE, offset, true).unwrap();
    assert!(completion
        .items
        .iter()
        .any(|item| item.label == "resultMarker"));
    assert_eq!(
        expression_type(&ws, "callbacks.callback(1)").name(),
        Some("Int")
    );
    assert_eq!(
        expression_type(&ws, "callbacks.callback(\"text\")"),
        Type::Unknown
    );
    ws.change(FILE, "package callbacks\nclass Function1<P, R>\nfun use(value: Function1<String, Int>) { value.invoke(\"text\") }\n".into());
    assert_eq!(
        expression_type(&ws, "value.invoke(\"text\")"),
        Type::Unknown
    );
}

#[test]
fn function_property_calls_keep_applicable_method_priority() {
    let mut ws = workspace();
    ws.change(FILE, "package callbacks\nclass Both {\n val callback = { value: String -> value.length }\n fun callback(value: String): String = value\n fun use() { callback(\"text\") }\n}\nfun use(both: Both) { both.callback(\"text\") }\n".into());
    for expression in ["callback(\"text\")", "both.callback(\"text\")"] {
        assert_eq!(
            expression_type(&ws, expression).name(),
            Some("String"),
            "{expression}"
        );
    }
}

#[test]
fn kotlin_function_members_use_the_indexed_runtime_interface_for_completion_and_navigation() {
    let mut ws = workspace();
    let runtime = "/functions/Function1.java";
    let declaration =
        "package kotlin.jvm.functions; public interface Function1<P, R> { R invoke(P value); }";
    ws.open(runtime, declaration.into());
    for prefix in ["callbacks.explicit.", "callbacks.callback."] {
        let source = ws.doc_text(FILE).unwrap();
        let offset = source.find(prefix).unwrap() + prefix.len();
        let labels: Vec<_> = ws
            .complete(FILE, offset, true)
            .unwrap_or_else(|| panic!("no function member completion for {prefix}"))
            .items
            .into_iter()
            .map(|item| item.label)
            .collect();
        assert!(
            labels.iter().any(|label| label == "invoke"),
            "{prefix}: {labels:?}"
        );
        let targets = ws.goto_definition(FILE, offset);
        assert!(
            targets.iter().any(|target| target.file == runtime
                && target.start_byte == declaration.find("invoke").unwrap()),
            "{prefix}: {targets:?}"
        );
    }
}

#[test]
fn java_function_getters_expose_boxed_invoke_results_and_reject_wrong_calls() {
    let mut ws = workspace();
    ws.open(
        "/functions/Function1.java",
        "package kotlin.jvm.functions; public interface Function1<P, R> { R invoke(P value); }"
            .into(),
    );
    ws.open(
        "/functions/Integer.java",
        "package java.lang; public final class Integer { public int intValue() { return 0; } }"
            .into(),
    );
    ws.open(
        "/functions/String.java",
        "package java.lang; public final class String {}".into(),
    );
    let file = "/functions/Use.java";
    let source = "package callbacks; class Use { void use(Callbacks callbacks) { callbacks.getCallback().invoke(\"text\").intValue(); callbacks.getExplicit().invoke(\"text\").intValue(); callbacks.getCallback().invoke(1).intValue(); callbacks.getExplicit().invoke().intValue(); } }";
    ws.open(file, source.into());
    for (prefix, label, expected) in [
        ("callbacks.getCallback().", "invoke", true),
        ("callbacks.getExplicit().", "invoke", true),
        (
            "callbacks.getCallback().invoke(\"text\").",
            "intValue",
            true,
        ),
        (
            "callbacks.getExplicit().invoke(\"text\").",
            "intValue",
            true,
        ),
        ("callbacks.getCallback().invoke(1).", "intValue", false),
        ("callbacks.getExplicit().invoke().", "intValue", false),
    ] {
        let offset = source.find(prefix).unwrap() + prefix.len();
        let labels: Vec<_> = ws
            .complete(file, offset, true)
            .map(|result| result.items.into_iter().map(|item| item.label).collect())
            .unwrap_or_default();
        assert_eq!(
            labels.iter().any(|item| item == label),
            expected,
            "{prefix}: {labels:?}"
        );
    }
}

#[test]
fn contextual_implicit_lambda_parameters_do_not_become_zero_arity_functions() {
    let mut ws = workspace();
    ws.change(FILE, "package callbacks\nval zero = { 1 }\nval explicit: (String) -> Int = { 2 }\nfun use(value: String) { value.let { it.length } }\n".into());
    let zero = expression_type(&ws, "{ 1 }");
    assert_eq!(zero.name(), Some("Function0"));
    assert_eq!(
        zero.args().iter().map(Type::name).collect::<Vec<_>>(),
        [Some("Int")]
    );
    for expression in ["{ it.length }", "{ 2 }"] {
        assert_eq!(
            expression_type(&ws, expression),
            Type::Unknown,
            "{expression}"
        );
    }
}

#[test]
fn nullable_function_property_accepts_null_in_partial_safe_call_without_accepting_unknowns() {
    let mut ws = workspace();
    let source = r#"package callbacks
class Product { fun propertyOnly() {} }
class Other { fun methodOnly() {} }
class Callables {
    val maybe: (String?) -> Product? = { null }
    val strict: (String) -> Product = { Product() }
}

fun use(callables: Callables) {
    callables.maybe(null)?.propertyOnly()
    callables.strict(null)
    callables.maybe(missing)
    callables.maybe(1)
}
"#;
    ws.change(FILE, source.into());
    let null = expression_type(&ws, "null");
    assert_eq!(null.name(), Some("Nothing"));
    assert_eq!(null.package(), Some("kotlin"));
    assert!(null.is_nullable());
    let product = expression_type(&ws, "callables.maybe(null)");
    assert_eq!(product.name(), Some("Product"));
    assert!(product.is_nullable());
    for expression in [
        "callables.strict(null)",
        "callables.maybe(missing)",
        "callables.maybe(1)",
    ] {
        assert_eq!(
            expression_type(&ws, expression),
            Type::Unknown,
            "{expression}"
        );
    }
    let partial = source.replace(
        "callables.maybe(null)?.propertyOnly()",
        "callables.maybe(null)?.pr",
    );
    let prefix = "callables.maybe(null)?.pr";
    let offset = partial.find(prefix).unwrap() + prefix.len();
    ws.change(FILE, partial);
    let labels: Vec<_> = ws
        .complete(FILE, offset, true)
        .unwrap()
        .items
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert!(
        labels.iter().any(|label| label == "propertyOnly"),
        "{labels:?}"
    );
    assert!(
        !labels.iter().any(|label| label == "methodOnly"),
        "{labels:?}"
    );
}

#[test]
fn kotlin_null_argument_to_unannotated_java_reference_keeps_platform_result() {
    let mut ws = workspace();
    ws.open("/functions/JavaApi.java", "package callbacks; public class JavaApi { public JavaApi(String value) {} public static Product consume(String value) { return null; } public Product take(String value) { return null; } public static Product primitive(int value) { return null; } }".into());
    let source = "package callbacks\nclass Product { fun propertyOnly() {} }\nfun strict(value: String): Product = Product()\nfun use(api: JavaApi) { JavaApi.consume(null).propertyOnly(); api.take(null).propertyOnly(); JavaApi(null).take(null).propertyOnly(); JavaApi.primitive(null).propertyOnly(); strict(null).propertyOnly() }\n";
    ws.change(FILE, source.into());
    let diagnostics = ws.diagnostics(FILE);
    let mismatches: Vec<_> = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == Some(ktlsp::diagnostics::DiagnosticCode::CallShapeMismatch)
        })
        .collect();
    assert_eq!(mismatches.len(), 2, "{diagnostics:?}");
    assert!(
        mismatches
            .iter()
            .any(|diagnostic| diagnostic.message.contains("primitive")),
        "{mismatches:?}"
    );
    assert!(
        mismatches
            .iter()
            .any(|diagnostic| diagnostic.message.contains("strict")),
        "{mismatches:?}"
    );
    for prefix in [
        "JavaApi.consume(null).pr",
        "api.take(null).pr",
        "JavaApi(null).take(null).pr",
    ] {
        let offset = source.find(prefix).unwrap() + prefix.len();
        let labels: Vec<_> = ws
            .complete(FILE, offset, true)
            .map(|result| result.items.into_iter().map(|item| item.label).collect())
            .unwrap_or_default();
        assert!(
            labels.iter().any(|label| label == "propertyOnly"),
            "{prefix}: {labels:?}"
        );
    }
    assert_eq!(
        expression_type(&ws, "JavaApi.primitive(null)"),
        Type::Unknown
    );
    // An invalid ordinary call keeps its declared result for editing; diagnostics reject it.
    assert_eq!(expression_type(&ws, "strict(null)").name(), Some("Product"));
}

#[test]
fn expected_property_lambda_body_members_and_goto_use_authored_parameter_types() {
    let mut ws = workspace();
    for (annotation, body, prefix) in [
        ("(String) -> Int", "{ it.length }", "it.len"),
        ("(String) -> Int", "{ text -> text.length }", "text.len"),
        ("((String) -> Int)?", "{ it.length }", "it.len"),
    ] {
        let mut source = format!("package callbacks\nval handler: {annotation} = {body}\n");
        if !annotation.ends_with('?') {
            source.push_str("fun use() { handler(\"text\").resultMarker() }\n");
        }
        ws.change(FILE, source.clone());
        if !annotation.ends_with('?') {
            assert_eq!(
                expression_type(&ws, "handler(\"text\")").name(),
                Some("Int")
            );
        }
        let offset = source.find(prefix).unwrap() + prefix.len();
        let labels: Vec<_> = ws
            .complete(FILE, offset, true)
            .map(|completion| {
                completion
                    .items
                    .into_iter()
                    .map(|item| item.label)
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            labels.iter().any(|label| label == "length"),
            "{annotation} {body}: {labels:?}"
        );
        let selector = source.find(".length").unwrap() + 1;
        let definitions = ws.goto_definition(FILE, selector);
        assert_eq!(definitions.len(), 1, "{body}: {definitions:?}");
        assert_eq!(definitions[0].file, "/functions/Builtins.kt");
        let builtin = ws.doc_text("/functions/Builtins.kt").unwrap();
        assert_eq!(
            &builtin[definitions[0].start_byte..definitions[0].end_byte],
            "length"
        );
    }
}

#[test]
fn expected_property_lambda_unsaved_annotation_edits_and_restore_change_body_members() {
    let mut ws = workspace();
    ws.open(
        "/functions/Other.kt",
        "package callbacks\nclass Other { fun otherOnly(): Int = 1 }\n".into(),
    );
    for (parameter, member, excluded) in [
        ("String", "length", "otherOnly"),
        ("Other", "otherOnly", "length"),
        ("String", "length", "otherOnly"),
    ] {
        let expression = if member == "length" {
            "it.length"
        } else {
            "it.otherOnly()"
        };
        let source =
            format!("package callbacks\nval handler: ({parameter}) -> Int = {{ {expression} }}\n");
        ws.change(FILE, source.clone());
        let offset = source.find("it.").unwrap() + 3;
        let labels: Vec<_> = ws
            .complete(FILE, offset, true)
            .map(|completion| {
                completion
                    .items
                    .into_iter()
                    .map(|item| item.label)
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            labels.iter().any(|label| label == member),
            "{parameter}: {labels:?}"
        );
        assert!(
            !labels.iter().any(|label| label == excluded),
            "{parameter}: {labels:?}"
        );
    }
}

#[test]
fn expected_property_lambda_shape_and_lexical_shadowing_guards_keep_useful_controls() {
    let mut ws = workspace();
    for (declaration, expression) in [
        (
            "val wrong: (String, Int) -> Int = { it.length }",
            "it.length",
        ),
        (
            "val wrong: (String) -> Int = { first, second -> first.length }",
            "first.length",
        ),
        (
            "val receiver: String.() -> Int = { it.length }",
            "it.length",
        ),
        (
            "class Function1<P,R>\nval foreign: Function1<String,Int> = { it.length }",
            "it.length",
        ),
    ] {
        let source = format!(
            "package callbacks\nval control = {{ typed: String -> typed.length }}\n{declaration}\n"
        );
        ws.change(FILE, source.clone());
        assert_eq!(expression_type(&ws, "typed.length").name(), Some("Int"));
        assert_eq!(
            expression_type(&ws, expression),
            Type::Unknown,
            "{declaration}"
        );
    }
    ws.open(
        "/functions/Other.kt",
        "package callbacks\nclass Other { fun otherOnly(): Int = 1 }\n".into(),
    );
    let source = "package callbacks\nval shadowed: (String) -> Int = { text -> val text = Other(); text.otherOnly() }\nval nested: (String) -> Int = { val inner: (Other) -> Int = { it.otherOnly() }; it.length }\n";
    ws.change(FILE, source.into());
    for (prefix, expected, excluded) in [
        ("text.other", "otherOnly", "length"),
        ("it.other", "otherOnly", "length"),
        ("it.len", "length", "otherOnly"),
    ] {
        let offset = source.find(prefix).unwrap() + prefix.len();
        let labels: Vec<_> = ws
            .complete(FILE, offset, true)
            .map(|completion| {
                completion
                    .items
                    .into_iter()
                    .map(|item| item.label)
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            labels.iter().any(|label| label == expected),
            "{prefix}: {labels:?}"
        );
        assert!(
            !labels.iter().any(|label| label == excluded),
            "{prefix}: {labels:?}"
        );
    }
}

#[test]
fn expected_property_lambda_smartcasts_keep_narrowed_body_members_and_goto() {
    let mut ws = workspace();
    let other_file = "/functions/Other.kt";
    let other = "package callbacks\nclass Other { fun otherOnly(): Int = 1 }\n";
    ws.open(other_file, other.into());
    for (binder, parameters) in [("it", ""), ("value", "value ->")] {
        for source in [
            format!("package callbacks\nval handler: (Any) -> Int = {{ {parameters} if ({binder} is Other) {binder}.otherOnly() else 0 }}\n"),
            format!("package callbacks\nfun f({binder}: Any) {{ if ({binder} is Other) run {{ {binder}.otherOnly() }} }}\n"),
            format!("package callbacks\nfun f({binder}: Any) {{ if ({binder} is Other) {{ val noArg: () -> Int = {{ {binder}.otherOnly() }} }} }}\n"),
            format!("package callbacks\nfun f({binder}: Any) {{ if ({binder} is Other) \"text\".let {{ unused -> {binder}.otherOnly() }} }}\n"),
        ] {
        ws.change(FILE, source.clone());
        let prefix = format!("{binder}.other");
        let offset = source.find(&prefix).unwrap() + prefix.len();
        let labels: Vec<_> = ws
            .complete(FILE, offset, true)
            .map(|completion| {
                completion
                    .items
                    .into_iter()
                    .map(|item| item.label)
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            labels.iter().any(|label| label == "otherOnly"),
            "{source}: {labels:?}"
        );
        let definitions = ws.goto_definition(FILE, source.find(".otherOnly").unwrap() + 1);
        assert_eq!(definitions.len(), 1, "{binder}: {definitions:?}");
        assert_eq!(definitions[0].file, other_file);
        assert_eq!(
            &other[definitions[0].start_byte..definitions[0].end_byte],
            "otherOnly"
        );
        }
    }
    for body in [
        "fun f(value: Any) { if (value is Other) { val handler: (String) -> Int = { value -> value.length } } }",
        "fun f(it: Any) { if (it is Other) { val handler: (String) -> Int = { it.length } } }",
        "fun f(it: Any) { if (it is Other) \"text\".let { it.length } }",
        "fun f(it: Any) { if (it is Other) \"text\".let { it -> it.length } }",
        "val handler: (Any) -> Int = { if (it is Other) \"text\".let { it.length } else 0 }",
        "val handler: (Any) -> Int = { value -> if (value is Other) { val value = \"\"; value.length } else 0 }",
    ] {
        let source = format!("package callbacks\n{body}\n");
        ws.change(FILE, source.clone());
        let offset = source.find(".length").unwrap() + ".len".len();
        let labels: Vec<_> = ws
            .complete(FILE, offset, true)
            .map(|completion| {
                completion
                    .items
                    .into_iter()
                    .map(|item| item.label)
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            labels.iter().any(|label| label == "length"),
            "{body}: {labels:?}"
        );
        assert!(
            !labels.iter().any(|label| label == "otherOnly"),
            "{body}: {labels:?}"
        );
        let definitions = ws.goto_definition(FILE, source.find(".length").unwrap() + 1);
        assert_eq!(definitions.len(), 1, "{body}: {definitions:?}");
        assert_eq!(definitions[0].file, "/functions/Builtins.kt");
        let builtin = ws.doc_text("/functions/Builtins.kt").unwrap();
        assert_eq!(
            &builtin[definitions[0].start_byte..definitions[0].end_byte],
            "length"
        );
    }
}
