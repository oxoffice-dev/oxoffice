//! Painting a paragraph's lines: text runs, tabs, fields, form marks, change bars and justification.

use super::*;

fn push_multilingual_text(
    elements: &mut Vec<PositionedElement>,
    policies: &mut PaintPolicies,
    owner: FieldPaintOwner,
    segment: &oxoffice_layout::MultilingualTextSegment,
    x: f64,
    baseline: f64,
    line_top: f64,
    line_height: f64,
    source_node: Option<Option<oxoffice_layout::SourceNodeId>>,
    justify_extra: f64,
) -> oxoffice_layout::Result<f64> {
    let base = segment.base();
    let segment_spaces = if justify_extra > 0.0 {
        segment
            .text()
            .chars()
            .filter(|character| *character == ' ')
            .count()
    } else {
        0
    };
    let segment_extra = segment_spaces as f64 * justify_extra;
    let effective_width = segment.width() + segment_extra;
    let mut x_advances = segment.x_advances().to_vec();
    if segment_extra > 0.0 {
        let characters = segment.text().chars().collect::<Vec<_>>();
        for cluster in segment.clusters() {
            let spaces = characters[cluster.char_start as usize..cluster.char_end as usize]
                .iter()
                .filter(|character| **character == ' ')
                .count();
            if spaces > 0 {
                let glyph = cluster.glyph_end as usize - 1;
                x_advances[glyph] += spaces as f64 * justify_extra;
            }
        }
    }
    let adjusted_baseline = baseline - base.baseline_offset;
    if let Some(color) = base.highlight {
        elements.push(PositionedElement::FilledRect {
            rect: Rect {
                x,
                y: line_top,
                width: effective_width,
                height: line_height,
            },
            color,
        });
    }
    let source = match source_node {
        Some(source_node) => base.source.and_then(|mut source| {
            source.node = source_node?;
            Some(source)
        }),
        None => base.source,
    };
    policies.emit(
        elements,
        PositionedElement::MultilingualText(MultilingualGlyphRun {
            origin: Point {
                x,
                y: adjusted_baseline,
            },
            font_id: segment.font_id(),
            font_size: base.font_size,
            glyph_ids: segment.glyph_ids().to_vec(),
            x_advances,
            y_advances: segment.y_advances().to_vec(),
            x_offsets: segment.x_offsets().to_vec(),
            y_offsets: segment.y_offsets().to_vec(),
            clusters: segment.clusters().to_vec(),
            logical_text: segment.text().to_owned(),
            logical_index: segment.logical_index(),
            source,
            script: segment.script(),
            language: segment.language().map(str::to_owned),
            direction: segment.direction(),
            bidi_level: segment.bidi_level(),
            color: base.color,
            bold: base.bold,
            italic: base.italic,
            field_kind: base.field_kind,
            field_source: match source_node {
                Some(source_node) => base.field_source.and_then(|mut field_source| {
                    field_source.node = source_node?;
                    Some(field_source)
                }),
                None => base.field_source,
            },
            note: base.note,
            note_reference_source: match source_node {
                Some(source_node) => base.note_reference_source.and_then(|mut source| {
                    source.node = source_node?;
                    Some(source)
                }),
                None => base.note_reference_source,
            },
        }),
        owner,
    )?;
    if let Some(underline) = base.underline {
        let underline_y = adjusted_baseline + base.descent * 0.3;
        let (thickness, second) = crate::convert::underline_strokes(underline, base.font_size);
        elements.push(PositionedElement::Line {
            start: Point { x, y: underline_y },
            end: Point {
                x: x + effective_width,
                y: underline_y,
            },
            width: thickness,
            color: base.color,
            dash_pattern: None,
        });
        if let Some(offset) = second {
            let second_y = underline_y + offset;
            elements.push(PositionedElement::Line {
                start: Point { x, y: second_y },
                end: Point {
                    x: x + effective_width,
                    y: second_y,
                },
                width: thickness,
                color: base.color,
                dash_pattern: None,
            });
        }
    }
    if base.strike || base.dstrike {
        let strike_y = adjusted_baseline - base.ascent * 0.3;
        let thickness = base.font_size / 24.0;
        let positions = if base.dstrike {
            let gap = thickness * 2.0;
            vec![strike_y - gap / 2.0, strike_y + gap / 2.0]
        } else {
            vec![strike_y]
        };
        for y in positions {
            elements.push(PositionedElement::Line {
                start: Point { x, y },
                end: Point {
                    x: x + effective_width,
                    y,
                },
                width: thickness,
                color: base.color,
                dash_pattern: None,
            });
        }
    }
    if let Some(url) = &base.hyperlink_url {
        elements.push(PositionedElement::LinkAnnotation {
            rect: Rect {
                x,
                y: line_top,
                width: effective_width,
                height: line_height,
            },
            url: url.clone(),
        });
    }
    Ok(effective_width)
}

/// Render paragraph lines as positioned elements.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReflowTextProvenanceKind {
    Text,
    Marker,
    Multilingual,
    ConditionalHyphen,
    TabLeader,
}

/// For each item of a line, where a field placeholder sits in the text after
/// a right, centre or decimal tab, measured from the start of the line.
fn tab_aligned_fields(items: &[LineItem]) -> Vec<Option<TabAlignedField>> {
    let mut fields = vec![None; items.len()];
    let mut x = 0.0;
    let mut index = 0;
    while index < items.len() {
        x += items[index].width();
        let LineItem::Tab { align, gap, .. } = items[index] else {
            index += 1;
            continue;
        };
        index += 1;
        if !matches!(
            align,
            TabAlign::Right | TabAlign::Center | TabAlign::Decimal
        ) {
            continue;
        }
        let end_index = items[index..]
            .iter()
            .position(|item| matches!(item, LineItem::Tab { .. }))
            .map_or(items.len(), |offset| index + offset);
        let segment = &items[index..end_index];
        let start = x;
        let end = start + segment.iter().map(LineItem::width).sum::<f64>();
        // A decimal stop aligns the first full stop, or else the end of the
        // first number. A page field is a number.
        let mut past_point = false;
        let mut in_number = false;
        for (offset, item) in segment.iter().enumerate() {
            let text = match item {
                LineItem::Text(text) => text,
                LineItem::MultilingualText(text) if text.uses_legacy_shaping() => text.base(),
                _ => continue,
            };
            if text.field_kind.is_some() || text.field_source.is_some() {
                fields[index + offset] = Some(TabAlignedField {
                    start,
                    end,
                    shift: match align {
                        TabAlign::Center => 0.5,
                        TabAlign::Decimal if past_point => 0.0,
                        _ => 1.0,
                    },
                    gap,
                });
                in_number = true;
                continue;
            }
            for ch in text.text.chars() {
                if ch == '.' || (in_number && !ch.is_ascii_digit() && ch != ',') {
                    past_point = true;
                }
                in_number |= ch.is_ascii_digit();
            }
        }
    }
    fields
}

/// Marks the fields of a right-aligned or centred line, which has no tab
/// that aligns them already, as aligned text: the whole line, which moves
/// back by all (right) or half (centred) of the change in width its fields
/// make once they hold their page's value, so that a header such as a right
/// aligned STYLEREF still ends at the margin. It moves back no further than
/// the room the alignment left before the text.
fn paragraph_aligned_fields(
    mut fields: Vec<Option<TabAlignedField>>,
    items: &[LineItem],
    jc: Option<Align>,
    remaining_width: f64,
) -> Vec<Option<TabAlignedField>> {
    let (shift, gap) = match jc {
        Some(Align::End) => (1.0, remaining_width),
        Some(Align::Center) => (0.5, remaining_width / 2.0),
        _ => return fields,
    };
    // A tab aligns its own text; a picture or a drawing on the line would
    // not move with the text.
    if items.iter().any(|item| {
        matches!(
            item,
            LineItem::Tab { .. }
                | LineItem::Image { .. }
                | LineItem::Group { .. }
                | LineItem::Figure { .. }
        )
    }) {
        return fields;
    }
    let end = items.iter().map(LineItem::width).sum::<f64>();
    for (field, item) in fields.iter_mut().zip(items) {
        let text = match item {
            LineItem::Text(text) => text,
            LineItem::MultilingualText(text) if text.uses_legacy_shaping() => text.base(),
            _ => continue,
        };
        if text.field_kind.is_some() || text.field_source.is_some() {
            *field = Some(TabAlignedField {
                start: 0.0,
                end,
                shift,
                gap: gap.max(0.0),
            });
        }
    }
    fields
}

/// A form field marker: no drawing, only the box a marked item drew on its
/// line. The engine's numbering pass gathers what lies wholly inside it
/// into the field's fragment, whose widget takes `band` when there is one.
fn form_marker(
    mark: oxoffice_layout::FormMark,
    rect: Rect,
    band: Option<Rect>,
    track: f64,
) -> PositionedElement {
    PositionedElement::FormField(oxoffice_layout::FormFieldFragment {
        field: oxoffice_layout::FormFieldId(u32::MAX),
        rect,
        children: Vec::new(),
        alternate: Vec::new(),
        mark: Some(oxoffice_layout::FormMarker { mark, band, track }),
    })
}

/// The form mark of a drawn segment or picture, `Some(None)` for one
/// outside any field, `None` for an item that is neither.
fn item_form_mark(item: &LineItem) -> Option<Option<oxoffice_layout::FormMark>> {
    match item {
        LineItem::Text(segment) if !segment.text.is_empty() || is_form_anchor(segment) => {
            Some(segment.form_mark)
        }
        LineItem::MultilingualText(segment)
            if !segment.base().text.is_empty() || is_form_anchor(segment.base()) =>
        {
            Some(segment.base().form_mark)
        }
        LineItem::Image { form_mark, .. } => Some(*form_mark),
        LineItem::Figure { item, .. } => item_form_mark(item),
        _ => None,
    }
}

/// Whether a segment is the anchor of an empty form control: it draws
/// nothing, and is neither a field's result nor a field event, which can be
/// empty too.
fn is_form_anchor(segment: &oxoffice_layout::TextSegment) -> bool {
    segment.form_mark.is_some()
        && segment.text.is_empty()
        && segment.field_kind.is_none()
        && segment.field_source.is_none()
}

/// The field a tab sits inside: the one mark of the drawn items on both
/// sides of it, so its leader belongs to the field.
fn enclosed_tab_mark(items: &[LineItem], tab: usize) -> Option<oxoffice_layout::FormMark> {
    let before = items[..tab].iter().rev().find_map(item_form_mark)??;
    let after = items[tab + 1..].iter().find_map(item_form_mark)??;
    (before == after).then_some(before)
}

/// The one form mark every drawn segment and picture of a line carries. A
/// list number is outside any field, so its line has no sole mark.
fn sole_form_mark(items: &[LineItem]) -> Option<oxoffice_layout::FormMark> {
    let mut sole = None;
    for (index, item) in items.iter().enumerate() {
        // A list number, a drawing group or a leader outside the field
        // draws on the line too: the widget keeps to the field's own boxes.
        match item {
            LineItem::Marker(_) | LineItem::Group { .. } => return None,
            LineItem::Figure { item, .. } if !matches!(item.as_ref(), LineItem::Image { .. }) => {
                return None;
            }
            LineItem::Tab {
                leader: Some(_), ..
            } if enclosed_tab_mark(items, index).is_none() => return None,
            _ => {}
        }
        let Some(mark) = item_form_mark(item) else {
            continue;
        };
        match (sole, mark) {
            (_, None) => return None,
            (None, Some(mark)) => sole = Some(mark),
            (Some(known), Some(mark)) if known == mark => {}
            _ => return None,
        }
    }
    sole
}

pub(super) fn render_paragraph_lines(
    lines: &[LayoutLine],
    para: ParagraphView<'_>,
    geometry: &PageGeometry,
    start_y: f64,
    fragment: (bool, bool, f64),
    media: &HashMap<MediaId, ImageData>,
) -> oxoffice_layout::Result<PaintLayers> {
    let mut elements = Vec::new();
    let mut policies = PaintPolicies::default();
    let mut layers = PaintLayers::default();
    let first_element = 0;
    let word_paint = para
        .paint
        .or_else(|| para.semantics.map(|semantics| semantics.paint.as_ref()));
    if word_paint.is_some_and(|paint| !paint.rows.is_empty())
        && lines.len() != para.block.lines.len()
    {
        return Err(oxoffice_layout::LayoutError::Layout(
            "prepainted Word rows cannot be structurally reflowed".into(),
        ));
    }
    let mut structural_count = 0usize;
    // A drawing this paragraph must clear rather than flow beside pushes its
    // first line down. `content_height` already counts the same offset. A
    // continuation starts at the top of its page or column: the drawing
    // stayed with the first fragment.
    let mut y = start_y
        + if fragment.0 {
            para.content_offset_top
        } else {
            0.0
        };
    let (top_padding, bottom_padding) = para.borders.as_ref().map_or((0.0, 0.0), |borders| {
        (
            if fragment.0 {
                crate::block::paragraph_border_padding(borders.top.as_ref())
            } else {
                0.0
            },
            if fragment.1 {
                crate::block::paragraph_border_padding(borders.bottom.as_ref())
            } else {
                0.0
            },
        )
    });
    if let Some(borders) = &para.borders {
        render_paragraph_border_edges(
            borders,
            Rect {
                x: geometry.margin_left + para.indent_left,
                y: geometry.margin_top + y + top_padding,
                width: geometry.content_width() - para.indent_left - para.indent_right,
                height: lines.iter().map(|line| line.height).sum::<f64>()
                    + para.line_gaps.iter().take(lines.len()).sum::<f64>()
                    - top_padding
                    - bottom_padding,
            },
            fragment,
            &mut elements,
        );
    }
    for (index, framed_line) in lines.iter().enumerate() {
        let owner_line = para.line_start.checked_add(index).ok_or_else(|| {
            oxoffice_layout::LayoutError::Layout("paragraph paint line overflow".into())
        })?;
        y += para.line_gaps.get(index).copied().unwrap_or(0.0);
        let top = if index == 0 { top_padding } else { 0.0 };
        let bottom = if index + 1 == lines.len() {
            bottom_padding
        } else {
            0.0
        };
        let mut content_line;
        let line = if top != 0.0 || bottom != 0.0 {
            content_line = framed_line.clone();
            content_line.height -= top + bottom;
            content_line.ascent -= top;
            &content_line
        } else {
            framed_line
        };
        y += top;
        let mut text_provenance = Vec::new();
        let mut tab_provenance = Vec::new();
        let conditional_hyphens = para
            .reflow
            .as_deref()
            .map(|reflow| {
                conditional_hyphen_visual_items(&line.items, &reflow.items, para.reflow_direction)
            })
            .unwrap_or_default();
        let baseline_y = geometry.margin_top
            + y
            + paragraph_line_baseline(line, para.baseline_policy).unwrap_or(line.ascent);

        // Compute x offset based on justification. Spaces that end the line
        // hang off its visual side, outside the aligned width.
        let (hang_start, hang_end) = line.hanging_space_counts();
        let ink = hang_start..line.items.len() - hang_end;
        let hanging_left: f64 = line.items[..ink.start]
            .iter()
            .map(|item| item.width())
            .sum();
        let text_width: f64 = line.items[ink.clone()]
            .iter()
            .map(|item| item.width())
            .sum();
        let remaining_width = line.available_width - text_width;

        // For justified text (Both), compute extra space per gap
        let justify_extra =
            if para.jc == Some(Align::Justify) && !line.is_last && remaining_width > 0.0 {
                // Count inter-word gaps: spaces between items + spaces within text segments
                let gap_count = count_word_gaps(&line.items[ink.clone()]);
                if gap_count > 0 {
                    remaining_width / gap_count as f64
                } else {
                    0.0
                }
            } else {
                0.0
            };

        // A justified line that is not filled (the last one) aligns on the
        // paragraph's start side: the right in a right-to-left paragraph.
        let jc = match para.jc {
            Some(Align::Justify) if para.right_to_left && justify_extra <= 0.0 => Some(Align::End),
            jc => jc,
        };
        let x_offset = match jc {
            Some(Align::Center) => geometry.margin_left + line.indent_left + remaining_width / 2.0,
            Some(Align::End) => geometry.margin_left + line.indent_left + remaining_width,
            Some(Align::Justify) if !line.is_last && justify_extra > 0.0 => {
                // Justified: start from left margin (extra space distributed in gaps)
                geometry.margin_left + line.indent_left
            }
            _ => geometry.margin_left + line.indent_left,
        };

        let mut x = x_offset - hanging_left;
        let mut _accumulated_extra = 0.0;
        let tab_aligned = paragraph_aligned_fields(
            tab_aligned_fields(&line.items),
            &line.items,
            para.jc,
            remaining_width,
        );
        // Each marked item gets a marker over the box it drew, which is what
        // its field gathers. A line that draws one form field only gives the
        // widget its whole text band, from where this line starts (a first
        // line's own indent) to its end, so there is room to type.
        let line_band = sole_form_mark(&line.items).map(|_| Rect {
            x: geometry.margin_left + line.indent_left - hanging_left,
            y: geometry.margin_top + y,
            width: line.available_width + hanging_left,
            height: line.height,
        });
        // The paragraph's bookmarks land at the top of its first line.
        if owner_line == 0 {
            for name in &para.bookmarks {
                elements.push(PositionedElement::Destination {
                    name: name.clone(),
                    point: Point {
                        x: geometry.margin_left + line.indent_left,
                        y: geometry.margin_top + y,
                    },
                });
            }
        }
        let mark_item = |elements: &mut Vec<PositionedElement>,
                         mark: Option<oxoffice_layout::FormMark>,
                         rect: Rect| {
            if let Some(mark) = mark {
                elements.push(form_marker(mark, rect, line_band, geometry.margin_left));
            }
        };

        for (visual_item, item) in line.items.iter().enumerate() {
            // A bookmark that starts here lands here, on this line.
            if let Some(name) = bookmark_target_name(item, &para.bookmark_targets) {
                elements.push(PositionedElement::Destination {
                    name: name.to_owned(),
                    point: Point {
                        x,
                        y: geometry.margin_top + y,
                    },
                });
            }
            match item {
                LineItem::MultilingualText(segment) if !segment.uses_legacy_shaping() => {
                    let first = elements.len();
                    let start = x;
                    x += push_multilingual_text(
                        &mut elements,
                        &mut policies,
                        word_field_owner(word_paint, Some(segment), None)?,
                        segment,
                        x,
                        baseline_y,
                        geometry.margin_top + y,
                        line.height,
                        para.source_node(),
                        if ink.contains(&visual_item) {
                            justify_extra
                        } else {
                            0.0
                        },
                    )?;
                    let element_position = (first..elements.len())
                        .find(|position| {
                            matches!(elements[*position], PositionedElement::MultilingualText(_))
                        })
                        .expect("rich text rendering emits its positioned text");
                    mark_item(
                        &mut elements,
                        segment.base().form_mark,
                        Rect {
                            x: start,
                            y: geometry.margin_top + y,
                            width: x - start,
                            height: line.height,
                        },
                    );
                    text_provenance.push(ReflowTextProvenance {
                        element_position,
                        visual_item,
                        kind: ReflowTextProvenanceKind::Multilingual,
                    });
                }
                LineItem::Text(_) | LineItem::Marker(_) | LineItem::MultilingualText(_) => {
                    let seg = match item {
                        LineItem::Text(segment) | LineItem::Marker(segment) => segment,
                        LineItem::MultilingualText(segment) => segment.base(),
                        _ => unreachable!("matched a text paint owner"),
                    };
                    let adjusted_baseline = baseline_y - seg.baseline_offset;
                    // Word does not widen, underline, strike or highlight the
                    // spaces hanging past the end of a line.
                    let hanging = !ink.contains(&visual_item);

                    // For justified text, compute the extra width from spaces in this segment
                    let segment_spaces = if justify_extra > 0.0 && !hanging {
                        seg.text.chars().filter(|c| *c == ' ').count()
                    } else {
                        0
                    };
                    let segment_extra = segment_spaces as f64 * justify_extra;
                    let effective_width = seg.width + segment_extra;

                    // Render highlight background
                    if let Some(hl_color) = seg.highlight.filter(|_| !hanging) {
                        elements.push(PositionedElement::FilledRect {
                            rect: Rect {
                                x,
                                y: geometry.margin_top + y,
                                width: effective_width,
                                height: line.height,
                            },
                            color: hl_color,
                        });
                    }

                    // Render text, adjusting advances for justified text
                    let advances = if justify_extra > 0.0 && segment_spaces > 0 {
                        // Widen advances for space glyphs
                        distribute_justify_advances(&seg.text, &seg.advances, justify_extra)
                    } else {
                        seg.advances.clone()
                    };

                    let owner = match item {
                        LineItem::MultilingualText(segment) => {
                            word_field_owner(word_paint, Some(segment), None)?
                        }
                        _ => {
                            if seg.field_kind.is_some() || seg.field_source.is_some() {
                                structural_count += 1;
                            }
                            word_field_owner(word_paint, None, Some(seg))?
                        }
                    };
                    policies.emit(
                        &mut elements,
                        PositionedElement::Text(GlyphRun {
                            origin: Point {
                                x,
                                y: adjusted_baseline,
                            },
                            font_id: seg.font_id,
                            font_size: seg.font_size,
                            glyph_ids: seg.glyph_ids.clone(),
                            advances,
                            text: seg.text.clone(),
                            source: match para.source_node() {
                                Some(source_node) => seg.source.and_then(|mut source| {
                                    source.node = source_node?;
                                    Some(source)
                                }),
                                None => seg.source,
                            },
                            color: seg.color,
                            bold: seg.bold,
                            italic: seg.italic,
                            field_kind: seg.field_kind,
                            field_source: match para.source_node() {
                                Some(source_node) => {
                                    seg.field_source.and_then(|mut field_source| {
                                        field_source.node = source_node?;
                                        Some(field_source)
                                    })
                                }
                                None => seg.field_source,
                            },
                            note: seg.note,
                            note_reference_source: match para.source_node() {
                                Some(source_node) => {
                                    seg.note_reference_source.and_then(|mut source| {
                                        source.node = source_node?;
                                        Some(source)
                                    })
                                }
                                None => seg.note_reference_source,
                            },
                            tab_aligned: tab_aligned[visual_item].map(|mut field| {
                                field.start += x_offset;
                                field.end += x_offset;
                                field
                            }),
                        }),
                        owner,
                    )?;
                    text_provenance.push(ReflowTextProvenance {
                        element_position: elements.len() - 1,
                        visual_item,
                        kind: if matches!(item, LineItem::Marker(_)) {
                            ReflowTextProvenanceKind::Marker
                        } else if conditional_hyphens.contains(&visual_item) {
                            ReflowTextProvenanceKind::ConditionalHyphen
                        } else {
                            ReflowTextProvenanceKind::Text
                        },
                    });

                    // Render underline
                    if let Some(ul_style) = seg.underline.filter(|_| !hanging) {
                        let ul_y = adjusted_baseline + seg.descent * 0.3;
                        let (ul_thickness, second) =
                            crate::convert::underline_strokes(ul_style, seg.font_size);
                        elements.push(PositionedElement::Line {
                            start: Point { x, y: ul_y },
                            end: Point {
                                x: x + effective_width,
                                y: ul_y,
                            },
                            width: ul_thickness,
                            color: seg.color,
                            dash_pattern: None,
                        });
                        // Second line for double underline
                        if let Some(offset) = second {
                            let ul_y2 = ul_y + offset;
                            elements.push(PositionedElement::Line {
                                start: Point { x, y: ul_y2 },
                                end: Point {
                                    x: x + effective_width,
                                    y: ul_y2,
                                },
                                width: ul_thickness,
                                color: seg.color,
                                dash_pattern: None,
                            });
                        }
                    }

                    // Render strikethrough
                    if seg.strike && !hanging {
                        let strike_y = adjusted_baseline - seg.ascent * 0.3;
                        let strike_thickness = seg.font_size / 24.0;
                        elements.push(PositionedElement::Line {
                            start: Point { x, y: strike_y },
                            end: Point {
                                x: x + effective_width,
                                y: strike_y,
                            },
                            width: strike_thickness,
                            color: seg.color,
                            dash_pattern: None,
                        });
                    }

                    // Render double strikethrough
                    if seg.dstrike && !hanging {
                        let strike_y = adjusted_baseline - seg.ascent * 0.3;
                        let strike_thickness = seg.font_size / 24.0;
                        let gap = strike_thickness * 2.0;
                        elements.push(PositionedElement::Line {
                            start: Point {
                                x,
                                y: strike_y - gap / 2.0,
                            },
                            end: Point {
                                x: x + effective_width,
                                y: strike_y - gap / 2.0,
                            },
                            width: strike_thickness,
                            color: seg.color,
                            dash_pattern: None,
                        });
                        elements.push(PositionedElement::Line {
                            start: Point {
                                x,
                                y: strike_y + gap / 2.0,
                            },
                            end: Point {
                                x: x + effective_width,
                                y: strike_y + gap / 2.0,
                            },
                            width: strike_thickness,
                            color: seg.color,
                            dash_pattern: None,
                        });
                    }

                    // Render hyperlink annotation
                    if let Some(ref url) = seg.hyperlink_url {
                        elements.push(PositionedElement::LinkAnnotation {
                            rect: Rect {
                                x,
                                y: geometry.margin_top + y,
                                width: effective_width,
                                height: line.height,
                            },
                            url: url.clone(),
                        });
                    }
                    let marked = Rect {
                        x,
                        y: geometry.margin_top + y,
                        width: effective_width,
                        height: line.height,
                    };
                    match seg.form_mark.filter(|_| is_form_anchor(seg)) {
                        // An empty control's anchor gathers only itself; its
                        // widget takes 6 em of the paragraph mark's font from
                        // where it stands towards its line's end, the left
                        // one in a right-to-left paragraph, unless it has
                        // its line's band.
                        Some(mark) => {
                            let start = geometry.margin_left + line.indent_left;
                            let room = if para.right_to_left {
                                x - start
                            } else {
                                start + line.available_width - x
                            };
                            let width = (6.0 * seg.font_size).min(room).max(0.0);
                            elements.push(form_marker(
                                mark,
                                marked,
                                line_band.or(Some(Rect {
                                    x: if para.right_to_left { x - width } else { x },
                                    width,
                                    ..marked
                                })),
                                geometry.margin_left,
                            ));
                        }
                        None => mark_item(&mut elements, seg.form_mark, marked),
                    }

                    _accumulated_extra += segment_extra;
                    x += effective_width;
                }
                LineItem::Tab {
                    width,
                    leader,
                    fill,
                    ..
                } => {
                    // A tab in a filled run is filled across its width,
                    // like the run's text.
                    if let Some(color) = fill {
                        elements.push(PositionedElement::FilledRect {
                            rect: Rect {
                                x,
                                y: geometry.margin_top + y,
                                width: *width,
                                height: line.height,
                            },
                            color: *color,
                        });
                    }
                    mark_item(
                        &mut elements,
                        enclosed_tab_mark(&line.items, visual_item),
                        Rect {
                            x,
                            y: geometry.margin_top + y,
                            width: *width,
                            height: line.height,
                        },
                    );
                    if let Some(leader_seg) = leader {
                        // Render the pre-shaped leader text
                        elements.push(PositionedElement::Text(GlyphRun {
                            origin: Point { x, y: baseline_y },
                            font_id: leader_seg.font_id,
                            font_size: leader_seg.font_size,
                            glyph_ids: leader_seg.glyph_ids.clone(),
                            advances: leader_seg.advances.clone(),
                            text: leader_seg.text.clone(),
                            source: None,
                            color: leader_seg.color,
                            bold: leader_seg.bold,
                            italic: leader_seg.italic,
                            field_kind: None,
                            field_source: None,
                            note: None,
                            note_reference_source: None,
                            tab_aligned: None,
                        }));
                        text_provenance.push(ReflowTextProvenance {
                            element_position: elements.len() - 1,
                            visual_item,
                            kind: ReflowTextProvenanceKind::TabLeader,
                        });
                    }
                    tab_provenance.push(ReflowTabProvenance { visual_item });
                    x += width;
                }
                LineItem::Image {
                    width,
                    height,
                    media_id,
                    form_mark,
                } => {
                    let image = media.get(media_id);
                    // An inline picture stands on the baseline, as in Word.
                    let rect = Rect {
                        x,
                        y: baseline_y - height,
                        width: *width,
                        height: *height,
                    };
                    let image = PositionedElement::Image {
                        rect,
                        data: image.map_or_else(Vec::new, |image| image.data.clone()),
                        content_type: image
                            .map_or_else(String::new, |image| image.content_type.clone()),
                        media_id: *media_id,
                    };
                    elements.push(image);
                    mark_item(&mut elements, *form_mark, rect);
                    x += width;
                }
                LineItem::Group {
                    width,
                    baseline,
                    group,
                    ..
                } => {
                    let mut positioned = group.clone();
                    let group_y =
                        baseline.map_or(geometry.margin_top + y, |baseline| baseline_y - baseline);
                    positioned.transform = positioned.transform.then(oxoffice_layout::Transform {
                        e: x,
                        f: group_y,
                        ..oxoffice_layout::Transform::IDENTITY
                    });
                    if let Some(annotation) = word_paint
                        .map(|paint| paint.annotation_at(owner_line, visual_item))
                        .transpose()?
                        .flatten()
                    {
                        let base_count = annotation.policies.field_count();
                        let child = PaintedElements::from_parts(
                            std::mem::take(&mut positioned.children),
                            annotation.policies.clone(),
                        )?;
                        let (roots, mut root_policies) = child
                            .wrap(PositionedElement::Group(positioned))?
                            .into_parts()?;
                        root_policies.attach_annotation(
                            &roots,
                            &[0],
                            Arc::clone(&annotation.recipe),
                            annotation.kind,
                            base_count,
                            [
                                x.to_bits(),
                                baseline_y.to_bits(),
                                (geometry.margin_top + y).to_bits(),
                                line.height.to_bits(),
                            ],
                        )?;
                        policies.append(
                            &mut elements,
                            PaintedElements::from_parts(roots, root_policies)?,
                        )?;
                    } else if let Some(row) = word_paint.and_then(|paint| {
                        paint
                            .rows
                            .iter()
                            .find(|row| row.line == owner_line && row.item == visual_item)
                    }) {
                        if row.children_len != positioned.children.len()
                            || row.behind_end > row.flow_end
                            || row.flow_end > row.children_len
                            || positioned.opacity != 1.0
                            || !positioned.effects.is_empty()
                        {
                            return Err(oxoffice_layout::LayoutError::Layout(
                                "invalid prepainted Word row owner".into(),
                            ));
                        }
                        let mut child = PaintedElements::from_parts(
                            std::mem::take(&mut positioned.children),
                            row.policies.clone(),
                        )?;
                        let front = child.split_off(row.flow_end)?;
                        let regular = child.split_off(row.behind_end)?;
                        let shell = PositionedElement::Group(positioned);
                        layers.behind.append(&mut child.wrap(shell.clone())?)?;
                        layers.front.append(&mut front.wrap(shell.clone())?)?;
                        policies.append(&mut elements, regular.wrap(shell)?)?;
                    } else {
                        policies.append(
                            &mut elements,
                            if word_paint.is_some() {
                                PaintedElements::non_field(vec![PositionedElement::Group(
                                    positioned,
                                )])?
                            } else {
                                PaintedElements::neutral(vec![PositionedElement::Group(
                                    positioned,
                                )])?
                            },
                        )?;
                    }
                    x += width;
                }
                LineItem::Figure {
                    item, structure_id, ..
                } => {
                    // A picture with alternative text stays on the page under
                    // its control's widget, so its figure keeps its content.
                    let figure_mark = match item.as_ref() {
                        LineItem::Image {
                            width,
                            height,
                            form_mark: Some(mark),
                            ..
                        } => Some((
                            *mark,
                            Rect {
                                x,
                                y: baseline_y - height,
                                width: *width,
                                height: *height,
                            },
                        )),
                        _ => None,
                    };
                    let figure = match item.as_ref() {
                        LineItem::Image {
                            width,
                            height,
                            media_id,
                            ..
                        } => {
                            let image = media.get(media_id);
                            PositionedElement::Image {
                                rect: Rect {
                                    x,
                                    y: baseline_y - height,
                                    width: *width,
                                    height: *height,
                                },
                                data: image.map_or_else(Vec::new, |image| image.data.clone()),
                                content_type: image
                                    .map_or_else(String::new, |image| image.content_type.clone()),
                                media_id: *media_id,
                            }
                        }
                        LineItem::Group {
                            baseline, group, ..
                        } => {
                            let mut positioned = group.clone();
                            let group_y = baseline
                                .map_or(geometry.margin_top + y, |baseline| baseline_y - baseline);
                            positioned.transform =
                                positioned.transform.then(oxoffice_layout::Transform {
                                    e: x,
                                    f: group_y,
                                    ..oxoffice_layout::Transform::IDENTITY
                                });
                            PositionedElement::Group(positioned)
                        }
                        _ => {
                            x += item.width();
                            continue;
                        }
                    };
                    let marked = vec![PositionedElement::MarkedContent {
                        structure: *structure_id,
                        children: vec![figure],
                    }];
                    policies.append(
                        &mut elements,
                        if word_paint.is_some() {
                            PaintedElements::non_field(marked)?
                        } else {
                            PaintedElements::neutral(marked)?
                        },
                    )?;
                    if let Some((mark, rect)) = figure_mark {
                        mark_item(&mut elements, Some(mark), rect);
                    }
                    x += item.width();
                }
                _ => x += item.width(),
            }
        }

        let text_positions = text_provenance
            .iter()
            .map(|provenance| provenance.element_position)
            .collect::<Vec<_>>();
        if let Some(logical_elements) = para.reflow.as_deref().and_then(|reflow| {
            logical_reflow_elements(
                &elements,
                &text_provenance,
                &tab_provenance,
                &reflow.items,
                para.reflow_direction,
                para.source_node(),
            )
        }) {
            let permutation = logical_elements
                .iter()
                .map(|position| {
                    text_positions.binary_search(position).map_err(|_| {
                        oxoffice_layout::LayoutError::Layout(
                            "invalid logical glyph permutation".into(),
                        )
                    })
                })
                .collect::<oxoffice_layout::Result<Vec<_>>>()?;
            policies.permute(&mut elements, &text_positions, &permutation)?;
            y += line.height + bottom;
            continue;
        }
        let mut leading = Vec::new();
        let mut logical_groups = Vec::<((u32, u32, u32), Vec<usize>)>::new();
        for position in &text_positions {
            let element = &elements[*position];
            let source = match &element {
                PositionedElement::Text(run) => run.source,
                PositionedElement::MultilingualText(run) => run.source,
                _ => None,
            };
            if let Some(source) = source {
                logical_groups.push((
                    (source.node.get(), source.char_start, source.char_end),
                    vec![*position],
                ));
            } else if let Some((_, group)) = logical_groups.last_mut() {
                group.push(*position);
            } else {
                leading.push(*position);
            }
        }
        if logical_groups.is_empty() {
            let rich_positions = text_positions
                .into_iter()
                .filter(|index| matches!(elements[*index], PositionedElement::MultilingualText(_)))
                .collect::<Vec<_>>();
            let mut logical_runs = rich_positions.to_vec();
            logical_runs.sort_by_key(|position| match &elements[*position] {
                PositionedElement::MultilingualText(run) => run.logical_index,
                _ => unreachable!("rich positions contain only multilingual text"),
            });
            let permutation = logical_runs
                .iter()
                .map(|position| {
                    rich_positions
                        .binary_search(position)
                        .expect("selected rich slot")
                })
                .collect::<Vec<_>>();
            policies.permute(&mut elements, &rich_positions, &permutation)?;
        } else {
            logical_groups.sort_by_key(|(key, _)| *key);
            let logical_elements = leading
                .into_iter()
                .chain(
                    logical_groups
                        .into_iter()
                        .flat_map(|(_, elements)| elements),
                )
                .collect::<Vec<_>>();
            let permutation = logical_elements
                .iter()
                .map(|position| {
                    text_positions
                        .binary_search(position)
                        .expect("selected logical slot")
                })
                .collect::<Vec<_>>();
            policies.permute(&mut elements, &text_positions, &permutation)?;
        }

        y += line.height;
    }

    if word_paint.is_some_and(|paint| structural_count > paint.structural_count) {
        return Err(oxoffice_layout::LayoutError::Layout(
            "Word structural producer count mismatch".into(),
        ));
    }
    let mut regular = PaintedElements::from_parts(elements, policies)?;
    if let Some(structure_id) = para.structure_id() {
        for position in first_element..regular.len() {
            let structure = match &regular.elements()[position] {
                PositionedElement::Text(run) if run.text.is_empty() && run.glyph_ids.is_empty() => {
                    continue;
                }
                // A link belongs to its paragraph, so a tagged PDF nests its
                // `/Link` element there.
                PositionedElement::Text(_)
                | PositionedElement::MultilingualText(_)
                | PositionedElement::LinkAnnotation { .. } => Some(structure_id),
                PositionedElement::MarkedContent { .. }
                | PositionedElement::FormField(_)
                | PositionedElement::Destination { .. } => continue,
                _ => None,
            };
            let detached =
                regular.splice_at(&[], position..position + 1, &mut PaintedElements::default())?;
            let mut wrapped = detached.wrap(PositionedElement::MarkedContent {
                structure,
                children: Vec::new(),
            })?;
            regular.splice_at(&[], position..position, &mut wrapped)?;
        }
    }
    layers.regular.append(&mut regular)?;
    Ok(layers)
}

pub(super) fn render_change_bar(
    para: &ParagraphBlock,
    start_y: f64,
    height: f64,
    geometry: &PageGeometry,
    page_number: usize,
    elements: &mut Vec<PositionedElement>,
) {
    let Some(color) = para.change_bar else {
        return;
    };
    if !height.is_finite() || height <= 0.0 {
        return;
    }
    render_change_bar_at(
        geometry.margin_top + start_y,
        height,
        color,
        geometry,
        page_number,
        elements,
    );
}

pub(super) fn render_change_bar_at(
    start_y: f64,
    height: f64,
    color: Color,
    geometry: &PageGeometry,
    page_number: usize,
    elements: &mut Vec<PositionedElement>,
) {
    if !height.is_finite() || height <= 0.0 {
        return;
    }
    let x = if page_number.is_multiple_of(2) {
        geometry.margin_left / 2.0
    } else {
        geometry.page_width - geometry.margin_right / 2.0
    };
    if !x.is_finite() || !start_y.is_finite() || !(start_y + height).is_finite() {
        return;
    }
    elements.push(PositionedElement::Line {
        start: Point { x, y: start_y },
        end: Point {
            x,
            y: start_y + height,
        },
        width: 1.5,
        color,
        dash_pattern: None,
    });
}

/// Count inter-word gap positions in a line (spaces within text segments).
fn count_word_gaps(items: &[LineItem]) -> usize {
    let mut count = 0;
    for item in items {
        match item {
            LineItem::Text(seg) | LineItem::Marker(seg) => {
                count += seg.text.chars().filter(|c| *c == ' ').count();
            }
            LineItem::MultilingualText(seg) => {
                count += seg.text().chars().filter(|c| *c == ' ').count();
            }
            LineItem::Tab { .. } => {
                count += 1;
            }
            _ => {}
        }
    }
    count
}

/// Distribute extra justify space across advances by widening space-character advances.
fn distribute_justify_advances(text: &str, advances: &[f64], extra_per_gap: f64) -> Vec<f64> {
    let chars: Vec<char> = text.chars().collect();
    let mut result = advances.to_vec();

    if chars.len() == result.len() {
        // 1:1 char-to-glyph mapping
        for (i, &ch) in chars.iter().enumerate() {
            if ch == ' ' {
                result[i] += extra_per_gap;
            }
        }
    } else {
        // Fallback: distribute evenly across all glyphs
        let total_extra = extra_per_gap * text.chars().filter(|c| *c == ' ').count() as f64;
        if !result.is_empty() {
            let per_glyph = total_extra / result.len() as f64;
            for a in &mut result {
                *a += per_glyph;
            }
        }
    }

    result
}
