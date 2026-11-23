//! Source maps: the original and generated ranges of a published Word part, and its publication tape.

use super::*;

impl WordOrdinaryFact {
    pub(crate) fn publish_all(facts: Vec<Self>) {
        let mut selected = std::collections::HashMap::new();
        for fact in facts {
            // Collector tapes may overlap and are not globally ordered.
            for piece in &fact.map.publication {
                let start = fact.selected.start.max(piece.output.start);
                let end = fact.selected.end.min(piece.output.end);
                if start >= end {
                    continue;
                }
                let Some(collector) = piece.collector.upgrade() else {
                    continue;
                };
                let key = std::sync::Arc::as_ptr(&collector) as usize;
                selected
                    .entry(key)
                    .or_insert_with(|| (collector, Vec::new()))
                    .1
                    .push(
                        piece.current.start + start - piece.output.start
                            ..piece.current.start + end - piece.output.start,
                    );
            }
        }
        for (_, (collector, ranges)) in selected {
            // Existing positive Field ownership always has priority. One
            // grouped union avoids rebuilding the inventory per style event.
            collector
                .lock()
                .expect("publication coverage poisoned")
                .add_ordinary_ranges(ranges);
        }
    }
}
impl PartialEq for WordPublicationPiece {
    fn eq(&self, other: &Self) -> bool {
        self.output == other.output
            && self.current == other.current
            && std::sync::Weak::ptr_eq(&self.collector, &other.collector)
    }
}
impl Eq for WordPublicationPiece {}
impl PartialEq for WordCaptureWindow {
    fn eq(&self, other: &Self) -> bool {
        self.output == other.output
            && self.current == other.current
            && std::sync::Weak::ptr_eq(&self.collector, &other.collector)
    }
}
impl Eq for WordCaptureWindow {}

impl WordCaptureWindow {
    pub(crate) fn collector_key(&self) -> usize {
        self.collector.as_ptr() as usize
    }

    pub(super) fn clipped(
        &self,
        map: &WordSourceMap,
        range: std::ops::Range<usize>,
    ) -> Option<Self> {
        let start = self.output.start.max(range.start);
        let end = self.output.end.min(range.end);
        if start >= end {
            return None;
        }
        let current_start = if start == self.output.start {
            self.current.start
        } else {
            WordPublicationPiece::copied_boundary(
                &map.publication,
                &self.collector,
                &self.current,
                start,
                false,
            )?
        };
        let current_end = if end == self.output.end {
            self.current.end
        } else {
            WordPublicationPiece::copied_boundary(
                &map.publication,
                &self.collector,
                &self.current,
                end,
                true,
            )?
        };
        (current_start < current_end
            && self.current.start <= current_start
            && current_end <= self.current.end)
            .then(|| Self {
                output: start - range.start..end - range.start,
                current: current_start..current_end,
                collector: self.collector.clone(),
            })
    }

    pub(crate) fn selected_ranges(
        &self,
        copies: &[WordPublicationPiece],
        owned: &[std::ops::Range<usize>],
        output: &mut Vec<std::ops::Range<usize>>,
        mut proof: Option<&mut Vec<std::ops::Range<usize>>>,
    ) {
        if self.collector.strong_count() == 0 {
            return;
        }
        for owned in owned {
            let start = owned.start.max(self.output.start);
            let end = owned.end.min(self.output.end);
            if start >= end {
                continue;
            }
            let Some(from) = WordPublicationPiece::copied_boundary(
                copies,
                &self.collector,
                &self.current,
                start,
                false,
            ) else {
                continue;
            };
            let Some(to) = WordPublicationPiece::copied_boundary(
                copies,
                &self.collector,
                &self.current,
                end,
                true,
            ) else {
                continue;
            };
            if from < to {
                output.push(from..to);
                if let Some(proof) = proof.as_deref_mut() {
                    proof.push(start..end);
                }
            }
        }
    }
}

pub(crate) type WordPublicationTape = (Vec<WordPublicationPiece>, Vec<WordCaptureWindow>);

impl WordPublicationPiece {
    fn copied_boundary(
        pieces: &[Self],
        collector: &std::sync::Weak<std::sync::Mutex<WordPublicationCoverage>>,
        bounds: &std::ops::Range<usize>,
        position: usize,
        end: bool,
    ) -> Option<usize> {
        let mut found = None;
        for piece in pieces {
            let contains = if end {
                piece.output.start < position && position <= piece.output.end
            } else {
                piece.output.start <= position && position < piece.output.end
            };
            if !contains || !std::sync::Weak::ptr_eq(&piece.collector, collector) {
                continue;
            }
            let current = piece
                .current
                .start
                .checked_add(position.checked_sub(piece.output.start)?)?;
            let in_bounds = if end {
                bounds.start < current && current <= bounds.end
            } else {
                bounds.start <= current && current < bounds.end
            };
            if !in_bounds {
                continue;
            }
            if found.is_some_and(|previous| previous != current) {
                return None;
            }
            found = Some(current);
        }
        found
    }

    pub(crate) fn selected_ranges(
        &self,
        collector: &std::sync::Arc<std::sync::Mutex<WordPublicationCoverage>>,
        owned: &[std::ops::Range<usize>],
        output: &mut Vec<std::ops::Range<usize>>,
        mut proof: Option<&mut Vec<std::ops::Range<usize>>>,
    ) {
        if !std::sync::Weak::ptr_eq(&self.collector, &std::sync::Arc::downgrade(collector)) {
            return;
        }
        for range in owned {
            let start = range.start.max(self.output.start);
            let end = range.end.min(self.output.end);
            if start < end {
                if let Some(proof) = proof.as_deref_mut() {
                    proof.push(start..end);
                }
                output.push(
                    self.current.start + start - self.output.start
                        ..self.current.start + end - self.output.start,
                );
            }
        }
    }

    pub(crate) fn select_owned(&self, ranges: &[std::ops::Range<usize>]) -> Vec<Self> {
        ranges
            .iter()
            .filter_map(|range| {
                let start = range.start.max(self.output.start);
                let end = range.end.min(self.output.end);
                (start < end).then(|| Self {
                    output: start..end,
                    current: self.current.start + start - self.output.start
                        ..self.current.start + end - self.output.start,
                    collector: self.collector.clone(),
                })
            })
            .collect()
    }

    pub(crate) fn collector_key(&self) -> usize {
        self.collector.as_ptr() as usize
    }
    pub(crate) fn live(&self) -> bool {
        self.collector.strong_count() > 0
    }
}

impl WordSourceMap {
    pub(crate) fn len(&self) -> usize {
        self.length
    }

    pub(crate) fn original(range: std::ops::Range<usize>) -> Self {
        Self::original_part(0, range)
    }

    pub(super) fn original_part(part: usize, range: std::ops::Range<usize>) -> Self {
        Self {
            length: range.len(),
            word_events: Vec::new(),
            publication: Vec::new(),
            publication_windows: Vec::new(),
            publication_owned: Vec::new(),
            ordinary_owned: Vec::new(),
            pieces: vec![WordSourcePiece {
                output: 0..range.len(),
                original: Some(WordOriginalRange { part, range }),
                word: None,
            }],
        }
    }

    pub(crate) fn generated(length: usize) -> Self {
        Self {
            length,
            word_events: Vec::new(),
            publication: Vec::new(),
            publication_windows: Vec::new(),
            publication_owned: Vec::new(),
            ordinary_owned: Vec::new(),
            pieces: vec![WordSourcePiece {
                output: 0..length,
                original: None,
                word: None,
            }],
        }
    }

    pub(crate) fn concat(parts: &[Self]) -> Result<Self> {
        let mut joined = Self::generated(0);
        for part in parts {
            joined.extend(part)?;
        }
        Ok(joined)
    }

    pub(crate) fn extend(&mut self, part: &Self) -> Result<()> {
        let overflow = || OxmlError::InvalidValue("derived source position overflow".into());
        let length = self.length;
        let next = length.checked_add(part.length).ok_or_else(overflow)?;
        if length == 0 {
            self.pieces.clear();
        }
        for piece in &part.pieces {
            if piece.output.is_empty() {
                continue;
            }
            let output_start = piece
                .output
                .start
                .checked_add(length)
                .ok_or_else(overflow)?;
            let output_end = piece.output.end.checked_add(length).ok_or_else(overflow)?;
            if let Some(previous) = self.pieces.last_mut() {
                let compatible = match (&previous.original, &piece.original) {
                    (Some(left), Some(right)) => {
                        left.part == right.part && left.range.end == right.range.start
                    }
                    (None, None) => true,
                    _ => false,
                };
                let word_compatible = match (&previous.word, &piece.word) {
                    (Some(left), Some(right)) => left.end == right.start,
                    (None, None) => true,
                    _ => false,
                };
                if previous.output.end == output_start && compatible && word_compatible {
                    previous.output.end = output_end;
                    if let (Some(left), Some(right)) = (&mut previous.original, &piece.original) {
                        left.range.end = right.range.end;
                    }
                    if let (Some(left), Some(right)) = (&mut previous.word, &piece.word) {
                        left.end = right.end;
                    }
                    continue;
                }
            }
            self.pieces.push(WordSourcePiece {
                output: output_start..output_end,
                original: piece.original.clone(),
                word: piece.word.clone(),
            });
        }
        for event in &part.word_events {
            let mut event = event.clone();
            event.encoded = event
                .encoded
                .start
                .checked_add(length)
                .ok_or_else(overflow)?
                ..event.encoded.end.checked_add(length).ok_or_else(overflow)?;
            self.word_events.push(event);
        }
        for (output, ranges) in [
            (&mut self.publication_owned, &part.publication_owned),
            (&mut self.ordinary_owned, &part.ordinary_owned),
        ] {
            for range in ranges {
                let start = range.start.checked_add(length).ok_or_else(overflow)?;
                let end = range.end.checked_add(length).ok_or_else(overflow)?;
                if let Some(previous) = output.last_mut()
                    && previous.end == start
                {
                    previous.end = end;
                } else {
                    output.push(start..end);
                }
            }
        }
        for piece in &part.publication {
            self.publication.push(WordPublicationPiece {
                output: piece
                    .output
                    .start
                    .checked_add(length)
                    .ok_or_else(overflow)?
                    ..piece.output.end.checked_add(length).ok_or_else(overflow)?,
                current: piece.current.clone(),
                collector: piece.collector.clone(),
            });
        }
        for window in &part.publication_windows {
            self.publication_windows.push(WordCaptureWindow {
                output: window
                    .output
                    .start
                    .checked_add(length)
                    .ok_or_else(overflow)?
                    ..window.output.end.checked_add(length).ok_or_else(overflow)?,
                current: window.current.clone(),
                collector: window.collector.clone(),
            });
        }
        self.length = next;
        Ok(())
    }

    pub(crate) fn original_pieces(&self) -> Vec<(usize, usize, usize)> {
        self.pieces
            .iter()
            .filter_map(|piece| piece.original.as_ref())
            .map(|original| (original.part, original.range.start, original.range.end))
            .collect()
    }

    pub(crate) fn entirely_original(&self) -> bool {
        self.length > 0 && self.pieces.iter().all(|piece| piece.original.is_some())
    }

    pub(crate) fn slice(&self, range: std::ops::Range<usize>) -> Result<Self> {
        if range.start > range.end || range.end > self.length {
            return Err(OxmlError::InvalidValue(
                "derived source range is invalid".into(),
            ));
        }
        let mut pieces = Vec::new();
        let first = self
            .pieces
            .partition_point(|piece| piece.output.end <= range.start);
        for piece in self.pieces[first..]
            .iter()
            .take_while(|piece| piece.output.start < range.end)
        {
            let start = range.start.max(piece.output.start);
            let end = range.end.min(piece.output.end);
            if start < end {
                pieces.push(WordSourcePiece {
                    output: start - range.start..end - range.start,
                    original: piece.original.as_ref().map(|original| WordOriginalRange {
                        part: original.part,
                        range: original.range.start + start - piece.output.start
                            ..original.range.start + end - piece.output.start,
                    }),
                    word: piece.word.as_ref().map(|word| {
                        word.start + start - piece.output.start
                            ..word.start + end - piece.output.start
                    }),
                });
            }
        }
        Ok(Self {
            length: range.len(),
            pieces,
            publication_windows: self
                .publication_windows
                .iter()
                .filter_map(|window| window.clipped(self, range.clone()))
                .collect(),
            publication_owned: self.publication_owned[self
                .publication_owned
                .partition_point(|owned| owned.end <= range.start)..]
                .iter()
                .take_while(|owned| owned.start < range.end)
                .filter_map(|owned| {
                    let start = range.start.max(owned.start);
                    let end = range.end.min(owned.end);
                    (start < end).then(|| start - range.start..end - range.start)
                })
                .collect(),
            ordinary_owned: self.ordinary_owned[self
                .ordinary_owned
                .partition_point(|owned| owned.end <= range.start)..]
                .iter()
                .take_while(|owned| owned.start < range.end)
                .filter_map(|owned| {
                    let start = range.start.max(owned.start);
                    let end = range.end.min(owned.end);
                    (start < end).then(|| start - range.start..end - range.start)
                })
                .collect(),
            publication: self
                .publication
                .iter()
                .filter_map(|piece| {
                    let start = range.start.max(piece.output.start);
                    let end = range.end.min(piece.output.end);
                    (start < end).then(|| WordPublicationPiece {
                        output: start - range.start..end - range.start,
                        current: piece.current.start + start - piece.output.start
                            ..piece.current.start + end - piece.output.start,
                        collector: piece.collector.clone(),
                    })
                })
                .collect(),
            word_events: self.word_events[self
                .word_events
                .partition_point(|event| event.encoded.end <= range.start)..]
                .iter()
                .take_while(|event| event.encoded.start < range.end)
                .filter(|event| {
                    range.start <= event.encoded.start && event.encoded.end <= range.end
                })
                .map(|event| {
                    let mut event = event.clone();
                    event.encoded =
                        event.encoded.start - range.start..event.encoded.end - range.start;
                    event
                })
                .collect(),
        })
    }

    pub(crate) fn word_only(mut self) -> Self {
        self.publication.clear();
        self.publication_windows.clear();
        for piece in &mut self.pieces {
            piece.original = None;
        }
        self
    }

    pub(super) fn with_publication_collector(
        mut self,
        collector: &std::sync::Arc<std::sync::Mutex<WordPublicationCoverage>>,
    ) -> Self {
        // A real new Source occurrence is a new CURRENT domain. Prior
        // uncommitted capture windows cannot qualify it under a new context.
        self.publication_windows.clear();
        self.publication = vec![WordPublicationPiece {
            output: 0..self.length,
            current: 0..self.length,
            collector: std::sync::Arc::downgrade(collector),
        }];
        self
    }

    pub(crate) fn with_publication_owned(
        mut self,
        ranges: Vec<std::ops::Range<usize>>,
    ) -> Result<Self> {
        if ranges
            .iter()
            .any(|range| range.start > range.end || range.end > self.length)
        {
            return Err(OxmlError::InvalidValue(
                "Field publication range is invalid".into(),
            ));
        }
        let mut ranges = ranges;
        WordPublicationCoverage::merge_ranges(&mut ranges);
        self.publication_owned = ranges;
        Ok(self)
    }

    pub(crate) fn has_publication_owned(&self) -> bool {
        !self.publication_owned.is_empty() || !self.ordinary_owned.is_empty()
    }

    pub(crate) fn mark_field_publication(&mut self) {
        if self.length > 0 {
            self.publication_owned = std::iter::once(0..self.length).collect();
        }
    }

    pub(super) fn materialized_publication(&self) -> WordPublicationCoverage {
        let mut coverage = WordPublicationCoverage::default();
        coverage.add_ranges(self.publication_owned.clone());
        coverage.add_ordinary_ranges(self.ordinary_owned.clone());
        // Materialize only exact, already-committed current-coordinate roles
        // while their source collector is live. The returned bytes must not
        // depend on keeping an obsolete Source or model buffer alive.
        for piece in &self.publication {
            let Some(collector) = piece.collector.upgrade() else {
                continue;
            };
            let owned = collector.lock().expect("publication coverage poisoned");
            for (output, ranges) in [
                (&mut coverage.fields, &owned.fields),
                (&mut coverage.ordinary, &owned.ordinary),
            ] {
                let first = ranges.partition_point(|range| range.end <= piece.current.start);
                for range in ranges[first..]
                    .iter()
                    .take_while(|range| range.start < piece.current.end)
                {
                    let start = range.start.max(piece.current.start);
                    let end = range.end.min(piece.current.end);
                    output.push(
                        piece.output.start + start - piece.current.start
                            ..piece.output.start + end - piece.current.start,
                    );
                }
            }
        }
        coverage.add_ranges(Vec::new());
        coverage.add_ordinary_ranges(Vec::new());
        coverage
    }

    pub(super) fn publication_output(
        &self,
    ) -> std::sync::Arc<std::sync::Mutex<WordPublicationCoverage>> {
        std::sync::Arc::new(std::sync::Mutex::new(self.materialized_publication()))
    }

    /// Freeze the actual local record's numeric roles before its collector
    /// expires. This neither retains its bytes nor supplies any input role.
    pub(crate) fn freeze_publication(&mut self) {
        let coverage = self.materialized_publication();
        self.publication_owned = coverage.fields;
        self.ordinary_owned = coverage.ordinary;
    }

    pub(crate) fn publication_collectors(
        &self,
    ) -> Vec<std::sync::Arc<std::sync::Mutex<WordPublicationCoverage>>> {
        let mut seen = std::collections::HashSet::new();
        self.publication
            .iter()
            .filter_map(|piece| piece.collector.upgrade())
            .filter(|collector| seen.insert(std::sync::Arc::as_ptr(collector) as usize))
            .collect()
    }

    pub(crate) fn publication_boundary(
        &self,
        range: std::ops::Range<usize>,
    ) -> Option<WordPublicationPiece> {
        let selected = self.slice(range).ok()?;
        let mut pieces = selected.publication.into_iter();
        let mut joined = pieces.next()?;
        if joined.output.start != 0 {
            return None;
        }
        for piece in pieces {
            if !std::sync::Weak::ptr_eq(&joined.collector, &piece.collector)
                || joined.output.end != piece.output.start
                || joined.current.end != piece.current.start
            {
                return None;
            }
            joined.output.end = piece.output.end;
            joined.current.end = piece.current.end;
        }
        (joined.output.end == selected.length).then_some(joined)
    }

    pub(crate) fn publication_owner_ranges(
        &self,
        start: &WordPublicationPiece,
        end: &WordPublicationPiece,
        shells: &[WordPublicationPiece],
    ) -> Option<Vec<std::ops::Range<usize>>> {
        if !std::sync::Weak::ptr_eq(&start.collector, &end.collector)
            || start.current.start > end.current.end
        {
            return None;
        }
        let mut selected = Vec::new();
        let envelope = start.current.start..end.current.end;
        for piece in &self.publication {
            if !std::sync::Weak::ptr_eq(&piece.collector, &start.collector) {
                continue;
            }
            for current in std::iter::once(&envelope).chain(shells.iter().filter_map(|shell| {
                std::sync::Weak::ptr_eq(&shell.collector, &start.collector)
                    .then_some(&shell.current)
            })) {
                let from = piece.current.start.max(current.start);
                let to = piece.current.end.min(current.end);
                if from < to {
                    selected.push(
                        piece.output.start + from - piece.current.start
                            ..piece.output.start + to - piece.current.start,
                    );
                }
            }
        }
        Some(selected)
    }

    pub(crate) fn publication_selection(
        &self,
        ranges: &[std::ops::Range<usize>],
    ) -> Vec<WordPublicationPiece> {
        ranges
            .iter()
            .filter_map(|range| self.slice(range.clone()).ok())
            .flat_map(|map| map.publication)
            .collect()
    }

    pub(crate) fn publication_relocated_ranges(
        &self,
        selected: &[WordPublicationPiece],
    ) -> Vec<std::ops::Range<usize>> {
        let mut output = Vec::new();
        for piece in &self.publication {
            for old in selected {
                if !std::sync::Weak::ptr_eq(&piece.collector, &old.collector) {
                    continue;
                }
                let start = piece.current.start.max(old.current.start);
                let end = piece.current.end.min(old.current.end);
                if start < end {
                    output.push(
                        piece.output.start + start - piece.current.start
                            ..piece.output.start + end - piece.current.start,
                    );
                }
            }
        }
        output
    }

    pub(crate) fn take_publication(&mut self) -> WordPublicationTape {
        (
            std::mem::take(&mut self.publication),
            std::mem::take(&mut self.publication_windows),
        )
    }

    pub(crate) fn has_publication(&self) -> bool {
        !self.publication.is_empty()
    }

    pub(crate) fn has_word_inputs(&self) -> bool {
        !self.word_events.is_empty() || self.pieces.iter().any(|piece| piece.word.is_some())
    }

    pub(crate) fn with_word_range(mut self, range: std::ops::Range<usize>) -> Result<Self> {
        if range.len() != self.length {
            return Err(OxmlError::InvalidValue(
                "Word transport correspondence length mismatch".into(),
            ));
        }
        for piece in &mut self.pieces {
            piece.word = Some(range.start + piece.output.start..range.start + piece.output.end);
        }
        Ok(self)
    }

    pub(super) fn with_word_source(
        &self,
        word: &WordSourceMap,
        events: &[crate::text::TemplateWordEvent],
    ) -> Result<Self> {
        if self.length != word.length {
            return Err(OxmlError::InvalidValue(
                "Word transport source length mismatch".into(),
            ));
        }
        let mut output = Self::generated(0);
        for piece in &word.pieces {
            let mut selected = self.slice(piece.output.clone())?;
            if let Some(origin) = &piece.original {
                selected = selected.with_word_range(origin.range.clone())?;
            }
            output.extend(&selected)?;
        }
        output.word_events.extend_from_slice(events);
        output.word_events.sort_by_key(|event| event.encoded.start);
        output.word_events.dedup();
        Ok(output)
    }

    pub(crate) fn with_retained_word_source(&self, word: &Self) -> Result<Self> {
        if self.length != word.length {
            return Err(OxmlError::InvalidValue(
                "returned Word source length mismatch".into(),
            ));
        }
        let mut output = Self::generated(0);
        for piece in &word.pieces {
            let mut selected = self.slice(piece.output.clone())?;
            if let Some(origin) = &piece.word {
                selected = selected.with_word_range(origin.clone())?;
            }
            output.extend(&selected)?;
        }
        output.word_events.extend_from_slice(&word.word_events);
        output.word_events.sort_by_key(|event| event.encoded.start);
        output.word_events.dedup();
        Ok(output)
    }

    pub(crate) fn push_word_event(&mut self, event: crate::text::TemplateWordEvent) -> Result<()> {
        if event.encoded.start > event.encoded.end || event.encoded.end > self.length {
            return Err(OxmlError::InvalidValue(
                "Word Text event exceeds produced source".into(),
            ));
        }
        self.word_events.push(event);
        Ok(())
    }

    pub(crate) fn take_word_events(&mut self) -> Vec<crate::text::TemplateWordEvent> {
        std::mem::take(&mut self.word_events)
    }

    pub(super) fn word_range(
        &self,
        range: std::ops::Range<usize>,
    ) -> Option<std::ops::Range<usize>> {
        let selected = self.slice(range).ok()?;
        let mut result = selected.pieces.first()?.word.clone()?;
        for piece in selected.pieces.iter().skip(1) {
            let next = piece.word.as_ref()?;
            if result.end != next.start {
                return None;
            }
            result.end = next.end;
        }
        Some(result)
    }

    /// A contiguous original occurrence, rather than an equal-text match.
    pub(super) fn original_range(
        &self,
        range: std::ops::Range<usize>,
    ) -> Option<WordOriginalRange> {
        let selected = self.slice(range).ok()?;
        let first = selected.pieces.first()?.original.as_ref()?;
        let mut original = first.clone();
        for piece in selected.pieces.iter().skip(1) {
            let next = piece.original.as_ref()?;
            if original.part != next.part || original.range.end != next.range.start {
                return None;
            }
            original.range.end = next.range.end;
        }
        Some(original)
    }

    pub(crate) fn after_insertion(&self, at: usize, bytes: usize) -> Result<Self> {
        if at > self.length {
            return Err(OxmlError::InvalidValue(
                "derived source insertion is invalid".into(),
            ));
        }
        Self::concat(&[
            self.slice(0..at)?,
            Self::generated(bytes),
            self.slice(at..self.length)?,
        ])
    }
}
