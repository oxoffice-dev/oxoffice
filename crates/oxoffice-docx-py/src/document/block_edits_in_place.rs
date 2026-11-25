//! Facade edits of direct body blocks made in place under one `BodyBlocks`
//! unit and published by the block replacement splice
//! ([`PyDocument::publish_body_blocks_replaced`]), and the direct body block
//! that holds a story item, read from the inventory pieces.

use super::paragraph_in_place::{BlockRetention, BlocksPublication};
use super::*;

/// Stops holding the edit undo however the edit ends; an edit that panics
/// or fails leaves the document as it was. Arm it right after
/// `hold_edit_undo`; [`HeldUndo::take`] ends the hold and hands the undo
/// over.
pub(super) struct HeldUndo<'a> {
    pub(super) document: &'a mut oxoffice_docx::Document,
    pub(super) armed: bool,
}

impl Drop for HeldUndo<'_> {
    fn drop(&mut self) {
        let undo = self.document.take_edit_undo();
        if self.armed
            && let Some(undo) = undo
        {
            self.document.undo_edit(undo);
        }
    }
}

impl HeldUndo<'_> {
    pub(super) fn take(mut self) -> Option<oxoffice_docx::EditUndo> {
        self.armed = false;
        self.document.take_edit_undo()
    }
}

impl PyDocument {
    /// The location of `item` for an in-place route, when it names an item
    /// of the main part's body, its table cells or text boxes: checked as
    /// [`Self::native_location`] checks it, without listing the stories,
    /// which reads the whole main part for the fingerprints the staged route
    /// checks. The in-place routes find the item in its block instead; the
    /// staged route a caller falls back to takes [`Self::native_location`].
    pub(crate) fn in_place_location(
        &self,
        py: Python<'_>,
        item: &PyStoryItem,
    ) -> PyResult<Option<oxoffice_docx::ContentLocation>> {
        use oxoffice_docx::StoryKind;
        self.validate_owned_item(py, item)?;
        if item.binding.is_none()
            && item.story.binding.is_none()
            && item.revision != self.revisions.current()
        {
            // The staged route's location refuses it.
            return Ok(None);
        }
        let story = &item.story;
        let Some(kind) = [StoryKind::Body, StoryKind::TableCell, StoryKind::TextBox]
            .into_iter()
            .find(|kind| story_kind_name(*kind) == story.kind)
        else {
            return Ok(None);
        };
        Ok(Some(oxoffice_docx::ContentLocation::new(
            oxoffice_docx::StoryId::new(kind, story.part_name.clone(), story.owner_index),
            story_item_kind_from_name(&item.kind)?,
            item.index_path.clone(),
        )))
    }

    /// The direct body block that holds the story item at `location`, from
    /// the pieces of the inventory, without reading the stories: its slot,
    /// the index of the item's story owner among the block's owners of its
    /// kind (0 for the body) and the index of the item among that owner's
    /// items inside the block. `None` when the item is not in the main
    /// part's body, its table cells or text boxes, or the pieces do not
    /// tell.
    pub(crate) fn story_item_block(
        &mut self,
        py: Python<'_>,
        location: &oxoffice_docx::ContentLocation,
    ) -> PyResult<Option<oxoffice_docx::ControlBlock>> {
        use oxoffice_docx::StoryKind;
        let story = location.story();
        let kind = story.kind();
        if !matches!(
            kind,
            StoryKind::Body | StoryKind::TableCell | StoryKind::TextBox
        ) || (kind == StoryKind::Body && story.owner_index() != 0)
            || location.index_path().len() != 1
        {
            return Ok(None);
        }
        self.ensure_word_authority(py)?;
        let Some(pieces) = self.pieces.as_ref() else {
            return Ok(None);
        };
        let context = self.inner.body_item_context();
        if pieces.context.as_ref() != Some(&context)
            || pieces.blocks.len() != self.inner.body_children().len()
            || story.part_name() != context.part_name()
        {
            return Ok(None);
        }
        // A body item is counted among the body's items, an item of a cell
        // or a text box by its story among the block's stories.
        let index = location.index_path()[0];
        let target = if kind == StoryKind::Body {
            index
        } else {
            story.owner_index()
        };
        let mut base = 0;
        for (slot, piece) in pieces.blocks.iter().enumerate() {
            let Some(owners) = piece.owners.as_ref() else {
                return Ok(None);
            };
            let count = match kind {
                StoryKind::Body => owners.body_items,
                StoryKind::TableCell => owners.cell_stories,
                _ => owners.text_boxes,
            };
            if target < base + count {
                let local = target - base;
                return Ok(Some(if kind == StoryKind::Body {
                    oxoffice_docx::ControlBlock {
                        slot,
                        owner_index: 0,
                        item_index: local,
                    }
                } else {
                    oxoffice_docx::ControlBlock {
                        slot,
                        owner_index: local,
                        item_index: index,
                    }
                }));
            }
            base += count;
        }
        Ok(None)
    }

    /// Run `edit`, a facade edit of direct body blocks made in place under
    /// one `BodyBlocks` unit, and publish the blocks it changed with the
    /// block replacement splice. `Ok(None)` from the edit, or a shape the
    /// splice does not take, leaves the document and every handle as they
    /// were and answers `Ok(None)`: the caller then takes its own route. An
    /// edit or a publication that fails leaves them as they were too, and
    /// its error is returned.
    pub(crate) fn publish_blocks_edited_in_place<T: Send>(
        &mut self,
        py: Python<'_>,
        operation: &'static str,
        retention: BlockRetention,
        edit: impl FnOnce(&mut oxoffice_docx::Document) -> oxoffice_docx::Result<Option<T>> + Send,
    ) -> PyResult<Option<T>> {
        self.ensure_word_authority(py)?;
        self.inner.hold_edit_undo();
        let held = HeldUndo {
            document: &mut self.inner,
            armed: true,
        };
        let (result, held) = py.detach(move || {
            let result = edit(held.document);
            (result, held)
        });
        let value = match result {
            Ok(Some(value)) => value,
            // Dropping the guard undoes whatever the edit left.
            Ok(None) => return Ok(None),
            Err(error) => {
                drop(held);
                return Err(docx_to_pyerr(py, error));
            }
        };
        let Some(undo) = held.take() else {
            return Ok(Some(value));
        };
        let Some(change) = undo.report().body.filter(|_| undo.report().changed()) else {
            if undo.report().changed() {
                // A unit other than the body changed: not an edit of blocks.
                self.inner.undo_edit(undo);
                return Ok(None);
            }
            return Ok(Some(value));
        };
        let previous = undo
            .previous_body_blocks()
            .map(<[_]>::to_vec)
            .unwrap_or_default();
        // A publication that panics leaves the inventory as it was: put the
        // blocks back before the panic goes on.
        let published = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.publish_body_blocks_replaced(
                py,
                change.slot,
                &previous,
                change.inserted,
                operation,
                retention,
            )
        }));
        let published = match published {
            Ok(published) => published,
            Err(panic) => {
                self.inner.undo_edit(undo);
                std::panic::resume_unwind(panic);
            }
        };
        match published {
            Ok(BlocksPublication::Unchanged | BlocksPublication::Published) => Ok(Some(value)),
            Ok(BlocksPublication::WholeRoute) => {
                self.inner.undo_edit(undo);
                Ok(None)
            }
            Err(error) => {
                self.inner.undo_edit(undo);
                Err(error)
            }
        }
    }

    /// Run `edit`, a facade form operation made in place on the direct body
    /// blocks that hold its targets (one `BodyBlocks` unit per block), the
    /// header and footer parts that hold them and the custom XML items its
    /// bound controls write, and publish it: a changed block keeps every
    /// handle when it holds the stories, items, paragraphs, tables and links
    /// it held (a value or a placeholder written inside a control keeps the
    /// control and every element around it), the owners of a changed header
    /// or footer part are read again from that part alone. `Ok(None)` from
    /// the edit, or a block of another shape, leaves the document and every
    /// handle as they were and answers `Ok(None)`: the caller then takes its
    /// staged route. An edit or a publication that fails leaves the document
    /// as it was.
    pub(crate) fn publish_form_edited_in_place(
        &mut self,
        py: Python<'_>,
        operation: &'static str,
        edit: impl FnOnce(
            &mut oxoffice_docx::Document,
        ) -> oxoffice_docx::Result<Option<oxoffice_docx::FormEdit>>
        + Send,
    ) -> PyResult<Option<Vec<usize>>> {
        self.ensure_word_authority(py)?;
        #[cfg(debug_assertions)]
        let before = self.inner.clone_for_staging();
        self.inner.hold_edit_undo();
        let held = HeldUndo {
            document: &mut self.inner,
            armed: true,
        };
        let (result, held) = py.detach(move || {
            let result = edit(held.document);
            (result, held)
        });
        let edited = match result {
            Ok(Some(edited)) => edited,
            // Dropping the guard undoes whatever the edit left.
            Ok(None) => return Ok(None),
            Err(error) => {
                drop(held);
                return Err(docx_to_pyerr(py, error));
            }
        };
        let Some(undo) = held.take() else {
            return Ok(Some(edited.counts));
        };
        let slots = undo
            .report()
            .body_blocks
            .iter()
            .map(|change| change.slot)
            .collect::<Vec<_>>();
        let kept = if slots.is_empty() {
            None
        } else {
            match self.blocks_keeping_owners(&slots) {
                Some(kept) => Some(kept),
                None => {
                    self.inner.undo_edit(undo);
                    return Ok(None);
                }
            }
        };
        for story in &edited.stories {
            let published = self
                .splice_story_owners(
                    py,
                    operation,
                    story,
                    #[cfg(debug_assertions)]
                    &before,
                )
                .and_then(|spliced| {
                    if spliced {
                        return Ok(());
                    }
                    let prepared = self.prepare_story_revision(
                        py,
                        operation,
                        story,
                        #[cfg(debug_assertions)]
                        &before,
                    )?;
                    self.commit_story_revision(prepared)
                });
            if let Err(error) = published {
                self.inner.undo_edit(undo);
                return Err(error);
            }
        }
        if let Some(kept) = kept {
            self.keep_block_owners(kept);
        }
        #[cfg(debug_assertions)]
        if self.snapshot_scope.is_some() {
            self.assert_word_inventory_rebuilds(false);
        }
        Ok(Some(edited.counts))
    }

    /// Read the owners of the direct body blocks at `slots` again after an
    /// edit inside them, and keep them in their pieces; false, changing
    /// nothing, when one block now holds other stories, items, paragraphs,
    /// tables or links than it held.
    pub(super) fn follow_blocks_keeping_owners(&mut self, slots: &[usize]) -> bool {
        match self.blocks_keeping_owners(slots) {
            Some(kept) => {
                self.keep_block_owners(kept);
                true
            }
            None => false,
        }
    }

    /// What [`Self::follow_blocks_keeping_owners`] keeps, read without
    /// changing anything; `None` when a block holds other owners than it held.
    fn blocks_keeping_owners(&self, slots: &[usize]) -> Option<KeptBlocks> {
        let mut read = Vec::with_capacity(slots.len());
        for &slot in slots {
            let live = self.inner.body_children().get(slot)?;
            let Ok(Some(owners)) = word_block_owners(&self.inner, live) else {
                return None;
            };
            read.push((slot, live.clone(), owners));
        }
        let pieces = self.pieces.as_ref()?;
        if pieces.context.as_ref() != Some(&self.inner.body_item_context())
            || pieces.blocks.len() != self.inner.body_children().len()
        {
            return None;
        }
        let shape = |owners: &WordBlockOwners| {
            (
                owners.body_items,
                owners.cell_stories,
                owners.text_boxes,
                owners.tables,
                owners.paragraphs.clone(),
                owners.relationships.clone(),
                owners.facts.links.len(),
            )
        };
        // The text a paragraph owner of a changed block reads, at its route
        // in the document: the block's owners keep their places.
        let body = oxoffice_docx::StoryId::body();
        let mut texts = Vec::new();
        for (slot, _, owners) in &read {
            let held = pieces.blocks[*slot].owners.as_ref()?;
            if shape(held) != shape(owners) {
                return None;
            }
            let mut before = (0, (0, 0));
            for piece in &pieces.blocks[..*slot] {
                let owners = piece.owners.as_ref()?;
                before.0 += owners.body_items;
                before.1.0 += owners.cell_stories;
                before.1.1 += owners.text_boxes;
            }
            let old = word_piece_owner_addresses(held.facts.clone(), 0);
            let new = word_piece_owner_addresses(owners.facts.clone(), 0);
            if old.len() != new.len() {
                return None;
            }
            for (old, new) in old.iter().zip(new) {
                match (&old.read_facts, new.read_facts, &new.location) {
                    (WordReadFacts::None, WordReadFacts::None, _) => {}
                    (WordReadFacts::Table(was), WordReadFacts::Table(is), _) if *was == is => {}
                    (WordReadFacts::Paragraph(was), WordReadFacts::Paragraph(is), _)
                        if *was == is => {}
                    (
                        WordReadFacts::Paragraph(_),
                        WordReadFacts::Paragraph(is),
                        WordOwnerLocation::Paragraph(local),
                    ) => {
                        let placed =
                            WordOwnerLocation::Paragraph(placed_route(local, &body, before));
                        let key = self.indexes.find(&self.owners, &placed)?;
                        texts.push((key, is));
                    }
                    _ => return None,
                }
            }
        }
        Some(KeptBlocks { read, texts })
    }

    /// Keep what [`Self::blocks_keeping_owners`] read.
    fn keep_block_owners(&mut self, KeptBlocks { read, texts }: KeptBlocks) {
        for (key, text) in texts {
            self.owners[key].read_facts = WordReadFacts::Paragraph(text);
        }
        let Some(pieces) = &mut self.pieces else {
            return;
        };
        for (slot, live, owners) in read {
            pieces.blocks[slot] = std::sync::Arc::new(WordBlockPiece {
                block: std::sync::Arc::new(live),
                owners: Some(owners),
            });
        }
    }
}

/// The owners of edited blocks read again, kept in their pieces by
/// [`PyDocument::keep_block_owners`], and the texts their paragraph owners
/// read now, by owner key.
struct KeptBlocks {
    read: Vec<(usize, oxoffice_docx_oxml::BodyContent, WordBlockOwners)>,
    texts: Vec<(usize, String)>,
}
