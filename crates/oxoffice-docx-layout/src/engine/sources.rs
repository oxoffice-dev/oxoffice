//! The source registry: provenance node ids for paragraphs, fields and text boxes.

use super::*;

pub(super) enum TextBoxSourceBinding {
    Paragraphs(Vec<SourceNodeId>),
    Body(Vec<SourceNodeId>),
}

impl SourceRegistry {
    pub(super) fn for_body(input: &LayoutInput, body: &CT_Body) -> Result<Self> {
        let mut registry = Self {
            nodes: Vec::new(),
            navigation_sources: HashMap::new(),
            ids: HashMap::new(),
            navigation_paths: BTreeMap::new(),
            body_ids: Vec::new(),
            field_descendants: HashMap::new(),
            field_source_xml: HashMap::new(),
            field_source_contexts: HashMap::new(),
            page_fields: HashMap::new(),
            styleref_requests: HashMap::new(),
            text_box_owner_indices: HashMap::new(),
            story_part_names: input.story_part_names.clone(),
            story_owner_children: HashMap::new(),
            revision_view: input.revision_view,
            text_box_sources: HashMap::new(),
            text_box_stories: HashMap::new(),
            preparation_error: None,
        };
        registry.collect_story_body(body, &WordStory::Document);
        if let Some(error) = registry.preparation_error.take() {
            return Err(LayoutError::Layout(error));
        }
        Ok(registry)
    }

    pub(super) fn bind_sequence_shape_sources(
        &mut self,
        input: &LayoutInput,
        parent: SourceNodeId,
        run_index: usize,
        drawing_index: usize,
        physical_owner: Option<usize>,
    ) -> std::result::Result<HashMap<SourceNodeId, SourceNodeId>, String> {
        let snapshot = input
            .sequence_snapshot
            .as_ref()
            .ok_or("text-box source snapshot is unavailable")?;
        let parent = snapshot
            .source_node(parent)
            .ok_or("text-box parent source is unregistered")?;
        let part_name = match &parent.story {
            WordStory::TextBox { part_name, .. } => part_name.clone(),
            story => input
                .story_part_names
                .get(story)
                .cloned()
                .ok_or("text-box parent part is unavailable")?,
        };
        let mut owner_children = match &parent.story {
            WordStory::TextBox { owner_children, .. } => owner_children.clone(),
            WordStory::Footnote { id } | WordStory::Endnote { id } => {
                let notes = if matches!(parent.story, WordStory::Footnote { .. }) {
                    input.footnotes.as_ref()
                } else {
                    input.endnotes.as_ref()
                };
                vec![
                    notes
                        .and_then(|notes| notes.footnotes.iter().position(|note| note.id == *id))
                        .ok_or("text-box note owner is unavailable")?,
                ]
            }
            _ => Vec::new(),
        };
        owner_children.extend(&parent.children);
        owner_children.extend([run_index, drawing_index]);
        let story = WordStory::TextBox {
            part_name: part_name.clone(),
            owner_children: owner_children.clone(),
        };
        if physical_owner
            .is_some_and(|owner| snapshot.text_box_owner_indices.get(&story) != Some(&owner))
        {
            return Err(
                "text-box selected physical owner differs from its registered source".into(),
            );
        }
        let mut forward = HashMap::new();
        let mut inverse = HashMap::new();
        for (index, path) in self.nodes.iter().enumerate() {
            let global_story = match &path.story {
                WordStory::Document => story.clone(),
                WordStory::TextBox {
                    owner_children: nested,
                    ..
                } => {
                    let mut children = owner_children.clone();
                    children.extend(nested);
                    WordStory::TextBox {
                        part_name: part_name.clone(),
                        owner_children: children,
                    }
                }
                _ => return Err("text-box scope contains an unrelated source story".into()),
            };
            let global = snapshot
                .source_id(&WordSourcePath {
                    story: global_story,
                    children: path.children.clone(),
                })
                .ok_or("text-box child path is not registered in its selected physical owner")?;
            let local = SourceNodeId::new((index + 1) as u32)
                .ok_or("text-box local source exceeds its identity range")?;
            if inverse.insert(global, local).is_some() {
                return Err("text-box source mapping is not injective".into());
            }
            forward.insert(local, global);
        }
        for (source, (instruction, locked, cached)) in &self.field_source_contexts {
            let global = FieldSource {
                node: forward[&source.node],
                index: source.index,
            };
            let Some((expected, inherited_lock, generated)) = snapshot.source_field_context(global)
            else {
                return Err("text-box child field context is unregistered".into());
            };
            if expected != instruction || (*locked && !inherited_lock) || *cached != generated {
                return Err("text-box child field context differs from its physical source".into());
            }
            if let Some(raw) = self.field_source_xml.get(source)
                && snapshot.source_field_xml(global) != Some(raw.as_slice())
            {
                return Err("text-box child raw field differs from its physical source".into());
            }
        }
        for node in self.ids.values_mut() {
            *node = forward[node];
        }
        for node in self.navigation_paths.values_mut() {
            *node = forward[node];
        }
        for node in self.body_ids.iter_mut().flatten() {
            *node = forward[node];
        }
        self.field_descendants = std::mem::take(&mut self.field_descendants)
            .into_iter()
            .map(|(source, value)| {
                (
                    FieldSource {
                        node: forward[&source.node],
                        index: source.index,
                    },
                    value,
                )
            })
            .collect();
        self.field_source_xml = std::mem::take(&mut self.field_source_xml)
            .into_iter()
            .map(|(source, value)| {
                (
                    FieldSource {
                        node: forward[&source.node],
                        index: source.index,
                    },
                    value,
                )
            })
            .collect();
        self.field_source_contexts = std::mem::take(&mut self.field_source_contexts)
            .into_iter()
            .map(|(source, value)| {
                (
                    FieldSource {
                        node: forward[&source.node],
                        index: source.index,
                    },
                    value,
                )
            })
            .collect();
        self.page_fields = std::mem::take(&mut self.page_fields)
            .into_iter()
            .map(|(source, value)| {
                (
                    FieldSource {
                        node: forward[&source.node],
                        index: source.index,
                    },
                    value,
                )
            })
            .collect();
        self.text_box_sources = std::mem::take(&mut self.text_box_sources)
            .into_iter()
            .map(|(owner, mut bindings)| {
                for binding in &mut bindings {
                    let nodes = match binding {
                        TextBoxSourceBinding::Paragraphs(nodes)
                        | TextBoxSourceBinding::Body(nodes) => nodes,
                    };
                    for node in nodes {
                        *node = forward[node];
                    }
                }
                (forward[&owner], bindings)
            })
            .collect();
        Ok(inverse)
    }

    pub(super) fn for_input(input: &LayoutInput) -> Result<Self> {
        let mut registry = Self {
            nodes: Vec::new(),
            navigation_sources: HashMap::new(),
            ids: HashMap::new(),
            navigation_paths: BTreeMap::new(),
            body_ids: Vec::with_capacity(input.document.body.content.len()),
            field_descendants: HashMap::new(),
            field_source_xml: HashMap::new(),
            field_source_contexts: HashMap::new(),
            page_fields: HashMap::new(),
            styleref_requests: HashMap::new(),
            text_box_owner_indices: HashMap::new(),
            story_part_names: input.story_part_names.clone(),
            story_owner_children: HashMap::new(),
            revision_view: input.revision_view,
            text_box_sources: HashMap::new(),
            text_box_stories: HashMap::new(),
            preparation_error: None,
        };

        for (body_index, content) in input.document.body.content.iter().enumerate() {
            match content {
                BodyContent::Paragraph(paragraph) => {
                    let id = registry.insert_node(WordSourcePath {
                        story: WordStory::Document,
                        children: vec![body_index],
                    });
                    registry.register_fields(id, paragraph);
                    registry.body_ids.push(Some(id));
                }
                BodyContent::Table(table) => {
                    registry.body_ids.push(None);
                    registry.collect_table(table, &WordStory::Document, &[body_index])
                }
                BodyContent::ContentControl(control) => {
                    registry.body_ids.push(None);
                    let mut items = Vec::new();
                    collect_body_control_layout_items(control, &[body_index], &mut items);
                    for item in items {
                        match item {
                            MainStoryLayoutItem::Paragraph(paragraph, children) => {
                                registry.insert(
                                    WordSourcePath {
                                        story: WordStory::Document,
                                        children,
                                    },
                                    paragraph,
                                );
                            }
                            MainStoryLayoutItem::Table(table, children) => {
                                registry.collect_table(table, &WordStory::Document, &children)
                            }
                        }
                    }
                }
                BodyContent::RawXml(_) => {
                    registry.body_ids.push(None);
                }
            }
        }

        let mut headers = input.headers.iter().collect::<Vec<_>>();
        headers.sort_unstable_by_key(|(relationship_id, _)| *relationship_id);
        for (relationship_id, header) in headers {
            let story = WordStory::Header {
                relationship_id: relationship_id.clone(),
            };
            if let Some(body) = input.story_bodies.get(&story) {
                registry.collect_story_body(body, &story);
                continue;
            }
            for paragraph_index in 0..header.paragraphs.len() {
                registry.insert(
                    WordSourcePath {
                        story: story.clone(),
                        children: vec![paragraph_index],
                    },
                    &header.paragraphs[paragraph_index],
                );
            }
        }

        let mut footers = input.footers.iter().collect::<Vec<_>>();
        footers.sort_unstable_by_key(|(relationship_id, _)| *relationship_id);
        for (relationship_id, footer) in footers {
            let story = WordStory::Footer {
                relationship_id: relationship_id.clone(),
            };
            if let Some(body) = input.story_bodies.get(&story) {
                registry.collect_story_body(body, &story);
                continue;
            }
            for paragraph_index in 0..footer.paragraphs.len() {
                registry.insert(
                    WordSourcePath {
                        story: story.clone(),
                        children: vec![paragraph_index],
                    },
                    &footer.paragraphs[paragraph_index],
                );
            }
        }

        for (story_kind, stream) in [
            (NoteStream::Footnote, input.footnotes.as_ref()),
            (NoteStream::Endnote, input.endnotes.as_ref()),
        ]
        .into_iter()
        .filter_map(|(story, stream)| stream.map(|stream| (story, stream)))
        {
            for (note_index, note) in stream.footnotes.iter().enumerate() {
                if stream.get_by_id(note.id).is_none() {
                    continue;
                }
                let story = match story_kind {
                    NoteStream::Footnote => WordStory::Footnote { id: note.id },
                    NoteStream::Endnote => WordStory::Endnote { id: note.id },
                };
                registry
                    .story_owner_children
                    .insert(story.clone(), vec![note_index]);
                if let Some(body) = input.story_bodies.get(&story) {
                    registry.collect_story_body(body, &story);
                    continue;
                }
                for paragraph_index in 0..note.paragraphs.len() {
                    registry.insert(
                        WordSourcePath {
                            story: story.clone(),
                            children: vec![paragraph_index],
                        },
                        &note.paragraphs[paragraph_index],
                    );
                }
            }
        }

        if let Some(error) = registry.preparation_error.take() {
            return Err(LayoutError::Layout(error));
        }
        Ok(registry)
    }

    fn collect_story_body(&mut self, body: &CT_Body, story: &WordStory) {
        for item in body_layout_items(body) {
            match item {
                MainStoryLayoutItem::Paragraph(paragraph, children) => {
                    self.insert(
                        WordSourcePath {
                            story: story.clone(),
                            children,
                        },
                        paragraph,
                    );
                }
                MainStoryLayoutItem::Table(table, children) => {
                    self.collect_table(table, story, &children)
                }
            }
        }
    }

    fn collect_table(&mut self, table: &CT_Tbl, story: &WordStory, prefix: &[usize]) {
        for (row, row_path) in table::layout_table_rows(table, prefix) {
            for (cell, cell_path) in table::layout_row_cells(row, &row_path) {
                for (content_index, content) in cell.content.iter().enumerate() {
                    let mut content_path = cell_path.clone();
                    content_path.push(content_index);
                    self.collect_cell_content(content, story, &content_path);
                }
            }
        }
    }

    fn collect_cell_content(&mut self, content: &CellContent, story: &WordStory, path: &[usize]) {
        match content {
            CellContent::Paragraph(paragraph) => self.insert(
                WordSourcePath {
                    story: story.clone(),
                    children: path.to_vec(),
                },
                paragraph,
            ),
            CellContent::Table(table) => self.collect_table(table, story, path),
            CellContent::ContentControl(control) => self.collect_cell_control(control, story, path),
        }
    }

    fn collect_cell_control(&mut self, control: &CT_Sdt, story: &WordStory, path: &[usize]) {
        for (content_index, content) in control.content.iter().enumerate() {
            let mut content_path = path.to_vec();
            content_path.push(content_index);
            match content {
                SdtContent::Paragraph(paragraph) => self.insert(
                    WordSourcePath {
                        story: story.clone(),
                        children: content_path,
                    },
                    paragraph,
                ),
                SdtContent::Table(table) => self.collect_table(table, story, &content_path),
                SdtContent::ContentControl(control) => {
                    self.collect_cell_control(control, story, &content_path)
                }
                SdtContent::Row(_)
                | SdtContent::Cell(_)
                | SdtContent::Run(_)
                | SdtContent::RawXml(_) => {}
            }
        }
    }

    pub(crate) fn record_table_omission(
        &self,
        story: &WordStory,
        prefix: &[usize],
        state: &mut NumberingState,
    ) {
        // Sorted physical paths bound each lookup to the omitted subtree.
        for (_, node) in self
            .navigation_paths
            .range((story.clone(), prefix.to_vec())..)
            .take_while(|((owner, children), _)| owner == story && children.starts_with(prefix))
        {
            if let Some(original) = self.navigation_sources.get(node) {
                let mut facts = original.clone();
                facts.omitted_by_table = true;
                facts.level = None;
                facts.style = None;
                state.record_navigation(*node, facts);
            }
        }
    }

    fn register_fields(&mut self, node: SourceNodeId, paragraph: &CT_P) {
        let text = match projected_paragraph_text(paragraph, self.revision_view) {
            Ok(text) => text,
            Err(error) => {
                self.preparation_error.get_or_insert_with(|| {
                    format!(
                        "source paragraph {} preparation failed: {error}",
                        node.get()
                    )
                });
                String::new()
            }
        };
        self.navigation_sources.insert(
            node,
            crate::ParagraphNavigation {
                omitted_by_table: false,
                level: None,
                suppressed_numbering: false,
                accepted_runs: project_paragraph_runs(paragraph, RevisionView::Accepted).len(),
                text,
                style: paragraph
                    .properties
                    .as_ref()
                    .and_then(|p| p.style_id.clone()),
            },
        );
        fn register(
            field: &oxoffice_docx_oxml::text::Field,
            node: SourceNodeId,
            index: &mut u32,
            result: &mut HashMap<FieldSource, Vec<u32>>,
        ) -> Vec<u32> {
            let own = *index;
            *index += 1;
            let mut inherited = Vec::new();
            for nested in field.nested_fields_in_source_order() {
                let child = *index;
                let descendants = register(nested, node, index, result);
                inherited.push(child);
                inherited.extend(descendants);
            }
            for nested in field.cached_fields_in_source_order() {
                register(nested, node, index, result);
            }
            result.insert(FieldSource { node, index: own }, inherited.clone());
            inherited
        }
        fn remember(
            registry: &mut SourceRegistry,
            field: &oxoffice_docx_oxml::text::Field,
            node: SourceNodeId,
            index: &mut u32,
            locked: bool,
            cached: bool,
            retain_raw: bool,
        ) {
            let source = FieldSource {
                node,
                index: *index,
            };
            let locked = locked || field.locked() == Some(true);
            let instruction = field.effective_instruction();
            if !locked && !cached && supported_styleref(&instruction) {
                registry
                    .styleref_requests
                    .insert(source, instruction.clone());
            }
            registry
                .field_source_contexts
                .insert(source, (field.effective_instruction_text(), locked, cached));
            if matches!(
                instruction.name.as_str(),
                "PAGE" | "NUMPAGES" | "SECTION" | "SECTIONPAGES" | "PAGEREF"
            ) {
                registry.page_fields.insert(
                    source,
                    (instruction, field.effective_display().into_owned()),
                );
            }

            if retain_raw && let Ok(Some((_, source_xml))) = field.source_replacement() {
                registry
                    .field_source_xml
                    .insert(source, source_xml.to_vec());
            }
            *index += 1;
            for nested in field.nested_fields_in_source_order() {
                remember(registry, nested, node, index, locked, cached, retain_raw);
            }
            for nested in field.cached_fields_in_source_order() {
                remember(registry, nested, node, index, locked, true, retain_raw);
            }
        }
        let is_text_box = matches!(
            self.nodes[(node.get() - 1) as usize].story,
            WordStory::TextBox { .. }
        );
        let mut index = 0;
        let mut raw_index = 0;
        for field in paragraph
            .source_runs()
            .into_iter()
            .flat_map(|run| &run.content)
            .filter_map(|content| match content {
                RunContent::Field(field) => Some(field),
                _ => None,
            })
        {
            register(field, node, &mut index, &mut self.field_descendants);
            remember(self, field, node, &mut raw_index, false, false, is_text_box);
        }
        self.register_text_boxes(node, paragraph);
    }

    pub(crate) fn bind_text_boxes(
        &self,
        block: &mut ParagraphBlock,
        paint: &mut WordParagraphPaint,
        owner: Option<SourceNodeId>,
    ) -> Result<()> {
        let Some(children) = owner.and_then(|owner| self.text_box_sources.get(&owner)) else {
            return Ok(());
        };
        if paint.anchors.len() != block.anchored.len() {
            return Err(LayoutError::Layout(
                "text box paint owner count mismatch".into(),
            ));
        }
        for ((drawing, binding), anchor_paint) in block
            .anchored
            .iter_mut()
            .zip(children)
            .zip(&mut paint.anchors)
        {
            if let block::AnchoredContent::Shape { text, .. } = &mut drawing.content {
                let WordAnchorPaint::Shape { paragraphs } = anchor_paint else {
                    return Err(LayoutError::Layout(
                        "text box paint owner variant mismatch".into(),
                    ));
                };
                if paragraphs.len() != text.len() {
                    return Err(LayoutError::Layout(
                        "text box paragraph paint count mismatch".into(),
                    ));
                }
                match binding {
                    TextBoxSourceBinding::Paragraphs(nodes) => {
                        for ((paragraph, paint), node) in text.iter_mut().zip(paragraphs).zip(nodes)
                        {
                            rebind_paragraph_source(paragraph, Some(*node))?;
                            paint.rebind_source(Some(*node))?;
                            self.bind_text_boxes(paragraph, paint, Some(*node))?;
                        }
                    }
                    TextBoxSourceBinding::Body(nodes) => {
                        let map = nodes
                            .iter()
                            .enumerate()
                            .map(|(index, node)| {
                                (
                                    SourceNodeId::new((index + 1) as u32)
                                        .expect("registered scoped source"),
                                    *node,
                                )
                            })
                            .collect();
                        for (paragraph, paint) in text.iter_mut().zip(paragraphs) {
                            remap_text_box_sources(paragraph, &map)?;
                            paint.remap_sources(&map)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn register_text_boxes(&mut self, owner: SourceNodeId, paragraph: &CT_P) {
        let path = self.nodes[(owner.get() - 1) as usize].clone();
        let part_name = match &path.story {
            WordStory::TextBox { part_name, .. } => Some(part_name.clone()),
            story => self.story_part_names.get(story).cloned(),
        };
        let Some(part_name) = part_name else {
            return;
        };
        let mut children = Vec::new();
        let mut stories = Vec::new();
        // Logical projected paths include accepted revisions. Physical cache
        // ownership comes from the independently bound source owner, not this index.
        for (run_index, projected) in project_paragraph_runs(paragraph, self.revision_view)
            .into_iter()
            .enumerate()
        {
            let run = projected.run;
            let drawings = run
                .content
                .iter()
                .filter_map(|content| match content {
                    RunContent::Drawing(drawing) => Some(drawing),
                    _ => None,
                })
                .chain(run.alt_drawings.iter());
            for (drawing_index, drawing) in drawings.enumerate() {
                let Some(anchor) = drawing.anchor.as_ref() else {
                    continue;
                };
                if anchor.chart_rel_id.is_some() || !anchor.embed_id.is_empty() {
                    children.push(TextBoxSourceBinding::Paragraphs(Vec::new()));
                    stories.push(None);
                    continue;
                }
                let Some(shape) = anchor.shape.as_ref() else {
                    continue;
                };
                let mut owner_children = match &path.story {
                    WordStory::TextBox { owner_children, .. } => {
                        let mut result = owner_children.clone();
                        result.extend(&path.children);
                        result
                    }
                    story => {
                        let mut result = self
                            .story_owner_children
                            .get(story)
                            .cloned()
                            .unwrap_or_default();
                        result.extend(&path.children);
                        result
                    }
                };
                owner_children.extend([run_index, drawing_index]);
                let story = WordStory::TextBox {
                    part_name: part_name.clone(),
                    owner_children,
                };
                if let Some(owner) = shape.source_text_box_owner {
                    self.text_box_owner_indices.insert(story.clone(), owner);
                }
                stories.push(Some(story.clone()));
                if let Some(body) = &shape.text_body {
                    let start = self.nodes.len();
                    self.collect_story_body(body, &story);
                    let nodes = (start..self.nodes.len())
                        .map(|index| {
                            SourceNodeId::new((index + 1) as u32).expect("registered source node")
                        })
                        .collect();
                    children.push(TextBoxSourceBinding::Body(nodes));
                    continue;
                }
                let mut nodes = Vec::new();
                for (index, paragraph) in shape.text.iter().enumerate() {
                    let child_path = WordSourcePath {
                        story: story.clone(),
                        children: vec![index],
                    };
                    self.insert(child_path.clone(), paragraph);
                    nodes.push(self.ids[&child_path]);
                }
                children.push(TextBoxSourceBinding::Paragraphs(nodes));
            }
        }
        self.text_box_sources.insert(owner, children);
        self.text_box_stories.insert(owner, stories);
    }

    /// The story of a paragraph's text box: the `drawing`-th anchored
    /// drawing of the paragraph `owner` names, counted as layout anchors
    /// them. `None` for a chart, a picture or no such drawing.
    pub(crate) fn text_box_story(&self, owner: SourceNodeId, drawing: usize) -> Option<&WordStory> {
        self.text_box_stories.get(&owner)?.get(drawing)?.as_ref()
    }

    fn insert(&mut self, path: WordSourcePath, paragraph: &CT_P) {
        if self.ids.contains_key(&path) {
            return;
        }
        let id = self.insert_node(path.clone());
        self.register_fields(id, paragraph);
        self.navigation_paths
            .insert((path.story.clone(), path.children.clone()), id);
        self.ids.insert(path, id);
    }

    fn insert_node(&mut self, path: WordSourcePath) -> SourceNodeId {
        let index = u32::try_from(self.nodes.len() + 1)
            .expect("a layout result cannot contain more than u32::MAX source paragraphs");
        let id = SourceNodeId::new(index).expect("source ids are one based");
        self.nodes.push(path);
        id
    }

    pub(crate) fn body_id(&self, body_index: usize) -> Option<SourceNodeId> {
        self.body_ids.get(body_index).copied().flatten()
    }

    pub(crate) fn id(&self, story: &WordStory, children: &[usize]) -> Option<SourceNodeId> {
        self.ids
            .get(&WordSourcePath {
                story: story.clone(),
                children: children.to_vec(),
            })
            .copied()
    }
}
