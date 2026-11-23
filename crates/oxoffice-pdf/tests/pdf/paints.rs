//! Images and gradients: the passes that register them and the content
//! stream that draws them agree on which element is which, whatever the
//! page holds before it.

use oxoffice_layout::{
    Color, GradientStop, MediaId, MultilingualGlyphRun, Paint, Path, PathElement, Point,
    PositionedElement, Rect, Stroke, TextDirection, TextScript,
};

use crate::support::{FONT, Pdf, contains, layout, run};

const ORIGIN: Point = Point { x: 72.0, y: 72.0 };

/// The two runs that draw nothing: a run and a multilingual run with no
/// text and no glyphs.
fn empty_runs() -> [(&'static str, PositionedElement); 2] {
    [
        ("an empty run", run("", ORIGIN, 12.0)),
        (
            "an empty multilingual run",
            PositionedElement::MultilingualText(MultilingualGlyphRun {
                origin: ORIGIN,
                font_id: FONT,
                font_size: 12.0,
                glyph_ids: Vec::new(),
                x_advances: Vec::new(),
                y_advances: Vec::new(),
                x_offsets: Vec::new(),
                y_offsets: Vec::new(),
                clusters: Vec::new(),
                logical_text: String::new(),
                logical_index: 0,
                source: None,
                script: TextScript::Latin,
                language: None,
                direction: TextDirection::LeftToRight,
                bidi_level: 0,
                color: Color::BLACK,
                bold: false,
                italic: false,
                field_kind: None,
                field_source: None,
                note: None,
                note_reference_source: None,
            }),
        ),
    ]
}

const BOX: Rect = Rect {
    x: 100.0,
    y: 200.0,
    width: 120.0,
    height: 80.0,
};

fn linear(from: Color, to: Color) -> Paint {
    Paint::Linear {
        start: Point { x: BOX.x, y: BOX.y },
        end: Point {
            x: BOX.x + BOX.width,
            y: BOX.y,
        },
        stops: vec![
            GradientStop {
                offset: 0.0,
                color: from,
            },
            GradientStop {
                offset: 1.0,
                color: to,
            },
        ],
        extend: (true, true),
    }
}

/// A box filled and stroked with two different gradients.
fn gradient_box() -> PositionedElement {
    let red = Color::from_hex("FF0000");
    let blue = Color::from_hex("0000FF");
    PositionedElement::Path(PathElement {
        path: Path::rect(BOX),
        fill: Some(linear(red, blue)),
        stroke: Some(Stroke::new(linear(blue, red), 2.0)),
    })
}

fn picture() -> PositionedElement {
    let pixmap = tiny_skia::Pixmap::new(2, 2).unwrap();
    PositionedElement::Image {
        rect: BOX,
        data: pixmap.encode_png().unwrap(),
        content_type: "image/png".to_owned(),
        media_id: MediaId(1),
    }
}

/// The names of a page's resources of one kind (`Pattern`, `XObject`).
fn resource_names(read: &Pdf, kind: &[u8]) -> Vec<Vec<u8>> {
    let page = read
        .doc
        .get_object(read.page_ids()[0])
        .unwrap()
        .as_dict()
        .unwrap();
    let resources = read.dict(page, b"Resources");
    resources
        .get(kind)
        .map(|names| {
            read.resolve(names)
                .as_dict()
                .unwrap()
                .iter()
                .map(|(name, _)| name.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// How many times `needle` occurs in `haystack`.
fn occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

#[test]
fn a_gradient_path_after_an_empty_run_keeps_its_fill_and_stroke_gradients() {
    for (what, empty) in empty_runs() {
        let read = Pdf::read(&oxoffice_pdf::render_to_pdf(&layout(vec![vec![
            empty,
            gradient_box(),
        ]])));
        let patterns = resource_names(&read, b"Pattern");
        assert_eq!(patterns.len(), 2, "after {what}: {patterns:?}");
        let content = read.page_content(0);
        let fill = [b"/".as_slice(), &patterns[0], b" scn"].concat();
        let stroke = [b"/".as_slice(), &patterns[1], b" SCN"].concat();
        assert!(
            contains(&content, &fill) && contains(&content, &stroke),
            "after {what}, the box lost a gradient:\n{}",
            String::from_utf8_lossy(&content)
        );
    }
}

#[test]
fn an_image_after_an_empty_run_is_drawn() {
    for (what, empty) in empty_runs() {
        let read = Pdf::read(&oxoffice_pdf::render_to_pdf(&layout(vec![vec![
            empty,
            picture(),
        ]])));
        let images = resource_names(&read, b"XObject");
        assert_eq!(images.len(), 1, "after {what}: {images:?}");
        let content = read.page_content(0);
        let drawn = [b"/".as_slice(), &images[0], b" Do"].concat();
        assert!(
            contains(&content, &drawn),
            "after {what}, the image is not drawn:\n{}",
            String::from_utf8_lossy(&content)
        );
    }
}

#[test]
fn images_and_gradients_after_runs_links_and_groups_are_each_drawn_once() {
    let group = PositionedElement::Group(oxoffice_layout::GroupElement {
        transform: oxoffice_layout::Transform::IDENTITY,
        clip: None,
        opacity: 1.0,
        effects: Vec::new(),
        children: vec![run("", ORIGIN, 12.0), picture()],
    });
    let link = PositionedElement::LinkAnnotation {
        rect: BOX,
        url: "https://example.com".to_owned(),
    };
    let read = Pdf::read(&oxoffice_pdf::render_to_pdf(&layout(vec![vec![
        run("Before", ORIGIN, 12.0),
        empty_runs()[1].1.clone(),
        link,
        group,
        gradient_box(),
        picture(),
    ]])));
    let content = read.page_content(0);
    let images = resource_names(&read, b"XObject");
    assert_eq!(images.len(), 2, "{images:?}");
    for image in images {
        let drawn = [b"/".as_slice(), &image, b" Do"].concat();
        assert_eq!(occurrences(&content, &drawn), 1, "{image:?}");
    }
    let patterns = resource_names(&read, b"Pattern");
    assert_eq!(patterns.len(), 2, "{patterns:?}");
    let fill = [b"/".as_slice(), &patterns[0], b" scn"].concat();
    let stroke = [b"/".as_slice(), &patterns[1], b" SCN"].concat();
    assert_eq!(occurrences(&content, &fill), 1);
    assert_eq!(occurrences(&content, &stroke), 1);
}
