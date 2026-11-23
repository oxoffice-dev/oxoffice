//! The pager: opening, filling and closing the pages of one pass.

use super::*;

impl<'a> Pager<'a> {
    pub(super) fn new(
        geometry: PageGeometry,
        header_footer: Option<&'a HeaderFooterContent>,
        header_footer_semantics: Option<&'a HeaderFooterSemantics>,
        title_pg: bool,
        footnotes_beneath_text: bool,
        written_tb_rl_band: bool,
        line_number_style: Option<WordLineNumberStyle>,
        media: &'a HashMap<MediaId, ImageData>,
        notes: &'a NoteRegistry,
        fm: &'a FontManager,
        resolved_in: &'a ResolvedWraps,
        first_page_number: usize,
        first_header_page_number: usize,
        is_first_page: bool,
        stop_at: Option<PaginationCheckpoint>,
    ) -> Self {
        let next_line_number = geometry.line_numbers.as_ref().map_or(1, |numbering| {
            u64::from(numbering.start) + u64::from(line_number_style.is_some())
        });
        let mut pager = Pager {
            line_number_style,
            written_tb_rl_band,
            pages: Vec::new(),
            elements: Vec::new(),
            behind_elements: Vec::new(),
            front_elements: Vec::new(),
            next_repetition_scope: 1,
            element_policies: PaintPolicies::default(),
            behind_policies: PaintPolicies::default(),
            front_policies: PaintPolicies::default(),
            page_policies: Vec::new(),
            page_boundaries: Vec::new(),
            cursor_y: 0.0,
            page_number: first_page_number,
            header_page_number: first_header_page_number,
            content_height: geometry.content_height(),
            section_geometry: geometry.clone(),
            page_geometry: geometry.clone(),
            furniture_geometry: geometry.clone(),
            prepared_header: None,
            prepared_footer: None,
            pending_furniture_failure: None,
            furniture_failure: None,
            tracks: Vec::new(),
            track_index: 0,
            numbered_lines: Vec::new(),
            next_line_number,
            geometry,
            header_footer,
            header_footer_semantics,
            continuous_sections: None,
            active_member: 0,
            pending_continuous_footnote_break: false,
            page_member: None,
            member_first_pages: HashMap::new(),
            has_content_flag: false,
            outlines: Vec::new(),
            pending_outline: None,
            is_first_page,
            title_pg,
            footnotes_beneath_text,
            media,
            notes,
            page_note_ids: Vec::new(),
            pending_notes: Vec::new(),
            fm,
            page_wraps: Vec::new(),
            page_floats: Vec::new(),
            ink_bottom: 0.0,
            page_flow_spaces: Vec::new(),
            previous_space_after: 0.0,
            resolved_in,
            resolved_out: ResolvedWraps::new(),
            body_fragments: Vec::new(),
            checkpoints: Vec::new(),
            stop_at,
            stopped_at: None,
            trailing_run_page_break_before: None,
        };
        pager.begin_page();
        pager
    }

    /// Resolve the geometry of the page now being built.
    ///
    /// A section that neither mirrors its margins nor declares column tracks,
    /// and whose header and footer stay within its margins, gets its own
    /// geometry back unchanged, which is the path most documents take.
    pub(super) fn begin_page(&mut self) {
        self.next_repetition_scope = 1;
        self.track_index = 0;
        let mut page = self.section_geometry.without_columns();
        if self.section_geometry.mirror_margins && self.header_page_number.is_multiple_of(2) {
            std::mem::swap(&mut page.margin_left, &mut page.margin_right);
        }
        // A negative top or bottom margin is laid out at its absolute value.
        page.margin_top = page.margin_top.abs();
        page.margin_bottom = page.margin_bottom.abs();
        self.furniture_geometry = page.clone();
        self.prepared_header = None;
        self.prepared_footer = None;
        self.pending_furniture_failure = None;
        if let Some(hf) = self.header_footer {
            let variant = self.story_variant(hf);
            for footer in [false, true] {
                let blocks = if footer {
                    hf.footer(variant)
                } else {
                    hf.header(variant)
                };
                let facts = self.header_footer_semantics.map(|facts| {
                    if footer {
                        facts.footer(variant)
                    } else {
                        facts.header(variant)
                    }
                });
                match prepare_furniture(blocks, facts, &self.furniture_geometry, footer, self.fm) {
                    Ok(prepared) => {
                        self.page_wraps
                            .extend_from_slice(&prepared.body_frame_wraps);
                        if footer {
                            self.prepared_footer = Some(prepared);
                        } else {
                            self.prepared_header = Some(prepared);
                        }
                    }
                    Err(reason) => {
                        self.pending_furniture_failure = Some(format!(
                            "{}: {reason}",
                            if footer { "footer" } else { "header" }
                        ));
                    }
                }
            }
        }
        self.clear_header_and_footer(&mut page);
        self.content_height = page.content_height();
        let shift = page.margin_left - self.section_geometry.margin_left;
        self.tracks = self
            .section_geometry
            .columns
            .iter()
            .map(|track| ColumnTrack {
                x: track.x + shift,
                width: track.width,
            })
            .collect();
        self.page_geometry = page;
        if let Some(numbering) = self.section_geometry.line_numbers.as_ref()
            && numbering.restart == ST_LineNumberRestart::NewPage
        {
            self.next_line_number =
                u64::from(numbering.start) + u64::from(self.line_number_style.is_some());
        }
        self.apply_active_track();
    }

    /// Move the body band of `page` clear of its header and footer.
    ///
    /// Word starts the body below a header that reaches past the top margin,
    /// and ends it above a footer that reaches past the bottom margin. Only
    /// the stories' flow counts, so an anchored drawing pushes nothing. A
    /// negative `w:top` or `w:bottom` asks for that margin exactly, and the
    /// header or footer may then overlap the body.
    fn clear_header_and_footer(&self, page: &mut PageGeometry) {
        if self.written_tb_rl_band {
            return;
        }
        if self.section_geometry.margin_top >= 0.0
            && let Some(prepared) = &self.prepared_header
            && prepared.height > 0.0
        {
            page.margin_top = page.margin_top.max(prepared.origin + prepared.height);
        }
        if self.section_geometry.margin_bottom >= 0.0
            && let Some(prepared) = &self.prepared_footer
            && prepared.height > 0.0
        {
            page.margin_bottom = page.margin_bottom.max(page.page_height - prepared.origin);
        }
    }

    /// Narrow the body geometry to the active column track.
    fn apply_active_track(&mut self) {
        let Some(track) = self.tracks.get(self.track_index).copied() else {
            self.geometry = self.page_geometry.clone();
            self.transpose_body_band();
            return;
        };
        self.geometry = PageGeometry {
            margin_left: track.x,
            margin_right: self.page_geometry.page_width - track.x - track.width,
            columns: vec![track],
            ..self.page_geometry.clone()
        };
        self.transpose_body_band();
    }

    /// Swap the body band about its own centre for a vertical section.
    ///
    /// The band is the box the flow fills, so transposing it here is what
    /// makes the paginator break lines down the page. Painting rotates the
    /// result back about the same centre. A horizontal section returns before
    /// touching anything.
    fn transpose_body_band(&mut self) {
        if self.geometry.body_rotation.is_none() {
            return;
        }
        let (x, y, width, height) = crate::table::transposed_box(
            self.geometry.margin_left,
            self.geometry.margin_top,
            self.geometry.text_measure(),
            self.geometry.content_height(),
        );
        self.geometry.margin_left = x;
        self.geometry.margin_right = self.geometry.page_width - x - width;
        self.geometry.margin_top = y;
        self.geometry.margin_bottom = self.geometry.page_height - y - height;
        // The transposed band is one track. A column track of the untransposed
        // band no longer describes it.
        self.geometry.columns = Vec::new();
        self.content_height = height;
    }

    /// Move the body into the next column track of this page.
    ///
    /// False when the section has no tracks, or the last one just filled, and
    /// then the caller finishes the page instead.
    pub(super) fn advance_column_track(&mut self) -> bool {
        if self.track_index + 1 >= self.tracks.len() {
            return false;
        }
        self.track_index += 1;
        self.apply_active_track();
        self.cursor_y = 0.0;
        self.has_content_flag = false;
        true
    }

    /// End the page outright, whichever column track the body is filling.
    ///
    /// An explicit page break leaves the remaining columns empty, which is
    /// what Word does. A column break is a different element and is not
    /// modeled.
    pub(super) fn finish_page_outright(&mut self) {
        self.track_index = self.tracks.len().saturating_sub(1);
        self.finish_page();
    }

    pub(super) fn has_content(&self) -> bool {
        self.has_content_flag
    }

    /// The space a paragraph adds above itself at the cursor.
    ///
    /// Nothing at the top of a page. Word otherwise keeps the larger of the
    /// previous paragraph's space after and this space before, so only the
    /// part of `before` that exceeds the space after already below the cursor
    /// is added, unless the document asks for the two to be summed.
    pub(super) fn space_before(&self, before: f64) -> f64 {
        if self.cursor_y == 0.0 {
            0.0
        } else if self.geometry.do_not_use_html_paragraph_auto_spacing {
            before
        } else {
            (before - self.previous_space_after).max(0.0)
        }
    }

    pub(super) fn consume_trailing_run_page_break_before(&mut self, block_index: usize) -> bool {
        let matches = self.trailing_run_page_break_before == Some(block_index);
        self.trailing_run_page_break_before = None;
        matches
    }
}

impl<'a> Pager<'a> {
    pub(super) fn select_empty_page_member(&mut self) {
        if self.page_member.is_some() {
            return;
        }
        let Some(sections) = self.continuous_sections else {
            return;
        };
        let member = &sections[self.active_member];
        let first = self
            .member_first_pages
            .get(&self.active_member)
            .copied()
            .unwrap_or(self.page_number);
        if let Some(start) = member.page_number_start {
            self.header_page_number = start.saturating_add(self.page_number - first);
        }
        self.header_footer = member.header_footer.as_ref();
        self.header_footer_semantics = member.header_footer_semantics.as_ref();
        self.title_pg = member.title_pg;
        self.is_first_page = self.page_number == first;
        self.footnotes_beneath_text = member.footnotes_beneath_text;
        self.section_geometry = member.geometry.clone();
        self.line_number_style = member.line_number_style;
        self.written_tb_rl_band = member.written_tb_rl_band;
        let track = self.track_index;
        self.begin_page();
        self.track_index = track.min(self.tracks.len().saturating_sub(1));
        self.apply_active_track();
    }

    pub(super) fn mark_content(&mut self) {
        if self.continuous_sections.is_some() {
            self.member_first_pages
                .entry(self.active_member)
                .or_insert(self.page_number);
            if self.page_member.is_none() {
                self.page_member = Some(self.active_member);
            }
        }
        self.has_content_flag = true;
    }

    /// Record the waiting heading's outline entry at the cursor, which is
    /// where the paragraph that carries it starts.
    pub(super) fn record_pending_outline(&mut self) {
        if let Some((title, level)) = self.pending_outline.take() {
            self.outlines.push(OutlineEntry {
                title,
                level,
                page_index: self.page_number - 1,
                y_position: self.geometry.margin_top + self.cursor_y,
            });
        }
    }

    pub(super) fn record_body_fragment(
        &mut self,
        body_index: usize,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) {
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        // A fragment is a page-space rectangle that comment anchors and float
        // keep-out bands are resolved against outside layout. A vertical
        // section places blocks in the transposed band, so the rectangle is
        // mapped through the same rotation the painted band takes and
        // recorded as the axis-aligned box it covers on the page.
        let (x, y, width, height) = match self.body_rotation_transform() {
            Some(transform) => {
                let rect = transform.transform_rect_bbox(Rect {
                    x,
                    y,
                    width,
                    height,
                });
                (rect.x, rect.y, rect.width, rect.height)
            }
            None => (x, y, width, height),
        };
        if let Some((previous_body, previous)) = self.body_fragments.last_mut()
            && *previous_body == body_index
            && previous.physical_page == self.page_number
        {
            let right = (previous.x + previous.width).max(x + width);
            let bottom = (previous.y + previous.height).max(y + height);
            previous.x = previous.x.min(x);
            previous.y = previous.y.min(y);
            previous.width = right - previous.x;
            previous.height = bottom - previous.y;
            return;
        }
        self.body_fragments.push((
            body_index,
            WordBodyLayoutFragment {
                physical_page: self.page_number,
                displayed_page: self.header_page_number,
                x,
                y,
                width,
                height,
            },
        ));
    }
}

impl<'a> Pager<'a> {
    /// Make room for the block at `next_block_index`.
    ///
    /// Content that simply ran out of room flows into the next column track
    /// when the section has one, and ends the page only when it does not.
    pub(super) fn advance_flow_before(&mut self, next_block_index: usize) {
        if self.advance_column_track() {
            return;
        }
        self.finish_page_before(next_block_index);
    }

    /// Move the flow on for a keep-with-next chain, without a restart
    /// checkpoint. The blocks after the boundary chose it, so a restart there
    /// would keep it after an edit to those blocks no longer calls for it.
    pub(super) fn advance_flow_for_chain(&mut self) {
        if !self.advance_column_track() {
            self.finish_page_outright();
        }
    }

    /// End the page before `next_block_index`, whatever column the body is in.
    pub(super) fn finish_page_before(&mut self, next_block_index: usize) {
        self.finish_page_outright();
        if self.pending_notes.is_empty()
            && self.page_note_ids.is_empty()
            && self.page_wraps.is_empty()
            && self.resolved_out.is_empty()
        {
            self.checkpoints.push(PaginationCheckpoint {
                next_block_index,
                page_count: self.page_number - 1,
                next_header_page_number: self.header_page_number,
            });
            if self.stop_at == self.checkpoints.last().copied() {
                self.stopped_at = self.checkpoints.last().copied();
            }
        }
    }

    pub(super) fn flush(
        &mut self,
    ) -> (
        Vec<PageFrame>,
        Vec<OutlineEntry>,
        Option<PageFlowSpace>,
        Option<FurnitureFailure>,
    ) {
        // Always create at least one page
        if self.has_content() || self.pages.is_empty() {
            self.finish_page_outright();
        }
        // A note that ran past the last page of body text still has to land
        // somewhere, so keep making pages until the queue drains. Each page
        // places at least one note line, so this terminates.
        while !self.pending_notes.is_empty() {
            let before = self.pending_notes.clone();
            self.finish_page_outright();
            if self.pending_notes == before {
                // Every page places at least one note line, so this is
                // unreachable. It exists so a future change that breaks that
                // guarantee stops rather than spins, and the assertion makes
                // it loud in tests instead of silently losing note text.
                debug_assert!(
                    false,
                    "a page placed no note content, dropping {:?}",
                    self.pending_notes
                );
                break;
            }
        }
        let last_flow_space = self.page_flow_spaces.last().copied();
        (
            std::mem::take(&mut self.pages),
            std::mem::take(&mut self.outlines),
            last_flow_space,
            self.furniture_failure.take(),
        )
    }
}
