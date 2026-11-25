//! Moving a comment within its direct body paragraph, in place.
//!
//! See `specs/2026-11-24-staged-edits-in-place.md`. The staged route of
//! [`Document::move_comment`] clones the document, writes the main part,
//! removes the comment's marks and reference run from its bytes, anchors
//! the range again and reopens the copy. When the comment has no reply, its
//! start, end and reference run are direct children of one direct body
//! paragraph and the destination is a range of that same paragraph, the
//! paragraph is edited alone on its typed model and swapped in place under
//! one `BodyBlocks` unit.

use super::*;
use crate::comments::StoryRunRange;
use oxoffice_docx_oxml::text::{CommentRangeMarker, RangeAnchor};

impl Document {
    /// The direct body slots that moving comment `id` onto `range` edits in
    /// place, `block` being the direct body block the caller found for the
    /// range's paragraph: `Some` when [`Self::move_comment_in_block`] moves
    /// it in place, decided before any edit, `None` for the staged route of
    /// [`Self::move_comment`].
    #[doc(hidden)]
    pub fn move_comment_in_place_slots(
        &self,
        id: i32,
        range: &StoryRunRange,
        block: crate::ControlBlock,
    ) -> Option<Vec<usize>> {
        self.comment_moved_in_block(id, range, block)
            .map(|_| vec![block.slot])
    }

    /// [`Self::move_comment`] for a root comment without replies whose
    /// start, end and reference run are direct children of the direct body
    /// paragraph at `block` (found by the caller for the range's paragraph,
    /// as [`Self::add_story_permission_range_in_block`] takes it), moved to
    /// a range of that same paragraph: the marks and the reference run are
    /// removed from the typed paragraph, the range anchored again on the
    /// accepted run axis and the original reference run put back in place
    /// of the one anchoring wrote, as the staged route does on the bytes;
    /// the paragraph is swapped in place. `Ok(Some(()))` when moved or
    /// already on the range. `Ok(None)`, changing nothing, for every other
    /// shape (decided before the edit, [`Self::move_comment_in_place_slots`]):
    /// the caller then takes [`Self::move_comment`].
    #[doc(hidden)]
    pub fn move_comment_in_block(
        &mut self,
        id: i32,
        range: &StoryRunRange,
        block: crate::ControlBlock,
    ) -> Result<Option<()>> {
        let Some(moved) = self.comment_moved_in_block(id, range, block) else {
            return Ok(None);
        };
        #[cfg(debug_assertions)]
        let before = self.clone_for_staging();
        let result = match moved {
            // Already on the range: the staged route publishes nothing.
            None => Ok(()),
            Some(paragraph) => self
                .replace_body_block_in_place(
                    block.slot,
                    BodyContent::Paragraph(Box::new(paragraph)),
                )
                .map(|_| ()),
        };
        #[cfg(debug_assertions)]
        self.assert_in_place_matches_staged_declarations_aside(before, &result, |staged| {
            let mut range = range.clone();
            range.start.location = staged.fingerprinted(&range.start.location);
            range.end.location = staged.fingerprinted(&range.end.location);
            staged.move_comment(id, range)
        });
        result.map(Some)
    }

    /// The paragraph at `block` with comment `id` moved onto `range`
    /// (`Some(None)` when the comment already has that range), or `None`
    /// for the staged route: see [`Self::move_comment_in_block`]. A
    /// destination the staged route refuses is the staged route's to
    /// refuse.
    fn comment_moved_in_block(
        &self,
        id: i32,
        range: &StoryRunRange,
        block: crate::ControlBlock,
    ) -> Option<Option<CT_P>> {
        // Pending notes or comments the staged route's flush would write, or
        // a comparison record its reopening reads.
        if self.footnotes_dirty
            || self.comments_dirty
            || self
                .package
                .contains_part(crate::comparison::COMMENT_COMPARISON_PART)
            || range.start.location != range.end.location
        {
            return None;
        }
        let paragraph = self.direct_body_paragraph_at(&range.start.location, block)?;
        if !self.comment_is_root_without_replies(id) {
            return None;
        }
        // The comment's three marks, all in this paragraph, as its direct
        // children.
        let hits = self.body_slots_counting_comment_references(&HashSet::from([id]));
        let [(hit, typed)] = hits.as_slice() else {
            return None;
        };
        if hit.raw || hit.slot != block.slot || *typed != 3 {
            return None;
        }
        let starts = paragraph
            .comment_ranges
            .iter()
            .filter(|marker| matches!(marker, CommentRangeMarker::Start { id: found, .. } if *found == id))
            .count();
        let ends = paragraph
            .comment_ranges
            .iter()
            .filter(|marker| matches!(marker, CommentRangeMarker::End { id: found, .. } if *found == id))
            .count();
        let is_reference = |content: &RunContent| matches!(content, RunContent::CommentReference { id: found, .. } if *found == id);
        let references = paragraph
            .runs
            .iter()
            .enumerate()
            .filter(|(_, run)| run.content.iter().any(is_reference))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let [reference_run] = references.as_slice() else {
            return None;
        };
        if starts != 1 || ends != 1 {
            return None;
        }
        // The reference run travels whole: it holds the reference alone,
        // its properties and the attributes of its root aside (else the
        // staged route keeps the rest).
        let transported = paragraph.runs[*reference_run].clone();
        if transported.content.len() != 1
            || transported.extra_xml_positions.len() != transported.extra_xml.len()
            || !transported
                .extra_xml_positions
                .iter()
                .all(|encoded| CT_R::raw_child_is_root_attributes(*encoded))
            || !transported.alt_drawings.is_empty()
        {
            return None;
        }
        // The current range on the accepted run axis: already there, the
        // staged route publishes nothing.
        let (_, false, Some(current_start), Some(current_end)) =
            paragraph.accepted_comment_projection(id, false).ok()?
        else {
            return None;
        };
        if (current_start, current_end) == (range.start.run_index, range.end.run_index) {
            return Some(None);
        }
        // The accepted index of the reference run, which the staged route
        // takes out of the destination's run boundaries after it.
        let mut reference_index = None;
        for (index, path) in paragraph.accepted_run_paths().iter().enumerate() {
            if paragraph
                .accepted_run(path)?
                .content
                .iter()
                .any(is_reference)
            {
                reference_index.replace(index).is_none().then_some(())?;
            }
        }
        let reference_index = reference_index?;
        let mut moved = paragraph.clone();
        // Without its root's attributes the emptied run goes whole, as the
        // staged route removes it.
        let emptied = &mut moved.runs[*reference_run];
        emptied.extra_xml.clear();
        emptied.extra_xml_positions.clear();
        moved.remove_comment_anchors(&[id]);
        let left = moved.comment_ranges.iter().any(|marker| {
            let (CommentRangeMarker::Start { id: found, .. }
            | CommentRangeMarker::End { id: found, .. }) = marker;
            *found == id
        }) || moved
            .runs
            .iter()
            .any(|run| run.content.iter().any(is_reference));
        if left || moved.runs.len() + 1 != paragraph.runs.len() {
            return None;
        }
        let rebased = |run_index: usize| run_index - usize::from(run_index > reference_index);
        let (start, end) = (rebased(range.start.run_index), rebased(range.end.run_index));
        if start > end {
            return None;
        }
        moved
            .anchor_accepted_range(Some(start), Some(end), RangeAnchor::Comment(id))
            .ok()?;
        // Put the original reference run in place of the one anchoring wrote.
        let written = moved
            .runs
            .iter()
            .enumerate()
            .filter(|(_, run)| run.content.iter().any(is_reference))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let [written] = written.as_slice() else {
            return None;
        };
        moved.runs[*written] = transported;
        Some(Some(moved))
    }

    /// Whether comment `id` is defined, is the root of its thread and has
    /// no reply, read from the comments and extended comments models.
    fn comment_is_root_without_replies(&self, id: i32) -> bool {
        let Some(comment) = self
            .comments
            .as_ref()
            .and_then(|comments| comments.comments.iter().find(|comment| comment.id == id))
        else {
            return false;
        };
        let Some(extended) = self.comments_extended.as_ref() else {
            return true;
        };
        let own = comment
            .paragraph_ids
            .iter()
            .filter_map(Option::as_deref)
            .collect::<HashSet<_>>();
        extended.comments.iter().all(|entry| {
            let parent = entry.para_id_parent.as_deref();
            !(own.contains(entry.para_id.as_str()) && parent.is_some())
                && !parent.is_some_and(|parent| own.contains(parent))
        })
    }
}
