//! The main part read in blocks: block scans, frame proofs, scoped normalization, comment paragraph values and the main story layout.

use super::*;

pub(super) struct StandaloneParagraphValue {
    keys: StandaloneParagraphKeys,
    model: std::sync::Weak<BodyContent>,
}

enum StandaloneParagraphKeys {
    Paragraph(Vec<u8>),
    Cells(HashMap<[usize; 3], Vec<u8>>),
}

#[derive(Clone, Copy)]
enum StandaloneParagraphSelector {
    Paragraph,
    Cell([usize; 3]),
}

pub(crate) struct CommentParagraphValue {
    storage: CommentParagraphStorage,
}

// Keep the actual parsed value inline on misses instead of adding a wrapper allocation.
#[allow(clippy::large_enum_variant)]
enum CommentParagraphStorage {
    Owned(CT_P),
    Model {
        root: Arc<BodyContent>,
        selector: StandaloneParagraphSelector,
    },
}

impl std::ops::Deref for CommentParagraphValue {
    type Target = CT_P;

    fn deref(&self) -> &Self::Target {
        match &self.storage {
            CommentParagraphStorage::Owned(paragraph) => paragraph,
            CommentParagraphStorage::Model { root, selector } => selector
                .resolve(root)
                .expect("a paragraph loan retains its validated immutable root and selector"),
        }
    }
}

impl StandaloneParagraphSelector {
    fn resolve(self, model: &BodyContent) -> Option<&CT_P> {
        match (self, model) {
            (Self::Paragraph, BodyContent::Paragraph(paragraph)) => Some(paragraph),
            (Self::Cell([row, cell, content]), BodyContent::Table(table)) => {
                match table.rows.get(row)?.cells.get(cell)?.content.get(content)? {
                    CellContent::Paragraph(paragraph) => Some(paragraph),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

impl StandaloneParagraphKeys {
    fn key(&self, selector: StandaloneParagraphSelector) -> Option<&[u8]> {
        match (self, selector) {
            (Self::Paragraph(xml), StandaloneParagraphSelector::Paragraph) => Some(xml),
            (Self::Cells(keys), StandaloneParagraphSelector::Cell(selector)) => {
                keys.get(&selector).map(Vec::as_slice)
            }
            _ => None,
        }
    }
}

struct CommentCellValueHint {
    table: Option<usize>,
    row: usize,
    cell: usize,
    paragraphs: Vec<usize>,
    next_paragraph: usize,
}

/// Call-local advisory value storage. It never supplies physical affiliation.
pub(crate) struct CommentParagraphRead<'a> {
    main_part: &'a str,
    candidates: Vec<Arc<BodyBlockScan>>,
    next_paragraph: usize,
    tables: Vec<Arc<BodyBlockScan>>,
    cells: Vec<CommentCellValueHint>,
}

impl<'a> CommentParagraphRead<'a> {
    pub(super) fn from_blocks(main_part: &'a str, blocks: &[Arc<BodyBlockScan>]) -> Self {
        let mut candidates = Vec::new();
        let mut tables = Vec::new();
        let mut cells = Vec::new();
        for block in blocks {
            match block.staged.as_deref() {
                Some(BodyContent::Paragraph(_)) => candidates.push(Arc::clone(block)),
                Some(BodyContent::Table(table)) => {
                    let eligible = table.extra_xml.is_empty()
                        && table.content_controls.is_empty()
                        && table.rows.iter().all(|row| {
                            row.extra_xml.is_empty()
                                && row.content_controls.is_empty()
                                && row.cells.iter().all(|cell| {
                                    cell.extra_xml.is_empty()
                                        && cell.properties.as_ref().is_none_or(|properties| {
                                            properties.grid_span.is_none()
                                                && properties.h_merge.is_none()
                                                && properties.v_merge.is_none()
                                        })
                                        && cell.content.iter().all(|content| {
                                            matches!(content, CellContent::Paragraph(_))
                                        })
                                })
                        });
                    let table_index = eligible.then(|| {
                        let index = tables.len();
                        tables.push(Arc::clone(block));
                        index
                    });
                    for (row_index, row) in table.rows.iter().enumerate() {
                        for (cell_index, cell) in row.cells.iter().enumerate() {
                            cells.push(CommentCellValueHint {
                                table: table_index,
                                row: row_index,
                                cell: cell_index,
                                paragraphs: if eligible {
                                    cell.content
                                        .iter()
                                        .enumerate()
                                        .filter_map(|(index, content)| {
                                            matches!(content, CellContent::Paragraph(_))
                                                .then_some(index)
                                        })
                                        .collect()
                                } else {
                                    Vec::new()
                                },
                                next_paragraph: 0,
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        Self {
            main_part,
            candidates,
            next_paragraph: 0,
            tables,
            cells,
        }
    }

    pub(crate) fn read(
        &mut self,
        location: &ContentLocation,
        xml: &[u8],
    ) -> Result<CommentParagraphValue> {
        let direct = location.story.kind == StoryKind::Body
            && location.story.part_name == self.main_part
            && location.story.owner_index == 0
            && location.item_kind == StoryItemKind::Paragraph
            && !location.is_end
            && location.index_path.len() == 1;
        let selected = if direct {
            let candidate = self.candidates.get(self.next_paragraph);
            self.next_paragraph += 1;
            candidate.map(|block| (block, StandaloneParagraphSelector::Paragraph))
        } else if location.story.kind == StoryKind::TableCell
            && location.story.part_name == self.main_part
            && location.item_kind == StoryItemKind::Paragraph
            && !location.is_end
            && location.index_path.len() == 1
        {
            self.cells
                .get_mut(location.story.owner_index)
                .and_then(|hint| {
                    let ordinal = hint.next_paragraph;
                    hint.next_paragraph += 1;
                    let content = *hint.paragraphs.get(ordinal)?;
                    Some((
                        self.tables.get(hint.table?)?,
                        StandaloneParagraphSelector::Cell([hint.row, hint.cell, content]),
                    ))
                })
        } else {
            None
        };
        let candidate = selected.and_then(|(block, selector)| {
            let model = block.staged.as_ref()?;
            Some((block, model, selector, selector.resolve(model)?))
        });
        let hit = candidate.is_some_and(|(block, model, selector, _)| {
            block.standalone_value.get().is_some_and(|cell| {
                let mut state = cell
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let same_root = state.as_ref().is_some_and(|proof| {
                    proof
                        .model
                        .upgrade()
                        .is_some_and(|proved| Arc::ptr_eq(&proved, model))
                });
                if !same_root {
                    *state = None;
                    return false;
                }
                state.as_ref().and_then(|proof| proof.keys.key(selector)) == Some(xml)
            })
        });
        if hit && let Some((_, model, selector, _)) = candidate {
            let value = CommentParagraphValue {
                storage: CommentParagraphStorage::Model {
                    root: Arc::clone(model),
                    selector,
                },
            };
            return Ok(value);
        }
        let parsed = CT_P::from_xml_fragment(xml)?;
        if let Some((block, model, selector, paragraph)) = candidate {
            let mut qualified_properties = None;
            let matches = !holds_field_words(xml)
                && paragraph_has_finite_plain_properties(paragraph, &mut qualified_properties)
                && paragraph_has_finite_plain_properties(&parsed, &mut qualified_properties)
                && paragraph == &parsed;
            if matches {
                let key = xml.to_vec();
                let mut proof = StandaloneParagraphValue {
                    keys: match selector {
                        StandaloneParagraphSelector::Paragraph => {
                            StandaloneParagraphKeys::Paragraph(key)
                        }
                        StandaloneParagraphSelector::Cell(selector) => {
                            StandaloneParagraphKeys::Cells(HashMap::from([(selector, key)]))
                        }
                    },
                    model: Arc::downgrade(model),
                };
                let mut state = block
                    .standalone_value
                    .get_or_init(|| Arc::new(Mutex::new(None)))
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let merged = state
                    .as_mut()
                    .filter(|prior| {
                        prior
                            .model
                            .upgrade()
                            .is_some_and(|root| Arc::ptr_eq(&root, model))
                    })
                    .is_some_and(|prior| match (&mut prior.keys, &mut proof.keys) {
                        (
                            StandaloneParagraphKeys::Paragraph(old),
                            StandaloneParagraphKeys::Paragraph(new),
                        ) => {
                            *old = std::mem::take(new);
                            true
                        }
                        (
                            StandaloneParagraphKeys::Cells(old),
                            StandaloneParagraphKeys::Cells(new),
                        ) => {
                            old.extend(std::mem::take(new));
                            true
                        }
                        _ => false,
                    });
                if !merged {
                    *state = Some(proof);
                }
            } else if let Some(cell) = block.standalone_value.get() {
                let mut state = cell
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let empty = state
                    .as_mut()
                    .filter(|prior| {
                        prior
                            .model
                            .upgrade()
                            .is_some_and(|root| Arc::ptr_eq(&root, model))
                    })
                    .is_some_and(|prior| match (&mut prior.keys, selector) {
                        (
                            StandaloneParagraphKeys::Paragraph(_),
                            StandaloneParagraphSelector::Paragraph,
                        ) => true,
                        (
                            StandaloneParagraphKeys::Cells(keys),
                            StandaloneParagraphSelector::Cell(selector),
                        ) => {
                            keys.remove(&selector);
                            keys.is_empty()
                        }
                        _ => false,
                    });
                if empty {
                    *state = None;
                }
            }
        }
        Ok(CommentParagraphValue {
            storage: CommentParagraphStorage::Owned(parsed),
        })
    }
}

#[derive(Clone, PartialEq)]
pub(super) struct ScopedNormalizationContext {
    source_head: Vec<u8>,
    source_tail: Vec<u8>,
    normal_head: Vec<u8>,
    normal_tail: Vec<u8>,
    writer_frame: (Vec<u8>, Vec<u8>),
}

#[derive(Clone)]
pub(super) struct ScopedChildCorrespondence {
    context: Arc<ScopedNormalizationContext>,
    canonical: Arc<BodyBlockScan>,
    normalized: Vec<u8>,
}

/// The empty-body head and tail produced no nested owners under the old policy.
/// A later child proof may use this context only while its head, tail and
/// root and body scopes are these exact values.
#[derive(Clone)]
pub(super) struct FrameNamespacePolicyEmpty {
    root_declarations: Vec<(String, String)>,
    body_declarations: Vec<(String, String)>,
    body_bindings: Vec<(String, String)>,
    head: Arc<Vec<u8>>,
    tail: Arc<Vec<u8>>,
}

/// One child whose wrapped bytes produced no nested owners. The stored scopes
/// and frame bytes are the context of that success. A different context declines.
#[derive(Clone)]
pub(super) struct EmptyNamespaceChildProof {
    root_declarations: Vec<(String, String)>,
    body_declarations: Vec<(String, String)>,
    body_bindings: Vec<(String, String)>,
    head: Arc<Vec<u8>>,
    tail: Arc<Vec<u8>>,
}

/// Modeled owner spans of one piece. Offsets are relative to that piece.
/// The stored scopes and frame bytes are the context of that success.
#[derive(Clone)]
pub(super) struct NamespaceSpanProof {
    root_declarations: Vec<(String, String)>,
    body_declarations: Vec<(String, String)>,
    body_bindings: Vec<(String, String)>,
    head: Arc<Vec<u8>>,
    tail: Arc<Vec<u8>>,
    spans: Vec<ModeledOwnerSpan>,
}

/// Modeled owner spans of the empty-body head and tail. Head offsets are
/// part offsets. Tail offsets are relative to the tail. A span that crosses
/// the insertion gap is not stored.
#[derive(Clone)]
pub(super) struct NamespaceFrameSpans {
    root_declarations: Vec<(String, String)>,
    body_declarations: Vec<(String, String)>,
    body_bindings: Vec<(String, String)>,
    head: Arc<Vec<u8>>,
    tail: Arc<Vec<u8>>,
    head_spans: Vec<ModeledOwnerSpan>,
    tail_spans: Vec<ModeledOwnerSpan>,
}

#[derive(Clone)]
pub(super) struct IdentifierChildProof {
    root_declarations: Vec<(String, String)>,
    body_declarations: Vec<(String, String)>,
    body_bindings: Vec<(String, String)>,
    head: Arc<Vec<u8>>,
    tail: Arc<Vec<u8>>,
    definitions: Vec<IdentifierDefinition>,
}

#[derive(Clone)]
pub(super) struct FrameIdentifierProof {
    root_declarations: Vec<(String, String)>,
    body_declarations: Vec<(String, String)>,
    body_bindings: Vec<(String, String)>,
    head: Arc<Vec<u8>>,
    tail: Arc<Vec<u8>>,
    definitions: Vec<IdentifierDefinition>,
}

#[derive(Clone)]
pub(super) struct ParsedChild {
    root_declarations: Vec<(String, String)>,
    body_declarations: Vec<(String, String)>,
    body_bindings: Vec<(String, String)>,
    parsed: Option<BodyContent>,
}

impl BodyBlockScan {
    fn identifier_definitions(
        &self,
        frame: &FrameIdentifierProof,
        categories: &HashSet<IdentifierXmlCategory>,
    ) -> Option<Vec<IdentifierDefinition>> {
        let head = frame.head.as_slice();
        let tail = frame.tail.as_slice();
        if let Some(cached) = self.identifier_proof.get() {
            return match cached {
                Some(proof)
                    if proof.root_declarations == frame.root_declarations
                        && proof.body_declarations == frame.body_declarations
                        && proof.body_bindings == frame.body_bindings
                        && proof.head.as_slice() == head
                        && proof.tail.as_slice() == tail =>
                {
                    Some(proof.definitions.clone())
                }
                _ => None,
            };
        }
        let child_start = head.len();
        let child_end = child_start.checked_add(self.bytes.len())?;
        let mut xml = Vec::with_capacity(child_end + tail.len());
        xml.extend_from_slice(head);
        xml.extend_from_slice(&self.bytes);
        xml.extend_from_slice(tail);
        if closed_word_body(&xml, child_start..child_end).is_none() {
            let _ = self.identifier_proof.set(None);
            return None;
        }
        let definitions = scan_identifier_definitions(&xml, categories);
        let stored = definitions
            .as_ref()
            .map(|definitions| IdentifierChildProof {
                root_declarations: frame.root_declarations.clone(),
                body_declarations: frame.body_declarations.clone(),
                body_bindings: frame.body_bindings.clone(),
                head: Arc::clone(&frame.head),
                tail: Arc::clone(&frame.tail),
                definitions: definitions.clone(),
            });
        let _ = self.identifier_proof.set(stored);
        definitions
    }

    fn canonical_child_facts(
        &self,
        head: &[u8],
        tail: &[u8],
        frame: &CanonicalFrameProof,
    ) -> Option<&CanonicalChildFacts> {
        if frame.head.as_slice() != head || frame.tail.as_slice() != tail {
            return None;
        }
        if let Some(cached) = self.canonical_read.get() {
            return match cached {
                Some(facts)
                    if facts.root_declarations == frame.root_declarations
                        && facts.body_declarations == frame.body_declarations
                        && facts.body_bindings == frame.body_bindings
                        && facts.head.as_slice() == head
                        && facts.tail.as_slice() == tail =>
                {
                    Some(facts)
                }
                _ => None,
            };
        }
        let child_start = head.len();
        let Some(child_end) = child_start.checked_add(self.bytes.len()) else {
            let _ = self.canonical_read.set(None);
            return None;
        };
        let mut xml = Vec::with_capacity(child_end + tail.len());
        xml.extend_from_slice(head);
        xml.extend_from_slice(&self.bytes);
        xml.extend_from_slice(tail);
        if closed_word_body(&xml, child_start..child_end).is_none() {
            let _ = self.canonical_read.set(None);
            return None;
        }
        let stored = read_child_canonical_facts(&xml, child_start, child_end, frame);
        let _ = self.canonical_read.set(stored);
        self.canonical_read.get().and_then(Option::as_ref)
    }

    fn items(&self) -> &[StoryItemSpan] {
        self.items
            .as_deref()
            .expect("a layout built to read items holds them")
    }

    /// Empty nested owners for this child under `frame`.
    ///
    /// A child with no namespace declaration uses that byte fact only after
    /// `closed_word_body` covers it and the wrapped root and body scopes are
    /// `frame`. A child that declares a namespace is decided by
    /// `modeled_owner_spans` and `namespace_owners_from_spans` on the wrapped
    /// head, child and tail. Nonempty, failed and unproved results decline.
    fn empty_namespace_owner(&self, frame: &FrameNamespacePolicyEmpty) -> Option<()> {
        if let Some(cached) = self.empty_namespace_owners.get() {
            return cached.as_ref().and_then(|proof| {
                (proof.root_declarations == frame.root_declarations
                    && proof.body_declarations == frame.body_declarations
                    && proof.body_bindings == frame.body_bindings
                    && proof.head.as_slice() == frame.head.as_slice()
                    && proof.tail.as_slice() == frame.tail.as_slice())
                .then_some(())
            });
        }
        let head = frame.head.as_slice();
        let tail = frame.tail.as_slice();
        let child_start = head.len();
        let child_end = child_start.checked_add(self.bytes.len())?;
        let mut xml = Vec::with_capacity(child_end.checked_add(tail.len())?);
        xml.extend_from_slice(head);
        xml.extend_from_slice(&self.bytes);
        xml.extend_from_slice(tail);
        if closed_word_body(&xml, child_start..child_end).is_none() {
            let _ = self.empty_namespace_owners.set(None);
            return None;
        }
        let context_matches = document_namespace_scopes(&xml).ok().is_some_and(|scopes| {
            scopes.root_declarations == frame.root_declarations
                && scopes.body_declarations == frame.body_declarations
                && scopes.body_bindings == frame.body_bindings
        });
        if !context_matches {
            let _ = self.empty_namespace_owners.set(None);
            return None;
        }
        let proved_empty = if self.declares_namespace {
            matches!(
                modeled_owner_spans(&xml)
                    .and_then(|spans| namespace_owners_from_spans(&xml, &spans)),
                Ok(owners) if owners.is_empty()
            )
        } else {
            true
        };
        if !proved_empty {
            let _ = self.empty_namespace_owners.set(None);
            return None;
        }
        let _ = self
            .empty_namespace_owners
            .set(Some(EmptyNamespaceChildProof {
                root_declarations: frame.root_declarations.clone(),
                body_declarations: frame.body_declarations.clone(),
                body_bindings: frame.body_bindings.clone(),
                head: Arc::clone(&frame.head),
                tail: Arc::clone(&frame.tail),
            }));
        Some(())
    }

    /// Modeled owner spans of this child, relative to `bytes`.
    ///
    /// A hit requires the same root, body, head and tail. The caller proves
    /// the current part partition and the closed parent before using a hit.
    /// The fill parses the head, this child and the tail. A word body inside
    /// the child, a scope that does not close, or a parser error stores
    /// `None` and is not an empty span list.
    fn namespace_span_proof(
        &self,
        head: &[u8],
        tail: &[u8],
        scopes: &DocumentNamespaceScopes,
    ) -> Option<&NamespaceSpanProof> {
        if let Some(cached) = self.namespace_spans.get() {
            return cached.as_ref().filter(|proof| {
                proof.root_declarations == scopes.root_declarations
                    && proof.body_declarations == scopes.body_declarations
                    && proof.body_bindings == scopes.body_bindings
                    && proof.head.as_slice() == head
                    && proof.tail.as_slice() == tail
            });
        }
        let child_start = head.len();
        let Some(child_end) = child_start.checked_add(self.bytes.len()) else {
            let _ = self.namespace_spans.set(None);
            return None;
        };
        let Some(wrapped_len) = child_end.checked_add(tail.len()) else {
            let _ = self.namespace_spans.set(None);
            return None;
        };
        let mut xml = Vec::with_capacity(wrapped_len);
        xml.extend_from_slice(head);
        xml.extend_from_slice(&self.bytes);
        xml.extend_from_slice(tail);
        if closed_word_body(&xml, child_start..child_end).is_none() {
            let _ = self.namespace_spans.set(None);
            return None;
        }
        let context_matches = document_namespace_scopes(&xml).ok().is_some_and(|wrapped| {
            wrapped.root_declarations == scopes.root_declarations
                && wrapped.body_declarations == scopes.body_declarations
                && wrapped.body_bindings == scopes.body_bindings
        });
        if !context_matches {
            let _ = self.namespace_spans.set(None);
            return None;
        }
        let relative = match modeled_owner_piece_spans(&xml, Some(child_start..child_end)) {
            Ok(Some(spans)) => {
                let mut relative = Vec::new();
                let mut contained = true;
                for span in spans {
                    if span.end <= child_start || span.start >= child_end {
                        continue;
                    }
                    if span.start < child_start || span.end > child_end {
                        contained = false;
                        break;
                    }
                    relative.push(ModeledOwnerSpan {
                        start: span.start - child_start,
                        end: span.end - child_start,
                        ..span
                    });
                }
                contained.then_some(relative)
            }
            Ok(None) | Err(_) => None,
        };
        let _ = self
            .namespace_spans
            .set(relative.map(|spans| NamespaceSpanProof {
                root_declarations: scopes.root_declarations.clone(),
                body_declarations: scopes.body_declarations.clone(),
                body_bindings: scopes.body_bindings.clone(),
                head: Arc::new(head.to_vec()),
                tail: Arc::new(tail.to_vec()),
                spans,
            }));
        self.namespace_spans.get().and_then(Option::as_ref)
    }

    /// Same child bytes and staged model, with no carried frame facts.
    ///
    /// Copying the bytes is not a child serialization. The new locks are empty,
    /// including after a failed `None`, so this piece can record the new frame.
    pub(super) fn fresh_context_piece(self: &Arc<Self>) -> Arc<Self> {
        self.fresh_context_piece_from(self.bytes.clone())
    }

    fn fresh_context_piece_from(self: &Arc<Self>, bytes: Vec<u8>) -> Arc<Self> {
        Arc::new(Self {
            bytes,
            owners: self.owners.clone(),
            items: self.items.clone(),
            staged: self.staged.clone(),
            standalone_value: self.standalone_value.clone(),
            proved: OnceLock::new(),
            declares_namespace: self.declares_namespace,
            holds_field_words: self.holds_field_words,
            identifier_proof: OnceLock::new(),
            canonical_read: OnceLock::new(),
            empty_namespace_owners: OnceLock::new(),
            namespace_spans: OnceLock::new(),
            decodes: OnceLock::new(),
            scoped_correspondence: OnceLock::new(),
        })
    }
}

impl PreparedMainRead<'_> {
    pub(super) fn sources<'a>(&'a self, related: &'a [StorySource<'_>]) -> Vec<StorySource<'a>> {
        let mut sources = vec![StorySource {
            root_kind: StoryKind::Body,
            part_name: self.document.doc_part_name.clone(),
            xml: Cow::Borrowed(&self.xml),
        }];
        sources.extend(related.iter().map(|source| StorySource {
            root_kind: source.root_kind,
            part_name: source.part_name.clone(),
            xml: Cow::Borrowed(source.xml.as_ref()),
        }));
        sources
    }

    pub(super) fn stories(&self, related: &[StorySource<'_>]) -> Result<Vec<StoryId>> {
        let main = self
            .document
            .main_story_layout(&self.xml, &self.content, &self.children, false)
            .ok();
        Document::stories_in_sources(self.sources(related), main)
    }

    pub(super) fn owner_and_layout<'a>(
        &'a self,
        story: &StoryId,
        items: bool,
        related: &'a [StorySource<'_>],
    ) -> Result<(StorySource<'a>, StoryOwnerSpan, Option<MainStoryLayout>)> {
        self.document.story_source_owner_and_layout_in(
            story,
            items,
            self.sources(related),
            &self.content,
            &self.children,
        )
    }

    pub(super) fn prove_scoped_main_source(
        &self,
        canonical: &[u8],
        canonical_span: Range<usize>,
        original: &[u8],
        retained_span: Range<usize>,
    ) -> Result<()> {
        if self.scoped_main_source_by_blocks(canonical, &canonical_span, original, &retained_span) {
            #[cfg(debug_assertions)]
            self.document
                .prove_whole_scoped_main_source(canonical, canonical_span, original, retained_span)
                .expect("the child proof differs from the whole scoped proof");
            return Ok(());
        }
        self.document.prove_whole_scoped_main_source(
            canonical,
            canonical_span,
            original,
            retained_span,
        )
    }

    fn scoped_main_source_by_blocks(
        &self,
        canonical: &[u8],
        canonical_span: &Range<usize>,
        original: &[u8],
        retained_span: &Range<usize>,
    ) -> bool {
        let Some(held) = self.document.package_main_part_blocks() else {
            return false;
        };
        let Some(serialized) = self.writer.as_ref() else {
            return false;
        };
        let xml = &self.xml;
        if xml.as_slice() != canonical
            || !held.part_partition_matches(original)
            || !serialized.part_partition_matches(canonical)
            || held.blocks.len() != serialized.blocks.len()
        {
            return false;
        }
        let Some(source_frame) = held.frame.get().and_then(Option::as_ref) else {
            return false;
        };
        let Some(normal_frame) = serialized.frame.get().and_then(Option::as_ref) else {
            return false;
        };
        if source_frame.to_xml().ok() != normal_frame.to_xml().ok() {
            return false;
        }
        let reads = |record: &MainPartBlocks| -> bool {
            let Some(root) = record.head.get(record.root.clone()) else {
                return false;
            };
            xml_decodes(&[record.head.as_slice(), record.tail.as_slice()].concat())
                && record.blocks.iter().all(|block| block.decodes(root))
        };
        if !reads(&held) || !reads(serialized) {
            return false;
        }
        let child = |record: &MainPartBlocks, span: &Range<usize>| -> Option<(usize, usize)> {
            let (starts, _) = record.child_starts()?;
            record
                .blocks
                .iter()
                .zip(starts)
                .enumerate()
                .find_map(|(i, (block, at))| holds(at, block, span).then_some((i, at)))
        };
        let closed =
            |record: &MainPartBlocks, span: &Range<usize>, part: &[u8]| -> Option<Vec<u8>> {
                let (index, at) = child(record, span)?;
                let mut wrapped = record.head.clone();
                wrapped.extend_from_slice(&record.blocks.get(index)?.bytes);
                wrapped.extend_from_slice(&record.tail);
                let position = record.head.len().checked_add(span.start.checked_sub(at)?)?;
                let scope = story_namespace_scope_at(&wrapped, position).ok()?;
                close_content_fragment_namespaces(part.get(span.clone())?, &scope).ok()
            };
        let Some(owners) = held.retained_nested_owners(original) else {
            return false;
        };
        let Some(targets) = serialized.proved_namespace_spans(canonical) else {
            return false;
        };
        for owner in owners {
            let Some(original_owner) = closed(&held, &owner.source, original) else {
                return false;
            };
            let mut redundant = 0;
            for target in targets.iter().filter(|target| {
                target.local_name == owner.local_name
                    && owner
                        .declarations
                        .iter()
                        .all(|value| target.declarations.contains(value))
            }) {
                let Some(target_owner) = closed(serialized, &(target.start..target.end), canonical)
                else {
                    return false;
                };
                match comment_namespace_source_boundaries(&original_owner, &target_owner) {
                    Ok(Some((_, changed))) if changed.is_empty() => redundant += 1,
                    Ok(_) => {}
                    Err(_) => return false,
                }
            }
            if redundant != 1 {
                return false;
            }
        }
        let Ok(marker) = scoped_proof_marker(original, canonical) else {
            return false;
        };
        let occurrences = |bytes: &[u8]| {
            bytes
                .windows(marker.len())
                .filter(|value| *value == marker.as_bytes())
                .count()
        };
        let Ok(source_scopes) = document_namespace_scopes(original) else {
            return false;
        };
        let Ok(normal_scopes) = document_namespace_scopes(canonical) else {
            return false;
        };
        let Some(context) =
            scoped_normalization_context(&self.document.document, &held, serialized)
        else {
            return false;
        };
        for (block, normal) in held.blocks.iter().zip(serialized.blocks.iter()) {
            if block.bytes == normal.bytes
                && block.staged.is_some()
                && normal.staged.is_some()
                && !block.reads_anew()
                && !normal.reads_anew()
                && block
                    .read_child(&held.head, &held.tail, &source_scopes)
                    .is_some()
                && normal
                    .read_child(&serialized.head, &serialized.tail, &normal_scopes)
                    .is_some()
            {
                continue;
            }
            if !scoped_children_correspond(
                &self.document.document,
                &held,
                serialized,
                (block, normal),
                &context,
                Some(&marker),
                None,
            ) {
                return false;
            }
        }
        let (Some((index, at)), Some((retained_index, retained_at))) = (
            child(serialized, canonical_span),
            child(&held, retained_span),
        ) else {
            return false;
        };
        if index != retained_index {
            return false;
        }
        let Some(selected) = closed(serialized, canonical_span, canonical) else {
            return false;
        };
        let Ok(sentinel) = scoped_proof_sentinel(&selected, &marker) else {
            return false;
        };
        let probe = |record: &MainPartBlocks,
                     span: &Range<usize>,
                     at: usize|
         -> Option<(BodyContent, Vec<u8>)> {
            let mut bytes = record.blocks.get(index)?.bytes.clone();
            let range = span.start.checked_sub(at)?..span.end.checked_sub(at)?;
            bytes.get(range.clone())?;
            bytes.splice(range, sentinel.bytes());
            normalized_scoped_child(&self.document.document, record, serialized, &bytes)
                .map(|(item, written, _)| (item, written))
        };
        match (
            probe(serialized, canonical_span, at),
            probe(&held, retained_span, retained_at),
        ) {
            (Some((left, a)), Some((right, b))) => {
                left == right && a == b && occurrences(&a) == 1 && occurrences(&b) == 1
            }
            _ => false,
        }
    }
}

impl Clone for MainPartBlocks {
    fn clone(&self) -> Self {
        let frame = std::sync::OnceLock::new();
        if let Some(value) = self.frame.get() {
            let _ = frame.set(value.clone());
        }
        let nested_namespace_owners = std::sync::OnceLock::new();
        if let Some(value) = self.nested_namespace_owners.get() {
            let _ = nested_namespace_owners.set(value.clone());
        }
        let identifier_frame = std::sync::OnceLock::new();
        if let Some(value) = self.identifier_frame.get() {
            let _ = identifier_frame.set(value.clone());
        }
        let canonical_frame = std::sync::OnceLock::new();
        if let Some(value) = self.canonical_frame.get() {
            let _ = canonical_frame.set(value.clone());
        }
        let namespace_frame = std::sync::OnceLock::new();
        if let Some(value) = self.namespace_frame.get() {
            let _ = namespace_frame.set(value.clone());
        }
        let namespace_frame_spans = std::sync::OnceLock::new();
        if let Some(value) = self.namespace_frame_spans.get() {
            let _ = namespace_frame_spans.set(value.clone());
        }
        Self {
            head: self.head.clone(),
            root: self.root.clone(),
            tail: self.tail.clone(),
            blocks: self.blocks.clone(),
            part: self.part.clone(),
            frame,
            nested_namespace_owners,
            identifier_frame,
            canonical_frame,
            namespace_frame,
            namespace_frame_spans,
        }
    }
}

pub(super) fn holds_field_words(xml: &[u8]) -> bool {
    xml.windows(b"fldChar".len())
        .any(|window| window == b"fldChar")
        || xml
            .windows(b"fldSimple".len())
            .any(|window| window == b"fldSimple")
}

impl MainPartBlocks {
    /// This written part with the declaration and root start tag of
    /// `existing` (the main part the package holds) in place of its own,
    /// when they differ only in the order of the root's attributes and in
    /// the whitespace after the declaration: the root a producer wrote stays
    /// as it was (rule 7) when an edit rewrites the body. `None` when this
    /// record does not prove `xml`, the declarations differ, or the roots
    /// differ in a name, an attribute or a value (a namespace the edit
    /// needs that the producer's root lacks keeps the written root).
    pub(super) fn with_producer_root(
        &self,
        xml: &[u8],
        existing: &[u8],
        previous: Option<&MainPartBlocks>,
    ) -> Option<(Vec<u8>, Self)> {
        if self.part.as_deref().map(Vec::as_slice) != Some(xml) || self.head.is_empty() {
            return None;
        }
        let written = self.head.get(..self.root.end)?;
        let kept = root_start_tag(existing)?;
        let decl = |bytes: &[u8], root: &std::ops::Range<usize>| {
            let before = bytes.get(..root.start)?;
            let end = before.windows(2).position(|window| window == b"?>")? + 2;
            before[end..]
                .iter()
                .all(u8::is_ascii_whitespace)
                .then(|| before[..end].to_vec())
        };
        let mut kept_root = kept.clone();
        kept_root.start += existing[kept.clone()]
            .iter()
            .take_while(|byte| byte.is_ascii_whitespace())
            .count();
        if decl(existing, &kept_root)? != decl(written, &self.root)? {
            return None;
        }
        let kept_tag = existing.get(kept_root.clone())?;
        let written_tag = written.get(self.root.clone())?;
        if kept_tag == written_tag {
            return None;
        }
        if root_attributes(kept_tag)? != root_attributes(written_tag)? {
            return None;
        }
        let mut head = Vec::with_capacity(self.head.len() + kept.end);
        head.extend_from_slice(&existing[..kept_root.end]);
        head.extend_from_slice(&self.head[self.root.end..]);
        let body = xml.get(self.head.len()..xml.len().checked_sub(self.tail.len())?)?;
        let mut part = Vec::with_capacity(head.len() + body.len() + self.tail.len());
        part.extend_from_slice(&head);
        part.extend_from_slice(body);
        part.extend_from_slice(&self.tail);
        // The frame reads as the written one, as a producer source record's
        // does: the proofs that start from it hold for these bytes too.
        let shell =
            CT_Document::from_xml(&[head.as_slice(), self.tail.as_slice()].concat()).ok()?;
        // A written frame that does not prove leaves this one unproved too,
        // with the producer's root kept all the same.
        let frame = match self.frame.get() {
            Some(Some(written)) => {
                if !shell.body.content.is_empty()
                    || shell.to_xml().ok()? != written.to_xml().ok()?
                {
                    return None;
                }
                std::sync::OnceLock::from(Some(shell))
            }
            _ => std::sync::OnceLock::new(),
        };
        // A different frame gets new pieces: the pieces' proofs are keyed by
        // the frame they were read under. The pieces of the previous record
        // under this same frame carry over where their bytes did not change
        // (the runs before and after the edit), so their proofs are kept.
        let carried = previous
            .filter(|previous| previous.head == head && previous.tail == self.tail)
            .map(|previous| previous.blocks.as_slice())
            .unwrap_or(&[]);
        let same = |old: &Arc<BodyBlockScan>, new: &Arc<BodyBlockScan>| old.bytes == new.bytes;
        let prefix = carried
            .iter()
            .zip(self.blocks.iter())
            .take_while(|(old, new)| same(old, new))
            .count();
        let room = self.blocks.len().min(carried.len()) - prefix;
        let suffix = carried
            .iter()
            .rev()
            .zip(self.blocks.iter().rev())
            .take(room)
            .take_while(|(old, new)| same(old, new))
            .count();
        let blocks = self
            .blocks
            .iter()
            .enumerate()
            .map(|(index, block)| {
                if index < prefix {
                    Arc::clone(&carried[index])
                } else if index >= self.blocks.len() - suffix {
                    Arc::clone(&carried[carried.len() - (self.blocks.len() - index)])
                } else {
                    block.fresh_context_piece()
                }
            })
            .collect();
        let record = Self {
            head,
            root: kept_root,
            tail: self.tail.clone(),
            blocks: Arc::new(blocks),
            part: Some(Arc::new(part.clone())),
            frame,
            nested_namespace_owners: std::sync::OnceLock::new(),
            identifier_frame: std::sync::OnceLock::new(),
            canonical_frame: std::sync::OnceLock::new(),
            namespace_frame: std::sync::OnceLock::new(),
            namespace_frame_spans: std::sync::OnceLock::new(),
        };
        Some((part, record))
    }
}

/// The name and the sorted attributes of a `w:document` start tag, when
/// `tag` is one.
fn root_attributes(tag: &[u8]) -> Option<Vec<(Vec<u8>, Vec<u8>)>> {
    let inner = tag.strip_prefix(b"<")?.strip_suffix(b">")?;
    if inner.ends_with(b"/") {
        return None;
    }
    let start = quick_xml::events::BytesStart::from_content(
        std::str::from_utf8(inner).ok()?,
        b"w:document".len(),
    );
    if start.name().as_ref() != b"w:document"
        || !inner.starts_with(b"w:document")
        || !inner
            .get(b"w:document".len())
            .is_some_and(u8::is_ascii_whitespace)
    {
        return None;
    }
    let mut attributes = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute.ok()?;
        attributes.push((attribute.key.as_ref().to_vec(), attribute.value.to_vec()));
    }
    attributes.sort();
    Some(attributes)
}

pub(super) fn root_start_tag(head: &[u8]) -> Option<std::ops::Range<usize>> {
    let decl = head.windows(2).position(|window| window == b"?>")?;
    let start = decl + 2;
    let end = head[start..].iter().position(|byte| *byte == b'>')? + start + 1;
    Some(start..end)
}

/// Query bindings have no declaration order. Keep values and multiplicity
/// exact while the stored frame and its proof retain their byte provenance.
fn same_query_namespace_bindings(left: &[(String, String)], right: &[(String, String)]) -> bool {
    if left == right {
        return true;
    }
    if left.len() != right.len() {
        return false;
    }
    let mut left = left.iter().collect::<Vec<_>>();
    let mut right = right.iter().collect::<Vec<_>>();
    left.sort_unstable();
    right.sort_unstable();
    left == right
}

impl MainPartBlocks {
    pub(super) fn body_spans(&self) -> Result<(Range<usize>, Vec<Range<usize>>)> {
        let start = self.head.len();
        let mut end = start;
        let mut children = Vec::with_capacity(self.blocks.len());
        for block in self.blocks.iter() {
            let next = end.checked_add(block.bytes.len()).ok_or_else(|| {
                Error::Other("main-part body span exceeds addressable bytes".to_owned())
            })?;
            children.push(end..next);
            end = next;
        }
        Ok((start..end, children))
    }

    fn child_starts(&self) -> Option<(Vec<usize>, usize)> {
        let mut at = self.head.len();
        let mut starts = Vec::with_capacity(self.blocks.len());
        for block in self.blocks.iter() {
            starts.push(at);
            at = at.checked_add(block.bytes.len())?;
        }
        Some((starts, at))
    }

    // Actual source reads only. Staged writer models never certify a paragraph
    // projection, and this optional inventory proof never fills a read memo.
    // The immutable block carries its read context: assemble and tail edits
    // refresh blocks on a frame change, and source splices require the same
    // head and tail. The exact partition below composes that frame provenance
    // with the stored namespace context before a paragraph is reused.
    pub(super) fn picture_free_block_ranges(
        &self,
        xml: &[u8],
    ) -> Option<Vec<(Range<usize>, StoryItemKind)>> {
        if !self.part_partition_matches(xml) {
            return None;
        }
        let (_, scopes) = self.proved_document_context(xml)?;
        let mut at = self.head.len();
        let mut ranges = Vec::new();
        for block in self.blocks.iter() {
            let end = at.checked_add(block.bytes.len())?;
            if !block.holds_field_words
                && let Some(read) = block.proved.get()
                && read.root_declarations == scopes.root_declarations
                && read.body_declarations == scopes.body_declarations
                && read.body_bindings == scopes.body_bindings
                && read
                    .parsed
                    .as_ref()
                    .is_some_and(BodyContent::owner_facts_reusable_by_value)
            {
                let kind = match read.parsed.as_ref() {
                    Some(BodyContent::Paragraph(paragraph))
                        if plain_picture_free_paragraph(paragraph) =>
                    {
                        Some(StoryItemKind::Paragraph)
                    }
                    Some(BodyContent::Table(table)) if plain_picture_free_table(table) => {
                        Some(StoryItemKind::Table)
                    }
                    _ => None,
                };
                if let Some(kind) = kind {
                    ranges.push((at..end, kind));
                }
            }
            at = end;
        }
        Some(ranges)
    }

    /// A comparison using complete current writer facts. It never grants read
    /// eligibility. An explicit w14 declaration makes whole-part declaration
    /// processing inert, so unqualified frames retain the full writer.
    pub(super) fn writer_bytes_match(
        &self,
        document: &CT_Document,
        expected: &[u8],
    ) -> Option<bool> {
        let (head, tail) = document.body_frame_xml().ok()?;
        if !document
            .extra_namespaces
            .iter()
            .chain(&document.root_attributes)
            .any(|(name, _)| name == "xmlns:w14")
            || head != self.head
            || tail != self.tail
            || document.body.content.len() != self.blocks.len()
        {
            return None;
        }
        for (item, block) in document.body.content.iter().zip(self.blocks.iter()) {
            if !item.owner_facts_reusable_by_value()
                || block.holds_field_words
                || block.staged.as_deref() != Some(item)
            {
                return None;
            }
        }
        let mut cursor = 0usize;
        let mut matched = true;
        for piece in std::iter::once(self.head.as_slice())
            .chain(self.blocks.iter().map(|block| block.bytes.as_slice()))
            .chain(std::iter::once(self.tail.as_slice()))
        {
            let end = cursor.checked_add(piece.len())?;
            matched &= expected.get(cursor..end) == Some(piece);
            cursor = end;
        }
        Some(matched && cursor == expected.len())
    }

    pub(super) fn serialize(
        frame: &CT_Document,
        content: &[BodyContent],
        kept: Option<&MainPartBlocks>,
    ) -> Result<(Vec<u8>, Self)> {
        Self::serialize_in(frame, content, kept, None)
    }

    pub(super) fn serialize_in(
        frame: &CT_Document,
        content: &[BodyContent],
        kept: Option<&MainPartBlocks>,
        mut template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<(Vec<u8>, Self)> {
        let mut previous = template.as_mut().map(|template| {
            (
                std::mem::take(&mut template.sources.items),
                std::mem::take(&mut template.sources.blocks),
            )
        });
        if let Some(template) = &mut template {
            template.sources.items.resize_with(content.len(), || None);
            template.sources.source = None;
        }
        let kept_root = kept
            .map(|kept| kept.head[kept.root.clone()].to_vec())
            .unwrap_or_default();
        let kept_blocks = kept.map(|kept| kept.blocks.as_slice()).unwrap_or(&[]);
        let aligned =
            |item: &BodyContent, block: &BodyBlockScan| block.staged.as_deref() == Some(item);
        let reusable = |item: &BodyContent, block: &BodyBlockScan| {
            item.owner_facts_reusable_by_value() && !block.holds_field_words
        };
        let prefix = content
            .iter()
            .zip(kept_blocks)
            .take_while(|(item, block)| aligned(item, block))
            .count();
        let suffix = content[prefix..]
            .iter()
            .rev()
            .zip(kept_blocks[prefix..].iter().rev())
            .take_while(|(item, block)| aligned(item, block))
            .count();
        let reusable_blocks = kept_blocks[..prefix]
            .iter()
            .map(Some)
            .chain(std::iter::repeat_n(None, content.len() - prefix - suffix))
            .chain(kept_blocks[kept_blocks.len() - suffix..].iter().map(Some));
        let mut blocks = Vec::with_capacity(content.len());
        let mut reused = false;
        let mut all_reused = kept.is_some_and(|record| record.blocks.len() == content.len());
        for (index, (item, block)) in content.iter().zip(reusable_blocks).enumerate() {
            let block = block.filter(|block| reusable(item, block));
            let mut block_source = None;
            if let (Some(block), Some(writer)) = (block, template.as_deref_mut())
                && writer.sources.reader_source.is_none()
            {
                if let Some((items, previous_blocks)) = &mut previous
                    && previous_blocks
                        .get(index)
                        .and_then(std::sync::Weak::upgrade)
                        .is_some_and(|prior| Arc::ptr_eq(&prior, block))
                    && let Some(source) = items.get_mut(index)
                {
                    block_source = source.take();
                }
                if block_source.is_none()
                    && let Some(records) = writer.lineage.records
                {
                    block_source = records.source_for_block(writer.operation, block)?;
                }
            }
            // An ordinary cache proof does not carry this operation's producer
            // facts. Decline it unless the actual cached Source is available.
            if let Some(block) = block.filter(|_| template.is_none() || block_source.is_some()) {
                reused = true;
                if let Some(writer) = template.as_deref_mut() {
                    writer.sources.items[index] = block_source;
                }
                blocks.push(Arc::clone(block));
            } else {
                all_reused = false;
                let xml = if let Some(template) = &mut template
                    && let Some(source) = &template.sources.reader_source
                {
                    let source = template.operation.source_model_body_item_from_read(
                        frame,
                        item,
                        source,
                        index,
                        &[],
                    )?;
                    let copied = template.operation.repeat_source(&source)?;
                    let bytes = template.operation.finish_source(copied)?;
                    template.sources.items[index] = Some(source);
                    bytes
                } else if let Some(template) = &mut template
                    && let Some(inputs) = template
                        .sources
                        .model_lineage_valid
                        .then(|| template.lineage.model_writer_inputs(index, item))
                        .flatten()
                {
                    let source = template.operation.source_model_body_item(
                        frame,
                        item,
                        &[],
                        &inputs.texts,
                        &inputs.fields,
                        &inputs.raw,
                    )?;
                    // The block cache owns a real second byte copy. Reserve
                    // that copy through the existing producer before moving it.
                    let copied = template.operation.repeat_source(&source)?;
                    let xml = template.operation.finish_source(copied)?;
                    template.sources.items[index] = Some(source);
                    xml
                } else {
                    frame.body_item_xml(item)?
                };
                let declares_namespace = bytes_declare_namespace(&xml);
                let holds_field_words = holds_field_words(&xml);
                blocks.push(Arc::new(BodyBlockScan {
                    bytes: xml,
                    owners: Vec::new(),
                    items: None,
                    staged: Some(Arc::new(item.clone())),
                    standalone_value: OnceLock::new(),
                    proved: OnceLock::new(),
                    declares_namespace,
                    holds_field_words,
                    identifier_proof: OnceLock::new(),
                    canonical_read: OnceLock::new(),
                    empty_namespace_owners: OnceLock::new(),
                    namespace_spans: OnceLock::new(),
                    decodes: OnceLock::new(),
                    scoped_correspondence: OnceLock::new(),
                }));
            }
        }
        let kept_frame = kept.map(|kept| (kept.head.as_slice(), kept.tail.as_slice()));
        match Self::assemble_in(
            frame,
            blocks,
            reused.then_some(kept_root.as_slice()),
            kept_frame,
            template.as_deref_mut(),
        )? {
            Some(mut assembled) => {
                // Carry the existing vector only after actual complete reuse.
                // The normal temporary Vec and all writer checks still ran.
                if all_reused
                    && let Some(kept) = kept
                    && assembled.1.head == kept.head
                    && assembled.1.tail == kept.tail
                {
                    assembled.1.blocks = Arc::clone(&kept.blocks);
                }
                if let Some(template) = &mut template {
                    template.sources.blocks =
                        assembled.1.blocks.iter().map(Arc::downgrade).collect();
                }
                Ok(assembled)
            }
            None if reused => Self::serialize_in(frame, content, None, template),
            None => {
                let xml = if let Some(template) = &mut template {
                    let source = template.document_source(frame)?;
                    let copied = template.operation.repeat_source(&source)?;
                    let bytes = template.operation.finish_source(copied)?;
                    template.sources.source = Some(Arc::new(source));
                    template.sources.blocks.clear();
                    bytes
                } else {
                    frame.to_xml()?
                };
                Ok((
                    xml,
                    Self {
                        head: Vec::new(),
                        root: 0..0,
                        tail: Vec::new(),
                        blocks: Arc::new(Vec::new()),
                        part: None,
                        frame: std::sync::OnceLock::new(),
                        nested_namespace_owners: std::sync::OnceLock::new(),
                        identifier_frame: std::sync::OnceLock::new(),
                        canonical_frame: std::sync::OnceLock::new(),
                        namespace_frame: std::sync::OnceLock::new(),
                        namespace_frame_spans: std::sync::OnceLock::new(),
                    },
                ))
            }
        }
    }

    pub(super) fn assemble(
        frame: &CT_Document,
        blocks: Vec<Arc<BodyBlockScan>>,
        kept_root: Option<&[u8]>,
        kept_frame: Option<(&[u8], &[u8])>,
    ) -> Result<Option<(Vec<u8>, Self)>> {
        Self::assemble_in(frame, blocks, kept_root, kept_frame, None)
    }

    fn assemble_in(
        frame: &CT_Document,
        blocks: Vec<Arc<BodyBlockScan>>,
        kept_root: Option<&[u8]>,
        kept_frame: Option<(&[u8], &[u8])>,
        mut template: Option<&mut TemplateMainWriter<'_, '_>>,
    ) -> Result<Option<(Vec<u8>, Self)>> {
        let (head, tail) = frame.body_frame_xml()?;
        let Some(root) = root_start_tag(&head) else {
            return Ok(None);
        };
        let mut xml = if let Some(template) = &mut template {
            let mut pieces = vec![(None, 0..0, head.as_slice())];
            for (index, block) in blocks.iter().enumerate() {
                pieces.push(match template.sources.items[index].as_ref() {
                    Some(source) => (Some(source), 0..source.as_bytes().len(), &[][..]),
                    None => (None, 0..0, block.bytes.as_slice()),
                });
            }
            pieces.push((None, 0..0, tail.as_slice()));
            // Scoped composition is this actual concatenation producer. Its
            // second owned result is the separately reserved legacy record.
            let source = template.operation.compose_sources(&pieces)?;
            let source = template.operation.declare_source_w14_on_root(source)?;
            let copied = template.operation.repeat_source(&source)?;
            let xml = template.operation.finish_source(copied)?;
            template.sources.source = Some(Arc::new(source));
            xml
        } else {
            let mut xml = Vec::with_capacity(
                head.len()
                    + blocks.iter().map(|block| block.bytes.len()).sum::<usize>()
                    + tail.len(),
            );
            xml.extend_from_slice(&head);
            for block in &blocks {
                xml.extend_from_slice(&block.bytes);
            }
            xml.extend_from_slice(&tail);
            xml
        };
        let written =
            head.len() + blocks.iter().map(|block| block.bytes.len()).sum::<usize>() + tail.len();
        if template.is_none() {
            oxoffice_docx_oxml::text::declare_w14_on_part_root(&mut xml)?;
        }
        let declared = xml.len() - written;
        let head = xml[..head.len() + declared].to_vec();
        let root = root.start..root.end + declared;
        if kept_root.is_some_and(|kept| !kept.is_empty() && kept != &head[root.clone()]) {
            return Ok(None);
        }
        let tail = xml[head.len() + blocks.iter().map(|block| block.bytes.len()).sum::<usize>()..]
            .to_vec();
        // Final head includes conditional `w14`. A different kept frame gets new
        // pieces. The old arcs stay on the previous record.
        let blocks = match kept_frame {
            Some((kept_head, kept_tail))
                if kept_head != head.as_slice() || kept_tail != tail.as_slice() =>
            {
                blocks
                    .iter()
                    .enumerate()
                    .map(|(index, block)| {
                        if let Some(template) = &mut template
                            && let Some(source) = template.sources.items[index].as_ref()
                        {
                            let copied = template.operation.repeat_source(source)?;
                            let bytes = template.operation.finish_source(copied)?;
                            Ok(block.fresh_context_piece_from(bytes))
                        } else {
                            Ok(block.fresh_context_piece())
                        }
                    })
                    .collect::<Result<Vec<_>>>()?
            }
            _ => blocks,
        };
        let frame = std::sync::OnceLock::new();
        let _ = frame.set(prove_empty_body_frame(&head, &tail));
        Ok(Some((
            xml,
            Self {
                head,
                root,
                tail,
                blocks: Arc::new(blocks),
                part: None,
                frame,
                nested_namespace_owners: std::sync::OnceLock::new(),
                identifier_frame: std::sync::OnceLock::new(),
                canonical_frame: std::sync::OnceLock::new(),
                namespace_frame: std::sync::OnceLock::new(),
                namespace_frame_spans: std::sync::OnceLock::new(),
            },
        )))
    }

    /// The live main part, when `part` is exactly `doc_xml` and every child
    /// has a positive one-child read under this frame. `None` leaves the
    /// whole parser authoritative.
    fn proved_document_context(
        &self,
        doc_xml: &[u8],
    ) -> Option<(&CT_Document, DocumentNamespaceScopes)> {
        let part = self.part.as_deref()?;
        if part.as_slice() != doc_xml || self.head.is_empty() {
            return None;
        }
        let scopes = document_namespace_scopes(doc_xml).ok()?;
        let shell_xml = [self.head.as_slice(), self.tail.as_slice()].concat();
        let shell_scopes = document_namespace_scopes(&shell_xml).ok()?;
        if shell_scopes.root_declarations != scopes.root_declarations
            || shell_scopes.body_declarations != scopes.body_declarations
            || shell_scopes.body_bindings != scopes.body_bindings
        {
            return None;
        }
        let shell = self.frame.get().and_then(Option::as_ref)?;
        if !shell.body.content.is_empty() {
            return None;
        }
        let mut cursor = self.head.len();
        for block in self.blocks.iter() {
            let end = cursor.checked_add(block.bytes.len())?;
            if doc_xml.get(cursor..end) != Some(block.bytes.as_slice()) {
                return None;
            }
            cursor = end;
        }
        if doc_xml.get(cursor..) != Some(self.tail.as_slice()) {
            return None;
        }
        Some((shell, scopes))
    }

    pub(super) fn proved_document(&self, doc_xml: &[u8]) -> Option<CT_Document> {
        let (shell, scopes) = self.proved_document_context(doc_xml)?;
        let mut content = Vec::with_capacity(self.blocks.len());
        for block in self.blocks.iter() {
            let child = block.read_child(&self.head, &self.tail, &scopes)?;
            content.push(child.into_owned());
        }
        let mut document = shell.clone();
        document.body.content = content;
        self.store_empty_namespace_owners();
        Some(document)
    }

    /// Compare the complete actual parse without copying unchanged child models.
    /// Every child is qualified even after a mismatch, preserving refusal of an
    /// unproved later child. This supplies equality, never writer permission.
    pub(super) fn proved_document_matches(
        &self,
        doc_xml: &[u8],
        document: &CT_Document,
    ) -> Option<bool> {
        let (shell, scopes) = self.proved_document_context(doc_xml)?;
        let CT_Document {
            body: oxoffice_docx_oxml::document::CT_Body { content, sect_pr },
            extra_namespaces,
            background_xml,
            background_extra_xml,
            root_attributes,
        } = document;
        let mut matches = &shell.body.sect_pr == sect_pr
            && &shell.extra_namespaces == extra_namespaces
            && &shell.background_xml == background_xml
            && &shell.background_extra_xml == background_extra_xml
            && &shell.root_attributes == root_attributes
            && content.len() == self.blocks.len();
        for (index, block) in self.blocks.iter().enumerate() {
            let child = block.read_child(&self.head, &self.tail, &scopes)?;
            matches &= content.get(index) == Some(child.as_ref());
        }
        self.store_empty_namespace_owners();
        Some(matches)
    }

    /// Empty nested owners for these exact part bytes, when the frame policy
    /// and every child proof are empty. A missing proof is unavailable.
    pub(super) fn retained_nested_owners(
        &self,
        doc_xml: &[u8],
    ) -> Option<Vec<NestedNamespaceOwner>> {
        let part = self.part.as_deref()?;
        if part.as_slice() != doc_xml {
            return None;
        }
        self.store_empty_namespace_owners();
        if self.nested_namespace_owners.get().is_none() {
            self.store_proved_namespace_owners(doc_xml);
        }
        self.nested_namespace_owners
            .get()
            .and_then(|owners| owners.clone())
    }

    /// Main-part identifier definitions when the frame contributes none and
    /// every child proof matches this scope. `None` keeps the whole-part scan.
    pub(super) fn identifier_hint(
        &self,
        doc_xml: &[u8],
        scopes: &DocumentNamespaceScopes,
    ) -> Option<ScannedIdentifierPart> {
        let part = self.part.as_ref()?;
        if part.as_slice() != doc_xml || self.head.is_empty() {
            return None;
        }
        let mut cursor = self.head.len();
        for block in self.blocks.iter() {
            let end = cursor.checked_add(block.bytes.len())?;
            if doc_xml.get(cursor..end) != Some(block.bytes.as_slice()) {
                return None;
            }
            cursor = end;
        }
        if doc_xml.get(cursor..) != Some(self.tail.as_slice()) {
            return None;
        }
        let shell = [self.head.as_slice(), self.tail.as_slice()].concat();
        let frame = self.frame_identifier_proof(&shell)?;
        if frame.root_declarations != scopes.root_declarations
            || frame.body_declarations != scopes.body_declarations
            || frame.body_bindings != scopes.body_bindings
            || frame.head.as_slice() != self.head.as_slice()
            || frame.tail.as_slice() != self.tail.as_slice()
            || !frame.definitions.is_empty()
        {
            return None;
        }
        let mut categories = HashSet::new();
        categories.insert(IdentifierXmlCategory::Bookmark);
        categories.insert(IdentifierXmlCategory::Drawing);
        let mut definitions = Vec::new();
        for block in self.blocks.iter() {
            definitions.extend(block.identifier_definitions(frame, &categories)?);
        }
        Some(ScannedIdentifierPart {
            bytes: Arc::clone(part),
            categories,
            definitions: Some(definitions),
        })
    }

    fn frame_identifier_proof(&self, shell: &[u8]) -> Option<&FrameIdentifierProof> {
        self.identifier_frame
            .get_or_init(|| {
                if self.head.is_empty() || shell.len() != self.head.len() + self.tail.len() {
                    return None;
                }
                let gap = self.head.len()..self.head.len();
                closed_word_body(shell, gap)?;
                let scopes = document_namespace_scopes(shell).ok()?;
                let mut categories = HashSet::new();
                categories.insert(IdentifierXmlCategory::Bookmark);
                categories.insert(IdentifierXmlCategory::Drawing);
                let definitions = scan_identifier_definitions(shell, &categories)?;
                Some(FrameIdentifierProof {
                    root_declarations: scopes.root_declarations,
                    body_declarations: scopes.body_declarations,
                    body_bindings: scopes.body_bindings,
                    head: Arc::new(self.head.clone()),
                    tail: Arc::new(self.tail.clone()),
                    definitions,
                })
            })
            .as_ref()
    }

    fn canonical_child_facts<'a>(
        &'a self,
        xml: &[u8],
        bindings: &[(String, String)],
    ) -> Option<Vec<&'a CanonicalChildFacts>> {
        if self.head.is_empty()
            || (!bindings.is_empty() && {
                let shell = [self.head.as_slice(), self.tail.as_slice()].concat();
                self.canonical_frame_proof(&shell).is_none_or(|frame| {
                    !same_query_namespace_bindings(bindings, &frame.body_bindings)
                })
            })
        {
            return None;
        }
        let mut cursor = self.head.len();
        for block in self.blocks.iter() {
            let end = cursor.checked_add(block.bytes.len())?;
            if xml.get(cursor..end) != Some(block.bytes.as_slice()) {
                return None;
            }
            cursor = end;
        }
        if xml.get(cursor..) != Some(self.tail.as_slice()) {
            return None;
        }
        let shell = [self.head.as_slice(), self.tail.as_slice()].concat();
        let frame = self.canonical_frame_proof(&shell)?;
        if !bindings.is_empty() && !same_query_namespace_bindings(bindings, &frame.body_bindings) {
            return None;
        }
        let mut facts = Vec::with_capacity(self.blocks.len());
        for block in self.blocks.iter() {
            facts.push(block.canonical_child_facts(&self.head, &self.tail, frame)?);
        }
        Some(facts)
    }

    pub(super) fn proved_relationship_ids(
        &self,
        xml: &[u8],
        bindings: &[(String, String)],
    ) -> Option<Vec<String>> {
        let facts = self.canonical_child_facts(xml, bindings)?;
        let shell = [self.head.as_slice(), self.tail.as_slice()].concat();
        let frame = self.canonical_frame_proof(&shell)?;
        let (head_ids, tail_ids) = if bindings.is_empty() {
            (
                frame.head_plain_relationship_ids.as_slice(),
                frame.tail_plain_relationship_ids.as_slice(),
            )
        } else {
            (
                frame.head_bound_relationship_ids.as_slice(),
                frame.tail_bound_relationship_ids.as_slice(),
            )
        };
        let mut ids = Vec::new();
        let mut seen = HashSet::new();
        let mut push = |id: &String| {
            let owned = id.clone();
            if seen.insert(owned.clone()) {
                ids.push(owned);
            }
        };
        for id in head_ids {
            push(id);
        }
        for child in facts {
            let child_ids = if bindings.is_empty() {
                &child.plain_relationship_ids
            } else {
                &child.bound_relationship_ids
            };
            for id in child_ids {
                push(id);
            }
        }
        for id in tail_ids {
            push(id);
        }
        Some(ids)
    }

    fn canonical_frame_proof(&self, shell: &[u8]) -> Option<&CanonicalFrameProof> {
        self.canonical_frame
            .get_or_init(|| {
                if self.head.is_empty()
                    || shell.len() != self.head.len() + self.tail.len()
                    || shell.get(..self.head.len()) != Some(self.head.as_slice())
                    || shell.get(self.head.len()..) != Some(self.tail.as_slice())
                {
                    return None;
                }
                let gap = self.head.len()..self.head.len();
                closed_word_body(shell, gap)?;
                let scopes = document_namespace_scopes(shell).ok()?;
                let bindings = scopes.body_bindings.as_slice();
                let shell_bound =
                    xml_relationship_ids_in_order_with_bindings(shell, bindings).ok()?;
                if !frame_drawings_are_absent(shell) {
                    return None;
                }
                let head_bound =
                    xml_relationship_ids_in_order_with_bindings(&self.head, bindings).ok()?;
                if shell_bound.get(..head_bound.len()) != Some(head_bound.as_slice()) {
                    return None;
                }
                let tail_bound = shell_bound[head_bound.len()..].to_vec();
                let (head_plain, tail_plain) = if bindings.is_empty() {
                    (head_bound.clone(), tail_bound.clone())
                } else {
                    let (head_plain, tail_plain) =
                        split_relationship_ids_at_head(shell, self.head.len(), &[])?;
                    (head_plain, tail_plain)
                };
                Some(CanonicalFrameProof {
                    root_declarations: scopes.root_declarations,
                    body_declarations: scopes.body_declarations,
                    body_bindings: scopes.body_bindings,
                    head: Arc::new(self.head.clone()),
                    tail: Arc::new(self.tail.clone()),
                    head_bound_relationship_ids: head_bound,
                    tail_bound_relationship_ids: tail_bound,
                    head_plain_relationship_ids: head_plain,
                    tail_plain_relationship_ids: tail_plain,
                })
            })
            .as_ref()
    }

    /// Empty-owner policy for this head and tail. The owner walk is the old
    /// `modeled_owner_spans` and `namespace_owners_from_spans` pair. One Word
    /// body must cover the empty insertion gap. A nonempty or failed result
    /// stays `None` and does not publish an empty owner list.
    fn frame_namespace_policy(&self, shell: &[u8]) -> Option<&FrameNamespacePolicyEmpty> {
        self.namespace_frame
            .get_or_init(|| {
                if self.head.is_empty() || shell.len() != self.head.len() + self.tail.len() {
                    return None;
                }
                let gap = self.head.len()..self.head.len();
                closed_word_body(shell, gap)?;
                let scopes = document_namespace_scopes(shell).ok()?;
                let spans = modeled_owner_spans(shell).ok()?;
                let owners = namespace_owners_from_spans(shell, &spans).ok()?;
                if !owners.is_empty() {
                    return None;
                }
                Some(FrameNamespacePolicyEmpty {
                    root_declarations: scopes.root_declarations,
                    body_declarations: scopes.body_declarations,
                    body_bindings: scopes.body_bindings,
                    head: Arc::new(self.head.clone()),
                    tail: Arc::new(self.tail.clone()),
                })
            })
            .as_ref()
    }

    fn store_empty_namespace_owners(&self) {
        if self.nested_namespace_owners.get().is_some() {
            return;
        }
        let Some(shell) = self.frame.get().and_then(Option::as_ref) else {
            return;
        };
        if !shell.body.content.is_empty() {
            return;
        }
        let shell_xml = [self.head.as_slice(), self.tail.as_slice()].concat();
        let Some(policy) = self.frame_namespace_policy(&shell_xml).cloned() else {
            return;
        };
        if policy.head.as_slice() != self.head.as_slice()
            || policy.tail.as_slice() != self.tail.as_slice()
        {
            return;
        }
        for block in self.blocks.iter() {
            if block.empty_namespace_owner(&policy).is_none() {
                return;
            }
        }
        let _ = self
            .nested_namespace_owners
            .set(Some(Vec::<NestedNamespaceOwner>::new()));
    }

    fn namespace_context_matches(
        proof_root: &[(String, String)],
        proof_body: &[(String, String)],
        proof_bindings: &[(String, String)],
        proof_head: &[u8],
        proof_tail: &[u8],
        scopes: &DocumentNamespaceScopes,
        head: &[u8],
        tail: &[u8],
    ) -> bool {
        proof_root == scopes.root_declarations.as_slice()
            && proof_body == scopes.body_declarations.as_slice()
            && proof_bindings == scopes.body_bindings.as_slice()
            && proof_head == head
            && proof_tail == tail
    }

    /// Frame spans for this head and tail. Scopes come from that shell.
    /// Head spans keep part offsets. Tail spans are relative to the tail.
    /// A background or section-tail owner stays in this split. The empty-body
    /// gap is a closed-body certificate. A span across that gap declines.
    fn frame_namespace_spans(&self) -> Option<&NamespaceFrameSpans> {
        if let Some(cached) = self.namespace_frame_spans.get() {
            return cached.as_ref().filter(|proof| {
                proof.head.as_slice() == self.head.as_slice()
                    && proof.tail.as_slice() == self.tail.as_slice()
            });
        }
        if self.head.is_empty() {
            let _ = self.namespace_frame_spans.set(None);
            return None;
        }
        let shell = [self.head.as_slice(), self.tail.as_slice()].concat();
        let gap = self.head.len()..self.head.len();
        if closed_word_body(&shell, gap).is_none() {
            let _ = self.namespace_frame_spans.set(None);
            return None;
        }
        let Some(scopes) = document_namespace_scopes(&shell).ok() else {
            let _ = self.namespace_frame_spans.set(None);
            return None;
        };
        let split = match modeled_owner_spans(&shell) {
            Ok(spans) => {
                let mut head_spans = Vec::new();
                let mut tail_spans = Vec::new();
                let mut contained = true;
                for span in spans {
                    if span.end <= self.head.len() {
                        head_spans.push(span);
                    } else if span.start >= self.head.len() {
                        let shift = self.head.len();
                        tail_spans.push(ModeledOwnerSpan {
                            start: span.start - shift,
                            end: span.end - shift,
                            ..span
                        });
                    } else {
                        contained = false;
                        break;
                    }
                }
                contained.then_some((head_spans, tail_spans))
            }
            Err(_) => None,
        };
        let _ = self
            .namespace_frame_spans
            .set(split.map(|(head_spans, tail_spans)| NamespaceFrameSpans {
                root_declarations: scopes.root_declarations,
                body_declarations: scopes.body_declarations,
                body_bindings: scopes.body_bindings,
                head: Arc::new(self.head.clone()),
                tail: Arc::new(self.tail.clone()),
                head_spans,
                tail_spans,
            }));
        self.namespace_frame_spans.get().and_then(Option::as_ref)
    }

    pub(super) fn part_partition_matches(&self, xml: &[u8]) -> bool {
        let part = self.part.as_deref();
        if part.is_none_or(|part| part.as_slice() != xml) || self.head.is_empty() {
            return false;
        }
        let mut cursor = self.head.len();
        if xml.get(..cursor) != Some(self.head.as_slice()) {
            return false;
        }
        for block in self.blocks.iter() {
            let Some(end) = cursor.checked_add(block.bytes.len()) else {
                return false;
            };
            if xml.get(cursor..end) != Some(block.bytes.as_slice()) {
                return false;
            }
            cursor = end;
        }
        xml.get(cursor..) == Some(self.tail.as_slice())
    }

    /// Every modeled owner span of `xml`, in physical order.
    ///
    /// The exact part partition, the cached frame-gap certificate and each
    /// cached closed-child certificate compose the parent proof. Frame scopes
    /// come from the head and tail shell. A child hit still requires that
    /// frame context. Child spans are rebased onto the part. Frame head
    /// spans, including a background owner, stay first. Frame tail spans,
    /// including a section-tail owner, stay last. One ineligible piece
    /// declines the whole list.
    pub(super) fn proved_namespace_spans(&self, xml: &[u8]) -> Option<Vec<ModeledOwnerSpan>> {
        if !self.part_partition_matches(xml) {
            return None;
        }
        let content_end = xml.len().checked_sub(self.tail.len())?;
        if content_end < self.head.len() {
            return None;
        }
        let frame = self.frame_namespace_spans()?;
        let scopes = DocumentNamespaceScopes {
            root_declarations: frame.root_declarations.clone(),
            body_declarations: frame.body_declarations.clone(),
            body_bindings: frame.body_bindings.clone(),
        };
        let mut spans = frame.head_spans.clone();
        let mut cursor = self.head.len();
        for block in self.blocks.iter() {
            let proof = block.namespace_span_proof(&self.head, &self.tail, &scopes)?;
            if !Self::namespace_context_matches(
                &proof.root_declarations,
                &proof.body_declarations,
                &proof.body_bindings,
                proof.head.as_slice(),
                proof.tail.as_slice(),
                &scopes,
                &self.head,
                &self.tail,
            ) {
                return None;
            }
            for span in &proof.spans {
                let start = cursor.checked_add(span.start)?;
                let end = cursor.checked_add(span.end)?;
                if start > end || end > content_end {
                    return None;
                }
                spans.push(ModeledOwnerSpan {
                    start,
                    end,
                    ..span.clone()
                });
            }
            cursor = cursor.checked_add(block.bytes.len())?;
        }
        if cursor != content_end {
            return None;
        }
        for span in &frame.tail_spans {
            let start = cursor.checked_add(span.start)?;
            let end = cursor.checked_add(span.end)?;
            if start > end || end > xml.len() {
                return None;
            }
            spans.push(ModeledOwnerSpan {
                start,
                end,
                ..span.clone()
            });
        }
        #[cfg(debug_assertions)]
        {
            let scanned = modeled_owner_spans(xml).ok();
            assert_eq!(
                format!("{:?}", Some(&spans)),
                format!("{scanned:?}"),
                "proved namespace spans differ from the old parser"
            );
        }
        Some(spans)
    }

    /// A masked query may change the tail without changing an earlier closed
    /// child's namespace environment. Both partitions and both empty frames
    /// are checked here. This does not transport ASTs or writer eligibility.
    fn masked_query_frames<'a>(
        &'a self,
        xml: &[u8],
        original: &'a Self,
    ) -> Option<(&'a NamespaceFrameSpans, &'a NamespaceFrameSpans)> {
        if self.head != original.head
            || self.blocks.len() != original.blocks.len()
            || !self.part_partition_matches(xml)
            || !original.part_partition_matches(original.part.as_deref()?)
            || !self.frame.get()?.as_ref()?.body.content.is_empty()
            || !original.frame.get()?.as_ref()?.body.content.is_empty()
        {
            return None;
        }
        for block in self.blocks.iter().chain(original.blocks.iter()) {
            let item = block.staged.as_deref()?;
            if matches!(item, BodyContent::RawXml(_))
                || !item.owner_facts_reusable_by_value()
                || block.holds_field_words
            {
                return None;
            }
        }
        let frame = self.frame_namespace_spans()?;
        let previous = original.frame_namespace_spans()?;
        (frame.root_declarations == previous.root_declarations
            && frame.body_declarations == previous.body_declarations
            && frame.body_bindings == previous.body_bindings)
            .then_some((frame, previous))
    }

    /// Compose a fresh query over the masked bytes from actual old relative
    /// child spans and freshly checked changed children and frame facts.
    pub(super) fn masked_query_namespace_spans(
        &self,
        xml: &[u8],
        original: &Self,
    ) -> Option<Vec<ModeledOwnerSpan>> {
        let (frame, _) = self.masked_query_frames(xml, original)?;
        let scopes = DocumentNamespaceScopes {
            root_declarations: frame.root_declarations.clone(),
            body_declarations: frame.body_declarations.clone(),
            body_bindings: frame.body_bindings.clone(),
        };
        let mut spans = frame.head_spans.clone();
        let mut cursor = self.head.len();
        for (block, previous) in self.blocks.iter().zip(original.blocks.iter()) {
            let proof = if block.bytes == previous.bytes {
                previous.namespace_span_proof(&original.head, &original.tail, &scopes)?
            } else {
                block.namespace_span_proof(&self.head, &self.tail, &scopes)?
            };
            for span in &proof.spans {
                if span.start > span.end || span.end > block.bytes.len() {
                    return None;
                }
                spans.push(ModeledOwnerSpan {
                    start: cursor.checked_add(span.start)?,
                    end: cursor.checked_add(span.end)?,
                    ..span.clone()
                });
            }
            cursor = cursor.checked_add(block.bytes.len())?;
        }
        for span in &frame.tail_spans {
            if span.start > span.end || span.end > self.tail.len() {
                return None;
            }
            spans.push(ModeledOwnerSpan {
                start: cursor.checked_add(span.start)?,
                end: cursor.checked_add(span.end)?,
                ..span.clone()
            });
        }
        #[cfg(debug_assertions)]
        {
            assert_eq!(
                format!("{spans:?}"),
                format!(
                    "{:?}",
                    modeled_owner_spans(xml).expect("masked query parses")
                ),
                "masked namespace query changed physical spans"
            );
        }
        Some(spans)
    }

    /// Relationship queries use the same tail-only transport certificate.
    /// The complete current occurrence set still chooses masking sentinels.
    pub(super) fn masked_query_relationship_ids(
        &self,
        xml: &[u8],
        original: &Self,
    ) -> Option<Vec<String>> {
        self.masked_query_frames(xml, original)?;
        let shell = [self.head.as_slice(), self.tail.as_slice()].concat();
        let old_shell = [original.head.as_slice(), original.tail.as_slice()].concat();
        let frame = self.canonical_frame_proof(&shell)?;
        let previous_frame = original.canonical_frame_proof(&old_shell)?;
        if frame.root_declarations != previous_frame.root_declarations
            || frame.body_declarations != previous_frame.body_declarations
            || frame.body_bindings != previous_frame.body_bindings
        {
            return None;
        }
        let mut ids = Vec::new();
        let mut seen = HashSet::new();
        let mut push = |id: &String| {
            if seen.insert(id.clone()) {
                ids.push(id.clone());
            }
        };
        for id in &frame.head_plain_relationship_ids {
            push(id);
        }
        for (block, previous) in self.blocks.iter().zip(original.blocks.iter()) {
            let proof = if block.bytes == previous.bytes {
                previous.canonical_child_facts(&original.head, &original.tail, previous_frame)?
            } else {
                block.canonical_child_facts(&self.head, &self.tail, frame)?
            };
            for id in &proof.plain_relationship_ids {
                push(id);
            }
        }
        for id in &frame.tail_plain_relationship_ids {
            push(id);
        }
        #[cfg(debug_assertions)]
        {
            assert_eq!(
                xml_relationship_ids_in_order_with_bindings(xml, &[]).ok(),
                Some(ids.clone()),
                "masked relationship query changed occurrence order"
            );
        }
        Some(ids)
    }

    /// Positive nested owners for these exact part bytes.
    ///
    /// Spans from every piece are combined first. Owner ambiguity is the old
    /// `namespace_owners_from_spans` policy over that full list. A failed or
    /// unproved list leaves the cache unset. It does not store an empty owner
    /// vector. The empty proof remains the only writer of an empty success.
    fn store_proved_namespace_owners(&self, xml: &[u8]) {
        if self.nested_namespace_owners.get().is_some() {
            return;
        }
        let Some(spans) = self.proved_namespace_spans(xml) else {
            return;
        };
        if let Ok(owners) = namespace_owners_from_spans(xml, &spans) {
            let _ = self.nested_namespace_owners.set(Some(owners));
        }
    }
}

fn bytes_declare_namespace(bytes: &[u8]) -> bool {
    bytes
        .windows(b"xmlns".len())
        .any(|window| window == b"xmlns")
}

pub(super) fn scoped_normalization_context(
    document: &CT_Document,
    source: &MainPartBlocks,
    normal: &MainPartBlocks,
) -> Option<Arc<ScopedNormalizationContext>> {
    Some(Arc::new(ScopedNormalizationContext {
        source_head: source.head.clone(),
        source_tail: source.tail.clone(),
        normal_head: normal.head.clone(),
        normal_tail: normal.tail.clone(),
        writer_frame: document.body_frame_xml().ok()?,
    }))
}

// Parsing fields allocates private source IDs. Neither shortcut may elide it.
pub(super) fn scoped_normalization_reuse_allowed(
    source: &MainPartBlocks,
    normal: &MainPartBlocks,
    left: &BodyBlockScan,
    right: &BodyBlockScan,
    item: &BodyContent,
) -> bool {
    item.owner_facts_reusable_by_value()
        && !left.holds_field_words
        && !right.holds_field_words
        && [&source.head, &source.tail, &normal.head, &normal.tail]
            .into_iter()
            .all(|bytes| !holds_field_words(bytes))
}

/// Only the freshly constructed producer record may offer this actual read.
/// Its head/tail have not changed since read_source_child, and the complete
/// record received complete borrowed source qualification before this call.
pub(super) fn producer_scoped_normalization_seed<'a>(
    source: &MainPartBlocks,
    normal: &MainPartBlocks,
    left: &'a BodyBlockScan,
    right: &BodyBlockScan,
    scopes: &DocumentNamespaceScopes,
) -> Option<&'a BodyContent> {
    let read = left.proved.get()?;
    if read.root_declarations != scopes.root_declarations
        || read.body_declarations != scopes.body_declarations
        || read.body_bindings != scopes.body_bindings
    {
        return None;
    }
    let item = read.parsed.as_ref()?;
    scoped_normalization_reuse_allowed(source, normal, left, right, item).then_some(item)
}

/// A successful normalization belongs to these exact source bytes, writer
/// bytes and frames. It never supplies writer eligibility or a source AST.
pub(super) fn scoped_children_correspond(
    document: &CT_Document,
    source: &MainPartBlocks,
    normal: &MainPartBlocks,
    children: (&Arc<BodyBlockScan>, &Arc<BodyBlockScan>),
    context: &Arc<ScopedNormalizationContext>,
    marker: Option<&str>,
    first_read: Option<&BodyContent>,
) -> bool {
    let (left, right) = children;
    let marker_absent = |bytes: &[u8]| {
        marker.is_none_or(|marker| {
            !bytes
                .windows(marker.len())
                .any(|part| part == marker.as_bytes())
        })
    };
    if let Some(proof) = left.scoped_correspondence.get()
        && proof.context.as_ref() == context.as_ref()
        && proof.canonical.bytes == right.bytes
    {
        return marker_absent(&proof.normalized);
    }
    // Only the fresh producer builder can supply an actual source read.
    // Recheck private/field safety before consuming it. Generic callers read.
    let left_result = match first_read
        .filter(|item| scoped_normalization_reuse_allowed(source, normal, left, right, item))
    {
        Some(item) => normalize_scoped_parsed_child(document, normal, item),
        None => normalized_scoped_child(document, source, normal, &left.bytes),
    };
    // The already executed normal-frame read of C1 produced a and C2. At this
    // checked fixed point both right rounds repeat that exact read/write with
    // the same immutable frame and writer. Fields keep their original rounds.
    let fixed = left_result.as_ref().is_some_and(|(item, written, first)| {
        scoped_normalization_reuse_allowed(source, normal, left, right, item)
            && first == &right.bytes
            && written == &right.bytes
    });
    // C1 is not retained across fallback. A failed left still executes the full
    // right side, as the old eager tuple did, including private field parsing.
    let left_result = left_result.map(|(item, written, first)| {
        drop(first);
        (item, written)
    });
    let right_result = if fixed {
        None
    } else {
        normalized_scoped_child(document, normal, normal, &right.bytes)
            .map(|(item, written, _)| (item, written))
    };
    let Some((a, written)) = left_result else {
        return false;
    };
    if !fixed {
        let Some((b, again)) = right_result else {
            return false;
        };
        if a != b || written != again {
            return false;
        }
    }
    if !marker_absent(&written) {
        return false;
    }
    if a.owner_facts_reusable_by_value()
        && !left.holds_field_words
        && !right.holds_field_words
        && right.scoped_correspondence.get().is_none()
    {
        let _ = left.scoped_correspondence.set(ScopedChildCorrespondence {
            context: Arc::clone(context),
            canonical: Arc::clone(right),
            normalized: written,
        });
    }
    true
}

/// One actual closed child read, including both authoritative shell writers.
fn read_scoped_normalization_child(record: &MainPartBlocks, bytes: &[u8]) -> Option<BodyContent> {
    let mut xml = record.head.clone();
    let end = xml.len().checked_add(bytes.len())?;
    xml.extend_from_slice(bytes);
    xml.extend_from_slice(&record.tail);
    closed_word_body(&xml, record.head.len()..end)?;
    let mut parsed = CT_Document::from_xml(&xml).ok()?;
    if parsed.body.content.len() != 1 {
        return None;
    }
    let item = parsed.body.content.pop()?;
    let actual = parsed.to_xml().ok()?;
    let frame = record.frame.get()?.as_ref()?;
    let expected = frame.to_xml().ok()?;
    if actual != expected {
        return None;
    }
    Some(item)
}

/// Normalize an actual source read through the same borrowed writer and frame.
fn normalize_scoped_parsed_child(
    document: &CT_Document,
    normal: &MainPartBlocks,
    item: &BodyContent,
) -> Option<(BodyContent, Vec<u8>, Vec<u8>)> {
    let written = document.body_item_xml(item).ok()?;
    let root = normal.head.get(normal.root.clone())?;
    if !root.windows(10).any(|bytes| bytes == b"xmlns:w14=")
        && written.windows(5).any(|bytes| {
            matches!(bytes[0], b'<' | b'/' | b' ' | b'\t' | b'\r' | b'\n') && &bytes[1..] == b"w14:"
        })
    {
        return None;
    }
    let item = read_scoped_normalization_child(normal, &written)?;
    let rewritten = document.body_item_xml(&item).ok()?;
    Some((item, rewritten, written))
}

/// Normalize one actual source child twice under exact source and writer frames.
fn normalized_scoped_child(
    document: &CT_Document,
    source: &MainPartBlocks,
    normal: &MainPartBlocks,
    bytes: &[u8],
) -> Option<(BodyContent, Vec<u8>, Vec<u8>)> {
    let item = read_scoped_normalization_child(source, bytes)?;
    normalize_scoped_parsed_child(document, normal, &item)
}

fn prove_empty_body_frame(head: &[u8], tail: &[u8]) -> Option<CT_Document> {
    if head.is_empty() {
        return None;
    }
    let shell = [head, tail].concat();
    let parsed = CT_Document::from_xml(&shell).ok()?;
    if !parsed.body.content.is_empty() {
        return None;
    }
    (parsed.to_xml().ok().as_deref() == Some(shell.as_slice())).then_some(parsed)
}

pub(super) fn parse_one_child(head: &[u8], tail: &[u8], child: &[u8]) -> Option<BodyContent> {
    if child.is_empty() {
        return None;
    }
    let mut xml = Vec::with_capacity(head.len() + child.len() + tail.len());
    xml.extend_from_slice(head);
    xml.extend_from_slice(child);
    xml.extend_from_slice(tail);
    let mut parsed = CT_Document::from_xml(&xml).ok()?;
    if parsed.body.content.len() != 1 {
        return None;
    }
    let again = parsed.body_item_xml(&parsed.body.content[0]).ok()?;
    if again.as_slice() != child {
        return None;
    }
    parsed.body.content.pop()
}

impl BodyBlockScan {
    pub(super) fn from_source(bytes: Vec<u8>) -> Self {
        Self {
            declares_namespace: bytes_declare_namespace(&bytes),
            holds_field_words: holds_field_words(&bytes),
            bytes,
            owners: Vec::new(),
            items: None,
            staged: None,
            standalone_value: OnceLock::new(),
            proved: OnceLock::new(),
            identifier_proof: OnceLock::new(),
            canonical_read: OnceLock::new(),
            empty_namespace_owners: OnceLock::new(),
            namespace_spans: OnceLock::new(),
            decodes: OnceLock::new(),
            scoped_correspondence: OnceLock::new(),
        }
    }

    fn decodes(&self, root: &[u8]) -> bool {
        *self.decodes.get_or_init(|| {
            let mut xml = root.to_vec();
            xml.extend_from_slice(b"<w:body>");
            xml.extend_from_slice(&self.bytes);
            xml.extend_from_slice(b"</w:body></w:document>");
            xml_decodes(&xml)
        })
    }

    /// Read one source child that may normalize on write. Only an exact
    /// writer round trip supplies `staged`, so a parsed source never grants
    /// serialization reuse by value. Private owner facts keep the full path
    /// when that exact writer proof is unavailable.
    pub(super) fn read_source_child(
        &mut self,
        head: &[u8],
        tail: &[u8],
        scopes: &DocumentNamespaceScopes,
    ) -> Option<BodyContent> {
        let end = head.len().checked_add(self.bytes.len())?;
        let mut xml = Vec::with_capacity(end.checked_add(tail.len())?);
        xml.extend_from_slice(head);
        xml.extend_from_slice(&self.bytes);
        xml.extend_from_slice(tail);
        closed_word_body(&xml, head.len()..end)?;
        let mut parsed = CT_Document::from_xml(&xml).ok()?;
        if parsed.body.content.len() != 1 {
            return None;
        }
        let item = parsed.body.content.pop()?;
        let written_child = parsed.body_item_xml(&item).ok()?;
        let round_trips = written_child == self.bytes;
        drop(written_child);
        if !round_trips
            && (self.bytes.contains(&b'&')
                || self.holds_field_words
                || !item.owner_facts_reusable_by_value())
        {
            return None;
        }
        let shell_xml = [head, tail].concat();
        let shell = CT_Document::from_xml(&shell_xml).ok()?;
        if !shell.body.content.is_empty() {
            return None;
        }
        let emptied = parsed.to_xml().ok()?;
        let expected_shell = shell.to_xml().ok()?;
        let frames_match = emptied == expected_shell;
        drop(emptied);
        drop(expected_shell);
        if !frames_match {
            return None;
        }
        self.proved = OnceLock::from(ParsedChild {
            root_declarations: scopes.root_declarations.clone(),
            body_declarations: scopes.body_declarations.clone(),
            body_bindings: scopes.body_bindings.clone(),
            parsed: Some(item.clone()),
        });
        self.standalone_value = OnceLock::new();
        self.staged = round_trips.then(|| Arc::new(item.clone()));
        Some(item)
    }

    pub(super) fn reads_anew(&self) -> bool {
        self.holds_field_words
            || self
                .staged
                .as_deref()
                .is_some_and(|item| !item.owner_facts_reusable_by_value())
    }

    fn read_child<'a>(
        &'a self,
        head: &[u8],
        tail: &[u8],
        scopes: &DocumentNamespaceScopes,
    ) -> Option<Cow<'a, BodyContent>> {
        if self.reads_anew() {
            return parse_one_child(head, tail, &self.bytes).map(Cow::Owned);
        }
        let cached = self.proved.get_or_init(|| ParsedChild {
            root_declarations: scopes.root_declarations.clone(),
            body_declarations: scopes.body_declarations.clone(),
            body_bindings: scopes.body_bindings.clone(),
            parsed: parse_one_child(head, tail, &self.bytes),
        });
        if cached.root_declarations != scopes.root_declarations
            || cached.body_declarations != scopes.body_declarations
            || cached.body_bindings != scopes.body_bindings
        {
            return parse_one_child(head, tail, &self.bytes).map(Cow::Owned);
        }
        cached.parsed.as_ref().map(Cow::Borrowed)
    }
}

fn plain_picture_free_paragraph(paragraph: &CT_P) -> bool {
    if !paragraph.hyperlinks.is_empty()
        || !paragraph.comment_ranges.is_empty()
        || !paragraph.bookmark_markers.is_empty()
        || !paragraph.extra_xml.is_empty()
        || !paragraph.content_controls.is_empty()
        || !paragraph.revisions.is_empty()
        || !paragraph.equations.is_empty()
        || !paragraph.rubies.is_empty()
        || paragraph.properties.as_ref().is_some_and(|p| {
            p.tabs.is_some()
                || p.change.is_some()
                || p.numbering_revision.is_some()
                || !p.revision_xml.is_empty()
                || !p.numbering_revision_xml.is_empty()
                || !p.num_pr_extra_xml.is_empty()
                || !p.num_pr_extra_attributes.is_empty()
                || p.rpr.as_ref().is_some_and(|r| {
                    !r.revision_xml.is_empty()
                        || r.change.is_some()
                        || !r.revision_markers.is_empty()
                })
        })
    {
        return false;
    }
    paragraph.runs.iter().all(|run| {
        run.extra_xml.is_empty()
            && run.extra_xml_positions.is_empty()
            && run.alt_drawings.is_empty()
            && run.properties.as_ref().is_none_or(|p| {
                p.revision_xml.is_empty() && p.change.is_none() && p.revision_markers.is_empty()
            })
            && run.content.iter().all(|content| {
                matches!(
                    content,
                    RunContent::Text(_) | RunContent::Tab | RunContent::Break(..)
                )
            })
    })
}

// Only the complete typed table/row/cell owner chain grants this proof.
// Property subtrees stay opaque to the unchanged story paragraph discovery.
fn plain_picture_free_table(table: &CT_Tbl) -> bool {
    let plain_borders = |borders: &oxoffice_docx_oxml::table::CT_TblBorders| {
        borders.extra_xml.is_empty()
            && [
                borders.top.as_ref(),
                borders.bottom.as_ref(),
                borders.left.as_ref(),
                borders.right.as_ref(),
                borders.inside_h.as_ref(),
                borders.inside_v.as_ref(),
            ]
            .into_iter()
            .flatten()
            .all(|edge| edge.extra_attributes.is_empty() && edge.source_xml.is_none())
    };
    table.extra_xml.is_empty()
        && table.content_controls.is_empty()
        && table.properties.as_ref().is_none_or(|p| {
            p.change.is_none()
                && p.revision_xml.is_empty()
                && p.extra_xml.is_empty()
                && p.borders.as_ref().is_none_or(plain_borders)
                && p.shading
                    .as_ref()
                    .is_none_or(|shading| shading.extra_attributes.is_empty())
        })
        && table
            .grid
            .as_ref()
            .is_none_or(|grid| grid.grid_change_xml.is_none() && grid.extra_xml.is_empty())
        && table.rows.iter().all(|row| {
            row.extra_xml.is_empty()
                && row.content_controls.is_empty()
                && row.table_property_exception.is_none()
                && row.properties.as_ref().is_none_or(|p| {
                    p.revision_markers.is_empty()
                        && p.revision_xml.is_empty()
                        && p.revision_xml_positions.is_empty()
                        && p.extra_xml.is_empty()
                        && !p.moved_away
                })
                && row.cells.iter().all(|cell| {
                    cell.extra_xml.is_empty()
                        && cell.properties.as_ref().is_none_or(|p| {
                            p.extra_xml.is_empty()
                                && p.borders.as_ref().is_none_or(plain_borders)
                                && p.shading
                                    .as_ref()
                                    .is_none_or(|shading| shading.extra_attributes.is_empty())
                        })
                        && cell.content.iter().all(|content| match content {
                            CellContent::Paragraph(paragraph) => {
                                plain_picture_free_paragraph(paragraph)
                            }
                            CellContent::Table(_) | CellContent::ContentControl(_) => false,
                        })
                })
        })
}

pub(super) fn picture_free_span_kind(
    ranges: &[(Range<usize>, StoryItemKind)],
    owner: &StoryOwnerSpan,
    span: &Range<usize>,
) -> Option<StoryItemKind> {
    if span.start >= span.end {
        return None;
    }
    let index = ranges.partition_point(|(range, _)| range.start <= span.start);
    let (range, kind) = ranges.get(index.checked_sub(1)?)?;
    match kind {
        StoryItemKind::Paragraph if range == span => Some(*kind),
        StoryItemKind::Table
            if owner.kind == StoryKind::TableCell
                && range.start < owner.full.start
                && owner.full.start < owner.full.end
                && owner.full.end < range.end
                && owner.full.start < span.start
                && span.end < owner.full.end =>
        {
            Some(*kind)
        }
        _ => None,
    }
}

fn holds(start: usize, block: &BodyBlockScan, range: &Range<usize>) -> bool {
    let end = start + block.bytes.len();
    start <= range.start && range.start < end && range.end <= end
}

impl MainStoryLayout {
    pub(super) fn owner_kinds(&self) -> Vec<StoryKind> {
        let (head, tail) = self.frame_owners();
        head.iter()
            .chain(
                self.blocks
                    .iter()
                    .flat_map(|(_, block)| block.owners.iter()),
            )
            .chain(&tail)
            .map(|owner| owner.kind)
            .collect()
    }

    fn frame_offset(&self, at: usize, is_end: bool) -> usize {
        let head = self.content.start;
        if at > head || (at == head && !is_end) {
            at - head + self.content.end
        } else {
            at
        }
    }

    fn frame_owners(&self) -> (Vec<StoryOwnerSpan>, Vec<StoryOwnerSpan>) {
        self.frame
            .owners
            .iter()
            .map(|owner| StoryOwnerSpan {
                full: self.frame_offset(owner.full.start, false)
                    ..self.frame_offset(owner.full.end, true),
                ..owner.clone()
            })
            .partition(|owner| owner.full.start < self.content.start)
    }

    pub(super) fn owners(&self, xml: &[u8]) -> Result<Vec<StoryOwnerSpan>> {
        let (head, tail) = self.frame_owners();
        let mut owners = head;
        for (start, block) in &self.blocks {
            owners.extend(block.owners.iter().map(|owner| StoryOwnerSpan {
                full: owner.full.start + start..owner.full.end + start,
                ..owner.clone()
            }));
        }
        owners.extend(tail);
        let mut counts = HashMap::new();
        for owner in &mut owners {
            let count = counts.entry(owner.kind).or_insert(0usize);
            owner.owner_index = *count;
            *count += 1;
            if owner.kind == StoryKind::Body {
                owner.fingerprint = self.body_fingerprint(xml, &owner.full)?;
            }
        }
        Ok(owners)
    }

    fn body_fingerprint(&self, xml: &[u8], full: &Range<usize>) -> Result<u64> {
        match self.fingerprint {
            Some(fingerprint) => Ok(fingerprint),
            None => structural_fingerprint(&xml[full.clone()]),
        }
    }

    pub(super) fn owner(
        &self,
        xml: &[u8],
        kind: StoryKind,
        index: usize,
    ) -> Result<Option<StoryOwnerSpan>> {
        let (head, tail) = self.frame_owners();
        let blocks = self.blocks.iter().flat_map(|(start, block)| {
            block.owners.iter().map(move |owner| StoryOwnerSpan {
                full: owner.full.start + start..owner.full.end + start,
                ..owner.clone()
            })
        });
        let Some(mut owner) = head
            .into_iter()
            .chain(blocks)
            .chain(tail)
            .filter(|owner| owner.kind == kind)
            .nth(index)
        else {
            return Ok(None);
        };
        owner.owner_index = index;
        if kind == StoryKind::Body {
            owner.fingerprint = self.body_fingerprint(xml, &owner.full)?;
        }
        Ok(Some(owner))
    }

    fn frame_items(&self) -> impl Iterator<Item = StoryItemSpan> + '_ {
        self.frame
            .items()
            .iter()
            .map(|item| item.moved(|at, is_end| self.frame_offset(at, is_end)))
    }

    pub(super) fn body_items(&self) -> Vec<StoryItemSpan> {
        let mut items = Vec::new();
        for (start, block) in &self.blocks {
            items.extend(
                block
                    .items()
                    .iter()
                    .map(|item| item.moved(|at, _| at + start)),
            );
        }
        items.extend(self.frame_items());
        items
    }

    fn body_item(&self, index: usize) -> (Option<StoryItemSpan>, usize) {
        let mut before = 0;
        let mut found = None;
        for (start, block) in &self.blocks {
            if found.is_none()
                && let Some(item) = index
                    .checked_sub(before)
                    .and_then(|local| block.items().get(local))
            {
                found = Some(item.moved(|at, _| at + start));
            }
            before += block.items().len();
        }
        if found.is_none() {
            found = index
                .checked_sub(before)
                .and_then(|local| self.frame_items().nth(local));
        }
        (found, before + self.frame.items().len())
    }

    fn is_section_properties(&self, xml: &[u8], item: &StoryItemSpan) -> Result<bool> {
        if item.kind != StoryItemKind::PreservedNode {
            return Ok(false);
        }
        match self.block_source(xml, &item.full) {
            Some((source, shift)) => content_fragment_root_is_section_properties(
                &source,
                &item.moved(|at, _| at - shift),
            ),
            None => content_fragment_root_is_section_properties(xml, item),
        }
    }

    pub(super) fn direct_body_item(
        &self,
        xml: &[u8],
        slot: usize,
        content: bool,
    ) -> Result<Option<(usize, StoryItemSpan)>> {
        let blocks = self.blocks.iter().flat_map(|(start, block)| {
            block
                .items()
                .iter()
                .map(move |item| item.moved(|at, _| at + start))
        });
        let mut direct = 0;
        for (index, item) in blocks.chain(self.frame_items()).enumerate() {
            if !item.direct_owner_child || (content && self.is_section_properties(xml, &item)?) {
                continue;
            }
            if direct == slot {
                return Ok(Some((index, item)));
            }
            direct += 1;
        }
        Ok(None)
    }

    fn child_holding(&self, range: &Range<usize>) -> Option<usize> {
        let index = self
            .blocks
            .partition_point(|(start, _)| *start <= range.start)
            .checked_sub(1)?;
        let (start, block) = &self.blocks[index];
        holds(*start, block, range).then_some(index)
    }

    fn joined_source(&self, xml: &[u8], start: usize, bytes: &[u8]) -> (Vec<u8>, usize) {
        let mut source = Vec::with_capacity(xml.len() - self.content.len() + bytes.len());
        source.extend_from_slice(&xml[..self.content.start]);
        source.extend_from_slice(bytes);
        source.extend_from_slice(&xml[self.content.end..]);
        (source, start - self.content.start)
    }

    fn child_source(&self, xml: &[u8], index: usize) -> (Vec<u8>, usize) {
        let (start, block) = &self.blocks[index];
        self.joined_source(xml, *start, &block.bytes)
    }

    fn block_source(&self, xml: &[u8], range: &Range<usize>) -> Option<(Vec<u8>, usize)> {
        if self.content.end <= range.start {
            return Some(self.joined_source(xml, self.content.end, &[]));
        }
        Some(self.child_source(xml, self.child_holding(range)?))
    }

    pub(super) fn owner_items(
        &self,
        xml: &[u8],
        owner: &StoryOwnerSpan,
    ) -> Result<Vec<StoryItemSpan>> {
        if owner.kind == StoryKind::Body {
            return Ok(self.body_items());
        }
        let Some((source, shift)) = self.block_source(xml, &owner.full) else {
            return scan_story_items(xml, owner, None);
        };
        let local = StoryOwnerSpan {
            full: owner.full.start - shift..owner.full.end - shift,
            ..owner.clone()
        };
        Ok(scan_story_items(&source, &local, None)?
            .iter()
            .map(|item| item.moved(|at, _| at + shift))
            .collect())
    }
}

/// One Word body covers `covered`. A scan error, a missing body, or another
/// body is an unproved wrapper. Callers keep the whole scanner and publish
/// no new error.
fn closed_word_body(xml: &[u8], covered: Range<usize>) -> Option<()> {
    let owners = scan_story_owners(xml, StoryKind::Body).ok()?;
    single_frame_body(&owners, Some(covered)).ok().map(|_| ())
}

/// The cache represents one frame body. More than one body, or one body that
/// does not cover `covered`, is a layout miss. Callers then use the old reader.
pub(super) fn single_frame_body(
    owners: &[StoryOwnerSpan],
    covered: Option<Range<usize>>,
) -> Result<&StoryOwnerSpan> {
    let mut body = None;
    for owner in owners {
        if owner.kind != StoryKind::Body {
            continue;
        }
        if body.is_some() {
            return Err(Error::Other(
                "main part body frame does not match the scanned body owners".to_owned(),
            ));
        }
        body = Some(owner);
    }
    let body =
        body.ok_or_else(|| Error::Other("main document body owner was not found".to_owned()))?;
    if let Some(covered) = covered
        && (covered.start < body.full.start || covered.end > body.full.end)
    {
        return Err(Error::Other(
            "main part body frame does not match the scanned body owners".to_owned(),
        ));
    }
    Ok(body)
}

pub(super) fn scan_body_block(
    head: &[u8],
    block: Option<&[u8]>,
    tail: &[u8],
    items: bool,
) -> Result<BodyBlockScan> {
    let xml = [head, block.unwrap_or_default(), tail].concat();
    let owners = scan_story_owners(&xml, StoryKind::Body)?;
    let covered = block.map(|block| head.len()..head.len() + block.len());
    let body = single_frame_body(&owners, covered.clone())?;
    let body_items = items
        .then(|| scan_story_items(&xml, body, None))
        .transpose()?;
    let Some(block) = block else {
        return Ok(BodyBlockScan {
            bytes: Vec::new(),
            owners,
            items: body_items,
            staged: None,
            standalone_value: OnceLock::new(),
            proved: OnceLock::new(),
            declares_namespace: bytes_declare_namespace(block.unwrap_or_default()),
            holds_field_words: false,
            identifier_proof: OnceLock::new(),
            canonical_read: OnceLock::new(),
            empty_namespace_owners: OnceLock::new(),
            namespace_spans: OnceLock::new(),
            decodes: OnceLock::new(),
            scoped_correspondence: OnceLock::new(),
        });
    };
    let range = head.len()..head.len() + block.len();
    Ok(child_scan(&range, block, &owners, body_items.as_deref()))
}

/// One forward sweep over spans already ordered by start. Children are
/// contiguous and ordered. Each span is compared with the one child that can
/// still contain its start. A span that overlaps that child and extends
/// outside it crosses a raw block boundary.
fn span_crosses_child(
    children: &[Range<usize>],
    spans: impl Iterator<Item = Range<usize>>,
) -> bool {
    let mut index = 0;
    for span in spans {
        while index < children.len() && children[index].end <= span.start {
            index += 1;
        }
        if let Some(child) = children.get(index) {
            let overlaps = span.start < child.end && child.start < span.end;
            let contains = child.start <= span.start && span.end <= child.end;
            if overlaps && !contains {
                return true;
            }
        }
    }
    false
}

fn body_spans_cross_children(
    children: &[Range<usize>],
    owners: &[StoryOwnerSpan],
    items: Option<&[StoryItemSpan]>,
) -> bool {
    span_crosses_child(
        children,
        owners
            .iter()
            .filter(|owner| owner.kind != StoryKind::Body)
            .map(|owner| owner.full.clone()),
    ) || items.is_some_and(|items| {
        span_crosses_child(children, items.iter().map(|item| item.full.clone()))
    })
}

fn child_scan(
    range: &Range<usize>,
    bytes: &[u8],
    owners: &[StoryOwnerSpan],
    items: Option<&[StoryItemSpan]>,
) -> BodyBlockScan {
    let contains = |span: &Range<usize>| range.start <= span.start && span.end <= range.end;
    let first = owners.partition_point(|owner| owner.full.start < range.start);
    BodyBlockScan {
        bytes: bytes.to_vec(),
        owners: owners[first..]
            .iter()
            .take_while(|owner| owner.full.start < range.end)
            .filter(|owner| owner.kind != StoryKind::Body && contains(&owner.full))
            .map(|owner| StoryOwnerSpan {
                full: owner.full.start - range.start..owner.full.end - range.start,
                ..owner.clone()
            })
            .collect(),
        items: items.map(|items| {
            let first = items.partition_point(|item| item.full.start < range.start);
            items[first..]
                .iter()
                .take_while(|item| item.full.start < range.end)
                .filter(|item| contains(&item.full))
                .map(|item| item.moved(|at, _| at - range.start))
                .collect()
        }),
        staged: None,
        standalone_value: OnceLock::new(),
        proved: OnceLock::new(),
        declares_namespace: bytes_declare_namespace(bytes),
        holds_field_words: holds_field_words(bytes),
        identifier_proof: OnceLock::new(),
        canonical_read: OnceLock::new(),
        empty_namespace_owners: OnceLock::new(),
        namespace_spans: OnceLock::new(),
        decodes: OnceLock::new(),
        scoped_correspondence: OnceLock::new(),
    }
}

pub(super) fn scan_body_children(
    xml: &[u8],
    children: &[Range<usize>],
    todo: &[usize],
    items: bool,
) -> Result<(Vec<BodyBlockScan>, u64)> {
    let owners = scan_story_owners(xml, StoryKind::Body)?;
    let covered = children
        .first()
        .map(|first| first.start..children.last().expect("last child").end);
    let body = single_frame_body(&owners, covered)?;
    let body_items = items
        .then(|| scan_story_items(xml, body, None))
        .transpose()?;
    if body_spans_cross_children(children, &owners, body_items.as_deref()) {
        return Err(Error::Other(
            "body block scan does not cover a span that crosses child ranges".to_owned(),
        ));
    }
    let scans = todo
        .iter()
        .map(|index| {
            let range = &children[*index];
            child_scan(range, &xml[range.clone()], &owners, body_items.as_deref())
        })
        .collect();
    Ok((scans, body.fingerprint))
}

pub(super) fn full_direct_body_item_location(
    xml: &[u8],
    part_name: &str,
    slot: usize,
) -> Result<Option<ContentLocation>> {
    let owners = scan_story_owners(xml, StoryKind::Body)?;
    let body = owners
        .iter()
        .find(|owner| owner.kind == StoryKind::Body)
        .ok_or_else(|| Error::Other("main document body owner was not found".to_owned()))?;
    let items = scan_story_items(xml, body, None)?;
    let direct = direct_story_content_items(xml, body)?;
    let Some(item) = direct.get(slot) else {
        return Ok(None);
    };
    let index = items
        .iter()
        .position(|candidate| candidate.full == item.full)
        .ok_or_else(|| {
            Error::Other("direct body item is missing from the body story".to_owned())
        })?;
    Ok(Some(ContentLocation::new(
        StoryId {
            kind: StoryKind::Body,
            part_name: part_name.to_owned(),
            owner_index: body.owner_index,
            fingerprint: body.fingerprint,
        },
        item.kind,
        vec![index],
    )))
}

pub(super) fn story_owner_items(
    xml: &[u8],
    owner: &StoryOwnerSpan,
    layout: Option<&MainStoryLayout>,
) -> Result<Vec<StoryItemSpan>> {
    match layout {
        Some(layout) => layout.owner_items(xml, owner),
        None => scan_story_items(xml, owner, None),
    }
}

pub(super) fn story_owner_item(
    xml: &[u8],
    owner: &StoryOwnerSpan,
    layout: Option<&MainStoryLayout>,
    index: usize,
) -> Result<(Option<StoryItemSpan>, usize)> {
    if let Some(layout) = layout
        && owner.kind == StoryKind::Body
    {
        return Ok(layout.body_item(index));
    }
    let items = story_owner_items(xml, owner, layout)?;
    Ok((items.get(index).cloned(), items.len()))
}

pub(super) fn story_control_paragraphs(
    xml: &[u8],
    control: &StoryItemSpan,
    layout: Option<&MainStoryLayout>,
) -> Result<Vec<Range<usize>>> {
    let Some((source, shift)) = layout.and_then(|layout| layout.block_source(xml, &control.full))
    else {
        return scan_story_control_paragraphs(xml, control);
    };
    let local = control.moved(|at, _| at - shift);
    Ok(scan_story_control_paragraphs(&source, &local)?
        .into_iter()
        .map(|paragraph| paragraph.start + shift..paragraph.end + shift)
        .collect())
}

#[cfg(test)]
mod producer_root_tests {
    use super::*;

    /// After a flush that kept Word's root, the record of the written part
    /// still proves it: its frame is read, so the flush's own checks and a
    /// later flush after another edit need no whole parse.
    #[test]
    fn a_kept_producer_root_leaves_a_record_that_proves_the_saved_part() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/corpus/files/word/report.docx");
        let mut doc = Document::open(path).unwrap();
        let mut previous: Option<Vec<Arc<BodyBlockScan>>> = None;
        for text in ["first", " second"] {
            doc.paragraph_mut(0).unwrap().add_run(text);
            doc.flush_document_to_package().unwrap();
            let saved = doc.package.get_part(&doc.doc_part_name).unwrap().to_vec();
            assert!(saved.starts_with(b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<w:document xmlns:wpc="));
            let record = doc
                .main_part_blocks
                .as_ref()
                .expect("a record of the saved part");
            assert!(matches!(record.frame.get(), Some(Some(_))));
            // The proofs answer for these bytes as they do for the written
            // ones, instead of declining to a whole read.
            let (written, canonical) = doc.canonical_source().unwrap();
            let canonical = canonical.unwrap();
            assert_eq!(
                record.proved_document_matches(&saved, &doc.document),
                canonical.proved_document_matches(&written, &doc.document),
            );
            assert!(
                record
                    .proved_document_matches(&saved, &doc.document)
                    .is_some()
            );
            assert!(record.retained_nested_owners(&saved).is_some());
            // New pieces under the producer's root, not the written record's.
            assert!(!Arc::ptr_eq(&record.blocks[1], &canonical.blocks[1]));
            // The blocks the second edit left alone keep the first flush's
            // pieces, with their proofs.
            if let Some(previous) = &previous {
                assert!(!Arc::ptr_eq(&record.blocks[0], &previous[0]));
                for (kept, before) in record.blocks.iter().zip(previous).skip(1) {
                    assert!(Arc::ptr_eq(kept, before));
                }
            }
            previous = Some(record.blocks.as_ref().clone());
        }
    }
}
