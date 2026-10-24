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

#[test]
fn bounded_method_arguments_share_nominal_bounds_and_method_shadowing() {
    let mut ws = workspace(&[
        (
            "T.java",
            "package app; public class T { public void poison() {} }",
        ),
        (
            "Item.java",
            "package app; public class Item { public void useful() {} }",
        ),
        (
            "Other.java",
            "package app; public class Other { public void usefulWrong() {} }",
        ),
        (
            "Box.java",
            "package app; public class Box<T extends Item> { public <U extends Item> U same(U value) { return value; } public <T extends Other> T shadow(T value) { return value; } public <U extends T> U related(U value) { return value; } }",
        ),
        (
            "KotlinBox.kt",
            "package app\nclass KotlinBox<T: Item> { fun <U: Item> same(value: U): U = value\n fun <T: Other> shadow(value: T): T = value\n fun <U: T> related(value: U): U = value }",
        ),
    ]);
    for (file, source) in [
        (
            "Use.kt",
            "package app\nfun use(java: Box<Item>, kotlin: KotlinBox<Item>, item: Item, other: Other) { java.same(item).useful(); kotlin.same(item).useful(); java.shadow(other).usefulWrong(); kotlin.shadow(other).usefulWrong(); java.same(other).usefulWrong(); kotlin.same(other).usefulWrong(); java.related(item).useful(); kotlin.related(item).useful() }",
        ),
        (
            "Use.java",
            "package app; class Use { void use(Box<Item> java, KotlinBox<Item> kotlin, Item item, Other other) { java.same(item).useful(); kotlin.same(item).useful(); java.shadow(other).usefulWrong(); kotlin.shadow(other).usefulWrong(); java.same(other).usefulWrong(); kotlin.same(other).usefulWrong(); java.related(item).useful(); kotlin.related(item).useful(); } }",
        ),
    ] {
        let source = source.replace("kotlin.related(item).useful()", "kotlin.related(item).useful(); java.related(other).usefulWrong(); kotlin.related(other).usefulWrong()");
        ws.open(file, source.clone());
        for receiver in ["java", "kotlin"] {
            assert_member(
                &mut ws,
                file,
                &format!("{receiver}.same(item).useful"),
                "useful",
                "usefulWrong",
            );
            assert_member(
                &mut ws,
                file,
                &format!("{receiver}.shadow(other).usefulWrong"),
                "usefulWrong",
                "useful",
            );
            assert!(
                !labels(
                    &mut ws,
                    file,
                    &format!("{receiver}.same(other).usefulWrong")
                )
                .contains("usefulWrong")
            );
            assert_member(
                &mut ws,
                file,
                &format!("{receiver}.related(item).useful"),
                "useful",
                "usefulWrong",
            );
            let member = ws
                .index
                .lookup_by_name("useful")
                .iter()
                .find(|entry| entry.path.as_ref() == "Item.java")
                .unwrap();
            let expected = ktlsp::symbol::Def {
                file: "Item.java".into(),
                start_byte: member.sym.start_byte,
                end_byte: member.sym.end_byte,
            };
            let prefix = format!("{receiver}.related(item).useful");
            assert_eq!(
                ws.goto_definition(
                    file,
                    source.find(&prefix).unwrap() + prefix.len() - "useful".len()
                ),
                vec![expected]
            );
            assert!(
                !labels(
                    &mut ws,
                    file,
                    &format!("{receiver}.related(other).usefulWrong")
                )
                .contains("usefulWrong")
            );
        }
        let java_box = ws.doc_text("Box.java").unwrap().to_string();
        let kotlin_box = ws.doc_text("KotlinBox.kt").unwrap().to_string();
        ws.change(
            "Box.java",
            java_box.replace("<U extends T>", "<U extends Other>"),
        );
        ws.change("KotlinBox.kt", kotlin_box.replace("<U: T>", "<U: Other>"));
        for receiver in ["java", "kotlin"] {
            assert_member(
                &mut ws,
                file,
                &format!("{receiver}.related(other).usefulWrong"),
                "usefulWrong",
                "useful",
            );
            assert!(
                !labels(&mut ws, file, &format!("{receiver}.related(item).useful"))
                    .contains("useful")
            );
        }
        ws.change("Box.java", java_box);
        ws.change("KotlinBox.kt", kotlin_box);
        for receiver in ["java", "kotlin"] {
            assert_member(
                &mut ws,
                file,
                &format!("{receiver}.related(item).useful"),
                "useful",
                "usefulWrong",
            );
        }
    }
    for (file, source) in [
        (
            "Raw.java",
            "package app; class Raw { void use(Box java, KotlinBox kotlin, T poison) { java.related(poison).poison(); kotlin.related(poison).poison(); } }",
        ),
        (
            "Star.kt",
            "package app\nfun raw(java: Box<*>, kotlin: KotlinBox<*>, poison: T) { java.related(poison).poison(); kotlin.related(poison).poison() }",
        ),
    ] {
        ws.open(file, source.into());
        for receiver in ["java", "kotlin"] {
            assert!(
                !labels(&mut ws, file, &format!("{receiver}.related(poison).poison"))
                    .contains("poison")
            );
        }
    }
    ws.open("JavaCycle.java", "package app; class JavaCycle { public <U extends V, V extends U> U cycle(U value) { return value; } }".into());
    ws.open(
        "KotlinCycle.kt",
        "package app\nclass KotlinCycle { fun <U: V, V: U> cycle(value: U): U = value }".into(),
    );
    for (file, source) in [
        (
            "Cycles.java",
            "package app; class Cycles { void use(JavaCycle java, KotlinCycle kotlin, Item item) { java.cycle(item).useful(); kotlin.cycle(item).useful(); } }",
        ),
        (
            "Cycles.kt",
            "package app\nfun cycles(java: JavaCycle, kotlin: KotlinCycle, item: Item) { java.cycle(item).useful(); kotlin.cycle(item).useful() }",
        ),
    ] {
        ws.open(file, source.into());
        for receiver in ["java", "kotlin"] {
            assert!(
                !labels(&mut ws, file, &format!("{receiver}.cycle(item).useful"))
                    .contains("useful")
            );
        }
    }
    let mut captured = workspace(&[
        ("Object.java", "package java.lang; public class Object {}"),
        (
            "Item.java",
            "package app; public class Item { public void useful() {} }",
        ),
        (
            "Other.java",
            "package app; public class Other { public void usefulWrong() {} }",
        ),
        (
            "T.java",
            "package app; public class T { public void poison() {} }",
        ),
        (
            "Outer.java",
            "package app; public class Outer<T extends Item> { public class Inner { public <U extends T> U related(U value) { return value; } } }",
        ),
        (
            "KotlinOuter.kt",
            "package app\nclass KotlinOuter<T: Item> { inner class Inner { fun <U: T> related(value: U): U = value } }",
        ),
    ]);
    for (file, source) in [
        (
            "Captured.java",
            "package app; class Captured { void use(Outer<Item>.Inner java, KotlinOuter<Item>.Inner kotlin, Item item, Other other) { java.related(item).useful(); kotlin.related(item).useful(); java.related(other).usefulWrong(); kotlin.related(other).usefulWrong(); } }",
        ),
        (
            "Captured.kt",
            "package app\nfun use(java: Outer<Item>.Inner, kotlin: KotlinOuter<Item>.Inner, item: Item, other: Other) { java.related(item).useful(); kotlin.related(item).useful(); java.related(other).usefulWrong(); kotlin.related(other).usefulWrong() }",
        ),
    ] {
        captured.open(file, source.into());
        for receiver in ["java", "kotlin"] {
            let prefix = format!("{receiver}.related(item).useful");
            assert_member(&mut captured, file, &prefix, "useful", "usefulWrong");
            let item = captured
                .index
                .lookup_by_name("useful")
                .iter()
                .find(|entry| entry.path.as_ref() == "Item.java")
                .unwrap();
            let expected = ktlsp::symbol::Def {
                file: "Item.java".into(),
                start_byte: item.sym.start_byte,
                end_byte: item.sym.end_byte,
            };
            assert_eq!(
                captured.goto_definition(
                    file,
                    source.find(&prefix).unwrap() + prefix.len() - "useful".len()
                ),
                vec![expected]
            );
            assert!(
                !labels(
                    &mut captured,
                    file,
                    &format!("{receiver}.related(other).usefulWrong")
                )
                .contains("usefulWrong")
            );
        }
    }
    for (file, source) in [
        (
            "CapturedRaw.java",
            "package app; class CapturedRaw { void use(Outer.Inner java, KotlinOuter.Inner kotlin, T poison) { java.related(poison).poison(); kotlin.related(poison).poison(); } }",
        ),
        (
            "CapturedStar.kt",
            "package app\nfun raw(java: Outer<*>.Inner, kotlin: KotlinOuter<*>.Inner, poison: T) { java.related(poison).poison(); kotlin.related(poison).poison() }",
        ),
    ] {
        captured.open(file, source.into());
        for receiver in ["java", "kotlin"] {
            assert!(
                !labels(
                    &mut captured,
                    file,
                    &format!("{receiver}.related(poison).poison")
                )
                .contains("poison")
            );
        }
    }
    for (file, source) in [
        (
            "StaticOuter.java",
            "package app; public class StaticOuter<T extends Item> { public static class Inner { public <U extends T> U blocked(U value) { return value; } } }",
        ),
        (
            "StaticMethod.java",
            "package app; public class StaticMethod<T extends Item> { public class Inner { public static <U extends T> U blocked(U value) { return value; } } }",
        ),
        (
            "NestedOuter.kt",
            "package app\nclass NestedOuter<T: Item> { class Inner { fun <U: T> blocked(value: U): U = value } }",
        ),
        (
            "InterfaceOuter.java",
            "package app; public interface InterfaceOuter<T extends Item> { class Inner { public <U extends T> U blocked(U value) { return value; } } }",
        ),
        (
            "AnnotationOuter.java",
            "package app; public class AnnotationOuter<T extends Item> { public @interface Nest { class Inner { public <U extends T> U blocked(U value) { return value; } } } }",
        ),
    ] {
        captured.open(file, source.into());
        let entry = captured
            .index
            .lookup_by_name("blocked")
            .iter()
            .find(|entry| entry.path.as_ref() == file)
            .unwrap();
        assert!(
            entry.sym.method_type_bounds.is_none(),
            "{file}: inaccessible outer type variable"
        );
        assert!(
            entry
                .sym
                .type_parameter_bounds
                .as_deref()
                .unwrap()
                .iter()
                .any(|parameter| !parameter.complete),
            "{file}: raw erasure must retain inaccessible lexical blockers"
        );
    }

    // Raw Java receivers erase method variables too; a SubItem argument must not leak subOnly.
    captured.open("Box.java", "package app; public class Box<T extends Item> { public T get() { return null; } public <U extends T> U related(U value) { return value; } public <U> U independent(U value) { return value; } public static <U> U stat(U value) { return value; } public static Object takeObject(Object value) { return value; } public java.util.List<String> concrete() { return null; } }".into());
    captured.open(
        "List.java",
        "package java.util; public interface List<E> { E get(int i); }".into(),
    );
    captured.open("Outer.java", "package app; public class Outer<T extends Item> { public class Inner { public T get() { return null; } public <U extends T> U related(U value) { return value; } } }".into());
    captured.open(
        "SubItem.java",
        "package app; public class SubItem extends Item { public void subOnly() {} }".into(),
    );
    captured.open("RawFactory.java", "package app; public class RawFactory { public static Box box() { return null; } public static Outer.Inner inner() { return null; } }".into());
    for (file, source, receivers) in [
        (
            "RawValid.java",
            "package app; class RawValid { void use(Box box, Outer.Inner inner, Item item, SubItem sub, Other other, T poison) { box.get().useful(); box.related(item).useful(); box.related(sub).useful(); box.related(sub).subOnly(); box.related(other).usefulWrong(); box.related(poison).poison(); inner.get().useful(); inner.related(item).useful(); inner.related(sub).useful(); inner.related(sub).subOnly(); inner.related(other).usefulWrong(); inner.related(poison).poison(); } }",
            ["box", "inner"],
        ),
        (
            "RawFactoryUse.kt",
            "package app\nfun raw(item: Item, sub: SubItem, other: Other, poison: T) { RawFactory.box().get().useful(); RawFactory.box().related(item).useful(); RawFactory.box().related(sub).useful(); RawFactory.box().related(sub).subOnly(); RawFactory.box().related(other).usefulWrong(); RawFactory.box().related(poison).poison(); RawFactory.inner().get().useful(); RawFactory.inner().related(item).useful(); RawFactory.inner().related(sub).useful(); RawFactory.inner().related(sub).subOnly(); RawFactory.inner().related(other).usefulWrong(); RawFactory.inner().related(poison).poison() }",
            ["RawFactory.box()", "RawFactory.inner()"],
        ),
    ] {
        captured.open(file, source.into());
        for receiver in receivers {
            for call in ["get()", "related(item)", "related(sub)"] {
                let expression = format!("{receiver}.{call}");
                if file.ends_with(".kt") {
                    assert_eq!(
                        infer_expression(&captured, file, &expression),
                        Type::class("Item", Some("app".into())),
                        "{file}: {expression}: raw receiver retains its erased return",
                    );
                }
                assert_member(
                    &mut captured,
                    file,
                    &format!("{expression}."),
                    "useful",
                    "subOnly",
                );
                let entry = captured
                    .index
                    .lookup_by_name("useful")
                    .iter()
                    .find(|entry| entry.path.as_ref() == "Item.java")
                    .unwrap();
                let expected = ktlsp::symbol::Def {
                    file: "Item.java".into(),
                    start_byte: entry.sym.start_byte,
                    end_byte: entry.sym.end_byte,
                };
                let prefix = format!("{expression}.useful");
                assert_eq!(
                    captured.goto_definition(
                        file,
                        source.find(&prefix).unwrap() + prefix.len() - "useful".len()
                    ),
                    vec![expected]
                );
            }
            for (call, forbidden) in [
                ("related(sub).", "subOnly"),
                ("related(other).", "usefulWrong"),
                ("related(poison).", "poison"),
            ] {
                assert!(
                    !labels(&mut captured, file, &format!("{receiver}.{call}")).contains(forbidden)
                );
            }
        }
    }
    captured.open("StarRead.kt", "package app\nfun read(box: Box<*>, inner: Outer<*>.Inner, item: Item) { box.get().useful(); inner.get().useful(); box.related(item).useful(); inner.related(item).useful() }".into());
    for receiver in ["box", "inner"] {
        assert_member(
            &mut captured,
            "StarRead.kt",
            &format!("{receiver}.get().useful"),
            "useful",
            "subOnly",
        );
        assert!(
            !labels(
                &mut captured,
                "StarRead.kt",
                &format!("{receiver}.related(item).useful")
            )
            .contains("useful")
        );
    }
    for (file, source) in [
        (
            "RawIndependent.java",
            "package app; class RawIndependent { void use(Box box, SubItem sub) { box.independent(sub).subOnly(); box.concrete().get(0).length(); Box.stat(sub).subOnly(); Box.stat(box).get().useful(); } }",
        ),
        (
            "RawIndependent.kt",
            "package app\nfun independent(sub:SubItem) { RawFactory.box().independent(sub).subOnly(); RawFactory.box().concrete().get(0).length(); Box.stat(sub).subOnly(); Box.stat(RawFactory.box()).get().useful(); Box.takeObject(RawFactory.box()) }",
        ),
    ] {
        captured.open(file, source.into());
        let raw = if file.ends_with(".java") {
            "box"
        } else {
            "RawFactory.box()"
        };
        assert!(
            !labels(&mut captured, file, &format!("{raw}.independent(sub).")).contains("subOnly")
        );
        assert!(
            !labels(&mut captured, file, &format!("{raw}.concrete().get(0).")).contains("length")
        );
        assert_member(
            &mut captured,
            file,
            "Box.stat(sub).",
            "subOnly",
            "usefulWrong",
        );
        let pass_raw = format!("Box.stat({raw}).get().");
        assert_member(&mut captured, file, &pass_raw, "useful", "subOnly");
        if file.ends_with(".kt") {
            assert_eq!(
                infer_expression(&captured, file, &format!("{raw}.independent(sub)")),
                Type::class("Object", Some("java.lang".into()))
            );
            assert!(matches!(
                infer_expression(&captured, file, &format!("{raw}.concrete()")),
                Type::Raw(_)
            ));
            assert_eq!(
                infer_expression(&captured, file, "Box.takeObject(RawFactory.box())"),
                Type::class("Object", Some("java.lang".into()))
            );
        }
    }
    for (file, source) in [
        (
            "Left.java",
            "package app; public interface Left { void leftOnly(); }",
        ),
        (
            "Right.java",
            "package app; public interface Right { void rightOnly(); }",
        ),
        (
            "ObjectFirst.java",
            "package app; public class ObjectFirst<T extends Object & Left> { public T get() { return null; } }",
        ),
        (
            "InterfaceFirst.java",
            "package app; public class InterfaceFirst<T extends Left & Right> { public T get() { return null; } }",
        ),
        (
            "Chain.java",
            "package app; public class Chain<T extends U,U extends Item> { public T get() { return null; } }",
        ),
        (
            "RawCycle.java",
            "package app; public class RawCycle<T extends T> { public T get() { return null; } }",
        ),
        (
            "RawChild.java",
            "package app; public class RawChild<T extends Item> extends Box<SubItem> {}",
        ),
        (
            "FixedChild.java",
            "package app; public class FixedChild extends Box<SubItem> {}",
        ),
        (
            "RawShadow.java",
            "package app; public class RawShadow<T extends Item,U extends T> { public <T extends Other> U shadow(T value) { return null; } }",
        ),
        (
            "KOrder.kt",
            "package app\ninterface KLeft { fun leftOnly() }\ninterface KRight { fun rightOnly() }\nclass KOrder<T> where T:KLeft,T:Item { fun get():T = TODO() }\nclass KInterfaces<T> where T:KLeft,T:KRight { fun get():T = TODO() }\nclass KUnknown<T> where T:Missing,T:Item { fun get():T = TODO() }\nclass KConflicting<T> where T:Other,T:Item { fun get():T = TODO() }\nclass KAny<T> where T:Any,T:KLeft { fun get():T = TODO() }\nclass KNullableAny<T> where T:Any?,T:KLeft { fun get():T = TODO() }",
        ),
        (
            "KOrderFactory.java",
            "package app; public class KOrderFactory { public static KOrder order() { return null; } public static KInterfaces interfaces() { return null; } public static KUnknown unknown() { return null; } public static KConflicting conflicting() { return null; } public static KAny any() { return null; } public static KNullableAny nullableAny() { return null; } }",
        ),
        (
            "OrderedFactory.java",
            "package app; public class OrderedFactory { public static ObjectFirst objectFirst() { return null; } public static InterfaceFirst interfaceFirst() { return null; } public static Chain chain() { return null; } public static RawChild child() { return null; } public static FixedChild fixed() { return null; } public static RawShadow shadow() { return null; } public static RawCycle cycle() { return null; } }",
        ),
    ] {
        captured.open(file, source.into());
    }
    for (file, source, raw, interface, chain, child, fixed, shadow, cycle) in [
        (
            "OrderedRaw.java",
            "package app; class OrderedRaw { void use(ObjectFirst a, InterfaceFirst b, Chain c, RawChild d, FixedChild e, RawShadow f, RawCycle g, Other other) { a.get().leftOnly(); b.get().leftOnly(); b.get().rightOnly(); c.get().useful(); d.get().useful(); d.get().subOnly(); e.get().subOnly(); f.shadow(other).useful(); g.get().poison(); } }",
            "a",
            "b",
            "c",
            "d",
            "e",
            "f",
            "g",
        ),
        (
            "OrderedRaw.kt",
            "package app\nfun ordered(other:Other) { OrderedFactory.objectFirst().get().leftOnly(); OrderedFactory.interfaceFirst().get().leftOnly(); OrderedFactory.interfaceFirst().get().rightOnly(); OrderedFactory.chain().get().useful(); OrderedFactory.child().get().useful(); OrderedFactory.child().get().subOnly(); OrderedFactory.fixed().get().subOnly(); OrderedFactory.shadow().shadow(other).useful(); OrderedFactory.cycle().get().poison() }",
            "OrderedFactory.objectFirst()",
            "OrderedFactory.interfaceFirst()",
            "OrderedFactory.chain()",
            "OrderedFactory.child()",
            "OrderedFactory.fixed()",
            "OrderedFactory.shadow()",
            "OrderedFactory.cycle()",
        ),
    ] {
        captured.open(file, source.into());
        assert!(!labels(&mut captured, file, &format!("{raw}.get().")).contains("leftOnly"));
        assert_member(
            &mut captured,
            file,
            &format!("{interface}.get()."),
            "leftOnly",
            "rightOnly",
        );
        assert_member(
            &mut captured,
            file,
            &format!("{chain}.get()."),
            "useful",
            "subOnly",
        );
        assert_member(
            &mut captured,
            file,
            &format!("{child}.get()."),
            "useful",
            "subOnly",
        );
        assert_member(
            &mut captured,
            file,
            &format!("{fixed}.get()."),
            "subOnly",
            "usefulWrong",
        );
        assert_member(
            &mut captured,
            file,
            &format!("{shadow}.shadow(other)."),
            "useful",
            "usefulWrong",
        );
        assert!(!labels(&mut captured, file, &format!("{cycle}.get().")).contains("poison"));
        if file.ends_with(".kt") {
            assert_eq!(
                infer_expression(&captured, file, &format!("{raw}.get()")),
                Type::class("Object", Some("java.lang".into()))
            );
        }
    }
    for (file, source) in [
        (
            "KOrderUse.java",
            "package app; class KOrderUse { void use(KOrder order, KInterfaces interfaces, KUnknown unknown, KConflicting conflicting) { order.get().useful(); order.get().leftOnly(); interfaces.get().leftOnly(); interfaces.get().rightOnly(); unknown.get().useful(); conflicting.get().useful(); } }",
        ),
        (
            "KOrderUse.kt",
            "package app\nfun kotlinOrder() { KOrderFactory.order().get().leftOnly(); KOrderFactory.order().get().useful(); KOrderFactory.interfaces().get().leftOnly(); KOrderFactory.interfaces().get().rightOnly(); KOrderFactory.unknown().get().useful(); KOrderFactory.conflicting().get().useful() }",
        ),
    ] {
        captured.open(file, source.into());
        let (order, interfaces, unknown, conflicting) = if file.ends_with(".java") {
            ("order", "interfaces", "unknown", "conflicting")
        } else {
            (
                "KOrderFactory.order()",
                "KOrderFactory.interfaces()",
                "KOrderFactory.unknown()",
                "KOrderFactory.conflicting()",
            )
        };
        let (expected, forbidden) = if file.ends_with(".java") {
            ("useful", "leftOnly")
        } else {
            ("leftOnly", "useful")
        };
        assert_member(
            &mut captured,
            file,
            &format!("{order}.get()."),
            expected,
            forbidden,
        );
        assert_member(
            &mut captured,
            file,
            &format!("{interfaces}.get()."),
            "leftOnly",
            "rightOnly",
        );
        assert!(!labels(&mut captured, file, &format!("{unknown}.get().")).contains("useful"));
        assert!(!labels(&mut captured, file, &format!("{conflicting}.get().")).contains("useful"));
        if file.ends_with(".kt") {
            assert_eq!(
                infer_expression(&captured, file, &format!("{order}.get()")),
                Type::class("KLeft", Some("app".into()))
            );
        }
    }
    let factory = captured
        .index
        .lookup_by_name("order")
        .iter()
        .find(|entry| entry.path.as_ref() == "KOrderFactory.java")
        .unwrap();
    let receiver = infer::declared_type(
        &captured.index,
        factory.sym.return_type.as_ref().unwrap(),
        &FileCtx::new("app".into(), Vec::new()).with_file("KOrderFactory.java"),
    );
    assert!(matches!(receiver, Type::Raw(_)));
    let get = captured
        .index
        .lookup_by_name("get")
        .iter()
        .find(|entry| {
            entry.path.as_ref() == "KOrder.kt" && entry.sym.container.as_deref() == Some("KOrder")
        })
        .unwrap();
    for (mode, expected) in [
        (ktcore::members::CallMode::Java, "Item"),
        (ktcore::members::CallMode::Kotlin, "KLeft"),
    ] {
        let ctx = FileCtx::new("app".into(), Vec::new());
        let tr = get.sym.return_type.as_ref().unwrap();
        let expected = Type::class(expected, Some("app".into()));
        assert_eq!(
            infer::declared_member_type(&captured.index, &receiver, get, tr, mode, &ctx),
            expected
        );
        assert_eq!(
            infer::declared_member_call_type(
                &captured.index,
                &receiver,
                get,
                tr,
                &ktcore::members::CallArguments {
                    count: Some(0),
                    types: Vec::new(),
                    names: None,
                    has_spread: false,
                    trailing_lambda: false,
                    trailing_lambda_arguments: None,
                },
                mode,
                &ctx
            ),
            expected
        );
    }
    captured.open("KDefault.kt", "package app\nclass KUnbounded<T> { fun get():T = TODO() }\nclass KMethod<T:Item> { fun <U> independent(value:U):U = value }".into());
    captured.open("KDefaultFactory.java", "package app; public class KDefaultFactory { public static KUnbounded unbounded() { return null; } public static KMethod method() { return null; } }".into());
    let roots = "package app\nfun roots(sub:SubItem) { KOrderFactory.any().get(); KOrderFactory.nullableAny().get(); KDefaultFactory.method().independent(sub); KDefaultFactory.unbounded().get() }";
    captured.open("KRoots.kt", roots.into());
    for expression in [
        "KDefaultFactory.method().independent(sub)",
        "KDefaultFactory.unbounded().get()",
    ] {
        assert_eq!(
            infer_expression(&captured, "KRoots.kt", expression),
            Type::class("Any", Some("kotlin".into())).with_nullability(true)
        );
    }
    for (factory, nullable) in [("any", false), ("nullableAny", true)] {
        assert_eq!(
            infer_expression(
                &captured,
                "KRoots.kt",
                &format!("KOrderFactory.{factory}().get()")
            ),
            Type::class("Any", Some("kotlin".into())).with_nullability(nullable)
        );
        let factory = captured
            .index
            .lookup_by_name(factory)
            .iter()
            .find(|entry| entry.path.as_ref() == "KOrderFactory.java")
            .unwrap();
        let ctx = FileCtx::new("app".into(), Vec::new());
        let receiver = infer::declared_type(
            &captured.index,
            factory.sym.return_type.as_ref().unwrap(),
            &ctx,
        );
        let get = captured
            .index
            .lookup_by_name("get")
            .iter()
            .find(|entry| {
                entry.path.as_ref() == "KOrder.kt"
                    && entry.sym.container.as_deref() == receiver.name()
            })
            .unwrap();
        assert_eq!(
            infer::declared_member_type(
                &captured.index,
                &receiver,
                get,
                get.sym.return_type.as_ref().unwrap(),
                ktcore::members::CallMode::Java,
                &ctx
            ),
            Type::class("Object", Some("java.lang".into()))
        );
    }
}

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
    infer::infer(
        &ws.index,
        node,
        &src,
        &FileCtx::from_tree_in_file(&tree, &src, file),
    )
}

const BOUND_LEVEL: &str = "package app\ninterface Level {\n fun getEnchantmentLevel(): Int\n}\n";
const BOUND_MOVE: &str = "package app; public interface Move { void move(); }";
const BOUND_OTHER: &str = "package app\ninterface Wrong {\n fun wrongOnly(): Int\n}\n";

fn bounds_workspace(file: &str, source: &str) -> Workspace {
    workspace(&[
        ("Level.kt", BOUND_LEVEL),
        ("Move.java", BOUND_MOVE),
        ("Wrong.kt", BOUND_OTHER),
        ("Item.kt", ITEM),
        ("Other.kt", OTHER),
        ("Box.java", JAVA_BOX),
        (
            "GlobalT.kt",
            "package app\nclass T {\n fun poisoned() {}\n}\n",
        ),
        (file, source),
    ])
}

#[test]
fn lexical_class_and_method_bounds_share_completion_navigation_and_inference() {
    for (file, source) in [
        (
            "Use.kt",
            "package app\nclass Use<T>(private val value: T) where T : Move, T : Level {\n fun run() { value.move(); value.getEnchantmentLevel() }\n}\n",
        ),
        (
            "Use.kt",
            "package app\nfun <T : Move> run(value: T) where T : Level { value.move(); value.getEnchantmentLevel() }\n",
        ),
        (
            "Use.java",
            "package app; class Use<T extends Move & Level> { T value; void run() { value.move(); value.getEnchantmentLevel(); } }",
        ),
        (
            "Use.java",
            "package app; class Use { <T extends Move & Level> void run(T value) { value.move(); value.getEnchantmentLevel(); } }",
        ),
    ] {
        let mut ws = bounds_workspace(file, source);
        for (name, target) in [("move", "Move.java"), ("getEnchantmentLevel", "Level.kt")] {
            assert_member(&mut ws, file, "value.", name, "poisoned");
            assert_exact_definition(&mut ws, file, &format!("value.{name}()"), target, name);
        }
        if file.ends_with(".kt") {
            assert_eq!(
                infer_expression(&ws, file, "value.getEnchantmentLevel()"),
                Type::class("Int", Some("kotlin".into()))
            );
        }
    }
}

#[test]
fn generic_bound_arguments_use_the_shared_receiver_substitution() {
    for (file, source) in [
        (
            "Use.kt",
            "package app\nclass Use<T>(val value: T) where T : Box<Item>, T : Level {\n fun run() { value.get().useful(); value.getEnchantmentLevel() }\n}\n",
        ),
        (
            "Use.java",
            "package app; class Use<T extends Box<Item> & Level> { T value; void run() { value.get().useful(); value.getEnchantmentLevel(); } }",
        ),
    ] {
        let mut ws = bounds_workspace(file, source);
        assert_member(&mut ws, file, "value.get().use", "useful", "usefulWrong");
        assert_exact_definition(&mut ws, file, "value.get().useful()", "Item.kt", "useful");
        if file.ends_with(".kt") {
            assert_eq!(
                infer_expression(&ws, file, "value.get()"),
                Type::class("Item", Some("app".into()))
            );
        }
    }
}

#[test]
fn lexical_bounds_shadow_class_parameters_imports_and_global_type_names() {
    for (file, source) in [
        (
            "Use.kt",
            "package app\nimport poison.T\nclass Use<T : Wrong>(val outer: T) {\n fun <T : Level> run(inner: T) { outer.wrongOnly(); inner.getEnchantmentLevel() }\n}\n",
        ),
        (
            "Use.java",
            "package app; import poison.T; class Use<T extends Wrong> { T outer; <T extends Level> void run(T inner) { outer.wrongOnly(); inner.getEnchantmentLevel(); } }",
        ),
    ] {
        let mut ws = bounds_workspace(file, source);
        ws.open(
            "Poison.java",
            "package poison; public class T { public void poisoned() {} }".into(),
        );
        assert_member(&mut ws, file, "outer.", "wrongOnly", "getEnchantmentLevel");
        assert_member(&mut ws, file, "inner.", "getEnchantmentLevel", "wrongOnly");
        assert!(!labels(&mut ws, file, "inner.").contains("poisoned"));
        assert_exact_definition(
            &mut ws,
            file,
            "inner.getEnchantmentLevel()",
            "Level.kt",
            "getEnchantmentLevel",
        );
    }
}

#[test]
fn nested_and_outer_bounds_resolve_in_their_declaration_scope() {
    let source = "package app\nclass Use<T : Level, U : T>(val outer: U) {\n inner class Child<T : Wrong>(val inner: T) {\n  fun run() { val alias: U = outer; alias.getEnchantmentLevel(); inner.wrongOnly() }\n }\n}\n";
    let mut ws = bounds_workspace("Use.kt", source);
    assert_member(
        &mut ws,
        "Use.kt",
        "alias.",
        "getEnchantmentLevel",
        "wrongOnly",
    );
    assert_member(
        &mut ws,
        "Use.kt",
        "inner.",
        "wrongOnly",
        "getEnchantmentLevel",
    );
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "alias.getEnchantmentLevel()",
        "Level.kt",
        "getEnchantmentLevel",
    );
}

#[test]
fn parameter_identity_requires_the_real_file_and_declaration_span() {
    let source = "package app\nfun <T : Level> run(value: T) { value.getEnchantmentLevel() }\n";
    let mut ws = bounds_workspace("First.kt", source);
    ws.open("Second.kt", source.into());
    let mut parser = KotlinParser::new();
    let tree = parser.parse(source);
    let start = source.find("value.getEnchantmentLevel").unwrap();
    let receiver = tree
        .root_node()
        .descendant_for_byte_range(start, start + "value".len())
        .unwrap();
    let first = infer::infer(
        &ws.index,
        receiver,
        source,
        &FileCtx::from_tree_in_file(&tree, source, "First.kt"),
    );
    let second = infer::infer(
        &ws.index,
        receiver,
        source,
        &FileCtx::from_tree_in_file(&tree, source, "Second.kt"),
    );
    let Type::Parameter { definition, .. } = &first else {
        panic!("missing parameter identity: {first:?}")
    };
    assert_eq!(definition.file, "First.kt");
    assert_eq!(&source[definition.start_byte..definition.end_byte], "T");
    assert_ne!(
        first, second,
        "same spelling and byte positions in different files are distinct parameters"
    );
    assert_eq!(
        infer::infer(
            &ws.index,
            receiver,
            source,
            &FileCtx::from_tree(&tree, source)
        ),
        Type::Unknown
    );
    let mut visited = Vec::new();
    let _ = infer::walk_supertypes(&ws.index, &first, 0, |ty| {
        visited.push(ty.clone());
        std::ops::ControlFlow::Continue(())
    });
    assert_eq!(
        visited,
        [Type::Unknown],
        "the bound expansion obeys the shared traversal cap"
    );
}

#[test]
fn unsaved_bound_and_bound_member_edits_replace_both_language_surfaces() {
    for (file, source, before, after) in [
        (
            "Use.kt",
            "package app\nclass Use<T>(val value: T) where T : Move, T : Level {\n fun run() { value.getEnchantmentLevel() }\n}\n",
            "T : Level",
            "T : Wrong",
        ),
        (
            "Use.java",
            "package app; class Use<T extends Move & Level> { T value; void run() { value.getEnchantmentLevel(); } }",
            "& Level",
            "& Wrong",
        ),
    ] {
        let mut ws = bounds_workspace(file, source);
        for changed in [source.replace(before, after), source.to_string()] {
            ws.change(file, changed.clone());
            let restored = changed == source;
            assert_member(
                &mut ws,
                file,
                "value.",
                if restored {
                    "getEnchantmentLevel"
                } else {
                    "wrongOnly"
                },
                if restored {
                    "wrongOnly"
                } else {
                    "getEnchantmentLevel"
                },
            );
            if restored {
                assert_exact_definition(
                    &mut ws,
                    file,
                    "value.getEnchantmentLevel()",
                    "Level.kt",
                    "getEnchantmentLevel",
                );
            } else {
                assert!(
                    ws.goto_definition(
                        file,
                        changed.find("value.getEnchantmentLevel").unwrap() + "value.".len()
                    )
                    .is_empty()
                );
            }
        }
        ws.change(
            "Level.kt",
            BOUND_LEVEL.replace("getEnchantmentLevel", "renamedLevel"),
        );
        assert_member(
            &mut ws,
            file,
            "value.",
            "renamedLevel",
            "getEnchantmentLevel",
        );
        ws.change("Level.kt", BOUND_LEVEL.into());
        assert_member(
            &mut ws,
            file,
            "value.",
            "getEnchantmentLevel",
            "renamedLevel",
        );
    }
}

#[test]
fn incomplete_bodies_keep_valid_bounds_but_invalid_or_unmodeled_bounds_stay_unknown() {
    for (file, source) in [
        (
            "Use.kt",
            "package app\nclass Use<T : Level>(val value: T) {\n fun run() { value.getEnchantmentLevel(); val broken = }\n}\n",
        ),
        (
            "Use.java",
            "package app; class Use<T extends Level> { T value; void run() { value.getEnchantmentLevel(); Object broken = ; } }",
        ),
    ] {
        let mut ws = bounds_workspace(file, source);
        assert_member(&mut ws, file, "value.", "getEnchantmentLevel", "poisoned");
        assert_exact_definition(
            &mut ws,
            file,
            "value.getEnchantmentLevel()",
            "Level.kt",
            "getEnchantmentLevel",
        );
    }
    for (file, source) in [
        (
            "Use.kt",
            "package app\nclass Use<T>(val value: T) {\n fun run() { value.poisoned() }\n}\n",
        ),
        (
            "Use.kt",
            "package app\nclass Use<T : Missing>(val value: T) {\n fun run() { value.getEnchantmentLevel() }\n}\n",
        ),
        (
            "Use.kt",
            "package app\nclass Use<T>(val value: T) where T : Move, T : Missing {\n fun run() { value.move() }\n}\n",
        ),
        (
            "Use.kt",
            "package app\nclass Use<T : U, U : T>(val value: T) {\n fun run() { value.poisoned() }\n}\n",
        ),
        (
            "Use.kt",
            "package app\nclass Use<T : Move, T : Level>(val value: T) {\n fun run() { value.move() }\n}\n",
        ),
        (
            "Use.kt",
            "package app\nclass Use<T>(val value: T) where T : Level, Missing : Wrong {\n fun run() { value.getEnchantmentLevel() }\n}\n",
        ),
        (
            "Use.kt",
            "package app\nclass Use<T : Recursive<T>>(val value: T) {\n fun run() { value.getEnchantmentLevel() }\n}\n",
        ),
        (
            "Use.java",
            "package app; class Use<T> { T value; void run() { value.poisoned(); } }",
        ),
        (
            "Use.java",
            "package app; class Use<T extends Missing> { T value; void run() { value.getEnchantmentLevel(); } }",
        ),
        (
            "Use.java",
            "package app; class Use<T extends Move & Missing> { T value; void run() { value.move(); } }",
        ),
        (
            "Use.java",
            "package app; class Use<T extends U, U extends T> { T value; void run() { value.poisoned(); } }",
        ),
        (
            "Use.java",
            "package app; class Use<T extends Move, T extends Level> { T value; void run() { value.move(); } }",
        ),
        (
            "Use.java",
            "package app; class Use<T extends Recursive<T>> { T value; void run() { value.getEnchantmentLevel(); } }",
        ),
    ] {
        let mut ws = bounds_workspace(file, source);
        ws.open(
            "Recursive.java",
            "package app; public interface Recursive<T> { int getEnchantmentLevel(); }".into(),
        );
        assert!(
            labels(&mut ws, file, "value.").is_empty(),
            "{file}: {source}"
        );
    }
}

#[test]
fn nullable_bound_receivers_preserve_safe_navigation_result_nullability() {
    let source = "package app\nfun <T : Level?> run(value: T) { value?.getEnchantmentLevel() }\n";
    let mut ws = bounds_workspace("Use.kt", source);
    assert_member(
        &mut ws,
        "Use.kt",
        "value?.",
        "getEnchantmentLevel",
        "poisoned",
    );
    let result = infer_expression(&ws, "Use.kt", "value?.getEnchantmentLevel()");
    assert_eq!(result.name(), Some("Int"));
    assert!(
        result.is_nullable(),
        "safe navigation lost nullable T: {result:?}"
    );
}

#[test]
fn conflicting_bound_results_do_not_choose_the_first_declaration() {
    let mut ws = bounds_workspace(
        "Use.kt",
        "package app\nfun <T> run(value: T) where T : Left, T : Right { value.read().useful() }\n",
    );
    ws.open(
        "Left.java",
        "package app; public interface Left { Item read(); }".into(),
    );
    ws.open(
        "Right.java",
        "package app; public interface Right { Other read(); }".into(),
    );
    assert_eq!(
        infer_expression(&ws, "Use.kt", "value.read()"),
        Type::Unknown
    );
    assert!(labels(&mut ws, "Use.kt", "value.read().use").is_empty());
}

#[test]
fn local_classes_capture_function_bounds_but_static_nested_scopes_do_not() {
    let source = "package app\nfun <T : Level> run(value: T) {\n class Local(val item: T) { fun read() { item.getEnchantmentLevel() } }\n}\n";
    let mut ws = bounds_workspace("Use.kt", source);
    assert_member(
        &mut ws,
        "Use.kt",
        "item.",
        "getEnchantmentLevel",
        "poisoned",
    );
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "item.getEnchantmentLevel()",
        "Level.kt",
        "getEnchantmentLevel",
    );
    for (file, source) in [
        (
            "Use.kt",
            "package app\nclass Use<T : Level> { class Nested(val value: T) { fun run() { value.getEnchantmentLevel() } } }\n",
        ),
        (
            "Use.java",
            "package app; class Use<T extends Level> { static class Nested { T value; void run() { value.getEnchantmentLevel(); } } }",
        ),
    ] {
        let mut ws = bounds_workspace(file, source);
        assert!(
            labels(&mut ws, file, "value.").is_empty(),
            "{file}: inaccessible outer parameter leaked"
        );
    }
}

#[test]
fn bounds_expose_only_proven_public_instance_members() {
    for (file, source) in [
        (
            "Use.kt",
            "package app\nfun <T : Access> run(value: T) { value.publicOnly(); value.privateOnly(); value.publicField; value.privateField }\n",
        ),
        (
            "Use.java",
            "package app; class Use { <T extends Access> void run(T value) { value.publicOnly(); value.privateOnly(); int result = value.publicField + value.privateField; } }",
        ),
    ] {
        let mut ws = bounds_workspace(file, source);
        ws.open("Access.java", "package app; public class Access { public int publicField; private int privateField; public void publicOnly() {} private void privateOnly() {} protected void protectedOnly() {} void packageOnly() {} public static void staticOnly() {} }".into());
        let got = labels(&mut ws, file, "value.");
        assert!(got.contains("publicOnly"), "{file}: {got:?}");
        for name in [
            "privateOnly",
            "protectedOnly",
            "packageOnly",
            "staticOnly",
            "privateField",
        ] {
            assert!(
                !got.contains(name),
                "{file}: member outside the proven bound surface: {name}: {got:?}"
            );
        }
        assert_exact_definition(
            &mut ws,
            file,
            "value.publicOnly()",
            "Access.java",
            "publicOnly",
        );
        assert_exact_definition(
            &mut ws,
            file,
            "value.publicField",
            "Access.java",
            "publicField",
        );
        assert!(
            ws.goto_definition(
                file,
                source.find("value.privateOnly").unwrap() + "value.".len()
            )
            .is_empty()
        );
        assert!(
            ws.goto_definition(
                file,
                source.find("value.privateField").unwrap() + "value.".len()
            )
            .is_empty()
        );
    }
}

#[test]
fn bounded_receivers_do_not_promote_unproven_extension_applicability() {
    let source = "package app\nfun Level.extensionOnly(): Int = 1\nfun run(known: Level) { known.extensionOnly() }\nfun <T : Level> runBound(value: T) { value.extensionOnly() }\n";
    let mut ws = bounds_workspace("Use.kt", source);
    assert_member(&mut ws, "Use.kt", "known.", "extensionOnly", "poisoned");
    assert_member(
        &mut ws,
        "Use.kt",
        "value.",
        "getEnchantmentLevel",
        "extensionOnly",
    );
    assert_eq!(
        infer_expression(&ws, "Use.kt", "value.extensionOnly()"),
        Type::Unknown
    );
    assert!(ws
        .goto_definition(
            "Use.kt",
            source.find("value.extensionOnly").unwrap() + "value.".len()
        )
        .is_empty());
    ws.open(
        "Access.kt",
        "package app\nclass Access { fun publicOnly() {} fun Wrong.memberExtensionOnly() {} }\n"
            .into(),
    );
    let source =
        "package app\nfun <T : Access> runMember(value: T) { value.memberExtensionOnly() }\n";
    ws.open("MemberUse.kt", source.into());
    assert_member(
        &mut ws,
        "MemberUse.kt",
        "value.",
        "publicOnly",
        "memberExtensionOnly",
    );
    assert_eq!(
        infer_expression(&ws, "MemberUse.kt", "value.memberExtensionOnly()"),
        Type::Unknown
    );
    assert!(ws
        .goto_definition(
            "MemberUse.kt",
            source.find("value.memberExtensionOnly").unwrap() + "value.".len()
        )
        .is_empty());
}

#[test]
fn lexical_parameter_identity_survives_existing_implicit_receiver_routes() {
    let source = "package app\nfun <T : Level> T.inside() { this.getEnchantmentLevel() }\nfun <T : Level> run(value: T) { value.let { it.getEnchantmentLevel() }; value.apply { this.getEnchantmentLevel() } }\n";
    let mut ws = bounds_workspace("Use.kt", source);
    for prefix in ["this.", "it.", "value.apply { this."] {
        assert_member(&mut ws, "Use.kt", prefix, "getEnchantmentLevel", "poisoned");
    }
    for usage in [
        "this.getEnchantmentLevel()",
        "it.getEnchantmentLevel()",
        "value.apply { this.getEnchantmentLevel()",
    ] {
        assert_exact_definition(&mut ws, "Use.kt", usage, "Level.kt", "getEnchantmentLevel");
    }
    assert_eq!(
        infer_expression(&ws, "Use.kt", "it.getEnchantmentLevel()"),
        Type::class("Int", Some("kotlin".into()))
    );
}

#[test]
fn repeated_parameter_bound_expansion_stays_unknown_with_bounded_work() {
    let mut source = String::from("package app\nclass Use<");
    for level in 0..7 {
        source.push_str(&format!(
            "T{level} : Repeated<{}>, ",
            vec![format!("T{}", level + 1); 8].join(", ")
        ));
    }
    source.push_str("T7 : Level>(val value: T0) { fun run() { value.getEnchantmentLevel() } }\n");
    let mut ws = bounds_workspace("Use.kt", &source);
    ws.open(
        "Repeated.java",
        "package app; public interface Repeated<A,B,C,D,E,F,G,H> { int getEnchantmentLevel(); }"
            .into(),
    );
    assert!(labels(&mut ws, "Use.kt", "value.").is_empty());
    assert_eq!(
        infer_expression(&ws, "Use.kt", "value.getEnchantmentLevel()"),
        Type::Unknown
    );
}

#[test]
fn constructor_initializers_do_not_borrow_their_own_local_binder() {
    let source = "package app\nfun run() { val Dog = Dog(); Dog.bark() }\n";
    let mut ws = bounds_workspace("Use.kt", source);
    ws.open(
        "Dog.kt",
        "package app\nclass Dog { fun bark(): Int = 1 }\n".into(),
    );
    assert_member(&mut ws, "Use.kt", "Dog.", "bark", "poisoned");
    assert_exact_definition(&mut ws, "Use.kt", "val Dog = Dog()", "Dog.kt", "Dog");
    assert_eq!(
        infer_expression(&ws, "Use.kt", "Dog.bark()"),
        Type::class("Int", Some("kotlin".into()))
    );
    for source in [
        "package app\nfun run() { val Dog = missingValue; Dog.bark() }\n",
        "package app\nfun run(Dog: Item) { Dog() }\n",
        "package app\nfun run() { val Dog = Item(); Dog() }\n",
    ] {
        ws.change("Use.kt", source.into());
        let expression = if source.contains("Dog.bark()") {
            "Dog.bark()"
        } else {
            "Dog()"
        };
        assert_eq!(
            infer_expression(&ws, "Use.kt", expression),
            Type::Unknown,
            "{source}"
        );
    }
    ws.open(
        "Built.java",
        "package external; public class Built { public Built() {} public void useful() {} }".into(),
    );
    ws.open(
        "Generic.java",
        "package external; public class Generic<T> { public Generic() {} public void useful() {} }"
            .into(),
    );
    ws.open(
        "Contract.java",
        "package external; public interface Contract { void useful(); }".into(),
    );
    ws.open(
        "Singleton.kt",
        "package external\nobject Singleton { fun useful() {} }".into(),
    );
    ws.open(
        "Choice.java",
        "package external; public enum Choice { VALUE; public void useful() {} }".into(),
    );
    let source = "package app\nfun run() { val built = external.Built(); built.useful() }";
    ws.change("Use.kt", source.into());
    assert_member(&mut ws, "Use.kt", "built.", "useful", "poisoned");
    assert_exact_definition(&mut ws, "Use.kt", "built.useful()", "Built.java", "useful");
    assert_eq!(
        infer_expression(&ws, "Use.kt", "external.Built()"),
        Type::class("Built", Some("external".into()))
    );
    for source in [
        "package app\nfun run(external: Item) { val built = external.Built(); built.useful() }",
        "package app\nfun run() { val external = missing; val built = external.Built(); built.useful() }",
        "package app\nval external = missing\nfun run() { val built = external.Built(); built.useful() }",
        "package app\nimport app.Other as external\nfun run() { val built = external.Built(); built.useful() }",
        "package app\nfun Missing.run() { val built = external.Built(); built.useful() }",
        "package app\nfun run() { val built = external.Generic(); built.useful() }",
        "package app\nfun run() { val built = external.Contract(); built.useful() }",
        "package app\nfun run() { val built = external.Singleton(); built.useful() }",
        "package app\nfun run() { val built = external.Choice(); built.useful() }",
        "package app\nfun run() { val built = external.Built(unknown); built.useful() }",
    ] {
        ws.change("Use.kt", source.into());
        assert!(
            !labels(&mut ws, "Use.kt", "built.").contains("useful"),
            "{source}"
        );
    }
    let bare = "package app\nfun run() { val bare = external.Built }";
    ws.change("Use.kt", bare.into());
    let position = bare.find("external.Built").unwrap() + "external.".len();
    assert!(ws.goto_definition("Use.kt", position).is_empty());
    ws.open(
        "Duplicate.java",
        "package external; public class Built { public Built() {} public void useful() {} }".into(),
    );
    ws.change(
        "Use.kt",
        "package app\nfun run() { val built = external.Built(); built.useful() }".into(),
    );
    assert!(labels(&mut ws, "Use.kt", "built.").is_empty());

    let mut overloaded = workspace(&[
        (
            "Built.java",
            "package p; public class Built { public <T> Built(T value) {} }",
        ),
        (
            "Use.kt",
            "package p\nclass Other { fun otherOnly() {} }\nfun Built(): Other = Other()\nfun use() { p.Built().otherOnly() }",
        ),
    ]);
    assert_member(
        &mut overloaded,
        "Use.kt",
        "p.Built().",
        "otherOnly",
        "poisoned",
    );
    let mut visibility = workspace(&[
        (
            "Built.java",
            "package p; public class Built { public Built() {} public void useful() {} }",
        ),
        (
            "Hidden.kt",
            "package p\nclass Other { fun otherOnly() {} }\nprivate fun Built(): Other = Other()",
        ),
        (
            "HiddenOuter.java",
            "package p; class HiddenOuter { public static class Built { public Built() {} public void useful() {} } }",
        ),
        (
            "PublicOuter.java",
            "package p; public class PublicOuter { public static class Built { public Built() {} public void useful() {} } }",
        ),
        (
            "Use.kt",
            "package app\nfun use() { p.Built().useful() }",
        ),
    ]);
    assert_member(
        &mut visibility,
        "Use.kt",
        "p.Built().",
        "useful",
        "otherOnly",
    );
    visibility.change(
        "Use.kt",
        "package app\nfun use() { p.PublicOuter.Built().useful() }".into(),
    );
    assert_member(
        &mut visibility,
        "Use.kt",
        "p.PublicOuter.Built().",
        "useful",
        "poisoned",
    );
    visibility.change(
        "Use.kt",
        "package app\nfun use() { p.HiddenOuter.Built().useful() }".into(),
    );
    assert!(labels(&mut visibility, "Use.kt", "p.HiddenOuter.Built().").is_empty());
    visibility.change(
        "Hidden.kt",
        "package p\nclass Other { fun otherOnly() {} }\nprivate fun Built(): Other = Other()\nfun use() { p.Built().otherOnly() }".into(),
    );
    assert!(labels(&mut visibility, "Hidden.kt", "p.Built().").is_empty());
    visibility.change(
        "Built.java",
        "package p; public class Built { public <T> Built(T value) {} public void useful() {} }"
            .into(),
    );
    assert_member(
        &mut visibility,
        "Hidden.kt",
        "p.Built().",
        "otherOnly",
        "useful",
    );
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
        ("Item.kt", ITEM),
        ("Other.kt", OTHER),
        ("Box.java", JAVA_BOX),
        (
            "Factory.java",
            "package app; public class Factory { public Box<Item> make() { return null; } }",
        ),
        (
            "Use.kt",
            "package app\nfun run(factory: Factory) { factory.make().get().useful() }\n",
        ),
        (
            "Use.java",
            "package app; class Use { void run(Factory factory) { factory.make().get().useful(); } }",
        ),
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
        ("Item.kt", ITEM),
        ("Other.kt", OTHER),
        (
            "Types.kt",
            "package app\nopen class PairBox<K, V> {\n fun get(): V = error(\"fixture\")\n}\nopen class Middle<X, Y> : PairBox<Y, X>()\nclass Concrete : Middle<Item, Other>()\n",
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
        (
            "LocalItem.kt",
            "package app\nclass Item {\n fun usefulWrong() {}\n}\n",
        ),
        (
            "ImportedItem.kt",
            "package model\nclass Item {\n fun useful() {}\n}\n",
        ),
        ("Box.java", JAVA_BOX),
        (
            "JavaFactory.java",
            "package app; public class JavaFactory { public Box<model.Item> make() { return null; } }",
        ),
        (
            "KotlinFactory.kt",
            "package app\nclass KotlinFactory {\n fun make(): Box<model.Item> = error(\"fixture\")\n}\n",
        ),
        (
            "Use.kt",
            "package app\nfun run(java: JavaFactory, kotlin: KotlinFactory) { java.make().get().useful(); kotlin.make().get().useful() }\n",
        ),
        (
            "Use.java",
            "package app; class Use { void run(JavaFactory java, KotlinFactory kotlin) { java.make().get().useful(); kotlin.make().get().useful(); } }",
        ),
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
        ("Item.kt", ITEM),
        ("Box.java", JAVA_BOX),
        (
            "Types.kt",
            "package app\nopen class Parent<T> {\n fun get(): Box<T?> = error(\"fixture\")\n}\nclass Concrete : Parent<Item>()\n",
        ),
        (
            "Use.kt",
            "package app\nfun run(value: Concrete) { value.get() }\n",
        ),
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
        ("Item.kt", ITEM),
        ("Other.kt", OTHER),
        ("Box.java", JAVA_BOX),
        (
            "Factory.java",
            "package app; public class Factory { public Box<Item> make() { return null; } }",
        ),
        (
            "Types.kt",
            "package app\nopen class Parent<T> {\n fun get(): T = error(\"fixture\")\n}\nclass Concrete : Parent<Item>()\n",
        ),
        (
            "Use.kt",
            "package app\nfun run(factory: Factory, value: Concrete) { factory.make().get().useful(); value.get().useful() }\n",
        ),
        (
            "Use.java",
            "package app; class Use { void run(Factory factory, Concrete value) { factory.make().get().useful(); value.get().useful(); } }",
        ),
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
        ("Item.kt", ITEM),
        ("Box.java", JAVA_BOX),
        (
            "Cycle.kt",
            "package app\nopen class A : B()\nopen class B : A()\n",
        ),
        (
            "Use.kt",
            "package app\nfun run(box: Box<*>, cycle: A) { box.get().useful(); cycle.get().useful() }\n",
        ),
        (
            "Use.java",
            "package app; class Use { void run(Box box) { box.get().useful(); } }",
        ),
    ]);
    assert!(labels(&mut ws, "Use.kt", "box.get().use").is_empty());
    assert!(labels(&mut ws, "Use.kt", "cycle.get().use").is_empty());
    assert!(labels(&mut ws, "Use.java", "box.get().use").is_empty());
}

#[test]
fn qualified_nested_types_resolve_without_casing_assumptions() {
    // The compiler oracle covers both languages for Outer.Inner, and Java for outer.inner.
    for (outer, inner, kotlin_supported) in [("Outer", "Inner", true), ("outer", "inner", false)] {
        let declaration = format!(
            "package model; public class {outer} {{ public static class {inner} {{ public void useful() {{}} }} }}"
        );
        let unrelated =
            format!("package app; public class {inner} {{ public void usefulWrong() {{}} }}");
        let factory = format!(
            "package app; public class Factory {{ public Box<model.{outer}.{inner}> make() {{ return null; }} }}"
        );
        let mut ws = workspace(&[
            ("Outer.java", &declaration),
            ("WrongInner.java", &unrelated),
            ("Box.java", JAVA_BOX),
            ("Factory.java", &factory),
            (
                "Use.kt",
                "package app\nfun run(factory: Factory) { factory.make().get().useful() }\n",
            ),
            (
                "Use.java",
                "package app; class Use { void run(Factory factory) { factory.make().get().useful(); } }",
            ),
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
        (
            "Outer.java",
            "package model; public class Outer { public static class Inner {} }",
        ),
        ("WrongInner.kt", "package app\nclass Inner\n"),
        (
            "Holder.kt",
            "package app\nopen class Holder<T> {\n fun accept(value: T) {}\n}\nclass Concrete : Holder<model.Outer.Inner>()\n",
        ),
        (
            "Use.kt",
            "package app\nfun run(holder: Concrete, value: model.Outer.Inner) { holder.accept(value) }\n",
        ),
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
        ("Item.kt", ITEM),
        ("Other.kt", OTHER),
        (
            "Base.java",
            "package library; import app.Item; public class Base<T> { public T get() { return null; } }",
        ),
        (
            "WrongBase.java",
            "package app; public class Base<T> { public Other get() { return null; } }",
        ),
        (
            "Concrete.kt",
            "package app\nclass Concrete : library.Base<Item>()\n",
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
fn unknown_projections_preserve_later_type_argument_positions() {
    let mut ws = workspace(&[
        ("Item.kt", ITEM),
        ("Other.kt", OTHER),
        (
            "Pair.java",
            "package app; public class Pair<K, V> { public V get() { return null; } }",
        ),
        (
            "Use.kt",
            "package app\nfun run(pair: Pair<*, Item>, unknown: Pair<Item, *>) { pair.get().useful(); unknown.get().useful() }\n",
        ),
        (
            "Use.java",
            "package app; class Use { void run(Pair<?, Item> pair, Pair<Item, ?> unknown) { pair.get().useful(); unknown.get().useful(); } }",
        ),
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
        ("Item.kt", ITEM),
        ("Other.kt", OTHER),
        (
            "Types.kt",
            "package app\ninterface Root<T> {\n fun get(): T\n}\ninterface Left : Root<Item>\ninterface Right : Root<Other>\nclass Conflict : Left, Right\n",
        ),
        (
            "Use.kt",
            "package app\nfun run(value: Conflict) { value.get().useful() }\n",
        ),
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
        ("Item.kt", ITEM),
        ("Other.kt", OTHER),
        (
            "Types.kt",
            "package app\ninterface Root<T> {\n fun get(): T\n}\ninterface Left : Root<Item>\ninterface Right : Root<Item>\nabstract class Diamond : Left, Right\n",
        ),
        (
            "Use.kt",
            "package app\nfun run(value: Diamond) { value.get().useful() }\n",
        ),
        (
            "Use.java",
            "package app; class Use { void run(Diamond value) { value.get().useful(); } }",
        ),
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
            ("Item.kt", ITEM),
            ("Other.kt", OTHER),
            ("Cycles.java", declarations),
            (
                "Use.kt",
                "package app\nfun run(value: A<Item>) { value.get().useful(); value.missing() }\n",
            ),
            (
                "Use.java",
                "package app; class Use { void run(A<Item> value) { value.get().useful(); value.missing(); } }",
            ),
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
        (
            "Left.kt",
            "package left\nclass Item {\n fun useful() {}\n}\n",
        ),
        (
            "Right.kt",
            "package right\nclass Item {\n fun usefulWrong() {}\n}\n",
        ),
        ("Box.java", JAVA_BOX),
        (
            "Factory.java",
            "package app; import left.*; import right.*; public class Factory { public Box<Item> make() { return null; } }",
        ),
        (
            "Use.kt",
            "package app\nfun run(factory: Factory) { factory.make().get().useful() }\n",
        ),
        (
            "Use.java",
            "package app; class Use { void run(Factory factory) { factory.make().get().useful(); } }",
        ),
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
        ("Item.kt", ITEM),
        ("Other.kt", OTHER),
        (
            "base/Base.kt",
            "package base\nimport app.Item\nopen class Base {\n fun withItem(block: Item.() -> Unit) { Item().block() }\n}\n",
        ),
        (
            "app/Base.kt",
            "package app\nopen class Base {\n fun withItem(block: Other.() -> Unit) {}\n}\n",
        ),
        ("app/Child.java", JAVA_CHILD),
        ("app/KotlinChild.kt", KOTLIN_CHILD),
        (
            "app/Use.kt",
            "package app\nfun run(child: Child, kotlin: KotlinChild) {\n child.withItem { useful() }\n kotlin.withItem { useful() }\n}\n",
        ),
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

#[test]
fn generic_java_class_property_shares_lookup_inference_navigation_and_completion() {
    // Authored declaration from kotlin-stdlib's JvmClassMapping.kt.
    const MAPPING: &str = "package kotlin.jvm\npublic inline val <T : Any> T.javaClass: Class<T>\n get() = (this as java.lang.Object).getClass() as Class<T>\n";
    let source = "package app\nfun run(item: Item) { item.javaClass.cast(item).useful() }\n";
    let mut ws = workspace(&[
        ("Item.kt", ITEM),
        ("Class.java", REFLECTION_CLASS),
        ("Object.java", "package java.lang; public class Object {}"),
        ("Any.kt", "package kotlin\nopen class Any\n"),
        ("JvmClassMapping.kt", MAPPING),
        ("Use.kt", source),
    ]);
    ws.assume_index_complete_for_tests();
    {
        use ktlsp::index::{Entry, InferenceIndex, LookupEntries, ScopedIndex};
        struct PrefixIndex<'a> {
            index: &'a dyn InferenceIndex,
            prefixes: std::cell::RefCell<Vec<String>>,
            generic_prefixes: std::cell::RefCell<Vec<String>>,
            receiver_queries: std::cell::Cell<usize>,
        }
        impl InferenceIndex for PrefixIndex<'_> {
            fn lookup_by_name(&self, name: &str) -> LookupEntries<'_> {
                InferenceIndex::lookup_by_name(self.index, name)
            }
            fn members_of(&self, name: &str) -> LookupEntries<'_> {
                InferenceIndex::members_of(self.index, name)
            }
            fn extensions_for(&self, name: &str) -> LookupEntries<'_> {
                self.receiver_queries.set(self.receiver_queries.get() + 1);
                self.index.extensions_for(name)
            }
            fn generic_receiver_extensions(&self, prefix: &str) -> LookupEntries<'_> {
                self.generic_prefixes.borrow_mut().push(prefix.into());
                self.index.generic_receiver_extensions(prefix)
            }
            fn entries_with_prefix<'a>(
                &'a self,
                prefix: &'a str,
                top_level_only: bool,
            ) -> Box<dyn Iterator<Item = &'a Entry> + 'a> {
                self.prefixes.borrow_mut().push(prefix.into());
                self.index.entries_with_prefix(prefix, top_level_only)
            }
        }
        let snapshot = ws.index.inference_snapshot();
        let scoped = ws.index.scoped_for_file("Use.kt");
        let scoped_snapshot = ScopedIndex::new(&snapshot, "Use.kt");
        let context = FileCtx::new("app".into(), Vec::new());
        let receiver = Type::class("Item", Some("app".into()));
        let alias_source = "package app\nimport kotlin.jvm.javaClass as runtimeClass\n";
        let alias_tree = KotlinParser::new().parse(alias_source);
        let alias_context = FileCtx::from_tree(&alias_tree, alias_source);
        for underlying in [
            &ws.index as &dyn InferenceIndex,
            &snapshot,
            &scoped,
            &scoped_snapshot,
        ] {
            let index = PrefixIndex {
                index: underlying,
                prefixes: Default::default(),
                generic_prefixes: Default::default(),
                receiver_queries: Default::default(),
            };
            for prefix in ["", "javaC"] {
                let java_candidates = ktcore::members::completion_candidates_at(
                    &index, &receiver, prefix, &context, 0, false,
                );
                assert!(java_candidates.extensions.is_empty());
                assert!(index.generic_prefixes.borrow().is_empty());
                assert_eq!(
                    index.receiver_queries.get(),
                    0,
                    "Java skips Kotlin extensions"
                );
            }
            for prefix in ["", "javaC"] {
                let candidates = ktcore::members::completion_candidates_at(
                    &index, &receiver, prefix, &context, 0, true,
                );
                assert!(candidates
                    .extensions
                    .iter()
                    .any(|entry| entry.sym.name == "javaClass"));
            }
            assert_eq!(*index.generic_prefixes.borrow(), ["", "javaC"]);
            for prefix in ["", "runtimeC", "javaC"] {
                let candidates = ktcore::members::completion_candidates_at(
                    &index,
                    &receiver,
                    prefix,
                    &alias_context,
                    0,
                    true,
                );
                assert_eq!(
                    candidates
                        .extensions
                        .iter()
                        .any(|entry| entry.sym.name == "javaClass"),
                    prefix != "javaC",
                    "aliased generic property: {prefix:?}"
                );
            }
            assert!(
                index.prefixes.borrow().is_empty(),
                "generic extension queries do not scan all symbols"
            );
        }
    }
    assert_eq!(
        infer_expression(&ws, "Use.kt", "item.javaClass"),
        Type::Class {
            name: "Class".into(),
            package: Some("java.lang".into()),
            container: None,
            nullable: false,
            args: vec![Type::class("Item", Some("app".into()))],

            owner: None,
        }
    );
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "item.javaClass",
        "JvmClassMapping.kt",
        "javaClass",
    );
    assert!(labels(&mut ws, "Use.kt", "item.javaC").contains("javaClass"));
    assert!(
        ws.diagnostics("Use.kt").is_empty(),
        "{:?}",
        ws.diagnostics("Use.kt")
    );
    ws.change("Use.kt", "package app\nimport kotlin.jvm.javaClass as runtimeClass\nfun run(item: Item) { item.runtimeClass; item.javaClass }\n".into());
    assert!(labels(&mut ws, "Use.kt", "item.runtimeC").contains("runtimeClass"));
    assert!(!labels(&mut ws, "Use.kt", "item.javaC").contains("javaClass"));
    assert_eq!(
        infer_expression(&ws, "Use.kt", "item.runtimeClass").args(),
        &[Type::class("Item", Some("app".into()))]
    );
    ws.change("Use.kt", source.into());
    ws.change(
        "Item.kt",
        "package app\nclass Item { val javaClass: Other = TODO() }\nclass Other\n".into(),
    );
    assert_eq!(
        infer_expression(&ws, "Use.kt", "item.javaClass"),
        Type::class("Other", Some("app".into()))
    );
    assert_exact_definition(&mut ws, "Use.kt", "item.javaClass", "Item.kt", "javaClass");
    ws.change("Item.kt", ITEM.into());
    ws.change(
        "Use.kt",
        "package app\nfun run(item: Item?) { item.javaClass }\n".into(),
    );
    assert_eq!(
        infer_expression(&ws, "Use.kt", "item.javaClass"),
        Type::Unknown
    );
    assert!(!labels(&mut ws, "Use.kt", "item.javaC").contains("javaClass"));
    ws.change("Use.kt", source.into());
    ws.change("JvmClassMapping.kt", "package kotlin.jvm\n".into());
    ws.set_library_index_complete(false);
    assert_eq!(
        infer_expression(&ws, "Use.kt", "item.javaClass"),
        Type::Unknown
    );
    assert!(
        ws.diagnostics("Use.kt").is_empty(),
        "missing stdlib cannot prove absence"
    );
    ws.open(
        "Left.kt",
        MAPPING.replace("package kotlin.jvm", "package left"),
    );
    ws.open(
        "Right.kt",
        MAPPING.replace("package kotlin.jvm", "package right"),
    );
    ws.change("Use.kt", "package app\nimport left.javaClass\nimport right.javaClass\nfun run(item: Item) { item.javaClass }\n".into());
    // Both candidates agree on the type; selection and navigation must retain their ambiguity.
    assert_eq!(
        infer_expression(&ws, "Use.kt", "item.javaClass").args(),
        &[Type::class("Item", Some("app".into()))]
    );
    let text = ws.doc_text("Use.kt").unwrap();
    let tree = KotlinParser::new().parse(&text);
    let context = FileCtx::from_tree_in_file(&tree, &text, "Use.kt");
    let receiver = Type::class("Item", Some("app".into()));
    let candidates = ktcore::members::candidates(&ws.index, &receiver, "javaClass", &context);
    assert_eq!(
        candidates
            .select(
                &ws.index,
                &receiver,
                &ktcore::members::MemberUse::Value,
                &context
            )
            .entries
            .len(),
        2
    );
    let definitions = ws.goto_definition("Use.kt", text.find("item.javaClass").unwrap() + 5);
    assert_ne!(
        definitions.len(),
        1,
        "ambiguous properties cannot navigate to an arbitrary declaration"
    );
    assert!(definitions
        .iter()
        .all(|definition| matches!(definition.file.as_str(), "Left.kt" | "Right.kt")));
    eprintln!(
        "ambiguous same-return property: two selected candidates, {} navigation targets, unanimous Class<Item>; compiler rejects access",
        definitions.len()
    );
    ws.open("Other.kt", "package app\nclass Other\n".into());
    ws.change(
        "Right.kt",
        "package right\nimport app.Other\nval <T : Any> T.javaClass: Other get() = TODO()\n".into(),
    );
    assert_eq!(
        infer_expression(&ws, "Use.kt", "item.javaClass"),
        Type::Unknown,
        "competing return types cannot choose one declaration"
    );
}
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
        ("Item.kt", ITEM),
        ("Other.kt", OTHER),
        ("Types.kt", declarations),
        (
            "Use.kt",
            "package app\nfun run(pair: PairBox<Other, Item>, child: Child, same: PairBox<Item, Item>, conflict: PairBox<Item, Other>) { pair.chosen.useful(); child.chosen.useful(); pair.choose().useful(); same.same.useful(); conflict.same.useful() }\n",
        ),
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
        ("Item.kt", ITEM),
        ("Other.kt", OTHER),
        ("Types.kt", declarations),
        (
            "Use.kt",
            "package app\nfun run(good: Diamond, bad: Conflict) { good.chosen.useful(); bad.chosen.useful() }\n",
        ),
    ]);
    assert_member(&mut ws, "Use.kt", "good.chosen.", "useful", "usefulWrong");
    assert_exact_definition(&mut ws, "Use.kt", "good.chosen.useful", "Item.kt", "useful");
    assert_eq!(infer_expression(&ws, "Use.kt", "bad.chosen"), Type::Unknown);
    assert!(!labels(&mut ws, "Use.kt", "bad.ch").contains("chosen"));
}

#[test]
fn generic_extension_properties_do_not_offer_wrong_concrete_receiver_arguments() {
    use ktlsp::resolve::{self, CompletenessFacts, ResolutionStatus};
    let mut ws = reflection_workspace(
        "package probe\nimport kotlin.reflect.KClass\nfun run(typed: KClass<Invocation>, other: KClass<Other>) { typed.java; other.java }\n",
    );
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
    let mut ws = reflection_workspace(
        "package probe\nfun run(exact: ExactRequest) { val result = exact.tag(Invocation::class.java)!!; result.service() }\n",
    );
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
        let source = format!(
            "package app\nfun run(value: PairBox<{projection}, Item>) {{ value.second().useful(); value.chosen.useful() }}\n"
        );
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
        (
            "Types.kt",
            "package app\nclass Holder<T> { fun body() {} }\nval <T : Item> Holder<T>.bounded: Item get() = error(\"fixture\")\nval <T> Holder<out T>.projected: Item get() = error(\"fixture\")\nval <T> Holder<T>.known: T get() = error(\"fixture\")\n",
        ),
        (
            "Use.kt",
            "package app\nfun run(value: Holder<Item>) { value.known.useful(); value.bounded.useful(); value.projected.useful() }\n",
        ),
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
            (
                "probe/Unit.kt",
                "package probe\nclass Unit { fun unitWrong() {} }\n",
            ),
            (
                "Stdlib.kt",
                "package kotlin\nclass Throwable\nclass Result<T> { fun value(): T = TODO() }\nfun <R> runCatching(block: () -> R): Result<R> = TODO()\nfun <T> Result<T>.onFailure(action: (Throwable) -> Unit): Result<T> = this\n",
            ),
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
        let mut ws = reflection_workspace(
            "package probe\nimport kotlin.reflect.KClass\nfun run(typed: KClass<Invocation>, exact: ExactRequest) { typed.java; exact.tag(Invocation::class.java)!!.service() }\n",
        );
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
            (
                "Holder.kt",
                "package app\nclass Holder<T> { fun body() {} }\nval <T> Holder<T>.chosen: T get() = TODO()\n",
            ),
            (
                "Use.kt",
                "package app\nfun run(holder: Holder<kotlin.String>) { holder.chosen; holder.body() }\n",
            ),
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

const PROJECTED_REQUEST: &str = r#"package probe;
public class Request {
 public @javax.annotation.Nullable <T> T tag(Class<? extends T> type) { return null; }
 public <T> T upper(Class<? super T> type) { return null; }
 public <T> T upperBoth(Class<? super T> left, Class<? super T> right) { return null; }
 public <T> T star(Class<?> type) { return null; }
 public <T> T both(Class<T> exact, Class<? extends T> lower) { return null; }
 public <T> T different(Class<? extends T> left, Class<? extends T> right) { return null; }
 public <T extends Invocation> T bounded(Class<? extends T> type) { return null; }
 public <T> T nested(Class<? extends Class<T>> type) { return null; }
 public <T> T invariantNested(Pair<Class<? extends T>,Class<T>> pair) { return null; }
 public <T> T variant(Variant<? extends T> type) { return null; }
 public <T> T variantIn(Variant<? super T> type) { return null; }
 public <T extends Invocation> T boundedOwner(BoundedOwner<? extends T> type) { return null; }
}
"#;
const PROJECTED_KOTLIN_REQUEST: &str = r#"package probe
class KotlinRequest {
 fun <T> tag(type: Class<out T>): T? = null
 fun <T> upper(type: Class<in T>): T? = null
 fun <T> upperBoth(left: Class<in T>, right: Class<in T>): T? = null
 fun <T> star(type: Class<*>): T? = null
 fun <T> both(exact: Class<T>, lower: Class<out T>): T? = null
 fun <T> bounded(type: Class<out T>): T? where T : Invocation = null
}
"#;

fn projected_workspace(source: &str) -> Workspace {
    let mut ws = reflection_workspace(source);
    ws.open("Request.java", PROJECTED_REQUEST.into());
    ws.open("KotlinRequest.kt", PROJECTED_KOTLIN_REQUEST.into());
    ws.open(
        "Nullable.java",
        "package javax.annotation; public @interface Nullable {}".into(),
    );
    ws.open(
        "Pair.java",
        "package probe; public class Pair<A,B> {}".into(),
    );
    ws.open(
        "Variant.kt",
        "package probe\nclass Variant<out T> { fun body() {} }\n".into(),
    );
    ws.open(
        "BoundedOwner.java",
        "package probe; public class BoundedOwner<T extends Invocation> { public void body() {} }"
            .into(),
    );
    ws
}

#[test]
fn projected_arguments_infer_their_own_target_in_java_and_kotlin() {
    let kotlin = r#"package probe
import kotlin.reflect.KClass
fun use(request: Request, kotlin: KotlinRequest, type: Class<Invocation>, other: Class<Other>, typed: KClass<Invocation>) {
 request.tag(type)!!.service()
 kotlin.tag(type)!!.service()
 request.tag(Invocation::class.java)!!.service()
 request.tag(typed.java)!!.service()
 request.tag(other)!!.body()
 kotlin.tag(other)!!.body()
}
"#;
    let mut ws = projected_workspace(kotlin);
    for expression in [
        "request.tag(type)!!",
        "kotlin.tag(type)!!",
        "request.tag(Invocation::class.java)!!",
        "request.tag(typed.java)!!",
    ] {
        let ty = infer_expression(&ws, "Use.kt", expression);
        assert_eq!(ty.name(), Some("Invocation"), "{expression}: {ty:?}");
        assert_eq!(ty.package(), Some("probe"));
        assert_member(
            &mut ws,
            "Use.kt",
            &format!("{expression}."),
            "service",
            "serviceWrong",
        );
        assert_exact_definition(
            &mut ws,
            "Use.kt",
            &format!("{expression}.service()"),
            "Invocation.java",
            "service",
        );
    }
    for expression in ["request.tag(other)!!", "kotlin.tag(other)!!"] {
        let ty = infer_expression(&ws, "Use.kt", expression);
        assert_eq!(ty.name(), Some("Other"));
        assert_member(
            &mut ws,
            "Use.kt",
            &format!("{expression}."),
            "body",
            "service",
        );
        assert_exact_definition(
            &mut ws,
            "Use.kt",
            &format!("{expression}.body()"),
            "Other.java",
            "body",
        );
    }
    let java = "package probe; class Use { void use(Request request, KotlinRequest kotlin, Class<Invocation> type, Class<Other> other) { request.tag(type).service(); kotlin.tag(type).service(); request.tag(other).body(); kotlin.tag(other).body(); } }";
    ws.open("Use.java", java.into());
    for expression in ["request.tag(type)", "kotlin.tag(type)"] {
        assert_member(
            &mut ws,
            "Use.java",
            &format!("{expression}."),
            "service",
            "serviceWrong",
        );
        assert_exact_definition(
            &mut ws,
            "Use.java",
            &format!("{expression}.service()"),
            "Invocation.java",
            "service",
        );
    }
    for expression in ["request.tag(other)", "kotlin.tag(other)"] {
        assert_member(
            &mut ws,
            "Use.java",
            &format!("{expression}."),
            "body",
            "service",
        );
        assert_exact_definition(
            &mut ws,
            "Use.java",
            &format!("{expression}.body()"),
            "Other.java",
            "body",
        );
    }
}

#[test]
fn projected_constraints_distinguish_equality_lower_and_unsupported_bounds() {
    let source = r#"package probe
fun <T> Pair<T,Other>.from(type: Class<out T>): T = error("fixture")
fun use(request: Request, kotlin: KotlinRequest, type: Class<Invocation>, derived: Class<DerivedInvocation>, other: Class<Other>, star: Class<*>, unknown: Class<Missing>, nested: Class<Class<Invocation>>, pair: Pair<Class<Invocation>,Class<Invocation>>, receiver: Pair<Invocation,Other>, variant: Variant<Invocation>, bounded: BoundedOwner<Invocation>, known: Other) {
 request.both(type,type)!!.service()
 kotlin.both(type,type)!!.service()
 request.nested(nested)!!.service()
 receiver.from(type).service()
 receiver.from(other)
 request.invariantNested(pair)
 request.upper(type)!!.service()
 kotlin.upper(type)!!.service()
 request.upperBoth(type,derived)!!.derivedOnly()
 request.upperBoth(derived,type)!!.derivedOnly()
 kotlin.upperBoth(type,derived)!!.derivedOnly()
 kotlin.upperBoth(derived,type)!!.derivedOnly()
 request.upperBoth(type,other)
 kotlin.upperBoth(type,other)
 request.star(type)
 kotlin.star(type)
 request.tag(star)
 request.tag(unknown)
 request.both(type,other)
 request.different(type,other)
 request.bounded(type)!!.service()
 kotlin.bounded(type)!!.service()
 request.bounded(other)
 kotlin.bounded(other)
 request.variant(variant)!!.service()
 request.variantIn(variant)!!.service()
 request.boundedOwner(bounded)!!.service()
 variant.body()
 bounded.body()
 known.body()
}
"#;
    let mut ws = projected_workspace(source);
    let derived_source = "package probe; public class DerivedInvocation extends Invocation { public void derivedOnly() {} }";
    ws.open("DerivedInvocation.java", derived_source.into());
    for expression in [
        "request.variant(variant)!!",
        "request.upper(type)!!",
        "kotlin.upper(type)!!",
        "request.both(type,type)!!",
        "kotlin.both(type,type)!!",
        "request.nested(nested)!!",
        "receiver.from(type)",
        "request.bounded(type)!!",
        "kotlin.bounded(type)!!",
        "request.boundedOwner(bounded)!!",
    ] {
        assert_member(
            &mut ws,
            "Use.kt",
            &format!("{expression}."),
            "service",
            "serviceWrong",
        );
        assert_exact_definition(
            &mut ws,
            "Use.kt",
            &format!("{expression}.service()"),
            "Invocation.java",
            "service",
        );
    }
    // Unknown producers, incompatible constraints, and unmodeled captures stay conservative.
    for expression in [
        "request.upperBoth(type,other)",
        "kotlin.upperBoth(type,other)",
        "request.star(type)",
        "kotlin.star(type)",
        "request.tag(star)",
        "request.tag(unknown)",
        "request.both(type,other)",
        "request.different(type,other)",
        "request.bounded(other)",
        "kotlin.bounded(other)",
        "request.invariantNested(pair)",
        "receiver.from(other)",
        "request.variantIn(variant)",
    ] {
        assert_eq!(
            infer_expression(&ws, "Use.kt", expression),
            Type::Unknown,
            "{expression}"
        );
    }
    for prefix in ["variant.", "bounded.", "known."] {
        assert_member(&mut ws, "Use.kt", prefix, "body", "service");
    }
    for expression in [
        "request.upperBoth(type,derived)!!",
        "request.upperBoth(derived,type)!!",
        "kotlin.upperBoth(type,derived)!!",
        "kotlin.upperBoth(derived,type)!!",
    ] {
        assert_member(
            &mut ws,
            "Use.kt",
            &format!("{expression}."),
            "derivedOnly",
            "serviceWrong",
        );
        assert_exact_definition(
            &mut ws,
            "Use.kt",
            &format!("{expression}.derivedOnly()"),
            "DerivedInvocation.java",
            "derivedOnly",
        );
    }
    let java_source = "package probe; class UpperUse { void use(Request request, KotlinRequest kotlin, Class<Invocation> type, Class<DerivedInvocation> derived, Variant<Invocation> variant) { request.variant(variant).service(); request.variantIn(variant).service(); request.upper(type).service(); kotlin.upper(type).service(); request.upperBoth(type,derived).derivedOnly(); request.upperBoth(derived,type).derivedOnly(); kotlin.upperBoth(type,derived).derivedOnly(); kotlin.upperBoth(derived,type).derivedOnly(); } }";
    ws.open("UpperUse.java", java_source.into());
    assert_member(
        &mut ws,
        "UpperUse.java",
        "request.variant(variant).",
        "service",
        "serviceWrong",
    );
    assert_exact_definition(
        &mut ws,
        "UpperUse.java",
        "request.variant(variant).service()",
        "Invocation.java",
        "service",
    );
    assert_member(
        &mut ws,
        "UpperUse.java",
        "request.variantIn(variant).",
        "service",
        "serviceWrong",
    );
    assert_exact_definition(
        &mut ws,
        "UpperUse.java",
        "request.variantIn(variant).service()",
        "Invocation.java",
        "service",
    );
    for receiver in ["request", "kotlin"] {
        let call = format!("{receiver}.upper(type)");
        assert_member(
            &mut ws,
            "UpperUse.java",
            &format!("{call}."),
            "service",
            "serviceWrong",
        );
        assert_exact_definition(
            &mut ws,
            "UpperUse.java",
            &format!("{call}.service()"),
            "Invocation.java",
            "service",
        );
        for order in ["type,derived", "derived,type"] {
            let call = format!("{receiver}.upperBoth({order})");
            assert_member(
                &mut ws,
                "UpperUse.java",
                &format!("{call}."),
                "derivedOnly",
                "serviceWrong",
            );
            assert_exact_definition(
                &mut ws,
                "UpperUse.java",
                &format!("{call}.derivedOnly()"),
                "DerivedInvocation.java",
                "derivedOnly",
            );
        }
    }
    // A known matching path cannot hide an unknown sibling hierarchy branch.
    ws.change(
        "DerivedInvocation.java",
        derived_source.replace(
            "extends Invocation",
            "extends Invocation implements Missing",
        ),
    );
    for expression in [
        "request.upperBoth(type,derived)",
        "kotlin.upperBoth(type,derived)",
    ] {
        assert_eq!(infer_expression(&ws, "Use.kt", expression), Type::Unknown);
        assert!(
            !labels(&mut ws, "Use.kt", &format!("{expression}!!.derivedOnly"))
                .contains("derivedOnly")
        );
    }
    ws.change("DerivedInvocation.java", derived_source.into());
    for receiver in ["request", "kotlin"] {
        assert_member(
            &mut ws,
            "Use.kt",
            &format!("{receiver}.upperBoth(type,derived)!!."),
            "derivedOnly",
            "serviceWrong",
        );
    }

    let variant_source = ws.doc_text("Variant.kt").unwrap().to_string();
    // Java uses the JVM signature; Kotlin additionally rejects opposing declaration/use directions.
    ws.change("Variant.kt", variant_source.replace("out T>", "in T>"));
    assert_eq!(
        infer_expression(&ws, "Use.kt", "request.variant(variant)"),
        Type::Unknown
    );
    assert!(!labels(&mut ws, "Use.kt", "request.variant(variant)!!.service").contains("service"));
    assert_member(
        &mut ws,
        "Use.kt",
        "request.variantIn(variant)!!.",
        "service",
        "serviceWrong",
    );
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "request.variantIn(variant)!!.service()",
        "Invocation.java",
        "service",
    );
    for call in ["request.variant(variant)", "request.variantIn(variant)"] {
        assert_member(
            &mut ws,
            "UpperUse.java",
            &format!("{call}."),
            "service",
            "serviceWrong",
        );
        assert_exact_definition(
            &mut ws,
            "UpperUse.java",
            &format!("{call}.service()"),
            "Invocation.java",
            "service",
        );
    }
    ws.change(
        "Variant.kt",
        variant_source.replace("out T>", "out T: Missing>"),
    );
    assert_eq!(
        infer_expression(&ws, "Use.kt", "request.variant(variant)"),
        Type::Unknown
    );
    assert!(!labels(&mut ws, "Use.kt", "request.variant(variant)!!.service").contains("service"));
    ws.change("Variant.kt", variant_source);
    assert_member(
        &mut ws,
        "Use.kt",
        "request.variant(variant)!!.",
        "service",
        "serviceWrong",
    );
}

#[test]
fn projected_overloads_keep_unknown_candidates_and_reject_exact_invariant_decoys() {
    let source = r#"package probe
fun use(request: Request, type: Class<Invocation>, marker: Invocation, known: Other) {
 request.pick(type,marker).service()
 request.independent(type,marker).service()
 request.uncertain(type,marker)
 known.body()
}
"#;
    let mut ws = projected_workspace(source);
    ws.open(
        "other/Class.java",
        "package other; public class Class<T> {}".into(),
    );
    ws.open("Request.java",r#"package probe; public class Request {
 public <T> T pick(Class<? extends T> type, Invocation marker) { return null; }
 public Other pick(Class<Other> type, Other marker) { return null; }
 public <T> T uncertain(Class<? extends T> type, Invocation marker) { return null; }
 public <T extends Invocation> Other uncertain(other.Class<? extends T> type, Invocation marker) { return null; }
 public <T> T independent(Class<? extends T> type, Invocation marker) { return null; }
 public <T extends Invocation> Other independent(other.Class<? extends T> type, Other marker) { return null; }
}"#.into());
    assert_member(
        &mut ws,
        "Use.kt",
        "request.pick(type,marker).",
        "service",
        "serviceWrong",
    );
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "request.pick(type,marker).service()",
        "Invocation.java",
        "service",
    );
    // Javac 21 and K2 both independently select Invocation here. A closed mismatch in
    // marker rejects the bounded competitor regardless of its unsupported generic head.
    assert_member(&mut ws, "Use.kt", "request.independent(type,marker).", "service", "serviceWrong");
    assert_exact_definition(&mut ws, "Use.kt", "request.independent(type,marker).service()", "Invocation.java", "service");
    // A mismatching nominal head without a complete absence proof remains a possible overload.
    assert_eq!(
        infer_expression(&ws, "Use.kt", "request.uncertain(type,marker)"),
        Type::Unknown
    );
    assert_member(&mut ws, "Use.kt", "known.", "body", "service");
    // The bounded competitor stays possible rather than disappearing as an unknown argument.
    let owner = Type::class("Request", Some("probe".into()));
    let argument = Type::Class {
        name: "Class".into(),
        package: Some("java.lang".into()),
        container: None,
        nullable: false,
        args: vec![Type::class("Invocation", Some("probe".into()))],

        owner: None,
    };
    let ctx = FileCtx::new("probe".into(), Vec::new());
    let candidates = ktcore::members::candidates(&ws.index, &owner, "uncertain", &ctx);
    let selection = candidates.select(
        &ws.index,
        &owner,
        &ktcore::members::MemberUse::Call(ktcore::members::CallArguments {
            count: Some(2),
            types: vec![argument, Type::class("Invocation", Some("probe".into()))],
            names: None,
            has_spread: false,
            trailing_lambda: false,
            trailing_lambda_arguments: None,
        }),
        &ctx,
    );
    assert_eq!(
        selection.applicability,
        ktcore::members::Applicability::Unknown
    );
    assert_eq!(
        selection.entries.len(),
        2,
        "unsupported overload must remain possible"
    );
}

#[test]
fn projected_source_facts_survive_cache_and_unsaved_variance_changes() {
    use ktlsp::types::Projection;
    let source = "package probe\nfun use(request: Request,type:Class<Invocation>,known:Other) { request.tag(type)!!.service(); known.body() }\n";
    let mut ws = projected_workspace(source);
    let class = ws.index.lookup_type("BoundedOwner");
    assert!(class[0].sym.type_parameter_facts.known_invariant());
    assert!(!class[0].sym.type_parameter_facts.known_unbounded());
    for file in ["Request.java", "KotlinRequest.kt"] {
        let entries = ws.index.entries_for_file(file);
        let symbols: Vec<_> = entries.iter().map(|entry| (*entry.sym).clone()).collect();
        let bytes = bincode::serialize(&symbols).unwrap();
        let decoded: Vec<ktlsp::symbol::IndexedSymbol> = bincode::deserialize(&bytes).unwrap();
        assert_eq!(decoded, symbols);
        for (name, projection) in [
            ("tag", Projection::Out),
            ("upper", Projection::In),
            ("star", Projection::Star),
        ] {
            let symbol = decoded.iter().find(|symbol| symbol.name == name).unwrap();
            assert_eq!(symbol.params[0].args[0].projection, projection);
        }
    }
    let entry = ws
        .index
        .lookup_by_name("tag")
        .iter()
        .find(|entry| entry.path.as_ref() == "Request.java")
        .unwrap();
    let projected = infer::declared_member_type(
        &ws.index,
        &Type::class("Request", Some("probe".into())),
        entry,
        &entry.sym.params[0],
        ktcore::members::CallMode::Kotlin,
        &FileCtx::new("probe".into(), Vec::new()),
    );
    assert!(
        matches!(&projected.args()[0],Type::Projected {projection:Projection::Out,bound} if **bound == Type::Unknown)
    );
    assert_member(
        &mut ws,
        "Use.kt",
        "request.tag(type)!!.",
        "service",
        "serviceWrong",
    );
    ws.change(
        "Request.java",
        PROJECTED_REQUEST.replace("Class<? extends T> type", "Class<? super T> type"),
    );
    assert_eq!(
        infer_expression(&ws, "Use.kt", "request.tag(type)!!"),
        Type::class("Invocation", Some("probe".into()))
    );
    ws.change(
        "Request.java",
        PROJECTED_REQUEST.replace("Class<? extends T> type", "Class<?> type"),
    );
    assert_eq!(
        infer_expression(&ws, "Use.kt", "request.tag(type)!!"),
        Type::Unknown
    );
    assert_member(&mut ws, "Use.kt", "known.", "body", "service");
    ws.change("Request.java", PROJECTED_REQUEST.into());
    assert_member(
        &mut ws,
        "Use.kt",
        "request.tag(type)!!.",
        "service",
        "serviceWrong",
    );
}

#[test]
fn projected_named_arguments_bind_to_their_declared_positions() {
    let source = r#"package probe
fun <A,B> pick(first:Class<out A>,second:Class<out B>):A=error("fixture")
fun use(left:Class<Invocation>,right:Class<Other>,known:Other) {
 pick(left,right).service()
 pick(second=right,first=left).service()
 pick(left,second=right).service()
 pick(`second`=right,`first`=left).service()
 known.body()
}
"#;
    let mut ws = projected_workspace(source);
    for expression in [
        "pick(left,right)",
        "pick(second=right,first=left)",
        "pick(left,second=right)",
        "pick(`second`=right,`first`=left)",
    ] {
        assert_eq!(
            infer_expression(&ws, "Use.kt", expression),
            Type::class("Invocation", Some("probe".into()))
        );
        assert_member(
            &mut ws,
            "Use.kt",
            &format!("{expression}."),
            "service",
            "body",
        );
        assert_exact_definition(
            &mut ws,
            "Use.kt",
            &format!("{expression}.service()"),
            "Invocation.java",
            "service",
        );
    }
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "pick(second=right,first=left)",
        "Use.kt",
        "pick",
    );
    assert_member(&mut ws, "Use.kt", "known.", "body", "service");
}

#[test]
fn projected_fixed_unknown_parameters_do_not_certify_calls() {
    let source = r#"package probe
fun use(request:MissingRequest,valid:Request,type:Class<Invocation>,missing:Missing,known:Other) {
 request.tag(type,missing)
 valid.tag(type)!!.service()
 known.body()
}
"#;
    let mut ws = projected_workspace(source);
    ws.open("MissingRequest.java","package probe; public class MissingRequest { public <T> T tag(Class<? extends T> type,Missing marker) { return null; } }".into());
    // An unresolved fixed parameter cannot certify a call merely because both symbolic heads agree.
    assert_eq!(
        infer_expression(&ws, "Use.kt", "request.tag(type,missing)"),
        Type::Unknown
    );
    assert_member(
        &mut ws,
        "Use.kt",
        "valid.tag(type)!!.",
        "service",
        "serviceWrong",
    );
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "valid.tag(type)!!.service()",
        "Invocation.java",
        "service",
    );
    assert_member(&mut ws, "Use.kt", "known.", "body", "service");
    let owner = Type::class("MissingRequest", Some("probe".into()));
    let argument = Type::Class {
        name: "Class".into(),
        package: Some("java.lang".into()),
        container: None,
        nullable: false,
        args: vec![Type::class("Invocation", Some("probe".into()))],

        owner: None,
    };
    let arguments = ktcore::members::CallArguments {
        count: Some(2),
        types: vec![argument, Type::class("Missing", None)],
        names: None,
        has_spread: false,
        trailing_lambda: false,
        trailing_lambda_arguments: None,
    };
    let ctx = FileCtx::new("probe".into(), Vec::new());
    let candidates = ktcore::members::candidates(&ws.index, &owner, "tag", &ctx);
    let selection = candidates.select(
        &ws.index,
        &owner,
        &ktcore::members::MemberUse::Call(arguments.clone()),
        &ctx,
    );
    assert_eq!(
        selection.applicability,
        ktcore::members::Applicability::Unknown
    );
    assert_eq!(
        selection.entries.len(),
        1,
        "unknown fixed parameter must retain its callable"
    );
    ws.change("MissingRequest.java","package probe; public class MissingRequest { public <T> T tag(Class<? extends T> type,Missing marker) { return null; } public <T> T tag(Class<? extends T> type,Other marker) { return null; } }".into());
    let candidates = ktcore::members::candidates(&ws.index, &owner, "tag", &ctx);
    let selection = candidates.select(
        &ws.index,
        &owner,
        &ktcore::members::MemberUse::Call(arguments),
        &ctx,
    );
    assert_eq!(
        selection.applicability,
        ktcore::members::Applicability::Unknown
    );
    assert_eq!(
        selection.entries.len(),
        2,
        "unresolved arguments must not remove possible overloads"
    );
}

#[test]
fn named_projected_calls_share_member_extension_and_receiver_constraints() {
    let source = r#"package probe
fun <T> Class<T>.choose(other:Class<out T>,marker:Invocation):T=error("fixture")
fun use(request:KotlinRequest,left:Class<Invocation>,right:Class<Other>,marker:Invocation,known:Other) {
 request.both(lower=left,exact=left)!!.service()
 left.choose(marker=marker,other=left).service()
 request.both(lower=right,exact=left)
 left.choose(marker=marker,other=right)
 known.body()
}
"#;
    let mut ws = projected_workspace(source);
    for expression in [
        "request.both(lower=left,exact=left)!!",
        "left.choose(marker=marker,other=left)",
    ] {
        assert_eq!(
            infer_expression(&ws, "Use.kt", expression),
            Type::class("Invocation", Some("probe".into()))
        );
        assert_member(
            &mut ws,
            "Use.kt",
            &format!("{expression}."),
            "service",
            "body",
        );
        assert_exact_definition(
            &mut ws,
            "Use.kt",
            &format!("{expression}.service()"),
            "Invocation.java",
            "service",
        );
    }
    for expression in [
        "request.both(lower=right,exact=left)",
        "left.choose(marker=marker,other=right)",
    ] {
        assert_eq!(
            infer_expression(&ws, "Use.kt", expression),
            Type::Unknown,
            "{expression}"
        );
    }
    assert_member(&mut ws, "Use.kt", "known.", "body", "service");
}

#[test]
fn named_projected_overloads_use_each_candidates_labels_order_and_types() {
    let declarations = r#"package probe
class NamedRouter {
 fun <T> pick(type:Class<out T>,marker:Class<Invocation>):T=error("fixture")
 @kotlin.jvm.JvmName("pickOther")
fun <T> pick(marker:Class<Other>,type:Class<out T>):T=error("fixture")
 fun <T> uncertain(type:Class<out T>,marker:Class<Invocation>):T=error("fixture")
 fun <T> uncertain(marker:Missing,type:Class<out T>):Other=error("fixture")
}
fun <T> freePick(type:Class<out T>,marker:Class<Invocation>):T=error("fixture")
@kotlin.jvm.JvmName("freePickOther")
fun freePick(marker:Class<Other>,value:Class<Other>):Other=error("fixture")
fun <T> ambiguous(type:Class<out T>,marker:Class<Invocation>):T=error("fixture")
fun <T> ambiguous(marker:Missing,type:Class<out T>):Other=error("fixture")
"#;
    let source = r#"package probe
fun <T> localPick(first:Class<out T>,second:Class<Invocation>):T=error("fixture")
@kotlin.jvm.JvmName("localPickOther")
fun localPick(second:Class<Other>,first:Class<Other>):Other=error("fixture")
fun use(router:NamedRouter,left:Class<Invocation>,right:Class<Other>,marker:Invocation,known:Other) {
 router.pick(marker=left,type=left).service()
 router.pick(type=right,marker=right).body()
 freePick(marker=left,type=left).service()
 freePick(marker=left,type=right).body()
 freePick(marker=right,value=right).body()
 localPick(second=left,first=left).service()
 localPick(second=left,first=right).body()
 localPick(first=right,second=right).body()
 probe.freePick(marker=left,type=left).service()
 probe.freePick(marker=left,type=right).body()
 probe.freePick(marker=right,value=right).body()
 ambiguous(marker=left,type=left)
 probe.ambiguous(marker=left,type=left)
 router.uncertain(marker=left,type=left)
 known.body()
}
"#;
    let mut ws = projected_workspace(source);
    ws.open("NamedRouter.kt", declarations.into());
    for (expression, target, name, absent) in [
        (
            "router.pick(marker=left,type=left)",
            "Invocation.java",
            "service",
            "body",
        ),
        (
            "freePick(marker=left,type=left)",
            "Invocation.java",
            "service",
            "body",
        ),
        (
            "freePick(marker=left,type=right)",
            "Other.java",
            "body",
            "service",
        ),
        (
            "router.pick(type=right,marker=right)",
            "Other.java",
            "body",
            "service",
        ),
        (
            "freePick(marker=right,value=right)",
            "Other.java",
            "body",
            "service",
        ),
        (
            "probe.freePick(marker=left,type=left)",
            "Invocation.java",
            "service",
            "body",
        ),
        (
            "probe.freePick(marker=left,type=right)",
            "Other.java",
            "body",
            "service",
        ),
        (
            "probe.freePick(marker=right,value=right)",
            "Other.java",
            "body",
            "service",
        ),
        (
            "localPick(second=left,first=left)",
            "Invocation.java",
            "service",
            "body",
        ),
        (
            "localPick(second=left,first=right)",
            "Other.java",
            "body",
            "service",
        ),
        (
            "localPick(first=right,second=right)",
            "Other.java",
            "body",
            "service",
        ),
    ] {
        assert_member(&mut ws, "Use.kt", &format!("{expression}."), name, absent);
        assert_exact_definition(
            &mut ws,
            "Use.kt",
            &format!("{expression}.{name}()"),
            target,
            name,
        );
    }
    for (expression, target, declaration) in [
        (
            "freePick(marker=left,type=left)",
            "NamedRouter.kt",
            "fun <T> freePick(",
        ),
        (
            "freePick(marker=left,type=right)",
            "NamedRouter.kt",
            "fun <T> freePick(",
        ),
        (
            "freePick(marker=right,value=right)",
            "NamedRouter.kt",
            "fun freePick(",
        ),
        (
            "probe.freePick(marker=left,type=left)",
            "NamedRouter.kt",
            "fun <T> freePick(",
        ),
        (
            "probe.freePick(marker=left,type=right)",
            "NamedRouter.kt",
            "fun <T> freePick(",
        ),
        (
            "probe.freePick(marker=right,value=right)",
            "NamedRouter.kt",
            "fun freePick(",
        ),
        (
            "localPick(second=left,first=left)",
            "Use.kt",
            "fun <T> localPick(",
        ),
        (
            "localPick(second=left,first=right)",
            "Use.kt",
            "fun <T> localPick(",
        ),
        (
            "localPick(first=right,second=right)",
            "Use.kt",
            "fun localPick(",
        ),
    ] {
        let callee = expression.split('(').next().unwrap();
        let name = callee.rsplit('.').next().unwrap();
        let offset = source.find(expression).unwrap() + callee.rfind(name).unwrap();
        let target_source = ws.doc_text(target).unwrap();
        let expected = target_source.find(declaration).unwrap() + declaration.find(name).unwrap();
        assert_eq!(
            ws.goto_definition("Use.kt", offset),
            vec![ktlsp::symbol::Def {
                file: target.into(),
                start_byte: expected,
                end_byte: expected + name.len(),
            }],
            "named callee must select the exact overload: {expression}"
        );
    }
    for expression in [
        "ambiguous(marker=left,type=left)",
        "probe.ambiguous(marker=left,type=left)",
    ] {
        let callee = expression.split('(').next().unwrap();
        let offset = source.find(expression).unwrap() + callee.rfind("ambiguous").unwrap();
        assert!(
            ws.goto_definition("Use.kt", offset).is_empty(),
            "unknown competitor must not select a callee: {expression}"
        );
    }
    assert_eq!(
        infer_expression(&ws, "Use.kt", "ambiguous(marker=left,type=left)"),
        Type::Unknown
    );
    assert_eq!(
        infer_expression(&ws, "Use.kt", "probe.ambiguous(marker=left,type=left)"),
        Type::Unknown
    );
    assert_eq!(
        infer_expression(&ws, "Use.kt", "router.uncertain(marker=left,type=left)"),
        Type::Unknown
    );
    let tree = KotlinParser::new().parse(source);
    let start = source
        .find("router.uncertain(marker=left,type=left)")
        .unwrap();
    let call = tree
        .root_node()
        .descendant_for_byte_range(
            start,
            start + "router.uncertain(marker=left,type=left)".len(),
        )
        .unwrap();
    let ctx = FileCtx::from_tree(&tree, source);
    let arguments = infer::call_arguments(&ws.index, call, source, &ctx);
    let owner = Type::class("NamedRouter", Some("probe".into()));
    let candidates = ktcore::members::candidates(&ws.index, &owner, "uncertain", &ctx);
    let selection = candidates.select(
        &ws.index,
        &owner,
        &ktcore::members::MemberUse::Call(arguments.clone()),
        &ctx,
    );
    assert_eq!(
        selection.applicability,
        ktcore::members::Applicability::Unknown
    );
    assert_eq!(
        selection.entries.len(),
        2,
        "unknown fixed formal must keep the competing overload"
    );
    let mut symbols: Vec<_> = ws
        .index
        .entries_for_file("NamedRouter.kt")
        .iter()
        .map(|entry| (*entry.sym).clone())
        .collect();
    for name in ["uncertain", "ambiguous"] {
        let unknown = symbols
            .iter_mut()
            .find(|symbol| symbol.name == name && symbol.params[0].name == "Missing")
            .unwrap();
        unknown.kotlin_parameters = None;
    }
    ws.index
        .replace_file("NamedRouter.kt", symbols, ktlsp::index::Tier::Volatile);
    let candidates = ktcore::members::candidates(&ws.index, &owner, "uncertain", &ctx);
    let selection = candidates.select(
        &ws.index,
        &owner,
        &ktcore::members::MemberUse::Call(arguments),
        &ctx,
    );
    assert_eq!(
        selection.applicability,
        ktcore::members::Applicability::Unknown
    );
    assert_eq!(
        selection.entries.len(),
        2,
        "absent source names cannot eliminate an overload"
    );
    for expression in [
        "ambiguous(marker=left,type=left)",
        "probe.ambiguous(marker=left,type=left)",
    ] {
        let callee = expression.split('(').next().unwrap();
        let offset = source.find(expression).unwrap() + callee.rfind("ambiguous").unwrap();
        assert!(
            ws.goto_definition("Use.kt", offset).is_empty(),
            "missing metadata must retain callee uncertainty: {expression}"
        );
    }
    assert_member(&mut ws, "Use.kt", "known.", "body", "service");
}

#[test]
fn named_generic_callbacks_bind_context_without_reentering_the_body() {
    let source = r#"package probe
class Candidate { fun useful():String = "ok" }
class Other { fun usefulWrong() {} }
fun <T> accept(value:T, block:(T)->String):String = block(value)
fun <T> select(value:T, block:(T)->Unit):T = value
fun <T> checked(value:T, block:(T)->String):T = value
fun use() {
 accept(value=Candidate()) { it.useful() }
 select(value=Candidate()) {}.useful()
}
"#;
    let mut ws = workspace(&[("Use.kt", source)]);
    // Diagnostics visit implicit receivers before the first editor completion request.
    let _ = ws.diagnostics("Use.kt");
    for prefix in ["it.", "select(value=Candidate()) {}."] {
        assert_member(&mut ws, "Use.kt", prefix, "useful", "usefulWrong");
        assert_exact_definition(
            &mut ws,
            "Use.kt",
            &format!("{prefix}useful()"),
            "Use.kt",
            "useful",
        );
    }
    assert_eq!(
        infer_expression(&ws, "Use.kt", "select(value=Candidate()) {}"),
        Type::class("Candidate", Some("probe".into()))
    );
    for expression in [
        "select(value=Candidate()) { item:Other -> }",
        "checked(value=Candidate()) { 42 }",
        "select(missing=Candidate()) {}",
    ] {
        ws.change("Use.kt", format!("{source}\nfun invalid() {{ {expression}.useful() }}"));
        assert_eq!(infer_expression(&ws, "Use.kt", expression), Type::Unknown, "{expression}");
        assert!(!labels(&mut ws, "Use.kt", &format!("{expression}.")).contains("useful"));
    }
}

#[test]
fn named_projected_calls_preserve_unknown_for_unsupported_argument_shapes() {
    let declarations = r#"package probe
class NamedShapes {
 fun <A,B> pick(first:Class<out A>,second:Class<out B>):A=error("fixture")
 fun <T> optional(ignored:Other=Other(),required:Class<out T>):T=error("fixture")
 fun <T> variable(vararg types:Class<out T>):T=error("fixture")
 fun <A,B> tail(first:Class<out A>,second:Class<out B>,block:()->Unit):A=error("fixture")
 fun <A,B> tailOne(first:Class<out A>,second:Class<out B>,block:(Invocation)->Unit):A=error("fixture")
 fun <A,B> tailItem(first:Class<out A>,second:Class<out B>,block:(Invocation)->Invocation):A=error("fixture")
 fun <A,B> tailAny(first:Class<out A>,second:Class<out B>,block:(Any)->Unit):A=error("fixture")
 fun <A,B> tailReceiver(first:Class<out A>,second:Class<out B>,block:Invocation.()->Unit):A=error("fixture")
 fun <A,B> tailUnknown(first:Class<out A>,second:Class<out B>,block:Missing):A=error("fixture")
 fun <A,B> tailOther(first:Class<out A>,second:Class<out B>,block:()->Other):A=error("fixture")
}
fun <A,B> freeTail(first:Class<out A>,second:Class<out B>,block:(Invocation)->Invocation):A=error("fixture")
"#;
    let source = r#"package probe
fun use(shapes:NamedShapes,left:Class<Invocation>,right:Class<Other>,spread:Array<Class<Invocation>>,known:Other,nullable:Invocation?,direct:Invocation) {
 shapes.optional(required=left,ignored=known).service()
 shapes.optional(required=left).service()
 shapes.pick(second=right,left)
 shapes.pick(first=left,first=right)
 shapes.pick(first=left,missing=right)
 shapes.pick(first=*left,second=right)
 shapes.variable(types=*spread)
 shapes.tail(second=right,first=left) {}.service()
 shapes.tail(second=right,first=left) { known.body() }.service()
 shapes.tail(second=right,first=left) { -> known.body() }.service()
 shapes.tail(second=right,first=left) { unfinishedBody }.service()
 shapes.tail(second=right,first=left) { value -> }
 shapes.tailOne(second=right,first=left) {}.service()
 shapes.tailOne(second=right,first=left) { item -> item }.service()
 shapes.tailOne(second=right,first=left) { it }.service()
 shapes.tailItem(second=right,first=left) { item -> item }.service()
 shapes.tailItem(second=right,first=left) { it }.service()
 shapes.tailItem(second=right,first=left) { item:Invocation -> item }.service()
 shapes.tailOne(second=right,first=left) { item:Any -> item }.service()
 shapes.tailItem(second=right,first=left) { item:Any -> direct }.service()
 shapes.tailOne(second=right,first=left) { item -> item.service() }.service()
 shapes.tailOne(second=right,first=left) { it.service() }.service()
 freeTail(second=right,first=left) { item -> item }.service()
 freeTail(second=right,first=left) { it }.service()
 shapes.tailAny(second=right,first=left) { if(it is Invocation) it.service() }.service()
 shapes.tailOne(second=right,first=left) { item:Any -> item.service() }
 shapes.tailOne(second=right,first=left) { -> }
 shapes.tailOne(second=right,first=left) { item:Other -> }
 shapes.tailItem(second=right,first=left) {}
 shapes.tailItem(second=right,first=left) { known }
 shapes.tailItem(second=right,first=left) { nullable }
 shapes.tailItem(second=right,first=left) { first,second -> first }
 shapes.tailReceiver(second=right,first=left) {}
 shapes.tailUnknown(second=right,first=left) {}
 shapes.tailOther(second=right,first=left) {}
 shapes.tail(first=left) {}
 shapes.tail(first=left,first=right) {}
 shapes.tail(first=known,second=right) {}
 shapes.tail(first=left,second=right,block={}) {}
 known.body()
}
fun shadowed(left:Class<Invocation>,right:Class<Other>,known:Other) {
 val tail:(Class<Invocation>,Class<Other>,()->Unit)->Other = { _,_,_ -> known }
 tail(first=left,second=right) {}
}
fun outerGuard(shapes:NamedShapes,left:Class<Invocation>,right:Class<Other>,it:Other?) {
 if(it!=null) shapes.tailItem(first=left,second=right) { it }.service()
 shapes.tailOne(first=left,second=right) { val it=Other();it.body() }.service()
}
"#;
    let mut ws = projected_workspace(source);
    ws.open("NamedShapes.kt", declarations.into());
    assert_member(
        &mut ws,
        "Use.kt",
        "shapes.optional(required=left,ignored=known).",
        "service",
        "body",
    );
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "shapes.optional(required=left,ignored=known).service()",
        "Invocation.java",
        "service",
    );
    assert_eq!(
        infer_expression(&ws, "Use.kt", "shapes.optional(required=left)"),
        Type::class("Invocation", Some("probe".into()))
    );
    // Default omission is valid Kotlin and must produce useful completion and exact navigation.
    assert_member(
        &mut ws,
        "Use.kt",
        "shapes.optional(required=left).",
        "service",
        "body",
    );
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "shapes.optional(required=left).service()",
        "Invocation.java",
        "service",
    );
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "shapes.optional(required=left)",
        "NamedShapes.kt",
        "optional",
    );
    for expression in [
        "shapes.tail(second=right,first=left) {}",
        "shapes.tail(second=right,first=left) { known.body() }",
        "shapes.tail(second=right,first=left) { -> known.body() }",
        "shapes.tail(second=right,first=left) { unfinishedBody }",
        "shapes.tailOne(second=right,first=left) {}",
        "shapes.tailOne(second=right,first=left) { item -> item }",
        "shapes.tailOne(second=right,first=left) { it }",
        "shapes.tailItem(second=right,first=left) { item -> item }",
        "shapes.tailItem(second=right,first=left) { it }",
        "shapes.tailItem(second=right,first=left) { item:Invocation -> item }",
        "shapes.tailOne(second=right,first=left) { item:Any -> item }",
        "shapes.tailItem(second=right,first=left) { item:Any -> direct }",
        "shapes.tailOne(second=right,first=left) { item -> item.service() }",
        "shapes.tailOne(second=right,first=left) { it.service() }",
        "freeTail(second=right,first=left) { item -> item }",
        "freeTail(second=right,first=left) { it }",
        "shapes.tailAny(second=right,first=left) { if(it is Invocation) it.service() }",
        "shapes.tailItem(first=left,second=right) { it }",
        "shapes.tailOne(first=left,second=right) { val it=Other();it.body() }",
    ] {
        assert_eq!(
            infer_expression(&ws, "Use.kt", expression),
            Type::class("Invocation", Some("probe".into())),
            "{expression}"
        );
        assert_member(&mut ws, "Use.kt", &format!("{expression}."), "service", "body");
        assert_exact_definition(&mut ws, "Use.kt", &format!("{expression}.service()"), "Invocation.java", "service");
    }
    for expression in [
        "shapes.pick(second=right,left)",
        "shapes.pick(first=left,first=right)",
        "shapes.pick(first=left,missing=right)",
        "shapes.pick(first=*left,second=right)",
        "shapes.variable(types=*spread)",
        "shapes.tail(second=right,first=left) { value -> }",
        "shapes.tailOne(second=right,first=left) { -> }",
        "shapes.tailOne(second=right,first=left) { item:Other -> }",
        "shapes.tailItem(second=right,first=left) {}",
        "shapes.tailItem(second=right,first=left) { known }",
        "shapes.tailItem(second=right,first=left) { nullable }",
        "shapes.tailItem(second=right,first=left) { first,second -> first }",
        "shapes.tailReceiver(second=right,first=left) {}",
        "shapes.tailUnknown(second=right,first=left) {}",
        "shapes.tailOther(second=right,first=left) {}",
        "shapes.tail(first=left) {}",
        "shapes.tail(first=left,first=right) {}",
        "shapes.tail(first=known,second=right) {}",
        "shapes.tail(first=left,second=right,block={}) {}",
        "tail(first=left,second=right) {}",
    ] {
        assert_eq!(
            infer_expression(&ws, "Use.kt", expression),
            Type::Unknown,
            "{expression}"
        );
    }
    let tree = KotlinParser::new().parse(source);
    for expression in [
        "shapes.pick(second=right,left)",
        "shapes.variable(types=*spread)",
        "shapes.tailOne(second=right,first=left) { -> }",
    ] {
        let start = source.find(expression).unwrap();
        let call = tree
            .root_node()
            .descendant_for_byte_range(start, start + expression.len())
            .unwrap();
        let ctx = FileCtx::from_tree(&tree, source);
        let arguments = infer::call_arguments(&ws.index, call, source, &ctx);
        let name = expression
            .split('.')
            .nth(1)
            .unwrap()
            .split('(')
            .next()
            .unwrap();
        let owner = Type::class("NamedShapes", Some("probe".into()));
        let candidates = ktcore::members::candidates(&ws.index, &owner, name, &ctx);
        let selection = candidates.select(
            &ws.index,
            &owner,
            &ktcore::members::MemberUse::Call(arguments),
            &ctx,
        );
        assert_eq!(
            selection.applicability,
            ktcore::members::Applicability::Unknown,
            "{expression}"
        );
        assert_eq!(
            selection.entries.len(),
            1,
            "unsupported call must retain its candidate: {expression}"
        );
    }
    assert_member(&mut ws, "Use.kt", "{ item -> item.", "service", "body");
    assert_member(&mut ws, "Use.kt", "{ it.", "service", "body");
    assert_member(&mut ws, "Use.kt", "if(it is Invocation) it.", "service", "body");
    assert_member(&mut ws, "Use.kt", "val it=Other();it.", "body", "service");
    assert!(!labels(&mut ws, "Use.kt", "{ item:Any -> item.").contains("service"));
    assert_member(&mut ws, "Use.kt", "known.", "body", "service");
}

#[test]
fn named_projected_formal_name_edits_invalidate_then_restore_cross_file_inference() {
    let declaration = r#"package probe
fun <A,B> pick(first:Class<out A>,second:Class<out B>):A=error("fixture")
"#;
    let source = "package probe
fun use(left:Class<Invocation>,right:Class<Other>,known:Other) { pick(second=right,first=left).service();known.body() }
";
    let renamed = declaration.replace("first:", "chosen:");
    let updated = source.replace("first=left", "chosen=left");
    let mut ws = projected_workspace(source);
    ws.open("Names.kt", declaration.into());
    // Warm the caller before changing either file, then query after each one-file edit.
    for (edit, expression, useful) in [
        (None, "pick(second=right,first=left)", true),
        (
            Some(("Names.kt", renamed.as_str())),
            "pick(second=right,first=left)",
            false,
        ),
        (
            Some(("Use.kt", updated.as_str())),
            "pick(second=right,chosen=left)",
            true,
        ),
        (
            Some(("Use.kt", source)),
            "pick(second=right,first=left)",
            false,
        ),
        (
            Some(("Names.kt", declaration)),
            "pick(second=right,first=left)",
            true,
        ),
    ] {
        if let Some((file, text)) = edit {
            assert_ne!(
                ws.doc_text(file).as_deref(),
                Some(text),
                "each invalidation step must change exactly one file"
            );
            ws.change(file, text.into());
        }
        let inferred = infer_expression(&ws, "Use.kt", expression);
        if useful {
            assert_eq!(inferred, Type::class("Invocation", Some("probe".into())));
            assert_member(
                &mut ws,
                "Use.kt",
                &format!("{expression}."),
                "service",
                "body",
            );
            assert_exact_definition(
                &mut ws,
                "Use.kt",
                &format!("{expression}.service()"),
                "Invocation.java",
                "service",
            );
            assert_exact_definition(&mut ws, "Use.kt", expression, "Names.kt", "pick");
        } else {
            assert_eq!(inferred, Type::Unknown);
            let found = labels(&mut ws, "Use.kt", &format!("{expression}."));
            assert!(
                !found.contains("service") && !found.contains("body"),
                "stale named mapping: {found:?}"
            );
        }
        assert_member(&mut ws, "Use.kt", "known.", "body", "service");
    }
    assert_eq!(ws.doc_text("Use.kt").as_deref(), Some(source));
    assert_eq!(ws.doc_text("Names.kt").as_deref(), Some(declaration));
    let entries = ws.index.entries_for_file("Names.kt");
    let symbols: Vec<_> = entries.iter().map(|entry| (*entry.sym).clone()).collect();
    let decoded: Vec<ktlsp::symbol::IndexedSymbol> =
        bincode::deserialize(&bincode::serialize(&symbols).unwrap()).unwrap();
    assert_eq!(decoded, symbols);
    assert_eq!(
        decoded[0]
            .kotlin_parameters
            .as_deref()
            .unwrap()
            .iter()
            .map(|parameter| (parameter.name.as_str(), parameter.has_default))
            .collect::<Vec<_>>(),
        [("first", false), ("second", false)]
    );
    ws.index
        .replace_file("Names.kt", decoded, ktlsp::index::Tier::Durable);
    assert_eq!(
        infer_expression(&ws, "Use.kt", "pick(second=right,first=left)"),
        Type::class("Invocation", Some("probe".into()))
    );
}

#[test]
fn named_calls_do_not_treat_java_parameter_spelling_as_kotlin_source_names() {
    let source = "package probe
fun use(request:Request,kotlin:KotlinRequest,type:Class<Invocation>,captured:Class<out Invocation>,wrong:Class<Other>,known:Other) { request.tag(type=type);kotlin.upper(type=type)!!.service();kotlin.star(type=type);kotlin.bounded(type=type)!!.service();kotlin.bounded(type=wrong);kotlin.both(exact=captured,lower=type);kotlin.tag(type=captured)!!.service();kotlin.tag(type=type)!!.service();request.tag(type)!!.service();known.body() }
";
    let mut ws = projected_workspace(source);
    for expression in [
        "request.tag(type=type)",
        "kotlin.star(type=type)",
        "kotlin.bounded(type=wrong)",
        "kotlin.both(exact=captured,lower=type)",
    ] {
        assert_eq!(
            infer_expression(&ws, "Use.kt", expression),
            Type::Unknown,
            "{expression}"
        );
    }
    // Matching out containment infers Invocation?, while the invariant named slot above
    // cannot replace a captured argument with its upper bound.
    assert_eq!(
        infer_expression(&ws, "Use.kt", "kotlin.tag(type=captured)"),
        Type::class("Invocation", Some("probe".into())).with_nullability(true),
    );
    assert_member(
        &mut ws,
        "Use.kt",
        "kotlin.tag(type=captured)!!.",
        "service",
        "body",
    );
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "kotlin.tag(type=captured)!!.service()",
        "Invocation.java",
        "service",
    );
    assert_member(
        &mut ws,
        "Use.kt",
        "kotlin.bounded(type=type)!!.",
        "service",
        "serviceWrong",
    );
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "kotlin.bounded(type=type)!!.service()",
        "Invocation.java",
        "service",
    );
    assert_member(
        &mut ws,
        "Use.kt",
        "kotlin.tag(type=type)!!.",
        "service",
        "body",
    );
    assert_member(&mut ws, "Use.kt", "request.tag(type)!!.", "service", "body");
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "kotlin.tag(type=type)!!.service()",
        "Invocation.java",
        "service",
    );
    assert_member(&mut ws, "Use.kt", "known.", "body", "service");
    assert_member(
        &mut ws,
        "Use.kt",
        "kotlin.upper(type=type)!!.",
        "service",
        "serviceWrong",
    );
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "kotlin.upper(type=type)!!.service()",
        "Invocation.java",
        "service",
    );
}

#[test]
fn incomplete_named_calls_keep_candidates_without_using_a_partial_name_map() {
    let source = r#"package probe
fun use(shapes:NamedShapes,left:Class<Invocation>,known:Other) {
 known.body()
 shapes.pick(second=left,first=)
}
"#;
    let mut ws = projected_workspace(source);
    ws.open(
        "NamedShapes.kt",
        r#"package probe
class NamedShapes { fun <A,B> pick(first:Class<out A>,second:Class<out B>):A=error("fixture") }
"#
        .into(),
    );
    assert_member(&mut ws, "Use.kt", "known.", "body", "service");
    let tree = KotlinParser::new().parse(source);
    let start = source.find("shapes.pick").unwrap() + "shapes.".len();
    let selector = tree
        .root_node()
        .descendant_for_byte_range(start, start + "pick".len())
        .unwrap();
    let ctx = FileCtx::from_tree(&tree, source);
    let usage = ktlsp::resolve::member_use(&ws.index, selector, source, &ctx);
    let ktcore::members::MemberUse::Call(arguments) = usage else {
        panic!("unfinished argument syntax must retain a call use");
    };
    assert!(
        arguments.count.is_none(),
        "broken syntax must not certify a complete binding"
    );
    let owner = Type::class("NamedShapes", Some("probe".into()));
    let candidates = ktcore::members::candidates(&ws.index, &owner, "pick", &ctx);
    let selection = candidates.select(
        &ws.index,
        &owner,
        &ktcore::members::MemberUse::Call(arguments),
        &ctx,
    );
    assert_eq!(
        selection.applicability,
        ktcore::members::Applicability::Unknown
    );
    assert_eq!(selection.entries.len(), 1);
}

#[test]
fn named_defaults_bind_leading_middle_and_trailing_slots_for_members_and_free_calls() {
    let declarations = r#"package probe
class DefaultBridge {
 fun <T> leading(ignored:Other=Other(),required:Class<out T>):T=error("fixture")
 fun <T> middle(required:Class<out T>,ignored:Other=Other(),marker:Class<Invocation>):T=error("fixture")
 fun <T> trailing(required:Class<out T>,ignored:Other=Other()):T=error("fixture")
 fun <T> punctuation(noise:String="a=b,c",required:Class<out T> /* = not a default */,ignored:Other /* = */ = run { Other() }):T=error("fixture")
 fun <T> plain(ignored:Other=Other(),required:T):T=error("fixture")
 fun concrete(ignored:Other=Other(),required:Invocation):Invocation=error("fixture")
 fun <T> choose(required:Class<out T>,marker:Class<Invocation>,ignored:Other=Other()):T=error("fixture")
 @kotlin.jvm.JvmName("chooseOther")
 fun choose(required:Class<Other>,marker:Class<Other>,ignored:Invocation=Invocation()):Other=error("fixture")
}
fun <T> freeLeading(ignored:Other=Other(),required:Class<out T>):T=error("fixture")
fun <T> freeMiddle(required:Class<out T>,ignored:Other=Other(),marker:Class<Invocation>):T=error("fixture")
fun <T> freeTrailing(required:Class<out T>,ignored:Other=Other()):T=error("fixture")
fun <T> freePlain(ignored:Other=Other(),required:T):T=error("fixture")
"#;
    let source = r#"package probe
fun use(bridge:DefaultBridge,left:Class<Invocation>,right:Class<Other>,value:Invocation,known:Other) {
 bridge.leading(required=left).service()
 bridge.middle(marker=left,required=left).service()
 bridge.middle(left,marker=left).service()
 bridge.trailing(required=left).service()
 bridge.punctuation(required=left).service()
 bridge.plain(required=value).service()
 bridge.concrete(required=value).service()
 bridge.choose(marker=left,required=left).service()
 freeLeading(required=left).service()
 probe.freeLeading(required=left).service()
 freeMiddle(marker=left,required=left).service()
 freeTrailing(required=left).service()
 freePlain(required=value).service()
 bridge.leading(required=right).body()
 known.body()
}
"#;
    let mut ws = projected_workspace(source);
    ws.open("Defaults.kt", declarations.into());
    for expression in [
        "bridge.leading(required=left)",
        "bridge.middle(marker=left,required=left)",
        "bridge.middle(left,marker=left)",
        "bridge.trailing(required=left)",
        "bridge.punctuation(required=left)",
        "bridge.plain(required=value)",
        "bridge.concrete(required=value)",
        "bridge.choose(marker=left,required=left)",
        "freeLeading(required=left)",
        "probe.freeLeading(required=left)",
        "freeMiddle(marker=left,required=left)",
        "freeTrailing(required=left)",
        "freePlain(required=value)",
    ] {
        assert_eq!(
            infer_expression(&ws, "Use.kt", expression),
            Type::class("Invocation", Some("probe".into())),
            "{expression}"
        );
        assert_member(
            &mut ws,
            "Use.kt",
            &format!("{expression}."),
            "service",
            "body",
        );
        assert_exact_definition(
            &mut ws,
            "Use.kt",
            &format!("{expression}.service()"),
            "Invocation.java",
            "service",
        );
        let callee = expression
            .split('(')
            .next()
            .unwrap()
            .rsplit('.')
            .next()
            .unwrap();
        assert_exact_definition(&mut ws, "Use.kt", expression, "Defaults.kt", callee);
    }
    assert_member(
        &mut ws,
        "Use.kt",
        "probe.freeLeading(required=left).",
        "service",
        "body",
    );
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "probe.freeLeading(required=left)",
        "Defaults.kt",
        "freeLeading",
    );
    assert_member(
        &mut ws,
        "Use.kt",
        "bridge.leading(required=right).",
        "body",
        "service",
    );
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "bridge.leading(required=right).body()",
        "Other.java",
        "body",
    );
    assert_member(&mut ws, "Use.kt", "known.", "body", "service");
}

#[test]
fn named_default_edits_invalidate_missing_slots_then_restore_cached_call_facts() {
    let declaration = "package probe\nfun <T> withDefault(ignored:Other=Other(),required:Class<out T>):T=error(\"fixture\")\n";
    let required = declaration.replace("ignored:Other=Other()", "ignored:Other");
    let source = "package probe\nfun use(left:Class<Invocation>,known:Other) { withDefault(required=left).service();known.body() }\n";
    let supplied = source.replace("required=left", "ignored=known,required=left");
    let mut ws = projected_workspace(source);
    ws.open("Defaults.kt", declaration.into());
    for (phase, (decl, caller, expression, useful)) in [
        (declaration, source, "withDefault(required=left)", true),
        (
            required.as_str(),
            source,
            "withDefault(required=left)",
            false,
        ),
        (
            required.as_str(),
            supplied.as_str(),
            "withDefault(ignored=known,required=left)",
            true,
        ),
        (
            required.as_str(),
            source,
            "withDefault(required=left)",
            false,
        ),
        (declaration, source, "withDefault(required=left)", true),
    ]
    .into_iter()
    .enumerate()
    {
        let mut edits = 0;
        for (file, text) in [("Defaults.kt", decl), ("Use.kt", caller)] {
            if ws.doc_text(file).as_deref() != Some(text) {
                ws.change(file, text.into());
                edits += 1;
            }
        }
        assert_eq!(edits, usize::from(phase > 0));
        if useful {
            assert_eq!(
                infer_expression(&ws, "Use.kt", expression),
                Type::class("Invocation", Some("probe".into()))
            );
            assert_member(
                &mut ws,
                "Use.kt",
                &format!("{expression}."),
                "service",
                "body",
            );
            assert_exact_definition(
                &mut ws,
                "Use.kt",
                &format!("{expression}.service()"),
                "Invocation.java",
                "service",
            );
            assert_exact_definition(&mut ws, "Use.kt", expression, "Defaults.kt", "withDefault");
        } else {
            assert_eq!(infer_expression(&ws, "Use.kt", expression), Type::Unknown);
            let found = labels(&mut ws, "Use.kt", &format!("{expression}."));
            assert!(
                !found.contains("service") && !found.contains("body"),
                "stale default facts: {found:?}"
            );
        }
        assert_member(&mut ws, "Use.kt", "known.", "body", "service");
    }
    assert_eq!(ws.doc_text("Defaults.kt").as_deref(), Some(declaration));
    assert_eq!(ws.doc_text("Use.kt").as_deref(), Some(source));
    let symbols: Vec<_> = ws
        .index
        .entries_for_file("Defaults.kt")
        .iter()
        .map(|entry| (*entry.sym).clone())
        .collect();
    let decoded: Vec<ktlsp::symbol::IndexedSymbol> =
        bincode::deserialize(&bincode::serialize(&symbols).unwrap()).unwrap();
    assert_eq!(decoded, symbols);
    assert_eq!(
        decoded[0]
            .kotlin_parameters
            .as_deref()
            .unwrap()
            .iter()
            .map(|parameter| (parameter.name.as_str(), parameter.has_default))
            .collect::<Vec<_>>(),
        [("ignored", true), ("required", false)]
    );
    ws.index
        .replace_file("Defaults.kt", decoded, ktlsp::index::Tier::Durable);
    assert_member(
        &mut ws,
        "Use.kt",
        "withDefault(required=left).",
        "service",
        "body",
    );
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "withDefault(required=left).service()",
        "Invocation.java",
        "service",
    );
}

#[test]
fn named_default_calls_retain_competing_unbound_and_unresolved_candidates() {
    let declarations = r#"package probe
fun <T> competing(type:Class<out T>,left:Other=Other()):T=error("fixture")
@kotlin.jvm.JvmName("competingOther")
fun <T> competing(type:Class<out T>,right:Other=Other()):Other=error("fixture")
fun <T> unbound(value:T=error("fixture"),marker:Other):T=error("fixture")
fun <T> unresolved(type:Class<out T>,missing:Missing=error("fixture")):T=error("fixture")
class DefaultGuards {
 fun <T> unresolvedMember(type:Class<out T>,missing:Missing=error("fixture")):T=error("fixture")
 fun <T> unboundMember(value:T=error("fixture"),marker:Other):T=error("fixture")
 fun <T:Other> boundedMember(required:T,ignored:Other=Other()):T=error("fixture")
 fun <T> whereBoundedMember(required:T,ignored:Other=Other()):T where T:Other=error("fixture")
}
"#;
    let source = r#"package probe
fun use(guards:DefaultGuards,left:Class<Invocation>,value:Invocation,known:Other) {
 competing(type=left)
 unbound(marker=known)
 unresolved(type=left)
 guards.unresolvedMember(type=left)
 guards.unboundMember(marker=known)
 guards.boundedMember(required=value)
 guards.whereBoundedMember(required=value)
 known.body()
}
"#;
    let mut ws = projected_workspace(source);
    ws.open("Guards.kt", declarations.into());
    for expression in [
        "competing(type=left)",
        "unbound(marker=known)",
        "unresolved(type=left)",
        "guards.unresolvedMember(type=left)",
        "guards.unboundMember(marker=known)",
        "guards.boundedMember(required=value)",
        "guards.whereBoundedMember(required=value)",
    ] {
        assert_eq!(
            infer_expression(&ws, "Use.kt", expression),
            Type::Unknown,
            "{expression}"
        );
    }
    assert!(ws
        .goto_definition("Use.kt", source.find("competing(type=left)").unwrap())
        .is_empty());
    let tree = KotlinParser::new().parse(source);
    let ctx = FileCtx::from_tree(&tree, source);
    let owner = Type::class("DefaultGuards", Some("probe".into()));
    for (expression, name) in [
        ("guards.unresolvedMember(type=left)", "unresolvedMember"),
        ("guards.boundedMember(required=value)", "boundedMember"),
        (
            "guards.whereBoundedMember(required=value)",
            "whereBoundedMember",
        ),
    ] {
        let start = source.find(expression).unwrap();
        let call = tree
            .root_node()
            .descendant_for_byte_range(start, start + expression.len())
            .unwrap();
        let usage =
            ktcore::members::MemberUse::Call(infer::call_arguments(&ws.index, call, source, &ctx));
        let selection = ktcore::members::candidates(&ws.index, &owner, name, &ctx)
            .select(&ws.index, &owner, &usage, &ctx);
        assert_eq!(
            selection.applicability,
            ktcore::members::Applicability::Unknown
        );
        assert_eq!(selection.entries.len(), 1);
        if name != "unresolvedMember" {
            assert!(!selection.entries[0]
                .sym
                .type_parameter_facts
                .known_unbounded());
        }
    }
    assert_member(&mut ws, "Use.kt", "known.", "body", "service");
}

#[test]
fn qualified_free_calls_preserve_value_roots_and_reject_non_dot_package_paths() {
    let declaration = "package probe\nfun <T> freeLeading(ignored:Other=Other(),required:Class<out T>):T=error(\"fixture\")\n";
    let receiver = "package holder\nimport probe.Invocation\nimport probe.Other\nclass Receiver { fun freeLeading(required:Class<Invocation>):Other=error(\"fixture\") }\nval instance:Receiver=error(\"fixture\")\n";
    for (source, extra) in [
        (
            "package app\nimport holder.Receiver\nimport probe.Invocation\nimport probe.Other\nfun use(probe:Receiver,left:Class<Invocation>,known:Other) { probe.freeLeading(required=left).body();known.body() }\n",
            None,
        ),
        (
            "package app\nimport holder.instance as probe\nimport probe.Invocation\nimport probe.Other\nfun use(left:Class<Invocation>,known:Other) { probe.freeLeading(required=left).body();known.body() }\n",
            None,
        ),
        (
            "package app\nimport probe.Invocation\nimport probe.Other\nfun use(left:Class<Invocation>,known:Other) { probe.freeLeading(required=left).body();known.body() }\n",
            Some("package app\nimport holder.Receiver\nval probe:Receiver=error(\"fixture\")\n"),
        ),
    ] {
        let mut ws = projected_workspace(source);
        ws.open("Defaults.kt", declaration.into());
        ws.open("Receiver.kt", receiver.into());
        if let Some(extra) = extra {
            ws.open("RootValue.kt", extra.into());
        }
        let expression = "probe.freeLeading(required=left)";
        assert_eq!(
            infer_expression(&ws, "Use.kt", expression),
            Type::class("Other", Some("probe".into()))
        );
        assert_member(
            &mut ws,
            "Use.kt",
            &format!("{expression}."),
            "body",
            "service",
        );
        assert_exact_definition(&mut ws, "Use.kt", expression, "Receiver.kt", "freeLeading");
        assert_exact_definition(
            &mut ws,
            "Use.kt",
            &format!("{expression}.body()"),
            "Other.java",
            "body",
        );
        assert_member(&mut ws, "Use.kt", "known.", "body", "service");
    }
    let source = "package app\nimport probe.Invocation\nimport probe.Other\nfun use(probe:Missing,left:Class<Invocation>,known:Other) { probe.freeLeading(required=left);known.body() }\n";
    let mut ws = projected_workspace(source);
    ws.open("Defaults.kt", declaration.into());
    assert_eq!(
        infer_expression(&ws, "Use.kt", "probe.freeLeading(required=left)"),
        Type::Unknown
    );
    assert_member(&mut ws, "Use.kt", "known.", "body", "service");
    let source = "package app\nimport probe.Invocation\nimport probe.Other\nfun use(left:Class<Invocation>,known:Other) { probe?.freeLeading(required=left);probe::freeLeading;known.body() }\n";
    ws.change("Use.kt", source.into());
    for expression in ["probe?.freeLeading(required=left)", "probe::freeLeading"] {
        assert_eq!(
            infer_expression(&ws, "Use.kt", expression),
            Type::Unknown,
            "{expression}"
        );
    }
    assert_member(&mut ws, "Use.kt", "known.", "body", "service");
}

#[test]
fn qualified_free_named_defaults_follow_unsaved_formal_and_default_changes() {
    let declaration = "package target.api\nimport probe.Invocation\nimport probe.Other\nfun <T> withDefault(ignored:Other=Other(),required:Class<out T>):T=error(\"fixture\")\n";
    let renamed = declaration.replace("required:Class", "renamed:Class");
    let no_default = renamed.replace("ignored:Other=Other()", "ignored:Other");
    let source = "package probe\nfun use(left:Class<Invocation>,known:Other) { target.api.withDefault(required=left).service();known.body() }\n";
    let matching = source.replace("required=left", "renamed=left");
    let supplied = source.replace("required=left", "ignored=known,renamed=left");
    let mut ws = projected_workspace(source);
    ws.open("QualifiedDefaults.kt", declaration.into());
    for (phase, (decl, caller, expression, useful)) in [
        (
            declaration,
            source,
            "target.api.withDefault(required=left)",
            true,
        ),
        (
            renamed.as_str(),
            source,
            "target.api.withDefault(required=left)",
            false,
        ),
        (
            renamed.as_str(),
            matching.as_str(),
            "target.api.withDefault(renamed=left)",
            true,
        ),
        (
            no_default.as_str(),
            matching.as_str(),
            "target.api.withDefault(renamed=left)",
            false,
        ),
        (
            no_default.as_str(),
            supplied.as_str(),
            "target.api.withDefault(ignored=known,renamed=left)",
            true,
        ),
        (
            no_default.as_str(),
            source,
            "target.api.withDefault(required=left)",
            false,
        ),
        (
            declaration,
            source,
            "target.api.withDefault(required=left)",
            true,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let mut edits = 0;
        for (file, text) in [("QualifiedDefaults.kt", decl), ("Use.kt", caller)] {
            if ws.doc_text(file).as_deref() != Some(text) {
                ws.change(file, text.into());
                edits += 1;
            }
        }
        assert_eq!(edits, usize::from(phase > 0));
        if useful {
            assert_eq!(
                infer_expression(&ws, "Use.kt", expression),
                Type::class("Invocation", Some("probe".into()))
            );
            assert_member(
                &mut ws,
                "Use.kt",
                &format!("{expression}."),
                "service",
                "body",
            );
            assert_exact_definition(
                &mut ws,
                "Use.kt",
                expression,
                "QualifiedDefaults.kt",
                "withDefault",
            );
            assert_exact_definition(
                &mut ws,
                "Use.kt",
                &format!("{expression}.service()"),
                "Invocation.java",
                "service",
            );
        } else {
            assert_eq!(infer_expression(&ws, "Use.kt", expression), Type::Unknown);
            let found = labels(&mut ws, "Use.kt", &format!("{expression}."));
            assert!(
                !found.contains("service") && !found.contains("body"),
                "stale FQN bindings: {found:?}"
            );
        }
        assert_member(&mut ws, "Use.kt", "known.", "body", "service");
    }
    assert_eq!(
        ws.doc_text("QualifiedDefaults.kt").as_deref(),
        Some(declaration)
    );
    assert_eq!(ws.doc_text("Use.kt").as_deref(), Some(source));
}

fn java_resource_workspace(source: &str) -> Workspace {
    workspace(&[
        (
            "AutoCloseable.java",
            "package java.lang; public interface AutoCloseable { void close(); }",
        ),
        (
            "Body.kt",
            "package app\nclass Body : AutoCloseable {\n fun string(): String = \"body\"\n override fun close() {}\n}\n",
        ),
        (
            "Other.kt",
            "package app\nclass Other : AutoCloseable {\n fun enqueue() {}\n override fun close() {}\n}\n",
        ),
        (
            "body.java",
            "package app; class body { public static void enqueue() {} }",
        ),
        ("Use.java", source),
    ])
}

#[test]
fn java_resource_ordinary_local_completion_control() {
    for line in [
        "body.",
        "System.out.println(body.);",
        "System.out.println(body.",
    ] {
        let source = format!(
            "package app; class Use {{ void run(Body supply) {{\n Body body = supply;\n {line}\n }} }}"
        );
        let mut ws = java_resource_workspace(&source);
        assert_member(&mut ws, "Use.java", "body.", "string", "enqueue");
    }
}

#[test]
fn java_resource_typed_kotlin_receiver_completion() {
    for line in [
        "body.",
        "System.out.println(body.);",
        "System.out.println(body.",
    ] {
        let source = format!(
            "package app; class Use {{ void run(Body supply) {{\n try (Body body = supply) {{\n {line}\n }} }} }}"
        );
        let mut ws = java_resource_workspace(&source);
        assert_member(&mut ws, "Use.java", "body.", "string", "enqueue");
    }
    let source = "package app; class Use { void run(Other supply) { try (Other body = supply) { body.enqueue(); } } }";
    let mut ws = java_resource_workspace(source);
    assert_member(&mut ws, "Use.java", "body.", "enqueue", "string");
    assert_exact_definition(&mut ws, "Use.java", "body.enqueue()", "Other.kt", "enqueue");
}

#[test]
fn java_resource_scope_returns_to_the_outer_field_in_catch_finally_and_after() {
    let source = r#"package app; class Use {
 Other body;
 void run(Body supply) {
  /*before*/ body.enqueue();
  try (Body body = supply) { /*inside*/ body.string(); }
  catch (Exception failure) { /*caught*/ body.enqueue(); }
  finally { /*finished*/ body.enqueue(); }
  /*after*/ body.enqueue();
 }
}"#;
    let mut ws = java_resource_workspace(source);
    for marker in ["before", "inside", "caught", "finished", "after"] {
        let prefix = format!("/*{marker}*/ body.");
        let (expected, absent, declaration) = if marker == "inside" {
            (
                "string",
                "enqueue",
                source.find("Body body =").unwrap() + "Body ".len(),
            )
        } else {
            (
                "enqueue",
                "string",
                source.find("Other body;").unwrap() + "Other ".len(),
            )
        };
        assert_member(&mut ws, "Use.java", &prefix, expected, absent);
        let usage = source.find(&prefix).unwrap() + prefix.find("body").unwrap();
        let definitions = ws.goto_definition("Use.java", usage);
        assert_eq!(definitions.len(), 1, "{marker}: {definitions:?}");
        assert_eq!(definitions[0].file, "Use.java");
        assert_eq!(definitions[0].start_byte, declaration, "{marker}");
    }
}

#[test]
fn java_resource_unknown_declared_type_does_not_fall_back_to_a_same_named_class() {
    for line in ["body.", "System.out.println(body."] {
        let source = format!(
            "package app; class Use {{ void run(Body supply) {{\n supply.string();\n try (Missing body = supply) {{\n {line}\n }} }} }}"
        );
        let mut ws = java_resource_workspace(&source);
        assert_member(&mut ws, "Use.java", "supply.", "string", "enqueue");
        let found = labels(&mut ws, "Use.java", "body.");
        assert!(
            !found.contains("string") && !found.contains("enqueue"),
            "unknown declared type, {line}: {found:?}"
        );
    }
}

#[test]
fn java_resource_completion_recovery_keeps_nested_lexical_scopes() {
    for (case, source) in [
        (
            "resource shadows field",
            "package app; class Use { Other body; void run(Body supply) {\n try (Body body = supply) {\n System.out.println(body.\n } } }",
        ),
        (
            "finished resource cannot shadow field",
            "package app; class Use { Body body; void run(Other supply) {\n try (Other body = supply) {}\n System.out.println(body.\n } }",
        ),
        (
            "nested local shadows field",
            "package app; class Use { Other body; void run(Body supply) {\n { Body body = supply;\n System.out.println(body.\n } } }",
        ),
        (
            "finished nested local cannot shadow field",
            "package app; class Use { Body body; void run(Other supply) {\n { Other body = supply; }\n System.out.println(body.\n } }",
        ),
    ] {
        let mut ws = java_resource_workspace(source);
        let found = labels(&mut ws, "Use.java", "body.");
        assert!(found.contains("string"), "{case}: {found:?}");
        assert!(!found.contains("enqueue"), "{case}: {found:?}");
        assert_eq!(ws.doc_text("Use.java").as_deref(), Some(source));
    }
}

#[test]
fn java_var_local_and_resource_receivers_use_current_initializers() {
    for initializer in [
        "supply",
        "stored",
        "Body",
        "new Body()",
        "make()",
        "(supply)",
    ] {
        for resource in [false, true] {
            for line in [
                "body.",
                "System.out.println(body.);",
                "System.out.println(body.",
            ] {
                let binding = if resource {
                    format!("try (var body = {initializer}) {{ {line} }}")
                } else {
                    format!("var body = {initializer}; {line}")
                };
                let source = format!(
                    "package app; class Use {{ static Body stored; static Body Body; Body make() {{ return null; }} void run(Body supply) {{ {binding} }} }}"
                );
                let mut ws = java_resource_workspace(&source);
                let found = labels(&mut ws, "Use.java", "body.");
                assert!(
                    found.contains("string"),
                    "resource={resource}, initializer={initializer}, line={line}: {found:?}"
                );
                assert!(
                    !found.contains("enqueue"),
                    "resource={resource}, initializer={initializer}, line={line}: {found:?}"
                );
                assert_eq!(ws.doc_text("Use.java").as_deref(), Some(source.as_str()));
            }
        }
    }
}

#[test]
fn java_var_resource_chain_uses_the_earlier_resource_in_the_next_initializer() {
    let source = "package app; class Use { void run(Body supply) { try (var first = supply; var body = first.next().next()) { body.string(); } } }";
    let mut ws = java_resource_workspace(source);
    ws.change("Body.kt", "package app\nclass Body : AutoCloseable {\n fun string(): String = \"body\"\n fun next(): Body = this\n override fun close() {}\n}\n".into());
    assert_member(&mut ws, "Use.java", "first.", "string", "enqueue");
    assert_member(&mut ws, "Use.java", "body.", "string", "enqueue");
    assert_exact_definition(&mut ws, "Use.java", "first.next()", "Body.kt", "next");
    assert_exact_definition(&mut ws, "Use.java", "body.string()", "Body.kt", "string");
    assert_eq!(ws.doc_text("Use.java").as_deref(), Some(source));
}

#[test]
fn java_var_receiver_tracks_unsaved_cross_language_factory_return_changes() {
    let source = "package app; class Use { void run(Factory factory) { try (var body = factory.make()) { body.string(); } } }";
    let original = "package app\nclass Factory {\n fun make(): Body = Body()\n}\n";
    let changed = "package app\nclass Factory {\n fun make(): Other = Other()\n}\n";
    let mut ws = java_resource_workspace(source);
    ws.open("Factory.kt", original.into());
    assert_member(&mut ws, "Use.java", "body.", "string", "enqueue");
    assert_exact_definition(&mut ws, "Use.java", "body.string()", "Body.kt", "string");
    ws.change("Factory.kt", changed.into());
    assert_member(&mut ws, "Use.java", "body.", "enqueue", "string");
    ws.change("Factory.kt", original.into());
    assert_member(&mut ws, "Use.java", "body.", "string", "enqueue");
    assert_eq!(ws.doc_text("Use.java").as_deref(), Some(source));
}

#[test]
fn java_var_unknown_initializers_and_explicit_missing_types_keep_value_shadowing() {
    for binding in [
        "var body = missing;",
        "var body = unknown();",
        "var body = (Body);",
        "Missing body = supply;",
        "try (var body = missing)",
        "try (Missing body = supply)",
    ] {
        let statements = if binding.starts_with("try") {
            format!("{binding} {{ body.string(); }}")
        } else {
            format!("{binding} body.string();")
        };
        let source = format!(
            "package app; class Use {{ Body body; void run(Body supply) {{ {statements} }} }}"
        );
        let mut ws = java_resource_workspace(&source);
        let found = labels(&mut ws, "Use.java", "body.");
        assert!(
            !found.contains("string") && !found.contains("enqueue"),
            "{binding}: {found:?}"
        );
    }
}

#[test]
fn java_var_own_initializer_never_falls_through_to_the_outer_field() {
    for initializer in ["body", "make(body)", "body.make()", "new Body(body)"] {
        for resource in [false, true] {
            let statements = if resource {
                format!("try (var body = {initializer}) {{ body.string(); }}")
            } else {
                format!("var body = {initializer}; body.string();")
            };
            let source = format!(
                "package app; class Use {{ Body body; Body make(Body value) {{ return value; }} void run() {{ {statements} }} }}"
            );
            let mut ws = java_resource_workspace(&source);
            let found = labels(&mut ws, "Use.java", "body.");
            assert!(
                !found.contains("string") && !found.contains("enqueue"),
                "resource={resource}, initializer={initializer}: {found:?}"
            );
            let declaration = source.find("var body =").unwrap() + "var ".len();
            let usage = source.rfind("body.string()").unwrap();
            let definitions = ws.goto_definition("Use.java", usage);
            assert_eq!(
                definitions.len(),
                1,
                "resource={resource}, initializer={initializer}: {definitions:?}"
            );
            assert_eq!(definitions[0].file, "Use.java");
            assert_eq!(definitions[0].start_byte, declaration);
        }
    }
}

#[test]
fn java_var_initializer_method_names_are_separate_from_the_local_value() {
    let source = "package app; class Use { Other body; Body body() { return null; } void run() { var body = body(); body.string(); } }";
    let mut ws = java_resource_workspace(source);
    assert_member(&mut ws, "Use.java", "body.", "string", "enqueue");
    let call_name = source.find("= body()").unwrap() + "= ".len();
    let definitions = ws.goto_definition("Use.java", call_name);
    assert_eq!(definitions.len(), 1, "{definitions:?}");
    assert_eq!(definitions[0].file, "Use.java");
    assert_eq!(
        definitions[0].start_byte,
        source.find("Body body()").unwrap() + "Body ".len()
    );
    assert_exact_definition(&mut ws, "Use.java", "body.string()", "Body.kt", "string");
}

#[test]
fn java_var_alias_inference_keeps_typical_chains_and_bounds_longer_chains() {
    for count in [infer::MAX_DEPTH, infer::MAX_DEPTH + 1] {
        let mut statements = "var value0 = supply;".to_string();
        for current in 1..count {
            statements.push_str(&format!(" var value{current} = value{};", current - 1));
        }
        let prefix = format!("value{}.", count - 1);
        let source = format!(
            "package app; class Use {{ void run(Body supply) {{ {statements} {prefix}string(); }} }}"
        );
        let mut ws = java_resource_workspace(&source);
        let found = labels(&mut ws, "Use.java", &prefix);
        if count == infer::MAX_DEPTH {
            assert!(
                found.contains("string"),
                "the typical chain must keep its useful type: {found:?}"
            );
        } else {
            assert!(
                !found.contains("string"),
                "inference beyond the shared limit must remain unknown: {found:?}"
            );
        }
        assert!(
            !found.contains("enqueue"),
            "an exhausted budget must not select an unrelated type: {found:?}"
        );
    }
}

#[test]
fn java_long_generic_identity_calls_keep_the_useful_receiver_type() {
    for depth in [1, 8, 20, 32] {
        let mut expression = "supply".to_string();
        for _ in 0..depth {
            expression = format!("identity.id({expression})");
        }
        let source = format!(
            "package app; class Use {{ void run(Identity identity, Body supply) {{ {expression}.string(); }} }}"
        );
        let mut ws = java_resource_workspace(&source);
        ws.open(
            "Identity.java",
            "package app; class Identity { public <T> T id(T value) { return value; } }".into(),
        );
        let found = labels(&mut ws, "Use.java", &format!("{expression}."));
        assert!(
            found.contains("string"),
            "ordinary generic call depth {depth} must retain Body.string: {found:?}"
        );
        assert!(
            !found.contains("enqueue"),
            "ordinary call depth {depth} selected an unrelated type: {found:?}"
        );
        assert_exact_definition(
            &mut ws,
            "Use.java",
            &format!("{expression}.string()"),
            "Body.kt",
            "string",
        );
        assert_eq!(ws.doc_text("Use.java").as_deref(), Some(source.as_str()));
    }
}

#[test]
fn java_ordinary_argument_values_retain_existing_base_members() {
    for expression in ["new Body() {}", "(new Body() {})", "Body"] {
        let prefix = format!("identity.id({expression}).");
        let source = format!(
            "package app; import static app.Values.Body; class Use {{ void run(Identity identity) {{ {prefix}string(); }} }}"
        );
        let mut ws = workspace(&[
            (
                "Body.java",
                "package app; public class Body { public String string() { return null; } }",
            ),
            (
                "Values.java",
                "package app; public class Values { public static Body Body = new Body(); }",
            ),
            (
                "Identity.java",
                "package app; public class Identity { public <T> T id(T value) { return value; } }",
            ),
            ("Use.java", &source),
        ]);
        assert_member(&mut ws, "Use.java", &prefix, "string", "enqueue");
        assert_exact_definition(
            &mut ws,
            "Use.java",
            &format!("{prefix}string()"),
            "Body.java",
            "string",
        );
        assert_eq!(ws.doc_text("Use.java").as_deref(), Some(source.as_str()));
    }
}

#[test]
fn java_and_kotlin_plain_method_variables_share_the_argument_type() {
    let mut ws = java_resource_workspace(
        "package app; class Use { void run(Identity identity, Body supply) { identity.id(supply).string(); } }",
    );
    ws.open(
        "Identity.java",
        "package app; public class Identity { public <T> T id(T value) { return value; } }".into(),
    );
    ws.open(
        "Use.kt",
        "package app\nfun run(identity: Identity, supply: Body) { identity.id(supply).string() }\n"
            .into(),
    );
    for file in ["Use.java", "Use.kt"] {
        assert_member(&mut ws, file, "identity.id(supply).", "string", "enqueue");
        assert_exact_definition(
            &mut ws,
            file,
            "identity.id(supply).string()",
            "Body.kt",
            "string",
        );
    }
}

#[test]
fn java_enhanced_for_typed_receivers_complete_java_and_kotlin_owners() {
    for (owner, declaration) in [
        ("Item.kt", ITEM),
        (
            "Item.java",
            "package app; public class Item { public void useful() {} }",
        ),
    ] {
        for line in [
            "item.",
            "System.out.println(item.);",
            "System.out.println(item.",
        ] {
            let source = format!(
                "package app; class Use {{ void run(Item[] values) {{\n for (Item item : values) {{\n {line}\n }} }} }}"
            );
            let mut ws = workspace(&[
                (owner, declaration),
                ("Other.kt", OTHER),
                ("Use.java", &source),
            ]);
            assert_member(&mut ws, "Use.java", "item.", "useful", "usefulWrong");
            assert_eq!(ws.doc_text("Use.java").as_deref(), Some(source.as_str()));
        }
    }
}

#[test]
fn java_enhanced_for_scope_keeps_the_outer_field_before_and_after() {
    let source = r#"package app; class Use {
 Other item;
 void run(Item[] values) {
  /*before*/ item.usefulWrong();
  for (Item item : values) { /*inside*/ item.useful(); }
  /*after*/ item.usefulWrong();
 }
}"#;
    let mut ws = workspace(&[("Item.kt", ITEM), ("Other.kt", OTHER), ("Use.java", source)]);
    for marker in ["before", "inside", "after"] {
        let prefix = format!("/*{marker}*/ item.");
        let (expected, excluded, declaration) = if marker == "inside" {
            (
                "useful",
                "usefulWrong",
                source.find("Item item :").unwrap() + "Item ".len(),
            )
        } else {
            (
                "usefulWrong",
                "useful",
                source.find("Other item;").unwrap() + "Other ".len(),
            )
        };
        assert_member(&mut ws, "Use.java", &prefix, expected, excluded);
        let usage = source.find(&prefix).unwrap() + prefix.find("item").unwrap();
        let definitions = ws.goto_definition("Use.java", usage);
        assert_eq!(definitions.len(), 1, "{marker}: {definitions:?}");
        assert_eq!(definitions[0].file, "Use.java");
        assert_eq!(definitions[0].start_byte, declaration, "{marker}");
    }
    assert_exact_definition(&mut ws, "Use.java", "item.useful()", "Item.kt", "useful");
}

#[test]
fn java_enhanced_for_generic_receivers_keep_the_result_type() {
    let source = "package app; class Use { void run(Box<Item>[] values) { for (Box<Item> item : values) { item.get().useful(); } } }";
    let mut ws = workspace(&[
        ("Item.kt", ITEM),
        ("Other.kt", OTHER),
        ("Box.java", JAVA_BOX),
        ("Use.java", source),
    ]);
    assert_member(&mut ws, "Use.java", "item.get().", "useful", "usefulWrong");
    assert_exact_definition(
        &mut ws,
        "Use.java",
        "item.get().useful()",
        "Item.kt",
        "useful",
    );
}

#[test]
fn java_enhanced_for_result_chains_follow_unsaved_java_and_kotlin_owners() {
    let source = "package app; class Use { void run(Holder[] values) { for (Holder item : values) { item.get().useful(); } } }";
    for (owner, original) in [
        (
            "Holder.kt",
            "package app\nclass Holder { fun get(): Item = error(\"fixture\") }\n",
        ),
        (
            "Holder.java",
            "package app; public class Holder { public Item get() { return null; } }",
        ),
    ] {
        let mut ws = workspace(&[
            ("Item.kt", ITEM),
            ("Other.kt", OTHER),
            (owner, original),
            ("Use.java", source),
        ]);
        assert_member(&mut ws, "Use.java", "item.get().", "useful", "usefulWrong");
        assert_exact_definition(
            &mut ws,
            "Use.java",
            "item.get().useful()",
            "Item.kt",
            "useful",
        );
        ws.change(owner, original.replace("Item", "Other"));
        assert_member(&mut ws, "Use.java", "item.get().", "usefulWrong", "useful");
        ws.change(owner, original.into());
        assert_member(&mut ws, "Use.java", "item.get().", "useful", "usefulWrong");
        assert_exact_definition(
            &mut ws,
            "Use.java",
            "item.get().useful()",
            "Item.kt",
            "useful",
        );
        assert_eq!(ws.doc_text("Use.java").as_deref(), Some(source));
        assert_eq!(ws.doc_text(owner).as_deref(), Some(original));
    }
}

#[test]
fn java_enhanced_for_unknown_or_unsupported_types_do_not_fall_back() {
    // Valid array receivers remain completion gaps; these are only no-false-type controls.
    for (binding, values_type) in [
        ("Missing item", "Item[]"),
        ("var item", "Missing[]"),
        ("Item item[]", "Item[][]"),
        ("Item[] item", "Item[][]"),
    ] {
        let source = format!(
            "package app; class Use {{ Other item; void run({values_type} values) {{ for ({binding} : values) {{\n item.\n }} }} }}"
        );
        let mut ws = workspace(&[
            ("Item.kt", ITEM),
            ("Other.kt", OTHER),
            ("Use.java", &source),
        ]);
        let found = labels(&mut ws, "Use.java", "item.");
        assert!(
            !found.contains("useful") && !found.contains("usefulWrong"),
            "{binding}: {found:?}"
        );
    }
}

const JAVA_FOREACH_ITERABLE: &str = "package java.lang; public interface Iterable<T> {}";

#[test]
fn java_enhanced_for_var_array_and_iterable_receivers_have_useful_completion() {
    for (owner, declaration) in [
        ("Item.kt", ITEM),
        (
            "Item.java",
            "package app; public class Item { public void useful() {} }",
        ),
    ] {
        for values in [
            "Item[] values",
            "Item values[]",
            "java.lang.Iterable<Item> values",
        ] {
            for line in [
                "item.",
                "System.out.println(item.);",
                "System.out.println(item.",
            ] {
                let source = format!(
                    "package app; class Use {{ void run({values}) {{\n for (var item : (values)) {{\n {line}\n }} }} }}"
                );
                let mut ws = workspace(&[
                    (owner, declaration),
                    ("Other.kt", OTHER),
                    ("Iterable.java", JAVA_FOREACH_ITERABLE),
                    ("Use.java", &source),
                ]);
                assert_member(&mut ws, "Use.java", "item.", "useful", "usefulWrong");
                assert_eq!(ws.doc_text("Use.java").as_deref(), Some(source.as_str()));
            }
        }
    }
}

#[test]
fn java_enhanced_for_var_generic_hierarchy_and_array_elements_keep_substitution() {
    for values in ["Values<Item> values", "Box<Item>[] values"] {
        let expression = if values.starts_with("Values") {
            "item."
        } else {
            "item.get()."
        };
        let source = format!(
            "package app; class Use {{ void run({values}) {{ for (var item : values) {{ {expression}useful(); }} }} }}"
        );
        let mut ws = workspace(&[
            ("Item.kt", ITEM),
            ("Other.kt", OTHER),
            ("Box.java", JAVA_BOX),
            ("Iterable.java", JAVA_FOREACH_ITERABLE),
            (
                "Values.java",
                "package app; abstract class Values<T> implements java.lang.Iterable<T> {}",
            ),
            ("Use.java", &source),
        ]);
        assert_member(&mut ws, "Use.java", expression, "useful", "usefulWrong");
        assert_exact_definition(
            &mut ws,
            "Use.java",
            &format!("{expression}useful()"),
            "Item.kt",
            "useful",
        );
    }
}

#[test]
fn java_enhanced_for_var_keeps_lexical_values_and_loop_scope() {
    let source = r#"package app; class Use {
 Other item;
 Item[] values;
 void run(Other[] values) {
  /*before*/ item.usefulWrong();
  for (var item : values) { /*inside*/ item.usefulWrong(); }
  /*after*/ item.usefulWrong();
 }
 Other[] sameItems;
 void sameName() { for (var sameItems : sameItems) { /*same*/ sameItems.usefulWrong(); } }
}"#;
    let mut ws = workspace(&[("Item.kt", ITEM), ("Other.kt", OTHER), ("Use.java", source)]);
    for marker in ["before", "inside", "after", "same"] {
        let name = if marker == "same" {
            "sameItems"
        } else {
            "item"
        };
        let prefix = format!("/*{marker}*/ {name}.");
        assert_member(&mut ws, "Use.java", &prefix, "usefulWrong", "useful");
        let usage = source.find(&prefix).unwrap() + prefix.find(name).unwrap();
        let definitions = ws.goto_definition("Use.java", usage);
        let declaration = if marker == "inside" {
            source.find("var item : values").unwrap() + "var ".len()
        } else if marker == "same" {
            source.find("var sameItems : sameItems").unwrap() + "var ".len()
        } else {
            source.find("Other item;").unwrap() + "Other ".len()
        };
        assert_eq!(definitions.len(), 1, "{marker}: {definitions:?}");
        assert_eq!(definitions[0].file, "Use.java");
        assert_eq!(definitions[0].start_byte, declaration, "{marker}");
    }
}

#[test]
fn java_enhanced_for_var_tracks_unsaved_element_and_hierarchy_edits() {
    for values in ["Item[] values", "java.lang.Iterable<Item> values"] {
        let source = format!(
            "package app; class Use {{ void run({values}) {{ for (var item : values) {{ item.useful(); }} }} }}"
        );
        let mut ws = workspace(&[
            ("Item.kt", ITEM),
            ("Other.kt", OTHER),
            ("Iterable.java", JAVA_FOREACH_ITERABLE),
            ("Use.java", &source),
        ]);
        assert_member(&mut ws, "Use.java", "item.", "useful", "usefulWrong");
        ws.change("Use.java", source.replace("Item", "Other"));
        assert_member(&mut ws, "Use.java", "item.", "usefulWrong", "useful");
        ws.change("Use.java", source.clone());
        assert_member(&mut ws, "Use.java", "item.", "useful", "usefulWrong");
        assert_exact_definition(&mut ws, "Use.java", "item.useful()", "Item.kt", "useful");
        ws.change("Item.kt", ITEM.replace("useful", "renamedUseful"));
        assert_member(&mut ws, "Use.java", "item.", "renamedUseful", "useful");
        ws.change("Item.kt", ITEM.into());
        assert_member(&mut ws, "Use.java", "item.", "useful", "renamedUseful");
    }
    let values = "package app; abstract class Values<T> implements java.lang.Iterable<T> {}";
    let source = "package app; class Use { void run(Values<Item> values) { for (var item : values) { item.useful(); } } }";
    let mut ws = workspace(&[
        ("Item.kt", ITEM),
        ("Other.kt", OTHER),
        ("Iterable.java", JAVA_FOREACH_ITERABLE),
        ("Values.java", values),
        ("Use.java", source),
    ]);
    assert_member(&mut ws, "Use.java", "item.", "useful", "usefulWrong");
    ws.change(
        "Values.java",
        values.replace("Iterable<T>", "Iterable<Other>"),
    );
    assert_member(&mut ws, "Use.java", "item.", "usefulWrong", "useful");
    ws.change("Values.java", values.into());
    assert_member(&mut ws, "Use.java", "item.", "useful", "usefulWrong");
}

#[test]
fn java_enhanced_for_var_unsupported_or_ambiguous_elements_never_invent_a_scalar() {
    // Raw/captured iterable and array-valued elements remain valid completion gaps, not useful acceptance passes.
    for values in [
        "Missing[] values",
        "Item[][] values",
        "Item[] values[]",
        "java.lang.Iterable values",
        "java.lang.Iterable<? extends Item> values",
        "java.lang.Iterable<? super Item> values",
        "java.lang.Iterable<?> values",
        "java.lang.Iterable<Missing> values",
        "NotIterable<Item> values",
    ] {
        let source = format!(
            "package app; class Use {{ Item item; void run({values}) {{ for (var item : values) {{\n item.\n }} }} }}"
        );
        let mut ws = workspace(&[
            ("Item.kt", ITEM),
            ("Other.kt", OTHER),
            ("Iterable.java", JAVA_FOREACH_ITERABLE),
            ("NotIterable.java", "package app; class NotIterable<T> {}"),
            ("Use.java", &source),
        ]);
        let found = labels(&mut ws, "Use.java", "item.");
        assert!(
            !found.contains("useful") && !found.contains("usefulWrong"),
            "{values}: {found:?}"
        );
    }
    for declaration in [
        "package app; abstract class Values implements java.lang.Iterable<Item>, Missing {}",
        "package app; interface Both extends java.lang.Iterable<Other> {} abstract class Values implements java.lang.Iterable<Item>, Both {}",
    ] {
        let source = "package app; class Use { Item item; void run(Values values) { for (var item : values) { item.useful(); } } }";
        let mut ws = workspace(&[
            ("Item.kt", ITEM),
            ("Other.kt", OTHER),
            ("Iterable.java", JAVA_FOREACH_ITERABLE),
            ("Values.java", declaration),
            ("Use.java", source),
        ]);
        let found = labels(&mut ws, "Use.java", "item.");
        assert!(
            !found.contains("useful") && !found.contains("usefulWrong"),
            "{declaration}: {found:?}"
        );
    }
}

const JAVA_ARRAY_FOREACH_USE: &str = r#"package app;

class Use {
    private Item[] makeItems() { return new Item[0]; }
    private Item makePostfixed()[] { return new Item[0]; }
    private Item makeMatrix()[][] { return new Item[0][0]; }

    void methods() {
        for (var methodItem : makeItems()) {
            Item exactMethodItem = methodItem;
            methodItem.useful();
        }
        for (var postfixedItem : makePostfixed()) {
            Item exactPostfixedItem = postfixedItem;
            postfixedItem.useful();
        }
    }

    void creation() {
        for (var createdItem : new Item[0]) {
            Item exactCreatedItem = createdItem;
            createdItem.useful();
        }
        for (var initializedItem : new Item[] { new Item() }) {
            Item exactInitializedItem = initializedItem;
            initializedItem.useful();
        }
    }

    void aliases(Item[] original) {
        var declaredAlias = original;
        for (var declaredAliasItem : declaredAlias) {
            Item exactDeclaredAliasItem = declaredAliasItem;
            declaredAliasItem.useful();
        }
        var methodAlias = makeItems();
        for (var methodAliasItem : methodAlias) {
            Item exactMethodAliasItem = methodAliasItem;
            methodAliasItem.useful();
        }
        var newAlias = new Item[0];
        for (var newAliasItem : newAlias) {
            Item exactNewAliasItem = newAliasItem;
            newAliasItem.useful();
        }
    }

    void multidimensional(Item[][] original) {
        var matrixAlias = original;
        for (var declaredRow : matrixAlias) {
            Item[] exactDeclaredRow = declaredRow;
            int declaredLength = declaredRow.length;
            for (var declaredCell : declaredRow) {
                Item exactDeclaredCell = declaredCell;
                declaredCell.useful();
            }
        }
        for (var methodRow : makeMatrix()) {
            Item[] exactMethodRow = methodRow;
            int methodLength = methodRow.length;
            for (var methodCell : methodRow) {
                Item exactMethodCell = methodCell;
                methodCell.useful();
            }
        }
        for (var createdRow : new Item[0][0]) {
            Item[] exactCreatedRow = createdRow;
            int createdLength = createdRow.length;
            for (var createdCell : createdRow) {
                Item exactCreatedCell = createdCell;
                createdCell.useful();
            }
        }
    }
}
"#;
const JAVA_ARRAY_ITERABLE_ITEM: &str = r#"package app;

public class Item implements java.lang.Iterable<Other> {
    public void useful() { }
    @Override public java.util.Iterator<Other> iterator() {
        return java.util.Collections.emptyIterator();
    }
}
"#;

#[test]
fn java_enhanced_for_rank_one_method_array_returns_complete_java_and_kotlin_owners() {
    for (owner, declaration) in [("Item.java", JAVA_ARRAY_ITERABLE_ITEM), ("Item.kt", ITEM)] {
        let mut ws = workspace(&[
            (owner, declaration),
            ("Other.kt", OTHER),
            ("Iterable.java", JAVA_FOREACH_ITERABLE),
            ("Use.java", JAVA_ARRAY_FOREACH_USE),
        ]);
        assert_member(&mut ws, "Use.java", "methodItem.", "useful", "usefulWrong");
        assert_exact_definition(&mut ws, "Use.java", "methodItem.useful()", owner, "useful");
    }
}

#[test]
fn java_enhanced_for_rank_one_array_creation_completes_java_and_kotlin_owners() {
    for (owner, declaration) in [("Item.java", JAVA_ARRAY_ITERABLE_ITEM), ("Item.kt", ITEM)] {
        let mut ws = workspace(&[
            (owner, declaration),
            ("Other.kt", OTHER),
            ("Iterable.java", JAVA_FOREACH_ITERABLE),
            ("Use.java", JAVA_ARRAY_FOREACH_USE),
        ]);
        for name in ["createdItem", "initializedItem"] {
            assert_member(
                &mut ws,
                "Use.java",
                &format!("{name}."),
                "useful",
                "usefulWrong",
            );
            assert_exact_definition(
                &mut ws,
                "Use.java",
                &format!("{name}.useful()"),
                owner,
                "useful",
            );
        }
    }
}

#[test]
fn java_enhanced_for_postfixed_array_methods_do_not_become_iterable_scalars() {
    for (owner, declaration) in [("Item.java", JAVA_ARRAY_ITERABLE_ITEM), ("Item.kt", ITEM)] {
        let mut ws = workspace(&[
            (owner, declaration),
            ("Other.kt", OTHER),
            ("Iterable.java", JAVA_FOREACH_ITERABLE),
            ("Use.java", JAVA_ARRAY_FOREACH_USE),
        ]);
        let found = labels(&mut ws, "Use.java", "postfixedItem.");
        assert!(
            !found.contains("usefulWrong"),
            "postfixed Item[] was misread as scalar Iterable<Other>: {found:?}"
        );
        assert_member(
            &mut ws,
            "Use.java",
            "postfixedItem.",
            "useful",
            "usefulWrong",
        );
        assert_exact_definition(
            &mut ws,
            "Use.java",
            "postfixedItem.useful()",
            owner,
            "useful",
        );
        // Rank2 row completion may remain Unknown, but must never borrow either scalar owner.
        for name in ["declaredRow", "methodRow", "createdRow"] {
            let found = labels(&mut ws, "Use.java", &format!("{name}."));
            assert!(
                !found.contains("useful") && !found.contains("usefulWrong"),
                "array row {name}: {found:?}"
            );
        }
    }
}

const JAVA_ARRAY_FACTORY_SCOPED: &str = r#"package factory;
import library.Item;
public class Factory<T> {
    public Item[] imported() { return null; }
    public Item postfixed()[] { return null; }
    public library.Item[] qualified() { return null; }
    public T[] generic() { return null; }
    public <U> U[] from(U value) { return null; }
}
"#;
const JAVA_ARRAY_FACTORY_USE: &str = r#"package app;
import factory.Factory;
class Use {
    void run(Factory<library.Item> factory, library.Item value) {
        for (var importedElement : factory.imported()) { importedElement.useful(); }
        for (var postfixedElement : factory.postfixed()) { postfixedElement.useful(); }
        for (var qualifiedElement : factory.qualified()) { qualifiedElement.useful(); }
        for (var genericElement : factory.generic()) { genericElement.useful(); }
        for (var methodGenericElement : factory.from(value)) { methodGenericElement.useful(); }
        for (var createdElement : new library.Item[0]) { createdElement.useful(); }
    }
}
"#;
const JAVA_ARRAY_LIBRARY_ITEM: &str = r#"package library; public class Item { public void useful() {} }
"#;
const JAVA_ARRAY_POISON_ITEM: &str = r#"package app; public class Item { public void usefulWrong() {} }
"#;

fn scoped_array_factory_workspace() -> Workspace {
    workspace(&[
        ("Factory.java", JAVA_ARRAY_FACTORY_SCOPED),
        ("Use.java", JAVA_ARRAY_FACTORY_USE),
        ("LibraryItem.java", JAVA_ARRAY_LIBRARY_ITEM),
        ("PoisonItem.java", JAVA_ARRAY_POISON_ITEM),
    ])
}

#[test]
fn java_enhanced_for_array_methods_preserve_imports_and_shared_generic_substitution() {
    let mut ws = scoped_array_factory_workspace();
    for name in [
        "importedElement",
        "postfixedElement",
        "qualifiedElement",
        "genericElement",
        "methodGenericElement",
        "createdElement",
    ] {
        assert_member(
            &mut ws,
            "Use.java",
            &format!("{name}."),
            "useful",
            "usefulWrong",
        );
        assert_exact_definition(
            &mut ws,
            "Use.java",
            &format!("{name}.useful()"),
            "LibraryItem.java",
            "useful",
        );
    }
}

#[test]
fn java_enhanced_for_array_methods_follow_unsaved_declaration_import_changes() {
    let mut ws = scoped_array_factory_workspace();
    for name in ["importedElement", "postfixedElement"] {
        assert_member(
            &mut ws,
            "Use.java",
            &format!("{name}."),
            "useful",
            "usefulWrong",
        );
    }
    ws.change(
        "Factory.java",
        JAVA_ARRAY_FACTORY_SCOPED.replace("import library.Item;", "import app.Item;"),
    );
    for name in ["importedElement", "postfixedElement"] {
        assert_member(
            &mut ws,
            "Use.java",
            &format!("{name}."),
            "usefulWrong",
            "useful",
        );
    }
    // Explicit qualification and T inferred from the current receiver/argument stay independent.
    for name in ["qualifiedElement", "genericElement", "methodGenericElement"] {
        assert_member(
            &mut ws,
            "Use.java",
            &format!("{name}."),
            "useful",
            "usefulWrong",
        );
    }
    ws.change("Factory.java", JAVA_ARRAY_FACTORY_SCOPED.into());
    for name in ["importedElement", "postfixedElement"] {
        assert_member(
            &mut ws,
            "Use.java",
            &format!("{name}."),
            "useful",
            "usefulWrong",
        );
        assert_exact_definition(
            &mut ws,
            "Use.java",
            &format!("{name}.useful()"),
            "LibraryItem.java",
            "useful",
        );
    }
}

const JAVA_VAR_ARRAY_ALIAS_FACTORY: &str = r#"package app;
public class Factory {
    public Item[] items() { return new Item[0]; }
    public Item[][] matrix() { return new Item[0][0]; }
}
"#;
const JAVA_VAR_ARRAY_ALIAS_USE: &str = r#"package app;
class AliasUse {
    void run(Factory factory) {
        var made = factory.items();
        for (var methodElement : made) {
            Item exactMethodElement = methodElement;
            methodElement.useful();
        }
        var created = new Item[0];
        for (var createdElement : created) {
            Item exactCreatedElement = createdElement;
            createdElement.useful();
        }
    }
    void rows(Factory factory, Item[][] input) {
        var fromCreated = new Item[0][0];
        for (var createdRow : fromCreated) {
            Item[] exactCreatedRow = createdRow;
            int createdLength = createdRow.length;
        }
        var fromMethod = factory.matrix();
        for (var methodRow : fromMethod) {
            Item[] exactMethodRow = methodRow;
            int methodLength = methodRow.length;
        }
        var fromDeclared = input;
        for (var declaredRow : fromDeclared) {
            Item[] exactDeclaredRow = declaredRow;
            int declaredLength = declaredRow.length;
        }
    }
}
"#;

#[test]
fn java_enhanced_for_var_array_alias_components_complete_current_java_and_kotlin_owners() {
    // These three exact alias bodies already occur in the independently compiled array fixture.
    for (owner, declaration) in [("Item.java", JAVA_ARRAY_ITERABLE_ITEM), ("Item.kt", ITEM)] {
        let mut ws = workspace(&[
            (owner, declaration),
            ("Other.kt", OTHER),
            ("Iterable.java", JAVA_FOREACH_ITERABLE),
            ("Use.java", JAVA_ARRAY_FOREACH_USE),
        ]);
        for name in ["declaredAliasItem", "methodAliasItem", "newAliasItem"] {
            assert_member(
                &mut ws,
                "Use.java",
                &format!("{name}."),
                "useful",
                "usefulWrong",
            );
            assert_exact_definition(
                &mut ws,
                "Use.java",
                &format!("{name}.useful()"),
                owner,
                "useful",
            );
        }
    }
}

#[test]
fn java_enhanced_for_var_array_alias_unsaved_factory_and_initializer_changes_are_current() {
    for (owner, declaration) in [("Item.java", JAVA_ARRAY_ITERABLE_ITEM), ("Item.kt", ITEM)] {
        let mut ws = workspace(&[
            (owner, declaration),
            ("Other.kt", OTHER),
            ("Iterable.java", JAVA_FOREACH_ITERABLE),
            ("Factory.java", JAVA_VAR_ARRAY_ALIAS_FACTORY),
            ("AliasUse.java", JAVA_VAR_ARRAY_ALIAS_USE),
        ]);
        for name in ["methodElement", "createdElement"] {
            assert_member(
                &mut ws,
                "AliasUse.java",
                &format!("{name}."),
                "useful",
                "usefulWrong",
            );
            assert_exact_definition(
                &mut ws,
                "AliasUse.java",
                &format!("{name}.useful()"),
                owner,
                "useful",
            );
        }
        // Factory changes first: the call body is temporarily invalid, but completion still follows
        // the current array component. An independently compiled valid counterpart follows below.
        ws.change(
            "Factory.java",
            JAVA_VAR_ARRAY_ALIAS_FACTORY.replace("Item", "Other"),
        );
        assert_member(
            &mut ws,
            "AliasUse.java",
            "methodElement.",
            "usefulWrong",
            "useful",
        );
        assert_member(
            &mut ws,
            "AliasUse.java",
            "createdElement.",
            "useful",
            "usefulWrong",
        );
        ws.change(
            "AliasUse.java",
            JAVA_VAR_ARRAY_ALIAS_USE
                .replace("Item", "Other")
                .replace(".useful()", ".usefulWrong()"),
        );
        for name in ["methodElement", "createdElement"] {
            assert_member(
                &mut ws,
                "AliasUse.java",
                &format!("{name}."),
                "usefulWrong",
                "useful",
            );
            assert_exact_definition(
                &mut ws,
                "AliasUse.java",
                &format!("{name}.usefulWrong()"),
                "Other.kt",
                "usefulWrong",
            );
        }
        ws.change("Factory.java", JAVA_VAR_ARRAY_ALIAS_FACTORY.into());
        ws.change("AliasUse.java", JAVA_VAR_ARRAY_ALIAS_USE.into());
        for name in ["methodElement", "createdElement"] {
            assert_member(
                &mut ws,
                "AliasUse.java",
                &format!("{name}."),
                "useful",
                "usefulWrong",
            );
            assert_exact_definition(
                &mut ws,
                "AliasUse.java",
                &format!("{name}.useful()"),
                owner,
                "useful",
            );
        }
    }
}

#[test]
fn java_enhanced_for_var_array_alias_rank_two_rows_never_become_scalar_components() {
    for (owner, declaration) in [("Item.java", JAVA_ARRAY_ITERABLE_ITEM), ("Item.kt", ITEM)] {
        let mut ws = workspace(&[
            (owner, declaration),
            ("Other.kt", OTHER),
            ("Iterable.java", JAVA_FOREACH_ITERABLE),
            ("Factory.java", JAVA_VAR_ARRAY_ALIAS_FACTORY),
            ("AliasUse.java", JAVA_VAR_ARRAY_ALIAS_USE),
        ]);
        for name in ["createdRow", "methodRow", "declaredRow"] {
            let found = labels(&mut ws, "AliasUse.java", &format!("{name}."));
            assert!(
                !found.contains("useful") && !found.contains("usefulWrong"),
                "rank2 alias row {name} leaked a scalar owner: {found:?}"
            );
        }
    }
}

#[test]
fn java_enhanced_for_var_array_aliases_keep_lexical_bindings_and_initializer_method_names() {
    let source = r#"package app; class Use {
 Item[] values;
 Other[] values() { return new Other[0]; }
 void localShadow() {
  var values = (new Other[0]);
  var alias = (values);
  for (var shadowElement : alias) { shadowElement.usefulWrong(); }
 }
 void methodName() {
  var values = values();
  for (var methodElement : values) { methodElement.usefulWrong(); }
 }
}"#;
    let mut ws = workspace(&[("Item.kt", ITEM), ("Other.kt", OTHER), ("Use.java", source)]);
    for name in ["shadowElement", "methodElement"] {
        assert_member(
            &mut ws,
            "Use.java",
            &format!("{name}."),
            "usefulWrong",
            "useful",
        );
        assert_exact_definition(
            &mut ws,
            "Use.java",
            &format!("{name}.usefulWrong()"),
            "Other.kt",
            "usefulWrong",
        );
    }
    let usage =
        source.find("for (var shadowElement : alias)").unwrap() + "for (var shadowElement : ".len();
    let definitions = ws.goto_definition("Use.java", usage);
    assert_eq!(definitions.len(), 1, "{definitions:?}");
    assert_eq!(definitions[0].file, "Use.java");
    assert_eq!(
        definitions[0].start_byte,
        source.find("var alias =").unwrap() + "var ".len()
    );

    // Incomplete/invalid initializers must keep the local value shadow without using the field.
    for initializer in [
        "values",
        "wrap(values)",
        "values.make()",
        "missing",
        "unknown()",
        "null",
    ] {
        let source = format!(
            "package app; class Use {{ Item[] values; Item[] wrap(Item[] value) {{ return value; }} void run() {{ var values = {initializer}; for (var item : values) {{ item.useful(); }} }} }}"
        );
        let mut ws = workspace(&[
            ("Item.kt", ITEM),
            ("Other.kt", OTHER),
            ("Use.java", &source),
        ]);
        let found = labels(&mut ws, "Use.java", "item.");
        assert!(
            !found.contains("useful") && !found.contains("usefulWrong"),
            "{initializer}: {found:?}"
        );
    }
}

#[test]
fn java_enhanced_for_var_array_aliases_share_the_initializer_depth_limit() {
    for count in [infer::MAX_DEPTH - 1, infer::MAX_DEPTH + 1] {
        let mut statements = "var value0 = input;".to_string();
        for current in 1..count {
            statements.push_str(&format!(" var value{current} = value{};", current - 1));
        }
        let source = format!(
            "package app; class Use {{ void run(Item[] input) {{ {statements} for (var item : value{}) {{ item.useful(); }} }} }}",
            count - 1
        );
        let mut ws = workspace(&[
            ("Item.kt", ITEM),
            ("Other.kt", OTHER),
            ("Use.java", &source),
        ]);
        let found = labels(&mut ws, "Use.java", "item.");
        if count < infer::MAX_DEPTH {
            assert!(
                found.contains("useful"),
                "bounded alias chain lost its component: {found:?}"
            );
        } else {
            assert!(
                !found.contains("useful"),
                "exhausted chain must stay Unknown: {found:?}"
            );
        }
        assert!(
            !found.contains("usefulWrong"),
            "alias budget leaked an unrelated scalar: {found:?}"
        );
    }
}

#[test]
fn java_enhanced_for_var_array_aliases_preserve_explicit_arrays_iterables_and_substitution() {
    for (parameter, statements, receiver) in [
        (
            "Item[] input",
            "Item[] explicit = input; var alias = explicit;",
            "item.",
        ),
        (
            "java.lang.Iterable<Item> input",
            "var alias = (input);",
            "item.",
        ),
        ("Box<Item>[] input", "var alias = input;", "item.get()."),
    ] {
        let source = format!(
            "package app; class Use {{ void run({parameter}) {{ {statements} for (var item : alias) {{ {receiver}useful(); }} }} }}"
        );
        let mut ws = workspace(&[
            ("Item.kt", ITEM),
            ("Other.kt", OTHER),
            ("Box.java", JAVA_BOX),
            ("Iterable.java", JAVA_FOREACH_ITERABLE),
            ("Use.java", &source),
        ]);
        assert_member(&mut ws, "Use.java", receiver, "useful", "usefulWrong");
        assert_exact_definition(
            &mut ws,
            "Use.java",
            &format!("{receiver}useful()"),
            "Item.kt",
            "useful",
        );
    }
}

const JAVA_RANK_TWO_ARRAY_MEMBERS_USE: &str = r#"package app;
class ArrayUse<T> {
    Item[][] matrix(Item[][] input) { return input; }
    void arrays(Item[][] values, int[] counts, Item postfixed[][]) {
        for (var row : values) {
            int length = row.length;
            int copyLength = row.clone().length;
            row[0].useful();
            row.clone()[0].useful();
            for (var cell : row.clone()) { cell.useful(); }
        }
        matrix(values).clone()[0][0].useful();
        values[0].clone()[0].useful();
        int count = counts.clone().length;
        int postLength = postfixed[0].length;
        var created = new Item[counts[0]][0];
        created.clone()[0][0].useful();
    }
    void shadow(Other[][] values) {
        for (var shadowRow : values) { shadowRow.clone()[0].usefulWrong(); }
    }
    void generic(Box<Item>[][] boxes) {
        boxes.clone()[0][0].get().useful();
    }
}
"#;

#[test]
fn java_rank_two_rows_offer_length_clone_and_nested_scalar_members() {
    for (owner, declaration) in [("Item.java", JAVA_ARRAY_ITERABLE_ITEM), ("Item.kt", ITEM)] {
        let mut ws = workspace(&[
            (owner, declaration),
            ("Other.kt", OTHER),
            ("Iterable.java", JAVA_FOREACH_ITERABLE),
            ("Use.java", JAVA_ARRAY_FOREACH_USE),
        ]);
        for name in ["declaredRow", "methodRow", "createdRow"] {
            let found = labels(&mut ws, "Use.java", &format!("{name}."));
            assert!(
                found.contains("length") && found.contains("clone"),
                "array row {name}: {found:?}"
            );
            assert!(
                !found.contains("useful") && !found.contains("usefulWrong"),
                "scalar leak {name}: {found:?}"
            );
        }
        for name in ["declaredCell", "methodCell", "createdCell"] {
            assert_member(
                &mut ws,
                "Use.java",
                &format!("{name}."),
                "useful",
                "usefulWrong",
            );
            assert_exact_definition(
                &mut ws,
                "Use.java",
                &format!("{name}.useful()"),
                owner,
                "useful",
            );
        }
    }
}

#[test]
fn java_array_clone_index_and_postfixed_chains_keep_rank_and_component_identity() {
    for (owner, declaration) in [("Item.java", JAVA_ARRAY_ITERABLE_ITEM), ("Item.kt", ITEM)] {
        let mut ws = workspace(&[
            (owner, declaration),
            ("Other.kt", OTHER),
            ("Iterable.java", JAVA_FOREACH_ITERABLE),
            ("Box.java", JAVA_BOX),
            ("ArrayUse.java", JAVA_RANK_TWO_ARRAY_MEMBERS_USE),
        ]);
        for prefix in ["row.", "row.clone().", "counts.clone().", "postfixed[0]."] {
            let found = labels(&mut ws, "ArrayUse.java", prefix);
            assert!(
                found.contains("length") && found.contains("clone"),
                "{prefix}: {found:?}"
            );
            assert!(
                !found.contains("useful") && !found.contains("usefulWrong"),
                "scalar leak {prefix}: {found:?}"
            );
        }
        for prefix in [
            "row[0].",
            "row.clone()[0].",
            "cell.",
            "matrix(values).clone()[0][0].",
            "values[0].clone()[0].",
            "created.clone()[0][0].",
            "boxes.clone()[0][0].get().",
        ] {
            assert_member(&mut ws, "ArrayUse.java", prefix, "useful", "usefulWrong");
            assert_exact_definition(
                &mut ws,
                "ArrayUse.java",
                &format!("{prefix}useful()"),
                owner,
                "useful",
            );
        }
        assert_member(
            &mut ws,
            "ArrayUse.java",
            "shadowRow.clone()[0].",
            "usefulWrong",
            "useful",
        );
    }
}

#[test]
fn java_array_members_keep_lexical_parameters_and_invalid_clone_calls_unknown() {
    let source = r#"package app; class Use<Item> {
      void run(Item[][] values) {
        int length = values[0].clone().length;
        values[0][0].missing();
      }
    }"#;
    let mut ws = workspace(&[("Item.kt", ITEM), ("Use.java", source)]);
    let found = labels(&mut ws, "Use.java", "values[0].clone().");
    assert!(
        found.contains("length") && found.contains("clone"),
        "{found:?}"
    );
    assert!(!labels(&mut ws, "Use.java", "values[0][0].").contains("useful"));
    for expression in [
        "values.clone(1)",
        "values.length()",
        "values[0L]",
        "values[\"wrong\"]",
    ] {
        let source = format!(
            "package app; class Use {{ void run(Item[][] values) {{ {expression}.missing(); }} }}"
        );
        ws.change("Use.java", source);
        let found = labels(&mut ws, "Use.java", &format!("{expression}."));
        assert!(
            !found.contains("useful") && !found.contains("length") && !found.contains("clone"),
            "{expression}: {found:?}"
        );
    }
    ws.change("Use.java", "package app; class Use { void run(Item[][] values) { for (var row[] : values) { row.missing(); } } }".into());
    let found = labels(&mut ws, "Use.java", "row.");
    assert!(
        !found.contains("length") && !found.contains("clone") && !found.contains("useful"),
        "invalid var dimensions: {found:?}"
    );
}

#[test]
fn java_array_clone_chains_follow_unsaved_return_component_and_rank_changes() {
    let factory = "package app; class Factory { Item[][] matrix() { return null; } }";
    let original = "package app; class Use { void run(Factory factory) { factory.matrix().clone()[0][0].useful(); } }";
    let mut ws = workspace(&[
        ("Item.kt", ITEM),
        ("Other.kt", OTHER),
        ("Factory.java", factory),
        ("Use.java", original),
    ]);
    assert_member(
        &mut ws,
        "Use.java",
        "factory.matrix().clone()[0][0].",
        "useful",
        "usefulWrong",
    );
    ws.change("Factory.java", factory.replace("Item", "Other"));
    assert_member(
        &mut ws,
        "Use.java",
        "factory.matrix().clone()[0][0].",
        "usefulWrong",
        "useful",
    );
    ws.change("Use.java", original.replace(".useful()", ".usefulWrong()"));
    assert_exact_definition(
        &mut ws,
        "Use.java",
        "factory.matrix().clone()[0][0].usefulWrong()",
        "Other.kt",
        "usefulWrong",
    );
    ws.change("Factory.java", factory.replace("Item[][]", "Item[]"));
    let found = labels(&mut ws, "Use.java", "factory.matrix().clone()[0][0].");
    assert!(
        !found.contains("useful") && !found.contains("usefulWrong") && !found.contains("length"),
        "rank leaked: {found:?}"
    );
    ws.change("Use.java", original.replace("[0][0]", "[0]"));
    assert_member(
        &mut ws,
        "Use.java",
        "factory.matrix().clone()[0].",
        "useful",
        "usefulWrong",
    );
    ws.change("Factory.java", factory.into());
    ws.change("Use.java", original.into());
    assert_member(
        &mut ws,
        "Use.java",
        "factory.matrix().clone()[0][0].",
        "useful",
        "usefulWrong",
    );
}

#[test]
fn java_array_structure_does_not_offer_java_length_to_kotlin_consumers() {
    let factory = "package app; public class Factory { public Item[][] matrix() { return null; } }";
    let source = "package app\nfun use(factory: Factory) { factory.matrix().size }\n";
    let mut ws = workspace(&[
        ("Item.kt", ITEM),
        ("Factory.java", factory),
        ("Use.kt", source),
    ]);
    let found = labels(&mut ws, "Use.kt", "factory.matrix().");
    assert!(
        !found.contains("length") && !found.contains("useful"),
        "Java surface leaked: {found:?}"
    );
    // Correct Kotlin Array<T>/size projection remains a useful-completion coverage gap.
}

const ARRAY_OBJECT_USE: &str = r#"package app;
class ArrayObjectUse {
    void run(Item[][] matrix, int[] numbers) {
        String rowText = matrix[0].toString();
        int rowTextLength = matrix[0].toString().length();
        String arrayText = matrix.toString();
        int arrayHash = matrix.hashCode();
        int primitiveHash = numbers.hashCode();
        Item[] rowCopy = matrix[0].clone();
    }
}
"#;

#[test]
fn java_array_rows_keep_public_object_completion_navigation_and_string_results() {
    // Minimal parsed JDK declarations isolate member routing. Native controls use the actual SDK.
    const OBJECT: &str = "package java.lang; public class Object { public String toString() { return null; } public native int hashCode(); protected void finalize() {} }";
    const STRING: &str =
        "package java.lang; public final class String { public int length() { return 0; } }";
    for (item_file, item_source) in [
        (
            "Item.java",
            "package app; public final class Item { public void useful() {} }",
        ),
        ("Item.kt", ITEM),
    ] {
        let mut ws = workspace(&[
            ("jdk/Object.java", OBJECT),
            ("jdk/String.java", STRING),
            (
                "app/Object.java",
                "package app; class Object { public void poisonedObject() {} }",
            ),
            (item_file, item_source),
            ("ArrayObjectUse.java", ARRAY_OBJECT_USE),
        ]);
        let expected = ktlsp::symbol::Def {
            file: "jdk/Object.java".into(),
            start_byte: OBJECT.find("toString").unwrap(),
            end_byte: OBJECT.find("toString").unwrap() + "toString".len(),
        };
        for prefix in ["matrix[0].", "matrix.", "numbers."] {
            let got = labels(&mut ws, "ArrayObjectUse.java", prefix);
            for name in ["length", "clone", "toString", "hashCode"] {
                assert!(
                    got.contains(name),
                    "{item_file} {prefix}: missing {name}: {got:?}"
                );
            }
            for name in ["useful", "poisonedObject", "finalize"] {
                assert!(
                    !got.contains(name),
                    "{item_file} {prefix}: leaked {name}: {got:?}"
                );
            }
        }
        assert_member(
            &mut ws,
            "ArrayObjectUse.java",
            "matrix[0].toString().",
            "length",
            "useful",
        );
        let use_at = ARRAY_OBJECT_USE.find("matrix[0].toString").unwrap() + "matrix[0].".len();
        assert_eq!(
            ws.goto_definition("ArrayObjectUse.java", use_at),
            vec![expected]
        );
    }
}

#[test]
fn java_array_object_members_require_the_selected_object_head_and_valid_call_shape() {
    const OBJECT: &str = "package java.lang; public class Object { public String toString() { return null; } protected void finalize() {} }";
    const STRING: &str =
        "package java.lang; public final class String { public int length() { return 0; } }";
    const USE: &str = "package app; class Use { void run(Item[][] matrix) { matrix[0].toString().length(); matrix[0].toString(1).length(); matrix[0].finalize(); } }";
    let mut ws = workspace(&[
        ("Item.kt", ITEM),
        ("Use.java", USE),
        ("jdk/String.java", STRING),
    ]);
    assert!(ws
        .goto_definition("Use.java", USE.find("toString").unwrap())
        .is_empty());
    assert!(!labels(&mut ws, "Use.java", "matrix[0].toString(1).").contains("length"));
    ws.open("jdk/Object.java", OBJECT.into());
    let at = OBJECT.find("toString").unwrap();
    assert_eq!(
        ws.goto_definition("Use.java", USE.find("toString").unwrap()),
        vec![ktlsp::symbol::Def {
            file: "jdk/Object.java".into(),
            start_byte: at,
            end_byte: at + "toString".len(),
        }]
    );
    assert!(labels(&mut ws, "Use.java", "matrix[0].toString().").contains("length"));
    assert!(!labels(&mut ws, "Use.java", "matrix[0].toString(1).").contains("length"));
    assert!(ws
        .goto_definition("Use.java", USE.find("finalize").unwrap())
        .is_empty());
    ws.open("duplicate/Object.java", OBJECT.into());
    assert!(ws
        .goto_definition("Use.java", USE.find("toString").unwrap())
        .is_empty());
    assert!(!labels(&mut ws, "Use.java", "matrix[0].toString().").contains("length"));
    for intrinsic in ["length", "clone"] {
        assert!(labels(&mut ws, "Use.java", "matrix[0].").contains(intrinsic));
    }
}

#[test]
fn java_primitive_array_elements_do_not_inherit_kotlin_keyword_homonym_members() {
    const POISON: &str = "package consumer\nopen class Ancestor { fun poisoned() {} }\nclass int : Ancestor() { val poisonedField: Int = 0 }\n";
    const COUNTS: &str =
        "package consumer; public class Counts { public int[] counts() { return new int[0]; } }";
    const USE: &str = r#"package consumer;
class Use<T> {
 void run(int[] values, int[][] matrix, Counts counts) {
  values[0].poisoned(); int direct = values[0].poisonedField;
  for (var fromArray : values) { fromArray.poisoned(); int field = fromArray.poisonedField; }
  counts.counts()[0].poisoned(); int count = counts.counts().length;
  for (var fromMethod : counts.counts()) { fromMethod.poisoned(); int field = fromMethod.poisonedField; }
  matrix[0].clone(); int width = matrix[0].length;
 }
}
"#;
    let mut ws = workspace(&[
        ("Poison.kt", POISON),
        ("Counts.java", COUNTS),
        (
            "Item.java",
            "package consumer; public class Item { public void useful() {} }",
        ),
        ("Use.java", USE),
        (
            "Use.kt",
            "package consumer\nfun use(value: int) { value.poisoned(); value.poisonedField }\n",
        ),
    ]);
    for prefix in [
        "values[0].",
        "fromArray.",
        "counts.counts()[0].",
        "fromMethod.",
    ] {
        let found = labels(&mut ws, "Use.java", prefix);
        assert!(
            !found.contains("poisoned") && !found.contains("poisonedField"),
            "{prefix}: {found:?}"
        );
        for name in ["poisoned", "poisonedField"] {
            let expression = format!("{prefix}{name}");
            if let Some(at) = USE.find(&expression) {
                assert!(
                    ws.goto_definition("Use.java", at + prefix.len()).is_empty(),
                    "{expression}"
                );
            }
        }
    }
    for prefix in ["matrix[0].", "counts.counts()."] {
        let found = labels(&mut ws, "Use.java", prefix);
        assert!(
            found.contains("length") && found.contains("clone"),
            "{prefix}: {found:?}"
        );
        assert!(!found.contains("poisoned"));
    }
    assert_member(&mut ws, "Use.kt", "value.", "poisoned", "useful");
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        "value.poisoned()",
        "Poison.kt",
        "poisoned",
    );
    assert!(labels(&mut ws, "Use.kt", "value.").contains("poisonedField"));
    let changed = COUNTS
        .replace("int[]", "Item[]")
        .replace("new int[0]", "new Item[0]");
    ws.change("Counts.java", changed);
    for prefix in ["counts.counts()[0].", "fromMethod."] {
        assert_member(&mut ws, "Use.java", prefix, "useful", "poisoned");
        assert!(!labels(&mut ws, "Use.java", prefix).contains("poisonedField"));
    }
    ws.change("Counts.java", COUNTS.into());
    for prefix in ["counts.counts()[0].", "fromMethod."] {
        let found = labels(&mut ws, "Use.java", prefix);
        assert!(
            !found.contains("useful")
                && !found.contains("poisoned")
                && !found.contains("poisonedField")
        );
    }
}

const KOTLIN_JAVA_ARRAY_READS: &str = r#"package app
fun read(factory: Factory, index: Int) {
    val size: Int = factory.matrix().size
    val row = factory.matrix()[index]
    val rowSize: Int = row.size
    factory.matrix()[index][0].useful()
    factory.matrix().get(index).get(0).useful()
    val count: Int = factory.numbers()[0]
    val booleanValue: Boolean = factory.booleans()[0]
    val byteValue: Byte = factory.bytes()[0]
    val charValue: Char = factory.chars()[0]
    val shortValue: Short = factory.shorts()[0]
    val longValue: Long = factory.longs()[0]
    val floatValue: Float = factory.floats()[0]
    val doubleValue: Double = factory.doubles()[0]
    val copy = factory.matrix().clone()
    val copySize: Int = copy.size
    copy[index][0].useful()
    factory.matrix().clone().get(index)[0].useful()
    val booleansClone: Boolean = factory.booleans().clone()[0]
    val bytesClone: Byte = factory.bytes().clone()[0]
    val charsClone: Char = factory.chars().clone()[0]
    val shortsClone: Short = factory.shorts().clone()[0]
    val numbersClone: Int = factory.numbers().clone()[0]
    val longsClone: Long = factory.longs().clone()[0]
    val floatsClone: Float = factory.floats().clone()[0]
    val doublesClone: Double = factory.doubles().clone()[0]
}
"#;

const KOTLIN_ARRAY_FACTORY: &str = r#"package app;
public class Factory {
    public Item[][] matrix() { return null; }
    public int[] numbers() { return null; }
    public boolean[] booleans() { return null; }
    public byte[] bytes() { return null; }
    public char[] chars() { return null; }
    public short[] shorts() { return null; }
    public long[] longs() { return null; }
    public float[] floats() { return null; }
    public double[] doubles() { return null; }
}
"#;

#[test]
fn kotlin_reads_java_array_size_and_each_index_dimension_without_java_members() {
    for (item_file, item_source) in [
        (
            "Item.java",
            "package app; public class Item { public void useful() {} }",
        ),
        ("Item.kt", ITEM),
    ] {
        let mut ws = workspace(&[
            (item_file, item_source),
            ("Other.kt", OTHER),
            ("Factory.java", KOTLIN_ARRAY_FACTORY),
            ("Use.kt", KOTLIN_JAVA_ARRAY_READS),
        ]);
        for prefix in ["factory.matrix().", "row."] {
            let found = labels(&mut ws, "Use.kt", prefix);
            assert!(
                found.contains("size") && found.contains("get") && found.contains("clone"),
                "{prefix}: {found:?}"
            );
            for absent in ["length", "wait", "getClass", "useful", "usefulWrong"] {
                assert!(
                    !found.contains(absent),
                    "{prefix} leaked {absent}: {found:?}"
                );
            }
        }
        for prefix in [
            "factory.matrix()[index][0].",
            "factory.matrix().get(index).get(0).",
            "copy[index][0].",
            "factory.matrix().clone().get(index)[0].",
        ] {
            assert_member(&mut ws, "Use.kt", prefix, "useful", "usefulWrong");
            assert_exact_definition(
                &mut ws,
                "Use.kt",
                &format!("{prefix}useful()"),
                item_file,
                "useful",
            );
        }
        let item = Type::class("Item", Some("app".into()));
        assert_eq!(
            infer_expression(&ws, "Use.kt", "factory.matrix()[index]"),
            Type::array(item.clone(), false)
        );
        assert_eq!(
            infer_expression(&ws, "Use.kt", "factory.matrix().get(index)"),
            Type::array(item, false)
        );
        for expr in [
            "factory.matrix().size",
            "row.size",
            "factory.numbers()[0]",
            "copy.size",
            "factory.numbers().clone()[0]",
        ] {
            assert_eq!(
                infer_expression(&ws, "Use.kt", expr),
                Type::class("Int", Some("kotlin".into())),
                "{expr}"
            );
        }
    }
}

#[test]
fn kotlin_java_array_reads_follow_unsaved_component_and_rank_edits() {
    let original = KOTLIN_JAVA_ARRAY_READS;
    let mut ws = workspace(&[
        ("Item.kt", ITEM),
        ("Other.kt", OTHER),
        ("Factory.java", KOTLIN_ARRAY_FACTORY),
        ("Use.kt", original),
    ]);
    let prefix = "factory.matrix()[index][0].";
    assert_member(&mut ws, "Use.kt", prefix, "useful", "usefulWrong");
    ws.change(
        "Factory.java",
        KOTLIN_ARRAY_FACTORY.replace("Item[][]", "Other[][]"),
    );
    assert_member(&mut ws, "Use.kt", prefix, "usefulWrong", "useful");
    ws.change("Use.kt", original.replace("useful()", "usefulWrong()"));
    assert_exact_definition(
        &mut ws,
        "Use.kt",
        &format!("{prefix}usefulWrong()"),
        "Other.kt",
        "usefulWrong",
    );
    ws.change(
        "Factory.java",
        KOTLIN_ARRAY_FACTORY.replace("Item[][]", "Item[]"),
    );
    let found = labels(&mut ws, "Use.kt", prefix);
    assert!(
        !found.contains("useful") && !found.contains("usefulWrong") && !found.contains("size"),
        "{found:?}"
    );
    ws.change("Factory.java", KOTLIN_ARRAY_FACTORY.into());
    ws.change("Use.kt", original.into());
    assert_member(&mut ws, "Use.kt", prefix, "useful", "usefulWrong");
}

#[test]
fn kotlin_java_array_reads_preserve_unknown_indices_and_lexical_nominal_receivers() {
    let src = r#"package app
class Array { fun nominalOnly() {} }
fun invalid(factory: Factory, unknown: Missing, nominal: Array, index: Other) {
    factory.matrix()[true].missing()
    factory.matrix()[0, 1].missing()
    factory.matrix()[unknown].missing()
    factory.matrix()[index].missing()
    factory.matrix().get().missing()
    factory.matrix().get("bad").missing()
    factory.matrix().get(0, 1).missing()
    factory.matrix().clone(0).missing()
    factory.matrix().clone { 0 }.missing()
    factory.matrix()[0][0][0].missing()
    nominal.nominalOnly()
}
"#;
    let mut ws = workspace(&[
        ("Item.kt", ITEM),
        ("Other.kt", OTHER),
        ("Factory.java", KOTLIN_ARRAY_FACTORY),
        ("Use.kt", src),
    ]);
    for prefix in [
        "factory.matrix()[true].",
        "factory.matrix()[0, 1].",
        "factory.matrix()[unknown].",
        "factory.matrix()[index].",
        "factory.matrix().get().",
        "factory.matrix().get(\"bad\").",
        "factory.matrix().get(0, 1).",
        "factory.matrix()[0][0][0].",
        "factory.matrix().clone(0).",
        "factory.matrix().clone { 0 }.",
    ] {
        let found = labels(&mut ws, "Use.kt", prefix);
        assert!(
            !found.contains("useful") && !found.contains("size") && !found.contains("get"),
            "{prefix}: {found:?}"
        );
    }
    let found = labels(&mut ws, "Use.kt", "nominal.");
    assert!(found.contains("nominalOnly"), "{found:?}");
    assert!(
        !found.contains("size") && !found.contains("get"),
        "{found:?}"
    );
}

#[test]
fn projected_mapped_collection_head_remains_possible_without_mapping_proof() {
    // These are index facts only; compiler control uses the actual JDK/stdlib declarations.
    let ws = workspace(&[
        (
            "JavaList.java",
            "package java.util; public interface List<E> {}",
        ),
        (
            "KotlinList.kt",
            "package kotlin.collections\ninterface List<out E>\n",
        ),
        (
            "MappedResult.java",
            "package mapped; public final class MappedResult {}",
        ),
        (
            "MappedConsumer.java",
            "package mapped; public final class MappedConsumer { public <T> MappedResult consume(java.util.List<? extends T> values) { return null; } public MappedResult consume(MappedResult value) { return value; } }",
        ),
    ]);
    let actual = Type::Class {
        name: "List".into(),
        package: Some("kotlin.collections".into()),
        container: None,
        nullable: false,
        args: vec![Type::class("String", Some("kotlin".into()))],

        owner: None,
    };
    let owner = Type::class("MappedConsumer", Some("mapped".into()));
    let ctx = FileCtx::new("mapped".into(), Vec::new());
    let arguments = ktcore::members::CallArguments {
        count: Some(1),
        types: vec![actual],
        names: None,
        has_spread: false,
        trailing_lambda: false,
        trailing_lambda_arguments: None,
    };
    let candidates = ktcore::members::candidates(&ws.index, &owner, "consume", &ctx);
    let selection = candidates.select(
        &ws.index,
        &owner,
        &ktcore::members::MemberUse::Call(arguments),
        &ctx,
    );
    assert_eq!(
        selection.applicability,
        ktcore::members::Applicability::Unknown,
        "an unmodeled mapped identity cannot prove rejection"
    );
    assert_eq!(
        selection.entries.len(),
        1,
        "the java.util.List candidate remains possible"
    );
    assert_eq!(selection.entries[0].sym.params[0].name, "List");
}

#[test]
fn holder_key_overload_kotlin_actual_nested_returns_preserve_exact_target() {
    let mut ws = holder_key_overload_workspace();
    ws.open("Holder.java", "package holder; public interface Holder<T> { public static class Reference<T> implements Holder<T> { public void resourceOnly() {} } }".into());
    ws.open("HolderSet.java", "package holder; public interface HolderSet<T> { public static class Named<T> implements HolderSet<T> { public void tagOnly() {} } }".into());
    ws.change("HolderGetter.java", "package holder; import keys.ResourceKey; import tags.TagKey; public interface HolderGetter<T> { Holder.Reference<T> getOrThrow(ResourceKey<T> key); HolderSet.Named<T> getOrThrow(TagKey<T> key); }".into());
    ws.change("HolderLookup.java", "package holder; public interface HolderLookup<T> extends HolderOwner<T>, HolderGetter<T> {}".into());
    let text = "package consumer\nimport holder.holderLookup\nimport registries.Registries\nimport enchantments.Enchantments\nfun use() { holderLookup(Registries.ENCHANTMENT).getOrThrow(Enchantments.EFFICIENCY).resourceOnly() }\n";
    ws.open("NestedHolderUse.kt", text.into());
    let argument = infer_expression(&ws, "NestedHolderUse.kt", "Enchantments.EFFICIENCY");
    let receiver = infer_expression(
        &ws,
        "NestedHolderUse.kt",
        "holderLookup(Registries.ENCHANTMENT)",
    );
    eprintln!("actual nested holder reduction: argument={argument:?}, receiver={receiver:?}");
    assert_eq!(argument.name(), Some("ResourceKey"), "{argument:?}");
    assert_eq!(receiver.name(), Some("HolderLookup"), "{receiver:?}");
    assert_holder_overload(&mut ws, "NestedHolderUse.kt", "ResourceKey");
    assert_member(
        &mut ws,
        "NestedHolderUse.kt",
        "getOrThrow(Enchantments.EFFICIENCY).resourceOnly",
        "resourceOnly",
        "tagOnly",
    );
}

#[test]
fn holder_key_overload_kotlin_implicit_block_entity_extension_over_java_level_method() {
    let mut source = holder_key_overload_workspace();
    // Kotlin 2.1.20 commonMain/kotlin/util/Preconditions.kt API signature.
    // Copyright JetBrains s.r.o. and Kotlin contributors; Apache-2.0.
    // The small executable body supplies the same nullable -> nonnull type API.
    source.open(
        "Preconditions.kt",
        "package kotlin\npublic inline fun <T : Any> checkNotNull(value: T?): T = value!!\n".into(),
    );
    source.open("Holder.java", "package holder; public interface Holder<T> { public static class Reference<T> implements Holder<T> { public void resourceOnly() {} } }".into());
    source.open("HolderSet.java", "package holder; public interface HolderSet<T> { public static class Named<T> implements HolderSet<T> { public void tagOnly() {} } }".into());
    source.change("HolderGetter.java", "package holder; import keys.ResourceKey; import tags.TagKey; public interface HolderGetter<T> { Holder.Reference<T> getOrThrow(ResourceKey<T> key); HolderSet.Named<T> getOrThrow(TagKey<T> key); }".into());
    source.change("HolderLookup.java", "package holder; public interface HolderLookup<T> extends HolderOwner<T>, HolderGetter<T> {}".into());
    for (file, text) in [
        ("Nullable.java", "package javax.annotation; public @interface Nullable {}"),
        ("LevelReader.java", "package world; import holder.HolderLookup; import holder.Registry; import keys.ResourceKey; public interface LevelReader { default <T> HolderLookup<T> holderLookup(ResourceKey<? extends Registry<? extends T>> registryKey) { return null; } }"),
        ("CommonLevelAccessor.java", "package world; public interface CommonLevelAccessor extends LevelReader {}"),
        ("LevelAccessor.java", "package world; public interface LevelAccessor extends CommonLevelAccessor {}"),
        ("Level.java", "package world; public abstract class Level implements LevelAccessor {}"),
        ("BlockEntity.java", "package world; import javax.annotation.Nullable; public abstract class BlockEntity { @Nullable protected Level level; @Nullable public Level getLevel() { return level; } }"),
        ("DrillBlockEntity.java", "package world; public abstract class DrillBlockEntity extends BlockEntity { public float getBreakSpeed() { return 0; } }"),
    ] {
        source.open(file, text.into());
    }
    source.change("Lookup.kt", "package holder\nimport keys.ResourceKey\nimport world.BlockEntity\nfun <T> BlockEntity.holderLookup(registryKey: ResourceKey<out Registry<out T>>): HolderLookup<T> = checkNotNull(level).holderLookup(registryKey)\n".into());
    let text = "package consumer\nimport holder.holderLookup\nimport world.DrillBlockEntity\nimport registries.Registries\nimport enchantments.Enchantments\nclass EnchantableDrillBlockEntity : DrillBlockEntity() {\n override fun getBreakSpeed(): Float {\n  val efficiency = holderLookup(Registries.ENCHANTMENT).getOrThrow(Enchantments.EFFICIENCY)\n  efficiency.resourceOnly()\n  return 0f\n }\n}\n";
    let root = std::env::temp_dir().join(format!(
        "ktlsp-holder-implicit-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut ws = Workspace::new();
    assert_eq!(ws.scan(&root), 0);
    ws.set_library_index_complete(true);
    ws.set_jdk_index_complete(true);
    for file in source.open_doc_keys() {
        let key = ktlsp::paths::file_key(&root.join(&file));
        ws.open(&key, source.doc_text(&file).unwrap());
    }
    let file = ktlsp::paths::file_key(&root.join("ImplicitHolderUse.kt"));
    let lookup_file = ktlsp::paths::file_key(&root.join("Lookup.kt"));
    let api_file = ktlsp::paths::file_key(&root.join("HolderGetter.java"));
    ws.open(&file, text.into());
    let argument = infer_expression(&ws, &file, "Enchantments.EFFICIENCY");
    let receiver = infer_expression(&ws, &file, "holderLookup(Registries.ENCHANTMENT)");
    eprintln!("actual implicit BlockEntity holder reduction: argument={argument:?}, receiver={receiver:?}");
    assert_eq!(argument.name(), Some("ResourceKey"), "{argument:?}");
    assert_eq!(receiver.name(), Some("HolderLookup"), "{receiver:?}");
    assert_holder_overload_at(&mut ws, &file, &api_file, "ResourceKey");
    assert_member(
        &mut ws,
        &file,
        "efficiency.resourceOnly",
        "resourceOnly",
        "tagOnly",
    );
    let delegated = infer_expression(
        &ws,
        &lookup_file,
        "checkNotNull(level).holderLookup(registryKey)",
    );
    assert_eq!(
        delegated.name(),
        Some("HolderLookup"),
        "the real Java generic delegate remains a typed lookup: {delegated:?}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

fn holder_key_overload_workspace() -> Workspace {
    workspace(&[
        ("Comparable.java", "package java.lang; public interface Comparable<T> { int compareTo(T other); }"),
        ("ResourceKey.java", "package keys; public class ResourceKey<T> implements java.lang.Comparable<ResourceKey<?>> { public int compareTo(ResourceKey<?> other) { return 0; } }"),
        ("TagKey.java", "package tags; import keys.ResourceKey; import holder.Registry; public record TagKey<T>(ResourceKey<? extends Registry<T>> registry) {}"),
        ("Registry.java", "package holder; public class Registry<T> {}"),
        ("Enchantment.java", "package enchantments; public class Enchantment {}"),
        ("Reference.java", "package holder; public class Reference<T> { public void resourceOnly() {} }"),
        ("NamedTag.java", "package holder; public class NamedTag<T> { public void tagOnly() {} }"),
        ("HolderGetter.java", "package holder; import keys.ResourceKey; import tags.TagKey; public interface HolderGetter<T> { Reference<T> getOrThrow(ResourceKey<T> key); NamedTag<T> getOrThrow(TagKey<T> key); }"),
        ("HolderOwner.java", "package holder; public interface HolderOwner<T> {}"),
        ("HolderLookup.java", "package holder; public interface HolderLookup<T> extends HolderGetter<T>, HolderOwner<T> {}"),
        ("Enchantments.java", "package enchantments; import keys.ResourceKey; import tags.TagKey; public class Enchantments { public static final ResourceKey<Enchantment> EFFICIENCY = null; public static final TagKey<Enchantment> EFFICIENCY_TAG = null; }"),
        ("Registries.java", "package registries; import keys.ResourceKey; import holder.Registry; import enchantments.Enchantment; public class Registries { public static final ResourceKey<Registry<Enchantment>> ENCHANTMENT = null; }"),
        ("Lookup.kt", "package holder\nimport keys.ResourceKey\nfun <T> holderLookup(key: ResourceKey<out Registry<out T>>): HolderLookup<T> = error(\"fixture\")\n"),
    ])
}

fn assert_holder_overload(ws: &mut Workspace, file: &str, parameter: &str) {
    assert_holder_overload_at(ws, file, "HolderGetter.java", parameter);
}

fn assert_holder_overload_at(ws: &mut Workspace, file: &str, api_file: &str, parameter: &str) {
    let source = ws.doc_text(file).unwrap();
    let offset = source.find("getOrThrow(").unwrap();
    let defs = ws.goto_definition(file, offset);
    let api = ws.doc_text(api_file).unwrap();
    let signature = format!("getOrThrow({parameter}<T>");
    let expected = api.find(&signature).unwrap();
    assert_eq!(
        defs.len(),
        1,
        "the typed argument must select only {parameter}, not every same-arity overload: {defs:?}"
    );
    assert_eq!(defs[0].file, api_file);
    assert_eq!(defs[0].start_byte, expected);
    assert_eq!(&api[defs[0].start_byte..defs[0].end_byte], "getOrThrow");
}

#[test]
fn holder_key_overload_kotlin_chained_static_argument_selects_exact_resource_key() {
    let mut ws = holder_key_overload_workspace();
    let text = "package consumer\nimport holder.holderLookup\nimport registries.Registries\nimport enchantments.Enchantments\nfun use() { holderLookup(Registries.ENCHANTMENT).getOrThrow(Enchantments.EFFICIENCY).resourceOnly() }\n";
    ws.open("HolderUse.kt", text.into());
    let argument = infer_expression(&ws, "HolderUse.kt", "Enchantments.EFFICIENCY");
    let receiver = infer_expression(&ws, "HolderUse.kt", "holderLookup(Registries.ENCHANTMENT)");
    eprintln!("actual holder reduction: argument={argument:?}, receiver={receiver:?}");
    assert_eq!(
        argument.name(),
        Some("ResourceKey"),
        "preserve the real static-field argument's declared type: {argument:?}"
    );
    assert_eq!(
        receiver.name(),
        Some("HolderLookup"),
        "preserve the chained generic lookup receiver: {receiver:?}"
    );
    assert_holder_overload(&mut ws, "HolderUse.kt", "ResourceKey");
    assert_member(
        &mut ws,
        "HolderUse.kt",
        "getOrThrow(Enchantments.EFFICIENCY).resourceOnly",
        "resourceOnly",
        "tagOnly",
    );
}

fn holder_key_overload_absolute_java_workspace() -> (std::path::PathBuf, Workspace, String, String)
{
    let source = holder_key_overload_workspace();
    let root = std::env::temp_dir().join(format!(
        "ktlsp-holder-qualified-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut ws = Workspace::new();
    assert_eq!(
        ws.scan(&root),
        0,
        "start from an actually scanned empty standalone project"
    );
    ws.set_library_index_complete(true);
    ws.set_jdk_index_complete(true);
    for file in [
        "Comparable.java",
        "ResourceKey.java",
        "TagKey.java",
        "Registry.java",
        "Enchantment.java",
        "Reference.java",
        "NamedTag.java",
        "HolderGetter.java",
        "HolderOwner.java",
        "HolderLookup.java",
        "Enchantments.java",
        "Registries.java",
        "Lookup.kt",
    ] {
        let key = ktlsp::paths::file_key(&root.join(file));
        ws.open(&key, source.doc_text(file).unwrap().to_string());
    }
    let file = ktlsp::paths::file_key(&root.join("HolderUse.java"));
    let api_file = ktlsp::paths::file_key(&root.join("HolderGetter.java"));
    ws.open(&file, "package consumer; import holder.HolderLookup; import enchantments.Enchantment; import enchantments.Enchantments; class HolderUse { void use(HolderLookup<Enchantment> lookup) { lookup.getOrThrow(Enchantments.EFFICIENCY).resourceOnly(); } }".into());
    (root, ws, file, api_file)
}

#[test]
fn holder_key_overload_java_static_argument_selects_exact_resource_key() {
    let (root, mut ws, file, api_file) = holder_key_overload_absolute_java_workspace();
    assert_holder_overload_at(&mut ws, &file, &api_file, "ResourceKey");
    assert_member(
        &mut ws,
        &file,
        "getOrThrow(Enchantments.EFFICIENCY).resourceOnly",
        "resourceOnly",
        "tagOnly",
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn holder_key_overload_unknown_argument_retains_both_possible_declarations() {
    let mut ws = holder_key_overload_workspace();
    for (file, text) in [
        ("Unknown.kt", "package consumer\nimport holder.HolderLookup\nimport enchantments.Enchantment\nfun use(lookup: HolderLookup<Enchantment>) { lookup.getOrThrow(missing) }\n"),
        ("Unknown.java", "package consumer; import holder.HolderLookup; import enchantments.Enchantment; class Unknown { void use(HolderLookup<Enchantment> lookup) { lookup.getOrThrow(missing); } }"),
    ] {
        ws.open(file, text.into());
        let defs = ws.goto_definition(file, text.find("getOrThrow(").unwrap());
        let api = ws.doc_text("HolderGetter.java").unwrap();
        let mut starts: Vec<_> = defs.iter().map(|d| { assert_eq!(d.file, "HolderGetter.java"); d.start_byte }).collect();
        starts.sort();
        assert_eq!(starts, vec![api.find("getOrThrow(ResourceKey<T>").unwrap(), api.find("getOrThrow(TagKey<T>").unwrap()], "an unknown argument cannot choose arbitrarily: {defs:?}");
    }
}

#[test]
fn holder_key_overload_unsaved_argument_flip_replaces_navigation_and_result_chain() {
    let mut ws = holder_key_overload_workspace();
    let original = "package consumer\nimport holder.HolderLookup\nimport enchantments.Enchantment\nimport enchantments.Enchantments\nfun use(lookup: HolderLookup<Enchantment>) { lookup.getOrThrow(Enchantments.EFFICIENCY).resourceOnly() }\n";
    ws.open("Flip.kt", original.into());
    assert_holder_overload(&mut ws, "Flip.kt", "ResourceKey");
    let edited = original.replace("EFFICIENCY).resourceOnly", "EFFICIENCY_TAG).tagOnly");
    ws.change("Flip.kt", edited);
    assert_holder_overload(&mut ws, "Flip.kt", "TagKey");
    assert_member(
        &mut ws,
        "Flip.kt",
        "getOrThrow(Enchantments.EFFICIENCY_TAG).tagOnly",
        "tagOnly",
        "resourceOnly",
    );
    ws.change("Flip.kt", original.into());
    assert_holder_overload(&mut ws, "Flip.kt", "ResourceKey");
}

#[test]
fn holder_key_overload_kotlin_getter_property_is_not_a_java_inherited_field() {
    // K2.4 + javac controls prove these properties have private backing fields/getters.
    // They do not shadow this Java type import as a public inherited JVM field would.
    for visibility in ["public", "protected"] {
        for pending in [false, true] {
            for explicit_type in [false, true] {
                let root = std::env::temp_dir().join(format!(
                    "ktlsp-kotlin-getter-not-field-{}-{}-{visibility}-{pending}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
                std::fs::create_dir_all(&root).unwrap();
                let mut ws = Workspace::new();
                assert_eq!(ws.scan(&root), 0);
                ws.set_library_index_complete(true);
                ws.set_jdk_index_complete(true);
                let imported = ktlsp::paths::file_key(&root.join("names/Enchantments.java"));
                let shadow = ktlsp::paths::file_key(&root.join("values/ShadowFields.java"));
                let parent = ktlsp::paths::file_key(&root.join("consumer/Parent.kt"));
                let file = ktlsp::paths::file_key(&root.join("consumer/Use.java"));
                ws.open(
                    &imported,
                    "package names; public class Enchantments { public static int TYPE_ONLY; }"
                        .into(),
                );
                ws.open(
                    &shadow,
                    "package values; public class ShadowFields { public int VALUE_ONLY; }".into(),
                );
                let type_annotation = if explicit_type {
                    ": values.ShadowFields"
                } else {
                    ""
                };
                ws.open(&parent, format!("package consumer\nopen class Parent {{ private val state = 1; {visibility} val Enchantments{type_annotation} = values.ShadowFields(); fun data(): Int {{ val data = state; return data }}; init {{ val by = 1 }} }}\n"));
                let text = "package consumer; import names.Enchantments; public class Use extends Parent { public int read() { return Enchantments.TYPE_ONLY; } }";
                ws.open(&file, text.into());
                {
                    let declarations =
                        ktlsp::index::InferenceIndex::lookup_by_name(&ws.index, "Enchantments");
                    let property = declarations
                        .into_iter()
                        .find(|entry| {
                            entry.path.as_ref() == parent.as_str()
                                && entry.sym.kind == ktcore::symbol::SymbolKind::Property
                        })
                        .unwrap();
                    assert_eq!(
                        property.sym.source_member.dispatch(),
                        Some(ktcore::extensions::Dispatch::Instance),
                        "qualify that Kotlin property dispatch alone cannot certify a Java field"
                    );
                    assert_eq!(
                        property.sym.source_member.visibility(),
                        Some(if visibility == "public" {
                            ktcore::extensions::Visibility::Public
                        } else {
                            ktcore::extensions::Visibility::Protected
                        })
                    );
                }
                // The compiler proves that a getter-only property does not hide the
                // explicit Java type import. Require its exact field target while the
                // real source generation is ready, before withdrawing producer proof.
                let definitions = ws.goto_definition(&file, text.find("TYPE_ONLY").unwrap());
                let imported_text = ws.doc_text(&imported).unwrap();
                assert_eq!(definitions.len(), 1, "getter-only property must preserve useful Java type navigation: {visibility}, pending={pending}, {definitions:?}");
                assert_eq!(definitions[0].file, imported);
                assert_eq!(
                    definitions[0].start_byte,
                    imported_text.find("TYPE_ONLY").unwrap()
                );
                assert_eq!(
                    definitions[0].end_byte,
                    imported_text.find("TYPE_ONLY").unwrap() + "TYPE_ONLY".len()
                );
                if pending {
                    ws.begin_analysis_generation();
                }
                assert_member(&mut ws, &file, "Enchantments.", "TYPE_ONLY", "VALUE_ONLY");
                let invalid_field = text.replace("TYPE_ONLY", "VALUE_ONLY");
                ws.change(&file, invalid_field.clone());
                let definitions =
                    ws.goto_definition(&file, invalid_field.find("VALUE_ONLY").unwrap());
                assert!(definitions.is_empty(), "a getter-backed Kotlin property cannot become Java field syntax: {visibility}, pending={pending}, {definitions:?}");
                std::fs::remove_dir_all(root).unwrap();
            }
        }
    }
}

#[test]
fn holder_key_overload_kotlin_jvm_field_does_not_fall_back_to_imported_type() {
    // Independent K2.4+javac controls prove this annotation emits a real public
    // field. Even without a source JVM-field projection, it cannot expose the
    // hidden TYPE_ONLY member from the explicit Java type import.
    for pending in [false, true] {
        let root = std::env::temp_dir().join(format!(
            "ktlsp-kotlin-jvm-field-shadows-import-{}-{}-{pending}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut ws = Workspace::new();
        assert_eq!(ws.scan(&root), 0);
        ws.set_library_index_complete(true);
        ws.set_jdk_index_complete(true);
        let imported = ktlsp::paths::file_key(&root.join("names/Enchantments.java"));
        let shadow = ktlsp::paths::file_key(&root.join("values/ShadowFields.java"));
        let parent = ktlsp::paths::file_key(&root.join("consumer/Parent.kt"));
        let file = ktlsp::paths::file_key(&root.join("consumer/Use.java"));
        ws.open(
            &imported,
            "package names; public class Enchantments { public static int TYPE_ONLY; }".into(),
        );
        ws.open(
            &shadow,
            "package values; public class ShadowFields { public int VALUE_ONLY; }".into(),
        );
        ws.open(&parent, "package consumer\nopen class Parent { @kotlin.jvm.JvmField public val Enchantments: values.ShadowFields = values.ShadowFields() }\n".into());
        let text = "package consumer; import names.Enchantments; public class Use extends Parent { public int read() { return Enchantments.TYPE_ONLY; } }";
        ws.open(&file, text.into());
        let property = ktlsp::index::InferenceIndex::lookup_by_name(&ws.index, "Enchantments")
            .into_iter()
            .find(|entry| {
                entry.path.as_ref() == parent.as_str()
                    && entry.sym.kind == ktcore::symbol::SymbolKind::Property
            })
            .unwrap();
        assert_eq!(
            property.sym.source_member.dispatch(),
            Some(ktcore::extensions::Dispatch::Instance)
        );
        assert_eq!(
            property.sym.source_member.visibility(),
            Some(ktcore::extensions::Visibility::Public)
        );
        if pending {
            ws.begin_analysis_generation();
        }
        let items = labels(&mut ws, &file, "Enchantments.");
        assert!(!items.contains("TYPE_ONLY"),
            "a real Kotlin JVM field hides the imported Java type; unsupported field projection must not invent TYPE_ONLY: pending={pending}, {items:?}");
        let definitions = ws.goto_definition(&file, text.find("TYPE_ONLY").unwrap());
        assert!(definitions.is_empty(), "invalid Java member cannot navigate to the hidden imported type: pending={pending}, {definitions:?}");
        let valid = text.replace("TYPE_ONLY", "VALUE_ONLY");
        ws.change(&file, valid.clone());
        assert_member(&mut ws, &file, "Enchantments.", "VALUE_ONLY", "TYPE_ONLY");
        let definitions = ws.goto_definition(&file, valid.find("VALUE_ONLY").unwrap());
        let shadow_text = ws.doc_text(&shadow).unwrap();
        assert_eq!(definitions.len(), 1, "a normalized JVM field must give useful member navigation: pending={pending}, {definitions:?}");
        assert_eq!(definitions[0].file, shadow);
        assert_eq!(
            definitions[0].start_byte,
            shadow_text.find("VALUE_ONLY").unwrap()
        );
        assert_eq!(
            definitions[0].end_byte,
            shadow_text.find("VALUE_ONLY").unwrap() + "VALUE_ONLY".len()
        );
        // An unsaved lowering flip replaces the accepted per-file facts immediately.
        ws.change(&parent, "package consumer\nopen class Parent { public val Enchantments: values.ShadowFields = values.ShadowFields() }\n".into());
        ws.change(&file, text.into());
        assert_member(&mut ws, &file, "Enchantments.", "TYPE_ONLY", "VALUE_ONLY");
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn holder_key_overload_inherited_value_shadow_wins_over_imported_type_completion() {
    // javac --release 17 independently proves that an inherited value wins over this
    // explicit type import. Exercise the same unsaved edit both ready and during refresh.
    for pending in [false, true] {
        let root = std::env::temp_dir().join(format!(
            "ktlsp-inherited-import-shadow-{}-{}-{pending}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut ws = Workspace::new();
        assert_eq!(ws.scan(&root), 0);
        ws.set_library_index_complete(true);
        ws.set_jdk_index_complete(true);
        let imported = ktlsp::paths::file_key(&root.join("names/Enchantments.java"));
        let shadow = ktlsp::paths::file_key(&root.join("values/ShadowFields.java"));
        let parent = ktlsp::paths::file_key(&root.join("consumer/Parent.java"));
        let file = ktlsp::paths::file_key(&root.join("consumer/Use.java"));
        let shadow_text = "package values; public class ShadowFields { public int VALUE_ONLY; }";
        ws.open(
            &imported,
            "package names; public class Enchantments { public static int TYPE_ONLY; }".into(),
        );
        ws.open(&shadow, shadow_text.into());
        ws.open(&parent, "package consumer; public class Parent {}".into());
        let before = "package consumer; import names.Enchantments; public class Use extends Parent { public int read() { return Enchantments.TYPE_ONLY; } }";
        ws.open(&file, before.into());
        assert!(!ktlsp::index::InferenceIndex::may_have_compiled_callables(
            &ws.index
        ));
        assert_member(&mut ws, &file, "Enchantments.", "TYPE_ONLY", "VALUE_ONLY");
        if pending {
            ws.begin_analysis_generation();
        }
        ws.change(
            &parent,
            "package consumer; public class Parent { protected values.ShadowFields Enchantments; }"
                .into(),
        );
        let after = before.replace("TYPE_ONLY", "VALUE_ONLY");
        ws.change(&file, after.clone());
        assert_member(&mut ws, &file, "Enchantments.", "VALUE_ONLY", "TYPE_ONLY");
        let definitions = ws.goto_definition(&file, after.find("VALUE_ONLY").unwrap());
        assert_eq!(
            definitions.len(),
            1,
            "the value member must have one exact target: {definitions:?}"
        );
        assert_eq!(definitions[0].file, shadow);
        assert_eq!(
            definitions[0].start_byte,
            shadow_text.find("VALUE_ONLY").unwrap()
        );
        assert_eq!(
            &shadow_text[definitions[0].start_byte..definitions[0].end_byte],
            "VALUE_ONLY"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn holder_key_overload_incomplete_java_namespace_retains_possible_overloads() {
    let (root, mut ws, file, api_file) = holder_key_overload_absolute_java_workspace();
    assert_holder_overload_at(&mut ws, &file, &api_file, "ResourceKey");
    let previous = ws.analysis_generation();
    let generation = ws.begin_analysis_generation();
    assert!(!ws.is_current_generation(previous));
    assert!(ws.is_current_generation(generation));
    let text = ws.doc_text(&file).unwrap().to_string();
    let argument_offset = text.find("Enchantments.EFFICIENCY").unwrap() + "Enchantments.".len();
    let argument = ws
        .resolved_symbol_query(&file, argument_offset)
        .expect("query the exact argument field while its real refresh is pending");
    assert_eq!(
        argument.reference().status_label(),
        "unknown",
        "qualify unavailable argument-field authority before conservative overload selection: {:?}",
        argument.targets
    );
    assert!(
        argument.targets.is_empty(),
        "the pending argument must not claim a field target: {:?}",
        argument.targets
    );
    assert!(
        ws.goto_definition(&file, argument_offset).is_empty(),
        "strict field navigation must agree with the unknown argument proof"
    );
    // A selected ordinary field cannot be assumed before generated/imported name shadows close.
    let defs = ws.goto_definition(&file, text.find("getOrThrow(").unwrap());
    let api = ws.doc_text(&api_file).unwrap();
    let mut starts: Vec<_> = defs
        .iter()
        .map(|d| {
            assert_eq!(d.file, api_file);
            d.start_byte
        })
        .collect();
    starts.sort();
    assert_eq!(
        starts,
        vec![
            api.find("getOrThrow(ResourceKey<T>").unwrap(),
            api.find("getOrThrow(TagKey<T>").unwrap()
        ],
        "incomplete namespace must not invent a typed static argument: {defs:?}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

fn java_getter_projection_workspace() -> Workspace {
    workspace(&[
        ("Payload.java", "package bean; public class Payload { public void touch() {} }"),
        ("Other.java", "package bean; public class Other { public void otherOnly() {} }"),
        ("PlainBean.java", "package bean; public class PlainBean { protected Payload payload; public Payload getPayload() { return payload; } }"),
    ])
}

#[test]
fn java_getter_property_explicit_and_implicit_use_original_getter_definition() {
    let mut ws = java_getter_projection_workspace();
    let text = "package consumer\nimport bean.PlainBean\nfun PlainBean.read() = payload\nfun explicit(bean: PlainBean) = bean.payload.touch()\n";
    ws.open("GetterUse.kt", text.into());
    ws.open("JavaGetterUse.java", "package consumer; import bean.PlainBean; class JavaGetterUse { void use(PlainBean bean) { bean.getPayload().touch(); } }".into());
    for marker in ["payload", "bean.payload"] {
        let ty = infer_expression(&ws, "GetterUse.kt", marker);
        assert_eq!(
            ty.name(),
            Some("Payload"),
            "ordinary getter property must type {marker}: {ty:?}"
        );
        assert_eq!(ty.package(), Some("bean"));
    }
    let api = ws.doc_text("PlainBean.java").unwrap();
    let target = api.find("getPayload").unwrap();
    for offset in [
        text.find("payload").unwrap(),
        text.find("bean.payload").unwrap() + "bean.".len(),
    ] {
        let defs = ws.goto_definition("GetterUse.kt", offset);
        assert_eq!(
            defs.len(),
            1,
            "the property must navigate to the real getter: {defs:?}"
        );
        assert_eq!(
            (defs[0].file.as_str(), defs[0].start_byte, defs[0].end_byte),
            ("PlainBean.java", target, target + "getPayload".len())
        );
    }
    let result = ws
        .complete(
            "GetterUse.kt",
            text.find("bean.payload").unwrap() + "bean.pa".len(),
            true,
        )
        .unwrap();
    assert!(
        result.items.iter().any(
            |item| item.label == "payload" && item.kind == ktcore::symbol::SymbolKind::Property
        ),
        "getter property completion must be useful: {result:?}"
    );
    assert_member(
        &mut ws,
        "GetterUse.kt",
        "bean.payload.touch",
        "touch",
        "otherOnly",
    );
    let java = ws.doc_text("JavaGetterUse.java").unwrap();
    let mut refs: Vec<_> = ws
        .references("PlainBean.java", target, true)
        .into_iter()
        .map(|site| (site.file, site.start_byte, site.end_byte))
        .collect();
    refs.sort();
    let mut expected = vec![
        (
            "PlainBean.java".to_string(),
            target,
            target + "getPayload".len(),
        ),
        (
            "JavaGetterUse.java".to_string(),
            java.find("getPayload").unwrap(),
            java.find("getPayload").unwrap() + "getPayload".len(),
        ),
    ];
    for (start, _) in text.match_indices("payload") {
        expected.push(("GetterUse.kt".into(), start, start + "payload".len()));
    }
    expected.sort();
    assert_eq!(
        refs, expected,
        "both spellings must resolve through the same getter identity"
    );
    // Mixed method/property spellings require distinct edits; this slice refuses partial rename.
    assert!(ws
        .prepare_rename("GetterUse.kt", text.find("payload").unwrap())
        .is_none());
    assert!(ws.prepare_rename("PlainBean.java", target).is_none());
    assert!(ws
        .rename("GetterUse.kt", text.find("payload").unwrap(), "other")
        .is_none());
    assert!(ws.rename("PlainBean.java", target, "getOther").is_none());
}

#[test]
fn java_getter_property_projection_preserves_language_dispatch_and_shape_boundaries() {
    let mut ws = java_getter_projection_workspace();
    ws.open("RejectedBean.java", "package bean; public class RejectedBean { public static Payload getStatic() { return null; } private Payload getSecret() { return null; } public Payload getArgument(int value) { return null; } public <T> T getGeneric() { return null; } public void getNothing() {} public Payload getURL() { return null; } public boolean isReady() { return true; } }".into());
    ws.open(
        "KotlinBean.kt",
        "package bean\nclass KotlinBean { fun getPayload(): Payload = Payload() }\n".into(),
    );
    for (owner, written) in [
        ("RejectedBean", "static"),
        ("RejectedBean", "secret"),
        ("RejectedBean", "argument"),
        ("RejectedBean", "generic"),
        ("RejectedBean", "nothing"),
        ("RejectedBean", "URL"),
        ("RejectedBean", "isReady"),
        ("KotlinBean", "payload"),
        ("PlainBean", "payload()"),
    ] {
        let text = format!(
            "package consumer\nimport bean.{owner}\nfun probe(bean: {owner}) = bean.{written}\n"
        );
        ws.open("GetterNegative.kt", text.clone());
        let offset = text.rfind("bean.").unwrap() + "bean.".len();
        assert!(text[offset..].starts_with(written));
        let defs = ws.goto_definition("GetterNegative.kt", offset);
        assert!(
            defs.is_empty(),
            "unsupported or invalid property must stay unknown: {owner}.{written}: {defs:?}"
        );
    }
    let java = "package consumer; import bean.PlainBean; class JavaFieldUse { void use(PlainBean bean) { bean.payload.touch(); } }";
    ws.open("JavaFieldUse.java", java.into());
    assert!(
        ws.goto_definition("JavaFieldUse.java", java.find("payload").unwrap())
            .is_empty(),
        "Kotlin's synthetic getter property must not expose a protected Java field"
    );
    // A local value remains stronger than the extension receiver's property.
    let local = "package consumer\nimport bean.PlainBean\nimport bean.Other\nfun PlainBean.local() { val payload: Other = Other(); payload.otherOnly() }\n";
    ws.open("GetterLocal.kt", local.into());
    assert_member(
        &mut ws,
        "GetterLocal.kt",
        "payload.otherOnly",
        "otherOnly",
        "touch",
    );
}

#[test]
fn java_getter_property_return_and_target_update_after_unsaved_java_edit() {
    let mut ws = java_getter_projection_workspace();
    let text = "package consumer\nimport bean.PlainBean\nfun use(bean: PlainBean) = bean.payload.touch()\n";
    ws.open("GetterChange.kt", text.into());
    assert_eq!(
        infer_expression(&ws, "GetterChange.kt", "bean.payload").name(),
        Some("Payload")
    );
    ws.change("PlainBean.java", "package bean; public class PlainBean { protected Other payload; public Other getPayload() { return payload; } }".into());
    assert_eq!(
        infer_expression(&ws, "GetterChange.kt", "bean.payload").name(),
        Some("Other")
    );
    assert_member(
        &mut ws,
        "GetterChange.kt",
        "bean.payload.",
        "otherOnly",
        "touch",
    );
    let api = ws.doc_text("PlainBean.java").unwrap();
    let defs = ws.goto_definition("GetterChange.kt", text.find("payload").unwrap());
    assert_eq!(defs.len(), 1, "{defs:?}");
    assert_eq!(defs[0].file, "PlainBean.java");
    assert_eq!(defs[0].start_byte, api.find("getPayload").unwrap());
}

#[test]
fn projected_passthrough_preserves_scoped_parameter_identity_and_nullability() {
    let source = r#"package probe
fun <S> invariant(request: Request, key: Class<Class<S>>) = request.nested(key)
fun <S> projected(request: Request, key: Class<out Class<S>>) = request.nested(key)
fun <S> nestedProjected(pass: Passage, key: Class<out Class<out S>>) = pass.pass(key)
fun concrete(pass: Passage, key: Class<out Class<out Invocation>>) = pass.pass(key)
"#;
    let mut ws = projected_workspace(source);
    ws.open("Passage.java", "package probe; public class Passage { public <T> T pass(Class<? extends Class<? extends T>> key) { return null; } }".into());
    for (expression, parameter) in [
        ("request.nested(key)", "fun <S> invariant"),
        ("pass.pass(key)", "fun <S> nestedProjected"),
    ] {
        let ty = infer_expression(&ws, "Use.kt", expression);
        let Type::Parameter {
            definition,
            name,
            nullable,
            ..
        } = ty
        else {
            panic!("{expression} lost scoped identity: {ty:?}")
        };
        assert_eq!(name, "S");
        assert!(
            nullable,
            "unbounded S retains its nullable-capable identity"
        );
        assert_eq!(definition.file, "Use.kt");
        assert_eq!(definition.start_byte, source.find(parameter).unwrap() + 5);
        assert_eq!(definition.end_byte, definition.start_byte + 1);
    }
    // Separate files/scopes with the same spelling are never unified by name.
    let first = infer_expression(&ws, "Use.kt", "request.nested(key)");
    ws.open("Second.kt", source.into());
    assert_ne!(
        first,
        infer_expression(&ws, "Second.kt", "request.nested(key)")
    );
    // Probe each body separately so repeated argument spellings cannot select the wrong scope.
    for (file, declaration, expected) in [
        (
            "Projected.kt",
            "fun <S> projected(request: Request, key: Class<out Class<S>>) = request.nested(key)",
            None,
        ),
        (
            "Concrete.kt",
            "fun concrete(pass: Passage, key: Class<out Class<out Invocation>>) = pass.pass(key)",
            Some("Invocation"),
        ),
    ] {
        ws.open(file, format!("package probe\n{declaration}\n"));
        let expression = if file == "Projected.kt" {
            "request.nested(key)"
        } else {
            "pass.pass(key)"
        };
        let ty = infer_expression(&ws, file, expression);
        if let Some(expected) = expected {
            assert_eq!(ty.name(), Some(expected), "{ty:?}");
        } else {
            assert!(matches!(ty, Type::Parameter { .. }), "{ty:?}");
        }
    }
}

#[test]
fn projected_passthrough_keeps_unresolved_and_incompatible_inputs_unknown() {
    for declaration in [
        "fun <S : Missing> use(pass: Passage, key: Class<out Class<out S>>) = pass.pass(key)",
        "fun <S : S> use(pass: Passage, key: Class<out Class<out S>>) = pass.pass(key)",
        "fun use(pass: Passage, key: Class<in Class<out Invocation>>) = pass.pass(key)",
        "fun use(pass: Passage, key: Class<out Class<*>>) = pass.pass(key)",
        "fun use(pass: Passage, key: Class<out Class<Missing>>) = pass.pass(key)",
    ] {
        let mut ws = projected_workspace(&format!("package probe\n{declaration}\n"));
        ws.open("Passage.java", "package probe; public class Passage { public <T> T pass(Class<? extends Class<? extends T>> key) { return null; } }".into());
        assert_eq!(
            infer_expression(&ws, "Use.kt", "pass.pass(key)"),
            Type::Unknown,
            "{declaration}"
        );
    }
    let mut ws = projected_workspace("package probe\nfun use(request: Request, key: Class<out Invocation>) = request.both(key,key)\n");
    assert_eq!(
        infer_expression(&ws, "Use.kt", "request.both(key,key)"),
        Type::Unknown,
        "projection is not equality in an invariant slot"
    );
}

#[test]
fn kotlin_runnable_class_java_arguments_keep_exact_types_before_overload_selection() {
    let source = "package ktlsp.typetools120\nimport net.jodah.typetools.TypeResolver\nclass TypeToolsProbe { fun sample(): Class<*> = TypeResolver.resolveRawArgument(Runnable::class.java, Runnable::class.java) }\n";
    let mut ws = reflection_workspace(source);
    ws.open(
        "Runnable.java",
        "package java.lang; public interface Runnable { void run(); }".into(),
    );
    ws.open(
        "Type.java",
        "package java.lang.reflect; public interface Type {}".into(),
    );
    ws.open("TypeResolver.java", "package net.jodah.typetools; import java.lang.reflect.Type; public class TypeResolver { public static <T,S extends T> Class<?> resolveRawArgument(Class<T> type,Class<S> subType) { return null; } public static Class<?> resolveRawArgument(Type genericType,Class<?> subType) { return null; } }".into());
    let runnable = Type::class("Runnable", Some("java.lang".into()));
    let literal = infer_expression(&ws, "Use.kt", "Runnable::class");
    assert_eq!(literal.name(), Some("KClass"));
    assert_eq!(literal.package(), Some("kotlin.reflect"));
    assert_eq!(literal.args(), [runnable.clone()]);
    let class = infer_expression(&ws, "Use.kt", "Runnable::class.java");
    assert_eq!(class.name(), Some("Class"));
    assert_eq!(class.package(), Some("java.lang"));
    assert_eq!(class.args(), [runnable]);
    let expression = "TypeResolver.resolveRawArgument(Runnable::class.java, Runnable::class.java)";
    let mut parser = KotlinParser::new();
    let tree = parser.parse(source);
    let start = source.find(expression).unwrap();
    let call = tree
        .root_node()
        .descendant_for_byte_range(start, start + expression.len())
        .unwrap();
    assert_eq!(call.kind(), "call_expression");
    let arguments = infer::call_arguments(
        &ws.index,
        call,
        source,
        &FileCtx::from_tree_in_file(&tree, source, "Use.kt"),
    );
    assert_eq!(arguments.count, Some(2));
    assert_eq!(arguments.types, vec![class.clone(), class]);
}

#[test]
fn direct_own_method_bounds_validate_substituted_class_arguments() {
    for (file, declaration) in [
        ("BoundApi.java", "package probe; public class BoundApi { public <T,S extends T> T choose(Class<T> type,Class<S> subType) { return null; } }"),
        ("BoundApi.kt", "package probe\nclass BoundApi { fun <T,S:T> choose(type:Class<T>,subType:Class<S>):T = TODO() }\n"),
    ] {
        let mut ws = reflection_workspace("package probe\nfun use(api:BoundApi) { api.choose(Runnable::class.java,Runnable::class.java); api.choose(Runnable::class.java,SubRunnable::class.java); api.choose(Runnable::class.java,String::class.java) }\n");
        ws.open("Runnable.java", "package java.lang; public interface Runnable { void run(); }".into());
        ws.open("SubRunnable.java", "package java.lang; public interface SubRunnable extends Runnable {}".into());
        ws.open("String.java", "package java.lang; public final class String {}".into());
        ws.open("T.java", "package probe; public class T {}".into());
        ws.open(file, declaration.into());
        let entry = ws.index.lookup_by_name("choose").iter().find(|entry| entry.path.as_ref() == file).unwrap();
        let bounds = entry.sym.method_type_bounds.as_deref().expect("same-method direct bound has scope proof");
        assert_eq!(bounds.len(), 1);
        assert_eq!(bounds[0].0, "S");
        assert_eq!(bounds[0].1.name, "T");
        assert!(bounds[0].1.qualification.is_none());
        let positive = infer_expression(&ws, "Use.kt", "api.choose(Runnable::class.java,Runnable::class.java)");
        assert_eq!(positive, Type::class("Runnable", Some("java.lang".into())), "{file}: {positive:?}");
        let subtype = infer_expression(&ws, "Use.kt", "api.choose(Runnable::class.java,SubRunnable::class.java)");
        assert_eq!(subtype, Type::class("Runnable", Some("java.lang".into())), "{file}: {subtype:?}");
        assert_eq!(infer_expression(&ws, "Use.kt", "api.choose(Runnable::class.java,String::class.java)"), Type::Unknown, "{file}");
    }
}

#[test]
fn method_bound_relations_keep_cycles_foreign_scopes_and_shadow_collisions_unknown() {
    for declaration in [
        "package probe; public class BoundApi { public <T extends S,S extends T> T choose(Class<T> type,Class<S> subType) { return null; } }",
        "package probe; public class BoundApi<T> { public static <S extends T> S choose(Class<S> type) { return null; } }",
        "package probe; public class BoundApi<T> { public <T,S extends T> T choose(Class<T> type,Class<S> subType) { return null; } }",
    ] {
        let ws = workspace(&[("BoundApi.java", declaration)]);
        let entry = &ws.index.lookup_by_name("choose")[0];
        assert!(entry.sym.method_type_bounds.is_none(), "{declaration}: {:?}", entry.sym.method_type_bounds);
    }
    let ws = workspace(&[("BoundApi.java", "package probe; public class BoundApi<T> { public <S extends T> S choose(Class<S> type) { return null; } }")]);
    let entry = &ws.index.lookup_by_name("choose")[0];
    let bounds = entry
        .sym
        .method_type_bounds
        .as_deref()
        .expect("the receiver owner's parameter is scoped");
    assert_eq!(bounds.len(), 1);
    assert_eq!(bounds[0].0, "S");
    assert_eq!(bounds[0].1.name, "T");
    assert!(bounds[0].1.qualification.is_none());
}

#[test]
fn f_bounded_property_arguments_preserve_enum_result_across_languages() {
    let source = workspace(&[
        ("Object.java", "package java.lang; public class Object {}"),
        ("Comparable.java", "package java.lang; public interface Comparable<T> { int compareTo(T other); }"),
        ("Enum.java", "package java.lang; public abstract class Enum<E extends Enum<E>> implements Comparable<E> { public int compareTo(E other) { return 0; } }"),
        ("Named.java", "package probe; public interface Named {}"),
        ("Direction.java", "package probe; public class Direction { public enum Axis implements Named { X,Y,Z; public boolean isVertical() { return this==Y; } } }"),
        ("Property.java", "package probe; public class Property<T extends Comparable<T>> {}"),
        ("EnumProperty.java", "package probe; public class EnumProperty<T extends Enum<T> & Named> extends Property<T> {}"),
        ("BlockStateProperties.java", "package probe; public class BlockStateProperties { public static final EnumProperty<Direction.Axis> AXIS = null; }"),
        ("StateHolder.java", "package probe; public class StateHolder { public <T extends Comparable<T>> T getValue(Property<T> property) { return null; } }"),
        ("BlockState.java", "package probe; public class BlockState extends StateHolder {}"),
    ]);
    let root = std::env::temp_dir().join(format!(
        "ktlsp-f-bounds-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let key = |name: &str| ktlsp::paths::file_key(&root.join(name));
    let mut ws = Workspace::new();
    assert_eq!(ws.scan(&root), 0);
    ws.set_library_index_complete(true);
    ws.set_jdk_index_complete(true);
    for file in source.open_doc_keys() {
        ws.open(key(&file), source.doc_text(&file).unwrap());
    }
    for (file, source) in [
        ("Use.java", "package consumer; import probe.BlockState; import probe.Property; import probe.Direction; import probe.BlockStateProperties; class Use { void use(BlockState state,Property<Direction.Axis> property) { state.getValue(property).isVertical(); state.getValue(BlockStateProperties.AXIS).isVertical(); state.getValue(probe.BlockStateProperties.AXIS).isVertical(); } }"),
        ("Use.kt", "package consumer\nimport probe.BlockState\nimport probe.Property\nimport probe.Direction\nimport probe.BlockStateProperties\nfun use(state:BlockState,property:Property<Direction.Axis>) { state.getValue(property).isVertical(); state.getValue(BlockStateProperties.AXIS).isVertical(); state.getValue(probe.BlockStateProperties.AXIS).isVertical() }\n"),
    ] {
        let file = key(file);
        ws.open(&file, source.into());
            if file.ends_with(".kt") {
            let actual = infer_expression(&ws, &file, "BlockStateProperties.AXIS");
            assert_eq!(actual.name(), Some("EnumProperty"));
            assert_eq!(actual.package(), Some("probe"));
            assert_eq!(actual.args()[0].name(), Some("Axis"));
            assert_eq!(actual.args()[0].container(), Some("Direction"));
            let supertypes = infer::direct_supertypes(&ws.index, &actual);
            assert_eq!(supertypes.len(), 1);
            assert_eq!(supertypes[0].name(), Some("Property"));
            assert_eq!(supertypes[0].args(), actual.args());
        }
        for expression in ["state.getValue(property)", "state.getValue(BlockStateProperties.AXIS)", "state.getValue(probe.BlockStateProperties.AXIS)"] {
            assert_member(&mut ws, &file, &format!("{expression}.isVert"), "isVertical", "missingAxisMethod");
            let start = source.find(&format!("{expression}.isVertical")).unwrap() + expression.len() + 1;
            let defs = ws.goto_definition(&file, start);
            assert_eq!(defs.len(), 1, "{file}: {expression}: {defs:?}");
            assert_eq!(defs[0].file, key("Direction.java"));
            let target = ws.doc_text(&key("Direction.java")).unwrap();
            assert_eq!(&target[defs[0].start_byte..defs[0].end_byte], "isVertical");
            if file.ends_with(".kt") {
                let result = infer_expression(&ws, &file, expression);
                assert_eq!(result.name(), Some("Axis"), "{expression}: {result:?}");
                assert_eq!(result.package(), Some("probe"));
                assert_eq!(result.container(), Some("Direction"));
            }
        }
    }
    for (name, source) in [
        ("KnownOwner.java", "package consumer; import probe.BlockState; class KnownOwner { void valid(BlockState state) { state.getValue(probe.BlockStateProperties.AXIS).isVertical(); } }"),
        ("KnownOwner.kt", "package consumer\nimport probe.BlockState\nclass KnownKotlinOwner { fun valid(state:BlockState) { state.getValue(probe.BlockStateProperties.AXIS).isVertical() } }\n"),
        ("PlainLambda.kt", "package consumer\nimport probe.BlockState\nfun plain(block:()->Unit) {}\nfun valid(state:BlockState) { plain { state.getValue(probe.BlockStateProperties.AXIS).isVertical() } }\n"),
    ] {
        let file = key(name);
        ws.open(&file, source.into());
        assert_member(&mut ws, &file, "state.getValue(probe.BlockStateProperties.AXIS).isVert", "isVertical", "missingAxisMethod");
        let start = source.find("isVertical").unwrap();
        let definitions = ws.goto_definition(&file, start);
        assert_eq!(definitions.len(), 1, "{name}: {definitions:?}");
        assert_eq!(definitions[0].file, key("Direction.java"));
        if name == "KnownOwner.kt" {
            ws.change(&file, source.replace("class KnownKotlinOwner", "class KnownKotlinOwner:Missing()"));
            assert!(!labels(&mut ws, &file, "state.getValue(probe.BlockStateProperties.AXIS).isVert").contains("isVertical"), "a changed header must retract namespace proof");
            ws.change(&file, source.into());
            assert_member(&mut ws, &file, "state.getValue(probe.BlockStateProperties.AXIS).isVert", "isVertical", "missingAxisMethod");
        }
    }
    ws.open(
        key("Root.java"),
        "package shadow; public class Root { public static Object probe; }".into(),
    );
    for (name, source) in [
        ("Local.java", "package consumer; import probe.BlockState; class Local { void invalid(BlockState state,Object probe) { state.getValue(probe.BlockStateProperties.AXIS).isVertical(); } }"),
        ("Local.kt", "package consumer\nimport probe.BlockState\nfun invalid(state:BlockState,probe:Any) { state.getValue(probe.BlockStateProperties.AXIS).isVertical() }\n"),
        ("Imported.java", "package consumer; import probe.BlockState; import static shadow.Root.probe; class Imported { void invalid(BlockState state) { state.getValue(probe.BlockStateProperties.AXIS).isVertical(); } }"),
        ("Imported.kt", "package consumer\nimport probe.BlockState\nimport shadow.Root.probe\nfun invalid(state:BlockState) { state.getValue(probe.BlockStateProperties.AXIS).isVertical() }\n"),
        ("Wildcard.java", "package consumer; import probe.BlockState; import static shadow.Root.*; class Wildcard { void invalid(BlockState state) { state.getValue(probe.BlockStateProperties.AXIS).isVertical(); } }"),
        ("Wildcard.kt", "package consumer\nimport probe.BlockState\nimport shadow.Root.*\nfun invalid(state:BlockState) { state.getValue(probe.BlockStateProperties.AXIS).isVertical() }\n"),
        ("MissingOwner.java", "package consumer; import probe.BlockState; class MissingOwner extends Missing { void invalid(BlockState state) { state.getValue(probe.BlockStateProperties.AXIS).isVertical(); } }"),
        ("MissingOwner.kt", "package consumer\nimport probe.BlockState\nclass MissingKotlinOwner:Missing() { fun invalid(state:BlockState) { state.getValue(probe.BlockStateProperties.AXIS).isVertical() } }\n"),
        ("UnknownApply.kt", "package consumer\nimport probe.BlockState\nfun invalid(state:BlockState) { missing.apply { state.getValue(probe.BlockStateProperties.AXIS).isVertical() } }\n"),
        ("UnknownReceiver.kt", "package consumer\nimport probe.BlockState\nfun unknownScope(block:Missing.()->Unit) {}\nfun invalid(state:BlockState) { unknownScope { state.getValue(probe.BlockStateProperties.AXIS).isVertical() } }\n"),
        ("AnnotatedOwner.kt", "package consumer\nimport probe.BlockState\n@UnknownPlugin class AnnotatedOwner { fun invalid(state:BlockState) { state.getValue(probe.BlockStateProperties.AXIS).isVertical() } }\n"),
        ("Anonymous.kt", "package consumer\nimport probe.BlockState\nfun invalid(state:BlockState) { val owner = object:Missing() { fun invalid() { state.getValue(probe.BlockStateProperties.AXIS).isVertical() } } }\n"),
        ("Anonymous.java", "package consumer; import probe.BlockState; class Anonymous { void invalid(BlockState state) { Object owner = new Missing() { void invalid() { state.getValue(probe.BlockStateProperties.AXIS).isVertical(); } }; } }"),
        ("LaterField.kt", "package consumer\nimport probe.BlockState\nclass LaterField { fun invalid(state:BlockState) { state.getValue(probe.BlockStateProperties.AXIS).isVertical() } val probe:Missing = TODO() }\n"),
        ("MissingImport.kt", "package consumer\nimport probe.BlockState\nimport shadow.Missing.probe\nfun invalid(state:BlockState) { state.getValue(probe.BlockStateProperties.AXIS).isVertical() }\n"),
    ] {
        let file = key(name);
        ws.open(&file, source.into());
        let prefix = "state.getValue(probe.BlockStateProperties.AXIS).isVert";
        assert!(!labels(&mut ws, &file, prefix).contains("isVertical"), "{name}: a value or unknown ancestor can shadow the package root");
        let start = source.find(prefix).unwrap() + prefix.len() - "isVert".len();
        assert!(ws.goto_definition(&file, start).is_empty(), "{name}: root shadow must also block navigation");
    }
    ws.open(key("KotlinStateHolder.kt"), "package probe\nclass KotlinStateHolder { fun <T:Comparable<T>> getValue(property:Property<T>):T = TODO() }\n".into());
    ws.open(key("Nullable.kt"), "package probe\nfun invalid(state:KotlinStateHolder,property:EnumProperty<Direction.Axis>?) { state.getValue(property).isVertical() }\n".into());
    assert_eq!(
        infer_expression(&ws, &key("Nullable.kt"), "state.getValue(property)"),
        Type::Unknown
    );
    assert!(!labels(
        &mut ws,
        &key("Nullable.kt"),
        "state.getValue(property).isVert"
    )
    .contains("isVertical"));

    // A visible matching path cannot conceal an unresolved or cyclic second path.
    ws.open(
        key("Loop.java"),
        "package probe; public interface Loop<T> extends Loop<T> {}".into(),
    );
    for parents in ["Missing", "Loop<T>"] {
        ws.open(key("EnumProperty.java"), format!("package probe; public class EnumProperty<T extends Enum<T> & Named> extends Property<T> implements {parents} {{}}"));
        assert_eq!(
            infer_expression(
                &ws,
                &key("Use.kt"),
                "state.getValue(BlockStateProperties.AXIS)"
            ),
            Type::Unknown,
            "{parents}"
        );
        for file in [key("Use.kt"), key("Use.java")] {
            assert!(
                !labels(
                    &mut ws,
                    &file,
                    "state.getValue(BlockStateProperties.AXIS).isVert"
                )
                .contains("isVertical"),
                "{file}: {parents}"
            );
        }
    }
    ws.open(key("EnumProperty.java"), "package probe; public class EnumProperty<T extends Enum<T> & Named> extends Property<T> {}".into());
    for file in [key("Use.kt"), key("Use.java")] {
        assert_member(
            &mut ws,
            &file,
            "state.getValue(BlockStateProperties.AXIS).isVert",
            "isVertical",
            "missingAxisMethod",
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn nominal_method_f_bounds_retain_scoped_method_and_receiver_variables() {
    for (file, declaration, parameter, head) in [
        ("BoundApi.java", "package probe; public class BoundApi { public <T extends Comparable<T>> T choose(T value) { return value; } }", "T", "Comparable"),
        ("BoundApi.kt", "package probe\nclass BoundApi { fun <T:Comparable<T>> choose(value:T):T = value }\n", "T", "Comparable"),
        ("BoundApi.java", "package probe; public class BoundApi { public <T,S extends java.util.List<T>> S choose(Class<T> type,Class<S> subType) { return null; } }", "S", "List"),
        ("BoundApi.java", "package probe; public class BoundApi<T> { public <S extends Comparable<T>> S choose(S value) { return value; } }", "S", "Comparable"),
    ] {
        let ws = workspace(&[(file, declaration)]);
        let entry = &ws.index.lookup_by_name("choose")[0];
        let bounds = entry.sym.method_type_bounds.as_deref().expect("scope-proven nominal F-bound");
        assert_eq!(bounds.len(), 1);
        assert_eq!(bounds[0].0, parameter);
        assert_eq!(bounds[0].1.name, head);
        assert_eq!(bounds[0].1.args.len(), 1);
        assert_eq!(bounds[0].1.args[0].name, "T");
        assert!(bounds[0].1.args[0].qualification.is_none());
    }
    for declaration in [
        "package probe; public class BoundApi<T> { public <T extends Comparable<T>> T choose(T value) { return value; } }",
    ] {
        let ws = workspace(&[("BoundApi.java", declaration)]);
        assert!(ws.index.lookup_by_name("choose")[0].sym.method_type_bounds.is_none());
    }
}
