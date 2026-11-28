//! Slides and layouts: adding slides from a layout and filling their
//! placeholders, reading, moving, duplicating, hiding, removing and importing
//! slides, the slide size, masters and the layouts they offer, backgrounds,
//! sections, and the links and parts that must follow a slide through it all.

use std::io::Cursor;

use oxoffice_opc::OpcPackage;
use oxoffice_pptx::{
    Comment, CommentAuthor, Emu, Error, Fill, HeaderFooter, HeaderFooterDate, ImportSlideOptions,
    Presentation, Section, ShapeOwner, SlideRef, TransitionDirection, TransitionKind,
    ValidationIssue,
};

use crate::support::{corpus_deck, new_deck, reopen};

/// The layouts of the default template, as PowerPoint's gallery lists them.
const OFFICE_LAYOUTS: [&str; 11] = [
    "Title Slide",
    "Title and Content",
    "Section Header",
    "Two Content",
    "Comparison",
    "Title Only",
    "Blank",
    "Content with Caption",
    "Picture with Caption",
    "Title and Vertical Text",
    "Vertical Title and Text",
];

const CORPUS_TITLES: [&str; 4] = [
    "Quarterly Operations Review",
    "Highlights",
    "Key figures",
    "Revenue by quarter",
];

const HIGHLIGHTS_NOTES: &str = "Start with the revenue story, then churn.";

const SECTION_A: &str = "{6F1D2C3B-0001-4A5B-9C3D-00000000000A}";
const SECTION_B: &str = "{6F1D2C3B-0002-4A5B-9C3D-00000000000B}";

/// The index of the first layout named `name`. The facade adds slides by
/// layout index only, and that index follows the package, not the gallery.
fn layout_named(prs: &Presentation, name: &str) -> usize {
    (0..prs.layout_count())
        .find(|&index| prs.layout_name(index) == Some(name))
        .unwrap_or_else(|| panic!("no layout named {name:?}"))
}

/// A new deck with one "Title Only" slide per title.
fn deck_with_titles(titles: &[&str]) -> Presentation {
    let mut prs = new_deck();
    let layout = layout_named(&prs, "Title Only");
    for title in titles {
        let index = prs.len();
        prs.add_slide(layout).unwrap();
        prs.slide_mut(index)
            .unwrap()
            .shape_mut(0)
            .unwrap()
            .set_text(title)
            .unwrap();
    }
    prs
}

/// The title of every slide, empty for a slide without one.
fn titles(prs: &Presentation) -> Vec<String> {
    prs.slides()
        .map(|slide| {
            slide
                .title()
                .and_then(|title| title.text())
                .unwrap_or_default()
        })
        .collect()
}

fn slide_ids(prs: &Presentation) -> Vec<u32> {
    prs.slides().map(|slide| slide.id()).collect()
}

/// The name of the layout of every slide.
fn slide_layout_names(prs: &Presentation) -> Vec<String> {
    (0..prs.len())
        .map(|index| {
            let layout = prs.slide_layout_index(index).expect("slide has a layout");
            prs.layout_name(layout).unwrap_or_default().to_owned()
        })
        .collect()
}

/// The `(type, idx)` of every placeholder of one layout, in z-order.
fn layout_placeholders(prs: &Presentation, layout: usize) -> Vec<(Option<String>, u32)> {
    (0..)
        .map_while(|index| prs.part_shape_at(ShapeOwner::Layout(layout), &[index]).ok())
        .filter_map(|shape| {
            let idx = shape.placeholder_idx()?;
            Some((shape.placeholder_type().map(str::to_owned), idx))
        })
        .collect()
}

/// The `(type, idx)` of every placeholder of one slide, in z-order.
fn slide_placeholders(slide: SlideRef<'_>) -> Vec<(Option<String>, u32)> {
    slide
        .shapes()
        .filter_map(|shape| {
            let idx = shape.placeholder_idx()?;
            Some((shape.placeholder_type().map(str::to_owned), idx))
        })
        .collect()
}

fn placeholder(kind: Option<&str>, idx: u32) -> (Option<String>, u32) {
    (kind.map(str::to_owned), idx)
}

/// The saved `p:sldSz` element.
fn saved_slide_size_element(prs: &Presentation) -> String {
    let bytes = prs.to_bytes().expect("deck saves");
    let package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    let xml = String::from_utf8(package.get_part("/ppt/presentation.xml").unwrap().to_vec())
        .expect("presentation part is UTF-8");
    let start = xml.find("<p:sldSz").expect("deck has a slide size");
    let end = start + xml[start..].find("/>").unwrap() + 2;
    xml[start..end].to_owned()
}

/// Every section as `(name, slide ids)`.
fn sections(prs: &Presentation) -> Vec<(String, Vec<u32>)> {
    prs.sections()
        .iter()
        .map(|section| {
            (
                section.name.clone().unwrap_or_default(),
                section.slide_ids.clone(),
            )
        })
        .collect()
}

/// A deck of four slides in two sections: A holds the first two, B the
/// last two.
fn deck_in_two_sections() -> Presentation {
    let mut prs = deck_with_titles(&["A1", "A2", "B1", "B2"]);
    let ids = slide_ids(&prs);
    prs.set_sections(vec![
        Section::new(SECTION_A, "A", ids[..2].to_vec()).unwrap(),
        Section::new(SECTION_B, "B", ids[2..].to_vec()).unwrap(),
    ])
    .unwrap();
    prs
}

#[test]
fn a_new_deck_has_no_slide_and_offers_the_eleven_office_layouts() {
    let prs = new_deck();
    assert!(prs.is_empty());
    assert_eq!(prs.len(), 0);
    assert_eq!(prs.slides().len(), 0);
    assert!(prs.slide(0).is_none());
    assert_eq!(prs.master_count(), 1);
    let names: Vec<_> = (0..prs.layout_count())
        .map(|index| prs.layout_name(index).unwrap())
        .collect();
    assert_eq!(names, OFFICE_LAYOUTS);
    assert_eq!(
        prs.master_layout_indices(0).unwrap(),
        (0..11).collect::<Vec<_>>()
    );
    assert_eq!(prs.slide_size(), Some((Emu(12_192_000), Emu(6_858_000))));
}

#[test]
fn the_placeholders_of_a_new_slide_take_text_that_reads_back_after_a_save() {
    let mut prs = new_deck();
    prs.add_slide(layout_named(&prs, "Title Slide")).unwrap();
    prs.add_slide(layout_named(&prs, "Title and Content"))
        .unwrap();
    let fresh = prs.slide(1).unwrap();
    assert_eq!(fresh.title().unwrap().text().as_deref(), Some(""));
    assert_eq!(fresh.placeholder(1).unwrap().text().as_deref(), Some(""));

    let mut cover = prs.slide_mut(0).unwrap();
    cover
        .shape_mut(0)
        .unwrap()
        .set_text("Rapport annuel 2026")
        .unwrap();
    cover
        .shape_mut(1)
        .unwrap()
        .set_text("年度报告 · التقرير السنوي")
        .unwrap();
    let mut content = prs.slide_mut(1).unwrap();
    content
        .shape_mut(0)
        .unwrap()
        .set_text("مرحبا بالعالم")
        .unwrap();
    let mut body = content.shape_mut(1).unwrap();
    body.set_text("Revenue up 13 %\nNew tier at 18 %\nChurn down")
        .unwrap();
    body.text_frame()
        .unwrap()
        .paragraph_mut(1)
        .unwrap()
        .set_level(1);

    let prs = reopen(&prs);
    let cover = prs.slide(0).unwrap();
    assert_eq!(
        cover.title().unwrap().text().as_deref(),
        Some("Rapport annuel 2026")
    );
    assert_eq!(
        cover.placeholder(1).unwrap().text().as_deref(),
        Some("年度报告 · التقرير السنوي")
    );
    let content = prs.slide(1).unwrap();
    assert_eq!(
        content.title().unwrap().text().as_deref(),
        Some("مرحبا بالعالم")
    );
    let body = content.placeholder(1).unwrap();
    assert_eq!(
        body.text().as_deref(),
        Some("Revenue up 13 %\nNew tier at 18 %\nChurn down")
    );
    let frame = body.text_frame().unwrap();
    let levels: Vec<_> = (0..frame.paragraph_count())
        .map(|index| frame.paragraph(index).unwrap().level())
        .collect();
    assert_eq!(levels, [0, 1, 0]);
}

#[test]
fn a_slide_is_added_by_layout_name_on_powerpoint_and_google_decks() {
    // PowerPoint for Mac lists the master's layouts out of order in the
    // package, so the name, not the gallery position, finds the layout.
    for (deck, name) in [
        ("powerpoint/deck.pptx", "Title Only"),
        ("google/deck.pptx", "TITLE_ONLY"),
    ] {
        let mut prs = corpus_deck(deck);
        prs.add_slide(layout_named(&prs, name)).unwrap();
        let prs = reopen(&prs);
        assert_eq!(prs.len(), 5, "{deck}");
        assert_eq!(slide_layout_names(&prs)[4], name, "{deck}");
        assert_eq!(
            slide_placeholders(prs.slide(4).unwrap()),
            [placeholder(Some("title"), 0)],
            "{deck}"
        );
    }
}

#[test]
fn out_of_range_slide_and_layout_indexes_are_refused_and_change_nothing() {
    let mut prs = deck_with_titles(&["A", "B"]);
    let before = prs.to_bytes().unwrap();
    let layouts = prs.layout_count();
    assert!(matches!(
        prs.add_slide(layouts),
        Err(Error::UnknownLayoutIndex { .. })
    ));
    assert!(matches!(
        prs.add_slide_at(layouts, 0),
        Err(Error::UnknownLayoutIndex { .. })
    ));
    assert!(matches!(
        prs.add_slide_at(0, 3),
        Err(Error::UnknownSlideIndex { .. })
    ));
    assert!(matches!(
        prs.remove_slide(2),
        Err(Error::UnknownSlideIndex { .. })
    ));
    assert!(prs.move_slide(2, 0).is_err());
    assert!(prs.move_slide(0, 2).is_err());
    assert!(prs.duplicate_slide(2).is_err());
    assert!(prs.slide(2).is_none());
    assert!(prs.slide_mut(2).is_none());
    assert_eq!(prs.to_bytes().unwrap(), before);

    let mut empty = new_deck();
    assert!(empty.remove_slide(0).is_err());
    assert!(empty.move_slide(0, 0).is_err());
    assert!(empty.duplicate_slide(0).is_err());
    assert!(empty.is_empty());
}

#[test]
fn a_link_to_a_slide_follows_it_when_slides_move_and_stops_when_it_is_removed() {
    let mut prs = deck_with_titles(&["A", "B", "C", "D"]);
    let shape = prs
        .slide(0)
        .unwrap()
        .shape(0)
        .unwrap()
        .non_visual_id()
        .unwrap();
    prs.set_shape_target_slide(0, shape, Some(2)).unwrap();
    prs.move_slide(2, 3).unwrap();
    prs.move_slide(0, 1).unwrap();
    let mut prs = reopen(&prs);
    assert_eq!(titles(&prs), ["B", "A", "D", "C"]);
    assert_eq!(prs.shape_target_slide(1, shape).unwrap(), Some(3));

    prs.remove_slide(3).unwrap();
    let prs = reopen(&prs);
    assert_eq!(prs.shape_target_slide(1, shape).unwrap(), None);
    assert!(prs.validate().is_empty(), "{:?}", prs.validate());
}

#[test]
fn a_hidden_slide_stays_hidden_after_a_save_and_in_its_duplicate() {
    let mut prs = corpus_deck("google/deck.pptx");
    prs.slide_mut(1).unwrap().set_hidden(true);
    prs.duplicate_slide(1).unwrap();
    let mut prs = reopen(&prs);
    let hidden: Vec<_> = prs.slides().map(|slide| slide.hidden()).collect();
    assert_eq!(hidden, [false, true, true, false, false]);

    prs.slide_mut(1).unwrap().set_hidden(false);
    let prs = reopen(&prs);
    let hidden: Vec<_> = prs.slides().map(|slide| slide.hidden()).collect();
    assert_eq!(hidden, [false, false, true, false, false]);
}

#[test]
fn a_slide_with_comments_can_be_duplicated() {
    let author = "{6E7B2A10-1111-4A5B-9C3D-000000000001}";
    let comment = "{6E7B2A10-2222-4A5B-9C3D-000000000002}";
    let mut prs = deck_with_titles(&["Budget"]);
    prs.add_comment_author(CommentAuthor::new(author, "Ada", Some("A"), "ada", "None").unwrap())
        .unwrap();
    prs.add_comment(
        0,
        Comment::new(comment, author, "2026-11-21T10:00:00Z", "Check the figures").unwrap(),
    )
    .unwrap();

    prs.duplicate_slide(0).unwrap();

    let prs = reopen(&prs);
    let copied = prs.comments(1).expect("the copy has the comment");
    assert_eq!(copied.len(), 1);
    assert_eq!(copied[0].text(), "Check the figures");
    assert_ne!(copied[0].id, comment, "the copy's comment has a fresh id");
    assert_eq!(prs.comments(0).unwrap()[0].id, comment);
}

#[test]
fn a_google_slide_imports_onto_a_layout_given_by_index_when_no_name_matches() {
    let source = corpus_deck("google/deck.pptx");
    let mut prs = new_deck();
    let before = prs.to_bytes().unwrap();
    let error = prs.import_slide(&source, 1, None, None).err().unwrap();
    assert!(
        error.to_string().contains("no layout named \"OBJECT\""),
        "{error}"
    );
    assert_eq!(prs.to_bytes().unwrap(), before);

    let layout = layout_named(&prs, "Title and Content");
    prs.import_slide(&source, 1, Some(layout), None).unwrap();
    let prs = reopen(&prs);
    assert_eq!(slide_layout_names(&prs), ["Title and Content"]);
    assert_eq!(
        prs.slide(0).unwrap().text(),
        source.slide(1).unwrap().text()
    );
    assert_eq!(
        prs.slide(0).unwrap().notes_text().as_deref(),
        Some(HIGHLIGHTS_NOTES)
    );
    assert!(prs.validate().is_empty(), "{:?}", prs.validate());
}

#[test]
fn slides_imported_together_land_in_order_and_keep_the_links_between_them() {
    let mut source = deck_with_titles(&["S0", "S1", "S2"]);
    let shape = source
        .slide(0)
        .unwrap()
        .shape(0)
        .unwrap()
        .non_visual_id()
        .unwrap();
    source.set_shape_target_slide(0, shape, Some(2)).unwrap();

    let mut prs = deck_with_titles(&["D0", "D1"]);
    let options = ImportSlideOptions {
        insert_index: Some(1),
        ..ImportSlideOptions::default()
    };
    let (inserted, diagnostics) = prs.import_slides(&source, &[2, 0], &options, &[]).unwrap();
    assert_eq!(inserted, [1, 2]);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let prs = reopen(&prs);
    assert_eq!(titles(&prs), ["D0", "S2", "S0", "D1"]);
    let copied = prs
        .slide(2)
        .unwrap()
        .shape(0)
        .unwrap()
        .non_visual_id()
        .unwrap();
    assert_eq!(prs.shape_target_slide(2, copied).unwrap(), Some(1));

    // Imported alone, the slide loses the jump to a slide left behind.
    let mut alone = new_deck();
    let (_, diagnostics) = alone
        .import_slides(&source, &[0], &ImportSlideOptions::default(), &[])
        .unwrap();
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    let copied = alone
        .slide(0)
        .unwrap()
        .shape(0)
        .unwrap()
        .non_visual_id()
        .unwrap();
    assert_eq!(alone.shape_target_slide(0, copied).unwrap(), None);
    assert!(reopen(&alone).validate().is_empty());
}

#[test]
fn keeping_source_formatting_brings_the_source_layout_and_master_once() {
    let source = corpus_deck("powerpoint/deck.pptx");
    let mut prs = new_deck();
    let options = ImportSlideOptions {
        keep_source_formatting: true,
        ..ImportSlideOptions::default()
    };
    prs.import_slides(&source, &[2, 3], &options, &[]).unwrap();

    let prs = reopen(&prs);
    assert_eq!(prs.master_count(), 2);
    assert_eq!(
        prs.layout_count(),
        12,
        "both slides share one copied layout"
    );
    let layout = prs.slide_layout_index(0).unwrap();
    assert_eq!(prs.slide_layout_index(1), Some(layout));
    assert_eq!(prs.layout_name(layout), Some("Title Only"));
    assert_eq!(prs.layout_master_index(layout).unwrap(), 1);
    assert_eq!(prs.master_layout_indices(1).unwrap(), [layout]);
    assert_eq!(titles(&prs), CORPUS_TITLES[2..]);
    assert!(prs.validate().is_empty(), "{:?}", prs.validate());
}

#[test]
fn a_notes_master_extension_other_than_its_creation_id_still_stops_a_source_formatted_import() {
    let source = corpus_deck("powerpoint/deck.pptx");
    let mut package = OpcPackage::from_reader(Cursor::new(source.to_bytes().unwrap())).unwrap();
    let master = "/ppt/notesMasters/notesMaster1.xml";
    let xml = String::from_utf8(package.get_part(master).unwrap().to_vec()).unwrap();
    let extended = xml.replacen(
        "</p:extLst></p:cSld>",
        r#"<p:ext uri="{00000000-0000-0000-0000-00000000C0DE}"><x:unknown xmlns:x="urn:example:unknown"/></p:ext></p:extLst></p:cSld>"#,
        1,
    );
    assert_ne!(
        extended, xml,
        "the notes master has a common extension list"
    );
    package.set_part(master, extended.into_bytes());
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    let source = Presentation::from_bytes(&bytes.into_inner()).unwrap();
    let mut prs = new_deck();
    let before = prs.to_bytes().unwrap();

    let refused = prs
        .import_slides(
            &source,
            &[1],
            &ImportSlideOptions {
                keep_source_formatting: true,
                ..ImportSlideOptions::default()
            },
            &[],
        )
        .unwrap_err();

    assert!(
        refused.to_string().contains("common extension authority"),
        "{refused}"
    );
    assert_eq!(prs.to_bytes().unwrap(), before);
}

/// A corpus deck with one part rewritten by `edit`, reopened.
fn corpus_deck_with(name: &str, part: &str, edit: impl FnOnce(&str) -> String) -> Presentation {
    let source = corpus_deck(name);
    let mut package = OpcPackage::from_reader(Cursor::new(source.to_bytes().unwrap())).unwrap();
    let xml = String::from_utf8(package.get_part(part).unwrap().to_vec()).unwrap();
    let edited = edit(&xml);
    assert_ne!(edited, xml, "{part} changed");
    package.set_part(part, edited.into_bytes());
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    Presentation::from_bytes(&bytes.into_inner()).unwrap()
}

fn import_with_source_formatting(source: &Presentation) -> (Presentation, Result<(), String>) {
    let mut prs = new_deck();
    let result = prs
        .import_slides(
            source,
            &[1],
            &ImportSlideOptions {
                keep_source_formatting: true,
                ..ImportSlideOptions::default()
            },
            &[],
        )
        .map(|_| ())
        .map_err(|error| error.to_string());
    (prs, result)
}

#[test]
fn an_empty_latin_theme_face_still_stops_a_source_formatted_import_with_notes() {
    let source = corpus_deck_with("powerpoint/deck.pptx", "/ppt/theme/theme2.xml", |xml| {
        let minor = xml.find("<a:minorFont>").unwrap();
        let latin = minor + xml[minor..].find("<a:latin typeface=\"Aptos\"").unwrap();
        format!(
            "{}{}",
            &xml[..latin],
            xml[latin..].replacen("<a:latin typeface=\"Aptos\"", "<a:latin typeface=\"\"", 1)
        )
    });
    let (prs, result) = import_with_source_formatting(&source);
    let refused = result.unwrap_err();
    assert!(refused.contains("script-font inheritance"), "{refused}");
    assert_eq!(prs.len(), 0);
}

#[test]
fn an_explicit_east_asian_theme_face_is_set_on_imported_notes() {
    let source = corpus_deck_with("powerpoint/deck.pptx", "/ppt/theme/theme2.xml", |xml| {
        let minor = xml.find("<a:minorFont>").unwrap();
        let ea = minor + xml[minor..].find("<a:ea typeface=\"\"").unwrap();
        format!(
            "{}{}",
            &xml[..ea],
            xml[ea..].replacen("<a:ea typeface=\"\"", "<a:ea typeface=\"Yu Gothic\"", 1)
        )
    });
    let (prs, result) = import_with_source_formatting(&source);
    result.unwrap();
    let prs = reopen(&prs);
    let bytes = prs.to_bytes().unwrap();
    let package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    let (_, notes) = package
        .parts
        .iter()
        .find(|(name, _)| name.starts_with("/ppt/notesSlides/"))
        .expect("the import has notes");
    let xml = String::from_utf8_lossy(notes);
    assert!(xml.contains(r#"typeface="Yu Gothic""#), "{xml}");
    assert!(!xml.contains(r#"typeface="+mn-ea""#), "{xml}");
    assert!(xml.contains(r#"typeface="+mn-cs""#), "{xml}");
}

#[test]
fn a_notes_tree_group_that_differs_beyond_its_id_and_name_still_stops_the_import() {
    let source = corpus_deck_with(
        "google/deck.pptx",
        "/ppt/notesMasters/notesMaster1.xml",
        |xml| {
            xml.replacen(
                r#"<p:cNvPr id="2" name="Shape 2"/>"#,
                r#"<p:cNvPr id="2" name="Shape 2" hidden="1"/>"#,
                1,
            )
        },
    );
    let (prs, result) = import_with_source_formatting(&source);
    let refused = result.unwrap_err();
    assert!(refused.contains("tree group authority"), "{refused}");
    assert_eq!(prs.len(), 0);
}

#[test]
fn an_imported_google_notes_page_keeps_its_source_look_and_validates() {
    let (prs, result) = import_with_source_formatting(&corpus_deck("google/deck.pptx"));
    result.unwrap();
    let prs = reopen(&prs);
    assert!(prs.validate().is_empty(), "{:?}", prs.validate());
    let bytes = prs.to_bytes().unwrap();
    let package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    let (_, notes) = package
        .parts
        .iter()
        .find(|(name, _)| name.starts_with("/ppt/notesSlides/"))
        .expect("the import has notes");
    let xml = String::from_utf8_lossy(notes);
    // Google's notes theme names Arial everywhere: set on the page, no token
    // left to resolve against this deck's theme, and the colours set too.
    assert!(xml.contains(r#"typeface="Arial""#), "{xml}");
    assert!(!xml.contains(r#"typeface="+m"#), "{xml}");
    assert!(!xml.contains("<a:schemeClr"), "{xml}");
    assert_eq!(
        prs.slide(0).unwrap().notes_text().as_deref(),
        Some(HIGHLIGHTS_NOTES)
    );
}

#[test]
fn the_slide_size_takes_four_by_three_widescreen_or_custom_and_refuses_nonsense() {
    let mut prs = new_deck();
    for (width, height) in [
        (9_144_000, 6_858_000),  // 4:3
        (7_772_400, 10_058_400), // US Letter, portrait
        (12_192_000, 6_858_000), // 16:9
    ] {
        prs.set_slide_size(Emu(width), Emu(height)).unwrap();
        prs = reopen(&prs);
        assert_eq!(prs.slide_size(), Some((Emu(width), Emu(height))));
    }

    let before = prs.to_bytes().unwrap();
    assert!(prs.set_slide_size(Emu(0), Emu(6_858_000)).is_err());
    assert!(prs.set_slide_size(Emu(9_144_000), Emu(-1)).is_err());
    assert_eq!(prs.to_bytes().unwrap(), before);
}

#[test]
fn a_deck_resized_to_four_by_three_is_not_saved_as_widescreen() {
    let mut prs = new_deck();
    assert!(saved_slide_size_element(&prs).contains("type=\"screen16x9\""));
    prs.set_slide_size(Emu(9_144_000), Emu(6_858_000)).unwrap();
    let size = saved_slide_size_element(&prs);
    assert!(
        !size.contains("type=") || size.contains("type=\"screen4x3\""),
        "a 4:3 deck is saved as {size}"
    );
}

#[test]
fn changing_the_layout_of_a_slide_keeps_its_text() {
    let mut prs = corpus_deck("powerpoint/deck.pptx");
    let text = prs.slide(1).unwrap().text();
    let title_only = layout_named(&prs, "Title Only");
    prs.set_slide_layout(1, title_only).unwrap();

    let prs = reopen(&prs);
    assert_eq!(
        slide_layout_names(&prs),
        ["Title Slide", "Title Only", "Title Only", "Title Only"]
    );
    assert_eq!(prs.slide(1).unwrap().text(), text);
    let title_only = layout_named(&prs, "Title Only");
    assert_eq!(prs.layout_used_by_slides(title_only).unwrap(), [1, 2, 3]);
    let content = layout_named(&prs, "Title and Content");
    assert!(prs.layout_used_by_slides(content).unwrap().is_empty());
    assert!(prs.validate().is_empty(), "{:?}", prs.validate());
}

#[test]
fn a_duplicated_layout_can_be_renamed_and_chosen_by_its_new_name_after_a_save() {
    let name = "Agenda · جدول الأعمال · 议程";
    let mut prs = new_deck();
    let copy = prs
        .duplicate_layout(layout_named(&prs, "Title and Content"))
        .unwrap();
    prs.set_layout_name(copy, Some(name)).unwrap();
    prs.add_slide(copy).unwrap();

    let prs = reopen(&prs);
    assert_eq!(prs.layout_count(), 12);
    let copy = layout_named(&prs, name);
    assert_eq!(prs.layout_master_index(copy).unwrap(), 0);
    assert!(prs.master_layout_indices(0).unwrap().contains(&copy));
    assert_eq!(slide_layout_names(&prs), [name]);
    assert_eq!(
        layout_placeholders(&prs, copy),
        layout_placeholders(&prs, layout_named(&prs, "Title and Content"))
    );
    assert!(prs.validate().is_empty(), "{:?}", prs.validate());
}

#[test]
fn only_an_unused_layout_can_be_removed_and_slides_keep_their_layouts() {
    let mut prs = corpus_deck("powerpoint/deck.pptx");
    let before = prs.to_bytes().unwrap();
    assert!(
        prs.remove_layout(layout_named(&prs, "Title Slide"))
            .is_err()
    );
    assert_eq!(prs.to_bytes().unwrap(), before);

    prs.remove_layout(layout_named(&prs, "Blank")).unwrap();
    let prs = reopen(&prs);
    assert_eq!(prs.layout_count(), 10);
    assert!((0..10).all(|index| prs.layout_name(index) != Some("Blank")));
    assert_eq!(
        slide_layout_names(&prs),
        [
            "Title Slide",
            "Title and Content",
            "Title Only",
            "Title Only"
        ]
    );
    assert!(prs.validate().is_empty(), "{:?}", prs.validate());
}

#[test]
fn a_slide_background_survives_a_save_and_can_be_removed() {
    let fill = Fill::from_xml(br#"<a:solidFill><a:srgbClr val="1F4E79"/></a:solidFill>"#).unwrap();
    let mut prs = deck_with_titles(&["Dark", "Plain"]);
    prs.slide_mut(0)
        .unwrap()
        .set_background(fill.clone())
        .unwrap();

    let mut prs = reopen(&prs);
    let (dark, plain) = (prs.slide(0).unwrap(), prs.slide(1).unwrap());
    assert!(dark.has_explicit_background());
    assert_eq!(dark.background_fill(), Some(&fill));
    assert!(!plain.has_explicit_background());
    assert_eq!(plain.background_fill(), None);

    prs.slide_mut(0).unwrap().remove_background();
    let prs = reopen(&prs);
    assert!(!prs.slide(0).unwrap().has_explicit_background());
    assert_eq!(prs.slide(0).unwrap().background_fill(), None);
}

#[test]
fn sections_keep_their_names_and_slides_after_a_save_and_lose_a_removed_slide() {
    let mut prs = deck_with_titles(&["Intro", "Data 1", "Data 2"]);
    let ids = slide_ids(&prs);
    let data = "数据 · البيانات";
    prs.set_sections(vec![
        Section::new(SECTION_A, "Introduction", vec![ids[0]]).unwrap(),
        Section::new(SECTION_B, data, ids[1..].to_vec()).unwrap(),
    ])
    .unwrap();
    let mut prs = reopen(&prs);
    assert_eq!(
        sections(&prs),
        [
            ("Introduction".to_owned(), vec![ids[0]]),
            (data.to_owned(), ids[1..].to_vec()),
        ]
    );

    prs.remove_slide(1).unwrap();
    let mut prs = reopen(&prs);
    assert_eq!(sections(&prs)[1], (data.to_owned(), vec![ids[2]]));

    prs.set_sections(Vec::new()).unwrap();
    assert!(reopen(&prs).sections().is_empty());
}

#[test]
fn a_section_naming_an_unknown_slide_or_a_slide_twice_is_refused() {
    assert!(Section::new("not a GUID", "A", Vec::new()).is_err());
    let mut prs = deck_with_titles(&["A", "B"]);
    let ids = slide_ids(&prs);
    let before = prs.to_bytes().unwrap();
    let unknown = vec![Section::new(SECTION_A, "A", vec![ids[0], 999]).unwrap()];
    assert!(prs.set_sections(unknown).is_err());
    let twice = vec![
        Section::new(SECTION_A, "A", vec![ids[0]]).unwrap(),
        Section::new(SECTION_B, "B", vec![ids[0], ids[1]]).unwrap(),
    ];
    assert!(prs.set_sections(twice).is_err());
    assert_eq!(prs.to_bytes().unwrap(), before);
    assert!(prs.sections().is_empty());
}

#[test]
fn an_entity_the_section_list_keeps_as_written_survives_a_section_edit() {
    // Character data the section list, a section, its slide list and one
    // of its slide ids keep as written, with an entity in each.
    let bytes = deck_in_two_sections().to_bytes().unwrap();
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    let part = "/ppt/presentation.xml";
    let mut xml = String::from_utf8(package.get_part(part).unwrap().to_vec()).unwrap();
    let slide_id = xml.find("<p14:sldId ").unwrap();
    let close = slide_id + xml[slide_id..].find("/>").unwrap();
    xml.replace_range(close..close + 2, ">i&amp;s</p14:sldId>");
    let xml = xml
        .replacen("<p14:sldIdLst>", "s&amp;e<p14:sldIdLst>", 1)
        .replacen("<p14:sldIdLst>", "<p14:sldIdLst>l&amp;s", 1)
        .replacen("<p14:section ", "l&amp;t<p14:section ", 1);
    package.set_part(part, xml.into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let mut prs = Presentation::from_bytes(&out.into_inner()).unwrap();

    prs.duplicate_slide(0).unwrap();
    let xml = saved_part_text(&prs, part).unwrap();
    assert_eq!(sections(&reopen(&prs))[0].1.len(), 3, "{xml}");
    for kept in [
        "i&amp;s</p14:sldId>",
        "s&amp;e<p14:sldIdLst>",
        "<p14:sldIdLst>l&amp;s",
        "l&amp;t<p14:section ",
    ] {
        assert!(xml.contains(kept), "{kept}: {xml}");
    }

    // Clearing the sections would drop what the list itself holds as
    // written: it is refused, as it is for plain text there.
    for direct in ["&amp;", "lt"] {
        let bytes = deck_in_two_sections().to_bytes().unwrap();
        let mut package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
        let xml = String::from_utf8(package.get_part(part).unwrap().to_vec()).unwrap();
        let xml = xml.replacen("<p14:section ", &format!("{direct}<p14:section "), 1);
        package.set_part(part, xml.into_bytes());
        let mut out = Cursor::new(Vec::new());
        package.write_to(&mut out).unwrap();
        let mut prs = Presentation::from_bytes(&out.into_inner()).unwrap();
        assert!(prs.set_sections(Vec::new()).is_err(), "{direct}");
    }
}

#[test]
fn an_entity_a_master_header_footer_keeps_as_written_survives_new_flags() {
    let bytes = deck_with_titles(&["One"]).to_bytes().unwrap();
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    let part = "/ppt/slideMasters/slideMaster1.xml";
    let xml = String::from_utf8(package.get_part(part).unwrap().to_vec()).unwrap();
    let xml = xml.replacen(
        "<p:txStyles>",
        "<p:hf hdr=\"0\">h&amp;f</p:hf><p:txStyles>",
        1,
    );
    package.set_part(part, xml.into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let mut prs = Presentation::from_bytes(&out.into_inner()).unwrap();

    let settings = HeaderFooter {
        slide_number: true,
        footer: None,
        date: HeaderFooterDate::Off,
    };
    assert!(prs.set_header_footer(&settings, false).unwrap());
    let xml = saved_part_text(&prs, part).unwrap();
    let hf = &xml[xml.find("<p:hf").unwrap()..];
    let hf = &hf[..hf.find("</p:hf>").unwrap_or(hf.len())];
    assert!(hf.contains("h&amp;f"), "{hf}");
}

#[test]
fn a_slide_moved_into_another_section_joins_it() {
    let mut prs = deck_in_two_sections();
    let ids = slide_ids(&prs);
    // A1 lands between B1 and B2.
    prs.move_slide(0, 2).unwrap();
    let prs = reopen(&prs);
    assert_eq!(titles(&prs), ["A2", "B1", "A1", "B2"]);
    assert_eq!(
        sections(&prs),
        [
            ("A".to_owned(), vec![ids[1]]),
            ("B".to_owned(), vec![ids[2], ids[0], ids[3]]),
        ],
        "the moved slide stays in its old section"
    );
}

#[test]
fn layout_section_and_theme_names_keep_their_tabs_and_line_feeds() {
    for name in [
        "powerpoint/deck.pptx",
        "google/deck.pptx",
        "libreoffice/deck.pptx",
        "authored/deck.pptx",
    ] {
        let mut prs = corpus_deck(name);
        let ids: Vec<u32> = prs.slides().map(|slide| slide.id()).collect();

        prs.set_layout_name(0, Some("Title\tand\nbody")).unwrap();
        prs.set_theme_name(0, "Brand\ttheme\n2026").unwrap();
        prs.set_sections(vec![
            Section::new(SECTION_A, "Part\tone\r\nA", ids[..2].to_vec()).unwrap(),
            Section::new(SECTION_B, "B", ids[2..].to_vec()).unwrap(),
        ])
        .unwrap();

        let prs = reopen(&prs);
        assert_eq!(prs.layout_name(0), Some("Title\tand\nbody"), "{name}");
        assert_eq!(
            prs.theme(0).unwrap().unwrap().name.as_deref(),
            Some("Brand\ttheme\n2026"),
            "{name}"
        );
        assert_eq!(
            prs.sections()[0].name.as_deref(),
            Some("Part\tone\r\nA"),
            "{name}"
        );
    }
}

/// The text of one saved part, if the saved package has it.
fn saved_part_text(prs: &Presentation, part_name: &str) -> Option<String> {
    let bytes = prs.to_bytes().expect("deck saves");
    let package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    package
        .get_part(part_name)
        .map(|part| String::from_utf8_lossy(part).into_owned())
}

#[test]
fn an_app_xml_outside_its_namespace_does_not_stop_a_removal() {
    let prs = corpus_deck("libreoffice/deck.pptx");
    let mut package = OpcPackage::from_reader(Cursor::new(prs.to_bytes().unwrap())).unwrap();
    let app = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties><Slides>4</Slides><Application>Example</Application></Properties>"#;
    package.set_part("/docProps/app.xml", app.to_vec());
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    let mut prs = Presentation::from_bytes(&bytes.into_inner()).unwrap();

    prs.remove_slide(1).unwrap();

    let prs = reopen(&prs);
    assert_eq!(prs.len(), 3);
    assert_eq!(
        saved_part_text(&prs, "/docProps/app.xml").as_deref(),
        Some(std::str::from_utf8(app).unwrap())
    );
}

#[test]
fn removing_a_slide_counts_what_is_left_and_adds_no_metadata() {
    // The authored deck says it has no slide and no notes; its counts follow
    // the deck, whose Highlights slide keeps its notes text.
    let mut prs = corpus_deck("authored/deck.pptx");
    prs.remove_slide(3).unwrap();
    let app = saved_part_text(&reopen(&prs), "/docProps/app.xml").unwrap();
    assert!(app.contains("<Slides>3</Slides>"), "{app}");
    assert!(app.contains("<Notes>1</Notes>"), "{app}");

    // Google writes no app.xml, LibreOffice one without slide counts or
    // titles, whose zero text counts go as any stale text count does.
    for name in ["google/deck.pptx", "libreoffice/deck.pptx"] {
        let mut prs = corpus_deck(name);
        let before = saved_part_text(&prs, "/docProps/app.xml").map(|app| {
            app.replace("<Words>0</Words>", "")
                .replace("<Paragraphs>0</Paragraphs>", "")
        });
        prs.remove_slide(1).unwrap();
        let prs = reopen(&prs);
        assert_eq!(saved_part_text(&prs, "/docProps/app.xml"), before, "{name}");
        assert!(prs.validate().is_empty(), "{name}: {:?}", prs.validate());
    }
}

#[test]
fn added_duplicated_and_imported_slides_are_counted_in_the_package_metadata() {
    // Google's Highlights slide, hidden, with its notes.
    let mut source = corpus_deck("google/deck.pptx");
    source.slide_mut(1).unwrap().set_hidden(true);
    type Edit = fn(&mut Presentation, &Presentation);
    let edits: [(&str, Edit); 4] = [
        ("add", |prs, _| {
            let layout = layout_named(prs, "Title Only");
            prs.add_slide(layout).unwrap();
        }),
        ("add at", |prs, _| {
            let layout = layout_named(prs, "Title Only");
            prs.add_slide_at(layout, 0).unwrap();
        }),
        ("duplicate", |prs, _| {
            prs.duplicate_slide(1).unwrap();
        }),
        ("import", |prs, source| {
            let layout = layout_named(prs, "Title Only");
            prs.import_slide(source, 1, Some(layout), None).unwrap();
        }),
    ];
    for (name, edit) in edits {
        let mut prs = corpus_deck("powerpoint/deck.pptx");
        edit(&mut prs, &source);

        let prs = reopen(&prs);
        let app = saved_part_text(&prs, "/docProps/app.xml").unwrap();
        assert!(app.contains("<Slides>5</Slides>"), "{name}: {app}");
        // The duplicate of Highlights and the imported one bring their
        // notes along; the imported one is hidden.
        let notes = if matches!(name, "duplicate" | "import") {
            2
        } else {
            1
        };
        assert!(
            app.contains(&format!("<Notes>{notes}</Notes>")),
            "{name}: {app}"
        );
        let hidden = usize::from(name == "import");
        assert!(
            app.contains(&format!("<HiddenSlides>{hidden}</HiddenSlides>")),
            "{name}: {app}"
        );
        assert!(
            !app.contains("TitlesOfParts") && !app.contains("HeadingPairs"),
            "{name}: {app}"
        );
        assert!(
            saved_part_text(&prs, "/docProps/thumbnail.jpeg").is_some(),
            "{name}: a new slide leaves the thumbnail"
        );
        assert!(prs.validate().is_empty(), "{name}: {:?}", prs.validate());
    }
}

#[test]
fn the_notes_count_follows_notes_set_cleared_and_created_and_the_text_counts_stay() {
    // The PowerPoint deck has notes text on one slide of four.
    let mut prs = corpus_deck("powerpoint/deck.pptx");
    let has = |prs: &Presentation, index: usize| prs.slide(index).unwrap().notes_text();
    let with = (0..prs.len())
        .find(|i| has(&prs, *i).is_some_and(|text| !text.trim().is_empty()))
        .unwrap();
    let without = (0..prs.len()).find(|i| has(&prs, *i).is_none()).unwrap();
    for (what, edit, notes) in [
        ("cleared", (with, ""), 0),
        ("set again", (with, "Say hello"), 1),
        ("created", (without, "New notes"), 2),
    ] {
        prs.set_notes_text(edit.0, edit.1).unwrap();
        prs = reopen(&prs);
        let app = saved_part_text(&prs, "/docProps/app.xml").unwrap();
        assert!(
            app.contains(&format!("<Notes>{notes}</Notes>")),
            "{what}: {app}"
        );
        // Notes text changes no slide text: the title lists and text counts stay.
        assert!(
            app.contains("<TitlesOfParts>") && app.contains("<Words>61</Words>"),
            "{what}: {app}"
        );
    }
}

#[test]
fn hiding_a_slide_recounts_hidden_slides_only_and_a_no_op_changes_nothing() {
    // The authored deck says it has no slide and no notes; it is also told
    // two slides are hidden, though none is.
    let mut prs = corpus_deck("authored/deck.pptx");
    let mut properties = prs.application_properties().unwrap().clone();
    properties.hidden_slides = Some(2);
    prs.set_application_properties(properties).unwrap();
    let before = saved_part_text(&prs, "/docProps/app.xml").unwrap();
    assert!(
        before.contains("<HiddenSlides>2</HiddenSlides>"),
        "{before}"
    );
    let bytes = prs.to_bytes().unwrap();

    prs.slide_mut(0).unwrap().set_hidden(false);
    assert_eq!(
        prs.to_bytes().unwrap(),
        bytes,
        "a no-op show changes nothing"
    );

    prs.slide_mut(0).unwrap().set_hidden(true);
    let app = saved_part_text(&reopen(&prs), "/docProps/app.xml").unwrap();
    assert_eq!(
        app,
        before.replace(
            "<HiddenSlides>2</HiddenSlides>",
            "<HiddenSlides>1</HiddenSlides>"
        )
    );
    assert!(
        app.contains("<Slides>0</Slides>") && app.contains("<Notes>0</Notes>"),
        "{app}"
    );
}

#[test]
fn slide_edits_on_decks_without_sections_add_none() {
    for name in [
        "powerpoint/deck.pptx",
        "google/deck.pptx",
        "libreoffice/deck.pptx",
    ] {
        let mut prs = corpus_deck(name);
        let layout = prs.slide_layout_index(0).unwrap();
        prs.duplicate_slide(1).unwrap();
        prs.add_slide_at(layout, 0).unwrap();
        prs.move_slide(0, 3).unwrap();

        let prs = reopen(&prs);
        assert!(prs.sections().is_empty(), "{name}");
        let bytes = prs.to_bytes().unwrap();
        let package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
        let xml = String::from_utf8_lossy(package.get_part("/ppt/presentation.xml").unwrap());
        assert!(!xml.contains("sectionLst"), "{name}");
    }
}

/// Every slide sits in exactly one section, and the sections, read in
/// order, list the slides in deck order.
fn assert_sections_follow_slides(prs: &Presentation) {
    let listed: Vec<u32> = prs
        .sections()
        .iter()
        .flat_map(|section| section.slide_ids.clone())
        .collect();
    assert_eq!(listed, slide_ids(prs), "{:?}", sections(prs));
}

#[test]
fn slides_added_to_a_deck_whose_sections_are_empty_join_the_first_one() {
    let mut prs = deck_in_two_sections();
    for _ in 0..4 {
        prs.remove_slide(0).unwrap();
    }
    assert_eq!(
        sections(&prs),
        [("A".to_owned(), vec![]), ("B".to_owned(), vec![])]
    );
    let layout = layout_named(&prs, "Title Only");

    prs.add_slide(layout).unwrap();
    prs.add_slide(layout).unwrap();

    let prs = reopen(&prs);
    let ids = slide_ids(&prs);
    assert_eq!(
        sections(&prs),
        [("A".to_owned(), ids.clone()), ("B".to_owned(), vec![])]
    );
}

#[test]
fn slides_imported_with_their_sections_keep_every_slide_in_one_section() {
    let source = deck_in_two_sections();
    let mut prs = reopen(&deck_in_two_sections());

    prs.import_slides(
        &source,
        &[1, 2],
        &ImportSlideOptions {
            insert_index: Some(1),
            keep_sections: true,
            ..ImportSlideOptions::default()
        },
        &[],
    )
    .unwrap();

    let prs = reopen(&prs);
    assert_eq!(titles(&prs), ["A1", "A2", "B1", "A2", "B1", "B2"]);
    assert_sections_follow_slides(&prs);
    assert!(prs.validate().is_empty(), "{:?}", prs.validate());
}

#[test]
fn section_entry_data_that_leans_on_its_old_section_is_left_behind_on_a_move() {
    let prs = deck_in_two_sections();
    let ids = slide_ids(&prs);
    let mut package = OpcPackage::from_reader(Cursor::new(prs.to_bytes().unwrap())).unwrap();
    let xml = String::from_utf8(package.get_part("/ppt/presentation.xml").unwrap().to_vec())
        .unwrap()
        .replacen(r#"name="A""#, r#"name="A" xmlns:x="urn:example:extra""#, 1)
        .replacen(
            &format!(r#"<p14:sldId id="{}"/>"#, ids[0]),
            &format!(r#"<p14:sldId id="{}" x:keep="1"/>"#, ids[0]),
            1,
        );
    assert!(xml.contains("x:keep"), "the section entry was edited");
    package.set_part("/ppt/presentation.xml", xml.into_bytes());
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    let mut prs = Presentation::from_bytes(&bytes.into_inner()).unwrap();

    prs.move_slide(0, 3).unwrap();

    // The saved part stays namespace-well-formed: it reopens.
    let prs = reopen(&prs);
    assert_eq!(sections(&prs)[1].1.last(), Some(&ids[0]));
    let bytes = prs.to_bytes().unwrap();
    let package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    let xml = String::from_utf8_lossy(package.get_part("/ppt/presentation.xml").unwrap());
    assert!(!xml.contains("x:keep"), "{xml}");
}

#[test]
fn setting_core_properties_keeps_a_bad_character_the_file_held_and_refuses_a_new_one() {
    // Another producer wrote a character reference XML 1.0 forbids. The
    // core part is patched field by field, so the untouched field keeps the
    // file's bytes; a new value with such a character is refused by name.
    let mut prs = Presentation::from_bytes(&saved_with_reference()).unwrap();
    let mut core = prs.core_properties().cloned().expect("core properties");
    core.subject = Some("bell\u{7}".to_owned());
    let message = prs
        .set_core_properties(core.clone())
        .expect_err("a new bad value is refused")
        .to_string();
    assert!(
        message.contains("core property subject") && message.contains("U+0007"),
        "{message}"
    );
    core.subject = None;
    core.title = Some("Q3".to_owned());
    prs.set_core_properties(core)
        .expect("the untouched field is the file's");
    let saved = prs.to_bytes().expect("it saves");
    let package = OpcPackage::from_reader(Cursor::new(saved.clone())).unwrap();
    let xml = String::from_utf8_lossy(package.get_part("/docProps/core.xml").unwrap());
    assert!(xml.contains("J&#11;Doe") && xml.contains(">Q3<"), "{xml}");
    let mut prs = Presentation::from_bytes(&saved).unwrap();
    let core = prs.core_properties().unwrap();
    assert_eq!(core.last_modified_by.as_deref(), Some("J\u{b}Doe"));
    // The mutable model goes through the same patch at the save.
    prs.core_properties_mut().category = Some("Plans".to_owned());
    let saved = prs.to_bytes().expect("it saves after a mutable edit");
    let package = OpcPackage::from_reader(Cursor::new(saved)).unwrap();
    let xml = String::from_utf8_lossy(package.get_part("/docProps/core.xml").unwrap());
    assert!(
        xml.contains("J&#11;Doe") && xml.contains(">Plans<"),
        "{xml}"
    );
    // A value the patch cannot express exactly is written whole, as before,
    // and the character the file held goes.
    prs.core_properties_mut().title = Some(String::new());
    let prs = Presentation::from_bytes(&prs.to_bytes().expect("an empty value saves")).unwrap();
    let core = prs.core_properties().unwrap();
    assert_eq!(core.category.as_deref(), Some("Plans"));
    assert_eq!(core.last_modified_by.as_deref(), Some("JDoe"));
    // So is a part holding a duplicate element.
    let mut package = OpcPackage::from_reader(Cursor::new(saved_with_reference())).unwrap();
    let xml = String::from_utf8(package.get_part("/docProps/core.xml").unwrap().to_vec()).unwrap();
    let xml = xml.replace(
        "<dc:title>Deck</dc:title>",
        "<dc:title>Deck</dc:title><dc:creator>A</dc:creator><dc:creator>B</dc:creator>",
    );
    package.set_part("/docProps/core.xml", xml.into_bytes());
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    let mut prs = Presentation::from_bytes(&bytes.into_inner()).unwrap();
    prs.core_properties_mut().creator = Some("C".to_owned());
    let prs = Presentation::from_bytes(&prs.to_bytes().expect("a duplicate saves")).unwrap();
    assert_eq!(prs.core_properties().unwrap().creator.as_deref(), Some("C"));
    // A bad value set through the mutable model is refused by name by the
    // next set_core_properties, which compares with the part.
    let mut prs = Presentation::from_bytes(&saved_with_reference()).unwrap();
    prs.core_properties_mut().title = Some("a\u{7}".to_owned());
    let mut core = prs.core_properties().cloned().unwrap();
    core.category = Some("Plans".to_owned());
    let message = prs
        .set_core_properties(core)
        .expect_err("refused")
        .to_string();
    assert!(message.contains("core property title"), "{message}");
}

#[test]
fn a_core_part_without_its_package_relationship_is_not_taken_over() {
    // Both the setter and the save refuse to adopt a part nothing points at.
    let mut package = OpcPackage::from_reader(Cursor::new(saved_with_reference())).unwrap();
    package
        .package_rels
        .items
        .retain(|relationship| !relationship.rel_type.ends_with("/core-properties"));
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    let mut prs = Presentation::from_bytes(&bytes.into_inner()).unwrap();
    assert!(prs.core_properties().is_none());
    let core = oxoffice_pptx::CoreProperties {
        title: Some("Q3".to_owned()),
        ..Default::default()
    };
    assert!(matches!(
        prs.set_core_properties(core),
        Err(Error::CorePropertiesPartCollision { .. })
    ));
}

/// A new deck whose core part holds a title and a `lastModifiedBy` with a
/// character reference XML 1.0 forbids, as another producer wrote it.
fn saved_with_reference() -> Vec<u8> {
    let mut package = OpcPackage::from_reader(Cursor::new(new_deck().to_bytes().unwrap())).unwrap();
    let core = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Deck</dc:title><cp:lastModifiedBy>J&#11;Doe</cp:lastModifiedBy></cp:coreProperties>"#;
    package.set_part("/docProps/core.xml", core.as_bytes().to_vec());
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    bytes.into_inner()
}

#[test]
fn a_number_property_set_to_negative_zero_replaces_zero() {
    let number = |value: f64| oxoffice_pptx::CustomProperty {
        fmtid: "{D5CDD505-2E9C-101B-9397-08002B2CF9AE}".to_owned(),
        pid: 2,
        name: Some("Offset".to_owned()),
        value: oxoffice_pptx::CustomPropertyValue::R8(value),
    };
    let mut prs = new_deck();
    prs.set_custom_property(number(0.0)).unwrap();
    // -0 == 0, but the two are written differently: the edit is no no-op.
    prs.set_custom_property(number(-0.0)).unwrap();
    match reopen(&prs).custom_property("Offset").map(|p| &p.value) {
        Some(oxoffice_pptx::CustomPropertyValue::R8(value)) => {
            assert!(value.is_sign_negative(), "Offset reads {value}")
        }
        other => panic!("Offset reads {other:?}"),
    }
}

#[test]
fn a_carriage_return_or_a_tab_in_a_patched_property_survives_a_save() {
    // A deck's application and custom properties parts are patched in place.
    let custom = |name: &str, pid: i32, text: &str| oxoffice_pptx::CustomProperty {
        fmtid: "{D5CDD505-2E9C-101B-9397-08002B2CF9AE}".to_owned(),
        pid,
        name: Some(name.to_owned()),
        value: oxoffice_pptx::CustomPropertyValue::Lpwstr(text.to_owned()),
    };
    let mut prs = new_deck();
    let mut app = prs.application_properties().cloned().unwrap_or_default();
    app.company = Some("Contoso\rFils".to_owned());
    // Not in the template's app.xml: appended.
    app.template = Some("t\re".to_owned());
    prs.set_application_properties(app).unwrap();
    // The template has an empty <dc:title/> and no description: the empty-tag
    // and append branches of the core patcher.
    let mut core = prs.core_properties().cloned().unwrap_or_default();
    core.title = Some("a\rb".to_owned());
    core.description = Some("c\rd".to_owned());
    prs.set_core_properties(core).unwrap();
    prs.set_custom_property(custom("First", 2, "plain"))
        .unwrap();
    let mut prs = reopen(&prs);
    // The custom part exists now: these go through the patcher.
    prs.set_custom_property(custom("Tab\tname", 3, "a\r\nb"))
        .unwrap();
    prs.set_custom_property(custom("First", 2, "x\ry")).unwrap();
    let prs = reopen(&prs);
    assert_eq!(
        prs.application_properties()
            .and_then(|app| app.company.as_deref()),
        Some("Contoso\rFils")
    );
    let value = |name: &str| {
        prs.custom_property(name)
            .map(|property| property.value.clone())
    };
    assert_eq!(
        value("Tab\tname"),
        Some(oxoffice_pptx::CustomPropertyValue::Lpwstr(
            "a\r\nb".to_owned()
        ))
    );
    assert_eq!(
        value("First"),
        Some(oxoffice_pptx::CustomPropertyValue::Lpwstr(
            "x\ry".to_owned()
        ))
    );
    let package = OpcPackage::from_reader(Cursor::new(prs.to_bytes().unwrap())).unwrap();
    // Each value is written with a reference, never a raw CR (the template's
    // own line ends may hold CRs elsewhere in the part).
    for (part, written) in [
        ("/docProps/app.xml", "Contoso&#13;Fils"),
        ("/docProps/app.xml", "t&#13;e"),
        ("/docProps/core.xml", "a&#13;b"),
        ("/docProps/core.xml", "c&#13;d"),
        ("/docProps/custom.xml", "a&#13;\nb"),
        ("/docProps/custom.xml", "x&#13;y"),
        ("/docProps/custom.xml", "Tab&#9;name"),
    ] {
        let xml = String::from_utf8_lossy(package.get_part(part).unwrap());
        assert!(xml.contains(written), "{part}: {written} in {xml}");
    }
}

#[test]
fn a_removed_property_removes_its_element() {
    // A new deck's own core part writes no empty element either.
    let mut prs = new_deck();
    prs.core_properties_mut().created = Some(String::new());
    let saved = saved_part_text(&prs, "/docProps/core.xml").unwrap();
    assert!(!saved.contains("created"), "{saved}");

    let mut prs = new_deck();
    let mut core = prs.core_properties().cloned().unwrap_or_default();
    core.created = Some("2026-01-01T00:00:00Z".to_owned());
    core.title = Some("Plan".to_owned());
    prs.set_core_properties(core).unwrap();
    let mut prs = reopen(&prs);
    let mut core = prs.core_properties().cloned().unwrap();
    core.created = None;
    // An empty value reads back as no value: it removes the property too.
    core.title = Some(String::new());
    prs.set_core_properties(core).unwrap();
    let saved = saved_part_text(&reopen(&prs), "/docProps/core.xml").unwrap();
    // An empty W3CDTF date is not a valid date: the element goes.
    assert!(
        !saved.contains("created") && !saved.contains("title"),
        "{saved}"
    );

    let mut prs = corpus_deck("authored/deck.pptx");
    let mut app = prs.application_properties().unwrap().clone();
    assert!(app.slides.is_some() && app.company.is_some(), "{app:?}");
    app.slides = None;
    app.company = None;
    prs.set_application_properties(app).unwrap();
    let saved = saved_part_text(&reopen(&prs), "/docProps/app.xml").unwrap();
    assert!(
        !saved.contains("<Slides>") && !saved.contains("Company"),
        "{saved}"
    );
}

#[test]
fn a_text_count_set_after_a_pending_slide_edit_is_kept() {
    let mut prs = corpus_deck("powerpoint/deck.pptx");
    prs.slide_mut(0)
        .unwrap()
        .shape_mut(0)
        .unwrap()
        .set_text("A new title")
        .unwrap();
    let mut app = prs.application_properties().unwrap().clone();
    app.words = Some(120);
    prs.set_application_properties(app).unwrap();
    assert_eq!(prs.application_properties().unwrap().words, Some(120));
    let saved = reopen(&prs);
    assert_eq!(saved.application_properties().unwrap().words, Some(120));
    let app = saved_part_text(&saved, "/docProps/app.xml").unwrap();
    // The stale counts of the copy do not come back.
    assert!(
        !app.contains("<TitlesOfParts>") && !app.contains("Paragraphs>"),
        "{app}"
    );
}

// ---- Transitions and shape ids, as a tool checks a deck ----

/// The effect and direction of each slide's transition, `None` without one.
fn transition_kinds(
    prs: &Presentation,
) -> Vec<Option<(Option<TransitionKind>, Option<TransitionDirection>)>> {
    prs.slides()
        .map(|slide| slide.transition().map(|t| (t.kind, t.direction)))
        .collect()
}

#[test]
fn the_transitions_of_the_pitch_deck_are_read_from_every_producer() {
    let authored = transition_kinds(&corpus_deck("authored/pitch.pptx"));
    use TransitionDirection as D;
    use TransitionKind as K;
    assert_eq!(
        authored,
        [
            Some((Some(K::Fade), None)),
            Some((Some(K::Push), Some(D::Up))),
            Some((Some(K::Push), Some(D::Up))),
            Some((Some(K::Wipe), Some(D::Right))),
            Some((Some(K::Wipe), Some(D::Right))),
            Some((Some(K::Split), Some(D::VerticalOut))),
            Some((Some(K::Cover), Some(D::Left))),
            Some((Some(K::Other("randomBar".to_owned())), None)),
            Some((Some(K::Zoom), Some(D::In))),
            Some((Some(K::Fade), None)),
        ]
    );
    // Every producer's copy reads a transition, with its effect, on each
    // slide whose file has one. PowerPoint keeps the authored effects and
    // directions but leaves out a kind's default: a cover from the left reads
    // as `None` (split's default is filled in, randomBar's direction is not
    // modelled). LibreOffice keeps the effects but turns that cover to the
    // right and drops the zoom; Google writes its own effects.
    for producer in ["powerpoint", "libreoffice", "google"] {
        let path = format!("{producer}/pitch.pptx");
        let read = transition_kinds(&corpus_deck(&path));
        let package = OpcPackage::from_reader(Cursor::new(
            std::fs::read(crate::support::corpus_root().join("files").join(&path)).unwrap(),
        ))
        .unwrap();
        let in_file = (1..=read.len())
            .map(|n| {
                let part = package
                    .get_part(&format!("/ppt/slides/slide{n}.xml"))
                    .unwrap();
                String::from_utf8_lossy(part).contains("<p:transition")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            read.iter()
                .map(|t| matches!(t, Some((Some(_), _))))
                .collect::<Vec<_>>(),
            in_file,
            "{producer}"
        );
        if producer == "powerpoint" {
            let defaulted = authored
                .iter()
                .map(|t| {
                    t.clone().map(|(kind, direction)| match kind {
                        Some(TransitionKind::Cover) => (kind, None),
                        _ => (kind, direction),
                    })
                })
                .collect::<Vec<_>>();
            assert_eq!(read, defaulted, "{producer}");
        }
    }
}

#[test]
fn validation_reports_two_shapes_of_a_slide_sharing_an_id() {
    let mut prs = new_deck();
    let blank = (0..prs.layout_count())
        .find(|&index| prs.layout_name(index) == Some("Blank"))
        .unwrap();
    prs.add_slide(blank).unwrap();
    for left in [1, 4] {
        prs.slide_mut(0)
            .unwrap()
            .add_textbox(
                Emu(left * 914_400),
                Emu(914_400),
                Emu(914_400),
                Emu(914_400),
            )
            .unwrap();
    }
    let mut package = OpcPackage::from_reader(Cursor::new(prs.to_bytes().unwrap())).unwrap();
    let slide = "/ppt/slides/slide1.xml";
    let xml = String::from_utf8(package.get_part(slide).unwrap().to_vec()).unwrap();
    let ids = xml
        .match_indices("<p:cNvPr id=\"")
        .map(|(at, found)| {
            let rest = &xml[at + found.len()..];
            rest[..rest.find('"').unwrap()].to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(ids.len(), 3, "the tree and two text boxes: {ids:?}");
    // The second text box takes the first one's id.
    let duplicated = xml.replacen(
        &format!("<p:cNvPr id=\"{}\"", ids[2]),
        &format!("<p:cNvPr id=\"{}\"", ids[1]),
        1,
    );
    package.set_part(slide, duplicated.into_bytes());
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    let prs = Presentation::from_bytes(&bytes.into_inner()).expect("a duplicate id opens");
    assert_eq!(
        prs.validate(),
        [ValidationIssue::DuplicateShapeId {
            slide: 0,
            id: ids[1].parse().unwrap()
        }]
    );
}

#[test]
fn a_slide_name_holding_a_tab_and_line_ends_survives_an_odp_round_trip() {
    let name = "Plan\tQ1\nend\rnote";
    let deck = deck_with_titles(&["Plan"]);
    let mut package = OpcPackage::from_reader(Cursor::new(deck.to_bytes().unwrap())).unwrap();
    let slide = String::from_utf8(package.get_part("/ppt/slides/slide1.xml").unwrap().to_vec())
        .unwrap()
        .replacen(
            "<p:cSld>",
            r#"<p:cSld name="Plan&#9;Q1&#10;end&#13;note">"#,
            1,
        );
    assert!(slide.contains("&#9;Q1"), "the slide has a p:cSld to name");
    package.set_part("/ppt/slides/slide1.xml", slide.into_bytes());
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    let deck = Presentation::from_bytes(&bytes.into_inner()).unwrap();
    assert_eq!(deck.slide(0).unwrap().name(), Some(name));

    let odp = deck.to_odp_bytes().unwrap().bytes;
    let read = Presentation::from_odp_bytes(&odp).unwrap().presentation;
    assert_eq!(read.slide(0).unwrap().name(), Some(name));
}

#[test]
fn slide_text_with_markup_characters_reads_back_from_odp() {
    // ODP writes them as `&amp;`, `&lt;`, `&apos;`... and reads them back.
    // A tab is written as `&#9;`, a character reference.
    let text = "R&D <don't> \"go\"\tnow";
    let mut deck = deck_with_titles(&["Plain"]);
    deck.slide_mut(0)
        .unwrap()
        .add_textbox(Emu(914_400), Emu(914_400), Emu(4_572_000), Emu(914_400))
        .unwrap()
        .set_text(text)
        .unwrap();
    let odp = deck.to_odp_bytes().unwrap().bytes;
    let read = Presentation::from_odp_bytes(&odp).unwrap().presentation;
    let slide = read.slide(0).unwrap().text();
    assert!(slide.contains(text), "{slide}");
}

#[test]
fn removing_a_slide_takes_its_title_out_of_the_package_metadata() {
    let mut prs = corpus_deck("powerpoint/deck.pptx");
    let before = saved_part_text(&prs, "/docProps/app.xml").unwrap();
    assert!(
        before.contains("<vt:lpstr>Highlights</vt:lpstr>"),
        "{before}"
    );
    // Hiding Highlights counts it as hidden; removing it must count it out.
    prs.slide_mut(1).unwrap().set_hidden(true);
    assert!(
        saved_part_text(&prs, "/docProps/app.xml")
            .unwrap()
            .contains("<HiddenSlides>1</HiddenSlides>")
    );

    prs.remove_slide(1).unwrap();

    let prs = reopen(&prs);
    let app = saved_part_text(&prs, "/docProps/app.xml").unwrap();
    assert!(!app.contains("Highlights"), "{app}");
    // Three slides left, the notes and the hidden slide went with Highlights;
    // the title lists go and every other byte stays.
    let cut = |xml: &str, from: &str, to: &str| {
        let start = xml.find(from).expect("the element is there");
        let end = xml[start..].find(to).expect("the element closes") + start + to.len();
        format!("{}{}", &xml[..start], &xml[end..])
    };
    // The text counts go too: oxoffice does not recount them.
    let expected = cut(
        &cut(
            &cut(
                &cut(&before, "<HeadingPairs>", "</HeadingPairs>"),
                "<TitlesOfParts>",
                "</TitlesOfParts>",
            ),
            "<Words>",
            "</Words>",
        ),
        "<Paragraphs>",
        "</Paragraphs>",
    )
    .replace("<Slides>4</Slides>", "<Slides>3</Slides>")
    .replace("<Notes>1</Notes>", "<Notes>0</Notes>");
    assert!(
        expected.contains("<HiddenSlides>0</HiddenSlides>"),
        "{expected}"
    );
    assert_eq!(app, expected);
    assert!(prs.validate().is_empty(), "{:?}", prs.validate());
}

#[test]
fn a_slide_text_edit_drops_the_stale_title_lists_and_text_counts_and_keeps_the_title() {
    let mut prs = corpus_deck("powerpoint/deck.pptx");
    let core = saved_part_text(&prs, "/docProps/core.xml").unwrap();
    assert!(
        core.contains("<dc:title>Quarterly Operations Review</dc:title>"),
        "{core}"
    );
    prs.slide_mut(0)
        .unwrap()
        .shape_mut(0)
        .unwrap()
        .set_text("A new title")
        .unwrap();
    let saved = reopen(&prs);
    let app = saved_part_text(&saved, "/docProps/app.xml").unwrap();
    for gone in [
        "<HeadingPairs>",
        "<TitlesOfParts>",
        "<Words>",
        "<Paragraphs>",
    ] {
        assert!(!app.contains(gone), "{gone}: {app}");
    }
    assert!(app.contains("<Slides>4</Slides>"), "{app}");
    // The core title is the user's: a slide edit leaves it alone.
    assert_eq!(saved_part_text(&saved, "/docProps/core.xml").unwrap(), core);
}

#[test]
fn a_slide_size_type_stays_with_its_size_and_goes_with_a_new_one() {
    let mut prs = new_deck();
    let (width, height) = prs.slide_size().unwrap();
    prs.set_slide_size(width, height).unwrap();
    assert!(
        saved_slide_size_element(&reopen(&prs)).contains("type=\"screen16x9\""),
        "the same size keeps its type"
    );

    // The authored corpus deck already says screen4x3 on a wide size.
    let mut prs = corpus_deck("authored/deck.pptx");
    assert!(saved_slide_size_element(&prs).contains("type=\"screen4x3\""));
    assert_ne!(prs.slide_size(), Some((Emu(9_906_000), Emu(6_858_000))));
    prs.set_slide_size(Emu(9_906_000), Emu(6_858_000)).unwrap();
    let prs = reopen(&prs);
    assert_eq!(
        saved_slide_size_element(&prs),
        r#"<p:sldSz cx="9906000" cy="6858000"/>"#
    );
    assert_eq!(prs.slide_size(), Some((Emu(9_906_000), Emu(6_858_000))));
}

#[test]
fn removing_a_slide_drops_a_tags_part_only_it_used() {
    let prs = deck_with_titles(&["Kept", "Tagged"]);
    let mut package = OpcPackage::from_reader(Cursor::new(prs.to_bytes().unwrap())).unwrap();
    package.set_part(
        "/ppt/tags/tag1.xml",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:tagLst xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:tag name="CLIENT" val="Contoso, internal only"/></p:tagLst>"#
            .to_vec(),
    );
    package.content_types.add_override(
        "/ppt/tags/tag1.xml",
        "application/vnd.openxmlformats-officedocument.presentationml.tags+xml",
    );
    package
        .get_or_create_part_rels("/ppt/slides/slide2.xml")
        .add(
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships/tags",
            "../tags/tag1.xml",
        );
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    let mut prs = Presentation::from_bytes(&bytes.into_inner()).unwrap();
    assert_eq!(count_parts(&prs, "ppt/tags/"), 1);

    prs.remove_slide(1).unwrap();

    let prs = reopen(&prs);
    assert_eq!(count_parts(&prs, "ppt/tags/"), 0);
    assert_eq!(titles(&prs), ["Kept"]);
    assert!(prs.validate().is_empty(), "{:?}", prs.validate());
}

fn count_parts(prs: &Presentation, folder: &str) -> usize {
    saved_parts(prs)
        .iter()
        .filter(|name| name.starts_with(folder) && !name.contains("_rels"))
        .count()
}

/// The part names of the saved package.
fn saved_parts(prs: &Presentation) -> Vec<String> {
    let bytes = prs.to_bytes().expect("deck saves");
    OpcPackage::from_reader(Cursor::new(bytes))
        .expect("package opens")
        .zip_entries()
        .expect("package serialises")
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}
