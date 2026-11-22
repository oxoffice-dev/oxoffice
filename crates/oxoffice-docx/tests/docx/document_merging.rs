//! Combining documents: `append`, `append_with_break` with each break kind,
//! `insert_document` at a body index, `import_fragment` of a range of another
//! document under each conflict policy (from and into headers, footers and
//! table cells too), and `insert_content` of a paragraph
//! taken from another document, with what follows the content: styles, lists,
//! pictures, links, notes, comments, bookmarks, controls, revisions, sections.

use std::io::Cursor;

use std::collections::BTreeMap;

use oxoffice_docx::{
    CommentDate, ContentLocation, Document, DocumentFragment, FragmentConflictPolicy, HdrFtrType,
    HeaderFooterKind, Length, ListLevel, MailMergeData, MailMergeRecord, MailMergeValue,
    RunItemRef, SectionBreak, StoryId, StoryItemKind, StoryKind, StyleBuilder, StyleType,
    TextCommentOptions, ValidationSeverity,
};
use oxoffice_docx_oxml::shared::{ST_PageOrientation, ST_SectionType};
use oxoffice_opc::OpcPackage;

use crate::support::{doc_with, doc_with_body, reopen, texts};

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

/// A saved document whose single paragraph shows the one-pixel PNG.
fn with_picture(text: &str) -> Document {
    let mut doc = doc_with(&[text]);
    doc.add_picture(PNG, "dot.png", Length::inches(1.0), Length::inches(1.0));
    reopen(&mut doc)
}

/// A saved document with one paragraph holding one external link.
fn with_link(text: &str, url: &str) -> Document {
    let mut doc = Document::new();
    doc.append_hyperlink(text, url);
    reopen(&mut doc)
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

/// The effective color of the first run of body paragraph `index`.
fn color_of(doc: &Document, index: usize) -> Option<String> {
    let paragraph = doc.paragraph(index).expect("paragraph exists");
    let run = paragraph.runs().next().expect("paragraph has a run");
    doc.effective_run_properties(&paragraph, &run).color
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

// ---- append_with_break ----

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

/// A saved two-section document whose first section holds the raw body XML
/// `cover` and has this raw header (or footer) with these images, and whose
/// final section holds "body".
fn with_raw_story(
    cover: &str,
    story: &str,
    images: &[(&str, &[u8], &str)],
    footer: bool,
) -> Document {
    let mut doc = doc_with_body(cover);
    let root = if footer { "ftr" } else { "hdr" };
    let xml = format!(
        concat!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
            r#"<w:{root} xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#,
            r#" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#,
            r#" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing""#,
            r#" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main""#,
            r#" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture">{story}</w:{root}>"#
        ),
        root = root,
        story = story
    );
    if footer {
        doc.set_raw_footer_with_images(xml.into_bytes(), images, HdrFtrType::Default);
    } else {
        doc.set_raw_header_with_images(xml.into_bytes(), images, HdrFtrType::Default);
    }
    doc.insert_section(1).expect("a section appends");
    doc.add_paragraph("body");
    reopen(&mut doc)
}

/// The values of `attribute` on every `element` of every saved Word part
/// of `doc`, as (part, value).
fn saved_values(doc: &mut Document, element: &str, attribute: &str) -> Vec<(String, String)> {
    let mut values = Vec::new();
    for (part, bytes) in crate::support::package_parts(&saved(doc)) {
        if !part.starts_with("word/") || !part.ends_with(".xml") {
            continue;
        }
        let xml = String::from_utf8_lossy(&bytes).into_owned();
        for (at, _) in xml.match_indices(&format!("<{element} ")) {
            let tag = &xml[at..at + xml[at..].find('>').expect("a closed tag")];
            if let Some(start) = tag.find(&format!(" {attribute}=\"")) {
                let value = &tag[start + attribute.len() + 3..];
                values.push((part.clone(), value[..value.find('"').unwrap()].to_owned()));
            }
        }
    }
    values
}

/// Whether every value of `values` is different.
fn all_different(values: &[(String, String)]) -> bool {
    let mut seen = std::collections::HashSet::new();
    values.iter().all(|(_, value)| seen.insert(value.clone()))
}

#[test]
fn an_imported_header_takes_fresh_drawing_bookmark_and_control_ids() {
    // The header holds what the destination's body also has: drawing id 1,
    // bookmark id 0 named Mark, control id 5. Its REF fields name its own
    // Mark and the source body's BodyMark, which the destination has too,
    // and the source body names the header's Mark.
    let header = concat!(
        r#"<w:p><w:r><w:drawing><wp:inline><wp:extent cx="91440" cy="91440"/><wp:docPr id="1" name="logo"/>"#,
        r#"<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">"#,
        r#"<pic:pic><pic:nvPicPr><pic:cNvPr id="1" name="logo"/><pic:cNvPicPr/></pic:nvPicPr>"#,
        r#"<pic:blipFill><a:blip r:embed="rIdLogo"/></pic:blipFill><pic:spPr/></pic:pic>"#,
        r#"</a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#,
        r#"<w:p><w:bookmarkStart w:id="0" w:name="Mark"/><w:r><w:t>Header mark</w:t></w:r><w:bookmarkEnd w:id="0"/>"#,
        r#"<w:fldSimple w:instr=" REF Mark \h "><w:r><w:t>Header mark</w:t></w:r></w:fldSimple>"#,
        r#"<w:fldSimple w:instr=" REF BodyMark "><w:r><w:t>Body mark</w:t></w:r></w:fldSimple></w:p>"#,
        r#"<w:sdt><w:sdtPr><w:id w:val="5"/><w:tag w:val="head"/></w:sdtPr>"#,
        r#"<w:sdtContent><w:p><w:r><w:t>Header control</w:t></w:r></w:p></w:sdtContent></w:sdt>"#,
    );
    let cover = concat!(
        r#"<w:p><w:bookmarkStart w:id="3" w:name="BodyMark"/><w:r><w:t>Body mark</w:t></w:r><w:bookmarkEnd w:id="3"/>"#,
        r#"<w:fldSimple w:instr=" REF Mark "><w:r><w:t>Header mark</w:t></w:r></w:fldSimple></w:p>"#,
    );
    let mut source = with_raw_story(cover, header, &[("rIdLogo", PNG, "logo.png")], false);
    let source_bytes = saved(&mut source);
    let destination = concat!(
        r#"<w:p><w:bookmarkStart w:id="0" w:name="Mark"/><w:r><w:t>dst mark</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#,
        r#"<w:p><w:bookmarkStart w:id="1" w:name="BodyMark"/><w:r><w:t>dst body mark</w:t></w:r><w:bookmarkEnd w:id="1"/></w:p>"#,
        r#"<w:sdt><w:sdtPr><w:id w:val="5"/><w:tag w:val="dst"/></w:sdtPr>"#,
        r#"<w:sdtContent><w:p><w:r><w:t>dst control</w:t></w:r></w:p></w:sdtContent></w:sdt>"#,
    );
    for entry in [
        "append",
        "append_with_break",
        "insert_document",
        "import_fragment",
        "mail_merge_rich",
    ] {
        // A rich merge puts the source in place of a merge field.
        let merge_field = if entry == "mail_merge_rich" {
            r#"<w:p><w:fldSimple w:instr=" MERGEFIELD Terms "><w:r><w:t>Terms</w:t></w:r></w:fldSimple></w:p>"#
        } else {
            ""
        };
        let mut doc = doc_with_body(&format!("{destination}{merge_field}"));
        doc.add_picture(PNG, "dot.png", Length::inches(1.0), Length::inches(1.0));
        let mut doc = reopen(&mut doc);
        match entry {
            "append" => doc.append(&source),
            "append_with_break" => doc.append_with_break(&source, SectionBreak::NextPage),
            "insert_document" => doc.insert_document(1, &source),
            "mail_merge_rich" => {
                let data = MailMergeData {
                    records: vec![MailMergeRecord {
                        values: BTreeMap::from([(
                            "Terms".to_owned(),
                            MailMergeValue::Fragment(source_bytes.clone()),
                        )]),
                        regions: BTreeMap::new(),
                    }],
                    sources: BTreeMap::new(),
                };
                doc = doc
                    .mail_merge_rich(&data, None)
                    .unwrap_or_else(|error| panic!("{entry}: {error}"))
                    .remove(0);
            }
            _ => {
                let start = item_at(&source, 0);
                let end = ContentLocation::end(body(&source));
                let fragment = DocumentFragment::from_range(&source, &start, &end, true)
                    .expect("the whole body is a fragment");
                doc.import_fragment(
                    &ContentLocation::end(body(&doc)),
                    &fragment,
                    FragmentConflictPolicy::default(),
                )
                .unwrap_or_else(|error| panic!("{entry}: {error}"));
            }
        }
        let mut doc = reopen(&mut doc);
        let drawings = saved_values(&mut doc, "wp:docPr", "id");
        assert_eq!(drawings.len(), 2, "{entry}: {drawings:?}");
        assert!(all_different(&drawings), "{entry}: {drawings:?}");
        let bookmarks = saved_values(&mut doc, "w:bookmarkStart", "w:id");
        assert!(all_different(&bookmarks), "{entry}: {bookmarks:?}");
        let controls = saved_values(&mut doc, "w:id", "w:val");
        assert!(all_different(&controls), "{entry}: {controls:?}");
        // Both imported bookmarks are renamed, and every REF follows the
        // bookmark it named in the source, across the parts.
        let names = saved_values(&mut doc, "w:bookmarkStart", "w:name");
        assert!(all_different(&names), "{entry}: {names:?}");
        let renamed = |in_header: bool| {
            names
                .iter()
                .find(|(part, name)| {
                    part.contains("header") == in_header && name != "Mark" && name != "BodyMark"
                })
                .map(|(_, name)| name.clone())
                .unwrap_or_else(|| panic!("{entry}: {names:?}"))
        };
        let (header_mark, body_mark) = (renamed(true), renamed(false));
        let mut references = saved_values(&mut doc, "w:fldSimple", "w:instr")
            .into_iter()
            .map(|(part, instruction)| {
                let name = instruction.split_whitespace().nth(1).unwrap().to_owned();
                (part.contains("header"), name)
            })
            .collect::<Vec<_>>();
        references.sort();
        let mut expected = vec![
            (false, header_mark.clone()),
            (true, header_mark),
            (true, body_mark),
        ];
        expected.sort();
        assert_eq!(references, expected, "{entry}");
    }
}

#[test]
fn a_header_reference_follows_a_renamed_footer_bookmark() {
    let story = |root: &str, content: &str| {
        format!(
            r#"<w:{root} xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">{content}</w:{root}>"#
        )
        .into_bytes()
    };
    let mut source = doc_with(&["cover"]);
    source.set_raw_header_with_images(
        story(
            "hdr",
            r#"<w:p><w:fldSimple w:instr=" REF FooterMark "><w:r><w:t>Footer mark</w:t></w:r></w:fldSimple></w:p>"#,
        ),
        &[],
        HdrFtrType::Default,
    );
    source.set_raw_footer_with_images(
        story(
            "ftr",
            r#"<w:p><w:bookmarkStart w:id="4" w:name="FooterMark"/><w:r><w:t>Footer mark</w:t></w:r><w:bookmarkEnd w:id="4"/></w:p>"#,
        ),
        &[],
        HdrFtrType::Default,
    );
    source.insert_section(1).expect("a section appends");
    source.add_paragraph("body");
    let source = reopen(&mut source);
    let mut doc = doc_with_body(
        r#"<w:p><w:bookmarkStart w:id="0" w:name="FooterMark"/><w:r><w:t>dst</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#,
    );
    doc.insert_document(1, &source);
    let mut doc = reopen(&mut doc);
    let names = saved_values(&mut doc, "w:bookmarkStart", "w:name");
    let (_, renamed) = names
        .iter()
        .find(|(part, _)| part.contains("footer"))
        .unwrap_or_else(|| panic!("{names:?}"));
    assert_ne!(renamed, "FooterMark");
    let references = saved_values(&mut doc, "w:fldSimple", "w:instr");
    assert_eq!(
        references,
        [("word/header1.xml".to_owned(), format!(" REF {renamed} "))]
    );
}

/// A two-section document whose first section's footer holds a control
/// bound to a custom XML store reading `value`; gives the store id too.
fn with_bound_footer(value: &str) -> (Document, String) {
    let mut doc = doc_with(&["cover"]);
    let store = doc
        .add_custom_xml_store(
            format!(r#"<c:customer xmlns:c="urn:crm"><c:name>{value}</c:name></c:customer>"#)
                .as_bytes(),
            &["urn:crm"],
        )
        .expect("the store is added");
    let footer = format!(
        concat!(
            r#"<w:sdt><w:sdtPr><w:id w:val="9"/><w:tag w:val="name"/>"#,
            r#"<w:dataBinding w:prefixMappings="xmlns:c='urn:crm'" w:xpath="/c:customer/c:name" w:storeItemID="{store}"/>"#,
            r#"</w:sdtPr><w:sdtContent><w:p><w:r><w:t>{value}</w:t></w:r></w:p></w:sdtContent></w:sdt>"#
        ),
        store = store,
        value = value
    );
    let bytes = doc.to_bytes().expect("document saves");
    let mut doc = Document::from_bytes(&bytes).expect("document reopens");
    let xml = format!(
        r#"<w:ftr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">{footer}</w:ftr>"#
    );
    doc.set_raw_footer_with_images(xml.into_bytes(), &[], HdrFtrType::Default);
    doc.insert_section(1).expect("a section appends");
    doc.add_paragraph("body");
    (reopen(&mut doc), store)
}

/// The text of the custom XML store `store` of the saved `doc`.
fn store_text(doc: &mut Document, store: &str) -> Option<String> {
    let parts = crate::support::package_parts(&saved(doc));
    let (properties, _) = parts.iter().find(|(part, bytes)| {
        part.starts_with("customXml/itemProps")
            && String::from_utf8_lossy(bytes).contains(&format!("itemID=\"{store}\""))
    })?;
    let item = properties.replace("itemProps", "item");
    Some(String::from_utf8_lossy(parts.get(&item)?).into_owned())
}

#[test]
fn an_imported_footer_control_stays_bound_to_the_data_it_showed() {
    for destination_has_the_store in [false, true] {
        let (source, _) = with_bound_footer("Source Ada");
        let mut doc = if destination_has_the_store {
            // The same store id with other data, as two copies of one
            // template filled for two customers have.
            let (mut doc, _) = with_bound_footer("Source Ada");
            assert_eq!(
                doc.set_content_control_value_by_tag("name", "Dest Bo")
                    .unwrap(),
                1
            );
            reopen(&mut doc)
        } else {
            doc_with(&["dest"])
        };
        doc.insert_document(1, &source);
        let mut doc = reopen(&mut doc);
        let bindings = saved_values(&mut doc, "w:dataBinding", "w:storeItemID");
        let footers: Vec<_> = bindings
            .iter()
            .filter(|(part, _)| part.contains("footer"))
            .collect();
        let expected = if destination_has_the_store { 2 } else { 1 };
        assert_eq!(footers.len(), expected, "{bindings:?}");
        // The copied footer shows the source's data; the destination's own
        // footer keeps its store.
        let mut texts: Vec<String> = footers
            .iter()
            .map(|(_, id)| {
                store_text(&mut doc, id)
                    .unwrap_or_else(|| panic!("{destination_has_the_store}: store {id} exists"))
            })
            .collect();
        texts.sort();
        let names: Vec<_> = texts
            .iter()
            .map(|text| &text[text.find("<c:name>").unwrap() + 8..text.find("</c:name>").unwrap()])
            .collect();
        if destination_has_the_store {
            assert_eq!(names, ["Dest Bo", "Source Ada"], "{bindings:?}");
        } else {
            assert_eq!(names, ["Source Ada"], "{bindings:?}");
        }
    }
}

#[test]
fn a_control_bound_to_a_document_property_imports_as_it_is() {
    // Word's Insert > Quick Parts > Document Property > Title binds the core
    // properties, which no custom XML store holds.
    let title = concat!(
        r#"<w:sdt><w:sdtPr><w:id w:val="12"/><w:dataBinding w:prefixMappings="xmlns:ns0='http://purl.org/dc/elements/1.1/'""#,
        r#" w:xpath="/ns1:coreProperties[1]/ns0:title[1]" w:storeItemID="{6C3C8BC8-F283-45AE-878A-BAB7291924A1}"/>"#,
        r#"<w:text/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>Title</w:t></w:r></w:p></w:sdtContent></w:sdt>"#,
    );
    // The same quick part in a footnote too.
    let mut source = with_raw_story(title, title, &[], true);
    let id = source.add_footnote("note");
    source.add_paragraph("noted").add_footnote_ref(id);
    let mut package = OpcPackage::from_reader(Cursor::new(saved(&mut source))).unwrap();
    let notes =
        String::from_utf8(package.get_part("/word/footnotes.xml").unwrap().to_vec()).unwrap();
    let at = notes.rfind("</w:footnote>").unwrap();
    package.set_part(
        "/word/footnotes.xml",
        format!("{}{title}{}", &notes[..at], &notes[at..]).into_bytes(),
    );
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let source = Document::from_bytes(&out.into_inner()).expect("the source opens");
    for entry in ["append", "insert_document"] {
        let mut doc = doc_with(&["dest"]);
        match entry {
            "append" => doc.append(&source),
            _ => doc.insert_document(1, &source),
        }
        let mut doc = reopen(&mut doc);
        let bindings = saved_values(&mut doc, "w:dataBinding", "w:storeItemID");
        let stories: Vec<_> = bindings.iter().map(|(part, _)| part.as_str()).collect();
        assert_eq!(
            stories,
            [
                "word/document.xml",
                "word/footer1.xml",
                "word/footnotes.xml"
            ],
            "{entry}"
        );
        assert!(
            bindings
                .iter()
                .all(|(_, id)| id == "{6C3C8BC8-F283-45AE-878A-BAB7291924A1}"),
            "{entry}: {bindings:?}"
        );
    }
}

/// A header holding a VML watermark and a page number, whose runs already
/// name the fonts and size the source's defaults give, so an import keeping
/// the source's formatting has nothing to prepare in it.
#[test]
fn a_header_the_source_formatting_leaves_alone_keeps_its_bytes() {
    let header = concat!(
        r#"<w:p><w:pPr><w:pStyle w:val="Header"/></w:pPr><w:r><w:rPr><w:rFonts w:ascii="Dest Sans" w:hAnsi="Dest Sans"/><w:sz w:val="22"/></w:rPr>"#,
        r##"<w:pict><v:shape xmlns:v="urn:schemas-microsoft-com:vml" id="PowerPlusWaterMarkObject" type="#_x0000_t136" style="width:400pt;height:100pt"/></w:pict>"##,
        r#"<w:t xml:space="preserve">Page  </w:t></w:r><w:fldSimple w:instr=" PAGE "><w:r><w:rPr><w:rFonts w:ascii="Dest Sans" w:hAnsi="Dest Sans"/><w:sz w:val="22"/></w:rPr><w:t>1</w:t></w:r></w:fldSimple></w:p>"#,
    );
    let mut source = with_raw_story(
        r#"<w:p><w:r><w:t>cover</w:t></w:r></w:p>"#,
        header,
        &[],
        false,
    );
    let original = source_stories_with(&mut source, "PowerPlusWaterMarkObject");
    let mut doc = doc_with(&["dest"]);
    let start = item_at(&source, 0);
    let end = ContentLocation::end(body(&source));
    let fragment = DocumentFragment::from_range(&source, &start, &end, true).unwrap();
    doc.import_fragment(
        &ContentLocation::end(body(&doc)),
        &fragment,
        FragmentConflictPolicy::for_insert_document(Some(
            FragmentConflictPolicy::reuse_equivalent(),
        )),
    )
    .unwrap();
    let mut doc = reopen(&mut doc);
    assert_eq!(
        source_stories_with(&mut doc, "PowerPlusWaterMarkObject"),
        original
    );
}

#[test]
fn an_imported_header_keeps_the_source_fonts_when_the_policy_keeps_them() {
    let mut source = with_raw_story(
        r#"<w:p><w:r><w:t>cover</w:t></w:r></w:p>"#,
        r#"<w:p><w:r><w:t>Plain header</w:t></w:r></w:p>"#,
        &[],
        false,
    );
    source
        .set_default_font_name_value(Some("Source Serif"))
        .unwrap();
    let source = reopen(&mut source);
    let mut doc = doc_with(&["dest"]);
    doc.set_default_font_name_value(Some("Dest Sans")).unwrap();
    let mut doc = reopen(&mut doc);
    let start = item_at(&source, 0);
    let end = ContentLocation::end(body(&source));
    let fragment = DocumentFragment::from_range(&source, &start, &end, true).unwrap();
    doc.import_fragment(
        &ContentLocation::end(body(&doc)),
        &fragment,
        FragmentConflictPolicy::for_insert_document(Some(
            FragmentConflictPolicy::reuse_equivalent(),
        )),
    )
    .unwrap();
    let mut doc = reopen(&mut doc);
    let fonts = saved_values(&mut doc, "w:rFonts", "w:ascii");
    for story in ["document", "header"] {
        assert!(
            fonts
                .iter()
                .any(|(part, font)| part.contains(story) && font == "Source Serif"),
            "{story}: {fonts:?}"
        );
    }
    let stories = source_stories_with(&mut doc, "Plain header");
    let [header] = stories.as_slice() else {
        panic!("{stories:?}");
    };
    assert!(word_value(header, "pStyle").is_some(), "{header}");
}

/// The header and footer parts of the saved `doc` holding `text`.
fn source_stories_with(doc: &mut Document, text: &str) -> Vec<String> {
    crate::support::package_parts(&saved(doc))
        .into_iter()
        .filter(|(name, _)| name.contains("header") || name.contains("footer"))
        .map(|(_, bytes)| String::from_utf8_lossy(&bytes).into_owned())
        .filter(|xml| xml.contains(text))
        .collect()
}

// ---- import_fragment ----

#[test]
fn an_import_leaves_out_the_half_bookmarks_it_would_copy() {
    // LibreOffice writes a bookmark start that never ends for each legacy
    // form field it copies (PaymentTerms starts twice, once for good).
    let mut source = crate::support::corpus_doc("libreoffice/form.docx");
    let source_before = saved(&mut source);
    let fields = |doc: &Document| {
        doc.legacy_form_fields()
            .unwrap()
            .into_iter()
            .map(|field| {
                (
                    field.name,
                    field.kind,
                    field.value,
                    field.choices,
                    field.enabled,
                )
            })
            .collect::<Vec<_>>()
    };
    let source_fields = fields(&source);
    assert!(
        ["PaymentTerms", "RemittanceEmail", "Declaration"]
            .iter()
            .all(|name| source_fields
                .iter()
                .any(|field| field.0.as_deref() == Some(*name))),
        "{source_fields:?}"
    );
    let mut doc = doc_with(&["dest"]);
    let start = item_at(&source, 0);
    let end = ContentLocation::end(body(&source));
    let fragment = DocumentFragment::from_range(&source, &start, &end, true).unwrap();
    doc.import_fragment(
        &ContentLocation::end(body(&doc)),
        &fragment,
        FragmentConflictPolicy::default(),
    )
    .expect("a source with a start that never ends imports");
    let mut doc = reopen(&mut doc);
    let starts = main_bookmark_values(&mut doc, "w:bookmarkStart");
    let ends = main_bookmark_values(&mut doc, "w:bookmarkEnd");
    let mut start_ids: Vec<_> = starts.iter().map(|(id, _)| id.clone()).collect();
    start_ids.sort();
    let mut end_ids: Vec<_> = ends.iter().map(|(id, _)| id.clone()).collect();
    end_ids.sort();
    assert_eq!(start_ids, end_ids, "every start ends: {starts:?} {ends:?}");
    let names: Vec<_> = starts.iter().map(|(_, name)| name.as_str()).collect();
    assert_eq!(
        names.iter().filter(|name| **name == "PaymentTerms").count(),
        1,
        "{names:?}"
    );
    assert_eq!(
        starts.len(),
        9,
        "the twelve starts but the three that never end: {names:?}"
    );
    // The form fields read as in the source, and the source is unchanged.
    assert_eq!(fields(&doc), source_fields);
    assert_eq!(saved(&mut source), source_before);

    // A range that cuts a bookmark leaves both halves out.
    let source = doc_with_body(concat!(
        r#"<w:p><w:bookmarkStart w:id="3" w:name="Across"/><w:r><w:t>first</w:t></w:r></w:p>"#,
        r#"<w:p><w:r><w:t>second</w:t></w:r><w:bookmarkEnd w:id="3"/></w:p>"#,
    ));
    let fragment =
        DocumentFragment::from_range(&source, &item_at(&source, 0), &item_at(&source, 1), false)
            .unwrap();
    let mut doc = doc_with(&["dest"]);
    doc.import_fragment(
        &ContentLocation::end(body(&doc)),
        &fragment,
        FragmentConflictPolicy::default(),
    )
    .expect("a range that cuts a bookmark imports");
    let mut doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["dest", "first"]);
    assert!(main_bookmark_values(&mut doc, "w:bookmarkStart").is_empty());
}

/// The (`w:id`, `w:name`) of every `element` of the saved main part.
fn main_bookmark_values(doc: &mut Document, element: &str) -> Vec<(String, String)> {
    let xml = String::from_utf8(
        crate::support::package_parts(&saved(doc))
            .remove("word/document.xml")
            .expect("a main part"),
    )
    .unwrap();
    let value = |tag: &str, attribute: &str| {
        tag.find(&format!(" {attribute}=\"")).map(|at| {
            let rest = &tag[at + attribute.len() + 3..];
            rest[..rest.find('"').unwrap()].to_owned()
        })
    };
    xml.match_indices(&format!("<{element} "))
        .map(|(at, _)| {
            let tag = &xml[at..at + xml[at..].find('>').unwrap()];
            (
                value(tag, "w:id").unwrap_or_default(),
                value(tag, "w:name").unwrap_or_default(),
            )
        })
        .collect()
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

/// The saved main story of `doc`.
fn main_story_xml(doc: &mut Document) -> String {
    String::from_utf8(crate::support::package_parts(&saved(doc))["word/document.xml"].clone())
        .expect("main part is UTF-8")
}

/// A complex field whose code is written in `codes`, one run each:
/// (element, text), the element being `instrText` or `delInstrText`.
fn field_in_runs(codes: &[(&str, &str)]) -> String {
    let mut xml = String::from(r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r>"#);
    for (element, text) in codes {
        xml.push_str(&format!(
            r#"<w:r><w:{element} xml:space="preserve">{text}</w:{element}></w:r>"#
        ));
    }
    xml.push_str(concat!(
        r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>value</w:t></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#
    ));
    xml
}

#[test]
fn formula_comparison_and_implicit_ref_operands_follow_a_renamed_bookmark() {
    let nested_address = concat!(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
        r#"<w:r><w:instrText xml:space="preserve"> HYPERLINK </w:instrText></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r>"#,
        r#"<w:r><w:instrText xml:space="preserve"> MERGEFIELD Url </w:instrText></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>url</w:t></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
        r#"<w:r><w:instrText xml:space="preserve"> \l "Mark" </w:instrText></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>away</w:t></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
    );
    // A deleted field keeps its code in delInstrText.
    let deleted = format!(
        r#"<w:p><w:del w:id="9" w:author="Ana" w:date="2026-01-01T00:00:00Z">{}</w:del></w:p>"#,
        field_in_runs(&[("delInstrText", " REF Mark ")])
            .trim_start_matches("<w:p>")
            .trim_end_matches("</w:p>")
    );
    let source = doc_with_body(
        &[
            r#"<w:p><w:bookmarkStart w:id="0" w:name="Mark"/><w:r><w:t>src marked</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#.to_owned(),
            field_in_runs(&[("instrText", r#" = Mark * 2 + Other \# "0.00" "#)]),
            field_in_runs(&[("instrText", r#" IF mark = "yes" "a" "b" "#)]),
            field_in_runs(&[("instrText", " COMPARE 1 &lt; MARK ")]),
            field_in_runs(&[("instrText", r#" REF &quot;mark&quot; \h "#)]),
            field_in_runs(&[("instrText", r#" Mark \* MERGEFORMAT "#)]),
            // A name written across two runs, as Word does after an edit.
            field_in_runs(&[("instrText", " REF Ma"), ("instrText", r#"rk \h "#)]),
            deleted,
            // The address is a nested field: `\l` names a place in it.
            nested_address.to_owned(),
            // A field of Word's that names no bookmark.
            field_in_runs(&[("instrText", " PAGE ")]),
        ]
        .concat(),
    );
    let mut doc = with_identities("dst");
    import_at_end(&mut doc, &source, FragmentConflictPolicy::default()).unwrap();
    let mut doc = reopen(&mut doc);

    let renamed = bookmark_list(&doc)
        .into_iter()
        .find(|(_, text)| text == "src marked")
        .map(|(name, _)| name)
        .unwrap();
    assert!(!renamed.eq_ignore_ascii_case("Mark"));
    // Quotes may be written escaped or not.
    let xml = main_story_xml(&mut doc).replace("&quot;", "\"");
    for expected in [
        format!(r#"> = {renamed} * 2 + Other \# "0.00" <"#),
        format!(r#"> IF {renamed} = "yes" "a" "b" <"#),
        format!("> COMPARE 1 &lt; {renamed} <"),
        format!(r#"> REF "{renamed}" \h <"#),
        format!(r#"> {renamed} \* MERGEFORMAT <"#),
        format!("<w:delInstrText xml:space=\"preserve\"> REF {renamed} </w:delInstrText>"),
        // The split name: the new one in the first run, the rest of the
        // code in the second.
        format!(r#"<w:instrText xml:space="preserve"> REF {renamed}</w:instrText>"#),
        r#"<w:instrText xml:space="preserve"> \h </w:instrText>"#.to_owned(),
        r#"<w:instrText xml:space="preserve"> \l "Mark" </w:instrText>"#.to_owned(),
        r#"<w:instrText xml:space="preserve"> PAGE </w:instrText>"#.to_owned(),
    ] {
        assert!(xml.contains(&expected), "{expected} in {xml}");
    }
}

#[test]
fn a_renamed_bookmark_never_takes_a_name_the_destination_compares_against() {
    // The destination's IF compares with a bookmark of a name an import
    // could give; its operator is written as an entity.
    let mut doc = doc_with_body(&format!(
        "{}{}",
        r#"<w:p><w:bookmarkStart w:id="0" w:name="Mark"/><w:r><w:t>dst marked</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#,
        field_in_runs(&[("instrText", r#" IF 1 &lt; MailMerge1 "a" "b" "#)])
    ));
    let source = doc_with_body(
        r#"<w:p><w:bookmarkStart w:id="0" w:name="Mark"/><w:r><w:t>src marked</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#,
    );
    import_at_end(&mut doc, &source, FragmentConflictPolicy::default()).unwrap();
    let doc = reopen(&mut doc);
    let renamed = bookmark_list(&doc)
        .into_iter()
        .find(|(_, text)| text == "src marked")
        .map(|(name, _)| name)
        .unwrap();
    assert!(!renamed.eq_ignore_ascii_case("Mark"));
    assert!(!renamed.eq_ignore_ascii_case("MailMerge1"), "{renamed}");
}

#[test]
fn a_form_field_keeps_the_name_of_its_renamed_bookmark() {
    let form = |name: &str, text: &str| {
        format!(
            concat!(
                r#"<w:p><w:bookmarkStart w:id="0" w:name="{name}"/><w:r><w:fldChar w:fldCharType="begin">"#,
                r#"<w:ffData><w:name w:val="{name}"/><w:enabled/><w:calcOnExit w:val="0"/><w:textInput/></w:ffData>"#,
                r#"</w:fldChar></w:r><w:r><w:instrText xml:space="preserve"> FORMTEXT </w:instrText></w:r>"#,
                r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>{text}</w:t></w:r>"#,
                r#"<w:r><w:fldChar w:fldCharType="end"/></w:r><w:bookmarkEnd w:id="0"/></w:p>"#
            ),
            name = name,
            text = text
        )
    };
    let source = doc_with_body(&form("Text1", "src"));
    let mut doc = doc_with_body(&form("Text1", "dst"));
    import_at_end(&mut doc, &source, FragmentConflictPolicy::default()).unwrap();
    let mut doc = reopen(&mut doc);

    let bookmarks = bookmark_list(&doc);
    assert_eq!(bookmarks.len(), 2, "{bookmarks:?}");
    let renamed = &bookmarks[1].0;
    assert_ne!(renamed, "Text1");
    let xml = main_story_xml(&mut doc);
    let form_names: Vec<_> = xml
        .match_indices(r#"<w:ffData><w:name w:val=""#)
        .map(|(at, found)| {
            let rest = &xml[at + found.len()..];
            rest[..rest.find('"').unwrap()].to_owned()
        })
        .collect();
    assert_eq!(form_names, ["Text1".to_owned(), renamed.clone()]);
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

// ---- Corpus ----

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

const RED_STANDARD: &str = r#"<w:style w:type="paragraph" w:styleId="Standard"><w:name w:val="Standard"/><w:rPr><w:color w:val="C00000"/></w:rPr></w:style>"#;

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

/// The values of `attribute` on every `element` of the saved main part.
fn main_values(doc: &mut Document, element: &str, attribute: &str) -> Vec<String> {
    let xml = String::from_utf8(
        crate::support::package_parts(&saved(doc))
            .remove("word/document.xml")
            .expect("a main part"),
    )
    .unwrap();
    xml.match_indices(&format!("<{element} "))
        .filter_map(|(at, _)| {
            let tag = &xml[at..at + xml[at..].find('>').unwrap()];
            let start = tag.find(&format!(" {attribute}=\""))? + attribute.len() + 3;
            Some(tag[start..start + tag[start..].find('"').unwrap()].to_owned())
        })
        .collect()
}

#[test]
fn a_cut_fragment_pasted_twice_keeps_its_ids_then_takes_fresh_ones() {
    let mut doc = doc_with_body(concat!(
        r#"<w:p><w:r><w:t>kept</w:t></w:r></w:p>"#,
        r#"<w:p><w:bookmarkStart w:id="5" w:name="Mark"/><w:r><w:t>marked</w:t></w:r><w:bookmarkEnd w:id="5"/>"#,
        r#"<w:fldSimple w:instr=" REF Mark "><w:r><w:t>marked</w:t></w:r></w:fldSimple></w:p>"#,
        // Word writes random signed control ids.
        r#"<w:sdt><w:sdtPr><w:id w:val="-1758118386"/><w:tag w:val="t"/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>control</w:t></w:r></w:p></w:sdtContent></w:sdt>"#,
    ));
    let marked = doc.remove_content_at(&item_at(&doc, 1)).unwrap();
    let control = doc.remove_content_at(&item_at(&doc, 1)).unwrap();
    // A cut and paste changes nothing.
    doc.insert_content(&ContentLocation::end(body(&doc)), marked.clone())
        .unwrap();
    doc.insert_content(&ContentLocation::end(body(&doc)), control.clone())
        .unwrap();
    assert_eq!(main_values(&mut doc, "w:bookmarkStart", "w:id"), ["5"]);
    assert_eq!(main_values(&mut doc, "w:bookmarkStart", "w:name"), ["Mark"]);
    assert_eq!(main_values(&mut doc, "w:id", "w:val"), ["-1758118386"]);
    // Pasted again, what the document holds is fresh, and the copy's REF
    // follows its own bookmark.
    doc.insert_content(&ContentLocation::end(body(&doc)), marked)
        .unwrap();
    doc.insert_content(&ContentLocation::end(body(&doc)), control)
        .unwrap();
    let mut doc = reopen(&mut doc);
    assert_eq!(
        texts(&doc),
        ["kept", "markedmarked", "control", "markedmarked", "control"]
    );
    let ids = main_values(&mut doc, "w:bookmarkStart", "w:id");
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
    let names = main_values(&mut doc, "w:bookmarkStart", "w:name");
    assert_eq!(names[0], "Mark");
    assert_ne!(names[1].to_lowercase(), "mark");
    assert_eq!(
        main_values(&mut doc, "w:fldSimple", "w:instr"),
        [" REF Mark ".to_owned(), format!(" REF {} ", names[1])]
    );
    let controls = main_values(&mut doc, "w:id", "w:val");
    assert_eq!(controls.len(), 2);
    assert_ne!(controls[0], controls[1]);
}

#[test]
fn a_fresh_id_never_repeats_one_the_inserted_fragment_keeps() {
    // The block control's id 5 is taken in the destination; the lowest free
    // id, 1, is the one its inner control keeps. Likewise for bookmarks:
    // id 4 is taken, the lowest free id, 0, is kept by the other.
    let mut source = doc_with_body(concat!(
        r#"<w:sdt><w:sdtPr><w:id w:val="5"/></w:sdtPr><w:sdtContent><w:p>"#,
        r#"<w:bookmarkStart w:id="4" w:name="Taken"/><w:bookmarkStart w:id="0" w:name="Free"/>"#,
        r#"<w:sdt><w:sdtPr><w:id w:val="1"/></w:sdtPr><w:sdtContent><w:r><w:t>inner</w:t></w:r></w:sdtContent></w:sdt>"#,
        r#"<w:bookmarkEnd w:id="0"/><w:bookmarkEnd w:id="4"/>"#,
        r#"</w:p></w:sdtContent></w:sdt><w:p><w:r><w:t>kept</w:t></w:r></w:p>"#,
    ));
    let fragment = source.remove_content_at(&item_at(&source, 0)).unwrap();
    let mut doc = doc_with_body(concat!(
        r#"<w:sdt><w:sdtPr><w:id w:val="5"/></w:sdtPr><w:sdtContent><w:p>"#,
        r#"<w:bookmarkStart w:id="4" w:name="Other"/><w:r><w:t>dest</w:t></w:r><w:bookmarkEnd w:id="4"/>"#,
        r#"</w:p></w:sdtContent></w:sdt>"#,
    ));
    doc.insert_content(&ContentLocation::end(body(&doc)), fragment)
        .unwrap();
    let mut doc = reopen(&mut doc);
    let mut ids = main_values(&mut doc, "w:id", "w:val");
    ids.sort();
    assert_eq!(ids.len(), 3, "{ids:?}");
    ids.dedup();
    assert_eq!(ids.len(), 3, "no id twice");
    let mut bookmarks = main_values(&mut doc, "w:bookmarkStart", "w:id");
    bookmarks.sort();
    assert_eq!(bookmarks.len(), 3, "{bookmarks:?}");
    bookmarks.dedup();
    assert_eq!(bookmarks.len(), 3, "no bookmark id twice");
}

#[test]
fn a_pasted_or_cloned_note_reference_gets_its_own_note() {
    let mut doc = doc_with(&["kept"]);
    let id = doc.add_footnote("the note");
    doc.add_paragraph("noted").add_footnote_ref(id);
    doc.add_picture(PNG, "dot.png", Length::inches(1.0), Length::inches(1.0));
    let mut doc = reopen(&mut doc);
    let noted = doc.remove_content_at(&item_at(&doc, 1)).unwrap();
    let picture = doc.remove_content_at(&item_at(&doc, 1)).unwrap();
    for _ in 0..2 {
        doc.insert_content(&ContentLocation::end(body(&doc)), noted.clone())
            .unwrap();
        doc.insert_content(&ContentLocation::end(body(&doc)), picture.clone())
            .unwrap();
    }
    doc.clone_content(&item_at(&doc, 1), &ContentLocation::end(body(&doc)))
        .unwrap();
    let mut doc = reopen(&mut doc);
    let references = main_values(&mut doc, "w:footnoteReference", "w:id");
    assert_eq!(references.len(), 3, "{references:?}");
    assert_eq!(
        references[0],
        id.to_string(),
        "the cut reference keeps its note"
    );
    let notes: std::collections::HashSet<_> = references.iter().collect();
    assert_eq!(notes.len(), 3, "{references:?}");
    let texts: Vec<_> = doc
        .footnotes()
        .into_iter()
        .filter(|(id, _)| references.contains(&id.to_string()))
        .map(|(_, text)| text.trim().to_owned())
        .collect();
    assert_eq!(texts, ["the note"; 3]);
    let drawings = main_values(&mut doc, "wp:docPr", "id");
    assert_eq!(drawings.len(), 2);
    assert_ne!(drawings[0], drawings[1]);
}

/// A body with a Google Docs heading, its bookmark start before it, and a
/// table wrapped whole by a bookmark, start just before it and end just
/// after it, between "kept" and "end".
fn with_wrapped_blocks() -> Document {
    doc_with_body(concat!(
        r#"<w:p><w:r><w:t>kept</w:t></w:r></w:p>"#,
        r#"<w:bookmarkStart w:id="7" w:name="_heading"/><w:p><w:r><w:t>Heading</w:t></w:r><w:bookmarkEnd w:id="7"/></w:p>"#,
        r#"<w:bookmarkStart w:id="8" w:name="table"/><w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="100"/></w:tblGrid>"#,
        r#"<w:tr><w:tc><w:p><w:r><w:t>cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:bookmarkEnd w:id="8"/>"#,
        r#"<w:p><w:r><w:t>end</w:t></w:r></w:p>"#,
    ))
}

/// The direct body items of the saved `doc` as one letter each: S bookmark
/// start, E bookmark end, P paragraph, T table (the final section left out).
fn body_shape(doc: &mut Document) -> String {
    use quick_xml::events::Event;
    let xml = crate::support::package_parts(&saved(doc))
        .remove("word/document.xml")
        .expect("a main part");
    let mut reader = quick_xml::Reader::from_reader(xml.as_slice());
    let mut buffer = Vec::new();
    let (mut depth, mut body, mut shape) = (0, None, String::new());
    loop {
        let event = reader.read_event_into(&mut buffer).unwrap();
        let (name, empty) = match &event {
            Event::Start(element) => (element.local_name().as_ref().to_vec(), false),
            Event::Empty(element) => (element.local_name().as_ref().to_vec(), true),
            Event::End(_) => {
                depth -= 1;
                buffer.clear();
                continue;
            }
            Event::Eof => return shape,
            _ => {
                buffer.clear();
                continue;
            }
        };
        if body.is_some_and(|body| depth == body + 1) && name != b"sectPr" {
            shape.push(match name.as_slice() {
                b"p" => 'P',
                b"tbl" => 'T',
                b"bookmarkStart" => 'S',
                b"bookmarkEnd" => 'E',
                _ => '?',
            });
        }
        if name == b"body" {
            body = Some(depth);
        }
        if !empty {
            depth += 1;
        }
        buffer.clear();
    }
}

#[test]
fn a_block_takes_the_bookmark_that_wraps_it_when_it_is_cut_moved_or_cloned() {
    let mut doc = with_wrapped_blocks();
    assert_eq!(body_shape(&mut doc), "PSPSTEP");
    // Cut and pasted, the heading keeps its bookmark whole.
    let mut cut = with_wrapped_blocks();
    let heading = cut.remove_content_at(&item_at(&cut, 2)).unwrap();
    assert_eq!(body_shape(&mut cut), "PSTEP");
    cut.insert_content(&ContentLocation::end(body(&cut)), heading)
        .unwrap();
    assert_eq!(body_shape(&mut cut), "PSTEPSP");
    assert_eq!(reopen(&mut cut).bookmarks().len(), 2);
    // The table moves and goes with its bookmark, start and end.
    let mut moved = with_wrapped_blocks();
    moved
        .move_content(&item_at(&moved, 4), &ContentLocation::end(body(&moved)))
        .unwrap();
    assert_eq!(body_shape(&mut moved), "PSPPSTE");
    let mut removed = with_wrapped_blocks();
    let table = removed.remove_content_at(&item_at(&removed, 4)).unwrap();
    assert_eq!(body_shape(&mut removed), "PSPP");
    removed
        .insert_content(&ContentLocation::end(body(&removed)), table)
        .unwrap();
    assert_eq!(body_shape(&mut removed), "PSPPSTE");
    for by_range in [false, true] {
        let mut doc = with_wrapped_blocks();
        if by_range {
            assert_eq!(doc.remove_content_range(4, 5).unwrap(), 3);
        } else {
            assert!(doc.remove_content(4));
        }
        assert_eq!(body_shape(&mut doc), "PSPP", "{by_range}");
    }
    // A clone copies the bookmark, under a fresh id and name.
    let mut cloned = with_wrapped_blocks();
    cloned
        .clone_content(&item_at(&cloned, 2), &ContentLocation::end(body(&cloned)))
        .unwrap();
    assert_eq!(body_shape(&mut cloned), "PSPSTEPSP");
    let mut cloned = reopen(&mut cloned);
    let names = main_values(&mut cloned, "w:bookmarkStart", "w:name");
    assert_eq!(names.len(), 3);
    assert_eq!(
        names.iter().collect::<std::collections::HashSet<_>>().len(),
        3
    );
}

#[test]
fn a_plain_paragraph_or_a_table_goes_with_the_bookmark_around_it() {
    // A plain paragraph wrapped whole: start just before, end just after.
    let wrapped = || {
        doc_with_body(concat!(
            r#"<w:p><w:r><w:t>kept</w:t></w:r></w:p>"#,
            r#"<w:bookmarkStart w:id="3" w:name="Around"/><w:p><w:r><w:t>plain</w:t></w:r></w:p><w:bookmarkEnd w:id="3"/>"#,
            r#"<w:p><w:r><w:t>end</w:t></w:r></w:p>"#,
        ))
    };
    let mut doc = wrapped();
    assert!(doc.remove_content(2));
    assert_eq!(body_shape(&mut doc), "PP");
    let mut doc = wrapped();
    assert_eq!(doc.remove_content_range(2, 3).unwrap(), 3);
    assert_eq!(body_shape(&mut doc), "PP");
    // A table whose bookmark starts in its first cell and ends after it.
    let table = || {
        doc_with_body(concat!(
            r#"<w:p><w:r><w:t>kept</w:t></w:r></w:p>"#,
            r#"<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="100"/></w:tblGrid><w:tr><w:tc><w:p>"#,
            r#"<w:bookmarkStart w:id="9" w:name="Inside"/><w:r><w:t>cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#,
            r#"<w:bookmarkEnd w:id="9"/><w:p><w:r><w:t>end</w:t></w:r></w:p>"#,
        ))
    };
    let mut doc = table();
    doc.move_content(&item_at(&doc, 1), &ContentLocation::end(body(&doc)))
        .unwrap();
    assert_eq!(body_shape(&mut doc), "PPTE");
    let mut doc = table();
    doc.clone_content(&item_at(&doc, 1), &ContentLocation::end(body(&doc)))
        .unwrap();
    assert_eq!(body_shape(&mut doc), "PTEPTE");
    let mut doc = table();
    assert!(doc.remove_content(1));
    assert_eq!(body_shape(&mut doc), "PP");
    // A range from the heading to the wrapped table takes the heading's
    // start and the table's end: five items.
    let mut doc = with_wrapped_blocks();
    assert_eq!(doc.remove_content_range(2, 5).unwrap(), 5);
    assert_eq!(body_shape(&mut doc), "PP");
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
