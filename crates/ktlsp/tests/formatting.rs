use ktlsp::format::{format_document, FormatterConfig};

#[test]
fn formatter_returns_no_edits_when_output_is_unchanged() {
    let config = if cfg!(windows) {
        FormatterConfig {
            command: "powershell.exe".to_string(),
            args: vec![
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                "[Console]::OpenStandardInput().CopyTo([Console]::OpenStandardOutput())".into(),
            ],
        }
    } else {
        FormatterConfig {
            command: "/bin/cat".to_string(),
            args: Vec::new(),
        }
    };

    let edits = format_document("mem:///Main.kt", "fun main() {}\n", &config)
        .expect("the platform passthrough formatter should be available");

    assert!(edits.is_empty());
}

#[test]
fn formatter_returns_none_when_command_is_missing() {
    let config = FormatterConfig {
        command: "/definitely/missing/ktlsp-formatter".to_string(),
        args: Vec::new(),
    };

    assert!(format_document("mem:///Main.kt", "fun main() {}\n", &config).is_none());
}
