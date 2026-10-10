//! Compiler-verified member roles: Java fields/methods, callable Kotlin properties and nested types.

use std::collections::HashSet;

use ktlsp::edit::apply_to_text;
use ktlsp::index::Tier;
use ktlsp::infer::{self, FileCtx};
use ktlsp::java::{self, JavaParser};
use ktlsp::parser::KotlinParser;
use ktlsp::project_model::{FileOrigin, ProjectScope, ScopeVisibility, SourceRepresentation};
use ktlsp::symbol::{Def, SymbolKind};
use ktlsp::types::Type;
use ktlsp::workspace::Workspace;

const API: &str = "/selection/api/Reader.java";
const OLD_API: &str = "/selection/old/Reader.java";
const JAVA: &str = "/selection/a/JavaUse.java";
const KOTLIN: &str = "/selection/a/KotlinUse.kt";
const B_JAVA: &str = "/selection/b/JavaUse.java";
const B_KOTLIN: &str = "/selection/b/KotlinUse.kt";
const NEW: &str = "package api; public class Reader { public int body; public Payload body() { return null; } public void currentOnly() {} public static class Factory {} }";
const OLD: &str = "package api; public class Reader { public int body; public Payload body() { return null; } public void oldOnly() {} public static class Factory {} }";
const JAVA_USE: &str = "package app; import api.Reader; class JavaUse { void run(Reader reader) { reader.body(); int value = reader.body; Reader.Factory factory = new Reader.Factory(); } }";
const KOTLIN_USE: &str = "package app\nimport api.Reader\nfun run(reader: Reader) { reader.body(); val value = reader.body; val factory: Reader.Factory = Reader.Factory() }\n";

fn origin(id: &str, module: Option<&str>) -> FileOrigin {
    FileOrigin {
        origin_id: id.to_string(),
        representation: SourceRepresentation::Source,
        project_scope: module.map(|module| ProjectScope {
            module: module.to_string(),
            source_set: "main".to_string(),
        }),
    }
}

fn workspace(duplicate: bool) -> Workspace {
    let mut ws = Workspace::new();
    ws.open(API, NEW.to_string());
    ws.index.set_file_origin(API, origin("api", Some("api")));
    ws.open(
        "/selection/api/Payload.java",
        "package api; public class Payload { public void useful() {} }".to_string(),
    );
    ws.index
        .set_file_origin("/selection/api/Payload.java", origin("api", Some("api")));
    if duplicate {
        let tree = JavaParser::new().parse(OLD);
        ws.index
            .replace_file(OLD_API, java::extract_symbols(&tree, OLD), Tier::Durable);
        ws.index.set_file_origin(OLD_API, origin("old", None));
    }
    for (file, text, module) in [
        (JAVA, JAVA_USE, "a"),
        (KOTLIN, KOTLIN_USE, "a"),
        (B_JAVA, JAVA_USE, "b"),
        (B_KOTLIN, KOTLIN_USE, "b"),
    ] {
        ws.open(file, text.to_string());
        ws.index.set_file_origin(file, origin(module, Some(module)));
    }
    ws.index.replace_scope_visibilities(
        [
            ("api", vec!["api"]),
            ("a", vec!["a", "api", "old"]),
            ("b", vec!["b", if duplicate { "old" } else { "api" }]),
        ]
        .into_iter()
        .map(|(module, ids)| ScopeVisibility {
            scope: origin(module, Some(module)).project_scope.unwrap(),
            ordered_origins: ids.into_iter().map(str::to_string).collect(),
        })
        .collect(),
    );
    ws.bump_index_revision();
    ws.assume_index_complete_for_tests();
    ws
}

fn target(ws: &Workspace, file: &str, name: &str, kind: SymbolKind) -> Def {
    let entry = ws
        .index
        .lookup_by_name(name)
        .iter()
        .find(|e| e.path.as_ref() == file && e.sym.kind == kind)
        .unwrap();
    Def {
        file: file.to_string(),
        start_byte: entry.sym.start_byte,
        end_byte: entry.sym.end_byte,
    }
}

fn goto(ws: &mut Workspace, file: &str, needle: &str) -> Vec<Def> {
    let source = ws.doc_text(file).unwrap();
    let offset = source.find(needle).unwrap() + needle.rfind('.').map_or(0, |dot| dot + 1);
    ws.goto_definition(file, offset)
}

fn infer_expression(ws: &Workspace, file: &str, expression: &str) -> Type {
    let source = ws.doc_text(file).unwrap();
    let tree = KotlinParser::new().parse(&source);
    let start = source.find(expression).unwrap();
    let node = tree
        .root_node()
        .descendant_for_byte_range(start, start + expression.len())
        .unwrap();
    assert_eq!(&source[node.byte_range()], expression);
    infer::infer(
        &ws.index.scoped_for_file(file),
        node,
        &source,
        &FileCtx::from_tree(&tree, &source),
    )
}

fn labels(ws: &mut Workspace, file: &str, prefix: &str) -> HashSet<String> {
    let source = ws.doc_text(file).unwrap();
    let offset = source.find(prefix).unwrap() + prefix.len();
    ws.complete(file, offset, true)
        .map(|completion| {
            completion
                .items
                .into_iter()
                .map(|item| item.label)
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn java_call_and_field_are_distinct() {
    let mut ws = workspace(false);
    for (needle, kind) in [
        ("reader.body()", SymbolKind::Function),
        ("reader.body;", SymbolKind::Property),
    ] {
        let expected = target(&ws, API, "body", kind);
        assert_eq!(goto(&mut ws, JAVA, needle), vec![expected]);
    }
}

#[test]
fn kotlin_call_excludes_same_named_field() {
    let mut ws = workspace(false);
    let expected = target(&ws, API, "body", SymbolKind::Function);
    assert_eq!(goto(&mut ws, KOTLIN, "reader.body()"), vec![expected]);
}

#[test]
fn kotlin_value_excludes_same_named_method() {
    let mut ws = workspace(false);
    let expected = target(&ws, API, "body", SymbolKind::Property);
    assert_eq!(goto(&mut ws, KOTLIN, "reader.body;"), vec![expected]);
}

#[test]
fn java_nested_type_uses_qualified_owner() {
    let mut ws = workspace(false);
    let expected = target(&ws, API, "Factory", SymbolKind::Class);
    assert_eq!(goto(&mut ws, JAVA, "Reader.Factory"), vec![expected]);
}

#[test]
fn java_imported_fqn_and_generic_outer_nested_types_keep_exact_owner() {
    let mut ws = workspace(true);
    let outer = "/selection/api/Outer.java";
    ws.open(
        outer,
        "package api; public class Outer<T> { public class Inner<U> { public class Leaf {} } }"
            .to_string(),
    );
    ws.index.set_file_origin(outer, origin("api", Some("api")));
    ws.open(
        "/selection/api/Unrelated.java",
        "package api; public class Unrelated { public static class Factory {} }".to_string(),
    );
    ws.index
        .set_file_origin("/selection/api/Unrelated.java", origin("api", Some("api")));
    let source = "package app; import api.Reader; import api.Outer; class JavaUse { Reader.Factory imported = new Reader.Factory(); api.Reader.Factory qualified = new api.Reader.Factory(); Outer<Reader>.Inner<Reader.Factory>.Leaf nested; }";
    ws.change(JAVA, source.to_string());
    let expected = target(&ws, API, "Factory", SymbolKind::Class);
    for needle in [
        "Reader.Factory imported",
        "Reader.Factory();",
        "api.Reader.Factory qualified",
        "api.Reader.Factory();",
    ] {
        let offset = source.find(needle).unwrap() + needle.find("Factory").unwrap();
        assert_eq!(
            ws.goto_definition(JAVA, offset),
            vec![expected.clone()],
            "{needle}"
        );
        assert_eq!(
            ws.type_definition(JAVA, offset),
            vec![expected.clone()],
            "{needle}"
        );
    }
    for (name, container) in [("Inner", Some("Outer")), ("Leaf", Some("Outer.Inner"))] {
        let expected = target(&ws, outer, name, SymbolKind::Class);
        let offset = source.find(&format!(".{name}")).unwrap() + 1;
        assert_eq!(ws.goto_definition(JAVA, offset), vec![expected], "{name}");
        let entry = ws
            .index
            .lookup_by_name(name)
            .iter()
            .find(|entry| entry.path.as_ref() == outer)
            .unwrap();
        assert_eq!(entry.sym.container.as_deref(), container);
    }
    ws.assume_index_complete_for_tests();
    let errors = ws.diagnostics(JAVA);
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn java_unknown_nested_owner_does_not_borrow_an_unrelated_factory() {
    let mut ws = workspace(true);
    ws.change(
        JAVA,
        "package app; class JavaUse { missing.Reader.Factory missing; }".to_string(),
    );
    assert!(goto(&mut ws, JAVA, "missing.Reader.Factory").is_empty());
    ws.change(
        JAVA,
        "package app; import api.Reader; class JavaUse { Reader.Missing.Factory missing; }"
            .to_string(),
    );
    assert!(goto(&mut ws, JAVA, "Reader.Missing.Factory").is_empty());
}

#[test]
fn java_nested_type_each_module_selects_its_actual_owner() {
    let mut ws = workspace(true);
    for (file, expected_file) in [(JAVA, API), (B_JAVA, OLD_API)] {
        let expected = target(&ws, expected_file, "Factory", SymbolKind::Class);
        assert_eq!(
            goto(&mut ws, file, "Reader.Factory"),
            vec![expected],
            "{file}"
        );
    }
}

#[test]
fn java_each_module_selects_only_its_actual_method_owner() {
    check_module_owners(JAVA, B_JAVA);
}

#[test]
fn kotlin_each_module_selects_only_its_actual_method_owner() {
    check_module_owners(KOTLIN, B_KOTLIN);
}

fn check_module_owners(a: &str, b: &str) {
    let mut ws = workspace(true);
    for (file, expected_file) in [(a, API), (b, OLD_API)] {
        let expected = target(&ws, expected_file, "body", SymbolKind::Function);
        assert_eq!(
            goto(&mut ws, file, "reader.body()"),
            vec![expected],
            "{file}"
        );
    }
}

#[test]
fn java_completion_excludes_other_artifact() {
    check_completion(JAVA, B_JAVA);
}

#[test]
fn kotlin_completion_excludes_other_artifact() {
    check_completion(KOTLIN, B_KOTLIN);
}

fn check_completion(a: &str, b: &str) {
    let mut ws = workspace(true);
    for (file, correct, wrong) in [(a, "currentOnly", "oldOnly"), (b, "oldOnly", "currentOnly")] {
        let source = ws.doc_text(file).unwrap();
        let offset = source.find("reader.").unwrap() + "reader.".len();
        let labels: HashSet<_> = ws
            .complete(file, offset, true)
            .unwrap()
            .items
            .into_iter()
            .map(|item| item.label)
            .collect();
        assert!(labels.contains(correct), "{file}: {labels:?}");
        assert!(!labels.contains(wrong), "{file}: {labels:?}");
    }
}

const CALLABLE: &str = r#"package callable
class Both {
    val action: () -> Int = { 1 }
    fun action(): String = "method"
}
class PropertyOnly {
    val action: () -> Int = { 2 }
}
class DifferentArity {
    val action: () -> Int = { 3 }
    fun action(value: Int): String = "method"
}
class DifferentTypes {
    val action: (Int) -> String = { "property" }
    fun action(value: String): String = value
}
fun run(both: Both, propertyOnly: PropertyOnly, different: DifferentArity, typed: DifferentTypes) {
    both.action()
    val value = both.action
    propertyOnly.action()
    different.action()
    typed.action(1)
    typed.action("x")
}
"#;
const CALLABLE_FILE: &str = "/selection/callable/Use.kt";

fn callable_target(ws: &Workspace, container: &str, kind: SymbolKind) -> Def {
    let entry = ws
        .index
        .lookup_by_name("action")
        .iter()
        .find(|e| {
            e.path.as_ref() == CALLABLE_FILE
                && e.sym.container.as_deref() == Some(container)
                && e.sym.kind == kind
        })
        .unwrap();
    Def {
        file: CALLABLE_FILE.to_string(),
        start_byte: entry.sym.start_byte,
        end_byte: entry.sym.end_byte,
    }
}

fn callable_workspace() -> Workspace {
    let mut ws = Workspace::new();
    // Explicit normalized built-in facts are needed before certifying the whole index complete.
    ws.open(
        "/selection/stdlib/Types.kt",
        "package kotlin\nclass Int\nclass String\n".to_string(),
    );
    ws.open(CALLABLE_FILE, CALLABLE.to_string());
    ws
}

#[test]
fn applicable_function_wins_over_callable_property_but_value_selects_property() {
    let mut ws = callable_workspace();
    let method = callable_target(&ws, "Both", SymbolKind::Function);
    let property = callable_target(&ws, "Both", SymbolKind::Property);
    assert_eq!(goto(&mut ws, CALLABLE_FILE, "both.action()"), vec![method]);
    assert_eq!(
        goto(&mut ws, CALLABLE_FILE, "both.action\n"),
        vec![property]
    );
}

#[test]
fn property_only_call_resolves_to_the_callable_property() {
    let mut ws = callable_workspace();
    let property = callable_target(&ws, "PropertyOnly", SymbolKind::Property);
    assert_eq!(
        goto(&mut ws, CALLABLE_FILE, "propertyOnly.action()"),
        vec![property]
    );
}

#[test]
fn wrong_arity_function_does_not_hide_applicable_callable_property() {
    let mut ws = callable_workspace();
    let property = callable_target(&ws, "DifferentArity", SymbolKind::Property);
    assert_eq!(
        goto(&mut ws, CALLABLE_FILE, "different.action()"),
        vec![property]
    );
    ws.assume_index_complete_for_tests();
    assert!(
        ws.diagnostics(CALLABLE_FILE).is_empty(),
        "{:?}",
        ws.diagnostics(CALLABLE_FILE)
    );
}

#[test]
fn same_arity_known_types_select_function_or_callable_property() {
    let mut ws = callable_workspace();
    let method = callable_target(&ws, "DifferentTypes", SymbolKind::Function);
    let property = callable_target(&ws, "DifferentTypes", SymbolKind::Property);
    assert_eq!(
        goto(&mut ws, CALLABLE_FILE, "typed.action(1)"),
        vec![property]
    );
    assert_eq!(
        goto(&mut ws, CALLABLE_FILE, "typed.action(\"x\")"),
        vec![method]
    );
    ws.assume_index_complete_for_tests();
    assert!(
        ws.diagnostics(CALLABLE_FILE).is_empty(),
        "{:?}",
        ws.diagnostics(CALLABLE_FILE)
    );
}

#[test]
fn callable_property_and_function_signatures_follow_the_selected_member() {
    let mut ws = callable_workspace();
    for (needle, expected) in [
        ("typed.action(1)", "action(p1: Int): String"),
        ("typed.action(\"x\")", "action(p1: String): String"),
        ("propertyOnly.action()", "action(): Int"),
    ] {
        let source = ws.doc_text(CALLABLE_FILE).unwrap();
        let offset = source.find(needle).unwrap() + needle.len() - 1;
        let help = ws
            .signature_help(CALLABLE_FILE, offset)
            .expect("selected callable signature");
        assert_eq!(help.signatures.len(), 1, "{needle}: {help:?}");
        assert_eq!(help.signatures[0].label, expected, "{needle}");
    }
}

#[test]
fn unknown_and_incomplete_arguments_keep_viable_function_and_property_candidates() {
    let mut ws = callable_workspace();
    let method = callable_target(&ws, "DifferentTypes", SymbolKind::Function);
    let property = callable_target(&ws, "DifferentTypes", SymbolKind::Property);
    ws.change(
        CALLABLE_FILE,
        format!("{CALLABLE}\nfun unknown(typed: DifferentTypes) {{ typed.action(missing) }}\n"),
    );
    let defs = goto(&mut ws, CALLABLE_FILE, "typed.action(missing)");
    assert!(
        defs.contains(&method) && defs.contains(&property),
        "{defs:?}"
    );
    ws.assume_index_complete_for_tests();
    let diagnostics = ws.diagnostics(CALLABLE_FILE);
    assert!(
        !diagnostics.iter().any(|diagnostic| diagnostic.code
            == Some(ktlsp::diagnostics::DiagnosticCode::CallShapeMismatch)),
        "{diagnostics:?}"
    );
    ws.change(
        CALLABLE_FILE,
        format!("{CALLABLE}\nfun unfinished(typed: DifferentTypes) {{ typed.action( }}\n"),
    );
    let defs = goto(&mut ws, CALLABLE_FILE, "typed.action( }");
    assert!(
        defs.contains(&method) && defs.contains(&property),
        "{defs:?}"
    );
    let source = ws.doc_text(CALLABLE_FILE).unwrap();
    let offset = source.rfind("typed.action(").unwrap() + "typed.".len();
    let labels: HashSet<_> = ws
        .complete(CALLABLE_FILE, offset, true)
        .unwrap()
        .items
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert!(labels.contains("action"), "{labels:?}");
}

const RESULT_CALLABLE: &str = r#"package callable
class Product {
    fun useful() {}
}
class Other {
    fun wrong() {}
}
class Factory {
    val make: (Int) -> Product = { Product() }
    fun make(value: String): Other = Other()
}
class Only {
    fun produce(value: Int): Product = Product()
}
fun use(factory: Factory, only: Only) {
    factory.make(1).useful()
    factory.make("x").wrong()
}
"#;

#[test]
fn callable_property_result_inference_and_completion_follow_argument_types() {
    let mut ws = callable_workspace();
    ws.change(CALLABLE_FILE, RESULT_CALLABLE.to_string());
    for (expression, expected) in [
        ("factory.make(1)", "Product"),
        ("factory.make(\"x\")", "Other"),
    ] {
        assert_eq!(
            infer_expression(&ws, CALLABLE_FILE, expression).name(),
            Some(expected),
            "{expression}"
        );
    }
    let property = labels(&mut ws, CALLABLE_FILE, "factory.make(1).us");
    assert!(
        property.contains("useful") && !property.contains("wrong"),
        "{property:?}"
    );
    let function = labels(&mut ws, CALLABLE_FILE, "factory.make(\"x\").wr");
    assert!(
        function.contains("wrong") && !function.contains("useful"),
        "{function:?}"
    );
    ws.change(
        CALLABLE_FILE,
        format!("{RESULT_CALLABLE}\nfun unknown(factory: Factory) {{ factory.make(missing) }}\n"),
    );
    assert_eq!(
        infer_expression(&ws, CALLABLE_FILE, "factory.make(missing)"),
        Type::Unknown
    );
}

#[test]
fn rejected_unique_call_keeps_navigation_but_has_no_successful_result_type() {
    let mut ws = callable_workspace();
    ws.change(
        CALLABLE_FILE,
        format!(
            "{RESULT_CALLABLE}\nfun invalid(only: Only) {{ only.produce(\"bad\").useful() }}\n"
        ),
    );
    let expected = target(&ws, CALLABLE_FILE, "produce", SymbolKind::Function);
    assert_eq!(
        goto(&mut ws, CALLABLE_FILE, "only.produce(\"bad\")"),
        vec![expected]
    );
    assert_eq!(
        infer_expression(&ws, CALLABLE_FILE, "only.produce(\"bad\")"),
        Type::Unknown
    );
    assert!(!labels(&mut ws, CALLABLE_FILE, "only.produce(\"bad\").us").contains("useful"));
    ws.assume_index_complete_for_tests();
    let errors = ws.diagnostics(CALLABLE_FILE);
    assert!(
        errors
            .iter()
            .any(|error| error.code == Some(ktlsp::diagnostics::DiagnosticCode::CallShapeMismatch)),
        "{errors:?}"
    );
}

#[test]
fn callable_member_unsaved_signature_change_invalidates_selected_result() {
    let mut ws = callable_workspace();
    ws.change(CALLABLE_FILE, RESULT_CALLABLE.to_string());
    assert_eq!(
        infer_expression(&ws, CALLABLE_FILE, "factory.make(1)").name(),
        Some("Product")
    );
    ws.change(
        CALLABLE_FILE,
        RESULT_CALLABLE.replace("fun make(value: String)", "fun make(value: Int)"),
    );
    assert_eq!(
        infer_expression(&ws, CALLABLE_FILE, "factory.make(1)").name(),
        Some("Other")
    );
    let selected = labels(&mut ws, CALLABLE_FILE, "factory.make(1).");
    assert!(
        selected.contains("wrong") && !selected.contains("useful"),
        "{selected:?}"
    );
    let expected = target(&ws, CALLABLE_FILE, "make", SymbolKind::Function);
    assert_eq!(
        goto(&mut ws, CALLABLE_FILE, "factory.make(1)"),
        vec![expected]
    );
}

#[test]
fn callable_type_metadata_preserves_parameters_results_nullability_and_cache_roundtrip() {
    let source = "package callable\nimport api.Payload\nclass Holder {\n val action: (text: String?, count: Int) -> Payload? = { _, _ -> null }\n val optional: ((Int) -> Payload)? = null\n val nested: () -> () -> Payload? = { { null } }\n val receiver: Payload.(String) -> Int = { 1 }\n}\n";
    let tree = KotlinParser::new().parse(source);
    let symbols = ktlsp::indexer::extract_symbols(&tree, source, "callable");
    let cached = bincode::serialize(&symbols).unwrap();
    let decoded: Vec<ktlsp::symbol::IndexedSymbol> = bincode::deserialize(&cached).unwrap();
    assert_eq!(decoded, symbols);
    let ty = symbols
        .iter()
        .find(|symbol| symbol.name == "action")
        .unwrap()
        .value_type
        .as_ref()
        .unwrap();
    assert_eq!(ty.name, "Function2");
    assert_eq!(*ty.package_candidates, ["kotlin"]);
    assert_eq!(
        ty.args
            .iter()
            .map(|arg| arg.name.as_str())
            .collect::<Vec<_>>(),
        ["String", "Int", "Payload"]
    );
    assert!(
        ty.args[0].nullable && ty.args[2].nullable,
        "{ty:?}; {}",
        tree.root_node().to_sexp()
    );
    assert_eq!(*ty.args[2].package_candidates, ["api"]);
    let optional = symbols
        .iter()
        .find(|symbol| symbol.name == "optional")
        .unwrap()
        .value_type
        .as_ref()
        .unwrap();
    assert_eq!(optional.name, "Function1");
    assert!(optional.nullable);
    let nested = symbols
        .iter()
        .find(|symbol| symbol.name == "nested")
        .unwrap()
        .value_type
        .as_ref()
        .unwrap();
    assert!(
        !nested.nullable && !nested.args[0].nullable && nested.args[0].args[0].nullable,
        "{nested:?}"
    );
    let receiver = symbols
        .iter()
        .find(|symbol| symbol.name == "receiver")
        .unwrap()
        .value_type
        .as_ref()
        .unwrap();
    assert_eq!(receiver.name, "Function2");
    assert_eq!(
        receiver
            .args
            .iter()
            .map(|arg| arg.name.as_str())
            .collect::<Vec<_>>(),
        ["Payload", "String", "Int"]
    );
}

#[test]
fn custom_function_n_class_is_not_mistaken_for_a_kotlin_function_type() {
    for name in ["Function1", "callable.Function1"] {
        let mut ws = callable_workspace();
        ws.change(CALLABLE_FILE, format!("package callable\nclass Function1<A, R>\nclass Holder {{\n val action: {name}<Int, String> = Function1()\n}}\nfun run(holder: Holder) {{ holder.action(1) }}\n"));
        assert_eq!(
            infer_expression(&ws, CALLABLE_FILE, "holder.action(1)"),
            Type::Unknown,
            "{name}"
        );
        let property = target(&ws, CALLABLE_FILE, "action", SymbolKind::Property);
        assert_eq!(
            goto(&mut ws, CALLABLE_FILE, "holder.action(1)"),
            vec![property]
        );
    }
}

#[test]
fn closed_kotlin_member_roles_and_result_completion_see_unsaved_java_edits() {
    let mut ws = workspace(false);
    let source = "package app\nimport api.Reader\nfun run(reader: Reader) { reader.body(); val field = reader.body; reader.body().useful() }\n";
    let root = std::env::temp_dir().join(format!(
        "ktlsp-member-selection-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("Closed.kt");
    std::fs::write(&path, source).unwrap();
    let file = ktlsp::paths::file_key(&path);
    ws.open(&file, source.to_string());
    ws.index.set_file_origin(&file, origin("a", Some("a")));
    ws.close(&file);
    for _ in 0..2 {
        let method = target(&ws, API, "body", SymbolKind::Function);
        assert_eq!(goto(&mut ws, &file, "reader.body()"), vec![method]);
        let field = target(&ws, API, "body", SymbolKind::Property);
        assert_eq!(goto(&mut ws, &file, "reader.body;"), vec![field]);
        assert!(labels(&mut ws, &file, "reader.body().us").contains("useful"));
    }
    ws.open(
        "/selection/api/Other.java",
        "package api; public class Other { public void wrong() {} }".to_string(),
    );
    ws.index
        .set_file_origin("/selection/api/Other.java", origin("api", Some("api")));
    ws.change(API, NEW.replace("Payload body", "Other body"));
    let selected = labels(&mut ws, &file, "reader.body().");
    assert!(
        selected.contains("wrong") && !selected.contains("useful"),
        "{selected:?}"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
    assert!(root.starts_with(std::env::temp_dir()));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn field_rename_applies_only_to_values_and_preserves_same_named_method_calls() {
    let mut ws = workspace(false);
    let declaration = target(&ws, API, "body", SymbolKind::Property);
    let references = ws.references(API, declaration.start_byte, true);
    assert_eq!(references.len(), 5, "{references:?}");
    let edits = ws.rename(API, declaration.start_byte, "stored").unwrap();
    assert_eq!(edits.len(), 5, "{edits:?}");
    for file in [API, JAVA, KOTLIN, B_JAVA, B_KOTLIN] {
        let source = ws.doc_text(file).unwrap();
        let changed = apply_to_text(file, &source, &edits).unwrap();
        assert_eq!(
            changed,
            source
                .replace("int body;", "int stored;")
                .replace("reader.body;", "reader.stored;"),
            "{file}"
        );
        ws.change(file, changed);
    }
    for file in [JAVA, KOTLIN, B_JAVA, B_KOTLIN] {
        let method = target(&ws, API, "body", SymbolKind::Function);
        assert_eq!(goto(&mut ws, file, "reader.body()"), vec![method]);
        let field = target(&ws, API, "stored", SymbolKind::Property);
        assert_eq!(goto(&mut ws, file, "reader.stored;"), vec![field]);
    }
}

#[test]
fn callable_property_and_function_renames_apply_only_to_their_selected_calls() {
    for (kind, expected) in [
        (
            SymbolKind::Property,
            CALLABLE
                .replace(
                    "val action: (Int) -> String",
                    "val transform: (Int) -> String",
                )
                .replace("typed.action(1)", "typed.transform(1)"),
        ),
        (
            SymbolKind::Function,
            CALLABLE
                .replace("fun action(value: String)", "fun transform(value: String)")
                .replace("typed.action(\"x\")", "typed.transform(\"x\")"),
        ),
    ] {
        let mut ws = callable_workspace();
        let declaration = callable_target(&ws, "DifferentTypes", kind);
        let references = ws.references(CALLABLE_FILE, declaration.start_byte, true);
        assert_eq!(references.len(), 2, "{kind:?}: {references:?}");
        let edits = ws
            .rename(CALLABLE_FILE, declaration.start_byte, "transform")
            .unwrap();
        assert_eq!(edits.len(), 2, "{kind:?}: {edits:?}");
        let changed = apply_to_text(CALLABLE_FILE, CALLABLE, &edits).unwrap();
        assert_eq!(changed, expected, "{kind:?}");
        ws.change(CALLABLE_FILE, changed);
        let expected = target(&ws, CALLABLE_FILE, "transform", kind);
        let needle = if kind == SymbolKind::Property {
            "typed.transform(1)"
        } else {
            "typed.transform(\"x\")"
        };
        assert_eq!(goto(&mut ws, CALLABLE_FILE, needle), vec![expected]);
    }
}

#[test]
fn nested_type_rename_applies_all_java_and_kotlin_type_and_constructor_uses() {
    let mut ws = workspace(false);
    let declaration = target(&ws, API, "Factory", SymbolKind::Class);
    let references = ws.references(API, declaration.start_byte, true);
    assert_eq!(references.len(), 9, "{references:?}");
    let edits = ws.rename(API, declaration.start_byte, "Maker").unwrap();
    assert_eq!(edits.len(), 9, "{edits:?}");
    for file in [API, JAVA, KOTLIN, B_JAVA, B_KOTLIN] {
        let source = ws.doc_text(file).unwrap();
        let changed = apply_to_text(file, &source, &edits).unwrap();
        assert_eq!(changed, source.replace("Factory", "Maker"), "{file}");
        ws.change(file, changed);
    }
    for file in [JAVA, KOTLIN, B_JAVA, B_KOTLIN] {
        let expected = target(&ws, API, "Maker", SymbolKind::Class);
        assert_eq!(goto(&mut ws, file, "Reader.Maker"), vec![expected]);
    }
}

#[test]
fn method_rename_applies_only_to_method_calls_and_preserves_same_named_fields() {
    let mut ws = workspace(false);
    let declaration = target(&ws, API, "body", SymbolKind::Function);
    // A and B share this artifact in this fixture: declaration + four actual method calls.
    let references = ws.references(API, declaration.start_byte, true);
    assert_eq!(references.len(), 5, "{references:?}");
    let edits = ws.rename(API, declaration.start_byte, "readBody").unwrap();
    assert_eq!(edits.len(), 5, "{edits:?}");
    for file in [API, JAVA, KOTLIN, B_JAVA, B_KOTLIN] {
        let source = ws.doc_text(file).unwrap();
        let changed = apply_to_text(file, &source, &edits).unwrap();
        assert_eq!(changed, source.replace("body()", "readBody()"), "{file}");
        ws.change(file, changed);
    }
    for file in [JAVA, KOTLIN, B_JAVA, B_KOTLIN] {
        let expected = target(&ws, API, "readBody", SymbolKind::Function);
        assert_eq!(goto(&mut ws, file, "reader.readBody()"), vec![expected]);
        let expected = target(&ws, API, "body", SymbolKind::Property);
        assert_eq!(goto(&mut ws, file, "reader.body;"), vec![expected]);
    }
}

#[test]
fn intrinsic_types_need_no_source_stubs_for_valid_diagnostics() {
    let mut ws = Workspace::new();
    ws.open(CALLABLE_FILE, RESULT_CALLABLE.to_string());
    ws.assume_index_complete_for_tests();
    let diagnostics = ws.diagnostics(CALLABLE_FILE);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn intrinsic_types_survive_unrelated_nested_type_arrival_and_unsaved_changes() {
    let mut ws = Workspace::new();
    ws.open(CALLABLE_FILE, RESULT_CALLABLE.to_string());
    let source = RESULT_CALLABLE.replace("fun make(value: String)", "fun make(value: Int)");
    ws.change(CALLABLE_FILE, source.clone());
    let expected = target(&ws, CALLABLE_FILE, "make", SymbolKind::Function);
    assert_eq!(
        goto(&mut ws, CALLABLE_FILE, "factory.make(1)"),
        vec![expected.clone()]
    );
    ws.open(
        "/selection/library/Library.java",
        "package unrelated; public class Library { public static class Int {} }".to_string(),
    );
    assert_eq!(
        goto(&mut ws, CALLABLE_FILE, "factory.make(1)"),
        vec![expected]
    );
    assert_eq!(
        infer_expression(&ws, CALLABLE_FILE, "factory.make(1)").name(),
        Some("Other")
    );
    let selected = labels(&mut ws, CALLABLE_FILE, "factory.make(1).");
    assert!(
        selected.contains("wrong") && !selected.contains("useful"),
        "{selected:?}"
    );
    ws.change(CALLABLE_FILE, RESULT_CALLABLE.to_string());
    let expected = target(&ws, CALLABLE_FILE, "make", SymbolKind::Property);
    assert_eq!(
        goto(&mut ws, CALLABLE_FILE, "factory.make(1)"),
        vec![expected]
    );
    assert_eq!(
        infer_expression(&ws, CALLABLE_FILE, "factory.make(1)").name(),
        Some("Product")
    );
}

#[test]
fn intrinsic_literal_identity_preserves_same_package_type_shadowing() {
    let source = r#"package callable
class Int
class Product {
    fun useful() {}
}
class Other {
    fun wrong() {}
}
class Factory {
    val make: (kotlin.Int) -> Product = { Product() }
    fun make(value: Int): Other = Other()
}
fun use(factory: Factory) { factory.make(1).useful() }
"#;
    let mut ws = Workspace::new();
    ws.open(CALLABLE_FILE, source.to_string());
    let literal = infer_expression(&ws, CALLABLE_FILE, "1");
    assert_eq!(literal, Type::class("Int", Some("kotlin".to_string())));
    let expected = target(&ws, CALLABLE_FILE, "make", SymbolKind::Property);
    assert_eq!(
        goto(&mut ws, CALLABLE_FILE, "factory.make(1)"),
        vec![expected]
    );
    assert_eq!(
        infer_expression(&ws, CALLABLE_FILE, "factory.make(1)").name(),
        Some("Product")
    );
    let method = ws
        .index
        .lookup_by_name("make")
        .iter()
        .find(|entry| entry.sym.kind == SymbolKind::Function)
        .unwrap();
    let formal = infer::declared_type(
        &ws.index,
        &method.sym.params[0],
        &FileCtx::new("elsewhere".into(), Vec::new()),
    );
    assert_eq!(formal, Type::class("Int", Some("callable".to_string())));
}

#[test]
fn intrinsic_literal_identity_preserves_explicit_import_shadowing() {
    let source = r#"package callable
import other.Int
class Product {
    fun useful() {}
}
class Other {
    fun wrong() {}
}
class Factory {
    val make: (kotlin.Int) -> Product = { Product() }
    fun make(value: Int): Other = Other()
}
fun use(factory: Factory) { factory.make(1).useful() }
"#;
    let mut ws = Workspace::new();
    ws.open(
        "/selection/other/Int.kt",
        "package other\nclass Int\n".to_string(),
    );
    ws.open(CALLABLE_FILE, source.to_string());
    let expected = target(&ws, CALLABLE_FILE, "make", SymbolKind::Property);
    assert_eq!(
        goto(&mut ws, CALLABLE_FILE, "factory.make(1)"),
        vec![expected]
    );
    assert_eq!(
        infer_expression(&ws, CALLABLE_FILE, "factory.make(1)").name(),
        Some("Product")
    );
    let method = ws
        .index
        .lookup_by_name("make")
        .iter()
        .find(|entry| entry.sym.kind == SymbolKind::Function)
        .unwrap();
    let formal = infer::declared_type(
        &ws.index,
        &method.sym.params[0],
        &FileCtx::new("elsewhere".into(), Vec::new()),
    );
    assert_eq!(formal, Type::class("Int", Some("other".to_string())));
}

#[test]
fn intrinsic_name_does_not_resolve_an_unknown_explicit_qualification() {
    let source = RESULT_CALLABLE
        .replace("package callable", "package callable\nimport missing.Int")
        .replace("(Int) -> Product", "(kotlin.Int) -> Product")
        .replace("fun make(value: String)", "fun make(value: Int)");
    let mut ws = Workspace::new();
    ws.open(CALLABLE_FILE, source);
    let method = target(&ws, CALLABLE_FILE, "make", SymbolKind::Function);
    let property = target(&ws, CALLABLE_FILE, "make", SymbolKind::Property);
    let defs = goto(&mut ws, CALLABLE_FILE, "factory.make(1)");
    assert!(
        defs.contains(&method) && defs.contains(&property),
        "{defs:?}"
    );
    assert_eq!(
        infer_expression(&ws, CALLABLE_FILE, "factory.make(1)"),
        Type::Unknown
    );
    let method = ws
        .index
        .lookup_by_name("make")
        .iter()
        .find(|entry| entry.sym.kind == SymbolKind::Function)
        .unwrap();
    assert_eq!(
        infer::declared_type(
            &ws.index,
            &method.sym.params[0],
            &FileCtx::new("elsewhere".into(), Vec::new())
        ),
        Type::Unknown
    );
    ws.assume_index_complete_for_tests();
    let diagnostics = ws.diagnostics(CALLABLE_FILE);
    assert!(
        !diagnostics.iter().any(|diagnostic| diagnostic.code
            == Some(ktlsp::diagnostics::DiagnosticCode::CallShapeMismatch)),
        "{diagnostics:?}"
    );
}

#[test]
fn intrinsic_string_literal_keeps_available_jvm_string_members() {
    let mut ws = Workspace::new();
    ws.open(
        "/selection/jdk/String.java",
        "package java.lang; public final class String { public String substring(int begin) { return this; } }".to_string(),
    );
    ws.open(
        CALLABLE_FILE,
        "package callable\nfun use() { \"text\".substring(1) }\n".to_string(),
    );
    let selected = labels(&mut ws, CALLABLE_FILE, "\"text\".sub");
    assert!(selected.contains("substring"), "{selected:?}");
}
