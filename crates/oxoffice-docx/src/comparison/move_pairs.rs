//! Pairing moved content, and the comment replies a comparison cannot carry.

use super::*;

/// Pair each deleted owner with an equal inserted one as a move, except the
/// `unmovable` owners of each side.
pub(super) fn pair_moves(
    aligned: &[(Option<usize>, Option<usize>)],
    original: &[String],
    edited: &[String],
    unmovable: [HashSet<usize>; 2],
    ids: &mut IdAllocator,
) -> Result<MovePairs> {
    let mut pairs = MovePairs {
        original: vec![None; original.len()],
        edited: vec![None; edited.len()],
    };
    let unmatched_edited = aligned
        .iter()
        .filter_map(|(left, right)| left.is_none().then_some(*right).flatten())
        .filter(|right| !unmovable[1].contains(right))
        .collect::<Vec<_>>();
    for left in aligned
        .iter()
        .filter_map(|(left, right)| right.is_none().then_some(*left).flatten())
        .filter(|left| !unmovable[0].contains(left))
    {
        let Some(right) = unmatched_edited
            .iter()
            .copied()
            .find(|right| pairs.edited[*right].is_none() && original[left] == edited[*right])
        else {
            continue;
        };
        let id = ids.allocate()?;
        pairs.original[left] = Some(id);
        pairs.edited[right] = Some(id);
    }
    Ok(pairs)
}

/// Enforce the same whole-owner law on the authored reply-copy producer.
fn refuse_changed_reply_held_owners(
    original: &Document,
    replay: &Document,
    stories: &[StoryPart],
) -> Result<()> {
    fn inner(
        left: &str,
        right: &str,
        left_bindings: &NamespaceBindings,
        right_bindings: &NamespaceBindings,
        location: &str,
    ) -> Result<()> {
        let left_spans = story_content_spans(left, left_bindings)?;
        let right_spans = story_content_spans(right, right_bindings)?;
        let held_left = HeldRevisions::of(left, &left_spans, left_bindings, true)?;
        let held_right = HeldRevisions::of(right, &right_spans, right_bindings, true)?;
        if !held_left
            .owners
            .iter()
            .chain(&held_right.owners)
            .any(|held| *held)
            && !held_left.section
            && !held_right.section
        {
            return Ok(());
        }
        let left_model = story_document(left, left_bindings)?;
        let right_model = story_document(right, right_bindings)?;
        if left_spans.len() != left_model.body.content.len()
            || right_spans.len() != right_model.body.content.len()
        {
            return Err(Error::Other(format!(
                "comparison could not correlate held reply owners at {location}"
            )));
        }
        for index in 0..held_left.owners.len().max(held_right.owners.len()) {
            if !held_left.holds(Some(index)) && !held_right.holds(Some(index)) {
                continue;
            }
            let left_owner = left_model.body.content.get(index);
            let right_owner = right_model.body.content.get(index);
            // The replay changes only the selected graph. Unrelated held
            // owners must stay the same owner, read as the compare path
            // reads one.
            if let Some((left_owner, right_owner)) = left_owner.zip(right_owner)
                && index < left_spans.len()
                && index < right_spans.len()
                && same_held_owner(
                    left_owner == right_owner,
                    held_left.same_owner_source(index, &held_right, index),
                    || {
                        same_owner_tree(
                            Some((left, &left_spans, left_bindings)),
                            index,
                            (right, &right_spans, right_bindings),
                            index,
                        )
                    },
                    || same_written_owner(left_owner, right_owner, left_bindings, right_bindings),
                )?
            {
                continue;
            }
            let owner_location = left_owner.or(right_owner).map_or_else(
                || format!("{location}/owner[{index}]"),
                |owner| body_location(location, owner, index),
            );
            return Err(Error::Other(format!(
                "comparison cannot change {owner_location}, which holds an existing tracked revision: the reply graph changes it"
            )));
        }
        if (held_left.section || held_right.section)
            && (left_model.body.sect_pr != right_model.body.sect_pr
                || !held_left
                    .section_source
                    .as_ref()
                    .zip(held_right.section_source.as_ref())
                    .is_some_and(|(left, right)| left == right))
        {
            return Err(Error::Other(format!(
                "comparison cannot change {location}/section, which holds an existing tracked revision: the reply graph changes it"
            )));
        }
        Ok(())
    }
    fn part(left: &[u8], right: &[u8], kind: ComparisonStoryKind, location: &str) -> Result<()> {
        if left == right {
            return Ok(());
        }
        let left = std::str::from_utf8(left).map_err(utf8_error)?;
        let right = std::str::from_utf8(right).map_err(utf8_error)?;
        if kind == ComparisonStoryKind::Main {
            let (a, a_bindings) = main_body_source(left.as_bytes())?;
            let (b, b_bindings) = main_body_source(right.as_bytes())?;
            return inner(&left[a], &right[b], &a_bindings, &b_bindings, location);
        }
        let a_bindings = root_namespace_bindings(left, kind.root_local(), &[])?;
        let b_bindings = root_namespace_bindings(right, kind.root_local(), &[])?;
        if let Some(local) = kind.owner_local() {
            let a = direct_word_element_spans(left, local)?;
            let b = direct_word_element_spans(right, local)?;
            if a.len() != b.len() {
                return Err(Error::Other(format!(
                    "comparison held reply story shell count changed at {location}"
                )));
            }
            for (index, (a, b)) in a.iter().zip(&b).enumerate() {
                let a = &left[a.clone()];
                let b = &right[b.clone()];
                let a_inner = element_inner_range_any_prefix(a, local)?;
                let b_inner = element_inner_range_any_prefix(b, local)?;
                let a_context = root_namespace_bindings(a, local, &a_bindings)?;
                let b_context = root_namespace_bindings(b, local, &b_bindings)?;
                inner(
                    &a[a_inner],
                    &b[b_inner],
                    &a_context,
                    &b_context,
                    &format!("{location}/{local}[{index}]"),
                )?;
            }
            Ok(())
        } else {
            let a = root_inner_range(left, kind.root_local())?;
            let b = root_inner_range(right, kind.root_local())?;
            inner(&left[a], &right[b], &a_bindings, &b_bindings, location)
        }
    }
    let left = original
        .package
        .get_part(&original.doc_part_name)
        .ok_or_else(|| Error::Other("comparison reply source has no main part".into()))?;
    let right = replay
        .package
        .get_part(&replay.doc_part_name)
        .ok_or_else(|| Error::Other("comparison reply replay has no main part".into()))?;
    part(left, right, ComparisonStoryKind::Main, "body")?;
    for story in stories {
        part(
            story_xml(original, story)?,
            story_xml(replay, story)?,
            story.kind,
            &format!("{}:{}", story.kind.label(), story.part_name),
        )?;
    }
    Ok(())
}

/// Prove a graph-only reply addition through the existing authoring owner.
/// Replaying on a staged original includes its required parent paraId and MC
/// preparation, without inventing another metadata-normalization policy.
pub(super) fn crossing_reply_additions(
    original: &Document,
    edited: &Document,
    options: &ComparisonOptions,
) -> Result<Option<(Vec<i32>, Document)>> {
    let old = original.comments();
    let new = edited.comments();
    let added = new
        .iter()
        .filter(|comment| !old.iter().any(|old| old.id() == comment.id()))
        .collect::<Vec<_>>();
    if added.is_empty()
        || old
            .iter()
            .any(|comment| !new.iter().any(|new| new.id() == comment.id()))
    {
        return Ok(None);
    }
    let mut replay = original.clone_for_staging();
    for comment in &added {
        let (Some(parent), Some(author)) = (comment.parent_id(), comment.author()) else {
            return Ok(None);
        };
        let id = match replay.reply_to_with_initials(
            parent,
            author,
            comment.initials(),
            &comment.text(),
            // Replay the edited reply's own date, or none when it had none.
            comment
                .date()
                .map_or(crate::CommentDate::Undated, crate::CommentDate::At),
        ) {
            Ok(id) => id,
            Err(_) => return Ok(None),
        };
        if id != comment.id() {
            return Ok(None);
        }
    }
    let replay = comparison_input(&replay)?;
    let replay_comments = comment_snapshot(&replay)?;
    let edited_comments = comment_snapshot(edited)?;
    if !comment_snapshots_match(&replay_comments, &edited_comments)?
        || !extended_comment_snapshots_match(&replay_comments, &edited_comments)
        || !comment_package_links_match(&replay_comments, &edited_comments)
    {
        return Ok(None);
    }
    let stories = story_parts_with_options(&replay, options)?
        .into_iter()
        .filter(|story| story.kind != ComparisonStoryKind::Comment)
        .collect::<Vec<_>>();
    let other_stories = story_parts_with_options(edited, options)?
        .into_iter()
        .filter(|story| story.kind != ComparisonStoryKind::Comment)
        .collect::<Vec<_>>();
    if stories != other_stories {
        return Ok(None);
    }
    let markers = comparison_text_box_markers(&replay, edited, &stories, options)?;
    if normalized_package(&replay, &stories, options, &markers)?
        != normalized_package(edited, &other_stories, options, &markers)?
    {
        return Ok(None);
    }
    refuse_changed_reply_held_owners(original, &replay, &stories)?;
    Ok(Some((
        added.into_iter().map(|comment| comment.id()).collect(),
        replay,
    )))
}
