//! Compiler-valid CEM reductions: superclass dispatch and implicit extension-call receivers.

use std::collections::HashSet;

use ktlsp::edit::apply_to_text;
use ktlsp::infer::{self, FileCtx};
use ktlsp::parser::KotlinParser;
use ktlsp::symbol::{Def, SymbolKind};
use ktlsp::types::Type;
use ktlsp::workspace::Workspace;

const BASE: &str =
    "package probe; public class Base { public float getBreakSpeed() { return 1f; } }";
const OTHER: &str =
    "package probe; public class Other { public float getBreakSpeed() { return 2f; } }";
const LOOKUP: &str =
    "package probe; public class Lookup<T> { public T getOrThrow(Key<T> key) { return null; } }";
const OTHER_LOOKUP: &str = "package probe; public class OtherLookup { public Object getOrThrow(Object key) { return null; } }";
const KEY: &str = "package probe; public class Key<T> {}";
const EXTENSIONS: &str = "package probe\nclass Payload {}\nfun <T> Base.holderLookup(key: Key<T>): Lookup<T> = Lookup<T>()\nfun <T> holderFree(key: Key<T>): Lookup<T> = Lookup<T>()\n";
const DRILL: &str = "package probe\nclass Drill : Base() {\n    override fun getBreakSpeed(): Float {\n        return super.getBreakSpeed()\n    }\n    fun bare(key: Key<Payload>): Payload {\n        return holderLookup(key).getOrThrow(key)\n    }\n    fun qualified(key: Key<Payload>): Payload {\n        return this.holderLookup(key).getOrThrow(key)\n    }\n    fun free(key: Key<Payload>): Payload {\n        return holderFree(key).getOrThrow(key)\n    }\n    fun typed(lookup: Lookup<Payload>, key: Key<Payload>): Payload {\n        return lookup.getOrThrow(key)\n    }\n}\n";

fn workspace() -> Workspace {
    let mut ws = Workspace::new();
    for (file, source) in [
        ("Base.java", BASE),
        ("Other.java", OTHER),
        ("Key.java", KEY),
        ("Lookup.java", LOOKUP),
        ("OtherLookup.java", OTHER_LOOKUP),
        ("Extensions.kt", EXTENSIONS),
        ("Drill.kt", DRILL),
    ] {
        ws.open(file, source.to_string());
    }
    ws
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

fn target(ws: &Workspace, file: &str, name: &str) -> Def {
    let entry = ws
        .index
        .lookup_by_name(name)
        .iter()
        .find(|entry| entry.path.as_ref() == file && entry.sym.kind == SymbolKind::Function)
        .unwrap();
    Def {
        file: file.to_string(),
        start_byte: entry.sym.start_byte,
        end_byte: entry.sym.end_byte,
    }
}

fn goto(ws: &mut Workspace, file: &str, expression: &str, name: &str) -> Vec<Def> {
    let source = ws.doc_text(file).unwrap();
    let offset = source.find(expression).unwrap() + expression.rfind(name).unwrap();
    ws.goto_definition(file, offset)
}

fn infer_expression(ws: &Workspace, file: &str, expression: &str) -> Type {
    let source = ws.doc_text(file).unwrap();
    let tree = KotlinParser::new().parse(&source);
    let start = source.find(expression).unwrap();
    let node = tree
        .root_node()
        .named_descendant_for_byte_range(start, start + expression.len())
        .unwrap();
    assert_eq!(&source[node.byte_range()], expression);
    infer::infer(
        &ws.index.scoped_for_file(file),
        node,
        &source,
        &FileCtx::from_tree_in_file(&tree, &source, file),
    )
}

#[test]
fn java_constructor_getters_share_completion_goto_and_generic_result_binding() {
    let mut ws = Workspace::new();
    let properties = "package probe\nclass Payload { fun work(): Int = 1 }\nclass Box<T>(val value: T)\nclass Plain(val payload: Payload)\n";
    let caller = "package probe; class Use { void use(Box<Payload> box, Plain plain) { box.getValue().work(); plain.getPayload().work(); } }";
    ws.open("Properties.kt", properties.into());
    ws.open(
        "Decoy.kt",
        "package probe\nclass Decoy { fun work(): Int = 2 }\n".into(),
    );
    ws.open("GetterUse.java", caller.into());
    for (expression, name, property) in [
        ("box.getValue()", "getValue", "value"),
        ("plain.getPayload()", "getPayload", "payload"),
    ] {
        let completion = labels(&mut ws, "GetterUse.java", &expression.replace("()", ""));
        assert!(
            completion.contains(name),
            "{expression}: {completion:?}; facts {:?}",
            ws.index.jvm_source_for_file("Properties.kt")
        );
        let expected = ws
            .index
            .lookup_by_name(property)
            .iter()
            .find(|entry| entry.path.as_ref() == "Properties.kt")
            .unwrap();
        let expected = Def {
            file: "Properties.kt".into(),
            start_byte: expected.sym.start_byte,
            end_byte: expected.sym.end_byte,
        };
        assert_eq!(
            goto(&mut ws, "GetterUse.java", expression, name),
            vec![expected]
        );
        assert_eq!(
            goto(
                &mut ws,
                "GetterUse.java",
                &format!("{expression}.work()"),
                "work"
            ),
            vec![target(&ws, "Properties.kt", "work")]
        );
    }
    let rename_offset = properties.find("val value").unwrap() + "val ".len();
    let edits = ws
        .rename("Properties.kt", rename_offset, "renamed")
        .unwrap();
    assert_eq!(
        apply_to_text("Properties.kt", properties, &edits).unwrap(),
        properties.replace("val value", "val renamed")
    );
    assert_eq!(
        apply_to_text("GetterUse.java", caller, &edits).unwrap(),
        caller.replace("box.getValue", "box.getRenamed")
    );
    let mut kotlin_only = Workspace::new();
    kotlin_only.open("Properties.kt", properties.into());
    let kotlin_use = "package probe\nfun use(box: Box<Payload>) = box.value\n";
    kotlin_only.open("PropertyUse.kt", kotlin_use.into());
    let edits = kotlin_only
        .rename("Properties.kt", rename_offset, "renamed")
        .expect("ordinary Kotlin-only property rename");
    assert_eq!(
        apply_to_text("Properties.kt", properties, &edits).unwrap(),
        properties.replace("val value", "val renamed")
    );
    assert_eq!(
        apply_to_text("PropertyUse.kt", kotlin_use, &edits).unwrap(),
        kotlin_use.replace("box.value", "box.renamed")
    );
    let is_property = properties.replace("val value", "val isValue");
    ws.change("Properties.kt", is_property.clone());
    ws.change("GetterUse.java", caller.replace("getValue", "isValue"));
    let edits = ws
        .rename(
            "Properties.kt",
            is_property.find("val isValue").unwrap() + 4,
            "renamed",
        )
        .unwrap();
    assert_eq!(
        apply_to_text("Properties.kt", &is_property, &edits).unwrap(),
        is_property.replace("val isValue", "val renamed")
    );
    assert_eq!(
        apply_to_text(
            "GetterUse.java",
            &caller.replace("getValue", "isValue"),
            &edits
        )
        .unwrap(),
        caller.replace("getValue", "getRenamed")
    );
    ws.change("Properties.kt", properties.into());
    ws.change("GetterUse.java", caller.into());
    ws.change(
        "Properties.kt",
        properties.replace("val value", "private val value"),
    );
    assert!(!labels(&mut ws, "GetterUse.java", "box.getValue").contains("getValue"));
    assert!(goto(&mut ws, "GetterUse.java", "box.getValue()", "getValue").is_empty());
    ws.change("Properties.kt", properties.into());
    assert!(labels(&mut ws, "GetterUse.java", "box.getValue").contains("getValue"));
    ws.change(
        "GetterUse.java",
        caller.replace("box.getValue()", "Box.getValue()"),
    );
    assert!(!labels(&mut ws, "GetterUse.java", "Box.getValue").contains("getValue"));
    assert!(goto(&mut ws, "GetterUse.java", "Box.getValue()", "getValue").is_empty());
}

#[test]
fn java_inherited_constructor_getters_keep_source_identity_and_generic_binding() {
    let mut ws = Workspace::new();
    let properties = "package probe\nclass Payload { fun work(): Int = 1 }\nopen class KotlinBase<T>(val value: T, val isReady: Boolean)\n";
    ws.open("Inherited.kt", properties.into());
    ws.open(
        "Child.java",
        "package probe; public class Child extends KotlinBase<Payload> { public Child() { super(null, true); } }".into(),
    );
    ws.open(
        "Decoy.kt",
        "package probe\nclass Decoy { fun work(): Int = 2 }\n".into(),
    );
    let caller = "package probe; class Use { void use(Child child) { child.getValue().work(); child.isReady(); } }";
    ws.open("InheritedUse.java", caller.into());
    for (name, property) in [("getValue", "value"), ("isReady", "isReady")] {
        assert!(labels(&mut ws, "InheritedUse.java", &format!("child.{name}")).contains(name));
        let entry = ws
            .index
            .lookup_by_name(property)
            .iter()
            .find(|entry| {
                entry.path.as_ref() == "Inherited.kt" && entry.sym.kind == SymbolKind::Property
            })
            .unwrap();
        let expected = Def {
            file: "Inherited.kt".into(),
            start_byte: entry.sym.start_byte,
            end_byte: entry.sym.end_byte,
        };
        assert_eq!(
            goto(
                &mut ws,
                "InheritedUse.java",
                &format!("child.{name}()"),
                name
            ),
            vec![expected]
        );
    }
    assert_eq!(
        goto(
            &mut ws,
            "InheritedUse.java",
            "child.getValue().work()",
            "work"
        ),
        vec![target(&ws, "Inherited.kt", "work")]
    );

    // Completion with an empty prefix still traverses each owner once, and source properties
    // only appear as JVM callables with their declaring owner, never as authored functions.
    {
        use ktlsp::index::{Index, InferenceIndex, LookupEntries};
        use std::cell::RefCell;
        struct Counted<'a> {
            index: &'a Index,
            owners: RefCell<Vec<String>>,
        }
        impl InferenceIndex for Counted<'_> {
            fn lookup_by_name(&self, name: &str) -> LookupEntries<'_> {
                LookupEntries::All(self.index.lookup_by_name(name))
            }
            fn members_of(&self, name: &str) -> LookupEntries<'_> {
                self.owners.borrow_mut().push(name.into());
                LookupEntries::All(self.index.members_of(name))
            }
            fn extensions_for(&self, name: &str) -> LookupEntries<'_> {
                LookupEntries::All(self.index.extensions_for(name))
            }
            fn jvm_sources(&self, owner: &str) -> Vec<&ktcore::jvm::SourceFile> {
                self.index.jvm_sources(owner)
            }
        }
        let index = Counted {
            index: &ws.index,
            owners: RefCell::new(Vec::new()),
        };
        let candidates =
            ktcore::jvm::member_candidates(&index, &Type::class("Child", Some("probe".into())), "");
        let getters: Vec<_> = candidates
            .iter()
            .filter(|candidate| matches!(candidate.name(), "getValue" | "isReady"))
            .collect();
        assert_eq!(getters.len(), 2);
        for getter in getters {
            assert_eq!(
                getter.identity(&index).unwrap().class.internal_name,
                "probe/KotlinBase"
            );
        }
        assert!(candidates.iter().all(|candidate| !matches!(candidate, ktcore::jvm::CallableCandidate::Authored(entry) if entry.sym.kind == SymbolKind::Property)));
        for owner in ["Child", "KotlinBase"] {
            assert_eq!(
                index
                    .owners
                    .borrow()
                    .iter()
                    .filter(|name| *name == owner)
                    .count(),
                1,
                "one source candidate walk for {owner}"
            );
        }
    }
    ws.change(
        "Inherited.kt",
        properties.replace("val value", "private val value"),
    );
    assert!(!labels(&mut ws, "InheritedUse.java", "child.getValue").contains("getValue"));
    assert!(goto(&mut ws, "InheritedUse.java", "child.getValue()", "getValue").is_empty());
    ws.change("Inherited.kt", properties.into());
    assert!(labels(&mut ws, "InheritedUse.java", "child.getValue").contains("getValue"));
}

#[test]
fn java_typed_body_and_overriding_property_getters_share_the_nearest_definition() {
    let mut ws = Workspace::new();
    let source = "package probe\nclass Payload { fun work(): Int = 1 }\nopen class Base<T>(open val value: T)\nclass Child(override val value: Payload) : Base<Payload>(value)\nopen class Body(seed: Payload) {\n open val body: Payload = seed\n var mutable: Payload = seed\n val computed: Payload get() = body\n}\nclass BodyChild(seed: Payload) : Body(seed) { override val body: Payload = seed }\n";
    ws.open("BodyProperties.kt", source.into());
    ws.open(
        "Decoy.kt",
        "package probe\nclass Decoy { fun work(): Int = 2 }\n".into(),
    );
    let caller = "package probe; class Use { void use(Base<Payload> base, Child child, Body body, BodyChild derived) { base.getValue().work(); child.getValue().work(); body.getBody().work(); body.getMutable().work(); body.getComputed().work(); derived.getBody().work(); } }";
    ws.open("BodyUse.java", caller.into());
    for (receiver, name, property, owner) in [
        ("base", "getValue", "value", "Base"),
        ("child", "getValue", "value", "Child"),
        ("body", "getBody", "body", "Body"),
        ("body", "getMutable", "mutable", "Body"),
        ("body", "getComputed", "computed", "Body"),
        ("derived", "getBody", "body", "BodyChild"),
    ] {
        let expression = format!("{receiver}.{name}()");
        assert!(
            labels(&mut ws, "BodyUse.java", &format!("{receiver}.{name}")).contains(name),
            "{expression}"
        );
        let entry = ws
            .index
            .lookup_by_name(property)
            .iter()
            .find(|entry| {
                entry.sym.container.as_deref() == Some(owner)
                    && entry.sym.kind == SymbolKind::Property
            })
            .unwrap();
        let expected = Def {
            file: "BodyProperties.kt".into(),
            start_byte: entry.sym.start_byte,
            end_byte: entry.sym.end_byte,
        };
        assert_eq!(
            goto(&mut ws, "BodyUse.java", &expression, name),
            vec![expected],
            "{expression}"
        );
        assert_eq!(
            goto(
                &mut ws,
                "BodyUse.java",
                &format!("{expression}.work()"),
                "work"
            ),
            vec![target(&ws, "BodyProperties.kt", "work")]
        );
    }
    ws.change(
        "BodyProperties.kt",
        source.replace("override val body", "@get:Unknown override val body"),
    );
    assert!(!labels(&mut ws, "BodyUse.java", "derived.getBody").contains("getBody"));
    assert!(
        goto(&mut ws, "BodyUse.java", "derived.getBody()", "getBody").is_empty(),
        "an unknown nearest override must not navigate to the base property"
    );
    ws.change("BodyProperties.kt", source.into());
    assert!(labels(&mut ws, "BodyUse.java", "derived.getBody").contains("getBody"));
}

#[test]
fn java_property_setters_share_generic_parameters_unit_return_and_source_definitions() {
    let mut ws = Workspace::new();
    // Stdlib 2.4.0 indexes common expect and JVM actual headers for the same intrinsic types.
    // Their expect marker is not evidence that an ordinary scalar accessor is value-class mangled.
    for (file, source) in [
        (
            "CommonPrimitives.kt",
            "package kotlin\npublic expect class String : Comparable<String>, CharSequence\npublic expect class Boolean : Comparable<Boolean>\npublic expect class Int : Number, Comparable<Int>\n",
        ),
        (
            "JvmPrimitives.kt",
            "package kotlin\npublic actual class String : Comparable<String>, CharSequence\npublic actual class Boolean private constructor() : Comparable<Boolean>\npublic actual class Int private constructor() : Number(), Comparable<Int>\n",
        ),
    ] {
        ws.open(file, source.into());
    }
    let source = "package probe\nopen class Box<T>(open var value: T)\nclass TextBox(override var value: String) : Box<String>(value)\nclass Flags(var isReady: Boolean, var is_foo: String, var is0: Int, var isfoo: String)\nclass Body { var text: String = \"body\"\n var hidden: String = \"hidden\"\n  private set\n}\n";
    ws.open("SetterProperties.kt", source.into());
    ws.open(
        "String.java",
        "package java.lang; public final class String { public int length() { return 1; } }".into(),
    );
    ws.open(
        "Decoy.java",
        "package probe; class Decoy { public int length() { return 2; } }".into(),
    );
    let caller = "package probe; class Use { void update(Box<String> box, TextBox child, Flags flags, Body body) { box.setValue(\"ok\"); child.setValue(\"ok\"); flags.setReady(true); flags.set_foo(\"ok\"); flags.set0(1); flags.setIsfoo(\"ok\"); body.setText(\"ok\"); body.getHidden(); body.setHidden(\"bad\"); box.setValue(1); box.setValue(\"ok\").length(); } }";
    ws.open("SetterUse.java", caller.into());
    for (receiver, method, argument, property, owner) in [
        ("box", "setValue", "\"ok\"", "value", "Box"),
        ("child", "setValue", "\"ok\"", "value", "TextBox"),
        ("flags", "setReady", "true", "isReady", "Flags"),
        ("flags", "set_foo", "\"ok\"", "is_foo", "Flags"),
        ("flags", "set0", "1", "is0", "Flags"),
        ("flags", "setIsfoo", "\"ok\"", "isfoo", "Flags"),
        ("body", "setText", "\"ok\"", "text", "Body"),
    ] {
        assert!(
            labels(&mut ws, "SetterUse.java", &format!("{receiver}.{method}")).contains(method),
            "{receiver}.{method}"
        );
        let entry = ws
            .index
            .lookup_by_name(property)
            .iter()
            .find(|entry| {
                entry.sym.container.as_deref() == Some(owner)
                    && entry.sym.kind == SymbolKind::Property
            })
            .unwrap();
        let definition = Def {
            file: "SetterProperties.kt".into(),
            start_byte: entry.sym.start_byte,
            end_byte: entry.sym.end_byte,
        };
        assert_eq!(
            goto(
                &mut ws,
                "SetterUse.java",
                &format!("{receiver}.{method}({argument})"),
                method
            ),
            vec![definition]
        );
    }
    assert!(!labels(&mut ws, "SetterUse.java", "body.setHidden").contains("setHidden"));
    assert!(labels(&mut ws, "SetterUse.java", "body.getHidden").contains("getHidden"));
    assert!(goto(
        &mut ws,
        "SetterUse.java",
        "body.setHidden(\"bad\")",
        "setHidden"
    )
    .is_empty());
    assert!(goto(
        &mut ws,
        "SetterUse.java",
        "box.setValue(\"ok\").length()",
        "length"
    )
    .is_empty());
    {
        use ktcore::{jvm, members};
        let index = ws.index.scoped_for_file("SetterUse.java");
        let mut receiver = Type::class("Box", Some("probe".into()));
        if let Type::Class { args, .. } = &mut receiver {
            args.push(Type::class("String", Some("java.lang".into())));
        }
        let ctx = FileCtx::new("probe".into(), Vec::new());
        for (argument, applicability) in [
            (
                Type::class("String", Some("java.lang".into())),
                members::Applicability::Applicable,
            ),
            (
                Type::class("Int", Some("kotlin".into())),
                members::Applicability::Rejected,
            ),
        ] {
            let selected = jvm::select_call(
                &index,
                &receiver,
                jvm::member_candidates(&index, &receiver, "setValue"),
                "setValue",
                &members::CallArguments {
                    count: Some(1),
                    types: vec![argument],
                    names: None,
                    has_spread: false,
                    trailing_lambda: false,
                },
                &ctx,
            );
            assert_eq!(selected.applicability, applicability);
            if applicability == members::Applicability::Applicable {
                let [candidate] = selected.consumable_candidates() else {
                    panic!("one applicable setter")
                };
                assert_eq!(
                    jvm::projected_return_type_in_context(&index, &receiver, *candidate, &ctx),
                    Some(Type::class("Unit", Some("kotlin".into())))
                );
                assert_eq!(
                    candidate.identity(&index).unwrap().descriptor,
                    "(Ljava/lang/Object;)V"
                );
            }
        }
    }
    // Existing Java navigation retains one known declaration for an invalid call; applicability
    // above is still Rejected, and no property-valued result is inferred for this void method.
    assert_eq!(
        goto(&mut ws, "SetterUse.java", "box.setValue(1)", "setValue").len(),
        1
    );
    ws.change(
        "SetterProperties.kt",
        source.replace("var value", "val value"),
    );
    assert!(!labels(&mut ws, "SetterUse.java", "box.setValue").contains("setValue"));
    assert!(goto(
        &mut ws,
        "SetterUse.java",
        "box.setValue(\"ok\")",
        "setValue"
    )
    .is_empty());
    ws.change("SetterProperties.kt", source.into());
    assert!(labels(&mut ws, "SetterUse.java", "box.setValue").contains("setValue"));
}

#[test]
fn inferred_property_accessors_follow_current_source_and_foreign_return_type_edits() {
    let mut ws = Workspace::new();
    let model = "package probe\nclass Model { val title = \"x\"\n var value = produce()\n}\n";
    let provider = "package probe\nclass Payload { fun payloadOnly(): Int = 1 }\nclass Other { fun otherOnly(): Int = 2 }\nfun produce(): Payload = Payload()\n";
    ws.open("Inferred.kt", model.into());
    ws.open("Provider.kt", provider.into());
    ws.open(
        "String.java",
        "package java.lang; public class String { public int length() { return 1; } }".into(),
    );
    ws.open(
        "Decoy.kt",
        "package probe\nclass Decoy { fun payloadOnly(): Int = 3\n fun otherOnly(): Int = 4 }\n"
            .into(),
    );
    let caller = "package probe; class Use { void use(Model model, Payload payload, Other other) { model.getTitle().length(); model.getValue().payloadOnly(); model.getValue().otherOnly(); model.setValue(payload); model.setValue(other); } }";
    ws.open("InferredUse.java", caller.into());
    assert!(labels(&mut ws, "InferredUse.java", "model.getTitle").contains("getTitle"));
    assert_eq!(
        goto(
            &mut ws,
            "InferredUse.java",
            "model.getTitle().length()",
            "length"
        ),
        vec![target(&ws, "String.java", "length")]
    );
    for (source, member, absent) in [
        (provider.to_string(), "payloadOnly", "otherOnly"),
        (
            provider.replace(
                "produce(): Payload = Payload()",
                "produce(): Other = Other()",
            ),
            "otherOnly",
            "payloadOnly",
        ),
        (provider.to_string(), "payloadOnly", "otherOnly"),
    ] {
        ws.change("Provider.kt", source);
        let entry = ws
            .index
            .lookup_by_name("value")
            .iter()
            .find(|entry| entry.path.as_ref() == "Inferred.kt")
            .unwrap();
        let definition = Def {
            file: "Inferred.kt".into(),
            start_byte: entry.sym.start_byte,
            end_byte: entry.sym.end_byte,
        };
        assert!(labels(&mut ws, "InferredUse.java", "model.getValue").contains("getValue"));
        assert_eq!(
            goto(&mut ws, "InferredUse.java", "model.getValue()", "getValue"),
            vec![definition]
        );
        assert_eq!(
            goto(
                &mut ws,
                "InferredUse.java",
                &format!("model.getValue().{member}()"),
                member
            ),
            vec![target(&ws, "Provider.kt", member)]
        );
        assert!(goto(
            &mut ws,
            "InferredUse.java",
            &format!("model.getValue().{absent}()"),
            absent
        )
        .is_empty());
        assert!(labels(&mut ws, "InferredUse.java", "model.setValue").contains("setValue"));
        let help = ws
            .signature_help(
                "InferredUse.java",
                caller.find("model.setValue(").unwrap() + "model.setValue(".len(),
            )
            .unwrap();
        assert_eq!(
            help.signatures[0].label,
            format!(
                "setValue(p1: {}): Unit",
                if member == "payloadOnly" {
                    "Payload"
                } else {
                    "Other"
                }
            )
        );
    }
    ws.change(
        "Provider.kt",
        provider.replace("produce(): Payload", "produce(): Missing"),
    );
    assert!(!labels(&mut ws, "InferredUse.java", "model.getValue").contains("getValue"));
    assert!(!labels(&mut ws, "InferredUse.java", "model.setValue").contains("setValue"));
    assert!(goto(&mut ws, "InferredUse.java", "model.getValue()", "getValue").is_empty());
    ws.change("Provider.kt", provider.into());
    ws.change(
        "Inferred.kt",
        model.replace("var value = produce()", "var value = value"),
    );
    assert!(!labels(&mut ws, "InferredUse.java", "model.getValue").contains("getValue"));
    assert!(!labels(&mut ws, "InferredUse.java", "model.setValue").contains("setValue"));
    ws.change("Inferred.kt", model.into());
    assert!(labels(&mut ws, "InferredUse.java", "model.getValue").contains("getValue"));
    assert!(labels(&mut ws, "InferredUse.java", "model.setValue").contains("setValue"));
    ws.open(
        "Layer.kt",
        "package probe\nclass Layer { val value = produce() }\n".into(),
    );
    ws.change(
        "Inferred.kt",
        model.replace("var value = produce()", "var value = Layer().value"),
    );
    assert_eq!(
        goto(
            &mut ws,
            "InferredUse.java",
            "model.getValue().payloadOnly()",
            "payloadOnly"
        ),
        vec![target(&ws, "Provider.kt", "payloadOnly")]
    );
    // A two-file cycle must withdraw both accessors and recover when the initializer is restored.
    ws.change(
        "Layer.kt",
        "package probe\nclass Layer { val value = Model().value }\n".into(),
    );
    assert!(!labels(&mut ws, "InferredUse.java", "model.getValue").contains("getValue"));
    assert!(!labels(&mut ws, "InferredUse.java", "model.setValue").contains("setValue"));
    ws.change(
        "Layer.kt",
        "package probe\nclass Layer { val value = produce() }\n".into(),
    );
    assert_eq!(
        goto(
            &mut ws,
            "InferredUse.java",
            "model.getValue().payloadOnly()",
            "payloadOnly"
        ),
        vec![target(&ws, "Provider.kt", "payloadOnly")]
    );
}

#[test]
fn inferred_accessors_hydrate_closed_source_chains_and_follow_unsaved_middle_edits() {
    let directory = std::env::temp_dir().join(format!(
        "ktlsp-inferred-closed-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let key = |name: &str| directory.join(name).to_string_lossy().replace('\\', "/");
    let a = "package probe\nclass A { var value = B().value }\n";
    let b = "package probe\nclass B { val value = C().value }\n";
    let c = "package probe\nclass Payload { fun payloadOnly(): Int = 1 }\nclass Other { fun otherOnly(): Int = 2 }\nclass C { val value = produce() }\nfun produce(): Payload = Payload()\n";
    let mut ws = Workspace::new();
    for (name, source) in [("A.kt", a), ("B.kt", b), ("C.kt", c)] {
        std::fs::write(key(name), source).unwrap();
        // Dependency/cache facts arrive without an open document or registered Salsa source input.
        let tree = ktlsp::parser::KotlinParser::new().parse(source);
        let symbols = ktcore::indexer::extract_symbols(&tree, source, "probe");
        let facts = ktcore::jvm::source_facts(&key(name), &tree, source, &symbols);
        ws.index
            .replace_file(&key(name), symbols, ktlsp::index::Tier::Volatile);
        ws.index
            .replace_jvm_source(&key(name), facts, ktlsp::index::Tier::Volatile);
    }
    ws.bump_index_revision();
    let caller = "package probe; class Use { void use(A a) { a.getValue().payloadOnly(); a.getValue().otherOnly(); a.setValue(null); } }";
    ws.open(key("Use.java"), caller.into());
    assert_eq!(
        goto(
            &mut ws,
            &key("Use.java"),
            "a.getValue().payloadOnly()",
            "payloadOnly"
        ),
        vec![target(&ws, &key("C.kt"), "payloadOnly")]
    );
    assert!(labels(&mut ws, &key("Use.java"), "a.setValue").contains("setValue"));
    ws.open(key("B.kt"), b.replace("C().value", "Other()"));
    assert_eq!(
        goto(
            &mut ws,
            &key("Use.java"),
            "a.getValue().otherOnly()",
            "otherOnly"
        ),
        vec![target(&ws, &key("C.kt"), "otherOnly")]
    );
    assert!(goto(
        &mut ws,
        &key("Use.java"),
        "a.getValue().payloadOnly()",
        "payloadOnly"
    )
    .is_empty());
    ws.change(&key("B.kt"), b.replace("C().value", "A().value"));
    assert!(!labels(&mut ws, &key("Use.java"), "a.getValue").contains("getValue"));
    assert!(!labels(&mut ws, &key("Use.java"), "a.setValue").contains("setValue"));
    ws.change(&key("B.kt"), b.into());
    assert_eq!(
        goto(
            &mut ws,
            &key("Use.java"),
            "a.getValue().payloadOnly()",
            "payloadOnly"
        ),
        vec![target(&ws, &key("C.kt"), "payloadOnly")]
    );
    drop(ws);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn java_constructor_string_getter_resolves_java_string_members() {
    let mut ws = Workspace::new();
    ws.open(
        "Text.kt",
        "package probe\nclass Text(val text: String)\n".into(),
    );
    ws.open(
        "String.java",
        "package java.lang; public final class String { public int length() { return 0; } }".into(),
    );
    ws.open(
        "Other.java",
        "package probe; public class Other { public int length() { return 1; } }".into(),
    );
    let caller =
        "package probe; class Use { int use(Text value) { return value.getText().length(); } }";
    ws.open("TextUse.java", caller.into());
    assert_eq!(
        goto(
            &mut ws,
            "TextUse.java",
            "value.getText().length()",
            "length"
        ),
        vec![target(&ws, "String.java", "length")]
    );
}

#[test]
fn kotlin_constructor_properties_do_not_expose_java_getters_when_compiled_calls_are_possible() {
    use ktlsp::index::{Index, InferenceIndex, LookupEntries};
    struct CompiledPossible<'a>(&'a Index);
    impl InferenceIndex for CompiledPossible<'_> {
        fn lookup_by_name(&self, name: &str) -> LookupEntries<'_> {
            LookupEntries::All(self.0.lookup_by_name(name))
        }
        fn members_of(&self, name: &str) -> LookupEntries<'_> {
            LookupEntries::All(self.0.members_of(name))
        }
        fn extensions_for(&self, name: &str) -> LookupEntries<'_> {
            LookupEntries::All(self.0.extensions_for(name))
        }
        fn jvm_sources(&self, owner: &str) -> Vec<&ktcore::jvm::SourceFile> {
            self.0.jvm_sources(owner)
        }
    }
    let mut ws = Workspace::new();
    ws.open(
        "Box.kt",
        "package probe\nclass Box(val value: Int)\n".into(),
    );
    let source = "package probe\nfun use(box: Box) = box.getValue()\n";
    ws.open("GetterUse.kt", source.into());
    let index = CompiledPossible(&ws.index);
    assert!(index.may_have_compiled_callables());
    assert_eq!(
        ktcore::jvm::member_candidates(
            &index,
            &Type::class("Box", Some("probe".into())),
            "getValue"
        )
        .len(),
        1
    );
    let tree = KotlinParser::new().parse(source);
    let completion = ktlsp::semantic_query::member_completion_query_for_type(
        &index,
        ktlsp::language::SourceLanguage::Kotlin,
        &tree,
        source,
        Type::class("Box", Some("probe".into())),
        ktcore::jvm::ReceiverKind::Value,
        "get".into(),
        100,
    )
    .unwrap();
    assert!(!completion
        .candidates
        .iter()
        .any(|candidate| candidate.label == "getValue"));
    assert!(goto(&mut ws, "GetterUse.kt", "box.getValue()", "getValue").is_empty());
}

#[test]
fn kotlin_super_completion_offers_superclass_member() {
    let mut ws = workspace();
    let got = labels(&mut ws, "Drill.kt", "super.get");
    assert!(
        got.contains("getBreakSpeed"),
        "superclass completion missing: {got:?}"
    );
}

#[test]
fn super_known_class_members_above_missing_ancestor() {
    for case in [
        "known",
        "nearest",
        "abstract-nearest",
        "unknown-sibling",
        "class-sibling",
        "between",
        "arguments",
        "generic",
        "vararg",
        "overload",
        "competing",
    ] {
        let mut ws = Workspace::new();
        let method = match case {
            "arguments" => "protected float getBreakSpeed(int value) { return 1; }",
            "generic" => "protected <T> float getBreakSpeed() { return 1; }",
            "vararg" => "protected float getBreakSpeed(Object... values) { return 1; }",
            "overload" => {
                "protected float getBreakSpeed() { return 1; } protected float getBreakSpeed(int value) { return 2; }"
            }
            _ => "protected float getBreakSpeed() { return 1; }",
        };
        ws.open(
            "Base.java",
            format!("package probe; public class Base extends Missing {{ {method} }}"),
        );
        ws.open(
            "Mid.java",
            format!(
                "package probe; public abstract class Mid extends {} {{ {} }}",
                if case == "between" {
                    "Unavailable"
                } else {
                    "Base"
                },
                match case {
                    "nearest" => "protected float getBreakSpeed() { return 2; }",
                    "abstract-nearest" => "protected abstract float getBreakSpeed();",
                    _ => "",
                }
            ),
        );
        match case {
            "unknown-sibling" => {}
            "class-sibling" => ws.open("Other.kt", "package probe\nopen class Other".into()),
            "competing" => ws.open(
                "Other.kt",
                "package probe\ninterface Other { fun getBreakSpeed(): Float = 2f }".into(),
            ),
            _ => ws.open("Other.kt", "package probe\ninterface Other".into()),
        }
        let source =
            "package probe\nclass Use : Mid(), Other { fun use() = super.getBreakSpeed() }";
        ws.open("Use.kt", source.into());
        let at = source.find("super.getBreakSpeed").unwrap() + "super.getBreakSpeed".len();
        let query = ws.completion_query("Use.kt", at).unwrap();
        assert_eq!(
            query
                .candidates
                .iter()
                .any(|candidate| candidate.label == "getBreakSpeed"),
            matches!(case, "known" | "nearest" | "arguments" | "overload"),
            "{case}: {:?}",
            query.candidates
        );
        assert_eq!(
            query.status_label(),
            "unknown",
            "partial hierarchy stays incomplete"
        );
        let exact = matches!(case, "known" | "nearest");
        let expected = if exact {
            vec![target(
                &ws,
                if case == "nearest" {
                    "Mid.java"
                } else {
                    "Base.java"
                },
                "getBreakSpeed",
            )]
        } else {
            Vec::new()
        };
        assert_eq!(
            goto(&mut ws, "Use.kt", "super.getBreakSpeed()", "getBreakSpeed"),
            expected,
            "{case}"
        );
        assert_eq!(
            infer_expression(&ws, "Use.kt", "super.getBreakSpeed()"),
            if exact {
                Type::class("Float", Some("kotlin".into()))
            } else {
                Type::Unknown
            },
            "{case}"
        );
        if case == "known" {
            ws.change("Base.java", "package probe; public class Base extends Missing { protected float renamed() { return 1; } }".into());
            assert!(!labels(&mut ws, "Use.kt", "super.get").contains("getBreakSpeed"));
            ws.change(
                "Base.java",
                format!("package probe; public class Base extends Missing {{ {method} }}"),
            );
            assert!(labels(&mut ws, "Use.kt", "super.get").contains("getBreakSpeed"));
        }
    }
}

#[test]
fn kotlin_super_goto_selects_superclass_not_override_or_unrelated_member() {
    let mut ws = workspace();
    let expected = target(&ws, "Base.java", "getBreakSpeed");
    assert_eq!(
        goto(
            &mut ws,
            "Drill.kt",
            "super.getBreakSpeed()",
            "getBreakSpeed"
        ),
        vec![expected]
    );
}

#[test]
fn bare_implicit_extension_result_offers_its_java_members() {
    let mut ws = workspace();
    let got = labels(&mut ws, "Drill.kt", "holderLookup(key).get");
    assert!(
        got.contains("getOrThrow"),
        "implicit extension result completion missing: {got:?}"
    );
}

#[test]
fn bare_implicit_extension_chain_goto_selects_its_java_member() {
    let mut ws = workspace();
    let expected = target(&ws, "Lookup.java", "getOrThrow");
    assert_eq!(
        goto(
            &mut ws,
            "Drill.kt",
            "return holderLookup(key).getOrThrow(key)",
            "getOrThrow"
        ),
        vec![expected]
    );
}

#[test]
fn explicit_extension_free_function_and_typed_parameter_keep_useful_results() {
    let mut ws = workspace();
    let expected = target(&ws, "Lookup.java", "getOrThrow");
    for expression in [
        "this.holderLookup(key).getOrThrow(key)",
        "holderFree(key).getOrThrow(key)",
        "lookup.getOrThrow(key)",
    ] {
        let prefix = &expression[..expression.find("getOrThrow").unwrap() + 3];
        let got = labels(&mut ws, "Drill.kt", prefix);
        assert!(got.contains("getOrThrow"), "control {expression}: {got:?}");
        assert_eq!(
            goto(&mut ws, "Drill.kt", expression, "getOrThrow"),
            vec![expected.clone()]
        );
    }
}

#[test]
fn explicit_extension_has_a_known_result_head_control() {
    let ws = workspace();
    let ty = infer_expression(&ws, "Drill.kt", "this.holderLookup(key)");
    assert_eq!(ty.name(), Some("Lookup"));
    assert_eq!(ty.package(), Some("probe"));
}

fn add_payload_members(ws: &mut Workspace) {
    ws.change(
        "Extensions.kt",
        EXTENSIONS.replace(
            "class Payload {}",
            "class Payload {\n    fun payloadOnly(): Payload = this\n}",
        ),
    );
    ws.open(
        "OtherPayload.kt",
        "package probe\nclass OtherPayload {\n    fun otherOnly(): OtherPayload = this\n}\n"
            .to_string(),
    );
}

#[test]
fn implicit_and_explicit_extension_arguments_preserve_generic_results() {
    let mut ws = workspace();
    add_payload_members(&mut ws);
    let source = "package probe\nclass MoreDrill : Base() {\n    fun bare(key: Key<Payload>): Payload = holderLookup(key).getOrThrow(key).payloadOnly()\n    fun explicit(key: Key<Payload>): Payload = this.holderLookup(key).getOrThrow(key).payloadOnly()\n}\n";
    ws.open("MoreDrill.kt", source.to_string());
    for call in ["holderLookup(key)", "this.holderLookup(key)"] {
        let result = infer_expression(&ws, "MoreDrill.kt", call);
        assert_eq!(result.name(), Some("Lookup"));
        assert_eq!(
            result.args(),
            [Type::class("Payload", Some("probe".to_string()))]
        );
        let expression = format!("{call}.getOrThrow(key)");
        assert_eq!(
            infer_expression(&ws, "MoreDrill.kt", &expression),
            Type::class("Payload", Some("probe".to_string()))
        );
        assert!(
            labels(&mut ws, "MoreDrill.kt", &format!("{expression}.pay")).contains("payloadOnly")
        );
    }
}

#[test]
fn extension_function_and_receiver_lambda_supply_implicit_receivers() {
    let mut ws = workspace();
    add_payload_members(&mut ws);
    ws.open("ReceiverUse.kt", "package probe\nfun Base.byExtension(key: Key<Payload>): Payload = holderLookup(key).getOrThrow(key).payloadOnly()\nfun inLambda(base: Base, key: Key<Payload>) {\n    base.apply { holderLookup(key).getOrThrow(key).payloadOnly() }\n}\n".to_string());
    let expected = target(&ws, "Lookup.java", "getOrThrow");
    let source = ws.doc_text("ReceiverUse.kt").unwrap();
    for (start, _) in source.match_indices("holderLookup(key).getOrThrow(key)") {
        assert_eq!(
            ws.goto_definition("ReceiverUse.kt", start + "holderLookup(key).".len()),
            vec![expected.clone()]
        );
        let offset = start + "holderLookup(key).getOrThrow(key).pay".len();
        let completion = ws.complete("ReceiverUse.kt", offset, true).unwrap();
        assert!(completion
            .items
            .iter()
            .any(|item| item.label == "payloadOnly"));
    }
}

#[test]
fn generic_super_receiver_preserves_arguments_and_dispatch_for_both_languages() {
    let mut ws = workspace();
    add_payload_members(&mut ws);
    ws.open(
        "GenericBase.java",
        "package probe; public class GenericBase<T> { public T inherited() { return null; } }"
            .to_string(),
    );
    ws.open(
        "OtherGeneric.java",
        "package probe; public class OtherGeneric<T> { public T inherited() { return null; } }"
            .to_string(),
    );
    ws.open(
        "Marker.java",
        "package probe; public interface Marker {}".to_string(),
    );
    for (file, source) in [
        (
            "GenericUse.kt",
            "package probe\nclass GenericUse : GenericBase<Payload>() {\n    fun run(): Payload = super.inherited().payloadOnly()\n}\n",
        ),
        (
            "JavaGenericUse.java",
            "package probe; class JavaGenericUse extends GenericBase<Payload> implements Marker { Payload run() { return super.inherited().payloadOnly(); } }",
        ),
    ] {
        ws.open(file, source.to_string());
        assert!(
            labels(&mut ws, file, "super.inherited().pay").contains("payloadOnly"),
            "{file}"
        );
        let expected = target(&ws, "GenericBase.java", "inherited");
        assert_eq!(
            goto(&mut ws, file, "super.inherited()", "inherited"),
            vec![expected]
        );
    }
    assert_eq!(
        infer_expression(&ws, "GenericUse.kt", "super.inherited()"),
        Type::class("Payload", Some("probe".to_string()))
    );
}

#[test]
fn qualified_and_outer_super_receivers_keep_declared_owner() {
    let mut ws = workspace();
    ws.open(
        "Named.java",
        "package probe; public interface Named { default float getBreakSpeed() { return 3f; } }"
            .to_string(),
    );
    let source = "package probe\nclass Outer : Base(), Named {\n    override fun getBreakSpeed(): Float = super<Base>.getBreakSpeed()\n    fun named(): Float = super<Named>.getBreakSpeed()\n    inner class Inner {\n        fun outer(): Float = super@Outer.getBreakSpeed()\n        fun qualifiedOuter(): Float = super<Base>@Outer.getBreakSpeed()\n    }\n}\n";
    ws.open("Outer.kt", source.to_string());
    for (expression, file) in [
        ("super<Base>.getBreakSpeed()", "Base.java"),
        ("super<Named>.getBreakSpeed()", "Named.java"),
        ("super<Base>@Outer.getBreakSpeed()", "Base.java"),
    ] {
        let expected = target(&ws, file, "getBreakSpeed");
        assert_eq!(
            goto(&mut ws, "Outer.kt", expression, "getBreakSpeed"),
            vec![expected]
        );
    }
    // The unqualified outer receiver has two direct supertypes. Without an intersection/member
    // dispatch model this remains Unknown; it must not choose the first unrelated declaration.
    assert_eq!(
        infer_expression(&ws, "Outer.kt", "super@Outer"),
        Type::Unknown
    );
}

#[test]
fn missing_or_ambiguous_super_receivers_never_use_global_member_homonyms() {
    let mut ws = workspace();
    ws.open(
        "Named.java",
        "package probe; public interface Named { default float getBreakSpeed() { return 3f; } }"
            .to_string(),
    );
    for source in [
        "package probe\nclass Use : Missing() {\n    fun run() = super.getBreakSpeed()\n}\n",
        "package probe\nclass Use : Base(), Named {\n    override fun getBreakSpeed(): Float = super.getBreakSpeed()\n}\n",
        "package probe\nclass Use : Base() {\n    fun run() = super<Other>.getBreakSpeed()\n}\n",
    ] {
        ws.open("Use.kt", source.to_string());
        let expression = if source.contains("super<Other>") {
            "super<Other>"
        } else {
            "super"
        };
        assert_eq!(infer_expression(&ws, "Use.kt", expression), Type::Unknown);
        assert!(
            goto(
                &mut ws,
                "Use.kt",
                &format!("{expression}.getBreakSpeed()"),
                "getBreakSpeed"
            )
            .is_empty()
        );
        assert!(labels(&mut ws, "Use.kt", &format!("{expression}.get")).is_empty());
    }
}

#[test]
fn unsaved_superclass_edit_replaces_receiver_binding() {
    let mut ws = workspace();
    let source = "package probe\nclass SuperUse : Base() {\n    override fun getBreakSpeed(): Float = super.getBreakSpeed()\n}\n";
    ws.open("SuperUse.kt", source.to_string());
    for (base, file) in [
        ("Base", "Base.java"),
        ("Other", "Other.java"),
        ("Base", "Base.java"),
    ] {
        ws.change(
            "SuperUse.kt",
            source.replace("Base()", &format!("{base}()")),
        );
        let expected = target(&ws, file, "getBreakSpeed");
        assert_eq!(
            goto(
                &mut ws,
                "SuperUse.kt",
                "super.getBreakSpeed()",
                "getBreakSpeed"
            ),
            vec![expected]
        );
    }
}

fn add_super_interfaces(ws: &mut Workspace) {
    ws.open("SuperInterfaces.kt", "package probe\ninterface Marker {\n    fun marker(): Float\n}\ninterface ParentMarker : Marker {}\nclass MarkerImpl : Marker {\n    override fun marker(): Float = 3f\n}\n".to_string());
}

#[test]
fn kotlin_super_selects_unique_member_provider_with_distinct_interfaces() {
    let mut ws = workspace();
    add_super_interfaces(&mut ws);
    let expected = target(&ws, "Base.java", "getBreakSpeed");
    for inheritance in [
        "Base(), Marker",
        "Base(), ParentMarker",
        "Base(), Marker by MarkerImpl()",
    ] {
        let source = format!(
            "package probe\nclass Multiple : {inheritance} {{\n    override fun marker(): Float = 3f\n    override fun getBreakSpeed(): Float = super.getBreakSpeed()\n}}\n"
        );
        ws.open("Multiple.kt", source.clone());
        assert!(
            labels(&mut ws, "Multiple.kt", "super.getB").contains("getBreakSpeed"),
            "{inheritance}"
        );
        assert_eq!(
            goto(
                &mut ws,
                "Multiple.kt",
                "super.getBreakSpeed()",
                "getBreakSpeed"
            ),
            vec![expected.clone()],
            "{inheritance}"
        );
        assert_eq!(
            infer_expression(&ws, "Multiple.kt", "super").name(),
            Some("Base"),
            "{inheritance}"
        );
        ws.change(
            "Multiple.kt",
            source.replace("super.getBreakSpeed()", "super.getB"),
        );
        assert!(
            labels(&mut ws, "Multiple.kt", "super.getB").contains("getBreakSpeed"),
            "incomplete {inheritance}"
        );
    }
}

#[test]
fn kotlin_super_with_distinct_interface_preserves_generic_arguments() {
    let mut ws = workspace();
    add_super_interfaces(&mut ws);
    add_payload_members(&mut ws);
    ws.open(
        "GenericBase.java",
        "package probe; public class GenericBase<T> { public T inherited() { return null; } }"
            .to_string(),
    );
    ws.open("Multiple.kt", "package probe\nclass Multiple : GenericBase<Payload>(), Marker by MarkerImpl() {\n    fun run(): Payload = super.inherited().payloadOnly()\n}\n".to_string());
    assert_eq!(
        infer_expression(&ws, "Multiple.kt", "super.inherited()"),
        Type::class("Payload", Some("probe".to_string()))
    );
    assert!(labels(&mut ws, "Multiple.kt", "super.inherited().pay").contains("payloadOnly"));
    let expected = target(&ws, "GenericBase.java", "inherited");
    assert_eq!(
        goto(&mut ws, "Multiple.kt", "super.inherited()", "inherited"),
        vec![expected]
    );
}

#[test]
fn kotlin_super_provider_changes_invalidate_positive_and_ambiguous_queries() {
    let mut ws = workspace();
    let source = "package probe\nclass Multiple : Base(), Marker {\n    override fun getBreakSpeed(): Float = super.getBreakSpeed()\n}\n";
    ws.open("Multiple.kt", source.to_string());
    for marker in [
        "package probe\ninterface Marker {}\n",
        "package probe\ninterface Marker {\n    fun getBreakSpeed(): Float = 3f\n}\n",
        "package probe\ninterface Marker : Missing {}\n",
        "package probe\ninterface Marker {}\n",
    ] {
        ws.open("SuperInterfaces.kt", marker.to_string());
        let expected = if marker.contains("getBreakSpeed") || marker.contains("Missing") {
            Vec::new()
        } else {
            vec![target(&ws, "Base.java", "getBreakSpeed")]
        };
        assert_eq!(
            goto(
                &mut ws,
                "Multiple.kt",
                "super.getBreakSpeed()",
                "getBreakSpeed"
            ),
            expected,
            "{marker}"
        );
        assert_eq!(
            labels(&mut ws, "Multiple.kt", "super.getB").contains("getBreakSpeed"),
            !expected.is_empty(),
            "{marker}"
        );
    }
}

#[test]
fn super_candidates_blank_keeps_distinct_concrete_names_and_excludes_abstract_names() {
    let mut ws = workspace();
    add_super_interfaces(&mut ws);
    ws.open(
        "Default.kt",
        "package probe\ninterface Default { fun defaultOnly(): Float = 2f }\n".into(),
    );
    ws.open("Multiple.kt", "package probe\nclass Multiple : Base(), Marker by MarkerImpl(), Default {\n    fun run() { super. }\n}\n".into());
    let got = labels(&mut ws, "Multiple.kt", "super.");
    assert!(got.contains("getBreakSpeed"), "{got:?}");
    assert!(got.contains("defaultOnly"), "{got:?}");
    assert!(!got.contains("marker"), "abstract delegate member: {got:?}");
}

#[test]
fn super_candidates_same_name_abstract_provider_does_not_hide_concrete_provider() {
    let mut ws = workspace();
    ws.open(
        "Marker.kt",
        "package probe\ninterface Marker { fun getBreakSpeed(): Float }\n".into(),
    );
    ws.open("Multiple.kt", "package probe\nclass Multiple : Base(), Marker {\n    override fun getBreakSpeed(): Float = super.getBreakSpeed()\n}\n".into());
    assert!(labels(&mut ws, "Multiple.kt", "super.").contains("getBreakSpeed"));
    assert_eq!(
        goto(
            &mut ws,
            "Multiple.kt",
            "super.getBreakSpeed()",
            "getBreakSpeed"
        ),
        vec![target(&ws, "Base.java", "getBreakSpeed")]
    );
}

#[test]
fn super_candidates_filters_private_static_and_constructor_but_keeps_native() {
    let mut ws = Workspace::new();
    ws.open("Access.java", "package foreign; public class Access { public Access() {} private int privateOnly() { return 1; } public static int staticOnly() { return 2; } int packageOnly() { return 3; } protected int protectedOnly() { return 4; } public native int nativeOnly(); public int Access(int value) { return value; } }".into());
    ws.open(
        "Use.kt",
        "package probe\nimport foreign.Access\nclass Use : Access() { fun run() { super. } }\n"
            .into(),
    );
    let got = labels(&mut ws, "Use.kt", "super.");
    for excluded in ["privateOnly", "staticOnly", "packageOnly"] {
        assert!(!got.contains(excluded), "{excluded}: {got:?}");
    }
    for included in ["protectedOnly", "nativeOnly", "Access"] {
        assert!(got.contains(included), "{included}: {got:?}");
    }
    ws.change("Access.java", "package foreign; public class Access { public Access() {} protected int protectedOnly() { return 4; } }".into());
    let got = labels(&mut ws, "Use.kt", "super.");
    assert!(
        !got.contains("Access"),
        "constructor is not a super method: {got:?}"
    );
    assert!(got.contains("protectedOnly"), "{got:?}");
}

#[test]
fn super_candidates_reabstracted_override_never_resurrects_concrete_ancestor() {
    let mut ws = Workspace::new();
    ws.open("Grand.java", "package probe; public class Grand { public int hidden() { return 1; } public int useful() { return 2; } }".into());
    ws.open("Middle.java", "package probe; public abstract class Middle extends Grand { public abstract int hidden(); }".into());
    ws.open(
        "Use.kt",
        "package probe\nabstract class Use : Middle() { fun run() { super. } }\n".into(),
    );
    let got = labels(&mut ws, "Use.kt", "super.");
    assert!(got.contains("useful"), "{got:?}");
    assert!(
        !got.contains("hidden"),
        "abstract override resurrected: {got:?}"
    );
}

#[test]
fn super_candidates_same_name_concrete_overloads_are_ambiguous_across_providers() {
    let mut ws = workspace();
    add_payload_members(&mut ws);
    ws.open("GenericBase.java", "package probe; public class GenericBase<T> { public T pick(T value) { return value; } public T inherited() { return null; } }".into());
    ws.open(
        "Default.kt",
        "package probe\ninterface Default { fun pick(value: String): String = value }\n".into(),
    );
    ws.open("Use.kt", "package probe\nclass Use : GenericBase<Payload>(), Default {\n fun run(value: Payload): Payload = super<GenericBase>.pick(value)\n fun chain() = super<GenericBase>.inherited().payloadOnly()\n fun incomplete() { super. }\n}\n".into());
    let got = labels(&mut ws, "Use.kt", "super.");
    assert!(
        !got.contains("pick"),
        "different argument shapes cannot select a provider: {got:?}"
    );
    assert!(got.contains("inherited"), "{got:?}");
    assert_eq!(
        infer_expression(&ws, "Use.kt", "super<GenericBase>.pick(value)"),
        Type::class("Payload", Some("probe".into()))
    );
    assert!(
        labels(&mut ws, "Use.kt", "super<GenericBase>.inherited().pay").contains("payloadOnly")
    );
}

#[test]
fn super_candidates_unknown_override_blocks_ancestor_without_erasing_distinct_names() {
    let mut ws = Workspace::new();
    ws.open("Grand.java", "package probe; public class Grand { public int pick(String value) { return 1; } public int useful() { return 2; } }".into());
    ws.open("Middle.java", "package probe; public abstract class Middle extends Grand { public abstract int pick(Missing value); }".into());
    ws.open(
        "Use.kt",
        "package probe\nabstract class Use : Middle() { fun run() { super. } }\n".into(),
    );
    let got = labels(&mut ws, "Use.kt", "super.");
    assert!(!got.contains("pick"), "unknown related override: {got:?}");
    assert!(got.contains("useful"), "unrelated known name: {got:?}");
    ws.change("Middle.java", "package probe; public abstract class Middle extends Grand { public abstract int pick(int value); }".into());
    assert!(
        labels(&mut ws, "Use.kt", "super.").contains("pick"),
        "known different overload must not hide ancestor"
    );
}

#[test]
fn super_candidates_source_external_is_concrete_while_internal_access_is_unknown() {
    let mut ws = Workspace::new();
    ws.open("KotlinBase.kt", "package probe\nopen class KotlinBase {\n external open fun externalOnly(): Int\n private fun privateOnly(): Int = 1\n internal fun internalOnly(): Int = 2\n open fun useful(): Int = 3\n}\n".into());
    ws.open(
        "Use.kt",
        "package probe\nclass Use : KotlinBase() { fun run() { super. } }\n".into(),
    );
    let got = labels(&mut ws, "Use.kt", "super.");
    for name in ["externalOnly", "useful"] {
        assert!(got.contains(name), "{name}: {got:?}");
    }
    for name in ["privateOnly", "internalOnly"] {
        assert!(!got.contains(name), "{name}: {got:?}");
    }
    ws.change(
        "Use.kt",
        "package probe\nclass Use : KotlinBase() { fun run() { super.ext } }\n".into(),
    );
    assert!(labels(&mut ws, "Use.kt", "super.ext").contains("externalOnly"));
}

#[test]
fn super_candidates_recovery_and_unsaved_body_preserve_later_generic_declarations() {
    let mut ws = Workspace::new();
    let source = "package probe\nclass Payload\ninterface Marker { fun marker(): Int }\nclass MarkerImpl : Marker { override fun marker(): Int = 1 }\nclass Use : Later<Payload>(), Marker by MarkerImpl() {\n fun incomplete() { super. }\n}\nopen class Later<T> { open fun inherited(): T = TODO() }\n";
    ws.open("Use.kt", source.into());
    assert!(labels(&mut ws, "Use.kt", "super.").contains("inherited"));
    ws.change(
        "Use.kt",
        source.replace(
            "open class Later<T> { open fun inherited(): T = TODO() }",
            "abstract class Later<T> { abstract fun inherited(): T }",
        ),
    );
    assert!(!labels(&mut ws, "Use.kt", "super.").contains("inherited"));
    ws.change("Use.kt", source.into());
    assert!(labels(&mut ws, "Use.kt", "super.").contains("inherited"));
}

#[test]
fn super_candidates_do_not_treat_unknown_private_ancestors_as_override_competitors() {
    let mut ws = Workspace::new();
    ws.open(
        "Grand.java",
        "package probe; public class Grand { private int pick(Missing value) { return 1; } }"
            .into(),
    );
    ws.open("Middle.java", "package probe; public class Middle extends Grand { public int pick(int value) { return value; } }".into());
    ws.open(
        "Use.kt",
        "package probe\nclass Use : Middle() { fun run() { super. } }\n".into(),
    );
    assert!(labels(&mut ws, "Use.kt", "super.").contains("pick"));
}

#[test]
fn super_candidates_invalid_mixed_static_override_is_unknown_without_resurrection() {
    let mut ws = Workspace::new();
    ws.open("Grand.java", "package probe; public class Grand { public int hidden() { return 1; } public int useful() { return 2; } }".into());
    ws.open("Middle.java", "package probe; public class Middle extends Grand { public static int hidden() { return 3; } }".into());
    ws.open(
        "Use.kt",
        "package probe\nclass Use : Middle() { fun run() { super. } }\n".into(),
    );
    let got = labels(&mut ws, "Use.kt", "super.");
    assert!(
        !got.contains("hidden"),
        "invalid instance/static override: {got:?}"
    );
    assert!(got.contains("useful"), "{got:?}");
}

#[test]
fn super_candidates_kotlin_reabstracted_override_blocks_concrete_ancestor() {
    let mut ws = Workspace::new();
    ws.open("Ancestors.kt", "package probe\nopen class Grand { open fun hidden(): Int = 1\n fun useful(): Int = 2 }\nabstract class Middle : Grand() { abstract override fun hidden(): Int }\n".into());
    ws.open(
        "Use.kt",
        "package probe\nabstract class Use : Middle() { fun run() { super. } }\n".into(),
    );
    let got = labels(&mut ws, "Use.kt", "super.");
    assert!(!got.contains("hidden"), "{got:?}");
    assert!(got.contains("useful"), "{got:?}");
}

#[test]
fn super_candidates_package_access_uses_kotlin_lexical_package_through_foreign_middle() {
    // JDK21 javac rejects the foreign-middle calls, while Kotlin2.4 compiles and
    // executes all four in package a. The super-form query here is Kotlin source.
    let mut ws = Workspace::new();
    ws.open("Base.java", "package a; public class Base { int packageOnly() { return 7; } public int useful() { return 8; } }".into());
    ws.open(
        "Same.java",
        "package a; public class Same extends Base {}".into(),
    );
    ws.open(
        "Foreign.java",
        "package b; public class Foreign extends a.Base {}".into(),
    );
    ws.open(
        "Again.java",
        "package a; public class Again extends b.Foreign {}".into(),
    );
    for parent in ["a.Base", "a.Same", "b.Foreign", "a.Again"] {
        for (package, allowed) in [("a", true), ("c", false)] {
            ws.open(
                "Use.kt",
                format!("package {package}\nclass Use : {parent}() {{ fun run() {{ super. }} }}\n"),
            );
            let got = labels(&mut ws, "Use.kt", "super.");
            assert_eq!(
                got.contains("packageOnly"),
                allowed,
                "{package}: {parent}: {got:?}"
            );
            assert!(got.contains("useful"), "{package}: {parent}: {got:?}");
        }
    }
}

#[test]
fn super_candidates_recovery_preserves_package_and_nested_owner_identity() {
    let mut ws = Workspace::new();
    ws.open(
        "Foreign.kt",
        "package other\nclass Outer { open class Later<T> { fun foreignDecoy(): Int = 1 } }\n"
            .into(),
    );
    ws.open(
        "OtherOwner.kt",
        "package probe\nclass Different { open class Later<T> { fun nestedDecoy(): Int = 2 } }\n"
            .into(),
    );
    let source = "package probe\nclass Payload\nclass Use : Outer.Later<Payload>() { fun run() { super. } }\nclass Outer { open class Later<T> { open fun inherited(): T = TODO() } }\n";
    ws.open("Use.kt", source.into());
    let got = labels(&mut ws, "Use.kt", "super.");
    assert!(got.contains("inherited"), "{got:?}");
    for decoy in ["foreignDecoy", "nestedDecoy"] {
        assert!(!got.contains(decoy), "{decoy}: {got:?}");
    }
}

#[test]
fn super_candidates_preserves_concrete_property_type_target_and_result_chain() {
    for base in [
        "package probe\nopen class Base { open val value: Payload = Payload() }\n",
        "package probe\nopen class Base { open val value: Payload get() = Payload() }\n",
        "package probe\nopen class Base(open val value: Payload = Payload())\n",
        "package probe\ninterface Base { val value: Payload get() = Payload() }\n",
    ] {
        let mut ws = Workspace::new();
        ws.open(
            "Payload.kt",
            "package probe\nclass Payload { fun payloadOnly(): Int = 1 }\n".into(),
        );
        ws.open("Base.kt", base.into());
        let constructor = if base.contains("interface") { "" } else { "()" };
        let source = format!(
            "package probe\nclass Use : Base{constructor} {{ override val value: Payload get() = super.value\n fun chain() = super.value.payloadOnly()\n fun incomplete() {{ super. }} }}\n"
        );
        ws.open("Use.kt", source);
        assert_eq!(
            infer_expression(&ws, "Use.kt", "super.value"),
            Type::class("Payload", Some("probe".into())),
            "{base}"
        );
        let entry = ws
            .index
            .lookup_by_name("value")
            .iter()
            .find(|entry| entry.path.as_ref() == "Base.kt")
            .unwrap()
            .clone();
        let expected = Def {
            file: "Base.kt".into(),
            start_byte: entry.sym.start_byte,
            end_byte: entry.sym.end_byte,
        };
        assert_eq!(
            goto(&mut ws, "Use.kt", "super.value", "value"),
            vec![expected],
            "{base}"
        );
        assert!(
            labels(&mut ws, "Use.kt", "super.").contains("value"),
            "{base}"
        );
        assert!(
            labels(&mut ws, "Use.kt", "super.value.pay").contains("payloadOnly"),
            "{base}"
        );
    }
}

#[test]
fn super_candidates_preserves_public_protected_fields_and_excludes_private_static_fields() {
    let mut ws = Workspace::new();
    ws.open(
        "Payload.kt",
        "package probe\nclass Payload { fun payloadOnly(): Int = 1 }\n".into(),
    );
    ws.open("Base.java", "package probe; public class Base { public Payload value; protected Payload protectedValue; private Payload privateValue; public static Payload staticValue; }".into());
    ws.open("Use.kt", "package other\nimport probe.Base\nclass Use : Base() { fun exact() = super.value\n fun chain() = super.protectedValue.payloadOnly()\n fun incomplete() { super. } }\n".into());
    let got = labels(&mut ws, "Use.kt", "super.");
    for name in ["value", "protectedValue"] {
        assert!(got.contains(name), "{name}: {got:?}");
    }
    for name in ["privateValue", "staticValue"] {
        assert!(!got.contains(name), "{name}: {got:?}");
    }
    assert_eq!(
        infer_expression(&ws, "Use.kt", "super.value"),
        Type::class("Payload", Some("probe".into()))
    );
    assert!(labels(&mut ws, "Use.kt", "super.protectedValue.pay").contains("payloadOnly"));
}

#[test]
fn super_candidates_abstract_property_blocks_ancestor_before_availability() {
    let mut ws = Workspace::new();
    ws.open(
        "Payload.kt",
        "package probe\nclass Payload { fun payloadOnly(): Int = 1 }\n".into(),
    );
    let concrete = "package probe\nopen class Grand { open val value: Payload = Payload()\n val useful: Payload = Payload() }\nabstract class Base : Grand() { abstract override val value: Payload }\n";
    ws.open("Base.kt", concrete.into());
    ws.open("Use.kt", "package probe\nabstract class Use : Base() { fun exact() = super.value\n fun chain() = super.value.payloadOnly()\n fun incomplete() { super. } }\n".into());
    let got = labels(&mut ws, "Use.kt", "super.");
    assert!(
        !got.contains("value"),
        "abstract override resurrected: {got:?}"
    );
    assert!(
        got.contains("useful"),
        "distinct concrete value lost: {got:?}"
    );
    assert_eq!(
        infer_expression(&ws, "Use.kt", "super.value"),
        Type::Unknown
    );
    assert!(!labels(&mut ws, "Use.kt", "super.value.pay").contains("payloadOnly"));
    ws.change(
        "Base.kt",
        concrete.replace(
            "abstract override val value: Payload",
            "override val value: Payload get() = Payload()",
        ),
    );
    assert!(labels(&mut ws, "Use.kt", "super.").contains("value"));
    ws.change("Base.kt", concrete.into());
    assert!(!labels(&mut ws, "Use.kt", "super.").contains("value"));
}

#[test]
fn super_candidates_unsaved_getter_body_keeps_read_header_and_abstract_interface_is_excluded() {
    let mut ws = Workspace::new();
    ws.open("Payload.kt", "package probe\nclass Payload\n".into());
    ws.open("Base.kt", "package probe\nopen class Base { open val value: Payload get() = Payload() }\ninterface AbstractValue { val abstractValue: Payload }\n".into());
    ws.open("Use.kt", "package probe\nabstract class Use : Base(), AbstractValue { fun incomplete() { super. } }\n".into());
    let got = labels(&mut ws, "Use.kt", "super.");
    assert!(got.contains("value"), "{got:?}");
    assert!(!got.contains("abstractValue"), "{got:?}");
    ws.change("Base.kt", "package probe\nopen class Base { open val value: Payload get() { return } }\ninterface AbstractValue { val abstractValue: Payload }\n".into());
    assert!(labels(&mut ws, "Use.kt", "super.").contains("value"));
}

#[test]
fn super_candidates_default_getter_is_independent_of_class_initialization_style() {
    for base in [
        "package probe\nopen class Base<T> { open val value: T\n init { value = TODO() } }\n",
        "package probe\nopen class Base<T : Any> { open lateinit var value: T\n init { value = TODO() } }\n",
        "package probe\nopen class Base<T> { open val value: T by lazy { TODO() } }\n",
    ] {
        let mut ws = Workspace::new();
        ws.open(
            "Payload.kt",
            "package probe\nclass Payload { fun payloadOnly(): Int = 1 }\n".into(),
        );
        ws.open("Base.kt", base.into());
        ws.open("Use.kt", "package probe\nclass Use : Base<Payload>() { fun exact() = super.value\n fun chain() = super.value.payloadOnly()\n fun incomplete() { super. } }\n".into());
        assert_eq!(
            infer_expression(&ws, "Use.kt", "super.value"),
            Type::class("Payload", Some("probe".into())),
            "{base}"
        );
        assert!(
            labels(&mut ws, "Use.kt", "super.").contains("value"),
            "{base}"
        );
        assert!(
            labels(&mut ws, "Use.kt", "super.value.pay").contains("payloadOnly"),
            "{base}"
        );
    }
}

#[test]
fn super_selection_never_readds_private_same_name_overloads() {
    let mut observed = Vec::new();
    let mut expected = Vec::new();
    for private_parameters in ["vararg ignored: Int", "ignored: Int"] {
        let mut ws = Workspace::new();
        let base = format!(
            "package probe\nopen class Base {{\n    open fun pick(): Int = 1\n    private fun pick({private_parameters}): String = \"private\"\n}}\n"
        );
        let use_source = "package probe\ninterface Marker\nclass Use : Base(), Marker {\n    fun run(): Int = super.pick()\n}\n";
        ws.open("Base.kt", base.clone());
        ws.open("Use.kt", use_source.into());
        let public = Def {
            file: "Base.kt".into(),
            start_byte: base.find("pick()").unwrap(),
            end_byte: base.find("pick()").unwrap() + "pick".len(),
        };

        let tree = KotlinParser::new().parse(use_source);
        let super_start = use_source.find("super.pick()").unwrap();
        let super_node = tree
            .root_node()
            .named_descendant_for_byte_range(super_start, super_start + "super".len())
            .unwrap();
        assert_eq!(super_node.kind(), "super_expression");
        let ctx = FileCtx::from_tree(&tree, use_source);
        let scoped = ws.index.scoped_for_file("Use.kt");
        let receivers = infer::super_receivers(&scoped, super_node, use_source, &ctx);
        let blank = ktcore::members::super_candidates(&scoped, receivers, "", &ctx);
        let blank_targets: Vec<_> = blank
            .providers
            .into_iter()
            .flat_map(|(_, entries)| entries)
            .filter(|entry| entry.sym.name == "pick")
            .map(|entry| Def {
                file: entry.path.to_string(),
                start_byte: entry.sym.start_byte,
                end_byte: entry.sym.end_byte,
            })
            .collect();
        let inferred = infer_expression(&ws, "Use.kt", "super.pick()")
            .name()
            .map(str::to_owned);
        let definitions = goto(&mut ws, "Use.kt", "super.pick()", "pick");
        observed.push((private_parameters, blank_targets, definitions, inferred));
        expected.push((
            private_parameters,
            vec![public.clone()],
            vec![public],
            Some("Int".to_string()),
        ));
    }
    assert_eq!(observed, expected);
}

#[test]
fn super_call_shape_uses_only_effective_public_overloads() {
    let mut ws = Workspace::new();
    ws.assume_index_complete_for_tests();
    ws.open("Base.kt", "package probe\nopen class Base { open fun pick(): Int = 1; private fun pick(vararg ignored: Int): String = \"private\" }\n".into());
    let source = "package probe\ninterface Marker\nclass Use : Base(), Marker { fun run() { super.pick(1) } }\n";
    ws.open("Use.kt", source.into());
    let tree = KotlinParser::new().parse(source);
    let start = source.find("super.pick(1)").unwrap();
    let call = tree
        .root_node()
        .named_descendant_for_byte_range(start, start + "super.pick(1)".len())
        .unwrap();
    assert_eq!(call.kind(), "call_expression");
    let facts = ktlsp::resolve::CompletenessFacts::complete();
    let scoped = ws.index.scoped_for_file("Use.kt");
    let shape =
        ktlsp::semantic_query::call_shape_query(&scoped, "Use.kt", &tree, source, call, &facts)
            .expect("private vararg cannot accept or suppress the public call mismatch");
    assert_eq!(shape.arg_count, 1);
    assert_eq!(shape.arities, vec![0]);
    let diagnostics =
        ktcore::indexed_diagnostics::compute(&scoped, "Use.kt", source, &tree, &facts);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("No overload of pick")),
        "{diagnostics:?}"
    );
}

#[test]
fn closed_super_goto_keeps_effective_pool_and_rejects_unique_private_fallback() {
    let directory = std::env::temp_dir().join(format!(
        "ktlsp-closed-super-private-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("Use.kt").to_string_lossy().into_owned();
    let source = "package probe\ninterface Marker\nclass Use : Base(), Marker { fun run() { super.pick(); super.hiddenOnly() } }\n";
    std::fs::write(&path, source).unwrap();
    let mut ws = Workspace::new();
    ws.open("Base.kt", "package probe\nopen class Base { open fun pick(): Int = 1; private fun pick(vararg ignored: Int): String = \"private\"; private fun hiddenOnly(): Int = 2 }\n".into());
    ws.open(&path, source.into());
    let expected = target(&ws, "Base.kt", "pick");
    ws.close(&path);
    assert_eq!(goto(&mut ws, &path, "super.pick()", "pick"), vec![expected]);
    assert!(goto(&mut ws, &path, "super.hiddenOnly()", "hiddenOnly").is_empty());
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_dir(&directory).unwrap();
}

#[test]
fn implicit_chain_references_and_applied_rename_include_every_real_use() {
    let mut ws = workspace();
    let definition = target(&ws, "Lookup.java", "getOrThrow");
    let references = ws.references(&definition.file, definition.start_byte, true);
    assert_eq!(
        references.len(),
        5,
        "declaration + bare/explicit/free/typed uses: {references:?}"
    );
    let edits = ws
        .rename(&definition.file, definition.start_byte, "findOrThrow")
        .unwrap();
    for file in [
        "Base.java",
        "Other.java",
        "Key.java",
        "Lookup.java",
        "OtherLookup.java",
        "Extensions.kt",
        "Drill.kt",
    ] {
        let original = ws.doc_text(file).unwrap();
        let expected = if file == "Lookup.java" || file == "Drill.kt" {
            original.replace("getOrThrow", "findOrThrow")
        } else {
            original.clone()
        };
        let applied = apply_to_text(file, &original, &edits).unwrap();
        assert_eq!(applied, expected, "whole-buffer rename {file}");
        ws.change(file, applied);
    }
    let expected = target(&ws, "Lookup.java", "findOrThrow");
    assert_eq!(
        goto(
            &mut ws,
            "Drill.kt",
            "return holderLookup(key).findOrThrow(key)",
            "findOrThrow"
        ),
        vec![expected]
    );
}

#[test]
fn implicit_member_and_extension_calls_follow_compiler_applicability_order() {
    let mut ws = workspace();
    let source = "package probe\nfun Base.choose(value: Int): Payload = Payload()\nfun choose(value: Int): OtherLookup = OtherLookup()\nclass Implicit : Base() {\n    fun run(): Payload = choose(1)\n}\nclass Member : Base() {\n    fun choose(value: Int): OtherLookup = OtherLookup()\n    fun run(): OtherLookup = choose(1)\n}\nclass Inapplicable : Base() {\n    fun choose(value: String): OtherLookup = OtherLookup()\n    fun run(): Payload = choose(1)\n}\n";
    ws.open("Priority.kt", source.to_string());
    let expected_extension = source.find("choose(value: Int): Payload").unwrap();
    let expected_member = source
        .find("choose(value: Int): OtherLookup = OtherLookup()\n    fun run")
        .unwrap();
    let expected = [expected_extension, expected_member, expected_extension];
    for ((start, _), definition) in source.match_indices("choose(1)").zip(expected) {
        assert_eq!(
            ws.goto_definition("Priority.kt", start),
            vec![Def {
                file: "Priority.kt".to_string(),
                start_byte: definition,
                end_byte: definition + "choose".len()
            }]
        );
        let tree = KotlinParser::new().parse(source);
        let node = tree
            .root_node()
            .descendant_for_byte_range(start, start + "choose(1)".len())
            .unwrap();
        let got = infer::infer(&ws.index, node, source, &FileCtx::from_tree(&tree, source));
        let expected_type = if definition == expected_member {
            "OtherLookup"
        } else {
            "Payload"
        };
        assert_eq!(got.name(), Some(expected_type));
    }
}

#[test]
fn lexical_function_and_parameter_do_not_infer_an_unrelated_implicit_extension() {
    let mut ws = workspace();
    ws.open("Lexical.kt", "package probe\nfun Base.choose(value: Int): Payload = Payload()\nclass Lexical : Base() {\n    fun local(): OtherLookup {\n        fun choose(value: Int): OtherLookup = OtherLookup()\n        return choose(1)\n    }\n    fun parameter(choose: (Int) -> OtherLookup): OtherLookup = choose(1)\n}\n".to_string());
    let source = ws.doc_text("Lexical.kt").unwrap();
    let definitions = [
        source.find("choose(value: Int): OtherLookup").unwrap(),
        source.find("choose: (Int)").unwrap(),
    ];
    for ((start, _), definition) in source.match_indices("choose(1)").zip(definitions) {
        assert_eq!(
            ws.goto_definition("Lexical.kt", start),
            vec![Def {
                file: "Lexical.kt".to_string(),
                start_byte: definition,
                end_byte: definition + "choose".len()
            }]
        );
        let tree = KotlinParser::new().parse(&source);
        let node = tree
            .root_node()
            .descendant_for_byte_range(start, start + "choose(1)".len())
            .unwrap();
        let got = infer::infer(
            &ws.index,
            node,
            &source,
            &FileCtx::from_tree(&tree, &source),
        );
        assert_ne!(
            got.name(),
            Some("Payload"),
            "a local callable must not borrow the extension's return type"
        );
    }
}

#[test]
fn bare_callable_class_property_uses_the_shared_call_selection() {
    let mut ws = workspace();
    add_payload_members(&mut ws);
    ws.open("Property.kt", "package probe\nclass Property {\n    val action: (Key<Payload>) -> Lookup<Payload> = { Lookup<Payload>() }\n    fun run(key: Key<Payload>): Payload = action(key).getOrThrow(key).payloadOnly()\n}\n".to_string());
    assert_eq!(
        infer_expression(&ws, "Property.kt", "action(key).getOrThrow(key)"),
        Type::class("Payload", Some("probe".to_string()))
    );
    assert!(
        labels(&mut ws, "Property.kt", "action(key).getOrThrow(key).pay").contains("payloadOnly")
    );
}

#[test]
fn unknown_arguments_do_not_choose_between_implicit_overloads() {
    let mut ws = workspace();
    add_payload_members(&mut ws);
    ws.open("Ambiguous.kt", "package probe\nfun Base.choose(value: Payload): Payload = value\nfun Base.choose(value: OtherPayload): OtherPayload = value\nclass Ambiguous : Base() {\n    fun run(value: Missing) { choose(value).payloadOnly() }\n}\n".to_string());
    assert_eq!(
        infer_expression(&ws, "Ambiguous.kt", "choose(value)"),
        Type::Unknown
    );
    assert!(labels(&mut ws, "Ambiguous.kt", "choose(value).pay").is_empty());
    let source = ws.doc_text("Ambiguous.kt").unwrap();
    let got = ws.goto_definition("Ambiguous.kt", source.find("choose(value).").unwrap());
    assert_eq!(
        got.len(),
        2,
        "both typed overloads remain possible: {got:?}"
    );
}

#[test]
fn nullable_method_type_variable_and_unknown_receiver_bindings_are_preserved() {
    let mut ws = workspace();
    ws.open("Nullable.kt", "package probe\nfun <T> Base.maybe(value: T): T? = value\nclass Nullable : Base() {\n    fun run(value: Payload) = maybe(value)\n}\nclass Box<T> {\n    fun keep(value: T): T = value\n}\nfun unknown(box: Box<Missing>, value: Payload) = box.keep(value)\n".to_string());
    let nullable = infer_expression(&ws, "Nullable.kt", "maybe(value)");
    assert_eq!(nullable.name(), Some("Payload"));
    assert!(nullable.is_nullable());
    assert_eq!(
        infer_expression(&ws, "Nullable.kt", "box.keep(value)"),
        Type::Unknown,
        "method arguments cannot replace an unknown owning-class T"
    );
}

#[test]
fn conflicting_method_arguments_do_not_choose_the_first_binding() {
    let mut ws = workspace();
    add_payload_members(&mut ws);
    ws.open("Conflicting.kt", "package probe\nfun <T> Base.choose(first: T, second: T): T = first\nclass Conflicting : Base() {\n    fun run(payload: Payload, other: OtherPayload) = choose(payload, other)\n}\n".to_string());
    // Kotlin infers a common supertype. Until that join is modeled, neither argument's concrete
    // members can be exposed as though the other constraint did not exist.
    assert_eq!(
        infer_expression(&ws, "Conflicting.kt", "choose(payload, other)"),
        Type::Unknown
    );
}

#[test]
fn reordered_named_arguments_cannot_be_unified_positionally() {
    let mut ws = workspace();
    add_payload_members(&mut ws);
    ws.open("NamedArguments.kt", "package probe\nfun <T, U> Base.first(first: T, second: U): T = first\nclass NamedArguments : Base() {\n    fun run(payload: Payload, other: OtherPayload): Payload = first(second = other, first = payload)\n}\n".to_string());
    let inferred = infer_expression(
        &ws,
        "NamedArguments.kt",
        "first(second = other, first = payload)",
    );
    assert_eq!(inferred, Type::class("Payload", Some("probe".into())));
    let source = ws.doc_text("NamedArguments.kt").unwrap();
    ws.change(
        "NamedArguments.kt",
        source.replace(
            "first(second = other, first = payload)",
            "first(second = other, first = payload).payloadOnly()",
        ),
    );
    let found = labels(
        &mut ws,
        "NamedArguments.kt",
        "first(second = other, first = payload).",
    );
    assert!(found.contains("payloadOnly"), "{found:?}");
    assert!(!found.contains("otherOnly"), "{found:?}");
    let expected = target(&ws, "Extensions.kt", "payloadOnly");
    assert_eq!(
        goto(
            &mut ws,
            "NamedArguments.kt",
            "first(second = other, first = payload).payloadOnly()",
            "payloadOnly"
        ),
        vec![expected]
    );
}

#[test]
fn unsaved_extension_return_edit_replaces_chain_members_and_navigation() {
    let mut ws = workspace();
    add_payload_members(&mut ws);
    let original = ws.doc_text("Extensions.kt").unwrap();
    ws.change("OtherLookup.java", "package probe; public class OtherLookup { public OtherPayload getOrThrow(Object key) { return null; } }".to_string());
    let source = "package probe\nclass Edit : Base() {\n    fun run(key: Key<Payload>) { holderLookup(key).getOrThrow(key).payloadOnly() }\n}\n";
    ws.open("Edit.kt", source.to_string());
    for (extensions, correct_file, expected, absent) in [
        (original.clone(), "Lookup.java", "payloadOnly", "otherOnly"),
        (
            original.replace("Lookup<T> = Lookup<T>()", "OtherLookup = OtherLookup()"),
            "OtherLookup.java",
            "otherOnly",
            "payloadOnly",
        ),
        (original, "Lookup.java", "payloadOnly", "otherOnly"),
    ] {
        ws.change("Extensions.kt", extensions);
        let cursor = source.find(".payloadOnly").unwrap() + 1;
        let labels: HashSet<_> = ws
            .complete("Edit.kt", cursor, true)
            .unwrap()
            .items
            .into_iter()
            .map(|item| item.label)
            .collect();
        assert!(labels.contains(expected), "{labels:?}");
        assert!(!labels.contains(absent), "stale result {labels:?}");
        let expected = target(&ws, correct_file, "getOrThrow");
        assert_eq!(
            goto(
                &mut ws,
                "Edit.kt",
                "holderLookup(key).getOrThrow(key)",
                "getOrThrow"
            ),
            vec![expected]
        );
    }
}

#[test]
fn top_level_extension_this_keeps_generic_member_completion_and_navigation() {
    let mut ws = workspace();
    add_payload_members(&mut ws);
    ws.open("ExplicitExtension.kt", "package probe\nfun Base.explicit(key: Key<Payload>): Payload = this.holderLookup(key).getOrThrow(key).payloadOnly()\n".to_string());
    assert_eq!(
        infer_expression(&ws, "ExplicitExtension.kt", "this"),
        Type::class("Base", Some("probe".to_string()))
    );
    assert!(labels(
        &mut ws,
        "ExplicitExtension.kt",
        "this.holderLookup(key).get"
    )
    .contains("getOrThrow"));
    let expected = target(&ws, "Lookup.java", "getOrThrow");
    assert_eq!(
        goto(
            &mut ws,
            "ExplicitExtension.kt",
            "this.holderLookup(key).getOrThrow(key)",
            "getOrThrow"
        ),
        vec![expected]
    );
    assert_eq!(
        infer_expression(
            &ws,
            "ExplicitExtension.kt",
            "this.holderLookup(key).getOrThrow(key)"
        ),
        Type::class("Payload", Some("probe".to_string()))
    );
}

#[test]
fn explicit_extension_this_changes_with_the_unsaved_receiver() {
    let mut ws = workspace();
    let original = "package probe\nfun Base.current(): Float = this.getBreakSpeed()\n";
    for (source, owner) in [
        (original.to_string(), "Base.java"),
        (
            original.replace("Base.current", "Other.current"),
            "Other.java",
        ),
        (original.to_string(), "Base.java"),
    ] {
        ws.change("ReceiverEdit.kt", source);
        assert!(labels(&mut ws, "ReceiverEdit.kt", "this.get").contains("getBreakSpeed"));
        let expected = target(&ws, owner, "getBreakSpeed");
        assert_eq!(
            goto(
                &mut ws,
                "ReceiverEdit.kt",
                "this.getBreakSpeed()",
                "getBreakSpeed"
            ),
            vec![expected]
        );
    }
}

#[test]
fn missing_extension_this_does_not_take_the_enclosing_class_receiver() {
    let mut ws = workspace();
    ws.open("UnknownExtension.kt", "package probe\nclass Outer {\n    fun outerOnly(): Outer = this\n    fun Missing.current() = this.outerOnly()\n}\n".to_string());
    assert!(labels(&mut ws, "UnknownExtension.kt", "this.outer").is_empty());
    assert!(goto(
        &mut ws,
        "UnknownExtension.kt",
        "this.outerOnly()",
        "outerOnly"
    )
    .is_empty());
    assert_eq!(
        infer_expression(&ws, "UnknownExtension.kt", "this.outerOnly()"),
        Type::Unknown
    );
}

#[test]
fn extension_this_preserves_inner_scope_receiver_precedence() {
    let mut ws = workspace();
    ws.open("ScopedExtension.kt", "package probe\nfun Base.current(other: Other) {\n    other.apply { this.getBreakSpeed() }\n}\n".to_string());
    let expected = target(&ws, "Other.java", "getBreakSpeed");
    assert_eq!(
        goto(
            &mut ws,
            "ScopedExtension.kt",
            "this.getBreakSpeed()",
            "getBreakSpeed"
        ),
        vec![expected]
    );
}

const SPRING_EXTENSION_SOURCE: &str =
    include_str!("../../../dev/corpus/fixtures/spring-provider/BeanFactoryExtensions.kt");

fn spring_workspace() -> Workspace {
    let mut ws = Workspace::new();
    for (file, source) in [
        (
            "BeanFactory.java",
            include_str!("../../../dev/corpus/fixtures/spring-provider/BeanFactory.java"),
        ),
        (
            "ObjectProvider.java",
            include_str!("../../../dev/corpus/fixtures/spring-provider/ObjectProvider.java"),
        ),
        (
            "ResolvableType.java",
            include_str!("../../../dev/corpus/fixtures/spring-provider/ResolvableType.java"),
        ),
        (
            "ParameterizedTypeReference.java",
            include_str!(
                "../../../dev/corpus/fixtures/spring-provider/ParameterizedTypeReference.java"
            ),
        ),
        ("BeanFactoryExtensions.kt", SPRING_EXTENSION_SOURCE),
    ] {
        ws.open(file, source.to_string());
    }
    ws
}

#[test]
fn spring_recovery_restores_later_extension_for_a_dangling_selector() {
    let mut ws = spring_workspace();
    let broken = SPRING_EXTENSION_SOURCE.replacen(
        "getBeanProvider<T>().getObject()",
        "getBeanProvider<T>().",
        1,
    );
    ws.change("BeanFactoryExtensions.kt", broken.clone());
    let got = labels(&mut ws, "BeanFactoryExtensions.kt", "getBeanProvider<T>().");
    assert!(got.contains("getObject"), "{got:?}");
    assert!(!got.contains("getBeanProvider"), "{got:?}");
    assert_eq!(ws.doc_text("BeanFactoryExtensions.kt").unwrap(), broken);
}

#[test]
fn spring_recovery_keeps_other_file_overloads_ambiguous() {
    let mut ws = spring_workspace();
    ws.open("Competing.kt", "package oracle\nclass DifferentProvider<T> { fun wrongProvider(): T = TODO() }\ninline fun <reified T : Any> BeanFactory.getBeanProvider(): DifferentProvider<T> = DifferentProvider<T>()\n".to_string());
    let broken = SPRING_EXTENSION_SOURCE.replacen(
        "getBeanProvider<T>().getObject()",
        "getBeanProvider<T>().",
        1,
    );
    ws.change("BeanFactoryExtensions.kt", broken);
    let got = labels(&mut ws, "BeanFactoryExtensions.kt", "getBeanProvider<T>().");
    assert!(got.is_empty(), "ambiguous provider: {got:?}");
}

#[test]
fn spring_recovery_respects_module_visibility_and_its_changes() {
    use ktlsp::project_model::{FileOrigin, ProjectScope, ScopeVisibility, SourceRepresentation};
    let mut ws = spring_workspace();
    ws.open("Competing.kt", "package oracle\nclass DifferentProvider<T> { fun wrongProvider(): T = TODO() }\ninline fun <reified T : Any> BeanFactory.getBeanProvider(): DifferentProvider<T> = DifferentProvider<T>()\n".to_string());
    for (file, module) in [
        ("BeanFactory.java", "a"),
        ("ObjectProvider.java", "a"),
        ("ResolvableType.java", "a"),
        ("ParameterizedTypeReference.java", "a"),
        ("BeanFactoryExtensions.kt", "a"),
        ("Competing.kt", "b"),
    ] {
        ws.index.set_file_origin(
            file,
            FileOrigin {
                origin_id: module.to_string(),
                representation: SourceRepresentation::Source,
                project_scope: Some(ProjectScope {
                    module: module.to_string(),
                    source_set: "main".to_string(),
                }),
            },
        );
    }
    let broken = SPRING_EXTENSION_SOURCE.replacen(
        "getBeanProvider<T>().getObject()",
        "getBeanProvider<T>().",
        1,
    );
    ws.change("BeanFactoryExtensions.kt", broken);
    for include_competitor in [false, true, false] {
        ws.index.replace_scope_visibilities(vec![ScopeVisibility {
            scope: ProjectScope {
                module: "a".to_string(),
                source_set: "main".to_string(),
            },
            ordered_origins: if include_competitor {
                vec!["a".to_string(), "b".to_string()]
            } else {
                vec!["a".to_string()]
            },
        }]);
        ws.bump_index_revision();
        let got = labels(&mut ws, "BeanFactoryExtensions.kt", "getBeanProvider<T>().");
        assert_eq!(got.contains("getObject"), !include_competitor, "{got:?}");
        assert!(!got.contains("wrongProvider"), "{got:?}");
    }
}

#[test]
fn spring_intermediate_selector_completes_the_provider_before_its_call_result() {
    let mut ws = spring_workspace();
    let got = labels(&mut ws, "BeanFactoryExtensions.kt", "provider.getObject");
    assert!(got.contains("getObject"), "{got:?}");
    let expected = target(&ws, "ObjectProvider.java", "getObject");
    assert_eq!(
        goto(
            &mut ws,
            "BeanFactoryExtensions.kt",
            "provider.getObject()",
            "getObject"
        ),
        vec![expected]
    );
}

#[test]
fn spring_closed_incomplete_file_uses_the_same_recovered_receiver() {
    let directory = std::env::temp_dir().join(format!(
        "ktlsp-spring-closed-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("Closed.kt").to_string_lossy().into_owned();
    let broken = SPRING_EXTENSION_SOURCE.replacen(
        "getBeanProvider<T>().getObject()",
        "getBeanProvider<T>().",
        1,
    );
    std::fs::write(&path, &broken).unwrap();
    let mut ws = spring_workspace();
    ws.close("BeanFactoryExtensions.kt");
    ws.index.remove_file("BeanFactoryExtensions.kt");
    ws.open(&path, broken);
    ws.close(&path);
    let got = labels(&mut ws, &path, "getBeanProvider<T>().");
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_dir(&directory).unwrap();
    assert!(got.contains("getObject"), "{got:?}");
}

#[test]
fn local_extension_typed_return_and_generic_receiver_are_inferred() {
    let mut ws = Workspace::new();
    ws.open("Local.kt", "package app\nclass Versioned\nclass Payload {\n    fun payloadOnly(): Int = 7\n}\nfun use(v: Versioned): Payload {\n    fun Versioned.oldApi(): Payload = Payload()\n    fun <T> T.keep(): T = this\n    return v.keep().oldApi()\n}\n".to_string());
    assert_eq!(
        infer_expression(&ws, "Local.kt", "v.keep()"),
        Type::class("Versioned", Some("app".to_string()))
    );
    assert_eq!(
        infer_expression(&ws, "Local.kt", "v.keep().oldApi()"),
        Type::class("Payload", Some("app".to_string()))
    );
    assert!(ws.index.lookup_by_name("oldApi").is_empty());
    assert!(ws.index.lookup_by_name("keep").is_empty());
}

#[test]
fn local_extension_inner_unknown_argument_keeps_both_return_types_unknown() {
    let mut ws = Workspace::new();
    ws.open("Local.kt", "package app\nclass Versioned\nclass Payload\nclass OtherPayload\nfun use(v: Versioned, unknown: Missing) {\n    fun Versioned.oldApi(value: Int): Payload = Payload()\n    if (true) {\n        fun Versioned.oldApi(value: String): OtherPayload = OtherPayload()\n        v.oldApi(unknown)\n    }\n}\n".to_string());
    assert_eq!(
        infer_expression(&ws, "Local.kt", "v.oldApi(unknown)"),
        Type::Unknown
    );
}

#[test]
fn local_extension_bare_call_uses_its_implicit_receiver() {
    let mut ws = Workspace::new();
    ws.open("Local.kt", "package app\nclass Versioned\nclass Payload\nclass OtherPayload\nfun Versioned.oldApi(value: Int): OtherPayload = OtherPayload()\nclass Use {\n    fun Versioned.use(): Payload {\n        fun Versioned.oldApi(value: Int): Payload = Payload()\n        return oldApi(1)\n    }\n}\n".to_string());
    assert_eq!(
        infer_expression(&ws, "Local.kt", "oldApi(1)"),
        Type::class("Payload", Some("app".to_string()))
    );
}

#[test]
fn local_extension_unsaved_replacement_updates_chain_without_global_publication() {
    let mut ws = Workspace::new();
    let original = "package app\nclass Versioned {\n    fun newApi(): Int = 2\n}\nclass Payload {\n    fun payloadOnly(): Int = 7\n}\nclass Changed {\n    fun changedOnly(): Int = 8\n}\nfun use(v: Versioned) {\n    fun Versioned.oldApi(): Payload = Payload()\n    v.oldApi().payloadOnly()\n}\n";
    ws.open("Local.kt", original.to_string());
    assert!(labels(&mut ws, "Local.kt", "v.oldApi().pay").contains("payloadOnly"));
    assert_eq!(
        infer_expression(&ws, "Local.kt", "v.oldApi()"),
        Type::class("Payload", Some("app".to_string()))
    );
    let changed = original
        .replace(
            "oldApi(): Payload = Payload()",
            "oldApi(): Changed = Changed()",
        )
        .replace("v.oldApi().payloadOnly()", "v.oldApi().changedOnly()");
    ws.change("Local.kt", changed);
    assert!(labels(&mut ws, "Local.kt", "v.oldApi().chan").contains("changedOnly"));
    assert_eq!(
        infer_expression(&ws, "Local.kt", "v.oldApi()"),
        Type::class("Changed", Some("app".to_string()))
    );
    assert!(ws.index.lookup_by_name("oldApi").is_empty());
    ws.change(
        "Local.kt",
        original.replace("    fun Versioned.oldApi(): Payload = Payload()\n", ""),
    );
    assert_eq!(
        infer_expression(&ws, "Local.kt", "v.oldApi()"),
        Type::Unknown
    );
    assert!(labels(&mut ws, "Local.kt", "v.oldApi().pay").is_empty());
}

#[test]
fn local_extension_unproved_receiver_never_becomes_a_known_target_or_type() {
    for declaration in [
        "fun Missing.oldApi(): Payload = Payload()",
        "fun <T> T.oldApi(): Payload where T : Missing = Payload()",
    ] {
        let mut ws = Workspace::new();
        let source = format!(
            "package app\nclass Versioned {{\n    fun newApi(): Int = 2\n}}\nclass Payload {{\n    fun payloadOnly(): Int = 7\n}}\nfun use(v: Versioned) {{\n    {declaration}\n    v.oldApi()\n    v.newApi()\n}}\n"
        );
        let offset = source.rfind("oldApi()").unwrap();
        ws.open("Local.kt", source);
        assert_eq!(
            infer_expression(&ws, "Local.kt", "v.oldApi()"),
            Type::Unknown,
            "{declaration}"
        );
        assert!(
            ws.goto_definition("Local.kt", offset).is_empty(),
            "{declaration}"
        );
        assert_eq!(
            ws.explain_resolution("Local.kt", offset).unwrap().status,
            "unknown",
            "{declaration}"
        );
        assert!(labels(&mut ws, "Local.kt", "v.new").contains("newApi"));
    }
}

#[test]
fn local_extension_any_upper_bound_resolves_known_non_null_receiver() {
    let mut ws = Workspace::new();
    let source = "package app\nclass Versioned {\n    fun newApi(): Int = 2\n}\nclass Payload {\n    fun payloadOnly(): Int = 7\n}\nfun use(v: Versioned) {\n    fun <T : Any> T.oldApi(): Payload = Payload()\n    v.oldApi().payloadOnly()\n    v.newApi()\n}\n";
    let start = source.find("T.oldApi").unwrap() + 2;
    ws.open("Local.kt", source.to_string());
    assert_eq!(
        infer_expression(&ws, "Local.kt", "v.oldApi()"),
        Type::class("Payload", Some("app".to_string()))
    );
    assert!(labels(&mut ws, "Local.kt", "v.old").contains("oldApi"));
    assert!(labels(&mut ws, "Local.kt", "v.oldApi().pay").contains("payloadOnly"));
    assert_eq!(
        goto(&mut ws, "Local.kt", "v.oldApi()", "oldApi"),
        [Def {
            file: "Local.kt".to_string(),
            start_byte: start,
            end_byte: start + "oldApi".len()
        }]
    );
    assert!(labels(&mut ws, "Local.kt", "v.new").contains("newApi"));
    assert!(ws.index.lookup_by_name("oldApi").is_empty());
}

#[test]
fn local_extension_nullable_receiver_requires_nullable_formal_or_safe_navigation() {
    for (declaration, expression, known) in [
        (
            "fun Versioned.oldApi(): Payload = Payload()",
            "v.oldApi()",
            false,
        ),
        (
            "fun Versioned?.oldApi(): Payload = Payload()",
            "v.oldApi()",
            true,
        ),
        (
            "fun Versioned.oldApi(): Payload = Payload()",
            "v?.oldApi()",
            true,
        ),
        (
            "fun <T : Any> T.oldApi(): Payload = Payload()",
            "v.oldApi()",
            false,
        ),
    ] {
        let mut ws = Workspace::new();
        ws.open("Local.kt", format!("package app\nclass Versioned\nclass Payload\nfun use(v: Versioned?) {{\n    {declaration}\n    {expression}\n}}\n"));
        let result = infer_expression(&ws, "Local.kt", expression);
        assert_eq!(
            result.name(),
            known.then_some("Payload"),
            "{declaration}: {expression}"
        );
        if known {
            assert_eq!(
                result.is_nullable(),
                expression.contains("?."),
                "{declaration}: {expression}"
            );
        }
    }
    let mut ws = Workspace::new();
    ws.open("Local.kt", "package app\nclass Versioned\nclass Payload { fun payloadOnly(): Int = 7 }\nfun use(v: Versioned?): Int? {\n    fun Versioned.oldApi(): Payload = Payload()\n    return v?.oldApi()?.payloadOnly()\n}\n".to_string());
    let result = infer_expression(&ws, "Local.kt", "v?.oldApi()?.payloadOnly()");
    assert_eq!(result.name(), Some("Int"));
    assert!(result.is_nullable());
}

#[test]
fn local_extension_lambda_receiver_precedes_scope_function_intrinsics() {
    for (declaration, body, expected) in [
        (
            "fun Versioned.run(block: Payload.() -> Int): Int = Payload().block()",
            "v.run { payloadOnly() }",
            "Int",
        ),
        (
            "fun Versioned.run(value: Int, block: Payload.() -> Int): Int = Payload().block()",
            "v.run { payloadOnly() }",
            "String",
        ),
        (
            "fun Versioned.run(block: (Payload) -> Int): Int = block(Payload())",
            "v.run { it.payloadOnly() }",
            "Int",
        ),
    ] {
        let mut ws = Workspace::new();
        let source = format!(
            "package app\nclass Versioned {{\n    fun payloadOnly(): String = \"decoy\"\n}}\nclass Payload {{\n    fun payloadOnly(): Int = 7\n}}\nfun use(v: Versioned) {{\n    {declaration}\n    {body}\n}}\n"
        );
        let use_offset = source.rfind("payloadOnly()").unwrap();
        let definition = source
            .find(if expected == "Int" {
                "payloadOnly(): Int"
            } else {
                "payloadOnly(): String"
            })
            .unwrap();
        ws.open("Local.kt", source);
        let source = ws.doc_text("Local.kt").unwrap();
        let tree = KotlinParser::new().parse(&source);
        let (start, expression) = if body.contains("it.") {
            (use_offset - "it.".len(), "it.payloadOnly()")
        } else {
            (use_offset, "payloadOnly()")
        };
        let node = tree
            .root_node()
            .named_descendant_for_byte_range(start, start + expression.len())
            .unwrap();
        assert_eq!(&source[node.byte_range()], expression);
        assert_eq!(
            infer::infer(
                &ws.index.scoped_for_file("Local.kt"),
                node,
                &source,
                &FileCtx::from_tree_in_file(&tree, &source, "Local.kt")
            )
            .name(),
            Some(expected),
            "{declaration}"
        );
        assert_eq!(
            ws.goto_definition("Local.kt", use_offset),
            vec![Def {
                file: "Local.kt".into(),
                start_byte: definition,
                end_byte: definition + "payloadOnly".len()
            }],
            "{declaration}"
        );
        let prefix = if body.contains("it.") {
            "v.run { it.pay"
        } else {
            "v.run { pay"
        };
        assert!(
            labels(&mut ws, "Local.kt", prefix).contains("payloadOnly"),
            "{declaration}"
        );
    }
}

#[test]
fn named_trailing_lambda_receiver_does_not_reuse_written_argument_order() {
    let mut ws = workspace();
    add_payload_members(&mut ws);
    let source = "package probe
fun <T,U> scope(first:T,second:U,block:T.()->Unit) {}
fun use(payload:Payload,other:OtherPayload) { scope(payload,other) { payloadOnly() };scope(second=other,first=payload) { payloadOnly() };other.otherOnly() }
";
    ws.open("NamedTrailing.kt", source.into());
    let positional = labels(&mut ws, "NamedTrailing.kt", "scope(payload,other) { ");
    assert!(positional.contains("payloadOnly"), "{positional:?}");
    assert!(!positional.contains("otherOnly"), "{positional:?}");
    let named = labels(
        &mut ws,
        "NamedTrailing.kt",
        "scope(second=other,first=payload) { ",
    );
    // Named trailing-lambda binding is still valid unsupported coverage, never the positional OtherPayload.
    assert!(
        !named.contains("payloadOnly") && !named.contains("otherOnly"),
        "{named:?}"
    );
    let known = labels(&mut ws, "NamedTrailing.kt", "other.");
    assert!(known.contains("otherOnly"), "{known:?}");
}

#[test]
fn spring_static_factory_projection_keeps_fixed_return_without_selecting_an_overload() {
    let mut ws = spring_workspace();
    ws.open(
        "Class.java",
        "package java.lang; public final class Class<T> {}".into(),
    );
    ws.open(
        "ReflectType.java",
        "package java.lang.reflect; public interface Type {}".into(),
    );
    ws.change("ResolvableType.java", "package oracle; public final class ResolvableType { public static ResolvableType forType(java.lang.reflect.Type type) { return new ResolvableType(); } public static ResolvableType forType(ParameterizedTypeReference<?> type) { return new ResolvableType(); } }".into());
    let factory = "ResolvableType.forType((object : ParameterizedTypeReference<T>() {}).type)";
    // The factory's target can remain uncertain; both declarations have this fixed result.
    assert_eq!(
        infer_expression(&ws, "BeanFactoryExtensions.kt", factory),
        Type::class("ResolvableType", Some("oracle".into()))
    );
    let expected = ws
        .index
        .lookup_by_name("getBeanProvider")
        .iter()
        .find(|entry| {
            entry.path.as_ref() == "BeanFactory.java"
                && entry
                    .sym
                    .params
                    .first()
                    .is_some_and(|parameter| parameter.name == "ResolvableType")
        })
        .map(|entry| Def {
            file: entry.path.to_string(),
            start_byte: entry.sym.start_byte,
            end_byte: entry.sym.end_byte,
        })
        .unwrap();
    let source = ws.doc_text("BeanFactoryExtensions.kt").unwrap();
    let outer = source
        .find("getBeanProvider(ResolvableType.forType(")
        .unwrap();
    assert_eq!(
        ws.goto_definition("BeanFactoryExtensions.kt", outer),
        vec![expected]
    );
}

#[test]
fn spring_known_nominal_argument_rejects_unbounded_class_type_parameter_overload() {
    let mut ws = spring_workspace();
    ws.open(
        "Class.java",
        "package java.lang; public final class Class<T> {}".into(),
    );
    ws.open("Known.kt", "package oracle\nfun BeanFactory.control(required: ResolvableType): ObjectProvider<Payload> = getBeanProvider(required)\n".into());
    let expected = ws
        .index
        .lookup_by_name("getBeanProvider")
        .iter()
        .find(|entry| {
            entry.path.as_ref() == "BeanFactory.java"
                && entry
                    .sym
                    .params
                    .first()
                    .is_some_and(|parameter| parameter.name == "ResolvableType")
        })
        .map(|entry| Def {
            file: entry.path.to_string(),
            start_byte: entry.sym.start_byte,
            end_byte: entry.sym.end_byte,
        })
        .unwrap();
    assert_eq!(
        goto(
            &mut ws,
            "Known.kt",
            "getBeanProvider(required)",
            "getBeanProvider"
        ),
        vec![expected]
    );
}

#[test]
fn projected_fixed_return_does_not_choose_a_factory_target_or_invent_generic_results() {
    let mut ws = spring_workspace();
    ws.open(
        "ReflectType.java",
        "package java.lang.reflect; public interface Type {}".into(),
    );
    ws.open(
        "Other.java",
        "package oracle; public final class Other {}".into(),
    );
    ws.change("ResolvableType.java", "package oracle; public final class ResolvableType { public static ResolvableType forType(java.lang.reflect.Type type) { return new ResolvableType(); } public static ResolvableType forType(ParameterizedTypeReference<?> type) { return new ResolvableType(); } public static Other different(ParameterizedTypeReference<?> type) { return null; } public static ResolvableType different(java.lang.reflect.Type type) { return null; } public static <T> T dependent(ParameterizedTypeReference<? extends T> type) { return null; } }".into());
    ws.open("Unknown.kt", "package oracle\nfun use(value: Missing) { ResolvableType.forType(value); ResolvableType.different(value); ResolvableType.dependent(value) }\n".into());
    assert_eq!(
        infer_expression(&ws, "Unknown.kt", "ResolvableType.forType(value)"),
        Type::class("ResolvableType", Some("oracle".into()))
    );
    let definitions = goto(
        &mut ws,
        "Unknown.kt",
        "ResolvableType.forType(value)",
        "forType",
    );
    assert_eq!(
        definitions.len(),
        2,
        "the unknown argument cannot select a factory overload: {definitions:?}"
    );
    assert_eq!(
        infer_expression(&ws, "Unknown.kt", "ResolvableType.different(value)"),
        Type::Unknown
    );
    assert_eq!(
        infer_expression(&ws, "Unknown.kt", "ResolvableType.dependent(value)"),
        Type::Unknown
    );
    ws.open(
        "Rejected.kt",
        "package oracle\nfun use() { ResolvableType.forType(true) }\n".into(),
    );
    assert_eq!(
        infer_expression(&ws, "Rejected.kt", "ResolvableType.forType(true)"),
        Type::Unknown
    );
}
