//! Filling a template: its programs, repeated blocks and published sources.

use super::*;

impl Document {
    /// Render scalar and structural template tags from structured JSON data.
    ///
    /// Tags use `{{ path.to.value }}` syntax and may cross ordinary Word run
    /// boundaries. String, number and boolean leaves render as text, while
    /// `null` renders as an empty string. In paragraphs and text boxes, a tab
    /// in a value renders as a tab and a line ending as a line break; chart
    /// labels take the value as plain text. Missing paths, malformed tags, and
    /// object or array leaves return an error without changing the document.
    /// Dedicated main-body paragraphs and table rows may contain nested
    /// `{% for item in path %}` or `{% if path %}` blocks with their matching
    /// end markers. Loop paths require arrays and introduce lexical scopes.
    /// Conditions use JSON truthiness, where false, null, zero, and empty
    /// strings or collections are false. Comments are left as written: their
    /// tags are not rendered.
    ///
    /// This additive API is native-only. Python, WASM, and CLI binding surfaces
    /// remain unchanged and continue to preserve documents rendered here.
    pub fn render_template(&mut self, data: &serde_json::Value) -> Result<usize> {
        crate::template::render_grammar(self, data, false, None)
    }

    /// Render with the explicitly selected stable grammar version.
    pub fn render_template_versioned(
        &mut self,
        data: &serde_json::Value,
        grammar_version: u32,
    ) -> Result<usize> {
        if grammar_version != crate::template::TEMPLATE_GRAMMAR_VERSION {
            return Err(Error::Other(format!(
                "unsupported template grammar version {grammar_version}"
            )));
        }
        crate::template::render_grammar(self, data, true, None)
    }

    /// Render grammar1 with occurrence-local native text and run formatting.
    pub fn render_template_with_formatter(
        &mut self,
        data: &serde_json::Value,
        formatter: &mut dyn FnMut(
            &crate::template::TemplateFormatContext<'_>,
            &mut crate::MailMergeFormattedText,
        ) -> Result<()>,
    ) -> Result<usize> {
        crate::template::render_grammar(self, data, true, Some(formatter))
    }

    pub(crate) fn template_numbering_reference_exists(&self, num_id: u32, level: u32) -> bool {
        self.numbering
            .as_ref()
            .and_then(|numbering| numbering.get_abstract_num_for(num_id))
            .is_some_and(|abstract_numbering| {
                abstract_numbering
                    .levels
                    .iter()
                    .any(|candidate| candidate.ilvl == level)
            })
    }

    pub(crate) fn template_word_record_inputs(&self) -> TemplateWordRecordInputs {
        let scans = self
            .main_body_scans
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut input = TemplateWordRecordInputs::default();
        for record in scans.serialized.iter().chain(self.main_part_blocks.iter()) {
            for block in record.blocks.iter() {
                input
                    .blocks
                    .entry(Arc::as_ptr(block) as usize)
                    .or_insert_with(|| Arc::clone(block));
            }
        }
        if let Some(kept) = self.kept_main_part()
            && let Some(part) = kept.serialization_proof.as_ref()
        {
            input
                .openings
                .entry((part.as_ptr() as usize, part.len()))
                .or_insert_with(|| Arc::clone(&kept));
        }
        input
    }

    pub(crate) fn template_programs_scoped<'frozen>(
        &mut self,
        operation: &mut oxoffice_docx_oxml::document::ScopedWordOperation<'frozen>,
        lineage: &crate::template::TemplateWordLineage<'frozen>,
        sources: &mut TemplateMainSources,
    ) -> Result<Vec<crate::template::TemplateProgram>> {
        if sources.first_visit {
            sources.first_visit = false;
            sources.original_part = lineage
                .initial_main_part()
                .map(|(part, length)| (part.to_owned(), length));
        }
        let mut writer = TemplateMainWriter {
            operation,
            lineage,
            sources,
        };
        self.flush_to_package_in(Some(&mut writer))?;
        writer.sources.original_part = None;
        let main = &writer.sources.source;
        let mut roots = Vec::new();
        if let Some(source) = main {
            let story = StorySource {
                root_kind: StoryKind::Body,
                part_name: self.doc_part_name.clone(),
                xml: Cow::Borrowed(source.as_bytes()),
            };
            roots.extend(prepare_template_source_programs_in(
                &story,
                Some((writer.operation, source)),
            )?);
        } else {
            for source in self.story_sources()? {
                if source.root_kind == StoryKind::Body {
                    roots.extend(prepare_template_source_programs(&source)?);
                }
            }
        }
        for story in self.related_story_sources()? {
            if story.root_kind != StoryKind::Comment {
                let bytes = self
                    .package
                    .get_part_shared(&story.part_name)
                    .expect("actual related package part");
                if let Some(source) = writer.take_related_source(&story.part_name, bytes)? {
                    let actual = StorySource {
                        root_kind: story.root_kind,
                        part_name: story.part_name.clone(),
                        xml: Cow::Borrowed(source.as_bytes()),
                    };
                    roots.extend(prepare_template_source_programs_in(
                        &actual,
                        Some((writer.operation, &source)),
                    )?);
                    writer
                        .sources
                        .put_related_source(&story.part_name, bytes, source);
                } else {
                    roots.extend(prepare_template_source_programs(&story)?);
                }
            }
        }
        Ok(roots)
    }

    pub(crate) fn template_note_identities(
        &self,
        mut template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<Vec<(StoryKind, String, i32, usize)>> {
        let stories = if let Some(writer) = template.as_deref_mut() {
            let (main, record) = self.serialized_main_part_in(Some(writer))?;
            let layout = if let Some(record) = record {
                let (content, children) = record.body_spans()?;
                self.main_story_layout(&main, &content, &children, false)
                    .ok()
            } else {
                None
            };
            let mut sources = vec![StorySource {
                root_kind: StoryKind::Body,
                part_name: self.doc_part_name.clone(),
                xml: Cow::Owned(main),
            }];
            sources.extend(self.related_story_sources()?);
            Self::stories_in_sources(sources, layout)?
        } else {
            self.stories()?
        };
        let mut result = Vec::new();
        for kind in [StoryKind::Footnote, StoryKind::Endnote] {
            let Some(part) = self.note_part_name(kind)? else {
                continue;
            };
            let bytes = self
                .package
                .get_part_shared(&part)
                .ok_or_else(|| Error::Other(format!("template note part {part} is missing")))?;
            let notes = if let Some(writer) = template.as_deref_mut() {
                if let Some(source) = writer.take_related_source(&part, bytes)? {
                    let notes = writer.operation.source_notes(&source)?;
                    writer.sources.put_related_source(&part, bytes, source);
                    notes
                } else {
                    oxoffice_docx_oxml::footnotes::CT_Footnotes::from_xml(bytes)?
                }
            } else {
                oxoffice_docx_oxml::footnotes::CT_Footnotes::from_xml(bytes)?
            };
            let normal = notes
                .footnotes
                .iter()
                .filter(|note| note.note_type == oxoffice_docx_oxml::footnotes::NoteType::Normal)
                .collect::<Vec<_>>();
            let actual = stories
                .iter()
                .filter(|story| story.kind == kind && story.part_name == part)
                .collect::<Vec<_>>();
            if actual.len() != normal.len()
                || normal
                    .iter()
                    .map(|note| note.id)
                    .collect::<HashSet<_>>()
                    .len()
                    != normal.len()
            {
                return Err(Error::Other(format!(
                    "template note identity in {part} is ambiguous"
                )));
            }
            for (note, story) in normal.into_iter().zip(actual) {
                result.push((kind, part.clone(), note.id, story.owner_index));
            }
        }
        Ok(result)
    }

    pub(crate) fn template_fragment_program(
        part: &str,
        raw: &[u8],
        scope: &BTreeMap<String, String>,
        kind: StoryKind,
    ) -> Result<crate::template::TemplateProgram> {
        let closed = close_content_fragment_namespaces(raw, scope)?;
        let xml = if kind == StoryKind::Body {
            let mut xml =
                format!(r#"<w:document xmlns:w="{WORD_NAMESPACE}"><w:body>"#).into_bytes();
            xml.extend_from_slice(&closed);
            xml.extend_from_slice(b"</w:body></w:document>");
            xml
        } else {
            closed
        };
        let source = StorySource {
            root_kind: kind,
            part_name: part.into(),
            xml: Cow::Owned(xml),
        };
        let mut programs = prepare_template_source_programs(&source)?;
        if programs.len() != 1 {
            return Err(Error::Other(
                "template imported source has ambiguous root ownership".into(),
            ));
        }
        Ok(programs.remove(0))
    }

    pub(crate) fn template_fragment_program_from_source(
        part: &str,
        source: oxoffice_docx_oxml::document::ScopedWordSource,
        scope: &BTreeMap<String, String>,
        kind: StoryKind,
        operation: &mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>,
    ) -> Result<crate::template::TemplateProgram> {
        let closed = operation.close_source_namespaces(source, scope)?;
        let source = if kind == StoryKind::Body {
            let open = format!(r#"<w:document xmlns:w="{WORD_NAMESPACE}"><w:body>"#);
            operation.compose_sources(&[
                (None, 0..0, open.as_bytes()),
                (Some(&closed), 0..closed.as_bytes().len(), &[]),
                (None, 0..0, b"</w:body></w:document>"),
            ])?
        } else {
            closed
        };
        let actual = StorySource {
            root_kind: kind,
            part_name: part.to_owned(),
            xml: Cow::Borrowed(source.as_bytes()),
        };
        let mut programs =
            prepare_template_source_programs_in(&actual, Some((operation, &source)))?;
        if programs.len() != 1 {
            return Err(Error::Other(
                "template imported source has ambiguous root ownership".into(),
            ));
        }
        Ok(programs.remove(0))
    }

    pub(crate) fn template_require_cell_source(
        &mut self,
        source: oxoffice_docx_oxml::document::ScopedWordSource,
        scope: &BTreeMap<String, String>,
        operation: &mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>,
    ) -> Result<oxoffice_docx_oxml::document::ScopedWordSource> {
        let selected = operation.repeat_source(&source)?;
        let program = Self::template_fragment_program_from_source(
            &self.doc_part_name,
            selected,
            scope,
            StoryKind::TableCell,
            operation,
        )?;
        if program.items.iter().any(|item| {
            matches!(
                item.kind,
                crate::template::TemplateItemKind::Source(
                    StoryItemKind::Paragraph | StoryItemKind::Table | StoryItemKind::ContentControl
                )
            )
        }) {
            return Ok(source);
        }
        let source = operation.close_source_namespaces(source, scope)?;
        let owner = scan_story_owners(source.as_bytes(), StoryKind::TableCell)?
            .into_iter()
            .next()
            .ok_or_else(|| Error::Other("template cell owner disappeared".into()))?;
        let destination = ContentLocation::end(program.story);
        let (boundary, _, _) = validated_content_boundary(source.as_bytes(), &owner, &destination)?;
        Ok(operation.compose_sources(&[
            (Some(&source), 0..boundary, &[]),
            (None, 0..0, b"<w:p/>"),
            (Some(&source), boundary..source.as_bytes().len(), &[]),
        ])?)
    }

    pub(crate) fn template_publish_program_source<'frozen>(
        &mut self,
        program: &crate::template::TemplateProgram,
        replacement: oxoffice_docx_oxml::document::ScopedWordSource,
        operation: &mut oxoffice_docx_oxml::document::ScopedWordOperation<'frozen>,
        lineage: &crate::template::TemplateWordLineage<'frozen>,
        sources: &mut TemplateMainSources,
    ) -> Result<()> {
        if program.raw == replacement.as_bytes() {
            return Ok(());
        }
        let mut context = TemplateMainWriter {
            operation,
            lineage,
            sources,
        };
        self.flush_to_package_in(Some(&mut context))?;
        let part = &program.part;
        let source = if part == &self.doc_part_name {
            match context.sources.source.take() {
                Some(source) => source,
                None => Arc::new(context.document_source(&self.document)?),
            }
        } else {
            let bytes = self.package.get_part_shared(part).ok_or_else(|| {
                Error::Other("template owning part disappeared during staging".into())
            })?;
            context.take_related_source(part, bytes)?.ok_or_else(|| {
                Error::Other("template owning part lost its produced source".into())
            })?
        };
        let root_kind = if part == &self.doc_part_name {
            StoryKind::Body
        } else {
            self.related_story_sources()?
                .into_iter()
                .find(|story| &story.part_name == part)
                .map(|story| story.root_kind)
                .ok_or_else(|| {
                    Error::Other("template owning part disappeared during staging".into())
                })?
        };
        let owner = scan_story_owners(source.as_bytes(), root_kind)?
            .into_iter()
            .find(|owner| {
                owner.kind == program.story.kind && owner.owner_index == program.story.owner_index
            })
            .ok_or_else(|| {
                Error::Other("template physical owner disappeared during staging".into())
            })?;
        if source.as_bytes().get(owner.full.clone()) != Some(program.raw.as_slice()) {
            return Err(Error::Other(
                "template original owner changed during staged dependency allocation".into(),
            ));
        }
        if program.ranges.len() != 1 {
            return Err(Error::Other(
                "root paired template owner requires its qualified containing item".into(),
            ));
        }
        let updated = context.operation.compose_sources(&[
            (Some(&source), 0..owner.full.start, &[]),
            (Some(&replacement), 0..replacement.as_bytes().len(), &[]),
            (Some(&source), owner.full.end..source.as_bytes().len(), &[]),
        ])?;
        let retained = context.operation.repeat_source(&updated)?;
        let bytes = context.operation.finish_source(retained)?;
        set_story_source_xml_in(self, part, bytes, Some((&mut context, updated)))
    }

    pub(crate) fn claim_pending_drawing_ids(
        &mut self,
        pending: &mut HashSet<u32>,
        xml: &[u8],
    ) -> Result<()> {
        pending.extend(drawing_ids_in_xml(xml)?);
        self.retain_pending_drawing_ids(pending);
        Ok(())
    }

    pub(crate) fn retain_pending_drawing_ids(&mut self, pending: &HashSet<u32>) {
        self.identifiers.drawing_ids.extend(pending.iter().copied());
    }

    pub(crate) fn template_import_repeated_blocks(
        &mut self,
        source: &crate::template::TemplateProgram,
        selected: TemplateFragmentSelection,
        note_copy_sources: &mut Vec<(StoryKind, String, i32, String, i32)>,
        pending_drawings: &mut HashSet<u32>,
        evaluation: &mut crate::template::TemplateEvaluation<'_, '_>,
    ) -> Result<crate::template::TemplateProgram> {
        let part = &source.part;
        let scoped = matches!(&selected, TemplateFragmentSelection::Source(_));
        let (fragment, input) = match selected {
            TemplateFragmentSelection::Source(selected) => {
                let selected = *selected;
                let selected = if source.table_rows {
                    let parent = source.source.as_ref().ok_or_else(|| {
                        Error::Other("selected table has no source companion".into())
                    })?;
                    let start = source
                        .items
                        .first()
                        .ok_or_else(|| Error::Other("template table has no row source".into()))?
                        .range
                        .start
                        - source.ranges[0].start;
                    let end = source.items.last().unwrap().range.end - source.ranges[0].start;
                    let joined = evaluation.operation.compose_sources(&[
                        (Some(parent.as_ref()), 0..start, &[]),
                        (Some(&selected), 0..selected.as_bytes().len(), &[]),
                        (Some(parent.as_ref()), end..parent.as_bytes().len(), &[]),
                    ])?;
                    evaluation
                        .operation
                        .close_source_namespaces(joined, &source.scope)?
                } else {
                    selected
                };
                let (fragment, input) =
                    DocumentFragment::from_part_source(self, part, selected, evaluation)?;
                (fragment, Some(input))
            }
            TemplateFragmentSelection::Bytes(selected) => {
                let selected = if source.table_rows {
                    let mut raw = source.raw.clone();
                    let start = source
                        .items
                        .first()
                        .ok_or_else(|| Error::Other("template table has no row source".into()))?
                        .range
                        .start
                        - source.ranges[0].start;
                    let end = source.items.last().unwrap().range.end - source.ranges[0].start;
                    raw.splice(start..end, selected);
                    close_content_fragment_namespaces(&raw, &source.scope)?
                } else {
                    selected
                };
                (
                    DocumentFragment::from_part_content(self, part, selected)?,
                    None,
                )
            }
        };
        // Preparation inspects only published owners. Earlier repeated copies
        // still awaiting publication keep their actual reserved drawing ids.
        self.retain_pending_drawing_ids(pending_drawings);
        let (imported, imported_source) = if let Some(input) = input {
            let (xml, source) = crate::field::import_document_fragment_content(
                self,
                &fragment.package,
                fragment.include_final_section_properties,
                FragmentConflictPolicy::reuse_equivalent(),
                part,
                true,
                note_copy_sources,
                Some((evaluation, input)),
            )?;
            if let Some(source) = source {
                let source = package_authoritative_body_source(
                    &mut evaluation.operation,
                    source.as_ref(),
                    fragment.include_final_section_properties,
                    &BTreeMap::new(),
                )?;
                let retained = evaluation.operation.repeat_source(&source)?;
                (evaluation.operation.finish_source(retained)?, Some(source))
            } else {
                (
                    package_authoritative_body_fragment(
                        &xml,
                        fragment.include_final_section_properties,
                        &BTreeMap::new(),
                    )?,
                    None,
                )
            }
        } else {
            (
                fragment.import_content_staged(
                    self,
                    part,
                    FragmentConflictPolicy::reuse_equivalent(),
                    true,
                    note_copy_sources,
                )?,
                None,
            )
        };
        // Preserve newly allocated note/comment companions before any owner
        // replacement reacquires the actual physical source. Staging then
        // rebuilds drawing occupancy from the live package. The repeated copy
        // is not in that package yet, so keep the ids this import reserved.
        if scoped {
            self.prepare_staged_template_package(
                &mut evaluation.operation,
                &mut evaluation.lineage,
                &mut evaluation.sources,
            )?;
        } else {
            self.prepare_staged_package()?;
        }
        self.claim_pending_drawing_ids(pending_drawings, &imported)?;
        let mut program = if let Some(source) = imported_source {
            Self::template_fragment_program_from_source(
                part,
                source,
                &BTreeMap::new(),
                StoryKind::Body,
                &mut evaluation.operation,
            )?
        } else {
            Self::template_fragment_program(part, &imported, &BTreeMap::new(), StoryKind::Body)?
        };
        if source.table_rows {
            if program.items.len() != 1
                || program.items[0].children.len() != 1
                || !program.items[0].children[0].table_rows
            {
                return Err(Error::Other(
                    "import changed the selected table-row shell".into(),
                ));
            }
            return Ok(program.items.remove(0).children.remove(0));
        }
        Ok(program)
    }

    pub(crate) fn template_require_cell_paragraph(
        &mut self,
        raw: &mut Vec<u8>,
        scope: &BTreeMap<String, String>,
    ) -> Result<()> {
        let program =
            Self::template_fragment_program(&self.doc_part_name, raw, scope, StoryKind::TableCell)?;
        // A direct paragraph, table, or block control already satisfies the
        // cell. An empty cell still receives the paragraph Word requires.
        if program.items.iter().any(|item| {
            matches!(
                item.kind,
                crate::template::TemplateItemKind::Source(
                    StoryItemKind::Paragraph | StoryItemKind::Table | StoryItemKind::ContentControl
                )
            )
        }) {
            return Ok(());
        }
        *raw = close_content_fragment_namespaces(raw, scope)?;
        let owner = scan_story_owners(raw, StoryKind::TableCell)?
            .into_iter()
            .next()
            .ok_or_else(|| Error::Other("template cell owner disappeared".into()))?;
        let destination = ContentLocation::end(program.story);
        let (boundary, _, _) = validated_content_boundary(raw, &owner, &destination)?;
        insert_story_fragment(raw, &owner, boundary, b"<w:p/>".to_vec())
    }

    pub(crate) fn template_publish_program(
        &mut self,
        program: &crate::template::TemplateProgram,
        replacement: Vec<u8>,
    ) -> Result<()> {
        if program.raw == replacement {
            return Ok(());
        }
        // Allocation already canonicalized identifiers. Another prepare here
        // renumbers the owner under the snapshot and makes the equality proof
        // fail. Flush the model, then require the selected owner bytes.
        self.flush_to_package()?;
        // Reacquire the physical part and owner, then prove the exact selected
        // original source instead of reusing a stale caller fingerprint.
        let source = self
            .story_sources()?
            .into_iter()
            .find(|source| source.part_name == program.part)
            .ok_or_else(|| {
                Error::Other("template owning part disappeared during staging".into())
            })?;
        let owner = scan_story_owners(&source.xml, source.root_kind)?
            .into_iter()
            .find(|owner| {
                owner.kind == program.story.kind && owner.owner_index == program.story.owner_index
            })
            .ok_or_else(|| {
                Error::Other("template physical owner disappeared during staging".into())
            })?;
        let part = source.part_name.clone();
        let mut xml = source.xml.into_owned();
        if xml[owner.full.clone()] != program.raw {
            return Err(Error::Other(
                "template original owner changed during staged dependency allocation".into(),
            ));
        }
        if program.ranges.len() != 1 {
            return Err(Error::Other(
                "root paired template owner requires its qualified containing item".into(),
            ));
        }
        xml.splice(owner.full, replacement);
        set_story_source_xml(self, &part, xml)
    }

    pub(crate) fn template_raw_sources_scoped<'frozen>(
        &self,
        operation: &mut oxoffice_docx_oxml::document::ScopedWordOperation<'frozen>,
        lineage: &crate::template::TemplateWordLineage<'frozen>,
        sources: &mut TemplateMainSources,
    ) -> Result<Vec<(String, Arc<oxoffice_docx_oxml::document::ScopedWordSource>)>> {
        let mut context = TemplateMainWriter {
            operation,
            lineage,
            sources,
        };
        let mut parts = Vec::new();
        for (part, _) in self.raw_text_bearing_part_names() {
            let source = if part == self.doc_part_name {
                Arc::new(context.document_source(&self.document)?)
            } else {
                let Some(bytes) = self.package.get_part_shared(&part) else {
                    continue;
                };
                context.take_related_source(&part, bytes)?.ok_or_else(|| {
                    Error::Other(format!(
                        "template part has no retained producer source: {part}"
                    ))
                })?
            };
            parts.push((part, source));
        }
        Ok(parts)
    }

    pub(crate) fn template_retain_raw_source(
        &self,
        part: &str,
        source: oxoffice_docx_oxml::document::ScopedWordSource,
        sources: &mut TemplateMainSources,
    ) -> Result<()> {
        if part != self.doc_part_name {
            let bytes = self
                .package
                .get_part_shared(part)
                .ok_or_else(|| Error::Other("template source part disappeared".into()))?;
            if bytes.as_slice() != source.as_bytes() {
                return Err(Error::Other(
                    "template source part changed before retention".into(),
                ));
            }
            sources
                .related
                .insert(part.to_owned(), (Arc::clone(bytes), Arc::new(source)));
        }
        Ok(())
    }

    pub(crate) fn template_publish_scalar_source<'frozen>(
        &mut self,
        part: &str,
        original: &[u8],
        source: oxoffice_docx_oxml::document::ScopedWordSource,
        operation: &mut oxoffice_docx_oxml::document::ScopedWordOperation<'frozen>,
        lineage: &crate::template::TemplateWordLineage<'frozen>,
        sources: &mut TemplateMainSources,
    ) -> Result<()> {
        // The caller supplies the actual output of the selected part producer.
        // Equality here checks its publication precondition, not input origin.
        let mut context = TemplateMainWriter {
            operation,
            lineage,
            sources,
        };
        let matches = if part == self.doc_part_name {
            context.document_source(&self.document)?.as_bytes() == original
        } else {
            self.package.get_part(part) == Some(original)
        };
        if !matches {
            return Err(Error::Other(format!(
                "template chart source changed during staging: {part}"
            )));
        }
        let retained = context.operation.repeat_source(&source)?;
        let xml = context.operation.finish_source(retained)?;
        set_story_source_xml_in(self, part, xml, Some((&mut context, source)))
    }

    pub(crate) fn template_chart_parts(&self) -> Vec<(String, Vec<u8>)> {
        self.chart_part_names()
            .into_iter()
            .filter_map(|part| self.package.get_part(&part).map(|xml| (part, xml.to_vec())))
            .collect()
    }

    pub(crate) fn template_publish_scalar_part(
        &mut self,
        part: &str,
        original: &[u8],
        replacement: Vec<u8>,
    ) -> Result<()> {
        let current = if part == self.doc_part_name {
            self.document.to_xml()?
        } else {
            self.package
                .get_part(part)
                .ok_or_else(|| {
                    Error::Other(format!(
                        "template chart source changed during staging: {part}"
                    ))
                })?
                .to_vec()
        };
        if current.as_slice() != original {
            return Err(Error::Other(format!(
                "template chart source changed during staging: {part}"
            )));
        }
        if self
            .raw_text_bearing_part_names()
            .iter()
            .any(|(name, _)| name == part)
        {
            set_story_source_xml(self, part, replacement)
        } else {
            self.package.set_part(part, replacement);
            Ok(())
        }
    }

    pub(crate) fn commit_template(&mut self, mut candidate: Self) {
        candidate.invalidate_layout();
        self.commit_staged_mutation(candidate);
    }
}
