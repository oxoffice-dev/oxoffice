//! `Document::story_paragraphs` lists every paragraph of every story with its
//! story kind, part and style, in the accepted view, and `count_text` counts a
//! text by story kind, on the Word and Google Docs files of the corpus.

use std::io::Cursor;
use std::path::Path;

use oxoffice_docx::{Document, StoryKind, StoryParagraph};
use oxoffice_opc::OpcPackage;

const PRODUCERS: [&str; 2] = ["word", "google"];

fn corpus_bytes(producer: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/files")
        .join(producer)
        .join("report.docx");
    std::fs::read(path).expect("corpus report is readable")
}

/// The corpus report of `producer` with `xml` inserted before the end of the
/// part `part_name`: before the last `w:sectPr` of the main part, before the
/// closing tag of another part.
fn report_with(producer: &str, part_name: &str, xml: &str) -> Document {
    let mut package = OpcPackage::from_reader(Cursor::new(corpus_bytes(producer))).unwrap();
    let part = String::from_utf8(package.get_part(part_name).unwrap().to_vec()).unwrap();
    let at = if part_name == "/word/document.xml" {
        part.rfind("<w:sectPr").unwrap()
    } else {
        part.rfind("</w:").unwrap()
    };
    let part = format!("{}{xml}{}", &part[..at], &part[at..]);
    package.set_part(part_name, part.into_bytes());
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    Document::from_bytes(&bytes.into_inner()).unwrap()
}

fn body_with(producer: &str, xml: &str) -> Document {
    report_with(producer, "/word/document.xml", xml)
}

fn find<'a>(paragraphs: &'a [StoryParagraph], text: &str) -> &'a StoryParagraph {
    paragraphs
        .iter()
        .find(|paragraph| paragraph.text == text)
        .unwrap_or_else(|| panic!("no paragraph {text:?} in {paragraphs:#?}"))
}

fn position(paragraphs: &[StoryParagraph], text: &str) -> usize {
    paragraphs
        .iter()
        .position(|paragraph| paragraph.text == text)
        .unwrap_or_else(|| panic!("no paragraph {text:?}"))
}

#[test]
fn smart_tags_and_inline_custom_xml_read_as_paragraph_text_and_replacement_read_them() {
    let wrapped = r#"<w:p><w:r><w:t xml:space="preserve">Contact </w:t></w:r><w:smartTag w:uri="urn:schemas-microsoft-com:office:smarttags" w:element="PersonName"><w:r><w:t>Acme Corp</w:t></w:r></w:smartTag></w:p><w:p><w:r><w:t xml:space="preserve">Client </w:t></w:r><w:customXml w:element="client"><w:r><w:t>Acme Corp</w:t></w:r></w:customXml></w:p>"#;
    for producer in PRODUCERS {
        let mut doc = body_with(producer, wrapped);
        let paragraphs = doc.story_paragraphs().unwrap();
        find(&paragraphs, "Contact Acme Corp");
        find(&paragraphs, "Client Acme Corp");
        let read = doc
            .paragraphs()
            .iter()
            .map(|paragraph| paragraph.text())
            .filter(|text| text.contains("Acme Corp"))
            .collect::<Vec<_>>();
        assert_eq!(
            read,
            ["Contact Acme Corp", "Client Acme Corp"],
            "{producer}"
        );
        assert_eq!(
            doc.count_text("Acme Corp").unwrap(),
            vec![(StoryKind::Body, 2)],
            "{producer}"
        );
        assert_eq!(
            doc.try_replace_text("Acme Corp", "Globex").unwrap(),
            2,
            "{producer}"
        );
        assert_eq!(doc.count_text("Acme Corp").unwrap(), vec![], "{producer}");
    }
}

#[test]
fn a_text_box_inside_a_block_custom_xml_element_is_listed_after_its_anchor() {
    let block = r#"<w:customXml w:element="section"><w:p><w:r><w:t xml:space="preserve">Acme anchor </w:t></w:r><w:r><mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:v="urn:schemas-microsoft-com:vml"><mc:Choice Requires="wps"><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="914400" cy="914400"/><wp:docPr id="79" name="Text Box 79"/><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:wsp><wps:cNvSpPr txBox="1"/><wps:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="914400"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></wps:spPr><wps:txbx><w:txbxContent><w:p><w:r><w:t>Acme boxed</w:t></w:r></w:p></w:txbxContent></wps:txbx><wps:bodyPr/></wps:wsp></a:graphicData></a:graphic></wp:inline></w:drawing></mc:Choice><mc:Fallback><w:pict><v:shape id="tb79"><v:textbox><w:txbxContent><w:p><w:r><w:t>Acme boxed</w:t></w:r></w:p></w:txbxContent></v:textbox></v:shape></w:pict></mc:Fallback></mc:AlternateContent></w:r></w:p></w:customXml><w:p><w:r><w:t>after the element</w:t></w:r></w:p>"#;
    for producer in PRODUCERS {
        let doc = body_with(producer, block);
        let paragraphs = doc.story_paragraphs().unwrap();
        let anchor = position(&paragraphs, "Acme anchor ");
        assert_eq!(
            position(&paragraphs, "Acme boxed"),
            anchor + 1,
            "{producer}"
        );
        assert_eq!(
            position(&paragraphs, "after the element"),
            anchor + 2,
            "{producer}"
        );
        assert_eq!(
            paragraphs
                .iter()
                .filter(|paragraph| paragraph.text == "Acme boxed")
                .count(),
            1,
            "{producer}: the VML fallback is not read again"
        );
        assert_eq!(
            doc.count_text("Acme").unwrap(),
            vec![(StoryKind::Body, 2)],
            "{producer}"
        );
    }
}

#[test]
fn a_smart_tag_text_box_is_read_from_the_first_choice_that_holds_it_only() {
    let tag =
        r#"<w:smartTag w:uri="urn:schemas-microsoft-com:office:smarttags" w:element="PersonName">"#;
    let run = crate::support::text_box_run(83, "<w:p><w:r><w:t>Acme boxed</w:t></w:r></w:p>");
    // A second choice holding the same box, as the story scan leaves out.
    let choice = &run[run.find("<mc:Choice").unwrap()..run.find("<mc:Fallback").unwrap()];
    let run = run.replacen("<mc:Fallback", &format!("{choice}<mc:Fallback"), 1);
    let body = format!(r#"<w:p><w:r><w:t>anchor</w:t></w:r>{tag}{run}</w:smartTag></w:p>"#);
    for producer in PRODUCERS {
        let doc = body_with(producer, &body);
        assert_eq!(
            doc.count_text("Acme").unwrap(),
            vec![(StoryKind::Body, 1)],
            "{producer}"
        );
    }
}
