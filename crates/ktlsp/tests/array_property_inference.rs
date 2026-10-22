use ktlsp::workspace::Workspace;

const KOTLIN: &str = "package arrayprobe\nclass Box(seed: Array<String>, counts: IntArray) {\n val inferred = seed\n val explicit: Array<String> = seed\n val ints: IntArray = counts\n}\nclass ForeignBox(seed: foreign.Array<String>) { val values = seed }\n";
const JAVA: &str = "package arrayprobe; class Use { void run(Box box, ForeignBox foreign) { box.getInferred()[0].length(); box.getExplicit()[0].length(); box.getInts()[0].length(); box.getExplicit()[true].length(); foreign.getValues()[0].length(); } }";

fn has_length(ws: &mut Workspace, prefix: &str) -> bool {
    let offset = JAVA.find(prefix).unwrap() + prefix.len();
    ws.complete("/arrays/Use.java", offset, true)
        .is_some_and(|result| result.items.iter().any(|item| item.label == "length"))
}

#[test]
fn java_kotlin_array_getter_elements_use_jvm_string_members_and_current_component() {
    let mut ws = Workspace::new();
    ws.open(
        "/arrays/Arrays.kt",
        "package kotlin\nclass Array<T>\nclass IntArray\n".into(),
    );
    ws.open(
        "/arrays/ForeignArray.kt",
        "package foreign\nclass Array<T>\n".into(),
    );
    ws.open(
        "/arrays/String.java",
        "package java.lang; public final class String { public int length() { return 0; } }".into(),
    );
    ws.open("/arrays/Box.kt", KOTLIN.into());
    ws.open("/arrays/Use.java", JAVA.into());
    ws.assume_index_complete_for_tests();
    for prefix in ["box.getInferred()[0].", "box.getExplicit()[0]."] {
        assert!(has_length(&mut ws, prefix), "{prefix}");
        let targets = ws.goto_definition(
            "/arrays/Use.java",
            JAVA.find(prefix).unwrap() + prefix.len(),
        );
        assert!(
            targets
                .iter()
                .any(|target| target.file == "/arrays/String.java"),
            "{prefix}: {targets:?}"
        );
    }
    for prefix in [
        "box.getInts()[0].",
        "box.getExplicit()[true].",
        "foreign.getValues()[0].",
    ] {
        assert!(!has_length(&mut ws, prefix), "{prefix}");
    }
    ws.change(
        "/arrays/Box.kt",
        KOTLIN.replace(
            "val explicit: Array<String> = seed",
            "val explicit: IntArray = counts",
        ),
    );
    assert!(!has_length(&mut ws, "box.getExplicit()[0]."));
    assert!(has_length(&mut ws, "box.getInferred()[0]."));
    ws.change("/arrays/Box.kt", KOTLIN.into());
    assert!(has_length(&mut ws, "box.getExplicit()[0]."));
}
