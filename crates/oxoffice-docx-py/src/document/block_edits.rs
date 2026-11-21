//! Edits of one block (one paragraph, or an insertion in one table cell): what they renumber, which owners keep their
//! states, and the debug proof that every kept owner still names its own element.

use super::*;

/// A hyperlink route in its story: the index path of the story item that
/// holds it and its index among that item's hyperlinks.
type LinkRoute = (Vec<usize>, usize);

/// What an edit of one paragraph or one cell renumbers, read from that
/// element before and after the edit
/// ([`oxoffice_docx::Document::element_extent`]).
pub(crate) struct BlockShift {
    /// The element is what it was: publishing would change nothing.
    pub(crate) unchanged: bool,
    /// The edited element's story: kind, part and ordinal.
    story: (oxoffice_docx::StoryKind, String, usize),
    /// The first story item after the element before the edit, and the
    /// change in the number of items inside it.
    #[cfg(debug_assertions)]
    items: (usize, isize),
    /// The hyperlinks of the edited story, by their routes before the edit,
    /// that retire: those inside the element, and those after it whose
    /// route the edit moves.
    retired_links: std::collections::HashSet<LinkRoute>,
    /// The route after the edit of every other hyperlink of the edited story.
    #[cfg(debug_assertions)]
    kept_links: HashMap<LinkRoute, LinkRoute>,
    /// The text box stories of the element's part: the ordinal of the first
    /// one inside the element, how many it holds before the edit, and the
    /// change in that number.
    text_boxes: (usize, usize, isize),
    /// The table cell stories of the element's part, the same way.
    cell_stories: (usize, usize, isize),
    /// The element's bytes before and after the edit.
    #[cfg(debug_assertions)]
    xml: (Vec<u8>, Vec<u8>),
}

impl BlockShift {
    /// The shift between two extents of one element, or `None` when they
    /// disagree outside the element: then the edit changed more than it.
    pub(super) fn of(
        story: &oxoffice_docx::StoryId,
        before: oxoffice_docx::ElementExtent,
        after: oxoffice_docx::ElementExtent,
    ) -> Option<Self> {
        if before.items.0 != after.items.0
            || before.text_boxes.0 != after.text_boxes.0
            || before.cell_stories.0 != after.cell_stories.0
        {
            return None;
        }
        let split = |links: Vec<(Vec<usize>, usize, std::cmp::Ordering)>| {
            let mut parts: [Vec<LinkRoute>; 3] = Default::default();
            for (path, index, place) in links {
                let part = match place {
                    std::cmp::Ordering::Less => 0,
                    std::cmp::Ordering::Equal => 1,
                    std::cmp::Ordering::Greater => 2,
                };
                parts[part].push((path, index));
            }
            parts
        };
        let [old_before, old_inside, old_after] = split(before.links);
        let [new_before, _, new_after] = split(after.links);
        if old_before != new_before || old_after.len() != new_after.len() {
            return None;
        }
        let mut retired_links = old_inside
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        let mut kept_links = old_before
            .into_iter()
            .map(|route| (route.clone(), route))
            .collect::<HashMap<_, _>>();
        for (old, new) in old_after.into_iter().zip(new_after) {
            if old == new {
                kept_links.insert(old, new);
            } else {
                retired_links.insert(old);
            }
        }
        let change = |before: usize, after: usize| {
            Some(isize::try_from(after).ok()? - isize::try_from(before).ok()?)
        };
        Some(Self {
            unchanged: before.xml == after.xml,
            story: (
                story.kind(),
                story.part_name().to_owned(),
                story.owner_index(),
            ),
            #[cfg(debug_assertions)]
            items: (
                before.items.0 + before.items.1,
                change(before.items.1, after.items.1)?,
            ),
            retired_links,
            #[cfg(debug_assertions)]
            kept_links,
            text_boxes: (
                before.text_boxes.0,
                before.text_boxes.1,
                change(before.text_boxes.1, after.text_boxes.1)?,
            ),
            cell_stories: (
                before.cell_stories.0,
                before.cell_stories.1,
                change(before.cell_stories.1, after.cell_stories.1)?,
            ),
            #[cfg(debug_assertions)]
            xml: (before.xml, after.xml),
        })
    }

    fn in_part(&self, story: &oxoffice_docx::StoryId) -> bool {
        story.part_name() == self.story.1
    }

    fn is_edited_story(&self, story: &oxoffice_docx::StoryId) -> bool {
        story.kind() == self.story.0 && self.in_part(story) && story.owner_index() == self.story.2
    }

    /// The first story of `story`'s kind after the element, before the edit,
    /// and the change in the number of such stories inside it, for the text
    /// box and table cell stories of the element's part.
    fn later_stories(&self, story: &oxoffice_docx::StoryId) -> Option<(usize, isize)> {
        if !self.in_part(story) {
            return None;
        }
        let (first, before, change) = match story.kind() {
            oxoffice_docx::StoryKind::TextBox => self.text_boxes,
            oxoffice_docx::StoryKind::TableCell => self.cell_stories,
            _ => return None,
        };
        Some((first + before, change))
    }

    /// Whether `story` lies inside the element (a text box or a table cell
    /// story it holds), by its ordinal before the edit.
    fn story_inside(&self, story: &oxoffice_docx::StoryId) -> bool {
        let (first, before, _) = match story.kind() {
            oxoffice_docx::StoryKind::TextBox => self.text_boxes,
            oxoffice_docx::StoryKind::TableCell => self.cell_stories,
            _ => return false,
        };
        self.in_part(story) && (first..first + before).contains(&story.owner_index())
    }

    /// Whether an owner of `story` other than a hyperlink is renumbered with
    /// its story: a text box after the element, when the edit changes how
    /// many text boxes the element holds. Table cell stories are found
    /// through their table, which the edit does not move.
    fn story_renumbered(&self, story: &oxoffice_docx::StoryId) -> bool {
        story.kind() == oxoffice_docx::StoryKind::TextBox
            && self
                .later_stories(story)
                .is_some_and(|(first, change)| change != 0 && story.owner_index() >= first)
    }

    /// Whether the hyperlink at `location`, index `index`, is retired: inside
    /// the element, or at a route the edit moves (in the edited story, in a
    /// later text box or table cell story).
    fn link_retired(&self, location: &oxoffice_docx::ContentLocation, index: usize) -> bool {
        let story = location.story();
        if self.is_edited_story(story) {
            return self
                .retired_links
                .contains(&(location.index_path().to_vec(), index));
        }
        self.story_inside(story)
            || self
                .later_stories(story)
                .is_some_and(|(first, change)| change != 0 && story.owner_index() >= first)
    }

    /// The story `story` is after the edit.
    #[cfg(debug_assertions)]
    fn moved_story(&self, story: &oxoffice_docx::StoryId) -> oxoffice_docx::StoryId {
        let mut moved =
            oxoffice_docx::StoryId::new(story.kind(), story.part_name(), story.owner_index());
        if let Some((first, change)) = self.later_stories(story)
            && story.owner_index() >= first
        {
            moved.set_owner_index(story.owner_index().saturating_add_signed(change));
        }
        moved
    }

    /// The route `location` names after the edit, for content outside the
    /// element: later story items of the edited story and later stories of
    /// its part are renumbered.
    #[cfg(debug_assertions)]
    fn moved_location(
        &self,
        location: &oxoffice_docx::ContentLocation,
    ) -> oxoffice_docx::ContentLocation {
        let mut path = location.index_path().to_vec();
        if self.is_edited_story(location.story())
            && let Some(head) = path.first_mut()
            && *head >= self.items.0
        {
            *head = head.saturating_add_signed(self.items.1);
        }
        oxoffice_docx::ContentLocation::new(
            self.moved_story(location.story()),
            location.item_kind(),
            path,
        )
    }

    /// The route a kept owner names after the edit.
    #[cfg(debug_assertions)]
    fn moved_owner(&self, location: &WordOwnerLocation) -> Option<WordOwnerLocation> {
        Some(match location {
            WordOwnerLocation::Paragraph(location) => {
                WordOwnerLocation::Paragraph(self.moved_location(location))
            }
            WordOwnerLocation::Table(location) => {
                WordOwnerLocation::Table(self.moved_location(location))
            }
            WordOwnerLocation::Run(location, path) => {
                WordOwnerLocation::Run(self.moved_location(location), path.clone())
            }
            WordOwnerLocation::Cell {
                table,
                row,
                cell,
                story,
            } => WordOwnerLocation::Cell {
                table: self.moved_location(table),
                row: *row,
                cell: *cell,
                story: self.moved_story(story),
            },
            WordOwnerLocation::Section(index) => WordOwnerLocation::Section(*index),
            WordOwnerLocation::Hyperlink(location, index) => {
                if self.is_edited_story(location.story()) {
                    let (path, index) = self
                        .kept_links
                        .get(&(location.index_path().to_vec(), *index))?
                        .clone();
                    WordOwnerLocation::Hyperlink(
                        oxoffice_docx::ContentLocation::new(
                            self.moved_story(location.story()),
                            location.item_kind(),
                            path,
                        ),
                        index,
                    )
                } else {
                    WordOwnerLocation::Hyperlink(self.moved_location(location), *index)
                }
            }
        })
    }
}

/// Which scopes of each owner of `new` keep the state of the owner of `old`
/// it matches, for an edit of the owner `selected` of `old` (decision 1 of
/// the one-block publication spec). The edited paragraph keeps its owner and
/// properties states, a cell its owner state; what the element holds is born
/// again, with every owner the edit renumbers; every other matched owner
/// keeps its states, unless its parent does not.
pub(super) fn block_retention(
    old: &[WordOwner],
    new: &[WordOwnerAddress],
    matches: &[Option<usize>],
    selected: usize,
    shift: &BlockShift,
) -> Vec<Vec<bool>> {
    let inside = |mut index: usize| {
        while let Some(parent) = old[index].parent {
            if parent == selected {
                return true;
            }
            index = parent;
        }
        false
    };
    let renumbered = |owner: &WordOwner| match &owner.location {
        WordOwnerLocation::Paragraph(location)
        | WordOwnerLocation::Table(location)
        | WordOwnerLocation::Run(location, _) => {
            shift.story_inside(location.story()) || shift.story_renumbered(location.story())
        }
        WordOwnerLocation::Cell { table, story, .. } => {
            shift.story_inside(story)
                || shift.story_inside(table.story())
                || shift.story_renumbered(table.story())
        }
        WordOwnerLocation::Hyperlink(location, index) => shift.link_retired(location, *index),
        WordOwnerLocation::Section(_) => false,
    };
    let mut keep: Vec<Option<Vec<bool>>> = vec![None; new.len()];
    for target in 0..new.len() {
        let mut stack = vec![target];
        while let Some(&index) = stack.last() {
            if keep[index].is_some() {
                stack.pop();
                continue;
            }
            let parent_kept = match new[index].parent {
                Some(parent) => match &keep[parent] {
                    Some(slots) => slots[0],
                    None => {
                        stack.push(parent);
                        continue;
                    }
                },
                None => true,
            };
            stack.pop();
            let slots = new[index].location.scopes().len();
            keep[index] = Some(match matches[index] {
                Some(before) if parent_kept && before == selected => {
                    match &old[before].location {
                        // A text-box paragraph is rewritten through its
                        // story: all its states are born again.
                        WordOwnerLocation::Paragraph(location)
                            if location.story().kind() == oxoffice_docx::StoryKind::TextBox =>
                        {
                            vec![false; slots]
                        }
                        // The paragraph keeps its owner and properties.
                        WordOwnerLocation::Paragraph(_) => vec![true, false, true],
                        // The cell keeps its owner.
                        _ => (0..slots).map(|slot| slot == 0).collect(),
                    }
                }
                Some(before) if parent_kept && !inside(before) && !renumbered(&old[before]) => {
                    vec![true; slots]
                }
                _ => vec![false; slots],
            });
        }
    }
    keep.into_iter().map(Option::unwrap).collect()
}

impl PyDocument {
    /// What the edit that made `candidate` from this document renumbers, for
    /// a block publication, or `None` when the element cannot be read
    /// alone in both, or the two disagree outside it.
    pub(crate) fn block_shift(
        &mut self,
        py: Python<'_>,
        candidate: &oxoffice_docx::Document,
        publication: &WordPublication,
    ) -> PyResult<Option<BlockShift>> {
        if !matches!(
            publication,
            WordPublication::ParagraphEdit(_) | WordPublication::CellInsert(..)
        ) {
            return Ok(None);
        }
        // The cell's story is read from the inventory.
        self.ensure_word_authority(py)?;
        let (story, path) = match publication {
            WordPublication::ParagraphEdit(location) => {
                (location.story().clone(), Some(location.index_path()))
            }
            WordPublication::CellInsert(table, row, cell) => {
                let Some(story) = self.owners.iter().find_map(|owner| match &owner.location {
                    WordOwnerLocation::Cell {
                        table: owned,
                        row: r,
                        cell: c,
                        story,
                    } if same_content_owner(owned, table) && r == row && c == cell => {
                        Some(story.clone())
                    }
                    _ => None,
                }) else {
                    return Ok(None);
                };
                (story, None)
            }
            _ => return Ok(None),
        };
        let read = |document: &oxoffice_docx::Document| {
            document
                .element_extent(&story, path)
                .map_err(|error| docx_to_pyerr(py, error))
        };
        Ok(match (read(&self.inner)?, read(candidate)?) {
            (Some(before), Some(after)) => BlockShift::of(&story, before, after),
            _ => None,
        })
    }

    /// Publish `candidate`, this document after one block edit, unless the
    /// edit left the element as it was: then nothing is published and every
    /// handle stays.
    pub(crate) fn publish_block_edit(
        &mut self,
        py: Python<'_>,
        candidate: oxoffice_docx::Document,
        operation: &'static str,
        publication: WordPublication,
    ) -> PyResult<()> {
        let shift = self.block_shift(py, &candidate, &publication)?;
        if shift.as_ref().is_some_and(|shift| shift.unchanged) {
            return Ok(());
        }
        let prepared =
            self.prepare_shifted_revision(py, &candidate, operation, publication, shift)?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }
}

/// Under debug assertions, prove that every owner of `new` that keeps the
/// owner state of an owner of `old` names the same element after the edit
/// that made `after` from `before`: at the old route moved by the edit's
/// shift, with the same bytes. The element the edit rewrote keeps its
/// route, and so does every owner whose element holds it (the cells and
/// tables around it, the paragraph and run that hold its text box), whose
/// bytes differ by that element alone.
#[cfg(debug_assertions)]
pub(super) fn assert_kept_owners_name_their_elements(
    (before, old): (&oxoffice_docx::Document, &[WordOwner]),
    (after, new): (&oxoffice_docx::Document, &[WordOwner]),
    shift: &BlockShift,
    selected: &RevisionState,
) {
    type Key = (oxoffice_docx::StoryKind, String, usize, Vec<usize>);
    let key = |location: &oxoffice_docx::ContentLocation| -> Key {
        let story = location.story();
        (
            story.kind(),
            story.part_name().to_owned(),
            story.owner_index(),
            location.index_path().to_vec(),
        )
    };
    // The bytes as the parts hold them, and each paragraph closed over its
    // namespaces to read its runs. A rewritten text-box paragraph may
    // declare namespaces of its own, which changes how the paragraph that
    // holds it closes, not its bytes outside the text box.
    struct Read {
        paragraphs: HashMap<Key, Vec<u8>>,
        elements: oxoffice_docx::OwnerElementXml,
    }
    let read = |document: &oxoffice_docx::Document| {
        let (_, _, paragraphs, _) = document
            .story_owner_snapshots()
            .expect("a published document has owner snapshots");
        Read {
            paragraphs: paragraphs
                .into_iter()
                .map(|(location, xml)| (key(&location), xml))
                .collect(),
            elements: document
                .owner_element_xml()
                .expect("a published document has owner element bytes"),
        }
    };
    let (old_read, new_read) = (read(before), read(after));
    // Elements compare by their resolved events: each name with its
    // namespace, the namespace declarations aside, which rewriting a
    // text-box paragraph moves onto the run that holds the drawing.
    let events = |xml: &[u8]| {
        oxoffice_docx::resolved_xml_events(xml).expect("an owner element is well formed XML")
    };
    let (edited_before, edited_after) = (events(&shift.xml.0), events(&shift.xml.1));
    // Equal, or equal but for the edited element, at one place in both.
    let holds_edit = |old: &[u8], new: &[u8]| {
        let (old, new) = (events(old), events(new));
        if old == new {
            return true;
        }
        let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let suffix = old
            .iter()
            .rev()
            .zip(new.iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let first = old.len().saturating_sub(edited_before.len() + suffix);
        old.len() + edited_after.len() == new.len() + edited_before.len()
            && (first..=prefix.min(old.len())).any(|at| {
                old.get(at..at + edited_before.len()) == Some(edited_before.as_slice())
                    && new.get(at..at + edited_after.len()) == Some(edited_after.as_slice())
            })
    };
    let same = |old: Option<&Vec<u8>>, new: Option<&Vec<u8>>, what: &str| {
        let (Some(old), Some(new)) = (old, new) else {
            panic!("kept {what} has no element before or after the edit");
        };
        assert!(
            holds_edit(old, new),
            "kept {what} names another element after the edit"
        );
    };
    let story_key = |story: &oxoffice_docx::StoryId| {
        (
            story.kind(),
            story.part_name().to_owned(),
            story.owner_index(),
        )
    };
    let by_generation = old
        .iter()
        .enumerate()
        .map(|(index, owner)| (owner.owner.generation(), index))
        .collect::<HashMap<_, _>>();
    for owner in new {
        let Some(&index) = by_generation.get(&owner.owner.generation()) else {
            continue;
        };
        let previous = &old[index];
        let expected = if owner.owner.same_state(selected) {
            Some(previous.location.clone())
        } else {
            shift.moved_owner(&previous.location)
        };
        assert_eq!(
            expected.as_ref().map(unfingerprinted_owner_location),
            Some(unfingerprinted_owner_location(&owner.location)),
            "kept owner route after a block edit"
        );
        if owner.owner.same_state(selected) {
            continue;
        }
        match (&previous.location, &owner.location) {
            (WordOwnerLocation::Paragraph(a), WordOwnerLocation::Paragraph(b)) => same(
                old_read.elements.paragraphs.get(&key(a)),
                new_read.elements.paragraphs.get(&key(b)),
                "paragraph",
            ),
            (WordOwnerLocation::Table(a), WordOwnerLocation::Table(b)) => same(
                old_read.elements.items.get(&key(a)),
                new_read.elements.items.get(&key(b)),
                "table",
            ),
            (
                WordOwnerLocation::Cell { story: a, .. },
                WordOwnerLocation::Cell { story: b, .. },
            ) => {
                same(
                    old_read.elements.stories.get(&story_key(a)),
                    new_read.elements.stories.get(&story_key(b)),
                    "cell",
                );
            }
            (WordOwnerLocation::Run(a, path), WordOwnerLocation::Run(b, _)) => {
                let (old_paragraph, new_paragraph) = (
                    old_read.paragraphs.get(&key(a)),
                    new_read.paragraphs.get(&key(b)),
                );
                let run = |xml: Option<&Vec<u8>>| {
                    xml.and_then(|xml| oxoffice_docx_oxml::CT_P::from_xml_fragment(xml).ok())
                        .and_then(|paragraph| paragraph.accepted_run(path).cloned())
                };
                let (old_run, new_run) = (run(old_paragraph), run(new_paragraph));
                // A run that holds the edited text box differs by it alone,
                // as the paragraph that holds the run does.
                assert!(
                    old_run.is_some() && old_run == new_run
                        || old_read
                            .elements
                            .paragraphs
                            .get(&key(a))
                            .zip(new_read.elements.paragraphs.get(&key(b)))
                            .is_some_and(|(old, new)| holds_edit(old, new)),
                    "kept run names another element after the edit"
                );
            }
            (WordOwnerLocation::Hyperlink(a, ai), WordOwnerLocation::Hyperlink(b, bi)) => {
                let link = |location: &oxoffice_docx::ContentLocation, index: usize| {
                    let (kind, part, ordinal) = story_key(location.story());
                    (kind, part, ordinal, location.index_path().to_vec(), index)
                };
                same(
                    old_read.elements.links.get(&link(a, *ai)),
                    new_read.elements.links.get(&link(b, *bi)),
                    "hyperlink",
                );
            }
            (WordOwnerLocation::Section(a), WordOwnerLocation::Section(b)) => assert_eq!(
                before.section_properties_xml(*a).ok(),
                after.section_properties_xml(*b).ok(),
                "kept section names another element after the edit"
            ),
            _ => panic!("kept owner changed kind after a block edit"),
        }
    }
}
