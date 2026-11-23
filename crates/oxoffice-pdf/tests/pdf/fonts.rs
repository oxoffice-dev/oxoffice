//! Embedded fonts: a font program is declared as the kind of outlines it
//! holds, TrueType or CFF.

use std::sync::Arc;

use lopdf::Dictionary;
use oxoffice_layout::{
    Color, FontData, FontId, GlyphRun, LayoutResult, PageFrame, Point, PositionedElement,
};

use crate::support::{PAGE_HEIGHT, PAGE_WIDTH, Pdf, layout, run};

/// A subset of Noto Sans CJK SC with CFF outlines (`OTTO`), test data of the
/// system fallback.
const CFF_FONT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../oxoffice-layout/tests/fixtures/OxofficeNamedTest.otf"
));
const CFF: FontId = FontId(2);

/// A run of `text` in the CFF font.
fn cff_run(text: &str, origin: Point) -> PositionedElement {
    let face = ttf_parser::Face::parse(CFF_FONT, 0).unwrap();
    let size = 12.0;
    let scale = size / f64::from(face.units_per_em());
    let glyph_ids: Vec<u16> = text
        .chars()
        .map(|ch| face.glyph_index(ch).expect("the subset covers the text").0)
        .collect();
    let advances = glyph_ids
        .iter()
        .map(|glyph| {
            f64::from(
                face.glyph_hor_advance(ttf_parser::GlyphId(*glyph))
                    .unwrap_or(0),
            ) * scale
        })
        .collect();
    PositionedElement::Text(GlyphRun {
        origin,
        font_id: CFF,
        font_size: size,
        glyph_ids,
        advances,
        text: text.to_owned(),
        source: None,
        color: Color::BLACK,
        bold: false,
        italic: false,
        field_kind: None,
        field_source: None,
        note: None,
        note_reference_source: None,
        tab_aligned: None,
    })
}

/// A page drawing a run in Liberation Sans and one in the CFF font.
fn both_fonts() -> LayoutResult {
    let base = layout(vec![Vec::new()]);
    let mut fonts = base.fonts.clone();
    fonts.push(FontData {
        id: CFF,
        family: "Oxoffice Named Test".to_owned(),
        data: Arc::from(CFF_FONT),
        face_index: 0,
        bold: false,
        italic: false,
        variations: Vec::new(),
        synthetic_bold: false,
        synthetic_italic: false,
    });
    let page = PageFrame::new(
        1,
        PAGE_WIDTH,
        PAGE_HEIGHT,
        vec![
            run("Liberation", Point { x: 72.0, y: 100.0 }, 12.0),
            cff_run("猫犬Abc", Point { x: 72.0, y: 130.0 }),
        ],
    );
    LayoutResult::new(vec![Arc::new(page)], fonts, None, Vec::new())
}

/// The descendant CID font of each Type0 font of the first page, by the
/// family its base font names.
fn cid_fonts(read: &Pdf) -> Vec<(String, &Dictionary)> {
    let page = read
        .doc
        .get_object(read.page_ids()[0])
        .unwrap()
        .as_dict()
        .unwrap();
    let fonts = read.dict(read.dict(page, b"Resources"), b"Font");
    fonts
        .iter()
        .map(|(_, font)| {
            let font = read.resolve(font).as_dict().unwrap();
            let descendant = read
                .resolve(&font.get(b"DescendantFonts").unwrap().as_array().unwrap()[0])
                .as_dict()
                .unwrap();
            let name = descendant.get(b"BaseFont").unwrap().as_name().unwrap();
            (String::from_utf8_lossy(name).into_owned(), descendant)
        })
        .collect()
}

#[test]
fn a_cff_font_is_embedded_as_a_cff_program_and_a_truetype_font_as_truetype() {
    let read = Pdf::read(&oxoffice_pdf::render_to_pdf(&both_fonts()));
    let fonts = cid_fonts(&read);
    assert_eq!(fonts.len(), 2);
    for (name, cid) in fonts {
        let descriptor = read.dict(cid, b"FontDescriptor");
        let subtype = cid.get(b"Subtype").unwrap().as_name().unwrap();
        if name.contains("Oxoffice") {
            assert_eq!(subtype, b"CIDFontType0", "{name}");
            assert!(
                cid.get(b"CIDToGIDMap").is_err(),
                "{name}: CIDs address glyphs"
            );
            assert!(descriptor.get(b"FontFile2").is_err(), "{name}");
            let file = descriptor.get(b"FontFile3").expect("a CFF program");
            let stream = read.resolve(file).as_stream().unwrap();
            assert_eq!(
                stream.dict.get(b"Subtype").unwrap().as_name().unwrap(),
                b"CIDFontType0C"
            );
            // A bare CFF table: version 1.0 and its header size, not an
            // OpenType font.
            let program = read.stream(file);
            assert_eq!(program[..3], [1, 0, 4], "{name}");
        } else {
            assert_eq!(subtype, b"CIDFontType2", "{name}");
            assert!(cid.get(b"CIDToGIDMap").is_ok(), "{name}");
            assert!(descriptor.get(b"FontFile3").is_err(), "{name}");
            let program = read.stream(descriptor.get(b"FontFile2").unwrap());
            assert_eq!(program[..4], [0, 1, 0, 0], "{name}: a TrueType font");
        }
    }
}
