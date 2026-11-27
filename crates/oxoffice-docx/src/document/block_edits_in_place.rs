//! Edits of one direct body paragraph on a copy of that paragraph alone,
//! swapped in when the edit succeeds.

use super::*;

impl Document {
    /// Apply `edit` to paragraph `index` (counted as [`Self::paragraph_mut`]
    /// counts it) without a copy of the document, when that paragraph is a
    /// direct body child: `edit` runs on a copy of the paragraph, which takes
    /// its place when the edit succeeds. Returns the edit's value and the
    /// paragraph it replaced, so that a caller whose own publication refuses
    /// can put it back with [`Self::restore_body_paragraph`].
    ///
    /// An edit that fails or panics leaves the document untouched. A
    /// paragraph that is not a direct body child (inside a block content
    /// control) returns `Ok(None)` before `edit` runs; an index out of range
    /// is an error. Only an edit that changes this one paragraph belongs
    /// here: one that reserves an identifier or adds a relationship does not.
    #[doc(hidden)]
    pub fn edit_body_paragraph_in_place<T>(
        &mut self,
        index: usize,
        edit: impl FnOnce(&mut Paragraph<'_>) -> Result<T>,
    ) -> Result<Option<(T, CT_P)>> {
        if self.paragraph(index).is_none() {
            return Err(Error::Other("paragraph index out of range".to_owned()));
        }
        let Some(slot) = self.content_index_of_paragraph(index) else {
            return Ok(None);
        };
        let Some(BodyContent::Paragraph(current)) = self.document.body.content.get(slot) else {
            unreachable!("a direct body paragraph's slot holds it");
        };
        let mut edited = current.as_ref().clone();
        let before = self.tracking.is_some().then(|| edited.clone());
        let value = edit(&mut Paragraph { inner: &mut edited })?;
        // Under tracked changes, the edit is written as revisions; the ids
        // it reserves are the only identifiers this route reserves. An edit
        // the session cannot write takes the caller's other route, which
        // refuses it.
        if let Some(before) = before {
            match self.redline_edited_paragraph(&before, edited) {
                Ok(redlined) => edited = redlined,
                Err(_) => return Ok(None),
            }
        }
        let Some(BodyContent::Paragraph(current)) = self.document.body.content.get_mut(slot) else {
            unreachable!("a direct body paragraph's slot holds it");
        };
        let old = std::mem::replace(current.as_mut(), edited);
        self.invalidate_layout();
        Ok(Some((value, old)))
    }

    /// Put back `paragraph`, which [`Self::edit_body_paragraph_in_place`]
    /// returned for paragraph `index`, in that paragraph's place. The edited
    /// paragraph it replaces is returned. An index that no longer names a
    /// direct body paragraph is an error and changes nothing.
    #[doc(hidden)]
    pub fn restore_body_paragraph(&mut self, index: usize, paragraph: CT_P) -> Result<CT_P> {
        let slot = self.content_index_of_paragraph(index).ok_or_else(|| {
            Error::Other("paragraph index does not name a direct body paragraph".to_owned())
        })?;
        let Some(BodyContent::Paragraph(current)) = self.document.body.content.get_mut(slot) else {
            unreachable!("a direct body paragraph's slot holds it");
        };
        let edited = std::mem::replace(current.as_mut(), paragraph);
        self.invalidate_layout();
        Ok(edited)
    }

    /// Replace each direct body block at `slot..slot + count` that an edit
    /// in place built with the block a reopen of the saved document reads
    /// ([`Self::reread_body_block`]), so that reads after the edit see what
    /// a save writes (a field added to a run reads as its own runs, as the
    /// reader splits it). Costs those blocks alone. Returns whether a block
    /// changed. An error, or a range out of the body, changes nothing.
    #[doc(hidden)]
    pub fn reread_body_blocks_in_place(&mut self, slot: usize, count: usize) -> Result<bool> {
        let blocks = slot
            .checked_add(count)
            .and_then(|end| self.document.body.content.get(slot..end))
            .ok_or_else(|| Error::Other("body block range out of range".to_owned()))?;
        let mut reread = Vec::new();
        for (offset, block) in blocks.iter().enumerate() {
            let read = self.reread_body_block(block)?;
            if read != *block {
                reread.push((slot + offset, read));
            }
        }
        if reread.is_empty() {
            return Ok(false);
        }
        for (slot, block) in reread {
            self.document.body.content[slot] = block;
        }
        self.invalidate_layout();
        Ok(true)
    }

    /// Put `block` in direct body slot `slot` in place of the block there,
    /// which is returned: the undo of an edit in place that the caller's
    /// publication refused, or a copy of the document as it was before one.
    /// A slot out of range is an error and changes nothing.
    #[doc(hidden)]
    pub fn swap_body_block(&mut self, slot: usize, block: BodyContent) -> Result<BodyContent> {
        let current = self
            .document
            .body
            .content
            .get_mut(slot)
            .ok_or_else(|| Error::Other("body slot out of range".to_owned()))?;
        let old = std::mem::replace(current, block);
        self.invalidate_layout();
        Ok(old)
    }
}

/// Direct body blocks [`Document::remove_body_blocks_in_place`] removed, held
/// so that the caller can put them back with
/// [`Document::restore_body_blocks`] when its own publication refuses.
#[doc(hidden)]
#[derive(Debug)]
pub struct RemovedBodyBlocks {
    slot: usize,
    blocks: Vec<BodyContent>,
    /// The content relationships the removal recorded as freed, which a
    /// restore takes back.
    freed: Vec<(String, String)>,
}

impl RemovedBodyBlocks {
    /// The direct body slot of the first block removed, an owned bookmark
    /// start included.
    pub fn slot(&self) -> usize {
        self.slot
    }

    /// The removed blocks, in body order, owned bookmark markers included.
    pub fn blocks(&self) -> &[BodyContent] {
        &self.blocks
    }
}

/// The elements whose presence in a removed block keeps the staged route:
/// comment ranges and references, note references and section breaks, which
/// the staged route reconciles with the comments, the notes and the sections.
const STAGED_REMOVAL_ELEMENTS: [&[u8]; 6] = [
    b"commentRangeStart",
    b"commentRangeEnd",
    b"commentReference",
    b"footnoteReference",
    b"endnoteReference",
    b"sectPr",
];

impl Document {
    /// Put `blocks` in place of the `removed` direct body blocks at `slot`,
    /// which are returned: the undo of a splice made in place, or a copy of
    /// the document as it was before one. A range out of the body is an
    /// error and changes nothing.
    #[doc(hidden)]
    pub fn splice_body_blocks(
        &mut self,
        slot: usize,
        removed: usize,
        blocks: Vec<BodyContent>,
    ) -> Result<Vec<BodyContent>> {
        let len = self.document.body.content.len();
        if slot.checked_add(removed).is_none_or(|end| end > len) {
            return Err(Error::Other("body block range out of range".to_owned()));
        }
        let old = self
            .document
            .body
            .content
            .splice(slot..slot + removed, blocks)
            .collect();
        self.invalidate_layout();
        Ok(old)
    }

    /// Remove the direct body blocks `start..end` in place, with the
    /// bookmark markers they own (as [`Self::remove_content_range`] removes
    /// them), and return them so that the caller can put them back with
    /// [`Self::restore_body_blocks`]. The content relationships they name
    /// (pictures, charts, embedded objects, links) are freed: the next write
    /// drops those nothing names then, as after [`Self::remove_content_at`].
    ///
    /// `Ok(None)` changes nothing and leaves the staged route to the caller,
    /// which reconciles what this one does not: blocks holding a comment
    /// range or reference, a note reference or a section break, owned
    /// markers not written right around the range, a body that a typed edit
    /// cannot save, or a range that is empty or out of the body. The bookmark
    /// and drawing identifiers of the removed blocks stay reserved until the
    /// document is reopened.
    #[doc(hidden)]
    pub fn remove_body_blocks_in_place(
        &mut self,
        start: usize,
        end: usize,
    ) -> Result<Option<RemovedBodyBlocks>> {
        let Some(range) = self.removable_body_range(start, end)? else {
            return Ok(None);
        };
        let freed = self.freed_by_blocks(range.clone())?;
        let blocks = self
            .document
            .body
            .content
            .drain(range.clone())
            .collect::<Vec<_>>();
        self.invalidate_after_removal();
        Ok(Some(RemovedBodyBlocks {
            slot: range.start,
            blocks,
            freed: self.record_freed(freed),
        }))
    }

    /// Put back blocks [`Self::remove_body_blocks_in_place`] or
    /// [`Self::pop_body_block_in_place`] removed, and take back the
    /// relationships the removal freed. A slot past the end of the body is
    /// an error and changes nothing.
    #[doc(hidden)]
    pub fn restore_body_blocks(&mut self, removed: RemovedBodyBlocks) -> Result<()> {
        let RemovedBodyBlocks {
            slot,
            blocks,
            freed,
        } = removed;
        self.splice_body_blocks(slot, 0, blocks)?;
        // The body is the one the removal found savable.
        self.unsavable_body_edit_cache = OnceLock::from(None);
        self.freed_relationships
            .retain(|entry| !freed.contains(entry));
        Ok(())
    }

    /// [`Self::remove_content_at`] for the direct body block at `slot`, in
    /// place: the block is removed with the bookmark markers it owns, and its
    /// fragment is built from the block and those markers alone. `Ok(None)`
    /// changes nothing, as [`Self::remove_body_blocks_in_place`] refuses.
    #[doc(hidden)]
    pub fn pop_body_block_in_place(
        &mut self,
        slot: usize,
    ) -> Result<Option<(ContentFragment, RemovedBodyBlocks)>> {
        let Some(range) = self.removable_body_range(slot, slot + 1)? else {
            return Ok(None);
        };
        let story = StoryId::new(StoryKind::Body, self.doc_part_name.clone(), 0);
        let item_xml = |document: &Self, block: &BodyContent| -> Result<_> {
            let part = document.document.standalone_body_item_xml(block)?;
            let owner = scan_story_owners(&part, StoryKind::Body)?
                .into_iter()
                .find(|owner| owner.kind == StoryKind::Body)
                .ok_or_else(|| Error::Other("standalone body item has no body owner".into()))?;
            let items = direct_story_content_items(&part, &owner)?;
            let [item] = items.as_slice() else {
                return Err(Error::Other(
                    "a standalone body item holds one direct item".into(),
                ));
            };
            let scope = story_namespace_scope_at(&part, item.full.start)?;
            Ok((item.kind, part[item.full.clone()].to_vec(), scope))
        };
        let (kind, xml, namespace_scope) = item_xml(self, &self.document.body.content[slot])?;
        let mut fragment = ContentFragment {
            kind,
            xml,
            source_part_name: Some(self.doc_part_name.clone()),
            source_story_kind: Some(StoryKind::Body),
            source_owner_index: Some(0),
            namespace_scope,
            relationships: Vec::new(),
            markers: FragmentMarkers::default(),
        };
        validate_fragment_relationships(self, &story, &fragment)?;
        fragment.relationships = referenced_relationships(self, &story, &fragment)?;
        let marker = |index: usize| -> Result<Vec<u8>> {
            Ok(item_xml(self, &self.document.body.content[index])?.1)
        };
        fragment.markers = FragmentMarkers {
            before: (range.start..slot).map(marker).collect::<Result<_>>()?,
            after: (slot + 1..range.end).map(marker).collect::<Result<_>>()?,
        };
        let freed = fragment
            .relationships
            .iter()
            .filter(|(relationship, _)| is_content_relationship(&relationship.rel_type))
            .map(|(relationship, _)| (self.doc_part_name.clone(), relationship.id.clone()))
            .collect();
        let blocks = self
            .document
            .body
            .content
            .drain(range.clone())
            .collect::<Vec<_>>();
        self.invalidate_after_removal();
        let removed = RemovedBodyBlocks {
            slot: range.start,
            blocks,
            freed: self.record_freed(freed),
        };
        Ok(Some((fragment, removed)))
    }

    /// The direct body slots a removal of `start..end` in place takes, owned
    /// bookmark markers included, or `None` for the staged route.
    fn removable_body_range(&self, start: usize, end: usize) -> Result<Option<Range<usize>>> {
        let content = &self.document.body.content;
        if start >= end || end > content.len() {
            return Ok(None);
        }
        if self.unsavable_body_edit().is_some() {
            return Ok(None);
        }
        let staged = STAGED_REMOVAL_ELEMENTS
            .iter()
            .copied()
            .collect::<HashSet<&[u8]>>();
        for block in &content[start..end] {
            let xml = self.document.body_item_xml(block)?;
            if xml_holds_element_named(&xml, &staged) {
                return Ok(None);
            }
        }
        // The owned markers must sit right around the range, so that one
        // contiguous range goes and comes back.
        let owned = self.owned_bookmark_markers(start, start..end)?;
        let first = start - owned.before.len();
        let last = end + owned.after.len();
        if !owned.before.iter().copied().eq(first..start)
            || !owned.after.iter().copied().eq(end..last)
        {
            return Ok(None);
        }
        Ok(Some(first..last))
    }

    /// Drop the layout after a removal in place, keeping the answer that the
    /// body saves after a typed edit: the removal found it savable, and a
    /// body without some of its blocks holds no namespace owner or prefix
    /// it did not hold (debug builds check a kept answer on each read).
    fn invalidate_after_removal(&mut self) {
        self.invalidate_layout();
        self.unsavable_body_edit_cache = OnceLock::from(None);
    }

    /// The content relationships of the main part that the blocks in `range`
    /// name.
    fn freed_by_blocks(&self, range: Range<usize>) -> Result<Vec<(String, String)>> {
        let Some(relationships) = self.package.get_part_rels(&self.doc_part_name) else {
            return Ok(Vec::new());
        };
        let mut freed = Vec::new();
        for block in &self.document.body.content[range] {
            let xml = self.document.body_item_xml(block)?;
            let mut named = xml_attribute_values(&xml)?
                .into_iter()
                .filter(|id| {
                    relationships
                        .get_by_id(id)
                        .is_some_and(|relationship| is_content_relationship(&relationship.rel_type))
                })
                .collect::<Vec<_>>();
            named.sort_unstable();
            freed.extend(named.into_iter().map(|id| (self.doc_part_name.clone(), id)));
        }
        Ok(freed)
    }

    /// Record `freed` relationships for the next write, and return those not
    /// recorded before, which a restore takes back.
    fn record_freed(&mut self, freed: Vec<(String, String)>) -> Vec<(String, String)> {
        let mut added = Vec::new();
        for entry in freed {
            if !self.freed_relationships.contains(&entry) {
                self.freed_relationships.push(entry.clone());
                added.push(entry);
            }
        }
        added
    }
}

impl ContentFragment {
    /// Whether two fragments are the same taken content: the same item,
    /// source, namespace scope, relationships and bookmark markers.
    #[doc(hidden)]
    pub fn same_taken_fragment(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.xml == other.xml
            && self.source_part_name == other.source_part_name
            && self.source_story_kind == other.source_story_kind
            && self.source_owner_index == other.source_owner_index
            && self.namespace_scope == other.namespace_scope
            && self.relationships == other.relationships
            && self.markers.before == other.markers.before
            && self.markers.after == other.markers.after
    }
}
