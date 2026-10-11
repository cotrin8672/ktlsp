use ktlsp::edit::apply_to_text;
use ktlsp::workspace::Workspace;

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
