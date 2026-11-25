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

/// The media parts the document would save.
fn media_parts(doc: &Document) -> Vec<String> {
    doc.part_names()
        .expect("document saves")
        .into_iter()
        .filter(|name| name.starts_with("/word/media/"))
        .collect()
}

#[test]
fn inheriting_again_drops_the_pictures_only_the_copy_showed() {
    // A logo or a signature in a dropped header must not travel with the file.
    let mut doc = inherited_header_and_footer();
    doc.unlink_section_story(1, HeaderFooterKind::Header, HdrFtrType::Default)
        .unwrap();
    let copy = header_story(&doc, 1).story().clone();
    doc.add_picture_to_story(&copy, PNG, "logo.png", twips(567), twips(567))
        .unwrap();
    let mut doc = reopen(&mut doc);
    assert_eq!(media_parts(&doc).len(), 1, "the copy's picture is saved");

    doc.inherit_section_story(1, HeaderFooterKind::Header, HdrFtrType::Default)
        .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(header_part_count(&doc), 1, "the copy leaves the file");
    assert!(media_parts(&doc).is_empty(), "its picture goes with it");
}

#[test]
fn inheriting_again_keeps_a_picture_the_body_also_shows() {
    let mut doc = inherited_header_and_footer();
    doc.add_picture(PNG, "logo.png", twips(567), twips(567));
    doc.unlink_section_story(1, HeaderFooterKind::Header, HdrFtrType::Default)
        .unwrap();
    let copy = header_story(&doc, 1).story().clone();
    doc.add_picture_to_story(&copy, PNG, "logo.png", twips(567), twips(567))
        .unwrap();
    let mut doc = reopen(&mut doc);
    let saved = media_parts(&doc);
    assert_eq!(saved.len(), 2, "one picture for the body, one for the copy");

    doc.inherit_section_story(1, HeaderFooterKind::Header, HdrFtrType::Default)
        .unwrap();
    let doc = reopen(&mut doc);
    let kept = media_parts(&doc);
    assert_eq!(
        kept.len(),
        1,
        "the body's picture stays: {saved:?} -> {kept:?}"
    );
    let body_relationships = doc.part_xml("word/_rels/document.xml.rels").unwrap();
    let file = kept[0].rsplit('/').next().unwrap();
    assert!(body_relationships.contains(file), "{body_relationships}");
    assert_eq!(doc.pictures().unwrap().len(), 1);
}

#[test]
fn inheriting_again_keeps_a_logo_the_first_section_still_shows() {
    // Unlinking copies the header with its relationships to the same logo,
    // so the logo has two users until the copy goes.
    let mut doc = inherited_header_and_footer();
    let original = header_story(&doc, 0).story().clone();
    doc.add_picture_to_story(&original, PNG, "logo.png", twips(567), twips(567))
        .unwrap();
    doc.unlink_section_story(1, HeaderFooterKind::Header, HdrFtrType::Default)
        .unwrap();
    let mut doc = reopen(&mut doc);
    assert_eq!(header_part_count(&doc), 2);
    let logo = media_parts(&doc);
    assert_eq!(logo.len(), 1, "the copy shares the logo");

    doc.inherit_section_story(1, HeaderFooterKind::Header, HdrFtrType::Default)
        .unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(header_part_count(&doc), 1, "the copy leaves the file");
    assert_eq!(media_parts(&doc), logo, "the first section's logo stays");
    let header_part = header_story(&doc, 0).story().part_name().to_owned();
    let (folder, file) = header_part
        .trim_start_matches('/')
        .rsplit_once('/')
        .unwrap();
    let header_relationships = doc
        .part_xml(&format!("{folder}/_rels/{file}.rels"))
        .unwrap();
    let logo_file = logo[0].rsplit('/').next().unwrap();
    assert!(
        header_relationships.contains(logo_file),
        "{header_relationships}"
    );
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

// ---- Watermarks and pictures ----

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

/// The text of the first-page header a section shows.
fn first_page_header(doc: &Document, section: usize) -> Option<String> {
    shown_text(doc, section, HeaderFooterKind::Header, HdrFtrType::First)
}

#[test]
fn a_first_page_header_the_last_section_shows_by_inheritance_changes_everywhere() {
    let mut doc = doc_with(&["cover"]);
    doc.set_first_page_header("Cover");
    doc.insert_section(1).unwrap();
    doc.add_paragraph("chapter");
    doc.set_different_first_page(true);
    assert_eq!(first_page_header(&doc, 1).as_deref(), Some("Cover"));
    doc.set_first_page_header("Chapter");
    let doc = reopen(&mut doc);
    assert_eq!(first_page_header(&doc, 0).as_deref(), Some("Chapter"));
    assert_eq!(first_page_header(&doc, 1).as_deref(), Some("Chapter"));
    assert!(
        shown(&doc, 1, HeaderFooterKind::Header, HdrFtrType::First)
            .unwrap()
            .is_inherited()
    );
}

/// The main part of a saved package.
fn main_part(bytes: &[u8]) -> Vec<u8> {
    let package = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes.to_vec()))
        .expect("package opens");
    let part = package.main_document_part().expect("main document part");
    package.get_part(&part).expect("main part").to_vec()
}

/// A document with a default, a first-page and an even-page header, whose
/// final section lists the header references in `order`, as other
/// producers write them.
fn headers_listed(order: &[&str]) -> Vec<u8> {
    let mut doc = Document::new();
    doc.add_paragraph("Body.");
    doc.set_header("Default header");
    doc.set_first_page_header("First header");
    let bytes = doc.to_bytes().expect("document saves");
    let mut package =
        oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes)).expect("package opens");
    let main = package.main_document_part().expect("main document part");
    let xml = String::from_utf8(package.get_part(&main).expect("main part").to_vec())
        .expect("main part is UTF-8");
    // An even-page header: a copy of the default one, under its own part.
    let reference = |kind: &str| {
        let start = xml
            .find(&format!(r#"<w:headerReference w:type="{kind}""#))
            .unwrap_or_else(|| panic!("{kind} header reference"));
        let end = start + xml[start..].find("/>").expect("reference end") + 2;
        (start..end, xml[start..end].to_owned())
    };
    let (_, default) = reference("default");
    let default_id = default
        .split(r#"r:id=""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("default header relationship")
        .to_owned();
    let rels = package
        .get_part_rels(&main)
        .expect("main part relationships");
    let target = rels
        .items
        .iter()
        .find(|relationship| relationship.id == default_id)
        .expect("default header relationship")
        .target
        .clone();
    let default_part = format!("/word/{target}");
    let header_type = rels
        .items
        .iter()
        .find(|relationship| relationship.id == default_id)
        .expect("default header relationship")
        .rel_type
        .clone();
    let header = package
        .get_part(&default_part)
        .expect("default header part")
        .to_vec();
    package.set_part("/word/headerEven.xml", header);
    package.content_types.add_override(
        "/word/headerEven.xml",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml",
    );
    package
        .get_or_create_part_rels(&main)
        .add_with_id("rIdEven", &header_type, "headerEven.xml");
    let even = r#"<w:headerReference w:type="even" r:id="rIdEven"/>"#.to_owned();
    let (first_range, first) = reference("first");
    let (default_range, _) = reference("default");
    let mut listed = String::new();
    for kind in order {
        listed.push_str(match *kind {
            "default" => &default,
            "first" => &first,
            "even" => &even,
            other => panic!("unknown header type {other}"),
        });
    }
    let (from, to) = (
        first_range.start.min(default_range.start),
        first_range.end.max(default_range.end),
    );
    let xml = format!("{}{listed}{}", &xml[..from], &xml[to..]);
    package.set_part(&main, xml.into_bytes());
    let mut out = std::io::Cursor::new(Vec::new());
    package.write_to(&mut out).expect("package writes");
    out.into_inner()
}

/// The `kind` (header or footer) reference types of the final section, in
/// the order written.
fn reference_order(bytes: &[u8], kind: &str) -> Vec<String> {
    let xml = String::from_utf8(main_part(bytes)).unwrap();
    xml.split(&format!("<w:{kind}Reference w:type=\""))
        .skip(1)
        .map(|rest| rest.split('"').next().unwrap().to_owned())
        .collect()
}

fn header_order(bytes: &[u8]) -> Vec<String> {
    reference_order(bytes, "header")
}

#[test]
fn linking_a_header_to_another_story_keeps_its_place_among_the_references() {
    // Neither pushing the reference at the end nor placing it by type keeps
    // this order.
    let bytes = headers_listed(&["even", "default", "first"]);
    let mut doc = Document::from_bytes(&bytes).unwrap();
    let first = doc
        .section_story(0, HeaderFooterKind::Header, HdrFtrType::First)
        .unwrap()
        .expect("first-page header")
        .story()
        .clone();
    doc.link_section_story(0, HeaderFooterKind::Header, HdrFtrType::Default, &first)
        .unwrap();
    assert_eq!(
        header_order(&doc.to_bytes().unwrap()),
        ["even", "default", "first"]
    );
}

#[test]
fn a_story_inherited_from_a_removed_section_takes_its_place_in_the_order() {
    let mut doc = two_sections();
    for kind in [HeaderFooterKind::Header, HeaderFooterKind::Footer] {
        doc.create_section_story(0, kind, HdrFtrType::Default)
            .unwrap();
        doc.create_section_story(1, kind, HdrFtrType::First)
            .unwrap();
    }
    doc.remove_section(0).unwrap();
    let bytes = doc.to_bytes().unwrap();
    assert_eq!(reference_order(&bytes, "header"), ["default", "first"]);
    assert_eq!(reference_order(&bytes, "footer"), ["default", "first"]);
}

#[test]
fn a_watermark_header_takes_its_place_in_the_order() {
    let mut doc = Document::new();
    doc.add_paragraph("Body.");
    doc.set_first_page_header("First header");
    doc.set_text_watermark("DRAFT").unwrap();
    assert_eq!(header_order(&doc.to_bytes().unwrap()), ["default", "first"]);
}

#[test]
fn section_twips_measures_written_with_a_unit_open() {
    // The section of a paragraph, so the template's own sectPr stays last.
    let doc = crate::support::doc_with_body(concat!(
        r#"<w:p><w:pPr><w:sectPr><w:pgSz w:w="8.5in" w:h="11in"/>"#,
        r#"<w:pgMar w:top="1in" w:right="2cm" w:bottom="72pt" w:left="6pc" w:header="0.5in" w:footer="0.25in" w:gutter="0"/>"#,
        r#"<w:lnNumType w:countBy="1" w:distance="0.25in"/>"#,
        r#"<w:cols w:space="0.5in"/></w:sectPr></w:pPr><w:r><w:t>Body</w:t></w:r></w:p>"#,
        r#"<w:p><w:pPr><w:sectPr><w:cols w:num="2" w:equalWidth="0"><w:col w:w="3in" w:space="0.5in"/>"#,
        r#"<w:col w:w="2in"/></w:cols></w:sectPr></w:pPr><w:r><w:t>Columns</w:t></w:r></w:p>"#,
    ));
    let section = doc.section(0).expect("the section");
    assert_eq!(
        section.page_size(),
        Some((Length::twips(12240), Length::twips(15840)))
    );
    assert_eq!(
        section.margins(),
        Some((
            Length::twips(1440),
            Length::twips(1134),
            Length::twips(1440),
            Length::twips(1440)
        ))
    );
    assert_eq!(
        section.header_footer_distance(),
        Some((Length::twips(720), Length::twips(360)))
    );
    assert_eq!(
        section.columns().map(|(_, space)| space),
        Some(Length::twips(720))
    );
    assert_eq!(
        section
            .line_numbers()
            .and_then(|numbers| numbers.distance)
            .map(|d| d.0),
        Some(360)
    );
    let columns = doc.section(1).expect("the second section").column_widths();
    assert_eq!(
        columns,
        Some(vec![
            (Length::twips(4320), Length::twips(720)),
            (Length::twips(2880), Length::twips(0))
        ])
    );
}

/// The header and footer references of every section of a saved package,
/// each as its kind and type ("h:even", "f:default"), in the order written.
fn section_references(bytes: &[u8]) -> Vec<Vec<String>> {
    let xml = String::from_utf8(main_part(bytes)).unwrap();
    xml.split("<w:sectPr")
        .skip(1)
        .filter(|section| !section.starts_with("Change"))
        .map(|section| {
            let section = &section[..section.find("</w:sectPr>").unwrap_or(section.len())];
            let mut references = Vec::new();
            for (kind, rest) in section.split("<w:").filter_map(|tag| {
                tag.strip_prefix("headerReference ")
                    .map(|rest| ("h", rest))
                    .or_else(|| tag.strip_prefix("footerReference ").map(|rest| ("f", rest)))
            }) {
                let tag = &rest[..rest.find('>').unwrap()];
                let hdr_type = tag
                    .split("w:type=\"")
                    .nth(1)
                    .and_then(|value| value.split('"').next())
                    .unwrap_or("default");
                references.push(format!("{kind}:{hdr_type}"));
            }
            references
        })
        .collect()
}

#[test]
fn an_edited_document_keeps_its_header_and_footer_references_in_the_order_it_read() {
    // Word interleaves them: its even and default headers, their footers,
    // then the first-page header and footer. The authored thesis mixes them
    // its own way, LibreOffice and Google write the headers first.
    for name in [
        "word/thesis.docx",
        "word/letter.docx",
        "authored/thesis.docx",
        "libreoffice/letter.docx",
        "google/letter.docx",
    ] {
        let mut doc = corpus_doc(name);
        let read = section_references(&doc.to_bytes().unwrap());
        if name == "word/thesis.docx" {
            assert_eq!(
                read[0],
                [
                    "h:even",
                    "h:default",
                    "f:even",
                    "f:default",
                    "h:first",
                    "f:first"
                ]
            );
        }
        doc.add_paragraph("Added.");
        assert_eq!(section_references(&doc.to_bytes().unwrap()), read, "{name}");
    }
}

#[test]
fn new_header_and_footer_references_are_written_header_then_footer_per_type() {
    // The order Word writes a default and a first-page header and footer in.
    let mut doc = Document::new();
    doc.add_paragraph("Body.");
    doc.set_header("Header");
    doc.set_first_page_header("First header");
    doc.set_footer("Footer");
    doc.set_first_page_footer("First footer");
    assert_eq!(
        section_references(&doc.to_bytes().unwrap()),
        [["h:default", "f:default", "h:first", "f:first"]]
    );
}

#[test]
fn relinking_a_header_or_a_footer_of_a_word_file_keeps_its_place_among_the_others() {
    let mut doc = corpus_doc("word/letter.docx");
    let read = section_references(&doc.to_bytes().unwrap());
    assert_eq!(read, [["h:default", "f:default", "h:first", "f:first"]]);
    let story = |doc: &Document, kind, hdr_type| {
        doc.section_story(0, kind, hdr_type)
            .unwrap()
            .expect("the section has this story")
            .story()
            .clone()
    };
    let first_header = story(&doc, HeaderFooterKind::Header, HdrFtrType::First);
    let default_footer = story(&doc, HeaderFooterKind::Footer, HdrFtrType::Default);
    doc.link_section_story(
        0,
        HeaderFooterKind::Header,
        HdrFtrType::Default,
        &first_header,
    )
    .unwrap();
    doc.link_section_story(
        0,
        HeaderFooterKind::Footer,
        HdrFtrType::First,
        &default_footer,
    )
    .unwrap();
    let mut doc = reopen(&mut doc);
    assert_eq!(section_references(&doc.to_bytes().unwrap()), read);
    assert_eq!(header(&doc, 0), first_page_header(&doc, 0));
}

#[test]
fn references_inherited_from_a_removed_section_are_new_to_the_next_one() {
    // The first section of the Word thesis has all six references, the
    // second its own even and default headers only. Removing the first, the
    // second inherits the four others, placed as new references: each just
    // before the next one read in its list, else after those read.
    let mut doc = corpus_doc("word/thesis.docx");
    doc.remove_section(0).unwrap();
    let saved = section_references(&doc.to_bytes().unwrap());
    assert_eq!(
        saved[0],
        [
            "h:first",
            "h:even",
            "h:default",
            "f:default",
            "f:first",
            "f:even"
        ]
    );
}

#[test]
fn a_header_set_over_one_read_keeps_its_place_among_the_others() {
    // The Word letter has a default and a first-page header and footer.
    // With its title page turned off, a new first-page header replaces the
    // one read: it takes the place of the one it replaces.
    let mut doc = corpus_doc("word/letter.docx");
    let read = section_references(&doc.to_bytes().unwrap());
    assert_eq!(read, [["h:default", "f:default", "h:first", "f:first"]]);
    doc.set_different_first_page(false);
    doc.set_first_page_header("New first page");
    // A body edit, so that the save writes the section again.
    doc.add_paragraph("Added.");
    let mut doc = reopen(&mut doc);
    assert_eq!(section_references(&doc.to_bytes().unwrap()), read);
    assert_eq!(
        first_page_header(&doc, 0).as_deref(),
        Some("New first page")
    );
}

#[test]
fn an_unknown_element_between_header_and_footer_references_keeps_its_place() {
    let source = corpus_doc("word/letter.docx").to_bytes().unwrap();
    let mut package = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(source)).unwrap();
    let part = package.main_document_part().unwrap();
    let xml = String::from_utf8(package.get_part(&part).unwrap().to_vec()).unwrap();
    let footer = r#"<w:footerReference w:type="default" r:id="rId12"/>"#;
    let foreign = r#"<x:mark xmlns:x="urn:example"/>"#;
    assert_eq!(xml.matches(footer).count(), 1);
    package.set_part(
        &part,
        xml.replace(footer, &format!("{footer}{foreign}"))
            .into_bytes(),
    );
    let mut out = std::io::Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let mut doc = Document::from_bytes(&out.into_inner()).unwrap();
    doc.add_paragraph("Added.");
    let xml = String::from_utf8(main_part(&doc.to_bytes().unwrap())).unwrap();
    let section = &xml[xml.rfind("<w:sectPr").unwrap()..];
    let after_footer = &section[section
        .find(r#"<w:footerReference w:type="default""#)
        .unwrap()..];
    let after_footer = &after_footer[after_footer.find("/>").unwrap() + 2..];
    assert!(after_footer.starts_with(foreign), "{section}");
    assert!(
        after_footer[foreign.len()..].starts_with(r#"<w:headerReference w:type="first""#),
        "{section}"
    );
}

// ---- Sections in RTF ----

/// Each section's break, orientation, page size, margins and columns, with
/// Word's defaults (next page, portrait, equal columns) spelled out.
fn section_layouts(doc: &Document) -> Vec<String> {
    doc.sections()
        .map(|section| {
            let properties = section.properties();
            let columns = properties.columns.clone().map(|mut columns| {
                columns.equal_width = columns.equal_width.filter(|equal| !equal);
                columns
            });
            format!(
                "{:?} {:?} {:?} {:?} {:?} {:?} {:?}",
                properties.section_type.unwrap_or(ST_SectionType::NextPage),
                properties
                    .orientation
                    .unwrap_or(ST_PageOrientation::Portrait),
                properties.page_width.zip(properties.page_height),
                section.margins(),
                properties.gutter,
                properties.header_distance.zip(properties.footer_distance),
                columns,
            )
        })
        .collect()
}

#[test]
fn rtf_round_trip_keeps_the_layout_of_each_section() {
    let doc = crate::support::doc_with_body(concat!(
        r#"<w:p><w:r><w:t>title</w:t></w:r></w:p>"#,
        r#"<w:p><w:pPr><w:sectPr><w:pgSz w:w="11906" w:h="16838"/>"#,
        r#"<w:pgMar w:top="1134" w:right="1134" w:bottom="1134" w:left="1134" w:header="567" w:footer="567" w:gutter="0"/>"#,
        r#"</w:sectPr></w:pPr></w:p>"#,
        r#"<w:p><w:r><w:t>two columns</w:t></w:r></w:p>"#,
        r#"<w:p><w:pPr><w:sectPr><w:type w:val="continuous"/><w:pgSz w:w="11906" w:h="16838"/>"#,
        r#"<w:pgMar w:top="1134" w:right="1134" w:bottom="1134" w:left="1134" w:header="567" w:footer="567" w:gutter="0"/>"#,
        r#"<w:cols w:num="2" w:space="425" w:sep="1"/></w:sectPr></w:pPr><w:r><w:t>end of two</w:t></w:r></w:p>"#,
        r#"<w:p><w:r><w:t>landscape</w:t></w:r></w:p>"#,
        r#"<w:sectPr><w:type w:val="nextPage"/><w:pgSz w:w="15840" w:h="12240" w:orient="landscape"/>"#,
        r#"<w:pgMar w:top="720" w:right="1440" w:bottom="720" w:left="1440" w:header="720" w:footer="720" w:gutter="360"/>"#,
        r#"<w:cols w:num="2" w:equalWidth="0"><w:col w:w="8000" w:space="720"/><w:col w:w="4240"/></w:cols>"#,
        r#"</w:sectPr>"#,
    ));
    let written = doc.to_rtf_bytes().unwrap();
    assert!(
        !written
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("section")),
        "{:?}",
        written.diagnostics
    );
    let rtf = String::from_utf8(written.bytes).unwrap();
    // The document's page is the first section's; each section restates its own.
    assert!(rtf.contains(r"\paperw11906\paperh16838\margl1134"), "{rtf}");
    assert!(rtf.contains(r"\sect"), "{rtf}");
    assert!(rtf.contains(r"\sectd\sbknone\pgwsxn11906"), "{rtf}");
    assert!(rtf.contains(r"\cols2\colsx425\linebetcol"), "{rtf}");
    assert!(
        rtf.contains(r"\colno1\colw8000\colsr720\colno2\colw4240"),
        "{rtf}"
    );
    let read = Document::from_rtf_bytes(rtf.as_bytes()).unwrap().document;
    assert_eq!(section_layouts(&read), section_layouts(&doc));
    assert_eq!(texts(&read), texts(&doc));
}

#[test]
fn rtf_import_reads_sections_as_word_writes_them() {
    // The document's page in the header, `\sectd` before the body, and a
    // section ended by `\sect` in a run group, the next one continuous in
    // two columns.
    let rtf = br"{\rtf1\ansi\deff0{\fonttbl{\f0 Calibri;}}
\paperw11906\paperh16838\margl1134\margr1134\margt1134\margb1134\gutter0\ltrsect
\widowctrl\sectd \ltrsect\linex0\endnhere\sectlinegrid360\sectdefaultcl\sftnbj
\pard\plain title\par
\pard\plain {\insrsid1 \sect }\sectd \ltrsect\sbknone\linex0\cols2\colsx425\colno0\colw9\endnhere\sftnbj
\pard\plain body\par}";
    let read = Document::from_rtf_bytes(rtf).unwrap();
    // The layout words are read, a column number out of range is reported.
    let messages: Vec<&str> = read
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    for word in [
        "paperw", "margl", "gutter", "sectd", "sect", "sbknone", "cols", "colw",
    ] {
        assert!(
            !messages
                .iter()
                .any(|message| message.contains(&format!("\\{word} "))),
            "{word}: {messages:?}"
        );
    }
    assert!(
        messages.contains(&r"RTF column \colno0 is out of range and was dropped"),
        "{messages:?}"
    );
    let doc = read.document;
    assert_eq!(texts(&doc), ["title", "", "body"]);
    let sections: Vec<_> = doc.sections().collect();
    assert_eq!(sections.len(), 2);
    for section in &sections {
        assert_eq!(
            section.page_size(),
            Some((Length::twips(11906), Length::twips(16838)))
        );
        assert_eq!(
            section.margins().map(|margins| margins.0),
            Some(Length::twips(1134))
        );
    }
    assert_eq!(sections[0].columns(), None);
    assert_eq!(sections[1].break_type(), Some(ST_SectionType::Continuous));
    assert_eq!(sections[1].columns(), Some((2, Length::twips(425))));
}

#[test]
fn rtf_round_trip_keeps_a_portrait_section_after_a_landscape_one() {
    // RTF has no word for portrait: the document's `\landscape` must not
    // reach a section that states its own page.
    let doc = crate::support::doc_with_body(concat!(
        r#"<w:p><w:pPr><w:sectPr><w:pgSz w:w="15840" w:h="12240" w:orient="landscape"/>"#,
        r#"<w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/>"#,
        r#"</w:sectPr></w:pPr><w:r><w:t>wide</w:t></w:r></w:p>"#,
        r#"<w:p><w:r><w:t>tall</w:t></w:r></w:p>"#,
    ));
    let rtf = doc.to_rtf_bytes().unwrap().bytes;
    let read = Document::from_rtf_bytes(&rtf).unwrap().document;
    assert_eq!(section_layouts(&read), section_layouts(&doc));
    let orientations: Vec<_> = read
        .sections()
        .map(|section| section.orientation())
        .collect();
    assert_eq!(
        orientations,
        [
            Some(ST_PageOrientation::Landscape),
            Some(ST_PageOrientation::Portrait)
        ]
    );
    // A section that states a wider than tall page without `\lndscpsxn`.
    let rtf =
        br"{\rtf1\ansi\landscape\paperw15840\paperh12240\sectd\pgwsxn15840\pghsxn12240 wide\par}";
    let read = Document::from_rtf_bytes(rtf).unwrap().document;
    assert_eq!(
        read.section(0).unwrap().orientation(),
        Some(ST_PageOrientation::Landscape)
    );
}

#[test]
fn rtf_export_reports_the_section_parts_it_drops() {
    // A tracked section change, a child the model keeps as XML, and section
    // properties in a table cell, which only a body paragraph can end.
    let doc = crate::support::doc_with_body(concat!(
        r#"<w:p><w:pPr><w:sectPr><w:pgNumType xmlns:x="http://schemas.openxmlformats.org/wordprocessingml/2006/main" x:fmt="lowerRoman"/>"#,
        r#"<w:bidi/><w:sectPrChange w:id="1" w:author="A" w:date="2026-11-23T00:00:00Z">"#,
        r#"<w:sectPr/></w:sectPrChange></w:sectPr></w:pPr><w:r><w:t>first</w:t></w:r></w:p>"#,
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc>"#,
        r#"<w:p><w:pPr><w:sectPr><w:pgSz w:w="12240" w:h="15840"/></w:sectPr></w:pPr>"#,
        r#"<w:r><w:t>cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#,
        r#"<w:p><w:r><w:t>last</w:t></w:r></w:p>"#,
    ));
    let messages: Vec<String> = doc
        .to_rtf_bytes()
        .unwrap()
        .diagnostics
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect();
    for expected in [
        "section page numbering was dropped during RTF export",
        "section property revision was dropped during RTF export",
        "unmodelled section XML was dropped during RTF export",
        "section properties of a table-cell paragraph were dropped during RTF export",
    ] {
        assert!(
            messages.iter().any(|message| message == expected),
            "{expected}: {messages:?}"
        );
    }
    // LibreOffice writes decimal numbering, left-to-right text and form
    // protection off in every section: nothing is dropped.
    let libreoffice = corpus_doc("libreoffice/paper.docx").to_rtf_bytes().unwrap();
    assert!(
        !libreoffice.diagnostics.iter().any(|diagnostic| {
            diagnostic.message.contains("page numbering")
                || diagnostic.message.contains("text direction")
                || diagnostic.message.contains("unmodelled section")
        }),
        "{:?}",
        libreoffice.diagnostics
    );
}

#[test]
fn rtf_export_reports_a_dropped_first_page_setting() {
    let mut doc = corpus_doc("word/paper.docx");
    doc.section_mut(0).unwrap().set_different_first_page(true);
    let written = doc.to_rtf_bytes().unwrap();
    let messages: Vec<&str> = written
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    assert!(
        messages.contains(&"section first-page setting was dropped during RTF export"),
        "{messages:?}"
    );
    assert!(
        !messages
            .iter()
            .any(|message| message.contains("paragraph section properties")),
        "{messages:?}"
    );
}

#[test]
fn the_word_paper_keeps_its_sections_through_rtf() {
    let doc = corpus_doc("word/paper.docx");
    assert!(
        doc.sections()
            .any(|section| section.columns().is_some_and(|(count, _)| count == 2)),
        "{:?}",
        section_layouts(&doc)
    );
    let rtf = doc.to_rtf_bytes().unwrap().bytes;
    let read = Document::from_rtf_bytes(&rtf).unwrap().document;
    assert_eq!(section_layouts(&read), section_layouts(&doc));
}

// ---- Linking drops a story only once nothing uses it ----

/// The footer parts the document would save.
fn footer_part_count(doc: &Document) -> usize {
    doc.part_names()
        .expect("document saves")
        .iter()
        .filter(|name| name.starts_with("/word/footer"))
        .count()
}

/// Two sections, each showing a footer of its own: the first one's shared
/// footer and the second one's copy, which says "own footer". Returns the
/// document, the first story and the copy.
fn own_footer_in_second_section() -> (Document, StoryId, StoryId) {
    let mut doc = doc_with(&["first"]);
    doc.set_footer("shared footer");
    doc.insert_section(1).expect("a section appends");
    doc.add_paragraph("second");
    let shared = shown(&doc, 0, HeaderFooterKind::Footer, HdrFtrType::Default)
        .expect("the first section shows a footer")
        .story()
        .clone();
    let copy = doc
        .unlink_section_story(1, HeaderFooterKind::Footer, HdrFtrType::Default)
        .expect("the footer unlinks");
    write_in(&mut doc, &copy, "own footer");
    (doc, shared, copy)
}

#[test]
fn linking_a_section_away_from_its_own_footer_drops_the_footer_nothing_uses() {
    let (mut doc, shared, _) = own_footer_in_second_section();
    assert_eq!(footer_part_count(&doc), 2);
    doc.link_section_story(1, HeaderFooterKind::Footer, HdrFtrType::Default, &shared)
        .expect("the footer links");
    let doc = reopen(&mut doc);
    assert_eq!(
        footer_part_count(&doc),
        1,
        "the unused copy leaves the file"
    );
    assert_eq!(footer(&doc, 1).as_deref(), Some("shared footer"));
}

#[test]
fn linking_a_section_away_from_a_footer_another_section_shows_keeps_it() {
    let (mut doc, shared, _) = own_footer_in_second_section();
    doc.insert_section(2).expect("a third section appends");
    doc.add_paragraph("third");
    let copy = shown(&doc, 1, HeaderFooterKind::Footer, HdrFtrType::Default)
        .expect("the second section shows its copy")
        .story()
        .clone();
    doc.link_section_story(2, HeaderFooterKind::Footer, HdrFtrType::Default, &copy)
        .expect("the third section links the copy");
    doc.link_section_story(1, HeaderFooterKind::Footer, HdrFtrType::Default, &shared)
        .expect("the footer links");
    let doc = reopen(&mut doc);
    assert_eq!(
        footer_part_count(&doc),
        2,
        "the third section's footer stays"
    );
    assert_eq!(footer(&doc, 1).as_deref(), Some("shared footer"));
    let third = footer(&doc, 2).expect("the third section shows a footer");
    assert!(third.contains("own footer"), "{third}");
}

#[test]
fn linking_a_section_away_from_a_footer_raw_body_xml_names_keeps_it() {
    // A block the model keeps verbatim holds a section that names the copy:
    // the copy is still in use, though no modeled section shows it.
    let (mut doc, _, copy) = own_footer_in_second_section();
    let bytes = doc.to_bytes().expect("document saves");
    let mut package =
        oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes)).expect("package opens");
    let main = package.main_document_part().expect("main document part");
    let copy_name = copy.part_name().rsplit('/').next().unwrap().to_owned();
    let copy_id = package
        .get_part_rels(&main)
        .expect("main part relationships")
        .items
        .iter()
        .find(|relationship| relationship.target.ends_with(&copy_name))
        .expect("the copy's relationship")
        .id
        .clone();
    let xml = String::from_utf8(package.get_part(&main).unwrap().to_vec()).unwrap();
    let raw = format!(
        r#"<w:customXml w:element="kept"><w:p><w:pPr><w:sectPr><w:footerReference w:type="default" r:id="{copy_id}"/></w:sectPr></w:pPr></w:p></w:customXml>"#
    );
    let at = xml.find("<w:body>").expect("body") + "<w:body>".len();
    let xml = format!("{}{raw}{}", &xml[..at], &xml[at..]);
    package.set_part(&main, xml.into_bytes());
    let mut out = std::io::Cursor::new(Vec::new());
    package.write_to(&mut out).expect("package writes");
    let mut doc = Document::from_bytes(&out.into_inner()).expect("document opens");
    let copy = shown(&doc, 1, HeaderFooterKind::Footer, HdrFtrType::Default)
        .expect("the second section shows its copy")
        .story()
        .clone();
    let shared = shown(&doc, 0, HeaderFooterKind::Footer, HdrFtrType::Default)
        .expect("the first section shows a footer")
        .story()
        .clone();
    assert_ne!(copy, shared);
    doc.link_section_story(1, HeaderFooterKind::Footer, HdrFtrType::Default, &shared)
        .expect("the footer links");
    let mut doc = reopen(&mut doc);
    assert_eq!(
        footer_part_count(&doc),
        2,
        "the copy the raw block names stays"
    );
    assert_eq!(footer(&doc, 1).as_deref(), Some("shared footer"));
    let saved = doc.to_bytes().expect("document saves");
    let main_xml = String::from_utf8(main_part(&saved)).unwrap();
    assert!(
        main_xml.contains(&format!(r#"r:id="{copy_id}""#)),
        "{main_xml}"
    );
    let rels = doc.part_xml("word/_rels/document.xml.rels").unwrap();
    assert!(rels.contains(&format!(r#"Id="{copy_id}""#)), "{rels}");
}

#[test]
fn the_stories_of_a_header_part_read_alone_are_the_documents() {
    let (doc, shared, copy) = own_footer_in_second_section();
    for story in [&shared, &copy] {
        let alone = doc
            .stories_in_part(story.part_name())
            .expect("stories read");
        let all = doc
            .stories()
            .expect("stories read")
            .into_iter()
            .filter(|candidate| candidate.part_name() == story.part_name())
            .collect::<Vec<_>>();
        assert_eq!(alone, all);
        assert!(
            !alone.is_empty(),
            "{} holds a footer story",
            story.part_name()
        );
    }
    let main = doc.stories().expect("stories read")[0]
        .part_name()
        .to_owned();
    assert!(!doc.stories_in_part(&main).expect("stories read").is_empty());
}
