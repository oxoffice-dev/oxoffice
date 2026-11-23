//! Multilingual and bidirectional shaping, and highlight colours.

use super::*;

fn word_language_for_slot(style: &WordMultilingualStyle, slot: WordLanguageSlot) -> Option<String> {
    match slot {
        WordLanguageSlot::Direct => style.language.clone(),
        WordLanguageSlot::EastAsia => style
            .language_east_asia
            .clone()
            .or_else(|| style.language.clone()),
        WordLanguageSlot::Bidi => style
            .language_bidi
            .clone()
            .or_else(|| style.language.clone()),
    }
}

/// The advance one character adds beyond its own glyph advance.
///
/// `w:spacing` is the run's own character spacing. A `linesAndChars` or
/// `snapToChars` section grid adds `w:charSpace` on top of it, and a run that
/// opts out with `w:snapToGrid w:val="0"` keeps only its own spacing.
pub(super) fn word_character_advance_pt(rpr: &CT_RPr, grid_char_space_pt: f64) -> f64 {
    let spacing = rpr.spacing.map_or(0.0, |value| value.to_pt());
    if rpr.snap_to_grid == Some(false) {
        spacing
    } else {
        spacing + grid_char_space_pt
    }
}

pub(super) fn word_text_direction(rtl: Option<bool>) -> TextDirection {
    match rtl {
        Some(true) => TextDirection::RightToLeft,
        Some(false) => TextDirection::LeftToRight,
        None => TextDirection::Auto,
    }
}

fn word_language_ranges(text: &str) -> Vec<(usize, usize, WordLanguageSlot)> {
    let mut ranges = Vec::new();
    let mut start = 0usize;
    let mut slot = WordLanguageSlot::Direct;
    for (offset, character) in text.char_indices() {
        let Some(next_slot) = word_language_slot(character) else {
            continue;
        };
        if next_slot != slot && offset > start {
            ranges.push((start, offset, slot));
            start = offset;
        }
        slot = next_slot;
    }
    if start < text.len() {
        ranges.push((start, text.len(), slot));
    }
    ranges
}

fn word_multilingual_segment_slice(
    segment: &TextSegment,
    byte_start: usize,
    byte_end: usize,
) -> Result<TextSegment> {
    let mut slice = segment.clone();
    slice.text = segment.text[byte_start..byte_end].to_owned();
    if let Some(source) = segment.source {
        let char_start =
            u32::try_from(segment.text[..byte_start].chars().count()).map_err(|_| {
                oxoffice_layout::LayoutError::Layout(
                    "Word multilingual source offset exceeds the supported range".to_owned(),
                )
            })?;
        let char_len = u32::try_from(slice.text.chars().count()).map_err(|_| {
            oxoffice_layout::LayoutError::Layout(
                "Word multilingual source length exceeds the supported range".to_owned(),
            )
        })?;
        slice.source = Some(SourceSpan {
            node: source.node,
            char_start: source.char_start.checked_add(char_start).ok_or_else(|| {
                oxoffice_layout::LayoutError::Layout(
                    "Word multilingual source offset overflowed".to_owned(),
                )
            })?,
            char_end: source
                .char_start
                .checked_add(char_start)
                .and_then(|start| start.checked_add(char_len))
                .ok_or_else(|| {
                    oxoffice_layout::LayoutError::Layout(
                        "Word multilingual source range overflowed".to_owned(),
                    )
                })?,
        });
    }
    slice.glyph_ids.clear();
    slice.advances.clear();
    slice.width = 0.0;
    Ok(slice)
}

/// Whether this text needs the rich shaping path rather than the legacy one.
///
/// The East Asian half is the same shared set the font and language tables
/// use, so no character is East Asian to one of the three and invisible to
/// another. The rest is every range `script_for_char` gives a non-Latin
/// script identity, which is why Hangul is here at all. Without it Korean
/// text never reached the shaper, and giving Hangul a script identity one
/// layer down would have changed nothing.
///
/// A paragraph that lands on this path leaves the paragraph block cache, since
/// `inline_bytes` cannot bound the retained size of a rich inline item and
/// scores it `usize::MAX`. That has always been true of Arabic, Hebrew and
/// CJK, and is now true of Korean.
pub(super) fn needs_word_multilingual_layout(text: &str) -> bool {
    text.chars().any(|character| {
        let codepoint = character as u32;
        is_east_asian(codepoint)
            || matches!(
                codepoint,
                0x0590..=0x08ff
                    | 0x0900..=0x097f
                    | 0x0e00..=0x0e7f
                    | 0xa8e0..=0xa8ff
                    | 0xfb1d..=0xfdff
                    | 0xfe70..=0xfeff
            )
    })
}

pub(super) fn inferred_word_base_direction(items: &[InlineItem]) -> TextDirection {
    for character in items
        .iter()
        .filter_map(|item| match item {
            InlineItem::Text(segment)
            | InlineItem::HyphenatedText { segment, .. }
            | InlineItem::Marker(segment) => Some(segment.text.as_str()),
            _ => None,
        })
        .flat_map(str::chars)
    {
        if character == '\u{200e}' {
            return TextDirection::LeftToRight;
        }
        if character == '\u{200f}' {
            return TextDirection::RightToLeft;
        }
        if !character.is_alphabetic() {
            continue;
        }
        return if matches!(
            character as u32,
            0x0590..=0x08ff | 0xfb1d..=0xfdff | 0xfe70..=0xfeff
        ) {
            TextDirection::RightToLeft
        } else {
            TextDirection::LeftToRight
        };
    }
    TextDirection::LeftToRight
}

pub(super) fn multilingual_candidate(item: &InlineItem) -> Option<&TextSegment> {
    match item {
        InlineItem::Text(segment) | InlineItem::HyphenatedText { segment, .. }
            if !segment.text.is_empty() && segment.field_kind.is_none() =>
        {
            Some(segment)
        }
        _ => None,
    }
}

fn apply_word_multilingual_spacing(
    segment: oxoffice_layout::MultilingualTextSegment,
    spacing: f64,
    exact_word_baseline: bool,
) -> Result<oxoffice_layout::MultilingualTextSegment> {
    if spacing == 0.0 && !exact_word_baseline {
        return Ok(segment);
    }
    let mut base = segment.base().clone();
    let x_advances = segment
        .x_advances()
        .iter()
        .map(|advance| advance + spacing)
        .collect::<Vec<_>>();
    base.advances = x_advances.clone();
    base.width = x_advances.iter().sum();
    if exact_word_baseline {
        base.ascent = base.font_size * WORD_EXACT_LINE_BASELINE_EM;
    }
    segment.with_base_and_advances(base, x_advances)
}

pub(super) fn shape_word_multilingual_items(
    font_manager: &mut FontManager,
    legacy_items: Vec<InlineItem>,
    styles: &HashMap<usize, WordMultilingualStyle>,
    base_direction: TextDirection,
    no_wrap: bool,
    exact_word_baseline: bool,
    legacy_shaping: bool,
) -> Result<WordShapedItems> {
    let mut styled = Vec::new();
    let mut kern = Vec::new();
    for (index, item) in legacy_items.iter().enumerate() {
        let segment = match item {
            InlineItem::Text(segment) | InlineItem::HyphenatedText { segment, .. } => segment,
            _ => continue,
        };
        let style = styles
            .get(&index)
            .ok_or_else(|| LayoutError::Layout("Word text lacks its producer context".into()))?;
        if style.origin == WordInputOrigin::Structural
            || (!legacy_shaping
                && (style.origin == WordInputOrigin::PlainField || segment.text.is_empty()))
        {
            continue;
        }
        if legacy_shaping {
            styled.push((segment.clone(), None));
            kern.push(style.kern);
        } else {
            for (byte_start, byte_end, slot) in word_language_ranges(&segment.text) {
                let mut slice = word_multilingual_segment_slice(segment, byte_start, byte_end)?;
                slice.direction = style.direction;
                styled.push((slice, word_language_for_slot(style, slot)));
                kern.push(style.kern);
            }
        }
    }
    let mut shaped = VecDeque::from(font_manager.shape_paragraph_with_kerning(
        styled,
        base_direction,
        no_wrap,
        &kern,
        legacy_shaping,
    )?);
    let mut items = Vec::with_capacity(legacy_items.len());
    let mut annotations = Vec::new();
    let mut occurrences = Vec::new();
    let mut logical_index = 0usize;
    for (index, mut item) in legacy_items.into_iter().enumerate() {
        let style = styles.get(&index);
        let requested = style.and_then(|style| style.requested_family.as_deref());
        let segment = match &item {
            InlineItem::Text(segment) | InlineItem::HyphenatedText { segment, .. }
                if style.is_some_and(|style| {
                    matches!(
                        style.origin,
                        WordInputOrigin::Text | WordInputOrigin::PlainField
                    )
                }) =>
            {
                Some(segment)
            }
            _ => None,
        };
        let Some(segment) = segment else {
            if let InlineItem::Text(segment) | InlineItem::Marker(segment) = &item {
                font_manager.record_font_contribution(segment.font_id, requested, &segment.text)?;
            }
            if exact_word_baseline {
                match &mut item {
                    InlineItem::Text(segment) | InlineItem::Marker(segment) => {
                        segment.ascent = segment.font_size * WORD_EXACT_LINE_BASELINE_EM
                    }
                    _ => {}
                }
            }
            if let Some(annotation) = style.and_then(|style| style.annotation.as_ref()) {
                let mut annotation = annotation.clone();
                annotation.slot = items.len();
                annotations.push(annotation);
            }
            items.push(item);
            continue;
        };
        // Former plain field inputs were not part of the rich batch. Their
        // local factory preserves their own legacy Unicode shaping context.
        let mut owner_shaped = if !legacy_shaping
            && (style.is_some_and(|style| style.origin == WordInputOrigin::PlainField)
                || segment.text.is_empty())
        {
            Some(VecDeque::from(font_manager.shape_paragraph_with_kerning(
                vec![(
                    segment.clone(),
                    style.and_then(|style| style.language.clone()),
                )],
                base_direction,
                no_wrap,
                &[style.is_some_and(|style| style.kern)],
                true,
            )?))
        } else {
            None
        };
        let target_bytes = segment.text.len();
        let mut consumed_bytes = 0usize;
        loop {
            let span = owner_shaped
                .as_mut()
                .unwrap_or(&mut shaped)
                .pop_front()
                .ok_or_else(|| LayoutError::Layout("Word shaping lost a styled text run".into()))?;
            consumed_bytes = consumed_bytes
                .checked_add(span.text().len())
                .filter(|n| *n <= target_bytes)
                .ok_or_else(|| {
                    LayoutError::Layout("Word shaping crossed a styled text boundary".into())
                })?;
            let span = apply_word_multilingual_spacing(
                span,
                style.map_or(0.0, |style| style.spacing),
                exact_word_baseline,
            )?;
            let legacy = span.uses_legacy_shaping()
                || (matches!(item, InlineItem::HyphenatedText { .. })
                    && !needs_word_multilingual_layout(span.text()));
            let span = if let InlineItem::HyphenatedText { language, .. } = &item {
                span.with_hyphenation_language(Some(language.clone()))
            } else {
                span
            };
            let span = span.with_logical_index_and_legacy_shaping(logical_index, legacy);
            logical_index = logical_index
                .checked_add(1)
                .ok_or_else(|| LayoutError::Layout("Word logical span ordinal overflow".into()))?;
            font_manager.record_font_contribution(span.base().font_id, requested, span.text())?;
            if let Some(occurrence) = style.and_then(|style| style.occurrence) {
                occurrences.push((span.logical_index(), occurrence));
            }
            items.push(InlineItem::MultilingualText(span));
            if consumed_bytes == target_bytes {
                break;
            }
        }
        if owner_shaped.is_some_and(|owner| !owner.is_empty()) {
            return Err(LayoutError::Layout(
                "Word shaping left an unmatched plain owner span".into(),
            ));
        }
    }
    if !shaped.is_empty() {
        return Err(LayoutError::Layout(
            "Word shaping produced an unmatched text run".into(),
        ));
    }
    Ok(WordShapedItems {
        items,
        annotations,
        occurrences,
    })
}

/// The colour of a `w:highlight`: Word's highlight palette, as LibreOffice
/// imports it too. The dark colours are half intensity (`000080`), not the
/// CSS colours of the same names, and the greys are `808080` and `C0C0C0`.
pub(super) fn highlight_to_color(h: ST_HighlightColor) -> Option<Color> {
    let hex = match h {
        ST_HighlightColor::None => return None,
        ST_HighlightColor::Black => "000000",
        ST_HighlightColor::Blue => "0000FF",
        ST_HighlightColor::Cyan => "00FFFF",
        ST_HighlightColor::DarkBlue => "000080",
        ST_HighlightColor::DarkCyan => "008080",
        ST_HighlightColor::DarkGray => "808080",
        ST_HighlightColor::DarkGreen => "008000",
        ST_HighlightColor::DarkMagenta => "800080",
        ST_HighlightColor::DarkRed => "800000",
        ST_HighlightColor::DarkYellow => "808000",
        ST_HighlightColor::Green => "00FF00",
        ST_HighlightColor::LightGray => "C0C0C0",
        ST_HighlightColor::Magenta => "FF00FF",
        ST_HighlightColor::Red => "FF0000",
        ST_HighlightColor::White => "FFFFFF",
        ST_HighlightColor::Yellow => "FFFF00",
    };
    Some(Color::from_hex(hex))
}
