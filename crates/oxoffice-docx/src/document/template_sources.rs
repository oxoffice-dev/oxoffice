//! The main part as a template reads and writes it: record inputs, sources and the writer.

use super::*;

impl TemplateWordRecordInputs {
    pub(crate) fn records(&self) -> Vec<&[u8]> {
        self.blocks
            .values()
            .map(|block| block.bytes.as_slice())
            .chain(
                self.openings
                    .values()
                    .filter_map(|opening| opening.serialization_proof.as_deref()),
            )
            .collect()
    }

    pub(super) fn source_for_block<'a>(
        &'a self,
        operation: &mut oxoffice_docx_oxml::document::ScopedWordOperation<'a>,
        block: &Arc<BodyBlockScan>,
    ) -> Result<Option<oxoffice_docx_oxml::document::ScopedWordSource>> {
        self.blocks
            .get(&(Arc::as_ptr(block) as usize))
            .filter(|frozen| Arc::ptr_eq(frozen, block))
            .map(|frozen| {
                operation
                    .source_current_word_record(&frozen.bytes, 0..frozen.bytes.len())
                    .map_err(Error::from)
            })
            .transpose()
    }

    pub(super) fn source_for_part<'a>(
        &'a self,
        operation: &mut oxoffice_docx_oxml::document::ScopedWordOperation<'a>,
        part: &[u8],
    ) -> Result<Option<oxoffice_docx_oxml::document::ScopedWordSource>> {
        let Some(frozen) = self
            .openings
            .get(&(part.as_ptr() as usize, part.len()))
            .and_then(|opening| opening.serialization_proof.as_deref())
        else {
            return Ok(None);
        };
        operation
            .source_current_word_record(frozen, 0..frozen.len())
            .map(Some)
            .map_err(Error::from)
    }
}

impl Default for TemplateMainSources {
    fn default() -> Self {
        Self {
            source: None,
            reader_source: None,
            comments_reader_source: None,
            published_main: None,
            read_note_count: 0,
            notes_model_read: false,
            related: BTreeMap::new(),
            items: Vec::new(),
            blocks: Vec::new(),
            model_lineage_valid: true,
            first_visit: true,
            original_part: None,
        }
    }
}

impl TemplateMainSources {
    pub(crate) fn put_related_source(
        &mut self,
        part: &str,
        bytes: &Arc<Vec<u8>>,
        source: impl Into<Arc<oxoffice_docx_oxml::document::ScopedWordSource>>,
    ) {
        self.related
            .insert(part.to_owned(), (Arc::clone(bytes), source.into()));
    }

    pub(crate) fn read_document(
        &mut self,
        operation: &mut oxoffice_docx_oxml::document::ScopedWordOperation<'_>,
        source: Arc<oxoffice_docx_oxml::document::ScopedWordSource>,
    ) -> Result<CT_Document> {
        let document = operation.source_document(&source)?;
        self.model_lineage_valid = false;
        self.source = Some(Arc::clone(&source));
        self.reader_source = Some(source);
        Ok(document)
    }

    pub(crate) fn from_fragment_read(
        package: &OpcPackage,
        main_part: &str,
        footnotes: &oxoffice_docx_oxml::footnotes::CT_Footnotes,
        input: TemplateFragmentSources,
    ) -> Self {
        let mut sources = Self {
            source: Some(Arc::clone(&input.main)),
            published_main: package
                .get_part_shared(main_part)
                .map(|bytes| (Arc::clone(bytes), Arc::clone(&input.main))),
            reader_source: Some(input.main),
            model_lineage_valid: false,
            first_visit: false,
            ..Default::default()
        };
        // This is the actual opened model. With a paired resolved part, a
        // nonempty model can only come from the loader's successful scoped
        // reader: its malformed fallback returns an empty model instead.
        let paired_notes = package
            .get_part_rels(main_part)
            .and_then(|rels| {
                rels.items.iter().find(|rel| {
                    rel.rel_type == rel_types::FOOTNOTES && relationship_is_internal(rel)
                })
            })
            .is_some_and(|rel| {
                input
                    .related
                    .contains_key(&OpcPackage::resolve_rel_target(main_part, &rel.target))
            });
        if paired_notes && !footnotes.footnotes.is_empty() {
            sources.notes_model_read = true;
            sources.read_note_count = footnotes.footnotes.len();
        }
        sources.comments_reader_source = package
            .get_part_rels(main_part)
            .and_then(|rels| {
                rels.items.iter().find(|rel| {
                    rel.rel_type == rel_types::COMMENTS && relationship_is_internal(rel)
                })
            })
            .and_then(|rel| {
                input
                    .related
                    .get(&OpcPackage::resolve_rel_target(main_part, &rel.target))
            })
            .cloned();
        for (part, source) in input.related {
            if let Some(bytes) = package.get_part_shared(&part) {
                sources.related.insert(part, (Arc::clone(bytes), source));
            }
        }
        sources
    }

    pub(crate) fn related_source(
        &self,
        part: &str,
        bytes: &Arc<Vec<u8>>,
    ) -> Option<&Arc<oxoffice_docx_oxml::document::ScopedWordSource>> {
        let (stored, source) = self.related.get(part)?;
        Arc::ptr_eq(stored, bytes).then_some(source)
    }

    pub(crate) fn invalidate_model(&mut self) {
        self.source = None;
        self.reader_source = None;
        self.items.clear();
        self.blocks.clear();
        self.model_lineage_valid = false;
    }
}

pub(crate) struct TemplateMainWriter<'frozen, 'operation> {
    pub(crate) operation:
        &'operation mut oxoffice_docx_oxml::document::ScopedWordOperation<'frozen>,
    pub(crate) lineage: &'operation crate::template::TemplateWordLineage<'frozen>,
    pub(crate) sources: &'operation mut TemplateMainSources,
}

impl TemplateMainWriter<'_, '_> {
    pub(crate) fn take_related_source(
        &mut self,
        part: &str,
        bytes: &Arc<Vec<u8>>,
    ) -> Result<Option<Arc<oxoffice_docx_oxml::document::ScopedWordSource>>> {
        if let Some((stored, source)) = self.sources.related.remove(part)
            && Arc::ptr_eq(&stored, bytes)
        {
            return Ok(Some(source));
        }
        self.lineage
            .original_part_source(self.operation, part, bytes)
            .map(|source| source.map(Arc::new))
    }

    // Both actual producers are current: an owned new emission and a retained
    // shared source. Into keeps their publication path concrete and identical.
    pub(crate) fn document_source(
        &mut self,
        document: &CT_Document,
    ) -> Result<oxoffice_docx_oxml::document::ScopedWordSource> {
        if let Some(source) = &self.sources.reader_source {
            return Ok(self
                .operation
                .source_model_document_from_read(document, source)?);
        }
        let mut texts = Vec::new();
        let mut fields = Vec::new();
        let mut raw = Vec::new();
        if self.sources.model_lineage_valid {
            for (index, item) in document.body.content.iter().enumerate() {
                if let Some(inputs) = self.lineage.model_writer_inputs(index, item) {
                    texts.extend(inputs.texts);
                    fields.extend(inputs.fields);
                    raw.extend(inputs.raw);
                }
            }
        }
        Ok(self
            .operation
            .source_model_document(document, &texts, &fields, &raw)?)
    }
}
