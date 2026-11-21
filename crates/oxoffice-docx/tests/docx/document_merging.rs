//! Combining documents: `append`, `append_with_break` with each break kind,
//! `insert_document` at a body index, `import_fragment` of a range of another
//! document under each conflict policy, and `insert_content` of a paragraph
//! taken from another document, with what follows the content: styles, lists,
//! pictures, links, notes, comments, bookmarks, controls, revisions, sections.

use std::collections::HashSet;
use std::io::Cursor;

use oxoffice_docx::{
    CommentDate, ContentLocation, Document, DocumentFragment, FragmentConflictPolicy, HdrFtrType,
    HeaderFooterKind, Length, ListLevel, RunItemRef, SectionBreak, StoryId, StoryItemKind,
    StoryKind, StyleBuilder, TextCommentOptions, ValidationSeverity,
};
use oxoffice_docx_oxml::shared::{ST_PageOrientation, ST_SectionType};
use oxoffice_opc::OpcPackage;

use crate::support::{corpus_doc, doc_with, doc_with_body, map_in_parallel, reopen, texts};

/// A one-pixel PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

const DATE: CommentDate = CommentDate::At("2026-11-20T00:00:00Z");

fn body(doc: &Document) -> StoryId {
    doc.stories()
        .expect("stories are listed")
        .into_iter()
        .find(|story| story.kind() == StoryKind::Body)
        .expect("the document has a body")
}

/// The location of body item `index`.
fn item_at(doc: &Document, index: usize) -> ContentLocation {
    doc.story_items(&body(doc)).expect("body items are listed")[index]
        .location()
        .clone()
}

/// The whole body of `doc` as a fragment, without its final section.
fn whole_body(doc: &Document) -> DocumentFragment {
    let start = item_at(doc, 0);
    DocumentFragment::from_range(doc, &start, &ContentLocation::end(body(doc)), false)
        .expect("the body is a fragment")
}

/// Import the whole body of `source` at the end of `doc`.
fn import_at_end(
    doc: &mut Document,
    source: &Document,
    policy: FragmentConflictPolicy,
) -> oxoffice_docx::Result<()> {
    let end = ContentLocation::end(body(doc));
    doc.import_fragment(&end, &whole_body(source), policy)
}

fn saved(doc: &mut Document) -> Vec<u8> {
    doc.to_bytes().expect("document saves")
}

/// The body paragraphs and tables in order: a paragraph as its text, a
/// table as `[table]`.
fn body_items(doc: &Document) -> Vec<String> {
    doc.story_items(&body(doc))
        .expect("body items are listed")
        .iter()
        .filter_map(|item| match item.kind() {
            StoryItemKind::Table => Some("[table]".to_owned()),
            StoryItemKind::Paragraph => Some(item.text().expect("item text").unwrap_or_default()),
            _ => None,
        })
        .collect()
}

/// A saved document whose single paragraph shows the one-pixel PNG.
fn with_picture(text: &str) -> Document {
    let mut doc = doc_with(&[text]);
    doc.add_picture(PNG, "dot.png", Length::inches(1.0), Length::inches(1.0));
    reopen(&mut doc)
}

/// The bytes each body picture shows, in document order, resolved through
/// the package relationships.
fn picture_bytes(doc: &Document) -> Vec<Option<Vec<u8>>> {
    doc.images()
        .iter()
        .map(|image| doc.image_data(&image.embed_id))
        .collect()
}

fn media_parts(doc: &Document) -> Vec<String> {
    doc.part_names()
        .expect("parts are listed")
        .into_iter()
        .filter(|part| part.starts_with("/word/media/"))
        .collect()
}

/// A saved document with one paragraph holding one external link.
fn with_link(text: &str, url: &str) -> Document {
    let mut doc = Document::new();
    doc.append_hyperlink(text, url);
    reopen(&mut doc)
}

/// The body links as (text, address).
fn link_targets(doc: &Document) -> Vec<(String, Option<String>)> {
    doc.links()
        .into_iter()
        .map(|link| (link.text, link.url))
        .collect()
}

fn link(text: &str, url: &str) -> (String, Option<String>) {
    (text.to_owned(), Some(url.to_owned()))
}

/// A saved document of two paragraphs, `<word> one` with a footnote and
/// `<word> two` with an endnote and a comment answered by a reply.
fn with_notes_and_comments(word: &str) -> Document {
    let mut doc = doc_with(&[&format!("{word} one"), &format!("{word} two")]);
    let first = doc.paragraph_story_location(0).unwrap().unwrap();
    doc.create_footnote(&first, &format!("{word} footnote"))
        .unwrap();
    let second = doc.paragraph_story_location(1).unwrap().unwrap();
    doc.create_endnote(&second, &format!("{word} endnote"))
        .unwrap();
    let question = doc
        .add_comment_on_text(
            &format!("{word} two"),
            "Ann",
            &format!("{word} question"),
            TextCommentOptions {
                date: DATE,
                ..Default::default()
            },
        )
        .unwrap();
    doc.reply_to_with_date(question, "Bob", &format!("{word} answer"), DATE)
        .unwrap();
    reopen(&mut doc)
}

/// The text of body paragraph `index` with its marks in place: `[f2]` for
/// footnote 2, `[e2]` for endnote 2, `[c2]` for comment 2.
fn marked_text(doc: &Document, index: usize) -> String {
    let paragraph = doc.paragraph(index).expect("paragraph exists");
    let mut out = String::new();
    for run in paragraph.runs() {
        for item in run.items() {
            match item {
                RunItemRef::Text(text) => out.push_str(text),
                RunItemRef::FootnoteReference(id) => out.push_str(&format!("[f{id}]")),
                RunItemRef::EndnoteReference(id) => out.push_str(&format!("[e{id}]")),
                RunItemRef::CommentReference(id) => out.push_str(&format!("[c{id}]")),
                _ => {}
            }
        }
    }
    out
}

/// The ids marked in `text` after `prefix`, such as the 2 of `[f2]`.
fn marks(text: &str, prefix: &str) -> Vec<i32> {
    text.split(prefix)
        .skip(1)
        .map(|rest| rest.split(']').next().unwrap().parse().unwrap())
        .collect()
}

/// The text of footnote or endnote `id`, its paragraphs joined.
fn note_text(doc: &Document, kind: StoryKind, id: i32) -> Option<String> {
    let story = match kind {
        StoryKind::Footnote => doc.footnote_story(id),
        _ => doc.endnote_story(id),
    }
    .expect("note lookup succeeds")?;
    let items = doc.story_items(&story).expect("note reads");
    Some(
        items
            .iter()
            .map(|item| item.text().expect("note text").unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// Each comment as (id, author, text, parent id).
fn comment_list(doc: &Document) -> Vec<(i32, String, String, Option<i32>)> {
    doc.comments()
        .iter()
        .map(|comment| {
            (
                comment.id(),
                comment.author().unwrap_or_default().to_owned(),
                comment.text(),
                comment.parent_id(),
            )
        })
        .collect()
}

/// A saved document whose body holds a bookmark `Mark` and a REF field and an
/// internal link that point at it, a block control tagged `tag-<word>` with
/// id 5, and an insertion tracked as revision 1.
fn with_identities(word: &str) -> Document {
    let mut doc = doc_with_body(&format!(
        concat!(
            r#"<w:p><w:bookmarkStart w:id="0" w:name="Mark"/><w:r><w:t>{w} marked</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#,
            r#"<w:p><w:r><w:t xml:space="preserve">see </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
            r#"<w:r><w:instrText xml:space="preserve"> REF Mark \h </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>"#,
            r#"<w:r><w:t>{w} marked</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
            r#"<w:p><w:hyperlink w:anchor="Mark"><w:r><w:t>{w} jump</w:t></w:r></w:hyperlink></w:p>"#,
            r#"<w:sdt><w:sdtPr><w:id w:val="5"/><w:tag w:val="tag-{w}"/></w:sdtPr>"#,
            r#"<w:sdtContent><w:p><w:r><w:t>{w} control</w:t></w:r></w:p></w:sdtContent></w:sdt>"#,
            r#"<w:p><w:ins w:id="1" w:author="Ana {w}" w:date="2026-01-01T00:00:00Z"><w:r><w:t>{w} inserted</w:t></w:r></w:ins></w:p>"#,
        ),
        w = word
    ));
    reopen(&mut doc)
}

/// The body bookmarks as (name, text).
fn bookmark_list(doc: &Document) -> Vec<(String, String)> {
    doc.bookmarks()
        .iter()
        .map(|bookmark| {
            (
                bookmark.name().unwrap_or_default().to_owned(),
                bookmark.text().to_owned(),
            )
        })
        .collect()
}

/// The instruction of every field of the body, in order.
fn field_instructions(doc: &Document) -> Vec<String> {
    let mut fields = Vec::new();
    for paragraph in doc.paragraphs() {
        for run in paragraph.runs() {
            for item in run.items() {
                if let RunItemRef::Field(field) = item {
                    fields.push(field.instruction().trim().to_owned());
                }
            }
        }
    }
    fields
}

/// The body controls as (id, tag, text).
fn control_list(doc: &Document) -> Vec<(Option<i32>, Option<String>, String)> {
    doc.content_controls()
        .iter()
        .map(|control| {
            (
                control.id(),
                control.tag().map(str::to_owned),
                control.text(),
            )
        })
        .collect()
}

/// The revisions of every story as (id, author, text).
fn revision_list(doc: &Document) -> Vec<(i32, String, String)> {
    doc.story_revisions()
        .expect("revisions are listed")
        .iter()
        .map(|revision| {
            (
                revision.id(),
                revision.author().to_owned(),
                revision.text().to_owned(),
            )
        })
        .collect()
}

fn all_distinct<T: Eq + std::hash::Hash>(values: &[T]) -> bool {
    values.iter().collect::<HashSet<_>>().len() == values.len()
}

/// A saved document with a paragraph style `id` in color `color`, and one
/// paragraph of it.
fn styled(id: &str, color: &str, text: &str) -> Document {
    let mut doc = Document::new();
    doc.add_style(StyleBuilder::paragraph(id, id).color(color))
        .expect("style is valid");
    doc.add_paragraph(text).style(id);
    reopen(&mut doc)
}

/// The effective color of the first run of body paragraph `index`.
fn color_of(doc: &Document, index: usize) -> Option<String> {
    let paragraph = doc.paragraph(index).expect("paragraph exists");
    let run = paragraph.runs().next().expect("paragraph has a run");
    doc.effective_run_properties(&paragraph, &run).color
}

fn style_of(doc: &Document, index: usize) -> Option<String> {
    doc.paragraph(index)
        .expect("paragraph exists")
        .style_id()
        .map(str::to_owned)
}

/// A new document whose styles part also holds these raw `w:style`
/// elements and whose body holds this raw XML: styles as a producer writes
/// them, which `add_style` refuses one at a time (a linked pair).
fn doc_with_styles(styles: &str, body_xml: &str) -> Document {
    let bytes = doc_with_body(body_xml).to_bytes().expect("document saves");
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    let xml = String::from_utf8(package.get_part("/word/styles.xml").unwrap().to_vec())
        .expect("styles are UTF-8");
    let xml = xml.replace("</w:styles>", &format!("{styles}</w:styles>"));
    package.set_part("/word/styles.xml", xml.into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).expect("package writes");
    Document::from_bytes(&out.into_inner()).expect("document with these styles opens")
}

/// The marker a reader sees before each body paragraph, as layout draws it:
/// "" for a paragraph that is not a list item.
fn markers(doc: &Document) -> Vec<String> {
    let layout = doc.layout_deterministic().expect("document lays out");
    (0..doc.paragraph_count())
        .map(|index| {
            doc.content_index_of_paragraph(index)
                .and_then(|body| layout.document_body_paragraph_numbering(body))
                .map(|numbering| numbering.marker_text.clone())
                .unwrap_or_default()
        })
        .collect()
}

/// A saved document with one decimal list holding these items.
fn list_doc(items: &[&str]) -> Document {
    let mut doc = Document::new();
    let definition = doc
        .add_numbering_definition(&[ListLevel::decimal()])
        .expect("levels are valid");
    let list = doc
        .add_numbering_instance(definition, &[])
        .expect("definition exists");
    for item in items {
        doc.add_paragraph(item).numbering(list, 0);
    }
    reopen(&mut doc)
}

/// The text of the default header section `section` shows, if any.
fn header_shown(doc: &Document, section: usize) -> Option<String> {
    let shown = doc
        .section_story(section, HeaderFooterKind::Header, HdrFtrType::Default)
        .expect("section index in range")?;
    Some(
        doc.story_items(shown.story())
            .expect("header reads")
            .iter()
            .map(|item| item.text().expect("header text").unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// A saved two-section document: `cover` in a first section with its own
/// header "Cover header", `body` in the final section that inherits it.
fn with_inner_section_header() -> Document {
    let mut doc = doc_with(&["cover"]);
    doc.set_header("Cover header");
    doc.insert_section(1).expect("a section appends");
    doc.add_paragraph("body");
    reopen(&mut doc)
}

// ---- append ----

#[test]
fn appending_adds_the_other_documents_paragraphs_and_tables_at_the_end() {
    let mut source = doc_with(&["intro"]);
    let mut table = source.add_table(1, 2);
    table.cell(0, 0).unwrap().set_text("left");
    table.cell(0, 1).unwrap().set_text("right");
    source.add_paragraph("outro");
    let mut source = reopen(&mut source);
    let before = saved(&mut source);

    let mut doc = doc_with(&["first", "second"]);
    doc.append(&source);
    let doc = reopen(&mut doc);
    assert_eq!(
        body_items(&doc),
        ["first", "second", "intro", "[table]", "outro"]
    );
    let table = doc.table(0).expect("the table came along");
    let row = table.row(0).unwrap();
    assert_eq!(
        [row.cell(0).unwrap().text(), row.cell(1).unwrap().text()],
        ["left", "right"]
    );
    assert_eq!(saved(&mut source), before, "the source is left as it was");
}

#[test]
fn appending_brings_missing_styles_and_keeps_the_destination_definition_of_shared_ones() {
    let mut source = styled("Accent", "C00000", "accent");
    source
        .add_style(StyleBuilder::paragraph("Note", "Note").color("00A000"))
        .unwrap();
    source.add_paragraph("note").style("Note");
    let source = reopen(&mut source);
    let mut doc = styled("Accent", "0000FF", "dest");
    assert!(doc.style("Note").is_none());
    doc.append(&source);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["dest", "accent", "note"]);
    // The style the destination lacked comes with its definition.
    assert_eq!(style_of(&doc, 2).as_deref(), Some("Note"));
    assert_eq!(color_of(&doc, 2).as_deref(), Some("00A000"));
    // As Word's "Insert text from file" does, the destination's definition
    // of a style both documents have wins.
    let accents = doc
        .styles()
        .iter()
        .filter(|style| style.style_id() == "Accent")
        .count();
    assert_eq!(accents, 1);
    assert_eq!(style_of(&doc, 1).as_deref(), Some("Accent"));
    assert_eq!(color_of(&doc, 0).as_deref(), Some("0000FF"));
    assert_eq!(color_of(&doc, 1).as_deref(), Some("0000FF"));
}

#[test]
fn an_appended_list_counts_from_one_again() {
    let source = list_doc(&["gamma", "delta"]);
    let mut doc = list_doc(&["alpha", "beta"]);
    doc.append(&source);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["alpha", "beta", "gamma", "delta"]);
    assert_eq!(markers(&doc), ["1.", "2.", "1.", "2."]);
    let lists: Vec<_> = doc
        .paragraphs()
        .iter()
        .map(|paragraph| paragraph.numbering().expect("a list item").0)
        .collect();
    assert_eq!(lists[0], lists[1]);
    assert_eq!(lists[2], lists[3]);
    assert_ne!(
        lists[0], lists[2],
        "the appended items are a list of their own"
    );
}

// Fails today: append copies the drawing but not its image relationship and
// part, so the picture points at nothing. Issue: #192
#[test]
#[should_panic(expected = "the appended picture shows its own image")]
fn an_appended_picture_keeps_its_image() {
    let source = with_picture("source");
    let mut doc = doc_with(&["dest"]);
    doc.append(&source);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["dest", "source", ""]);
    let shown = picture_bytes(&doc);
    assert!(
        shown == [Some(PNG.to_vec())],
        "the appended picture shows its own image, got {:?}",
        shown
            .iter()
            .map(|bytes| bytes.as_ref().map(Vec::len))
            .collect::<Vec<_>>()
    );
}

// Fails today: append copies the link's relationship id, not the
// relationship, so it resolves to the destination's own link. Issue: #192
#[test]
#[should_panic(expected = "the appended link goes to its own address")]
fn an_appended_hyperlink_keeps_its_address() {
    let source = with_link("source site", "https://source.example/");
    let mut doc = with_link("dest site", "https://dest.example/");
    doc.append(&source);
    let doc = reopen(&mut doc);
    assert_eq!(
        link_targets(&doc),
        [
            link("dest site", "https://dest.example/"),
            link("source site", "https://source.example/"),
        ],
        "the appended link goes to its own address"
    );
}

// Fails today: append copies the note and comment marks but not the notes
// and comments, so the appended footnote mark shows the destination's note
// and the comment marks collide with the destination's. Issue: #192
#[test]
#[should_panic(expected = "the appended footnote mark shows the source's note")]
fn an_appended_footnote_and_comment_keep_their_text() {
    let source = with_notes_and_comments("src");
    let mut doc = with_notes_and_comments("dst");
    doc.append(&source);
    let doc = reopen(&mut doc);
    let appended = marked_text(&doc, 2);
    assert!(appended.starts_with("src one"), "{appended}");
    let footnotes: Vec<_> = marks(&appended, "[f")
        .into_iter()
        .map(|id| note_text(&doc, StoryKind::Footnote, id))
        .collect();
    assert_eq!(
        footnotes,
        [Some(" src footnote".to_owned())],
        "the appended footnote mark shows the source's note"
    );
    let comments: Vec<_> = comment_list(&doc).into_iter().map(|c| c.2).collect();
    assert_eq!(
        comments,
        ["dst question", "dst answer", "src question", "src answer"]
    );
}

// Fails today: append keeps the source's bookmark and control ids and
// names, so both documents' bookmarks are named Mark with id 0 and the
// reader no longer tells them apart. Issue: #192
#[test]
#[should_panic(expected = "appended bookmarks keep distinct names")]
fn appended_bookmarks_and_controls_get_distinct_identities() {
    let source = with_identities("src");
    let mut doc = with_identities("dst");
    doc.append(&source);
    let doc = reopen(&mut doc);
    let bookmarks = bookmark_list(&doc);
    let names: Vec<_> = bookmarks.iter().map(|(name, _)| name.clone()).collect();
    assert!(
        bookmarks.len() == 2 && all_distinct(&names),
        "appended bookmarks keep distinct names: {bookmarks:?}"
    );
    let ids: Vec<_> = control_list(&doc).into_iter().map(|c| c.0).collect();
    assert!(all_distinct(&ids), "appended controls keep distinct ids");
}

// ---- append_with_break ----

#[test]
fn appending_with_a_break_puts_the_other_document_in_a_section_of_its_own() {
    for kind in [
        SectionBreak::NextPage,
        SectionBreak::Continuous,
        SectionBreak::EvenPage,
        SectionBreak::OddPage,
    ] {
        let mut source = doc_with(&["source"]);
        let before = saved(&mut source);
        let mut doc = doc_with(&["dest"]);
        doc.append_with_break(&source, kind);
        let doc = reopen(&mut doc);
        assert_eq!(doc.section_count(), 2, "{kind:?}");
        // The break sits in an empty paragraph of its own.
        assert_eq!(texts(&doc), ["dest", "", "source"], "{kind:?}");
        assert_eq!(saved(&mut source), before, "{kind:?}: source unchanged");
    }
}

// Fails today: the break's type is written on the section that ends at the
// break (the destination's) instead of the section that starts there, so a
// continuous break starts the appended content on a new page. Issue: #193
#[test]
#[should_panic(expected = "the appended section starts as the break asks")]
fn each_break_kind_starts_the_appended_section_as_asked() {
    let mut wrong = Vec::new();
    for (kind, expected) in [
        (SectionBreak::Continuous, ST_SectionType::Continuous),
        (SectionBreak::EvenPage, ST_SectionType::EvenPage),
        (SectionBreak::OddPage, ST_SectionType::OddPage),
    ] {
        let mut doc = doc_with(&["dest"]);
        doc.append_with_break(&doc_with(&["source"]), kind);
        let doc = reopen(&mut doc);
        let started = doc.section(1).unwrap().break_type();
        if started != Some(expected) {
            wrong.push(format!("{kind:?}: the appended section starts {started:?}"));
        }
        if kind == SectionBreak::Continuous {
            let pages = doc.layout_deterministic().unwrap().layout.pages.len();
            if pages != 1 {
                wrong.push(format!("{kind:?}: {pages} pages"));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "the appended section starts as the break asks: {wrong:?}"
    );
}

// Fails today: the break paragraph gets new Letter section properties, so
// the destination's own pages lose their size, margins and header (the
// destination's section properties move to the appended content). Issue: #193
#[test]
#[should_panic(expected = "the destination's pages keep their layout")]
fn appending_with_a_break_keeps_the_destination_pages_layout_and_header() {
    let a4 = (Length::mm(210.0), Length::mm(297.0));
    let margin = Length::inches(0.5);
    let mut doc = doc_with(&["dest"]);
    doc.set_page_size(a4.0, a4.1);
    doc.set_margins(margin, margin, margin, margin);
    doc.set_header("Dest header");
    let mut doc = reopen(&mut doc);
    let size = doc.section(0).unwrap().page_size();
    let margins = doc.section(0).unwrap().margins();

    doc.append_with_break(&doc_with(&["source"]), SectionBreak::NextPage);
    let doc = reopen(&mut doc);
    let first = doc.section(0).unwrap();
    assert_eq!(
        (first.page_size(), first.margins(), header_shown(&doc, 0)),
        (size, margins, Some("Dest header".to_owned())),
        "the destination's pages keep their layout"
    );
}

// ---- insert_document ----

#[test]
fn inserting_a_document_puts_its_content_at_the_body_index() {
    let mut source = doc_with(&["one", "two"]);
    let before = saved(&mut source);
    let mut doc = doc_with(&["first", "last"]);
    doc.insert_document(1, &source);
    // An index past the end inserts at the end.
    doc.insert_document(99, &source);
    let mut doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["first", "one", "two", "last", "one", "two"]);
    assert_eq!(saved(&mut source), before, "the source is left as it was");

    // An empty document inserts nothing.
    let unchanged = saved(&mut doc);
    doc.insert_document(1, &Document::new());
    assert_eq!(saved(&mut doc), unchanged);
}

#[test]
fn each_inserted_copy_of_a_list_counts_from_one() {
    let source = list_doc(&["step", "check"]);
    let mut doc = doc_with(&["intro"]);
    doc.insert_document(1, &source);
    doc.insert_document(3, &source);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["intro", "step", "check", "step", "check"]);
    assert_eq!(markers(&doc), ["", "1.", "2.", "1.", "2."]);
}

#[test]
fn an_inserted_document_keeps_its_own_font() {
    let mut source = Document::new();
    source.set_default_font_name_value(Some("Georgia")).unwrap();
    source
        .set_default_font_size_value(Some(Length::pt(14.0)))
        .unwrap();
    source.add_paragraph("source");
    let source = reopen(&mut source);
    let font = |doc: &Document, index: usize| {
        let paragraph = doc.paragraph(index).unwrap();
        let run = paragraph.runs().next().unwrap();
        let properties = doc.effective_run_properties(&paragraph, &run);
        (properties.font_ascii, properties.sz)
    };
    let mut doc = doc_with(&["dest"]);
    let own = font(&doc, 0);
    assert_ne!(own.0.as_deref(), Some("Georgia"));

    doc.insert_document(1, &source);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["dest", "source"]);
    assert_eq!(font(&doc, 0), own, "the destination keeps its font");
    assert_eq!(
        font(&doc, 1),
        (
            Some("Georgia".to_owned()),
            Some(oxoffice_docx::HalfPoint(28))
        )
    );
}

#[test]
fn inserting_a_document_gives_taken_bookmarks_controls_and_revisions_new_identities() {
    let source = with_identities("src");
    let mut doc = with_identities("dst");
    let end = doc.content_count();
    doc.insert_document(end, &source);
    let doc = reopen(&mut doc);

    let bookmarks = bookmark_list(&doc);
    assert_eq!(bookmarks.len(), 2);
    assert_eq!(bookmarks[0], ("Mark".to_owned(), "dst marked".to_owned()));
    assert_ne!(bookmarks[1].0, "Mark");
    assert_eq!(bookmarks[1].1, "src marked");
    // The inserted REF field follows its renamed bookmark.
    assert_eq!(
        field_instructions(&doc),
        [
            "REF Mark \\h".to_owned(),
            format!("REF {} \\h", bookmarks[1].0)
        ]
    );

    let controls = control_list(&doc);
    let tags: Vec<_> = controls.iter().map(|c| c.1.clone().unwrap()).collect();
    assert_eq!(tags, ["tag-dst", "tag-src"]);
    assert!(all_distinct(
        &controls.iter().map(|c| c.0).collect::<Vec<_>>()
    ));

    let revisions = revision_list(&doc);
    let kept: Vec<_> = revisions.iter().map(|r| (&*r.1, &*r.2)).collect();
    assert_eq!(
        kept,
        [("Ana dst", "dst inserted"), ("Ana src", "src inserted")]
    );
    assert_ne!(revisions[0].0, revisions[1].0);
}

// Fails today: insert_document copies the body only, without the image and
// link relationships its drawings and links name: here the destination has
// no such ids, so the picture shows nothing and the link has no address
// (an id the destination uses resolves to its own target). The Python
// binding's insert_document goes through
// `FragmentConflictPolicy::for_insert_document` and keeps both. Issue: #192
#[test]
#[should_panic(expected = "the inserted picture and link keep their targets")]
fn an_inserted_picture_and_hyperlink_keep_their_image_and_address() {
    let mut source = with_picture("source");
    source.append_hyperlink("source site", "https://source.example/");
    let source = reopen(&mut source);
    let mut doc = with_link("dest site", "https://dest.example/");
    let end = doc.content_count();
    doc.insert_document(end, &source);
    let doc = reopen(&mut doc);
    assert_eq!(
        (picture_bytes(&doc), link_targets(&doc)),
        (
            vec![Some(PNG.to_vec())],
            vec![
                link("dest site", "https://dest.example/"),
                link("source site", "https://source.example/"),
            ]
        ),
        "the inserted picture and link keep their targets"
    );
}

// Fails today: insert_document copies the note and comment marks without
// the notes and comments. Issue: #192
#[test]
#[should_panic(expected = "the inserted footnote mark shows the source's note")]
fn an_inserted_footnote_and_comment_come_with_the_content() {
    let source = with_notes_and_comments("src");
    let mut doc = with_notes_and_comments("dst");
    doc.insert_document(2, &source);
    let doc = reopen(&mut doc);
    let inserted = marked_text(&doc, 2);
    assert!(inserted.starts_with("src one"), "{inserted}");
    let footnotes: Vec<_> = marks(&inserted, "[f")
        .into_iter()
        .map(|id| note_text(&doc, StoryKind::Footnote, id))
        .collect();
    assert_eq!(
        footnotes,
        [Some(" src footnote".to_owned())],
        "the inserted footnote mark shows the source's note"
    );
    let comments: Vec<_> = comment_list(&doc).into_iter().map(|c| c.2).collect();
    assert_eq!(
        comments,
        ["dst question", "dst answer", "src question", "src answer"]
    );
}

// Fails today: the pasted section keeps the source's header relationship id,
// which names the destination's header, so it shows that header. Issue: #192
#[test]
#[should_panic(expected = "the source's first section keeps its own header")]
fn an_appended_or_inserted_document_keeps_the_header_of_its_inner_section() {
    let source = with_inner_section_header();
    assert_eq!(header_shown(&source, 0).as_deref(), Some("Cover header"));
    let mut shown = Vec::new();
    for append in [true, false] {
        let mut doc = doc_with(&["dest"]);
        doc.set_header("Dest header");
        let mut doc = reopen(&mut doc);
        if append {
            doc.append(&source);
        } else {
            doc.insert_document(1, &source);
        }
        let doc = reopen(&mut doc);
        assert_eq!(texts(&doc), ["dest", "cover", "", "body"]);
        shown.push((append, header_shown(&doc, 0), header_shown(&doc, 1)));
    }
    let header = |text: &str| Some(text.to_owned());
    assert_eq!(
        shown,
        [
            (true, header("Cover header"), header("Dest header")),
            (false, header("Cover header"), header("Dest header")),
        ],
        "the source's first section keeps its own header"
    );
}

// ---- import_fragment ----

#[test]
fn importing_a_body_brings_its_pictures_and_hyperlinks() {
    let mut source = with_picture("source");
    source.append_hyperlink("source site", "https://source.example/");
    let mut source = reopen(&mut source);
    let before = saved(&mut source);
    let mut doc = with_link("dest site", "https://dest.example/");
    import_at_end(&mut doc, &source, FragmentConflictPolicy::default()).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["dest site", "source", "source site"]);
    assert_eq!(picture_bytes(&doc), [Some(PNG.to_vec())]);
    assert_eq!(
        link_targets(&doc),
        [
            link("dest site", "https://dest.example/"),
            link("source site", "https://source.example/"),
        ]
    );
    let errors: Vec<_> = doc
        .validate()
        .unwrap()
        .findings
        .into_iter()
        .filter(|finding| finding.severity == ValidationSeverity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(saved(&mut source), before, "the source is left as it was");
}

#[test]
fn importing_renumbers_notes_and_comments_whose_ids_are_taken() {
    let source = with_notes_and_comments("src");
    let mut doc = with_notes_and_comments("dst");
    import_at_end(&mut doc, &source, FragmentConflictPolicy::default()).unwrap();
    let doc = reopen(&mut doc);
    let lines: Vec<_> = (0..4).map(|index| marked_text(&doc, index)).collect();

    let footnotes = [marks(&lines[0], "[f"), marks(&lines[2], "[f")].concat();
    let endnotes = [marks(&lines[1], "[e"), marks(&lines[3], "[e")].concat();
    assert_eq!(footnotes.len(), 2);
    assert_eq!(endnotes.len(), 2);
    assert_ne!(footnotes[0], footnotes[1]);
    assert_ne!(endnotes[0], endnotes[1]);
    assert_eq!(
        note_text(&doc, StoryKind::Footnote, footnotes[0]).as_deref(),
        Some(" dst footnote")
    );
    assert_eq!(
        note_text(&doc, StoryKind::Footnote, footnotes[1]).as_deref(),
        Some(" src footnote")
    );
    assert_eq!(
        note_text(&doc, StoryKind::Endnote, endnotes[1]).as_deref(),
        Some(" src endnote")
    );

    let comments = comment_list(&doc);
    let ids: Vec<_> = comments.iter().map(|c| c.0).collect();
    assert!(all_distinct(&ids), "{ids:?}");
    let by_id = |id: i32| comments.iter().find(|c| c.0 == id).unwrap();
    let marked = marks(&lines[3], "[c");
    assert_eq!(marked.len(), 2);
    let (question, answer) = (by_id(marked[0]), by_id(marked[1]));
    assert_eq!((&*question.1, &*question.2), ("Ann", "src question"));
    assert_eq!((&*answer.1, &*answer.2), ("Bob", "src answer"));
    assert_eq!(answer.3, Some(question.0), "the reply keeps its thread");
    assert_eq!(
        doc.comment_anchor_text(question.0).unwrap().as_deref(),
        Some("src two")
    );
}

#[test]
fn importing_gives_taken_bookmarks_controls_and_revisions_new_identities() {
    let source = with_identities("src");
    let mut doc = with_identities("dst");
    import_at_end(&mut doc, &source, FragmentConflictPolicy::default()).unwrap();
    let doc = reopen(&mut doc);

    let bookmarks = bookmark_list(&doc);
    assert_eq!(bookmarks.len(), 2);
    assert_eq!(bookmarks[0], ("Mark".to_owned(), "dst marked".to_owned()));
    let renamed = bookmarks[1].0.clone();
    assert_ne!(renamed, "Mark");
    assert_eq!(bookmarks[1].1, "src marked");
    // The imported REF field and internal link follow the new name.
    assert_eq!(
        field_instructions(&doc),
        ["REF Mark \\h".to_owned(), format!("REF {renamed} \\h")]
    );
    let anchors: Vec<_> = doc
        .links()
        .into_iter()
        .map(|link| (link.text, link.anchor))
        .collect();
    assert_eq!(
        anchors,
        [
            ("dst jump".to_owned(), Some("Mark".to_owned())),
            ("src jump".to_owned(), Some(renamed)),
        ]
    );

    let controls = control_list(&doc);
    assert_eq!(
        controls[0],
        (
            Some(5),
            Some("tag-dst".to_owned()),
            "dst control".to_owned()
        )
    );
    assert_eq!(controls[1].1.as_deref(), Some("tag-src"));
    assert_eq!(controls[1].2, "src control");
    assert_ne!(controls[1].0, Some(5));

    let revisions = revision_list(&doc);
    assert_eq!(revisions.len(), 2);
    assert_eq!(revisions[1].1, "Ana src");
    assert_eq!(revisions[1].2, "src inserted");
    assert_ne!(revisions[0].0, revisions[1].0);
}

#[test]
fn a_bookmark_whose_name_is_free_keeps_it_on_import() {
    let source = doc_with_body(
        r#"<w:p><w:bookmarkStart w:id="0" w:name="Summary"/><w:r><w:t>summary</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#,
    );
    let mut doc = with_identities("dst");
    import_at_end(&mut doc, &source, FragmentConflictPolicy::default()).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(
        bookmark_list(&doc),
        [
            ("Mark".to_owned(), "dst marked".to_owned()),
            ("Summary".to_owned(), "summary".to_owned()),
        ],
        "a free bookmark name survives the import"
    );
}

#[test]
fn a_style_with_a_taken_id_and_another_look_is_imported_under_a_new_id() {
    for policy in [
        FragmentConflictPolicy::default(),
        FragmentConflictPolicy::rename_all(),
    ] {
        let source = styled("Accent", "C00000", "source");
        let mut doc = styled("Accent", "0000FF", "dest");
        import_at_end(&mut doc, &source, policy).unwrap();
        let doc = reopen(&mut doc);
        assert_eq!(texts(&doc), ["dest", "source"]);
        assert_eq!(style_of(&doc, 0).as_deref(), Some("Accent"));
        assert_eq!(color_of(&doc, 0).as_deref(), Some("0000FF"));
        let renamed = style_of(&doc, 1).expect("the import keeps a style");
        assert_ne!(renamed, "Accent", "{policy:?}");
        assert!(doc.style(&renamed).is_some());
        assert_eq!(color_of(&doc, 1).as_deref(), Some("C00000"), "{policy:?}");
    }
}

#[test]
fn an_equal_style_is_reused_unless_every_dependency_is_renamed() {
    let source = styled("Accent", "C00000", "source");
    let styles = source.styles().len();
    let mut reused = styled("Accent", "C00000", "dest");
    import_at_end(&mut reused, &source, FragmentConflictPolicy::default()).unwrap();
    let reused = reopen(&mut reused);
    assert_eq!(style_of(&reused, 1).as_deref(), Some("Accent"));
    assert_eq!(reused.styles().len(), styles, "no style was added");

    let mut renamed = styled("Accent", "C00000", "dest");
    import_at_end(&mut renamed, &source, FragmentConflictPolicy::rename_all()).unwrap();
    let renamed = reopen(&mut renamed);
    assert_ne!(style_of(&renamed, 1).as_deref(), Some("Accent"));
    assert_eq!(color_of(&renamed, 1).as_deref(), Some("C00000"));
}

#[test]
fn an_imported_list_continues_an_equal_list_unless_numbering_reuse_is_off() {
    let source = list_doc(&["gamma", "delta"]);
    for (policy, expected) in [
        (FragmentConflictPolicy::default(), ["1.", "2.", "3.", "4."]),
        (
            FragmentConflictPolicy::default().with_numbering_reuse(false),
            ["1.", "2.", "1.", "2."],
        ),
        (
            FragmentConflictPolicy::for_insert_document(None),
            ["1.", "2.", "1.", "2."],
        ),
    ] {
        let mut doc = list_doc(&["alpha", "beta"]);
        import_at_end(&mut doc, &source, policy).unwrap();
        let doc = reopen(&mut doc);
        assert_eq!(texts(&doc), ["alpha", "beta", "gamma", "delta"]);
        assert_eq!(markers(&doc), expected, "{policy:?}");
    }
}

#[test]
fn an_imported_picture_shares_an_equal_image_part_unless_related_part_reuse_is_off() {
    let source = with_picture("source");
    for (policy, parts) in [
        (FragmentConflictPolicy::default(), 1),
        (
            FragmentConflictPolicy::default().with_related_part_reuse(false),
            3,
        ),
    ] {
        let mut doc = with_picture("dest");
        import_at_end(&mut doc, &source, policy).unwrap();
        import_at_end(&mut doc, &source, policy).unwrap();
        let doc = reopen(&mut doc);
        assert_eq!(
            picture_bytes(&doc),
            vec![Some(PNG.to_vec()); 3],
            "{policy:?}"
        );
        assert_eq!(media_parts(&doc).len(), parts, "{policy:?}");
    }
}

#[test]
fn a_section_inclusive_fragment_brings_its_page_layout_and_header() {
    let mut source = doc_with(&["wide"]);
    source.set_landscape();
    source.set_header("Source header");
    let source = reopen(&mut source);
    let fragment = DocumentFragment::from_range(
        &source,
        &item_at(&source, 0),
        &ContentLocation::end(body(&source)),
        true,
    )
    .unwrap();
    let mut doc = doc_with(&["dest"]);
    doc.set_header("Dest header");
    let mut doc = reopen(&mut doc);
    doc.import_fragment(
        &item_at(&doc, 0),
        &fragment,
        FragmentConflictPolicy::default(),
    )
    .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["wide", "", "dest"]);
    assert_eq!(doc.section_count(), 2);
    assert_eq!(
        doc.section(0).unwrap().orientation(),
        Some(ST_PageOrientation::Landscape)
    );
    assert_eq!(header_shown(&doc, 0).as_deref(), Some("Source header"));
    assert_ne!(
        doc.section(1).unwrap().orientation(),
        Some(ST_PageOrientation::Landscape)
    );
    assert_eq!(header_shown(&doc, 1).as_deref(), Some("Dest header"));
}

#[test]
fn an_imported_inner_section_keeps_its_header() {
    let source = with_inner_section_header();
    let mut doc = doc_with(&["dest"]);
    doc.set_header("Dest header");
    let mut doc = reopen(&mut doc);
    import_at_end(&mut doc, &source, FragmentConflictPolicy::default()).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["dest", "cover", "", "body"]);
    assert_eq!(header_shown(&doc, 0).as_deref(), Some("Cover header"));
    assert_eq!(header_shown(&doc, 1).as_deref(), Some("Dest header"));
}

#[test]
fn a_range_fragment_takes_only_its_items_and_lands_where_asked() {
    let source = doc_with(&["a", "b", "c", "d"]);
    let fragment =
        DocumentFragment::from_range(&source, &item_at(&source, 1), &item_at(&source, 3), false)
            .unwrap();
    let mut doc = doc_with(&["first", "last"]);
    doc.import_fragment(
        &item_at(&doc, 1),
        &fragment,
        FragmentConflictPolicy::default(),
    )
    .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["first", "b", "c", "last"]);
}

#[test]
fn a_bad_range_or_destination_is_refused_and_changes_nothing() {
    let source = doc_with(&["a", "b"]);
    let (first, second) = (item_at(&source, 0), item_at(&source, 1));
    let error = DocumentFragment::from_range(&source, &first, &first, false)
        .unwrap_err()
        .to_string();
    assert!(error.contains("at least one direct body item"), "{error}");
    let error = DocumentFragment::from_range(&source, &second, &first, false)
        .unwrap_err()
        .to_string();
    assert!(error.contains("at least one direct body item"), "{error}");
    let error = DocumentFragment::from_range(&source, &first, &second, true)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("must end at the main-body boundary"),
        "{error}"
    );

    // A fragment holding section properties goes into the body only.
    let fragment =
        DocumentFragment::from_range(&source, &first, &ContentLocation::end(body(&source)), true)
            .unwrap();
    let mut doc = doc_with(&["dest"]);
    doc.set_header("header");
    let mut doc = reopen(&mut doc);
    let before = saved(&mut doc);
    let header = doc
        .stories()
        .unwrap()
        .into_iter()
        .find(|story| story.kind() == StoryKind::Header)
        .unwrap();
    let error = doc
        .import_fragment(
            &ContentLocation::end(header),
            &fragment,
            FragmentConflictPolicy::default(),
        )
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("require the main body destination"),
        "{error}"
    );
    assert_eq!(saved(&mut doc), before);
}

/// A linked paragraph and character style pair `Big` and `BigChar`, the
/// character style in `color`.
fn linked_pair(color: &str) -> String {
    format!(
        concat!(
            r#"<w:style w:type="paragraph" w:styleId="Big"><w:name w:val="Big"/><w:link w:val="BigChar"/>"#,
            r#"<w:rPr><w:sz w:val="40"/></w:rPr></w:style>"#,
            r#"<w:style w:type="character" w:styleId="BigChar"><w:name w:val="Big Char"/><w:link w:val="Big"/>"#,
            r#"<w:rPr><w:color w:val="{}"/><w:sz w:val="40"/></w:rPr></w:style>"#,
        ),
        color
    )
}

// Fails today: the character style is renamed for its other color but keeps
// its link to the reused paragraph style, whose link still names the
// destination's character style, so the style graph check refuses the
// import (and insert_document panics). LibreOffice's contract into its
// report fails the same way on Heading1Char. Issue: #195
#[test]
#[should_panic(expected = "a source whose linked character style differs imports")]
fn a_renamed_linked_style_keeps_a_reciprocal_link() {
    let paragraph = |text: &str| {
        format!(
            r#"<w:p><w:pPr><w:pStyle w:val="Big"/></w:pPr><w:r><w:rPr><w:rStyle w:val="BigChar"/></w:rPr><w:t>{text}</w:t></w:r></w:p>"#
        )
    };
    let source = doc_with_styles(&linked_pair("C00000"), &paragraph("source"));
    let mut doc = doc_with_styles(&linked_pair("0000FF"), &paragraph("dest"));
    import_at_end(&mut doc, &source, FragmentConflictPolicy::default())
        .expect("a source whose linked character style differs imports");
    let doc = reopen(&mut doc);
    assert_eq!(color_of(&doc, 1).as_deref(), Some("C00000"));
    doc.validate_style_graph().unwrap();
}

// ---- insert_content ----

#[test]
fn a_paragraph_taken_from_another_document_can_be_inserted_unless_its_link_would_dangle() {
    let mut source = Document::new();
    source.add_paragraph("moved ").add_run("bold").bold(true);
    source.add_paragraph("");
    source.append_hyperlink("source site", "https://source.example/");
    source.add_paragraph("kept");
    let mut source = reopen(&mut source);
    let first = item_at(&source, 0);
    let moved = source.remove_content_at(&first).unwrap();
    let mut doc = doc_with(&["first", "last"]);
    doc.insert_content(&item_at(&doc, 1), moved).unwrap();
    let mut doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["first", "moved bold", "last"]);
    let paragraph = doc.paragraph(1).unwrap();
    let bold: Vec<_> = paragraph.runs().map(|run| run.bold_value()).collect();
    assert_eq!(bold, [None, Some(true)]);

    // The paragraph with the link: the destination has no relationship for
    // it, so the insert is refused and changes nothing.
    let first = item_at(&source, 0);
    let linked = source.remove_content_at(&first).unwrap();
    assert_eq!(texts(&reopen(&mut source)), ["kept"]);
    let before = saved(&mut doc);
    let error = doc
        .insert_content(&ContentLocation::end(body(&doc)), linked)
        .unwrap_err()
        .to_string();
    assert!(error.contains("missing relationship"), "{error}");
    assert_eq!(saved(&mut doc), before);
}

// Fails today: the relationship id of the moved link is only checked to
// exist in the destination, where it names the destination's own link, so
// the inserted link silently goes there. Issue: #196
#[test]
#[should_panic(expected = "the moved link keeps its address or is refused")]
fn a_paragraph_from_another_document_never_takes_the_destinations_link() {
    let mut source = with_link("source site", "https://source.example/");
    let first = item_at(&source, 0);
    let fragment = source.remove_content_at(&first).unwrap();
    let mut doc = with_link("dest site", "https://dest.example/");
    if doc
        .insert_content(&ContentLocation::end(body(&doc)), fragment)
        .is_ok()
    {
        let doc = reopen(&mut doc);
        assert_eq!(
            link_targets(&doc),
            [
                link("dest site", "https://dest.example/"),
                link("source site", "https://source.example/"),
            ],
            "the moved link keeps its address or is refused"
        );
    }
}

// ---- Corpus ----

#[test]
fn the_contract_imported_into_itself_keeps_every_comment_note_and_revision() {
    let source = corpus_doc("authored/contract.docx");
    let mut doc = corpus_doc("authored/contract.docx");
    let paragraphs = texts(&source);
    let comments: Vec<_> = comment_list(&source).into_iter().map(|c| c.2).collect();
    let footnotes: Vec<_> = source.footnotes().into_iter().map(|f| f.1).collect();
    let authors: Vec<_> = revision_list(&source).into_iter().map(|r| r.1).collect();
    assert!(!comments.is_empty() && !footnotes.is_empty() && !authors.is_empty());

    import_at_end(&mut doc, &source, FragmentConflictPolicy::default()).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), [paragraphs.clone(), paragraphs].concat());

    let merged = comment_list(&doc);
    assert!(all_distinct(
        &merged.iter().map(|c| c.0).collect::<Vec<_>>()
    ));
    let mut merged_texts: Vec<_> = merged.iter().map(|c| c.2.clone()).collect();
    let mut twice = [comments.clone(), comments].concat();
    merged_texts.sort();
    twice.sort();
    assert_eq!(merged_texts, twice);
    // Every reply still answers a comment of the document.
    for parent in merged.iter().filter_map(|c| c.3) {
        assert!(merged.iter().any(|c| c.0 == parent), "reply to {parent}");
    }

    let notes = doc.footnotes();
    assert!(all_distinct(&notes.iter().map(|n| n.0).collect::<Vec<_>>()));
    let mut note_texts: Vec<_> = notes.into_iter().map(|n| n.1).collect();
    let mut twice = [footnotes.clone(), footnotes].concat();
    note_texts.sort();
    twice.sort();
    assert_eq!(note_texts, twice);

    let revisions = revision_list(&doc);
    assert!(all_distinct(
        &revisions.iter().map(|r| r.0).collect::<Vec<_>>()
    ));
    let merged_authors: Vec<_> = revisions.into_iter().map(|r| r.1).collect();
    assert_eq!(merged_authors, [authors.clone(), authors].concat());
}

#[test]
fn the_form_imported_into_itself_keeps_every_control_tag_with_new_ids() {
    let source = corpus_doc("authored/form.docx");
    let mut doc = corpus_doc("authored/form.docx");
    let controls = control_list(&source);
    assert_eq!(controls.len(), 14);
    import_at_end(&mut doc, &source, FragmentConflictPolicy::default()).unwrap();
    let doc = reopen(&mut doc);
    let merged = control_list(&doc);
    let tags: Vec<_> = merged.iter().map(|c| c.1.clone()).collect();
    let source_tags: Vec<_> = controls.iter().map(|c| c.1.clone()).collect();
    assert_eq!(tags, [source_tags.clone(), source_tags].concat());
    let ids: Vec<_> = merged.iter().map(|c| c.0).collect();
    assert!(all_distinct(&ids), "{ids:?}");
    // The original controls keep their ids.
    assert_eq!(
        merged[..14].iter().map(|c| c.0).collect::<Vec<_>>(),
        controls.iter().map(|c| c.0).collect::<Vec<_>>()
    );
    let names: Vec<_> = bookmark_list(&doc).into_iter().map(|b| b.0).collect();
    assert!(all_distinct(&names), "{names:?}");
}

/// The destination <- source pairs of report copies that append merges today.
const REPORT_PAIRS_THAT_APPEND: [(&str, &str); 4] = [
    ("libreoffice", "google"),
    ("libreoffice", "authored"),
    ("authored", "google"),
    ("authored", "libreoffice"),
];

/// Appends one producer's report to another's: the body texts after a save,
/// and the texts of both reports one after the other.
fn append_report(destination: &str, source: &str) -> (Vec<String>, Vec<String>) {
    let source = corpus_doc(&format!("{source}/report.docx"));
    let mut doc = corpus_doc(&format!("{destination}/report.docx"));
    let mut expected = texts(&doc);
    expected.extend(texts(&source));
    doc.append(&source);
    (texts(&reopen(&mut doc)), expected)
}

#[test]
fn appending_a_report_saved_by_another_producer_keeps_both_texts() {
    let results = map_in_parallel(&REPORT_PAIRS_THAT_APPEND, |(destination, source)| {
        append_report(destination, source)
    });
    for (pair, result) in REPORT_PAIRS_THAT_APPEND.iter().zip(results) {
        let (found, expected) =
            result.unwrap_or_else(|| panic!("{} <- {}: append panicked", pair.0, pair.1));
        assert_eq!(found, expected, "{} <- {}", pair.0, pair.1);
    }
}

// Fails today: append merges styles by id only. With a French Word file,
// "Default Paragraph Font" (Policepardfaut) and "Normal Table" (TableauNormal)
// become a second default of their type next to the English ids; into a
// Google file, Heading1Char comes in linked to a Heading1 that does not link
// back. The style graph check then panics. Issue: #195
#[test]
#[should_panic(expected = "append fails for these destination <- source pairs")]
fn appending_any_other_pair_of_reports_keeps_both_texts() {
    let producers = ["word", "google", "libreoffice", "authored"];
    let pairs: Vec<_> = producers
        .iter()
        .flat_map(|destination| {
            producers
                .iter()
                .filter(move |source| *source != destination)
                .map(move |source| (*destination, *source))
        })
        .filter(|pair| !REPORT_PAIRS_THAT_APPEND.contains(pair))
        .collect();
    assert_eq!(pairs.len(), 8);
    let results = map_in_parallel(&pairs, |(destination, source)| {
        append_report(destination, source)
    });
    let failed: Vec<_> = pairs
        .iter()
        .zip(&results)
        .filter(|(_, result)| !matches!(result, Some((found, expected)) if found == expected))
        .map(|(pair, _)| format!("{} <- {}", pair.0, pair.1))
        .collect();
    assert!(
        failed.is_empty(),
        "append fails for these destination <- source pairs: {failed:?}"
    );
}
