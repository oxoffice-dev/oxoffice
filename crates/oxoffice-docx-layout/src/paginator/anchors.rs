//! Anchored drawings: their painted elements, the text of their shapes and their anchor frames.

use super::*;

#[derive(Clone, Copy)]
pub(super) enum AnchorPaintOwner<'a> {
    Word(&'a WordAnchorPaint),
    Neutral,
}

pub(super) fn anchored_elements(
    anchor: &AnchoredDrawing,
    owner: AnchorPaintOwner<'_>,
    rect: Rect,
    geometry: &PageGeometry,
    media: &HashMap<MediaId, ImageData>,
) -> oxoffice_layout::Result<PaintedElements> {
    let mut produced = PaintedElements::default();
    match (&anchor.content, owner) {
        (
            AnchoredContent::Image { media_id },
            AnchorPaintOwner::Neutral | AnchorPaintOwner::Word(WordAnchorPaint::Image),
        ) => {
            let image = media.get(media_id);
            produced.push_non_field(PositionedElement::Image {
                rect,
                data: image.map_or_else(Vec::new, |image| image.data.clone()),
                content_type: image.map_or_else(String::new, |image| image.content_type.clone()),
                media_id: *media_id,
            })?;
        }
        (
            AnchoredContent::Group(group),
            owner @ (AnchorPaintOwner::Neutral
            | AnchorPaintOwner::Word(WordAnchorPaint::Group { .. })),
        ) => {
            let mut positioned = group.clone();
            positioned.transform = positioned.transform.then(Transform {
                e: rect.x,
                f: rect.y,
                ..Transform::IDENTITY
            });
            let elements = vec![PositionedElement::Group(positioned)];
            produced = match owner {
                AnchorPaintOwner::Neutral => PaintedElements::neutral(elements)?,
                AnchorPaintOwner::Word(WordAnchorPaint::Group { policies }) => {
                    let PositionedElement::Group(mut shell) =
                        elements.into_iter().next().expect("one group")
                    else {
                        unreachable!()
                    };
                    let child = PaintedElements::from_parts(
                        std::mem::take(&mut shell.children),
                        policies.clone(),
                    )?;
                    child.wrap(PositionedElement::Group(shell))?
                }
                _ => unreachable!(),
            };
        }
        (
            AnchoredContent::Shape {
                preset,
                fill,
                text,
                fixed_height,
                insets,
            },
            owner @ (AnchorPaintOwner::Neutral
            | AnchorPaintOwner::Word(WordAnchorPaint::Shape { .. })),
        ) => {
            match (preset, fill) {
                (ShapePreset::Rect, Some(color)) => {
                    produced.push_non_field(PositionedElement::FilledRect {
                        rect,
                        color: *color,
                    })?
                }
                (ShapePreset::Line, Some(color)) => {
                    produced.push_non_field(PositionedElement::Line {
                        start: Point {
                            x: rect.x,
                            y: rect.y,
                        },
                        end: Point {
                            x: rect.x + anchor.width,
                            y: rect.y + anchor.height,
                        },
                        width: 1.0,
                        color: *color,
                        dash_pattern: None,
                    })?
                }
                _ => {}
            }
            let paragraphs = match owner {
                AnchorPaintOwner::Word(WordAnchorPaint::Shape { paragraphs }) => {
                    Some(paragraphs.as_slice())
                }
                _ => None,
            };
            let [left, top, right, bottom] = *insets;
            let text_rect = Rect {
                x: rect.x + left,
                y: rect.y + top,
                width: (rect.width - left - right).max(0.0),
                height: (rect.height - top - bottom).max(0.0),
            };
            produced.append(&mut render_shape_text(
                text,
                paragraphs,
                geometry,
                text_rect,
                fixed_height.then_some(text_rect.height),
                media,
            )?)?;
        }
        _ => {
            return Err(oxoffice_layout::LayoutError::Layout(
                "Word anchor paint variant mismatch".into(),
            ));
        }
    }
    if anchor.rotation_degrees != 0.0 {
        produced = produced.wrap(PositionedElement::Group(GroupElement {
            transform: Transform::rotate_about(
                anchor.rotation_degrees,
                rect.x + rect.width / 2.0,
                rect.y + rect.height / 2.0,
            ),
            clip: None,
            opacity: 1.0,
            effects: Vec::new(),
            children: Vec::new(),
        }))?;
    }
    let mut result = PaintedElements::default();
    while produced.len() > 0 {
        let child = produced.splice_at(&[], 0..1, &mut PaintedElements::default())?;
        result.append(&mut child.wrap(PositionedElement::MarkedContent {
            structure: anchor.structure_id,
            children: Vec::new(),
        })?)?;
    }
    Ok(result)
}

/// Paginate a single paragraph, handling splitting across pages.
/// Shift a positioned element by a fixed offset.
///
/// Paragraph rendering always lays out against the page margins, so a text box
/// is rendered at the margin first and then moved to where the shape sits.
/// Move one element and everything nested inside it.
///
/// The shallow form below deliberately leaves nested elements alone, and
/// `render_shape_text` depends on that. Vertical page alignment moves the
/// whole body band, so it needs this one. The two are named apart so neither
/// can be reached for by accident.
pub(super) fn translate_element_tree(element: &mut PositionedElement, dx: f64, dy: f64) {
    match element {
        PositionedElement::MarkedContent { children, .. } => {
            for child in children {
                translate_element_tree(child, dx, dy);
            }
        }
        PositionedElement::Group(group) => {
            group.transform.e += dx;
            group.transform.f += dy;
        }
        other => translate_element_shallow(other, dx, dy),
    }
}

/// Move one element, leaving anything nested inside it where it is.
fn translate_element_shallow(element: &mut PositionedElement, dx: f64, dy: f64) {
    match element {
        PositionedElement::Text(run) => {
            run.origin.x += dx;
            run.origin.y += dy;
        }
        PositionedElement::MultilingualText(run) => {
            run.origin.x += dx;
            run.origin.y += dy;
        }
        PositionedElement::Line { start, end, .. } => {
            start.x += dx;
            start.y += dy;
            end.x += dx;
            end.y += dy;
        }
        PositionedElement::FilledRect { rect, .. }
        | PositionedElement::Image { rect, .. }
        | PositionedElement::LinkAnnotation { rect, .. } => {
            rect.x += dx;
            rect.y += dy;
        }
        PositionedElement::Destination { point, .. } => {
            point.x += dx;
            point.y += dy;
        }
        // A marker moves with its band, which becomes the widget.
        PositionedElement::FormField(fragment) => {
            fragment.rect.x += dx;
            fragment.rect.y += dy;
            if let Some(band) = fragment
                .mark
                .as_mut()
                .and_then(|marker| marker.band.as_mut())
            {
                band.x += dx;
                band.y += dy;
            }
        }
        _ => {}
    }
}

/// Render a shape's text box inside `rect`, the shape less its insets.
///
/// The paragraphs arrive already laid out at the shape's width. They are
/// rendered as if they sat at the left margin and then translated onto the
/// shape, which keeps all the justification and indent handling in one place.
///
/// A shape whose extent is fixed shows `height` of text: Word hides the first
/// line that does not fit whole, and every line after it.
fn render_shape_text(
    text: &[ParagraphBlock],
    owners: Option<&[WordParagraphPaint]>,
    geometry: &PageGeometry,
    rect: Rect,
    height: Option<f64>,
    media: &HashMap<MediaId, ImageData>,
) -> oxoffice_layout::Result<PaintedElements> {
    if owners.is_some_and(|owners| owners.len() != text.len()) {
        return Err(oxoffice_layout::LayoutError::Layout(
            "Word shape paragraph owner count mismatch".into(),
        ));
    }
    let mut local = PaintLayers::default();
    let mut y = 0.0;
    let mut previous_space_after = 0.0;
    for (index, para) in text.iter().enumerate() {
        // Paragraph spacing as in the body: none above the first line, then
        // the part of the space before that exceeds the space after above.
        y += if y == 0.0 {
            0.0
        } else if geometry.do_not_use_html_paragraph_auto_spacing {
            para.space_before
        } else {
            (para.space_before - previous_space_after).max(0.0)
        };
        let mut bottom = y + para.content_offset_top;
        let fit = height.map_or(para.lines.len(), |height| {
            para.lines
                .iter()
                .take_while(|line| {
                    bottom += line.height;
                    bottom <= height + 0.01
                })
                .count()
        });
        let (kept, kept_paint);
        let (para, paint) = if fit < para.lines.len() {
            if fit == 0 {
                break;
            }
            let mut lines = para.clone();
            lines.lines.truncate(fit);
            kept = lines;
            kept_paint = owners
                .map(|owners| owners[index].for_line_range(&para.lines, 0..fit))
                .transpose()?;
            (&kept, kept_paint.as_ref())
        } else {
            (para, owners.map(|owners| &owners[index]))
        };
        local.append(&mut render_paragraph_lines(
            &para.lines,
            ParagraphView {
                line_start: 0,
                paint,
                baseline_policy: None,
                line_gaps: &[],
                border_group_before: false,
                block: para,
                semantics: None,
                reflow_direction: oxoffice_layout::TextDirection::Auto,
                reflow_allowed: true,
            },
            geometry,
            y,
            (true, true, 0.0),
            media,
        )?)?;
        if fit < text[index].lines.len() {
            break;
        }
        y += para.content_height() + para.space_after;
        previous_space_after = para.space_after;
    }
    let dx = rect.x - geometry.margin_left;
    let dy = rect.y - geometry.margin_top;
    let mut local = local.flatten()?;
    for element in local.coordinate_elements_mut() {
        translate_element_shallow(element, dx, dy);
    }
    Ok(local)
}

/// The elements of a shape placed in line with text, `width` by `height`
/// with its top left corner at the origin: its fill, and its text inside
/// `insets` as a floating shape draws them.
#[allow(clippy::too_many_arguments)]
pub(crate) fn inline_shape_elements(
    preset: ShapePreset,
    fill: Option<Color>,
    text: &[ParagraphBlock],
    (width, height): (f64, f64),
    insets: [f64; 4],
    fixed_height: bool,
    geometry: &PageGeometry,
    media: &HashMap<MediaId, ImageData>,
) -> oxoffice_layout::Result<Vec<PositionedElement>> {
    let mut elements = Vec::new();
    match (preset, fill) {
        (ShapePreset::Rect, Some(color)) => elements.push(PositionedElement::FilledRect {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width,
                height,
            },
            color,
        }),
        (ShapePreset::Line, Some(color)) => elements.push(PositionedElement::Line {
            start: Point { x: 0.0, y: 0.0 },
            end: Point {
                x: width,
                y: height,
            },
            width: 1.0,
            color,
            dash_pattern: None,
        }),
        _ => {}
    }
    let [left, top, right, bottom] = insets;
    let text_rect = Rect {
        x: left,
        y: top,
        width: (width - left - right).max(0.0),
        height: (height - top - bottom).max(0.0),
    };
    let (mut painted, _) = render_shape_text(
        text,
        None,
        geometry,
        text_rect,
        fixed_height.then_some(text_rect.height),
        media,
    )?
    .into_parts()?;
    elements.append(&mut painted);
    Ok(elements)
}

fn frame_h(rel: ST_RelativeFromH, g: &PageGeometry, indent_left: f64) -> (f64, f64) {
    let text_width = g.page_width - g.margin_left - g.margin_right;
    match rel {
        ST_RelativeFromH::Page | ST_RelativeFromH::LeftMargin => (0.0, g.page_width),
        ST_RelativeFromH::RightMargin | ST_RelativeFromH::OutsideMargin => {
            (g.page_width - g.margin_right, g.margin_right)
        }
        ST_RelativeFromH::InsideMargin => (g.margin_left, g.margin_left),
        // A character-relative offset starts where the text does on the line.
        ST_RelativeFromH::Character => (g.margin_left + indent_left, text_width),
        // Margin and column both start at the left edge of the text area.
        // Multiple columns are not laid out yet, so the two coincide.
        ST_RelativeFromH::Margin | ST_RelativeFromH::Column => (g.margin_left, text_width),
    }
}

pub(super) fn resolve_anchor_h(
    rel: ST_RelativeFromH,
    off: f64,
    align: Option<AnchorAlignH>,
    width: f64,
    g: &PageGeometry,
    indent_left: f64,
) -> f64 {
    let (start, size) = frame_h(rel, g, indent_left);
    match align {
        // Inside and outside mean binding-side and outer-edge, which differ on
        // facing pages. Facing pages are not modelled, so the odd-page reading
        // stands in for both.
        Some(AnchorAlignH::Left | AnchorAlignH::Inside) => start,
        Some(AnchorAlignH::Center) => start + (size - width) / 2.0,
        Some(AnchorAlignH::Right | AnchorAlignH::Outside) => start + size - width,
        None => start + off,
    }
}

/// Resolve a vertical anchor offset against the frame it is measured from.
///
/// `para_top` is the top of the anchoring paragraph, measured from the top of
/// the content area.
fn frame_v(rel: ST_RelativeFromV, g: &PageGeometry, para_top: f64) -> (f64, f64) {
    let text_height = g.page_height - g.margin_top - g.margin_bottom;
    match rel {
        ST_RelativeFromV::Page | ST_RelativeFromV::TopMargin => (0.0, g.page_height),
        ST_RelativeFromV::BottomMargin | ST_RelativeFromV::OutsideMargin => {
            (g.page_height - g.margin_bottom, g.margin_bottom)
        }
        ST_RelativeFromV::Margin | ST_RelativeFromV::InsideMargin => (g.margin_top, text_height),
        // Paragraph and line are both relative to where this paragraph landed.
        // Per-line anchoring would need the line box, which is finer than we
        // track here, so the paragraph top stands in for both.
        ST_RelativeFromV::Paragraph | ST_RelativeFromV::Line => {
            (g.margin_top + para_top, text_height)
        }
    }
}

pub(super) fn resolve_anchor_v(
    rel: ST_RelativeFromV,
    off: f64,
    align: Option<AnchorAlignV>,
    height: f64,
    g: &PageGeometry,
    para_top: f64,
) -> f64 {
    let (start, size) = frame_v(rel, g, para_top);
    match align {
        Some(AnchorAlignV::Top | AnchorAlignV::Inside) => start,
        Some(AnchorAlignV::Center) => start + (size - height) / 2.0,
        Some(AnchorAlignV::Bottom | AnchorAlignV::Outside) => start + size - height,
        None => start + off,
    }
}

pub(super) fn anchored_rect(
    anchor: &AnchoredDrawing,
    page: &PageGeometry,
    paragraph: &PageGeometry,
    top: f64,
    indent: f64,
) -> Rect {
    let horizontal = match anchor.rel_h {
        ST_RelativeFromH::Column | ST_RelativeFromH::Character => paragraph,
        _ => page,
    };
    Rect {
        x: resolve_anchor_h(
            anchor.rel_h,
            anchor.off_h,
            anchor.align_h,
            anchor.width,
            horizontal,
            indent,
        ),
        y: resolve_anchor_v(
            anchor.rel_v,
            anchor.off_v,
            anchor.align_v,
            anchor.height,
            page,
            paragraph.margin_top + top - page.margin_top,
        ),
        width: anchor.width,
        height: anchor.height,
    }
}
