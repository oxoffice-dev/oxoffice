//! Paragraphs on pages: lines split across pages, keep-with-next chains and painting a split.

use super::*;

pub(super) fn paginate_paragraph<B: LayoutBlockLike>(
    para: ParagraphView<'_>,
    body_index: Option<usize>,
    block_idx: usize,
    blocks: &[B],
    pager: &mut Pager,
) {
    let para = ParagraphView {
        line_start: 0,
        paint: para.paint,
        baseline_policy: para
            .semantics
            .and_then(|semantics| semantics.tb_rl_exact_baseline)
            .map(|(height, central)| (height, central, pager.fm)),
        ..para
    };
    let space_before = pager.space_before(para.space_before);

    // Flow the paragraph around anything floating in its band of the page,
    // before anything is measured. A reflow changes the paragraph's height, so
    // doing it after the fitting decision would measure the wrong thing.
    let reflowed = {
        let para_top = pager.cursor_y + space_before;
        let mut wraps = pager.page_wraps.clone();
        wraps.extend(pager.wrap_rects_for(&para.anchored, para_top, para.indent_left));
        wraps.extend(pager.lookahead_wraps(block_idx, blocks));
        para.reflow_allowed
            .then(|| {
                reflow_around_wraps(
                    para.block,
                    (
                        para.reflow_direction,
                        para.paint
                            .or_else(|| para.semantics.map(|semantics| semantics.paint.as_ref())),
                    ),
                    &wraps,
                    para_top,
                    &pager.geometry,
                    pager.fm,
                    WrapReflowPolicy::Body,
                )
                .map_err(|reason| {
                    pager.pending_furniture_failure.get_or_insert(reason);
                })
            })
            .transpose()
    };
    let reflowed = match reflowed {
        Ok(value) => value.flatten(),
        Err(()) => return,
    };
    let para = reflowed.as_ref().map_or(para, |reflowed| ParagraphView {
        line_start: 0,
        paint: reflowed.paint.as_ref().or(para.paint),
        baseline_policy: para.baseline_policy,
        line_gaps: &reflowed.before_lines,
        border_group_before: para.border_group_before,
        block: &reflowed.paragraph,
        semantics: para.semantics,
        reflow_direction: para.reflow_direction,
        reflow_allowed: false,
    });

    // A page break inside the paragraph ends the page after its line, whatever
    // widow control or keep-lines would decide. When the lines up to the break
    // do not fit, the checks below split or move the paragraph first, and the
    // continuation meets the break again.
    if let Some(split_at) = forced_page_split(&para.lines)
        && pager.count_lines_that_fit_with_notes(
            &para.lines[..split_at],
            pager.cursor_y + space_before + para.content_offset_top,
        ) == split_at
    {
        render_para_split(para, body_index, split_at, space_before, pager, block_idx);
        return;
    }

    // Check if paragraph fits on current page. The note area its references
    // will demand is priced in, but not claimed: the paragraph may yet move to
    // the next page, and its notes must move with it.
    let total_needed = space_before + para.content_height();
    let remaining = pager.available_height_for(&para.lines) - pager.cursor_y;

    if total_needed > remaining && pager.has_content() {
        // Paragraph doesn't fit. Decide: move whole or split.
        if para.keep_lines || para.lines.len() <= 2 {
            pager.advance_flow_before(block_idx);
            if pager.stopped_at.is_some() {
                return;
            }
            // Re-call with fresh page
            paginate_paragraph(para, body_index, block_idx, blocks, pager);
            return;
        }

        // The lines start below any drawing the paragraph must clear, so the
        // counter has to be told where they actually begin.
        let lines_that_fit = pager.count_lines_that_fit_with_notes(
            &para.lines,
            pager.cursor_y + space_before + para.content_offset_top,
        );

        if para.widow_control && lines_that_fit < 2 {
            // Can't fit enough lines — move whole paragraph
            pager.advance_flow_before(block_idx);
            if pager.stopped_at.is_some() {
                return;
            }
            paginate_paragraph(para, body_index, block_idx, blocks, pager);
            return;
        }

        let lines_remaining = para.lines.len() - lines_that_fit;
        if para.widow_control && lines_remaining < 2 && lines_that_fit >= 3 {
            // Would leave orphan — move one line to next page
            let split_at = lines_that_fit - 1;
            render_para_split(para, body_index, split_at, space_before, pager, block_idx);
            return;
        }

        if lines_that_fit > 0 {
            render_para_split(
                para,
                body_index,
                lines_that_fit,
                space_before,
                pager,
                block_idx,
            );
            return;
        }

        // No lines fit (shouldn't happen since we checked has_content above)
        pager.advance_flow_before(block_idx);
        if pager.stopped_at.is_some() {
            return;
        }
        paginate_paragraph(para, body_index, block_idx, blocks, pager);
        return;
    }

    // Paragraph fits OR we're at the top of a page
    // If it doesn't fit and we're at the top, we must split line by line
    if total_needed > pager.available_height_for(&para.lines) && pager.cursor_y == 0.0 {
        // Paragraph is taller than a page; split line by line
        let lines_that_fit =
            pager.count_lines_that_fit_with_notes(&para.lines, para.content_offset_top);
        if lines_that_fit > 0 && lines_that_fit < para.lines.len() {
            render_para_split(para, body_index, lines_that_fit, 0.0, pager, block_idx);
            return;
        }
    }

    // Keep with next: the first paragraph of a keep-with-next chain moves to
    // the next page unless the whole chain fits below it, as in Word. The
    // paragraphs after it follow it, so a chain no page could hold breaks
    // where the page ends.
    let first_of_chain = !block_idx
        .checked_sub(1)
        .and_then(|previous| blocks[previous].paragraph())
        .is_some_and(|previous| previous.keep_next);
    if para.keep_next && first_of_chain && pager.has_content() {
        let chain = keep_next_chain_height(para.block, block_idx, blocks, pager);
        if pager.cursor_y + space_before + chain > pager.available_height_for(&para.lines) {
            pager.advance_flow_for_chain();
        }
    }

    // Render the paragraph. The ordinary collapsed spacing remains outside
    // line metrics, but the grouped sides must bridge that priced gap.
    let border_gap = if para.border_group_before && pager.cursor_y > 0.0 {
        pager.previous_space_after + pager.space_before(para.space_before)
    } else {
        0.0
    };
    pager.record_pending_outline();
    pager.cursor_y += pager.space_before(para.space_before);

    if let Some(body_index) = body_index {
        pager.record_body_fragment(
            body_index,
            pager.geometry.margin_left + para.indent_left,
            pager.geometry.margin_top + pager.cursor_y,
            pager.geometry.content_width() - para.indent_left - para.indent_right,
            para.content_height(),
        );
    }

    if let Some(shading) = para.shading {
        pager.elements.push(PositionedElement::FilledRect {
            rect: Rect {
                x: pager.geometry.margin_left + para.indent_left,
                y: pager.geometry.margin_top + pager.cursor_y,
                width: pager.geometry.content_width() - para.indent_left - para.indent_right,
                height: para.content_height(),
            },
            color: shading,
        });
    }

    // Anchored drawings resolve against the paragraph's position, so place
    // them now that the page and the cursor are settled.
    pager.place_anchored(
        &para.anchored,
        para.paint
            .or_else(|| para.semantics.map(|semantics| semantics.paint.as_ref())),
        pager.cursor_y,
        para.indent_left,
        block_idx,
    );

    pager.accept_layers(render_paragraph_lines(
        &para.lines,
        para,
        &pager.geometry,
        pager.cursor_y,
        (true, true, border_gap),
        pager.media,
    ));
    pager.record_numbered_lines(
        &para.lines,
        para,
        pager.cursor_y + para.content_offset_top,
        true,
    );
    render_change_bar(
        para.block,
        pager.cursor_y,
        para.content_height(),
        &pager.geometry,
        pager.page_number,
        &mut pager.elements,
    );
    pager.claim_notes(&para.lines);
    pager.cursor_y += para.content_height();
    pager.ink_bottom = pager.cursor_y;
    pager.cursor_y += para.space_after;
    pager.previous_space_after = para.space_after;
    pager.mark_content();
}

/// Height a keep-with-next chain needs below the space before its first
/// paragraph, as Word keeps it on one page: that paragraph, every following
/// paragraph that also keeps with its next, the spacing between them, and then
/// the opening of the block that ends the chain. That opening is a table's
/// first row, or the lines a paragraph's own pagination cannot leave alone at
/// the foot of a page: all of them under keep-lines or when there are two or
/// fewer, two under widow control, else one, and never past a page break
/// inside it. A page break before a block ends the chain without it, and the
/// walk stops once the chain is taller than a page.
fn keep_next_chain_height<B: LayoutBlockLike>(
    para: &ParagraphBlock,
    block_idx: usize,
    blocks: &[B],
    pager: &Pager,
) -> f64 {
    let mut height = para.content_height();
    let mut space_after = para.space_after;
    for next in &blocks[block_idx + 1..] {
        if next.page_break_before() || height > pager.content_height {
            break;
        }
        let Some(paragraph) = next.paragraph() else {
            if let Some(table) = next.table() {
                height += space_after + table.rows.first().map_or(0.0, |row| row.height);
            }
            break;
        };
        height += if pager.geometry.do_not_use_html_paragraph_auto_spacing {
            space_after + paragraph.space_before
        } else {
            space_after.max(paragraph.space_before)
        };
        let page_break = forced_page_split(&paragraph.lines);
        if paragraph.keep_next && page_break.is_none() {
            height += paragraph.content_height();
            space_after = paragraph.space_after;
            continue;
        }
        let opening = if paragraph.keep_lines || paragraph.lines.len() <= 2 {
            paragraph.lines.len()
        } else if paragraph.widow_control {
            2
        } else {
            1
        };
        let opening = page_break.map_or(opening, |split_at| opening.min(split_at));
        height += paragraph.content_offset_top
            + paragraph.lines[..opening]
                .iter()
                .map(|line| line.height)
                .sum::<f64>();
        break;
    }
    height
}

/// Return the line boundary immediately after the first page break that has a
/// continuation. A trailing page break has no following content to move.
fn forced_page_split(lines: &[LayoutLine]) -> Option<usize> {
    lines
        .iter()
        .position(|line| line.forced_break_after == Some(ForcedBreakKind::Page))
        .map(|index| index + 1)
        .filter(|&split_at| split_at < lines.len())
}

/// Split a paragraph at the given line index, rendering first part on current page
/// and continuing the rest on a new page (recursively if needed).
fn render_para_split(
    para: ParagraphView<'_>,
    body_index: Option<usize>,
    split_at: usize,
    space_before: f64,
    pager: &mut Pager,
    block_idx: usize,
) {
    // Render lines before split on current page.
    let border_gap = if para.border_group_before && pager.cursor_y > 0.0 {
        pager.previous_space_after + space_before
    } else {
        0.0
    };
    pager.record_pending_outline();
    pager.cursor_y += space_before;
    // A split paragraph anchors its drawings to where it starts.
    pager.place_anchored(
        &para.anchored,
        para.paint
            .or_else(|| para.semantics.map(|semantics| semantics.paint.as_ref())),
        pager.cursor_y,
        para.indent_left,
        block_idx,
    );
    pager.accept_layers(render_paragraph_lines(
        &para.lines[..split_at],
        para,
        &pager.geometry,
        pager.cursor_y,
        (true, false, border_gap),
        pager.media,
    ));
    pager.record_numbered_lines(
        &para.lines[..split_at],
        para,
        pager.cursor_y + para.content_offset_top,
        true,
    );
    let first_height = para.content_offset_top
        + para.lines[..split_at]
            .iter()
            .map(|line| line.height)
            .sum::<f64>();
    if let Some(body_index) = body_index {
        pager.record_body_fragment(
            body_index,
            pager.geometry.margin_left + para.indent_left,
            pager.geometry.margin_top + pager.cursor_y,
            pager.geometry.content_width() - para.indent_left - para.indent_right,
            first_height,
        );
    }
    render_change_bar(
        para.block,
        pager.cursor_y,
        first_height,
        &pager.geometry,
        pager.page_number,
        &mut pager.elements,
    );
    // Only the lines placed on this page count toward its notes. The rest of
    // the paragraph, and any note it references, belong to the next page.
    pager.claim_notes(&para.lines[..split_at]);
    pager.ink_bottom = pager.cursor_y
        + para.content_offset_top
        + para.lines[..split_at].iter().map(|l| l.height).sum::<f64>();
    pager.mark_content();
    pager.finish_page();

    // Handle remaining lines, which may themselves need splitting
    let remaining_lines = &para.lines[split_at..];
    let remaining_height: f64 = remaining_lines.iter().map(|l| l.height).sum();
    let ends_with_run_page_break = para.shading.is_none()
        && para.borders.is_none()
        && para.change_bar.is_none()
        && para.content_offset_top == 0.0
        && para.lines[split_at - 1].forced_break_after == Some(ForcedBreakKind::Page)
        && remaining_lines.iter().all(|line| {
            line.forced_break_after.is_none()
                && line.items.iter().all(|item| match item {
                    LineItem::Text(segment) | LineItem::Marker(segment) => segment.text.is_empty(),
                    _ => false,
                })
        });

    // Split again where the remaining lines are still too tall, or at a page
    // break among them, whichever comes first.
    let overflow_split = (remaining_height > pager.available_height_for(remaining_lines))
        .then(|| pager.count_lines_that_fit_with_notes(remaining_lines, 0.0))
        .filter(|&fit| fit > 0 && fit < remaining_lines.len());
    let next_split = match (overflow_split, forced_page_split(remaining_lines)) {
        (Some(fit), Some(forced)) => Some(fit.min(forced)),
        (fit, forced) => fit.or(forced),
    };
    if let Some(lines_that_fit) = next_split {
        // Build a temporary paragraph with the remaining lines.
        let temp_para = ParagraphBlock {
            // The anchors were placed with the first part of the paragraph, so
            // the continuation must not place them again.
            anchored: Vec::new(),
            change_bar: para.change_bar,
            lines: remaining_lines.to_vec(),
            space_before: 0.0,
            space_after: para.space_after,
            borders: para.borders.clone().map(|mut borders| {
                borders.top = None;
                borders
            }),
            shading: para.shading,
            indent_left: para.indent_left,
            indent_right: para.indent_right,
            jc: para.jc,
            right_to_left: para.right_to_left,
            keep_next: para.keep_next,
            keep_lines: false,
            page_break_before: false,
            widow_control: para.widow_control,
            heading_level: None,
            heading_text: None,
            // The bookmarks land on the first fragment; one with a marker
            // lands where its marker is drawn, maybe in this one.
            bookmarks: Vec::new(),
            bookmark_targets: para.bookmark_targets.clone(),
            list: para.list,
            structure_id: para.structure_id(),
            // The continuation keeps the logical input sequence for text
            // extraction. `reflow_allowed` below prevents a second break.
            reflow: para.reflow.clone(),
            content_offset_top: 0.0,
            absorbed_by_section_break: para.absorbed_by_section_break,
        };
        let continuation_paint = para
            .paint
            .or_else(|| para.semantics.map(|semantics| semantics.paint.as_ref()))
            .map(|paint| {
                let mut selected = paint.for_line_range(&para.lines, split_at..para.lines.len())?;
                selected.anchors.clear();
                Ok::<_, oxoffice_layout::LayoutError>(selected)
            })
            .transpose();
        let continuation_paint = match continuation_paint {
            Ok(paint) => paint,
            Err(error) => {
                pager
                    .pending_furniture_failure
                    .get_or_insert(error.to_string());
                return;
            }
        };
        render_para_split(
            ParagraphView {
                line_start: 0,
                paint: continuation_paint.as_ref(),
                baseline_policy: para.baseline_policy,
                line_gaps: &[],
                border_group_before: para.border_group_before,
                block: &temp_para,
                semantics: para.semantics,
                reflow_direction: para.reflow_direction,
                reflow_allowed: false,
            },
            body_index,
            lines_that_fit,
            0.0,
            pager,
            block_idx,
        );
        return;
    }

    // Remaining fits on the new page
    if let Some(body_index) = body_index {
        pager.record_body_fragment(
            body_index,
            pager.geometry.margin_left + para.indent_left,
            pager.geometry.margin_top,
            pager.geometry.content_width() - para.indent_left - para.indent_right,
            remaining_height,
        );
    }
    pager.accept_layers(render_paragraph_lines(
        remaining_lines,
        ParagraphView {
            line_start: para.line_start + split_at,
            ..para
        },
        &pager.geometry,
        0.0,
        (false, true, 0.0),
        pager.media,
    ));
    pager.record_numbered_lines(remaining_lines, para, 0.0, false);
    render_change_bar(
        para.block,
        0.0,
        remaining_height,
        &pager.geometry,
        pager.page_number,
        &mut pager.elements,
    );
    pager.claim_notes(remaining_lines);
    pager.ink_bottom = remaining_height;
    pager.cursor_y = remaining_height + para.space_after;
    pager.previous_space_after = para.space_after;
    pager.mark_content();
    if ends_with_run_page_break {
        pager.trailing_run_page_break_before = Some(block_idx + 1);
    }
}

/// A rotated exact line keeps its raw advance and font metrics. This private
/// paint baseline has separate line-height and actual face-metric authorities.
/// Decorations and occupied container bands still use the restored line box.
pub(super) fn paragraph_line_baseline(
    line: &LayoutLine,
    policy: Option<(f64, bool, &FontManager)>,
) -> Option<f64> {
    let (height, central, fm) = policy?;
    if !height.is_finite() || height <= 0.0 {
        return None;
    }
    fn collect(item: &LineItem, fm: &FontManager, central: &mut Option<f64>, failed: &mut bool) {
        let mut face = |font, size| match fm.metrics(font, size) {
            Ok(metrics) => {
                let value = (metrics.ascent - metrics.descent) / 2.0;
                if value.is_finite() {
                    *central = Some((*central).map_or(value, |prior| prior.max(value)));
                } else {
                    *failed = true;
                }
            }
            Err(_) => *failed = true,
        };
        match item {
            LineItem::Text(segment) | LineItem::Marker(segment) => {
                face(segment.font_id, segment.font_size)
            }
            LineItem::MultilingualText(segment) => {
                face(segment.base().font_id, segment.base().font_size)
            }
            LineItem::Tab {
                leader: Some(segment),
                ..
            } => face(segment.font_id, segment.font_size),
            LineItem::Group { group, .. } => {
                oxoffice_layout::walk(&group.children, &mut |element, _| match element {
                    PositionedElement::Text(run) => face(run.font_id, run.font_size),
                    PositionedElement::MultilingualText(run) => face(run.font_id, run.font_size),
                    _ => {}
                })
            }
            LineItem::Figure { item, .. } => collect(item, fm, central, failed),
            _ => {}
        }
    }
    let mut central_metric = None;
    let mut failed = false;
    for item in &line.items {
        collect(item, fm, &mut central_metric, &mut failed);
    }
    if failed {
        return None;
    }
    let central_metric = central_metric?;
    Some(0.8 * height + if central { central_metric } else { 0.0 })
}
