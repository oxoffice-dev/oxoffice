//! `oxoffice docx fill` reaches the content controls of every story, a
//! header's as the body's, by `--tag` and by `--values`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use oxoffice_docx::{
    ContentControlPlacement, ContentControlSpec, ContentLocation, Document, HdrFtrType,
    HeaderFooterKind, StoryItemKind, StoryKind,
};
use oxoffice_docx_oxml::content_control::SdtType;

/// A scratch folder of its own for `name`.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("oxoffice-cli-fill-{name}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("scratch folder");
    dir
}

/// A document with a plain text control tagged `title` in its header and a
/// rich text one tagged `summary` in its body, saved in `dir`.
fn form_with_a_header_control(dir: &Path) -> PathBuf {
    let mut doc = Document::new();
    doc.add_paragraph("Body");
    let header = doc
        .create_section_story(0, HeaderFooterKind::Header, HdrFtrType::Default)
        .expect("a header story is created");
    let control = |kind, tag: &str| ContentControlSpec {
        placement: ContentControlPlacement::Block,
        kind,
        tag: Some(tag.to_owned()),
        alias: None,
        lock: None,
        appearance: None,
        placeholder: None,
    };
    doc.insert_content_control(
        &ContentLocation::end(header),
        control(SdtType::PlainText, "title"),
    )
    .expect("a header control is inserted");
    let body = doc
        .stories()
        .unwrap()
        .into_iter()
        .find(|story| story.kind() == StoryKind::Body)
        .unwrap();
    doc.insert_content_control(
        &ContentLocation::end(body),
        control(SdtType::RichText, "summary"),
    )
    .expect("a body control is inserted");
    let path = dir.join("form.docx");
    doc.save(&path).expect("the form saves");
    path
}

fn fill(input: &Path, args: &[&str], output: &Path) {
    let run = Command::new(env!("CARGO_BIN_EXE_oxoffice"))
        .args(["docx", "fill"])
        .arg(input)
        .args(args)
        .arg("-o")
        .arg(output)
        .output()
        .expect("oxoffice runs");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
}

/// The text of the header's control, as its story item reads.
fn header_value(path: &Path) -> String {
    let doc = Document::open(path).expect("the filled form opens");
    let header = doc
        .stories()
        .unwrap()
        .into_iter()
        .find(|story| story.kind() == StoryKind::Header)
        .expect("the header story");
    let item = doc
        .story_items(&header)
        .unwrap()
        .into_iter()
        .find(|item| item.kind() == StoryItemKind::ContentControl)
        .expect("the header control");
    item.text()
        .expect("the control's text reads")
        .unwrap_or_default()
}

#[test]
fn fill_by_tag_and_by_values_reaches_a_control_in_a_header() {
    let dir = scratch("header");
    let form = form_with_a_header_control(&dir);

    let by_tag = dir.join("by-tag.docx");
    fill(
        &form,
        &["--tag", "title=Annual report", "--tag", "summary=Done"],
        &by_tag,
    );
    assert_eq!(header_value(&by_tag), "Annual report");

    let values = dir.join("values.json");
    fs::write(&values, r#"{"title": "Quarterly report"}"#).unwrap();
    let by_values = dir.join("by-values.docx");
    fill(&form, &["--values", values.to_str().unwrap()], &by_values);
    assert_eq!(header_value(&by_values), "Quarterly report");

    let _ = fs::remove_dir_all(&dir);
}
