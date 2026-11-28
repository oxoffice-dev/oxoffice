//! Importing the content of another document: the names its bookmarks keep.

use oxoffice_docx::{
    CommentDate, Document, Length, RunPosition, RunRange, StoryKind, TextCommentOptions,
};

use crate::support::reopen;

/// A document of one paragraph whose run is bookmarked as `name`.
fn bookmarked(name: &str) -> Document {
    bookmarked_paragraphs(&[name])
}

/// A document of one bookmarked paragraph per name, in order.
fn bookmarked_paragraphs(names: &[&str]) -> Document {
    let mut doc = Document::new();
    for name in names {
        bookmark_a_new_paragraph(&mut doc, name);
    }
    doc
}

fn bookmark_a_new_paragraph(doc: &mut Document, name: &str) {
    doc.add_paragraph("bookmarked text");
    let index = doc.content_count() - 1;
    let range = RunRange {
        start: RunPosition {
            body_index: index,
            run_index: 0,
        },
        end: RunPosition {
            body_index: index,
            run_index: 1,
        },
    };
    doc.add_bookmark(name, range).unwrap();
}

/// A one-pixel PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

fn body_story(doc: &Document) -> oxoffice_docx::StoryId {
    doc.stories()
        .unwrap()
        .into_iter()
        .find(|story| story.kind() == StoryKind::Body)
        .unwrap()
}

/// A document with a picture, a link, a footnote and a comment, and a
/// bookmark named "Mark".
fn rich_source() -> Document {
    let mut doc = bookmarked("Mark");
    doc.add_picture(PNG, "dot.png", Length::inches(1.0), Length::inches(1.0));
    let body = body_story(&doc);
    doc.add_hyperlink_to_story(&body, "source link", "https://source.example/")
        .unwrap();
    let note = doc.add_footnote("source note");
    doc.add_paragraph("with a note").add_footnote_ref(note);
    doc.add_paragraph("commented text");
    doc.add_comment_on_text(
        "commented text",
        "Ada",
        "source comment",
        TextCommentOptions {
            date: CommentDate::Undated,
            ..Default::default()
        },
    )
    .unwrap();
    doc
}

/// A destination with its own link and its own "Mark" bookmark.
fn rich_destination() -> Document {
    let mut doc = bookmarked("Mark");
    let body = body_story(&doc);
    doc.add_hyperlink_to_story(&body, "destination link", "https://dest.example/")
        .unwrap();
    doc
}

/// What `doc` shows of the source's dependencies after a save.
fn check_dependencies(mut doc: Document, how: &str) {
    let doc = reopen(&mut doc);
    let images = doc.images();
    assert_eq!(images.len(), 1, "{how}: the picture");
    assert!(
        doc.image_data(&images[0].embed_id).is_some(),
        "{how}: its image"
    );
    let mut urls = doc
        .links()
        .into_iter()
        .filter_map(|link| link.url)
        .collect::<Vec<_>>();
    urls.sort();
    assert_eq!(
        urls,
        ["https://dest.example/", "https://source.example/"],
        "{how}: each link keeps its address"
    );
    assert!(
        doc.footnotes()
            .iter()
            .any(|(_, text)| text.contains("source note")),
        "{how}: the footnote text"
    );
    assert!(
        doc.comments()
            .iter()
            .any(|comment| comment.text() == "source comment"),
        "{how}: the comment"
    );
    let mut ids = doc
        .bookmarks()
        .iter()
        .map(|bookmark| bookmark.id())
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 2, "{how}: fresh bookmark ids");
    let report = doc.validate().unwrap();
    assert!(report.is_valid(), "{how}: {:?}", report.findings);
}

#[test]
fn an_inserted_document_keeps_its_pictures_links_notes_comments_and_fresh_ids() {
    let mut destination = rich_destination();
    destination.insert_document(0, &rich_source());
    check_dependencies(destination, "insert_document");
}
