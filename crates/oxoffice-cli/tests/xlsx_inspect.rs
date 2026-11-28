//! `oxoffice xlsx inspect` names each sheet from 1 with its state, kind and
//! used range, the date system and the class, as text or a schema-1 record
//! that adds each sheet's page setup, and refuses a file that is not a
//! workbook with the facade's plain message.

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
    assert_eq!(record["recalculation_needed"], false);
    let rates = &record["sheets"][3];
    assert_eq!(
        (
            &rates["sheet"],
            &rates["name"],
            &rates["state"],
            &rates["kind"],
            &rates["used_range"]
        ),
        (
            &json!(4),
            &json!("Rates"),
            &json!("hidden"),
            &json!("worksheet"),
            &json!("A1:B5")
        )
    );
    assert_eq!(rates["page_setup"]["orientation"], "default");
    let output = oxoffice(&["xlsx", "inspect"], &file);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("Sheets: 5\n  1. Summary (worksheet, A1:G10)\n"),
        "{text}"
    );
    assert!(
        text.contains("  4. Rates (worksheet, hidden, A1:B5)\n"),
        "{text}"
    );
    let budget = oxoffice(&["xlsx", "inspect", "--json"], &corpus("excel/budget.xlsx"));
    let record: Value = serde_json::from_slice(&budget.stdout).expect("one JSON record");
    let setup = &record["sheets"][0]["page_setup"];
    assert_eq!(
        (
            &setup["orientation"],
            &setup["paper_size"],
            &setup["print_area"]
        ),
        (&json!("landscape"), &json!(9), &json!("Budget!$A$1:$J$36"))
    );

    let output = oxoffice(&["xlsx", "inspect"], &corpus("word/report.docx"));
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "Error: a Word document, not a workbook: open it with oxoffice docx\n"
    );
}
