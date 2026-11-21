//! `Document::add_comment_on_text` anchors a comment on a chosen match of a
//! text, in the whole main story or in its table cells only, checks the
//! anchor before it publishes, and carries the initials and date asked for,
//! on the Word and Google Docs files of the corpus.

use std::io::Cursor;
use std::path::Path;

use oxoffice_docx::{CommentDate, Document, StoryKind, TextCommentOptions};
use oxoffice_opc::OpcPackage;

const PRODUCERS: [&str; 2] = ["word", "google"];
const STAMP: &str = "2026-01-02T03:04:05Z";

fn corpus_bytes(producer: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/files")
        .join(producer)
        .join("report.docx");
    std::fs::read(path).expect("corpus report is readable")
}

/// The corpus report of `producer` with `xml` before the last `w:sectPr`.
fn body_with(producer: &str, xml: &str) -> Document {
    let mut package = OpcPackage::from_reader(Cursor::new(corpus_bytes(producer))).unwrap();
    let main = String::from_utf8(package.get_part("/word/document.xml").unwrap().to_vec()).unwrap();
    let at = main.rfind("<w:sectPr").unwrap();
    package.set_part(
        "/word/document.xml",
        format!("{}{xml}{}", &main[..at], &main[at..]).into_bytes(),
    );
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    Document::from_bytes(&bytes.into_inner()).unwrap()
}

fn options(occurrence: usize, tables_only: bool) -> TextCommentOptions<'static> {
    TextCommentOptions {
        occurrence,
        tables_only,
        date: CommentDate::At(STAMP),
        ..Default::default()
    }
}

/// The story kind and text the comment `id` is anchored on.
fn anchored(doc: &Document, id: i32) -> (StoryKind, String) {
    let comment = doc
        .comments()
        .into_iter()
        .find(|comment| comment.id() == id)
        .expect("the comment");
    let range = comment.anchor().unwrap().expect("an anchor");
    (
        range.start.location.story().kind(),
        comment.anchor_text().unwrap().expect("an anchored text"),
    )
}

#[test]
fn tables_only_counts_the_matches_of_table_cells_only() {
    // "Churn" opens a body paragraph, then heads a column of the appendix table.
    for producer in PRODUCERS {
        let mut doc = Document::from_bytes(&corpus_bytes(producer)).unwrap();
        let anywhere = doc
            .add_comment_on_text("Churn", "Reviewer", "c", options(0, false))
            .unwrap();
        assert_eq!(
            anchored(&doc, anywhere),
            (StoryKind::Body, "Churn".to_owned()),
            "{producer}"
        );
        let in_cell = doc
            .add_comment_on_text("Churn", "Reviewer", "c", options(0, true))
            .unwrap();
        assert_eq!(
            anchored(&doc, in_cell),
            (StoryKind::TableCell, "Churn".to_owned()),
            "{producer}"
        );
    }
}

#[test]
fn tables_only_reaches_a_nested_table() {
    let nested = r#"<w:p><w:r><w:t>ACME first</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>Outer cell</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>Nested ACME</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p/></w:tc></w:tr></w:tbl><w:p><w:r><w:t>ACME last</w:t></w:r></w:p>"#;
    for producer in PRODUCERS {
        let mut doc = body_with(producer, nested);
        let id = doc
            .add_comment_on_text("ACME", "Reviewer", "c", options(0, true))
            .unwrap();
        assert_eq!(
            anchored(&doc, id),
            (StoryKind::TableCell, "ACME".to_owned()),
            "{producer}"
        );
        assert!(
            doc.add_comment_on_text("ACME", "Reviewer", "c", options(1, true))
                .is_err(),
            "{producer}: one match in table cells"
        );
    }
}

#[test]
fn a_missing_occurrence_names_where_it_counted_and_changes_nothing() {
    for producer in PRODUCERS {
        let mut doc = Document::from_bytes(&corpus_bytes(producer)).unwrap();
        let before = doc.to_bytes().unwrap();
        let error = doc
            .add_comment_on_text("Churn", "Reviewer", "c", options(1, true))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("occurs 1 time in the table cells of the main story"),
            "{producer}: {error}"
        );
        let error = doc
            .add_comment_on_text("Churn", "Reviewer", "c", options(5, false))
            .unwrap_err()
            .to_string();
        assert!(error.contains("in the main story"), "{producer}: {error}");
        assert_eq!(doc.to_bytes().unwrap(), before, "{producer}");
    }
}

#[test]
fn a_match_that_cannot_be_anchored_exactly_is_refused_before_anything_changes() {
    // The simple field shows its result between the two runs of the match.
    let field = r#"<w:p><w:r><w:t>AC</w:t></w:r><w:fldSimple w:instr=" AUTHOR "><w:r><w:t>x</w:t></w:r></w:fldSimple><w:r><w:t>ME end</w:t></w:r></w:p>"#;
    for producer in PRODUCERS {
        let mut doc = body_with(producer, field);
        let before = doc.to_bytes().unwrap();
        let error = doc
            .add_comment_on_text("ACME", "Reviewer", "c", options(0, false))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("cannot be anchored exactly"),
            "{producer}: {error}"
        );
        assert_eq!(doc.to_bytes().unwrap(), before, "{producer}");
    }
}

#[test]
fn the_comment_carries_the_initials_and_date_asked_for() {
    for producer in PRODUCERS {
        let mut doc = Document::from_bytes(&corpus_bytes(producer)).unwrap();
        let id = doc
            .add_comment_on_text(
                "Churn",
                "Ann Reviewer",
                "Check this",
                TextCommentOptions {
                    initials: Some("AR"),
                    date: CommentDate::At(STAMP),
                    ..Default::default()
                },
            )
            .unwrap();
        let saved = Document::from_bytes(&doc.to_bytes().unwrap()).unwrap();
        let comment = saved
            .comments()
            .into_iter()
            .find(|comment| comment.id() == id)
            .unwrap();
        assert_eq!(comment.author(), Some("Ann Reviewer"), "{producer}");
        assert_eq!(comment.initials(), Some("AR"), "{producer}");
        assert_eq!(comment.date(), Some(STAMP), "{producer}");
        assert_eq!(comment.text(), "Check this", "{producer}");
    }
}

#[test]
fn tables_only_reaches_a_table_in_a_block_control_and_a_row_in_a_row_control() {
    let controlled = r#"<w:p><w:r><w:t>ACME first</w:t></w:r></w:p><w:sdt><w:sdtContent><w:p><w:r><w:t>ACME in a controlled paragraph</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>ACME in a controlled table</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:sdtContent></w:sdt><w:p/><w:tbl><w:sdt><w:sdtContent><w:tr><w:tc><w:p><w:r><w:t>ACME in a controlled row</w:t></w:r></w:p></w:tc></w:tr></w:sdtContent></w:sdt></w:tbl><w:p/>"#;
    for producer in PRODUCERS {
        let mut doc = body_with(producer, controlled);
        for (occurrence, paragraph) in [
            (0, "ACME in a controlled table"),
            (1, "ACME in a controlled row"),
        ] {
            let id = doc
                .add_comment_on_text("ACME", "Reviewer", "c", options(occurrence, true))
                .unwrap_or_else(|error| panic!("{producer} {occurrence}: {error}"));
            let comment = doc
                .comments()
                .into_iter()
                .find(|comment| comment.id() == id)
                .unwrap();
            let range = comment.anchor().unwrap().unwrap();
            assert_eq!(
                range.start.location.story().kind(),
                StoryKind::TableCell,
                "{producer} {occurrence}"
            );
            let cell_text = doc
                .story_items(range.start.location.story())
                .unwrap()
                .into_iter()
                .filter_map(|item| item.text().unwrap())
                .collect::<String>();
            assert_eq!(cell_text, paragraph, "{producer} {occurrence}");
        }
    }
}
