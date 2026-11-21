//! Sections and their page setup (size, orientation, margins, columns, alignment,
//! page and line numbering, break type), and the headers and footers each
//! section shows: set, read, inherited, linked, unlinked and removed.

use oxoffice_docx::{
    ContentFragment, ContentLocation, Document, HdrFtrType, HeaderFooterKind, Length, SectionBreak,
    SectionStory, StoryId, StoryItemKind, StoryKind, Twips,
};
use oxoffice_docx_oxml::document::{CT_LineNumber, ST_LineNumberRestart};
use oxoffice_docx_oxml::shared::{ST_PageOrientation, ST_SectionType};
use oxoffice_docx_oxml::table::ST_VerticalJc;
use oxoffice_docx_oxml::text::CT_P;

use crate::support::{corpus_doc, doc_with, reopen, texts};

const REPORT_HEADER: &str = "Northwind Analytics: Quarterly Operations Review";
const REPORT_FOOTER: &str = "Internal. Prepared by the operations team.";
const PRODUCERS: [&str; 4] = ["word", "google", "libreoffice", "authored"];

/// A one-pixel PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

/// A document of two sections: "first" in the first, "second" in the last.
fn two_sections() -> Document {
    let mut doc = doc_with(&["first"]);
    doc.insert_section(1).expect("a section appends");
    doc.add_paragraph("second");
    doc
}

/// A document of two sections whose last section inherits the header and
/// footer of the first.
fn inherited_header_and_footer() -> Document {
    let mut doc = doc_with(&["first"]);
    doc.set_header("shared header");
    doc.set_footer("shared footer");
    // The new section takes over as the last one and has no header of its own.
    doc.insert_section(1).expect("a section appends");
    doc.add_paragraph("second");
    doc
}

fn twips(value: i32) -> Length {
    Length::twips(value)
}

fn report(producer: &str) -> Document {
    corpus_doc(&format!("{producer}/report.docx"))
}

/// The story a section shows for this header or footer variant, if any.
fn shown(
    doc: &Document,
    section: usize,
    kind: HeaderFooterKind,
    variant: HdrFtrType,
) -> Option<SectionStory> {
    doc.section_story(section, kind, variant)
        .expect("section index in range")
}

/// The paragraph texts of a story, one line each.
fn story_text(doc: &Document, story: &StoryId) -> String {
    doc.story_items(story)
        .expect("story reads")
        .iter()
        .filter(|item| item.kind() == StoryItemKind::Paragraph)
        .map(|item| item.text().expect("paragraph text").unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n")
}

fn shown_text(
    doc: &Document,
    section: usize,
    kind: HeaderFooterKind,
    variant: HdrFtrType,
) -> Option<String> {
    shown(doc, section, kind, variant).map(|story| story_text(doc, story.story()))
}

/// The text of the default header a section shows.
fn header(doc: &Document, section: usize) -> Option<String> {
    shown_text(doc, section, HeaderFooterKind::Header, HdrFtrType::Default)
}

/// The text of the default footer a section shows.
fn footer(doc: &Document, section: usize) -> Option<String> {
    shown_text(doc, section, HeaderFooterKind::Footer, HdrFtrType::Default)
}

fn header_story(doc: &Document, section: usize) -> SectionStory {
    shown(doc, section, HeaderFooterKind::Header, HdrFtrType::Default)
        .expect("the section shows a header")
}

/// The header parts the document would save, whether a section shows them
/// or not.
fn header_part_count(doc: &Document) -> usize {
    doc.part_names()
        .expect("document saves")
        .iter()
        .filter(|name| name.starts_with("/word/header"))
        .count()
}

/// Append a paragraph of text to a header or footer story.
fn write_in(doc: &mut Document, story: &StoryId, text: &str) {
    let mut paragraph = CT_P::new();
    paragraph.add_run(text);
    let fragment = ContentFragment::paragraph(paragraph).expect("paragraph fragment");
    doc.insert_content(&ContentLocation::end(story.clone()), fragment)
        .expect("paragraph inserts");
}

/// Replace the text of the first paragraph of a story.
fn rewrite_first_paragraph(doc: &mut Document, story: &StoryId, text: &str) {
    let location = doc
        .story_items(story)
        .expect("story reads")
        .iter()
        .find(|item| item.kind() == StoryItemKind::Paragraph)
        .expect("the story has a paragraph")
        .location()
        .clone();
    doc.set_story_text(&location, text).expect("text replaces");
}

// ---- Sections ----

#[test]
fn a_new_document_has_one_letter_portrait_section() {
    let doc = Document::new();
    assert_eq!(doc.section_count(), 1);
    let section = doc.section(0).unwrap();
    assert!(section.is_final());
    assert_eq!(section.orientation(), Some(ST_PageOrientation::Portrait));
    assert_eq!(section.page_size(), Some((twips(12240), twips(15840))));
    let inch = twips(1440);
    assert_eq!(section.margins(), Some((inch, inch, inch, inch)));
    assert!(doc.section(1).is_none());
}

#[test]
fn appending_a_section_puts_later_paragraphs_in_it() {
    let doc = reopen(&mut two_sections());
    assert_eq!(doc.section_count(), 2);
    assert_eq!(
        doc.sections()
            .map(|section| (section.ordinal(), section.is_final()))
            .collect::<Vec<_>>(),
        [(0, false), (1, true)]
    );
    // The break that ends the first section is carried by an empty paragraph.
    assert_eq!(texts(&doc), ["first", "", "second"]);
}

#[test]
fn an_inserted_section_copies_the_page_setup_of_the_section_before() {
    let mut doc = doc_with(&["first"]);
    {
        let mut section = doc.section_mut(0).unwrap();
        section.set_orientation(ST_PageOrientation::Landscape);
        section
            .set_margins(twips(1080), twips(1260), twips(900), twips(1440))
            .unwrap();
        section.set_gutter(twips(360)).unwrap();
        section.set_page_number_start(7).unwrap();
        section.set_different_first_page(true);
    }
    doc.insert_section(1).unwrap();
    let doc = reopen(&mut doc);
    let (before, inserted) = (doc.section(0).unwrap(), doc.section(1).unwrap());
    assert_eq!(inserted.orientation(), Some(ST_PageOrientation::Landscape));
    assert_eq!(inserted.page_size(), Some((twips(15840), twips(12240))));
    assert_eq!(inserted.page_size(), before.page_size());
    assert_eq!(inserted.margins(), before.margins());
    assert_eq!(inserted.gutter(), Some(twips(360)));
    // Page numbering and the first page variant stay with the old section.
    assert_eq!(inserted.page_number_start(), None);
    assert_ne!(inserted.different_first_page(), Some(true));
}

#[test]
fn section_edits_out_of_range_are_refused_without_a_change() {
    let mut doc = two_sections();
    let before = doc.to_bytes().unwrap();
    assert!(doc.section_mut(2).is_none());
    assert!(doc.insert_section(3).is_err());
    assert!(doc.remove_section(2).is_err());
    assert!(
        doc.section_story(2, HeaderFooterKind::Header, HdrFtrType::Default)
            .is_err()
    );
    assert!(
        doc.create_section_story(9, HeaderFooterKind::Footer, HdrFtrType::Default)
            .is_err()
    );
    assert_eq!(doc.to_bytes().unwrap(), before);

    let mut single = doc_with(&["only"]);
    let before = single.to_bytes().unwrap();
    let error = single.remove_section(0).unwrap_err().to_string();
    assert!(error.contains("sole document section"), "{error}");
    assert_eq!(single.to_bytes().unwrap(), before);
}

#[test]
fn removing_a_section_keeps_its_paragraphs() {
    // A section that is not the last merges into the one after it and takes
    // its page setup.
    let mut doc = two_sections();
    doc.section_mut(0)
        .unwrap()
        .set_orientation(ST_PageOrientation::Landscape);
    doc.remove_section(0).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(doc.section_count(), 1);
    assert_eq!(texts(&doc), ["first", "second"]);
    assert_eq!(
        doc.section(0).unwrap().page_size(),
        Some((twips(12240), twips(15840)))
    );

    // Removing the last section gives its paragraphs the setup of the one
    // before.
    let mut doc = two_sections();
    doc.section_mut(0)
        .unwrap()
        .set_orientation(ST_PageOrientation::Landscape);
    doc.remove_section(1).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(doc.section_count(), 1);
    assert_eq!(texts(&doc), ["first", "second"]);
    assert_eq!(
        doc.section(0).unwrap().orientation(),
        Some(ST_PageOrientation::Landscape)
    );
}

#[test]
fn a_paragraph_can_end_a_landscape_section() {
    let mut doc = Document::new();
    doc.add_paragraph("wide table page")
        .section_break(SectionBreak::NextPage)
        .section_landscape();
    doc.add_paragraph("back to portrait");
    let doc = reopen(&mut doc);
    assert_eq!(doc.section_count(), 2);
    assert_eq!(texts(&doc), ["wide table page", "back to portrait"]);
    let wide = doc.section(0).unwrap();
    assert_eq!(wide.orientation(), Some(ST_PageOrientation::Landscape));
    assert_eq!(wide.page_size(), Some((twips(15840), twips(12240))));
    assert_eq!(wide.break_type(), Some(ST_SectionType::NextPage));
    // Portrait is the default, so a saved file need not spell it out.
    let narrow = doc.section(1).unwrap();
    assert_ne!(narrow.orientation(), Some(ST_PageOrientation::Landscape));
    assert_eq!(narrow.page_size(), Some((twips(12240), twips(15840))));
}

#[test]
fn every_section_break_type_survives_a_save() {
    for break_type in [
        ST_SectionType::NextPage,
        ST_SectionType::Continuous,
        ST_SectionType::EvenPage,
        ST_SectionType::OddPage,
        ST_SectionType::NextColumn,
    ] {
        let mut doc = two_sections();
        doc.section_mut(1).unwrap().set_break_type(break_type);
        let doc = reopen(&mut doc);
        assert_eq!(doc.section(1).unwrap().break_type(), Some(break_type));
    }
}

// ---- Page setup ----

#[test]
fn landscape_swaps_the_page_width_and_height() {
    let mut doc = doc_with(&["body"]);
    doc.set_landscape();
    let mut doc = reopen(&mut doc);
    let section = doc.section(0).unwrap();
    assert_eq!(section.orientation(), Some(ST_PageOrientation::Landscape));
    assert_eq!(section.page_size(), Some((twips(15840), twips(12240))));

    doc.set_portrait();
    let mut doc = reopen(&mut doc);
    assert_eq!(
        doc.section(0).unwrap().page_size(),
        Some((twips(12240), twips(15840)))
    );

    // A custom size turns with its section too.
    {
        let mut section = doc.section_mut(0).unwrap();
        section.set_page_size(twips(11906), twips(16838)).unwrap();
        section.set_orientation(ST_PageOrientation::Landscape);
    }
    let doc = reopen(&mut doc);
    assert_eq!(
        doc.section(0).unwrap().page_size(),
        Some((twips(16838), twips(11906)))
    );
}

#[test]
fn page_size_margins_gutter_and_header_distance_survive_a_save() {
    let mut doc = two_sections();
    {
        let mut section = doc.section_mut(1).unwrap();
        section.set_page_size(twips(11906), twips(16838)).unwrap();
        section
            .set_margins(twips(720), twips(1080), twips(1000), twips(1300))
            .unwrap();
        section.set_gutter(twips(567)).unwrap();
        section
            .set_header_footer_distance(twips(400), twips(500))
            .unwrap();
    }
    let doc = reopen(&mut doc);
    let section = doc.section(1).unwrap();
    assert_eq!(section.page_size(), Some((twips(11906), twips(16838))));
    assert_eq!(
        section.margins(),
        Some((twips(720), twips(1080), twips(1000), twips(1300)))
    );
    assert_eq!(section.gutter(), Some(twips(567)));
    assert_eq!(
        section.header_footer_distance(),
        Some((twips(400), twips(500)))
    );
    // The other section keeps its letter page.
    assert_eq!(
        doc.section(0).unwrap().page_size(),
        Some((twips(12240), twips(15840)))
    );
}

#[test]
fn invalid_page_setup_is_refused_and_leaves_the_section_as_it_was() {
    let mut doc = doc_with(&["body"]);
    let before = doc.to_bytes().unwrap();
    let beyond_a_twip_count = Length::emu((i64::from(i32::MAX) + 1) * 635);
    {
        let mut section = doc.section_mut(0).unwrap();
        assert!(section.set_page_size(twips(0), twips(15840)).is_err());
        assert!(
            section
                .set_page_size(beyond_a_twip_count, twips(15840))
                .is_err()
        );
        assert!(section.set_columns(0, twips(720)).is_err());
        assert!(section.set_columns(2, twips(-1)).is_err());
        assert!(section.set_column_widths(&[]).is_err());
        assert!(section.set_gutter(twips(-1)).is_err());
        assert!(
            section
                .set_header_footer_distance(twips(-1), twips(720))
                .is_err()
        );
        assert!(section.set_page_number_start(0).is_err());
    }
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn columns_survive_a_save() {
    let mut doc = two_sections();
    {
        let mut section = doc.section_mut(0).unwrap();
        section.set_columns(2, twips(720)).unwrap();
        section.set_column_separator(true);
    }
    doc.section_mut(1)
        .unwrap()
        .set_column_widths(&[(twips(3000), twips(360)), (twips(5000), twips(0))])
        .unwrap();
    let doc = reopen(&mut doc);

    let equal = doc.section(0).unwrap();
    assert_eq!(equal.columns(), Some((2, twips(720))));
    assert_eq!(equal.column_separator(), Some(true));
    assert_eq!(equal.column_widths(), None);

    let unequal = doc.section(1).unwrap();
    assert_eq!(
        unequal.column_widths(),
        Some(vec![(twips(3000), twips(360)), (twips(5000), twips(0))])
    );
    // Columns of different widths are not reported as equal columns.
    assert_eq!(unequal.columns(), None);
}

#[test]
fn vertical_alignment_survives_a_save() {
    for alignment in [
        ST_VerticalJc::Top,
        ST_VerticalJc::Center,
        ST_VerticalJc::Bottom,
        ST_VerticalJc::Both,
    ] {
        let mut doc = doc_with(&["title page"]);
        doc.section_mut(0)
            .unwrap()
            .set_vertical_alignment(alignment);
        let doc = reopen(&mut doc);
        assert_eq!(
            doc.section(0).unwrap().vertical_alignment(),
            Some(alignment)
        );
    }
}

#[test]
fn a_page_numbering_restart_survives_a_save() {
    let mut doc = two_sections();
    doc.section_mut(1)
        .unwrap()
        .set_page_number_start(5)
        .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(doc.section(0).unwrap().page_number_start(), None);
    assert_eq!(doc.section(1).unwrap().page_number_start(), Some(5));
}

#[test]
fn setting_the_page_number_start_keeps_the_page_number_format() {
    // LibreOffice writes `w:pgNumType w:fmt="decimal"`, which the facade does
    // not model: it must survive a new start.
    let mut doc = report("libreoffice");
    doc.section_mut(1)
        .unwrap()
        .set_page_number_start(3)
        .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(doc.section(1).unwrap().page_number_start(), Some(3));
    let xml = doc.part_xml("word/document.xml").unwrap();
    let elements = xml
        .match_indices("<w:pgNumType")
        .map(|(start, _)| &xml[start..start + xml[start..].find('>').unwrap()])
        .collect::<Vec<_>>();
    assert_eq!(elements.len(), 2, "{elements:?}");
    assert!(
        elements[1].contains(r#"w:fmt="decimal""#) && elements[1].contains(r#"w:start="3""#),
        "{}",
        elements[1]
    );
}

#[test]
fn line_numbering_survives_a_save() {
    let numbering = CT_LineNumber {
        count_by: Some(5),
        start: Some(1),
        distance: Some(Twips(360)),
        restart: Some(ST_LineNumberRestart::NewSection),
        ..CT_LineNumber::default()
    };
    let mut doc = two_sections();
    doc.section_mut(1)
        .unwrap()
        .set_line_numbers(numbering.clone());
    let doc = reopen(&mut doc);
    assert_eq!(doc.section(0).unwrap().line_numbers(), None);
    assert_eq!(doc.section(1).unwrap().line_numbers(), Some(&numbering));
}

// ---- Headers and footers ----

#[test]
fn a_new_document_shows_no_header_or_footer() {
    let doc = Document::new();
    assert_eq!(doc.header_text(), None);
    assert_eq!(doc.footer_text(), None);
    assert!(!doc.has_header_footer_content());
    for kind in [HeaderFooterKind::Header, HeaderFooterKind::Footer] {
        for variant in [HdrFtrType::Default, HdrFtrType::First, HdrFtrType::Even] {
            assert_eq!(shown(&doc, 0, kind, variant), None, "{kind:?} {variant:?}");
        }
    }
}

#[test]
fn header_and_footer_text_survive_a_save() {
    let mut doc = doc_with(&["body"]);
    doc.set_header("Quarterly review");
    doc.set_footer("Internal use only");
    let doc = reopen(&mut doc);
    assert_eq!(doc.header_text().as_deref(), Some("Quarterly review"));
    assert_eq!(doc.footer_text().as_deref(), Some("Internal use only"));
    assert_eq!(header(&doc, 0).as_deref(), Some("Quarterly review"));
    assert_eq!(footer(&doc, 0).as_deref(), Some("Internal use only"));
    assert!(doc.has_header_footer_content());
    assert_eq!(texts(&doc), ["body"]);
}

#[test]
fn header_text_keeps_unicode_and_right_to_left_text() {
    let text = "Bilan trimestriel \u{2013} \u{6771}\u{4EAC} \u{2013} \u{645}\u{631}\u{62D}\u{628}\u{627} \u{1F4C8}";
    let mut doc = doc_with(&["body"]);
    doc.set_header(text);
    doc.set_footer(text);
    let doc = reopen(&mut doc);
    assert_eq!(doc.header_text().as_deref(), Some(text));
    assert_eq!(doc.footer_text().as_deref(), Some(text));
}

#[test]
fn setting_the_header_again_replaces_it() {
    let mut doc = doc_with(&["body"]);
    doc.set_header("draft one");
    doc.set_header("draft two");
    let mut doc = reopen(&mut doc);
    assert_eq!(doc.header_text().as_deref(), Some("draft two"));
    assert_eq!(header_part_count(&doc), 1);

    // An empty text keeps a header, with nothing in it.
    doc.set_header("");
    let doc = reopen(&mut doc);
    assert_eq!(doc.header_text().as_deref(), Some(""));
    assert!(doc.has_header_footer_content());
    assert_eq!(header_part_count(&doc), 1);
}

#[test]
fn a_first_page_header_and_footer_turn_on_a_different_first_page() {
    let mut doc = doc_with(&["cover", "body"]);
    doc.try_set_header("Every page").unwrap();
    doc.try_set_footer("Every footer").unwrap();
    assert_eq!(doc.section(0).unwrap().different_first_page(), None);
    doc.try_set_first_page_header("Cover header").unwrap();
    doc.try_set_first_page_footer("Cover footer").unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(doc.section(0).unwrap().different_first_page(), Some(true));
    assert_eq!(doc.header_text().as_deref(), Some("Every page"));
    assert_eq!(doc.footer_text().as_deref(), Some("Every footer"));
    assert_eq!(
        shown_text(&doc, 0, HeaderFooterKind::Header, HdrFtrType::First).as_deref(),
        Some("Cover header")
    );
    assert_eq!(
        shown_text(&doc, 0, HeaderFooterKind::Footer, HdrFtrType::First).as_deref(),
        Some("Cover footer")
    );
}

#[test]
fn an_even_page_footer_turns_on_even_and_odd_headers() {
    let mut doc = doc_with(&["body"]);
    assert!(!doc.even_and_odd_headers());
    let even = doc
        .create_section_story(0, HeaderFooterKind::Footer, HdrFtrType::Even)
        .unwrap();
    assert!(doc.even_and_odd_headers());
    write_in(&mut doc, &even, "even pages");
    let mut doc = reopen(&mut doc);
    assert!(doc.even_and_odd_headers());
    assert_eq!(
        shown_text(&doc, 0, HeaderFooterKind::Footer, HdrFtrType::Even).as_deref(),
        Some("even pages")
    );
    // Odd pages do not borrow the even footer.
    assert_eq!(footer(&doc, 0), None);

    doc.set_even_and_odd_headers(false).unwrap();
    let doc = reopen(&mut doc);
    assert!(!doc.even_and_odd_headers());
}

// ---- Section stories ----

#[test]
fn a_later_section_inherits_the_header_of_the_section_before() {
    let doc = reopen(&mut inherited_header_and_footer());
    let own = header_story(&doc, 0);
    let inherited = header_story(&doc, 1);
    assert!(!own.is_inherited());
    assert!(inherited.is_inherited());
    assert_eq!(inherited.source_section(), 0);
    assert_eq!(inherited.story().part_name(), own.story().part_name());
    assert_eq!(header(&doc, 1).as_deref(), Some("shared header"));
    assert_eq!(footer(&doc, 1).as_deref(), Some("shared footer"));
}

#[test]
fn unlinking_gives_a_section_its_own_copy_to_edit() {
    let mut doc = inherited_header_and_footer();
    let copy = doc
        .unlink_section_story(1, HeaderFooterKind::Header, HdrFtrType::Default)
        .unwrap();
    assert_ne!(copy.part_name(), header_story(&doc, 0).story().part_name());
    assert_eq!(header(&doc, 1).as_deref(), Some("shared header"));
    rewrite_first_paragraph(&mut doc, &copy, "landscape pages");
    let doc = reopen(&mut doc);
    assert_eq!(header(&doc, 0).as_deref(), Some("shared header"));
    assert_eq!(header(&doc, 1).as_deref(), Some("landscape pages"));
    assert!(!header_story(&doc, 1).is_inherited());
    assert_eq!(header_part_count(&doc), 2);
}

#[test]
fn inheriting_again_in_the_same_session_drops_the_copy() {
    let mut doc = inherited_header_and_footer();
    doc.unlink_section_story(1, HeaderFooterKind::Header, HdrFtrType::Default)
        .unwrap();
    doc.inherit_section_story(1, HeaderFooterKind::Header, HdrFtrType::Default)
        .unwrap();
    let doc = reopen(&mut doc);
    assert!(header_story(&doc, 1).is_inherited());
    assert_eq!(header(&doc, 1).as_deref(), Some("shared header"));
    assert_eq!(header_part_count(&doc), 1);
}

// Fails today: the copy a section stops using stays in the saved file as a
// header part nothing references. Issue: #28
#[test]
#[should_panic(expected = "the copy leaves the file")]
fn inheriting_again_drops_the_copy_from_the_file() {
    let mut doc = inherited_header_and_footer();
    let copy = doc
        .unlink_section_story(1, HeaderFooterKind::Header, HdrFtrType::Default)
        .unwrap();
    rewrite_first_paragraph(&mut doc, &copy, "landscape pages");
    let mut doc = reopen(&mut doc);
    doc.inherit_section_story(1, HeaderFooterKind::Header, HdrFtrType::Default)
        .unwrap();
    let doc = reopen(&mut doc);
    assert!(header_story(&doc, 1).is_inherited());
    assert_eq!(header(&doc, 1).as_deref(), Some("shared header"));
    assert_eq!(header_part_count(&doc), 1, "the copy leaves the file");
}

#[test]
fn linking_points_a_section_at_the_header_of_another() {
    let mut doc = two_sections();
    doc.set_header("last section header");
    let first = doc
        .create_section_story(0, HeaderFooterKind::Header, HdrFtrType::Default)
        .unwrap();
    write_in(&mut doc, &first, "first section header");
    assert_eq!(header(&doc, 0).as_deref(), Some("first section header"));

    // A header slot refuses a footer story.
    let footer_story = doc
        .create_section_story(1, HeaderFooterKind::Footer, HdrFtrType::Default)
        .unwrap();
    let before = doc.to_bytes().unwrap();
    assert!(
        doc.link_section_story(
            0,
            HeaderFooterKind::Header,
            HdrFtrType::Default,
            &footer_story
        )
        .is_err()
    );
    assert_eq!(doc.to_bytes().unwrap(), before);

    let last = header_story(&doc, 1).story().clone();
    doc.link_section_story(0, HeaderFooterKind::Header, HdrFtrType::Default, &last)
        .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(header(&doc, 0).as_deref(), Some("last section header"));
    assert_eq!(
        header_story(&doc, 0).story().part_name(),
        header_story(&doc, 1).story().part_name()
    );
    // The header nothing shows any more is gone.
    assert_eq!(header_part_count(&doc), 1);
}

#[test]
fn replacing_a_section_header_copies_another_one() {
    let mut doc = inherited_header_and_footer();
    doc.remove_section_story(1, HeaderFooterKind::Header, HdrFtrType::Default)
        .unwrap();
    let source = header_story(&doc, 0).story().clone();
    let copy = doc
        .replace_section_story(1, HeaderFooterKind::Header, HdrFtrType::Default, &source)
        .unwrap();
    assert_ne!(copy.part_name(), source.part_name());
    rewrite_first_paragraph(&mut doc, &copy, "edited copy");
    let doc = reopen(&mut doc);
    assert_eq!(header(&doc, 0).as_deref(), Some("shared header"));
    assert_eq!(header(&doc, 1).as_deref(), Some("edited copy"));
}

#[test]
fn removing_a_section_header_blanks_it_for_that_section_only() {
    let mut doc = inherited_header_and_footer();
    doc.remove_section_story(1, HeaderFooterKind::Header, HdrFtrType::Default)
        .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(header(&doc, 0).as_deref(), Some("shared header"));
    assert_eq!(header(&doc, 1).as_deref(), Some(""));
    assert!(!header_story(&doc, 1).is_inherited());
    assert_eq!(footer(&doc, 1).as_deref(), Some("shared footer"));
}

#[test]
fn removing_a_section_keeps_the_header_its_successor_inherited() {
    let mut doc = inherited_header_and_footer();
    doc.remove_section(0).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(doc.section_count(), 1);
    assert_eq!(texts(&doc), ["first", "second"]);
    assert_eq!(doc.header_text().as_deref(), Some("shared header"));
    assert_eq!(doc.footer_text().as_deref(), Some("shared footer"));
}

// ---- Watermarks and pictures ----

#[test]
fn a_text_watermark_creates_a_header_that_survives_a_save() {
    let mut doc = doc_with(&["body"]);
    doc.set_text_watermark("CONFIDENTIAL").unwrap();
    let doc = reopen(&mut doc);
    assert!(doc.has_header_footer_content());
    let story = header_story(&doc, 0);
    let items = doc.story_items(story.story()).unwrap();
    assert!(
        items
            .iter()
            .any(|item| item.kind() == StoryItemKind::Drawing),
        "{:?}",
        items.iter().map(|item| item.kind()).collect::<Vec<_>>()
    );
    let xml = items
        .iter()
        .map(|item| String::from_utf8(item.xml().unwrap().into_owned()).unwrap())
        .collect::<String>();
    assert!(xml.contains("CONFIDENTIAL"), "{xml}");
    assert_eq!(texts(&doc), ["body"]);
}

#[test]
fn a_header_picture_survives_a_save() {
    let mut doc = doc_with(&["body"]);
    doc.set_header_image(PNG, "logo.png", Length::inches(1.0), Length::inches(0.5));
    let doc = reopen(&mut doc);
    let pictures = doc.pictures().unwrap();
    let in_header = pictures
        .iter()
        .filter(|picture| picture.location.story().kind() == StoryKind::Header)
        .collect::<Vec<_>>();
    assert_eq!(in_header.len(), 1, "{} pictures", pictures.len());
    assert_eq!(in_header[0].bytes, PNG);
    assert_eq!(in_header[0].width, 914_400);
    assert_eq!(in_header[0].height, 457_200);
}

// ---- The corpus reports ----

#[test]
fn every_corpus_report_has_a_portrait_section_then_a_landscape_section() {
    for producer in PRODUCERS {
        let doc = report(producer);
        assert_eq!(doc.section_count(), 2, "{producer}");
        let (portrait, landscape) = (doc.section(0).unwrap(), doc.section(1).unwrap());
        assert_ne!(
            portrait.orientation(),
            Some(ST_PageOrientation::Landscape),
            "{producer}"
        );
        assert_eq!(
            portrait.page_size(),
            Some((twips(12240), twips(15840))),
            "{producer}"
        );
        assert_eq!(
            landscape.orientation(),
            Some(ST_PageOrientation::Landscape),
            "{producer}"
        );
        assert_eq!(
            landscape.page_size(),
            Some((twips(15840), twips(12240))),
            "{producer}"
        );
        for section in [portrait, landscape] {
            assert_eq!(
                section.margins(),
                Some((twips(1440), twips(1247), twips(1440), twips(1247))),
                "{producer}"
            );
        }
    }
}

#[test]
fn every_corpus_report_shows_its_header_and_footer_on_both_sections() {
    for producer in PRODUCERS {
        let doc = report(producer);
        for section in 0..2 {
            assert_eq!(
                header(&doc, section).as_deref(),
                Some(REPORT_HEADER),
                "{producer} section {section}"
            );
            assert_eq!(
                footer(&doc, section).as_deref(),
                Some(REPORT_FOOTER),
                "{producer} section {section}"
            );
        }
    }
}

#[test]
fn header_text_reads_the_header_the_last_section_inherits() {
    // Word, Google Docs and python-docx put the header on the first section
    // and let the landscape section inherit it; LibreOffice gives the second
    // section its own copy.
    let mut missed = Vec::new();
    for producer in ["word", "google", "authored", "libreoffice"] {
        let doc = report(producer);
        if doc.header_text().as_deref() != Some(REPORT_HEADER)
            || doc.footer_text().as_deref() != Some(REPORT_FOOTER)
        {
            missed.push(format!(
                "{producer}: {:?} / {:?}",
                doc.header_text(),
                doc.footer_text()
            ));
        }
    }
    assert!(
        missed.is_empty(),
        "header_text and footer_text miss the inherited header and footer: {missed:?}"
    );
}

#[test]
fn header_text_follows_a_new_section_that_inherits_and_one_that_has_its_own() {
    let mut doc = Document::new();
    doc.add_paragraph("body");
    doc.set_header("h");
    doc.set_footer("f");
    doc.insert_section(1).unwrap();
    assert_eq!(doc.section_count(), 2);
    assert_eq!(
        doc.header_text().as_deref(),
        Some("h"),
        "inherited by the new last section"
    );
    assert_eq!(doc.footer_text().as_deref(), Some("f"));
    let doc = reopen(&mut doc);
    assert_eq!(doc.header_text().as_deref(), Some("h"), "after a reopen");
    assert_eq!(Document::new().header_text(), None, "no header anywhere");
}

#[test]
fn editing_the_header_of_a_word_report_changes_it_on_both_sections() {
    let mut doc = report("word");
    let paragraphs = doc.paragraphs().len();
    let story = header_story(&doc, 0).story().clone();
    rewrite_first_paragraph(&mut doc, &story, "Northwind Analytics: Annual Review");
    let doc = reopen(&mut doc);
    for section in 0..2 {
        assert_eq!(
            header(&doc, section).as_deref(),
            Some("Northwind Analytics: Annual Review")
        );
        assert_eq!(footer(&doc, section).as_deref(), Some(REPORT_FOOTER));
    }
    assert_eq!(
        doc.section(1).unwrap().orientation(),
        Some(ST_PageOrientation::Landscape)
    );
    assert_eq!(doc.paragraphs().len(), paragraphs);
}

// Fails today: set_header changes only the last section, the first keeps its header. Issue: #28
#[test]
#[should_panic(expected = "the first section still shows the old header")]
fn setting_the_header_of_a_word_report_changes_it_on_every_section() {
    let mut doc = report("word");
    doc.set_header("Northwind Analytics: Annual Review");
    let doc = reopen(&mut doc);
    assert_eq!(
        header(&doc, 1).as_deref(),
        Some("Northwind Analytics: Annual Review")
    );
    assert_eq!(
        header(&doc, 0).as_deref(),
        Some("Northwind Analytics: Annual Review"),
        "the first section still shows the old header"
    );
}

#[test]
fn changing_the_landscape_margins_of_a_google_report_survives_a_save() {
    let mut doc = report("google");
    let inch = twips(1440);
    doc.section_mut(1)
        .unwrap()
        .set_margins(inch, inch, inch, inch)
        .unwrap();
    let doc = reopen(&mut doc);
    let landscape = doc.section(1).unwrap();
    assert_eq!(landscape.margins(), Some((inch, inch, inch, inch)));
    assert_eq!(landscape.orientation(), Some(ST_PageOrientation::Landscape));
    assert_eq!(
        doc.section(0).unwrap().margins(),
        Some((twips(1440), twips(1247), twips(1440), twips(1247)))
    );
    assert_eq!(header(&doc, 1).as_deref(), Some(REPORT_HEADER));
}
