use ktlsp::edit::apply_to_text;
use ktlsp::workspace::Workspace;

#[test]
fn kotlin_property_rename_applies_each_java_accessor_and_constructor_label_spelling() {
    let properties = "package probe\nclass Payload\nopen class Box<T>(var value: T)\nclass Child : Box<Payload>(Payload())\nclass Other(var value: Payload)\n";
    let kotlin = "package use\nimport probe.Box as Alias\nimport probe.Payload\nimport probe.Other\nfun use(box: Alias<Payload>, other: Other) { box.value = Payload(); Alias(value = Payload()); Other(value = Payload()); other.value }\n";
    let java = "package probe; class Use { void use(Box<Payload> box, Child child, Other other) { box.setValue(new Payload()); box.getValue(); child.getValue(); other.getValue(); other.setValue(new Payload()); } }";
    for (file, cursor, placeholder) in [
        (
            "Properties.kt",
            properties.find("var value").unwrap() + 4,
            "value",
        ),
        ("Use.java", java.find("getValue").unwrap(), "value"),
        ("Use.java", java.find("setValue").unwrap(), "value"),
    ] {
        let mut ws = Workspace::new();
        for (key, source) in [
            ("Properties.kt", properties),
            ("Use.kt", kotlin),
            ("Use.java", java),
        ] {
            ws.open(key, source.into());
        }
        let prepared = ws
            .prepare_rename(file, cursor)
            .expect("an accessor renames its authored property");
        assert_eq!(prepared.placeholder, placeholder);
        let source = if file.ends_with(".java") {
            java
        } else {
            properties
        };
        assert_eq!(
            &source[prepared.range.start_byte..prepared.range.end_byte],
            if file.ends_with(".java") {
                &source[cursor..cursor + 8]
            } else {
                "value"
            }
        );
        let edits = ws
            .rename(file, cursor, "renamed")
            .expect("mixed property rename");
        let expected_properties = properties.replacen("var value", "var renamed", 1);
        let expected_kotlin = kotlin
            .replace("box.value", "box.renamed")
            .replace("Alias(value", "Alias(renamed");
        let expected_java = java
            .replace("box.setValue", "box.setRenamed")
            .replace("box.getValue", "box.getRenamed")
            .replace("child.getValue", "child.getRenamed");
        for (key, original, expected) in [
            ("Properties.kt", properties, expected_properties.as_str()),
            ("Use.kt", kotlin, expected_kotlin.as_str()),
            ("Use.java", java, expected_java.as_str()),
        ] {
            let actual = apply_to_text(key, original, &edits).unwrap();
            assert_eq!(actual, expected, "whole buffer {file} -> {key}");
            ws.change(key, actual);
        }
        let definitions = ws.goto_definition(
            "Use.java",
            expected_java.find("box.getRenamed").unwrap() + 4,
        );
        assert_eq!(definitions.len(), 1, "{definitions:?}");
        assert_eq!(definitions[0].file, "Properties.kt");
        assert_eq!(
            &expected_properties[definitions[0].start_byte..definitions[0].end_byte],
            "renamed"
        );
    }
}

#[test]
fn property_rename_preserves_is_prefix_rules_in_both_directions() {
    for (old, new, old_getter, new_getter) in [
        ("isReady", "ready", "isReady", "getReady"),
        ("ready", "isReady", "getReady", "isReady"),
    ] {
        let mut ws = Workspace::new();
        let properties = format!("package probe\nclass Flag(var {old}: Boolean)\n");
        let kotlin = format!(
            "package probe\nfun use(flag: Flag) {{ flag.{old} = true; Flag({old} = true) }}\n"
        );
        let java = format!("package probe; class Use {{ void use(Flag flag) {{ flag.{old_getter}(); flag.setReady(true); }} }}");
        for (file, text) in [
            ("Flag.kt", &properties),
            ("Use.kt", &kotlin),
            ("Use.java", &java),
        ] {
            ws.open(file, text.clone());
        }
        let cursor = java.find(old_getter).unwrap();
        let prepared = ws.prepare_rename("Use.java", cursor).unwrap();
        assert_eq!(prepared.placeholder, old);
        assert_eq!(
            &java[prepared.range.start_byte..prepared.range.end_byte],
            old_getter
        );
        let edits = ws.rename("Use.java", cursor, new).unwrap();
        for (file, original, expected) in [
            ("Flag.kt", properties.as_str(), properties.replace(old, new)),
            ("Use.kt", kotlin.as_str(), kotlin.replace(old, new)),
            (
                "Use.java",
                java.as_str(),
                java.replace(old_getter, new_getter),
            ),
        ] {
            assert_eq!(
                apply_to_text(file, original, &edits).unwrap(),
                expected,
                "whole buffer {old} -> {new}: {file}"
            );
        }
    }
}

#[test]
fn property_rename_refuses_overrides_cycles_unknown_and_non_primary_constructor_labels() {
    for source in [
        "package probe\nopen class Box(open val value: Int)\nclass Child : Box(1) { override val value: Int = 2 }\n",
        "package probe\nopen class Box(val value: Int)\nclass Child : Box(1)\nclass BoxCycle(val value: Int) : OtherCycle()\nclass OtherCycle : BoxCycle(1)\n",
        "package probe\nclass Box(val value: Int) { constructor(value: String) : this(1) }\n",
        "package probe\nclass Box(val value: Int, val renamed: Int)\n",
    ] {
        let mut ws = Workspace::new();
        ws.open("Properties.kt", source.into());
        let target = if source.contains("BoxCycle") { "val value: Int) : OtherCycle" } else { "val value: Int" };
        let cursor = source.find(target).unwrap() + 4;
        assert!(ws.rename("Properties.kt", cursor, "renamed").is_none(), "must refuse a partial family: {source}");
    }
    let mut ws = Workspace::new();
    let private =
        "package probe\nclass Hidden(private val value: Int) { fun read(): Int = value }\n";
    let java = "package probe; class Use { void use(Hidden hidden, Missing unknown) { hidden.getValue(); unknown.getValue(); } }";
    ws.open("Private.kt", private.into());
    ws.open("Use.java", java.into());
    assert!(ws
        .prepare_rename("Use.java", java.find("getValue").unwrap())
        .is_none());
    assert!(ws
        .rename("Use.java", java.rfind("getValue").unwrap(), "renamed")
        .is_none());
    let edits = ws
        .rename(
            "Private.kt",
            private.find("val value").unwrap() + 4,
            "renamed",
        )
        .unwrap();
    assert_eq!(
        apply_to_text("Private.kt", private, &edits).unwrap(),
        private.replace("value", "renamed")
    );
    assert_eq!(apply_to_text("Use.java", java, &edits).unwrap(), java);
    let fixed = "package probe\nclass Fixed { @get:JvmName(\"fixedGetter\") val value: Int = 1 }\nfun use(f: Fixed) = f.value\n";
    let fixed_java = "package probe; class Use { int use(Fixed f) { return f.fixedGetter(); } }";
    ws.open("Fixed.kt", fixed.into());
    ws.change("Use.java", fixed_java.into());
    let edits = ws
        .rename("Fixed.kt", fixed.find("val value").unwrap() + 4, "renamed")
        .unwrap();
    assert_eq!(
        apply_to_text("Fixed.kt", fixed, &edits).unwrap(),
        fixed
            .replace("val value", "val renamed")
            .replace("f.value", "f.renamed")
    );
    assert_eq!(
        apply_to_text("Use.java", fixed_java, &edits).unwrap(),
        fixed_java
    );
}

#[test]
fn rename_top_level_function_across_files() {
    let mut ws = Workspace::new();
    let helper_key = "mem:///Helper.kt".to_string();
    let helper = "package app\nfun helper(): Int = 1\n";
    let main_key = "mem:///Main.kt".to_string();
    let main = "package app\nfun main() { helper() }\n";
    ws.open(helper_key.clone(), helper.to_string());
    ws.open(main_key.clone(), main.to_string());

    let edits = ws
        .rename(&main_key, main.find("helper").unwrap(), "renamed")
        .expect("rename should produce edits");

    assert_eq!(
        apply_to_text(&helper_key, helper, &edits).unwrap(),
        "package app\nfun renamed(): Int = 1\n"
    );
    assert_eq!(
        apply_to_text(&main_key, main, &edits).unwrap(),
        "package app\nfun main() { renamed() }\n"
    );
}

#[test]
fn rename_local_variable_excludes_shadowed_homonyms() {
    let mut ws = Workspace::new();
    let key = "mem:///Main.kt".to_string();
    let src =
        "fun f(x: Int): Int {\n    return x + x\n}\nfun g() {\n    val x = 99\n    println(x)\n}\n";
    ws.open(key.clone(), src.to_string());

    let edits = ws
        .rename(&key, src.find("x: Int").unwrap(), "value")
        .expect("local rename should produce edits");
    let edited = apply_to_text(&key, src, &edits).unwrap();

    assert!(edited.contains("fun f(value: Int): Int"));
    assert!(edited.contains("return value + value"));
    assert!(edited.contains("val x = 99"));
    assert!(edited.contains("println(x)"));
}

#[test]
fn rename_rejects_invalid_names_and_library_symbols() {
    let mut ws = Workspace::new();
    let key = "mem:///Main.kt".to_string();
    let src = "fun helper() {}\nfun main() { helper() }\n";
    ws.open(key.clone(), src.to_string());

    assert!(ws
        .rename(&key, src.find("helper").unwrap(), "1bad")
        .is_none());
    assert!(ws
        .rename(&key, src.find("helper").unwrap(), "class")
        .is_none());
}

#[test]
fn prepare_rename_returns_identifier_range() {
    let mut ws = Workspace::new();
    let key = "mem:///Main.kt".to_string();
    let src = "fun helper() {}\nfun main() { helper() }\n";
    ws.open(key.clone(), src.to_string());

    let prepared = ws
        .prepare_rename(&key, src.rfind("helper").unwrap())
        .expect("prepare rename should succeed");

    assert_eq!(prepared.placeholder, "helper");
    assert_eq!(
        &src[prepared.range.start_byte..prepared.range.end_byte],
        "helper"
    );
}

#[test]
fn authored_extension_rename_preserves_import_alias_spelling_and_decoys() {
    let mut ws = Workspace::new();
    let api_key = "mem:///Api.kt";
    let api = "package lib\nclass Value\nfun Value.compat(): Int = 1\nfun direct(v: Value): Int = v.compat()\n";
    let alias_key = "mem:///Alias.kt";
    let alias = "package app\nimport lib.Value\nimport lib.compat as oldApi\nfun use(v: Value): Int = v.oldApi()\n";
    let direct_key = "mem:///Direct.kt";
    let direct = "package direct\nimport lib.Value\nimport lib.compat\nfun use(v: Value): Int = v.compat()\n";
    let decoy_key = "mem:///Decoy.kt";
    let decoy = "package decoy\nclass Other\nfun Other.compat(): Int = 2\nfun Other.oldApi(): Int = 3\nfun use(v: Other): Int = v.compat() + v.oldApi()\n";
    for (key, text) in [
        (api_key, api),
        (alias_key, alias),
        (direct_key, direct),
        (decoy_key, decoy),
    ] {
        ws.open(key.to_string(), text.to_string());
    }
    let edits = ws
        .rename(api_key, api.find("compat").unwrap(), "freshCompat")
        .expect("authored extension rename should succeed");
    assert_eq!(edits.len(), 5, "{edits:?}");
    let expected_api = api.replace("compat", "freshCompat");
    let expected_alias = alias.replace("lib.compat as oldApi", "lib.freshCompat as oldApi");
    let expected_direct = direct.replace("compat", "freshCompat");
    for (key, original, expected) in [
        (api_key, api, expected_api.as_str()),
        (alias_key, alias, expected_alias.as_str()),
        (direct_key, direct, expected_direct.as_str()),
        (decoy_key, decoy, decoy),
    ] {
        let actual = apply_to_text(key, original, &edits).unwrap();
        assert_eq!(actual, expected, "whole buffer {key}");
        ws.change(key, actual);
    }
    let definitions = ws.goto_definition(alias_key, expected_alias.rfind("oldApi").unwrap());
    assert_eq!(definitions.len(), 1, "{definitions:?}");
    assert_eq!(definitions[0].file, api_key);
    assert_eq!(
        &expected_api[definitions[0].start_byte..definitions[0].end_byte],
        "freshCompat"
    );
}

#[test]
fn bare_alias_rename_keeps_authored_policy_and_prepares_cursor_range() {
    let mut ws = Workspace::new();
    let api_key = "mem:///Api.kt";
    let api = "package lib\n\n\nfun compat(): Int = 1\n";
    let alias_key = "mem:///Alias.kt";
    let alias = "package app\nimport lib.compat as oldApi\nfun use(): Int = oldApi()\n";
    ws.open(api_key.to_string(), api.to_string());
    ws.open(alias_key.to_string(), alias.to_string());
    let cursor = alias.rfind("oldApi").unwrap();
    let prepared = ws.prepare_rename(alias_key, cursor).unwrap();
    assert_eq!(prepared.range.file, alias_key);
    assert_eq!(prepared.range.start_byte, cursor);
    assert_eq!(prepared.range.end_byte, cursor + "oldApi".len());
    assert_eq!(prepared.placeholder, "compat");
    let edits = ws.rename(alias_key, cursor, "freshCompat").unwrap();
    assert_eq!(edits.len(), 2, "{edits:?}");
    assert_eq!(
        apply_to_text(api_key, api, &edits).unwrap(),
        api.replace("compat", "freshCompat")
    );
    assert_eq!(
        apply_to_text(alias_key, alias, &edits).unwrap(),
        alias.replace("lib.compat as oldApi", "lib.freshCompat as oldApi")
    );
    assert!(ws
        .rename(alias_key, alias.find("oldApi").unwrap(), "localAlias")
        .is_none());
}

#[test]
fn overloaded_import_alias_rename_refuses_partial_edits() {
    for (api, alias) in [
        (
            "package lib\nclass Value\nfun Value.compat(x: Int): Int = x\nfun Value.compat(x: String): String = x\n",
            "package app\nimport lib.Value\nimport lib.compat as oldApi\nfun one(v: Value): Int = v.oldApi(1)\nfun two(v: Value): String = v.oldApi(\"two\")\n",
        ),
        (
            "package lib\nfun compat(x: Int): Int = x\nfun compat(x: String): String = x\n",
            "package app\nimport lib.compat as oldApi\nfun one(): Int = oldApi(1)\nfun two(): String = oldApi(\"two\")\n",
        ),
    ] {
        let mut ws = Workspace::new();
        ws.open("mem:///Api.kt".to_string(), api.to_string());
        ws.open("mem:///Alias.kt".to_string(), alias.to_string());
        assert!(ws
            .rename("mem:///Api.kt", api.find("compat").unwrap(), "freshCompat")
            .is_none(), "overloaded import must not produce a partial rename: {api}");
    }
}

#[test]
fn same_spelling_import_alias_rename_refuses_partial_edits() {
    for (api, alias) in [
        (
            "package lib\nclass Value\nfun Value.compat(): Int = 1\n",
            "package app\nimport lib.Value\nimport lib.compat as compat\nfun use(v: Value): Int = v.compat()\n",
        ),
        (
            "package lib\nfun compat(): Int = 1\n",
            "package app\nimport lib.compat as compat\nfun use(): Int = compat()\n",
        ),
    ] {
        let mut ws = Workspace::new();
        let api_key = "mem:///Api.kt";
        let alias_key = "mem:///Alias.kt";
        ws.open(api_key.to_string(), api.to_string());
        ws.open(alias_key.to_string(), alias.to_string());
        let definitions = ws.goto_definition(alias_key, alias.rfind("compat").unwrap());
        assert_eq!(definitions.len(), 1, "{definitions:?}");
        assert_eq!(definitions[0].file, api_key);
        assert_eq!(definitions[0].start_byte, api.find("compat").unwrap());
        assert!(ws.rename(api_key, api.find("compat").unwrap(), "freshCompat").is_none());
        assert!(ws.rename(alias_key, alias.rfind("compat").unwrap(), "freshCompat").is_none());
    }
}

#[test]
fn import_alias_rename_refuses_aggregate_candidate_overflow() {
    let mut ws = Workspace::new();
    let api = "package lib\nfun compat(): Int = 1\n";
    let alias = format!(
        "package app\nimport lib.compat as oldApi\nfun use() {{\n{}}}\n",
        "oldApi()\n".repeat(5001)
    );
    ws.open("mem:///Api.kt".to_string(), api.to_string());
    ws.open("mem:///Alias.kt".to_string(), alias);
    assert!(
        ws.rename("mem:///Api.kt", api.find("compat").unwrap(), "freshCompat")
            .is_none(),
        "a capped reference result must not become a partial rename"
    );
}

#[test]
fn local_extension_rename_applies_exact_scoped_overload_edits_to_whole_files() {
    let cases: &[(&str, &str, &str, &str, &str, &str)] = &[
        (
            r#"nested-and-sibling-outerApi"#,
            r#"package app
import vendor.Versioned
import compat.Payload
fun localUse(v:Versioned):Int {
 fun Versioned.oldApi():Payload=Payload()
 if(true) {
  fun Versioned.oldApi():Versioned=this
  val inner:Int=v.oldApi().newApi()
 }
 if(false) {
  fun Versioned.oldApi():Payload=Payload()
  val sibling:Int=v.oldApi().payloadOnly()
 }
 return v.oldApi().payloadOnly()
}
"#,
            r#"package app
import vendor.Versioned
import compat.Payload
fun localUse(v:Versioned):Int {
 fun Versioned.outerApi():Payload=Payload()
 if(true) {
  fun Versioned.oldApi():Versioned=this
  val inner:Int=v.oldApi().newApi()
 }
 if(false) {
  fun Versioned.oldApi():Payload=Payload()
  val sibling:Int=v.oldApi().payloadOnly()
 }
 return v.outerApi().payloadOnly()
}
"#,
            r#" fun Versioned.oldApi():Payload=Payload()
"#,
            r#"return v.oldApi().payloadOnly()"#,
            r#"outerApi"#,
        ),
        (
            r#"nested-and-sibling-innerApi"#,
            r#"package app
import vendor.Versioned
import compat.Payload
fun localUse(v:Versioned):Int {
 fun Versioned.oldApi():Payload=Payload()
 if(true) {
  fun Versioned.oldApi():Versioned=this
  val inner:Int=v.oldApi().newApi()
 }
 if(false) {
  fun Versioned.oldApi():Payload=Payload()
  val sibling:Int=v.oldApi().payloadOnly()
 }
 return v.oldApi().payloadOnly()
}
"#,
            r#"package app
import vendor.Versioned
import compat.Payload
fun localUse(v:Versioned):Int {
 fun Versioned.oldApi():Payload=Payload()
 if(true) {
  fun Versioned.innerApi():Versioned=this
  val inner:Int=v.innerApi().newApi()
 }
 if(false) {
  fun Versioned.oldApi():Payload=Payload()
  val sibling:Int=v.oldApi().payloadOnly()
 }
 return v.oldApi().payloadOnly()
}
"#,
            r#"  fun Versioned.oldApi():Versioned=this
"#,
            r#"v.oldApi().newApi()"#,
            r#"innerApi"#,
        ),
        (
            r#"nested-and-sibling-siblingApi"#,
            r#"package app
import vendor.Versioned
import compat.Payload
fun localUse(v:Versioned):Int {
 fun Versioned.oldApi():Payload=Payload()
 if(true) {
  fun Versioned.oldApi():Versioned=this
  val inner:Int=v.oldApi().newApi()
 }
 if(false) {
  fun Versioned.oldApi():Payload=Payload()
  val sibling:Int=v.oldApi().payloadOnly()
 }
 return v.oldApi().payloadOnly()
}
"#,
            r#"package app
import vendor.Versioned
import compat.Payload
fun localUse(v:Versioned):Int {
 fun Versioned.oldApi():Payload=Payload()
 if(true) {
  fun Versioned.oldApi():Versioned=this
  val inner:Int=v.oldApi().newApi()
 }
 if(false) {
  fun Versioned.siblingApi():Payload=Payload()
  val sibling:Int=v.siblingApi().payloadOnly()
 }
 return v.oldApi().payloadOnly()
}
"#,
            r#"  fun Versioned.oldApi():Payload=Payload()
"#,
            r#"val sibling:Int=v.oldApi().payloadOnly()"#,
            r#"siblingApi"#,
        ),
        (
            r#"typed-overload-intApi"#,
            r#"package app
import vendor.Versioned
import compat.Payload
fun localUse(v:Versioned):Int {
 fun Versioned.oldApi(value:Int):Payload=Payload()
 fun Versioned.oldApi(value:String):Versioned=this
 return v.oldApi(1).payloadOnly()+v.oldApi("text").newApi()
}
"#,
            r#"package app
import vendor.Versioned
import compat.Payload
fun localUse(v:Versioned):Int {
 fun Versioned.intApi(value:Int):Payload=Payload()
 fun Versioned.oldApi(value:String):Versioned=this
 return v.intApi(1).payloadOnly()+v.oldApi("text").newApi()
}
"#,
            r#"fun Versioned.oldApi(value:Int)"#,
            r#"v.oldApi(1)"#,
            r#"intApi"#,
        ),
        (
            r#"typed-overload-stringApi"#,
            r#"package app
import vendor.Versioned
import compat.Payload
fun localUse(v:Versioned):Int {
 fun Versioned.oldApi(value:Int):Payload=Payload()
 fun Versioned.oldApi(value:String):Versioned=this
 return v.oldApi(1).payloadOnly()+v.oldApi("text").newApi()
}
"#,
            r#"package app
import vendor.Versioned
import compat.Payload
fun localUse(v:Versioned):Int {
 fun Versioned.oldApi(value:Int):Payload=Payload()
 fun Versioned.stringApi(value:String):Versioned=this
 return v.oldApi(1).payloadOnly()+v.stringApi("text").newApi()
}
"#,
            r#"fun Versioned.oldApi(value:String)"#,
            r#"v.oldApi("text")"#,
            r#"stringApi"#,
        ),
        (
            r#"local-over-imported-alias-localApi"#,
            r#"package app
import vendor.Versioned
import compat.Payload
import compat.compat as oldApi
fun localUse(v:Versioned):Int {
 fun Versioned.oldApi():Versioned=this
 return v.oldApi().newApi()
}
"#,
            r#"package app
import vendor.Versioned
import compat.Payload
import compat.compat as oldApi
fun localUse(v:Versioned):Int {
 fun Versioned.localApi():Versioned=this
 return v.localApi().newApi()
}
"#,
            r#"fun Versioned.oldApi():Versioned"#,
            r#"v.oldApi().newApi()"#,
            r#"localApi"#,
        ),
    ];
    for &(case_name, source, expected, declaration_anchor, call_anchor, new_name) in cases {
        let mut ws = Workspace::new();
        let key = "mem:///LocalUse.kt";
        let versioned = r#"package vendor; public class Versioned { public int newApi() { return 2; } }
"#;
        let payload = r#"package compat
import vendor.Versioned
class Payload { fun payloadOnly(): Int = 1 }
fun Versioned.compat(): Payload = Payload()
"#;
        let decoy = r#"package other
import vendor.Versioned
import compat.Payload
fun Versioned.oldApi(): Payload = Payload()
fun otherUse(v: Versioned): Int = v.oldApi().payloadOnly()
"#;
        ws.open("mem:///Versioned.java".to_string(), versioned.to_string());
        ws.open("mem:///Payload.kt".to_string(), payload.to_string());
        ws.open("mem:///Other.kt".to_string(), decoy.to_string());
        ws.open(key.to_string(), source.to_string());
        let cursor = source.find(call_anchor).unwrap() + call_anchor.find("oldApi").unwrap();
        let edits = ws.rename(key, cursor, new_name).expect(case_name);
        assert_eq!(edits.len(), 2, "case={case_name} {edits:?}");
        assert!(
            edits.iter().all(|edit| edit.file == key),
            "case={case_name} {edits:?}"
        );
        for (file, original, wanted) in [
            (key, source, expected),
            ("mem:///Versioned.java", versioned, versioned),
            ("mem:///Payload.kt", payload, payload),
            ("mem:///Other.kt", decoy, decoy),
        ] {
            assert_eq!(
                apply_to_text(file, original, &edits).unwrap(),
                wanted,
                "case={case_name} whole file {file}"
            );
        }
        ws.change(key, expected.to_string());
        let renamed_call = call_anchor.replace("oldApi", new_name);
        let renamed_declaration = declaration_anchor.replace("oldApi", new_name);
        let renamed_cursor =
            expected.find(&renamed_call).unwrap() + renamed_call.find(new_name).unwrap();
        let expected_target = expected.find(&renamed_declaration).unwrap()
            + renamed_declaration.find(new_name).unwrap();
        let definitions = ws.goto_definition(key, renamed_cursor);
        assert_eq!(definitions.len(), 1, "case={case_name} {definitions:?}");
        assert_eq!(definitions[0].file, key, "case={case_name}");
        assert_eq!(
            definitions[0].start_byte, expected_target,
            "case={case_name}"
        );
        assert_eq!(
            definitions[0].end_byte,
            expected_target + new_name.len(),
            "case={case_name}"
        );
    }
}
