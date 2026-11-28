//! Content controls: inserting them inline, as blocks, as table rows and cells
//! and in headers, typing them, their properties, setting and reading values,
//! custom XML binding, repeating, cloning, moving, unwrapping and removing
//! them, and the controls of the corpus form and thesis.

use std::collections::HashMap;
use std::io::Cursor;

use oxoffice_docx::{
    BuildingBlock, BuildingBlockKind, ContentControlAppearance, ContentControlLock,
    ContentControlPlacement, ContentControlSpec, ContentControlTypePayload, ContentLocation,
    Document, FormFieldAssignment, FormFieldRule, FormFieldTarget, FormFieldValue, FormResetValue,
    HdrFtrType, HeaderFooterKind, LegacyFormFieldValue, Length, StoryId, StoryItemKind, StoryKind,
};
use oxoffice_docx_oxml::content_control::SdtType;
use oxoffice_docx_oxml::document::{BodyContent, CT_Body};
use oxoffice_docx_oxml::text::CT_P;
use oxoffice_opc::OpcPackage;
use regex::Regex;

use crate::support::{corpus_doc, doc_with, doc_with_body, reopen, texts};

use ContentControlPlacement::{Block, Cell, Inline, Row};

/// A one-pixel PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

/// The first story of this kind.
fn story(doc: &Document, kind: StoryKind) -> StoryId {
    doc.stories()
        .expect("stories are listed")
        .into_iter()
        .find(|story| story.kind() == kind)
        .unwrap_or_else(|| panic!("the document has a {kind:?} story"))
}

fn body(doc: &Document) -> StoryId {
    story(doc, StoryKind::Body)
}

/// The location of the first body item of this kind.
fn first_item(doc: &Document, kind: StoryItemKind) -> ContentLocation {
    doc.story_items(&body(doc))
        .expect("body items are listed")
        .into_iter()
        .find(|item| item.kind() == kind)
        .unwrap_or_else(|| panic!("the body has a {kind:?} item"))
        .location()
        .clone()
}

/// A control of this kind holding only a tag.
fn spec(placement: ContentControlPlacement, kind: SdtType, tag: &str) -> ContentControlSpec {
    ContentControlSpec {
        placement,
        kind,
        tag: Some(tag.to_owned()),
        alias: None,
        lock: None,
        appearance: None,
        placeholder: None,
    }
}

/// The XML of the control tagged `tag` in `story`, read from its story item.
/// The tag is matched in the control's own properties, never in a control
/// nested in its content. The location comes with it.
fn control_item(doc: &Document, story: &StoryId, tag: &str) -> (ContentLocation, String) {
    let needle = format!("<w:tag w:val=\"{tag}\"/>");
    doc.story_items(story)
        .expect("story items are listed")
        .into_iter()
        .filter(|item| item.kind() == StoryItemKind::ContentControl)
        .map(|item| {
            let xml = String::from_utf8(item.xml().expect("item XML").into_owned())
                .expect("item XML is UTF-8");
            (item.location().clone(), xml)
        })
        .find(|(_, xml)| {
            let content = xml.find("<w:sdtContent").unwrap_or(xml.len());
            xml.find(&needle).is_some_and(|at| at < content)
        })
        .unwrap_or_else(|| panic!("no control tagged {tag}"))
}

fn control_at(doc: &Document, story: &StoryId, tag: &str) -> ContentLocation {
    control_item(doc, story, tag).0
}

/// The text of the control tagged `tag` in `story`, as its story item reads.
fn item_text(doc: &Document, story: &StoryId, tag: &str) -> String {
    let location = control_at(doc, story, tag);
    doc.story_items(story)
        .expect("story items are listed")
        .into_iter()
        .find(|item| item.location() == &location)
        .expect("the control is an item")
        .text()
        .expect("item text reads")
        .unwrap_or_default()
}

/// Tag, id, kind and text of one listed control.
type Listed = (Option<String>, Option<i32>, Option<SdtType>, String);

/// Tag, id, kind and text of each control `content_controls` lists.
fn listed(doc: &Document) -> Vec<Listed> {
    doc.content_controls()
        .iter()
        .map(|control| {
            (
                control.tag().map(str::to_owned),
                control.id(),
                control.control_type(),
                control.text(),
            )
        })
        .collect()
}

/// The text of the one control tagged `tag` that `content_controls` lists.
fn value(doc: &Document, tag: &str) -> String {
    let controls = doc.content_controls_by_tag(tag);
    assert_eq!(controls.len(), 1, "one control is tagged {tag}");
    controls[0].text()
}

/// The `w:sdtPr` of the control whose id is `id`, as `part` saves now.
fn saved_properties(doc: &Document, part: &str, id: i32) -> String {
    let xml = doc.part_xml(part).expect("part is present");
    let at = xml
        .find(&format!("<w:id w:val=\"{id}\"/>"))
        .unwrap_or_else(|| panic!("no control has id {id} in {part}"));
    let start = xml[..at]
        .rfind("<w:sdtPr")
        .expect("the id sits in a w:sdtPr");
    let end = at + xml[at..].find("</w:sdtPr>").expect("w:sdtPr closes") + "</w:sdtPr>".len();
    xml[start..end].to_owned()
}

/// The value of `attribute` on the first `element` of `xml`.
fn attribute(xml: &str, element: &str, attribute: &str) -> Option<String> {
    Regex::new(&format!(r#"<{element}\b[^>]*\s{attribute}="([^"]*)""#))
        .expect("pattern compiles")
        .captures(xml)
        .map(|captures| captures[1].to_owned())
}

/// The value of the first `element` of `xml`, under any prefix.
fn val(xml: &str, element: &str) -> Option<String> {
    Regex::new(&format!(r#"<{element}\b[^>]*\s\w+:val="([^"]*)""#))
        .expect("pattern compiles")
        .captures(xml)
        .map(|captures| captures[1].to_owned())
}

/// Whether `xml` holds an `element`.
fn has(xml: &str, element: &str) -> bool {
    Regex::new(&format!(r#"<{element}[\s/>]"#))
        .expect("pattern compiles")
        .is_match(xml)
}

/// A new document holding the paragraph "Intro" then one empty block rich
/// text control with tag `tag`, alias "Typed" and id 1.
fn doc_with_control(tag: &str) -> Document {
    let mut doc = doc_with(&["Intro"]);
    let end = ContentLocation::end(body(&doc));
    doc.insert_content_control(
        &end,
        ContentControlSpec {
            alias: Some("Typed".to_owned()),
            ..spec(Block, SdtType::RichText, tag)
        },
    )
    .expect("a block control is inserted");
    doc
}

/// `doc_with_control` with the control typed by `payload`.
fn typed_control(tag: &str, payload: ContentControlTypePayload) -> Document {
    let mut doc = doc_with_control(tag);
    let location = control_at(&doc, &body(&doc), tag);
    doc.set_content_control_type(&location, payload)
        .expect("the shell takes the type");
    doc
}

/// `doc` saved with its whole body wrapped in one rich text control tagged
/// `tag`, id 5, as Word wraps a selection.
fn wrapped_in_control(doc: &mut Document, tag: &str) -> Document {
    let bytes = doc.to_bytes().expect("document saves");
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    let part = package.main_document_part().expect("main document part");
    let xml = String::from_utf8(package.get_part(&part).expect("main part").to_vec())
        .expect("main part is UTF-8");
    let start = xml.find("<w:body>").expect("body start") + "<w:body>".len();
    let end = xml.find("<w:sectPr").expect("section properties");
    let xml = format!(
        r#"{}<w:sdt><w:sdtPr><w:tag w:val="{tag}"/><w:id w:val="5"/><w:richText/></w:sdtPr><w:sdtContent>{}</w:sdtContent></w:sdt>{}"#,
        &xml[..start],
        &xml[start..end],
        &xml[end..]
    );
    package.set_part(&part, xml.into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).expect("package writes");
    Document::from_bytes(&out.into_inner()).expect("wrapped document opens")
}

fn target(tag: &str) -> FormFieldTarget {
    FormFieldTarget::ContentControlTag(tag.to_owned())
}

fn legacy(ordinal: usize) -> FormFieldTarget {
    FormFieldTarget::Legacy {
        source_part: "/word/document.xml".to_owned(),
        ordinal,
    }
}

fn assign(target: FormFieldTarget, value: FormFieldValue) -> FormFieldAssignment {
    FormFieldAssignment { target, value }
}

fn text(value: &str) -> FormFieldValue {
    FormFieldValue::Text(value.to_owned())
}

fn prompt(target: FormFieldTarget, required: bool, text: &str) -> FormFieldRule {
    FormFieldRule {
        target,
        required,
        reset: FormResetValue::Placeholder {
            text: text.to_owned(),
        },
    }
}

/// Set the value of the control tagged `tag` with the tag setter, or with a
/// form fill when `by_form`.
fn set_value(doc: &mut Document, tag: &str, value: &str, by_form: bool) {
    if by_form {
        let rule = prompt(target(tag), false, "");
        assert_eq!(
            doc.fill_form(&[rule], &[assign(target(tag), text(value))])
                .expect("the form fill writes the value"),
            [1]
        );
    } else {
        assert_eq!(
            doc.set_content_control_value_by_tag(tag, value)
                .expect("the tag setter writes the value"),
            1
        );
    }
}

/// The two ways to set a value, for the messages of the tests that try both.
fn way(by_form: bool) -> &'static str {
    if by_form { "fill_form" } else { "tag setter" }
}

/// The `w:sdtContent` of the control tagged `tag`, as `part` saves now.
fn saved_content(doc: &Document, part: &str, tag: &str) -> String {
    let xml = doc.part_xml(part).expect("part is present");
    let at = xml
        .find(&format!("<w:tag w:val=\"{tag}\"/>"))
        .unwrap_or_else(|| panic!("no control has tag {tag} in {part}"));
    let start = at + xml[at..].find("<w:sdtContent").expect("its content starts");
    let end = at + xml[at..].find("</w:sdtContent>").expect("its content ends");
    xml[start..end].to_owned()
}

/// The number of `w:p` in `xml`.
fn paragraph_count(xml: &str) -> usize {
    Regex::new(r"<w:p[\s>/]").unwrap().find_iter(xml).count()
}

/// A document whose header holds an empty plain text control tagged "title".
fn doc_with_header_control() -> Document {
    let mut doc = doc_with(&["Body"]);
    let header = doc
        .create_section_story(0, HeaderFooterKind::Header, HdrFtrType::Default)
        .expect("a header story is created");
    doc.insert_content_control(
        &ContentLocation::end(header),
        spec(Block, SdtType::PlainText, "title"),
    )
    .expect("a block control is inserted in the header");
    doc
}

const WORD_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const VML_NS: &str = "urn:schemas-microsoft-com:vml";

/// A block control tagged `tag` holding one paragraph of `text`, as XML.
fn block_control_xml(tag: &str, text: &str) -> String {
    format!(
        r#"<w:sdt><w:sdtPr><w:tag w:val="{tag}"/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:sdtContent></w:sdt>"#
    )
}

/// A paragraph holding a VML text box whose content is `content`, as XML.
fn text_box_xml(content: &str) -> String {
    format!(
        r#"<w:p><w:r><w:pict xmlns:v="{VML_NS}"><v:shape id="box" style="width:120pt;height:40pt"><v:textbox><w:txbxContent>{content}</w:txbxContent></v:textbox></v:shape></w:pict></w:r></w:p>"#
    )
}

/// A one-paragraph body and a default header of `content`, written raw.
fn doc_with_raw_header(content: &str) -> Document {
    let mut doc = doc_with(&["Body"]);
    let header = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:hdr xmlns:w="{WORD_NS}">{content}</w:hdr>"#
    );
    doc.set_raw_header_with_images(header.into_bytes(), &[], HdrFtrType::Default);
    doc
}

// ---- Inserting ----

#[test]
fn row_and_cell_controls_add_a_row_as_wide_as_the_table_and_a_cell_to_a_row() {
    let mut doc = doc_with(&["Order"]);
    {
        let mut table = doc.add_table(1, 3);
        for (column, heading) in ["Item", "Qty", "Price"].into_iter().enumerate() {
            table.cell(0, column).unwrap().set_text(heading);
        }
    }
    let table = first_item(&doc, StoryItemKind::Table);
    doc.insert_content_control(&table, spec(Row, SdtType::RichText, "line"))
        .unwrap();
    // A row is addressed by its index under the table.
    let table = first_item(&doc, StoryItemKind::Table);
    let first_row = ContentLocation::new(
        table.story().clone(),
        StoryItemKind::Table,
        [table.index_path(), &[0]].concat(),
    );
    doc.insert_content_control(&first_row, spec(Cell, SdtType::PlainText, "note"))
        .unwrap();
    assert_eq!(
        doc.fill_content_control_tags(&[("line", "Widget"), ("note", "per unit")])
            .unwrap(),
        [1, 1]
    );
    let doc = reopen(&mut doc);

    // The cell control closes the first row, the row control the table.
    assert_eq!(
        listed(&doc),
        [
            (
                Some("note".into()),
                Some(2),
                Some(SdtType::PlainText),
                "per unit".into()
            ),
            (
                Some("line".into()),
                Some(1),
                Some(SdtType::RichText),
                "Widget".into()
            ),
        ]
    );
    let row = control_item(&doc, &body(&doc), "line").1;
    assert_eq!(row.matches("<w:tr>").count(), 1, "{row}");
    assert_eq!(
        row.matches("<w:tc>").count(),
        3,
        "one cell per column: {row}"
    );
    let cell = control_item(&doc, &body(&doc), "note").1;
    assert_eq!(cell.matches("<w:tc>").count(), 1, "{cell}");
    // The text puts a tab after each cell.
    let lines: Vec<String> = doc.text().lines().map(str::to_owned).collect();
    assert_eq!(
        lines,
        ["Order", "Item\tQty\tPrice\tper unit\t", "Widget\t\t\t"]
    );
}

#[test]
fn a_row_inside_a_row_control_counts_among_the_table_rows() {
    let mut doc = doc_with(&["Order"]);
    doc.add_table(1, 2);
    let table = first_item(&doc, StoryItemKind::Table);
    doc.insert_content_control(&table, spec(Row, SdtType::RichText, "line"))
        .unwrap();
    doc.set_content_control_value_by_tag("line", "Widget")
        .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(value(&doc, "line"), "Widget");
    assert_eq!(
        doc.table(0).unwrap().row_count(),
        2,
        "the row inside the control is a row of the table"
    );
}

/// A table of four rows: A, then a row control holding B and a nested row
/// control holding C, then D. Each cell reads its row's letter and column.
fn doc_with_rows_in_row_controls() -> Document {
    let row = |name: &str| {
        format!(
            r#"<w:tr><w:tc><w:p><w:r><w:t>{name}1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>{name}2</w:t></w:r></w:p></w:tc></w:tr>"#
        )
    };
    let nested = format!(
        r#"<w:sdt><w:sdtPr><w:tag w:val="inner"/></w:sdtPr><w:sdtContent>{}</w:sdtContent></w:sdt>"#,
        row("C")
    );
    doc_with_body(&format!(
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="2000"/><w:gridCol w:w="2000"/></w:tblGrid>{}<w:sdt><w:sdtPr><w:tag w:val="lines"/></w:sdtPr><w:sdtContent>{}{nested}</w:sdtContent></w:sdt>{}</w:tbl><w:p/>"#,
        row("A"),
        row("B"),
        row("D")
    ))
}

/// A row of two cells, its first cell's vertical merge `merge` when given.
fn merge_row(name: &str, merge: Option<&str>) -> String {
    let properties = merge.map_or_else(String::new, |merge| {
        format!(r#"<w:tcPr><w:vMerge w:val="{merge}"/></w:tcPr>"#)
    });
    format!(
        r#"<w:tr><w:tc>{properties}<w:p><w:r><w:t>{name}1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>{name}2</w:t></w:r></w:p></w:tc></w:tr>"#
    )
}

/// A two-column table of `content`, alone in the body or in a raw header.
fn table_xml(content: &str) -> String {
    format!(
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="2000"/><w:gridCol w:w="2000"/></w:tblGrid>{content}</w:tbl><w:p/>"#
    )
}

fn control_xml(tag: &str, content: &str) -> String {
    format!(
        r#"<w:sdt><w:sdtPr><w:tag w:val="{tag}"/></w:sdtPr><w:sdtContent>{content}</w:sdtContent></w:sdt>"#
    )
}

/// Rows A, B in a row control after a column bookmark start, D, then E in a
/// row control after the bookmark's end: table children beside the controls.
fn bookmarked_rows() -> String {
    table_xml(&format!(
        r#"{}<w:bookmarkStart w:id="7" w:name="column" w:colFirst="0" w:colLast="1"/>{}{}<w:bookmarkEnd w:id="7"/>{}"#,
        merge_row("A", None),
        control_xml("lines", &merge_row("B", None)),
        merge_row("D", None),
        control_xml("tail", &merge_row("E", None)),
    ))
}

/// The first column's text of each row of the first table of `story`.
fn story_first_column(doc: &Document, story: &StoryId) -> Vec<String> {
    let table = doc
        .story_items(story)
        .unwrap()
        .into_iter()
        .find(|item| item.kind() == StoryItemKind::Table)
        .unwrap()
        .location()
        .clone();
    doc.read_story_table(&table, |table| {
        (0..table.row_count())
            .map(|row| table.cell(row, 0).unwrap().text())
            .collect::<Vec<_>>()
    })
    .unwrap()
}

#[test]
fn a_copied_row_keeps_the_row_controls_and_the_table_children_beside_them() {
    let mut doc = doc_with_body(&bookmarked_rows());
    assert_eq!(first_column(&doc), ["A1", "B1", "D1", "E1"]);
    // Before D, after the control the bookmark start comes before.
    assert_eq!(doc.clone_table_row(0, 0, 2).unwrap(), 2);
    // After the last row, after the control the bookmark end comes before.
    assert_eq!(doc.clone_table_row(0, 0, 5).unwrap(), 5);
    // Before B: the bookmark start stays before the copy.
    assert_eq!(doc.clone_table_row(0, 2, 1).unwrap(), 1);
    let doc = reopen(&mut doc);
    assert_eq!(
        first_column(&doc),
        ["A1", "A1", "B1", "A1", "D1", "E1", "A1"]
    );
    let xml = doc.part_xml("word/document.xml").unwrap();
    let order: Vec<usize> = [
        ">A1<",
        "bookmarkStart",
        ">B1<",
        ">D1<",
        "bookmarkEnd",
        ">E1<",
    ]
    .iter()
    .map(|needle| {
        xml.find(needle)
            .unwrap_or_else(|| panic!("{needle} is kept"))
    })
    .collect();
    assert!(order.is_sorted(), "{order:?}: {xml}");
    assert_eq!(xml.matches("bookmarkStart").count(), 1);

    let mut doc = doc_with_raw_header(&bookmarked_rows());
    let header = story(&doc, StoryKind::Header);
    let table = doc
        .story_items(&header)
        .unwrap()
        .into_iter()
        .find(|item| item.kind() == StoryItemKind::Table)
        .unwrap()
        .location()
        .clone();
    assert_eq!(doc.clone_story_table_row(&table, 0, 2).unwrap(), 2);
    let header = story(&doc, StoryKind::Header);
    let table = doc
        .story_items(&header)
        .unwrap()
        .into_iter()
        .find(|item| item.kind() == StoryItemKind::Table)
        .unwrap()
        .location()
        .clone();
    assert_eq!(doc.clone_story_table_row(&table, 0, 5).unwrap(), 5);
    let doc = reopen(&mut doc);
    let header = story(&doc, StoryKind::Header);
    assert_eq!(
        story_first_column(&doc, &header),
        ["A1", "B1", "A1", "D1", "E1", "A1"]
    );
}

#[test]
fn a_cell_of_a_cell_control_is_reported_once_in_the_web_page_losses() {
    let inline = r#"<w:p><w:sdt><w:sdtPr><w:tag w:val="name"/></w:sdtPr><w:sdtContent><w:r><w:t>Ada</w:t></w:r></w:sdtContent></w:sdt></w:p>"#;
    let doc = doc_with_body(&format!(
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="2000"/><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p/></w:tc>{}</w:tr></w:tbl><w:p/>"#,
        control_xml("cells", &format!("<w:tc>{inline}</w:tc>"))
    ));
    let losses = doc.to_mhtml_bytes().unwrap().diagnostics;
    let flattened: Vec<&str> = losses
        .iter()
        .filter(|loss| loss.message == "flattened Word paragraph content control")
        .map(|loss| loss.location.as_str())
        .collect();
    assert_eq!(flattened.len(), 1, "{flattened:?}");
}

#[test]
fn the_mutable_table_counts_and_edits_the_rows_of_row_controls_as_the_reader_does() {
    let mut doc = doc_with_rows_in_row_controls();
    {
        let mut table = doc.table_mut(0).unwrap();
        assert_eq!(table.row_count(), 4);
        table.cell(2, 0).unwrap().set_text("Changed");
        table
            .set_cell_vertical_merge(1, 1, Some(oxoffice_docx::VMerge::Restart))
            .unwrap();
        table.cell(2, 1).unwrap().set_text("");
        table.set_cell_grid_span_checked(2, 0, Some(2)).unwrap();
        table.cell(3, 1).unwrap().set_text("");
        table.set_row_grid_omissions(3, None, Some(1)).unwrap();
    }
    let doc = reopen(&mut doc);
    let table = doc.table(0).unwrap();
    assert_eq!(table.cell(2, 0).unwrap().text(), "Changed");
    assert_eq!(table.cell(1, 1).unwrap().text(), "B2");
    assert_eq!(
        table.cell(1, 1).unwrap().v_merge(),
        Some(&oxoffice_docx::VMerge::Restart)
    );
    assert_eq!(
        table.cell(2, 0).unwrap().grid_span(),
        Some(2),
        "C1 spans row C"
    );
    assert_eq!(table.row(2).unwrap().cell_count(), 1);
    let last = table.row(3).unwrap();
    assert_eq!(
        (last.cell_count(), last.grid_after()),
        (1, Some(1)),
        "D2 is omitted"
    );
    assert_eq!(
        table.cell(1, 1).unwrap().grid_span(),
        None,
        "row B keeps its cells"
    );
    assert_eq!(
        texts(&doc).last().map(String::as_str),
        Some(""),
        "the body is left alone"
    );
}

/// The first column's text of each row of the first table.
fn first_column(doc: &Document) -> Vec<String> {
    let table = doc.table(0).unwrap();
    (0..table.row_count())
        .map(|row| table.cell(row, 0).unwrap().text())
        .collect()
}

#[test]
fn the_physical_cell_routes_count_rows_of_row_controls_as_the_table_does() {
    let mut doc = doc_with_rows_in_row_controls();
    let table = first_item(&doc, StoryItemKind::Table);
    let xml = |bytes: Vec<u8>| String::from_utf8(bytes).unwrap();
    assert!(xml(doc.cell_element_xml(&table, 2, 1).unwrap()).contains("C2"));
    assert!(xml(doc.cell_element_xml(&table, 3, 0).unwrap()).contains("D1"));
    assert!(xml(doc.direct_body_cell_xml(0, 2, 0).unwrap().unwrap()).contains("C1"));
    let paragraph = doc.cell_paragraph_story_location(0, 1, 1, 0).unwrap();
    assert_eq!(
        doc.read_story_paragraph(&paragraph, |paragraph| paragraph.text())
            .unwrap(),
        "B2"
    );
    let inventory = doc.cell_paragraph_story_locations().unwrap();
    assert!(inventory.contains_key(&(0, 2, 1)) && inventory.contains_key(&(0, 3, 1)));
    let cells = doc.story_owner_snapshots().unwrap().cells;
    let mut coordinates: Vec<(usize, usize)> = cells
        .iter()
        .map(|(_, row, cell, _)| (*row, *cell))
        .collect();
    coordinates.sort_unstable();
    assert_eq!(
        coordinates,
        [
            (0, 0),
            (0, 1),
            (1, 0),
            (1, 1),
            (2, 0),
            (2, 1),
            (3, 0),
            (3, 1)
        ]
    );

    assert_eq!(
        doc.try_replace_text_in_cell((0, 2, 1), None, "C2", "Third", None)
            .unwrap(),
        Ok(1)
    );
    let doc = reopen(&mut doc);
    let table = doc.table(0).unwrap();
    assert_eq!(table.cell(2, 1).unwrap().text(), "Third");
    assert_eq!(table.cell(3, 1).unwrap().text(), "D2");
}

#[test]
fn the_local_cell_route_leaves_a_table_holding_row_controls_to_the_checked_one() {
    let mut doc = doc_with_rows_in_row_controls();
    let plain = doc_with_body(
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl><w:p/>"#,
    );
    let edit = |cell: &mut oxoffice_docx::Cell<'_>| cell.try_set_text("x");
    assert!(
        plain
            .direct_body_table_cell_candidate(0, 0, 0, edit)
            .unwrap()
            .is_some(),
        "a plain table takes the local route"
    );
    assert!(
        doc.direct_body_table_cell_candidate(0, 1, 1, edit)
            .unwrap()
            .is_none(),
        "its own row 1 is D, row 1 is B"
    );
    doc.try_set_cell_text(0, 1, 1, "Bee").unwrap();
    assert_eq!(doc.table(0).unwrap().cell(1, 1).unwrap().text(), "Bee");
    assert_eq!(doc.table(0).unwrap().cell(3, 1).unwrap().text(), "D2");
}

#[test]
fn rows_are_copied_and_removed_by_the_rows_the_table_counts() {
    let mut doc = doc_with_rows_in_row_controls();
    // Before the row control: the copy comes before its rows.
    assert_eq!(doc.clone_table_row(0, 0, 1).unwrap(), 1);
    assert_eq!(first_column(&doc), ["A1", "A1", "B1", "C1", "D1"]);
    // Before row D: the copy comes after the control's rows.
    assert_eq!(doc.clone_table_row(0, 4, 4).unwrap(), 4);
    assert_eq!(first_column(&doc), ["A1", "A1", "B1", "C1", "D1", "D1"]);
    // After the last row.
    assert_eq!(doc.clone_table_row(0, 0, 6).unwrap(), 6);
    assert_eq!(doc.table(0).unwrap().row_count(), 7);

    let before = doc.to_bytes().unwrap();
    assert!(
        doc.clone_table_row(0, 2, 0).is_err(),
        "row B sits in a control"
    );
    assert!(
        doc.clone_table_row(0, 0, 3).is_err(),
        "row C is inside a control"
    );
    assert!(
        doc.remove_table_row(0, 3).is_err(),
        "row C sits in a control"
    );
    assert_eq!(doc.to_bytes().unwrap(), before);

    assert!(doc.remove_table_row(0, 1).unwrap());
    assert!(doc.remove_table_row(0, 4).unwrap());
    let doc = reopen(&mut doc);
    assert_eq!(first_column(&doc), ["A1", "B1", "C1", "D1", "A1"]);
}

/// A table of one row: A, then a cell control holding X and a nested cell
/// control holding Y, then D.
fn doc_with_cells_in_cell_controls() -> Document {
    let cell = |text: &str| format!(r#"<w:tc><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:tc>"#);
    let control = |tag: &str, content: String| {
        format!(
            r#"<w:sdt><w:sdtPr><w:tag w:val="{tag}"/></w:sdtPr><w:sdtContent>{content}</w:sdtContent></w:sdt>"#
        )
    };
    let nested = control("inner", cell("Y"));
    doc_with_body(&format!(
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="1000"/><w:gridCol w:w="1000"/><w:gridCol w:w="1000"/><w:gridCol w:w="1000"/></w:tblGrid><w:tr>{}{}{}</w:tr><w:tr>{}{}{}{}</w:tr></w:tbl><w:p/>"#,
        cell("A"),
        control("cells", format!("{}{nested}", cell("X"))),
        cell("D"),
        cell("E"),
        cell("F"),
        cell("G"),
        cell("H")
    ))
}

#[test]
fn every_cell_counted_is_addressed_among_the_cells_of_cell_controls() {
    let mut doc = doc_with_cells_in_cell_controls();
    {
        let table = doc.table(0).unwrap();
        let row = table.row(0).unwrap();
        assert_eq!(row.cell_count(), 4);
        let texts: Vec<String> = (0..4).map(|cell| row.cell(cell).unwrap().text()).collect();
        assert_eq!(texts, ["A", "X", "Y", "D"]);
        assert!(row.cell(4).is_none());
        assert_eq!(table.cell(0, 2).unwrap().text(), "Y");
        assert_eq!(table.cell(1, 2).unwrap().text(), "G");
        let cells = doc.story_owner_snapshots().unwrap().cells;
        assert!(
            cells
                .iter()
                .any(|(_, row, cell, _)| (*row, *cell) == (0, 3))
        );
        let table_location = first_item(&doc, StoryItemKind::Table);
        let xml = String::from_utf8(doc.cell_element_xml(&table_location, 0, 2).unwrap()).unwrap();
        assert!(xml.contains(">Y<"), "{xml}");
    }
    {
        let mut table = doc.table_mut(0).unwrap();
        assert_eq!(table.row(0).unwrap().cell_count(), 4);
        table.cell(0, 1).unwrap().set_text("Changed");
        table.row(0).unwrap().cell(2).unwrap().set_text("Nested");
        table
            .set_cell_vertical_merge(0, 3, Some(oxoffice_docx::VMerge::Restart))
            .unwrap();
        assert!(
            table.set_cell_grid_span_checked(0, 0, Some(2)).is_err(),
            "the grid of a row whose cells sit in controls is refused"
        );
    }
    let doc = reopen(&mut doc);
    let table = doc.table(0).unwrap();
    assert_eq!(table.cell(0, 1).unwrap().text(), "Changed");
    assert_eq!(table.cell(0, 2).unwrap().text(), "Nested");
    assert_eq!(table.cell(0, 0).unwrap().text(), "A");
    assert_eq!(
        table.cell(0, 3).unwrap().v_merge(),
        Some(&oxoffice_docx::VMerge::Restart),
        "the merge is D's"
    );
    assert_eq!(table.cell(1, 3).unwrap().text(), "H");
}

#[test]
fn a_control_in_a_header_survives_a_save_and_the_form_calls_fill_and_reset_it() {
    let mut doc = doc_with_header_control();
    let rule = prompt(target("title"), true, "Document title");
    assert_eq!(
        doc.fill_form(
            std::slice::from_ref(&rule),
            &[assign(target("title"), text("Annual report"))]
        )
        .unwrap(),
        [1]
    );
    let mut doc = reopen(&mut doc);
    let header = story(&doc, StoryKind::Header);
    assert_eq!(item_text(&doc, &header, "title"), "Annual report");
    assert!(
        doc.validate_form(std::slice::from_ref(&rule))
            .unwrap()
            .is_empty()
    );

    assert_eq!(doc.reset_form(std::slice::from_ref(&rule)).unwrap(), [1]);
    let doc = reopen(&mut doc);
    let header = story(&doc, StoryKind::Header);
    assert_eq!(item_text(&doc, &header, "title"), "Document title");
    let (_, xml) = control_item(&doc, &header, "title");
    assert!(has(&xml, "w:showingPlcHdr"), "the prompt shows: {xml}");
    let issues = doc.validate_form(&[rule]).unwrap();
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].target, target("title"));
    assert!(
        issues[0].source_part.starts_with("/word/header"),
        "{issues:?}"
    );
    assert_eq!(texts(&doc), ["Body"], "the body is left alone");
}

#[test]
fn a_value_set_by_tag_reaches_controls_in_text_boxes_of_the_body_and_of_a_header() {
    let mut doc = doc_with_body(&text_box_xml(&block_control_xml("boxed", "Old")));
    assert_eq!(listed(&doc).len(), 1, "the text box control is listed once");
    assert_eq!(
        doc.set_content_control_value_by_tag("boxed", "New")
            .unwrap(),
        1
    );
    let doc = reopen(&mut doc);
    assert_eq!(value(&doc, "boxed"), "New");
    let text_box = story(&doc, StoryKind::TextBox);
    assert_eq!(item_text(&doc, &text_box, "boxed"), "New");

    let mut doc = doc_with_raw_header(&text_box_xml(&block_control_xml("boxed", "Old")));
    assert_eq!(
        doc.set_content_control_value_by_tag("boxed", "New")
            .unwrap(),
        1
    );
    let doc = reopen(&mut doc);
    assert_eq!(value(&doc, "boxed"), "New");
    let header_box = doc
        .stories()
        .unwrap()
        .into_iter()
        .find(|story| story.kind() == StoryKind::TextBox && story.part_name().contains("header"))
        .expect("the header's text box is a story");
    assert_eq!(item_text(&doc, &header_box, "boxed"), "New");
}

#[test]
fn nested_controls_that_both_take_a_value_are_refused_in_every_story() {
    let nested = r#"<w:sdt><w:sdtPr><w:tag w:val="same"/></w:sdtPr><w:sdtContent><w:p><w:sdt><w:sdtPr><w:tag w:val="same"/></w:sdtPr><w:sdtContent><w:r><w:t>inner</w:t></w:r></w:sdtContent></w:sdt></w:p></w:sdtContent></w:sdt>"#;
    for mut doc in [doc_with_raw_header(nested), doc_with_body(nested)] {
        assert_eq!(
            doc.content_controls_by_tag("same").len(),
            2,
            "the outer control, then the inner one"
        );
        let before = doc.to_bytes().unwrap();
        let error = doc
            .set_content_control_value_by_tag("same", "x")
            .unwrap_err();
        assert!(error.to_string().contains("nested"), "{error}");
        assert_eq!(doc.to_bytes().unwrap(), before);
    }
    // A control in a cell of a row control.
    let mut doc = doc_with_body(&table_xml(&control_xml(
        "same",
        r#"<w:tr><w:tc><w:p><w:sdt><w:sdtPr><w:tag w:val="same"/></w:sdtPr><w:sdtContent><w:r><w:t>inner</w:t></w:r></w:sdtContent></w:sdt></w:p></w:tc><w:tc><w:p/></w:tc></w:tr>"#,
    )));
    assert_eq!(doc.content_controls_by_tag("same").len(), 2);
    let before = doc.to_bytes().unwrap();
    let error = doc
        .set_content_control_value_by_tag("same", "x")
        .unwrap_err();
    assert!(error.to_string().contains("nested"), "{error}");
    assert_eq!(doc.to_bytes().unwrap(), before);
    // Either one alone takes its value.
    let mut doc = doc_with_body(&nested.replacen("same", "outer", 1));
    assert_eq!(
        doc.set_content_control_value_by_tag("same", "x").unwrap(),
        1
    );
}

#[test]
fn a_placement_or_a_kind_that_does_not_fit_is_refused_and_changes_nothing() {
    let mut doc = doc_with(&["Paragraph"]);
    doc.add_table(1, 1);
    let paragraph = first_item(&doc, StoryItemKind::Paragraph);
    let table = first_item(&doc, StoryItemKind::Table);
    let end = ContentLocation::end(body(&doc));
    let before = doc.to_bytes().unwrap();

    for (location, placement) in [
        (&end, Inline),
        (&table, Inline),
        (&paragraph, Row),
        (&table, Cell),
    ] {
        let result = doc.insert_content_control(location, spec(placement, SdtType::RichText, "x"));
        assert!(result.is_err(), "{placement:?}: {result:?}");
    }
    // A new control is a rich or plain text shell; set_content_control_type
    // gives it any other kind.
    for kind in [
        SdtType::Date,
        SdtType::CheckBox,
        SdtType::ComboBox,
        SdtType::DropDownList,
        SdtType::Picture,
        SdtType::Group,
        SdtType::RepeatingSection,
    ] {
        let result = doc.insert_content_control(&end, spec(Block, kind, "x"));
        assert!(result.is_err(), "{kind:?}: {result:?}");
    }
    assert_eq!(doc.to_bytes().unwrap(), before);
    assert!(doc.content_controls().is_empty());
}

// ---- Properties ----

#[test]
fn tag_alias_lock_appearance_and_placeholder_are_set_and_cleared_without_touching_the_content() {
    let mut doc = doc_with_body(
        r#"<w:sdt><w:sdtPr><w:tag w:val="terms"/><w:id w:val="9"/><w:richText/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>First clause.</w:t></w:r></w:p><w:p><w:r><w:rPr><w:b/></w:rPr><w:t>Second clause.</w:t></w:r></w:p></w:sdtContent></w:sdt>"#,
    );
    let content = |doc: &Document| {
        let xml = doc.part_xml("word/document.xml").unwrap();
        let start = xml.find("<w:sdtContent>").expect("content start");
        let end = xml.find("</w:sdtContent>").expect("content end");
        xml[start..end].to_owned()
    };
    let original = content(&doc);
    let at = |doc: &Document, tag: &str| control_at(doc, &body(doc), tag);

    doc.set_content_control_tag(&at(&doc, "terms"), Some("conditions"))
        .unwrap();
    let location = at(&doc, "conditions");
    doc.set_content_control_alias(&location, Some("Conditions générales"))
        .unwrap();
    let location = at(&doc, "conditions");
    doc.set_content_control_lock(&location, Some(ContentControlLock::SdtContentLocked))
        .unwrap();
    let location = at(&doc, "conditions");
    doc.set_content_control_appearance(&location, Some(ContentControlAppearance::Hidden))
        .unwrap();
    let location = at(&doc, "conditions");
    doc.set_content_control_placeholder(&location, Some(true))
        .unwrap();
    let mut doc = reopen(&mut doc);

    let control = &doc.content_controls()[0];
    assert_eq!(
        (control.tag(), control.alias(), control.id()),
        (Some("conditions"), Some("Conditions générales"), Some(9))
    );
    let properties = saved_properties(&doc, "word/document.xml", 9);
    assert_eq!(
        val(&properties, "w:lock").as_deref(),
        Some("sdtContentLocked")
    );
    assert_eq!(
        val(&properties, "w15:appearance").as_deref(),
        Some("hidden")
    );
    assert!(has(&properties, "w:showingPlcHdr"), "{properties}");
    assert_eq!(content(&doc), original, "the content is not rewritten");

    for clear in 0..4 {
        let location = at(&doc, "conditions");
        match clear {
            0 => doc.set_content_control_alias(&location, None),
            1 => doc.set_content_control_lock(&location, None),
            2 => doc.set_content_control_appearance(&location, None),
            _ => doc.set_content_control_placeholder(&location, None),
        }
        .unwrap();
    }
    let mut doc = reopen(&mut doc);
    let properties = saved_properties(&doc, "word/document.xml", 9);
    for element in ["w:alias", "w:lock", "w15:appearance", "w:showingPlcHdr"] {
        assert!(!has(&properties, element), "{element}: {properties}");
    }
    let location = at(&doc, "conditions");
    doc.set_content_control_tag(&location, None).unwrap();
    let doc = reopen(&mut doc);
    let control = &doc.content_controls()[0];
    assert_eq!((control.tag(), control.id()), (None, Some(9)));
    assert_eq!(texts(&doc), ["First clause.", "Second clause."]);
    assert_eq!(content(&doc), original);
}

#[test]
fn properties_the_facade_does_not_model_survive_its_edits() {
    // Word's run formatting of the control, its temporary flag, its colour and
    // the multi-line flag of a plain text control.
    let mut doc = doc_with_body(
        r#"<w:p><w:r><w:t xml:space="preserve">Ship to: </w:t></w:r><w:sdt><w:sdtPr><w:rPr><w:i/></w:rPr><w:alias w:val="Ship to"/><w:tag w:val="ship"/><w:id w:val="12"/><w:temporary/><w15:color xmlns:w15="http://schemas.microsoft.com/office/word/2012/wordml" w:val="FF0000"/><w:text w:multiLine="1"/></w:sdtPr><w:sdtContent><w:r><w:t>Depot</w:t></w:r></w:sdtContent></w:sdt></w:p>"#,
    );
    let location = control_at(&doc, &body(&doc), "ship");
    doc.set_content_control_alias(&location, Some("Delivery address"))
        .unwrap();
    let location = control_at(&doc, &body(&doc), "ship");
    doc.set_content_control_lock(&location, Some(ContentControlLock::SdtLocked))
        .unwrap();
    let doc = reopen(&mut doc);

    let properties = saved_properties(&doc, "word/document.xml", 12);
    assert_eq!(
        val(&properties, "w:alias").as_deref(),
        Some("Delivery address")
    );
    assert_eq!(val(&properties, "w:lock").as_deref(), Some("sdtLocked"));
    assert!(has(&properties, "w:temporary"), "{properties}");
    assert_eq!(val(&properties, "w15:color").as_deref(), Some("FF0000"));
    assert_eq!(
        attribute(&properties, "w:text", "w:multiLine").as_deref(),
        Some("1")
    );
    let run_properties = &properties[properties.find("<w:rPr>").expect("run properties")..];
    assert!(
        run_properties.starts_with("<w:rPr><w:i/></w:rPr>"),
        "{properties}"
    );
    assert_eq!(texts(&doc), ["Ship to: Depot"]);
}

// ---- Values ----

/// A paragraph holding a plain text control as Word writes one that shows
/// its placeholder text: the prompt in the Placeholder Text style, the run
/// formatting for the value in the control's own `w:rPr`.
const PROMPTING: &str = r#"<w:p><w:r><w:t xml:space="preserve">Name: </w:t></w:r><w:sdt><w:sdtPr><w:rPr><w:b/></w:rPr><w:alias w:val="Name"/><w:tag w:val="name"/><w:id w:val="42"/><w:showingPlcHdr/><w:text w:multiLine="1"/></w:sdtPr><w:sdtContent><w:r><w:rPr><w:rStyle w:val="PlaceholderText"/></w:rPr><w:t>Type your name</w:t></w:r></w:sdtContent></w:sdt></w:p>"#;

/// Text, style id, bold and italic of one run.
type RunFormatting = (String, Option<String>, Option<bool>, Option<bool>);

/// The formatting of each run with text of the first paragraph, those of its
/// inline controls included.
fn run_formatting(doc: &Document) -> Vec<RunFormatting> {
    doc.paragraph(0)
        .unwrap()
        .runs()
        .map(|run| {
            (
                run.text(),
                run.style_id().map(str::to_owned),
                run.bold_value(),
                run.italic_value(),
            )
        })
        .filter(|(text, ..)| !text.is_empty())
        .collect()
}

#[test]
fn a_value_set_on_a_prompt_drops_the_placeholder_text_style() {
    for by_form in [false, true] {
        let mut doc = doc_with_body(PROMPTING);
        set_value(&mut doc, "name", "Ada Lovelace", by_form);
        let doc = reopen(&mut doc);
        assert_eq!(texts(&doc), ["Name: Ada Lovelace"], "{}", way(by_form));
        let styles: Vec<(String, Option<String>)> = run_formatting(&doc)
            .into_iter()
            .map(|(text, style, ..)| (text, style))
            .collect();
        assert_eq!(
            styles,
            [
                ("Name: ".to_owned(), None),
                ("Ada Lovelace".to_owned(), None)
            ],
            "{}: the value is not styled as placeholder text",
            way(by_form)
        );
    }
}

#[test]
fn a_value_set_on_a_prompt_takes_the_run_properties_of_the_control() {
    for by_form in [false, true] {
        let mut doc = doc_with_body(PROMPTING);
        set_value(&mut doc, "name", "Ada Lovelace", by_form);
        let doc = reopen(&mut doc);
        assert_eq!(
            run_formatting(&doc),
            [
                ("Name: ".to_owned(), None, None, None),
                ("Ada Lovelace".to_owned(), None, Some(true), None)
            ],
            "{}: the value is bold, as the control's w:rPr says",
            way(by_form)
        );
    }
    // Run properties under a prefix the control declares: the value's run,
    // outside the control's properties, needs the declaration too.
    let prefixed = PROMPTING.replace(
        "<w:sdtPr><w:rPr><w:b/></w:rPr>",
        r#"<w:sdtPr xmlns:x="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><x:rPr><x:b/></x:rPr>"#,
    );
    for by_form in [false, true] {
        let mut doc = doc_with_body(&prefixed);
        set_value(&mut doc, "name", "Ada Lovelace", by_form);
        let doc = reopen(&mut doc);
        assert_eq!(
            run_formatting(&doc)[1],
            ("Ada Lovelace".to_owned(), None, Some(true), None),
            "{}",
            way(by_form)
        );
    }
    // A checkbox showing its placeholder: the glyph takes the control's run
    // properties, then the font of its state.
    let checkbox = r#"<w:p><w:sdt><w:sdtPr><w:rPr><w:b/></w:rPr><w:tag w:val="agree"/><w:id w:val="43"/><w:showingPlcHdr/><w14:checkbox xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml"><w14:checked w14:val="0"/><w14:checkedState w14:val="2612" w14:font="MS Gothic"/><w14:uncheckedState w14:val="2610" w14:font="MS Gothic"/></w14:checkbox></w:sdtPr><w:sdtContent><w:r><w:rPr><w:rStyle w:val="PlaceholderText"/></w:rPr><w:t>☐</w:t></w:r></w:sdtContent></w:sdt></w:p>"#;
    for by_form in [false, true] {
        let mut doc = doc_with_body(checkbox);
        if by_form {
            let rule = FormFieldRule {
                target: target("agree"),
                required: false,
                reset: FormResetValue::Value(FormFieldValue::Checked(false)),
            };
            doc.fill_form(
                &[rule],
                &[assign(target("agree"), FormFieldValue::Checked(true))],
            )
            .expect("the form fill ticks the box");
        } else {
            doc.set_content_control_value_by_tag("agree", "true")
                .expect("the tag setter ticks the box");
        }
        let doc = reopen(&mut doc);
        assert_eq!(
            run_formatting(&doc),
            [("\u{2612}".to_owned(), None, Some(true), None)],
            "{}",
            way(by_form)
        );
        let content = saved_content(&doc, "word/document.xml", "agree");
        assert_eq!(
            attribute(&content, "w:rFonts", "w:ascii").as_deref(),
            Some("MS Gothic"),
            "{}: {content}",
            way(by_form)
        );
    }
    // A value over an earlier value keeps that value's formatting.
    let earlier = r#"<w:p><w:sdt><w:sdtPr><w:rPr><w:b/></w:rPr><w:tag w:val="name"/><w:id w:val="42"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:rPr><w:i/></w:rPr><w:t>Grace Hopper</w:t></w:r></w:sdtContent></w:sdt></w:p>"#;
    for by_form in [false, true] {
        let mut doc = doc_with_body(earlier);
        set_value(&mut doc, "name", "Ada Lovelace", by_form);
        let doc = reopen(&mut doc);
        assert_eq!(
            run_formatting(&doc),
            [("Ada Lovelace".to_owned(), None, None, Some(true))],
            "{}",
            way(by_form)
        );
    }
}

#[test]
fn a_value_set_on_a_prompt_in_a_localized_placeholder_style_drops_that_style() {
    // French Word gives the built-in style an id of its own, and keeps its
    // English name.
    let localized = r#"<w:style w:type="character" w:styleId="Textedelespacerserv"><w:name w:val="Placeholder Text"/><w:basedOn w:val="DefaultParagraphFont"/><w:rPr><w:color w:val="666666"/></w:rPr></w:style>"#;
    // The control has no run properties of its own: the run keeps its
    // other formatting.
    for (properties, italic) in [
        (r#"<w:rStyle w:val="Textedelespacerserv"/>"#, None),
        (
            r#"<w:rStyle w:val="Textedelespacerserv"/><w:i/>"#,
            Some(true),
        ),
    ] {
        let body = format!(
            r#"<w:p><w:sdt><w:sdtPr><w:tag w:val="name"/><w:id w:val="42"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:rPr>{properties}</w:rPr><w:t>Saisissez votre nom</w:t></w:r></w:sdtContent></w:sdt></w:p>"#
        );
        for by_form in [false, true] {
            let mut doc = with_style(&mut doc_with_body(&body), localized);
            set_value(&mut doc, "name", "Ada Lovelace", by_form);
            let doc = reopen(&mut doc);
            assert_eq!(
                run_formatting(&doc),
                [("Ada Lovelace".to_owned(), None, None, italic)],
                "{properties}, {}: the value is not greyed out",
                way(by_form)
            );
        }
    }
}

/// `doc` saved with `style` added to its styles part.
fn with_style(doc: &mut Document, style: &str) -> Document {
    let bytes = doc.to_bytes().expect("document saves");
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    let xml = String::from_utf8(
        package
            .get_part("/word/styles.xml")
            .expect("styles")
            .to_vec(),
    )
    .expect("styles are UTF-8");
    let end = xml.rfind("</w:styles>").expect("styles end");
    let xml = format!("{}{style}{}", &xml[..end], &xml[end..]);
    package.set_part("/word/styles.xml", xml.into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).expect("package writes");
    Document::from_bytes(&out.into_inner()).expect("document with the style opens")
}

#[test]
fn a_line_break_in_a_value_keeps_a_run_child_after_the_text_after_the_value() {
    let body = r#"<w:p><w:sdt><w:sdtPr><w:tag w:val="name"/><w:id w:val="42"/><w:text w:multiLine="1"/></w:sdtPr><w:sdtContent><w:r><w:t>Grace</w:t><x:mark xmlns:x="urn:example"/></w:r></w:sdtContent></w:sdt></w:p>"#;
    let child = Regex::new(r"<(w:t|w:br|x:mark)\b").unwrap();
    for by_form in [false, true] {
        let mut doc = doc_with_body(body);
        set_value(&mut doc, "name", "Ada\nLondon", by_form);
        let doc = reopen(&mut doc);
        let content = saved_content(&doc, "word/document.xml", "name");
        let order = child
            .captures_iter(&content)
            .map(|captures| captures[1].to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            order,
            ["w:t", "w:br", "w:t", "x:mark"],
            "{}: {content}",
            way(by_form)
        );
    }
}

#[test]
fn values_by_alias_and_from_a_map_prefer_the_tag() {
    let control = |tag: &str, alias: &str, id: i32| {
        let tag = if tag.is_empty() {
            String::new()
        } else {
            format!(r#"<w:tag w:val="{tag}"/>"#)
        };
        format!(
            r#"<w:p><w:sdt><w:sdtPr><w:alias w:val="{alias}"/>{tag}<w:id w:val="{id}"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>old</w:t></w:r></w:sdtContent></w:sdt></w:p>"#
        )
    };
    let mut doc = doc_with_body(&format!(
        "{}{}{}",
        control("city", "City", 1),
        control("", "City", 2),
        control("zip", "Postcode", 3)
    ));
    assert_eq!(doc.content_controls_by_alias("City").len(), 2);
    assert_eq!(
        doc.set_content_control_value_by_alias("City", "Lyon")
            .unwrap(),
        2
    );
    let mut doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Lyon", "Lyon", "old"]);

    let values: HashMap<String, String> =
        [("city", "Paris"), ("City", "Nice"), ("Postcode", "69001")]
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect();
    assert_eq!(doc.bind_content_controls(&values).unwrap(), 3);
    let doc = reopen(&mut doc);
    // The first control has a tag in the map, so its alias does not count.
    assert_eq!(texts(&doc), ["Paris", "Nice", "69001"]);
    assert_eq!(value(&doc, "zip"), "69001");
}

#[test]
fn filling_several_tags_is_all_or_nothing() {
    let mut doc = doc_with_body(
        r#"<w:p><w:sdt><w:sdtPr><w:tag w:val="party"/><w:id w:val="1"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>A</w:t></w:r></w:sdtContent></w:sdt></w:p><w:p><w:sdt><w:sdtPr><w:tag w:val="party"/><w:id w:val="2"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>A</w:t></w:r></w:sdtContent></w:sdt></w:p><w:p><w:sdt><w:sdtPr><w:tag w:val="date"/><w:id w:val="3"/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>B</w:t></w:r></w:sdtContent></w:sdt></w:p>"#,
    );
    let before = doc.to_bytes().unwrap();
    for pairs in [
        vec![("party", "Contoso"), ("missing", "x")],
        vec![("party", "Contoso"), ("party", "Fabrikam")],
        vec![("party", "Contoso"), ("date", "bad\u{1}")],
    ] {
        assert!(doc.fill_content_control_tags(&pairs).is_err(), "{pairs:?}");
        assert_eq!(doc.to_bytes().unwrap(), before, "{pairs:?}");
    }
    assert_eq!(
        doc.fill_content_control_tags(&[]).unwrap(),
        Vec::<usize>::new()
    );

    // One count per pair, in the caller's order.
    assert_eq!(
        doc.fill_content_control_tags(&[("date", "1 May"), ("party", "Contoso")])
            .unwrap(),
        [1, 2]
    );
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Contoso", "Contoso", "1 May"]);
}

/// `doc_with_control` with a date control in `format` and, when given, the
/// language `lid`, written as Word writes them (`w:dateFormat`, `w:lid`):
/// a format oxoffice refuses to author is still read from a file.
fn date_control(tag: &str, format: &str, lid: Option<&str>) -> Document {
    let mut doc = typed_control(
        tag,
        ContentControlTypePayload::Date {
            format: "yyyy-MM-dd".to_owned(),
        },
    );
    let bytes = doc.to_bytes().expect("document saves");
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    let part = package.main_document_part().expect("main document part");
    let xml = String::from_utf8(package.get_part(&part).expect("main part").to_vec())
        .expect("main part is UTF-8");
    let authored = r#"<w:dateFormat w:val="yyyy-MM-dd"/>"#;
    assert_eq!(xml.matches(authored).count(), 1, "{xml}");
    let lid = lid.map_or(String::new(), |lid| format!(r#"<w:lid w:val="{lid}"/>"#));
    let xml = xml.replace(
        authored,
        &format!(r#"<w:dateFormat w:val="{format}"/>{lid}"#),
    );
    package.set_part(&part, xml.into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).expect("package writes");
    Document::from_bytes(&out.into_inner()).expect("the date control opens")
}

#[test]
fn a_date_name_the_controls_language_has_no_table_for_is_refused() {
    for (format, lid, named) in [
        ("d MMMM yyyy", Some("ja-JP"), "ja-JP"),
        ("d MMM yyyy", Some("fr-FR"), "fr-FR"),
        ("dd/MM/yyyy Q", Some("en-GB"), "\"Q\""),
    ] {
        for by_form in [false, true] {
            let mut doc = date_control("due", format, lid);
            let before = doc.to_bytes().unwrap();
            let result = if by_form {
                doc.fill_form(
                    &[prompt(target("due"), false, "")],
                    &[assign(target("due"), text("2015-01-02"))],
                )
                .map(|_| ())
            } else {
                doc.set_content_control_value_by_tag("due", "2015-01-02")
                    .map(|_| ())
            };
            let error = result.expect_err("the date cannot be shown");
            assert!(
                error.to_string().contains(named) && error.to_string().contains(format),
                "{} {format}: {error}",
                way(by_form)
            );
            assert_eq!(doc.to_bytes().unwrap(), before, "{format}");
        }
        let doc = date_control("due", format, lid);
        let issues = doc
            .validate_form(&[prompt(target("due"), false, "")])
            .unwrap();
        assert_eq!(issues.len(), 1, "{format}: {issues:?}");
        assert_eq!(issues[0].message, "unsupported date metadata");
    }
}

/// `doc` saved, with `from` replaced by `to` once in its main part.
fn with_main_part_edit(doc: &mut Document, from: &str, to: &str) -> Document {
    let bytes = doc.to_bytes().expect("document saves");
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    let part = package.main_document_part().expect("main document part");
    let xml = String::from_utf8(package.get_part(&part).expect("main part").to_vec())
        .expect("main part is UTF-8");
    assert_eq!(xml.matches(from).count(), 1, "{from} in {xml}");
    package.set_part(&part, xml.replace(from, to).into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).expect("package writes");
    Document::from_bytes(&out.into_inner()).expect("the edited document opens")
}

#[test]
fn a_time_typed_in_word_is_checked_like_a_date() {
    let mut doc = date_control("due", "M/d/yyyy h:mm am/pm", Some("en-US"));
    set_value(&mut doc, "due", "2015-01-02", false);
    let rule = prompt(target("due"), false, "");
    // Word saves the time typed in the control, 3:05 PM.
    let mut doc = with_main_part_edit(&mut doc, "2015-01-02T00:00:00Z", "2015-01-02T15:05:00Z");
    let mut doc = with_main_part_edit(&mut doc, "1/2/2015 12:00 AM", "1/2/2015 3:05 PM");
    let issues = doc.validate_form(std::slice::from_ref(&rule)).unwrap();
    assert!(issues.is_empty(), "{issues:?}");
    // Text that is not what the saved time shows is a wrong value.
    let mut doc = with_main_part_edit(&mut doc, "1/2/2015 3:05 PM", "1/2/2015 3:06 PM");
    let issues = doc.validate_form(std::slice::from_ref(&rule)).unwrap();
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].message, "current form value is invalid");
    // A date followed by anything but a time is a wrong value.
    let doc = with_main_part_edit(&mut doc, "2015-01-02T15:05:00Z", "2015-01-02xyz");
    let issues = doc.validate_form(std::slice::from_ref(&rule)).unwrap();
    assert_eq!(issues[0].message, "current form value is invalid");
    // A date-only format with a time is a wrong value, as before.
    let mut doc = date_control("due", "d MMMM yyyy", None);
    set_value(&mut doc, "due", "2015-01-02", false);
    let doc = with_main_part_edit(&mut doc, "2015-01-02T00:00:00Z", "2015-01-02T15:05:00Z");
    let issues = doc.validate_form(&[rule]).unwrap();
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].message, "current form value is invalid");
}

#[test]
fn a_control_whose_format_shows_a_time_takes_a_date_and_time() {
    let rule = prompt(target("due"), false, "");
    for by_form in [false, true] {
        let mut doc = date_control("due", "M/d/yyyy h:mm am/pm", Some("en-US"));
        set_value(&mut doc, "due", "2015-01-02T15:05", by_form);
        let doc = reopen(&mut doc);
        assert_eq!(value(&doc, "due"), "1/2/2015 3:05 PM", "{}", way(by_form));
        let properties = saved_properties(&doc, "word/document.xml", 1);
        assert_eq!(
            attribute(&properties, "w:date", "w:fullDate").as_deref(),
            Some("2015-01-02T15:05:00Z")
        );
        let issues = doc.validate_form(std::slice::from_ref(&rule)).unwrap();
        assert!(issues.is_empty(), "{}: {issues:?}", way(by_form));

        // A format that shows no time cannot show one: refused, unchanged.
        let mut doc = date_control("due", "d MMMM yyyy", None);
        let before = doc.to_bytes().unwrap();
        let result = if by_form {
            doc.fill_form(
                std::slice::from_ref(&rule),
                &[assign(target("due"), text("2015-01-02T15:05"))],
            )
            .map(|_| ())
        } else {
            doc.set_content_control_value_by_tag("due", "2015-01-02T15:05")
                .map(|_| ())
        };
        let error = result.expect_err("a date-only format shows no time");
        assert!(error.to_string().contains("shows no time"), "{error}");
        assert_eq!(doc.to_bytes().unwrap(), before, "{}", way(by_form));
    }
}

#[test]
fn a_date_control_on_the_us_gregorian_calendar_is_set_and_checked() {
    // The calendar is Gregorian in US English: English names, whatever the
    // control's language, also one oxoffice has no names for.
    for lid in ["fr-FR", "ja-JP"] {
        let mut doc = date_control("due", "MMMM d, yyyy", Some(lid));
        let mut doc = with_main_part_edit(
            &mut doc,
            r#"<w:calendar w:val="gregorian"/>"#,
            r#"<w:calendar w:val="gregorianUs"/>"#,
        );
        for by_form in [false, true] {
            set_value(&mut doc, "due", "2015-01-02", by_form);
            assert_eq!(
                value(&doc, "due"),
                "January 2, 2015",
                "{lid} {}",
                way(by_form)
            );
        }
        let doc = reopen(&mut doc);
        let issues = doc
            .validate_form(&[prompt(target("due"), false, "")])
            .unwrap();
        assert!(issues.is_empty(), "{lid}: {issues:?}");
    }
}

#[test]
fn a_bound_combo_box_stores_text_outside_its_items_and_a_form_takes_it() {
    let items = vec![("France".to_owned(), "FR".to_owned())];
    let mut doc = typed_control(
        "country",
        ContentControlTypePayload::ComboBox {
            items: items.clone(),
        },
    );
    let store = doc
        .add_custom_xml_store(b"<supplier><country>FR</country></supplier>", &[])
        .unwrap();
    let location = control_at(&doc, &body(&doc), "country");
    doc.set_content_control_binding(&location, &store, "/supplier/country", None)
        .unwrap();
    doc.set_content_control_value_by_tag("country", "Belgium")
        .unwrap();
    let mut doc = reopen(&mut doc);
    assert_eq!(value(&doc, "country"), "Belgium");
    assert_eq!(
        doc.part_xml("customXml/item1.xml").unwrap(),
        "<supplier><country>Belgium</country></supplier>"
    );

    let rule = prompt(target("country"), true, "Choose a country");
    assert_eq!(
        doc.fill_form(
            std::slice::from_ref(&rule),
            &[assign(target("country"), text("Luxembourg"))]
        )
        .unwrap(),
        [1]
    );
    let doc = reopen(&mut doc);
    assert_eq!(value(&doc, "country"), "Luxembourg");
    assert_eq!(
        doc.part_xml("customXml/item1.xml").unwrap(),
        "<supplier><country>Luxembourg</country></supplier>"
    );
    assert!(
        doc.validate_form(&[rule]).unwrap().is_empty(),
        "free text in a combo box is a value"
    );
}

#[test]
fn picture_and_group_controls_refuse_a_text_value() {
    let mut source = Document::new();
    source.add_picture(PNG, "logo.png", Length::inches(1.0), Length::inches(1.0));
    let mut doc = wrapped_in_control(&mut source, "logo");
    let relationship = doc.pictures().unwrap()[0].relationship_id.clone();
    let location = control_at(&doc, &body(&doc), "logo");
    let before = doc.to_bytes().unwrap();
    // The picture must be one the control holds.
    assert!(
        doc.set_content_control_type(
            &location,
            ContentControlTypePayload::Picture {
                relationship_id: "rId999".to_owned()
            }
        )
        .is_err()
    );
    assert_eq!(doc.to_bytes().unwrap(), before);
    doc.set_content_control_type(
        &location,
        ContentControlTypePayload::Picture {
            relationship_id: relationship,
        },
    )
    .unwrap();
    let mut doc = reopen(&mut doc);
    assert_eq!(
        listed(&doc),
        [(
            Some("logo".into()),
            Some(5),
            Some(SdtType::Picture),
            "".into()
        )]
    );
    let pictures = doc.pictures().unwrap();
    assert_eq!(pictures.len(), 1);
    assert_eq!(pictures[0].bytes, PNG);

    let mut group = typed_control("group", ContentControlTypePayload::Group);
    for (doc, tag) in [(&mut doc, "logo"), (&mut group, "group")] {
        let before = doc.to_bytes().unwrap();
        let error = doc
            .set_content_control_value_by_tag(tag, "text")
            .unwrap_err();
        assert!(error.to_string().contains("text value"), "{tag}: {error}");
        assert_eq!(doc.to_bytes().unwrap(), before, "{tag}");
    }
}

// ---- Data binding ----

#[test]
fn a_bound_control_writes_its_value_into_the_custom_xml_part() {
    let item = r#"<c:customer xmlns:c="urn:crm"><c:name>Ada</c:name><!--kept--><c:city>Paris</c:city></c:customer>"#;
    let mut doc = doc_with_control("name");
    let store = doc
        .add_custom_xml_store(item.as_bytes(), &["urn:crm"])
        .unwrap();
    let location = control_at(&doc, &body(&doc), "name");
    let mapping = Some("xmlns:c='urn:crm'");

    let before = doc.to_bytes().unwrap();
    let missing_store = "{00000000-0000-4000-8000-0000000000AA}";
    assert!(
        doc.set_content_control_binding(&location, missing_store, "/c:customer/c:name", mapping)
            .is_err()
    );
    assert!(
        doc.set_content_control_binding(&location, &store, "/c:customer/c:phone", mapping)
            .is_err()
    );
    assert!(
        doc.set_content_control_binding(&location, &store, "/c:customer/c:name", None)
            .is_err(),
        "the prefix needs its mapping"
    );
    assert_eq!(doc.to_bytes().unwrap(), before);

    doc.set_content_control_binding(&location, &store, "/c:customer/c:name", mapping)
        .unwrap();
    assert_eq!(
        doc.set_content_control_value_by_tag("name", "Grace")
            .unwrap(),
        1
    );
    let doc = reopen(&mut doc);

    assert_eq!(value(&doc, "name"), "Grace");
    let properties = saved_properties(&doc, "word/document.xml", 1);
    assert_eq!(
        attribute(&properties, "w:dataBinding", "w:storeItemID").as_deref(),
        Some(store.as_str())
    );
    assert_eq!(
        attribute(&properties, "w:dataBinding", "w:xpath").as_deref(),
        Some("/c:customer/c:name")
    );
    assert_eq!(
        doc.part_xml("customXml/item1.xml").unwrap(),
        item.replace("Ada", "Grace"),
        "only the bound element changes"
    );
    let item_properties = doc.part_xml("customXml/itemProps1.xml").unwrap();
    assert_eq!(
        attribute(&item_properties, "ds:datastoreItem", "ds:itemID").as_deref(),
        Some(store.as_str())
    );
    assert_eq!(
        attribute(&item_properties, "ds:schemaRef", "ds:uri").as_deref(),
        Some("urn:crm")
    );
}

#[test]
fn bound_checkbox_and_date_controls_store_what_word_reads() {
    let mut doc = typed_control("agree", ContentControlTypePayload::CheckBox);
    let end = ContentLocation::end(body(&doc));
    doc.insert_content_control(&end, spec(Block, SdtType::RichText, "due"))
        .unwrap();
    let location = control_at(&doc, &body(&doc), "due");
    doc.set_content_control_type(
        &location,
        ContentControlTypePayload::Date {
            format: "dd/MM/yyyy".to_owned(),
        },
    )
    .unwrap();
    let store = doc
        .add_custom_xml_store(b"<order><agreed>false</agreed><due/></order>", &[])
        .unwrap();
    for (tag, xpath) in [("agree", "/order/agreed"), ("due", "/order/due")] {
        let location = control_at(&doc, &body(&doc), tag);
        doc.set_content_control_binding(&location, &store, xpath, None)
            .unwrap();
    }
    assert_eq!(
        doc.fill_content_control_tags(&[("agree", "yes"), ("due", "2026-12-25")])
            .unwrap(),
        [1, 1]
    );
    let doc = reopen(&mut doc);
    assert_eq!(value(&doc, "agree"), "\u{2612}");
    assert_eq!(value(&doc, "due"), "25/12/2026");
    // A checkbox stores true or false; a date stored as text stores what it shows.
    assert_eq!(
        doc.part_xml("customXml/item1.xml").unwrap(),
        "<order><agreed>true</agreed><due>25/12/2026</due></order>"
    );
}

// ---- Repeating, cloning, moving, unwrapping, removing ----

#[test]
fn a_repeating_section_repeats_its_item_and_keeps_the_last_one() {
    let mut doc = doc_with_body(
        r#"<w:sdt><w:sdtPr><w:tag w:val="lines"/><w:id w:val="1"/><w:richText/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>Line item</w:t></w:r></w:p></w:sdtContent></w:sdt>"#,
    );
    let location = control_at(&doc, &body(&doc), "lines");
    doc.set_content_control_type(&location, ContentControlTypePayload::RepeatingSection)
        .unwrap();
    let location = control_at(&doc, &body(&doc), "lines");
    doc.repeat_content_control(&location).unwrap();
    let mut doc = reopen(&mut doc);

    // The section's content became its first item, and the repeat a second
    // one. The section reads its items' paragraphs, one line each.
    let found = listed(&doc);
    let kinds: Vec<_> = found
        .iter()
        .map(|(tag, id, kind, _)| (tag.clone(), *id, *kind))
        .collect();
    assert_eq!(
        kinds,
        [
            (
                Some("lines".to_owned()),
                Some(1),
                Some(SdtType::RepeatingSection)
            ),
            (None, Some(2), Some(SdtType::RepeatingSectionItem)),
            (None, Some(3), Some(SdtType::RepeatingSectionItem)),
        ]
    );
    assert_eq!(found[0].3, "Line item\nLine item");
    assert_eq!(found[1].3, "Line item");
    assert_eq!(found[2].3, "Line item");
    assert_eq!(texts(&doc), ["Line item", "Line item"]);

    let items = |doc: &Document| -> Vec<ContentLocation> {
        doc.story_items(&body(doc))
            .unwrap()
            .into_iter()
            .filter(|item| item.kind() == StoryItemKind::ContentControl)
            .skip(1)
            .map(|item| item.location().clone())
            .collect()
    };
    doc.remove_content_control(&items(&doc)[1]).unwrap();
    let before = doc.to_bytes().unwrap();
    let error = doc.remove_content_control(&items(&doc)[0]).unwrap_err();
    assert!(
        error.to_string().contains("last repeating section item"),
        "{error}"
    );
    assert_eq!(doc.to_bytes().unwrap(), before);
    let doc = reopen(&mut doc);
    assert_eq!(listed(&doc).len(), 2);
    assert_eq!(texts(&doc), ["Line item"]);
}

#[test]
fn a_cloned_control_gets_a_new_id_and_a_moved_one_keeps_its_own() {
    let mut doc = doc_with(&["first", "second"]);
    let first = doc.paragraph_story_location(0).unwrap().unwrap();
    doc.insert_content_control(&first, spec(Inline, SdtType::PlainText, "who"))
        .unwrap();
    let end = ContentLocation::end(body(&doc));
    doc.insert_content_control(&end, spec(Block, SdtType::RichText, "block"))
        .unwrap();
    doc.fill_content_control_tags(&[("who", " Ada"), ("block", "Block text")])
        .unwrap();
    let header = doc
        .create_section_story(0, HeaderFooterKind::Header, HdrFtrType::Default)
        .unwrap();

    let source = control_at(&doc, &body(&doc), "who");
    let second = doc.paragraph_story_location(1).unwrap().unwrap();
    doc.clone_content_control(&source, &second).unwrap();
    let block = control_at(&doc, &body(&doc), "block");
    doc.clone_content_control(&block, &ContentLocation::end(header.clone()))
        .unwrap();
    // A move stays inside its story.
    let block = control_at(&doc, &body(&doc), "block");
    let before = doc.to_bytes().unwrap();
    assert!(
        doc.move_content_control(&block, &ContentLocation::end(header))
            .is_err()
    );
    assert_eq!(doc.to_bytes().unwrap(), before);
    let start = doc.paragraph_story_location(0).unwrap().unwrap();
    doc.move_content_control(&block, &start).unwrap();
    let doc = reopen(&mut doc);

    assert_eq!(texts(&doc), ["Block text", "first Ada", "second Ada"]);
    // The body's controls come first, then the header's copy.
    assert_eq!(
        listed(&doc),
        [
            (
                Some("block".into()),
                Some(2),
                Some(SdtType::RichText),
                "Block text".into()
            ),
            (
                Some("who".into()),
                Some(1),
                Some(SdtType::PlainText),
                " Ada".into()
            ),
            (
                Some("who".into()),
                Some(3),
                Some(SdtType::PlainText),
                " Ada".into()
            ),
            (
                Some("block".into()),
                Some(4),
                Some(SdtType::RichText),
                "Block text".into()
            ),
        ]
    );
    let header = story(&doc, StoryKind::Header);
    assert_eq!(item_text(&doc, &header, "block"), "Block text");
    let (_, copy) = control_item(&doc, &header, "block");
    assert!(copy.contains(r#"<w:id w:val="4"/>"#), "{copy}");
}

#[test]
fn a_control_that_is_all_of_a_cell_can_be_unwrapped_but_not_removed() {
    let mut doc = doc_with_body(
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="4000"/></w:tblGrid><w:tr><w:tc><w:sdt><w:sdtPr><w:tag w:val="address"/><w:id w:val="7"/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>2 Canal Wharf</w:t></w:r></w:p><w:p><w:r><w:t>Leeds</w:t></w:r></w:p></w:sdtContent></w:sdt></w:tc></w:tr></w:tbl><w:p/>"#,
    );
    let cell = story(&doc, StoryKind::TableCell);
    let location = control_at(&doc, &cell, "address");
    let before = doc.to_bytes().unwrap();
    // A cell needs a paragraph, and the control holds all of them.
    assert!(doc.remove_content_control(&location).is_err());
    assert_eq!(doc.to_bytes().unwrap(), before);

    doc.unwrap_content_control(&location).unwrap();
    let doc = reopen(&mut doc);
    assert!(doc.content_controls().is_empty());
    let table = doc.table(0).unwrap();
    let cell = table.cell(0, 0).unwrap();
    let paragraphs: Vec<String> = cell.paragraphs().map(|p| p.text()).collect();
    assert_eq!(paragraphs, ["2 Canal Wharf", "Leeds"]);
}

#[test]
fn a_placeholder_building_block_gives_a_control_its_prompt() {
    let block = |name: &str, gallery: &str| {
        let mut paragraph = CT_P::new();
        paragraph.add_run("Enter the client name");
        let mut body = CT_Body::new();
        body.sect_pr = None;
        body.content
            .push(BodyContent::Paragraph(Box::new(paragraph)));
        BuildingBlock {
            name: name.to_owned(),
            kind: BuildingBlockKind::BuildingBlock,
            category: Some("General".to_owned()),
            description: None,
            guid: None,
            gallery: Some(gallery.to_owned()),
            behaviors: Vec::new(),
            body,
        }
    };
    let mut doc = doc_with_control("client");
    let prompt = doc
        .create_building_block(block("ClientPrompt", "placeholder"))
        .unwrap();
    let other = doc
        .create_building_block(block("Signature", "autoTxt"))
        .unwrap();
    let location = control_at(&doc, &body(&doc), "client");
    let before = doc.to_bytes().unwrap();
    assert!(
        doc.bind_building_block_placeholder(&location, &other)
            .is_err()
    );
    assert_eq!(doc.to_bytes().unwrap(), before);

    doc.bind_building_block_placeholder(&location, &prompt)
        .unwrap();
    let doc = reopen(&mut doc);
    let properties = saved_properties(&doc, "word/document.xml", 1);
    assert_eq!(
        val(&properties, "w:docPart").as_deref(),
        Some("ClientPrompt")
    );
    let entry = doc
        .building_blocks()
        .unwrap()
        .into_iter()
        .find(|entry| entry.block.name == "ClientPrompt")
        .expect("the placeholder entry is kept");
    assert_eq!(entry.block.gallery.as_deref(), Some("placeholder"));
    let prompt_text: Vec<String> = entry.block.body.paragraphs().map(|p| p.text()).collect();
    assert_eq!(prompt_text, ["Enter the client name"]);
}

// ---- The corpus form and thesis ----

#[test]
fn a_value_drops_the_emptied_paragraphs_that_hold_only_proofing_and_rendering_marks() {
    // Word marks a flagged word with w:proofErr around it, and where its
    // last layout broke the page with w:lastRenderedPageBreak.
    let body = r#"<w:sdt><w:sdtPr><w:tag w:val="address"/><w:id w:val="7"/><w:richText/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>2 Canal Wharf</w:t></w:r></w:p><w:p><w:proofErr w:type="spellStart"/><w:r><w:t>Leeds</w:t></w:r><w:proofErr w:type="spellEnd"/></w:p><w:p><w:r><w:lastRenderedPageBreak/><w:t>United Kingdom</w:t></w:r></w:p></w:sdtContent></w:sdt>"#;
    for by_form in [false, true] {
        let mut doc = doc_with_body(body);
        set_value(&mut doc, "address", "1 New Road", by_form);
        let doc = reopen(&mut doc);
        assert_eq!(texts(&doc), ["1 New Road"], "{}", way(by_form));
        let content = saved_content(&doc, "word/document.xml", "address");
        assert_eq!(paragraph_count(&content), 1, "{}: {content}", way(by_form));
    }
}

#[test]
fn a_value_keeps_a_paragraph_of_the_control_that_holds_a_section_break_or_a_property_change() {
    let kept = [
        (
            "w:sectPr",
            r#"<w:pPr><w:sectPr><w:pgSz w:w="16838" w:h="11906" w:orient="landscape"/></w:sectPr></w:pPr>"#,
        ),
        (
            "w:pPrChange",
            r#"<w:pPr><w:jc w:val="center"/><w:pPrChange w:id="92" w:author="Ada" w:date="2026-11-21T10:00:00Z"><w:pPr/></w:pPrChange></w:pPr>"#,
        ),
    ];
    // A section break, a paragraph property change: the emptied paragraph
    // goes, the one with the state stays.
    for (state, properties) in kept {
        let body = format!(
            r#"<w:sdt><w:sdtPr><w:tag w:val="terms"/><w:id w:val="7"/><w:richText/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>Old terms</w:t></w:r></w:p><w:p><w:r><w:t>more</w:t></w:r></w:p><w:p>{properties}<w:r><w:t>end</w:t></w:r></w:p></w:sdtContent></w:sdt><w:p><w:r><w:t>After</w:t></w:r></w:p>"#
        );
        for by_form in [false, true] {
            let mut doc = doc_with_body(&body);
            set_value(&mut doc, "terms", "New terms", by_form);
            let doc = reopen(&mut doc);
            let content = saved_content(&doc, "word/document.xml", "terms");
            assert_eq!(
                paragraph_count(&content),
                2,
                "{state}, {}: {content}",
                way(by_form)
            );
            assert!(has(&content, state), "{state}, {}: {content}", way(by_form));
        }
    }
}

#[test]
fn a_value_replaces_the_tracked_paragraph_marks_of_a_control() {
    // A mark inserted, deleted or moved with track changes on is content the
    // value replaces: a paragraph that held only old content goes, the one
    // that takes the value ends with a plain mark, which keeps it apart from
    // the paragraph after the control.
    let marked = |id: i32, change: &str| {
        format!(r#"<w:pPr><w:rPr><w:{change} w:id="{id}" {TRACKED}/></w:rPr></w:pPr>"#)
    };
    for change in ["ins", "del", "moveFrom", "moveTo"] {
        let first = marked(90, change);
        let last = marked(91, change);
        let body = format!(
            r#"<w:sdt><w:sdtPr><w:tag w:val="terms"/><w:id w:val="7"/><w:richText/></w:sdtPr><w:sdtContent><w:p>{first}<w:r><w:t>Old terms</w:t></w:r></w:p><w:p><w:r><w:t>more</w:t></w:r></w:p><w:p>{last}<w:r><w:t>end</w:t></w:r></w:p></w:sdtContent></w:sdt><w:p><w:r><w:t>After</w:t></w:r></w:p>"#
        );
        for by_form in [false, true] {
            let mut doc = doc_with_body(&body);
            set_value(&mut doc, "terms", "New terms", by_form);
            let doc = reopen(&mut doc);
            assert_eq!(
                texts(&doc),
                ["New terms", "After"],
                "w:{change}, {}",
                way(by_form)
            );
            let content = saved_content(&doc, "word/document.xml", "terms");
            assert_eq!(
                paragraph_count(&content),
                1,
                "w:{change}, {}: {content}",
                way(by_form)
            );
            assert!(
                !has(&content, &format!("w:{change}")),
                "w:{change}, {}: {content}",
                way(by_form)
            );
        }
    }
}

/// Rules over two controls and two legacy fields of the corpus form: the
/// bank name (ordinal 0) and the payment terms list (ordinal 3).
fn form_rules() -> [FormFieldRule; 4] {
    [
        prompt(target("company_name"), true, "Enter the company name"),
        prompt(target("category"), true, "Choose a category"),
        FormFieldRule {
            target: legacy(0),
            required: true,
            reset: FormResetValue::Value(text("")),
        },
        FormFieldRule {
            target: legacy(3),
            required: false,
            reset: FormResetValue::Value(FormFieldValue::SelectedIndex(0)),
        },
    ]
}

/// Name and value of the bank name and payment terms legacy fields.
fn bank_fields(doc: &Document) -> Vec<(Option<String>, LegacyFormFieldValue)> {
    doc.legacy_form_fields()
        .unwrap()
        .into_iter()
        .filter(|field| [0, 3].contains(&field.ordinal))
        .map(|field| (field.name, field.value))
        .collect()
}

#[test]
fn a_form_fill_sets_controls_and_legacy_fields_in_one_call() {
    let mut doc = corpus_doc("authored/form.docx");
    assert_eq!(
        doc.fill_form(
            &form_rules(),
            &[
                assign(target("company_name"), text("Fabrikam Ltd")),
                assign(target("category"), text("Strategic")),
                assign(legacy(0), text("HSBC UK")),
                assign(legacy(3), FormFieldValue::SelectedIndex(2)),
            ],
        )
        .unwrap(),
        [1, 1, 1, 1]
    );
    let doc = reopen(&mut doc);
    assert_eq!(value(&doc, "company_name"), "Fabrikam Ltd");
    assert_eq!(value(&doc, "category"), "Strategic");
    assert_eq!(value(&doc, "company_type"), "Limited company", "untouched");
    assert_eq!(
        bank_fields(&doc),
        [
            (
                Some("BankName".into()),
                LegacyFormFieldValue::Text("HSBC UK".into())
            ),
            (
                Some("PaymentTerms".into()),
                LegacyFormFieldValue::SelectedIndex(2)
            ),
        ]
    );
}

#[test]
fn a_checkbox_state_spelled_in_another_case_is_an_invalid_form_value() {
    // On/off values are lowercase: `True` is no spelling of on, for the form
    // reader as for the checkbox model.
    let checkbox = |value: &str| {
        format!(
            r#"<w:p><w:sdt><w:sdtPr><w:tag w:val="agree"/><w14:checkbox xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml"><w14:checked w14:val="{value}"/><w14:checkedState w14:val="2612" w14:font="MS Gothic"/><w14:uncheckedState w14:val="2610" w14:font="MS Gothic"/></w14:checkbox></w:sdtPr><w:sdtContent><w:r><w:t>☒</w:t></w:r></w:sdtContent></w:sdt></w:p>"#
        )
    };
    let rule = FormFieldRule {
        target: target("agree"),
        required: false,
        reset: FormResetValue::Value(FormFieldValue::Checked(false)),
    };
    let doc = doc_with_body(&checkbox("on"));
    let issues = doc.validate_form(std::slice::from_ref(&rule)).unwrap();
    assert!(issues.is_empty(), "{issues:?}");
    let doc = doc_with_body(&checkbox("True"));
    let issues = doc.validate_form(&[rule]).unwrap();
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].message, "current form value is invalid");
}

#[test]
fn a_placeholder_entry_named_with_a_tab_is_bound_as_named() {
    let mut paragraph = CT_P::new();
    paragraph.add_run("Enter the client name");
    let mut body_content = CT_Body::new();
    body_content.sect_pr = None;
    body_content
        .content
        .push(BodyContent::Paragraph(Box::new(paragraph)));
    let mut doc = doc_with_control("client");
    let prompt = doc
        .create_building_block(BuildingBlock {
            name: "Client\tPrompt".to_owned(),
            kind: BuildingBlockKind::BuildingBlock,
            category: Some("General".to_owned()),
            description: None,
            guid: None,
            gallery: Some("placeholder".to_owned()),
            behaviors: Vec::new(),
            body: body_content,
        })
        .unwrap();
    let location = control_at(&doc, &body(&doc), "client");
    doc.bind_building_block_placeholder(&location, &prompt)
        .unwrap();
    let doc = reopen(&mut doc);
    // Raw, the tab would read as a space: it is written as a reference.
    let properties = saved_properties(&doc, "word/document.xml", 1);
    assert_eq!(
        val(&properties, "w:docPart").as_deref(),
        Some("Client&#9;Prompt"),
        "{properties}"
    );
}

#[test]
fn an_inline_control_text_reads_its_tracked_insertions() {
    let doc = doc_with_body(concat!(
        r#"<w:p><w:r><w:t>A</w:t></w:r><w:sdt><w:sdtPr><w:id w:val="1"/></w:sdtPr><w:sdtContent>"#,
        r#"<w:r><w:t>B</w:t></w:r>"#,
        r#"<w:ins w:id="5" w:author="A" w:date="2026-01-01T00:00:00Z"><w:r><w:t>C</w:t></w:r></w:ins>"#,
        r#"<w:r><w:t>D</w:t></w:r>"#,
        r#"<w:ins w:id="6" w:author="A" w:date="2026-01-01T00:00:00Z"><w:r><w:t>E</w:t></w:r></w:ins>"#,
        r#"<w:del w:id="7" w:author="A" w:date="2026-01-01T00:00:00Z"><w:r><w:delText>X</w:delText></w:r></w:del>"#,
        r#"</w:sdtContent></w:sdt></w:p>"#,
    ));
    assert_eq!(doc.paragraph(0).unwrap().text(), "ABCDE");
    assert_eq!(doc.content_controls()[0].text(), "BCDE");
}

/// The author and date Word writes on a tracked change.
const TRACKED: &str = r#"w:author="Ada" w:date="2026-11-24T09:00:00Z""#;

/// A tracked insertion of `text`, as Word writes it.
fn insertion(id: i32, text: &str) -> String {
    format!(
        r#"<w:ins w:id="{id}" {TRACKED}><w:r><w:t xml:space="preserve">{text}</w:t></w:r></w:ins>"#
    )
}

/// A tracked deletion of `text`, as Word writes it.
fn deletion(id: i32, text: &str) -> String {
    format!(
        r#"<w:del w:id="{id}" {TRACKED}><w:r><w:delText xml:space="preserve">{text}</w:delText></w:r></w:del>"#
    )
}

/// A paragraph holding "A " then an inline control tagged "t" holding
/// `content`.
fn inline_control(content: &str) -> String {
    format!(
        r#"<w:p><w:r><w:t xml:space="preserve">A </w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="t"/><w:id w:val="1"/></w:sdtPr><w:sdtContent>{content}</w:sdtContent></w:sdt></w:p>"#
    )
}

#[test]
fn a_value_replaces_the_tracked_insertions_of_a_control() {
    // Typed with track changes on: the inserted text shows in the control
    // until the value replaces it, as an edit made with them off. A control
    // holding nothing but the insertion takes the value in a run of its own,
    // and a hyperlink holding nothing but one goes with it. One written
    // without its author, which the model keeps as raw XML, goes too.
    let inline = |content: &str| (inline_control(content), "A X");
    let shapes = [
        inline(&format!(r#"<w:r><w:t>B</w:t></w:r>{}"#, insertion(5, "C"))),
        inline(&insertion(5, "C")),
        inline(r#"<w:r><w:t>B</w:t></w:r><w:ins w:id="5"><w:r><w:t>C</w:t></w:r></w:ins>"#),
        (
            format!(
                r#"<w:sdt><w:sdtPr><w:tag w:val="t"/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>B</w:t></w:r><w:hyperlink w:anchor="Terms">{}</w:hyperlink></w:p></w:sdtContent></w:sdt>"#,
                insertion(5, "C")
            ),
            "X",
        ),
    ];
    for (body, expected) in shapes {
        for by_form in [false, true] {
            let mut doc = doc_with_body(&body);
            set_value(&mut doc, "t", "X", by_form);
            let doc = reopen(&mut doc);
            assert_eq!(texts(&doc), [expected], "{}: {body}", way(by_form));
            assert_eq!(value(&doc, "t"), "X", "{}: {body}", way(by_form));
            let saved = saved_content(&doc, "word/document.xml", "t");
            for left in ["w:ins", "w:hyperlink"] {
                assert!(!has(&saved, left), "{left}, {}: {saved}", way(by_form));
            }
        }
    }
}

#[test]
fn a_value_replaces_the_tracked_deletions_and_moves_of_a_control_and_keeps_those_outside() {
    // The control's own formatting change stays, and so do the insertion and
    // the other end of the move outside it.
    let properties = format!(
        r#"<w:sdtPr><w:rPr><w:b/><w:rPrChange w:id="1" {TRACKED}><w:rPr/></w:rPrChange></w:rPr><w:tag w:val="t"/><w:id w:val="2"/></w:sdtPr>"#
    );
    let source = format!(
        r#"<w:moveFromRangeStart w:id="3" {TRACKED} w:name="move1"/><w:moveFrom w:id="4" {TRACKED}><w:r><w:t>moved</w:t></w:r></w:moveFrom><w:moveFromRangeEnd w:id="3"/>"#
    );
    let destination = format!(
        r#"<w:moveToRangeStart w:id="5" {TRACKED} w:name="move1"/><w:moveTo w:id="6" {TRACKED}><w:r><w:t>moved</w:t></w:r></w:moveTo><w:moveToRangeEnd w:id="5"/>"#
    );
    let outside = insertion(7, "kept ");
    let old = format!(r#"<w:r><w:t>Old</w:t></w:r>{}"#, deletion(8, " gone"));
    let shapes = [
        // A block control holding deleted text and the source of a move
        // whose destination follows it.
        (
            format!(
                r#"<w:sdt>{properties}<w:sdtContent><w:p>{old}{source}</w:p></w:sdtContent></w:sdt><w:p>{outside}{destination}</w:p>"#
            ),
            vec!["X", "kept moved"],
            ["w:ins", "w:moveTo"],
        ),
        // An inline control holding deleted text and the destination of a
        // move whose source comes before it.
        (
            format!(
                r#"<w:p>{source}{outside}<w:sdt>{properties}<w:sdtContent>{old}{destination}</w:sdtContent></w:sdt></w:p>"#
            ),
            vec!["kept X"],
            ["w:ins", "w:moveFrom"],
        ),
    ];
    for (body, expected, kept) in shapes {
        for by_form in [false, true] {
            let mut doc = doc_with_body(&body);
            set_value(&mut doc, "t", "X", by_form);
            let doc = reopen(&mut doc);
            assert_eq!(texts(&doc), expected, "{}: {body}", way(by_form));
            assert_eq!(value(&doc, "t"), "X", "{}", way(by_form));
            let saved = saved_content(&doc, "word/document.xml", "t");
            for change in ["w:ins", "w:del", "w:delText", "w:moveFrom", "w:moveTo"] {
                assert!(!has(&saved, change), "{change}, {}: {saved}", way(by_form));
            }
            let xml = doc.part_xml("word/document.xml").unwrap();
            for change in kept {
                assert!(has(&xml, change), "{change}, {}: {xml}", way(by_form));
            }
            let properties = saved_properties(&doc, "word/document.xml", 2);
            assert!(
                has(&properties, "w:rPrChange"),
                "{}: {properties}",
                way(by_form)
            );
        }
    }
}

#[test]
fn a_value_a_control_already_shows_through_a_tracked_insertion_still_replaces_it() {
    let content = format!(r#"<w:r><w:t>B</w:t></w:r>{}"#, insertion(5, "C"));
    let mut doc = doc_with_body(&inline_control(&content));
    doc.set_content_control_value_by_tag("t", "BC").unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["A BC"]);
    let saved = saved_content(&doc, "word/document.xml", "t");
    assert!(!has(&saved, "w:ins"), "{saved}");
    // A header's control is written in its part.
    let mut doc = doc_with_raw_header(&inline_control(&content));
    assert_eq!(doc.set_content_control_value_by_tag("t", "BC").unwrap(), 1);
    let doc = reopen(&mut doc);
    assert_eq!(value(&doc, "t"), "BC");
    let saved = saved_content(&doc, "word/header1.xml", "t");
    assert!(!has(&saved, "w:ins"), "{saved}");
}

#[test]
fn a_value_drops_the_proofing_mark_paired_with_one_in_a_tracked_change() {
    // Word marks the ends of the text its grammar check flags: one end in
    // the insertion the value drops, the other beside it, among the
    // control's content or in its paragraph. Left alone, the other end would
    // break the document's story ranges.
    let flagged = format!(
        r#"<w:proofErr w:type="gramStart"/><w:r><w:t>They is</w:t></w:r><w:ins w:id="5" {TRACKED}><w:r><w:t xml:space="preserve"> going</w:t></w:r><w:proofErr w:type="gramEnd"/></w:ins>"#
    );
    let shapes = [
        inline_control(&flagged),
        format!(
            r#"<w:sdt><w:sdtPr><w:tag w:val="t"/></w:sdtPr><w:sdtContent><w:p>{flagged}</w:p></w:sdtContent></w:sdt>"#
        ),
    ];
    for body in shapes {
        for by_form in [false, true] {
            let mut doc = doc_with_body(&body);
            set_value(&mut doc, "t", "X", by_form);
            let doc = reopen(&mut doc);
            assert_eq!(value(&doc, "t"), "X", "{}: {body}", way(by_form));
            let saved = saved_content(&doc, "word/document.xml", "t");
            assert!(!has(&saved, "w:proofErr"), "{}: {saved}", way(by_form));
            doc.story_ranges()
                .unwrap_or_else(|error| panic!("{}: {error}: {saved}", way(by_form)));
        }
    }
}

#[test]
fn a_value_is_refused_over_a_tracked_change_holding_what_a_partner_outside_it_needs() {
    // Dropped with the change, a comment would lose its mark, a bookmark its
    // start and a misspelling flagged before the control its end. A bookmark
    // wholly inside the change goes with it.
    let refused = [
        (
            inline_control(&format!(
                r#"<w:commentRangeStart w:id="0"/><w:r><w:t>B</w:t></w:r><w:commentRangeEnd w:id="0"/><w:ins w:id="5" {TRACKED}><w:r><w:t>C</w:t></w:r><w:r><w:commentReference w:id="0"/></w:r></w:ins>"#
            )),
            "comment 0",
        ),
        (
            inline_control(&format!(
                r#"<w:bookmarkStart w:id="3" w:name="Client"/><w:r><w:t>B</w:t></w:r><w:ins w:id="5" {TRACKED}><w:r><w:t>C</w:t></w:r><w:bookmarkEnd w:id="3"/></w:ins>"#
            )),
            "bookmark 3",
        ),
        (
            format!(
                r#"<w:p><w:proofErr w:type="spellStart"/><w:r><w:t xml:space="preserve">A </w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="t"/></w:sdtPr><w:sdtContent><w:r><w:t>B</w:t></w:r><w:ins w:id="5" {TRACKED}><w:r><w:t>C</w:t></w:r><w:proofErr w:type="spellEnd"/></w:ins></w:sdtContent></w:sdt></w:p>"#
            ),
            "spelling mark",
        ),
    ];
    for (body, partner) in refused {
        for by_form in [false, true] {
            let mut doc = doc_with_body(&body);
            let error = if by_form {
                let rule = prompt(target("t"), false, "");
                doc.fill_form(&[rule], &[assign(target("t"), text("X"))])
                    .unwrap_err()
            } else {
                doc.set_content_control_value_by_tag("t", "X").unwrap_err()
            };
            let message = error.to_string();
            assert!(
                message.contains(partner) && message.contains("accept or reject it first"),
                "{}: {message}",
                way(by_form)
            );
            assert_eq!(value(&doc, "t"), "BC", "{}: unchanged", way(by_form));
        }
    }
    let inside = format!(
        r#"<w:r><w:t>B</w:t></w:r><w:ins w:id="5" {TRACKED}><w:bookmarkStart w:id="3" w:name="_GoBack"/><w:bookmarkEnd w:id="3"/><w:r><w:t>C</w:t></w:r></w:ins>"#
    );
    for by_form in [false, true] {
        let mut doc = doc_with_body(&inline_control(&inside));
        set_value(&mut doc, "t", "X", by_form);
        let doc = reopen(&mut doc);
        assert_eq!(value(&doc, "t"), "X", "{}", way(by_form));
    }
}

/// A block content control holding this content.
fn block_control(tag: &str, content: &str) -> String {
    format!(
        r#"<w:sdt><w:sdtPr><w:tag w:val="{tag}"/></w:sdtPr><w:sdtContent>{content}</w:sdtContent></w:sdt>"#
    )
}

/// A paragraph holding this text.
fn text_paragraph(text: &str) -> String {
    format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
}

/// A one-cell table whose cell holds this text.
fn one_cell_table(text: &str) -> String {
    format!(
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc>{}</w:tc></w:tr></w:tbl>"#,
        text_paragraph(text)
    )
}

/// The body story route of each body paragraph text, as story paragraphs
/// lists them.
fn story_paragraph_routes(doc: &Document) -> HashMap<String, ContentLocation> {
    doc.story_paragraphs()
        .expect("story paragraphs are listed")
        .into_iter()
        .filter(|paragraph| paragraph.location.story().kind() == StoryKind::Body)
        .map(|paragraph| (paragraph.text, paragraph.location))
        .collect()
}

#[test]
fn a_paragraph_two_and_three_controls_deep_has_the_story_paragraph_route() {
    let inner = block_control(
        "inner",
        &(text_paragraph("inner a") + &text_paragraph("inner b")),
    );
    let middle = block_control(
        "middle",
        &(text_paragraph("middle a") + &inner + &text_paragraph("middle b")),
    );
    let outer = block_control(
        "outer",
        &(text_paragraph("outer a") + &middle + &text_paragraph("outer b")),
    );
    let doc = doc_with_body(&(text_paragraph("before") + &outer + &text_paragraph("after")));
    let routes = story_paragraph_routes(&doc);
    let texts = texts(&doc);
    assert_eq!(
        texts,
        [
            "before", "outer a", "middle a", "inner a", "inner b", "middle b", "outer b", "after"
        ]
    );
    for (index, text) in texts.iter().enumerate() {
        let location = doc
            .paragraph_story_location(index)
            .unwrap()
            .unwrap_or_else(|| panic!("{text} has a route"));
        assert_eq!(location, routes[text], "{text}");
    }
    // The innermost control's story item, then the ordinal among its own
    // paragraphs.
    let inner_b = doc.paragraph_story_location(4).unwrap().unwrap();
    assert_eq!(inner_b.index_path(), [3, 1]);
    assert_eq!(
        doc.read_story_paragraph(&inner_b, |paragraph| paragraph.text())
            .unwrap(),
        "inner b"
    );
}

#[test]
fn every_body_paragraph_route_is_the_same_one_by_one_and_all_at_once() {
    let inline = |lead: &str| {
        format!(
            r#"<w:p><w:r><w:t>{lead} </w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="who"/></w:sdtPr><w:sdtContent><w:r><w:t>inline</w:t></w:r></w:sdtContent></w:sdt></w:p>"#
        )
    };
    let deep = block_control(
        "deep",
        &(text_paragraph("deep a") + &one_cell_table("deep cell") + &text_paragraph("deep b")),
    );
    let nested = block_control(
        "nested",
        &(text_paragraph("nested a")
            + &inline("nested")
            + &deep
            + &one_cell_table("nested cell")
            + &text_paragraph("nested b")),
    );
    let sibling = block_control("sibling", &text_paragraph("sibling a"));
    // A block control in a cell belongs to the cell story.
    let control_in_cell = format!(
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc>{}</w:tc></w:tr></w:tbl>"#,
        block_control("in cell", &text_paragraph("control in cell"))
    );
    let empty = r#"<w:sdt><w:sdtPr><w:tag w:val="empty"/></w:sdtPr><w:sdtContent/></w:sdt>"#;
    let custom = format!(
        r#"<w:customXml w:element="item">{}{}</w:customXml>"#,
        text_paragraph("custom xml"),
        block_control("in custom", &text_paragraph("custom control"))
    );
    let after = block_control(
        "after",
        &(empty.to_owned() + &custom + &text_paragraph("after a")),
    );
    let outer = block_control(
        "outer",
        &(one_cell_table("outer cell")
            + &text_paragraph("outer a")
            + &nested
            + &control_in_cell
            + &sibling
            + &after
            + &text_paragraph("outer b")),
    );
    let body = text_paragraph("first")
        + &inline("body")
        + &one_cell_table("body cell")
        + &outer
        + &block_control("flat", &text_paragraph("flat a"))
        + &text_paragraph("last");
    let doc = doc_with_body(&body);
    let routes = story_paragraph_routes(&doc);
    let all = doc.paragraph_story_locations().unwrap();
    let texts = texts(&doc);
    assert_eq!(all.len(), texts.len());
    for (index, text) in texts.iter().enumerate() {
        let single = doc.paragraph_story_location(index).unwrap();
        assert_eq!(single, all[index], "{text}");
        if text.ends_with("cell") {
            // A table cell is a story of its own.
            assert_eq!(single, None, "{text}");
        } else {
            assert_eq!(single.as_ref(), Some(&routes[text]), "{text}");
        }
    }
}

#[test]
fn a_literal_replacement_at_a_paragraph_of_a_nested_control_changes_that_paragraph_only() {
    let inner = block_control(
        "inner",
        &(text_paragraph("same") + &text_paragraph("inner")),
    );
    let middle = block_control("middle", &(text_paragraph("same") + &inner));
    let outer = block_control("outer", &(text_paragraph("same") + &middle));
    let body = text_paragraph("same") + &outer + &text_paragraph("same");
    for (index, expected) in [
        (1, ["same", "new", "same", "same", "inner", "same"]),
        (2, ["same", "same", "new", "same", "inner", "same"]),
        (3, ["same", "same", "same", "new", "inner", "same"]),
    ] {
        let mut doc = doc_with_body(&body);
        let location = doc.paragraph_story_location(index).unwrap().unwrap();
        assert_eq!(
            doc.try_replace_text_at(&location, "same", "new", Some(1))
                .unwrap(),
            Ok(1)
        );
        assert_eq!(texts(&reopen(&mut doc)), expected, "paragraph {index}");
    }
}

#[test]
fn a_store_refused_after_its_reservations_leaves_the_document_as_it_was() {
    // An item whose properties relationship names a missing part: the new
    // store's part names and relationship id are reserved before the scan of
    // the store identities refuses.
    let mut package =
        OpcPackage::from_reader(Cursor::new(Document::new().to_bytes().unwrap())).unwrap();
    package.set_part("/customXml/item90.xml", b"<data/>".to_vec());
    package
        .get_or_create_part_rels("/customXml/item90.xml")
        .add_with_id(
            "rId1",
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXmlProps",
            "itemProps90.xml",
        );
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let mut doc = Document::from_bytes(&out.into_inner()).unwrap();
    let before = doc.to_bytes().unwrap();
    let error = doc
        .add_custom_xml_store(b"<data/>", &[])
        .unwrap_err()
        .to_string();
    assert!(error.contains("dangling custom XML edge"), "{error}");
    assert_eq!(doc.to_bytes().unwrap(), before);
}
