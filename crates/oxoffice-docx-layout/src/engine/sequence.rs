//! SEQ fields: evaluating sequence numbers over the document, with navigation exclusions.

use super::*;

/// Evaluate sequence fields from accepted physical sources without layout.
pub fn evaluate_sequence_fields(input: &LayoutInput) -> Result<crate::WordSequenceSnapshot> {
    if input.revision_view != RevisionView::Accepted {
        return Err(LayoutError::Layout(
            "sequence evaluation requires the accepted revision view".into(),
        ));
    }
    let sources = SourceRegistry::for_input(input)?;
    sequence_snapshot(input, &sources)
}

#[derive(Default)]
struct SequenceCounter {
    value: Option<i64>,
    heading: Option<usize>,
}

struct SequenceCollector<'a> {
    input: &'a LayoutInput,
    snapshot: crate::WordSequenceSnapshot,
    counters: HashMap<String, SequenceCounter>,
    headings: [Option<usize>; 9],
    paragraph_index: usize,
    note_contexts: HashMap<WordStory, Vec<usize>>,
    bookmark_starts: Vec<(i32, String, WordStory, usize, usize)>,
    bookmark_ends: Vec<(i32, WordStory, usize, usize)>,
    bookmark_order: usize,
    bookmark_requests: Vec<(FieldSource, FieldInstruction)>,
}

pub(crate) fn validate_navigation_exclusions(
    input: &LayoutInput,
    facts: &HashMap<WordSourcePath, Vec<usize>>,
) -> Result<()> {
    if input.revision_view != RevisionView::Accepted
        || !input.story_part_names.contains_key(&WordStory::Document)
    {
        return Err(LayoutError::Layout(
            "navigation exclusions require a qualified accepted main story".into(),
        ));
    }
    let sources = SourceRegistry::for_input(input)?;
    let mut counts = HashMap::new();
    for (index, path) in sources.nodes.iter().enumerate() {
        let node = SourceNodeId::new(
            u32::try_from(index + 1)
                .map_err(|_| LayoutError::Layout("navigation source limit".into()))?,
        )
        .expect("one based source");
        if let Some(source) = sources.navigation_sources.get(&node)
            && counts.insert(path, source.accepted_runs).is_some()
        {
            return Err(LayoutError::Layout(
                "ambiguous navigation physical path".into(),
            ));
        }
    }
    for (path, runs) in facts {
        let count = counts.get(path);
        if path.story != WordStory::Document
            || count.is_none()
            || runs.windows(2).any(|pair| pair[0] >= pair[1])
            || runs
                .last()
                .is_some_and(|run| *run >= count.copied().unwrap_or(0))
        {
            return Err(LayoutError::Layout(
                "navigation exclusion lacks a unique bounded physical source".into(),
            ));
        }
    }
    Ok(())
}

pub(super) fn sequence_snapshot(
    input: &LayoutInput,
    sources: &SourceRegistry,
) -> Result<crate::WordSequenceSnapshot> {
    let source_ids = sources
        .nodes
        .iter()
        .enumerate()
        .map(|(index, path)| {
            (
                path.clone(),
                SourceNodeId::new((index + 1) as u32).expect("registered source"),
            )
        })
        .collect();
    let navigation = input
        .sequence_snapshot
        .as_ref()
        .filter(|snapshot| snapshot.navigation_document.is_some());
    if let Some(snapshot) = navigation {
        if snapshot.navigation_document.as_ref() != Some(&input.document)
            || snapshot.navigation_part.as_ref() != input.story_part_names.get(&WordStory::Document)
        {
            return Err(LayoutError::Layout(
                "stale navigation exclusion source stamp".into(),
            ));
        }
        validate_navigation_exclusions(input, &snapshot.navigation_exclusions)?;
    }
    let mut collector = SequenceCollector {
        input,
        snapshot: crate::WordSequenceSnapshot {
            source_nodes: sources.nodes.clone(),
            navigation_document: navigation.and_then(|s| s.navigation_document.clone()),
            navigation_failure: navigation.and_then(|s| s.navigation_failure.clone()),
            navigation_part: navigation.and_then(|s| s.navigation_part.clone()),
            navigation_exclusions: navigation
                .map(|s| s.navigation_exclusions.clone())
                .unwrap_or_default(),
            navigation_owners: navigation
                .map(|s| s.navigation_owners.clone())
                .unwrap_or_default(),
            source_ids,
            field_source_xml: sources.field_source_xml.clone(),
            field_source_contexts: sources.field_source_contexts.clone(),
            text_box_owner_indices: sources.text_box_owner_indices.clone(),
            physical_story_owners: sources
                .nodes
                .iter()
                .filter_map(|path| {
                    let (part, owner) = match &path.story {
                        WordStory::Document => (String::new(), WordStory::Document),
                        WordStory::Header { .. } => (
                            input.story_part_names.get(&path.story)?.clone(),
                            WordStory::Header {
                                relationship_id: String::new(),
                            },
                        ),
                        WordStory::Footer { .. } => (
                            input.story_part_names.get(&path.story)?.clone(),
                            WordStory::Footer {
                                relationship_id: String::new(),
                            },
                        ),
                        WordStory::Footnote { .. } | WordStory::Endnote { .. } => (
                            input
                                .story_part_names
                                .get(&path.story)
                                .cloned()
                                .unwrap_or_default(),
                            path.story.clone(),
                        ),
                        WordStory::TextBox { part_name, .. } => (
                            part_name.clone(),
                            WordStory::TextBox {
                                part_name: part_name.clone(),
                                owner_children: vec![
                                    *sources.text_box_owner_indices.get(&path.story)?,
                                ],
                            },
                        ),
                    };
                    Some((path.story.clone(), (part, owner)))
                })
                .collect(),
            ..Default::default()
        },
        counters: HashMap::new(),
        headings: [None; 9],
        paragraph_index: 0,
        note_contexts: HashMap::new(),
        bookmark_starts: Vec::new(),
        bookmark_ends: Vec::new(),
        bookmark_order: 0,
        bookmark_requests: Vec::new(),
    };
    collector.body(&input.document.body, &WordStory::Document, true, None)?;
    for (header, parts) in [(true, &input.headers), (false, &input.footers)] {
        let mut entries = parts.iter().collect::<Vec<_>>();
        entries.sort_by_key(|(id, _)| *id);
        for (id, part) in entries {
            let story = if header {
                WordStory::Header {
                    relationship_id: id.clone(),
                }
            } else {
                WordStory::Footer {
                    relationship_id: id.clone(),
                }
            };
            if let Some(body) = input.story_bodies.get(&story) {
                collector.body(body, &story, false, None)?;
            } else {
                for (index, paragraph) in part.paragraphs.iter().enumerate() {
                    collector.paragraph(
                        paragraph,
                        &WordSourcePath {
                            story: story.clone(),
                            children: vec![index],
                        },
                        false,
                        None,
                    )?;
                }
            }
        }
    }
    for (endnote, stream) in [
        (false, input.footnotes.as_ref()),
        (true, input.endnotes.as_ref()),
    ] {
        let Some(stream) = stream else {
            continue;
        };
        for note in &stream.footnotes {
            if stream.get_by_id(note.id).is_none() {
                continue;
            }
            let story = if endnote {
                WordStory::Endnote { id: note.id }
            } else {
                WordStory::Footnote { id: note.id }
            };
            let context = collector
                .note_contexts
                .get(&story)
                .and_then(|contexts| (contexts.len() == 1).then(|| contexts[0]));
            if let Some(body) = input.story_bodies.get(&story) {
                collector.body(body, &story, false, context)?;
            } else {
                for (index, paragraph) in note.paragraphs.iter().enumerate() {
                    collector.paragraph(
                        paragraph,
                        &WordSourcePath {
                            story: story.clone(),
                            children: vec![index],
                        },
                        false,
                        context,
                    )?;
                }
            }
        }
    }
    collector.resolve_bookmark_requests();
    Ok(collector.snapshot)
}

impl SequenceCollector<'_> {
    fn body(
        &mut self,
        body: &CT_Body,
        story: &WordStory,
        main: bool,
        context: Option<usize>,
    ) -> Result<()> {
        for item in body_layout_items(body) {
            match item {
                MainStoryLayoutItem::Paragraph(paragraph, children) => self.paragraph(
                    paragraph,
                    &WordSourcePath {
                        story: story.clone(),
                        children,
                    },
                    main,
                    context,
                )?,
                MainStoryLayoutItem::Table(table, children) => {
                    self.table(table, story, &children, main, context)?
                }
            }
        }
        Ok(())
    }

    fn table(
        &mut self,
        table: &CT_Tbl,
        story: &WordStory,
        prefix: &[usize],
        main: bool,
        context: Option<usize>,
    ) -> Result<()> {
        for (row, row_path) in table::layout_table_rows(table, prefix) {
            for (cell, cell_path) in table::layout_row_cells(row, &row_path) {
                for (index, content) in cell.content.iter().enumerate() {
                    let mut path = cell_path.clone();
                    path.push(index);
                    self.cell_content(content, story, &path, main, context)?;
                }
            }
        }
        Ok(())
    }

    fn cell_content(
        &mut self,
        content: &CellContent,
        story: &WordStory,
        path: &[usize],
        main: bool,
        context: Option<usize>,
    ) -> Result<()> {
        match content {
            CellContent::Paragraph(paragraph) => self.paragraph(
                paragraph,
                &WordSourcePath {
                    story: story.clone(),
                    children: path.to_vec(),
                },
                main,
                context,
            ),
            CellContent::Table(table) => self.table(table, story, path, main, context),
            CellContent::ContentControl(control) => {
                self.cell_control(control, story, path, main, context)
            }
        }
    }

    fn cell_control(
        &mut self,
        control: &CT_Sdt,
        story: &WordStory,
        prefix: &[usize],
        main: bool,
        context: Option<usize>,
    ) -> Result<()> {
        for (index, content) in control.content.iter().enumerate() {
            let mut path = prefix.to_vec();
            path.push(index);
            match content {
                SdtContent::Paragraph(paragraph) => self.paragraph(
                    paragraph,
                    &WordSourcePath {
                        story: story.clone(),
                        children: path,
                    },
                    main,
                    context,
                )?,
                SdtContent::Table(table) => self.table(table, story, &path, main, context)?,
                SdtContent::ContentControl(control) => {
                    self.cell_control(control, story, &path, main, context)?
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn bookmark_boundaries(
        &mut self,
        markers: &[oxoffice_docx_oxml::text::BookmarkMarker],
        story: &WordStory,
        run: usize,
    ) {
        for marker in markers
            .iter()
            .filter(|marker| marker.projected_run_index() == run)
        {
            let Some(id) = marker.id() else {
                continue;
            };
            let boundary = self.snapshot.main_events.len();
            let order = self.bookmark_order;
            self.bookmark_order += 1;
            if marker.is_start() {
                if opens_bookmark(self.input, marker)
                    && let Some(name) = marker.name()
                {
                    self.bookmark_starts.push((
                        id,
                        name.to_owned(),
                        story.clone(),
                        boundary,
                        order,
                    ));
                }
            } else {
                self.bookmark_ends
                    .push((id, story.clone(), boundary, order));
            }
        }
    }

    fn resolve_bookmark_requests(&mut self) {
        for (source, instruction) in std::mem::take(&mut self.bookmark_requests) {
            let target = field_text_argument(&instruction, 1).expect("validated bookmark operand");
            let starts = self
                .bookmark_starts
                .iter()
                .filter(|(_, name, _, _, _)| {
                    oxoffice_docx_oxml::text::same_bookmark_name(name, target)
                })
                .collect::<Vec<_>>();
            let result = if starts.is_empty() {
                Ok("Error! Bookmark not defined.".into())
            } else if starts.len() != 1 {
                Err("SEQ bookmark target is ambiguous".into())
            } else {
                let (id, _, story, start, start_order) = starts[0];
                let ends = self
                    .bookmark_ends
                    .iter()
                    .filter(|(end_id, end_story, _, _)| end_id == id && end_story == story)
                    .collect::<Vec<_>>();
                if ends.len() != 1
                    || ends[0].3 < *start_order
                    || self
                        .bookmark_starts
                        .iter()
                        .filter(|(other_id, _, other_story, _, _)| {
                            other_id == id && other_story == story
                        })
                        .count()
                        != 1
                {
                    Err("SEQ bookmark target has no unique forward paired range".into())
                } else {
                    let end = ends[0].2;
                    let identifier = field_text_argument(&instruction, 0)
                        .expect("validated identifier")
                        .to_ascii_lowercase();
                    let indexes = self.snapshot.identifier_events.get(&identifier);
                    let in_range = indexes.and_then(|indexes| {
                        let upper = indexes.partition_point(|index| *index < end);
                        upper
                            .checked_sub(1)
                            .map(|index| indexes[index])
                            .filter(|index| *index >= *start)
                    });
                    let value = in_range
                        .map(|index| self.snapshot.main_events[index].value)
                        .or_else(|| self.snapshot.context_value(*start, &identifier))
                        .unwrap_or(0);
                    let display = if field_has_switch(&instruction, "h")
                        && !field_has_switch(&instruction, "*")
                    {
                        String::new()
                    } else {
                        value.to_string()
                    };
                    format_sequence_display(&instruction, display)
                }
            };
            self.snapshot.field_values.insert(source, result);
        }
    }

    fn paragraph(
        &mut self,
        paragraph: &CT_P,
        path: &WordSourcePath,
        main: bool,
        context: Option<usize>,
    ) -> Result<()> {
        let node = self.snapshot.source_id(path).ok_or_else(|| {
            LayoutError::Layout("sequence paragraph has no registered physical source".into())
        })?;
        if main {
            let mut properties = style_resolver::resolve_paragraph_properties(
                paragraph
                    .properties
                    .as_ref()
                    .and_then(|properties| properties.style_id.as_deref()),
                &self.input.styles,
            );
            if let Some(direct) = &paragraph.properties {
                properties.merge_from(direct);
            }
            let heading_level = properties.outline_lvl.or_else(|| {
                detect_heading_level(paragraph, &self.input.styles).map(|level| level - 1)
            });
            let cached = self
                .snapshot
                .navigation_exclusions
                .get(path)
                .is_some_and(|runs| {
                    runs.len() == project_paragraph_runs(paragraph, RevisionView::Accepted).len()
                });
            if !cached && let Some(level) = heading_level.filter(|level| *level < 9) {
                self.headings[level as usize] = Some(self.paragraph_index);
            }
            self.paragraph_index += 1;
        }
        let mut registered_fields = HashMap::new();
        let mut next_field = 0u32;
        for run in paragraph.source_runs() {
            for content in &run.content {
                if let RunContent::Field(field) = content {
                    registered_fields.insert(std::ptr::from_ref(field), next_field);
                    let mut pending = vec![field];
                    while let Some(field) = pending.pop() {
                        next_field += 1;
                        pending.extend(field.all_nested_fields_in_source_order().into_iter().rev());
                    }
                }
            }
        }
        let projected_runs = project_paragraph_runs(paragraph, RevisionView::Accepted);
        let projected_count = projected_runs.len();
        let current_bookmarks = self.input.block_bookmarks().paragraph_projection(paragraph);
        for (order, marker) in current_bookmarks.iter().enumerate() {
            let Some(id) = marker.id() else {
                continue;
            };
            let run = marker.projected_run_index();
            if run > projected_count {
                continue;
            }
            if marker.is_start() {
                if opens_bookmark(self.input, marker)
                    && let Some(name) = marker.name()
                {
                    self.snapshot.position_bookmark_starts.push((
                        id,
                        name.to_owned(),
                        node,
                        run,
                        order,
                    ));
                }
            } else {
                self.snapshot
                    .position_bookmark_ends
                    .push((id, node, run, order));
            }
        }
        for (run_index, projected) in projected_runs.into_iter().enumerate() {
            if main {
                self.bookmark_boundaries(&current_bookmarks, &path.story, run_index);
            }
            for content in &projected.run.content {
                match content {
                    RunContent::Field(field) => {
                        if let Some(mut field_index) =
                            registered_fields.get(&std::ptr::from_ref(field)).copied()
                        {
                            self.field(
                                field,
                                node,
                                &mut field_index,
                                run_index,
                                main,
                                context,
                                false,
                                self.snapshot
                                    .navigation_exclusions
                                    .get(path)
                                    .is_some_and(|runs| runs.binary_search(&run_index).is_ok()),
                            );
                        }
                        if main {
                            for reference in
                                content_note_references(content, RevisionView::Accepted)?
                            {
                                let story = match reference {
                                    RunContent::FootnoteRef { id, .. } => {
                                        WordStory::Footnote { id }
                                    }
                                    RunContent::EndnoteRef { id, .. } => WordStory::Endnote { id },
                                    _ => continue,
                                };
                                self.note_contexts
                                    .entry(story)
                                    .or_default()
                                    .push(self.snapshot.main_events.len());
                            }
                        }
                    }
                    RunContent::FootnoteRef { id, .. } if main => self
                        .note_contexts
                        .entry(WordStory::Footnote { id: *id })
                        .or_default()
                        .push(self.snapshot.main_events.len()),
                    RunContent::EndnoteRef { id, .. } if main => self
                        .note_contexts
                        .entry(WordStory::Endnote { id: *id })
                        .or_default()
                        .push(self.snapshot.main_events.len()),
                    _ => {}
                }
            }
            // Selected typed anchors alone participate. Fallback drawings have
            // no second source and are not traversed alongside their Choice.
            let drawings = projected
                .run
                .content
                .iter()
                .filter_map(|content| match content {
                    RunContent::Drawing(drawing) => Some(drawing),
                    _ => None,
                })
                .chain(projected.run.alt_drawings.iter());
            for (drawing_index, drawing) in drawings.enumerate() {
                let Some(anchor) = drawing.anchor.as_ref() else {
                    continue;
                };
                if anchor.chart_rel_id.is_some() || !anchor.embed_id.is_empty() {
                    continue;
                }
                let Some(shape) = anchor.shape.as_ref() else {
                    continue;
                };
                let part_name = match &path.story {
                    WordStory::TextBox { part_name, .. } => Some(part_name.clone()),
                    story => self.input.story_part_names.get(story).cloned(),
                };
                let Some(part_name) = part_name else {
                    continue;
                };
                let mut owner_children = match &path.story {
                    WordStory::TextBox { owner_children, .. } => {
                        let mut result = owner_children.clone();
                        result.extend(&path.children);
                        result
                    }
                    WordStory::Footnote { id } | WordStory::Endnote { id } => {
                        let stream = if matches!(path.story, WordStory::Footnote { .. }) {
                            self.input.footnotes.as_ref()
                        } else {
                            self.input.endnotes.as_ref()
                        };
                        let mut result = stream
                            .and_then(|stream| {
                                stream.footnotes.iter().position(|note| note.id == *id)
                            })
                            .map(|index| vec![index])
                            .unwrap_or_default();
                        result.extend(&path.children);
                        result
                    }
                    _ => path.children.clone(),
                };
                owner_children.extend([run_index, drawing_index]);
                let story = WordStory::TextBox {
                    part_name,
                    owner_children,
                };
                if let Some(body) = &shape.text_body {
                    self.body(body, &story, main, context)?;
                } else {
                    for (index, paragraph) in shape.text.iter().enumerate() {
                        self.paragraph(
                            paragraph,
                            &WordSourcePath {
                                story: story.clone(),
                                children: vec![index],
                            },
                            main,
                            context,
                        )?;
                    }
                }
            }
        }
        if main {
            self.bookmark_boundaries(&current_bookmarks, &path.story, projected_count);
        }
        Ok(())
    }

    fn field(
        &mut self,
        field: &oxoffice_docx_oxml::text::Field,
        node: SourceNodeId,
        index: &mut u32,
        run: usize,
        main: bool,
        context: Option<usize>,
        locked: bool,
        cached: bool,
    ) {
        let source = FieldSource {
            node,
            index: *index,
        };
        *index += 1;
        let locked = locked || field.locked() == Some(true);
        if cached && let Some(context) = self.snapshot.field_source_contexts.get_mut(&source) {
            context.2 = true;
        }
        if !cached {
            self.snapshot.field_run_positions.insert(source, run);
        }
        for nested in field.nested_fields_in_source_order() {
            self.field(nested, node, index, run, main, context, locked, cached);
        }
        let instruction = field.effective_instruction();
        if instruction.name == "SEQ" {
            let value = if locked {
                Err("locked SEQ field retains its stored display".into())
            } else if cached {
                Err("cached SEQ result is not an independent sequence source".into())
            } else if !instruction.quotes_are_balanced() {
                Err("field SEQ has unclosed quoting".into())
            } else {
                self.sequence(&instruction, source, run, main, context)
            };
            if let Some((_, stored_display)) = self.snapshot.repeat_requests.get_mut(&source) {
                *stored_display = field.cached_result.clone();
            }
            let value = value.and_then(|value| format_sequence_display(&instruction, value));
            self.snapshot.field_values.insert(source, value);
        }
        for nested in field.cached_fields_in_source_order() {
            self.field(nested, node, index, run, main, context, locked, true);
        }
    }

    fn sequence(
        &mut self,
        instruction: &FieldInstruction,
        source: FieldSource,
        run: usize,
        main: bool,
        context: Option<usize>,
    ) -> std::result::Result<String, String> {
        let identifier = field_text_argument(instruction, 0)
            .filter(|identifier| !identifier.is_empty())
            .ok_or("SEQ requires an identifier")?
            .to_ascii_lowercase();
        if identifier.len() > 40
            || !identifier.as_bytes()[0].is_ascii_alphabetic()
            || !identifier
                .bytes()
                .all(|value| value.is_ascii_alphanumeric() || value == b'_')
        {
            return Err("SEQ identifier requires a letter followed by letters, digits or underscores, at most 40 characters".into());
        }
        if !(1..=2).contains(&instruction.arguments.len())
            || (instruction.arguments.len() == 2
                && field_text_argument(instruction, 1).is_none_or(str::is_empty))
        {
            return Err("SEQ requires an identifier and optional bookmark name".into());
        }
        for switch in &instruction.switches {
            let takes_argument = matches!(switch.name.as_str(), "r" | "s" | "*" | "#");
            if !matches!(
                switch.name.as_str(),
                "n" | "c" | "h" | "r" | "s" | "*" | "#"
            ) {
                return Err(format!("SEQ uses unsupported switch \\{}", switch.name));
            }
            if takes_argument != matches!(switch.argument, Some(FieldArgument::Text(_))) {
                return Err(format!(
                    "SEQ switch \\{} has an invalid operand",
                    switch.name
                ));
            }
        }
        if instruction.arguments.len() == 2 {
            if !main {
                return Err(
                    "SEQ bookmark reference lacks a supported related-story context".into(),
                );
            }
            self.bookmark_requests.push((source, instruction.clone()));
            return Err("SEQ bookmark reference awaits complete physical source traversal".into());
        }
        let repeat = field_has_switch(instruction, "c");
        if !main {
            if !repeat {
                return Ok("Error! Main Document Only.".into());
            }
            self.snapshot
                .repeat_requests
                .insert(source, (instruction.clone(), String::new()));
            let context = context.ok_or("SEQ repeat has no unique physical document context")?;
            let value = self
                .snapshot
                .context_value(context, &identifier)
                .ok_or("SEQ repeat has no preceding value")?;
            return Ok(value.to_string());
        }
        let heading = instruction
            .switches
            .iter()
            .find(|switch| switch.name == "s")
            .and_then(|switch| switch.argument.as_ref())
            .and_then(|argument| match argument {
                FieldArgument::Text(level) => level.parse::<usize>().ok(),
                _ => None,
            });
        let heading = if field_has_switch(instruction, "s") {
            Some(
                heading
                    .filter(|level| (1..=9).contains(level))
                    .and_then(|level| self.headings[level - 1])
                    .ok_or("SEQ heading restart has no matching heading")?,
            )
        } else {
            None
        };
        let counter = self.counters.entry(identifier.clone()).or_default();
        if heading.is_some() && counter.heading != heading {
            counter.value = None;
            counter.heading = heading;
        }
        let reset = instruction
            .switches
            .iter()
            .find(|switch| switch.name == "r")
            .and_then(|switch| switch.argument.as_ref());
        let value = if let Some(FieldArgument::Text(reset)) = reset {
            let value = reset
                .parse::<i64>()
                .map_err(|_| "SEQ reset value is not an integer")?;
            counter.value = Some(value);
            value
        } else if repeat {
            counter.value.ok_or("SEQ repeat has no preceding value")?
        } else {
            let value = counter
                .value
                .unwrap_or(0)
                .checked_add(1)
                .ok_or("SEQ value overflowed")?;
            counter.value = Some(value);
            value
        };
        if !repeat || reset.is_some() {
            let event = self.snapshot.main_events.len();
            self.snapshot
                .identifier_events
                .entry(identifier.clone())
                .or_default()
                .push(event);
            self.snapshot.event_sources.insert(source, event);
            self.snapshot.main_events.push(crate::WordSequenceEvent {
                source,
                accepted_run: run,
                identifier,
                value,
            });
        }
        if field_has_switch(instruction, "h") && !field_has_switch(instruction, "*") {
            Ok(String::new())
        } else {
            Ok(value.to_string())
        }
    }
}
