//! `oxoffice xlsx inspect` names each sheet from 1 with its state and kind,
//! the date system and the class, as text or a schema-1 record, and refuses
//! a file that is not a workbook with the facade's plain message.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/files")
        .join(name)
}

fn oxoffice(args: &[&str], file: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oxoffice"))
        .args(args)
        .arg(file)
        .output()
        .expect("oxoffice runs")
}

#[test]
fn inspect_lists_the_sheets_from_one() {
    let file = corpus("excel/multi.xlsx");
    let output = oxoffice(&["xlsx", "inspect", "--json"], &file);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let record: Value = serde_json::from_slice(&output.stdout).expect("one JSON record");
    assert_eq!(record["schema"], 1);
    assert_eq!(record["class"], "workbook");
    assert_eq!(record["date1904"], false);
    assert_eq!(
        record["sheets"][3],
        json!({"sheet": 4, "name": "Rates", "state": "hidden", "kind": "worksheet"})
    );
    let output = oxoffice(&["xlsx", "inspect"], &file);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("Sheets: 5\n  1. Summary (worksheet)\n"),
        "{text}"
    );
    assert!(text.contains("  4. Rates (worksheet, hidden)\n"), "{text}");

    let output = oxoffice(&["xlsx", "inspect"], &corpus("word/report.docx"));
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "Error: a Word document, not a workbook: open it with oxoffice docx\n"
    );
}
