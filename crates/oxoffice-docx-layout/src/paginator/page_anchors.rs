//! Anchored drawings and floating tables placed on the page being filled.

use super::*;

impl<'a> Pager<'a> {
    /// Resolve the wrapping drawings a paragraph carries, without placing
    /// them. Measuring a paragraph needs to know what it must flow around
    /// before anything is committed to the page.
    pub(super) fn wrap_rects_for(
        &self,
        anchored: &[AnchoredDrawing],
        para_top: f64,
        indent_left: f64,
    ) -> Vec<PlacedWrap> {
        anchored
            .iter()
            .filter(|a| a.wrap != WrapType::None)
            .map(|a| PlacedWrap {
                rect: anchored_rect(
                    a,
                    &self.furniture_geometry,
                    &self.geometry,
                    para_top,
                    indent_left,
                ),
                wrap: a.wrap,
                dist_top: a.dist_top,
                dist_bottom: a.dist_bottom,
                dist_left: a.dist_left,
                dist_right: a.dist_right,
            })
            .collect()
    }

    /// Wrapping drawings anchored to blocks after `block_idx`, positioned well
    /// enough to flow this block's text around them.
    ///
    /// A drawing anchored to a later paragraph still pushes earlier text aside,
    /// and Word documents do this routinely: the arrow beside a paragraph is
    /// often anchored to the paragraph after it. Where its position comes from
    /// depends on the frame it is measured against.
    ///
    /// A drawing framed by the page or a margin is positioned here, because
    /// that needs nothing from the block that owns it. A drawing framed by its
    /// own paragraph has no position until that paragraph is placed, so the
    /// first pass offers nothing for it and the second offers what the first
    /// recorded, for the drawings the first put on the page being built now.
    pub(super) fn lookahead_wraps<B: LayoutBlockLike>(
        &self,
        block_idx: usize,
        blocks: &[B],
    ) -> Vec<PlacedWrap> {
        let mut out = Vec::new();
        let mut height = self.cursor_y;
        // Seeded with the floats already on this page, so a float the flow has
        // not reached yet is offered at the place it will actually settle on
        // rather than at its unresolved anchor.
        let mut floats = self.page_floats.clone();

        for (offset, block) in blocks.iter().enumerate().skip(block_idx + 1) {
            if block.page_break_before() || height > self.content_height {
                break;
            }
            height += block.space_before() + block.content_height() + block.space_after();

            if let Some(table) = block.table() {
                if let Some(floating) = table.floating.as_deref() {
                    // A floating table is the same obstacle as a drawing, and
                    // the same rule decides whether this pass can position it.
                    // Anchor index 0 cannot collide, because a table block
                    // carries no `anchored` vector.
                    //
                    // Only a block-measured float is re-offered from the
                    // previous pass, so a float that moved to the next page for
                    // any other reason is still offered here. That is the
                    // recorded boundary in `08-rendering-spec.md`.
                    let placed = if is_paragraph_relative_float(floating) {
                        // Already settled by the previous pass, or not at all.
                        self.resolved_in
                            .get(&(offset, 0))
                            .filter(|(page, _)| *page == self.page_number)
                            .map(|(_, placed)| *placed)
                    } else {
                        let mut candidate = floating_wrap(
                            floating,
                            table.table_width,
                            table.content_height(),
                            &self.geometry,
                            &self.furniture_geometry,
                            0.0,
                        );
                        settle_float(&mut candidate, floating.overlap_allowed, &floats);
                        Some(candidate)
                    };
                    if let Some(placed) = placed {
                        floats.push((placed, floating.overlap_allowed));
                        out.push(placed);
                    }
                }
                continue;
            }

            let Some(para) = block.paragraph() else {
                continue;
            };
            for (anchor_idx, a) in para.anchored.iter().enumerate() {
                if a.wrap == WrapType::None {
                    continue;
                }
                if is_paragraph_relative_wrap(a) {
                    // Resolved by the previous pass, or not at all. The page
                    // check is what stops a drawing that landed overleaf from
                    // pushing this page's text aside.
                    if let Some((page, placed)) = self.resolved_in.get(&(offset, anchor_idx))
                        && *page == self.page_number
                    {
                        out.push(*placed);
                    }
                    continue;
                }
                out.extend(self.wrap_rects_for(std::slice::from_ref(a), 0.0, para.indent_left));
            }
        }

        out
    }

    /// Place the drawings anchored to a paragraph whose top sits at `para_top`,
    /// measured from the top of the content area.
    ///
    /// `block_idx` identifies the owning block, so a paragraph-relative
    /// wrapping drawing can be recorded for the pass that follows this one.
    pub(super) fn place_anchored(
        &mut self,
        anchored: &[AnchoredDrawing],
        paint: Option<&WordParagraphPaint>,
        para_top: f64,
        indent_left: f64,
        block_idx: usize,
    ) {
        if paint.is_some_and(|paint| paint.anchors.len() != anchored.len()) {
            self.pending_furniture_failure
                .get_or_insert("Word anchor producer count mismatch".into());
            return;
        }
        for (anchor_idx, a) in anchored.iter().enumerate() {
            let rect = anchored_rect(
                a,
                &self.furniture_geometry,
                &self.geometry,
                para_top,
                indent_left,
            );

            if a.wrap != WrapType::None {
                let placed = PlacedWrap {
                    rect,
                    wrap: a.wrap,
                    dist_top: a.dist_top,
                    dist_bottom: a.dist_bottom,
                    dist_left: a.dist_left,
                    dist_right: a.dist_right,
                };
                if is_paragraph_relative_wrap(a) {
                    self.resolved_out
                        .insert((block_idx, anchor_idx), (self.page_number, placed));
                }
                self.page_wraps.push(placed);
            }

            let owner = paint.map_or(AnchorPaintOwner::Neutral, |paint| {
                AnchorPaintOwner::Word(&paint.anchors[anchor_idx])
            });
            let result =
                anchored_elements(a, owner, rect, &self.geometry, self.media).map(|produced| {
                    let mut layers = PaintLayers::default();
                    if a.behind_doc {
                        layers.behind = produced;
                    } else {
                        layers.front = produced;
                    }
                    layers
                });
            self.accept_layers(result);
        }
    }
}

impl<'a> Pager<'a> {
    pub(super) fn accept_layers(&mut self, result: oxoffice_layout::Result<PaintLayers>) {
        let result = result.and_then(|layers| {
            self.element_policies
                .append(&mut self.elements, layers.regular)?;
            self.behind_policies
                .append(&mut self.behind_elements, layers.behind)?;
            self.front_policies
                .append(&mut self.front_elements, layers.front)
        });
        if let Err(error) = result {
            self.pending_furniture_failure
                .get_or_insert(error.to_string());
        }
    }

    /// Place a floating table at its resolved anchor, leaving the flow alone.
    ///
    /// A float is positioned, not flowed, so `cursor_y` is untouched and every
    /// block after it sits where it would have without it. What changes is the
    /// text, which flows around the keep-out band this pushes onto the page.
    ///
    /// `para_top` is the top of the floating block, measured from the top of
    /// the content area, which is what a `w:vertAnchor="text"` float resolves
    /// against.
    pub(super) fn place_floating_table(
        &mut self,
        table: &TableView<'_>,
        floating: &FloatingTable,
        body_index: Option<usize>,
        block_idx: usize,
        para_top: f64,
    ) {
        let width = table.table_width;
        let height = table.content_height();
        let mut placed = floating_wrap(
            floating,
            width,
            height,
            &self.geometry,
            &self.furniture_geometry,
            para_top,
        );
        settle_float(&mut placed, floating.overlap_allowed, &self.page_floats);

        // A float that runs past the bottom of the body moves whole to the next
        // page, which is what Word does and what a row loop cannot express.
        //
        // Moving helps only when the rect depends on this page. That is a float
        // measured from its own block, and a float that had to drop below one
        // already on the page. A page or margin frame on an empty page resolves
        // to the same place overleaf, so moving it changes nothing and retrying
        // it would not terminate. One attempt either way.
        let depends_on_this_page =
            is_paragraph_relative_float(floating) || !self.page_floats.is_empty();
        if depends_on_this_page
            && self.has_content()
            && placed.rect.y + height > self.geometry.margin_top + self.available_height() + 0.01
        {
            self.finish_page();
            placed = floating_wrap(
                floating,
                width,
                height,
                &self.geometry,
                &self.furniture_geometry,
                self.cursor_y,
            );
            settle_float(&mut placed, floating.overlap_allowed, &self.page_floats);
        }

        // Render every row at the resolved origin. A float never crosses a page
        // boundary, so it never repeats a header row.
        let mut row_y = placed.rect.y;
        for (row_idx, row) in table.rows.iter().enumerate() {
            if let Some(body_index) = body_index {
                self.record_body_fragment(body_index, placed.rect.x, row_y, width, row.height);
            }
            self.accept_layers(render_table_row(
                row,
                table
                    .semantics
                    .and_then(|semantics| semantics.rows.get(row_idx)),
                &table.col_widths,
                placed.rect.x,
                row_y,
                &self.geometry,
                &self.furniture_geometry,
                self.page_number,
                table.borders.as_ref(),
                table.bidi_visual,
                self.media,
                true,
                true,
            ));
            row_y += row.height;
        }

        if is_paragraph_relative_float(floating) {
            self.resolved_out
                .insert((block_idx, 0), (self.page_number, placed));
        }
        self.page_wraps.push(placed);
        self.page_floats.push((placed, floating.overlap_allowed));
        self.mark_content();
    }
}
