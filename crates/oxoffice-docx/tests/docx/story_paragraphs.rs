//! `Document::story_paragraphs` lists every paragraph of every story with its
//! story kind, part and style, in the accepted view, and `count_text` counts a
//! text by story kind, on the Word and Google Docs files of the corpus.

use std::io::Cursor;
use std::path::Path;

use oxoffice_docx::{Document, HdrFtrType, HeaderFooterKind, StoryKind, StoryParagraph};
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
fn every_story_of_the_corpus_reports_is_listed_with_its_kind_part_and_style() {
    for producer in PRODUCERS {
        let doc = Document::from_bytes(&corpus_bytes(producer)).unwrap();
        let paragraphs = doc.story_paragraphs().unwrap();
        let heading_style = if producer == "word" {
            "Titre1"
        } else {
            "Heading1"
        };

        let heading = find(&paragraphs, "1. Highlights");
        assert_eq!(heading.kind, StoryKind::Body, "{producer}");
        assert_eq!(heading.part_name(), "/word/document.xml", "{producer}");
        assert_eq!(heading.style.as_deref(), Some(heading_style), "{producer}");
        let summary = paragraphs
            .iter()
            .find(|paragraph| paragraph.text.starts_with("Summary. Revenue grew"))
            .unwrap();
        assert_eq!(
            summary.style, None,
            "{producer}: a paragraph without w:pStyle"
        );

        let cell = find(&paragraphs, "Indicator");
        assert_eq!(cell.kind, StoryKind::Body, "{producer}");
        assert_eq!(
            cell.location.story().kind(),
            StoryKind::TableCell,
            "{producer}"
        );

        let header = find(
            &paragraphs,
            "Northwind Analytics: Quarterly Operations Review",
        );
        assert_eq!(header.kind, StoryKind::Header, "{producer}");
        assert_eq!(header.part_name(), "/word/header1.xml", "{producer}");
        let footer = find(&paragraphs, "Internal. Prepared by the operations team.");
        assert_eq!(footer.kind, StoryKind::Footer, "{producer}");
        assert_eq!(footer.part_name(), "/word/footer1.xml", "{producer}");
        let comment = find(&paragraphs, "Confirm with finance before sending.");
        assert_eq!(comment.kind, StoryKind::Comment, "{producer}");
    }
}

#[test]
fn table_cells_come_where_the_table_is() {
    for producer in PRODUCERS {
        let doc = Document::from_bytes(&corpus_bytes(producer)).unwrap();
        let paragraphs = doc.story_paragraphs().unwrap();
        let before = paragraphs
            .iter()
            .position(|paragraph| paragraph.text.starts_with("The table below compares"))
            .unwrap();
        let cell = position(&paragraphs, "Indicator");
        let after = paragraphs
            .iter()
            .position(|paragraph| paragraph.text.starts_with("Figure 1."))
            .unwrap();
        assert!(
            before < cell && cell < after,
            "{producer}: {before} {cell} {after}"
        );
    }
}

#[test]
fn paragraphs_of_a_block_content_control_are_listed_one_by_one() {
    let toc = r#"<w:sdt><w:sdtPr><w:docPartObj><w:docPartGallery w:val="Table of Contents"/><w:docPartUnique/></w:docPartObj></w:sdtPr><w:sdtContent><w:p><w:pPr><w:pStyle w:val="TOC1"/></w:pPr><w:r><w:t>First entry</w:t></w:r></w:p><w:sdt><w:sdtContent><w:p><w:pPr><w:pStyle w:val="TOC2"/></w:pPr><w:r><w:t>Nested entry</w:t></w:r></w:p></w:sdtContent></w:sdt><w:tbl><w:tr><w:tc><w:p><w:r><w:t>Cell in control</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:sdtContent></w:sdt>"#;
    for producer in PRODUCERS {
        let doc = body_with(producer, toc);
        let paragraphs = doc.story_paragraphs().unwrap();
        let first = find(&paragraphs, "First entry");
        assert_eq!(first.style.as_deref(), Some("TOC1"), "{producer}");
        assert_eq!(
            first.location.index_path().len(),
            2,
            "{producer}: control, paragraph"
        );
        assert_eq!(
            find(&paragraphs, "Nested entry").style.as_deref(),
            Some("TOC2")
        );
        let cell = find(&paragraphs, "Cell in control");
        assert_eq!(
            cell.location.story().kind(),
            StoryKind::TableCell,
            "{producer}"
        );
        assert!(
            !paragraphs
                .iter()
                .any(|paragraph| paragraph.text.contains("First entryNested")),
            "{producer}: the control is not read as one paragraph"
        );
        assert!(position(&paragraphs, "First entry") < position(&paragraphs, "Cell in control"));
    }
}

#[test]
fn tracked_changes_read_in_the_accepted_view() {
    let revisions = r#"<w:p><w:r><w:t xml:space="preserve">kept </w:t></w:r><w:ins w:id="901" w:author="a" w:date="2026-01-01T00:00:00Z"><w:r><w:t>inserted</w:t></w:r></w:ins><w:del w:id="902" w:author="a" w:date="2026-01-01T00:00:00Z"><w:r><w:delText>deleted</w:delText></w:r></w:del></w:p>"#;
    for producer in PRODUCERS {
        let doc = body_with(producer, revisions);
        let paragraphs = doc.story_paragraphs().unwrap();
        find(&paragraphs, "kept inserted");
        assert!(
            !paragraphs
                .iter()
                .any(|paragraph| paragraph.text.contains("deleted")),
            "{producer}"
        );
    }
}

#[test]
fn a_tracked_style_change_reports_the_current_style() {
    let changed = r#"<w:p><w:pPr><w:pStyle w:val="Heading2"/><w:pPrChange w:id="903" w:author="a" w:date="2026-01-01T00:00:00Z"><w:pPr><w:pStyle w:val="Heading1"/></w:pPr></w:pPrChange></w:pPr><w:r><w:t>Restyled</w:t></w:r></w:p>"#;
    for producer in PRODUCERS {
        let doc = body_with(producer, changed);
        let paragraphs = doc.story_paragraphs().unwrap();
        assert_eq!(
            find(&paragraphs, "Restyled").style.as_deref(),
            Some("Heading2")
        );
    }
}

#[test]
fn content_control_field_and_hyperlink_text_belongs_to_its_paragraph() {
    let inline = r#"<w:p><w:r><w:t>AC</w:t></w:r><w:sdt><w:sdtContent><w:r><w:t>ME</w:t></w:r></w:sdtContent></w:sdt><w:r><w:t xml:space="preserve"> by </w:t></w:r><w:fldSimple w:instr=" AUTHOR "><w:r><w:t>Ann</w:t></w:r></w:fldSimple><w:hyperlink w:anchor="top"><w:r><w:t xml:space="preserve"> here</w:t></w:r></w:hyperlink></w:p>"#;
    for producer in PRODUCERS {
        let doc = body_with(producer, inline);
        let paragraphs = doc.story_paragraphs().unwrap();
        find(&paragraphs, "ACME by Ann here");
        assert!(
            !paragraphs.iter().any(|paragraph| paragraph.text == "ME"),
            "{producer}"
        );
    }
}

#[test]
fn a_text_box_is_read_once_right_after_its_anchor() {
    let text_box = r#"<w:p><w:r><w:t xml:space="preserve">anchor </w:t></w:r><w:r><mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:v="urn:schemas-microsoft-com:vml"><mc:Choice Requires="wps"><w:drawing><wp:anchor distT="0" distB="0" distL="0" distR="0" simplePos="0" relativeHeight="1" behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="column"><wp:posOffset>0</wp:posOffset></wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset></wp:positionV><wp:extent cx="914400" cy="914400"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:wrapNone/><wp:docPr id="77" name="Text Box 77"/><wp:cNvGraphicFramePr/><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:wsp><wps:cNvSpPr txBox="1"/><wps:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="914400"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></wps:spPr><wps:txbx><w:txbxContent><w:p><w:pPr><w:pStyle w:val="Caption"/></w:pPr><w:r><w:t>boxed text</w:t></w:r></w:p></w:txbxContent></wps:txbx><wps:bodyPr/></wps:wsp></a:graphicData></a:graphic></wp:anchor></w:drawing></mc:Choice><mc:Fallback><w:pict><v:shape id="tb77" style="width:72pt;height:72pt"><v:textbox><w:txbxContent><w:p><w:r><w:t>boxed text</w:t></w:r></w:p></w:txbxContent></v:textbox></v:shape></w:pict></mc:Fallback></mc:AlternateContent></w:r></w:p><w:p><w:r><w:t>after the box</w:t></w:r></w:p>"#;
    for producer in PRODUCERS {
        let doc = body_with(producer, text_box);
        let paragraphs = doc.story_paragraphs().unwrap();
        let boxed = paragraphs
            .iter()
            .filter(|paragraph| paragraph.text == "boxed text")
            .collect::<Vec<_>>();
        assert_eq!(
            boxed.len(),
            1,
            "{producer}: the VML fallback is not read again"
        );
        assert_eq!(boxed[0].kind, StoryKind::Body, "{producer}");
        assert_eq!(
            boxed[0].location.story().kind(),
            StoryKind::TextBox,
            "{producer}"
        );
        assert_eq!(boxed[0].style.as_deref(), Some("Caption"), "{producer}");
        assert_eq!(find(&paragraphs, "anchor ").text, "anchor ", "{producer}");
        let anchor = position(&paragraphs, "anchor ");
        assert_eq!(
            position(&paragraphs, "boxed text"),
            anchor + 1,
            "{producer}"
        );
        assert_eq!(
            position(&paragraphs, "after the box"),
            anchor + 2,
            "{producer}"
        );
    }
}

#[test]
fn a_table_in_a_header_takes_the_header_kind() {
    let table = r#"<w:tbl><w:tr><w:tc><w:p><w:pPr><w:pStyle w:val="HeaderCell"/></w:pPr><w:r><w:t>Header cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
    for producer in PRODUCERS {
        let doc = report_with(producer, "/word/header1.xml", table);
        let paragraphs = doc.story_paragraphs().unwrap();
        let cell = find(&paragraphs, "Header cell");
        assert_eq!(cell.kind, StoryKind::Header, "{producer}");
        assert_eq!(cell.part_name(), "/word/header1.xml", "{producer}");
        assert_eq!(
            cell.location.story().kind(),
            StoryKind::TableCell,
            "{producer}"
        );
        assert_eq!(cell.style.as_deref(), Some("HeaderCell"), "{producer}");
    }
}

#[test]
fn notes_are_listed_without_their_separators() {
    for producer in PRODUCERS {
        let mut doc = Document::from_bytes(&corpus_bytes(producer)).unwrap();
        doc.add_footnote("A note about ACME");
        let doc = Document::from_bytes(&doc.to_bytes().unwrap()).unwrap();
        let notes = doc
            .story_paragraphs()
            .unwrap()
            .into_iter()
            .filter(|paragraph| paragraph.kind == StoryKind::Footnote)
            .collect::<Vec<_>>();
        assert_eq!(notes.len(), 1, "{producer}: {notes:#?}");
        assert!(notes[0].text.ends_with("A note about ACME"), "{producer}");
    }
}

#[test]
fn text_is_counted_by_story_kind_in_a_fixed_order() {
    let body = r#"<w:p><w:r><w:t>ACME and ACME</w:t></w:r></w:p>"#;
    for producer in PRODUCERS {
        let mut doc = report_with(
            producer,
            "/word/header1.xml",
            r#"<w:p><w:r><w:t>ACME</w:t></w:r></w:p>"#,
        );
        let mut package = OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).unwrap();
        let main =
            String::from_utf8(package.get_part("/word/document.xml").unwrap().to_vec()).unwrap();
        let at = main.rfind("<w:sectPr").unwrap();
        package.set_part(
            "/word/document.xml",
            format!("{}{body}{}", &main[..at], &main[at..]).into_bytes(),
        );
        let mut bytes = Cursor::new(Vec::new());
        package.write_to(&mut bytes).unwrap();
        doc = Document::from_bytes(&bytes.into_inner()).unwrap();
        doc.add_footnote("ACME again");
        assert_eq!(
            doc.count_text("ACME").unwrap(),
            vec![
                (StoryKind::Body, 2),
                (StoryKind::Header, 1),
                (StoryKind::Footnote, 1)
            ],
            "{producer}"
        );
        assert_eq!(
            doc.count_text("finance").unwrap(),
            vec![(StoryKind::Comment, 1)]
        );
        assert_eq!(doc.count_text("not in the document").unwrap(), vec![]);
    }
}

#[test]
fn counts_include_text_that_replacement_cannot_reach() {
    // The second ACME straddles the edge of a content control.
    let split = r#"<w:p><w:r><w:t>ACME</w:t></w:r></w:p><w:p><w:r><w:t>AC</w:t></w:r><w:sdt><w:sdtContent><w:r><w:t>ME</w:t></w:r></w:sdtContent></w:sdt></w:p>"#;
    for producer in PRODUCERS {
        let mut doc = body_with(producer, split);
        assert_eq!(
            doc.count_text("ACME").unwrap(),
            vec![(StoryKind::Body, 2)],
            "{producer}"
        );
        assert_eq!(
            doc.try_replace_text("ACME", "Globex").unwrap(),
            1,
            "{producer}"
        );
        assert_eq!(
            doc.count_text("ACME").unwrap(),
            vec![(StoryKind::Body, 1)],
            "{producer}"
        );
    }
}

#[test]
fn counting_an_empty_text_is_refused() {
    let doc = Document::from_bytes(&corpus_bytes("word")).unwrap();
    assert!(doc.count_text("").is_err());
}

#[test]
fn body_paragraphs_read_as_paragraph_text_reads_them() {
    for file in crate::support::corpus_files("docx") {
        let doc = Document::from_bytes(&file.bytes()).unwrap();
        let listed = doc
            .story_paragraphs()
            .unwrap()
            .into_iter()
            .filter(|paragraph| paragraph.location.story().kind() == StoryKind::Body)
            .map(|paragraph| paragraph.text)
            .collect::<Vec<_>>();
        let read = doc
            .paragraphs()
            .iter()
            .map(|paragraph| paragraph.text())
            .collect::<Vec<_>>();
        assert_eq!(listed, read, "{}", file.path);
    }
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
fn a_block_custom_xml_element_lists_its_paragraphs_once_though_replacement_does_not_reach_them() {
    let block = r#"<w:customXml w:element="section"><w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>Acme Corp block</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>Acme Corp cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:customXml><w:p><w:r><w:t>after the element</w:t></w:r></w:p>"#;
    for producer in PRODUCERS {
        let mut doc = body_with(producer, block);
        let paragraphs = doc.story_paragraphs().unwrap();
        let heading = find(&paragraphs, "Acme Corp block");
        assert_eq!(heading.style.as_deref(), Some("Heading2"), "{producer}");
        assert_eq!(
            heading.location.index_path().len(),
            2,
            "{producer}: element, paragraph"
        );
        assert_eq!(
            paragraphs
                .iter()
                .filter(|paragraph| paragraph.text == "Acme Corp cell")
                .count(),
            1,
            "{producer}: the cell inside the element is listed once"
        );
        let block = position(&paragraphs, "Acme Corp block");
        assert_eq!(
            position(&paragraphs, "Acme Corp cell"),
            block + 1,
            "{producer}"
        );
        assert_eq!(
            position(&paragraphs, "after the element"),
            block + 2,
            "{producer}"
        );
        assert_eq!(
            doc.count_text("Acme Corp").unwrap(),
            vec![(StoryKind::Body, 2)],
            "{producer}"
        );
        assert_eq!(
            doc.try_replace_text("Acme Corp", "Globex").unwrap(),
            0,
            "{producer}"
        );
    }
}

#[test]
fn a_smart_tag_inside_a_tracked_insertion_is_read() {
    let inserted = r#"<w:p><w:r><w:t xml:space="preserve">Signed by </w:t></w:r><w:ins w:id="904" w:author="a" w:date="2026-01-01T00:00:00Z"><w:smartTag w:uri="urn:schemas-microsoft-com:office:smarttags" w:element="PersonName"><w:r><w:t>Acme Corp</w:t></w:r></w:smartTag></w:ins></w:p>"#;
    for producer in PRODUCERS {
        let doc = body_with(producer, inserted);
        find(&doc.story_paragraphs().unwrap(), "Signed by Acme Corp");
    }
}

#[test]
fn a_field_shows_its_result_not_its_code() {
    let fields = r#"<w:p><w:r><w:t xml:space="preserve">Total: </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> IF </w:instrText></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> MERGEFIELD amount </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>nested result</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:instrText xml:space="preserve"> = 0 "none" "some" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>some</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#;
    for producer in PRODUCERS {
        let doc = body_with(producer, fields);
        find(&doc.story_paragraphs().unwrap(), "Total: some");
    }
}

#[test]
fn an_endnote_is_listed_with_the_endnote_kind() {
    for producer in PRODUCERS {
        let mut doc = Document::from_bytes(&corpus_bytes(producer)).unwrap();
        let summary = doc
            .story_paragraphs()
            .unwrap()
            .into_iter()
            .find(|paragraph| paragraph.text.starts_with("Summary. Revenue grew"))
            .unwrap();
        doc.create_endnote(&summary.location, "An endnote about ACME")
            .unwrap();
        let notes = doc
            .story_paragraphs()
            .unwrap()
            .into_iter()
            .filter(|paragraph| paragraph.kind == StoryKind::Endnote)
            .collect::<Vec<_>>();
        assert_eq!(notes.len(), 1, "{producer}: {notes:#?}");
        assert!(
            notes[0].text.ends_with("An endnote about ACME"),
            "{producer}"
        );
        assert_eq!(notes[0].part_name(), "/word/endnotes.xml", "{producer}");
        assert_eq!(
            doc.count_text("ACME").unwrap(),
            vec![(StoryKind::Endnote, 1)],
            "{producer}"
        );
    }
}

#[test]
fn a_first_page_header_counts_its_text_once_more() {
    for producer in PRODUCERS {
        let mut doc = Document::from_bytes(&corpus_bytes(producer)).unwrap();
        doc.try_set_first_page_header("Northwind, first page")
            .unwrap();
        let headers = doc
            .story_paragraphs()
            .unwrap()
            .into_iter()
            .filter(|paragraph| paragraph.text.contains("Northwind"))
            .map(|paragraph| (paragraph.kind, paragraph.part_name().to_owned()))
            .collect::<Vec<_>>();
        assert_eq!(headers.len(), 2, "{producer}: {headers:?}");
        assert_ne!(headers[0].1, headers[1].1, "{producer}: two header parts");
        assert_eq!(
            doc.count_text("Northwind").unwrap(),
            vec![(StoryKind::Header, 2)],
            "{producer}"
        );
    }
}

#[test]
fn a_header_part_shared_by_two_sections_is_listed_once() {
    for producer in PRODUCERS {
        let mut doc = Document::from_bytes(&corpus_bytes(producer)).unwrap();
        assert_eq!(doc.section_count(), 2, "{producer}");
        let header = doc
            .section_story(0, HeaderFooterKind::Header, HdrFtrType::Default)
            .unwrap()
            .unwrap()
            .story()
            .clone();
        doc.link_section_story(1, HeaderFooterKind::Header, HdrFtrType::Default, &header)
            .unwrap();
        let linked = doc
            .section_story(1, HeaderFooterKind::Header, HdrFtrType::Default)
            .unwrap()
            .unwrap();
        assert!(
            !linked.is_inherited(),
            "{producer}: section 2 names the part itself"
        );
        assert_eq!(linked.story().part_name(), header.part_name(), "{producer}");
        assert_eq!(
            doc.count_text("Northwind").unwrap(),
            vec![(StoryKind::Header, 1)],
            "{producer}"
        );
    }
}

#[test]
fn a_ruby_reads_as_its_base_without_its_phonetic_guide() {
    let ruby = r#"<w:p><w:r><w:t>首都は</w:t></w:r><w:r><w:ruby><w:rubyPr><w:rubyAlign w:val="distributeSpace"/><w:hps w:val="10"/><w:hpsRaise w:val="18"/><w:hpsBaseText w:val="21"/><w:lid w:val="ja-JP"/></w:rubyPr><w:rt><w:r><w:t>とうきょう</w:t></w:r></w:rt><w:rubyBase><w:r><w:t>東京</w:t></w:r></w:rubyBase></w:ruby></w:r></w:p>"#;
    for producer in PRODUCERS {
        let mut doc = body_with(producer, ruby);
        find(&doc.story_paragraphs().unwrap(), "首都は東京");
        assert_eq!(doc.count_text("とうきょう").unwrap(), vec![], "{producer}");
        // Replacement does not reach a ruby, and `Paragraph::text` leaves it
        // out whole: the count names text a reader sees and nothing else
        // reaches.
        assert_eq!(
            doc.count_text("東京").unwrap(),
            vec![(StoryKind::Body, 1)],
            "{producer}"
        );
        assert_eq!(
            doc.try_replace_text("東京", "大阪").unwrap(),
            0,
            "{producer}"
        );
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
fn a_ruby_child_of_the_paragraph_reads_as_paragraph_text_reads_it() {
    let ruby = r#"<w:p><w:r><w:t>首都は</w:t></w:r><w:ruby><w:rubyPr><w:lid w:val="ja-JP"/></w:rubyPr><w:rt><w:r><w:t>とうきょう</w:t></w:r></w:rt><w:rubyBase><w:r><w:t>東京</w:t></w:r></w:rubyBase></w:ruby></w:p>"#;
    for producer in PRODUCERS {
        let doc = body_with(producer, ruby);
        let listed = doc
            .story_paragraphs()
            .unwrap()
            .into_iter()
            .map(|paragraph| paragraph.text)
            .filter(|text| text.starts_with("首都は"))
            .collect::<Vec<_>>();
        let read = doc
            .paragraphs()
            .iter()
            .map(|paragraph| paragraph.text())
            .filter(|text| text.starts_with("首都は"))
            .collect::<Vec<_>>();
        assert_eq!(listed, read, "{producer}");
        assert_eq!(listed, ["首都は東京"], "{producer}");
    }
}
