//! The engine: construction, the layout transaction, body layout and its caches.

use super::*;

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    fn with_font_manager(font_manager: FontManager) -> Self {
        Self {
            font_manager,
            caller_font_aliases: Vec::new(),
            paragraph_cache_context: None,
            paragraph_cache: VecDeque::new(),
            paragraph_cache_bytes: 0,
            pending_paragraph_cache: None,
            pending_paragraph_cache_bytes: 0,
            paragraph_cache_reads_enabled: false,
            paragraph_cache_notes_match: false,
            retained_context_matches_full: false,
            table_cache: VecDeque::new(),
            table_cache_bytes: 0,
            pending_table_cache: None,
            pending_table_cache_bytes: 0,
            header_footer_cache: VecDeque::new(),
            header_footer_cache_bytes: 0,
            pending_header_footer_cache: None,
            pending_header_footer_cache_bytes: 0,
            header_footer_cache_reads_enabled: false,
            restart_cache: None,
            numbering_by_source: HashMap::new(),
            navigation_by_source: HashMap::new(),
            bookmark_numbering: HashMap::new(),
            bookmark_suppressed: std::collections::HashSet::new(),
            last_body_fragments: Vec::new(),
            last_page_sections: Vec::new(),
            last_field_placements: Vec::new(),
            last_bookmark_pages: HashMap::new(),
            last_field_source_xml: HashMap::new(),
            last_text_box_owner_indices: HashMap::new(),
            last_picture_draw_owners: Vec::new(),
        }
    }

    pub fn new() -> Self {
        Self::with_font_manager(FontManager::new())
    }

    /// Create an engine that resolves fonts without system font discovery.
    pub fn new_deterministic() -> Result<Self> {
        Ok(Self::with_font_manager(FontManager::new_deterministic()?))
    }

    /// Create an engine whose font universe is supplied entirely by the
    /// layout input, without bundled or system-font discovery.
    pub(crate) fn new_with_caller_fonts() -> Self {
        Self::with_font_manager(FontManager::new_with_fonts(Vec::new()))
    }

    /// Set byte-free caller aliases from requested family to loaded family.
    pub fn set_caller_font_aliases(&mut self, aliases: &[(String, String)]) {
        let aliases = bounded_caller_aliases(aliases);
        if self.caller_font_aliases != aliases {
            self.caller_font_aliases = aliases;
        }
    }

    /// Take a reusable engine only when its complete retained-work context
    /// matches the proposed receiver input.
    #[doc(hidden)]
    pub fn take_if_compatible(source: &mut Option<Self>, input: &LayoutInput) -> Option<Self> {
        Self::take_if_compatible_with_caller_aliases(source, input, &[])
    }

    /// Take a reusable engine only when its complete caller-font and alias
    /// context matches the proposed receiver input.
    #[doc(hidden)]
    pub fn take_if_compatible_with_caller_aliases(
        source: &mut Option<Self>,
        input: &LayoutInput,
        caller_font_aliases: &[(String, String)],
    ) -> Option<Self> {
        let caller_font_aliases = bounded_caller_aliases(caller_font_aliases);
        let has_wrapping_drawing = document_has_wrapping_drawing(input);
        let compatible = source.as_ref().is_some_and(|engine| {
            engine.caller_font_aliases == caller_font_aliases
                && engine
                    .paragraph_cache_context
                    .as_ref()
                    .is_some_and(|context| {
                        context.matches_input(input, &caller_font_aliases, has_wrapping_drawing)
                    })
                && engine.pending_paragraph_cache.is_none()
                && engine.pending_header_footer_cache.is_none()
        });
        compatible.then(|| source.take()).flatten()
    }

    /// Lay out the entire document.
    pub fn layout(&mut self, input: &LayoutInput) -> Result<LayoutResult> {
        self.layout_inner(prepared_semantic_layout_input(input)?, None)
    }

    pub(crate) fn measure_content(
        &mut self,
        input: &LayoutInput,
        content: &BodyContent,
        available_width: f64,
        related_story_scope: Option<&str>,
    ) -> Result<(f64, Vec<Diagnostic>)> {
        self.font_manager
            .load_embedded_fonts(&input.embedded_fonts)?;
        self.font_manager.load_additional_fonts(&input.fonts);
        self.font_manager.begin_layout();
        let media = MediaRegistry::new(&input.images);
        let scoped_media = related_story_scope.map(|scope| media.scoped_to_part(scope));
        let media = scoped_media.as_ref().unwrap_or(&media);
        let mut numbering = NumberingState::new();
        let mut diagnostics = Vec::new();
        // Caller-width measurement loads what whole-document layout loads, so
        // it reads the body section's grid rather than measuring ungridded. A
        // related story measures ungridded, because that is what header,
        // footer and note layout does with the same content.
        let doc_grid = if related_story_scope.is_none() {
            input
                .document
                .body
                .sect_pr
                .as_ref()
                .and_then(|sect_pr| sect_pr.doc_grid.as_deref())
        } else {
            None
        };
        let height = match content {
            BodyContent::Paragraph(paragraph) => layout_paragraph_with_grid(
                paragraph,
                available_width,
                &input.styles,
                input,
                media,
                &mut self.font_manager,
                &mut numbering,
                &mut diagnostics,
                doc_grid,
            )?
            .total_height(),
            BodyContent::Table(table) => crate::table::layout_table(
                table,
                available_width,
                &input.styles,
                input,
                media,
                &mut self.font_manager,
                &mut numbering,
                &mut diagnostics,
                doc_grid,
            )?
            .total_height(),
            BodyContent::ContentControl(_) | BodyContent::RawXml(_) => {
                return Err(LayoutError::Layout(
                    "only paragraphs and tables can be measured".to_owned(),
                ));
            }
        };
        diagnostics.extend(self.font_manager.current_font_diagnostics().iter().cloned());
        diagnostics.extend(self.font_manager.word_substitution_diagnostics());
        Ok((height, diagnostics))
    }

    /// Lay out the document and retain its result-local Word source table.
    pub(crate) fn layout_with_provenance(
        &mut self,
        input: &LayoutInput,
    ) -> Result<(LayoutResult, Vec<WordSourcePath>)> {
        let input = prepared_semantic_layout_input(input)?;
        let sources = SourceRegistry::for_input(input.as_ref())?;
        let result = self.layout_inner(input, Some(&sources))?;
        self.last_field_source_xml = sources.field_source_xml;
        self.last_text_box_owner_indices = sources.text_box_owner_indices;
        Ok((result, sources.nodes))
    }

    pub(crate) fn take_field_snapshot(
        &mut self,
    ) -> (
        Vec<crate::WordPageSection>,
        Vec<crate::WordFieldPlacement>,
        HashMap<usize, crate::WordPageSection>,
    ) {
        (
            std::mem::take(&mut self.last_page_sections),
            std::mem::take(&mut self.last_field_placements),
            std::mem::take(&mut self.last_bookmark_pages),
        )
    }

    pub(crate) fn take_picture_draw_owners(&mut self) -> Vec<(String, Option<String>)> {
        std::mem::take(&mut self.last_picture_draw_owners)
    }

    pub(crate) fn take_text_box_owner_indices(&mut self) -> HashMap<WordStory, usize> {
        std::mem::take(&mut self.last_text_box_owner_indices)
    }

    pub(crate) fn take_field_source_xml(&mut self) -> HashMap<FieldSource, Vec<u8>> {
        std::mem::take(&mut self.last_field_source_xml)
    }

    pub(crate) fn take_body_fragments(&mut self) -> Vec<Vec<WordBodyLayoutFragment>> {
        std::mem::take(&mut self.last_body_fragments)
    }

    pub(crate) fn take_bookmark_suppressed(&mut self) -> std::collections::HashSet<String> {
        std::mem::take(&mut self.bookmark_suppressed)
    }

    pub(crate) fn take_bookmark_numbering(&mut self) -> HashMap<String, ResolvedNumbering> {
        std::mem::take(&mut self.bookmark_numbering)
    }

    pub(crate) fn take_navigation(&mut self) -> HashMap<SourceNodeId, crate::ParagraphNavigation> {
        std::mem::take(&mut self.navigation_by_source)
    }

    pub(crate) fn numbering_by_source(
        &self,
        source_count: usize,
    ) -> Vec<Option<ResolvedNumbering>> {
        (0..source_count)
            .map(|index| {
                let id = SourceNodeId::new(u32::try_from(index + 1).expect("source index fits"))
                    .expect("source ids are one based");
                self.numbering_by_source.get(&id).cloned()
            })
            .collect()
    }

    fn layout_inner(
        &mut self,
        mut prepared_input: Cow<'_, LayoutInput>,
        sources: Option<&SourceRegistry>,
    ) -> Result<LayoutResult> {
        let input = prepared_input.as_ref();
        self.navigation_by_source.clear();
        self.numbering_by_source.clear();
        self.bookmark_numbering.clear();
        self.bookmark_suppressed.clear();
        self.last_body_fragments.clear();
        let needs_ref_projection = document_has_ref_projection(input);
        let needs_sequences = document_has_sequence_fields(input);
        let generated_sources = (sources.is_none()
            && (needs_ref_projection
                || needs_sequences
                || input.form_fields
                || document_has_section_fields(input)
                || input
                    .sequence_snapshot
                    .as_ref()
                    .is_some_and(|s| s.navigation_document.is_some())))
        .then(|| SourceRegistry::for_input(input))
        .transpose()?;
        let sources = sources.or(generated_sources.as_ref());
        // Caller hints never establish provenance or freshness. Derive once,
        // before any retained shaping, furniture or restart cache can be read.
        if needs_sequences || needs_ref_projection || input.sequence_snapshot.is_some() {
            let snapshot = if (needs_sequences
                || needs_ref_projection
                || input
                    .sequence_snapshot
                    .as_ref()
                    .is_some_and(|s| s.navigation_document.is_some()))
                && input.revision_view == RevisionView::Accepted
            {
                Some(Arc::new(sequence_snapshot(
                    input,
                    sources.expect("sequence source registry"),
                )?))
            } else {
                None
            };
            // Reuse the semantic-preparation clone when one was required.
            prepared_input.to_mut().sequence_snapshot = snapshot;
        }
        if prepared_input.form_fields
            && let Some(sources) = sources
        {
            let paragraphs = crate::forms::form_paragraphs(prepared_input.as_ref(), sources);
            prepared_input.to_mut().form_paragraphs = Some(Arc::new(paragraphs));
        }
        let input = prepared_input.as_ref();
        // Load user-provided / DOCX-embedded fonts (highest priority). An exact
        // unchanged set is a no-op in a reusable engine.
        let font_context_changed = self
            .font_manager
            .load_embedded_fonts(&input.embedded_fonts)?
            | self.font_manager.load_additional_fonts(&input.fonts)
            | self
                .font_manager
                .set_caller_aliases(&self.caller_font_aliases);
        self.font_manager.begin_layout();
        let line_number_context = prepare_word_line_numbers(input, &mut self.font_manager);
        let has_wrapping_drawing = document_has_wrapping_drawing(input);

        if font_context_changed {
            self.paragraph_cache.clear();
            self.paragraph_cache_bytes = 0;
            self.table_cache.clear();
            self.table_cache_bytes = 0;
            self.header_footer_cache.clear();
            self.header_footer_cache_bytes = 0;
        }
        let base_context_matches = !font_context_changed
            && self
                .paragraph_cache_context
                .as_ref()
                .is_some_and(|context| {
                    context.matches_input_after_unchanged_fonts_ignoring_notes(
                        input,
                        &self.caller_font_aliases,
                        has_wrapping_drawing,
                    )
                });
        let notes_match = self
            .paragraph_cache_context
            .as_ref()
            .is_some_and(|context| context.notes_match(input));
        let context_matches = base_context_matches && notes_match;
        let page_note_restart = input_uses_page_note_restart(input);
        self.paragraph_cache_reads_enabled = base_context_matches && !page_note_restart;
        self.paragraph_cache_notes_match = notes_match;
        self.retained_context_matches_full = context_matches && !page_note_restart;
        self.header_footer_cache_reads_enabled = context_matches && !page_note_restart;
        self.pending_paragraph_cache = Some(VecDeque::new());
        self.pending_paragraph_cache_bytes = 0;
        self.pending_table_cache = Some(VecDeque::new());
        self.pending_table_cache_bytes = 0;
        self.pending_header_footer_cache = Some(VecDeque::new());
        self.pending_header_footer_cache_bytes = 0;

        let mut reference_seed = None;
        let mut result = match self.layout_transaction(
            input,
            sources,
            has_wrapping_drawing,
            None,
            None,
            &line_number_context,
        ) {
            Ok((first, references)) if needs_ref_projection => {
                self.navigation_by_source.clear();
                self.numbering_by_source.clear();
                self.bookmark_numbering.clear();
                self.bookmark_suppressed.clear();
                if let Some(pending) = &mut self.pending_paragraph_cache {
                    pending.clear();
                }
                if let Some(pending) = &mut self.pending_table_cache {
                    pending.clear();
                }
                if let Some(pending) = &mut self.pending_header_footer_cache {
                    pending.clear();
                }
                self.pending_paragraph_cache_bytes = 0;
                self.pending_table_cache_bytes = 0;
                self.pending_header_footer_cache_bytes = 0;
                drop(first);
                let projected = self
                    .layout_transaction(
                        input,
                        sources,
                        has_wrapping_drawing,
                        Some(&references),
                        None,
                        &line_number_context,
                    )
                    .map(|(result, _)| result);
                reference_seed = Some(references);
                projected
            }
            Ok((result, _)) => Ok(result),
            Err(error) => Err(error),
        };
        if page_note_restart {
            result = result.and_then(|mut current| {
                for _ in 0..4 {
                    let pages = note_reference_pages(&current);
                    self.navigation_by_source.clear();
                    self.numbering_by_source.clear();
                    self.bookmark_numbering.clear();
                    self.bookmark_suppressed.clear();
                    self.pending_paragraph_cache.as_mut().map(VecDeque::clear);
                    self.pending_table_cache.as_mut().map(VecDeque::clear);
                    self.pending_header_footer_cache
                        .as_mut()
                        .map(VecDeque::clear);
                    self.pending_paragraph_cache_bytes = 0;
                    self.pending_table_cache_bytes = 0;
                    self.pending_header_footer_cache_bytes = 0;
                    let (next, _) = self.layout_transaction(
                        input,
                        sources,
                        has_wrapping_drawing,
                        reference_seed.as_ref(),
                        Some(&pages),
                        &line_number_context,
                    )?;
                    let stable = note_reference_pages(&next) == pages;
                    current = next;
                    if stable {
                        return Ok(current);
                    }
                }
                Err(LayoutError::Layout(
                    "page-restarted note labels did not converge".to_owned(),
                ))
            });
        }
        let pending = self.pending_paragraph_cache.take().unwrap_or_default();
        let pending_tables = self.pending_table_cache.take().unwrap_or_default();
        let pending_header_footers = self.pending_header_footer_cache.take().unwrap_or_default();
        self.pending_paragraph_cache_bytes = 0;
        self.pending_table_cache_bytes = 0;
        self.pending_header_footer_cache_bytes = 0;
        self.paragraph_cache_reads_enabled = false;
        self.paragraph_cache_notes_match = false;
        self.retained_context_matches_full = false;
        self.header_footer_cache_reads_enabled = false;
        if result.is_ok() {
            if !base_context_matches {
                self.paragraph_cache.clear();
                self.paragraph_cache_bytes = 0;
            } else if !notes_match {
                self.paragraph_cache.retain(|entry| {
                    paragraph_note_references(&entry.key.paragraph, entry.key.revision_view)
                        .is_ok_and(|references| references.is_empty())
                });
                self.paragraph_cache_bytes =
                    self.paragraph_cache.iter().map(|entry| entry.bytes).sum();
            }
            if !context_matches {
                self.table_cache.clear();
                self.table_cache_bytes = 0;
                self.header_footer_cache.clear();
                self.header_footer_cache_bytes = 0;
                self.paragraph_cache_context = Some(ReusableEngineContext::for_input_with_wrap(
                    input,
                    &self.caller_font_aliases,
                    has_wrapping_drawing,
                ));
            }
            for entry in pending {
                self.publish_paragraph_cache_entry(entry);
            }
            for entry in pending_tables {
                self.publish_table_cache_entry(entry);
            }
            for entry in pending_header_footers {
                self.publish_header_footer_cache_entry(entry);
            }
        }
        let current_fonts = self
            .font_manager
            .current_layout_fonts()
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        self.paragraph_cache.retain(|entry| {
            entry.font_trace.iter().all(|event| {
                current_fonts.contains(match event {
                    FontTraceEvent::Resolution(id) => id,
                    FontTraceEvent::Contribution(contribution) => &contribution.font_id,
                })
            })
        });
        self.paragraph_cache_bytes = self.paragraph_cache.iter().map(|entry| entry.bytes).sum();
        self.table_cache.retain(|entry| {
            entry.font_trace.iter().all(|event| {
                current_fonts.contains(match event {
                    FontTraceEvent::Resolution(id) => id,
                    FontTraceEvent::Contribution(contribution) => &contribution.font_id,
                })
            })
        });
        self.table_cache_bytes = self.table_cache.iter().map(|entry| entry.bytes).sum();
        self.header_footer_cache.retain(|entry| {
            entry.font_trace.iter().all(|event| {
                current_fonts.contains(match event {
                    FontTraceEvent::Resolution(id) => id,
                    FontTraceEvent::Contribution(contribution) => &contribution.font_id,
                })
            })
        });
        self.header_footer_cache_bytes = self
            .header_footer_cache
            .iter()
            .map(|entry| entry.bytes)
            .sum();
        self.font_manager.retain_current_fonts();
        result
    }

    fn layout_transaction(
        &mut self,
        input: &LayoutInput,
        sources: Option<&SourceRegistry>,
        document_wraps: bool,
        reference_seed: Option<&NumberingState>,
        note_pages: Option<&HashMap<NoteRef, usize>>,
        line_number_context: &(Option<paginator::WordLineNumberStyle>, Vec<Diagnostic>),
    ) -> Result<(LayoutResult, NumberingState)> {
        let retained_context_matches = self.retained_context_matches_full;
        let styles = &input.styles;
        let mut num_state = reference_seed
            .map(NumberingState::references_only)
            .unwrap_or_default();
        num_state.set_note_labels(note_labels(input, note_pages)?);
        if let Some(sources) = sources {
            for (index, path) in sources.nodes.iter().enumerate() {
                if path.story == WordStory::Document {
                    let source = SourceNodeId::new(
                        u32::try_from(index + 1).expect("source index fits in u32"),
                    )
                    .expect("source ids are one based");
                    num_state.record_main_story_source(source);
                }
            }
        }
        let media = MediaRegistry::new(&input.images);
        let mut diagnostics = line_number_context.1.clone();
        if let Some(failure) = input
            .sequence_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.navigation_failure.as_deref())
        {
            diagnostics.push(Diagnostic {
                message: format!("numbering-aware navigation unavailable: {failure}"),
            });
        }

        // Re-breaking a paragraph around a floating drawing needs its line
        // breaking inputs kept alive past layout. Nearly no document has a
        // drawing that wraps, so the state is dropped again unless one does.
        // Get final section properties (body-level sectPr)
        let final_sect_pr = input
            .document
            .body
            .sect_pr
            .as_ref()
            .cloned()
            .unwrap_or_else(CT_SectPr::default_letter);

        // Build sections: each section has blocks + geometry + header/footer
        let mut sections: Vec<paginator::SharedSection> = Vec::new();
        let mut current_blocks: Vec<SharedLayoutBlock> = Vec::new();
        let mut body_heading_sources = Vec::new();
        let items = body_layout_items(&input.document.body);
        let item_sections = main_story_item_sections(&items, &final_sect_pr);

        let mut carried: Option<CT_P> = None;
        for (content, sect_pr_for_layout) in items.into_iter().zip(item_sections) {
            match content {
                // Accepting a deleted or moved-away paragraph mark joins the
                // paragraph to the next one, so one with no accepted content
                // leaves no block, spacing or list number behind.
                MainStoryLayoutItem::Paragraph(para, path)
                    if input.revision_view == RevisionView::Accepted
                        && path.len() == 1
                        && input.document.body.accepted_paragraph_joins_next(path[0])
                        && para.equations.is_empty()
                        && para.bookmark_markers.is_empty()
                        && {
                            let block_bookmarks = input.block_bookmarks();
                            let (before, after) = block_bookmarks.markers(para);
                            before.is_empty() && after.is_empty()
                        } =>
                {
                    if !accepted_paragraph_is_blank(para) {
                        carried = Some(match carried.take() {
                            Some(prefix) => join_accepted_layout_paragraphs(prefix, para)?,
                            None => para.accepted_view().into_owned(),
                        });
                    }
                }
                MainStoryLayoutItem::Paragraph(para, path) => {
                    let joined = carried
                        .take()
                        .map(|prefix| join_accepted_layout_paragraphs(prefix, para))
                        .transpose()?;
                    let para = joined.as_ref().unwrap_or(para);
                    // Check if this paragraph ends a section (has sect_pr)
                    let para_sect_pr = para.properties.as_ref().and_then(|p| p.sect_pr.as_ref());
                    let geometry = sect_pr_to_geometry(sect_pr_for_layout);

                    let source = sources.and_then(|sources| {
                        if path.len() == 1 {
                            sources.body_id(path[0])
                        } else {
                            sources.id(&WordStory::Document, &path)
                        }
                    });
                    let mut block = self.layout_body_paragraph(
                        para,
                        geometry.body_measure(),
                        styles,
                        input,
                        &media,
                        &mut num_state,
                        &mut diagnostics,
                        source,
                        sect_pr_for_layout.doc_grid.as_deref(),
                    )?;

                    if let (Some(registry), Some(source)) = (sources, source)
                        && let SharedLayoutBlock::Owned {
                            block,
                            semantics: Some(semantics),
                            ..
                        } = &mut block
                        && let LayoutBlock::Paragraph(paragraph) = block.as_mut()
                    {
                        registry.bind_text_boxes(
                            paragraph,
                            Arc::make_mut(&mut semantics.paint),
                            Some(source),
                        )?;
                    }

                    if sect_pr_for_layout.text_direction.as_deref() == Some("tbRl") {
                        let direct = para.properties.as_ref();
                        let mut effective = style_resolver::resolve_paragraph_properties(
                            direct.and_then(|p| p.style_id.as_deref()),
                            styles,
                        );
                        let num_id = direct.and_then(|p| p.num_id).or(effective.num_id);
                        let level = direct
                            .and_then(|p| p.num_ilvl)
                            .or(effective.num_ilvl)
                            .unwrap_or(0);
                        if let (Some(id), Some(numbering)) = (num_id, input.numbering.as_ref())
                            && let Some(properties) =
                                style_resolver::level_paragraph_properties(id, level, numbering)
                        {
                            merge_direct_ppr(&mut effective, properties);
                        }
                        if let Some(direct) = direct {
                            merge_direct_ppr(&mut effective, direct);
                        }
                        let policy = (effective.line_rule.as_deref() == Some("exact"))
                            .then_some(effective.line_spacing)
                            .flatten()
                            .filter(|height| height.0 > 0)
                            .and_then(|height| match effective.text_alignment.as_deref() {
                                None | Some("auto" | "center") => Some((height.to_pt(), true)),
                                Some("baseline") => Some((height.to_pt(), false)),
                                _ => None,
                            });
                        if let Some(policy) = policy {
                            block = match block {
                                SharedLayoutBlock::Paragraph {
                                    block,
                                    mut semantics,
                                    body_index,
                                } => {
                                    semantics.tb_rl_exact_baseline = Some(policy);
                                    SharedLayoutBlock::Paragraph {
                                        block,
                                        semantics,
                                        body_index,
                                    }
                                }
                                SharedLayoutBlock::Owned {
                                    block,
                                    border_group_before,
                                    reflow_direction,
                                    body_index,
                                    mut semantics,
                                } => {
                                    // Preserve list and anchor ownership. Owned paragraphs
                                    // already carry the same baseline policy into pagination.
                                    semantics
                                        .as_mut()
                                        .expect("owned Word paragraph retains its paint")
                                        .tb_rl_exact_baseline = Some(policy);
                                    SharedLayoutBlock::Owned {
                                        block,
                                        border_group_before,
                                        reflow_direction,
                                        body_index,
                                        semantics,
                                    }
                                }
                                SharedLayoutBlock::Table { .. } => unreachable!(),
                            };
                        }
                    }

                    if let Some(source) = source {
                        let paragraph = match &block {
                            SharedLayoutBlock::Owned { block, .. } => match block.as_ref() {
                                LayoutBlock::Paragraph(paragraph) => paragraph,
                                LayoutBlock::Table(_) => unreachable!(),
                            },
                            SharedLayoutBlock::Paragraph { block, .. } => block,
                            SharedLayoutBlock::Table { .. } => unreachable!(),
                        };
                        let mut facts = num_state.remove_navigation(source).unwrap_or_else(|| {
                            crate::ParagraphNavigation {
                                omitted_by_table: false,
                                level: None,
                                suppressed_numbering: false,
                                accepted_runs: 0,
                                text: String::new(),
                                style: None,
                            }
                        });
                        facts.level = paragraph.heading_level;
                        // Cache hits return before layout records this count.
                        facts.accepted_runs =
                            project_paragraph_runs(para, input.revision_view).len();
                        if !excluded_owner_for_source(input, source) {
                            facts.text = projected_paragraph_text(para, input.revision_view)?;
                            facts.style = para.properties.as_ref().and_then(|p| p.style_id.clone());
                        }
                        if facts.level.is_some() {
                            body_heading_sources.push(source);
                        }
                        num_state.record_navigation(source, facts);
                    }
                    if sources.is_some() {
                        block.set_body_index(path[0]);
                    }

                    current_blocks.push(block);

                    // If this paragraph has sect_pr, it ends a section
                    if let Some(sect_pr) = para_sect_pr {
                        let geometry = section_page_geometry(
                            sect_pr,
                            input,
                            line_number_context.0,
                            &mut diagnostics,
                        );
                        let header_footer = layout_header_footer(
                            self,
                            sect_pr,
                            input,
                            styles,
                            &media,
                            &mut diagnostics,
                            sources,
                            &mut num_state,
                        )?;
                        let title_pg = sect_pr.title_pg.unwrap_or(false);
                        let (header_footer, header_footer_semantics) = header_footer
                            .map_or((None, None), |(content, semantics)| {
                                (Some(content), Some(semantics))
                            });
                        block::collapse_shared_border_groups(&mut current_blocks);
                        sections.push(paginator::SharedSection {
                            line_number_style: line_number_context.0,
                            written_tb_rl_band: sect_pr.text_direction.as_deref() == Some("tbRl"),
                            continuous: sect_pr.section_type
                                == Some(oxoffice_docx_oxml::shared::ST_SectionType::Continuous),
                            parity: match sect_pr.section_type {
                                Some(oxoffice_docx_oxml::shared::ST_SectionType::OddPage) => {
                                    Some(paginator::SectionParity::Odd)
                                }
                                Some(oxoffice_docx_oxml::shared::ST_SectionType::EvenPage) => {
                                    Some(paginator::SectionParity::Even)
                                }
                                _ => None,
                            },
                            blocks: std::mem::take(&mut current_blocks),
                            modern_footnote_layout: input.modern_footnote_layout,
                            footnote_layout_like_word8: input.footnote_layout_like_word8,
                            geometry,
                            header_footer,
                            header_footer_semantics,
                            title_pg,
                            page_number_start: section_page_number_start(sect_pr),
                            endnotes_at_section_end: sect_pr
                                .endnote_pr
                                .as_deref()
                                .and_then(|policy| policy.pos.as_deref())
                                .or_else(|| {
                                    input.note_defaults[1]
                                        .as_ref()
                                        .and_then(|policy| policy.pos.as_deref())
                                })
                                == Some("sectEnd"),
                            footnotes_beneath_text: sect_pr
                                .footnote_pr
                                .as_deref()
                                .and_then(|policy| policy.pos.as_deref())
                                .or_else(|| {
                                    input.note_defaults[0]
                                        .as_ref()
                                        .and_then(|policy| policy.pos.as_deref())
                                })
                                == Some("beneathText"),
                        });
                        if let Some(numbering) = input.numbering.as_ref() {
                            num_state.restart_after_section_break(numbering);
                        }
                    }
                }
                // Accepting every tracked change removes a table whose rows
                // are all deleted, so it leaves no block or spacing behind.
                MainStoryLayoutItem::Table(tbl, path)
                    if input.revision_view == RevisionView::Accepted
                        && tbl.accepted_view_removes() =>
                {
                    if let Some(registry) = sources.as_ref() {
                        registry.record_table_omission(&WordStory::Document, &path, &mut num_state);
                    }
                }
                MainStoryLayoutItem::Table(tbl, path) => {
                    let geometry = sect_pr_to_geometry(sect_pr_for_layout);

                    let mut table_block = self.layout_body_table(
                        tbl,
                        geometry.body_measure(),
                        styles,
                        input,
                        &media,
                        &mut num_state,
                        &mut diagnostics,
                        sources,
                        &WordStory::Document,
                        &path,
                        sect_pr_for_layout.doc_grid.as_deref(),
                    )?;
                    if let (
                        Some(registry),
                        SharedLayoutBlock::Table {
                            block, semantics, ..
                        },
                    ) = (sources, &table_block)
                    {
                        record_table_navigation(block, semantics, registry, &mut num_state);
                    }
                    if sources.is_some() {
                        table_block.set_body_index(path[0]);
                    }
                    current_blocks.push(table_block);
                }
            }
        }

        // Remaining blocks belong to the final section
        let final_geometry = section_page_geometry(
            &final_sect_pr,
            input,
            line_number_context.0,
            &mut diagnostics,
        );
        let final_hf = layout_header_footer(
            self,
            &final_sect_pr,
            input,
            styles,
            &media,
            &mut diagnostics,
            sources,
            &mut num_state,
        )?;
        let final_title_pg = final_sect_pr.title_pg.unwrap_or(false);
        let (final_hf, final_hf_semantics) = final_hf
            .map_or((None, None), |(content, semantics)| {
                (Some(content), Some(semantics))
            });
        block::collapse_shared_border_groups(&mut current_blocks);
        sections.push(paginator::SharedSection {
            line_number_style: line_number_context.0,
            written_tb_rl_band: final_sect_pr.text_direction.as_deref() == Some("tbRl"),
            continuous: final_sect_pr.section_type
                == Some(oxoffice_docx_oxml::shared::ST_SectionType::Continuous),
            parity: match final_sect_pr.section_type {
                Some(oxoffice_docx_oxml::shared::ST_SectionType::OddPage) => {
                    Some(paginator::SectionParity::Odd)
                }
                Some(oxoffice_docx_oxml::shared::ST_SectionType::EvenPage) => {
                    Some(paginator::SectionParity::Even)
                }
                _ => None,
            },
            blocks: current_blocks,
            modern_footnote_layout: input.modern_footnote_layout,
            footnote_layout_like_word8: input.footnote_layout_like_word8,
            geometry: final_geometry.clone(),
            header_footer: final_hf,
            header_footer_semantics: final_hf_semantics,
            title_pg: final_title_pg,
            page_number_start: section_page_number_start(&final_sect_pr),
            endnotes_at_section_end: final_sect_pr
                .endnote_pr
                .as_deref()
                .and_then(|policy| policy.pos.as_deref())
                .or_else(|| {
                    input.note_defaults[1]
                        .as_ref()
                        .and_then(|policy| policy.pos.as_deref())
                })
                == Some("sectEnd"),
            footnotes_beneath_text: final_sect_pr
                .footnote_pr
                .as_deref()
                .and_then(|policy| policy.pos.as_deref())
                .or_else(|| {
                    input.note_defaults[0]
                        .as_ref()
                        .and_then(|policy| policy.pos.as_deref())
                })
                == Some("beneathText"),
        });
        let structure = assign_shared_document_structure(&mut sections);

        // Lay the notes out once per width, before pagination, so the paginator
        // can reserve exactly the height it will later draw. A note is broken to
        // the measure of the section carrying its reference, so every section's
        // width is registered. The endnote pages that follow the last body page
        // are drawn against `final_geometry`, which belongs to the section
        // pushed just above and is therefore already in this list.
        let content_widths: Vec<f64> = sections
            .iter()
            .map(|section| section.geometry.content_width())
            .collect();
        let notes = NoteRegistry::build(
            input,
            styles,
            &media,
            &mut self.font_manager,
            &mut num_state,
            &content_widths,
            &mut diagnostics,
            sources,
        )?;

        let mut font_trace = self.font_manager.current_layout_fonts().to_vec();
        let producer_paints = if sections.len() == 1 {
            sections[0]
                .blocks
                .iter()
                .map(|block| match block {
                    SharedLayoutBlock::Paragraph { semantics, .. } => {
                        Some(RestartPaint::Paragraph(Arc::clone(&semantics.paint)))
                    }
                    SharedLayoutBlock::Owned {
                        block,
                        semantics: Some(semantics),
                        ..
                    } if matches!(block.as_ref(), LayoutBlock::Paragraph(_)) => {
                        Some(RestartPaint::Paragraph(Arc::clone(&semantics.paint)))
                    }
                    SharedLayoutBlock::Table { semantics, .. } => {
                        Some(RestartPaint::Table(semantics.clone()))
                    }
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
        } else {
            None
        };
        let restart_record_eligible = sections.len() == 1
            && producer_paints.is_some()
            && sections[0].blocks.len() == input.document.body.content.len()
            && input.document.background_xml.is_none()
            && !document_wraps
            && input
                .document
                .body
                .content
                .iter()
                .zip(&sections[0].blocks)
                .all(|(content, block)| match content {
                    BodyContent::Paragraph(paragraph) if block.paragraph().is_some() => {
                        paragraph_is_restart_record_source_safe(paragraph, styles)
                            && restart_record_block_is_safe(block)
                    }
                    BodyContent::Table(table) if block.table().is_some() => {
                        table_is_cache_safe(table, styles)
                    }
                    _ => false,
                });
        let restart_eligible = restart_record_eligible
            && input
                .document
                .body
                .content
                .iter()
                .zip(&sections[0].blocks)
                .all(|(content, block)| {
                    !matches!(content, BodyContent::Paragraph(paragraph) if paragraph_has_field(paragraph))
                        && restart_block_is_safe(block)
                });
        let current_body_notes = body_note_references(input)?;
        let reusable_restart_record = restart_record_eligible
            && retained_context_matches
            && self.restart_cache.as_ref().is_some_and(|cache| {
                cache.paint.len() == cache.body.len()
                    && cache.font_trace == font_trace
                    && cache.with_provenance == sources.is_some()
                    && cache
                        .body
                        .iter()
                        .flat_map(|entry| entry.note_references().iter().copied())
                        .eq(current_body_notes.iter().copied())
            });
        let reusable_restart = restart_eligible && reusable_restart_record;
        let mut restart_identity_memo =
            RestartBodyIdentityMemo::new(input.document.body.content.len());
        let body_unchanged = reusable_restart_record
            && self.restart_cache.as_ref().is_some_and(|cache| {
                cache.paint.as_slice() == producer_paints.as_deref().unwrap_or(&[])
                    && cache.body.len() == input.document.body.content.len()
                    && input
                        .document
                        .body
                        .content
                        .iter()
                        .zip(&cache.body)
                        .enumerate()
                        .all(|(index, (content, retained))| {
                            retained.matches_memoized(content, index, &mut restart_identity_memo)
                        })
            });
        let first_changed = reusable_restart.then(|| {
            let cache = self.restart_cache.as_ref().expect("restart cache exists");
            input
                .document
                .body
                .content
                .iter()
                .zip(&cache.body)
                .enumerate()
                .position(|(index, (current, previous))| {
                    cache.paint.get(index)
                        != producer_paints.as_ref().and_then(|paint| paint.get(index))
                        || !previous.matches_memoized(current, index, &mut restart_identity_memo)
                })
                .unwrap_or_else(|| input.document.body.content.len().min(cache.body.len()))
        });
        let common_suffix = reusable_restart.then(|| {
            let cache = self.restart_cache.as_ref().expect("restart cache exists");
            input
                .document
                .body
                .content
                .iter()
                .enumerate()
                .rev()
                .zip(cache.body.iter().enumerate().rev())
                .take_while(|((index, current), (old_index, previous))| {
                    cache.paint.get(*old_index)
                        == producer_paints.as_ref().and_then(|paint| paint.get(*index))
                        && previous.matches_memoized(current, *index, &mut restart_identity_memo)
                })
                .count()
        });
        // The restart begins strictly before the first changed block. A page
        // boundary right before it may have been chosen by that block, which
        // moved there whole or broke the page before itself, and an edit can
        // undo that choice. The document start is always a safe restart.
        let restart_checkpoint = first_changed.and_then(|first_changed| {
            self.restart_cache
                .as_ref()
                .expect("restart cache exists")
                .checkpoints
                .iter()
                .rev()
                .find(|checkpoint| {
                    checkpoint.next_block_index < first_changed || checkpoint.page_count == 0
                })
                .copied()
        });
        let tail_reusable = sources.is_none()
            || self
                .restart_cache
                .as_ref()
                .is_some_and(|cache| cache.body.len() == input.document.body.content.len());
        let tail_source = restart_checkpoint
            .filter(|_| tail_reusable)
            .and_then(|restart| {
                let cache = self.restart_cache.as_ref().expect("restart cache exists");
                let common_suffix = common_suffix.expect("reusable restart has an exact suffix");
                let new_tail = input.document.body.content.len() - common_suffix;
                let old_tail = cache.body.len() - common_suffix;
                let block_delta = new_tail as isize - old_tail as isize;
                cache
                    .checkpoints
                    .iter()
                    .find(|checkpoint| {
                        checkpoint.next_block_index >= old_tail
                            && checkpoint
                                .next_block_index
                                .checked_add_signed(block_delta)
                                .is_some_and(|next| next > restart.next_block_index)
                    })
                    .copied()
                    .map(|old| {
                        (
                            paginator::PaginationCheckpoint {
                                next_block_index: old
                                    .next_block_index
                                    .checked_add_signed(block_delta)
                                    .expect("common suffix block index remains in range"),
                                page_count: old.page_count,
                                next_header_page_number: old.next_header_page_number,
                            },
                            old,
                        )
                    })
            });

        let mut page_sections = Vec::new();
        let (mut pages, mut outlines, mut checkpoints, mut body_fragments, mut page_policies) =
            if restart_eligible {
                let mut recorded = paginator::paginate_shared_single_section_recorded(
                    &sections[0],
                    &self.font_manager,
                    &media,
                    &notes,
                    restart_checkpoint,
                    tail_source.map(|(stop, _)| stop),
                );
                if let Some(failure) = recorded.furniture_failure.take() {
                    return Err(LayoutError::Layout(format!(
                        "section {} page {}: {}",
                        failure.section + 1,
                        failure.page,
                        failure.reason
                    )));
                }
                if recorded.stopped_at.is_none() {
                    // Completing a changed region must include references retained
                    // in the prefix, while continuing in the actual final flow band.
                    let references = body_note_references(input)?;
                    paginator::append_endnote_pages_for_references(
                        &mut recorded.pages,
                        &mut recorded.paint,
                        &references,
                        &notes,
                        final_geometry.clone(),
                        restart_checkpoint.map_or(0, |checkpoint| checkpoint.page_count),
                        restart_checkpoint
                            .map_or(1, |checkpoint| checkpoint.next_header_page_number),
                        recorded.last_flow_space,
                        Some(&paginator::EndnoteFurniture {
                            section: &sections[0],
                            section_index: 0,
                            fm: &self.font_manager,
                            media: media.media(),
                        }),
                    )
                    .map_err(|failure| {
                        LayoutError::Layout(format!(
                            "section {} page {}: {}",
                            failure.section + 1,
                            failure.page,
                            failure.reason
                        ))
                    })?;
                }
                recorded.paint.validate(&recorded.pages)?;
                recorded.paint.boundaries.clear();
                for (page, policies) in recorded.pages.iter_mut().zip(&mut recorded.paint.policies)
                {
                    mark_remaining_artifacts(&mut page.elements, policies)?;
                }
                let mut body_fragments = vec![Vec::new(); input.document.body.content.len()];
                if let Some(checkpoint) = restart_checkpoint
                    && let Some(cache) = self.restart_cache.as_ref()
                {
                    for (body_index, fragments) in cache
                        .body_fragments
                        .iter()
                        .enumerate()
                        .take(checkpoint.next_block_index)
                    {
                        body_fragments[body_index].clone_from(fragments);
                    }
                }
                for (body_index, fragment) in recorded.body_fragments.drain(..) {
                    if let Some(fragments) = body_fragments.get_mut(body_index) {
                        fragments.push(fragment);
                    }
                }
                let mut pages = restart_checkpoint.map_or_else(Vec::new, |checkpoint| {
                    self.restart_cache
                        .as_ref()
                        .expect("a restart checkpoint belongs to retained pages")
                        .raw_pages[..checkpoint.page_count]
                        .iter()
                        .map(Arc::clone)
                        .collect()
                });
                let mut policies = restart_checkpoint.map_or_else(Vec::new, |checkpoint| {
                    self.restart_cache
                        .as_ref()
                        .expect("retained prefix owns policies")
                        .raw_policies[..checkpoint.page_count]
                        .to_vec()
                });
                policies.append(&mut recorded.paint.policies);
                pages.extend(recorded.pages.into_iter().map(Arc::new));
                let mut outlines = restart_checkpoint.map_or_else(Vec::new, |checkpoint| {
                    self.restart_cache
                        .as_ref()
                        .expect("a restart checkpoint belongs to retained outlines")
                        .outlines
                        .iter()
                        .filter(|outline| outline.page_index < checkpoint.page_count)
                        .cloned()
                        .collect()
                });
                outlines.extend(recorded.outlines);
                let mut checkpoints = restart_checkpoint.map_or_else(Vec::new, |checkpoint| {
                    self.restart_cache
                        .as_ref()
                        .expect("a restart checkpoint belongs to retained state")
                        .checkpoints
                        .iter()
                        .copied()
                        .filter(|candidate| candidate.page_count <= checkpoint.page_count)
                        .collect()
                });
                checkpoints.extend(recorded.checkpoints);
                if let (Some(stopped), Some((_, old_tail))) = (recorded.stopped_at, tail_source) {
                    debug_assert_eq!(stopped.page_count, old_tail.page_count);
                    let cache = self.restart_cache.as_ref().expect("restart cache exists");
                    policies.extend_from_slice(&cache.raw_policies[old_tail.page_count..]);
                    pages.extend(
                        cache.raw_pages[old_tail.page_count..]
                            .iter()
                            .map(Arc::clone),
                    );
                    outlines.extend(
                        cache
                            .outlines
                            .iter()
                            .filter(|outline| outline.page_index >= old_tail.page_count)
                            .cloned(),
                    );
                    let block_delta =
                        stopped.next_block_index as isize - old_tail.next_block_index as isize;
                    checkpoints.extend(
                        cache
                            .checkpoints
                            .iter()
                            .filter(|candidate| candidate.page_count > old_tail.page_count)
                            .map(|candidate| paginator::PaginationCheckpoint {
                                next_block_index: candidate
                                    .next_block_index
                                    .checked_add_signed(block_delta)
                                    .expect("common suffix block index remains in range"),
                                page_count: candidate.page_count,
                                next_header_page_number: candidate.next_header_page_number,
                            }),
                    );
                    for (body_index, fragments) in cache
                        .body_fragments
                        .iter()
                        .enumerate()
                        .skip(old_tail.next_block_index)
                    {
                        if let Some(destination) = body_fragments.get_mut(body_index) {
                            destination.clone_from(fragments);
                        }
                    }
                }
                checkpoints.sort_unstable_by_key(|checkpoint| checkpoint.next_block_index);
                checkpoints.dedup();
                (pages, outlines, checkpoints, body_fragments, policies)
            } else {
                let mut pagination = paginator::paginate_shared_sections(
                    &sections,
                    &self.font_manager,
                    &media,
                    &notes,
                );
                if let Some(failure) = pagination.furniture_failure.take() {
                    return Err(LayoutError::Layout(format!(
                        "section {} page {}: {}",
                        failure.section + 1,
                        failure.page,
                        failure.reason
                    )));
                }
                // Endnotes read at the end of the document, so they follow the last
                // body page rather than sitting at the foot of their reference's page.
                if input.note_defaults[1]
                    .as_ref()
                    .and_then(|policy| policy.pos.as_deref())
                    .unwrap_or("docEnd")
                    == "docEnd"
                {
                    paginator::append_endnote_pages_at_document_end(
                        &mut pagination.pages,
                        &mut pagination.paint,
                        &notes,
                        final_geometry.clone(),
                        pagination.last_flow_space,
                        &pagination.endnotes_placed,
                        sections
                            .last()
                            .map(|section| paginator::EndnoteFurniture {
                                section,
                                section_index: sections.len() - 1,
                                fm: &self.font_manager,
                                media: media.media(),
                            })
                            .as_ref(),
                    )
                    .map_err(|failure| {
                        LayoutError::Layout(format!(
                            "section {} page {}: {}",
                            failure.section + 1,
                            failure.page,
                            failure.reason
                        ))
                    })?;
                } else if !pagination.endnotes_placed.is_empty() {
                    paginator::append_endnote_pages_at_document_end(
                        &mut pagination.pages,
                        &mut pagination.paint,
                        &notes,
                        final_geometry.clone(),
                        None,
                        &pagination.endnotes_placed,
                        sections
                            .last()
                            .map(|section| paginator::EndnoteFurniture {
                                section,
                                section_index: sections.len() - 1,
                                fm: &self.font_manager,
                                media: media.media(),
                            })
                            .as_ref(),
                    )
                    .map_err(|failure| {
                        LayoutError::Layout(format!(
                            "section {} page {}: {}",
                            failure.section + 1,
                            failure.page,
                            failure.reason
                        ))
                    })?;
                } else {
                    paginator::append_endnote_pages_at_document_end(
                        &mut pagination.pages,
                        &mut pagination.paint,
                        &notes,
                        final_geometry.clone(),
                        None,
                        &[],
                        sections
                            .last()
                            .map(|section| paginator::EndnoteFurniture {
                                section,
                                section_index: sections.len() - 1,
                                fm: &self.font_manager,
                                media: media.media(),
                            })
                            .as_ref(),
                    )
                    .map_err(|failure| {
                        LayoutError::Layout(format!(
                            "section {} page {}: {}",
                            failure.section + 1,
                            failure.page,
                            failure.reason
                        ))
                    })?;
                }
                apply_page_background(
                    &mut pagination.pages,
                    &mut pagination.paint,
                    input,
                    &pagination.blank_pages,
                )?;
                pagination.paint.validate(&pagination.pages)?;
                pagination.paint.boundaries.clear();
                for (page, policies) in pagination
                    .pages
                    .iter_mut()
                    .zip(&mut pagination.paint.policies)
                {
                    mark_remaining_artifacts(&mut page.elements, policies)?;
                }
                page_sections = pagination.page_sections;
                let mut body_fragments = vec![Vec::new(); input.document.body.content.len()];
                for (body_index, fragment) in pagination.body_fragments {
                    if let Some(fragments) = body_fragments.get_mut(body_index) {
                        fragments.push(fragment);
                    }
                }
                (
                    pagination.pages.into_iter().map(Arc::new).collect(),
                    pagination.outlines,
                    Vec::new(),
                    body_fragments,
                    pagination.paint.policies,
                )
            };
        if body_unchanged
            && let Some(cache) = self.restart_cache.as_ref()
            && cache.raw_pages.len() == pages.len()
            && cache.raw_policies == page_policies
        {
            for (page, retained) in pages.iter_mut().zip(&cache.raw_pages) {
                *page = Arc::clone(retained);
            }
            if cache.body_fragments.len() == body_fragments.len() {
                body_fragments.clone_from(&cache.body_fragments);
            }
        }
        if sources.is_none() {
            body_fragments.clear();
        }
        if page_policies.len() != pages.len() {
            return Err(LayoutError::Layout(
                "page paint companion count mismatch".into(),
            ));
        }
        for (page, policies) in pages.iter().zip(&page_policies) {
            policies.validate(&page.elements)?;
        }
        let mut raw_policies = restart_record_eligible.then(|| page_policies.clone());
        let mut raw_pages = restart_record_eligible.then(|| pages.clone());

        if page_sections.is_empty() {
            page_sections.extend(pages.iter().map(|page| crate::WordPageSection {
                physical_page: page.page_number,
                displayed_page: page.displayed_page_number,
                section_index: 0,
            }));
        }
        // Document-end note overflow pages belong to the final section.
        for page in &pages {
            if !page_sections
                .iter()
                .any(|record| record.physical_page == page.page_number)
            {
                page_sections.push(crate::WordPageSection {
                    physical_page: page.page_number,
                    displayed_page: page.displayed_page_number,
                    section_index: sections.len().saturating_sub(1),
                });
            }
        }
        let mut section_paths = Vec::new();
        let mut section_index = 0;
        for item in body_layout_items(&input.document.body) {
            match item {
                MainStoryLayoutItem::Paragraph(paragraph, path) => {
                    section_paths.push((path, section_index));
                    if paragraph
                        .properties
                        .as_ref()
                        .is_some_and(|properties| properties.sect_pr.is_some())
                    {
                        section_index += 1;
                    }
                }
                MainStoryLayoutItem::Table(_, path) => section_paths.push((path, section_index)),
            }
        }
        let owner_section = |node: SourceNodeId, fallback: usize| {
            sources
                .and_then(|registry| registry.nodes.get(node.get() as usize - 1))
                .and_then(|path| {
                    let children = match &path.story {
                        WordStory::Document => &path.children,
                        WordStory::TextBox {
                            part_name,
                            owner_children,
                        } if input.story_part_names.get(&WordStory::Document)
                            == Some(part_name) =>
                        {
                            owner_children
                        }
                        _ => return None,
                    };
                    section_paths
                        .iter()
                        .find(|(prefix, _)| children.starts_with(prefix))
                        .map(|(_, index)| *index)
                })
                .unwrap_or(fallback)
        };
        let mut note_owners = HashMap::<NoteRef, Option<crate::WordPageSection>>::new();
        for page in &pages {
            let fallback = page_sections
                .iter()
                .find(|record| record.physical_page == page.page_number)
                .map_or(0, |record| record.section_index);
            oxoffice_layout::walk(&page.elements, &mut |element, _| {
                let (reference, span) = match element {
                    PositionedElement::Text(run) => (run.note, run.note_reference_source),
                    PositionedElement::MultilingualText(run) => {
                        (run.note, run.note_reference_source)
                    }
                    _ => return,
                };
                let Some(reference) = reference else {
                    return;
                };
                let Some(span) = span else {
                    return;
                };
                let Some(path) =
                    sources.and_then(|sources| sources.nodes.get(span.node.get() as usize - 1))
                else {
                    return;
                };
                if path.story != WordStory::Document {
                    return;
                }
                let record = crate::WordPageSection {
                    physical_page: page.page_number,
                    displayed_page: page.displayed_page_number,
                    section_index: owner_section(span.node, fallback),
                };
                note_owners
                    .entry(reference)
                    .and_modify(|owner| {
                        if *owner != Some(record) {
                            *owner = None;
                        }
                    })
                    .or_insert(Some(record));
            });
        }
        let note_for_node = |node: SourceNodeId| {
            let sources = sources?;
            let path = sources.nodes.get(node.get() as usize - 1)?;
            let story = match &path.story {
                WordStory::TextBox {
                    part_name,
                    owner_children,
                } => sources
                    .story_owner_children
                    .iter()
                    .filter(|(story, prefix)| {
                        sources.story_part_names.get(*story) == Some(part_name)
                            && owner_children.starts_with(prefix)
                    })
                    .max_by_key(|(_, prefix)| prefix.len())
                    .map(|(story, _)| story)?,
                story => story,
            };
            match story {
                WordStory::Footnote { id } => Some(NoteRef {
                    stream: NoteStream::Footnote,
                    id: *id,
                }),
                WordStory::Endnote { id } => Some(NoteRef {
                    stream: NoteStream::Endnote,
                    id: *id,
                }),
                _ => None,
            }
        };
        let mut field_placements = Vec::new();
        for page in &pages {
            let section_index = page_sections
                .iter()
                .find(|record| record.physical_page == page.page_number)
                .map_or(0, |record| record.section_index);
            oxoffice_layout::walk(&page.elements, &mut |element, _| {
                let source = match element {
                    PositionedElement::Text(run) => run.field_source,
                    PositionedElement::MultilingualText(run) => run.field_source,
                    _ => None,
                };
                if let Some(source) = source {
                    let reference_owner = note_for_node(source.node)
                        .map(|note| note_owners.get(&note).copied().flatten());
                    if reference_owner == Some(None) {
                        return;
                    }
                    let reference_owner = reference_owner.flatten();
                    let placement = crate::WordFieldPlacement {
                        source,
                        physical_page: page.page_number,
                        displayed_page: reference_owner
                            .map_or(page.displayed_page_number, |owner| owner.displayed_page),
                        section_index: reference_owner.map_or_else(
                            || owner_section(source.node, section_index),
                            |owner| owner.section_index,
                        ),
                    };
                    if !field_placements.contains(&placement) {
                        field_placements.push(placement);
                    }
                    if let Some(descendants) =
                        sources.and_then(|registry| registry.field_descendants.get(&source))
                    {
                        for &index in descendants {
                            let child = crate::WordFieldPlacement {
                                source: FieldSource {
                                    node: source.node,
                                    index,
                                },
                                ..placement
                            };
                            if !field_placements.contains(&child) {
                                field_placements.push(child);
                            }
                        }
                    }
                }
            });
        }
        self.last_page_sections = page_sections;
        self.last_field_placements = field_placements;
        let styleref_values = sources.map_or_else(HashMap::new, |registry| {
            placed_styleref_values(
                input,
                registry,
                &num_state,
                &pages,
                &self.last_field_placements,
            )
        });
        // Post-pagination pass: record bookmark targets and substitute fields.
        let total_pages = pages.len();
        let bookmark_pages = pages
            .iter()
            .flat_map(|page| {
                let mut targets = Vec::new();
                oxoffice_layout::walk(&page.elements, &mut |element, _| {
                    if let PositionedElement::Text(run) = element
                        && let Some(FieldKind::Target(target)) = run.field_kind
                    {
                        targets.push((target, page.displayed_page_number));
                    }
                });
                targets
            })
            .collect::<HashMap<_, _>>();
        let mut bookmark_owners = HashMap::new();
        for page in &pages {
            let fallback = self
                .last_page_sections
                .iter()
                .find(|record| record.physical_page == page.page_number)
                .map_or(0, |record| record.section_index);
            oxoffice_layout::walk(&page.elements, &mut |element, _| {
                if let PositionedElement::Text(run) = element
                    && let Some(FieldKind::Target(target)) = run.field_kind
                {
                    bookmark_owners.insert(
                        target,
                        crate::WordPageSection {
                            physical_page: page.page_number,
                            displayed_page: page.displayed_page_number,
                            section_index: run
                                .source
                                .map_or(fallback, |span| owner_section(span.node, fallback)),
                        },
                    );
                }
            });
        }
        self.last_bookmark_pages = bookmark_owners;
        let page_reference_names = page_reference_names(input);
        let mut unresolved_targets = Vec::new();
        for page in &pages {
            oxoffice_layout::walk(&page.elements, &mut |element, _| {
                if let PositionedElement::Text(run) = element
                    && let Some(FieldKind::TargetPage(target)) = run.field_kind
                    && !bookmark_pages.contains_key(&target)
                {
                    unresolved_targets.push(target);
                }
            });
        }
        unresolved_targets.sort_unstable();
        unresolved_targets.dedup();
        for target in unresolved_targets {
            let name = page_reference_names
                .get(target)
                .cloned()
                .unwrap_or_else(|| format!("#{target}"));
            diagnostics.push(Diagnostic {
                message: format!(
                    "PAGEREF target {name} did not reach pagination, unresolved placeholder retained"
                ),
            });
        }
        let mut bookmark_identity = bookmark_pages
            .iter()
            .map(|(&target, &page_number)| (target, page_number))
            .collect::<Vec<_>>();
        bookmark_identity.sort_unstable();
        let mut sequence_boundary = 0;
        let sequence_boundaries = pages
            .iter()
            .map(|page| {
                sequence_boundary = sequence_event_boundary(&page.elements, sequence_boundary);
                sequence_boundary
            })
            .collect::<Vec<_>>();
        let mut section_formats = document_sections(&input.document)
            .into_iter()
            .map(section_page_number_format)
            .collect::<Vec<_>>();
        // Pagination supplies this same implicit final section when no body
        // sectPr exists. It is qualified absence, not an unknown owner index.
        if input.document.body.sect_pr.is_none() {
            section_formats.push(Some("decimal".into()));
        }
        let mut section_pages = HashMap::<usize, std::collections::HashSet<usize>>::new();
        for record in &self.last_page_sections {
            section_pages
                .entry(record.section_index)
                .or_default()
                .insert(record.physical_page);
        }
        let section_page_counts = section_pages
            .into_iter()
            .map(|(section, pages)| (section, pages.len()))
            .collect::<HashMap<_, _>>();
        let mut placements_by_page = HashMap::<usize, Vec<&crate::WordFieldPlacement>>::new();
        for placement in &self.last_field_placements {
            placements_by_page
                .entry(placement.physical_page)
                .or_default()
                .push(placement);
        }
        let mut substitution_inputs = Vec::with_capacity(pages.len());
        let mut reuse_result_pages = vec![false; pages.len()];
        for (page_index, page) in pages.iter_mut().enumerate() {
            if !page_has_substitution_state(page)
                && !styleref_values.contains_key(&page.page_number)
            {
                reuse_result_pages[page_index] = self.restart_cache.as_ref().is_some_and(|cache| {
                    cache.substitution_inputs.get(page_index) == Some(&None)
                        && cache.raw_policies.get(page_index) == page_policies.get(page_index)
                        && cache.policies.len() == cache.pages.len()
                        && cache
                            .raw_pages
                            .get(page_index)
                            .is_some_and(|retained| Arc::ptr_eq(page, retained))
                });
                substitution_inputs.push(None);
                continue;
            }
            let placements = placements_by_page
                .get(&page.page_number)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let mut page_field_values = HashMap::new();
            let mut format_context = Vec::new();
            if let Some(sources) = sources {
                for placement in placements {
                    let Some((instruction, stored)) = sources.page_fields.get(&placement.source)
                    else {
                        continue;
                    };
                    let locked = sources
                        .field_source_contexts
                        .get(&placement.source)
                        .is_some_and(|(_, locked, _)| *locked);
                    let target = (instruction.name == "PAGEREF")
                        .then(|| {
                            field_text_argument(instruction, 0)
                                .and_then(|name| page_ref_id(input, name))
                                .and_then(|target| self.last_bookmark_pages.get(&target))
                        })
                        .flatten();
                    let owner =
                        target.map_or(placement.section_index, |target| target.section_index);
                    let section_format = if matches!(instruction.name.as_str(), "PAGE" | "PAGEREF")
                    {
                        section_formats.get(owner).and_then(Option::as_deref)
                    } else {
                        Some("decimal")
                    };
                    let value = match instruction.name.as_str() {
                        "PAGE" => Some(placement.displayed_page),
                        "NUMPAGES" => Some(total_pages),
                        "SECTION" => Some(placement.section_index + 1),
                        "SECTIONPAGES" => Some(
                            *section_page_counts
                                .get(&placement.section_index)
                                .unwrap_or(&0),
                        ),
                        "PAGEREF" => target.map(|target| target.displayed_page),
                        _ => None,
                    };
                    format_context.push((placement.source.node.get(), placement.source.index, format!("{instruction:?}{owner:?}{section_format:?}{target:?}{locked:?}{value:?}{stored:?}")));
                    let result = if locked {
                        Err("locked owner".into())
                    } else {
                        value
                            .ok_or_else(|| {
                                "bookmark target is missing, ambiguous or unplaced".to_owned()
                            })
                            .and_then(|value| {
                                format_page_field_result(instruction, value, section_format)
                            })
                    };
                    let value = result.unwrap_or_else(|reason| {
                        diagnostics.push(Diagnostic {
                            message: format!(
                                "{} field {:?} retained stored display: {reason}",
                                instruction.name, placement.source
                            ),
                        });
                        stored.clone()
                    });
                    page_field_values.insert(
                        placement.source,
                        PageFieldValue {
                            display: value,
                            target_unplaced: instruction.name == "PAGEREF" && target.is_none(),
                        },
                    );
                }
            }
            format_context.sort_unstable();
            let page_format_context = format!("{format_context:?}");
            let inputs = FieldSubstitutionInputs {
                page_index,
                page_number: page.displayed_page_number,
                total_pages,
                bookmark_pages: bookmark_identity.clone(),
                page_format_context,
                font_identity: font_trace.clone(),
                revision_view: input.revision_view,
                revision_style: input.revision_style,
            };
            let reusable = input.sequence_snapshot.is_none()
                && self.restart_cache.as_ref().is_some_and(|cache| {
                    cache
                        .substitution_inputs
                        .get(page_index)
                        .and_then(Option::as_ref)
                        == Some(&inputs)
                        && cache.raw_policies.get(page_index) == page_policies.get(page_index)
                        && cache.policies.len() == cache.pages.len()
                        && cache
                            .raw_pages
                            .get(page_index)
                            .is_some_and(|retained| Arc::ptr_eq(page, retained))
                });
            if reusable {
                page_policies[page_index] = self
                    .restart_cache
                    .as_ref()
                    .expect("reusable page owns final policy")
                    .policies[page_index]
                    .clone();
                reuse_result_pages[page_index] = true;
                substitution_inputs.push(Some(inputs));
                continue;
            }
            let page = Arc::make_mut(page);
            let page_num = page.displayed_page_number;
            let section_index = self
                .last_page_sections
                .iter()
                .find(|record| record.physical_page == page.page_number)
                .map_or(0, |record| record.section_index);
            let section_pages = *section_page_counts.get(&section_index).unwrap_or(&0);
            let field_sections = placements
                .iter()
                .map(|placement| {
                    (
                        placement.source,
                        (
                            placement.section_index + 1,
                            *section_page_counts
                                .get(&placement.section_index)
                                .unwrap_or(&0),
                            placement.displayed_page,
                        ),
                    )
                })
                .collect();
            let mut painted = PaintedElements::from_parts(
                std::mem::take(&mut page.elements),
                std::mem::take(&mut page_policies[page_index]),
            )?;
            // Aligned text moves once, by the change of every field on its
            // line: a STYLEREF and a PAGE side by side move it together.
            let mut changes = Vec::new();
            let mut occurrence_state = substitute_fields(
                &mut painted,
                page_num,
                total_pages,
                (section_index + 1, section_pages),
                &field_sections,
                &bookmark_pages,
                &page_field_values,
                input.sequence_snapshot.as_deref(),
                sequence_boundaries[page_index],
                &mut self.font_manager,
                &mut changes,
            )?;
            if let Some(values) = styleref_values.get(&page.page_number) {
                substitute_styleref(
                    &mut painted,
                    values,
                    &mut std::collections::HashSet::new(),
                    &mut self.font_manager,
                    &mut changes,
                    &mut occurrence_state,
                )?;
            }
            if !changes.is_empty() {
                realign_tab_aligned_text(painted.coordinate_elements_mut(), &changes);
            }
            for diagnostic in occurrence_state.diagnostics {
                push_unique_diagnostic(&mut diagnostics, diagnostic.message);
            }
            let (elements, mut policies) = painted.into_parts()?;
            policies.retire_annotations();
            policies.validate(&elements)?;
            (page.elements, page_policies[page_index]) = (elements, policies);
            substitution_inputs.push(Some(inputs));
        }

        if restart_checkpoint.is_some()
            && let Some(cache) = self.restart_cache.as_ref()
            && cache.font_trace == font_trace
        {
            for (page_index, (page, retained)) in pages.iter().zip(&cache.raw_pages).enumerate() {
                if Arc::ptr_eq(page, retained)
                    && raw_policies.as_ref().and_then(|maps| maps.get(page_index))
                        == cache.raw_policies.get(page_index)
                {
                    reuse_result_pages[page_index] = true;
                }
            }
        }
        // Metrics-only empty carriers must still resolve through the result,
        // but they do not get to move a glyph-bearing font earlier in the
        // deterministic result order.
        let mut carrier_fonts = Vec::new();
        fn collect_carrier_fonts(elements: &[PositionedElement], fonts: &mut Vec<FontId>) {
            for element in elements {
                match element {
                    PositionedElement::Text(run)
                        if run.text.is_empty() && run.glyph_ids.is_empty() =>
                    {
                        if !fonts.contains(&run.font_id) {
                            fonts.push(run.font_id);
                        }
                    }
                    PositionedElement::Group(group) => {
                        collect_carrier_fonts(&group.children, fonts)
                    }
                    PositionedElement::MarkedContent { children, .. } => {
                        collect_carrier_fonts(children, fonts)
                    }
                    _ => {}
                }
            }
        }
        for page in &pages {
            collect_carrier_fonts(&page.elements, &mut carrier_fonts);
        }
        self.font_manager.replay_layout_font_trace(&carrier_fonts);

        // Remap persistent manager ids to result-local ids and omit faces that
        // are no longer present in the current layout.
        let fonts = if self.font_manager.every_loaded_font_is_current() {
            self.font_manager.all_font_data()
        } else {
            let current_fonts = self.font_manager.current_layout_fonts().to_vec();
            canonicalize_layout_fonts(&mut pages, &self.font_manager, &current_fonts)?
        };
        if let Some(cache) = self.restart_cache.as_ref() {
            for (page_index, reuse) in reuse_result_pages.into_iter().enumerate() {
                if reuse
                    && cache.policies.get(page_index) == page_policies.get(page_index)
                    && let Some(retained) = cache.pages.get(page_index)
                {
                    pages[page_index] = Arc::clone(retained);
                }
            }
        }
        let mut retained_pages = restart_record_eligible.then(|| pages.clone());

        // Convert core properties to document metadata
        let language = document_language(&input.styles).or_else(|| {
            input
                .core_properties
                .as_ref()
                .and_then(|cp| cp.language.as_deref())
                .map(str::trim)
                .filter(|language| oxoffice_layout::is_language_tag(language))
                .map(str::to_owned)
        });
        let metadata = (input.core_properties.is_some() || language.is_some()).then(|| {
            let cp = input.core_properties.as_ref();
            DocumentMetadata {
                title: cp.and_then(|cp| cp.title.clone()),
                author: cp.and_then(|cp| cp.creator.clone()),
                subject: cp.and_then(|cp| cp.subject.clone()),
                keywords: cp.and_then(|cp| cp.keywords.clone()),
                creator: Some("oxoffice".to_string()),
                language,
            }
        });

        if restart_record_eligible
            && pages.len().max(checkpoints.len()) <= RESTART_CACHE_MAX_ENTRIES
            && let Some(raw_pages) = raw_pages.as_mut()
            && let Some(retained_pages) = retained_pages.as_mut()
        {
            let old_body = self
                .restart_cache
                .as_ref()
                .map(|cache| cache.body.as_slice());
            let prefix = first_changed.unwrap_or(0);
            let suffix = common_suffix.unwrap_or(0);
            let new_len = input.document.body.content.len();
            let old_len = old_body.map_or(0, <[RestartBodyEntry]>::len);
            let mut body = input
                .document
                .body
                .content
                .iter()
                .enumerate()
                .filter_map(|(index, content)| {
                    if index < prefix {
                        return old_body.and_then(|body| body.get(index)).cloned();
                    }
                    if index >= new_len.saturating_sub(suffix) {
                        let old_index = old_len.saturating_sub(new_len - index);
                        return old_body.and_then(|body| body.get(old_index)).cloned();
                    }
                    RestartBodyEntry::for_content_memoized(
                        content,
                        input.revision_view,
                        index,
                        &mut restart_identity_memo,
                    )
                })
                .collect::<Vec<_>>();
            let body_complete = body.len() == new_len;
            body.shrink_to_fit();
            raw_pages.shrink_to_fit();
            retained_pages.shrink_to_fit();
            substitution_inputs.shrink_to_fit();
            outlines.shrink_to_fit();
            checkpoints.shrink_to_fit();
            font_trace.shrink_to_fit();
            for fragments in &mut body_fragments {
                fragments.shrink_to_fit();
            }
            body_fragments.shrink_to_fit();
            let mut candidate = RestartCache {
                paint: producer_paints.expect("restart candidate has actual Word producers"),
                body,
                with_provenance: sources.is_some(),
                body_fragments: body_fragments.clone(),
                raw_pages: std::mem::take(raw_pages),
                raw_policies: raw_policies
                    .take()
                    .expect("raw artifacts retain companion maps"),
                policies: page_policies.clone(),
                pages: std::mem::take(retained_pages),
                substitution_inputs,
                outlines: outlines.clone(),
                checkpoints,
                font_trace,
                bytes: 0,
            };
            candidate.outlines.shrink_to_fit();
            for inputs in candidate.substitution_inputs.iter_mut().flatten() {
                inputs.bookmark_pages.shrink_to_fit();
                inputs.font_identity.shrink_to_fit();
                inputs.page_format_context.shrink_to_fit();
            }
            let bytes = if body_complete {
                restart_cache_bytes(&candidate)
            } else {
                usize::MAX
            };
            let entries = restart_cache_entries(&candidate);
            if self.restart_candidate_fits_aggregate(entries, bytes) {
                candidate.bytes = bytes;
                self.restart_cache = Some(candidate);
            } else {
                self.restart_cache = None;
            }
        } else {
            self.restart_cache = None;
        }
        drop(restart_identity_memo);
        self.navigation_by_source
            .extend(num_state.take_navigation());
        if let Some(registry) = sources
            && input
                .sequence_snapshot
                .as_ref()
                .is_none_or(|snapshot| snapshot.navigation_failure.is_none())
        {
            let mut positions = HashMap::new();
            for (page_index, page) in pages.iter().enumerate() {
                oxoffice_layout::walk(&page.elements, &mut |element, transform| {
                    let (source, origin) = match element {
                        PositionedElement::Text(run) => (run.source, run.origin),
                        PositionedElement::MultilingualText(run) => (run.source, run.origin),
                        _ => return,
                    };
                    if let Some(source) = source {
                        positions
                            .entry(source.node)
                            .or_insert((page_index, transform.apply(origin).y));
                    }
                });
            }
            for (node, outline) in body_heading_sources.iter().zip(&outlines) {
                positions.insert(*node, (outline.page_index, outline.y_position));
            }
            outlines = registry
                .nodes
                .iter()
                .enumerate()
                .filter_map(|(index, path)| {
                    if path.story != WordStory::Document {
                        return None;
                    }
                    let node = SourceNodeId::new(u32::try_from(index + 1).ok()?)?;
                    let facts = self.navigation_by_source.get(&node)?;
                    let level = facts.level?;
                    let (page_index, y_position) = positions.get(&node).copied()?;
                    let title = match num_state.resolved_for_source(node) {
                        Some(number)
                            if !number.marker_text.is_empty()
                                && number.marker_rpr.vanish != Some(true) =>
                        {
                            format!("{} {}", number.marker_text, facts.text)
                        }
                        _ => facts.text.clone(),
                    };
                    Some(oxoffice_layout::OutlineEntry {
                        title,
                        level,
                        page_index,
                        y_position,
                    })
                })
                .collect();
        }
        let mut result = LayoutResult::new(pages, fonts, metadata, outlines);
        diagnostics.extend(self.font_manager.current_font_diagnostics().iter().cloned());
        diagnostics.extend(self.font_manager.word_substitution_diagnostics());
        diagnostics.extend(oxoffice_layout::missing_glyph_diagnostics(
            &result.pages,
            &result.fonts,
            "render with system fonts (CLI --system-fonts, Python system_fonts=True, Rust \
             RenderOptions::with_system_fonts) on a machine with a font for it, or pass a font \
             file (CLI --font-dir, Python fonts= or font_dir=, Rust to_pdf_with_fonts)",
        ));
        result.diagnostics = diagnostics;
        self.last_picture_draw_owners = media.picture_draw_owners_since(0);
        result.structure = Some(structure);
        if input.form_fields
            && let Some(sources) = sources
        {
            crate::forms::number_form_fields(&mut result, input, sources);
        }
        let references = num_state.references_only();
        self.bookmark_numbering
            .extend(
                num_state
                    .bookmark_numbering()
                    .filter_map(|(name, source, numbering)| {
                        let registered = sources.is_some_and(|registry| {
                            registry.nodes.get(source.get() as usize - 1).is_some()
                        });
                        (registered && bookmark_text(input, name).is_some())
                            .then(|| (name.to_owned(), numbering.clone()))
                    }),
            );
        self.bookmark_suppressed.extend(
            num_state
                .bookmark_sources()
                .filter(|(name, source)| {
                    self.navigation_by_source
                        .get(source)
                        .is_some_and(|facts| facts.suppressed_numbering)
                        && bookmark_text(input, name).is_some()
                })
                .map(|(name, _)| name.to_owned()),
        );
        self.numbering_by_source.extend(num_state.take_resolved());
        self.last_body_fragments = body_fragments;
        Ok((result, references))
    }

    #[allow(clippy::too_many_arguments)]
    fn layout_body_paragraph(
        &mut self,
        paragraph: &CT_P,
        content_width: f64,
        styles: &CT_Styles,
        input: &LayoutInput,
        media: &MediaRegistry,
        numbering: &mut NumberingState,
        diagnostics: &mut Vec<Diagnostic>,
        source_node: Option<SourceNodeId>,
        doc_grid: Option<&CT_DocGrid>,
    ) -> Result<SharedLayoutBlock> {
        let excluded_owner = source_node
            .and_then(|node| input.sequence_snapshot.as_ref()?.source_node(node))
            .is_some_and(|path| {
                input
                    .sequence_snapshot
                    .as_ref()
                    .is_some_and(|s| s.navigation_exclusions.contains_key(path))
            });
        // A block control field's paragraph draws marks, which a cached
        // block would not carry.
        let form = crate::forms::body_form_paragraph(input, source_node);
        let form_block = form.is_some_and(|paragraph| paragraph.block);
        // A bookmark marker between blocks belongs to this paragraph too.
        let block_markers = input.block_bookmarks();
        let (block_before, block_after) = block_markers.markers(paragraph);
        let block_bookmarks = !block_before.is_empty() || !block_after.is_empty();
        if !paragraph_is_cache_safe(paragraph, styles)
            || block_bookmarks
            || excluded_owner
            || form_block
        {
            // Traversal-sensitive content can change generated state consumed
            // by later blocks. The conservative boundary is the first such
            // block, after which no retained block is read in this layout.
            self.paragraph_cache_reads_enabled = false;
            let (block, reflow_direction, paint) = layout_paragraph_with_source_and_direction(
                paragraph,
                content_width,
                styles,
                input,
                media,
                &mut self.font_manager,
                numbering,
                diagnostics,
                source_node,
                source_node,
                doc_grid,
                form,
            )?;
            return Ok(SharedLayoutBlock::Owned {
                semantics: Some(ParagraphSemantics {
                    tb_rl_exact_baseline: None,
                    border_group_before: false,
                    source_node,
                    structure_id: block.structure_id,
                    reflow_direction,
                    paint: Arc::new(paint),
                }),
                border_group_before: false,
                block: Box::new(LayoutBlock::Paragraph(block)),
                reflow_direction,
                body_index: None,
            });
        }

        let fingerprint = paragraph_fingerprint(paragraph);
        let notes_allow_reuse = self.paragraph_cache_notes_match
            || paragraph_note_references(paragraph, input.revision_view)?.is_empty();
        if self.paragraph_cache_reads_enabled
            && notes_allow_reuse
            && let Some(entry) = self.paragraph_cache.iter().find(|entry| {
                entry.fingerprint == fingerprint
                    && entry.key.paragraph == *paragraph
                    && entry.key.content_width_bits == content_width.to_bits()
                    && entry.key.revision_view == input.revision_view
                    && entry.key.revision_style == input.revision_style
                    && entry.key.doc_grid.as_ref() == doc_grid
            })
        {
            diagnostics.extend(entry.diagnostics.iter().cloned());
            self.font_manager
                .replay_font_trace_events(&entry.font_trace)?;
            return Ok(SharedLayoutBlock::Paragraph {
                block: Arc::clone(&entry.block),
                semantics: ParagraphSemantics {
                    tb_rl_exact_baseline: None,
                    border_group_before: false,
                    source_node,
                    structure_id: None,
                    reflow_direction: entry.reflow_direction,
                    paint: Arc::clone(&entry.paint),
                },
                body_index: None,
            });
        }

        let diagnostics_start = diagnostics.len();
        self.font_manager.begin_paragraph_font_trace();
        let block_result = layout_paragraph_with_source_and_direction(
            paragraph,
            content_width,
            styles,
            input,
            media,
            &mut self.font_manager,
            numbering,
            diagnostics,
            Some(CACHE_SOURCE_NODE),
            source_node,
            doc_grid,
            // A cache-safe paragraph holds no control and no field.
            None,
        );
        let font_trace = self.font_manager.finish_paragraph_font_events();
        let (mut block, reflow_direction, paint) = block_result?;
        let paint = Arc::new(paint);

        let cached_diagnostics = diagnostics[diagnostics_start..].to_vec();
        if let Some(font_trace) = font_trace {
            let bytes = paragraph_cache_entry_bytes(
                paragraph,
                &block,
                Some(&paint),
                &cached_diagnostics,
                font_contribution_trace_bytes(&font_trace),
            );
            let block = Arc::new(block);
            self.stage_paragraph_cache_entry(ParagraphCacheEntry {
                fingerprint,
                key: ParagraphCacheKey {
                    paragraph: paragraph.clone(),
                    content_width_bits: content_width.to_bits(),
                    revision_view: input.revision_view,
                    revision_style: input.revision_style,
                    doc_grid: doc_grid.cloned(),
                },
                block: Arc::clone(&block),
                paint: Arc::clone(&paint),
                diagnostics: cached_diagnostics,
                font_trace,
                reflow_direction,
                bytes,
            });
            return Ok(SharedLayoutBlock::Paragraph {
                block,
                semantics: ParagraphSemantics {
                    tb_rl_exact_baseline: None,
                    border_group_before: false,
                    source_node,
                    structure_id: None,
                    reflow_direction,
                    paint: Arc::clone(&paint),
                },
                body_index: None,
            });
        }

        rebind_paragraph_source(&mut block, source_node)?;
        Ok(SharedLayoutBlock::Owned {
            semantics: Some(ParagraphSemantics {
                tb_rl_exact_baseline: None,
                border_group_before: false,
                source_node,
                structure_id: block.structure_id,
                reflow_direction,
                paint,
            }),
            border_group_before: false,
            block: Box::new(LayoutBlock::Paragraph(block)),
            reflow_direction,
            body_index: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn layout_body_table(
        &mut self,
        table: &CT_Tbl,
        content_width: f64,
        styles: &CT_Styles,
        input: &LayoutInput,
        media: &MediaRegistry,
        numbering: &mut NumberingState,
        diagnostics: &mut Vec<Diagnostic>,
        sources: Option<&SourceRegistry>,
        story: &WordStory,
        path: &[usize],
        doc_grid: Option<&CT_DocGrid>,
    ) -> Result<SharedLayoutBlock> {
        let excluded_owner = input.sequence_snapshot.as_ref().is_some_and(|s| {
            s.navigation_owners.contains(&WordSourcePath {
                story: story.clone(),
                children: path.to_vec(),
            })
        });
        if !table_is_cache_safe(table, styles) || excluded_owner {
            self.paragraph_cache_reads_enabled = false;
            return table::layout_table_with_provenance(
                table,
                content_width,
                styles,
                input,
                media,
                &mut self.font_manager,
                numbering,
                diagnostics,
                sources,
                story,
                path,
                doc_grid,
            )
            .map(|(block, semantics)| SharedLayoutBlock::Table {
                block: Arc::new(block),
                semantics,
                body_index: None,
            });
        }

        let fingerprint = table_fingerprint(table);
        if self.paragraph_cache_reads_enabled
            && self.retained_context_matches_full
            && let Some(entry) = self.table_cache.iter().find(|entry| {
                entry.fingerprint == fingerprint
                    && entry.key.table == *table
                    && entry.key.content_width_bits == content_width.to_bits()
                    && entry.key.revision_view == input.revision_view
                    && entry.key.revision_style == input.revision_style
                    && entry.key.with_provenance == sources.is_some()
                    && entry.key.doc_grid.as_ref() == doc_grid
            })
        {
            diagnostics.extend(entry.diagnostics.iter().cloned());
            self.font_manager
                .replay_font_trace_events(&entry.font_trace)?;
            if let Some(sources) = sources {
                record_cached_table_omissions(table, &entry.block, sources, story, path, numbering);
            }
            return Ok(SharedLayoutBlock::Table {
                block: Arc::clone(&entry.block),
                semantics: table_semantics(
                    table,
                    entry.block.as_ref(),
                    &entry.semantics,
                    sources,
                    story,
                    path,
                ),
                body_index: None,
            });
        }

        let diagnostics_start = diagnostics.len();
        self.font_manager.begin_paragraph_font_trace();
        let block_result = table::layout_table_with_provenance(
            table,
            content_width,
            styles,
            input,
            media,
            &mut self.font_manager,
            numbering,
            diagnostics,
            sources,
            story,
            path,
            doc_grid,
        );
        let font_trace = self.font_manager.finish_paragraph_font_events();
        let (mut block, semantics) = block_result?;

        let cached_diagnostics = diagnostics[diagnostics_start..].to_vec();
        if let Some(font_trace) = font_trace {
            canonicalize_table_sources(&mut block)?;
            let key = TableCacheKey {
                table: table.clone(),
                content_width_bits: content_width.to_bits(),
                revision_view: input.revision_view,
                revision_style: input.revision_style,
                with_provenance: sources.is_some(),
                doc_grid: doc_grid.cloned(),
            };
            let bytes = table_cache_entry_bytes(
                &key,
                &block,
                &semantics,
                &cached_diagnostics,
                font_contribution_trace_bytes(&font_trace),
            );
            let block = Arc::new(block);
            self.stage_table_cache_entry(TableCacheEntry {
                fingerprint,
                key,
                block: Arc::clone(&block),
                semantics: semantics.clone(),
                diagnostics: cached_diagnostics,
                font_trace,
                bytes,
            });
            return Ok(SharedLayoutBlock::Table {
                block,
                semantics,
                body_index: None,
            });
        }
        Ok(SharedLayoutBlock::Table {
            block: Arc::new(block),
            semantics,
            body_index: None,
        })
    }

    fn publish_paragraph_cache_entry(&mut self, entry: ParagraphCacheEntry) {
        if entry.bytes > PARAGRAPH_CACHE_MAX_BYTES {
            return;
        }
        while self.paragraph_cache.len() >= PARAGRAPH_CACHE_MAX_ENTRIES
            || self.paragraph_cache_bytes.saturating_add(entry.bytes) > PARAGRAPH_CACHE_MAX_BYTES
        {
            let Some(evicted) = self.paragraph_cache.pop_front() else {
                break;
            };
            self.paragraph_cache_bytes = self.paragraph_cache_bytes.saturating_sub(evicted.bytes);
        }
        let Some(bytes) = self.paragraph_cache_bytes.checked_add(entry.bytes) else {
            return;
        };
        if bytes > PARAGRAPH_CACHE_MAX_BYTES {
            return;
        }
        self.paragraph_cache_bytes = bytes;
        self.paragraph_cache.push_back(entry);
    }

    fn restart_candidate_fits_aggregate(
        &self,
        candidate_entries: usize,
        candidate_bytes: usize,
    ) -> bool {
        let pending_paragraph_entries = self
            .pending_paragraph_cache
            .as_ref()
            .map_or(0, VecDeque::len);
        let pending_table_entries = self.pending_table_cache.as_ref().map_or(0, VecDeque::len);
        let pending_header_footer_entries = self
            .pending_header_footer_cache
            .as_ref()
            .map_or(0, VecDeque::len);
        let entries = self
            .paragraph_cache
            .len()
            .checked_add(self.table_cache.len())
            .and_then(|entries| entries.checked_add(self.header_footer_cache.len()))
            .and_then(|entries| entries.checked_add(pending_paragraph_entries))
            .and_then(|entries| entries.checked_add(pending_table_entries))
            .and_then(|entries| entries.checked_add(pending_header_footer_entries))
            .and_then(|entries| entries.checked_add(candidate_entries));
        let bytes = self
            .paragraph_cache_bytes
            .checked_add(self.table_cache_bytes)
            .and_then(|bytes| bytes.checked_add(self.header_footer_cache_bytes))
            .and_then(|bytes| bytes.checked_add(self.pending_paragraph_cache_bytes))
            .and_then(|bytes| bytes.checked_add(self.pending_table_cache_bytes))
            .and_then(|bytes| bytes.checked_add(self.pending_header_footer_cache_bytes))
            .and_then(|bytes| bytes.checked_add(candidate_bytes));
        entries.is_some_and(|entries| entries <= CACHE_MAX_ENTRIES)
            && bytes.is_some_and(|bytes| bytes <= CACHE_MAX_BYTES)
    }

    fn stage_paragraph_cache_entry(&mut self, entry: ParagraphCacheEntry) {
        if entry.bytes > PARAGRAPH_CACHE_MAX_BYTES {
            return;
        }
        let Some(pending) = self.pending_paragraph_cache.as_mut() else {
            return;
        };
        while pending.len() >= PARAGRAPH_CACHE_MAX_ENTRIES
            || self
                .pending_paragraph_cache_bytes
                .saturating_add(entry.bytes)
                > PARAGRAPH_CACHE_MAX_BYTES
        {
            let Some(evicted) = pending.pop_front() else {
                break;
            };
            self.pending_paragraph_cache_bytes = self
                .pending_paragraph_cache_bytes
                .saturating_sub(evicted.bytes);
        }
        self.pending_paragraph_cache_bytes += entry.bytes;
        pending.push_back(entry);
    }

    fn publish_table_cache_entry(&mut self, entry: TableCacheEntry) {
        if entry.bytes > TABLE_CACHE_MAX_BYTES {
            return;
        }
        while self.table_cache.len() >= TABLE_CACHE_MAX_ENTRIES
            || self.table_cache_bytes.saturating_add(entry.bytes) > TABLE_CACHE_MAX_BYTES
        {
            let Some(evicted) = self.table_cache.pop_front() else {
                break;
            };
            self.table_cache_bytes = self.table_cache_bytes.saturating_sub(evicted.bytes);
        }
        self.table_cache_bytes += entry.bytes;
        self.table_cache.push_back(entry);
        let restart_entries = self.restart_cache.as_ref().map_or(0, restart_cache_entries);
        let restart_bytes = self.restart_cache.as_ref().map_or(0, |cache| cache.bytes);
        debug_assert!(
            self.paragraph_cache.len() + self.table_cache.len() + restart_entries
                <= CACHE_MAX_ENTRIES
        );
        debug_assert!(
            self.paragraph_cache_bytes + self.table_cache_bytes + restart_bytes <= CACHE_MAX_BYTES
        );
    }

    fn stage_table_cache_entry(&mut self, entry: TableCacheEntry) {
        if entry.bytes > TABLE_CACHE_MAX_BYTES {
            return;
        }
        let Some(pending) = self.pending_table_cache.as_mut() else {
            return;
        };
        while pending.len() >= TABLE_CACHE_MAX_ENTRIES
            || self.pending_table_cache_bytes.saturating_add(entry.bytes) > TABLE_CACHE_MAX_BYTES
        {
            let Some(evicted) = pending.pop_front() else {
                break;
            };
            self.pending_table_cache_bytes =
                self.pending_table_cache_bytes.saturating_sub(evicted.bytes);
        }
        self.pending_table_cache_bytes += entry.bytes;
        pending.push_back(entry);
    }

    fn publish_header_footer_cache_entry(&mut self, entry: HeaderFooterCacheEntry) {
        if entry.bytes > HEADER_FOOTER_CACHE_MAX_BYTES {
            return;
        }
        while self.header_footer_cache.len() >= HEADER_FOOTER_CACHE_MAX_ENTRIES
            || self.header_footer_cache_bytes.saturating_add(entry.bytes)
                > HEADER_FOOTER_CACHE_MAX_BYTES
        {
            let Some(evicted) = self.header_footer_cache.pop_front() else {
                break;
            };
            self.header_footer_cache_bytes =
                self.header_footer_cache_bytes.saturating_sub(evicted.bytes);
        }
        self.header_footer_cache_bytes += entry.bytes;
        self.header_footer_cache.push_back(entry);
        let restart_entries = self.restart_cache.as_ref().map_or(0, restart_cache_entries);
        let restart_bytes = self.restart_cache.as_ref().map_or(0, |cache| cache.bytes);
        debug_assert!(
            self.paragraph_cache.len()
                + self.table_cache.len()
                + self.header_footer_cache.len()
                + restart_entries
                <= CACHE_MAX_ENTRIES
        );
        debug_assert!(
            self.paragraph_cache_bytes
                + self.table_cache_bytes
                + self.header_footer_cache_bytes
                + restart_bytes
                <= CACHE_MAX_BYTES
        );
    }

    pub(super) fn stage_header_footer_cache_entry(&mut self, entry: HeaderFooterCacheEntry) {
        if entry.bytes > HEADER_FOOTER_CACHE_MAX_BYTES {
            return;
        }
        let Some(pending) = self.pending_header_footer_cache.as_mut() else {
            return;
        };
        while pending.len() >= HEADER_FOOTER_CACHE_MAX_ENTRIES
            || self
                .pending_header_footer_cache_bytes
                .saturating_add(entry.bytes)
                > HEADER_FOOTER_CACHE_MAX_BYTES
        {
            let Some(evicted) = pending.pop_front() else {
                break;
            };
            self.pending_header_footer_cache_bytes = self
                .pending_header_footer_cache_bytes
                .saturating_sub(evicted.bytes);
        }
        self.pending_header_footer_cache_bytes += entry.bytes;
        pending.push_back(entry);
    }
}

/// The document's main language, as Word reads it: the language of the
/// default paragraph style, else of the document defaults (`w:lang/@w:val`).
/// A value that is not a language tag (Word's `x-none`) names none.
fn document_language(styles: &CT_Styles) -> Option<String> {
    let language = |rpr: Option<&CT_RPr>| {
        rpr.and_then(|rpr| rpr.language.as_deref())
            .map(str::trim)
            .filter(|language| oxoffice_layout::is_language_tag(language))
            .map(str::to_owned)
    };
    language(
        styles
            .get_default(oxoffice_docx_oxml::styles::StyleType::Paragraph)
            .and_then(|style| style.rpr.as_ref()),
    )
    .or_else(|| {
        language(
            styles
                .doc_defaults
                .as_ref()
                .and_then(|defaults| defaults.rpr.as_ref()),
        )
    })
}
