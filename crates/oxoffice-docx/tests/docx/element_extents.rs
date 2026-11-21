//! `Document::element_extent` tells what an edit of one paragraph or one table
//! cell alone can renumber: the story items inside the element, where each
//! hyperlink of its story lies against it, and the text box and table cell
//! stories of its part before and inside it. `resolved_xml_events` compares
//! two elements by the namespaces their names resolve to, wherever their
//! declarations sit.

use std::cmp::Ordering::{Equal, Greater, Less};

use oxoffice_docx::{Document, StoryId, StoryKind, resolved_xml_events};

use crate::support::doc_with_body;

fn run(text: &str) -> String {
    format!(r#"<w:r><w:t xml:space="preserve">{text}</w:t></w:r>"#)
}

fn link(anchor: &str) -> String {
    format!(
        r#"<w:hyperlink w:anchor="{anchor}">{}</w:hyperlink>"#,
        run(anchor)
    )
}

fn paragraph(content: &str) -> String {
    format!("<w:p>{content}</w:p>")
}

fn text_box(text: &str) -> String {
    format!(
        r#"<w:r><w:drawing><wp:anchor distT="0" distB="0" distL="0" distR="0" simplePos="0" relativeHeight="1" behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="column"><wp:posOffset>0</wp:posOffset></wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset></wp:positionV><wp:extent cx="914400" cy="457200"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:wrapNone/><wp:docPr id="7" name="Text Box"/><wp:cNvGraphicFramePr/><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:wsp xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:cNvSpPr txBox="1"/><wps:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="914400" cy="457200"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></wps:spPr><wps:txbx><w:txbxContent>{}</w:txbxContent></wps:txbx><wps:bodyPr/></wps:wsp></a:graphicData></a:graphic></wp:anchor></w:drawing></w:r>"#,
        paragraph(&run(text))
    )
}

/// A body of five blocks: a paragraph with a link; the edited paragraph with a
/// field, a link and a text box; a block content control of three linked
/// paragraphs; a table whose cell holds a link; a last linked paragraph.
fn document() -> Document {
    let field = format!(r#"<w:fldSimple w:instr="PAGE">{}</w:fldSimple>"#, run("1"));
    let body = [
        paragraph(&format!("{}{}", run("before"), link("a"))),
        paragraph(&format!(
            "{}{field}{}{}",
            run("edited"),
            link("b"),
            text_box("in box")
        )),
        format!(
            "<w:sdt><w:sdtPr/><w:sdtContent>{}{}{}</w:sdtContent></w:sdt>",
            paragraph(&link("c")),
            paragraph(&link("d")),
            paragraph(&link("e"))
        ),
        format!(
            r#"<w:tbl><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc>{}</w:tc></w:tr></w:tbl>"#,
            paragraph(&link("f"))
        ),
        paragraph(&format!("{}{}", run("after"), link("g"))),
    ]
    .concat();
    doc_with_body(&body)
}

fn story(doc: &Document, kind: StoryKind, owner_index: usize) -> StoryId {
    doc.stories()
        .unwrap()
        .into_iter()
        .find(|story| story.kind() == kind && story.owner_index() == owner_index)
        .unwrap_or_else(|| panic!("no {kind:?} story {owner_index}"))
}

#[test]
fn a_paragraph_extent_holds_its_items_its_links_and_its_text_box() {
    let doc = document();
    let body = story(&doc, StoryKind::Body, 0);
    let extent = doc.element_extent(&body, Some(&[1])).unwrap().unwrap();
    // The paragraph is item 1, its field and its drawing items 2 and 3.
    assert_eq!(extent.items, (1, 3));
    let control = 4;
    let after = 6;
    assert_eq!(
        extent.links,
        vec![
            (vec![0], 0, Less),
            (vec![1], 0, Equal),
            (vec![control], 0, Greater),
            (vec![control], 1, Greater),
            (vec![control], 2, Greater),
            (vec![after], 0, Greater),
        ]
    );
    assert_eq!(extent.text_boxes, (0, 1));
    assert_eq!(extent.cell_stories, (0, 0));
    // The bytes are closed over their namespaces, so they read alone.
    let xml = String::from_utf8(extent.xml).unwrap();
    assert!(xml.starts_with("<w:p") && xml.contains("in box"), "{xml}");
    assert!(xml.contains(r#"xmlns:w=""#), "{xml}");
    resolved_xml_events(xml.as_bytes()).unwrap();
}

#[test]
fn a_paragraph_of_a_block_control_shares_the_links_of_the_control() {
    let doc = document();
    let body = story(&doc, StoryKind::Body, 0);
    let control = 4;
    let extent = doc
        .element_extent(&body, Some(&[control, 1]))
        .unwrap()
        .unwrap();
    // It holds no item: the next one would come after the control's.
    assert_eq!(extent.items, (control + 1, 0));
    assert_eq!(
        extent.links,
        vec![
            (vec![0], 0, Less),
            (vec![1], 0, Less),
            (vec![control], 0, Less),
            (vec![control], 1, Equal),
            (vec![control], 2, Greater),
            (vec![6], 0, Greater),
        ]
    );
    assert_eq!(extent.text_boxes, (1, 0));
}

#[test]
fn a_cell_extent_is_its_story_with_what_it_holds() {
    let doc = document();
    let cell = story(&doc, StoryKind::TableCell, 0);
    let extent = doc.element_extent(&cell, None).unwrap().unwrap();
    assert_eq!(extent.items, (0, 1));
    assert_eq!(extent.links, vec![(vec![0], 0, Equal)]);
    assert_eq!(extent.text_boxes, (1, 0));
    // The cell's own story counts as neither before nor inside.
    assert_eq!(extent.cell_stories, (0, 0));
}

#[test]
fn a_paragraph_in_a_cell_counts_its_own_cell_story_before_it() {
    let doc = document();
    let cell = story(&doc, StoryKind::TableCell, 0);
    let extent = doc.element_extent(&cell, Some(&[0])).unwrap().unwrap();
    assert_eq!(extent.items, (0, 1));
    assert_eq!(extent.cell_stories, (1, 0));
    assert_eq!(extent.text_boxes, (1, 0));
}

#[test]
fn a_block_read_gives_the_extent_of_the_whole_part_for_its_block() {
    let doc = document();
    let body = story(&doc, StoryKind::Body, 0);
    let whole = doc.element_extent(&body, Some(&[1])).unwrap().unwrap();
    // Block 1 is the edited paragraph; block 0 holds one item and no story.
    let block = doc
        .block_element_extent(1, (1, 0, 0), &body, Some(&[1]))
        .unwrap()
        .unwrap();
    assert_eq!(block.read, 1..4);
    assert_eq!(
        (
            &block.xml,
            block.items,
            block.text_boxes,
            block.cell_stories
        ),
        (
            &whole.xml,
            whole.items,
            whole.text_boxes,
            whole.cell_stories
        )
    );
    assert_eq!(block.links, vec![(vec![1], 0, Equal)]);
    // The cell of the table in block 3, after one text box and no cell story.
    let cell = story(&doc, StoryKind::TableCell, 0);
    let control = 4;
    let in_block = doc
        .block_element_extent(3, (control + 1, 1, 0), &cell, Some(&[0]))
        .unwrap()
        .unwrap();
    let in_part = doc.element_extent(&cell, Some(&[0])).unwrap().unwrap();
    assert_eq!(in_block, in_part);
    // Not in that block.
    assert_eq!(
        doc.block_element_extent(0, (0, 0, 0), &body, Some(&[1]))
            .unwrap(),
        None
    );
}

#[test]
fn an_element_the_part_does_not_hold_has_no_extent() {
    let doc = document();
    let body = story(&doc, StoryKind::Body, 0);
    assert_eq!(doc.element_extent(&body, Some(&[2])).unwrap(), None);
    let missing = StoryId::new(StoryKind::TableCell, body.part_name(), 9);
    assert_eq!(doc.element_extent(&missing, None).unwrap(), None);
}

const A: &str = "urn:a";
const B: &str = "urn:b";

#[test]
fn moved_namespace_declarations_leave_the_resolved_events_equal() {
    let on_root = format!(r#"<x:p xmlns:x="{A}" xmlns:y="{B}"><x:r y:id="1">t</x:r></x:p>"#);
    let on_child = format!(r#"<x:p xmlns:x="{A}"><x:r xmlns:y="{B}" y:id="1">t</x:r></x:p>"#);
    let other_prefix = format!(r#"<z:p xmlns:z="{A}" xmlns:y="{B}"><z:r y:id="1">t</z:r></z:p>"#);
    let events = resolved_xml_events(on_root.as_bytes()).unwrap();
    assert_eq!(resolved_xml_events(on_child.as_bytes()).unwrap(), events);
    assert_eq!(
        resolved_xml_events(other_prefix.as_bytes()).unwrap(),
        events
    );
}

#[test]
fn a_prefix_bound_to_another_namespace_changes_the_resolved_events() {
    let kept = format!(r#"<x:p xmlns:x="{A}" xmlns:y="{B}"><x:r y:id="1"/></x:p>"#);
    // The element name, then the attribute name, now resolve elsewhere.
    let element =
        format!(r#"<x:p xmlns:x="{A}" xmlns:y="{B}"><x:r xmlns:x="{B}" y:id="1"/></x:p>"#);
    let attribute =
        format!(r#"<x:p xmlns:x="{A}" xmlns:y="{B}"><x:r xmlns:y="{A}" y:id="1"/></x:p>"#);
    let events = resolved_xml_events(kept.as_bytes()).unwrap();
    assert_ne!(resolved_xml_events(element.as_bytes()).unwrap(), events);
    assert_ne!(resolved_xml_events(attribute.as_bytes()).unwrap(), events);
    assert!(
        resolved_xml_events(b"<x:p/>").is_err(),
        "an undeclared prefix"
    );
}
