//! Attribute values as read back after an edit rewrites their element: a
//! value holding a markup character, or written with a character reference,
//! reads back as written, and an attribute kept as read stays well-formed.

use std::io::Cursor;

use oxoffice_docx::{Alignment, Document};
use oxoffice_opc::OpcPackage;
use quick_xml::Reader;
use quick_xml::escape::unescape;
use quick_xml::events::Event;

use crate::support::{doc_with, doc_with_body, reopen};

/// The value of each attribute named `attribute` on an element named
/// `element` in `part` of the saved document, as an XML reader reads it.
fn read_attribute(doc: &mut Document, part: &str, element: &str, attribute: &str) -> Vec<String> {
    let bytes = doc.to_bytes().expect("document saves");
    let package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    let xml = package.get_part(part).expect("part is saved");
    let mut reader = Reader::from_reader(xml);
    let mut values = Vec::new();
    loop {
        match reader.read_event().expect("saved part is well-formed") {
            Event::Start(start) | Event::Empty(start)
                if start.name().as_ref() == element.as_bytes() =>
            {
                for found in start.attributes() {
                    let found = found.expect("saved attribute is well-formed");
                    if found.key.as_ref() == attribute.as_bytes() {
                        let raw = std::str::from_utf8(&found.value).expect("UTF-8 value");
                        values.push(unescape(raw).expect("value resolves").into_owned());
                    }
                }
            }
            Event::Eof => return values,
            _ => {}
        }
    }
}

/// A new document whose styles part holds `style` too.
fn doc_with_style(style: &str) -> Document {
    let bytes = Document::new().to_bytes().expect("new document saves");
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    let xml = String::from_utf8(
        package
            .get_part("/word/styles.xml")
            .expect("styles part")
            .to_vec(),
    )
    .expect("styles part is UTF-8");
    let end = xml.rfind("</w:styles>").expect("styles end");
    let xml = format!("{}{style}{}", &xml[..end], &xml[end..]);
    package.set_part("/word/styles.xml", xml.into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).expect("package writes");
    Document::from_bytes(&out.into_inner()).expect("document with this style opens")
}

#[test]
fn a_style_id_holding_markup_characters_reads_back_as_set_after_two_saves() {
    let id = "Q&A \"<x>\"";
    let mut doc = doc_with(&["text"]);
    let mut paragraph = doc.paragraph_mut(0).unwrap();
    paragraph.set_style(id);
    paragraph.add_run("run").set_style(id);
    let mut doc = reopen(&mut reopen(&mut doc));
    let paragraph = &doc.paragraphs()[0];
    assert_eq!(paragraph.style_id(), Some(id));
    let runs: Vec<_> = paragraph
        .runs()
        .map(|run| run.style_id().map(str::to_owned))
        .collect();
    assert_eq!(runs.last().cloned().flatten().as_deref(), Some(id));
    assert_eq!(
        read_attribute(&mut doc, "/word/document.xml", "w:pStyle", "w:val"),
        [id]
    );
    assert_eq!(
        read_attribute(&mut doc, "/word/document.xml", "w:rStyle", "w:val"),
        [id]
    );
}

#[test]
fn values_written_with_character_references_read_back_resolved_after_an_edit() {
    let mut doc = doc_with_body(concat!(
        r#"<w:p><w:pPr><w:pStyle w:val="&#72;eading1"/>"#,
        r#"<w:framePr w:dropCap="&#100;rop" w:lines="2" w:wrap="&#97;round"/>"#,
        r#"<w:shd w:val="&#99;lear" w:color="&#97;uto" w:fill="&#70;1F5F9"/>"#,
        r#"<w:spacing w:line="240" w:lineRule="&#97;uto"/></w:pPr>"#,
        r#"<w:r><w:rPr><w:rStyle w:val="Q&amp;A"/>"#,
        r#"<w:color w:val="&#70;F0000" w:themeColor="&#97;ccent1"/></w:rPr>"#,
        r#"<w:t>x</w:t></w:r>"#,
        r#"<w:r><w:sym w:font="&#87;ingdings" w:char="F04A"/></w:r></w:p>"#,
        r#"<w:tbl><w:tblPr><w:tblW w:w="2000" w:type="dxa"/>"#,
        r#"<w:tblLayout w:type="&#102;ixed"/><w:tblLook w:val="&#48;4A0"/></w:tblPr>"#,
        r#"<w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:tcPr>"#,
        r#"<w:tcW w:w="2000" w:type="dxa"/><w:tcBorders>"#,
        r#"<w:top w:val="single" w:sz="4" w:space="0" w:color="&#48;000FF"/>"#,
        r#"</w:tcBorders></w:tcPr><w:p><w:r><w:t>c</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#,
    ));
    let mut paragraph = doc.paragraph_mut(0).unwrap();
    paragraph.set_alignment(Alignment::Center);
    paragraph.run_mut(0).unwrap().set_bold(true);
    doc.table_mut(0).unwrap().set_alignment(Alignment::Center);
    let mut doc = reopen(&mut reopen(&mut doc));
    let expected = [
        ("w:pStyle", "w:val", "Heading1"),
        ("w:framePr", "w:dropCap", "drop"),
        ("w:framePr", "w:wrap", "around"),
        ("w:shd", "w:val", "clear"),
        ("w:shd", "w:color", "auto"),
        ("w:shd", "w:fill", "F1F5F9"),
        ("w:spacing", "w:lineRule", "auto"),
        ("w:rStyle", "w:val", "Q&A"),
        ("w:color", "w:val", "FF0000"),
        ("w:color", "w:themeColor", "accent1"),
        ("w:sym", "w:font", "Wingdings"),
        ("w:tblLayout", "w:type", "fixed"),
        ("w:tblLook", "w:val", "04A0"),
        ("w:top", "w:color", "0000FF"),
    ];
    let wrong: Vec<_> = expected
        .into_iter()
        .filter_map(|(element, attribute, value)| {
            let read = read_attribute(&mut doc, "/word/document.xml", element, attribute);
            (read != [value]).then(|| format!("{element}/@{attribute}: {read:?}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{wrong:#?}");
    assert_eq!(doc.paragraphs()[0].style_id(), Some("Heading1"));
}

#[test]
fn a_header_reference_written_with_a_character_reference_keeps_its_header_after_an_edit() {
    let mut doc = doc_with(&["body"]);
    doc.set_header("Running head");
    let bytes = doc.to_bytes().unwrap();
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    let xml = String::from_utf8(package.get_part("/word/document.xml").unwrap().to_vec()).unwrap();
    let at = xml.find("<w:headerReference").expect("a header reference");
    let id = xml[at..].find("r:id=\"r").expect("its relationship id") + at + "r:id=\"".len();
    let xml = format!("{}&#114;{}", &xml[..id], &xml[id + 1..]);
    package.set_part("/word/document.xml", xml.into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let mut doc = Document::from_bytes(&out.into_inner()).unwrap();
    assert_eq!(doc.header_text().as_deref(), Some("Running head"));
    doc.paragraph_mut(0)
        .unwrap()
        .set_alignment(Alignment::Center);
    let doc = reopen(&mut reopen(&mut doc));
    assert_eq!(doc.header_text().as_deref(), Some("Running head"));
}

#[test]
fn a_style_id_and_name_written_with_references_read_back_after_a_styles_edit() {
    let mut doc = doc_with_style(concat!(
        r#"<w:style w:type="paragraph" w:styleId="Q&amp;A">"#,
        r#"<w:name w:val="Q &amp; A"/><w:basedOn w:val="&#78;ormal"/>"#,
        r#"<w:next w:val="&#78;ormal"/></w:style>"#,
    ));
    doc.add_common_styles().unwrap();
    let mut doc = reopen(&mut reopen(&mut doc));
    let style = doc.style("Q&A").expect("the style keeps its id");
    assert_eq!(style.name(), Some("Q & A"));
    assert_eq!(style.based_on(), Some("Normal"));
    let read = |doc: &mut Document, element: &str| {
        read_attribute(doc, "/word/styles.xml", element, "w:val")
    };
    assert!(read(&mut doc, "w:name").contains(&"Q & A".to_owned()));
    assert!(
        !read(&mut doc, "w:next")
            .iter()
            .any(|value| value.contains('&'))
    );
}

#[test]
fn an_attribute_kept_as_read_with_a_double_quote_stays_well_formed_after_an_edit() {
    let mut doc = doc_with_body(concat!(
        r#"<w:p><w:r><w:rPr>"#,
        r#"<w:rFonts xmlns:x="urn:example" w:ascii="Arial" x:note='say "hi"'/>"#,
        r#"<w:color xmlns:x="urn:example" w:val="FF0000" x:note='a"b'/>"#,
        r#"<w:shd xmlns:x="urn:example" w:val="clear" w:fill="auto" x:note='&amp;"c'/>"#,
        r#"<w:lang xmlns:x="urn:example" w:val="en-US" x:note='&lt;"d'/>"#,
        r#"</w:rPr><w:t>x</w:t></w:r></w:p>"#,
    ));
    doc.paragraph_mut(0)
        .unwrap()
        .run_mut(0)
        .unwrap()
        .set_bold(true);
    let mut doc = reopen(&mut reopen(&mut doc));
    let read = |doc: &mut Document, element: &str| {
        read_attribute(doc, "/word/document.xml", element, "x:note")
    };
    assert_eq!(read(&mut doc, "w:rFonts"), ["say \"hi\""]);
    assert_eq!(read(&mut doc, "w:color"), ["a\"b"]);
    assert_eq!(read(&mut doc, "w:shd"), ["&\"c"]);
    assert_eq!(read(&mut doc, "w:lang"), ["<\"d"]);
}

#[test]
fn a_bookmark_between_blocks_written_with_a_reference_reads_its_name_resolved() {
    let doc = doc_with_body(concat!(
        r#"<w:bookmarkStart w:id="0" w:name="&#95;Toc1"/>"#,
        r#"<w:p><w:r><w:t>Heading</w:t></w:r></w:p><w:bookmarkEnd w:id="0"/>"#,
    ));
    let names: Vec<_> = doc
        .bookmarks()
        .iter()
        .map(|bookmark| bookmark.name().map(str::to_owned))
        .collect();
    assert_eq!(names, [Some("_Toc1".to_owned())]);
}
