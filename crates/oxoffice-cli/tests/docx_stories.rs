//! `oxoffice docx count` counts a text by story kind, and `oxoffice docx text
//! --json` gives the style of the paragraphs of every story, on the Word and
//! Google Docs files of the corpus. The read commands keep their JSON keys,
//! and `oxoffice docx diff` shows what each producer changed in a corpus file
//! and nothing else.

use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::process::{Command, Output};

use oxoffice_opc::OpcPackage;
use serde_json::Value;

#[path = "support/snapshots.rs"]
mod snapshots;

use snapshots::{assert_keys, assert_snapshot, corpus, every_copy, snapshot_path};

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

#[test]
fn a_header_table_reads_in_place_and_its_story_stays_one_json_entry() {
    // The Word report with a letterhead: a line, a one-row table, a line.
    let mut package =
        OpcPackage::from_reader(Cursor::new(fs::read(report("word")).unwrap())).unwrap();
    let header = package
        .parts
        .keys()
        .find(|part| part.starts_with("/word/header"))
        .expect("the report has a header")
        .clone();
    let line = |text: &str| format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>");
    let cell = |text: &str| format!("<w:tc>{}</w:tc>", line(text));
    package.set_part(
        &header,
        format!(
            concat!(
                r#"<w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#,
                r#"{}<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="4000"/><w:gridCol w:w="4000"/></w:tblGrid>"#,
                "<w:tr>{}{}</w:tr></w:tbl>{}</w:hdr>"
            ),
            line("Letterhead"),
            cell("Reference"),
            cell("NW-2026-114"),
            line("Confidential"),
        )
        .into_bytes(),
    );
    let path = std::env::temp_dir().join(format!(
        "oxoffice-cli-header-table-{}.docx",
        std::process::id()
    ));
    package.save(&path).unwrap();
    let plain = docx(&["text"], &path);
    let markdown_path = path.with_extension("md");
    let converted = Command::new(env!("CARGO_BIN_EXE_oxoffice"))
        .args(["docx", "convert"])
        .arg(&path)
        .args(["--to", "md", "-o"])
        .arg(&markdown_path)
        .output()
        .expect("oxoffice runs");
    let record = json(&docx(&["text", "--json"], &path));
    let markdown = fs::read_to_string(&markdown_path).unwrap_or_default();
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(&markdown_path);
    assert!(
        converted.status.success(),
        "{}",
        String::from_utf8_lossy(&converted.stderr)
    );

    // Plain text and Markdown read the table where it sits.
    let in_order = |text: &str| {
        let at = |needle: &str| {
            text.find(needle)
                .unwrap_or_else(|| panic!("{needle}: {text}"))
        };
        at("Letterhead") < at("Reference")
            && at("Reference") < at("NW-2026-114")
            && at("NW-2026-114") < at("Confidential")
    };
    let plain = String::from_utf8(plain.stdout).unwrap();
    assert!(in_order(&plain), "{plain}");
    assert!(in_order(&markdown), "{markdown}");

    // JSON keeps one entry per story, its items in story order.
    let headers: Vec<&Value> = record["stories"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|story| story["part_name"] == header.as_str() && story["kind"] == "header")
        .collect();
    assert_eq!(headers.len(), 1, "{record}");
    let texts: Vec<&str> = headers[0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, ["Letterhead", "Confidential"]);
}

fn oxoffice(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oxoffice"))
        .args(args)
        .output()
        .expect("oxoffice runs")
}

fn read_json(args: &[&str]) -> Value {
    json(&oxoffice(args))
}

fn path_of(file: &str) -> String {
    corpus(file).to_str().unwrap().to_owned()
}

#[test]
fn the_read_commands_keep_their_json_keys() {
    let contract = ["word/contract.docx", "google/contract.docx"].map(path_of);
    let report = path_of("word/report.docx");
    let text = contract
        .iter()
        .chain([&report])
        .map(|file| read_json(&["docx", "text", file, "--json"]))
        .collect::<Vec<_>>();
    assert_keys("docx-text", &text);
    let revisions = contract
        .iter()
        .map(|file| read_json(&["docx", "revision", "list", file, "--json"]))
        .collect::<Vec<_>>();
    assert_keys("docx-revision-list", &revisions);
    let comments = contract
        .iter()
        .map(|file| read_json(&["docx", "comment", "list", file, "--json"]))
        .collect::<Vec<_>>();
    assert_keys("docx-comment-list", &comments);
    assert_keys(
        "docx-layout",
        &[read_json(&["docx", "layout", &report, "--json"])],
    );
    // LibreOffice adds header and footer paragraphs and drops an empty one:
    // both kinds of row, with and without a side.
    let diff = read_json(&[
        "docx",
        "diff",
        &path_of("authored/report.docx"),
        &path_of("libreoffice/report.docx"),
        "--json",
    ]);
    assert!(diff["added"].as_u64().unwrap() > 0 && diff["removed"].as_u64().unwrap() > 0);
    assert_keys("docx-diff", &[diff]);
}

/// The families whose producer copies are compared with their source: the
/// report and the contract on a pull request, every family otherwise.
fn diffed_families() -> Vec<String> {
    if !every_copy() {
        return vec!["report".into(), "contract".into()];
    }
    let mut families = fs::read_dir(corpus("authored"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "docx"))
        .map(|path| path.file_stem().unwrap().to_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    families.sort();
    families
}

#[test]
fn a_producer_copy_differs_from_its_source_only_by_what_that_producer_changes() {
    let mut read = Vec::new();
    for family in diffed_families() {
        for producer in ["word", "google", "libreoffice"] {
            let copy = format!("{producer}/{family}.docx");
            assert!(
                corpus(&copy).exists(),
                "{copy}: every family has this producer's copy"
            );
            let diff = read_json(&[
                "docx",
                "diff",
                &path_of(&format!("authored/{family}.docx")),
                &path_of(&copy),
                "--json",
            ]);
            let short = |text: &Value| {
                text.as_str()
                    .map(|text| text.chars().take(40).collect::<String>())
                    .unwrap_or_else(|| "-".into())
            };
            let rows = diff["differences"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| {
                    format!(
                        "{} {}: {:?} -> {:?}",
                        row["change"].as_str().unwrap(),
                        row["story"].as_str().unwrap(),
                        short(&row["text_a"]),
                        short(&row["text_b"]),
                    )
                })
                .collect::<Vec<_>>();
            assert_snapshot(&format!("producer-diff/{producer}-{family}"), &rows);
            read.push(format!("{producer}-{family}.txt"));
        }
    }
    if every_copy() {
        // No snapshot is left behind by a copy that went.
        let mut kept = fs::read_dir(snapshot_path("producer-diff/x").parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        kept.sort();
        read.sort();
        assert_eq!(
            kept, read,
            "every producer-diff snapshot belongs to a corpus copy"
        );
    }
}

#[test]
fn a_comment_added_to_a_copy_is_the_one_difference() {
    let dir =
        std::env::temp_dir().join(format!("oxoffice-cli-diff-comment-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let source = path_of("google/report.docx");
    let edited = dir.join("edited.docx");
    let added = oxoffice(&[
        "docx",
        "comment",
        "add",
        &source,
        "--anchor",
        "Highlights",
        "--author",
        "Reviewer",
        "--text",
        "Check this figure",
        "--undated",
        "--output",
        edited.to_str().unwrap(),
    ]);
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let diff = read_json(&["docx", "diff", &source, edited.to_str().unwrap(), "--json"]);
    let _ = fs::remove_dir_all(&dir);
    let rows = diff["differences"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{diff}");
    assert_eq!(
        (&rows[0]["change"], &rows[0]["story"], &rows[0]["text_b"]),
        (
            &serde_json::json!("added"),
            &serde_json::json!("comment"),
            &serde_json::json!("Check this figure")
        ),
        "{diff}"
    );
}

// Known failure #356: diff matches comments by their order in comments.xml,
// which Word and LibreOffice rewrite (see the producer-diff snapshots of the
// contract). When it passes, drop `should_panic` and update the
// producer-diff snapshots of the contract.
#[test]
#[should_panic(expected = "no comment of the Word copy differs")]
fn comments_a_producer_reordered_are_no_difference() {
    let diff = read_json(&[
        "docx",
        "diff",
        &path_of("authored/contract.docx"),
        &path_of("word/contract.docx"),
        "--json",
    ]);
    let comments = diff["differences"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["story"] == "comment")
        .collect::<Vec<_>>();
    // Word keeps every comment of the contract, threads and texts unchanged.
    assert!(
        comments.is_empty(),
        "no comment of the Word copy differs: {comments:?}"
    );
}
