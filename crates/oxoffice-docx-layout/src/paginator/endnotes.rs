//! Endnote pages: collecting the references and paginating the notes at a section's or the document's end.

use super::*;

/// Append the document's endnotes as pages after the last body page.
///
/// Final section source for fresh endnote pages. Reused body pages already own
/// their prepared furniture. Continuations use the same Pager preparation and
/// paint authority as body/footnote pages, with continued physical/display ids.
pub(crate) struct EndnoteFurniture<'a> {
    pub section: &'a SharedSection,
    pub section_index: usize,
    pub fm: &'a FontManager,
    pub media: &'a HashMap<MediaId, ImageData>,
}

impl<'a> EndnoteFurniture<'a> {
    fn page(
        &self,
        page: usize,
        displayed: usize,
        notes: &'a NoteRegistry,
        resolved: &'a ResolvedWraps,
    ) -> Result<Pager<'a>, FurnitureFailure> {
        // Endnotes retain single-column upright top flow. Establish that
        // context before Pager derives its effective band and transforms.
        let mut geometry = self.section.geometry.without_columns();
        geometry.vertical_alignment = None;
        geometry.body_rotation = None;
        let mut pager = Pager::new(
            geometry,
            self.section.header_footer.as_ref(),
            self.section.header_footer_semantics.as_ref(),
            self.section.title_pg,
            self.section.footnotes_beneath_text,
            false,
            self.section.line_number_style,
            self.media,
            notes,
            self.fm,
            resolved,
            page,
            displayed,
            false,
            None,
        );
        if let Some(reason) = pager.pending_furniture_failure.take() {
            return Err(FurnitureFailure {
                section: self.section_index,
                page,
                reason,
            });
        }
        Ok(pager)
    }
}

/// Endnotes are flow content read at the end, not marginalia, so they start at
/// the top of a fresh page and carry no separator rule. There is no body text
/// on these pages for a rule to divide them from.
pub fn append_endnote_pages(
    pages: &mut Vec<PageFrame>,
    notes: &NoteRegistry,
    geometry: PageGeometry,
) {
    // First-reference order across the document, which is the order a reader
    // met them in.
    let mut ordered: Vec<NoteRef> = Vec::new();
    for page in pages.iter() {
        oxoffice_layout::walk(&page.elements, &mut |element, _| {
            let note = match element {
                PositionedElement::Text(run) => run.note,
                PositionedElement::MultilingualText(run) => run.note,
                _ => None,
            };
            if let Some(note) = note
                && note.stream == NoteStream::Endnote
                && notes.get(note, geometry.content_width()).is_some()
                && !ordered.contains(&note)
            {
                ordered.push(note);
            }
        });
    }

    if let Ok(mut paint) = PagePaintRecords::neutral(pages) {
        let _ = append_ordered_endnote_pages(
            pages, &mut paint, &ordered, notes, geometry, 0, 1, None, None,
        );
    }
}

/// Flow document-end endnotes into the measured free band of the final body page.
pub(crate) fn append_endnote_pages_at_document_end(
    pages: &mut Vec<PageFrame>,
    paint: &mut PagePaintRecords,
    notes: &NoteRegistry,
    geometry: PageGeometry,
    flow_space: Option<PageFlowSpace>,
    excluded: &[NoteRef],
    furniture: Option<&EndnoteFurniture<'_>>,
) -> Result<(), FurnitureFailure> {
    let ordered = collect_endnote_refs(pages, notes, geometry.content_width())
        .into_iter()
        .filter(|reference| !excluded.contains(reference))
        .collect::<Vec<_>>();
    append_ordered_endnote_pages(
        pages, paint, &ordered, notes, geometry, 0, 1, flow_space, furniture,
    )?;
    Ok(())
}

pub(super) fn collect_endnote_refs(
    pages: &[PageFrame],
    notes: &NoteRegistry,
    width: f64,
) -> Vec<NoteRef> {
    let mut ordered = Vec::new();
    for page in pages {
        oxoffice_layout::walk(&page.elements, &mut |element, _| {
            let note = match element {
                PositionedElement::Text(run) => run.note,
                PositionedElement::MultilingualText(run) => run.note,
                _ => None,
            };
            if let Some(note) = note
                && note.stream == NoteStream::Endnote
                && notes.get(note, width).is_some()
                && !ordered.contains(&note)
            {
                ordered.push(note);
            }
        });
    }
    ordered
}

pub(crate) fn append_endnote_pages_for_references(
    pages: &mut Vec<PageFrame>,
    paint: &mut PagePaintRecords,
    references: &[NoteRef],
    notes: &NoteRegistry,
    geometry: PageGeometry,
    preceding_page_count: usize,
    next_displayed_page_number: usize,
    flow_space: Option<PageFlowSpace>,
    furniture: Option<&EndnoteFurniture<'_>>,
) -> Result<(), FurnitureFailure> {
    let mut ordered = Vec::new();
    for &note in references {
        if note.stream == NoteStream::Endnote
            && notes.get(note, geometry.content_width()).is_some()
            && !ordered.contains(&note)
        {
            ordered.push(note);
        }
    }

    append_ordered_endnote_pages(
        pages,
        paint,
        &ordered,
        notes,
        geometry,
        preceding_page_count,
        next_displayed_page_number,
        flow_space,
        furniture,
    )?;
    Ok(())
}

fn endnote_paint_failure(
    error: oxoffice_layout::LayoutError,
    furniture: Option<&EndnoteFurniture<'_>>,
    page: usize,
) -> FurnitureFailure {
    FurnitureFailure {
        section: furniture.map_or(0, |owner| owner.section_index),
        page,
        reason: error.to_string(),
    }
}

pub(super) fn append_ordered_endnote_pages(
    pages: &mut Vec<PageFrame>,
    paint: &mut PagePaintRecords,
    ordered: &[NoteRef],
    notes: &NoteRegistry,
    mut geometry: PageGeometry,
    preceding_page_count: usize,
    next_displayed_page_number: usize,
    flow_space: Option<PageFlowSpace>,
    furniture: Option<&EndnoteFurniture<'_>>,
) -> Result<Option<PageFlowSpace>, FurnitureFailure> {
    paint
        .validate(pages)
        .map_err(|error| endnote_paint_failure(error, furniture, preceding_page_count + 1))?;
    if ordered.is_empty() {
        return Ok(flow_space);
    }

    let authored_separator = notes.get_special(
        NoteStream::Endnote,
        NoteType::Separator,
        geometry.content_width(),
    );
    let separator_height = authored_separator.map_or(0.0, |(note, _)| note.height());

    let reuse_last = flow_space.is_some_and(|space| {
        pages.last().is_some()
            && matches!(geometry.vertical_alignment, None | Some(ST_VerticalJc::Top))
            && geometry.body_rotation.is_none()
            && geometry.columns.is_empty()
            && ordered
                .first()
                .and_then(|note| notes.get(*note, geometry.content_width()))
                .and_then(|note| note.lines.first())
                .is_some_and(|line| {
                    space.body_bottom + NOTE_SEPARATOR_OFFSET * 2.0 + separator_height + line.height
                        <= space.note_top
                })
    });
    let mut page_number = pages.last().map_or(preceding_page_count + 1, |page| {
        page.page_number + usize::from(!reuse_last)
    });
    let mut displayed_page_number = pages.last().map_or(next_displayed_page_number, |page| {
        page.displayed_page_number
            .saturating_add(usize::from(!reuse_last))
    });
    let authored_margin_top = geometry.margin_top;
    let resolved = ResolvedWraps::new();
    let mut current_pager = if reuse_last {
        None
    } else {
        furniture
            .map(|context| context.page(page_number, displayed_page_number, notes, &resolved))
            .transpose()?
    };
    if let Some(pager) = &current_pager {
        geometry = pager.geometry.clone();
    }
    let mut content_height = if reuse_last {
        flow_space.map_or(geometry.content_height(), |space| space.note_top)
    } else {
        geometry.content_height()
    };
    let mut painted_notes = PaintLayers::default();
    let mut cursor_y = if reuse_last {
        flow_space.map_or(0.0, |space| space.body_bottom + NOTE_SEPARATOR_OFFSET * 2.0)
    } else {
        0.0
    };
    if reuse_last {
        if let Some((note, render)) = authored_separator {
            painted_notes
                .accept_note(draw_note(
                    &geometry,
                    note,
                    render,
                    0,
                    note.lines.len(),
                    false,
                    geometry.margin_top + cursor_y - NOTE_SEPARATOR_OFFSET,
                    pages
                        .last()
                        .map_or(preceding_page_count + 1, |page| page.page_number),
                ))
                .map_err(|error| endnote_paint_failure(error, furniture, page_number))?;
            cursor_y += separator_height;
        } else {
            painted_notes
                .regular
                .push_non_field(PositionedElement::Line {
                    start: Point {
                        x: geometry.margin_left,
                        y: geometry.margin_top + cursor_y - NOTE_SEPARATOR_OFFSET,
                    },
                    end: Point {
                        x: geometry.margin_left + geometry.content_width() * 0.33,
                        y: geometry.margin_top + cursor_y - NOTE_SEPARATOR_OFFSET,
                    },
                    width: 0.5,
                    color: Color::BLACK,
                    dash_pattern: None,
                })
                .map_err(|error| endnote_paint_failure(error, furniture, page_number))?;
        }
    } else if let Some((note, render)) = authored_separator
        && ordered
            .first()
            .and_then(|reference| notes.get(*reference, geometry.content_width()))
            .and_then(|first| first.lines.first())
            .is_some_and(|line| {
                note.height() + NOTE_SEPARATOR_OFFSET + line.height <= content_height
            })
    {
        painted_notes
            .accept_note(draw_note(
                &geometry,
                note,
                render,
                0,
                note.lines.len(),
                false,
                geometry.margin_top,
                pages
                    .last()
                    .map_or(preceding_page_count + 1, |page| page.page_number + 1),
            ))
            .map_err(|error| endnote_paint_failure(error, furniture, page_number))?;
        cursor_y += separator_height + NOTE_SEPARATOR_OFFSET;
    }
    let mut reuse_pending = reuse_last;

    let page_width = geometry.page_width;
    let page_height = geometry.page_height;
    let mut flush = |layers: &mut PaintLayers,
                     page_number: &mut usize,
                     displayed_page_number: &mut usize,
                     current_pager: &mut Option<Pager<'_>>|
     -> Result<(), FurnitureFailure> {
        let fail = |error| endnote_paint_failure(error, furniture, *page_number);
        if reuse_pending {
            paint
                .append_late(pages, std::mem::take(layers))
                .map_err(fail)?;
            reuse_pending = false;
        } else if let Some(mut pager) = current_pager.take() {
            pager.accept_layers(Ok(std::mem::take(layers)));
            pager.has_content_flag = true;
            pager.finish_page_outright();
            if let Some(failure) = pager.furniture_failure.take() {
                return Err(failure);
            }
            if pager.pages.len() != 1
                || pager.page_policies.len() != 1
                || pager.page_boundaries.len() != 1
            {
                return Err(fail(oxoffice_layout::LayoutError::Layout(
                    "endnote page paint owner count mismatch".into(),
                )));
            }
            pages.append(&mut pager.pages);
            paint.policies.append(&mut pager.page_policies);
            paint.boundaries.append(&mut pager.page_boundaries);
        } else {
            let layers = std::mem::take(layers);
            let behind_end = layers.behind.len();
            let regular_end = behind_end
                .checked_add(layers.regular.len())
                .ok_or_else(|| {
                    fail(oxoffice_layout::LayoutError::Layout(
                        "endnote paint overflow".into(),
                    ))
                })?;
            let painted = layers.flatten().map_err(fail)?;
            let len = painted.len();
            let (elements, policies) = painted.into_parts().map_err(fail)?;
            let mut page = PageFrame::new(*page_number, page_width, page_height, elements);
            page.displayed_page_number = *displayed_page_number;
            pages.push(page);
            paint.policies.push(policies);
            paint.boundaries.push(PagePaintBoundary {
                behind_end,
                regular_end,
                body_front_end: len,
                furniture_front_end: len,
                elements_len: len,
            });
        }
        *page_number += 1;
        *displayed_page_number = displayed_page_number.saturating_add(1);
        Ok(())
    };

    for &note_ref in ordered {
        let Some((note, render)) = notes.get_render(note_ref, geometry.content_width()) else {
            continue;
        };

        let mut first = 0;
        let mut continued = false;
        while first < note.lines.len() {
            let notice = notes.get_special(
                NoteStream::Endnote,
                NoteType::ContinuationNotice,
                geometry.content_width(),
            );
            let notice_height = notice.map_or(0.0, |(special, _)| special.height());
            let notice_reserve = if note.height_from(first) > content_height - cursor_y + 0.01 {
                notice_height
            } else {
                0.0
            };
            let mut count = 0;
            let mut used = cursor_y;
            for line in note.lines.iter().skip(first) {
                if used + line.height + notice_reserve > content_height + 0.01 {
                    break;
                }
                used += line.height;
                count += 1;
            }

            if count == 0 {
                // Nothing more fits on this page. Start a fresh one, unless
                // the page is already empty. An empty page that still cannot
                // take a line means the line is taller than the page, so it is
                // placed and allowed to overflow rather than looping forever.
                // Overflowing beats dropping the text, and body text on a page
                // of its own behaves the same way.
                if cursor_y == 0.0 {
                    count = 1;
                } else {
                    flush(
                        &mut painted_notes,
                        &mut page_number,
                        &mut displayed_page_number,
                        &mut current_pager,
                    )?;
                    cursor_y = 0.0;
                    current_pager = furniture
                        .map(|context| {
                            context.page(page_number, displayed_page_number, notes, &resolved)
                        })
                        .transpose()?;
                    if let Some(pager) = &current_pager {
                        geometry = pager.geometry.clone();
                    }
                    content_height = geometry.content_height();
                    if let Some((special, special_render)) = notes.get_special(
                        NoteStream::Endnote,
                        NoteType::ContinuationSeparator,
                        geometry.content_width(),
                    ) && note.lines.get(first).is_some_and(|line| {
                        let available_after_separator =
                            content_height - special.height() - NOTE_SEPARATOR_OFFSET;
                        let notice_height =
                            if note.height_from(first) > available_after_separator + 0.01 {
                                notice.map_or(0.0, |(record, _)| record.height())
                            } else {
                                0.0
                            };
                        line.height + notice_height <= available_after_separator + 0.01
                    }) {
                        painted_notes
                            .accept_note(draw_note(
                                &geometry,
                                special,
                                special_render,
                                0,
                                special.lines.len(),
                                false,
                                geometry.margin_top,
                                page_number,
                            ))
                            .map_err(|error| {
                                endnote_paint_failure(error, furniture, page_number)
                            })?;
                        cursor_y += special.height() + NOTE_SEPARATOR_OFFSET;
                    }
                    continue;
                }
            }

            cursor_y += painted_notes
                .accept_note(draw_note(
                    &geometry,
                    note,
                    render,
                    first,
                    count,
                    continued,
                    geometry.margin_top + cursor_y,
                    page_number,
                ))
                .map_err(|error| endnote_paint_failure(error, furniture, page_number))?;
            if first + count < note.lines.len()
                && let Some((special, special_render)) = notice
                && cursor_y + special.height() <= content_height + 0.01
            {
                cursor_y += painted_notes
                    .accept_note(draw_note(
                        &geometry,
                        special,
                        special_render,
                        0,
                        special.lines.len(),
                        false,
                        geometry.margin_top + cursor_y,
                        page_number,
                    ))
                    .map_err(|error| endnote_paint_failure(error, furniture, page_number))?;
            }
            first += count;
            continued = true;
        }
    }

    if painted_notes.behind.len() + painted_notes.regular.len() + painted_notes.front.len() > 0 {
        flush(
            &mut painted_notes,
            &mut page_number,
            &mut displayed_page_number,
            &mut current_pager,
        )?;
    }
    let offset = geometry.margin_top - authored_margin_top;
    Ok(Some(PageFlowSpace {
        body_bottom: cursor_y + offset,
        note_top: content_height + offset,
    }))
}
