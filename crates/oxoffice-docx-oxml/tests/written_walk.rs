//! The written walk against the writer: for every body block, the style,
//! note and comment references read from the walk (raw fragments searched
//! by local name, typed owners read by field) are the ones a namespace-aware
//! reader finds in the bytes the writer writes for that block.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use oxoffice_docx_oxml::SdtContent;
use oxoffice_docx_oxml::namespace::W_NS;
use oxoffice_docx_oxml::text::{CommentRangeMarker, RunContent};
use oxoffice_docx_oxml::{
    BodyContent, CT_Document, CT_P, CT_PPr, CT_R, CT_RPr, CT_TblPr, WrittenVisitor,
};
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::{NsReader, Reader, XmlVersion};

type References = BTreeSet<(&'static str, String)>;

/// Element local name and the attribute local name that holds its reference.
const TARGETS: &[(&str, &str)] = &[
    ("pStyle", "val"),
    ("rStyle", "val"),
    ("tblStyle", "val"),
    ("footnoteReference", "id"),
    ("endnoteReference", "id"),
    ("commentRangeStart", "id"),
    ("commentRangeEnd", "id"),
    ("commentReference", "id"),
];

fn target(local: &[u8]) -> Option<(&'static str, &'static str)> {
    TARGETS
        .iter()
        .find(|(name, _)| name.as_bytes() == local)
        .copied()
}

fn local(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

/// Prefix-agnostic search of a raw fragment, as the consumer does it.
fn scan_raw(xml: &[u8], found: &mut References) {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().check_end_names = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) | Ok(Event::Empty(element)) => {
                let Some((name, attribute)) = target(local(element.name().as_ref())) else {
                    continue;
                };
                if let Some(value) = attribute_value(&element, attribute) {
                    found.insert((name, value));
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => panic!("raw fragment does not read: {error}"),
        }
    }
}

fn attribute_value(element: &BytesStart<'_>, name: &str) -> Option<String> {
    element.attributes().flatten().find_map(|attribute| {
        (local(attribute.key.as_ref()) == name.as_bytes()).then(|| {
            attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                .expect("attribute value")
                .into_owned()
        })
    })
}

/// Namespace-aware search of the bytes the writer writes.
fn scan_written(xml: &[u8], found: &mut References) {
    let mut reader = NsReader::from_reader(xml);
    loop {
        let (namespace, event) = reader.read_resolved_event().expect("written XML reads");
        match event {
            Event::Start(element) | Event::Empty(element) => {
                if namespace != ResolveResult::Bound(Namespace(W_NS.as_bytes())) {
                    continue;
                }
                let Some((name, attribute)) = target(element.local_name().as_ref()) else {
                    continue;
                };
                let value = element.attributes().flatten().find_map(|candidate| {
                    let (namespace, key) = reader.resolver().resolve_attribute(candidate.key);
                    (namespace == ResolveResult::Bound(Namespace(W_NS.as_bytes()))
                        && key.as_ref() == attribute.as_bytes())
                    .then(|| {
                        candidate
                            .decoded_and_normalized_value(
                                XmlVersion::Implicit1_0,
                                element.decoder(),
                            )
                            .expect("value")
                            .into_owned()
                    })
                });
                if let Some(value) = value {
                    found.insert((name, value));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
}

#[derive(Default)]
struct Walk {
    found: References,
    raw_fragments: usize,
}

impl WrittenVisitor for Walk {
    fn raw(&mut self, xml: &[u8]) {
        self.raw_fragments += 1;
        scan_raw(xml, &mut self.found);
    }

    fn paragraph(&mut self, paragraph: &CT_P) {
        for marker in &paragraph.comment_ranges {
            let (name, id) = match marker {
                CommentRangeMarker::Start { id, .. } => ("commentRangeStart", id),
                CommentRangeMarker::End { id, .. } => ("commentRangeEnd", id),
            };
            self.found.insert((name, id.to_string()));
        }
    }

    fn paragraph_properties(&mut self, properties: &CT_PPr) {
        if let Some(style) = &properties.style_id {
            self.found.insert(("pStyle", style.clone()));
        }
    }

    fn run(&mut self, run: &CT_R) {
        for content in &run.content {
            let (name, id) = match content {
                RunContent::FootnoteRef { id, .. } => ("footnoteReference", id),
                RunContent::EndnoteRef { id, .. } => ("endnoteReference", id),
                RunContent::CommentReference { id, .. } => ("commentReference", id),
                _ => continue,
            };
            self.found.insert((name, id.to_string()));
        }
    }

    fn run_properties(&mut self, properties: &CT_RPr) {
        if let Some(style) = &properties.style_id {
            self.found.insert(("rStyle", style.clone()));
        }
    }

    fn table_properties(&mut self, properties: &CT_TblPr) {
        if let Some(style) = &properties.style_id {
            self.found.insert(("tblStyle", style.clone()));
        }
    }
}

/// Check every block of `document` and the content outside the body;
/// return the walk's references of each block.
fn check_document(document: &CT_Document, label: &str) -> Vec<References> {
    let mut blocks = Vec::new();
    for (index, block) in document.body.content.iter().enumerate() {
        let written = document
            .standalone_body_item_xml(block)
            .unwrap_or_else(|error| panic!("{label}: block {index} does not write: {error}"));
        let mut expected = References::new();
        scan_written(&written, &mut expected);
        let mut walk = Walk::default();
        block.visit_written(&mut walk);
        assert_eq!(
            walk.found,
            expected,
            "{label}: block {index} walk differs from the writer\n{}",
            String::from_utf8_lossy(&written)
        );
        blocks.push(walk.found);
    }
    let (head, tail) = document.body_frame_xml().expect("frame writes");
    let mut expected = References::new();
    let mut frame = head;
    frame.extend_from_slice(&tail);
    scan_written(&frame, &mut expected);
    let mut walk = Walk::default();
    document.visit_written_outside_body(&mut walk);
    assert_eq!(walk.found, expected, "{label}: outside the body");
    blocks
}

const ROOT: &str = concat!(
    r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#,
    r#" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#,
    r#" xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math""#,
    r#" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006""#,
    r#" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing""#,
    r#" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main""#,
    r#" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture""#,
    r#" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape""#,
    r#" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml""#,
    r#" xmlns:v="urn:schemas-microsoft-com:vml""#,
    r#" xmlns:x="urn:example:other""#,
    r#" mc:Ignorable="w14">"#,
);

fn document(body: &str) -> CT_Document {
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>{ROOT}<w:body>{body}</w:body></w:document>"#
    );
    CT_Document::from_xml(xml.as_bytes())
        .unwrap_or_else(|error| panic!("fixture does not read: {error}\n{body}"))
}

fn text_box(paragraph: &str) -> String {
    format!(
        concat!(
            r#"<wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="10" cy="10"/>"#,
            r#"<wp:docPr id="7" name="Box"/><a:graphic>"#,
            r#"<a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape">"#,
            r#"<wps:wsp><wps:spPr><a:prstGeom prst="rect"/></wps:spPr><wps:txbx><w:txbxContent>{}"#,
            r#"</w:txbxContent></wps:txbx><wps:bodyPr/></wps:wsp></a:graphicData></a:graphic></wp:inline>"#,
        ),
        paragraph
    )
}

/// Each fixture: a label, the body XML and the references the block must
/// show (a subset: the walk must reach them, the writer must write them).
/// A label, the body XML and the references the fixture must show.
type Fixture = (&'static str, String, Vec<(&'static str, &'static str)>);

fn fixtures() -> Vec<Fixture> {
    vec![
        (
            "tracked insertion",
            concat!(
                r#"<w:p><w:pPr><w:pStyle w:val="InsP"/></w:pPr>"#,
                r#"<w:ins w:id="1" w:author="A" w:date="2024-01-01T00:00:00Z">"#,
                r#"<w:r><w:rPr><w:rStyle w:val="InsR"/></w:rPr><w:t>x</w:t></w:r>"#,
                r#"<w:r><w:footnoteReference w:id="7"/></w:r></w:ins></w:p>"#,
            )
            .to_owned(),
            vec![("pStyle", "InsP"), ("rStyle", "InsR"), ("footnoteReference", "7")],
        ),
        (
            "alternate content text box",
            format!(
                concat!(
                    r#"<w:p><w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:drawing>{}"#,
                    r#"</w:drawing></mc:Choice><mc:Fallback><w:pict><v:rect/></w:pict></mc:Fallback>"#,
                    r#"</mc:AlternateContent></w:r></w:p>"#,
                ),
                text_box(concat!(
                    r#"<w:p><w:pPr><w:pStyle w:val="BoxP"/></w:pPr>"#,
                    r#"<w:r><w:footnoteReference w:id="8"/></w:r></w:p>"#
                ))
            ),
            vec![("pStyle", "BoxP"), ("footnoteReference", "8")],
        ),
        (
            "raw body block",
            concat!(
                r#"<x:block><w:p><w:pPr><w:pStyle w:val="RawP"/></w:pPr>"#,
                r#"<w:r><w:endnoteReference w:id="4"/></w:r></w:p></x:block>"#,
            )
            .to_owned(),
            vec![("pStyle", "RawP"), ("endnoteReference", "4")],
        ),
        (
            "complex field",
            concat!(
                r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
                r#"<w:r><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r>"#,
                r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r>"#,
                r#"<w:r><w:rPr><w:rStyle w:val="FieldR"/></w:rPr><w:t>1</w:t></w:r>"#,
                r#"<w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
                r#"<w:r><w:rPr><w:rStyle w:val="AfterR"/></w:rPr><w:t>a</w:t></w:r></w:p>"#,
            )
            .to_owned(),
            vec![("rStyle", "FieldR"), ("rStyle", "AfterR")],
        ),
        (
            "field sharing a run with text",
            concat!(
                r#"<w:p><w:r><w:rPr><w:rStyle w:val="SharedR"/></w:rPr><w:t>a</w:t>"#,
                r#"<w:fldChar w:fldCharType="begin"/><w:instrText> PAGE </w:instrText>"#,
                r#"<w:fldChar w:fldCharType="separate"/><w:t>1</w:t>"#,
                r#"<w:fldChar w:fldCharType="end"/><w:t>b</w:t></w:r></w:p>"#,
            )
            .to_owned(),
            vec![("rStyle", "SharedR")],
        ),
        (
            "simple field",
            concat!(
                r#"<w:p><w:fldSimple w:instr=" PAGE "><w:r><w:rPr><w:rStyle w:val="SimpleR"/>"#,
                r#"</w:rPr><w:t>1</w:t></w:r></w:fldSimple></w:p>"#,
            )
            .to_owned(),
            vec![("rStyle", "SimpleR")],
        ),
        (
            "ruby",
            concat!(
                r#"<w:p><w:r><w:rPr><w:rStyle w:val="RubyRun"/></w:rPr><w:ruby><w:rubyPr>"#,
                r#"<w:rubyAlign w:val="center"/><w:hps w:val="10"/><w:hpsRaise w:val="18"/>"#,
                r#"<w:hpsBaseText w:val="21"/><w:lid w:val="ja-JP"/></w:rubyPr>"#,
                r#"<w:rt><w:r><w:rPr><w:rStyle w:val="RubyText"/></w:rPr><w:t>ka</w:t></w:r></w:rt>"#,
                r#"<w:rubyBase><w:r><w:rPr><w:rStyle w:val="RubyBase"/></w:rPr><w:t>K</w:t></w:r>"#,
                r#"</w:rubyBase></w:ruby></w:r></w:p>"#,
            )
            .to_owned(),
            vec![
                ("rStyle", "RubyRun"),
                ("rStyle", "RubyText"),
                ("rStyle", "RubyBase"),
            ],
        ),
        (
            "nested content controls",
            concat!(
                r#"<w:sdt><w:sdtPr><w:rPr><w:rStyle w:val="SdtR"/></w:rPr><w:alias w:val="a"/>"#,
                r#"</w:sdtPr><w:sdtEndPr><w:rPr><w:rStyle w:val="SdtEnd"/></w:rPr></w:sdtEndPr>"#,
                r#"<w:sdtContent><w:sdt><w:sdtPr><w:rPr><w:rStyle w:val="InnerR"/></w:rPr>"#,
                r#"</w:sdtPr><w:sdtContent><w:p><w:pPr><w:pStyle w:val="InnerP"/></w:pPr>"#,
                r#"<w:sdt><w:sdtPr><w:rPr><w:rStyle w:val="InlineSdtR"/></w:rPr></w:sdtPr>"#,
                r#"<w:sdtContent><w:r><w:rPr><w:rStyle w:val="InlineRunR"/></w:rPr>"#,
                r#"<w:t>y</w:t></w:r></w:sdtContent></w:sdt></w:p></w:sdtContent></w:sdt>"#,
                r#"</w:sdtContent></w:sdt>"#,
            )
            .to_owned(),
            vec![
                ("rStyle", "SdtR"),
                ("rStyle", "SdtEnd"),
                ("rStyle", "InnerR"),
                ("pStyle", "InnerP"),
                ("rStyle", "InlineSdtR"),
                ("rStyle", "InlineRunR"),
            ],
        ),
        (
            "inline content control with a field and a tracked run",
            concat!(
                r#"<w:p><w:sdt><w:sdtPr><w:alias w:val="f"/></w:sdtPr><w:sdtContent>"#,
                r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
                r#"<w:r><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r>"#,
                r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r>"#,
                r#"<w:r><w:rPr><w:rStyle w:val="SdtFieldR"/></w:rPr><w:t>1</w:t></w:r>"#,
                r#"<w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
                r#"<w:ins w:id="12" w:author="A"><w:r><w:rPr><w:rStyle w:val="SdtInsR"/></w:rPr>"#,
                r#"<w:t>s</w:t></w:r></w:ins>"#,
                r#"<w:r><w:rPr><w:rStyle w:val="SdtRunR"/></w:rPr><w:t>t</w:t></w:r>"#,
                r#"</w:sdtContent></w:sdt></w:p>"#,
                r#"<w:p><w:sdt><w:sdtPr/><w:sdtContent>"#,
                r#"<w:ins w:id="13" w:author="A"><w:r><w:rPr><w:rStyle w:val="SdtIns2R"/></w:rPr>"#,
                r#"<w:t>s</w:t></w:r></w:ins>"#,
                r#"<w:r><w:rPr><w:rStyle w:val="SdtRun2R"/></w:rPr><w:footnoteReference w:id="5"/></w:r>"#,
                r#"</w:sdtContent></w:sdt></w:p>"#,
            )
            .to_owned(),
            vec![
                ("rStyle", "SdtFieldR"),
                ("rStyle", "SdtInsR"),
                ("rStyle", "SdtRunR"),
                ("rStyle", "SdtIns2R"),
                ("rStyle", "SdtRun2R"),
                ("footnoteReference", "5"),
            ],
        ),
        (
            "table",
            concat!(
                r#"<w:tbl><w:tblPr><w:tblStyle w:val="TblNew"/><w:tblW w:w="0" w:type="auto"/>"#,
                r#"<w:tblPrChange w:id="3" w:author="A"><w:tblPr><w:tblStyle w:val="TblOld"/>"#,
                r#"</w:tblPr></w:tblPrChange></w:tblPr><w:tblGrid><w:gridCol w:w="100"/></w:tblGrid>"#,
                r#"<w:tr><w:tc><w:tcPr><w:tcW w:w="100" w:type="dxa"/></w:tcPr>"#,
                r#"<w:p><w:pPr><w:pStyle w:val="CellP"/></w:pPr>"#,
                r#"<w:r><w:endnoteReference w:id="3"/></w:r></w:p></w:tc></w:tr>"#,
                r#"<w:sdt><w:sdtPr><w:rPr><w:rStyle w:val="RowSdtR"/></w:rPr></w:sdtPr>"#,
                r#"<w:sdtContent><w:tr><w:tc><w:p><w:pPr><w:pStyle w:val="RowSdtP"/></w:pPr>"#,
                r#"</w:p></w:tc></w:tr></w:sdtContent></w:sdt></w:tbl>"#,
            )
            .to_owned(),
            vec![
                ("tblStyle", "TblNew"),
                ("tblStyle", "TblOld"),
                ("pStyle", "CellP"),
                ("endnoteReference", "3"),
                ("rStyle", "RowSdtR"),
                ("pStyle", "RowSdtP"),
            ],
        ),
        (
            "property changes",
            concat!(
                r#"<w:p><w:pPr><w:pStyle w:val="NewP"/><w:rPr><w:rStyle w:val="MarkR"/></w:rPr>"#,
                r#"<w:pPrChange w:id="4" w:author="A"><w:pPr><w:pStyle w:val="OldP"/></w:pPr>"#,
                r#"</w:pPrChange></w:pPr><w:r><w:rPr><w:rStyle w:val="NewR"/>"#,
                r#"<w:rPrChange w:id="5" w:author="A"><w:rPr><w:rStyle w:val="OldR"/></w:rPr>"#,
                r#"</w:rPrChange></w:rPr><w:t>z</w:t></w:r></w:p>"#,
            )
            .to_owned(),
            vec![
                ("pStyle", "NewP"),
                ("rStyle", "MarkR"),
                ("pStyle", "OldP"),
                ("rStyle", "NewR"),
                ("rStyle", "OldR"),
            ],
        ),
        (
            "comment",
            concat!(
                r#"<w:p><w:commentRangeStart w:id="9"/><w:r><w:t>c</w:t></w:r>"#,
                r#"<w:commentRangeEnd w:id="9"/><w:r><w:rPr><w:rStyle w:val="CommentReference"/>"#,
                r#"</w:rPr><w:commentReference w:id="9" w14:extra="1"/></w:r>"#,
                r#"<w:r><w:commentReference w:id="10"/></w:r></w:p>"#,
            )
            .to_owned(),
            vec![
                ("commentRangeStart", "9"),
                ("commentRangeEnd", "9"),
                ("commentReference", "9"),
                ("commentReference", "10"),
                ("rStyle", "CommentReference"),
            ],
        ),
        (
            "hyperlinks",
            concat!(
                r#"<w:p><w:hyperlink w:anchor="x"><w:r><w:rPr><w:rStyle w:val="Hyperlink"/></w:rPr>"#,
                r#"<w:t>h</w:t></w:r></w:hyperlink><w:hyperlink w:anchor="y"/>"#,
                r#"<w:hyperlink w:anchor="z"><w:ins w:id="11" w:author="A"><w:r><w:rPr>"#,
                r#"<w:rStyle w:val="HyperlinkIns"/></w:rPr><w:t>i</w:t></w:r></w:ins></w:hyperlink>"#,
                r#"<w:hyperlink w:anchor="u"><x:child><w:pStyle w:val="HyperlinkRaw"/></x:child>"#,
                r#"<w:r><w:t>u</w:t></w:r></w:hyperlink></w:p>"#,
            )
            .to_owned(),
            vec![
                ("rStyle", "Hyperlink"),
                ("rStyle", "HyperlinkIns"),
                ("pStyle", "HyperlinkRaw"),
            ],
        ),
        (
            "equation",
            concat!(
                r#"<w:p><m:oMath><m:r><w:rPr><w:rStyle w:val="MathR"/></w:rPr><m:t>x</m:t>"#,
                r#"</m:r></m:oMath></w:p>"#,
            )
            .to_owned(),
            vec![("rStyle", "MathR")],
        ),
        (
            "drawing with kept bytes",
            format!(
                r#"<w:p><w:r><w:drawing>{}</w:drawing></w:r></w:p>"#,
                text_box(r#"<w:p><w:pPr><w:pStyle w:val="InlineBoxP"/></w:pPr></w:p>"#)
            ),
            vec![("pStyle", "InlineBoxP")],
        ),
        (
            "unknown children",
            concat!(
                r#"<w:p><x:child><w:r><w:rPr><w:rStyle w:val="ParagraphChildR"/></w:rPr>"#,
                r#"<w:t>q</w:t></w:r></x:child><w:r><x:child><w:rStyle w:val="RunChildR"/>"#,
                r#"</x:child><w:t>r</w:t></w:r></w:p>"#,
            )
            .to_owned(),
            vec![("rStyle", "ParagraphChildR"), ("rStyle", "RunChildR")],
        ),
        (
            "section in a paragraph",
            concat!(
                r#"<w:p><w:pPr><w:pStyle w:val="SectionP"/><w:sectPr><w:pgSz w:w="12240" w:h="15840"/>"#,
                r#"<x:child/></w:sectPr></w:pPr></w:p>"#,
            )
            .to_owned(),
            vec![("pStyle", "SectionP")],
        ),
    ]
}

#[test]
fn walk_reaches_every_reference_the_writer_writes() {
    for (label, body, required) in fixtures() {
        let document = document(&body);
        let blocks = check_document(&document, label);
        let all = blocks.into_iter().flatten().collect::<References>();
        for (name, value) in required {
            assert!(
                all.contains(&(name, value.to_owned())),
                "{label}: {name} {value} is not written: {all:?}"
            );
        }
    }
}

#[test]
fn raw_body_block_is_one_raw_fragment() {
    let document =
        document(r#"<x:block><w:p><w:pPr><w:pStyle w:val="RawP"/></w:pPr></w:p></x:block>"#);
    assert!(matches!(document.body.content[0], BodyContent::RawXml(_)));
    let mut walk = Walk::default();
    document.body.content[0].visit_written(&mut walk);
    assert_eq!(walk.raw_fragments, 1);
}

#[test]
fn edited_field_is_walked_from_what_the_writer_writes() {
    let mut document = document(concat!(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
        r#"<w:r><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r>"#,
        r#"<w:r><w:rPr><w:rStyle w:val="FieldR"/></w:rPr><w:t>1</w:t></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
        r#"<w:r><w:rPr><w:rStyle w:val="AfterR"/></w:rPr><w:t>a</w:t></w:r></w:p>"#,
    ));
    let BodyContent::Paragraph(paragraph) = &mut document.body.content[0] else {
        panic!("a paragraph");
    };
    let mut edited = false;
    for run in &mut paragraph.runs {
        for content in &mut run.content {
            if let RunContent::Field(field) = content {
                let mut run = CT_R::new("2");
                run.properties = Some(CT_RPr {
                    style_id: Some("NewFieldR".to_owned()),
                    ..CT_RPr::default()
                });
                field.set_cached_runs(vec![run]).expect("result runs set");
                edited = true;
            }
        }
    }
    assert!(edited, "the fixture holds a field");
    let blocks = check_document(&document, "edited field");
    // The writer writes the new result, not the stored source's.
    assert!(blocks[0].contains(&("rStyle", "NewFieldR".to_owned())));
    assert!(!blocks[0].contains(&("rStyle", "FieldR".to_owned())));
    assert!(blocks[0].contains(&("rStyle", "AfterR".to_owned())));
}

#[test]
fn edited_field_in_an_inline_content_control_is_walked_from_what_the_writer_writes() {
    let mut document = document(concat!(
        r#"<w:p><w:sdt><w:sdtPr><w:alias w:val="f"/></w:sdtPr><w:sdtContent>"#,
        r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
        r#"<w:r><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r>"#,
        r#"<w:r><w:rPr><w:rStyle w:val="FieldR"/></w:rPr><w:t>1</w:t></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
        r#"</w:sdtContent></w:sdt></w:p>"#,
    ));
    let BodyContent::Paragraph(paragraph) = &mut document.body.content[0] else {
        panic!("a paragraph");
    };
    let control = &mut paragraph.content_controls[0].3;
    let mut edited = false;
    for child in &mut control.content {
        if let SdtContent::Run(run) = child {
            for content in &mut run.content {
                if let RunContent::Field(field) = content {
                    let mut result = CT_R::new("2");
                    result.properties = Some(CT_RPr {
                        style_id: Some("NewFieldR".to_owned()),
                        ..CT_RPr::default()
                    });
                    field
                        .set_cached_runs(vec![result])
                        .expect("result runs set");
                    edited = true;
                }
            }
        }
    }
    assert!(edited, "the control holds a field run");
    let blocks = check_document(&document, "edited control field");
    assert!(blocks[0].contains(&("rStyle", "NewFieldR".to_owned())));
}

fn corpus_documents() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/files");
    let mut found = Vec::new();
    let mut pending = vec![root];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "docx")
            {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

#[test]
fn walk_matches_the_writer_on_the_corpus() {
    let documents = corpus_documents();
    assert!(!documents.is_empty(), "the corpus holds Word documents");
    let mut checked = 0;
    for path in documents {
        let file = std::fs::File::open(&path).expect("corpus file opens");
        let mut archive = zip::ZipArchive::new(file).expect("corpus file is a package");
        let mut xml = Vec::new();
        let mut part = archive
            .by_name("word/document.xml")
            .unwrap_or_else(|_| panic!("{}: no main part at word/document.xml", path.display()));
        part.read_to_end(&mut xml).expect("main part reads");
        let document = CT_Document::from_xml(&xml)
            .unwrap_or_else(|error| panic!("{}: does not read: {error}", path.display()));
        check_document(&document, &path.display().to_string());
        checked += 1;
    }
    assert!(
        checked >= 40,
        "only {checked} corpus documents were checked"
    );
}
