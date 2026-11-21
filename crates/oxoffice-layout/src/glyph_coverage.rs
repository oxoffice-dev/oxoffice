//! Text drawn as empty boxes because no font in use has its characters.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use crate::font::{FontManager, SystemFonts, TextScript, script_for_char};
use crate::output::{Diagnostic, FontData, FontId, PageFrame, PositionedElement, walk};

/// What a render says about text its bundled fonts cannot draw.
///
/// One diagnostic per script and system font that drew such text in the
/// fallback mode ([`SystemFonts::Fallback`]), naming the font; then one per
/// script still drawn as empty boxes, saying why for the manager's mode and
/// ending with `way_out`, how the caller gives a font file.
pub fn glyph_coverage_diagnostics(
    pages: &[Arc<PageFrame>],
    fonts: &FontManager,
    way_out: &str,
) -> Vec<Diagnostic> {
    let data = fonts.all_font_data();
    let mut diagnostics =
        system_font_diagnostics(pages, &data, |id| fonts.is_system_fallback_font(id));
    let why = match fonts.system_fonts() {
        SystemFonts::Off => {
            "the render uses the bundled fonts only; render without that switch (CLI: drop \
             --bundled-fonts-only; Python bundled_fonts_only=False; Rust \
             with_bundled_fonts_only(false)) on a machine with a font for it, or "
        }
        SystemFonts::Unavailable => "this build has no system fonts; ",
        SystemFonts::Fallback | SystemFonts::First => {
            "no system font on this machine has them; install one, or "
        }
    };
    diagnostics.extend(missing_glyph_diagnostics(
        pages,
        &data,
        &format!("{why}{way_out}"),
    ));
    diagnostics
}

/// One diagnostic per script and system fallback font that drew text.
fn system_font_diagnostics(
    pages: &[Arc<PageFrame>],
    fonts: &[FontData],
    is_system_fallback: impl Fn(FontId) -> bool,
) -> Vec<Diagnostic> {
    let families: HashMap<FontId, &str> = fonts
        .iter()
        .filter(|font| is_system_fallback(font.id))
        .map(|font| (font.id, font.family.as_str()))
        .collect();
    if families.is_empty() {
        return Vec::new();
    }
    let mut drawn: BTreeMap<(&'static str, &str), BTreeSet<char>> = BTreeMap::new();
    for page in pages {
        walk(&page.elements, &mut |element, _| {
            let (font_id, text) = match element {
                PositionedElement::Text(run) => (run.font_id, run.text.as_str()),
                PositionedElement::MultilingualText(run) => {
                    (run.font_id, run.logical_text.as_str())
                }
                _ => return,
            };
            let Some(family) = families.get(&font_id) else {
                return;
            };
            for character in text.chars().filter(|character| draws_a_glyph(*character)) {
                // Latin and symbols in a CJK run are not what the font was
                // chosen for.
                if matches!(
                    script_for_char(character),
                    TextScript::Latin | TextScript::Common
                ) && !matches!(character, '\u{3000}'..='\u{303f}' | '\u{ff00}'..='\u{ffef}')
                {
                    continue;
                }
                drawn
                    .entry((script_name(character), family))
                    .or_default()
                    .insert(character);
            }
        });
    }
    drawn
        .into_iter()
        .map(|((script, family), characters)| Diagnostic {
            message: format!(
                "{script} text has no glyph in the bundled fonts and is drawn with the system \
                 font '{family}' ({}, such as \"{}\")",
                count_of(&characters),
                sample_of(&characters),
            ),
        })
        .collect()
}

fn count_of(characters: &BTreeSet<char>) -> String {
    match characters.len() {
        1 => "1 distinct character".to_owned(),
        count => format!("{count} distinct characters"),
    }
}

fn sample_of(characters: &BTreeSet<char>) -> String {
    characters.iter().take(3).collect()
}

/// One diagnostic per script whose characters a drawn run's font has no
/// glyph for, so that they are drawn as empty boxes.
///
/// The bundled fonts cover Latin, Greek, Cyrillic, Arabic, Hebrew,
/// Devanagari and Thai, and only a few characters of Chinese, Japanese and
/// Korean: the message names the script and ends with `way_out`, how the
/// caller's family gets a font for it (system fonts, a font file).
pub fn missing_glyph_diagnostics(
    pages: &[Arc<PageFrame>],
    fonts: &[FontData],
    way_out: &str,
) -> Vec<Diagnostic> {
    let faces: HashMap<FontId, &FontData> = fonts.iter().map(|font| (font.id, font)).collect();
    let mut parsed: HashMap<FontId, Option<ttf_parser::Face<'_>>> = HashMap::new();
    let mut missing: BTreeMap<&'static str, BTreeSet<char>> = BTreeMap::new();
    for page in pages {
        walk(&page.elements, &mut |element, _| {
            let (font_id, glyphs, text) = match element {
                PositionedElement::Text(run) => (run.font_id, &run.glyph_ids, run.text.as_str()),
                PositionedElement::MultilingualText(run) => {
                    (run.font_id, &run.glyph_ids, run.logical_text.as_str())
                }
                _ => return,
            };
            if !glyphs.contains(&0) {
                return;
            }
            let Some(face) = parsed
                .entry(font_id)
                .or_insert_with(|| {
                    faces
                        .get(&font_id)
                        .and_then(|font| ttf_parser::Face::parse(&font.data, font.face_index).ok())
                })
                .as_ref()
            else {
                return;
            };
            for character in text.chars().filter(|character| draws_a_glyph(*character)) {
                if face.glyph_index(character).is_none() {
                    missing
                        .entry(script_name(character))
                        .or_default()
                        .insert(character);
                }
            }
        });
    }
    missing
        .into_iter()
        .map(|(script, characters)| Diagnostic {
            message: format!(
                "{script} text has no glyph in the fonts used and is drawn as empty boxes \
                 ({}, such as \"{}\"): {way_out}",
                count_of(&characters),
                sample_of(&characters),
            ),
        })
        .collect()
}

/// Whether a character is drawn with a glyph of its own: not white space,
/// a control, a zero-width format character, a variation selector or a
/// combining mark (which a font may draw composed with its base).
fn draws_a_glyph(character: char) -> bool {
    !character.is_whitespace()
        && !character.is_control()
        && !matches!(
            character,
            '\u{00ad}'
                | '\u{0300}'..='\u{036f}'
                | '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{2069}'
                | '\u{fe00}'..='\u{fe0f}'
                | '\u{feff}'
                | '\u{e0100}'..='\u{e01ef}'
        )
}

fn script_name(character: char) -> &'static str {
    if matches!(character, '\u{3000}'..='\u{303f}' | '\u{ff00}'..='\u{ffef}') {
        return "Chinese, Japanese or Korean punctuation and full-width";
    }
    match script_for_char(character) {
        TextScript::Han => "Chinese, Japanese or Korean ideograph (Han)",
        TextScript::Kana => "Japanese (kana)",
        TextScript::Hangul => "Korean (Hangul)",
        TextScript::Arabic => "Arabic",
        TextScript::Hebrew => "Hebrew",
        TextScript::Devanagari => "Devanagari",
        TextScript::Thai => "Thai",
        TextScript::Latin => "Latin",
        TextScript::Common => "Symbol and other",
    }
}
