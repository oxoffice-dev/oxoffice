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

/// The tags of the corpus form, in document order.
const FORM_TAGS: [&str; 14] = [
    "company_name",
    "registered_address",
    "date_of_incorporation",
    "company_type",
    "country",
    "vat_registered",
    "iso_9001",
    "contact_person",
    "signatory",
    "signature_date",
    "signature",
    "supplier_reference",
    "category",
    "reviewed",
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

// ---- Inserting ----

#[test]
fn a_block_control_goes_before_the_item_at_its_location_or_at_the_end_of_the_story() {
    let mut doc = doc_with(&["Intro", "Outro"]);
    let end = ContentLocation::end(body(&doc));
    doc.insert_content_control(
        &end,
        ContentControlSpec {
            alias: Some("Nom du client".to_owned()),
            ..spec(Block, SdtType::RichText, "client")
        },
    )
    .unwrap();
    let outro = doc
        .paragraph_story_location(1)
        .unwrap()
        .expect("paragraph Outro");
    doc.insert_content_control(&outro, spec(Block, SdtType::PlainText, "summary"))
        .unwrap();
    let doc = reopen(&mut doc);

    // Each control holds one empty paragraph, a body paragraph of its own.
    assert_eq!(texts(&doc), ["Intro", "", "Outro", ""]);
    assert_eq!(
        listed(&doc),
        [
            (
                Some("summary".into()),
                Some(2),
                Some(SdtType::PlainText),
                "".into()
            ),
            (
                Some("client".into()),
                Some(1),
                Some(SdtType::RichText),
                "".into()
            ),
        ]
    );
    assert_eq!(
        doc.content_controls_by_tag("client")[0].alias(),
        Some("Nom du client")
    );
    assert_eq!(doc.content_controls_by_tag("summary")[0].alias(), None);
    // Nothing that was not asked for.
    let properties = saved_properties(&doc, "word/document.xml", 1);
    for element in [
        "w:lock",
        "w15:appearance",
        "w:showingPlcHdr",
        "w:dataBinding",
    ] {
        assert!(!has(&properties, element), "{element}: {properties}");
    }
}

#[test]
fn an_inline_control_holds_its_value_at_the_end_of_its_paragraph() {
    let mut doc = doc_with(&["Name: ", "Other"]);
    let paragraph = doc.paragraph_story_location(0).unwrap().unwrap();
    doc.insert_content_control(&paragraph, spec(Inline, SdtType::PlainText, "name"))
        .unwrap();
    assert_eq!(
        doc.set_content_control_value_by_tag("name", "Ada").unwrap(),
        1
    );
    let doc = reopen(&mut doc);

    assert_eq!(texts(&doc), ["Name: Ada", "Other"]);
    assert_eq!(
        listed(&doc),
        [(
            Some("name".into()),
            Some(1),
            Some(SdtType::PlainText),
            "Ada".into()
        )]
    );
    // The control is an item of its own, inside the first paragraph.
    let item = doc
        .story_items(&body(&doc))
        .unwrap()
        .into_iter()
        .find(|item| item.kind() == StoryItemKind::ContentControl)
        .expect("the control is a story item");
    assert_eq!(item.direct_body_index().unwrap(), Some(0));
    assert_eq!(item.text().unwrap().as_deref(), Some("Ada"));
}

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

// Fails today: the typed table counts its direct rows only, so a row inside a
// row control, as Word writes repeating table rows, is not one of its rows. Issue: #148
#[test]
#[should_panic(expected = "the row inside the control is a row of the table")]
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

// Fails today: the tag setters walk the body only and return 0 for a control in
// a header. Issue: #148
#[test]
#[should_panic(expected = "the tag setter fills the header control")]
fn a_value_set_by_tag_reaches_a_control_in_a_header() {
    let mut doc = doc_with_header_control();
    let count = doc
        .set_content_control_value_by_tag("title", "Annual report")
        .unwrap();
    let doc = reopen(&mut doc);
    let shown = item_text(&doc, &story(&doc, StoryKind::Header), "title");
    assert_eq!(
        (count, shown.as_str()),
        (1, "Annual report"),
        "the tag setter fills the header control"
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

#[test]
fn every_kind_is_typed_onto_a_shell_and_keeps_its_tag_alias_id_and_lock() {
    let items = || {
        vec![
            ("Red".to_owned(), "r".to_owned()),
            ("Blue".to_owned(), "b".to_owned()),
        ]
    };
    for (payload, kind) in [
        (ContentControlTypePayload::PlainText, SdtType::PlainText),
        (
            ContentControlTypePayload::Date {
                format: "dd/MM/yyyy".to_owned(),
            },
            SdtType::Date,
        ),
        (ContentControlTypePayload::CheckBox, SdtType::CheckBox),
        (
            ContentControlTypePayload::ComboBox { items: items() },
            SdtType::ComboBox,
        ),
        (
            ContentControlTypePayload::DropDownList { items: items() },
            SdtType::DropDownList,
        ),
        (ContentControlTypePayload::Group, SdtType::Group),
    ] {
        let mut doc = doc_with_control("typed");
        let location = control_at(&doc, &body(&doc), "typed");
        doc.set_content_control_lock(&location, Some(ContentControlLock::ContentLocked))
            .unwrap();
        let location = control_at(&doc, &body(&doc), "typed");
        doc.set_content_control_type(&location, payload).unwrap();
        let doc = reopen(&mut doc);
        let control = &doc.content_controls()[0];
        assert_eq!(control.control_type(), Some(kind));
        assert_eq!(
            (control.tag(), control.alias(), control.id()),
            (Some("typed"), Some("Typed"), Some(1)),
            "{kind:?}"
        );
        let properties = saved_properties(&doc, "word/document.xml", 1);
        assert_eq!(val(&properties, "w:lock").as_deref(), Some("contentLocked"));
        assert!(!has(&properties, "w:richText"), "{kind:?}: {properties}");
    }
}

#[test]
fn new_controls_take_the_smallest_id_no_control_uses() {
    let mut doc = doc_with_body(
        r#"<w:sdt><w:sdtPr><w:tag w:val="a"/><w:id w:val="1"/></w:sdtPr><w:sdtContent><w:p/></w:sdtContent></w:sdt><w:p><w:sdt><w:sdtPr><w:tag w:val="b"/><w:id w:val="3"/></w:sdtPr><w:sdtContent><w:r><w:t>b</w:t></w:r></w:sdtContent></w:sdt></w:p>"#,
    );
    for tag in ["c", "d"] {
        let end = ContentLocation::end(body(&doc));
        doc.insert_content_control(&end, spec(Block, SdtType::RichText, tag))
            .unwrap();
    }
    let doc = reopen(&mut doc);
    let ids: Vec<(Option<String>, Option<i32>)> = listed(&doc)
        .into_iter()
        .map(|(tag, id, _, _)| (tag, id))
        .collect();
    assert_eq!(
        ids,
        [
            (Some("a".into()), Some(1)),
            (Some("b".into()), Some(3)),
            (Some("c".into()), Some(2)),
            (Some("d".into()), Some(4)),
        ]
    );
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

#[test]
fn text_xml_cannot_carry_is_refused_and_markup_characters_round_trip() {
    let mut doc = doc_with_control("notes");
    let location = control_at(&doc, &body(&doc), "notes");
    let before = doc.to_bytes().unwrap();
    assert!(
        doc.set_content_control_tag(&location, Some("bad\u{1}"))
            .is_err()
    );
    assert!(
        doc.set_content_control_alias(&location, Some("bad\u{b}"))
            .is_err()
    );
    assert!(
        doc.set_content_control_value_by_tag("notes", "bad\u{0}")
            .is_err()
    );
    assert_eq!(doc.to_bytes().unwrap(), before);

    let tag = "étiquette & \"citée\" <1>";
    let value = "a < b & \"c\" — مرحبا 中文 😀 e\u{301}";
    doc.set_content_control_tag(&location, Some(tag)).unwrap();
    assert_eq!(doc.set_content_control_value_by_tag(tag, value).unwrap(), 1);
    let doc = reopen(&mut doc);
    assert_eq!(doc.content_controls_by_tag(tag).len(), 1);
    assert_eq!(doc.content_controls_by_tag(tag)[0].text(), value);
    assert_eq!(texts(&doc), ["Intro", value]);
}

// ---- Values ----

#[test]
fn a_text_value_replaces_the_content_and_ends_the_placeholder_state() {
    let long = "a long client name ".repeat(200);
    for given in ["Contoso & Co", "", long.as_str()] {
        let mut doc = doc_with_body(
            r#"<w:p><w:r><w:t xml:space="preserve">Client: </w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="client"/><w:id w:val="4"/><w:showingPlcHdr/><w:text/></w:sdtPr><w:sdtContent><w:r><w:t>Click to enter the client</w:t></w:r></w:sdtContent></w:sdt></w:p>"#,
        );
        assert_eq!(
            doc.set_content_control_value_by_tag("client", given)
                .unwrap(),
            1
        );
        let doc = reopen(&mut doc);
        assert_eq!(value(&doc, "client"), given);
        assert_eq!(texts(&doc), [format!("Client: {given}")]);
        let properties = saved_properties(&doc, "word/document.xml", 4);
        assert!(!has(&properties, "w:showingPlcHdr"), "{properties}");
    }
}

/// A paragraph holding a plain text control as Word writes one that shows
/// its placeholder text: the prompt in the Placeholder Text style, the run
/// formatting for the value in the control's own `w:rPr`.
const PROMPTING: &str = r#"<w:p><w:r><w:t xml:space="preserve">Name: </w:t></w:r><w:sdt><w:sdtPr><w:rPr><w:b/></w:rPr><w:alias w:val="Name"/><w:tag w:val="name"/><w:id w:val="42"/><w:showingPlcHdr/><w:text w:multiLine="1"/></w:sdtPr><w:sdtContent><w:r><w:rPr><w:rStyle w:val="PlaceholderText"/></w:rPr><w:t>Type your name</w:t></w:r></w:sdtContent></w:sdt></w:p>"#;

#[test]
fn a_value_set_on_a_prompt_drops_the_placeholder_text_style() {
    let mut doc = doc_with_body(PROMPTING);
    doc.set_content_control_value_by_tag("name", "Ada Lovelace")
        .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["Name: Ada Lovelace"]);
    let paragraph = doc.paragraph(0).unwrap();
    let styles: Vec<(String, Option<String>)> = paragraph
        .runs()
        .map(|run| (run.text(), run.style_id().map(str::to_owned)))
        .filter(|(text, _)| !text.is_empty())
        .collect();
    assert_eq!(
        styles,
        [
            ("Name: ".to_owned(), None),
            ("Ada Lovelace".to_owned(), None)
        ],
        "the value is not styled as placeholder text"
    );
}

#[test]
fn a_line_break_in_a_value_is_written_as_a_break() {
    let mut doc = doc_with_body(PROMPTING);
    doc.set_content_control_value_by_tag("name", "Ada Lovelace\nLondon")
        .unwrap();
    let doc = reopen(&mut doc);
    let (_, xml) = control_item(&doc, &body(&doc), "name");
    assert!(xml.contains("Ada Lovelace"), "{xml}");
    assert!(
        has(&xml, "w:br") && !xml.contains("Lovelace\nLondon"),
        "the line break is written as a w:br: {xml}"
    );
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

#[test]
fn a_checkbox_takes_true_false_one_zero_yes_no_and_shows_its_glyph() {
    let mut doc = typed_control("agree", ContentControlTypePayload::CheckBox);
    for (given, glyph, checked) in [
        ("yes", "\u{2612}", "1"),
        ("0", "\u{2610}", "0"),
        (" TRUE ", "\u{2612}", "1"),
        ("no", "\u{2610}", "0"),
    ] {
        assert_eq!(
            doc.set_content_control_value_by_tag("agree", given)
                .unwrap(),
            1
        );
        doc = reopen(&mut doc);
        assert_eq!(value(&doc, "agree"), glyph, "{given:?}");
        let properties = saved_properties(&doc, "word/document.xml", 1);
        assert_eq!(
            val(&properties, "w14:checked").as_deref(),
            Some(checked),
            "{given:?}"
        );
    }
    let before = doc.to_bytes().unwrap();
    let error = doc
        .set_content_control_value_by_tag("agree", "maybe")
        .unwrap_err();
    assert!(error.to_string().contains("true or false"), "{error}");
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn a_date_is_given_in_iso_form_and_shown_in_the_control_format() {
    for (format, shown) in [("yyyy-MM-dd", "2026-12-25"), ("dd/MM/yyyy", "25/12/2026")] {
        let mut doc = typed_control(
            "due",
            ContentControlTypePayload::Date {
                format: format.to_owned(),
            },
        );
        let before = doc.to_bytes().unwrap();
        for refused in ["2026-02-30", "25/12/2026", "next week", ""] {
            assert!(
                doc.set_content_control_value_by_tag("due", refused)
                    .is_err(),
                "{format}: {refused:?}"
            );
        }
        assert_eq!(doc.to_bytes().unwrap(), before, "{format}");

        assert_eq!(
            doc.set_content_control_value_by_tag("due", "2026-12-25")
                .unwrap(),
            1
        );
        let doc = reopen(&mut doc);
        assert_eq!(value(&doc, "due"), shown);
        let properties = saved_properties(&doc, "word/document.xml", 1);
        assert_eq!(
            attribute(&properties, "w:date", "w:fullDate").as_deref(),
            Some("2026-12-25T00:00:00Z")
        );
        assert_eq!(val(&properties, "w:dateFormat").as_deref(), Some(format));
    }
}

// Fails today: the long date format Word offers ("d MMMM yyyy", the corpus
// form's) is refused with "unsupported or missing date format". Issue: #149
#[test]
#[should_panic(expected = "a long date control takes a date")]
fn a_date_control_in_words_long_format_takes_a_date() {
    let mut doc = corpus_doc("authored/form.docx");
    let result = doc.set_content_control_value_by_tag("date_of_incorporation", "2015-01-02");
    assert!(
        result.is_ok(),
        "a long date control takes a date: {result:?}"
    );
    let doc = reopen(&mut doc);
    assert_eq!(value(&doc, "date_of_incorporation"), "2 January 2015");
}

#[test]
fn a_list_value_is_one_of_its_items_chosen_by_display_text_or_value() {
    let items = vec![
        ("France".to_owned(), "FR".to_owned()),
        ("Spain".to_owned(), "ES".to_owned()),
    ];
    let list_item = Regex::new(r#"<w:listItem w:displayText="([^"]*)" w:value="([^"]*)"/>"#)
        .expect("pattern compiles");
    for payload in [
        ContentControlTypePayload::DropDownList {
            items: items.clone(),
        },
        ContentControlTypePayload::ComboBox {
            items: items.clone(),
        },
    ] {
        let mut doc = typed_control("country", payload.clone());
        assert_eq!(
            doc.set_content_control_value_by_tag("country", "FR")
                .unwrap(),
            1
        );
        let mut doc = reopen(&mut doc);
        assert_eq!(value(&doc, "country"), "France", "{payload:?}");
        assert_eq!(
            doc.set_content_control_value_by_tag("country", "Spain")
                .unwrap(),
            1
        );
        let doc = reopen(&mut doc);
        assert_eq!(value(&doc, "country"), "Spain", "{payload:?}");
        // The items keep their order and their two texts.
        let properties = saved_properties(&doc, "word/document.xml", 1);
        let pairs: Vec<(String, String)> = list_item
            .captures_iter(&properties)
            .map(|captures| (captures[1].to_owned(), captures[2].to_owned()))
            .collect();
        assert_eq!(pairs, items, "{properties}");
    }

    let mut doc = typed_control("country", ContentControlTypePayload::DropDownList { items });
    let before = doc.to_bytes().unwrap();
    let error = doc
        .set_content_control_value_by_tag("country", "Belgium")
        .unwrap_err();
    assert!(
        error.to_string().contains("France") && error.to_string().contains("Spain"),
        "the refusal names the choices: {error}"
    );
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn a_combo_box_accepts_text_outside_its_items() {
    let mut doc = typed_control(
        "country",
        ContentControlTypePayload::ComboBox {
            items: vec![("France".to_owned(), "FR".to_owned())],
        },
    );
    let result = doc.set_content_control_value_by_tag("country", "Belgium");
    assert!(
        result.is_ok(),
        "a combo box takes text outside its items: {result:?}"
    );
    let doc = reopen(&mut doc);
    assert_eq!(value(&doc, "country"), "Belgium");
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
    // one. The section's own text() is not checked: it leaves nested controls
    // out (#147).
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
        ]
    );
    let header = story(&doc, StoryKind::Header);
    assert_eq!(item_text(&doc, &header, "block"), "Block text");
    let (_, copy) = control_item(&doc, &header, "block");
    assert!(copy.contains(r#"<w:id w:val="4"/>"#), "{copy}");
}

#[test]
fn unwrapping_keeps_the_content_and_removing_deletes_it() {
    let mut doc = doc_with(&["Name:", "Note"]);
    for (index, tag) in [(0, "kept"), (1, "dropped")] {
        let paragraph = doc.paragraph_story_location(index).unwrap().unwrap();
        doc.insert_content_control(&paragraph, spec(Inline, SdtType::PlainText, tag))
            .unwrap();
    }
    let end = ContentLocation::end(body(&doc));
    doc.insert_content_control(&end, spec(Block, SdtType::RichText, "clause"))
        .unwrap();
    let end = ContentLocation::end(body(&doc));
    doc.insert_content_control(&end, spec(Block, SdtType::RichText, "draft"))
        .unwrap();
    doc.fill_content_control_tags(&[
        ("kept", " Ada"),
        ("dropped", " (draft)"),
        ("clause", "Clause text"),
        ("draft", "Draft text"),
    ])
    .unwrap();

    for tag in ["kept", "clause"] {
        let location = control_at(&doc, &body(&doc), tag);
        doc.unwrap_content_control(&location).unwrap();
    }
    for tag in ["dropped", "draft"] {
        let location = control_at(&doc, &body(&doc), tag);
        doc.remove_content_control(&location).unwrap();
    }
    let doc = reopen(&mut doc);
    assert!(doc.content_controls().is_empty(), "{:?}", listed(&doc));
    assert_eq!(texts(&doc), ["Name: Ada", "Note", "Clause text"]);
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
fn the_corpus_forms_list_their_fourteen_controls_with_tags_aliases_and_kinds() {
    use SdtType::{CheckBox, ComboBox, Date, DropDownList, PlainText};
    for name in ["authored/form.docx", "libreoffice/form.docx"] {
        let doc = corpus_doc(name);
        let controls = doc.content_controls();
        let tags: Vec<&str> = controls.iter().filter_map(|c| c.tag()).collect();
        assert_eq!(tags, FORM_TAGS, "{name}");
        let ids: Vec<Option<i32>> = controls.iter().map(|c| c.id()).collect();
        let mut expected: Vec<Option<i32>> = (501_238_002..=501_238_014).map(Some).collect();
        expected.insert(1, Some(501_238_001));
        assert_eq!(ids, expected, "{name}");
        let kinds: Vec<Option<SdtType>> = controls.iter().map(|c| c.control_type()).collect();
        // The authored form gives the address no kind, which makes it rich
        // text; LibreOffice keeps its first line in a plain text control.
        let address = if name.starts_with("authored") {
            None
        } else {
            Some(PlainText)
        };
        assert_eq!(
            kinds,
            [
                Some(PlainText),
                address,
                Some(Date),
                Some(DropDownList),
                Some(ComboBox),
                Some(CheckBox),
                Some(CheckBox),
                Some(PlainText),
                Some(PlainText),
                Some(Date),
                Some(PlainText),
                Some(PlainText),
                Some(DropDownList),
                Some(CheckBox),
            ],
            "{name}"
        );
        let values: Vec<(&str, String)> = [
            "company_type",
            "country",
            "vat_registered",
            "iso_9001",
            "signature",
            "supplier_reference",
        ]
        .into_iter()
        .map(|tag| (tag, value(&doc, tag)))
        .collect();
        assert_eq!(
            values,
            [
                ("company_type", "Limited company".to_owned()),
                ("country", "United Kingdom".to_owned()),
                ("vat_registered", "\u{2612}".to_owned()),
                ("iso_9001", "\u{2610}".to_owned()),
                ("signature", "(sign here)".to_owned()),
                ("supplier_reference", "SUP-2026-0147".to_owned()),
            ],
            "{name}"
        );
        let signature = doc.content_controls_by_alias("Signature");
        assert_eq!(signature.len(), 1, "{name}");
        assert_eq!(signature[0].tag(), Some("signature"), "{name}");
        let reference = saved_properties(&doc, "word/document.xml", 501_238_012);
        assert_eq!(
            val(&reference, "w:lock").as_deref(),
            Some("contentLocked"),
            "{name}"
        );
    }
}

#[test]
fn the_text_of_a_control_of_several_paragraphs_reads_one_line_per_paragraph() {
    let doc = corpus_doc("authored/form.docx");
    let address = doc.content_controls_by_tag("registered_address");
    assert_eq!(address.len(), 1);
    assert!(address[0].text().starts_with("2 Canal Wharf"));
    assert_eq!(
        address[0].text(),
        "2 Canal Wharf\nLeeds LS11 5PS\nUnited Kingdom",
        "one line per paragraph"
    );
}

#[test]
fn a_value_set_on_a_control_of_several_paragraphs_leaves_one_paragraph() {
    let mut doc = corpus_doc("authored/form.docx");
    assert_eq!(
        doc.set_content_control_value_by_tag("registered_address", "1 New Road, Leeds")
            .unwrap(),
        1
    );
    let doc = reopen(&mut doc);
    assert_eq!(value(&doc, "registered_address"), "1 New Road, Leeds");
    let xml = doc.part_xml("word/document.xml").unwrap();
    let tag = xml
        .find(r#"<w:tag w:val="registered_address"/>"#)
        .expect("the address control");
    let content = &xml[tag..tag
        + xml[tag..]
            .find("</w:sdtContent>")
            .expect("its content ends")];
    let paragraphs = Regex::new(r"<w:p[\s>/]")
        .unwrap()
        .find_iter(content)
        .count();
    assert_eq!(
        paragraphs, 1,
        "the value is the only paragraph of the control: {content}"
    );
}

#[test]
fn filling_the_corpus_forms_by_tag_keeps_the_content_lock_and_writes_each_value_once() {
    for name in ["authored/form.docx", "libreoffice/form.docx"] {
        let mut doc = corpus_doc(name);
        assert_eq!(
            doc.fill_content_control_tags(&[
                ("company_name", "Fabrikam Ltd"),
                ("company_type", "Partnership"),
                ("vat_registered", "false"),
                ("signature", "Signed: O. Grant"),
                ("supplier_reference", "SUP-2026-0999"),
            ])
            .unwrap(),
            [1; 5],
            "{name}"
        );
        let doc = reopen(&mut doc);
        let values: Vec<String> = FORM_TAGS.iter().map(|tag| value(&doc, tag)).collect();
        assert_eq!(values[0], "Fabrikam Ltd", "{name}");
        assert_eq!(values[3], "Partnership", "{name}");
        assert_eq!(values[5], "\u{2610}", "{name}");
        assert_eq!(values[10], "Signed: O. Grant", "{name}");
        assert_eq!(values[11], "SUP-2026-0999", "{name}");
        assert_eq!(values[4], "United Kingdom", "{name}: the others stay");
        assert_eq!(values[7], "Oliver Grant, Chief Operating Officer", "{name}");

        let xml = doc.part_xml("word/document.xml").unwrap();
        for (old, new) in [
            ("Contoso Logistics Ltd", "Fabrikam Ltd"),
            ("SUP-2026-0147", "SUP-2026-0999"),
            ("(sign here)", "Signed: O. Grant"),
        ] {
            assert_eq!(xml.matches(old).count(), 0, "{name}: {old}");
            assert_eq!(xml.matches(new).count(), 1, "{name}: {new}");
        }
        let vat = saved_properties(&doc, "word/document.xml", 501_238_006);
        assert_eq!(val(&vat, "w14:checked").as_deref(), Some("0"), "{name}");
        // The program may fill a content-locked control; the lock stays for
        // the people who open the form in Word.
        let reference = saved_properties(&doc, "word/document.xml", 501_238_012);
        assert_eq!(
            val(&reference, "w:lock").as_deref(),
            Some("contentLocked"),
            "{name}"
        );
        assert_eq!(
            doc.document_protection().map(|protection| protection.mode),
            Some(oxoffice_docx::ProtectionMode::Forms),
            "{name}"
        );
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
fn a_form_reset_shows_the_prompts_and_the_required_fields_read_empty() {
    let mut doc = corpus_doc("authored/form.docx");
    let rules = form_rules();
    assert!(
        doc.validate_form(&rules).unwrap().is_empty(),
        "the corpus form is filled in"
    );
    assert_eq!(doc.reset_form(&rules).unwrap(), [1, 1, 1, 1]);
    let doc = reopen(&mut doc);

    assert_eq!(value(&doc, "company_name"), "Enter the company name");
    assert_eq!(value(&doc, "category"), "Choose a category");
    for id in [501_238_002, 501_238_013] {
        let properties = saved_properties(&doc, "word/document.xml", id);
        assert!(has(&properties, "w:showingPlcHdr"), "{properties}");
    }
    assert_eq!(
        bank_fields(&doc),
        [
            (
                Some("BankName".into()),
                LegacyFormFieldValue::Text(String::new())
            ),
            (
                Some("PaymentTerms".into()),
                LegacyFormFieldValue::SelectedIndex(0)
            ),
        ]
    );
    // A prompt is no value: the three required fields read empty.
    let empty: Vec<FormFieldTarget> = doc
        .validate_form(&rules)
        .unwrap()
        .into_iter()
        .map(|issue| issue.target)
        .collect();
    assert_eq!(
        empty,
        [target("company_name"), target("category"), legacy(0)]
    );
}

#[test]
fn the_thesis_table_of_contents_is_a_document_part_control_in_both_producers() {
    for (name, expected) in [("authored/thesis.docx", 1), ("libreoffice/thesis.docx", 2)] {
        let doc = corpus_doc(name);
        let controls = doc.content_controls();
        // LibreOffice also wraps the list of tables and figures in one.
        assert_eq!(controls.len(), expected, "{name}");
        assert!(
            controls
                .iter()
                .all(|c| c.control_type() == Some(SdtType::DocumentPartObject) && c.tag().is_none()),
            "{name}"
        );
        // The entries, without pinning how text() joins the control's
        // paragraphs (#147).
        let toc = controls[0].text();
        for entry in ["Contents", "Introduction\t1", "Literature review"] {
            assert!(toc.contains(entry), "{name}: {entry} in {toc}");
        }
        let xml = doc.part_xml("word/document.xml").unwrap();
        assert_eq!(
            val(&xml, "w:docPartGallery").as_deref(),
            Some("Table of Contents"),
            "{name}"
        );
    }
}

#[test]
fn a_control_added_to_word_and_google_files_survives_a_save() {
    for name in ["word/report.docx", "google/report.docx"] {
        let mut doc = corpus_doc(name);
        assert!(doc.content_controls().is_empty(), "{name}");
        let title = doc.paragraph_story_location(0).unwrap().unwrap();
        let before = texts(&doc);
        doc.insert_content_control(
            &title,
            ContentControlSpec {
                alias: Some("Status".to_owned()),
                ..spec(Inline, SdtType::PlainText, "status")
            },
        )
        .unwrap();
        doc.set_content_control_value_by_tag("status", " (draft)")
            .unwrap();
        let doc = reopen(&mut doc);
        assert_eq!(
            listed(&doc),
            [(
                Some("status".into()),
                Some(1),
                Some(SdtType::PlainText),
                " (draft)".into()
            )],
            "{name}"
        );
        let after = texts(&doc);
        assert_eq!(after[0], format!("{} (draft)", before[0]), "{name}");
        assert_eq!(after[1..], before[1..], "{name}: the rest is untouched");
    }
}
