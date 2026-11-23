//! Table rows: split across pages, painted, repeated as headers, and their anchored content.

use super::*;

/// Split plain flowed cells at paragraph or guarded line boundaries. Rows with
/// merges, exact-height clipping, rotations, or anchored content retain the
/// existing whole-row path until those cases have their own safe break model.
/// A cell's vertical alignment does not move the break: each part of the cell
/// is aligned in its own part of the row.
pub(super) fn split_simple_table_row(
    row: &TableRow,
    semantics: Option<&RowSemantics>,
    space: f64,
) -> Option<(
    TableRow,
    TableRow,
    Option<RowSemantics>,
    Option<RowSemantics>,
)> {
    if row.height <= space || row.cant_split || row.is_header || row.cells.is_empty() {
        return None;
    }
    let mut first = row.clone();
    let mut rest = row.clone();
    let mut first_semantics = semantics.cloned();
    let mut rest_semantics = semantics.cloned();
    let mut first_height: f64 = 0.0;
    let mut rest_height: f64 = 0.0;
    let mut placed_any = false;
    let mut remains_any = false;
    for (cell_index, cell) in row.cells.iter().enumerate() {
        if cell.is_vmerge_continue
            || cell.starts_vmerge
            || cell.merge_with_below
            || cell.clip_content
            || cell.rotation.is_some()
        {
            return None;
        }
        let mut used = cell.margin_top + cell.border_band_top;
        let mut count = 0;
        let mut split_lines = None;
        for block in &cell.blocks {
            let CellBlock::Paragraph(paragraph) = block else {
                return None;
            };
            if !paragraph.anchored.is_empty() || paragraph.keep_next || paragraph.page_break_before
            {
                return None;
            }
            if used + paragraph.total_height() > space {
                if !paragraph.keep_lines && paragraph.lines.len() >= 4 {
                    let mut line_height =
                        used + paragraph.space_before + paragraph.content_offset_top;
                    let mut lines = 0;
                    for line in &paragraph.lines {
                        if line_height + line.height > space {
                            break;
                        }
                        line_height += line.height;
                        lines += 1;
                    }
                    let minimum = if paragraph.widow_control { 2 } else { 1 };
                    if lines >= minimum && paragraph.lines.len() - lines >= minimum {
                        used = line_height;
                        split_lines = Some(lines);
                    }
                }
                break;
            }
            used += paragraph.total_height();
            count += 1;
        }
        let has_more = count < cell.blocks.len();
        placed_any |= count > 0 || split_lines.is_some();
        remains_any |= has_more;
        if !has_more {
            used += cell.margin_bottom;
        }
        first_height = first_height.max(used);
        let first_cell = &mut first.cells[cell_index];
        let rest_cell = &mut rest.cells[cell_index];
        first_cell
            .blocks
            .truncate(count + usize::from(split_lines.is_some()));
        rest_cell.blocks.drain(..count);
        if let Some(lines) = split_lines {
            let CellBlock::Paragraph(first_paragraph) = &mut first_cell.blocks[count] else {
                unreachable!("checked paragraph blocks above")
            };
            first_paragraph.lines.truncate(lines);
            if let Some(borders) = &mut first_paragraph.borders {
                borders.bottom = None;
            }
            first_paragraph.space_after = 0.0;
            let CellBlock::Paragraph(rest_paragraph) = &mut rest_cell.blocks[0] else {
                unreachable!("checked paragraph blocks above")
            };
            rest_paragraph.lines.drain(..lines);
            if let Some(borders) = &mut rest_paragraph.borders {
                borders.top = None;
            }
            rest_paragraph.space_before = 0.0;
            rest_paragraph.content_offset_top = 0.0;
        }
        first_cell.margin_bottom = if has_more { 0.0 } else { cell.margin_bottom };
        first_cell.border_band_bottom = 0.0;
        rest_cell.margin_top = 0.0;
        rest_cell.border_band_top = 0.0;
        if !has_more {
            rest_cell.margin_bottom = 0.0;
        }
        let remaining_content = rest_cell
            .blocks
            .iter()
            .map(CellBlock::total_height)
            .sum::<f64>();
        rest_height = rest_height
            .max(remaining_content + rest_cell.margin_bottom + rest_cell.border_band_bottom);
        if let Some(lines) = split_lines {
            let CellBlock::Paragraph(original) = &cell.blocks[count] else {
                return None;
            };
            if let Some(CellBlockSemantics::Paragraph(companion)) = first_semantics
                .as_mut()
                .and_then(|row| row.cells.get_mut(cell_index))
                .and_then(|cell| cell.blocks.get_mut(count))
            {
                companion.paint = Arc::new(
                    companion
                        .paint
                        .for_line_range(&original.lines, 0..lines)
                        .ok()?,
                );
            }
            if let Some(CellBlockSemantics::Paragraph(companion)) = rest_semantics
                .as_mut()
                .and_then(|row| row.cells.get_mut(cell_index))
                .and_then(|cell| cell.blocks.get_mut(count))
            {
                companion.paint = Arc::new(
                    companion
                        .paint
                        .for_line_range(&original.lines, lines..original.lines.len())
                        .ok()?,
                );
            }
        }
        if let Some(row_semantics) = &mut first_semantics {
            row_semantics.cells[cell_index]
                .blocks
                .truncate(count + usize::from(split_lines.is_some()));
        }
        if let Some(row_semantics) = &mut rest_semantics {
            row_semantics.cells[cell_index].blocks.drain(..count);
        }
    }
    if !placed_any || !remains_any || first_height > space {
        return None;
    }
    first.height = first_height;
    rest.height = rest_height.max(row.height - first_height);
    rest.min_height = 0.0;
    for cell in &mut first.cells {
        cell.height = first.height;
        cell.merged_height = first.height;
    }
    for cell in &mut rest.cells {
        cell.height = rest.height;
        cell.merged_height = rest.height;
    }
    Some((first, rest, first_semantics, rest_semantics))
}

pub(super) fn paint_flowed_table_row(
    pager: &mut Pager<'_>,
    table: &TableView<'_>,
    row: &TableRow,
    semantics: Option<&RowSemantics>,
    body_index: Option<usize>,
    draw_top_border: bool,
    draw_bottom_border: bool,
) {
    let table_x = pager.geometry.margin_left + table.table_indent;
    let row_y = pager.geometry.margin_top + pager.cursor_y;
    if let Some(body_index) = body_index {
        pager.record_body_fragment(body_index, table_x, row_y, table.table_width, row.height);
    }
    pager.accept_layers(render_table_row(
        row,
        semantics,
        &table.col_widths,
        table_x,
        row_y,
        &pager.geometry,
        &pager.furniture_geometry,
        pager.page_number,
        table.borders.as_ref(),
        table.bidi_visual,
        pager.media,
        draw_top_border,
        draw_bottom_border,
    ));
    pager.cursor_y += row.height;
    // A row is ink like a line: notes beneath the text and endnotes that
    // follow the body start below it, and vertical alignment measures it.
    pager.ink_bottom = pager.cursor_y;
    pager.previous_space_after = 0.0;
    pager.mark_content();
}

pub(super) fn repeat_table_headers(
    pager: &mut Pager<'_>,
    table: &TableView<'_>,
    before_row: usize,
    body_index: Option<usize>,
) {
    for &header_index in &table.header_row_indices {
        if header_index < before_row {
            let row = &table.rows[header_index];
            let semantics = table
                .semantics
                .and_then(|semantics| semantics.rows.get(header_index));
            let scoped = semantics
                .map(|semantics| semantics.with_repetition_scope(pager.next_repetition_scope))
                .transpose();
            let scoped = match scoped {
                Ok(value) => value.flatten(),
                Err(error) => {
                    pager
                        .pending_furniture_failure
                        .get_or_insert(error.to_string());
                    return;
                }
            };
            if scoped.is_some() {
                let Some(next) = pager.next_repetition_scope.checked_add(1) else {
                    pager
                        .pending_furniture_failure
                        .get_or_insert("repeated field paint scope overflow".into());
                    return;
                };
                pager.next_repetition_scope = next;
            }
            paint_flowed_table_row(
                pager,
                table,
                row,
                scoped.as_ref().or(semantics),
                body_index,
                true,
                true,
            );
        }
    }
}

/// Keep a related-story table's row geometry and cell clipping in its
/// paragraph-based furniture flow. Each row is one indivisible flow line.
pub(crate) fn story_table_rows(
    table: &crate::table::TableBlock,
    semantics: &crate::block::TableSemantics,
    geometry: &PageGeometry,
    media: &HashMap<MediaId, ImageData>,
) -> oxoffice_layout::Result<Vec<(ParagraphBlock, WordParagraphPaint)>> {
    let local_geometry = PageGeometry {
        margin_top: 0.0,
        margin_left: 0.0,
        margin_right: 0.0,
        margin_bottom: 0.0,
        ..geometry.without_columns()
    };
    table
        .rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let painted = render_table_row(
                row,
                semantics.rows.get(index),
                &table.col_widths,
                table.table_indent,
                0.0,
                &local_geometry,
                &local_geometry,
                1,
                table.borders.as_ref(),
                table.bidi_visual,
                media,
                index == 0,
                index + 1 == table.rows.len(),
            )?;
            let behind_end = painted.behind.len();
            let flow_end = behind_end
                .checked_add(painted.regular.len())
                .ok_or_else(|| {
                    oxoffice_layout::LayoutError::Layout("Word row paint length overflow".into())
                })?;
            let (children, policies) = painted.flatten()?.into_parts()?;
            let children_len = children.len();
            let line = LayoutLine {
                items: vec![LineItem::Group {
                    width: table.table_width,
                    height: row.height,
                    baseline: Some(row.height),
                    math: false,
                    group: GroupElement {
                        transform: Transform::IDENTITY,
                        clip: None,
                        opacity: 1.0,
                        effects: Vec::new(),
                        children,
                    },
                }],
                width: table.table_width,
                ascent: row.height,
                descent: 0.0,
                line_gap: 0.0,
                height: row.height,
                indent_left: 0.0,
                available_width: table.table_width,
                is_last: true,
                forced_break_after: None,
            };
            let paragraph = crate::block::build_paragraph_block(
                vec![line],
                0.0,
                0.0,
                None,
                None,
                0.0,
                0.0,
                None,
                false,
                true,
                false,
                false,
            );
            Ok((
                paragraph,
                WordParagraphPaint {
                    rows: vec![WordRowPaint {
                        line: 0,
                        item: 0,
                        behind_end,
                        flow_end,
                        children_len,
                        policies,
                    }],
                    ..Default::default()
                },
            ))
        })
        .collect()
}

/// Render a table row.
pub(super) fn render_table_row(
    row: &crate::table::TableRow,
    row_semantics: Option<&crate::block::RowSemantics>,
    _col_widths: &[f64],
    table_x: f64,
    row_y: f64,
    geometry: &PageGeometry,
    written_geometry: &PageGeometry,
    page_number: usize,
    table_borders: Option<&oxoffice_docx_oxml::table::CT_TblBorders>,
    bidi: bool,
    media: &HashMap<MediaId, ImageData>,
    draw_top_border: bool,
    draw_bottom_border: bool,
) -> oxoffice_layout::Result<PaintLayers> {
    if let Some(semantics) = row_semantics
        && (semantics.cells.len() != row.cells.len()
            || row
                .cells
                .iter()
                .zip(&semantics.cells)
                .any(|(cell, semantics)| {
                    cell.blocks.len() != semantics.blocks.len()
                        || cell
                            .blocks
                            .iter()
                            .zip(&semantics.blocks)
                            .any(|(block, semantics)| {
                                !matches!(
                                    (block, semantics),
                                    (
                                        crate::table::CellBlock::Paragraph(_),
                                        CellBlockSemantics::Paragraph(_)
                                    ) | (
                                        crate::table::CellBlock::Table(_),
                                        CellBlockSemantics::Table(_)
                                    )
                                )
                            })
                }))
    {
        return Err(oxoffice_layout::LayoutError::Layout(
            "Word table paint companion mismatch".into(),
        ));
    }
    let mut elements = Vec::new();
    let mut policies = PaintPolicies::default();
    let mut behind_elements = Vec::new();
    let mut behind_policies = PaintPolicies::default();
    let mut front_elements = Vec::new();
    let mut front_policies = PaintPolicies::default();
    let mut cell_x = table_x + row.offset_left;
    let num_cells = row.cells.len();

    for visual_idx in 0..num_cells {
        // A bidirectional row places its logically last cell first. The row's
        // own vector, the retained semantics and the structure tree stay in
        // reading order, and a left-to-right row keeps the index and the
        // arithmetic it always had.
        let cell_idx = if bidi {
            num_cells - 1 - visual_idx
        } else {
            visual_idx
        };
        let cell = &row.cells[cell_idx];
        let cell_semantics = row_semantics.and_then(|row| row.cells.get(cell_idx));
        if cell.is_vmerge_continue {
            cell_x += cell.width;
            continue;
        }
        let paint_height = cell.merged_height;
        // Render cell shading
        if let Some(ref shading) = cell.shading {
            elements.push(PositionedElement::FilledRect {
                rect: Rect {
                    x: cell_x,
                    y: row_y,
                    width: cell.width,
                    height: paint_height,
                },
                color: *shading,
            });
        }

        // Render cell borders
        render_cell_borders(
            cell_x,
            row_y,
            cell.width,
            paint_height,
            &cell.borders,
            table_borders,
            visual_idx,
            num_cells,
            cell.is_first_row,
            cell.is_last_row,
            draw_top_border,
            draw_bottom_border,
            &mut elements,
        );

        let content_element_start = elements.len();
        let behind_element_start = behind_elements.len();
        let front_element_start = front_elements.len();
        // A change bar is drawn at the page margin and an anchored drawing
        // resolves against a page frame, so both are page furniture. They are
        // collected apart from the cell's own content and appended after the
        // rotation below, or a rotated cell would carry them off the page.
        let mut cell_furniture = Vec::new();
        let mut behind_furniture = Vec::new();
        let mut behind_furniture_policies = PaintPolicies::default();
        let mut front_furniture = Vec::new();
        let mut front_furniture_policies = PaintPolicies::default();
        let mut change_bar_drawn = false;
        // The cell's own upright band, built once for a rotated cell.
        let upright_band = cell.rotation.map(|_| PageGeometry {
            margin_left: cell_x + cell.margin_left,
            margin_right: 0.0,
            page_width: cell_x + cell.width - cell.margin_right,
            ..geometry.without_columns()
        });

        let content_height = cell
            .blocks
            .iter()
            .map(crate::table::CellBlock::total_height)
            .sum::<f64>();
        // The content sits between the horizontal border bands the row
        // height includes. A rotated cell is laid out in a same-centre
        // transposed box and the result is rotated back over the cell. A
        // horizontal cell's box is the cell itself less those bands.
        let band_y = row_y + cell.border_band_top;
        let band_height = (paint_height - cell.border_band_top - cell.border_band_bottom).max(0.0);
        let (box_x, box_y, box_width, box_height) = match cell.rotation {
            Some(_) => crate::table::transposed_box(cell_x, band_y, cell.width, band_height),
            None => (cell_x, band_y, cell.width, band_height),
        };
        let v_offset = match cell.v_align {
            Some(oxoffice_docx_oxml::table::ST_VerticalJc::Center) => {
                ((box_height - cell.margin_top - content_height) / 2.0).max(0.0)
            }
            Some(oxoffice_docx_oxml::table::ST_VerticalJc::Bottom) => {
                (box_height - cell.margin_top - content_height).max(0.0)
            }
            _ => 0.0,
        };
        let mut content_y = box_y - geometry.margin_top + cell.margin_top + v_offset;
        for (block_index, block) in cell.blocks.iter().enumerate() {
            let block_semantics = cell_semantics.and_then(|cell| cell.blocks.get(block_index));
            match block {
                crate::table::CellBlock::Paragraph(paragraph) => {
                    // The space before sits above the lines, as in the body.
                    content_y += paragraph.space_before;
                    let semantics = match block_semantics {
                        Some(CellBlockSemantics::Paragraph(semantics)) => Some(semantics),
                        _ => None,
                    };
                    let cell_geometry = PageGeometry {
                        margin_left: box_x + cell.margin_left,
                        margin_right: 0.0,
                        page_width: box_x + box_width - cell.margin_right,
                        ..geometry.without_columns()
                    };
                    let painted = render_paragraph_lines(
                        &paragraph.lines,
                        ParagraphView {
                            line_start: 0,
                            paint: None,
                            baseline_policy: None,
                            line_gaps: &[],
                            border_group_before: semantics
                                .is_some_and(|value| value.border_group_before),
                            block: paragraph,
                            semantics,
                            reflow_direction: semantics
                                .map_or(oxoffice_layout::TextDirection::Auto, |semantics| {
                                    semantics.reflow_direction
                                }),
                            reflow_allowed: true,
                        },
                        &cell_geometry,
                        content_y,
                        (
                            true,
                            true,
                            if block_index > 0
                                && semantics.is_some_and(|value| value.border_group_before)
                            {
                                match &cell.blocks[block_index - 1] {
                                    crate::table::CellBlock::Paragraph(previous) => {
                                        previous.space_after + paragraph.space_before
                                    }
                                    crate::table::CellBlock::Table(_) => 0.0,
                                }
                            } else {
                                0.0
                            },
                        ),
                        media,
                    )?;
                    policies.append(&mut elements, painted.regular)?;
                    behind_policies.append(&mut behind_elements, painted.behind)?;
                    front_policies.append(&mut front_elements, painted.front)?;
                    // A horizontal cell keeps the interleaved painted order
                    // and the frame it always had. A rotated cell's change bar
                    // and anchored drawings are page furniture that must not
                    // turn with the text, so they are collected apart and
                    // placed against the cell's own upright row band rather
                    // than against the transposed box the text was laid out
                    // in. That band is the only page-space statement of where
                    // the cell is.
                    let rotated = cell.rotation.is_some();
                    // The upright band is built once per rotated cell rather
                    // than per paragraph, and a horizontal cell borrows the
                    // geometry it already built instead of cloning it.
                    let (furniture_y, furniture_height) = if rotated {
                        (
                            band_y - geometry.margin_top + cell.margin_top,
                            (band_height - cell.margin_top - cell.margin_bottom).max(0.0),
                        )
                    } else {
                        (content_y, paragraph.content_height())
                    };
                    let furniture_geometry = upright_band.as_ref().unwrap_or(&cell_geometry);
                    let anchored = place_cell_anchored(
                        &paragraph.anchored,
                        semantics.map(|semantics| semantics.paint.as_ref()),
                        written_geometry,
                        furniture_geometry,
                        furniture_y,
                        paragraph.indent_left,
                        media,
                    )?;
                    if rotated {
                        front_furniture_policies.append(&mut front_furniture, anchored.front)?;
                        behind_furniture_policies.append(&mut behind_furniture, anchored.behind)?;
                    } else {
                        front_policies.append(&mut front_elements, anchored.front)?;
                        behind_policies.append(&mut behind_elements, anchored.behind)?;
                    }
                    // A rotated cell's bar spans the whole row band, so it is
                    // drawn once however many revised paragraphs the cell
                    // holds. The flag records that a bar was drawn, not that a
                    // paragraph was seen, or an unrevised first paragraph
                    // would suppress the bar of a revised second one.
                    if !rotated || !change_bar_drawn {
                        render_change_bar(
                            paragraph,
                            furniture_y,
                            furniture_height,
                            geometry,
                            page_number,
                            if rotated {
                                &mut cell_furniture
                            } else {
                                &mut elements
                            },
                        );
                        change_bar_drawn |= rotated && paragraph.change_bar.is_some();
                    }
                    content_y += paragraph.content_height() + paragraph.space_after;
                }
                crate::table::CellBlock::Table(table) => {
                    let semantics = match block_semantics {
                        Some(CellBlockSemantics::Table(semantics)) => Some(semantics),
                        _ => None,
                    };
                    let nested_x = box_x + cell.margin_left + table.table_indent;
                    let mut nested_y = geometry.margin_top + content_y;
                    for (nested_row_index, nested_row) in table.rows.iter().enumerate() {
                        let nested = render_table_row(
                            nested_row,
                            semantics.and_then(|semantics| semantics.rows.get(nested_row_index)),
                            &table.col_widths,
                            nested_x,
                            nested_y,
                            geometry,
                            written_geometry,
                            page_number,
                            table.borders.as_ref(),
                            table.bidi_visual,
                            media,
                            true,
                            true,
                        )?;
                        policies.append(&mut elements, nested.regular)?;
                        behind_policies.append(&mut behind_elements, nested.behind)?;
                        front_policies.append(&mut front_elements, nested.front)?;
                        nested_y += nested_row.height;
                    }
                    content_y += table.total_height();
                }
            }
        }
        // Rotate the cell's painted content about the box centre, which is
        // the cell centre, so the transposed layout lands back in the cell.
        if let Some(degrees) = cell.rotation {
            let transform = Transform::rotate_about(
                degrees,
                cell_x + cell.width / 2.0,
                band_y + band_height / 2.0,
            );
            wrap_paint_suffix(
                &mut elements,
                &mut policies,
                content_element_start,
                transform,
                None,
                false,
            )?;
            wrap_paint_suffix(
                &mut behind_elements,
                &mut behind_policies,
                behind_element_start,
                transform,
                None,
                false,
            )?;
            wrap_paint_suffix(
                &mut front_elements,
                &mut front_policies,
                front_element_start,
                transform,
                None,
                false,
            )?;
        }
        elements.append(&mut cell_furniture);
        behind_policies.append(
            &mut behind_elements,
            PaintedElements::from_parts(behind_furniture, behind_furniture_policies)?,
        )?;
        front_policies.append(
            &mut front_elements,
            PaintedElements::from_parts(front_furniture, front_furniture_policies)?,
        )?;
        if cell.clip_content {
            let clip = Some(Path::rect(Rect {
                x: cell_x,
                y: row_y,
                width: cell.width,
                height: paint_height,
            }));
            wrap_paint_suffix(
                &mut elements,
                &mut policies,
                content_element_start,
                Transform::IDENTITY,
                clip.clone(),
                true,
            )?;
            wrap_paint_suffix(
                &mut behind_elements,
                &mut behind_policies,
                behind_element_start,
                Transform::IDENTITY,
                clip.clone(),
                false,
            )?;
            wrap_paint_suffix(
                &mut front_elements,
                &mut front_policies,
                front_element_start,
                Transform::IDENTITY,
                clip,
                false,
            )?;
        }
        cell_x += cell.width;
    }
    Ok(PaintLayers {
        behind: PaintedElements::from_parts(behind_elements, behind_policies)?,
        regular: PaintedElements::from_parts(elements, policies)?,
        front: PaintedElements::from_parts(front_elements, front_policies)?,
    })
}

pub(super) fn wrap_paint_suffix(
    elements: &mut Vec<PositionedElement>,
    policies: &mut PaintPolicies,
    start: usize,
    transform: Transform,
    clip: Option<Path>,
    keep_empty: bool,
) -> oxoffice_layout::Result<()> {
    policies.validate(elements)?;
    let mut painted =
        PaintedElements::from_parts(std::mem::take(elements), std::mem::take(policies))?;
    let children = painted.split_off(start)?;
    if children.len() > 0 || keep_empty {
        painted.append(&mut children.wrap(PositionedElement::Group(GroupElement {
            transform,
            clip,
            opacity: 1.0,
            effects: Vec::new(),
            children: Vec::new(),
        }))?)?;
    }
    (*elements, *policies) = painted.into_parts()?;
    Ok(())
}

pub(super) fn place_cell_anchored(
    anchors: &[AnchoredDrawing],
    paint: Option<&WordParagraphPaint>,
    page_geometry: &PageGeometry,
    cell_geometry: &PageGeometry,
    paragraph_top: f64,
    paragraph_indent: f64,
    media: &HashMap<MediaId, ImageData>,
) -> oxoffice_layout::Result<PaintLayers> {
    if paint.is_some_and(|paint| paint.anchors.len() != anchors.len()) {
        return Err(oxoffice_layout::LayoutError::Layout(
            "Word anchor producer count mismatch".into(),
        ));
    }
    let mut layers = PaintLayers::default();
    for (index, anchor) in anchors.iter().enumerate() {
        let owner = paint.map_or(AnchorPaintOwner::Neutral, |paint| {
            AnchorPaintOwner::Word(&paint.anchors[index])
        });
        let rect = anchored_rect(
            anchor,
            page_geometry,
            cell_geometry,
            paragraph_top,
            paragraph_indent,
        );
        let mut produced = anchored_elements(anchor, owner, rect, page_geometry, media)?;
        if anchor.behind_doc {
            layers.behind.append(&mut produced)?;
        } else {
            layers.front.append(&mut produced)?;
        }
    }
    Ok(layers)
}
