//! Borders of cells, paragraphs and pages: their edges and dash patterns.

use super::*;

/// Render borders for a table cell.
pub(super) fn render_cell_borders(
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    cell_borders: &Option<oxoffice_docx_oxml::table::CT_TblBorders>,
    table_borders: Option<&oxoffice_docx_oxml::table::CT_TblBorders>,
    cell_idx: usize,
    num_cells: usize,
    is_first_row: bool,
    is_last_row: bool,
    draw_top_border: bool,
    draw_bottom_border: bool,
    elements: &mut Vec<PositionedElement>,
) {
    // Determine effective border for each edge (cell overrides table)
    let get_edge = |cell_edge: Option<&oxoffice_docx_oxml::borders::CT_BorderEdge>,
                    table_edge: Option<&oxoffice_docx_oxml::borders::CT_BorderEdge>|
     -> Option<BorderEdge> {
        let edge = crate::table::resolved_cell_edge(cell_edge, table_edge)?;
        let thickness = edge.sz.unwrap_or(4) as f64 / 8.0; // sz is in 1/8 pt
        let color = edge
            .color
            .as_ref()
            .filter(|c| c.as_str() != "auto")
            .map(|c| Color::from_hex(c))
            .unwrap_or(Color::BLACK);
        let dash = border_dash_pattern(edge.val, thickness);
        Some((thickness, color, dash))
    };

    // Top border: use table top for first row, table insideH otherwise
    let table_top = table_borders.and_then(|b| {
        if is_first_row {
            b.top.as_ref()
        } else {
            b.inside_h.as_ref()
        }
    });
    // A horizontal border fills the band below the row boundary it sits on,
    // which the row heights reserve, rather than straddling the boundary.
    let cell_top = cell_borders.as_ref().and_then(|b| b.top.as_ref());
    if draw_top_border && let Some((thickness, color, dash_pattern)) = get_edge(cell_top, table_top)
    {
        let line_y = y + thickness / 2.0;
        elements.push(PositionedElement::Line {
            start: Point { x, y: line_y },
            end: Point {
                x: x + w,
                y: line_y,
            },
            width: thickness,
            color,
            dash_pattern,
        });
    }

    // Bottom border: use table bottom for last row, table insideH otherwise
    let table_bottom = table_borders.and_then(|b| {
        if is_last_row {
            b.bottom.as_ref()
        } else {
            b.inside_h.as_ref()
        }
    });
    let cell_bottom = cell_borders.as_ref().and_then(|b| b.bottom.as_ref());
    if draw_bottom_border
        && let Some((thickness, color, dash_pattern)) = get_edge(cell_bottom, table_bottom)
    {
        // The table's bottom band is the last row's own, so its line sits
        // inside the row. Any other bottom edge is in the next row's band.
        let line_y = if is_last_row {
            y + h - thickness / 2.0
        } else {
            y + h + thickness / 2.0
        };
        elements.push(PositionedElement::Line {
            start: Point { x, y: line_y },
            end: Point {
                x: x + w,
                y: line_y,
            },
            width: thickness,
            color,
            dash_pattern,
        });
    }

    // Left border: use table left for first cell, table insideV otherwise
    let table_left = table_borders.and_then(|b| {
        if cell_idx == 0 {
            b.left.as_ref()
        } else {
            b.inside_v.as_ref()
        }
    });
    let cell_left = cell_borders.as_ref().and_then(|b| b.left.as_ref());
    if let Some((thickness, color, dash_pattern)) = get_edge(cell_left, table_left) {
        elements.push(PositionedElement::Line {
            start: Point { x, y },
            end: Point { x, y: y + h },
            width: thickness,
            color,
            dash_pattern,
        });
    }

    // Right border: use table right for last cell, table insideV otherwise
    let table_right = table_borders.and_then(|b| {
        if cell_idx == num_cells - 1 {
            b.right.as_ref()
        } else {
            b.inside_v.as_ref()
        }
    });
    let cell_right = cell_borders.as_ref().and_then(|b| b.right.as_ref());
    if let Some((thickness, color, dash_pattern)) = get_edge(cell_right, table_right) {
        elements.push(PositionedElement::Line {
            start: Point { x: x + w, y },
            end: Point { x: x + w, y: y + h },
            width: thickness,
            color,
            dash_pattern,
        });
    }
}

/// Paragraph frames measure space to the inside edge and join corresponding
/// single or double bands. Page borders keep their separate existing geometry.
pub(super) fn render_paragraph_border_edges(
    borders: &CT_PBdr,
    rect: Rect,
    fragment: (bool, bool, f64),
    elements: &mut Vec<PositionedElement>,
) {
    let top = fragment.0.then_some(borders.top.as_ref()).flatten();
    let bottom = fragment.1.then_some(borders.bottom.as_ref()).flatten();
    let left = borders.left.as_ref();
    let right = borders.right.as_ref();
    let band = crate::block::paragraph_border_width;
    let padding = crate::block::paragraph_border_padding;
    let outer_left = rect.x - 1.5 - padding(left);
    let outer_right = rect.x + rect.width + 1.5 + padding(right);
    let outer_top = rect.y - padding(top) - if top.is_none() { fragment.2 } else { 0.0 };
    let outer_bottom = rect.y + rect.height + padding(bottom);
    let inner_inset = |edge: Option<&CT_BorderEdge>, layer: usize| {
        if layer == 1 && edge.is_some_and(|edge| edge.val == ST_Border::Double) {
            band(edge) * 2.0 / 3.0
        } else {
            0.0
        }
    };
    // Horizontal bands own the corner paint, as in the independent Word frame.
    for (edge, horizontal, trailing) in [
        (left, false, false),
        (right, false, true),
        (top, true, false),
        (bottom, true, true),
    ] {
        let full_width = band(edge);
        let Some(edge) = edge.filter(|_| full_width > 0.0) else {
            continue;
        };
        let double = edge.val == ST_Border::Double;
        let width = if double { full_width / 3.0 } else { full_width };
        let color = edge
            .color
            .as_ref()
            .filter(|color| color.as_str() != "auto")
            .map(|color| Color::from_hex(color))
            .unwrap_or(Color::BLACK);
        let dash_pattern = border_dash_pattern(edge.val, width);
        for layer in 0..if double { 2 } else { 1 } {
            let offset = width / 2.0 + layer as f64 * width * 2.0;
            let (start, end) = if horizontal {
                let y = if trailing {
                    outer_bottom - offset
                } else {
                    outer_top + offset
                };
                (
                    Point {
                        x: outer_left + inner_inset(left, layer),
                        y,
                    },
                    Point {
                        x: outer_right - inner_inset(right, layer),
                        y,
                    },
                )
            } else {
                let x = if trailing {
                    outer_right - offset
                } else {
                    outer_left + offset
                };
                (
                    Point {
                        x,
                        y: outer_top + inner_inset(top, layer),
                    },
                    Point {
                        x,
                        y: outer_bottom - inner_inset(bottom, layer),
                    },
                )
            };
            elements.push(PositionedElement::Line {
                start,
                end,
                width,
                color,
                dash_pattern,
            });
        }
    }
}

/// Render paragraph border edges as positioned lines.
pub(super) fn render_border_edges(
    borders: &oxoffice_docx_oxml::borders::CT_PBdr,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    elements: &mut Vec<PositionedElement>,
) {
    let render_edge = |edge: &oxoffice_docx_oxml::borders::CT_BorderEdge,
                       start: Point,
                       end: Point,
                       elements: &mut Vec<PositionedElement>| {
        // A picture border has no line to draw, so it keeps its token and
        // draws nothing, as it did when it read as `none`.
        if edge.val.is_none() || matches!(edge.val, ST_Border::Art(_)) {
            return;
        }
        let thickness = edge.sz.unwrap_or(4) as f64 / 8.0; // sz is in eighths of a point
        let color = edge
            .color
            .as_ref()
            .filter(|c| c.as_str() != "auto")
            .map(|c| Color::from_hex(c))
            .unwrap_or(Color::BLACK);
        let dash_pattern = border_dash_pattern(edge.val, thickness);

        if edge.val == ST_Border::Double {
            // Double border: emit two parallel lines
            let gap = thickness * 2.0;
            let dx = end.x - start.x;
            let dy = end.y - start.y;
            let len = (dx * dx + dy * dy).sqrt();
            let (nx, ny) = if len > 0.0 {
                (-dy / len, dx / len)
            } else {
                (0.0, 1.0)
            };
            let offset = gap / 2.0;
            elements.push(PositionedElement::Line {
                start: Point {
                    x: start.x + nx * offset,
                    y: start.y + ny * offset,
                },
                end: Point {
                    x: end.x + nx * offset,
                    y: end.y + ny * offset,
                },
                width: thickness,
                color,
                dash_pattern: None,
            });
            elements.push(PositionedElement::Line {
                start: Point {
                    x: start.x - nx * offset,
                    y: start.y - ny * offset,
                },
                end: Point {
                    x: end.x - nx * offset,
                    y: end.y - ny * offset,
                },
                width: thickness,
                color,
                dash_pattern: None,
            });
        } else {
            elements.push(PositionedElement::Line {
                start,
                end,
                width: thickness,
                color,
                dash_pattern,
            });
        }
    };

    if let Some(ref edge) = borders.top {
        let space = edge.space.unwrap_or(0) as f64;
        render_edge(
            edge,
            Point { x, y: y - space },
            Point {
                x: x + w,
                y: y - space,
            },
            elements,
        );
    }
    if let Some(ref edge) = borders.bottom {
        let space = edge.space.unwrap_or(0) as f64;
        render_edge(
            edge,
            Point {
                x,
                y: y + h + space,
            },
            Point {
                x: x + w,
                y: y + h + space,
            },
            elements,
        );
    }
    if let Some(ref edge) = borders.left {
        let space = edge.space.unwrap_or(0) as f64;
        render_edge(
            edge,
            Point { x: x - space, y },
            Point {
                x: x - space,
                y: y + h,
            },
            elements,
        );
    }
    if let Some(ref edge) = borders.right {
        let space = edge.space.unwrap_or(0) as f64;
        render_edge(
            edge,
            Point {
                x: x + w + space,
                y,
            },
            Point {
                x: x + w + space,
                y: y + h,
            },
            elements,
        );
    }
}

/// Map a border style to a dash pattern (dash_on, dash_off) in points.
/// Returns None for solid lines (Single, Thick, Double, etc.).
///
/// A style with no pattern of its own draws as the nearest one here:
/// `dashSmallGap` as `dashed` and `dashDotStroked` as `dotDash`.
fn border_dash_pattern(style: ST_Border, thickness: f64) -> Option<(f64, f64)> {
    match style {
        ST_Border::Dashed | ST_Border::DashSmallGap => Some((3.0 * thickness, 2.0 * thickness)),
        ST_Border::Dotted => Some((thickness, thickness)),
        ST_Border::DotDash | ST_Border::DotDotDash | ST_Border::DashDotStroked => {
            Some((3.0 * thickness, thickness))
        }
        _ => None,
    }
}
