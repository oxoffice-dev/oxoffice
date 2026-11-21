//! Paragraph layout and the field values computed while laying a paragraph out.

use super::*;

fn computed_layout_field(
    field: &oxoffice_docx_oxml::text::Field,
    source: Option<FieldSource>,
    source_node: Option<SourceNodeId>,
    input: &LayoutInput,
    num_state: &NumberingState,
    diagnostics: &mut Vec<Diagnostic>,
) -> (Option<String>, Option<FieldKind>) {
    let instruction = field.effective_instruction();
    let page_family = matches!(
        instruction.name.as_str(),
        "PAGE" | "NUMPAGES" | "SECTION" | "SECTIONPAGES" | "PAGEREF"
    );
    let (computed_value, field_kind) = if page_family && field.locked() == Some(true) {
        (None, None)
    } else if page_family && let Err(reason) = validate_page_field_instruction(&instruction) {
        diagnostics.push(Diagnostic {
            message: format!("{} retained stored display: {reason}", instruction.name),
        });
        (None, None)
    } else {
        match instruction.name.as_str() {
            "PAGE" => (Some("99".to_owned()), Some(FieldKind::Page)),
            "NUMPAGES" => (Some("99".to_owned()), Some(FieldKind::NumPages)),
            "SECTION" => (Some("99".to_owned()), Some(FieldKind::Section)),
            "SECTIONPAGES" => (Some("99".to_owned()), Some(FieldKind::SectionPages)),
            "SEQ" => {
                let result =
                    source.and_then(|source| input.sequence_snapshot.as_ref()?.field_value(source));
                match result {
                    Some(Ok(value)) => (Some(value.to_owned()), None),
                    Some(Err(message)) => {
                        if source.is_some_and(|source| {
                            input.sequence_snapshot.as_ref().is_some_and(|snapshot| {
                                snapshot.repeat_requests.contains_key(&source)
                            })
                        }) {
                            (Some(String::new()), Some(FieldKind::SequenceRepeat))
                        } else {
                            diagnostics.push(Diagnostic {
                                message: message.to_owned(),
                            });
                            (None, None)
                        }
                    }
                    None => {
                        diagnostics.push(Diagnostic { message: "SEQ retains its stored display without accepted source-qualified context".into() });
                        (None, None)
                    }
                }
            }
            "REF" => {
                let Some(bookmark) = field_text_argument(&instruction, 0) else {
                    return (None, None);
                };
                if let Some(text) = bookmark_text(input, bookmark) {
                    match numbered_ref_text(
                        &instruction,
                        bookmark,
                        text,
                        num_state,
                        source_node,
                        if field_has_switch(&instruction, "p") {
                            input
                                .sequence_snapshot
                                .as_ref()
                                .zip(source)
                                .ok_or_else(|| {
                                    "REF relative source lacks an accepted physical binding"
                                        .to_owned()
                                })
                                .and_then(|(snapshot, source)| {
                                    snapshot
                                        .bookmark_relative_position(bookmark, source)
                                        .map(Some)
                                })
                        } else {
                            Ok(None)
                        },
                    )
                    .and_then(|value| format_numeric_field_display(&instruction, value))
                    {
                        Ok(value) => (Some(value), None),
                        Err(message) => {
                            diagnostics.push(Diagnostic {
                                message: format!("{message}, stored REF display retained"),
                            });
                            (None, None)
                        }
                    }
                } else {
                    diagnostics.push(Diagnostic {
                        message: format!(
                            "REF target {bookmark} was not found, stored display retained"
                        ),
                    });
                    (None, None)
                }
            }
            "PAGEREF" => {
                let Some(bookmark) = field_text_argument(&instruction, 0) else {
                    return (None, None);
                };
                if bookmark_text(input, bookmark).is_none() {
                    diagnostics.push(Diagnostic {
                        message: format!(
                            "PAGEREF target {bookmark} was not found, stored display retained"
                        ),
                    });
                    (None, None)
                } else if page_reference_keeps_stored_display(&field.cached_result) {
                    (None, None)
                } else if let Some(target) = page_ref_id(input, bookmark) {
                    (Some("99".to_owned()), Some(FieldKind::TargetPage(target)))
                } else {
                    (None, None)
                }
            }
            _ => (None, None),
        }
    };
    // Word draws a legacy checkbox itself from its state, locked or not;
    // any stored result only gives the box its formatting.
    if let Some(glyph) = drawn_legacy_checkbox(field) {
        return (Some(glyph.to_owned()), None);
    }
    if let Some(entry) = drawn_legacy_dropdown(field) {
        return (Some(entry.to_owned()), None);
    }
    let (computed_value, field_kind) = if field.locked() == Some(true) {
        (None, None)
    } else {
        (computed_value, field_kind)
    };
    (computed_value, field_kind)
}

/// The box a legacy checkbox draws from its state, as Word does whatever
/// result it stores: ☒ when checked, ☐ when not.
pub(super) fn drawn_legacy_checkbox(
    field: &oxoffice_docx_oxml::text::Field,
) -> Option<&'static str> {
    let data = field.legacy_form.as_ref()?;
    let oxoffice_docx_oxml::text::LegacyFormFieldValue::Checked(checked) = data.value else {
        return None;
    };
    Some(if checked { "\u{2612}" } else { "\u{2610}" })
}

/// The entry a legacy drop-down that stores no result draws: its selected
/// one, as Word draws it. Word saves drop-downs that way.
pub(super) fn drawn_legacy_dropdown(field: &oxoffice_docx_oxml::text::Field) -> Option<&str> {
    let data = field.legacy_form.as_ref()?;
    let oxoffice_docx_oxml::text::LegacyFormFieldValue::SelectedIndex(index) = data.value else {
        return None;
    };
    if !field.effective_display().is_empty() {
        return None;
    }
    data.choices.get(index).map(String::as_str)
}

/// The fields of a paragraph's physical runs in source order, nested fields
/// after the field holding them: the order that names a field by index.
pub(crate) fn source_fields(para: &CT_P) -> Vec<&oxoffice_docx_oxml::text::Field> {
    fn collect<'a>(
        field: &'a oxoffice_docx_oxml::text::Field,
        fields: &mut Vec<&'a oxoffice_docx_oxml::text::Field>,
    ) {
        fields.push(field);
        for nested in field.all_nested_fields_in_source_order() {
            collect(nested, fields);
        }
    }
    let mut fields = Vec::new();
    for field in para
        .source_runs()
        .into_iter()
        .flat_map(|run| &run.content)
        .filter_map(|content| match content {
            RunContent::Field(field) => Some(field),
            _ => None,
        })
    {
        collect(field, &mut fields);
    }
    fields
}

/// Lay out one paragraph on a section character grid.
#[allow(clippy::too_many_arguments)]
pub(crate) fn layout_paragraph_with_grid(
    para: &CT_P,
    available_width: f64,
    styles: &CT_Styles,
    input: &LayoutInput,
    media: &MediaRegistry,
    fm: &mut FontManager,
    num_state: &mut NumberingState,
    diagnostics: &mut Vec<Diagnostic>,
    doc_grid: Option<&CT_DocGrid>,
) -> Result<ParagraphBlock> {
    layout_paragraph_with_source_and_table(
        para,
        available_width,
        styles,
        input,
        media,
        fm,
        num_state,
        diagnostics,
        None,
        None,
        None,
        None,
        None,
        None,
        doc_grid,
        false,
        None,
    )
    .map(|(block, _)| block)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn layout_paragraph_with_source_and_direction(
    para: &CT_P,
    available_width: f64,
    styles: &CT_Styles,
    input: &LayoutInput,
    media: &MediaRegistry,
    fm: &mut FontManager,
    num_state: &mut NumberingState,
    diagnostics: &mut Vec<Diagnostic>,
    source_node: Option<SourceNodeId>,
    navigation_node: Option<SourceNodeId>,
    doc_grid: Option<&CT_DocGrid>,
    form: Option<crate::forms::FormParagraph>,
) -> Result<(ParagraphBlock, TextDirection, WordParagraphPaint)> {
    let mut direction = TextDirection::Auto;
    let (block, paint) = layout_paragraph_with_source_and_table(
        para,
        available_width,
        styles,
        input,
        media,
        fm,
        num_state,
        diagnostics,
        source_node,
        navigation_node,
        None,
        None,
        None,
        Some(&mut direction),
        doc_grid,
        false,
        form,
    )?;
    Ok((block, direction, paint))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn layout_paragraph_with_source_in_table(
    para: &CT_P,
    available_width: f64,
    styles: &CT_Styles,
    input: &LayoutInput,
    media: &MediaRegistry,
    fm: &mut FontManager,
    num_state: &mut NumberingState,
    diagnostics: &mut Vec<Diagnostic>,
    source_node: Option<SourceNodeId>,
    table_properties: Option<&oxoffice_docx_oxml::properties::CT_PPr>,
    table_run_properties: Option<&oxoffice_docx_oxml::properties::CT_RPr>,
    cell_shading: Option<Color>,
    doc_grid: Option<&CT_DocGrid>,
    form: Option<crate::forms::FormParagraph>,
) -> Result<(ParagraphBlock, TextDirection, WordParagraphPaint)> {
    let mut direction = TextDirection::Auto;
    let (block, paint) = layout_paragraph_with_source_and_table(
        para,
        available_width,
        styles,
        input,
        media,
        fm,
        num_state,
        diagnostics,
        source_node,
        source_node,
        table_properties,
        table_run_properties,
        cell_shading,
        Some(&mut direction),
        doc_grid,
        true,
        form,
    )?;
    Ok((block, direction, paint))
}

#[allow(clippy::too_many_arguments)]
fn layout_paragraph_with_source_and_table(
    para: &CT_P,
    available_width: f64,
    styles: &CT_Styles,
    input: &LayoutInput,
    media: &MediaRegistry,
    fm: &mut FontManager,
    num_state: &mut NumberingState,
    diagnostics: &mut Vec<Diagnostic>,
    source_node: Option<SourceNodeId>,
    navigation_node: Option<SourceNodeId>,
    table_properties: Option<&oxoffice_docx_oxml::properties::CT_PPr>,
    table_run_properties: Option<&oxoffice_docx_oxml::properties::CT_RPr>,
    cell_shading: Option<Color>,
    reflow_direction_out: Option<&mut TextDirection>,
    doc_grid: Option<&CT_DocGrid>,
    in_table_cell: bool,
    // The main-story paragraph this is, from a caller that knows: other
    // stories, text boxes and cached copies draw no form marks.
    form: Option<crate::forms::FormParagraph>,
) -> Result<(ParagraphBlock, WordParagraphPaint)> {
    // Resolve paragraph properties
    let para_style_id = para.properties.as_ref().and_then(|p| p.style_id.as_deref());

    let resolved_ppr = style_resolver::resolve_paragraph_properties_in_table(
        para_style_id,
        styles,
        table_properties,
    );

    let mut effective_ppr = resolved_ppr;

    // A numbering level carries paragraph properties of its own, mainly the
    // indentation for that level. They sit between the style and direct
    // formatting, so merge them before the direct properties rather than
    // after. Without this every level of a list draws at the same indent.
    let direct_ppr = para.properties.as_ref();
    let list_num_id = direct_ppr.and_then(|p| p.num_id).or(effective_ppr.num_id);
    let list_ilvl = direct_ppr
        .and_then(|p| p.num_ilvl)
        .or(effective_ppr.num_ilvl)
        .unwrap_or(0);
    if let (Some(num_id), Some(numbering)) = (list_num_id, input.numbering.as_ref())
        && let Some(lvl_ppr) =
            style_resolver::level_paragraph_properties(num_id, list_ilvl, numbering)
    {
        merge_direct_ppr(&mut effective_ppr, lvl_ppr);
    }

    // Merge direct paragraph properties
    if let Some(direct_ppr) = direct_ppr {
        merge_direct_ppr(&mut effective_ppr, direct_ppr);
    }

    let exclusions = navigation_node.and_then(|node| {
        let snapshot = input.sequence_snapshot.as_ref()?;
        snapshot
            .navigation_exclusions
            .get(snapshot.source_node(node)?)
    });
    let projected = project_paragraph_runs(para, input.revision_view);
    let heading_text = projected.iter().enumerate().filter(|(index, _)| exclusions.is_none_or(|runs| runs.binary_search(index).is_err()))
        .map(|(_, projected)| {
            let mut run = projected.run.clone();
            if exclusions.is_some() {
                run.content.retain(|content| !matches!(content, RunContent::Field(field) if matches!(field.effective_instruction().name.as_str(), "TOC" | "INDEX" | "TOA" | "BIBLIOGRAPHY")));
            }
            run.text()
        }).collect::<String>();
    let cache_only = exclusions.is_some() && heading_text.is_empty();
    let heading_level = effective_ppr
        .outline_lvl
        .or_else(|| detect_heading_level(para, styles).map(|level| level - 1))
        .filter(|level| *level < 9 && !cache_only)
        .map(|level| level + 1);
    if let Some(source) = navigation_node {
        num_state.record_navigation(
            source,
            crate::ParagraphNavigation {
                omitted_by_table: false,
                level: heading_level,
                text: heading_text.clone(),
                style: (!cache_only)
                    .then(|| para_style_id.map(str::to_owned))
                    .flatten(),
                suppressed_numbering: effective_ppr.num_id == Some(0),
                accepted_runs: projected.len(),
            },
        );
    }

    // Convert paragraph properties to layout values
    let space_before = effective_ppr.space_before.map(|t| t.to_pt()).unwrap_or(0.0);
    let space_after = effective_ppr.space_after.map(|t| t.to_pt()).unwrap_or(0.0);
    let base_direction = match effective_ppr.bidi {
        Some(true) => TextDirection::RightToLeft,
        Some(false) => TextDirection::LeftToRight,
        None => TextDirection::Auto,
    };
    let keep_next = effective_ppr.keep_next.unwrap_or(false);
    let keep_lines = effective_ppr.keep_lines.unwrap_or(false);
    let page_break_before = effective_ppr.page_break_before.unwrap_or(false);
    let widow_control = effective_ppr.widow_control.unwrap_or(true);
    let automatic_hyphenation =
        input.automatic_hyphenation && effective_ppr.suppress_auto_hyphens != Some(true);

    // The section character grid, gated by the paragraph's own `w:snapToGrid`.
    // A `default` grid, and an absent one, leave both values at their
    // ungridded form, so an ungridded paragraph takes the branch it always
    // took with no new arithmetic on it.
    let doc_grid = doc_grid.filter(|_| effective_ppr.snap_to_grid != Some(false));
    let grid_char_space_pt = doc_grid
        .filter(|grid| grid.grid_type.is_some_and(ST_DocGrid::snaps_characters))
        .and_then(|grid| grid.char_space)
        .map_or(0.0, |space| oxoffice_docx_oxml::units::Twips(space).to_pt());
    let grid_line_pitch_pt = doc_grid
        .filter(|grid| grid.grid_type.is_some_and(ST_DocGrid::snaps_lines))
        .and_then(|grid| grid.line_pitch)
        .map(|pitch| pitch.to_pt())
        .filter(|pitch| *pitch > 0.0);

    let shading = effective_ppr.shading.as_deref().and_then(shading_color);
    // Text in the automatic colour contrasts with the fill behind it, the
    // paragraph's own or else the cell's.
    let automatic_color = automatic_text_color(shading.or(cell_shading));
    let toc_paragraph =
        para_style_id.is_some_and(|style_id| is_toc_paragraph_style(style_id, styles));

    // Convert runs to inline items
    let mut inline_items = Vec::new();
    let mut multilingual_styles = HashMap::<usize, WordMultilingualStyle>::new();
    let mut requested_context_bytes = 0usize;
    let mut requested_context_entries = 0usize;
    let current_bookmarks = para.current_bookmark_projection();

    if let Some(source_node) = source_node {
        for name in para
            .bookmark_markers
            .iter()
            .filter(|bookmark| bookmark.is_start())
            .filter_map(|bookmark| bookmark.name())
        {
            num_state.record_bookmark_source(name, source_node);
        }
    }

    // Handle numbering marker
    if let (Some(num_id), Some(numbering)) = (effective_ppr.num_id, input.numbering.as_ref()) {
        let ilvl = effective_ppr.num_ilvl.unwrap_or(0);
        if let Some(marker) = style_resolver::generate_marker(num_id, ilvl, numbering, num_state) {
            if let Some(source_node) = source_node {
                num_state.record(source_node, &marker);
                for name in para
                    .bookmark_markers
                    .iter()
                    .filter(|bookmark| bookmark.is_start())
                    .filter_map(|bookmark| bookmark.name())
                {
                    num_state.record_bookmark(name, source_node, &marker);
                }
            }
            // Shape the marker text
            let marker_rpr = marker.marker_rpr;
            let marker_font_size = marker_rpr.sz.map(|hp| hp.to_pt()).unwrap_or_else(|| {
                style_resolver::resolve_run_properties(
                    para_style_id,
                    None,
                    styles,
                    table_run_properties,
                )
                .sz
                .map(|hp| hp.to_pt())
                .unwrap_or(11.0)
            });
            let marker_bold = marker_rpr.bold.unwrap_or(false);
            let marker_italic = marker_rpr.italic.unwrap_or(false);
            let marker_font_family = marker_rpr.font_ascii.as_deref();

            // Bullet glyphs are not in every font either, so the marker gets
            // the same coverage check as body text.
            let display_selection = if marker
                .marker_text
                .chars()
                .any(|character| (0xf000..=0xf0ff).contains(&(character as u32)))
            {
                fm.resolve_word_symbol_text(
                    marker_font_family,
                    marker_bold,
                    marker_italic,
                    &marker.marker_text,
                    false,
                )
            } else {
                fm.resolve_font_for_text(
                    marker_font_family,
                    marker_bold,
                    marker_italic,
                    &marker.marker_text,
                )
                .map(|id| (marker.marker_text.clone(), id))
            };
            if let Ok((display_text, font_id)) = display_selection
                && let Ok(shaped) = fm.shape_text_with_kerning(
                    font_id,
                    &display_text,
                    marker_font_size,
                    marker_rpr
                        .kern
                        .is_some_and(|threshold| threshold.to_pt() <= marker_font_size),
                )
            {
                let metrics = fm.metrics(font_id, marker_font_size)?;
                for message in fm.unknown_word_symbol_diagnostics(
                    font_id,
                    marker_font_family,
                    &marker.marker_text,
                    &display_text,
                ) {
                    push_unique_diagnostic(diagnostics, message);
                }
                // The number takes the fill of its level's run properties,
                // as a run does, and automatic ink contrasts with it.
                let marker_fill = run_fill(&marker_rpr);
                let color = marker_rpr
                    .color
                    .as_ref()
                    .filter(|c| c.as_str() != "auto")
                    .map(|c| Color::from_hex(c))
                    .unwrap_or_else(|| {
                        marker_fill.map_or(automatic_color, |fill| automatic_text_color(Some(fill)))
                    });

                // The level's run properties affect numbering ink, not the
                // counter or paragraph heading. The suffix keeps its normal
                // layout policy independently of hidden marker glyphs.
                if marker_rpr.vanish != Some(true) {
                    multilingual_styles.insert(
                        inline_items.len(),
                        WordMultilingualStyle {
                            kern: marker_rpr
                                .kern
                                .is_some_and(|threshold| threshold.to_pt() <= marker_font_size),
                            origin: WordInputOrigin::AtomicMarker,
                            requested_family: fm.copy_font_request_context(
                                marker_font_family,
                                &mut requested_context_bytes,
                                &mut requested_context_entries,
                            ),
                            ..Default::default()
                        },
                    );
                    inline_items.push(InlineItem::Marker(TextSegment {
                        form_mark: None,
                        text: display_text,
                        direction: TextDirection::Auto,
                        source: None,
                        font_id,
                        font_size: marker_font_size,
                        glyph_ids: shaped.glyph_ids,
                        advances: shaped.advances,
                        width: shaped.width,
                        ascent: metrics.ascent,
                        descent: metrics.descent,
                        line_gap: 0.0,
                        color,
                        bold: marker_bold,
                        italic: marker_italic,
                        underline: None,
                        strike: false,
                        dstrike: false,
                        highlight: marker_fill,
                        baseline_offset: 0.0,
                        hyperlink_url: None,
                        field_kind: None,
                        field_source: None,
                        note: None,
                        note_reference_source: None,
                    }));
                }

                match marker.suffix {
                    ST_LvlSuffix::Tab => inline_items.push(InlineItem::Tab),
                    ST_LvlSuffix::Space => {
                        multilingual_styles.insert(
                            inline_items.len(),
                            WordMultilingualStyle {
                                kern: marker_rpr
                                    .kern
                                    .is_some_and(|threshold| threshold.to_pt() <= marker_font_size),
                                ..Default::default()
                            },
                        );
                        let shaped = fm.shape_text_with_kerning(
                            font_id,
                            " ",
                            marker_font_size,
                            marker_rpr
                                .kern
                                .is_some_and(|threshold| threshold.to_pt() <= marker_font_size),
                        )?;
                        inline_items.push(InlineItem::Text(TextSegment {
                            form_mark: None,
                            text: " ".to_owned(),
                            direction: TextDirection::Auto,
                            source: None,
                            font_id,
                            font_size: marker_font_size,
                            glyph_ids: shaped.glyph_ids,
                            advances: shaped.advances,
                            width: shaped.width,
                            ascent: metrics.ascent,
                            descent: metrics.descent,
                            line_gap: 0.0,
                            color,
                            bold: marker_bold,
                            italic: marker_italic,
                            underline: None,
                            strike: false,
                            dstrike: false,
                            highlight: marker_fill,
                            baseline_offset: 0.0,
                            hyperlink_url: None,
                            field_kind: None,
                            field_source: None,
                            note: None,
                            note_reference_source: None,
                        }));
                    }
                    ST_LvlSuffix::Nothing => {}
                }
            }
        }
    }

    // Build hyperlink URL map: run index → URL
    let mut run_hyperlink_url: std::collections::HashMap<usize, String> =
        std::collections::HashMap::new();
    for hl in &para.hyperlinks {
        if let Some(ref rel_id) = hl.rel_id
            && let Some(url) = input.hyperlink_urls.get(rel_id)
        {
            for run_idx in hl.run_start..hl.run_end {
                run_hyperlink_url.insert(run_idx, url.clone());
            }
        }
    }

    // Physical typed runs reserve stable field identity before accepted
    // selection, including deleted and inserted revision owners.
    let field_indices = source_node
        .map(|_| {
            source_fields(para)
                .into_iter()
                .map(std::ptr::from_ref)
                .zip(0u32..)
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();

    // Process ordinary and revision-wrapped runs in their preserved order.
    let mut projection_char_offset = 0usize;
    let mut equation_cursor = 0usize;
    let mut display_break = None;
    let original_runs = project_paragraph_runs(para, input.revision_view);
    let projected_run_count = original_runs.len();
    let mut prepared_runs = Vec::with_capacity(original_runs.len());
    let mut original_indices = Vec::with_capacity(original_runs.len());
    for projected in &original_runs {
        let mut indices = Vec::new();
        for (slot, content) in projected.run.content.iter().enumerate() {
            if let RunContent::Field(field) = content {
                field
                    .validate_for_attachment()
                    .map_err(|error| LayoutError::Layout(error.to_string()))?;
                indices.push((slot, field_indices.get(&std::ptr::from_ref(field)).copied()));
            }
        }
        let prepared = if indices.is_empty() {
            None
        } else {
            let mut run = projected.run.clone();
            for (slot, _) in &indices {
                let RunContent::Field(field) = &mut run.content[*slot] else {
                    unreachable!()
                };
                field
                    .normalize_cached_result_owner()
                    .map_err(|error| LayoutError::Layout(error.to_string()))?;
            }
            Some(run)
        };
        prepared_runs.push(prepared);
        original_indices.push(indices);
    }
    let mut field_indices = field_indices;
    let projected_runs = original_runs
        .iter()
        .enumerate()
        .map(|(index, projected)| {
            let run = prepared_runs[index].as_ref().unwrap_or(projected.run);
            for (slot, source_index) in &original_indices[index] {
                if let (RunContent::Field(field), Some(source_index)) =
                    (&run.content[*slot], source_index)
                {
                    let mut pending = vec![field];
                    let mut next = *source_index;
                    while let Some(field) = pending.pop() {
                        field_indices.insert(std::ptr::from_ref(field), next);
                        next += 1;
                        pending.extend(field.all_nested_fields_in_source_order().into_iter().rev());
                    }
                }
            }
            ProjectedRun { run, ..*projected }
        })
        .collect::<Vec<_>>();
    let mut computed_fields = HashMap::new();
    for projected in &projected_runs {
        for content in &projected.run.content {
            if let RunContent::Field(field) = content {
                let source = source_node
                    .zip(field_indices.get(&std::ptr::from_ref(field)).copied())
                    .map(|(node, index)| FieldSource { node, index });
                let computed = if matches!(
                    projected.revision,
                    Some(RevisionKind::Deletion | RevisionKind::MoveFrom)
                ) {
                    (None, None)
                } else {
                    computed_layout_field(field, source, source_node, input, num_state, diagnostics)
                };
                computed_fields.insert(std::ptr::from_ref(field), computed);
            }
        }
    }
    let mut stored_caches = Vec::with_capacity(projected_runs.len());
    let mut stored_char_lengths = Vec::with_capacity(projected_runs.len());
    // The legacy form field a whole run holds, by projected run: its stored
    // result draws as ordinary runs, which take the field's mark.
    let mut legacy_marks = HashMap::new();
    for (projected_index, projected) in projected_runs.iter().enumerate() {
        let cache = if let [RunContent::Field(field)] = projected.run.content.as_slice() {
            let source = source_node
                .zip(field_indices.get(&std::ptr::from_ref(field)).copied())
                .map(|(node, index)| FieldSource { node, index });
            if let Some(form) = form.filter(|_| input.form_fields)
                && crate::forms::is_mapped_legacy(field)
                && let Some(source) = source
            {
                legacy_marks.insert(
                    projected_index,
                    oxoffice_layout::FormMark {
                        node: form.node,
                        field: crate::forms::legacy_mark(source.index),
                    },
                );
            }
            let cache = stored_note_cache_runs(
                field,
                projected.run.properties.as_ref(),
                input.revision_view,
                source,
                matches!(
                    projected.revision,
                    Some(RevisionKind::Deletion | RevisionKind::MoveFrom)
                ),
                Some(input),
                false,
            )?;
            stored_char_lengths.push(
                cache
                    .iter()
                    .map(|stored| stored.run.text().chars().count())
                    .sum::<usize>(),
            );
            let has_notes = cache.iter().any(|stored| {
                stored.run.content.iter().any(|content| {
                    matches!(
                        content,
                        RunContent::FootnoteRef { .. } | RunContent::EndnoteRef { .. }
                    )
                })
            });
            let stored = computed_fields
                .get(&std::ptr::from_ref(field))
                .is_none_or(|(value, _)| value.is_none());
            (has_notes || stored).then_some(cache)
        } else {
            stored_char_lengths.push(projected.run.text().chars().count());
            None
        };
        stored_caches.push(cache);
    }
    let mut expanded = Vec::new();
    for (index, (projected, cache)) in projected_runs.into_iter().zip(&stored_caches).enumerate() {
        let start = projection_char_offset;
        projection_char_offset += stored_char_lengths[index];
        if let Some(cache) = cache {
            let mut stored_start = start;
            for (cache_index, stored) in cache.iter().enumerate() {
                expanded.push((
                    index,
                    ProjectedRun {
                        run: &stored.run,
                        force_underline: projected.force_underline || stored.force_underline,
                        force_strike: projected.force_strike || stored.force_strike,
                        revision: stored.revision.or(projected.revision),
                        ..projected
                    },
                    stored_start,
                    true,
                    cache_index == 0,
                    stored.source,
                    stored.field_kind,
                    stored.occurrence,
                ));
                stored_start += stored.run.text().chars().count();
            }
        } else {
            expanded.push((index, projected, start, false, true, None, None, None));
        }
    }
    // Only a main-story paragraph draws form marks, named after its own
    // source node: a cached, header, note or text-box layout passes none.
    let form_paragraph = form.filter(|_| input.form_fields);
    let inline_controls = form_paragraph
        .filter(|_| !para.content_controls.is_empty() || !para.revisions.is_empty())
        .map(|_| crate::forms::InlineControls::new(para));
    // A paragraph inside a block control field: all it draws is the field's.
    let form_block = form_paragraph.is_some_and(|paragraph| paragraph.block);
    for (
        projected_index,
        projected,
        projected_run_start,
        stored_cache,
        first,
        stored_source,
        stored_field_kind,
        stored_occurrence,
    ) in expanded
    {
        let run = projected.run;
        if first {
            push_targeted_bookmark_markers(
                &mut inline_items,
                &mut multilingual_styles,
                &current_bookmarks,
                projected_index,
                input,
                fm,
                source_node,
            )?;
        }

        let current_form_mark = inline_controls
            .as_ref()
            .zip(projected.control)
            .and_then(|(controls, control)| controls.mark_of(control))
            .or(form_block.then_some(crate::forms::BLOCK_FIELD))
            .zip(form_paragraph)
            .map(|(field, paragraph)| oxoffice_layout::FormMark {
                node: paragraph.node,
                field,
            })
            .or_else(|| legacy_marks.get(&projected_index).copied());
        let current_hyperlink_url = projected
            .ordinary_run_index
            .and_then(|run_index| run_hyperlink_url.get(&run_index).cloned())
            .or_else(|| {
                projected
                    .hyperlink_index
                    .and_then(|index| para.hyperlinks.get(index))
                    .and_then(|hyperlink| hyperlink.rel_id.as_deref())
                    .and_then(|rel_id| input.hyperlink_urls.get(rel_id).cloned())
            });

        // Word writes each entry of a table of contents as a hyperlink to a
        // `_Toc` bookmark whose runs carry the Hyperlink character style, and
        // draws the entry in its TOC paragraph style without that style's
        // colour and underline. LibreOffice does the same.
        let toc_entry = toc_paragraph
            && projected
                .hyperlink_index
                .and_then(|index| para.hyperlinks.get(index))
                .or_else(|| {
                    let run_index = projected.ordinary_run_index?;
                    para.hyperlinks.iter().find(|hyperlink| {
                        (hyperlink.run_start..hyperlink.run_end).contains(&run_index)
                    })
                })
                .and_then(|hyperlink| hyperlink.anchor.as_deref())
                .is_some_and(|anchor| anchor.starts_with("_Toc"));
        let run_style_id = run
            .properties
            .as_ref()
            .and_then(|p| p.style_id.as_deref())
            .filter(|style_id| !(toc_entry && is_hyperlink_character_style(style_id, styles)));

        let resolved_rpr = style_resolver::resolve_run_properties(
            para_style_id,
            run_style_id,
            styles,
            table_run_properties,
        );

        // Merge direct run properties
        let mut effective_rpr = resolved_rpr;
        if let Some(ref direct_rpr) = run.properties {
            effective_rpr.merge_from(direct_rpr);
        }

        push_equations_before_order(
            para,
            projected.boundary,
            projected.raw_order,
            &mut equation_cursor,
            &mut inline_items,
            fm,
            effective_rpr.sz.map(|hp| hp.to_pt()).unwrap_or(11.0),
            resolve_run_color(&effective_rpr, input.theme.as_ref(), automatic_color),
            available_width,
            input.math_properties.as_ref(),
            &mut display_break,
            diagnostics,
        )?;

        // A ruby annotation owns its base runs, so the span is laid out once
        // at its first run and the rest of the span is skipped rather than
        // painted a second time on the ordinary run path.
        if let Some(run_index) = projected.ordinary_run_index
            && let Some(ruby) = para
                .rubies
                .iter()
                .find(|ruby| ruby.base_range().contains(&run_index))
        {
            if ruby.base_start == run_index
                && let Some(base_runs) = para.runs.get(ruby.base_range())
                && let Some(item) = ruby_inline_item(
                    ruby,
                    base_runs,
                    &AnnotationContext {
                        styles,
                        input,
                        para_style_id,
                        table_run_properties,
                        automatic_color,
                    },
                    fm,
                )?
            {
                inline_items.push(item);
            }
            continue;
        }

        // Skip hidden text
        if effective_rpr.vanish == Some(true) {
            continue;
        }

        let mut font_size = effective_rpr.sz.map(|hp| hp.to_pt()).unwrap_or(11.0);
        let bold = effective_rpr.bold.unwrap_or(false);
        let italic = effective_rpr.italic.unwrap_or(false);

        // Resolve font family: theme font takes priority when no explicit font is set
        let run_text = run.text();
        let font_family = resolve_font_family(
            &effective_rpr,
            input.theme.as_ref(),
            word_font_slot_for_text(&run_text, effective_rpr.font_hint.as_deref()),
        );

        // The run's own fill, drawn behind its text, and the automatic
        // colour that contrasts with it.
        let highlight = run_fill(&effective_rpr);
        let run_automatic_color =
            highlight.map_or(automatic_color, |fill| automatic_text_color(Some(fill)));
        // Resolve color: theme color takes priority over literal color value
        let (color, underline) = tracked_marks(
            &projected,
            input.revision_style,
            resolve_run_color(&effective_rpr, input.theme.as_ref(), run_automatic_color),
            convert::underline(effective_rpr.underline),
        );
        let strike = projected.force_strike || effective_rpr.strike.unwrap_or(false);
        let dstrike = effective_rpr.dstrike.unwrap_or(false);

        // Superscript/subscript handling
        let mut baseline_offset = 0.0;
        if let Some(ref va) = effective_rpr.vert_align {
            match va.as_str() {
                "superscript" => {
                    // Reduce font size to ~58% and raise baseline
                    let original_size = font_size;
                    font_size *= 0.58;
                    baseline_offset = original_size * 0.33; // raise by 1/3 of original size
                }
                "subscript" => {
                    // Reduce font size to ~58% and lower baseline
                    let original_size = font_size;
                    font_size *= 0.58;
                    baseline_offset = -(original_size * 0.14); // lower
                }
                _ => {}
            }
        }

        // Position offset (in half-points, positive=raise)
        if let Some(pos) = effective_rpr.position {
            baseline_offset += pos as f64 / 2.0; // half-points to points
        }

        // Resolved against the run's own text, so a family without glyphs for
        // this script is replaced by one that has them.
        let font_id = fm.resolve_font_for_text(font_family.as_deref(), bold, italic, &run_text)?;
        let metrics = fm.metrics(font_id, font_size)?;

        // Stored-display expansion replaces the sole Field run with its
        // cache runs. Its instruction events still belong before that display.
        if stored_cache && first {
            let owner_run = prepared_runs[projected_index]
                .as_ref()
                .unwrap_or(original_runs[projected_index].run);
            if let [RunContent::Field(field)] = owner_run.content.as_slice() {
                push_sequence_context_markers(
                    field,
                    input.sequence_snapshot.as_deref().zip(source_node),
                    &field_indices,
                    font_id,
                    &mut inline_items,
                    &mut multilingual_styles,
                );
            }
        }
        let run_inline_start = inline_items.len();
        let content_char_starts = projected_content_char_starts(run);
        let note_events = if num_state.note_body_context.is_some() {
            crate::notes::run_note_events(run)?
        } else {
            Vec::new()
        };
        for (content_index, content) in run
            .content
            .iter()
            .map(Some)
            .chain(std::iter::once(None))
            .enumerate()
        {
            for (_, event) in note_events
                .iter()
                .filter(|(position, _)| *position == content_index)
            {
                if matches!(event.as_str(), "separator" | "continuationSeparator") {
                    let width = available_width
                        * if event == "continuationSeparator" {
                            1.0
                        } else {
                            crate::notes::SEPARATOR_WIDTH_FRACTION
                        };
                    inline_items.push(InlineItem::Group {
                        width,
                        height: 0.0,
                        baseline: None,
                        math: false,
                        group: GroupElement {
                            transform: Transform::IDENTITY,
                            clip: None,
                            opacity: 1.0,
                            effects: Vec::new(),
                            children: vec![PositionedElement::Line {
                                start: Point { x: 0.0, y: 0.0 },
                                end: Point { x: width, y: 0.0 },
                                width: 0.5,
                                color,
                                dash_pattern: None,
                            }],
                        },
                    });
                } else if let Some(note) = num_state.note_body_context
                    && ((note.stream == NoteStream::Footnote && event == "footnoteRef")
                        || (note.stream == NoteStream::Endnote && event == "endnoteRef"))
                {
                    let text = num_state.note_label(note);
                    let (size, rise) = if matches!(
                        effective_rpr.vert_align.as_deref(),
                        Some("superscript" | "subscript")
                    ) {
                        (font_size, baseline_offset)
                    } else {
                        (font_size * 0.58, font_size * 0.33 + baseline_offset)
                    };
                    let marker_font =
                        fm.resolve_font_for_text(font_family.as_deref(), bold, italic, &text)?;
                    let shaped = fm.shape_text_with_kerning(
                        marker_font,
                        &text,
                        size,
                        style_resolver::word_kerning_enabled(&effective_rpr),
                    )?;
                    let marker_metrics = fm.metrics(marker_font, size)?;
                    inline_items.push(InlineItem::Text(TextSegment {
                        form_mark: current_form_mark,
                        text,
                        direction: TextDirection::Auto,
                        source: None,
                        font_id: marker_font,
                        font_size: size,
                        glyph_ids: shaped.glyph_ids,
                        advances: shaped.advances,
                        width: shaped.width,
                        ascent: marker_metrics.ascent,
                        descent: marker_metrics.descent,
                        line_gap: 0.0,
                        color,
                        bold,
                        italic,
                        underline,
                        strike,
                        dstrike,
                        highlight,
                        baseline_offset: rise,
                        hyperlink_url: current_hyperlink_url.clone(),
                        field_kind: None,
                        field_source: None,
                        note: None,
                        note_reference_source: None,
                    }));
                }
            }
            let Some(content) = content else {
                break;
            };
            let content_char_start = projected_run_start + content_char_starts[content_index];
            match content {
                RunContent::Text(ct_text) | RunContent::DeletedText(ct_text) => {
                    let text = if effective_rpr.caps == Some(true) {
                        ct_text.text.to_uppercase()
                    } else {
                        ct_text.text.clone()
                    };

                    if text.is_empty() && stored_source.is_none() && stored_field_kind.is_none() {
                        continue;
                    }

                    let source = if !stored_cache && text == ct_text.text {
                        source_node.and_then(|node| {
                            let char_start = u32::try_from(content_char_start).ok()?;
                            let char_end =
                                u32::try_from(content_char_start + ct_text.text.chars().count())
                                    .ok()?;
                            Some(SourceSpan {
                                node,
                                char_start,
                                char_end,
                            })
                        })
                    } else {
                        None
                    };

                    let annotation_base = AnnotationBase {
                        fallback_emphasis: effective_rpr.emphasis_mark.as_ref(),
                        occurrence: stored_occurrence,
                        field_kind: stored_field_kind,
                        kern: style_resolver::word_kerning_enabled(&effective_rpr),
                        font_id,
                        font_size,
                        color,
                        bold,
                        italic,
                        baseline_offset,
                        spacing: word_character_advance_pt(&effective_rpr, grid_char_space_pt),
                        underline,
                        strike,
                        dstrike,
                        highlight,
                        font_family: font_family.as_deref(),
                    };

                    if text.is_empty() && stored_occurrence.is_some() {
                        let kind = effective_rpr
                            .east_asian_layout
                            .as_deref()
                            .filter(|layout| {
                                layout.combine == Some(true) || layout.vert == Some(true)
                            })
                            .map(|layout| WordAnnotationKind::EastAsian(layout.clone()))
                            .or_else(|| {
                                effective_rpr
                                    .emphasis_mark
                                    .as_ref()
                                    .filter(|mark| emphasis_mark_glyph(mark).is_some())
                                    .map(|mark| WordAnnotationKind::Emphasis(mark.clone()))
                            });
                        if let Some(kind) = kind {
                            push_empty_field_annotation(
                                &mut inline_items,
                                &mut multilingual_styles,
                                kind,
                                &annotation_base,
                                source,
                            )?;
                            continue;
                        }
                    }

                    if let Some(layout) = effective_rpr.east_asian_layout.as_deref()
                        && push_east_asian_layout_text(
                            &mut inline_items,
                            &mut multilingual_styles,
                            fm,
                            layout,
                            &text,
                            &annotation_base,
                            source,
                        )?
                    {
                        continue;
                    }

                    if let Some(ref mark) = effective_rpr.emphasis_mark
                        && push_emphasis_marked_text(
                            &mut inline_items,
                            &mut multilingual_styles,
                            fm,
                            diagnostics,
                            mark,
                            &text,
                            &annotation_base,
                        )?
                    {
                        continue;
                    }

                    let mut shaped = fm.shape_text_with_kerning(
                        font_id,
                        &text,
                        font_size,
                        style_resolver::word_kerning_enabled(&effective_rpr),
                    )?;

                    // Apply character spacing from run properties, plus the
                    // section character grid when one is in force.
                    let extra = word_character_advance_pt(&effective_rpr, grid_char_space_pt);
                    if extra != 0.0 {
                        for advance in &mut shaped.advances {
                            *advance += extra;
                        }
                        shaped.width += extra * shaped.advances.len() as f64;
                    }

                    let segment = TextSegment {
                        form_mark: current_form_mark,
                        text,
                        direction: TextDirection::Auto,
                        source,
                        font_id,
                        font_size,
                        glyph_ids: shaped.glyph_ids,
                        advances: shaped.advances,
                        width: shaped.width,
                        ascent: metrics.ascent,
                        descent: metrics.descent,
                        line_gap: 0.0,
                        color,
                        bold,
                        italic,
                        underline,
                        strike,
                        dstrike,
                        highlight,
                        baseline_offset,
                        hyperlink_url: current_hyperlink_url.clone(),
                        field_kind: stored_field_kind,
                        field_source: stored_source,
                        note: None,
                        note_reference_source: None,
                    };
                    let item_index = inline_items.len();
                    multilingual_styles.insert(
                        item_index,
                        WordMultilingualStyle {
                            occurrence: stored_occurrence,
                            annotation: None,
                            kern: style_resolver::word_kerning_enabled(&effective_rpr),
                            origin: if stored_field_kind.is_some() {
                                WordInputOrigin::PlainField
                            } else {
                                WordInputOrigin::Text
                            },
                            requested_family: fm.copy_font_request_context(
                                font_family.as_deref(),
                                &mut requested_context_bytes,
                                &mut requested_context_entries,
                            ),
                            language: effective_rpr.language.clone(),
                            language_east_asia: effective_rpr.language_east_asia.clone(),
                            language_bidi: effective_rpr.language_bidi.clone(),
                            direction: word_text_direction(effective_rpr.rtl),
                            spacing: word_character_advance_pt(&effective_rpr, grid_char_space_pt),
                        },
                    );
                    if automatic_hyphenation
                        && !stored_cache
                        && let Some(language) = effective_rpr.language.as_ref()
                    {
                        inline_items.push(InlineItem::HyphenatedText {
                            segment,
                            language: language.clone(),
                        });
                    } else {
                        inline_items.push(InlineItem::Text(segment));
                    }
                }
                RunContent::Tab => {
                    inline_items.push(InlineItem::Tab);
                }
                // Word drops a page or column break inside a table cell, so
                // the text on either side of it joins on one line.
                RunContent::Break(bt) => match bt {
                    BreakType::Line => inline_items.push(InlineItem::LineBreak),
                    BreakType::Page | BreakType::Column if in_table_cell => {}
                    BreakType::Page => inline_items.push(InlineItem::PageBreak),
                    BreakType::Column => inline_items.push(InlineItem::ColumnBreak),
                },
                RunContent::Drawing(drawing) => {
                    if let Some(ref inline) = drawing.inline {
                        let width = inline.extent_cx.to_pt();
                        let height = inline.extent_cy.to_pt();
                        let item = if let Some(relationship_id) = inline.chart_rel_id.as_deref() {
                            InlineItem::Group {
                                width,
                                height,
                                baseline: None,
                                math: false,
                                group: render_word_chart(
                                    relationship_id,
                                    width,
                                    height,
                                    input,
                                    fm,
                                    diagnostics,
                                )?,
                            }
                        } else {
                            media.record_picture_draw_owner(&inline.embed_id);
                            InlineItem::Image {
                                form_mark: current_form_mark,
                                width,
                                height,
                                media_id: media.id_for_relationship_with_diagnostic(
                                    &inline.embed_id,
                                    diagnostics,
                                ),
                            }
                        };
                        if let Some(alternate_text) = inline
                            .description
                            .as_deref()
                            .map(str::trim)
                            .filter(|text| !text.is_empty())
                        {
                            inline_items.push(InlineItem::Figure {
                                item: Box::new(item),
                                alternate_text: alternate_text.to_owned(),
                                structure_id: None,
                            });
                        } else {
                            inline_items.push(item);
                        }
                    }
                }
                RunContent::Field(field) => {
                    push_sequence_context_markers(
                        field,
                        input.sequence_snapshot.as_deref().zip(source_node),
                        &field_indices,
                        font_id,
                        &mut inline_items,
                        &mut multilingual_styles,
                    );
                    let (computed_value, field_kind) = computed_fields
                        .get(&std::ptr::from_ref(field))
                        .cloned()
                        .unwrap_or((None, None));
                    // A legacy form field outside any control field is a
                    // field of its own.
                    let field_form_mark = current_form_mark.or_else(|| {
                        let form = form_paragraph?;
                        if !crate::forms::is_mapped_legacy(field) {
                            return None;
                        }
                        let index = field_indices.get(&std::ptr::from_ref(field)).copied()?;
                        Some(oxoffice_layout::FormMark {
                            node: form.node,
                            field: crate::forms::legacy_mark(index),
                        })
                    });
                    let stored_segments = field.effective_display_field_segments();
                    // A legacy checkbox, or a drop-down that stores no result,
                    // takes the formatting of the run that opens it when it has
                    // no formatted result runs.
                    let drawn_form_properties = (drawn_legacy_checkbox(field).is_some()
                        || drawn_legacy_dropdown(field).is_some())
                    .then(|| field.begin_run_properties())
                    .flatten();
                    let segments: Vec<(
                        Cow<'_, str>,
                        Option<&CT_RPr>,
                        &oxoffice_docx_oxml::text::Field,
                    )> = if let Some(value) = computed_value.as_deref() {
                        let stored_properties = stored_segments
                            .first()
                            .and_then(|(_, properties, _)| *properties)
                            .or(drawn_form_properties.as_ref());
                        vec![(Cow::Borrowed(value), stored_properties, field)]
                    } else {
                        stored_segments
                    };
                    for (value, stored_properties, display_owner) in segments {
                        let value = value.as_ref();
                        let segment_field_source = source_node
                            .zip(
                                field_indices
                                    .get(&std::ptr::from_ref(display_owner))
                                    .copied(),
                            )
                            .map(|(node, index)| FieldSource { node, index });
                        let segment_field_kind = if std::ptr::eq(display_owner, field) {
                            field_kind
                        } else if field.cached_display_owner_is_locked(display_owner) {
                            None
                        } else {
                            match display_owner.instruction.name.as_str() {
                                "PAGE" => Some(FieldKind::Page),
                                "NUMPAGES" => Some(FieldKind::NumPages),
                                "SECTION" => Some(FieldKind::Section),
                                "SECTIONPAGES" => Some(FieldKind::SectionPages),
                                "PAGEREF"
                                    if !page_reference_keeps_stored_display(
                                        &display_owner.cached_result,
                                    ) =>
                                {
                                    field_text_argument(&display_owner.instruction, 0)
                                        .and_then(|name| page_ref_id(input, name))
                                        .map(FieldKind::TargetPage)
                                }
                                _ => None,
                            }
                        };
                        let segment_style_id = stored_properties
                            .and_then(|properties| properties.style_id.as_deref())
                            .filter(|style_id| {
                                !(toc_entry && is_hyperlink_character_style(style_id, styles))
                            });
                        let mut segment_rpr = if stored_properties.is_some() {
                            style_resolver::resolve_run_properties(
                                para_style_id,
                                segment_style_id,
                                styles,
                                table_run_properties,
                            )
                        } else {
                            effective_rpr.clone()
                        };
                        if let Some(properties) = stored_properties {
                            segment_rpr.merge_from(properties);
                        }
                        if computed_value.is_some() && drawn_legacy_checkbox(field).is_some() {
                            // The box's own size, unless automatic.
                            if let Some(oxoffice_docx_oxml::text::LegacyCheckBoxSize::HalfPoints(
                                size,
                            )) = field
                                .legacy_form
                                .as_ref()
                                .and_then(|data| data.checkbox_size)
                            {
                                let size = oxoffice_docx_oxml::HalfPoint(
                                    u32::try_from(size).unwrap_or(u32::MAX),
                                );
                                segment_rpr.sz = Some(size);
                                segment_rpr.sz_cs = Some(size);
                            }
                        }
                        if segment_rpr.vanish == Some(true) {
                            continue;
                        }
                        let mut segment_font_size =
                            segment_rpr.sz.map(|hp| hp.to_pt()).unwrap_or(11.0);
                        let segment_bold = segment_rpr.bold.unwrap_or(false);
                        let segment_italic = segment_rpr.italic.unwrap_or(false);
                        let segment_font_family = resolve_font_family(
                            &segment_rpr,
                            input.theme.as_ref(),
                            word_font_slot_for_text(value, segment_rpr.font_hint.as_deref()),
                        );
                        let segment_highlight = run_fill(&segment_rpr);
                        let segment_automatic_color = segment_highlight
                            .map_or(automatic_color, |fill| automatic_text_color(Some(fill)));
                        let (segment_color, segment_underline) = tracked_marks(
                            &projected,
                            input.revision_style,
                            resolve_run_color(
                                &segment_rpr,
                                input.theme.as_ref(),
                                segment_automatic_color,
                            ),
                            convert::underline(segment_rpr.underline),
                        );
                        let segment_strike =
                            projected.force_strike || segment_rpr.strike.unwrap_or(false);
                        let segment_dstrike = segment_rpr.dstrike.unwrap_or(false);
                        let mut segment_baseline_offset = 0.0;
                        if let Some(vertical) = segment_rpr.vert_align.as_deref() {
                            match vertical {
                                "superscript" => {
                                    let original_size = segment_font_size;
                                    segment_font_size *= 0.58;
                                    segment_baseline_offset = original_size * 0.33;
                                }
                                "subscript" => {
                                    let original_size = segment_font_size;
                                    segment_font_size *= 0.58;
                                    segment_baseline_offset = -(original_size * 0.14);
                                }
                                _ => {}
                            }
                        }
                        if let Some(position) = segment_rpr.position {
                            segment_baseline_offset += position as f64 / 2.0;
                        }
                        let segment_font_id = fm.resolve_font_for_text(
                            segment_font_family.as_deref(),
                            segment_bold,
                            segment_italic,
                            value,
                        )?;
                        let segment_metrics = fm.metrics(segment_font_id, segment_font_size)?;

                        let mut start = 0usize;
                        for (index, character) in value
                            .char_indices()
                            .chain(std::iter::once((value.len(), '\0')))
                        {
                            let control = match character {
                                '\t' => Some(InlineItem::Tab),
                                '\n' => Some(InlineItem::LineBreak),
                                '\u{000c}' => Some(InlineItem::PageBreak),
                                '\u{000b}' => Some(InlineItem::ColumnBreak),
                                '\0' if index == value.len() => None,
                                _ => continue,
                            };
                            if start < index
                                || (value.is_empty()
                                    && segment_field_kind == Some(FieldKind::SequenceRepeat))
                            {
                                let mut text = value[start..index].to_owned();
                                if segment_rpr.caps == Some(true) {
                                    text = text.to_uppercase();
                                }
                                let mut shaped = fm.shape_text_with_kerning(
                                    segment_font_id,
                                    &text,
                                    segment_font_size,
                                    style_resolver::word_kerning_enabled(&segment_rpr),
                                )?;
                                let extra =
                                    word_character_advance_pt(&segment_rpr, grid_char_space_pt);
                                if extra != 0.0 {
                                    for advance in &mut shaped.advances {
                                        *advance += extra;
                                    }
                                    shaped.width += extra * shaped.advances.len() as f64;
                                }
                                let item_index = inline_items.len();
                                multilingual_styles.insert(
                                    item_index,
                                    WordMultilingualStyle {
                                        occurrence: None,
                                        annotation: None,
                                        kern: style_resolver::word_kerning_enabled(&segment_rpr),
                                        origin: if segment_field_kind.is_some() {
                                            WordInputOrigin::PlainField
                                        } else {
                                            WordInputOrigin::Text
                                        },
                                        requested_family: fm.copy_font_request_context(
                                            segment_font_family.as_deref(),
                                            &mut requested_context_bytes,
                                            &mut requested_context_entries,
                                        ),
                                        language: segment_rpr.language.clone(),
                                        language_east_asia: segment_rpr.language_east_asia.clone(),
                                        language_bidi: segment_rpr.language_bidi.clone(),
                                        direction: word_text_direction(segment_rpr.rtl),
                                        spacing: word_character_advance_pt(
                                            &segment_rpr,
                                            grid_char_space_pt,
                                        ),
                                    },
                                );
                                inline_items.push(InlineItem::Text(TextSegment {
                                    form_mark: field_form_mark,
                                    text,
                                    direction: word_text_direction(segment_rpr.rtl),
                                    source: None,
                                    font_id: segment_font_id,
                                    font_size: segment_font_size,
                                    glyph_ids: shaped.glyph_ids,
                                    advances: shaped.advances,
                                    width: shaped.width,
                                    ascent: segment_metrics.ascent,
                                    descent: segment_metrics.descent,
                                    line_gap: 0.0,
                                    color: segment_color,
                                    bold: segment_bold,
                                    italic: segment_italic,
                                    underline: segment_underline,
                                    strike: segment_strike,
                                    dstrike: segment_dstrike,
                                    highlight: segment_highlight,
                                    baseline_offset: segment_baseline_offset,
                                    hyperlink_url: current_hyperlink_url.clone(),
                                    field_kind: segment_field_kind,
                                    field_source: segment_field_source,
                                    note: None,
                                    note_reference_source: None,
                                }));
                            }
                            if let Some(control) = control {
                                let dropped = in_table_cell
                                    && matches!(
                                        control,
                                        InlineItem::PageBreak | InlineItem::ColumnBreak
                                    );
                                if !dropped {
                                    inline_items.push(control);
                                }
                                start = index + character.len_utf8();
                            }
                        }
                    }
                }
                RunContent::FootnoteRef { id, .. } | RunContent::EndnoteRef { id, .. } => {
                    // The two streams number independently, so the marker has
                    // to carry which one it came from.
                    let stream = match content {
                        RunContent::EndnoteRef { .. } => NoteStream::Endnote,
                        _ => NoteStream::Footnote,
                    };
                    // Render as superscript number
                    let marker = num_state
                        .note_label(NoteRef { stream, id: *id })
                        .to_string();
                    // Explicit vertical alignment has already resolved size and
                    // rise above. Default note superscripting must happen once,
                    // while retaining an explicit half-point position shift.
                    let (sup_size, sup_offset) = if matches!(
                        effective_rpr.vert_align.as_deref(),
                        Some("superscript" | "subscript")
                    ) {
                        (font_size, baseline_offset)
                    } else {
                        (font_size * 0.58, font_size * 0.33 + baseline_offset)
                    };
                    let shaped = fm.shape_text_with_kerning(
                        font_id,
                        &marker,
                        sup_size,
                        style_resolver::word_kerning_enabled(&effective_rpr),
                    )?;
                    let sup_metrics = fm.metrics(font_id, sup_size)?;
                    let revision_marker = input.revision_view == RevisionView::Tracked
                        && projected.ordinary_run_index.is_none();
                    inline_items.push(InlineItem::Text(TextSegment {
                        form_mark: None,
                        text: marker,
                        direction: TextDirection::Auto,
                        source: None,
                        font_id,
                        font_size: sup_size,
                        glyph_ids: shaped.glyph_ids,
                        advances: shaped.advances,
                        width: shaped.width,
                        ascent: sup_metrics.ascent,
                        descent: sup_metrics.descent,
                        line_gap: 0.0,
                        color,
                        bold,
                        italic,
                        underline: revision_marker.then_some(underline).flatten(),
                        strike: revision_marker && strike,
                        dstrike: revision_marker && dstrike,
                        highlight: revision_marker.then_some(highlight).flatten(),
                        baseline_offset: sup_offset,
                        hyperlink_url: None,
                        field_kind: None,
                        field_source: None,
                        note: Some(NoteRef { stream, id: *id }),
                        note_reference_source: source_node.and_then(|node| {
                            let offset = u32::try_from(if stored_cache {
                                projected_run_start
                            } else {
                                content_char_start
                            })
                            .ok()?;
                            Some(SourceSpan {
                                node,
                                char_start: offset,
                                char_end: offset,
                            })
                        }),
                    }));
                }
                RunContent::Symbol { font, char_code } => {
                    // The code point is the symbol font's own, typically in
                    // the F020 to F0FF private-use block Word writes. Font
                    // resolution runs against that text so a family without
                    // the glyph is replaced by one that has it.
                    let Some(symbol) = char::from_u32(u32::from(*char_code)) else {
                        continue;
                    };
                    let (text, symbol_font_id) = fm.resolve_word_symbol_text(
                        Some(font.as_str()),
                        bold,
                        italic,
                        &symbol.to_string(),
                        false,
                    )?;
                    let symbol_metrics = fm.metrics(symbol_font_id, font_size)?;
                    let shaped = fm.shape_text_with_kerning(
                        symbol_font_id,
                        &text,
                        font_size,
                        style_resolver::word_kerning_enabled(&effective_rpr),
                    )?;
                    for message in fm.unknown_word_symbol_diagnostics(
                        symbol_font_id,
                        Some(font),
                        &symbol.to_string(),
                        &text,
                    ) {
                        push_unique_diagnostic(diagnostics, message);
                    }
                    multilingual_styles.insert(
                        inline_items.len(),
                        WordMultilingualStyle {
                            occurrence: stored_occurrence,
                            annotation: None,
                            kern: style_resolver::word_kerning_enabled(&effective_rpr),
                            origin: if stored_field_kind.is_some() {
                                WordInputOrigin::PlainField
                            } else {
                                WordInputOrigin::Text
                            },
                            requested_family: fm.copy_font_request_context(
                                Some(font),
                                &mut requested_context_bytes,
                                &mut requested_context_entries,
                            ),
                            ..Default::default()
                        },
                    );
                    inline_items.push(InlineItem::Text(TextSegment {
                        form_mark: current_form_mark,
                        text,
                        direction: TextDirection::Auto,
                        source: None,
                        font_id: symbol_font_id,
                        font_size,
                        glyph_ids: shaped.glyph_ids,
                        advances: shaped.advances,
                        width: shaped.width,
                        ascent: symbol_metrics.ascent,
                        descent: symbol_metrics.descent,
                        line_gap: 0.0,
                        color,
                        bold,
                        italic,
                        underline,
                        strike,
                        dstrike,
                        highlight,
                        baseline_offset,
                        hyperlink_url: current_hyperlink_url.clone(),
                        field_kind: None,
                        field_source: None,
                        note: None,
                        note_reference_source: None,
                    }));
                }
                RunContent::SpecialCharacter(character) => match character {
                    SpecialCharacter::CarriageReturn => {
                        inline_items.push(InlineItem::LineBreak);
                    }
                    SpecialCharacter::NoBreakHyphen => {
                        // U+2011 carries Unicode line-break class GL, so the
                        // hyphen is drawn without becoming a break
                        // opportunity. A plain U+002D would become one.
                        let text = "\u{2011}".to_owned();
                        let shaped = fm.shape_text_with_kerning(
                            font_id,
                            &text,
                            font_size,
                            style_resolver::word_kerning_enabled(&effective_rpr),
                        )?;
                        inline_items.push(InlineItem::Text(TextSegment {
                            form_mark: current_form_mark,
                            text,
                            direction: TextDirection::Auto,
                            source: None,
                            font_id,
                            font_size,
                            glyph_ids: shaped.glyph_ids,
                            advances: shaped.advances,
                            width: shaped.width,
                            ascent: metrics.ascent,
                            descent: metrics.descent,
                            line_gap: 0.0,
                            color,
                            bold,
                            italic,
                            underline,
                            strike,
                            dstrike,
                            highlight,
                            baseline_offset,
                            hyperlink_url: current_hyperlink_url.clone(),
                            field_kind: None,
                            field_source: None,
                            note: None,
                            note_reference_source: None,
                        }));
                    }
                    // A soft hyphen is drawn only on the line it breaks, and
                    // the line breaker has no discretionary-break input, so it
                    // is round-tripped without a render projection.
                    SpecialCharacter::SoftHyphen => {}
                    // The absolute position is a paragraph-relative tab stop
                    // the tab resolver already places.
                    SpecialCharacter::PositionalTab { .. } => {
                        inline_items.push(InlineItem::Tab);
                    }
                },
                RunContent::CommentReference { .. } => {}
            }
        }
        for index in run_inline_start..inline_items.len() {
            multilingual_styles
                .entry(index)
                .or_insert_with(|| WordMultilingualStyle {
                    kern: style_resolver::word_kerning_enabled(&effective_rpr),
                    requested_family: fm.copy_font_request_context(
                        font_family.as_deref(),
                        &mut requested_context_bytes,
                        &mut requested_context_entries,
                    ),
                    ..Default::default()
                });
        }
    }

    let mut equation_rpr =
        style_resolver::resolve_run_properties(para_style_id, None, styles, table_run_properties);
    if let Some(paragraph_mark_rpr) = direct_ppr.and_then(|ppr| ppr.rpr.as_ref()) {
        equation_rpr.merge_from(paragraph_mark_rpr);
    }
    push_equations_before_order(
        para,
        usize::MAX,
        RawOrder::AfterRaw,
        &mut equation_cursor,
        &mut inline_items,
        fm,
        equation_rpr.sz.map(|hp| hp.to_pt()).unwrap_or(11.0),
        resolve_run_color(&equation_rpr, input.theme.as_ref(), automatic_color),
        available_width,
        input.math_properties.as_ref(),
        &mut display_break,
        diagnostics,
    )?;

    push_targeted_bookmark_markers(
        &mut inline_items,
        &mut multilingual_styles,
        &current_bookmarks,
        projected_run_count,
        input,
        fm,
        source_node,
    )?;
    drop_trailing_display_break(&mut inline_items, &mut multilingual_styles, display_break);

    let attributed_empty_paragraph = inline_items.is_empty()
        || inline_items.iter().enumerate().all(|(index, _)| {
            multilingual_styles
                .get(&index)
                .and_then(|style| style.annotation.as_ref())
                .is_some_and(|annotation| annotation.kind == WordAnnotationRootKind::EmptySpan)
        });
    if attributed_empty_paragraph {
        let mut caret_rpr = style_resolver::resolve_run_properties(
            para_style_id,
            None,
            styles,
            table_run_properties,
        );
        if let Some(paragraph_mark_rpr) = direct_ppr.and_then(|ppr| ppr.rpr.as_ref()) {
            caret_rpr.merge_from(paragraph_mark_rpr);
        }
        let font_size = caret_rpr.sz.map(|hp| hp.to_pt()).unwrap_or(11.0);
        let bold = caret_rpr.bold.unwrap_or(false);
        let italic = caret_rpr.italic.unwrap_or(false);
        // The caret of an empty paragraph draws no character, so it has no
        // slot of its own and keeps the ASCII one it has always used.
        let font_family =
            resolve_font_family(&caret_rpr, input.theme.as_ref(), WordFontSlot::Ascii);
        let font_id = fm.resolve_font_for_metrics(font_family.as_deref(), bold, italic)?;
        let metrics = fm.metrics(font_id, font_size)?;
        if inline_items.is_empty() {
            multilingual_styles.insert(
                inline_items.len(),
                WordMultilingualStyle {
                    kern: style_resolver::word_kerning_enabled(&caret_rpr),
                    ..Default::default()
                },
            );
            inline_items.push(InlineItem::Text(TextSegment {
                form_mark: None,
                text: String::new(),
                direction: TextDirection::Auto,
                source: source_node.map(|node| SourceSpan {
                    node,
                    char_start: 0,
                    char_end: 0,
                }),
                font_id,
                font_size,
                glyph_ids: Vec::new(),
                advances: Vec::new(),
                width: 0.0,
                ascent: metrics.ascent,
                descent: metrics.descent,
                line_gap: 0.0,
                color: resolve_run_color(&caret_rpr, input.theme.as_ref(), automatic_color),
                bold,
                italic,
                underline: None,
                strike: false,
                dstrike: false,
                highlight: None,
                baseline_offset: 0.0,
                hyperlink_url: None,
                field_kind: None,
                field_source: None,
                note: None,
                note_reference_source: None,
            }));
        } else {
            // These atomic roots stand in for the ordinary empty caret. A
            // baseline-bearing Group retains its supplied metrics during Word
            // restoration, so include the same external leading here. Exact
            // spacing retains the line breaker's original font metrics.
            let (ascent, descent) = if matches!(
                convert::line_spacing(&effective_ppr),
                oxoffice_layout::LineSpacing::Exact(_)
            ) {
                (metrics.ascent, metrics.descent)
            } else {
                let word = fm.word_line_metrics(font_id, font_size)?;
                (word.ascent + word.line_gap, word.descent)
            };
            for item in &mut inline_items {
                let InlineItem::Group {
                    height,
                    baseline,
                    group,
                    ..
                } = item
                else {
                    return Err(LayoutError::Layout(
                        "empty annotation lost its atomic root".into(),
                    ));
                };
                *height = ascent + descent;
                *baseline = Some(ascent);
                let [PositionedElement::Text(base)] = group.children.as_mut_slice() else {
                    return Err(LayoutError::Layout(
                        "empty annotation lost its base leaf".into(),
                    ));
                };
                base.origin.y = ascent;
            }
        }
    }

    let layout_direction = match base_direction {
        TextDirection::Auto => inferred_word_base_direction(&inline_items),
        direction => direction,
    };
    if let Some(reflow_direction_out) = reflow_direction_out {
        *reflow_direction_out = layout_direction;
    }
    let logical_left = match layout_direction {
        TextDirection::RightToLeft => effective_ppr.ind_end,
        TextDirection::Auto | TextDirection::LeftToRight => effective_ppr.ind_start,
    };
    let logical_right = match layout_direction {
        TextDirection::RightToLeft => effective_ppr.ind_start,
        TextDirection::Auto | TextDirection::LeftToRight => effective_ppr.ind_end,
    };
    let ind_left = effective_ppr
        .ind_left
        .or(logical_left)
        .map(|t| t.to_pt())
        .unwrap_or(0.0);
    let ind_right = effective_ppr
        .ind_right
        .or(logical_right)
        .map(|t| t.to_pt())
        .unwrap_or(0.0);
    let jc = convert::alignment_for_direction(effective_ppr.jc, layout_direction);

    // Line breaking
    let mut line_params =
        convert::line_break_params(&effective_ppr, available_width, input.default_tab_stop);
    line_params.clamp_tabs_past_margin = input.clamp_tabs_past_margin;
    line_params.ind_left = ind_left;
    line_params.ind_right = ind_right;
    line_params.jc = jc;

    // A line holding only inline objects takes its proportional spacing from
    // the paragraph mark, whose run properties `equation_rpr` already are. A
    // mark whose font does not resolve adds none rather than failing a
    // paragraph whose runs all resolved.
    let paragraph_mark = {
        let family = resolve_font_family(&equation_rpr, input.theme.as_ref(), WordFontSlot::Ascii);
        fm.resolve_font_for_metrics(
            family.as_deref(),
            equation_rpr.bold.unwrap_or(false),
            equation_rpr.italic.unwrap_or(false),
        )
        .and_then(|font_id| {
            fm.word_line_metrics(font_id, equation_rpr.sz.map_or(11.0, |hp| hp.to_pt()))
        })
        .ok()
    };
    // An exact `w:lineRule` is an author's absolute height and stays off the
    // grid, which is what Word does with the two together.
    let grid_line_pitch_pt =
        grid_line_pitch_pt.filter(|_| effective_ppr.line_rule.as_deref() != Some("exact"));

    let uses_multilingual_layout = base_direction != TextDirection::Auto
        || multilingual_styles
            .values()
            .any(|style| style.direction != TextDirection::Auto)
        || inline_items.iter().any(|item| {
            multilingual_candidate(item)
                .is_some_and(|segment| needs_word_multilingual_layout(&segment.text))
        });
    let uses_exact_word_baseline = uses_multilingual_layout
        && effective_ppr.line_rule.as_deref() == Some("exact")
        && effective_ppr.line_spacing.is_some();
    let structural_count = multilingual_styles
        .values()
        .filter(|style| style.origin == WordInputOrigin::Structural)
        .count();
    let mut producers = Vec::new();
    let mut language_bytes = None;
    let mut uniform_language_bytes = true;
    for (index, item) in inline_items.iter().enumerate() {
        if let InlineItem::Text(_) | InlineItem::Marker(_) | InlineItem::HyphenatedText { .. } =
            item
        {
            let style = multilingual_styles
                .get(&index)
                .ok_or_else(|| LayoutError::Layout("Word text lacks producer proof".into()))?;
            let language = match item {
                InlineItem::HyphenatedText { language, .. } => Some(language.clone()),
                _ => None,
            };
            let bytes = language.as_ref().map_or(0, String::len);
            if language_bytes.is_some_and(|previous| previous != bytes) {
                uniform_language_bytes = false;
            }
            language_bytes = Some(bytes);
            producers.push(WordLegacyProducer {
                kern: style.kern,
                language,
                structural: style.origin == WordInputOrigin::Structural,
                atomic_marker: style.origin == WordInputOrigin::AtomicMarker,
            });
        }
    }
    let mut paint = WordParagraphPaint {
        rich_layout: uses_multilingual_layout,
        plain_field_policy: Some((false, true)),
        structural_count,
        legacy_cache: (!uses_multilingual_layout && uniform_language_bytes).then_some(
            WordLegacyCache {
                language_bytes_per_span: language_bytes.unwrap_or(0),
                producers,
            },
        ),
        ..Default::default()
    };
    let shaped = shape_word_multilingual_items(
        fm,
        inline_items,
        &multilingual_styles,
        layout_direction,
        !line_params.wrap,
        uses_exact_word_baseline,
        !uses_multilingual_layout,
    )?;
    inline_items = shaped.items;
    paint.input_annotations = shaped.annotations;
    paint.field_occurrences = shaped.occurrences;
    let (mut lines, origins) = oxoffice_layout::line::break_into_lines_with_opaque_origins(
        &inline_items,
        &line_params,
        fm,
        uses_multilingual_layout.then_some(layout_direction),
    )?;
    paint.bind_annotation_origins(&lines, &origins)?;
    let actual_structural = lines
        .iter()
        .flat_map(|line| &line.items)
        .filter(|item| match item {
            LineItem::Text(base) | LineItem::Marker(base) => {
                base.field_kind.is_some() || base.field_source.is_some()
            }
            _ => false,
        })
        .count();
    if actual_structural != paint.structural_count {
        return Err(LayoutError::Layout(
            "Word structural producer count changed in line breaking".into(),
        ));
    }
    convert::restore_word_line_heights(
        &mut lines,
        line_params.line_spacing,
        grid_line_pitch_pt,
        fm,
        paragraph_mark,
    );

    let mut result = block::build_paragraph_block(
        lines,
        space_before,
        space_after,
        effective_ppr.borders.map(|borders| *borders),
        shading,
        ind_left,
        ind_right,
        jc,
        keep_next,
        keep_lines,
        page_break_before,
        widow_control,
    );
    result.heading_level = heading_level;
    result.heading_text = heading_level.map(|_| heading_text);
    result.change_bar = change_bar_color(para, input);
    result.list = list_num_id.map(|num_id| (num_id, list_ilvl.min(8) as u8));
    let (anchored, anchors) = collect_anchored_drawings(
        para,
        styles,
        input,
        media,
        fm,
        num_state,
        diagnostics,
        source_node,
    )?;
    result.anchored = anchored;
    paint.anchors = anchors;
    // `inline_items` is finished with here and would otherwise be dropped, so
    // handing it to the reflow costs nothing but the memory it already holds.
    // `Engine::layout` frees it again unless the document wraps.
    result.reflow = Some(Box::new(block::ParagraphReflow {
        items: inline_items,
        params: line_params,
        grid_line_pitch_pt,
        paragraph_mark,
    }));
    Ok((result, paint))
}

/// Remove the line break that closes a display equation when nothing that
/// draws follows it in the paragraph.
///
/// A display equation stands on a line of its own, so it ends the line it is
/// on. At the end of the paragraph Word opens no further line: the paragraph
/// ends on the equation's line. Kept, the break would give the paragraph an
/// empty last line, measured like the equation line before it.
fn drop_trailing_display_break(
    items: &mut Vec<InlineItem>,
    styles: &mut HashMap<usize, WordMultilingualStyle>,
    display_break: Option<usize>,
) {
    let Some(index) = display_break else {
        return;
    };
    if !matches!(items.get(index), Some(InlineItem::LineBreak)) {
        return;
    }
    if !items[index + 1..].iter().all(draws_nothing) {
        return;
    }
    items.remove(index);
    styles.remove(&index);
    // Styles are keyed by item index: the markers after the break move down
    // one place, in ascending order so that each lands on a freed key.
    let mut keys = styles
        .keys()
        .copied()
        .filter(|key| *key > index)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    for key in keys {
        if let Some(style) = styles.remove(&key) {
            styles.insert(key - 1, style);
        }
    }
}

fn push_targeted_bookmark_markers(
    items: &mut Vec<InlineItem>,
    styles: &mut HashMap<usize, WordMultilingualStyle>,
    markers: &[oxoffice_docx_oxml::text::BookmarkMarker],
    projected_run_index: usize,
    input: &LayoutInput,
    fm: &mut FontManager,
    source_node: Option<SourceNodeId>,
) -> Result<()> {
    let mut font_id = None;
    for marker in markers.iter().filter(|marker| {
        let marker_run_index = match input.revision_view {
            RevisionView::Accepted => marker.projected_run_index(),
            RevisionView::Tracked => marker.tracked_run_index(),
        };
        marker.is_start()
            && marker_run_index == projected_run_index
            && marker.name().is_some_and(|name| {
                document_has_page_ref(input, name) && bookmark_text(input, name).is_some()
            })
    }) {
        if let Some(target) = marker.name().and_then(|name| page_ref_id(input, name)) {
            let resolved_font = match font_id {
                Some(font_id) => font_id,
                None => {
                    let resolved = fm.resolve_font_for_text(None, false, false, " ")?;
                    font_id = Some(resolved);
                    resolved
                }
            };
            push_structural_field_marker(
                items,
                styles,
                FieldKind::Target(target),
                resolved_font,
                source_node,
                None,
            );
        }
    }
    Ok(())
}

fn push_sequence_context_markers(
    field: &oxoffice_docx_oxml::text::Field,
    source: Option<(&crate::WordSequenceSnapshot, SourceNodeId)>,
    field_indices: &HashMap<*const oxoffice_docx_oxml::text::Field, u32>,
    font_id: oxoffice_layout::FontId,
    items: &mut Vec<InlineItem>,
    styles: &mut HashMap<usize, WordMultilingualStyle>,
) {
    let Some((snapshot, node)) = source else {
        return;
    };
    let mut pending = vec![field];
    while let Some(owner) = pending.pop() {
        if let Some(index) = field_indices.get(&std::ptr::from_ref(owner)).copied() {
            let source = FieldSource { node, index };
            if let Some(event) = snapshot.event_sources.get(&source) {
                push_structural_field_marker(
                    items,
                    styles,
                    FieldKind::SequenceContext(*event),
                    font_id,
                    Some(node),
                    Some(source),
                );
            }
        }
        pending.extend(owner.nested_fields_in_source_order().into_iter().rev());
    }
}

fn push_structural_field_marker(
    items: &mut Vec<InlineItem>,
    styles: &mut HashMap<usize, WordMultilingualStyle>,
    kind: FieldKind,
    font_id: oxoffice_layout::FontId,
    source_node: Option<SourceNodeId>,
    field_source: Option<FieldSource>,
) {
    styles.insert(
        items.len(),
        WordMultilingualStyle {
            origin: WordInputOrigin::Structural,
            ..Default::default()
        },
    );
    let segment = TextSegment {
        form_mark: None,
        text: "\u{2060}".to_owned(),
        direction: TextDirection::Auto,
        source: source_node.map(|node| oxoffice_layout::SourceSpan {
            node,
            char_start: 0,
            char_end: 0,
        }),
        font_id,
        font_size: 1.0,
        glyph_ids: vec![0],
        advances: vec![0.0],
        width: 0.0,
        ascent: 0.0,
        descent: 0.0,
        line_gap: 0.0,
        color: Color::BLACK,
        bold: false,
        italic: false,
        underline: None,
        strike: false,
        dstrike: false,
        highlight: None,
        baseline_offset: 0.0,
        hyperlink_url: None,
        field_kind: Some(kind),
        field_source,
        note: None,
        note_reference_source: None,
    };
    items.push(if matches!(kind, FieldKind::SequenceContext(_)) {
        InlineItem::Marker(segment)
    } else {
        InlineItem::Text(segment)
    });
}
