//! An entity in XML text (`&amp;`, `&quot;`, `&#65;`) reaches every reader.
//! quick-xml reports it as an event of its own, apart from the text around
//! it, so a reader that only takes text events would drop it or misjudge
//! the content it stands in.

use std::io::Cursor;

use oxoffice_docx::{Document, Length};
use oxoffice_opc::OpcPackage;

use crate::support::{doc_with, doc_with_body, docx_with_body, reopen, texts};

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

#[test]
fn an_entity_alone_outside_the_body_is_refused_as_text_there_is() {
    for outside in ["x", "&amp;"] {
        let bytes = Document::new().to_bytes().unwrap();
        let mut package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
        let part = package.main_document_part().unwrap();
        let xml = String::from_utf8(package.get_part(&part).unwrap().to_vec()).unwrap();
        let xml = xml.replacen("<w:body>", &format!("{outside}<w:body>"), 1);
        package.set_part(&part, xml.into_bytes());
        let mut out = Cursor::new(Vec::new());
        package.write_to(&mut out).unwrap();
        assert!(
            Document::from_bytes(&out.into_inner()).is_err(),
            "{outside}"
        );
    }
}

#[test]
fn a_ruby_holding_an_entity_alone_where_it_holds_no_text_stays_raw() {
    // The ruby model cannot write character data back between its
    // children: such a ruby is kept as written, as one holding text there.
    for direct in ["x", "&amp;"] {
        let body = format!(
            concat!(
                r#"<w:p><w:ruby>{}<w:rubyPr><w:hps w:val="10"/></w:rubyPr>"#,
                r#"<w:rt><w:r><w:t>とうきょう</w:t></w:r></w:rt>"#,
                r#"<w:rubyBase><w:r><w:t>東京</w:t></w:r></w:rubyBase></w:ruby></w:p>"#,
            ),
            direct
        );
        let mut doc = doc_with_body(&body);
        assert!(doc.paragraph(0).unwrap().ruby(0).is_none(), "{direct}");
        doc.paragraph_mut(0).unwrap().add_run(", edited");
        let doc = reopen(&mut doc);
        let xml = doc.part_xml("word/document.xml").unwrap();
        assert!(
            xml.contains(&format!("<w:ruby>{direct}<w:rubyPr>")),
            "{direct}: {xml}"
        );
    }
}

#[test]
fn a_property_value_written_with_character_references_reads_as_its_characters() {
    use oxoffice_docx::Alignment;

    // A paragraph and a table cell width, plainly and with the first
    // character of each value as a reference (`w:val="&#99;enter"`).
    let body = |center: &str, before: &str, width: &str| {
        format!(
            concat!(
                r#"<w:p><w:pPr><w:jc w:val="{center}"/><w:spacing w:before="{before}"/></w:pPr>"#,
                r#"<w:r><w:t>Centred heading</w:t></w:r></w:p>"#,
                r#"<w:tbl><w:tblPr><w:tblW w:w="0" w:type="auto"/></w:tblPr>"#,
                r#"<w:tblGrid><w:gridCol w:w="{width}"/></w:tblGrid><w:tr><w:tc>"#,
                r#"<w:tcPr><w:tcW w:w="{width}" w:type="dxa"/></w:tcPr>"#,
                r#"<w:p><w:r><w:t>Cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p/>"#,
            ),
            center = center,
            before = before,
            width = width
        )
    };
    let read = |body: String| {
        let doc = doc_with_body(&body);
        let paragraph = doc.paragraph(0).expect("the paragraph");
        (
            paragraph.alignment(),
            paragraph.space_before(),
            doc.to_markdown(),
        )
    };
    let plain = read(body("center", "240", "4000"));
    assert_eq!(plain.0, Some(Alignment::Center));
    assert_eq!(read(body("&#99;enter", "&#50;40", "&#52;000")), plain);
}

/// `bytes` with `children` written into the final section properties, after
/// the page margins.
fn with_section_children(bytes: Vec<u8>, children: &str) -> Vec<u8> {
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    let part = package.main_document_part().unwrap();
    let xml = String::from_utf8(package.get_part(&part).unwrap().to_vec()).unwrap();
    let margins = xml.find("<w:pgMar").expect("the page margins");
    let after = margins + xml[margins..].find("/>").expect("an empty element") + 2;
    let xml = format!("{}{children}{}", &xml[..after], &xml[after..]);
    package.set_part(&part, xml.into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    out.into_inner()
}

#[test]
fn a_value_written_with_character_references_reads_as_its_characters_everywhere() {
    let equation = |begin: &str| {
        format!(
            concat!(
                r#"<w:p><m:oMath xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math">"#,
                r#"<m:d><m:dPr><m:begChr m:val="{begin}"/></m:dPr><m:e><m:r><m:t>x</m:t></m:r></m:e></m:d>"#,
                r#"</m:oMath></w:p>"#,
            ),
            begin = begin
        )
    };
    let rtf = |body: &str, section: &str| {
        let bytes = with_section_children(docx_with_body(body), section);
        let result = Document::from_bytes(&bytes)
            .unwrap()
            .to_rtf_bytes()
            .unwrap();
        (
            String::from_utf8(result.bytes).unwrap(),
            format!("{:?}", result.diagnostics),
        )
    };
    // Each case: a value written with a reference, and whether the reader
    // found its characters.
    let numbering = rtf("<w:p/>", r#"<w:pgNumType w:fmt="&#100;ecimal"/>"#);
    let delimiter = rtf(&equation("&#60;"), "");
    for (what, found, read) in [
        (
            "a decimal page numbering, which RTF keeps",
            !numbering.1.contains("page numbering"),
            &numbering,
        ),
        (
            "the delimiter of an equation",
            delimiter.0.contains("{\\mbegChr <}"),
            &delimiter,
        ),
    ] {
        assert!(found, "{what}: {read:?}");
    }
}

#[test]
fn a_value_kept_as_written_keeps_its_references_through_an_edit() {
    // Values the section model does not type are written back as read: a
    // reference resolved and written raw would leave `&` bare in the part.
    let bytes = with_section_children(
        docx_with_body("<w:p/>"),
        concat!(
            r#"<w:paperSrc w:first="&#49;" w:other="a&amp;b"/>"#,
            r#"<w:lnNumType w:countBy="x&lt;y" w:start="&#50;"/>"#,
        ),
    );
    let mut doc = Document::from_bytes(&bytes).unwrap();
    doc.set_margins(
        Length::pt(36.0),
        Length::pt(36.0),
        Length::pt(36.0),
        Length::pt(36.0),
    );
    let doc = reopen(&mut doc);
    let xml = doc.part_xml("word/document.xml").unwrap();
    for kept in [r#"w:other="a&amp;b""#, r#"w:countBy="x&lt;y""#] {
        assert!(xml.contains(kept), "{kept}: {xml}");
    }
    for read in [r#"w:first="1""#, r#"w:start="2""#] {
        assert!(xml.contains(read), "{read}: {xml}");
    }
}
