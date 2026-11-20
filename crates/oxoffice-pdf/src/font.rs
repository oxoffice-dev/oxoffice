//! Font subsetting and ToUnicode CMap generation for PDF embedding.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use oxoffice_layout::{FontData, FontId, LayoutResult, PositionedElement, walk};
use pdf_writer::types::{SystemInfo, UnicodeCmap};
use pdf_writer::{Name, Str};
use subsetter::GlyphRemapper;
use ttf_parser::gsub::SubstitutionSubtable;
use ttf_parser::opentype_layout::Coverage;

/// Per-font glyph usage collected across all pages.
pub(crate) struct FontUsage {
    /// Mapping from original glyph ID to the Unicode text it draws.
    ///
    /// A ligature draws several characters with one glyph, so the text is a
    /// string. The CMap holds one entry per glyph for the whole font, so when
    /// runs pair one glyph with different text, the strongest pairing wins,
    /// and the first one seen among equals.
    ///
    /// Ordered by glyph ID, because this map is iterated to emit the ToUnicode
    /// CMap. A hashed order put the same pairs in a different order on every
    /// run, which made the written PDF differ from itself byte for byte.
    pub glyph_to_unicode: BTreeMap<u16, (Pairing, String)>,
    /// Every glyph a run draws. Each declares its width, including a glyph
    /// that carries no text of its own.
    pub drawn_glyphs: BTreeSet<u16>,
    /// The GlyphRemapper for subsetting.
    pub remapper: GlyphRemapper,
}

/// Collected font info ready for PDF embedding.
pub(crate) struct PreparedFont {
    pub font_data: FontData,
    pub subset_bytes: Vec<u8>,
    pub remapper: GlyphRemapper,
    pub cmap_bytes: Vec<u8>,
    pub widths: Vec<(u16, f64)>, // (new_gid, width_in_font_units)
}

impl FontUsage {
    /// Subset a drawn glyph, declare its width, and map it to its text unless
    /// another run paired it more strongly.
    fn draw(&mut self, glyph: u16, text: Option<(Pairing, String)>) {
        self.remapper.remap(glyph);
        self.drawn_glyphs.insert(glyph);
        let Some((pairing, text)) = text else {
            return;
        };
        match self.glyph_to_unicode.entry(glyph) {
            Entry::Vacant(entry) => {
                entry.insert((pairing, text));
            }
            Entry::Occupied(mut entry) if entry.get().0 < pairing => {
                entry.insert((pairing, text));
            }
            Entry::Occupied(_) => {}
        }
    }
}

/// How strongly a glyph was paired with the text it draws, weakest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Pairing {
    /// A glyph of a rich run that draws none of its cluster's characters by
    /// itself, such as the dots an Arabic font draws apart from their letter.
    /// It repeats the cluster's text, which the run's `ActualText` covers, so
    /// that every glyph of a rich run maps to Unicode.
    Shared,
    /// The first of several glyphs that draw some characters between them
    /// in no order the font explains. It carries all of those characters.
    Group,
    /// Paired by position among as many unexplained glyphs as characters.
    Position,
    /// The one glyph left to draw the characters of its group that no other
    /// glyph explains.
    Whole,
    /// The glyph the font's cmap gives the character.
    Nominal,
    /// The glyph a GSUB ligature joins the glyphs of several characters into.
    /// It outranks the cmap because one glyph can be both. Carlito draws "fi"
    /// and U+FB01 with one glyph, and one literal U+FB01 must not turn every
    /// "fi" of the document into U+FB01.
    Ligature,
}

/// What a font says about the glyphs a run draws.
struct FontGlyphs<'a> {
    face: Option<ttf_parser::Face<'a>>,
    /// Each ligature glyph, with the glyph sequences GSUB joins into it.
    ligatures: HashMap<u16, Vec<Vec<u16>>>,
}

impl<'a> FontGlyphs<'a> {
    fn new(font: Option<&'a FontData>) -> Self {
        let face = font.and_then(|font| ttf_parser::Face::parse(&font.data, font.face_index).ok());
        let ligatures = face.as_ref().map(ligature_components).unwrap_or_default();
        Self { face, ligatures }
    }

    /// The glyph the cmap gives each character, `.notdef` for a character
    /// the font lacks, which is also what the shaper draws for it.
    fn nominal_glyphs(&self, characters: &[char]) -> Vec<Option<u16>> {
        characters
            .iter()
            .map(|&character| {
                let face = self.face.as_ref()?;
                Some(face.glyph_index(character).map_or(0, |glyph| glyph.0))
            })
            .collect()
    }

    /// How many characters, from the start of `nominal`, the glyph draws when
    /// it is the cmap glyph of the first or a GSUB ligature of the first few.
    fn explain(&self, glyph: u16, nominal: &[Option<u16>]) -> Option<(usize, Pairing)> {
        if nominal.first() == Some(&Some(glyph)) {
            return Some((1, Pairing::Nominal));
        }
        let length = self.ligature_length(glyph, nominal, 2)?;
        Some((length, Pairing::Ligature))
    }

    fn ligature_length(&self, glyph: u16, nominal: &[Option<u16>], depth: u8) -> Option<usize> {
        let sequences = self.ligatures.get(&glyph)?;
        sequences
            .iter()
            .filter_map(|components| {
                let mut length = 0;
                for &component in components {
                    let rest = &nominal[length..];
                    if rest.first() == Some(&Some(component)) {
                        length += 1;
                    } else if depth > 0 {
                        length += self.ligature_length(component, rest, depth - 1)?;
                    } else {
                        return None;
                    }
                }
                Some(length)
            })
            .max()
    }
}

/// Every ligature glyph of the font's GSUB table, with the glyph sequences
/// it replaces.
fn ligature_components(face: &ttf_parser::Face<'_>) -> HashMap<u16, Vec<Vec<u16>>> {
    let mut ligatures: HashMap<u16, Vec<Vec<u16>>> = HashMap::new();
    let Some(gsub) = face.tables().gsub else {
        return ligatures;
    };
    for lookup in gsub.lookups {
        for subtable in lookup.subtables.into_iter::<SubstitutionSubtable>() {
            let SubstitutionSubtable::Ligature(table) = subtable else {
                continue;
            };
            let first_glyphs: Vec<ttf_parser::GlyphId> = match table.coverage {
                Coverage::Format1 { glyphs } => glyphs.into_iter().collect(),
                Coverage::Format2 { records } => records
                    .into_iter()
                    .flat_map(|record| record.start.0..=record.end.0)
                    .map(ttf_parser::GlyphId)
                    .collect(),
            };
            for first in first_glyphs {
                let Some(set) = table
                    .coverage
                    .get(first)
                    .and_then(|index| table.ligature_sets.get(index))
                else {
                    continue;
                };
                for ligature in set {
                    let mut components = vec![first.0];
                    components.extend(ligature.components.into_iter().map(|glyph| glyph.0));
                    ligatures
                        .entry(ligature.glyph.0)
                        .or_default()
                        .push(components);
                }
            }
        }
    }
    ligatures
}

/// How far past an unexplained glyph the pairing looks for the next glyph the
/// font explains. A ligature or a cluster spans a few characters, and the
/// bound keeps a run the font explains nowhere linear.
const RESYNC_WINDOW: usize = 16;

/// Pair each glyph of a plain run with the characters it draws.
///
/// A plain run carries its glyphs and its text but not the shaper's clusters,
/// and a ligature draws several characters with one glyph, so pairing them by
/// index shifts every later glyph. The pairing is rebuilt from the font
/// instead. A glyph draws the next character when the cmap gives it that
/// character, and the next few when GSUB joins their glyphs into it. Glyphs
/// the font explains neither way share the characters up to the next glyph
/// it does explain. When there is none within `RESYNC_WINDOW`, an unexplained
/// glyph takes one character by position. A plain run has no `ActualText`, so
/// a glyph that draws none of its group's characters by itself gets no text
/// rather than repeat a character another glyph carries, and so does a glyph
/// left after the last character.
fn pair_plain_run(
    characters: &[char],
    glyphs: &[u16],
    font: &FontGlyphs<'_>,
) -> Vec<Option<(Pairing, String)>> {
    let nominal = font.nominal_glyphs(characters);
    let mut texts = vec![None; glyphs.len()];
    let (mut char_at, mut glyph_at) = (0, 0);
    while char_at < characters.len() && glyph_at < glyphs.len() {
        if let Some((length, pairing)) = font.explain(glyphs[glyph_at], &nominal[char_at..]) {
            let text = characters[char_at..char_at + length].iter().collect();
            texts[glyph_at] = Some((pairing, text));
            char_at += length;
            glyph_at += 1;
            continue;
        }

        let char_limit = characters.len().min(char_at + 1 + RESYNC_WINDOW);
        let glyph_limit = glyphs.len().min(glyph_at + 1 + RESYNC_WINDOW);
        let next_explained = (glyph_at + 1..glyph_limit).find_map(|glyph_end| {
            (char_at + 1..char_limit)
                .find(|&char_end| {
                    font.explain(glyphs[glyph_end], &nominal[char_end..])
                        .is_some()
                })
                .map(|char_end| (char_end, glyph_end))
        });
        let (char_end, glyph_end) = match next_explained {
            Some(end) => end,
            None if characters.len() - char_at <= RESYNC_WINDOW
                && glyphs.len() - glyph_at <= RESYNC_WINDOW =>
            {
                (characters.len(), glyphs.len())
            }
            None => {
                texts[glyph_at] = Some((Pairing::Position, characters[char_at].to_string()));
                char_at += 1;
                glyph_at += 1;
                continue;
            }
        };
        let group = pair_group(
            &characters[char_at..char_end],
            &glyphs[glyph_at..glyph_end],
            &nominal[char_at..char_end],
            font,
        );
        texts.splice(glyph_at..glyph_end, group);
        char_at = char_end;
        glyph_at = glyph_end;
    }
    texts
}

/// Pair the glyphs of a group with the characters they draw between them,
/// such as one shaping cluster, in which glyph order need not follow text
/// order.
///
/// A glyph the font explains draws the characters it explains, when no other
/// glyph of the group has taken them. The characters left over go to the one
/// glyph left over, or pair with the glyphs left over by position when they
/// are as many. Otherwise the first glyph left over carries them all, so that
/// none is lost. A glyph still without text draws none of the characters by
/// itself and gets no text, so that no character is paired twice.
fn pair_group(
    characters: &[char],
    glyphs: &[u16],
    nominal: &[Option<u16>],
    font: &FontGlyphs<'_>,
) -> Vec<Option<(Pairing, String)>> {
    let mut texts = vec![None; glyphs.len()];
    let mut claimed = vec![false; characters.len()];
    for (text, &glyph) in texts.iter_mut().zip(glyphs) {
        let explained = (0..characters.len()).find_map(|start| {
            let (length, pairing) = font.explain(glyph, &nominal[start..])?;
            let range = start..start + length;
            claimed[range.clone()]
                .iter()
                .all(|taken| !taken)
                .then_some((range, pairing))
        });
        if let Some((range, pairing)) = explained {
            claimed[range.clone()].fill(true);
            *text = Some((pairing, characters[range].iter().collect()));
        }
    }
    let rest_characters = characters
        .iter()
        .zip(&claimed)
        .filter(|(_, claimed)| !**claimed)
        .map(|(character, _)| *character)
        .collect::<Vec<_>>();
    let rest_glyphs = (0..glyphs.len())
        .filter(|&index| texts[index].is_none())
        .collect::<Vec<_>>();
    match rest_glyphs.as_slice() {
        _ if rest_characters.is_empty() => {}
        [] => {}
        [only] => texts[*only] = Some((Pairing::Whole, rest_characters.into_iter().collect())),
        rest if rest.len() == rest_characters.len() => {
            for (&index, character) in rest.iter().zip(rest_characters) {
                texts[index] = Some((Pairing::Position, character.to_string()));
            }
        }
        [first, ..] => {
            texts[*first] = Some((Pairing::Group, rest_characters.into_iter().collect()));
        }
    }
    texts
}

fn font_glyphs<'f, 'a>(
    fonts: &'f mut HashMap<FontId, FontGlyphs<'a>>,
    layout: &'a LayoutResult,
    font_id: FontId,
) -> &'f FontGlyphs<'a> {
    fonts
        .entry(font_id)
        .or_insert_with(|| FontGlyphs::new(layout.fonts.iter().find(|font| font.id == font_id)))
}

/// Collect glyph usage across all pages for each font.
pub(crate) fn collect_glyph_usage(layout: &LayoutResult) -> HashMap<FontId, FontUsage> {
    let mut usage: HashMap<FontId, FontUsage> = HashMap::new();
    let mut fonts: HashMap<FontId, FontGlyphs<'_>> = HashMap::new();

    for page in &layout.pages {
        walk(&page.elements, &mut |element, _| {
            if let PositionedElement::Text(run) = element
                && !(run.text.is_empty() && run.glyph_ids.is_empty())
            {
                let entry = usage.entry(run.font_id).or_insert_with(|| FontUsage {
                    glyph_to_unicode: BTreeMap::new(),
                    drawn_glyphs: BTreeSet::new(),
                    remapper: GlyphRemapper::new(),
                });

                // Map glyph IDs to the characters each one draws
                let chars: Vec<char> = run.text.chars().collect();
                let font = font_glyphs(&mut fonts, layout, run.font_id);
                let texts = pair_plain_run(&chars, &run.glyph_ids, font);
                for (&gid, text) in run.glyph_ids.iter().zip(texts) {
                    entry.draw(gid, text);
                }
            }
            if let PositionedElement::MultilingualText(run) = element
                && !(run.logical_text.is_empty() && run.glyph_ids.is_empty())
                && run.is_valid()
            {
                let entry = usage.entry(run.font_id).or_insert_with(|| FontUsage {
                    glyph_to_unicode: BTreeMap::new(),
                    drawn_glyphs: BTreeSet::new(),
                    remapper: GlyphRemapper::new(),
                });
                let chars = run.logical_text.chars().collect::<Vec<_>>();
                let font = font_glyphs(&mut fonts, layout, run.font_id);
                for cluster in &run.clusters {
                    let characters =
                        chars.get(cluster.char_start as usize..cluster.char_end as usize);
                    let glyphs = run
                        .glyph_ids
                        .get(cluster.glyph_start as usize..cluster.glyph_end as usize);
                    let (Some(characters), Some(glyphs)) = (characters, glyphs) else {
                        continue;
                    };
                    let nominal = font.nominal_glyphs(characters);
                    let texts = pair_group(characters, glyphs, &nominal, font);
                    // The run's ActualText carries its text, so a glyph that
                    // draws none of the cluster's characters by itself repeats
                    // them all, and every glyph of the run maps to Unicode.
                    let cluster_text = characters.iter().collect::<String>();
                    for (&gid, text) in glyphs.iter().zip(texts) {
                        let text = text.unwrap_or_else(|| (Pairing::Shared, cluster_text.clone()));
                        entry.draw(gid, Some(text));
                    }
                }
            }
        });
    }

    usage
}

/// Apply the exact selected face coordinates, refusing malformed metadata.
pub(crate) fn instanced_face(font: &FontData) -> Option<ttf_parser::Face<'_>> {
    let mut face = ttf_parser::Face::parse(&font.data, font.face_index).ok()?;
    for (tag, value) in &font.variations {
        if !value.is_finite() {
            return None;
        }
        face.set_variation(ttf_parser::Tag::from_bytes(tag), *value)?;
    }
    Some(face)
}

/// Prepare every contributing font before any PDF output is allocated.
/// Loaded but unused faces are deliberately excluded from this operation.
pub(crate) fn prepare_fonts(
    layout: &LayoutResult,
) -> Result<BTreeMap<FontId, PreparedFont>, crate::PdfError> {
    let mut usage = collect_glyph_usage(layout)
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let fonts = layout
        .fonts
        .iter()
        .map(|font| (font.id, font))
        .collect::<HashMap<_, _>>();
    let mut prepared = BTreeMap::new();
    for (id, usage) in &mut usage {
        let source = fonts
            .get(id)
            .ok_or(crate::PdfError::MissingEmbeddedFont { font_id: id.0 })?;
        let font = prepare_font(source, usage)
            .ok_or(crate::PdfError::MissingEmbeddedFont { font_id: id.0 })?;
        prepared.insert(*id, font);
    }
    Ok(prepared)
}

/// Subset a font and prepare it for PDF embedding.
pub(crate) fn prepare_font(font_data: &FontData, usage: &mut FontUsage) -> Option<PreparedFont> {
    // Subset the font
    instanced_face(font_data)?;
    let coordinates = font_data
        .variations
        .iter()
        .map(|(tag, value)| (subsetter::Tag::new(tag), *value))
        .collect::<Vec<_>>();
    let subset_bytes = if coordinates.is_empty() {
        subsetter::subset(&font_data.data, font_data.face_index, &usage.remapper).ok()?
    } else {
        subsetter::subset_with_variations(
            &font_data.data,
            font_data.face_index,
            &coordinates,
            &usage.remapper,
        )
        .ok()?
    };

    // Build ToUnicode CMap
    let mut cmap = UnicodeCmap::new(
        Name(b"Adobe"),
        SystemInfo {
            registry: Str(b"Adobe"),
            ordering: Str(b"Identity"),
            supplement: 0,
        },
    );
    for (&old_gid, (_, text)) in &usage.glyph_to_unicode {
        if let Some(new_gid) = usage.remapper.get(old_gid) {
            cmap.pair_with_multiple(new_gid, text.chars());
        }
    }
    let cmap_bytes = cmap.finish().to_vec();

    // Compute glyph widths from the original font for the CID widths array.
    // We need to parse the font to get per-glyph advance widths.
    let widths = compute_glyph_widths(font_data, usage);

    Some(PreparedFont {
        font_data: font_data.clone(),
        subset_bytes,
        remapper: usage.remapper.clone(),
        cmap_bytes,
        widths,
    })
}

/// Compute per-glyph widths in font design units (1000 units = 1 em).
fn compute_glyph_widths(font_data: &FontData, usage: &FontUsage) -> Vec<(u16, f64)> {
    let mut widths = Vec::new();

    let Some(face) = instanced_face(font_data) else {
        return widths;
    };

    let units_per_em = face.units_per_em() as f64;
    let scale = 1000.0 / units_per_em;

    for &old_gid in &usage.drawn_glyphs {
        if let Some(new_gid) = usage.remapper.get(old_gid) {
            let advance = face
                .glyph_hor_advance(ttf_parser::GlyphId(old_gid))
                .unwrap_or(0) as f64;
            widths.push((new_gid, advance * scale));
        }
    }

    // Also include glyphs that may not have unicode mapping but were remapped
    // (e.g. .notdef glyph 0 is always included by subsetter)

    widths.sort_by_key(|&(gid, _)| gid);
    widths
}

/// Get font metrics from raw font data for the font descriptor.
pub(crate) struct FontMetricsInfo {
    pub ascent: f64,
    pub descent: f64,
    pub cap_height: f64,
    pub bbox: [f64; 4],
    pub italic_angle: f64,
    pub stem_v: f64,
}

pub(crate) fn get_font_metrics(font_data: &FontData) -> Option<FontMetricsInfo> {
    let face = instanced_face(font_data)?;
    let units_per_em = face.units_per_em() as f64;
    let scale = 1000.0 / units_per_em;

    let ascent = face.ascender() as f64 * scale;
    let descent = face.descender() as f64 * scale;
    let cap_height = match face.capital_height() {
        Some(h) => h as f64 * scale,
        None => ascent,
    };
    let italic_angle = face.italic_angle() as f64;

    let bbox = face.global_bounding_box();
    let bbox = [
        bbox.x_min as f64 * scale,
        bbox.y_min as f64 * scale,
        bbox.x_max as f64 * scale,
        bbox.y_max as f64 * scale,
    ];

    // Approximate stem_v from weight class
    let stem_v = if font_data.bold { 120.0 } else { 80.0 };

    Some(FontMetricsInfo {
        ascent,
        descent,
        cap_height,
        bbox,
        italic_angle,
        stem_v,
    })
}
