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

const OVERLOAD_JAVA: &str = "/selection/typed/JavaUse.java";
const OVERLOAD_KOTLIN: &str = "/selection/typed/KotlinUse.kt";
const OVERLOAD_USE: &str = "package typed; class JavaUse { void run(Target target, Decoy decoy, int number, String text) { target.twice(1); target.twice(\"x\"); target.twice(number); target.twice(text); decoy.twice(\"x\"); } }";

fn overload_workspace(java_target: bool) -> (Workspace, &'static str) {
    let mut ws = Workspace::new();
    ws.open("/selection/typed/Results.kt", "package typed\nclass NumberResult { fun numberOnly() {} }\nclass TextResult { fun textOnly() {} }\nclass Decoy { fun twice(value: String): TextResult = TextResult() }\n".into());
    let (file, source) = if java_target {
        (
            "/selection/typed/Target.java",
            "package typed; public class Target { public NumberResult twice(int value) { return null; } public TextResult twice(String value) { return null; } public TextResult only(int value) { return null; } }",
        )
    } else {
        (
            "/selection/typed/Target.kt",
            "package typed\nclass Target { fun twice(value: Int): NumberResult = NumberResult()\nfun twice(value: String): TextResult = TextResult()\nfun only(value: Int): TextResult = TextResult() }\n",
        )
    };
    ws.open(file, source.into());
    ws.open(OVERLOAD_JAVA, OVERLOAD_USE.into());
    ws.open(OVERLOAD_KOTLIN, "package typed\nfun run(target: Target, decoy: Decoy) { target.twice(1); target.twice(\"x\"); decoy.twice(\"x\") }\n".into());
    ws.assume_index_complete_for_tests();
    (ws, file)
}

fn overload_target(ws: &Workspace, file: &str, parameter: &str) -> Def {
    let entry = ws
        .index
        .lookup_by_name("twice")
        .iter()
        .find(|entry| {
            entry.path.as_ref() == file
                && entry
                    .sym
                    .params
                    .first()
                    .is_some_and(|ty| ty.name == parameter)
        })
        .unwrap();
    Def {
        file: file.into(),
        start_byte: entry.sym.start_byte,
        end_byte: entry.sym.end_byte,
    }
}

#[test]
fn java_known_arguments_select_exact_java_and_kotlin_overloads() {
    for java_target in [false, true] {
        let (mut ws, file) = overload_workspace(java_target);
        for (needle, parameter) in [
            ("target.twice(1)", if java_target { "int" } else { "Int" }),
            ("target.twice(\"x\")", "String"),
            (
                "target.twice(number)",
                if java_target { "int" } else { "Int" },
            ),
            ("target.twice(text)", "String"),
        ] {
            let expected = overload_target(&ws, file, parameter);
            assert_eq!(
                goto(&mut ws, OVERLOAD_JAVA, needle),
                vec![expected],
                "{file}: {needle}"
            );
        }
    }
}

#[test]
fn java_unknown_argument_keeps_both_authored_overloads() {
    for java_target in [false, true] {
        let (mut ws, file) = overload_workspace(java_target);
        ws.change(
            OVERLOAD_JAVA,
            "package typed; class JavaUse { void run(Target target) { target.twice(missing); } }"
                .into(),
        );
        let mut expected = vec![
            overload_target(&ws, file, if java_target { "int" } else { "Int" }),
            overload_target(&ws, file, "String"),
        ];
        expected.sort_by_key(|def| def.start_byte);
        let mut actual = goto(&mut ws, OVERLOAD_JAVA, "target.twice(missing)");
        actual.sort_by_key(|def| def.start_byte);
        assert_eq!(actual, expected, "{file}");
    }
}

#[test]
fn java_selected_overload_result_is_not_expanded_again() {
    for java_target in [false, true] {
        let (mut ws, file) = overload_workspace(java_target);
        ws.change(OVERLOAD_JAVA, "package typed; class JavaUse { void run(Target target) { target.twice(\"x\").textOnly(); target.twice(1).numberOnly(); } }".into());
        assert!(
            labels(&mut ws, OVERLOAD_JAVA, "target.twice(\"x\").textO").contains("textOnly"),
            "{file}"
        );
        assert!(
            labels(&mut ws, OVERLOAD_JAVA, "target.twice(1).numberO").contains("numberOnly"),
            "{file}"
        );
    }
}

#[test]
fn java_unique_invalid_call_has_navigation_without_a_result_type() {
    for java_target in [false, true] {
        let (mut ws, file) = overload_workspace(java_target);
        ws.change(OVERLOAD_JAVA, "package typed; class JavaUse { void run(Target target) { target.only(true).textOnly(); target.twice(true); } }".into());
        let expected = target(&ws, file, "only", SymbolKind::Function);
        assert_eq!(
            goto(&mut ws, OVERLOAD_JAVA, "target.only(true)"),
            vec![expected]
        );
        assert!(
            !labels(&mut ws, OVERLOAD_JAVA, "target.only(true).textO").contains("textOnly"),
            "{file}"
        );
        assert!(
            goto(&mut ws, OVERLOAD_JAVA, "target.twice(true)").is_empty(),
            "{file}"
        );
        let diagnostics = ws.diagnostics(OVERLOAD_JAVA);
        assert_eq!(
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code
                    == Some(ktlsp::diagnostics::DiagnosticCode::CallShapeMismatch))
                .count(),
            2,
            "{file}: {diagnostics:?}"
        );
    }
}

#[test]
fn java_overload_references_and_applied_rename_preserve_other_calls() {
    for java_target in [false, true] {
        let (mut ws, file) = overload_workspace(java_target);
        // An ambiguous call remains omitted; it must never be edited as one resolved overload.
        ws.change(
            OVERLOAD_JAVA,
            OVERLOAD_USE.replace("decoy.twice", "target.twice(missing); decoy.twice"),
        );
        let declaration = overload_target(&ws, file, "String");
        let refs = ws.references(file, declaration.start_byte, true);
        assert_eq!(refs.len(), 4, "{file}: {refs:?}");
        let edits = ws
            .rename(file, declaration.start_byte, "textTwice")
            .unwrap();
        for changed_file in [
            file,
            OVERLOAD_JAVA,
            OVERLOAD_KOTLIN,
            "/selection/typed/Results.kt",
        ] {
            let source = ws.doc_text(changed_file).unwrap();
            let expected = source
                .replace(
                    "twice(value: String): TextResult",
                    "textTwice(value: String): TextResult",
                )
                .replace("twice(String value)", "textTwice(String value)")
                .replace("target.twice(\"x\")", "target.textTwice(\"x\")")
                .replace("target.twice(text)", "target.textTwice(text)");
            let expected = if changed_file.ends_with("Results.kt") {
                source.to_string()
            } else {
                expected
            };
            let changed = apply_to_text(changed_file, &source, &edits).unwrap();
            assert_eq!(changed, expected, "{changed_file}");
            ws.change(changed_file, changed);
        }
        let expected = target(&ws, file, "textTwice", SymbolKind::Function);
        assert_eq!(
            goto(&mut ws, OVERLOAD_JAVA, "target.textTwice(\"x\")"),
            vec![expected]
        );
    }
}

#[test]
fn java_jvm_facade_overloads_share_typed_argument_selection() {
    let mut ws = Workspace::new();
    let file = "/selection/typed/Functions.kt";
    ws.open(file, "@file:JvmName(\"TypedApi\")\npackage typed\nsuspend fun choose(value: Int): Int = value\nsuspend fun choose(value: String): String = value\n".into());
    ws.open(OVERLOAD_JAVA, "package typed; class JavaUse { void run(kotlin.coroutines.Continuation<?> c) { TypedApi.choose(1, c); TypedApi.choose(\"x\", c); TypedApi.choose(missing, c); } }".into());
    for (needle, parameter) in [
        ("TypedApi.choose(1, c)", "Int"),
        ("TypedApi.choose(\"x\", c)", "String"),
    ] {
        let entry = ws
            .index
            .lookup_by_name("choose")
            .iter()
            .find(|entry| entry.sym.params[0].name == parameter)
            .unwrap();
        let expected = Def {
            file: file.into(),
            start_byte: entry.sym.start_byte,
            end_byte: entry.sym.end_byte,
        };
        assert_eq!(
            goto(&mut ws, OVERLOAD_JAVA, needle),
            vec![expected],
            "{needle}"
        );
    }
    assert_eq!(
        goto(&mut ws, OVERLOAD_JAVA, "TypedApi.choose(missing, c)").len(),
        2
    );
}

#[test]
fn java_unknown_formal_type_remains_a_competing_overload() {
    let mut ws = Workspace::new();
    ws.open(
        "/selection/typed/Target.java",
        "package typed; class Target { void take(String value) {} void take(Missing value) {} }"
            .into(),
    );
    ws.open(
        OVERLOAD_JAVA,
        "package typed; class JavaUse { void run(Target target) { target.take(\"x\"); } }".into(),
    );
    assert_eq!(goto(&mut ws, OVERLOAD_JAVA, "target.take(\"x\")").len(), 2);
}

#[test]
fn java_nested_argument_types_preserve_their_declared_owner() {
    let mut ws = Workspace::new();
    let file = "/selection/typed/Target.java";
    let source = "package typed; class Left { static class Value {} } class Right { static class Value {} } class Target { void take(Left.Value value) {} void take(Right.Value value) {} }";
    ws.open(file, source.into());
    ws.open(OVERLOAD_JAVA, "package typed; class JavaUse { void run(Target target, Left.Value left, Right.Value right) { target.take(left); target.take(right); target.take(new Left.Value()); } }".into());
    for (needle, owner) in [
        ("target.take(left)", "Left"),
        ("target.take(right)", "Right"),
        ("target.take(new Left.Value())", "Left"),
    ] {
        let start_byte = source.find(&format!("take({owner}.Value")).unwrap();
        let offset = ws.doc_text(OVERLOAD_JAVA).unwrap().find(needle).unwrap() + "target.".len();
        assert_eq!(
            ws.goto_definition(OVERLOAD_JAVA, offset),
            vec![Def {
                file: file.into(),
                start_byte,
                end_byte: start_byte + 4
            }],
            "{needle}"
        );
    }
}

#[test]
fn java_widening_and_boxing_candidates_keep_unknown_applicability() {
    use ktcore::members::{Applicability, CallArguments};
    let mut ws = Workspace::new();
    ws.open("/selection/typed/Target.java", "package typed; class Target { void widen(long value) {} void widen(String value) {} void box(Integer value) {} void box(String value) {} void boxRef(java.io.Serializable value) {} void boxRef(String value) {} }".into());
    // Reduced declarations make the reference identities known without implementing conversions.
    ws.open(
        "/selection/java/lang/Integer.java",
        "package java.lang; class Integer {}".into(),
    );
    ws.open(
        "/selection/java/io/Serializable.java",
        "package java.io; interface Serializable {}".into(),
    );
    let receiver = Type::class("Target", Some("typed".into()));
    let ctx = FileCtx::new("typed".into(), Vec::new());
    let args = CallArguments {
        count: Some(1),
        types: vec![Type::class("int", None)],
        names: None,
        has_spread: false,
        trailing_lambda: false,
    };
    for name in ["widen", "box", "boxRef"] {
        let selected = ktcore::jvm::select_call(
            &ws.index,
            &receiver,
            ktcore::jvm::member_candidates_at(&ws.index, &receiver, name, &ctx, 0),
            name,
            &args,
            &ctx,
        );
        assert_eq!(selected.applicability, Applicability::Unknown, "{name}");
        assert_eq!(selected.navigation_candidates().len(), 1, "{name}");
        assert_ne!(selected.candidates[0].params()[0].name, "String");
    }
}

#[test]
fn java_overloaded_jvm_alias_rename_applies_only_to_its_exact_signature() {
    let mut ws = Workspace::new();
    let file = "/selection/typed/Functions.kt";
    let source = "@file:JvmName(\"TypedApi\")\npackage typed\n@JvmName(\"choose\")\nsuspend fun takeInt(value: Int): Int = value\n@JvmName(\"choose\")\nsuspend fun takeText(value: String): String = value\n";
    let java = "package typed; class JavaUse { void run(kotlin.coroutines.Continuation<?> c) { TypedApi.choose(1, c); TypedApi.choose(\"x\", c); TypedApi.choose(missing, c); } }";
    let kotlin = "package typed\nsuspend fun use() { takeInt(1); takeText(\"x\") }\n";
    ws.open(file, source.into());
    ws.open(OVERLOAD_JAVA, java.into());
    ws.open(OVERLOAD_KOTLIN, kotlin.into());
    let offset = java.find("choose(\"x\"").unwrap();
    let declaration = target(&ws, file, "takeText", SymbolKind::Function);
    assert_eq!(
        ws.goto_definition(OVERLOAD_JAVA, offset),
        vec![declaration.clone()]
    );
    let refs = ws.references(OVERLOAD_JAVA, offset, true);
    assert_eq!(refs.len(), 3, "{refs:?}");
    assert!(refs.contains(&declaration));
    let edits = ws.rename(OVERLOAD_JAVA, offset, "chooseText").unwrap();
    assert_eq!(edits.len(), 2, "{edits:?}");
    let changed_source = apply_to_text(file, source, &edits).unwrap();
    let changed_java = apply_to_text(OVERLOAD_JAVA, java, &edits).unwrap();
    assert_eq!(
        changed_source,
        source.replace(
            "@JvmName(\"choose\")\nsuspend fun takeText",
            "@JvmName(\"chooseText\")\nsuspend fun takeText"
        )
    );
    assert_eq!(
        changed_java,
        java.replace("TypedApi.choose(\"x\", c)", "TypedApi.chooseText(\"x\", c)")
    );
    assert_eq!(
        apply_to_text(OVERLOAD_KOTLIN, kotlin, &edits).unwrap(),
        kotlin
    );
    ws.change(file, changed_source);
    ws.change(OVERLOAD_JAVA, changed_java);
    let declaration = target(&ws, file, "takeText", SymbolKind::Function);
    assert_eq!(
        goto(&mut ws, OVERLOAD_JAVA, "TypedApi.chooseText(\"x\", c)"),
        vec![declaration]
    );
}

#[test]
fn overridden_member_has_exact_goto_references_incoming_calls_and_applied_rename() {
    let mut ws = Workspace::new();
    let worker = "/selection/override/Worker.java";
    let helper = "/selection/override/Helper.java";
    let java = "/selection/override/Use.java";
    let kotlin = "/selection/override/Use.kt";
    let worker_source = "package inherited; public interface Worker { void assist(); }";
    let helper_source =
        "package inherited; public class Helper implements Worker { public void assist() {} }";
    let java_source = "package inherited; class Use { void run(Helper helper, Worker worker) { helper.assist(); worker.assist(); } }";
    let kotlin_source = "package inherited\nfun runKt(helper: Helper, worker: Worker) { helper.assist(); worker.assist() }\n";
    for (file, source) in [
        (worker, worker_source),
        (helper, helper_source),
        (java, java_source),
        (kotlin, kotlin_source),
    ] {
        ws.open(file, source.into());
    }
    let declaration = target(&ws, helper, "assist", SymbolKind::Function);
    let base = target(&ws, worker, "assist", SymbolKind::Function);
    for file in [java, kotlin] {
        assert_eq!(
            goto(&mut ws, file, "helper.assist()"),
            vec![declaration.clone()]
        );
        assert_eq!(goto(&mut ws, file, "worker.assist()"), vec![base.clone()]);
    }
    let refs = ws.references(helper, declaration.start_byte, true);
    assert_eq!(refs.len(), 3, "{refs:?}");
    assert!(refs.contains(&declaration));
    let item = ws
        .hierarchy_item_at(helper, declaration.start_byte)
        .unwrap();
    let incoming = ws.incoming_calls(&item);
    assert_eq!(incoming.len(), 2, "{incoming:?}");
    assert!(incoming
        .iter()
        .any(|call| call.from.name == "run" && call.ranges.len() == 1));
    assert!(incoming
        .iter()
        .any(|call| call.from.name == "runKt" && call.ranges.len() == 1));
    let edits = ws
        .rename(helper, declaration.start_byte, "support")
        .unwrap();
    assert_eq!(edits.len(), 6, "{edits:?}");
    assert_eq!(
        apply_to_text(helper, helper_source, &edits).unwrap(),
        helper_source.replace("assist", "support")
    );
    for (file, source) in [(java, java_source), (kotlin, kotlin_source)] {
        assert_eq!(
            apply_to_text(file, source, &edits).unwrap(),
            source.replace(".assist", ".support")
        );
    }
    assert_eq!(
        apply_to_text(worker, worker_source, &edits).unwrap(),
        worker_source.replace("assist", "support")
    );
}

#[test]
fn overridden_member_keeps_different_inherited_overloads_and_unknown_signatures() {
    let mut ws = Workspace::new();
    let base = "/selection/override/Base.java";
    let child = "/selection/override/Child.java";
    let use_file = "/selection/override/Use.java";
    ws.open(base, "package inherited; class Base { void ping(int value) {} void ping(String value) {} void probe(Missing value) {} }".into());
    ws.open(child, "package inherited; class Child extends Base { void ping(int value) {} void probe(String value) {} }".into());
    ws.open(use_file, "package inherited; class Use { void run(Child child) { child.ping(1); child.ping(\"x\"); child.ping(missing); child.probe(\"x\"); } }".into());
    let own = target(&ws, child, "ping", SymbolKind::Function);
    assert_eq!(goto(&mut ws, use_file, "child.ping(1)"), vec![own.clone()]);
    let text = goto(&mut ws, use_file, "child.ping(\"x\")");
    assert_eq!(text.len(), 1, "{text:?}");
    assert_eq!(text[0].file, base);
    let unknown = goto(&mut ws, use_file, "child.ping(missing)");
    assert_eq!(unknown.len(), 2, "{unknown:?}");
    assert!(unknown.contains(&own) && unknown.contains(&text[0]));
    assert_eq!(goto(&mut ws, use_file, "child.probe(\"x\")").len(), 2);
}

#[test]
fn overridden_member_substitutes_generic_owner_without_collapsing_sibling_contracts() {
    let mut ws = Workspace::new();
    let base = "/selection/override/Base.java";
    let child = "/selection/override/Child.java";
    let java = "/selection/override/Use.java";
    ws.open(base, "package inherited; class Base<T> { void take(T value) {} } interface Left { void same(); } interface Right { void same(); } class Both implements Left, Right {}".into());
    ws.open(
        child,
        "package inherited; class Child extends Base<String> { void take(String value) {} }".into(),
    );
    ws.open(java, "package inherited; class Use { void run(Child child, Both both) { child.take(\"x\"); both.same(); } }".into());
    let own = target(&ws, child, "take", SymbolKind::Function);
    assert_eq!(goto(&mut ws, java, "child.take(\"x\")"), vec![own]);
    assert_eq!(goto(&mut ws, java, "both.same()").len(), 2);
}

#[test]
fn overridden_member_normalizes_java_kotlin_primitives_and_preserves_qualified_homonyms() {
    let mut ws = Workspace::new();
    let worker = "/selection/override/Worker.java";
    let child = "/selection/override/Child.kt";
    let java = "/selection/override/Use.java";
    let kotlin = "/selection/override/Use.kt";
    ws.open(worker, "package inherited; interface Worker { void take(int value); void accept(left.Value value); }".into());
    ws.open(child, "package inherited\nabstract class Child: Worker { override fun take(value: Int) {}\nfun accept(value: right.Value) {} }\n".into());
    ws.open(
        "/selection/left/Value.java",
        "package left; class Value {}".into(),
    );
    ws.open(
        "/selection/right/Value.java",
        "package right; class Value {}".into(),
    );
    ws.open(java, "package inherited; class Use { void run(Child child) { child.take(1); child.accept(missing); } }".into());
    ws.open(
        kotlin,
        "package inherited\nfun run(child: Child) { child.take(1); child.accept(missing) }\n"
            .into(),
    );
    let own = target(&ws, child, "take", SymbolKind::Function);
    for file in [java, kotlin] {
        assert_eq!(goto(&mut ws, file, "child.take(1)"), vec![own.clone()]);
        assert_eq!(goto(&mut ws, file, "child.accept(missing)").len(), 2);
    }
}

#[test]
fn overridden_member_keeps_boxed_and_primitive_signatures_distinct() {
    let mut ws = Workspace::new();
    let base = "/selection/override/Base.java";
    let child = "/selection/override/Child.java";
    let java = "/selection/override/Use.java";
    ws.open(base, "package inherited; class Base { void take(java.lang.Long value) {} void flag(java.lang.Boolean value) {} }".into());
    ws.open(child, "package inherited; class Child extends Base { void take(long value) {} void flag(boolean value) {} }".into());
    ws.open(
        "/selection/java/lang/Long.java",
        "package java.lang; class Long {}".into(),
    );
    ws.open(
        "/selection/java/lang/Boolean.java",
        "package java.lang; class Boolean {}".into(),
    );
    ws.open(java, "package inherited; class Use { void run(Child child) { child.take(missing); child.flag(missing); } }".into());
    for call in ["child.take(missing)", "child.flag(missing)"] {
        assert_eq!(goto(&mut ws, java, call).len(), 2, "{call}");
    }
}

#[test]
fn rename_override_family_keeps_overloads_and_unrelated_calls_unchanged() {
    let mut ws = Workspace::new();
    let worker = "/selection/family/Worker.java";
    let child = "/selection/family/Child.java";
    let java = "/selection/family/Use.java";
    let kotlin = "/selection/family/Use.kt";
    let decoy = "/selection/family/Decoy.java";
    let worker_source = "package family; public interface Worker { void assist(); void assist(String value); void assist(int value); }";
    let child_source = "package family; public class Child implements Worker { public void assist() {} public void assist(String value) {} public void assist(int value) {} }";
    let decoy_source = "package family; public class Decoy { public void assist() {} }";
    let java_source = "package family; class Use { void run(Child child, Worker worker, Decoy decoy) { child.assist(); worker.assist(); child.assist(\"x\"); worker.assist(\"x\"); decoy.assist(); } }";
    let kotlin_source = "package family\nfun run(child: Child, worker: Worker, decoy: Decoy) { child.assist(); worker.assist(); child.assist(\"x\"); worker.assist(\"x\"); decoy.assist() }\n";
    for (file, source) in [
        (worker, worker_source),
        (child, child_source),
        (decoy, decoy_source),
        (java, java_source),
        (kotlin, kotlin_source),
    ] {
        ws.open(file, source.into());
    }
    let definition = target(&ws, child, "assist", SymbolKind::Function);
    let edits = ws.rename(child, definition.start_byte, "support").unwrap();
    assert_eq!(edits.len(), 6, "{edits:?}");
    for (file, source) in [
        (worker, worker_source),
        (child, child_source),
        (java, java_source),
        (kotlin, kotlin_source),
        (decoy, decoy_source),
    ] {
        let expected = if file == decoy {
            source.to_string()
        } else if file == java || file == kotlin {
            source
                .replace("child.assist()", "child.support()")
                .replace("worker.assist()", "worker.support()")
        } else {
            source.replace("assist()", "support()")
        };
        assert_eq!(
            apply_to_text(file, source, &edits).unwrap(),
            expected,
            "{file}"
        );
    }
    // Unknown overload calls are not unique enough to start a rename.
    ws.change(
        java,
        java_source.replace("child.assist();", "child.assist(missing);"),
    );
    assert!(ws
        .rename(
            java,
            ws.doc_text(java).unwrap().find("assist(missing)").unwrap(),
            "support"
        )
        .is_none());
}

#[test]
fn rename_override_family_substitutes_generic_parameters_in_both_languages() {
    let mut ws = Workspace::new();
    let worker = "/selection/family/Worker.java";
    let child = "/selection/family/Child.java";
    let java = "/selection/family/Use.java";
    let kotlin = "/selection/family/Use.kt";
    let worker_source = "package family; public interface Worker<T> { void assist(T value); }";
    let child_source = "package family; public class Child implements Worker<String> { public void assist(String value) {} }";
    let java_source = "package family; class Use { void run(Child child, Worker<String> worker) { child.assist(\"x\"); worker.assist(\"x\"); } }";
    let kotlin_source = "package family\nfun run(child: Child, worker: Worker<String>) { child.assist(\"x\"); worker.assist(\"x\") }\n";
    for (file, source) in [
        (worker, worker_source),
        (child, child_source),
        (java, java_source),
        (kotlin, kotlin_source),
    ] {
        ws.open(file, source.into());
    }
    let definition = target(&ws, worker, "assist", SymbolKind::Function);
    let edits = ws.rename(worker, definition.start_byte, "support").unwrap();
    assert_eq!(edits.len(), 6, "{edits:?}");
    for (file, source) in [
        (worker, worker_source),
        (child, child_source),
        (java, java_source),
        (kotlin, kotlin_source),
    ] {
        assert_eq!(
            apply_to_text(file, source, &edits).unwrap(),
            source.replace("assist", "support"),
            "{file}"
        );
    }
}

#[test]
fn rename_override_family_is_transitive_across_java_and_kotlin() {
    let mut ws = Workspace::new();
    let worker = "/selection/family/Worker.java";
    let child = "/selection/family/Child.java";
    let grandchild = "/selection/family/Grandchild.kt";
    let use_file = "/selection/family/Use.kt";
    let worker_source = "package family; public interface Worker { void assist(); }";
    let child_source =
        "package family; public class Child implements Worker { public void assist() {} }";
    let grandchild_source =
        "package family\nclass Grandchild: Child() { override fun assist() {} }\n";
    let use_source = "package family\nfun run(child: Child, grandchild: Grandchild, worker: Worker) { child.assist(); grandchild.assist(); worker.assist() }\n";
    for (file, source) in [
        (worker, worker_source),
        (child, child_source),
        (grandchild, grandchild_source),
        (use_file, use_source),
    ] {
        ws.open(file, source.into());
    }
    let definition = target(&ws, grandchild, "assist", SymbolKind::Function);
    let edits = ws
        .rename(grandchild, definition.start_byte, "support")
        .unwrap();
    assert_eq!(edits.len(), 6, "{edits:?}");
    for (file, source) in [
        (worker, worker_source),
        (child, child_source),
        (grandchild, grandchild_source),
        (use_file, use_source),
    ] {
        assert_eq!(
            apply_to_text(file, source, &edits).unwrap(),
            source.replace("assist", "support"),
            "{file}"
        );
    }
}

#[test]
fn rename_override_family_refuses_inherited_common_descendant_contracts() {
    let mut ws = Workspace::new();
    let base = "/selection/family/Base.java";
    let worker = "/selection/family/Worker.java";
    let child = "/selection/family/Child.java";
    ws.open(
        base,
        "package family; public class Base { public void assist() {} }".into(),
    );
    ws.open(
        worker,
        "package family; public interface Worker { void assist(); }".into(),
    );
    ws.open(
        child,
        "package family; public class Child extends Base implements Worker {}".into(),
    );
    for file in [base, worker] {
        let definition = target(&ws, file, "assist", SymbolKind::Function);
        assert!(
            ws.prepare_rename(file, definition.start_byte).is_none(),
            "{file}"
        );
        assert!(
            ws.rename(file, definition.start_byte, "support").is_none(),
            "{file}"
        );
    }
}

#[test]
fn rename_override_family_refuses_unknown_signatures_cycles_and_read_only_ancestors() {
    for (base_source, child_source, durable) in [
        (
            "package family; class Base { public void assist(Missing value) {} }",
            "package family; class Child extends Base { public void assist(String value) {} }",
            false,
        ),
        (
            "package family; class Base extends Child { public void assist() {} }",
            "package family; class Child extends Base { public void assist() {} }",
            false,
        ),
        (
            "package family; class Base { public void assist() {} }",
            "package family; class Child extends Base { public void assist() {} }",
            true,
        ),
    ] {
        let mut ws = Workspace::new();
        let base = "/selection/family/Base.java";
        let child = "/selection/family/Child.java";
        ws.open(base, base_source.into());
        ws.open(child, child_source.into());
        if durable {
            let tree = JavaParser::new().parse(base_source);
            ws.index.replace_file(
                base,
                java::extract_symbols(&tree, base_source),
                Tier::Durable,
            );
        }
        let definition = target(&ws, child, "assist", SymbolKind::Function);
        assert!(
            ws.prepare_rename(child, definition.start_byte).is_none(),
            "{base_source}"
        );
        assert!(
            ws.rename(child, definition.start_byte, "support").is_none(),
            "{base_source}"
        );
    }
}

#[test]
fn java_bare_member_call_resolves_before_same_package_decoys() {
    let mut ws = Workspace::new();
    let base = "/selection/bare/Base.java";
    let child = "/selection/bare/Child.java";
    let decoy = "/selection/bare/Decoy.java";
    let child_source = "package bare; public class Child extends Base { public static int assist() { return 2; } public static int run() { return assist(); } }";
    ws.open(
        base,
        "package bare; public class Base { public static int assist() { return 1; } }".into(),
    );
    ws.open(child, child_source.into());
    ws.open(
        decoy,
        "package bare; public class Decoy { public static int assist() { return 3; } }".into(),
    );
    let definition = target(&ws, child, "assist", SymbolKind::Function);
    let offset = child_source.find("return assist()").unwrap() + "return ".len();
    assert_eq!(ws.goto_definition(child, offset), vec![definition.clone()]);
    assert_eq!(
        ws.references(child, definition.start_byte, true),
        vec![
            definition,
            Def {
                file: child.into(),
                start_byte: offset,
                end_byte: offset + "assist".len(),
            }
        ]
    );
}

#[test]
fn rename_bare_java_static_call_preserves_hidden_parent_and_decoy() {
    let mut ws = Workspace::new();
    let base = "/selection/bare/Base.java";
    let child = "/selection/bare/Child.java";
    let decoy = "/selection/bare/Decoy.java";
    let java = "/selection/bare/JavaUse.java";
    let kotlin = "/selection/bare/KotlinUse.kt";
    let sources = [
        (
            base,
            "package bare; public class Base { public static int assist() { return 1; } }",
        ),
        (
            child,
            "package bare; public class Child extends Base { public static int assist() { return 2; } public static int run() { return assist(); } }",
        ),
        (
            decoy,
            "package bare; public class Decoy { public static int assist() { return 3; } }",
        ),
        (
            java,
            "package bare; public class JavaUse { public static int run() { return Child.assist(); } }",
        ),
        (kotlin, "package bare\nfun run(): Int = Child.assist()\n"),
    ];
    for (file, source) in sources {
        ws.open(file, source.into());
    }
    let definition = target(&ws, child, "assist", SymbolKind::Function);
    let edits = ws.rename(child, definition.start_byte, "support").unwrap();
    assert_eq!(edits.len(), 4, "{edits:?}");
    for (file, source) in sources {
        let expected = if file == base || file == decoy {
            source.into()
        } else {
            source.replace("assist", "support")
        };
        assert_eq!(
            apply_to_text(file, source, &edits).unwrap(),
            expected,
            "{file}"
        );
    }
}

#[test]
fn java_bare_calls_share_typed_selection_for_navigation_signature_and_result() {
    let (mut ws, file) = overload_workspace(true);
    let source = "package typed; public class Target { public NumberResult twice(int value) { return null; } public TextResult twice(String value) { return null; } void run() { twice(1).numberOnly(); twice(\"x\").textOnly(); twice(missing); } }";
    ws.change(file, source.into());
    for (call, parameter, result) in [
        ("twice(1)", "int", "numberOnly"),
        ("twice(\"x\")", "String", "textOnly"),
    ] {
        let definition = overload_target(&ws, file, parameter);
        let offset = source.find(call).unwrap();
        assert_eq!(ws.goto_definition(file, offset), vec![definition]);
        let help = ws.signature_help(file, offset + "twice(".len()).unwrap();
        assert_eq!(help.signatures.len(), 1, "{help:?}");
        assert!(help.signatures[0].label.contains(parameter), "{help:?}");
        let completion = labels(&mut ws, file, &format!("{call}."));
        assert!(completion.contains(result), "{completion:?}");
        assert!(!completion.contains(if result == "numberOnly" {
            "textOnly"
        } else {
            "numberOnly"
        }));
    }
    let mut actual = ws.goto_definition(file, source.find("twice(missing)").unwrap());
    actual.sort();
    let mut expected = vec![
        overload_target(&ws, file, "int"),
        overload_target(&ws, file, "String"),
    ];
    expected.sort();
    assert_eq!(actual, expected);
}

#[test]
fn rename_override_family_does_not_join_static_or_private_methods() {
    for modifier in ["public static", "private"] {
        let mut ws = Workspace::new();
        let base = "/selection/family/Base.java";
        let child = "/selection/family/Child.java";
        let base_source = format!("package family; class Base {{ {modifier} void assist() {{}} }}");
        let receiver = if modifier == "private" {
            "this"
        } else {
            "Child"
        };
        let child_source = format!(
            "package family; class Child extends Base {{ {modifier} void assist() {{}} void run() {{ {receiver}.assist(); }} }}"
        );
        ws.open(base, base_source.clone());
        ws.open(child, child_source.clone());
        let definition = target(&ws, child, "assist", SymbolKind::Function);
        let edits = ws.rename(child, definition.start_byte, "support").unwrap();
        assert_eq!(edits.len(), 2, "{modifier}: {edits:?}");
        assert_eq!(
            apply_to_text(base, &base_source, &edits).unwrap(),
            base_source
        );
        assert_eq!(
            apply_to_text(child, &child_source, &edits).unwrap(),
            child_source.replace("assist", "support")
        );
    }
}

#[test]
fn rename_override_family_preserves_module_and_package_identity() {
    let mut ws = Workspace::new();
    let worker = "/selection/current/Worker.java";
    let old_worker = "/selection/old/Worker.java";
    let child = "/selection/current/Child.java";
    let decoy = "/selection/other/Worker.java";
    let worker_source = "package family; public interface Worker { void assist(); }";
    let child_source =
        "package family; public class Child implements Worker { public void assist() {} }";
    let decoy_source = "package other; public interface Worker { void assist(); }";
    for (file, source, module) in [
        (worker, worker_source, "current"),
        (child, child_source, "current"),
        (old_worker, worker_source, "old"),
        (decoy, decoy_source, "current"),
    ] {
        ws.open(file, source.into());
        ws.index.set_file_origin(file, origin(module, Some(module)));
    }
    ws.index.replace_scope_visibilities(
        ["current", "old"]
            .into_iter()
            .map(|module| ScopeVisibility {
                scope: origin(module, Some(module)).project_scope.unwrap(),
                ordered_origins: vec![module.into()],
            })
            .collect(),
    );
    ws.bump_index_revision();
    let definition = target(&ws, child, "assist", SymbolKind::Function);
    let edits = ws.rename(child, definition.start_byte, "support").unwrap();
    assert_eq!(edits.len(), 2, "{edits:?}");
    for (file, source) in [(worker, worker_source), (child, child_source)] {
        assert_eq!(
            apply_to_text(file, source, &edits).unwrap(),
            source.replace("assist", "support"),
            "{file}"
        );
    }
    assert_eq!(
        apply_to_text(old_worker, worker_source, &edits).unwrap(),
        worker_source
    );
    assert_eq!(
        apply_to_text(decoy, decoy_source, &edits).unwrap(),
        decoy_source
    );
}

#[test]
fn rename_unrelated_object_and_suspend_members_keeps_ordinary_exact_edits() {
    for declaration in [
        "object Worker { fun assist() {} }",
        "class Worker { suspend fun assist() {} }",
    ] {
        let mut ws = Workspace::new();
        let worker = "/selection/family/Worker.kt";
        let first = "/selection/family/First.kt";
        let second = "/selection/family/Second.kt";
        let worker_source = format!("package family\n{declaration}\n");
        let use_source = if declaration.starts_with("object") {
            "package family\nfun run() { Worker.assist() }\n"
        } else {
            "package family\nsuspend fun run(worker: Worker) { worker.assist() }\n"
        };
        ws.open(worker, worker_source.clone());
        ws.open(first, use_source.into());
        ws.open(second, use_source.replace("fun run", "fun other"));
        let definition = target(&ws, worker, "assist", SymbolKind::Function);
        let edits = ws.rename(worker, definition.start_byte, "support").unwrap();
        assert_eq!(edits.len(), 3, "{declaration}: {edits:?}");
        for file in [worker, first, second] {
            let source = ws.doc_text(file).unwrap();
            assert_eq!(
                apply_to_text(file, &source, &edits).unwrap(),
                source.replace("assist", "support"),
                "{file}"
            );
        }
    }
}

#[test]
fn rename_override_family_refuses_ambiguous_calls_in_either_language() {
    let mut ws = Workspace::new();
    let worker = "/selection/family/Worker.java";
    let child = "/selection/family/Child.java";
    let java = "/selection/family/Use.java";
    let kotlin = "/selection/family/Use.kt";
    let worker_source = "package family; public interface Worker { void assist(String value); void assist(int value); }";
    let child_source = "package family; public class Child implements Worker { public void assist(String value) {} public void assist(int value) {} }";
    let java_source = "package family; class Use { void run(Child child, Decoy decoy) { child.assist(missing); decoy.assist(unrelated); } }";
    let kotlin_source = "package family\nfun run(child: Child, decoy: Decoy) { child.assist(missing); decoy.assist(unrelated) }\n";
    ws.open(
        "/selection/family/Decoy.java",
        "package family; class Decoy { void assist(String value) {} void assist(int value) {} }"
            .into(),
    );
    for (file, source) in [
        (worker, worker_source),
        (child, child_source),
        (java, java_source),
        (kotlin, kotlin_source),
    ] {
        ws.open(file, source.into());
    }
    let definition = target(&ws, child, "assist", SymbolKind::Function);
    assert_eq!(goto(&mut ws, java, "child.assist(missing)").len(), 2);
    assert_eq!(goto(&mut ws, kotlin, "child.assist(missing)").len(), 2);
    assert!(ws.rename(child, definition.start_byte, "support").is_none());
    ws.change(java, java_source.replace("missing", "\"x\""));
    assert!(ws.rename(child, definition.start_byte, "support").is_none());
    ws.change(kotlin, kotlin_source.replace("missing", "\"x\""));
    assert_eq!(goto(&mut ws, java, "decoy.assist(unrelated)").len(), 2);
    assert_eq!(goto(&mut ws, kotlin, "decoy.assist(unrelated)").len(), 2);
    let edits = ws.rename(child, definition.start_byte, "support").unwrap();
    assert_eq!(edits.len(), 4, "{edits:?}");
    for (file, source) in [(worker, worker_source), (child, child_source)] {
        assert_eq!(
            apply_to_text(file, source, &edits).unwrap(),
            source.replace("assist(String", "support(String")
        );
    }
    for file in [java, kotlin] {
        let source = ws.doc_text(file).unwrap();
        assert_eq!(
            apply_to_text(file, &source, &edits).unwrap(),
            source.replace("child.assist", "child.support")
        );
    }
}

#[test]
fn rename_override_family_refuses_edits_to_read_only_usage_files() {
    let mut ws = Workspace::new();
    let worker = "/selection/family/Worker.java";
    let child = "/selection/family/Child.java";
    let library = "/selection/library/Use.java";
    let library_source = "package family; class Use { void run(Child child) { child.assist(); } }";
    ws.open(
        worker,
        "package family; interface Worker { void assist(); }".into(),
    );
    ws.open(
        child,
        "package family; class Child implements Worker { public void assist() {} }".into(),
    );
    ws.open(library, library_source.into());
    let tree = JavaParser::new().parse(library_source);
    ws.index.replace_file(
        library,
        java::extract_symbols(&tree, library_source),
        Tier::Durable,
    );
    let definition = target(&ws, child, "assist", SymbolKind::Function);
    assert_eq!(ws.references(child, definition.start_byte, true).len(), 2);
    assert!(ws.rename(child, definition.start_byte, "support").is_none());
}

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

#[test]
fn receiver_storage_preserves_inline_symbol_cache_wire() {
    use ktlsp::symbol::{ExtensionReceiverFacts, IndexedSymbol};
    use ktlsp::types::TypeRef;
    use serde::{Deserialize, Serialize};
    use std::sync::Arc;

    // Current cache schema: boxed receivers encode like inline options, followed by source flags.
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct InlineSymbol {
        name: String,
        kind: SymbolKind,
        package: String,
        container: Option<String>,
        start_byte: usize,
        end_byte: usize,
        documentation: Option<String>,
        supertypes: Vec<String>,
        supertype_refs: Vec<TypeRef>,
        unavailable_members: Option<Vec<(String, SymbolKind, Option<u8>)>>,
        is_annotation: bool,
        ext_receiver: Option<String>,
        extension_receiver: Option<ExtensionReceiverFacts>,
        arity: Option<u8>,
        min_arity: Option<u8>,
        has_vararg: bool,
        trailing_lambda_min_arity: Option<u8>,
        last_parameter_min_arity: Option<u8>,
        trailing_lambda_receiver_type: Option<TypeRef>,
        function_type_receiver: Option<TypeRef>,
        return_type: Option<TypeRef>,
        value_type: Option<TypeRef>,
        params: Vec<TypeRef>,
        kotlin_parameters: Option<Vec<ktlsp::symbol::KotlinValueParameter>>,
        type_params: Vec<String>,
        type_parameter_facts: ktlsp::symbol::TypeParameterFacts,
    }

    let empty = IndexedSymbol::new("Empty", SymbolKind::Class, "api", None, 0, 5);
    let mut rich = IndexedSymbol::new(
        "handler",
        SymbolKind::Function,
        "api",
        Some("Owner".into()),
        12,
        19,
    );
    let mut receiver = TypeRef::simple("Receiver");
    receiver.qualification = Some("other".into());
    receiver.nullable = true;
    receiver.package_candidates = Arc::new(vec!["api".into(), "wildcard".into(), "api".into()]);
    receiver.container_candidates = vec!["Outer".into()];
    receiver.args = vec![TypeRef::simple("Payload")];
    rich.documentation = Some("receiver metadata".into());
    rich.supertypes = vec!["Base".into()];
    rich.supertype_refs = vec![receiver.clone()];
    rich.unavailable_members = Some(Box::new(vec![("f".into(), SymbolKind::Function, Some(0))]));
    rich.is_annotation = true;
    rich.ext_receiver = Some("Owner".into());
    rich.extension_receiver = Some(Box::new(ExtensionReceiverFacts {
        ty: receiver.clone(),
        upper_bounds: vec![("Payload".into(), receiver.clone())],
        has_type_constraints: true,
    }));
    rich.arity = Some(3);
    rich.min_arity = Some(1);
    rich.has_vararg = true;
    rich.trailing_lambda_min_arity = Some(0);
    rich.last_parameter_min_arity = Some(1);
    rich.trailing_lambda_receiver_type = Some(Box::new(receiver.clone()));
    rich.function_type_receiver = Some(Box::new(receiver.clone()));
    rich.return_type = Some(receiver.clone());
    rich.value_type = Some(receiver.clone());
    rich.params = vec![receiver];
    rich.kotlin_parameters = Some(Box::new(vec![ktlsp::symbol::KotlinValueParameter {
        name: "value".into(),
        has_default: true,
    }]));
    rich.type_params = vec!["Payload".into()];
    rich.type_parameter_facts = ktlsp::symbol::TypeParameterFacts::new(true, false);
    rich.method_type_bounds = Some(Box::new(vec![("Payload".into(), TypeRef::simple("Known"))]));
    let source = "package api; public class Owner { public native void method(); }";
    let tree = ktlsp::java::JavaParser::new().parse(source);
    rich.source_member = ktlsp::java::extract_symbols(&tree, source)
        .into_iter()
        .find(|symbol| symbol.name == "method")
        .unwrap()
        .source_member;

    for symbol in [empty, rich] {
        let mut value = serde_json::to_value(&symbol).unwrap();
        value.as_object_mut().unwrap().remove("source_member");
        value.as_object_mut().unwrap().remove("method_type_bounds");
        let inline: InlineSymbol = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&inline).unwrap(), value);
        let inline_bytes = bincode::serialize(&inline).unwrap();
        let current_bytes = bincode::serialize(&symbol).unwrap();
        assert_eq!(inline_bytes, current_bytes[..inline_bytes.len()]);
        assert_eq!(
            current_bytes.len(),
            inline_bytes.len()
                + 1
                + bincode::serialize(&symbol.method_type_bounds)
                    .unwrap()
                    .len()
        );
        assert_eq!(
            bincode::deserialize::<IndexedSymbol>(&current_bytes).unwrap(),
            symbol
        );
        let mut absent_names = value.clone();
        absent_names
            .as_object_mut()
            .unwrap()
            .remove("kotlin_parameters");
        assert!(serde_json::from_value::<IndexedSymbol>(absent_names)
            .unwrap()
            .kotlin_parameters
            .is_none());
        let mut absent_facts = value.clone();
        absent_facts
            .as_object_mut()
            .unwrap()
            .remove("type_parameter_facts");
        let decoded: IndexedSymbol = serde_json::from_value(absent_facts).unwrap();
        assert!(!decoded.type_parameter_facts.known_unbounded());
        assert!(!decoded.type_parameter_facts.known_invariant());
        let mut defaulted = symbol.clone();
        defaulted.source_member = Default::default();
        defaulted.method_type_bounds = None;
        assert_eq!(
            serde_json::from_value::<IndexedSymbol>(value).unwrap(),
            defaulted
        );
    }
}

fn assert_known_leaf_binary_override(java_caller: bool) {
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    let decoded = ktcore::classfile::decode(
        "fixture:shadow",
        include_bytes!("../../ktcore/tests/fixtures/classfile/shadowfixture/GenericMid.class"),
    )
    .unwrap();
    let unknown = decoded
        .methods
        .iter()
        .find(|method| method.name.as_ref() == "f" && method.access_flags & 0x1040 == 0)
        .unwrap();
    assert_eq!(unknown.descriptor.as_ref(), "()Ljava/util/List;");
    assert!(unknown.signature.is_some());
    assert_eq!(unknown.access_flags, 0x0001);
    let root = std::env::temp_dir().join(format!(
        "ktlsp-known-leaf-{}-{java_caller}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let jar = root.join("shadow.jar");
    let mut archive = zip::ZipWriter::new(std::fs::File::create(&jar).unwrap());
    for (name, bytes) in [
        (
            "Base",
            include_bytes!("../../ktcore/tests/fixtures/classfile/shadowfixture/Base.class")
                .as_slice(),
        ),
        (
            "GenericMid",
            include_bytes!("../../ktcore/tests/fixtures/classfile/shadowfixture/GenericMid.class")
                .as_slice(),
        ),
        (
            "NestedBoundMid",
            include_bytes!(
                "../../ktcore/tests/fixtures/classfile/shadowfixture/NestedBoundMid.class"
            )
            .as_slice(),
        ),
    ] {
        archive
            .start_file(
                format!("shadowfixture/{name}.class"),
                SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(bytes).unwrap();
    }
    archive.finish().unwrap();
    const LEAF: &str = "/selection/shadow/KnownLeaf.java";
    // This exact source is accepted by the JDK21 javac/javap compiler control.
    const LEAF_SOURCE: &str = r#"package shadowfixture;
public final class KnownLeaf extends GenericMid {
    @Override public TypedList<String> f() { return new TypedList<>(); }
}
"#;
    // Real fixture-owned covariance keeps the return type indexed without inventing SDK facts.
    const TYPED: &str = "/selection/shadow/TypedList.java";
    const TYPED_SOURCE: &str = r#"package shadowfixture;
public final class TypedList<T> extends shadowfixture.Container.NestedList<T> {
    @Override public T get(int index) { throw new IndexOutOfBoundsException(); }
    @Override public int size() { return 0; }
    public int ownSize() { return size(); }
}
"#;
    let mut observed = Vec::new();
    for ancestor in ["none", "Base", "GenericMid", "NestedBoundMid"] {
        for repeated_load in [false, true] {
            let batches = ktlsp::deps::resolve_local_jar_stubs(
                &jar,
                &root.join("extract"),
                &mut KotlinParser::new(),
                &mut JavaParser::new(),
            )
            .unwrap();
            assert!(batches
                .iter()
                .all(|batch| batch.origin.as_ref().unwrap().representation
                    == SourceRepresentation::Stub));
            assert!(batches
                .iter()
                .flat_map(|batch| &batch.symbols)
                .any(|owner| owner.name == "NestedBoundMid"
                    && owner
                        .unavailable_members
                        .as_deref()
                        .is_some_and(|names| names.iter().any(|(name, kind, arity)| name == "f"
                            && *kind == SymbolKind::Function
                            && *arity == Some(0)))));
            let base_owner = batches
                .iter()
                .flat_map(|batch| &batch.symbols)
                .find(|symbol| symbol.name == "Base" && symbol.kind.is_type_like())
                .unwrap();
            let base_f_unavailable = base_owner
                .unavailable_members
                .as_deref()
                .unwrap()
                .iter()
                .any(|(name, kind, _)| name == "f" && *kind == SymbolKind::Function);
            let base_f_supported = batches
                .iter()
                .flat_map(|batch| &batch.symbols)
                .any(|symbol| {
                    symbol.name == "f"
                        && symbol.kind == SymbolKind::Function
                        && symbol.container.as_deref() == Some("Base")
                });
            assert!(
                base_f_supported && !base_f_unavailable,
                "actual Base facts: f supported={base_f_supported}, unavailable={base_f_unavailable}"
            );
            let binary_origin = batches[0].origin.as_ref().unwrap().origin_id.clone();
            let mut ws = Workspace::new();
            for batch in batches {
                assert!(ws.index_dependency_batch(batch));
            }
            ws.open(
                LEAF,
                match ancestor {
                    "none" => LEAF_SOURCE
                        .replace(" extends GenericMid", "")
                        .replace("@Override ", ""),
                    "Base" => LEAF_SOURCE.replace("extends GenericMid", "extends Base"),
                    "NestedBoundMid" => {
                        LEAF_SOURCE.replace("extends GenericMid", "extends NestedBoundMid")
                    }
                    _ => LEAF_SOURCE.to_string(),
                },
            );
            let (caller, source) = if java_caller {
                (
                    "/selection/shadow/Use.java",
                    "package shadowfixture; class Use { void use(KnownLeaf value) { var result = value.f(); value.f().size(); } }",
                )
            } else {
                (
                    "/selection/shadow/Use.kt",
                    "package shadowfixture
fun use(value: KnownLeaf) { val result = value.f(); value.f().size }
",
                )
            };
            ws.open(caller, source.into());
            ws.open(TYPED, TYPED_SOURCE.into());
            for path in [LEAF, caller, TYPED] {
                ws.index.set_file_origin(path, origin("app", Some("app")));
            }
            ws.index.replace_scope_visibilities(vec![ScopeVisibility {
                scope: origin("app", Some("app")).project_scope.unwrap(),
                ordered_origins: vec!["app".into(), binary_origin],
            }]);
            ws.bump_index_revision();
            let expected = target(&ws, LEAF, "f", SymbolKind::Function);
            let completion = labels(&mut ws, caller, "value.f");
            let definitions = ws.goto_definition(caller, source.find("f()").unwrap());
            let hint_labels: Vec<_> = ws
                .inlay_hints(caller, 0, source.len())
                .iter()
                .map(|hint| hint.label.clone())
                .collect();
            let chain = labels(&mut ws, caller, "value.f().size");
            let raw_type = if java_caller {
                None
            } else {
                Some(infer_expression(&ws, caller, "value.f()"))
            };
            observed.push((
                ancestor,
                repeated_load,
                completion.contains("f"),
                definitions == vec![expected],
                hint_labels
                    .iter()
                    .any(|label| label == ": TypedList<String>"),
                definitions,
                hint_labels,
                chain.contains("size"),
                raw_type,
            ));
        }
    }
    std::fs::remove_dir_all(&root).unwrap();
    assert!(
        observed.iter().all(
            |(_, _, completion, target, inference, _, _, chain, _)| *completion
                && *target
                && *inference
                && *chain
        ),
        "known nearest override and no-ancestry control must remain useful on initial/repeated loads: {observed:?}"
    );
}

#[test]
fn binary_known_leaf_override_java() {
    assert_known_leaf_binary_override(true);
}
#[test]
fn binary_known_leaf_override_kotlin() {
    assert_known_leaf_binary_override(false);
}

#[test]
fn jvm_string_return_chains_use_caller_identity_without_changing_foreign_strings() {
    const KOTLIN: &str = "/selection/return-chain/Use.kt";
    const JAVA: &str = "/selection/return-chain/Use.java";
    for java_string_available in [false, true] {
        let mut ws = Workspace::new();
        ws.open(
            "/selection/return-chain/KotlinTypes.kt",
            "package kotlin\nclass Int\nclass String { fun kotlinOnly() {}\nfun get(index: Int): String = this\nfun plus(other: String): String = this }".into(),
        );
        ws.open(
            "/selection/return-chain/KotlinText.kt",
            "package kotlin.text\nfun String.substring(begin: Int): String = this".into(),
        );
        if java_string_available {
            ws.open("/selection/return-chain/jdk/String.java", "package java.lang; public final class String { public String substring(int begin) { return this; } }".into());
        }
        ws.open("/selection/return-chain/AtomicInteger.java", "package java.util.concurrent.atomic; public class AtomicInteger { public int incrementAndGet() { return 0; } }".into());
        ws.open(
            "/selection/return-chain/foreign/String.java",
            "package foreign; public class String { public void foreignOnly() {} }".into(),
        );
        ws.open(
            "/selection/return-chain/JBox.java",
            "package com.squareup.moshi; public class JBox<T> { public T get() { return null; } }"
                .into(),
        );
        ws.open("/selection/return-chain/JsonReader.java", "package com.squareup.moshi; public abstract class JsonReader { public String ktlspSliceText() { return \"slice\"; } public java.util.concurrent.atomic.AtomicInteger ktlspSliceCounter() { return null; } public foreign.String ktlspOther() { return null; } public JBox<String> ktlspGeneric() { return null; } }".into());
        ws.open("/selection/return-chain/OkHttpClient.kt", "package okhttp3\nimport foreign.String as ForeignText\nclass KBox<T> { fun get(): T = TODO() }\nclass OkHttpClient { fun ktlspSliceText(): String = \"slice\"\nfun ktlspSliceCounter(): java.util.concurrent.atomic.AtomicInteger = TODO()\nfun ktlspOther(): ForeignText = TODO()\nfun ktlspGeneric(): KBox<String> = TODO() }".into());
        ws.open(KOTLIN, "package reduction\nimport com.squareup.moshi.JsonReader\nfun echo(value: String): String = value\nfun echo(value: foreign.String): java.util.concurrent.atomic.AtomicInteger = TODO()\nfun use(provider: JsonReader, text: String, counter: java.util.concurrent.atomic.AtomicInteger) { text.substring(0); val localText: String = \"slice\"; localText.substring(0); provider.ktlspSliceText().substring(0); counter.incrementAndGet(); provider.ktlspSliceCounter().incrementAndGet(); provider.ktlspOther().foreignOnly(); provider.ktlspGeneric().get().substring(0); val assignedText: String = provider.ktlspSliceText(); echo(text).substring(0); echo(provider.ktlspSliceText()).substring(0); echo(assignedText).substring(0) }".into());
        for expression in [
            "text.sub",
            "localText.sub",
            "provider.ktlspSliceText().sub",
            "provider.ktlspGeneric().get().sub",
            "echo(text).sub",
            "echo(provider.ktlspSliceText()).sub",
            "echo(assignedText).sub",
        ] {
            assert!(
                labels(&mut ws, KOTLIN, expression).contains("substring"),
                "{java_string_available}: {expression}"
            );
        }
        assert_eq!(
            infer_expression(&ws, KOTLIN, "provider.ktlspSliceText()"),
            Type::class("String", Some("kotlin".into()))
        );
        let generic = infer_expression(&ws, KOTLIN, "provider.ktlspGeneric()");
        assert_eq!(generic.name(), Some("JBox"));
        assert_eq!(generic.args().len(), 1);
        assert_eq!(generic.args()[0].name(), Some("String"));
        for expression in ["counter.inc", "provider.ktlspSliceCounter().inc"] {
            assert!(
                labels(&mut ws, KOTLIN, expression).contains("incrementAndGet"),
                "{expression}"
            );
        }
        assert!(labels(&mut ws, KOTLIN, "provider.ktlspOther().foreign").contains("foreignOnly"));
        assert!(!labels(&mut ws, KOTLIN, "provider.ktlspOther().").contains("substring"));
        assert!(!labels(&mut ws, KOTLIN, "provider.ktlspSliceText().").contains("incrementAndGet"));
        assert!(!labels(&mut ws, KOTLIN, "provider.ktlspSliceCounter().").contains("substring"));
        let expected = target(
            &ws,
            "/selection/return-chain/KotlinText.kt",
            "substring",
            SymbolKind::Function,
        );
        assert_eq!(
            goto(&mut ws, KOTLIN, "provider.ktlspSliceText().substring"),
            vec![expected]
        );
        if java_string_available {
            ws.open(JAVA, "package reduction; import okhttp3.OkHttpClient; class Use { String echo(String text) { return text; } java.util.concurrent.atomic.AtomicInteger echo(foreign.String text) { return null; } void use(OkHttpClient provider, String text, java.util.concurrent.atomic.AtomicInteger counter) { text.substring(0); provider.ktlspSliceText().substring(0); counter.incrementAndGet(); provider.ktlspSliceCounter().incrementAndGet(); provider.ktlspOther().foreignOnly(); provider.ktlspGeneric().get().substring(0); String assignedText = provider.ktlspSliceText(); echo(text).substring(0); echo(provider.ktlspSliceText()).substring(0); echo(assignedText).substring(0); } }".into());
            for expression in [
                "text.sub",
                "provider.ktlspSliceText().sub",
                "provider.ktlspGeneric().get().sub",
                "echo(text).sub",
                "echo(provider.ktlspSliceText()).sub",
                "echo(assignedText).sub",
            ] {
                assert!(
                    labels(&mut ws, JAVA, expression).contains("substring"),
                    "{expression}"
                );
            }
            for expression in ["counter.inc", "provider.ktlspSliceCounter().inc"] {
                assert!(
                    labels(&mut ws, JAVA, expression).contains("incrementAndGet"),
                    "{expression}"
                );
            }
            assert!(labels(&mut ws, JAVA, "provider.ktlspOther().foreign").contains("foreignOnly"));
            assert!(!labels(&mut ws, JAVA, "provider.ktlspOther().").contains("substring"));
            assert!(
                !labels(&mut ws, JAVA, "provider.ktlspSliceText().").contains("incrementAndGet")
            );
            assert!(!labels(&mut ws, JAVA, "provider.ktlspSliceCounter().").contains("substring"));
            let mapped = ktcore::jvm::string_result_for_call_mode(
                &ws.index,
                Type::class("String", Some("kotlin".into())).with_nullability(true),
                &FileCtx::new("reduction".into(), Vec::new()),
                ktcore::members::CallMode::Java,
            );
            assert_eq!(
                mapped,
                Type::class("String", Some("java.lang".into())).with_nullability(true)
            );
            let expected = target(
                &ws,
                "/selection/return-chain/jdk/String.java",
                "substring",
                SymbolKind::Function,
            );
            assert_eq!(
                goto(&mut ws, JAVA, "provider.ktlspSliceText().substring"),
                vec![expected]
            );
        } else {
            // A missing Java API remains a coverage gap, without advertising Kotlin-only methods.
            ws.open(JAVA, "package reduction; import okhttp3.OkHttpClient; class Use { void missing(OkHttpClient provider) { provider.ktlspSliceText().get(0); provider.ktlspSliceText().plus(null); } }".into());
            assert!(labels(&mut ws, JAVA, "provider.ktlspS").contains("ktlspSliceText"));
            let missing = labels(&mut ws, JAVA, "provider.ktlspSliceText().");
            assert!(!missing.contains("get") && !missing.contains("plus"));
        }
    }
}
