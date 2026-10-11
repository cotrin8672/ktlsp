use ktlsp::workspace::Workspace;

#[test]
fn signature_help_returns_explicit_function_signature() {
    let mut ws = Workspace::new();
    let key = "mem:///Main.kt".to_string();
    let src = "package app\nfun add(a: Int, b: Int): Int = a + b\nfun main() { add(1, 2) }\n";
    ws.open(key.clone(), src.to_string());

    let help = ws
        .signature_help(&key, src.find("2)").unwrap())
        .expect("signature help");

    assert_eq!(help.active_parameter, Some(1));
    assert!(
        help.signatures
            .iter()
            .any(|sig| sig.label == "add(p1: Int, p2: Int): Int"),
        "{help:?}"
    );
}

#[test]
fn signature_help_is_absent_for_unresolved_calls() {
    let mut ws = Workspace::new();
    let key = "mem:///Main.kt".to_string();
    let src = "fun main() { missing(1) }\n";
    ws.open(key.clone(), src.to_string());

    assert!(ws.signature_help(&key, src.find("1)").unwrap()).is_none());
}

#[test]
fn signature_help_is_absent_before_callee_end() {
    let mut ws = Workspace::new();
    let key = "mem:///Main.kt".to_string();
    let src = "fun add(a: Int): Int = a\nfun main() { add(1) }\n";
    ws.open(key.clone(), src.to_string());

    let call_start = src.rfind("add(1").unwrap();

    assert!(ws.signature_help(&key, call_start).is_none());
}

#[test]
fn java_signature_help_returns_method_overloads() {
    let mut ws = Workspace::new();
    let key = "mem:///Main.java".to_string();
    let src = r#"
package app;

class Main {
    String greet(String name) { return name; }
    String greet(String name, int count) { return name; }
    void run() { greet("Ada", 2); }
}
"#;
    ws.open(key.clone(), src.to_string());

    let help = ws
        .signature_help(&key, src.find("2);").unwrap())
        .expect("java signature help");

    assert_eq!(help.active_parameter, Some(1));
    assert!(
        help.signatures
            .iter()
            .any(|sig| sig.label == "greet(p1: String): String"),
        "{help:?}"
    );
    assert!(
        help.signatures
            .iter()
            .any(|sig| sig.label == "greet(p1: String, p2: int): String"),
        "{help:?}"
    );
}

#[test]
fn java_signature_help_keeps_receiver_overloads_while_arguments_are_being_edited() {
    for call in ["greet(\"Ada\", 2)", "this.greet(\"Ada\", 2)"] {
        let mut ws = Workspace::new();
        let key = "mem:///Main.java";
        let src = format!(
            "package app; class Main {{ String greet(String name) {{ return name; }} String greet(String name, int count) {{ return name; }} int greet(boolean flag) {{ return 0; }} void run() {{ {call}; }} }}"
        );
        ws.open(key, src.clone());
        ws.open(
            "mem:///Decoy.java",
            "package app; class Decoy { boolean greet(String name, int first, int second) { return false; } }".into(),
        );
        let help = ws
            .signature_help(key, src.find("2);").unwrap())
            .expect("receiver overload signatures");
        let labels: Vec<_> = help
            .signatures
            .iter()
            .map(|sig| sig.label.as_str())
            .collect();
        assert_eq!(labels.len(), 2, "{call}: {help:?}");
        assert!(labels.contains(&"greet(p1: String): String"), "{help:?}");
        assert!(
            labels.contains(&"greet(p1: String, p2: int): String"),
            "{help:?}"
        );
        assert_eq!(help.active_parameter, Some(1));
    }
}

#[test]
fn java_signature_help_returns_constructor_signature() {
    let mut ws = Workspace::new();
    let key = "mem:///Main.java".to_string();
    let src = r#"
package app;

class Main {
    Main(String name, int count) {}
    void run() { new Main("Ada", 2); }
}
"#;
    ws.open(key.clone(), src.to_string());

    let help = ws
        .signature_help(&key, src.find("2);").unwrap())
        .expect("java constructor signature help");

    assert_eq!(help.active_parameter, Some(1));
    assert!(
        help.signatures
            .iter()
            .any(|sig| sig.label == "Main(p1: String, p2: int): Main"),
        "{help:?}"
    );
}

#[test]
fn local_extension_signature_and_hover_keep_local_facts_over_global_decoy() {
    let mut ws = Workspace::new();
    let key = "mem:///LocalSignature.kt";
    let src = r#"package app
class Versioned
class Payload
fun Versioned.oldApi(value: String): Versioned = this
fun localUse(v: Versioned): Payload {
    fun Versioned.oldApi(value: Int): Payload =
        if (value > 0) oldApi(0) else Payload()
    return v.oldApi(1)
}
"#;
    ws.open(key, src.to_string());
    let declaration = src.find("oldApi(value: Int)").unwrap();
    let recursive_call = src.find("oldApi(0)").unwrap();
    let explicit_call = src.find("v.oldApi(1)").unwrap() + "v.".len();
    let target = ktlsp::symbol::Def {
        file: key.to_string(),
        start_byte: declaration,
        end_byte: declaration + "oldApi".len(),
    };

    for offset in [declaration, recursive_call, explicit_call] {
        let query = ws
            .resolved_symbol_query(key, offset)
            .expect("local extension query");
        assert_eq!(query.reference().status_label(), "ok");
        assert_eq!(query.targets, vec![target.clone()]);
        let summary = query.symbol_summary().expect("local extension facts");
        assert_eq!(summary.file, key);
        assert_eq!(summary.start_byte, target.start_byte);
        assert_eq!(summary.end_byte, target.end_byte);
        let hover = query.hover_markdown().expect("local extension hover");
        assert!(hover.contains("fun oldApi(Int): Payload"), "{hover}");
        assert!(!hover.contains("String"), "global decoy hover: {hover}");
    }
    for call in [recursive_call, explicit_call] {
        let help = ws
            .signature_help(key, call + "oldApi(".len())
            .expect("local extension signature");
        let labels: Vec<_> = help
            .signatures
            .iter()
            .map(|signature| signature.label.as_str())
            .collect();
        assert_eq!(labels, vec!["oldApi(p1: Int): Payload"]);
        assert_eq!(help.active_parameter, Some(0));
    }
}
