//! `oxoffice docx diff` pairs comments by what identifies them across a save
//! (author, text, anchor), not by their order in the comments part: Word and
//! Google Docs write the comments of the contract of the corpus in another
//! order than the file they opened.

use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "oxoffice-cli-diff-comments-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch folder");
        Self(dir)
    }

    fn path(&self, name: &str) -> String {
        self.0.join(name).to_str().unwrap().to_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn contract(producer: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/files")
        .join(producer)
        .join("contract.docx")
        .to_str()
        .unwrap()
        .to_owned()
}

/// `oxoffice docx diff --json` of two files.
fn diff(a: &str, b: &str) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_oxoffice"))
        .args(["docx", "diff", a, b, "--json"])
        .output()
        .expect("oxoffice runs");
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "{error}: {}",
            String::from_utf8_lossy(&output.stderr).into_owned()
        )
    })
}

/// The comment rows of a diff as (change, text before, text after).
fn comment_rows(diff: &Value) -> Vec<(String, Option<String>, Option<String>)> {
    diff["differences"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["story"] == "comment")
        .map(|row| {
            (
                row["change"].as_str().unwrap().to_owned(),
                row["text_a"].as_str().map(str::to_owned),
                row["text_b"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

/// A copy of `source` with one part edited.
fn with_part(source: &str, part: &str, edit: impl Fn(&str) -> String, target: &str) {
    let bytes = fs::read(source).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).unwrap();
        let name = file.name().unwrap().into_owned();
        let mut content = Vec::new();
        file.read_to_end(&mut content).unwrap();
        if name == part {
            content = edit(std::str::from_utf8(&content).unwrap()).into_bytes();
        }
        out.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        out.write_all(&content).unwrap();
    }
    fs::write(target, out.finish().unwrap().into_inner()).unwrap();
}

#[test]
fn comments_word_and_libreoffice_reordered_are_no_difference() {
    // The Google copy is pinned in docx_stories.rs: one resolved comment goes.
    for producer in ["word", "libreoffice"] {
        assert_eq!(
            comment_rows(&diff(&contract("authored"), &contract(producer))),
            [],
            "{producer}"
        );
    }
}

#[test]
fn a_reordered_comment_whose_text_changed_is_one_changed_row() {
    let scratch = Scratch::new("changed");
    let edited = scratch.path("edited.docx");
    with_part(
        &contract("word"),
        "word/comments.xml",
        |xml| {
            assert!(xml.contains("We need the extra month"), "{xml}");
            xml.replacen("We need the extra month", "We need two extra months", 1)
        },
        &edited,
    );
    let rows = comment_rows(&diff(&contract("authored"), &edited));
    assert_eq!(rows.len(), 1, "{rows:?}");
    let (change, before, after) = &rows[0];
    assert_eq!(change, "changed");
    assert!(
        before
            .as_deref()
            .unwrap()
            .contains("We need the extra month")
    );
    assert!(
        after
            .as_deref()
            .unwrap()
            .contains("We need two extra months")
    );
}
