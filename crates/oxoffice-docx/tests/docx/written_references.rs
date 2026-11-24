//! References found by a walk of what the body writes: the blocks that
//! hold a note reference or a comment mark, and the styles content names.
//! Debug builds check each answer against the serialized main part.

use std::collections::HashSet;

use oxoffice_docx::{SlotHit, StoryKind};

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

#[test]
fn a_style_named_in_a_text_box_or_a_tracked_change_is_kept() {
    for named in [
        format!(
            "<w:p>{}</w:p>",
            text_box_run(
                1,
                r#"<w:p><w:pPr><w:pStyle w:val="Probe"/></w:pPr><w:r><w:t>x</w:t></w:r></w:p>"#
            )
        ),
        tracked(r#"<w:rPr><w:rStyle w:val="Probe"/></w:rPr><w:t>x</w:t>"#),
        r#"<w:p><w:pPr><w:pPrChange w:id="91" w:author="a"><w:pPr><w:pStyle w:val="Probe"/></w:pPr></w:pPrChange></w:pPr><w:r><w:t>x</w:t></w:r></w:p>"#
            .to_string(),
        r#"<w:p><w:pPr><w:rPr><w:rStyle w:val="Probe"/></w:rPr></w:pPr><w:r><w:t>x</w:t></w:r></w:p>"#
            .to_string(),
    ] {
        let mut doc = doc_with_body(&named);
        doc.add_style(oxoffice_docx::StyleBuilder::paragraph("Probe", "Probe"))
            .expect("the style is added");
        let error = doc.remove_style("Probe").expect_err("content names the style");
        assert!(
            error.to_string().contains("referenced by document content"),
            "{named}: {error}"
        );
        assert!(doc.styles().iter().any(|style| style.style_id() == "Probe"), "{named}");
        // A style nothing names still goes.
        doc.add_style(oxoffice_docx::StyleBuilder::paragraph("Free", "Free"))
            .expect("the style is added");
        assert!(doc.remove_style("Free").expect("an unused style goes"));
    }
}

/// The steady cost of the slot queries and of the style search at 400 and
/// 4,000 paragraphs, for the pull request: `cargo test --profile scaling -p
/// oxoffice-docx --test docx slot_queries_cost -- --ignored --nocapture`.
#[test]
#[ignore = "a timing, run by hand on the scaling profile"]
fn slot_queries_cost() {
    for size in [400, 4_000] {
        let body = (0..size)
            .map(|index| {
                format!(
                    r#"<w:p><w:pPr><w:pStyle w:val="Normal"/></w:pPr><w:r><w:rPr><w:b/></w:rPr><w:t>paragraph {index}</w:t></w:r><w:r><w:t xml:space="preserve"> more text</w:t></w:r></w:p>"#
                )
            })
            .collect::<String>();
        let mut doc = doc_with_body(&body);
        let ids = HashSet::from([1]);
        let time = |doc: &mut oxoffice_docx::Document, call: &dyn Fn(&mut oxoffice_docx::Document)| {
            (0..7)
                .map(|_| {
                    let start = std::time::Instant::now();
                    call(doc);
                    start.elapsed().as_secs_f64() * 1e3
                })
                .fold(f64::INFINITY, f64::min)
        };
        let note = time(&mut doc, &|doc| {
            std::hint::black_box(doc.body_slots_referencing_note(StoryKind::Footnote, 2));
        });
        let comments = time(&mut doc, &|doc| {
            std::hint::black_box(doc.body_slots_referencing_comments(&ids));
        });
        let style = time(&mut doc, &|doc| {
            doc.add_style(oxoffice_docx::StyleBuilder::paragraph("Free", "Free"))
                .expect("the style is added");
            assert!(doc.remove_style("Free").expect("an unused style goes"));
        });
        println!(
            "{size} paragraphs: note {note:.3} ms, comments {comments:.3} ms, add+remove style {style:.3} ms"
        );
    }
}
