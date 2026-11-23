//! `oxoffice docx font` addresses one of several records with the same name
//! by its index in `font list`, as LibreOffice writes `Symbol` twice.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

/// A scratch folder of its own for `name`, empty.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("oxoffice-cli-font-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch folder");
    dir
}

fn corpus(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/files")
        .join(path)
}

fn font(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oxoffice"))
        .args(["docx", "font"])
        .args(args)
        .output()
        .expect("oxoffice runs")
}

fn json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("JSON output")
}

/// The `(index, name, face count)` of every record `font list` gives.
fn records(file: &Path) -> Vec<(u64, String, usize)> {
    json(&font(&["list", file.to_str().unwrap(), "--json"]))["fonts"]
        .as_array()
        .expect("a font list")
        .iter()
        .map(|record| {
            (
                record["index"].as_u64().expect("an index"),
                record["name"].as_str().expect("a name").to_owned(),
                record["faces"].as_array().expect("faces").len(),
            )
        })
        .collect()
}

#[test]
fn a_font_name_written_twice_is_embedded_and_removed_by_its_index() {
    let dir = scratch("index");
    let report = corpus("libreoffice/report.docx");
    let listed = records(&report);
    let symbols: Vec<u64> = listed
        .iter()
        .filter(|(_, name, _)| name == "Symbol")
        .map(|(index, _, _)| *index)
        .collect();
    assert_eq!(symbols, [1, 8]);

    // By name, the CLI refuses to pick one and points at the index.
    let out = dir.join("by-name.docx");
    let refused = font(&[
        "remove",
        report.to_str().unwrap(),
        "--name",
        "Symbol",
        "-o",
        out.to_str().unwrap(),
    ]);
    assert!(!refused.status.success());
    let error = String::from_utf8_lossy(&refused.stderr);
    assert!(
        error.contains("2 records are named Symbol") && error.contains("index"),
        "{error}"
    );
    assert!(!out.exists());
    // A name and an index together are refused before anything is read.
    let both = font(&[
        "remove",
        report.to_str().unwrap(),
        "--name",
        "Symbol",
        "--index",
        "8",
        "-o",
        out.to_str().unwrap(),
    ]);
    assert!(!both.status.success());

    let face = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../oxoffice-layout/fonts/NotoSansSymbols2-bullets-subset.ttf");
    let embedded = dir.join("embedded.docx");
    let record = json(&font(&[
        "embed",
        report.to_str().unwrap(),
        "--index",
        "8",
        "--font",
        face.to_str().unwrap(),
        "--license-authorized",
        "--license-identity",
        "SIL Open Font License 1.1",
        "-o",
        embedded.to_str().unwrap(),
        "--json",
    ]));
    assert_eq!(record["index"], 8);
    assert_eq!(record["name"], Value::Null);
    let faces: Vec<usize> = records(&embedded)
        .into_iter()
        .filter(|(_, name, _)| name == "Symbol")
        .map(|(_, _, faces)| faces)
        .collect();
    assert_eq!(faces, [0, 1]);

    let removed = dir.join("removed.docx");
    let record = json(&font(&[
        "remove",
        embedded.to_str().unwrap(),
        "--index",
        "8",
        "-o",
        removed.to_str().unwrap(),
        "--json",
    ]));
    assert_eq!(
        (record["index"].clone(), record["removed"].clone()),
        (8.into(), true.into())
    );
    let names: Vec<String> = records(&removed)
        .into_iter()
        .map(|(_, name, _)| name)
        .collect();
    let before: Vec<String> = listed.into_iter().map(|(_, name, _)| name).collect();
    assert_eq!(names, before[..8]);
    let _ = fs::remove_dir_all(&dir);
}
