//! Accepting or rejecting one revision id in the body paragraphs that hold
//! it, in place.
//!
//! See `specs/2026-11-24-staged-edits-in-place.md` (S2). The staged route
//! clones the document, renders the whole main part and every story part,
//! and reopens the copy. When the id sits in direct body paragraphs alone
//! and resolving it changes nothing else, each of those paragraphs is
//! resolved alone, written in the frame of the main part, and swapped in
//! place under one `BodyBlocks` unit each.

use super::written_references::{REVISION_ELEMENTS, raw_attribute_values};
use super::*;
use crate::revision::{Resolution, body_paragraph_resolves_alone, resolve_body_paragraph_alone};

impl Document {
    /// The direct body blocks that accepting (`accept`) or rejecting
    /// revision `id` changes in place, in slot order, or `None` when the call takes the staged
    /// route. The binding asks it before the call, to know that the
    /// resolution publishes these blocks alone.
    ///
    /// `Some` only when the id sits in direct body paragraphs and in no
    /// other story (headers, footers, comments, notes, the body's final
    /// section), the document holds no comparison comment record, no note
    /// or comment model waits to be written, a typed edit of the body can be
    /// saved, each paragraph resolves alone
    /// (`revision::body_paragraph_resolves_alone`: no paragraph mark of the
    /// id, no section break, comment mark, note reference, field or custody
    /// pair in it), and each resolved paragraph writes back the bytes the
    /// resolver gave it, which the staged route saves as they are. It costs
    /// those paragraphs and the parts of the other stories, not the body.
    #[doc(hidden)]
    pub fn revision_id_in_place_slots(&self, id: i32, accept: bool) -> Option<Vec<usize>> {
        let resolution = if accept {
            Resolution::Accept
        } else {
            Resolution::Reject
        };
        self.revision_id_in_place_blocks(resolution, id)
            .map(|resolved| resolved.slots)
    }

    /// The resolved paragraphs of [`Self::revision_id_in_place_slots`] for
    /// `resolution`, each resolved alone in its main-part frame
    /// (`Self::body_block_in_frame`).
    fn revision_id_in_place_blocks(
        &self,
        resolution: Resolution,
        id: i32,
    ) -> Option<ResolvedBlocks> {
        if self.footnotes_dirty
            || self.comments_dirty
            || self
                .package
                .contains_part(crate::comparison::COMMENT_COMPARISON_PART)
        {
            return None;
        }
        let hits = self.body_slots_with_revision_id(id);
        if hits.is_empty() {
            return None;
        }
        // Read from the cache, or computed once over the main part.
        if self.unsavable_body_edit().is_some() {
            return None;
        }
        if self.main_part_frame_has_revision_id(id) || self.story_parts_hold_revision_id(id)? {
            return None;
        }
        let mut resolved = ResolvedBlocks {
            slots: Vec::with_capacity(hits.len()),
            blocks: Vec::with_capacity(hits.len()),
            count: 0,
        };
        for hit in hits {
            let block = self.document.body.content.get(hit.slot)?;
            if !matches!(block, BodyContent::Paragraph(_)) {
                return None;
            }
            let (xml, head, tail) = self.body_block_in_frame(block).ok()?;
            if !body_paragraph_resolves_alone(&xml, id).ok()? {
                return None;
            }
            // A refusal of the resolver is the staged route's to give.
            let (output, count) = resolve_body_paragraph_alone(&xml, resolution, id).ok()?;
            resolved.slots.push(hit.slot);
            if count == 0 {
                continue;
            }
            let (head, tail) = (&xml[..head], &xml[xml.len() - tail..]);
            if !output.starts_with(head)
                || !output.ends_with(tail)
                || output.len() < head.len() + tail.len()
            {
                return None;
            }
            let child = &output[head.len()..output.len() - tail.len()];
            // The paragraph must write back what the resolver wrote: the
            // staged route keeps those bytes as the block's source.
            let paragraph @ BodyContent::Paragraph(_) =
                Self::reparse_body_block(head, tail, child)?
            else {
                return None;
            };
            resolved.blocks.push((hit.slot, paragraph));
            resolved.count = resolved.count.checked_add(count)?;
        }
        Some(resolved)
    }

    /// Whether a story part the staged route resolves besides the main
    /// part (a header, a footer, the comments, the footnotes or the
    /// endnotes) holds a revision element of `id`, read from its bytes:
    /// `None` when such a part is missing. A part costs its own length, not
    /// the body's.
    fn story_parts_hold_revision_id(&self, id: i32) -> Option<bool> {
        let Some(relationships) = self.package.get_part_rels(&self.doc_part_name) else {
            return Some(false);
        };
        for relationship in &relationships.items {
            if !matches!(
                relationship.rel_type.as_str(),
                rel_types::HEADER
                    | rel_types::FOOTER
                    | rel_types::COMMENTS
                    | rel_types::FOOTNOTES
                    | rel_types::ENDNOTES
            ) || !relationship_is_internal(relationship)
            {
                continue;
            }
            let part_name =
                OpcPackage::resolve_rel_target(&self.doc_part_name, &relationship.target);
            let bytes = self.package.get_part(&part_name)?;
            if raw_attribute_values(bytes, REVISION_ELEMENTS, b"id", &mut |value| {
                value.trim().parse::<i32>() == Ok(id)
            }) {
                return Some(true);
            }
        }
        Some(false)
    }

    /// Accept or reject revision `id`: in place in the body paragraphs that
    /// hold it when [`Self::revision_id_in_place_slots`] allows it, else by
    /// the staged route.
    pub(crate) fn resolve_revision_id(&mut self, resolution: Resolution, id: i32) -> Result<usize> {
        let Some(resolved) = self.revision_id_in_place_blocks(resolution, id) else {
            return self.resolve_revision_id_staged(resolution, id);
        };
        #[cfg(debug_assertions)]
        let before = self.clone_for_staging();
        let result = self.resolve_revision_id_in_place(resolved);
        #[cfg(debug_assertions)]
        self.assert_in_place_matches_staged(before, &result, |staged| {
            staged.resolve_revision_id_staged(resolution, id)
        });
        result
    }

    /// The staged route of [`Self::resolve_revision_id`], its debug oracle.
    fn resolve_revision_id_staged(&mut self, resolution: Resolution, id: i32) -> Result<usize> {
        self.resolve_revisions_with_note_publication(
            resolution,
            crate::revision::RevisionScope::Id(id),
            None,
        )
    }

    fn resolve_revision_id_in_place(&mut self, resolved: ResolvedBlocks) -> Result<usize> {
        let ResolvedBlocks { blocks, count, .. } = resolved;
        if blocks.is_empty() {
            return Ok(0);
        }
        let savable_before = self.unsavable_body_edit_cache.get() == Some(&None);
        let slots = blocks.iter().map(|(slot, _)| *slot).collect::<Vec<_>>();
        let units = slots
            .iter()
            .map(|slot| EditUnit::BodyBlocks {
                slot: *slot,
                count: 1,
            })
            .collect::<Vec<_>>();
        self.edit_in_place(&units, |document| {
            for (slot, block) in blocks {
                document.document.body.content[slot] = block;
            }
            document.invalidate_layout();
            Ok(())
        })?;
        if self.take_last_edit_changed() {
            self.keep_body_savable_after_typed_inserts(&slots, savable_before);
        }
        Ok(count)
    }
}

/// The body paragraphs that hold a revision id, resolved alone.
struct ResolvedBlocks {
    /// The slots of the paragraphs that hold the id.
    slots: Vec<usize>,
    /// Each paragraph the resolution changes, resolved.
    blocks: Vec<(usize, BodyContent)>,
    /// The revision elements resolved.
    count: usize,
}
