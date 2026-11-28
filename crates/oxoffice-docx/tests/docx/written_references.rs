//! References found by a walk of what the body writes: the blocks that
//! hold a note reference or a comment mark, and the styles content names.
//! Debug builds check each answer against the serialized main part.

use oxoffice_docx::{SlotHit, StoryKind};
use std::collections::HashSet;

use super::support::{doc_with_body, text_box_run};

const PLAIN: &str = r#"<w:p><w:r><w:t>plain</w:t></w:r></w:p>"#;

fn hit(slot: usize, raw: bool) -> SlotHit {
    SlotHit { slot, raw }
}

fn tracked(content: &str) -> String {
    format!(
        r#"<w:p><w:ins w:id="90" w:author="a" w:date="2024-01-01T00:00:00Z"><w:r>{content}</w:r></w:ins></w:p>"#
    )
}

#[test]
fn a_note_reference_is_found_typed_in_a_tracked_insertion_and_in_a_text_box() {
    let body = [
        r#"<w:p><w:r><w:footnoteReference w:id="2"/></w:r></w:p>"#.to_string(),
        PLAIN.to_string(),
        tracked(r#"<w:footnoteReference w:id="2"/>"#),
        format!(
            "<w:p>{}</w:p>",
            text_box_run(
                1,
                r#"<w:p><w:r><w:footnoteReference w:id="2"/></w:r></w:p>"#
            )
        ),
        r#"<w:p><w:r><w:footnoteReference w:id="3"/><w:endnoteReference w:id="2"/></w:r></w:p>"#
            .to_string(),
    ]
    .concat();
    let doc = doc_with_body(&body);
    assert_eq!(
        doc.body_slots_referencing_note(StoryKind::Footnote, 2),
        [hit(0, false), hit(2, true), hit(3, true)]
    );
    assert_eq!(
        doc.body_slots_referencing_note(StoryKind::Endnote, 2),
        [hit(4, false)]
    );
    assert!(
        doc.body_slots_referencing_note(StoryKind::Footnote, 7)
            .is_empty()
    );
}

#[test]
fn comment_marks_are_found_for_a_set_of_ids() {
    let body = [
        r#"<w:p><w:commentRangeStart w:id="5"/><w:r><w:t>a</w:t></w:r></w:p>"#.to_string(),
        PLAIN.to_string(),
        r#"<w:p><w:r><w:t>b</w:t></w:r><w:commentRangeEnd w:id="5"/><w:r><w:commentReference w:id="5"/></w:r></w:p>"#
            .to_string(),
        tracked(r#"<w:commentReference w:id="6"/>"#),
        r#"<w:p><w:commentRangeStart w:id="8"/><w:r><w:t>c</w:t></w:r><w:commentRangeEnd w:id="8"/></w:p>"#
            .to_string(),
    ]
    .concat();
    let doc = doc_with_body(&body);
    assert_eq!(
        doc.body_slots_referencing_comments(&HashSet::from([5, 6])),
        [hit(0, false), hit(2, false), hit(3, true)]
    );
    assert_eq!(
        doc.body_slots_referencing_comments(&HashSet::from([8])),
        [hit(4, false)]
    );
    assert!(
        doc.body_slots_referencing_comments(&HashSet::new())
            .is_empty()
    );
}
