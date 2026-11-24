//! Splicing the body: splice plans, prepared splices, and the owners they move.

use super::*;

/// One direct body item that a splice inserts, with what the owner
/// inventory reads for it.
pub(crate) enum SplicedBodyItem {
    /// A plain paragraph's accepted text and run paths.
    Paragraph(String, Vec<oxoffice_docx_oxml::text::AcceptedRunPath>),
    /// A table's namespace-complete XML and its owner snapshots in a body
    /// that holds only that table.
    Table(Vec<u8>, WordOwnerSnapshots),
}

/// An appended paragraph whose publication is ready: what remains cannot
/// fail before the paragraph reaches the document.
pub(crate) enum StagedAppend {
    /// A prepared body splice and the handle of the paragraph it appends.
    Splice(Box<PreparedBodySplice>, Py<PyParagraph>),
    /// A candidate that holds the paragraph, its revision committed, and the
    /// paragraph's handle.
    Revision(Box<oxoffice_docx::Document>, Py<PyParagraph>),
}

/// Plain direct body paragraphs removed from a body slot, or one item
/// inserted at it.
pub(crate) struct BodySplice {
    pub(crate) slot: usize,
    pub(crate) removed: usize,
    pub(crate) inserted: Option<SplicedBodyItem>,
}

/// A splice of the current owner inventory for one direct body edit.
///
/// The inserted item is one body story item, a plain paragraph or a table
/// with no nested table, text box or link, and the removed items are plain
/// paragraphs. So only body routes after the slot move, by the items inserted
/// less those removed, and only the cell stories after an inserted table,
/// by its cells. Every other owner keeps its place in the inventory.
pub(crate) struct BodySplicePlan {
    slot: usize,
    removed: usize,
    // The body story item index of the splice slot.
    first: usize,
    // The inserted owners, in blocks that each precede a current inventory
    // index, with next-inventory parents.
    blocks: Vec<(usize, Vec<WordOwnerAddress>)>,
    // The current inventory range of the removed paragraphs and their runs.
    removed_owners: std::ops::Range<usize>,
    // Cell story owners from the first index on move by the second.
    cells: (usize, usize),
    inserted_paragraph: bool,
    inserted_table: bool,
    // The facade paragraph and table indexes of the splice slot.
    pub(crate) paragraph: usize,
    pub(crate) table: usize,
    // The inserted table's route and the routes of its cell paragraphs.
    table_entry: Option<(oxoffice_docx::ContentLocation, Vec<CellParagraphs>)>,
    piece: Option<WordBlockOwners>,
    kept_scopes: usize,
}

/// The facade coordinates of a cell and the routes of its paragraphs.
type CellParagraphs = ((usize, usize), Vec<oxoffice_docx::ContentLocation>);

impl BodySplicePlan {
    fn added(&self) -> usize {
        self.blocks.iter().map(|(_, block)| block.len()).sum()
    }

    /// The body story item index that `item` moves to.
    fn moved(&self, item: usize) -> usize {
        if item >= self.first + self.removed {
            item + usize::from(self.inserted_paragraph || self.inserted_table) - self.removed
        } else {
            item
        }
    }

    /// The next inventory index of current owner `index`, unless removed.
    fn next_index(&self, index: usize) -> Option<usize> {
        if self.removed_owners.contains(&index) {
            return None;
        }
        let mut next = index;
        if index >= self.removed_owners.end {
            next -= self.removed_owners.len();
        }
        for (at, block) in &self.blocks {
            if index >= *at {
                next += block.len();
            }
        }
        Some(next)
    }

    /// The next inventory, in order: kept current owners and inserted ones.
    fn next_owners(&self, current: usize) -> impl Iterator<Item = NextWordOwner<'_>> + '_ {
        (0..=current).flat_map(move |index| {
            self.blocks
                .iter()
                .filter(move |(at, _)| *at == index)
                .flat_map(|(_, block)| block.iter().map(NextWordOwner::Inserted))
                .chain(
                    (index < current && !self.removed_owners.contains(&index))
                        .then_some(NextWordOwner::Kept(index)),
                )
        })
    }

    /// Move the cell stories of a kept owner's routes.
    fn move_cell_stories(&self, location: &mut WordOwnerLocation, part: &str) {
        let (first, count) = self.cells;
        if count == 0 {
            return;
        }
        let route = |location: &mut oxoffice_docx::ContentLocation| {
            let story = location.story();
            if story.kind() == oxoffice_docx::StoryKind::TableCell
                && story.part_name() == part
                && story.owner_index() >= first
            {
                let mut story = story.clone();
                story.set_owner_index(story.owner_index() + count);
                *location = oxoffice_docx::ContentLocation::new(
                    story,
                    location.item_kind(),
                    location.index_path().to_vec(),
                );
            }
        };
        match location {
            WordOwnerLocation::Paragraph(location)
            | WordOwnerLocation::Table(location)
            | WordOwnerLocation::Run(location, _)
            | WordOwnerLocation::Hyperlink(location, _) => route(location),
            WordOwnerLocation::Cell { table, story, .. } => {
                route(table);
                if story.kind() == oxoffice_docx::StoryKind::TableCell
                    && story.part_name() == part
                    && story.owner_index() >= first
                {
                    story.set_owner_index(story.owner_index() + count);
                }
            }
            WordOwnerLocation::Section(_) => {}
        }
    }
}

/// One owner of the next inventory.
enum NextWordOwner<'a> {
    Kept(usize),
    Inserted(&'a WordOwnerAddress),
}

/// A prepared body splice: its revision and the scope states of the next
/// inventory, in order.
pub(crate) struct PreparedBodySplice {
    pub(crate) revision: PreparedRevision,
    pub(crate) plan: BodySplicePlan,
    states: Vec<RevisionState>,
    inserted: Vec<WordOwner>,
    // The index in `states` of the first inserted owner's scopes.
    inserted_state: usize,
    snapshot_scope: RevisionState,
    // The document and the inventory before the native edit, for the
    // identity oracle of the kept owners.
    #[cfg(debug_assertions)]
    before: Box<(oxoffice_docx::Document, Vec<WordOwner>)>,
}

/// The body story item index of an owner's content route.
fn word_owner_body_item(location: &WordOwnerLocation) -> Option<usize> {
    let route = match location {
        WordOwnerLocation::Paragraph(location)
        | WordOwnerLocation::Table(location)
        | WordOwnerLocation::Run(location, _)
        | WordOwnerLocation::Hyperlink(location, _) => location,
        WordOwnerLocation::Cell { table, .. } => table,
        WordOwnerLocation::Section(_) => return None,
    };
    (route.story().kind() == oxoffice_docx::StoryKind::Body)
        .then(|| route.index_path().first().copied())
        .flatten()
}

/// Move the body story item index of an owner's content route.
fn move_word_owner_body_item(location: &mut WordOwnerLocation, item: usize) {
    match location {
        WordOwnerLocation::Paragraph(location)
        | WordOwnerLocation::Table(location)
        | WordOwnerLocation::Run(location, _)
        | WordOwnerLocation::Hyperlink(location, _)
        | WordOwnerLocation::Cell {
            table: location, ..
        } => location.set_index_path_head(item),
        WordOwnerLocation::Section(_) => {}
    }
}

/// Binary search over projection records: the index of the first one `pred`
/// rejects, as `slice::partition_point` finds it.
fn location_partition_point(
    items: &[Option<oxoffice_docx::ContentLocation>],
    mut pred: impl FnMut(&Option<oxoffice_docx::ContentLocation>) -> bool,
) -> usize {
    let mut left = 0;
    let mut right = items.len();
    while left < right {
        let mid = left + (right - left) / 2;
        if pred(&items[mid]) {
            left = mid + 1;
        } else {
            right = mid;
        }
    }
    left
}

/// The story of an owner's content route.
fn word_owner_story(location: &WordOwnerLocation) -> Option<&oxoffice_docx::StoryId> {
    match location {
        WordOwnerLocation::Paragraph(location)
        | WordOwnerLocation::Table(location)
        | WordOwnerLocation::Run(location, _)
        | WordOwnerLocation::Hyperlink(location, _) => Some(location.story()),
        WordOwnerLocation::Cell { table, .. } => Some(table.story()),
        WordOwnerLocation::Section(_) => None,
    }
}

impl PyDocument {
    /// Plan `splice` from the current inventory alone. `None` keeps the
    /// rebuild for every case this does not prove, and debug builds check
    /// each committed splice against the rebuild.
    pub(crate) fn plan_body_splice(
        &mut self,
        py: Python<'_>,
        splice: BodySplice,
    ) -> PyResult<Option<BodySplicePlan>> {
        self.ensure_word_authority(py)?;
        let Some(body_paragraphs) = &self.indexes.body_paragraphs else {
            return Ok(None);
        };
        let content_count = self.inner.content_count();
        let Some(end_slot) = splice.slot.checked_add(splice.removed) else {
            return Ok(None);
        };
        let inserted_table = matches!(splice.inserted, Some(SplicedBodyItem::Table(..)));
        if end_slot > content_count || (splice.removed == 0) == splice.inserted.is_none() {
            return Ok(None);
        }
        let mut tables_indexed = true;
        if inserted_table {
            for location in &self.indexes.body_tables {
                if location.is_none() {
                    tables_indexed = false;
                    break;
                }
            }
        }
        if !tables_indexed
            || !(splice.slot..end_slot).all(|slot| self.inner.plain_removable_paragraph(slot))
        {
            return Ok(None);
        }
        let mut story = None;
        let mut current_slot_item = None;
        let mut previous_slot_item = None;
        let mut kept_scopes = 0;
        let mut family = 0;
        let mut table = None;
        let mut paragraph = None;
        let mut body_item = Some(0);
        for (index, owner) in self.owners.iter().enumerate() {
            kept_scopes += owner.location.scopes().len();
            let (next, parent) = match &owner.location {
                WordOwnerLocation::Table(_) => {
                    table = Some(index);
                    (0, owner.parent)
                }
                WordOwnerLocation::Cell { .. } => (0, table),
                WordOwnerLocation::Paragraph(location) => {
                    paragraph = Some(index);
                    if location.story().kind() == oxoffice_docx::StoryKind::Body {
                        let item = location.index_path().first().copied();
                        if body_item.is_none() || item < body_item {
                            return Ok(None);
                        }
                        body_item = item;
                    } else {
                        body_item = None;
                    }
                    (1, owner.parent)
                }
                WordOwnerLocation::Run(..) => (1, paragraph),
                WordOwnerLocation::Section(_) => (2, None),
                WordOwnerLocation::Hyperlink(..) => (3, None),
            };
            let child = matches!(
                owner.location,
                WordOwnerLocation::Cell { .. } | WordOwnerLocation::Run(..)
            );
            if next < family || (child && (parent.is_none() || owner.parent != parent)) {
                return Ok(None);
            }
            family = next;
            if let WordOwnerLocation::Paragraph(location) | WordOwnerLocation::Table(location) =
                &owner.location
                && location.story().kind() == oxoffice_docx::StoryKind::Body
            {
                story.get_or_insert(location.story());
                if let (Some(slot), Some(item)) = (owner.body_slot, location.index_path().first()) {
                    if slot == splice.slot {
                        current_slot_item.get_or_insert(*item);
                    }
                    if splice.slot > 0 && slot == splice.slot - 1 {
                        previous_slot_item.get_or_insert(*item);
                    }
                }
            }
        }
        let Some(story) = story.cloned() else {
            return Ok(None);
        };
        let part = story.part_name().to_owned();
        let first = if let Some(item) = current_slot_item {
            item
        } else if splice.removed == 0 && splice.slot > 0 {
            let Some(previous) = previous_slot_item else {
                return Ok(None);
            };
            let Some(count) = self
                .inner
                .direct_body_story_item_count(splice.slot - 1)
                .ok()
                .flatten()
            else {
                return Ok(None);
            };
            previous + count
        } else {
            return Ok(None);
        };
        // Each removed slot is the direct paragraph owner at its own item.
        for offset in 0..splice.removed {
            let Some(owner) = self
                .indexes
                .direct_body_paragraphs
                .get(&(splice.slot + offset))
                .and_then(|index| self.owners.get(*index))
            else {
                return Ok(None);
            };
            match &owner.location {
                WordOwnerLocation::Paragraph(location)
                    if location.index_path() == [first + offset] => {}
                _ => return Ok(None),
            }
        }
        let Some(cells_before) = (if inserted_table {
            self.cell_stories_before(first, &part)
        } else {
            Some(0)
        }) else {
            return Ok(None);
        };
        let removed_items = first..first + splice.removed;
        let mut removed_owners = None::<std::ops::Range<usize>>;
        let mut paragraph_at = None;
        let mut table_at = None;
        let mut cells_at = None;
        let mut paragraph_ordinal = 0;
        let mut table_ordinal = 0;
        for (index, owner) in self.owners.iter().enumerate() {
            let item = word_owner_body_item(&owner.location);
            let story = word_owner_story(&owner.location);
            let body = story.is_some_and(|story| story.kind() == oxoffice_docx::StoryKind::Body);
            let (paragraph, table) = match &owner.location {
                WordOwnerLocation::Paragraph(_) => (true, false),
                WordOwnerLocation::Table(_) => (false, true),
                _ => (false, false),
            };
            let later = matches!(
                owner.location,
                WordOwnerLocation::Section(_) | WordOwnerLocation::Hyperlink(..)
            );
            if paragraph_at.is_none() && (later || (paragraph && !(body && item < Some(first)))) {
                paragraph_at = Some(index);
            }
            if table_at.is_none()
                && (later
                    || paragraph
                    || matches!(owner.location, WordOwnerLocation::Run(..))
                    || (table && !(body && item < Some(first))))
            {
                table_at = Some(index);
            }
            let later_cell_story = story.is_some_and(|story| {
                story.part_name() != part
                    || (story.kind() == oxoffice_docx::StoryKind::TableCell
                        && story.owner_index() >= cells_before)
            });
            if cells_at.is_none() && (later || (paragraph && later_cell_story)) {
                cells_at = Some(index);
            }
            if item.is_some_and(|item| removed_items.contains(&item)) {
                // Only the removed paragraphs and their runs, contiguously.
                let contiguous = removed_owners
                    .as_ref()
                    .is_none_or(|range| range.end == index);
                if !contiguous
                    || !matches!(
                        owner.location,
                        WordOwnerLocation::Paragraph(_) | WordOwnerLocation::Run(..)
                    )
                {
                    return Ok(None);
                }
                removed_owners =
                    Some(removed_owners.map_or(index..index + 1, |range| range.start..index + 1));
                continue;
            }
            if owner.parent.is_some_and(|parent| {
                removed_owners
                    .as_ref()
                    .is_some_and(|range| range.contains(&parent))
            }) {
                return Ok(None);
            }
            if body && item < Some(first) {
                paragraph_ordinal += usize::from(paragraph);
                table_ordinal += usize::from(table);
            }
        }
        if splice.removed > 0 && removed_owners.is_none() {
            return Ok(None);
        }
        let current = self.owners.len();
        // Facade paragraphs and body tables keep source order, so the splice
        // is one contiguous run of their entries.
        let head = |location: &Option<oxoffice_docx::ContentLocation>| {
            location
                .as_ref()
                .and_then(|location| location.index_path().first().copied())
        };
        let paragraph = self.inner.paragraph_count_before_content(splice.slot);
        let Some(after) = body_paragraphs.get(paragraph..) else {
            return Ok(None);
        };
        for (offset, location) in after.iter().enumerate() {
            let fits = if offset < splice.removed {
                location
                    .as_ref()
                    .map(oxoffice_docx::ContentLocation::index_path)
                    == Some(&[first + offset])
            } else {
                location.is_none() || head(location) >= Some(removed_items.end)
            };
            if !fits {
                return Ok(None);
            }
        }
        if body_paragraphs[..paragraph]
            .iter()
            .any(|location| location.is_some() && head(location) >= Some(first))
        {
            return Ok(None);
        }
        let table = location_partition_point(&self.indexes.body_tables, |location| {
            head(location) < Some(first)
        });
        for location in &self.indexes.body_tables {
            if location.as_ref().is_some_and(|location| {
                location.story().kind() == oxoffice_docx::StoryKind::Body
                    && removed_items.contains(&location.index_path()[0])
            }) {
                return Ok(None);
            }
        }
        let mut plan = BodySplicePlan {
            slot: splice.slot,
            removed: splice.removed,
            first,
            blocks: Vec::new(),
            removed_owners: removed_owners.unwrap_or(0..0),
            cells: (cells_before, 0),
            inserted_paragraph: false,
            inserted_table: false,
            paragraph,
            table,
            table_entry: None,
            piece: None,
            kept_scopes,
        };
        match splice.inserted {
            None => {}
            Some(SplicedBodyItem::Paragraph(text, runs)) => {
                plan.piece = Some(plain_paragraph_owners(&story, &text, &runs));
                let at = paragraph_at.unwrap_or(current);
                let location = oxoffice_docx::ContentLocation::new(
                    story,
                    oxoffice_docx::StoryItemKind::Paragraph,
                    vec![first],
                );
                let mut block = vec![WordOwnerAddress {
                    location: WordOwnerLocation::Paragraph(location.clone()),
                    parent: None,
                    body_slot: Some(splice.slot),
                    projection_ordinal: Some(paragraph_ordinal),
                    read_facts: WordReadFacts::Paragraph(text),
                }];
                block.extend(runs.into_iter().map(|path| WordOwnerAddress {
                    location: WordOwnerLocation::Run(location.clone(), path),
                    parent: Some(at),
                    body_slot: None,
                    projection_ordinal: None,
                    read_facts: WordReadFacts::None,
                }));
                plan.blocks.push((at, block));
                plan.inserted_paragraph = true;
            }
            Some(SplicedBodyItem::Table(xml, snapshots)) => {
                let table_at = table_at.unwrap_or(current);
                let cells_at = cells_at.unwrap_or(current);
                let Some((blocks, cells, entry, piece)) = spliced_table_blocks(
                    xml,
                    snapshots,
                    &story,
                    (splice.slot, first, table_ordinal),
                    (table_at, cells_at, cells_before),
                ) else {
                    return Ok(None);
                };
                plan.blocks = blocks;
                plan.cells = (cells_before, cells);
                plan.inserted_table = true;
                plan.table_entry = Some(entry);
                plan.piece = piece;
            }
        }
        Ok(Some(plan))
    }

    /// The number of cell stories of the main part before body story item
    /// `first`, when the inventory holds every cell story of that part and
    /// can place each one: no text box story, and every cell under a body
    /// table, so that cell story owner indexes count exactly those cells.
    fn cell_stories_before(&self, first: usize, part: &str) -> Option<usize> {
        let mut cells = Vec::new();
        let mut referenced = std::collections::HashSet::new();
        for owner in &self.owners {
            if let Some(story) = word_owner_story(&owner.location)
                && story.part_name() == part
            {
                match story.kind() {
                    oxoffice_docx::StoryKind::TextBox => return None,
                    oxoffice_docx::StoryKind::TableCell => {
                        referenced.insert(story.owner_index());
                    }
                    _ => {}
                }
            }
            let WordOwnerLocation::Cell { story, .. } = &owner.location else {
                continue;
            };
            if story.part_name() != part {
                continue;
            }
            // The body table that holds this cell, through nested tables.
            let mut table = owner.parent;
            let root = loop {
                let holder = self.owners.get(table?)?;
                match &holder.location {
                    WordOwnerLocation::Table(location)
                        if location.story().kind() == oxoffice_docx::StoryKind::Body =>
                    {
                        break location.index_path().first().copied()?;
                    }
                    WordOwnerLocation::Table(_) => {
                        let parent = holder.parent?;
                        table = self.owners.get(parent)?.parent;
                    }
                    _ => return None,
                }
            };
            cells.push((story.owner_index(), root));
        }
        cells.sort_unstable();
        let mut dense = true;
        for (index, (owner_index, _)) in cells.iter().enumerate() {
            if index != *owner_index {
                dense = false;
                break;
            }
        }
        let mut referenced_known = true;
        for owner_index in &referenced {
            if *owner_index >= cells.len() {
                referenced_known = false;
                break;
            }
        }
        let mut before = 0;
        for (_, root) in &cells {
            if *root < first {
                before += 1;
            }
        }
        let mut ordered = true;
        for (owner_index, root) in &cells {
            if (*root < first) != (*owner_index < before) {
                ordered = false;
                break;
            }
        }
        (dense && referenced_known && ordered).then_some(before)
    }

    /// Prepare a conservative, append or keeping publication of `plan`, with
    /// the retirements and births, in inventory order, before the native
    /// edit.
    pub(crate) fn prepare_body_splice(
        &mut self,
        plan: BodySplicePlan,
        operation: &'static str,
        publication: &WordPublication,
    ) -> PyResult<PreparedBodySplice> {
        // An append and a keeping splice retire the removed owners alone:
        // every other owner keeps its states, and its handles follow it to
        // its moved route. A conservative publication retires every owner
        // and gives the next inventory new scopes.
        let keep = match publication {
            WordPublication::AppendBody | WordPublication::BodySplice => true,
            WordPublication::Conservative => false,
            _ => return Err(PyValueError::new_err("body splice publication")),
        };
        #[cfg(debug_assertions)]
        let before = Box::new((self.inner.clone_for_staging(), self.owners.clone()));
        let fail =
            |_| pyo3::exceptions::PyMemoryError::new_err("Word owner splice allocation failed");
        let current = self.owners.len();
        let scopes = |next: &NextWordOwner<'_>| match next {
            NextWordOwner::Kept(index) => self.owners[*index].location.scopes().len(),
            NextWordOwner::Inserted(address) => address.location.scopes().len(),
        };
        let kept_scopes = plan.kept_scopes;
        let mut inserted_scopes = 0;
        for (_, block) in &plan.blocks {
            for address in block {
                inserted_scopes += address.location.scopes().len();
            }
        }
        let mut births = Vec::new();
        births
            .try_reserve(inserted_scopes + if keep { 0 } else { kept_scopes } + 1)
            .map_err(fail)?;
        let retiring = if keep {
            &self.owners[plan.removed_owners.clone()]
        } else {
            &self.owners[..]
        };
        let mut retired = Vec::new();
        retired
            .try_reserve(
                retiring
                    .iter()
                    .map(|owner| owner.location.scopes().len())
                    .sum::<usize>()
                    + 1,
            )
            .map_err(fail)?;
        for owner in retiring {
            retired.extend(owner.scope_states().cloned());
        }
        for next in plan.next_owners(current) {
            match next {
                NextWordOwner::Kept(_) if keep => {}
                NextWordOwner::Kept(index) => {
                    births.extend_from_slice(self.owners[index].location.scopes());
                }
                NextWordOwner::Inserted(address) => {
                    births.extend_from_slice(address.location.scopes());
                }
            }
        }
        retired.push(
            self.snapshot_scope
                .as_ref()
                .expect("active Word authority")
                .clone(),
        );
        births.push(RevisionScope::Text);
        let revision = if keep {
            self.revisions.prepare_scopes(operation, &retired, &births)
        } else {
            self.revisions.prepare_revision(
                Some(operation),
                &retired,
                &births,
                RevisionPreparation::Replacement,
            )
        }
        .map_err(revision_to_pyerr)?;
        let mut states = Vec::new();
        states
            .try_reserve_exact(inserted_scopes + kept_scopes)
            .map_err(fail)?;
        let mut inserted = Vec::new();
        inserted.try_reserve_exact(plan.added()).map_err(fail)?;
        let mut born = 0;
        let mut inserted_state = None;
        for next in plan.next_owners(current) {
            let start = states.len();
            let count = scopes(&next);
            match next {
                NextWordOwner::Kept(index) if keep => {
                    for state in self.owners[index].scope_states() {
                        states.push(state.clone());
                    }
                    continue;
                }
                NextWordOwner::Inserted(_) => {
                    inserted_state.get_or_insert(states.len());
                }
                NextWordOwner::Kept(_) => {}
            }
            for _ in 0..count {
                states.push(revision.born(born).expect("prepared owner birth"));
                born += 1;
            }
            if let NextWordOwner::Inserted(address) = next {
                inserted.extend(word_owners_with_states(
                    vec![WordOwnerAddress {
                        location: address.location.clone(),
                        parent: address.parent,
                        body_slot: address.body_slot,
                        projection_ordinal: address.projection_ordinal,
                        read_facts: address.read_facts.clone(),
                    }],
                    &states[start..start + count],
                ));
            }
        }
        let snapshot_scope = revision.born(born).expect("prepared snapshot birth");
        // Capacity for the commit, which moves owners and indexes them again.
        let added = plan.added();
        self.owners.try_reserve(added).map_err(fail)?;
        if let Some(paragraphs) = &mut self.indexes.body_paragraphs {
            paragraphs.try_reserve(1).map_err(fail)?;
        }
        self.indexes.body_tables.try_reserve(1).map_err(fail)?;
        Ok(PreparedBodySplice {
            revision,
            plan,
            states,
            inserted,
            inserted_state: inserted_state.unwrap_or(0),
            snapshot_scope,
            #[cfg(debug_assertions)]
            before,
        })
    }

    /// Capture the first owner a prepared splice inserts, the paragraph or
    /// table it places, as [`Self::capture_prepared_owner`] captures a
    /// prepared owner.
    pub(crate) fn capture_prepared_splice_owner(
        &self,
        prepared: &PreparedBodySplice,
        segments: smallvec::SmallVec<[PathSeg; 5]>,
    ) -> PyResult<(ContentPath, RevisionState)> {
        self.revisions
            .capture_prepared(&prepared.revision, smallvec![])
            .map_err(revision_to_pyerr)?;
        let unbound = || PyValueError::new_err("element has no current checked physical owner");
        let address = prepared
            .plan
            .blocks
            .first()
            .and_then(|(_, block)| block.first())
            .filter(|address| address.parent.is_none())
            .ok_or_else(unbound)?;
        let at = prepared.inserted_state;
        let states = prepared
            .states
            .get(at..at + address.location.scopes().len())
            .ok_or_else(unbound)?;
        let owners = word_owners_with_states(
            vec![WordOwnerAddress {
                location: address.location.clone(),
                parent: None,
                body_slot: address.body_slot,
                projection_ordinal: address.projection_ordinal,
                read_facts: WordReadFacts::None,
            }],
            states,
        );
        let (mut path, owner) = word_owner_capture(
            &self.revisions,
            &owners,
            prepared.revision.document_scope(),
            0,
            segments,
            WordView::Owner,
            None,
        )?;
        path.revision = prepared.revision.committed_serial();
        Ok((path, owner))
    }

    /// Publish a prepared splice after its native edit: move the kept owners
    /// in place, place the inserted ones, and index the inventory again.
    pub(crate) fn commit_body_splice(
        &mut self,
        prepared: PreparedBodySplice,
        publication: &WordPublication,
    ) -> PyResult<()> {
        #[cfg(debug_assertions)]
        if matches!(publication, WordPublication::AppendBody) {
            self.assert_body_append_matches(&prepared.plan, publication);
        }
        #[cfg(not(debug_assertions))]
        let _ = publication;
        let PreparedBodySplice {
            revision,
            mut plan,
            states,
            inserted,
            snapshot_scope,
            #[cfg(debug_assertions)]
            before,
            ..
        } = prepared;
        self.revisions
            .commit_scopes(revision)
            .map_err(revision_to_pyerr)?;
        let mut part = String::new();
        for owner in &self.owners {
            if let Some(story) = word_owner_story(&owner.location)
                .filter(|story| story.kind() == oxoffice_docx::StoryKind::Body)
            {
                part = story.part_name().to_owned();
                break;
            }
        }
        self.owners.drain(plan.removed_owners.clone());
        let mut inserted = inserted.into_iter();
        let mut spans = Vec::with_capacity(plan.blocks.len());
        let mut placed = 0;
        for (at, block) in &plan.blocks {
            let start = *at + placed;
            self.owners
                .splice(start..start, inserted.by_ref().take(block.len()));
            spans.push(start..start + block.len());
            placed += block.len();
        }
        let after = plan.first + plan.removed;
        let paragraph_delta = isize::from(plan.inserted_paragraph) - plan.removed as isize;
        let moved_ordinal = |ordinal: usize, delta: isize| ordinal.saturating_add_signed(delta);
        // Kept owners take their next states, in inventory order, by move.
        let mut states = states.into_iter();
        for (index, owner) in self.owners.iter_mut().enumerate() {
            let mut inserted_here = false;
            for span in &spans {
                if span.contains(&index) {
                    inserted_here = true;
                    break;
                }
            }
            if inserted_here {
                for _ in 0..owner.location.scopes().len() {
                    states.next();
                }
                continue;
            }
            let item = word_owner_body_item(&owner.location);
            let body_route = matches!(
                &owner.location,
                WordOwnerLocation::Paragraph(location) | WordOwnerLocation::Table(location)
                    if location.story().kind() == oxoffice_docx::StoryKind::Body
            );
            if let Some(item) = item.filter(|item| *item >= after) {
                move_word_owner_body_item(&mut owner.location, plan.moved(item));
                if body_route {
                    let delta = match owner.location {
                        WordOwnerLocation::Paragraph(_) => paragraph_delta,
                        _ => isize::from(plan.inserted_table),
                    };
                    owner.projection_ordinal = owner
                        .projection_ordinal
                        .map(|ordinal| moved_ordinal(ordinal, delta));
                }
            }
            if body_route {
                owner.body_slot = owner.body_slot.map(|slot| {
                    if slot >= plan.slot + plan.removed {
                        slot + usize::from(plan.inserted_paragraph || plan.inserted_table)
                            - plan.removed
                    } else {
                        slot
                    }
                });
            }
            plan.move_cell_stories(&mut owner.location, &part);
            owner.parent = owner.parent.map(|parent| {
                plan.next_index(parent)
                    .expect("a kept owner's parent is kept")
            });
            let mut next = || states.next().expect("a prepared state for every scope");
            owner.owner = next();
            for state in [&mut owner.content, &mut owner.properties, &mut owner.grid]
                .into_iter()
                .flatten()
            {
                *state = next();
            }
        }
        self.indexes.index_owners_again(&self.owners);
        let moved = |location: &mut oxoffice_docx::ContentLocation| {
            if location.story().kind() == oxoffice_docx::StoryKind::Body {
                let item = location.index_path()[0];
                location.set_index_path_head(plan.moved(item));
            }
        };
        if let Some(paragraphs) = &mut self.indexes.body_paragraphs {
            paragraphs.drain(plan.paragraph..plan.paragraph + plan.removed);
            for location in paragraphs.iter_mut().flatten() {
                moved(location);
            }
            if plan.inserted_paragraph
                && let Some(WordOwnerLocation::Paragraph(location)) = plan
                    .blocks
                    .first()
                    .and_then(|(_, block)| block.first())
                    .map(|address| &address.location)
            {
                paragraphs.insert(plan.paragraph, Some(location.clone()));
            }
        }
        for location in self.indexes.body_tables.iter_mut().flatten() {
            moved(location);
        }
        if let Some((location, cells)) = &plan.table_entry {
            self.indexes
                .body_tables
                .insert(plan.table, Some(location.clone()));
            let mut shifted =
                HashMap::with_capacity(self.indexes.cell_paragraphs.len() + cells.len());
            for ((table, row, cell), mut locations) in self.indexes.cell_paragraphs.drain() {
                for location in &mut locations {
                    let mut route = WordOwnerLocation::Paragraph(location.clone());
                    plan.move_cell_stories(&mut route, &part);
                    if let WordOwnerLocation::Paragraph(route) = route {
                        *location = route;
                    }
                }
                let table = if table >= plan.table {
                    table + 1
                } else {
                    table
                };
                shifted.insert((table, row, cell), locations);
            }
            for ((row, cell), locations) in cells {
                shifted.insert((plan.table, *row, *cell), locations.clone());
            }
            self.indexes.cell_paragraphs = shifted;
        }
        self.snapshot_scope = Some(snapshot_scope);
        let inserted = plan.piece.take();
        self.pieces.as_mut().expect("active pieces").splice(
            &self.inner,
            plan.slot,
            plan.removed,
            inserted,
        );
        #[cfg(debug_assertions)]
        {
            self.assert_word_inventory_rebuilds(true);
            if !matches!(publication, WordPublication::Conservative) {
                let (document, owners) = &*before;
                assert_kept_owners_name_their_elements(
                    (document, owners),
                    (&self.inner, &self.owners),
                    None,
                );
            }
        }
        Ok(())
    }

    /// Under debug assertions, prove that an append keeps exactly the owners
    /// the rebuild's matcher keeps. Runs after the native edit.
    #[cfg(debug_assertions)]
    fn assert_body_append_matches(&self, plan: &BodySplicePlan, publication: &WordPublication) {
        let rebuilt =
            word_owner_locations(&self.inner, None).expect("a spliced inventory has a rebuild");
        let mut expected = vec![None; self.owners.len() + plan.added()];
        for index in 0..self.owners.len() {
            if let Some(next) = plan.next_index(index) {
                expected[next] = Some(index);
            }
        }
        assert_eq!(
            indexed_inventory_matches(&self.owners, &rebuilt, publication),
            expected,
            "spliced Word owner matches"
        );
    }

    /// Under debug assertions, prove the inventory and its document indexes
    /// equal their rebuild from the current document, story fingerprints
    /// aside, which local edits leave stale in the inventory too. Unless
    /// `in_order`, the owners may come in any order, each known by its route
    /// and its parent's: a local cell edit appends the owners it adds.
    #[cfg(debug_assertions)]
    pub(crate) fn assert_word_inventory_rebuilds(&self, in_order: bool) {
        let address = |location: &WordOwnerLocation,
                       parent: Option<usize>,
                       body_slot: Option<usize>,
                       projection_ordinal: Option<usize>,
                       read_facts: &WordReadFacts| WordOwnerAddress {
            location: unfingerprinted_owner_location(location),
            parent,
            body_slot,
            projection_ordinal,
            read_facts: read_facts.clone(),
        };
        let actual = self
            .owners
            .iter()
            .map(|owner| {
                address(
                    &owner.location,
                    owner.parent,
                    owner.body_slot,
                    owner.projection_ordinal,
                    &owner.read_facts,
                )
            })
            .collect::<Vec<_>>();
        let expected = word_owner_locations(&self.inner, None)
            .expect("a spliced inventory has a rebuild")
            .iter()
            .map(|owner| {
                address(
                    &owner.location,
                    owner.parent,
                    owner.body_slot,
                    owner.projection_ordinal,
                    &owner.read_facts,
                )
            })
            .collect::<Vec<_>>();
        if in_order {
            assert_eq!(actual, expected, "spliced Word owner inventory");
        } else {
            let records = |addresses: &[WordOwnerAddress]| {
                let mut records = addresses
                    .iter()
                    .map(|address| {
                        format!(
                            "{:?}",
                            (
                                &address.location,
                                address.parent.map(|parent| &addresses[parent].location),
                                address.body_slot,
                                address.projection_ordinal,
                                &address.read_facts,
                            )
                        )
                    })
                    .collect::<Vec<_>>();
                records.sort();
                records
            };
            assert_eq!(records(&actual), records(&expected), "Word owner inventory");
        }
        let indexes =
            WordOwnerIndexes::prepare(&self.owners, WordIndexProjections::of(&self.inner))
                .expect("a spliced inventory has rebuilt indexes");
        let locations = |locations: &[Option<oxoffice_docx::ContentLocation>]| {
            locations
                .iter()
                .map(|location| location.as_ref().map(unfingerprinted_location))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            self.indexes.body_paragraphs.as_deref().map(locations),
            indexes.body_paragraphs.as_deref().map(locations),
            "spliced body paragraph index"
        );
        assert_eq!(
            locations(&self.indexes.body_tables),
            locations(&indexes.body_tables),
            "spliced body table index"
        );
        let cells =
            |cells: &HashMap<(usize, usize, usize), Vec<oxoffice_docx::ContentLocation>>| {
                cells
                    .iter()
                    .map(|(key, locations)| {
                        (
                            *key,
                            locations
                                .iter()
                                .map(unfingerprinted_location)
                                .collect::<Vec<_>>(),
                        )
                    })
                    .collect::<BTreeMap<_, _>>()
            };
        assert_eq!(
            cells(&self.indexes.cell_paragraphs),
            cells(&indexes.cell_paragraphs),
            "spliced cell paragraph index"
        );
        assert_eq!(
            self.indexes.direct_body_paragraphs, indexes.direct_body_paragraphs,
            "spliced direct body paragraph index"
        );
        // The owner indexes, each list in any order, an empty one as none.
        fn lists<K: Copy + Ord>(map: &IndexMap<K, Vec<usize>>) -> BTreeMap<K, Vec<usize>> {
            map.iter()
                .filter(|(_, list)| !list.is_empty())
                .map(|(key, list)| {
                    let mut list = list.clone();
                    list.sort_unstable();
                    (*key, list)
                })
                .collect()
        }
        assert_eq!(
            self.indexes
                .by_generation
                .iter()
                .collect::<BTreeMap<_, _>>(),
            indexes.by_generation.iter().collect::<BTreeMap<_, _>>(),
            "owner index by generation"
        );
        assert_eq!(
            lists(&self.indexes.by_route),
            lists(&indexes.by_route),
            "owner index by route"
        );
        for (name, actual, expected) in [
            (
                "paragraphs",
                &self.indexes.paragraphs_by_parent,
                &indexes.paragraphs_by_parent,
            ),
            (
                "tables",
                &self.indexes.tables_by_parent,
                &indexes.tables_by_parent,
            ),
            (
                "cells",
                &self.indexes.cells_by_parent,
                &indexes.cells_by_parent,
            ),
            (
                "runs",
                &self.indexes.runs_by_parent,
                &indexes.runs_by_parent,
            ),
        ] {
            assert_eq!(
                lists(actual),
                lists(expected),
                "owner index of {name} by parent"
            );
        }
    }
}

/// The inserted blocks of a detached table placed at body story item
/// `first` of slot `slot`: the table and its cells before current owner
/// `table_at`, its cell paragraphs and runs before `cells_at`, with cell
/// story owners counted from `cells_before`. Also the number of cells, and
/// the table route with the routes of each cell's paragraphs.
#[allow(clippy::type_complexity)] // The blocks, the cell count and the index entry of one table.
fn spliced_table_blocks(
    xml: Vec<u8>,
    snapshots: WordOwnerSnapshots,
    story: &oxoffice_docx::StoryId,
    (slot, first, ordinal): (usize, usize, usize),
    (table_at, cells_at, cells_before): (usize, usize, usize),
) -> Option<(
    Vec<(usize, Vec<WordOwnerAddress>)>,
    usize,
    (oxoffice_docx::ContentLocation, Vec<CellParagraphs>),
    Option<WordBlockOwners>,
)> {
    // The detached body holds this table as its only item.
    let table_location = {
        let mut items = snapshots
            .0
            .iter()
            .filter(|item| item.location().story().kind() == oxoffice_docx::StoryKind::Body);
        let item = items.next()?;
        if items.next().is_some()
            || item.location().item_kind() != oxoffice_docx::StoryItemKind::Table
            || item.location().index_path() != [0]
        {
            return None;
        }
        item.location().clone()
    };
    let piece_facts = word_table_facts(&xml).and_then(|rows| {
        let mut facts = WordOwnerFacts {
            cells: snapshots.1.clone(),
            ..WordOwnerFacts::default()
        };
        facts.items.push((table_location.clone(), Some(0), rows));
        for (location, paragraph_xml) in &snapshots.2 {
            let paragraph = oxoffice_docx_oxml::CT_P::from_xml_fragment(paragraph_xml).ok()?;
            facts.paragraphs.push((
                location.clone(),
                paragraph.accepted_text(),
                paragraph.accepted_run_paths(),
            ));
        }
        Some(facts)
    });
    let facts = word_table_facts(&xml)
        .map(|facts| (table_location, facts))
        .into_iter()
        .collect::<HashMap<_, _>>();
    let addresses = word_owner_addresses(snapshots, &facts, None, None).ok()?;
    let cells = addresses
        .iter()
        .skip(1)
        .take_while(|address| matches!(address.location, WordOwnerLocation::Cell { .. }))
        .count();
    // The table, its cells in story order, then only their paragraphs and runs.
    let cell_story = |address: &WordOwnerAddress| match &address.location {
        WordOwnerLocation::Cell { story, .. } => Some(story.owner_index()),
        _ => None,
    };
    let in_cells = |location: &oxoffice_docx::ContentLocation| {
        location.story().kind() == oxoffice_docx::StoryKind::TableCell
            && location.story().owner_index() < cells
    };
    let cell_parent =
        |parent: Option<usize>| parent.is_some_and(|parent| (1..=cells).contains(&parent));
    let shaped = matches!(addresses.first()?.location, WordOwnerLocation::Table(_))
        && addresses[1..=cells]
            .iter()
            .enumerate()
            .all(|(index, address)| {
                cell_story(address) == Some(index) && address.parent == Some(0)
            })
        && addresses[cells + 1..]
            .iter()
            .all(|address| match &address.location {
                WordOwnerLocation::Paragraph(location) => {
                    in_cells(location) && cell_parent(address.parent)
                }
                WordOwnerLocation::Run(location, _) => {
                    in_cells(location) && address.parent.is_some_and(|parent| parent > cells)
                }
                _ => false,
            });
    if !shaped {
        return None;
    }
    // Detached owner `index` takes this next-inventory index.
    let next = |index: usize| {
        if index <= cells {
            table_at + index
        } else {
            cells_at + index
        }
    };
    let moved = |location: &oxoffice_docx::ContentLocation| {
        let mut story = location.story().clone();
        story.set_owner_index(story.owner_index() + cells_before);
        oxoffice_docx::ContentLocation::new(
            story,
            location.item_kind(),
            location.index_path().to_vec(),
        )
    };
    let table = oxoffice_docx::ContentLocation::new(
        story.clone(),
        oxoffice_docx::StoryItemKind::Table,
        vec![first],
    );
    let mut entries = Vec::with_capacity(cells);
    let mut head = Vec::with_capacity(cells + 1);
    let mut tail = Vec::with_capacity(addresses.len() - cells - 1);
    for (index, address) in addresses.into_iter().enumerate() {
        let parent = address.parent.map(next);
        let location = match &address.location {
            WordOwnerLocation::Table(_) => WordOwnerLocation::Table(table.clone()),
            WordOwnerLocation::Cell {
                row, cell, story, ..
            } => {
                let mut story = story.clone();
                story.set_owner_index(story.owner_index() + cells_before);
                entries.push(((*row, *cell), Vec::new()));
                WordOwnerLocation::Cell {
                    table: table.clone(),
                    row: *row,
                    cell: *cell,
                    story,
                }
            }
            WordOwnerLocation::Paragraph(location) => {
                let location = moved(location);
                entries[address.parent? - 1].1.push(location.clone());
                WordOwnerLocation::Paragraph(location)
            }
            WordOwnerLocation::Run(location, path) => {
                WordOwnerLocation::Run(moved(location), path.clone())
            }
            _ => return None,
        };
        let address = if index == 0 {
            WordOwnerAddress {
                location,
                parent: None,
                body_slot: Some(slot),
                projection_ordinal: Some(ordinal),
                read_facts: address.read_facts,
            }
        } else {
            WordOwnerAddress {
                location,
                parent,
                ..address
            }
        };
        if index <= cells {
            head.push(address);
        } else {
            tail.push(address);
        }
    }
    let mut blocks = vec![(table_at, head)];
    if !tail.is_empty() {
        blocks.push((cells_at, tail));
    }
    let piece = piece_facts.and_then(|facts| table_owners(facts, cells));
    Some((blocks, cells, (table, entries), piece))
}
