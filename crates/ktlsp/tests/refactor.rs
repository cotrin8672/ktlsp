use ktlsp::edit::apply_to_text;
use ktlsp::workspace::Workspace;

fn extract_java(src: &str, expression: &str) -> String {
    let key = "mem:///Extract.java";
    let mut ws = Workspace::new();
    ws.open(key, src.into());
    let start = src.find(expression).unwrap();
    let actions = ws.code_actions(key, start, start + expression.len(), start);
    let action = actions
        .iter()
        .find(|action| action.title == "Extract to local variable")
        .unwrap_or_else(|| panic!("missing extract action: {actions:?}"));
    assert_eq!(action.kind, ktlsp::actions::ActionKind::RefactorExtract);
    let edited = apply_to_text(key, src, &action.edits).unwrap();
    assert!(
        !ktlsp::java::JavaParser::new()
            .parse(&edited)
            .root_node()
            .has_error(),
        "{edited}"
    );
    edited
}

#[test]
fn java_extract_local_applies_a_declaration_and_replaces_the_whole_return_expression() {
    let src =
        "class Sum {\n    int sum(int left, int right) {\n        return left + right;\n    }\n}\n";
    assert_eq!(
        extract_java(src, "left + right"),
        "class Sum {\n    int sum(int left, int right) {\n        int extracted = left + right;\n        return extracted;\n    }\n}\n"
    );
    // The declaration preserves the return conversion without changing int arithmetic to long.
    assert_eq!(
        extract_java(
            "class Sum { long sum(int left, int right) { return left + right; } }",
            "left + right"
        ),
        "class Sum { long sum(int left, int right) { long extracted = left + right; return extracted; } }"
    );
    assert_eq!(
        extract_java(
            "class Box { java.util.List<String> value() { return java.util.Collections.emptyList(); } }",
            "java.util.Collections.emptyList()"
        ),
        "class Box { java.util.List<String> value() { java.util.List<String> extracted = java.util.Collections.emptyList(); return extracted; } }"
    );
    assert_eq!(
        extract_java(
            "class Box { int[] value() { return new int[1]; } }",
            "new int[1]"
        ),
        "class Box { int[] value() { int[] extracted = new int[1]; return extracted; } }"
    );
}

#[test]
fn java_extract_local_preserves_unicode_comments_crlf_tabs_and_avoids_method_names() {
    let src = "class 値 {\r\n\tlong sum(int 左, int right) {\r\n\t\tint extracted = 0, extracted1 = 1;\r\n\t\t// keep\r\n\t\treturn /* before */ 左 /* plus */ + right; // after\r\n\t}\r\n}\r\n";
    assert_eq!(
        extract_java(src, "左 /* plus */ + right"),
        src.replace(
            "return /* before */ 左 /* plus */ + right;",
            "long extracted2 = 左 /* plus */ + right;\r\n\t\treturn /* before */ extracted2;"
        )
    );
}

#[test]
fn java_extract_local_rejects_unsafe_contexts_and_invalid_selections() {
    for (key, src, expression) in [
        (
            "Reject.java",
            "class C { int f(int left, int right) { return left + right; } }",
            "left",
        ),
        (
            "Reject.java",
            "class C { int f(boolean flag, int left, int right) { return flag ? left + right : 0; } }",
            "left + right",
        ),
        (
            "Reject.java",
            "class C { int f(boolean flag, int left, int right) { if (flag) return left + right; return 0; } }",
            "left + right",
        ),
        (
            "Reject.java",
            "class C { java.util.function.IntSupplier f(int left, int right) { return () -> { return left + right; }; } }",
            "left + right",
        ),
        (
            "Reject.java",
            "class C { void f() { return call(); } }",
            "call()",
        ),
        (
            "Reject.java",
            "class C { C() { return call(); } }",
            "call()",
        ),
        (
            "Reject.java",
            "class C { int f() { return 1 + ; } }",
            "1 +",
        ),
        (
            "Reject.java",
            "class C { int f()[] { return new int[1]; } }",
            "new int[1]",
        ),
        (
            "Reject.java",
            "class C { static class T {} T f() { class T {} return new C.T(); } }",
            "new C.T()",
        ),
        (
            "Reject.java",
            "class C { int f() { // \\u000a int extracted=1;\n return 1+2; } }",
            "1+2",
        ),
        (
            "Reject.java",
            "class C { int f() { int x = 1 + 2; return x; } }",
            "1 + 2",
        ),
        (
            "Reject.kt",
            "fun f(left: Int, right: Int): Int { return left + right }",
            "left + right",
        ),
    ] {
        let mut ws = Workspace::new();
        ws.open(key, src.into());
        let start = src.rfind(expression).unwrap();
        assert!(
            !ws.code_actions(key, start, start + expression.len(), start)
                .iter()
                .any(|action| action.title == "Extract to local variable"),
            "{src}"
        );
    }
    let key = "New.java";
    let src = "class C { int f(int 左) { return 左 + 1; } }";
    let start = src.find("左 + 1").unwrap();
    let end = start + "左 + 1".len();
    let mut ws = Workspace::new();
    ws.open(key, src.into());
    for (start, end) in [
        (end, start),
        (start, start),
        (start + 1, end),
        (start, src.len() + 1),
        (start, end + 1),
    ] {
        assert!(!ws
            .code_actions(key, start, end, 0)
            .iter()
            .any(|action| action.title == "Extract to local variable"));
    }
}

#[test]
fn java_extract_local_respects_readonly_sources_and_accepts_new_unsaved_java() {
    use ktcore::project_model::{FileOrigin, SourceRepresentation};
    use ktlsp::classpath::{ModuleClasspath, ProjectSnapshot, SourceSetClasspath};
    let src = "class C { int f() { return 1 + 2; } }";
    let start = src.find("1 + 2").unwrap();
    for representation in [SourceRepresentation::Source, SourceRepresentation::Stub] {
        let mut ws = Workspace::new();
        ws.open("Library.java", src.into());
        ws.index.set_file_origin(
            "Library.java",
            FileOrigin {
                origin_id: "selected-library".into(),
                representation,
                project_scope: None,
            },
        );
        assert!(!ws
            .code_actions("Library.java", start, start + 5, start)
            .iter()
            .any(|action| action.title == "Extract to local variable"));
    }
    let root = std::env::temp_dir().join("ktlsp-extract-local-scope");
    let authored = root.join("src");
    let generated = root.join("generated");
    let generated_file = generated.join("Generated.java");
    let task = serde_json::from_value(serde_json::json!({
        "task": ":compileJava",
        "language": "java",
        "task_type": "JavaCompile",
        "arguments": [],
        "annotation_processor_path": [],
        "source_files": [generated_file],
        "java_home": null,
        "output_roots": [],
        "complete": false,
        "problem": "source ownership fixture",
        "options": {
            "sdkSourcePolicy": {
                "adapter": "gradle-source-tree-stock-patterns-v1",
                "language": "java",
                "task": ":compileJava",
                "authored_roots": [authored],
                "future_roots": [],
                "generated_roots": [generated]
            }
        }
    })).unwrap();
    let model = std::sync::Arc::new(ProjectSnapshot::new(vec![ModuleClasspath {
        module: ":extract".into(),
        project_dir: root,
        build: "extract".into(),
        source_sets: vec![SourceSetClasspath {
            name: "main".into(),
            source_roots: vec![authored.clone(), generated],
            compiler_tasks: vec![task],
            ..Default::default()
        }],
        entries: vec![],
        unresolved: vec![],
    }]));
    let mut ws = Workspace::new();
    ws.set_project_model(model);
    let generated_key = ktlsp::paths::file_key(&generated_file);
    ws.open(&generated_key, src.into());
    let origin = ws.index.file_origin(&generated_key).unwrap();
    assert_eq!(origin.representation, SourceRepresentation::Source);
    assert!(origin.project_scope.is_some());
    assert!(!ws
        .code_actions(&generated_key, start, start + 5, start)
        .iter()
        .any(|action| action.title == "Extract to local variable"));
    let new_key = ktlsp::paths::file_key(&authored.join("New.java"));
    ws.open(&new_key, src.into());
    let action = ws
        .code_actions(&new_key, start, start + 5, start)
        .into_iter()
        .find(|action| action.title == "Extract to local variable")
        .expect("new unsaved Java source");
    assert_eq!(
        apply_to_text(&new_key, src, &action.edits).unwrap(),
        "class C { int f() { int extracted = 1 + 2; return extracted; } }"
    );
}

#[test]
fn expression_body_can_convert_to_block_body() {
    let mut ws = Workspace::new();
    let key = "mem:///Main.kt".to_string();
    let src = "fun answer(): Int = 42\n";
    ws.open(key.clone(), src.to_string());

    let actions = ws.code_actions(
        &key,
        src.find("42").unwrap(),
        src.find("42").unwrap(),
        src.find("42").unwrap(),
    );
    let action = actions
        .iter()
        .find(|action| action.title == "Convert expression body to block body")
        .unwrap_or_else(|| panic!("missing refactor action: {actions:?}"));

    assert_eq!(
        apply_to_text(&key, src, &action.edits).unwrap(),
        "fun answer(): Int { return 42 }\n"
    );
}

#[test]
fn simple_block_body_can_convert_to_expression_body() {
    let mut ws = Workspace::new();
    let key = "mem:///Main.kt".to_string();
    let src = "fun answer(): Int { return 42 }\n";
    ws.open(key.clone(), src.to_string());

    let actions = ws.code_actions(
        &key,
        src.find("return").unwrap(),
        src.find("return").unwrap(),
        src.find("return").unwrap(),
    );
    let action = actions
        .iter()
        .find(|action| action.title == "Convert block body to expression body")
        .unwrap_or_else(|| panic!("missing refactor action: {actions:?}"));

    assert_eq!(
        apply_to_text(&key, src, &action.edits).unwrap(),
        "fun answer(): Int = 42\n"
    );
}

#[test]
fn multi_statement_block_body_has_no_expression_conversion() {
    let mut ws = Workspace::new();
    let key = "mem:///Main.kt".to_string();
    let src = "fun answer(): Int { println(1); return 42 }\n";
    ws.open(key.clone(), src.to_string());

    let actions = ws.code_actions(
        &key,
        src.find("return").unwrap(),
        src.find("return").unwrap(),
        src.find("return").unwrap(),
    );

    assert!(
        !actions
            .iter()
            .any(|action| action.title == "Convert block body to expression body"),
        "multi-statement block must not offer conversion: {actions:?}"
    );
}
