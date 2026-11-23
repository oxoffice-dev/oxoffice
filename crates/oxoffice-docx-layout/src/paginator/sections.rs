//! Sections: pagination across section breaks, continuous sections and recorded checkpoints.

use super::*;

/// A section with its blocks, geometry, and header/footer content.
pub struct Section {
    pub blocks: Vec<LayoutBlock>,
    pub geometry: PageGeometry,
    pub header_footer: Option<HeaderFooterContent>,
    /// Whether this section uses a different first page header/footer.
    pub title_pg: bool,
    /// Displayed page number assigned to the first page of this section.
    pub page_number_start: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WordLineNumberStyle {
    pub font_id: Option<oxoffice_layout::FontId>,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    pub kerning: bool,
}

/// The parity of the page an odd-page or even-page section starts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SectionParity {
    Odd,
    Even,
}

pub(crate) struct SharedSection {
    pub line_number_style: Option<WordLineNumberStyle>,
    pub written_tb_rl_band: bool,
    pub modern_footnote_layout: bool,
    pub footnote_layout_like_word8: bool,
    pub continuous: bool,
    /// Word's `w:evenAndOddHeaders`: a section starting on a new page whose
    /// first displayed number has the parity of the page before it starts
    /// after an empty page, so odd numbers stay on right-hand pages.
    pub odd_numbers_on_right_pages: bool,
    pub parity: Option<SectionParity>,
    pub blocks: Vec<SharedLayoutBlock>,
    pub geometry: PageGeometry,
    pub header_footer: Option<HeaderFooterContent>,
    pub header_footer_semantics: Option<HeaderFooterSemantics>,
    pub title_pg: bool,
    pub page_number_start: Option<usize>,
    pub endnotes_at_section_end: bool,
    pub footnotes_beneath_text: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PaginationCheckpoint {
    pub next_block_index: usize,
    pub page_count: usize,
    pub next_header_page_number: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct FurnitureFailure {
    pub section: usize,
    pub page: usize,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PagePaintBoundary {
    pub behind_end: usize,
    pub regular_end: usize,
    pub body_front_end: usize,
    pub furniture_front_end: usize,
    pub elements_len: usize,
}

impl PagePaintBoundary {
    pub(crate) fn validate(self, elements_len: usize) -> oxoffice_layout::Result<()> {
        if self.behind_end > self.regular_end
            || self.regular_end > self.body_front_end
            || self.body_front_end > self.furniture_front_end
            || self.furniture_front_end > self.elements_len
            || self.elements_len != elements_len
        {
            return Err(oxoffice_layout::LayoutError::Layout(
                "inconsistent physical page paint boundary".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PagePaintRecords {
    pub policies: Vec<PaintPolicies>,
    pub boundaries: Vec<PagePaintBoundary>,
}

impl PagePaintRecords {
    pub(crate) fn validate(&self, pages: &[PageFrame]) -> oxoffice_layout::Result<()> {
        if self.policies.len() != pages.len() || self.boundaries.len() != pages.len() {
            return Err(oxoffice_layout::LayoutError::Layout(
                "page paint companion count mismatch".into(),
            ));
        }
        for ((page, policies), boundary) in pages.iter().zip(&self.policies).zip(&self.boundaries) {
            policies.validate(&page.elements)?;
            boundary.validate(page.elements.len())?;
        }
        Ok(())
    }

    pub(super) fn neutral(pages: &[PageFrame]) -> oxoffice_layout::Result<Self> {
        let mut records = Self::default();
        for page in pages {
            let (_, policies) = PaintedElements::neutral(page.elements.clone())?.into_parts()?;
            let len = page.elements.len();
            records.policies.push(policies);
            records.boundaries.push(PagePaintBoundary {
                behind_end: 0,
                regular_end: len,
                body_front_end: len,
                furniture_front_end: len,
                elements_len: len,
            });
        }
        Ok(records)
    }

    pub(super) fn append_late(
        &mut self,
        pages: &mut [PageFrame],
        mut layers: PaintLayers,
    ) -> oxoffice_layout::Result<()> {
        self.validate(pages)?;
        let index = pages.len().checked_sub(1).ok_or_else(|| {
            oxoffice_layout::LayoutError::Layout("late notes have no owning page".into())
        })?;
        let mut boundary = self.boundaries[index];
        let mut painted = PaintedElements::from_parts(
            std::mem::take(&mut pages[index].elements),
            std::mem::take(&mut self.policies[index]),
        )?;
        let count = layers.behind.len();
        painted.splice_at(
            &[],
            boundary.behind_end..boundary.behind_end,
            &mut layers.behind,
        )?;
        boundary.behind_end = boundary.behind_end.checked_add(count).ok_or_else(|| {
            oxoffice_layout::LayoutError::Layout("late note paint overflow".into())
        })?;
        boundary.regular_end = boundary.regular_end.checked_add(count).ok_or_else(|| {
            oxoffice_layout::LayoutError::Layout("late note paint overflow".into())
        })?;
        boundary.body_front_end = boundary.body_front_end.checked_add(count).ok_or_else(|| {
            oxoffice_layout::LayoutError::Layout("late note paint overflow".into())
        })?;
        boundary.furniture_front_end =
            boundary
                .furniture_front_end
                .checked_add(count)
                .ok_or_else(|| {
                    oxoffice_layout::LayoutError::Layout("late note paint overflow".into())
                })?;
        let count = layers.regular.len();
        painted.splice_at(
            &[],
            boundary.regular_end..boundary.regular_end,
            &mut layers.regular,
        )?;
        boundary.regular_end = boundary.regular_end.checked_add(count).ok_or_else(|| {
            oxoffice_layout::LayoutError::Layout("late note paint overflow".into())
        })?;
        boundary.body_front_end = boundary.body_front_end.checked_add(count).ok_or_else(|| {
            oxoffice_layout::LayoutError::Layout("late note paint overflow".into())
        })?;
        boundary.furniture_front_end =
            boundary
                .furniture_front_end
                .checked_add(count)
                .ok_or_else(|| {
                    oxoffice_layout::LayoutError::Layout("late note paint overflow".into())
                })?;
        let count = layers.front.len();
        painted.splice_at(
            &[],
            boundary.body_front_end..boundary.body_front_end,
            &mut layers.front,
        )?;
        boundary.body_front_end = boundary.body_front_end.checked_add(count).ok_or_else(|| {
            oxoffice_layout::LayoutError::Layout("late note paint overflow".into())
        })?;
        boundary.furniture_front_end =
            boundary
                .furniture_front_end
                .checked_add(count)
                .ok_or_else(|| {
                    oxoffice_layout::LayoutError::Layout("late note paint overflow".into())
                })?;
        boundary.elements_len = painted.len();
        boundary.validate(painted.len())?;
        (pages[index].elements, self.policies[index]) = painted.into_parts()?;
        self.boundaries[index] = boundary;
        Ok(())
    }
}

pub(crate) struct RecordedPagination {
    pub paint: PagePaintRecords,
    pub furniture_failure: Option<FurnitureFailure>,
    pub pages: Vec<PageFrame>,
    pub outlines: Vec<OutlineEntry>,
    pub body_fragments: Vec<(usize, WordBodyLayoutFragment)>,
    pub checkpoints: Vec<PaginationCheckpoint>,
    pub stopped_at: Option<PaginationCheckpoint>,
    pub last_flow_space: Option<PageFlowSpace>,
}

/// Remaining vertical body band after the last body mark and before footnotes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PageFlowSpace {
    pub body_bottom: f64,
    pub note_top: f64,
}

pub(crate) struct SharedPagination {
    pub paint: PagePaintRecords,
    pub furniture_failure: Option<FurnitureFailure>,
    pub page_sections: Vec<crate::WordPageSection>,
    pub pages: Vec<PageFrame>,
    pub outlines: Vec<OutlineEntry>,
    pub body_fragments: Vec<(usize, WordBodyLayoutFragment)>,
    pub last_flow_space: Option<PageFlowSpace>,
    pub endnotes_placed: Vec<NoteRef>,
    /// The indices of the blank pages added before odd-page and even-page
    /// sections, which nothing is painted on.
    pub blank_pages: Vec<usize>,
}

/// Paginate across multiple sections, each with its own geometry and header/footer.
pub fn paginate_sections(
    sections: &[Section],
    fm: &FontManager,
    media: &MediaRegistry,
    notes: &NoteRegistry,
) -> (Vec<PageFrame>, Vec<OutlineEntry>) {
    let media = media.media();
    if sections.is_empty() {
        return (
            vec![PageFrame::new(1, 612.0, 792.0, Vec::new())],
            Vec::new(),
        );
    }

    // For a single section, delegate to the existing paginate function
    if sections.len() == 1 {
        let s = &sections[0];
        return paginate_with_media(
            &s.blocks,
            s.geometry.clone(),
            s.header_footer.as_ref(),
            None,
            s.title_pg,
            fm,
            media,
            notes,
            1,
            s.page_number_start.unwrap_or(1),
        );
    }

    // Multi-section pagination
    let mut all_pages = Vec::new();
    let mut all_outlines = Vec::new();
    let mut page_offset = 0;
    let mut next_section_page_number = 1usize;

    for section in sections {
        let section_page_number = section
            .page_number_start
            .unwrap_or(next_section_page_number);
        let (mut pages, mut outlines) = paginate_with_media(
            &section.blocks,
            section.geometry.clone(),
            section.header_footer.as_ref(),
            None,
            section.title_pg,
            fm,
            media,
            notes,
            page_offset + 1,
            section_page_number,
        );

        next_section_page_number = section_page_number.saturating_add(pages.len());
        page_offset += pages.len();
        all_pages.append(&mut pages);
        all_outlines.append(&mut outlines);
    }

    // If a section produced no pages (empty blocks), we might have duplicates
    // Renumber pages sequentially
    for (i, page) in all_pages.iter_mut().enumerate() {
        page.page_number = i + 1;
    }

    (all_pages, all_outlines)
}

/// Whether a line draws nothing: no text, no drawing, no break. An empty
/// paragraph mark can be a multilingual segment, when the paragraph states a
/// language or a script, as Word writes it.
fn line_draws_nothing(line: &LayoutLine) -> bool {
    line.forced_break_after.is_none()
        && line.items.iter().all(|item| match item {
            LineItem::Text(segment) | LineItem::Marker(segment) => segment.text.is_empty(),
            LineItem::MultilingualText(segment) => segment.text().is_empty(),
            _ => false,
        })
}

/// Whether a paragraph draws nothing: empty lines, and no drawing, frame,
/// shading or change bar of its own.
fn paragraph_draws_nothing(paragraph: &ParagraphBlock) -> bool {
    paragraph.anchored.is_empty()
        && paragraph.borders.is_none()
        && paragraph.shading.is_none()
        && paragraph.change_bar.is_none()
        && paragraph.lines.iter().all(line_draws_nothing)
}

/// The paragraph a shared block holds, if it holds one.
fn shared_paragraph(block: &SharedLayoutBlock) -> Option<&ParagraphBlock> {
    match block {
        SharedLayoutBlock::Owned { block, .. } => match block.as_ref() {
            LayoutBlock::Paragraph(paragraph) => Some(paragraph),
            LayoutBlock::Table(_) => None,
        },
        SharedLayoutBlock::Paragraph { block, .. } => Some(block),
        SharedLayoutBlock::Table { .. } => None,
    }
}

/// A page break that ends the last paragraph drawing anything in a group of
/// sections, followed only by paragraphs that draw nothing (the one holding
/// the section break), would start the page the next section starts anyway.
/// Word and LibreOffice give it no page of its own, so the break goes, and
/// what follows it takes no room: the space after its paragraph, the empty
/// end of that paragraph and the empty paragraphs stay on the page it ends,
/// even a full one, with no line number and no layout fragment.
fn absorb_page_break_before_section_break(blocks: &mut [SharedLayoutBlock]) {
    let trailing_empty = blocks
        .iter()
        .rev()
        .take_while(|block| shared_paragraph(block).is_some_and(paragraph_draws_nothing))
        .count();
    let Some(last) = blocks.len().checked_sub(trailing_empty + 1) else {
        return;
    };
    let Some(paragraph) = shared_paragraph(&blocks[last]) else {
        return;
    };
    if paragraph.borders.is_some() || paragraph.shading.is_some() || paragraph.change_bar.is_some()
    {
        return;
    }
    let Some(last_break) = paragraph
        .lines
        .iter()
        .rposition(|line| line.forced_break_after.is_some())
    else {
        return;
    };
    if paragraph.lines[last_break].forced_break_after != Some(ForcedBreakKind::Page)
        || !paragraph.lines[last_break + 1..]
            .iter()
            .all(line_draws_nothing)
    {
        return;
    }
    let paragraph = shared_paragraph_mut(&mut blocks[last]);
    // Nothing that takes room follows it on its page, and Word drops the
    // space after a paragraph at the foot of a page.
    paragraph.space_after = 0.0;
    absorb_into_section_break(paragraph);
    for block in &mut blocks[last + 1..] {
        let paragraph = shared_paragraph_mut(block);
        paragraph.space_before = 0.0;
        paragraph.space_after = 0.0;
        absorb_into_section_break(paragraph);
    }
}

/// Drop the last page break of a paragraph a following section break absorbs,
/// and make the lines after it take no room; an empty paragraph after it has
/// no break, so all its lines take no room. A re-break around a floating
/// drawing starts from the paragraph's items, which still hold the break, so
/// it applies this again.
pub(super) fn absorb_into_section_break(paragraph: &mut ParagraphBlock) {
    let rest = match paragraph
        .lines
        .iter()
        .rposition(|line| line.forced_break_after.is_some())
    {
        Some(last_break) => {
            paragraph.lines[last_break].forced_break_after = None;
            last_break + 1
        }
        None => {
            // Nothing of it is drawn, so nothing must clear a drawing.
            paragraph.content_offset_top = 0.0;
            0
        }
    };
    paragraph.lines[rest..].iter_mut().for_each(take_no_room);
    paragraph.absorbed_by_section_break = true;
}

/// The paragraph of a shared block found to hold one, made this block's own.
fn shared_paragraph_mut(block: &mut SharedLayoutBlock) -> &mut ParagraphBlock {
    match block {
        SharedLayoutBlock::Owned { block, .. } => match block.as_mut() {
            LayoutBlock::Paragraph(paragraph) => paragraph,
            LayoutBlock::Table(_) => unreachable!("only paragraphs are absorbed"),
        },
        SharedLayoutBlock::Paragraph { block, .. } => Arc::make_mut(block),
        SharedLayoutBlock::Table { .. } => unreachable!("only paragraphs are absorbed"),
    }
}

/// An empty line made to take no room on its page.
fn take_no_room(line: &mut LayoutLine) {
    line.height = 0.0;
    line.ascent = 0.0;
    line.descent = 0.0;
    line.line_gap = 0.0;
}

/// Whether a line was made to take no room by [`take_no_room`].
pub(super) fn takes_no_room(line: &LayoutLine) -> bool {
    line.height == 0.0 && line.ascent == 0.0 && line.descent == 0.0
}

/// The endnotes a section printing them at its end draws there, in the
/// order of their references.
fn section_end_endnotes(section: &SharedSection, notes: &NoteRegistry) -> Vec<NoteRef> {
    let mut ordered = Vec::new();
    for block in &section.blocks {
        let references = block
            .paragraph()
            .map(|paragraph| {
                paragraph
                    .lines
                    .iter()
                    .flat_map(notes_in_line)
                    .collect::<Vec<_>>()
            })
            .or_else(|| block.table().map(|table| table_note_references(&table)))
            .unwrap_or_default();
        for note in references {
            if note.stream == NoteStream::Endnote
                && notes.get(note, section.geometry.content_width()).is_some()
                && !ordered.contains(&note)
            {
                ordered.push(note);
            }
        }
    }
    ordered
}

pub(crate) fn paginate_shared_sections(
    sections: &[SharedSection],
    fm: &FontManager,
    media: &MediaRegistry,
    notes: &NoteRegistry,
) -> SharedPagination {
    let media = media.media();
    if sections.is_empty() {
        return SharedPagination {
            paint: PagePaintRecords {
                policies: vec![PaintPolicies::default()],
                boundaries: vec![PagePaintBoundary {
                    behind_end: 0,
                    regular_end: 0,
                    body_front_end: 0,
                    furniture_front_end: 0,
                    elements_len: 0,
                }],
            },
            furniture_failure: None,
            page_sections: vec![crate::WordPageSection {
                physical_page: 1,
                displayed_page: 1,
                section_index: 0,
            }],
            pages: vec![PageFrame::new(1, 612.0, 792.0, Vec::new())],
            outlines: Vec::new(),
            body_fragments: Vec::new(),
            last_flow_space: None,
            endnotes_placed: Vec::new(),
            blank_pages: Vec::new(),
        };
    }
    if sections.len() == 1 {
        let section = &sections[0];
        let mut result = paginate_with_media_recorded(
            &section.blocks,
            section.geometry.clone(),
            section.header_footer.as_ref(),
            section.header_footer_semantics.as_ref(),
            section.title_pg,
            section.footnotes_beneath_text,
            section.written_tb_rl_band,
            section.line_number_style,
            fm,
            media,
            notes,
            1,
            section.page_number_start.unwrap_or(1),
            None,
        );
        let mut endnotes_placed = Vec::new();
        if section.endnotes_at_section_end {
            endnotes_placed =
                collect_endnote_refs(&result.pages, notes, section.geometry.content_width());
            if let Err(failure) = append_endnote_pages_at_document_end(
                &mut result.pages,
                &mut result.paint,
                notes,
                section.geometry.clone(),
                result.last_flow_space,
                &[],
                Some(&EndnoteFurniture {
                    section,
                    section_index: 0,
                    fm,
                    media,
                }),
            ) {
                result.furniture_failure.get_or_insert(failure);
            }
        }
        return SharedPagination {
            paint: result.paint,
            furniture_failure: result.furniture_failure,
            page_sections: result
                .pages
                .iter()
                .map(|page| crate::WordPageSection {
                    physical_page: page.page_number,
                    displayed_page: page.displayed_page_number,
                    section_index: 0,
                })
                .collect(),
            pages: result.pages,
            outlines: result.outlines,
            body_fragments: result.body_fragments,
            last_flow_space: result.last_flow_space,
            endnotes_placed,
            blank_pages: Vec::new(),
        };
    }

    let mut furniture_failure = None;
    let mut page_sections = Vec::new();
    let mut pages = Vec::new();
    let mut paint = PagePaintRecords::default();
    let mut outlines = Vec::new();
    let mut body_fragments = Vec::new();
    let mut page_offset = 0;
    let mut next_section_page_number = 1usize;
    let mut last_flow_space = None;
    let mut endnotes_placed = Vec::new();
    let mut blank_pages = Vec::new();
    let mut section_index = 0;
    while section_index < sections.len() {
        let section = &sections[section_index];
        let mut group_end = section_index + 1;
        while group_end < sections.len()
            && sections[group_end].continuous
            && section.written_tb_rl_band == sections[group_end].written_tb_rl_band
            && !sections[group_end - 1].endnotes_at_section_end
            && continuous_geometry_matches(&section.geometry, &sections[group_end].geometry)
        {
            group_end += 1;
        }
        let mut blocks = sections[section_index..group_end]
            .iter()
            .flat_map(|section| section.blocks.iter().cloned())
            .collect::<Vec<_>>();
        let boundary = &sections[group_end - 1];
        let section_endnotes = if boundary.endnotes_at_section_end {
            section_end_endnotes(boundary, notes)
        } else {
            Vec::new()
        };
        // Endnotes printed at the end of the boundary section follow the
        // break: then it ends a page that draws them, and stays.
        if group_end < sections.len() && section_endnotes.is_empty() {
            absorb_page_break_before_section_break(&mut blocks);
        }
        let section_page_number = section
            .page_number_start
            .unwrap_or(next_section_page_number);
        let mut result = paginate_with_media_recorded(
            &blocks,
            section.geometry.clone(),
            section.header_footer.as_ref(),
            section.header_footer_semantics.as_ref(),
            section.title_pg,
            section.footnotes_beneath_text,
            section.written_tb_rl_band,
            section.line_number_style,
            fm,
            media,
            notes,
            page_offset + 1,
            section_page_number,
            (group_end > section_index + 1).then_some(&sections[section_index..group_end]),
        );
        if let Some(mut failure) = result.furniture_failure.take() {
            failure.section += section_index;
            furniture_failure.get_or_insert(failure);
        }
        let mut section_end_last_page = None;
        if boundary.endnotes_at_section_end {
            let ordered = section_endnotes;
            endnotes_placed.extend(&ordered);
            match append_ordered_endnote_pages(
                &mut result.pages,
                &mut result.paint,
                &ordered,
                notes,
                boundary.geometry.clone(),
                0,
                1,
                result.last_flow_space,
                Some(&EndnoteFurniture {
                    section: boundary,
                    section_index: group_end - 1,
                    fm,
                    media,
                }),
            ) {
                Ok(space) => result.last_flow_space = space,
                Err(failure) => {
                    furniture_failure.get_or_insert(failure);
                }
            }
            section_end_last_page = result.pages.last().map(|page| page.page_number);
        }
        // An odd-page or even-page section starts on a physical page of its
        // parity: when the next page has the other one, this group ends with
        // a blank page of the next section's size, as LibreOffice prints it.
        // It takes the number the next section would continue from, and
        // nothing is painted on it: no header, footer, border, line number or
        // page colour.
        //
        // With Word's even and odd headers on, odd numbers stay on right-hand
        // pages: a new-page section whose first number restarts with the
        // parity of the page before it starts after such a blank page too.
        if let Some(next) = sections.get(group_end) {
            let physical = page_offset + result.pages.len() + 1;
            let wrong_side = match next.parity {
                Some(parity) => (physical % 2 == 1) != (parity == SectionParity::Odd),
                None => {
                    next.odd_numbers_on_right_pages
                        && !next.continuous
                        && next.page_number_start.zip(result.pages.last()).is_some_and(
                            |(start, last)| start % 2 == last.displayed_page_number % 2,
                        )
                }
            };
            if wrong_side {
                let continued = result.pages.last().map_or(section_page_number, |page| {
                    page.displayed_page_number.saturating_add(1)
                });
                let geometry = PageGeometry {
                    page_width: next.geometry.page_width,
                    page_height: next.geometry.page_height,
                    page_borders: None,
                    column_separator: false,
                    ..boundary.geometry.clone()
                };
                let mut blank = paginate_with_media_recorded::<SharedLayoutBlock>(
                    &[],
                    geometry,
                    None,
                    None,
                    false,
                    boundary.footnotes_beneath_text,
                    boundary.written_tb_rl_band,
                    None,
                    fm,
                    media,
                    notes,
                    physical,
                    continued,
                    None,
                );
                section_end_last_page = blank.pages.last().map(|page| page.page_number);
                blank_pages.push(physical - 1);
                result.paint.policies.append(&mut blank.paint.policies);
                result.paint.boundaries.append(&mut blank.paint.boundaries);
                result.pages.append(&mut blank.pages);
            }
        }
        for (offset, member) in sections[section_index..group_end].iter().enumerate() {
            let body_indices = member
                .blocks
                .iter()
                .filter_map(|block| match block {
                    SharedLayoutBlock::Owned { body_index, .. }
                    | SharedLayoutBlock::Paragraph { body_index, .. }
                    | SharedLayoutBlock::Table { body_index, .. } => *body_index,
                })
                .collect::<std::collections::HashSet<_>>();
            let occupied = result
                .body_fragments
                .iter()
                .filter(|(index, _)| body_indices.contains(index))
                .map(|(_, fragment)| fragment.physical_page)
                .collect::<std::collections::HashSet<_>>();
            let first = occupied
                .iter()
                .min()
                .copied()
                .or_else(|| result.pages.first().map(|page| page.page_number));
            let last = if section_index + offset + 1 == group_end {
                section_end_last_page
                    .or_else(|| occupied.iter().max().copied())
                    .or(first)
            } else {
                occupied.iter().max().copied().or(first)
            };
            for page in &result.pages {
                if first
                    .zip(last)
                    .is_some_and(|(first, last)| (first..=last).contains(&page.page_number))
                {
                    page_sections.push(crate::WordPageSection {
                        physical_page: page.page_number,
                        displayed_page: page.displayed_page_number,
                        section_index: section_index + offset,
                    });
                }
            }
        }
        next_section_page_number = result.pages.last().map_or(section_page_number, |page| {
            page.displayed_page_number.saturating_add(1)
        });
        last_flow_space = result.last_flow_space;
        page_offset += result.pages.len();
        paint.policies.append(&mut result.paint.policies);
        paint.boundaries.append(&mut result.paint.boundaries);
        pages.append(&mut result.pages);
        outlines.append(&mut result.outlines);
        body_fragments.append(&mut result.body_fragments);
        section_index = group_end;
    }
    for (index, page) in pages.iter_mut().enumerate() {
        page.page_number = index + 1;
    }
    SharedPagination {
        paint,
        furniture_failure,
        page_sections,
        pages,
        outlines,
        body_fragments,
        last_flow_space,
        endnotes_placed,
        blank_pages,
    }
}

fn continuous_geometry_matches(left: &PageGeometry, right: &PageGeometry) -> bool {
    left.page_width == right.page_width
        && left.page_height == right.page_height
        && left.margin_left == right.margin_left
        && left.margin_right == right.margin_right
        && left.margin_top == right.margin_top
        && left.margin_bottom == right.margin_bottom
        && left.body_rotation == right.body_rotation
        && left.columns.len() == right.columns.len()
        && left
            .columns
            .iter()
            .zip(&right.columns)
            .all(|(left, right)| left.width == right.width && left.x == right.x)
}

pub(crate) fn paginate_shared_single_section_recorded(
    section: &SharedSection,
    fm: &FontManager,
    media: &MediaRegistry,
    notes: &NoteRegistry,
    restart: Option<PaginationCheckpoint>,
    stop_at: Option<PaginationCheckpoint>,
) -> RecordedPagination {
    let checkpoint = restart.unwrap_or(PaginationCheckpoint {
        next_block_index: 0,
        page_count: 0,
        next_header_page_number: section.page_number_start.unwrap_or(1),
    });
    let context = PassContext {
        line_number_style: section.line_number_style,
        written_tb_rl_band: section.written_tb_rl_band,
        geometry: section.geometry.clone(),
        header_footer: section.header_footer.as_ref(),
        header_footer_semantics: section.header_footer_semantics.as_ref(),
        title_pg: section.title_pg,
        footnotes_beneath_text: section.footnotes_beneath_text,
        fm,
        media: media.media(),
        notes,
        first_page_number: checkpoint.page_count + 1,
        first_header_page_number: checkpoint.next_header_page_number,
        continuous_sections: None,
    };
    let result = paginate_pass_from(
        &section.blocks,
        &context,
        &ResolvedWraps::new(),
        checkpoint.next_block_index,
        checkpoint.page_count == 0,
        stop_at,
    );
    let mut checkpoints = result.checkpoints;
    if checkpoint.page_count == 0 {
        checkpoints.insert(0, checkpoint);
    }
    RecordedPagination {
        paint: result.paint,
        furniture_failure: result.furniture_failure,
        pages: result.pages,
        outlines: result.outlines,
        body_fragments: result.body_fragments,
        checkpoints,
        stopped_at: result.stopped_at,
        last_flow_space: result.last_flow_space,
    }
}

/// Paginate a sequence of blocks into pages.
pub fn paginate(
    blocks: &[LayoutBlock],
    geometry: PageGeometry,
    header_footer: Option<&HeaderFooterContent>,
    title_pg: bool,
    _fm: &FontManager,
    media: &MediaRegistry,
    notes: &NoteRegistry,
) -> (Vec<PageFrame>, Vec<OutlineEntry>) {
    paginate_with_media(
        blocks,
        geometry,
        header_footer,
        None,
        title_pg,
        _fm,
        media.media(),
        notes,
        1,
        1,
    )
}
