//! Closing a page: line numbers, vertical alignment, rotation, column separators and page borders.

use super::*;

/// One body line the page has placed, waiting to be numbered in the margin.
#[derive(Debug, Clone, Copy)]
pub(super) struct NumberedLine {
    /// Baseline, measured from the top of the content area.
    baseline: f64,
    /// Left edge of the column track the line sits in.
    track_x: f64,
}

/// Point size the margin line numbers are drawn at.
///
/// Word takes this from the document default. A fixed size keeps the numbers
/// deterministic, which a rendering baseline needs.
const LINE_NUMBER_FONT_SIZE: f64 = 9.0;

impl<'a> Pager<'a> {
    /// Number the body lines this page placed, in the margin beside them.
    ///
    /// The numbers are page furniture, so they are wrapped in marked content
    /// with no structure and never enter the PDF reading order.
    fn draw_line_numbers(&mut self) {
        let placed = std::mem::take(&mut self.numbered_lines);
        let Some(numbering) = self.section_geometry.line_numbers.clone() else {
            return;
        };
        let mut children = Vec::new();
        for line in placed {
            let value = self.next_line_number;
            self.next_line_number = if self.line_number_style.is_some() {
                self.next_line_number.saturating_add(1)
            } else {
                self.next_line_number
                    .saturating_add(1)
                    .min(u64::from(u32::MAX))
            };
            if !value.is_multiple_of(u64::from(numbering.count_by)) {
                continue;
            }
            let (font_id, font_size, bold, italic, kerning) = match self.line_number_style {
                Some(style) => (
                    style.font_id,
                    style.size,
                    style.bold,
                    style.italic,
                    Some(style.kerning),
                ),
                None => (numbering.font_id, LINE_NUMBER_FONT_SIZE, false, false, None),
            };
            let Some(font_id) = font_id else {
                continue;
            };
            if !font_size.is_finite() || font_size <= 0.0 {
                continue;
            }
            let text = value.to_string();
            let shaped = match kerning {
                Some(enabled) => self
                    .fm
                    .shape_text_with_kerning(font_id, &text, font_size, enabled),
                None => self.fm.shape_text(font_id, &text, font_size),
            };
            let Ok(shaped) = shaped else {
                continue;
            };
            children.push(PositionedElement::Text(GlyphRun {
                origin: Point {
                    x: line.track_x - numbering.distance - shaped.width,
                    // The baseline and the track were both recorded against
                    // the body band, so the top margin comes from the band
                    // too. For every section but a vertical one the band's
                    // top margin is the page's, so nothing moves.
                    y: self.geometry.margin_top + line.baseline,
                },
                font_id,
                font_size,
                glyph_ids: shaped.glyph_ids,
                advances: shaped.advances,
                text,
                source: None,
                color: Color::BLACK,
                bold,
                italic,
                field_kind: None,
                field_source: None,
                note: None,
                note_reference_source: None,
                tab_aligned: None,
            }));
        }
        if !children.is_empty() {
            self.elements.push(PositionedElement::MarkedContent {
                structure: None,
                children,
            });
        }
    }

    /// Record the body lines placed at `top`, so the page can number them.
    ///
    /// A section without line numbering records nothing, so nothing else in
    /// the workspace pays for this.
    pub(super) fn record_numbered_lines(
        &mut self,
        lines: &[LayoutLine],
        paragraph: ParagraphView<'_>,
        top: f64,
        first_fragment: bool,
    ) {
        if self.section_geometry.line_numbers.is_none() {
            return;
        }
        let track_x = self.geometry.margin_left;
        let mut y = top;
        let top_padding = if first_fragment {
            paragraph.borders.as_ref().map_or(0.0, |borders| {
                crate::block::paragraph_border_padding(borders.top.as_ref())
            })
        } else {
            0.0
        };
        for (index, line) in lines.iter().enumerate() {
            y += paragraph.line_gaps.get(index).copied().unwrap_or(0.0);
            // What follows an absorbed page break takes no room and gets no
            // number of its own.
            if takes_no_room(line) {
                continue;
            }
            let baseline =
                if let Some(baseline) = paragraph_line_baseline(line, paragraph.baseline_policy) {
                    // The painter removes framing from the content line before
                    // applying the private baseline, then adds top padding once.
                    y + if index == 0 { top_padding } else { 0.0 } + baseline
                } else {
                    y + line.ascent
                };
            self.numbered_lines.push(NumberedLine { baseline, track_x });
            y += line.height;
        }
    }

    /// Shift the body band down for a centred or bottom vertical alignment.
    ///
    /// `None`, `Top` and `Both` return before touching anything, which is the
    /// untouched code path. `Both` keeps its source value and lays out as
    /// `Top`, and true vertical distribution is a named follow-up.
    fn apply_vertical_alignment(&mut self) {
        // The note area is reserved out of the content height before the band
        // moves, or a bottom-aligned page would push its body into the notes
        // it already made room for.
        let unused = (self.content_height - self.reserved_height() - self.ink_bottom).max(0.0);
        let dy = match self.page_geometry.vertical_alignment {
            Some(ST_VerticalJc::Center) => unused / 2.0,
            Some(ST_VerticalJc::Bottom) => unused,
            _ => return,
        };
        if dy <= 0.0 {
            return;
        }
        let translated = (|| {
            self.element_policies
                .translate_annotation_roots(&self.elements, 0.0, dy)?;
            self.behind_policies
                .translate_annotation_roots(&self.behind_elements, 0.0, dy)?;
            self.front_policies
                .translate_annotation_roots(&self.front_elements, 0.0, dy)
        })();
        if let Err(error) = translated {
            self.pending_furniture_failure
                .get_or_insert(error.to_string());
            return;
        }
        for element in self
            .elements
            .iter_mut()
            .chain(self.behind_elements.iter_mut())
            .chain(self.front_elements.iter_mut())
        {
            translate_element_tree(element, 0.0, dy);
        }
        // Fragments were captured during placement, including the section's
        // final rotation. Publish the same aligned origin as painted content.
        let (dx, dy) = self
            .body_rotation_transform()
            .map_or((0.0, dy), |transform| (transform.c * dy, transform.d * dy));
        for (_, fragment) in &mut self.body_fragments {
            if fragment.physical_page == self.page_number {
                fragment.x += dx;
                fragment.y += dy;
            }
        }
    }

    /// The transform a vertical section's body band is painted through.
    ///
    /// The band was laid out transposed about its own centre, so rotating
    /// about that same centre lands it back on the page. `None` is the
    /// horizontal section, which is painted where it was laid out.
    pub(super) fn body_rotation_transform(&self) -> Option<Transform> {
        let degrees = self.page_geometry.body_rotation?;
        Some(Transform::rotate_about(
            degrees,
            self.page_geometry.margin_left + self.page_geometry.text_measure() / 2.0,
            self.page_geometry.margin_top + self.page_geometry.content_height() / 2.0,
        ))
    }

    /// Rotate the painted body band for a vertical section.
    ///
    /// Notes, headers, footers and page borders are placed after this and stay
    /// upright, and so do a cell's change bars and anchored drawings.
    fn apply_body_rotation(&mut self) {
        let Some(transform) = self.body_rotation_transform() else {
            return;
        };
        let result = (|| {
            wrap_paint_suffix(
                &mut self.elements,
                &mut self.element_policies,
                0,
                transform,
                None,
                false,
            )?;
            wrap_paint_suffix(
                &mut self.behind_elements,
                &mut self.behind_policies,
                0,
                transform,
                None,
                false,
            )?;
            wrap_paint_suffix(
                &mut self.front_elements,
                &mut self.front_policies,
                0,
                transform,
                None,
                false,
            )
        })();
        if let Err(error) = result {
            self.pending_furniture_failure
                .get_or_insert(error.to_string());
        }
    }

    /// Draw the rule between column tracks, when `w:sep` asked for one.
    fn draw_column_separators(&self, elements: &mut Vec<PositionedElement>) {
        if !self.section_geometry.column_separator || self.tracks.len() < 2 {
            return;
        }
        let top = self.page_geometry.margin_top;
        let bottom = top + self.content_height;
        for pair in self.tracks.windows(2) {
            let x = (pair[0].x + pair[0].width + pair[1].x) / 2.0;
            elements.push(PositionedElement::Line {
                start: Point { x, y: top },
                end: Point { x, y: bottom },
                width: 0.5,
                color: Color::BLACK,
                dash_pattern: None,
            });
        }
    }

    /// The section's page border frame, for the page being built.
    fn page_border_elements(&self) -> Vec<PositionedElement> {
        let Some(frame) = self.furniture_geometry.page_borders.as_ref() else {
            return Vec::new();
        };
        let draws = match frame.display {
            ST_PageBorderDisplay::AllPages => true,
            ST_PageBorderDisplay::FirstPage => self.is_first_page,
            ST_PageBorderDisplay::NotFirstPage => !self.is_first_page,
        };
        if !draws {
            return Vec::new();
        }
        let geometry = &self.furniture_geometry;
        let mut elements = Vec::new();
        match frame.offset_from {
            ST_PageBorderOffset::Text => {
                // Each edge's `w:space` is measured outward from the text,
                // which is exactly what the shared border renderer applies.
                render_border_edges(
                    &frame.edges,
                    geometry.margin_left,
                    geometry.margin_top,
                    geometry.text_measure(),
                    geometry.content_height(),
                    &mut elements,
                );
            }
            ST_PageBorderOffset::Page => {
                // Here `w:space` is measured inward from the page edge, so the
                // rectangle is inset here and the edges carry no offset.
                let inset = |edge: &Option<CT_BorderEdge>| {
                    edge.as_ref().and_then(|edge| edge.space).unwrap_or(0) as f64
                };
                let left = inset(&frame.edges.left);
                let right = inset(&frame.edges.right);
                let top = inset(&frame.edges.top);
                let bottom = inset(&frame.edges.bottom);
                let strip = |edge: &Option<CT_BorderEdge>| {
                    edge.as_ref().map(|edge| CT_BorderEdge {
                        space: None,
                        ..edge.clone()
                    })
                };
                let edges = CT_PBdr {
                    top: strip(&frame.edges.top),
                    left: strip(&frame.edges.left),
                    bottom: strip(&frame.edges.bottom),
                    right: strip(&frame.edges.right),
                    between: None,
                    bar: None,
                };
                render_border_edges(
                    &edges,
                    left,
                    top,
                    geometry.page_width - left - right,
                    geometry.page_height - top - bottom,
                    &mut elements,
                );
            }
        }
        elements
    }

    /// The header and footer variant Word selects for the page being built.
    ///
    /// A title page takes its first variant and, when the document turns
    /// even and odd headers on, an even displayed page takes its even
    /// variant. A selected variant stays selected when it is blank.
    pub(super) fn story_variant(&self, hf: &HeaderFooterContent) -> StoryVariant {
        if self.is_first_page && self.title_pg {
            StoryVariant::First
        } else if hf.even_headers_active && self.header_page_number.is_multiple_of(2) {
            StoryVariant::Even
        } else {
            StoryVariant::Default
        }
    }

    pub(super) fn finish_page(&mut self) {
        if let Some(reason) = self.pending_furniture_failure.take() {
            self.furniture_failure.get_or_insert(FurnitureFailure {
                section: self.page_member.unwrap_or(self.active_member),
                page: self.page_number,
                reason,
            });
        }
        if self.advance_column_track() {
            return;
        }
        // Numbers label body lines, so they are drawn before the band moves.
        // Notes are anchored to the bottom margin and must not move with it,
        // so they are placed after.
        self.draw_line_numbers();
        self.apply_vertical_alignment();
        self.apply_body_rotation();
        // Endnotes that follow the body on this page are placed from the
        // section's top margin, so the band's offset from it is added back.
        let band_offset = self.page_geometry.margin_top - self.section_geometry.margin_top;
        self.page_flow_spaces.push(PageFlowSpace {
            body_bottom: self.ink_bottom + band_offset,
            note_top: self.content_height - self.reserved_height() + band_offset,
        });
        self.place_page_notes();
        let composed =
            (|| -> oxoffice_layout::Result<(PageFrame, PaintPolicies, PagePaintBoundary)> {
                if let Some(reason) = self.pending_furniture_failure.take() {
                    return Err(oxoffice_layout::LayoutError::Layout(reason));
                }
                let mut header = if let (Some(hf), Some(prepared)) =
                    (self.header_footer, &self.prepared_header)
                {
                    let variant = self.story_variant(hf);
                    render_hf_blocks(
                        prepared,
                        self.header_footer_semantics
                            .map(|facts| facts.header(variant)),
                        &self.furniture_geometry,
                        self.page_number,
                        self.media,
                    )?
                } else {
                    PaintLayers::default()
                };
                let mut footer = if let (Some(hf), Some(prepared)) =
                    (self.header_footer, &self.prepared_footer)
                {
                    let variant = self.story_variant(hf);
                    render_hf_blocks(
                        prepared,
                        self.header_footer_semantics
                            .map(|facts| facts.footer(variant)),
                        &self.furniture_geometry,
                        self.page_number,
                        self.media,
                    )?
                } else {
                    PaintLayers::default()
                };
                let mut all = PaintedElements::default();
                let mut page_borders = PaintedElements::non_field(self.page_border_elements())?;
                if !self
                    .page_geometry
                    .page_borders
                    .as_ref()
                    .is_some_and(|frame| frame.in_front)
                {
                    all.append(&mut page_borders)?;
                }
                if let Some(hf) = self.header_footer
                    && let Some(watermark) = hf.watermark(self.story_variant(hf))
                {
                    all.push_non_field(PositionedElement::Group(watermark.clone()))?;
                }
                all.append(&mut PaintedElements::from_parts(
                    std::mem::take(&mut self.behind_elements),
                    std::mem::take(&mut self.behind_policies),
                )?)?;
                all.append(&mut header.behind)?;
                all.append(&mut footer.behind)?;
                let behind_end = all.len();
                all.append(&mut header.regular)?;
                let mut separators = Vec::new();
                self.draw_column_separators(&mut separators);
                all.append(&mut PaintedElements::non_field(separators)?)?;
                all.append(&mut PaintedElements::from_parts(
                    std::mem::take(&mut self.elements),
                    std::mem::take(&mut self.element_policies),
                )?)?;
                all.append(&mut footer.regular)?;
                let regular_end = all.len();
                all.append(&mut PaintedElements::from_parts(
                    std::mem::take(&mut self.front_elements),
                    std::mem::take(&mut self.front_policies),
                )?)?;
                let body_front_end = all.len();
                all.append(&mut header.front)?;
                all.append(&mut footer.front)?;
                let furniture_front_end = all.len();
                all.append(&mut page_borders)?;
                let elements_len = all.len();
                let boundary = PagePaintBoundary {
                    behind_end,
                    regular_end,
                    body_front_end,
                    furniture_front_end,
                    elements_len,
                };
                boundary.validate(elements_len)?;
                let (elements, policies) = all.into_parts()?;
                let mut page = PageFrame::new(
                    self.page_number,
                    self.page_geometry.page_width,
                    self.page_geometry.page_height,
                    elements,
                );
                page.displayed_page_number = self.header_page_number;
                Ok((page, policies, boundary))
            })();
        match composed {
            Ok((page, policies, boundary)) => {
                self.pages.push(page);
                self.page_policies.push(policies);
                self.page_boundaries.push(boundary);
            }
            Err(error) => {
                self.furniture_failure.get_or_insert(FurnitureFailure {
                    section: self.page_member.unwrap_or(self.active_member),
                    page: self.page_number,
                    reason: error.to_string(),
                });
                return;
            }
        }
        self.page_number += 1;
        self.header_page_number += 1;
        self.cursor_y = 0.0;
        self.page_wraps.clear();
        self.page_floats.clear();
        self.ink_bottom = 0.0;
        self.has_content_flag = false;
        self.is_first_page = false;
        self.page_member = None;
        self.pending_continuous_footnote_break = false;
        self.begin_page();
        self.select_empty_page_member();
    }
}
