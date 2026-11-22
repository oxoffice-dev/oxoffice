//! Importing the content of another document: the names its bookmarks keep.

use oxoffice_docx::{
    CommentDate, ContentLocation, Document, DocumentFragment, FragmentConflictPolicy, Length,
    RunPosition, RunRange, StoryKind, TextCommentOptions,
};

use std::io::Cursor;

use oxoffice_opc::OpcPackage;

use crate::support::{doc_with_body, reopen};

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

/// A document whose style "Accent" sets `run_property`, with one paragraph
/// in that style reading `text`.
fn with_accent_style(run_property: &str, text: &str) -> Document {
    with_accent_style_of("paragraph", run_property, text)
}

/// The same, with "Accent" a style of `kind`.
fn with_accent_style_of(kind: &str, run_property: &str, text: &str) -> Document {
    let mut doc = doc_with_body(&format!(
        r#"<w:p><w:pPr><w:pStyle w:val="Accent"/></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>"#
    ));
    let bytes = doc.to_bytes().unwrap();
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    let styles = String::from_utf8(package.get_part("/word/styles.xml").unwrap().to_vec()).unwrap();
    let at = styles.rfind("</w:styles>").unwrap();
    let style = format!(
        r#"<w:style w:type="{kind}" w:styleId="Accent"><w:name w:val="Accent"/><w:rPr>{run_property}</w:rPr></w:style>"#
    );
    package.set_part(
        "/word/styles.xml",
        format!("{}{style}{}", &styles[..at], &styles[at..]).into_bytes(),
    );
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    doc = Document::from_bytes(&out.into_inner()).unwrap();
    doc
}

/// Import the source's body with `policy` and give the style id of the
/// imported paragraph and the ids of the destination's styles named Accent.
fn import_with(policy: FragmentConflictPolicy) -> (String, Vec<String>) {
    let mut destination = with_accent_style("<w:b/>", "destination");
    let source = with_accent_style("<w:i/>", "imported");
    let start = source
        .story_paragraphs()
        .unwrap()
        .into_iter()
        .find(|paragraph| paragraph.location.story().kind() == StoryKind::Body)
        .unwrap()
        .location;
    let end = ContentLocation::end(start.story().clone());
    let fragment = DocumentFragment::from_range(&source, &start, &end, false).unwrap();
    let body = body_story(&destination);
    destination
        .import_fragment(&ContentLocation::end(body), &fragment, policy)
        .unwrap();
    let destination = reopen(&mut destination);
    let imported = destination
        .paragraphs()
        .iter()
        .find(|paragraph| paragraph.text() == "imported")
        .and_then(|paragraph| paragraph.style_id().map(str::to_owned))
        .unwrap();
    let accents = destination
        .styles()
        .iter()
        .map(|style| style.style_id().to_owned())
        .filter(|id| id.starts_with("Accent"))
        .collect();
    (imported, accents)
}

#[test]
fn without_the_destination_formatting_a_different_style_is_imported_under_a_new_id() {
    let rename = FragmentConflictPolicy::for_insert_document(Some(
        FragmentConflictPolicy::reuse_equivalent().with_destination_formatting(false),
    ));
    let (imported, accents) = import_with(rename);
    assert_ne!(imported, "Accent", "the source's Accent is renamed");
    assert_eq!(accents.len(), 2, "{accents:?}");
    assert!(accents.contains(&imported));
}
