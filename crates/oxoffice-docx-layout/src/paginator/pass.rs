//! One pagination pass over a block list, with its media.

use super::*;

pub(super) fn paginate_with_media<B: LayoutBlockLike>(
    blocks: &[B],
    geometry: PageGeometry,
    header_footer: Option<&HeaderFooterContent>,
    header_footer_semantics: Option<&HeaderFooterSemantics>,
    title_pg: bool,
    _fm: &FontManager,
    media: &HashMap<MediaId, ImageData>,
    notes: &NoteRegistry,
    first_page_number: usize,
    first_header_page_number: usize,
) -> (Vec<PageFrame>, Vec<OutlineEntry>) {
    let result = paginate_with_media_recorded(
        blocks,
        geometry,
        header_footer,
        header_footer_semantics,
        title_pg,
        false,
        false,
        None,
        _fm,
        media,
        notes,
        first_page_number,
        first_header_page_number,
        None,
    );
    (result.pages, result.outlines)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn paginate_with_media_recorded<B: LayoutBlockLike>(
    blocks: &[B],
    geometry: PageGeometry,
    header_footer: Option<&HeaderFooterContent>,
    header_footer_semantics: Option<&HeaderFooterSemantics>,
    title_pg: bool,
    footnotes_beneath_text: bool,
    written_tb_rl_band: bool,
    line_number_style: Option<WordLineNumberStyle>,
    _fm: &FontManager,
    media: &HashMap<MediaId, ImageData>,
    notes: &NoteRegistry,
    first_page_number: usize,
    first_header_page_number: usize,
    continuous_sections: Option<&[SharedSection]>,
) -> PassResult {
    let context = PassContext {
        line_number_style,
        written_tb_rl_band,
        geometry,
        header_footer,
        header_footer_semantics,
        title_pg,
        footnotes_beneath_text,
        fm: _fm,
        media,
        notes,
        first_page_number,
        first_header_page_number,
        continuous_sections,
    };
    let first = paginate_pass(blocks, &context, &ResolvedWraps::new());

    // A paragraph-relative drawing has no vertical position until its own
    // paragraph is placed, so the first pass cannot offer one to the text above
    // it. The second pass can, because the first recorded where each landed.
    //
    // Two passes, not a fixed point. The second pass reflows earlier text, which
    // can move the drawing's own paragraph, so the rect it offered may be
    // slightly stale. Iterating is not guaranteed to terminate: growing a
    // paragraph can push a drawing to the next page, which shrinks the
    // paragraph, which pulls it back.
    if !has_paragraph_relative_wrap(blocks) {
        return first;
    }

    paginate_pass(blocks, &context, &first.resolved)
}

fn paginate_pass<B: LayoutBlockLike>(
    blocks: &[B],
    context: &PassContext,
    resolved_in: &ResolvedWraps,
) -> PassResult {
    paginate_pass_from(blocks, context, resolved_in, 0, true, None)
}

pub(super) fn paginate_pass_from<B: LayoutBlockLike>(
    blocks: &[B],
    context: &PassContext,
    resolved_in: &ResolvedWraps,
    first_block_index: usize,
    is_first_page: bool,
    stop_at: Option<PaginationCheckpoint>,
) -> PassResult {
    if first_block_index >= blocks.len() && !is_first_page {
        return PassResult {
            paint: PagePaintRecords::default(),
            furniture_failure: None,
            pages: Vec::new(),
            outlines: Vec::new(),
            resolved: resolved_in.clone(),
            body_fragments: Vec::new(),
            checkpoints: Vec::new(),
            stopped_at: None,
            last_flow_space: None,
        };
    }
    let mut pager = Pager::new(
        context.geometry.clone(),
        context.header_footer,
        context.header_footer_semantics,
        context.title_pg,
        context.footnotes_beneath_text,
        context.written_tb_rl_band,
        context.line_number_style,
        context.media,
        context.notes,
        context.fm,
        resolved_in,
        context.first_page_number,
        context.first_header_page_number,
        is_first_page,
        stop_at,
    );

    pager.continuous_sections = context.continuous_sections;
    for (block_idx, block) in blocks.iter().enumerate().skip(first_block_index) {
        if !block.paint_owner_valid() {
            pager
                .pending_furniture_failure
                .get_or_insert_with(|| "Word paragraph lacks its source paint companion".into());
            break;
        }
        if let Some(sections) = context.continuous_sections {
            let mut end = 0;
            let previous_member = pager.active_member;
            pager.active_member = sections
                .iter()
                .position(|section| {
                    end += section.blocks.len();
                    block_idx < end
                })
                .unwrap_or(sections.len() - 1);
            if pager.active_member != previous_member
                && !pager.page_note_ids.is_empty()
                && !sections[pager.active_member].modern_footnote_layout
            {
                if sections[pager.active_member].footnote_layout_like_word8 {
                    pager.pending_continuous_footnote_break = true;
                } else {
                    pager.finish_page_outright();
                }
            }
        }
        if pager.pending_continuous_footnote_break {
            let references = block
                .paragraph()
                .map(|paragraph| {
                    paragraph
                        .lines
                        .iter()
                        .flat_map(page_foot_notes_in_line)
                        .collect::<Vec<_>>()
                })
                .or_else(|| block.table().map(|table| table_note_references(&table)))
                .unwrap_or_default();
            if references.iter().any(|reference| {
                reference.stream == NoteStream::Footnote
                    && pager
                        .notes
                        .get(*reference, pager.geometry.content_width())
                        .is_some()
            }) {
                pager.finish_page_outright();
            }
        }
        let follows_trailing_run_page_break =
            pager.consume_trailing_run_page_break_before(block_idx);
        // Check for page break before
        if block.page_break_before() && pager.has_content() && !follows_trailing_run_page_break {
            pager.finish_page_before(block_idx);
            if pager.stopped_at.is_some() {
                break;
            }
        }

        pager.select_empty_page_member();
        if let Some(para) = block.paragraph() {
            // The entry is recorded where the paragraph's first line lands.
            pager.pending_outline = para
                .heading_level
                .zip(para.heading_text.clone())
                .map(|(level, title)| (title, level));
            paginate_paragraph(para, block.body_index(), block_idx, blocks, &mut pager);
            if pager.stopped_at.is_some() {
                break;
            }
        } else if let Some(table) = block.table() {
            let body_index = block.body_index();

            // A floating table is positioned rather than flowed, so it never
            // reaches the row loop below and never advances the cursor.
            if let Some(floating) = table.floating.as_deref() {
                let para_top = pager.cursor_y;
                pager.place_floating_table(&table, floating, body_index, block_idx, para_top);
                continue;
            }

            for (row_idx, row) in table.rows.iter().enumerate() {
                // Read per row, because finishing a page may have moved the
                // body into the next column track.
                let row_semantics = table
                    .semantics
                    .and_then(|semantics| semantics.rows.get(row_idx));
                let mut pending = row.clone();
                let mut pending_semantics = row_semantics.cloned();
                let mut moved_whole = false;
                let mut first_fragment = true;
                loop {
                    let space = pager.available_height() - pager.cursor_y;
                    // Word splits a row with a minimum height only when the
                    // part that stays on this page reaches that minimum, and
                    // otherwise moves the row whole.
                    let split = split_simple_table_row(&pending, pending_semantics.as_ref(), space)
                        .filter(|(fragment, ..)| {
                            fragment.height >= pending.min_height || !pager.has_content()
                        });
                    if let Some((fragment, rest, fragment_semantics, rest_semantics)) = split {
                        paint_flowed_table_row(
                            &mut pager,
                            &table,
                            &fragment,
                            fragment_semantics.as_ref(),
                            body_index,
                            first_fragment,
                            false,
                        );
                        pending = rest;
                        pending_semantics = rest_semantics;
                        first_fragment = false;
                        moved_whole = false;
                        pager.finish_page();
                        repeat_table_headers(&mut pager, &table, row_idx, body_index);
                        continue;
                    }
                    if pending.height > space && pager.has_content() && !moved_whole {
                        moved_whole = true;
                        pager.finish_page();
                        repeat_table_headers(&mut pager, &table, row_idx, body_index);
                        continue;
                    }
                    paint_flowed_table_row(
                        &mut pager,
                        &table,
                        &pending,
                        pending_semantics.as_ref(),
                        body_index,
                        first_fragment,
                        true,
                    );
                    break;
                }
            }
        }
    }

    let resolved = std::mem::take(&mut pager.resolved_out);
    let checkpoints = std::mem::take(&mut pager.checkpoints);
    let stopped_at = pager.stopped_at;
    let (pages, outlines, last_flow_space, furniture_failure) = if stopped_at.is_some() {
        (
            std::mem::take(&mut pager.pages),
            std::mem::take(&mut pager.outlines),
            pager.page_flow_spaces.last().copied(),
            pager.furniture_failure.take(),
        )
    } else {
        pager.flush()
    };
    let body_fragments = std::mem::take(&mut pager.body_fragments);
    PassResult {
        paint: PagePaintRecords {
            policies: std::mem::take(&mut pager.page_policies),
            boundaries: std::mem::take(&mut pager.page_boundaries),
        },
        furniture_failure,
        pages,
        outlines,
        body_fragments,
        resolved,
        checkpoints,
        stopped_at,
        last_flow_space,
    }
}
