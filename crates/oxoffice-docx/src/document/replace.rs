//! Replacing text: placeholders, batches and regular expressions, in every story.

use super::*;

/// The pair of a [`Document::try_replace_all_expected`] batch whose count
/// differed from the count its caller expected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("pair {index}: expected {expected} replacement(s) of \"{placeholder}\", found {found}{}", out_of_reach(*.unreached))]
pub struct ReplacementCountMismatch {
    /// Zero-based position of the pair in the batch.
    pub index: usize,
    /// The literal text the pair searched for.
    pub placeholder: String,
    /// The count the caller expected.
    pub expected: usize,
    /// The count the pair found once the pairs before it had run.
    pub found: usize,
    /// The occurrences a reader sees in the paragraph or block control a
    /// scoped replacement searched, beyond those it found: text it cannot
    /// reach, such as a field's result. Zero for a table, a cell and a
    /// document-wide pair.
    pub unreached: usize,
}

/// How a count mismatch names the occurrences out of reach, if any.
pub(crate) fn out_of_reach(unreached: usize) -> String {
    if unreached == 0 {
        String::new()
    } else {
        format!(
            ", and {unreached} it cannot reach (in a field result, or across the edge of a content control or tracked change)"
        )
    }
}

enum ScopedReplacementTarget<'a> {
    Item(&'a ContentLocation),
    // Constructed only by the selector-owned operation after actual source
    // qualification of this same private candidate, before any mutation.
    CheckedItem(&'a ContentLocation),
    RelatedCell(&'a StoryId),
    Cell {
        coordinates: (usize, usize, usize),
        paragraph: Option<usize>,
    },
}

impl Document {
    // ---- Placeholder replacement ----

    /// Replace all occurrences of `placeholder` with `replacement` throughout the document.
    ///
    /// Searches body paragraphs, tables (including nested), headers, footers,
    /// footnotes, endnotes, the text boxes of the body, headers and footers,
    /// and the content controls at every
    /// level of them, inline controls included. It reads the text
    /// [`crate::Paragraph::text`] reads: the text of a tracked insertion or
    /// move destination, replaced inside that revision, and not deleted or
    /// moved-away text. A text box inside a deleted run is the exception, as
    /// the text box pass rewrites it like any other text box. A text box
    /// inside a note, and the separator and continuation entries of the notes
    /// parts, are never searched. Handles placeholders split across
    /// multiple runs. A match that straddles a content-control or a
    /// tracked-insertion boundary is not replaced. Returns the total number
    /// of replacements made.
    ///
    /// A tab or a positional tab reads as `\t`, a non-breaking hyphen as `-`
    /// and every break, a page or column break included, as `\n`, as in
    /// [`crate::Paragraph::text`], so a match does not skip them and one that
    /// holds them removes them. A `\t` in `replacement` becomes a tab, a `-` a
    /// plain hyphen and a `\n`, `\r` or `\r\n` one line break: a page or
    /// column break written back by a replacement is a line break, a
    /// positional tab an ordinary tab and a non-breaking hyphen one a line
    /// can break at.
    ///
    /// A `replacement` that contains `placeholder` is substituted once, not
    /// repeatedly.
    ///
    /// A character XML 1.0 cannot carry is dropped from `replacement`;
    /// [`Self::try_replace_text`] refuses it instead.
    pub fn replace_text(&mut self, placeholder: &str, replacement: &str) -> usize {
        let replacement = oxoffice_core::xml::strip_non_xml_characters(replacement);
        let mut candidate = self.clone_for_staging();
        let count = candidate
            .replace_batch(&[(placeholder, &replacement)])
            .expect("text replacement package preflight failed");
        self.commit_staged_mutation(candidate);
        count
    }

    /// Fallible twin of [`Self::replace_text`].
    ///
    /// The complete replacement is staged, serialized, and reopened before it
    /// replaces the live document. A preflight failure leaves `self` unchanged.
    /// A `replacement` holding a character XML 1.0 cannot carry, such as
    /// U+0001, is refused with an error naming it and its position, as
    /// python-docx refuses such text.
    pub fn try_replace_text(&mut self, placeholder: &str, replacement: &str) -> Result<usize> {
        oxoffice_core::xml::reject_non_xml_characters("replacement text", replacement)?;
        let mut candidate = self.clone_for_staging();
        let count = candidate.replace_batch(&[(placeholder, replacement)])?;
        self.commit_staged_mutation(candidate);
        Ok(count)
    }

    /// Replace literal text only within one checked paragraph, table or block control.
    /// Matching retains run formatting and never crosses paragraph or wrapper boundaries.
    /// Count mismatch and preparation failures leave the complete document unchanged.
    pub fn try_replace_text_at(
        &mut self,
        location: &ContentLocation,
        placeholder: &str,
        replacement: &str,
        expected: Option<usize>,
    ) -> Result<std::result::Result<usize, ReplacementCountMismatch>> {
        self.try_replace_scoped(
            ScopedReplacementTarget::Item(location),
            placeholder,
            replacement,
            expected,
        )
    }

    /// Replace text in exactly one physical body table cell or one of its paragraphs.
    /// Whole-cell scope includes supported nested tables and controls. The
    /// row and cell count as [`crate::TableRef::cell`] does.
    #[doc(hidden)]
    pub fn try_replace_text_in_cell(
        &mut self,
        cell: (usize, usize, usize),
        paragraph: Option<usize>,
        placeholder: &str,
        replacement: &str,
        expected: Option<usize>,
    ) -> Result<std::result::Result<usize, ReplacementCountMismatch>> {
        self.try_replace_scoped(
            ScopedReplacementTarget::Cell {
                coordinates: cell,
                paragraph,
            },
            placeholder,
            replacement,
            expected,
        )
    }

    fn try_replace_scoped(
        &mut self,
        target: ScopedReplacementTarget<'_>,
        placeholder: &str,
        replacement: &str,
        expected: Option<usize>,
    ) -> Result<std::result::Result<usize, ReplacementCountMismatch>> {
        oxoffice_core::xml::reject_non_xml_characters("replacement text", replacement)?;
        let literal_input = Cell::new(false);
        let candidate = self.clone_for_staging();
        match Self::replace_scoped_candidate(
            candidate,
            target,
            placeholder,
            replacement,
            expected,
            &literal_input,
        )? {
            Err(mismatch) => Ok(Err(mismatch)),
            Ok(None) => Ok(Ok(0)),
            Ok(Some((updated, count))) => {
                self.commit_staged_mutation(updated);
                Ok(Ok(count))
            }
        }
    }

    /// Consume an already private candidate. No change or mismatch returns no candidate.
    #[doc(hidden)]
    pub fn try_replace_text_at_owned(
        self,
        location: &ContentLocation,
        placeholder: &str,
        replacement: &str,
        expected: Option<usize>,
    ) -> Result<std::result::Result<Option<(Self, usize)>, ReplacementCountMismatch>> {
        self.replace_scoped_owned(
            ScopedReplacementTarget::Item(location),
            placeholder,
            replacement,
            expected,
        )
    }

    /// Resolve a current story selector and replace a checked one-index item.
    /// Full story discovery precedes literal and item validation, matching
    /// bound Python StoryItem selection without trusting a stored fingerprint.
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub fn try_replace_text_at_selector_owned(
        self,
        story_kind: StoryKind,
        part_name: &str,
        owner_index: usize,
        item_kind: StoryItemKind,
        index_path: &[usize],
        placeholder: &str,
        replacement: &str,
        expected: Option<usize>,
    ) -> Result<std::result::Result<Option<(Self, usize)>, ReplacementCountMismatch>> {
        let location = {
            let (sources, content, children) = self.spanned_story_sources()?;
            let main = self
                .main_story_layout(&sources[0].xml, &content, &children, false)
                .ok();
            let borrowed = sources
                .iter()
                .map(|source| StorySource {
                    root_kind: source.root_kind,
                    part_name: source.part_name.clone(),
                    xml: Cow::Borrowed(source.xml.as_ref()),
                })
                .collect();
            // Collect the complete discovery before selection. An unrelated
            // malformed source keeps its original precedence.
            let story = Self::stories_in_sources(borrowed, main)?
                .into_iter()
                .find(|story| {
                    story.kind == story_kind
                        && story.part_name == part_name
                        && story.owner_index == owner_index
                })
                .ok_or_else(|| {
                    let kind = match story_kind {
                        StoryKind::Body => "body",
                        StoryKind::TableCell => "table_cell",
                        StoryKind::Header => "header",
                        StoryKind::Footer => "footer",
                        StoryKind::Footnote => "footnote",
                        StoryKind::Endnote => "endnote",
                        StoryKind::Comment => "comment",
                        StoryKind::TextBox => "text_box",
                    };
                    Error::Other(format!(
                        "document has no {kind} story {part_name} at owner index {owner_index}"
                    ))
                })?;
            oxoffice_core::xml::reject_non_xml_characters("replacement text", replacement)?;
            let location = ContentLocation::new(story, item_kind, index_path.to_vec());
            if !matches!(
                item_kind,
                StoryItemKind::Paragraph | StoryItemKind::Table | StoryItemKind::ContentControl
            ) {
                return Err(Error::Other(
                    "scoped replacement requires an existing paragraph, table or content control"
                        .into(),
                ));
            }
            let (source, owner, layout) = self.story_source_owner_and_layout_in(
                &location.story,
                true,
                sources,
                &content,
                &children,
            )?;
            Self::story_item_span(source.xml.as_ref(), &owner, layout.as_ref(), &location)?;
            location
        };
        // No model/package mutation occurs between qualification and this
        // owned call. CheckedItem cannot be supplied by any public caller.
        self.replace_scoped_owned(
            ScopedReplacementTarget::CheckedItem(&location),
            placeholder,
            replacement,
            expected,
        )
    }

    /// Consume an already private candidate. No change or mismatch returns no candidate.
    #[doc(hidden)]
    pub fn try_replace_text_in_cell_owned(
        self,
        cell: (usize, usize, usize),
        paragraph: Option<usize>,
        placeholder: &str,
        replacement: &str,
        expected: Option<usize>,
    ) -> Result<std::result::Result<Option<(Self, usize)>, ReplacementCountMismatch>> {
        self.replace_scoped_owned(
            ScopedReplacementTarget::Cell {
                coordinates: cell,
                paragraph,
            },
            placeholder,
            replacement,
            expected,
        )
    }

    /// Consume an already private candidate. No change or mismatch returns no candidate.
    #[doc(hidden)]
    pub fn try_replace_text_in_story_cell_owned(
        self,
        story: &StoryId,
        placeholder: &str,
        replacement: &str,
        expected: Option<usize>,
    ) -> Result<std::result::Result<Option<(Self, usize)>, ReplacementCountMismatch>> {
        self.replace_scoped_owned(
            ScopedReplacementTarget::RelatedCell(story),
            placeholder,
            replacement,
            expected,
        )
    }

    fn replace_scoped_owned(
        mut self,
        target: ScopedReplacementTarget<'_>,
        placeholder: &str,
        replacement: &str,
        expected: Option<usize>,
    ) -> Result<std::result::Result<Option<(Self, usize)>, ReplacementCountMismatch>> {
        oxoffice_core::xml::reject_non_xml_characters("replacement text", replacement)?;
        let literal_input = Cell::new(false);
        let mut origin = self.take_staged_commit_origin();
        match Self::replace_scoped_candidate(
            self,
            target,
            placeholder,
            replacement,
            expected,
            &literal_input,
        )? {
            Err(mismatch) => Ok(Err(mismatch)),
            Ok(None) => Ok(Ok(None)),
            Ok(Some((updated, count))) => Ok(Ok(Some((
                Self::finish_staged_candidate(
                    &origin.package,
                    origin.signature_invalidated,
                    &mut origin.normal_engine,
                    &mut origin.bundled_engine,
                    updated,
                ),
                count,
            )))),
        }
    }

    fn replace_scoped_candidate(
        mut candidate: Self,
        target: ScopedReplacementTarget<'_>,
        placeholder: &str,
        replacement: &str,
        expected: Option<usize>,
        literal_input: &Cell<bool>,
    ) -> Result<std::result::Result<Option<(Self, usize)>, ReplacementCountMismatch>> {
        let searched = match &target {
            ScopedReplacementTarget::CheckedItem(location)
            | ScopedReplacementTarget::Item(location) => Some((*location).clone()),
            _ => None,
        };
        let (story, path, paragraph) = match target {
            ScopedReplacementTarget::CheckedItem(location) => {
                (location.story.clone(), Some(location.clone()), None)
            }
            ScopedReplacementTarget::Item(location) => {
                if location.is_end
                    || !matches!(
                        location.item_kind,
                        StoryItemKind::Paragraph
                            | StoryItemKind::Table
                            | StoryItemKind::ContentControl
                    )
                {
                    return Err(Error::Other("scoped replacement requires an existing paragraph, table or content control".into()));
                }
                candidate.story_range_paragraph_source(location)?;
                (location.story.clone(), Some(location.clone()), None)
            }
            ScopedReplacementTarget::RelatedCell(story) => {
                if story.kind() != StoryKind::TableCell {
                    return Err(Error::Other(
                        "scoped cell replacement needs a table-cell owner".to_owned(),
                    ));
                }
                candidate.story_source_and_owner(story)?;
                (story.clone(), None, None)
            }
            ScopedReplacementTarget::Cell {
                coordinates: (table, row, cell),
                paragraph,
            } => {
                let selected = candidate
                    .table(table)
                    .and_then(|t| t.cell(row, cell).map(|c| c.inner as *const CT_Tc))
                    .ok_or_else(|| {
                        Error::Other(format!("cell {table}:{row}:{cell} is out of range"))
                    })?;
                let mut found = candidate.content_index_of_table(table).and_then(|slot| {
                    candidate
                        .direct_body_cell_story(slot, row, cell)
                        .ok()
                        .flatten()
                });
                // Tables inside controlled content retain the legacy modeled
                // pointer correspondence, including its failure behavior.
                if found.is_none() {
                    let xml = candidate.document.to_xml()?;
                    let owners = scan_story_owners(&xml, StoryKind::Body)?;
                    let routes = owners
                        .iter()
                        .find(|owner| owner.kind == StoryKind::Body)
                        .and_then(|body| direct_story_content_items(&xml, body).ok())
                        .and_then(|items| modeled_main_cell_routes(&owners, &items));
                    for owner in owners
                        .iter()
                        .filter(|owner| owner.kind == StoryKind::TableCell)
                    {
                        let (content, mut ordinal) = match routes
                            .as_ref()
                            .and_then(|routes| routes.get(&owner.owner_index))
                            .copied()
                        {
                            Some(route) => route,
                            None => modeled_main_cell_route(&xml, owner)?,
                        };
                        let modeled = match candidate.document.body.content.get_mut(content) {
                            Some(BodyContent::Table(t)) => nth_cell_in_table(t, &mut ordinal),
                            Some(BodyContent::ContentControl(c)) => {
                                nth_cell_in_control(c, &mut ordinal)
                            }
                            _ => None,
                        };
                        if modeled.is_some_and(|c| std::ptr::eq(selected, c)) {
                            found = Some(StoryId {
                                kind: owner.kind,
                                part_name: candidate.doc_part_name.clone(),
                                owner_index: owner.owner_index,
                                fingerprint: owner.fingerprint,
                            });
                            break;
                        }
                    }
                }
                (
                    found.ok_or_else(|| {
                        Error::Other("cell has no checked physical story owner".into())
                    })?,
                    None,
                    paragraph,
                )
            }
        };
        // Flush only the private candidate before choosing the preserved physical span.
        candidate.prepare_staged_package()?;
        let read = candidate.prepared_main_read()?;
        let related = candidate.related_story_sources()?;
        let (canonical, canonical_owner, layout) =
            read.owner_and_layout(&story, path.is_some(), &related)?;
        let (source, owner, actual_owners) =
            candidate.fragment_story_source_from_canonical(&story, &canonical)?;
        let canonical_owners = match &layout {
            Some(layout) => layout.owner_kinds(),
            None => scan_story_owners(&canonical.xml, canonical.root_kind)?
                .iter()
                .map(|owner| owner.kind)
                .collect(),
        };
        if canonical_owners
            != actual_owners
                .iter()
                .map(|owner| owner.kind)
                .collect::<Vec<_>>()
        {
            return Err(Error::Other(
                "scoped replacement namespace context changes physical ownership".into(),
            ));
        }
        let (canonical_span, span) = if let Some(location) = path {
            let canonical_items = match &layout {
                Some(layout) => layout.owner_items(&canonical.xml, &canonical_owner)?,
                None => scan_story_items(&canonical.xml, &canonical_owner, None)?,
            };
            let index = location.index_path[0];
            let selected = canonical_items.get(index).ok_or(StoryError::OutOfBounds {
                index,
                len: canonical_items.len(),
            })?;
            let block_owner_item =
                || Error::Other("scoped replacement requires a block owner item".into());
            // A paragraph of a nested block control names that control's own
            // item: it is found through its direct ancestor control.
            let (direct_index, nested) = if selected.direct_owner_child {
                (index, 0)
            } else if location.index_path.len() == 2 {
                let (direct_index, _) = canonical_items[..index]
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, item)| item.direct_owner_child && selected.full.end <= item.full.end)
                    .ok_or_else(block_owner_item)?;
                let nested = control_items(&canonical_items, direct_index)
                    .iter()
                    .position(|listed| *listed == index)
                    .ok_or_else(block_owner_item)?;
                (direct_index, nested)
            } else {
                return Err(block_owner_item());
            };
            let direct_selected = &canonical_items[direct_index];
            let canonical_direct =
                direct_story_content_items_in(&canonical.xml, &canonical_owner, &canonical_items)?;
            let actual_items = scan_story_items(&source.xml, &owner, None)?;
            let actual_direct = direct_story_content_items_in(&source.xml, &owner, &actual_items)?;
            let slot = canonical_direct
                .iter()
                .position(|item| item.full == direct_selected.full)
                .ok_or_else(|| {
                    Error::Other("scoped replacement has no direct physical slot".into())
                })?;
            let direct_item = actual_direct.get(slot).ok_or(StoryError::OutOfBounds {
                index: slot,
                len: actual_direct.len(),
            })?;
            if direct_item.kind != direct_selected.kind {
                return Err(StoryError::KindMismatch {
                    expected: direct_selected.kind,
                    actual: direct_item.kind,
                }
                .into());
            }
            let item = if nested == 0 {
                direct_item
            } else {
                let actual_index = actual_items
                    .iter()
                    .position(|item| item.full == direct_item.full)
                    .ok_or_else(block_owner_item)?;
                let canonical_controls = control_items(&canonical_items, direct_index);
                let actual_controls = control_items(&actual_items, actual_index);
                if canonical_controls.len() != actual_controls.len() {
                    return Err(Error::Other(
                        "scoped replacement nested controls changed".into(),
                    ));
                }
                &actual_items[actual_controls[nested]]
            };
            if let [_, ordinal] = location.index_path.as_slice() {
                // Paragraph or table `ordinal` of the control, by the kind.
                let block = control_block_name(location.item_kind)
                    .ok_or_else(|| Error::Other("scoped replacement requires a block".into()))?;
                let canonical_paragraphs =
                    scan_story_control_blocks(&canonical.xml, selected, block)?;
                let paragraphs = scan_story_control_blocks(&source.xml, item, block)?;
                let select = |paragraphs: &[Range<usize>]| {
                    paragraphs
                        .get(*ordinal)
                        .cloned()
                        .ok_or(StoryError::OutOfBounds {
                            index: *ordinal,
                            len: paragraphs.len(),
                        })
                };
                (select(&canonical_paragraphs)?, select(&paragraphs)?)
            } else {
                (selected.full.clone(), item.full.clone())
            }
        } else {
            (canonical_owner.full.clone(), owner.full.clone())
        };
        if source.part_name == candidate.doc_part_name {
            read.prove_scoped_main_source(
                &canonical.xml,
                canonical_span,
                &source.xml,
                span.clone(),
            )?;
        } else if canonical.xml != source.xml || canonical_span != span {
            return Err(Error::Other(
                "scoped related-story source correspondence changed".into(),
            ));
        }
        let part = source.part_name.clone();
        let mut xml = source.xml.into_owned();
        let scope = story_namespace_scope_at(&xml, span.start)?;
        let mut operation = oxoffice_docx_oxml::document::ScopedWordOperation::new(&xml);
        let selected = operation.replace_block_with_literal_input(
            span.clone(),
            &scope,
            paragraph,
            placeholder,
            replacement,
            literal_input,
        );
        let (raw, count) = selected?;
        let mut raw = without_inherited_root_declarations(&xml[span.clone()], raw, &scope);
        if let Some(expected) = expected
            && count != expected
        {
            // What a reader sees of the placeholder in the item, beyond what
            // the replacement found there.
            let unreached = searched
                .filter(|location| location.item_kind != StoryItemKind::Table)
                .map_or(0, |location| {
                    candidate.story_paragraphs().map_or(0, |paragraphs| {
                        paragraphs
                            .iter()
                            .filter(|paragraph| {
                                paragraph.location.story.kind == location.story.kind
                                    && paragraph.location.story.part_name
                                        == location.story.part_name
                                    && paragraph.location.story.owner_index
                                        == location.story.owner_index
                                    && paragraph
                                        .location
                                        .index_path
                                        .starts_with(&location.index_path)
                            })
                            .map(|paragraph| paragraph.text.matches(placeholder).count())
                            .sum::<usize>()
                            .saturating_sub(count)
                    })
                });
            return Ok(Err(ReplacementCountMismatch {
                index: 0,
                placeholder: placeholder.into(),
                expected,
                found: count,
                unreached,
            }));
        }
        if count == 0 {
            return Ok(Ok(None));
        }
        if let Some(edits) = paired_scoped_edits(
            &xml,
            &owner,
            &PairedReplacement {
                span: &span,
                paragraph,
                placeholder,
                replacement,
                literal_input,
                count,
            },
            &mut raw,
            &mut operation,
        )? {
            for edit in edits.into_iter().rev() {
                xml.splice(edit.range, edit.raw);
            }
        } else {
            xml.splice(span, raw);
        }
        drop(canonical);
        drop(read);
        drop(related);
        set_story_source_xml(&mut candidate, &part, xml)?;
        candidate.prepare_staged_package()?;
        let limits = PackageReadLimits::UNBOUNDED;
        let reopened = candidate.reopen_prepared_staged_with_limits(limits)?;
        Ok(Ok(Some((reopened, count))))
    }

    // A private model probe distinguishes physical positions even when siblings
    // contain identical text. Producer normalization is allowed, raw output is
    // still edited only at the proved retained span.

    pub(super) fn prove_whole_scoped_main_source(
        &self,
        canonical: &[u8],
        canonical_span: Range<usize>,
        original: &[u8],
        retained_span: Range<usize>,
    ) -> Result<()> {
        let restored = self.namespace_restored_main_source()?;
        let (boundaries, _) = comment_namespace_source_boundaries(canonical, &restored)?
            .ok_or_else(|| {
                Error::Other("scoped canonical namespace source changed payload".into())
            })?;
        let start = *boundaries.get(&canonical_span.start).ok_or_else(|| {
            Error::Other("scoped start has no physical namespace boundary".into())
        })?;
        let end = *boundaries
            .get(&canonical_span.end)
            .ok_or_else(|| Error::Other("scoped end has no physical namespace boundary".into()))?;
        // One typed serialization establishes the same canonical defaults and
        // root namespace inventory on both sides before model comparison.
        let normalize = |xml: &[u8]| -> Result<CT_Document> {
            let written = CT_Document::from_xml(xml)?.to_xml()?;
            Ok(CT_Document::from_xml(&written)?)
        };
        let baseline = normalize(original)?;
        let current = self.document.to_xml()?;
        if normalize(&current)? != baseline || normalize(&restored)? != baseline {
            return Err(Error::Other(
                "scoped source has no equivalent checked model projection".into(),
            ));
        }
        let marker = scoped_proof_marker(original, &restored)?;
        let scope = story_namespace_scope_at(&restored, start)?;
        let selected_closed = close_content_fragment_namespaces(&restored[start..end], &scope)?;
        let sentinel = scoped_proof_sentinel(&selected_closed, &marker)?;
        let mut checked_probe = restored;
        checked_probe.splice(start..end, sentinel.bytes());
        let mut retained_probe = original.to_vec();
        retained_probe.splice(retained_span, sentinel.bytes());
        let checked_model = normalize(&checked_probe)?;
        let retained_model = normalize(&retained_probe)?;
        let occurrences = |model: &CT_Document| -> Result<usize> {
            let written = model.to_xml()?;
            Ok(written
                .windows(marker.len())
                .filter(|bytes| *bytes == marker.as_bytes())
                .count())
        };
        if occurrences(&checked_model)? != 1
            || occurrences(&retained_model)? != 1
            || checked_model != retained_model
        {
            return Err(Error::Other(
                "scoped selection does not identify the same physical source".into(),
            ));
        }
        Ok(())
    }

    /// Replace multiple placeholders at once. Returns total replacements.
    ///
    /// The pairs apply together, each to the text as it was before the call:
    /// a value is never searched by another pair, so `{"left": "right",
    /// "right": "left"}` swaps the two words. Where two placeholders overlap,
    /// the longer one (in characters) wins, then the one that sorts first,
    /// whatever the order of the map. [`Self::try_replace_all_expected`] and
    /// [`Self::try_replace_all_checked`] apply ordered pairs one after another
    /// instead, as Python `Document.replace_all` and the CLI's `replace --map`
    /// do through the checked one: their pairs come in a list, so a later pair
    /// sees what an earlier one wrote.
    ///
    /// Cheaper than calling [`Self::replace_text`] per entry: the document is
    /// serialised and re-parsed once per pass for the whole batch rather than
    /// once per placeholder. A character XML 1.0 cannot carry is dropped from
    /// each value.
    pub fn replace_all(&mut self, replacements: &std::collections::HashMap<&str, &str>) -> usize {
        let values: Vec<(&str, std::borrow::Cow<'_, str>)> = replacements
            .iter()
            .map(|(k, v)| (*k, oxoffice_core::xml::strip_non_xml_characters(v)))
            .collect();
        let mut pairs: Vec<(&str, &str)> = values.iter().map(|(k, v)| (*k, v.as_ref())).collect();
        pairs.sort_by(|(left, _), (right, _)| {
            (right.chars().count().cmp(&left.chars().count())).then(left.cmp(right))
        });
        // Each placeholder becomes a private marker first, then each marker
        // its value, so that no value meets another pair.
        let markers = batch_markers(&pairs);
        let to_markers = pairs
            .iter()
            .zip(&markers)
            .map(|((placeholder, _), marker)| (*placeholder, marker.as_str()))
            .collect::<Vec<_>>();
        let to_values = markers
            .iter()
            .zip(&pairs)
            .map(|(marker, (_, value))| (marker.as_str(), *value))
            .collect::<Vec<_>>();
        let mut candidate = self.clone_for_staging();
        let count = candidate
            .replace_batch(&to_markers)
            .expect("text replacement package preflight failed");
        // Even with no match counted: the VML copy of a text box is rewritten
        // without counting.
        candidate
            .replace_batch(&to_values)
            .expect("text replacement package preflight failed");
        self.commit_staged_mutation(candidate);
        count
    }

    /// Replace literal pairs in order, all or nothing, and return the count of
    /// each pair.
    ///
    /// Each pair searches what [`Self::replace_text`] searches and runs over
    /// the whole document after the pairs before it, so a later pair sees the
    /// text an earlier one wrote. A pair may give the count it expects. When a
    /// pair finds another count, the document is left unchanged and the inner
    /// error names that pair. A pair without an expected count may find
    /// nothing. The outer error is a staging failure, which also leaves the
    /// document unchanged.
    pub fn try_replace_all_expected(
        &mut self,
        pairs: &[(&str, &str, Option<usize>)],
    ) -> Result<std::result::Result<Vec<usize>, ReplacementCountMismatch>> {
        if pairs.is_empty() {
            return Ok(Ok(Vec::new()));
        }
        let mut candidate = self.clone_for_staging();
        let mut counts = Vec::with_capacity(pairs.len());
        for (index, &(placeholder, replacement, expected)) in pairs.iter().enumerate() {
            let found = candidate.replace_batch(&[(placeholder, replacement)])?;
            if let Some(expected) = expected
                && expected != found
            {
                return Ok(Err(ReplacementCountMismatch {
                    index,
                    placeholder: placeholder.to_owned(),
                    expected,
                    found,
                    unreached: 0,
                }));
            }
            counts.push(found);
        }
        self.commit_staged_mutation(candidate);
        Ok(Ok(counts))
    }

    /// Consume an already-private ordered replacement candidate, publishing only positive counts.
    #[doc(hidden)]
    #[allow(clippy::type_complexity)]
    pub fn try_replace_all_expected_owned(
        mut self,
        pairs: &[(&str, &str, Option<usize>)],
    ) -> Result<std::result::Result<Option<(Self, Vec<usize>)>, ReplacementCountMismatch>> {
        if pairs.is_empty() {
            return Ok(Ok(None));
        }
        let mut origin = self.take_staged_commit_origin();
        let mut counts = Vec::with_capacity(pairs.len());
        for (index, &(placeholder, replacement, expected)) in pairs.iter().enumerate() {
            let found = self.replace_batch(&[(placeholder, replacement)])?;
            if let Some(expected) = expected
                && expected != found
            {
                return Ok(Err(ReplacementCountMismatch {
                    index,
                    placeholder: placeholder.to_owned(),
                    expected,
                    found,
                    unreached: 0,
                }));
            }
            counts.push(found);
        }
        if counts.iter().all(|count| *count == 0) {
            return Ok(Ok(None));
        }
        let candidate = Self::finish_staged_candidate(
            &origin.package,
            origin.signature_invalidated,
            &mut origin.normal_engine,
            &mut origin.bundled_engine,
            self,
        );
        Ok(Ok(Some((candidate, counts))))
    }
}

impl Document {
    /// Apply a batch of literal replacements across the whole document.
    pub(super) fn replace_batch(&mut self, pairs: &[(&str, &str)]) -> Result<usize> {
        if pairs.is_empty() {
            return Ok(0);
        }
        if let [(placeholder, replacement)] = pairs {
            return self.with_replace_pattern(
                || super::tracked::ReplacePattern::literal(placeholder, replacement),
                |document| document.replace_batch_with(pairs),
            );
        }
        self.replace_batch_with(pairs)
    }

    fn replace_batch_with(&mut self, pairs: &[(&str, &str)]) -> Result<usize> {
        let mut count = 0;

        // Typed model: body content, then headers and footers. Under tracked
        // changes, each paragraph the replacement changed is written as
        // tracked changes.
        let before = self
            .tracking
            .is_some()
            .then(|| self.document.body.content.clone());
        for (placeholder, replacement) in pairs {
            count += self.replace_in_body(placeholder, replacement)?;
        }
        if let Some(before) = before {
            self.redline_body(&before)?;
        }
        count += self.replace_in_headers_footers(pairs)?;

        // Raw XML: text boxes and shapes live in markup the typed model
        // does not cover, so flush first and work on the serialised parts.
        self.flush_to_package()?;
        count += self.replace_in_xml_parts(pairs)?;

        Ok(count)
    }

    /// Run the typed replacement over every body paragraph, those of tables
    /// and content controls at every level included.
    fn replace_in_body(&mut self, placeholder: &str, replacement: &str) -> Result<usize> {
        let mut failure = None;
        let mut count = 0;
        visit_body_paragraphs_mut(&mut self.document.body.content, &mut |paragraph| {
            if failure.is_none() {
                match oxoffice_docx_oxml::placeholder::try_replace_in_paragraph(
                    paragraph,
                    placeholder,
                    replacement,
                ) {
                    Ok(found) => count += found,
                    Err(error) => failure = Some(error),
                }
            }
        });
        match failure {
            Some(error) => Err(error.into()),
            None => Ok(count),
        }
    }

    /// Run the typed replacement over every referenced header and footer part.
    fn replace_in_headers_footers(&mut self, pairs: &[(&str, &str)]) -> Result<usize> {
        use oxoffice_docx_oxml::placeholder;

        let mut count = 0;
        for (rel_id, is_header) in self.header_footer_rel_ids() {
            let Some(mut hf) = self.load_header_footer(&rel_id, is_header) else {
                continue;
            };
            let before = self.tracking.is_some().then(|| hf.clone());
            let mut part_count = 0;
            for (placeholder, replacement) in pairs {
                part_count +=
                    placeholder::try_replace_in_header_footer(&mut hf, placeholder, replacement)?;
            }
            if part_count > 0 {
                if let Some(before) = before {
                    self.redline_header_footer(&before, &mut hf)?;
                }
                self.save_header_footer(&rel_id, &hf, is_header)?;
                count += part_count;
            }
        }
        Ok(count)
    }
}

impl Document {
    // ---- Regex replacement ----

    /// Replace all regex matches with `replacement` throughout the document.
    ///
    /// The `replacement` string supports capture groups: `$1`, `$2`, etc.
    /// Searches the stories [`Self::replace_text`] searches, except chart
    /// labels: body paragraphs, tables (including nested), headers, footers,
    /// footnotes, endnotes and text boxes, the content controls at every level
    /// of them and the text of tracked insertions. Returns the total number of replacements made, or an error if the
    /// regex is invalid.
    ///
    /// The text reads tabs, positional tabs, non-breaking hyphens and breaks
    /// as [`Self::replace_text`] does: `\s` matches a tab, a positional tab
    /// and any break, a page break included, so a pattern such as `\s+`
    /// replaces them too, and a break a capture group writes back becomes a
    /// line break, a positional tab an ordinary tab.
    pub fn replace_regex(&mut self, pattern: &str, replacement: &str) -> Result<usize> {
        oxoffice_core::xml::reject_non_xml_characters("replacement text", replacement)?;
        let re =
            regex::Regex::new(pattern).map_err(|e| Error::Other(format!("invalid regex: {e}")))?;
        let mut candidate = self.clone_for_staging();
        let count = candidate.replace_regex_compiled(&re, replacement)?;
        self.commit_staged_mutation(candidate);
        Ok(count)
    }

    /// Replace multiple regex patterns at once. Returns total replacements.
    pub fn replace_all_regex(&mut self, patterns: &[(String, String)]) -> Result<usize> {
        for (_, replacement) in patterns {
            oxoffice_core::xml::reject_non_xml_characters("replacement text", replacement)?;
        }
        let compiled = patterns
            .iter()
            .map(|(pattern, replacement)| {
                regex::Regex::new(pattern)
                    .map(|regex| (regex, replacement.as_str()))
                    .map_err(|error| Error::Other(format!("invalid regex: {error}")))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut candidate = self.clone_for_staging();
        let mut count = 0;
        for (regex, replacement) in &compiled {
            count += candidate.replace_regex_compiled(regex, replacement)?;
        }
        self.commit_staged_mutation(candidate);
        Ok(count)
    }

    /// Apply ordered regex pairs with checked owner errors and atomic expected counts.
    /// Patterns use Rust regex syntax. Empty batches preserve the original state.
    pub fn try_replace_all_regex_expected(
        &mut self,
        pairs: &[(&str, &str, Option<usize>)],
    ) -> Result<std::result::Result<Vec<usize>, ReplacementCountMismatch>> {
        let compiled = pairs
            .iter()
            .map(|(pattern, replacement, expected)| {
                oxoffice_core::xml::reject_non_xml_characters("replacement text", replacement)?;
                let regex = regex::Regex::new(pattern)
                    .map_err(|error| Error::Other(format!("invalid regex: {error}")))?;
                Ok((regex, *replacement, *expected))
            })
            .collect::<Result<Vec<_>>>()?;
        if pairs.is_empty() {
            return Ok(Ok(Vec::new()));
        }
        let mut candidate = self.clone_for_staging();
        let mut counts = Vec::with_capacity(pairs.len());
        for (index, (regex, replacement, expected)) in compiled.iter().enumerate() {
            let found = candidate.replace_regex_compiled_checked(regex, replacement)?;
            if let Some(expected) = expected
                && *expected != found
            {
                return Ok(Err(ReplacementCountMismatch {
                    index,
                    placeholder: pairs[index].0.to_owned(),
                    expected: *expected,
                    found,
                    unreached: 0,
                }));
            }
            counts.push(found);
        }
        self.commit_staged_mutation(candidate.prepare_and_reopen_staged()?);
        Ok(Ok(counts))
    }

    fn replace_regex_compiled_checked(
        &mut self,
        re: &regex::Regex,
        replacement: &str,
    ) -> Result<usize> {
        self.with_replace_pattern(
            || super::tracked::ReplacePattern::Regex {
                regex: re.clone(),
                replacement: replacement.to_owned(),
            },
            |document| document.replace_regex_checked_with(re, replacement),
        )
    }

    fn replace_regex_checked_with(
        &mut self,
        re: &regex::Regex,
        replacement: &str,
    ) -> Result<usize> {
        use oxoffice_docx_oxml::placeholder;
        self.flush_to_package()?;
        let mut count = 0;
        let before = self
            .tracking
            .is_some()
            .then(|| self.document.body.content.clone());
        let mut failure = None;
        visit_body_paragraphs_mut(&mut self.document.body.content, &mut |paragraph| {
            if failure.is_none() {
                match placeholder::try_replace_regex_in_paragraph(paragraph, re, replacement) {
                    Ok(found) => count += found,
                    Err(error) => failure = Some(error),
                }
            }
        });
        if let Some(error) = failure {
            return Err(error.into());
        }
        if let Some(before) = before {
            self.redline_body(&before)?;
        }
        for (rel_id, is_header) in self.header_footer_rel_ids() {
            let Some(mut hf) = self.load_header_footer(&rel_id, is_header) else {
                continue;
            };
            let before = self.tracking.is_some().then(|| hf.clone());
            let n = placeholder::try_replace_regex_in_header_footer(&mut hf, re, replacement)?;
            if n > 0 {
                if let Some(before) = before {
                    self.redline_header_footer(&before, &mut hf)?;
                }
                self.save_header_footer(&rel_id, &hf, is_header)?;
                count += n;
            }
        }
        self.flush_to_package()?;
        for (part_name, _) in self.text_bearing_part_names() {
            let xml = self
                .package
                .get_part(&part_name)
                .ok_or_else(|| Error::Other(format!("required text owner {part_name} is missing")))?
                .to_vec();
            let (changed, n) = placeholder::replace_regex_in_xml_part(&xml, re, replacement)?;
            if self.tracking.is_some() && changed != xml {
                return Err(super::redline::refused("a replacement in a text box"));
            }
            if changed != xml {
                if part_name == self.doc_part_name {
                    self.document =
                        crate::document::DocumentModel::unproved(CT_Document::from_xml(&changed)?);
                }
                self.package.set_part(&part_name, changed);
            }
            count += n;
        }
        for part_name in self.note_part_names() {
            let xml = self
                .package
                .get_part(&part_name)
                .ok_or_else(|| Error::Other(format!("required note owner {part_name} is missing")))?
                .to_vec();
            let (mut changed, n) = placeholder::replace_regex_in_notes_part(&xml, re, replacement)?;
            if let Some(tracking) = &self.tracking
                && changed != xml
            {
                changed = super::redline::redline_part_paragraphs(
                    &xml,
                    &changed,
                    tracking,
                    &mut self.identifiers,
                )?;
            }
            if changed != xml {
                if self.footnotes_part_name.as_deref() == Some(part_name.as_str()) {
                    self.footnotes =
                        oxoffice_docx_oxml::footnotes::CT_Footnotes::from_xml(&changed)?;
                }
                self.package.set_part(&part_name, changed);
            }
            count += n;
        }
        Ok(count)
    }

    /// Internal: replace using a pre-compiled regex.
    fn replace_regex_compiled(&mut self, re: &regex::Regex, replacement: &str) -> Result<usize> {
        self.with_replace_pattern(
            || super::tracked::ReplacePattern::Regex {
                regex: re.clone(),
                replacement: replacement.to_owned(),
            },
            |document| document.replace_regex_with(re, replacement),
        )
    }

    fn replace_regex_with(&mut self, re: &regex::Regex, replacement: &str) -> Result<usize> {
        use oxoffice_docx_oxml::placeholder;

        let mut count = 0;

        // Replace in body paragraphs, tables and content controls
        let before = self
            .tracking
            .is_some()
            .then(|| self.document.body.content.clone());
        let mut failure = None;
        visit_body_paragraphs_mut(&mut self.document.body.content, &mut |paragraph| {
            if failure.is_none() {
                match placeholder::try_replace_regex_in_paragraph(paragraph, re, replacement) {
                    Ok(found) => count += found,
                    Err(error) => failure = Some(error),
                }
            }
        });
        if let Some(error) = failure {
            return Err(error.into());
        }
        if let Some(before) = before {
            self.redline_body(&before)?;
        }

        // Replace in headers and footers
        for (rel_id, is_header) in self.header_footer_rel_ids() {
            let Some(mut hf) = self.load_header_footer(&rel_id, is_header) else {
                continue;
            };
            let before = self.tracking.is_some().then(|| hf.clone());
            let n = placeholder::try_replace_regex_in_header_footer(&mut hf, re, replacement)?;
            if n > 0 {
                if let Some(before) = before {
                    self.redline_header_footer(&before, &mut hf)?;
                }
                self.save_header_footer(&rel_id, &hf, is_header)?;
                count += n;
            }
        }

        // Text boxes and shapes live in raw markup the typed model does not
        // reach. `replace_text` has always covered them; do the same here so
        // the two entry points search the same places.
        self.flush_to_package()?;
        count += self.replace_regex_in_xml_parts(re, replacement)?;

        Ok(count)
    }

    /// Apply a regex replacement to the text-box content of the raw XML parts,
    /// then to the notes of the footnotes and endnotes parts.
    fn replace_regex_in_xml_parts(
        &mut self,
        re: &regex::Regex,
        replacement: &str,
    ) -> Result<usize> {
        let mut count = 0;
        let mut changed = false;

        for (part_name, _) in self.text_bearing_part_names() {
            let Some(xml) = self.package.get_part(&part_name).map(<[u8]>::to_vec) else {
                continue;
            };
            // A part can change without a count, when a replacement reaches
            // only the uncounted copy of a text box that Word writes twice.
            let (new_xml, n) =
                oxoffice_docx_oxml::placeholder::replace_regex_in_xml_part(&xml, re, replacement)?;
            if self.tracking.is_some() && (n > 0 || new_xml != xml) {
                return Err(super::redline::refused("a replacement in a text box"));
            }
            if n > 0 || new_xml != xml {
                self.package.set_part(&part_name, new_xml);
                count += n;
                changed = true;
            }
        }

        // Re-parse so the in-memory model reflects the edited markup; otherwise
        // the next flush would write the pre-replacement document back out.
        if changed && let Some(doc_xml) = self.package.get_part(&self.doc_part_name) {
            self.document =
                crate::document::DocumentModel::unproved(CT_Document::from_xml(doc_xml)?);
        }

        Ok(count
            + self.rewrite_note_parts(|xml| {
                oxoffice_docx_oxml::placeholder::replace_regex_in_notes_part(xml, re, replacement)
            })?)
    }

    /// The main document part plus every header and footer part: everywhere
    /// text boxes and shapes with replaceable text can appear.
    fn text_bearing_part_names(&self) -> Vec<(String, Option<bool>)> {
        let mut names = vec![(self.doc_part_name.clone(), None)];
        for (rel_id, is_header) in self.header_footer_rel_ids() {
            if let Some(part_name) = self.header_footer_part_name(&rel_id, is_header) {
                names.push((part_name, Some(is_header)));
            }
        }
        names
    }

    pub(super) fn raw_text_bearing_part_names(&self) -> Vec<(String, Option<bool>)> {
        let mut names = vec![(self.doc_part_name.clone(), None)];
        for (rel_id, is_header) in self.header_footer_rel_ids() {
            if let Some(part_name) = self.header_footer_part_name(&rel_id, is_header) {
                names.push((part_name, Some(is_header)));
            }
        }
        names
    }

    /// The footnotes and endnotes parts of the main document, each once.
    fn note_part_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        for relationship in self
            .package
            .get_part_rels(&self.doc_part_name)
            .map(|relationships| relationships.items.as_slice())
            .unwrap_or_default()
        {
            if matches!(
                relationship.rel_type.as_str(),
                rel_types::FOOTNOTES | rel_types::ENDNOTES
            ) && relationship_is_internal(relationship)
            {
                let name =
                    OpcPackage::resolve_rel_target(&self.doc_part_name, &relationship.target);
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        names
    }

    /// Apply `rewrite` to every footnotes and endnotes part and keep each
    /// part it changes. The typed footnotes are read again from the new
    /// part, since the next flush writes only what they add.
    fn rewrite_note_parts(
        &mut self,
        mut rewrite: impl FnMut(&[u8]) -> oxoffice_docx_oxml::error::Result<(Vec<u8>, usize)>,
    ) -> Result<usize> {
        let mut count = 0;
        for part_name in self.note_part_names() {
            let Some(original) = self.package.get_part(&part_name) else {
                continue;
            };
            let (mut xml, n) = rewrite(original)?;
            if n == 0 {
                continue;
            }
            if let Some(tracking) = &self.tracking {
                xml = super::redline::redline_part_paragraphs(
                    original,
                    &xml,
                    tracking,
                    &mut self.identifiers,
                )?;
            }
            if self.footnotes_part_name.as_deref() == Some(part_name.as_str()) {
                self.footnotes = oxoffice_docx_oxml::footnotes::CT_Footnotes::from_xml(&xml)?;
            }
            self.package.set_part(&part_name, xml);
            count += n;
        }
        Ok(count)
    }

    pub(super) fn chart_part_names(&self) -> Vec<String> {
        self.package
            .get_part_rels(&self.doc_part_name)
            .map(|relationships| {
                relationships
                    .get_all_by_type(rel_types::CHART)
                    .iter()
                    .filter(|relationship| relationship_is_internal(relationship))
                    .map(|relationship| {
                        OpcPackage::resolve_rel_target(&self.doc_part_name, &relationship.target)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Document {
    /// Run raw XML replacement on the text boxes of the body, header and footer
    /// parts, and on the notes of the footnotes and endnotes parts.
    ///
    /// This is called after the typed-model replacement and flush_to_package.
    fn replace_in_xml_parts(&mut self, pairs: &[(&str, &str)]) -> Result<usize> {
        use oxoffice_docx_oxml::placeholder::replace_many_in_xml_part;

        let mut count = 0;
        let mut changed = false;

        // Collect part names for XML parts to process (text boxes/shapes)
        for (part_name, _) in self.raw_text_bearing_part_names() {
            if let Some(xml) = self.package.get_part(&part_name) {
                let xml = xml.to_vec();
                // A part can change without a count, when a replacement
                // reaches only the uncounted copy of a text box that Word
                // writes twice.
                let (new_xml, n) = replace_many_in_xml_part(&xml, pairs)?;
                if self.tracking.is_some() && (n > 0 || new_xml != xml) {
                    return Err(super::redline::refused("a replacement in a text box"));
                }
                if n > 0 || new_xml != xml {
                    self.package.set_part(&part_name, new_xml);
                    count += n;
                    changed = true;
                }
            }
        }

        // Re-parse document from the (possibly modified) package XML
        if changed && let Some(doc_xml) = self.package.get_part(&self.doc_part_name) {
            self.document =
                crate::document::DocumentModel::unproved(CT_Document::from_xml(doc_xml)?);
        }

        Ok(count
            + self.rewrite_note_parts(|xml| {
                oxoffice_docx_oxml::placeholder::replace_many_in_notes_part(xml, pairs)
            })?)
    }
}

/// One marker per pair of a [`Document::replace_all`] batch: a character of
/// the supplementary private use area between two copies of a delimiter that
/// no placeholder or value holds, so that no marker overlaps another or text
/// the caller wrote.
fn batch_markers(pairs: &[(&str, &str)]) -> Vec<String> {
    let delimiter = (0x10_FF00..=0x10_FFFD)
        .rev()
        .filter_map(char::from_u32)
        .find(|delimiter| {
            pairs.iter().all(|(placeholder, value)| {
                !placeholder.contains(*delimiter) && !value.contains(*delimiter)
            })
        })
        .unwrap_or('\u{10FFFD}');
    // The index in base 0xFF00, one private character per digit, below the
    // delimiters, so a marker never holds its delimiter.
    (0..pairs.len())
        .map(|index| {
            let mut marker = String::from(delimiter);
            let mut rest = index;
            loop {
                marker.extend(char::from_u32(0x10_0000 + (rest % 0xFF00) as u32));
                rest /= 0xFF00;
                if rest == 0 {
                    break;
                }
            }
            marker.push(delimiter);
            marker
        })
        .collect()
}
