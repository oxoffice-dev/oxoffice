//! Text wrapping around anchored drawings and floating tables, and lines reflowed around them.

use super::*;

/// Where a pass placed the wrapping drawings whose vertical anchor is their own
/// paragraph, keyed by block index and the drawing's index within that block.
///
/// The key is stable across passes because both passes walk the same block list
/// in the same order.
pub(super) type ResolvedWraps = HashMap<(usize, usize), (usize, PlacedWrap)>;

/// Whether any block anchors a wrapping drawing, or a floating table, to its
/// own paragraph or line.
///
/// A document without one paginates in a single pass, which is every sample and
/// every corpus document today. A table that does not float, or that floats
/// against the page or a margin, keeps it that way: its vertical position does
/// not depend on where the flow put it, so one pass already knows where it is.
pub(super) fn has_paragraph_relative_wrap<B: LayoutBlockLike>(blocks: &[B]) -> bool {
    blocks.iter().any(|block| {
        if let Some(table) = block.table() {
            return table
                .floating
                .as_deref()
                .is_some_and(is_paragraph_relative_float);
        }
        let Some(para) = block.paragraph() else {
            return false;
        };
        para.anchored.iter().any(is_paragraph_relative_wrap)
    })
}

/// The filter both the look-ahead and the two-pass predicate agree on.
pub(super) fn is_paragraph_relative_wrap(anchored: &AnchoredDrawing) -> bool {
    anchored.wrap != WrapType::None
        && matches!(
            anchored.rel_v,
            ST_RelativeFromV::Paragraph | ST_RelativeFromV::Line
        )
}

/// The same filter for a floating table, which always wraps.
///
/// A `w:vertAnchor="text"` float is measured from the block that carries it, so
/// it has no vertical position until the flow has placed that block.
pub(super) fn is_paragraph_relative_float(floating: &FloatingTable) -> bool {
    matches!(
        floating.rel_v,
        ST_RelativeFromV::Paragraph | ST_RelativeFromV::Line
    )
}

/// Drop a floating table below every float on the page it may not overlap.
///
/// `w:tblOverlap` is a rule between floating tables, so `placed` holds only
/// those, each with whether it lets another float overlap it. Two floats that
/// both allow the overlap are left intersecting, and otherwise the later one in
/// body order is the one that moves. Each sweep moves the candidate strictly
/// downward, so one sweep per already placed float settles it.
///
/// Resolution is within one page. Facing-page and section-scoped resolution is
/// not implemented.
pub(super) fn settle_float(
    candidate: &mut PlacedWrap,
    overlap_allowed: bool,
    placed: &[(PlacedWrap, bool)],
) {
    for _ in 0..placed.len() {
        let mut moved = false;
        for (other, other_allows) in placed {
            if (overlap_allowed && *other_allows) || !rects_intersect(&candidate.rect, &other.rect)
            {
                continue;
            }
            candidate.rect.y = other.keep_out_bottom() + candidate.dist_top;
            moved = true;
        }
        if !moved {
            break;
        }
    }
}

/// Whether two placed rectangles share any area.
///
/// Touching edges do not count, so two floats stacked exactly band to band are
/// settled rather than pushed again.
fn rects_intersect(left: &Rect, right: &Rect) -> bool {
    left.x < right.x + right.width
        && right.x < left.x + left.width
        && left.y < right.y + right.height
        && right.y < left.y + left.height
}

/// The keep-out band a floating table asks the text around it to respect.
///
/// A float takes its origin entirely from the resolved anchor, which is what
/// `w:tblpPr` means, so `table_indent` does not reach this function.
pub(super) fn floating_wrap(
    floating: &FloatingTable,
    width: f64,
    height: f64,
    geometry: &PageGeometry,
    written: &PageGeometry,
    para_top: f64,
) -> PlacedWrap {
    PlacedWrap {
        rect: Rect {
            x: resolve_anchor_h(
                floating.rel_h,
                floating.off_h,
                floating.align_h,
                width,
                if matches!(
                    floating.rel_h,
                    ST_RelativeFromH::Column | ST_RelativeFromH::Character
                ) {
                    geometry
                } else {
                    written
                },
                0.0,
            ),
            y: resolve_anchor_v(
                floating.rel_v,
                floating.off_v,
                floating.align_v,
                height,
                written,
                geometry.margin_top + para_top - written.margin_top,
            ),
            width,
            height,
        },
        wrap: WrapType::Square,
        dist_top: floating.dist_top,
        dist_bottom: floating.dist_bottom,
        dist_left: floating.dist_left,
        dist_right: floating.dist_right,
    }
}

/// Body pagination retains its established two-pass approximation. Selected
/// furniture must prove reservations against its final line bands before paint.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum WrapReflowPolicy {
    Body,
    Furniture,
}

#[derive(Debug, PartialEq)]
struct LineReservations {
    offset_top: f64,
    prefix: Vec<f64>,
    suffix: Vec<f64>,
    before_lines: Vec<f64>,
}

/// Which side of the text a wrapping float stands on (`true` for the left)
/// and the width it keeps text out of there, or `None` when it reserves none.
fn wrap_side(wrap: &PlacedWrap, text_left: f64, text_right: f64) -> Option<(bool, f64)> {
    let on_the_left = wrap.rect.x + wrap.rect.width / 2.0 < (text_left + text_right) / 2.0;
    let reserve = if on_the_left {
        (wrap.rect.x + wrap.rect.width + wrap.dist_right - text_left).max(0.0)
    } else {
        (text_right - (wrap.rect.x - wrap.dist_left)).max(0.0)
    };
    (reserve > 0.0).then_some((on_the_left, reserve))
}

#[allow(clippy::too_many_arguments)]
fn line_reservations(
    para: &ParagraphBlock,
    lines: &[LayoutLine],
    wraps: &[PlacedWrap],
    paragraph_top: f64,
    top_band: f64,
    geometry: &PageGeometry,
    policy: WrapReflowPolicy,
) -> LineReservations {
    let mut offset_top: f64 = 0.0;
    if policy == WrapReflowPolicy::Body {
        for wrap in wraps
            .iter()
            .filter(|wrap| wrap.wrap == WrapType::TopAndBottom)
        {
            if wrap.keep_out_top() <= paragraph_top + 1.0 && wrap.keep_out_bottom() > paragraph_top
            {
                offset_top = offset_top.max(wrap.keep_out_bottom() - paragraph_top);
            }
        }
    }
    let text_left = geometry.margin_left + para.indent_left;
    let text_right = geometry.page_width - geometry.margin_right - para.indent_right;
    let mut prefix = Vec::new();
    let mut suffix = Vec::new();
    let mut before_lines = Vec::new();
    let mut line_top = paragraph_top + offset_top + top_band;
    // These are the owning line breaker's restored Word-height lines, without
    // synthetic gaps. Earlier lines stay above a later object. Only a content
    // band that intersects it advances to the next clear line position.
    for (index, line) in lines.iter().enumerate() {
        // The line after a clearing break starts below the floats on the
        // break's side that share its band.
        let clear =
            index
                .checked_sub(1)
                .and_then(|previous| match lines[previous].forced_break_after {
                    Some(ForcedBreakKind::Line(clear)) if clear != LineClear::None => Some(clear),
                    _ => None,
                });
        if policy == WrapReflowPolicy::Furniture || clear.is_some() {
            let before = line_top;
            // Every sweep moves below at least one newly reached obstacle.
            // Reversed order and touching chains need at most this many sweeps.
            for _ in 0..wraps.len() {
                let previous = line_top;
                for wrap in wraps {
                    let moves_below = match wrap.wrap {
                        WrapType::TopAndBottom => policy == WrapReflowPolicy::Furniture,
                        WrapType::Square | WrapType::Tight | WrapType::Through => clear
                            .is_some_and(|clear| {
                                wrap_side(wrap, text_left, text_right).is_some_and(|(left, _)| {
                                    match clear {
                                        LineClear::Left => left,
                                        LineClear::Right => !left,
                                        LineClear::All | LineClear::None => true,
                                    }
                                })
                            }),
                        WrapType::None => false,
                    };
                    if moves_below
                        && wrap.keep_out_top() < line_top + line.height
                        && wrap.keep_out_bottom() > line_top
                    {
                        line_top = line_top.max(wrap.keep_out_bottom());
                    }
                }
                if line_top == previous {
                    break;
                }
            }
            let gap = line_top - before;
            if index == 0 {
                offset_top = gap;
            } else if gap > 0.0 {
                before_lines.resize(index + 1, 0.0);
                before_lines[index] = gap;
            }
        }
        for wrap in wraps
            .iter()
            .filter(|wrap| wrap.wrap != WrapType::TopAndBottom)
        {
            if line_top + line.height <= wrap.keep_out_top() || line_top >= wrap.keep_out_bottom() {
                continue;
            }
            let Some((on_the_left, reserve)) = wrap_side(wrap, text_left, text_right) else {
                continue;
            };
            let target = if on_the_left {
                &mut prefix
            } else {
                &mut suffix
            };
            if target.len() <= index {
                target.resize(index + 1, 0.0);
            }
            if policy == WrapReflowPolicy::Furniture {
                // One side's keep-out envelope is a union, including tables
                // whose existing policy explicitly permits overlap.
                target[index] = f64::max(target[index], reserve);
            } else {
                target[index] += reserve;
            }
        }
        line_top += line.height;
    }
    LineReservations {
        offset_top,
        prefix,
        suffix,
        before_lines,
    }
}

/// Re-break a paragraph so its text flows around the wrapping drawings that
/// share its band of the page.
///
/// `para_top` is where the paragraph's content starts, measured from the top of
/// the content area. Returns `None` when nothing applies, so the caller keeps
/// the paragraph it already has.
pub(super) fn reflow_around_wraps(
    para: &ParagraphBlock,
    context: (oxoffice_layout::TextDirection, Option<&WordParagraphPaint>),
    wraps: &[PlacedWrap],
    para_top: f64,
    geometry: &PageGeometry,
    fm: &FontManager,
    policy: WrapReflowPolicy,
) -> Result<Option<WrapReflow>, String> {
    let (reflow_direction, word_paint) = context;
    let mut paint = word_paint.cloned();
    if wraps.is_empty() {
        return Ok(None);
    }
    let top = geometry.margin_top + para_top;
    if !wraps.iter().any(|wrap| {
        wrap.keep_out_bottom() > top && wrap.keep_out_top() < top + para.content_height()
    }) {
        return Ok(None);
    }
    let Some(reflow) = para.reflow.as_ref() else {
        // Public neutral pre-laid blocks have no Word producer companion.
        // Keep their established placement when no rebreak recipe was supplied.
        if word_paint.is_none() && policy == WrapReflowPolicy::Body {
            return Ok(None);
        }
        return Err("intersecting furniture flow has no retained line-break input".to_owned());
    };
    let reflow_items = reflow.items.as_slice();

    let mut lines = para.lines.clone();
    let (top_band, bottom_band) = para.borders.as_ref().map_or((0.0, 0.0), |borders| {
        (
            crate::block::paragraph_border_padding(borders.top.as_ref()),
            crate::block::paragraph_border_padding(borders.bottom.as_ref()),
        )
    });
    // Every reservation pass sees content lines at the same painted origin.
    // Fresh line-breaker output has no border bands. Strip the original bands
    // before the first pass and restore them once after the final re-break.
    if let Some(first) = lines.first_mut() {
        first.height -= top_band;
        first.ascent -= top_band;
    }
    if let Some(last) = lines.last_mut() {
        last.height -= bottom_band;
    }
    let mut offset_top = 0.0;
    let mut line_gaps = Vec::new();

    // Furniture has a separate per-paragraph32-rebreak work guard. It is
    // distinct from the outer page's32 candidate preparations and does not
    // claim a native or performance budget. Every rebreak uses original items.
    let passes = if policy == WrapReflowPolicy::Furniture {
        32
    } else {
        2
    };
    let mut seen = Vec::new();
    let mut consistent = policy == WrapReflowPolicy::Body;
    for pass in 0..passes {
        let required = line_reservations(para, &lines, wraps, top, top_band, geometry, policy);
        if required.prefix.is_empty()
            && required.suffix.is_empty()
            && required.offset_top == 0.0
            && required.before_lines.is_empty()
            && (policy == WrapReflowPolicy::Body || pass == 0)
        {
            return Ok(None);
        }
        let mut params = reflow.params.clone();
        params.line_prefix_widths = required.prefix.clone();
        params.line_suffix_widths = required.suffix.clone();

        let rich = word_paint.map_or_else(
            || {
                reflow_direction != oxoffice_layout::TextDirection::Auto
                    || reflow_items.iter().any(|item| {
                        matches!(item, oxoffice_layout::InlineItem::MultilingualText(_))
                            || matches!(
                                item,
                                oxoffice_layout::InlineItem::Text(segment)
                                    | oxoffice_layout::InlineItem::HyphenatedText { segment, .. }
                                    | oxoffice_layout::InlineItem::Marker(segment)
                                    if segment.direction != oxoffice_layout::TextDirection::Auto
                            )
                    })
            },
            |paint| paint.rich_layout,
        );
        let (mut reflowed, origins) = oxoffice_layout::line::break_into_lines_with_opaque_origins(
            reflow_items,
            &params,
            fm,
            rich.then_some(reflow_direction),
        )
        .map_err(|error| format!("furniture line reflow failed: {error}"))?;
        if let Some(paint) = &mut paint {
            paint
                .bind_annotation_origins(&reflowed, &origins)
                .map_err(|error| error.to_string())?;
        }
        // The re-break went through the generic line breaker, which knows
        // nothing about Word's line measure or the section grid, so both are
        // applied again over its result, as they were to the first break. It
        // runs inside the loop, so the second pass reserves against the
        // heights the first will actually paint. A paragraph off the grid
        // carries no pitch.
        crate::convert::restore_word_line_heights(
            &mut reflowed,
            reflow.params.line_spacing,
            reflow.grid_line_pitch_pt,
            fm,
            reflow.paragraph_mark,
        );
        if policy == WrapReflowPolicy::Furniture {
            let actual = line_reservations(para, &reflowed, wraps, top, top_band, geometry, policy);
            // Positive final-band validation: exactly these reservations were
            // supplied to the owning line breaker. Equal repeated output alone
            // cannot certify a newly reached obstacle or transitive clearance.
            if actual == required {
                lines = reflowed;
                offset_top = required.offset_top;
                line_gaps = required.before_lines;
                consistent = true;
                break;
            }
            let state = format!("{reflowed:?}{actual:?}");
            if seen.contains(&state) {
                return Err("furniture line reservations repeat a non-stable state".into());
            }
            seen.push(state);
        }
        lines = reflowed;
        offset_top = required.offset_top;
    }
    if !consistent {
        return Err("furniture line reservations did not stabilize within32 rebreaks".into());
    }
    if policy == WrapReflowPolicy::Body
        && lines.iter().any(|line| {
            matches!(
                line.forced_break_after,
                Some(ForcedBreakKind::Line(clear)) if clear != LineClear::None
            )
        })
    {
        // Body pagination measures lines and no gaps between them, so the
        // gap a clearing break opens goes above the text of the line it
        // moves down. It is measured on the final lines.
        let gaps = line_reservations(para, &lines, wraps, top, top_band, geometry, policy);
        for (line, gap) in lines.iter_mut().zip(gaps.before_lines) {
            line.height += gap;
            line.ascent += gap;
        }
    }

    let mut adjusted = para.clone();
    adjusted.lines = lines;
    adjusted.content_offset_top = offset_top;
    if adjusted.absorbed_by_section_break {
        absorb_into_section_break(&mut adjusted);
    }
    // Raw Word metrics stay unchanged. The prepared furniture owner carries
    // proved later gaps separately to both occupied-band paint and pricing.
    adjusted.add_border_padding();
    Ok(Some(WrapReflow {
        paint,
        paragraph: adjusted,
        before_lines: line_gaps,
    }))
}
