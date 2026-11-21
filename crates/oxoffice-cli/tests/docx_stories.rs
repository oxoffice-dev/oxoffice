//! `oxoffice docx count` counts a text by story kind, and `oxoffice docx text
//! --json` gives the style of the paragraphs of every story, on the Word and
//! Google Docs files of the corpus.

use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::process::{Command, Output};

use oxoffice_opc::OpcPackage;
use serde_json::Value;

fn report(producer: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/files")
        .join(producer)
        .join("report.docx")
}

fn docx(args: &[&str], file: &PathBuf) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oxoffice"))
        .arg("docx")
        .arg(args[0])
        .arg(file)
        .args(&args[1..])
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

#[test]
fn count_gives_each_story_kind_and_the_total() {
    for producer in ["word", "google"] {
        let file = report(producer);
        let record = json(&docx(&["count", "the", "--json"], &file));
        assert_eq!(record["text"], "the", "{producer}");
        assert_eq!(record["revision_view"], "accepted", "{producer}");
        let counts = record["counts"].as_object().unwrap();
        assert!(counts["body"].as_u64().unwrap() > 0, "{producer}: {record}");
        assert_eq!(counts["footer"], 1, "{producer}: 'the operations team'");
        let sum = counts
            .values()
            .map(|count| count.as_u64().unwrap())
            .sum::<u64>();
        assert_eq!(record["total"], sum, "{producer}");
        assert_eq!(
            record["outside_comments"], sum,
            "{producer}: 'the' is in no comment"
        );
        let finance = json(&docx(&["count", "finance", "--json"], &file));
        assert_eq!(
            (&finance["total"], &finance["outside_comments"]),
            (&serde_json::json!(1), &serde_json::json!(0)),
            "{producer}: the comment counts in the total only"
        );

        let plain = docx(&["count", "finance"], &file);
        assert!(plain.status.success(), "{producer}");
        assert_eq!(
            String::from_utf8(plain.stdout).unwrap(),
            "comment: 1\ntotal: 1\noutside comments: 0\n",
            "{producer}"
        );
    }
}

#[test]
fn count_of_an_absent_text_is_zero_and_of_an_empty_text_is_refused() {
    let file = report("word");
    let record = json(&docx(&["count", "not in the report", "--json"], &file));
    assert_eq!(record["total"], 0);
    assert!(record["counts"].as_object().unwrap().is_empty());
    assert!(!docx(&["count", ""], &file).status.success());
}

#[test]
fn text_json_gives_the_style_of_the_paragraphs_of_every_story() {
    for (producer, header_style) in [("word", Some("En-tte")), ("google", None)] {
        let record = json(&docx(&["text", "--json"], &report(producer)));
        let stories = record["stories"].as_array().unwrap();
        let header = stories
            .iter()
            .find(|story| story["kind"] == "header")
            .unwrap();
        assert_eq!(header["part_name"], "/word/header1.xml", "{producer}");
        let item = &header["items"][0];
        assert_eq!(item["kind"], "paragraph", "{producer}");
        assert_eq!(item["style"].as_str(), header_style, "{producer}");
        let comment = stories
            .iter()
            .find(|story| story["kind"] == "comment")
            .unwrap();
        assert!(comment["items"][0]["style"].is_null(), "{producer}");
        let heading = record["paragraphs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|paragraph| paragraph["text"] == "1. Highlights")
            .unwrap();
        assert!(heading["style"].is_string(), "{producer}");
    }
}

#[test]
fn text_json_gives_the_result_of_a_simple_field_in_its_run() {
    // "Page " and a PAGE field as Word writes it, before the last section
    // properties of the Word report.
    let mut package =
        OpcPackage::from_reader(Cursor::new(fs::read(report("word")).unwrap())).unwrap();
    let main = String::from_utf8(package.get_part("/word/document.xml").unwrap().to_vec()).unwrap();
    let at = main.rfind("<w:sectPr").unwrap();
    let field = concat!(
        r#"<w:p><w:r><w:t xml:space="preserve">Page </w:t></w:r>"#,
        r#"<w:fldSimple w:instr=" PAGE "><w:r><w:t>1</w:t></w:r></w:fldSimple></w:p>"#,
    );
    package.set_part(
        "/word/document.xml",
        format!("{}{field}{}", &main[..at], &main[at..]).into_bytes(),
    );
    let path = std::env::temp_dir().join(format!(
        "oxoffice-cli-simple-field-{}.docx",
        std::process::id()
    ));
    package.save(&path).unwrap();
    let record = json(&docx(&["text", "--json"], &path));
    let _ = fs::remove_file(&path);
    let paragraph = record["paragraphs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|paragraph| paragraph["text"] == "Page 1")
        .expect("the paragraph reads its field result");
    let runs = paragraph["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run["text"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(runs, ["Page ", "1"], "as Rust and Python read the runs");
}
