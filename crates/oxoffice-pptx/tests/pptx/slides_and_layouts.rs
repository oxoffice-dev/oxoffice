//! Slides and layouts: adding slides from a layout and filling their
//! placeholders, reading, moving, duplicating, hiding, removing and importing
//! slides, the slide size, masters and the layouts they offer, backgrounds,
//! sections, and the links and parts that must follow a slide through it all.

use std::io::Cursor;

use oxoffice_opc::OpcPackage;
use oxoffice_pptx::{
    Comment, CommentAuthor, Emu, Error, Fill, ImportSlideOptions, Presentation, Section, ShapeKind,
    ShapeOwner, SlideRef,
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

fn count_parts(prs: &Presentation, folder: &str) -> usize {
    saved_parts(prs)
        .iter()
        .filter(|name| name.starts_with(folder) && !name.contains("_rels"))
        .count()
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

/// The non-visual id of the first picture on one slide.
fn picture_id(prs: &Presentation, slide: usize) -> u32 {
    prs.slide(slide)
        .unwrap()
        .shapes()
        .find(|shape| shape.kind() == ShapeKind::Picture)
        .and_then(|shape| shape.non_visual_id())
        .expect("slide has a picture")
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
fn a_slide_added_from_a_layout_gets_the_placeholders_the_layout_offers() {
    let mut prs = new_deck();
    let date_footer_number = [
        placeholder(Some("dt"), 10),
        placeholder(Some("ftr"), 11),
        placeholder(Some("sldNum"), 12),
    ];
    // The date, footer and slide-number placeholders stay on the layout
    // until headers and footers are turned on, as in PowerPoint.
    let cases = [
        (
            "Title Slide",
            vec![
                placeholder(Some("ctrTitle"), 0),
                placeholder(Some("subTitle"), 1),
            ],
        ),
        (
            "Two Content",
            vec![
                placeholder(Some("title"), 0),
                placeholder(None, 1),
                placeholder(None, 2),
            ],
        ),
        (
            "Picture with Caption",
            vec![
                placeholder(Some("title"), 0),
                placeholder(Some("pic"), 1),
                placeholder(Some("body"), 2),
            ],
        ),
        ("Blank", vec![]),
    ];
    for (name, expected) in &cases {
        let layout = layout_named(&prs, name);
        let mut offered = expected.clone();
        offered.extend(date_footer_number.iter().cloned());
        assert_eq!(layout_placeholders(&prs, layout), offered, "{name}");
        prs.add_slide(layout).unwrap();
    }
    let prs = reopen(&prs);
    for (index, (name, expected)) in cases.iter().enumerate() {
        assert_eq!(slide_layout_names(&prs)[index], *name);
        assert_eq!(
            slide_placeholders(prs.slide(index).unwrap()),
            *expected,
            "{name}"
        );
    }
    assert!(prs.validate().is_empty(), "{:?}", prs.validate());
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
fn every_layout_of_every_corpus_deck_gives_a_slide_that_reopens_clean() {
    for deck in [
        "powerpoint/deck.pptx",
        "google/deck.pptx",
        "libreoffice/deck.pptx",
        "authored/deck.pptx",
    ] {
        let mut prs = corpus_deck(deck);
        let names: Vec<_> = (0..prs.layout_count())
            .map(|index| prs.layout_name(index).unwrap().to_owned())
            .collect();
        for layout in 0..prs.layout_count() {
            prs.add_slide(layout).unwrap();
        }
        let prs = reopen(&prs);
        assert_eq!(slide_layout_names(&prs)[4..], names[..], "{deck}");
        assert!(prs.validate().is_empty(), "{deck}: {:?}", prs.validate());
    }
}

#[test]
fn a_slide_is_inserted_at_the_position_asked() {
    let mut prs = deck_with_titles(&["A", "B", "C"]);
    let layout = layout_named(&prs, "Title Only");
    for (position, title) in [(0, "first"), (2, "middle"), (5, "last")] {
        assert_eq!(prs.add_slide_at(layout, position).unwrap(), position);
        prs.slide_mut(position)
            .unwrap()
            .shape_mut(0)
            .unwrap()
            .set_text(title)
            .unwrap();
    }
    let prs = reopen(&prs);
    assert_eq!(titles(&prs), ["first", "A", "middle", "B", "C", "last"]);
    let mut ids = slide_ids(&prs);
    assert!(ids.iter().all(|id| *id >= 256), "{ids:?}");
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 6, "slide ids are unique");
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
fn every_corpus_deck_reads_four_titled_slides_on_their_layouts_before_and_after_a_save() {
    let office = [
        "Title Slide",
        "Title and Content",
        "Title Only",
        "Title Only",
    ];
    let google = ["TITLE", "OBJECT", "TITLE_ONLY", "TITLE_ONLY"];
    for (deck, layouts) in [
        ("powerpoint/deck.pptx", office),
        ("google/deck.pptx", google),
        ("libreoffice/deck.pptx", office),
        ("authored/deck.pptx", office),
    ] {
        let opened = corpus_deck(deck);
        for prs in [&opened, &reopen(&opened)] {
            assert_eq!(titles(prs), CORPUS_TITLES, "{deck}");
            assert_eq!(slide_ids(prs), [256, 257, 258, 259], "{deck}");
            assert_eq!(slide_layout_names(prs), layouts, "{deck}");
            let title_only = prs.slide_layout_index(2).unwrap();
            assert_eq!(
                prs.layout_used_by_slides(title_only).unwrap(),
                [2, 3],
                "{deck}"
            );
            assert!(prs.slides().all(|slide| !slide.hidden()), "{deck}");
            assert_eq!(
                prs.slide(1).unwrap().notes_text().as_deref(),
                Some(HIGHLIGHTS_NOTES),
                "{deck}"
            );
            assert_eq!(prs.slide_size().unwrap().1, Emu(6_858_000), "{deck}");
        }
    }
}

#[test]
fn the_master_lists_its_layouts_in_gallery_order_and_indexes_stay_put_after_a_save() {
    // Layout indexes follow the package; the master's list gives the order
    // the producer shows in its layout gallery.
    let google = [
        "TITLE",
        "OBJECT",
        "TITLE_ONLY",
        "SECTION_HEADER",
        "TWO_OBJECTS",
        "TWO_OBJECTS_WITH_TEXT",
        "BLANK",
        "OBJECT_WITH_CAPTION_TEXT",
        "PICTURE_WITH_CAPTION_TEXT",
        "VERTICAL_TEXT",
        "VERTICAL_TITLE_AND_VERTICAL_TEXT",
    ];
    for (deck, gallery) in [
        ("powerpoint/deck.pptx", &OFFICE_LAYOUTS),
        ("google/deck.pptx", &google),
    ] {
        let mut prs = corpus_deck(deck);
        let listed: Vec<_> = prs
            .master_layout_indices(0)
            .unwrap()
            .into_iter()
            .map(|index| prs.layout_name(index).unwrap())
            .collect();
        assert_eq!(listed, gallery, "{deck}");
        assert!(
            (0..prs.layout_count()).all(|index| prs.layout_master_index(index).unwrap() == 0),
            "{deck}"
        );
        let names = |prs: &Presentation| -> Vec<String> {
            (0..prs.layout_count())
                .map(|index| prs.layout_name(index).unwrap().to_owned())
                .collect()
        };
        let before = names(&prs);
        prs.add_slide(0).unwrap();
        assert_eq!(names(&reopen(&prs)), before, "{deck}");
    }
}

#[test]
fn moving_a_slide_reorders_the_deck_and_the_slide_keeps_its_id_layout_and_notes() {
    let mut prs = corpus_deck("powerpoint/deck.pptx");
    let before = prs.to_bytes().unwrap();
    prs.move_slide(1, 3).unwrap();
    let moved = reopen(&prs);
    assert_eq!(
        titles(&moved),
        [
            CORPUS_TITLES[0],
            CORPUS_TITLES[2],
            CORPUS_TITLES[3],
            CORPUS_TITLES[1]
        ]
    );
    assert_eq!(slide_ids(&moved), [256, 258, 259, 257]);
    assert_eq!(
        slide_layout_names(&moved),
        [
            "Title Slide",
            "Title Only",
            "Title Only",
            "Title and Content"
        ]
    );
    assert_eq!(
        moved.slide(3).unwrap().notes_text().as_deref(),
        Some(HIGHLIGHTS_NOTES)
    );
    assert!(moved.picture_image(2, picture_id(&moved, 2)).is_ok());
    assert!(moved.validate().is_empty(), "{:?}", moved.validate());

    // A move only reorders the slide list: moving back gives the same file.
    prs.move_slide(3, 1).unwrap();
    assert_eq!(prs.to_bytes().unwrap(), before);
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
fn a_duplicate_follows_its_source_with_the_same_content_layout_and_notes() {
    let mut prs = corpus_deck("powerpoint/deck.pptx");
    let copy_id = prs.duplicate_slide(1).unwrap().id();
    assert!(!slide_ids(&corpus_deck("powerpoint/deck.pptx")).contains(&copy_id));
    prs.slide_mut(2)
        .unwrap()
        .shape_mut(0)
        .unwrap()
        .set_text("Highlights, continued")
        .unwrap();

    let prs = reopen(&prs);
    assert_eq!(slide_ids(&prs), [256, 257, copy_id, 258, 259]);
    assert_eq!(titles(&prs)[1..3], ["Highlights", "Highlights, continued"]);
    let (source, copy) = (prs.slide(1).unwrap(), prs.slide(2).unwrap());
    assert_eq!(
        source.placeholder(1).unwrap().text(),
        copy.placeholder(1).unwrap().text()
    );
    assert_eq!(copy.notes_text().as_deref(), Some(HIGHLIGHTS_NOTES));
    assert_eq!(slide_layout_names(&prs)[2], "Title and Content");
    assert!(prs.validate().is_empty(), "{:?}", prs.validate());
}

#[test]
fn a_picture_slide_shares_its_image_with_its_duplicate_and_the_last_removal_drops_it() {
    let mut prs = corpus_deck("powerpoint/deck.pptx");
    let picture = picture_id(&prs, 3);
    let image = prs.picture_image(3, picture).unwrap().bytes.to_vec();
    prs.duplicate_slide(3).unwrap();
    let mut prs = reopen(&prs);
    // The copy's shapes get fresh ids.
    let copied = picture_id(&prs, 4);
    assert_eq!(prs.picture_image(4, copied).unwrap().bytes, image);
    assert_eq!(count_parts(&prs, "ppt/media/"), 1, "one image part, shared");

    prs.remove_slide(3).unwrap();
    assert_eq!(prs.picture_image(3, copied).unwrap().bytes, image);
    assert_eq!(count_parts(&prs, "ppt/media/"), 1);
    prs.remove_slide(3).unwrap();
    assert_eq!(count_parts(&prs, "ppt/media/"), 0);
    let prs = reopen(&prs);
    assert_eq!(titles(&prs), CORPUS_TITLES[..3]);
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
fn removing_a_slide_drops_it_and_its_notes_and_keeps_the_others() {
    let mut prs = corpus_deck("powerpoint/deck.pptx");
    assert_eq!(count_parts(&prs, "ppt/notesSlides/"), 1);
    prs.remove_slide(1).unwrap();
    assert_eq!(count_parts(&prs, "ppt/notesSlides/"), 0);
    assert_eq!(count_parts(&prs, "ppt/slides/"), 3);
    let prs = reopen(&prs);
    assert_eq!(
        titles(&prs),
        [CORPUS_TITLES[0], CORPUS_TITLES[2], CORPUS_TITLES[3]]
    );
    assert_eq!(slide_ids(&prs), [256, 258, 259]);
    assert_eq!(
        slide_layout_names(&prs),
        ["Title Slide", "Title Only", "Title Only"]
    );
    assert!(prs.validate().is_empty(), "{:?}", prs.validate());
}

#[test]
fn removing_every_slide_leaves_an_empty_deck_that_takes_new_slides() {
    let mut prs = corpus_deck("powerpoint/deck.pptx");
    while !prs.is_empty() {
        prs.remove_slide(0).unwrap();
    }
    let mut prs = reopen(&prs);
    assert!(prs.is_empty());
    assert_eq!(prs.layout_count(), 11);
    prs.add_slide(layout_named(&prs, "Title Slide")).unwrap();
    let prs = reopen(&prs);
    assert_eq!(prs.len(), 1);
    assert_eq!(slide_layout_names(&prs), ["Title Slide"]);
    assert!(prs.validate().is_empty(), "{:?}", prs.validate());
}

#[test]
fn imported_slides_take_the_layouts_named_like_theirs_and_keep_text_notes_and_pictures() {
    let source = corpus_deck("powerpoint/deck.pptx");
    let image = source
        .picture_image(3, picture_id(&source, 3))
        .unwrap()
        .bytes
        .to_vec();
    let mut prs = new_deck();
    let (inserted, diagnostics) = prs
        .import_slides(&source, &[1, 3], &ImportSlideOptions::default(), &[])
        .unwrap();
    assert_eq!(inserted, [0, 1]);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");

    let prs = reopen(&prs);
    assert_eq!(prs.master_count(), 1, "the slides take this deck's theme");
    assert_eq!(
        slide_layout_names(&prs),
        ["Title and Content", "Title Only"]
    );
    assert_eq!(
        prs.slide(0).unwrap().text(),
        source.slide(1).unwrap().text()
    );
    assert_eq!(
        prs.slide(0).unwrap().notes_text().as_deref(),
        Some(HIGHLIGHTS_NOTES)
    );
    assert_eq!(
        prs.picture_image(1, picture_id(&prs, 1)).unwrap().bytes,
        image
    );
    assert!(prs.validate().is_empty(), "{:?}", prs.validate());
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

// Fails today: keep_source_formatting refuses PowerPoint and Google slides with notes. Issue: #40
#[test]
#[should_panic(expected = "unrepresentable source notes-master")]
fn keeping_source_formatting_imports_a_slide_with_speaker_notes() {
    let options = ImportSlideOptions {
        keep_source_formatting: true,
        ..ImportSlideOptions::default()
    };
    let mut failures = Vec::new();
    for deck in ["powerpoint/deck.pptx", "google/deck.pptx"] {
        let source = corpus_deck(deck);
        let mut prs = new_deck();
        match prs.import_slides(&source, &[1], &options, &[]) {
            Ok(_) => assert_eq!(
                reopen(&prs).slide(0).unwrap().notes_text().as_deref(),
                Some(HIGHLIGHTS_NOTES),
                "{deck}"
            ),
            Err(error) => failures.push(format!("{deck}: {error}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
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

// Fails today: set_slide_size keeps the old `type`, a 4:3 deck says screen16x9. Issue: #41
#[test]
#[should_panic(expected = "a 4:3 deck is saved as")]
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

// Fails today: added, duplicated and imported slides belong to no section. Issue: #41
#[test]
#[should_panic(expected = "a new slide joins no section")]
fn a_slide_added_or_duplicated_inside_a_section_joins_it() {
    let source = deck_with_titles(&["Imported"]);
    let layout = layout_named(&new_deck(), "Title Only");
    let mut failures = Vec::new();
    let mut check = |operation: &str, prs: &Presentation, position: usize, section: &str| {
        let id = prs.slide(position).unwrap().id();
        let owner = prs
            .sections()
            .iter()
            .find(|candidate| candidate.slide_ids.contains(&id))
            .and_then(|owner| owner.name.clone());
        if owner.as_deref() != Some(section) {
            failures.push(format!("{operation}: expected {section}, got {owner:?}"));
        }
    };

    let mut prs = deck_in_two_sections();
    prs.duplicate_slide(0).unwrap();
    check("duplicate_slide(0)", &reopen(&prs), 1, "A");
    let mut prs = deck_in_two_sections();
    prs.add_slide_at(layout, 3).unwrap();
    check("add_slide_at(_, 3)", &reopen(&prs), 3, "B");
    let mut prs = deck_in_two_sections();
    prs.add_slide(layout).unwrap();
    check("add_slide", &reopen(&prs), 4, "B");
    let mut prs = deck_in_two_sections();
    prs.import_slide(&source, 0, Some(layout), Some(1)).unwrap();
    check("import_slide(_, _, _, Some(1))", &reopen(&prs), 1, "A");

    assert!(
        failures.is_empty(),
        "a new slide joins no section:\n{}",
        failures.join("\n")
    );
}

// Fails today: move_slide leaves the moved slide listed in its old section. Issue: #41
#[test]
#[should_panic(expected = "the moved slide stays in its old section")]
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

/// The text of one saved part, if the saved package has it.
fn saved_part_text(prs: &Presentation, part_name: &str) -> Option<String> {
    let bytes = prs.to_bytes().expect("deck saves");
    let package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    package
        .get_part(part_name)
        .map(|part| String::from_utf8_lossy(part).into_owned())
}

#[test]
fn removing_a_slide_takes_its_title_out_of_the_package_metadata() {
    let mut prs = corpus_deck("powerpoint/deck.pptx");
    prs.slide_mut(3).unwrap().set_hidden(true);
    let before = saved_part_text(&prs, "/docProps/app.xml").unwrap();
    assert!(
        before.contains("<vt:lpstr>Highlights</vt:lpstr>"),
        "{before}"
    );

    prs.remove_slide(1).unwrap();

    let prs = reopen(&prs);
    let app = saved_part_text(&prs, "/docProps/app.xml").unwrap();
    assert!(!app.contains("Highlights"), "{app}");
    // Three slides left, the notes went with Highlights, the last is hidden;
    // the title lists go and every other byte stays.
    let cut = |xml: &str, from: &str, to: &str| {
        let start = xml.find(from).expect("the element is there");
        let end = xml[start..].find(to).expect("the element closes") + start + to.len();
        format!("{}{}", &xml[..start], &xml[end..])
    };
    let expected = cut(
        &cut(&before, "<HeadingPairs>", "</HeadingPairs>"),
        "<TitlesOfParts>",
        "</TitlesOfParts>",
    )
    .replace("<Slides>4</Slides>", "<Slides>3</Slides>")
    .replace("<Notes>1</Notes>", "<Notes>0</Notes>")
    .replace(
        "<HiddenSlides>0</HiddenSlides>",
        "<HiddenSlides>1</HiddenSlides>",
    );
    assert_eq!(app, expected);
    assert!(prs.validate().is_empty(), "{:?}", prs.validate());
}

#[test]
fn removing_any_slide_drops_the_thumbnail_that_may_picture_it() {
    let thumbnail_gone = |prs: &Presentation, name: &str| {
        assert!(
            saved_part_text(prs, "/docProps/thumbnail.jpeg").is_none(),
            "{name}"
        );
        let package = OpcPackage::from_reader(Cursor::new(prs.to_bytes().unwrap())).unwrap();
        assert!(
            package
                .package_rels
                .items
                .iter()
                .all(|relationship| !relationship.rel_type.ends_with("/metadata/thumbnail")),
            "{name}: the package still points at a thumbnail"
        );
        assert!(prs.validate().is_empty(), "{name}: {:?}", prs.validate());
    };
    for name in ["powerpoint/deck.pptx", "authored/deck.pptx"] {
        let mut prs = corpus_deck(name);
        assert!(
            saved_part_text(&prs, "/docProps/thumbnail.jpeg").is_some(),
            "{name}"
        );
        prs.remove_slide(0).unwrap();
        thumbnail_gone(&reopen(&prs), name);
    }

    // The pictured first slide, moved last, then removed.
    let mut prs = corpus_deck("powerpoint/deck.pptx");
    prs.move_slide(0, 3).unwrap();
    prs.remove_slide(3).unwrap();
    let prs = reopen(&prs);
    assert!(!titles(&prs).contains(&CORPUS_TITLES[0].to_owned()));
    thumbnail_gone(&prs, "moved first slide");
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

    // Google writes no app.xml, LibreOffice one without counts or titles.
    for name in ["google/deck.pptx", "libreoffice/deck.pptx"] {
        let mut prs = corpus_deck(name);
        let before = saved_part_text(&prs, "/docProps/app.xml");
        prs.remove_slide(1).unwrap();
        let prs = reopen(&prs);
        assert_eq!(saved_part_text(&prs, "/docProps/app.xml"), before, "{name}");
        assert!(prs.validate().is_empty(), "{name}: {:?}", prs.validate());
    }
}

#[test]
fn the_notes_count_after_a_removal_counts_slides_with_notes_text() {
    let mut prs = corpus_deck("powerpoint/deck.pptx");
    prs.set_notes_text(1, "").unwrap();

    prs.remove_slide(3).unwrap();

    let app = saved_part_text(&reopen(&prs), "/docProps/app.xml").unwrap();
    assert!(app.contains("<Notes>0</Notes>"), "{app}");
}
