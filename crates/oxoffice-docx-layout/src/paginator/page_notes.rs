//! Notes on a page: the references lines and tables carry, room reserved for footnotes, and drawing notes.

use super::*;

/// Private source line ranges, shared by note prediction and publication.
struct NoteFragmentPlan {
    placed: Vec<(NoteRef, usize, usize, bool)>,
    carried: Vec<(NoteRef, usize)>,
    height: f64,
    accepts_fresh: bool,
}

impl Default for NoteFragmentPlan {
    fn default() -> Self {
        Self {
            placed: Vec::new(),
            carried: Vec::new(),
            height: 0.0,
            accepts_fresh: true,
        }
    }
}

impl<'a> Pager<'a> {
    /// Height the note area needs for a given set of notes, in full.
    ///
    /// Zero when there are none, so a page without notes keeps every point of
    /// its content height.
    /// One ordered plan owns both reference admission and committed splitting.
    /// Short fresh notes retain whole-note movement. Intrinsically long notes
    /// can start with a nonempty fragment beside their actual reference.
    fn note_fragments(
        &self,
        carried: &[(NoteRef, usize)],
        fresh: &[NoteRef],
        body_height: f64,
    ) -> NoteFragmentPlan {
        let mut plan = NoteFragmentPlan::default();
        if carried.is_empty() && fresh.is_empty() {
            return plan;
        }
        let record = if carried.is_empty() {
            NoteType::Separator
        } else {
            NoteType::ContinuationSeparator
        };
        let width = self.geometry.content_width();
        let separator = self
            .notes
            .get_special(NoteStream::Footnote, record, width)
            .map_or(0.0, |(note, _)| note.height());
        let notice = self
            .notes
            .get_special(NoteStream::Footnote, NoteType::ContinuationNotice, width)
            .map_or(0.0, |(note, _)| note.height());
        let overhead = NOTE_SEPARATOR_OFFSET + separator;
        let fresh_capacity = (self.content_height - overhead).max(0.0);
        let mut capacity = (fresh_capacity - body_height).max(0.0);
        let queue = carried
            .iter()
            .map(|(id, first)| (*id, *first, true))
            .chain(fresh.iter().map(|id| (*id, 0, false)))
            .collect::<Vec<_>>();
        let remaining = queue
            .iter()
            .filter_map(|(id, first, _)| {
                self.notes
                    .get(*id, width)
                    .map(|note| note.height_from(*first))
            })
            .sum::<f64>();
        let will_continue = remaining > capacity + 0.01;
        if will_continue {
            capacity = (capacity - notice).max(0.0);
        }
        let mut used = 0.0;
        for (id, first, continued) in queue {
            let Some(note) = self.notes.get(id, width) else {
                continue;
            };
            let mut count = 0;
            let short = !continued && note.height() <= fresh_capacity + 0.01;
            if plan.carried.is_empty() && (!short || used + note.height() <= capacity + 0.01) {
                for line in note.lines.iter().skip(first) {
                    if used + line.height > capacity + 0.01 {
                        // The usable area includes any continuation notice.
                        // A line taller than that area overflows once on a
                        // note-only page, never spins or drops the source line.
                        if body_height == 0.0
                            && count == 0
                            && used == 0.0
                            && line.height > capacity + 0.01
                        {
                            used += line.height;
                            count = 1;
                        }
                        break;
                    }
                    used += line.height;
                    count += 1;
                }
            }
            if count > 0 {
                plan.placed.push((id, first, count, continued));
            }
            if !continued && count == 0 && !note.lines.is_empty() {
                plan.accepts_fresh = false;
            }
            if first + count < note.lines.len() {
                plan.carried.push((id, first + count));
            }
        }
        if !plan.placed.is_empty() {
            plan.height = overhead + used + if plan.carried.is_empty() { 0.0 } else { notice };
        }
        plan
    }

    fn reserve_for(&self, carried: &[(NoteRef, usize)], fresh: &[NoteRef]) -> f64 {
        self.note_fragments(carried, fresh, self.ink_bottom).height
    }

    /// The note area currently committed for the page being built.
    pub(super) fn reserved_height(&self) -> f64 {
        self.reserve_for(&self.pending_notes, &self.page_note_ids)
    }

    /// Content height still usable by body text on this page.
    pub(super) fn available_height(&self) -> f64 {
        (self.content_height - self.reserved_height()).max(0.0)
    }

    /// What the note area would cost if `lines` were placed on this page,
    /// without committing to placing them.
    ///
    /// A paragraph is measured before anyone knows which page it lands on, so
    /// its notes must be priced without being claimed. Claiming first and
    /// moving the paragraph afterwards leaves the note stranded on the page
    /// before its own reference.
    pub(super) fn available_height_for(&self, lines: &[LayoutLine]) -> f64 {
        let mut fresh = self.page_note_ids.clone();
        for line in lines {
            for id in page_foot_notes_in_line(line) {
                if self.notes.get(id, self.geometry.content_width()).is_some()
                    && !fresh.contains(&id)
                    && !self.pending_notes.iter().any(|(pending, _)| *pending == id)
                {
                    fresh.push(id);
                }
            }
        }
        let body_height = self.cursor_y + lines.iter().map(|line| line.height).sum::<f64>();
        let plan = self.note_fragments(&self.pending_notes, &fresh, body_height);
        if plan.accepts_fresh {
            (self.content_height - plan.height).max(0.0)
        } else {
            0.0
        }
    }

    /// Record the footnotes referenced by lines about to be placed.
    ///
    /// Endnotes are ignored here. They are emitted at the document end, so
    /// they cost the page carrying their reference nothing.
    pub(super) fn claim_notes(&mut self, lines: &[LayoutLine]) {
        for id in lines.iter().flat_map(page_foot_notes_in_line) {
            {
                if self.notes.get(id, self.geometry.content_width()).is_some()
                    && !self.page_note_ids.contains(&id)
                    && !self.pending_notes.iter().any(|(pending, _)| *pending == id)
                {
                    self.page_note_ids.push(id);
                }
            }
        }
    }

    /// How many of `lines` fit, once the note area their references demand is
    /// taken out of the page.
    ///
    /// A line is admitted only if the whole note area still fits after it, so
    /// a note is not split merely because body text was greedy. The one
    /// exception is a page that has placed nothing yet: there the line goes
    /// down regardless and the note splits, because a page that admits neither
    /// body nor note makes no progress and pagination would not terminate.
    pub(super) fn count_lines_that_fit_with_notes(
        &self,
        lines: &[LayoutLine],
        start_y: f64,
    ) -> usize {
        let mut fresh = self.page_note_ids.clone();
        let mut used = 0.0;

        for (index, line) in lines.iter().enumerate() {
            for id in page_foot_notes_in_line(line) {
                if self.notes.get(id, self.geometry.content_width()).is_some()
                    && !fresh.contains(&id)
                    && !self.pending_notes.iter().any(|(pending, _)| *pending == id)
                {
                    fresh.push(id);
                }
            }

            let body_height = start_y + used + line.height;
            let plan = self.note_fragments(&self.pending_notes, &fresh, body_height);
            if !plan.accepts_fresh || body_height > self.content_height - plan.height + 0.01 {
                let page_is_empty = !self.has_content() && used == 0.0 && index == 0;
                if !page_is_empty {
                    return index;
                }
            }
            used += line.height;
        }

        lines.len()
    }
}

impl<'a> Pager<'a> {
    fn accept_note(&mut self, result: oxoffice_layout::Result<(f64, PaintLayers)>) -> f64 {
        match result {
            Ok((height, layers)) => {
                self.accept_layers(Ok(layers));
                height
            }
            Err(error) => {
                self.pending_furniture_failure
                    .get_or_insert(error.to_string());
                0.0
            }
        }
    }
}

impl<'a> Pager<'a> {
    /// Draw the note area for the page being built, and carry what did not
    /// fit onto the next one.
    ///
    /// Notes sit above the bottom margin and grow upward, so the body text
    /// above them was already kept clear by `available_height`.
    pub(super) fn place_page_notes(&mut self) {
        let plan = self.note_fragments(&self.pending_notes, &self.page_note_ids, self.ink_bottom);
        let opens_with_continuation = !self.pending_notes.is_empty();
        self.pending_notes = plan.carried;
        self.page_note_ids.clear();
        let placed = plan.placed;
        if placed.is_empty() {
            return;
        }
        let special_record = if opens_with_continuation {
            NoteType::ContinuationSeparator
        } else {
            NoteType::Separator
        };
        let authored_separator = self.notes.get_special(
            NoteStream::Footnote,
            special_record,
            self.geometry.content_width(),
        );
        let separator_height = authored_separator.map_or(0.0, |(note, _)| note.height());
        let continuation_notice = self.notes.get_special(
            NoteStream::Footnote,
            NoteType::ContinuationNotice,
            self.geometry.content_width(),
        );
        let total = placed
            .iter()
            .filter_map(|(id, first, count, _)| {
                self.notes
                    .get(*id, self.geometry.content_width())
                    .map(|note| note.height_of(*first, *count))
            })
            .sum::<f64>();

        let separator_y = if self.footnotes_beneath_text {
            self.geometry.margin_top + self.ink_bottom
        } else {
            self.geometry.page_height
                - self.geometry.margin_bottom
                - total
                - NOTE_SEPARATOR_OFFSET
                - separator_height
                - if self.pending_notes.is_empty() {
                    0.0
                } else {
                    continuation_notice.map_or(0.0, |(note, _)| note.height())
                }
        };

        // A page opening with carried content gets the full-width rule, which
        // is how Word says "this continues from the previous page". A document
        // that never defined one keeps the short rule.
        let separator_width = if opens_with_continuation
            && self.notes.has_continuation_separator(NoteStream::Footnote)
        {
            self.geometry.content_width()
        } else {
            self.geometry.content_width() * SEPARATOR_WIDTH_FRACTION
        };

        if let Some((note, render)) = authored_separator {
            self.accept_note(draw_note(
                &self.geometry,
                note,
                render,
                0,
                note.lines.len(),
                false,
                separator_y,
                self.page_number,
            ));
        } else {
            self.elements.push(PositionedElement::Line {
                start: Point {
                    x: self.geometry.margin_left,
                    y: separator_y,
                },
                end: Point {
                    x: self.geometry.margin_left + separator_width,
                    y: separator_y,
                },
                width: 0.5,
                color: Color::BLACK,
                dash_pattern: None,
            });
        }

        let mut cursor_y = separator_y + separator_height + NOTE_SEPARATOR_OFFSET;
        for (id, first, count, continued) in placed {
            let Some((note, render)) = self.notes.get_render(id, self.geometry.content_width())
            else {
                continue;
            };
            cursor_y += self.accept_note(draw_note(
                &self.geometry,
                note,
                render,
                first,
                count,
                continued,
                cursor_y,
                self.page_number,
            ));
        }
        if !self.pending_notes.is_empty()
            && let Some((note, render)) = continuation_notice
        {
            self.accept_note(draw_note(
                &self.geometry,
                note,
                render,
                0,
                note.lines.len(),
                false,
                cursor_y,
                self.page_number,
            ));
        }
    }
}

/// Draw one note, or one slice of one, with its top edge at `top`.
///
/// Returns the height consumed. Shared by the page foot and the document end
/// so the two regions cannot drift apart in how a note looks.
pub(super) fn draw_note(
    geometry: &PageGeometry,
    note: &NoteLayout,
    render: &[NoteRenderParagraph],
    first: usize,
    count: usize,
    continued: bool,
    top: f64,
    page_number: usize,
) -> oxoffice_layout::Result<(f64, PaintLayers)> {
    let mut elements = Vec::new();
    let mut layers = PaintLayers::default();
    let baseline = top + note.lines.get(first).map_or(0.0, |line| line.ascent);

    // A continuation does not repeat the marker.
    if !continued && note.draw_marker {
        elements.push(PositionedElement::Text(GlyphRun {
            origin: Point {
                x: geometry.margin_left,
                y: baseline - note.marker_rise,
            },
            font_id: note.marker.font_id,
            font_size: note.marker.font_size,
            glyph_ids: note.marker.glyph_ids.clone(),
            advances: note.marker.advances.clone(),
            text: note.marker.text.clone(),
            source: None,
            color: note.marker.color,
            bold: note.marker.bold,
            italic: note.marker.italic,
            field_kind: None,
            field_source: None,
            note: None,
            note_reference_source: None,
            tab_aligned: None,
        }));
    }

    layers
        .regular
        .append(&mut PaintedElements::non_field(std::mem::take(
            &mut elements,
        ))?)?;
    let mut cursor_y = top;
    let selected = first..first.saturating_add(count);
    let note_geometry = PageGeometry {
        margin_top: 0.0,
        margin_left: geometry.margin_left,
        ..geometry.without_columns()
    };
    let empty_media = HashMap::new();
    for paragraph in render {
        let start = paragraph.lines.start.max(selected.start);
        let end = paragraph.lines.end.min(selected.end);
        if start >= end {
            continue;
        }
        let local_start = start - paragraph.lines.start;
        let local_end = end - paragraph.lines.start;
        let lines = &paragraph.block.lines[local_start..local_end];
        let gap = if local_start == 0 {
            paragraph.border_gap_before
        } else {
            0.0
        };
        let mut content_lines;
        let painted_lines = if gap > 0.0 {
            content_lines = lines.to_vec();
            content_lines[0].height -= gap;
            content_lines[0].ascent -= gap;
            &content_lines
        } else {
            lines
        };
        layers.append(&mut render_paragraph_lines(
            painted_lines,
            ParagraphView {
                line_start: local_start,
                paint: Some(&paragraph.paint),
                baseline_policy: None,
                line_gaps: &[],
                border_group_before: false,
                block: &paragraph.block,
                semantics: None,
                reflow_direction: paragraph.direction,
                reflow_allowed: false,
            },
            &note_geometry,
            cursor_y + gap,
            (
                local_start == 0,
                local_end == paragraph.block.lines.len(),
                if start > selected.start { gap } else { 0.0 },
            ),
            &empty_media,
        )?)?;
        cursor_y += lines.iter().map(|line| line.height).sum::<f64>();
    }

    for (range, color) in &note.revision_ranges {
        let visible_start = range.start.max(first);
        let visible_end = range.end.min(first + count);
        if visible_start >= visible_end {
            continue;
        }
        let offset = note
            .lines
            .iter()
            .skip(first)
            .take(visible_start - first)
            .map(|line| line.height)
            .sum::<f64>();
        let height = note
            .lines
            .iter()
            .skip(visible_start)
            .take(visible_end - visible_start)
            .map(|line| line.height)
            .sum::<f64>();
        render_change_bar_at(
            top + offset,
            height,
            *color,
            geometry,
            page_number,
            &mut elements,
        );
    }

    layers
        .regular
        .append(&mut PaintedElements::non_field(elements)?)?;
    Ok((cursor_y - top, layers))
}

pub(super) fn table_note_references(table: &crate::table::TableBlock) -> Vec<NoteRef> {
    let mut references = Vec::new();
    for row in &table.rows {
        for cell in &row.cells {
            for block in &cell.blocks {
                match block {
                    crate::table::CellBlock::Paragraph(paragraph) => {
                        references.extend(paragraph.lines.iter().flat_map(notes_in_line))
                    }
                    crate::table::CellBlock::Table(table) => {
                        references.extend(table_note_references(table))
                    }
                }
            }
        }
    }
    references
}

/// The notes referenced by the segments on one line.
pub(super) fn notes_in_line(line: &LayoutLine) -> impl Iterator<Item = NoteRef> + '_ {
    line.items.iter().filter_map(|item| match item {
        LineItem::Text(seg) | LineItem::Marker(seg) => seg.note,
        LineItem::MultilingualText(seg) => seg.base().note,
        _ => None,
    })
}

/// The notes on one line that belong at the foot of its page.
///
/// Endnotes are excluded. They are emitted at the document end and take no
/// height from the page their reference sits on.
pub(super) fn page_foot_notes_in_line(line: &LayoutLine) -> impl Iterator<Item = NoteRef> + '_ {
    notes_in_line(line).filter(|note| note.stream == NoteStream::Footnote)
}
