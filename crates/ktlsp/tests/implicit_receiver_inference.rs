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
        let source = format!("package probe\nclass Multiple : {inheritance} {{\n    override fun marker(): Float = 3f\n    override fun getBreakSpeed(): Float = super.getBreakSpeed()\n}}\n");
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
        let source = format!("package probe\nclass Use : Base{constructor} {{ override val value: Payload get() = super.value\n fun chain() = super.value.payloadOnly()\n fun incomplete() {{ super. }} }}\n");
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
        ws.open("Payload.kt", "package probe\nclass Payload { fun payloadOnly(): Int = 1 }\n".into());
        ws.open("Base.kt", base.into());
        ws.open("Use.kt", "package probe\nclass Use : Base<Payload>() { fun exact() = super.value\n fun chain() = super.value.payloadOnly()\n fun incomplete() { super. } }\n".into());
        assert_eq!(infer_expression(&ws, "Use.kt", "super.value"), Type::class("Payload", Some("probe".into())), "{base}");
        assert!(labels(&mut ws, "Use.kt", "super.").contains("value"), "{base}");
        assert!(labels(&mut ws, "Use.kt", "super.value.pay").contains("payloadOnly"), "{base}");
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
