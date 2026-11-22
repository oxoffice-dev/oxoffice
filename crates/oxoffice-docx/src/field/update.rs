//! Updating fields: the update methods, page fields and how updates reach the model.

use super::*;

impl Document {
    /// Evaluate every typed field without mutating stored results.
    pub fn evaluate_fields(
        &self,
        context: &FieldEvaluationContext,
    ) -> Result<Vec<FieldEvaluation>> {
        let mut source = self.clone_for_staging();
        source.flush_dirty_related_story_models()?;
        prepare_physical_story_projection(&mut source.document.body, &mut [])?;
        let (evaluations, visible) = source.evaluate_fields_with_policy(context, false)?;
        Ok(evaluations
            .into_iter()
            .zip(visible)
            .filter_map(|(evaluation, visible)| visible.then_some(evaluation))
            .enumerate()
            .map(|(index, mut evaluation)| {
                evaluation.field_index = index;
                evaluation
            })
            .collect())
    }

    fn evaluate_fields_with_policy(
        &self,
        context: &FieldEvaluationContext,
        missing_merge_fields_as_empty: bool,
    ) -> Result<(Vec<FieldEvaluation>, Vec<bool>)> {
        let source = self;
        let mut evaluator = if missing_merge_fields_as_empty {
            Evaluator::for_mail_merge(source, context)
        } else {
            Evaluator::new(source, context)
        };
        let input = source.build_layout_input()?;
        let snapshot = oxoffice_docx_layout::engine::evaluate_sequence_fields(&input)
            .map_err(|error| Error::Other(format!("sequence source evaluation failed: {error}")))?;
        let mut nodes_by_story = HashMap::<
            oxoffice_docx_layout::WordStory,
            Vec<oxoffice_docx_layout::SourceNodeId>,
        >::new();
        for index in 1u32.. {
            let Some(node) = oxoffice_docx_layout::SourceNodeId::new(index) else {
                break;
            };
            let Some(path) = snapshot.source_node(node) else {
                break;
            };
            nodes_by_story
                .entry(path.story.clone())
                .or_default()
                .push(node);
        }
        evaluator.sequence_snapshot = Some(Arc::new(snapshot));
        let mut main = Vec::new();
        collect_body_paragraphs(&source.document.body, &mut main, None);
        evaluator.sequence_nodes = nodes_by_story
            .get(&oxoffice_docx_layout::WordStory::Document)
            .map(|nodes| nodes.iter().copied().map(Some).collect())
            .unwrap_or_default();
        evaluator.refresh_main_bookmark_sequence_text(&main)?;
        evaluator.evaluate_story("main", &main)?;

        let general_parts = [true, false]
            .into_iter()
            .flat_map(|header| referenced_header_footer_parts(source, header, false))
            .map(|(part, _)| part)
            .collect::<HashSet<_>>();
        for (part_name, kind, xml) in field_story_parts(source, true) {
            let records = legacy_story_paragraphs(&xml, kind)?;
            evaluator.general_field_contexts = records
                .iter()
                .map(|record| {
                    record.general_context
                        && (!matches!(kind, PackageStoryKind::Header | PackageStoryKind::Footer)
                            || general_parts.contains(&part_name))
                })
                .collect();
            let paragraphs = records
                .iter()
                .map(|entry| &entry.paragraph)
                .collect::<Vec<_>>();
            let mut owners = nodes_by_story
                .iter()
                .filter(|(story, _)| {
                    input.story_part_names.get(*story) == Some(&part_name)
                        && matches!(
                            (story, kind),
                            (
                                oxoffice_docx_layout::WordStory::Header { .. },
                                PackageStoryKind::Header
                            ) | (
                                oxoffice_docx_layout::WordStory::Footer { .. },
                                PackageStoryKind::Footer
                            ) | (
                                oxoffice_docx_layout::WordStory::Footnote { .. },
                                PackageStoryKind::Footnotes
                            ) | (
                                oxoffice_docx_layout::WordStory::Endnote { .. },
                                PackageStoryKind::Endnotes
                            )
                        )
                })
                .collect::<Vec<_>>();
            owners.sort_by_key(|(_, nodes)| nodes.first().map(|id| id.get()));
            evaluator.sequence_nodes =
                if matches!(kind, PackageStoryKind::Header | PackageStoryKind::Footer) {
                    // A header or footer no section shows, such as the even
                    // page header LibreOffice keeps with even and odd pages
                    // off, is not laid out: its fields evaluate without a
                    // physical identity, as comments do. One the layout
                    // registers keeps the strict paragraph count.
                    let laid_out = input.story_part_names.iter().any(|(story, part)| {
                        part == &part_name
                            && matches!(
                                (story, kind),
                                (
                                    oxoffice_docx_layout::WordStory::Header { .. },
                                    PackageStoryKind::Header
                                ) | (
                                    oxoffice_docx_layout::WordStory::Footer { .. },
                                    PackageStoryKind::Footer
                                )
                            )
                    });
                    if laid_out {
                        owners
                            .first()
                            .map(|(_, nodes)| nodes.iter().copied().map(Some).collect())
                            .unwrap_or_default()
                    } else {
                        vec![None; paragraphs.len()]
                    }
                } else {
                    // Each note owner is physically distinct, unlike relationship aliases to furniture.
                    owners
                        .into_iter()
                        .flat_map(|(_, nodes)| nodes.iter().copied().map(Some))
                        .collect()
                };
            let story = match kind {
                PackageStoryKind::Header => format!("header:{part_name}"),
                PackageStoryKind::Footer => format!("footer:{part_name}"),
                PackageStoryKind::Footnotes => "footnotes".into(),
                PackageStoryKind::Endnotes => "endnotes".into(),
                PackageStoryKind::Comments => "comments".into(),
                PackageStoryKind::Document => "document".into(),
            };
            if matches!(kind, PackageStoryKind::Comments) {
                evaluator.sequence_nodes = vec![None; paragraphs.len()];
            }
            evaluator.evaluate_story(&story, &paragraphs)?;
        }

        for source in source.text_box_cache_paragraphs()? {
            let paragraph = parsed_physical_field_paragraph(&source.xml)?;
            let mut fields = Vec::new();
            let mut root_indices = HashSet::new();
            for run in paragraph.source_runs() {
                for content in &run.content {
                    if let RunContent::Field(field) = content {
                        root_indices.insert(fields.len());
                        let mut pending = vec![(field, false, false)];
                        while let Some((field, inherited_lock, cached)) = pending.pop() {
                            let locked = inherited_lock || field.locked() == Some(true);
                            fields.push((field, locked, cached));
                            pending.extend(
                                field
                                    .cached_fields_in_source_order()
                                    .into_iter()
                                    .rev()
                                    .map(|child| (child, locked, true)),
                            );
                            pending.extend(
                                field
                                    .nested_fields_in_source_order()
                                    .into_iter()
                                    .rev()
                                    .map(|child| (child, locked, cached)),
                            );
                        }
                    }
                }
            }
            if fields.is_empty() {
                continue;
            }
            let snapshot = evaluator
                .sequence_snapshot
                .as_ref()
                .expect("sequence inventory");
            let mut matching = Vec::new();
            for (story, nodes) in &nodes_by_story {
                if !matches!(story, oxoffice_docx_layout::WordStory::TextBox { part_name, .. } if part_name == source.location.story().part_name())
                    || snapshot.text_box_owner_index(story) != Some(source.owner_index)
                {
                    continue;
                }
                let Some(node) = nodes.get(source.paragraph_index).copied() else {
                    continue;
                };
                let mut enclosing_qualified = false;
                let qualified =
                    fields
                        .iter()
                        .enumerate()
                        .all(|(index, (field, locked, cached))| {
                            let root = root_indices.contains(&index);
                            if root {
                                enclosing_qualified = false;
                            }
                            let source = oxoffice_layout::FieldSource {
                                node,
                                index: index as u32,
                            };
                            if snapshot.source_field_context(source)
                                != Some((
                                    field.effective_instruction_text().as_str(),
                                    *locked,
                                    *cached,
                                ))
                            {
                                return false;
                            }
                            let model_raw = field
                                .source_replacement()
                                .ok()
                                .flatten()
                                .map(|(raw, _)| raw);
                            let source_raw = snapshot.source_field_xml(source);
                            match (model_raw, source_raw) {
                                (Some(model), Some(source)) if model == source => {
                                    if root {
                                        enclosing_qualified = true;
                                    }
                                    true
                                }
                                (None, None) if !root && enclosing_qualified => true,
                                _ => false,
                            }
                        });
                if qualified
                    && snapshot
                        .source_field_context(oxoffice_layout::FieldSource {
                            node,
                            index: fields.len() as u32,
                        })
                        .is_none()
                {
                    matching.push(node);
                }
            }
            evaluator.sequence_nodes = vec![(matching.len() == 1).then(|| matching[0])];
            evaluator.general_field_contexts = vec![false];
            evaluator.evaluate_story("textbox", &[&paragraph])?;
        }

        Ok((evaluator.results, evaluator.visible_results))
    }

    /// Evaluate and materialize every typed field cache in document order.
    pub fn update_fields(&mut self, context: &FieldEvaluationContext) -> Result<usize> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_dirty_related_story_models()?;
        let updated = candidate.update_fields_with_policy(context, false)?;
        if updated != 0 {
            self.commit_staged_mutation(candidate.prepare_and_reopen_staged()?);
        }
        Ok(updated)
    }

    /// Refresh ordinary and deterministic layout caches in one outer transaction.
    /// Omitted time remains unresolved. Ordinary retention diagnostics precede layout diagnostics.
    pub fn update_all_fields(
        &mut self,
        context: &FieldEvaluationContext,
    ) -> Result<FieldsUpdateReport> {
        let mut candidate = self.clone_for_staging();
        let evaluations = candidate.evaluate_fields(context)?;
        let ordinary = candidate.update_fields(context)?;
        let layout = candidate.update_layout_backed_fields()?;
        let mut diagnostics = evaluations
            .iter()
            .filter_map(|field| match &field.outcome {
                FieldOutcome::KeepStored { diagnostic } => Some(diagnostic.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        diagnostics.extend(layout.diagnostics.iter().cloned());
        let total_updated = ordinary
            .checked_add(layout.updated_count())
            .ok_or_else(|| Error::Other("field update count overflow".to_owned()))?;
        if total_updated != 0 {
            self.commit_staged_mutation(candidate.prepare_and_reopen_staged()?);
        }
        Ok(FieldsUpdateReport {
            ordinary_updated: ordinary,
            layout,
            evaluations,
            diagnostics,
            total_updated,
            saved_cache_scope: "supported-typed-and-placed-caches-with-dynamic-header-footer-page-caches-retained",
        })
    }

    /// Write layout-backed field caches and return the number changed.
    ///
    /// PAGE takes its placed story owner's displayed number. Word preserves
    /// dynamic PAGE and NUMPAGES caches in headers and footers on save, so
    /// those caches remain unchanged. PAGEREF takes its resolved
    /// bookmark. Written fields are marked clean. Every other field keeps its
    /// cache, as does a layout-backed field that layout does not place, that
    /// uses an unsupported switch, or whose section page number format layout
    /// does not reproduce. `update_fields` still defers these kinds. Use
    /// [`Self::update_layout_backed_fields`] for per-kind counts and layout
    /// diagnostics.
    pub fn update_page_fields(&mut self) -> Result<usize> {
        Ok(self.update_layout_backed_fields()?.updated_count())
    }

    /// Write page, section, total and resolved bookmark page caches from one layout.
    ///
    /// Each field uses its immutable placement and owning section. Header and
    /// footer PAGE and NUMPAGES caches retain Word's saved-cache behavior. Fields
    /// with an unsupported switch or unresolved target retain their caches.
    /// Successful writes are marked clean and committed atomically.
    pub fn update_layout_backed_fields(&mut self) -> Result<LayoutBackedFieldUpdateReport> {
        let mut candidate = self.clone_for_staging();
        candidate.flush_dirty_related_story_models()?;
        prepare_physical_story_projection(&mut candidate.document.body, &mut [])?;
        let layout = candidate.layout_deterministic()?;
        let (updates, mut report) = candidate.page_field_updates(&layout)?;
        let textbox_updates = candidate.text_box_page_field_updates(&layout, &mut report)?;
        report.diagnostics.extend(
            layout
                .layout
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect::<Vec<_>>(),
        );
        let mut updated = candidate.apply_cached_field_updates(&updates, false)?;
        for patch in textbox_updates {
            candidate.patch_story_paragraph_field_sources(&patch.location, &patch.replacements)?;
            updated += patch.updated;
        }
        debug_assert_eq!(updated, report.updated_count());
        if updated != 0 {
            self.commit_staged_mutation(candidate);
        }
        Ok(report)
    }

    /// Stage one optional cache update per field, in update traversal order,
    /// for every PAGE, NUMPAGES, and PAGEREF field that `layout` places.
    fn page_field_updates(
        &self,
        layout: &oxoffice_docx_layout::WordLayoutResult,
    ) -> Result<(
        Vec<Option<CachedFieldUpdate>>,
        LayoutBackedFieldUpdateReport,
    )> {
        use oxoffice_docx_layout::{SourceNodeId, WordSourcePath, WordStory};

        let placed = placed_page_fields(layout);
        let section_formats = section_page_formats(&self.document);
        let mut updates = Vec::new();
        let mut report = LayoutBackedFieldUpdateReport::default();

        let mut main = Vec::new();
        collect_body_paragraphs(&self.document.body, &mut main, None);
        // Layout registers main-story paragraphs in this same flattened order.
        let main_paths = (1u32..)
            .map_while(|index| SourceNodeId::new(index).and_then(|id| layout.source_node(id)))
            .filter(|path| path.story == WordStory::Document)
            .map(|path| vec![path.clone()])
            .collect::<Vec<_>>();
        if main_paths.len() != main.len() {
            return Err(Error::Other(format!(
                "layout identified {} of {} main story paragraphs",
                main_paths.len(),
                main.len()
            )));
        }
        push_page_field_updates(
            &main,
            &main_paths,
            &placed,
            layout,
            &section_formats,
            &mut updates,
            &mut report,
        );

        let input = self.build_layout_input()?;
        for (part_name, kind, xml) in field_story_parts(self, false) {
            let story = legacy_story_paragraphs(&xml, kind)?;
            let paragraphs = story
                .iter()
                .map(|entry| &entry.paragraph)
                .collect::<Vec<_>>();
            let mut physical_paths = BTreeMap::<String, Vec<WordSourcePath>>::new();
            for path in (1u32..)
                .map_while(|index| SourceNodeId::new(index).and_then(|id| layout.source_node(id)))
            {
                let matches_kind = matches!(
                    (&path.story, kind),
                    (WordStory::Header { .. }, PackageStoryKind::Header)
                        | (WordStory::Footer { .. }, PackageStoryKind::Footer)
                        | (WordStory::Footnote { .. }, PackageStoryKind::Footnotes)
                        | (WordStory::Endnote { .. }, PackageStoryKind::Endnotes)
                );
                if matches_kind && input.story_part_names.get(&path.story) == Some(&part_name) {
                    // Header relationships sharing one physical part describe
                    // alternative placements of the same paragraph inventory.
                    let key = match &path.story {
                        WordStory::Header { relationship_id }
                        | WordStory::Footer { relationship_id } => relationship_id.clone(),
                        _ => String::new(),
                    };
                    physical_paths.entry(key).or_default().push(path.clone());
                }
            }
            let paths = (0..paragraphs.len())
                .map(|index| {
                    physical_paths
                        .values()
                        .filter(|paths| paths.len() == paragraphs.len())
                        .filter_map(|paths| paths.get(index).cloned())
                        .collect()
                })
                .collect::<Vec<_>>();
            push_page_field_updates(
                &paragraphs,
                &paths,
                &placed,
                layout,
                &section_formats,
                &mut updates,
                &mut report,
            );
        }

        Ok((updates, report))
    }

    fn text_box_page_field_updates(
        &self,
        layout: &oxoffice_docx_layout::WordLayoutResult,
        report: &mut LayoutBackedFieldUpdateReport,
    ) -> Result<Vec<TextBoxCachePatch>> {
        use oxoffice_docx_layout::{SourceNodeId, WordSourcePath, WordStory};
        let mut nodes = Vec::new();
        for index in 1u32.. {
            let Some(node) = SourceNodeId::new(index) else {
                break;
            };
            let Some(path) = layout.source_node(node) else {
                break;
            };
            if matches!(path.story, WordStory::TextBox { .. }) {
                nodes.push((node, path));
            }
        }
        let placed = placed_page_fields(layout);
        let section_formats = section_page_formats(&self.document);
        let mut result = Vec::new();
        for source in self.text_box_cache_paragraphs()? {
            let crate::document::TextBoxCacheParagraph {
                location,
                xml,
                owner_index,
                paragraph_index,
            } = source;
            let mut paragraph = parsed_physical_field_paragraph(&xml)?;
            let mut fields = Vec::new();
            for field in paragraph
                .source_runs()
                .into_iter()
                .flat_map(|run| &run.content)
                .filter_map(|content| match content {
                    RunContent::Field(field) => Some(field),
                    _ => None,
                })
            {
                collect_preorder_fields(field, false, &mut fields);
            }
            if fields.is_empty() {
                continue;
            }
            let raw = fields
                .iter()
                .map(|(field, _)| {
                    field
                        .source_replacement()
                        .ok()
                        .flatten()
                        .map(|(source, _)| source.to_vec())
                })
                .collect::<Vec<_>>();
            let matching = nodes.iter().filter(|(node, path)| {
                matches!(&path.story, WordStory::TextBox { part_name, .. } if part_name == location.story().part_name())
                    && layout.text_box_owner_index(&path.story) == Some(owner_index)
                    && layout.text_box_paragraph_index(*node) == Some(paragraph_index)
                    && raw.iter().enumerate().all(|(index, raw)| raw.as_deref().is_some_and(|raw| layout.source_field_xml(oxoffice_layout::FieldSource { node: *node, index: index as u32 }) == Some(raw)))
                    && layout.source_field_xml(oxoffice_layout::FieldSource { node: *node, index: raw.len() as u32 }).is_none()
            }).collect::<Vec<_>>();
            let paths = if matching.len() == 1 {
                vec![matching[0].1.clone()]
            } else {
                Vec::<WordSourcePath>::new()
            };
            let mut updates = Vec::new();
            push_page_field_updates(
                &[&paragraph],
                &[paths],
                &placed,
                layout,
                &section_formats,
                &mut updates,
                report,
            );
            let count = updates.iter().flatten().count();
            if count == 0 {
                continue;
            }
            let original = paragraph.clone();
            let mut consumed = 0;
            apply_updates_to_paragraph(&mut paragraph, &updates, &mut consumed)?;
            if consumed != updates.len() {
                return Err(Error::Other(
                    "text-box field traversal lost source identity".to_owned(),
                ));
            }
            let replacements = paragraph_field_source_replacements(&original, &paragraph)?;
            if replacements.is_empty() {
                return Err(Error::Other(
                    "text-box cache edit has no producer source span".to_owned(),
                ));
            }
            result.push(TextBoxCachePatch {
                location,
                replacements,
                updated: count,
            });
        }
        Ok(result)
    }
}
impl Document {
    pub(super) fn update_fields_with_policy(
        &mut self,
        context: &FieldEvaluationContext,
        missing_merge_fields_as_empty: bool,
    ) -> Result<usize> {
        prepare_physical_story_projection(&mut self.document.body, &mut [])?;
        let (evaluations, visible) =
            self.evaluate_fields_with_policy(context, missing_merge_fields_as_empty)?;
        let original = self.clone_for_staging();
        let mut fields = Vec::new();
        let mut body = Vec::new();
        collect_body_paragraphs(&original.document.body, &mut body, None);
        for paragraph in body {
            append_ref_copy_paragraph(paragraph, false, &mut fields);
        }
        for (_, kind, xml) in field_story_parts(&original, true) {
            for record in legacy_story_paragraphs(&xml, kind)? {
                append_ref_copy_paragraph(&record.paragraph, true, &mut fields);
            }
        }
        for source in original.text_box_cache_paragraphs()? {
            append_ref_copy_paragraph(
                &parsed_physical_field_paragraph(&source.xml)?,
                true,
                &mut fields,
            );
        }
        if fields.len() != evaluations.len() {
            return Err(Error::Other(
                "REF copy inventory disagrees with physical field evaluations".into(),
            ));
        }
        let mut updates = evaluations
            .iter()
            .map(|evaluation| match &evaluation.outcome {
                FieldOutcome::Resolved(value) => CachedFieldUpdate {
                    typed_runs: None,
                    comment_ranges: Vec::new(),
                    cached_result: value.clone(),
                    dirty: false,
                },
                FieldOutcome::DeferredPagination
                | FieldOutcome::TableOfContents(_)
                | FieldOutcome::TableOfContentsEntry(_)
                | FieldOutcome::MailMergeControl(_)
                | FieldOutcome::Barcode(_)
                | FieldOutcome::KeepStored { .. } => CachedFieldUpdate {
                    typed_runs: None,
                    comment_ranges: Vec::new(),
                    cached_result: evaluation.cached_result.clone(),
                    dirty: true,
                },
            })
            .map(Some)
            .collect::<Vec<_>>();
        for ((source, update), visible) in fields.iter().zip(&mut updates).zip(visible) {
            if source.locked || !visible {
                *update = None;
            }
        }
        let source_projection = if fields.iter().any(|source| {
            let instruction = source.field.effective_instruction();
            instruction.name == "REF"
                && has_switch(&instruction, "f")
                && !source.locked
                && !source.generated
        }) {
            let sequence_updates = fields
                .iter()
                .zip(&evaluations)
                .map(|(source, evaluation)| {
                    if source.locked
                        || source.generated
                        || source.field.effective_instruction().name != "SEQ"
                    {
                        return None;
                    }
                    if let FieldOutcome::Resolved(value) = &evaluation.outcome {
                        Some(CachedFieldUpdate {
                            cached_result: value.clone(),
                            dirty: false,
                            typed_runs: None,
                            comment_ranges: Vec::new(),
                        })
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            if sequence_updates.iter().any(Option::is_some) {
                let mut projection = original.clone_for_staging();
                projection.apply_cached_physical_field_updates(&sequence_updates)?;
                let canonical = projection.main_part_with_body_spans()?.0;
                projection.document =
                    crate::document::DocumentModel::unproved(CT_Document::from_xml(&canonical)?);
                Some(projection)
            } else {
                None
            }
        } else {
            None
        };
        let ref_source = source_projection.as_ref().unwrap_or(&original);
        let mut note_copies = Vec::new();
        let mut comment_copies = Vec::new();
        let mut occupied_comments = original
            .comments
            .as_ref()
            .map(|comments| {
                comments
                    .comments
                    .iter()
                    .map(|comment| comment.id)
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();
        let mut occupied_comment_identities = None;
        let mut occupied_notes = HashMap::new();
        let note_reference_counts = if fields.iter().any(|source| {
            let instruction = source.field.effective_instruction();
            !source.suppress_copied_references
                && !source.locked
                && !source.generated
                && instruction.name == "REF"
                && has_switch(&instruction, "f")
        }) {
            ref_note_reference_counts(&original.package)?
        } else {
            HashMap::new()
        };
        for (source, update) in fields.iter().zip(&mut updates) {
            let instruction = source.field.effective_instruction();
            if instruction.name != "REF" || !has_switch(&instruction, "f") {
                continue;
            }
            if source.locked || source.generated {
                *update = None;
                continue;
            }
            if update.as_ref().is_some_and(|update| update.dirty) {
                *update = None;
                continue;
            }
            let Some(update) = update else {
                continue;
            };
            let target = text_argument(&instruction, 0)
                .ok_or_else(|| Error::Other("REF f target is absent".into()))?;
            let Some(cache) = ref_bookmark_runs(ref_source, target)? else {
                continue;
            };
            let comment_replacements = if source.suppress_copied_references {
                Vec::new()
            } else {
                ref_comment_replacement_ids(&original, &source.field, &cache)?
            };
            let mut comment_cursor = 0;
            let mut runs = cache.runs;
            let mut comment_ranges = if source.suppress_copied_references {
                Vec::new()
            } else {
                cache.comment_ranges
            };
            let mut old_references = Vec::new();
            collect_ref_cached_note_references(&source.field, &mut old_references)?;
            for run in &mut runs {
                if source.suppress_copied_references {
                    run.content.retain(|content| {
                        !matches!(
                            content,
                            RunContent::FootnoteRef { .. }
                                | RunContent::EndnoteRef { .. }
                                | RunContent::CommentReference { .. }
                        )
                    });
                } else {
                    for content in &mut run.content {
                        if let RunContent::CommentReference { id, .. } = content {
                            if occupied_comment_identities.is_none() {
                                occupied_comment_identities =
                                    Some(ref_comment_occupied_identity_values(&original.package)?);
                            }
                            let (new_id, copy) = prepare_ref_comment_copy(
                                self,
                                ref_source,
                                *id,
                                comment_replacements[comment_cursor],
                                &mut occupied_comments,
                                occupied_comment_identities.as_mut().unwrap(),
                            )?;
                            comment_cursor += 1;
                            for marker in &mut comment_ranges {
                                let marker_id = match marker {
                                    CommentRangeMarker::Start { id, .. }
                                    | CommentRangeMarker::End { id, .. } => id,
                                };
                                if *marker_id == *id {
                                    *marker_id = new_id;
                                }
                            }
                            *id = new_id;
                            comment_copies.push(copy);
                            continue;
                        }
                        let kind = match content {
                            RunContent::FootnoteRef { .. } => StoryKind::Footnote,
                            RunContent::EndnoteRef { .. } => StoryKind::Endnote,
                            _ => continue,
                        };
                        let id = match content {
                            RunContent::FootnoteRef { id, .. }
                            | RunContent::EndnoteRef { id, .. } => id,
                            _ => unreachable!("note kind"),
                        };
                        let replacement = old_references
                            .iter()
                            .position(|(family, _)| *family == kind)
                            .map(|index| old_references.remove(index).1)
                            .filter(|old| {
                                *old != *id && note_reference_counts.get(&(kind, *old)) == Some(&1)
                            });
                        let (new_id, copy) = prepare_ref_note_copy(
                            self,
                            ref_source,
                            kind,
                            *id,
                            replacement,
                            &mut occupied_notes,
                        )?;
                        *id = new_id;
                        note_copies.push(copy);
                    }
                }
            }
            let mut candidate = source.field.clone();
            candidate.set_cached_runs_with_comment_ranges(runs.clone(), comment_ranges.clone())?;
            update.cached_result = candidate.cached_result;
            update.typed_runs = Some(runs);
            update.comment_ranges = comment_ranges;
        }
        let updated = self.apply_cached_physical_field_updates(&updates)?;
        for copy in note_copies {
            self.publish_fragment_note_staged(
                copy.kind,
                &copy.destination_part,
                copy.xml,
                copy.replace_id,
            )?;
        }
        for copy in comment_copies {
            publish_ref_comment_copy(self, copy)?;
        }
        Ok(updated)
    }

    fn apply_cached_physical_field_updates(
        &mut self,
        updates: &[Option<CachedFieldUpdate>],
    ) -> Result<usize> {
        let boxes = self.text_box_cache_paragraphs()?;
        let mut paragraphs = Vec::new();
        let mut box_count = 0;
        for source in boxes {
            let paragraph = parsed_physical_field_paragraph(&source.xml)?;
            let mut fields = Vec::new();
            for run in accepted_toc_runs(&paragraph) {
                for content in &run.run.content {
                    if let RunContent::Field(field) = content {
                        collect_preorder_fields(field, false, &mut fields);
                    }
                }
            }
            if fields.is_empty() {
                continue;
            }
            let count = fields.len();
            box_count += count;
            paragraphs.push((source.location, paragraph, count));
        }
        let split = updates.len().checked_sub(box_count).ok_or_else(|| {
            Error::Other("text-box update inventory exceeds field evaluations".into())
        })?;
        let mut cursor = split;
        let mut patches = Vec::new();
        for (location, mut paragraph, count) in paragraphs {
            let end = cursor + count;
            if !updates[cursor..end].iter().any(Option::is_some) {
                cursor = end;
                continue;
            }
            let original = paragraph.clone();
            let mut consumed = 0;
            apply_updates_to_paragraph(&mut paragraph, &updates[cursor..end], &mut consumed)?;
            if consumed != count {
                return Err(Error::Other(
                    "text-box update field inventory is inconsistent".into(),
                ));
            }
            let replacements = paragraph_field_source_replacements(&original, &paragraph)?;
            if replacements.is_empty() {
                return Err(Error::Other(
                    "text-box update has no preserved field source".into(),
                ));
            }
            patches.push(TextBoxCachePatch {
                location,
                replacements,
                updated: updates[cursor..end].iter().flatten().count(),
            });
            cursor = end;
        }
        if cursor != updates.len() {
            return Err(Error::Other(
                "text-box update left unconsumed evaluations".into(),
            ));
        }
        let mut updated = self.apply_cached_field_updates(&updates[..split], true)?;
        for patch in patches {
            self.patch_story_paragraph_field_sources(&patch.location, &patch.replacements)?;
            updated += patch.updated;
        }
        Ok(updated)
    }

    fn apply_cached_field_updates(
        &mut self,
        updates: &[Option<CachedFieldUpdate>],
        include_comments: bool,
    ) -> Result<usize> {
        if updates
            .iter()
            .flatten()
            .any(|update| !update.cached_result.chars().all(valid_xml_character))
        {
            return Err(Error::Other(
                "field result contains a character forbidden by XML 1.0".to_owned(),
            ));
        }
        let updated = updates.iter().flatten().count();
        if updated == 0 {
            return Ok(0);
        }
        let package_before = self.package.clone();
        let mut update_index = 0;
        let main_source = self.updated_main_field_source(updates, &mut update_index)?;
        let mut staged = Vec::new();
        for (part_name, kind, xml) in field_story_parts(self, include_comments) {
            let mut paragraphs = legacy_story_paragraphs(&xml, kind)?;
            let first = update_index;
            for entry in &mut paragraphs {
                apply_updates_to_paragraph(&mut entry.paragraph, updates, &mut update_index)?;
            }
            if updates[first..update_index].iter().any(Option::is_some) {
                let xml = patch_legacy_story_field_sources(&xml, &paragraphs, true)?;
                match kind {
                    PackageStoryKind::Header | PackageStoryKind::Footer => {
                        CT_HdrFtr::from_xml(&xml)?;
                    }
                    PackageStoryKind::Footnotes | PackageStoryKind::Endnotes => {
                        CT_Footnotes::from_xml(&xml)?;
                    }
                    PackageStoryKind::Comments => {
                        CT_Comments::from_xml(&xml)?;
                    }
                    PackageStoryKind::Document => {
                        CT_Document::from_xml(&xml)?;
                    }
                }
                staged.push((part_name, kind, xml));
            }
        }
        if update_index != updates.len() {
            return Err(Error::Other(format!(
                "page field traversal consumed {update_index} of {} staged evaluations",
                updates.len()
            )));
        }
        if let Some(xml) = main_source {
            let part = self.doc_part_name.clone();
            crate::document::set_story_source_xml(self, &part, xml)?;
        }
        for (part_name, kind, xml) in staged {
            if matches!(kind, PackageStoryKind::Footnotes) {
                self.footnotes = CT_Footnotes::from_xml(&xml)?;
                self.footnotes_dirty = false;
            }
            if matches!(kind, PackageStoryKind::Comments) {
                self.comments = Some(CT_Comments::from_xml(&xml)?);
                self.comments_dirty = false;
            }
            self.package.set_part(&part_name, xml);
        }
        self.package_signatures_invalidated |= self
            .retained_package_signature_would_be_invalidated()?
            || crate::embedded::synchronized_package_mutation_invalidates_signature(
                &package_before,
                &self.package,
            );
        self.invalidate_layout();
        Ok(updated)
    }

    // Map the one immutable current writer into the guarded restored source.
    // Paths select owners, same-context emission proves each occurrence, and
    // only those independently parsed source owners receive cache updates.
    fn updated_main_field_source(
        &self,
        updates: &[Option<CachedFieldUpdate>],
        update_index: &mut usize,
    ) -> Result<Option<Vec<u8>>> {
        self.validate_model_resource_limits()?;
        let (canonical, _, roots) = self.main_part_with_body_spans()?;
        let source = self.namespace_restored_main_source()?;
        let (boundaries, _) =
            crate::document::comment_namespace_source_boundaries(&canonical, &source)?.ok_or_else(
                || Error::Other("field-cache restored source changed physical events".into()),
            )?;
        let mut inverse = BTreeMap::new();
        for (before, after) in boundaries {
            if inverse
                .insert(after, before)
                .is_some_and(|prior| prior != before)
            {
                return Err(Error::Other(
                    "field-cache source has ambiguous event boundaries".into(),
                ));
            }
        }
        if roots.len() != self.document.body.content.len() {
            return Err(Error::Other(
                "field-cache direct body inventory changed".into(),
            ));
        }
        for (item, range) in self.document.body.content.iter().zip(&roots) {
            let emitted = self.document.body_item_xml(item)?;
            if canonical.get(range.clone()) != Some(emitted.as_slice()) {
                return Err(Error::Other(
                    "field-cache direct body emitter differs from current source".into(),
                ));
            }
        }
        let root_indices = self
            .document
            .body
            .content
            .iter()
            .enumerate()
            .filter_map(|(index, item)| (!matches!(item, BodyContent::RawXml(_))).then_some(index))
            .collect::<Vec<_>>();
        let mut paragraphs = Vec::new();
        let mut paths = Vec::new();
        collect_body_paragraphs(&self.document.body, &mut paragraphs, Some(&mut paths));
        let parsed = legacy_story_paragraphs(&source, PackageStoryKind::Document)?;
        if parsed.len() != paragraphs.len() || paths.len() != paragraphs.len() {
            return Err(Error::Other(
                "field-cache modeled/source paragraph inventory differs".into(),
            ));
        }
        let mut source_paragraphs = BTreeMap::new();
        for paragraph in parsed {
            if source_paragraphs
                .insert(paragraph.path.clone(), paragraph)
                .is_some()
            {
                return Err(Error::Other(
                    "field-cache duplicate physical paragraph path".into(),
                ));
            }
        }
        let canonical_starts = source_paragraphs
            .values()
            .map(|paragraph| {
                inverse.get(&paragraph.start).copied().ok_or_else(|| {
                    Error::Other("field-cache paragraph start lacks a source boundary".into())
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let (ancestor_frames, ancestor_paths) =
            crate::document::story_namespace_ancestors_at(&canonical, &canonical_starts)?;
        let mut edits = Vec::<FieldSourceEdit>::new();
        for (paragraph, path) in paragraphs.into_iter().zip(paths) {
            let mut physical = source_paragraphs.remove(&path).ok_or_else(|| {
                Error::Other("field-cache paragraph has no matching container path".into())
            })?;
            let first = *inverse.get(&physical.start).ok_or_else(|| {
                Error::Other("field-cache paragraph start lacks a source boundary".into())
            })?;
            let last = *inverse.get(&physical.end).ok_or_else(|| {
                Error::Other("field-cache paragraph end lacks a source boundary".into())
            })?;
            let root = path
                .first()
                .and_then(|(_, index)| root_indices.get(*index))
                .and_then(|index| roots.get(*index))
                .ok_or_else(|| {
                    Error::Other("field-cache paragraph lacks a direct body certificate".into())
                })?;
            if first > last || first < root.start || last > root.end {
                return Err(Error::Other(
                    "field-cache paragraph escapes its certified body owner".into(),
                ));
            }
            paragraph.validate_resource_limits()?;
            let ancestor_indices = ancestor_paths.get(&first).ok_or_else(|| {
                Error::Other("field-cache paragraph lacks writer-time ancestors".into())
            })?;
            let ancestors = ancestor_indices
                .iter()
                .map(|index| ancestor_frames[*index].as_slice())
                .collect::<Vec<_>>();
            let emitted = self.document.body_item_xml_with_ancestor_declarations(
                &BodyContent::Paragraph(Box::new(paragraph.clone())),
                &ancestors,
            )?;
            if canonical.get(first..last) != Some(emitted.as_slice()) {
                return Err(Error::Other(
                    "field-cache paragraph emitter differs at its proved physical path".into(),
                ));
            }
            if !same_field_inventory(paragraph, &physical.original) {
                return Err(Error::Other(
                    "field-cache evaluation and physical field inventories differ".into(),
                ));
            }
            let before = *update_index;
            apply_updates_to_paragraph(&mut physical.paragraph, updates, update_index)?;
            if !updates
                .get(before..*update_index)
                .ok_or_else(|| Error::Other("field-cache update cursor escaped inventory".into()))?
                .iter()
                .any(Option::is_some)
            {
                continue;
            }
            let pairs =
                paragraph_field_source_replacements(&physical.original, &physical.paragraph)?;
            let mut cursor = 0;
            for (original, replacement) in pairs {
                if original.is_empty() {
                    return Err(Error::Other("empty field source identity".into()));
                }
                let relative = find_typed_field_source(
                    &source,
                    physical.start,
                    physical.end,
                    &original,
                    cursor,
                )?
                .ok_or_else(|| {
                    Error::Other("field cache source left its proved paragraph owner".into())
                })?;
                cursor = relative
                    .checked_add(original.len())
                    .ok_or_else(|| Error::Other("field-cache source bounds overflow".into()))?;
                if original == replacement {
                    continue;
                }
                let start = physical
                    .start
                    .checked_add(relative)
                    .ok_or_else(|| Error::Other("field-cache source bounds overflow".into()))?;
                let end = start
                    .checked_add(original.len())
                    .filter(|end| *end <= physical.end)
                    .ok_or_else(|| {
                        Error::Other("field-cache source escaped paragraph bounds".into())
                    })?;
                edits.push(FieldSourceEdit {
                    start,
                    end,
                    replacement,
                });
            }
        }
        if !source_paragraphs.is_empty() {
            return Err(Error::Other(
                "field-cache physical paragraph inventory not consumed".into(),
            ));
        }
        if edits.is_empty() {
            return Ok(None);
        }
        edits.sort_by_key(|edit| edit.start);
        let mut cursor = 0;
        let mut length = source.len();
        for edit in &edits {
            if edit.start < cursor || edit.start > edit.end || edit.end > source.len() {
                return Err(Error::Other(
                    "field-cache source edits overlap or escape source".into(),
                ));
            }
            length = length
                .checked_sub(edit.end - edit.start)
                .and_then(|length| length.checked_add(edit.replacement.len()))
                .ok_or_else(|| Error::Other("field-cache output length overflow".into()))?;
            cursor = edit.end;
        }
        let mut output = Vec::with_capacity(length);
        cursor = 0;
        for edit in edits {
            output.extend_from_slice(&source[cursor..edit.start]);
            output.extend_from_slice(&edit.replacement);
            cursor = edit.end;
        }
        output.extend_from_slice(&source[cursor..]);
        debug_assert_eq!(output.len(), length);
        Ok(Some(output))
    }

    /// Update typed field caches, then save the package to a file path.
    pub fn save_with_field_updates<P: AsRef<Path>>(
        &mut self,
        path: P,
        context: &FieldEvaluationContext,
    ) -> Result<()> {
        self.update_fields(context)?;
        self.save(path)
    }

    /// Update typed field caches, then save the package to bytes.
    pub fn to_bytes_with_field_updates(
        &mut self,
        context: &FieldEvaluationContext,
    ) -> Result<Vec<u8>> {
        self.update_fields(context)?;
        self.to_bytes()
    }
}

pub(super) fn referenced_header_footer_parts(
    document: &Document,
    is_header: bool,
    include_control_sections: bool,
) -> Vec<(String, Vec<u8>)> {
    let Some(relationships) = document.package.get_part_rels(&document.doc_part_name) else {
        return Vec::new();
    };
    let relationship_type = if is_header {
        rel_types::HEADER
    } else {
        rel_types::FOOTER
    };
    // Pagination inventories modeled block-control section endings. General
    // field evaluation retains its existing direct-body story discovery.
    let sections = if include_control_sections {
        oxoffice_docx_layout::engine::document_sections(&document.document)
    } else {
        document
            .document
            .body
            .content
            .iter()
            .filter_map(|content| match content {
                BodyContent::Paragraph(paragraph) => paragraph
                    .properties
                    .as_ref()
                    .and_then(|properties| properties.sect_pr.as_ref()),
                _ => None,
            })
            .chain(document.document.body.sect_pr.as_ref())
            .collect()
    };
    let mut seen = HashSet::new();
    let mut parts = Vec::new();
    for section in sections {
        let references = section_header_footer_references(section, is_header);
        for reference in references {
            let Some(relationship) = relationships.get_by_id(&reference.rel_id) else {
                continue;
            };
            if relationship.rel_type != relationship_type
                || !crate::document::relationship_is_internal(relationship)
            {
                continue;
            }
            let part_name =
                OpcPackage::resolve_rel_target(&document.doc_part_name, &relationship.target);
            if seen.insert(part_name.clone())
                && let Some(xml) = document.package.get_part(&part_name)
            {
                parts.push((part_name, xml.to_vec()));
            }
        }
    }
    parts
}

fn section_header_footer_references(
    section: &CT_SectPr,
    is_header: bool,
) -> &[oxoffice_docx_oxml::header_footer::HdrFtrRef] {
    if is_header {
        &section.header_refs
    } else {
        &section.footer_refs
    }
}

pub(super) fn relationship_parts(
    document: &Document,
    relationship_type: &str,
) -> Vec<(String, Vec<u8>)> {
    let Some(relationships) = document.package.get_part_rels(&document.doc_part_name) else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    relationships
        .items
        .iter()
        .filter(|relationship| relationship.rel_type == relationship_type)
        .filter(|relationship| crate::document::relationship_is_internal(relationship))
        .filter_map(|relationship| {
            let part_name =
                OpcPackage::resolve_rel_target(&document.doc_part_name, &relationship.target);
            if !seen.insert(part_name.clone()) {
                return None;
            }
            Some((
                part_name.clone(),
                document.package.get_part(&part_name)?.to_vec(),
            ))
        })
        .collect()
}

/// All immutable field occurrences keyed by paragraph path and preorder index.
type PlacedPageFields = HashMap<
    (oxoffice_docx_layout::WordSourcePath, u32),
    Vec<oxoffice_docx_layout::WordFieldPlacement>,
>;

fn placed_page_fields(layout: &oxoffice_docx_layout::WordLayoutResult) -> PlacedPageFields {
    let mut placed = PlacedPageFields::new();
    for &placement in layout.field_placements() {
        if let Some(path) = layout.source_node(placement.source.node) {
            placed
                .entry((path.clone(), placement.source.index))
                .or_default()
                .push(placement);
        }
    }
    placed
}

pub(super) fn collect_preorder_fields<'a>(
    field: &'a Field,
    locked: bool,
    fields: &mut Vec<(&'a Field, bool)>,
) {
    let locked = locked || field.locked() == Some(true);
    fields.push((field, locked));
    for nested in field.all_nested_fields_in_source_order() {
        collect_preorder_fields(nested, locked, fields);
    }
}

#[allow(clippy::too_many_arguments)]
fn push_page_field_updates(
    paragraphs: &[&CT_P],
    paths: &[Vec<oxoffice_docx_layout::WordSourcePath>],
    placed: &PlacedPageFields,
    layout: &oxoffice_docx_layout::WordLayoutResult,
    section_formats: &[Option<String>],
    updates: &mut Vec<Option<CachedFieldUpdate>>,
    report: &mut LayoutBackedFieldUpdateReport,
) {
    for (paragraph, paths) in paragraphs.iter().zip(paths) {
        let mut physical_fields = Vec::new();
        for run in paragraph.source_runs() {
            for content in &run.content {
                if let RunContent::Field(field) = content {
                    collect_preorder_fields(field, false, &mut physical_fields);
                }
            }
        }
        let physical_indices = physical_fields
            .iter()
            .enumerate()
            .map(|(index, (field, _))| (std::ptr::from_ref(*field), index as u32))
            .collect::<HashMap<_, _>>();
        let mut fields = Vec::new();
        for field in accepted_toc_runs(paragraph)
            .into_iter()
            .flat_map(|run| &run.run.content)
            .filter_map(|content| match content {
                RunContent::Field(field) => Some(field),
                _ => None,
            })
        {
            collect_preorder_fields(field, false, &mut fields);
        }
        for (field, locked) in fields {
            let Some(&index) = physical_indices.get(&std::ptr::from_ref(field)) else {
                report.diagnostics.push("field cache retained because its accepted owner has no physical source identity".into());
                updates.push(None);
                continue;
            };
            let instruction = field.effective_instruction();
            let name = instruction.name.as_str();
            if !matches!(
                name,
                "PAGE" | "NUMPAGES" | "SECTION" | "SECTIONPAGES" | "PAGEREF"
            ) {
                updates.push(None);
                continue;
            }
            let retained = |report: &mut LayoutBackedFieldUpdateReport, reason: &str| {
                report
                    .diagnostics
                    .push(format!("{name} field {index} retained cache: {reason}"))
            };
            if locked {
                retained(report, "locked owner");
                updates.push(None);
                continue;
            }
            let placement = paths
                .iter()
                .filter_map(|path| placed.get(&(path.clone(), index)))
                .flatten()
                .min_by_key(|placement| placement.physical_page);
            if placement.is_none() && name != "PAGEREF" {
                retained(report, "source did not reach pagination");
                updates.push(None);
                continue;
            }
            if matches!(name, "PAGE" | "NUMPAGES")
                && paths.iter().any(|path| {
                    matches!(
                        path.story,
                        oxoffice_docx_layout::WordStory::Header { .. }
                            | oxoffice_docx_layout::WordStory::Footer { .. }
                    )
                })
            {
                retained(
                    report,
                    "Word preserves dynamic header and footer caches on update and save",
                );
                updates.push(None);
                continue;
            }
            if let Some(switch) = unsupported_switch(&instruction) {
                retained(report, &format!("unsupported switch \\{switch}"));
                updates.push(None);
                continue;
            }
            if let Err(reason) = validate_instruction_shape(&instruction) {
                retained(report, &reason);
                updates.push(None);
                continue;
            }
            if name == "PAGEREF"
                && oxoffice_docx_layout::engine::page_reference_keeps_stored_display(
                    &field.cached_result,
                )
            {
                retained(report, "the result holds more than a page number");
                updates.push(None);
                continue;
            }
            let value = match name {
                "PAGE" => placement.map(|placement| placement.displayed_page),
                "NUMPAGES" => Some(layout.layout.pages.len()),
                "SECTION" => placement.map(|placement| placement.section_index + 1),
                "SECTIONPAGES" => placement.map(|placement| {
                    layout
                        .page_sections()
                        .iter()
                        .filter(|record| record.section_index == placement.section_index)
                        .map(|record| record.physical_page)
                        .collect::<HashSet<_>>()
                        .len()
                }),
                "PAGEREF" => {
                    text_argument(&instruction, 0).and_then(|name| layout.bookmark_page(name))
                }
                _ => None,
            };
            let Some(value) = value else {
                retained(report, "bookmark target is missing, ambiguous or unplaced");
                updates.push(None);
                continue;
            };
            let cached_result = oxoffice_docx_layout::engine::format_page_field_result(
                &instruction,
                value,
                if name == "PAGEREF" {
                    text_argument(&instruction, 0)
                        .and_then(|target| layout.bookmark_page_section(target))
                        .and_then(|target| {
                            section_formats
                                .get(target.section_index)
                                .and_then(Option::as_deref)
                        })
                } else if name == "PAGE" {
                    placement.and_then(|placement| {
                        section_formats
                            .get(placement.section_index)
                            .and_then(Option::as_deref)
                    })
                } else {
                    Some("decimal")
                },
            );
            match cached_result {
                Ok(cached_result) => {
                    match name {
                        "PAGE" => report.page_fields += 1,
                        "NUMPAGES" => report.num_pages_fields += 1,
                        "SECTION" => report.section_fields += 1,
                        "SECTIONPAGES" => report.section_pages_fields += 1,
                        "PAGEREF" => report.page_reference_fields += 1,
                        _ => {}
                    }
                    updates.push(Some(CachedFieldUpdate {
                        typed_runs: None,
                        comment_ranges: Vec::new(),
                        cached_result,
                        dirty: false,
                    }));
                }
                Err(reason) => {
                    retained(report, &reason);
                    updates.push(None);
                }
            }
        }
    }
}

pub(super) fn section_page_formats(document: &CT_Document) -> Vec<Option<String>> {
    let mut formats = oxoffice_docx_layout::engine::document_sections(document)
        .into_iter()
        .map(oxoffice_docx_layout::engine::section_page_number_format)
        .collect::<Vec<_>>();
    if document.body.sect_pr.is_none() {
        formats.push(Some("decimal".into()));
    }
    formats
}

pub(super) fn apply_updates_to_paragraph(
    paragraph: &mut CT_P,
    updates: &[Option<CachedFieldUpdate>],
    update_index: &mut usize,
) -> Result<()> {
    for path in paragraph.accepted_run_paths() {
        let Some(source) = paragraph.accepted_run(&path) else {
            return Err(Error::Other(
                "accepted field update run path is stale".into(),
            ));
        };
        if !source
            .content
            .iter()
            .any(|content| matches!(content, RunContent::Field(_)))
        {
            continue;
        }
        let mut replacement = source.clone();
        apply_updates_to_run(&mut replacement, updates, update_index)?;
        if !paragraph.replace_accepted_run(&path, replacement)? {
            return Err(Error::Other(
                "accepted field update owner disappeared".into(),
            ));
        }
    }
    Ok(())
}

fn apply_updates_to_run(
    run: &mut oxoffice_docx_oxml::text::CT_R,
    updates: &[Option<CachedFieldUpdate>],
    update_index: &mut usize,
) -> Result<()> {
    for content in &mut run.content {
        if let RunContent::Field(field) = content {
            apply_updates_to_field(field, updates, update_index)?;
        }
    }
    Ok(())
}

fn apply_updates_to_field(
    field: &mut Field,
    updates: &[Option<CachedFieldUpdate>],
    update_index: &mut usize,
) -> Result<()> {
    let Some(update) = updates.get(*update_index) else {
        return Ok(());
    };
    if let Some(update) = update {
        field.cached_result.clone_from(&update.cached_result);
        field.dirty = Some(update.dirty);
    }
    *update_index += 1;

    let nested_pointers = field
        .nested_fields_in_source_order()
        .into_iter()
        .map(|nested| std::ptr::from_ref(nested) as usize)
        .collect::<Vec<_>>();
    for pointer in nested_pointers {
        if let Some(nested) = nested_field_mut(field, pointer) {
            apply_updates_to_field(nested, updates, update_index)?;
        }
    }
    for index in 0..field.cached_fields_in_source_order().len() {
        if let Some(nested) = field.cached_field_mut(index) {
            apply_updates_to_field(nested, updates, update_index)?;
        }
    }
    if let Some(runs) = update
        .as_ref()
        .and_then(|update| update.typed_runs.as_ref())
    {
        field.set_cached_runs_with_comment_ranges(
            runs.clone(),
            update
                .as_ref()
                .expect("typed cache update")
                .comment_ranges
                .clone(),
        )?;
    }
    if update.is_none() && !field.cached_fields_in_source_order().is_empty() {
        field.refresh_cached_field_projection();
    }
    field.normalize_cached_result_owner()?;
    Ok(())
}

fn nested_field_mut(field: &mut Field, pointer: usize) -> Option<&mut Field> {
    field
        .instruction
        .arguments
        .iter_mut()
        .chain(
            field
                .instruction
                .switches
                .iter_mut()
                .filter_map(|field_switch| field_switch.argument.as_mut()),
        )
        .flat_map(FieldArgument::nested_fields_mut)
        .find(|nested| std::ptr::from_ref::<Field>(nested) as usize == pointer)
}
