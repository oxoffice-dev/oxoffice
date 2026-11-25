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

    /// Run `edit`, a facade edit that may change direct body blocks in place
    /// without changing any owner they hold (the extents of their pictures),
    /// and refresh the pieces of the blocks it changed. Nothing is
    /// published: every handle stays, as no element a handle names changed.
    /// A piece that did not hold the block before the edit stays as it is:
    /// the next publication reads that block again.
    pub(crate) fn edit_blocks_keeping_owners<T: Send>(
        &mut self,
        py: Python<'_>,
        edit: impl FnOnce(&mut oxoffice_docx::Document) -> oxoffice_docx::Result<T> + Send,
    ) -> PyResult<T> {
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
            Ok(value) => value,
            Err(error) => {
                drop(held);
                return Err(docx_to_pyerr(py, error));
            }
        };
        let Some(undo) = held.take() else {
            return Ok(value);
        };
        let context = self.inner.body_item_context();
        let mut refreshed = Vec::new();
        if let Some(pieces) = self.pieces.as_ref()
            && pieces.context.as_ref() == Some(&context)
            && pieces.blocks.len() == self.inner.body_children().len()
        {
            for (slot, previous) in undo.previous_body_block_units() {
                let ([previous], Some(piece), Some(block)) = (
                    previous,
                    pieces.blocks.get(slot),
                    self.inner.body_children().get(slot),
                ) else {
                    continue;
                };
                if !aligned_block(piece, previous) || piece.block.as_ref() == block {
                    continue;
                }
                // The edit refuses a block that reads back with other runs:
                // a kept handle names the owner it named.
                debug_assert_eq!(
                    piece.block.owner_shape(),
                    block.owner_shape(),
                    "an edit keeping the owners changed the owners of block {slot}"
                );
                if let Ok(Some(owners)) = word_block_owners(&self.inner, block) {
                    refreshed.push((
                        slot,
                        std::sync::Arc::new(WordBlockPiece {
                            block: std::sync::Arc::new(block.clone()),
                            owners: Some(owners),
                        }),
                    ));
                }
            }
        }
        if let Some(pieces) = self.pieces.as_mut() {
            for (slot, piece) in refreshed {
                pieces.blocks[slot] = piece;
            }
        }
        #[cfg(debug_assertions)]
        self.assert_word_inventory_rebuilds(false);
        Ok(value)
    }
}
