//! The body story route of a facade paragraph, at any block-control depth.
//!
//! The story model gives every block content control of the body a story
//! item of its own, the nested ones too, in document order, and a paragraph
//! of a control the path `[control item, ordinal]`, its ordinal counting
//! that control's direct paragraphs only (`story_range_paragraph_spans`).
//! The facade model nests controls in their parents instead. A direct body
//! control and the block controls it nests are matched, in document order,
//! to the direct item of its slot and the block content control items that
//! the scan lists inside it; the two lists must have the same length, or the
//! paragraph has no route.

use super::*;

/// Where a body paragraph sits in the facade model: its direct body slot,
/// then, for a paragraph of a block control, the document-order index of its
/// control among the direct control and the block controls it nests (0 for
/// the direct control) and its ordinal among that control's direct
/// paragraphs.
type ParagraphTarget = (usize, Option<(usize, usize)>);

/// The direct paragraphs of `control` and of the block controls it nests,
/// with their control's document-order index under the first control
/// (counted by `next`) and their ordinal in it. Tables are cell stories of
/// their own, and their paragraphs are not listed.
fn control_paragraphs<'a>(
    control: &'a CT_Sdt,
    next: &mut usize,
    found: &mut Vec<(&'a CT_P, usize, usize)>,
) {
    let index = *next;
    *next += 1;
    let mut ordinal = 0;
    for child in &control.content {
        match child {
            SdtContent::Paragraph(paragraph) => {
                found.push((paragraph, index, ordinal));
                ordinal += 1;
            }
            SdtContent::ContentControl(nested) => control_paragraphs(nested, next, found),
            _ => {}
        }
    }
}

/// The story item indexes of the direct body control at `item` and of the
/// block content controls the scan lists inside it, in document order.
pub(super) fn control_items(items: &[StoryItemSpan], item: usize) -> Vec<usize> {
    let end = items[item].full.end;
    std::iter::once(item)
        .chain(
            items
                .iter()
                .enumerate()
                .skip(item + 1)
                .take_while(|(_, nested)| nested.full.start < end)
                .filter(|(_, nested)| {
                    nested.kind == StoryItemKind::ContentControl
                        && nested.sdt_context == Some(StorySdtContext::Block)
                })
                .map(|(index, _)| index),
        )
        .collect()
}

/// The route of a paragraph at `target`, given the scan's direct items and,
/// for a control, the number of controls the facade model counts under it.
fn paragraph_route(
    items: &[StoryItemSpan],
    direct: &[usize],
    (slot, control): ParagraphTarget,
    controls: usize,
) -> Option<Vec<usize>> {
    let item_index = *direct.get(slot)?;
    let item = &items[item_index];
    let Some((nested, ordinal)) = control else {
        return (item.kind == StoryItemKind::Paragraph).then(|| vec![item_index]);
    };
    if item.kind != StoryItemKind::ContentControl {
        return None;
    }
    // A paragraph directly in the direct control needs no nested match.
    if nested == 0 {
        return Some(vec![item_index, ordinal]);
    }
    let listed = control_items(items, item_index);
    (listed.len() == controls).then(|| vec![listed[nested], ordinal])
}

impl Document {
    /// Return the checked body story location of the paragraph addressed by
    /// `paragraph_index`, the index of [`Self::paragraph_mut`].
    ///
    /// A direct body paragraph has its one-segment story item path. A
    /// paragraph inside a block content control, at any depth, has a
    /// two-segment path: the story item index of its innermost control, then
    /// the paragraph's position among that control's own paragraphs, the
    /// route [`Self::story_paragraphs`] gives it. Comment operations and
    /// scoped literal replacement accept this form. Returns `None` when the
    /// index is out of range, the paragraph belongs to a table, or the
    /// controls the source lists do not match the modeled ones.
    pub fn paragraph_story_location(
        &self,
        paragraph_index: usize,
    ) -> Result<Option<ContentLocation>> {
        let Some(paragraph) = self.paragraph(paragraph_index) else {
            return Ok(None);
        };
        let selected = paragraph.inner;
        // A facade handle counts recursively; the story route names the
        // innermost control and the paragraph's ordinal in it.
        let mut controls = 0;
        let target = self
            .document
            .body
            .content
            .iter()
            .enumerate()
            .find_map(|(slot, child)| match child {
                BodyContent::Paragraph(paragraph) if std::ptr::eq(paragraph.as_ref(), selected) => {
                    Some((slot, None))
                }
                BodyContent::ContentControl(control) => {
                    let mut found = Vec::new();
                    let mut next = 0;
                    control_paragraphs(control, &mut next, &mut found);
                    let route = found
                        .into_iter()
                        .find(|(paragraph, _, _)| std::ptr::eq(*paragraph, selected))
                        .map(|(_, nested, ordinal)| (slot, Some((nested, ordinal))));
                    controls = next;
                    route
                }
                _ => None,
            });
        let Some(target) = target else {
            return Ok(None);
        };
        let story = self
            .stories()?
            .into_iter()
            .find(|story| story.kind == StoryKind::Body)
            .ok_or_else(|| Error::Other("document body story is missing".to_owned()))?;
        // Preserve the complete story-source validation before selecting the
        // incremental direct-body route, including unrelated malformed parts.
        if target.1.is_none() {
            return self.direct_body_item_location(target.0).map(|location| {
                location.filter(|location| location.item_kind == StoryItemKind::Paragraph)
            });
        }
        let (source, owner) = self.story_source_and_owner(&story)?;
        let items = scan_story_items(source.xml.as_ref(), &owner, None)?;
        // The body section properties come last, so the n-th direct item is
        // the direct body child at slot n.
        let direct = direct_item_indexes(&items);
        let location = paragraph_route(&items, &direct, target, controls)
            .map(|path| ContentLocation::new(story, StoryItemKind::Paragraph, path));
        #[cfg(debug_assertions)]
        if let Some(location) = &location {
            let spans = story_range_paragraph_spans(
                source.xml.as_ref(),
                &[(location.story.clone(), owner)],
            )?;
            assert!(
                spans[0].iter().any(|(listed, _)| listed == location),
                "paragraph {paragraph_index}: route {:?} is no story paragraph",
                location.index_path
            );
        }
        Ok(location)
    }

    /// Resolve every body paragraph at once, in [`Self::paragraphs`] order.
    ///
    /// Entry `i` is what [`Self::paragraph_story_location`] returns for `i`
    /// when that is a location. `None` marks a paragraph that the single
    /// lookup does not resolve, or resolves to an error; callers that cache
    /// this inventory ask the single lookup for those. One call costs one
    /// scan of the body, where a single lookup costs one scan per paragraph.
    #[doc(hidden)]
    pub fn paragraph_story_locations(&self) -> Result<Vec<Option<ContentLocation>>> {
        let story = self
            .stories()?
            .into_iter()
            .find(|story| story.kind == StoryKind::Body)
            .ok_or_else(|| Error::Other("document body story is missing".to_owned()))?;
        let (source, owner) = self.story_source_and_owner(&story)?;
        let items = scan_story_items(source.xml.as_ref(), &owner, None)?;
        let locations = body_paragraph_routes(&self.document.body, &items)
            .into_iter()
            .map(|path| {
                path.map(|path| ContentLocation::new(story.clone(), StoryItemKind::Paragraph, path))
            })
            .collect::<Vec<_>>();
        #[cfg(debug_assertions)]
        {
            // The resolved routes are story paragraphs, in document order.
            let mut spans =
                story_range_paragraph_spans(source.xml.as_ref(), &[(story.clone(), owner)])?
                    .remove(0);
            spans.sort_by_key(|(_, span)| span.start);
            let mut listed = spans.iter().map(|(location, _)| location);
            for location in locations.iter().flatten() {
                assert!(
                    listed.any(|listed| listed == location),
                    "route {:?} is no story paragraph in document order",
                    location.index_path
                );
            }
        }
        Ok(locations)
    }
}

/// The body story path of each paragraph of `body`, in
/// [`CT_Body::paragraphs`] order, as [`Document::paragraph_story_location`]
/// resolves it from the body story items `items`, or `None` where it gives
/// no location.
pub(super) fn body_paragraph_routes(
    body: &CT_Body,
    items: &[StoryItemSpan],
) -> Vec<Option<Vec<usize>>> {
    let mut targets = HashMap::new();
    let mut controls = HashMap::new();
    let mut found = Vec::new();
    for (slot, child) in body.content.iter().enumerate() {
        match child {
            BodyContent::Paragraph(paragraph) => {
                targets
                    .entry(paragraph.as_ref() as *const CT_P)
                    .or_insert((slot, None));
            }
            BodyContent::ContentControl(control) => {
                let mut next = 0;
                control_paragraphs(control, &mut next, &mut found);
                controls.insert(slot, next);
                for (paragraph, nested, ordinal) in found.drain(..) {
                    targets
                        .entry(paragraph as *const CT_P)
                        .or_insert((slot, Some((nested, ordinal))));
                }
            }
            _ => {}
        }
    }
    let direct = direct_item_indexes(items);
    body.paragraphs()
        .map(|paragraph| {
            let target = *targets.get(&(paragraph as *const CT_P))?;
            let count = controls.get(&target.0).copied().unwrap_or(0);
            paragraph_route(items, &direct, target, count)
        })
        .collect()
}

/// The indexes of the scan's direct owner children, in order.
fn direct_item_indexes(items: &[StoryItemSpan]) -> Vec<usize> {
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.direct_owner_child)
        .map(|(index, _)| index)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: StoryItemKind, full: Range<usize>, direct: bool) -> StoryItemSpan {
        StoryItemSpan {
            kind,
            scan: full.clone(),
            full,
            direct_owner_child: direct,
            complex_field: false,
            complex_ancestors: Vec::new(),
            sdt_context: Some(StorySdtContext::Block),
        }
    }

    #[test]
    fn a_paragraph_of_the_direct_control_keeps_its_route_when_nested_controls_do_not_match() {
        // A direct paragraph, then a direct control holding one block control
        // the scan lists, where the model counts two nested controls.
        let items = [
            item(StoryItemKind::Paragraph, 0..10, true),
            item(StoryItemKind::ContentControl, 10..100, true),
            item(StoryItemKind::ContentControl, 20..40, false),
        ];
        let direct = direct_item_indexes(&items);
        assert_eq!(
            paragraph_route(&items, &direct, (1, Some((0, 2))), 3),
            Some(vec![1, 2])
        );
        assert_eq!(paragraph_route(&items, &direct, (1, Some((1, 0))), 3), None);
        assert_eq!(
            paragraph_route(&items, &direct, (1, Some((1, 0))), 2),
            Some(vec![2, 0])
        );
    }
}
