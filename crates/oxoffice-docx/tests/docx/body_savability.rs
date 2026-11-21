//! Typed inserts next to the nested namespace owners a body keeps from its
//! source: the body still saves and reads back.

use crate::support::doc_with_body;

/// A content control that declares `w15` and uses it, which the root of a new
/// document does not declare: a nested namespace owner.
const CONTROL_OWNER: &str = concat!(
    r#"<w:sdt xmlns:w15="http://schemas.microsoft.com/office/word/2012/wordml"><w:sdtPr>"#,
    r#"<w15:repeatingSection/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>row</w:t></w:r></w:p>"#,
    r#"</w:sdtContent></w:sdt>"#
);

#[test]
fn typed_inserts_next_to_a_retained_owner_save_and_read_back() {
    let mut doc = doc_with_body(CONTROL_OWNER);
    assert_eq!(doc.unsavable_body_edit(), None);
    for text in ["one", "two", "three"] {
        let slot = doc.content_count();
        doc.add_paragraph(text);
        doc.keep_body_savable_after_typed_insert(slot, true);
        assert_eq!(doc.unsavable_body_edit(), None);
    }
    let slot = doc.content_count();
    doc.add_table(1, 1);
    doc.keep_body_savable_after_typed_insert(slot, true);
    let saved = doc.to_bytes().expect("the body saves");
    let reopened = oxoffice_docx::Document::from_bytes(&saved).expect("the saved body opens");
    assert_eq!(reopened.content_count(), 5);
}
