//! Compiler-valid one-line class bodies used by real unsaved source and KSP output.

use ktlsp::indexer;
use ktlsp::parser::{self, KotlinParser};
use ktlsp::symbol::{Def, SymbolKind};
use ktlsp::workspace::Workspace;

const SOURCE: &str = "package fixture\n@Target(AnnotationTarget.CLASS)\n@Retention(AnnotationRetention.SOURCE)\nannotation class GenerateKsp\n@GenerateKsp\n@processor.GenerateKapt\nclass Source { fun editorUnsaved(): Int = 7 }\n";
const KSP: &str = "package generated\nclass KspKotlin { fun value(): String = \"ksp-kotlin-v1\"; fun versionOne(): String = value() }\n";

fn assert_class_member(source: &str, owner: &str, member: &str) {
    let tree = KotlinParser::new().parse(source);
    assert!(
        !tree.root_node().has_error(),
        "{}",
        tree.root_node().to_sexp()
    );
    let package = parser::package_of(&tree, source);
    let symbols = indexer::extract_symbols(&tree, source, &package);
    assert!(
        symbols
            .iter()
            .any(|symbol| symbol.kind == SymbolKind::Class && symbol.name == owner),
        "{owner}: {symbols:?}"
    );
    assert!(
        symbols
            .iter()
            .any(|symbol| symbol.kind == SymbolKind::Function
                && symbol.name == member
                && symbol.container.as_deref() == Some(owner)),
        "{owner}.{member}: {symbols:?}"
    );
}

fn workspace() -> Workspace {
    let mut ws = Workspace::new();
    ws.open(
        "GenerateKapt.java",
        "package processor; public @interface GenerateKapt {}".to_string(),
    );
    ws.open("Source.kt", SOURCE.to_string());
    ws.open("KspKotlin.kt", KSP.to_string());
    ws
}

fn consumer(
    ws: &mut Workspace,
    java: bool,
    package: &str,
    ty: &str,
    member: &str,
) -> (String, usize) {
    let file = if java { "Use.java" } else { "Use.kt" }.to_string();
    let source = if java {
        format!("package caller; import {package}.{ty}; class Use {{ void run({ty} value) {{ value.{member}(); }} }}")
    } else {
        format!(
            "package caller\nimport {package}.{ty}\nfun run(value: {ty}) {{ value.{member}() }}\n"
        )
    };
    let offset = source.find(&format!("value.{member}")).unwrap() + "value.".len();
    ws.open(&file, source);
    (file, offset)
}

fn expected(ws: &Workspace, file: &str, member: &str) -> Def {
    let entry = ws
        .index
        .lookup_by_name(member)
        .iter()
        .find(|entry| entry.path.as_ref() == file)
        .unwrap();
    Def {
        file: file.to_string(),
        start_byte: entry.sym.start_byte,
        end_byte: entry.sym.end_byte,
    }
}

#[test]
fn annotated_source_keeps_its_class_and_owned_one_line_member() {
    assert_class_member(SOURCE, "Source", "editorUnsaved");
}

#[test]
fn one_line_ksp_expression_body_preserves_a_clean_class_tree() {
    assert_class_member(KSP, "KspKotlin", "versionOne");
}

#[test]
fn source_one_line_member_completion_is_useful_in_java_and_kotlin() {
    let mut ws = workspace();
    for java in [true, false] {
        let (file, offset) = consumer(&mut ws, java, "fixture", "Source", "editorUnsaved");
        let labels: Vec<_> = ws
            .complete(&file, offset + 3, true)
            .unwrap()
            .items
            .into_iter()
            .map(|item| item.label)
            .collect();
        assert!(
            labels.iter().any(|label| label == "editorUnsaved"),
            "{file}: {labels:?}"
        );
    }
}

#[test]
fn source_one_line_member_navigation_selects_the_exact_declaration_in_both_languages() {
    let mut ws = workspace();
    let expected = expected(&ws, "Source.kt", "editorUnsaved");
    for java in [true, false] {
        let (file, offset) = consumer(&mut ws, java, "fixture", "Source", "editorUnsaved");
        assert_eq!(
            ws.goto_definition(&file, offset),
            vec![expected.clone()],
            "{file}"
        );
    }
}

#[test]
fn ksp_one_line_members_keep_useful_completion_and_exact_navigation() {
    let mut ws = workspace();
    let expected = expected(&ws, "KspKotlin.kt", "versionOne");
    for java in [true, false] {
        let (file, offset) = consumer(&mut ws, java, "generated", "KspKotlin", "versionOne");
        let labels: Vec<_> = ws
            .complete(&file, offset + 3, true)
            .unwrap()
            .items
            .into_iter()
            .map(|item| item.label)
            .collect();
        assert!(
            labels.iter().any(|label| label == "versionOne"),
            "{file}: {labels:?}"
        );
        assert_eq!(
            ws.goto_definition(&file, offset),
            vec![expected.clone()],
            "{file}"
        );
    }
}

#[test]
fn source_unsaved_multiline_to_one_line_edit_keeps_owned_members() {
    let mut ws = workspace();
    let multiline = SOURCE.replace(
        "class Source { fun editorUnsaved(): Int = 7 }",
        "class Source {\n    fun editorUnsaved(): Int = 7\n}",
    );
    let changed = SOURCE.replace("editorUnsaved", "editorChanged");
    for text in [multiline.as_str(), SOURCE, changed.as_str(), SOURCE] {
        ws.change("Source.kt", text.to_string());
        let member = if text.contains("editorChanged") {
            "editorChanged"
        } else {
            "editorUnsaved"
        };
        for java in [true, false] {
            let (file, offset) = consumer(&mut ws, java, "fixture", "Source", member);
            let labels: Vec<_> = ws
                .complete(&file, offset + 3, true)
                .unwrap()
                .items
                .into_iter()
                .map(|item| item.label)
                .collect();
            assert!(
                labels.iter().any(|label| label == member),
                "{file}: {labels:?}"
            );
            let removed = if member == "editorChanged" {
                "editorUnsaved"
            } else {
                "editorChanged"
            };
            assert!(
                !labels.iter().any(|label| label == removed),
                "{file}: {labels:?}"
            );
            let target = expected(&ws, "Source.kt", member);
            assert_eq!(ws.goto_definition(&file, offset), vec![target]);
        }
    }
}
