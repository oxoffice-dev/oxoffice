//! What a user reads shows what a reader sees: the accepted view, with the
//! results of simple fields and the text of tracked insertions, in every
//! reader, export and generated text built from document text.

use std::io::{Cursor, Read};

use oxoffice_docx::paragraph::ParagraphItemRef;
use oxoffice_docx::{Document, FieldEvaluationContext, RunPosition, RunRange};
use oxoffice_opc::OpcPackage;

use crate::support::{doc_with, doc_with_body, reopen, texts};

/// Runs reading `Chapter 1`, the 1 a simple field's result.
const CHAPTER: &str = concat!(
    r#"<w:r><w:t xml:space="preserve">Chapter </w:t></w:r>"#,
    r#"<w:fldSimple w:instr=" SEQ Chapter "><w:r><w:t>1</w:t></w:r></w:fldSimple>"#,
);

/// Runs reading `A 12C`: a tracked insertion holding a simple field.
const INSERTED: &str = concat!(
    r#"<w:r><w:t xml:space="preserve">A </w:t></w:r>"#,
    r#"<w:ins w:id="905" w:author="a" w:date="2026-01-01T00:00:00Z">"#,
    r#"<w:fldSimple w:instr=" QUOTE a "><w:r><w:t>12</w:t></w:r></w:fldSimple>"#,
    r#"<w:r><w:t>C</w:t></w:r></w:ins>"#,
);

/// `doc` saved, its part `name` rewritten by `edit`, and opened again.
fn with_part(doc: &mut Document, name: &str, edit: impl FnOnce(&str) -> String) -> Document {
    let mut package = OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).unwrap();
    let xml = String::from_utf8(package.get_part(name).unwrap().to_vec()).unwrap();
    let edited = edit(&xml);
    assert_ne!(edited, xml, "{name} changed");
    package.set_part(name, edited.into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    Document::from_bytes(&out.into_inner()).unwrap()
}

/// `xml` with the run `<w:r><w:t>{text}</w:t></w:r>` replaced by `runs`.
fn replace_run(xml: &str, text: &str, runs: &str) -> String {
    let at = xml.find(&format!(">{text}</w:t>")).expect("the text");
    let start = xml[..at]
        .rfind("<w:r>")
        .max(xml[..at].rfind("<w:r "))
        .unwrap();
    let end = at + xml[at..].find("</w:r>").unwrap() + "</w:r>".len();
    format!("{}{runs}{}", &xml[..start], &xml[end..])
}

#[test]
fn a_tracked_insertion_holding_a_simple_field_reads_with_its_result() {
    let mut doc = doc_with_body(&format!("<w:p>{INSERTED}</w:p>"));
    assert_eq!(texts(&doc), ["A 12C"]);
    assert_eq!(doc.to_markdown().trim(), "A 12C");
    assert!(
        doc.to_html().contains("12"),
        "the HTML shows the field result"
    );
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["A 12C"]);
}

#[test]
fn a_comment_text_shows_its_simple_field_results() {
    let mut doc = doc_with(&["Body"]);
    let id = doc
        .add_comment(
            RunRange {
                start: RunPosition {
                    body_index: 0,
                    run_index: 0,
                },
                end: RunPosition {
                    body_index: 0,
                    run_index: 1,
                },
            },
            "Ada",
            None,
            "Marker",
        )
        .unwrap();
    let doc = with_part(&mut doc, "/word/comments.xml", |xml| {
        replace_run(xml, "Marker", CHAPTER)
    });
    let comment = doc.comments().into_iter().find(|c| c.id() == id).unwrap();
    assert_eq!(comment.text(), "Chapter 1");
}

#[test]
fn a_footnote_text_shows_its_simple_field_results() {
    let mut doc = doc_with(&["Body"]);
    let at = doc.paragraph_story_location(0).unwrap().unwrap();
    let id = doc.create_footnote(&at, "Marker").unwrap();
    let doc = with_part(&mut doc, "/word/footnotes.xml", |xml| {
        replace_run(xml, "Marker", CHAPTER)
    });
    assert_eq!(doc.footnotes(), [(id, " Chapter 1".to_owned())]);
}

#[test]
fn a_header_text_shows_its_simple_field_results() {
    let mut doc = doc_with(&["Body"]);
    doc.set_header("Marker");
    let doc = with_part(&mut doc, "/word/header1.xml", |xml| {
        replace_run(xml, "Marker", CHAPTER)
    });
    assert_eq!(doc.header_text().as_deref(), Some("Chapter 1"));
}

#[test]
fn a_hyperlink_text_shows_the_tracked_insertion_inside_it() {
    let doc = doc_with_body(concat!(
        r#"<w:p><w:hyperlink w:anchor="target"><w:r><w:t xml:space="preserve">See </w:t></w:r>"#,
        r#"<w:ins w:id="906" w:author="a" w:date="2026-01-01T00:00:00Z"><w:r><w:t>more</w:t></w:r></w:ins>"#,
        r#"</w:hyperlink></w:p>"#,
    ));
    assert_eq!(doc.links()[0].text, "See more");
    let paragraph = doc.paragraph(0).unwrap();
    let link = paragraph
        .items()
        .find_map(|item| match item {
            ParagraphItemRef::Hyperlink(link) => Some(link.text()),
            _ => None,
        })
        .unwrap();
    assert_eq!(link, "See more");
}

#[test]
fn finding_content_matches_the_text_of_a_simple_field_result() {
    let doc = doc_with_body(&format!(
        "<w:p><w:r><w:t>Intro</w:t></w:r></w:p><w:p>{CHAPTER}</w:p>"
    ));
    assert_eq!(doc.find_content_index("Chapter 1"), Some(1));
}

#[test]
fn a_preformatted_paragraph_exports_its_simple_field_results_to_markdown() {
    let doc = doc_with_body(&format!(
        r#"<w:p><w:pPr><w:pStyle w:val="HTMLPreformatted"/></w:pPr>{CHAPTER}</w:p>"#
    ));
    let markdown = doc.to_markdown();
    assert!(markdown.contains("Chapter 1"), "{markdown}");
    assert!(markdown.contains("```"), "{markdown}");
}

#[test]
fn a_table_of_contents_lists_a_heading_with_its_simple_field_result() {
    let mut doc = doc_with_body(&format!(
        r#"<w:p><w:r><w:t>Intro</w:t></w:r></w:p><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr>{CHAPTER}</w:p>"#
    ));
    doc.insert_toc(0, 3).unwrap();
    let doc = reopen(&mut doc);
    // The heading itself and its entry in the table.
    let texts = texts(&doc);
    assert_eq!(
        texts
            .iter()
            .filter(|text| text.starts_with("Chapter 1"))
            .count(),
        2,
        "{texts:?}"
    );
}

#[test]
fn an_epub_navigation_label_shows_a_heading_simple_field_result() {
    let doc = doc_with_body(&format!(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr>{CHAPTER}</w:p><w:p><w:r><w:t>Text</w:t></w:r></w:p>"#
    ));
    let bytes = doc.to_epub_bytes().unwrap().bytes;
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut navigation = String::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).unwrap();
        if file.name().is_ok_and(|name| name.ends_with("nav.xhtml")) {
            file.read_to_string(&mut navigation).unwrap();
        }
    }
    assert!(navigation.contains("Chapter 1"), "{navigation}");
}

#[test]
fn a_reference_inside_the_bookmark_it_names_stays_the_same_across_updates() {
    let mut doc = doc_with_body(concat!(
        r#"<w:p><w:bookmarkStart w:id="1" w:name="self"/><w:r><w:t xml:space="preserve">X </w:t></w:r>"#,
        r#"<w:fldSimple w:instr=" REF self "><w:r><w:t>old</w:t></w:r></w:fldSimple><w:bookmarkEnd w:id="1"/></w:p>"#,
    ));
    doc.update_fields(&FieldEvaluationContext::default())
        .unwrap();
    let once = texts(&doc);
    doc.update_fields(&FieldEvaluationContext::default())
        .unwrap();
    assert_eq!(texts(&doc), once, "a second update changes nothing");
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), once);
}
