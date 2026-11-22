//! An entity in XML text (`&amp;`, `&quot;`, `&#65;`) reaches every reader.
//! quick-xml reports it as an event of its own, apart from the text around
//! it, so a reader that only takes text events would drop it or misjudge
//! the content it stands in.

use oxoffice_docx::{Document, Length};

use crate::support::{doc_with, doc_with_body, reopen, texts};

/// A one-pixel PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

#[test]
fn a_flat_opc_package_with_markup_characters_in_its_text_round_trips() {
    let doc = doc_with(&["Tom & Jerry <3 \"quoted\""]);
    let flat = doc.to_flat_opc_bytes().unwrap();
    let reopened = Document::from_flat_opc_bytes(&flat).unwrap();
    assert_eq!(texts(&reopened), ["Tom & Jerry <3 \"quoted\""]);
}

#[test]
fn a_flat_opc_binary_part_may_hold_a_character_reference() {
    let mut doc = Document::new();
    doc.add_picture(PNG, "dot.png", Length::pt(10.0), Length::pt(10.0));
    let flat = String::from_utf8(doc.to_flat_opc_bytes().unwrap()).unwrap();
    assert!(flat.contains("<pkg:binaryData>"), "{flat}");
    // A line break written as a reference inside the base64 text.
    let flat = flat.replace("<pkg:binaryData>", "<pkg:binaryData>&#10;");
    let reopened = Document::from_flat_opc_bytes(flat.as_bytes()).unwrap();
    assert_eq!(reopened.pictures().unwrap()[0].bytes, PNG);
}

#[test]
fn a_link_whose_new_text_is_a_markup_character_stays_a_link() {
    // The new text is written as `&amp;`: the link still shows something.
    for text in ["&", "<", "A & B"] {
        let mut doc = Document::new();
        let url = doc.add_hyperlink_relationship("https://example.com");
        doc.add_paragraph("").add_hyperlink("Site", &url);
        let location = doc
            .story_items(&doc.stories().unwrap()[0])
            .unwrap()
            .into_iter()
            .next()
            .expect("the body paragraph")
            .location()
            .clone();
        doc.set_story_text(&location, text).unwrap();
        let doc = reopen(&mut doc);
        assert_eq!(texts(&doc), [text]);
        let xml = doc.part_xml("word/document.xml").unwrap();
        assert!(xml.contains("<w:hyperlink"), "{text}: {xml}");
    }
}

#[test]
fn an_entity_an_equation_keeps_as_written_survives_an_edit_of_its_paragraph() {
    let mut doc = doc_with_body(concat!(
        r#"<w:p><m:oMath xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math">"#,
        r#"<m:r><m:t>x</m:t></m:r>a&amp;b</m:oMath></w:p>"#,
    ));
    doc.paragraph_mut(0).unwrap().add_run(", edited");
    let doc = reopen(&mut doc);
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert!(xml.contains("</m:r>a&amp;b</m:oMath>"), "{xml}");
}
