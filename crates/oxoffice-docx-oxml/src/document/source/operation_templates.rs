//! Scoped operations on templates: inputs, data, fields and replaceable texts.

use super::*;

impl<'a> ScopedWordOperation<'a> {
    /// Freeze a template root without inspecting or crediting its descendants.
    pub fn with_template_inputs(
        original_parts: Vec<(&'a str, &'a [u8])>,
        current_paragraphs: Vec<&'a CT_P>,
        data: &'a serde_json::Value,
    ) -> Result<Self> {
        let mut operation = Self::with_inputs(original_parts, current_paragraphs)?;
        operation
            .template_datums
            .insert(std::ptr::from_ref(data) as usize, data);
        Ok(operation)
    }

    /// Freeze Word input membership without selecting or crediting its text.
    pub fn with_template_word_inputs(
        original_parts: Vec<(&'a str, &'a [u8])>,
        current_paragraphs: Vec<&'a CT_P>,
        current_bodies: Vec<&'a CT_Body>,
        current_texts: Vec<&'a crate::text::CT_Text>,
        current_fields: Vec<&'a crate::text::Field>,
        mut current_word_sources: Vec<&'a [u8]>,
        data: &'a serde_json::Value,
    ) -> Result<Self> {
        let mut operation = Self::with_template_inputs(original_parts, current_paragraphs, data)?;
        operation.current_texts = current_texts
            .into_iter()
            .map(|text| (std::ptr::from_ref(text) as usize, text))
            .collect();
        operation
            .field_work
            .template_field_anchor(&operation.identity);
        operation.current_fields = current_fields
            .into_iter()
            .map(|field| (std::ptr::from_ref(field) as usize, field))
            .collect();
        let roots = current_bodies
            .into_iter()
            .map(WordOwner::Body)
            .chain(
                operation
                    .current_paragraphs
                    .values()
                    .copied()
                    .map(WordOwner::Paragraph),
            )
            .chain(
                operation
                    .current_fields
                    .values()
                    .copied()
                    .map(WordOwner::Field),
            )
            .collect();
        operation.freeze_template_model_membership(roots, &mut current_word_sources);
        // Identity is the frozen live backing view, never contents. Keeping
        // overlapping views is harmless: Word origins use physical intervals.
        let mut seen = std::collections::HashSet::new();
        operation.current_word_sources = current_word_sources
            .into_iter()
            .filter(|record| seen.insert((record.as_ptr() as usize, record.len())))
            .collect();
        operation.current_word_source_keys = seen;
        let mut ranges = operation
            .current_word_sources
            .iter()
            .filter(|record| !record.is_empty())
            .map(|record| {
                let start = record.as_ptr() as usize;
                let end = start.checked_add(record.len()).ok_or_else(|| {
                    OxmlError::InvalidValue("frozen Word record position overflow".into())
                })?;
                Ok((start, end))
            })
            .collect::<Result<Vec<_>>>()?;
        ranges.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for (start, end) in ranges {
            if let Some(previous) = merged.last_mut()
                && start < previous.1
            {
                previous.1 = previous.1.max(end);
            } else {
                merged.push((start, end));
            }
        }
        operation.current_word_views.extend(merged);
        Ok(operation)
    }

    // Enrollment borrows existing physical state only. The same resource graph
    // includes stored caches and original control runs without selecting them.
    fn freeze_template_model_membership(
        &mut self,
        mut pending: Vec<WordOwner<'a>>,
        records: &mut Vec<&'a [u8]>,
    ) {
        use crate::content_control::SdtContent;
        use crate::text::RunContent;
        let mut seen = std::collections::HashSet::new();
        while let Some(owner) = pending.pop() {
            if !seen.insert(owner.key()) {
                continue;
            }
            match owner {
                WordOwner::Body(body) => {
                    for content in &body.content {
                        if let BodyContent::RawXml(raw) = content {
                            records.push(raw);
                        }
                    }
                }
                WordOwner::Paragraph(paragraph) => {
                    self.current_paragraphs
                        .insert(std::ptr::from_ref(paragraph) as usize, paragraph);
                    records.extend(paragraph.extra_xml.iter().map(|(_, raw)| raw.as_slice()));
                    for ruby in &paragraph.rubies {
                        records.extend(ruby.raw_xml.iter().map(Vec::as_slice));
                    }
                }
                WordOwner::Run(run) => {
                    for content in &run.content {
                        if let RunContent::Text(text) | RunContent::DeletedText(text) = content {
                            self.current_texts
                                .insert(std::ptr::from_ref(text) as usize, text);
                        }
                    }
                    records.extend(run.extra_xml.iter().map(Vec::as_slice));
                }
                WordOwner::Field(field) => {
                    self.current_fields
                        .insert(std::ptr::from_ref(field) as usize, field);
                }
                WordOwner::Control(control) => {
                    records.extend(control.retained_resource_run_xml().map(|(_, raw)| raw));
                    for content in &control.content {
                        if let SdtContent::RawXml(raw) = content {
                            records.push(raw);
                        }
                    }
                }
                WordOwner::Table(table) => {
                    records.extend(table.extra_xml.iter().map(|(_, raw)| raw.as_slice()));
                }
                WordOwner::Row(row) => {
                    records.extend(row.extra_xml.iter().map(|(_, raw)| raw.as_slice()));
                    records.extend(row.table_property_exception.as_deref());
                }
                WordOwner::Cell(cell) => {
                    records.extend(cell.extra_xml.iter().map(|(_, raw)| raw.as_slice()));
                }
                WordOwner::Revision(revision) => records.push(revision.retained_xml()),
                WordOwner::Drawing(drawing) => {
                    if let Some(inline) = &drawing.inline {
                        records.extend(inline.raw_xml.as_deref());
                    }
                }
                WordOwner::Anchor(anchor) => records.extend(anchor.raw_xml.as_deref()),
            }
            pending.extend(owner.resource_children());
        }
    }

    /// Select an actual child of an already proved template input.
    pub fn template_datum_key(
        &mut self,
        parent: &'a serde_json::Value,
        key: &str,
    ) -> Result<Option<&'a serde_json::Value>> {
        self.require_template_datum(parent)?;
        let value = parent.as_object().and_then(|object| object.get(key));
        if let Some(value) = value {
            self.template_datums
                .insert(std::ptr::from_ref(value) as usize, value);
        }
        Ok(value)
    }

    /// Select an actual array slot without visiting other elements.
    pub fn template_datum_index(
        &mut self,
        parent: &'a serde_json::Value,
        index: usize,
    ) -> Result<Option<&'a serde_json::Value>> {
        self.require_template_datum(parent)?;
        let value = parent.as_array().and_then(|array| array.get(index));
        if let Some(value) = value {
            self.template_datums
                .insert(std::ptr::from_ref(value) as usize, value);
        }
        Ok(value)
    }

    fn require_template_datum(&self, value: &serde_json::Value) -> Result<()> {
        if !self
            .template_datums
            .get(&(std::ptr::from_ref(value) as usize))
            .is_some_and(|original| std::ptr::eq(*original, value))
        {
            return Err(OxmlError::InvalidValue(
                "template datum is not a selected input".into(),
            ));
        }
        Ok(())
    }

    /// Count canonical scalar text without constructing it, then admit this
    /// actual input occurrence once. Containers do not supply operand credit.
    pub fn template_scalar_input_bytes(&mut self, value: &'a serde_json::Value) -> Result<usize> {
        self.require_template_datum(value)?;
        let bytes = template_scalar_bytes(value)?;
        let key = std::ptr::from_ref(value) as usize;
        if !self.template_copy_work.inputs.contains(&key) {
            let allowance = bytes
                .checked_add(64)
                .and_then(|bytes| bytes.checked_mul(W_NS.len() + 64))
                .and_then(|bytes| self.template_copy_work.allowance.checked_add(bytes))
                .ok_or_else(|| {
                    OxmlError::InvalidValue("template text-copy allowance overflow".into())
                })?;
            self.template_copy_work.inputs.insert(key);
            self.template_copy_work.allowance = allowance;
        }
        Ok(bytes)
    }

    /// Select the text of one genuine successful, validated external callback.
    /// The native event owns this delivery flag and shares it through replay.
    /// JSON operand overhead and Field current input are separate roles.
    pub fn register_template_callback_input(
        &mut self,
        value: &str,
        delivered: &std::cell::Cell<bool>,
    ) -> Result<()> {
        if self.template_copy_work.refused {
            return Err(OxmlError::InvalidValue(
                "template text copy exceeds selected input work".into(),
            ));
        }
        if delivered.get() {
            return Ok(());
        }
        let allowance = value
            .len()
            .checked_mul(W_NS.len() + 64)
            .and_then(|bytes| self.template_copy_work.allowance.checked_add(bytes))
            .ok_or_else(|| {
                OxmlError::InvalidValue("template callback allowance overflow".into())
            })?;
        self.template_copy_work.allowance = allowance;
        delivered.set(true);
        Ok(())
    }

    /// Debit a real upcoming text allocation or copy before growing its buffer.
    pub fn reserve_template_text_copy(&mut self, bytes: usize) -> Result<()> {
        if self.field_work.resource_refused() {
            return Err(self.field_work.writer_error(OxmlError::InvalidValue(
                "scoped operation resource admission refused".into(),
            )));
        }
        self.template_copy_work.reserve(bytes)
    }
}

impl<'a> ScopedWordOperation<'a> {
    pub(super) fn validate_template_word_origins(
        &self,
        text_origins: &[(&crate::text::CT_Text, &'a crate::text::CT_Text)],
        field_origins: &[(&crate::text::Field, &'a crate::text::Field)],
        raw_origins: &[(&[u8], &'a [u8])],
    ) -> Result<()> {
        let mut texts = std::collections::HashSet::new();
        for (current, frozen) in text_origins {
            let key = std::ptr::from_ref(*frozen) as usize;
            if !self
                .current_texts
                .get(&key)
                .is_some_and(|input| std::ptr::eq(*input, *frozen))
                || !texts.insert(std::ptr::from_ref(*current) as usize)
            {
                return Err(OxmlError::InvalidValue(
                    "invalid frozen Word Text relationship".into(),
                ));
            }
        }
        let mut fields = std::collections::HashSet::new();
        for (current, frozen) in field_origins {
            let key = std::ptr::from_ref(*frozen) as usize;
            if !self
                .current_fields
                .get(&key)
                .is_some_and(|input| std::ptr::eq(*input, *frozen))
                || !fields.insert(std::ptr::from_ref(*current) as usize)
            {
                return Err(OxmlError::InvalidValue(
                    "invalid frozen Word Field relationship".into(),
                ));
            }
        }
        let mut raw_keys = std::collections::HashSet::new();
        for (current, frozen) in raw_origins {
            let start = frozen.as_ptr() as usize;
            let end = start.checked_add(frozen.len()).ok_or_else(|| {
                OxmlError::InvalidValue("frozen Word record position overflow".into())
            })?;
            if current.len() != frozen.len()
                || !raw_keys.insert((current.as_ptr() as usize, current.len()))
                || !(self
                    .current_word_views
                    .range(..=start)
                    .next_back()
                    .is_some_and(|(_, limit)| end <= *limit)
                    || self
                        .current_word_source_keys
                        .contains(&(start, frozen.len())))
            {
                return Err(OxmlError::InvalidValue(
                    "invalid frozen Word retained-record relationship".into(),
                ));
            }
        }
        Ok(())
    }

    pub(super) fn template_model_field_origins<'current>(
        &self,
        mut pending: Vec<WordOwner<'current>>,
        provided: &[(&'current crate::text::Field, &'a crate::text::Field)],
    ) -> Vec<(&'current crate::text::Field, &'a crate::text::Field)> {
        let mut origins = provided.to_vec();
        let mut selected = origins
            .iter()
            .map(|(current, _)| std::ptr::from_ref(*current) as usize)
            .collect::<std::collections::HashSet<_>>();
        while let Some(owner) = pending.pop() {
            if let WordOwner::Field(field) = owner
                && !selected.contains(&(std::ptr::from_ref(field) as usize))
                && let Some(frozen) = self
                    .field_work
                    .prepared_template_field_origin(field)
                    .and_then(|key| self.current_fields.get(&key))
            {
                origins.push((field, *frozen));
                selected.insert(std::ptr::from_ref(field) as usize);
            }
            pending.extend(owner.resource_children());
        }
        origins
    }

    pub(super) fn register_template_model_fields(
        &mut self,
        mut pending: Vec<WordOwner<'_>>,
        field_origins: &[(&crate::text::Field, &'a crate::text::Field)],
    ) -> Result<()> {
        let pairs = field_origins
            .iter()
            .map(|(current, frozen)| (std::ptr::from_ref(*current) as usize, *frozen))
            .collect::<std::collections::HashMap<_, _>>();
        let mut selected = Vec::new();
        while let Some(owner) = pending.pop() {
            if let WordOwner::Field(field) = owner
                && let Some(frozen) = pairs.get(&(std::ptr::from_ref(field) as usize))
            {
                selected.push(*frozen);
            }
            pending.extend(owner.resource_children());
        }
        self.field_work.register_model_fields(selected)
    }

    /// Preserve actual early Native preparation order under this operation.
    pub fn prepare_template_field(
        &mut self,
        current: &crate::text::Field,
        frozen: &'a crate::text::Field,
        normalize: bool,
    ) -> Result<crate::text::Field> {
        self.field_work.template_field_anchor(&self.identity);
        if self.field_work.resource_refused() {
            return Err(self.field_work.writer_error(OxmlError::InvalidValue(
                "scoped operation resource admission refused".into(),
            )));
        }
        let key = std::ptr::from_ref(frozen) as usize;
        if !self.current_fields.contains_key(&key) {
            return Err(OxmlError::InvalidValue(
                "Field is not a frozen template input".into(),
            ));
        }
        if normalize {
            self.field_work.register_model_fields(vec![frozen])?;
        }
        let lease = self
            .field_work
            .template_word_source_context(&[], &[(current, frozen)], &[])?;
        let prepared = current.prepare_template_in(Some(frozen), normalize, lease.work)?;
        drop(lease);
        Ok(prepared)
    }

    /// Install only operation-prepared Field slots in the actual accepted run.
    /// The run clone is owned here so ordinary Text keeps its clone producer.
    #[doc(hidden)]
    pub fn install_template_prepared_fields(
        &mut self,
        paragraph: &mut CT_P,
        path: &crate::text::AcceptedRunPath,
        fields: Vec<(usize, crate::text::Field)>,
        frozen: Option<&'a CT_P>,
    ) -> Result<bool> {
        use crate::text::RunContent;
        if self.field_work.resource_refused() {
            return Err(self.field_work.writer_error(OxmlError::InvalidValue(
                "scoped operation resource admission refused".into(),
            )));
        }
        if let Some(frozen) = frozen
            && !self
                .current_paragraphs
                .contains_key(&(std::ptr::from_ref(frozen) as usize))
        {
            return Err(OxmlError::InvalidValue(
                "paragraph is not a frozen template input".into(),
            ));
        }
        let mut owner = WordOwner::Paragraph(paragraph);
        for step in path.segments() {
            use crate::text::AcceptedRunPathSegment as S;
            owner = match (owner, step) {
                (WordOwner::Paragraph(p), S::Revision(index)) => {
                    let Some((_, _, r)) = p.revisions.get(*index) else {
                        return Ok(false);
                    };
                    r.validate_template_install_context(&self.field_work, frozen.is_none())?;
                    let Some(p) = r.content_paragraph() else {
                        return Ok(false);
                    };
                    WordOwner::Paragraph(p)
                }
                (WordOwner::Control(c), S::Revision(index)) => {
                    let Some((_, r)) = c.revisions().get(*index) else {
                        return Ok(false);
                    };
                    r.validate_template_install_context(&self.field_work, frozen.is_none())?;
                    let Some(p) = r.content_paragraph() else {
                        return Ok(false);
                    };
                    WordOwner::Paragraph(p)
                }
                (WordOwner::Paragraph(p), S::ContentControl(index)) => {
                    let Some((_, _, _, c)) = p.content_controls.get(*index) else {
                        return Ok(false);
                    };
                    WordOwner::Control(c)
                }
                (WordOwner::Control(c), S::ContentControl(index)) => {
                    let Some(crate::content_control::SdtContent::ContentControl(c)) =
                        c.content.get(*index)
                    else {
                        return Ok(false);
                    };
                    WordOwner::Control(c)
                }
                (_, S::Run(_)) => break,
                _ => return Ok(false),
            };
        }
        let Some(run) = paragraph.accepted_run(path) else {
            return Ok(false);
        };
        let mut selected = std::collections::HashSet::new();
        for (slot, field) in &fields {
            if !selected.insert(*slot)
                || !matches!(run.content.get(*slot), Some(RunContent::Field(_)))
                || self
                    .field_work
                    .prepared_template_field_origin(field)
                    .is_none()
            {
                return Err(OxmlError::InvalidValue(
                    "invalid operation-prepared Field slot".into(),
                ));
            }
        }
        if fields.is_empty() {
            return Ok(false);
        }
        let mut replacement = run.clone();
        for (slot, field) in fields {
            replacement.content[slot] = RunContent::Field(field);
        }
        let emission = self.field_work.field_xml_emission();
        if let Some(frozen) = frozen {
            let (texts, field_pairs, raws) = template_paragraph_clone_pairs(frozen, paragraph)?;
            self.validate_template_word_origins(&texts, &field_pairs, &raws)?;
            let lease =
                self.field_work
                    .template_word_source_context(&texts, &field_pairs, &raws)?;
            // Only the selected run is cloned above. The existing nested
            // installer keeps its own owner candidate alive through refresh.
            paragraph.replace_accepted_run_segments_in(
                path.segments(),
                replacement,
                Some(lease.work),
                Some(&emission),
                true,
            )
        } else {
            paragraph.replace_accepted_run_segments_in(
                path.segments(),
                replacement,
                Some(&mut self.field_work),
                Some(&emission),
                true,
            )
        }
    }

    /// Prepare a Field returned by this operation's actual successful model read.
    /// The caller keeps the paired returned model and Source together. Derived
    /// fields forward their existing source facts and never become frozen input.
    #[doc(hidden)]
    pub fn prepare_template_field_from_read(
        &mut self,
        current: &crate::text::Field,
        source: &ScopedWordSource,
        normalize: bool,
    ) -> Result<crate::text::Field> {
        self.check_source(source)?;
        self.field_work.template_field_anchor(&self.identity);
        if self.field_work.resource_refused() {
            return Err(self.field_work.writer_error(OxmlError::InvalidValue(
                "scoped operation resource admission refused".into(),
            )));
        }
        if source
            .read_facts
            .lock()
            .expect("returned Word facts poisoned")
            .is_none()
            || self
                .field_origins
                .lock()
                .expect("field origin registry poisoned")
                .source_map(current)
                .is_none()
        {
            return Err(OxmlError::InvalidValue(
                "Field has no actual scoped model read".into(),
            ));
        }
        let fields = self.template_model_field_origins(vec![WordOwner::Field(current)], &[]);
        if normalize {
            self.field_work.register_original_fields(vec![current])?;
            self.register_template_model_fields(vec![WordOwner::Field(current)], &fields)?;
        }
        let lease = self
            .field_work
            .template_word_source_context(&[], &fields, &[])?;
        let prepared = current.prepare_template_in(None, normalize, lease.work)?;
        drop(lease);
        Ok(prepared)
    }
}

impl<'a> ScopedWordOperation<'a> {
    /// Read actual grammar stretches, selecting only the TextWalk's inputs.
    pub fn template_replaceable_texts(&mut self, source: &ScopedWordSource) -> Result<Vec<String>> {
        self.check_source(source)?;
        let mut extras = Vec::new();
        let mut texts = Vec::new();
        let word_map = source
            .source_map
            .with_word_source(&source.word_source_map, &source.word_events)?;
        let mut nested_maps = crate::text::WordBlockSourceMaps::default();
        let mut paragraph = self.parse_source_paragraph_with_properties(
            source,
            Some(&mut extras),
            Some(&mut texts),
            Some(&word_map),
            None,
            Some(&mut nested_maps),
        )?;
        let nested = returned_block_inputs(WordOwner::Paragraph(&paragraph), &nested_maps)?
            .0
            .into_iter()
            .map(|(text, event)| (std::ptr::from_ref(text) as usize, event))
            .collect();
        let mut roots = paragraph.writer_field_roots();
        roots.extend(
            crate::text::ordered_normalization_children(&paragraph)?
                .into_iter()
                .map(|(field, _)| field),
        );
        self.field_work.register_original_fields(roots)?;
        let mut read = TemplateWordRead {
            map: &word_map,
            texts: &texts,
            work: &mut self.template_copy_work,
            nested: Some(&nested),
        };
        crate::placeholder::template_replaceable_texts_in(
            &mut paragraph,
            &extras,
            &mut self.field_work,
            &mut read,
        )
    }

    /// Discover or replace original raw-textbox literals with actual scoped inputs.
    #[doc(hidden)]
    #[allow(clippy::type_complexity)] // Exact approved literal/dependency and carrier tuples.
    pub fn replace_original_template_literals(
        &mut self,
        source: &ScopedWordSource,
        excluded: &[std::ops::Range<usize>],
        replacements: &[(&str, &str, &[&'a serde_json::Value])],
    ) -> Result<(ScopedWordSource, usize, Vec<String>)> {
        self.check_source(source)?;
        let map = source
            .source_map
            .with_word_source(&source.word_source_map, &source.word_events)?
            .with_publication_collector(&source.publication);
        let input = WordFieldInput {
            initial_delivery: source.initial_delivery,
            map: std::sync::Arc::new(map),
            registry: std::sync::Arc::clone(&self.field_origins),
        };
        let (bytes, count, texts, source_map) =
            crate::placeholder::replace_original_template_literals_in(
                &source.bytes,
                excluded,
                replacements,
                &input,
                &mut self.field_work,
                &mut self.template_copy_work,
                &self.template_datums,
            )?;
        let constructors = if replacements.is_empty() {
            source.constructors.clone()
        } else {
            Some(admit_word_source(&bytes, &[], None, None)?.constructors)
        };
        Ok((
            ScopedWordSource {
                publication: source_map.publication_output(),
                initial_delivery: false,
                word_source_map: WordSourceMap::generated(bytes.len()),
                bytes,
                source_map,
                word_events: Vec::new(),
                constructors,
                read_facts: Default::default(),
                operation: std::sync::Arc::clone(&self.identity),
            },
            count,
            texts,
        ))
    }

    /// Read the existing direct and recursive grammar views of one checked row.
    #[doc(hidden)]
    #[allow(clippy::type_complexity)] // Exact approved row view tuple.
    pub fn template_row_replaceable_texts(
        &mut self,
        source: &ScopedWordSource,
    ) -> Result<(Vec<String>, Vec<usize>, Vec<usize>, bool)> {
        self.check_source(source)?;
        const OPEN: &[u8] =
            b"<w:tbl xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">";
        const CLOSE: &[u8] = b"</w:tbl>";
        let length = OPEN
            .len()
            .checked_add(source.bytes.len())
            .and_then(|length| length.checked_add(CLOSE.len()))
            .ok_or_else(|| OxmlError::InvalidValue("wrapped row source length overflow".into()))?;
        let wrapped = self.compose_sources(&[
            (None, 0..0, OPEN),
            (Some(source), 0..source.bytes.len(), &[]),
            (None, 0..0, CLOSE),
        ])?;
        debug_assert_eq!(wrapped.bytes.len(), length);
        let source_map = source
            .source_map
            .with_word_source(&source.word_source_map, &source.word_events)?
            .with_publication_collector(&source.publication);
        let mut map = WordSourceMap::generated(OPEN.len());
        map.extend(&source_map)?;
        map.extend(&WordSourceMap::generated(CLOSE.len()))?;
        // Preserve the old whole wrapped-table admission before its owner
        // validator and parser. The generated shell contributes no input role.
        let admitted = wrapped.admitted_word_source()?.with_source_map(map)?;
        let wrapped = wrapped.bytes;
        let mut controls = admitted.controls_for(&wrapped).with_field_origins(
            std::sync::Arc::clone(&self.field_origins),
            source.initial_delivery,
        );
        let mut maps = crate::text::WordBlockSourceMaps {
            collect_word_texts: true,
            ..Default::default()
        };
        let mut table =
            CT_Tbl::from_xml_fragment_checked_in(&wrapped, &mut controls, Some(&mut maps))?;
        self.field_origins
            .lock()
            .expect("field origin registry poisoned")
            .commit_publication_owner(WordOwner::Table(&table), &admitted.source_map);
        let row = table
            .rows
            .first_mut()
            .ok_or_else(|| OxmlError::MissingElement("prepared row source disappeared".into()))?;
        self.field_work
            .register_original_fields(row.writer_field_roots())?;
        let hazard = !row.content_controls.is_empty()
            || row.extra_xml.iter().any(|(position, raw)| {
                !crate::table::CT_Row::raw_is_root_attributes(*position, raw)
            });
        let (buffers, direct, all) = crate::placeholder::template_row_texts_in(
            row,
            maps,
            &admitted.source_map,
            &mut self.field_work,
            &mut self.template_copy_work,
        )?;
        Ok((buffers, direct, all, hazard))
    }
}
