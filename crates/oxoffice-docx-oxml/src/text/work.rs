//! Edit work and publication accounting: work budgets, field origins, template captures, leases, transports and the operation ledger.

use super::*;

pub(super) fn initial_field_work_base(
    raw_bytes: usize,
    name_bytes: usize,
    cached_bytes: usize,
    scope_bytes: usize,
) -> Option<usize> {
    raw_bytes
        .checked_add(name_bytes)?
        .checked_add(cached_bytes)?
        .checked_add(scope_bytes)
}

pub(super) fn field_argument_work(
    bytes: Option<usize>,
    text_bytes: Option<usize>,
) -> Option<usize> {
    bytes?.checked_add(64)?.checked_add(text_bytes.unwrap_or(0))
}

pub(super) fn field_switch_work(
    bytes: Option<usize>,
    name_bytes: usize,
    text_bytes: Option<usize>,
) -> Option<usize> {
    bytes?
        .checked_add(64)?
        .checked_add(name_bytes)?
        .checked_add(text_bytes.unwrap_or(0))
}

pub(super) fn initial_field_work_bytes(
    instruction: &FieldInstruction,
    cached_result: &str,
    scope_bytes: usize,
) -> Option<usize> {
    let mut bytes = initial_field_work_base(
        instruction.raw.len(),
        instruction.name.len(),
        cached_result.len(),
        scope_bytes,
    );
    for argument in &instruction.arguments {
        bytes = field_argument_work(
            bytes,
            match argument {
                FieldArgument::Text(text) => Some(text.len()),
                FieldArgument::Nested(_) => None,
            },
        );
    }
    for switch in &instruction.switches {
        bytes = field_switch_work(
            bytes,
            switch.name.len(),
            match &switch.argument {
                Some(FieldArgument::Text(text)) => Some(text.len()),
                _ => None,
            },
        );
    }
    bytes
}

pub(super) fn legacy_field_work_bytes(
    mut current_bytes: Option<usize>,
    legacy_form: Option<&LegacyFormFieldData>,
    reset_default: Option<&LegacyFormFieldValue>,
    text_default: Option<&String>,
    author: Option<&LegacyFormAuthor>,
) -> Option<usize> {
    if let Some(form) = legacy_form {
        if let Some(name) = &form.name {
            current_bytes = current_bytes.and_then(|bytes| bytes.checked_add(name.len()));
        }
        if let LegacyFormFieldValue::Text(text) = &form.value {
            current_bytes = current_bytes.and_then(|bytes| bytes.checked_add(text.len()));
        }
        for choice in &form.choices {
            current_bytes = current_bytes
                .and_then(|bytes| bytes.checked_add(64))
                .and_then(|bytes| bytes.checked_add(choice.len()));
        }
    }
    if let Some(LegacyFormFieldValue::Text(text)) = reset_default {
        current_bytes = current_bytes.and_then(|bytes| bytes.checked_add(text.len()));
    }
    if let Some(text) = text_default {
        current_bytes = current_bytes.and_then(|bytes| bytes.checked_add(text.len()));
    }
    if let Some(author) = author {
        for text in [&author.text_default, &author.format].into_iter().flatten() {
            current_bytes = current_bytes.and_then(|bytes| bytes.checked_add(text.len()));
        }
        for text in [&author.help_text, &author.status_text]
            .into_iter()
            .flatten()
        {
            let (LegacyFormInfoText::Text(text) | LegacyFormInfoText::AutoText(text)) = text;
            current_bytes = current_bytes.and_then(|bytes| bytes.checked_add(text.len()));
        }
    }
    current_bytes
}
impl ParsedFieldMetadata {
    pub(super) fn read(
        source: crate::document::WordXmlInput<'_>,
        form: FieldForm,
        scope: &[String],
        name: &str,
        cache: &str,
    ) -> Self {
        let result_published = form == FieldForm::Simple || complex_outer_result_present(source);
        let (legacy_form, reset_default, text_default, parse_error) =
            match parse_legacy_form_data(source, scope, name, cache, result_published) {
                Ok(Some((data, reset, text))) => (Some(data), reset, text, false),
                Ok(None) => (None, None, None, false),
                Err(_) => (None, None, None, true),
            };
        Self {
            legacy_form,
            reset_default,
            text_default,
            parse_error,
            result_published,
            form,
        }
    }
}

impl NormalizationInputWork {
    pub(super) fn dynamic_work(self) -> Option<usize> {
        self.current
            .checked_add(self.scope)
            .and_then(affected_source_dynamic_work)
    }

    pub(super) fn source_work(self, dynamic: usize) -> Option<usize> {
        self.retained_source
            .checked_add(dynamic)?
            .checked_add(self.checked_payload[0])?
            .checked_add(self.checked_payload[1])?
            .checked_add(self.stored_result)?
            .checked_add(self.pending_payload)
    }
}

pub(super) struct StoredSourceOrigin {
    source: std::sync::Weak<Vec<u8>>,
    map: crate::document::WordSourceMap,
}

pub(super) struct OriginalFieldFacts {
    key: OriginalFieldKey,
    current: usize,
}

pub(super) struct FieldOriginEntry {
    source: std::sync::Weak<FieldSource>,
    map: crate::document::WordSourceMap,
    original: Option<Arc<OriginalFieldFacts>>,
    // Minted only by actual initial parser delivery for this live source owner.
    // Forwarding a map or original facts never forwards this transport role.
    incoming_record: bool,
    publication: std::collections::HashMap<usize, Vec<crate::document::WordPublicationPiece>>,
    publication_sweep: std::collections::VecDeque<usize>,
    publication_windows: std::collections::HashMap<usize, Vec<crate::document::WordCaptureWindow>>,
    publication_owned: Option<Vec<std::ops::Range<usize>>>,
}

impl FieldOriginEntry {
    fn extend_publication(&mut self, tape: crate::document::WordPublicationTape) {
        let (pieces, windows) = tape;
        // One incremental probe, never a scan over prior Source occurrences.
        if let Some(key) = self.publication_sweep.pop_front() {
            if self
                .publication
                .get(&key)
                .is_some_and(|pieces| !pieces.iter().any(|piece| piece.live()))
            {
                self.publication.remove(&key);
                self.publication_windows.remove(&key);
            } else if self.publication.contains_key(&key) {
                self.publication_sweep.push_back(key);
            }
        }
        for piece in pieces {
            let key = piece.collector_key();
            if !self.publication.contains_key(&key) {
                self.publication_sweep.push_back(key);
            }
            let values = self.publication.entry(key).or_default();
            if !values.contains(&piece) {
                values.push(piece);
            }
        }
        for window in windows {
            let values = self
                .publication_windows
                .entry(window.collector_key())
                .or_default();
            if !values.contains(&window) {
                values.push(window);
            }
        }
    }
}

impl FieldOriginRegistry {
    pub(crate) fn record_source_resource_failure(&mut self, error: &OxmlError) {
        self.source_resource_refused = true;
        // Classification happened at the actual resource producer. This only
        // retains its existing InvalidValue payload, it does not classify it.
        if self.source_resource_message.is_none()
            && let OxmlError::InvalidValue(message) = error
        {
            self.source_resource_message = Some(message.clone());
        }
    }

    pub(crate) fn source_resource_failure(&self) -> Option<OxmlError> {
        self.source_resource_refused.then(|| {
            OxmlError::InvalidValue(
                self.source_resource_message
                    .clone()
                    .unwrap_or_else(|| "mapped source resource admission refused".into()),
            )
        })
    }

    pub(super) fn result_map(
        &self,
        source: &Arc<Vec<u8>>,
    ) -> Option<crate::document::WordSourceMap> {
        let entry = self.stored.get(&(Arc::as_ptr(source) as usize))?;
        let old = entry.source.upgrade()?;
        Arc::ptr_eq(&old, source).then(|| entry.map.clone())
    }

    pub(crate) fn source_map(&self, field: &Field) -> Option<crate::document::WordSourceMap> {
        let entry = self.entries.get(&(Arc::as_ptr(&field.source) as usize))?;
        let source = entry.source.upgrade()?;
        Arc::ptr_eq(&source, &field.source).then(|| entry.map.clone())
    }

    pub(super) fn word_source_map(&self, field: &Field) -> Option<crate::document::WordSourceMap> {
        let entry = self.entries.get(&(Arc::as_ptr(&field.source) as usize))?;
        if !entry.map.has_word_inputs() {
            return None;
        }
        let source = entry.source.upgrade()?;
        Arc::ptr_eq(&source, &field.source).then(|| entry.map.clone())
    }

    pub(crate) fn register_initial(
        &mut self,
        field: &Field,
        map: crate::document::WordSourceMap,
        initial_delivery: bool,
    ) -> Result<()> {
        let publication_simple = map.has_publication() && field.form() == FieldForm::Simple;
        let publication_length = map.len();
        let original = map.entirely_original().then(|| OriginalFieldKey {
            physical: map.original_pieces(),
            marker: None,
        });
        let stored = field
            .ordered_result
            .as_ref()
            .map(|owner| {
                map.slice(owner.source_range.clone())
                    .map(|map| (Arc::clone(&owner.normalized_xml), map))
            })
            .transpose()?;
        self.register_occurrence(field, map, original, initial_delivery)?;
        if publication_simple {
            self.register_publication_ranges(
                field,
                std::iter::once(0..publication_length).collect(),
            );
        }
        if let Some((source, map)) = stored {
            self.register_result(&source, map)?;
        }
        Ok(())
    }

    pub(super) fn incoming_record(&self, field: &Field) -> bool {
        self.entries
            .get(&(Arc::as_ptr(&field.source) as usize))
            .is_some_and(|entry| {
                entry.incoming_record
                    && entry
                        .source
                        .upgrade()
                        .is_some_and(|source| Arc::ptr_eq(&source, &field.source))
            })
    }

    pub(super) fn original_for(&self, field: &Field) -> Option<Arc<OriginalFieldFacts>> {
        let entry = self.entries.get(&(Arc::as_ptr(&field.source) as usize))?;
        let source = entry.source.upgrade()?;
        Arc::ptr_eq(&source, &field.source)
            .then(|| entry.original.clone())
            .flatten()
    }

    pub(super) fn register_occurrence(
        &mut self,
        field: &Field,
        map: crate::document::WordSourceMap,
        original: Option<OriginalFieldKey>,
        initial_delivery: bool,
    ) -> Result<()> {
        let incoming_record =
            initial_delivery && map.entirely_original() && map.original_pieces().len() == 1;
        let facts = if let Some(key) = original {
            if let Some(facts) = self.original_facts.get(&key) {
                Some(Arc::clone(facts))
            } else if initial_delivery {
                let facts = Arc::new(OriginalFieldFacts {
                    key: key.clone(),
                    current: field.normalization_input_work()?.current,
                });
                self.original_facts.insert(key, Arc::clone(&facts));
                Some(facts)
            } else {
                None
            }
        } else {
            None
        };
        self.register(field, map)?;
        if incoming_record {
            self.entries
                .get_mut(&(Arc::as_ptr(&field.source) as usize))
                .expect("registered field source")
                .incoming_record = true;
        }
        if let Some(facts) = facts {
            self.entries
                .get_mut(&(Arc::as_ptr(&field.source) as usize))
                .expect("registered field source")
                .original = Some(facts);
        }
        Ok(())
    }

    pub(super) fn register_forwarded(
        &mut self,
        field: &Field,
        map: crate::document::WordSourceMap,
        facts: Option<Arc<OriginalFieldFacts>>,
    ) -> Result<()> {
        self.register(field, map)?;
        if let Some(facts) = facts {
            self.entries
                .get_mut(&(Arc::as_ptr(&field.source) as usize))
                .expect("registered field source")
                .original = Some(facts);
        }
        Ok(())
    }

    pub(super) fn register_result(
        &mut self,
        source: &Arc<Vec<u8>>,
        mut map: crate::document::WordSourceMap,
    ) -> Result<()> {
        map.take_publication();
        if source.len() != map.len() {
            return Err(OxmlError::InvalidValue(
                "mapped stored result length changed".into(),
            ));
        }
        let key = Arc::as_ptr(source) as usize;
        if let Some(entry) = self.stored.get(&key)
            && entry
                .source
                .upgrade()
                .is_some_and(|old| Arc::ptr_eq(&old, source))
        {
            if entry.map != map {
                return Err(OxmlError::InvalidValue(
                    "mapped stored result correspondence changed".into(),
                ));
            }
            return Ok(());
        }
        if let Some(old) = self.stored_sweep.pop_front() {
            if self
                .stored
                .get(&old)
                .is_some_and(|entry| entry.source.strong_count() == 0)
            {
                self.stored.remove(&old);
            } else if self.stored.contains_key(&old) {
                self.stored_sweep.push_back(old);
            }
        }
        self.stored_sweep.push_back(key);
        self.stored.insert(
            key,
            StoredSourceOrigin {
                source: Arc::downgrade(source),
                map,
            },
        );
        Ok(())
    }

    pub(crate) fn register(
        &mut self,
        field: &Field,
        mut map: crate::document::WordSourceMap,
    ) -> Result<()> {
        let FieldSource::Parsed { raw_xml, .. } = field.source.as_ref() else {
            return Err(OxmlError::InvalidValue(
                "mapped parsed field lost its source".into(),
            ));
        };
        if map.len() != raw_xml.len() {
            return Err(OxmlError::InvalidValue(
                "mapped field source length changed".into(),
            ));
        }
        let publication = map.take_publication();
        let key = Arc::as_ptr(&field.source) as usize;
        if let Some(existing) = self.entries.get_mut(&key)
            && existing
                .source
                .upgrade()
                .is_some_and(|source| Arc::ptr_eq(&source, &field.source))
        {
            if existing.map != map {
                return Err(OxmlError::InvalidValue(
                    "mapped field source correspondence changed".into(),
                ));
            }
            existing.extend_publication(publication);
            return Ok(());
        }
        // One incremental probe per registration keeps expired weak records
        // from becoming a history scan. Live source owners remain registered.
        if let Some(old) = self.sweep.pop_front() {
            if self
                .entries
                .get(&old)
                .is_some_and(|entry| entry.source.strong_count() == 0)
            {
                self.entries.remove(&old);
            } else if self.entries.contains_key(&old) {
                self.sweep.push_back(old);
            }
        }
        self.sweep.push_back(key);
        self.entries.insert(
            key,
            FieldOriginEntry {
                source: Arc::downgrade(&field.source),
                map,
                original: None,
                incoming_record: false,
                publication: Default::default(),
                publication_sweep: Default::default(),
                publication_windows: Default::default(),
                publication_owned: None,
            },
        );
        self.entries
            .get_mut(&key)
            .expect("registered field source")
            .extend_publication(publication);
        Ok(())
    }

    pub(super) fn register_publication_ranges(
        &mut self,
        field: &Field,
        ranges: Vec<std::ops::Range<usize>>,
    ) {
        if let Some(entry) = self.entries.get_mut(&(Arc::as_ptr(&field.source) as usize))
            && entry
                .source
                .upgrade()
                .is_some_and(|source| Arc::ptr_eq(&source, &field.source))
        {
            entry.publication_owned = Some(ranges);
        }
    }

    pub(super) fn publication_selection(
        &self,
        field: &Field,
    ) -> Option<Vec<crate::document::WordPublicationPiece>> {
        let entry = self.entries.get(&(Arc::as_ptr(&field.source) as usize))?;
        if !entry
            .source
            .upgrade()
            .is_some_and(|source| Arc::ptr_eq(&source, &field.source))
        {
            return None;
        }
        let owned = entry.publication_owned.as_ref()?;
        Some(
            entry
                .publication
                .values()
                .flatten()
                .flat_map(|piece| piece.select_owned(owned))
                .collect(),
        )
    }

    /// Commit only owners returned by a successful actual owner factory.
    /// The input tape limits this join to its live current-source occurrences.
    pub(crate) fn commit_publication_owner(
        &self,
        root: crate::document::WordOwner<'_>,
        input: &crate::document::WordSourceMap,
    ) {
        let collectors = input.publication_collectors();
        if collectors.is_empty() {
            return;
        }
        // This entry is called only for the freshly returned successful
        // parser model. Traverse its retained physical owners, including
        // deleted revisions and drawing text boxes, rather than the editable
        // TextWalk selection. Registry source/current joins still gate facts.
        let mut pending = vec![root];
        let mut seen = HashSet::new();
        let mut delivered = Vec::new();
        while let Some(owner) = pending.pop() {
            if !seen.insert(owner.key()) {
                continue;
            }
            if let crate::document::WordOwner::Field(field) = owner {
                delivered.push(field);
            }
            pending.extend(owner.resource_children());
        }
        for collector in collectors {
            let mut selected = Vec::new();
            let mut joined = true;
            for field in &delivered {
                joined &= self.publication_ranges(field, &collector, &mut selected);
            }
            let mut coverage = collector.lock().expect("publication coverage poisoned");
            if !delivered.is_empty() {
                coverage.returned_fields_joined =
                    Some(coverage.returned_fields_joined.unwrap_or(true) && joined);
            }
            coverage.add_ranges(selected);
        }
    }

    pub(crate) fn publication_ranges(
        &self,
        field: &Field,
        collector: &Arc<std::sync::Mutex<crate::document::WordPublicationCoverage>>,
        output: &mut Vec<std::ops::Range<usize>>,
    ) -> bool {
        let Some(entry) = self.entries.get(&(Arc::as_ptr(&field.source) as usize)) else {
            return false;
        };
        if !entry
            .source
            .upgrade()
            .is_some_and(|source| Arc::ptr_eq(&source, &field.source))
        {
            return false;
        }
        let Some(owned) = &entry.publication_owned else {
            return false;
        };
        let mut proof = Vec::new();
        if let Some(pieces) = entry.publication.get(&(Arc::as_ptr(collector) as usize)) {
            for piece in pieces {
                piece.selected_ranges(collector, owned, output, Some(&mut proof));
            }
            if let Some(windows) = entry
                .publication_windows
                .get(&(Arc::as_ptr(collector) as usize))
            {
                for window in windows {
                    window.selected_ranges(pieces, owned, output, Some(&mut proof));
                }
            }
        }
        let mut coverage = crate::document::WordPublicationCoverage::default();
        coverage.add_ranges(proof);
        coverage.covers_ranges(owned)
    }
}

/// A Word operand identity is independent of every Field allowance.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum TemplateWordOrigin {
    Raw { start: usize, end: usize },
    Text(usize),
    CachedResult(usize),
}

#[derive(Clone, Debug)]
pub(crate) struct ParsedWordText {
    pub(crate) drawing: Option<crate::drawing::DrawingSourceMaps>,
    pub(crate) content: usize,
    pub(crate) encoded: std::ops::Range<usize>,
}

#[derive(Clone, Debug)]
pub(crate) struct ParsedWordRunTexts {
    pub(crate) source: std::ops::Range<usize>,
    pub(crate) source_map: Option<Arc<crate::document::WordSourceMap>>,
    pub(crate) texts: Vec<ParsedWordText>,
}

pub(crate) type WordParagraphTextFacts = Vec<Option<ParsedWordRunTexts>>;

#[derive(Clone)]
pub(crate) struct WordParagraphReadFacts {
    pub(crate) source_map: Arc<crate::document::WordSourceMap>,
    pub(crate) texts: WordParagraphTextFacts,
    pub(crate) nested: WordBlockSourceMaps,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TemplateWordEvent {
    pub(crate) encoded: std::ops::Range<usize>,
    pub(crate) origin: TemplateWordOrigin,
    pub(crate) decoded: std::ops::Range<usize>,
}

struct TemplatePreparedFieldFact {
    frozen: usize,
    scalar: bool,
}

pub(super) struct TemplateFieldRegistry {
    anchor: std::sync::Weak<()>,
    next: usize,
    facts: std::collections::HashMap<usize, TemplatePreparedFieldFact>,
}

#[derive(Clone)]
pub(super) struct TemplateScalarOrigin {
    origin: TemplateWordOrigin,
    decoded: std::ops::Range<usize>,
}

pub(super) struct TemplateScalarBorrow {
    start: usize,
    length: usize,
    origin: TemplateScalarOrigin,
}

/// This lexical lease retains the actual selected scalar borrow. Its pointer
/// arithmetic only forwards sub-slices produced by FieldResultParts during
/// this call, and never discovers an input or survives the selected operand.
pub(super) struct TemplateScalarLease<'a> {
    _value: &'a str,
    capture: Arc<std::sync::Mutex<TemplateWordCapture>>,
    previous: Option<TemplateScalarBorrow>,
}
impl Drop for TemplateScalarLease<'_> {
    fn drop(&mut self) {
        self.capture
            .lock()
            .expect("Word text capture poisoned")
            .scalar = self.previous.take();
    }
}

impl TemplateWordCapture {
    fn returned_raw_map(&self, bytes: &[u8]) -> Result<Option<crate::document::WordSourceMap>> {
        let start = bytes.as_ptr() as usize;
        let Some((&base, (end, map))) = self.returned_raw.range(..=start).next_back() else {
            return Ok(None);
        };
        let Some(limit) = start.checked_add(bytes.len()) else {
            return Err(OxmlError::InvalidValue(
                "returned Word record overflow".into(),
            ));
        };
        if limit > *end {
            return Ok(None);
        }
        Ok(Some(map.slice(start - base..limit - base)?))
    }

    fn raw_range(&self, bytes: &[u8]) -> Result<Option<std::ops::Range<usize>>> {
        let start = bytes.as_ptr() as usize;
        let end = start
            .checked_add(bytes.len())
            .ok_or_else(|| OxmlError::InvalidValue("Word record position overflow".into()))?;
        let Some((&base, &(limit, frozen))) = self.raw_origins.range(..=start).next_back() else {
            return Ok(None);
        };
        if end > limit {
            return Ok(None);
        }
        let origin = frozen
            .checked_add(start - base)
            .ok_or_else(|| OxmlError::InvalidValue("Word record origin overflow".into()))?;
        let last = origin
            .checked_add(bytes.len())
            .ok_or_else(|| OxmlError::InvalidValue("Word record origin overflow".into()))?;
        Ok(Some(origin..last))
    }
}

pub(crate) struct TemplateWordWriterLease<'a> {
    previous: Option<Arc<std::sync::Mutex<TemplateWordCapture>>>,
    pub(crate) work: &'a mut WordOperationWork,
}
impl Drop for TemplateWordWriterLease<'_> {
    fn drop(&mut self) {
        self.work.template_word_capture = self.previous.take();
    }
}

// Facts stay borrowed through the actual clone serialization. This lease
// never discovers an origin by address. It forwards an already selected old
// object along the real clone pair, and restores all temporary destination keys.
pub(crate) struct TemplateParagraphCloneLease<'a> {
    _previous: &'a CT_P,
    _current: &'a CT_P,
    capture: Arc<std::sync::Mutex<TemplateWordCapture>>,
    texts: Vec<(usize, Option<TemplateScalarOrigin>)>,
    raws: Vec<(usize, Option<(usize, crate::document::WordSourceMap)>)>,
}
impl<'a> TemplateParagraphCloneLease<'a> {
    pub(crate) fn new(
        previous: &'a CT_P,
        current: &'a CT_P,
        work: &WordOperationWork,
    ) -> Result<Option<Self>> {
        let Some(capture) = work.template_word_capture.clone() else {
            return Ok(None);
        };
        let (texts, _, raws) = crate::document::template_paragraph_clone_pairs(previous, current)?;
        let mut selected_texts = Vec::new();
        let mut selected_raws = Vec::new();
        for (new, old) in texts {
            if new.text == old.text
                && let Some(input) = template_word_text_input(Some(&capture), old)
            {
                selected_texts.push((std::ptr::from_ref(new) as usize, input.origin));
            }
        }
        for (new, old) in raws {
            if new != old {
                continue;
            }
            let map = work.template_word_raw_map(old)?;
            if map.has_word_inputs() {
                let start = new.as_ptr() as usize;
                let end = start
                    .checked_add(new.len())
                    .ok_or_else(|| OxmlError::InvalidValue("cloned Word record overflow".into()))?;
                selected_raws.push((start, end, map));
            }
        }
        let mut facts = capture.lock().expect("Word text capture poisoned");
        let texts = selected_texts
            .into_iter()
            .map(|(key, origin)| (key, facts.produced_texts.insert(key, origin)))
            .collect();
        let mut raws = Vec::new();
        for (key, end, map) in selected_raws {
            raws.push((key, facts.returned_raw.insert(key, (end, map))));
        }
        drop(facts);
        Ok(Some(Self {
            _previous: previous,
            _current: current,
            capture,
            texts,
            raws,
        }))
    }
}
impl Drop for TemplateParagraphCloneLease<'_> {
    fn drop(&mut self) {
        let mut facts = self.capture.lock().expect("Word text capture poisoned");
        for (key, old) in self.texts.drain(..).rev() {
            if let Some(old) = old {
                facts.produced_texts.insert(key, old);
            } else {
                facts.produced_texts.remove(&key);
            }
        }
        for (key, old) in self.raws.drain(..).rev() {
            if let Some(old) = old {
                facts.returned_raw.insert(key, old);
            } else {
                facts.returned_raw.remove(&key);
            }
        }
    }
}

pub(super) fn template_word_text_input(
    capture: Option<&Arc<std::sync::Mutex<TemplateWordCapture>>>,
    text: &CT_Text,
) -> Option<TemplateWordTextInput> {
    let facts = capture?.lock().expect("Word text capture poisoned");
    let key = std::ptr::from_ref(text) as usize;
    let origin = if let Some(origin) = facts.produced_texts.get(&key) {
        if text.text.len() != origin.decoded.len() {
            return None;
        }
        origin.clone()
    } else {
        let (origin, length) = facts.text_origins.get(&key)?;
        if text.text.len() > *length {
            return None;
        }
        TemplateScalarOrigin {
            origin: origin.clone(),
            decoded: 0..text.text.len(),
        }
    };
    Some(TemplateWordTextInput {
        origin,
        output: Arc::clone(&facts.output),
    })
}

/// Only the actual run-segment clone producer installs these facts. Both
/// sides stay borrowed through serialization, so temporary addresses cannot
/// escape the corresponding source/clone lifetime or authorize later models.
pub(super) struct TemplateTextCloneLease<'a> {
    _source: &'a [RunContent],
    _current: &'a CT_R,
    previous: Vec<(usize, Option<TemplateScalarOrigin>)>,
    capture: Arc<std::sync::Mutex<TemplateWordCapture>>,
}
impl<'a> TemplateTextCloneLease<'a> {
    pub(super) fn new(
        source: &'a [RunContent],
        current: &'a CT_R,
        capture: Option<&Arc<std::sync::Mutex<TemplateWordCapture>>>,
    ) -> Option<Self> {
        let capture = capture?;
        let mut previous = Vec::new();
        for (old, new) in source.iter().zip(&current.content) {
            if let (RunContent::Text(old), RunContent::Text(new)) = (old, new)
                && let Some(input) = template_word_text_input(Some(capture), old)
            {
                let key = std::ptr::from_ref(new) as usize;
                let prior = capture
                    .lock()
                    .expect("Word text capture poisoned")
                    .produced_texts
                    .insert(key, input.origin);
                previous.push((key, prior));
            }
        }
        Some(Self {
            _source: source,
            _current: current,
            previous,
            capture: Arc::clone(capture),
        })
    }
}
impl Drop for TemplateTextCloneLease<'_> {
    fn drop(&mut self) {
        let mut capture = self
            .capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (key, previous) in self.previous.drain(..).rev() {
            if let Some(origin) = previous {
                capture.produced_texts.insert(key, origin);
            } else {
                capture.produced_texts.remove(&key);
            }
        }
    }
}

/// Only positively proved direct run slots are emitted by the replacement
/// producer. Nested refresh paths require their own parser-return transcript.
/// The complete paragraph stays borrowed while its writer
/// consumes these slots, and every temporary address is removed on exit.
pub(super) struct TemplateParagraphTextLease<'a> {
    _paragraph: &'a CT_P,
    previous: Vec<(usize, Option<TemplateScalarOrigin>)>,
    capture: Arc<std::sync::Mutex<TemplateWordCapture>>,
}
impl<'a> TemplateParagraphTextLease<'a> {
    pub(super) fn new(
        paragraph: &'a CT_P,
        produced: Vec<TemplateProducedText>,
        capture: Option<&Arc<std::sync::Mutex<TemplateWordCapture>>>,
    ) -> Option<Self> {
        let capture = capture?;
        let mut previous = Vec::new();
        for fact in produced {
            if let Some(run) = paragraph.runs.get(fact.run)
                && let Some(RunContent::Text(text)) = run.content.get(fact.content)
            {
                let key = std::ptr::from_ref(text) as usize;
                let prior = capture
                    .lock()
                    .expect("Word text capture poisoned")
                    .produced_texts
                    .insert(key, fact.origin);
                previous.push((key, prior));
            }
        }
        Some(Self {
            _paragraph: paragraph,
            previous,
            capture: Arc::clone(capture),
        })
    }
}
impl Drop for TemplateParagraphTextLease<'_> {
    fn drop(&mut self) {
        let mut capture = self
            .capture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (key, previous) in self.previous.drain(..).rev() {
            if let Some(origin) = previous {
                capture.produced_texts.insert(key, origin);
            } else {
                capture.produced_texts.remove(&key);
            }
        }
    }
}

pub(super) fn template_word_scalar_input(
    work: &WordOperationWork,
    value: &str,
) -> Option<TemplateWordTextInput> {
    let capture = work
        .template_word_capture
        .as_ref()?
        .lock()
        .expect("Word text capture poisoned");
    let selected = capture.scalar.as_ref()?;
    let offset = (value.as_ptr() as usize).checked_sub(selected.start)?;
    let end = offset.checked_add(value.len())?;
    if end > selected.length {
        return None;
    }
    Some(TemplateWordTextInput {
        origin: TemplateScalarOrigin {
            origin: selected.origin.origin.clone(),
            decoded: selected.origin.decoded.start + offset..selected.origin.decoded.start + end,
        },
        output: Arc::clone(&capture.output),
    })
}

pub(super) struct WordPayloadRange {
    start: usize,
    next: usize,
    end: usize,
    origin: TemplateScalarOrigin,
}

/// The actual escaped payload stays borrowed for the complete write. Metadata
/// locks are released before reentering Writer, and every exit restores state.
pub(super) struct WordPayloadTransport<'a> {
    _payload: &'a [u8],
    previous: Option<WordPayloadRange>,
    output: Arc<std::sync::Mutex<NormalizedOutputWork>>,
}
impl<'a> WordPayloadTransport<'a> {
    pub(super) fn new(payload: &'a [u8], input: TemplateWordTextInput) -> Result<Self> {
        let start = payload.as_ptr() as usize;
        let end = start
            .checked_add(payload.len())
            .ok_or_else(|| OxmlError::InvalidValue("Word payload position overflow".into()))?;
        let previous = input
            .output
            .lock()
            .expect("normalization ledger poisoned")
            .word_payload
            .replace(WordPayloadRange {
                start,
                next: start,
                end,
                origin: input.origin,
            });
        Ok(Self {
            _payload: payload,
            previous,
            output: input.output,
        })
    }
}
impl Drop for WordPayloadTransport<'_> {
    fn drop(&mut self) {
        self.output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .word_payload = self.previous.take();
    }
}

pub(super) struct FieldPublicationLease {
    previous: bool,
    work: Arc<std::sync::Mutex<NormalizedOutputWork>>,
}
impl Drop for FieldPublicationLease {
    fn drop(&mut self) {
        self.work
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .publication_field = self.previous;
    }
}

struct FinalValidationWriterLease {
    previous: Option<usize>,
    work: Arc<std::sync::Mutex<NormalizedOutputWork>>,
}
impl Drop for FinalValidationWriterLease {
    fn drop(&mut self) {
        self.work
            .lock()
            .expect("normalization ledger poisoned")
            .final_validation_writer = self.previous;
    }
}

/// The live concrete final writer stays mutably borrowed throughout this call.
/// Nested normalization writers have distinct live storage and cannot acquire
/// this role. The lexical receipt restores even on ordinary error or unwind.
pub(crate) fn write_prepared_validation_xml(
    paragraph: &CT_P,
    writer: &mut Writer<Vec<u8>>,
    work: &mut WordOperationWork,
) -> Result<()> {
    write_final_word_block(FinalWordBlock::Paragraph(paragraph), writer, work)
}

/// These are the four actual scoped block publishers. The same concrete writer
/// also serves the existing paragraph preservation validator above.
pub(crate) enum FinalWordBlock<'a> {
    Paragraph(&'a CT_P),
    Table(&'a crate::table::CT_Tbl),
    Cell(&'a crate::table::CT_Tc),
    Control(&'a crate::content_control::CT_Sdt),
}

pub(crate) fn write_final_word_block<W: std::io::Write>(
    block: FinalWordBlock<'_>,
    writer: &mut Writer<W>,
    work: &mut WordOperationWork,
) -> Result<()> {
    let target = std::ptr::from_ref(&*writer) as usize;
    let previous = work
        .normalized_output
        .lock()
        .expect("normalization ledger poisoned")
        .final_validation_writer
        .replace(target);
    let _lease = FinalValidationWriterLease {
        previous,
        work: Arc::clone(&work.normalized_output),
    };
    match block {
        FinalWordBlock::Paragraph(paragraph) => {
            paragraph.write_xml_core_in(writer, None, XmlWriteRole::Publication, None, work, None)
        }
        FinalWordBlock::Table(table) => {
            table.write_xml_role_in(writer, XmlWriteRole::Publication, None, work, None)
        }
        FinalWordBlock::Cell(cell) => {
            cell.write_xml_role_in(writer, XmlWriteRole::Publication, None, work, None)
        }
        FinalWordBlock::Control(control) => {
            control.write_xml_role_in(writer, XmlWriteRole::Publication, None, work, None)
        }
    }
}

/// This borrow pins the exact mapped final writer for the complete call.
/// Nested normalization writers never receive its target, even on this ledger.
struct FinalMappedWriterLease<'a> {
    writer: &'a mut Writer<NormalizedWordOutput>,
    previous: Option<usize>,
}
impl Drop for FinalMappedWriterLease<'_> {
    fn drop(&mut self) {
        self.writer.get_mut().final_writer = self.previous;
    }
}

pub(crate) fn final_word_source(
    block: FinalWordBlock<'_>,
    work: &mut WordOperationWork,
) -> Result<(
    Vec<u8>,
    crate::document::WordSourceMap,
    Vec<TemplateWordEvent>,
)> {
    let mut writer = Writer::new(work.model_source_output());
    let target = std::ptr::from_ref(&writer) as usize;
    let previous = writer.get_mut().final_writer.replace(target);
    {
        let lease = FinalMappedWriterLease {
            writer: &mut writer,
            previous,
        };
        // The same substantive serializer has actual Vec and Normalized sinks.
        write_final_word_block(block, lease.writer, work)?;
    }
    Ok(writer.into_inner().into_word_parts())
}

impl Drop for PrepaidXmlTransport<'_> {
    fn drop(&mut self) {
        self.work
            .lock()
            .expect("normalization ledger poisoned")
            .prepaid_xml = self.previous;
    }
}

pub(super) struct MappedFieldRange {
    next: usize,
    end: usize,
    consumed: usize,
    map: crate::document::WordSourceMap,
}

pub(crate) struct MappedFieldTransport<'a> {
    _source: &'a [u8],
    previous: Option<MappedFieldRange>,
    work: Arc<std::sync::Mutex<NormalizedOutputWork>>,
}

impl Drop for MappedFieldTransport<'_> {
    fn drop(&mut self) {
        self.work
            .lock()
            .expect("normalization ledger poisoned")
            .mapped_transport = self.previous.take();
    }
}

pub(crate) struct RetainedRevisionTransport<'a> {
    _source: &'a [u8],
    previous: Option<RetainedRevisionRange>,
    work: Arc<std::sync::Mutex<NormalizedOutputWork>>,
}

impl Drop for RetainedRevisionTransport<'_> {
    fn drop(&mut self) {
        self.work
            .lock()
            .expect("normalization ledger poisoned")
            .retained_revision = self.previous;
    }
}

pub(crate) struct NewFieldEmission {
    previous: bool,
    work: Arc<std::sync::Mutex<NormalizedOutputWork>>,
}

impl Drop for NewFieldEmission {
    fn drop(&mut self) {
        self.work
            .lock()
            .expect("normalization ledger poisoned")
            .new_field = self.previous;
    }
}

impl NormalizedWordOutput {
    pub(crate) fn into_word_parts(
        mut self,
    ) -> (
        Vec<u8>,
        crate::document::WordSourceMap,
        Vec<TemplateWordEvent>,
    ) {
        let events = self.source_map.take_word_events();
        (self.bytes, self.source_map, events)
    }

    pub(crate) fn into_mapped_parts(self) -> (Vec<u8>, crate::document::WordSourceMap) {
        (self.bytes, self.source_map)
    }
}

impl std::io::Write for NormalizedWordOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let mut work = self.work.lock().expect("normalization ledger poisoned");
        let local = self.bytes.len().checked_add(bytes.len());
        let start = bytes.as_ptr() as usize;
        let end = start.checked_add(bytes.len());
        let retained = !self.new_source_record
            && work.retained_revision.is_some_and(|range| {
                start == range.next && end.is_some_and(|end| end <= range.end)
            });
        let prepaid = work
            .prepaid_xml
            .is_some_and(|range| start == range.next && end.is_some_and(|end| end <= range.end));
        let final_transport = self
            .final_writer
            .is_some_and(|target| work.final_validation_writer == Some(target));
        let generation = (!self.field_only || work.new_field) && !final_transport;
        let aggregate = work
            .emitted
            .checked_add(if retained || prepaid || !generation {
                0
            } else {
                bytes.len()
            });
        if local.is_none_or(|n| self.local_limit.is_some_and(|limit| n > limit))
            || aggregate.is_none_or(|n| n > work.allowance)
        {
            work.refused = true;
            return Err(std::io::Error::other(
                "normalized result exceeds its preflight source work",
            ));
        }
        let mapped = work
            .mapped_transport
            .as_mut()
            .filter(|range| start == range.next && end.is_some_and(|end| end <= range.end));
        let mut map = if let Some(range) = mapped {
            let next = range
                .consumed
                .checked_add(bytes.len())
                .ok_or_else(|| std::io::Error::other("derived source position overflow"))?;
            let map = if self.new_source_record
                && !range.map.has_word_inputs()
                && !range.map.has_publication_owned()
            {
                crate::document::WordSourceMap::generated(bytes.len())
            } else {
                let map = range
                    .map
                    .slice(range.consumed..next)
                    .map_err(std::io::Error::other)?;
                if self.new_source_record {
                    map.word_only()
                } else {
                    map
                }
            };
            range.consumed = next;
            range.next = end.expect("checked mapped source range");
            map
        } else {
            crate::document::WordSourceMap::generated(bytes.len())
        };
        if let Some(capture) = &self.word_capture {
            let capture = capture.lock().expect("Word text capture poisoned");
            if let Some(origin) = capture.raw_range(bytes).map_err(std::io::Error::other)? {
                map = map.with_word_range(origin).map_err(std::io::Error::other)?;
            }
            if let Some(returned) = capture
                .returned_raw_map(bytes)
                .map_err(std::io::Error::other)?
            {
                map = map
                    .with_retained_word_source(&returned)
                    .map_err(std::io::Error::other)?;
            }
        }
        if let Some(payload) = &mut work.word_payload
            && start == payload.next
            && end.is_some_and(|end| end <= payload.end)
        {
            if start == payload.start && end == Some(payload.end) && !bytes.is_empty() {
                map.push_word_event(TemplateWordEvent {
                    encoded: 0..bytes.len(),
                    origin: payload.origin.origin.clone(),
                    decoded: payload.origin.decoded.clone(),
                })
                .map_err(std::io::Error::other)?;
            }
            payload.next = end.expect("checked Word payload range");
        }
        if work.publication_field {
            map.mark_field_publication();
        }
        self.source_map
            .extend(&map)
            .map_err(std::io::Error::other)?;
        self.bytes.extend_from_slice(bytes);
        work.emitted = aggregate.expect("checked output sum");
        if prepaid {
            work.prepaid_xml
                .as_mut()
                .expect("selected prepaid production")
                .next = end.expect("checked prepaid range");
        }
        if retained {
            work.retained_revision
                .as_mut()
                .expect("selected transport")
                .next = end.expect("checked range");
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl WordOperationWork {
    pub(super) fn model_field(root: &Field) -> Result<Self> {
        Self::model_fields(vec![root])
    }

    pub(crate) fn empty() -> Self {
        Self {
            constructor_allowance: 0,
            constructor_actions: 0,
            field_output_metric: false,
            resource_refused: false,
            current_inputs: HashSet::new(),
            template_current_inputs: HashSet::new(),
            retained_inputs: HashSet::new(),
            stored_inputs: HashSet::new(),
            original_current_inputs: HashSet::new(),
            original_source_ranges: Default::default(),
            field_origins: Default::default(),
            template_word_capture: None,
            template_field_registry: None,
            normalized_output: Arc::new(std::sync::Mutex::new(NormalizedOutputWork {
                allowance: 0,
                emitted: 0,
                refused: false,
                retained_revision: None,
                new_field: false,
                mapped_transport: None,
                prepaid_xml: None,
                final_validation_writer: None,
                word_payload: None,
                publication_field: false,
            })),
        }
    }

    pub(crate) fn model_fields(pending: Vec<&Field>) -> Result<Self> {
        let mut work = Self::empty();
        work.register_model_fields(pending)?;
        Ok(work)
    }

    /// Only an actual frozen model ingress may call this. Candidate clones and
    /// derived parses forward their selected context instead of registering.
    pub(crate) fn register_model_fields(&mut self, mut pending: Vec<&Field>) -> Result<()> {
        let selected_fields = !pending.is_empty();
        let overflow =
            || OxmlError::InvalidValue("normalization affected-source work overflow".into());
        let mut current_owners = HashSet::new();
        let mut source_records = HashSet::new();
        let mut stored_records = HashSet::new();
        let mut source_bytes = 0usize;
        let mut current_bytes = 0usize;
        while let Some(field) = pending.pop() {
            // The borrowed frozen ingress supplies identity here. These
            // addresses index this frozen baseline membership, never OPC proof
            // or permission to register later candidate allocations as input.
            let current_key = std::ptr::from_ref(field) as usize;
            if self.current_inputs.contains(&current_key) || !current_owners.insert(current_key) {
                continue;
            }
            let inputs = field.normalization_input_work()?;
            current_bytes = current_bytes
                .checked_add(inputs.current)
                .ok_or_else(overflow)?;
            source_bytes = source_bytes
                .checked_add(inputs.checked_payload[1])
                .and_then(|n| n.checked_add(inputs.pending_payload))
                .ok_or_else(overflow)?;
            let source_key = Arc::as_ptr(&field.source) as usize;
            if !self.retained_inputs.contains(&source_key) && source_records.insert(source_key) {
                source_bytes = source_bytes
                    .checked_add(inputs.retained_source)
                    .and_then(|n| n.checked_add(inputs.checked_payload[0]))
                    .ok_or_else(overflow)?;
                // Scope credit is selected by the actual namespace producer.
                // Scanning raw XML here would duplicate that producer, move
                // source errors ahead of child normalization, and grant an
                // unvisited binding to typed leaf construction.
            }
            if let Some(owner) = &field.ordered_result
                && !self
                    .stored_inputs
                    .contains(&(Arc::as_ptr(&owner.normalized_xml) as usize))
                && stored_records.insert(Arc::as_ptr(&owner.normalized_xml) as usize)
            {
                source_bytes = source_bytes
                    .checked_add(inputs.stored_result)
                    .ok_or_else(overflow)?;
            }
            pending.extend(
                field
                    .normalization_children()?
                    .into_iter()
                    .map(|(child, _)| child),
            );
        }
        let added = affected_source_dynamic_work(current_bytes)
            .and_then(|n| n.checked_add(source_bytes))
            .ok_or_else(overflow)?;
        let allowance = self
            .constructor_allowance
            .checked_add(added)
            .ok_or_else(overflow)?;
        let mut output = self
            .normalized_output
            .lock()
            .expect("normalization ledger poisoned");
        let output_allowance = output.allowance.checked_add(added).ok_or_else(overflow)?;
        self.current_inputs.extend(current_owners);
        self.retained_inputs.extend(source_records);
        self.stored_inputs.extend(stored_records);
        self.constructor_allowance = allowance;
        self.field_output_metric |= selected_fields;
        output.allowance = output_allowance;
        Ok(())
    }

    /// Activate only facts delivered by a successful original constructor. A
    /// derived parse can forward such a fact but cannot create an ingress role.
    pub(crate) fn register_original_fields(&mut self, pending: Vec<&Field>) -> Result<()> {
        self.register_original_fields_with_inputs(pending, &[], &[])
    }

    /// The actual scoped caller event accompanies only genuinely accepted
    /// literal matches. Check its arithmetic with the selected original packet
    /// before changing any ledger membership or the shared delivery flag.
    pub(crate) fn register_original_fields_with_inputs(
        &mut self,
        mut pending: Vec<&Field>,
        literals: &[(&str, &Cell<bool>)],
        datums: &[(usize, usize)],
    ) -> Result<()> {
        let selected_fields = !pending.is_empty();
        if self.resource_refused() {
            return Err(OxmlError::InvalidValue(
                "normalized result exceeds its preflight source work".into(),
            ));
        }
        let mut event_keys = HashSet::new();
        let mut selected_datums = HashSet::new();
        let overflow =
            || OxmlError::InvalidValue("normalization affected-source work overflow".into());
        let mut selected = HashSet::new();
        let mut physical = Vec::new();
        let mut current = 0usize;
        while let Some(field) = pending.pop() {
            let registered = {
                let origins = self
                    .field_origins
                    .lock()
                    .expect("field origin registry poisoned");
                (origins.source_map(field), origins.original_for(field))
            };
            if let Some(map) = registered.0 {
                physical.extend(map.original_pieces());
            }
            if let Some(facts) = registered.1
                && !self.original_current_inputs.contains(&facts.key)
                && selected.insert(facts.key.clone())
            {
                current = current.checked_add(facts.current).ok_or_else(overflow)?;
            }
            pending.extend(
                field
                    .normalization_children()?
                    .into_iter()
                    .map(|(field, _)| field),
            );
        }
        let (added_raw, removed, updates) = self.original_source_union(physical)?;
        for &(value, delivered) in literals {
            if selected_fields
                && !delivered.get()
                && (literals.len() == 1
                    || event_keys.insert(std::ptr::from_ref(delivered) as usize))
            {
                current = current.checked_add(value.len()).ok_or_else(overflow)?;
            }
        }
        if selected_fields {
            for &(key, bytes) in datums {
                if !self.template_current_inputs.contains(&key) && selected_datums.insert(key) {
                    current = current.checked_add(bytes).ok_or_else(overflow)?;
                }
            }
        }
        let added = affected_source_dynamic_work(current)
            .and_then(|n| n.checked_add(added_raw))
            .ok_or_else(overflow)?;
        let allowance = self
            .constructor_allowance
            .checked_add(added)
            .ok_or_else(overflow)?;
        let mut output = self
            .normalized_output
            .lock()
            .expect("normalization ledger poisoned");
        let output_allowance = output.allowance.checked_add(added).ok_or_else(overflow)?;
        for key in removed {
            self.original_source_ranges.remove(&key);
        }
        for (part, start, end) in updates {
            self.original_source_ranges.insert((part, start), end);
        }
        self.original_current_inputs.extend(selected);
        self.template_current_inputs.extend(selected_datums);
        self.constructor_allowance = allowance;
        output.allowance = output_allowance;
        self.field_output_metric |= selected_fields;
        if selected_fields {
            for (_, delivered) in literals {
                delivered.set(true);
            }
        }
        Ok(())
    }

    // Both parsed-Field activation and actual retained Source copies select
    // physical input through this one union. Derived output never renews it.
    #[allow(clippy::type_complexity)] // Staged raw byte delta, removals and replacements commit together.
    fn original_source_union(
        &self,
        mut physical: Vec<(usize, usize, usize)>,
    ) -> Result<(usize, HashSet<(usize, usize)>, Vec<(usize, usize, usize)>)> {
        let overflow =
            || OxmlError::InvalidValue("normalization affected-source work overflow".into());
        // Normalize this selected packet before intersecting the existing union.
        // This is actual range provenance, not an antichain of local envelopes.
        physical.sort_unstable();
        let mut packet = Vec::<(usize, usize, usize)>::new();
        for (part, start, end) in physical {
            if let Some(last) = packet.last_mut()
                && last.0 == part
                && start <= last.2
            {
                last.2 = last.2.max(end);
            } else {
                packet.push((part, start, end));
            }
        }
        let mut added_raw = 0usize;
        let mut removed = HashSet::new();
        let mut updates = Vec::<(usize, usize, usize)>::new();
        for (part, start, end) in packet {
            let mut merged_start = start;
            let mut merged_end = end;
            let mut overlap = 0usize;
            let mut predecessor = None;
            if let Some((&(old_part, old_start), &old_end)) = self
                .original_source_ranges
                .range(..=(part, start))
                .next_back()
                && old_part == part
                && old_end >= start
            {
                predecessor = Some(old_start);
                merged_start = old_start;
                merged_end = merged_end.max(old_end);
                overlap = overlap
                    .checked_add(old_end.min(end).saturating_sub(start))
                    .ok_or_else(overflow)?;
                removed.insert((part, old_start));
            }
            for (&(_, old_start), &old_end) in self
                .original_source_ranges
                .range((part, start)..=(part, end))
            {
                if predecessor != Some(old_start) {
                    overlap = overlap
                        .checked_add(old_end.min(end).saturating_sub(old_start))
                        .ok_or_else(overflow)?;
                }
                removed.insert((part, old_start));
                merged_end = merged_end.max(old_end);
            }
            added_raw = added_raw
                .checked_add((end - start).checked_sub(overlap).ok_or_else(overflow)?)
                .ok_or_else(overflow)?;
            if let Some(last) = updates.last_mut()
                && last.0 == part
                && merged_start <= last.2
            {
                last.2 = last.2.max(merged_end);
            } else {
                updates.push((part, merged_start, merged_end));
            }
        }
        Ok((added_raw, removed, updates))
    }

    /// Select only original physical Field bytes reached by this actual copy.
    /// Discovery and semantic-current input membership stay unchanged. The
    /// copy itself, `bytes` long, never exhausts the output ledger.
    pub(crate) fn reserve_source_publication_copy(
        &mut self,
        map: &crate::document::WordSourceMap,
        fields: &[std::ops::Range<usize>],
        bytes: usize,
    ) -> Result<()> {
        if self.resource_refused() {
            return Err(OxmlError::InvalidValue(
                "normalized result exceeds its preflight source work".into(),
            ));
        }
        let mut physical = Vec::new();
        for range in fields {
            physical.extend(map.slice(range.clone())?.original_pieces());
        }
        let (added, removed, updates) = self.original_source_union(physical)?;
        let overflow =
            || OxmlError::InvalidValue("normalization affected-source work overflow".into());
        let constructor_allowance = self
            .constructor_allowance
            .checked_add(added)
            .ok_or_else(overflow)?;
        let mut output = self
            .normalized_output
            .lock()
            .expect("normalization ledger poisoned");
        // A verbatim copy pays for itself: its bytes go to both sides of the
        // output ledger, which bounds what normalization generates, not how
        // often a pipeline copies its input. Rendering a template copies each
        // block about twenty times, so a field beside a tag ran out of work.
        let allowance = output.allowance.checked_add(bytes).ok_or_else(overflow)?;
        let emitted = output.emitted.checked_add(bytes).ok_or_else(overflow)?;
        for key in removed {
            self.original_source_ranges.remove(&key);
        }
        for (part, start, end) in updates {
            self.original_source_ranges.insert((part, start), end);
        }
        self.constructor_allowance = constructor_allowance;
        output.allowance = allowance;
        output.emitted = emitted;
        Ok(())
    }

    pub(super) fn prepare_leaf_actions(&mut self, actions: usize) -> Result<usize> {
        if let Some(reserved) = self
            .constructor_actions
            .checked_add(actions)
            .filter(|n| *n <= self.constructor_allowance)
        {
            return Ok(reserved);
        }
        self.resource_refused = true;
        Err(OxmlError::InvalidValue(
            "normalized result exceeds its preflight source work".into(),
        ))
    }

    pub(crate) fn resource_refused(&self) -> bool {
        self.resource_refused
            || self
                .normalized_output
                .lock()
                .expect("normalization ledger poisoned")
                .refused
            || self
                .field_origins
                .lock()
                .expect("field origin registry poisoned")
                .source_resource_refused
    }

    pub(crate) fn retained_revision_transport<'a>(
        &self,
        source: &'a [u8],
    ) -> Result<RetainedRevisionTransport<'a>> {
        let start = source.as_ptr() as usize;
        let end = start.checked_add(source.len()).ok_or_else(|| {
            OxmlError::InvalidValue("revision source transport range overflow".into())
        })?;
        let mut work = self
            .normalized_output
            .lock()
            .expect("normalization ledger poisoned");
        let previous = work
            .retained_revision
            .replace(RetainedRevisionRange { next: start, end });
        Ok(RetainedRevisionTransport {
            _source: source,
            previous,
            work: Arc::clone(&self.normalized_output),
        })
    }

    pub(super) fn normalized_output(&self, local_limit: Option<usize>) -> NormalizedWordOutput {
        NormalizedWordOutput {
            bytes: Vec::new(),
            source_map: crate::document::WordSourceMap::generated(0),
            local_limit,
            field_only: false,
            new_source_record: false,
            final_writer: None,
            word_capture: None,
            work: Arc::clone(&self.normalized_output),
        }
    }

    pub(crate) fn model_source_output(&self) -> NormalizedWordOutput {
        let mut output = self.normalized_output(None);
        output.field_only = true;
        output
    }

    /// Only the actual invocation-borrowed current/frozen pairing enters this
    /// writer. Nested normalization sinks have no capture and cannot stamp it.
    pub(crate) fn template_word_source_context(
        &mut self,
        text_origins: &[(&CT_Text, &CT_Text)],
        field_origins: &[(&Field, &Field)],
        raw_origins: &[(&[u8], &[u8])],
    ) -> Result<TemplateWordWriterLease<'_>> {
        // Field ingress holds this private record alive. Shared immutable
        // storage proves only retained XML transport, independently of either
        // owner's mutable CachedResult scalar slot.
        let field_records = field_origins.iter().filter_map(|(current, frozen)| {
            if !Arc::ptr_eq(&current.source, &frozen.source) {
                return None;
            }
            match (current.source.as_ref(), frozen.source.as_ref()) {
                (
                    FieldSource::Parsed {
                        raw_xml: current, ..
                    },
                    FieldSource::Parsed {
                        raw_xml: frozen, ..
                    },
                ) => Some((current.as_slice(), frozen.as_slice())),
                _ => None,
            }
        });
        let mut raw_ranges = raw_origins
            .iter()
            .copied()
            .chain(field_records)
            .map(|(current, frozen)| {
                let start = current.as_ptr() as usize;
                let end = start.checked_add(current.len()).ok_or_else(|| {
                    OxmlError::InvalidValue("current Word record position overflow".into())
                })?;
                Ok((start, end, frozen.as_ptr() as usize))
            })
            .collect::<Result<Vec<_>>>()?;
        raw_ranges.sort_unstable();
        let mut merged: Vec<(usize, usize, usize)> = Vec::new();
        for (start, end, frozen) in raw_ranges {
            if let Some(previous) = merged.last_mut()
                && start < previous.1
            {
                if previous.2.checked_add(start - previous.0) != Some(frozen) {
                    return Err(OxmlError::InvalidValue(
                        "conflicting current Word record relationships".into(),
                    ));
                }
                previous.1 = previous.1.max(end);
            } else {
                merged.push((start, end, frozen));
            }
        }
        let raw_origins = merged
            .into_iter()
            .map(|(start, end, frozen)| (start, (end, frozen)))
            .collect();
        let mut fields = std::collections::HashMap::new();
        let mut receipts = std::collections::HashMap::new();
        for (current, frozen) in field_origins {
            if current.cached_result != frozen.cached_result {
                continue;
            }
            let frozen_key = std::ptr::from_ref(*frozen) as usize;
            let receipt = self.template_receipt_fact(current);
            if receipt.is_some_and(|(_, fact)| fact.frozen != frozen_key || !fact.scalar) {
                continue;
            }
            let origin = TemplateScalarOrigin {
                origin: TemplateWordOrigin::CachedResult(frozen_key),
                decoded: 0..frozen.cached_result.len(),
            };
            fields.insert(std::ptr::from_ref(*current) as usize, origin.clone());
            if let Some((id, _)) = receipt {
                receipts.insert(id, origin);
            }
        }
        let capture = Arc::new(std::sync::Mutex::new(TemplateWordCapture {
            note_drawing_bindings: false,
            fields: Arc::new(fields),
            receipts: Arc::new(std::sync::Mutex::new(receipts)),
            scalar: None,
            produced_texts: std::collections::HashMap::new(),
            returned_raw: Default::default(),
            text_origins: Arc::new(
                text_origins
                    .iter()
                    .map(|(current, frozen)| {
                        (
                            std::ptr::from_ref(*current) as usize,
                            (
                                TemplateWordOrigin::Text(std::ptr::from_ref(*frozen) as usize),
                                frozen.text.len(),
                            ),
                        )
                    })
                    .collect(),
            ),
            raw_origins: Arc::new(raw_origins),
            output: Arc::clone(&self.normalized_output),
        }));
        let previous = self.template_word_capture.replace(capture);
        Ok(TemplateWordWriterLease {
            previous,
            work: self,
        })
    }

    pub(crate) fn template_word_source_output(
        &mut self,
        text_origins: &[(&CT_Text, &CT_Text)],
        field_origins: &[(&Field, &Field)],
        raw_origins: &[(&[u8], &[u8])],
    ) -> Result<(NormalizedWordOutput, TemplateWordWriterLease<'_>)> {
        let lease = self.template_word_source_context(text_origins, field_origins, raw_origins)?;
        let mut output = lease.work.model_source_output();
        output.word_capture = lease.work.template_word_capture.clone();
        Ok((output, lease))
    }

    pub(crate) fn template_read_source_output(
        &mut self,
        texts: &[(&CT_Text, TemplateWordEvent)],
        raws: &[(&[u8], crate::document::WordSourceMap)],
        fields: &[(&Field, &Field)],
    ) -> Result<(NormalizedWordOutput, TemplateWordWriterLease<'_>)> {
        let (output, lease) = self.template_word_source_output(&[], fields, &[])?;
        {
            let mut capture = lease
                .work
                .template_word_capture
                .as_ref()
                .expect("actual reader writer lease")
                .lock()
                .expect("Word text capture poisoned");
            for (text, event) in texts {
                capture.produced_texts.insert(
                    std::ptr::from_ref(*text) as usize,
                    TemplateScalarOrigin {
                        origin: event.origin.clone(),
                        decoded: event.decoded.clone(),
                    },
                );
            }
            for (raw, map) in raws {
                let start = raw.as_ptr() as usize;
                let end = start.checked_add(raw.len()).ok_or_else(|| {
                    OxmlError::InvalidValue("returned Word record overflow".into())
                })?;
                capture
                    .returned_raw
                    .insert(start, (end, map.clone().word_only()));
            }
        }
        Ok((output, lease))
    }

    // Preserve the surrounding synchronous frozen-input context while adding
    // this actual successful Revision reader's numeric Text/raw facts.
    pub(crate) fn template_revision_read_context(
        &mut self,
        texts: &[(&CT_Text, TemplateWordEvent)],
        raws: &[(&[u8], crate::document::WordSourceMap)],
    ) -> Result<TemplateWordWriterLease<'_>> {
        let (_, lease) = self.template_read_source_output(texts, raws, &[])?;
        if let Some(previous) = &lease.previous {
            let previous = previous.lock().expect("Word text capture poisoned");
            let mut current = lease
                .work
                .template_word_capture
                .as_ref()
                .expect("revision reader lease")
                .lock()
                .expect("Word text capture poisoned");
            current.fields = Arc::clone(&previous.fields);
            current.receipts = Arc::clone(&previous.receipts);
            current.text_origins = Arc::clone(&previous.text_origins);
            current.raw_origins = Arc::clone(&previous.raw_origins);
            for (key, origin) in &previous.produced_texts {
                current
                    .produced_texts
                    .entry(*key)
                    .or_insert_with(|| origin.clone());
            }
            for (key, value) in &previous.returned_raw {
                current
                    .returned_raw
                    .entry(*key)
                    .or_insert_with(|| value.clone());
            }
        }
        Ok(lease)
    }

    pub(crate) fn prepare_note_drawing_bindings(&mut self) {
        if let Some(capture) = &self.template_word_capture {
            capture
                .lock()
                .expect("Word text capture poisoned")
                .note_drawing_bindings = true;
        }
    }

    pub(crate) fn template_field_anchor(&mut self, anchor: &Arc<()>) {
        if self.template_field_registry.is_some() {
            return;
        }
        self.template_field_registry = Some(TemplateFieldRegistry {
            anchor: Arc::downgrade(anchor),
            next: 0,
            facts: std::collections::HashMap::new(),
        });
    }

    fn template_receipt_fact(&self, field: &Field) -> Option<(usize, &TemplatePreparedFieldFact)> {
        let (anchor, id) = field.template_word_receipt.as_ref()?;
        let registry = self.template_field_registry.as_ref()?;
        if !std::sync::Weak::ptr_eq(anchor, &registry.anchor) || anchor.strong_count() == 0 {
            return None;
        }
        Some((*id, registry.facts.get(id)?))
    }

    // Only the actual operation-minted returned Field receipt can supply this
    // ingress key. A candidate value/address or equal cache cannot discover it.
    pub(crate) fn prepared_template_field_origin(&self, field: &Field) -> Option<usize> {
        self.template_receipt_fact(field)
            .map(|(_, fact)| fact.frozen)
    }

    pub(super) fn template_mint_field(
        &mut self,
        current: &Field,
        frozen: Option<&Field>,
        cloned: &mut Field,
    ) -> Result<()> {
        let prior = self
            .template_receipt_fact(current)
            .map(|(_, fact)| (fact.frozen, fact.scalar));
        let direct = self.template_word_capture.as_ref().and_then(|capture| {
            let capture = capture.lock().expect("Word text capture poisoned");
            match capture
                .fields
                .get(&(std::ptr::from_ref(current) as usize))?
                .origin
            {
                TemplateWordOrigin::CachedResult(frozen) => Some((frozen, true)),
                _ => None,
            }
        });
        let fact = prior
            .or_else(|| {
                frozen.map(|frozen| {
                    (
                        std::ptr::from_ref(frozen) as usize,
                        current.cached_result == frozen.cached_result,
                    )
                })
            })
            .or(direct)
            // A derived prepared owner needs an installer receipt too, but no
            // frozen scalar key or incoming credit follows from this receipt.
            .or_else(|| {
                self.field_origins
                    .lock()
                    .expect("field origin registry poisoned")
                    .source_map(current)
                    .map(|_| (0, false))
            });
        let Some((frozen, scalar)) = fact else {
            return Ok(());
        };
        let Some(registry) = &mut self.template_field_registry else {
            return Ok(());
        };
        let next = registry
            .next
            .checked_add(1)
            .ok_or_else(|| OxmlError::InvalidValue("prepared Field occurrence overflow".into()))?;
        registry.next = next;
        registry
            .facts
            .insert(next, TemplatePreparedFieldFact { frozen, scalar });
        cloned.template_word_receipt = Some((registry.anchor.clone(), next));
        if scalar && let Some(capture) = &self.template_word_capture {
            let origin = capture
                .lock()
                .expect("Word text capture poisoned")
                .fields
                .get(&(std::ptr::from_ref(current) as usize))
                .cloned();
            if let Some(origin) = origin {
                capture
                    .lock()
                    .expect("Word text capture poisoned")
                    .receipts
                    .lock()
                    .expect("Word receipt capture poisoned")
                    .insert(next, origin);
            }
        }
        Ok(())
    }

    pub(super) fn template_generated_field_cache(&mut self, field: &mut Field) -> Result<()> {
        let prior = self
            .template_receipt_fact(field)
            .map(|(_, fact)| fact.frozen);
        let Some(frozen) = prior else {
            return Ok(());
        };
        let registry = self
            .template_field_registry
            .as_mut()
            .expect("resolved receipt registry");
        let next = registry
            .next
            .checked_add(1)
            .ok_or_else(|| OxmlError::InvalidValue("prepared Field occurrence overflow".into()))?;
        registry.next = next;
        registry.facts.insert(
            next,
            TemplatePreparedFieldFact {
                frozen,
                scalar: false,
            },
        );
        field.template_word_receipt = Some((registry.anchor.clone(), next));
        Ok(())
    }

    pub(super) fn template_field_scalar<'a>(
        &self,
        field: &Field,
        value: &'a str,
    ) -> Option<TemplateScalarLease<'a>> {
        let capture = Arc::clone(self.template_word_capture.as_ref()?);
        let mut facts = capture.lock().expect("Word text capture poisoned");
        let origin = facts
            .fields
            .get(&(std::ptr::from_ref(field) as usize))
            .cloned()
            .or_else(|| {
                let (id, fact) = self.template_receipt_fact(field)?;
                fact.scalar
                    .then(|| {
                        facts
                            .receipts
                            .lock()
                            .expect("Word receipt capture poisoned")
                            .get(&id)
                            .cloned()
                    })
                    .flatten()
            })?;
        if origin.decoded.len() != value.len() {
            return None;
        }
        let previous = facts.scalar.replace(TemplateScalarBorrow {
            start: value.as_ptr() as usize,
            length: value.len(),
            origin,
        });
        drop(facts);
        Some(TemplateScalarLease {
            _value: value,
            capture,
            previous,
        })
    }

    pub(crate) fn template_word_raw_map(
        &self,
        bytes: &[u8],
    ) -> Result<crate::document::WordSourceMap> {
        let mut map = crate::document::WordSourceMap::generated(bytes.len());
        if let Some(capture) = &self.template_word_capture
            && let Some(origin) = capture
                .lock()
                .expect("Word text capture poisoned")
                .raw_range(bytes)?
        {
            map = map.with_word_range(origin)?;
        }
        if let Some(capture) = &self.template_word_capture
            && let Some(returned) = capture
                .lock()
                .expect("Word text capture poisoned")
                .returned_raw_map(bytes)?
        {
            map = map.with_retained_word_source(&returned)?;
        }
        Ok(map)
    }

    /// A real nested glossary record has its own output coordinates. Keep the
    /// invocation's borrowed input relationships, but not another sink's events.
    pub(crate) fn template_word_record_output(
        &mut self,
    ) -> (NormalizedWordOutput, TemplateWordWriterLease<'_>) {
        let capture = self.template_word_capture.clone();
        let previous = std::mem::replace(&mut self.template_word_capture, capture.clone());
        let mut output = self.model_source_output();
        output.word_capture = capture;
        (
            output,
            TemplateWordWriterLease {
                previous,
                work: self,
            },
        )
    }

    pub(crate) fn template_word_capture(
        &self,
    ) -> Option<&Arc<std::sync::Mutex<TemplateWordCapture>>> {
        self.template_word_capture.as_ref()
    }

    pub(super) fn mapped_field_transport<'a>(
        &self,
        field: &Field,
        raw: &'a [u8],
    ) -> Result<Option<MappedFieldTransport<'a>>> {
        let FieldSource::Parsed { raw_xml, .. } = field.source.as_ref() else {
            return Ok(None);
        };
        // Only this actual borrowed source record can carry its constructor map.
        // Equal copied or surgically changed bytes have no such proof.
        if raw.len() != raw_xml.len() || raw.as_ptr() != raw_xml.as_ptr() {
            return Ok(None);
        }
        let Some(map) = self
            .field_origins
            .lock()
            .expect("field origin registry poisoned")
            .source_map(field)
        else {
            return Ok(None);
        };
        self.mapped_source_record(raw, map).map(Some)
    }

    /// The inline source record was captured alongside this exact original
    /// Field owner. Shared source ownership plus the existing selected-record
    /// validation forwards its map. Equal unrelated Fields cannot authorize it.
    pub(crate) fn mapped_inline_field_transport<'a>(
        &self,
        run: &CT_R,
        original: &CT_R,
        raw: &'a [u8],
    ) -> Result<Option<MappedFieldTransport<'a>>> {
        let ([RunContent::Field(field)], [RunContent::Field(original)]) =
            (run.content.as_slice(), original.content.as_slice())
        else {
            return Ok(None);
        };
        if !Arc::ptr_eq(&field.source, &original.source) {
            return Ok(None);
        }
        let FieldSource::Parsed { raw_xml, .. } = original.source.as_ref() else {
            return Ok(None);
        };
        if raw != raw_xml.as_slice() {
            return Ok(None);
        }
        let Some(map) = self
            .field_origins
            .lock()
            .expect("field origin registry poisoned")
            .source_map(original)
        else {
            return Ok(None);
        };
        self.mapped_source_record(raw, map).map(Some)
    }

    pub(crate) fn mapped_source_record<'a>(
        &self,
        raw: &'a [u8],
        map: crate::document::WordSourceMap,
    ) -> Result<MappedFieldTransport<'a>> {
        let start = raw.as_ptr() as usize;
        let end = start
            .checked_add(raw.len())
            .ok_or_else(|| OxmlError::InvalidValue("mapped source range overflow".into()))?;
        let mut output = self
            .normalized_output
            .lock()
            .expect("normalization ledger poisoned");
        let previous = output.mapped_transport.replace(MappedFieldRange {
            next: start,
            end,
            consumed: 0,
            map,
        });
        Ok(MappedFieldTransport {
            _source: raw,
            previous,
            work: Arc::clone(&self.normalized_output),
        })
    }

    pub(crate) fn field_xml_emission(&self) -> FieldXmlEmission {
        FieldXmlEmission {
            enabled: self.field_output_metric,
            output: Arc::clone(&self.normalized_output),
            word_capture: self.template_word_capture.clone(),
        }
    }

    pub(super) fn reserve_inner_xml(&self, bytes: usize) -> Result<bool> {
        self.field_xml_emission().reserve_inner_xml(bytes)
    }

    pub(super) fn prepaid_xml_transport<'a>(
        &self,
        produced: &'a [u8],
    ) -> Result<PrepaidXmlTransport<'a>> {
        self.field_xml_emission().prepaid_xml_transport(produced)
    }

    pub(super) fn field_publication(&self) -> FieldPublicationLease {
        let mut work = self
            .normalized_output
            .lock()
            .expect("normalization ledger poisoned");
        let previous = work.publication_field;
        work.publication_field = true;
        FieldPublicationLease {
            previous,
            work: Arc::clone(&self.normalized_output),
        }
    }

    pub(super) fn new_field_emission(&self) -> NewFieldEmission {
        let mut work = self
            .normalized_output
            .lock()
            .expect("normalization ledger poisoned");
        let previous = work.new_field;
        work.new_field = true;
        NewFieldEmission {
            previous,
            work: Arc::clone(&self.normalized_output),
        }
    }

    pub(crate) fn writer_error(&self, error: OxmlError) -> OxmlError {
        if self.resource_refused() {
            OxmlError::InvalidValue("normalized result exceeds its preflight source work".into())
        } else {
            error
        }
    }

    pub(super) fn complete_leaf_actions(&mut self, reserved: usize) {
        self.constructor_actions = reserved;
    }
}
impl ParsedFieldSummary {
    pub(super) fn add_child(&mut self, child: Self, delta: usize) -> Result<()> {
        self.affine.0 = self.affine.0.checked_add(child.affine.0).ok_or_else(|| {
            OxmlError::InvalidValue("normalization prospective child work overflow".into())
        })?;
        self.affine.1 = child
            .affine
            .0
            .checked_mul(delta)
            .and_then(|work| work.checked_add(child.affine.1))
            .and_then(|work| work.checked_add(self.affine.1))
            .ok_or_else(|| {
                OxmlError::InvalidValue("normalization prospective child work overflow".into())
            })?;
        self.maxima.0 = self.maxima.0.max(child.maxima.0);
        self.maxima.1 = self.maxima.1.max(child.maxima.1);
        Ok(())
    }

    pub(super) fn from_model(field: &Field) -> Result<Self> {
        Ok(Self {
            affine: field.normalization_work_summary()?,
            maxima: crate::document::word_model_maxima(crate::document::WordOwner::Field(field))?,
        })
    }
}

pub(super) enum ParsedFieldCache<'a> {
    Runs {
        runs: &'a [CT_R],
        summaries: Option<&'a [ParsedFieldSummary]>,
    },
    Ordered {
        paragraph: &'a CT_P,
        source_bytes: usize,
    },
}

pub(crate) fn ordered_normalization_children(paragraph: &CT_P) -> Result<Vec<(&Field, usize)>> {
    let mut children = Vec::new();
    for path in paragraph.accepted_run_paths() {
        let delta = path
            .segments()
            .iter()
            .filter(|segment| matches!(segment, AcceptedRunPathSegment::Revision(_)))
            .count()
            .checked_add(1)
            .ok_or_else(|| {
                OxmlError::InvalidValue("normalization source ancestry overflow".into())
            })?;
        let run = paragraph
            .accepted_run(&path)
            .ok_or_else(|| OxmlError::InvalidValue("normalization source path is stale".into()))?;
        for content in &run.content {
            if let RunContent::Field(child) = content {
                children.push((child, delta));
            }
        }
    }
    Ok(children)
}

#[allow(clippy::too_many_arguments)] // The actual source resource receipt is independent of capture facts.
pub(super) fn admit_initial_field_capture(
    instruction: &InstructionSyntax,
    nested_summaries: &[ParsedFieldSummary],
    cached_result: &str,
    raw_bytes: usize,
    scope: &[String],
    metadata: &ParsedFieldMetadata,
    cached: ParsedFieldCache<'_>,
    registry: Option<&Arc<std::sync::Mutex<FieldOriginRegistry>>>,
) -> Result<ParsedFieldSummary> {
    let record = |error: &OxmlError| {
        if let Some(registry) = registry {
            registry
                .lock()
                .expect("field origin registry poisoned")
                .record_source_resource_failure(error);
        }
    };
    let scope_bytes = checked_field_scope_bytes(scope).inspect_err(&record)?;
    let current = legacy_field_work_bytes(
        instruction.initial_work_bytes(cached_result, 0),
        metadata.legacy_form.as_ref(),
        metadata.reset_default.as_ref(),
        metadata.text_default.as_ref(),
        None,
    );
    let local = current
        .and_then(|current| {
            let inputs = NormalizationInputWork {
                retained_source: raw_bytes,
                scope: scope_bytes,
                current,
                checked_payload: [0, 0],
                stored_result: 0,
                pending_payload: 0,
            };
            inputs
                .dynamic_work()
                .and_then(|dynamic| inputs.source_work(dynamic))
        })
        .ok_or_else(|| {
            let error =
                OxmlError::InvalidValue("normalization prospective source scope overflow".into());
            record(&error);
            error
        })?;
    let mut result = ParsedFieldSummary {
        affine: (local, 0),
        maxima: (0, 0),
    };
    for operand in instruction.arguments.iter().chain(
        instruction
            .switches
            .iter()
            .filter_map(|(_, argument)| argument.as_ref()),
    ) {
        if let InstructionOperand::Nested(index) = operand {
            result
                .add_child(nested_summaries[*index], 0)
                .inspect_err(&record)?;
        }
    }
    match cached {
        ParsedFieldCache::Runs { runs, summaries } => {
            let mut summaries = summaries.map(|values| values.iter());
            for run in runs {
                for content in &run.content {
                    match content {
                        RunContent::Field(child) => {
                            let summary = if let Some(values) = &mut summaries {
                                *values.next().ok_or_else(|| {
                                    OxmlError::InvalidValue(
                                        "cached field occurrence summary is missing".into(),
                                    )
                                })?
                            } else {
                                ParsedFieldSummary::from_model(child)?
                            };
                            result.add_child(summary, 1).inspect_err(&record)?;
                        }
                        RunContent::Drawing(drawing) => {
                            let maxima = crate::document::word_model_maxima(
                                crate::document::WordOwner::Drawing(drawing),
                            )
                            .inspect_err(&record)?;
                            result.maxima.0 = result.maxima.0.max(maxima.0);
                            result.maxima.1 = result.maxima.1.max(maxima.1);
                        }
                        _ => {}
                    }
                }
                // Retained AlternateContent drawings are actual run children.
                // This is only the same ancestry maximum as the model walk,
                // never another affine/source allowance.
                for drawing in &run.alt_drawings {
                    let maxima = crate::document::word_model_maxima(
                        crate::document::WordOwner::Drawing(drawing),
                    )
                    .inspect_err(&record)?;
                    result.maxima.0 = result.maxima.0.max(maxima.0);
                    result.maxima.1 = result.maxima.1.max(maxima.1);
                }
            }
            if summaries.is_some_and(|mut values| values.next().is_some()) {
                return Err(OxmlError::InvalidValue(
                    "cached field occurrence summary is unconsumed".into(),
                ));
            }
        }
        ParsedFieldCache::Ordered {
            paragraph,
            source_bytes,
        } => {
            result.affine.0 = result.affine.0.checked_add(source_bytes).ok_or_else(|| {
                let error = OxmlError::InvalidValue(
                    "normalization prospective source scope overflow".into(),
                );
                record(&error);
                error
            })?;
            let maxima = crate::document::word_model_maxima(crate::document::WordOwner::Paragraph(
                paragraph,
            ))
            .inspect_err(&record)?;
            result.maxima.0 = result.maxima.0.max(maxima.0);
            result.maxima.1 = result.maxima.1.max(maxima.1);
            for (child, delta) in ordered_normalization_children(paragraph)? {
                result
                    .add_child(
                        ParsedFieldSummary {
                            affine: child.normalization_work_summary()?,
                            maxima: (0, 0),
                        },
                        delta,
                    )
                    .inspect_err(&record)?;
            }
            if metadata.form == FieldForm::Simple {
                result.maxima.0 =
                    crate::document::word_owner_cost(result.maxima.0, 1).inspect_err(&record)?;
            }
        }
    }
    result
        .affine
        .0
        .checked_add(result.affine.1)
        .ok_or_else(|| {
            let error =
                OxmlError::InvalidValue("normalization prospective child work overflow".into());
            record(&error);
            error
        })?;
    Ok(result)
}

pub(super) fn checked_field_scope_bytes(prefixes: &[String]) -> Result<usize> {
    prefixes
        .iter()
        .try_fold(0usize, |sum, prefix| sum.checked_add(prefix.len()))
        .ok_or_else(|| OxmlError::InvalidValue("normalization source scope overflow".into()))
}

/// Bytes one attribute value occupies once the writer escapes it. Collapsed
/// hyperlink reservations must charge the escaped form, not the raw form.
pub(super) fn escaped_source_len(value: &str) -> Result<usize> {
    value
        .bytes()
        .try_fold(0usize, |sum, byte| {
            sum.checked_add(escaped_source_piece(byte).map_or(1, <[u8]>::len))
        })
        .ok_or_else(|| OxmlError::InvalidValue("escaped source length overflow".into()))
}

// quick-xml 0.41 full escaping, plus the tab, line feed and carriage return
// that XML readers change: end-of-line handling reads a literal carriage
// return as a line feed, and attribute-value normalisation reads all three as
// spaces. Written as references they read back as written, in text and in
// attribute values alike. Both measurement and emission consume this
// selector, so an emitter writes [`escaped_source`], never quick-xml's own
// escaping.
pub(super) fn escaped_source_piece(byte: u8) -> Option<&'static [u8]> {
    match byte {
        b'&' => Some(b"&amp;"),
        b'<' => Some(b"&lt;"),
        b'>' => Some(b"&gt;"),
        b'\'' => Some(b"&apos;"),
        b'"' => Some(b"&quot;"),
        b'\t' => Some(b"&#9;"),
        b'\n' => Some(b"&#10;"),
        b'\r' => Some(b"&#13;"),
        _ => None,
    }
}

/// `value` escaped by [`escaped_source_piece`], as measured by
/// [`escaped_source_len`].
pub(crate) fn escaped_source(value: &str) -> std::borrow::Cow<'_, str> {
    if value
        .bytes()
        .all(|byte| escaped_source_piece(byte).is_none())
    {
        return std::borrow::Cow::Borrowed(value);
    }
    let mut output = Vec::with_capacity(value.len());
    append_escaped_source(&mut output, value);
    std::borrow::Cow::Owned(String::from_utf8(output).expect("escaping preserves UTF-8"))
}

pub(super) fn append_escaped_source(output: &mut Vec<u8>, value: &str) {
    let mut start = 0;
    for (index, byte) in value.bytes().enumerate() {
        if let Some(escaped) = escaped_source_piece(byte) {
            output.extend_from_slice(&value.as_bytes()[start..index]);
            output.extend_from_slice(escaped);
            start = index + 1;
        }
    }
    output.extend_from_slice(&value.as_bytes()[start..]);
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn copies_of_field_bytes_never_exhaust_the_ledger_but_generated_bytes_still_do() {
        // Field bytes a copy makes (no original), and original field bytes,
        // whose first reach credits only the constructors.
        for map in [
            crate::document::WordSourceMap::generated(65),
            crate::document::WordSourceMap::original(0..65),
        ] {
            let mut work = WordOperationWork::empty();
            let field = 0..65;
            for _ in 0..100 {
                work.reserve_source_publication_copy(&map, std::slice::from_ref(&field), 65)
                    .expect("a verbatim copy pays for itself");
            }
            assert!(!work.resource_refused());
            let mut output = work.normalized_output(None);
            assert!(
                output.write_all(b"<").is_err(),
                "a generated byte beyond the preflight work is refused"
            );
            assert!(work.resource_refused());
        }
    }
}
