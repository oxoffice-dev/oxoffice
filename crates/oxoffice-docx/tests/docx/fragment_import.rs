//! Importing the content of another document: the names its bookmarks keep.

use oxoffice_docx::{
    CommentDate, ContentLocation, Document, DocumentFragment, FragmentConflictPolicy, Length,
    RunPosition, RunRange, StoryKind, TextCommentOptions,
};

use std::io::Cursor;

use oxoffice_opc::OpcPackage;

use crate::support::{corpus_doc, doc_with_body, reopen};

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

/// Every bookmark name of `doc`, in document order.
fn bookmark_names(doc: &Document) -> Vec<String> {
    doc.bookmarks()
        .iter()
        .filter_map(|bookmark| bookmark.name().map(str::to_owned))
        .collect()
}

/// Import the whole body of `source` at the end of the body of `destination`.
fn import_body(destination: &mut Document, source: &Document) {
    let start = source
        .story_paragraphs()
        .unwrap()
        .into_iter()
        .find(|paragraph| paragraph.location.story().kind() == StoryKind::Body)
        .expect("a body paragraph")
        .location;
    let end = ContentLocation::end(start.story().clone());
    let fragment = DocumentFragment::from_range(source, &start, &end, false).unwrap();
    let body = destination
        .stories()
        .unwrap()
        .into_iter()
        .find(|story| story.kind() == StoryKind::Body)
        .unwrap();
    destination
        .import_fragment(
            &ContentLocation::end(body),
            &fragment,
            FragmentConflictPolicy::for_insert_document(None),
        )
        .unwrap();
}

#[test]
fn an_imported_bookmark_whose_name_is_free_keeps_it() {
    let mut destination = bookmarked("Intro");
    import_body(&mut destination, &bookmarked("Summary"));
    let destination = reopen(&mut destination);
    assert_eq!(bookmark_names(&destination), ["Intro", "Summary"]);
}

#[test]
fn an_inserted_document_s_free_bookmark_name_is_kept() {
    let mut destination = bookmarked("Intro");
    destination.insert_document(0, &bookmarked("Summary"));
    let destination = reopen(&mut destination);
    let mut names = bookmark_names(&destination);
    names.sort();
    assert_eq!(names, ["Intro", "Summary"]);
}

#[test]
fn an_imported_bookmark_whose_name_is_taken_in_any_case_gets_a_new_one() {
    let mut destination = bookmarked("summary");
    import_body(&mut destination, &bookmarked("Summary"));
    let destination = reopen(&mut destination);
    let names = bookmark_names(&destination);
    assert_eq!(names.len(), 2, "{names:?}");
    assert_eq!(names[0], "summary", "the destination's stays");
    assert!(
        !names[1].eq_ignore_ascii_case("summary"),
        "the imported one is renamed: {names:?}"
    );
}

#[test]
fn two_documents_imported_with_the_same_free_name_keep_it_once() {
    let mut destination = Document::new();
    import_body(&mut destination, &bookmarked("Summary"));
    import_body(&mut destination, &bookmarked("Summary"));
    let destination = reopen(&mut destination);
    let names = bookmark_names(&destination);
    assert_eq!(names.len(), 2, "{names:?}");
    assert_eq!(names[0], "Summary", "the first import keeps it");
    assert_ne!(names[1], "Summary", "the second is renamed: {names:?}");
}

#[test]
fn a_free_name_is_kept_even_after_a_taken_one_got_a_fresh_name() {
    // The fresh name of "Intro" must not take the source's own MailMerge1.
    let mut destination = bookmarked("Intro");
    import_body(
        &mut destination,
        &bookmarked_paragraphs(&["Intro", "MailMerge1"]),
    );
    let destination = reopen(&mut destination);
    let names = bookmark_names(&destination);
    assert_eq!(names.len(), 3, "{names:?}");
    assert_eq!(names[0], "Intro");
    assert_eq!(names[2], "MailMerge1", "kept: {names:?}");
    assert!(
        !["Intro", "MailMerge1"].contains(&names[1].as_str()),
        "the imported Intro is renamed: {names:?}"
    );
}

#[test]
fn a_name_the_destination_only_references_is_free_and_its_reference_then_resolves() {
    let mut destination = doc_with_body(
        r#"<w:p><w:r><w:t xml:space="preserve">See </w:t></w:r><w:fldSimple w:instr=" REF Methods "><w:r><w:t>Error! Reference source not found.</w:t></w:r></w:fldSimple></w:p>"#,
    );
    import_body(&mut destination, &bookmarked("Methods"));
    let destination = reopen(&mut destination);
    assert_eq!(bookmark_names(&destination), ["Methods"]);
}

#[test]
fn a_name_taken_only_in_a_header_of_the_destination_gets_a_new_one() {
    let bytes = corpus_doc("word/report.docx").to_bytes().unwrap();
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    let header = String::from_utf8(package.get_part("/word/header1.xml").unwrap().to_vec())
        .unwrap()
        .replacen(
            "<w:r><w:t>Northwind",
            r#"<w:bookmarkStart w:id="900" w:name="Masthead"/><w:r><w:t>Northwind"#,
            1,
        )
        .replacen(
            "Review</w:t></w:r>",
            r#"Review</w:t></w:r><w:bookmarkEnd w:id="900"/>"#,
            1,
        );
    assert!(
        header.contains("Masthead"),
        "the header bookmark is in place"
    );
    package.set_part("/word/header1.xml", header.into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let mut destination = Document::from_bytes(&out.into_inner()).unwrap();
    import_body(&mut destination, &bookmarked("Masthead"));
    let destination = reopen(&mut destination);
    let names = bookmark_names(&destination);
    assert!(
        !names.iter().any(|name| name == "Masthead"),
        "the body has no Masthead: the imported one is renamed: {names:?}"
    );
    assert_eq!(names.len(), 1, "{names:?}");
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
fn an_appended_document_keeps_its_pictures_links_notes_comments_and_fresh_ids() {
    let mut destination = rich_destination();
    destination.append(&rich_source());
    check_dependencies(destination, "append");
    let mut destination = rich_destination();
    destination.append_with_break(&rich_source(), oxoffice_docx::SectionBreak::NextPage);
    check_dependencies(destination, "append_with_break");
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
fn keeping_the_destination_formatting_keeps_its_style_for_imported_text() {
    let keep = FragmentConflictPolicy::for_insert_document(Some(
        FragmentConflictPolicy::reuse_equivalent().with_destination_formatting(true),
    ));
    assert_eq!(
        import_with(keep),
        ("Accent".to_owned(), vec!["Accent".to_owned()])
    );
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

#[test]
fn an_appended_or_inserted_document_takes_the_destination_s_styles() {
    for how in ["append", "insert_document"] {
        let mut destination = with_accent_style("<w:b/>", "destination");
        let source = with_accent_style("<w:i/>", "imported");
        match how {
            "append" => destination.append(&source),
            _ => destination.insert_document(0, &source),
        }
        let destination = reopen(&mut destination);
        let imported = destination
            .paragraphs()
            .iter()
            .find(|paragraph| paragraph.text() == "imported")
            .and_then(|paragraph| paragraph.style_id().map(str::to_owned));
        assert_eq!(imported.as_deref(), Some("Accent"), "{how}");
        let accents = destination
            .styles()
            .iter()
            .filter(|style| style.style_id().starts_with("Accent"))
            .count();
        assert_eq!(accents, 1, "{how}: the destination's Accent only");
    }
}

#[test]
fn a_word_file_with_crossing_caption_bookmarks_appends_and_inserts() {
    // Word writes a caption's _Ref and _Toc bookmarks as crossing ranges.
    let source = crate::support::corpus_doc("word/thesis.docx");
    for how in ["append", "insert_document"] {
        let mut destination = Document::new();
        destination.add_paragraph("before");
        match how {
            "append" => destination.append(&source),
            _ => destination.insert_document(1, &source),
        }
        let destination = reopen(&mut destination);
        assert!(
            destination.content_count() > source.content_count(),
            "{how}: the source's content is there"
        );
    }
}

#[test]
fn a_google_report_takes_a_libreoffice_report_with_its_linked_heading_styles() {
    // Google's Heading1 has no linked character style; LibreOffice's links
    // Heading1Char, which then comes in unlinked.
    let mut destination = crate::support::corpus_doc("google/report.docx");
    let source = crate::support::corpus_doc("libreoffice/report.docx");
    let end = destination.content_count();
    destination.insert_document(end, &source);
    let destination = reopen(&mut destination);
    let errors = |doc: &Document| {
        doc.validate()
            .unwrap()
            .findings
            .into_iter()
            .filter(|finding| finding.severity == oxoffice_docx::ValidationSeverity::Error)
            .map(|finding| finding.message)
            .collect::<Vec<_>>()
    };
    // The styles of the inner section's header and footer are left behind
    // (#290); the body and its linked styles come in clean.
    let new_errors = errors(&destination)
        .into_iter()
        .filter(|error| !error.contains("/word/header") && !error.contains("/word/footer"))
        .collect::<Vec<_>>();
    assert_eq!(
        new_errors,
        errors(&crate::support::corpus_doc("google/report.docx")),
        "no new error in the body"
    );
    assert!(destination.style("Heading1Char").is_some());
}

#[test]
fn a_same_id_style_of_another_type_comes_in_under_a_new_id() {
    // The destination's Accent is a character style, the source's a paragraph
    // style: the imported paragraph keeps a paragraph style.
    let mut destination = with_accent_style_of("character", "<w:b/>", "destination");
    destination.append(&with_accent_style("<w:i/>", "imported"));
    let destination = reopen(&mut destination);
    let imported = destination
        .paragraphs()
        .iter()
        .find(|paragraph| paragraph.text() == "imported")
        .and_then(|paragraph| paragraph.style_id().map(str::to_owned))
        .unwrap();
    assert_ne!(imported, "Accent");
    assert!(imported.starts_with("Accent"), "{imported}");
}
