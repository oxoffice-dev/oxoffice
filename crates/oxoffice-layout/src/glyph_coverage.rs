//! Text drawn as empty boxes because no font in use has its characters.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use crate::font::{TextScript, script_for_char};
use crate::output::{Diagnostic, FontData, FontId, PageFrame, PositionedElement, walk};

/// One diagnostic per script whose characters a drawn run's font has no
/// glyph for, so that they are drawn as empty boxes.
///
/// The bundled fonts cover Latin, Greek, Cyrillic, Arabic, Hebrew,
/// Devanagari and Thai, and only a few characters of Chinese, Japanese and
/// Korean: the message names the script and the way out, system fonts or a
/// font file of the caller's.
pub fn missing_glyph_diagnostics(pages: &[Arc<PageFrame>], fonts: &[FontData]) -> Vec<Diagnostic> {
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
        .map(|(script, characters)| {
            let sample: String = characters.iter().take(3).collect();
            let count = match characters.len() {
                1 => "1 character".to_owned(),
                count => format!("{count} characters"),
            };
            Diagnostic {
                message: format!(
                    "{script} text has no glyph in the fonts used and is drawn as empty boxes \
                     ({count}, such as \"{sample}\"): render with system fonts (CLI \
                     --system-fonts, Python system_fonts=True, Rust \
                     RenderOptions::with_system_fonts) on a machine with a font for it, or \
                     pass a font file"
                ),
            }
        })
        .collect()
}

/// Whether a character is drawn with a glyph of its own: not white space,
/// a control or a zero-width format character.
fn draws_a_glyph(character: char) -> bool {
    !character.is_whitespace()
        && !character.is_control()
        && !matches!(
            character,
            '\u{00ad}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{2069}' | '\u{feff}'
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
