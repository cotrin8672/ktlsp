//! Tests for the reverse-reference index (textDocument/references).

use ktlsp::workspace::Workspace;

#[test]
fn references_finds_all_usages_in_file() {
    let mut ws = Workspace::new();
    let key = "mem:///Main.kt".to_string();
    let src = "package app\n\
               fun helper(x: Int): Int = x\n\
               fun main() {\n\
               \x20\x20\x20\x20helper(1)\n\
               \x20\x20\x20\x20val y = helper(2)\n\
               \x20\x20\x20\x20println(y)\n\
               }\n";
    ws.open(key.clone(), src.to_string());

    // cursor on the last `helper` usage (the `val y = helper(2)` call)
    let cursor = src.rfind("helper").unwrap();

    let with_decl = ws.references(&key, cursor, true);
    let without_decl = ws.references(&key, cursor, false);

    // declaration `fun helper` + two call sites
    assert_eq!(with_decl.len(), 3, "with declaration: {with_decl:?}");
    assert_eq!(
        without_decl.len(),
        2,
        "without declaration: {without_decl:?}"
    );

    // every returned site spans the identifier `helper`
    for r in &with_decl {
        assert_eq!(&src[r.start_byte..r.end_byte], "helper");
    }
    // the declaration site is exactly the `fun helper` name
    let decl_off = src.find("helper").unwrap();
    assert!(
        with_decl.iter().any(|r| r.start_byte == decl_off),
        "declaration site missing from with-declaration result"
    );
    assert!(
        !without_decl.iter().any(|r| r.start_byte == decl_off),
        "declaration site should be excluded when include_declaration=false"
    );
}

#[test]
fn references_distinguishes_shadowed_homonyms() {
    // Two different `x`: a function parameter and an unrelated local. References on the parameter
    // must not include the unrelated local's usages (goto-grade precision via re-resolution).
    let mut ws = Workspace::new();
    let key = "mem:///M.kt".to_string();
    let src = "package app\n\
               fun f(x: Int): Int {\n\
               \x20\x20\x20\x20return x + x\n\
               }\n\
               fun g() {\n\
               \x20\x20\x20\x20val x = 99\n\
               \x20\x20\x20\x20println(x)\n\
               }\n";
    ws.open(key.clone(), src.to_string());

    // cursor on the parameter declaration `x` in `fun f(x: Int)`
    let param = src.find("x: Int").unwrap();
    let refs = ws.references(&key, param, true);

    // param decl + two uses in `x + x` = 3; the `x` in g() must NOT be included
    assert_eq!(refs.len(), 3, "{refs:?}");
    let g_local = src.rfind("val x").unwrap() + "val ".len();
    assert!(
        !refs.iter().any(|r| r.start_byte >= g_local),
        "references leaked into the unrelated local `x` in g(): {refs:?}"
    );
}

#[test]
fn references_on_non_identifier_is_empty() {
    let mut ws = Workspace::new();
    let key = "mem:///W.kt".to_string();
    let src = "fun main() { }\n";
    ws.open(key.clone(), src.to_string());
    let ws_off = src.find("{ }").unwrap() + 1; // whitespace inside the block
    assert!(ws.references(&key, ws_off, true).is_empty());
}

#[test]
fn references_include_file_local_extension_import_aliases() {
    let mut ws = Workspace::new();
    let api_key = "mem:///Api.kt";
    let api = "package lib\nclass Value\nfun Value.compat(): Int = 1\n";
    let alias_key = "mem:///Alias.kt";
    let alias = "package app\nimport lib.Value\nimport lib.compat as oldApi\nfun use(v: Value): Int = v.oldApi()\n";
    let second_key = "mem:///Second.kt";
    let second = "package another\nimport lib.Value\nimport lib.compat as secondApi\nfun use(v: Value): Int = v.secondApi()\n";
    let decoy = "package decoy\nclass Other\nfun Other.oldApi(): Int = 2\nfun use(v: Other): Int = v.oldApi()\n";
    for (key, text) in [
        (api_key, api),
        (alias_key, alias),
        (second_key, second),
        ("mem:///Decoy.kt", decoy),
    ] {
        ws.open(key.to_string(), text.to_string());
    }
    let site = |key: &str, text: &str, name: &str, occurrence: usize| {
        let start = text.match_indices(name).nth(occurrence).unwrap().0;
        (key.to_string(), start, start + name.len())
    };
    let declaration = site(api_key, api, "compat", 0);
    let mut expected = vec![
        declaration.clone(),
        site(alias_key, alias, "compat", 0),
        site(alias_key, alias, "oldApi", 0),
        site(alias_key, alias, "oldApi", 1),
        site(second_key, second, "compat", 0),
        site(second_key, second, "secondApi", 0),
        site(second_key, second, "secondApi", 1),
    ];
    expected.sort();
    for (key, cursor) in [
        (api_key, api.find("compat").unwrap()),
        (alias_key, alias.rfind("oldApi").unwrap()),
    ] {
        let actual = ws
            .references(key, cursor, true)
            .into_iter()
            .map(|r| (r.file, r.start_byte, r.end_byte))
            .collect::<Vec<_>>();
        assert_eq!(actual, expected, "references from {key}");
        let actual = ws
            .references(key, cursor, false)
            .into_iter()
            .map(|r| (r.file, r.start_byte, r.end_byte))
            .collect::<Vec<_>>();
        let without_declaration = expected
            .iter()
            .filter(|site| **site != declaration)
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(actual, without_declaration, "uses from {key}");
    }
}

#[test]
fn references_include_bare_import_alias_without_same_spelling_decoy() {
    let mut ws = Workspace::new();
    let api_key = "mem:///Api.kt";
    let api = "package lib\nfun compat(): Int = 1\n";
    let alias_key = "mem:///Alias.kt";
    let alias = "package app\nimport lib.compat as oldApi\nfun use(): Int = oldApi()\n";
    let decoy_key = "mem:///Decoy.kt";
    let decoy = "package decoy\nfun oldApi(): Int = 2\nfun use(): Int = oldApi()\n";
    for (key, text) in [(api_key, api), (alias_key, alias), (decoy_key, decoy)] {
        ws.open(key.to_string(), text.to_string());
    }
    let refs = ws.references(alias_key, alias.rfind("oldApi").unwrap(), true);
    assert_eq!(refs.len(), 4, "{refs:?}");
    assert!(refs.iter().all(|r| r.file != decoy_key));
    for name in ["compat", "oldApi"] {
        for (start, _) in alias.match_indices(name) {
            assert!(
                refs.iter().any(|r| {
                    r.file == alias_key && r.start_byte == start && r.end_byte == start + name.len()
                }),
                "missing {name} at {start}: {refs:?}"
            );
        }
    }
}

#[test]
fn references_distinguish_local_extension_scopes_and_typed_overloads_after_unsaved_change() {
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
    let cases: &[(&str, &str, &[&[usize]])] = &[
        (
            "nested-and-sibling",
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
            &[&[0, 5], &[1, 2], &[3, 4]],
        ),
        (
            "typed-overload",
            r#"package app
import vendor.Versioned
import compat.Payload
fun localUse(v:Versioned):Int {
 fun Versioned.oldApi(value:Int):Payload=Payload()
 fun Versioned.oldApi(value:String):Versioned=this
 return v.oldApi(1).payloadOnly()+v.oldApi("text").newApi()
}
"#,
            &[&[0, 2], &[1, 3]],
        ),
        (
            "local-over-imported-alias",
            r#"package app
import vendor.Versioned
import compat.Payload
import compat.compat as oldApi
fun localUse(v:Versioned):Int {
 fun Versioned.oldApi():Versioned=this
 return v.oldApi().newApi()
}
"#,
            &[&[1, 2]],
        ),
        (
            "sibling-hidden",
            r#"package app
import vendor.Versioned
import compat.Payload
fun localUse(v:Versioned):Int {
 if(true) { fun Versioned.oldApi():Payload=Payload() }
 return v.oldApi().payloadOnly()
}
"#,
            &[&[0]],
        ),
    ];
    for (case_index, &(case_name, source, groups)) in cases.iter().enumerate() {
        if case_index == 0 {
            ws.open(key.to_string(), source.to_string());
        } else {
            ws.change(key, source.to_string());
        }
        let positions: Vec<_> = source
            .match_indices("oldApi")
            .map(|(start, _)| start)
            .collect();
        for group in groups {
            let mut expected = group
                .iter()
                .map(|&occurrence| {
                    (
                        key.to_string(),
                        positions[occurrence],
                        positions[occurrence] + "oldApi".len(),
                    )
                })
                .collect::<Vec<_>>();
            expected.sort();
            let declaration = positions[group[0]];
            let expected_uses = expected
                .iter()
                .filter(|site| site.1 != declaration)
                .cloned()
                .collect::<Vec<_>>();
            for &occurrence in *group {
                for (include_declaration, wanted) in [(true, &expected), (false, &expected_uses)] {
                    let actual = ws
                        .references(key, positions[occurrence], include_declaration)
                        .into_iter()
                        .map(|site| (site.file, site.start_byte, site.end_byte))
                        .collect::<Vec<_>>();
                    assert_eq!(&actual, wanted, "case={case_name} cursor={occurrence} include_declaration={include_declaration}");
                }
            }
        }
    }
}
