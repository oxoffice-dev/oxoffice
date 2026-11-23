//! The content control source cursor and admitted Word sources.

use super::*;

impl<'a> ControlSourceCursor<'a> {
    pub(crate) fn empty(&self) -> Self {
        Self {
            occurrences: self.occurrences,
            next: self.end,
            end: self.end,
            projected_aliases: self.projected_aliases,
            initial_field_delivery: self.initial_field_delivery,
            simple_headers: self.simple_headers,
            simple_selectors: &[],
            selector_paragraph_root: None,
            selector_requested: false,
            selector_ready: false,
            input: None,
            source_map: std::sync::Arc::clone(&self.source_map),
            field_origins: self.field_origins.clone(),
        }
    }
    pub(crate) fn field_registry(
        &self,
    ) -> Option<std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>>> {
        self.field_origins.clone()
    }

    pub(crate) fn initial_field_delivery(&self) -> bool {
        self.initial_field_delivery
    }

    pub(crate) fn has_field_origins(&self) -> bool {
        self.field_origins.is_some()
    }

    pub(crate) fn with_field_origins(
        mut self,
        registry: std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>>,
        initial_delivery: bool,
    ) -> Self {
        self.field_origins = Some(registry);
        self.initial_field_delivery = initial_delivery;
        self
    }

    /// Bind the completed same-reader paragraph transcript to this live input.
    pub(crate) fn paragraph_word_facts(
        &self,
        reader: &Reader<&[u8]>,
        texts: crate::text::WordParagraphTextFacts,
    ) -> Option<crate::text::WordParagraphReadFacts> {
        let input = self.input?;
        let consumed = usize::try_from(reader.buffer_position()).ok()?;
        let remaining = *reader.get_ref();
        if remaining.len().checked_add(consumed) != Some(input.len())
            || (input.as_ptr() as usize).checked_add(consumed) != Some(remaining.as_ptr() as usize)
        {
            return None;
        }
        Some(crate::text::WordParagraphReadFacts {
            source_map: std::sync::Arc::clone(&self.source_map),
            texts,
            nested: Default::default(),
        })
    }

    /// The same core capture emits its source/output transcript before this
    /// constructor input is rebound. Failure to prove transport stays inert.
    pub(crate) fn capture_element(
        &self,
        reader: &mut Reader<&[u8]>,
        start: &quick_xml::events::BytesStart<'_>,
        opening_start: u64,
    ) -> Result<(Vec<u8>, Option<WordFieldInput>)> {
        let (Some(input), Some(registry)) = (self.input, self.field_origins.as_ref()) else {
            return Ok((crate::raw_xml::capture_element(reader, start)?, None));
        };
        let Some(physical) = input.len().checked_sub(reader.get_ref().len()) else {
            return Ok((crate::raw_xml::capture_element(reader, start)?, None));
        };
        let logical = usize::try_from(reader.buffer_position()).ok();
        let opening = logical
            .and_then(|logical| physical.checked_sub(logical))
            .and_then(|offset| usize::try_from(opening_start).ok()?.checked_add(offset))
            .map(|start| start..physical);
        let Some(opening) = opening else {
            return Ok((crate::raw_xml::capture_element(reader, start)?, None));
        };
        let capture_start = opening.start;
        let capture_windows = !reader.config().trim_text_start && !reader.config().trim_text_end;
        let (bytes, ranges) =
            crate::raw_xml::capture_element_with_source_ranges(reader, start, input, opening)?;
        let Some(ranges) = ranges.filter(|_| self.source_map.len() == input.len()) else {
            // The existing fixed-point proof remains valid independently of
            // whether the optional new transcript supports this reader mode.
            let mapped = self.field_input(reader, opening_start..reader.buffer_position(), &bytes);
            return Ok((bytes, mapped));
        };
        let mut map = WordSourceMap::generated(0);
        let mut written = 0;
        for (source, produced) in ranges {
            map.extend(&WordSourceMap::generated(produced.start - written))?;
            map.extend(&self.source_map.slice(source)?)?;
            written = produced.end;
        }
        map.extend(&WordSourceMap::generated(bytes.len() - written))?;
        // Only this actual capture can create a nonlinear relation. The core
        // transcript proved supported transport, and trim(false) preserves all
        // consumed events apart from their known lexical normalization.
        map.publication_windows.clear();
        if capture_windows {
            let capture_end = input.len() - reader.get_ref().len();
            let captured = capture_start..capture_end;
            for piece in &self.source_map.publication {
                if piece.output.start <= captured.start && captured.end <= piece.output.end {
                    map.publication_windows.push(WordCaptureWindow {
                        output: 0..bytes.len(),
                        current: piece.current.start + captured.start - piece.output.start
                            ..piece.current.start + captured.end - piece.output.start,
                        collector: piece.collector.clone(),
                    });
                }
            }
            for window in &self.source_map.publication_windows {
                // A later capture must fit one retained contiguous subwindow.
                // Assembly of disjoint pieces never manufactures such a fit.
                if window.output.start <= captured.start
                    && captured.end <= window.output.end
                    && let Some(mut window) = window.clipped(&self.source_map, captured.clone())
                {
                    window.output = 0..bytes.len();
                    if !map.publication_windows.contains(&window) {
                        map.publication_windows.push(window);
                    }
                }
            }
        }

        Ok((
            bytes,
            Some(WordFieldInput {
                initial_delivery: self.initial_field_delivery,
                map: std::sync::Arc::new(map),
                registry: std::sync::Arc::clone(registry),
            }),
        ))
    }

    pub(crate) fn captured_map(
        &self,
        reader: &Reader<&[u8]>,
        captured: std::ops::Range<u64>,
        raw: &[u8],
    ) -> Option<std::sync::Arc<WordSourceMap>> {
        self.field_origins.as_ref()?;
        let range = self.unchanged_capture_range(reader, captured, raw)?;
        self.source_map.slice(range).ok().map(std::sync::Arc::new)
    }

    pub(crate) fn unchanged_capture_range(
        &self,
        reader: &Reader<&[u8]>,
        captured: std::ops::Range<u64>,
        raw: &[u8],
    ) -> Option<std::ops::Range<usize>> {
        let input = self.input?;
        let origin = reader_input_origin(input, reader)?;
        let start = usize::try_from(captured.start).ok()?.checked_add(origin)?;
        let end = usize::try_from(captured.end).ok()?.checked_add(origin)?;
        // The actual capture range supplies provenance. Equality verifies only
        // this producer's unchanged byte transport, never discovers an origin.
        if input.get(start..end) != Some(raw) {
            return None;
        }
        Some(start..end)
    }

    pub(crate) fn request_standalone_selectors(&mut self) {
        self.selector_requested = self.selector_paragraph_root.is_some();
    }

    pub(crate) fn confirm_standalone_paragraph(
        &mut self,
        reader: &Reader<&[u8]>,
        opening: std::ops::Range<u64>,
    ) {
        if !self.selector_requested {
            return;
        }
        let Some(input) = self.input else {
            return;
        };
        let Ok(consumed) = usize::try_from(reader.buffer_position()) else {
            return;
        };
        let remaining = *reader.get_ref();
        self.selector_ready = reader.decoder() == Reader::from_reader(&b""[..]).decoder()
            && remaining.len().checked_add(consumed) == Some(input.len())
            && (input.as_ptr() as usize).checked_add(consumed) == Some(remaining.as_ptr() as usize)
            && self.selector_paragraph_root.is_some_and(|root| {
                root.start as u64 == opening.start && root.end as u64 == opening.end
            });
    }

    pub(crate) fn simple_selector(
        &self,
        reader: &Reader<&[u8]>,
        captured: std::ops::Range<u64>,
        raw: &[u8],
    ) -> Option<(bool, std::ops::Range<usize>)> {
        if !self.selector_ready {
            return None;
        }
        let span = self.unchanged_capture_range(reader, captured, raw)?;
        let index = self
            .simple_selectors
            .binary_search_by_key(&span.start, |fact| fact.opening.start)
            .ok()?;
        let fact = &self.simple_selectors[index];
        if fact.whole != span {
            return None;
        }
        Some((fact.ordered, fact.result.clone()))
    }

    pub(crate) fn bind_capture<'b>(
        self,
        input: &'b [u8],
        map: std::sync::Arc<WordSourceMap>,
    ) -> ControlSourceCursor<'b>
    where
        'a: 'b,
    {
        let valid = map.len() == input.len();
        ControlSourceCursor {
            occurrences: self.occurrences,
            next: self.next,
            end: self.end,
            projected_aliases: self.projected_aliases,
            initial_field_delivery: self.initial_field_delivery,
            simple_headers: self.simple_headers,
            simple_selectors: &[],
            selector_paragraph_root: None,
            selector_requested: false,
            selector_ready: false,
            input: valid.then_some(input),
            source_map: map,
            field_origins: self.field_origins,
        }
    }

    pub(crate) fn field_input(
        &self,
        reader: &Reader<&[u8]>,
        captured: std::ops::Range<u64>,
        raw: &[u8],
    ) -> Option<WordFieldInput> {
        Some(WordFieldInput {
            initial_delivery: self.initial_field_delivery,
            map: self.captured_map(reader, captured, raw)?,
            registry: std::sync::Arc::clone(self.field_origins.as_ref()?),
        })
    }

    pub(crate) fn simple_header(
        &self,
        reader: &Reader<&[u8]>,
        opening: std::ops::Range<u64>,
        start: &BytesStart<'_>,
        prefixes: &[String],
    ) -> Option<crate::text::SimpleFieldHeader> {
        if !self.projected_aliases {
            return None;
        }
        let input = self.input?;
        let consumed = usize::try_from(reader.buffer_position()).ok()?;
        let remaining = *reader.get_ref();
        // The borrowed backing stays live. No equal-content or recycled-address
        // match can establish origin in a captured or synthetic reader.
        if remaining.len().checked_add(consumed)? != input.len()
            || (input.as_ptr() as usize).checked_add(consumed)? != remaining.as_ptr() as usize
        {
            return None;
        }
        let opening = usize::try_from(opening.start).ok()?..usize::try_from(opening.end).ok()?;
        let index = self
            .simple_headers
            .binary_search_by_key(&opening.start, |fact| fact.opening.start)
            .ok()?;
        let fact = &self.simple_headers[index];
        if fact.opening != opening
            || fact.original_opening.len() != opening.len()
            || self.source_map.original_range(opening.clone()).as_ref()
                != Some(&WordOriginalRange {
                    part: fact.original_part,
                    range: fact.original_opening.clone(),
                })
            || !fact.header.matches_attributes(start, prefixes)
        {
            return None;
        }
        Some(fact.header.clone())
    }

    pub(crate) fn projected_aliases(&self) -> bool {
        self.projected_aliases
    }
    pub(crate) fn take(&mut self, owner: SdtOwner) -> Option<(bool, Self)> {
        if self.next >= self.end {
            return None;
        }
        let occurrence = &self.occurrences[self.next];
        if occurrence.owner != owner {
            return None;
        }
        let children = Self {
            occurrences: self.occurrences,
            next: self.next + 1,
            end: occurrence.after,
            projected_aliases: self.projected_aliases,
            initial_field_delivery: self.initial_field_delivery,
            simple_headers: self.simple_headers,
            simple_selectors: &[],
            selector_paragraph_root: None,
            selector_requested: false,
            selector_ready: false,
            input: None,
            source_map: std::sync::Arc::clone(&self.source_map),
            field_origins: self.field_origins.clone(),
        };
        self.next = occurrence.after;
        Some((occurrence.raw, children))
    }
}
impl AdmittedWordSource {
    pub(crate) fn controls(&self) -> ControlSourceCursor<'_> {
        ControlSourceCursor {
            occurrences: &self.constructors.controls,
            next: 0,
            end: self.constructors.controls.len(),
            projected_aliases: self.projected_aliases,
            initial_field_delivery: false,
            simple_headers: &self.constructors.simple_headers,
            simple_selectors: &self.constructors.simple_selectors,
            selector_paragraph_root: self.constructors.selector_paragraph_root.as_ref(),
            selector_requested: false,
            selector_ready: false,
            input: None,
            source_map: std::sync::Arc::clone(&self.source_map),
            field_origins: None,
        }
    }

    pub(crate) fn controls_for<'a>(&'a self, input: &'a [u8]) -> ControlSourceCursor<'a> {
        let mut controls = self.controls();
        controls.input = Some(input);
        controls
    }

    pub(crate) fn with_source_map(mut self, source_map: WordSourceMap) -> Result<Self> {
        if self.source_map.length != source_map.length {
            return Err(OxmlError::InvalidValue(
                "admitted source correspondence length differs".into(),
            ));
        }
        self.source_map = std::sync::Arc::new(source_map);
        Ok(self)
    }
}
