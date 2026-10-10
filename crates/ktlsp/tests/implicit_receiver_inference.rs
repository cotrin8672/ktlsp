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
        ("GenericUse.kt", "package probe\nclass GenericUse : GenericBase<Payload>() {\n    fun run(): Payload = super.inherited().payloadOnly()\n}\n"),
        ("JavaGenericUse.java", "package probe; class JavaGenericUse extends GenericBase<Payload> implements Marker { Payload run() { return super.inherited().payloadOnly(); } }"),
    ] {
        ws.open(file, source.to_string());
        assert!(labels(&mut ws, file, "super.inherited().pay").contains("payloadOnly"), "{file}");
        let expected = target(&ws, "GenericBase.java", "inherited");
        assert_eq!(goto(&mut ws, file, "super.inherited()", "inherited"), vec![expected]);
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
        let expression = if source.contains("super<Other>") { "super<Other>" } else { "super" };
        assert_eq!(infer_expression(&ws, "Use.kt", expression), Type::Unknown);
        assert!(goto(&mut ws, "Use.kt", &format!("{expression}.getBreakSpeed()"), "getBreakSpeed").is_empty());
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
    // Named generic argument mapping remains a coverage gap; never return the positional OtherPayload.
    assert_eq!(inferred, Type::Unknown);
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
