//! Scoped Word sources, template copy work and template reads.

use super::*;

impl ScopedWordSource {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(super) fn admitted_word_source(&self) -> Result<AdmittedWordSource> {
        match &self.constructors {
            Some(constructors) => Ok(admit_word_input_classified(
                WordXmlInput::Contiguous(&self.bytes),
                &[],
                None,
                None,
                std::sync::Arc::clone(constructors),
            )?),
            None => admit_word_source(&self.bytes, &[], None, None),
        }
    }

    pub(super) fn current_publication(&self) -> WordPublicationCoverage {
        let mut coverage = self.source_map.materialized_publication();
        let current = self
            .publication
            .lock()
            .expect("publication coverage poisoned");
        coverage.add_ranges(current.fields.clone());
        coverage.add_ordinary_ranges(current.ordinary.clone());
        coverage
    }
}

impl TemplateCopyWork {
    pub(super) fn reserve(&mut self, bytes: usize) -> Result<()> {
        let needed = self.copied.checked_add(bytes);
        if self.refused || needed.is_none_or(|needed| needed > self.allowance) {
            self.refused = true;
            return Err(OxmlError::InvalidValue(
                "template text copy exceeds selected input work".into(),
            ));
        }
        self.copied = needed.expect("checked template copy size");
        Ok(())
    }

    pub(crate) fn select_word(
        &mut self,
        origin: crate::text::TemplateWordOrigin,
        range: std::ops::Range<usize>,
    ) -> Result<()> {
        if self.refused {
            return Err(OxmlError::InvalidValue(
                "template text copy exceeds selected input work".into(),
            ));
        }
        let mut merged = range;
        let mut removed = Vec::new();
        let mut previous_bytes = 0usize;
        if let Some(ranges) = self.word_inputs.get(&origin) {
            if let Some((&start, &end)) = ranges.range(..=merged.start).next_back()
                && end >= merged.start
            {
                merged.start = start;
                merged.end = merged.end.max(end);
            }
            for (&start, &end) in ranges.range(merged.start..) {
                if start > merged.end {
                    break;
                }
                merged.end = merged.end.max(end);
                previous_bytes = previous_bytes.checked_add(end - start).ok_or_else(|| {
                    OxmlError::InvalidValue("Word text input length overflow".into())
                })?;
                removed.push(start);
            }
        }
        let added = merged
            .len()
            .checked_sub(previous_bytes)
            .and_then(|bytes| bytes.checked_mul(W_NS.len() + 64))
            .and_then(|bytes| self.allowance.checked_add(bytes))
            .ok_or_else(|| OxmlError::InvalidValue("Word text input allowance overflow".into()))?;
        let ranges = self.word_inputs.entry(origin).or_default();
        for start in removed {
            ranges.remove(&start);
        }
        ranges.insert(merged.start, merged.end);
        self.allowance = added;
        Ok(())
    }
}

/// The same live parsed paragraph and its actual constructor-return companions
/// stay borrowed through the substantive placeholder TextWalk.
pub(crate) struct TemplateWordRead<'a> {
    pub(crate) map: &'a WordSourceMap,
    pub(crate) texts: &'a crate::text::WordParagraphTextFacts,
    pub(crate) work: &'a mut TemplateCopyWork,
    // Actual Text objects returned by this one reader, borrowed by the live walk.
    pub(crate) nested: Option<&'a std::collections::HashMap<usize, crate::text::TemplateWordEvent>>,
}
impl TemplateWordRead<'_> {
    pub(crate) fn select(
        &mut self,
        path: &crate::text::AcceptedRunPath,
        wrapper: Option<usize>,
        wrapped: Option<&crate::text::WordParagraphReadFacts>,
        content: usize,
        text: &crate::text::CT_Text,
    ) -> Result<()> {
        if self.work.refused {
            return Err(OxmlError::InvalidValue(
                "template text copy exceeds selected input work".into(),
            ));
        }
        if wrapper.is_none()
            && let Some(event) = self
                .nested
                .and_then(|inputs| inputs.get(&(std::ptr::from_ref(text) as usize)))
        {
            return self
                .work
                .select_word(event.origin.clone(), event.decoded.clone());
        }
        let facts = if wrapper.is_some() {
            wrapped.map(|facts| (&facts.texts, facts.source_map.as_ref()))
        } else {
            Some((self.texts, self.map))
        };
        if let Some((texts, map)) = facts
            && let [crate::text::AcceptedRunPathSegment::Run(index)] = path.segments()
            && let Some(Some(run)) = texts.get(*index)
        {
            Self::select_run_input(self.work, map, run, content, text)?;
        }
        Ok(())
    }

    pub(crate) fn select_run(
        &mut self,
        run: &crate::text::ParsedWordRunTexts,
        content: usize,
        text: &crate::text::CT_Text,
    ) -> Result<()> {
        Self::select_run_input(self.work, self.map, run, content, text)
    }

    fn select_run_input(
        work: &mut TemplateCopyWork,
        map: &WordSourceMap,
        run: &crate::text::ParsedWordRunTexts,
        content: usize,
        text: &crate::text::CT_Text,
    ) -> Result<()> {
        if work.refused {
            return Err(OxmlError::InvalidValue(
                "template text copy exceeds selected input work".into(),
            ));
        }
        if let Ok(index) = run
            .texts
            .binary_search_by_key(&content, |text| text.content)
        {
            let parsed = &run.texts[index];
            let map = run.source_map.as_deref().unwrap_or(map);
            let encoded =
                run.source.start + parsed.encoded.start..run.source.start + parsed.encoded.end;
            if let Ok(index) = map
                .word_events
                .binary_search_by_key(&encoded.start, |event| event.encoded.start)
                && let Some(event) = map.word_events.get(index)
                && event.encoded == encoded
                && event.decoded.len() == text.text.len()
            {
                work.select_word(event.origin.clone(), event.decoded.clone())?;
            } else if let Some(origin) = map.word_range(encoded) {
                work.select_word(
                    crate::text::TemplateWordOrigin::Raw {
                        start: origin.start,
                        end: origin.end,
                    },
                    0..text.text.len(),
                )?;
            }
        }
        Ok(())
    }

    pub(crate) fn append(&mut self, output: &mut String, text: &str) -> Result<()> {
        self.work.reserve(text.len())?;
        output.push_str(text);
        Ok(())
    }
}

/// Allocation-free canonical scalar counting is shared by Word Field input
/// and native template copy roles. Their grant equations remain independent.
pub(crate) fn template_scalar_bytes(value: &serde_json::Value) -> Result<usize> {
    Ok(match value {
        serde_json::Value::Null => 0,
        serde_json::Value::String(text) => text.len(),
        serde_json::Value::Bool(value) => {
            if *value {
                4
            } else {
                5
            }
        }
        serde_json::Value::Number(number) => {
            struct TextLength(usize);
            impl std::fmt::Write for TextLength {
                fn write_str(&mut self, text: &str) -> std::fmt::Result {
                    self.0 = self.0.checked_add(text.len()).ok_or(std::fmt::Error)?;
                    Ok(())
                }
            }
            let mut length = TextLength(0);
            std::fmt::write(&mut length, format_args!("{number}"))
                .map_err(|_| OxmlError::InvalidValue("template scalar length overflow".into()))?;
            length.0
        }
        _ => {
            return Err(OxmlError::InvalidValue(
                "template operand is not a scalar".into(),
            ));
        }
    })
}
