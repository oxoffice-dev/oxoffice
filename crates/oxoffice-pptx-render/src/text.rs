use std::borrow::Cow;
use std::collections::HashMap;

use oxoffice_layout::{
    Align, Color, FieldKind, FontManager, FontTraceEvent, GlyphRun, InlineItem, LayoutError,
    LayoutLine, LineBreakParams, LineClear, LineItem, LineSpacing, MultilingualGlyphRun, Paint,
    Point, PositionedElement, Rect, TextSegment, Underline, break_multilingual_into_lines,
};
use oxoffice_pptx_layout::{
    ParagraphAlignment, ResolvedAutofit, ResolvedBullet, ResolvedBulletSize, ResolvedParagraph,
    ResolvedRunStyle, ResolvedShape, ResolvedTextBody, ResolvedTextRun, ResolvedTextSpacing,
    TextAnchor, TextDirection,
};

use crate::TextLineLayout;

const DEFAULT_FONT_SIZE: f64 = 18.0;

/// PowerPoint's single line spacing, in ems of the largest run on the line.
///
/// It is the same for every font, whatever its ascent, descent and line gap.
const SINGLE_SPACING_EM: f64 = 1.2;

struct AutoNumberSequence {
    scheme: String,
    next: u32,
}

struct NumberingState {
    levels: Vec<Option<AutoNumberSequence>>,
}

#[derive(Default)]
struct ShapingCache {
    entries: Vec<(String, ResolvedRunStyle, TextSegment, Vec<FontTraceEvent>)>,
    bytes: usize,
}

const NATIVE_SHAPING_CACHE_MAX_BYTES: usize = 16 * 1024 * 1024;
const NATIVE_SHAPING_CACHE_MAX_ENTRIES: usize = 4_096;

fn shaping_cache_entry_bytes(
    text: &str,
    style: &ResolvedRunStyle,
    segment: &TextSegment,
    events: &Vec<FontTraceEvent>,
) -> usize {
    let strings = [
        &style.latin_typeface,
        &style.east_asian_typeface,
        &style.complex_script_typeface,
        &style.symbol_typeface,
        &style.language,
        &style.hyperlink_url,
        &segment.hyperlink_url,
    ];
    let mut bytes = text.len().saturating_add(segment.text.capacity());
    for string in strings {
        bytes = bytes.saturating_add(string.as_ref().map_or(0, String::capacity));
    }
    bytes = bytes
        .saturating_add(
            segment
                .glyph_ids
                .capacity()
                .saturating_mul(std::mem::size_of::<u16>()),
        )
        .saturating_add(
            segment
                .advances
                .capacity()
                .saturating_mul(std::mem::size_of::<f64>()),
        )
        .saturating_add(
            events
                .capacity()
                .saturating_mul(std::mem::size_of::<FontTraceEvent>()),
        );
    if let Some(Paint::Linear { stops, .. } | Paint::Radial { stops, .. }) = &style.fill {
        bytes = bytes.saturating_add(
            stops
                .capacity()
                .saturating_mul(std::mem::size_of::<oxoffice_layout::GradientStop>()),
        );
    }
    if let Some(Paint::Grid { colors, .. }) = &style.fill {
        bytes = bytes.saturating_add(
            colors
                .capacity()
                .saturating_mul(std::mem::size_of::<oxoffice_layout::Color>()),
        );
    }
    for event in events {
        if let FontTraceEvent::Contribution(contribution) = event {
            bytes = bytes.saturating_add(
                contribution
                    .requested_family
                    .as_ref()
                    .map_or(0, String::capacity),
            );
        }
    }
    bytes
}

impl ShapingCache {
    fn shape(
        &mut self,
        font_manager: &mut FontManager,
        text: &str,
        style: &ResolvedRunStyle,
    ) -> Result<TextSegment, LayoutError> {
        if let Some((_, _, segment, contributions)) = self
            .entries
            .iter()
            .find(|(cached_text, cached_style, _, _)| cached_text == text && cached_style == style)
            && font_manager
                .validate_font_trace_events(contributions)
                .is_ok()
        {
            let ids = contributions
                .iter()
                .filter_map(|event| match event {
                    FontTraceEvent::Resolution(id) => Some(*id),
                    FontTraceEvent::Contribution(_) => None,
                })
                .collect::<Vec<_>>();
            font_manager.replay_layout_font_trace(&ids);
            return Ok(segment.clone());
        }
        if text.is_empty() {
            return shape_run(font_manager, text, style);
        }
        let Some(scope) = font_manager.begin_font_contribution_scope() else {
            return shape_run(font_manager, text, style);
        };
        let result = shape_run(font_manager, text, style);
        if let Ok(segment) = &result {
            font_manager.record_font_contribution(
                segment.font_id,
                typeface_for_text(style, &segment.text),
                &segment.text,
            )?;
        }
        let captured = font_manager.finish_font_contribution_scope(scope)?;
        let segment = result?;
        if let Some(contributions) = captured {
            let entry_bytes = shaping_cache_entry_bytes(text, style, &segment, &contributions);
            let vector_bytes =
                self.entries
                    .len()
                    .saturating_add(1)
                    .saturating_mul(std::mem::size_of::<(
                        String,
                        ResolvedRunStyle,
                        TextSegment,
                        Vec<FontTraceEvent>,
                    )>());
            if self.entries.len() < NATIVE_SHAPING_CACHE_MAX_ENTRIES
                && self
                    .bytes
                    .saturating_add(entry_bytes)
                    .saturating_add(vector_bytes)
                    <= NATIVE_SHAPING_CACHE_MAX_BYTES
            {
                self.entries.reserve_exact(1);
                let actual_vector_bytes =
                    self.entries.capacity().saturating_mul(std::mem::size_of::<(
                        String,
                        ResolvedRunStyle,
                        TextSegment,
                        Vec<FontTraceEvent>,
                    )>());
                if self
                    .bytes
                    .saturating_add(entry_bytes)
                    .saturating_add(actual_vector_bytes)
                    <= NATIVE_SHAPING_CACHE_MAX_BYTES
                {
                    self.entries.push((
                        text.to_owned(),
                        style.clone(),
                        segment.clone(),
                        contributions,
                    ));
                    self.bytes = self.bytes.saturating_add(entry_bytes);
                }
            }
        }
        Ok(segment)
    }
}

impl Default for NumberingState {
    fn default() -> Self {
        Self {
            levels: std::iter::repeat_with(|| None).take(9).collect(),
        }
    }
}

impl NumberingState {
    fn marker(
        &mut self,
        font_manager: &mut FontManager,
        shaping_cache: &mut ShapingCache,
        paragraph: &ResolvedParagraph,
    ) -> Result<Option<TextSegment>, LayoutError> {
        let level = usize::from(paragraph.level.min(8));
        self.levels[level + 1..].fill_with(|| None);

        let (text, font, color, size) = match paragraph.bullet.as_ref() {
            Some(ResolvedBullet::Character {
                character,
                font,
                color,
                size,
            }) => {
                self.levels[level] = None;
                (character.clone(), font, color, size)
            }
            Some(ResolvedBullet::AutoNumber {
                scheme,
                start_at,
                font,
                color,
                size,
            }) => {
                let value = self.levels[level]
                    .as_ref()
                    .filter(|sequence| sequence.scheme == *scheme)
                    .map_or(*start_at, |sequence| sequence.next);
                self.levels[level] = Some(AutoNumberSequence {
                    scheme: scheme.clone(),
                    next: value.saturating_add(1),
                });
                (format_auto_number(scheme, value), font, color, size)
            }
            None => {
                self.levels[level] = None;
                return Ok(None);
            }
        };

        let mut marker = shape_marker(
            font_manager,
            shaping_cache,
            paragraph,
            &text,
            font,
            *color,
            size,
        )?;
        if paragraph.indent < 0.0 {
            marker.width = -paragraph.indent;
        }
        Ok(Some(marker))
    }
}

fn inline_items_with_caps(
    font_manager: &mut FontManager,
    shaping_cache: &mut ShapingCache,
    paragraph: &ResolvedParagraph,
    page_number: usize,
    requests: &mut Vec<(bool, bool, Option<f64>, Option<String>)>,
    contexts: &mut HashMap<usize, Option<String>>,
) -> Result<Vec<InlineItem>, LayoutError> {
    let mut items = Vec::with_capacity(paragraph.runs.len());
    let mut requested_context_bytes = 0usize;
    let mut requested_context_entries = 0usize;
    for run in &paragraph.runs {
        let item_start = items.len();
        match run {
            ResolvedTextRun::Text { text, style } => {
                push_displayed_text(&mut items, text, style, |line| {
                    let shaped = shaping_cache.shape(font_manager, line, style)?;
                    requests.push((
                        style.small_caps,
                        style.all_caps,
                        style.spacing,
                        style.language.clone(),
                    ));
                    Ok(shaped)
                })?;
            }
            ResolvedTextRun::Field {
                text,
                field_type,
                style,
            } => {
                let slide_number = field_type
                    .as_deref()
                    .is_some_and(|field_type| field_type.eq_ignore_ascii_case("slidenum"));
                let rendered = if slide_number {
                    page_number.to_string()
                } else {
                    text.clone()
                };
                push_displayed_text(&mut items, &rendered, style, |line| {
                    let mut segment = shaping_cache.shape(font_manager, line, style)?;
                    requests.push((
                        style.small_caps,
                        style.all_caps,
                        style.spacing,
                        style.language.clone(),
                    ));
                    if slide_number {
                        segment.field_kind = Some(FieldKind::Page);
                    }
                    Ok(segment)
                })?;
            }
            ResolvedTextRun::Break => items.push(InlineItem::LineBreak(LineClear::None)),
        }
        let style = match run {
            ResolvedTextRun::Text { style, .. } | ResolvedTextRun::Field { style, .. } => {
                Some(style)
            }
            ResolvedTextRun::Break => None,
        };
        if let Some(style) = style {
            for (index, item) in items.iter().enumerate().skip(item_start) {
                if let InlineItem::Text(segment) = item {
                    let family = font_manager.copy_font_request_context(
                        typeface_for_text(style, &segment.text),
                        &mut requested_context_bytes,
                        &mut requested_context_entries,
                    );
                    contexts.insert(index, family);
                }
            }
        }
    }
    Ok(items)
}

/// Pushes run text as PowerPoint displays it when `a:t` holds a line
/// separator. Measured with PowerPoint for Mac, each line feed and each
/// carriage return breaks the line as an `a:br` does, so a CRLF pair leaves an
/// empty line, while U+0085, U+2028 and U+2029 show as a space. U+001C to
/// U+001E, which only reach here from memory since XML cannot carry them, also
/// show as a space. No Unicode paragraph separator reaches line layout, which
/// reorders each line within one bidi paragraph.
fn push_displayed_text(
    items: &mut Vec<InlineItem>,
    text: &str,
    style: &ResolvedRunStyle,
    mut shape: impl FnMut(&str) -> Result<TextSegment, LayoutError>,
) -> Result<(), LayoutError> {
    let spaced = |character: char| {
        matches!(
            character,
            '\u{1c}'..='\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}'
        )
    };
    let displayed = if text.contains(spaced) {
        Cow::Owned(text.replace(spaced, " "))
    } else {
        Cow::Borrowed(text)
    };
    for (index, line) in displayed.split(['\n', '\r']).enumerate() {
        if index > 0 {
            items.push(InlineItem::LineBreak(LineClear::None));
        }
        if !line.is_empty() {
            // Keep shaping context within each script. Common whitespace,
            // punctuation and combining marks stay with their preceding face.
            // Identical resolved slots retain the original single-run path.
            let mut start = 0;
            let mut family = None;
            for (index, character) in line.char_indices() {
                let common = character.is_whitespace()
                    || character.is_ascii_punctuation()
                    || matches!(character as u32, 0x0300..=0x036F);
                let mut encoded = [0; 4];
                let selected = if common && index > 0 {
                    family
                } else {
                    typeface_for_text(style, character.encode_utf8(&mut encoded))
                };
                if index > start && selected != family {
                    items.push(InlineItem::Text(shape(&line[start..index])?));
                    start = index;
                }
                family = selected;
            }
            items.push(InlineItem::Text(shape(&line[start..])?));
        }
    }
    Ok(())
}

/// The characters of `text` that draw a glyph of their own: a tab, a space
/// or a variation selector is no reason to look for another face.
fn drawn_characters(text: &str) -> String {
    text.chars()
        .filter(|character| oxoffice_layout::draws_a_glyph(*character))
        .collect()
}

/// Whether `font_id` draws any of the characters of `text` that draw a
/// glyph; `true` for text with none.
fn draws_any(
    font_manager: &FontManager,
    font_id: oxoffice_layout::FontId,
    text: &str,
) -> Result<bool, LayoutError> {
    let drawn = drawn_characters(text);
    Ok(drawn.is_empty()
        || font_manager
            .shape_text(font_id, &drawn, DEFAULT_FONT_SIZE)?
            .glyph_ids
            .iter()
            .any(|glyph| *glyph != 0))
}

/// Whether a shaped run's face lacks a character of it that draws a glyph.
fn lacks_a_glyph(font_manager: &FontManager, segment: &TextSegment) -> bool {
    segment.glyph_ids.contains(&0) && {
        let drawn = drawn_characters(&segment.text);
        !drawn.is_empty()
            && font_manager
                .shape_text(segment.font_id, &drawn, segment.font_size)
                .is_ok_and(|shaped| shaped.glyph_ids.contains(&0))
    }
}

fn shape_run(
    font_manager: &mut FontManager,
    text: &str,
    style: &ResolvedRunStyle,
) -> Result<TextSegment, LayoutError> {
    let rendered_text = if style.all_caps {
        text.to_uppercase()
    } else {
        text.to_owned()
    };
    let font_size = style.font_size.unwrap_or(DEFAULT_FONT_SIZE);
    let typeface = typeface_for_text(style, &rendered_text);
    // A run keeps the face of its family when that face draws part of it:
    // the glyphs it lacks are left as .notdef, which sends the paragraph to
    // the grapheme split, where only those graphemes look for a face, a
    // system one included. A face picked for the whole run would take the
    // text the family draws too, and draw it as boxes when it has no Latin.
    // A run the family draws none of takes a face that covers it.
    let primary = font_manager.resolve_font(typeface, style.bold, style.italic)?;
    let mut font_id = primary;
    let mut shaped = font_manager.shape_text(primary, &rendered_text, font_size)?;
    if shaped.glyph_ids.contains(&0) && !draws_any(font_manager, primary, &rendered_text)? {
        font_id = font_manager.resolve_font_for_text(
            typeface,
            style.bold,
            style.italic,
            &rendered_text,
        )?;
        if font_id != primary {
            shaped = font_manager.shape_text(font_id, &rendered_text, font_size)?;
        }
    }
    let metrics = font_manager.metrics(font_id, font_size)?;
    if let Some(spacing) = style.spacing {
        for advance in &mut shaped.advances {
            *advance += spacing;
        }
        shaped.width += spacing * shaped.advances.len() as f64;
    }

    Ok(TextSegment {
        form_mark: None,
        text: text.to_owned(),
        direction: oxoffice_layout::TextDirection::Auto,
        source: None,
        font_id,
        measure_font: None,
        font_size,
        glyph_ids: shaped.glyph_ids,
        advances: shaped.advances,
        width: shaped.width,
        ascent: metrics.ascent,
        descent: metrics.descent,
        line_gap: metrics.line_gap,
        color: text_color(style.fill.as_ref()),
        bold: style.bold,
        italic: style.italic,
        underline: style.underline.then_some(Underline::Single),
        strike: style.strike,
        dstrike: false,
        highlight: style.highlight,
        baseline_offset: style.baseline.unwrap_or(0.0) * font_size,
        hyperlink_url: style.hyperlink_url.clone(),
        field_kind: None,
        field_source: None,
        note: None,
        note_reference_source: None,
    })
}

fn shape_marker(
    font_manager: &mut FontManager,
    shaping_cache: &mut ShapingCache,
    paragraph: &ResolvedParagraph,
    text: &str,
    font: &Option<String>,
    color: Option<Color>,
    size: &Option<ResolvedBulletSize>,
) -> Result<TextSegment, LayoutError> {
    let mut style = first_run_style(paragraph).clone();
    style.small_caps = false;
    style.highlight = None;
    let base_size = style.font_size.unwrap_or(DEFAULT_FONT_SIZE);
    style.font_size = Some(match size {
        Some(ResolvedBulletSize::Percent(factor)) => base_size * factor,
        Some(ResolvedBulletSize::Points(points)) => *points,
        None => base_size,
    });
    if let Some(font) = font {
        style.latin_typeface = Some(font.clone());
        style.east_asian_typeface = Some(font.clone());
        style.complex_script_typeface = Some(font.clone());
        style.symbol_typeface = Some(font.clone());
    }
    if let Some(color) = color {
        style.fill = Some(Paint::Solid(color));
    }
    let (displayed, _) = font_manager.resolve_symbol_text(
        typeface_for_text(&style, text),
        style.bold,
        style.italic,
        text,
        true,
    )?;
    shaping_cache.shape(font_manager, &displayed, &style)
}

fn first_run_style(paragraph: &ResolvedParagraph) -> &ResolvedRunStyle {
    paragraph
        .runs
        .iter()
        .find_map(|run| match run {
            ResolvedTextRun::Text { style, .. } | ResolvedTextRun::Field { style, .. } => {
                Some(style)
            }
            ResolvedTextRun::Break => None,
        })
        .unwrap_or(&DEFAULT_RUN_STYLE)
}

static DEFAULT_RUN_STYLE: ResolvedRunStyle = ResolvedRunStyle {
    font_size: None,
    bold: false,
    italic: false,
    all_caps: false,
    small_caps: false,
    language: None,
    highlight: None,
    underline: false,
    strike: false,
    spacing: None,
    baseline: None,
    fill: None,
    latin_typeface: None,
    east_asian_typeface: None,
    complex_script_typeface: None,
    symbol_typeface: None,
    hyperlink_url: None,
};

fn format_auto_number(scheme: &str, value: u32) -> String {
    match scheme {
        "arabicPlain" => value.to_string(),
        "arabicParenR" => format!("{value})"),
        "arabicParenBoth" => format!("({value})"),
        "alphaLcPeriod" => format!("{}.", alphabetic_number(value, false)),
        "alphaUcPeriod" => format!("{}.", alphabetic_number(value, true)),
        "romanLcPeriod" => format!("{}.", roman_number(value, false)),
        "romanUcPeriod" => format!("{}.", roman_number(value, true)),
        "arabicPeriod" => format!("{value}."),
        _ => format!("{value}."),
    }
}

fn alphabetic_number(value: u32, uppercase: bool) -> String {
    let mut value = value.max(1);
    let mut letters = Vec::new();
    while value > 0 {
        value -= 1;
        let base = if uppercase { b'A' } else { b'a' };
        letters.push(char::from(base + (value % 26) as u8));
        value /= 26;
    }
    letters.into_iter().rev().collect()
}

fn roman_number(value: u32, uppercase: bool) -> String {
    let mut value = value.max(1);
    let mut result = String::new();
    for (amount, numeral) in [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ] {
        while value >= amount {
            result.push_str(numeral);
            value -= amount;
        }
    }
    if uppercase {
        result
    } else {
        result.to_ascii_lowercase()
    }
}

fn typeface_for_text<'a>(style: &'a ResolvedRunStyle, text: &str) -> Option<&'a str> {
    let preferred = if text.chars().any(is_symbol_character) {
        style.symbol_typeface.as_deref()
    } else if text.chars().any(is_east_asian_character) {
        style.east_asian_typeface.as_deref()
    } else if text.chars().any(is_complex_script_character) {
        style.complex_script_typeface.as_deref()
    } else {
        style.latin_typeface.as_deref()
    };
    preferred
        .or(style.latin_typeface.as_deref())
        .or(style.east_asian_typeface.as_deref())
        .or(style.complex_script_typeface.as_deref())
        .or(style.symbol_typeface.as_deref())
}

fn is_symbol_character(character: char) -> bool {
    matches!(character as u32, 0xE000..=0xF8FF)
}

fn is_east_asian_character(character: char) -> bool {
    matches!(
        character as u32,
        0x1100..=0x11FF
            | 0x2E80..=0xA4CF
            | 0xAC00..=0xD7AF
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFFEF
            | 0x20000..=0x323AF
    )
}

fn is_complex_script_character(character: char) -> bool {
    matches!(
        character as u32,
        0x0590..=0x10FF | 0x1780..=0x17FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF
    )
}

fn text_color(fill: Option<&Paint>) -> Color {
    match fill {
        Some(Paint::Solid(color)) => *color,
        Some(Paint::Linear { stops, .. }) | Some(Paint::Radial { stops, .. }) => {
            stops.first().map_or(Color::BLACK, |stop| stop.color)
        }
        Some(Paint::Grid { colors, .. }) => colors.first().copied().unwrap_or(Color::BLACK),
        Some(Paint::Tile { .. }) | None => Color::BLACK,
    }
}

pub(super) struct StackedText {
    pub(super) elements: Vec<PositionedElement>,
    /// One entry per laid-out line, anchored like `elements`.
    pub(super) lines: Vec<TextLineLayout>,
    line_ranges: Vec<(usize, usize)>,
    pub(super) width: f64,
    pub(super) height: f64,
    pub(super) font_scale: f64,
    pub(super) width_fits: bool,
    pub(super) diagnostics: Vec<String>,
}

impl StackedText {
    /// Applies the fit test that normal autofit uses to accept a candidate.
    pub(super) fn fits(&self, content: Rect) -> bool {
        self.width_fits && self.height <= content.height + 0.01
    }

    /// Explicit authoring measures unanchored physical participating segments.
    /// Ordinary rendering continues using summed line advances in `fits`.
    pub(super) fn authoring_extent(
        &self,
        fonts: &mut FontManager,
        text: &ResolvedTextBody,
        content: Rect,
    ) -> Result<(f64, bool), LayoutError> {
        let mut bottom = content.y;
        for (index, (line, &(start, end))) in self.lines.iter().zip(&self.line_ranges).enumerate() {
            let mut line_bottom = line.baseline;
            let mut physical = false;
            for element in &self.elements[start..end] {
                match element {
                    PositionedElement::Text(run) => {
                        physical = true;
                        line_bottom = line_bottom
                            .max(run.origin.y + fonts.metrics(run.font_id, run.font_size)?.descent);
                    }
                    PositionedElement::MultilingualText(run) => {
                        physical = true;
                        let descent = fonts.metrics(run.font_id, run.font_size)?.descent;
                        let mut pen_y = 0.0;
                        for (offset, advance) in run.y_offsets.iter().zip(&run.y_advances) {
                            line_bottom = line_bottom.max(run.origin.y - pen_y - offset + descent);
                            pen_y += advance;
                        }
                    }
                    _ => {}
                }
            }
            if !physical {
                let style = &text.paragraphs[line.paragraph_index].end_style;
                let id = fonts.resolve_font_for_metrics(
                    style.latin_typeface.as_deref(),
                    style.bold,
                    style.italic,
                )?;
                line_bottom = line.baseline + fonts.metrics(id, line.font_size)?.descent;
            }
            if index + 1 == self.lines.len() && text.space_first_last_paragraph {
                let paragraph = &text.paragraphs[line.paragraph_index];
                line_bottom += paragraph_spacing(
                    paragraph.space_after.as_ref(),
                    first_run_font_size(paragraph) * self.font_scale,
                );
            }
            bottom = bottom.max(line_bottom);
        }
        let extent = (bottom - content.y).max(0.0);
        if !extent.is_finite() {
            return Err(LayoutError::Layout(
                "autofit physical extent is non-finite".to_owned(),
            ));
        }
        Ok((extent, self.width_fits && extent <= content.height + 0.01))
    }
}

pub(super) fn stack_text_for_page_with_directions(
    font_manager: &mut FontManager,
    content: Rect,
    text: &ResolvedTextBody,
    page_number: usize,
    paragraph_directions: &[oxoffice_layout::TextDirection],
) -> Result<StackedText, LayoutError> {
    let mut diagnostics = Vec::new();
    let shaped_paragraphs = shape_paragraphs(
        font_manager,
        text,
        page_number,
        paragraph_directions,
        &mut diagnostics,
    )?;
    let mut stacked = (|| match text.autofit {
        ResolvedAutofit::None | ResolvedAutofit::Shape => {
            stack_shaped_text(font_manager, content, text, &shaped_paragraphs, 1.0, None)
        }
        ResolvedAutofit::Normal {
            font_scale: None,
            line_spacing_reduction: None,
        } => {
            let mut floor =
                stack_shaped_text(font_manager, content, text, &shaped_paragraphs, 1.0, None)?;
            if floor.fits(content) {
                return Ok(floor);
            }
            for step in 1..=30 {
                let scale = 1.0 - f64::from(step) * 0.025;
                let candidate = stack_shaped_text(
                    font_manager,
                    content,
                    text,
                    &shaped_paragraphs,
                    scale,
                    None,
                )?;
                if candidate.fits(content) {
                    return Ok(candidate);
                }
                floor = candidate;
            }
            Ok(floor)
        }
        ResolvedAutofit::Normal {
            font_scale,
            line_spacing_reduction,
        } => stack_shaped_text(
            font_manager,
            content,
            text,
            &shaped_paragraphs,
            font_scale.unwrap_or(1.0),
            line_spacing_reduction,
        ),
    })()?;
    stacked.diagnostics = diagnostics;
    Ok(stacked)
}

fn shape_paragraphs(
    font_manager: &mut FontManager,
    text: &ResolvedTextBody,
    page_number: usize,
    paragraph_directions: &[oxoffice_layout::TextDirection],
    diagnostics: &mut Vec<String>,
) -> Result<Vec<(Vec<InlineItem>, oxoffice_layout::TextDirection)>, LayoutError> {
    let mut numbering = NumberingState::default();
    let mut shaping_cache = ShapingCache::default();
    text.paragraphs
        .iter()
        .enumerate()
        .map(|(paragraph_index, paragraph)| {
            let base_direction = paragraph_directions
                .get(paragraph_index)
                .copied()
                .unwrap_or(oxoffice_layout::TextDirection::Auto);
            let marker = numbering.marker(font_manager, &mut shaping_cache, paragraph)?;
            let mut requests = Vec::new();
            let mut contexts = HashMap::new();
            let legacy_items = inline_items_with_caps(font_manager, &mut shaping_cache, paragraph, page_number, &mut requests, &mut contexts)?;
            let uses_rich_text = requests.iter().any(|(small, all, _, _)| *small || *all) || base_direction != oxoffice_layout::TextDirection::Auto
                || legacy_items.iter().any(|item| {
                    // A run its face cannot wholly draw is split too, so each
                    // grapheme it lacks finds a face of its own and the rest
                    // keeps the run's face.
                    matches!(item, InlineItem::Text(segment)
                        if needs_multilingual_layout(&segment.text) || lacks_a_glyph(font_manager, segment))
                });
            let mut items = if uses_rich_text {
                shape_multilingual_items_preserving_controls(
                    font_manager,
                    legacy_items,
                    base_direction,
                    &requests,
                    diagnostics,
                    &contexts,
                )?
            } else {
                for (index, item) in legacy_items.iter().enumerate() {
                    if let InlineItem::Text(segment) = item {
                        font_manager.record_font_contribution(segment.font_id,
                            contexts.get(&index).and_then(|family| family.as_deref()), &segment.text)?;
                    }
                }
                legacy_items
            };
            let has_visible_text = items.iter().any(|item| match item {
                InlineItem::Text(segment) => !segment.text.is_empty(),
                InlineItem::MultilingualText(segment) => !segment.text().is_empty(),
                _ => false,
            });
            if items.is_empty() {
                items.push(InlineItem::Text(shaping_cache.shape(
                    font_manager,
                    "",
                    &paragraph.end_style,
                )?));
            }
            if let Some(marker) = marker.filter(|_| has_visible_text) {
                let explicit = match paragraph.bullet.as_ref() {
                    Some(ResolvedBullet::Character { font, .. })
                    | Some(ResolvedBullet::AutoNumber { font, .. }) => font.as_deref(),
                    None => None,
                };
                let requested = explicit.or_else(|| typeface_for_text(first_run_style(paragraph), &marker.text));
                font_manager.record_font_contribution(marker.font_id, requested, &marker.text)?;
                if let Some(ResolvedBullet::Character { character, .. }) = &paragraph.bullet {
                    append_symbol_marker_diagnostics(
                        font_manager,
                        requested,
                        character,
                        &marker,
                        diagnostics,
                    );
                }
                items.insert(0, InlineItem::Marker(marker));
            }
            Ok((items, base_direction))
        })
        .collect()
}

// Unknown semantic mappings are distinct from glyph coverage. A fallback face's
// private-use glyph does not authenticate the requested symbol's meaning.
fn append_symbol_marker_diagnostics(
    fonts: &FontManager,
    requested: Option<&str>,
    original: &str,
    marker: &TextSegment,
    diagnostics: &mut Vec<String>,
) {
    for message in
        fonts.unknown_symbol_diagnostics(marker.font_id, requested, original, &marker.text)
    {
        // At most 1024 canonical family/original-code pairs (512 per family).
        // Only accepted visible markers enter this local layout vector.
        if !diagnostics.contains(&message) {
            diagnostics.push(message);
        }
    }
}

fn shape_multilingual_items_preserving_controls(
    font_manager: &mut FontManager,
    legacy_items: Vec<InlineItem>,
    base_direction: oxoffice_layout::TextDirection,
    requests: &[(bool, bool, Option<f64>, Option<String>)],
    diagnostics: &mut Vec<String>,
    contexts: &HashMap<usize, Option<String>>,
) -> Result<Vec<InlineItem>, LayoutError> {
    let mut occurrence_languages = requests.iter();
    let styled = legacy_items
        .iter()
        .filter_map(|item| match item {
            InlineItem::Text(segment) => Some((
                segment.clone(),
                occurrence_languages
                    .next()
                    .and_then(|(_, _, _, language)| language.clone()),
            )),
            _ => None,
        })
        .collect();
    let (spans, fallback_diagnostics) = font_manager.shape_multilingual_paragraph_with_caps(
        styled,
        base_direction,
        false,
        &requests
            .iter()
            .map(|(small, all, _, _)| (*small, *all))
            .collect::<Vec<_>>(),
    )?;
    diagnostics.extend(fallback_diagnostics);
    let mut shaped = std::collections::VecDeque::from(spans);
    let mut items = Vec::new();
    let mut occurrence = 0usize;
    for (index, item) in legacy_items.into_iter().enumerate() {
        let requested = contexts.get(&index).and_then(|family| family.as_deref());
        let InlineItem::Text(segment) = item else {
            items.push(item);
            continue;
        };
        let spacing = requests
            .get(occurrence)
            .and_then(|(_, _, spacing, _)| *spacing);
        occurrence += 1;
        let target_bytes = segment.text.len();
        let mut consumed_bytes = 0usize;
        while consumed_bytes < target_bytes {
            let Some(mut span) = shaped.pop_front() else {
                return Err(LayoutError::Layout(
                    "multilingual paragraph shaping lost a styled text run".to_owned(),
                ));
            };
            consumed_bytes = consumed_bytes
                .checked_add(span.text().len())
                .filter(|consumed| *consumed <= target_bytes)
                .ok_or_else(|| {
                    LayoutError::Layout(
                        "multilingual paragraph shaping crossed a styled text boundary".to_owned(),
                    )
                })?;
            if let Some(spacing) = spacing {
                let mut base = span.base().clone();
                let advances = span
                    .x_advances()
                    .iter()
                    .map(|advance| advance + spacing)
                    .collect::<Vec<_>>();
                base.advances = advances.clone();
                base.width = advances.iter().sum();
                span = oxoffice_layout::MultilingualTextSegment::new(
                    base,
                    span.logical_index(),
                    span.language().map(str::to_owned),
                    span.script(),
                    span.direction(),
                    span.bidi_level(),
                    advances,
                    span.y_advances().to_vec(),
                    span.x_offsets().to_vec(),
                    span.y_offsets().to_vec(),
                    span.clusters().to_vec(),
                    span.break_after(),
                )?;
            }
            font_manager.record_font_contribution(span.base().font_id, requested, span.text())?;
            items.push(InlineItem::MultilingualText(span));
        }
    }
    if !shaped.is_empty() {
        return Err(LayoutError::Layout(
            "multilingual paragraph shaping produced an unmatched text run".to_owned(),
        ));
    }
    Ok(items)
}

/// Whether a run is shaped by grapheme, each taking a face that draws it.
///
/// Every East Asian character is, Hangul and the fullwidth forms included,
/// so that a face found for the characters the bundled fonts lack draws only
/// those, and the text beside them keeps its bundled face.
fn needs_multilingual_layout(text: &str) -> bool {
    text.chars().any(|character| {
        is_east_asian_character(character)
            || matches!(
                character as u32,
                0x0590..=0x08ff | 0x0900..=0x097f | 0x0e00..=0x0e7f
            )
    })
}

fn stack_shaped_text(
    font_manager: &FontManager,
    content: Rect,
    text: &ResolvedTextBody,
    shaped_paragraphs: &[(Vec<InlineItem>, oxoffice_layout::TextDirection)],
    font_scale: f64,
    line_spacing_reduction: Option<f64>,
) -> Result<StackedText, LayoutError> {
    let mut elements = Vec::new();
    let mut line_ranges = Vec::new();
    let mut laid_out_lines = Vec::new();
    let mut y = content.y;
    let mut width = 0.0_f64;
    let mut width_fits = true;

    let last_paragraph = text.paragraphs.len().saturating_sub(1);
    for (index, (paragraph, (shaped_items, base_direction))) in
        text.paragraphs.iter().zip(shaped_paragraphs).enumerate()
    {
        let font_size = first_run_font_size(paragraph) * font_scale;
        if index > 0 || text.space_first_last_paragraph {
            y += paragraph_spacing(paragraph.space_before.as_ref(), font_size);
        }
        let items = scaled_inline_items(shaped_items, font_scale, paragraph.indent);
        let params = line_break_params(
            paragraph,
            content.width,
            text.wrap,
            line_spacing_reduction.unwrap_or(0.0),
        );
        let lines = break_multilingual_into_lines(&items, &params, font_manager, *base_direction)?;

        for line in &lines {
            width_fits &= ink_width(line) <= line.available_width + 0.01;
            let line_size = line_font_size(line).unwrap_or(font_size);
            let baseline = y + baseline_offset(line, SINGLE_SPACING_EM * line_size);
            let element_start = elements.len();
            let (start_x, occupied_width) = emit_line_items(
                line,
                paragraph.alignment,
                content.x,
                baseline,
                &mut elements,
            );
            line_ranges.push((element_start, elements.len()));
            laid_out_lines.push(TextLineLayout {
                paragraph_index: index,
                text: line_text(line),
                bounds: Rect {
                    x: start_x,
                    y,
                    width: occupied_width,
                    height: line.height,
                },
                baseline,
                font_size: line_size,
            });
            width = width.max(occupied_width);
            y += line.height;
        }

        if index < last_paragraph || text.space_first_last_paragraph {
            y += paragraph_spacing(paragraph.space_after.as_ref(), font_size);
        }
    }

    let height = y - content.y;
    anchor_lines(
        &mut elements,
        &mut laid_out_lines,
        &line_ranges,
        text.anchor,
        content.height - height,
    );

    Ok(StackedText {
        elements,
        lines: laid_out_lines,
        line_ranges,
        width,
        height,
        font_scale,
        width_fits,
        diagnostics: Vec::new(),
    })
}

/// Returns a line's run and field text without its marker.
///
/// Rich spans are ordered by their logical index, as `emit_line_items` orders
/// them for extraction. A paragraph shapes either every run on the rich path or
/// none, so legacy runs never share a line with rich spans and keep line order.
fn line_text(line: &LayoutLine) -> String {
    let mut spans = line
        .items
        .iter()
        .enumerate()
        .filter_map(|(position, item)| match item {
            LineItem::Text(segment) => Some((position, segment.text.as_str())),
            LineItem::MultilingualText(segment) => Some((segment.logical_index(), segment.text())),
            _ => None,
        })
        .collect::<Vec<_>>();
    spans.sort_by_key(|(order, _)| *order);
    spans.into_iter().map(|(_, text)| text).collect()
}

/// Returns the widths of the spaces hanging off a line's visual left and
/// right, which alignment and the fit test leave out.
fn hanging_widths(line: &LayoutLine) -> (f64, f64) {
    let (start, end) = line.hanging_space_counts();
    let left = line.items[..start].iter().map(LineItem::width).sum();
    let right = line.items[line.items.len() - end..]
        .iter()
        .map(LineItem::width)
        .sum();
    (left, right)
}

/// Returns a line's width without the spaces hanging off either side.
fn ink_width(line: &LayoutLine) -> f64 {
    let (left, right) = hanging_widths(line);
    line.width - left - right
}

/// Returns the largest scaled run size on a line, ignoring its marker.
fn line_font_size(line: &LayoutLine) -> Option<f64> {
    line.items
        .iter()
        .filter_map(|item| match item {
            LineItem::Text(segment) => Some(segment.font_size),
            LineItem::MultilingualText(segment) => Some(segment.base().font_size),
            _ => None,
        })
        .reduce(f64::max)
}

fn scaled_inline_items(items: &[InlineItem], scale: f64, indent: f64) -> Vec<InlineItem> {
    items
        .iter()
        .cloned()
        .map(|item| match item {
            InlineItem::Text(mut segment) => {
                scale_segment(&mut segment, scale);
                InlineItem::Text(segment)
            }
            InlineItem::MultilingualText(segment) => {
                let mut base = segment.base().clone();
                scale_segment(&mut base, scale);
                InlineItem::MultilingualText(
                    oxoffice_layout::MultilingualTextSegment::new(
                        base,
                        segment.logical_index(),
                        segment.language().map(str::to_owned),
                        segment.script(),
                        segment.direction(),
                        segment.bidi_level(),
                        segment
                            .x_advances()
                            .iter()
                            .map(|value| value * scale)
                            .collect(),
                        segment
                            .y_advances()
                            .iter()
                            .map(|value| value * scale)
                            .collect(),
                        segment
                            .x_offsets()
                            .iter()
                            .map(|value| value * scale)
                            .collect(),
                        segment
                            .y_offsets()
                            .iter()
                            .map(|value| value * scale)
                            .collect(),
                        segment.clusters().to_vec(),
                        segment.break_after(),
                    )
                    .expect("scaling preserves multilingual segment invariants"),
                )
            }
            InlineItem::Marker(mut segment) => {
                scale_segment(&mut segment, scale);
                if indent < 0.0 {
                    segment.width = -indent;
                }
                InlineItem::Marker(segment)
            }
            other => other,
        })
        .collect()
}

fn scale_segment(segment: &mut TextSegment, scale: f64) {
    segment.font_size *= scale;
    segment.width *= scale;
    segment.ascent *= scale;
    segment.descent *= scale;
    segment.line_gap *= scale;
    segment.baseline_offset *= scale;
    for advance in &mut segment.advances {
        *advance *= scale;
    }
}

/// Distance from a line box's top to its baseline, where PowerPoint puts it.
///
/// A line taller than single spacing puts its baseline three quarters of the
/// way down, or at the font's ascent share of the line when that is smaller. A
/// line no taller than single spacing puts its descent at the bottom of the
/// line, so it gives up space above the glyphs first, unless that lifts the
/// baseline above the same share.
fn baseline_offset(line: &LayoutLine, single_height: f64) -> f64 {
    let glyph_height = line.ascent + line.descent;
    let share = if glyph_height > 0.0 {
        (line.ascent / glyph_height).min(0.75)
    } else {
        0.75
    };
    let proportional = line.height * share;
    if line.height > single_height + 0.01 {
        proportional
    } else {
        proportional.max(line.height - line.descent)
    }
}

fn anchor_lines(
    elements: &mut [PositionedElement],
    lines: &mut [TextLineLayout],
    line_ranges: &[(usize, usize)],
    anchor: TextAnchor,
    spare_height: f64,
) {
    let line_count = line_ranges.len();
    for (line_index, (&(start, end), line)) in line_ranges.iter().zip(lines).enumerate() {
        let offset = match anchor {
            TextAnchor::Top => 0.0,
            TextAnchor::Center => spare_height / 2.0,
            TextAnchor::Bottom => spare_height,
            TextAnchor::Justified if spare_height > 0.0 && line_count > 1 => {
                spare_height * line_index as f64 / (line_count - 1) as f64
            }
            TextAnchor::Distributed if spare_height > 0.0 && line_count > 1 => {
                spare_height * (line_index as f64 + 0.5) / line_count as f64
            }
            TextAnchor::Justified | TextAnchor::Distributed
                if spare_height > 0.0 && line_count == 1 =>
            {
                spare_height / 2.0
            }
            TextAnchor::Justified | TextAnchor::Distributed => 0.0,
        };
        for element in &mut elements[start..end] {
            translate_element_y(element, offset);
        }
        line.bounds.y += offset;
        line.baseline += offset;
    }
}

fn translate_element_y(element: &mut PositionedElement, offset: f64) {
    match element {
        PositionedElement::Text(run) => run.origin.y += offset,
        PositionedElement::MultilingualText(run) => run.origin.y += offset,
        PositionedElement::Line { start, end, .. } => {
            start.y += offset;
            end.y += offset;
        }
        PositionedElement::FilledRect { rect, .. }
        | PositionedElement::Image { rect, .. }
        | PositionedElement::LinkAnnotation { rect, .. } => rect.y += offset,
        PositionedElement::Group(group) => group.transform.f += offset,
        PositionedElement::Path(_) => {}
        _ => {}
    }
}

/// `line_spacing_reduction` is a stored `a:normAutofit/@lnSpcReduction`. It
/// comes off the percentage, and omitted spacing counts as 100 percent.
fn line_break_params(
    paragraph: &ResolvedParagraph,
    available_width: f64,
    wrap: bool,
    line_spacing_reduction: f64,
) -> LineBreakParams {
    let (ind_first_line, ind_hanging) = if paragraph.indent < 0.0 {
        (0.0, -paragraph.indent)
    } else {
        (paragraph.indent, 0.0)
    };
    LineBreakParams {
        // Slide text has nothing floating beside it to flow around.
        line_prefix_widths: Vec::new(),
        line_suffix_widths: Vec::new(),
        available_width,
        ind_left: paragraph.left_margin,
        ind_right: paragraph.right_margin,
        ind_first_line,
        ind_hanging,
        first_line_at_right: false,
        tab_stops: Vec::new(),
        line_spacing: match paragraph.line_spacing.as_ref() {
            Some(ResolvedTextSpacing::Percent(factor)) => LineSpacing::Multiple(
                SINGLE_SPACING_EM * (factor - line_spacing_reduction).max(0.0),
            ),
            Some(ResolvedTextSpacing::Points(points)) => LineSpacing::Exact(*points),
            None => {
                LineSpacing::Multiple(SINGLE_SPACING_EM * (1.0 - line_spacing_reduction).max(0.0))
            }
        },
        jc: Some(paragraph_align(paragraph.alignment)),
        wrap,
        // Slide text has no document-level tab interval, so it keeps the
        // half-inch default this field replaced.
        default_tab_interval_pt: 36.0,
        clamp_tabs_past_margin: false,
    }
}

fn paragraph_align(alignment: ParagraphAlignment) -> Align {
    match alignment {
        ParagraphAlignment::Left => Align::Start,
        ParagraphAlignment::Center => Align::Center,
        ParagraphAlignment::Right => Align::End,
        ParagraphAlignment::Justified => Align::Justify,
        ParagraphAlignment::Distributed => Align::Distribute,
    }
}

fn first_run_font_size(paragraph: &ResolvedParagraph) -> f64 {
    paragraph
        .runs
        .iter()
        .find_map(|run| match run {
            ResolvedTextRun::Text { style, .. } | ResolvedTextRun::Field { style, .. } => {
                Some(style.font_size.unwrap_or(DEFAULT_FONT_SIZE))
            }
            ResolvedTextRun::Break => None,
        })
        .unwrap_or_else(|| paragraph.end_style.font_size.unwrap_or(DEFAULT_FONT_SIZE))
}

/// A percentage is a share of single spacing, as for line spacing.
fn paragraph_spacing(spacing: Option<&ResolvedTextSpacing>, font_size: f64) -> f64 {
    match spacing {
        Some(ResolvedTextSpacing::Percent(factor)) => SINGLE_SPACING_EM * font_size * factor,
        Some(ResolvedTextSpacing::Points(points)) => *points,
        None => 0.0,
    }
}

/// Emits one line and returns its start x and the width its items occupy,
/// hanging spaces included.
fn emit_line_items(
    line: &LayoutLine,
    alignment: ParagraphAlignment,
    content_x: f64,
    baseline: f64,
    elements: &mut Vec<PositionedElement>,
) -> (f64, f64) {
    let element_start = elements.len();
    let (hanging_left, _) = hanging_widths(line);
    let (hang_start, hang_end) = line.hanging_space_counts();
    // Spaces hanging past the line end are not gaps to widen.
    let ink = hang_start..line.items.len() - hang_end;
    let remaining = line.available_width - ink_width(line);
    let distribute = match alignment {
        ParagraphAlignment::Justified if !line.is_last => {
            let gaps = word_gap_count(&line.items[ink.clone()]);
            (gaps > 0).then_some((remaining.max(0.0) / gaps as f64, false))
        }
        ParagraphAlignment::Distributed => {
            let gaps = glyph_gap_count(&line.items[ink.clone()]);
            (gaps > 0).then_some((remaining.max(0.0) / gaps as f64, true))
        }
        _ => None,
    };
    let alignment_offset = match alignment {
        ParagraphAlignment::Center => remaining / 2.0,
        ParagraphAlignment::Right => remaining,
        _ => 0.0,
    };
    // Spaces hanging off the visual left sit outside the aligned ink.
    let mut x = content_x + line.indent_left + alignment_offset - hanging_left;
    let start_x = x;
    let mut distributed_gaps = distribute
        .filter(|(_, every_glyph)| *every_glyph)
        .map_or(0, |_| glyph_gap_count(&line.items[ink.clone()]));

    for (index, item) in line.items.iter().enumerate() {
        match item {
            LineItem::Text(segment) => {
                let (advances, effective_width) = distributed_advances(
                    segment,
                    distribute.filter(|_| ink.contains(&index)),
                    &mut distributed_gaps,
                );
                emit_segment(
                    segment,
                    advances,
                    effective_width,
                    x,
                    baseline,
                    line.height,
                    elements,
                );
                x += effective_width;
            }
            LineItem::MultilingualText(segment) => {
                emit_multilingual_segment(segment, x, baseline, line.height, elements);
                x += segment.width();
            }
            LineItem::Marker(segment) => {
                emit_segment(
                    segment,
                    segment.advances.clone(),
                    segment.width,
                    x,
                    baseline,
                    line.height,
                    elements,
                );
                x += segment.width;
            }
            LineItem::Tab { width, leader, .. } => {
                if let Some(segment) = leader {
                    emit_segment(
                        segment,
                        segment.advances.clone(),
                        *width,
                        x,
                        baseline,
                        line.height,
                        elements,
                    );
                }
                x += width;
            }
            LineItem::Image { width, .. } => x += width,
            LineItem::Group { width, .. } => x += width,
            _ => x += item.width(),
        }
    }
    let rich_positions = (element_start..elements.len())
        .filter(|index| matches!(elements[*index], PositionedElement::MultilingualText(_)))
        .collect::<Vec<_>>();
    let mut logical_runs = rich_positions
        .iter()
        .filter_map(|index| match &elements[*index] {
            PositionedElement::MultilingualText(run) => Some(run.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    logical_runs.sort_by_key(|run| run.logical_index);
    for (position, run) in rich_positions.into_iter().zip(logical_runs) {
        elements[position] = PositionedElement::MultilingualText(run);
    }
    (start_x, x - start_x)
}

fn emit_segment(
    segment: &TextSegment,
    advances: Vec<f64>,
    effective_width: f64,
    x: f64,
    baseline: f64,
    line_height: f64,
    elements: &mut Vec<PositionedElement>,
) {
    if segment.text.is_empty() {
        return;
    }
    let adjusted_baseline = baseline - segment.baseline_offset;
    if let Some(color) = segment.highlight {
        elements.push(PositionedElement::FilledRect {
            rect: Rect {
                x,
                y: adjusted_baseline - segment.ascent,
                width: effective_width,
                height: line_height,
            },
            color,
        });
    }
    elements.push(PositionedElement::Text(GlyphRun {
        origin: Point {
            x,
            y: adjusted_baseline,
        },
        font_id: segment.font_id,
        font_size: segment.font_size,
        glyph_ids: segment.glyph_ids.clone(),
        advances,
        text: segment.text.clone(),
        source: None,
        color: segment.color,
        bold: segment.bold,
        italic: segment.italic,
        field_kind: segment.field_kind,
        field_source: segment.field_source,
        note: segment.note,
        note_reference_source: segment.note_reference_source,
        tab_aligned: None,
    }));

    if segment.underline.is_some() {
        let y = adjusted_baseline + segment.descent * 0.3;
        elements.push(PositionedElement::Line {
            start: Point { x, y },
            end: Point {
                x: x + effective_width,
                y,
            },
            width: segment.font_size / 18.0,
            color: segment.color,
            dash_pattern: None,
        });
    }
    if segment.strike {
        let y = adjusted_baseline - segment.ascent * 0.3;
        elements.push(PositionedElement::Line {
            start: Point { x, y },
            end: Point {
                x: x + effective_width,
                y,
            },
            width: segment.font_size / 24.0,
            color: segment.color,
            dash_pattern: None,
        });
    }
    if let Some(url) = &segment.hyperlink_url {
        elements.push(PositionedElement::LinkAnnotation {
            rect: Rect {
                x,
                y: adjusted_baseline - segment.ascent,
                width: effective_width,
                height: line_height,
            },
            url: url.clone(),
        });
    }
}

fn emit_multilingual_segment(
    segment: &oxoffice_layout::MultilingualTextSegment,
    x: f64,
    baseline: f64,
    line_height: f64,
    elements: &mut Vec<PositionedElement>,
) {
    let base = segment.base();
    if base.text.is_empty() {
        return;
    }
    let adjusted_baseline = baseline - base.baseline_offset;
    if let Some(color) = base.highlight {
        elements.push(PositionedElement::FilledRect {
            rect: Rect {
                x,
                y: adjusted_baseline - base.ascent,
                width: segment.width(),
                height: line_height,
            },
            color,
        });
    }
    elements.push(PositionedElement::MultilingualText(MultilingualGlyphRun {
        origin: Point {
            x,
            y: adjusted_baseline,
        },
        font_id: segment.font_id(),
        font_size: base.font_size,
        glyph_ids: segment.glyph_ids().to_vec(),
        x_advances: segment.x_advances().to_vec(),
        y_advances: segment.y_advances().to_vec(),
        x_offsets: segment.x_offsets().to_vec(),
        y_offsets: segment.y_offsets().to_vec(),
        clusters: segment.clusters().to_vec(),
        logical_text: segment.text().to_owned(),
        logical_index: segment.logical_index(),
        source: base.source,
        script: segment.script(),
        language: segment.language().map(str::to_owned),
        direction: segment.direction(),
        bidi_level: segment.bidi_level(),
        color: base.color,
        bold: base.bold,
        italic: base.italic,
        field_kind: base.field_kind,
        field_source: base.field_source,
        note: base.note,
        note_reference_source: base.note_reference_source,
    }));

    if base.underline.is_some() {
        let y = adjusted_baseline + base.descent * 0.3;
        elements.push(PositionedElement::Line {
            start: Point { x, y },
            end: Point {
                x: x + segment.width(),
                y,
            },
            width: base.font_size / 18.0,
            color: base.color,
            dash_pattern: None,
        });
    }
    if base.strike {
        let y = adjusted_baseline - base.ascent * 0.3;
        elements.push(PositionedElement::Line {
            start: Point { x, y },
            end: Point {
                x: x + segment.width(),
                y,
            },
            width: base.font_size / 24.0,
            color: base.color,
            dash_pattern: None,
        });
    }
    if let Some(url) = &base.hyperlink_url {
        elements.push(PositionedElement::LinkAnnotation {
            rect: Rect {
                x,
                y: adjusted_baseline - base.ascent,
                width: segment.width(),
                height: line_height,
            },
            url: url.clone(),
        });
    }
}

fn word_gap_count(items: &[LineItem]) -> usize {
    items
        .iter()
        .filter_map(text_segment)
        .map(|segment| {
            segment
                .text
                .chars()
                .filter(|character| character.is_whitespace())
                .count()
        })
        .sum()
}

fn glyph_gap_count(items: &[LineItem]) -> usize {
    items
        .iter()
        .filter_map(text_segment)
        .map(|segment| segment.advances.len())
        .sum::<usize>()
        .saturating_sub(1)
}

fn text_segment(item: &LineItem) -> Option<&TextSegment> {
    match item {
        LineItem::Text(segment) => Some(segment),
        LineItem::MultilingualText(segment) => Some(segment.base()),
        LineItem::Marker(_)
        | LineItem::Tab { .. }
        | LineItem::Image { .. }
        | LineItem::Group { .. } => None,
        _ => None,
    }
}

fn distributed_advances(
    segment: &TextSegment,
    distribution: Option<(f64, bool)>,
    distributed_gaps: &mut usize,
) -> (Vec<f64>, f64) {
    let Some((extra, every_glyph)) = distribution else {
        return (segment.advances.clone(), segment.width);
    };
    let mut advances = segment.advances.clone();
    let mut additions = 0_usize;
    if every_glyph {
        for advance in &mut advances {
            if *distributed_gaps == 0 {
                break;
            }
            *advance += extra;
            additions += 1;
            *distributed_gaps -= 1;
        }
    } else {
        let characters = segment.text.chars().collect::<Vec<_>>();
        if characters.len() == advances.len() {
            for (character, advance) in characters.into_iter().zip(&mut advances) {
                if character.is_whitespace() {
                    *advance += extra;
                    additions += 1;
                }
            }
        } else if let Some(last) = advances.last_mut() {
            let whitespace_count = characters
                .into_iter()
                .filter(|character| character.is_whitespace())
                .count();
            if whitespace_count > 0 {
                *last += extra * whitespace_count as f64;
                additions = whitespace_count;
            }
        }
    }
    (advances, segment.width + extra * additions as f64)
}

pub(super) fn content_box(shape: &ResolvedShape, text: &ResolvedTextBody) -> Rect {
    let boundary = text_boundary(shape);
    Rect {
        x: boundary.x + text.insets.left,
        y: boundary.y + text.insets.top,
        width: (boundary.width - text.insets.left - text.insets.right).max(0.0),
        height: (boundary.height - text.insets.top - text.insets.bottom).max(0.0),
    }
}

fn text_boundary(shape: &ResolvedShape) -> Rect {
    match &shape.geometry {
        oxoffice_pptx_layout::ResolvedGeometry::Custom {
            text_rect: Some(rect),
            ..
        } => *rect,
        _ => Rect {
            x: 0.0,
            y: 0.0,
            width: shape.bounds.width,
            height: shape.bounds.height,
        },
    }
}

pub(super) fn oriented_content_box(
    content: Rect,
    direction: TextDirection,
) -> (Rect, Option<oxoffice_layout::Transform>) {
    let degrees = match direction {
        TextDirection::Horizontal => return (content, None),
        TextDirection::Vertical
        | TextDirection::EastAsianVertical
        | TextDirection::WordArtVertical => 90.0,
        TextDirection::Vertical270
        | TextDirection::MongolianVertical
        | TextDirection::WordArtVerticalRtl => -90.0,
    };
    let center_x = content.x + content.width / 2.0;
    let center_y = content.y + content.height / 2.0;
    (
        Rect {
            x: center_x - content.height / 2.0,
            y: center_y - content.width / 2.0,
            width: content.height,
            height: content.width,
        },
        Some(oxoffice_layout::Transform::rotate_about(
            degrees, center_x, center_y,
        )),
    )
}

#[cfg(test)]
mod tests {
    use super::{ResolvedRunStyle, lacks_a_glyph, needs_multilingual_layout, shape_run};
    use oxoffice_layout::FontManager;

    fn family(fonts: &FontManager, id: oxoffice_layout::FontId) -> String {
        fonts.font_data(id).unwrap().family
    }

    // Only characters that draw a glyph count: a tab or a variation selector
    // neither sends a run to the grapheme split nor keeps a face that draws
    // none of the rest.
    #[test]
    fn characters_that_draw_nothing_never_choose_a_face() {
        let mut fonts = FontManager::new_deterministic().unwrap();
        let style = ResolvedRunStyle::default();
        let tabbed = shape_run(&mut fonts, "Q1\tQ2", &style).unwrap();
        assert!(!lacks_a_glyph(&fonts, &tabbed));
        let check = shape_run(&mut fonts, "\u{2611}\u{fe0f}", &style).unwrap();
        assert_eq!(family(&fonts, check.font_id), "Noto Sans Symbols 2");
    }

    #[test]
    fn every_east_asian_run_is_shaped_by_grapheme() {
        for text in [
            "2024\u{b144} \u{b9e4}\u{cd9c}",
            "\u{ff21}\u{ff22}",
            "\u{8ba1}",
            "\u{3042}",
            "\u{20000}",
        ] {
            assert!(needs_multilingual_layout(text), "{text}");
        }
        assert!(!needs_multilingual_layout("Revenue 2024"));
    }
}
