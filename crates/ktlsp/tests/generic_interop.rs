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
