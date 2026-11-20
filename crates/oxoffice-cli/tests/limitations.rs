//! `oxoffice limitations` lists every entry of limitations.json, filters by
//! format, and every entry carries what an agent needs to act on it.

use std::collections::BTreeSet;
use std::process::Command;

fn limitations(args: &[&str]) -> Vec<serde_json::Value> {
    let output = Command::new(env!("CARGO_BIN_EXE_oxoffice"))
        .arg("limitations")
        .args(args)
        .arg("--json")
        .output()
        .expect("oxoffice runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("the output is a JSON array")
}

#[test]
fn every_limitation_has_an_id_a_format_a_workaround_and_an_issue() {
    let all = limitations(&[]);
    assert!(!all.is_empty());
    let mut ids = BTreeSet::new();
    for entry in &all {
        let id = entry["id"].as_str().expect("id");
        assert!(ids.insert(id.to_owned()), "duplicate id {id}");
        let format = entry["format"].as_str().expect("format");
        assert!(
            ["docx", "pptx", "xlsx", "all"].contains(&format),
            "{id}: format {format}"
        );
        for key in ["area", "title", "symptom", "workaround"] {
            let value = entry[key].as_str().unwrap_or_default();
            assert!(!value.trim().is_empty(), "{id}: empty {key}");
        }
        assert!(
            entry["issue"].as_u64().is_some_and(|n| n > 0),
            "{id}: no issue number"
        );
    }
}

#[test]
fn a_format_keeps_only_its_limitations() {
    let all = limitations(&[]);
    let docx = limitations(&["docx"]);
    let pptx = limitations(&["pptx"]);
    assert!(
        docx.iter()
            .all(|e| e["format"] == "docx" || e["format"] == "all")
    );
    assert!(
        pptx.iter()
            .all(|e| e["format"] == "pptx" || e["format"] == "all")
    );
    assert_eq!(docx.len() + pptx.len(), all.len());
}

#[test]
fn the_text_listing_names_every_title() {
    let output = Command::new(env!("CARGO_BIN_EXE_oxoffice"))
        .arg("limitations")
        .output()
        .expect("oxoffice runs");
    let text = String::from_utf8(output.stdout).expect("utf-8");
    for entry in limitations(&[]) {
        assert!(
            text.contains(entry["title"].as_str().unwrap()),
            "missing {}",
            entry["id"]
        );
    }
}
