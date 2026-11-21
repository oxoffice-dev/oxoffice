//! Document-level elements: `CT_Document` and `CT_Body`.

use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, Event};
use quick_xml::{Reader, Writer, XmlVersion};

use crate::borders::CT_BorderEdge;
use crate::content_control::{CT_Sdt, SdtOwner};
use crate::error::{OxmlError, Result};
use crate::header_footer::{HdrFtrRef, HdrFtrType};
use crate::namespace::{W_NS, matches_local_name};
use crate::numbering::{
    local_namespace_overrides, merged_owner_bindings, namespace_bindings, split_namespace_binding,
    word_prefixes_at,
};
use crate::properties::{
    get_word_val_attr, is_word_attribute, is_word_element, parse_integer_measurement,
};
use crate::raw_xml::{capture_element, capture_empty_element};
use crate::revision::{CT_Revision, RevisionKind};
use crate::shared::{ST_PageOrientation, ST_SectionType};
use crate::table::{CT_Tbl, ST_VerticalJc};
use crate::text::{
    CT_P, ROOT_MC_BINDING, ROOT_R_BINDING, ROOT_WP_BINDING, capture_root_attribute_record,
    declare_w14_on_part_root, is_root_attribute_record,
};
use crate::units::Twips;

/// Admission keeps resource refusals distinct from ordinary raw-input failure.
/// Public parsers retain their existing OxmlError representation.
#[derive(Debug, thiserror::Error)]
enum WordSourceFailure {
    #[error("{0}")]
    Input(#[from] OxmlError),
    #[error("{0}")]
    Resource(OxmlError),
}
impl WordSourceFailure {
    fn resource(message: String) -> Self {
        Self::Resource(OxmlError::InvalidValue(message))
    }
}
impl From<WordSourceFailure> for OxmlError {
    fn from(failure: WordSourceFailure) -> Self {
        match failure {
            WordSourceFailure::Input(error) | WordSourceFailure::Resource(error) => error,
        }
    }
}
type WordSourceResult<T> = std::result::Result<T, WordSourceFailure>;

/// Correspondence to one live operation input. Inserted bytes have no original
/// range. Repeated pieces keep repeated output positions and the same origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WordSourceMap {
    length: usize,
    pieces: Vec<WordSourcePiece>,
    word_events: Vec<crate::text::TemplateWordEvent>,
    publication: Vec<WordPublicationPiece>,
    publication_windows: Vec<WordCaptureWindow>,
    publication_owned: Vec<std::ops::Range<usize>>,
    ordinary_owned: Vec<std::ops::Range<usize>>,
}

/// One observation from an actual retained property reader. The map carries
/// only its real capture images, not a guessed enclosing ordinary subtree.
#[derive(Clone, Debug)]
pub(crate) struct WordOrdinaryFact {
    map: std::sync::Arc<WordSourceMap>,
    selected: std::ops::Range<usize>,
}

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

#[derive(Clone, Debug, PartialEq, Eq)]
struct WordSourcePiece {
    output: std::ops::Range<usize>,
    original: Option<WordOriginalRange>,
    word: Option<std::ops::Range<usize>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WordOriginalRange {
    part: usize,
    range: std::ops::Range<usize>,
}

/// Positive, partial publication inventory. Empty fields are NOT proof of
/// fieldlessness: unvisited wrapper and fallback consumers can remain pending.
#[derive(Clone, Debug, Default)]
pub(crate) struct WordPublicationCoverage {
    fields: Vec<std::ops::Range<usize>>,
    ordinary: Vec<std::ops::Range<usize>>,
    // Necessary delivery evidence only. Even Some(true) says nothing about
    // unvisited raw consumers or complete retained physical inventory.
    pub(crate) returned_fields_joined: Option<bool>,
}

impl WordPublicationCoverage {
    fn copied_field_bytes(&self, length: usize) -> Result<usize> {
        self.fields.iter().try_fold(0usize, |total, range| {
            if range.start > range.end || range.end > length {
                return Err(OxmlError::InvalidValue(
                    "copied Field publication range is invalid".into(),
                ));
            }
            total.checked_add(range.len()).ok_or_else(|| {
                OxmlError::InvalidValue("copied Field publication work overflow".into())
            })
        })
    }

    pub(crate) fn covers_ranges(&self, ranges: &[std::ops::Range<usize>]) -> bool {
        !ranges.is_empty()
            && ranges.iter().all(|range| {
                if range.is_empty() {
                    return false;
                }
                let index = self
                    .fields
                    .partition_point(|field| field.end <= range.start);
                self.fields
                    .get(index)
                    .is_some_and(|field| field.start <= range.start && range.end <= field.end)
            })
    }

    pub(crate) fn add_ranges(&mut self, ranges: Vec<std::ops::Range<usize>>) {
        self.fields.extend(ranges);
        Self::merge_ranges(&mut self.fields);
        self.remove_field_overlap();
    }

    fn add_ordinary_ranges(&mut self, ranges: Vec<std::ops::Range<usize>>) {
        self.ordinary.extend(ranges);
        Self::merge_ranges(&mut self.ordinary);
        self.remove_field_overlap();
    }

    fn merge_ranges(ranges: &mut Vec<std::ops::Range<usize>>) {
        ranges.sort_by_key(|range| range.start);
        let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
        for range in std::mem::take(ranges) {
            if let Some(last) = merged.last_mut()
                && range.start <= last.end
            {
                last.end = last.end.max(range.end);
            } else {
                merged.push(range);
            }
        }
        *ranges = merged;
    }

    // A later positive Field consumer supersedes earlier ordinary metadata.
    // Unknown gaps are never filled by either kind of positive fact.
    fn remove_field_overlap(&mut self) {
        if self.ordinary.is_empty() || self.fields.is_empty() {
            return;
        }
        let mut ordinary = Vec::new();
        let mut first = 0;
        for range in std::mem::take(&mut self.ordinary) {
            while first < self.fields.len() && self.fields[first].end <= range.start {
                first += 1;
            }
            let mut at = range.start;
            for field in self.fields[first..]
                .iter()
                .take_while(|field| field.start < range.end)
            {
                if at < field.start {
                    ordinary.push(at..field.start.min(range.end));
                }
                at = at.max(field.end);
                if at >= range.end {
                    break;
                }
            }
            if at < range.end {
                ordinary.push(at..range.end);
            }
        }
        self.ordinary = ordinary;
    }
}

#[derive(Clone, Debug)]
pub(crate) struct WordPublicationPiece {
    output: std::ops::Range<usize>,
    current: std::ops::Range<usize>,
    collector: std::sync::Weak<std::sync::Mutex<WordPublicationCoverage>>,
}
impl PartialEq for WordPublicationPiece {
    fn eq(&self, other: &Self) -> bool {
        self.output == other.output
            && self.current == other.current
            && std::sync::Weak::ptr_eq(&self.collector, &other.collector)
    }
}
impl Eq for WordPublicationPiece {}

/// An event-preserving, trim(false) capture's contiguous retained subwindow.
/// Unlike a copied piece its two lengths can differ. Ownership still requires
/// actual scanner-selected boundaries with monotonic copied CURRENT images.
#[derive(Clone, Debug)]
pub(crate) struct WordCaptureWindow {
    output: std::ops::Range<usize>,
    current: std::ops::Range<usize>,
    collector: std::sync::Weak<std::sync::Mutex<WordPublicationCoverage>>,
}
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

    fn clipped(&self, map: &WordSourceMap, range: std::ops::Range<usize>) -> Option<Self> {
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

    fn original(range: std::ops::Range<usize>) -> Self {
        Self::original_part(0, range)
    }

    fn original_part(part: usize, range: std::ops::Range<usize>) -> Self {
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

    fn with_publication_collector(
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

    fn materialized_publication(&self) -> WordPublicationCoverage {
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

    fn publication_output(&self) -> std::sync::Arc<std::sync::Mutex<WordPublicationCoverage>> {
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

    fn with_word_source(
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

    fn word_range(&self, range: std::ops::Range<usize>) -> Option<std::ops::Range<usize>> {
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
    fn original_range(&self, range: std::ops::Range<usize>) -> Option<WordOriginalRange> {
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

/// A concrete byte source for original or explicitly planned source pieces.
/// Piece order is the producer's occurrence order. No output Vec is required
/// to run the actual parser/admission decisions.
#[derive(Clone, Copy)]
pub(crate) enum WordXmlInput<'a> {
    Contiguous(&'a [u8]),
    Pieces {
        chunks: &'a [&'a [u8]],
        index: usize,
        offset: usize,
    },
}
impl<'a> WordXmlInput<'a> {
    pub(crate) fn pieces(chunks: &'a [&'a [u8]]) -> Self {
        Self::Pieces {
            chunks,
            index: 0,
            offset: 0,
        }
    }
    fn len(self) -> Result<usize> {
        let mut total = 0usize;
        for chunk in self.chunks() {
            total = total
                .checked_add(chunk.len())
                .ok_or_else(|| OxmlError::InvalidValue("Word source byte overflow".into()))?;
        }
        Ok(total)
    }
    fn chunks(self) -> Vec<&'a [u8]> {
        match self {
            Self::Contiguous(raw) => vec![raw],
            Self::Pieces {
                chunks,
                index,
                offset,
            } => chunks
                .iter()
                .enumerate()
                .skip(index)
                .map(|(at, chunk)| {
                    if at == index {
                        &chunk[offset..]
                    } else {
                        *chunk
                    }
                })
                .collect(),
        }
    }
}
impl std::io::Read for WordXmlInput<'_> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        let input = std::io::BufRead::fill_buf(self)?;
        let bytes = input.len().min(output.len());
        output[..bytes].copy_from_slice(&input[..bytes]);
        std::io::BufRead::consume(self, bytes);
        Ok(bytes)
    }
}
impl std::io::BufRead for WordXmlInput<'_> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        match self {
            Self::Contiguous(raw) => Ok(raw),
            Self::Pieces {
                chunks,
                index,
                offset,
            } => {
                while *index < chunks.len() && *offset == chunks[*index].len() {
                    *index += 1;
                    *offset = 0;
                }
                Ok(chunks.get(*index).map_or(&[], |chunk| &chunk[*offset..]))
            }
        }
    }
    fn consume(&mut self, bytes: usize) {
        match self {
            Self::Contiguous(raw) => *raw = &raw[bytes.min(raw.len())..],
            Self::Pieces {
                chunks,
                index,
                offset,
            } => {
                if let Some(chunk) = chunks.get(*index) {
                    *offset += bytes.min(chunk.len() - *offset);
                }
            }
        }
    }
}

struct WordNamespaceBinding {
    uri: Vec<u8>,
    depth: usize,
}

struct WordAdmissionFrame {
    admitted: bool,
    word: bool,
    // An admitted legacy unbound run route, not a namespace URI binding.
    legacy_run_prefix: Option<Vec<u8>>,
    mc: bool,
    vml: bool,
    owner: Option<SdtOwner>,
    anchor_depth: Option<usize>,
    namespace_undo: Vec<(Vec<u8>, Option<WordNamespaceBinding>)>,
    mc_capture_depth: Option<usize>,
    qualified_drawing_depth: Option<usize>,
    control: bool,
    ordered_field_charged: bool,
    cost: usize,
    table_depth: usize,
    local: std::borrow::Cow<'static, [u8]>,
    paragraph_reader_root: bool,
    ordered_revision_root: bool,
    fact_start: usize,
    copy_fact: bool,
    copy_depth: usize,
    copy_scope_bytes: usize,
}

fn restore_namespace_depths(
    depths: &mut std::collections::HashMap<Vec<u8>, WordNamespaceBinding>,
    undo: Vec<(Vec<u8>, Option<WordNamespaceBinding>)>,
    copy_scope_weight: &mut Option<usize>,
) {
    for (prefix, previous) in undo.into_iter().rev() {
        *copy_scope_weight = None;
        if let Some(previous) = previous {
            depths.insert(prefix, previous);
        } else {
            depths.remove(&prefix);
        }
    }
}

fn decoded_namespace_is(uri: &[u8], expected: &str) -> bool {
    uri == expected.as_bytes()
        || std::str::from_utf8(uri)
            .ok()
            .and_then(|uri| quick_xml::escape::unescape(uri).ok())
            .is_some_and(|uri| uri == expected)
}

fn captured_namespace_is(
    binding: Option<&WordNamespaceBinding>,
    capture_depth: usize,
    expected: &str,
) -> bool {
    binding.is_some_and(|binding| {
        if binding.depth < capture_depth {
            decoded_namespace_is(&binding.uri, expected)
        } else {
            binding.uri == expected.as_bytes()
        }
    })
}

pub(crate) fn admitted_word_child(parent: &[u8], child: &[u8]) -> bool {
    match parent {
        b"document" => child == b"body",
        b"body" | b"hdr" | b"ftr" | b"comment" | b"footnote" | b"endnote" | b"docPartBody"
        | b"txbxContent" | b"tc" => matches!(child, b"p" | b"tbl" | b"sdt"),
        b"comments" => child == b"comment",
        b"footnotes" => child == b"footnote",
        b"endnotes" => child == b"endnote",
        b"glossaryDocument" => child == b"docParts",
        b"docParts" => child == b"docPart",
        b"docPart" => child == b"docPartBody",
        b"tbl" => matches!(child, b"tr" | b"sdt"),
        b"tr" => matches!(child, b"tc" | b"sdt"),
        b"p" => matches!(
            child,
            b"r" | b"sdt"
                | b"hyperlink"
                | b"fldSimple"
                | b"ruby"
                | b"ins"
                | b"del"
                | b"moveFrom"
                | b"moveTo"
        ),
        b"r" => matches!(child, b"drawing" | b"pict" | b"object"),
        b"sdt" => child == b"sdtContent",
        // The actual owner-specific SdtOwner predicate decides content below.
        b"sdtContent" => true,
        b"hyperlink" => matches!(child, b"r" | b"ins" | b"del" | b"moveFrom" | b"moveTo"),
        b"fldSimple" => matches!(
            child,
            b"r" | b"fldSimple"
                | b"hyperlink"
                | b"sdt"
                | b"ins"
                | b"del"
                | b"moveFrom"
                | b"moveTo"
                | b"ruby"
        ),
        b"rt" | b"rubyBase" => child == b"r",
        b"ruby" => matches!(child, b"rt" | b"rubyBase"),
        b"ins" | b"del" | b"moveFrom" | b"moveTo" => matches!(
            child,
            b"r" | b"sdt" | b"hyperlink" | b"ins" | b"del" | b"moveFrom" | b"moveTo"
        ),
        _ => false,
    }
}

pub(crate) fn word_owner_cost(path_cost: usize, increment: usize) -> Result<usize> {
    path_cost.checked_add(increment).filter(|cost| *cost <= 256).ok_or_else(|| {
        OxmlError::InvalidValue(
            "Word owner nesting exceeds weighted limit 256 (content control 1, typed text box 16)".into(),
        )
    })
}

/// Concrete borrowed owners used by the iterative pre-recursion guard.
#[derive(Clone, Copy)]
pub(crate) enum WordOwner<'a> {
    Body(&'a CT_Body),
    Paragraph(&'a CT_P),
    Run(&'a crate::text::CT_R),
    Control(&'a CT_Sdt),
    Table(&'a CT_Tbl),
    Row(&'a crate::table::CT_Row),
    Cell(&'a crate::table::CT_Tc),
    Field(&'a crate::text::Field),
    Revision(&'a CT_Revision),
    Drawing(&'a crate::drawing::CT_Drawing),
    Anchor(&'a crate::drawing::CT_Anchor),
}

impl<'a> WordOwner<'a> {
    pub(crate) fn key(self) -> (u8, usize) {
        match self {
            Self::Body(value) => (0, std::ptr::from_ref(value) as usize),
            Self::Paragraph(value) => (1, std::ptr::from_ref(value) as usize),
            Self::Run(value) => (2, std::ptr::from_ref(value) as usize),
            Self::Control(value) => (3, std::ptr::from_ref(value) as usize),
            Self::Table(value) => (4, std::ptr::from_ref(value) as usize),
            Self::Row(value) => (5, std::ptr::from_ref(value) as usize),
            Self::Cell(value) => (6, std::ptr::from_ref(value) as usize),
            Self::Field(value) => (7, std::ptr::from_ref(value) as usize),
            Self::Revision(value) => (8, std::ptr::from_ref(value) as usize),
            Self::Drawing(value) => (9, std::ptr::from_ref(value) as usize),
            Self::Anchor(value) => (10, std::ptr::from_ref(value) as usize),
        }
    }
    fn local_depth(self) -> (usize, usize) {
        match self {
            Self::Control(_) => (1, 0),
            Self::Table(_) => (0, 1),
            Self::Field(field) => (usize::from(field.has_ordered_result_owner()), 0),
            Self::Anchor(anchor)
                if anchor
                    .shape
                    .as_ref()
                    .is_some_and(|shape| shape.text_body.is_some() || !shape.text.is_empty()) =>
            {
                (16, 0)
            }
            _ => (0, 0),
        }
    }
    pub(crate) fn resource_children(self) -> Vec<Self> {
        use crate::content_control::SdtContent;
        use crate::table::CellContent;
        use crate::text::RunContent;
        let mut pending = Vec::new();
        let mut push = |child| pending.push(child);
        match self {
            WordOwner::Body(body) => {
                for child in &body.content {
                    match child {
                        BodyContent::Paragraph(p) => push(WordOwner::Paragraph(p)),
                        BodyContent::Table(t) => push(WordOwner::Table(t)),
                        BodyContent::ContentControl(s) => push(WordOwner::Control(s)),
                        BodyContent::RawXml(_) => {}
                    }
                }
            }
            WordOwner::Paragraph(p) => {
                for run in &p.runs {
                    push(WordOwner::Run(run));
                }
                for (_, _, _, control) in &p.content_controls {
                    push(WordOwner::Control(control));
                }
                for (_, _, revision) in &p.revisions {
                    push(WordOwner::Revision(revision));
                }
                for ruby in &p.rubies {
                    for run in &ruby.ruby_text {
                        push(WordOwner::Run(run));
                    }
                }
            }
            WordOwner::Run(run) => {
                for child in &run.content {
                    match child {
                        RunContent::Drawing(d) => push(WordOwner::Drawing(d)),
                        RunContent::Field(f) => push(WordOwner::Field(f)),
                        _ => {}
                    }
                }
                for drawing in &run.alt_drawings {
                    push(WordOwner::Drawing(drawing));
                }
            }
            WordOwner::Control(control) => {
                for run in control.original_resource_runs() {
                    pending.push(WordOwner::Run(run));
                }
                for (_, revision) in &control.revisions {
                    pending.push(WordOwner::Revision(revision));
                }
                for child in &control.content {
                    let child = match child {
                        SdtContent::Paragraph(p) => WordOwner::Paragraph(p),
                        SdtContent::Table(t) => WordOwner::Table(t),
                        SdtContent::Row(r) => WordOwner::Row(r),
                        SdtContent::Cell(c) => WordOwner::Cell(c),
                        SdtContent::Run(r) => WordOwner::Run(r),
                        SdtContent::ContentControl(s) => WordOwner::Control(s),
                        SdtContent::RawXml(_) => continue,
                    };
                    pending.push(child);
                }
            }
            WordOwner::Table(table) => {
                for row in &table.rows {
                    pending.push(WordOwner::Row(row));
                }
                for (_, _, control) in &table.content_controls {
                    pending.push(WordOwner::Control(control));
                }
            }
            WordOwner::Row(row) => {
                for cell in &row.cells {
                    push(WordOwner::Cell(cell));
                }
                for (_, _, control) in &row.content_controls {
                    push(WordOwner::Control(control));
                }
            }
            WordOwner::Cell(cell) => {
                for child in &cell.content {
                    match child {
                        CellContent::Paragraph(p) => push(WordOwner::Paragraph(p)),
                        CellContent::Table(t) => push(WordOwner::Table(t)),
                        CellContent::ContentControl(s) => push(WordOwner::Control(s)),
                    }
                }
            }
            WordOwner::Field(field) => {
                for child in field.resource_owner_children() {
                    pending.push(child);
                }
            }
            WordOwner::Revision(revision) => {
                if let Some(p) = revision.content_paragraph() {
                    push(WordOwner::Paragraph(p));
                }
                if let crate::revision::RevisionContent::Runs(runs) = revision.content() {
                    for run in runs {
                        push(WordOwner::Run(run));
                    }
                }
                for (_, revision) in revision.nested_revisions() {
                    push(WordOwner::Revision(revision));
                }
            }
            WordOwner::Drawing(drawing) => {
                if let Some(anchor) = &drawing.anchor {
                    push(WordOwner::Anchor(anchor));
                }
            }
            WordOwner::Anchor(anchor) => {
                if let Some(shape) = &anchor.shape
                    && (shape.text_body.is_some() || !shape.text.is_empty())
                {
                    if let Some(body) = &shape.text_body {
                        pending.push(WordOwner::Body(body));
                    }
                    // All stored models are cloned, including the authored
                    // legacy paragraphs if an authoritative body is present.
                    for p in &shape.text {
                        pending.push(WordOwner::Paragraph(p));
                    }
                }
            }
        }
        pending
    }
}

enum ModelAdmissionStep<'a> {
    Enter(WordOwner<'a>, usize, usize),
    Finish((u8, usize), (usize, usize), Vec<WordOwner<'a>>),
}

fn checked_model_table_depth(depth: usize, increment: usize) -> Result<usize> {
    depth
        .checked_add(increment)
        .filter(|depth| *depth <= crate::table::MAX_RECOGNIZED_TABLE_NESTING)
        .ok_or_else(|| {
            OxmlError::InvalidValue("recognized model nesting exceeds table limit".into())
        })
}

pub(crate) fn admit_word_model(root: WordOwner<'_>) -> Result<()> {
    word_model_maxima(root).map(|_| ())
}

pub(crate) fn word_model_maxima(root: WordOwner<'_>) -> Result<(usize, usize)> {
    word_model_children_maxima(&[root], 0)
}

/// Guard actual prospective children under their real not-yet-built parent.
/// The memo is shared across every alternative, not renewed for each child.
pub(crate) fn word_model_children_maxima(
    roots: &[WordOwner<'_>],
    parent_cost: usize,
) -> Result<(usize, usize)> {
    word_owner_cost(0, parent_cost)?;
    // None means an actually entered active node. Merely queued siblings are
    // not marked active, so shared siblings cannot masquerade as back edges.
    let mut summaries = std::collections::HashMap::<(u8, usize), Option<(usize, usize)>>::new();
    let mut pending = roots
        .iter()
        .copied()
        .map(|root| ModelAdmissionStep::Enter(root, parent_cost, 0))
        .collect::<Vec<_>>();
    while let Some(step) = pending.pop() {
        match step {
            ModelAdmissionStep::Enter(owner, cost, tables) => {
                let key = owner.key();
                let completed = match summaries.get(&key) {
                    Some(Some(summary)) => Some(*summary),
                    Some(None) => {
                        return Err(OxmlError::InvalidValue(
                            "Word owner resource graph contains an active cycle".into(),
                        ));
                    }
                    None => None,
                };
                if let Some((weighted, table_depth)) = completed
                    && cost.checked_add(weighted).is_some_and(|value| value <= 256)
                    && tables
                        .checked_add(table_depth)
                        .is_some_and(|value| value <= crate::table::MAX_RECOGNIZED_TABLE_NESTING)
                {
                    continue;
                }
                let local = owner.local_depth();
                // Check this actual entry before enumerating any invalid tail.
                let cost = word_owner_cost(cost, local.0)?;
                let tables = checked_model_table_depth(tables, local.1)?;
                if completed.is_none() {
                    summaries.insert(key, None);
                }
                let children = owner.resource_children();
                if completed.is_none() && children.is_empty() {
                    pending.push(ModelAdmissionStep::Finish(key, local, children));
                } else if completed.is_none() {
                    let finish = pending.len();
                    pending.push(ModelAdmissionStep::Finish(key, local, Vec::new()));
                    pending.extend(
                        children
                            .iter()
                            .copied()
                            .map(|child| ModelAdmissionStep::Enter(child, cost, tables)),
                    );
                    // Extension only appends. Install the original children
                    // before the loop can process this Finish or any Enter.
                    let ModelAdmissionStep::Finish(_, _, retained) = &mut pending[finish] else {
                        unreachable!("new summary Finish is unchanged while queuing children");
                    };
                    *retained = children;
                } else {
                    // Preserve the former append/LIFO visitation order. On an
                    // over-budget memo hit, fitting children are skipped by their
                    // maxima and the actual first offending entry still refuses.
                    pending.extend(
                        children
                            .into_iter()
                            .map(|child| ModelAdmissionStep::Enter(child, cost, tables)),
                    );
                }
            }
            ModelAdmissionStep::Finish(key, local, children) => {
                let mut maxima = (0, 0);
                for child in children {
                    let Some(Some(summary)) = summaries.get(&child.key()) else {
                        return Err(OxmlError::InvalidValue(
                            "Word owner resource summary is incomplete".into(),
                        ));
                    };
                    maxima.0 = maxima.0.max(summary.0);
                    maxima.1 = maxima.1.max(summary.1);
                }
                let summary = (
                    word_owner_cost(local.0, maxima.0)?,
                    checked_model_table_depth(local.1, maxima.1)?,
                );
                summaries.insert(key, Some(summary));
            }
        }
    }
    let mut maxima = (0, 0);
    for root in roots {
        let Some(Some(summary)) = summaries.get(&root.key()) else {
            return Err(OxmlError::InvalidValue(
                "Word owner resource summary is incomplete".into(),
            ));
        };
        maxima.0 = maxima.0.max(summary.0);
        maxima.1 = maxima.1.max(summary.1);
    }
    Ok(maxima)
}

#[derive(Default)]
struct CaptureDeclarations {
    prefixes: Vec<String>,
    namespaces: Vec<String>,
    declaration_bytes: Vec<std::result::Result<usize, &'static str>>,
    indices: std::collections::HashMap<u64, Vec<u32>>,
}

impl CaptureDeclarations {
    /// Identical prefix and namespace pairs share one pool entry. Later
    /// physical copies still charge every retained record. Hashes only select
    /// a lookup bucket. Exact pair equality remains mandatory.
    fn push(&mut self, prefix: String, namespace: String) -> Result<u32> {
        use std::hash::BuildHasher;
        let hash = self
            .indices
            .hasher()
            .hash_one((prefix.as_str(), namespace.as_str()));
        if let Some(indices) = self.indices.get(&hash)
            && let Some(index) = indices
                .iter()
                .copied()
                .find(|index| self.prefix(*index) == prefix && self.namespace(*index) == namespace)
        {
            return Ok(index);
        }
        let index = u32::try_from(self.prefixes.len())
            .map_err(|_| OxmlError::InvalidValue("original capture declaration overflow".into()))?;
        self.declaration_bytes
            .push(capture_declaration_bytes(&prefix, &namespace));
        self.prefixes.push(prefix);
        self.namespaces.push(namespace);
        self.indices.entry(hash).or_default().push(index);
        Ok(index)
    }

    /// Reusing an original fact never first clones its namespace URI.
    fn intern(&mut self, prefix: &str, namespace: &str) -> Result<u32> {
        use std::hash::BuildHasher;
        let hash = self.indices.hasher().hash_one((prefix, namespace));
        if let Some(indices) = self.indices.get(&hash)
            && let Some(index) = indices
                .iter()
                .copied()
                .find(|index| self.prefix(*index) == prefix && self.namespace(*index) == namespace)
        {
            return Ok(index);
        }
        self.push(prefix.to_owned(), namespace.to_owned())
    }

    fn prefix(&self, index: u32) -> &str {
        &self.prefixes[index as usize]
    }
    fn namespace(&self, index: u32) -> &str {
        &self.namespaces[index as usize]
    }
    fn declaration_bytes(&self, index: u32) -> Result<usize> {
        self.declaration_bytes[index as usize]
            .map_err(|message| OxmlError::InvalidValue(message.into()))
    }
}

#[derive(Clone, Copy)]
struct CaptureBinding {
    index: u32,
    depth: usize,
    declared: bool,
    /// Physical namespace attribute bytes already equal their decoded value.
    literal_namespace: bool,
}

/// Operation-local effective bindings. Empty XML scopes add no history entry.
struct CaptureNamespaceState {
    histories: std::collections::HashMap<String, Vec<CaptureBinding>>,
    word_aliases: std::collections::HashSet<String>,
}

impl CaptureNamespaceState {
    fn retain_part_declarations(&mut self, declarations: &[u32], pool: &CaptureDeclarations) {
        for &index in declarations {
            let history = self
                .histories
                .entry(pool.prefix(index).to_owned())
                .or_default();
            history.clear();
            history.push(CaptureBinding {
                index,
                depth: 0,
                declared: true,
                literal_namespace: true,
            });
        }
    }

    fn new(inherited: &[u32], pool: &CaptureDeclarations, aliases: &[String]) -> Self {
        let mut histories = std::collections::HashMap::<String, Vec<CaptureBinding>>::new();
        for &index in inherited {
            histories
                .entry(pool.prefix(index).to_owned())
                .or_default()
                .push(CaptureBinding {
                    index,
                    depth: 0,
                    declared: false,
                    literal_namespace: true,
                });
        }
        Self {
            histories,
            word_aliases: aliases
                .iter()
                .filter(|alias| !alias.starts_with('\0'))
                .cloned()
                .collect(),
        }
    }

    fn enter(
        &mut self,
        declarations: &[u32],
        depth: usize,
        pool: &CaptureDeclarations,
        literals: Option<&[bool]>,
    ) {
        for (at, &index) in declarations.iter().enumerate() {
            self.histories
                .entry(pool.prefix(index).to_owned())
                .or_default()
                .push(CaptureBinding {
                    index,
                    depth,
                    declared: true,
                    literal_namespace: literals.is_none_or(|values| values[at]),
                });
        }
    }

    fn leave(&mut self, declarations: &[u32], pool: &CaptureDeclarations) {
        for &index in declarations.iter().rev() {
            let history = self
                .histories
                .get_mut(pool.prefix(index))
                .expect("entered namespace prefix");
            let binding = history.pop().expect("entered namespace binding");
            debug_assert_eq!(binding.index, index);
        }
    }
}

struct OriginalCaptureScope {
    declarations: Vec<u32>,
    /// This element and its descendants are retained as one verbatim raw
    /// record, so modeled descendants are part of the capture source.
    raw: bool,
    /// This element is inside a captured WP inline or anchor raw record. The
    /// record closes against its ancestors, so only run-frame and drawing-scope
    /// bindings are copied into it.
    typed_drawing: bool,
    /// Inline drawing raw never constructs embedded Word runs.
    inline_drawing: bool,
    /// Inner lexical markup is decoded as a text value, never parsed as models.
    text_span: bool,
    /// This element is a direct `w:drawing` whose children the drawing parser
    /// either captures as one raw inline or anchor or discards.
    drawing_root: bool,
    /// The drawing parser discards this subtree with `read_to_end_into`, so no
    /// capture record is created for it.
    dropped: bool,
    /// This element is one physical inline or anchor raw stored by the drawing
    /// parser. Each such root gets its own injected declaration set.
    raw_copy: bool,
    /// Direct `w:r` children are constructed by this element's parser.
    constructs_runs: bool,
    /// Direct `w:hyperlink` children are constructed by this element's parser.
    constructs_hyperlinks: bool,
    /// Actual and synthetic wrapper paragraphs parse revisions without a
    /// required author.
    paragraph: bool,
    /// Ruby rejected the annotation, so its runs are not constructed.
    ruby_opaque: bool,
    /// A rejected fallible child retains bytes without constructing typed
    /// descendants. Its enclosing raw-capture action remains independent.
    unconstructed: bool,
    /// This direct run child owns its independently namespace-closed raw source.
    comment_root: bool,
}

struct OriginalCaptureOwner {
    depth: usize,
    /// Run and hyperlink parents have different direct-child grammars.
    run: bool,
    /// Bindings stored once on the shared run-frame record.
    shared: Vec<u32>,
    /// Run-root declarations stored again on the lexical local record.
    local: Vec<u32>,
    /// Declaration sets injected into each physical drawing raw.
    raw_copies: Vec<Vec<u32>>,
    open_raw: Vec<Vec<u32>>,
    /// The one direct comment-reference carrier currently being inspected.
    open_comment: Option<(usize, Vec<u32>)>,
}

struct OriginalCaptureClass {
    drawing_root: bool,
    typed_root: bool,
    inside_typed: bool,
    dropped: bool,
    raw: bool,
    owner: bool,
}

fn escaped_capture_len(value: &str) -> Result<usize> {
    value
        .bytes()
        .try_fold(0usize, |sum, byte| {
            sum.checked_add(match byte {
                b'&' => 5,
                b'<' | b'>' => 4,
                b'\'' | b'"' => 6,
                _ => 1,
            })
        })
        .ok_or_else(|| OxmlError::InvalidValue("original capture escaping overflow".into()))
}

fn capture_declaration_bytes(
    prefix: &str,
    namespace: &str,
) -> std::result::Result<usize, &'static str> {
    let mut bytes = 9usize;
    if !prefix.is_empty() {
        bytes = bytes
            .checked_add(prefix.len())
            .and_then(|bytes| bytes.checked_add(1))
            .ok_or("original capture declaration overflow")?;
    }
    let escaped =
        escaped_capture_len(namespace).map_err(|_| "original capture escaping overflow")?;
    bytes
        .checked_add(escaped)
        .ok_or("original capture declaration overflow")
}

fn capture_index_push(indices: &mut Vec<u32>, index: u32) {
    if !indices.contains(&index) {
        indices.push(index);
    }
}

fn classify_original_capture(
    empty: bool,
    word: bool,
    local: &[u8],
    modeled: bool,
    scope: &[OriginalCaptureScope],
    owners: &[OriginalCaptureOwner],
) -> OriginalCaptureClass {
    let parent_raw = scope.last().is_some_and(|parent| parent.raw);
    let parent_typed = scope.last().is_some_and(|parent| parent.typed_drawing);
    let parent_drawing_root = scope.last().is_some_and(|parent| parent.drawing_root);
    let parent_dropped = scope.last().is_some_and(|parent| parent.dropped);
    let direct_owner_child = owners
        .last()
        .is_some_and(|owner| scope.len() == owner.depth + 1);
    // Only a nonempty direct drawing under the run is inspected by the drawing
    // parser. Empty drawings stay verbatim raw children.
    let drawing_root = !empty
        && !parent_raw
        && !parent_typed
        && !parent_dropped
        && !parent_drawing_root
        && direct_owner_child
        && owners.last().is_some_and(|owner| owner.run)
        && word
        && local == b"drawing";
    // The drawing parser installs only a Start inline or anchor, matched by
    // local name. Empty inline and anchor elements are discarded.
    let typed_root = parent_drawing_root
        && crate::drawing::DrawingContainerKind::for_name(local, empty).is_some();
    let inside_typed = parent_typed || typed_root;
    let dropped = parent_dropped || (parent_drawing_root && !typed_root);
    let parent = scope.last();
    // Paragraph properties, annotations and other retained children cannot
    // introduce run owners merely by containing qualified-looking names.
    // These two wrappers are different: accepted reads expand their content
    // through the same paragraph parser.
    let opaque_paragraph_child = parent.is_some_and(|parent| parent.paragraph)
        && !(word
            && (admitted_word_child(b"p", local) || matches!(local, b"smartTag" | b"customXml")));
    let constructed = if scope.is_empty() {
        true
    } else {
        parent.is_some_and(|parent| match local {
            b"r" => parent.constructs_runs,
            b"hyperlink" => parent.constructs_hyperlinks,
            _ => false,
        })
    };
    // A run or hyperlink is an owner only when the enclosing constructor
    // parses that child. Property leaves, opaque extensions, and rejected
    // ruby annotations keep the bytes without manufacturing a nested owner.
    let owner = word
        && matches!(local, b"r" | b"hyperlink")
        && constructed
        && !parent_raw
        && !parent_dropped
        && !dropped
        && !parent.is_some_and(|parent| {
            parent.ruby_opaque || parent.unconstructed || parent.inline_drawing || parent.text_span
        });
    // Namespace membership is not a modeled-child test. Unknown Word-local
    // children such as contentPart stay raw. Descendants of an opaque or
    // discarded payload are not new owners.
    let raw = !inside_typed
        && !dropped
        && !drawing_root
        && !owner
        && (parent_raw || opaque_paragraph_child || (direct_owner_child && !modeled));
    OriginalCaptureClass {
        drawing_root,
        typed_root,
        inside_typed,
        dropped,
        raw,
        owner,
    }
}

fn inherited_capture_binding(value: &str) -> (&str, &str) {
    value
        .strip_prefix('\0')
        .and_then(|rest| rest.split_once('\0'))
        .unwrap_or((value, W_NS))
}

fn reduce_inherited_bindings(prefixes: &[String]) -> Vec<(&str, &str)> {
    let mut last = std::collections::HashMap::new();
    for (position, value) in prefixes.iter().enumerate() {
        last.insert(inherited_capture_binding(value).0, position);
    }
    prefixes
        .iter()
        .enumerate()
        .filter_map(|(position, value)| {
            let (prefix, namespace) = inherited_capture_binding(value);
            (last.get(prefix) == Some(&position)).then_some((prefix, namespace))
        })
        .collect()
}

fn original_capture_binding(
    prefix: &str,
    namespace_state: &CaptureNamespaceState,
) -> Option<(u32, usize)> {
    namespace_state
        .histories
        .get(prefix)
        .and_then(|history| history.last())
        .map(|binding| (binding.index, binding.depth))
}

fn original_capture_root_attributes(
    start: &BytesStart<'_>,
    namespace_state: &CaptureNamespaceState,
    pool: &CaptureDeclarations,
) -> WordSourceResult<(usize, usize)> {
    let mut attribute_bytes = 0usize;
    let mut attributes = 0usize;
    let mut used = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(OxmlError::from)?;
        let name = attribute.key.as_ref();
        if name == b"xmlns" || name.starts_with(b"xmlns:") {
            continue;
        }
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
            .map_err(OxmlError::from)?
            .into_owned();
        let (prefix, local) = match name.iter().position(|byte| *byte == b':') {
            Some(separator) => (
                std::str::from_utf8(&name[..separator]).map_err(OxmlError::from)?,
                &name[separator + 1..],
            ),
            None => ("", name),
        };
        let namespace = if prefix == "xml" {
            Some("http://www.w3.org/XML/1998/namespace")
        } else {
            original_capture_binding(prefix, namespace_state)
                .map(|(index, _)| pool.namespace(index))
        };
        for value_prefix in crate::text::root_attribute_value_prefixes(namespace, local, &value) {
            if value_prefix == "xml"
                || used
                    .iter()
                    .any(|candidate: &String| candidate == value_prefix)
            {
                continue;
            }
            used.push(value_prefix.to_owned());
        }
        if prefix != "xml" && !used.iter().any(|candidate| candidate == prefix) {
            used.push(prefix.to_owned());
        }
        attribute_bytes = attribute_bytes
            .checked_add(name.len())
            .and_then(|bytes| bytes.checked_add(escaped_capture_len(&value).ok()?))
            .and_then(|bytes| bytes.checked_add(4))
            .ok_or_else(|| {
                WordSourceFailure::resource("original capture attribute overflow".into())
            })?;
        attributes += 1;
    }
    if attributes == 0 {
        return Ok((0, 0));
    }
    let mut record = attribute_bytes
        .checked_add(22)
        .ok_or_else(|| WordSourceFailure::resource("original capture record overflow".into()))?;
    for prefix in used {
        let Some((index, _)) = original_capture_binding(&prefix, namespace_state) else {
            continue;
        };
        record = record
            .checked_add(
                pool.declaration_bytes(index)
                    .map_err(WordSourceFailure::Resource)?,
            )
            .ok_or_else(|| {
                WordSourceFailure::resource("original capture record overflow".into())
            })?;
    }
    Ok((record, attribute_bytes))
}

fn original_capture_event_prefixes(
    start: &BytesStart<'_>,
    namespace_state: &CaptureNamespaceState,
    pool: &CaptureDeclarations,
) -> Result<Vec<String>> {
    let name = start.name();
    let name = name.as_ref();
    let mut used = Vec::new();
    // An unprefixed element uses the default namespace. An unprefixed
    // attribute does not, matching record_external_bindings_used.
    let prefix = match name.iter().position(|byte| *byte == b':') {
        Some(separator) => std::str::from_utf8(&name[..separator])?,
        None => "",
    };
    if prefix != "xml" {
        used.push(prefix.to_owned());
    }
    for attribute in start.attributes() {
        let attribute = attribute?;
        let key = attribute.key.as_ref();
        if key == b"xmlns" || key.starts_with(b"xmlns:") {
            continue;
        }
        let Some(separator) = key.iter().position(|byte| *byte == b':') else {
            continue;
        };
        let prefix = std::str::from_utf8(&key[..separator])?;
        if prefix != "xml" && !used.iter().any(|candidate| candidate == prefix) {
            used.push(prefix.to_owned());
        }
        let Some((index, _)) = original_capture_binding(prefix, namespace_state) else {
            continue;
        };
        let local = &key[separator + 1..];
        // Opaque payload values stay verbatim. Only a decodable namespace-
        // valued attribute contributes dependency prefixes. An undecodable
        // raw value is not a reason to refuse the whole input here.
        let Ok(value) =
            attribute.decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
        else {
            continue;
        };
        for value_prefix in
            crate::text::root_attribute_value_prefixes(Some(pool.namespace(index)), local, &value)
        {
            if value_prefix == "xml"
                || used
                    .iter()
                    .any(|candidate: &String| candidate == value_prefix)
            {
                continue;
            }
            used.push(value_prefix.to_owned());
        }
    }
    Ok(used)
}

fn capture_non_namespace_attribute(start: &BytesStart<'_>) -> bool {
    start.attributes().flatten().any(|attribute| {
        let key = attribute.key.as_ref();
        key != b"xmlns" && !key.starts_with(b"xmlns:")
    })
}

fn original_capture_word_name(
    name: &[u8],
    attribute: bool,
    namespace_state: &CaptureNamespaceState,
    pool: &CaptureDeclarations,
) -> bool {
    let prefix = match name.iter().position(|byte| *byte == b':') {
        Some(separator) => std::str::from_utf8(&name[..separator]).ok(),
        None if !attribute => Some(""),
        None => None,
    };
    let Some(prefix) = prefix else {
        return false;
    };
    namespace_state
        .histories
        .get(prefix)
        .and_then(|history| history.last())
        .is_some_and(|binding| {
            (binding.declared || namespace_state.word_aliases.contains(prefix))
                && pool.namespace(binding.index) == W_NS
        })
}

fn constructor_namespace_name(
    name: &[u8],
    local: &[u8],
    namespace: &str,
    state: &CaptureNamespaceState,
    pool: &CaptureDeclarations,
    physical_since: usize,
) -> bool {
    if !matches_local_name(name, local) {
        return false;
    }
    let prefix = name
        .iter()
        .position(|byte| *byte == b':')
        .map_or(Ok(""), |at| std::str::from_utf8(&name[..at]));
    // These actual drawing consumers compare canonical ASCII namespace URIs.
    // NsReader preserves physical declaration bytes inside the captured
    // container. External bindings are decoded and emitted by its real closure.
    prefix
        .ok()
        .and_then(|prefix| state.histories.get(prefix)?.last())
        .is_some_and(|binding| {
            pool.namespace(binding.index) == namespace
                && (binding.depth < physical_since || binding.literal_namespace)
        })
}

fn drawing_metadata_after_capture((input, length): (WordXmlInput<'_>, usize)) -> Result<u32> {
    let mut reader = Reader::from_reader(std::io::Read::take(input, length as u64));
    let mut buffer = Vec::new();
    match reader.read_event_into(&mut buffer)? {
        Event::Start(start) | Event::Empty(start) => first_drawing_metadata_id(&start),
        _ => Err(OxmlError::MissingElement("wp:docPr".into())),
    }
}

fn first_drawing_metadata_id(start: &BytesStart<'_>) -> Result<u32> {
    for attribute in start.attributes() {
        let attribute = attribute?;
        if let Some(value) = crate::drawing::drawing_id_value(
            &attribute,
            attribute.key.as_ref() == b"id",
            start.decoder(),
        )? {
            return Ok(value);
        }
    }
    Err(OxmlError::InvalidValue(
        "wp:docPr requires an unqualified id attribute".into(),
    ))
}

/// Only aliases physically used by this element enter the scalar view.
/// The actual scalar decoders retain their attribute-selection rules.
fn constructor_attribute_prefixes(
    start: &BytesStart<'_>,
    namespace_state: &CaptureNamespaceState,
    pool: &CaptureDeclarations,
) -> Result<Vec<String>> {
    let mut prefixes = Vec::new();
    let name = start.name();
    if original_capture_word_name(name.as_ref(), false, namespace_state, pool) {
        let prefix = name
            .as_ref()
            .iter()
            .position(|byte| *byte == b':')
            .map_or("", |at| {
                std::str::from_utf8(&name.as_ref()[..at]).unwrap_or("")
            });
        prefixes.push(prefix.to_owned());
    }
    for attribute in start.attributes() {
        let attribute = attribute?;
        let name = attribute.key.as_ref();
        if original_capture_word_name(name, true, namespace_state, pool) {
            let at = name
                .iter()
                .position(|byte| *byte == b':')
                .expect("qualified attribute");
            let prefix = std::str::from_utf8(&name[..at])?;
            if !prefixes.iter().any(|known| known == prefix) {
                prefixes.push(prefix.to_owned());
            }
        }
    }
    Ok(prefixes)
}

/// Only lexical attribute prefixes can make this decoder fail resolution.
/// Namespace-valued payload dependencies are charged independently by capture.
fn root_attribute_plan_failed(
    start: &BytesStart<'_>,
    namespaces: &CaptureNamespaceState,
    pool: &CaptureDeclarations,
) -> Result<bool> {
    let mut bindings = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute?;
        let Some(at) = attribute.key.as_ref().iter().position(|byte| *byte == b':') else {
            continue;
        };
        let Ok(prefix) = std::str::from_utf8(&attribute.key.as_ref()[..at]) else {
            return Ok(true);
        };
        if prefix == "xmlns"
            || prefix == "xml"
            || bindings.iter().any(|(known, _)| *known == prefix)
        {
            continue;
        }
        if let Some((index, _)) = original_capture_binding(prefix, namespaces) {
            bindings.push((pool.prefix(index), pool.namespace(index)));
        } else if let Some(alias) = namespaces.word_aliases.get(prefix) {
            bindings.push((alias.as_str(), W_NS));
        }
    }
    Ok(crate::text::RootAttributePlan::read(start, &bindings).is_err())
}

fn simple_field_header_projection_stable(
    start: &BytesStart<'_>,
    namespace_state: &CaptureNamespaceState,
    pool: &CaptureDeclarations,
) -> Result<bool> {
    for attribute in start.attributes() {
        let attribute = attribute?;
        let key = attribute.key.as_ref();
        let Some(at) = key.iter().position(|byte| *byte == b':') else {
            continue;
        };
        if &key[at + 1..] != b"instr" {
            continue;
        }
        let prefix = std::str::from_utf8(&key[..at])?;
        if original_capture_binding(prefix, namespace_state)
            .is_some_and(|(index, _)| pool.namespace(index) == W_NS)
        {
            return Ok(original_capture_word_name(key, true, namespace_state, pool));
        }
    }
    Ok(true)
}

fn capture_revision_opens_runs(
    start: &BytesStart<'_>,
    parent_paragraph: bool,
    namespace_state: &CaptureNamespaceState,
    pool: &CaptureDeclarations,
) -> Result<bool> {
    let prefixes = constructor_attribute_prefixes(start, namespace_state, pool)?;
    Ok(crate::revision::RevisionHeader::read(start, &prefixes, !parent_paragraph).is_ok())
}

fn effective_run_frame_binding(
    prefix: &str,
    owner_depth: usize,
    namespace_state: &CaptureNamespaceState,
) -> Option<(u32, usize)> {
    let history = namespace_state.histories.get(prefix)?;
    let end = history.partition_point(|binding| binding.depth <= owner_depth);
    history
        .get(end.checked_sub(1)?)
        .map(|binding| (binding.index, binding.depth))
}

fn charge_original_capture_binding(
    owner: &mut OriginalCaptureOwner,
    index: u32,
    depth: usize,
    pool: &CaptureDeclarations,
    inside_typed: bool,
    namespace_state: &CaptureNamespaceState,
) {
    let canonical_w = pool.prefix(index) == "w" && pool.namespace(index) == W_NS;
    let in_frame = depth <= owner.depth;
    let drawing_scope = depth == owner.depth + 1;
    if inside_typed && drawing_scope && !in_frame {
        // lost_bindings injects a drawing declaration only when the run frame
        // does not already carry the same prefix and URI. A repeated pair is
        // the ancestor binding on the shared record, and on the local record
        // when that ancestor declaration sits on the run itself.
        if let Some((frame_index, frame_depth)) =
            effective_run_frame_binding(pool.prefix(index), owner.depth, namespace_state)
            && pool.prefix(frame_index) == pool.prefix(index)
            && pool.namespace(frame_index) == pool.namespace(index)
        {
            capture_index_push(&mut owner.shared, index);
            if frame_depth == owner.depth && !canonical_w {
                capture_index_push(&mut owner.local, index);
            }
        } else if let Some(copy) = owner.open_raw.last_mut() {
            capture_index_push(copy, index);
        }
    } else if inside_typed && in_frame {
        capture_index_push(&mut owner.shared, index);
        // A run-root declaration used by a drawing is stored on the shared
        // record and again on the lexical local record.
        if depth == owner.depth && !canonical_w {
            capture_index_push(&mut owner.local, index);
        }
    } else if in_frame && !canonical_w {
        capture_index_push(&mut owner.shared, index);
    }
}

fn add_capture_declaration(
    total_needed: &mut usize,
    facts: &mut Vec<u32>,
    index: u32,
    pool: &CaptureDeclarations,
) -> Result<()> {
    *total_needed = total_needed
        .checked_add(pool.declaration_bytes(index)?)
        .ok_or_else(|| OxmlError::InvalidValue("original capture work overflow".into()))?;
    capture_index_push(facts, index);
    Ok(())
}

fn add_capture_record(
    indices: &[u32],
    total_needed: &mut usize,
    facts: &mut Vec<u32>,
    pool: &CaptureDeclarations,
) -> Result<()> {
    if indices.is_empty() {
        return Ok(());
    }
    *total_needed = total_needed
        .checked_add(22)
        .ok_or_else(|| OxmlError::InvalidValue("original capture work overflow".into()))?;
    for index in indices {
        add_capture_declaration(total_needed, facts, *index, pool)?;
    }
    Ok(())
}

fn finish_original_capture_owner(
    owner: OriginalCaptureOwner,
    total_needed: &mut usize,
    facts: &mut Vec<u32>,
    pool: &CaptureDeclarations,
) -> Result<()> {
    add_capture_record(&owner.shared, total_needed, facts, pool)?;
    add_capture_record(&owner.local, total_needed, facts, pool)?;
    for copy in &owner.raw_copies {
        for index in copy {
            add_capture_declaration(total_needed, facts, *index, pool)?;
        }
    }
    for copy in &owner.open_raw {
        for index in copy {
            add_capture_declaration(total_needed, facts, *index, pool)?;
        }
    }
    if let Some((_, copy)) = &owner.open_comment {
        for index in copy {
            add_capture_declaration(total_needed, facts, *index, pool)?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum ConstructorKind {
    Opaque,
    Unconstructed,
    Document,
    Notes,
    Comments,
    ParagraphCollection,
    PartWrapper,
    Glossary,
    DocParts,
    DocPart,
    Body,
    Table,
    Row,
    Cell,
    Paragraph,
    Run,
    Drawing,
    DrawingContainer(crate::drawing::DrawingContainerKind),
    DrawingWalk(crate::drawing::DrawingContainerKind),
    DrawingConsumed,
    DrawingDocPr,
    DrawingDecorative(usize, bool),
    Hyperlink,
    RevisionHyperlink,
    SimpleFieldUnknown,
    SimpleFieldRaw,
    SimpleFieldResult,
    Ruby,
    RubyProperties,
    RubyContent,
    RunText,
    Revision(bool),
    ParagraphProperties,
    NumberingProperties,
    ParagraphBorders,
    SectionProperties,
    TableProperties,
    TableGrid,
    TableBorders,
    TableMargins,
    RowProperties,
    CellProperties,
    ControlProperties,
    Tabs,
    RunProperties,
    Control(SdtOwner),
    ControlContent(SdtOwner),
}

/// The three complete readers whose outer loops retain wrapper scope across End.
#[derive(Clone, Copy)]
pub(crate) enum PartReaderKind {
    HeaderFooter,
    Notes,
    Comments,
}

#[derive(Clone, Copy)]
pub(crate) enum WordSourceReader {
    StandaloneParagraph,
    Part(PartReaderKind),
    OrderedRevision {
        require_author: bool,
    },
    SimpleParagraph {
        opening_end: usize,
        result_start: usize,
        result_end: usize,
    },
}

impl PartReaderKind {
    fn action(self, local: &[u8], word: bool, empty: bool) -> ConstructorKind {
        use ConstructorKind as K;
        match self {
            Self::HeaderFooter if word && local == b"p" => K::Paragraph,
            Self::HeaderFooter if word && !empty && matches!(local, b"hdr" | b"ftr") => {
                K::PartWrapper
            }
            Self::Notes if word && !empty && matches!(local, b"footnotes" | b"endnotes") => {
                K::PartWrapper
            }
            Self::Notes if word && !empty && matches!(local, b"footnote" | b"endnote") => {
                K::ParagraphCollection
            }
            Self::Comments if word && !empty && local == b"comments" => K::PartWrapper,
            Self::Comments if word && !empty && local == b"comment" => K::ParagraphCollection,
            _ => K::Unconstructed,
        }
    }
}

/// Global dispatch and only its original wrapper declaration updates. Item
/// readers keep lexical scopes. Wrapper End never restores global reader scope.
#[derive(Debug)]
struct PartReaderEvent {
    start: usize,
    kind: ConstructorKind,
    updates: Vec<u32>,
}

struct ConstructorFrame<'a> {
    kind: ConstructorKind,
    start: usize,
    failed: bool,
    content_seen: bool,
    properties_seen: bool,
    control_occurrence: Option<usize>,
    /// Decoded at the source occurrence, applied only at the actual late boundary.
    root_attribute_failed: bool,
    /// Success is proved only for shared primitive actions and exact cursor modes.
    known: bool,
    /// First actual Option::None boundary, distinct from a propagated Err.
    ruby_none_at: Option<usize>,
    /// Reached header only. Decode its scalar after this Started child closes.
    ruby_property_opening: Option<(WordXmlInput<'a>, usize)>,
    revision_mode: Option<crate::revision::RevisionBodyMode>,
    revision_stopped: bool,
    lexical_revision: bool,
    revision_height: usize,
    drawing_stopped: bool,
    drawing_has_id: bool,
    drawing_metadata_opening: Option<(WordXmlInput<'a>, usize)>,
    /// Nearest active containing constructor, excluding this frame itself.
    drawing_owner: Option<usize>,
    source_depth: usize,
    /// A reached typed body needs its own resource admission before late facts.
    drawing_prefix_unknown: bool,
}

impl ConstructorFrame<'_> {
    fn record_ruby_property(&mut self, outcome: Result<crate::ruby::RubyPropertyChild>) {
        match outcome {
            Ok(crate::ruby::RubyPropertyChild::Raw) => self.content_seen = true,
            Ok(_) => {}
            Err(_) => {
                self.failed = true;
                self.known = false;
            }
        }
    }
}

#[derive(Debug)]
struct SimpleSelectorOccurrence {
    opening: std::ops::Range<usize>,
    whole: std::ops::Range<usize>,
    ordered: bool,
    result: std::ops::Range<usize>,
}

struct DirectSimpleSelectorFold {
    opening: std::ops::Range<usize>,
    result_start: usize,
    ordered: bool,
    result_seen: bool,
    traversed: usize,
    consumed: Option<(usize, bool)>,
    reusable: bool,
}
impl DirectSimpleSelectorFold {
    fn element(
        &mut self,
        local: &[u8],
        word: bool,
        empty: bool,
        physical_depth: usize,
        after: usize,
    ) {
        if self.consumed.is_some() {
            return;
        }
        match crate::text::simple_ownership_action(
            local,
            word,
            self.traversed,
            self.result_seen,
            empty,
        ) {
            crate::text::SimpleOwnershipAction::LeadingData => {
                if empty {
                    self.result_start = after;
                } else {
                    self.consumed = Some((physical_depth, true));
                }
            }
            crate::text::SimpleOwnershipAction::Traverse { ordered } => {
                self.result_seen = true;
                self.ordered |= ordered;
                self.traversed += 1;
            }
            crate::text::SimpleOwnershipAction::Consume { ordered } => {
                self.result_seen = true;
                self.ordered |= ordered;
                self.consumed = Some((physical_depth, false));
            }
            crate::text::SimpleOwnershipAction::Empty { ordered } => {
                self.result_seen = true;
                self.ordered |= ordered;
            }
            crate::text::SimpleOwnershipAction::Root => {
                unreachable!("selector owner already entered")
            }
        }
    }
    fn end(&mut self, physical_depth: usize, after: usize) -> bool {
        if let Some((depth, data)) = self.consumed {
            if depth == physical_depth {
                self.consumed = None;
                if data {
                    self.result_start = after;
                }
            }
            return false;
        }
        self.traversed -= 1;
        self.traversed == 0
    }
}

#[derive(Debug)]
pub(crate) struct ConstructorFailures {
    raw_subtrees: Vec<RawConstructorSpan>,
    root_raw: bool,
    controls: Vec<ControlOccurrence>,
    discarded: Vec<std::ops::Range<usize>>,
    simple_headers: Vec<SimpleHeaderOccurrence>,
    part_events: Vec<PartReaderEvent>,
    paragraph_result_root: Option<usize>,
    ordered_revision_root: Option<usize>,
    selector_paragraph_root: Option<std::ops::Range<usize>>,
    simple_selectors: Vec<SimpleSelectorOccurrence>,
}

#[derive(Debug)]
struct SimpleHeaderOccurrence {
    /// Identity in the actual admission input before explicit insertion mapping.
    original_opening: std::ops::Range<usize>,
    original_part: usize,
    opening: std::ops::Range<usize>,
    header: crate::text::SimpleFieldHeader,
}

impl ConstructorFailures {
    fn with_original_offset(mut self, part: usize, offset: usize) -> Result<Self> {
        for fact in &mut self.simple_headers {
            fact.original_part = part;
        }
        for fact in &mut self.controls {
            fact.original_part = part;
        }
        for span in self
            .simple_headers
            .iter_mut()
            .map(|fact| &mut fact.original_opening)
            .chain(self.controls.iter_mut().map(|fact| &mut fact.source_span))
        {
            *span = span.start.checked_add(offset).ok_or_else(|| {
                OxmlError::InvalidValue("original constructor position overflow".into())
            })?..span.end.checked_add(offset).ok_or_else(|| {
                OxmlError::InvalidValue("original constructor position overflow".into())
            })?;
        }
        Ok(self)
    }

    fn after_insertion(mut self, at: usize, bytes: usize) -> Result<Self> {
        self.selector_paragraph_root = None;
        self.simple_selectors.clear();
        let shifted = |position: usize| {
            if position < at {
                Ok(position)
            } else {
                position.checked_add(bytes).ok_or_else(|| {
                    OxmlError::InvalidValue("derived constructor position overflow".into())
                })
            }
        };
        for span in self
            .raw_subtrees
            .iter_mut()
            .map(|fact| &mut fact.span)
            .chain(self.discarded.iter_mut())
            .chain(self.simple_headers.iter_mut().map(|fact| &mut fact.opening))
        {
            span.start = shifted(span.start)?;
            span.end = shifted(span.end)?;
        }
        if let Some(root) = &mut self.paragraph_result_root {
            *root = shifted(*root)?;
        }
        if let Some(root) = &mut self.ordered_revision_root {
            *root = shifted(*root)?;
        }
        // Control cursors consume preorder occurrences. Their diagnostic source
        // positions remain original positions, not expanded-input identities.
        Ok(self)
    }
}

#[derive(Debug)]
struct RawConstructorSpan {
    span: std::ops::Range<usize>,
    /// Controls replace their complete owner with a raw capture. Other fallible
    /// children retain their parent's capture policy and stop typed children.
    control: bool,
}

// The same live input/reader suffix establishes physical displacement. A BOM
// is removed during reader initialization without entering its logical count.
// No equal-content or unrelated empty slice can establish this relationship.
pub(crate) fn reader_input_origin(input: &[u8], reader: &Reader<&[u8]>) -> Option<usize> {
    let remaining = *reader.get_ref();
    let physical = input.len().checked_sub(remaining.len())?;
    if (input.as_ptr() as usize).checked_add(physical) != Some(remaining.as_ptr() as usize) {
        return None;
    }
    let logical = usize::try_from(reader.buffer_position()).ok()?;
    let origin = physical.checked_sub(logical)?;
    (origin == 0 || origin == 3 && input.starts_with(b"\xef\xbb\xbf")).then_some(origin)
}

/// An original source occurrence, never a derived-byte or equal-text identity.
#[derive(Debug)]
struct ControlOccurrence {
    owner: SdtOwner,
    original_part: usize,
    source_span: std::ops::Range<usize>,
    after: usize,
    raw: bool,
}

pub(crate) struct WordFieldInput {
    initial_delivery: bool,
    map: std::sync::Arc<WordSourceMap>,
    registry: std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>>,
}

impl WordFieldInput {
    pub(crate) fn produced(
        map: WordSourceMap,
        registry: std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>>,
    ) -> Self {
        Self {
            initial_delivery: false,
            map: std::sync::Arc::new(map),
            registry,
        }
    }

    /// Bind the real immutable capture record as another CURRENT-coordinate
    /// domain. Keep the returned numeric collector live through its constructor
    /// and freeze its roles before returning a map beyond that record scope.
    /// Existing A correspondence and first-delivery authority are unchanged.
    pub(crate) fn record_publication(
        &self,
        raw: &[u8],
    ) -> Result<(
        Self,
        std::sync::Arc<std::sync::Mutex<WordPublicationCoverage>>,
    )> {
        if raw.len() != self.map.len() {
            return Err(OxmlError::InvalidValue(
                "publication capture record length changed".into(),
            ));
        }
        let collector =
            std::sync::Arc::new(std::sync::Mutex::new(WordPublicationCoverage::default()));
        let mut map = (*self.map).clone();
        map.publication.push(WordPublicationPiece {
            output: 0..raw.len(),
            current: 0..raw.len(),
            collector: std::sync::Arc::downgrade(&collector),
        });
        Ok((
            Self {
                initial_delivery: self.initial_delivery,
                map: std::sync::Arc::new(map),
                registry: std::sync::Arc::clone(&self.registry),
            },
            collector,
        ))
    }

    pub(crate) fn property_observation(
        &self,
        selected: std::ops::Range<usize>,
    ) -> Option<WordOrdinaryFact> {
        (selected.start < selected.end && selected.end <= self.map.len()).then(|| {
            WordOrdinaryFact {
                map: std::sync::Arc::clone(&self.map),
                selected,
            }
        })
    }

    pub(crate) fn registry(
        &self,
    ) -> &std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>> {
        &self.registry
    }

    pub(crate) fn resource_failure(&self) -> Option<OxmlError> {
        self.registry
            .lock()
            .expect("field origin registry poisoned")
            .source_resource_failure()
    }

    pub(crate) fn projection_parts(
        &self,
    ) -> (
        &WordSourceMap,
        std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>>,
        bool,
    ) {
        (
            &self.map,
            std::sync::Arc::clone(&self.registry),
            self.initial_delivery,
        )
    }

    pub(crate) fn slice(&self, range: std::ops::Range<usize>) -> Result<Self> {
        Ok(Self {
            initial_delivery: self.initial_delivery,
            map: std::sync::Arc::new(self.map.slice(range)?),
            registry: std::sync::Arc::clone(&self.registry),
        })
    }

    pub(crate) fn with_shell(&self, prefix: usize, suffix: usize) -> Result<Self> {
        Ok(Self {
            initial_delivery: self.initial_delivery,
            map: std::sync::Arc::new(WordSourceMap::concat(&[
                WordSourceMap::generated(prefix),
                (*self.map).clone(),
                WordSourceMap::generated(suffix),
            ])?),
            registry: std::sync::Arc::clone(&self.registry),
        })
    }

    /// Preserve this actual constructor input through its namespace closure.
    /// The flag comes only from this input, never from its Original pieces.
    pub(crate) fn close_namespaces(
        &self,
        raw: &[u8],
        bindings: &[(String, String)],
    ) -> Result<(Vec<u8>, Self)> {
        if self.map.len() != raw.len() {
            return Err(OxmlError::InvalidValue(
                "mapped capture input length changed".into(),
            ));
        }
        let (bytes, map) = crate::text::raw_with_external_bindings_and_map(
            raw,
            bindings,
            None,
            Some(std::sync::Arc::clone(&self.map)),
        )?;
        // The closure's concrete insertion transcript normally preserves Some.
        // An unproved map stays Generated and cannot supply input authority.
        let map = map.unwrap_or_else(|| std::sync::Arc::new(WordSourceMap::generated(bytes.len())));
        if map.len() != bytes.len() {
            return Err(OxmlError::InvalidValue(
                "mapped capture output length changed".into(),
            ));
        }
        Ok((
            bytes,
            Self {
                initial_delivery: self.initial_delivery,
                map,
                registry: std::sync::Arc::clone(&self.registry),
            },
        ))
    }

    pub(crate) fn cursor<'a>(&self, input: &'a [u8]) -> Result<ControlSourceCursor<'a>> {
        if self.map.len() != input.len() {
            return Err(OxmlError::InvalidValue(
                "mapped field reader input length changed".into(),
            ));
        }
        Ok(ControlSourceCursor {
            occurrences: &[],
            next: 0,
            end: 0,
            projected_aliases: false,
            initial_field_delivery: self.initial_delivery,
            simple_headers: &[],
            simple_selectors: &[],
            selector_paragraph_root: None,
            selector_requested: false,
            selector_ready: false,
            input: Some(input),
            source_map: std::sync::Arc::clone(&self.map),
            field_origins: Some(std::sync::Arc::clone(&self.registry)),
        })
    }

    pub(crate) fn register(&self, field: &crate::text::Field) -> Result<()> {
        self.registry
            .lock()
            .expect("field origin registry poisoned")
            .register_initial(field, (*self.map).clone(), self.initial_delivery)
    }
}

/// A disjoint preorder range reserved by the actual parent constructor.
/// Root namespace injection and capture retain element occurrence order.
pub(crate) struct ControlSourceCursor<'a> {
    occurrences: &'a [ControlOccurrence],
    next: usize,
    end: usize,
    projected_aliases: bool,
    initial_field_delivery: bool,
    simple_headers: &'a [SimpleHeaderOccurrence],
    simple_selectors: &'a [SimpleSelectorOccurrence],
    selector_paragraph_root: Option<&'a std::ops::Range<usize>>,
    selector_requested: bool,
    selector_ready: bool,
    input: Option<&'a [u8]>,
    source_map: std::sync::Arc<WordSourceMap>,
    field_origins: Option<std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>>>,
}
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

#[derive(Debug)]
pub(crate) struct AdmittedWordSource {
    work: (usize, usize),
    constructors: std::sync::Arc<ConstructorFailures>,
    projected_aliases: bool,
    source_map: std::sync::Arc<WordSourceMap>,
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

fn paragraph_constructor_kind(local: &[u8], empty: bool) -> ConstructorKind {
    use ConstructorKind as K;
    match local {
        b"r" => K::Run,
        b"hyperlink" => K::Hyperlink,
        b"fldSimple" => K::SimpleFieldUnknown,
        b"ruby" if !empty => K::Ruby,
        b"ins" | b"del" | b"moveFrom" | b"moveTo" => K::Revision(false),
        b"pPr" if !empty => K::ParagraphProperties,
        b"sdt" if !empty => K::Control(SdtOwner::Inline),
        _ => K::Opaque,
    }
}

fn constructor_child_kind(
    mut parent: Option<&mut ConstructorFrame<'_>>,
    local: &[u8],
    word: bool,
    empty: bool,
    root_owner: Option<SdtOwner>,
) -> ConstructorKind {
    use ConstructorKind as K;
    if let Some(parent) = parent.as_deref_mut() {
        match parent.kind {
            K::ParagraphCollection => {
                return if word && local == b"p" {
                    K::Paragraph
                } else {
                    K::Unconstructed
                };
            }
            K::Notes => {
                return if word && matches!(local, b"footnote" | b"endnote") {
                    K::ParagraphCollection
                } else {
                    K::Unconstructed
                };
            }
            K::Comments => {
                return if word && local == b"comment" {
                    K::ParagraphCollection
                } else {
                    K::Unconstructed
                };
            }
            _ => {}
        }
        if matches!(parent.kind, K::Revision(_)) {
            if parent.failed {
                return K::Opaque;
            }
            if parent.revision_stopped {
                return K::Unconstructed;
            }
            match parent.revision_mode {
                Some(crate::revision::RevisionBodyMode::Ordered) => {
                    // This actual factory calls CT_P with RevisionSourceFacts.
                    // Its physical lane captures direct simple fields raw.
                    // Later complex ownership/projection remains unproved here.
                    return if word && local == b"fldSimple" {
                        K::Unconstructed
                    } else if word {
                        paragraph_constructor_kind(local, empty)
                    } else {
                        K::Opaque
                    };
                }
                Some(crate::revision::RevisionBodyMode::Legacy(kind)) => {
                    use crate::revision::RevisionContentAction as A;
                    return match A::for_names(Some(kind), local, word, empty) {
                        A::Run => K::Run,
                        A::NestedRevision => K::Revision(true),
                        A::Hyperlink => K::RevisionHyperlink,
                        A::PriorRunProperties => {
                            parent.revision_stopped = true;
                            K::RunProperties
                        }
                        A::PriorParagraphProperties => {
                            parent.revision_stopped = true;
                            K::ParagraphProperties
                        }
                        A::PriorTableProperties => {
                            parent.revision_stopped = true;
                            K::TableProperties
                        }
                        A::PriorSectionProperties => {
                            parent.revision_stopped = true;
                            K::SectionProperties
                        }
                        A::Skip => K::Unconstructed,
                    };
                }
                None => {
                    // Deletion/moveFrom still require their actual whole-source
                    // ordered-owner decision. Only this direct-run action is
                    // identical in both existing readers, never whole success.
                    return if word && !empty && local == b"r" {
                        K::Run
                    } else {
                        K::Opaque
                    };
                }
            }
        }
        if matches!(parent.kind, K::RevisionHyperlink) {
            use crate::revision::RevisionContentAction as A;
            return match A::for_names(None, local, word, empty) {
                A::Run => K::Run,
                A::NestedRevision => K::Revision(true),
                A::Hyperlink => K::RevisionHyperlink,
                _ => K::Unconstructed,
            };
        }
        if parent.ruby_none_at.is_some() {
            return K::Opaque;
        }
        if matches!(parent.kind, K::Ruby) {
            if !parent.known || parent.failed {
                return K::Opaque;
            }
            return match crate::ruby::RubyChildAction::for_names(local, word, empty) {
                crate::ruby::RubyChildAction::Properties if !empty => K::RubyProperties,
                crate::ruby::RubyChildAction::Properties => K::Opaque,
                crate::ruby::RubyChildAction::Text | crate::ruby::RubyChildAction::Base => {
                    K::RubyContent
                }
                _ => {
                    // Raw children are captured now. Their namespace completion
                    // happens only after the root reader reaches success.
                    parent.content_seen = true;
                    K::Opaque
                }
            };
        }
        if matches!(parent.kind, K::RunText) {
            parent.content_seen = true;
            parent.known = false;
        }
        if matches!(parent.kind, K::RubyContent) {
            return if !parent.content_seen
                && crate::ruby::RubyContentAction::for_names(local, word)
                    == crate::ruby::RubyContentAction::Run
            {
                K::Run
            } else {
                parent.content_seen = true;
                parent.known = false;
                K::Opaque
            };
        }
    }
    if let Some(parent) = parent.as_deref() {
        if matches!(parent.kind, K::Drawing) {
            return crate::drawing::DrawingContainerKind::for_name(local, empty)
                .map_or(K::Opaque, K::DrawingContainer);
        }
        if matches!(
            parent.kind,
            K::DrawingContainer(_) | K::DrawingWalk(_) | K::DrawingConsumed
        ) {
            return K::Opaque;
        }
    }
    if !word {
        return K::Opaque;
    }
    let named = || match local {
        b"p" => K::Paragraph,
        b"r" => K::Run,
        b"tbl" => K::Table,
        b"tr" => K::Row,
        b"tc" => K::Cell,
        _ => K::Opaque,
    };
    let Some(parent) = parent else {
        return match local {
            b"document" => K::Document,
            b"footnotes" | b"endnotes" => K::Notes,
            b"comments" => K::Comments,
            b"glossaryDocument" => K::Glossary,
            b"hdr" | b"ftr" => K::ParagraphCollection,
            b"body" | b"comment" | b"footnote" | b"endnote" | b"docPartBody" | b"txbxContent" => {
                K::Body
            }
            b"sdt" => root_owner.map_or(K::Opaque, K::Control),
            _ => named(),
        };
    };
    match parent.kind {
        K::Document if local == b"body" => K::Body,
        K::Glossary if local == b"docParts" => K::DocParts,
        K::DocParts if local == b"docPart" => K::DocPart,
        K::DocPart if local == b"docPartBody" => K::Body,
        K::Body | K::Cell => match local {
            b"tcPr" if !empty && matches!(parent.kind, K::Cell) => K::CellProperties,
            b"p" | b"tbl" => named(),
            b"sdt" if !empty => K::Control(if matches!(parent.kind, K::Cell) {
                SdtOwner::Cell
            } else {
                SdtOwner::Body
            }),
            _ => K::Opaque,
        },
        K::Table => match local {
            b"tblPr" if !empty => K::TableProperties,
            b"tblGrid" if !empty => K::TableGrid,
            b"tr" => K::Row,
            b"sdt" if !empty => K::Control(SdtOwner::Table),
            _ => K::Opaque,
        },
        K::Row => match local {
            b"trPr" if !empty => K::RowProperties,
            b"tc" => K::Cell,
            b"sdt" if !empty => K::Control(SdtOwner::Row),
            _ => K::Opaque,
        },
        K::Paragraph => paragraph_constructor_kind(local, empty),
        K::Hyperlink | K::SimpleFieldResult if local == b"r" => K::Run,
        K::Hyperlink if matches!(local, b"ins" | b"del" | b"moveFrom" | b"moveTo") => {
            K::Revision(true)
        }
        K::RunProperties if matches!(local, b"ins" | b"del" | b"rPrChange") => K::Revision(true),
        K::ParagraphProperties if local == b"pPrChange" => K::Revision(true),
        K::ParagraphProperties if local == b"numPr" && !empty => K::NumberingProperties,
        K::NumberingProperties if local == b"ins" => K::Revision(true),
        K::SectionProperties if local == b"sectPrChange" => K::Revision(true),
        K::TableProperties if local == b"tblPrChange" => K::Revision(true),
        K::RowProperties if matches!(local, b"ins" | b"del" | b"moveFrom") => K::Revision(true),
        K::ControlContent(_)
            if matches!(
                local,
                b"ins"
                    | b"del"
                    | b"moveFrom"
                    | b"moveTo"
                    | b"rPrChange"
                    | b"pPrChange"
                    | b"tblPrChange"
                    | b"sectPrChange"
            ) =>
        {
            K::Revision(true)
        }
        K::TableProperties if local == b"tblBorders" && !empty => K::TableBorders,
        K::CellProperties if local == b"tcBorders" && !empty => K::TableBorders,
        K::TableProperties if local == b"tblCellMar" && !empty => K::TableMargins,
        K::CellProperties if local == b"tcMar" && !empty => K::TableMargins,
        K::Run if !empty && matches!(local, b"t" | b"delText") => K::RunText,
        K::Run if local == b"drawing" && !empty => K::Drawing,
        K::Run | K::ParagraphProperties if local == b"rPr" && !empty => K::RunProperties,
        K::ParagraphProperties if local == b"sectPr" && !empty => K::SectionProperties,
        K::ParagraphProperties if local == b"pBdr" && !empty => K::ParagraphBorders,
        K::ParagraphProperties if local == b"tabs" && !empty => K::Tabs,
        K::Control(_) if local == b"sdtPr" && !empty && !parent.properties_seen => {
            parent.properties_seen = true;
            K::ControlProperties
        }
        K::Control(owner) if local == b"sdtContent" && !empty && !parent.content_seen => {
            parent.content_seen = true;
            K::ControlContent(owner)
        }
        K::ControlContent(SdtOwner::Inline) if local == b"fldSimple" => K::SimpleFieldUnknown,
        K::ControlContent(owner) if local == b"sdt" && !empty => K::Control(owner),
        K::ControlContent(owner) if owner.owns(local) => named(),
        _ => K::Opaque,
    }
}

/// Replay only a completed child's physical opening through the same header
/// reader and scalar decoder. The outer result retains reader/context errors,
/// while the inner result is the actual property constructor outcome.
fn ruby_property_after_capture(
    (input, length): (WordXmlInput<'_>, usize),
    namespace_state: &CaptureNamespaceState,
    pool: &CaptureDeclarations,
) -> Result<Result<crate::ruby::RubyPropertyChild>> {
    let mut reader = Reader::from_reader(std::io::Read::take(input, length as u64));
    let mut buffer = Vec::new();
    match reader.read_event_into(&mut buffer)? {
        Event::Start(start) => {
            let aliases = constructor_attribute_prefixes(&start, namespace_state, pool)?;
            Ok(crate::ruby::RubyPropertyChild::read(&start, &aliases))
        }
        _ => Err(OxmlError::InvalidValue(
            "Missing captured Ruby property start".into(),
        )),
    }
}

/// Negative facts only. Unknown constructor families do not become valid here.
/// Each source event is visited once, with namespace facts pooled once and no
/// recursively owned model or expanded capture record constructed.
fn qualify_constructor_failures(
    xml: WordXmlInput<'_>,
    inherited: &[u32],
    pool: &mut CaptureDeclarations,
    root_owner: Option<SdtOwner>,
    word_aliases: &[String],
    consumed_owner: Option<&[u8]>,
    source_reader: Option<WordSourceReader>,
) -> WordSourceResult<ConstructorFailures> {
    let part_reader = match source_reader {
        Some(WordSourceReader::Part(reader)) => Some(reader),
        _ => None,
    };
    let bare: std::collections::HashSet<&str> = word_aliases
        .iter()
        .filter(|alias| !alias.starts_with('\0'))
        .map(String::as_str)
        .collect();
    let mut first_bindings = std::collections::HashMap::new();
    let primitive_context_stable = word_aliases
        .iter()
        .filter_map(|alias| alias.strip_prefix('\0')?.split_once('\0'))
        .all(|(prefix, uri)| {
            let same = first_bindings
                .insert(prefix, uri)
                .is_none_or(|old| old == uri);
            same && (uri != W_NS || bare.contains(prefix))
        });
    let namespace_state = &mut CaptureNamespaceState::new(inherited, pool, word_aliases);
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut scopes = Vec::<OriginalCaptureScope>::new();
    let mut frames = Vec::<ConstructorFrame<'_>>::new();
    if let Some(owner) = consumed_owner {
        frames.push(ConstructorFrame {
            kind: if owner == b"p" {
                ConstructorKind::Paragraph
            } else {
                ConstructorKind::Run
            },
            start: 0,
            failed: false,
            content_seen: false,
            properties_seen: false,
            control_occurrence: None,
            root_attribute_failed: false,
            known: false,
            ruby_none_at: None,
            ruby_property_opening: None,
            revision_mode: None,
            revision_stopped: false,
            lexical_revision: false,
            revision_height: 0,
            drawing_stopped: false,
            drawing_has_id: false,
            drawing_metadata_opening: None,
            drawing_owner: None,
            source_depth: 0,
            drawing_prefix_unknown: false,
        });
    }
    let mut failures = ConstructorFailures {
        raw_subtrees: Vec::new(),
        root_raw: false,
        controls: Vec::new(),
        discarded: Vec::new(),
        simple_headers: Vec::new(),
        part_events: Vec::new(),
        paragraph_result_root: None,
        ordered_revision_root: None,
        selector_paragraph_root: None,
        simple_selectors: Vec::new(),
    };
    let collect_selectors = matches!(source_reader, Some(WordSourceReader::StandaloneParagraph))
        && word_aliases.is_empty()
        && inherited.is_empty()
        && consumed_owner.is_none()
        && root_owner.is_none();
    let mut selector: Option<DirectSimpleSelectorFold> = None;
    // The actual detached Field reader starts with this decoder. An outer
    // declaration or a later declaration cannot silently supply another role.
    let selector_decoder = Reader::from_reader(&b""[..]).decoder();
    let mut buffer = Vec::new();
    loop {
        let start_offset = reader.buffer_position() as usize;
        let opening_input = *reader.get_ref();
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?;
        if let Some(active) = &mut selector {
            active.reusable &= reader.decoder() == selector_decoder;
        }
        match event {
            Event::Start(ref start) | Event::Empty(ref start) => {
                let empty = matches!(event, Event::Empty(_));
                let mut declarations = Vec::new();
                let mut literal_namespaces = Vec::new();
                let mut raw_namespaces = start.attributes().filter_map(|attribute| {
                    let attribute = attribute.ok()?;
                    let name = attribute.key.as_ref();
                    (name == b"xmlns" || name.starts_with(b"xmlns:")).then_some(attribute.value)
                });
                for (prefix, namespace) in crate::text::raw_namespace_declarations(start)? {
                    let raw = raw_namespaces
                        .next()
                        .expect("same validated namespace attributes");
                    literal_namespaces.push(raw.as_ref() == namespace.as_bytes());
                    declarations.push(
                        pool.push(prefix, namespace)
                            .map_err(WordSourceFailure::Resource)?,
                    );
                }
                let global_reader = part_reader.filter(|_| {
                    frames
                        .last()
                        .is_none_or(|frame| matches!(frame.kind, ConstructorKind::PartWrapper))
                });
                namespace_state.enter(&declarations, scopes.len(), pool, Some(&literal_namespaces));
                let local = start.local_name();
                let word =
                    original_capture_word_name(start.name().as_ref(), false, namespace_state, pool);
                if collect_selectors {
                    if let Some(active) = &mut selector {
                        active.element(
                            local.as_ref(),
                            word,
                            empty,
                            scopes.len() + 1,
                            reader.buffer_position() as usize,
                        );
                    } else if !empty && scopes.is_empty() && word && local.as_ref() == b"p" {
                        failures.selector_paragraph_root =
                            Some(start_offset..reader.buffer_position() as usize);
                    } else if !empty
                        && scopes.len() == 1
                        && failures.selector_paragraph_root.is_some()
                        && word
                        && local.as_ref() == b"fldSimple"
                    {
                        selector = Some(DirectSimpleSelectorFold {
                            opening: start_offset..reader.buffer_position() as usize,
                            result_start: reader.buffer_position() as usize,
                            ordered: false,
                            result_seen: false,
                            traversed: 1,
                            consumed: None,
                            reusable: reader.decoder() == selector_decoder,
                        });
                    }
                }
                let part_action =
                    global_reader.map(|mode| mode.action(local.as_ref(), word, empty));
                if let Some(kind) = part_action {
                    failures.part_events.push(PartReaderEvent {
                        start: start_offset,
                        kind,
                        updates: if matches!(kind, ConstructorKind::PartWrapper) {
                            declarations.clone()
                        } else {
                            Vec::new()
                        },
                    });
                    if matches!(kind, ConstructorKind::PartWrapper) {
                        namespace_state.retain_part_declarations(&declarations, pool);
                        declarations.clear();
                    }
                }
                let word_name = |name: &[u8], attribute: bool| {
                    original_capture_word_name(name, attribute, namespace_state, pool)
                };
                let drawing_owner = frames.last().and_then(|parent| {
                    if matches!(parent.kind, ConstructorKind::DrawingContainer(_)) {
                        Some(frames.len() - 1)
                    } else {
                        parent.drawing_owner
                    }
                });
                let physical_since =
                    drawing_owner.map_or(usize::MAX, |index| frames[index].source_depth);
                let doc_pr = constructor_namespace_name(
                    start.name().as_ref(),
                    b"docPr",
                    crate::drawing::drawing_ns::WP,
                    namespace_state,
                    pool,
                    physical_since,
                );
                // The actual independent metadata postscan sees the complete
                // captured container, including consumed or late payloads.
                if primitive_context_stable && doc_pr {
                    let mut owner = drawing_owner;
                    while let Some(index) = owner {
                        if frames[index].drawing_metadata_opening.is_some() {
                            break;
                        }
                        frames[index].drawing_metadata_opening = Some((
                            opening_input,
                            reader.buffer_position() as usize - start_offset,
                        ));
                        // Each ancestor is initialized once. A previously seen
                        // inner owner already initialized its entire chain.
                        owner = frames[index].drawing_owner;
                    }
                }
                let drawing_kind = frames.last().and_then(|parent| match parent.kind {
                    ConstructorKind::DrawingContainer(kind)
                    | ConstructorKind::DrawingWalk(kind) => Some(kind),
                    _ => None,
                });
                let mut drawing_child = None;
                let decorative_parent = frames.last().and_then(|parent| match parent.kind {
                    ConstructorKind::DrawingDocPr => Some((0, true)),
                    ConstructorKind::DrawingDecorative(depth, admitted) => {
                        Some((depth + 1, admitted))
                    }
                    _ => None,
                });
                if primitive_context_stable && let Some((depth, admitted)) = decorative_parent {
                    if frames.last().is_some_and(|parent| parent.failed) {
                        drawing_child = Some(ConstructorKind::Unconstructed);
                    } else {
                        let outcome = crate::drawing::DecorativeChildPlan::read(
                            start,
                            depth,
                            admitted,
                            constructor_namespace_name(
                                start.name().as_ref(),
                                start.local_name().as_ref(),
                                crate::drawing::drawing_ns::A,
                                namespace_state,
                                pool,
                                physical_since,
                            ),
                            constructor_namespace_name(
                                start.name().as_ref(),
                                start.local_name().as_ref(),
                                crate::drawing::DECORATIVE_NAMESPACE,
                                namespace_state,
                                pool,
                                physical_since,
                            ),
                            start.decoder(),
                        );
                        drawing_child = Some(match outcome {
                            Ok(plan) if !empty => {
                                ConstructorKind::DrawingDecorative(depth, plan.admitted)
                            }
                            Ok(_) => ConstructorKind::Opaque,
                            Err(_) => {
                                frames.last_mut().expect("decorative owner").failed = true;
                                ConstructorKind::Unconstructed
                            }
                        });
                    }
                }
                if primitive_context_stable && let Some(kind) = drawing_kind {
                    use crate::drawing::DrawingChildAction as A;
                    let action = crate::drawing::DrawingChildAction::for_name(
                        kind,
                        start.name().as_ref(),
                        empty,
                        doc_pr,
                        constructor_namespace_name(
                            start.name().as_ref(),
                            b"txbxContent",
                            W_NS,
                            namespace_state,
                            pool,
                            physical_since,
                        ),
                    );
                    let parent = frames.last_mut().expect("drawing lane");
                    if parent.drawing_stopped || parent.failed {
                        drawing_child = Some(ConstructorKind::Unconstructed);
                    } else {
                        if action == A::TextBox {
                            parent.drawing_prefix_unknown = true;
                        }
                        if !parent.drawing_prefix_unknown {
                            match action {
                                A::Extent => {
                                    parent.failed |= crate::drawing::read_extent(
                                        start,
                                        &mut crate::units::Emu(0),
                                        &mut crate::units::Emu(0),
                                    )
                                    .is_err()
                                }
                                A::SourceRect => {
                                    parent.failed |=
                                        crate::drawing::parse_source_rect(start).is_err()
                                }
                                A::PositionH | A::PositionV => {
                                    parent.failed |=
                                        crate::drawing::DrawingPositionReference::read(
                                            start,
                                            action == A::PositionH,
                                        )
                                        .is_err();
                                }
                                A::DocPr => {
                                    for attribute in start.attributes() {
                                        let outcome = attribute.map_err(OxmlError::from).and_then(
                                            |attribute| {
                                                Ok(matches!(
                                                    crate::drawing::DrawingDocPrAttribute::read(
                                                        &attribute,
                                                        start.decoder()
                                                    )?,
                                                    crate::drawing::DrawingDocPrAttribute::Id(_)
                                                ))
                                            },
                                        );
                                        match outcome {
                                            Ok(id) => parent.drawing_has_id |= id,
                                            Err(_) => {
                                                parent.failed = true;
                                                break;
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        drawing_child = Some(if parent.failed {
                            ConstructorKind::Unconstructed
                        } else if empty {
                            ConstructorKind::Opaque
                        } else {
                            match action {
                                A::Walk => ConstructorKind::DrawingWalk(kind),
                                A::DocPr if !parent.drawing_prefix_unknown => {
                                    ConstructorKind::DrawingDocPr
                                }
                                _ => ConstructorKind::DrawingConsumed,
                            }
                        });
                    }
                }
                if let Some(frame) = frames.last_mut()
                    && matches!(frame.kind, ConstructorKind::RubyContent)
                    && frame.known
                    && !frame.failed
                    && crate::ruby::RubyContentAction::for_names(local.as_ref(), word)
                        == crate::ruby::RubyContentAction::Reject
                {
                    frame.ruby_none_at = Some(start_offset);
                }
                if frames.last().is_some_and(|frame| {
                    matches!(frame.kind, ConstructorKind::Ruby) && frame.known && !frame.failed
                }) && word
                    && empty
                    && matches!(local.as_ref(), b"rt" | b"rubyBase")
                {
                    frames.last_mut().expect("ruby").ruby_none_at = Some(start_offset);
                }
                if frames
                    .last()
                    .is_some_and(|frame| matches!(frame.kind, ConstructorKind::Run))
                {
                    let action =
                        crate::text::RunChildAction::for_names(local.as_ref(), empty, word, false);
                    let aliases = constructor_attribute_prefixes(start, namespace_state, pool)?;
                    let parent = frames.last_mut().expect("run parent");
                    match crate::text::RunChildPlan::read(start, &aliases, action) {
                        Err(_) => parent.failed = true,
                        Ok(plan) => {
                            let simple = plan.leaf.is_some()
                                || matches!(
                                    action,
                                    crate::text::RunChildAction::IgnoreEmptyProperties
                                        | crate::text::RunChildAction::Properties
                                        | crate::text::RunChildAction::Text
                                        | crate::text::RunChildAction::DeletedText
                                );
                            let comment_raw = action
                                == crate::text::RunChildAction::CommentReference
                                && (crate::text::comment_reference_has_payload_attributes(
                                    start, &aliases,
                                ) || !empty);
                            parent.known &= simple && !comment_raw;
                        }
                    }
                }
                let mut ruby_property_opening = None;
                if frames.last().is_some_and(|frame| {
                    matches!(frame.kind, ConstructorKind::RubyProperties)
                        && frame.known
                        && frame.ruby_none_at.is_none()
                        && !frame.failed
                }) {
                    // Started children are captured before their scalar is read.
                    // Empty raw capture writes this exact event, but its namespace
                    // completion is deferred until the property reader succeeds.
                    if empty {
                        let aliases = constructor_attribute_prefixes(start, namespace_state, pool)?;
                        let outcome = crate::ruby::RubyPropertyChild::read(start, &aliases);
                        let parent = frames.last_mut().expect("ruby property owner");
                        parent.record_ruby_property(outcome);
                    } else {
                        ruby_property_opening = Some((
                            opening_input,
                            reader.buffer_position() as usize - start_offset,
                        ));
                    }
                }
                if frames
                    .last()
                    .is_some_and(|frame| matches!(frame.kind, ConstructorKind::RunProperties))
                {
                    let aliases = constructor_attribute_prefixes(start, namespace_state, pool)?;
                    let bold = if empty {
                        crate::run_properties::empty_bold_value(start, &aliases)
                    } else {
                        Ok(None)
                    };
                    let parent = frames.last_mut().expect("property owner");
                    match bold {
                        Ok(Some(_)) => {}
                        Ok(None) => parent.known = false,
                        Err(_) => {
                            parent.failed = true;
                            parent.known = false;
                        }
                    }
                }
                if empty
                    && word
                    && frames
                        .last()
                        .is_some_and(|frame| matches!(frame.kind, ConstructorKind::RunProperties))
                    && let Some(measurement) =
                        crate::run_properties::RunMeasurement::for_local(local.as_ref())
                {
                    let mut value = None;
                    for attribute in start.attributes() {
                        let attribute = attribute.map_err(OxmlError::from)?;
                        if matches_local_name(attribute.key.as_ref(), b"val")
                            && word_name(attribute.key.as_ref(), true)
                        {
                            value = Some(
                                std::str::from_utf8(&attribute.value)
                                    .map_err(OxmlError::from)?
                                    .to_owned(),
                            );
                            break;
                        }
                    }
                    if measurement
                        .decode(start.name().as_ref(), value.as_deref())
                        .is_err()
                    {
                        frames.last_mut().expect("property frame exists").failed = true;
                    }
                }
                if empty
                    && word
                    && frames.last().is_some_and(|frame| {
                        matches!(frame.kind, ConstructorKind::ParagraphProperties)
                    })
                {
                    let all_values = matches!(local.as_ref(), b"spacing" | b"ind" | b"framePr");
                    let first_value = matches!(local.as_ref(), b"divId" | b"outlineLvl");
                    if all_values || first_value {
                        let mut failed = false;
                        for attribute in start.attributes() {
                            let attribute = attribute.map_err(OxmlError::from)?;
                            let key = attribute.key.as_ref();
                            let qualified = word_name(key, true);
                            if !all_values && !qualified {
                                continue;
                            }
                            let attribute_local = attribute.key.local_name();
                            let measurement = qualified
                                .then(|| {
                                    crate::paragraph_properties::ParagraphMeasurement::for_names(
                                        local.as_ref(),
                                        attribute_local.as_ref(),
                                    )
                                })
                                .flatten();
                            if !all_values && measurement.is_none() {
                                continue;
                            }
                            // Spacing/indent/frame construction decodes every attribute's
                            // UTF-8, while val-only leaves stop at their first value.
                            let Ok(value) = std::str::from_utf8(&attribute.value) else {
                                failed = true;
                                break;
                            };
                            if let Some(measurement) = measurement {
                                failed = measurement
                                    .decode(start.name().as_ref(), key, Some(value))
                                    .is_err();
                                if failed || first_value {
                                    break;
                                }
                            }
                        }
                        if failed {
                            frames.last_mut().expect("property frame exists").failed = true;
                        }
                    }
                }
                if word && let Some(parent) = frames.last() {
                    let compound = empty
                        && matches!(parent.kind, ConstructorKind::RunProperties)
                        && matches!(local.as_ref(), b"fitText" | b"eastAsianLayout");
                    let border = (empty
                        && matches!(parent.kind, ConstructorKind::RunProperties)
                        && local.as_ref() == b"bdr")
                        || (matches!(parent.kind, ConstructorKind::ParagraphBorders)
                            && matches!(
                                local.as_ref(),
                                b"top"
                                    | b"bottom"
                                    | b"left"
                                    | b"start"
                                    | b"right"
                                    | b"end"
                                    | b"between"
                                    | b"bar"
                            ));
                    let border = border
                        || (empty
                            && matches!(parent.kind, ConstructorKind::TableBorders)
                            && matches!(
                                local.as_ref(),
                                b"top"
                                    | b"bottom"
                                    | b"left"
                                    | b"start"
                                    | b"right"
                                    | b"end"
                                    | b"insideH"
                                    | b"insideV"
                            ));
                    let tab =
                        matches!(parent.kind, ConstructorKind::Tabs) && local.as_ref() == b"tab";
                    if compound || border || tab {
                        let mut failed = false;
                        for attribute in start.attributes() {
                            let attribute = attribute.map_err(OxmlError::from)?;
                            let Ok(value) = std::str::from_utf8(&attribute.value) else {
                                failed = true;
                                break;
                            };
                            if !word_name(attribute.key.as_ref(), true) {
                                continue;
                            }
                            let attribute_local = attribute.key.local_name();
                            if compound
                                && let Some(measurement) =
                                    crate::run_properties::RunCompoundMeasurement::for_names(
                                        local.as_ref(),
                                        attribute_local.as_ref(),
                                    )
                            {
                                failed = measurement
                                    .decode(start.name().as_ref(), attribute.key.as_ref(), value)
                                    .is_err();
                            } else if let Some(measurement) = if border {
                                crate::borders::BorderMeasurement::border_attribute(
                                    attribute_local.as_ref(),
                                )
                            } else if tab {
                                crate::borders::BorderMeasurement::tab_attribute(
                                    attribute_local.as_ref(),
                                )
                            } else {
                                None
                            } {
                                failed = measurement
                                    .decode(start.name().as_ref(), attribute.key.as_ref(), value)
                                    .is_err();
                            }
                            if failed {
                                break;
                            }
                        }
                        if failed {
                            frames.last_mut().expect("property parent exists").failed = true;
                        }
                    }
                }
                if word && let Some(parent) = frames.last() {
                    use ConstructorKind as K;
                    let table = match parent.kind {
                        K::TableProperties if empty => matches!(
                            local.as_ref(),
                            b"tblW"
                                | b"tblInd"
                                | b"tblCellSpacing"
                                | b"tblpPr"
                                | b"tblStyleRowBandSize"
                                | b"tblStyleColBandSize"
                        ),
                        K::RowProperties if empty => matches!(
                            local.as_ref(),
                            b"trHeight"
                                | b"gridBefore"
                                | b"gridAfter"
                                | b"wBefore"
                                | b"wAfter"
                                | b"tblCellSpacing"
                        ),
                        K::CellProperties => {
                            matches!(local.as_ref(), b"tcW" | b"gridSpan" | b"vMerge")
                        }
                        K::TableGrid => local.as_ref() == b"gridCol",
                        K::TableMargins if empty => matches!(
                            local.as_ref(),
                            b"top" | b"bottom" | b"left" | b"start" | b"right" | b"end"
                        ),
                        _ => false,
                    };
                    let section = empty
                        && matches!(parent.kind, K::SectionProperties)
                        && matches!(local.as_ref(), b"pgSz" | b"pgMar" | b"cols");
                    let control = matches!(parent.kind, K::ControlProperties)
                        .then(|| crate::content_control::ControlScalar::for_local(local.as_ref()))
                        .flatten();
                    let mut failed = false;
                    if let Some(scalar) = control {
                        let mut value = None;
                        for attribute in start.attributes() {
                            let attribute = attribute.map_err(OxmlError::from)?;
                            if matches_local_name(attribute.key.as_ref(), b"val")
                                && word_name(attribute.key.as_ref(), true)
                            {
                                value = Some(
                                    attribute
                                        .decoded_and_normalized_value(
                                            XmlVersion::Implicit1_0,
                                            start.decoder(),
                                        )
                                        .map_err(OxmlError::from)?
                                        .into_owned(),
                                );
                                break;
                            }
                        }
                        failed = scalar.decode(value).is_err();
                    } else if table || section {
                        let first_value = matches!(
                            local.as_ref(),
                            b"tblStyleRowBandSize"
                                | b"tblStyleColBandSize"
                                | b"gridBefore"
                                | b"gridAfter"
                                | b"gridSpan"
                                | b"vMerge"
                        );
                        for attribute in start.attributes() {
                            let attribute = attribute.map_err(OxmlError::from)?;
                            let key = attribute.key.as_ref();
                            let qualified = word_name(key, true);
                            let attr_local = attribute.key.local_name();
                            let margin = matches!(parent.kind, K::TableMargins);
                            let table_scalar = if margin && qualified && attr_local.as_ref() == b"w"
                            {
                                Some(crate::table::TableScalar::Measurement)
                            } else {
                                (table && qualified)
                                    .then(|| {
                                        crate::table::TableScalar::for_names(
                                            local.as_ref(),
                                            attr_local.as_ref(),
                                        )
                                    })
                                    .flatten()
                            };
                            let section_scalar = (section && qualified)
                                .then(|| {
                                    SectionScalar::for_names(local.as_ref(), attr_local.as_ref())
                                })
                                .flatten();
                            // Grid and margin readers decode only selected numeric attributes.
                            // Val-only readers stop at the first qualified val.
                            let selected_only = margin
                                || first_value
                                || matches!(local.as_ref(), b"gridCol" | b"pgMar");
                            if selected_only && table_scalar.is_none() && section_scalar.is_none() {
                                continue;
                            }
                            let Ok(value) = std::str::from_utf8(&attribute.value) else {
                                failed = true;
                                break;
                            };
                            if let Some(scalar) = table_scalar {
                                failed = scalar.decode(start.name().as_ref(), key, value).is_err();
                            } else if let Some(scalar) = section_scalar {
                                failed = scalar.decode(start.name().as_ref(), key, value).is_err();
                            }
                            if failed || first_value || margin {
                                break;
                            }
                        }
                    }
                    if failed {
                        frames.last_mut().expect("scalar owner exists").failed = true;
                    }
                }
                let root_revision = match source_reader {
                    Some(WordSourceReader::OrderedRevision { require_author })
                        if frames.is_empty()
                            && word
                            && crate::revision::revision_kind_for_local(local.as_ref())
                                .is_some() =>
                    {
                        Some(ConstructorKind::Revision(require_author))
                    }
                    _ => None,
                };
                let paragraph_result_root = matches!(source_reader, Some(WordSourceReader::SimpleParagraph { opening_end, .. })
                    if primitive_context_stable && frames.is_empty() && word && local.as_ref() == b"fldSimple" && reader.buffer_position() as usize == opening_end);
                if paragraph_result_root {
                    failures.paragraph_result_root = Some(start_offset);
                }
                let excluded_result_child = matches!(source_reader, Some(WordSourceReader::SimpleParagraph { result_start, result_end, .. })
                    if frames.len() == 1 && failures.paragraph_result_root == frames.first().map(|frame| frame.start)
                        && !(result_start..result_end).contains(&start_offset));
                let failed_result_child = frames.last().is_some_and(|parent| {
                    parent.failed && failures.paragraph_result_root == Some(parent.start)
                });
                let mut kind = (excluded_result_child || failed_result_child)
                    .then_some(ConstructorKind::Unconstructed)
                    .or(drawing_child)
                    .or(root_revision)
                    .or(paragraph_result_root.then_some(ConstructorKind::Paragraph))
                    .or(part_action)
                    .unwrap_or_else(|| {
                        constructor_child_kind(
                            frames.last_mut(),
                            local.as_ref(),
                            word,
                            empty,
                            root_owner,
                        )
                    });
                let lexical_revision =
                    word && crate::revision::revision_kind_for_local(local.as_ref()).is_some();
                let mut revision_mode = None;
                let failed = if matches!(kind, ConstructorKind::SimpleFieldUnknown) {
                    let aliases = constructor_attribute_prefixes(start, namespace_state, pool)?;
                    let header = crate::text::SimpleFieldHeader::read(start, &aliases, empty);
                    if simple_field_header_projection_stable(start, namespace_state, pool)? {
                        let raw = header.is_raw();
                        if !empty && primitive_context_stable {
                            let opening = start_offset..reader.buffer_position() as usize;
                            failures.simple_headers.push(SimpleHeaderOccurrence {
                                original_part: 0,
                                original_opening: opening.clone(),
                                opening,
                                header,
                            });
                        }
                        if raw {
                            kind = ConstructorKind::SimpleFieldRaw;
                            true
                        } else {
                            kind = ConstructorKind::SimpleFieldResult;
                            false
                        }
                    } else {
                        false
                    }
                } else if let ConstructorKind::Revision(require_author) = kind {
                    let prefixes = constructor_attribute_prefixes(start, namespace_state, pool)?;
                    match crate::revision::RevisionHeader::read(start, &prefixes, require_author) {
                        Ok(header) => {
                            revision_mode = if root_revision.is_some() && primitive_context_stable {
                                failures.ordered_revision_root = Some(start_offset);
                                Some(crate::revision::RevisionBodyMode::Ordered)
                            } else {
                                crate::revision::RevisionBodyMode::without_source_probe(header.kind)
                            };
                            false
                        }
                        Err(_) => true,
                    }
                } else if primitive_context_stable
                    && matches!(
                        kind,
                        ConstructorKind::DrawingContainer(
                            crate::drawing::DrawingContainerKind::Anchor
                        )
                    )
                {
                    crate::drawing::AnchorAttributes::read(start).is_err()
                } else {
                    false
                };
                if empty
                    && primitive_context_stable
                    && matches!(kind, ConstructorKind::Run)
                    && root_attribute_plan_failed(start, namespace_state, pool)?
                    && let Some(parent) = frames.last_mut()
                {
                    parent.failed = true;
                }
                if empty && matches!(kind, ConstructorKind::Unconstructed) {
                    failures
                        .discarded
                        .push(start_offset..reader.buffer_position() as usize);
                }
                if empty && failed {
                    failures.raw_subtrees.push(RawConstructorSpan {
                        span: start_offset..reader.buffer_position() as usize,
                        control: false,
                    });
                }
                if empty && let Some(parent) = frames.last_mut() {
                    parent.revision_height =
                        parent.revision_height.max(usize::from(lexical_revision));
                }
                if !empty {
                    let control_occurrence = if let ConstructorKind::Control(owner) = kind {
                        let index = failures.controls.len();
                        failures.controls.push(ControlOccurrence {
                            original_part: 0,
                            owner,
                            source_span: start_offset..0,
                            after: 0,
                            raw: false,
                        });
                        Some(index)
                    } else {
                        None
                    };
                    let root_attribute_failed = primitive_context_stable
                        && matches!(kind, ConstructorKind::Run)
                        && root_attribute_plan_failed(start, namespace_state, pool)?;
                    frames.push(ConstructorFrame {
                        kind,
                        start: start_offset,
                        failed,
                        content_seen: false,
                        properties_seen: false,
                        control_occurrence,
                        root_attribute_failed,
                        revision_mode,
                        revision_stopped: false,
                        lexical_revision,
                        revision_height: 0,
                        drawing_stopped: false,
                        drawing_has_id: false,
                        drawing_metadata_opening: None,
                        drawing_owner,
                        source_depth: scopes.len(),
                        drawing_prefix_unknown: frames.last().is_some_and(|parent| {
                            matches!(
                                parent.kind,
                                ConstructorKind::DrawingContainer(_)
                                    | ConstructorKind::DrawingWalk(_)
                            ) && parent.drawing_prefix_unknown
                        }),
                        known: primitive_context_stable
                            && matches!(
                                kind,
                                ConstructorKind::Run
                                    | ConstructorKind::RunText
                                    | ConstructorKind::RunProperties
                                    | ConstructorKind::Ruby
                                    | ConstructorKind::RubyContent
                                    | ConstructorKind::RubyProperties
                            ),
                        ruby_property_opening,
                        ruby_none_at: if matches!(
                            kind,
                            ConstructorKind::Ruby | ConstructorKind::RubyContent
                        ) && crate::ruby::has_foreign_attributes(start)?
                        {
                            Some(start_offset)
                        } else {
                            None
                        },
                    });
                    scopes.push(OriginalCaptureScope {
                        declarations,
                        raw: false,
                        typed_drawing: false,
                        inline_drawing: false,
                        text_span: false,
                        drawing_root: false,
                        dropped: false,
                        raw_copy: false,
                        constructs_runs: false,
                        constructs_hyperlinks: false,
                        paragraph: false,
                        ruby_opaque: false,
                        unconstructed: false,
                        comment_root: false,
                    });
                } else {
                    namespace_state.leave(&declarations, pool);
                }
            }
            Event::End(ref end) => {
                if let Some(active) = &mut selector
                    && active.end(scopes.len(), reader.buffer_position() as usize)
                {
                    let active = selector.take().expect("completed selector owner");
                    let owner_start = active.opening.start;
                    if active.reusable {
                        failures.simple_selectors.push(SimpleSelectorOccurrence {
                            opening: active.opening,
                            whole: owner_start..reader.buffer_position() as usize,
                            ordered: active.ordered,
                            result: active.result_start - owner_start..start_offset - owner_start,
                        });
                    }
                }
                if let Some(opening) = frames.last().and_then(|frame| frame.ruby_property_opening) {
                    // Local declarations are still active here. No nested child
                    // scope survives this successful matching capture End.
                    let outcome = ruby_property_after_capture(opening, namespace_state, pool)?;
                    let parent_index = frames.len() - 2;
                    let parent = &mut frames[parent_index];
                    parent.record_ruby_property(outcome);
                }
                if let Some(scope) = scopes.pop() {
                    namespace_state.leave(&scope.declarations, pool);
                }
                if let Some(mut frame) = frames.pop() {
                    if let ConstructorKind::DrawingContainer(kind)
                    | ConstructorKind::DrawingWalk(kind) = frame.kind
                    {
                        frame.drawing_stopped |= kind.matches_end(end.name().as_ref());
                    }
                    if matches!(frame.kind, ConstructorKind::DrawingContainer(_))
                        && !frame.drawing_prefix_unknown
                    {
                        frame.failed |= !frame.drawing_has_id;
                        if !frame.failed {
                            frame.failed |= frame.drawing_metadata_opening.is_none_or(|opening| {
                                drawing_metadata_after_capture(opening).is_err()
                            });
                        }
                    }
                    let revision_height = crate::revision::revision_depth_height(
                        frame.lexical_revision,
                        frame.revision_height,
                    );
                    // The actual revision reader checks this entire lexical
                    // nesting before even its header/body. Other opaque frames
                    // carry height only, not typed constructor authority.
                    if primitive_context_stable
                        && matches!(frame.kind, ConstructorKind::Revision(_))
                        && crate::revision::revision_depth_exceeds(revision_height)
                    {
                        frame.failed = true;
                    }
                    if matches!(frame.kind, ConstructorKind::Unconstructed) {
                        failures
                            .discarded
                            .push(frame.start..reader.buffer_position() as usize);
                    }
                    if matches!(
                        frame.kind,
                        ConstructorKind::RubyProperties | ConstructorKind::Ruby
                    ) && frame.content_seen
                        && frame.ruby_none_at.is_none()
                    {
                        // Successful raw-property completion is a separate action.
                        // A direct None never reaches it in the actual reader.
                        frame.known = false;
                    }
                    frame.failed |= frame.root_attribute_failed;
                    if matches!(frame.kind, ConstructorKind::Ruby)
                        && !frame.failed
                        && let Some(start) = frame.ruby_none_at
                    {
                        failures
                            .discarded
                            .push(start..reader.buffer_position() as usize);
                    }
                    if let Some(parent) = frames.last_mut() {
                        parent.revision_height = parent.revision_height.max(revision_height);
                        if matches!(
                            parent.kind,
                            ConstructorKind::DrawingContainer(_) | ConstructorKind::DrawingWalk(_)
                        ) && matches!(frame.kind, ConstructorKind::DrawingWalk(_))
                        {
                            parent.drawing_stopped |= frame.drawing_stopped;
                            parent.drawing_has_id |= frame.drawing_has_id;
                            parent.drawing_prefix_unknown |= frame.drawing_prefix_unknown;
                        }
                        if matches!(
                            parent.kind,
                            ConstructorKind::RubyContent
                                | ConstructorKind::Ruby
                                | ConstructorKind::Run
                        ) && !(matches!(parent.kind, ConstructorKind::Ruby)
                            && matches!(frame.kind, ConstructorKind::Opaque))
                        {
                            parent.known &= frame.known && !frame.failed;
                        }
                        if matches!(
                            frame.kind,
                            ConstructorKind::RubyProperties | ConstructorKind::RubyContent
                        ) && !frame.failed
                            && let Some(start) = frame.ruby_none_at
                        {
                            parent.ruby_none_at = Some(start);
                        }
                    }
                    if let Some(index) = frame.control_occurrence {
                        let after = failures.controls.len();
                        let occurrence = &mut failures.controls[index];
                        occurrence.source_span.end = reader.buffer_position() as usize;
                        occurrence.after = after;
                        occurrence.raw = frame.failed;
                    }
                    if frame.failed && matches!(frame.kind, ConstructorKind::Control(_)) {
                        failures.raw_subtrees.push(RawConstructorSpan {
                            span: frame.start..reader.buffer_position() as usize,
                            control: true,
                        });
                        failures.root_raw |= frames.is_empty();
                        // A failed nested control is retained raw by its parent.
                    } else if frame.failed
                        && matches!(
                            frame.kind,
                            ConstructorKind::Revision(_) | ConstructorKind::SimpleFieldRaw
                        )
                    {
                        failures.raw_subtrees.push(RawConstructorSpan {
                            span: frame.start..reader.buffer_position() as usize,
                            control: false,
                        });
                        // Revisions swallow constructor errors. Simple fields swallow
                        // only an early header None, not errors from parsed children.
                    } else if frame.failed
                        && let Some(parent) = frames.last_mut()
                    {
                        parent.failed = true;
                    }
                }
            }
            Event::Text(ref text)
                if frames
                    .last()
                    .is_some_and(|frame| matches!(frame.kind, ConstructorKind::RunText)) =>
            {
                let frame = frames.last_mut().expect("text span");
                frame.failed |= crate::text::decode_run_text_span(text).is_err();
                frame.known &= !frame.content_seen;
                frame.content_seen = true;
            }
            event
                if frames
                    .last()
                    .is_some_and(|frame| matches!(frame.kind, ConstructorKind::RunText))
                    && !matches!(event, Event::Eof) =>
            {
                frames.last_mut().expect("text span").known = false;
            }
            event if crate::ruby::rejects_direct_characters(&event) => {
                if let Some(frame) = frames.last_mut()
                    && matches!(
                        frame.kind,
                        ConstructorKind::Ruby
                            | ConstructorKind::RubyProperties
                            | ConstructorKind::RubyContent
                    )
                    && frame.known
                    && !frame.failed
                {
                    frame.ruby_none_at = Some(start_offset);
                }
            }
            Event::Comment(_) | Event::PI(_)
                if frames
                    .last()
                    .is_some_and(|frame| matches!(frame.kind, ConstructorKind::Run)) =>
            {
                frames.last_mut().expect("run").known = false;
            }
            Event::Eof => {
                // capture_element deliberately accepts EOF. Discharge the same
                // pending scalar there, restoring its opening scope if an
                // unfinished descendant introduced additional declarations.
                if let Some(index) = frames
                    .iter()
                    .rposition(|frame| frame.ruby_property_opening.is_some())
                {
                    let remaining_scopes = scopes.len() - (frames.len() - index - 1);
                    while scopes.len() > remaining_scopes {
                        let scope = scopes.pop().expect("nested capture scope");
                        namespace_state.leave(&scope.declarations, pool);
                    }
                    let opening = frames[index]
                        .ruby_property_opening
                        .expect("pending opening");
                    let outcome = ruby_property_after_capture(opening, namespace_state, pool)?;
                    frames[index - 1].record_ruby_property(outcome);
                }
                break;
            }
            _ => {}
        }
        buffer.clear();
    }
    failures
        .raw_subtrees
        .sort_unstable_by_key(|fact| fact.span.start);
    failures.discarded.sort_unstable_by_key(|span| span.start);
    let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
    for span in failures.discarded {
        if let Some(previous) = merged.last_mut()
            && span.start <= previous.end
        {
            previous.end = previous.end.max(span.end);
        } else {
            merged.push(span);
        }
    }
    failures.discarded = merged;
    Ok(failures)
}

pub(crate) fn control_has_property_measurement_failure_in(
    xml: &[u8],
    prefixes: &[String],
    owner: SdtOwner,
    registry: Option<&std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>>>,
) -> Result<bool> {
    let mut pool = CaptureDeclarations::default();
    let mut inherited = Vec::new();
    for (prefix, namespace) in reduce_inherited_bindings(prefixes) {
        inherited.push(pool.intern(prefix, namespace).map_err(|error| {
            record_word_source_failure(WordSourceFailure::Resource(error), registry)
        })?);
    }
    let verdict = (|| -> WordSourceResult<bool> {
        Ok(qualify_constructor_failures(
            WordXmlInput::Contiguous(xml),
            &inherited,
            &mut pool,
            Some(owner),
            prefixes,
            None,
            None,
        )?
        .root_raw)
    })();
    verdict.map_err(|failure| record_word_source_failure(failure, registry))
}

fn original_comment_reference_has_payload(input: WordXmlInput<'_>) -> Result<bool> {
    let mut reader = Reader::from_reader(input);
    reader.config_mut().allow_unmatched_ends = true;
    reader.config_mut().check_end_names = false;
    let mut buffer = Vec::new();
    Ok(!matches!(
        reader.read_event_into(&mut buffer)?,
        Event::End(_)
    ))
}

struct FragmentNamespaceClosure {
    insertion: usize,
    bindings: Vec<u32>,
    bytes: usize,
    original_w_binding: bool,
}

impl FragmentNamespaceClosure {
    fn read(
        xml: &[u8],
        scope: &std::collections::BTreeMap<String, String>,
        pool: &mut CaptureDeclarations,
    ) -> Result<Self> {
        let invalid = |message: String| OxmlError::InvalidValue(message);
        let mut reader = Reader::from_reader(xml);
        reader.config_mut().trim_text(false);
        let (opening_end, empty, existing) = loop {
            let event = reader
                .read_event()
                .map_err(|error| invalid(format!("content fragment scan failed: {error}")))?;
            let empty = matches!(event, Event::Empty(_));
            match event {
                Event::Start(element) | Event::Empty(element) => {
                    let mut existing = std::collections::HashSet::new();
                    for attribute in element.attributes() {
                        let attribute = attribute.map_err(|error| {
                            invalid(format!("content fragment attribute scan failed: {error}"))
                        })?;
                        let name = attribute.key.as_ref();
                        if name == b"xmlns" {
                            existing.insert(String::new());
                        } else if let Some(prefix) = name.strip_prefix(b"xmlns:") {
                            existing.insert(
                                std::str::from_utf8(prefix)
                                    .map_err(|error| {
                                        invalid(format!(
                                            "content fragment namespace prefix is invalid: {error}"
                                        ))
                                    })?
                                    .to_owned(),
                            );
                        }
                    }
                    break (xml.len() - reader.get_ref().len(), empty, existing);
                }
                Event::Eof => {
                    return Err(invalid("content fragment has no root element".into()));
                }
                _ => {}
            }
        };
        let mut bindings = Vec::new();
        let mut bytes = 0usize;
        for (prefix, namespace) in scope {
            if prefix == "xml" || existing.contains(prefix) {
                continue;
            }
            let index = pool.intern(prefix, namespace)?;
            bytes = bytes
                .checked_add(pool.declaration_bytes(index)?)
                .ok_or_else(|| invalid("content fragment namespace byte overflow".into()))?;
            bindings.push(index);
        }
        let insertion = opening_end
            .checked_sub(if empty { 2 } else { 1 })
            .ok_or_else(|| invalid("content fragment opening tag is incomplete".into()))?;
        Ok(Self {
            insertion,
            bindings,
            bytes,
            original_w_binding: scope.contains_key("w") || existing.contains("w"),
        })
    }

    fn write(&self, xml: &[u8], pool: &CaptureDeclarations) -> Result<Vec<u8>> {
        let length = xml
            .len()
            .checked_add(self.bytes)
            .ok_or_else(|| OxmlError::InvalidValue("content fragment byte overflow".into()))?;
        let mut closed = Vec::with_capacity(length);
        closed.extend_from_slice(&xml[..self.insertion]);
        self.write_declarations(&mut closed, pool);
        closed.extend_from_slice(&xml[self.insertion..]);
        debug_assert_eq!(closed.len(), length);
        Ok(closed)
    }

    fn write_declarations(&self, closed: &mut Vec<u8>, pool: &CaptureDeclarations) {
        for &index in &self.bindings {
            let prefix = pool.prefix(index);
            closed.extend_from_slice(b" xmlns");
            if !prefix.is_empty() {
                closed.push(b':');
                closed.extend_from_slice(prefix.as_bytes());
            }
            closed.extend_from_slice(b"=\"");
            closed.extend_from_slice(quick_xml::escape::escape(pool.namespace(index)).as_bytes());
            closed.push(b'"');
        }
    }
}

/// Close a retained fragment with every missing caller-supplied binding.
/// Local declarations, original bytes and the caller's binding order survive.
#[doc(hidden)]
pub fn close_content_fragment_namespaces(
    xml: &[u8],
    scope: &std::collections::BTreeMap<String, String>,
) -> Result<Vec<u8>> {
    let mut pool = CaptureDeclarations::default();
    FragmentNamespaceClosure::read(xml, scope, &mut pool)?.write(xml, &pool)
}

#[derive(Default)]
struct ReturnedWordRead {
    comments: Option<crate::comments::CommentsReadFacts>,
    body: Vec<(usize, crate::text::WordBlockSourceMaps)>,
    notes: Vec<Vec<(usize, crate::text::WordBlockSourceMaps)>>,
}

fn returned_body_owner(item: &BodyContent) -> Option<WordOwner<'_>> {
    match item {
        BodyContent::Paragraph(value) => Some(WordOwner::Paragraph(value)),
        BodyContent::Table(value) => Some(WordOwner::Table(value)),
        BodyContent::ContentControl(value) => Some(WordOwner::Control(value)),
        BodyContent::RawXml(_) => None,
    }
}

fn returned_path_owner<'b>(
    mut owner: WordOwner<'b>,
    path: &[crate::text::WordBlockComponent],
) -> Result<WordOwner<'b>> {
    use crate::text::WordBlockComponent as C;
    // The checked row reader returns the sole actual row of its existing
    // temporary table. Its companion still includes that physical root slot.
    let path = if matches!(owner, WordOwner::Row(_)) && matches!(path.first(), Some(C::TableRow(0)))
    {
        &path[1..]
    } else {
        path
    };
    let invalid = || OxmlError::InvalidValue("returned Word model path changed".into());
    for component in path {
        owner = match (owner, component) {
            (WordOwner::Paragraph(paragraph), C::ParagraphControl(index)) => WordOwner::Control(
                &paragraph
                    .content_controls
                    .get(*index)
                    .ok_or_else(invalid)?
                    .3,
            ),
            (WordOwner::Paragraph(paragraph), C::ParagraphRevision(index)) => {
                WordOwner::Revision(&paragraph.revisions.get(*index).ok_or_else(invalid)?.2)
            }
            (WordOwner::Revision(revision), C::RevisionParagraph) => {
                WordOwner::Paragraph(revision.content_paragraph().ok_or_else(invalid)?)
            }
            (WordOwner::Control(control), C::ControlRevision(index)) => {
                WordOwner::Revision(&control.revisions().get(*index).ok_or_else(invalid)?.1)
            }
            (WordOwner::Table(table), C::TableRow(index)) => {
                WordOwner::Row(table.rows.get(*index).ok_or_else(invalid)?)
            }
            (WordOwner::Table(table), C::TableControl(index)) => {
                WordOwner::Control(&table.content_controls.get(*index).ok_or_else(invalid)?.2)
            }
            (WordOwner::Row(row), C::RowCell(index)) => {
                WordOwner::Cell(row.cells.get(*index).ok_or_else(invalid)?)
            }
            (WordOwner::Row(row), C::RowControl(index)) => {
                WordOwner::Control(&row.content_controls.get(*index).ok_or_else(invalid)?.2)
            }
            (WordOwner::Cell(cell), C::CellContent(index)) => {
                match cell.content.get(*index).ok_or_else(invalid)? {
                    crate::table::CellContent::Paragraph(value) => WordOwner::Paragraph(value),
                    crate::table::CellContent::Table(value) => WordOwner::Table(value),
                    crate::table::CellContent::ContentControl(value) => WordOwner::Control(value),
                }
            }
            (WordOwner::Control(control), C::SdtContent(index)) => {
                match control.content.get(*index).ok_or_else(invalid)? {
                    crate::content_control::SdtContent::Paragraph(value) => {
                        WordOwner::Paragraph(value)
                    }
                    crate::content_control::SdtContent::Run(value) => WordOwner::Run(value),
                    crate::content_control::SdtContent::Table(value) => WordOwner::Table(value),
                    crate::content_control::SdtContent::Row(value) => WordOwner::Row(value),
                    crate::content_control::SdtContent::Cell(value) => WordOwner::Cell(value),
                    crate::content_control::SdtContent::ContentControl(value) => {
                        WordOwner::Control(value)
                    }
                    _ => return Err(invalid()),
                }
            }
            _ => return Err(invalid()),
        };
    }
    Ok(owner)
}

// Text, Field and borrowed XML are actual consumers of this clone-pair check.
fn dedup_template_clone_pairs<T: ?Sized>(pairs: &mut Vec<(&T, &T)>) -> Result<()> {
    let mut seen = std::collections::HashMap::new();
    let mut conflict = false;
    pairs.retain(|(current, frozen)| {
        let current = (
            std::ptr::from_ref(*current).cast::<()>() as usize,
            std::mem::size_of_val(*current),
        );
        let frozen = (
            std::ptr::from_ref(*frozen).cast::<()>() as usize,
            std::mem::size_of_val(*frozen),
        );
        if let Some(previous) = seen.insert(current, frozen) {
            conflict |= previous != frozen;
            false
        } else {
            true
        }
    });
    if conflict {
        return Err(OxmlError::InvalidValue(
            "conflicting actual paragraph clone pairs".into(),
        ));
    }
    Ok(())
}

// These pairs come from one actual typed clone, before the checked Field-slot
// installer changes ordinary Text or retained raw slots. Field children retain
// their own preparation receipts instead of borrowing a new clone baseline.
#[allow(clippy::type_complexity)]
pub(crate) fn template_paragraph_clone_pairs<'b, 'c>(
    previous: &'b CT_P,
    current: &'c CT_P,
) -> Result<(
    Vec<(&'c crate::text::CT_Text, &'b crate::text::CT_Text)>,
    Vec<(&'c crate::text::Field, &'b crate::text::Field)>,
    Vec<(&'c [u8], &'b [u8])>,
)> {
    use crate::text::RunContent;
    let mut texts = Vec::new();
    let mut fields = Vec::new();
    let mut raws = Vec::new();
    let mut pending = vec![(
        WordOwner::Paragraph(previous),
        WordOwner::Paragraph(current),
    )];
    while let Some((old, new)) = pending.pop() {
        match (old, new) {
            (WordOwner::Run(old), WordOwner::Run(new)) => {
                for (old, new) in old.content.iter().zip(&new.content) {
                    if let (RunContent::Text(old), RunContent::Text(new)) = (old, new) {
                        texts.push((new, old));
                    }
                }
                raws.extend(
                    new.extra_xml
                        .iter()
                        .zip(&old.extra_xml)
                        .map(|(new, old)| (new.as_slice(), old.as_slice())),
                );
            }
            (WordOwner::Paragraph(old), WordOwner::Paragraph(new)) => {
                raws.extend(
                    new.extra_xml
                        .iter()
                        .zip(&old.extra_xml)
                        .map(|(new, old)| (new.1.as_slice(), old.1.as_slice())),
                );
                for (old, new) in old.rubies.iter().zip(&new.rubies) {
                    raws.extend(
                        new.raw_xml
                            .iter()
                            .zip(&old.raw_xml)
                            .map(|(new, old)| (new.as_slice(), old.as_slice())),
                    );
                }
            }
            (WordOwner::Control(old), WordOwner::Control(new)) => {
                for ((old_index, old), (new_index, new)) in old
                    .retained_resource_run_xml()
                    .zip(new.retained_resource_run_xml())
                {
                    if old_index != new_index {
                        return Err(OxmlError::InvalidValue(
                            "cloned control source slots changed".into(),
                        ));
                    }
                    raws.push((new, old));
                }
                for (old, new) in old.content.iter().zip(&new.content) {
                    if let (
                        crate::content_control::SdtContent::RawXml(old),
                        crate::content_control::SdtContent::RawXml(new),
                    ) = (old, new)
                    {
                        raws.push((new, old));
                    }
                }
            }
            (WordOwner::Revision(old), WordOwner::Revision(new)) => {
                raws.push((new.retained_xml(), old.retained_xml()))
            }
            (WordOwner::Field(old), WordOwner::Field(new)) => {
                fields.push((new, old));
                continue;
            }
            (WordOwner::Drawing(old), WordOwner::Drawing(new)) => {
                if let (Some(old), Some(new)) = (&old.inline, &new.inline)
                    && let (Some(old), Some(new)) = (&old.raw_xml, &new.raw_xml)
                {
                    raws.push((new, old));
                }
            }
            (WordOwner::Anchor(old), WordOwner::Anchor(new)) => {
                if let (Some(old), Some(new)) = (&old.raw_xml, &new.raw_xml) {
                    raws.push((new, old));
                }
            }
            _ => {}
        }
        let before = old.resource_children();
        let after = new.resource_children();
        if before.len() != after.len()
            || before
                .iter()
                .zip(&after)
                .any(|(old, new)| old.key().0 != new.key().0)
        {
            return Err(OxmlError::InvalidValue(
                "cloned paragraph source shape changed".into(),
            ));
        }
        pending.extend(before.into_iter().zip(after));
    }
    dedup_template_clone_pairs(&mut texts)?;
    dedup_template_clone_pairs(&mut fields)?;
    dedup_template_clone_pairs(&mut raws)?;
    Ok((texts, fields, raws))
}

fn returned_run_inputs<'b>(
    run: &'b crate::text::CT_R,
    facts: &crate::text::ParsedWordRunTexts,
    outer: &WordSourceMap,
    output: &mut Vec<(&'b crate::text::CT_Text, crate::text::TemplateWordEvent)>,
    raws: &mut Vec<(&'b [u8], WordSourceMap)>,
) -> Result<()> {
    let map = facts.source_map.as_deref().unwrap_or(outer);
    for parsed in &facts.texts {
        if let Some(maps) = &parsed.drawing
            && let Some(crate::text::RunContent::Drawing(drawing)) = run.content.get(parsed.content)
        {
            for (raw, map) in [
                (
                    drawing
                        .inline
                        .as_ref()
                        .and_then(|owner| owner.raw_xml.as_ref()),
                    maps.inline.as_deref(),
                ),
                (
                    drawing
                        .anchor
                        .as_ref()
                        .and_then(|owner| owner.raw_xml.as_ref()),
                    maps.anchor.as_deref(),
                ),
            ] {
                if let (Some(raw), Some(map)) = (raw, map)
                    && raw.len() == map.len()
                {
                    raws.push((raw.as_slice(), map.clone()));
                }
            }
            continue;
        }
        let Some(crate::text::RunContent::Text(text)) = run.content.get(parsed.content) else {
            continue;
        };
        let encoded =
            facts.source.start + parsed.encoded.start..facts.source.start + parsed.encoded.end;
        let event = map
            .word_events
            .binary_search_by_key(&encoded.start, |event| event.encoded.start)
            .ok()
            .and_then(|index| map.word_events.get(index))
            .filter(|event| event.encoded == encoded && event.decoded.len() == text.text.len())
            .cloned()
            .or_else(|| {
                map.word_range(encoded.clone())
                    .map(|origin| crate::text::TemplateWordEvent {
                        encoded,
                        origin: crate::text::TemplateWordOrigin::Raw {
                            start: origin.start,
                            end: origin.end,
                        },
                        decoded: 0..text.text.len(),
                    })
            });
        if let Some(event) = event {
            output.push((text, event));
        }
    }
    Ok(())
}

#[allow(clippy::type_complexity)]
pub(crate) fn returned_block_inputs<'b>(
    owner: WordOwner<'b>,
    maps: &crate::text::WordBlockSourceMaps,
) -> Result<(
    Vec<(&'b crate::text::CT_Text, crate::text::TemplateWordEvent)>,
    Vec<(&'b [u8], WordSourceMap)>,
)> {
    let mut texts = Vec::new();
    let mut raws = Vec::new();
    for (path, facts) in &maps.word_texts {
        let Some(facts) = facts else {
            continue;
        };
        let WordOwner::Paragraph(paragraph) = returned_path_owner(owner, path)? else {
            return Err(OxmlError::InvalidValue(
                "returned paragraph path changed".into(),
            ));
        };
        let (nested_texts, nested_raws) =
            returned_block_inputs(WordOwner::Paragraph(paragraph), &facts.nested)?;
        texts.extend(nested_texts);
        raws.extend(nested_raws);
        for (index, run) in facts.texts.iter().enumerate() {
            if let (Some(run_facts), Some(run)) = (run, paragraph.runs.get(index)) {
                returned_run_inputs(run, run_facts, &facts.source_map, &mut texts, &mut raws)?;
            }
        }
    }
    for (path, map) in &maps.word_raws {
        if let WordOwner::Revision(revision) = returned_path_owner(owner, path)? {
            let raw = revision.retained_xml();
            if map.len() == raw.len() {
                raws.push((raw, map.as_ref().clone()));
            }
        }
    }
    for (path, facts) in &maps.word_runs {
        let Some(facts) = facts else {
            continue;
        };
        if let WordOwner::Run(run) = returned_path_owner(owner, path)?
            && let Some(map) = facts.source_map.as_deref()
        {
            returned_run_inputs(run, facts, map, &mut texts, &mut raws)?;
            if let Some(crate::text::WordBlockComponent::SdtContent(index)) = path.last()
                && let WordOwner::Control(control) =
                    returned_path_owner(owner, &path[..path.len() - 1])?
            {
                if let Some((_, raw)) = control
                    .retained_resource_run_xml()
                    .find(|(actual, _)| actual == index)
                    && map.len() == raw.len()
                {
                    raws.push((raw, map.clone()));
                }
                if let Some((_, original)) = control
                    .retained_resource_run_xml()
                    .zip(control.original_resource_runs())
                    .find(|((actual, _), _)| actual == index)
                {
                    returned_run_inputs(original, facts, map, &mut texts, &mut raws)?;
                }
            }
        }
    }
    for (path, extra_maps) in &maps.paragraphs {
        let WordOwner::Paragraph(paragraph) = returned_path_owner(owner, path)? else {
            continue;
        };
        for (raw, map) in paragraph.extra_xml.iter().zip(extra_maps) {
            if let Some(map) = map
                && map.len() == raw.1.len()
            {
                raws.push((raw.1.as_slice(), map.as_ref().clone()));
            }
        }
    }
    Ok((texts, raws))
}

/// An operation-produced source. Its bytes and correspondence move together.
/// There is deliberately no public constructor or Clone implementation.
#[doc(hidden)]
pub struct ScopedWordSource {
    read_facts: std::sync::Mutex<Option<ReturnedWordRead>>,
    initial_delivery: bool,
    bytes: Vec<u8>,
    publication: std::sync::Arc<std::sync::Mutex<WordPublicationCoverage>>,
    source_map: WordSourceMap,
    // Word-only physical correspondence never grants Field source/current work.
    word_source_map: WordSourceMap,
    word_events: Vec<crate::text::TemplateWordEvent>,
    constructors: Option<std::sync::Arc<ConstructorFailures>>,
    payload: Option<Box<ImportedPayload>>,
    operation: std::sync::Arc<()>,
}

impl ScopedWordSource {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    fn admitted_word_source(&self) -> Result<AdmittedWordSource> {
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

    fn current_publication(&self) -> WordPublicationCoverage {
        let mut coverage = self.source_map.materialized_publication();
        let current = self
            .publication
            .lock()
            .expect("publication coverage poisoned");
        coverage.add_ranges(current.fields.clone());
        coverage.add_ordinary_ranges(current.ordinary.clone());
        coverage
    }

    fn imported_owner_kind(&self, owner: &std::ops::Range<usize>) -> Option<bool> {
        // The usual Field owner is already committed in this source domain.
        // Avoid cloning every other Field for each small scalar event.
        if self
            .publication
            .lock()
            .expect("publication coverage poisoned")
            .covers_ranges(std::slice::from_ref(owner))
        {
            return Some(true);
        }
        let coverage = self.current_publication();
        if coverage.covers_ranges(std::slice::from_ref(owner)) {
            return Some(true);
        }
        let first = coverage
            .ordinary
            .partition_point(|range| range.end <= owner.start);
        coverage
            .ordinary
            .get(first)
            .is_some_and(|range| range.start <= owner.start && owner.end <= range.end)
            .then_some(false)
    }
}

// This binding is inseparable from the produced owned buffer. Only the moved
// rewrite path can consume it. A Weak parent retains no source/model bytes.
struct ImportedPayload {
    parent: std::sync::Weak<std::sync::Mutex<WordPublicationCoverage>>,
    owner: std::ops::Range<usize>,
    // Some(len), including zero, is exact Field prepayment. None is ordinary.
    prepaid: Option<usize>,
}

// Both carrier assembly and imported splicing use this one byte/map recipe.
// The caller chooses its actual admission boundary before materialization.
struct SourceAssembly<'s> {
    chunks: Vec<&'s [u8]>,
    coverage: WordPublicationCoverage,
    source_map: WordSourceMap,
    word_source_map: WordSourceMap,
    word_events: Vec<crate::text::TemplateWordEvent>,
    length: usize,
}

/// Template copies have their own input and demand accounting. Field work
/// cannot spend this allowance, including the per-operand fixed component.
#[derive(Default)]
pub(crate) struct TemplateCopyWork {
    allowance: usize,
    copied: usize,
    inputs: std::collections::HashSet<usize>,
    word_inputs: std::collections::HashMap<
        crate::text::TemplateWordOrigin,
        std::collections::BTreeMap<usize, usize>,
    >,
    refused: bool,
}

impl TemplateCopyWork {
    fn reserve(&mut self, bytes: usize) -> Result<()> {
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

/// One scoped edit's reached original sources and cumulative materializations.
/// Field/current-value aggregation remains separate from capture and insertion.
#[doc(hidden)]
pub struct ScopedWordOperation<'a> {
    original_parts: Vec<&'a [u8]>,
    part_names: std::collections::HashMap<&'a str, usize>,
    current_paragraphs: std::collections::HashMap<usize, &'a CT_P>,
    current_word_sources: Vec<&'a [u8]>,
    current_word_source_keys: std::collections::HashSet<(usize, usize)>,
    current_word_views: std::collections::BTreeMap<usize, usize>,
    current_texts: std::collections::HashMap<usize, &'a crate::text::CT_Text>,
    current_fields: std::collections::HashMap<usize, &'a crate::text::Field>,
    field_work: crate::text::WordOperationWork,
    template_datums: std::collections::HashMap<usize, &'a serde_json::Value>,
    template_copy_work: TemplateCopyWork,
    field_origins: std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>>,
    identity: std::sync::Arc<()>,
    declarations: CaptureDeclarations,
    ranges: std::collections::BTreeMap<(usize, usize), usize>,
    source_bytes: usize,
    attributes: std::collections::HashMap<(usize, usize, usize), usize>,
    attribute_bytes: usize,
    capture_namespaces: std::collections::HashSet<u32>,
    capture_scope_bytes: usize,
    insertion_namespaces: std::collections::HashSet<u32>,
    insertion_scope_bytes: usize,
    capture_needed: usize,
    insertion_needed: usize,
}

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

    /// Serialize only a current paragraph frozen at checked-operation ingress.
    /// Its retained/current records are distinct from original OPC ranges.
    pub fn source_model_paragraph(
        &mut self,
        paragraph: &'a CT_P,
    ) -> Result<(CT_P, ScopedWordSource)> {
        let owner = std::ptr::from_ref(paragraph) as usize;
        if !self
            .current_paragraphs
            .get(&owner)
            .is_some_and(|frozen| std::ptr::eq(*frozen, paragraph))
        {
            return Err(OxmlError::InvalidValue(
                "paragraph is not in the frozen operation input".into(),
            ));
        }
        admit_word_model(WordOwner::Paragraph(paragraph))?;
        let mut roots = paragraph.writer_field_roots();
        roots.extend(
            crate::text::ordered_normalization_children(paragraph)?
                .into_iter()
                .map(|(field, _)| field),
        );
        self.field_work.register_model_fields(roots)?;
        self.field_work
            .register_model_result_properties(paragraph)?;
        let mut prepared = paragraph.clone();
        prepared.normalize_field_owners_in(&mut self.field_work)?;
        let mut writer = quick_xml::Writer::new(self.field_work.model_source_output());
        prepared
            .write_xml_core_in(
                &mut writer,
                None,
                crate::text::XmlWriteRole::Source,
                None,
                &mut self.field_work,
                None,
            )
            .map_err(|error| self.field_work.writer_error(error))?;
        let (bytes, source_map) = writer.into_inner().into_mapped_parts();
        // This is constructor policy inventory of a produced buffer, never an
        // ingress registration or credit for that generated buffer.
        let admitted = admit_word_source(&bytes, &[], None, None)?;
        let source = ScopedWordSource {
            publication: source_map.publication_output(),
            initial_delivery: false,
            word_source_map: WordSourceMap::generated(bytes.len()),
            source_map,
            word_events: Vec::new(),
            constructors: Some(admitted.constructors),
            bytes,
            payload: None,
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        };
        Ok((prepared, source))
    }

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
            payload: None,
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
            payload: None,
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        })
    }

    fn validate_template_word_origins(
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

    fn template_model_field_origins<'current>(
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

    fn register_template_model_fields(
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

    /// Emit this real body child through the same ancestor-aware writer.
    pub fn source_model_body_item(
        &mut self,
        frame: &CT_Document,
        item: &BodyContent,
        ancestors: &[&[(String, String)]],
        text_origins: &[(&crate::text::CT_Text, &'a crate::text::CT_Text)],
        field_origins: &[(&crate::text::Field, &'a crate::text::Field)],
        raw_origins: &[(&[u8], &'a [u8])],
    ) -> Result<ScopedWordSource> {
        self.validate_template_word_origins(text_origins, field_origins, raw_origins)?;
        let owner = match item {
            BodyContent::Paragraph(value) => Some(WordOwner::Paragraph(value)),
            BodyContent::Table(value) => Some(WordOwner::Table(value)),
            BodyContent::ContentControl(value) => Some(WordOwner::Control(value)),
            BodyContent::RawXml(_) => None,
        };
        if let Some(owner) = owner {
            admit_word_model(owner)?;
        }
        let field_origins =
            self.template_model_field_origins(owner.into_iter().collect(), field_origins);
        self.register_template_model_fields(owner.into_iter().collect(), &field_origins)?;
        let (output, lease) = self.field_work.template_word_source_output(
            text_origins,
            &field_origins,
            raw_origins,
        )?;
        let mut writer = Writer::new(output);
        frame.write_body_item_in(item, ancestors, &mut writer, Some(lease.work))?;
        let (bytes, source_map, word_events) = writer.into_inner().into_word_parts();
        drop(lease);
        let admitted = admit_word_source(&bytes, &[], None, None)?;
        Ok(ScopedWordSource {
            publication: source_map.publication_output(),
            initial_delivery: false,
            word_source_map: WordSourceMap::generated(bytes.len()),
            bytes,
            source_map,
            word_events,
            constructors: Some(admitted.constructors),
            payload: None,
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        })
    }

    /// Emit the actual full-document fallback through its existing part policy.
    pub fn source_model_document(
        &mut self,
        document: &CT_Document,
        text_origins: &[(&crate::text::CT_Text, &'a crate::text::CT_Text)],
        field_origins: &[(&crate::text::Field, &'a crate::text::Field)],
        raw_origins: &[(&[u8], &'a [u8])],
    ) -> Result<ScopedWordSource> {
        self.validate_template_word_origins(text_origins, field_origins, raw_origins)?;
        document.validate_resource_limits()?;
        let field_origins =
            self.template_model_field_origins(vec![WordOwner::Body(&document.body)], field_origins);
        self.register_template_model_fields(vec![WordOwner::Body(&document.body)], &field_origins)?;
        let (output, lease) = self.field_work.template_word_source_output(
            text_origins,
            &field_origins,
            raw_origins,
        )?;
        let mut writer = Writer::new(output);
        document.write_part_contents(
            &mut writer,
            PartWrite::Whole,
            &mut |_| {},
            Some(lease.work),
        )?;
        let (bytes, source_map, word_events) = writer.into_inner().into_word_parts();
        drop(lease);
        let source = ScopedWordSource {
            publication: source_map.publication_output(),
            initial_delivery: false,
            word_source_map: WordSourceMap::generated(bytes.len()),
            bytes,
            source_map,
            word_events,
            constructors: None,
            payload: None,
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        };
        let mut source = self.declare_source_w14_on_root(source)?;
        if source.constructors.is_none() {
            let admitted = admit_word_source(&source.bytes, &[], None, None)?;
            source.constructors = Some(admitted.constructors);
        }
        Ok(source)
    }

    /// Emit the actual document paired with this operation's successful reader.
    #[doc(hidden)]
    pub fn source_model_document_from_read(
        &mut self,
        document: &CT_Document,
        source: &ScopedWordSource,
    ) -> Result<ScopedWordSource> {
        self.check_source(source)?;
        document.validate_resource_limits()?;
        let reads = source
            .read_facts
            .lock()
            .expect("returned Word facts poisoned");
        let reads = reads
            .as_ref()
            .ok_or_else(|| OxmlError::InvalidValue("source has no successful model read".into()))?;
        let mut texts = Vec::new();
        let mut raws = Vec::new();
        for (index, maps) in &reads.body {
            let item = document
                .body
                .content
                .get(*index)
                .ok_or_else(|| OxmlError::InvalidValue("returned body path changed".into()))?;
            if let Some(owner) = returned_body_owner(item) {
                let (child_texts, child_raws) = returned_block_inputs(owner, maps)?;
                texts.extend(child_texts);
                raws.extend(child_raws);
            }
        }
        let fields = self.template_model_field_origins(vec![WordOwner::Body(&document.body)], &[]);
        self.register_template_model_fields(vec![WordOwner::Body(&document.body)], &fields)?;
        let (output, lease) = self
            .field_work
            .template_read_source_output(&texts, &raws, &fields)?;
        let mut writer = Writer::new(output);
        document.write_part_contents(
            &mut writer,
            PartWrite::Whole,
            &mut |_| {},
            Some(lease.work),
        )?;
        let (bytes, source_map, word_events) = writer.into_inner().into_word_parts();
        drop(lease);
        let source = ScopedWordSource {
            publication: source_map.publication_output(),
            initial_delivery: false,
            word_source_map: WordSourceMap::generated(bytes.len()),
            bytes,
            source_map,
            word_events,
            constructors: None,
            payload: None,
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        };
        let mut source = self.declare_source_w14_on_root(source)?;
        if source.constructors.is_none() {
            source.constructors =
                Some(admit_word_source(&source.bytes, &[], None, None)?.constructors);
        }
        Ok(source)
    }

    /// Emit an actual selected item paired with the reader's physical body slot.
    #[doc(hidden)]
    pub fn source_model_body_item_from_read(
        &mut self,
        frame: &CT_Document,
        item: &BodyContent,
        source: &ScopedWordSource,
        body_index: usize,
        ancestors: &[&[(String, String)]],
    ) -> Result<ScopedWordSource> {
        self.check_source(source)?;
        let reads = source
            .read_facts
            .lock()
            .expect("returned Word facts poisoned");
        let reads = reads
            .as_ref()
            .ok_or_else(|| OxmlError::InvalidValue("source has no successful model read".into()))?;
        let owner = returned_body_owner(item);
        if let Some(owner) = owner {
            admit_word_model(owner)?;
        }
        let (texts, raws) = if let (Some(owner), Some((_, maps))) = (
            owner,
            reads.body.iter().find(|(index, _)| *index == body_index),
        ) {
            returned_block_inputs(owner, maps)?
        } else {
            (Vec::new(), Vec::new())
        };
        let fields = self.template_model_field_origins(owner.into_iter().collect(), &[]);
        self.register_template_model_fields(owner.into_iter().collect(), &fields)?;
        let (output, lease) = self
            .field_work
            .template_read_source_output(&texts, &raws, &fields)?;
        let mut writer = Writer::new(output);
        frame.write_body_item_in(item, ancestors, &mut writer, Some(lease.work))?;
        let (bytes, source_map, word_events) = writer.into_inner().into_word_parts();
        drop(lease);
        let admitted = admit_word_source(&bytes, &[], None, None)?;
        Ok(ScopedWordSource {
            publication: source_map.publication_output(),
            initial_delivery: false,
            word_source_map: WordSourceMap::generated(bytes.len()),
            bytes,
            source_map,
            word_events,
            constructors: Some(admitted.constructors),
            payload: None,
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        })
    }

    /// Emit the actual related-note family through its existing root writer.
    pub fn source_model_notes(
        &mut self,
        notes: &crate::footnotes::CT_Footnotes,
        endnotes: bool,
        text_origins: &[(&crate::text::CT_Text, &'a crate::text::CT_Text)],
        field_origins: &[(&crate::text::Field, &'a crate::text::Field)],
        raw_origins: &[(&[u8], &'a [u8])],
        prepare_drawing_namespaces: bool,
    ) -> Result<ScopedWordSource> {
        self.validate_template_word_origins(text_origins, field_origins, raw_origins)?;
        let owners = notes
            .footnotes
            .iter()
            .flat_map(|note| note.paragraphs.iter())
            .map(WordOwner::Paragraph)
            .collect::<Vec<_>>();
        for owner in &owners {
            admit_word_model(*owner)?;
        }
        let field_origins = self.template_model_field_origins(owners.clone(), field_origins);
        self.register_template_model_fields(owners, &field_origins)?;
        let (output, lease) = self.field_work.template_word_source_output(
            text_origins,
            &field_origins,
            raw_origins,
        )?;
        let mut writer = Writer::new(output);
        let (root, item) = if endnotes {
            ("w:endnotes", "w:endnote")
        } else {
            ("w:footnotes", "w:footnote")
        };
        if prepare_drawing_namespaces {
            lease.work.prepare_note_drawing_bindings();
        }
        notes.write_xml_root_in(&mut writer, root, item, Some(lease.work))?;
        let (bytes, source_map, word_events) = writer.into_inner().into_word_parts();
        drop(lease);
        let source = ScopedWordSource {
            publication: source_map.publication_output(),
            initial_delivery: false,
            word_source_map: WordSourceMap::generated(bytes.len()),
            bytes,
            source_map,
            word_events,
            constructors: None,
            payload: None,
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        };
        let mut source = self.declare_source_w14_on_root(source)?;
        if source.constructors.is_none() {
            source.constructors =
                Some(admit_word_source(&source.bytes, &[], None, None)?.constructors);
        }
        Ok(source)
    }

    /// Emit the existing whole-comments writer with actual frozen clone facts.
    #[doc(hidden)]
    pub fn source_model_comments(
        &mut self,
        comments: &crate::comments::CT_Comments,
        text_origins: &[(&crate::text::CT_Text, &'a crate::text::CT_Text)],
        field_origins: &[(&crate::text::Field, &'a crate::text::Field)],
        raw_origins: &[(&[u8], &'a [u8])],
    ) -> Result<ScopedWordSource> {
        self.validate_template_word_origins(text_origins, field_origins, raw_origins)?;
        comments.validate_writer()?;
        let owners = comments
            .comments
            .iter()
            .flat_map(|comment| &comment.paragraphs)
            .map(WordOwner::Paragraph)
            .collect::<Vec<_>>();
        let fields = self.template_model_field_origins(owners.clone(), field_origins);
        self.register_template_model_fields(owners, &fields)?;
        let (output, lease) =
            self.field_work
                .template_word_source_output(text_origins, &fields, raw_origins)?;
        let mut writer = Writer::new(output);
        lease.work.prepare_note_drawing_bindings();
        comments.write_xml_in(&mut writer, Some(lease.work))?;
        let (bytes, source_map, word_events) = writer.into_inner().into_word_parts();
        drop(lease);
        let source = ScopedWordSource {
            publication: source_map.publication_output(),
            initial_delivery: false,
            word_source_map: WordSourceMap::generated(bytes.len()),
            bytes,
            source_map,
            word_events,
            constructors: None,
            payload: None,
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        };
        self.declare_source_w14_on_root(source)
    }

    /// Parse the same successful comments factory and retain its actual owners.
    #[doc(hidden)]
    pub fn source_comments(
        &mut self,
        source: &ScopedWordSource,
    ) -> Result<crate::comments::CT_Comments> {
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
        let admitted = admit_reader_source(
            &source.bytes,
            WordSourceReader::Part(PartReaderKind::Comments),
        )?
        .with_source_map(map)?;
        let mut controls = admitted.controls_for(&source.bytes).with_field_origins(
            std::sync::Arc::clone(&self.field_origins),
            source.initial_delivery,
        );
        let mut facts = crate::comments::CommentsReadFacts::default();
        let comments = crate::comments::CT_Comments::from_xml_with_read_output(
            &source.bytes,
            &mut controls,
            Some(&mut facts),
        )?;
        for comment in &comments.comments {
            for paragraph in &comment.paragraphs {
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
            comments: Some(facts),
            ..Default::default()
        });
        Ok(comments)
    }

    /// Emit the comments model paired with this operation's successful reader.
    #[doc(hidden)]
    pub fn source_model_comments_from_read(
        &mut self,
        comments: &crate::comments::CT_Comments,
        source: &ScopedWordSource,
    ) -> Result<ScopedWordSource> {
        self.check_source(source)?;
        let reads = source
            .read_facts
            .lock()
            .expect("returned Word facts poisoned");
        let reads = reads
            .as_ref()
            .and_then(|reads| reads.comments.as_ref())
            .ok_or_else(|| {
                OxmlError::InvalidValue("source has no successful comments read".into())
            })?;
        comments.validate_writer()?;
        let mut texts = Vec::new();
        let mut raws = Vec::new();
        let mut owners = Vec::new();
        for ((_, raw), map) in comments.extra_xml.iter().zip(&reads.raws) {
            if let Some(map) = map
                && map.len() == raw.len()
            {
                raws.push((raw.as_slice(), map.clone()));
            }
        }
        for (index, comment) in comments.comments.iter().enumerate() {
            owners.extend(comment.paragraphs.iter().map(WordOwner::Paragraph));
            if let Some(read) = reads.comments.get(index) {
                for ((_, raw), map) in comment.extra_xml.iter().zip(&read.raws) {
                    if let Some(map) = map
                        && map.len() == raw.len()
                    {
                        raws.push((raw.as_slice(), map.clone()));
                    }
                }
                for (index, maps) in &read.paragraphs {
                    let paragraph = comment.paragraphs.get(*index).ok_or_else(|| {
                        OxmlError::InvalidValue("returned comment paragraph path changed".into())
                    })?;
                    let (child_texts, child_raws) =
                        returned_block_inputs(WordOwner::Paragraph(paragraph), maps)?;
                    texts.extend(child_texts);
                    raws.extend(child_raws);
                }
            }
        }
        let fields = self.template_model_field_origins(owners.clone(), &[]);
        self.register_template_model_fields(owners, &fields)?;
        let (output, lease) = self
            .field_work
            .template_read_source_output(&texts, &raws, &fields)?;
        let mut writer = Writer::new(output);
        lease.work.prepare_note_drawing_bindings();
        comments.write_xml_in(&mut writer, Some(lease.work))?;
        let (bytes, source_map, word_events) = writer.into_inner().into_word_parts();
        drop(lease);
        let source = ScopedWordSource {
            publication: source_map.publication_output(),
            initial_delivery: false,
            word_source_map: WordSourceMap::generated(bytes.len()),
            bytes,
            source_map,
            word_events,
            constructors: None,
            payload: None,
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        };
        self.declare_source_w14_on_root(source)
    }

    /// Emit the actual glossary structural edit recipe with its source witnesses.
    pub fn source_model_glossary(
        &mut self,
        glossary: &crate::glossary::CT_GlossaryDocument,
        text_origins: &[(&crate::text::CT_Text, &'a crate::text::CT_Text)],
        field_origins: &[(&crate::text::Field, &'a crate::text::Field)],
        raw_origins: &[(&[u8], &'a [u8])],
    ) -> Result<ScopedWordSource> {
        self.validate_template_word_origins(text_origins, field_origins, raw_origins)?;
        glossary.validate_resource_limits()?;
        self.register_template_model_fields(
            glossary
                .doc_parts
                .iter()
                .map(|part| WordOwner::Body(&part.body))
                .collect(),
            field_origins,
        )?;
        let lease = self.field_work.template_word_source_context(
            text_origins,
            field_origins,
            raw_origins,
        )?;
        let output = glossary.to_xml_in(Some(lease.work))?;
        drop(lease);
        let admitted = admit_word_source(&output.bytes, &[], None, None)?;
        let source_map = output.source_map.expect("scoped glossary map");
        Ok(ScopedWordSource {
            publication: source_map.publication_output(),
            initial_delivery: false,
            word_source_map: WordSourceMap::generated(output.bytes.len()),
            bytes: output.bytes,
            source_map,
            word_events: output.events,
            constructors: Some(admitted.constructors),
            payload: None,
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        })
    }

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
            payload: None,
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
        // actual copy reservation. Imported rewrite has its own reservation.
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
            payload: None,
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
            payload: None,
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        })
    }

    fn check_source(&self, source: &ScopedWordSource) -> Result<()> {
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

    /// Emit the actual note-family model paired with its successful reader.
    #[doc(hidden)]
    pub fn source_model_notes_from_read(
        &mut self,
        notes: &crate::footnotes::CT_Footnotes,
        endnotes: bool,
        source: &ScopedWordSource,
    ) -> Result<ScopedWordSource> {
        self.check_source(source)?;
        let reads = source
            .read_facts
            .lock()
            .expect("returned Word facts poisoned");
        let reads = reads
            .as_ref()
            .ok_or_else(|| OxmlError::InvalidValue("source has no successful model read".into()))?;
        let mut texts = Vec::new();
        let mut raws = Vec::new();
        let mut owners = Vec::new();
        for (index, note) in notes.footnotes.iter().enumerate() {
            for paragraph in &note.paragraphs {
                admit_word_model(WordOwner::Paragraph(paragraph))?;
                owners.push(WordOwner::Paragraph(paragraph));
            }
            if let Some(paragraphs) = reads.notes.get(index) {
                for (index, maps) in paragraphs {
                    let paragraph = note.paragraphs.get(*index).ok_or_else(|| {
                        OxmlError::InvalidValue("returned note paragraph path changed".into())
                    })?;
                    let (child_texts, child_raws) =
                        returned_block_inputs(WordOwner::Paragraph(paragraph), maps)?;
                    texts.extend(child_texts);
                    raws.extend(child_raws);
                }
            }
        }
        let fields = self.template_model_field_origins(owners.clone(), &[]);
        self.register_template_model_fields(owners, &fields)?;
        let (output, lease) = self
            .field_work
            .template_read_source_output(&texts, &raws, &fields)?;
        let mut writer = Writer::new(output);
        let (root, item) = if endnotes {
            ("w:endnotes", "w:endnote")
        } else {
            ("w:footnotes", "w:footnote")
        };
        notes.write_xml_root_in(&mut writer, root, item, Some(lease.work))?;
        let (bytes, source_map, word_events) = writer.into_inner().into_word_parts();
        drop(lease);
        let source = ScopedWordSource {
            publication: source_map.publication_output(),
            initial_delivery: false,
            word_source_map: WordSourceMap::generated(bytes.len()),
            bytes,
            source_map,
            word_events,
            constructors: None,
            payload: None,
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        };
        let mut source = self.declare_source_w14_on_root(source)?;
        if source.constructors.is_none() {
            source.constructors =
                Some(admit_word_source(&source.bytes, &[], None, None)?.constructors);
        }
        Ok(source)
    }

    /// Emit exactly the selected note from its actual reader vector position.
    #[doc(hidden)]
    pub fn source_model_note_from_read(
        &mut self,
        note: &crate::footnotes::CT_Footnote,
        endnotes: bool,
        source: &ScopedWordSource,
        note_index: usize,
    ) -> Result<ScopedWordSource> {
        self.check_source(source)?;
        let reads = source
            .read_facts
            .lock()
            .expect("returned Word facts poisoned");
        let paragraphs = reads
            .as_ref()
            .and_then(|reads| reads.notes.get(note_index))
            .ok_or_else(|| OxmlError::InvalidValue("source has no selected note read".into()))?;
        let mut texts = Vec::new();
        let mut raws = Vec::new();
        let owners = note
            .paragraphs
            .iter()
            .map(WordOwner::Paragraph)
            .collect::<Vec<_>>();
        for owner in &owners {
            admit_word_model(*owner)?;
        }
        for (index, maps) in paragraphs {
            let paragraph = note.paragraphs.get(*index).ok_or_else(|| {
                OxmlError::InvalidValue("returned selected note path changed".into())
            })?;
            let (child_texts, child_raws) =
                returned_block_inputs(WordOwner::Paragraph(paragraph), maps)?;
            texts.extend(child_texts);
            raws.extend(child_raws);
        }
        let fields = self.template_model_field_origins(owners.clone(), &[]);
        self.register_template_model_fields(owners, &fields)?;
        let (output, lease) = self
            .field_work
            .template_read_source_output(&texts, &raws, &fields)?;
        let mut writer = Writer::new(output);
        let (root, item) = if endnotes {
            ("w:endnotes", "w:endnote")
        } else {
            ("w:footnotes", "w:footnote")
        };
        crate::footnotes::CT_Footnotes::write_notes_root_in(
            &[note],
            &mut writer,
            root,
            item,
            Some(lease.work),
        )?;
        let (bytes, source_map, word_events) = writer.into_inner().into_word_parts();
        drop(lease);
        let source = ScopedWordSource {
            publication: source_map.publication_output(),
            initial_delivery: false,
            word_source_map: WordSourceMap::generated(bytes.len()),
            bytes,
            source_map,
            word_events,
            constructors: None,
            payload: None,
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        };
        let mut source = self.declare_source_w14_on_root(source)?;
        if source.constructors.is_none() {
            source.constructors =
                Some(admit_word_source(&source.bytes, &[], None, None)?.constructors);
        }
        Ok(source)
    }

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
                payload: None,
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
            payload: None,
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

    fn parse_source_paragraph_with_properties(
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

    /// Produce an import scalar under a proved whole current owner.
    /// Raw mode deliberately leaves XML validation to the existing later reader.
    #[doc(hidden)]
    pub fn source_imported_field_xml(
        &mut self,
        source: &ScopedWordSource,
        owner: std::ops::Range<usize>,
        value: &str,
        escape: bool,
    ) -> Result<ScopedWordSource> {
        self.check_source(source)?;
        if self.field_work.resource_refused() {
            return Err(self.field_work.writer_error(OxmlError::InvalidValue(
                "scoped operation resource admission refused".into(),
            )));
        }
        if owner.start >= owner.end || owner.end > source.bytes.len() {
            return Err(OxmlError::InvalidValue(
                "imported scalar owner range is invalid".into(),
            ));
        }
        let field = source.imported_owner_kind(&owner).ok_or_else(|| {
            OxmlError::InvalidValue("imported scalar owner lacks proved whole ownership".into())
        })?;
        let bytes = self.field_work.produce_imported_xml(value, escape, field)?;
        let mut publication = WordPublicationCoverage::default();
        if field && !bytes.is_empty() {
            publication.add_ranges(std::iter::once(0..bytes.len()).collect());
        }
        let length = bytes.len();
        Ok(ScopedWordSource {
            initial_delivery: false,
            bytes,
            publication: std::sync::Arc::new(std::sync::Mutex::new(publication)),
            source_map: WordSourceMap::generated(length),
            word_source_map: WordSourceMap::generated(length),
            word_events: Vec::new(),
            constructors: None,
            payload: Some(Box::new(ImportedPayload {
                parent: std::sync::Arc::downgrade(&source.publication),
                owner,
                prepaid: field.then_some(length),
            })),
            read_facts: Default::default(),
            operation: std::sync::Arc::clone(&self.identity),
        })
    }

    /// Consume only actual scalar payloads produced for this exact parent.
    /// Unchanged pieces retain their maps. Replacements grant no input credit.
    #[doc(hidden)]
    pub fn rewrite_imported_source(
        &mut self,
        source: ScopedWordSource,
        mut edits: Vec<(std::ops::Range<usize>, ScopedWordSource)>,
    ) -> Result<ScopedWordSource> {
        self.check_source(&source)?;
        if self.field_work.resource_refused() {
            return Err(self.field_work.writer_error(OxmlError::InvalidValue(
                "scoped operation resource admission refused".into(),
            )));
        }
        if edits.is_empty() {
            return Ok(source);
        }
        edits.sort_by_key(|(range, _)| range.start);
        let coverage = source.current_publication();
        let parent = std::sync::Arc::downgrade(&source.publication);
        let mut previous = 0;
        let mut projected = source.bytes.len();
        let mut demand = coverage
            .fields
            .iter()
            .try_fold(0usize, |sum, field| sum.checked_add(field.len()))
            .ok_or_else(|| OxmlError::InvalidValue("imported Field output work overflow".into()))?;
        let mut pieces = Vec::new();
        for (range, payload) in &edits {
            self.check_source(payload)?;
            let binding = payload.payload.as_ref().ok_or_else(|| {
                OxmlError::InvalidValue("replacement is not a produced scalar payload".into())
            })?;
            if !std::sync::Weak::ptr_eq(&parent, &binding.parent) {
                return Err(OxmlError::InvalidValue(
                    "scalar payload belongs to another source occurrence".into(),
                ));
            }
            if range.start < previous
                || range.start > range.end
                || range.end > source.bytes.len()
                || range.start < binding.owner.start
                || binding.owner.end < range.end
                || range.is_empty()
                    && (range.start <= binding.owner.start || range.end >= binding.owner.end)
            {
                return Err(OxmlError::InvalidValue(
                    "imported scalar edit range is invalid".into(),
                ));
            }
            let field = coverage.covers_ranges(std::slice::from_ref(&binding.owner));
            let ordinary_index = coverage
                .ordinary
                .partition_point(|owner| owner.end <= binding.owner.start);
            let ordinary = coverage.ordinary.get(ordinary_index).is_some_and(|owner| {
                owner.start <= binding.owner.start && binding.owner.end <= owner.end
            });
            if binding.prepaid.is_some() != field
                || !field && !ordinary
                || binding
                    .prepaid
                    .is_some_and(|length| length != payload.bytes.len())
            {
                return Err(OxmlError::InvalidValue(
                    "scalar payload ownership or prepayment is invalid".into(),
                ));
            }
            projected = projected
                .checked_sub(range.len())
                .and_then(|length| length.checked_add(payload.bytes.len()))
                .ok_or_else(|| OxmlError::InvalidValue("imported source length overflow".into()))?;
            let first = coverage
                .fields
                .partition_point(|field| field.end <= range.start);
            for field in coverage.fields[first..]
                .iter()
                .take_while(|field| field.start < range.end)
            {
                let start = field.start.max(range.start);
                let end = field.end.min(range.end);
                if start < end {
                    demand = demand.checked_sub(end - start).ok_or_else(|| {
                        OxmlError::InvalidValue("imported Field removal work overflow".into())
                    })?;
                }
            }
            if binding.prepaid.is_none() {
                for field in payload.current_publication().fields {
                    demand = demand.checked_add(field.len()).ok_or_else(|| {
                        OxmlError::InvalidValue("imported Field output work overflow".into())
                    })?;
                }
            }
            pieces.push((Some(&source), previous..range.start, &[][..]));
            pieces.push((Some(payload), 0..payload.bytes.len(), &[][..]));
            previous = range.end;
        }
        pieces.push((Some(&source), previous..source.bytes.len(), &[][..]));
        let assembly = self.source_assembly(&pieces)?;
        debug_assert_eq!(assembly.length, projected);
        // All validation/map preparation finishes before this reservation or
        // byte growth. No active ambient exemption survives a moved payload.
        let mut copied = assembly.source_map.materialized_publication();
        copied.add_ranges(assembly.coverage.fields.clone());
        // Scalar payloads have generated maps and their own exact prepayment.
        // Only unchanged original Field pieces can add first-copy raw input.
        self.field_work.reserve_source_publication_copy(
            &assembly.source_map,
            &copied.fields,
            demand,
        )?;
        Ok(self.materialize_source(assembly, None))
    }

    /// Copy one actual source occurrence without renewing any input role.
    /// Only positively observed Field intervals supply copied Field demand.
    /// Their complement remains unproved, rather than certified ordinary.
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
            payload: None,
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

    pub fn replace_block(
        &mut self,
        range: std::ops::Range<usize>,
        scope: &std::collections::BTreeMap<String, String>,
        paragraph: Option<usize>,
        placeholder: &str,
        replacement: &str,
    ) -> Result<(Vec<u8>, usize)> {
        let delivered = std::cell::Cell::new(false);
        self.replace_block_with_literal_input(
            range,
            scope,
            paragraph,
            placeholder,
            replacement,
            &delivered,
        )
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

/// Exact capture-only metric. Physical attribute occurrences and pooled
/// namespace facts stay separate so a scoped operation can register origins.
struct OriginalCaptureWork {
    needed: usize,
    attributes: Vec<(std::ops::Range<usize>, usize)>,
    namespaces: Vec<u32>,
}
impl OriginalCaptureWork {
    fn current_bytes(&self, pool: &CaptureDeclarations) -> Result<usize> {
        let mut current = self
            .attributes
            .iter()
            .try_fold(0usize, |sum, (_, bytes)| sum.checked_add(*bytes))
            .ok_or_else(|| OxmlError::InvalidValue("original capture attribute overflow".into()))?;
        for index in &self.namespaces {
            current = current
                .checked_add(2)
                .and_then(|bytes| bytes.checked_add(pool.prefix(*index).len()))
                .and_then(|bytes| bytes.checked_add(pool.namespace(*index).len()))
                .ok_or_else(|| OxmlError::InvalidValue("original capture scope overflow".into()))?;
        }
        Ok(current)
    }
    fn admit(&self, source_bytes: usize, pool: &CaptureDeclarations) -> Result<()> {
        let allowance =
            crate::text::retained_owner_work(source_bytes, &[], self.current_bytes(pool)?)?;
        if self.needed > allowance {
            return Err(OxmlError::InvalidValue(
                "original source capture exceeds admitted source work".into(),
            ));
        }
        Ok(())
    }
}

// Install only existing inherited namespace facts for both real reader routes.
fn inherited_capture_declarations(
    prefixes: &[String],
    pool: &mut CaptureDeclarations,
) -> WordSourceResult<Vec<u32>> {
    let reduced_inherited = reduce_inherited_bindings(prefixes);
    let mut inherited = Vec::with_capacity(reduced_inherited.len());
    for (prefix, namespace) in reduced_inherited {
        inherited.push(
            pool.intern(prefix, namespace)
                .map_err(WordSourceFailure::Resource)?,
        );
    }
    Ok(inherited)
}

/// Aggregate the retained namespace records a capture owner would create from
/// the ORIGINAL input, before any model construction. Physical declarations
/// are pooled once. Owners keep indices. Lengths are resolved from that pool,
/// so a shared URI is not copied once per owner. Duplicate stored declarations
/// still count. The allowance receives each original external fact once.
fn plan_original_capture_records(
    xml: WordXmlInput<'_>,
    prefixes: &[String],
    root_owner: Option<SdtOwner>,
    consumed_owner: Option<&[u8]>,
    pool: &mut CaptureDeclarations,
) -> WordSourceResult<(ConstructorFailures, OriginalCaptureWork)> {
    let inherited = inherited_capture_declarations(prefixes, pool)?;
    plan_capture_with_context(
        xml,
        &inherited,
        prefixes,
        root_owner,
        consumed_owner,
        pool,
        None,
    )
}

fn plan_capture_with_context(
    xml: WordXmlInput<'_>,
    inherited: &[u32],
    prefixes: &[String],
    root_owner: Option<SdtOwner>,
    consumed_owner: Option<&[u8]>,
    pool: &mut CaptureDeclarations,
    source_reader: Option<WordSourceReader>,
) -> WordSourceResult<(ConstructorFailures, OriginalCaptureWork)> {
    let failures = qualify_constructor_failures(
        xml,
        inherited,
        pool,
        root_owner,
        prefixes,
        consumed_owner,
        source_reader,
    )?;
    let namespace_state = &mut CaptureNamespaceState::new(inherited, pool, prefixes);
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut scope: Vec<OriginalCaptureScope> = Vec::new();
    let mut owners: Vec<OriginalCaptureOwner> = Vec::new();
    let mut discarded_index = 0usize;
    let mut part_event_index = 0usize;
    let mut total_needed = 0usize;
    let mut attribute_facts = Vec::new();
    let mut facts: Vec<u32> = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let start_offset = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?;
        match event {
            Event::Start(ref start) | Event::Empty(ref start) => {
                let empty = matches!(event, Event::Empty(_));
                let declared = crate::text::raw_namespace_declarations(start)?;
                let mut declarations = Vec::with_capacity(declared.len());
                for (prefix, namespace) in declared {
                    declarations.push(
                        pool.push(prefix, namespace)
                            .map_err(WordSourceFailure::Resource)?,
                    );
                }
                let part_event = failures
                    .part_events
                    .get(part_event_index)
                    .filter(|fact| fact.start == start_offset);
                if part_event.is_some() {
                    part_event_index += 1;
                }
                namespace_state.enter(&declarations, scope.len(), pool, None);
                if part_event.is_some_and(|fact| matches!(fact.kind, ConstructorKind::PartWrapper))
                {
                    namespace_state
                        .retain_part_declarations(&part_event.expect("part wrapper").updates, pool);
                    declarations.clear();
                }
                let local = start.local_name();
                let name = start.name();
                let name = name.as_ref();
                let element_prefix = name
                    .iter()
                    .position(|byte| *byte == b':')
                    .map_or("", |separator| {
                        std::str::from_utf8(&name[..separator]).unwrap_or("")
                    });
                let word = original_capture_binding(element_prefix, namespace_state)
                    .is_some_and(|(index, _)| pool.namespace(index) == W_NS);
                let direct_run_child = owners
                    .last()
                    .is_some_and(|owner| owner.run && scope.len() == owner.depth + 1)
                    && !scope
                        .last()
                        .is_some_and(|parent| parent.raw || parent.dropped || parent.unconstructed);
                let action = crate::text::RunChildAction::for_names(
                    local.as_ref(),
                    empty,
                    original_capture_word_name(name, false, namespace_state, pool),
                    false,
                );
                let special_typed =
                    if direct_run_child && action == crate::text::RunChildAction::Special {
                        let aliases = constructor_attribute_prefixes(start, namespace_state, pool)?;
                        crate::text::RunChildPlan::read(start, &aliases, action)?
                            .leaf
                            .is_some()
                    } else {
                        false
                    };
                let comment_raw = if direct_run_child
                    && action == crate::text::RunChildAction::CommentReference
                {
                    let aliases = constructor_attribute_prefixes(start, namespace_state, pool)?;
                    crate::text::comment_reference_has_payload_attributes(start, &aliases)
                        || (!empty && original_comment_reference_has_payload(*reader.get_ref())?)
                } else {
                    false
                };
                let modeled =
                    direct_run_child && !action.retains_raw(special_typed) && !comment_raw;
                let mut class = classify_original_capture(
                    empty,
                    word,
                    local.as_ref(),
                    modeled,
                    &scope,
                    &owners,
                );
                let raw_action = failures
                    .raw_subtrees
                    .binary_search_by_key(&start_offset, |fact| fact.span.start)
                    .ok()
                    .map(|index| &failures.raw_subtrees[index]);
                if raw_action.is_some_and(|fact| fact.control) {
                    class.raw = true;
                }
                while failures
                    .discarded
                    .get(discarded_index)
                    .is_some_and(|range| range.end <= start_offset)
                {
                    discarded_index += 1;
                }
                let discarded = failures
                    .discarded
                    .get(discarded_index)
                    .is_some_and(|range| range.contains(&start_offset));
                let unconstructed = scope.last().is_some_and(|parent| parent.unconstructed)
                    || raw_action.is_some_and(|fact| !fact.control)
                    || discarded;
                if discarded {
                    class.owner = false;
                }
                if class.owner {
                    let (record, attribute_bytes) =
                        original_capture_root_attributes(start, namespace_state, pool)?;
                    total_needed = total_needed.checked_add(record).ok_or_else(|| {
                        WordSourceFailure::resource("original capture work overflow".into())
                    })?;
                    if attribute_bytes > 0 {
                        attribute_facts.push((
                            start_offset..reader.buffer_position() as usize,
                            attribute_bytes,
                        ));
                    }
                    owners.push(OriginalCaptureOwner {
                        depth: scope.len(),
                        run: local.as_ref() == b"r",
                        shared: Vec::new(),
                        local: Vec::new(),
                        raw_copies: Vec::new(),
                        open_raw: Vec::new(),
                        open_comment: None,
                    });
                } else if let Some(owner) = owners.last_mut()
                    && !class.dropped
                    && (class.raw || class.inside_typed)
                {
                    if class.typed_root {
                        owner.open_raw.push(Vec::new());
                    }
                    if comment_raw {
                        owner.open_comment = Some((scope.len(), Vec::new()));
                    }
                    let used = original_capture_event_prefixes(start, namespace_state, pool)?;
                    for prefix in used {
                        let Some((index, depth)) =
                            original_capture_binding(&prefix, namespace_state)
                        else {
                            continue;
                        };
                        if let Some((root_depth, copy)) = &mut owner.open_comment {
                            if depth < *root_depth {
                                capture_index_push(copy, index);
                            }
                        } else {
                            charge_original_capture_binding(
                                owner,
                                index,
                                depth,
                                pool,
                                class.inside_typed,
                                namespace_state,
                            );
                        }
                    }
                    if empty
                        && comment_raw
                        && let Some((_, copy)) = owner.open_comment.take()
                    {
                        owner.raw_copies.push(copy);
                    }
                }
                if !empty {
                    let parent = scope.last();
                    let ruby_opaque = parent.is_some_and(|parent| parent.ruby_opaque)
                        || (word
                            && matches!(local.as_ref(), b"ruby" | b"rt" | b"rubyBase")
                            && capture_non_namespace_attribute(start));
                    let paragraph_result_root =
                        failures.paragraph_result_root == Some(start_offset);
                    let ordered_revision_root =
                        failures.ordered_revision_root == Some(start_offset);
                    let parent_paragraph = parent.is_some_and(|parent| parent.paragraph);
                    let revision =
                        word && matches!(local.as_ref(), b"ins" | b"del" | b"moveFrom" | b"moveTo");
                    let revision_runs = if ordered_revision_root {
                        true
                    } else if revision && !ruby_opaque {
                        capture_revision_opens_runs(start, parent_paragraph, namespace_state, pool)?
                    } else {
                        false
                    };
                    let parsed_hyperlink = local.as_ref() == b"hyperlink"
                        && (scope.is_empty()
                            || parent.is_some_and(|parent| parent.constructs_hyperlinks));
                    // A paragraph retains these bytes, but accepted reads also
                    // parse their contents as an ordinary synthetic paragraph.
                    // Keep that dispatch inside the original reservation. Raw
                    // wrappers inside a run or an opaque payload do not expand.
                    let wrapper_paragraph = word
                        && matches!(local.as_ref(), b"smartTag" | b"customXml")
                        && parent.is_some_and(|parent| {
                            parent.paragraph && !parent.raw && !parent.dropped
                        });
                    let inline_drawing = parent.is_some_and(|parent| parent.inline_drawing)
                        || (class.typed_root
                            && crate::drawing::DrawingContainerKind::for_name(
                                local.as_ref(),
                                empty,
                            ) == Some(crate::drawing::DrawingContainerKind::Inline));
                    let text_span = parent.is_some_and(|parent| parent.text_span)
                        || (direct_run_child
                            && matches!(
                                action,
                                crate::text::RunChildAction::Text
                                    | crate::text::RunChildAction::DeletedText
                            ));
                    let constructs_runs = !text_span
                        && !inline_drawing
                        && !ruby_opaque
                        && !unconstructed
                        && word
                        && (matches!(
                            local.as_ref(),
                            b"p" | b"rt" | b"rubyBase" | b"fldSimple" | b"sdtContent"
                        ) || wrapper_paragraph
                            || parsed_hyperlink
                            || revision_runs);
                    let constructs_hyperlinks = !text_span
                        && !inline_drawing
                        && !ruby_opaque
                        && !unconstructed
                        && word
                        && (local.as_ref() == b"p"
                            || wrapper_paragraph
                            || revision_runs
                            || paragraph_result_root);
                    scope.push(OriginalCaptureScope {
                        declarations,
                        raw: class.raw,
                        typed_drawing: class.inside_typed,
                        inline_drawing,
                        text_span,
                        drawing_root: class.drawing_root,
                        dropped: class.dropped,
                        raw_copy: class.typed_root,
                        constructs_runs,
                        constructs_hyperlinks,
                        paragraph: !text_span
                            && !inline_drawing
                            && word
                            && (local.as_ref() == b"p"
                                || wrapper_paragraph
                                || paragraph_result_root)
                            && !ruby_opaque
                            && !unconstructed,
                        ruby_opaque,
                        unconstructed,
                        comment_root: comment_raw,
                    });
                } else {
                    if let Some(owner) = owners.pop_if(|owner| owner.depth == scope.len()) {
                        finish_original_capture_owner(owner, &mut total_needed, &mut facts, pool)
                            .map_err(WordSourceFailure::Resource)?;
                    }
                    namespace_state.leave(&declarations, pool);
                }
            }
            Event::End(_) => {
                if scope.last().is_some_and(|frame| frame.comment_root)
                    && let Some(owner) = owners.last_mut()
                    && let Some((_, copy)) = owner.open_comment.take()
                {
                    owner.raw_copies.push(copy);
                }
                if scope.last().is_some_and(|frame| frame.raw_copy)
                    && let Some(owner) = owners.last_mut()
                    && let Some(copy) = owner.open_raw.pop()
                    && !copy.is_empty()
                {
                    owner.raw_copies.push(copy);
                }
                if let Some(scope) = scope.pop() {
                    namespace_state.leave(&scope.declarations, pool);
                }
                if let Some(owner) = owners.pop_if(|owner| owner.depth == scope.len()) {
                    finish_original_capture_owner(owner, &mut total_needed, &mut facts, pool)
                        .map_err(WordSourceFailure::Resource)?;
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok((
        failures,
        OriginalCaptureWork {
            needed: total_needed,
            attributes: attribute_facts,
            namespaces: facts,
        },
    ))
}

fn admit_original_capture_records(
    xml: WordXmlInput<'_>,
    prefixes: &[String],
    root_owner: Option<SdtOwner>,
    consumed_owner: Option<&[u8]>,
) -> WordSourceResult<ConstructorFailures> {
    let mut pool = CaptureDeclarations::default();
    let (constructors, work) =
        plan_original_capture_records(xml, prefixes, root_owner, consumed_owner, &mut pool)?;
    work.admit(xml.len().map_err(WordSourceFailure::Resource)?, &pool)
        .map_err(WordSourceFailure::Resource)?;
    Ok(constructors)
}

/// Admit each independent input before recursive construction. Internal parser
/// routes consume only bytes derived from that admitted input, without ambient
/// thread state or a second whole-source scan at each nested owner.
pub(crate) fn admit_word_xml(
    xml: &[u8],
    prefixes: &[String],
    consumed_owner: Option<&[u8]>,
) -> Result<(usize, usize)> {
    Ok(admit_word_source(xml, prefixes, consumed_owner, None)?.work)
}

/// Admit one original operation and retain its constructor occurrence facts.
/// Prefixes are the actual inherited scope. Complete readers that learn their
/// scope from root declarations pass an empty slice, not a conventional alias.
pub(crate) fn admit_word_source(
    xml: &[u8],
    prefixes: &[String],
    consumed_owner: Option<&[u8]>,
    root_owner: Option<SdtOwner>,
) -> Result<AdmittedWordSource> {
    Ok(admit_word_input(
        WordXmlInput::Contiguous(xml),
        prefixes,
        consumed_owner,
        root_owner,
    )?)
}

/// Admit one concrete reader through qualification, capture and weighted
/// admission. Part dispatch keeps persistent wrapper bindings. The standalone
/// paragraph role also collects inert success-only selector occurrences.
pub(crate) fn admit_reader_source(
    xml: &[u8],
    reader: WordSourceReader,
) -> Result<AdmittedWordSource> {
    let mut pool = CaptureDeclarations::default();
    let (constructors, capture) = plan_capture_with_context(
        WordXmlInput::Contiguous(xml),
        &[],
        &[],
        None,
        None,
        &mut pool,
        Some(reader),
    )?;
    capture.admit(xml.len(), &pool)?;
    Ok(admit_word_input_classified(
        WordXmlInput::Contiguous(xml),
        &[],
        None,
        None,
        std::sync::Arc::new(constructors),
    )?)
}

/// Raw editing preserves ordinary input refusal, but must surface a resource
/// failure before constructing or publishing any part of its candidate.
pub(crate) fn admit_raw_word_source(
    xml: &[u8],
    prefixes: &[String],
    root_owner: Option<SdtOwner>,
) -> Result<Option<AdmittedWordSource>> {
    match admit_word_input(WordXmlInput::Contiguous(xml), prefixes, None, root_owner) {
        Ok(admitted) => Ok(Some(admitted)),
        Err(WordSourceFailure::Input(_)) => Ok(None),
        Err(WordSourceFailure::Resource(error)) => Err(error),
    }
}

/// Preserve the actual source admission verdict across legacy parser adapters.
/// Ordinary input failures never mark this operation's resource receipt.
pub(crate) fn admit_word_field_input(
    xml: WordXmlInput<'_>,
    prefixes: &[String],
    registry: Option<&std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>>>,
) -> Result<(usize, usize)> {
    admit_word_input(xml, prefixes, None, None)
        .map(|admitted| admitted.work)
        .map_err(|failure| record_word_source_failure(failure, registry))
}

/// The actual revision factory's root/header role. Its complete existing
/// capture and weighted admission use the same constructor facts before model
/// construction, including ordinary raw fallback. No XML wrapper is injected.
pub(crate) fn admit_revision_field_input(
    xml: &[u8],
    prefixes: &[String],
    require_author: bool,
    registry: Option<&std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>>>,
) -> Result<(usize, usize)> {
    admit_actual_field_reader(
        xml,
        prefixes,
        WordSourceReader::OrderedRevision { require_author },
        registry,
    )
}

/// The same actual paragraph reader is used for ordered and nested-run caches.
/// Its real producer supplies the already-selected root opening and body bounds.
pub(crate) fn admit_paragraph_result_field_input(
    xml: &[u8],
    prefixes: &[String],
    opening_end: usize,
    result: std::ops::Range<usize>,
    registry: Option<&std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>>>,
) -> Result<(usize, usize)> {
    if result.start > result.end || result.end > xml.len() || result.start < opening_end {
        // Unknown legacy range/EOF shapes retain their existing admission.
        return admit_word_field_input(WordXmlInput::Contiguous(xml), prefixes, registry);
    }
    admit_actual_field_reader(
        xml,
        prefixes,
        WordSourceReader::SimpleParagraph {
            opening_end,
            result_start: result.start,
            result_end: result.end,
        },
        registry,
    )
}

fn admit_actual_field_reader(
    xml: &[u8],
    prefixes: &[String],
    source_reader: WordSourceReader,
    registry: Option<&std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>>>,
) -> Result<(usize, usize)> {
    let admitted = (|| -> WordSourceResult<AdmittedWordSource> {
        let mut pool = CaptureDeclarations::default();
        let inherited = inherited_capture_declarations(prefixes, &mut pool)?;
        let (constructors, capture) = plan_capture_with_context(
            WordXmlInput::Contiguous(xml),
            &inherited,
            prefixes,
            None,
            None,
            &mut pool,
            Some(source_reader),
        )?;
        capture
            .admit(xml.len(), &pool)
            .map_err(WordSourceFailure::Resource)?;
        admit_word_input_classified(
            WordXmlInput::Contiguous(xml),
            prefixes,
            None,
            None,
            std::sync::Arc::new(constructors),
        )
    })();
    admitted
        .map(|admitted| admitted.work)
        .map_err(|failure| record_word_source_failure(failure, registry))
}

fn record_word_source_failure(
    failure: WordSourceFailure,
    registry: Option<&std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>>>,
) -> OxmlError {
    match failure {
        WordSourceFailure::Input(error) => error,
        WordSourceFailure::Resource(error) => {
            if let Some(registry) = registry {
                registry
                    .lock()
                    .expect("field origin registry poisoned")
                    .record_source_resource_failure(&error);
            }
            error
        }
    }
}

fn admit_word_input(
    xml: WordXmlInput<'_>,
    prefixes: &[String],
    consumed_owner: Option<&[u8]>,
    root_owner: Option<SdtOwner>,
) -> WordSourceResult<AdmittedWordSource> {
    let constructors = admit_original_capture_records(xml, prefixes, root_owner, consumed_owner)?;
    admit_word_input_classified(
        xml,
        prefixes,
        consumed_owner,
        root_owner,
        std::sync::Arc::new(constructors),
    )
}

pub(crate) fn admit_word_input_with_constructors(
    xml: WordXmlInput<'_>,
    prefixes: &[String],
    consumed_owner: Option<&[u8]>,
    root_owner: Option<SdtOwner>,
    constructors: ConstructorFailures,
) -> Result<AdmittedWordSource> {
    Ok(admit_word_input_classified(
        xml,
        prefixes,
        consumed_owner,
        root_owner,
        std::sync::Arc::new(constructors),
    )?)
}

fn admission_envelope(prefixes: &[String]) -> WordSourceResult<(Vec<u8>, usize)> {
    // The wrapper supplies only the inherited bindings the actual reader
    // already uses. Declarations in the input still shadow this scope.
    let context_bytes = prefixes
        .iter()
        .try_fold(0usize, |sum, prefix| sum.checked_add(prefix.len()))
        .ok_or_else(|| WordSourceFailure::resource("Word source context byte overflow".into()))?;
    let mut bindings: Vec<(&str, &str)> = prefixes
        .iter()
        .filter_map(|value| split_namespace_binding(value))
        .collect();
    // Context multiplicity belongs to revision identity, not XML attributes
    // on this synthetic envelope. Keep the last identical binding so repeated
    // metadata cannot move an earlier binding past a conflicting later one.
    // Conflicting pairs and the stored input context remain untouched.
    let mut seen = std::collections::HashSet::new();
    bindings.reverse();
    bindings.retain(|binding| seen.insert(*binding));
    bindings.reverse();
    for prefix in prefixes.iter().filter(|prefix| !prefix.starts_with('\0')) {
        if !bindings.iter().any(|(name, _)| *name == prefix.as_str()) {
            bindings.push((prefix.as_str(), W_NS));
        }
    }
    let effective_context_bytes = bindings
        .iter()
        .try_fold(0usize, |sum, (prefix, namespace)| {
            let marker = if *namespace == W_NS { prefix.len() } else { 0 };
            sum.checked_add(prefix.len())
                .and_then(|sum| sum.checked_add(namespace.len()))
                .and_then(|sum| sum.checked_add(2))
                .and_then(|sum| sum.checked_add(marker))
        })
        .ok_or_else(|| {
            WordSourceFailure::resource("Word effective context byte overflow".into())
        })?;
    let duplicate_context_bytes = context_bytes.saturating_sub(effective_context_bytes);
    let mut source = b"<rdocxAdmission".to_vec();
    for (prefix, namespace) in bindings {
        source.extend_from_slice(b" xmlns");
        if !prefix.is_empty() {
            source.push(b':');
            source.extend_from_slice(prefix.as_bytes());
        }
        source.extend_from_slice(b"=\"");
        source.extend_from_slice(quick_xml::escape::escape(namespace).as_bytes());
        source.push(b'"');
    }
    source.push(b'>');
    Ok((source, duplicate_context_bytes))
}

fn admit_word_input_classified(
    xml: WordXmlInput<'_>,
    prefixes: &[String],
    consumed_owner: Option<&[u8]>,
    root_owner: Option<SdtOwner>,
    constructors: std::sync::Arc<ConstructorFailures>,
) -> WordSourceResult<AdmittedWordSource> {
    debug_assert!(consumed_owner.is_none_or(|owner| matches!(owner, b"p" | b"r")));
    let (source, duplicate_context_bytes) = admission_envelope(prefixes)?;
    let source_start = source.len();
    let mut source_chunks = vec![source.as_slice()];
    source_chunks.extend(xml.chunks());
    source_chunks.push(b"</rdocxAdmission>");
    let mut reader = Reader::from_reader(WordXmlInput::pieces(&source_chunks));
    let mut stack: Vec<WordAdmissionFrame> = Vec::new();
    let mut discarded_index = 0usize;
    let mut part_event_index = 0usize;
    let mut controls = 0usize;
    let mut path_cost = 0usize;
    let mut copy_nodes = 0usize;
    let mut copy_bytes = 0usize;
    let mut namespace_depths: std::collections::HashMap<Vec<u8>, WordNamespaceBinding> =
        std::collections::HashMap::new();
    let mut copy_scope_weight: Option<usize> = None;
    let mut buffer = Vec::new();
    loop {
        let fact_start = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?;
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let local = element.local_name();
                let parent_admitted = stack.last().is_none_or(|frame| frame.admitted);
                let parent_owner = stack.last().and_then(|frame| frame.owner);
                let in_control_content = stack
                    .last()
                    .is_some_and(|frame| frame.local.as_ref() == b"sdtContent");
                let part_event = constructors
                    .part_events
                    .get(part_event_index)
                    .filter(|fact| fact_start.checked_sub(source_start) == Some(fact.start));
                if part_event.is_some() {
                    part_event_index += 1;
                }
                let part_wrapper = part_event
                    .is_some_and(|fact| matches!(fact.kind, ConstructorKind::PartWrapper));
                let mut namespace_undo = Vec::new();
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(OxmlError::from)?;
                    let key = attribute.key.as_ref();
                    let prefix = if key == b"xmlns" {
                        Some(b"".as_slice())
                    } else {
                        key.strip_prefix(b"xmlns:")
                    };
                    if let Some(prefix) = prefix {
                        let prefix = prefix.to_vec();
                        let uri = attribute.value.as_ref();
                        const XML_URI: &[u8] = b"http://www.w3.org/XML/1998/namespace";
                        if prefix == b"xmlns"
                            || prefix == b"xml" && uri != XML_URI
                            || prefix != b"xml" && uri == XML_URI
                            || uri == b"http://www.w3.org/2000/xmlns/"
                        {
                            return Err(WordSourceFailure::Input(OxmlError::InvalidValue(
                                "reserved XML namespace binding".into(),
                            )));
                        }
                        copy_scope_weight = None;
                        let previous = namespace_depths.insert(
                            prefix.clone(),
                            WordNamespaceBinding {
                                uri: uri.to_vec(),
                                depth: if part_wrapper { 0 } else { stack.len() },
                            },
                        );
                        namespace_undo.push((prefix, previous));
                    }
                }
                if part_event.is_some_and(|fact| matches!(fact.kind, ConstructorKind::PartWrapper))
                {
                    namespace_undo.clear();
                }
                let name = element.name();
                let name = name.as_ref();
                let prefix = name
                    .iter()
                    .position(|byte| *byte == b':')
                    .map_or(b"".as_slice(), |separator| &name[..separator]);
                let binding = namespace_depths.get(prefix);
                let namespace_uri =
                    binding.map_or(b"".as_slice(), |binding| binding.uri.as_slice());
                let virtual_owner = stack.is_empty() && consumed_owner.is_some();
                let fallback_run = consumed_owner == Some(b"p".as_slice())
                    && binding.is_none()
                    && local.as_ref() == b"r"
                    && stack.last().is_some_and(|parent| {
                        parent.admitted && parent.word && parent.local.as_ref() == b"p"
                    });
                let legacy_run_prefix = if fallback_run {
                    Some(prefix.to_vec())
                } else {
                    stack
                        .last()
                        .and_then(|parent| parent.legacy_run_prefix.clone())
                };
                let legacy_word = binding.is_none() && legacy_run_prefix.as_deref() == Some(prefix);
                let word = virtual_owner
                    || legacy_word
                    || !stack.is_empty() && decoded_namespace_is(namespace_uri, W_NS);
                // Only modeled owner containers admit recursive construction.
                // A qualified-looking descendant inside an opaque extension or
                // foreign rebinding remains opaque, including its control names.
                let owned_control_child = !in_control_content
                    || !word
                    || local.as_ref() == b"sdt"
                    || parent_owner.is_some_and(|owner: SdtOwner| owner.owns(local.as_ref()));
                let qualified_control_owner =
                    !word || local.as_ref() != b"sdt" || parent_owner.is_some() || stack.len() == 1;
                let mut admitted = parent_admitted && owned_control_child && qualified_control_owner
                    && ((stack.is_empty() && local.as_ref() == b"rdocxAdmission")
                        || (word && matches!(local.as_ref(),
                            b"document" | b"body" | b"p" | b"r" | b"tbl" | b"tr" | b"tc"
                            | b"sdt" | b"sdtContent" | b"hdr" | b"ftr" | b"comments" | b"comment"
                            | b"footnotes" | b"footnote" | b"endnotes" | b"endnote"
                            | b"glossaryDocument" | b"docParts" | b"docPart" | b"docPartBody"
                            | b"hyperlink" | b"fldSimple" | b"ins" | b"del" | b"moveFrom" | b"moveTo"
                            | b"drawing" | b"pict" | b"object" | b"txbxContent" | b"ruby" | b"rt" | b"rubyBase"))
                        || (decoded_namespace_is(namespace_uri, crate::namespace::MC_NS)
                            && matches!(local.as_ref(), b"AlternateContent" | b"Choice" | b"Fallback"))
                        || (namespace_uri == b"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
                            // Inline shape text is opaque. Only the anchor
                            // parser constructs a recursive Word text body.
                            && local.as_ref() == b"anchor")
                        || (namespace_uri == b"http://schemas.openxmlformats.org/drawingml/2006/main"
                            && matches!(local.as_ref(), b"graphic" | b"graphicData"))
                        || (namespace_uri == b"http://schemas.microsoft.com/office/word/2010/wordprocessingShape"
                            && matches!(local.as_ref(), b"wsp" | b"txbx"))
                        || (namespace_uri == b"urn:schemas-microsoft-com:vml"
                            && matches!(local.as_ref(), b"shape" | b"group" | b"textbox" | b"rect" | b"roundrect" | b"oval" | b"line" | b"polyline" | b"curve" | b"arc")));
                if let Some(fact) = part_event {
                    admitted = !matches!(fact.kind, ConstructorKind::Unconstructed);
                } else if word
                    && let Some(parent) = stack.last()
                    && parent.word
                {
                    let parent_name = if parent.paragraph_reader_root {
                        b"p".as_slice()
                    } else {
                        parent.local.as_ref()
                    };
                    admitted &= admitted_word_child(parent_name, local.as_ref())
                        && !(parent.ordered_revision_root && local.as_ref() == b"fldSimple");
                }
                // A qualified drawing owner is projected only from an actual
                // run drawing, or as an independently admitted drawing root.
                if namespace_uri
                    == b"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
                {
                    admitted &= stack.len() == 1
                        || stack.last().is_some_and(|parent| {
                            parent.word && parent.local.as_ref() == b"drawing"
                        });
                }
                let mc = !stack.is_empty()
                    && decoded_namespace_is(namespace_uri, crate::namespace::MC_NS);
                let mut mc_capture_depth = stack.last().and_then(|frame| frame.mc_capture_depth);
                let mut qualified_drawing_depth = None;
                let vml = !stack.is_empty() && namespace_uri == b"urn:schemas-microsoft-com:vml";
                if mc && let Some(parent) = stack.last() {
                    admitted &= if local.as_ref() == b"AlternateContent" {
                        (stack.len() == 1 || parent.word && parent.local.as_ref() == b"r")
                            && captured_namespace_is(binding, stack.len(), crate::namespace::MC_NS)
                    } else {
                        parent.mc
                            && parent.local.as_ref() == b"AlternateContent"
                            && mc_capture_depth.is_some_and(|depth| {
                                captured_namespace_is(binding, depth, crate::namespace::MC_NS)
                            })
                    };
                    if local.as_ref() == b"AlternateContent" {
                        mc_capture_depth = admitted.then_some(stack.len());
                    }
                }
                if word
                    && let Some(parent) = stack.last()
                    && parent.mc
                {
                    admitted &= match (parent.local.as_ref(), local.as_ref()) {
                        (b"Choice", b"drawing") => mc_capture_depth.is_some_and(|depth| {
                            captured_namespace_is(binding, depth, W_NS) || legacy_word
                        }),
                        (b"Choice" | b"Fallback", b"pict") => true,
                        _ => false,
                    };
                    if admitted && local.as_ref() == b"drawing" {
                        qualified_drawing_depth = Some(stack.len());
                    }
                }
                if vml && let Some(parent) = stack.last() {
                    let shape = matches!(
                        local.as_ref(),
                        b"shape"
                            | b"group"
                            | b"rect"
                            | b"roundrect"
                            | b"oval"
                            | b"line"
                            | b"polyline"
                            | b"curve"
                            | b"arc"
                    );
                    admitted &= parent.word
                        && matches!(parent.local.as_ref(), b"pict" | b"object")
                        && shape
                        || parent.vml && parent.local.as_ref() == b"group" && shape
                        || parent.vml
                            && matches!(
                                parent.local.as_ref(),
                                b"shape"
                                    | b"rect"
                                    | b"roundrect"
                                    | b"oval"
                                    | b"line"
                                    | b"polyline"
                                    | b"curve"
                                    | b"arc"
                            )
                            && local.as_ref() == b"textbox";
                }
                if word && matches!(local.as_ref(), b"ruby" | b"rt" | b"rubyBase") {
                    // The native ruby parser preserves the entire owner as
                    // opaque when one of these attribute-free owners has
                    // any non-namespace attribute.
                    for attribute in element.attributes() {
                        let attribute = attribute.map_err(OxmlError::from)?;
                        let key = attribute.key.as_ref();
                        if key != b"xmlns" && !key.starts_with(b"xmlns:") {
                            admitted = false;
                        }
                    }
                }
                let parent_anchor = stack.last().and_then(|frame| frame.anchor_depth);
                if parent_anchor.is_none()
                    && matches!(
                        namespace_uri,
                        b"http://schemas.openxmlformats.org/drawingml/2006/main"
                            | b"http://schemas.microsoft.com/office/word/2010/wordprocessingShape"
                    )
                {
                    admitted = false;
                }
                let direct_drawing = stack.last().is_some_and(|frame| {
                    frame.word && frame.local.as_ref() == b"drawing" && frame.admitted
                });
                // Direct CT_Drawing entries choose anchor by local name after
                // their qualified Word drawing owner has been established.
                // AlternateContent has its separate qualified owner admission.
                if direct_drawing && local.as_ref() == b"anchor" {
                    admitted = stack.last().unwrap().qualified_drawing_depth.is_none_or(|depth|
                        captured_namespace_is(binding, depth, "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"));
                }
                let mut anchor_depth =
                    (admitted && local.as_ref() == b"anchor").then_some(stack.len());
                let typed_anchor_body = parent_anchor.is_some_and(|depth| {
                    local.as_ref() == b"txbxContent"
                        && (captured_namespace_is(binding, depth, W_NS) || legacy_word)
                });
                if let Some(parent_anchor) = parent_anchor {
                    // The actual anchor reader traverses unknown wrappers and
                    // any graphicData URI. These captured branches are the
                    // exceptions it skips without constructing a Word body.
                    admitted = !matches!(
                        local.as_ref(),
                        b"srcRect"
                            | b"positionH"
                            | b"positionV"
                            | b"spPr"
                            | b"blip"
                            | b"wrapNone"
                            | b"wrapSquare"
                            | b"wrapTight"
                            | b"wrapThrough"
                            | b"wrapTopAndBottom"
                    ) && !(captured_namespace_is(
                        binding,
                        parent_anchor,
                        "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing",
                    ) && local.as_ref() == b"docPr");
                    anchor_depth = (admitted && !typed_anchor_body).then_some(parent_anchor);
                }
                if fact_start.checked_sub(source_start).is_some_and(|start| {
                    while constructors
                        .discarded
                        .get(discarded_index)
                        .is_some_and(|range| range.end <= start)
                    {
                        discarded_index += 1;
                    }
                    constructors
                        .discarded
                        .get(discarded_index)
                        .is_some_and(|range| range.contains(&start))
                        || constructors
                            .raw_subtrees
                            .binary_search_by_key(&start, |fact| fact.span.start)
                            .is_ok()
                }) {
                    admitted = false;
                    anchor_depth = None;
                }
                let control =
                    admitted && anchor_depth.is_none() && word && local.as_ref() == b"sdt";
                let typed_vml_body = admitted
                    && word
                    && local.as_ref() == b"txbxContent"
                    && stack
                        .last()
                        .is_some_and(|frame| frame.local.as_ref() == b"textbox");
                let textbox = matches!(event, Event::Start(_))
                    && admitted
                    && (typed_anchor_body || typed_vml_body);
                // A field gains an ordered owner only when an admitted result
                // revision/control is encountered. Pure old field chains keep
                // their previous admission limit. Stop at a real run/body slot.
                if admitted
                    && anchor_depth.is_none()
                    && word
                    && matches!(
                        local.as_ref(),
                        b"sdt" | b"ins" | b"del" | b"moveFrom" | b"moveTo"
                    )
                {
                    for frame in stack.iter_mut().rev() {
                        if !frame.admitted
                            || !frame.word
                            || matches!(frame.local.as_ref(), b"r" | b"p" | b"body" | b"tc")
                        {
                            break;
                        }
                        if frame.local.as_ref() == b"fldSimple" && !frame.ordered_field_charged {
                            path_cost = word_owner_cost(path_cost, 1)
                                .map_err(WordSourceFailure::Resource)?;
                            frame.cost += 1;
                            frame.ordered_field_charged = true;
                        }
                    }
                }
                let cost = usize::from(control) + 16 * usize::from(textbox);
                if control && controls == 256 {
                    return Err(WordSourceFailure::resource(
                        "content-control nesting exceeds inclusive limit 256".into(),
                    ));
                }
                let next_cost =
                    word_owner_cost(path_cost, cost).map_err(WordSourceFailure::Resource)?;
                // Keep effective modeled table depth across controls and
                // captured typed text bodies, whose recursive parser entries
                // otherwise begin with a fresh local table counter.
                let table_depth = stack.last().map_or(0, |frame| frame.table_depth);
                let table_depth =
                    if admitted && anchor_depth.is_none() && word && local.as_ref() == b"tbl" {
                        if table_depth >= crate::table::MAX_RECOGNIZED_TABLE_NESTING {
                            return Err(WordSourceFailure::resource(
                                "recognized model nesting exceeds table limit".into(),
                            ));
                        }
                        table_depth + 1
                    } else {
                        table_depth
                    };
                let copy_depth = stack.last().map_or(0usize, |frame| frame.copy_depth);
                let copy_owner = admitted
                    && word
                    && matches!(
                        local.as_ref(),
                        b"p" | b"r"
                            | b"fldSimple"
                            | b"sdt"
                            | b"ins"
                            | b"del"
                            | b"moveFrom"
                            | b"moveTo"
                            | b"tbl"
                            | b"tr"
                            | b"tc"
                    );
                // A retained opaque child is one physical fact. Its descendants
                // do not become separate modeled-copy owners through this budget.
                let copy_fact =
                    copy_owner || stack.last().is_some_and(|frame| frame.admitted) && !admitted;
                let copy_scope_bytes = if let Some(bytes) = copy_scope_weight {
                    bytes
                } else {
                    let bytes = namespace_depths
                        .iter()
                        .try_fold(0usize, |sum, (prefix, binding)| {
                            let marker = if decoded_namespace_is(&binding.uri, W_NS) {
                                prefix.len()
                            } else {
                                0
                            };
                            sum.checked_add(prefix.len())
                                .and_then(|sum| sum.checked_add(binding.uri.len()))
                                .and_then(|sum| sum.checked_add(2))
                                .and_then(|sum| sum.checked_add(marker))
                        })
                        .ok_or_else(|| {
                            WordSourceFailure::resource("Word local copy context overflow".into())
                        })?;
                    copy_scope_weight = Some(bytes);
                    bytes
                };
                let next_copy_depth =
                    copy_depth
                        .checked_add(usize::from(copy_owner))
                        .ok_or_else(|| {
                            WordSourceFailure::resource(
                                "Word source-copy ownership overflow".into(),
                            )
                        })?;
                if copy_fact && matches!(event, Event::Empty(_)) {
                    let visits = copy_depth.checked_add(1).ok_or_else(|| {
                        WordSourceFailure::resource("Word source-copy visit overflow".into())
                    })?;
                    copy_nodes = copy_nodes.checked_add(visits).ok_or_else(|| {
                        WordSourceFailure::resource("Word source-copy visit overflow".into())
                    })?;
                    let bytes = (reader.buffer_position() as usize - fact_start)
                        .checked_add(copy_scope_bytes)
                        .and_then(|bytes| bytes.checked_mul(visits))
                        .ok_or_else(|| {
                            WordSourceFailure::resource("Word source-copy byte overflow".into())
                        })?;
                    copy_bytes = copy_bytes.checked_add(bytes).ok_or_else(|| {
                        WordSourceFailure::resource("Word source-copy byte overflow".into())
                    })?;
                }
                if matches!(event, Event::Start(_)) {
                    let owner = if !admitted || anchor_depth.is_some() {
                        None
                    } else if word {
                        match consumed_owner
                            .filter(|_| virtual_owner)
                            .unwrap_or(local.as_ref())
                        {
                            b"p" | b"r" | b"hyperlink" | b"fldSimple" => Some(SdtOwner::Inline),
                            b"body" | b"hdr" | b"ftr" | b"comment" | b"footnote" | b"endnote"
                            | b"docPartBody" | b"txbxContent" => Some(SdtOwner::Body),
                            b"tbl" => Some(SdtOwner::Table),
                            b"tr" => Some(SdtOwner::Row),
                            b"tc" => Some(SdtOwner::Cell),
                            b"sdt" => Some(
                                parent_owner
                                    .or(root_owner.filter(|_| stack.len() == 1))
                                    .unwrap_or(SdtOwner::Standalone),
                            ),
                            b"sdtContent" => parent_owner,
                            b"ins" | b"del" | b"moveFrom" | b"moveTo" => parent_owner
                                .or_else(|| (stack.len() == 1).then_some(SdtOwner::Inline)),
                            _ => None,
                        }
                    } else if namespace_uri == crate::namespace::MC_NS.as_bytes() {
                        parent_owner
                    } else {
                        None
                    };
                    stack.push(WordAdmissionFrame {
                        fact_start,
                        paragraph_reader_root: fact_start.checked_sub(source_start).is_some_and(
                            |start| {
                                constructors.paragraph_result_root == Some(start)
                                    || constructors.ordered_revision_root == Some(start)
                            },
                        ),
                        ordered_revision_root: fact_start
                            .checked_sub(source_start)
                            .is_some_and(|start| constructors.ordered_revision_root == Some(start)),
                        copy_fact,
                        copy_depth: next_copy_depth,
                        copy_scope_bytes,
                        control,
                        ordered_field_charged: false,
                        admitted,
                        word,
                        legacy_run_prefix: admitted.then_some(legacy_run_prefix).flatten(),
                        mc,
                        vml,
                        owner,
                        anchor_depth,
                        namespace_undo,
                        mc_capture_depth,
                        qualified_drawing_depth,
                        cost,
                        table_depth,
                        local: {
                            let value = consumed_owner
                                .filter(|_| virtual_owner)
                                .unwrap_or(local.as_ref());
                            match value {
                                b"p" => std::borrow::Cow::Borrowed(b"p".as_slice()),
                                b"r" => std::borrow::Cow::Borrowed(b"r".as_slice()),
                                b"rPr" => std::borrow::Cow::Borrowed(b"rPr".as_slice()),
                                b"t" => std::borrow::Cow::Borrowed(b"t".as_slice()),
                                _ => std::borrow::Cow::Owned(value.to_vec()),
                            }
                        },
                    });
                    controls += usize::from(control);
                    path_cost = next_cost;
                } else {
                    restore_namespace_depths(
                        &mut namespace_depths,
                        namespace_undo,
                        &mut copy_scope_weight,
                    );
                }
            }
            Event::End(_) => {
                if let Some(frame) = stack.pop() {
                    if frame.copy_fact {
                        let visits = stack
                            .last()
                            .map_or(0usize, |parent| parent.copy_depth)
                            .checked_add(1)
                            .ok_or_else(|| {
                                WordSourceFailure::resource(
                                    "Word source-copy visit overflow".into(),
                                )
                            })?;
                        copy_nodes = copy_nodes.checked_add(visits).ok_or_else(|| {
                            WordSourceFailure::resource("Word source-copy visit overflow".into())
                        })?;
                        let bytes = (reader.buffer_position() as usize - frame.fact_start)
                            .checked_add(frame.copy_scope_bytes)
                            .and_then(|bytes| bytes.checked_mul(visits))
                            .ok_or_else(|| {
                                WordSourceFailure::resource("Word source-copy byte overflow".into())
                            })?;
                        copy_bytes = copy_bytes.checked_add(bytes).ok_or_else(|| {
                            WordSourceFailure::resource("Word source-copy byte overflow".into())
                        })?;
                    }
                    controls -= usize::from(frame.control);
                    path_cost -= frame.cost;
                    restore_namespace_depths(
                        &mut namespace_depths,
                        frame.namespace_undo,
                        &mut copy_scope_weight,
                    );
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let context_work = copy_nodes
        .checked_mul(duplicate_context_bytes)
        .ok_or_else(|| WordSourceFailure::resource("Word source-copy context overflow".into()))?;
    copy_bytes = copy_bytes
        .checked_add(context_work)
        .ok_or_else(|| WordSourceFailure::resource("Word source-copy context overflow".into()))?;
    // Namespace closure may activate an inherited encoded-only Word alias.
    // Such caller-supplied contexts retain the existing constructor scan until
    // their projected alias actions are represented. Closed source declarations
    // already activate aliases in the original qualifier's scope.
    let projected_aliases = prefixes
        .iter()
        .filter_map(|value| split_namespace_binding(value))
        .all(|(prefix, namespace)| {
            namespace != W_NS || prefixes.iter().any(|alias| alias.as_str() == prefix)
        });
    Ok(AdmittedWordSource {
        work: (copy_nodes, copy_bytes),
        constructors,
        projected_aliases,
        source_map: std::sync::Arc::new(WordSourceMap::original(
            0..xml.len().map_err(WordSourceFailure::Resource)?,
        )),
    })
}

/// Public reader entrypoints begin after the owning start event. Reconstruct
/// exactly that remaining owner, without consuming the caller or its siblings.
pub(crate) fn admit_word_reader(
    input: &Reader<&[u8]>,
    prefixes: &[String],
    owner: &[u8],
    namespace: Option<&str>,
) -> Result<AdmittedWordSource> {
    // The clone retains the actual consumed QName stack. Validate its close
    // without moving the production cursor or inventing an owning binding.
    let mut reader = input.clone();
    reader.config_mut().check_end_names = true;
    let xml = *input.get_ref();
    let offset = input.buffer_position();
    let mut depth = 0usize;
    loop {
        let before = reader.buffer_position();
        match reader.read_event()? {
            Event::Start(_) => depth += 1,
            Event::End(_) if depth > 0 => depth -= 1,
            Event::End(end) => {
                let name = end.name();
                if end.local_name().as_ref() != owner {
                    return Err(OxmlError::InvalidValue(
                        "remaining XML closes a different owner".into(),
                    ));
                }
                if let Some(namespace) = namespace {
                    let qualified = if namespace == W_NS {
                        is_word_element(name.as_ref(), owner, prefixes)
                    } else {
                        let bindings = namespace_bindings(prefixes);
                        name.as_ref()
                            .iter()
                            .position(|byte| *byte == b':')
                            .is_some_and(|at| {
                                bindings.iter().any(|(prefix, uri)| {
                                    prefix.as_bytes() == &name.as_ref()[..at] && uri == namespace
                                })
                            })
                    };
                    if !qualified {
                        return Err(OxmlError::InvalidValue(
                            "remaining XML closes a different qualified owner".into(),
                        ));
                    }
                    let mut source = vec![b'<'];
                    source.extend_from_slice(name.as_ref());
                    source.push(b'>');
                    source.extend_from_slice(&xml[..(reader.buffer_position() - offset) as usize]);
                    return admit_word_source(&source, prefixes, None, None);
                }
                // Legacy CT_P/CT_R callers provide a typed operation but no
                // start event or namespace scope. Admit its actual children
                // under that concrete role, retaining their real bindings.
                return admit_word_source(
                    &xml[..(before - offset) as usize],
                    prefixes,
                    Some(owner),
                    None,
                );
            }
            Event::Eof => {
                return Err(OxmlError::InvalidValue(
                    "remaining XML has no owning close".into(),
                ));
            }
            _ => {}
        }
    }
}

/// Content that can appear in a document body (paragraphs and tables).
///
/// Each child is boxed. A direct-body removal then shifts pointers instead of
/// copying the child value, which is what keeps that removal from growing with
/// the rest of the body.
#[derive(Debug, Clone, PartialEq)]
pub enum BodyContent {
    Paragraph(Box<CT_P>),
    Table(Box<CT_Tbl>),
    ContentControl(Box<CT_Sdt>),
    /// Raw XML for unknown elements and controls that cannot be typed safely.
    RawXml(Box<Vec<u8>>),
}

impl BodyContent {
    /// Whether this block's typed owners may be reused by value.
    ///
    /// The walk matches [`admit_word_model`]. A field, a paragraph bookmark,
    /// or a tab stop disqualifies the block. Raw XML stays exact bytes and
    /// is not rejected by itself. This does not serialize the block.
    #[doc(hidden)]
    pub fn owner_facts_reusable_by_value(&self) -> bool {
        use crate::content_control::SdtContent;
        use crate::revision::RevisionContent;
        use crate::table::CellContent;
        use crate::text::RunContent;
        let root = match self {
            BodyContent::Paragraph(paragraph) => WordOwner::Paragraph(paragraph),
            BodyContent::Table(table) => WordOwner::Table(table),
            BodyContent::ContentControl(control) => WordOwner::Control(control),
            BodyContent::RawXml(_) => return true,
        };
        let mut pending = vec![root];
        while let Some(owner) = pending.pop() {
            match owner {
                WordOwner::Body(body) => {
                    for child in &body.content {
                        match child {
                            BodyContent::Paragraph(paragraph) => {
                                pending.push(WordOwner::Paragraph(paragraph))
                            }
                            BodyContent::Table(table) => pending.push(WordOwner::Table(table)),
                            BodyContent::ContentControl(control) => {
                                pending.push(WordOwner::Control(control))
                            }
                            BodyContent::RawXml(_) => {}
                        }
                    }
                }
                WordOwner::Paragraph(paragraph) => {
                    if !paragraph.bookmark_markers.is_empty() {
                        return false;
                    }
                    if paragraph
                        .properties
                        .as_ref()
                        .and_then(|properties| properties.tabs.as_ref())
                        .is_some_and(|tabs| !tabs.tabs.is_empty())
                    {
                        return false;
                    }
                    for run in &paragraph.runs {
                        pending.push(WordOwner::Run(run));
                    }
                    for (_, _, _, control) in &paragraph.content_controls {
                        pending.push(WordOwner::Control(control));
                    }
                    for (_, _, revision) in &paragraph.revisions {
                        pending.push(WordOwner::Revision(revision));
                    }
                    for ruby in &paragraph.rubies {
                        for run in &ruby.ruby_text {
                            pending.push(WordOwner::Run(run));
                        }
                    }
                }
                WordOwner::Run(run) => {
                    for child in &run.content {
                        match child {
                            RunContent::Drawing(drawing) => {
                                pending.push(WordOwner::Drawing(drawing))
                            }
                            RunContent::Field(_) => return false,
                            _ => {}
                        }
                    }
                }
                WordOwner::Control(control) => {
                    for run in control.original_resource_runs() {
                        pending.push(WordOwner::Run(run));
                    }
                    for (_, revision) in &control.revisions {
                        pending.push(WordOwner::Revision(revision));
                    }
                    for child in &control.content {
                        let child = match child {
                            SdtContent::Paragraph(paragraph) => WordOwner::Paragraph(paragraph),
                            SdtContent::Table(table) => WordOwner::Table(table),
                            SdtContent::Row(row) => WordOwner::Row(row),
                            SdtContent::Cell(cell) => WordOwner::Cell(cell),
                            SdtContent::Run(run) => WordOwner::Run(run),
                            SdtContent::ContentControl(nested) => WordOwner::Control(nested),
                            SdtContent::RawXml(_) => continue,
                        };
                        pending.push(child);
                    }
                }
                WordOwner::Table(table) => {
                    for row in &table.rows {
                        pending.push(WordOwner::Row(row));
                    }
                    for (_, _, control) in &table.content_controls {
                        pending.push(WordOwner::Control(control));
                    }
                }
                WordOwner::Row(row) => {
                    for cell in &row.cells {
                        pending.push(WordOwner::Cell(cell));
                    }
                    for (_, _, control) in &row.content_controls {
                        pending.push(WordOwner::Control(control));
                    }
                }
                WordOwner::Cell(cell) => {
                    for child in &cell.content {
                        match child {
                            CellContent::Paragraph(paragraph) => {
                                pending.push(WordOwner::Paragraph(paragraph))
                            }
                            CellContent::Table(table) => pending.push(WordOwner::Table(table)),
                            CellContent::ContentControl(control) => {
                                pending.push(WordOwner::Control(control))
                            }
                        }
                    }
                }
                WordOwner::Field(_) => return false,
                WordOwner::Revision(revision) => {
                    if let Some(paragraph) = revision.content_paragraph() {
                        pending.push(WordOwner::Paragraph(paragraph));
                    }
                    match revision.content() {
                        RevisionContent::Runs(runs) => {
                            for run in runs {
                                pending.push(WordOwner::Run(run));
                            }
                        }
                        RevisionContent::PriorParagraphProperties(properties) => {
                            if properties
                                .tabs
                                .as_ref()
                                .is_some_and(|tabs| !tabs.tabs.is_empty())
                            {
                                return false;
                            }
                        }
                        RevisionContent::Marker
                        | RevisionContent::PriorRunProperties(_)
                        | RevisionContent::PriorTableProperties(_)
                        | RevisionContent::PriorSectionProperties(_) => {}
                    }
                    for (_, nested) in revision.nested_revisions() {
                        pending.push(WordOwner::Revision(nested));
                    }
                }
                WordOwner::Drawing(drawing) => {
                    if let Some(anchor) = &drawing.anchor {
                        pending.push(WordOwner::Anchor(anchor));
                    }
                }
                WordOwner::Anchor(anchor) => {
                    if let Some(shape) = &anchor.shape
                        && (shape.text_body.is_some() || !shape.text.is_empty())
                    {
                        if let Some(body) = &shape.text_body {
                            pending.push(WordOwner::Body(body));
                        }
                        for paragraph in &shape.text {
                            pending.push(WordOwner::Paragraph(paragraph));
                        }
                    }
                }
            }
        }
        true
    }
}

#[derive(Clone, Copy)]
enum SectionScalar {
    Measurement,
    Count,
}
impl SectionScalar {
    fn for_names(element: &[u8], attribute: &[u8]) -> Option<Self> {
        match (element, attribute) {
            (b"pgSz", b"w" | b"h")
            | (
                b"pgMar",
                b"top" | b"right" | b"end" | b"bottom" | b"left" | b"start" | b"gutter" | b"header"
                | b"footer",
            )
            | (b"cols", b"space")
            | (b"col", b"w" | b"space") => Some(Self::Measurement),
            (b"cols", b"num") => Some(Self::Count),
            _ => None,
        }
    }

    fn count(value: &str) -> Result<u32> {
        Ok(value.parse()?)
    }
    fn decode(self, element: &[u8], attribute: &[u8], value: &str) -> Result<()> {
        match self {
            Self::Measurement => {
                parse_integer_measurement::<i32>(element, attribute, value).map(|_| ())
            }
            Self::Count => Self::count(value).map(|_| ()),
        }
    }
}

/// Column definition for multi-column layouts.
#[derive(Debug, Clone, PartialEq)]
pub struct CT_Column {
    /// Column width in twips
    pub width: Option<Twips>,
    /// Space after this column in twips
    pub space: Option<Twips>,
}

/// `CT_Columns` — Column layout configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct CT_Columns {
    /// Number of columns (if equal width)
    pub num: Option<u32>,
    /// Space between columns in twips (when equal width)
    pub space: Option<Twips>,
    /// Whether columns are equal width
    pub equal_width: Option<bool>,
    /// Separator line between columns
    pub sep: Option<bool>,
    /// Individual column definitions (for unequal widths)
    pub columns: Vec<CT_Column>,
}

/// `CT_PageNumberType` -- the M23 page-number restart with retained M24 state.
#[derive(Debug, Clone, PartialEq)]
pub struct CT_PageNumberType {
    /// Displayed page number at which this section starts.
    pub start: Option<u32>,
    /// Original `w:pgNumType` subtree, including unsupported attributes.
    #[doc(hidden)]
    pub raw_xml: Option<Vec<u8>>,
    /// Parsed start value used to detect an authored change.
    #[doc(hidden)]
    pub parsed_start: Option<u32>,
    /// Qualified name of the retained start attribute.
    #[doc(hidden)]
    pub start_attribute_name: Option<Vec<u8>>,
    /// In-scope Word prefix preferred when adding a missing start attribute.
    #[doc(hidden)]
    pub start_insertion_prefix: Option<Vec<u8>>,
    /// Every inherited or local prefix reserved by the retained source scope.
    #[doc(hidden)]
    pub reserved_insertion_prefixes: Vec<Vec<u8>>,
}

impl CT_PageNumberType {
    /// Create an authored page-number restart.
    pub fn new(start: u32) -> Self {
        Self {
            start: Some(start),
            raw_xml: None,
            parsed_start: None,
            start_attribute_name: None,
            start_insertion_prefix: None,
            reserved_insertion_prefixes: Vec::new(),
        }
    }
}

impl Default for CT_Columns {
    fn default() -> Self {
        CT_Columns {
            num: Some(1),
            space: Some(Twips(720)),
            equal_width: Some(true),
            sep: None,
            columns: Vec::new(),
        }
    }
}

/// `ST_PageBorderZOrder` -- whether a page border draws in front of content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ST_PageBorderZOrder {
    Front,
    Back,
}

impl ST_PageBorderZOrder {
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "front" => Some(Self::Front),
            "back" => Some(Self::Back),
            _ => None,
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::Front => "front",
            Self::Back => "back",
        }
    }
}

/// `ST_PageBorderDisplay` -- which pages of a section carry the page border.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ST_PageBorderDisplay {
    AllPages,
    FirstPage,
    NotFirstPage,
}

impl ST_PageBorderDisplay {
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "allPages" => Some(Self::AllPages),
            "firstPage" => Some(Self::FirstPage),
            "notFirstPage" => Some(Self::NotFirstPage),
            _ => None,
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::AllPages => "allPages",
            Self::FirstPage => "firstPage",
            Self::NotFirstPage => "notFirstPage",
        }
    }
}

/// `ST_PageBorderOffset` -- what the page-border edge offsets are measured from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ST_PageBorderOffset {
    Page,
    Text,
}

impl ST_PageBorderOffset {
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "page" => Some(Self::Page),
            "text" => Some(Self::Text),
            _ => None,
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Text => "text",
        }
    }
}

/// `CT_PageBorders` -- the `w:pgBorders` frame drawn around a section's pages.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CT_PageBorders {
    /// `w:zOrder`, whether the frame draws in front of or behind page content.
    pub z_order: Option<ST_PageBorderZOrder>,
    /// `w:display`, which physical pages of the section receive the frame.
    pub display: Option<ST_PageBorderDisplay>,
    /// `w:offsetFrom`, the rectangle each edge offset is measured from.
    pub offset_from: Option<ST_PageBorderOffset>,
    pub top: Option<CT_BorderEdge>,
    pub left: Option<CT_BorderEdge>,
    pub bottom: Option<CT_BorderEdge>,
    pub right: Option<CT_BorderEdge>,
    /// Attributes this type does not model, in source order, written first.
    ///
    /// Values are serialized verbatim, so a caller storing one owns the
    /// escaping, exactly as `CT_BorderEdge::extra_attributes` does.
    pub extra_attributes: Vec<(String, String)>,
    /// Child elements this type does not model, in source order.
    pub extra_xml: Vec<Vec<u8>>,
}

/// `ST_LineNumberRestart` -- where a section's line numbering starts over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ST_LineNumberRestart {
    NewPage,
    NewSection,
    Continuous,
}

impl ST_LineNumberRestart {
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "newPage" => Some(Self::NewPage),
            "newSection" => Some(Self::NewSection),
            "continuous" => Some(Self::Continuous),
            _ => None,
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::NewPage => "newPage",
            Self::NewSection => "newSection",
            Self::Continuous => "continuous",
        }
    }
}

/// `CT_LineNumber` -- the `w:lnNumType` margin line numbering of a section.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CT_LineNumber {
    /// `w:countBy`, the interval at which a number is printed.
    pub count_by: Option<u32>,
    /// `w:start`, the first line number of the section.
    pub start: Option<u32>,
    /// `w:distance`, the gap between the number and the text it labels.
    pub distance: Option<Twips>,
    /// `w:restart`, where numbering starts over.
    pub restart: Option<ST_LineNumberRestart>,
    /// Attributes this type does not model, in source order, written first.
    pub extra_attributes: Vec<(String, String)>,
}

/// `CT_PaperSource` -- the `w:paperSrc` printer trays a section prints from.
///
/// Tray selection happens in the printer driver, so this has no on-page
/// consequence and pagination is identical with and without it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CT_PaperSource {
    /// `w:first`, the tray feeding the section's first page.
    pub first: Option<u32>,
    /// `w:other`, the tray feeding every later page.
    pub other: Option<u32>,
    /// Attributes this type does not model, in source order, written first.
    pub extra_attributes: Vec<(String, String)>,
}

/// `ST_DocGrid` -- how a section's character grid constrains its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum ST_DocGrid {
    /// No grid. Lines and characters keep their ordinary metrics.
    Default,
    /// Line advance snaps to the grid pitch. Characters keep their advance.
    Lines,
    /// Line advance snaps to the pitch and characters snap to the char space.
    LinesAndChars,
    /// Characters snap to the char space, and line advance snaps too.
    SnapToChars,
}

impl ST_DocGrid {
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "default" => Some(Self::Default),
            "lines" => Some(Self::Lines),
            "linesAndChars" => Some(Self::LinesAndChars),
            "snapToChars" => Some(Self::SnapToChars),
            _ => None,
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Lines => "lines",
            Self::LinesAndChars => "linesAndChars",
            Self::SnapToChars => "snapToChars",
        }
    }

    /// Whether this grid type puts line advance on the grid pitch.
    ///
    /// `Default` is deliberately false. Keeping the default type off the grid
    /// path is what leaves every ungridded document on the arithmetic it
    /// already had, with no new floating-point step on the existing branch.
    pub fn snaps_lines(self) -> bool {
        matches!(self, Self::Lines | Self::LinesAndChars | Self::SnapToChars)
    }

    /// Whether this grid type puts per-character advance on the grid.
    pub fn snaps_characters(self) -> bool {
        matches!(self, Self::LinesAndChars | Self::SnapToChars)
    }
}

/// `CT_DocGrid` -- the `w:docGrid` character grid of a section.
///
/// `line_pitch` is the twip distance between two grid lines. `char_space` is
/// the twip addition to one East Asian character cell.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub struct CT_DocGrid {
    /// `w:type`, which of the grid constraints apply.
    pub grid_type: Option<ST_DocGrid>,
    /// `w:linePitch`, the grid line spacing in twips.
    pub line_pitch: Option<Twips>,
    /// `w:charSpace`, the grid character advance addition in twips.
    pub char_space: Option<i32>,
    /// Attributes this type does not model, in source order, written first.
    pub extra_attributes: Vec<(String, String)>,
}

/// Section-level note configuration, shared by `w:footnotePr` and `w:endnotePr`.
///
/// The values and the authoring surface are modeled. Their effect on marker
/// text, placement and restart is not applied yet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CT_NoteProperties {
    /// `w:pos/@w:val`, where the notes are placed.
    pub pos: Option<String>,
    /// `w:numFmt/@w:val`, the marker number format.
    pub num_fmt: Option<String>,
    /// `w:numStart/@w:val`, the first marker number.
    pub num_start: Option<u32>,
    /// `w:numRestart/@w:val`, where marker numbering starts over.
    pub num_restart: Option<String>,
    /// IDs of special note records selected by document settings.
    pub special_references: Vec<i32>,
    /// Unmodelled children retained at their schema insertion slots.
    ///
    /// A modelled child carrying an attribute this type does not read is kept
    /// here whole rather than typed, which is the choice `w:pgSz` already makes.
    #[doc(hidden)]
    pub extra_xml: Vec<(usize, Vec<u8>)>,
}

/// Value and occurrence anchor for one repeated section reference.
///
/// Distinguishable values follow their source predecessor or successor. Equal
/// duplicates are indistinguishable through the public `Vec` mutation surface,
/// so they resolve deterministically by their ordinal among equal current
/// values. The following anchor takes precedence when both neighbors resolve.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct CT_SectPrReferenceAnchor {
    reference: HdrFtrRef,
    source_occurrence: usize,
}

impl CT_SectPrReferenceAnchor {
    fn at(references: &[HdrFtrRef], index: usize) -> Option<Self> {
        let reference = references.get(index)?.clone();
        let source_occurrence = references[..index]
            .iter()
            .filter(|candidate| **candidate == reference)
            .count();
        Some(Self {
            reference,
            source_occurrence,
        })
    }

    fn resolve(&self, references: &[HdrFtrRef]) -> Option<usize> {
        references
            .iter()
            .enumerate()
            .filter(|(_, reference)| **reference == self.reference)
            .nth(self.source_occurrence)
            .map(|(index, _)| index)
    }
}

/// Retained section-child position, including repeated-child occurrence anchors.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub enum CT_SectPrRawPosition {
    /// A schema slot and modeled occurrence boundary.
    Schema { slot: usize, occurrence: usize },
    /// A boundary tied to neighboring header-reference values and occurrences.
    Header {
        preceding: Option<CT_SectPrReferenceAnchor>,
        following: Option<CT_SectPrReferenceAnchor>,
    },
    /// A boundary tied to neighboring footer-reference values and occurrences.
    Footer {
        preceding: Option<CT_SectPrReferenceAnchor>,
        following: Option<CT_SectPrReferenceAnchor>,
    },
}

/// `CT_SectPr` — Section properties (page size, margins, columns, orientation).
#[derive(Debug, Clone, PartialEq)]
#[allow(non_snake_case)]
pub struct CT_SectPr {
    /// Page width in twips
    pub page_width: Option<Twips>,
    /// Page height in twips
    pub page_height: Option<Twips>,
    /// Page orientation
    pub orientation: Option<ST_PageOrientation>,
    /// Top margin in twips
    pub margin_top: Option<Twips>,
    /// Right margin in twips
    pub margin_right: Option<Twips>,
    /// Bottom margin in twips
    pub margin_bottom: Option<Twips>,
    /// Left margin in twips
    pub margin_left: Option<Twips>,
    /// Gutter margin in twips
    pub gutter: Option<Twips>,
    /// Header distance from top edge in twips
    pub header_distance: Option<Twips>,
    /// Footer distance from bottom edge in twips
    pub footer_distance: Option<Twips>,
    /// Section break type
    pub section_type: Option<ST_SectionType>,
    /// Column layout
    pub columns: Option<CT_Columns>,
    /// Page-number restart. Unsupported format and chapter attributes remain raw.
    pub page_number: Option<CT_PageNumberType>,
    /// Section footnote configuration. Not applied to note rendering yet.
    ///
    /// Boxed, with the other large members, so a section stays cheap on the
    /// stack. `CT_SectPr` sits inside `CT_PPr`, which test threads build by
    /// value against a 2 MiB ceiling.
    pub footnote_pr: Option<Box<CT_NoteProperties>>,
    /// Section endnote configuration. Not applied to note rendering yet.
    pub endnote_pr: Option<Box<CT_NoteProperties>>,
    /// Printer tray selection. Round-tripped, with no effect on page geometry.
    pub paper_source: Option<Box<CT_PaperSource>>,
    /// Page border frame.
    pub page_borders: Option<Box<CT_PageBorders>>,
    /// Margin line numbering.
    pub line_numbers: Option<Box<CT_LineNumber>>,
    /// Vertical alignment of the body band within the page.
    pub vertical_alignment: Option<ST_VerticalJc>,
    /// `w:textDirection`. Authored and preserved here, projected by layout.
    pub text_direction: Option<String>,
    /// `w:docGrid`, the section character grid.
    ///
    /// Boxed with the other composite members so a section stays cheap on the
    /// stack. `CT_SectPr` sits inside `CT_PPr`, which test threads build by
    /// value against a 2 MiB ceiling.
    pub doc_grid: Option<Box<CT_DocGrid>>,
    /// Title page (different first page header/footer)
    pub title_pg: Option<bool>,
    /// Header references
    pub header_refs: Vec<HdrFtrRef>,
    /// Footer references
    pub footer_refs: Vec<HdrFtrRef>,
    /// Unknown child elements captured as raw XML.
    pub extra_xml: Vec<Vec<u8>>,
    /// Schema slots and modeled-child boundaries for retained child elements.
    #[doc(hidden)]
    pub extra_xml_positions: Vec<CT_SectPrRawPosition>,
    /// Prior section properties from the schema-final `w:sectPrChange`.
    pub change: Option<CT_Revision>,
}

#[allow(non_snake_case)]
impl CT_SectPr {
    fn empty() -> Self {
        Self {
            page_width: None,
            page_height: None,
            orientation: None,
            margin_top: None,
            margin_right: None,
            margin_bottom: None,
            margin_left: None,
            gutter: None,
            header_distance: None,
            footer_distance: None,
            section_type: None,
            columns: None,
            page_number: None,
            footnote_pr: None,
            endnote_pr: None,
            paper_source: None,
            page_borders: None,
            line_numbers: None,
            vertical_alignment: None,
            text_direction: None,
            doc_grid: None,
            title_pg: None,
            header_refs: Vec::new(),
            footer_refs: Vec::new(),
            extra_xml: Vec::new(),
            extra_xml_positions: Vec::new(),
            change: None,
        }
    }

    /// Report whether one raw section carrier retains root attributes.
    #[doc(hidden)]
    pub fn raw_position_is_root_attributes(position: &CT_SectPrRawPosition, raw: &[u8]) -> bool {
        matches!(
            position,
            CT_SectPrRawPosition::Schema {
                slot: usize::MAX,
                occurrence: usize::MAX
            }
        ) && is_root_attribute_record(raw)
    }

    fn from_empty_root(root: &BytesStart<'_>, word_prefixes: &[String]) -> Result<Self> {
        let mut section = Self::empty();
        if let Some(record) = capture_root_attribute_record(root, word_prefixes)? {
            section.extra_xml.push(record);
            section
                .extra_xml_positions
                .push(CT_SectPrRawPosition::Schema {
                    slot: usize::MAX,
                    occurrence: usize::MAX,
                });
        }
        Ok(section)
    }

    /// Default US Letter page with 1-inch margins.
    pub fn default_letter() -> Self {
        CT_SectPr {
            page_width: Some(Twips(12240)),  // 8.5"
            page_height: Some(Twips(15840)), // 11"
            orientation: Some(ST_PageOrientation::Portrait),
            margin_top: Some(Twips(1440)),    // 1"
            margin_right: Some(Twips(1440)),  // 1"
            margin_bottom: Some(Twips(1440)), // 1"
            margin_left: Some(Twips(1440)),   // 1"
            gutter: Some(Twips(0)),
            header_distance: Some(Twips(720)),
            footer_distance: Some(Twips(720)),
            section_type: None,
            columns: None,
            page_number: None,
            footnote_pr: None,
            endnote_pr: None,
            paper_source: None,
            page_borders: None,
            line_numbers: None,
            vertical_alignment: None,
            text_direction: None,
            doc_grid: None,
            title_pg: None,
            header_refs: Vec::new(),
            footer_refs: Vec::new(),
            extra_xml: Vec::new(),
            extra_xml_positions: Vec::new(),
            change: None,
        }
    }

    /// Default A4 page with 1-inch margins.
    pub fn default_a4() -> Self {
        CT_SectPr {
            page_width: Some(Twips(11906)),  // 210mm
            page_height: Some(Twips(16838)), // 297mm
            orientation: Some(ST_PageOrientation::Portrait),
            margin_top: Some(Twips(1440)),
            margin_right: Some(Twips(1440)),
            margin_bottom: Some(Twips(1440)),
            margin_left: Some(Twips(1440)),
            gutter: Some(Twips(0)),
            header_distance: Some(Twips(720)),
            footer_distance: Some(Twips(720)),
            section_type: None,
            columns: None,
            page_number: None,
            footnote_pr: None,
            endnote_pr: None,
            paper_source: None,
            page_borders: None,
            line_numbers: None,
            vertical_alignment: None,
            text_direction: None,
            doc_grid: None,
            title_pg: None,
            header_refs: Vec::new(),
            footer_refs: Vec::new(),
            extra_xml: Vec::new(),
            extra_xml_positions: Vec::new(),
            change: None,
        }
    }

    pub fn from_xml(reader: &mut Reader<&[u8]>) -> Result<Self> {
        Self::from_xml_with_prefixes(reader, &["w".to_owned()])
    }

    pub(crate) fn from_xml_with_prefixes(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
    ) -> Result<Self> {
        Self::from_xml_with_prefixes_and_owner_bindings(reader, word_prefixes, &[])
    }

    pub(crate) fn from_xml_with_prefixes_and_owner_bindings(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        owner_bindings: &[(String, String)],
    ) -> Result<Self> {
        Self::from_xml_with_prefixes_owner_bindings_and_root(
            reader,
            word_prefixes,
            owner_bindings,
            None,
        )
    }

    /// Admit one checked section owner before typed construction.
    #[doc(hidden)]
    pub fn from_xml_fragment_checked(xml: &[u8]) -> Result<Self> {
        admit_word_xml(xml, &[], None)?;
        crate::text::validate_word_owner_fragment(xml, b"sectPr", true)?;
        let mut reader = Reader::from_reader(xml);
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer)? {
                Event::Start(root) => {
                    let prefixes = word_prefixes_at(&root, &[])?;
                    let bindings = local_namespace_overrides(&root, &prefixes)?;
                    return Self::from_xml_with_prefixes_owner_bindings_and_root(
                        &mut reader,
                        &prefixes,
                        &bindings,
                        Some(&root),
                    );
                }
                Event::Empty(root) => {
                    let prefixes = word_prefixes_at(&root, &[])?;
                    return Self::from_empty_root(&root, &prefixes);
                }
                _ => {}
            }
            buffer.clear();
        }
    }

    pub(crate) fn from_xml_with_prefixes_owner_bindings_and_root(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        owner_bindings: &[(String, String)],
        root: Option<&BytesStart<'_>>,
    ) -> Result<Self> {
        let raw_owner_bindings =
            crate::text::bindings_outside_retained_root(owner_bindings, root, word_prefixes)?;
        // A page-number raw owner is also read independently by layout. Preserve
        // its actual inherited URI authority before section/part scope is lost.
        // The established fixed writer supplies verified canonical w already.
        let page_number_bindings =
            merged_owner_bindings(&namespace_bindings(word_prefixes), owner_bindings)
                .into_iter()
                .filter(|(prefix, uri)| prefix != "w" || uri != W_NS)
                .collect::<Vec<_>>();
        let mut sect = Self::empty();
        let mut change_raw_index = 0usize;
        let mut raw_position = (0usize, 0usize);
        let mut header_count = 0usize;
        let mut footer_count = 0usize;
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Empty(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    if is_word_element(name.as_ref(), b"pgSz", &prefixes) {
                        Self::parse_page_size_attributes(&mut sect, e, &prefixes)?;
                        raw_position = (4, 0);
                    } else if is_word_element(name.as_ref(), b"pgMar", &prefixes) {
                        Self::parse_page_margin_attributes(&mut sect, e, &prefixes)?;
                        raw_position = (5, 0);
                    } else if is_word_element(name.as_ref(), b"type", &prefixes) {
                        if let Some(val) = get_word_val_attr(e, &prefixes)? {
                            sect.section_type = ST_SectionType::from_str(&val).ok();
                        }
                        raw_position = (3, 0);
                    } else if is_word_element(name.as_ref(), b"pgNumType", &prefixes) {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_empty_element(e)?,
                            &page_number_bindings,
                        )?;
                        if sect.page_number.is_none() {
                            sect.page_number =
                                Some(Self::parse_page_number(e, &prefixes, owner_bindings, raw)?);
                        } else {
                            sect.push_extra_xml(raw, 6, 1);
                        }
                        raw_position = (6, 1);
                    } else if is_word_element(name.as_ref(), b"cols", &prefixes) {
                        sect.columns = Some(Self::parse_cols_empty(e, &prefixes)?);
                        raw_position = (7, 0);
                    } else if is_word_element(name.as_ref(), b"footnotePr", &prefixes) {
                        if sect.footnote_pr.is_none() && e.attributes().next().is_none() {
                            sect.footnote_pr = Some(Box::default());
                        } else {
                            let raw = crate::text::raw_with_external_bindings(
                                &capture_empty_element(e)?,
                                &raw_owner_bindings,
                            )?;
                            sect.push_extra_xml(raw, 2, 0);
                        }
                        raw_position = (2, 0);
                    } else if is_word_element(name.as_ref(), b"endnotePr", &prefixes) {
                        if sect.endnote_pr.is_none() && e.attributes().next().is_none() {
                            sect.endnote_pr = Some(Box::default());
                        } else {
                            let raw = crate::text::raw_with_external_bindings(
                                &capture_empty_element(e)?,
                                &raw_owner_bindings,
                            )?;
                            sect.push_extra_xml(raw, 2, 1);
                        }
                        raw_position = (2, 1);
                    } else if is_word_element(name.as_ref(), b"paperSrc", &prefixes) {
                        if sect.paper_source.is_none() {
                            sect.paper_source =
                                Some(Box::new(Self::parse_paper_source(e, &prefixes)?));
                        } else {
                            let raw = crate::text::raw_with_external_bindings(
                                &capture_empty_element(e)?,
                                &raw_owner_bindings,
                            )?;
                            sect.push_extra_xml(raw, 5, 1);
                        }
                        raw_position = (5, 1);
                    } else if is_word_element(name.as_ref(), b"pgBorders", &prefixes) {
                        if sect.page_borders.is_none() {
                            sect.page_borders =
                                Some(Box::new(Self::parse_page_border_attrs(e, &prefixes)?));
                        } else {
                            let raw = crate::text::raw_with_external_bindings(
                                &capture_empty_element(e)?,
                                &raw_owner_bindings,
                            )?;
                            sect.push_extra_xml(raw, 5, 2);
                        }
                        raw_position = (5, 2);
                    } else if is_word_element(name.as_ref(), b"lnNumType", &prefixes) {
                        if sect.line_numbers.is_none() {
                            sect.line_numbers =
                                Some(Box::new(Self::parse_line_number(e, &prefixes)?));
                        } else {
                            let raw = crate::text::raw_with_external_bindings(
                                &capture_empty_element(e)?,
                                &raw_owner_bindings,
                            )?;
                            sect.push_extra_xml(raw, 5, 3);
                        }
                        raw_position = (5, 3);
                    } else if is_word_element(name.as_ref(), b"vAlign", &prefixes) {
                        let typed = sole_val_attribute(e, &prefixes)?
                            .filter(|value| ST_VerticalJc::from_str(value).to_str() == value)
                            .map(|value| ST_VerticalJc::from_str(&value));
                        match typed {
                            Some(value) if sect.vertical_alignment.is_none() => {
                                sect.vertical_alignment = Some(value);
                            }
                            _ => {
                                let raw = crate::text::raw_with_external_bindings(
                                    &capture_empty_element(e)?,
                                    &raw_owner_bindings,
                                )?;
                                sect.push_extra_xml(raw, 7, 0);
                            }
                        }
                        raw_position = (7, 1);
                    } else if is_word_element(name.as_ref(), b"textDirection", &prefixes) {
                        match sole_val_attribute(e, &prefixes)? {
                            Some(value) if sect.text_direction.is_none() => {
                                sect.text_direction = Some(value);
                            }
                            _ => {
                                let raw = crate::text::raw_with_external_bindings(
                                    &capture_empty_element(e)?,
                                    &raw_owner_bindings,
                                )?;
                                sect.push_extra_xml(raw, 8, 0);
                            }
                        }
                        raw_position = (8, 1);
                    } else if is_word_element(name.as_ref(), b"docGrid", &prefixes) {
                        if sect.doc_grid.is_none() {
                            sect.doc_grid = Some(Box::new(Self::parse_doc_grid(e, &prefixes)?));
                        } else {
                            // The schema allows one. A second is retained
                            // after the typed one rather than before it, the
                            // way a second `w:paperSrc` already is.
                            let raw = crate::text::raw_with_external_bindings(
                                &capture_empty_element(e)?,
                                &raw_owner_bindings,
                            )?;
                            sect.push_extra_xml(raw, 8, 2);
                        }
                        raw_position = (8, 2);
                    } else if is_word_element(name.as_ref(), b"headerReference", &prefixes) {
                        let mut hdr_type = HdrFtrType::Default;
                        let mut rel_id = String::new();
                        for attr in e.attributes() {
                            let attr = attr?;
                            let key = attr.key.as_ref();
                            let val = std::str::from_utf8(&attr.value)?;
                            if is_word_attribute(key, b"type", &prefixes) {
                                hdr_type = HdrFtrType::from_str(val);
                            } else if attribute_in_namespace(
                                key,
                                b"id",
                                crate::namespace::R_NS,
                                &prefixes,
                            ) {
                                rel_id = val.to_string();
                            }
                        }
                        if !rel_id.is_empty() {
                            sect.header_refs.push(HdrFtrRef {
                                hdr_ftr_type: hdr_type,
                                rel_id,
                            });
                            header_count += 1;
                        }
                        raw_position = (0, header_count);
                    } else if is_word_element(name.as_ref(), b"footerReference", &prefixes) {
                        let mut ftr_type = HdrFtrType::Default;
                        let mut rel_id = String::new();
                        for attr in e.attributes() {
                            let attr = attr?;
                            let key = attr.key.as_ref();
                            let val = std::str::from_utf8(&attr.value)?;
                            if is_word_attribute(key, b"type", &prefixes) {
                                ftr_type = HdrFtrType::from_str(val);
                            } else if attribute_in_namespace(
                                key,
                                b"id",
                                crate::namespace::R_NS,
                                &prefixes,
                            ) {
                                rel_id = val.to_string();
                            }
                        }
                        if !rel_id.is_empty() {
                            sect.footer_refs.push(HdrFtrRef {
                                hdr_ftr_type: ftr_type,
                                rel_id,
                            });
                            footer_count += 1;
                        }
                        raw_position = (1, footer_count);
                    } else if is_word_element(name.as_ref(), b"titlePg", &prefixes) {
                        sect.title_pg = Some(
                            get_word_val_attr(e, &prefixes)?
                                .is_none_or(|value| value != "0" && value != "false"),
                        );
                        raw_position = (8, 0);
                    } else if is_word_element(name.as_ref(), b"sectPrChange", &prefixes) {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_empty_element(e)?,
                            &raw_owner_bindings,
                        )?;
                        if let Some(revision) = CT_Revision::from_raw(raw.clone(), &prefixes) {
                            if let Some(previous) = sect.change.replace(revision) {
                                sect.extra_xml
                                    .insert(change_raw_index, previous.into_raw_xml());
                                sect.extra_xml_positions.insert(
                                    change_raw_index,
                                    CT_SectPrRawPosition::Schema {
                                        slot: 9,
                                        occurrence: 0,
                                    },
                                );
                            }
                            change_raw_index = sect.extra_xml.len();
                        } else {
                            sect.push_extra_xml(raw, 9, 0);
                        }
                        raw_position = (9, 0);
                    } else {
                        // Capture unknown empty elements
                        let position = Self::raw_child_schema_slot(name.as_ref(), &prefixes)
                            .unwrap_or(raw_position);
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_empty_element(e)?,
                            &raw_owner_bindings,
                        )?;
                        sect.push_extra_xml(raw, position.0, position.1);
                        raw_position = position;
                    }
                }
                Ok(Event::Start(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    if is_word_element(name.as_ref(), b"pgSz", &prefixes) {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            &raw_owner_bindings,
                        )?;
                        if raw_element_has_child_content(&raw)?
                            || !page_size_attributes_are_modeled(e, &prefixes)?
                        {
                            sect.push_extra_xml(raw, 4, 0);
                        } else {
                            Self::parse_page_size_attributes(&mut sect, e, &prefixes)?;
                        }
                        raw_position = (4, 0);
                    } else if is_word_element(name.as_ref(), b"pgMar", &prefixes) {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            &raw_owner_bindings,
                        )?;
                        if raw_element_has_child_content(&raw)?
                            || !page_margin_attributes_are_modeled(e, &prefixes)?
                        {
                            sect.push_extra_xml(raw, 5, 0);
                        } else {
                            Self::parse_page_margin_attributes(&mut sect, e, &prefixes)?;
                        }
                        raw_position = (5, 0);
                    } else if is_word_element(name.as_ref(), b"headerReference", &prefixes) {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            &raw_owner_bindings,
                        )?;
                        if raw_element_has_child_content(&raw)?
                            || !story_reference_attributes_are_modeled(e, &prefixes)?
                        {
                            sect.push_extra_xml(raw, 0, header_count);
                        } else if let Some(reference) = Self::parse_story_reference(e, &prefixes)? {
                            sect.header_refs.push(reference);
                            header_count += 1;
                        }
                        raw_position = (0, header_count);
                    } else if is_word_element(name.as_ref(), b"footerReference", &prefixes) {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            &raw_owner_bindings,
                        )?;
                        if raw_element_has_child_content(&raw)?
                            || !story_reference_attributes_are_modeled(e, &prefixes)?
                        {
                            sect.push_extra_xml(raw, 1, footer_count);
                        } else if let Some(reference) = Self::parse_story_reference(e, &prefixes)? {
                            sect.footer_refs.push(reference);
                            footer_count += 1;
                        }
                        raw_position = (1, footer_count);
                    } else if is_word_element(name.as_ref(), b"cols", &prefixes) {
                        sect.columns = Some(Self::parse_cols_start(reader, e, &prefixes)?);
                        raw_position = (7, 0);
                    } else if is_word_element(name.as_ref(), b"footnotePr", &prefixes) {
                        if sect.footnote_pr.is_none() && e.attributes().next().is_none() {
                            sect.footnote_pr = Some(Box::new(Self::parse_note_properties(
                                reader,
                                &prefixes,
                                owner_bindings,
                                b"footnotePr",
                            )?));
                        } else {
                            let raw = crate::text::raw_with_external_bindings(
                                &capture_element(reader, e)?,
                                &raw_owner_bindings,
                            )?;
                            sect.push_extra_xml(raw, 2, 0);
                        }
                        raw_position = (2, 0);
                    } else if is_word_element(name.as_ref(), b"endnotePr", &prefixes) {
                        if sect.endnote_pr.is_none() && e.attributes().next().is_none() {
                            sect.endnote_pr = Some(Box::new(Self::parse_note_properties(
                                reader,
                                &prefixes,
                                owner_bindings,
                                b"endnotePr",
                            )?));
                        } else {
                            let raw = crate::text::raw_with_external_bindings(
                                &capture_element(reader, e)?,
                                &raw_owner_bindings,
                            )?;
                            sect.push_extra_xml(raw, 2, 1);
                        }
                        raw_position = (2, 1);
                    } else if is_word_element(name.as_ref(), b"pgBorders", &prefixes) {
                        if sect.page_borders.is_none() {
                            sect.page_borders = Some(Box::new(Self::parse_page_borders(
                                reader,
                                e,
                                &prefixes,
                                owner_bindings,
                            )?));
                        } else {
                            let raw = crate::text::raw_with_external_bindings(
                                &capture_element(reader, e)?,
                                &raw_owner_bindings,
                            )?;
                            sect.push_extra_xml(raw, 5, 2);
                        }
                        raw_position = (5, 2);
                    } else if is_word_element(name.as_ref(), b"paperSrc", &prefixes) {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            &raw_owner_bindings,
                        )?;
                        sect.push_extra_xml(raw, 5, 0);
                        raw_position = (5, 1);
                    } else if is_word_element(name.as_ref(), b"lnNumType", &prefixes) {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            &raw_owner_bindings,
                        )?;
                        sect.push_extra_xml(raw, 5, 2);
                        raw_position = (5, 3);
                    } else if is_word_element(name.as_ref(), b"vAlign", &prefixes) {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            &raw_owner_bindings,
                        )?;
                        sect.push_extra_xml(raw, 7, 0);
                        raw_position = (7, 1);
                    } else if is_word_element(name.as_ref(), b"textDirection", &prefixes) {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            &raw_owner_bindings,
                        )?;
                        sect.push_extra_xml(raw, 8, 0);
                        raw_position = (8, 1);
                    } else if is_word_element(name.as_ref(), b"docGrid", &prefixes) {
                        // The schema makes `w:docGrid` empty. One that carries
                        // children is a producer extension, so it stays raw at
                        // the slot the typed element would have written.
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            &raw_owner_bindings,
                        )?;
                        sect.push_extra_xml(raw, 8, 1);
                        raw_position = (8, 2);
                    } else if is_word_element(name.as_ref(), b"pgNumType", &prefixes) {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            &page_number_bindings,
                        )?;
                        if sect.page_number.is_none() {
                            sect.page_number =
                                Some(Self::parse_page_number(e, &prefixes, owner_bindings, raw)?);
                        } else {
                            sect.push_extra_xml(raw, 6, 1);
                        }
                        raw_position = (6, 1);
                    } else if is_word_element(name.as_ref(), b"sectPrChange", &prefixes) {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            &raw_owner_bindings,
                        )?;
                        if let Some(revision) = CT_Revision::from_raw(raw.clone(), &prefixes) {
                            if let Some(previous) = sect.change.replace(revision) {
                                sect.extra_xml
                                    .insert(change_raw_index, previous.into_raw_xml());
                                sect.extra_xml_positions.insert(
                                    change_raw_index,
                                    CT_SectPrRawPosition::Schema {
                                        slot: 9,
                                        occurrence: 0,
                                    },
                                );
                            }
                            change_raw_index = sect.extra_xml.len();
                        } else {
                            sect.push_extra_xml(raw, 9, 0);
                        }
                        raw_position = (9, 0);
                    } else {
                        // Capture unknown start elements as raw XML
                        let position = Self::raw_child_schema_slot(name.as_ref(), &prefixes)
                            .unwrap_or(raw_position);
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            &raw_owner_bindings,
                        )?;
                        sect.push_extra_xml(raw, position.0, position.1);
                        raw_position = position;
                    }
                }
                Ok(Event::End(ref e)) if matches_local_name(e.name().as_ref(), b"sectPr") => {
                    break;
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        if let Some(record) = root
            .map(|root| capture_root_attribute_record(root, word_prefixes))
            .transpose()?
            .flatten()
        {
            sect.extra_xml.push(record);
            sect.extra_xml_positions.push(CT_SectPrRawPosition::Schema {
                slot: usize::MAX,
                occurrence: usize::MAX,
            });
        }
        sect.bind_story_reference_positions();
        Ok(sect)
    }

    fn parse_page_size_attributes(
        sect: &mut Self,
        element: &BytesStart<'_>,
        word_prefixes: &[String],
    ) -> Result<()> {
        for attribute in element.attributes() {
            let attribute = attribute?;
            let key = attribute.key.as_ref();
            let value = std::str::from_utf8(&attribute.value)?;
            let measurement = || parse_integer_measurement(element.name().as_ref(), key, value);
            if is_word_attribute(key, b"w", word_prefixes) {
                sect.page_width = Some(Twips(measurement()?));
            } else if is_word_attribute(key, b"h", word_prefixes) {
                sect.page_height = Some(Twips(measurement()?));
            } else if is_word_attribute(key, b"orient", word_prefixes) {
                sect.orientation = ST_PageOrientation::from_str(value).ok();
            }
        }
        Ok(())
    }

    fn parse_page_margin_attributes(
        sect: &mut Self,
        element: &BytesStart<'_>,
        word_prefixes: &[String],
    ) -> Result<()> {
        for attribute in element.attributes() {
            let attribute = attribute?;
            let key = attribute.key.as_ref();
            let field = if is_word_attribute(key, b"top", word_prefixes) {
                &mut sect.margin_top
            } else if is_word_attribute(key, b"right", word_prefixes)
                || is_word_attribute(key, b"end", word_prefixes)
            {
                &mut sect.margin_right
            } else if is_word_attribute(key, b"bottom", word_prefixes) {
                &mut sect.margin_bottom
            } else if is_word_attribute(key, b"left", word_prefixes)
                || is_word_attribute(key, b"start", word_prefixes)
            {
                &mut sect.margin_left
            } else if is_word_attribute(key, b"gutter", word_prefixes) {
                &mut sect.gutter
            } else if is_word_attribute(key, b"header", word_prefixes) {
                &mut sect.header_distance
            } else if is_word_attribute(key, b"footer", word_prefixes) {
                &mut sect.footer_distance
            } else {
                continue;
            };
            let value = std::str::from_utf8(&attribute.value)?;
            *field = Some(Twips(parse_integer_measurement(
                element.name().as_ref(),
                key,
                value,
            )?));
        }
        Ok(())
    }

    fn parse_story_reference(
        element: &BytesStart<'_>,
        word_prefixes: &[String],
    ) -> Result<Option<HdrFtrRef>> {
        let mut hdr_ftr_type = HdrFtrType::Default;
        let mut relationship_id = None;
        for attribute in element.attributes() {
            let attribute = attribute?;
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())?
                .into_owned();
            if is_word_attribute(attribute.key.as_ref(), b"type", word_prefixes) {
                hdr_ftr_type = HdrFtrType::from_str(&value);
            } else if attribute_in_namespace(
                attribute.key.as_ref(),
                b"id",
                crate::namespace::R_NS,
                word_prefixes,
            ) {
                relationship_id = Some(value);
            }
        }
        Ok(relationship_id.map(|rel_id| HdrFtrRef {
            hdr_ftr_type,
            rel_id,
        }))
    }

    fn parse_page_number(
        e: &BytesStart<'_>,
        word_prefixes: &[String],
        owner_bindings: &[(String, String)],
        raw_xml: Vec<u8>,
    ) -> Result<CT_PageNumberType> {
        let mut start = None;
        let mut start_attribute_name = None;
        for attr in e.attributes() {
            let attr = attr?;
            if is_word_attribute(attr.key.as_ref(), b"start", word_prefixes) {
                start = attr
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, e.decoder())?
                    .parse()
                    .ok();
                start_attribute_name = Some(attr.key.as_ref().to_vec());
            }
        }
        let element_name = e.name();
        let element_prefix = element_name
            .as_ref()
            .iter()
            .position(|byte| *byte == b':')
            .map(|separator| &element_name.as_ref()[..separator]);
        let start_insertion_prefix = element_prefix
            .filter(|prefix| {
                !prefix.is_empty()
                    && word_prefixes
                        .iter()
                        .any(|candidate| candidate.as_bytes() == *prefix)
            })
            .or_else(|| {
                word_prefixes
                    .iter()
                    .find(|prefix| !prefix.is_empty() && !prefix.starts_with('\0'))
                    .map(String::as_bytes)
            })
            .map(<[u8]>::to_vec);
        let mut reserved_insertion_prefixes = owner_bindings
            .iter()
            .map(|(prefix, _)| prefix.as_bytes().to_vec())
            .collect::<Vec<_>>();
        reserved_insertion_prefixes.extend(
            namespace_bindings(word_prefixes)
                .into_iter()
                .map(|(prefix, _)| prefix.into_bytes()),
        );
        reserved_insertion_prefixes.sort();
        reserved_insertion_prefixes.dedup();
        Ok(CT_PageNumberType {
            start,
            raw_xml: Some(raw_xml),
            parsed_start: start,
            start_attribute_name,
            start_insertion_prefix,
            reserved_insertion_prefixes,
        })
    }

    fn push_extra_xml(&mut self, raw: Vec<u8>, slot: usize, occurrence: usize) {
        self.extra_xml.push(raw);
        self.extra_xml_positions
            .push(CT_SectPrRawPosition::Schema { slot, occurrence });
    }

    fn bind_story_reference_positions(&mut self) {
        for position in &mut self.extra_xml_positions {
            let (slot, occurrence) = match position {
                CT_SectPrRawPosition::Schema { slot, occurrence } => (*slot, *occurrence),
                _ => continue,
            };
            let (references, is_header) = match slot {
                0 => (&self.header_refs, true),
                1 => (&self.footer_refs, false),
                _ => continue,
            };
            let preceding = occurrence
                .checked_sub(1)
                .and_then(|index| CT_SectPrReferenceAnchor::at(references, index));
            let following = CT_SectPrReferenceAnchor::at(references, occurrence);
            *position = if is_header {
                CT_SectPrRawPosition::Header {
                    preceding,
                    following,
                }
            } else {
                CT_SectPrRawPosition::Footer {
                    preceding,
                    following,
                }
            };
        }
    }

    /// Schema slot and sub-slot for a `w:sectPr` child this type keeps raw.
    ///
    /// The sub-slot orders a retained child against the modeled siblings that
    /// share its slot, which is what keeps `xsd:sequence` intact now that
    /// `w:vAlign` and `w:textDirection` sit between them.
    fn raw_child_schema_slot(name: &[u8], word_prefixes: &[String]) -> Option<(usize, usize)> {
        let local = name.rsplit(|byte| *byte == b':').next().unwrap_or(name);
        if !is_word_element(name, local, word_prefixes) {
            return None;
        }
        match local {
            b"formProt" => Some((7, 0)),
            b"noEndnote" => Some((7, 1)),
            b"bidi" | b"rtlGutter" => Some((8, 1)),
            b"printerSettings" => Some((8, 2)),
            _ => None,
        }
    }

    /// Read `w:docGrid`, keeping every attribute this type does not model.
    fn parse_doc_grid(e: &BytesStart, word_prefixes: &[String]) -> Result<CT_DocGrid> {
        let mut grid = CT_DocGrid::default();
        for attribute in e.attributes() {
            let attribute = attribute?;
            let key = attribute.key.as_ref();
            let value = std::str::from_utf8(&attribute.value)?;
            let modeled = if is_word_attribute(key, b"type", word_prefixes) {
                match ST_DocGrid::from_str(value) {
                    Some(grid_type) => {
                        grid.grid_type = Some(grid_type);
                        true
                    }
                    None => false,
                }
            } else if is_word_attribute(key, b"linePitch", word_prefixes) {
                match value.parse() {
                    Ok(parsed) => {
                        grid.line_pitch = Some(Twips(parsed));
                        true
                    }
                    Err(_) => false,
                }
            } else if is_word_attribute(key, b"charSpace", word_prefixes) {
                match value.parse() {
                    Ok(parsed) => {
                        grid.char_space = Some(parsed);
                        true
                    }
                    Err(_) => false,
                }
            } else {
                false
            };
            if !modeled {
                grid.extra_attributes
                    .push((std::str::from_utf8(key)?.to_owned(), value.to_owned()));
            }
        }
        Ok(grid)
    }

    fn parse_paper_source(e: &BytesStart, word_prefixes: &[String]) -> Result<CT_PaperSource> {
        let mut source = CT_PaperSource::default();
        for attribute in e.attributes() {
            let attribute = attribute?;
            let key = attribute.key.as_ref();
            let value = std::str::from_utf8(&attribute.value)?;
            if is_word_attribute(key, b"first", word_prefixes) {
                store_unsigned_attribute(
                    value,
                    key,
                    &mut source.first,
                    &mut source.extra_attributes,
                )?;
            } else if is_word_attribute(key, b"other", word_prefixes) {
                store_unsigned_attribute(
                    value,
                    key,
                    &mut source.other,
                    &mut source.extra_attributes,
                )?;
            } else {
                source
                    .extra_attributes
                    .push((std::str::from_utf8(key)?.to_owned(), value.to_owned()));
            }
        }
        Ok(source)
    }

    fn parse_line_number(e: &BytesStart, word_prefixes: &[String]) -> Result<CT_LineNumber> {
        let mut numbering = CT_LineNumber::default();
        for attribute in e.attributes() {
            let attribute = attribute?;
            let key = attribute.key.as_ref();
            let value = std::str::from_utf8(&attribute.value)?;
            if is_word_attribute(key, b"countBy", word_prefixes) {
                store_unsigned_attribute(
                    value,
                    key,
                    &mut numbering.count_by,
                    &mut numbering.extra_attributes,
                )?;
            } else if is_word_attribute(key, b"start", word_prefixes) {
                store_unsigned_attribute(
                    value,
                    key,
                    &mut numbering.start,
                    &mut numbering.extra_attributes,
                )?;
            } else if is_word_attribute(key, b"distance", word_prefixes) {
                match value.parse() {
                    Ok(parsed) => numbering.distance = Some(Twips(parsed)),
                    Err(_) => numbering
                        .extra_attributes
                        .push((std::str::from_utf8(key)?.to_owned(), value.to_owned())),
                }
            } else if is_word_attribute(key, b"restart", word_prefixes) {
                match ST_LineNumberRestart::from_str(value) {
                    Some(parsed) => numbering.restart = Some(parsed),
                    None => numbering
                        .extra_attributes
                        .push((std::str::from_utf8(key)?.to_owned(), value.to_owned())),
                }
            } else {
                numbering
                    .extra_attributes
                    .push((std::str::from_utf8(key)?.to_owned(), value.to_owned()));
            }
        }
        Ok(numbering)
    }

    fn parse_page_border_attrs(e: &BytesStart, word_prefixes: &[String]) -> Result<CT_PageBorders> {
        let mut borders = CT_PageBorders::default();
        for attribute in e.attributes() {
            let attribute = attribute?;
            let key = attribute.key.as_ref();
            let value = std::str::from_utf8(&attribute.value)?;
            let retained = if is_word_attribute(key, b"zOrder", word_prefixes) {
                borders.z_order = ST_PageBorderZOrder::from_str(value);
                borders.z_order.is_none()
            } else if is_word_attribute(key, b"display", word_prefixes) {
                borders.display = ST_PageBorderDisplay::from_str(value);
                borders.display.is_none()
            } else if is_word_attribute(key, b"offsetFrom", word_prefixes) {
                borders.offset_from = ST_PageBorderOffset::from_str(value);
                borders.offset_from.is_none()
            } else {
                true
            };
            if retained {
                borders
                    .extra_attributes
                    .push((std::str::from_utf8(key)?.to_owned(), value.to_owned()));
            }
        }
        Ok(borders)
    }

    fn parse_page_borders(
        reader: &mut Reader<&[u8]>,
        e: &BytesStart,
        word_prefixes: &[String],
        owner_bindings: &[(String, String)],
    ) -> Result<CT_PageBorders> {
        let mut borders = Self::parse_page_border_attrs(e, word_prefixes)?;
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Empty(ref child)) => {
                    let name = child.name();
                    let prefixes = word_prefixes_at(child, word_prefixes)?;
                    let edge = |child: &BytesStart| {
                        CT_BorderEdge::from_xml_attrs_with_prefixes(child, &prefixes)
                    };
                    let slot = if is_word_element(name.as_ref(), b"top", &prefixes) {
                        Some(&mut borders.top)
                    } else if is_word_element(name.as_ref(), b"left", &prefixes) {
                        Some(&mut borders.left)
                    } else if is_word_element(name.as_ref(), b"bottom", &prefixes) {
                        Some(&mut borders.bottom)
                    } else if is_word_element(name.as_ref(), b"right", &prefixes) {
                        Some(&mut borders.right)
                    } else {
                        None
                    };
                    if let Some(slot) = slot {
                        if slot.is_some() {
                            return Err(OxmlError::InvalidValue(
                                "duplicate recognized page border edge".into(),
                            ));
                        }
                        *slot = Some(edge(child)?);
                    } else {
                        borders
                            .extra_xml
                            .push(crate::text::raw_with_external_bindings(
                                &capture_empty_element(child)?,
                                owner_bindings,
                            )?);
                    }
                }
                Ok(Event::Start(ref child)) => {
                    let prefixes = word_prefixes_at(child, word_prefixes)?;
                    let name = child.name();
                    let slot = if is_word_element(name.as_ref(), b"top", &prefixes) {
                        Some(&mut borders.top)
                    } else if is_word_element(name.as_ref(), b"left", &prefixes) {
                        Some(&mut borders.left)
                    } else if is_word_element(name.as_ref(), b"bottom", &prefixes) {
                        Some(&mut borders.bottom)
                    } else if is_word_element(name.as_ref(), b"right", &prefixes) {
                        Some(&mut borders.right)
                    } else {
                        None
                    };
                    let mut edge = if slot.is_some() {
                        Some(CT_BorderEdge::from_xml_attrs_with_prefixes(
                            child, &prefixes,
                        )?)
                    } else {
                        None
                    };
                    let mut bindings = owner_bindings.to_vec();
                    for (prefix, uri) in namespace_bindings(&prefixes) {
                        bindings.retain(|(name, _)| name != &prefix);
                        bindings.push((prefix, uri));
                    }
                    for prefix in prefixes.iter().filter(|prefix| !prefix.starts_with('\0')) {
                        if !bindings.iter().any(|(name, _)| name == prefix) {
                            bindings.push((
                                prefix.clone(),
                                "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
                                    .into(),
                            ));
                        }
                    }
                    let raw = crate::text::raw_with_external_bindings(
                        &capture_element(reader, child)?,
                        if slot.is_some() {
                            &bindings
                        } else {
                            owner_bindings
                        },
                    )?;
                    if let Some(slot) = slot {
                        if slot.is_some() {
                            return Err(OxmlError::InvalidValue(
                                "duplicate recognized page border edge".into(),
                            ));
                        }
                        let mut edge = edge.take().unwrap();
                        edge.source_xml = Some(raw);
                        *slot = Some(edge);
                    } else {
                        borders.extra_xml.push(raw);
                    }
                }
                Ok(Event::End(ref end))
                    if matches_local_name(end.name().as_ref(), b"pgBorders") =>
                {
                    break;
                }
                Ok(Event::Eof) => break,
                Err(error) => return Err(error.into()),
                _ => {}
            }
            buf.clear();
        }
        Ok(borders)
    }

    pub(crate) fn parse_note_properties(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        owner_bindings: &[(String, String)],
        tag: &[u8],
    ) -> Result<CT_NoteProperties> {
        let mut properties = CT_NoteProperties::default();
        let mut slot = 0usize;
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Empty(ref child)) => {
                    let name = child.name();
                    let prefixes = word_prefixes_at(child, word_prefixes)?;
                    let modeled = if is_word_element(name.as_ref(), b"pos", &prefixes) {
                        slot = 1;
                        sole_val_attribute(child, &prefixes)?
                            .map(|value| properties.pos = Some(value))
                    } else if is_word_element(name.as_ref(), b"numFmt", &prefixes) {
                        slot = 2;
                        sole_val_attribute(child, &prefixes)?
                            .map(|value| properties.num_fmt = Some(value))
                    } else if is_word_element(name.as_ref(), b"numStart", &prefixes) {
                        slot = 3;
                        sole_val_attribute(child, &prefixes)?
                            .and_then(|value| value.parse().ok())
                            .map(|value| properties.num_start = Some(value))
                    } else if is_word_element(name.as_ref(), b"numRestart", &prefixes) {
                        slot = 4;
                        sole_val_attribute(child, &prefixes)?
                            .map(|value| properties.num_restart = Some(value))
                    } else if is_word_element(
                        name.as_ref(),
                        if matches_local_name(tag, b"footnotePr") {
                            b"footnote".as_slice()
                        } else {
                            b"endnote".as_slice()
                        },
                        &prefixes,
                    ) {
                        slot = 5;
                        child
                            .attributes()
                            .filter_map(|attribute| attribute.ok())
                            .find(|attribute| {
                                is_word_attribute(attribute.key.as_ref(), b"id", &prefixes)
                            })
                            .and_then(|attribute| {
                                std::str::from_utf8(&attribute.value)
                                    .ok()
                                    .and_then(|value| value.parse::<i32>().ok())
                            })
                            .map(|id| properties.special_references.push(id))
                    } else {
                        None
                    };
                    if modeled.is_none() {
                        properties.extra_xml.push((
                            slot,
                            crate::text::raw_with_external_bindings(
                                &capture_empty_element(child)?,
                                owner_bindings,
                            )?,
                        ));
                    }
                }
                Ok(Event::Start(ref child)) => {
                    properties.extra_xml.push((
                        slot,
                        crate::text::raw_with_external_bindings(
                            &capture_element(reader, child)?,
                            owner_bindings,
                        )?,
                    ));
                }
                Ok(Event::End(ref end)) if matches_local_name(end.name().as_ref(), tag) => break,
                Ok(Event::Eof) => break,
                Err(error) => return Err(error.into()),
                _ => {}
            }
            buf.clear();
        }
        Ok(properties)
    }

    fn parse_cols_attrs(e: &BytesStart, word_prefixes: &[String]) -> Result<CT_Columns> {
        let mut cols = CT_Columns::default();
        for attr in e.attributes() {
            let attr = attr?;
            let key = attr.key.as_ref();
            let val_str = std::str::from_utf8(&attr.value)?;
            if is_word_attribute(key, b"num", word_prefixes) {
                cols.num = Some(SectionScalar::count(val_str)?);
            } else if is_word_attribute(key, b"space", word_prefixes) {
                cols.space = Some(Twips(parse_integer_measurement(
                    e.name().as_ref(),
                    key,
                    val_str,
                )?));
            } else if is_word_attribute(key, b"equalWidth", word_prefixes) {
                cols.equal_width = Some(val_str == "1" || val_str == "true");
            } else if is_word_attribute(key, b"sep", word_prefixes) {
                cols.sep = Some(val_str == "1" || val_str == "true");
            }
        }
        Ok(cols)
    }

    fn parse_cols_empty(e: &BytesStart, word_prefixes: &[String]) -> Result<CT_Columns> {
        Self::parse_cols_attrs(e, word_prefixes)
    }

    fn parse_cols_start(
        reader: &mut Reader<&[u8]>,
        e: &BytesStart,
        word_prefixes: &[String],
    ) -> Result<CT_Columns> {
        let mut cols = Self::parse_cols_attrs(e, word_prefixes)?;
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Empty(ref e)) => {
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    if !is_word_element(e.name().as_ref(), b"col", &prefixes) {
                        buf.clear();
                        continue;
                    }
                    let mut width = None;
                    let mut space = None;
                    for attr in e.attributes() {
                        let attr = attr?;
                        let key = attr.key.as_ref();
                        let value = std::str::from_utf8(&attr.value)?;
                        let measurement =
                            || parse_integer_measurement(e.name().as_ref(), key, value);
                        if is_word_attribute(key, b"w", &prefixes) {
                            width = Some(Twips(measurement()?));
                        } else if is_word_attribute(key, b"space", &prefixes) {
                            space = Some(Twips(measurement()?));
                        }
                    }
                    cols.columns.push(CT_Column { width, space });
                }
                Ok(Event::End(ref e)) if matches_local_name(e.name().as_ref(), b"cols") => {
                    break;
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        Ok(cols)
    }

    pub fn to_xml<W: std::io::Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.write_xml_role(writer, crate::text::XmlWriteRole::Publication, None)
    }

    pub(crate) fn write_xml_role<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: crate::text::XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
    ) -> Result<()> {
        self.write_xml_role_in(writer, role, source_context, None)
    }

    pub(crate) fn write_xml_role_in<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: crate::text::XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let mut buf = itoa::Buffer::new();
        let mut root = crate::text::FieldXmlStart::new("w:sectPr", emission)?;
        for (raw, position) in self.extra_xml.iter().zip(&self.extra_xml_positions) {
            if Self::raw_position_is_root_attributes(position, raw) {
                root.root_record(raw, None, role)?;
            }
        }

        // Source composition resolves generated qualifiers from the explicit
        // original consumer frame plus this section's own retained root
        // declarations. Publication keeps canonical names and ambient facts.
        // Retained records are already staged on the root, so the existing
        // qualifier helper overlays them without copying inherited URIs.
        let word_prefix = if role == crate::text::XmlWriteRole::Source {
            std::borrow::Cow::Owned(root.qualify(&[], role, source_context)?)
        } else {
            std::borrow::Cow::Borrowed("w")
        };
        let source_declarations = if role == crate::text::XmlWriteRole::Source {
            Some(crate::text::raw_namespace_declarations(root.element())?)
        } else {
            None
        };
        let child_context = source_declarations.as_ref().map(|declarations| {
            borrowed_source_overlay(source_context.unwrap_or(&[]), declarations)
        });
        let mut relationship_scratch = [0u8; 6 + std::mem::size_of::<usize>() * 3];
        let (relationship_prefix, declare_relationship) =
            if role == crate::text::XmlWriteRole::Source {
                source_relationship_prefix(
                    child_context.as_deref().unwrap_or(&[]),
                    &mut relationship_scratch,
                )
            } else {
                ("r", false)
            };
        root.write(writer, false)?;
        let ordered_raw = self.extra_xml_positions.len() == self.extra_xml.len();
        if ordered_raw {
            self.write_story_reference_boundary(writer, &self.header_refs, true, 0)?;
        }

        // headerReference elements
        for (index, hdr) in self.header_refs.iter().enumerate() {
            let mut e =
                crate::text::FieldXmlStart::word(&word_prefix, "headerReference", emission)?;
            e.push_word(&word_prefix, "type", hdr.hdr_ftr_type.to_str())?;
            if declare_relationship {
                e.push_word("xmlns", relationship_prefix, crate::namespace::R_NS)?;
            }
            e.push_word(relationship_prefix, "id", hdr.rel_id.as_str())?;
            e.write(writer, true)?;
            if ordered_raw {
                self.write_story_reference_boundary(writer, &self.header_refs, true, index + 1)?;
            }
        }
        if ordered_raw {
            self.write_story_reference_boundary(writer, &self.footer_refs, false, 0)?;
        }

        // footerReference elements
        for (index, ftr) in self.footer_refs.iter().enumerate() {
            let mut e =
                crate::text::FieldXmlStart::word(&word_prefix, "footerReference", emission)?;
            e.push_word(&word_prefix, "type", ftr.hdr_ftr_type.to_str())?;
            if declare_relationship {
                e.push_word("xmlns", relationship_prefix, crate::namespace::R_NS)?;
            }
            e.push_word(relationship_prefix, "id", ftr.rel_id.as_str())?;
            e.write(writer, true)?;
            if ordered_raw {
                self.write_story_reference_boundary(writer, &self.footer_refs, false, index + 1)?;
            }
        }
        // footnotePr and endnotePr. Their effect on notes is not applied yet.
        if let Some(footnote_pr) = &self.footnote_pr {
            if role == crate::text::XmlWriteRole::Source {
                footnote_pr.write_xml_role_in(
                    writer,
                    "footnotePr",
                    role,
                    child_context.as_deref(),
                    emission,
                )?;
            } else {
                footnote_pr.write_xml_role_in(
                    writer,
                    "w:footnotePr",
                    role,
                    child_context.as_deref(),
                    emission,
                )?;
            }
        }
        if ordered_raw {
            self.write_raw_position(writer, 2, 0)?;
        }
        if let Some(endnote_pr) = &self.endnote_pr {
            if role == crate::text::XmlWriteRole::Source {
                endnote_pr.write_xml_role_in(
                    writer,
                    "endnotePr",
                    role,
                    child_context.as_deref(),
                    emission,
                )?;
            } else {
                endnote_pr.write_xml_role_in(
                    writer,
                    "w:endnotePr",
                    role,
                    child_context.as_deref(),
                    emission,
                )?;
            }
        }
        if ordered_raw {
            self.write_raw_position(writer, 2, 1)?;
        }

        // type (section break type)
        if let Some(st) = self.section_type {
            let mut e = crate::text::FieldXmlStart::word(&word_prefix, "type", emission)?;
            e.push_word(&word_prefix, "val", st.to_str())?;
            e.write(writer, true)?;
        }
        if ordered_raw {
            self.write_raw_position(writer, 3, 0)?;
        }

        // pgSz
        if self.page_width.is_some() || self.page_height.is_some() || self.orientation.is_some() {
            let mut e = crate::text::FieldXmlStart::word(&word_prefix, "pgSz", emission)?;
            if let Some(w) = self.page_width {
                e.push_word(&word_prefix, "w", buf.format(w.0))?;
            }
            if let Some(h) = self.page_height {
                e.push_word(&word_prefix, "h", buf.format(h.0))?;
            }
            if let Some(orient) = self.orientation
                && orient == ST_PageOrientation::Landscape
            {
                e.push_word(&word_prefix, "orient", orient.to_str())?;
            }
            e.write(writer, true)?;
        }
        if ordered_raw {
            self.write_raw_position(writer, 4, 0)?;
        }

        // pgMar
        if self.margin_top.is_some()
            || self.margin_right.is_some()
            || self.margin_bottom.is_some()
            || self.margin_left.is_some()
            || self.gutter.is_some()
            || self.header_distance.is_some()
            || self.footer_distance.is_some()
        {
            let mut e = crate::text::FieldXmlStart::word(&word_prefix, "pgMar", emission)?;
            if let Some(t) = self.margin_top {
                e.push_word(&word_prefix, "top", buf.format(t.0))?;
            }
            if let Some(r) = self.margin_right {
                e.push_word(&word_prefix, "right", buf.format(r.0))?;
            }
            if let Some(b) = self.margin_bottom {
                e.push_word(&word_prefix, "bottom", buf.format(b.0))?;
            }
            if let Some(l) = self.margin_left {
                e.push_word(&word_prefix, "left", buf.format(l.0))?;
            }
            if let Some(g) = self.gutter {
                e.push_word(&word_prefix, "gutter", buf.format(g.0))?;
            }
            if let Some(h) = self.header_distance {
                e.push_word(&word_prefix, "header", buf.format(h.0))?;
            }
            if let Some(f) = self.footer_distance {
                e.push_word(&word_prefix, "footer", buf.format(f.0))?;
            }
            e.write(writer, true)?;
        }
        if ordered_raw {
            self.write_raw_position(writer, 5, 0)?;
        }

        // paperSrc. A printer tray, so pagination is identical with and without it.
        if let Some(paper_source) = &self.paper_source {
            let mut e = crate::text::FieldXmlStart::new("w:paperSrc", emission)?;
            for (name, value) in &paper_source.extra_attributes {
                e.push_raw(name, value)?;
            }
            let word_prefix = if role == crate::text::XmlWriteRole::Source {
                std::borrow::Cow::Owned(e.qualify(&[], role, child_context.as_deref())?)
            } else {
                word_prefix.clone()
            };
            if let Some(first) = paper_source.first {
                e.push_word(&word_prefix, "first", buf.format(first))?;
            }
            if let Some(other) = paper_source.other {
                e.push_word(&word_prefix, "other", buf.format(other))?;
            }
            e.write(writer, true)?;
        }
        if ordered_raw {
            self.write_raw_position(writer, 5, 1)?;
        }

        // pgBorders
        if let Some(page_borders) = &self.page_borders {
            page_borders.write_xml_role_in(writer, role, child_context.as_deref(), emission)?;
        }
        if ordered_raw {
            self.write_raw_position(writer, 5, 2)?;
        }

        // lnNumType
        if let Some(line_numbers) = &self.line_numbers {
            let mut e = crate::text::FieldXmlStart::new("w:lnNumType", emission)?;
            for (name, value) in &line_numbers.extra_attributes {
                e.push_raw(name, value)?;
            }
            let word_prefix = if role == crate::text::XmlWriteRole::Source {
                std::borrow::Cow::Owned(e.qualify(&[], role, child_context.as_deref())?)
            } else {
                word_prefix.clone()
            };
            if let Some(count_by) = line_numbers.count_by {
                e.push_word(&word_prefix, "countBy", buf.format(count_by))?;
            }
            if let Some(start) = line_numbers.start {
                e.push_word(&word_prefix, "start", buf.format(start))?;
            }
            if let Some(distance) = line_numbers.distance {
                e.push_word(&word_prefix, "distance", buf.format(distance.0))?;
            }
            if let Some(restart) = line_numbers.restart {
                e.push_word(&word_prefix, "restart", restart.to_str())?;
            }
            e.write(writer, true)?;
        }
        if ordered_raw {
            self.write_raw_position(writer, 5, 3)?;
        }

        // pgNumType. M24 format and chapter attributes stay byte-exact unless start changes.
        if let Some(page_number) = &self.page_number {
            page_number.write_xml_role_in(writer, role, child_context.as_deref(), emission)?;
        }
        if ordered_raw {
            self.write_raw_position(writer, 6, 1)?;
        }

        // cols
        if let Some(ref cols) = self.columns {
            if cols.columns.is_empty() {
                // Simple equal-width columns
                let mut e = crate::text::FieldXmlStart::word(&word_prefix, "cols", emission)?;
                if let Some(num) = cols.num {
                    e.push_word(&word_prefix, "num", buf.format(num))?;
                }
                if let Some(space) = cols.space {
                    e.push_word(&word_prefix, "space", buf.format(space.0))?;
                }
                if let Some(eq) = cols.equal_width
                    && !eq
                {
                    e.push_word(&word_prefix, "equalWidth", "0")?;
                }
                if let Some(sep) = cols.sep
                    && sep
                {
                    e.push_word(&word_prefix, "sep", "1")?;
                }
                e.write(writer, true)?;
            } else {
                // Individual column definitions
                let mut e = crate::text::FieldXmlStart::word(&word_prefix, "cols", emission)?;
                if let Some(num) = cols.num {
                    e.push_word(&word_prefix, "num", buf.format(num))?;
                }
                if let Some(eq) = cols.equal_width {
                    e.push_word(&word_prefix, "equalWidth", if eq { "1" } else { "0" })?;
                }
                if let Some(sep) = cols.sep
                    && sep
                {
                    e.push_word(&word_prefix, "sep", "1")?;
                }
                e.write(writer, false)?;

                for col in &cols.columns {
                    let mut ce = crate::text::FieldXmlStart::word(&word_prefix, "col", emission)?;
                    if let Some(w) = col.width {
                        ce.push_word(&word_prefix, "w", buf.format(w.0))?;
                    }
                    if let Some(s) = col.space {
                        ce.push_word(&word_prefix, "space", buf.format(s.0))?;
                    }
                    ce.write(writer, true)?;
                }

                if let Some(emission) = emission {
                    emission.write_word_end(writer, &word_prefix, "cols")?;
                } else {
                    writer.write_event(Event::End(BytesEnd::new(format!("{word_prefix}:cols"))))?;
                }
            }
        }
        if ordered_raw {
            self.write_raw_position(writer, 7, 0)?;
        }

        // vAlign
        if let Some(vertical_alignment) = self.vertical_alignment {
            let mut e = crate::text::FieldXmlStart::word(&word_prefix, "vAlign", emission)?;
            e.push_word(&word_prefix, "val", vertical_alignment.to_str())?;
            e.write(writer, true)?;
        }
        if ordered_raw {
            self.write_raw_position(writer, 7, 1)?;
        }

        // titlePg
        if let Some(title_pg) = self.title_pg {
            let mut e = crate::text::FieldXmlStart::word(&word_prefix, "titlePg", emission)?;
            if !title_pg {
                e.push_word(&word_prefix, "val", "0")?;
            }
            e.write(writer, true)?;
        }

        if ordered_raw {
            self.write_raw_position(writer, 8, 0)?;
        }

        // textDirection. Authored and preserved here, projected by layout.
        if let Some(text_direction) = &self.text_direction {
            let mut e = crate::text::FieldXmlStart::word(&word_prefix, "textDirection", emission)?;
            e.push_word(&word_prefix, "val", text_direction.as_str())?;
            e.write(writer, true)?;
        }

        // docGrid. The grid type, line pitch and character space this crate
        // models, with any producer attribute it does not written first. A
        // legacy caller that populates only `extra_xml` gets the modeled
        // element ahead of its raw dump, which is the order the rest of this
        // serialiser already writes a modeled child in.
        let mut write_doc_grid = |writer: &mut Writer<W>| -> Result<()> {
            if let Some(doc_grid) = &self.doc_grid {
                let mut e = crate::text::FieldXmlStart::new("w:docGrid", emission)?;
                for (name, value) in &doc_grid.extra_attributes {
                    e.push_raw(name, value)?;
                }
                let word_prefix = if role == crate::text::XmlWriteRole::Source {
                    std::borrow::Cow::Owned(e.qualify(&[], role, child_context.as_deref())?)
                } else {
                    word_prefix.clone()
                };
                if let Some(grid_type) = doc_grid.grid_type {
                    e.push_word(&word_prefix, "type", grid_type.to_str())?;
                }
                if let Some(line_pitch) = doc_grid.line_pitch {
                    e.push_word(&word_prefix, "linePitch", buf.format(line_pitch.0))?;
                }
                if let Some(char_space) = doc_grid.char_space {
                    e.push_word(&word_prefix, "charSpace", buf.format(char_space))?;
                }
                e.write(writer, true)?;
            }
            Ok(())
        };
        if ordered_raw {
            self.write_raw_position(writer, 8, 1)?;
            write_doc_grid(writer)?;
            self.write_raw_position(writer, 8, 2)?;
            self.write_raw_position(writer, 9, 0)?;
        } else {
            write_doc_grid(writer)?;
            // Legacy callers that populate only `extra_xml` retain the previous position.
            for raw in &self.extra_xml {
                if is_root_attribute_record(raw) {
                    continue;
                }
                writer.get_mut().write_all(raw)?;
            }
        }
        if let Some(change) = &self.change {
            change.write_xml_with_role_override(
                writer,
                None,
                role,
                child_context.as_deref(),
                emission,
            )?;
        }

        if let Some(emission) = emission {
            emission.write_word_end(writer, &word_prefix, "sectPr")?;
        } else {
            writer.write_event(Event::End(BytesEnd::new(format!("{word_prefix}:sectPr"))))?;
        }
        Ok(())
    }

    fn write_raw_position<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        slot: usize,
        occurrence: usize,
    ) -> Result<()> {
        for (position, raw) in self.extra_xml_positions.iter().zip(&self.extra_xml) {
            if matches!(
                position,
                CT_SectPrRawPosition::Schema {
                    slot: candidate_slot,
                    occurrence: candidate_occurrence,
                } if (*candidate_slot, *candidate_occurrence) == (slot, occurrence)
            ) {
                writer.get_mut().write_all(raw)?;
            }
        }
        Ok(())
    }

    fn write_story_reference_boundary<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        references: &[HdrFtrRef],
        header: bool,
        boundary: usize,
    ) -> Result<()> {
        for (position, raw) in self.extra_xml_positions.iter().zip(&self.extra_xml) {
            let anchored = match position {
                CT_SectPrRawPosition::Header {
                    preceding,
                    following,
                } if header => Some((preceding, following)),
                CT_SectPrRawPosition::Footer {
                    preceding,
                    following,
                } if !header => Some((preceding, following)),
                CT_SectPrRawPosition::Schema { slot, occurrence }
                    if (*slot == 0 && header) || (*slot == 1 && !header) =>
                {
                    if (*occurrence).min(references.len()) == boundary {
                        writer.get_mut().write_all(raw)?;
                    }
                    None
                }
                _ => None,
            };
            let Some((preceding, following)) = anchored else {
                continue;
            };
            let resolved = following
                .as_ref()
                .and_then(|reference| reference.resolve(references))
                .or_else(|| {
                    preceding
                        .as_ref()
                        .and_then(|reference| reference.resolve(references).map(|index| index + 1))
                })
                .unwrap_or(references.len());
            if resolved == boundary {
                writer.get_mut().write_all(raw)?;
            }
        }
        Ok(())
    }
}

impl CT_PageBorders {
    /// Write `w:pgBorders` with its edges in `xsd:sequence` order.
    ///
    /// Retained attributes are written ahead of the modeled ones and retained
    /// children after the modeled edges, which is what `CT_BorderEdge` does.
    fn write_xml_role_in<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: crate::text::XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let mut e = crate::text::FieldXmlStart::new("w:pgBorders", emission)?;
        for (name, value) in &self.extra_attributes {
            e.push_raw(name, value)?;
        }
        let word_prefix = if role == crate::text::XmlWriteRole::Source {
            std::borrow::Cow::Owned(e.qualify(&[], role, source_context)?)
        } else {
            std::borrow::Cow::Borrowed("w")
        };
        let page_declarations = if role == crate::text::XmlWriteRole::Source {
            Some(crate::text::raw_namespace_declarations(e.element())?)
        } else {
            None
        };
        let edge_context = page_declarations.as_ref().map(|declarations| {
            borrowed_source_overlay(source_context.unwrap_or(&[]), declarations)
        });
        if let Some(z_order) = self.z_order {
            e.push_word(&word_prefix, "zOrder", z_order.to_str())?;
        }
        if let Some(display) = self.display {
            e.push_word(&word_prefix, "display", display.to_str())?;
        }
        if let Some(offset_from) = self.offset_from {
            e.push_word(&word_prefix, "offsetFrom", offset_from.to_str())?;
        }
        if self.top.is_none()
            && self.left.is_none()
            && self.bottom.is_none()
            && self.right.is_none()
            && self.extra_xml.is_empty()
        {
            e.write(writer, true)?;
            return Ok(());
        }
        e.write(writer, false)?;
        for (edge, local, qualified) in [
            (&self.top, "top", "w:top"),
            (&self.left, "left", "w:left"),
            (&self.bottom, "bottom", "w:bottom"),
            (&self.right, "right", "w:right"),
        ] {
            if let Some(edge) = edge {
                if role == crate::text::XmlWriteRole::Source {
                    edge.write_xml_in(writer, local, role, edge_context.as_deref(), emission)?;
                } else {
                    edge.write_xml_in(writer, qualified, role, edge_context.as_deref(), emission)?;
                }
            }
        }
        for raw in &self.extra_xml {
            writer.get_mut().write_all(raw)?;
        }
        if let Some(emission) = emission {
            emission.write_word_end(writer, &word_prefix, "pgBorders")?;
        } else {
            writer.write_event(Event::End(BytesEnd::new(format!(
                "{word_prefix}:pgBorders"
            ))))?;
        }
        Ok(())
    }
}

impl CT_NoteProperties {
    /// Write section note properties under `tag`, in `xsd:sequence` order.
    pub(crate) fn to_xml<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        tag: &str,
    ) -> Result<()> {
        self.write_xml_role_in(
            writer,
            tag,
            crate::text::XmlWriteRole::Publication,
            None,
            None,
        )
    }

    fn write_xml_role_in<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        name: &str,
        role: crate::text::XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let source = role == crate::text::XmlWriteRole::Source;
        let local = if source {
            name
        } else {
            name.rsplit(':').next().unwrap_or(name)
        };
        let mut start = if source {
            crate::text::FieldXmlStart::word("w", name, emission)?
        } else {
            crate::text::FieldXmlStart::new(name, emission)?
        };
        let word_prefix = if source {
            std::borrow::Cow::Owned(start.qualify(&[], role, source_context)?)
        } else {
            std::borrow::Cow::Borrowed("w")
        };
        self.write_xml_core(
            writer,
            start,
            &word_prefix,
            local,
            (!source).then_some(name),
            emission,
        )
    }

    fn write_xml_core<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        start: crate::text::FieldXmlStart<'_, '_>,
        word_prefix: &str,
        local: &str,
        publication_end: Option<&str>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        if self.pos.is_none()
            && self.num_fmt.is_none()
            && self.num_start.is_none()
            && self.num_restart.is_none()
            && self.special_references.is_empty()
            && self.extra_xml.is_empty()
        {
            start.write(writer, true)?;
            return Ok(());
        }
        start.write(writer, false)?;
        self.write_retained_children(writer, 0)?;
        if let Some(pos) = &self.pos {
            let mut e = crate::text::FieldXmlStart::word(word_prefix, "pos", emission)?;
            e.push_word(word_prefix, "val", pos.as_str())?;
            e.write(writer, true)?;
        }
        self.write_retained_children(writer, 1)?;
        if let Some(num_fmt) = &self.num_fmt {
            let mut e = crate::text::FieldXmlStart::word(word_prefix, "numFmt", emission)?;
            e.push_word(word_prefix, "val", num_fmt.as_str())?;
            e.write(writer, true)?;
        }
        self.write_retained_children(writer, 2)?;
        if let Some(num_start) = self.num_start {
            let mut buf = itoa::Buffer::new();
            let mut e = crate::text::FieldXmlStart::word(word_prefix, "numStart", emission)?;
            e.push_word(word_prefix, "val", buf.format(num_start))?;
            e.write(writer, true)?;
        }
        self.write_retained_children(writer, 3)?;
        if let Some(num_restart) = &self.num_restart {
            let mut e = crate::text::FieldXmlStart::word(word_prefix, "numRestart", emission)?;
            e.push_word(word_prefix, "val", num_restart.as_str())?;
            e.write(writer, true)?;
        }
        self.write_retained_children(writer, 4)?;
        let child_local = if local.ends_with("footnotePr") {
            "footnote"
        } else {
            "endnote"
        };
        for id in &self.special_references {
            let mut buf = itoa::Buffer::new();
            let mut child = crate::text::FieldXmlStart::word(word_prefix, child_local, emission)?;
            child.push_word(word_prefix, "id", buf.format(*id))?;
            child.write(writer, true)?;
        }
        self.write_retained_children(writer, 5)?;
        if let Some(name) = publication_end {
            if let Some(emission) = emission {
                emission.write_end_name(writer, name)?;
            } else {
                writer.write_event(Event::End(BytesEnd::new(name)))?;
            }
        } else if let Some(emission) = emission {
            emission.write_word_end(writer, word_prefix, local)?;
        } else {
            writer.write_event(Event::End(BytesEnd::new(format!("{word_prefix}:{local}"))))?;
        }
        Ok(())
    }

    fn write_retained_children<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        slot: usize,
    ) -> Result<()> {
        for (candidate, raw) in &self.extra_xml {
            if *candidate == slot {
                writer.get_mut().write_all(raw)?;
            }
        }
        Ok(())
    }
}

impl CT_PageNumberType {
    fn write_xml_role_in<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: crate::text::XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        if let Some(raw) = &self.raw_xml {
            if self.start == self.parsed_start {
                writer.get_mut().write_all(raw)?;
                return Ok(());
            }
            let produced = replace_page_number_start_in(
                raw,
                self.start,
                self.start_attribute_name.as_deref(),
                self.start_insertion_prefix.as_deref(),
                &self.reserved_insertion_prefixes,
                emission,
            )?;
            let _transport = emission
                .map(|emission| emission.prepaid_xml_transport(&produced))
                .transpose()?;
            writer.get_mut().write_all(&produced)?;
            return Ok(());
        }

        let mut element = crate::text::FieldXmlStart::new("w:pgNumType", emission)?;
        let word_prefix = if role == crate::text::XmlWriteRole::Source {
            std::borrow::Cow::Owned(element.qualify(&[], role, source_context)?)
        } else {
            std::borrow::Cow::Borrowed("w")
        };
        if let Some(start) = self.start {
            let mut buf = itoa::Buffer::new();
            element.push_word(&word_prefix, "start", buf.format(start))?;
        }
        element.write(writer, true)?;
        Ok(())
    }
}

/// Store a parsed unsigned attribute, or retain it verbatim when the source
/// value falls outside what the field models, so nothing is dropped on write.
fn store_unsigned_attribute(
    value: &str,
    key: &[u8],
    field: &mut Option<u32>,
    retained: &mut Vec<(String, String)>,
) -> Result<()> {
    match value.parse() {
        Ok(parsed) => *field = Some(parsed),
        Err(_) => retained.push((std::str::from_utf8(key)?.to_owned(), value.to_owned())),
    }
    Ok(())
}

/// Return the `w:val` of an element whose only attribute is `w:val`.
///
/// `None` means the element carries something this crate does not model, and
/// the caller keeps the element raw rather than typing part of it.
fn sole_val_attribute(e: &BytesStart<'_>, word_prefixes: &[String]) -> Result<Option<String>> {
    let mut value = None;
    for attribute in e.attributes() {
        let attribute = attribute?;
        if !is_word_attribute(attribute.key.as_ref(), b"val", word_prefixes) {
            return Ok(None);
        }
        value = Some(
            attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, e.decoder())?
                .into_owned(),
        );
    }
    Ok(value)
}

/// Overlay owned physical declarations on a borrowed consumer frame without
/// copying inherited prefix or URI strings. Later local entries shadow earlier
/// base entries while preserving their lexical order.
fn borrowed_source_overlay<'a>(
    base: &'a [(&'a str, &'a str)],
    local: &'a [(String, String)],
) -> Vec<(&'a str, &'a str)> {
    let mut overlay = base
        .iter()
        .copied()
        .filter(|(prefix, _)| !local.iter().any(|(known, _)| known == prefix))
        .collect::<Vec<_>>();
    overlay.extend(
        local
            .iter()
            .map(|(prefix, namespace)| (prefix.as_str(), namespace.as_str())),
    );
    overlay
}

/// Resolve a source relationship qualifier without rebinding a local `r`
/// shadow. An existing relationship alias wins, then the free preferred
/// prefix, then a deterministic bounded `rdocxR` alias declared by the caller.
fn source_relationship_prefix<'a>(
    bindings: &'a [(&str, &str)],
    scratch: &'a mut [u8; 6 + std::mem::size_of::<usize>() * 3],
) -> (&'a str, bool) {
    let relationships = crate::namespace::R_NS;
    if bindings
        .iter()
        .any(|(prefix, namespace)| *prefix == "r" && *namespace == relationships)
    {
        return ("r", false);
    }
    if let Some((prefix, _)) = bindings
        .iter()
        .find(|(prefix, namespace)| !prefix.is_empty() && *namespace == relationships)
    {
        return (*prefix, false);
    }
    if !bindings.iter().any(|(prefix, _)| *prefix == "r") {
        return ("r", true);
    }
    let mut suffix = 0usize;
    let mut decimal = itoa::Buffer::new();
    loop {
        let digits = if suffix == 0 {
            ""
        } else {
            decimal.format(suffix)
        };
        let occupied = bindings.iter().any(|(prefix, _)| {
            prefix.as_bytes().strip_prefix(b"rdocxR") == Some(digits.as_bytes())
        });
        if !occupied {
            scratch[..6].copy_from_slice(b"rdocxR");
            scratch[6..6 + digits.len()].copy_from_slice(digits.as_bytes());
            return (
                std::str::from_utf8(&scratch[..6 + digits.len()])
                    .expect("ASCII relationship prefix"),
                true,
            );
        }
        suffix += 1;
    }
}

fn replace_page_number_start_in(
    raw: &[u8],
    start: Option<u32>,
    attribute_name: Option<&[u8]>,
    insertion_prefix: Option<&[u8]>,
    reserved_prefixes: &[Vec<u8>],
    emission: Option<&crate::text::FieldXmlEmission>,
) -> Result<Vec<u8>> {
    let mut decimal = itoa::Buffer::new();
    let replacement = start.map(|value| decimal.format(value) as &str);
    let Some((tag_end, attributes)) = start_tag_attributes(raw) else {
        if let Some(emission) = emission {
            emission.reserve_inner_xml(raw.len())?;
        }
        return Ok(raw.to_vec());
    };
    let overflow = || OxmlError::InvalidValue("page number source output overflow".into());
    if let Some(name) = attribute_name
        && let Some((_, value, full)) = attributes
            .iter()
            .find(|(candidate, _, _)| &raw[candidate.clone()] == name)
    {
        let range = if replacement.is_some() { value } else { full };
        let length = raw
            .len()
            .checked_sub(range.len())
            .and_then(|n| n.checked_add(replacement.map_or(0, str::len)))
            .ok_or_else(overflow)?;
        if let Some(emission) = emission {
            emission.reserve_inner_xml(length)?;
        }
        let mut output = Vec::with_capacity(length);
        output.extend_from_slice(&raw[..range.start]);
        if let Some(value) = replacement {
            output.extend_from_slice(value.as_bytes());
        }
        output.extend_from_slice(&raw[range.end..]);
        return Ok(output);
    }
    let Some(value) = replacement else {
        if let Some(emission) = emission {
            emission.reserve_inner_xml(raw.len())?;
        }
        return Ok(raw.to_vec());
    };
    let insert_at = raw[..tag_end]
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace() && *byte != b'/')
        .map_or(tag_end, |index| index + 1);
    let mut prefix_scratch = [0u8; 9 + std::mem::size_of::<usize>() * 3];
    let mut suffix_decimal = itoa::Buffer::new();
    let prefix = if let Some(prefix) = insertion_prefix {
        prefix
    } else {
        let index = unused_page_number_prefix_index(raw, tag_end, &attributes, reserved_prefixes);
        let digits = if index == 0 {
            ""
        } else {
            suffix_decimal.format(index)
        };
        prefix_scratch[..9].copy_from_slice(b"rdocxWord");
        prefix_scratch[9..9 + digits.len()].copy_from_slice(digits.as_bytes());
        &prefix_scratch[..9 + digits.len()]
    };
    // This is exactly String::from_utf8_lossy's public primitive, without a
    // temporary lossy prefix String before the actual output reservation.
    let prefix_len = prefix
        .utf8_chunks()
        .try_fold(0usize, |sum, chunk| {
            sum.checked_add(chunk.valid().len())
                .and_then(|n| n.checked_add(if chunk.invalid().is_empty() { 0 } else { 3 }))
        })
        .ok_or_else(overflow)?;
    let mut added = 10usize
        .checked_add(prefix_len)
        .and_then(|n| n.checked_add(value.len()))
        .ok_or_else(overflow)?;
    if insertion_prefix.is_none() {
        added = added
            .checked_add(10)
            .and_then(|n| n.checked_add(prefix_len))
            .and_then(|n| n.checked_add(W_NS.len()))
            .ok_or_else(overflow)?;
    }
    let length = raw.len().checked_add(added).ok_or_else(overflow)?;
    if let Some(emission) = emission {
        emission.reserve_inner_xml(length)?;
    }
    let mut output = Vec::with_capacity(length);
    output.extend_from_slice(&raw[..insert_at]);
    if insertion_prefix.is_none() {
        output.extend_from_slice(b" xmlns:");
        append_lossy_prefix(&mut output, prefix);
        output.extend_from_slice(b"=\"");
        output.extend_from_slice(W_NS.as_bytes());
        output.push(b'"');
    }
    output.push(b' ');
    append_lossy_prefix(&mut output, prefix);
    output.extend_from_slice(b":start=\"");
    output.extend_from_slice(value.as_bytes());
    output.push(b'"');
    output.extend_from_slice(&raw[insert_at..]);
    debug_assert_eq!(output.len(), length);
    Ok(output)
}

fn append_lossy_prefix(output: &mut Vec<u8>, prefix: &[u8]) {
    for chunk in prefix.utf8_chunks() {
        output.extend_from_slice(chunk.valid().as_bytes());
        if !chunk.invalid().is_empty() {
            output.extend_from_slice("�".as_bytes());
        }
    }
}

fn unused_page_number_prefix_index(
    raw: &[u8],
    tag_end: usize,
    attributes: &[AttributeSpans],
    reserved_prefixes: &[Vec<u8>],
) -> usize {
    let mut decimal = itoa::Buffer::new();
    for suffix in 0usize.. {
        let digits = if suffix == 0 {
            ""
        } else {
            decimal.format(suffix)
        };
        let exact = |name: &[u8]| name.strip_prefix(b"rdocxWord") == Some(digits.as_bytes());
        let qualified = |name: &[u8]| {
            name.strip_prefix(b"rdocxWord")
                .and_then(|tail| tail.strip_prefix(digits.as_bytes()))
                .is_some_and(|tail| tail.starts_with(b":"))
        };
        let used = reserved_prefixes.iter().any(|candidate| exact(candidate))
            || attributes.iter().any(|(name, _, _)| {
                let name = &raw[name.clone()];
                name.strip_prefix(b"xmlns:").is_some_and(exact) || qualified(name)
            })
            || qualified(&raw[1..tag_end]);
        if !used {
            return suffix;
        }
    }
    unreachable!("the finite start tag cannot use every generated prefix")
}

fn attribute_in_namespace(
    name: &[u8],
    local_name: &[u8],
    namespace: &str,
    scope: &[String],
) -> bool {
    let Some(separator) = name.iter().position(|byte| *byte == b':') else {
        return false;
    };
    if name.get(separator + 1..) != Some(local_name) {
        return false;
    }
    let prefix = &name[..separator];
    scope.iter().any(|binding| {
        binding
            .strip_prefix('\0')
            .and_then(|binding| binding.split_once('\0'))
            .is_some_and(|(candidate, value)| candidate.as_bytes() == prefix && value == namespace)
    })
}

fn page_size_attributes_are_modeled(
    element: &BytesStart<'_>,
    word_prefixes: &[String],
) -> Result<bool> {
    for attribute in element.attributes() {
        let attribute = attribute?;
        let key = attribute.key.as_ref();
        if namespace_declaration(key) {
            continue;
        }
        let value = std::str::from_utf8(&attribute.value)?;
        if is_word_attribute(key, b"w", word_prefixes)
            || is_word_attribute(key, b"h", word_prefixes)
        {
            if parse_integer_measurement::<i32>(element.name().as_ref(), key, value).is_err() {
                return Ok(false);
            }
        } else if is_word_attribute(key, b"orient", word_prefixes) {
            if ST_PageOrientation::from_str(value).is_err() {
                return Ok(false);
            }
        } else {
            return Ok(false);
        }
    }
    Ok(true)
}

fn page_margin_attributes_are_modeled(
    element: &BytesStart<'_>,
    word_prefixes: &[String],
) -> Result<bool> {
    for attribute in element.attributes() {
        let attribute = attribute?;
        let key = attribute.key.as_ref();
        if namespace_declaration(key) {
            continue;
        }
        let modeled = [
            b"top".as_slice(),
            b"right".as_slice(),
            b"end".as_slice(),
            b"bottom".as_slice(),
            b"left".as_slice(),
            b"start".as_slice(),
            b"gutter".as_slice(),
            b"header".as_slice(),
            b"footer".as_slice(),
        ]
        .iter()
        .any(|local| is_word_attribute(key, local, word_prefixes));
        let value = std::str::from_utf8(&attribute.value)?;
        if !modeled
            || parse_integer_measurement::<i32>(element.name().as_ref(), key, value).is_err()
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The modeled section sequence used by the owning serializer.
pub(crate) fn checked_section_child_slot(local: &[u8]) -> Option<(u8, bool)> {
    let slot = match local {
        b"headerReference" => return Some((0, true)),
        b"footerReference" => return Some((1, true)),
        b"footnotePr" => 2,
        b"endnotePr" => 3,
        b"type" => 4,
        b"pgSz" => 5,
        b"pgMar" => 6,
        b"paperSrc" => 7,
        b"pgBorders" => 8,
        b"lnNumType" => 9,
        b"pgNumType" => 10,
        b"cols" => 11,
        b"vAlign" => 12,
        b"titlePg" => 13,
        b"textDirection" => 14,
        b"docGrid" => 15,
        b"sectPrChange" => 16,
        _ => return None,
    };
    Some((slot, false))
}

pub(crate) fn validate_checked_section_child(
    element: &BytesStart<'_>,
    prefixes: &[String],
) -> Result<bool> {
    let name = element.local_name();
    let local = name.as_ref();
    if matches!(
        local,
        b"p" | b"r" | b"tbl" | b"tr" | b"tc" | b"pPr" | b"rPr" | b"sectPr"
    ) {
        return Err(OxmlError::InvalidValue(
            "element XML section has a known child with the wrong owner".into(),
        ));
    }
    if matches!(local, b"headerReference" | b"footerReference") {
        if !story_reference_attributes_are_modeled(element, prefixes)? {
            return Err(OxmlError::InvalidValue(
                "element XML section reference has unsupported or invalid owner attributes".into(),
            ));
        }
        return Ok(true);
    }
    for attribute in element.attributes() {
        let attribute = attribute?;
        let key = attribute.key.as_ref();
        let value =
            attribute.decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())?;
        let invalid = match local {
            b"type" if is_word_attribute(key, b"val", prefixes) => {
                ST_SectionType::from_str(&value).is_err()
            }
            b"pgSz" if is_word_attribute(key, b"orient", prefixes) => {
                ST_PageOrientation::from_str(&value).is_err()
            }
            b"titlePg" if is_word_attribute(key, b"val", prefixes) => {
                !matches!(value.as_ref(), "true" | "false" | "on" | "off" | "1" | "0")
            }
            b"vAlign" if is_word_attribute(key, b"val", prefixes) => {
                !matches!(value.as_ref(), "top" | "center" | "bottom" | "both")
            }
            b"pgSz"
                if is_word_attribute(key, b"w", prefixes)
                    || is_word_attribute(key, b"h", prefixes) =>
            {
                parse_integer_measurement::<i32>(element.name().as_ref(), key, &value).is_err()
            }
            _ => false,
        };
        if invalid {
            return Err(OxmlError::InvalidValue(
                "element XML section property has an invalid modeled value".into(),
            ));
        }
    }
    Ok(matches!(
        local,
        b"type"
            | b"pgSz"
            | b"pgMar"
            | b"paperSrc"
            | b"lnNumType"
            | b"pgNumType"
            | b"vAlign"
            | b"titlePg"
            | b"textDirection"
            | b"docGrid"
    ))
}

fn story_reference_attributes_are_modeled(
    element: &BytesStart<'_>,
    word_prefixes: &[String],
) -> Result<bool> {
    let mut type_count = 0usize;
    let mut relationship_count = 0usize;
    for attribute in element.attributes() {
        let attribute = attribute?;
        let key = attribute.key.as_ref();
        if namespace_declaration(key) {
            continue;
        }
        let value = std::str::from_utf8(&attribute.value)?;
        if is_word_attribute(key, b"type", word_prefixes) {
            type_count += 1;
            if type_count > 1 || !matches!(value, "default" | "first" | "even") {
                return Ok(false);
            }
        } else if attribute_in_namespace(key, b"id", crate::namespace::R_NS, word_prefixes) {
            relationship_count += 1;
            if relationship_count > 1 || value.is_empty() {
                return Ok(false);
            }
        } else {
            return Ok(false);
        }
    }
    Ok(relationship_count == 1)
}

fn namespace_declaration(name: &[u8]) -> bool {
    name == b"xmlns" || name.starts_with(b"xmlns:")
}

fn raw_element_has_child_content(raw: &[u8]) -> Result<bool> {
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(_) => {
                if depth > 0 {
                    return Ok(true);
                }
                depth += 1;
            }
            Event::Empty(_) if depth > 0 => return Ok(true),
            Event::Text(text)
                if depth > 0 && text.as_ref().iter().any(|byte| !byte.is_ascii_whitespace()) =>
            {
                return Ok(true);
            }
            Event::CData(text)
                if depth > 0 && text.as_ref().iter().any(|byte| !byte.is_ascii_whitespace()) =>
            {
                return Ok(true);
            }
            Event::GeneralRef(_) if depth > 0 => return Ok(true),
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Eof => return Ok(false),
            _ => {}
        }
        buffer.clear();
    }
}

type AttributeSpans = (
    std::ops::Range<usize>,
    std::ops::Range<usize>,
    std::ops::Range<usize>,
);

fn start_tag_attributes(raw: &[u8]) -> Option<(usize, Vec<AttributeSpans>)> {
    if raw.first() != Some(&b'<') {
        return None;
    }
    let mut quote = None;
    let tag_end = raw
        .iter()
        .enumerate()
        .find_map(|(index, byte)| match (*byte, quote) {
            (b'\'' | b'"', None) => {
                quote = Some(*byte);
                None
            }
            (value, Some(open)) if value == open => {
                quote = None;
                None
            }
            (b'>', None) => Some(index),
            _ => None,
        })?;

    let mut cursor = 1usize;
    while cursor < tag_end && !raw[cursor].is_ascii_whitespace() && raw[cursor] != b'/' {
        cursor += 1;
    }
    let mut attributes = Vec::new();
    while cursor < tag_end {
        let full_start = cursor;
        while cursor < tag_end && raw[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor == tag_end || raw[cursor] == b'/' {
            break;
        }
        let name_start = cursor;
        while cursor < tag_end
            && !raw[cursor].is_ascii_whitespace()
            && raw[cursor] != b'='
            && raw[cursor] != b'/'
        {
            cursor += 1;
        }
        let name_end = cursor;
        while cursor < tag_end && raw[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if raw.get(cursor) != Some(&b'=') {
            return None;
        }
        cursor += 1;
        while cursor < tag_end && raw[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let open_quote = *raw.get(cursor)?;
        if open_quote != b'\'' && open_quote != b'"' {
            return None;
        }
        cursor += 1;
        let value_start = cursor;
        while cursor < tag_end && raw[cursor] != open_quote {
            cursor += 1;
        }
        if cursor == tag_end {
            return None;
        }
        let value_end = cursor;
        cursor += 1;
        attributes.push((
            name_start..name_end,
            value_start..value_end,
            full_start..cursor,
        ));
    }
    Some((tag_end, attributes))
}

/// `CT_Body` — The document body containing paragraphs, tables, and section properties.
#[derive(Debug, Clone, PartialEq)]
#[allow(non_snake_case)]
pub struct CT_Body {
    /// Mixed content: paragraphs and tables in document order.
    pub content: Vec<BodyContent>,
    pub sect_pr: Option<CT_SectPr>,
}

#[allow(non_snake_case)]
impl CT_Body {
    pub fn new() -> Self {
        CT_Body {
            content: Vec::new(),
            sect_pr: Some(CT_SectPr::default_letter()),
        }
    }

    /// Get an iterator over only the paragraphs.
    pub fn paragraphs(&self) -> impl Iterator<Item = &CT_P> {
        let mut paragraphs = Vec::new();
        for content in &self.content {
            match content {
                BodyContent::Paragraph(paragraph) => paragraphs.push(paragraph.as_ref()),
                BodyContent::ContentControl(sdt) => {
                    sdt.collect_paragraphs(SdtOwner::Body, &mut paragraphs)
                }
                BodyContent::Table(_) | BodyContent::RawXml(_) => {}
            }
        }
        paragraphs.into_iter()
    }

    /// The number of paragraphs [`Self::paragraphs`] visits in the body
    /// children before `index`.
    #[doc(hidden)]
    pub fn paragraph_count_before(&self, index: usize) -> usize {
        let mut nested = Vec::new();
        self.content[..index.min(self.content.len())]
            .iter()
            .map(|content| match content {
                BodyContent::Paragraph(_) => 1,
                BodyContent::ContentControl(sdt) => {
                    nested.clear();
                    sdt.collect_paragraphs(SdtOwner::Body, &mut nested);
                    nested.len()
                }
                _ => 0,
            })
            .sum()
    }

    /// Whether accepting every tracked change joins the body paragraph at
    /// `index` to the next body paragraph, as Word does when a paragraph mark
    /// is deleted or moved away (`w:del` or `w:moveFrom` in the mark `w:rPr`).
    ///
    /// The joined paragraph keeps the properties of the next paragraph, so a
    /// joining paragraph with no accepted content leaves nothing. The next
    /// body item must be a paragraph, the adjacency `Document::accept_all`
    /// requires before it merges a paragraph. `accept_all` refuses a removed
    /// mark before a table, a content control, preserved XML or the end of
    /// the body, and those paragraphs keep their marks here. A paragraph that
    /// ends a section also keeps its mark here, although `accept_all` merges
    /// its section break away.
    #[doc(hidden)]
    pub fn accepted_paragraph_joins_next(&self, index: usize) -> bool {
        let Some(BodyContent::Paragraph(paragraph)) = self.content.get(index) else {
            return false;
        };
        let Some(properties) = paragraph.properties.as_ref() else {
            return false;
        };
        let Some(mark) = properties.rpr.as_ref() else {
            return false;
        };
        // The model keeps a moved-away mark as preserved XML, read with the
        // fixed `w` prefix and any binding the XML declares itself. A mark
        // whose Word prefix is bound only outside it is not recognized, so
        // that paragraph keeps its mark.
        let moved_away = || {
            mark.revision_xml.iter().any(|raw| {
                let mut reader = Reader::from_reader(raw.as_slice());
                matches!(
                    reader.read_event(),
                    Ok(Event::Start(start) | Event::Empty(start))
                        if word_prefixes_at(&start, &["w".to_owned()]).is_ok_and(|prefixes| {
                            is_word_element(start.name().as_ref(), b"moveFrom", &prefixes)
                        })
                )
            })
        };
        let removed = mark
            .revision_markers
            .iter()
            .any(|marker| marker.kind() == RevisionKind::Deletion)
            || moved_away();
        removed
            && properties.sect_pr.is_none()
            && matches!(self.content.get(index + 1), Some(BodyContent::Paragraph(_)))
    }

    /// Get a mutable iterator over only the paragraphs.
    pub fn paragraphs_mut(&mut self) -> impl Iterator<Item = &mut CT_P> {
        self.content.iter_mut().filter_map(|c| match c {
            BodyContent::Paragraph(p) => Some(p.as_mut()),
            _ => None,
        })
    }

    /// Get an iterator over only the tables.
    pub fn tables(&self) -> impl Iterator<Item = &CT_Tbl> {
        let mut tables = Vec::new();
        for content in &self.content {
            match content {
                BodyContent::Table(table) => tables.push(table.as_ref()),
                BodyContent::ContentControl(sdt) => sdt.collect_tables(SdtOwner::Body, &mut tables),
                BodyContent::Paragraph(_) | BodyContent::RawXml(_) => {}
            }
        }
        tables.into_iter()
    }

    /// Get a mutable iterator over only the tables.
    pub fn tables_mut(&mut self) -> impl Iterator<Item = &mut CT_Tbl> {
        self.content.iter_mut().filter_map(|c| match c {
            BodyContent::Table(t) => Some(t.as_mut()),
            _ => None,
        })
    }

    /// Add a paragraph to the body.
    pub fn add_paragraph(&mut self, p: CT_P) {
        self.content.push(BodyContent::Paragraph(Box::new(p)));
    }

    /// Add a table to the body.
    pub fn add_table(&mut self, tbl: CT_Tbl) {
        self.content.push(BodyContent::Table(Box::new(tbl)));
    }

    /// Get the number of body content elements (paragraphs + tables).
    pub fn content_count(&self) -> usize {
        self.content.len()
    }

    /// Insert a paragraph at the given index.
    ///
    /// Panics if `index > content_count()`.
    pub fn insert_paragraph(&mut self, index: usize, p: CT_P) {
        self.content
            .insert(index, BodyContent::Paragraph(Box::new(p)));
    }

    /// Insert a table at the given index.
    ///
    /// Panics if `index > content_count()`.
    pub fn insert_table(&mut self, index: usize, tbl: CT_Tbl) {
        self.content
            .insert(index, BodyContent::Table(Box::new(tbl)));
    }

    /// Find the index of the first paragraph whose text contains the given substring.
    pub fn find_paragraph_index(&self, text: &str) -> Option<usize> {
        self.find_paragraph_indices(text).into_iter().next()
    }

    /// Find every matching body-content index, preferring direct paragraphs.
    pub fn find_paragraph_indices(&self, text: &str) -> Vec<usize> {
        let mut direct = Vec::new();
        let mut enclosing = Vec::new();
        for (index, content) in self.content.iter().enumerate() {
            match content {
                BodyContent::Paragraph(paragraph) if paragraph.text().contains(text) => {
                    direct.push(index);
                }
                BodyContent::ContentControl(sdt) => {
                    let mut paragraphs = Vec::new();
                    sdt.collect_paragraphs(SdtOwner::Body, &mut paragraphs);
                    if paragraphs
                        .iter()
                        .any(|paragraph| paragraph.text().contains(text))
                    {
                        enclosing.push(index);
                    }
                }
                _ => {}
            }
        }
        direct.extend(enclosing);
        direct
    }

    /// Return every content control in document order, including nested controls.
    pub fn content_controls(&self) -> Vec<&CT_Sdt> {
        let mut controls = Vec::new();
        for content in &self.content {
            match content {
                BodyContent::Paragraph(paragraph) => paragraph.collect_controls(&mut controls),
                BodyContent::Table(table) => table.collect_controls(&mut controls),
                BodyContent::ContentControl(sdt) => {
                    controls.push(sdt);
                    sdt.collect_controls(SdtOwner::Body, &mut controls);
                }
                BodyContent::RawXml(_) => {}
            }
        }
        controls
    }

    /// Remove and return the content at the given index, or `None` if out of bounds.
    pub fn remove(&mut self, index: usize) -> Option<BodyContent> {
        if index < self.content.len() {
            Some(self.content.remove(index))
        } else {
            None
        }
    }

    /// Get a reference to the content at the given index.
    pub fn get(&self, index: usize) -> Option<&BodyContent> {
        self.content.get(index)
    }

    /// Get a mutable reference to the content at the given index.
    pub fn get_mut(&mut self, index: usize) -> Option<&mut BodyContent> {
        self.content.get_mut(index)
    }

    pub fn from_xml(reader: &mut Reader<&[u8]>) -> Result<Self> {
        let admitted = admit_word_reader(reader, &["w".to_owned()], b"body", Some(W_NS))?;
        let mut controls = admitted.controls();
        Self::from_xml_with_prefixes_and_owner_bindings_until(
            reader,
            &["w".to_string()],
            &[],
            b"body",
            Some(&mut controls),
        )
    }

    pub(crate) fn from_xml_with_prefixes_and_owner_bindings_until(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        owner_bindings: &[(String, String)],
        end_local_name: &[u8],
        control_facts: Option<&mut ControlSourceCursor<'_>>,
    ) -> Result<Self> {
        Self::from_xml_with_property_output(
            reader,
            word_prefixes,
            owner_bindings,
            end_local_name,
            control_facts,
            None,
        )
    }

    fn from_xml_with_property_output(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        owner_bindings: &[(String, String)],
        end_local_name: &[u8],
        control_facts: Option<&mut ControlSourceCursor<'_>>,
        ordinary: Option<&mut Vec<WordOrdinaryFact>>,
    ) -> Result<Self> {
        Self::from_xml_with_read_output(
            reader,
            word_prefixes,
            owner_bindings,
            end_local_name,
            control_facts,
            ordinary,
            None,
        )
    }

    fn from_xml_with_read_output(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        owner_bindings: &[(String, String)],
        end_local_name: &[u8],
        mut control_facts: Option<&mut ControlSourceCursor<'_>>,
        mut ordinary: Option<&mut Vec<WordOrdinaryFact>>,
        mut read_output: Option<&mut ReturnedWordRead>,
    ) -> Result<Self> {
        let mut pending_reads = Vec::new();
        if let Some(output) = &mut ordinary {
            output.clear();
        }
        let mut pending_ordinary = Vec::new();
        let mut content = Vec::new();
        let mut sect_pr = None;
        let mut buf = Vec::new();

        loop {
            let event_start = reader.buffer_position();
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    if is_word_element(name.as_ref(), b"p", &prefixes) {
                        let mut properties = Vec::new();
                        let mut extras = Vec::new();
                        let mut texts = Vec::new();
                        let mut maps = crate::text::WordBlockSourceMaps::default();
                        let paragraph = CT_P::from_xml_with_read_output(
                            reader,
                            &prefixes,
                            Some(e),
                            control_facts.as_deref_mut(),
                            crate::text::WordParagraphReadOutput {
                                ordinary: ordinary.as_ref().map(|_| &mut properties),
                                extra_maps: read_output.as_ref().map(|_| &mut extras),
                                text_facts: read_output.as_ref().map(|_| &mut texts),
                                nested_maps: read_output.as_ref().map(|_| &mut maps),
                                ..Default::default()
                            },
                        )?;
                        if read_output.is_some() {
                            maps.paragraph(extras);
                            maps.word_texts.push((
                                Vec::new(),
                                control_facts
                                    .as_deref()
                                    .and_then(|cursor| cursor.paragraph_word_facts(reader, texts)),
                            ));
                            pending_reads.push((content.len(), maps));
                        }
                        content.push(BodyContent::Paragraph(Box::new(paragraph)));
                        pending_ordinary.extend(properties);
                    } else if is_word_element(name.as_ref(), b"tbl", &prefixes) {
                        let local_bindings = local_namespace_overrides(e, word_prefixes)?;
                        let table_bindings = merged_owner_bindings(owner_bindings, &local_bindings);
                        let mut maps = (ordinary.is_some() || read_output.is_some()).then(|| {
                            crate::text::WordBlockSourceMaps {
                                collect_properties: ordinary.is_some(),
                                collect_word_texts: read_output.is_some(),
                                ..Default::default()
                            }
                        });
                        let mut table =
                            CT_Tbl::from_xml_with_prefixes_and_owner_bindings_at_depth_and_maps(
                                reader,
                                &prefixes,
                                &table_bindings,
                                0,
                                Some(e),
                                control_facts.as_deref_mut(),
                                maps.as_mut(),
                            )?;
                        table.retain_root_attributes(e, &prefixes)?;
                        if let Some(mut maps) = maps {
                            pending_ordinary.append(&mut maps.ordinary);
                            if read_output.is_some() {
                                pending_reads.push((content.len(), maps));
                            }
                        }
                        content.push(BodyContent::Table(Box::new(table)));
                    } else if is_word_element(name.as_ref(), b"sdt", &prefixes) {
                        let (captured, captured_map) =
                            if ordinary.is_some() || read_output.is_some() {
                                if let Some(cursor) = control_facts.as_deref() {
                                    let (raw, mapped) =
                                        cursor.capture_element(reader, e, event_start)?;
                                    let map = mapped.map(|input| {
                                        std::sync::Arc::new(input.projection_parts().0.clone())
                                    });
                                    (raw, map)
                                } else {
                                    (capture_element(reader, e)?, None)
                                }
                            } else {
                                let captured = capture_element(reader, e)?;
                                let map = control_facts.as_deref().and_then(|facts| {
                                    facts.captured_map(
                                        reader,
                                        event_start..reader.buffer_position(),
                                        &captured,
                                    )
                                });
                                (captured, map)
                            };
                        let (raw, captured_map) = crate::text::raw_with_external_bindings_and_map(
                            &captured,
                            owner_bindings,
                            None,
                            captured_map,
                        )?;
                        let mut maps = (ordinary.is_some() || read_output.is_some()).then(|| {
                            crate::text::WordBlockSourceMaps {
                                collect_properties: ordinary.is_some(),
                                collect_word_texts: read_output.is_some(),
                                ..Default::default()
                            }
                        });
                        if let Some(sdt) = CT_Sdt::from_raw_with_mapped_context_and_maps(
                            &raw,
                            &prefixes,
                            SdtOwner::Body,
                            control_facts.as_deref_mut(),
                            captured_map,
                            maps.as_mut(),
                        ) {
                            if let Some(mut maps) = maps {
                                pending_ordinary.append(&mut maps.ordinary);
                                if read_output.is_some() {
                                    pending_reads.push((content.len(), maps));
                                }
                            }
                            content.push(BodyContent::ContentControl(Box::new(sdt)));
                        } else {
                            content.push(BodyContent::RawXml(Box::new(raw)));
                        }
                    } else if is_word_element(name.as_ref(), b"sectPr", &prefixes) {
                        if sect_pr.is_none() {
                            let local_bindings = local_namespace_overrides(e, word_prefixes)?;
                            let section_bindings =
                                merged_owner_bindings(owner_bindings, &local_bindings);
                            sect_pr =
                                Some(CT_SectPr::from_xml_with_prefixes_owner_bindings_and_root(
                                    reader,
                                    &prefixes,
                                    &section_bindings,
                                    Some(e),
                                )?);
                        } else {
                            content.push(BodyContent::RawXml(Box::new(
                                crate::text::raw_with_external_bindings(
                                    &capture_element(reader, e)?,
                                    owner_bindings,
                                )?,
                            )));
                        }
                    } else {
                        // Capture unknown elements as raw XML
                        content.push(BodyContent::RawXml(Box::new(
                            crate::text::raw_with_external_bindings(
                                &capture_element(reader, e)?,
                                owner_bindings,
                            )?,
                        )));
                    }
                }
                Ok(Event::Empty(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    if is_word_element(name.as_ref(), b"p", &prefixes) {
                        content.push(BodyContent::Paragraph(Box::new(CT_P::from_empty_root(
                            e, &prefixes,
                        )?)));
                    } else if is_word_element(name.as_ref(), b"tbl", &prefixes) {
                        content.push(BodyContent::Table(Box::default()));
                    } else if is_word_element(name.as_ref(), b"sectPr", &prefixes) {
                        if sect_pr.is_none() {
                            sect_pr = Some(CT_SectPr::from_empty_root(e, &prefixes)?);
                        } else {
                            content.push(BodyContent::RawXml(Box::new(
                                crate::text::raw_with_external_bindings(
                                    &capture_empty_element(e)?,
                                    owner_bindings,
                                )?,
                            )));
                        }
                    } else {
                        content.push(BodyContent::RawXml(Box::new(
                            crate::text::raw_with_external_bindings(
                                &capture_empty_element(e)?,
                                owner_bindings,
                            )?,
                        )));
                    }
                }
                Ok(Event::End(ref e))
                    if is_word_element(e.name().as_ref(), end_local_name, word_prefixes) =>
                {
                    break;
                }
                Ok(Event::Eof) => {
                    let end = String::from_utf8_lossy(end_local_name);
                    return Err(OxmlError::MissingElement(format!("w:{end} end")));
                }
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        if let Some(output) = ordinary {
            *output = pending_ordinary;
        }
        if let Some(output) = read_output.as_mut() {
            output.body = pending_reads;
        }
        Ok(CT_Body { content, sect_pr })
    }

    /// Validate borrowed owners before cloning or serializing this body.
    #[doc(hidden)]
    pub fn validate_resource_limits(&self) -> Result<()> {
        admit_word_model(WordOwner::Body(self))
    }

    pub fn to_xml<W: std::io::Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.write_marked(writer, PartWrite::Whole, &mut |_| {}, None)
    }

    /// [`Self::to_xml`], showing `mark` the writer after the body's start
    /// tag and after each child. `PartWrite::Frame` omits the children and
    /// marks once, at that child boundary, before the final section.
    pub(crate) fn write_marked<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        mode: PartWrite,
        mark: &mut dyn FnMut(&Writer<W>),
        mut work: Option<&mut crate::text::WordOperationWork>,
    ) -> Result<()> {
        admit_word_model(WordOwner::Body(self))?;
        writer.write_event(Event::Start(BytesStart::new("w:body")))?;
        mark(writer);

        if mode == PartWrite::Whole {
            for item in &self.content {
                write_body_content(item, writer, work.as_deref_mut())?;
                mark(writer);
            }
        }

        if let Some(ref sect) = self.sect_pr {
            sect.to_xml(writer)?;
        }

        writer.write_event(Event::End(BytesEnd::new("w:body")))?;
        Ok(())
    }
}

/// The two part serializations that exist today. `Whole` writes children and
/// then the conditional `w14` declaration. `Frame` stops at the child
/// boundary and leaves `w14` for the caller.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PartWrite {
    Whole,
    Frame,
}

impl Default for CT_Body {
    fn default() -> Self {
        Self::new()
    }
}

fn write_body_content<W: std::io::Write>(
    item: &BodyContent,
    writer: &mut Writer<W>,
    work: Option<&mut crate::text::WordOperationWork>,
) -> Result<()> {
    match (item, work) {
        (BodyContent::Paragraph(p), Some(work)) => p.write_xml_core_in(
            writer,
            None,
            crate::text::XmlWriteRole::Publication,
            None,
            work,
            None,
        )?,
        (BodyContent::Table(t), Some(work)) => t.write_xml_role_in(
            writer,
            crate::text::XmlWriteRole::Publication,
            None,
            work,
            None,
        )?,
        (BodyContent::ContentControl(sdt), Some(work)) => sdt.write_xml_role_in(
            writer,
            crate::text::XmlWriteRole::Publication,
            None,
            work,
            None,
        )?,
        (BodyContent::Paragraph(p), None) => p.to_xml(writer)?,
        (BodyContent::Table(t), None) => t.to_xml(writer)?,
        (BodyContent::ContentControl(sdt), None) => sdt.to_xml(writer)?,
        (BodyContent::RawXml(raw), _) => writer.get_mut().write_all(raw)?,
    }
    Ok(())
}

fn paragraph_root_has_foreign_word(paragraph: &CT_P) -> Result<bool> {
    for (position, raw) in &paragraph.extra_xml {
        if !CT_P::raw_is_root_attributes(*position, raw) {
            continue;
        }
        let mut reader = Reader::from_reader(raw.as_slice());
        let Event::Empty(record) = reader.read_event()? else {
            continue;
        };
        let declarations = crate::text::raw_namespace_declarations(&record)?;
        if declarations
            .iter()
            .any(|(prefix, namespace)| prefix == "w" && namespace != W_NS)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

// The operation and actual Source observe the same substantive document reader.
// Owning metadata handles does not clone source bytes or activate input work.
struct ScopedDocumentInput {
    registry: std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>>,
    initial_delivery: bool,
    publication: std::sync::Arc<std::sync::Mutex<WordPublicationCoverage>>,
    reads: std::sync::Arc<std::sync::Mutex<ReturnedWordRead>>,
}

/// `CT_Document` — The root document element.
#[derive(Debug, Clone, PartialEq)]
#[allow(non_snake_case)]
pub struct CT_Document {
    pub body: CT_Body,
    /// Extra namespace declarations captured from the original document element.
    /// Each entry is (prefix, uri), e.g. ("xmlns:wp14", "http://...").
    pub extra_namespaces: Vec<(String, String)>,
    /// Raw XML for `<w:background>` element if present.
    pub background_xml: Option<Vec<u8>>,
    /// Foreign same-local-name background children retained before the body.
    #[doc(hidden)]
    pub background_extra_xml: Vec<Vec<u8>>,
    /// Non-namespace attributes of the original document element, such as
    /// `mc:Ignorable`, in source order.
    #[doc(hidden)]
    pub root_attributes: Vec<(String, String)>,
}

#[allow(non_snake_case)]
impl CT_Document {
    /// Set the owned background color while retaining every producer byte outside its value.
    pub fn set_background_color(&mut self, color: &str) -> Result<()> {
        if color.len() != 6 || !color.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(OxmlError::InvalidValue(
                "page color must be six hexadecimal digits".to_owned(),
            ));
        }
        let mut prefixes = vec!["w".to_owned()];
        for (name, namespace) in &self.extra_namespaces {
            if namespace == W_NS {
                prefixes.push(name.strip_prefix("xmlns:").unwrap_or("").to_owned());
            }
        }
        self.background_xml = Some(match &self.background_xml {
            Some(raw) => {
                crate::settings::rewrite_word_root_attribute(raw, &prefixes, b"color", color)?
            }
            None => format!("<w:background xmlns:w=\"{W_NS}\" w:color=\"{color}\"/>").into_bytes(),
        });
        Ok(())
    }

    pub fn new() -> Self {
        CT_Document {
            body: CT_Body::new(),
            extra_namespaces: Vec::new(),
            background_xml: None,
            background_extra_xml: Vec::new(),
            root_attributes: Vec::new(),
        }
    }

    /// Parse from XML bytes (the content of word/document.xml).
    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        let admitted = admit_word_source(xml, &[], None, None)?;
        Self::from_xml_admitted(xml, admitted, None)
    }

    fn from_xml_admitted(
        xml: &[u8],
        admitted: AdmittedWordSource,
        origins: Option<ScopedDocumentInput>,
    ) -> Result<Self> {
        Self::from_xml_admitted_with_properties(xml, admitted, origins, None)
    }

    fn from_xml_admitted_with_properties(
        xml: &[u8],
        admitted: AdmittedWordSource,
        origins: Option<ScopedDocumentInput>,
        mut property_output: Option<&mut Vec<WordOrdinaryFact>>,
    ) -> Result<Self> {
        if let Some(output) = &mut property_output {
            output.clear();
        }
        let mut pending_properties = Vec::new();
        let mut control_facts = admitted.controls_for(xml);
        if let Some(origins) = origins.as_ref() {
            control_facts = control_facts.with_field_origins(
                std::sync::Arc::clone(&origins.registry),
                origins.initial_delivery,
            );
        }
        let mut reader = Reader::from_reader(xml);
        reader.config_mut().trim_text(false);

        let mut ordinary = origins.as_ref().map(|_| Vec::new());
        let mut body = None;
        let mut extra_namespaces = Vec::new();
        let mut background_xml = None;
        let mut background_extra_xml = Vec::new();
        let mut root_attributes = Vec::new();
        let mut buf = Vec::new();
        let mut word_prefixes = Vec::new();
        let mut document_open = false;
        let mut document_closed = false;

        // Known namespace prefixes that we always emit ourselves
        let known_ns: &[&[u8]] = &[b"xmlns:w", b"xmlns:r", b"xmlns:mc", b"xmlns"];

        loop {
            let event_start = reader.buffer_position() as usize;
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, &word_prefixes)?;
                    if is_word_element(name.as_ref(), b"document", &prefixes) {
                        if document_open || document_closed {
                            return Err(OxmlError::UnexpectedElement("w:document".to_owned()));
                        }
                        for attr in e.attributes().flatten() {
                            let key = attr.key.as_ref();
                            let is_namespace = key.starts_with(b"xmlns:") || key == b"xmlns";
                            if is_namespace && known_ns.contains(&key) {
                                continue;
                            }
                            let key_str = std::str::from_utf8(key).unwrap_or("").to_string();
                            let val_str = attr
                                .decoded_and_normalized_value(XmlVersion::Implicit1_0, e.decoder())?
                                .into_owned();
                            if is_namespace {
                                extra_namespaces.push((key_str, val_str));
                            } else {
                                root_attributes.push((key_str, val_str));
                            }
                        }
                        document_open = true;
                        word_prefixes = prefixes.into_owned();
                        if let Some(ordinary) = &mut ordinary
                            && let Some(origin) = reader_input_origin(xml, &reader)
                        {
                            ordinary.push(
                                event_start + origin..reader.buffer_position() as usize + origin,
                            );
                        }
                    } else if !document_open || document_closed {
                        return Err(OxmlError::UnexpectedElement(
                            String::from_utf8_lossy(name.as_ref()).into_owned(),
                        ));
                    } else if is_word_element(name.as_ref(), b"body", &prefixes) {
                        if !document_open || document_closed || body.is_some() {
                            return Err(OxmlError::UnexpectedElement("w:body".to_owned()));
                        }
                        let opening = ordinary.as_ref().and_then(|_| {
                            let origin = reader_input_origin(xml, &reader)?;
                            Some(event_start + origin..reader.buffer_position() as usize + origin)
                        });
                        let owner_bindings = local_namespace_overrides(e, &word_prefixes)?;
                        let mut reads = origins.as_ref().map(|origins| {
                            origins.reads.lock().expect("returned Word facts poisoned")
                        });
                        body = Some(CT_Body::from_xml_with_read_output(
                            &mut reader,
                            &prefixes,
                            &owner_bindings,
                            b"body",
                            Some(&mut control_facts),
                            property_output.as_ref().map(|_| &mut pending_properties),
                            reads.as_deref_mut(),
                        )?);
                        if let (Some(ordinary), Some(opening)) = (&mut ordinary, opening) {
                            ordinary.push(opening);
                        }
                    } else if is_word_element(name.as_ref(), b"background", &prefixes) {
                        background_xml = Some(capture_element(&mut reader, e)?);
                    } else if matches_local_name(name.as_ref(), b"background") {
                        background_extra_xml.push(capture_element(&mut reader, e)?);
                    } else {
                        reader.read_to_end_into(name, &mut Vec::new())?;
                    }
                }
                Ok(Event::Empty(ref e)) => {
                    let prefixes = word_prefixes_at(e, &word_prefixes)?;
                    if is_word_element(e.name().as_ref(), b"document", &prefixes)
                        || !document_open
                        || document_closed
                    {
                        return Err(OxmlError::UnexpectedElement(
                            String::from_utf8_lossy(e.name().as_ref()).into_owned(),
                        ));
                    } else if is_word_element(e.name().as_ref(), b"body", &prefixes) {
                        if body.is_some() {
                            return Err(OxmlError::UnexpectedElement("w:body".to_owned()));
                        }
                        body = Some(CT_Body::new());
                        if let Some(ordinary) = &mut ordinary
                            && let Some(origin) = reader_input_origin(xml, &reader)
                        {
                            ordinary.push(
                                event_start + origin..reader.buffer_position() as usize + origin,
                            );
                        }
                    } else if is_word_element(e.name().as_ref(), b"background", &prefixes) {
                        background_xml = Some(capture_empty_element(e)?);
                    } else if matches_local_name(e.name().as_ref(), b"background") {
                        background_extra_xml.push(capture_empty_element(e)?);
                    }
                }
                Ok(Event::End(ref e))
                    if is_word_element(e.name().as_ref(), b"document", &word_prefixes) =>
                {
                    if !document_open {
                        return Err(OxmlError::UnexpectedElement("w:document end".to_owned()));
                    }
                    document_open = false;
                    document_closed = true;
                    if let Some(ordinary) = &mut ordinary
                        && let Some(origin) = reader_input_origin(xml, &reader)
                    {
                        ordinary
                            .push(event_start + origin..reader.buffer_position() as usize + origin);
                    }
                }
                Ok(Event::Text(ref text)) if !text.as_ref().iter().all(u8::is_ascii_whitespace) => {
                    return Err(OxmlError::UnexpectedElement(
                        "text outside w:body".to_owned(),
                    ));
                }
                Ok(Event::Eof) => {
                    if document_open || !document_closed {
                        return Err(OxmlError::MissingElement("w:document end".to_owned()));
                    }
                    break;
                }
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        let document = CT_Document {
            body: body.ok_or_else(|| OxmlError::MissingElement("w:body".to_owned()))?,
            extra_namespaces,
            background_xml,
            background_extra_xml,
            root_attributes,
        };
        if let (Some(origins), Some(ordinary)) = (origins, ordinary) {
            origins
                .publication
                .lock()
                .expect("publication coverage poisoned")
                .add_ordinary_ranges(ordinary);
        }
        if let Some(output) = property_output {
            *output = pending_properties;
        }
        Ok(document)
    }

    /// Validate borrowed modeled owners before recursive work.
    #[doc(hidden)]
    pub fn validate_resource_limits(&self) -> Result<()> {
        admit_word_model(WordOwner::Body(&self.body))
    }

    /// Serialize to XML bytes.
    pub fn to_xml(&self) -> Result<Vec<u8>> {
        Ok(self.write_part(PartWrite::Whole, None)?.0)
    }

    /// [`Self::to_xml`], and where the body's children are in its bytes:
    /// the range between the body's start tag and its section properties or
    /// end tag, and the range of each child there, one after the other.
    #[doc(hidden)]
    #[allow(clippy::type_complexity)] // The bytes, the children's range and each child's range.
    pub fn to_xml_with_body_spans(
        &self,
    ) -> Result<(Vec<u8>, std::ops::Range<usize>, Vec<std::ops::Range<usize>>)> {
        let mut marks = Vec::with_capacity(self.body.content.len() + 2);
        let xml = self.write_part(PartWrite::Whole, Some(&mut marks))?.0;
        // A `w14` declaration added to the root moves everything after it.
        let shift = xml.len() - marks.pop().unwrap_or(xml.len());
        let marks = marks
            .into_iter()
            .map(|mark| mark + shift)
            .collect::<Vec<_>>();
        let content = marks[0]..marks[marks.len() - 1];
        let children = marks.windows(2).map(|pair| pair[0]..pair[1]).collect();
        Ok((xml, content, children))
    }

    /// The part's bytes, with `marks`, when given, holding the offset after
    /// the body's start tag and after each child, and then the length before
    /// any `w14` declaration is added.
    fn write_part(
        &self,
        mode: PartWrite,
        mut marks: Option<&mut Vec<usize>>,
    ) -> Result<(Vec<u8>, Option<Vec<u8>>)> {
        let mut writer = Writer::new(Vec::new());
        let mut frame_at = None;
        self.write_part_contents(
            &mut writer,
            mode,
            &mut |writer| {
                if mode == PartWrite::Frame {
                    frame_at = Some(writer.get_ref().len());
                } else if let Some(marks) = marks.as_deref_mut() {
                    marks.push(writer.get_ref().len());
                }
            },
            None,
        )?;
        if let Some(marks) = marks {
            marks.push(writer.get_ref().len());
        }
        let mut xml = writer.into_inner();
        if mode == PartWrite::Frame {
            let at = frame_at.ok_or_else(|| {
                OxmlError::InvalidValue("frame child boundary was not marked".to_owned())
            })?;
            let tail = xml.split_off(at);
            return Ok((xml, Some(tail)));
        }
        declare_w14_on_part_root(&mut xml)?;
        Ok((xml, None))
    }

    fn write_part_contents<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        mode: PartWrite,
        mark: &mut dyn FnMut(&Writer<W>),
        work: Option<&mut crate::text::WordOperationWork>,
    ) -> Result<()> {
        self.validate_resource_limits()?;
        let _binding_scope = self.root_binding_scope()?;
        writer.write_event(Event::Decl(BytesDecl::new(
            "1.0",
            Some("UTF-8"),
            Some("yes"),
        )))?;
        writer.write_event(Event::Start(self.root_start()))?;
        if let Some(bg) = &self.background_xml {
            writer.get_mut().write_all(bg)?;
        }
        for raw in &self.background_extra_xml {
            writer.get_mut().write_all(raw)?;
        }
        self.body.write_marked(writer, mode, mark, work)?;
        writer.write_event(Event::End(BytesEnd::new("w:document")))?;
        Ok(())
    }

    /// The bytes [`Self::to_xml`] writes before the first body child, through
    /// the body start, and after the last child, including the final section.
    /// This is before the conditional `w14` root declaration. A later block
    /// record applies that declaration to the stored frame.
    #[doc(hidden)]
    pub fn body_frame_xml(&self) -> Result<(Vec<u8>, Vec<u8>)> {
        let (head, tail) = self.write_part(PartWrite::Frame, None)?;
        tail.map(|tail| (head, tail))
            .ok_or_else(|| OxmlError::InvalidValue("frame tail was not written".to_owned()))
    }

    /// The canonical source bytes for `item` under this part root. A paragraph
    /// that keeps a root-local foreign `w` binding uses the explicit Source
    /// path, so its bytes can differ from [`Self::to_xml`], which stays
    /// Publication. The item need not belong to the body. Raw bytes pass
    /// through.
    #[doc(hidden)]
    pub fn body_item_xml(&self, item: &BodyContent) -> Result<Vec<u8>> {
        self.body_item_xml_with_ancestor_declarations(item, &[])
    }

    /// Serialize an item under this writer's original part root and actual
    /// non-root local ancestor declarations, ordered outer to inner.
    /// Callers must prove these frames from current writer coordinates.
    #[doc(hidden)]
    pub fn body_item_xml_with_ancestor_declarations(
        &self,
        item: &BodyContent,
        ancestors: &[&[(String, String)]],
    ) -> Result<Vec<u8>> {
        let mut writer = Writer::new(Vec::new());
        self.write_body_item_in(item, ancestors, &mut writer, None)?;
        Ok(writer.into_inner())
    }

    fn write_body_item_in<W: std::io::Write>(
        &self,
        item: &BodyContent,
        ancestors: &[&[(String, String)]],
        writer: &mut Writer<W>,
        mut work: Option<&mut crate::text::WordOperationWork>,
    ) -> Result<()> {
        // Census all borrowed input before the first owned frame copy. Ancestor
        // frames, the reconstructed writer-time root and the item's own source
        // bytes share one envelope, so a frame never receives a fresh
        // allowance after its guard copy.
        let mut ancestor_work = 0usize;
        let mut scope_bytes = 0usize;
        for frame in ancestors {
            ancestor_work = ancestor_work
                .checked_add(crate::text::retained_binding_work(0, frame, 0)?)
                .ok_or_else(|| {
                    OxmlError::InvalidValue("ancestor declaration work overflow".into())
                })?;
            for (prefix, namespace) in frame.iter() {
                scope_bytes = scope_bytes
                    .checked_add(prefix.len())
                    .and_then(|bytes| bytes.checked_add(namespace.len()))
                    .ok_or_else(|| {
                        OxmlError::InvalidValue("ancestor declaration scope overflow".into())
                    })?;
            }
        }
        let root_declarations = crate::text::raw_namespace_declarations(&self.root_start())?;
        let root_work = crate::text::retained_binding_work(0, &root_declarations, 0)?;
        for (prefix, namespace) in &root_declarations {
            scope_bytes = scope_bytes
                .checked_add(prefix.len())
                .and_then(|bytes| bytes.checked_add(namespace.len()))
                .ok_or_else(|| {
                    OxmlError::InvalidValue("ancestor declaration scope overflow".into())
                })?;
        }
        let item_bytes = match item {
            BodyContent::Paragraph(paragraph) => paragraph.retained_paragraph_source_bytes()?,
            BodyContent::Table(_) | BodyContent::ContentControl(_) | BodyContent::RawXml(_) => 0,
        };
        let allowance = crate::text::retained_source_envelope(scope_bytes, item_bytes)?;
        let needed = ancestor_work
            .checked_add(root_work)
            .and_then(|work| work.checked_add(item_bytes))
            .ok_or_else(|| OxmlError::InvalidValue("ancestor declaration work overflow".into()))?;
        if needed > allowance {
            return Err(OxmlError::InvalidValue(
                "ancestor source frame work exceeds admitted source work".into(),
            ));
        }
        match item {
            BodyContent::Paragraph(paragraph) => {
                admit_word_model(WordOwner::Paragraph(paragraph))?;
            }
            BodyContent::Table(table) => admit_word_model(WordOwner::Table(table))?,
            BodyContent::ContentControl(control) => {
                admit_word_model(WordOwner::Control(control))?;
            }
            BodyContent::RawXml(_) => {}
        }
        let _binding_scope = self.root_binding_scope()?;
        let mut ancestor_scopes = Vec::with_capacity(ancestors.len());
        for frame in ancestors {
            ancestor_scopes.push(crate::text::emitted_ancestor_scope(frame)?);
        }
        let result = match item {
            BodyContent::Paragraph(paragraph) if paragraph_root_has_foreign_word(paragraph)? => {
                // A fresh authored paragraph may keep a root-local foreign `w`
                // binding while its own Word content uses an existing alias.
                // Publication's canonical `w` would rebind that opaque root
                // fact. Emit the paragraph in its actual consumer frame instead,
                // so the foreign binding and QName-valued root attributes stay
                // authoritative. The paragraph's own retained declarations are
                // staged by `write_xml_core` after this ancestor frame.
                //
                // Decode the part root once, then borrow every inherited
                // prefix and URI from its owner. Do not clone full URI strings
                // per item; the approved D7 contract keeps these as borrowed
                // source facts and bounded overlays.
                let mut borrowed = Vec::with_capacity(
                    root_declarations.len()
                        + ancestors.iter().map(|frame| frame.len()).sum::<usize>(),
                );
                borrowed.extend(
                    root_declarations
                        .iter()
                        .map(|(prefix, namespace)| (prefix.as_str(), namespace.as_str())),
                );
                for frame in ancestors {
                    borrowed.extend(
                        frame
                            .iter()
                            .map(|(prefix, namespace)| (prefix.as_str(), namespace.as_str())),
                    );
                }
                if let Some(work) = work.as_deref_mut() {
                    paragraph.write_xml_core_in(
                        writer,
                        None,
                        crate::text::XmlWriteRole::Source,
                        Some(&borrowed),
                        work,
                        None,
                    )
                } else {
                    paragraph.write_xml_core(
                        writer,
                        None,
                        crate::text::XmlWriteRole::Source,
                        Some(&borrowed),
                    )
                }
            }
            _ => write_body_content(item, writer, work),
        };
        // Scoped overlays must restore in reverse installation order.
        while ancestor_scopes.pop().is_some() {}
        result?;
        Ok(())
    }

    /// Serialize `item` as the only child of this document's body, under the
    /// part root and with the bytes [`Self::to_xml`] writes for it there.
    ///
    /// The item need not belong to the body. As in [`Self::to_xml`], the root
    /// declares `w14` when the original root does or the written bytes use it,
    /// so a root without `w14` here does not say whether the whole part has it.
    #[doc(hidden)]
    pub fn standalone_body_item_xml(&self, item: &BodyContent) -> Result<Vec<u8>> {
        let _binding_scope = self.root_binding_scope()?;
        let mut writer = Writer::new(Vec::new());
        writer.write_event(Event::Start(self.root_start()))?;
        writer.write_event(Event::Start(BytesStart::new("w:body")))?;
        write_body_content(item, &mut writer, None)?;
        writer.write_event(Event::End(BytesEnd::new("w:body")))?;
        writer.write_event(Event::End(BytesEnd::new("w:document")))?;
        let mut xml = writer.into_inner();
        declare_w14_on_part_root(&mut xml)?;
        Ok(xml)
    }

    /// The part-root guarantees the body is serialized under.
    fn root_binding_scope(&self) -> Result<crate::text::RootBindingScope> {
        crate::text::emitted_root_binding_scope(
            &self.root_start(),
            ROOT_R_BINDING | ROOT_MC_BINDING | ROOT_WP_BINDING,
            true,
        )
    }

    /// The `w:document` start tag, before any `w14` declaration.
    fn root_start(&self) -> BytesStart<'_> {
        let wp_ns = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
        let mut doc_start = BytesStart::new("w:document");
        doc_start.push_attribute(("xmlns:w", W_NS));
        doc_start.push_attribute((
            "xmlns:r",
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
        ));
        doc_start.push_attribute((
            "xmlns:mc",
            "http://schemas.openxmlformats.org/markup-compatibility/2006",
        ));

        // Always emit xmlns:wp for drawing elements
        let mut has_wp = false;
        for (key, _) in &self.extra_namespaces {
            if key == "xmlns:wp" {
                has_wp = true;
                break;
            }
        }
        if !has_wp {
            doc_start.push_attribute(("xmlns:wp", wp_ns));
        }

        // Replay captured extra namespaces, then the root attributes that
        // may name their prefixes, such as `mc:Ignorable`.
        for (key, val) in self.extra_namespaces.iter().chain(&self.root_attributes) {
            doc_start.push_attribute((key.as_str(), val.as_str()));
        }
        doc_start
    }
}

impl Default for CT_Document {
    fn default() -> Self {
        Self::new()
    }
}

/// Section source ownership mirrors the actual section readers, including raw subsets.
pub(crate) fn source_modeled_attributes(
    parent: &[u8],
    local: &[u8],
) -> Option<&'static [&'static [u8]]> {
    Some(match (parent, local) {
        (b"sectPr", b"pgSz") => {
            crate::numbering::property_attributes(crate::numbering::PropertyKind::Paragraph, local)
        }
        (b"sectPr", b"pgMar") => {
            crate::numbering::property_attributes(crate::numbering::PropertyKind::Paragraph, local)
        }
        (b"sectPr", b"headerReference" | b"footerReference") => &[b"type"],
        (b"sectPr", b"type" | b"titlePg" | b"vAlign" | b"textDirection") => &[b"val"],
        (b"sectPr", b"cols") => &[b"num", b"space", b"equalWidth", b"sep"],
        (b"cols", b"col") => &[b"w", b"space"],
        // This reader owns only start. Format/chapter metadata remains source-backed.
        (b"sectPr", b"pgNumType") => &[b"start"],
        (b"sectPr", b"paperSrc") => &[b"first", b"other"],
        (b"sectPr", b"lnNumType") => &[b"countBy", b"start", b"distance", b"restart"],
        (b"sectPr", b"docGrid") => &[b"type", b"linePitch", b"charSpace"],
        (b"sectPr", b"footnotePr" | b"endnotePr" | b"sectPrChange") => &[],
        _ => return None,
    })
}

pub(crate) fn source_value_is_modeled(
    parent: &[u8],
    local: &[u8],
    key: &[u8],
    value: &str,
) -> bool {
    if parent != b"sectPr" {
        return true;
    }
    if matches!(local, b"headerReference" | b"footerReference") && key == b"type" {
        return HdrFtrType::from_str(value).to_str() == value;
    }
    if local == b"cols" && matches!(key, b"equalWidth" | b"sep") {
        return crate::numbering::parse_on_off_token(value)
            .is_some_and(|parsed| parsed == matches!(value, "1" | "true"));
    }
    if local == b"pgSz" && key == b"orient" {
        return ST_PageOrientation::from_str(value).is_ok();
    }
    if local == b"vAlign" && key == b"val" {
        return ST_VerticalJc::from_str(value).to_str() == value;
    }
    if local == b"type" && key == b"val" {
        return ST_SectionType::from_str(value).is_ok();
    }
    if local == b"titlePg" && key == b"val" {
        return crate::numbering::parse_on_off_token(value).is_some();
    }
    if local == b"docGrid" {
        return match key {
            b"type" => ST_DocGrid::from_str(value).is_some(),
            b"linePitch" | b"charSpace" => value.parse::<i32>().is_ok(),
            _ => false,
        };
    }
    if local == b"paperSrc" {
        return value.parse::<u32>().is_ok();
    }
    if local == b"lnNumType" {
        return match key {
            b"countBy" | b"start" => value.parse::<u32>().is_ok(),
            b"distance" => value.parse::<i32>().is_ok(),
            b"restart" => ST_LineNumberRestart::from_str(value).is_some(),
            _ => false,
        };
    }
    true
}
