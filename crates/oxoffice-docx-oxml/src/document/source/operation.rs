//! Scoped operations: building them, composing, materializing and finishing sources.

use super::*;

impl<'a> ScopedWordOperation<'a> {
    /// Borrow an original part without scanning or crediting unvisited bytes.
    pub fn new(original: &'a [u8]) -> Self {
        let field_work = crate::text::WordOperationWork::empty();
        let field_origins = std::sync::Arc::clone(&field_work.field_origins);
        Self {
            original_parts: vec![original],
            part_names: Default::default(),
            current_paragraphs: Default::default(),
            current_word_sources: Vec::new(),
            current_word_source_keys: Default::default(),
            current_word_views: Default::default(),
            current_texts: Default::default(),
            current_fields: Default::default(),
            field_work,
            template_datums: Default::default(),
            template_copy_work: Default::default(),
            field_origins,
            identity: std::sync::Arc::new(()),
            declarations: CaptureDeclarations::default(),
            ranges: Default::default(),
            source_bytes: 0,
            attributes: Default::default(),
            attribute_bytes: 0,
            capture_namespaces: Default::default(),
            capture_scope_bytes: 0,
            insertion_namespaces: Default::default(),
            insertion_scope_bytes: 0,
            capture_needed: 0,
            insertion_needed: 0,
        }
    }

    /// Freeze actual checked-ingress inputs without scanning or funding them.
    /// Part names select frozen OPC entries. They never register later bytes.
    pub fn with_inputs(
        originals: Vec<(&'a str, &'a [u8])>,
        current_paragraphs: Vec<&'a CT_P>,
    ) -> Result<Self> {
        let mut operation = Self::new(originals.first().map_or(&[], |(_, bytes)| *bytes));
        let mut names = std::collections::HashMap::new();
        for (index, (name, _)) in originals.iter().enumerate() {
            if names.insert(*name, index).is_some() {
                return Err(OxmlError::InvalidValue(
                    "duplicate original part name".into(),
                ));
            }
        }
        if !originals.is_empty() {
            operation.original_parts = originals.into_iter().map(|(_, bytes)| bytes).collect();
        }
        operation.part_names = names;
        operation.current_paragraphs = current_paragraphs
            .into_iter()
            .map(|paragraph| (std::ptr::from_ref(paragraph) as usize, paragraph))
            .collect();
        Ok(operation)
    }
}

impl<'a> ScopedWordOperation<'a> {
    /// Select only an original entry frozen at this checked operation's ingress.
    pub fn source_part(
        &mut self,
        part: &str,
        range: std::ops::Range<usize>,
        scope: &std::collections::BTreeMap<String, String>,
    ) -> Result<ScopedWordSource> {
        let index = *self.part_names.get(part).ok_or_else(|| {
            OxmlError::InvalidValue("source part is not in the frozen operation input".into())
        })?;
        let (bytes, constructors, _, source_map) = self.prepare_part(index, range, scope, None)?;
        Ok(ScopedWordSource {
            publication: Default::default(),
            initial_delivery: true,
            word_source_map: self.original_word_source_map(&source_map)?,
            bytes,
            source_map,
            word_events: Vec::new(),
            constructors: Some(std::sync::Arc::new(constructors)),
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        })
    }

    /// Materialize only this reached original range, carrying its correspondence.
    pub fn source(
        &mut self,
        range: std::ops::Range<usize>,
        scope: &std::collections::BTreeMap<String, String>,
    ) -> Result<ScopedWordSource> {
        let (bytes, constructors, _, source_map) = self.prepare(range, scope, None)?;
        Ok(ScopedWordSource {
            publication: Default::default(),
            initial_delivery: true,
            word_source_map: self.original_word_source_map(&source_map)?,
            bytes,
            source_map,
            word_events: Vec::new(),
            constructors: Some(std::sync::Arc::new(constructors)),
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        })
    }
}

impl<'a> ScopedWordOperation<'a> {
    fn original_word_source_map(&self, source: &WordSourceMap) -> Result<WordSourceMap> {
        let mut map = source.clone();
        for piece in &mut map.pieces {
            if let Some(original) = &mut piece.original {
                let part = self.original_parts.get(original.part).ok_or_else(|| {
                    OxmlError::InvalidValue("Word input part is not frozen".into())
                })?;
                let base = part.as_ptr() as usize;
                original.range = base
                    .checked_add(original.range.start)
                    .ok_or_else(|| OxmlError::InvalidValue("Word input position overflow".into()))?
                    ..base.checked_add(original.range.end).ok_or_else(|| {
                        OxmlError::InvalidValue("Word input position overflow".into())
                    })?;
                // All these ranges refer to actual live frozen backing bytes.
                // Absolute physical intervals also deduplicate overlapping views.
                original.part = 0;
            }
        }
        Ok(map)
    }

    /// Select an actual ingress-frozen serialization record as Word input only.
    pub fn source_current_word_record(
        &mut self,
        record: &'a [u8],
        range: std::ops::Range<usize>,
    ) -> Result<ScopedWordSource> {
        if !self
            .current_word_source_keys
            .contains(&(record.as_ptr() as usize, record.len()))
        {
            return Err(OxmlError::InvalidValue(
                "Word source record is not in the frozen operation input".into(),
            ));
        }
        let raw = record
            .get(range.clone())
            .ok_or_else(|| OxmlError::InvalidValue("Word source record range is invalid".into()))?;
        let admitted = admit_word_source(raw, &[], None, None)?;
        let base = record.as_ptr() as usize;
        let start = base
            .checked_add(range.start)
            .ok_or_else(|| OxmlError::InvalidValue("Word input position overflow".into()))?;
        let end = base
            .checked_add(range.end)
            .ok_or_else(|| OxmlError::InvalidValue("Word input position overflow".into()))?;
        Ok(ScopedWordSource {
            publication: Default::default(),
            initial_delivery: false,
            bytes: raw.to_vec(),
            source_map: WordSourceMap::generated(raw.len()),
            word_source_map: WordSourceMap::original(start..end),
            word_events: Vec::new(),
            constructors: Some(admitted.constructors),
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        })
    }

    /// Assemble actual selected carrier pieces without renewing any input role.
    pub fn compose_sources(
        &mut self,
        pieces: &[(Option<&ScopedWordSource>, std::ops::Range<usize>, &[u8])],
    ) -> Result<ScopedWordSource> {
        let assembly = self.source_assembly(pieces)?;
        if self.field_work.resource_refused() {
            return Err(self.field_work.writer_error(OxmlError::InvalidValue(
                "scoped operation resource admission refused".into(),
            )));
        }
        let admitted = admit_word_input(WordXmlInput::pieces(&assembly.chunks), &[], None, None)?;
        let mut coverage = assembly.source_map.materialized_publication();
        coverage.add_ranges(assembly.coverage.fields.clone());
        let copied = coverage.copied_field_bytes(assembly.length)?;
        // Admission and all fallible map/length preparation precede the one
        // actual copy reservation.
        self.field_work.reserve_source_publication_copy(
            &assembly.source_map,
            &coverage.fields,
            copied,
        )?;
        Ok(self.materialize_source(assembly, Some(admitted.constructors)))
    }

    /// Apply the actual part-root w14 declaration through checked composition.
    #[doc(hidden)]
    pub fn declare_source_w14_on_root(
        &mut self,
        source: ScopedWordSource,
    ) -> Result<ScopedWordSource> {
        self.check_source(&source)?;
        if self.field_work.resource_refused() {
            return Err(self.field_work.writer_error(OxmlError::InvalidValue(
                "scoped operation resource admission refused".into(),
            )));
        }
        let Some((at, declaration)) = crate::text::plan_w14_on_part_root(&source.bytes)? else {
            return Ok(source);
        };
        self.compose_sources(&[
            (Some(&source), 0..at, &[]),
            (None, 0..0, declaration.as_bytes()),
            (Some(&source), at..source.bytes.len(), &[]),
        ])
    }

    fn source_assembly<'s>(
        &self,
        pieces: &[(
            Option<&'s ScopedWordSource>,
            std::ops::Range<usize>,
            &'s [u8],
        )],
    ) -> Result<SourceAssembly<'s>> {
        let mut chunks = Vec::with_capacity(pieces.len());
        let mut coverage = WordPublicationCoverage::default();
        let mut source_map = WordSourceMap::generated(0);
        let mut word_source_map = WordSourceMap::generated(0);
        let mut word_events = Vec::new();
        let mut length = 0usize;
        for (source, range, generated) in pieces {
            let chunk = if let Some(source) = source {
                self.check_source(source)?;
                let captured = source
                    .publication
                    .lock()
                    .expect("publication coverage poisoned");
                for (output, ranges) in [
                    (&mut coverage.fields, &captured.fields),
                    (&mut coverage.ordinary, &captured.ordinary),
                ] {
                    let first = ranges.partition_point(|field| field.end <= range.start);
                    for field in ranges[first..]
                        .iter()
                        .take_while(|field| field.start < range.end)
                    {
                        let start = field.start.max(range.start);
                        let end = field.end.min(range.end);
                        if start < end {
                            output.push(
                                length.checked_add(start - range.start).ok_or_else(|| {
                                    OxmlError::InvalidValue("publication position overflow".into())
                                })?
                                    ..length.checked_add(end - range.start).ok_or_else(|| {
                                        OxmlError::InvalidValue(
                                            "publication position overflow".into(),
                                        )
                                    })?,
                            );
                        }
                    }
                }
                drop(captured);
                if !generated.is_empty() {
                    return Err(OxmlError::InvalidValue(
                        "selected source piece also supplies generated bytes".into(),
                    ));
                }
                let chunk = source.bytes.get(range.clone()).ok_or_else(|| {
                    OxmlError::InvalidValue("selected source piece range is invalid".into())
                })?;
                source_map.extend(&source.source_map.slice(range.clone())?)?;
                word_source_map.extend(&source.word_source_map.slice(range.clone())?)?;
                let first = source
                    .word_events
                    .partition_point(|event| event.encoded.end <= range.start);
                for event in source.word_events[first..]
                    .iter()
                    .take_while(|event| event.encoded.start < range.end)
                {
                    if range.start <= event.encoded.start && event.encoded.end <= range.end {
                        let mut carried = event.clone();
                        carried.encoded = length
                            .checked_add(event.encoded.start - range.start)
                            .ok_or_else(|| {
                                OxmlError::InvalidValue("Word event position overflow".into())
                            })?
                            ..length
                                .checked_add(event.encoded.end - range.start)
                                .ok_or_else(|| {
                                    OxmlError::InvalidValue("Word event position overflow".into())
                                })?;
                        word_events.push(carried);
                    }
                }
                chunk
            } else {
                if *range != (0..0) {
                    return Err(OxmlError::InvalidValue(
                        "generated source piece has a selected range".into(),
                    ));
                }
                source_map.extend(&WordSourceMap::generated(generated.len()))?;
                word_source_map.extend(&WordSourceMap::generated(generated.len()))?;
                *generated
            };
            length = length.checked_add(chunk.len()).ok_or_else(|| {
                OxmlError::InvalidValue("composed Word source byte overflow".into())
            })?;
            chunks.push(chunk);
        }
        coverage.add_ordinary_ranges(Vec::new());
        Ok(SourceAssembly {
            chunks,
            coverage,
            source_map,
            word_source_map,
            word_events,
            length,
        })
    }

    fn materialize_source(
        &self,
        assembly: SourceAssembly<'_>,
        constructors: Option<std::sync::Arc<ConstructorFailures>>,
    ) -> ScopedWordSource {
        let mut bytes = Vec::with_capacity(assembly.length);
        for chunk in assembly.chunks {
            bytes.extend_from_slice(chunk);
        }
        ScopedWordSource {
            publication: std::sync::Arc::new(std::sync::Mutex::new(assembly.coverage)),
            initial_delivery: false,
            bytes,
            source_map: assembly.source_map,
            word_source_map: assembly.word_source_map,
            word_events: assembly.word_events,
            constructors,
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        }
    }

    /// Close the same source through the existing lexical insertion plan.
    pub fn close_source_namespaces(
        &mut self,
        source: ScopedWordSource,
        scope: &std::collections::BTreeMap<String, String>,
    ) -> Result<ScopedWordSource> {
        self.check_source(&source)?;
        if self.field_work.resource_refused() {
            return Err(self.field_work.writer_error(OxmlError::InvalidValue(
                "scoped operation resource admission refused".into(),
            )));
        }
        let mut pool = CaptureDeclarations::default();
        let closure = FragmentNamespaceClosure::read(&source.bytes, scope, &mut pool)?;
        let length = source
            .bytes
            .len()
            .checked_add(closure.bytes)
            .ok_or_else(|| OxmlError::InvalidValue("content fragment byte overflow".into()))?;
        let source_map = source
            .source_map
            .after_insertion(closure.insertion, closure.bytes)?;
        let word_source_map = source
            .word_source_map
            .after_insertion(closure.insertion, closure.bytes)?;
        let previous = source.current_publication();
        let copied = previous.copied_field_bytes(source.bytes.len())?;
        let mut coverage = WordPublicationCoverage::default();
        for (output, ranges) in [
            (&mut coverage.fields, &previous.fields),
            (&mut coverage.ordinary, &previous.ordinary),
        ] {
            for range in ranges {
                if range.start < closure.insertion {
                    output.push(range.start..range.end.min(closure.insertion));
                }
                if range.end > closure.insertion {
                    output.push(
                        range.start.max(closure.insertion) + closure.bytes
                            ..range.end + closure.bytes,
                    );
                }
            }
        }
        coverage.add_ordinary_ranges(Vec::new());
        let mut word_events = source.word_events;
        for event in &mut word_events {
            if event.encoded.start >= closure.insertion {
                event.encoded.start =
                    event
                        .encoded
                        .start
                        .checked_add(closure.bytes)
                        .ok_or_else(|| {
                            OxmlError::InvalidValue("namespace Word event position overflow".into())
                        })?;
                event.encoded.end =
                    event
                        .encoded
                        .end
                        .checked_add(closure.bytes)
                        .ok_or_else(|| {
                            OxmlError::InvalidValue("namespace Word event position overflow".into())
                        })?;
            }
        }
        let (bytes, constructors) = if closure.bytes == 0 {
            // This is a genuine move, preserving the existing admission and
            // generated-delivery policy without manufacturing a byte copy.
            let admitted = admit_word_source(&source.bytes, &[], None, None)?;
            (source.bytes, admitted.constructors)
        } else {
            let mut declarations = Vec::with_capacity(closure.bytes);
            closure.write_declarations(&mut declarations, &pool);
            let chunks = [
                &source.bytes[..closure.insertion],
                declarations.as_slice(),
                &source.bytes[closure.insertion..],
            ];
            let admitted = admit_word_input(WordXmlInput::pieces(&chunks), &[], None, None)?;
            self.field_work.reserve_source_publication_copy(
                &source.source_map,
                &previous.fields,
                copied,
            )?;
            let mut bytes = Vec::with_capacity(length);
            for chunk in chunks {
                bytes.extend_from_slice(chunk);
            }
            (bytes, admitted.constructors)
        };
        Ok(ScopedWordSource {
            publication: std::sync::Arc::new(std::sync::Mutex::new(coverage)),
            initial_delivery: false,
            source_map,
            word_source_map,
            bytes,
            word_events,
            constructors: Some(constructors),
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        })
    }

    pub(super) fn check_source(&self, source: &ScopedWordSource) -> Result<()> {
        if !std::sync::Arc::ptr_eq(&self.identity, &source.operation) {
            return Err(OxmlError::InvalidValue(
                "source belongs to another scoped operation".into(),
            ));
        }
        Ok(())
    }

    /// Parse this operation's actual whole-document source through the same
    /// reader as the ordinary document factory. Returned Fields are positive
    /// observations only, never a complete ordinary or fieldless certificate.
    #[doc(hidden)]
    pub fn source_document(&mut self, source: &ScopedWordSource) -> Result<CT_Document> {
        self.check_source(source)?;
        if self.field_work.resource_refused() {
            return Err(self.field_work.writer_error(OxmlError::InvalidValue(
                "scoped operation resource admission refused".into(),
            )));
        }
        let map = source
            .source_map
            .with_word_source(&source.word_source_map, &source.word_events)?
            .with_publication_collector(&source.publication);
        let admitted = source.admitted_word_source()?.with_source_map(map)?;
        let publication = std::sync::Arc::clone(&admitted.source_map);
        let mut properties = Vec::new();
        let reads = std::sync::Arc::new(std::sync::Mutex::new(ReturnedWordRead::default()));
        let document = CT_Document::from_xml_admitted_with_properties(
            &source.bytes,
            admitted,
            Some(ScopedDocumentInput {
                registry: std::sync::Arc::clone(&self.field_origins),
                initial_delivery: source.initial_delivery,
                publication: std::sync::Arc::clone(&source.publication),
                reads: std::sync::Arc::clone(&reads),
            }),
            Some(&mut properties),
        )?;
        self.field_origins
            .lock()
            .expect("field origin registry poisoned")
            .commit_publication_owner(WordOwner::Body(&document.body), &publication);
        WordOrdinaryFact::publish_all(properties);
        *source
            .read_facts
            .lock()
            .expect("returned Word facts poisoned") = Some(std::mem::take(
            &mut *reads.lock().expect("returned Word facts poisoned"),
        ));
        Ok(document)
    }

    /// Inspect an actual operation source using its existing admission facts.
    pub fn source_paragraph(&mut self, source: &ScopedWordSource) -> Result<CT_P> {
        let mut ordinary = Vec::new();
        let mut extras = Vec::new();
        let mut texts = Vec::new();
        let map = source
            .source_map
            .with_word_source(&source.word_source_map, &source.word_events)?;
        let mut maps = crate::text::WordBlockSourceMaps::default();
        let paragraph = self.parse_source_paragraph_with_properties(
            source,
            Some(&mut extras),
            Some(&mut texts),
            Some(&map),
            Some(&mut ordinary),
            Some(&mut maps),
        )?;
        let mut roots = paragraph.writer_field_roots();
        roots.extend(
            crate::text::ordered_normalization_children(&paragraph)?
                .into_iter()
                .map(|(field, _)| field),
        );
        self.field_work.register_original_fields(roots)?;
        WordOrdinaryFact::publish_all(ordinary);
        maps.paragraph(extras);
        maps.word_texts.push((
            Vec::new(),
            Some(crate::text::WordParagraphReadFacts {
                source_map: std::sync::Arc::new(map),
                texts,
                nested: Default::default(),
            }),
        ));
        *source
            .read_facts
            .lock()
            .expect("returned Word facts poisoned") = Some(ReturnedWordRead {
            body: vec![(0, maps)],
            notes: Vec::new(),
            comments: None,
        });
        Ok(paragraph)
    }

    /// Read the actual note-family factory and retain its successful paths.
    #[doc(hidden)]
    pub fn source_notes(
        &mut self,
        source: &ScopedWordSource,
    ) -> Result<crate::footnotes::CT_Footnotes> {
        self.check_source(source)?;
        if self.field_work.resource_refused() {
            return Err(self.field_work.writer_error(OxmlError::InvalidValue(
                "scoped operation resource admission refused".into(),
            )));
        }
        let map = source
            .source_map
            .with_word_source(&source.word_source_map, &source.word_events)?
            .with_publication_collector(&source.publication);
        let admitted = source.admitted_word_source()?.with_source_map(map)?;
        let mut controls = admitted.controls_for(&source.bytes).with_field_origins(
            std::sync::Arc::clone(&self.field_origins),
            source.initial_delivery,
        );
        let mut reads = Vec::new();
        let notes = crate::footnotes::CT_Footnotes::from_xml_with_read_output(
            &source.bytes,
            &mut controls,
            Some(&mut reads),
        )?;
        for note in &notes.footnotes {
            for paragraph in &note.paragraphs {
                self.field_origins
                    .lock()
                    .expect("field origin registry poisoned")
                    .commit_publication_owner(
                        WordOwner::Paragraph(paragraph),
                        &admitted.source_map,
                    );
            }
        }
        *source
            .read_facts
            .lock()
            .expect("returned Word facts poisoned") = Some(ReturnedWordRead {
            body: Vec::new(),
            notes: reads,
            comments: None,
        });
        Ok(notes)
    }
}

impl<'a> ScopedWordOperation<'a> {
    /// Apply prepared native operands using this operation's actual parser,
    /// matched Field owners, child carriers and transactional borrowed kernel.
    #[allow(clippy::type_complexity)] // Exact approved native operand tuple.
    pub fn format_paragraph(
        &mut self,
        source: ScopedWordSource,
        mut children: Vec<(std::ops::Range<usize>, ScopedWordSource)>,
        occurrences: &[(
            usize,
            std::ops::Range<usize>,
            &str,
            Option<&crate::properties::CT_RPr>,
            Option<&std::cell::Cell<bool>>,
            &[&'a serde_json::Value],
        )],
    ) -> Result<ScopedWordSource> {
        self.check_source(&source)?;
        children.sort_by_key(|(range, _)| range.start);
        let mut previous = 0;
        let mut projected = source.bytes.len();
        for (range, child) in &children {
            self.check_source(child)?;
            if range.start < previous || range.start > range.end || range.end > source.bytes.len() {
                return Err(OxmlError::InvalidValue(
                    "invalid formatted child range".into(),
                ));
            }
            projected = projected
                .checked_sub(range.len())
                .and_then(|size| size.checked_add(child.bytes.len()))
                .ok_or_else(|| OxmlError::InvalidValue("formatted child source overflow".into()))?;
            previous = range.end;
        }
        let source = if children.is_empty() {
            source
        } else {
            let capacity = children
                .len()
                .checked_mul(2)
                .and_then(|count| count.checked_add(1))
                .ok_or_else(|| OxmlError::InvalidValue("formatted child count overflow".into()))?;
            let mut pieces = Vec::with_capacity(capacity);
            let mut at = 0;
            for (range, child) in &children {
                pieces.push((Some(&source), at..range.start, &[][..]));
                pieces.push((Some(child), 0..child.bytes.len(), &[][..]));
                at = range.end;
            }
            pieces.push((Some(&source), at..source.bytes.len(), &[][..]));
            self.compose_sources(&pieces)?
        };
        let mut maps = Vec::new();
        let word_map = source
            .source_map
            .with_word_source(&source.word_source_map, &source.word_events)?;
        let mut paragraph =
            self.parse_source_paragraph(&source, Some(&mut maps), None, Some(&word_map))?;
        let borrowed = occurrences
            .iter()
            .map(|(stretch, range, value, properties, _, _)| {
                (*stretch, range.clone(), *value, *properties)
            })
            .collect::<Vec<_>>();
        let operands = occurrences
            .iter()
            .map(|(_, _, _, _, callback, dependencies)| (*callback, *dependencies))
            .collect::<Vec<_>>();
        crate::placeholder::replace_prepared_occurrences_borrowed_in(
            &mut paragraph,
            &borrowed,
            Some(&mut maps),
            Some(&mut self.field_work),
            Some(crate::placeholder::PreparedTemplateInputs {
                datums: &self.template_datums,
                operands: &operands,
            }),
        )?;
        let mut roots = paragraph.writer_field_roots();
        roots.extend(
            crate::text::ordered_normalization_children(&paragraph)?
                .into_iter()
                .map(|(field, _)| field),
        );
        self.field_work.register_original_fields(roots)?;
        let (bytes, source_map, word_events, scope) = {
            let _scope = crate::text::raw_root_binding_scope(&source.bytes, true)?;
            let scope = crate::text::emitted_bindings()
                .bindings()
                .into_iter()
                .map(|(prefix, uri)| (prefix.to_owned(), uri.to_owned()))
                .collect::<std::collections::BTreeMap<_, _>>();
            let (bytes, map, events) = crate::text::final_word_source(
                crate::text::FinalWordBlock::Paragraph(&paragraph),
                &mut self.field_work,
            )?;
            (bytes, map, events, scope)
        };
        let admitted = admit_word_source(&bytes, &[], None, None)?;
        let source = ScopedWordSource {
            publication: source_map.publication_output(),
            initial_delivery: false,
            word_source_map: WordSourceMap::generated(bytes.len()),
            bytes,
            source_map,
            word_events,
            constructors: Some(admitted.constructors),
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        };
        self.close_source_namespaces(source, &scope)
    }

    fn parse_source_paragraph(
        &mut self,
        source: &ScopedWordSource,
        extra_maps: Option<&mut crate::text::WordExtraSourceMaps>,
        text_facts: Option<&mut crate::text::WordParagraphTextFacts>,
        word_map: Option<&WordSourceMap>,
    ) -> Result<CT_P> {
        self.parse_source_paragraph_with_properties(
            source, extra_maps, text_facts, word_map, None, None,
        )
    }

    pub(super) fn parse_source_paragraph_with_properties(
        &mut self,
        source: &ScopedWordSource,
        extra_maps: Option<&mut crate::text::WordExtraSourceMaps>,
        text_facts: Option<&mut crate::text::WordParagraphTextFacts>,
        word_map: Option<&WordSourceMap>,
        ordinary: Option<&mut Vec<WordOrdinaryFact>>,
        nested_maps: Option<&mut crate::text::WordBlockSourceMaps>,
    ) -> Result<CT_P> {
        self.check_source(source)?;
        let admitted = source.admitted_word_source()?.with_source_map(
            word_map
                .unwrap_or(&source.source_map)
                .clone()
                .with_publication_collector(&source.publication),
        )?;
        let paragraph = CT_P::from_xml_fragment_with_properties(
            &source.bytes,
            Some(
                &mut admitted.controls_for(&source.bytes).with_field_origins(
                    std::sync::Arc::clone(&self.field_origins),
                    source.initial_delivery,
                ),
            ),
            extra_maps,
            text_facts,
            ordinary,
            nested_maps,
        )?;
        self.field_origins
            .lock()
            .expect("field origin registry poisoned")
            .commit_publication_owner(WordOwner::Paragraph(&paragraph), &admitted.source_map);
        Ok(paragraph)
    }

    /// Copy one actual source occurrence without renewing any input role.
    /// Only positively observed Field intervals are selected for the field
    /// work ledger, which credits their first reach to the constructors; the
    /// copy itself never debits the output ledger. Their complement remains
    /// unproved, rather than certified ordinary.
    pub fn repeat_source(&mut self, source: &ScopedWordSource) -> Result<ScopedWordSource> {
        self.check_source(source)?;
        if self.field_work.resource_refused() {
            return Err(self.field_work.writer_error(OxmlError::InvalidValue(
                "normalized result exceeds its preflight source work".into(),
            )));
        }
        let mut coverage = source.source_map.materialized_publication();
        coverage.add_ranges(
            source
                .publication
                .lock()
                .expect("publication coverage poisoned")
                .fields
                .clone(),
        );
        coverage.add_ordinary_ranges(
            source
                .publication
                .lock()
                .expect("publication coverage poisoned")
                .ordinary
                .clone(),
        );
        let mut copied = 0usize;
        for range in &coverage.fields {
            if range.start > range.end || range.end > source.bytes.len() {
                return Err(OxmlError::InvalidValue(
                    "copied Field publication range is invalid".into(),
                ));
            }
            copied = copied.checked_add(range.len()).ok_or_else(|| {
                OxmlError::InvalidValue("copied Field publication work overflow".into())
            })?;
        }
        let mut source_map = source.source_map.clone();
        source_map.publication_owned = coverage.fields.clone();
        source_map.ordinary_owned = coverage.ordinary.clone();
        let word_source_map = source.word_source_map.clone();
        let word_events = source.word_events.clone();
        let constructors = source.constructors.clone();
        let operation = std::sync::Arc::clone(&self.identity);
        let publication = std::sync::Arc::new(std::sync::Mutex::new(coverage));
        // All fallible preparations precede both reservation and actual byte
        // growth. No fallible producer remains after this one reservation.
        self.field_work.reserve_source_publication_copy(
            &source_map,
            &source_map.publication_owned,
            copied,
        )?;
        let bytes = source.bytes.clone();
        Ok(ScopedWordSource {
            initial_delivery: false,
            bytes,
            publication,
            source_map,
            word_source_map,
            word_events,
            constructors,
            read_facts: Default::default(),
            operation,
        })
    }

    /// Final transport of a source already materialized by this operation.
    pub fn finish_source(&mut self, source: ScopedWordSource) -> Result<Vec<u8>> {
        self.check_source(&source)?;
        Ok(source.bytes)
    }

    fn prepare(
        &mut self,
        range: std::ops::Range<usize>,
        scope: &std::collections::BTreeMap<String, String>,
        root_owner: Option<SdtOwner>,
    ) -> Result<(Vec<u8>, ConstructorFailures, bool, WordSourceMap)> {
        self.prepare_part(0, range, scope, root_owner)
    }

    fn prepare_part(
        &mut self,
        part: usize,
        range: std::ops::Range<usize>,
        scope: &std::collections::BTreeMap<String, String>,
        root_owner: Option<SdtOwner>,
    ) -> Result<(Vec<u8>, ConstructorFailures, bool, WordSourceMap)> {
        let overflow = || OxmlError::InvalidValue("scoped original source work overflow".into());
        let original = self.original_parts.get(part).ok_or_else(|| {
            OxmlError::InvalidValue("source part is not in the frozen operation input".into())
        })?;
        let raw = original.get(range.clone()).ok_or_else(|| {
            OxmlError::InvalidValue("scoped original source range is invalid".into())
        })?;
        let closure = FragmentNamespaceClosure::read(raw, scope, &mut self.declarations)?;
        let mut inherited = Vec::with_capacity(scope.len());
        let mut aliases = Vec::new();
        for (prefix, namespace) in scope {
            inherited.push(self.declarations.intern(prefix, namespace)?);
            if namespace == W_NS {
                aliases.push(prefix.clone());
            }
        }
        let (constructors, capture) = plan_capture_with_context(
            WordXmlInput::Contiguous(raw),
            &inherited,
            &aliases,
            root_owner,
            None,
            &mut self.declarations,
            None,
        )?;

        // The disjoint interval map credits physical bytes, never equal content.
        let mut merged = range.clone();
        if let Some((&(_, start), &end)) = self
            .ranges
            .range((part, 0)..=(part, range.start))
            .next_back()
            && end >= range.start
        {
            merged.start = start;
            merged.end = merged.end.max(end);
        }
        let mut replaced = Vec::new();
        let mut replaced_bytes = 0usize;
        for (&(_, start), &end) in self.ranges.range((part, merged.start)..=(part, usize::MAX)) {
            if start > merged.end {
                break;
            }
            merged.end = merged.end.max(end);
            replaced.push(start);
            replaced_bytes = replaced_bytes
                .checked_add(end - start)
                .ok_or_else(overflow)?;
        }
        let source_bytes = self
            .source_bytes
            .checked_sub(replaced_bytes)
            .and_then(|bytes| bytes.checked_add(merged.len()))
            .ok_or_else(overflow)?;
        let mut new_attributes = std::collections::HashMap::new();
        let mut attribute_bytes = self.attribute_bytes;
        for (origin, bytes) in capture.attributes {
            let start = range.start.checked_add(origin.start).ok_or_else(overflow)?;
            let end = range.start.checked_add(origin.end).ok_or_else(overflow)?;
            if end > range.end {
                return Err(OxmlError::InvalidValue(
                    "scoped attribute origin is invalid".into(),
                ));
            }
            let key = (part, start, end);
            if let Some(old) = self
                .attributes
                .get(&key)
                .or_else(|| new_attributes.get(&key))
            {
                if *old != bytes {
                    return Err(OxmlError::InvalidValue(
                        "scoped attribute origin changed".into(),
                    ));
                }
            } else {
                attribute_bytes = attribute_bytes.checked_add(bytes).ok_or_else(overflow)?;
                new_attributes.insert(key, bytes);
            }
        }
        let mut new_capture = std::collections::HashSet::new();
        let mut capture_scope_bytes = self.capture_scope_bytes;
        for index in capture.namespaces {
            if !self.capture_namespaces.contains(&index) && new_capture.insert(index) {
                capture_scope_bytes = capture_scope_bytes
                    .checked_add(2)
                    .and_then(|bytes| bytes.checked_add(self.declarations.prefix(index).len()))
                    .and_then(|bytes| bytes.checked_add(self.declarations.namespace(index).len()))
                    .ok_or_else(overflow)?;
            }
        }
        let mut new_insertion = std::collections::HashSet::new();
        let mut insertion_scope_bytes = self.insertion_scope_bytes;
        for &index in &closure.bindings {
            if !self.insertion_namespaces.contains(&index) && new_insertion.insert(index) {
                insertion_scope_bytes = insertion_scope_bytes
                    .checked_add(self.declarations.prefix(index).len())
                    .and_then(|bytes| bytes.checked_add(self.declarations.namespace(index).len()))
                    .ok_or_else(overflow)?;
            }
        }
        let capture_needed = self
            .capture_needed
            .checked_add(capture.needed)
            .ok_or_else(overflow)?;
        let insertion_needed = self
            .insertion_needed
            .checked_add(closure.bytes)
            .ok_or_else(overflow)?;
        let capture_allowance = crate::text::retained_source_envelope(
            capture_scope_bytes
                .checked_add(attribute_bytes)
                .ok_or_else(overflow)?,
            source_bytes,
        )?;
        let insertion_allowance =
            crate::text::retained_source_envelope(insertion_scope_bytes, source_bytes)?;
        if capture_needed > capture_allowance {
            return Err(OxmlError::InvalidValue(
                "scoped capture exceeds original source work".into(),
            ));
        }
        if insertion_needed > insertion_allowance {
            return Err(OxmlError::InvalidValue(
                "scoped namespace insertion exceeds original source work".into(),
            ));
        }
        let source_map = WordSourceMap::original_part(part, range.clone())
            .after_insertion(closure.insertion, closure.bytes)?;
        let constructors = constructors
            .with_original_offset(part, range.start)?
            .after_insertion(closure.insertion, closure.bytes)?;
        let closed = closure.write(raw, &self.declarations)?;
        for start in replaced {
            self.ranges.remove(&(part, start));
        }
        self.ranges.insert((part, merged.start), merged.end);
        self.source_bytes = source_bytes;
        self.attributes.extend(new_attributes);
        self.attribute_bytes = attribute_bytes;
        self.capture_namespaces.extend(new_capture);
        self.capture_scope_bytes = capture_scope_bytes;
        self.insertion_namespaces.extend(new_insertion);
        self.insertion_scope_bytes = insertion_scope_bytes;
        self.capture_needed = capture_needed;
        self.insertion_needed = insertion_needed;
        Ok((closed, constructors, closure.original_w_binding, source_map))
    }

    pub fn paragraph(
        &mut self,
        range: std::ops::Range<usize>,
        scope: &std::collections::BTreeMap<String, String>,
    ) -> Result<CT_P> {
        let source = self.source(range, scope)?;
        self.source_paragraph(&source)
    }

    pub fn document_block(
        &mut self,
        range: std::ops::Range<usize>,
        scope: &std::collections::BTreeMap<String, String>,
    ) -> Result<CT_Document> {
        let (closed, constructors, original_w_binding, source_map) =
            self.prepare(range, scope, Some(SdtOwner::Body))?;
        let mut wrapped = format!("<w:document xmlns:w=\"{W_NS}\"><w:body>").into_bytes();
        let prefix_bytes = wrapped.len();
        wrapped.extend_from_slice(&closed);
        wrapped.extend_from_slice(b"</w:body></w:document>");
        if !original_w_binding {
            // This wrapper can activate w owners that did not exist in the
            // original qualifier's context, changing control cursor order.
            return CT_Document::from_xml(&wrapped);
        }
        let constructors = constructors.after_insertion(0, prefix_bytes)?;
        let source_map = WordSourceMap::concat(&[
            WordSourceMap::generated(prefix_bytes),
            source_map,
            WordSourceMap::generated(b"</w:body></w:document>".len()),
        ])?;
        let admitted = admit_word_input_with_constructors(
            WordXmlInput::Contiguous(&wrapped),
            &["w".to_owned()],
            None,
            None,
            constructors,
        )?
        .with_source_map(source_map)?;
        CT_Document::from_xml_admitted(&wrapped, admitted, None)
    }

    /// Replace one scoped block using this actual caller literal event. Paired
    /// source occurrences forward the same borrowed value and delivery flag.
    #[doc(hidden)]
    pub fn replace_block_with_literal_input(
        &mut self,
        range: std::ops::Range<usize>,
        scope: &std::collections::BTreeMap<String, String>,
        paragraph: Option<usize>,
        placeholder: &str,
        replacement: &str,
        delivered: &std::cell::Cell<bool>,
    ) -> Result<(Vec<u8>, usize)> {
        if self.field_work.resource_refused() {
            return Err(self.field_work.writer_error(OxmlError::InvalidValue(
                "scoped operation resource admission refused".into(),
            )));
        }
        let (mut closed, constructors, _, source_map) =
            self.prepare(range, scope, Some(SdtOwner::Body))?;
        let count = crate::placeholder::try_replace_in_block_with_constructors_in(
            &mut closed,
            paragraph,
            placeholder,
            replacement,
            Some((constructors, source_map)),
            Some(crate::placeholder::ScopedLiteralContext {
                work: &mut self.field_work,
                delivered,
                initial_delivery: true,
            }),
        )?;
        Ok((closed, count))
    }
}
