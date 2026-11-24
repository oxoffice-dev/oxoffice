//! The commands that report something give, with `--json`, one schema-1
//! record on standard output and nothing else: `docx validate`, `convert`
//! and `render`, `pptx diff`, `validate`, `convert`, `render` and
//! `thumbnail`. A plain `pptx convert` of a slide range names the slides it
//! wrote.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "oxoffice-cli-json-records-{}-{name}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch folder");
    dir
}

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/files")
        .join(name)
}

fn oxoffice(args: &[&str], paths: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oxoffice"))
        .args(args)
        .args(paths)
        .output()
        .expect("oxoffice runs")
}

/// The record a successful run printed, the whole of standard output.
fn json_record(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let record: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "standard output is one JSON record ({error}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    assert_eq!(record["schema"], 1, "{record}");
    record
}

/// Each output of a `convert`, `render` or `thumbnail` record names a file
/// that exists with the size it gives, and holds the pages or slides `unit`
/// lists.
fn assert_outputs(record: &Value, unit: &str, expected: &[&[u64]]) {
    let outputs = record["outputs"].as_array().expect("outputs");
    assert_eq!(outputs.len(), expected.len(), "{record}");
    for (output, numbers) in outputs.iter().zip(expected) {
        let path = output["path"].as_str().expect("path");
        let size = fs::metadata(path).expect("the file is written").len();
        assert_eq!(output["bytes"], size, "{output}");
        let held = output[unit]
            .as_array()
            .unwrap_or_else(|| panic!("{unit} in {output}"))
            .iter()
            .map(|number| number.as_u64().expect("a number"))
            .collect::<Vec<_>>();
        assert_eq!(held, *numbers, "{output}");
    }
}

#[test]
fn docx_validate_json_gives_the_verdict_with_its_errors_and_warnings() {
    let report = corpus("word/report.docx");
    let record = json_record(&oxoffice(&["docx", "validate", "--json"], &[&report]));
    assert_eq!(record["file"], report.display().to_string());
    assert_eq!(record["valid"], true);
    assert_eq!(record["errors"], serde_json::json!([]));
    assert!(record["warnings"].is_array(), "{record}");
}

#[test]
fn docx_validate_json_keeps_the_failing_exit_status() {
    // A style id no style defines is an error, a numbering instance the
    // document does not define a warning.
    let source = corpus("word/contract.docx");
    let mut package =
        oxoffice_opc::OpcPackage::from_reader(Cursor::new(fs::read(source).unwrap())).unwrap();
    let mut xml =
        String::from_utf8(package.get_part("/word/document.xml").unwrap().to_vec()).unwrap();
    for (attribute, value) in [
        ("<w:pStyle w:val=\"", "NoSuchStyle"),
        ("<w:numId w:val=\"", "4242"),
    ] {
        let at = xml.find(attribute).expect("the attribute") + attribute.len();
        let end = at + xml[at..].find('"').unwrap();
        xml.replace_range(at..end, value);
    }
    package.set_part("/word/document.xml", xml.into_bytes());
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    let path = scratch("docx-invalid").join("contract.docx");
    fs::write(&path, bytes.into_inner()).unwrap();

    let output = oxoffice(&["docx", "validate", "--json"], &[&path]);
    assert_eq!(output.status.code(), Some(1));
    let record: Value = serde_json::from_slice(&output.stdout).expect("one JSON record");
    assert_eq!(record["schema"], 1);
    assert_eq!(record["valid"], false);
    let holds = |key: &str, text: &str| {
        record[key]
            .as_array()
            .unwrap_or_else(|| panic!("{key} in {record}"))
            .iter()
            .any(|issue| issue.as_str().unwrap().contains(text))
    };
    assert!(holds("errors", "NoSuchStyle"), "{record}");
    assert!(holds("warnings", "4242"), "{record}");
}

#[test]
fn docx_convert_json_lists_the_files_written_with_their_pages() {
    let dir = scratch("docx-convert");
    let report = corpus("word/report.docx");
    let pdf = dir.join("report.pdf");
    let record = json_record(&oxoffice(
        &["docx", "convert", "--to", "pdf", "--json", "-o"],
        &[&pdf, &report],
    ));
    assert_eq!(record["file"], report.display().to_string());
    assert_eq!(record["format"], "pdf");
    assert_eq!(record["outputs"][0]["path"], pdf.display().to_string());
    let pages = record["outputs"][0]["pages"].as_array().expect("pages");
    assert!(!pages.is_empty(), "{record}");
    let all = (1..=pages.len() as u64).collect::<Vec<_>>();
    assert_outputs(&record, "pages", &[&all]);

    let png = dir.join("report.png");
    let record = json_record(&oxoffice(
        &[
            "docx", "convert", "--to", "png", "--pages", "1-2", "--json", "-o",
        ],
        &[&png, &report],
    ));
    assert_eq!(record["format"], "png");
    assert_eq!(record["dpi"], 150.0);
    assert_outputs(&record, "pages", &[&[1], &[2]]);

    let md = dir.join("report.md");
    let record = json_record(&oxoffice(
        &["docx", "convert", "--to", "md", "--json", "-o"],
        &[&md, &report],
    ));
    assert_eq!(record["format"], "md");
    assert_eq!(record["outputs"][0]["path"], md.display().to_string());
    assert!(record["outputs"][0].get("pages").is_none(), "{record}");

    let docx = dir.join("from-markdown.docx");
    let record = json_record(&oxoffice(
        &["docx", "convert", "--json", "-o"],
        &[&docx, &md],
    ));
    assert_eq!(record["format"], "docx");
    assert_eq!(record["outputs"][0]["path"], docx.display().to_string());
}

#[test]
fn docx_render_json_lists_each_page_image() {
    let dir = scratch("docx-render");
    let report = corpus("word/report.docx");
    let record = json_record(&oxoffice(
        &[
            "docx", "render", "--pages", "1,2", "--format", "jpeg", "--json", "-o",
        ],
        &[&dir, &report],
    ));
    assert_eq!(record["file"], report.display().to_string());
    assert_eq!(record["format"], "jpeg");
    assert_eq!(record["dpi"], 150.0);
    assert_outputs(&record, "pages", &[&[1], &[2]]);
}

#[test]
fn pptx_diff_json_gives_the_counts_and_differences_docx_diff_gives() {
    // The deck's slides are a title, highlights, key figures and a chart.
    // The edited copy drops the highlights, retitles the chart and ends with
    // a copy of the title slide.
    let a = corpus("powerpoint/deck.pptx");
    let b = scratch("pptx-diff").join("edited.pptx");
    let mut edited = oxoffice_pptx::Presentation::open(&a).unwrap();
    edited.remove_slide(1).unwrap();
    let retitled = edited
        .try_replace_text("Revenue by quarter", "Turnover by quarter")
        .unwrap();
    assert_eq!(retitled, 1);
    edited.duplicate_slide(0).unwrap();
    let last = edited.len() - 1;
    edited.move_slide(1, last).unwrap();
    edited.save(&b).unwrap();

    let record = json_record(&oxoffice(&["pptx", "diff", "--json"], &[&a, &b]));
    assert_eq!(record["file_a"], a.display().to_string());
    assert_eq!(record["file_b"], b.display().to_string());
    assert_eq!(record["scope"], "slide-text");
    assert_eq!(record["not_compared"], serde_json::json!([]));
    let differences = record["differences"].as_array().expect("differences");
    let found = differences
        .iter()
        .map(|difference| {
            assert_eq!(difference["story"], "slide");
            (
                difference["change"].as_str().unwrap(),
                difference["location_a"].as_str(),
                difference["location_b"].as_str(),
            )
        })
        .collect::<Vec<_>>();
    // Title and key figures match; the highlights (slide 2) go, the chart
    // (slide 4, now 3) changes, and the copy of the title comes as slide 4.
    assert_eq!(
        found,
        [
            ("removed", Some("2"), None),
            ("changed", Some("4"), Some("3")),
            ("added", None, Some("4")),
        ],
        "{record}"
    );
    assert!(
        differences[0]["text_a"]
            .as_str()
            .unwrap()
            .starts_with("Highlights")
    );
    assert!(differences[0]["text_b"].is_null());
    assert!(
        differences[1]["text_a"]
            .as_str()
            .unwrap()
            .starts_with("Revenue by quarter")
    );
    assert!(
        differences[1]["text_b"]
            .as_str()
            .unwrap()
            .starts_with("Turnover by quarter")
    );
    assert!(
        differences[2]["text_b"]
            .as_str()
            .unwrap()
            .starts_with("Quarterly Operations Review")
    );
    assert_eq!(
        (&record["changed"], &record["added"], &record["removed"]),
        (&Value::from(1), &Value::from(1), &Value::from(1))
    );

    let same = json_record(&oxoffice(&["pptx", "diff", "--json"], &[&a, &a]));
    assert_eq!(same["differences"], serde_json::json!([]));
    assert_eq!(
        (&same["changed"], &same["added"], &same["removed"]),
        (&Value::from(0), &Value::from(0), &Value::from(0))
    );
}

#[test]
fn pptx_validate_json_gives_the_verdict_with_its_issues() {
    let deck = corpus("powerpoint/deck.pptx");
    let record = json_record(&oxoffice(&["pptx", "validate", "--json"], &[&deck]));
    assert_eq!(record["file"], deck.display().to_string());
    assert_eq!(record["valid"], true);
    assert_eq!(record["errors"], serde_json::json!([]));
    assert_eq!(record["warnings"], serde_json::json!([]));
}

#[test]
fn pptx_convert_json_lists_the_files_written_with_their_slides() {
    let dir = scratch("pptx-convert");
    let deck = corpus("powerpoint/deck.pptx");
    let slides = oxoffice_pptx::Presentation::open(&deck).unwrap().len() as u64;
    assert!(slides >= 3, "the deck has {slides} slides");

    let pdf = dir.join("deck.pdf");
    let record = json_record(&oxoffice(
        &["pptx", "convert", "--to", "pdf", "--json", "-o"],
        &[&pdf, &deck],
    ));
    assert_eq!(record["format"], "pdf");
    assert_outputs(&record, "slides", &[&(1..=slides).collect::<Vec<_>>()]);

    // The slides the files hold, not their order in the selection.
    let png = dir.join("deck.png");
    let record = json_record(&oxoffice(
        &[
            "pptx", "convert", "--to", "png", "--slides", "2-3", "--json", "-o",
        ],
        &[&png, &deck],
    ));
    assert_eq!(record["format"], "png");
    assert_eq!(record["dpi"], 150.0);
    assert_outputs(&record, "slides", &[&[2], &[3]]);

    // One TIFF holds the slides of the range.
    let tiff = dir.join("deck.tiff");
    let record = json_record(&oxoffice(
        &[
            "pptx", "convert", "--to", "tiff", "--slides", "2-3", "--json", "-o",
        ],
        &[&tiff, &deck],
    ));
    assert_eq!(record["format"], "tiff");
    assert_eq!(record["outputs"][0]["path"], tiff.display().to_string());
    assert_outputs(&record, "slides", &[&[2, 3]]);

    // The document is the one output, the images beside it its assets.
    let html = dir.join("deck.html");
    let record = json_record(&oxoffice(
        &[
            "pptx",
            "convert",
            "--to",
            "html",
            "--embed-images=false",
            "--json",
            "-o",
        ],
        &[&html, &deck],
    ));
    assert_eq!(record["format"], "html");
    let outputs = record["outputs"].as_array().expect("outputs");
    assert_eq!(outputs.len(), 1, "{record}");
    assert_eq!(outputs[0]["path"], html.display().to_string());
    assert_eq!(
        outputs[0]["bytes"],
        fs::metadata(&html).unwrap().len(),
        "{record}"
    );
    let assets = record["assets"].as_array().expect("assets");
    assert!(!assets.is_empty(), "{record}");
    let document = fs::read_to_string(&html).unwrap();
    for asset in assets {
        let path = Path::new(asset["path"].as_str().expect("path"));
        assert_ne!(path, html, "{record}");
        assert_eq!(asset["bytes"], fs::metadata(path).unwrap().len(), "{asset}");
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(document.contains(name), "the document refers to {name}");
    }
}

#[test]
fn pptx_convert_names_the_slide_each_image_of_a_range_holds() {
    let dir = scratch("pptx-convert-plain");
    let deck = corpus("powerpoint/deck.pptx");
    let png = dir.join("deck.png");
    let output = oxoffice(
        &["pptx", "convert", "--to", "png", "--slides", "2-3", "-o"],
        &[&png, &deck],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // The files are numbered in the order of the selection, as `docx
    // convert` numbers pages.
    let expected = format!(
        "Slide 2 -> {}\nSlide 3 -> {}\n",
        dir.join("deck_001.png").display(),
        dir.join("deck_002.png").display()
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
}

#[test]
fn pptx_render_json_lists_each_slide_image() {
    let dir = scratch("pptx-render");
    let deck = corpus("powerpoint/deck.pptx");
    let record = json_record(&oxoffice(
        &["pptx", "render", "--slide", "2", "--json", "-o"],
        &[&dir, &deck],
    ));
    assert_eq!(record["file"], deck.display().to_string());
    assert_eq!(record["format"], "png");
    assert_eq!(record["dpi"], 150.0);
    assert_outputs(&record, "slides", &[&[2]]);
}

#[test]
fn pptx_thumbnail_json_names_the_image_of_slide_one() {
    let dir = scratch("pptx-thumbnail");
    let deck = corpus("powerpoint/deck.pptx");
    let png = dir.join("thumb.png");
    let record = json_record(&oxoffice(
        &["pptx", "thumbnail", "--json", "-o"],
        &[&png, &deck],
    ));
    assert_eq!(record["format"], "png");
    assert_eq!(record["outputs"][0]["path"], png.display().to_string());
    assert_outputs(&record, "slides", &[&[1]]);
    // The resolution draws slide one 320 pixels wide.
    let width_points = oxoffice_pptx::Presentation::open(&deck)
        .unwrap()
        .slide_size()
        .expect("a slide size")
        .0
        .0 as f64
        / 12_700.0;
    let dpi = record["dpi"].as_f64().expect("dpi");
    assert_eq!((dpi * width_points / 72.0).round(), 320.0, "{record}");
}
