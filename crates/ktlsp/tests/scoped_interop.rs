//! One shared semantic view for actual module/source-set classpaths.

use std::collections::HashSet;

use ktlsp::edit::apply_to_text;
use ktlsp::index::Tier;
use ktlsp::java::{self, JavaParser};
use ktlsp::project_model::{FileOrigin, ProjectScope, ScopeVisibility, SourceRepresentation};
use ktlsp::symbol::{Def, SymbolKind};
use ktlsp::workspace::Workspace;

const NEW: &str = "package api; public class Reader { public Payload body() { return null; } public void currentOnly() {} public void accept(String value) {} public static class Factory {} }";
const OLD: &str = "package api; public class Reader { public Payload body() { return null; } public void oldOnly() {} public void accept(int value) {} public static class Factory {} }";
const NEW_PAYLOAD: &str = "package api; public class Payload { public void useful() {} }";
const OLD_PAYLOAD: &str = "package api; public class Payload { public void usefulWrong() {} }";
const A_JAVA: &str = "package app; import api.Reader; class JavaUse { void run(Reader reader) { reader.currentOnly(); reader.body().useful(); reader.accept(\"ok\"); } }";
const B_JAVA: &str = "package app; import api.Reader; class JavaUse { void run(Reader reader) { reader.oldOnly(); reader.body().usefulWrong(); reader.accept(1); } }";
const A_KOTLIN: &str = "package app\nimport api.Reader\nfun run(reader: Reader) { reader.currentOnly(); reader.body().useful(); reader.accept(\"ok\"); val factory: Reader.Factory = Reader.Factory() }\n";
const B_KOTLIN: &str = "package app\nimport api.Reader\nfun run(reader: Reader) { reader.oldOnly(); reader.body().usefulWrong(); reader.accept(1); val factory: Reader.Factory = Reader.Factory() }\n";
const API: &str = "/scope/api/src/main/java/api/Reader.java";
const PAYLOAD: &str = "/scope/api/src/main/java/api/Payload.java";
const OLD_API: &str = "/scope/library/v1/api/Reader.java";
const OLD_PAYLOAD_FILE: &str = "/scope/library/v1/api/Payload.java";
const A_JAVA_FILE: &str = "/scope/a/src/main/java/app/Use.java";
const A_KOTLIN_FILE: &str = "/scope/a/src/main/kotlin/app/Use.kt";
const B_JAVA_FILE: &str = "/scope/b/src/main/java/app/Use.java";
const B_KOTLIN_FILE: &str = "/scope/b/src/main/kotlin/app/Use.kt";

fn scope(module: &str) -> ProjectScope {
    ProjectScope {
        module: module.to_string(),
        source_set: "main".to_string(),
    }
}

fn origin(id: &str, module: Option<&str>) -> FileOrigin {
    FileOrigin {
        origin_id: id.to_string(),
        representation: SourceRepresentation::Source,
        project_scope: module.map(scope),
    }
}

fn visibility(module: &str, origins: &[&str]) -> ScopeVisibility {
    ScopeVisibility {
        scope: scope(module),
        ordered_origins: origins.iter().map(|id| (*id).to_string()).collect(),
    }
}

fn visibilities(a_uses_project: bool) -> Vec<ScopeVisibility> {
    vec![
        visibility("api", &["project-api"]),
        visibility(
            "a",
            if a_uses_project {
                &["a", "project-api", "library-v1"]
            } else {
                &["a", "library-v1"]
            },
        ),
        visibility("b", &["b", "library-v1"]),
    ]
}

fn workspace() -> Workspace {
    let mut ws = Workspace::new();
    for (file, source, id, module) in [
        (API, NEW, "project-api", "api"),
        (PAYLOAD, NEW_PAYLOAD, "project-api", "api"),
        (A_JAVA_FILE, A_JAVA, "a", "a"),
        (A_KOTLIN_FILE, A_KOTLIN, "a", "a"),
        (B_JAVA_FILE, B_JAVA, "b", "b"),
        (B_KOTLIN_FILE, B_KOTLIN, "b", "b"),
    ] {
        ws.open(file, source.to_string());
        ws.index.set_file_origin(file, origin(id, Some(module)));
    }
    for (file, source) in [(OLD_API, OLD), (OLD_PAYLOAD_FILE, OLD_PAYLOAD)] {
        let tree = JavaParser::new().parse(source);
        ws.index
            .replace_file(file, java::extract_symbols(&tree, source), Tier::Durable);
        ws.index.set_file_origin(file, origin("library-v1", None));
    }
    ws.index.replace_scope_visibilities(visibilities(true));
    ws.bump_index_revision();
    ws.assume_index_complete_for_tests();
    ws
}

fn target(ws: &Workspace, file: &str, name: &str, kind: SymbolKind) -> Def {
    let entry = ws
        .index
        .lookup_by_name(name)
        .iter()
        .find(|entry| entry.path.as_ref() == file && entry.sym.kind == kind)
        .unwrap();
    Def {
        file: file.to_string(),
        start_byte: entry.sym.start_byte,
        end_byte: entry.sym.end_byte,
    }
}

fn member_goto(ws: &mut Workspace, file: &str, prefix: &str) -> Vec<Def> {
    let source = ws.doc_text(file).unwrap();
    let offset = source.find(prefix).unwrap() + prefix.rfind('.').unwrap() + 1;
    ws.goto_definition(file, offset)
}

fn labels(ws: &mut Workspace, file: &str, prefix: &str) -> HashSet<String> {
    let source = ws.doc_text(file).unwrap();
    let offset = source.find(prefix).unwrap() + prefix.len();
    ws.complete(file, offset, true)
        .unwrap()
        .items
        .into_iter()
        .map(|item| item.label)
        .collect()
}

#[test]
fn both_languages_use_module_specific_owner_for_completion_goto_and_result_chains() {
    let mut ws = workspace();
    for (file, expected_file, useful, wrong) in [
        (A_JAVA_FILE, API, "currentOnly", "oldOnly"),
        (A_KOTLIN_FILE, API, "currentOnly", "oldOnly"),
        (B_JAVA_FILE, OLD_API, "oldOnly", "currentOnly"),
        (B_KOTLIN_FILE, OLD_API, "oldOnly", "currentOnly"),
    ] {
        let expected = target(&ws, expected_file, "body", SymbolKind::Function);
        assert_eq!(
            member_goto(&mut ws, file, "reader.body"),
            vec![expected],
            "{file}"
        );
        let items = labels(&mut ws, file, "reader.");
        assert!(items.contains(useful), "{file}: {items:?}");
        assert!(!items.contains(wrong), "{file}: {items:?}");
        let items = labels(&mut ws, file, "reader.body().useful");
        let (useful, wrong) = if expected_file == API {
            ("useful", "usefulWrong")
        } else {
            ("usefulWrong", "useful")
        };
        assert!(items.contains(useful), "{file}: result chain {items:?}");
        assert!(!items.contains(wrong), "{file}: result chain {items:?}");
    }
    for (file, expected_file) in [(A_KOTLIN_FILE, API), (B_KOTLIN_FILE, OLD_API)] {
        let expected = target(&ws, expected_file, "Factory", SymbolKind::Class);
        assert_eq!(member_goto(&mut ws, file, "Reader.Factory"), vec![expected]);
    }
}

#[test]
fn valid_module_specific_calls_have_no_false_diagnostics_and_project_mismatch_remains_visible() {
    let mut ws = workspace();
    for file in [A_JAVA_FILE, A_KOTLIN_FILE, B_JAVA_FILE, B_KOTLIN_FILE] {
        assert!(
            ws.diagnostics(file).is_empty(),
            "{file}: {:?}",
            ws.diagnostics(file)
        );
    }
    for file in [A_JAVA_FILE, A_KOTLIN_FILE] {
        let source = ws
            .doc_text(file)
            .unwrap()
            .replace("accept(\"ok\")", "accept(1)");
        ws.change(file, source);
        let diagnostics = ws.diagnostics(file);
        assert_eq!(diagnostics.len(), 1, "{file}: {diagnostics:?}");
        assert_eq!(
            diagnostics[0].code,
            Some(ktlsp::diagnostics::DiagnosticCode::CallShapeMismatch)
        );
    }
}

#[test]
fn unsaved_source_edit_preserves_origin_and_never_recovers_stale_dependency_members() {
    let mut ws = workspace();
    ws.change(
        API,
        NEW.replace("body", "changedBody")
            .replace("currentOnly", "currentChanged"),
    );
    assert_eq!(
        ws.index.file_origin(API),
        Some(&origin("project-api", Some("api")))
    );
    for file in [A_JAVA_FILE, A_KOTLIN_FILE] {
        assert!(
            member_goto(&mut ws, file, "reader.body").is_empty(),
            "stale dependency body in {file}"
        );
        let got = labels(&mut ws, file, "reader.");
        assert!(
            got.contains("changedBody") && got.contains("currentChanged"),
            "{got:?}"
        );
        assert!(
            !got.contains("body") && !got.contains("oldOnly") && !got.contains("currentOnly"),
            "{got:?}"
        );
        let changed = ws
            .doc_text(file)
            .unwrap()
            .replace("reader.body", "reader.changedBody")
            .replace("reader.currentOnly", "reader.currentChanged");
        ws.change(file, changed);
        let expected = target(&ws, API, "changedBody", SymbolKind::Function);
        assert_eq!(
            member_goto(&mut ws, file, "reader.changedBody"),
            vec![expected]
        );
    }
    for file in [B_JAVA_FILE, B_KOTLIN_FILE] {
        let expected = target(&ws, OLD_API, "body", SymbolKind::Function);
        assert_eq!(member_goto(&mut ws, file, "reader.body"), vec![expected]);
    }
}

#[test]
fn scope_snapshot_change_immediately_changes_both_language_results() {
    let mut ws = workspace();
    for file in [A_JAVA_FILE, A_KOTLIN_FILE] {
        assert_eq!(member_goto(&mut ws, file, "reader.body")[0].file, API);
    }
    assert!(ws.index.replace_scope_visibilities(visibilities(false)));
    ws.bump_index_revision();
    for file in [A_JAVA_FILE, A_KOTLIN_FILE] {
        let expected = target(&ws, OLD_API, "body", SymbolKind::Function);
        assert_eq!(member_goto(&mut ws, file, "reader.body"), vec![expected]);
        let got = labels(&mut ws, file, "reader.");
        assert!(
            got.contains("oldOnly") && !got.contains("currentOnly"),
            "{got:?}"
        );
    }
}

#[test]
fn references_and_applied_rename_exclude_same_named_type_in_another_module() {
    let mut ws = workspace();
    let declaration = target(&ws, API, "body", SymbolKind::Function);
    let references = ws.references(API, declaration.start_byte, true);
    assert_eq!(references.len(), 3, "{references:?}");
    let files: HashSet<_> = references.iter().map(|def| def.file.as_str()).collect();
    assert_eq!(files, HashSet::from([API, A_JAVA_FILE, A_KOTLIN_FILE]));
    let edits = ws
        .rename(API, declaration.start_byte, "renamedBody")
        .unwrap();
    assert_eq!(edits.len(), 3, "{edits:?}");
    for file in [API, A_JAVA_FILE, A_KOTLIN_FILE, B_JAVA_FILE, B_KOTLIN_FILE] {
        let source = ws.doc_text(file).unwrap();
        let edited = apply_to_text(file, &source, &edits).unwrap();
        if file == B_JAVA_FILE || file == B_KOTLIN_FILE {
            assert_eq!(edited, source, "another module's dependency caller changed");
        } else {
            assert_eq!(edited, source.replace("body", "renamedBody"), "{file}");
        }
        ws.change(file, edited);
    }
    for file in [A_JAVA_FILE, A_KOTLIN_FILE] {
        let expected = target(&ws, API, "renamedBody", SymbolKind::Function);
        assert_eq!(
            member_goto(&mut ws, file, "reader.renamedBody"),
            vec![expected]
        );
    }
    for file in [B_JAVA_FILE, B_KOTLIN_FILE] {
        let expected = target(&ws, OLD_API, "body", SymbolKind::Function);
        assert_eq!(member_goto(&mut ws, file, "reader.body"), vec![expected]);
    }
}

#[test]
fn unowned_file_does_not_guess_global_members_or_emit_absence_errors() {
    let mut ws = workspace();
    let file = "/scope/unowned/Use.kt";
    let source =
        "package app\nimport api.Reader\nfun run(reader: Reader) { reader.currentOnly() }\n";
    ws.open(file, source.to_string());
    assert!(member_goto(&mut ws, file, "reader.currentOnly").is_empty());
    assert!(ws.diagnostics(file).is_empty());
    let query = ws
        .explain_resolution(file, source.find("currentOnly").unwrap())
        .unwrap();
    assert_eq!(query.status, "unknown");
    assert!(
        query
            .reasons
            .iter()
            .any(|reason| reason == "query-scope-incomplete"),
        "{:?}",
        query.reasons
    );
}

#[test]
fn signature_help_never_borrows_an_unrelated_visible_members_signature() {
    let mut ws = workspace();
    for (file, source) in [
        (
            A_JAVA_FILE,
            "package app; class JavaUse { void run() { unknown.accept(1); } }",
        ),
        (
            A_KOTLIN_FILE,
            "package app\nfun run() { unknown.accept(1) }\n",
        ),
    ] {
        ws.change(file, source.to_string());
        let offset = source.find("accept(").unwrap() + "accept(".len();
        assert!(
            ws.signature_help(file, offset).is_none(),
            "{file}: unknown receiver must not borrow Reader.accept"
        );
    }
    let source = "package app\nfun top(value: String): String = value\nfun run() { top(\"ok\") }\n";
    ws.change(A_KOTLIN_FILE, source.to_string());
    let offset = source.find("top(\"ok").unwrap() + "top(".len();
    let signatures = ws.signature_help(A_KOTLIN_FILE, offset).unwrap();
    assert_eq!(signatures.signatures.len(), 1);
    assert!(signatures.signatures[0]
        .label
        .starts_with("top(p1: String)"));
}

#[test]
fn reverse_hierarchy_resolves_each_subtype_in_its_own_module() {
    let mut ws = workspace();
    let children = [
        ("/scope/a/src/main/java/app/JavaChild.java", "a", "package app; import api.Reader; import api.Payload; class JavaChild extends Reader { @Override public Payload body() { return null; } }"),
        ("/scope/a/src/main/kotlin/app/KotlinChild.kt", "a", "package app\nimport api.Reader\nimport api.Payload\nclass KotlinChild : Reader() { override fun body(): Payload = Payload() }\n"),
        ("/scope/b/src/main/java/app/JavaChild.java", "b", "package app; import api.Reader; import api.Payload; class JavaChild extends Reader { @Override public Payload body() { return null; } }"),
        ("/scope/b/src/main/kotlin/app/KotlinChild.kt", "b", "package app\nimport api.Reader\nimport api.Payload\nclass KotlinChild : Reader() { override fun body(): Payload = Payload() }\n"),
    ];
    for (file, module, source) in children {
        ws.open(file, source.to_string());
        ws.index.set_file_origin(file, origin(module, Some(module)));
    }
    ws.bump_index_revision();
    for (file, module, _) in children {
        let name = if file.ends_with(".java") {
            "JavaChild"
        } else {
            "KotlinChild"
        };
        let declaration = target(&ws, file, name, SymbolKind::Class);
        let entry = ktlsp::hierarchy::entry_for_name_range(
            &ws.index,
            file,
            declaration.start_byte,
            declaration.end_byte,
        )
        .unwrap();
        let item = ktlsp::hierarchy::item_from_entry(&entry);
        let parents: Vec<_> = ws
            .type_supertypes(&item)
            .into_iter()
            .map(|item| item.file)
            .collect();
        assert_eq!(
            parents,
            vec![if module == "a" { API } else { OLD_API }.to_string()],
            "{file}: imported superclass uses the child's scope"
        );
    }
    for (parent, module) in [(API, "a"), (OLD_API, "b")] {
        let declaration = target(&ws, parent, "Reader", SymbolKind::Class);
        let entry = ktlsp::hierarchy::entry_for_name_range(
            &ws.index,
            parent,
            declaration.start_byte,
            declaration.end_byte,
        )
        .unwrap();
        let item = ktlsp::hierarchy::item_from_entry(&entry);
        let expected: HashSet<_> = children
            .iter()
            .filter(|(_, child_module, _)| *child_module == module)
            .map(|(file, _, _)| (*file).to_string())
            .collect();
        let subtypes: HashSet<_> = ws
            .type_subtypes(&item)
            .into_iter()
            .map(|item| item.file)
            .collect();
        assert_eq!(subtypes, expected, "{parent}: declaration's classpath cannot see consumer modules, each subtype has its own scope");
        let method = target(&ws, parent, "body", SymbolKind::Function);
        let entry = ktlsp::hierarchy::entry_for_name_range(
            &ws.index,
            parent,
            method.start_byte,
            method.end_byte,
        )
        .unwrap();
        let implementations: HashSet<_> = ktlsp::hierarchy::implementations(&ws.index, &entry)
            .into_iter()
            .map(|def| def.file)
            .collect();
        assert_eq!(
            implementations, expected,
            "{parent}: another version's overrides must be excluded"
        );
    }
}

#[test]
fn closed_salsa_queries_and_synthetic_completion_follow_scope_and_unsaved_type_changes() {
    let mut ws = workspace();
    // Different return names make stale expression-cache reuse observable, not
    // merely a new member lookup on the same nominal type.
    let old = OLD.replace("Payload body", "OldPayload body");
    let tree = JavaParser::new().parse(&old);
    ws.index
        .replace_file(OLD_API, java::extract_symbols(&tree, &old), Tier::Durable);
    let old_payload = OLD_PAYLOAD.replace("class Payload", "class OldPayload");
    let tree = JavaParser::new().parse(&old_payload);
    ws.index.replace_file(
        OLD_PAYLOAD_FILE,
        java::extract_symbols(&tree, &old_payload),
        Tier::Durable,
    );
    ws.bump_index_revision();
    let root = std::env::temp_dir().join(format!(
        "ktlsp-scoped-closed-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("Use.kt");
    std::fs::write(&path, A_KOTLIN).unwrap();
    let file = ktlsp::paths::file_key(&path);
    ws.open(&file, A_KOTLIN.to_string());
    ws.index.set_file_origin(&file, origin("a", Some("a")));
    ws.close(&file);
    for _ in 0..2 {
        let expected = target(&ws, PAYLOAD, "useful", SymbolKind::Function);
        assert_eq!(
            member_goto(&mut ws, &file, "reader.body().useful"),
            vec![expected]
        );
        let got = labels(&mut ws, &file, "reader.body().use");
        assert!(
            got.contains("useful") && !got.contains("usefulWrong"),
            "{got:?}"
        );
    }
    ws.index.replace_scope_visibilities(visibilities(false));
    ws.bump_index_revision();
    let got = labels(&mut ws, &file, "reader.body().use");
    assert!(
        got.contains("usefulWrong") && !got.contains("useful"),
        "scope change: {got:?}"
    );
    assert!(member_goto(&mut ws, &file, "reader.body().useful").is_empty());
    ws.index.replace_scope_visibilities(visibilities(true));
    ws.change(API, NEW.replace("Payload body", "UpdatedPayload body"));
    let updated_file = "/scope/api/src/main/java/api/UpdatedPayload.java";
    ws.open(
        updated_file,
        "package api; public class UpdatedPayload { public void usefulChanged() {} }".to_string(),
    );
    ws.index
        .set_file_origin(updated_file, origin("project-api", Some("api")));
    ws.bump_index_revision();
    let got = labels(&mut ws, &file, "reader.body().use");
    assert!(
        got.contains("usefulChanged") && !got.contains("useful") && !got.contains("usefulWrong"),
        "unsaved source: {got:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        A_KOTLIN,
        "query must not rewrite source"
    );
    assert!(root.starts_with(std::env::temp_dir()));
    std::fs::remove_dir_all(root).unwrap();
}
