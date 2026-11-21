//! Whether a typed edit of a body can be saved, and how that answer is kept
//! after a typed insert instead of being computed again over the whole body.

use crate::support::doc_with_body;

/// A hyperlink that declares the relationships namespace itself: a nested
/// namespace owner the body keeps from its source.
const HYPERLINK_OWNER: &str = concat!(
    r#"<w:p><w:hyperlink xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" "#,
    r#"r:id="rId5"><w:r><w:t>site</w:t></w:r></w:hyperlink></w:p>"#
);

#[test]
fn a_typed_insert_with_nothing_named_like_an_owner_keeps_the_body_savable() {
    let mut doc = doc_with_body(HYPERLINK_OWNER);
    assert_eq!(doc.unsavable_body_edit(), None);
    for text in ["one", "two", "three"] {
        let slot = doc.content_count();
        doc.add_paragraph(text);
        doc.keep_body_savable_after_typed_insert(slot, true);
        assert_eq!(doc.unsavable_body_edit(), None);
    }
    let saved = doc.to_bytes().expect("the body saves");
    let reopened = oxoffice_docx::Document::from_bytes(&saved).expect("the saved body opens");
    assert_eq!(reopened.content_count(), 4);
}

#[test]
fn a_typed_insert_holding_an_element_named_like_an_owner_gets_the_full_answer() {
    // The typed hyperlink is named like the retained owner, so the answer is
    // computed again over the whole body rather than kept; the owner is still
    // told apart from it, and the body stays savable.
    let mut doc = doc_with_body(HYPERLINK_OWNER);
    assert_eq!(doc.unsavable_body_edit(), None);
    let slot = doc.content_count();
    doc.add_paragraph("").add_hyperlink("site", "rId5");
    doc.keep_body_savable_after_typed_insert(slot, true);
    assert_eq!(doc.unsavable_body_edit(), None);
    doc.to_bytes().expect("the body saves");
}

#[test]
fn a_body_that_could_not_be_saved_before_the_insert_is_not_marked_savable() {
    let mut doc = doc_with_body(HYPERLINK_OWNER);
    let slot = doc.content_count();
    doc.add_paragraph("plain");
    doc.keep_body_savable_after_typed_insert(slot, false);
    assert_eq!(doc.unsavable_body_edit(), None, "computed again, not kept");
}
