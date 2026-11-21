//! Shapes and their text: text boxes, auto shapes, connectors and groups, their
//! place, size, name and z-order, removal; paragraphs, runs and their
//! formatting, bullets, wrap and autofit, find and replace, run hyperlinks,
//! unicode text, and the text of the corpus decks, read back after a save.

use std::io::Cursor;

use oxoffice_pptx::{
    Angle, AutofitMode, CT_TextCharacterProperties, CT_TextParagraphProperties, ColorChoice,
    ConnectorEnd, ConnectorType, Emu, Error, Fill, OpcPackage, Presentation, RgbColor, ShapeKind,
    ShapeRef, ShapeType, SolidFill, TextAlignment, TextAnchor, TextAutoNumber,
    TextAutoNumberScheme, TextBullet, TextBulletCharacter, TextBulletChoice, TextFont,
    TextFrameRef, TextNoBullet, TextUnderline,
};

use crate::support::{corpus_deck, new_deck, reopen, slide_texts};

/// One inch in EMU.
const IN: i64 = 914_400;

/// A new deck with one slide of the Blank layout.
fn blank_slide_deck() -> Presentation {
    let mut prs = new_deck();
    let blank = (0..prs.layout_count())
        .find(|&index| prs.layout_name(index) == Some("Blank"))
        .expect("the default template has a Blank layout");
    prs.add_slide(blank).unwrap();
    prs
}

/// A blank-slide deck whose only shape is a text box holding `text`.
fn text_box_deck(text: &str) -> Presentation {
    let mut prs = blank_slide_deck();
    prs.slide_mut(0)
        .unwrap()
        .add_textbox(Emu(IN), Emu(IN), Emu(6 * IN), Emu(2 * IN))
        .unwrap()
        .set_text(text)
        .unwrap();
    prs
}

/// One immediate shape of one slide.
fn shape_at(prs: &Presentation, slide: usize, index: usize) -> ShapeRef<'_> {
    prs.slide(slide)
        .unwrap()
        .shape(index)
        .unwrap_or_else(|| panic!("slide {slide} has a shape {index}"))
}

/// The text of each run, paragraph by paragraph.
fn run_texts(shape: ShapeRef<'_>) -> Vec<Vec<String>> {
    let frame = shape.text_frame().expect("the shape has a text frame");
    (0..frame.paragraph_count())
        .map(|index| {
            let paragraph = frame.paragraph(index).unwrap();
            (0..paragraph.run_count())
                .map(|run| paragraph.run(run).unwrap().text().to_owned())
                .collect()
        })
        .collect()
}

/// The text of every immediate shape of a slide that has text, in z-order.
fn shape_texts(prs: &Presentation, slide: usize) -> Vec<String> {
    prs.slide(slide)
        .unwrap()
        .shapes()
        .filter_map(|shape| shape.text())
        .collect()
}

/// Character properties with one direct change.
fn character(change: impl FnOnce(&mut CT_TextCharacterProperties)) -> CT_TextCharacterProperties {
    let mut properties = CT_TextCharacterProperties::default();
    change(&mut properties);
    properties
}

/// Paragraph properties with one direct change.
fn paragraph(change: impl FnOnce(&mut CT_TextParagraphProperties)) -> CT_TextParagraphProperties {
    let mut properties = CT_TextParagraphProperties::default();
    change(&mut properties);
    properties
}

/// A part of the saved package, as text.
fn saved_part(prs: &Presentation, part_name: &str) -> String {
    let bytes = prs.to_bytes().expect("deck saves");
    let package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    let part = package.get_part(part_name).expect("the part is saved");
    String::from_utf8(part.to_vec()).expect("the part is UTF-8")
}

fn solid(red: u8, green: u8, blue: u8) -> Fill {
    let mut fill = SolidFill::default();
    fill.color = Some(ColorChoice::srgb(RgbColor::new(red, green, blue)));
    Fill::Solid(fill)
}

// Shapes

#[test]
fn a_text_box_keeps_its_text_place_and_size_after_a_save() {
    let mut prs = blank_slide_deck();
    prs.slide_mut(0)
        .unwrap()
        .add_textbox(Emu(IN), Emu(2 * IN), Emu(5 * IN), Emu(IN))
        .unwrap()
        .set_text("Quarterly results")
        .unwrap();

    let prs = reopen(&prs);
    let shape = shape_at(&prs, 0, 0);
    assert_eq!(shape.kind(), ShapeKind::Shape);
    assert_eq!(shape.shape_type(), Some(ShapeType::TextBox));
    assert_eq!(
        shape.auto_shape_type(),
        None,
        "a text box is not an auto shape"
    );
    assert_eq!(shape.position(), Some((Emu(IN), Emu(2 * IN))));
    assert_eq!(shape.size(), Some((Emu(5 * IN), Emu(IN))));
    assert_eq!(shape.text().as_deref(), Some("Quarterly results"));
    assert_eq!(slide_texts(&prs), ["Quarterly results"]);
}

#[test]
fn auto_shapes_keep_their_geometry_and_text_after_a_save() {
    let presets = [
        "rect",
        "roundRect",
        "ellipse",
        "triangle",
        "rightArrow",
        "star5",
        "chevron",
        "flowChartDecision",
    ];
    let mut prs = blank_slide_deck();
    {
        let mut slide = prs.slide_mut(0).unwrap();
        for (index, preset) in presets.iter().enumerate() {
            let left = Emu(i64::try_from(index).unwrap() * IN);
            slide
                .add_shape(preset, left, Emu(IN), Emu(IN), Emu(IN))
                .unwrap()
                .set_text(preset)
                .unwrap();
        }
        assert!(
            slide
                .add_shape("notAShape", Emu(0), Emu(0), Emu(IN), Emu(IN))
                .is_err(),
            "an unknown geometry name is refused"
        );
    }

    let prs = reopen(&prs);
    assert_eq!(prs.slide(0).unwrap().shapes().len(), presets.len());
    for (index, preset) in presets.iter().enumerate() {
        let shape = shape_at(&prs, 0, index);
        assert_eq!(shape.shape_type(), Some(ShapeType::AutoShape), "{preset}");
        assert_eq!(shape.auto_shape_type(), Some(*preset));
        assert_eq!(shape.text().as_deref(), Some(*preset));
    }
}

#[test]
fn changing_the_geometry_of_an_auto_shape_keeps_its_text_and_box() {
    let mut prs = blank_slide_deck();
    {
        let mut slide = prs.slide_mut(0).unwrap();
        let mut shape = slide
            .add_shape("roundRect", Emu(IN), Emu(IN), Emu(3 * IN), Emu(2 * IN))
            .unwrap();
        shape.set_text("Rounded").unwrap();
        shape.set_adjust_value("adj", 30_000.0).unwrap();
    }
    let mut prs = reopen(&prs);
    assert_eq!(
        shape_at(&prs, 0, 0).adjustments().unwrap(),
        [("adj".to_owned(), 30_000.0)]
    );

    prs.slide_mut(0)
        .unwrap()
        .shape_mut(0)
        .unwrap()
        .set_auto_shape_type("ellipse")
        .unwrap();
    let prs = reopen(&prs);
    let shape = shape_at(&prs, 0, 0);
    assert_eq!(shape.auto_shape_type(), Some("ellipse"));
    assert_eq!(shape.adjustments().unwrap(), []);
    assert_eq!(shape.text().as_deref(), Some("Rounded"));
    assert_eq!(shape.position(), Some((Emu(IN), Emu(IN))));
    assert_eq!(shape.size(), Some((Emu(3 * IN), Emu(2 * IN))));
}

#[test]
fn connectors_keep_their_endpoints_after_a_save() {
    let mut prs = blank_slide_deck();
    let kinds = [
        ConnectorType::Straight,
        ConnectorType::Elbow,
        ConnectorType::Curve,
    ];
    // The end lies left of and below the begin, so the connector is flipped.
    for (index, kind) in kinds.into_iter().enumerate() {
        let shift = i64::try_from(index).unwrap() * IN;
        prs.slide_mut(0)
            .unwrap()
            .add_connector(
                kind,
                Emu(4 * IN + shift),
                Emu(IN),
                Emu(IN + shift),
                Emu(3 * IN),
            )
            .unwrap();
    }

    let prs = reopen(&prs);
    for index in 0..kinds.len() {
        let shift = i64::try_from(index).unwrap() * IN;
        let connector = shape_at(&prs, 0, index);
        assert_eq!(connector.kind(), ShapeKind::Connector);
        assert_eq!(connector.shape_type(), Some(ShapeType::Line));
        assert_eq!(
            connector.connector_endpoint(ConnectorEnd::Begin).unwrap(),
            (Emu(4 * IN + shift), Emu(IN))
        );
        assert_eq!(
            connector.connector_endpoint(ConnectorEnd::End).unwrap(),
            (Emu(IN + shift), Emu(3 * IN))
        );
    }
}

#[test]
fn a_connector_glued_to_a_shape_follows_it_and_is_released_when_it_goes() {
    let mut prs = blank_slide_deck();
    let (from, to) = {
        let mut slide = prs.slide_mut(0).unwrap();
        slide
            .add_shape("rect", Emu(IN), Emu(IN), Emu(2 * IN), Emu(IN))
            .unwrap();
        slide
            .add_shape("rect", Emu(6 * IN), Emu(3 * IN), Emu(2 * IN), Emu(IN))
            .unwrap();
        slide
            .add_connector(ConnectorType::Straight, Emu(0), Emu(0), Emu(IN), Emu(IN))
            .unwrap();
        (
            slide.shape(0).unwrap().non_visual_id().unwrap(),
            slide.shape(1).unwrap().non_visual_id().unwrap(),
        )
    };
    // Sites of a rectangle, in preset order: top, left, bottom, right.
    prs.connect_connector(0, &[2], ConnectorEnd::Begin, from, 3)
        .unwrap();
    prs.connect_connector(0, &[2], ConnectorEnd::End, to, 1)
        .unwrap();
    let endpoints = |prs: &Presentation| {
        let connector = shape_at(prs, 0, prs.slide(0).unwrap().shapes().len() - 1);
        (
            connector.connector_endpoint(ConnectorEnd::Begin).unwrap(),
            connector.connector_endpoint(ConnectorEnd::End).unwrap(),
        )
    };
    assert_eq!(
        endpoints(&prs),
        (
            (Emu(3 * IN), Emu(3 * IN / 2)),
            (Emu(6 * IN), Emu(7 * IN / 2))
        )
    );

    let mut prs = reopen(&prs);
    prs.set_shape_position(0, &[1], Emu(6 * IN), Emu(5 * IN))
        .unwrap();
    let mut prs = reopen(&prs);
    assert_eq!(
        endpoints(&prs),
        (
            (Emu(3 * IN), Emu(3 * IN / 2)),
            (Emu(6 * IN), Emu(11 * IN / 2))
        ),
        "the glued end follows the moved shape"
    );

    prs.remove_shape(0, 0).unwrap();
    let prs = reopen(&prs);
    assert_eq!(prs.slide(0).unwrap().shapes().len(), 2);
    assert_eq!(
        endpoints(&prs),
        (
            (Emu(3 * IN), Emu(3 * IN / 2)),
            (Emu(6 * IN), Emu(11 * IN / 2))
        ),
        "the connector stays where it was"
    );
    // The facade has no reader for glue: the released end has no a:stCxn.
    let xml = String::from_utf8(shape_at(&prs, 0, 1).xml().unwrap()).unwrap();
    assert!(!xml.contains("stCxn"), "{xml}");
    assert!(xml.contains("endCxn"), "{xml}");
}

#[test]
fn shapes_added_to_a_group_read_back_as_its_members() {
    let mut prs = blank_slide_deck();
    prs.slide_mut(0).unwrap().add_group_shape().unwrap();
    {
        let mut members = prs.shapes_mut(0, &[0]).unwrap();
        members
            .add_textbox(Emu(IN), Emu(IN), Emu(2 * IN), Emu(IN))
            .unwrap()
            .set_text("Left")
            .unwrap();
        members
            .add_shape(
                "ellipse",
                Emu(4 * IN),
                Emu(2 * IN),
                Emu(2 * IN),
                Emu(2 * IN),
            )
            .unwrap()
            .set_text("Right")
            .unwrap();
    }
    assert!(
        prs.shapes_mut(0, &[0, 0]).is_none(),
        "a text box is not a group to add to"
    );

    let mut prs = reopen(&prs);
    let group = shape_at(&prs, 0, 0);
    assert_eq!(group.shape_type(), Some(ShapeType::Group));
    assert_eq!(group.child_count(), 2);
    let texts: Vec<_> = group.children().filter_map(|child| child.text()).collect();
    assert_eq!(texts, ["Left", "Right"]);
    assert_eq!(group.child(1).unwrap().auto_shape_type(), Some("ellipse"));
    // The group fits the union of its members.
    assert_eq!(group.position(), Some((Emu(IN), Emu(IN))));
    assert_eq!(group.size(), Some((Emu(5 * IN), Emu(3 * IN))));
    assert_eq!(slide_texts(&prs), ["Left\nRight"]);
    assert_eq!(group.text(), None, "a group has no text of its own");
    assert!(group.text_frame().is_none());

    let mut slide = prs.slide_mut(0).unwrap();
    let mut group = slide.shape_mut(0).unwrap();
    assert!(matches!(
        group.set_text("x"),
        Err(Error::UnsupportedShapeMutation { .. })
    ));
}

#[test]
fn a_group_inside_a_group_keeps_its_members_editable_after_a_save() {
    let mut prs = blank_slide_deck();
    prs.slide_mut(0).unwrap().add_group_shape().unwrap();
    {
        let mut outer = prs.shapes_mut(0, &[0]).unwrap();
        outer
            .add_textbox(Emu(IN), Emu(IN), Emu(2 * IN), Emu(IN))
            .unwrap()
            .set_text("Outer")
            .unwrap();
        outer.add_group_shape().unwrap();
    }
    prs.shapes_mut(0, &[0, 1])
        .unwrap()
        .add_textbox(Emu(4 * IN), Emu(3 * IN), Emu(2 * IN), Emu(IN))
        .unwrap()
        .set_text("Inner")
        .unwrap();

    let mut prs = reopen(&prs);
    let inner = shape_at(&prs, 0, 0).child(1).unwrap();
    assert_eq!(inner.shape_type(), Some(ShapeType::Group));
    assert_eq!(inner.child(0).unwrap().text().as_deref(), Some("Inner"));
    assert_eq!(
        shape_at(&prs, 0, 0).size(),
        Some((Emu(5 * IN), Emu(3 * IN))),
        "the outer group fits the inner group's member too"
    );

    prs.slide_mut(0)
        .unwrap()
        .shape_mut(0)
        .unwrap()
        .into_child_mut(1)
        .unwrap()
        .into_child_mut(0)
        .unwrap()
        .set_text("Inner, edited")
        .unwrap();
    let prs = reopen(&prs);
    assert_eq!(slide_texts(&prs), ["Outer\nInner, edited"]);
}

#[test]
fn grouping_and_ungrouping_shapes_keeps_them_where_they_appear() {
    let mut prs = blank_slide_deck();
    {
        let mut slide = prs.slide_mut(0).unwrap();
        for (index, text) in ["A", "B", "C"].into_iter().enumerate() {
            let left = Emu(i64::try_from(index).unwrap() * 3 * IN + IN);
            slide
                .add_shape("rect", left, Emu(IN), Emu(2 * IN), Emu(IN))
                .unwrap()
                .set_text(text)
                .unwrap();
        }
    }
    let placed = |shape: ShapeRef<'_>| (shape.text().unwrap(), shape.position().unwrap());

    prs.shapes_mut(0, &[]).unwrap().group(&[0, 1]).unwrap();
    let mut prs = reopen(&prs);
    let slide = prs.slide(0).unwrap();
    assert_eq!(slide.shapes().len(), 2);
    let group = slide
        .shapes()
        .find(|shape| shape.kind() == ShapeKind::Group)
        .expect("the slide holds the new group");
    let members: Vec<_> = group.children().map(placed).collect();
    assert_eq!(
        members,
        [
            ("A".to_owned(), (Emu(IN), Emu(IN))),
            ("B".to_owned(), (Emu(4 * IN), Emu(IN)))
        ]
    );
    assert_eq!(group.position(), Some((Emu(IN), Emu(IN))));
    assert_eq!(group.size(), Some((Emu(5 * IN), Emu(IN))));
    let group_index = slide
        .shapes()
        .position(|shape| shape.kind() == ShapeKind::Group)
        .unwrap();

    // Moving and stretching the group moves and stretches what it shows.
    {
        let mut slide = prs.slide_mut(0).unwrap();
        let mut group = slide.shape_mut(group_index).unwrap();
        group.set_size(Emu(10 * IN), Emu(2 * IN)).unwrap();
        group.set_position(Emu(0), Emu(2 * IN)).unwrap();
    }
    let mut prs = reopen(&prs);
    let range = prs
        .shapes_mut(0, &[])
        .unwrap()
        .ungroup(group_index)
        .unwrap();
    assert_eq!(range.len(), 2);
    let prs = reopen(&prs);
    let mut shapes: Vec<_> = prs
        .slide(0)
        .unwrap()
        .shapes()
        .map(|shape| (placed(shape), shape.size().unwrap()))
        .collect();
    shapes.sort();
    assert_eq!(
        shapes,
        [
            (
                ("A".to_owned(), (Emu(0), Emu(2 * IN))),
                (Emu(4 * IN), Emu(2 * IN))
            ),
            (
                ("B".to_owned(), (Emu(6 * IN), Emu(2 * IN))),
                (Emu(4 * IN), Emu(2 * IN))
            ),
            (
                ("C".to_owned(), (Emu(7 * IN), Emu(IN))),
                (Emu(2 * IN), Emu(IN))
            )
        ]
    );
}

#[test]
fn new_shapes_get_ids_unused_on_the_slide_and_keep_a_name_set_by_the_user() {
    fn ids(shape: ShapeRef<'_>, out: &mut Vec<u32>) {
        out.push(shape.non_visual_id().unwrap());
        for child in shape.children() {
            ids(child, out);
        }
    }
    let mut prs = corpus_deck("powerpoint/deck.pptx");
    {
        let mut slide = prs.slide_mut(3).unwrap();
        slide
            .add_textbox(Emu(0), Emu(0), Emu(IN), Emu(IN))
            .unwrap()
            .set_name("Résumé – 概要")
            .unwrap();
        slide
            .add_shape("ellipse", Emu(0), Emu(0), Emu(IN), Emu(IN))
            .unwrap();
        slide
            .add_connector(ConnectorType::Elbow, Emu(0), Emu(0), Emu(IN), Emu(IN))
            .unwrap();
        slide.add_group_shape().unwrap();
    }
    let group = prs.slide(3).unwrap().shapes().len() - 1;
    prs.shapes_mut(3, &[group])
        .unwrap()
        .add_textbox(Emu(0), Emu(0), Emu(IN), Emu(IN))
        .unwrap();

    let prs = reopen(&prs);
    let slide = prs.slide(3).unwrap();
    let mut all = Vec::new();
    for shape in slide.shapes() {
        ids(shape, &mut all);
    }
    assert_eq!(all.len(), 3 + 5);
    let mut unique = all.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), all.len(), "ids {all:?}");
    assert_eq!(
        shape_at(&prs, 3, 3).non_visual_name().as_deref(),
        Some("Résumé – 概要")
    );
    for shape in slide.shapes() {
        assert!(
            shape.non_visual_name().is_some_and(|name| !name.is_empty()),
            "every shape has a name"
        );
    }
}

#[test]
fn moving_resizing_rotating_and_flipping_a_shape_survives_a_save() {
    let mut prs = text_box_deck("Moved");
    {
        let mut slide = prs.slide_mut(0).unwrap();
        let mut shape = slide.shape_mut(0).unwrap();
        shape.set_position(Emu(2 * IN), Emu(3 * IN)).unwrap();
        shape.set_size(Emu(4 * IN), Emu(IN / 2)).unwrap();
        shape.set_rotation(Angle::from_degrees(45.0)).unwrap();
        shape.set_flips(true, false).unwrap();
    }
    let mut prs = reopen(&prs);
    let shape = shape_at(&prs, 0, 0);
    assert_eq!(shape.position(), Some((Emu(2 * IN), Emu(3 * IN))));
    assert_eq!(shape.size(), Some((Emu(4 * IN), Emu(IN / 2))));
    assert_eq!(shape.rotation(), Some(Angle::from_degrees(45.0)));
    assert_eq!(shape.flips(), Some((true, false)));
    assert_eq!(shape.text().as_deref(), Some("Moved"));

    let before = prs.to_bytes().unwrap();
    assert!(
        prs.slide_mut(0)
            .unwrap()
            .add_textbox(Emu(0), Emu(0), Emu(-1), Emu(IN))
            .is_err(),
        "a negative width is refused"
    );
    assert!(prs.set_shape_size(0, &[0], Emu(IN), Emu(-1)).is_err());
    assert!(
        prs.slide_mut(0)
            .unwrap()
            .shape_mut(0)
            .unwrap()
            .set_size(Emu(-1), Emu(IN))
            .is_err()
    );
    assert_eq!(prs.to_bytes().unwrap(), before, "a refusal changes nothing");
}

#[test]
fn moving_a_shape_in_the_z_order_keeps_every_shape() {
    let mut prs = blank_slide_deck();
    {
        let mut slide = prs.slide_mut(0).unwrap();
        for text in ["A", "B", "C"] {
            slide
                .add_textbox(Emu(IN), Emu(IN), Emu(IN), Emu(IN))
                .unwrap()
                .set_text(text)
                .unwrap();
        }
    }
    prs.move_shape(0, 0, 2).unwrap();
    let mut prs = reopen(&prs);
    assert_eq!(shape_texts(&prs, 0), ["B", "C", "A"], "A is now on top");

    prs.move_shape(0, 2, 0).unwrap();
    assert_eq!(shape_texts(&prs, 0), ["A", "B", "C"]);
    assert!(prs.move_shape(0, 0, 3).is_err());
    assert!(prs.move_shape(0, 3, 0).is_err());
    let prs = reopen(&prs);
    assert_eq!(shape_texts(&prs, 0), ["A", "B", "C"]);
}

#[test]
fn removing_a_shape_keeps_the_others_in_order() {
    let mut prs = corpus_deck("powerpoint/deck.pptx");
    let before = shape_texts(&prs, 3);
    assert_eq!(before, ["Revenue by quarter", "Indexed, Q1 = 42"]);
    // The picture sits between the title and the text box.
    assert_eq!(shape_at(&prs, 3, 1).kind(), ShapeKind::Picture);
    prs.remove_shape(3, 2).unwrap();
    assert!(prs.remove_shape(3, 2).is_err(), "only two shapes are left");

    let prs = reopen(&prs);
    let slide = prs.slide(3).unwrap();
    let kinds: Vec<_> = slide.shapes().map(|shape| shape.kind()).collect();
    assert_eq!(kinds, [ShapeKind::Shape, ShapeKind::Picture]);
    assert_eq!(slide.text(), "Revenue by quarter");
    assert_eq!(
        prs.slide(1).unwrap().text(),
        corpus_deck("powerpoint/deck.pptx").slide(1).unwrap().text()
    );
}

// Text

#[test]
fn each_line_of_assigned_text_becomes_a_paragraph() {
    // A line feed or CRLF starts a paragraph, a vertical tab breaks the line.
    let mut prs = text_box_deck("one\ntwo\r\nthree\u{b}and a half");
    let frame = shape_at(&prs, 0, 0).text_frame().unwrap();
    assert_eq!(frame.paragraph_count(), 3);
    {
        let mut slide = prs.slide_mut(0).unwrap();
        let mut shape = slide.shape_mut(0).unwrap();
        let mut frame = shape.text_frame().unwrap();
        // A line feed in paragraph text breaks the line inside the paragraph.
        frame.add_paragraph().set_text("four\nfive");
    }
    let mut prs = reopen(&prs);
    let shape = shape_at(&prs, 0, 0);
    assert_eq!(
        run_texts(shape),
        [
            vec!["one"],
            vec!["two"],
            vec!["three", "and a half"],
            vec!["four", "five"]
        ]
    );
    assert_eq!(
        shape.text_frame().unwrap().paragraph(3).unwrap().text(),
        "four\u{b}five"
    );
    assert_eq!(
        shape.text().as_deref(),
        Some("one\ntwo\nthree\nand a half\nfour\nfive")
    );

    prs.slide_mut(0)
        .unwrap()
        .shape_mut(0)
        .unwrap()
        .set_text("")
        .unwrap();
    let prs = reopen(&prs);
    let frame = shape_at(&prs, 0, 0).text_frame().unwrap();
    assert_eq!(
        frame.paragraph_count(),
        1,
        "an empty text keeps one paragraph"
    );
    assert_eq!(frame.text(), "");
}

#[test]
fn paragraphs_and_runs_appended_by_hand_keep_their_order() {
    let mut prs = text_box_deck("one");
    {
        let mut slide = prs.slide_mut(0).unwrap();
        let mut shape = slide.shape_mut(0).unwrap();
        let mut frame = shape.text_frame().unwrap();
        frame.paragraph_mut(0).unwrap().add_run(" two");
        let mut second = frame.add_paragraph();
        second.add_run("three");
        second.add_run(" four");
        frame.add_paragraph();
        frame.add_paragraph().add_run("six");
    }
    let prs = reopen(&prs);
    let shape = shape_at(&prs, 0, 0);
    assert_eq!(
        run_texts(shape),
        [
            vec!["one", " two"],
            vec!["three", " four"],
            vec![],
            vec!["six"]
        ]
    );
    assert_eq!(shape.text().as_deref(), Some("one two\nthree four\n\nsix"));
}

#[test]
fn run_formatting_survives_a_save() {
    let mut prs = text_box_deck("plain");
    {
        let mut slide = prs.slide_mut(0).unwrap();
        let mut shape = slide.shape_mut(0).unwrap();
        let mut frame = shape.text_frame().unwrap();
        let mut paragraph = frame.paragraph_mut(0).unwrap();
        paragraph.add_run(" bold").set_properties(character(|p| {
            p.bold = Some(true);
            p.font_size = Some(2_400);
        }));
        paragraph.add_run(" italic").set_properties(character(|p| {
            p.italic = Some(true);
            p.underline = Some(TextUnderline::Single);
        }));
        let mut coloured = paragraph.add_run(" coloured");
        coloured.set_properties(character(|p| p.fill = Some(solid(0x1F, 0x4E, 0x79))));
        coloured.set_font(Some(TextFont::new("Georgia").unwrap()));
    }

    let prs = reopen(&prs);
    let frame = shape_at(&prs, 0, 0).text_frame().unwrap();
    let paragraph = frame.paragraph(0).unwrap();
    assert_eq!(paragraph.text(), "plain bold italic coloured");
    let plain = paragraph.run(0).unwrap();
    assert_eq!(plain.properties().and_then(|p| p.bold), None);
    let bold = paragraph.run(1).unwrap();
    assert_eq!(bold.properties().unwrap().bold, Some(true));
    assert_eq!(bold.font_size(), Some(2_400));
    let italic = paragraph.run(2).unwrap();
    assert_eq!(italic.properties().unwrap().italic, Some(true));
    assert_eq!(
        italic.properties().unwrap().underline,
        Some(TextUnderline::Single)
    );
    assert_eq!(italic.properties().unwrap().bold, None);
    let coloured = paragraph.run(3).unwrap();
    assert_eq!(coloured.font_color().as_deref(), Some("1F4E79"));
    assert_eq!(coloured.font_name(), Some("Georgia"));
}

#[test]
fn editing_text_keeps_the_formatting_of_the_run_it_replaces() {
    let mut prs = text_box_deck("first");
    {
        let mut slide = prs.slide_mut(0).unwrap();
        let mut shape = slide.shape_mut(0).unwrap();
        let mut frame = shape.text_frame().unwrap();
        let mut paragraph = frame.paragraph_mut(0).unwrap();
        paragraph.run_mut(0).unwrap().set_properties(character(|p| {
            p.bold = Some(true);
            p.font_size = Some(3_200);
        }));
    }
    let mut prs = reopen(&prs);
    {
        let mut slide = prs.slide_mut(0).unwrap();
        let mut shape = slide.shape_mut(0).unwrap();
        let mut frame = shape.text_frame().unwrap();
        frame
            .paragraph_mut(0)
            .unwrap()
            .run_mut(0)
            .unwrap()
            .set_text("run edit");
    }
    let mut prs = reopen(&prs);
    let run = |prs: &Presentation, paragraph: usize| {
        let frame = shape_at(prs, 0, 0).text_frame().unwrap();
        let run = frame.paragraph(paragraph).unwrap().run(0).unwrap();
        (
            run.text().to_owned(),
            run.properties().and_then(|p| p.bold),
            run.font_size(),
        )
    };
    assert_eq!(
        run(&prs, 0),
        ("run edit".to_owned(), Some(true), Some(3_200))
    );

    // Shape text: every new paragraph copies the first one's formatting.
    prs.slide_mut(0)
        .unwrap()
        .shape_mut(0)
        .unwrap()
        .set_text("shape edit\nsecond line")
        .unwrap();
    let prs = reopen(&prs);
    assert_eq!(
        run(&prs, 0),
        ("shape edit".to_owned(), Some(true), Some(3_200))
    );
    assert_eq!(
        run(&prs, 1),
        ("second line".to_owned(), Some(true), Some(3_200))
    );
}

#[test]
fn bullet_levels_and_bullet_styles_survive_a_save() {
    let mut prs = text_box_deck("Revenue\nSubscriptions\nNew tier\nNotes");
    {
        let mut slide = prs.slide_mut(0).unwrap();
        let mut shape = slide.shape_mut(0).unwrap();
        let mut frame = shape.text_frame().unwrap();
        let bullets = [
            TextBulletChoice::Character(TextBulletCharacter::new("•").unwrap()),
            TextBulletChoice::AutoNumber(TextAutoNumber::new(TextAutoNumberScheme::ArabicPeriod)),
            TextBulletChoice::Character(TextBulletCharacter::new("–").unwrap()),
            TextBulletChoice::None(TextNoBullet::default()),
        ];
        for (index, (level, bullet)) in [0, 1, 2, 0].into_iter().zip(bullets).enumerate() {
            let mut paragraph = frame.paragraph_mut(index).unwrap();
            assert!(paragraph.set_level(level));
            paragraph.set_bullet(Some(TextBullet {
                choice: Some(bullet),
                ..TextBullet::default()
            }));
        }
        let mut first = frame.paragraph_mut(0).unwrap();
        assert!(!first.set_level(9), "levels stop at 8");
    }

    let prs = reopen(&prs);
    let frame = shape_at(&prs, 0, 0).text_frame().unwrap();
    let levels: Vec<_> = (0..4)
        .map(|i| frame.paragraph(i).unwrap().level())
        .collect();
    assert_eq!(
        levels,
        [0, 1, 2, 0],
        "a refused level leaves the level as it was"
    );
    let bullet = |index: usize| {
        frame
            .paragraph(index)
            .unwrap()
            .properties()
            .and_then(|p| p.bullet.clone())
            .and_then(|bullet| bullet.choice)
    };
    assert!(matches!(bullet(0), Some(TextBulletChoice::Character(c)) if c.character == "•"));
    assert!(matches!(
        bullet(1),
        Some(TextBulletChoice::AutoNumber(n)) if n.scheme == TextAutoNumberScheme::ArabicPeriod
    ));
    assert!(matches!(bullet(2), Some(TextBulletChoice::Character(c)) if c.character == "–"));
    assert!(matches!(bullet(3), Some(TextBulletChoice::None(_))));
    assert_eq!(frame.text(), "Revenue\nSubscriptions\nNew tier\nNotes");
}

#[test]
fn paragraph_alignment_and_right_to_left_direction_survive_a_save() {
    let arabic = "مرحبا بالعالم، الربع الثالث";
    let hebrew = "שלום עולם, רבעון שלישי";
    let paragraphs = [
        (arabic, Some(true), TextAlignment::Right),
        (hebrew, Some(true), TextAlignment::Justified),
        ("Centred", None, TextAlignment::Center),
        ("Left", None, TextAlignment::Left),
    ];
    let lines: Vec<_> = paragraphs.iter().map(|(text, _, _)| *text).collect();
    let mut prs = text_box_deck(&lines.join("\n"));
    {
        let mut slide = prs.slide_mut(0).unwrap();
        let mut shape = slide.shape_mut(0).unwrap();
        let mut frame = shape.text_frame().unwrap();
        for (index, (_, right_to_left, alignment)) in paragraphs.iter().enumerate() {
            let mut paragraph = frame.paragraph_mut(index).unwrap();
            paragraph.set_properties(self::paragraph(|p| {
                p.right_to_left = *right_to_left;
                p.alignment = Some(*alignment);
            }));
            if right_to_left.is_some() {
                paragraph.run_mut(0).unwrap().set_properties(character(|p| {
                    p.complex_script = Some(TextFont::new("Arial").unwrap());
                }));
            }
        }
    }

    let prs = reopen(&prs);
    let frame = shape_at(&prs, 0, 0).text_frame().unwrap();
    for (index, (text, right_to_left, alignment)) in paragraphs.iter().enumerate() {
        let paragraph = frame.paragraph(index).unwrap();
        let properties = paragraph.properties().unwrap();
        assert_eq!(paragraph.text(), *text);
        assert_eq!(properties.right_to_left, *right_to_left, "{text}");
        assert_eq!(properties.alignment, Some(*alignment), "{text}");
    }
    let run = frame.paragraph(0).unwrap().run(0).unwrap();
    let script = run.properties().unwrap().complex_script.as_ref();
    assert_eq!(script.map(|font| font.typeface.as_str()), Some("Arial"));
}

#[test]
fn cjk_emoji_and_combining_text_round_trips_unchanged() {
    let lines = [
        "四半期の業績は好調です",
        "분기 실적 요약",
        "季度业绩：收入增长 13%",
        "👩‍💻 Launch 🚀 ✓",
        "Cafe\u{301} na\u{ef}ve",
    ];
    let mut prs = text_box_deck(&lines.join("\n"));
    prs.slide_mut(0)
        .unwrap()
        .shape_mut(0)
        .unwrap()
        .text_frame()
        .unwrap()
        .paragraph_mut(0)
        .unwrap()
        .run_mut(0)
        .unwrap()
        .set_properties(character(|p| {
            p.east_asian = Some(TextFont::new("MS Gothic").unwrap());
        }));
    let mut prs = reopen(&prs);
    let shape = shape_at(&prs, 0, 0);
    assert_eq!(shape.text(), Some(lines.join("\n")));
    let run = shape
        .text_frame()
        .unwrap()
        .paragraph(0)
        .unwrap()
        .run(0)
        .unwrap();
    assert_eq!(
        run.properties()
            .unwrap()
            .east_asian
            .as_ref()
            .map(|f| f.typeface.as_str()),
        Some("MS Gothic")
    );

    assert_eq!(prs.replace_text("業績", "決算"), 1);
    assert_eq!(prs.replace_text("🚀", "🛰️"), 1);
    let prs = reopen(&prs);
    let text = shape_at(&prs, 0, 0).text().unwrap();
    assert!(text.starts_with("四半期の決算は好調です\n"), "{text}");
    assert!(text.contains("👩‍💻 Launch 🛰️ ✓"), "{text}");
}

#[test]
fn markup_characters_stay_literal_and_control_characters_are_escaped() {
    let mut prs = text_box_deck("<b>Tom & \"Jerry\"</b> 'quoted'\tafter a tab");
    let saved = reopen(&prs);
    assert_eq!(
        shape_at(&saved, 0, 0).text().as_deref(),
        Some("<b>Tom & \"Jerry\"</b> 'quoted'\tafter a tab")
    );

    // A character XML 1.0 cannot carry is stored as _xHHHH_, as python-pptx does.
    prs.slide_mut(0)
        .unwrap()
        .shape_mut(0)
        .unwrap()
        .set_text("bell\u{7}here")
        .unwrap();
    assert_eq!(prs.replace_text("here", "and\u{1}there"), 1);
    let prs = reopen(&prs);
    assert_eq!(
        shape_at(&prs, 0, 0).text().as_deref(),
        Some("bell_x0007_and_x0001_there")
    );
}

#[test]
fn a_shape_name_with_a_tab_or_a_line_feed_survives_a_save() {
    let mut prs = text_box_deck("Named");
    {
        let mut slide = prs.slide_mut(0).unwrap();
        let mut shape = slide.shape_mut(0).unwrap();
        shape.set_alt_text(Some("Revenue\tQ3\nup 13 %")).unwrap();
        shape.set_name("Revenue\tQ3\nchart").unwrap();
    }
    assert_eq!(
        shape_at(&prs, 0, 0).non_visual_name().as_deref(),
        Some("Revenue\tQ3\nchart")
    );
    let prs = reopen(&prs);
    let shape = shape_at(&prs, 0, 0);
    assert_eq!(
        shape.alt_text().as_deref(),
        Some("Revenue\tQ3\nup 13 %"),
        "the alt text keeps its tab and line feed"
    );
    assert_eq!(
        shape.non_visual_name().as_deref(),
        Some("Revenue\tQ3\nchart"),
        "the name keeps its tab and line feed"
    );
}

#[test]
fn a_carriage_return_in_run_text_is_saved_so_that_other_readers_keep_it() {
    let mut prs = text_box_deck("before");
    prs.slide_mut(0)
        .unwrap()
        .shape_mut(0)
        .unwrap()
        .text_frame()
        .unwrap()
        .paragraph_mut(0)
        .unwrap()
        .run_mut(0)
        .unwrap()
        .set_text("line\rend");
    let prs = reopen(&prs);
    assert_eq!(shape_at(&prs, 0, 0).text().as_deref(), Some("line\rend"));
    // The facade reads its own output back; a conforming XML reader, such as
    // PowerPoint's or python-pptx's, normalises a raw carriage return.
    let slide = saved_part(&prs, "/ppt/slides/slide1.xml");
    assert!(
        !slide.contains('\r'),
        "the carriage return is escaped in the saved slide"
    );
}

#[test]
fn every_kind_of_shape_keeps_a_name_with_a_tab_or_a_line_feed() {
    const NAME: &str = "Flow\tA\nB";
    let mut prs = blank_slide_deck();
    {
        let mut slide = prs.slide_mut(0).unwrap();
        slide.add_textbox(Emu(0), Emu(0), Emu(IN), Emu(IN)).unwrap();
        slide
            .add_shape("ellipse", Emu(0), Emu(0), Emu(IN), Emu(IN))
            .unwrap();
        slide
            .add_connector(ConnectorType::Straight, Emu(0), Emu(0), Emu(IN), Emu(IN))
            .unwrap();
        slide.add_group_shape().unwrap();
        slide
            .add_table(2, 2, Emu(0), Emu(0), Emu(2 * IN), Emu(IN))
            .unwrap();
    }
    let count = prs.slide(0).unwrap().shapes().count();
    assert_eq!(count, 5);
    for index in 0..count {
        prs.slide_mut(0)
            .unwrap()
            .shape_mut(index)
            .unwrap()
            .set_name(NAME)
            .unwrap();
    }

    let saved = reopen(&prs);
    for index in 0..count {
        assert_eq!(
            shape_at(&saved, 0, index).non_visual_name().as_deref(),
            Some(NAME),
            "shape {index}"
        );
    }
    let slide = saved_part(&saved, "/ppt/slides/slide1.xml");
    assert_eq!(slide.matches("Flow&#9;A&#10;B").count(), count, "{slide}");
}

#[test]
fn tabs_line_feeds_and_carriage_returns_in_names_and_runs_survive_on_every_editors_deck() {
    const NAME: &str = "Highlights\tbody\nQ3";
    const TEXT: &str = "Revenue\tup\rQ3";
    for name in [
        "powerpoint/deck.pptx",
        "google/deck.pptx",
        "libreoffice/deck.pptx",
        "authored/deck.pptx",
    ] {
        let mut prs = corpus_deck(name);
        let index = prs
            .slide(1)
            .unwrap()
            .shapes()
            .position(|shape| {
                shape.text_frame().is_some_and(|frame| {
                    frame
                        .paragraph(0)
                        .is_some_and(|paragraph| paragraph.run_count() > 0)
                })
            })
            .expect("the Highlights slide has a shape with a run");
        {
            let mut slide = prs.slide_mut(1).unwrap();
            let mut shape = slide.shape_mut(index).unwrap();
            shape.set_name(NAME).unwrap();
            shape
                .text_frame()
                .unwrap()
                .paragraph_mut(0)
                .unwrap()
                .run_mut(0)
                .unwrap()
                .set_text(TEXT);
        }

        let saved = reopen(&prs);
        let shape = shape_at(&saved, 1, index);
        assert_eq!(shape.non_visual_name().as_deref(), Some(NAME), "{name}");
        assert_eq!(run_texts(shape)[0][0], TEXT, "{name}");
        // What another reader sees: no raw tab, line feed or carriage return
        // inside an attribute, no raw carriage return anywhere.
        let slide = (1..=saved.len())
            .map(|number| saved_part(&saved, &format!("/ppt/slides/slide{number}.xml")))
            .find(|xml| xml.contains("Highlights&#9;body&#10;Q3"))
            .unwrap_or_else(|| panic!("{name}: the name is saved escaped"));
        assert!(slide.contains("Revenue\tup&#13;Q3"), "{name}");
        assert!(!slide.contains('\r'), "{name}");
    }
}

#[test]
fn a_raw_carriage_return_in_a_saved_run_reads_as_a_line_feed_and_stays_one_after_an_edit() {
    // A part written with Windows line ends holds a raw CR LF inside a:t,
    // which every XML reader reads as one line feed.
    let prs = text_box_deck("Dear {name},");
    let mut package = OpcPackage::from_reader(Cursor::new(prs.to_bytes().unwrap())).unwrap();
    let slide = String::from_utf8(package.get_part("/ppt/slides/slide1.xml").unwrap().to_vec())
        .unwrap()
        .replacen("Dear {name},</a:t>", "Dear {name},\r\nWelcome</a:t>", 1);
    package.set_part("/ppt/slides/slide1.xml", slide.into_bytes());
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    let mut prs = Presentation::from_bytes(&bytes.into_inner()).unwrap();
    assert_eq!(
        run_texts(shape_at(&prs, 0, 0))[0][0],
        "Dear {name},\nWelcome"
    );

    assert_eq!(prs.replace_text("{name}", "Ada"), 1);

    let saved = reopen(&prs);
    assert_eq!(
        run_texts(shape_at(&saved, 0, 0))[0][0],
        "Dear Ada,\nWelcome"
    );
    let xml = saved_part(&saved, "/ppt/slides/slide1.xml");
    assert!(!xml.contains("&#13;") && !xml.contains('\r'), "{xml}");
}

#[test]
fn raw_line_ends_in_cdata_run_text_read_as_line_feeds() {
    let prs = text_box_deck("Dear reader,");
    let mut package = OpcPackage::from_reader(Cursor::new(prs.to_bytes().unwrap())).unwrap();
    let slide = String::from_utf8(package.get_part("/ppt/slides/slide1.xml").unwrap().to_vec())
        .unwrap()
        .replacen(
            "Dear reader,</a:t>",
            "<![CDATA[Dear reader,\r\nWelcome\rin]]></a:t>",
            1,
        );
    package.set_part("/ppt/slides/slide1.xml", slide.into_bytes());
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    let prs = Presentation::from_bytes(&bytes.into_inner()).unwrap();

    assert_eq!(
        run_texts(shape_at(&prs, 0, 0))[0][0],
        "Dear reader,\nWelcome\nin"
    );
}

#[test]
fn wrap_autofit_anchor_and_insets_survive_a_save_and_can_be_cleared() {
    let mut prs = text_box_deck("Framed");
    {
        let mut slide = prs.slide_mut(0).unwrap();
        let mut shape = slide.shape_mut(0).unwrap();
        let mut frame = shape.text_frame().unwrap();
        frame.set_word_wrap(Some(false));
        frame.set_autofit_mode(Some(AutofitMode::Shape));
        frame.set_vertical_anchor(Some(TextAnchor::Bottom));
        frame
            .set_insets(Some(Emu(0)), Some(Emu(45_720)), None, Some(Emu(12_700)))
            .unwrap();
        let outside = Emu(i64::from(i32::MAX) + 1);
        assert!(frame.set_insets(None, None, None, Some(outside)).is_err());
    }
    let mut prs = reopen(&prs);
    let frame = shape_at(&prs, 0, 0).text_frame().unwrap();
    assert_eq!(frame.word_wrap(), Some(false));
    assert_eq!(frame.autofit_mode(), Some(AutofitMode::Shape));
    assert_eq!(frame.vertical_anchor(), Some(TextAnchor::Bottom));
    assert_eq!(
        frame.insets(),
        (Some(Emu(0)), Some(Emu(45_720)), None, Some(Emu(12_700))),
        "a refused inset leaves the insets as they were"
    );

    {
        let mut slide = prs.slide_mut(0).unwrap();
        let mut shape = slide.shape_mut(0).unwrap();
        let mut frame = shape.text_frame().unwrap();
        frame.set_word_wrap(Some(true));
        frame.set_autofit_mode(Some(AutofitMode::Normal));
    }
    let mut prs = reopen(&prs);
    let frame = shape_at(&prs, 0, 0).text_frame().unwrap();
    assert_eq!(frame.word_wrap(), Some(true));
    assert_eq!(frame.autofit_mode(), Some(AutofitMode::Normal));

    {
        let mut slide = prs.slide_mut(0).unwrap();
        let mut shape = slide.shape_mut(0).unwrap();
        let mut frame = shape.text_frame().unwrap();
        frame.set_word_wrap(None);
        frame.set_autofit_mode(None);
        frame.set_vertical_anchor(None);
        frame.set_insets(None, None, None, None).unwrap();
    }
    let prs = reopen(&prs);
    let frame = shape_at(&prs, 0, 0).text_frame().unwrap();
    assert_eq!(frame.word_wrap(), None);
    assert_eq!(frame.autofit_mode(), None);
    assert_eq!(frame.vertical_anchor(), None);
    assert_eq!(frame.insets(), (None, None, None, None));
    assert_eq!(frame.text(), "Framed");
}

// Find and replace

/// Two slides holding "target" in a text box (twice in one run), a nested
/// group member, a table cell, and an auto shape on the second slide.
fn replace_deck() -> Presentation {
    let mut prs = blank_slide_deck();
    prs.add_slide(prs.slide_layout_index(0).unwrap()).unwrap();
    {
        let mut slide = prs.slide_mut(0).unwrap();
        slide
            .add_textbox(Emu(IN), Emu(IN), Emu(4 * IN), Emu(IN))
            .unwrap()
            .set_text("target and target")
            .unwrap();
        slide.add_group_shape().unwrap();
        slide
            .add_table(1, 2, Emu(IN), Emu(4 * IN), Emu(4 * IN), Emu(IN))
            .unwrap()
            .into_table_mut()
            .unwrap()
            .cell_mut(0, 1)
            .unwrap()
            .set_text("cell target");
    }
    prs.shapes_mut(0, &[1])
        .unwrap()
        .add_textbox(Emu(IN), Emu(2 * IN), Emu(4 * IN), Emu(IN))
        .unwrap()
        .set_text("grouped target")
        .unwrap();
    prs.slide_mut(1)
        .unwrap()
        .add_shape("rect", Emu(IN), Emu(IN), Emu(2 * IN), Emu(IN))
        .unwrap()
        .set_text("second slide target")
        .unwrap();
    prs
}

#[test]
fn replace_text_counts_every_match_across_slides_groups_and_tables() {
    let mut prs = replace_deck();
    assert_eq!(prs.replace_text("target", "goal"), 5);
    assert_eq!(
        prs.replace_text("target", "goal"),
        0,
        "nothing is left to replace"
    );
    assert_eq!(
        prs.replace_text("Target", "goal"),
        0,
        "matching is case sensitive"
    );

    let prs = reopen(&prs);
    assert_eq!(
        slide_texts(&prs),
        [
            "goal and goal\ngrouped goal\n\tcell goal",
            "second slide goal"
        ]
    );
}

#[test]
fn a_match_split_across_runs_is_replaced_in_the_first_run_with_its_formatting() {
    let mut prs = text_box_deck("Total: tar");
    {
        let mut slide = prs.slide_mut(0).unwrap();
        let mut shape = slide.shape_mut(0).unwrap();
        let mut frame = shape.text_frame().unwrap();
        let mut paragraph = frame.paragraph_mut(0).unwrap();
        paragraph
            .run_mut(0)
            .unwrap()
            .set_properties(character(|p| p.bold = Some(true)));
        paragraph
            .add_run("get reached")
            .set_properties(character(|p| p.italic = Some(true)));
    }
    assert_eq!(prs.replace_text("target", "goal"), 1);

    let prs = reopen(&prs);
    let shape = shape_at(&prs, 0, 0);
    assert_eq!(run_texts(shape), [vec!["Total: goal", " reached"]]);
    let paragraph = shape.text_frame().unwrap().paragraph(0).unwrap();
    assert_eq!(
        paragraph.run(0).unwrap().properties().unwrap().bold,
        Some(true)
    );
    assert_eq!(
        paragraph.run(1).unwrap().properties().unwrap().italic,
        Some(true)
    );
}

#[test]
fn scoped_replacements_touch_only_their_text_frame_or_slide() {
    let mut prs = replace_deck();
    {
        let mut slide = prs.slide_mut(0).unwrap();
        let mut shape = slide.shape_mut(0).unwrap();
        let mut frame = shape.text_frame().unwrap();
        // A count other than the expected one leaves the frame as it was.
        assert_eq!(
            frame.try_replace_text("target", "goal", Some(1)).unwrap(),
            2
        );
        assert_eq!(frame.text(), "target and target");
        assert_eq!(
            frame.try_replace_text("target", "goal", Some(2)).unwrap(),
            2
        );
        assert_eq!(frame.try_replace_text("absent", "goal", None).unwrap(), 0);
    }
    let mut prs = reopen(&prs);
    assert_eq!(
        slide_texts(&prs),
        [
            "goal and goal\ngrouped target\n\tcell target",
            "second slide target"
        ]
    );

    let before = prs.to_bytes().unwrap();
    assert_eq!(
        prs.try_replace_slide_text(0, "target", "goal", false, Some(1))
            .unwrap(),
        2,
        "the count is returned for the caller to compare"
    );
    assert_eq!(
        prs.to_bytes().unwrap(),
        before,
        "a count mismatch changes nothing"
    );
    assert!(matches!(
        prs.try_replace_slide_text(2, "target", "goal", false, None),
        Err(Error::UnknownSlideIndex { .. })
    ));
    assert_eq!(
        prs.try_replace_slide_text(1, "target", "goal", false, None)
            .unwrap(),
        1
    );
    let prs = reopen(&prs);
    assert_eq!(
        slide_texts(&prs),
        [
            "goal and goal\ngrouped target\n\tcell target",
            "second slide goal"
        ]
    );
}

#[test]
fn an_empty_search_text_changes_nothing() {
    let mut prs = replace_deck();
    let before = prs.to_bytes().unwrap();
    assert_eq!(prs.replace_text("", "goal"), 0);
    assert!(prs.try_replace_text("", "goal").is_err());
    assert!(
        prs.try_replace_slide_text(0, "", "goal", true, None)
            .is_err()
    );
    assert!(
        prs.try_replace_all_expected(&[("target", "goal", None), ("", "x", None)])
            .is_err()
    );
    {
        let mut slide = prs.slide_mut(0).unwrap();
        let mut shape = slide.shape_mut(0).unwrap();
        assert!(
            shape
                .text_frame()
                .unwrap()
                .try_replace_text("", "goal", None)
                .is_err()
        );
    }
    assert_eq!(prs.to_bytes().unwrap(), before);
}

#[test]
fn ordered_replacements_with_expected_counts_apply_all_or_nothing() {
    let mut prs = replace_deck();
    let before = prs.to_bytes().unwrap();
    let mismatch = prs
        .try_replace_all_expected(&[("target", "goal", Some(5)), ("goal", "aim", Some(4))])
        .unwrap()
        .expect_err("the second pair finds five, not four");
    assert_eq!(
        (mismatch.index, mismatch.expected, mismatch.found),
        (1, 4, 5)
    );
    assert_eq!(
        prs.to_bytes().unwrap(),
        before,
        "a mismatch changes nothing"
    );

    // Each pair counts on the result of the pairs before it.
    let counts = prs
        .try_replace_all_expected(&[
            ("target", "goal", Some(5)),
            ("goal", "aim", None),
            ("second slide", "last slide", Some(1)),
        ])
        .unwrap()
        .unwrap();
    assert_eq!(counts, [5, 5, 1]);
    assert_eq!(prs.try_replace_text("aim", "goal").unwrap(), 5);
    let prs = reopen(&prs);
    assert_eq!(
        slide_texts(&prs),
        [
            "goal and goal\ngrouped goal\n\tcell goal",
            "last slide goal"
        ]
    );
}

// Hyperlinks

/// The relationship id of a run's click hyperlink.
fn run_link(prs: &Presentation, paragraph: usize, run: usize) -> Option<String> {
    shape_at(prs, 0, 0)
        .text_frame()?
        .paragraph(paragraph)?
        .run(run)?
        .properties()?
        .hyperlink_click
        .as_ref()?
        .relationship_id
        .clone()
}

#[test]
fn a_run_hyperlink_survives_a_save_and_resolves_to_its_address() {
    let mut prs = text_box_deck("Read the report");
    prs.slide_mut(0)
        .unwrap()
        .shape_mut(0)
        .unwrap()
        .text_frame()
        .unwrap()
        .paragraph_mut(0)
        .unwrap()
        .add_run(" online");
    let id = shape_at(&prs, 0, 0).non_visual_id().unwrap();
    let address = "https://example.com/report?q=1&lang=fr#summary";
    prs.set_run_hyperlink(0, id, 0, 0, Some(address)).unwrap();
    prs.set_run_hyperlink(0, id, 0, 1, Some(address)).unwrap();

    let mut prs = reopen(&prs);
    let first = run_link(&prs, 0, 0).expect("the first run has a link");
    assert_eq!(
        run_link(&prs, 0, 1),
        Some(first.clone()),
        "two runs to one address share a relationship"
    );
    assert_eq!(prs.hyperlink_address(0, &first), Some(address));
    assert_eq!(
        shape_at(&prs, 0, 0).text().as_deref(),
        Some("Read the report online")
    );

    prs.set_run_hyperlink(0, id, 0, 0, Some("https://example.org/"))
        .unwrap();
    prs.set_run_hyperlink(0, id, 0, 1, None).unwrap();
    let prs = reopen(&prs);
    let retargeted = run_link(&prs, 0, 0).unwrap();
    assert_eq!(
        prs.hyperlink_address(0, &retargeted),
        Some("https://example.org/")
    );
    assert_eq!(run_link(&prs, 0, 1), None);
    assert_eq!(
        prs.hyperlink_address(0, &first),
        None,
        "a relationship no run uses any more is removed"
    );
}

#[test]
fn an_invalid_hyperlink_address_or_a_missing_run_is_refused_without_change() {
    let mut prs = text_box_deck("Link me");
    let id = shape_at(&prs, 0, 0).non_visual_id().unwrap();
    let before = prs.to_bytes().unwrap();
    for address in [
        "",
        "https://example.com/\nnext",
        "https://example.com/\u{1}",
    ] {
        assert!(matches!(
            prs.set_run_hyperlink(0, id, 0, 0, Some(address)),
            Err(Error::InvalidShapeMutation { .. })
        ));
    }
    for (slide, shape, paragraph, run) in [
        (1, id, 0, 0),
        (0, id + 1, 0, 0),
        (0, id, 1, 0),
        (0, id, 0, 1),
    ] {
        assert!(
            prs.set_run_hyperlink(slide, shape, paragraph, run, Some("https://example.com/"))
                .is_err(),
            "slide {slide}, shape {shape}, paragraph {paragraph}, run {run}"
        );
    }
    assert_eq!(prs.to_bytes().unwrap(), before);
}

#[test]
fn a_run_in_a_group_can_jump_to_another_slide() {
    let mut prs = blank_slide_deck();
    prs.add_slide(prs.slide_layout_index(0).unwrap()).unwrap();
    prs.slide_mut(0).unwrap().add_group_shape().unwrap();
    prs.shapes_mut(0, &[0])
        .unwrap()
        .add_textbox(Emu(IN), Emu(IN), Emu(2 * IN), Emu(IN))
        .unwrap()
        .set_text("Go to the appendix")
        .unwrap();
    let id = shape_at(&prs, 0, 0)
        .child(0)
        .unwrap()
        .non_visual_id()
        .unwrap();
    prs.set_run_target_slide(0, id, 0, 0, Some(1)).unwrap();
    assert!(prs.set_run_target_slide(0, id, 0, 0, Some(2)).is_err());

    let mut prs = reopen(&prs);
    assert_eq!(prs.run_target_slide(0, id, 0, 0).unwrap(), Some(1));
    prs.set_run_target_slide(0, id, 0, 0, None).unwrap();
    let prs = reopen(&prs);
    assert_eq!(prs.run_target_slide(0, id, 0, 0).unwrap(), None);
    assert_eq!(slide_texts(&prs)[0], "Go to the appendix");
}

// Corpus decks

const PRODUCERS: [&str; 4] = [
    "powerpoint/deck.pptx",
    "google/deck.pptx",
    "libreoffice/deck.pptx",
    "authored/deck.pptx",
];

/// The body placeholder of the "Highlights" slide: its second shape.
fn highlights_body(prs: &Presentation) -> ShapeRef<'_> {
    shape_at(prs, 1, 1)
}

#[test]
fn the_highlights_bullets_read_on_two_levels_for_every_producer() {
    for name in PRODUCERS {
        let prs = corpus_deck(name);
        let slide = prs.slide(1).unwrap();
        assert_eq!(
            slide.title().and_then(|title| title.text()).as_deref(),
            Some("Highlights"),
            "{name}"
        );
        let frame = highlights_body(&prs).text_frame().unwrap();
        let bullets: Vec<_> = (0..frame.paragraph_count())
            .map(|index| {
                let paragraph = frame.paragraph(index).unwrap();
                (paragraph.level(), paragraph.text())
            })
            .collect();
        assert_eq!(
            bullets,
            [
                (0, "Subscription revenue up 13 %".to_owned()),
                (1, "New tier adopted by 18 % of customers".to_owned()),
                (0, "Churn at 2.1 % per month".to_owned()),
                (0, "Lyon warehouse open".to_owned()),
            ],
            "{name}"
        );
    }
}

#[test]
fn editing_corpus_titles_and_bullets_keeps_placeholders_levels_and_formatting() {
    for name in ["powerpoint/deck.pptx", "google/deck.pptx"] {
        let original = corpus_deck(name);
        let mut prs = original.clone();
        {
            let mut slide = prs.slide_mut(1).unwrap();
            slide
                .shape_mut(0)
                .unwrap()
                .set_text("Faits marquants")
                .unwrap();
            let mut body = slide.shape_mut(1).unwrap();
            let mut frame = body.text_frame().unwrap();
            frame
                .paragraph_mut(1)
                .unwrap()
                .set_text("Nouvelle offre : 18 % des clients");
            let mut added = frame.add_paragraph();
            added.set_text("Entrepôt de Lille en 2027");
            assert!(added.set_level(1));
        }

        let prs = reopen(&prs);
        let title = shape_at(&prs, 1, 0);
        assert_eq!(title.placeholder_type(), Some("title"), "{name}");
        assert_eq!(title.text().as_deref(), Some("Faits marquants"), "{name}");
        let before = shape_at(&original, 1, 0).text_frame().unwrap();
        let after = title.text_frame().unwrap();
        let formatting = |frame: &TextFrameRef<'_>| {
            let run = frame.paragraph(0).unwrap().run(0).unwrap();
            (run.font_name().map(str::to_owned), run.font_size())
        };
        assert_eq!(formatting(&after), formatting(&before), "{name}");
        let settings = |frame: &TextFrameRef<'_>| {
            (
                frame.autofit_mode(),
                frame.word_wrap(),
                frame.vertical_anchor(),
                frame.insets(),
            )
        };
        assert_eq!(settings(&after), settings(&before), "{name}");

        let body = highlights_body(&prs);
        assert_eq!(
            body.placeholder_idx(),
            highlights_body(&original).placeholder_idx()
        );
        let frame = body.text_frame().unwrap();
        assert_eq!(
            settings(&frame),
            settings(&highlights_body(&original).text_frame().unwrap()),
            "{name}"
        );
        let bullets: Vec<_> = (0..frame.paragraph_count())
            .map(|index| {
                let paragraph = frame.paragraph(index).unwrap();
                (paragraph.level(), paragraph.text())
            })
            .collect();
        assert_eq!(
            bullets,
            [
                (0, "Subscription revenue up 13 %".to_owned()),
                (1, "Nouvelle offre : 18 % des clients".to_owned()),
                (0, "Churn at 2.1 % per month".to_owned()),
                (0, "Lyon warehouse open".to_owned()),
                (1, "Entrepôt de Lille en 2027".to_owned()),
            ],
            "{name}"
        );
    }
}

#[test]
fn the_corpus_text_box_keeps_its_autofit_and_run_size_after_an_edit() {
    for name in PRODUCERS {
        let mut prs = corpus_deck(name);
        let index = prs.slide(3).unwrap().shapes().len() - 1;
        let size = shape_at(&prs, 3, index).size();
        // LibreOffice writes its text box without the text-box flag.
        let kind = if name.starts_with("libreoffice") {
            ShapeType::AutoShape
        } else {
            ShapeType::TextBox
        };
        assert_eq!(shape_at(&prs, 3, index).shape_type(), Some(kind), "{name}");
        assert_eq!(
            shape_at(&prs, 3, index).text().as_deref(),
            Some("Indexed, Q1 = 42")
        );
        prs.slide_mut(3)
            .unwrap()
            .shape_mut(index)
            .unwrap()
            .set_text("Indexed, Q1 = 100")
            .unwrap();

        let prs = reopen(&prs);
        let shape = shape_at(&prs, 3, index);
        assert_eq!(shape.text().as_deref(), Some("Indexed, Q1 = 100"), "{name}");
        let frame = shape.text_frame().unwrap();
        assert_eq!(frame.autofit_mode(), Some(AutofitMode::Shape), "{name}");
        assert_eq!(
            frame.paragraph(0).unwrap().run(0).unwrap().font_size(),
            Some(1_200),
            "{name}"
        );
        assert_eq!(shape.size(), size, "{name}");
    }
}

#[test]
fn replacing_a_word_finds_the_same_matches_in_every_corpus_deck() {
    for name in PRODUCERS {
        let mut prs = corpus_deck(name);
        // The subtitle and the table header.
        assert_eq!(prs.try_replace_text("Q3", "Q4").unwrap(), 2, "{name}");
        let prs = reopen(&prs);
        let texts = slide_texts(&prs);
        assert_eq!(
            texts[0], "Quarterly Operations Review\nNorthwind Analytics, Q4",
            "{name}"
        );
        assert!(
            texts[2].starts_with("Key figures\nIndicator\tQ2\tQ4\n"),
            "{name}: {}",
            texts[2]
        );
        assert!(texts.iter().all(|text| !text.contains("Q3")), "{name}");
    }
}
