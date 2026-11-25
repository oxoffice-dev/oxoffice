//! Word owners and the body items they are stored relative to: a body splice
//! moves the records of the items after it, never the owners.

use super::*;
use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};

/// Keys of body item records, unique in the process.
static ITEM_KEYS: AtomicU64 = AtomicU64::new(1);

/// One top-level body story item that holds owners: where it sits now.
///
/// The owners of the item and the projection routes into it store their
/// numbers relative to this record (see [`WordAnchor`]). A body splice
/// updates the records of the items after it, and touches no owner.
pub(crate) struct BodyItem {
    /// A key that never changes, for the route index.
    key: u64,
    /// The item's index among the body story items.
    index: AtomicUsize,
    /// The direct body slot that holds the item: when no owner of the item
    /// names it, the slot of the item before, so that slots stay in order.
    slot: AtomicUsize,
    has_slot: bool,
    /// The body paragraph owners, the body table owners and the main-part
    /// cell stories before the item.
    paragraphs: AtomicUsize,
    tables: AtomicUsize,
    cells: AtomicUsize,
    /// How many of each the item holds.
    counts: (usize, usize, usize),
    /// The body story hyperlink owners of the item.
    links: usize,
}

impl BodyItem {
    fn new(
        index: usize,
        slot: Option<usize>,
        before: (usize, usize, usize),
        counts: (usize, usize, usize),
        links: usize,
        previous_slot: usize,
    ) -> Arc<Self> {
        Arc::new(Self {
            key: ITEM_KEYS.fetch_add(1, Relaxed),
            index: AtomicUsize::new(index),
            slot: AtomicUsize::new(slot.unwrap_or(previous_slot)),
            has_slot: slot.is_some(),
            paragraphs: AtomicUsize::new(before.0),
            tables: AtomicUsize::new(before.1),
            cells: AtomicUsize::new(before.2),
            counts,
            links,
        })
    }

    /// The item's index among the body story items.
    pub(super) fn index(&self) -> usize {
        self.index.load(Relaxed)
    }

    /// The direct body slot that holds the item.
    pub(super) fn slot(&self) -> usize {
        self.slot.load(Relaxed)
    }

    /// The body paragraphs, body tables and main-part cell stories the item
    /// holds.
    pub(super) fn counts(&self) -> (usize, usize, usize) {
        self.counts
    }

    /// The body story hyperlink owners the item holds.
    pub(super) fn links(&self) -> usize {
        self.links
    }

    fn paragraphs(&self) -> usize {
        self.paragraphs.load(Relaxed)
    }

    fn tables(&self) -> usize {
        self.tables.load(Relaxed)
    }

    fn cells(&self) -> usize {
        self.cells.load(Relaxed)
    }

    /// Move the record by a splice before it: items, slots, body paragraphs,
    /// body tables and cell stories inserted (positive) or removed.
    fn shift(&self, by: BodyShift) {
        // Wrapping adds of a two's complement change.
        self.index.fetch_add(by.items as usize, Relaxed);
        self.slot.fetch_add(by.slots as usize, Relaxed);
        self.paragraphs.fetch_add(by.paragraphs as usize, Relaxed);
        self.tables.fetch_add(by.tables as usize, Relaxed);
        self.cells.fetch_add(by.cells as usize, Relaxed);
    }

    /// The record's numbers, to compare with a record derived from a walk:
    /// index, slot, the bases before it, its counts and its links.
    #[cfg(debug_assertions)]
    fn facts(&self) -> RecordFacts {
        (
            self.index(),
            self.has_slot.then(|| self.slot()),
            (self.paragraphs(), self.tables(), self.cells()),
            self.counts,
            self.links,
        )
    }
}

/// The numbers of a record: index, slot, the bases before it, its counts
/// and its links.
#[cfg(debug_assertions)]
type RecordFacts = (
    usize,
    Option<usize>,
    (usize, usize, usize),
    (usize, usize, usize),
    usize,
);

/// What a body splice inserts (positive) or removes before the records after it.
#[derive(Clone, Copy, Default)]
pub(super) struct BodyShift {
    pub(super) items: isize,
    pub(super) slots: isize,
    pub(super) paragraphs: isize,
    pub(super) tables: isize,
    pub(super) cells: isize,
}

/// The body item an owner or a projection route is stored relative to.
#[derive(Clone)]
pub(super) struct WordAnchor {
    item: Arc<BodyItem>,
    /// Whether a main-part cell story ordinal is stored relative to the
    /// item's first cell story.
    cells: bool,
}

impl WordAnchor {
    pub(super) fn key(&self) -> u64 {
        self.item.key
    }

    /// The current form of a stored route.
    fn resolve(&self, location: &oxoffice_docx::ContentLocation) -> oxoffice_docx::ContentLocation {
        match location.story().kind() {
            oxoffice_docx::StoryKind::Body => {
                let mut resolved = location.clone();
                resolved.set_index_path_head(self.item.index());
                resolved
            }
            oxoffice_docx::StoryKind::TableCell if self.cells => {
                oxoffice_docx::ContentLocation::new(
                    self.resolve_story(location.story()),
                    location.item_kind(),
                    location.index_path().to_vec(),
                )
            }
            _ => location.clone(),
        }
    }

    fn resolve_story(&self, story: &oxoffice_docx::StoryId) -> oxoffice_docx::StoryId {
        let mut resolved = story.clone();
        if self.cells && story.kind() == oxoffice_docx::StoryKind::TableCell {
            resolved.set_owner_index(story.owner_index().wrapping_add(self.item.cells()));
        }
        resolved
    }

    /// The stored form of a current route: a body route's head is 0, and a
    /// main-part cell story ordinal counts from the item's first.
    fn normalize(
        &self,
        location: &oxoffice_docx::ContentLocation,
    ) -> oxoffice_docx::ContentLocation {
        match location.story().kind() {
            oxoffice_docx::StoryKind::Body => {
                let mut stored = location.clone();
                stored.set_index_path_head(0);
                stored
            }
            oxoffice_docx::StoryKind::TableCell if self.cells => {
                oxoffice_docx::ContentLocation::new(
                    self.normalize_story(location.story()),
                    location.item_kind(),
                    location.index_path().to_vec(),
                )
            }
            _ => location.clone(),
        }
    }

    fn normalize_story(&self, story: &oxoffice_docx::StoryId) -> oxoffice_docx::StoryId {
        let mut stored = story.clone();
        if self.cells && story.kind() == oxoffice_docx::StoryKind::TableCell {
            stored.set_owner_index(story.owner_index().wrapping_sub(self.item.cells()));
        }
        stored
    }

    fn map_location(
        location: &WordOwnerLocation,
        content: impl Fn(&oxoffice_docx::ContentLocation) -> oxoffice_docx::ContentLocation,
        story: impl Fn(&oxoffice_docx::StoryId) -> oxoffice_docx::StoryId,
    ) -> WordOwnerLocation {
        match location {
            WordOwnerLocation::Paragraph(location) => {
                WordOwnerLocation::Paragraph(content(location))
            }
            WordOwnerLocation::Table(location) => WordOwnerLocation::Table(content(location)),
            WordOwnerLocation::Run(location, path) => {
                WordOwnerLocation::Run(content(location), path.clone())
            }
            WordOwnerLocation::Hyperlink(location, index) => {
                WordOwnerLocation::Hyperlink(content(location), *index)
            }
            WordOwnerLocation::Cell {
                table,
                row,
                cell,
                story: cell_story,
            } => WordOwnerLocation::Cell {
                table: content(table),
                row: *row,
                cell: *cell,
                story: story(cell_story),
            },
            WordOwnerLocation::Section(index) => WordOwnerLocation::Section(*index),
        }
    }

    /// [`Self::normalize_location`] in place: a body route keeps its path.
    fn normalize_owned(&self, mut location: WordOwnerLocation) -> WordOwnerLocation {
        let content = |route: &mut oxoffice_docx::ContentLocation| match route.story().kind() {
            oxoffice_docx::StoryKind::Body => route.set_index_path_head(0),
            oxoffice_docx::StoryKind::TableCell if self.cells => *route = self.normalize(route),
            _ => {}
        };
        match &mut location {
            WordOwnerLocation::Paragraph(route)
            | WordOwnerLocation::Table(route)
            | WordOwnerLocation::Run(route, _)
            | WordOwnerLocation::Hyperlink(route, _) => content(route),
            WordOwnerLocation::Cell { table, story, .. } => {
                content(table);
                if self.cells && story.kind() == oxoffice_docx::StoryKind::TableCell {
                    story.set_owner_index(story.owner_index().wrapping_sub(self.item.cells()));
                }
            }
            WordOwnerLocation::Section(_) => {}
        }
        location
    }

    fn resolve_location(&self, location: &WordOwnerLocation) -> WordOwnerLocation {
        Self::map_location(
            location,
            |content| self.resolve(content),
            |story| self.resolve_story(story),
        )
    }

    fn normalize_location(&self, location: &WordOwnerLocation) -> WordOwnerLocation {
        Self::map_location(
            location,
            |content| self.normalize(content),
            |story| self.normalize_story(story),
        )
    }
}

/// The key of the route index for a stored route and its item.
pub(super) fn route_key(route: &WordOwnerLocation, anchor: Option<u64>) -> u64 {
    let hash = route.route_hash();
    match anchor {
        None => hash,
        Some(key) => (hash ^ key.wrapping_mul(0x9e37_79b9_7f4a_7c15)).rotate_left(29),
    }
}

/// The body story content route of an owner's location: its own, or for a
/// cell its table's.
fn content_route(location: &WordOwnerLocation) -> Option<&oxoffice_docx::ContentLocation> {
    match location {
        WordOwnerLocation::Paragraph(location)
        | WordOwnerLocation::Table(location)
        | WordOwnerLocation::Run(location, _)
        | WordOwnerLocation::Hyperlink(location, _) => Some(location),
        WordOwnerLocation::Cell { table, .. } => Some(table),
        WordOwnerLocation::Section(_) => None,
    }
}

/// A route of a facade projection, stored relative to its body item when it
/// has one.
#[derive(Clone)]
pub(super) struct AnchoredRoute {
    location: oxoffice_docx::ContentLocation,
    anchor: Option<WordAnchor>,
}

impl AnchoredRoute {
    /// The route as the document resolves it now.
    pub(super) fn location(&self) -> oxoffice_docx::ContentLocation {
        match &self.anchor {
            Some(anchor) => anchor.resolve(&self.location),
            None => self.location.clone(),
        }
    }

    /// The body story item index at the head of a body route.
    pub(super) fn head(&self) -> Option<usize> {
        if self.location.story().kind() != oxoffice_docx::StoryKind::Body {
            return None;
        }
        match &self.anchor {
            Some(anchor) => Some(anchor.item.index()),
            None => self.location.index_path().first().copied(),
        }
    }

    /// Whether the route names the same element as `location`.
    pub(super) fn names(&self, location: &oxoffice_docx::ContentLocation) -> bool {
        same_content_owner(&self.location(), location)
    }
}

/// The records of the body items that hold owners, in body order, and what
/// a splice may move.
#[derive(Default)]
pub(super) struct WordBodyItems {
    /// One record per body story item that holds owners, by item index.
    items: Vec<Arc<BodyItem>>,
    /// The body story, from its first owner.
    story: Option<oxoffice_docx::StoryId>,
    /// Main-part cell story ordinals are stored relative to their items.
    cells: bool,
    /// Every owner of the body is stored relative to its record, and the
    /// records agree with each other: a splice may move them.
    ready: bool,
    /// Body paragraph owners, body table owners and main-part cell stories.
    totals: (usize, usize, usize),
}

impl WordBodyItems {
    /// The records of an inventory read from the document, with its
    /// projections. Records that would not agree with the walk are left out
    /// (no item is anchored), and a splice then takes the whole route.
    pub(super) fn derive(
        addresses: &[WordOwnerAddress],
        projections: &WordIndexProjections,
    ) -> Self {
        Self::derive_checked(addresses, projections).unwrap_or_default()
    }

    fn derive_checked(
        addresses: &[WordOwnerAddress],
        projections: &WordIndexProjections,
    ) -> Option<Self> {
        let body = addresses.iter().find_map(|address| {
            content_route(&address.location)
                .map(oxoffice_docx::ContentLocation::story)
                .filter(|story| story.kind() == oxoffice_docx::StoryKind::Body)
        })?;
        let part = body.part_name();
        let body_head = |location: &oxoffice_docx::ContentLocation| {
            (location.story().kind() == oxoffice_docx::StoryKind::Body
                && location.story().part_name() == part)
                .then(|| location.index_path().first().copied())
                .flatten()
        };
        // The ordinals one item holds, which must follow those of the items
        // before it.
        #[derive(Clone, Copy, Default)]
        struct Span {
            first: usize,
            last: usize,
            count: usize,
        }
        impl Span {
            fn add(&mut self, ordinal: usize) {
                if self.count == 0 {
                    (self.first, self.last) = (ordinal, ordinal);
                } else {
                    self.first = self.first.min(ordinal);
                    self.last = self.last.max(ordinal);
                }
                self.count += 1;
            }

            fn follows(&self, base: usize) -> bool {
                self.count == 0 || (self.first == base && self.last + 1 == base + self.count)
            }
        }
        // Per item index: whether it holds owners, its slot, and the
        // projection ordinals of its body paragraphs and tables.
        #[derive(Clone, Copy, Default)]
        struct Facts {
            owned: bool,
            slot: Option<usize>,
            paragraphs: Span,
            tables: Span,
            cells: Span,
            links: usize,
        }
        let mut items = Vec::<Facts>::new();
        for address in addresses {
            let Some(route) = content_route(&address.location) else {
                continue;
            };
            if route.story().kind() == oxoffice_docx::StoryKind::Body && body_head(route).is_none()
            {
                return None;
            }
            let Some(head) = body_head(route) else {
                continue;
            };
            if items.len() <= head {
                items.resize(head + 1, Facts::default());
            }
            let facts = &mut items[head];
            facts.owned = true;
            match &address.location {
                WordOwnerLocation::Paragraph(_) => {
                    facts.paragraphs.add(address.projection_ordinal?)
                }
                WordOwnerLocation::Table(_) => facts.tables.add(address.projection_ordinal?),
                WordOwnerLocation::Hyperlink(..) => facts.links += 1,
                _ => {}
            }
            if matches!(
                address.location,
                WordOwnerLocation::Paragraph(_) | WordOwnerLocation::Table(_)
            ) && let Some(slot) = address.body_slot
            {
                match facts.slot {
                    Some(seen) if seen != slot => return None,
                    _ => facts.slot = Some(slot),
                }
            }
        }
        for location in projections.body_paragraphs.iter().flatten().flatten() {
            if !items.get(body_head(location)?)?.owned {
                return None;
            }
        }
        // Main-part cell stories, by the item of the body table that holds
        // them, through nested tables.
        let mut cells_anchored = true;
        let mut cell_items = HashMap::<usize, usize>::new();
        let mut referenced = std::collections::HashSet::new();
        let main_cell = |story: &oxoffice_docx::StoryId| {
            story.kind() == oxoffice_docx::StoryKind::TableCell && story.part_name() == part
        };
        for address in addresses {
            if let Some(route) = content_route(&address.location) {
                let story = route.story();
                if story.part_name() == part && story.kind() == oxoffice_docx::StoryKind::TextBox {
                    cells_anchored = false;
                }
                if main_cell(story) {
                    referenced.insert(story.owner_index());
                }
            }
            let WordOwnerLocation::Cell { story, .. } = &address.location else {
                continue;
            };
            if !main_cell(story) {
                continue;
            }
            referenced.insert(story.owner_index());
            // The body table that holds this cell, through nested tables.
            let mut table = address.parent;
            let root = loop {
                let Some(holder) = table.and_then(|table| addresses.get(table)) else {
                    break None;
                };
                match &holder.location {
                    WordOwnerLocation::Table(location) => {
                        if let Some(head) = body_head(location) {
                            break Some(head);
                        }
                        table = holder
                            .parent
                            .and_then(|cell| addresses.get(cell))
                            .and_then(|cell| cell.parent);
                    }
                    _ => break None,
                }
            };
            match root {
                Some(root) if cell_items.insert(story.owner_index(), root).is_none() => {}
                _ => cells_anchored = false,
            }
        }
        for locations in projections.cell_paragraphs.values() {
            for location in locations {
                if main_cell(location.story()) {
                    referenced.insert(location.story().owner_index());
                }
            }
        }
        if referenced
            .iter()
            .any(|ordinal| !cell_items.contains_key(ordinal))
        {
            cells_anchored = false;
        }
        if cells_anchored {
            for (ordinal, item) in &cell_items {
                match items.get_mut(*item) {
                    Some(facts) if facts.owned => facts.cells.add(*ordinal),
                    _ => cells_anchored = false,
                }
            }
        }
        // The records, each base the count of what comes before.
        let mut records = Vec::with_capacity(items.len());
        let (mut paragraphs, mut tables, mut cells) = (0, 0, 0);
        let mut last_slot = 0;
        for (index, facts) in items.into_iter().enumerate() {
            if !facts.owned {
                continue;
            }
            if let Some(slot) = facts.slot {
                if slot < last_slot {
                    return None;
                }
                last_slot = slot;
            }
            if !facts.paragraphs.follows(paragraphs) || !facts.tables.follows(tables) {
                return None;
            }
            if cells_anchored && !facts.cells.follows(cells) {
                cells_anchored = false;
            }
            let counts = (
                facts.paragraphs.count,
                facts.tables.count,
                facts.cells.count,
            );
            records.push(BodyItem::new(
                index,
                facts.slot,
                (paragraphs, tables, cells),
                counts,
                facts.links,
                last_slot,
            ));
            paragraphs += counts.0;
            tables += counts.1;
            cells += counts.2;
        }
        if cells_anchored && cells != referenced.len() {
            cells_anchored = false;
        }
        Some(Self {
            items: records,
            story: Some(body.clone()),
            cells: cells_anchored,
            ready: true,
            totals: (paragraphs, tables, cells),
        })
    }

    /// Room for `additional` records.
    pub(super) fn reserve(
        &mut self,
        additional: usize,
    ) -> Result<(), std::collections::TryReserveError> {
        self.items.try_reserve(additional)
    }

    /// Whether a splice may move the records.
    pub(super) fn ready(&self) -> bool {
        self.ready
    }

    /// Whether main-part cell stories follow their items, so that a table
    /// inserted before others moves their ordinals with its record.
    pub(super) fn cells_anchored(&self) -> bool {
        self.cells
    }

    /// The body story.
    pub(super) fn story(&self) -> Option<&oxoffice_docx::StoryId> {
        self.story.as_ref()
    }

    /// The position in the records of the first item at or after `index`.
    fn position(&self, index: usize) -> usize {
        self.items.partition_point(|item| item.index() < index)
    }

    /// The record of body story item `index`.
    pub(super) fn at(&self, index: usize) -> Option<&Arc<BodyItem>> {
        self.items
            .get(self.position(index))
            .filter(|item| item.index() == index)
    }

    /// The record of the first item that direct body slot `slot` holds.
    pub(super) fn at_slot(&self, slot: usize) -> Option<&Arc<BodyItem>> {
        self.items[self.items.partition_point(|item| item.slot() < slot)..]
            .iter()
            .take_while(|item| item.slot() == slot)
            .find(|item| item.has_slot)
    }

    /// The record of the item that holds main-part cell story `ordinal`.
    fn of_cell(&self, ordinal: usize) -> Option<&Arc<BodyItem>> {
        let at = self.items.partition_point(|item| item.cells() <= ordinal);
        let item = self.items.get(at.checked_sub(1)?)?;
        (ordinal < item.cells() + item.counts.2).then_some(item)
    }

    /// What comes before body story item `index`: body paragraph owners,
    /// body table owners and main-part cell stories.
    pub(super) fn before(&self, index: usize) -> (usize, usize, usize) {
        match self.items.get(self.position(index)) {
            Some(item) => (item.paragraphs(), item.tables(), item.cells()),
            None => self.totals,
        }
    }

    /// The item a current route is stored relative to.
    pub(super) fn anchor_of(&self, location: &WordOwnerLocation) -> Option<WordAnchor> {
        let route = content_route(location)?;
        self.anchor_of_content(route)
    }

    fn anchor_of_content(&self, route: &oxoffice_docx::ContentLocation) -> Option<WordAnchor> {
        if !self.ready {
            return None;
        }
        let story = route.story();
        if story.part_name() != self.story.as_ref()?.part_name() {
            return None;
        }
        let item = match story.kind() {
            oxoffice_docx::StoryKind::Body => self.at(*route.index_path().first()?)?,
            oxoffice_docx::StoryKind::TableCell if self.cells => {
                self.of_cell(story.owner_index())?
            }
            _ => return None,
        };
        Some(WordAnchor {
            item: Arc::clone(item),
            cells: self.cells,
        })
    }

    /// A current route in its stored form and the key of its item, as the
    /// route index holds it.
    pub(super) fn stored<'a>(
        &self,
        location: &'a WordOwnerLocation,
    ) -> (Cow<'a, WordOwnerLocation>, Option<u64>) {
        match self.anchor_of(location) {
            Some(anchor) => (
                Cow::Owned(anchor.normalize_location(location)),
                Some(anchor.key()),
            ),
            None => (Cow::Borrowed(location), None),
        }
    }

    /// A projection route, stored relative to its item.
    pub(super) fn route(&self, location: oxoffice_docx::ContentLocation) -> AnchoredRoute {
        match self.anchor_of_content(&location) {
            Some(anchor) => AnchoredRoute {
                location: anchor.normalize(&location),
                anchor: Some(anchor),
            },
            None => AnchoredRoute {
                location,
                anchor: None,
            },
        }
    }

    /// Insert the record of body story item `index` in direct body slot
    /// `slot`, holding `counts` body paragraphs, body tables and main-part
    /// cell stories, and move the records after it.
    pub(super) fn insert(&mut self, index: usize, slot: usize, counts: (usize, usize, usize)) {
        let at = self.position(index);
        let before = self.before(index);
        let shift = BodyShift {
            items: 1,
            slots: 1,
            paragraphs: counts.0 as isize,
            tables: counts.1 as isize,
            cells: counts.2 as isize,
        };
        for item in &self.items[at..] {
            item.shift(shift);
        }
        self.items.insert(
            at,
            BodyItem::new(index, Some(slot), before, counts, 0, slot),
        );
        self.totals.0 += counts.0;
        self.totals.1 += counts.1;
        self.totals.2 += counts.2;
    }

    /// Remove the records of the `count` items from body story item `index`,
    /// in as many direct body slots, and move the records after them.
    pub(super) fn remove(&mut self, index: usize, count: usize) {
        let start = self.position(index);
        let end = self.position(index + count);
        let mut counts = (0, 0, 0);
        for item in self.items.drain(start..end) {
            counts.0 += item.counts.0;
            counts.1 += item.counts.1;
            counts.2 += item.counts.2;
        }
        let shift = BodyShift {
            items: -(count as isize),
            slots: -(count as isize),
            paragraphs: -(counts.0 as isize),
            tables: -(counts.1 as isize),
            cells: -(counts.2 as isize),
        };
        for item in &self.items[start..] {
            item.shift(shift);
        }
        self.totals.0 -= counts.0;
        self.totals.1 -= counts.1;
        self.totals.2 -= counts.2;
    }

    /// Whether a record lies strictly between body story items `first` and
    /// `end`.
    #[cfg(debug_assertions)]
    pub(super) fn any_between(&self, first: usize, end: usize) -> bool {
        self.items
            .get(self.position(first.saturating_add(1)))
            .is_some_and(|item| item.index() < end)
    }

    /// Whether an item after body story item `index` holds a body story
    /// hyperlink. Integer work on the records after it, as a splice moves
    /// them.
    pub(super) fn links_after(&self, index: usize) -> bool {
        self.items[self.position(index.saturating_add(1))..]
            .iter()
            .any(|item| item.links != 0)
    }

    /// Move the records after body story item `index` by `items` body story
    /// items: an edit of the block at `index` changed how many it holds,
    /// and nothing else they count.
    pub(super) fn shift_after(&mut self, index: usize, items: isize) {
        if items == 0 {
            return;
        }
        let shift = BodyShift {
            items,
            ..BodyShift::default()
        };
        for item in &self.items[self.position(index.saturating_add(1))..] {
            item.shift(shift);
        }
    }

    /// Under debug assertions, check the records against those derived from
    /// a full walk of the document.
    #[cfg(debug_assertions)]
    pub(super) fn assert_rebuilds(&self, rebuilt: &Self) {
        if !self.ready {
            return;
        }
        assert!(rebuilt.ready, "a walk of a spliced body has records");
        let facts = |items: &Self| {
            items
                .items
                .iter()
                .map(|item| item.facts())
                .collect::<Vec<_>>()
        };
        assert_eq!(facts(self), facts(rebuilt), "body item records");
        assert_eq!(self.totals, rebuilt.totals, "body item totals");
        if self.cells {
            assert!(rebuilt.cells, "spliced cell stories follow their items");
        }
    }
}

// Only current physical owners are retained. A committed mutation supplies
// the replacement inventory before any state is published.
pub(crate) struct WordOwner {
    /// The route as stored: relative to `anchor` when it has one.
    route: WordOwnerLocation,
    anchor: Option<WordAnchor>,
    pub(super) parent: Option<usize>,
    /// For an anchored owner, whether it has one: then its item's.
    body_slot: Option<usize>,
    /// For an anchored body paragraph or body table, counted from the
    /// item's first.
    projection_ordinal: Option<usize>,
    pub(super) read_facts: WordReadFacts,
    pub(super) owner: RevisionState,
    pub(super) content: Option<RevisionState>,
    pub(super) properties: Option<RevisionState>,
    pub(super) grid: Option<RevisionState>,
}

impl WordOwner {
    /// The owner of `address` with its scope states, stored relative to its
    /// body item when `items` holds one.
    pub(super) fn new(
        address: WordOwnerAddress,
        scopes: &[RevisionState],
        items: &WordBodyItems,
    ) -> Self {
        let WordOwnerAddress {
            location,
            parent,
            body_slot,
            projection_ordinal,
            read_facts,
        } = address;
        let (content, properties, grid) = match &location {
            WordOwnerLocation::Paragraph(_) => {
                (Some(scopes[1].clone()), Some(scopes[2].clone()), None)
            }
            WordOwnerLocation::Table(_) => (None, None, Some(scopes[1].clone())),
            WordOwnerLocation::Hyperlink(..) => (None, None, None),
            _ => (Some(scopes[1].clone()), None, None),
        };
        let anchor = items.anchor_of(&location);
        let (route, projection_ordinal) = match &anchor {
            Some(anchor) => {
                let base = match &location {
                    WordOwnerLocation::Paragraph(route)
                        if route.story().kind() == oxoffice_docx::StoryKind::Body =>
                    {
                        anchor.item.paragraphs()
                    }
                    WordOwnerLocation::Table(route)
                        if route.story().kind() == oxoffice_docx::StoryKind::Body =>
                    {
                        anchor.item.tables()
                    }
                    _ => 0,
                };
                (
                    anchor.normalize_owned(location),
                    projection_ordinal.map(|ordinal| ordinal.wrapping_sub(base)),
                )
            }
            None => (location, projection_ordinal),
        };
        Self {
            route,
            anchor,
            parent,
            body_slot,
            projection_ordinal,
            read_facts,
            owner: scopes[0].clone(),
            content,
            properties,
            grid,
        }
    }

    /// The owner of `address` with the states of `previous`.
    pub(super) fn with_states_of(
        address: WordOwnerAddress,
        previous: &WordOwner,
        items: &WordBodyItems,
    ) -> Self {
        let owner_state = previous.owner.clone();
        let mut owner = Self::new(
            address,
            &[owner_state.clone(), owner_state.clone(), owner_state],
            items,
        );
        owner.content = previous.content.clone();
        owner.properties = previous.properties.clone();
        owner.grid = previous.grid.clone();
        owner
    }

    /// The owner's route as the document resolves it now.
    pub(super) fn location(&self) -> Cow<'_, WordOwnerLocation> {
        match &self.anchor {
            Some(anchor) => Cow::Owned(anchor.resolve_location(&self.route)),
            None => Cow::Borrowed(&self.route),
        }
    }

    /// The route as stored, to read its kind, its story kind and the parts
    /// of it no splice moves (run path, row, cell, hyperlink index). Its
    /// body item index and main-part cell story ordinal may be relative:
    /// read them through [`Self::location`].
    pub(super) fn route_shape(&self) -> &WordOwnerLocation {
        &self.route
    }

    /// The owner's route, read for its stories: the stored route when it is
    /// a body route (whose story a splice never changes, only its head),
    /// else the route as the document resolves it now.
    pub(super) fn story_route(&self) -> Cow<'_, WordOwnerLocation> {
        match &self.route {
            WordOwnerLocation::Paragraph(route)
            | WordOwnerLocation::Table(route)
            | WordOwnerLocation::Run(route, _)
                if route.story().kind() == oxoffice_docx::StoryKind::Body =>
            {
                Cow::Borrowed(&self.route)
            }
            _ => self.location(),
        }
    }

    /// The direct body slot of a body paragraph or table.
    pub(super) fn body_slot(&self) -> Option<usize> {
        match &self.anchor {
            Some(anchor) => self.body_slot.map(|_| anchor.item.slot()),
            None => self.body_slot,
        }
    }

    /// The owner's ordinal in its story among owners of its kind.
    pub(super) fn projection_ordinal(&self) -> Option<usize> {
        let ordinal = self.projection_ordinal?;
        let Some(anchor) = &self.anchor else {
            return Some(ordinal);
        };
        let base = match &self.route {
            WordOwnerLocation::Paragraph(route)
                if route.story().kind() == oxoffice_docx::StoryKind::Body =>
            {
                anchor.item.paragraphs()
            }
            WordOwnerLocation::Table(route)
                if route.story().kind() == oxoffice_docx::StoryKind::Body =>
            {
                anchor.item.tables()
            }
            _ => 0,
        };
        Some(ordinal.wrapping_add(base))
    }

    /// The key of the owner's stored route in the route index.
    pub(super) fn route_key(&self) -> u64 {
        route_key(&self.route, self.anchor.as_ref().map(WordAnchor::key))
    }

    /// Whether the owner has the stored route of [`WordBodyItems::stored`].
    pub(super) fn has_stored_route(&self, route: &WordOwnerLocation, anchor: Option<u64>) -> bool {
        self.anchor.as_ref().map(WordAnchor::key) == anchor && self.route.same_route(route)
    }

    /// The owner's address as the document resolves it now.
    pub(super) fn address(&self) -> WordOwnerAddress {
        WordOwnerAddress {
            location: self.location().into_owned(),
            parent: self.parent,
            body_slot: self.body_slot(),
            projection_ordinal: self.projection_ordinal(),
            read_facts: self.read_facts.clone(),
        }
    }
}

/// A debug copy of an owner resolves its route: it keeps naming what the
/// owner named when it was taken, whatever later splices move.
#[cfg(debug_assertions)]
impl Clone for WordOwner {
    fn clone(&self) -> Self {
        Self {
            route: self.location().into_owned(),
            anchor: None,
            parent: self.parent,
            body_slot: self.body_slot(),
            projection_ordinal: self.projection_ordinal(),
            read_facts: self.read_facts.clone(),
            owner: self.owner.clone(),
            content: self.content.clone(),
            properties: self.properties.clone(),
            grid: self.grid.clone(),
        }
    }
}
