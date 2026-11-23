//! The logical source of painted items: bookmarks, field owners, reflow order and tab ranks.

use super::*;

/// The bookmark a page target marker records, among a paragraph's.
pub(super) fn bookmark_target_name<'a>(
    item: &LineItem,
    targets: &'a [(usize, String)],
) -> Option<&'a str> {
    let field_kind = match item {
        LineItem::Text(segment) | LineItem::Marker(segment) => segment.field_kind,
        LineItem::MultilingualText(segment) => segment.base().field_kind,
        _ => None,
    };
    let Some(oxoffice_layout::FieldKind::Target(target)) = field_kind else {
        return None;
    };
    targets
        .iter()
        .find(|(id, _)| *id == target)
        .map(|(_, name)| name.as_str())
}

pub(super) fn word_field_owner(
    paint: Option<&WordParagraphPaint>,
    segment: Option<&oxoffice_layout::MultilingualTextSegment>,
    plain: Option<&oxoffice_layout::TextSegment>,
) -> oxoffice_layout::Result<FieldPaintOwner> {
    let Some(paint) = paint else {
        return Ok(FieldPaintOwner::Neutral);
    };
    if paint.plain_field_policy.is_none() {
        if plain
            .is_some_and(|segment| segment.field_kind.is_some() || segment.field_source.is_some())
            || segment.is_some_and(|segment| {
                segment.base().field_kind.is_some() || segment.base().field_source.is_some()
            })
        {
            return Err(oxoffice_layout::LayoutError::Layout(
                "Word field lacks known producer context".into(),
            ));
        }
        return Ok(FieldPaintOwner::Word {
            kern: false,
            legacy: true,
            occurrence: None,
        });
    }
    let (kern, legacy) = if let Some(segment) = segment {
        (
            segment.kerning_override().ok_or_else(|| {
                oxoffice_layout::LayoutError::Layout(
                    "Word span lacks explicit shaping context".into(),
                )
            })?,
            segment.uses_legacy_shaping(),
        )
    } else {
        paint.plain_field_policy.ok_or_else(|| {
            oxoffice_layout::LayoutError::Layout(
                "Word plain field lacks structural producer context".into(),
            )
        })?
    };
    let occurrence = segment.and_then(|segment| {
        paint
            .field_occurrences
            .iter()
            .find_map(|(index, occurrence)| {
                (*index == segment.logical_index()).then_some(*occurrence)
            })
    });
    Ok(FieldPaintOwner::Word {
        kern,
        legacy,
        occurrence,
    })
}

pub(super) fn logical_reflow_elements(
    elements: &[PositionedElement],
    provenance: &[ReflowTextProvenance],
    tabs: &[ReflowTabProvenance],
    logical_items: &[oxoffice_layout::InlineItem],
    base_direction: oxoffice_layout::TextDirection,
    source_node: Option<Option<oxoffice_layout::SourceNodeId>>,
) -> Option<Vec<usize>> {
    let has_source_less_text =
        provenance
            .iter()
            .any(|provenance| match &elements[provenance.element_position] {
                PositionedElement::Text(run) => run.source.is_none(),
                PositionedElement::MultilingualText(run) => run.source.is_none(),
                _ => false,
            });
    if !has_source_less_text {
        return None;
    }

    let mut used_source_less = vec![false; logical_items.len()];
    let mut ranked = Vec::with_capacity(provenance.len());
    let mut known_visual_ranks = Vec::new();
    for (visual_order, item_provenance) in provenance.iter().copied().enumerate() {
        if item_provenance.kind == ReflowTextProvenanceKind::TabLeader {
            continue;
        }
        let element = elements[item_provenance.element_position].clone();
        let (source, field_kind, note, text, rich_logical_index) = match &element {
            PositionedElement::Text(run) => (
                run.source,
                run.field_kind,
                run.note,
                run.text.as_str(),
                None,
            ),
            PositionedElement::MultilingualText(run) => (
                run.source,
                run.field_kind,
                run.note,
                run.logical_text.as_str(),
                Some(run.logical_index),
            ),
            _ => return None,
        };
        let key = if let Some(source) = source {
            logical_items.iter().enumerate().find_map(|(index, item)| {
                let item_source = normalize_reflow_source(inline_item_source(item)?, source_node)?;
                (item_source.node == source.node
                    && item_source.char_start <= source.char_start
                    && item_source.char_end >= source.char_end)
                    .then_some((index, source.char_start, source.char_end))
            })
        } else if let Some(field_kind) = field_kind {
            let exact = logical_items.iter().enumerate().position(|(index, item)| {
                !used_source_less[index]
                    && inline_item_field(item) == Some(field_kind)
                    && inline_item_note(item) == note
                    && inline_item_text(item) == Some(text)
            });
            let index = exact.or_else(|| {
                logical_items.iter().enumerate().position(|(index, item)| {
                    !used_source_less[index] && inline_item_field(item) == Some(field_kind)
                })
            })?;
            used_source_less[index] = true;
            Some((index, 0, 0))
        } else if let Some(logical_index) = rich_logical_index {
            logical_items
                .iter()
                .enumerate()
                .find(|(_, item)| {
                    matches!(
                        item,
                        oxoffice_layout::InlineItem::MultilingualText(segment)
                            if segment.logical_index() == logical_index
                    )
                })
                .map(|(index, _)| (index, 0, 0))
        } else if item_provenance.kind == ReflowTextProvenanceKind::ConditionalHyphen {
            conditional_hyphen_key(elements, provenance, logical_items, source_node)
        } else {
            let exact = logical_items.iter().enumerate().position(|(index, item)| {
                !used_source_less[index]
                    && source_less_item_matches(item, item_provenance.kind, field_kind, note, text)
            });
            if let Some(index) = exact {
                used_source_less[index] = true;
                Some((index, 0, 0))
            } else {
                None
            }
        }?;
        known_visual_ranks.push((item_provenance.visual_item, key.0));
        ranked.push((key, visual_order, item_provenance.element_position));
    }

    let tab_ranks = logical_tab_ranks(tabs, &known_visual_ranks, logical_items, base_direction)?;
    for (visual_order, item_provenance) in provenance.iter().copied().enumerate() {
        if item_provenance.kind != ReflowTextProvenanceKind::TabLeader {
            continue;
        }
        let logical_index = *tab_ranks.get(&item_provenance.visual_item)?;
        ranked.push((
            (logical_index, 0, 0),
            visual_order,
            item_provenance.element_position,
        ));
    }
    ranked.sort_by_key(|(key, visual_order, _)| (*key, *visual_order));
    Some(ranked.into_iter().map(|(_, _, element)| element).collect())
}

fn normalize_reflow_source(
    mut source: oxoffice_layout::SourceSpan,
    source_node: Option<Option<oxoffice_layout::SourceNodeId>>,
) -> Option<oxoffice_layout::SourceSpan> {
    if let Some(source_node) = source_node {
        source.node = source_node?;
    }
    Some(source)
}

fn conditional_hyphen_key(
    elements: &[PositionedElement],
    provenance: &[ReflowTextProvenance],
    logical_items: &[oxoffice_layout::InlineItem],
    source_node: Option<Option<oxoffice_layout::SourceNodeId>>,
) -> Option<(usize, u32, u32)> {
    logical_items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let segment = match item {
                oxoffice_layout::InlineItem::HyphenatedText { segment, .. } => segment,
                oxoffice_layout::InlineItem::MultilingualText(segment)
                    if segment.uses_legacy_shaping() =>
                {
                    segment.base()
                }
                _ => return None,
            };
            let item_source = normalize_reflow_source(segment.source?, source_node)?;
            let prefix_end = provenance
                .iter()
                .filter_map(|provenance| match &elements[provenance.element_position] {
                    PositionedElement::Text(run) => run.source,
                    PositionedElement::MultilingualText(run) => run.source,
                    _ => None,
                })
                .filter(|source| {
                    source.node == item_source.node
                        && source.char_start >= item_source.char_start
                        && source.char_end < item_source.char_end
                })
                .map(|source| source.char_end)
                .max()?;
            Some((index, prefix_end, prefix_end))
        })
        .max()
}

pub(super) fn conditional_hyphen_visual_items(
    line_items: &[LineItem],
    logical_items: &[oxoffice_layout::InlineItem],
    base_direction: oxoffice_layout::TextDirection,
) -> Vec<usize> {
    line_items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let hyphen = match item {
                LineItem::Text(hyphen) => hyphen,
                LineItem::MultilingualText(hyphen) if hyphen.uses_legacy_shaping() => hyphen.base(),
                _ => return None,
            };
            if hyphen.text != "-"
                || hyphen.source.is_some()
                || hyphen.field_kind.is_some()
                || hyphen.note.is_some()
            {
                return None;
            }
            logical_items
                .iter()
                .filter_map(|item| match item {
                    oxoffice_layout::InlineItem::HyphenatedText { segment, .. } => Some(segment),
                    oxoffice_layout::InlineItem::MultilingualText(segment)
                        if segment.uses_legacy_shaping() =>
                    {
                        Some(segment.base())
                    }
                    _ => None,
                })
                .any(|segment| {
                    let direction = match segment.direction {
                        oxoffice_layout::TextDirection::Auto => {
                            inferred_text_direction(&segment.text).unwrap_or(base_direction)
                        }
                        direction => direction,
                    };
                    let prefix_index = match direction {
                        oxoffice_layout::TextDirection::RightToLeft => index.checked_add(1),
                        oxoffice_layout::TextDirection::LeftToRight => index.checked_sub(1),
                        oxoffice_layout::TextDirection::Auto => unreachable!("Auto was resolved"),
                    };
                    let Some(prefix_source) = prefix_index
                        .and_then(|prefix_index| line_items.get(prefix_index))
                        .and_then(line_item_source)
                    else {
                        return false;
                    };
                    segment.source.is_some_and(|source| {
                        prefix_source.node == source.node
                            && prefix_source.char_start >= source.char_start
                            && prefix_source.char_end < source.char_end
                    })
                })
                .then_some(index)
        })
        .collect()
}

fn inferred_text_direction(text: &str) -> Option<oxoffice_layout::TextDirection> {
    for character in text.chars() {
        if character == '\u{200e}' {
            return Some(oxoffice_layout::TextDirection::LeftToRight);
        }
        if character == '\u{200f}' {
            return Some(oxoffice_layout::TextDirection::RightToLeft);
        }
        if !character.is_alphabetic() {
            continue;
        }
        return Some(
            if matches!(
                character as u32,
                0x0590..=0x08ff | 0xfb1d..=0xfdff | 0xfe70..=0xfeff
            ) {
                oxoffice_layout::TextDirection::RightToLeft
            } else {
                oxoffice_layout::TextDirection::LeftToRight
            },
        );
    }
    None
}

fn line_item_source(item: &LineItem) -> Option<oxoffice_layout::SourceSpan> {
    match item {
        LineItem::Text(segment) | LineItem::Marker(segment) => segment.source,
        LineItem::MultilingualText(segment) => segment.base().source,
        _ => None,
    }
}

fn source_less_item_matches(
    item: &oxoffice_layout::InlineItem,
    kind: ReflowTextProvenanceKind,
    field_kind: Option<oxoffice_layout::FieldKind>,
    note: Option<oxoffice_layout::NoteRef>,
    text: &str,
) -> bool {
    let segment = match (kind, item) {
        (ReflowTextProvenanceKind::Marker, oxoffice_layout::InlineItem::Marker(segment))
        | (ReflowTextProvenanceKind::Text, oxoffice_layout::InlineItem::Text(segment)) => segment,
        _ => return false,
    };
    segment.source.is_none()
        && segment.field_kind == field_kind
        && segment.note == note
        && segment.text == text
}

fn logical_tab_ranks(
    tabs: &[ReflowTabProvenance],
    known_visual_ranks: &[(usize, usize)],
    logical_items: &[oxoffice_layout::InlineItem],
    base_direction: oxoffice_layout::TextDirection,
) -> Option<HashMap<usize, usize>> {
    if tabs.is_empty() {
        return Some(HashMap::new());
    }
    let logical_tabs = logical_items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            matches!(item, oxoffice_layout::InlineItem::Tab { .. }).then_some(index)
        })
        .collect::<Vec<_>>();
    if logical_tabs.len() < tabs.len() {
        return None;
    }
    let mut known = known_visual_ranks.to_vec();
    known.sort_unstable_by_key(|(visual_item, _)| *visual_item);
    let default_ascending = known
        .windows(2)
        .find_map(|pair| (pair[0].1 != pair[1].1).then_some(pair[0].1 < pair[1].1))
        .unwrap_or(base_direction != oxoffice_layout::TextDirection::RightToLeft);
    let mut result = HashMap::new();
    let mut used = Vec::new();
    let mut cursor = 0usize;
    while cursor < tabs.len() {
        let start = cursor;
        while cursor + 1 < tabs.len()
            && tabs[cursor + 1].visual_item == tabs[cursor].visual_item + 1
        {
            cursor += 1;
        }
        let group = &tabs[start..=cursor];
        let left = known
            .iter()
            .rev()
            .find(|(visual_item, _)| *visual_item < group[0].visual_item)
            .map(|(_, logical_index)| *logical_index);
        let right = known
            .iter()
            .find(|(visual_item, _)| *visual_item > group[group.len() - 1].visual_item)
            .map(|(_, logical_index)| *logical_index);
        let ascending = match (left, right) {
            (Some(left), Some(right)) if left != right => left < right,
            _ => default_ascending,
        };
        let mut candidates = logical_tabs
            .iter()
            .copied()
            .filter(|index| !used.contains(index))
            .filter(|index| match (left, right, ascending) {
                (Some(left), Some(right), true) => *index > left && *index < right,
                (Some(left), Some(right), false) => *index < left && *index > right,
                (Some(left), None, true) => *index > left,
                (Some(left), None, false) => *index < left,
                (None, Some(right), true) => *index < right,
                (None, Some(right), false) => *index > right,
                (None, None, _) => true,
            })
            .collect::<Vec<_>>();
        candidates.sort_unstable();
        let count = group.len();
        if candidates.len() < count {
            return None;
        }
        let mut selected = match (left, right, ascending) {
            (None, Some(_), true) | (Some(_), None, false) => {
                candidates.split_off(candidates.len() - count)
            }
            (None, Some(_), false) | (Some(_), None, true) | (None, None, _) => {
                candidates.into_iter().take(count).collect()
            }
            (Some(_), Some(_), _) => candidates.into_iter().take(count).collect(),
        };
        if !ascending {
            selected.reverse();
        }
        for (tab, logical_index) in group.iter().zip(selected) {
            used.push(logical_index);
            result.insert(tab.visual_item, logical_index);
        }
        cursor += 1;
    }
    Some(result)
}

fn inline_item_source(item: &oxoffice_layout::InlineItem) -> Option<oxoffice_layout::SourceSpan> {
    match item {
        oxoffice_layout::InlineItem::Text(segment)
        | oxoffice_layout::InlineItem::HyphenatedText { segment, .. }
        | oxoffice_layout::InlineItem::Marker(segment) => segment.source,
        oxoffice_layout::InlineItem::MultilingualText(segment) => segment.base().source,
        _ => None,
    }
}

fn inline_item_field(item: &oxoffice_layout::InlineItem) -> Option<oxoffice_layout::FieldKind> {
    match item {
        oxoffice_layout::InlineItem::Text(segment)
        | oxoffice_layout::InlineItem::HyphenatedText { segment, .. }
        | oxoffice_layout::InlineItem::Marker(segment) => segment.field_kind,
        oxoffice_layout::InlineItem::MultilingualText(segment) => segment.base().field_kind,
        _ => None,
    }
}

fn inline_item_note(item: &oxoffice_layout::InlineItem) -> Option<oxoffice_layout::NoteRef> {
    match item {
        oxoffice_layout::InlineItem::Text(segment)
        | oxoffice_layout::InlineItem::HyphenatedText { segment, .. }
        | oxoffice_layout::InlineItem::Marker(segment) => segment.note,
        oxoffice_layout::InlineItem::MultilingualText(segment) => segment.base().note,
        _ => None,
    }
}

fn inline_item_text(item: &oxoffice_layout::InlineItem) -> Option<&str> {
    match item {
        oxoffice_layout::InlineItem::Text(segment)
        | oxoffice_layout::InlineItem::HyphenatedText { segment, .. }
        | oxoffice_layout::InlineItem::Marker(segment) => Some(segment.text.as_str()),
        oxoffice_layout::InlineItem::MultilingualText(segment) => Some(segment.text()),
        _ => None,
    }
}
