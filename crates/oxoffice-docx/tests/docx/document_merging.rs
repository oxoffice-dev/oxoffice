//! Combining documents: `append`, `append_with_break` with each break kind,
//! `insert_document` at a body index, `import_fragment` of a range of another
//! document under each conflict policy (from and into headers, footers and
//! table cells too), and `insert_content` of a paragraph
//! taken from another document, with what follows the content: styles, lists,
//! pictures, links, notes, comments, bookmarks, controls, revisions, sections.

use std::collections::HashSet;
use std::io::Cursor;

use oxoffice_docx::{
    CommentDate, ContentLocation, Document, DocumentFragment, FragmentConflictPolicy, HdrFtrType,
    HeaderFooterKind, Length, ListLevel, RunItemRef, SectionBreak, StoryId, StoryItemKind,
    StoryKind, StyleBuilder, StyleType, TextCommentOptions, ValidationSeverity,
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

#[test]
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

#[test]
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

#[test]
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

#[test]
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

#[test]
fn appending_drops_the_other_documents_final_section_and_keeps_the_destination_s() {
    let mut source = doc_with(&["wide"]);
    source.set_page_size(Length::mm(297.0), Length::mm(210.0));
    source.set_landscape();
    source.set_margins(
        Length::inches(0.3),
        Length::inches(0.3),
        Length::inches(0.3),
        Length::inches(0.3),
    );
    source.set_header("Source header");
    let source = reopen(&mut source);
    let mut doc = doc_with(&["dest"]);
    doc.set_header("Dest header");
    let mut doc = reopen(&mut doc);
    let layout = |doc: &Document| {
        let section = doc.section(0).expect("a first section");
        (
            section.page_size(),
            section.orientation(),
            section.margins(),
        )
    };
    let before = layout(&doc);
    assert_ne!(layout(&source), before, "the two layouts differ");

    doc.append(&source);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["dest", "wide"]);
    assert_eq!(doc.section_count(), 1, "no section of the source comes in");
    assert_eq!(layout(&doc), before, "the destination's page layout stays");
    assert_eq!(header_shown(&doc, 0).as_deref(), Some("Dest header"));
    let headers: Vec<_> = doc
        .stories()
        .unwrap()
        .into_iter()
        .filter(|story| story.kind() == StoryKind::Header)
        .map(|story| story_texts(&doc, &story))
        .collect();
    assert_eq!(
        headers,
        [["Dest header"]],
        "the source's header is not carried"
    );
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
        // The section mark goes on the destination's last paragraph.
        assert_eq!(texts(&doc), ["dest", "source"], "{kind:?}");
        assert_eq!(saved(&mut source), before, "{kind:?}: source unchanged");
    }
}

#[test]
fn each_break_kind_starts_the_appended_section_as_asked() {
    let mut wrong = Vec::new();
    for (kind, expected) in [
        (SectionBreak::NextPage, ST_SectionType::NextPage),
        (SectionBreak::Continuous, ST_SectionType::Continuous),
        (SectionBreak::EvenPage, ST_SectionType::EvenPage),
        (SectionBreak::OddPage, ST_SectionType::OddPage),
    ] {
        let mut doc = doc_with(&["dest"]);
        doc.append_with_break(&doc_with(&["source"]), kind);
        let doc = reopen(&mut doc);
        assert_eq!(texts(&doc), ["dest", "source"], "{kind:?}");
        assert_eq!(doc.section_count(), 2, "{kind:?}");
        let started = doc.section(1).unwrap().break_type();
        if started.unwrap_or(ST_SectionType::NextPage) != expected {
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

#[test]
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
    let appended = doc.section(1).unwrap();
    assert_eq!(
        (
            appended.page_size(),
            appended.margins(),
            header_shown(&doc, 1)
        ),
        (size, margins, Some("Dest header".to_owned())),
        "the appended section takes the same layout and header"
    );
}

#[test]
fn appending_with_a_break_starts_the_first_appended_section_as_asked() {
    // The source brings two sections (cover | body): the break kind goes on
    // the first one, and the last one starts as it did in the source.
    let mut source = with_inner_section_header();
    source
        .section_mut(1)
        .unwrap()
        .set_break_type(ST_SectionType::EvenPage);
    let source = reopen(&mut source);
    let mut doc = doc_with(&["dest"]);
    doc.append_with_break(&source, SectionBreak::Continuous);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["dest", "cover", "", "body"]);
    assert_eq!(doc.section_count(), 3);
    assert_eq!(
        doc.section(1).unwrap().break_type(),
        Some(ST_SectionType::Continuous)
    );
    assert_eq!(
        doc.section(2).unwrap().break_type(),
        Some(ST_SectionType::EvenPage)
    );
}

#[test]
fn appending_with_a_break_marks_the_section_on_the_last_paragraph_or_after_a_table() {
    // As Word does: no empty paragraph after a paragraph, one after a table,
    // which cannot hold a section mark.
    let mut doc = doc_with(&["dest"]);
    doc.append_with_break(&doc_with(&["source"]), SectionBreak::Continuous);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["dest", "source"]);
    assert_eq!(doc.section_count(), 2);
    assert_eq!(
        doc.layout_deterministic().unwrap().layout.pages.len(),
        1,
        "a continuous break keeps one page"
    );

    let mut doc = doc_with(&["dest"]);
    doc.add_table(1, 1).cell(0, 0).unwrap().set_text("cell");
    let mut doc = reopen(&mut doc);
    let before = doc.content_count();
    doc.append_with_break(&doc_with(&["source"]), SectionBreak::NextPage);
    let doc = reopen(&mut doc);
    assert_eq!(
        doc.content_count(),
        before + 2,
        "a mark paragraph, then the source"
    );
    assert_eq!(doc.section_count(), 2);
}

#[test]
fn appending_with_a_break_after_a_tracked_paragraph_mark_keeps_the_mark_paragraph() {
    // Rejecting the tracked mark joins "dest" to the next paragraph: the
    // section mark must not ride on it, or the destination's section goes.
    let mut doc = doc_with_body(concat!(
        r#"<w:p><w:pPr><w:rPr><w:ins w:id="1" w:author="A" w:date="2026-11-21T00:00:00Z"/></w:rPr></w:pPr>"#,
        r#"<w:r><w:t>dest</w:t></w:r></w:p>"#,
    ));
    let mut doc = reopen(&mut doc);
    doc.append_with_break(&doc_with(&["source"]), SectionBreak::OddPage);
    let mut doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["dest", "", "source"]);
    doc.reject_all().unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(doc.section_count(), 2, "the destination's section stays");
}

#[test]
fn appending_with_a_break_to_a_document_ending_with_a_section_mark_keeps_its_layout() {
    // No final body section properties, but the last paragraph ends an A4
    // section: those become the final ones, and the appended section is A4.
    let a4 = (Length::mm(210.0), Length::mm(297.0));
    let mut doc = doc_with(&["dest"]);
    doc.set_page_size(a4.0, a4.1);
    let bytes = doc.to_bytes().unwrap();
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    let main = String::from_utf8(package.get_part("/word/document.xml").unwrap().to_vec()).unwrap();
    let start = main.find("<w:sectPr").unwrap();
    let end = main.find("</w:sectPr>").unwrap() + "</w:sectPr>".len();
    let sect_pr = main[start..end].to_owned();
    let main = format!("{}{}", &main[..start], &main[end..]);
    let close = main.rfind("</w:p>").unwrap();
    let paragraph = main[..close]
        .rfind("<w:p>")
        .map(|at| at + "<w:p>".len())
        .unwrap();
    let main = format!(
        "{}<w:pPr>{sect_pr}</w:pPr>{}",
        &main[..paragraph],
        &main[paragraph..]
    );
    package.set_part("/word/document.xml", main.into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let mut doc = Document::from_bytes(&out.into_inner()).unwrap();
    let size = doc.section(0).unwrap().page_size();
    doc.append_with_break(&doc_with(&["source"]), SectionBreak::NextPage);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["dest", "source"]);
    assert_eq!(doc.section_count(), 2);
    assert_eq!(doc.section(0).unwrap().page_size(), size);
    assert_eq!(doc.section(1).unwrap().page_size(), size);
}

#[test]
fn appending_with_a_break_to_a_document_without_final_section_properties_adds_them() {
    let bytes = doc_with(&["dest"]).to_bytes().unwrap();
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    let main = String::from_utf8(package.get_part("/word/document.xml").unwrap().to_vec()).unwrap();
    let start = main
        .find("<w:sectPr")
        .expect("a new document has a final sectPr");
    let end = main.find("</w:sectPr>").expect("closed") + "</w:sectPr>".len();
    package.set_part(
        "/word/document.xml",
        format!("{}{}", &main[..start], &main[end..]).into_bytes(),
    );
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let mut doc = Document::from_bytes(&out.into_inner()).unwrap();
    doc.append_with_break(&doc_with(&["source"]), SectionBreak::OddPage);
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["dest", "source"]);
    assert_eq!(doc.section_count(), 2);
    assert_eq!(doc.section(0).unwrap().break_type(), None);
    assert_eq!(
        doc.section(1).unwrap().break_type(),
        Some(ST_SectionType::OddPage)
    );
}

#[test]
fn the_appended_section_takes_the_destination_s_section_formatting_but_its_page_restart() {
    let mut doc = doc_with(&["dest"]);
    doc.set_first_page_header("First page");
    doc.set_header("Other pages");
    {
        let mut section = doc.section_mut(0).unwrap();
        section.set_different_first_page(true);
        let properties = section.properties_mut();
        properties.columns = Some(oxoffice_docx_oxml::document::CT_Columns {
            num: Some(2),
            ..Default::default()
        });
        properties.page_number = Some(oxoffice_docx_oxml::document::CT_PageNumberType::new(5));
    }
    let mut doc = reopen(&mut doc);
    doc.append_with_break(&doc_with(&["source"]), SectionBreak::NextPage);
    let doc = reopen(&mut doc);
    let appended = doc.section(1).unwrap().properties().clone();
    assert_eq!(appended.title_pg, Some(true), "first-page variants stay on");
    assert_eq!(appended.columns.and_then(|columns| columns.num), Some(2));
    assert_eq!(
        appended.page_number.and_then(|number| number.start),
        None,
        "the page numbers go on"
    );
    assert!(appended.header_refs.is_empty(), "the headers are inherited");
    let first = doc
        .section_story(1, HeaderFooterKind::Header, HdrFtrType::First)
        .unwrap()
        .expect("a first-page header shows");
    let text: Vec<_> = doc
        .story_items(first.story())
        .unwrap()
        .iter()
        .map(|item| item.text().unwrap().unwrap_or_default())
        .collect();
    assert_eq!(text, ["First page"]);
    assert_eq!(
        doc.section(0)
            .unwrap()
            .properties()
            .page_number
            .as_ref()
            .and_then(|n| n.start),
        Some(5),
        "the destination keeps its restart"
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
fn an_inserted_document_takes_the_destination_s_font_unless_asked_to_keep_its_own() {
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

    // As Word's Insert > Text from File: the destination's look.
    doc.insert_document(1, &source);
    let inserted = reopen(&mut doc);
    assert_eq!(texts(&inserted), ["dest", "source"]);
    assert_eq!(font(&inserted, 0), own, "the destination keeps its font");
    assert_eq!(font(&inserted, 1), own, "the inserted text takes it too");

    // A policy that does not keep the destination's formatting keeps the
    // source's effective font.
    let mut doc = doc_with(&["dest"]);
    let start = source.direct_body_item_location(0).unwrap().unwrap();
    let end = ContentLocation::end(start.story().clone());
    let fragment =
        oxoffice_docx::DocumentFragment::from_range(&source, &start, &end, false).unwrap();
    let body = doc
        .stories()
        .unwrap()
        .into_iter()
        .find(|story| story.kind() == StoryKind::Body)
        .unwrap();
    doc.import_fragment(
        &ContentLocation::end(body),
        &fragment,
        oxoffice_docx::FragmentConflictPolicy::for_insert_document(Some(
            oxoffice_docx::FragmentConflictPolicy::reuse_equivalent().with_numbering_reuse(false),
        )),
    )
    .unwrap();
    let kept = reopen(&mut doc);
    assert_eq!(font(&kept, 0), own);
    let georgia = (
        Some("Georgia".to_owned()),
        Some(oxoffice_docx::HalfPoint(28)),
    );
    assert_eq!(font(&kept, 1), georgia);

    // The builder may come after for_insert_document too.
    let mut doc = doc_with(&["dest"]);
    let body = doc
        .stories()
        .unwrap()
        .into_iter()
        .find(|story| story.kind() == StoryKind::Body)
        .unwrap();
    doc.import_fragment(
        &ContentLocation::end(body),
        &fragment,
        oxoffice_docx::FragmentConflictPolicy::for_insert_document(None)
            .with_destination_formatting(false),
    )
    .unwrap();
    assert_eq!(font(&reopen(&mut doc), 1), georgia);
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

#[test]
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

#[test]
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

#[test]
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

/// The raw XML of a header (or footer) in style `Banner` holding an item of
/// list 1, with the text "Source story".
fn styled_list_story(footer: bool) -> Vec<u8> {
    let root = if footer { "ftr" } else { "hdr" };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:{root} xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:p><w:pPr><w:pStyle w:val="Banner"/><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Source story</w:t></w:r></w:p></w:{root}>"#
    )
    .into_bytes()
}

/// A saved two-section document whose first section has a header (or a
/// footer) in style `Banner` (`color`) holding an item of a numbered list,
/// list 1, that nothing in the body uses, and whose final section holds
/// `body`.
fn with_styled_list_header(color: &str, footer: bool) -> Document {
    let mut doc = doc_with(&["cover"]);
    doc.add_style(StyleBuilder::paragraph("Banner", "Banner").color(color))
        .expect("style is valid");
    doc.add_numbered_list_item("listed", 0);
    let listed = item_at(&doc, 1);
    doc.remove_content_at(&listed).expect("the list item goes");
    assert_eq!(
        doc.numbering_is_bullet(1),
        Some(false),
        "the list is list 1"
    );
    if footer {
        doc.set_raw_footer_with_images(styled_list_story(true), &[], HdrFtrType::Default);
    } else {
        doc.set_raw_header_with_images(styled_list_story(false), &[], HdrFtrType::Default);
    }
    doc.insert_section(1).expect("a section appends");
    doc.add_paragraph("body");
    reopen(&mut doc)
}

/// The run color style `id` defines, if the style exists.
fn banner_color(doc: &Document, id: &str) -> Option<String> {
    doc.style(id)?.run_properties()?.color.clone()
}

/// The header and footer parts of the saved `doc` holding "Source story".
fn source_stories(doc: &mut Document) -> Vec<String> {
    let package = OpcPackage::from_reader(Cursor::new(saved(doc))).expect("package opens");
    package
        .parts
        .iter()
        .filter(|(name, _)| name.contains("header") || name.contains("footer"))
        .map(|(_, bytes)| String::from_utf8_lossy(bytes).into_owned())
        .filter(|xml| xml.contains("Source story"))
        .collect()
}

/// The `w:val` of the first `element` in `xml`.
fn word_value(xml: &str, element: &str) -> Option<String> {
    let start = xml.find(&format!("<w:{element} w:val=\""))? + element.len() + 11;
    Some(xml[start..start + xml[start..].find('"')?].to_owned())
}

/// The `w:val` of the first `element` in the saved header or footer part
/// showing "Source story".
fn source_header_value(doc: &mut Document, element: &str) -> Option<String> {
    let stories = source_stories(doc);
    let [story] = stories.as_slice() else {
        panic!("one part holds the source story: {stories:?}");
    };
    word_value(story, element)
}

#[test]
fn an_imported_header_keeps_its_style_and_list_however_the_document_comes_in() {
    for footer in [false, true] {
        for entry in ["append", "insert_document", "import_fragment"] {
            let entry = format!("{entry}, footer {footer}");
            let source = with_styled_list_header("C00000", footer);
            let mut doc = doc_with(&["dest"]);
            let mut doc = reopen(&mut doc);
            if entry.starts_with("append") {
                doc.append(&source);
            } else if entry.starts_with("insert_document") {
                doc.insert_document(1, &source);
            } else {
                doc.import_fragment(
                    &ContentLocation::end(body(&doc)),
                    &whole_body(&source),
                    FragmentConflictPolicy::default(),
                )
                .expect("the fragment imports");
            }
            let mut doc = reopen(&mut doc);
            let style = source_header_value(&mut doc, "pStyle").expect("the part has a style");
            assert_eq!(style, "Banner", "{entry}: the part keeps its style id");
            assert_eq!(
                banner_color(&doc, &style).as_deref(),
                Some("C00000"),
                "{entry}: the style is defined as in the source"
            );
            let num_id: u32 = source_header_value(&mut doc, "numId")
                .expect("the part has a list")
                .parse()
                .unwrap();
            assert_eq!(
                doc.numbering_is_bullet(num_id),
                Some(false),
                "{entry}: the part's numbered list is defined"
            );
            let errors: Vec<_> = doc
                .validate()
                .unwrap()
                .findings
                .into_iter()
                .filter(|finding| finding.severity == ValidationSeverity::Error)
                .collect();
            assert!(errors.is_empty(), "{entry}: {errors:?}");
        }
    }
}

#[test]
fn an_imported_header_holding_a_list_never_shares_an_equal_destination_header() {
    // The destination's header has the same bytes, but its list 1 is the
    // destination's bullets: sharing the part would renumber one of them.
    let source = with_styled_list_header("C00000", false);
    let mut doc = doc_with(&["dest"]);
    doc.add_style(StyleBuilder::paragraph("Banner", "Banner").color("C00000"))
        .expect("style is valid");
    doc.add_bullet_list_item("dest item", 0);
    doc.set_raw_header_with_images(styled_list_story(false), &[], HdrFtrType::Default);
    let mut doc = reopen(&mut doc);
    assert_eq!(doc.numbering_is_bullet(1), Some(true));
    let mut source_copy = with_styled_list_header("C00000", false);
    assert_eq!(
        source_stories(&mut doc),
        source_stories(&mut source_copy),
        "the two headers have the same bytes"
    );
    doc.import_fragment(
        &ContentLocation::end(body(&doc)),
        &whole_body(&source),
        FragmentConflictPolicy::reuse_equivalent(),
    )
    .expect("the fragment imports");
    let mut doc = reopen(&mut doc);
    let mut lists: Vec<u32> = source_stories(&mut doc)
        .iter()
        .map(|xml| word_value(xml, "numId").unwrap().parse().unwrap())
        .collect();
    lists.sort();
    let [own, imported] = lists[..] else {
        panic!("two header parts: {lists:?}");
    };
    assert_eq!(own, 1, "the destination's header keeps its bullets");
    assert_eq!(doc.numbering_is_bullet(own), Some(true));
    assert_eq!(doc.numbering_is_bullet(imported), Some(false));
}

#[test]
fn an_imported_header_follows_a_renamed_style_and_a_renumbered_list() {
    for entry in ["append", "insert_document", "import_fragment"] {
        let source = with_styled_list_header("C00000", false);
        let mut doc = doc_with(&["dest"]);
        doc.add_style(StyleBuilder::paragraph("Banner", "Banner").color("0000FF"))
            .expect("style is valid");
        doc.add_bullet_list_item("dest item", 0);
        let mut doc = reopen(&mut doc);
        assert_eq!(doc.numbering_is_bullet(1), Some(true));
        match entry {
            "append" => doc.append(&source),
            "insert_document" => doc.insert_document(2, &source),
            _ => doc
                .import_fragment(
                    &ContentLocation::end(body(&doc)),
                    &whole_body(&source),
                    FragmentConflictPolicy::default(),
                )
                .map(|_| ())
                .expect("the fragment imports"),
        }
        let mut doc = reopen(&mut doc);
        // A whole document takes the destination's look, as Word's Insert >
        // Text from File does; a fragment brings the source's under a new id.
        let style = source_header_value(&mut doc, "pStyle").expect("the header has a style");
        let expected = if entry == "import_fragment" {
            assert_ne!(
                style, "Banner",
                "{entry}: the destination keeps its own Banner"
            );
            "C00000"
        } else {
            assert_eq!(
                style, "Banner",
                "{entry}: the header takes the destination's Banner"
            );
            "0000FF"
        };
        assert_eq!(
            banner_color(&doc, &style).as_deref(),
            Some(expected),
            "{entry}"
        );
        assert_eq!(
            banner_color(&doc, "Banner").as_deref(),
            Some("0000FF"),
            "{entry}"
        );
        let num_id: u32 = source_header_value(&mut doc, "numId")
            .expect("the header has a list")
            .parse()
            .unwrap();
        assert_ne!(num_id, 1, "{entry}: list 1 is the destination's bullets");
        assert_eq!(doc.numbering_is_bullet(num_id), Some(false), "{entry}");
        assert_eq!(doc.numbering_is_bullet(1), Some(true), "{entry}");
    }
}

#[test]
fn a_word_report_inserted_into_another_keeps_the_styles_of_its_headers_and_footers() {
    let source = corpus_doc("libreoffice/report.docx");
    let mut doc = corpus_doc("google/report.docx");
    let end = doc.story_items(&body(&doc)).unwrap().len();
    doc.insert_document(end, &source);
    let doc = reopen(&mut doc);
    let undefined: Vec<_> = doc
        .validate()
        .unwrap()
        .findings
        .into_iter()
        .filter(|finding| finding.message.contains("is not defined"))
        .collect();
    assert!(undefined.is_empty(), "{undefined:?}");
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
fn every_reference_to_a_renamed_bookmark_follows_it_whatever_its_kind_or_case() {
    let field = |instruction: &str, result: &str| {
        format!(
            concat!(
                r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
                r#"<w:r><w:instrText xml:space="preserve"> {} </w:instrText></w:r>"#,
                r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>{}</w:t></w:r>"#,
                r#"<w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#
            ),
            instruction, result
        )
    };
    // Word compares bookmark names without regard to case.
    let source = doc_with_body(&format!(
        concat!(
            r#"<w:p><w:bookmarkStart w:id="0" w:name="Mark"/><w:r><w:t>src marked</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#,
            "{}{}{}{}{}{}",
            r#"<w:p><w:hyperlink w:anchor="MARK"><w:r><w:t>src jump</w:t></w:r></w:hyperlink></w:p>"#,
        ),
        field(r#"REF mark \h"#, "src marked"),
        field(r#"HYPERLINK \l "Mark""#, "link"),
        field(
            r#"HYPERLINK "https://example.com/Mark" \l "Mark""#,
            "address"
        ),
        field(r#"NOTEREF Mark \h"#, "1"),
        format!(
            "{}{}{}",
            field(r#"GOTOBUTTON Mark "Go""#, "Go"),
            field(r#"SET Mark "value""#, ""),
            field(r#"TOC \o "1-3" \b Mark"#, "contents"),
        ),
        format!(
            "{}{}",
            field(r#"XE "Entry" \r Mark"#, ""),
            field(r#"INDEX \b Mark"#, "index"),
        ),
    ));
    let mut doc = with_identities("dst");
    import_at_end(&mut doc, &source, FragmentConflictPolicy::default()).unwrap();
    let doc = reopen(&mut doc);

    let bookmarks = bookmark_list(&doc);
    let renamed = bookmarks
        .iter()
        .find(|(_, text)| text == "src marked")
        .map(|(name, _)| name.clone())
        .unwrap();
    assert!(!renamed.eq_ignore_ascii_case("Mark"), "{bookmarks:?}");
    assert_eq!(
        field_instructions(&doc),
        [
            "REF Mark \\h".to_owned(),
            format!("REF {renamed} \\h"),
            format!("HYPERLINK \\l \"{renamed}\""),
            // With an address, \l names a place in that address: left alone.
            "HYPERLINK \"https://example.com/Mark\" \\l \"Mark\"".to_owned(),
            format!("NOTEREF {renamed} \\h"),
            format!("GOTOBUTTON {renamed} \"Go\""),
            format!("SET {renamed} \"value\""),
            format!("TOC \\o \"1-3\" \\b {renamed}"),
            format!("XE \"Entry\" \\r {renamed}"),
            format!("INDEX \\b {renamed}"),
        ]
    );
    let anchors: Vec<_> = doc
        .links()
        .into_iter()
        .filter(|link| link.text == "src jump")
        .map(|link| link.anchor)
        .collect();
    assert_eq!(anchors, [Some(renamed)]);
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

#[test]
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
    assert_eq!(
        color_of(&doc, 0).as_deref(),
        Some("0000FF"),
        "the destination's"
    );
    assert_eq!(color_of(&doc, 1).as_deref(), Some("C00000"), "the source's");
    doc.validate_style_graph().unwrap();
}

// ---- import_fragment into and from other stories ----

/// The first story of `kind` in `doc`.
fn story_of(doc: &Document, kind: StoryKind) -> StoryId {
    doc.stories()
        .expect("stories are listed")
        .into_iter()
        .find(|story| story.kind() == kind)
        .expect("the document has a story of this kind")
}

/// The text of each paragraph of `story`, a table as `[table]`.
fn story_texts(doc: &Document, story: &StoryId) -> Vec<String> {
    doc.story_items(story)
        .expect("story items are listed")
        .iter()
        .filter_map(|item| match item.kind() {
            StoryItemKind::Table => Some("[table]".to_owned()),
            StoryItemKind::Paragraph => Some(item.text().expect("item text").unwrap_or_default()),
            _ => None,
        })
        .collect()
}

/// The addresses of the links of `story`.
fn story_link_urls(doc: &Document, story: &StoryId) -> Vec<Option<String>> {
    doc.story_items(story)
        .expect("story items are listed")
        .iter()
        .flat_map(|item| item.links().expect("item links"))
        .map(|link| link.url)
        .collect()
}

/// A saved document of two paragraphs: `source`, then the one-pixel picture
/// followed by a link to `https://source.example/`.
fn with_picture_and_link() -> Document {
    let mut source = with_picture("source");
    source.append_hyperlink("source site", "https://source.example/");
    reopen(&mut source)
}

#[test]
fn a_fragment_imported_into_a_header_or_a_footer_lands_there_with_its_picture_and_link() {
    let source = with_picture_and_link();
    let mut doc = doc_with(&["dest"]);
    doc.set_header("Head");
    doc.set_footer("Foot");
    let destination = saved(&mut reopen(&mut doc));
    for (kind, own) in [(StoryKind::Header, "Head"), (StoryKind::Footer, "Foot")] {
        let mut doc = Document::from_bytes(&destination).unwrap();
        let end = ContentLocation::end(story_of(&doc, kind));
        doc.import_fragment(
            &end,
            &whole_body(&source),
            FragmentConflictPolicy::default(),
        )
        .unwrap_or_else(|error| panic!("{kind:?}: {error}"));
        let doc = reopen(&mut doc);
        let story = story_of(&doc, kind);
        assert_eq!(
            story_texts(&doc, &story),
            [own, "source", "source site"],
            "{kind:?}"
        );
        assert_eq!(texts(&doc), ["dest"], "{kind:?}: the body is unchanged");
        let (other_kind, other_own) = if kind == StoryKind::Header {
            (StoryKind::Footer, "Foot")
        } else {
            (StoryKind::Header, "Head")
        };
        let other = story_of(&doc, other_kind);
        assert_eq!(
            story_texts(&doc, &other),
            [other_own],
            "{kind:?}: the other story is unchanged"
        );
        let pictures: Vec<_> = doc
            .pictures()
            .unwrap()
            .into_iter()
            .map(|picture| (picture.location.story().kind(), picture.bytes))
            .collect();
        assert_eq!(pictures, [(kind, PNG.to_vec())], "{kind:?}: the picture");
        assert_eq!(
            story_link_urls(&doc, &story),
            [Some("https://source.example/".to_owned())],
            "{kind:?}: the link"
        );
        let errors: Vec<_> = doc
            .validate()
            .unwrap()
            .findings
            .into_iter()
            .filter(|finding| finding.severity == ValidationSeverity::Error)
            .collect();
        assert!(errors.is_empty(), "{kind:?}: {errors:?}");
    }
}

#[test]
fn a_fragment_imported_into_a_table_cell_lands_in_that_cell_only() {
    let source = doc_with(&["one", "two"]);
    let mut doc = doc_with(&["before"]);
    {
        let mut table = doc.add_table(1, 2);
        table.cell(0, 0).unwrap().set_text("left");
        table.cell(0, 1).unwrap().set_text("right");
    }
    doc.add_paragraph("after");
    let mut doc = reopen(&mut doc);
    let body_before = body_items(&doc);
    let cell = story_of(&doc, StoryKind::TableCell);
    doc.import_fragment(
        &ContentLocation::end(cell),
        &whole_body(&source),
        FragmentConflictPolicy::default(),
    )
    .unwrap();
    let doc = reopen(&mut doc);
    let cells: Vec<_> = doc
        .stories()
        .unwrap()
        .into_iter()
        .filter(|story| story.kind() == StoryKind::TableCell)
        .map(|story| story_texts(&doc, &story))
        .collect();
    assert_eq!(cells, [vec!["left", "one", "two"], vec!["right"]]);
    assert_eq!(body_items(&doc), body_before, "the body keeps its items");
}

#[test]
fn a_fragment_taken_from_a_header_or_a_table_cell_imports_into_the_body() {
    let mut source = doc_with(&["source body"]);
    source.set_header("Header line");
    source
        .add_table(1, 1)
        .cell(0, 0)
        .unwrap()
        .set_text("Cell line");
    let mut source = reopen(&mut source);
    let before = saved(&mut source);
    let mut doc = doc_with(&["first", "last"]);
    for kind in [StoryKind::Header, StoryKind::TableCell] {
        let story = story_of(&source, kind);
        let start = source.story_items(&story).unwrap()[0].location().clone();
        let fragment =
            DocumentFragment::from_range(&source, &start, &ContentLocation::end(story), false)
                .unwrap_or_else(|error| panic!("{kind:?}: {error}"));
        let at = item_at(&doc, doc.content_count() - 1);
        doc.import_fragment(&at, &fragment, FragmentConflictPolicy::default())
            .unwrap_or_else(|error| panic!("{kind:?}: {error}"));
    }
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["first", "Header line", "Cell line", "last"]);
    assert_eq!(saved(&mut source), before, "the source is left as it was");
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

#[test]
fn a_paragraph_from_another_document_never_takes_the_destinations_link() {
    let mut source = with_link("source site", "https://source.example/");
    let first = item_at(&source, 0);
    let fragment = source.remove_content_at(&first).unwrap();
    let mut doc = with_link("dest site", "https://dest.example/");
    let before = doc.to_bytes().unwrap();
    match doc.insert_content(&ContentLocation::end(body(&doc)), fragment) {
        Ok(()) => {
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
        Err(error) => {
            // The relationship id names the destination's own link here.
            assert!(error.to_string().contains("where it was taken"), "{error}");
            assert_eq!(doc.to_bytes().unwrap(), before, "a refusal changes nothing");
        }
    }
}

#[test]
fn a_paragraph_with_a_link_cut_and_pasted_in_its_document_keeps_its_address() {
    let mut doc = with_link("our site", "https://example.com/");
    doc.add_paragraph("after");
    let first = item_at(&doc, 0);
    let cut = doc.remove_content_at(&first).unwrap();
    // A save in between renames nothing a link names.
    let mut doc = reopen(&mut doc);
    doc.insert_content(&ContentLocation::end(body(&doc)), cut)
        .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(
        link_targets(&doc),
        [link("our site", "https://example.com/")]
    );
    assert_eq!(texts(&doc).last().map(String::as_str), Some("our site"));
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

#[test]
fn the_contract_imported_into_itself_answers_each_imported_reply_with_an_imported_comment() {
    let source = corpus_doc("authored/contract.docx");
    let original = comment_list(&source);
    let replies = original.iter().filter(|c| c.3.is_some()).count();
    assert!(replies > 0, "the contract has replies: {original:?}");
    let mut doc = corpus_doc("authored/contract.docx");
    import_at_end(&mut doc, &source, FragmentConflictPolicy::default()).unwrap();
    let doc = reopen(&mut doc);
    let merged = comment_list(&doc);
    let original_ids: HashSet<i32> = original.iter().map(|c| c.0).collect();
    // The destination's own comments keep their ids, text and thread.
    let kept: Vec<_> = merged
        .iter()
        .filter(|c| original_ids.contains(&c.0))
        .cloned()
        .collect();
    assert_eq!(kept, original);
    let imported: Vec<_> = merged
        .iter()
        .filter(|c| !original_ids.contains(&c.0))
        .collect();
    assert_eq!(imported.len(), original.len(), "{merged:?}");
    let imported_ids: HashSet<i32> = imported.iter().map(|c| c.0).collect();
    let imported_replies: Vec<_> = imported.iter().filter(|c| c.3.is_some()).collect();
    assert_eq!(imported_replies.len(), replies);
    for reply in imported_replies {
        let parent = reply.3.unwrap();
        assert!(
            imported_ids.contains(&parent),
            "imported reply {} answers comment {parent}, not an imported one",
            reply.0
        );
        // The parent is the imported copy of the source's parent.
        let source_reply = original
            .iter()
            .find(|c| (&c.1, &c.2) == (&reply.1, &reply.2) && c.3.is_some())
            .expect("the reply comes from the source");
        let source_parent = original
            .iter()
            .find(|c| Some(c.0) == source_reply.3)
            .unwrap();
        let imported_parent = imported.iter().find(|c| c.0 == parent).unwrap();
        assert_eq!(
            (&imported_parent.1, &imported_parent.2),
            (&source_parent.1, &source_parent.2)
        );
    }
}

#[test]
fn the_form_imported_into_itself_holds_both_copies_of_its_bookmarks() {
    let source = corpus_doc("authored/form.docx");
    let original = bookmark_list(&source);
    assert_eq!(original.len(), 6, "{original:?}");
    let mut doc = corpus_doc("authored/form.docx");
    import_at_end(&mut doc, &source, FragmentConflictPolicy::default()).unwrap();
    let doc = reopen(&mut doc);
    let merged = bookmark_list(&doc);
    assert_eq!(merged.len(), 12, "6 + 6 bookmarks: {merged:?}");
    let names: Vec<_> = merged.iter().map(|b| b.0.clone()).collect();
    assert!(all_distinct(&names), "{names:?}");
    assert_eq!(merged[..6], original[..], "the originals stay first");
    let texts_of = |list: &[(String, String)]| list.iter().map(|b| b.1.clone()).collect::<Vec<_>>();
    assert_eq!(
        texts_of(&merged[6..]),
        texts_of(&original),
        "the imported copies mark the same texts"
    );
}

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
    let producers = ["word", "google", "libreoffice", "authored"];
    let pairs: Vec<_> = producers
        .iter()
        .flat_map(|destination| {
            producers
                .iter()
                .filter(move |source| *source != destination)
                .map(move |source| (*destination, *source))
        })
        .collect();
    assert_eq!(pairs.len(), 12);
    let results = map_in_parallel(&pairs, |(destination, source)| {
        append_report(destination, source)
    });
    for (pair, result) in pairs.iter().zip(results) {
        let (found, expected) =
            result.unwrap_or_else(|| panic!("{} <- {}: append panicked", pair.0, pair.1));
        assert_eq!(found, expected, "{} <- {}", pair.0, pair.1);
    }
}

#[test]
fn a_localized_default_style_takes_the_destination_s_default() {
    // Word's French file names the default character, table and numbering
    // styles Policepardfaut, TableauNormal and Aucuneliste; a type keeps one
    // default. The authored report has the same defaults under the English
    // ids and no style graph defect of its own.
    let source = corpus_doc("word/report.docx");
    for id in ["Policepardfaut", "TableauNormal", "Aucuneliste"] {
        assert!(source.style(id).is_some(), "{id}");
    }
    let policies = [
        ("append", None),
        (
            "reuse_equivalent",
            Some(FragmentConflictPolicy::reuse_equivalent()),
        ),
        ("rename_all", Some(FragmentConflictPolicy::rename_all())),
    ];
    for (name, policy) in policies {
        let mut doc = corpus_doc("authored/report.docx");
        assert_eq!(
            doc.validate_style_graph().map_err(|e| e.to_string()),
            Ok(())
        );
        match policy {
            None => doc.append(&source),
            Some(policy) => import_at_end(&mut doc, &source, policy).unwrap(),
        }
        let doc = reopen(&mut doc);
        assert_eq!(
            doc.validate_style_graph().map_err(|e| e.to_string()),
            Ok(()),
            "{name}: the import adds no style graph defect"
        );
        for id in ["Policepardfaut", "TableauNormal", "Aucuneliste"] {
            assert!(doc.style(id).is_none(), "{name}: {id} maps to the default");
        }
    }
    // LibreOffice writes the same defaults a little differently (an empty
    // run properties element, no table indent): they still map.
    for policy in [
        FragmentConflictPolicy::reuse_equivalent(),
        FragmentConflictPolicy::rename_all(),
    ] {
        let mut doc = corpus_doc("libreoffice/report.docx");
        import_at_end(&mut doc, &source, policy).unwrap();
        let doc = reopen(&mut doc);
        for id in ["Policepardfaut", "TableauNormal", "Aucuneliste"] {
            assert!(
                doc.style(id).is_none(),
                "{policy:?}: {id} maps to the default"
            );
        }
    }
}

/// A document whose default paragraph style has the id `default` (Word in
/// German writes `Standard`, named "Normal"), red when `red`, saved in the
/// editing session `rsid` if given, whose styles
/// part also holds these raw `w:style` elements, and whose body holds this
/// raw XML.
fn with_paragraph_default(
    default: &str,
    red: bool,
    rsid: Option<&str>,
    styles: &str,
    body_xml: &str,
) -> Document {
    let bytes = doc_with_body(body_xml).to_bytes().expect("document saves");
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    let xml = String::from_utf8(package.get_part("/word/styles.xml").unwrap().to_vec())
        .expect("styles are UTF-8");
    let name = r#"<w:name w:val="Normal"/>"#;
    let xml = xml
        .replace(r#"w:val="Normal"/>"#, &format!(r#"w:val="{default}"/>"#))
        .replace(&format!(r#"<w:name w:val="{default}"/>"#), name)
        .replace(
            r#"w:styleId="Normal""#,
            &format!(r#"w:styleId="{default}""#),
        )
        .replacen(
            name,
            &format!(
                "{name}{}{}",
                rsid.map(|rsid| format!(r#"<w:rsid w:val="{rsid}"/>"#))
                    .unwrap_or_default(),
                if red {
                    r#"<w:rPr><w:color w:val="C00000"/></w:rPr>"#
                } else {
                    ""
                }
            ),
            1,
        )
        .replace("</w:styles>", &format!("{styles}</w:styles>"));
    package.set_part("/word/styles.xml", xml.into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).expect("package writes");
    let doc = Document::from_bytes(&out.into_inner()).expect("document opens");
    assert_eq!(paragraph_defaults(&doc), [default]);
    doc
}

/// The ids of the default paragraph styles.
fn paragraph_defaults(doc: &Document) -> Vec<String> {
    doc.styles()
        .into_iter()
        .filter(|style| style.style_type() == StyleType::Paragraph && style.is_default())
        .map(|style| style.style_id().to_owned())
        .collect()
}

/// A paragraph in style `style` holding `text`, as raw body XML.
fn styled_paragraph(style: &str, text: &str) -> String {
    format!(r#"<w:p><w:pPr><w:pStyle w:val="{style}"/></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>"#)
}

/// The style of the body paragraph holding `text`.
fn style_of_text(doc: &Document, text: &str) -> Option<String> {
    (0..doc.paragraph_count())
        .map(|index| doc.paragraph(index).expect("paragraph exists"))
        .find(|paragraph| paragraph.text() == text)
        .unwrap_or_else(|| panic!("a paragraph holds {text:?}"))
        .style_id()
        .map(str::to_owned)
}

const RED_STANDARD: &str = r#"<w:style w:type="paragraph" w:styleId="Standard"><w:name w:val="Standard"/><w:rPr><w:color w:val="C00000"/></w:rPr></w:style>"#;

#[test]
fn a_default_mapped_to_a_destination_style_is_not_renamed_with_the_source_style_of_that_id() {
    // The German destination's default is Standard; the source's default
    // Normal is the same style under another id, and the source has its own
    // red Standard, which is renamed. The default's text lands in Standard,
    // the red text in the renamed copy.
    let body = format!(
        "{}{}",
        styled_paragraph("Normal", "default text"),
        styled_paragraph("Standard", "red text")
    );
    let source = doc_with_styles(RED_STANDARD, &body);
    let mut doc = with_paragraph_default(
        "Standard",
        false,
        None,
        "",
        &styled_paragraph("Standard", "dest"),
    );
    import_at_end(
        &mut doc,
        &source,
        FragmentConflictPolicy::reuse_equivalent(),
    )
    .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(
        style_of_text(&doc, "default text").as_deref(),
        Some("Standard")
    );
    let red = style_of_text(&doc, "red text").expect("the red text has a style");
    assert_ne!(red, "Standard");
    assert_eq!(banner_color(&doc, &red).as_deref(), Some("C00000"));
    assert_eq!(banner_color(&doc, "Standard"), None);
}

/// The style of the paragraph holding `text` in `part` of the saved `doc`.
fn part_paragraph_style(doc: &mut Document, part: &str, text: &str) -> Option<String> {
    let package = OpcPackage::from_reader(Cursor::new(saved(doc))).expect("package opens");
    let xml =
        String::from_utf8_lossy(package.get_part(part).expect("the part exists")).into_owned();
    let at = xml
        .find(text)
        .unwrap_or_else(|| panic!("{part} holds {text:?}"));
    let paragraph = xml[..at].rfind("<w:p>").max(xml[..at].rfind("<w:p "))?;
    word_value(&xml[paragraph..at], "pStyle")
}

#[test]
fn notes_and_comments_take_the_style_renames_in_one_pass_too() {
    // As in the body: the source default Normal maps to the destination's
    // Standard, and the source's own red Standard is renamed.
    let mut source = with_notes_and_comments("src");
    let mut package = OpcPackage::from_reader(Cursor::new(saved(&mut source))).unwrap();
    for (part, from, to) in [
        (
            "/word/styles.xml",
            "</w:styles>".to_owned(),
            format!("{RED_STANDARD}</w:styles>"),
        ),
        (
            "/word/comments.xml",
            r#"<w:pStyle w:val="CommentText"/>"#.to_owned(),
            r#"<w:pStyle w:val="Normal"/>"#.to_owned(),
        ),
    ] {
        let xml = String::from_utf8(package.get_part(part).unwrap().to_vec()).unwrap();
        assert!(xml.contains(&from), "{part}");
        let mut xml = xml.replacen(&from, &to, 1);
        if part == "/word/comments.xml" {
            // The answer uses the source's own Standard, so it is imported.
            xml = xml.replacen(&from, r#"<w:pStyle w:val="Standard"/>"#, 1);
        }
        package.set_part(part, xml.into_bytes());
    }
    // The note paragraphs name the source's default.
    for (part, text) in [
        ("/word/footnotes.xml", "src footnote"),
        ("/word/endnotes.xml", "src endnote"),
    ] {
        let xml = String::from_utf8(package.get_part(part).unwrap().to_vec()).unwrap();
        let at = xml
            .find(text)
            .unwrap_or_else(|| panic!("{part} holds {text:?}"));
        let paragraph = xml[..at].rfind("<w:p>").expect("a plain paragraph start") + "<w:p>".len();
        let xml = format!(
            r#"{}<w:pPr><w:pStyle w:val="Normal"/></w:pPr>{}"#,
            &xml[..paragraph],
            &xml[paragraph..]
        );
        package.set_part(part, xml.into_bytes());
    }
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let source = Document::from_bytes(&out.into_inner()).unwrap();
    let mut doc = with_paragraph_default(
        "Standard",
        false,
        None,
        "",
        &styled_paragraph("Standard", "dest"),
    );
    import_at_end(
        &mut doc,
        &source,
        FragmentConflictPolicy::reuse_equivalent(),
    )
    .unwrap();
    let mut doc = reopen(&mut doc);
    for (part, text) in [
        ("/word/footnotes.xml", "src footnote"),
        ("/word/endnotes.xml", "src endnote"),
        ("/word/comments.xml", "src question"),
    ] {
        assert_eq!(
            part_paragraph_style(&mut doc, part, text).as_deref(),
            Some("Standard"),
            "{part}"
        );
    }
    let red = part_paragraph_style(&mut doc, "/word/comments.xml", "src answer")
        .expect("the answer has a style");
    assert_ne!(red, "Standard");
    assert_eq!(banner_color(&doc, &red).as_deref(), Some("C00000"));
}

#[test]
fn a_style_based_on_a_localized_default_equals_the_same_style_based_on_the_destination_s() {
    let pair = |base: &str| {
        format!(
            r#"<w:style w:type="character" w:default="1" w:styleId="{base}"><w:name w:val="Default Paragraph Font"/><w:uiPriority w:val="1"/><w:semiHidden/></w:style><w:style w:type="paragraph" w:customStyle="1" w:styleId="Big"><w:name w:val="Big"/><w:link w:val="BigChar"/><w:rPr><w:sz w:val="40"/></w:rPr></w:style><w:style w:type="character" w:customStyle="1" w:styleId="BigChar"><w:name w:val="Big Char"/><w:basedOn w:val="{base}"/><w:link w:val="Big"/><w:rPr><w:sz w:val="40"/></w:rPr></w:style>"#
        )
    };
    let source = doc_with_styles(
        &pair("Policepardfaut"),
        &styled_paragraph("Big", "imported"),
    );
    for policy in [
        FragmentConflictPolicy::reuse_equivalent(),
        FragmentConflictPolicy::for_insert_document(Some(
            FragmentConflictPolicy::reuse_equivalent(),
        )),
    ] {
        let mut doc = doc_with_styles(
            &pair("DefaultParagraphFont"),
            &styled_paragraph("Big", "dest"),
        );
        import_at_end(&mut doc, &source, policy).unwrap();
        let doc = reopen(&mut doc);
        assert_eq!(style_of_text(&doc, "imported").as_deref(), Some("Big"));
        for id in ["BigMerge1", "BigCharMerge1", "Policepardfaut"] {
            assert!(doc.style(id).is_none(), "{id}");
        }
    }
}

#[test]
fn an_equal_default_saved_in_another_editing_session_is_the_same_style() {
    // Word writes the session that last saved a style (w:rsid): an equal
    // default from another file maps, under the destination's id or its own.
    for source_default in ["Normal", "Standard"] {
        let source = with_paragraph_default(
            source_default,
            false,
            Some("00AAAAAA"),
            "",
            &styled_paragraph(source_default, "imported"),
        );
        for policy in [
            FragmentConflictPolicy::reuse_equivalent(),
            FragmentConflictPolicy::rename_all(),
        ] {
            let mut doc = with_paragraph_default(
                "Normal",
                false,
                Some("00BBBBBB"),
                "",
                &styled_paragraph("Normal", "dest"),
            );
            import_at_end(&mut doc, &source, policy).unwrap();
            let doc = reopen(&mut doc);
            let case = format!("{source_default}, {policy:?}");
            assert_eq!(
                style_of_text(&doc, "imported").as_deref(),
                Some("Normal"),
                "{case}"
            );
            assert!(doc.style("Standard").is_none(), "{case}");
            assert!(doc.style("NormalMerge1").is_none(), "{case}");
        }
    }
}

#[test]
fn a_different_default_keeps_its_look_whatever_the_language_of_its_id() {
    // A red default paragraph style, under the English id Normal or the
    // German Standard, imported into an English document: either way the
    // imported text keeps red, the destination's Normal stays its default
    // and the document keeps one paragraph default.
    for (source_default, policy) in [
        ("Normal", FragmentConflictPolicy::reuse_equivalent()),
        ("Standard", FragmentConflictPolicy::reuse_equivalent()),
        ("Normal", FragmentConflictPolicy::rename_all()),
        ("Standard", FragmentConflictPolicy::rename_all()),
    ] {
        let source = with_paragraph_default(
            source_default,
            true,
            None,
            "",
            &format!(
                "{}<w:p><w:r><w:t>plain</w:t></w:r></w:p>",
                styled_paragraph(source_default, "imported")
            ),
        );
        assert!(
            banner_color(&source, source_default).is_some(),
            "the default is red"
        );
        let mut doc = doc_with(&["dest"]);
        let mut doc = reopen(&mut doc);
        import_at_end(&mut doc, &source, policy).unwrap();
        let doc = reopen(&mut doc);
        let case = format!("{source_default}, {policy:?}");
        let imported = style_of_text(&doc, "imported").expect("the text has a style");
        assert_ne!(imported, "Normal", "{case}");
        assert_eq!(
            banner_color(&doc, &imported).as_deref(),
            Some("C00000"),
            "{case}"
        );
        assert_eq!(paragraph_defaults(&doc), ["Normal"], "{case}");
        assert_eq!(
            style_of_text(&doc, "plain"),
            None,
            "{case}: text that names no style takes the destination's default"
        );
        assert_eq!(banner_color(&doc, "Normal"), None, "{case}");
    }
}

#[test]
fn the_same_fragment_inserted_twice_writes_no_paragraph_id_twice() {
    let mut doc = doc_with_body(concat!(
        r#"<w:p xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" w14:paraId="1A2B3C4D" w14:textId="77777777">"#,
        r#"<w:r><w:t>repeated</w:t></w:r></w:p><w:p><w:r><w:t>kept</w:t></w:r></w:p>"#,
    ));
    let first = item_at(&doc, 0);
    let fragment = doc.remove_content_at(&first).unwrap();
    for _ in 0..2 {
        doc.insert_content(&ContentLocation::end(body(&doc)), fragment.clone())
            .unwrap();
    }
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["kept", "repeated", "repeated"]);
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert_eq!(xml.matches("1A2B3C4D").count(), 0, "{xml}");
    assert_eq!(xml.matches("77777777").count(), 0, "{xml}");
}

#[test]
fn a_fragment_inserted_twice_drops_ids_declared_on_the_document_root() {
    // Word declares the w14 prefix on w:document, not on the paragraph.
    let bytes = doc_with(&["first", "kept"]).to_bytes().unwrap();
    let mut package = OpcPackage::from_reader(std::io::Cursor::new(bytes)).unwrap();
    let main = String::from_utf8(package.get_part("/word/document.xml").unwrap().to_vec()).unwrap();
    let main = if main.contains("xmlns:w14=") {
        main
    } else {
        main.replacen(
            "<w:document ",
            "<w:document xmlns:w14=\"http://schemas.microsoft.com/office/word/2010/wordml\" ",
            1,
        )
    };
    let first = main.find("<w:p>").or_else(|| main.find("<w:p ")).unwrap();
    let main = format!(
        "{}<w:p w14:paraId=\"2B3C4D5E\" w14:textId=\"66666666\"{}",
        &main[..first],
        &main[first + main[first..].find('>').unwrap()..]
    );
    package.set_part("/word/document.xml", main.into_bytes());
    let mut out = std::io::Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let mut doc = Document::from_bytes(&out.into_inner()).unwrap();
    let fragment = doc.remove_content_at(&item_at(&doc, 0)).unwrap();
    for _ in 0..2 {
        doc.insert_content(&ContentLocation::end(body(&doc)), fragment.clone())
            .unwrap();
    }
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["kept", "first", "first"]);
    let xml = doc.part_xml("word/document.xml").unwrap();
    assert!(
        !xml.contains("2B3C4D5E") && !xml.contains("66666666"),
        "{xml}"
    );
}

#[test]
fn a_picture_from_another_document_never_shows_the_destinations_picture() {
    let picture_document = |grey: u8| {
        let mut doc = Document::new();
        let image = [PNG, &[grey]].concat();
        doc.add_picture(&image, "p.png", Length::inches(1.0), Length::inches(1.0));
        reopen(&mut doc)
    };
    let mut source = picture_document(1);
    let mut doc = picture_document(2);
    let fragment = source.remove_content_at(&item_at(&source, 0)).unwrap();
    let before = doc.to_bytes().unwrap();
    // Both pictures have the same relationship id: an existence check passes.
    match doc.insert_content(&ContentLocation::end(body(&doc)), fragment) {
        Ok(()) => {
            let doc = reopen(&mut doc);
            let pictures = doc.pictures().unwrap();
            assert_ne!(
                pictures[0].bytes, pictures[1].bytes,
                "the picture keeps its image or is refused"
            );
        }
        Err(error) => {
            assert!(error.to_string().contains("where it was taken"), "{error}");
            assert_eq!(doc.to_bytes().unwrap(), before);
        }
    }
}

#[test]
fn a_picture_cut_saved_and_pasted_in_its_document_keeps_its_image() {
    let image = [PNG, &[3u8]].concat();
    let mut doc = doc_with(&["Body"]);
    doc.add_picture(&image, "p.png", Length::inches(1.0), Length::inches(1.0));
    let mut doc = reopen(&mut doc);
    let cut = doc.remove_content_at(&item_at(&doc, 1)).unwrap();
    // A save of the live document in between (reopening the saved bytes would
    // be another document, without the cut picture).
    doc.to_bytes().unwrap();
    doc.insert_content(&ContentLocation::end(body(&doc)), cut)
        .unwrap();
    let doc = reopen(&mut doc);
    let pictures = doc.pictures().unwrap();
    assert_eq!(pictures.len(), 1);
    assert_eq!(pictures[0].bytes, image);
}

#[test]
fn replacing_a_paragraph_with_its_own_xml_keeps_its_paragraph_id() {
    let mut doc = doc_with_body(concat!(
        r#"<w:p xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" w14:paraId="3C4D5E6F" w14:textId="55555555">"#,
        r#"<w:r><w:t>before</w:t></w:r></w:p><w:p><w:r><w:t>other</w:t></w:r></w:p>"#,
    ));
    let location = item_at(&doc, 0);
    let xml = String::from_utf8(doc.paragraph_element_xml(&location).unwrap()).unwrap();
    doc.replace_paragraph_xml(&location, &xml.replace("before", "after"))
        .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["after", "other"]);
    let saved = doc.part_xml("word/document.xml").unwrap();
    assert!(saved.contains("3C4D5E6F"), "{saved}");
}

#[test]
fn a_short_bookmark_name_is_renamed_in_its_operand_not_in_the_keyword() {
    let field = |instruction: &str| {
        format!(
            concat!(
                r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
                r#"<w:r><w:instrText xml:space="preserve"> {} </w:instrText></w:r>"#,
                r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>x</w:t></w:r>"#,
                r#"<w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#
            ),
            instruction
        )
    };
    let source = doc_with_body(&format!(
        "{}{}{}",
        r#"<w:p><w:bookmarkStart w:id="0" w:name="A"/><w:r><w:t>a</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#,
        field(r#"ASK A "Rate?""#),
        field(r#"REF A"#),
    ));
    let mut doc = doc_with_body(
        r#"<w:p><w:bookmarkStart w:id="0" w:name="A"/><w:r><w:t>dst</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#,
    );
    import_at_end(&mut doc, &source, FragmentConflictPolicy::default()).unwrap();
    let doc = reopen(&mut doc);
    let renamed = bookmark_list(&doc)
        .into_iter()
        .find(|(_, text)| text == "a")
        .map(|(name, _)| name)
        .unwrap();
    assert_ne!(renamed, "A");
    assert_eq!(
        field_instructions(&doc),
        [format!("ASK {renamed} \"Rate?\""), format!("REF {renamed}")]
    );
}

#[test]
fn a_link_to_another_file_and_a_nested_field_keep_their_places_on_rename() {
    // Source: a bookmark Mark, a link to another file whose anchor is also
    // "Mark" (a place in that file), and an ASK on Mark with a nested default.
    let mut source = with_link("other file", "https://example.com/report.docx");
    let bytes = source.to_bytes().unwrap();
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    let main = String::from_utf8(package.get_part("/word/document.xml").unwrap().to_vec()).unwrap();
    let main = main.replacen("<w:hyperlink ", "<w:hyperlink w:anchor=\"Mark\" ", 1);
    let at = main.find("<w:body>").unwrap() + "<w:body>".len();
    let added = concat!(
        r#"<w:p><w:bookmarkStart w:id="0" w:name="Mark"/><w:r><w:t>src marked</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#,
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
        r#"<w:r><w:instrText xml:space="preserve"> ASK Mark "Name?" \d "</w:instrText></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> USERNAME </w:instrText></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>Ana</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
        r#"<w:r><w:instrText xml:space="preserve">" </w:instrText></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
    );
    package.set_part(
        "/word/document.xml",
        format!("{}{added}{}", &main[..at], &main[at..]).into_bytes(),
    );
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let source = Document::from_bytes(&out.into_inner()).unwrap();

    let mut doc = with_identities("dst");
    import_at_end(&mut doc, &source, FragmentConflictPolicy::default()).unwrap();
    let mut doc = reopen(&mut doc);
    let renamed = bookmark_list(&doc)
        .into_iter()
        .find(|(_, text)| text == "src marked")
        .map(|(name, _)| name)
        .unwrap();
    assert_ne!(renamed, "Mark");
    let anchors: Vec<_> = doc
        .links()
        .into_iter()
        .filter(|link| link.text == "other file")
        .map(|link| link.anchor)
        .collect();
    assert_eq!(
        anchors,
        [Some("Mark".to_owned())],
        "a place in the other file"
    );
    // The ASK's operand follows; its nested default stays inside the quotes.
    let xml = doc.part_xml("word/document.xml").unwrap();
    let ask = &xml[xml
        .find(&format!("ASK {renamed}"))
        .expect("the ASK is renamed")..];
    let username = ask.find("USERNAME").unwrap();
    let closing = ask
        .find("\" </w:instrText>")
        .or_else(|| ask.find("&quot; </w:instrText>"))
        .unwrap();
    assert!(username < closing, "{ask}");
    let _ = doc.to_bytes().unwrap();
}

#[test]
fn an_inserted_reference_deleted_in_word_follows_its_renamed_bookmark() {
    // The source's REF to Mark was deleted in Word: its code is in
    // w:delInstrText, and it follows the rename as the standing one does.
    let mut source = doc_with_body(concat!(
        r#"<w:p><w:bookmarkStart w:id="0" w:name="Mark"/><w:r><w:t>src marked</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#,
        r#"<w:p><w:del w:id="3" w:author="Ana" w:date="2026-01-01T00:00:00Z"><w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
        r#"<w:r><w:delInstrText xml:space="preserve"> REF Mark \h </w:delInstrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>"#,
        r#"<w:r><w:delText>src marked</w:delText></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:del></w:p>"#,
    ));
    let source = reopen(&mut source);
    let mut doc = with_identities("dst");
    let end = doc.content_count();
    doc.insert_document(end, &source);
    let renamed = bookmark_list(&reopen(&mut doc))
        .into_iter()
        .map(|(name, _)| name)
        .find(|name| name != "Mark")
        .expect("the inserted bookmark is renamed");
    let package = OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).unwrap();
    let part = package.main_document_part().unwrap();
    let xml = String::from_utf8(package.get_part(&part).unwrap().to_vec()).unwrap();
    assert!(
        xml.contains(&format!(
            r#"<w:delInstrText xml:space="preserve"> REF {renamed} \h </w:delInstrText>"#
        )),
        "{xml}"
    );
}
