//! Importing the content of another document: the names its bookmarks keep.

use oxoffice_docx::{
    ContentLocation, Document, DocumentFragment, FragmentConflictPolicy, RunPosition, RunRange,
    StoryKind,
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
