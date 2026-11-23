//! Constructor kinds, part readers, simple selector folds and constructor failures.

use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) enum ConstructorKind {
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
    pub(super) fn action(self, local: &[u8], word: bool, empty: bool) -> ConstructorKind {
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

impl ConstructorFrame<'_> {
    pub(super) fn record_ruby_property(&mut self, outcome: Result<crate::ruby::RubyPropertyChild>) {
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
impl DirectSimpleSelectorFold {
    pub(super) fn element(
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
    pub(super) fn end(&mut self, physical_depth: usize, after: usize) -> bool {
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

impl ConstructorFailures {
    pub(super) fn with_original_offset(mut self, part: usize, offset: usize) -> Result<Self> {
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

    pub(super) fn after_insertion(mut self, at: usize, bytes: usize) -> Result<Self> {
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
