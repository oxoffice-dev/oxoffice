//! Evaluating fields: the evaluator and what it reads from the document.

use super::*;
use oxoffice_docx_oxml::text::{bookmarks_depending_on, field_ref_targets};

pub(super) struct BookmarkValue {
    text: String,
    range: Option<crate::RunRange>,
    /// The bookmark's runs as a reader sees them, one row per paragraph,
    /// when the value is read from them.
    runs: Option<Vec<Vec<CT_R>>>,
}

/// The text a reader sees in `runs`, without the display of a field that
/// depends on a bookmark `excluded` refuses.
fn runs_text_without(runs: &[Vec<CT_R>], excluded: &dyn Fn(&str) -> bool) -> String {
    runs.iter()
        .map(|paragraph| {
            paragraph
                .iter()
                .map(|run| {
                    run.accepted_text_keeping(&mut |field| {
                        let mut targets = BTreeSet::new();
                        field_ref_targets(field, &mut targets);
                        !targets.iter().any(|target| excluded(target))
                    })
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Give each bookmark read from its runs the text a reader sees there, less
/// the display of every field whose result depends on the bookmark itself: a
/// REF to it, or to a bookmark whose value depends on it in turn. Word
/// refuses such a self-reference, and keeping its stored result would make
/// the value grow on every update. Every other field shows its result.
fn settle_bookmark_texts(bookmarks: &mut BTreeMap<String, BookmarkValue>) {
    let mut depends = HashMap::<String, BTreeSet<String>>::new();
    for (name, bookmark) in bookmarks.iter() {
        let Some(runs) = &bookmark.runs else {
            continue;
        };
        let targets = depends.entry(name.to_lowercase()).or_default();
        for run in runs.iter().flatten() {
            run.accepted_text_keeping(&mut |field| {
                field_ref_targets(field, targets);
                true
            });
        }
    }
    for (name, bookmark) in bookmarks.iter_mut() {
        let Some(runs) = &bookmark.runs else {
            continue;
        };
        let depending = bookmarks_depending_on(name, &mut |current| {
            depends.get(current).cloned().unwrap_or_default()
        });
        bookmark.text = runs_text_without(runs, &|target| depending.contains(target));
    }
}

/// The runs of a body bookmark's range, one row per paragraph.
fn bookmark_range_runs(displays: &[Vec<CT_R>], range: crate::RunRange) -> Result<Vec<Vec<CT_R>>> {
    let mut rows = Vec::new();
    for index in range.start.body_index..=range.end.body_index {
        let Some(runs) = displays.get(index) else {
            return Err(Error::Other("bookmark range owner is missing".into()));
        };
        let start = if index == range.start.body_index {
            range.start.run_index
        } else {
            0
        };
        let end = if index == range.end.body_index {
            range.end.run_index
        } else {
            runs.len()
        };
        let Some(slice) = runs.get(start..end) else {
            return Err(Error::Other("bookmark range is stale".into()));
        };
        rows.push(slice.to_vec());
    }
    Ok(rows)
}

/// The field of `current` in the place `effective` holds in `effective_slot`:
/// the same operand slot, and the same nested piece of a joined operand.
fn nested_in_same_slot<'f>(
    effective_slot: Option<&FieldArgument>,
    current_slot: Option<&'f FieldArgument>,
    effective: &Field,
) -> Option<&'f Field> {
    let piece = effective_slot?
        .nested_fields()
        .iter()
        .position(|child| std::ptr::eq(*child, effective))?;
    current_slot?.nested_fields().get(piece).copied()
}

fn field_needs_numbering_layout(field: &Field) -> bool {
    let instruction = field.effective_instruction();
    let own = unsupported_switch(&instruction).is_none()
        && validate_instruction_shape(&instruction).is_ok()
        && (instruction.name == "STYLEREF"
            || (instruction.name == "REF"
                && ["n", "r", "w"]
                    .iter()
                    .any(|name| has_switch(&instruction, name))));
    own || field
        .effective_nested_fields_in_source_order(&instruction)
        .into_iter()
        .any(field_needs_numbering_layout)
}

impl<'a> Evaluator<'a> {
    pub(super) fn new(document: &'a Document, context: &'a FieldEvaluationContext) -> Self {
        let mut bookmarks: BTreeMap<String, BookmarkValue> = document
            .bookmarks()
            .into_iter()
            .filter(|bookmark| bookmark.issue().is_none())
            .filter_map(|bookmark| {
                Some((
                    bookmark.name()?.to_owned(),
                    BookmarkValue {
                        text: bookmark.text().to_owned(),
                        range: bookmark.range(),
                        runs: None,
                    },
                ))
            })
            .collect();
        let mut main = Vec::new();
        collect_body_paragraphs(&document.document.body, &mut main, None);
        let displays = main
            .iter()
            .map(|paragraph| {
                paragraph
                    .accepted_bookmark_runs()
                    .into_iter()
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        for bookmark in bookmarks.values_mut() {
            bookmark.runs = bookmark
                .range
                .and_then(|range| bookmark_range_runs(&displays, range).ok());
        }
        // Paragraph inventory keeps the legacy body map and its name filter.
        // Pairing failure blocks only extra related admission.
        let mut cross_owner_bookmarks = HashSet::new();
        if let Ok((paired, name_counts)) = document.story_range_metadata() {
            if let Ok((ranges, across)) = paired {
                for kind in across {
                    if let crate::StoryRangeKind::Bookmark { name, .. } = kind
                        && !bookmarks.contains_key(&name)
                    {
                        cross_owner_bookmarks.insert(name);
                    }
                }
                for range in ranges {
                    let crate::StoryRangeKind::Bookmark { name, .. } = range.kind() else {
                        continue;
                    };
                    if bookmarks.contains_key(name)
                        || range.range().start.location.story().kind() == StoryKind::Body
                    {
                        continue;
                    }
                    if let Ok(Some(cache)) = ref_bookmark_runs(document, name) {
                        bookmarks.insert(
                            name.clone(),
                            BookmarkValue {
                                text: String::new(),
                                range: None,
                                runs: Some(cache.rows),
                            },
                        );
                    }
                }
            }
            bookmarks.retain(|name, _| name_counts.get(name).is_some_and(|count| *count == 1));
        }
        settle_bookmark_texts(&mut bookmarks);
        Self {
            document,
            context,
            bookmarks,
            cross_owner_bookmarks,
            numbering_layout: None,
            numbering_nodes: HashMap::new(),
            navigation_styles: HashMap::new(),
            navigation_style_pages: HashMap::new(),
            navigation_field_pages: HashMap::new(),
            results: Vec::new(),
            visible_results: Vec::new(),
            general_field_contexts: Vec::new(),
            general_context: true,
            mail_merge_stories: BTreeMap::new(),
            nested_outcomes: Vec::new(),
            missing_merge_fields_as_empty: false,
            current_field_source: None,
            sequence_snapshot: None,
            ref_note_counts: None,
            locked_fields: HashSet::new(),
            sequence_nodes: Vec::new(),
            field_sources: HashMap::new(),
        }
    }

    pub(super) fn for_mail_merge(
        document: &'a Document,
        context: &'a FieldEvaluationContext,
    ) -> Self {
        let mut evaluator = Self::new(document, context);
        evaluator.missing_merge_fields_as_empty = true;
        evaluator
    }

    pub(super) fn refresh_main_bookmark_sequence_text(
        &mut self,
        paragraphs: &[&CT_P],
    ) -> Result<()> {
        let Some(snapshot) = &self.sequence_snapshot else {
            return Ok(());
        };
        if self.sequence_nodes.len() != paragraphs.len() {
            return Err(Error::Other(
                "main bookmark sequence source inventory is incomplete".into(),
            ));
        }
        let mut displays = Vec::with_capacity(paragraphs.len());
        for (paragraph, node) in paragraphs.iter().zip(&self.sequence_nodes) {
            let mut fields = Vec::new();
            for run in accepted_toc_runs(paragraph) {
                for content in &run.run.content {
                    if let RunContent::Field(field) = content {
                        collect_preorder_fields(field, false, &mut fields);
                    }
                }
            }
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
            let updates = fields
                .iter()
                .map(|(field, _)| {
                    match node.and_then(|node| {
                        snapshot.field_value(oxoffice_layout::FieldSource {
                            node,
                            index: *physical_indices.get(&std::ptr::from_ref(*field))?,
                        })
                    }) {
                        Some(Ok(value)) => Some(CachedFieldUpdate {
                            typed_runs: None,
                            comment_ranges: Vec::new(),
                            cached_result: value.to_owned(),
                            dirty: false,
                        }),
                        _ => None,
                    }
                })
                .collect::<Vec<_>>();
            let mut projection = (*paragraph).clone();
            let mut consumed = 0;
            apply_updates_to_paragraph(&mut projection, &updates, &mut consumed)?;
            if consumed != updates.len() {
                return Err(Error::Other(
                    "bookmark sequence field traversal is inconsistent".into(),
                ));
            }
            displays.push(
                projection
                    .accepted_bookmark_runs()
                    .into_iter()
                    .map(|run| {
                        let mut displayed = run.clone();
                        for content in &mut displayed.content {
                            if let RunContent::Field(field) = content
                                && field.effective_instruction().name == "SEQ"
                            {
                                *content = RunContent::Text(
                                    oxoffice_docx_oxml::text::CT_Text::new(&field.cached_result),
                                );
                            }
                        }
                        displayed
                    })
                    .collect::<Vec<_>>(),
            );
        }
        for bookmark in self.bookmarks.values_mut() {
            let Some(range) = bookmark.range else {
                continue;
            };
            bookmark.runs = Some(bookmark_range_runs(&displays, range)?);
        }
        settle_bookmark_texts(&mut self.bookmarks);
        Ok(())
    }

    pub(super) fn evaluate_story(&mut self, story: &str, paragraphs: &[&CT_P]) -> Result<()> {
        if self.numbering_layout.is_none()
            && paragraphs.iter().any(|paragraph| {
                paragraph.runs().into_iter().any(|run| {
                    run.content.iter().any(|content| match content {
                        RunContent::Field(field) => field_needs_numbering_layout(field),
                        _ => false,
                    })
                })
            })
        {
            self.prepare_numbering_layout()?;
        }
        if self.sequence_snapshot.is_some() && self.sequence_nodes.len() != paragraphs.len() {
            return Err(Error::Other(format!(
                "source inventory identified {} of {} paragraphs in {story}",
                self.sequence_nodes.len(),
                paragraphs.len()
            )));
        }
        for (paragraph_index, paragraph) in paragraphs.iter().enumerate() {
            self.general_context = self
                .general_field_contexts
                .get(paragraph_index)
                .copied()
                .unwrap_or(true);
            self.register_paragraph_fields(
                paragraph,
                self.sequence_nodes.get(paragraph_index).copied().flatten(),
            );
            for run in accepted_toc_runs(paragraph) {
                for content in &run.run.content {
                    if let RunContent::Field(field) = content {
                        self.evaluate_field(field, story, paragraphs, paragraph_index, None);
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn register_paragraph_fields(
        &mut self,
        paragraph: &CT_P,
        node: Option<oxoffice_docx_layout::SourceNodeId>,
    ) {
        self.field_sources.clear();
        self.locked_fields.clear();
        let mut fields = Vec::new();
        for run in paragraph.source_runs() {
            for content in &run.content {
                if let RunContent::Field(field) = content {
                    collect_preorder_fields(field, false, &mut fields);
                }
            }
        }
        self.locked_fields.extend(
            fields
                .iter()
                .filter(|(_, locked)| *locked)
                .map(|(field, _)| std::ptr::from_ref(*field) as usize),
        );
        if let Some(node) = node {
            self.field_sources
                .extend(fields.into_iter().enumerate().map(|(index, (field, _))| {
                    (
                        std::ptr::from_ref(field) as usize,
                        oxoffice_layout::FieldSource {
                            node,
                            index: index as u32,
                        },
                    )
                }));
        }
    }

    fn prepare_numbering_layout(&mut self) -> Result<()> {
        let layout = self.document.layout_deterministic()?;
        self.bind_numbering_layout(layout);
        Ok(())
    }

    pub(super) fn bind_numbering_layout(
        &mut self,
        layout: Arc<oxoffice_docx_layout::WordLayoutResult>,
    ) {
        self.numbering_nodes.clear();
        for index in 1.. {
            let Some(node) = oxoffice_docx_layout::SourceNodeId::new(index) else {
                break;
            };
            let Some(path) = layout.source_node(node) else {
                break;
            };
            self.numbering_nodes.insert(path.clone(), node);
        }
        self.navigation_styles.clear();
        self.navigation_style_pages.clear();
        self.navigation_field_pages.clear();
        let mut aliases: HashMap<&str, Vec<String>> = HashMap::new();
        for style in &self.document.styles.styles {
            let names = aliases.entry(&style.style_id).or_default();
            names.push(style.style_id.to_lowercase());
            if let Some(name) = &style.name {
                names.push(name.to_lowercase());
            }
            names.sort();
            names.dedup();
        }
        for (node, path, _, _, style, _, page) in layout.paragraph_navigation() {
            if path.story != oxoffice_docx_layout::WordStory::Document {
                continue;
            }
            for alias in style
                .and_then(|style| aliases.get(style))
                .into_iter()
                .flatten()
            {
                self.navigation_styles
                    .entry(alias.clone())
                    .or_default()
                    .push(node);
                if let Some(page) = page {
                    self.navigation_style_pages
                        .entry(alias.clone())
                        .or_default()
                        .entry(page)
                        .or_default()
                        .push(node);
                }
            }
        }
        for nodes in self.navigation_styles.values_mut() {
            nodes.sort_unstable_by_key(|node| node.get());
            nodes.dedup();
        }
        for placement in layout.field_placements() {
            self.navigation_field_pages
                .entry(placement.source)
                .or_default()
                .insert(placement.physical_page);
        }
        self.numbering_layout = Some(layout);
    }

    pub(super) fn numbering_source(
        &self,
        node: oxoffice_docx_layout::SourceNodeId,
    ) -> Option<oxoffice_docx_layout::SourceNodeId> {
        let path = self.sequence_snapshot.as_ref()?.source_node(node)?;
        self.numbering_nodes.get(path).copied()
    }

    pub(super) fn ensure_numbering_layout_for_field(&mut self, field: &Field) -> Result<()> {
        if self.numbering_layout.is_none() && field_needs_numbering_layout(field) {
            self.prepare_numbering_layout()?;
        }
        Ok(())
    }

    fn record_field_preparation_failure(&mut self, field: &Field, message: &str) -> FieldOutcome {
        let outcome = keep(message);
        self.visible_results
            .push(self.general_context || matches!(field.instruction.name.as_str(), "SEQ" | "REF"));
        self.results.push(FieldEvaluation {
            field_index: self.results.len(),
            instruction: field.instruction.raw.clone(),
            cached_result: field.cached_result.clone(),
            outcome: outcome.clone(),
        });
        outcome
    }

    pub(super) fn evaluate_field(
        &mut self,
        field: &Field,
        story: &str,
        paragraphs: &[&CT_P],
        paragraph_index: usize,
        occurrence: Option<(Option<oxoffice_layout::FieldSource>, bool)>,
    ) -> FieldOutcome {
        let previous_source = self.current_field_source;
        let (source, locked) = occurrence.unwrap_or_else(|| {
            (
                self.field_sources
                    .get(&(std::ptr::from_ref(field) as usize))
                    .copied(),
                self.locked_fields
                    .contains(&(std::ptr::from_ref(field) as usize)),
            )
        });
        let prepared = if occurrence.is_some() {
            None
        } else {
            match prepared_current_field(field) {
                Ok(current) => Some(current),
                Err(error) => {
                    return self.record_field_preparation_failure(
                        field,
                        &format!("field preparation failed: {error}"),
                    );
                }
            }
        };
        let field = prepared.as_ref().unwrap_or(field);
        let instruction = field.effective_instruction();
        self.current_field_source = source;
        let result_index = self.results.len();
        let visible = self.general_context || matches!(instruction.name.as_str(), "SEQ" | "REF");
        self.visible_results.push(visible);
        self.results.push(FieldEvaluation {
            field_index: result_index,
            instruction: instruction.raw.clone(),
            cached_result: field.cached_result.clone(),
            outcome: keep("field evaluation did not complete"),
        });

        self.nested_outcomes.push(BTreeMap::new());
        let nested = self.evaluate_nested_fields(
            field,
            &instruction,
            story,
            paragraphs,
            paragraph_index,
            locked,
        );
        let mut outcome = if let Err(error) = nested {
            keep(&format!("field occurrence association failed: {error}"))
        } else if !visible {
            keep("ordinary field is outside its established evaluation context")
        } else if locked {
            keep("locked field retains its stored display")
        } else if instruction.name == "SEQ" && self.sequence_snapshot.is_some() {
            match self
                .current_field_source
                .and_then(|source| self.sequence_snapshot.as_ref()?.field_value(source))
            {
                Some(Ok(value)) => FieldOutcome::Resolved(value.to_owned()),
                Some(Err(message)) => keep(message),
                None => keep("SEQ lacks a unique physical accepted source"),
            }
        } else {
            self.evaluate_instruction(&instruction, story, paragraphs, paragraph_index)
        };
        if story == "main"
            && instruction.name == "REF"
            && has_switch(&instruction, "f")
            && matches!(outcome, FieldOutcome::Resolved(_))
        {
            let mut old_references = Vec::new();
            if let Err(error) = collect_ref_cached_note_references(field, &mut old_references) {
                outcome = keep(&error.to_string());
            } else if !old_references.is_empty() {
                let qualification = (|| -> Result<()> {
                    if self.ref_note_counts.is_none() {
                        self.ref_note_counts =
                            Some(ref_note_reference_counts(&self.document.package)?);
                    }
                    let target = text_argument(&instruction, 0)
                        .ok_or_else(|| Error::Other("REF f target is absent".into()))?;
                    let cache = ref_bookmark_runs(self.document, target)?
                        .ok_or_else(|| Error::Other("REF f target is missing".into()))?;
                    let new_references = cache
                        .runs
                        .iter()
                        .flat_map(|run| &run.content)
                        .filter_map(|content| match content {
                            RunContent::FootnoteRef { id, .. } => Some((StoryKind::Footnote, *id)),
                            RunContent::EndnoteRef { id, .. } => Some((StoryKind::Endnote, *id)),
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    if old_references.len() != new_references.len() {
                        return Err(Error::Other(
                            "cached note graph differs from the current source reference inventory"
                                .into(),
                        ));
                    }
                    for ((kind, id), (source_kind, source_id)) in
                        old_references.iter().zip(&new_references)
                    {
                        if kind != source_kind
                            || id == source_id
                            || self
                                .ref_note_counts
                                .as_ref()
                                .and_then(|counts| counts.get(&(*kind, *id)))
                                != Some(&1)
                        {
                            return Err(Error::Other("cached note reference ownership is ambiguous, shared or borrowed from its source".into()));
                        }
                        self.document.fragment_note_dependency(*kind, *id)?;
                    }
                    Ok(())
                })();
                if let Err(error) = qualification {
                    outcome = keep(&format!(
                        "REF f repeated note copy retains its cache: {error}"
                    ));
                }
            }
        }
        if story == "main"
            && instruction.name == "REF"
            && has_switch(&instruction, "f")
            && matches!(outcome, FieldOutcome::Resolved(_))
        {
            let qualification = text_argument(&instruction, 0)
                .ok_or_else(|| Error::Other("REF f target is absent".into()))
                .and_then(|target| ref_bookmark_runs(self.document, target))
                .and_then(|cache| {
                    cache.ok_or_else(|| Error::Other("REF f target is absent".into()))
                })
                .and_then(|cache| {
                    let replacements = ref_comment_replacement_ids(self.document, field, &cache)?;
                    let ids = ref_comment_reference_ids(&cache)?;
                    if !ids.is_empty() {
                        let mut staged = self.document.clone_for_staging();
                        let mut occupied = self
                            .document
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
                        let mut identities =
                            ref_comment_occupied_identity_values(&self.document.package)?;
                        for (id, replacement) in ids.into_iter().zip(replacements) {
                            prepare_ref_comment_copy(
                                &mut staged,
                                self.document,
                                id,
                                replacement,
                                &mut occupied,
                                &mut identities,
                            )?;
                        }
                    }
                    Ok(())
                });
            if let Err(error) = qualification {
                outcome = keep(&format!(
                    "REF f repeated annotation copy retains its cache: {error}"
                ));
            }
        }
        self.nested_outcomes.pop();
        self.results[result_index].outcome = outcome.clone();
        self.current_field_source = previous_source;
        outcome
    }

    fn evaluate_instruction(
        &mut self,
        instruction: &FieldInstruction,
        story: &str,
        paragraphs: &[&CT_P],
        paragraph_index: usize,
    ) -> FieldOutcome {
        if instruction.name.is_empty() {
            return keep("field instruction has no name");
        }
        if let Some(name) = unsupported_switch(instruction) {
            return keep(&format!(
                "field {} uses unsupported switch \\{name}",
                instruction.name
            ));
        }
        if let Err(diagnostic) = validate_instruction_shape(instruction) {
            return keep(&diagnostic);
        }

        let outcome = match instruction.name.as_str() {
            "PAGE" | "NUMPAGES" | "SECTION" | "SECTIONPAGES" => FieldOutcome::DeferredPagination,
            "PAGEREF" => self.evaluate_pageref(instruction),
            "REF" => self.evaluate_ref(instruction, story, paragraph_index),
            "IF" => self.evaluate_if(instruction, story, paragraphs, paragraph_index),
            "SEQ" => keep("SEQ requires accepted physical source context"),
            "DOCPROPERTY" => self.evaluate_docproperty(instruction),
            "DOCVARIABLE" => self.evaluate_docvariable(instruction),
            "STYLEREF" => self.evaluate_styleref(instruction, paragraphs, paragraph_index),
            "INCLUDETEXT" => self.evaluate_includetext(instruction),
            "DATE" | "TIME" => self.evaluate_date_time(instruction),
            "FILENAME" => self.evaluate_filename(instruction),
            "AUTHOR" => self.evaluate_author(),
            "MERGEFIELD" => self.evaluate_mergefield(instruction),
            "=" => self.evaluate_formula(instruction, story, paragraphs, paragraph_index),
            "TOC" => self.evaluate_toc(instruction, story, paragraphs, paragraph_index),
            "TC" => self.evaluate_tc(instruction, story, paragraphs, paragraph_index),
            "NEXT" | "NEXTIF" | "SKIPIF" | "MERGEREC" | "MERGESEQ" => {
                self.evaluate_mail_merge_control(instruction, story, paragraphs, paragraph_index)
            }
            "DISPLAYBARCODE" | "MERGEBARCODE" => {
                self.evaluate_barcode(instruction, story, paragraphs, paragraph_index)
            }
            name => keep(&format!("field {name} is unsupported")),
        };

        match outcome {
            FieldOutcome::Resolved(value) => {
                match apply_formats(instruction, &value, self.context.now) {
                    Ok(value) => FieldOutcome::Resolved(value),
                    Err(diagnostic) => keep(&diagnostic),
                }
            }
            other => other,
        }
    }

    fn evaluate_nested_fields(
        &mut self,
        field: &Field,
        instruction: &FieldInstruction,
        story: &str,
        paragraphs: &[&CT_P],
        paragraph_index: usize,
        inherited_locked: bool,
    ) -> Result<()> {
        // Canonical source indices are preorder occurrences under this actual
        // outer source. Resolve effective clones through argument/switch slots,
        // not by equal text, count-only zips or shared child addresses.
        let parent_source = self.current_field_source;
        let original_nested = field.nested_fields_in_source_order();
        let mut next = match parent_source {
            Some(source) => Some(
                source
                    .index
                    .checked_add(1)
                    .ok_or_else(|| Error::Other("field source index overflow".into()))?,
            ),
            None => None,
        };
        let mut instruction_sources = Vec::new();
        for original in &original_nested {
            instruction_sources.push(next);
            let mut subtree = Vec::new();
            collect_preorder_fields(original, false, &mut subtree);
            let count = u32::try_from(subtree.len())
                .map_err(|_| Error::Other("field source count overflow".into()))?;
            next = match next {
                Some(index) => Some(
                    index
                        .checked_add(count)
                        .ok_or_else(|| Error::Other("field source index overflow".into()))?,
                ),
                None => None,
            };
        }
        let mut selected = HashSet::new();
        for effective in field.effective_nested_fields_in_source_order(instruction) {
            let original = (0..instruction.arguments.len())
                .find_map(|slot| {
                    nested_in_same_slot(
                        instruction.arguments.get(slot),
                        field.instruction.arguments.get(slot),
                        effective,
                    )
                })
                .or_else(|| {
                    (0..instruction.switches.len()).find_map(|slot| {
                        nested_in_same_slot(
                            instruction
                                .switches
                                .get(slot)
                                .and_then(|s| s.argument.as_ref()),
                            field
                                .instruction
                                .switches
                                .get(slot)
                                .and_then(|s| s.argument.as_ref()),
                            effective,
                        )
                    })
                });
            let Some(original) = original else {
                return Err(Error::Other(
                    "effective nested field has no current instruction slot".into(),
                ));
            };
            let position = original_nested
                .iter()
                .position(|candidate| std::ptr::eq(*candidate, original))
                .ok_or_else(|| {
                    Error::Other("effective nested field has no active source occurrence".into())
                })?;
            if !selected.insert(position)
                || original != effective
                || original.form() != effective.form()
                || original.source_owner_id() != effective.source_owner_id()
            {
                return Err(Error::Other(
                    "effective instruction slot association is not injective".into(),
                ));
            }
            let source = parent_source
                .zip(instruction_sources[position])
                .map(|(parent, index)| oxoffice_layout::FieldSource {
                    node: parent.node,
                    index,
                });
            let outcome = self.evaluate_field(
                effective,
                story,
                paragraphs,
                paragraph_index,
                Some((source, inherited_locked || effective.locked() == Some(true))),
            );
            self.nested_outcomes
                .last_mut()
                .expect("field evaluation frame exists")
                .insert(std::ptr::from_ref(effective) as usize, outcome);
        }
        if selected.len() != original_nested.len() {
            return Err(Error::Other(
                "effective instruction source association is incomplete".into(),
            ));
        }
        for nested in field.cached_fields_in_source_order() {
            let source =
                parent_source
                    .zip(next)
                    .map(|(parent, index)| oxoffice_layout::FieldSource {
                        node: parent.node,
                        index,
                    });
            let outcome = self.evaluate_field(
                nested,
                story,
                paragraphs,
                paragraph_index,
                Some((source, inherited_locked || nested.locked() == Some(true))),
            );
            self.nested_outcomes
                .last_mut()
                .expect("field evaluation frame exists")
                .insert(std::ptr::from_ref(nested) as usize, outcome);
            let mut subtree = Vec::new();
            collect_preorder_fields(nested, false, &mut subtree);
            let count = u32::try_from(subtree.len())
                .map_err(|_| Error::Other("field source count overflow".into()))?;
            next = match next {
                Some(index) => Some(
                    index
                        .checked_add(count)
                        .ok_or_else(|| Error::Other("field source index overflow".into()))?,
                ),
                None => None,
            };
        }
        Ok(())
    }

    fn evaluate_ref(
        &self,
        instruction: &FieldInstruction,
        _story: &str,
        _paragraph_index: usize,
    ) -> FieldOutcome {
        let Some(target) = text_argument(instruction, 0) else {
            return keep("REF requires a bookmark name");
        };
        if has_switch(instruction, "f") {
            return match ref_bookmark_runs(self.document, target) {
                Ok(Some(cache)) => FieldOutcome::Resolved(
                    self.bookmarks
                        .get(target)
                        .map(|bookmark| bookmark.text.clone())
                        .unwrap_or_else(|| {
                            let name = target.to_lowercase();
                            runs_text_without(&cache.rows, &|candidate| candidate == name)
                        }),
                ),
                Ok(None) => keep(&format!("REF target {target} was not found")),
                Err(error) => keep(&format!("REF f source could not be resolved: {error}")),
            };
        }
        match self.bookmarks.get(target) {
            Some(bookmark) => {
                let position = if has_switch(instruction, "p") {
                    match self
                        .sequence_snapshot
                        .as_ref()
                        .zip(self.current_field_source)
                        .ok_or_else(|| {
                            "REF relative source lacks an accepted physical binding".to_owned()
                        })
                        .and_then(|(snapshot, source)| {
                            snapshot.bookmark_relative_position(target, source)
                        }) {
                        Ok(position) => Some(position.to_owned()),
                        Err(diagnostic) => return keep(&diagnostic),
                    }
                } else {
                    None
                };
                let numbering_switch = if has_switch(instruction, "w") {
                    Some("full")
                } else if has_switch(instruction, "r") {
                    Some("relative")
                } else if has_switch(instruction, "n") {
                    Some("level")
                } else {
                    None
                };
                let Some(_) = numbering_switch else {
                    if let Some(position) = position {
                        return FieldOutcome::Resolved(position);
                    }
                    return FieldOutcome::Resolved(bookmark.text.clone());
                };
                let Some(numbering) = self
                    .numbering_layout
                    .as_ref()
                    .and_then(|layout| layout.bookmark_numbering(target))
                else {
                    let mut value = if self
                        .numbering_layout
                        .as_ref()
                        .is_some_and(|layout| layout.bookmark_numbering_suppressed(target))
                    {
                        "0".into()
                    } else {
                        bookmark.text.clone()
                    };
                    if let Some(position) = position {
                        value.push(' ');
                        value.push_str(&position);
                    }
                    return FieldOutcome::Resolved(value);
                };
                let mut value = match numbering.numbered_reference_text(
                    instruction,
                    self.current_field_source
                        .and_then(|source| self.numbering_source(source.node))
                        .and_then(|node| self.numbering_layout.as_ref()?.paragraph_numbering(node)),
                ) {
                    Ok(value) => value,
                    Err(diagnostic) => return keep(&diagnostic),
                };
                if let Some(position) = position {
                    value.push(' ');
                    value.push_str(&position);
                }
                FieldOutcome::Resolved(value)
            }
            None if self.cross_owner_bookmarks.contains(target) => {
                keep(&format!("REF target {target} runs across story owners"))
            }
            None => keep(&format!("REF target {target} was not found")),
        }
    }

    fn evaluate_pageref(&self, instruction: &FieldInstruction) -> FieldOutcome {
        let Some(target) = text_argument(instruction, 0) else {
            return keep("PAGEREF requires a bookmark name");
        };
        if self.bookmarks.contains_key(target) {
            FieldOutcome::DeferredPagination
        } else {
            keep(&format!("PAGEREF target {target} was not found"))
        }
    }

    fn evaluate_if(
        &mut self,
        instruction: &FieldInstruction,
        story: &str,
        paragraphs: &[&CT_P],
        paragraph_index: usize,
    ) -> FieldOutcome {
        if instruction.arguments.len() != 5 {
            return keep("IF requires two operands, an operator, and two results");
        }
        let arguments = instruction
            .arguments
            .iter()
            .map(|argument| self.resolve_argument(argument, story, paragraphs, paragraph_index))
            .collect::<Vec<_>>();
        let left = match &arguments[0] {
            Ok(value) => value,
            Err(diagnostic) => return keep(diagnostic),
        };
        let operator = match &arguments[1] {
            Ok(value) => value,
            Err(diagnostic) => return keep(diagnostic),
        };
        let right = match &arguments[2] {
            Ok(value) => value,
            Err(diagnostic) => return keep(diagnostic),
        };
        let Some(condition) = compare_if(left, operator, right) else {
            return keep(&format!("IF operator {operator} is unsupported"));
        };
        let selected = if condition { 3 } else { 4 };
        match &arguments[selected] {
            Ok(value) => FieldOutcome::Resolved(value.clone()),
            Err(diagnostic) => keep(diagnostic),
        }
    }

    fn resolve_argument(
        &mut self,
        argument: &FieldArgument,
        story: &str,
        paragraphs: &[&CT_P],
        paragraph_index: usize,
    ) -> std::result::Result<String, String> {
        match argument {
            FieldArgument::Text(value) => Ok(value.clone()),
            FieldArgument::Nested(field) => {
                let key = std::ptr::from_ref(field.as_ref()) as usize;
                let outcome = if let Some(outcome) = self
                    .nested_outcomes
                    .last()
                    .and_then(|outcomes| outcomes.get(&key))
                {
                    outcome.clone()
                } else {
                    let outcome =
                        self.evaluate_field(field, story, paragraphs, paragraph_index, None);
                    self.nested_outcomes
                        .last_mut()
                        .expect("field evaluation frame exists")
                        .insert(key, outcome.clone());
                    outcome
                };
                match outcome {
                    FieldOutcome::Resolved(value) => Ok(value),
                    FieldOutcome::DeferredPagination => {
                        Err("nested field requires deferred pagination".to_owned())
                    }
                    FieldOutcome::TableOfContents(_)
                    | FieldOutcome::TableOfContentsEntry(_)
                    | FieldOutcome::MailMergeControl(_)
                    | FieldOutcome::Barcode(_) => {
                        Err("nested field produced a non-text result".to_owned())
                    }
                    FieldOutcome::KeepStored { diagnostic } => {
                        Err(format!("nested field was not resolved: {diagnostic}"))
                    }
                }
            }
            // Word reads the pieces of one operand joined, each nested field
            // giving its result.
            FieldArgument::Joined(pieces) => {
                let mut value = String::new();
                for piece in pieces {
                    value.push_str(&self.resolve_argument(
                        piece,
                        story,
                        paragraphs,
                        paragraph_index,
                    )?);
                }
                Ok(value)
            }
        }
    }

    fn evaluate_docproperty(&self, instruction: &FieldInstruction) -> FieldOutcome {
        let Some(name) = text_argument(instruction, 0) else {
            return keep("DOCPROPERTY requires a property name");
        };
        if let Some(value) = self.core_property(name) {
            return FieldOutcome::Resolved(value.to_owned());
        }
        let matches = self
            .document
            .custom_properties
            .iter()
            .flat_map(|properties| &properties.properties)
            .filter(|property| {
                property
                    .name
                    .as_deref()
                    .is_some_and(|candidate| candidate.eq_ignore_ascii_case(name))
            })
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return keep(&format!(
                "DOCPROPERTY target {name} was not found or is ambiguous"
            ));
        }
        match custom_property_text(&matches[0].value) {
            Some(value) => FieldOutcome::Resolved(value),
            None => keep(&format!(
                "DOCPROPERTY target {name} has an unsupported value"
            )),
        }
    }

    fn core_property(&self, name: &str) -> Option<&str> {
        let properties = self.document.core_properties.as_ref()?;
        match normalized_name(name).as_str() {
            "title" => properties.title.as_deref(),
            "subject" => properties.subject.as_deref(),
            "author" | "creator" => properties.creator.as_deref(),
            "comments" | "description" => properties.description.as_deref(),
            "keywords" => properties.keywords.as_deref(),
            "lastsavedby" | "lastmodifiedby" => properties.last_modified_by.as_deref(),
            "createtime" | "created" => properties.created.as_deref(),
            "savedate" | "modified" => properties.modified.as_deref(),
            _ => None,
        }
    }

    fn evaluate_docvariable(&self, instruction: &FieldInstruction) -> FieldOutcome {
        let Some(name) = text_argument(instruction, 0) else {
            return keep("DOCVARIABLE requires a variable name");
        };
        let matches = self
            .document
            .settings
            .iter()
            .flat_map(|settings| settings.document_variables())
            .filter(|variable| variable.name.eq_ignore_ascii_case(name))
            .collect::<Vec<_>>();
        if matches.len() == 1 {
            FieldOutcome::Resolved(matches[0].value.clone())
        } else {
            keep(&format!(
                "DOCVARIABLE target {name} was not found or is ambiguous"
            ))
        }
    }

    fn evaluate_styleref(
        &self,
        instruction: &FieldInstruction,
        _paragraphs: &[&CT_P],
        _paragraph_index: usize,
    ) -> FieldOutcome {
        let Some(target) = text_argument(instruction, 0) else {
            return keep("STYLEREF requires a style name");
        };
        let Some(layout) = &self.numbering_layout else {
            return keep("STYLEREF source layout is unavailable");
        };
        if let Some(failure) = layout.navigation_failure() {
            return keep(&format!(
                "STYLEREF navigation source discovery unavailable: {failure}"
            ));
        }
        let Some(field_source) = self.current_field_source else {
            return keep("STYLEREF field lacks an accepted physical binding");
        };
        let Some(caller) = self.numbering_source(field_source.node) else {
            return keep("STYLEREF field source is unavailable in this layout");
        };
        let caller_path = layout.source_node(caller).expect("registered caller");
        let key = target.to_lowercase();
        let candidates = self.navigation_styles.get(&key);
        let source = if caller_path.story == oxoffice_docx_layout::WordStory::Document
            && !has_switch(instruction, "l")
        {
            candidates.and_then(|nodes| {
                let boundary = nodes.partition_point(|node| node.get() < caller.get());
                boundary
                    .checked_sub(1)
                    .and_then(|i| nodes.get(i))
                    .or_else(|| nodes.iter().find(|node| **node != caller))
                    .copied()
            })
        } else {
            let source = oxoffice_layout::FieldSource {
                node: caller,
                index: field_source.index,
            };
            let Some(pages) = self
                .navigation_field_pages
                .get(&source)
                .filter(|pages| pages.len() == 1)
            else {
                return keep("STYLEREF page selection requires one actual placement");
            };
            let page = *pages.first().expect("one actual placement");
            self.navigation_style_pages.get(&key).and_then(|pages| {
                pages
                    .get(&page)
                    .and_then(|nodes| {
                        if has_switch(instruction, "l") {
                            nodes.iter().rev().find(|node| **node != caller)
                        } else {
                            nodes.iter().find(|node| **node != caller)
                        }
                    })
                    .copied()
                    .or_else(|| {
                        pages
                            .range(..page)
                            .next_back()
                            .and_then(|(_, nodes)| nodes.last())
                            .copied()
                    })
                    .or_else(|| {
                        pages
                            .range((std::ops::Bound::Excluded(page), std::ops::Bound::Unbounded))
                            .next()
                            .and_then(|(_, nodes)| nodes.first())
                            .copied()
                    })
            })
        };
        let Some(node) = source else {
            return keep(&format!("STYLEREF target {target} was not found"));
        };
        let Some((path, _, text, _, numbering, placed)) = layout.paragraph_navigation_source(node)
        else {
            return keep("STYLEREF selected source is unavailable");
        };
        if placed.is_none() {
            return keep("STYLEREF selected source is unplaced");
        }
        let numbered = ["n", "r", "w"]
            .iter()
            .any(|name| has_switch(instruction, name));
        let mut value = if numbered {
            match numbering {
                Some(numbering) => match numbering
                    .numbered_reference_text(instruction, layout.paragraph_numbering(caller))
                {
                    Ok(value) => value,
                    Err(diagnostic) => return keep(&diagnostic),
                },
                None if layout.paragraph_numbering_suppressed(node) => "0".into(),
                None => return keep("STYLEREF source numbering is unavailable"),
            }
        } else {
            text.to_owned()
        };
        if has_switch(instruction, "p") {
            if path.story != caller_path.story {
                return keep("STYLEREF relative position lacks a comparable physical story");
            }
            value.push_str(if path.children < caller_path.children {
                " above"
            } else {
                " below"
            });
        }
        FieldOutcome::Resolved(value)
    }

    fn evaluate_includetext(&self, instruction: &FieldInstruction) -> FieldOutcome {
        if has_switch(instruction, "c") {
            return keep("INCLUDETEXT converter selection is unsupported");
        }
        let Some(source) = text_argument(instruction, 0) else {
            return keep("INCLUDETEXT requires a source name");
        };
        let key = text_argument(instruction, 1)
            .map(|bookmark| format!("{source}#{bookmark}"))
            .unwrap_or_else(|| source.to_owned());
        match self.context.included_text.get(&key) {
            Some(value) => FieldOutcome::Resolved(value.clone()),
            None => keep(&format!("INCLUDETEXT input {key} was not supplied")),
        }
    }

    fn evaluate_date_time(&self, instruction: &FieldInstruction) -> FieldOutcome {
        let Some(now) = self.context.now else {
            return keep(&format!(
                "{} requires an explicit date and time",
                instruction.name
            ));
        };
        if !valid_date_time(now) {
            return keep("field date and time is invalid");
        }
        let default_picture = if instruction.name == "DATE" {
            "M/d/yyyy"
        } else {
            "h:mm:ss AM/PM"
        };
        let picture = switch_text(instruction, "@").unwrap_or(default_picture);
        match format_date_time(now, picture) {
            Ok(value) => FieldOutcome::Resolved(value),
            Err(diagnostic) => keep(&diagnostic),
        }
    }

    fn evaluate_filename(&self, instruction: &FieldInstruction) -> FieldOutcome {
        if has_switch(instruction, "p") {
            return self
                .context
                .file_path
                .clone()
                .map(FieldOutcome::Resolved)
                .unwrap_or_else(|| keep("FILENAME path was not supplied"));
        }
        self.context
            .file_name
            .clone()
            .or_else(|| {
                self.context
                    .file_path
                    .as_deref()
                    .and_then(lexical_file_name)
                    .map(str::to_owned)
            })
            .map(FieldOutcome::Resolved)
            .unwrap_or_else(|| keep("FILENAME name was not supplied"))
    }

    fn evaluate_author(&self) -> FieldOutcome {
        self.document
            .core_properties
            .as_ref()
            .and_then(|properties| properties.creator.clone())
            .map(FieldOutcome::Resolved)
            .unwrap_or_else(|| keep("AUTHOR metadata was not found"))
    }

    fn evaluate_mergefield(&self, instruction: &FieldInstruction) -> FieldOutcome {
        let value = text_argument(instruction, 0)
            .and_then(|name| self.context.merge_fields.get(name))
            .map(String::as_str);
        match resolve_mergefield_text(instruction, value, self.missing_merge_fields_as_empty) {
            Ok(value) => FieldOutcome::Resolved(value),
            Err(diagnostic) => keep(&diagnostic),
        }
    }

    fn evaluate_formula(
        &mut self,
        instruction: &FieldInstruction,
        story: &str,
        paragraphs: &[&CT_P],
        paragraph_index: usize,
    ) -> FieldOutcome {
        if instruction.arguments.is_empty() {
            return keep("formula requires an expression");
        }
        let mut expression = String::new();
        for argument in &instruction.arguments {
            let value = match self.resolve_argument(argument, story, paragraphs, paragraph_index) {
                Ok(value) => value,
                Err(diagnostic) => return keep(&diagnostic),
            };
            if !expression.is_empty() {
                expression.push(' ');
            }
            expression.push_str(&value);
            if expression.len() > MAX_FORMULA_BYTES {
                return keep("formula exceeds the 4096-byte limit");
            }
        }
        match FormulaParser::new(&expression).and_then(FormulaParser::parse) {
            Ok(value) => FieldOutcome::Resolved(format_formula_value(value)),
            Err(diagnostic) => keep(&diagnostic),
        }
    }

    fn evaluate_mail_merge_control(
        &mut self,
        instruction: &FieldInstruction,
        story: &str,
        paragraphs: &[&CT_P],
        paragraph_index: usize,
    ) -> FieldOutcome {
        match instruction.name.as_str() {
            "MERGESEQ" if self.context.merge_sequence_number == Some(0) => {
                return keep("MERGESEQ merge sequence number must be one-based");
            }
            "NEXT" | "NEXTIF" | "SKIPIF" | "MERGEREC"
                if self.context.merge_record_number == Some(0) =>
            {
                return keep(&format!(
                    "{} merge record number must be one-based",
                    instruction.name
                ));
            }
            _ => {}
        }
        let condition = match instruction.name.as_str() {
            "NEXTIF" | "SKIPIF" => {
                let values = instruction
                    .arguments
                    .iter()
                    .map(|argument| {
                        self.resolve_argument(argument, story, paragraphs, paragraph_index)
                    })
                    .collect::<std::result::Result<Vec<_>, _>>();
                let values = match values {
                    Ok(values) => values,
                    Err(diagnostic) => return keep(&diagnostic),
                };
                let Some(condition) = compare_if(&values[0], &values[1], &values[2]) else {
                    return keep(&format!(
                        "{} operator {} is unsupported",
                        instruction.name, values[1]
                    ));
                };
                condition
            }
            _ => false,
        };
        let state =
            self.mail_merge_stories
                .entry(story.to_owned())
                .or_insert(MailMergeStoryState {
                    record_number: self.context.merge_record_number,
                    sequence_number: self.context.merge_sequence_number,
                });
        match instruction.name.as_str() {
            "NEXT" => {
                let Some(current_record) = state.record_number else {
                    return keep("NEXT requires an explicit merge record number");
                };
                let Some(record_number) = current_record.checked_add(1) else {
                    return keep("NEXT record number overflowed");
                };
                state.record_number = Some(record_number);
                FieldOutcome::MailMergeControl(MailMergeControl::NextRecord { record_number })
            }
            "NEXTIF" => {
                let Some(mut record_number) = state.record_number else {
                    return keep("NEXTIF requires an explicit merge record number");
                };
                if condition {
                    let Some(next_record_number) = record_number.checked_add(1) else {
                        return keep("NEXTIF record number overflowed");
                    };
                    record_number = next_record_number;
                    state.record_number = Some(record_number);
                }
                FieldOutcome::MailMergeControl(MailMergeControl::NextRecordIf {
                    condition,
                    record_number,
                })
            }
            "SKIPIF" => match state.record_number {
                Some(record_number) => {
                    FieldOutcome::MailMergeControl(MailMergeControl::SkipRecordIf {
                        condition,
                        record_number,
                    })
                }
                None => keep("SKIPIF requires an explicit merge record number"),
            },
            "MERGEREC" => state
                .record_number
                .map(MailMergeControl::RecordNumber)
                .map(FieldOutcome::MailMergeControl)
                .unwrap_or_else(|| keep("MERGEREC requires an explicit merge record number")),
            "MERGESEQ" => state
                .sequence_number
                .map(MailMergeControl::SequenceNumber)
                .map(FieldOutcome::MailMergeControl)
                .unwrap_or_else(|| keep("MERGESEQ requires an explicit merge sequence number")),
            _ => unreachable!(),
        }
    }

    fn evaluate_barcode(
        &mut self,
        instruction: &FieldInstruction,
        story: &str,
        paragraphs: &[&CT_P],
        paragraph_index: usize,
    ) -> FieldOutcome {
        let Some(source) = instruction.arguments.first() else {
            return keep(&format!("{} requires a value", instruction.name));
        };
        let value = match source {
            FieldArgument::Text(source) if instruction.name == "MERGEBARCODE" => {
                match self.context.merge_fields.get(source) {
                    Some(value) => value.clone(),
                    None => return keep(&format!("MERGEBARCODE input {source} was not supplied")),
                }
            }
            _ => match self.resolve_argument(source, story, paragraphs, paragraph_index) {
                Ok(value) => value,
                Err(diagnostic) => return keep(&diagnostic),
            },
        };
        let Some(kind) = instruction.arguments.get(1) else {
            return keep(&format!("{} requires a barcode type", instruction.name));
        };
        let kind = match self.resolve_argument(kind, story, paragraphs, paragraph_index) {
            Ok(value) => value,
            Err(diagnostic) => return keep(&diagnostic),
        };
        let mut switches = Vec::with_capacity(instruction.switches.len());
        for field_switch in &instruction.switches {
            let argument = match &field_switch.argument {
                Some(argument) => {
                    match self.resolve_argument(argument, story, paragraphs, paragraph_index) {
                        Ok(value) => Some(value),
                        Err(diagnostic) => return keep(&diagnostic),
                    }
                }
                None => None,
            };
            switches.push((field_switch.name.clone(), argument));
        }
        match parse_barcode(&instruction.name, &switches, value, &kind) {
            Ok(barcode) => FieldOutcome::Barcode(barcode),
            Err(diagnostic) => keep(&diagnostic),
        }
    }

    fn evaluate_toc(
        &mut self,
        instruction: &FieldInstruction,
        story: &str,
        paragraphs: &[&CT_P],
        paragraph_index: usize,
    ) -> FieldOutcome {
        let mut toc = TocField {
            heading_levels: None,
            custom_styles: Vec::new(),
            entries: TocEntrySelection::None,
            sequence_identifier: None,
            bookmark: None,
            hyperlink: false,
            use_outline_levels: false,
            omit_page_number_levels: None,
            page_number_separator: None,
            entry_page_separator: None,
        };
        let mut has_explicit_source = false;
        for field_switch in &instruction.switches {
            let argument = match &field_switch.argument {
                Some(argument) => {
                    match self.resolve_argument(argument, story, paragraphs, paragraph_index) {
                        Ok(value) => Some(value),
                        Err(diagnostic) => return keep(&diagnostic),
                    }
                }
                None => None,
            };
            match field_switch.name.as_str() {
                "h" if argument.is_none() => toc.hyperlink = true,
                "z" if argument.is_none() => {}
                "u" if argument.is_none() => {
                    toc.use_outline_levels = true;
                    has_explicit_source = true;
                }
                "o" => {
                    has_explicit_source = true;
                    toc.heading_levels = match argument {
                        Some(value) => match parse_level_range(&value, "TOC heading") {
                            Ok(levels) => Some(levels),
                            Err(diagnostic) => return keep(&diagnostic),
                        },
                        None => Some((1, 9)),
                    };
                }
                "n" => {
                    toc.omit_page_number_levels = match argument {
                        Some(value) => match parse_level_range(&value, "TOC omitted page-number") {
                            Ok(levels) => Some(levels),
                            Err(diagnostic) => return keep(&diagnostic),
                        },
                        None => Some((1, 9)),
                    };
                }
                "t" => {
                    let Some(value) = argument else {
                        return keep("field TOC switch \\t requires a text argument");
                    };
                    has_explicit_source = true;
                    toc.custom_styles = match parse_custom_styles(&value) {
                        Ok(styles) => styles,
                        Err(diagnostic) => return keep(&diagnostic),
                    };
                }
                "f" => {
                    has_explicit_source = true;
                    toc.entries = argument.map_or(TocEntrySelection::All, |value| {
                        TocEntrySelection::Identifier(value)
                    });
                }
                "b" | "p" | "s" | "d" => {
                    let Some(value) = argument else {
                        return keep(&format!(
                            "field TOC switch \\{} requires a text argument",
                            field_switch.name
                        ));
                    };
                    match field_switch.name.as_str() {
                        "b" => toc.bookmark = Some(value),
                        "p" if value.chars().count() == 1 => {
                            toc.page_number_separator = Some(value)
                        }
                        "p" => {
                            return keep(
                                "TOC page-number separator must contain exactly one character",
                            );
                        }
                        "s" if !value.is_empty() => toc.sequence_identifier = Some(value),
                        "s" => return keep("TOC sequence identifier must not be empty"),
                        "d" => toc.entry_page_separator = Some(value),
                        _ => unreachable!(),
                    }
                }
                name if argument.is_some() => {
                    return keep(&format!(
                        "field TOC switch \\{name} does not take an argument"
                    ));
                }
                name => return keep(&format!("field TOC uses unsupported switch \\{name}")),
            }
        }
        if !has_explicit_source {
            toc.heading_levels = Some((1, 9));
        }
        FieldOutcome::TableOfContents(toc)
    }

    fn evaluate_tc(
        &mut self,
        instruction: &FieldInstruction,
        story: &str,
        paragraphs: &[&CT_P],
        paragraph_index: usize,
    ) -> FieldOutcome {
        let Some(entry) = instruction.arguments.first() else {
            return keep("TC requires entry text");
        };
        let entry = match self.resolve_argument(entry, story, paragraphs, paragraph_index) {
            Ok(entry) => entry,
            Err(diagnostic) => return keep(&diagnostic),
        };
        if entry.is_empty() {
            return keep("TC entry text must not be empty");
        }
        let mut tc = TcField {
            entry,
            level: 1,
            table_identifier: None,
            omit_page_number: false,
        };
        for field_switch in &instruction.switches {
            let argument = match &field_switch.argument {
                Some(argument) => {
                    match self.resolve_argument(argument, story, paragraphs, paragraph_index) {
                        Ok(value) => Some(value),
                        Err(diagnostic) => return keep(&diagnostic),
                    }
                }
                None => None,
            };
            match field_switch.name.as_str() {
                "n" if argument.is_none() => tc.omit_page_number = true,
                "f" => match argument {
                    Some(value) if !value.is_empty() => tc.table_identifier = Some(value),
                    Some(_) => return keep("TC table identifier must not be empty"),
                    None => return keep("field TC switch \\f requires a text argument"),
                },
                "l" => {
                    let Some(value) = argument else {
                        return keep("field TC switch \\l requires a text argument");
                    };
                    tc.level = match parse_toc_level(&value, "TC") {
                        Ok(level) => level,
                        Err(diagnostic) => return keep(&diagnostic),
                    };
                }
                name if argument.is_some() => {
                    return keep(&format!(
                        "field TC switch \\{name} does not take an argument"
                    ));
                }
                name => return keep(&format!("field TC uses unsupported switch \\{name}")),
            }
        }
        FieldOutcome::TableOfContentsEntry(tc)
    }
}
