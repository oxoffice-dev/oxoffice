//! The system-font fallback: bundled fonts first, a system font only for the
//! characters they cannot draw.
//!
//! The host's fonts differ between machines, so these tests give the fallback
//! tier a fixed set of fonts (`tests/fixtures`, see its README) in place of
//! the host scan. The host scan itself is exercised by the docx and pptx
//! render tests, which hold on any machine.

use std::sync::Arc;

use oxoffice_layout::{
    Color, FontFile, FontId, FontManager, GlyphRun, PageFrame, Point, PositionedElement,
    SystemFonts, glyph_coverage_diagnostics,
};

const FALLBACK_TEST: &[u8] = include_bytes!("fixtures/OxofficeFallbackTest.otf");
const NAMED_TEST: &[u8] = include_bytes!("fixtures/OxofficeNamedTest.otf");

fn fallback_fonts(order: &[(&str, &[u8])]) -> Vec<FontFile> {
    order
        .iter()
        .map(|(family, data)| FontFile {
            family: (*family).to_owned(),
            data: data.to_vec(),
        })
        .collect()
}

/// A manager whose system tier is the fallback test font, then the named one.
fn manager() -> FontManager {
    FontManager::new_with_fallback_fonts(&fallback_fonts(&[
        ("Oxoffice Fallback Test", FALLBACK_TEST),
        ("Oxoffice Named Test", NAMED_TEST),
    ]))
    .expect("fallback fonts load")
}

fn family(fonts: &FontManager, id: FontId) -> String {
    fonts.font_data(id).expect("font is loaded").family
}

#[test]
fn text_the_bundled_fonts_cover_never_opens_the_system_tier() {
    let mut fonts = manager();
    assert_eq!(fonts.system_fonts(), SystemFonts::Fallback);

    let latin = fonts
        .resolve_font_for_text(Some("Calibri"), false, false, "Hello")
        .unwrap();
    assert_eq!(family(&fonts, latin), "Carlito");
    // The bundled Noto Sans SC subset draws these two ideographs.
    let han = fonts
        .resolve_font_for_text(Some("Calibri"), false, false, "你好")
        .unwrap();
    assert_eq!(family(&fonts, han), "Noto Sans SC");

    assert!(!fonts.system_fallback_opened());
    assert!(!fonts.is_system_fallback_font(latin));
    assert!(!fonts.is_system_fallback_font(han));
}

#[test]
fn characters_no_bundled_font_has_are_drawn_with_a_system_font() {
    let mut fonts = manager();

    let cat = fonts
        .resolve_font_for_text(Some("Calibri"), false, false, "猫")
        .unwrap();

    assert!(fonts.system_fallback_opened());
    assert!(fonts.is_system_fallback_font(cat));
    // The PDF and the diagnostics see the font's own family name.
    assert_eq!(family(&fonts, cat), "Oxoffice Fallback Test");
    let shaped = fonts.shape_text(cat, "猫", 12.0).unwrap();
    assert!(shaped.glyph_ids.iter().all(|glyph| *glyph != 0));
}

#[test]
fn a_system_font_is_never_chosen_by_its_family_name() {
    let mut fonts = manager();
    // Open the system tier first, so its faces are known to the manager.
    fonts
        .resolve_font_for_text(Some("Calibri"), false, false, "猫")
        .unwrap();
    assert!(fonts.system_fallback_opened());

    // The named test font covers this Latin text and carries the requested
    // name, but family resolution only sees the bundled fonts.
    let latin = fonts
        .resolve_font_for_text(Some("Oxoffice Named Test"), false, false, "abc")
        .unwrap();
    assert!(!fonts.is_system_fallback_font(latin));
    assert_ne!(family(&fonts, latin), "Oxoffice Named Test");
    let by_name = fonts
        .resolve_font(Some("Oxoffice Named Test"), false, false)
        .unwrap();
    assert!(!fonts.is_system_fallback_font(by_name));
}

#[test]
fn a_system_font_that_rescued_a_run_does_not_take_text_the_bundled_fonts_cover() {
    // The named test font comes first and covers both runs.
    let mut fonts = FontManager::new_with_fallback_fonts(&fallback_fonts(&[
        ("Oxoffice Named Test", NAMED_TEST),
        ("Oxoffice Fallback Test", FALLBACK_TEST),
    ]))
    .unwrap();

    let cat = fonts
        .resolve_font_for_text(Some("Calibri"), false, false, "猫")
        .unwrap();
    assert_eq!(family(&fonts, cat), "Oxoffice Named Test");

    let han = fonts
        .resolve_font_for_text(Some("Calibri"), false, false, "你好")
        .unwrap();
    assert_eq!(family(&fonts, han), "Noto Sans SC");
    assert!(!fonts.is_system_fallback_font(han));
}

#[test]
fn a_run_the_bundled_fonts_cover_between_them_never_takes_a_system_font() {
    // No bundled face has both scripts (你好 is in Noto Sans SC, こ in Noto
    // Sans JP), while the named test font has all three characters.
    let mut fonts = FontManager::new_with_fallback_fonts(&fallback_fonts(&[(
        "Oxoffice Named Test",
        NAMED_TEST,
    )]))
    .unwrap();

    let mixed = fonts
        .resolve_font_for_text(Some("Calibri"), false, false, "你好こ")
        .unwrap();

    assert!(!fonts.is_system_fallback_font(mixed));
    assert!(!fonts.system_fallback_opened());
}

#[test]
fn a_run_with_a_character_no_bundled_font_has_takes_the_system_font_covering_it_all() {
    let mut fonts = FontManager::new_with_fallback_fonts(&fallback_fonts(&[(
        "Oxoffice Named Test",
        NAMED_TEST,
    )]))
    .unwrap();

    let mixed = fonts
        .resolve_font_for_text(Some("Calibri"), false, false, "你好猫")
        .unwrap();

    assert!(fonts.is_system_fallback_font(mixed));
    assert_eq!(family(&fonts, mixed), "Oxoffice Named Test");
}

#[test]
fn a_character_no_system_font_has_is_searched_for_once() {
    let mut fonts = manager();
    // 猫 and 犬 are in the test fonts; 한 is in no font at all.
    let first = fonts
        .resolve_font_for_text(Some("Calibri"), false, false, "猫한")
        .unwrap();
    assert!(fonts.is_system_fallback_font(first));
    assert_eq!(fonts.system_fallback_scans(), 1);

    // The next run holding it takes the font that drew the first, without
    // reading every system font again.
    let second = fonts
        .resolve_font_for_text(Some("Calibri"), false, false, "犬한")
        .unwrap();
    assert_eq!(second, first);
    assert_eq!(fonts.system_fallback_scans(), 1);
}

#[test]
fn the_system_tier_survives_a_change_of_caller_fonts() {
    let mut fonts = manager();
    fonts
        .resolve_font_for_text(Some("Calibri"), false, false, "猫")
        .unwrap();
    // Replacing the caller fonts rebuilds the font database.
    fonts.load_additional_fonts(&fallback_fonts(&[("Caller Cat", FALLBACK_TEST)]));
    fonts.load_additional_fonts(&[]);

    let cat = fonts
        .resolve_font_for_text(Some("Calibri"), false, false, "猫")
        .unwrap();
    assert!(fonts.is_system_fallback_font(cat));
    assert_eq!(family(&fonts, cat), "Oxoffice Fallback Test");
}

#[test]
fn bundled_only_fonts_draw_missing_characters_as_boxes() {
    let mut fonts = FontManager::new_deterministic().unwrap();
    assert_eq!(fonts.system_fonts(), SystemFonts::Off);

    let cat = fonts
        .resolve_font_for_text(Some("Calibri"), false, false, "猫")
        .unwrap();

    assert!(!fonts.system_fallback_opened());
    assert!(!fonts.is_system_fallback_font(cat));
    let shaped = fonts.shape_text(cat, "猫", 12.0).unwrap();
    assert_eq!(shaped.glyph_ids, vec![0]);
}

fn page_with(fonts: &mut FontManager, text: &str) -> Arc<PageFrame> {
    let id = fonts
        .resolve_font_for_text(Some("Calibri"), false, false, text)
        .unwrap();
    let shaped = fonts.shape_text(id, text, 12.0).unwrap();
    let run = GlyphRun {
        origin: Point { x: 72.0, y: 72.0 },
        font_id: id,
        font_size: 12.0,
        advances: shaped.advances,
        glyph_ids: shaped.glyph_ids,
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
    };
    Arc::new(PageFrame::new(
        1,
        612.0,
        792.0,
        vec![PositionedElement::Text(run)],
    ))
}

#[test]
fn the_diagnostic_names_the_system_font_that_drew_the_text() {
    let mut fonts = manager();
    let page = page_with(&mut fonts, "猫犬");

    let diagnostics = glyph_coverage_diagnostics(&[page], &fonts, "WAY OUT");

    let messages: Vec<&str> = diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(messages.len(), 1, "{messages:?}");
    let message = messages[0];
    assert!(
        message.contains("Chinese, Japanese or Korean ideograph (Han)")
            && message.contains("system font 'Oxoffice Fallback Test'")
            && message.contains("2 distinct characters"),
        "{message}"
    );
    assert!(!message.contains("empty boxes") && !message.contains("WAY OUT"));
}

#[test]
fn the_diagnostic_says_when_no_system_font_has_the_characters() {
    let mut fonts = manager();
    // Hangul: neither the bundled fonts nor the test fonts have it.
    let page = page_with(&mut fonts, "한국어");

    let diagnostics = glyph_coverage_diagnostics(&[page], &fonts, "WAY OUT");

    let message = &diagnostics.first().expect("one diagnostic").message;
    assert!(
        message.contains("Korean (Hangul)")
            && message.contains("drawn as empty boxes")
            && message.contains("no system font on this machine has them")
            && message.contains("WAY OUT"),
        "{message}"
    );
}

#[test]
fn the_bundled_only_diagnostic_names_the_switch_to_system_fonts() {
    let mut fonts = FontManager::new_deterministic().unwrap();
    let page = page_with(&mut fonts, "猫");

    let diagnostics = glyph_coverage_diagnostics(&[page], &fonts, "WAY OUT");

    let message = &diagnostics.first().expect("one diagnostic").message;
    assert!(
        message.contains("drawn as empty boxes")
            && message.contains("bundled fonts only")
            && message.contains("--bundled-fonts-only")
            && message.contains("WAY OUT"),
        "{message}"
    );
}
