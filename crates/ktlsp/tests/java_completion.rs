//! Fixture-based completion tests for Java source files.
//!
//! Mirrors the Kotlin completion suite: fixtures use `//- file.java` headers and a single
//! `/*^*/` cursor marker. Tests assert label presence/absence and silent-omission positions.

use std::collections::HashSet;

use ktlsp::complete::ShapedCompletions;
use ktlsp::workspace::Workspace;

const CURSOR: &str = "/*^*/";

fn strip_cursor(raw: &str) -> (String, usize) {
    let mut clean = String::with_capacity(raw.len());
    let mut cursor = None;
    let mut rest = raw;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix(CURSOR) {
            assert!(cursor.is_none(), "fixture has more than one /*^*/ cursor");
            cursor = Some(clean.len());
            rest = after;
        } else {
            let ch = rest.chars().next().unwrap();
            clean.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }
    (clean, cursor.expect("fixture must contain a /*^*/ cursor"))
}

struct Fixture {
    files: Vec<(String, String)>,
    cursor: (String, usize),
}

fn parse_fixture(input: &str) -> Fixture {
    let mut raw_files: Vec<(String, String)> = Vec::new();
    let mut name: Option<String> = None;
    let mut buf = String::new();
    for line in input.lines() {
        if let Some(rest) = line.strip_prefix("//-") {
            raw_files.push((
                name.take().unwrap_or_else(|| "Main.java".into()),
                std::mem::take(&mut buf),
            ));
            name = Some(rest.trim().to_string());
        } else {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    raw_files.push((name.unwrap_or_else(|| "Main.java".into()), buf));
    raw_files.retain(|(_, body)| !body.trim().is_empty());

    let mut files = Vec::new();
    let mut cursor = None;
    for (key, raw) in raw_files {
        if raw.contains(CURSOR) {
            let (clean, off) = strip_cursor(&raw);
            assert!(cursor.is_none(), "fixture has more than one /*^*/ cursor");
            cursor = Some((key.clone(), off));
            files.push((key, clean));
        } else {
            files.push((key, raw));
        }
    }
    Fixture {
        files,
        cursor: cursor.expect("fixture must contain a /*^*/ cursor"),
    }
}

fn shaped(input: &str) -> Option<ShapedCompletions> {
    let fx = parse_fixture(input);
    let mut ws = Workspace::new();
    for (key, text) in &fx.files {
        ws.open(key.clone(), text.clone());
    }
    let (cursor_key, cursor_off) = &fx.cursor;
    ws.complete(cursor_key, *cursor_off, true)
}

fn labels(input: &str) -> Option<HashSet<String>> {
    shaped(input).map(|s| s.items.into_iter().map(|i| i.label).collect())
}

fn ordered_labels(input: &str) -> Vec<String> {
    shaped(input)
        .map(|s| s.items.into_iter().map(|i| i.label).collect())
        .unwrap_or_default()
}

fn check_contains(input: &str, expected: &[&str]) {
    let got = labels(input).unwrap_or_else(|| panic!("expected completions, got None:\n{input}"));
    for e in expected {
        assert!(
            got.contains(*e),
            "expected label `{e}` in {got:?}\nfixture:\n{input}"
        );
    }
}

fn check_none(input: &str) {
    assert!(
        shaped(input).is_none(),
        "expected no completions, got {:?}\nfixture:\n{input}",
        shaped(input)
    );
}

#[test]
fn completes_same_file_members() {
    check_contains(
        r#"
//- Main.java
package app;
public class Main {
    public void run() { /*^*/ }
    public void helper() {}
    private int count = 0;
    enum Color { RED, GREEN }
}
"#,
        &["helper", "count", "Color", "RED"],
    );
}

#[test]
fn completes_same_package_top_level_types() {
    check_contains(
        r#"
//- Helper.java
package app;
public class Helper { }

//- Main.java
package app;
public class Main {
    public void run() { Hel/*^*/ }
}
"#,
        &["Helper"],
    );
}

#[test]
fn completes_explicitly_imported_types() {
    check_contains(
        r#"
//- Helper.java
package other;
public class Helper { }

//- Main.java
package app;
import other.Helper;
public class Main {
    public void run() { Hel/*^*/ }
}
"#,
        &["Helper"],
    );
}

#[test]
fn completes_wildcard_imported_types() {
    check_contains(
        r#"
//- Helper.java
package other;
public class Helper { }

//- Main.java
package app;
import other.*;
public class Main {
    public void run() { Hel/*^*/ }
}
"#,
        &["Helper"],
    );
}

#[test]
fn completes_java_lang_when_indexed() {
    check_contains(
        r#"
//- String.java
package java.lang;
public class String { }

//- Main.java
package app;
public class Main {
    public void run() { Str/*^*/ }
}
"#,
        &["String"],
    );
}

#[test]
fn offers_keywords_matching_prefix() {
    check_contains(
        r#"
//- Main.java
package app;
public class Main {
    public void run() { ret/*^*/ }
}
"#,
        &["return"],
    );
}

#[test]
fn auto_imports_cross_package_types() {
    let result = shaped(
        r#"
//- Helper.java
package other;
public class Helper { }

//- Main.java
package app;
public class Main {
    public void run() { Hel/*^*/ }
}
"#,
    )
    .expect("expected completions");
    let helper = result
        .items
        .iter()
        .find(|i| i.label == "Helper")
        .expect("Helper should be offered");
    assert_eq!(
        helper.auto_import.as_ref().map(|i| i.text.as_str()),
        Some("import other.Helper")
    );
}

#[test]
fn empty_prefix_offers_members_and_keywords() {
    let labels = ordered_labels(
        r#"
//- Main.java
package app;
public class Main {
    public void run() { /*^*/ }
    public void helper() {}
}
"#,
    );
    assert!(labels.contains(&"helper".to_string()));
    assert!(labels.contains(&"return".to_string()));
}

#[test]
fn omits_in_string_literal() {
    check_none(
        r#"
//- Main.java
package app;
public class Main {
    public void run() { String s = "Hel/*^*/"; }
}
"#,
    );
}

#[test]
fn omits_in_line_comment() {
    check_none(
        r#"
//- Main.java
package app;
public class Main {
    // Hel/*^*/
    public void run() { }
}
"#,
    );
}

#[test]
fn omits_in_import_declaration() {
    check_none(
        r#"
//- Main.java
package app;
import java.util./*^*/
public class Main { }
"#,
    );
}

#[test]
fn completes_this_members_after_dot() {
    check_contains(
        r#"
//- Main.java
package app;
public class Main {
    public void run() { this./*^*/ }
    public void assist() {}
}
"#,
        &["run", "assist"],
    );
}

#[test]
fn completes_typed_receiver_prefix_without_semicolon() {
    check_contains(
        r#"
//- Helper.java
package app;
public class Helper { public void assist() {} }
//- Main.java
package app;
public class Main { void run(Helper h) { h.as/*^*/ } }
"#,
        &["assist"],
    );
}

#[test]
fn completes_inherited_receiver_members() {
    check_contains(
        r#"
//- Base.java
package app;
public class Base { public void inherited() {} }
//- Helper.java
package app;
public class Helper extends Base { public void assist() {} }
//- Main.java
package app;
public class Main { void run(Helper h) { h.in/*^*/; } }
"#,
        &["inherited"],
    );
}

#[test]
fn completes_kotlin_members_without_kotlin_extensions() {
    let got = labels(
        r#"
//- Helper.kt
package app
class Helper { fun assist() {} }
fun Helper.extensionOnly() {}
//- Main.java
package app;
public class Main { void run(Helper h) { h./*^*/ } }
"#,
    )
    .expect("Kotlin receiver members");
    assert!(got.contains("assist"), "{got:?}");
    assert!(!got.contains("extensionOnly"), "{got:?}");
    assert!(!got.contains("Main"), "{got:?}");
    assert!(!got.contains("run"), "{got:?}");
}

#[test]
fn receiver_completion_preserves_package_identity() {
    let got = labels(
        r#"
//- Visible.java
package lib;
public class Helper { public void assist() {} }
//- Unrelated.java
package other;
public class Helper { public void unrelated() {} }
//- Main.java
package app;
import lib.Helper;
public class Main { void run(Helper h) { h./*^*/ } }
"#,
    )
    .expect("imported receiver members");
    assert!(got.contains("assist"), "{got:?}");
    assert!(!got.contains("unrelated"), "{got:?}");
}

#[test]
fn unknown_receiver_does_not_complete_unrelated_members() {
    check_none(
        r#"
//- Helper.java
package app;
public class Helper { public void assist() {} }
//- Main.java
package app;
public class Main { void run() { missing./*^*/ } }
"#,
    );
}

#[test]
fn receiver_completion_observes_unsaved_java_and_kotlin_changes() {
    let mut ws = Workspace::new();
    let kotlin = "package app\nclass Helper {\n    fun assist() {}\n}\nclass Other {\n    fun unrelated() {}\n}\n";
    let java = "package app; class Main { void run(Helper h) { h.; } }";
    ws.open("Types.kt", kotlin.to_string());
    ws.open("Main.java", java.to_string());
    let labels_at = |ws: &mut Workspace, source: &str| {
        ws.complete("Main.java", source.find("h.").unwrap() + 2, true)
            .expect("receiver members")
            .items
            .into_iter()
            .map(|item| item.label)
            .collect::<HashSet<_>>()
    };
    assert!(labels_at(&mut ws, java).contains("assist"));

    let edited_java = java.replace("Helper h", "Other h");
    ws.change("Main.java", edited_java.clone());
    let got = labels_at(&mut ws, &edited_java);
    assert!(got.contains("unrelated"), "{got:?}");
    assert!(!got.contains("assist"), "{got:?}");

    ws.change("Types.kt", kotlin.replace("unrelated", "changed"));
    let got = labels_at(&mut ws, &edited_java);
    assert!(got.contains("changed"), "{got:?}");
    assert!(!got.contains("unrelated"), "{got:?}");
}

#[test]
fn explains_java_member_completion_and_unknown_receiver() {
    let mut ws = Workspace::new();
    let src = "package app; class Helper { public void assist() {} } class Main { void run(Helper h) { h.as; missing.; } }";
    ws.open("Main.java", src.to_string());
    let explanation = ws
        .explain_completion("Main.java", src.find("h.as").unwrap() + 4)
        .expect("member completion explanation");
    assert_eq!(explanation.context, "member");
    assert_eq!(explanation.status, "ok");
    assert!(explanation.candidates.contains(&"assist".to_string()));

    let unknown = ws
        .explain_completion("Main.java", src.find("missing.").unwrap() + 8)
        .expect("unknown receiver explanation");
    assert_eq!(unknown.context, "member");
    assert_eq!(unknown.status, "unknown");
    assert!(unknown
        .reasons
        .contains(&"unknown-receiver-type".to_string()));
}

#[test]
fn this_recovery_does_not_change_index_or_retain_deleted_members() {
    let mut ws = Workspace::new();
    let src = "package app; class Main { void run() { this.; } void assist() {} }";
    ws.open("Main.java", src.to_string());
    let indexed_symbols = |ws: &Workspace| {
        ws.index
            .entries_for_file("Main.java")
            .iter()
            .map(|entry| {
                (
                    entry.sym.name.clone(),
                    entry.sym.start_byte,
                    entry.sym.end_byte,
                )
            })
            .collect::<Vec<_>>()
    };
    let before = indexed_symbols(&ws);
    let got = ws
        .complete("Main.java", src.find("this.").unwrap() + 5, true)
        .expect("recovered this members");
    assert!(got.items.iter().any(|item| item.label == "assist"));
    assert_eq!(indexed_symbols(&ws), before);

    let edited = src.replace("void assist() {}", "void changed() {}");
    ws.change("Main.java", edited.clone());
    let got = ws
        .complete("Main.java", edited.find("this.").unwrap() + 5, true)
        .expect("updated recovered this members");
    assert!(got.items.iter().any(|item| item.label == "changed"));
    assert!(!got.items.iter().any(|item| item.label == "assist"));
}
