//! `oxoffice docx meta set` refuses a property value holding a character XML
//! cannot carry, and writes nothing.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn meta_set_refuses_a_title_xml_cannot_carry_and_writes_nothing() {
    let input =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/files/word/report.docx");
    let output = std::env::temp_dir().join(format!(
        "oxoffice-cli-meta-characters-{}.docx",
        std::process::id()
    ));
    std::fs::remove_file(&output).ok();
    let run = Command::new(env!("CARGO_BIN_EXE_oxoffice"))
        .args(["docx", "meta", "set"])
        .arg(&input)
        .args(["--title", "Q3\u{7}", "-o"])
        .arg(&output)
        .output()
        .expect("oxoffice runs");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(!run.status.success(), "refused: {stderr}");
    assert!(
        stderr.contains("title") && stderr.contains("U+0007"),
        "{stderr}"
    );
    assert!(!output.exists(), "nothing is written");
}
