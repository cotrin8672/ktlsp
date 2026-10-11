//! Shared generic receiver inference through Java and Kotlin declaration facts.

use std::collections::HashSet;

use ktlsp::infer::{self, FileCtx};
use ktlsp::java::{self, JavaParser};
use ktlsp::parser::KotlinParser;
use ktlsp::symbol::SymbolKind;
use ktlsp::types::Type;
use ktlsp::workspace::Workspace;

const ITEM: &str = "package app\nclass Item {\n fun useful() {}\n}\n";
const OTHER: &str = "package app\nclass Other {\n fun usefulWrong() {}\n}\n";
const JAVA_BOX: &str = "package app; public class Box<T> { public T get() { return null; } }";
const KOTLIN_BOX: &str =
    "package app\nclass KotlinBox<T> {\n fun get(): T = error(\"fixture\")\n}\n";
const IMPORTED_BASE: &str = "package base; public class Base { public void baseOnly() {} public void accept(String value) {} }";
const WRONG_BASE: &str = "package app; public class Base { public void baseWrong() {} public void accept(int value) {} }";
const JAVA_CHILD: &str = "package app; import base.Base; public class Child extends Base {}";
const KOTLIN_CHILD: &str = "package app\nimport base.Base\nclass KotlinChild : Base()\n";
const JAVA_CHILD_USE: &str = "package app; class Use { void run(Child child, KotlinChild kotlin) { child.baseOnly(); child.accept(\"ok\"); kotlin.baseOnly(); kotlin.accept(\"ok\"); } }";
const KOTLIN_CHILD_USE: &str = "package app\nfun run(child: Child, kotlin: KotlinChild) { child.baseOnly(); child.accept(\"ok\"); kotlin.baseOnly(); kotlin.accept(\"ok\") }\n";

fn imported_superclass_workspace() -> Workspace {
    workspace(&[
        ("base/Base.java", IMPORTED_BASE),
        ("app/Base.java", WRONG_BASE),
        ("app/Child.java", JAVA_CHILD),
        ("app/KotlinChild.kt", KOTLIN_CHILD),
        ("app/Use.java", JAVA_CHILD_USE),
        ("app/Use.kt", KOTLIN_CHILD_USE),
    ])
}

fn workspace(files: &[(&str, &str)]) -> Workspace {
    let mut ws = Workspace::new();
    for (file, src) in files {
        ws.open(*file, (*src).to_string());
    }
    ws
}

fn labels(ws: &mut Workspace, file: &str, prefix: &str) -> HashSet<String> {
    let src = ws.doc_text(file).expect("open fixture");
    let offset = src.find(prefix).expect("cursor prefix") + prefix.len();
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

fn assert_member(ws: &mut Workspace, file: &str, prefix: &str, expected: &str, absent: &str) {
    let got = labels(ws, file, prefix);
    assert!(
        got.contains(expected),
        "{file} {prefix}: expected {expected}, got {got:?}"
    );
    assert!(
        !got.contains(absent),
        "{file} {prefix}: leaked {absent}: {got:?}"
    );
}

fn infer_expression(ws: &Workspace, file: &str, expression: &str) -> Type {
    let src = ws.doc_text(file).expect("open Kotlin fixture");
    let mut parser = KotlinParser::new();
    let tree = parser.parse(&src);
    let start = src.find(expression).expect("expression");
    let node = tree
        .root_node()
        .descendant_for_byte_range(start, start + expression.len())
        .unwrap();
    assert_eq!(&src[node.byte_range()], expression);
    infer::infer(&ws.index, node, &src, &FileCtx::from_tree(&tree, &src))
}

#[test]
fn java_declarations_keep_type_parameters_and_structured_type_arguments() {
    let src = "package app; import model.Item; public class Box<T> { public <U> U pick(U value) { return value; } public Box<Item> make() { return null; } public Box<model.Outer.Inner> nested; }";
    let tree = JavaParser::new().parse(src);
    let symbols = java::extract_symbols(&tree, src);
    let cached = bincode::serialize(&symbols).unwrap();
    let decoded: Vec<ktlsp::symbol::IndexedSymbol> = bincode::deserialize(&cached).unwrap();
    assert_eq!(
        symbols, decoded,
        "generic facts must survive the source-symbol cache"
    );
    let class = symbols
        .iter()
        .find(|s| s.kind == SymbolKind::Class)
        .unwrap();
    assert_eq!(class.type_params, ["T"]);
    let method = symbols.iter().find(|s| s.name == "pick").unwrap();
    assert_eq!(method.type_params, ["U"]);
    assert_eq!(method.params[0].name, "U");
    let ty = symbols
        .iter()
        .find(|s| s.name == "make")
        .unwrap()
        .return_type
        .as_ref()
        .unwrap();
    assert_eq!(ty.name, "Box");
    assert_eq!(ty.args[0].name, "Item");
    assert_eq!(*ty.args[0].package_candidates, ["model"]);
    let nested = symbols
        .iter()
        .find(|s| s.name == "nested")
        .unwrap()
        .value_type
        .as_ref()
        .unwrap();
    assert_eq!(nested.args[0].name, "Inner");
    assert_eq!(nested.args[0].qualification.as_deref(), Some("model.Outer"));
}

#[test]
fn kotlin_calls_members_of_java_generic_result() {
    let mut ws = workspace(&[
        ("Item.kt", ITEM),
        ("Other.kt", OTHER),
        ("Box.java", JAVA_BOX),
        (
            "Use.kt",
            "package app\nfun run(box: Box<Item>) { box.get().useful() }\n",
        ),
    ]);
    assert_member(&mut ws, "Use.kt", "box.get().use", "useful", "usefulWrong");
}

#[test]
fn java_calls_members_of_java_and_kotlin_generic_results() {
    for box_name in ["Box", "KotlinBox"] {
        let source = format!(
            "package app; class Use {{ void run({box_name}<Item> box) {{ box.get().useful(); }} }}"
        );
        let mut ws = workspace(&[
            ("Item.kt", ITEM),
            ("Other.kt", OTHER),
            ("Box.java", JAVA_BOX),
            ("KotlinBox.kt", KOTLIN_BOX),
            ("Use.java", &source),
        ]);
        assert_member(
            &mut ws,
            "Use.java",
            "box.get().use",
            "useful",
            "usefulWrong",
        );
    }
}

#[test]
fn generic_return_arguments_flow_through_both_languages() {
    let mut ws = workspace(&[
        ("Item.kt", ITEM), ("Other.kt", OTHER), ("Box.java", JAVA_BOX),
        ("Factory.java", "package app; public class Factory { public Box<Item> make() { return null; } }"),
        ("Use.kt", "package app\nfun run(factory: Factory) { factory.make().get().useful() }\n"),
        ("Use.java", "package app; class Use { void run(Factory factory) { factory.make().get().useful(); } }"),
    ]);
    for file in ["Use.kt", "Use.java"] {
        assert_member(
            &mut ws,
            file,
            "factory.make().get().use",
            "useful",
            "usefulWrong",
        );
    }
}

#[test]
fn kotlin_generic_superclasses_substitute_reordered_arguments() {
    let mut ws = workspace(&[
        ("Item.kt", ITEM), ("Other.kt", OTHER),
        ("Types.kt", "package app\nopen class PairBox<K, V> {\n fun get(): V = error(\"fixture\")\n}\nopen class Middle<X, Y> : PairBox<Y, X>()\nclass Concrete : Middle<Item, Other>()\n"),
        ("Use.kt", "package app\nfun run(value: Concrete) { value.get().useful() }\n"),
        ("Use.java", "package app; class Use { void run(Concrete value) { value.get().useful(); } }"),
    ]);
    for file in ["Use.kt", "Use.java"] {
        assert_member(&mut ws, file, "value.get().use", "useful", "usefulWrong");
    }
}

#[test]
fn java_generic_superclasses_substitute_reordered_arguments() {
    let mut ws = workspace(&[
        ("Item.kt", ITEM),
        ("Other.kt", OTHER),
        (
            "PairBox.java",
            "package app; public class PairBox<K, V> { public V get() { return null; } }",
        ),
        (
            "Middle.java",
            "package app; public class Middle<X, Y> extends PairBox<Y, X> {}",
        ),
        (
            "Concrete.java",
            "package app; public class Concrete extends Middle<Item, Other> {}",
        ),
        (
            "Use.kt",
            "package app\nfun run(value: Concrete) { value.get().useful() }\n",
        ),
        (
            "Use.java",
            "package app; class Use { void run(Concrete value) { value.get().useful(); } }",
        ),
    ]);
    for file in ["Use.kt", "Use.java"] {
        assert_member(&mut ws, file, "value.get().use", "useful", "usefulWrong");
    }
}

#[test]
fn fully_qualified_generic_arguments_keep_their_declaration_package() {
    let mut ws = workspace(&[
        ("LocalItem.kt", "package app\nclass Item {\n fun usefulWrong() {}\n}\n"),
        ("ImportedItem.kt", "package model\nclass Item {\n fun useful() {}\n}\n"),
        ("Box.java", JAVA_BOX),
        ("JavaFactory.java", "package app; public class JavaFactory { public Box<model.Item> make() { return null; } }"),
        ("KotlinFactory.kt", "package app\nclass KotlinFactory {\n fun make(): Box<model.Item> = error(\"fixture\")\n}\n"),
        ("Use.kt", "package app\nfun run(java: JavaFactory, kotlin: KotlinFactory) { java.make().get().useful(); kotlin.make().get().useful() }\n"),
        ("Use.java", "package app; class Use { void run(JavaFactory java, KotlinFactory kotlin) { java.make().get().useful(); kotlin.make().get().useful(); } }"),
    ]);
    for file in ["Use.kt", "Use.java"] {
        for prefix in ["java.make().get().use", "kotlin.make().get().use"] {
            assert_member(&mut ws, file, prefix, "useful", "usefulWrong");
        }
    }
}

#[test]
fn generic_substitution_preserves_nested_arguments_and_nullability() {
    let ws = workspace(&[
        ("Item.kt", ITEM), ("Box.java", JAVA_BOX),
        ("Types.kt", "package app\nopen class Parent<T> {\n fun get(): Box<T?> = error(\"fixture\")\n}\nclass Concrete : Parent<Item>()\n"),
        ("Use.kt", "package app\nfun run(value: Concrete) { value.get() }\n"),
    ]);
    let ty = infer_expression(&ws, "Use.kt", "value.get()");
    assert_eq!(ty.name(), Some("Box"));
    assert_eq!(ty.package(), Some("app"));
    assert_eq!(ty.args().len(), 1);
    assert_eq!(ty.args()[0].name(), Some("Item"));
    assert_eq!(ty.args()[0].package(), Some("app"));
    assert!(ty.args()[0].is_nullable(), "lost T?: {ty:?}");
}

#[test]
fn unsaved_java_return_and_kotlin_supertype_edits_replace_generic_bindings() {
    let mut ws = workspace(&[
        ("Item.kt", ITEM), ("Other.kt", OTHER), ("Box.java", JAVA_BOX),
        ("Factory.java", "package app; public class Factory { public Box<Item> make() { return null; } }"),
        ("Types.kt", "package app\nopen class Parent<T> {\n fun get(): T = error(\"fixture\")\n}\nclass Concrete : Parent<Item>()\n"),
        ("Use.kt", "package app\nfun run(factory: Factory, value: Concrete) { factory.make().get().useful(); value.get().useful() }\n"),
        ("Use.java", "package app; class Use { void run(Factory factory, Concrete value) { factory.make().get().useful(); value.get().useful(); } }"),
    ]);
    for file in ["Use.kt", "Use.java"] {
        for prefix in ["factory.make().get().use", "value.get().use"] {
            assert_member(&mut ws, file, prefix, "useful", "usefulWrong");
        }
    }
    ws.change(
        "Factory.java",
        "package app; public class Factory { public Box<Other> make() { return null; } }".into(),
    );
    ws.change("Types.kt", "package app\nopen class Parent<T> {\n fun get(): T = error(\"fixture\")\n}\nclass Concrete : Parent<Other>()\n".into());
    for file in ["Use.kt", "Use.java"] {
        for prefix in ["factory.make().get().use", "value.get().use"] {
            assert_member(&mut ws, file, prefix, "usefulWrong", "useful");
        }
    }
}

#[test]
fn unknown_type_arguments_and_inheritance_cycles_do_not_guess_members() {
    let mut ws = workspace(&[
        ("Item.kt", ITEM), ("Box.java", JAVA_BOX),
        ("Cycle.kt", "package app\nopen class A : B()\nopen class B : A()\n"),
        ("Use.kt", "package app\nfun run(box: Box<*>, cycle: A) { box.get().useful(); cycle.get().useful() }\n"),
        ("Use.java", "package app; class Use { void run(Box box) { box.get().useful(); } }"),
    ]);
    assert!(labels(&mut ws, "Use.kt", "box.get().use").is_empty());
    assert!(labels(&mut ws, "Use.kt", "cycle.get().use").is_empty());
    assert!(labels(&mut ws, "Use.java", "box.get().use").is_empty());
}

#[test]
fn qualified_nested_types_resolve_without_casing_assumptions() {
    // The compiler oracle covers both languages for Outer.Inner, and Java for outer.inner.
    for (outer, inner, kotlin_supported) in [("Outer", "Inner", true), ("outer", "inner", false)] {
        let declaration = format!("package model; public class {outer} {{ public static class {inner} {{ public void useful() {{}} }} }}");
        let unrelated =
            format!("package app; public class {inner} {{ public void usefulWrong() {{}} }}");
        let factory = format!("package app; public class Factory {{ public Box<model.{outer}.{inner}> make() {{ return null; }} }}");
        let mut ws = workspace(&[
        ("Outer.java", &declaration),
        ("WrongInner.java", &unrelated),
        ("Box.java", JAVA_BOX),
        ("Factory.java", &factory),
        ("Use.kt", "package app\nfun run(factory: Factory) { factory.make().get().useful() }\n"),
        ("Use.java", "package app; class Use { void run(Factory factory) { factory.make().get().useful(); } }"),
    ]);
        for file in ["Use.kt", "Use.java"] {
            if file == "Use.kt" && !kotlin_supported {
                continue;
            }
            assert_member(
                &mut ws,
                file,
                "factory.make().get().use",
                "useful",
                "usefulWrong",
            );
        }
        if kotlin_supported {
            let ty = infer_expression(&ws, "Use.kt", "factory.make().get()");
            assert_eq!(ty.package(), Some("model"));
            assert_eq!(ty.container(), Some(outer));
        }
    }
}

#[test]
fn inherited_generic_parameter_accepts_the_correct_nested_argument_type() {
    let mut ws = workspace(&[
        ("Outer.java", "package model; public class Outer { public static class Inner {} }"),
        ("WrongInner.kt", "package app\nclass Inner\n"),
        ("Holder.kt", "package app\nopen class Holder<T> {\n fun accept(value: T) {}\n}\nclass Concrete : Holder<model.Outer.Inner>()\n"),
        ("Use.kt", "package app\nfun run(holder: Concrete, value: model.Outer.Inner) { holder.accept(value) }\n"),
    ]);
    ws.assume_index_complete_for_tests();
    let diagnostics = ws.diagnostics("Use.kt");
    assert!(
        diagnostics.is_empty(),
        "valid nested generic parameter produced diagnostics: {diagnostics:?}"
    );
}

#[test]
fn imported_generic_superclass_and_return_types_keep_declaration_visibility() {
    let mut ws = workspace(&[
        ("Item.kt", ITEM), ("Other.kt", OTHER),
        ("Base.java", "package library; import app.Item; public class Base<T> { public T get() { return null; } }"),
        ("WrongBase.java", "package app; public class Base<T> { public Other get() { return null; } }"),
        ("Concrete.kt", "package app\nclass Concrete : library.Base<Item>()\n"),
        ("Use.kt", "package app\nfun run(value: Concrete) { value.get().useful() }\n"),
        ("Use.java", "package app; class Use { void run(Concrete value) { value.get().useful(); } }"),
    ]);
    for file in ["Use.kt", "Use.java"] {
        assert_member(&mut ws, file, "value.get().use", "useful", "usefulWrong");
    }
}

#[test]
fn unknown_projections_preserve_later_type_argument_positions() {
    let mut ws = workspace(&[
        ("Item.kt", ITEM), ("Other.kt", OTHER),
        ("Pair.java", "package app; public class Pair<K, V> { public V get() { return null; } }"),
        ("Use.kt", "package app\nfun run(pair: Pair<*, Item>, unknown: Pair<Item, *>) { pair.get().useful(); unknown.get().useful() }\n"),
        ("Use.java", "package app; class Use { void run(Pair<?, Item> pair, Pair<Item, ?> unknown) { pair.get().useful(); unknown.get().useful(); } }"),
    ]);
    for file in ["Use.kt", "Use.java"] {
        assert_member(&mut ws, file, "pair.get().use", "useful", "usefulWrong");
        assert!(labels(&mut ws, file, "unknown.get().use").is_empty());
    }
}

#[test]
fn method_type_parameters_shadow_class_parameters_without_inventing_bindings() {
    let mut ws = workspace(&[
        ("Item.kt", ITEM),
        (
            "Box.java",
            "package app; public class Box<T> { public <T> T get() { return null; } }",
        ),
        (
            "Use.kt",
            "package app\nfun run(box: Box<Item>) { box.get().useful() }\n",
        ),
        (
            "Use.java",
            "package app; class Use { void run(Box<Item> box) { box.get().useful(); } }",
        ),
    ]);
    // Java cannot infer the method's T from the unrelated class argument; Kotlin requires context.
    for file in ["Use.kt", "Use.java"] {
        assert!(labels(&mut ws, file, "box.get().use").is_empty());
    }
}

#[test]
fn conflicting_inherited_bindings_remain_unknown() {
    let mut ws = workspace(&[
        ("Item.kt", ITEM), ("Other.kt", OTHER),
        ("Types.kt", "package app\ninterface Root<T> {\n fun get(): T\n}\ninterface Left : Root<Item>\ninterface Right : Root<Other>\nclass Conflict : Left, Right\n"),
        ("Use.kt", "package app\nfun run(value: Conflict) { value.get().useful() }\n"),
    ]);
    assert_eq!(
        infer_expression(&ws, "Use.kt", "value.get()"),
        Type::Unknown
    );
    assert!(labels(&mut ws, "Use.kt", "value.get().use").is_empty());
}

#[test]
fn matching_diamond_bindings_preserve_the_generic_result() {
    let mut ws = workspace(&[
        ("Item.kt", ITEM), ("Other.kt", OTHER),
        ("Types.kt", "package app\ninterface Root<T> {\n fun get(): T\n}\ninterface Left : Root<Item>\ninterface Right : Root<Item>\nabstract class Diamond : Left, Right\n"),
        ("Use.kt", "package app\nfun run(value: Diamond) { value.get().useful() }\n"),
        ("Use.java", "package app; class Use { void run(Diamond value) { value.get().useful(); } }"),
    ]);
    for file in ["Use.kt", "Use.java"] {
        assert_member(&mut ws, file, "value.get().use", "useful", "usefulWrong");
    }
}

#[test]
fn invalid_growing_generic_cycles_stop_in_both_language_clients() {
    for declarations in [
        "package app; class Pair<L, R> {} class A<T> extends A<Pair<T, T>> { T get() { return null; } }",
        "package app; class Pair<L, R> {} class A<T> extends B<Pair<T, T>> { T get() { return null; } } class B<T> extends A<Pair<T, T>> {}",
    ] {
        let mut ws = workspace(&[
            ("Item.kt", ITEM), ("Other.kt", OTHER),
            ("Cycles.java", declarations),
            ("Use.kt", "package app\nfun run(value: A<Item>) { value.get().useful(); value.missing() }\n"),
            ("Use.java", "package app; class Use { void run(A<Item> value) { value.get().useful(); value.missing(); } }"),
        ]);
        for file in ["Use.kt", "Use.java"] {
            // The direct member's declared T remains usable while invalid inherited cycles stop.
            assert_member(&mut ws, file, "value.get().use", "useful", "usefulWrong");
            let source = ws.doc_text(file).unwrap();
            let offset = source.find("missing").unwrap();
            assert!(ws.goto_definition(file, offset).is_empty());
        }
    }
}

#[test]
fn ambiguous_imported_type_arguments_do_not_merge_unrelated_members() {
    let mut ws = workspace(&[
        ("Left.kt", "package left\nclass Item {\n fun useful() {}\n}\n"),
        ("Right.kt", "package right\nclass Item {\n fun usefulWrong() {}\n}\n"),
        ("Box.java", JAVA_BOX),
        ("Factory.java", "package app; import left.*; import right.*; public class Factory { public Box<Item> make() { return null; } }"),
        ("Use.kt", "package app\nfun run(factory: Factory) { factory.make().get().useful() }\n"),
        ("Use.java", "package app; class Use { void run(Factory factory) { factory.make().get().useful(); } }"),
    ]);
    for file in ["Use.kt", "Use.java"] {
        assert!(labels(&mut ws, file, "factory.make().get().use").is_empty());
    }
}

#[test]
fn imported_superclass_completion_uses_the_declarations_own_imports() {
    let mut ws = imported_superclass_workspace();
    for file in ["app/Use.java", "app/Use.kt"] {
        for receiver in ["child", "kotlin"] {
            assert_member(
                &mut ws,
                file,
                &format!("{receiver}.ba"),
                "baseOnly",
                "baseWrong",
            );
        }
    }
}

#[test]
fn imported_superclass_navigation_uses_the_declarations_own_imports() {
    let mut ws = imported_superclass_workspace();
    for file in ["app/Use.java", "app/Use.kt"] {
        let source = ws.doc_text(file).unwrap();
        for receiver in ["child", "kotlin"] {
            for member in ["baseOnly", "accept"] {
                let offset =
                    source.find(&format!("{receiver}.{member}")).unwrap() + receiver.len() + 1;
                let defs = ws.goto_definition(file, offset);
                assert_eq!(defs.len(), 1, "{file} {receiver}.{member}: {defs:?}");
                assert_eq!(
                    defs[0].file, "base/Base.java",
                    "{file} {receiver}.{member}: {defs:?}"
                );
            }
        }
    }
}

#[test]
fn imported_superclass_call_diagnostics_use_the_declarations_own_imports() {
    let mut ws = imported_superclass_workspace();
    ws.assume_index_complete_for_tests();
    for file in ["app/Use.java", "app/Use.kt"] {
        assert!(
            ws.diagnostics(file).is_empty(),
            "{file}: {:?}",
            ws.diagnostics(file)
        );
    }
}

#[test]
fn unsaved_superclass_import_edits_update_both_languages() {
    let mut ws = imported_superclass_workspace();
    for (declaration, changed, receiver) in [
        (
            "app/Child.java",
            JAVA_CHILD.replace("import base.Base", "import app.Base"),
            "child",
        ),
        (
            "app/KotlinChild.kt",
            KOTLIN_CHILD.replace("import base.Base", "import app.Base"),
            "kotlin",
        ),
    ] {
        for file in ["app/Use.java", "app/Use.kt"] {
            assert_member(
                &mut ws,
                file,
                &format!("{receiver}.ba"),
                "baseOnly",
                "baseWrong",
            );
        }
        ws.change(declaration, changed);
        for file in ["app/Use.java", "app/Use.kt"] {
            assert_member(
                &mut ws,
                file,
                &format!("{receiver}.ba"),
                "baseWrong",
                "baseOnly",
            );
            let source = ws.doc_text(file).unwrap();
            let offset = source.find(&format!("{receiver}.accept")).unwrap() + receiver.len() + 1;
            let defs = ws.goto_definition(file, offset);
            assert_eq!(defs.len(), 1, "{file}: {defs:?}");
            assert_eq!(
                defs[0].file, "app/Base.java",
                "stale superclass after {declaration} edit"
            );
        }
    }
}

#[test]
fn missing_imported_superclass_facts_keep_member_absence_unknown() {
    use ktlsp::resolve::{self, CompletenessFacts, ResolutionStatus};
    let mut ws = workspace(&[
        ("app/Base.java", WRONG_BASE),
        ("app/Child.java", JAVA_CHILD),
        ("app/KotlinChild.kt", KOTLIN_CHILD),
        ("app/Use.kt", KOTLIN_CHILD_USE),
    ]);
    ws.assume_index_complete_for_tests();
    let source = ws.doc_text("app/Use.kt").unwrap();
    let tree = KotlinParser::new().parse(&source);
    let offset = source.find("child.baseOnly").unwrap() + "child.".len();
    let usage = ktlsp::parser::identifier_at(&tree, offset).unwrap();
    let status = resolve::reference_status(
        &ws.index,
        "app/Use.kt",
        &tree,
        &source,
        usage,
        &CompletenessFacts::complete(),
    );
    assert!(
        matches!(status, ResolutionStatus::Unknown(_)),
        "missing superclass: {status:?}"
    );
    assert!(
        ws.diagnostics("app/Use.kt").is_empty(),
        "missing dependency symbols cannot prove an invalid call"
    );
}

#[test]
fn imported_superclass_invalid_arguments_still_report_call_errors() {
    use ktlsp::diagnostics::DiagnosticCode;
    let mut ws = imported_superclass_workspace();
    ws.assume_index_complete_for_tests();
    for (file, source) in [
        ("app/Use.java", JAVA_CHILD_USE),
        ("app/Use.kt", KOTLIN_CHILD_USE),
    ] {
        ws.change(file, source.replace("\"ok\"", "1"));
        let diagnostics = ws.diagnostics(file);
        assert_eq!(diagnostics.len(), 2, "{file}: {diagnostics:?}");
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.code == Some(DiagnosticCode::CallShapeMismatch)),
            "{file}: {diagnostics:?}"
        );
    }
}

#[test]
fn imported_superclass_library_completeness_follows_its_actual_package() {
    use ktlsp::index::Tier;
    use ktlsp::resolve::{self, CompletenessFacts, IncompletenessReason};
    let mut ws = imported_superclass_workspace();
    ws.index.replace_file(
        "base/Base.java",
        java::extract_symbols(&JavaParser::new().parse(IMPORTED_BASE), IMPORTED_BASE),
        Tier::Durable,
    );
    let source = ws.doc_text("app/Use.kt").unwrap();
    let tree = KotlinParser::new().parse(&source);
    let offset = source.find("child.accept").unwrap() + "child.".len();
    let usage = ktlsp::parser::identifier_at(&tree, offset).unwrap();
    let facts = CompletenessFacts {
        library_index_complete: false,
        ..CompletenessFacts::complete()
    };
    let reasons = resolve::member_call_completeness_reasons(
        &ws.index,
        "app/Use.kt",
        &tree,
        &source,
        usage,
        &facts,
    );
    assert!(
        reasons.contains(&IncompletenessReason::LibraryPackageIncomplete(
            "base".into()
        )),
        "{reasons:?}"
    );
}

#[test]
fn recovered_this_completion_keeps_typed_superclasses_and_later_own_members() {
    let java_child = "package app; import base.Base; public class Child extends Base { void run() { this. } void javaOwn() {} }";
    let kotlin_child = "package app\nimport base.Base\nclass KotlinChild : Base() {\n fun run() { this. }\n fun kotlinOwn() {}\n}\n";
    let mut ws = workspace(&[
        ("base/Base.java", IMPORTED_BASE),
        ("app/Base.java", WRONG_BASE),
        ("app/Child.java", java_child),
        ("app/KotlinChild.kt", kotlin_child),
    ]);
    for (file, own, unrelated) in [
        ("app/Child.java", "javaOwn", "kotlinOwn"),
        ("app/KotlinChild.kt", "kotlinOwn", "javaOwn"),
    ] {
        let original = ws.doc_text(file).unwrap();
        let candidates = labels(&mut ws, file, "this.");
        assert!(candidates.contains("baseOnly"), "{file}: {candidates:?}");
        assert!(
            candidates.contains(own),
            "later recovered own member missing in {file}: {candidates:?}"
        );
        assert!(
            !candidates.contains("baseWrong"),
            "wrong same-package superclass in {file}: {candidates:?}"
        );
        assert!(
            !candidates.contains(unrelated),
            "unrelated type member in {file}: {candidates:?}"
        );
        assert_eq!(
            ws.doc_text(file).unwrap(),
            original,
            "recovery must preserve canonical source"
        );
    }
}

#[test]
fn qualified_nested_superclasses_keep_package_and_container_identity() {
    let mut ws = imported_superclass_workspace();
    ws.open("base/Outer.java", "package base; public class Outer { public static class Base { public void baseOnly() {} public void accept(String value) {} } }".into());
    ws.open("base/OtherOuter.java", "package base; public class OtherOuter { public static class Base { public void baseWrong() {} public void accept(int value) {} } }".into());
    ws.change(
        "app/Child.java",
        "package app; public class Child extends base.Outer.Base {}".into(),
    );
    ws.change(
        "app/KotlinChild.kt",
        "package app\nclass KotlinChild : base.Outer.Base()\n".into(),
    );
    ws.assume_index_complete_for_tests();
    for file in ["app/Use.java", "app/Use.kt"] {
        let source = ws.doc_text(file).unwrap();
        for receiver in ["child", "kotlin"] {
            assert_member(
                &mut ws,
                file,
                &format!("{receiver}.ba"),
                "baseOnly",
                "baseWrong",
            );
            let offset = source.find(&format!("{receiver}.accept")).unwrap() + receiver.len() + 1;
            let defs = ws.goto_definition(file, offset);
            assert_eq!(defs.len(), 1, "{file}: {defs:?}");
            assert_eq!(defs[0].file, "base/Outer.java", "{file}: {defs:?}");
        }
        assert!(
            ws.diagnostics(file).is_empty(),
            "{file}: {:?}",
            ws.diagnostics(file)
        );
    }
}

#[test]
fn imported_superclass_receiver_lambdas_use_the_declaration_context() {
    let mut ws = workspace(&[
        ("Item.kt", ITEM), ("Other.kt", OTHER),
        ("base/Base.kt", "package base\nimport app.Item\nopen class Base {\n fun withItem(block: Item.() -> Unit) { Item().block() }\n}\n"),
        ("app/Base.kt", "package app\nopen class Base {\n fun withItem(block: Other.() -> Unit) {}\n}\n"),
        ("app/Child.java", JAVA_CHILD), ("app/KotlinChild.kt", KOTLIN_CHILD),
        ("app/Use.kt", "package app\nfun run(child: Child, kotlin: KotlinChild) {\n child.withItem { useful() }\n kotlin.withItem { useful() }\n}\n"),
    ]);
    ws.assume_index_complete_for_tests();
    for receiver in ["child", "kotlin"] {
        let prefix = format!("{receiver}.withItem {{ use");
        assert_member(&mut ws, "app/Use.kt", &prefix, "useful", "usefulWrong");
    }
    let diagnostics = ws.diagnostics("app/Use.kt");
    assert!(
        diagnostics.is_empty(),
        "valid inherited receiver lambda: {diagnostics:?}"
    );
}

const REFLECTION_CLASS: &str = "package java.lang; public final class Class<T> { public T cast(Object value) { return null; } }";
const REFLECTION_KCLASS: &str = "package kotlin.reflect\ninterface KClass<out T>\n";
const CLASS_MAPPING: &str = "package kotlin.jvm\nimport kotlin.reflect.KClass\npublic val <T> KClass<T>.java: Class<T> get() = error(\"fixture\")\n";
const INVOCATION: &str =
    "package probe; public class Invocation { public Class<?> service() { return null; } }";
const OTHER_INVOCATION: &str = "package probe; public class Other { public String body() { return \"fixture\"; } public void serviceWrong() {} }";
const EXACT_REQUEST: &str =
    "package probe; public class ExactRequest { public <T> T tag(Class<T> type) { return null; } }";

fn reflection_workspace(source: &str) -> Workspace {
    workspace(&[
        ("Class.java", REFLECTION_CLASS),
        ("KClass.kt", REFLECTION_KCLASS),
        ("JvmClassMapping.kt", CLASS_MAPPING),
        ("Invocation.java", INVOCATION),
        ("Other.java", OTHER_INVOCATION),
        ("ExactRequest.java", EXACT_REQUEST),
        ("Use.kt", source),
    ])
}

fn assert_exact_definition(
    ws: &mut Workspace,
    file: &str,
    usage: &str,
    declaration: &str,
    name: &str,
) {
    let source = ws.doc_text(file).unwrap();
    let offset = source.find(usage).unwrap() + usage.rfind(name).unwrap();
    let definitions = ws.goto_definition(file, offset);
    assert_eq!(definitions.len(), 1, "{file} {usage}: {definitions:?}");
    assert_eq!(definitions[0].file, declaration);
    let expected = ws.doc_text(declaration).unwrap().find(name).unwrap();
    assert_eq!(definitions[0].start_byte, expected);
    assert_eq!(definitions[0].end_byte, expected + name.len());
}

#[test]
fn class_literals_and_typed_kclass_use_the_generic_java_property() {
    let mut ws = reflection_workspace(
        r#"package app
import kotlin.reflect.KClass
import probe.ExactRequest
import probe.Invocation
import probe.Invocation as Alias
import probe.Outer
fun run(exact: ExactRequest, typed: KClass<Invocation>) {
 exact.tag(Invocation::class.java)!!.service()
 exact.tag(Alias::class.java)!!.service()
 exact.tag(probe.Invocation::class.java)!!.service()
 exact.tag(Outer.Inner::class.java)!!.nestedOnly()
 exact.tag(typed.java)!!.service()
}
"#,
    );
    ws.open("Outer.java", "package probe; public class Outer { public static class Inner { public void nestedOnly() {} } }".into());
    ws.open(
        "WrongInvocation.java",
        "package unrelated; public class Invocation { public void serviceWrong() {} }".into(),
    );
    for literal in [
        "Invocation::class",
        "Alias::class",
        "probe.Invocation::class",
    ] {
        let ty = infer_expression(&ws, "Use.kt", literal);
        assert_eq!(ty.name(), Some("KClass"), "{literal}: {ty:?}");
        assert_eq!(ty.package(), Some("kotlin.reflect"));
        assert_eq!(ty.args(), [Type::class("Invocation", Some("probe".into()))]);
        let receiver = format!("exact.tag({literal}.java)!!.");
        assert_member(&mut ws, "Use.kt", &receiver, "service", "serviceWrong");
        assert_exact_definition(
            &mut ws,
            "Use.kt",
            &format!("{receiver}service"),
            "Invocation.java",
            "service",
        );
    }
    let nested = infer_expression(&ws, "Use.kt", "Outer.Inner::class");
    assert_eq!(nested.args().len(), 1);
    assert_eq!(nested.args()[0].name(), Some("Inner"));
    assert_eq!(nested.args()[0].package(), Some("probe"));
    assert_eq!(nested.args()[0].container(), Some("Outer"));
    assert_member(
        &mut ws,
        "Use.kt",
        "exact.tag(Outer.Inner::class.java)!!.",
        "nestedOnly",
        "service",
    );
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "exact.tag(Outer.Inner::class.java)!!.nestedOnly",
        "Outer.java",
        "nestedOnly",
    );
    let typed = infer_expression(&ws, "Use.kt", "typed.java");
    assert_eq!(typed.name(), Some("Class"));
    assert_eq!(typed.package(), Some("java.lang"));
    assert_eq!(
        typed.args(),
        [Type::class("Invocation", Some("probe".into()))]
    );
    assert_member(&mut ws, "Use.kt", "typed.ja", "java", "body");
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "typed.java",
        "JvmClassMapping.kt",
        "java",
    );
    assert_member(
        &mut ws,
        "Use.kt",
        "exact.tag(typed.java)!!.",
        "service",
        "serviceWrong",
    );
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "exact.tag(typed.java)!!.service",
        "Invocation.java",
        "service",
    );
}

#[test]
fn generic_extension_receiver_templates_bind_positions_and_repeated_variables() {
    let declarations = r#"package app
open class PairBox<X, Y> { fun body() {} }
class Child : PairBox<Other, Item>()
val <A, B> PairBox<B, A>.chosen: A get() = error("fixture")
fun <A, B> PairBox<B, A>.choose(): A = error("fixture")
val <T> PairBox<T, T>.same: T get() = error("fixture")
"#;
    let mut ws = workspace(&[
        ("Item.kt", ITEM), ("Other.kt", OTHER), ("Types.kt", declarations),
        ("Use.kt", "package app\nfun run(pair: PairBox<Other, Item>, child: Child, same: PairBox<Item, Item>, conflict: PairBox<Item, Other>) { pair.chosen.useful(); child.chosen.useful(); pair.choose().useful(); same.same.useful(); conflict.same.useful() }\n"),
    ]);
    for receiver in [
        "pair.chosen.",
        "child.chosen.",
        "pair.choose().",
        "same.same.",
    ] {
        assert_member(&mut ws, "Use.kt", receiver, "useful", "usefulWrong");
        assert_exact_definition(
            &mut ws,
            "Use.kt",
            &format!("{receiver}useful"),
            "Item.kt",
            "useful",
        );
    }
    assert_member(&mut ws, "Use.kt", "pair.ch", "chosen", "same");
    assert_exact_definition(&mut ws, "Use.kt", "pair.chosen", "Types.kt", "chosen");
    assert_eq!(
        infer_expression(&ws, "Use.kt", "conflict.same"),
        Type::Unknown
    );
    // A failed repeated-variable match still has a known receiver with useful ordinary members.
    assert_member(&mut ws, "Use.kt", "conflict.", "body", "same");
    let source = ws.doc_text("Use.kt").unwrap();
    let offset = source.find("conflict.same").unwrap() + "conflict.".len();
    assert!(ws.goto_definition("Use.kt", offset).is_empty());
}

#[test]
fn generic_extension_properties_keep_conflicting_inheritance_unknown() {
    let declarations = r#"package app
interface Root<T>
interface Left : Root<Item>
interface Right : Root<Other>
interface Agree : Root<Item>
abstract class Conflict : Left, Right
abstract class Diamond : Left, Agree
val <T> Root<T>.chosen: T get() = error("fixture")
"#;
    let mut ws = workspace(&[
        ("Item.kt", ITEM), ("Other.kt", OTHER), ("Types.kt", declarations),
        ("Use.kt", "package app\nfun run(good: Diamond, bad: Conflict) { good.chosen.useful(); bad.chosen.useful() }\n"),
    ]);
    assert_member(&mut ws, "Use.kt", "good.chosen.", "useful", "usefulWrong");
    assert_exact_definition(&mut ws, "Use.kt", "good.chosen.useful", "Item.kt", "useful");
    assert_eq!(infer_expression(&ws, "Use.kt", "bad.chosen"), Type::Unknown);
    assert!(!labels(&mut ws, "Use.kt", "bad.ch").contains("chosen"));
}

#[test]
fn generic_extension_properties_do_not_offer_wrong_concrete_receiver_arguments() {
    use ktlsp::resolve::{self, CompletenessFacts, ResolutionStatus};
    let mut ws = reflection_workspace("package probe\nimport kotlin.reflect.KClass\nfun run(typed: KClass<Invocation>, other: KClass<Other>) { typed.java; other.java }\n");
    ws.change("JvmClassMapping.kt", "package kotlin.jvm\nimport kotlin.reflect.KClass\nimport probe.Other\nval KClass<Other>.java: Class<Other> get() = error(\"fixture\")\n".into());
    ws.change(
        "KClass.kt",
        "package kotlin.reflect\ninterface KClass<out T> { fun body() }\n".into(),
    );
    assert_member(&mut ws, "Use.kt", "other.", "java", "service");
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "other.java",
        "JvmClassMapping.kt",
        "java",
    );
    assert_member(&mut ws, "Use.kt", "typed.", "body", "java");
    assert_eq!(infer_expression(&ws, "Use.kt", "typed.java"), Type::Unknown);
    let source = ws.doc_text("Use.kt").unwrap();
    let offset = source.find("typed.java").unwrap() + "typed.".len();
    assert!(ws.goto_definition("Use.kt", offset).is_empty());
    let tree = KotlinParser::new().parse(&source);
    let usage = ktlsp::parser::identifier_at(&tree, offset).unwrap();
    let status = resolve::reference_status(
        &ws.index,
        "Use.kt",
        &tree,
        &source,
        usage,
        &CompletenessFacts::complete(),
    );
    assert!(
        matches!(status, ResolutionStatus::Unknown(_)),
        "variance is not a proof of absence: {status:?}"
    );
}

#[test]
fn generic_extension_property_edits_replace_bindings_and_respect_type_parameter_shadowing() {
    let mut ws = reflection_workspace("package probe\nfun run(exact: ExactRequest) { val result = exact.tag(Invocation::class.java)!!; result.service() }\n");
    assert_member(&mut ws, "Use.kt", "result.", "service", "serviceWrong");
    ws.open(
        "WrongT.java",
        "package unrelated; public class WrongT {}".into(),
    );
    ws.change(
        "JvmClassMapping.kt",
        CLASS_MAPPING.replace(
            "import kotlin.reflect.KClass",
            "import kotlin.reflect.KClass\nimport unrelated.WrongT as T",
        ),
    );
    assert_member(&mut ws, "Use.kt", "result.", "service", "serviceWrong");
    ws.change(
        "JvmClassMapping.kt",
        CLASS_MAPPING.replace("Class<T> get()", "Class<probe.Other> get()"),
    );
    assert_member(&mut ws, "Use.kt", "result.", "body", "service");
    let changed = ws
        .doc_text("Use.kt")
        .unwrap()
        .replace("result.service()", "result.body()");
    ws.change("Use.kt", changed);
    assert_exact_definition(&mut ws, "Use.kt", "result.body", "Other.java", "body");
    ws.change("JvmClassMapping.kt", CLASS_MAPPING.into());
    assert_member(&mut ws, "Use.kt", "result.", "service", "body");
    ws.change("Use.kt", "package probe\nfun run(exact: ExactRequest) { val result = exact.tag(Invocation::class.java)!!; result.ser }\n".into());
    assert_member(&mut ws, "Use.kt", "result.ser", "service", "serviceWrong");
}

#[test]
fn kotlin_projected_arguments_preserve_slots_without_inventing_extension_bindings() {
    for projection in ["out Item", "in Item", "*"] {
        let source = format!("package app\nfun run(value: PairBox<{projection}, Item>) {{ value.second().useful(); value.chosen.useful() }}\n");
        let mut ws = workspace(&[
            ("Item.kt", ITEM),
            ("Other.kt", OTHER),
            (
                "PairBox.java",
                "package app; public class PairBox<X, Y> { public Y second() { return null; } }",
            ),
            (
                "Extensions.kt",
                "package app\nval <T> PairBox<T, Item>.chosen: T get() = error(\"fixture\")\n",
            ),
            ("Use.kt", &source),
        ]);
        assert_member(
            &mut ws,
            "Use.kt",
            "value.second().",
            "useful",
            "usefulWrong",
        );
        // Reading an upper bound remains a coverage gap; preserving the slot is not support for it.
        assert_eq!(
            infer_expression(&ws, "Use.kt", "value.chosen"),
            Type::Unknown
        );
    }
    let mut ws = workspace(&[
        ("Item.kt", ITEM),
        ("Types.kt", "package app\nclass Holder<T> { fun body() {} }\nval <T : Item> Holder<T>.bounded: Item get() = error(\"fixture\")\nval <T> Holder<out T>.projected: Item get() = error(\"fixture\")\nval <T> Holder<T>.known: T get() = error(\"fixture\")\n"),
        ("Use.kt", "package app\nfun run(value: Holder<Item>) { value.known.useful(); value.bounded.useful(); value.projected.useful() }\n"),
    ]);
    assert_member(&mut ws, "Use.kt", "value.known.", "useful", "missing");
    assert_member(&mut ws, "Use.kt", "value.", "bounded", "projected");
    assert_member(&mut ws, "Use.kt", "value.bounded.", "useful", "missing");
    assert_exact_definition(&mut ws, "Use.kt", "value.bounded", "Types.kt", "bounded");
    assert_eq!(
        infer_expression(&ws, "Use.kt", "value.bounded"),
        Type::class("Item", Some("app".into()))
    );
    assert_eq!(
        infer_expression(&ws, "Use.kt", "value.projected"),
        Type::Unknown
    );
}

#[test]
fn class_literals_do_not_reinterpret_shadowed_values_or_callable_references() {
    let mut ws = reflection_workspace(
        r#"package probe
fun bound(Invocation: Other) { val parameterClass = Invocation::class }
fun local(known: Invocation) { val Other = known; val localClass = Other::class }
fun expression(known: Invocation) { val expressionClass = known::class; val boundMethod = known::service }
inline fun <reified T> parameter() { val parameterClass = T::class }
fun unbound() { val method = Invocation::service; val named = Invocation::class }
"#,
    );
    ws.open("GlobalT.java", "package probe; public class T {}".into());
    // Expression/reified class literals and callable-reference types remain explicit coverage gaps.
    // These guards only prevent them from binding to unrelated global declarations.
    for expression in [
        "Invocation::class",
        "Other::class",
        "known::class",
        "T::class",
        "known::service",
        "Invocation::service",
    ] {
        assert_eq!(
            infer_expression(&ws, "Use.kt", expression),
            Type::Unknown,
            "{expression}"
        );
    }
    let source = ws.doc_text("Use.kt").unwrap().replace(
        "fun bound(Invocation: Other) { val parameterClass = Invocation::class }\n",
        "",
    );
    ws.change("Use.kt", source);
    let named = infer_expression(&ws, "Use.kt", "Invocation::class");
    assert_eq!(named.name(), Some("KClass"));
    assert_eq!(
        named.args(),
        [Type::class("Invocation", Some("probe".into()))]
    );
    ws.open(
        "Unrelated.java",
        "package unrelated; public class Invocation {}".into(),
    );
    for imports in ["", "import probe.*\nimport unrelated.*\n"] {
        ws.change("Use.kt", format!("package app\n{imports}fun run() {{ val ambiguous = Invocation::class; val exact = probe.Invocation::class }}\n"));
        assert_eq!(
            infer_expression(&ws, "Use.kt", "Invocation::class"),
            Type::Unknown
        );
        let exact = infer_expression(&ws, "Use.kt", "probe.Invocation::class");
        assert_eq!(
            exact.args(),
            [Type::class("Invocation", Some("probe".into()))]
        );
    }
}

#[test]
fn kotlin_block_body_returns_intrinsic_unit_in_generic_receiver_chains() {
    for shadow in ["class Unit { fun unitWrong() {} }", "import probe.Unit"] {
        let source = format!(
            r#"package app
{shadow}
fun block() {{ Item() }}
fun explicit(): Item {{ return Item() }}
fun expression() = Item()
fun unknownExpression() = missingFactory()
fun run() {{
    val result = runCatching {{ block() }}
    result.onFailure {{ }}
    result.value()
    explicit().useful()
    expression().useful()
    unknownExpression().missing()
}}
"#
        );
        let mut ws = workspace(&[
            ("Item.kt", ITEM),
            ("probe/Unit.kt", "package probe\nclass Unit { fun unitWrong() {} }\n"),
            ("Stdlib.kt", "package kotlin\nclass Throwable\nclass Result<T> { fun value(): T = TODO() }\nfun <R> runCatching(block: () -> R): Result<R> = TODO()\nfun <T> Result<T>.onFailure(action: (Throwable) -> Unit): Result<T> = this\n"),
            ("Use.kt", &source),
        ]);
        assert_eq!(
            infer_expression(&ws, "Use.kt", "result.value()"),
            Type::class("Unit", Some("kotlin".into())),
            "{shadow}"
        );
        assert_member(&mut ws, "Use.kt", "result.", "onFailure", "unitWrong");
        assert_exact_definition(
            &mut ws,
            "Use.kt",
            "result.onFailure",
            "Stdlib.kt",
            "onFailure",
        );
        assert_member(&mut ws, "Use.kt", "explicit().", "useful", "unitWrong");
        assert_member(&mut ws, "Use.kt", "expression().", "useful", "unitWrong");
        assert_eq!(
            infer_expression(&ws, "Use.kt", "unknownExpression().missing"),
            Type::Unknown
        );
    }
}

#[test]
fn generic_extension_upper_bounds_require_exact_nominal_identity_and_complete_supertypes() {
    let mut ws = workspace(&[
        ("Item.kt", ITEM),
        ("Other.kt", OTHER),
        (
            "base/Bounds.kt",
            r#"package base
open class Bound { fun boundOnly() {} }
interface Marker<T>
class Outer { open class Nested { fun nestedOnly() {} } }
class OtherOuter { open class Nested }
"#,
        ),
        (
            "other/Bounds.kt",
            "package other\nclass Bound { fun wrongOnly() {} }\n",
        ),
        (
            "Types.kt",
            r#"package app
import base.Bound as Upper
import base.Marker
class Holder<T> { fun body() {} }
class Good : Upper()
class GoodMarker : Marker<Item> { fun markerOnly() {} }
class WrongMarker : Marker<Other>
interface Left : Marker<Item>
interface Right : Marker<Other>
class Conflict : Left, Right
class Broken : Upper(), Missing
class LoopA : Upper(), LoopB
class LoopB : LoopA
val <T : Upper> Holder<T>.bounded: T get() = error("fixture")
val <T : Marker<Item>> Holder<T>.marked: T get() = error("fixture")
val <T : base.Outer.Nested> Holder<T>.nested: T get() = error("fixture")
val <T : Missing> Holder<T>.unknownBound: T get() = error("fixture")
val <T : Marker<T>> Holder<T>.recursive: T get() = error("fixture")
val <T> Holder<T>.whereBound: T where T : Upper get() = error("fixture")
"#,
        ),
        (
            "Use.kt",
            r#"package app
fun run(
    exact: Holder<base.Bound>, child: Holder<Good>, wrong: Holder<other.Bound>,
    unrelated: Holder<Other>, unresolved: Holder<Missing>, nullable: Holder<base.Bound?>,
    goodMarker: Holder<GoodMarker>, wrongMarker: Holder<WrongMarker>,
    conflict: Holder<Conflict>, broken: Holder<Broken>, cycle: Holder<LoopA>,
    goodNested: Holder<base.Outer.Nested>, wrongNested: Holder<base.OtherOuter.Nested>
) {
    exact.bounded.boundOnly(); child.bounded.boundOnly()
    wrong.bounded; unrelated.bounded; unresolved.bounded; nullable.bounded
    goodMarker.marked.markerOnly(); wrongMarker.marked; conflict.marked
    broken.bounded; cycle.bounded
    goodNested.nested.nestedOnly(); wrongNested.nested
    exact.unknownBound; exact.recursive; child.whereBound
}
"#,
        ),
    ]);
    for receiver in ["exact", "child"] {
        assert_member(
            &mut ws,
            "Use.kt",
            &format!("{receiver}."),
            "bounded",
            "unknownBound",
        );
        assert_member(
            &mut ws,
            "Use.kt",
            &format!("{receiver}.bounded."),
            "boundOnly",
            "wrongOnly",
        );
        assert_exact_definition(
            &mut ws,
            "Use.kt",
            &format!("{receiver}.bounded"),
            "Types.kt",
            "bounded",
        );
    }
    assert_member(
        &mut ws,
        "Use.kt",
        "goodMarker.marked.",
        "markerOnly",
        "wrongOnly",
    );
    assert_exact_definition(&mut ws, "Use.kt", "goodMarker.marked", "Types.kt", "marked");
    assert_member(
        &mut ws,
        "Use.kt",
        "goodNested.nested.",
        "nestedOnly",
        "wrongOnly",
    );
    assert_exact_definition(&mut ws, "Use.kt", "goodNested.nested", "Types.kt", "nested");
    // Incomplete bounds and F-bounds remain coverage gaps, not useful-positive acceptance.
    for (receiver, property) in [
        ("wrong", "bounded"),
        ("unrelated", "bounded"),
        ("unresolved", "bounded"),
        ("nullable", "bounded"),
        ("wrongMarker", "marked"),
        ("conflict", "marked"),
        ("broken", "bounded"),
        ("cycle", "bounded"),
        ("wrongNested", "nested"),
        ("exact", "unknownBound"),
        ("exact", "recursive"),
        ("child", "whereBound"),
    ] {
        assert_member(&mut ws, "Use.kt", &format!("{receiver}."), "body", property);
        let expression = format!("{receiver}.{property}");
        assert_eq!(
            infer_expression(&ws, "Use.kt", &expression),
            Type::Unknown,
            "{expression}"
        );
        let source = ws.doc_text("Use.kt").unwrap();
        let offset = source.find(&expression).unwrap() + receiver.len() + 1;
        assert!(
            ws.goto_definition("Use.kt", offset).is_empty(),
            "{expression}"
        );
    }
}

#[test]
fn uncertain_implicit_extension_receiver_does_not_fall_back_to_global_homonyms() {
    let mut ws = workspace(&[
        (
            "Pipeline.kt",
            r#"package pipeline
class Item
class Other
class Holder<T> { fun body() {} }
val Holder<Other>.choice: Other get() = TODO()
val Other.choice: Int get() = 1
typealias Handler<C> = Holder<C>.() -> Unit
fun handle(body: Handler<Item>) {}
"#,
        ),
        (
            "Use.kt",
            r#"package app
import pipeline.handle
import pipeline.choice
fun run() { handle { body(); choice } }
"#,
        ),
    ]);
    assert_exact_definition(&mut ws, "Use.kt", "body();", "Pipeline.kt", "body");
    let source = ws.doc_text("Use.kt").unwrap();
    let offset = source.rfind("choice").unwrap();
    assert!(ws.goto_definition("Use.kt", offset).is_empty());
}

#[test]
fn generic_extension_function_bounds_are_checked_after_call_argument_binding() {
    let mut ws = workspace(&[
        ("Item.kt", ITEM),
        ("Other.kt", OTHER),
        (
            "Provider.kt",
            r#"package app
class Factory { fun body() {} }
class Provider<T> { fun getObject(): T = TODO() }
inline fun <reified T : Any> Factory.pending(): Provider<T> = TODO()
fun <T : Item> Factory.produce(value: T): Provider<T> = TODO()
fun <T : Item> Factory.build(value: T, body: T.() -> Unit) {}
"#,
        ),
        (
            "Use.kt",
            r#"package app
inline fun <reified T : Any> run(factory: Factory, item: Item, other: Other) {
    factory.pending<T>().getObject()
    factory.produce(item).getObject().useful()
    factory.produce(other).getObject().usefulWrong()
    factory.build(item) { useful() }
    factory.build(other) { usefulWrong() }
    factory.body()
}
"#,
        ),
    ]);
    // Explicit/reified call type arguments remain unbound; their known container is still useful.
    let pending = infer_expression(&ws, "Use.kt", "factory.pending<T>()");
    assert_eq!(pending.name(), Some("Provider"));
    assert_eq!(pending.package(), Some("app"));
    assert_eq!(pending.args(), [Type::Unknown]);
    assert_member(
        &mut ws,
        "Use.kt",
        "factory.pending<T>().",
        "getObject",
        "usefulWrong",
    );
    assert_member(
        &mut ws,
        "Use.kt",
        "factory.produce(item).getObject().",
        "useful",
        "usefulWrong",
    );
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "factory.produce(item).getObject().useful",
        "Item.kt",
        "useful",
    );
    assert_eq!(
        infer_expression(&ws, "Use.kt", "factory.produce(other)"),
        Type::Unknown
    );
    assert!(labels(&mut ws, "Use.kt", "factory.produce(other).").is_empty());
    assert_member(
        &mut ws,
        "Use.kt",
        "factory.build(item) { use",
        "useful",
        "usefulWrong",
    );
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "factory.build(item) { useful",
        "Item.kt",
        "useful",
    );
    assert!(!labels(&mut ws, "Use.kt", "factory.build(other) { useful").contains("usefulWrong"));
    assert_member(&mut ws, "Use.kt", "factory.", "body", "usefulWrong");
}

#[test]
fn nullable_extension_variables_require_proven_non_null_bounds() {
    let mut ws = workspace(&[
        ("Item.kt", ITEM),
        ("other/Any.kt", "package other\nclass Any\n"),
        (
            "Nullable.kt",
            r#"package app
class Holder<T> { fun body() {} }
val <T : kotlin.Any> Holder<T?>.picked: T get() = TODO()
fun <T : kotlin.Any> Holder<T?>.read(): T? = TODO()
val <T : kotlin.Any> Holder<T>.nonNull: T get() = TODO()
val <T : other.Any> Holder<T?>.wrongAny: T get() = TODO()
val <T> Holder<T?>.unbounded: T get() = TODO()
"#,
        ),
        (
            "Use.kt",
            r#"package app
fun run(nullable: Holder<Item?>, strict: Holder<Item>, unknown: Holder<Missing?>) {
    nullable.picked.useful(); nullable.read()
    nullable.nonNull; nullable.wrongAny; nullable.unbounded
    strict.picked; unknown.picked
}
"#,
        ),
    ]);
    assert_eq!(
        infer_expression(&ws, "Use.kt", "nullable.picked"),
        Type::class("Item", Some("app".to_string()))
    );
    let result = infer_expression(&ws, "Use.kt", "nullable.read()");
    assert_eq!(result.name(), Some("Item"));
    assert_eq!(result.package(), Some("app"));
    assert!(result.is_nullable());
    assert_member(&mut ws, "Use.kt", "nullable.", "picked", "wrongAny");
    assert_member(
        &mut ws,
        "Use.kt",
        "nullable.picked.",
        "useful",
        "usefulWrong",
    );
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "nullable.picked",
        "Nullable.kt",
        "picked",
    );
    assert_exact_definition(&mut ws, "Use.kt", "nullable.read", "Nullable.kt", "read");
    // Unbounded T? remains an explicit coverage gap: it cannot yet establish the binding of T.
    // The other cases must not fabricate a match across invariant arguments or a non-null bound.
    for (receiver, property) in [
        ("nullable", "nonNull"),
        ("nullable", "wrongAny"),
        ("nullable", "unbounded"),
        ("strict", "picked"),
        ("unknown", "picked"),
    ] {
        assert_member(&mut ws, "Use.kt", &format!("{receiver}."), "body", property);
        let expression = format!("{receiver}.{property}");
        assert_eq!(
            infer_expression(&ws, "Use.kt", &expression),
            Type::Unknown,
            "{expression}"
        );
        let source = ws.doc_text("Use.kt").unwrap();
        let offset = source.find(&expression).unwrap() + receiver.len() + 1;
        assert!(
            ws.goto_definition("Use.kt", offset).is_empty(),
            "{expression}"
        );
    }
}

#[test]
fn expect_actual_receiver_shapes_do_not_require_identical_parent_lists() {
    let mut ws = reflection_workspace(
        r#"package probe
import kotlin.reflect.KClass
fun run(exact: ExactRequest, typed: KClass<Invocation>, other: KClass<Other>, holder: Holder<Subject>) {
    typed.java.cast(null)
    typed.objectInstance
    exact.tag(typed.java)!!.service()
    exact.tag(Invocation::class.java)!!.service()
    other.java
    Subject::class.java.cast(null)
    holder.bounded; holder.body()
}
"#,
    );
    // Kotlin 1.9.23 sources contain these same nominal KClass<T> declarations in two source sets.
    ws.change("KClass.kt", "package kotlin.reflect\npublic actual interface KClass<T : Any> : KDeclarationContainer, KAnnotatedElement, KClassifier {\n public actual val qualifiedName: String?\n public val objectInstance: T?\n}\n".into());
    ws.open("CommonKClass.kt", "package kotlin.reflect\npublic expect interface KClass<T : Any> : KClassifier {\n public val qualifiedName: String?\n}\n".into());
    ws.open("ReflectionParents.kt", "package kotlin.reflect\ninterface KDeclarationContainer\ninterface KAnnotatedElement\ninterface KClassifier\n".into());
    // An unrelated homonym with another arity cannot change the selected package's shape.
    ws.open(
        "UnrelatedKClass.kt",
        "package other\nclass KClass<T, U>\n".into(),
    );
    ws.change(
        "JvmClassMapping.kt",
        r#"@file:JvmName("JvmClassMappingKt")
@file:Suppress("PLATFORM_CLASS_MAPPED_TO_KOTLIN", "UNCHECKED_CAST")
package kotlin.jvm
import kotlin.reflect.KClass
import kotlin.jvm.internal.ClassBasedDeclarationContainer
@Suppress("UPPER_BOUND_VIOLATED")
public val <T> KClass<T>.java: Class<T>
    @JvmName("getJavaClass")
    get() = (this as ClassBasedDeclarationContainer).jClass as Class<T>
"#
        .into(),
    );
    ws.open("SubjectParents.kt", "package probe\nopen class ExpectedBase\nopen class ActualBase : ExpectedBase()\nclass Holder<T> { fun body() {} }\nval <T : ExpectedBase> Holder<T>.bounded: T get() = TODO()\n".into());
    ws.open(
        "ExpectedSubject.kt",
        "package probe\nexpect class Subject : ExpectedBase\n".into(),
    );
    ws.open(
        "ActualSubject.kt",
        "package probe\nactual class Subject : ActualBase()\n".into(),
    );
    for expression in [
        "typed.java",
        "Invocation::class.java",
        "Subject::class.java",
    ] {
        let ty = infer_expression(&ws, "Use.kt", expression);
        let argument = if expression.starts_with("Subject") {
            "Subject"
        } else {
            "Invocation"
        };
        assert_eq!(ty.name(), Some("Class"), "{expression}: {ty:?}");
        assert_eq!(ty.package(), Some("java.lang"));
        assert_eq!(ty.args(), [Type::class(argument, Some("probe".into()))]);
    }
    assert_member(&mut ws, "Use.kt", "typed.ja", "java", "body");
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "typed.java",
        "JvmClassMapping.kt",
        "java",
    );
    assert_member(&mut ws, "Use.kt", "typed.java.", "cast", "service");
    for expression in [
        "exact.tag(typed.java)!!.",
        "exact.tag(Invocation::class.java)!!.",
    ] {
        assert_member(&mut ws, "Use.kt", expression, "service", "serviceWrong");
        assert_exact_definition(
            &mut ws,
            "Use.kt",
            &format!("{expression}service"),
            "Invocation.java",
            "service",
        );
    }
    let own = infer_expression(&ws, "Use.kt", "typed.objectInstance");
    assert_eq!(own.name(), Some("Invocation"));
    assert_eq!(own.package(), Some("probe"));
    assert!(own.is_nullable());
    // The direct shape is known, but different parent lists still leave bound traversal unproved.
    // This valid expect/actual bound remains a coverage gap, not useful-positive acceptance.
    assert_member(&mut ws, "Use.kt", "holder.", "body", "bounded");
    assert_eq!(
        infer_expression(&ws, "Use.kt", "holder.bounded"),
        Type::Unknown
    );
    let source = ws.doc_text("Use.kt").unwrap();
    assert!(ws
        .goto_definition(
            "Use.kt",
            source.find("holder.bounded").unwrap() + "holder.".len()
        )
        .is_empty());
    ws.change("JvmClassMapping.kt", "package kotlin.jvm\nimport kotlin.reflect.KClass\nimport probe.Other\nval KClass<Other>.java: Class<Other> get() = TODO()\n".into());
    assert_member(&mut ws, "Use.kt", "typed.", "qualifiedName", "java");
    assert_eq!(infer_expression(&ws, "Use.kt", "typed.java"), Type::Unknown);
    assert!(ws
        .goto_definition(
            "Use.kt",
            source.find("typed.java").unwrap() + "typed.".len()
        )
        .is_empty());
    let other = infer_expression(&ws, "Use.kt", "other.java");
    assert_eq!(other.name(), Some("Class"));
    assert_eq!(other.args(), [Type::class("Other", Some("probe".into()))]);
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "other.java",
        "JvmClassMapping.kt",
        "java",
    );
}

#[test]
fn conflicting_nominal_shapes_never_become_known_receiver_arguments() {
    for conflicting in [
        "package kotlin.reflect\ninterface KClass<T, U>\n",
        "package kotlin.reflect\nclass KClass<T>\n",
        "package kotlin.reflect\ninterface KClass<U>\n",
    ] {
        let mut ws = reflection_workspace("package probe\nimport kotlin.reflect.KClass\nfun run(typed: KClass<Invocation>, exact: ExactRequest) { typed.java; exact.tag(Invocation::class.java)!!.service() }\n");
        ws.change(
            "KClass.kt",
            "package kotlin.reflect\ninterface KClass<T> { val qualifiedName: String? }\n".into(),
        );
        ws.open("ConflictingKClass.kt", conflicting.into());
        assert_member(&mut ws, "Use.kt", "typed.", "qualifiedName", "java");
        assert_eq!(
            infer_expression(&ws, "Use.kt", "typed.java"),
            Type::Unknown,
            "{conflicting}"
        );
        assert_eq!(
            infer_expression(&ws, "Use.kt", "Invocation::class.java"),
            Type::Unknown,
            "{conflicting}"
        );
        let source = ws.doc_text("Use.kt").unwrap();
        assert!(ws
            .goto_definition(
                "Use.kt",
                source.find("typed.java").unwrap() + "typed.".len()
            )
            .is_empty());
    }
    // Even a fundamental name needs to retain conflicting-declaration uncertainty.
    for conflicting in [
        "package kotlin\ninterface String\n",
        "package kotlin\nclass String<T>\n",
    ] {
        let mut ws = workspace(&[
            ("String.kt", "package kotlin\nclass String\n"),
            ("ConflictingString.kt", conflicting),
            ("Holder.kt", "package app\nclass Holder<T> { fun body() {} }\nval <T> Holder<T>.chosen: T get() = TODO()\n"),
            ("Use.kt", "package app\nfun run(holder: Holder<kotlin.String>) { holder.chosen; holder.body() }\n"),
        ]);
        assert_member(&mut ws, "Use.kt", "holder.", "body", "chosen");
        assert_eq!(
            infer_expression(&ws, "Use.kt", "holder.chosen"),
            Type::Unknown,
            "{conflicting}"
        );
        let source = ws.doc_text("Use.kt").unwrap();
        assert!(ws
            .goto_definition(
                "Use.kt",
                source.find("holder.chosen").unwrap() + "holder.".len()
            )
            .is_empty());
    }
}
