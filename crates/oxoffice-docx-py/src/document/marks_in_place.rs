//! Comment marks written in place in direct body paragraphs: the facade
//! changes the live document, and the paragraphs and the new comment story
//! are published alone, every other handle kept.

use super::paragraph_in_place::BlocksPublication;
use super::*;

/// Puts an in-place edit back on every exit but a publication: an error,
/// the whole route, a panic.
struct HeldEdit<'a> {
    document: &'a mut PyDocument,
    undo: Option<oxoffice_docx::EditUndo>,
}

impl Drop for HeldEdit<'_> {
    fn drop(&mut self) {
        if let Some(undo) = self.undo.take() {
            self.document.inner.undo_edit(undo);
        }
    }
}

impl PyDocument {
    /// Publish an in-place edit that changed the direct body paragraphs at
    /// the slots of `previous` (each was the block beside its slot, one
    /// paragraph for one) and, with `story`, added that story in a related
    /// part (a note or a comment, the last entry of its part): the
    /// paragraphs keep their owners and properties states and their runs
    /// are born again, then the story's owners are born; every other owner
    /// keeps its states.
    ///
    /// [`BlocksPublication::WholeRoute`] with nothing published when any
    /// half cannot be taken. One paragraph, with or without a story, is
    /// published under one revision
    /// ([`Self::publish_body_paragraph_and_story`],
    /// [`Self::publish_body_blocks_replaced`]). Several paragraphs answer
    /// the whole route: their plans would have to be read before one
    /// commit, which the paragraph publication does not expose yet.
    pub(crate) fn publish_body_paragraphs_and_story(
        &mut self,
        py: Python<'_>,
        previous: &[(usize, oxoffice_docx_oxml::BodyContent)],
        story: Option<&oxoffice_docx::StoryId>,
        operation: &'static str,
    ) -> PyResult<BlocksPublication> {
        let [(slot, block)] = previous else {
            return Ok(BlocksPublication::WholeRoute);
        };
        match story {
            Some(story) => {
                self.publish_body_paragraph_and_story(py, *slot, block, story, operation)
            }
            None => self.publish_body_blocks_replaced(
                py,
                *slot,
                std::slice::from_ref(block),
                1,
                operation,
                paragraph_in_place::BlockRetention::EditedParagraphKeepsItsOwner,
            ),
        }
    }

    /// Whether [`Self::publish_body_paragraphs_and_story`] can take an
    /// edit of the direct body paragraphs at `slots`, from the inventory
    /// alone, before the edit runs. The caller ensured the Word authority.
    fn body_paragraphs_publishable(&self, slots: &[usize]) -> bool {
        slots.len() == 1 && slots.iter().all(|slot| self.publishable_body_slot(*slot))
    }

    /// Add the reply of `Document.reply_to` on the live document when the
    /// facade writes its marks in place in direct body paragraphs
    /// ([`oxoffice_docx::Document::reply_in_place_slots`]) that a block
    /// replacement publishes, and the comments part is published by part:
    /// the paragraphs and the reply's story are published alone
    /// ([`Self::publish_body_paragraphs_and_story`]), every other handle
    /// stays. `Ok(None)` leaves the document and every handle as they were,
    /// for the whole route, decided before the edit where the inventory
    /// tells. A refusal of the edit leaves them as they were too, and is the
    /// answer.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn reply_in_place(
        &mut self,
        py: Python<'_>,
        parent_id: i32,
        author: &str,
        initials: Option<&str>,
        text: &str,
        date: oxoffice_docx::CommentDate<'_>,
    ) -> PyResult<Option<i32>> {
        const OPERATION: &str = "Document.reply_to";
        let Some(slots) = self.inner.reply_in_place_slots(parent_id) else {
            return Ok(None);
        };
        self.ensure_word_authority(py)?;
        if !self.body_paragraphs_publishable(&slots)
            || !self.annotation_part_published(py, oxoffice_docx::StoryKind::Comment)?
        {
            return Ok(None);
        }
        self.inner.hold_edit_undo();
        let replied = self
            .inner
            .reply_to_with_initials(parent_id, author, initials, text, date);
        let undo = self.inner.take_edit_undo();
        let id = match replied {
            Ok(id) => id,
            Err(error) => {
                if let Some(undo) = undo {
                    self.inner.undo_edit(undo);
                }
                return Err(docx_to_pyerr(py, error));
            }
        };
        let undo = undo.expect("a reply in body paragraphs edits in place");
        let report = undo.report();
        let previous = undo
            .previous_body_block_units()
            .filter(|(slot, _)| report.body_blocks.iter().any(|change| change.slot == *slot))
            .map(|(slot, blocks)| blocks.first().cloned().map(|block| (slot, block)))
            .collect::<Option<Vec<_>>>();
        let mut held = HeldEdit {
            document: self,
            undo: Some(undo),
        };
        let (Some(previous), Ok(Some(story))) = (previous, held.document.inner.comment_story(id))
        else {
            return Ok(None);
        };
        let published = held.document.publish_body_paragraphs_and_story(
            py,
            &previous,
            Some(&story),
            OPERATION,
        )?;
        if published != BlocksPublication::Published {
            return Ok(None);
        }
        held.undo = None;
        Ok(Some(id))
    }

    /// Accept (`accept`) or reject revision `id` on the live document when
    /// the facade resolves it in place in one direct body paragraph
    /// ([`oxoffice_docx::Document::revision_id_in_place_slots`]) that a
    /// block replacement publishes: that paragraph is published alone
    /// ([`Self::publish_body_paragraphs_and_story`]), every other handle
    /// stays, and a resolution that changes nothing keeps every handle.
    /// `Ok(None)` leaves the document and every handle as they were, for
    /// the whole route, decided before the edit (no slot, or several). A
    /// refusal of the facade leaves them as they were too, and is the
    /// answer.
    pub(super) fn revision_id_in_place(
        &mut self,
        py: Python<'_>,
        id: i32,
        accept: bool,
        operation: &'static str,
    ) -> PyResult<Option<usize>> {
        let Some(slots) = self.inner.revision_id_in_place_slots(id, accept) else {
            return Ok(None);
        };
        if slots.len() != 1 {
            return Ok(None);
        }
        self.ensure_word_authority(py)?;
        if !self.body_paragraphs_publishable(&slots) {
            return Ok(None);
        }
        self.inner.hold_edit_undo();
        let resolved = if accept {
            self.inner.accept_revision_id(id)
        } else {
            self.inner.reject_revision_id(id)
        };
        let undo = self.inner.take_edit_undo();
        let count = match resolved {
            Ok(count) => count,
            Err(error) => {
                if let Some(undo) = undo {
                    self.inner.undo_edit(undo);
                }
                return Err(docx_to_pyerr(py, error));
            }
        };
        // Nothing changed in place: every handle stays.
        let Some(undo) = undo else {
            return Ok(Some(count));
        };
        let report = undo.report();
        let previous = undo
            .previous_body_block_units()
            .filter(|(slot, _)| report.body_blocks.iter().any(|change| change.slot == *slot))
            .map(|(slot, blocks)| blocks.first().cloned().map(|block| (slot, block)))
            .collect::<Option<Vec<_>>>();
        let mut held = HeldEdit {
            document: self,
            undo: Some(undo),
        };
        let Some(previous) = previous else {
            return Ok(None);
        };
        match held
            .document
            .publish_body_paragraphs_and_story(py, &previous, None, operation)?
        {
            BlocksPublication::Published | BlocksPublication::Unchanged => {
                held.undo = None;
                Ok(Some(count))
            }
            BlocksPublication::WholeRoute => Ok(None),
        }
    }
}
