//! Direct body blocks spliced in place: N blocks removed and M inserted at
//! one slot, published from the owners of those blocks alone. The old
//! owners are found by route, the new ones are read from the new blocks, and
//! the records of the body items after them move once.

use super::paragraph_in_place::{BlockRetention, BlocksPublication};
use super::*;

/// The owners of some direct body blocks read alone, placed as the first
/// blocks of an empty body: body story items, slots, cell stories and every
/// ordinal counted from 0.
struct PlacedBlocks {
    /// The owners, with parents among themselves.
    addresses: Vec<WordOwnerAddress>,
    /// The records of their body items.
    records: WordBodyItems,
    /// Body story items, main-part cell stories and facade paragraphs.
    items: usize,
    cells: usize,
    /// The facade paragraph projection: each paragraph's route, if any.
    paragraphs: Vec<Option<oxoffice_docx::ContentLocation>>,
    /// The facade table projection: a direct body table's route, `None` for
    /// a table inside a block content control.
    tables: Vec<Option<oxoffice_docx::ContentLocation>>,
    /// The routes of each cell's paragraphs, by facade table.
    cell_paragraphs: Vec<HashMap<(usize, usize), Vec<oxoffice_docx::ContentLocation>>>,
}

/// Where placed blocks go: their first body story item, their first slot,
/// and what comes before them: main-part cell stories, body paragraph, body
/// table and body hyperlink owners.
#[derive(Clone, Copy)]
struct BlockBases {
    first: usize,
    slot: usize,
    cells: usize,
    paragraphs: usize,
    tables: usize,
    links: usize,
}

impl BlockBases {
    fn location(
        &self,
        location: &oxoffice_docx::ContentLocation,
    ) -> oxoffice_docx::ContentLocation {
        placed_route(location, location.story(), (self.first, (self.cells, 0)))
    }

    fn story(&self, story: &oxoffice_docx::StoryId) -> oxoffice_docx::StoryId {
        placed_story(story, (self.cells, 0))
    }

    /// `address`, placed at these bases.
    fn address(&self, address: &WordOwnerAddress) -> WordOwnerAddress {
        let body = |location: &oxoffice_docx::ContentLocation| {
            location.story().kind() == oxoffice_docx::StoryKind::Body
        };
        let (location, base) = match &address.location {
            WordOwnerLocation::Paragraph(location) => (
                WordOwnerLocation::Paragraph(self.location(location)),
                body(location).then_some(self.paragraphs),
            ),
            WordOwnerLocation::Table(location) => (
                WordOwnerLocation::Table(self.location(location)),
                body(location).then_some(self.tables),
            ),
            WordOwnerLocation::Run(location, path) => (
                WordOwnerLocation::Run(self.location(location), path.clone()),
                None,
            ),
            WordOwnerLocation::Hyperlink(location, index) => (
                WordOwnerLocation::Hyperlink(self.location(location), *index),
                body(location).then_some(self.links),
            ),
            WordOwnerLocation::Cell {
                table,
                row,
                cell,
                story,
            } => (
                WordOwnerLocation::Cell {
                    table: self.location(table),
                    row: *row,
                    cell: *cell,
                    story: self.story(story),
                },
                None,
            ),
            WordOwnerLocation::Section(index) => (WordOwnerLocation::Section(*index), None),
        };
        WordOwnerAddress {
            location,
            parent: address.parent,
            body_slot: address.body_slot.map(|slot| slot + self.slot),
            projection_ordinal: address
                .projection_ordinal
                .map(|ordinal| ordinal + base.unwrap_or(0)),
            read_facts: address.read_facts.clone(),
        }
    }
}

/// Place `blocks` (each block with its owners) as the first blocks of an
/// empty body, as [`WordPieces::inventory`] places pieces. `None` for a block
/// that holds a text box, or whose cell paragraphs are not all routed.
fn placed_blocks(
    body: &oxoffice_docx::StoryId,
    blocks: &[(&oxoffice_docx_oxml::BodyContent, &WordBlockOwners)],
) -> Option<PlacedBlocks> {
    let mut facts = WordOwnerFacts::default();
    let mut cells = WordOwnerFacts::default();
    let mut placed = PlacedBlocks {
        addresses: Vec::new(),
        records: WordBodyItems::default(),
        items: 0,
        cells: 0,
        paragraphs: Vec::new(),
        tables: Vec::new(),
        cell_paragraphs: Vec::new(),
    };
    let in_body = |location: &oxoffice_docx::ContentLocation| {
        location.story().kind() == oxoffice_docx::StoryKind::Body
    };
    for (slot, (block, owners)) in blocks.iter().enumerate() {
        if owners.text_boxes != 0 {
            return None;
        }
        let Some((routes, true)) = &owners.cell_paragraphs else {
            return None;
        };
        let stories = (placed.cells, 0);
        let place = |location: &oxoffice_docx::ContentLocation| {
            placed_route(location, body, (placed.items, stories))
        };
        for (location, direct, read_facts) in &owners.facts.items {
            let family = if in_body(location) {
                &mut facts.items
            } else {
                &mut cells.items
            };
            family.push((place(location), direct.map(|_| slot), read_facts.clone()));
        }
        for (table, row, cell, story) in &owners.facts.cells {
            facts
                .cells
                .push((place(table), *row, *cell, placed_story(story, stories)));
        }
        for (location, text, runs) in &owners.facts.paragraphs {
            let family = if in_body(location) {
                &mut facts.paragraphs
            } else {
                &mut cells.paragraphs
            };
            family.push((place(location), text.clone(), runs.clone()));
        }
        for location in &owners.facts.links {
            let family = if in_body(location) {
                &mut facts.links
            } else {
                &mut cells.links
            };
            family.push(place(location));
        }
        placed
            .paragraphs
            .extend(owners.paragraphs.iter().map(|path| {
                path.as_ref().map(|path| {
                    let mut location = oxoffice_docx::ContentLocation::new(
                        body.clone(),
                        oxoffice_docx::StoryItemKind::Paragraph,
                        path.clone(),
                    );
                    location.set_index_path_head(path[0] + placed.items);
                    location
                })
            }));
        let tables = placed.tables.len();
        if matches!(block, oxoffice_docx_oxml::BodyContent::Table(_)) {
            placed.tables.push(Some(oxoffice_docx::ContentLocation::new(
                body.clone(),
                oxoffice_docx::StoryItemKind::Table,
                vec![placed.items],
            )));
            placed.cell_paragraphs.push(HashMap::new());
        } else {
            placed
                .tables
                .extend(std::iter::repeat_n(None, owners.tables));
            placed
                .cell_paragraphs
                .extend(std::iter::repeat_n(HashMap::new(), owners.tables));
        }
        for ((table, row, cell), locations) in routes {
            let table = placed.cell_paragraphs.get_mut(tables + table)?;
            table.insert((*row, *cell), locations.iter().map(&place).collect());
        }
        placed.items += owners.body_items;
        placed.cells += owners.cell_stories;
    }
    facts.items.append(&mut cells.items);
    facts.paragraphs.append(&mut cells.paragraphs);
    facts.links.append(&mut cells.links);
    placed.addresses = word_piece_owner_addresses(facts, 0);
    placed.records = WordBodyItems::of_blocks(&placed.addresses)?;
    Some(placed)
}

/// A splice of direct body blocks, planned from the inventory alone.
pub(crate) struct BlocksSplice {
    slot: usize,
    removed: usize,
    inserted: usize,
    bases: BlockBases,
    old: PlacedBlocks,
    new: PlacedBlocks,
    /// The keys of the old owners, in the order of `old.addresses`.
    old_keys: Vec<usize>,
    /// The new owners, placed.
    born: Vec<WordOwnerAddress>,
    /// The facade paragraph index of the slot.
    paragraph: usize,
    pieces: Vec<WordBlockOwners>,
    /// The states that retire and the scopes born, the snapshot scope aside.
    pub(super) retired: Vec<RevisionState>,
    pub(super) births: Vec<RevisionScope>,
}

#[cfg(debug_assertions)]
impl BlocksSplice {
    /// The owners the splice retires and those it bears.
    pub(super) fn counts(&self) -> (usize, usize) {
        (self.old_keys.len(), self.born.len())
    }
}

impl PyDocument {
    /// The plan of a splice of `previous` by the `inserted` blocks now at
    /// `slot`, read from the inventory, the pieces and the new blocks, or
    /// `None` for the whole route.
    pub(super) fn plan_blocks_splice(
        &self,
        slot: usize,
        previous: &[oxoffice_docx_oxml::BodyContent],
        inserted: usize,
    ) -> Option<BlocksSplice> {
        let indexes = &self.indexes;
        let items = &indexes.items;
        let pieces = self.pieces.as_ref()?;
        let blocks = self.inner.body_children();
        let removed = previous.len();
        if !items.ready()
            || pieces.context.as_ref() != Some(&self.inner.body_item_context())
            || pieces.blocks.len() + inserted != blocks.len() + removed
            || slot.checked_add(removed)? > pieces.blocks.len()
            || slot + inserted > blocks.len()
        {
            return None;
        }
        let story = items.story()?.clone();
        // A paragraph that ends a section moves the sections: their owners
        // would not follow.
        let ends_section = |block: &oxoffice_docx_oxml::BodyContent| match block {
            oxoffice_docx_oxml::BodyContent::Paragraph(paragraph) => paragraph
                .properties
                .as_ref()
                .is_some_and(|properties| properties.sect_pr.is_some()),
            _ => false,
        };
        let new_blocks = &blocks[slot..slot + inserted];
        if previous.iter().chain(new_blocks).any(ends_section) {
            return None;
        }
        let old_pieces = &pieces.blocks[slot..slot + removed];
        let mut old_blocks = Vec::with_capacity(removed);
        for (piece, block) in old_pieces.iter().zip(previous) {
            if !aligned_block(piece, block) {
                return None;
            }
            old_blocks.push((block, piece.owners.as_ref()?));
        }
        let mut new_owners = Vec::with_capacity(inserted);
        for block in new_blocks {
            new_owners.push(word_block_owners(&self.inner, block).ok().flatten()?);
        }
        let old = placed_blocks(&story, &old_blocks)?;
        let new = placed_blocks(
            &story,
            &new_blocks.iter().zip(&new_owners).collect::<Vec<_>>(),
        )?;
        // Unanchored cell stories after the slot would not follow a change.
        if !items.cells_anchored() && (old.cells != 0 || new.cells != 0) {
            return None;
        }
        let first = self.first_item_of_slot(slot, removed, &old)?;
        // The old items hold exactly the records of the old blocks.
        #[cfg(debug_assertions)]
        assert_eq!(
            items.records_between(first, first + old.items),
            old.records.len(),
            "records of the old blocks"
        );
        let (paragraphs, tables, cells) = items.before(first);
        let bases = BlockBases {
            first,
            slot,
            cells,
            paragraphs,
            tables,
            links: items.links_before(first),
        };
        // The old owners, found by their routes.
        let mut old_keys = Vec::with_capacity(old.addresses.len());
        for address in &old.addresses {
            old_keys.push(indexes.find(&self.owners, &bases.address(address).location)?);
        }
        // The projections take one range each.
        let paragraph = self.inner.paragraph_count_before_content(slot);
        if indexes
            .body_paragraphs
            .as_ref()
            .is_some_and(|routes| paragraph + old.paragraphs.len() > routes.len())
            || tables + old.tables.len() > indexes.body_tables.len()
            || tables + old.tables.len() > indexes.cell_paragraphs.len()
        {
            return None;
        }
        let born = new
            .addresses
            .iter()
            .map(|address| bases.address(address))
            .collect::<Vec<WordOwnerAddress>>();
        // The old owners retire, the new ones are born.
        let mut retired = Vec::new();
        retired.try_reserve(old_keys.len() * 3).ok()?;
        for key in &old_keys {
            retired.extend(self.owners[*key].scope_states().cloned());
        }
        let mut births = Vec::new();
        births.try_reserve(born.len() * 3).ok()?;
        for address in &born {
            births.extend_from_slice(address.location.scopes());
        }
        Some(BlocksSplice {
            slot,
            removed,
            inserted,
            bases,
            old,
            new,
            old_keys,
            born,
            paragraph,
            pieces: new_owners,
            retired,
            births,
        })
    }

    /// The body story item index of the first item of direct body slot
    /// `slot`, before a splice of the `removed` blocks there that `old`
    /// placed: read from the records around the slot and the pieces of the
    /// blocks between them, never from the blocks before.
    fn first_item_of_slot(&self, slot: usize, removed: usize, old: &PlacedBlocks) -> Option<usize> {
        let items = &self.indexes.items;
        let pieces = &self.pieces.as_ref()?.blocks;
        let count = |range: std::ops::Range<usize>| -> Option<usize> {
            let mut total = 0;
            for piece in pieces.get(range)? {
                total += piece.owners.as_ref()?.body_items;
            }
            Some(total)
        };
        let first = match items.first_at_or_after_slot(slot) {
            Some((index, at, true)) if at < slot + removed => {
                // The first record of the old blocks.
                let (head, relative, true) = old.records.first_record()? else {
                    return None;
                };
                (at == slot + relative).then_some(())?;
                index.checked_sub(head)?
            }
            Some((index, at, true)) => index.checked_sub(count(slot..at)?)?,
            Some(_) => return None,
            None => match items.last_before_slot(slot) {
                Some((index, at, true)) => index + count(at..slot)?,
                Some(_) => return None,
                None => count(0..slot)?,
            },
        };
        #[cfg(debug_assertions)]
        if let Some(walked) = count(0..slot) {
            assert_eq!(first, walked, "first body story item of a slot");
        }
        Some(first)
    }

    /// Commit `plan`: retire the old owners and the snapshot, replace the
    /// records of the old items, splice the projections, remove the old
    /// owners by swap and append the new ones, then the pieces.
    /// Reserve what [`Self::apply_blocks_splice`] adds, so that nothing
    /// fails once the revision commits.
    pub(super) fn reserve_blocks_splice(&mut self, plan: &BlocksSplice) -> PyResult<()> {
        let fail =
            |_| pyo3::exceptions::PyMemoryError::new_err("Word block splice allocation failed");
        let added = plan.born.len();
        self.owners.try_reserve(added).map_err(fail)?;
        self.indexes
            .by_generation
            .try_reserve(added)
            .map_err(fail)?;
        self.indexes.by_route.try_reserve(added).map_err(fail)?;
        self.indexes
            .items
            .reserve(plan.new.records.len())
            .map_err(fail)?;
        self.indexes
            .direct_body_paragraphs
            .try_reserve(plan.inserted)
            .map_err(fail)?;
        if let Some(paragraphs) = &mut self.indexes.body_paragraphs {
            paragraphs
                .try_reserve(plan.new.paragraphs.len())
                .map_err(fail)?;
        }
        self.indexes
            .body_tables
            .try_reserve(plan.new.tables.len())
            .map_err(fail)?;
        self.indexes
            .cell_paragraphs
            .try_reserve(plan.new.tables.len())
            .map_err(fail)?;
        Ok(())
    }

    /// Install `plan` after its revision committed, under `states` (the
    /// births of `plan.births`, in order): replace the records of the old
    /// items, splice the projections, remove the old owners by swap and
    /// append the new ones, then the pieces. Nothing here fails.
    pub(super) fn apply_blocks_splice(&mut self, plan: BlocksSplice, states: &[RevisionState]) {
        let BlocksSplice {
            slot,
            removed,
            inserted,
            bases,
            old,
            new,
            old_keys,
            born,
            paragraph,
            pieces,
            ..
        } = plan;
        let indexes = &mut self.indexes;
        indexes.items.replace(
            (bases.first, slot),
            (old.items, removed),
            &new.records,
            (new.items, inserted),
        );
        let slots = &mut indexes.direct_body_paragraphs;
        if slot < slots.len() {
            let end = (slot + removed).min(slots.len());
            slots.splice(slot..end, std::iter::repeat_n(None, inserted));
        }
        let items = &indexes.items;
        if let Some(routes) = &mut indexes.body_paragraphs {
            routes.splice(
                paragraph..paragraph + old.paragraphs.len(),
                new.paragraphs.iter().map(|location| {
                    location
                        .as_ref()
                        .map(|location| items.route(bases.location(location)))
                }),
            );
        }
        let table = bases.tables;
        indexes.body_tables.splice(
            table..table + old.tables.len(),
            new.tables.iter().map(|location| {
                location
                    .as_ref()
                    .map(|location| items.route(bases.location(location)))
            }),
        );
        indexes.cell_paragraphs.splice(
            table..table + old.tables.len(),
            new.cell_paragraphs.iter().map(|cells| {
                cells
                    .iter()
                    .map(|(cell, locations)| {
                        let routes = locations
                            .iter()
                            .map(|location| items.route(bases.location(location)))
                            .collect();
                        (*cell, routes)
                    })
                    .collect()
            }),
        );
        self.remove_owners(&old_keys);
        // The new owners take keys after every current one.
        let base = self.owners.len();
        let mut offset = 0;
        for mut address in born {
            let scopes = &states[offset..offset + address.location.scopes().len()];
            offset += scopes.len();
            address.parent = address.parent.map(|parent| base + parent);
            let owner = WordOwner::new(address, scopes, &self.indexes.items);
            self.indexes.index_owner(self.owners.len(), &owner);
            self.owners.push(owner);
        }
        self.pieces.as_mut().expect("active pieces").splice_blocks(
            &self.inner,
            slot,
            removed,
            pieces,
        );
    }

    /// Under debug assertions, before a splice commits: the old owners found
    /// by route are every owner of the old blocks, as a walk of the owners
    /// finds them by their body story item or main-part cell story.
    #[cfg(debug_assertions)]
    pub(super) fn assert_old_owners_are_the_blocks_closure(&self, plan: &BlocksSplice) {
        let items = plan.bases.first..plan.bases.first + plan.old.items;
        let cells = plan.bases.cells..plan.bases.cells + plan.old.cells;
        let part = self
            .indexes
            .items
            .story()
            .expect("a ready body")
            .part_name()
            .to_owned();
        let inside = |location: &oxoffice_docx::ContentLocation| {
            let story = location.story();
            story.part_name() == part
                && match story.kind() {
                    oxoffice_docx::StoryKind::Body => location
                        .index_path()
                        .first()
                        .is_some_and(|head| items.contains(head)),
                    oxoffice_docx::StoryKind::TableCell => cells.contains(&story.owner_index()),
                    _ => false,
                }
        };
        let mut walked = self
            .owners
            .iter()
            .enumerate()
            .filter(|(_, owner)| match &*owner.location() {
                WordOwnerLocation::Paragraph(location)
                | WordOwnerLocation::Table(location)
                | WordOwnerLocation::Run(location, _)
                | WordOwnerLocation::Hyperlink(location, _) => inside(location),
                WordOwnerLocation::Cell { table, .. } => inside(table),
                WordOwnerLocation::Section(_) => false,
            })
            .map(|(key, _)| key)
            .collect::<Vec<_>>();
        walked.sort_unstable();
        let mut found = plan.old_keys.clone();
        found.sort_unstable();
        assert_eq!(found, walked, "old block owners found by route");
        // The parent maps hold the same closure from the blocks' roots.
        let mut closure = found
            .iter()
            .copied()
            .filter(|key| {
                self.owners[*key].parent.is_none()
                    || !found.contains(&self.owners[*key].parent.expect("a parent"))
            })
            .collect::<Vec<_>>();
        let mut index = 0;
        while index < closure.len() {
            let key = closure[index];
            for map in [
                &self.indexes.paragraphs_by_parent,
                &self.indexes.tables_by_parent,
                &self.indexes.cells_by_parent,
                &self.indexes.runs_by_parent,
            ] {
                closure.extend(map.get(&key).into_iter().flatten().copied());
            }
            index += 1;
        }
        closure.sort_unstable();
        assert_eq!(closure, found, "old block owners are their roots' closure");
    }

    /// Under debug assertions, after a splice: the inventory equals its
    /// rebuild, every kept owner names its own element, and the splice kept
    /// every owner outside the blocks and only those, as "body handles follow
    /// their element" keeps them.
    #[cfg(debug_assertions)]
    pub(super) fn assert_blocks_splice(
        &self,
        (document, owners): &(oxoffice_docx::Document, Vec<WordOwner>),
        retired: usize,
        born: usize,
    ) {
        self.assert_word_inventory_rebuilds(false);
        assert_kept_owners_name_their_elements(
            (document, owners),
            (&self.inner, &self.owners),
            None,
        );
        let before = owners
            .iter()
            .map(|owner| (owner.owner.generation(), owner))
            .collect::<HashMap<_, _>>();
        let mut kept = 0;
        for owner in &self.owners {
            if let Some(old) = before.get(&owner.owner.generation()) {
                kept += 1;
                assert!(
                    old.scope_states()
                        .zip(owner.scope_states())
                        .all(|(old, new)| old.same_state(new)),
                    "a kept owner keeps its states"
                );
            }
        }
        assert_eq!(
            (owners.len() - kept, self.owners.len() - kept),
            (retired, born),
            "a splice retires the owners of its old blocks and bears those of its new ones"
        );
    }
}

/// Blocks the facade removed in place, held until their publication
/// commits: any other exit (a refusal, an error, a panic) puts them back.
struct RemovedBlocks<'a> {
    document: &'a mut PyDocument,
    removed: Option<oxoffice_docx::RemovedBodyBlocks>,
}

impl RemovedBlocks<'_> {
    /// Publish the removal; `false` when the splice answers the whole route,
    /// and the guard then puts the blocks back when it drops.
    fn publish(&mut self, py: Python<'_>, operation: &'static str) -> PyResult<Option<usize>> {
        let Self { document, removed } = self;
        let removed = removed.as_ref().expect("held removed blocks");
        let published = document.publish_body_blocks_replaced(
            py,
            removed.slot(),
            removed.blocks(),
            0,
            operation,
            BlockRetention::Spliced,
        )?;
        if published != BlocksPublication::Published {
            return Ok(None);
        }
        let count = removed.blocks().len();
        self.removed = None;
        Ok(Some(count))
    }
}

impl Drop for RemovedBlocks<'_> {
    fn drop(&mut self) {
        if let Some(removed) = self.removed.take() {
            self.document
                .inner
                .restore_body_blocks(removed)
                .expect("removed blocks go back where they were");
        }
    }
}

/// Debug builds take the staged route on request, so that tests compare
/// what the two routes save.
fn staged_on_request() -> bool {
    #[cfg(debug_assertions)]
    {
        std::env::var_os("OXOFFICE_DEBUG_STAGED_BODY_REMOVAL").is_some()
    }
    #[cfg(not(debug_assertions))]
    {
        false
    }
}

impl PyDocument {
    /// Remove the direct body blocks `start..end` in place, with the
    /// bookmark markers they own, and publish their removal as a splice:
    /// the number of body items removed, or `None` with the document and
    /// every handle as they were when the facade or the splice refuses, so
    /// that the caller takes its staged route.
    pub(super) fn remove_body_blocks_local(
        &mut self,
        py: Python<'_>,
        start: usize,
        end: usize,
        operation: &'static str,
    ) -> PyResult<Option<usize>> {
        self.ensure_word_authority(py)?;
        if !self.indexes.items.ready() || self.pieces.is_none() || staged_on_request() {
            return Ok(None);
        }
        let Some(removed) = self
            .inner
            .remove_body_blocks_in_place(start, end)
            .map_err(|error| docx_to_pyerr(py, error))?
        else {
            return Ok(None);
        };
        RemovedBlocks {
            document: self,
            removed: Some(removed),
        }
        .publish(py, operation)
    }

    /// `Document.pop_content` of direct body slot `slot`, in place: its
    /// fragment, or `None` with the document and every handle as they were
    /// for the staged route. `location` is the slot's body story item, which
    /// debug builds use to compare the fragment with the staged one.
    pub(super) fn pop_body_block_local(
        &mut self,
        py: Python<'_>,
        slot: usize,
        location: Option<&oxoffice_docx::ContentLocation>,
    ) -> PyResult<Option<oxoffice_docx::ContentFragment>> {
        self.ensure_word_authority(py)?;
        if !self.indexes.items.ready() || self.pieces.is_none() || staged_on_request() {
            return Ok(None);
        }
        #[cfg(debug_assertions)]
        let staged = {
            let location = match location {
                Some(location) => Some(location.clone()),
                None => self.inner.direct_body_item_location(slot).ok().flatten(),
            };
            location.and_then(|location| {
                let mut candidate = self.inner.clone_for_staging();
                candidate.remove_content_at(&location).ok()
            })
        };
        #[cfg(not(debug_assertions))]
        let _ = location;
        let Some((fragment, removed)) = self
            .inner
            .pop_body_block_in_place(slot)
            .map_err(|error| docx_to_pyerr(py, error))?
        else {
            return Ok(None);
        };
        let published = RemovedBlocks {
            document: self,
            removed: Some(removed),
        }
        .publish(py, "Document.pop_content")?;
        if published.is_none() {
            return Ok(None);
        }
        #[cfg(debug_assertions)]
        {
            let staged = staged.expect("the staged route takes what the in-place route takes");
            assert!(
                fragment.same_taken_fragment(&staged),
                "an in-place fragment equals the staged one:\n{fragment:?}\n{staged:?}"
            );
        }
        Ok(Some(fragment))
    }
}
