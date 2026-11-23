//! Frames and header and footer stories: their rectangles and their painted blocks.

use super::*;

pub(super) fn frame_rect(
    properties: &oxoffice_docx_oxml::CT_FramePr,
    blocks: &[ParagraphBlock],
    facts: &[HeaderFooterBlockSemantics],
    geometry: &PageGeometry,
    anchor_top: f64,
) -> Rect {
    let width = properties
        .w
        .map(|value| value.to_pt())
        .unwrap_or_else(|| {
            blocks
                .iter()
                .flat_map(|block| &block.lines)
                .map(|line| line.width)
                .fold(0.0, f64::max)
        })
        .max(0.0);
    let content_height = blocks
        .iter()
        .zip(facts)
        .map(|(block, facts)| facts.flow_gap + block.content_height())
        .sum::<f64>()
        + blocks.last().map_or(0.0, |block| block.space_after);
    let declared = properties.h.map_or(0.0, |value| value.to_pt());
    // MS-OE376 2.1.48(e-h): Word's defaults differ from ECMA here.
    // Imported attributes stay untouched. Zero height always means automatic.
    let height = if declared == 0.0 {
        content_height
    } else {
        match properties.h_rule.as_deref() {
            Some("exact") => declared,
            Some("atLeast") | None => declared.max(content_height),
            _ => content_height,
        }
    };
    let rel_h = match properties.h_anchor.as_deref() {
        Some("margin") => ST_RelativeFromH::Margin,
        None | Some("text" | "column") => ST_RelativeFromH::Column,
        _ => ST_RelativeFromH::Page,
    };
    let rel_v = match properties.v_anchor.as_deref() {
        None | Some("margin") => ST_RelativeFromV::Margin,
        Some("text") => ST_RelativeFromV::Paragraph,
        _ => ST_RelativeFromV::Page,
    };
    Rect {
        x: resolve_anchor_h(
            rel_h,
            properties.x.map_or(0.0, |value| value.to_pt()),
            properties.x_align.as_deref().and_then(AnchorAlignH::parse),
            width,
            geometry,
            0.0,
        ),
        y: resolve_anchor_v(
            rel_v,
            properties.y.map_or(0.0, |value| value.to_pt()),
            properties.y_align.as_deref().and_then(|value| match value {
                "top" => Some(AnchorAlignV::Top),
                "center" => Some(AnchorAlignV::Center),
                "bottom" => Some(AnchorAlignV::Bottom),
                "inside" => Some(AnchorAlignV::Inside),
                "outside" => Some(AnchorAlignV::Outside),
                _ => None,
            }),
            height,
            geometry,
            anchor_top - geometry.margin_top,
        ),
        width,
        height,
    }
}

#[allow(clippy::too_many_arguments)]
fn render_frame_blocks(
    blocks: &[ParagraphBlock],
    facts: &[HeaderFooterBlockSemantics],
    rect: Rect,
    geometry: &PageGeometry,
    page_number: usize,
    media: &HashMap<MediaId, ImageData>,
) -> oxoffice_layout::Result<PaintLayers> {
    let mut layers = PaintLayers::default();
    let frame = PageGeometry {
        margin_left: rect.x,
        margin_right: geometry.page_width - rect.x - rect.width,
        margin_top: rect.y,
        ..geometry.clone()
    };
    let mut children = PaintedElements::default();
    let mut y = 0.0;
    for (paragraph, facts) in blocks.iter().zip(facts) {
        y += facts.flow_gap;
        if facts.paint.anchors.len() != paragraph.anchored.len() {
            return Err(oxoffice_layout::LayoutError::Layout(
                "Word anchor producer count mismatch".into(),
            ));
        }
        for (index, anchor) in paragraph.anchored.iter().enumerate() {
            let mut positioned = anchored_rect(anchor, geometry, &frame, y, paragraph.indent_left);
            // Word uses the ordinary furniture frame as the origin for page
            // offsets inside that frame. Alignment keeps its existing basis.
            if anchor.rel_h == ST_RelativeFromH::Page && anchor.align_h.is_none() {
                positioned.x += rect.x;
            }
            if anchor.rel_v == ST_RelativeFromV::Page && anchor.align_v.is_none() {
                positioned.y += rect.y;
            }
            let mut produced = anchored_elements(
                anchor,
                AnchorPaintOwner::Word(&facts.paint.anchors[index]),
                positioned,
                geometry,
                media,
            )?;
            if anchor.behind_doc {
                layers.behind.append(&mut produced)?;
            } else {
                layers.front.append(&mut produced)?;
            }
        }
        let mut painted = render_paragraph_lines(
            &paragraph.lines,
            ParagraphView {
                line_start: 0,
                paint: Some(&facts.paint),
                baseline_policy: None,
                line_gaps: &[],
                border_group_before: false,
                block: paragraph,
                semantics: None,
                reflow_direction: facts.direction,
                reflow_allowed: true,
            },
            &frame,
            y,
            (true, true, facts.border_gap),
            media,
        )?;
        children.append(&mut painted.regular)?;
        layers.behind.append(&mut painted.behind)?;
        layers.front.append(&mut painted.front)?;
        let mut bars = Vec::new();
        render_change_bar(
            paragraph,
            rect.y + y - geometry.margin_top,
            paragraph.content_height(),
            geometry,
            page_number,
            &mut bars,
        );
        children.append(&mut PaintedElements::non_field(bars)?)?;
        y += paragraph.content_height();
    }
    layers
        .regular
        .append(&mut children.wrap(PositionedElement::Group(GroupElement {
            transform: Transform::IDENTITY,
            clip: None,
            opacity: 1.0,
            effects: Vec::new(),
            children: Vec::new(),
        }))?)?;
    Ok(layers)
}

/// Price and paint only real furniture flow. Positioned owners retain their page context.
pub(super) fn render_hf_blocks(
    prepared: &PreparedFurniture,
    directions: Option<&[HeaderFooterBlockSemantics]>,
    geometry: &PageGeometry,
    page_number: usize,
    media: &HashMap<MediaId, ImageData>,
) -> oxoffice_layout::Result<PaintLayers> {
    let mut layers = PaintLayers::default();
    if directions.is_some_and(|facts| facts.len() != prepared.blocks.len()) {
        return Err(oxoffice_layout::LayoutError::Layout(
            "Word furniture owner count mismatch".into(),
        ));
    }
    let blocks = &prepared.blocks;
    for (index, para) in blocks.iter().enumerate() {
        let facts = directions.and_then(|values| values.get(index));
        let y = prepared.starts[index];
        if let Some(facts) = facts {
            match &facts.placement {
                HeaderFooterPlacement::Frame { paragraphs, .. } => {
                    let end = index + paragraphs;
                    let frame_facts =
                        &directions.expect("frame facts accompany blocks")[index..end];
                    layers.append(&mut render_frame_blocks(
                        &blocks[index..end],
                        frame_facts,
                        prepared.placements[index].expect("prepared frame"),
                        geometry,
                        page_number,
                        media,
                    )?)?;
                    continue;
                }
                HeaderFooterPlacement::FrameContinuation => continue,
                HeaderFooterPlacement::TableRow {
                    table,
                    semantics,
                    row,
                } => {
                    let rect = prepared.placements[index].expect("prepared table row");
                    layers.append(&mut render_table_row(
                        &table.rows[*row],
                        semantics.rows.get(*row),
                        &table.col_widths,
                        rect.x,
                        rect.y,
                        geometry,
                        geometry,
                        page_number,
                        table.borders.as_ref(),
                        table.bidi_visual,
                        media,
                        *row == 0,
                        *row + 1 == table.rows.len(),
                    )?)?;
                    continue;
                }
                HeaderFooterPlacement::Flow => {}
            }
        }
        let border_gap = facts.map_or(0.0, |facts| facts.border_gap);
        layers.append(&mut place_cell_anchored(
            &para.anchored,
            prepared.paints.get(index).and_then(Option::as_deref),
            geometry,
            geometry,
            y - geometry.margin_top,
            para.indent_left,
            media,
        )?)?;
        layers.append(&mut render_paragraph_lines(
            &para.lines,
            ParagraphView {
                line_start: 0,
                paint: prepared.paints.get(index).and_then(Option::as_deref),
                baseline_policy: None,
                line_gaps: &prepared.before_lines[index],
                border_group_before: false,
                block: para,
                semantics: None,
                reflow_direction: facts.map_or(oxoffice_layout::TextDirection::Auto, |facts| {
                    facts.direction
                }),
                reflow_allowed: true,
            },
            geometry,
            y - geometry.margin_top,
            (true, true, border_gap),
            media,
        )?)?;
        let mut bars = Vec::new();
        render_change_bar(
            para,
            y - geometry.margin_top,
            prepared.block_height(index),
            geometry,
            page_number,
            &mut bars,
        );
        layers
            .regular
            .append(&mut PaintedElements::non_field(bars)?)?;
    }
    Ok(layers)
}
