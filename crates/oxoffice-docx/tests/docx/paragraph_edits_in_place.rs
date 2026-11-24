//! A direct body paragraph edited on a copy of itself, swapped in on success.

use oxoffice_docx::{Document, Error};

use crate::support::corpus_root;

fn word_report() -> Document {
    let path = corpus_root().join("files/word/report.docx");
    Document::open(&path).expect("the Word report opens")
}

/// The first paragraph of the Word report that is a direct body child and
/// holds text.
fn direct_paragraph(document: &Document) -> usize {
    (0..document.paragraph_count())
        .find(|index| {
            document.content_index_of_paragraph(*index).is_some()
                && !document
                    .paragraph(*index)
                    .expect("paragraph")
                    .text()
                    .is_empty()
        })
        .expect("the report has a direct body paragraph with text")
}

#[test]
fn an_edit_in_place_replaces_the_paragraph_and_returns_the_old_one() {
    let mut document = word_report();
    let index = direct_paragraph(&document);
    let before = document.paragraph(index).expect("paragraph").text();
    let (value, old) = document
        .edit_body_paragraph_in_place(index, |paragraph| {
            paragraph.set_text("changed")?;
            Ok(7)
        })
        .expect("edits")
        .expect("a direct body paragraph");
    assert_eq!(value, 7);
    assert_eq!(
        document.paragraph(index).expect("paragraph").text(),
        "changed"
    );
    document
        .restore_body_paragraph(index, old)
        .expect("restores");
    assert_eq!(document.paragraph(index).expect("paragraph").text(), before);
}

#[test]
fn a_refused_edit_in_place_leaves_the_saved_bytes_unchanged() {
    let mut document = word_report();
    let index = direct_paragraph(&document);
    let before = document.to_bytes().expect("saves");
    let refused = document.edit_body_paragraph_in_place(index, |paragraph| {
        paragraph.add_run("half done");
        Err::<(), _>(Error::Other("refused".to_owned()))
    });
    assert!(refused.is_err());
    assert_eq!(document.to_bytes().expect("saves"), before);
}

#[test]
fn a_panicking_edit_in_place_leaves_the_saved_bytes_unchanged() {
    let mut document = word_report();
    let index = direct_paragraph(&document);
    let before = document.to_bytes().expect("saves");
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        document.edit_body_paragraph_in_place(index, |paragraph| -> oxoffice_docx::Result<()> {
            paragraph.add_run("half done");
            panic!("an edit that panics");
        })
    }));
    assert!(panicked.is_err());
    assert_eq!(document.to_bytes().expect("saves"), before);
}

#[test]
fn a_restored_paragraph_saves_the_bytes_of_before_the_edit() {
    let mut document = word_report();
    let index = direct_paragraph(&document);
    let before = document.to_bytes().expect("saves");
    let (_, old) = document
        .edit_body_paragraph_in_place(index, |paragraph| paragraph.set_text("undone"))
        .expect("edits")
        .expect("a direct body paragraph");
    assert_ne!(document.to_bytes().expect("saves"), before);
    document
        .restore_body_paragraph(index, old)
        .expect("restores");
    assert_eq!(document.to_bytes().expect("saves"), before);
}

#[test]
fn a_save_after_an_edit_in_place_writes_the_new_paragraph() {
    let mut document = word_report();
    let index = direct_paragraph(&document);
    // Reads that fill the save and layout caches before the edit.
    let _ = document.to_bytes().expect("saves");
    let _ = document.text();
    document
        .edit_body_paragraph_in_place(index, |paragraph| paragraph.set_text("rewritten"))
        .expect("edits")
        .expect("a direct body paragraph");
    assert!(document.text().contains("rewritten"));
    let reopened = Document::from_bytes(&document.to_bytes().expect("saves")).expect("reopens");
    assert_eq!(
        reopened.paragraph(index).expect("paragraph").text(),
        "rewritten"
    );
    assert_eq!(reopened.paragraph_count(), document.paragraph_count());
    for other in (0..document.paragraph_count()).filter(|other| *other != index) {
        assert_eq!(
            reopened.paragraph(other).expect("paragraph").text(),
            document.paragraph(other).expect("paragraph").text()
        );
    }
}

#[test]
fn a_paragraph_that_is_not_a_direct_body_child_is_not_edited_in_place() {
    let path = corpus_root().join("files/word/record.docx");
    let mut document = Document::open(&path).expect("the Word record opens");
    let index = (0..document.paragraph_count())
        .find(|index| document.content_index_of_paragraph(*index).is_none())
        .expect("a paragraph inside the control");
    let before = document.to_bytes().expect("saves");
    let mut ran = false;
    let outcome = document
        .edit_body_paragraph_in_place(index, |paragraph| {
            ran = true;
            paragraph.set_text("never")
        })
        .expect("no error");
    assert!(outcome.is_none());
    assert!(!ran);
    assert_eq!(document.to_bytes().expect("saves"), before);
}

#[test]
fn an_index_out_of_range_is_an_error() {
    let mut document = word_report();
    let count = document.paragraph_count();
    assert!(
        document
            .edit_body_paragraph_in_place(count, |paragraph| paragraph.set_text("x"))
            .is_err()
    );
}

#[test]
fn an_edit_in_place_drops_the_layout_of_before_the_edit() {
    let mut document = Document::new();
    document.add_paragraph("first");
    document.add_paragraph("second");
    let mut document = Document::from_bytes(&document.to_bytes().expect("saves")).expect("reopens");
    let before = document.layout_deterministic().expect("lays out");
    document
        .edit_body_paragraph_in_place(1, |paragraph| paragraph.set_text("a longer second"))
        .expect("edits")
        .expect("a direct body paragraph");
    let after = document.layout_deterministic().expect("lays out");
    assert!(!std::sync::Arc::ptr_eq(&before, &after));
}
