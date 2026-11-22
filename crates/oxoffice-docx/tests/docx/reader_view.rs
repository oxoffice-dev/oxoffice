//! What a user reads shows what a reader sees: the accepted view, with the
//! results of simple fields and the text of tracked insertions, in every
//! reader, export and generated text built from document text.

use std::io::{Cursor, Read};

use oxoffice_docx::paragraph::ParagraphItemRef;

use crate::support::doc_with_body;

/// Runs reading `Chapter 1`, the 1 a simple field's result.
const CHAPTER: &str = concat!(
    r#"<w:r><w:t xml:space="preserve">Chapter </w:t></w:r>"#,
    r#"<w:fldSimple w:instr=" SEQ Chapter "><w:r><w:t>1</w:t></w:r></w:fldSimple>"#,
);

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

/// An insertion holding a simple field, as Word writes a field typed with
/// tracked changes on.
const INSERTED_FIELD: &str = concat!(
    r#"<w:ins w:id="5" w:author="A" w:date="2026-01-01T00:00:00Z">"#,
    r#"<w:fldSimple w:instr=" QUOTE a "><w:r><w:t>12</w:t></w:r></w:fldSimple>"#,
    r#"<w:r><w:t>C</w:t></w:r></w:ins>"#,
);

#[test]
fn a_link_holding_an_inserted_simple_field_reads_its_result_in_both_link_lists() {
    let doc = doc_with_body(&format!(
        r#"<w:p><w:hyperlink w:anchor="x"><w:r><w:t xml:space="preserve">See </w:t></w:r>{INSERTED_FIELD}</w:hyperlink></w:p>"#
    ));
    assert_eq!(doc.links()[0].text, "See 12C");
    let body = doc
        .stories()
        .unwrap()
        .into_iter()
        .find(|story| story.kind() == oxoffice_docx::StoryKind::Body)
        .unwrap();
    let links = doc.story_links(&body).unwrap();
    assert_eq!(links[0].1.text, "See 12C");
}

#[test]
fn an_inline_content_control_and_a_field_hyperlink_read_a_simple_field_result() {
    let doc = doc_with_body(concat!(
        r#"<w:p><w:sdt><w:sdtPr><w:id w:val="1"/></w:sdtPr><w:sdtContent>"#,
        r#"<w:r><w:t xml:space="preserve">No. </w:t></w:r>"#,
        r#"<w:fldSimple w:instr=" SEQ Item "><w:r><w:t>7</w:t></w:r></w:fldSimple>"#,
        r#"</w:sdtContent></w:sdt></w:p>"#,
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
        r#"<w:r><w:instrText xml:space="preserve"> HYPERLINK "https://example.com/" </w:instrText></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t xml:space="preserve">our site, page </w:t></w:r>"#,
        r#"<w:fldSimple w:instr=" PAGE "><w:r><w:t>3</w:t></w:r></w:fldSimple>"#,
        r#"<w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
    ));
    assert_eq!(doc.content_controls()[0].text(), "No. 7");
    assert_eq!(doc.links()[0].text, "our site, page 3");
}
