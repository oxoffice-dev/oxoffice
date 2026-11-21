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

/// Run oxoffice and return its standard output, failing on an error.
fn oxoffice(args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_oxoffice"))
        .args(args)
        .output()
        .expect("oxoffice runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// The comment rows of a diff as (change, location before, location after).
fn comment_locations(diff: &Value) -> Vec<(String, Option<String>, Option<String>)> {
    diff["differences"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["story"] == "comment" || row["story"] == "table_cell")
        .map(|row| {
            (
                row["change"].as_str().unwrap().to_owned(),
                row["location_a"].as_str().map(str::to_owned),
                row["location_b"].as_str().map(str::to_owned),
            )
        })
        .collect()
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

#[test]
fn of_two_equal_replies_the_one_left_pairs_with_the_one_on_its_anchor() {
    // James answers "Done." on two clauses; the copy loses the first answer.
    let scratch = Scratch::new("equal");
    let (one, both, second) = (
        scratch.path("one.docx"),
        scratch.path("both.docx"),
        scratch.path("second.docx"),
    );
    let add = |source: &str, anchor: &str, target: &str| {
        oxoffice(&[
            "docx",
            "comment",
            "add",
            source,
            "--anchor",
            anchor,
            "--author",
            "James Patel",
            "--text",
            "Done.",
            "--date",
            "2026-11-22T10:00:00Z",
            "--output",
            target,
            "--json",
        ])["comment_id"]
            .as_i64()
            .unwrap()
    };
    let first = add(&contract("authored"), "Confidential Information", &one);
    add(&one, "shall not exceed", &both);
    oxoffice(&[
        "docx",
        "comment",
        "remove",
        &both,
        "--id",
        &first.to_string(),
        "--output",
        &second,
        "--json",
    ]);
    // The authored contract has seven comments: the answers are the eighth
    // and ninth, and the copy keeps the ninth as its eighth.
    assert_eq!(
        comment_locations(&diff(&both, &second)),
        [(
            "removed".to_owned(),
            Some("comment 8, paragraph 1".to_owned()),
            None
        )]
    );
}

#[test]
fn a_table_in_a_comment_is_still_compared() {
    let scratch = Scratch::new("table");
    let table = |text: &str| {
        format!(
            r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:comment>"#
        )
    };
    let (before, after) = (scratch.path("before.docx"), scratch.path("after.docx"));
    for (target, text) in [(&before, "Net 30"), (&after, "Net 45")] {
        with_part(
            &contract("authored"),
            "word/comments.xml",
            |xml| xml.replacen("</w:comment>", &table(text), 1),
            target,
        );
    }
    let diff = diff(&before, &after);
    let rows = comment_locations(&diff);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, "changed");
    let row = diff["differences"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["change"] == "changed")
        .unwrap();
    assert_eq!(row["story"], "table_cell");
    assert_eq!(
        (row["text_a"].as_str(), row["text_b"].as_str()),
        (Some("Net 30"), Some("Net 45"))
    );
}
