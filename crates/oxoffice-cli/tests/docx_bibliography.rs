//! `oxoffice docx bibliography`: sources in and out as JSON, the style,
//! citations after text or at a run boundary, bibliographies and the
//! update of their results.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use oxoffice_docx::Document;
use oxoffice_opc::OpcPackage;
use serde_json::{Value, json};

/// A scratch folder of its own for `name`.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "oxoffice-cli-bibliography-{name}-{}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("scratch folder");
    dir
}

fn document(dir: &Path, paragraphs: &[&str]) -> PathBuf {
    let mut doc = Document::new();
    for text in paragraphs {
        doc.add_paragraph(text);
    }
    let path = dir.join("input.docx");
    doc.save(&path).expect("the document saves");
    path
}

fn bibliography(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oxoffice"))
        .args(["docx", "bibliography"])
        .args(args)
        .output()
        .expect("oxoffice runs")
}

/// Run a command that must succeed and return its standard output.
fn ok(args: &[&str]) -> String {
    let run = bibliography(args);
    assert!(
        run.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    String::from_utf8(run.stdout).expect("output is UTF-8")
}

/// Run a command that must fail and return its standard error.
fn refused(args: &[&str]) -> String {
    let run = bibliography(args);
    assert!(!run.status.success(), "{args:?} succeeded");
    String::from_utf8(run.stderr).expect("errors are UTF-8")
}

fn json_of(args: &[&str]) -> Value {
    serde_json::from_str(&ok(args)).expect("output is JSON")
}

fn write_json(dir: &Path, name: &str, value: &Value) -> String {
    let path = dir.join(name);
    fs::write(&path, value.to_string()).unwrap();
    path.to_str().unwrap().to_owned()
}

fn texts(path: &str) -> Vec<String> {
    Document::open(path)
        .expect("the output opens")
        .paragraphs()
        .iter()
        .map(|paragraph| paragraph.text())
        .collect()
}

/// The field instructions of the main document part, in order, trimmed.
fn instructions(path: &str) -> Vec<String> {
    let bytes = fs::read(path).unwrap();
    let package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    let part = package.main_document_part().expect("main document part");
    let xml = String::from_utf8(package.get_part(&part).unwrap().to_vec()).unwrap();
    xml.split("<w:instrText")
        .skip(1)
        .map(|rest| {
            let text = &rest[rest.find('>').unwrap() + 1..rest.find("</w:instrText>").unwrap()];
            text.replace("&quot;", "\"")
                .replace("&amp;", "&")
                .trim()
                .to_owned()
        })
        .collect()
}

fn smith() -> Value {
    json!({
        "tag": "Smith20",
        "kind": "book",
        "contributors": [{"role": "author", "people": [{"last": "Smith", "first": "Ada"}]}],
        "fields": [["title", "A history"], ["year", "2020"], ["publisher", "Press"]],
    })
}

fn doe() -> Value {
    json!({
        "tag": "Doe19",
        "kind": "journal_article",
        "guid": "{11111111-2222-4333-8444-555555555555}",
        "locale": 1033,
        "contributors": [
            {"role": "author", "people": [{"last": ["Doe"], "first": ["Jane", "Q"], "middle": []}]},
            {"role": "editor", "people": [{"last": "Roe"}]},
        ],
        "fields": [["title", "A study"], ["year", "2019"], ["journal_name", "Liver"], ["pages", "1-9"]],
    })
}

#[test]
fn sources_go_in_and_come_back_as_the_same_json() {
    let dir = scratch("sources");
    let input = document(&dir, &["Text"]);
    let input = input.to_str().unwrap();
    let added = dir.join("added.docx");
    let added = added.to_str().unwrap();
    let sources = write_json(&dir, "sources.json", &json!([smith(), doe()]));

    let record = json_of(&[
        "add-source",
        input,
        "--source",
        &sources,
        "-o",
        added,
        "--json",
    ]);
    let guids = record["sources"].as_array().unwrap();
    assert_eq!(guids[0]["tag"], "Smith20");
    assert!(guids[0]["guid"].as_str().unwrap().starts_with('{'));
    assert_eq!(guids[1]["guid"], "{11111111-2222-4333-8444-555555555555}");

    let listed = json_of(&["sources", added, "--json"]);
    let listed = listed["sources"].as_array().unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0]["guid"], guids[0]["guid"]);
    assert_eq!(listed[0]["source_type"], "Book");
    assert_eq!(listed[0]["diagnostics"], json!([]));
    // What was written comes back, with every name part as an array.
    assert_eq!(
        listed[1]["source"],
        json!({
            "tag": "Doe19",
            "kind": "journal_article",
            "guid": "{11111111-2222-4333-8444-555555555555}",
            "locale": 1033,
            "contributors": [
                {"role": "author", "people": [{"last": ["Doe"], "first": ["Jane", "Q"], "middle": []}]},
                {"role": "editor", "people": [{"last": ["Roe"], "first": [], "middle": []}]},
            ],
            "fields": [["title", "A study"], ["year", "2019"], ["journal_name", "Liver"], ["pages", "1-9"]],
        })
    );
    assert_eq!(
        listed[0]["source"]["contributors"],
        json!([{"role": "author", "people": [{"last": ["Smith"], "first": ["Ada"], "middle": []}]}])
    );

    // A listed source, edited, replaces the one with its tag and keeps its GUID.
    let mut edited = listed[0]["source"].clone();
    edited["fields"][0] = json!(["title", "A longer history"]);
    edited.as_object_mut().unwrap().remove("guid");
    let edited = write_json(&dir, "edited.json", &edited);
    let replaced = dir.join("replaced.docx");
    let replaced = replaced.to_str().unwrap();
    ok(&["replace-source", added, "--source", &edited, "-o", replaced]);
    let listed = json_of(&["sources", replaced, "--json"]);
    assert_eq!(listed["sources"][0]["guid"], guids[0]["guid"]);
    assert_eq!(
        listed["sources"][0]["source"]["fields"][0],
        json!(["title", "A longer history"])
    );

    let removed = dir.join("removed.docx");
    let removed = removed.to_str().unwrap();
    ok(&["remove-source", replaced, "Doe19", "-o", removed]);
    let text = ok(&["sources", removed]);
    assert!(text.starts_with("Smith20\tbook\t{"), "{text}");
    assert!(text.ends_with("\tA longer history\n"), "{text}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_cited_document_gets_its_citation_and_bibliography_results() {
    let dir = scratch("cite");
    let input = document(&dir, &["One brave step", "Two brave words"]);
    let input = input.to_str().unwrap();
    let path = |name: &str| dir.join(name).to_str().unwrap().to_owned();
    let sources = write_json(&dir, "sources.json", &smith());
    ok(&[
        "add-source",
        input,
        "--source",
        &sources,
        "-o",
        &path("1.docx"),
    ]);
    ok(&[
        "set-style",
        &path("1.docx"),
        "--style",
        "apa6",
        "-o",
        &path("2.docx"),
    ]);
    let record = json_of(&[
        "cite",
        &path("2.docx"),
        "--after",
        "brave",
        "--occurrence",
        "1",
        "--source",
        "Smith20",
        "--locale",
        "1033",
        "-o",
        &path("3.docx"),
        "--json",
    ]);
    assert_eq!(record["after"], "brave");
    assert_eq!(record["occurrence"], 1);
    ok(&[
        "insert",
        &path("3.docx"),
        "--index",
        "2",
        "--style",
        "apa6",
        "-o",
        &path("4.docx"),
    ]);
    let error = refused(&["update", &path("4.docx"), "-o", &path("5.docx")]);
    assert!(
        error.contains("requires application default locale"),
        "{error}"
    );
    let report = json_of(&[
        "update",
        &path("4.docx"),
        "--default-locale",
        "1033",
        "-o",
        &path("5.docx"),
        "--json",
    ]);
    assert_eq!(report["updated_citations"], 1);
    assert_eq!(report["rebuilt_bibliographies"], 1);

    let texts = texts(&path("5.docx"));
    assert_eq!(
        texts[..2],
        ["One brave step", "Two brave (Smith, 2020) words"]
    );
    assert!(
        texts[2..]
            .iter()
            .any(|text| text.contains("Smith, A. (2020)")),
        "{texts:?}"
    );
    let style = json_of(&["style", &path("5.docx"), "--json"]);
    assert_eq!(style["style"]["name"], "apa6");

    // The cited source cannot be removed.
    let error = refused(&[
        "remove-source",
        &path("5.docx"),
        "Smith20",
        "-o",
        &path("6.docx"),
    ]);
    assert!(
        error.contains("source is referenced by a citation"),
        "{error}"
    );
    assert!(!dir.join("6.docx").exists());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn citation_switches_come_inline_or_from_json() {
    let dir = scratch("switches");
    let input = document(&dir, &["Cited here"]);
    let input = input.to_str().unwrap();
    let path = |name: &str| dir.join(name).to_str().unwrap().to_owned();
    let sources = write_json(&dir, "sources.json", &json!([smith(), doe()]));
    ok(&[
        "add-source",
        input,
        "--source",
        &sources,
        "-o",
        &path("1.docx"),
    ]);
    ok(&[
        "set-style",
        &path("1.docx"),
        "--style",
        "apa6",
        "-o",
        &path("2.docx"),
    ]);

    ok(&[
        "cite",
        &path("2.docx"),
        "--paragraph",
        "0",
        "--run",
        "1",
        "--source",
        "Smith20,pages=12-14,no-author",
        "--source",
        "Doe19,prefix=see ",
        "--locale",
        "1033",
        "-o",
        &path("3.docx"),
    ]);
    let cited = json!([{"tag": "Smith20", "pages": "12, 14", "suppress_year": true}]);
    let cited = write_json(&dir, "cite.json", &cited);
    ok(&[
        "cite",
        &path("3.docx"),
        "--after",
        "Cited",
        "--sources",
        &cited,
        "--locale",
        "1033",
        "-o",
        &path("4.docx"),
    ]);
    assert_eq!(
        instructions(&path("4.docx")),
        [
            r#"CITATION Smith20 \l 1033 \p "12, 14" \y"#,
            r#"CITATION Smith20 \l 1033 \p 12-14 \n \m Doe19 \f "see ""#,
        ]
    );
    let report = json_of(&["update", &path("4.docx"), "-o", &path("5.docx"), "--json"]);
    assert_eq!(report["updated_citations"], 2);

    // A value holding a comma cannot be written inline.
    let error = refused(&[
        "cite",
        &path("2.docx"),
        "--after",
        "Cited",
        "--source",
        "Smith20,pages=12,14",
        "-o",
        &path("6.docx"),
    ]);
    assert!(error.contains("--sources CITE.json"), "{error}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn names_are_taken_in_kebab_or_snake_case_and_refused_with_the_known_ones() {
    let dir = scratch("names");
    let input = document(&dir, &["Text"]);
    let input = input.to_str().unwrap();
    let path = |name: &str| dir.join(name).to_str().unwrap().to_owned();
    for (style, name) in [("harvard-anglia", "1.docx"), ("harvard_anglia", "2.docx")] {
        ok(&["set-style", input, "--style", style, "-o", &path(name)]);
        assert_eq!(
            json_of(&["style", &path(name), "--json"])["style"]["name"],
            "harvard_anglia"
        );
    }
    let error = refused(&["set-style", input, "--style", "apa", "-o", &path("3.docx")]);
    assert!(error.contains("apa6, chicago, gb7714"), "{error}");

    let mut source = smith();
    source["kind"] = json!("Book");
    let source = write_json(&dir, "xml-name.json", &source);
    let error = refused(&[
        "add-source",
        input,
        "--source",
        &source,
        "-o",
        &path("4.docx"),
    ]);
    assert!(error.contains("unknown source kind \"Book\""), "{error}");
    let mut source = smith();
    source["contributors"][0]["corporate"] = json!("WHO");
    let source = write_json(&dir, "both.json", &source);
    let error = refused(&[
        "add-source",
        input,
        "--source",
        &source,
        "-o",
        &path("5.docx"),
    ]);
    assert!(error.contains("either \"people\""), "{error}");
    assert!(!dir.join("4.docx").exists() && !dir.join("5.docx").exists());

    let _ = fs::remove_dir_all(&dir);
}
