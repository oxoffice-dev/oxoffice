//! A comment or a reply created without a date is dated now, UTC to the
//! second; an explicit date is written as given, and `CommentDate::Undated`
//! writes none, on the Word and Google Docs files of the corpus.

use std::path::Path;

use oxoffice_docx::{CommentDate, Document};

fn report(producer: &str) -> Document {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/files")
        .join(producer)
        .join("report.docx");
    Document::open(path).expect("corpus report opens")
}

/// A copy of `doc` through its saved bytes.
fn copy(doc: &mut Document) -> Document {
    Document::from_bytes(&doc.to_bytes().unwrap()).unwrap()
}

/// The date of comment `id` once `doc` is saved and reopened.
fn date_of(doc: &mut Document, id: i32) -> Option<String> {
    copy(doc)
        .comments()
        .into_iter()
        .find(|comment| comment.id() == id)
        .unwrap_or_else(|| panic!("comment {id} is in the saved document"))
        .date()
        .map(str::to_owned)
}

#[test]
fn comparison_keeps_an_undated_reply_undated() {
    // One comparison only: each costs seconds in an unoptimised build. A
    // dated reply replays its own date, as before `CommentDate`.
    let mut original = report("word");
    let parent = original.comments()[0].id();
    let mut edited = copy(&mut original);
    let reply = edited
        .reply_to_with_date(parent, "Author", "A reply", CommentDate::Undated)
        .unwrap();
    let mut redline = copy(&mut original);
    redline
        .compare(&edited, "Comparer", "2026-02-01T00:00:00Z")
        .unwrap();
    assert_eq!(date_of(&mut redline, reply), None);
}
