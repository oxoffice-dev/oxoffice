//! Direct body blocks removed in place, held so that a caller whose own
//! publication refuses can put them back.

use oxoffice_docx::{Document, Length};

/// A one-pixel PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

/// An opened document of paragraphs, a table, a picture paragraph, a
/// paragraph with a footnote reference and one that ends a section.
fn body() -> Document {
    let mut doc = Document::new();
    doc.add_paragraph("first");
    doc.add_paragraph("second");
    doc.add_table(2, 2);
    doc.add_paragraph("third");
    doc.add_picture(PNG, "dot.png", Length::cm(1.0), Length::cm(1.0));
    doc.add_paragraph("noted");
    let note = doc.add_footnote("A note");
    let noted = doc.paragraph_count() - 1;
    doc.paragraph_mut(noted).unwrap().add_footnote_ref(note);
    doc.add_paragraph("last");
    Document::from_bytes(&doc.to_bytes().unwrap()).unwrap()
}

fn texts(doc: &Document) -> Vec<String> {
    (0..doc.paragraph_count())
        .map(|index| doc.paragraph(index).unwrap().text())
        .collect()
}

#[test]
fn a_removal_in_place_saves_what_the_staged_removal_saves() {
    for (start, end) in [(0, 1), (1, 3), (3, 5)] {
        let mut staged = body();
        let mut local = body();
        let count = staged.remove_content_range(start, end).unwrap();
        let removed = local
            .remove_body_blocks_in_place(start, end)
            .unwrap()
            .expect("plain blocks go in place");
        assert_eq!(removed.blocks().len(), count);
        assert_eq!(removed.slot(), start);
        assert_eq!(texts(&local), texts(&staged));
        assert_eq!(
            local.to_bytes().unwrap(),
            staged.to_bytes().unwrap(),
            "{start}..{end}"
        );
    }
}

#[test]
fn a_restored_removal_saves_the_bytes_of_before() {
    let mut doc = body();
    let before = doc.to_bytes().unwrap();
    let paragraphs = texts(&doc);
    // The picture paragraph: its relationship is freed, then taken back.
    let removed = doc.remove_body_blocks_in_place(3, 5).unwrap().unwrap();
    assert_ne!(doc.to_bytes().unwrap(), before);
    doc.restore_body_blocks(removed).unwrap();
    assert_eq!(texts(&doc), paragraphs);
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn blocks_the_staged_route_reconciles_are_not_removed_in_place() {
    let mut doc = body();
    let noted = doc.content_count() - 2;
    let before = doc.to_bytes().unwrap();
    // A note reference, a range out of the body, an empty range.
    for (start, end) in [(noted, noted + 1), (0, doc.content_count() + 1), (2, 2)] {
        assert!(
            doc.remove_body_blocks_in_place(start, end)
                .unwrap()
                .is_none()
        );
    }
    assert!(doc.pop_body_block_in_place(noted).unwrap().is_none());
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn a_comment_range_keeps_the_staged_route() {
    let mut doc = Document::new();
    doc.add_paragraph("plain");
    doc.add_paragraph("commented");
    let range = oxoffice_docx::RunRange {
        start: oxoffice_docx::RunPosition {
            body_index: 1,
            run_index: 0,
        },
        end: oxoffice_docx::RunPosition {
            body_index: 1,
            run_index: 0,
        },
    };
    doc.add_comment(range, "Reviewer", Some("RV"), "a comment")
        .unwrap();
    let mut doc = Document::from_bytes(&doc.to_bytes().unwrap()).unwrap();
    let before = doc.to_bytes().unwrap();
    assert!(doc.remove_body_blocks_in_place(1, 2).unwrap().is_none());
    assert!(doc.pop_body_block_in_place(1).unwrap().is_none());
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn a_block_popped_in_place_is_the_fragment_the_staged_route_takes() {
    for slot in 0..5 {
        let mut staged = body();
        let location = staged.direct_body_item_location(slot).unwrap().unwrap();
        let expected = staged.remove_content_at(&location).unwrap();
        let mut local = body();
        let (fragment, removed) = local.pop_body_block_in_place(slot).unwrap().unwrap();
        assert_eq!(removed.slot(), slot);
        assert!(
            fragment.same_taken_fragment(&expected),
            "slot {slot}:\n{fragment:?}\n{expected:?}"
        );
        assert_eq!(local.to_bytes().unwrap(), staged.to_bytes().unwrap());
    }
}
