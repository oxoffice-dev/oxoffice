//! Word owners: where the owner of each live handle sits, its read facts, and the indexes over them.

use super::*;

// Only current physical owners are retained. A committed mutation supplies the
// replacement inventory before any state is published.
#[derive(Clone)]
#[cfg_attr(debug_assertions, derive(Debug, PartialEq, Eq))]
pub(crate) enum WordOwnerLocation {
    Paragraph(oxoffice_docx::ContentLocation),
    Run(
        oxoffice_docx::ContentLocation,
        oxoffice_docx_oxml::text::AcceptedRunPath,
    ),
    Table(oxoffice_docx::ContentLocation),
    Cell {
        table: oxoffice_docx::ContentLocation,
        row: usize,
        cell: usize,
        story: oxoffice_docx::StoryId,
    },
    Section(usize),
    Hyperlink(oxoffice_docx::ContentLocation, usize),
}

#[derive(Clone, Default)]
#[cfg_attr(debug_assertions, derive(Debug, PartialEq, Eq))]
pub(super) enum WordReadFacts {
    #[default]
    None,
    Paragraph(String),
    // One entry per modeled row/cell, containing the facade paragraph count.
    Table(Vec<Vec<usize>>),
}

#[derive(Clone, Copy)]
pub(crate) enum WordView {
    Owner,
    Content,
    Properties,
    Grid,
}

/// A multiply-rotate hasher over machine words, for owner routes and the
/// owner index maps. Their keys are process-local integers and route
/// hashes whose bucket members are still compared with `same_route`, so
/// they need no keyed hash, and a body edit indexes every owner again.
#[derive(Default)]
pub(super) struct IndexHasher(u64);

/// A map of the owner indexes.
pub(super) type IndexMap<K, V> = HashMap<K, V, std::hash::BuildHasherDefault<IndexHasher>>;

impl Hasher for IndexHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.write_u64(u64::from_le_bytes(word));
        }
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = (self.0.rotate_left(5) ^ value).wrapping_mul(0x517c_c1b7_2722_0a95);
    }

    fn write_usize(&mut self, value: usize) {
        self.write_u64(value as u64);
    }
}

impl WordOwnerLocation {
    pub(super) fn route_hash(&self) -> u64 {
        let mut hash = IndexHasher::default();
        std::mem::discriminant(self).hash(&mut hash);
        let mut content = |location: &oxoffice_docx::ContentLocation| {
            let story = location.story();
            story.kind().hash(&mut hash);
            // Equal routes share their part, so its length tells parts apart
            // well enough for buckets.
            story.part_name().len().hash(&mut hash);
            story.owner_index().hash(&mut hash);
            location.item_kind().hash(&mut hash);
            location.index_path().hash(&mut hash);
        };
        match self {
            Self::Paragraph(location) | Self::Table(location) => content(location),
            Self::Run(location, path) => {
                content(location);
                path.hash(&mut hash);
            }
            Self::Cell {
                table, row, cell, ..
            } => {
                content(table);
                row.hash(&mut hash);
                cell.hash(&mut hash);
            }
            Self::Section(index) => index.hash(&mut hash),
            Self::Hyperlink(location, index) => {
                content(location);
                index.hash(&mut hash);
            }
        }
        hash.finish()
    }
}

/// The routes of the paragraphs of each cell of each facade table, by
/// facade table index, then by row and cell.
pub(super) type CellParagraphIndex = Vec<HashMap<(usize, usize), Vec<AnchoredRoute>>>;

impl WordOwnerIndexes {
    fn empty(items: WordBodyItems, owners: usize) -> PyResult<Self> {
        let mut indexes = Self {
            items,
            by_generation: IndexMap::default(),
            by_route: IndexMap::default(),
            direct_body_paragraphs: Vec::new(),
            body_paragraphs: None,
            body_tables: Vec::new(),
            cell_paragraphs: Vec::new(),
            paragraphs_by_parent: IndexMap::default(),
            tables_by_parent: IndexMap::default(),
            cells_by_parent: IndexMap::default(),
            runs_by_parent: IndexMap::default(),
        };
        let fail =
            |_| pyo3::exceptions::PyMemoryError::new_err("Word owner index allocation failed");
        indexes.by_generation.try_reserve(owners).map_err(fail)?;
        indexes.by_route.try_reserve(owners).map_err(fail)?;
        Ok(indexes)
    }

    /// The projections of the document, stored relative to the body items,
    /// with the body table owner route of each direct body slot.
    fn project(
        &mut self,
        projections: WordIndexProjections,
        tables: &HashMap<usize, AnchoredRoute>,
    ) {
        let WordIndexProjections {
            body_paragraphs,
            cell_paragraphs,
            table_slots,
        } = projections;
        let items = &self.items;
        self.body_paragraphs = body_paragraphs.map(|locations| {
            locations
                .into_iter()
                .map(|location| location.map(|location| items.route(location)))
                .collect()
        });
        self.body_tables = table_slots
            .iter()
            .map(|slot| slot.and_then(|slot| tables.get(&slot).cloned()))
            .collect();
        let mut cells = vec![HashMap::new(); table_slots.len()];
        for ((table, row, cell), locations) in cell_paragraphs {
            if table >= cells.len() {
                cells.resize_with(table + 1, HashMap::new);
            }
            cells[table].insert(
                (row, cell),
                locations
                    .into_iter()
                    .map(|location| items.route(location))
                    .collect(),
            );
        }
        self.cell_paragraphs = cells;
    }

    /// Index `owners`, which are stored relative to `items`.
    pub(super) fn prepare(
        owners: &[WordOwner],
        items: WordBodyItems,
        projections: WordIndexProjections,
    ) -> PyResult<Self> {
        let mut indexes = Self::empty(items, owners.len())?;
        let mut tables = HashMap::new();
        for (key, owner) in owners.iter().enumerate() {
            indexes.by_generation.insert(owner.owner.generation(), key);
            let body_slot = owner.body_slot();
            indexes.index_entry(
                key,
                owner.route_key(),
                owner.route_shape(),
                owner.parent,
                body_slot,
            );
            // Only a body table is resolved: its route in the table projection.
            if let (WordOwnerLocation::Table(shape), Some(slot)) = (owner.route_shape(), body_slot)
                && shape.story().kind() == oxoffice_docx::StoryKind::Body
                && !tables.contains_key(&slot)
                && let WordOwnerLocation::Table(location) = owner.location().into_owned()
            {
                tables.insert(slot, indexes.items.route(location));
            }
        }
        indexes.project(projections, &tables);
        Ok(indexes)
    }

    /// Index the owners of `addresses` before they hold states: an owner's
    /// key is its address's index. Its generation is indexed with its states.
    pub(super) fn prepare_addresses(
        addresses: &[WordOwnerAddress],
        items: WordBodyItems,
        projections: WordIndexProjections,
    ) -> PyResult<Self> {
        let mut indexes = Self::empty(items, addresses.len())?;
        let mut tables = HashMap::new();
        for (key, address) in addresses.iter().enumerate() {
            let (route, anchor) = indexes.items.stored(&address.location);
            let hash = route_key(&route, anchor);
            indexes.index_entry(key, hash, &route, address.parent, address.body_slot);
            if let (WordOwnerLocation::Table(location), Some(slot)) =
                (&address.location, address.body_slot)
                && location.story().kind() == oxoffice_docx::StoryKind::Body
            {
                tables
                    .entry(slot)
                    .or_insert_with(|| indexes.items.route(location.clone()));
            }
        }
        indexes.project(projections, &tables);
        Ok(indexes)
    }

    /// Drop the entries of the owner at `key`, whose children go with it:
    /// the caller indexes its key again for another owner, or for none.
    pub(super) fn unindex_owner(&mut self, key: usize, owner: &WordOwner) {
        if self.by_generation.get(&owner.owner.generation()) == Some(&key) {
            self.by_generation.remove(&owner.owner.generation());
        }
        let hash = owner.route_key();
        if let Some(bucket) = self.by_route.get_mut(&hash) {
            bucket.retain(|known| *known != key);
            if bucket.is_empty() {
                self.by_route.remove(&hash);
            }
        }
        if let Some(slot) = owner.body_slot()
            && self.direct_body_paragraphs.get(slot) == Some(&Some(key))
        {
            self.direct_body_paragraphs[slot] = None;
        }
        for children in [
            &mut self.paragraphs_by_parent,
            &mut self.tables_by_parent,
            &mut self.cells_by_parent,
            &mut self.runs_by_parent,
        ] {
            children.remove(&key);
            if let Some(parent) = owner.parent
                && let Some(siblings) = children.get_mut(&parent)
            {
                siblings.retain(|known| *known != key);
                if siblings.is_empty() {
                    children.remove(&parent);
                }
            }
        }
    }

    /// Index one owner at `key`.
    pub(super) fn index_owner(&mut self, key: usize, owner: &WordOwner) {
        self.by_generation.insert(owner.owner.generation(), key);
        self.index_entry(
            key,
            owner.route_key(),
            owner.route_shape(),
            owner.parent,
            owner.body_slot(),
        );
    }

    /// The route and parent indexes of one owner. `route` is read for its
    /// kind, its story kind and its path length alone.
    fn index_entry(
        &mut self,
        key: usize,
        hash: u64,
        route: &WordOwnerLocation,
        parent: Option<usize>,
        body_slot: Option<usize>,
    ) {
        self.by_route.entry(hash).or_default().push(key);
        match route {
            WordOwnerLocation::Paragraph(location) => {
                if location.story().kind() == oxoffice_docx::StoryKind::Body
                    && location.index_path().len() == 1
                    && let Some(slot) = body_slot
                {
                    if self.direct_body_paragraphs.len() <= slot {
                        self.direct_body_paragraphs.resize(slot + 1, None);
                    }
                    self.direct_body_paragraphs[slot].get_or_insert(key);
                }
                if let Some(parent) = parent {
                    self.paragraphs_by_parent
                        .entry(parent)
                        .or_default()
                        .push(key);
                }
            }
            WordOwnerLocation::Table(_) => {
                if let Some(parent) = parent {
                    self.tables_by_parent.entry(parent).or_default().push(key);
                }
            }
            WordOwnerLocation::Cell { .. } => {
                if let Some(parent) = parent {
                    self.cells_by_parent.entry(parent).or_default().push(key);
                }
            }
            WordOwnerLocation::Run(..) => {
                if let Some(parent) = parent {
                    self.runs_by_parent.entry(parent).or_default().push(key);
                }
            }
            _ => {}
        }
    }

    /// The key of the owner at a current route.
    pub(super) fn find(&self, owners: &[WordOwner], location: &WordOwnerLocation) -> Option<usize> {
        let (route, anchor) = self.items.stored(location);
        self.by_route
            .get(&route_key(&route, anchor))?
            .iter()
            .copied()
            .find(|key| owners[*key].has_stored_route(&route, anchor))
    }

    /// The direct body paragraph owner of direct body slot `slot`.
    pub(super) fn direct_body_paragraph(&self, slot: usize) -> Option<usize> {
        self.direct_body_paragraphs.get(slot).copied().flatten()
    }
}

pub(super) fn word_owner_capture(
    counter: &RevisionCounter,
    owners: &[WordOwner],
    document_scope: RevisionState,
    index: usize,
    segments: smallvec::SmallVec<[PathSeg; 5]>,
    view: WordView,
    furniture_scope: Option<&RevisionState>,
) -> PyResult<(ContentPath, RevisionState)> {
    let target = &owners[index];
    // Every structural publisher retires unproved current-owner states. Actual
    // Run handles retain the conservative document epoch and its first cause.
    let mut states = vec![target.owner.clone()];
    if matches!(target.route_shape(), WordOwnerLocation::Run(..)) {
        states.push(document_scope);
    }
    if let Some(scope) = furniture_scope {
        states.push(scope.clone());
    }
    let child = match view {
        WordView::Owner => None,
        WordView::Content => target.content.as_ref(),
        WordView::Properties => target.properties.as_ref(),
        WordView::Grid => target.grid.as_ref(),
    };
    if let Some(child) = child {
        states.push(child.clone());
    }
    let mut parent = target.parent;
    while let Some(index) = parent {
        let ancestor = &owners[index];
        states.push(ancestor.owner.clone());
        if let Some(content) = &ancestor.content {
            states.push(content.clone());
        }
        if let Some(grid) = &ancestor.grid {
            states.push(grid.clone());
        }
        parent = ancestor.parent;
    }
    let path = counter
        .capture_scopes(segments, &states)
        .map_err(revision_to_pyerr)?;
    Ok((path, target.owner.clone()))
}

pub(super) fn same_story_owner(a: &oxoffice_docx::StoryId, b: &oxoffice_docx::StoryId) -> bool {
    a.kind() == b.kind() && a.part_name() == b.part_name() && a.owner_index() == b.owner_index()
}

pub(super) fn same_content_owner(
    a: &oxoffice_docx::ContentLocation,
    b: &oxoffice_docx::ContentLocation,
) -> bool {
    same_story_owner(a.story(), b.story())
        && a.item_kind() == b.item_kind()
        && a.index_path() == b.index_path()
}

impl WordOwnerLocation {
    pub(super) fn same_route(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Paragraph(a), Self::Paragraph(b)) | (Self::Table(a), Self::Table(b)) => {
                same_content_owner(a, b)
            }
            (Self::Run(a, ap), Self::Run(b, bp)) => same_content_owner(a, b) && ap == bp,
            (
                Self::Cell {
                    table: a,
                    row: ar,
                    cell: ac,
                    ..
                },
                Self::Cell {
                    table: b,
                    row: br,
                    cell: bc,
                    ..
                },
            ) => same_content_owner(a, b) && ar == br && ac == bc,
            (Self::Section(a), Self::Section(b)) => a == b,
            (Self::Hyperlink(a, ai), Self::Hyperlink(b, bi)) => {
                same_content_owner(a, b) && ai == bi
            }
            _ => false,
        }
    }

    pub(super) fn scopes(&self) -> &'static [RevisionScope] {
        match self {
            Self::Paragraph(_) => &[
                RevisionScope::Owner,
                RevisionScope::Text,
                RevisionScope::Text,
            ],
            Self::Table(_) => &[RevisionScope::Owner, RevisionScope::Grid],
            Self::Hyperlink(..) => &[RevisionScope::Owner],
            Self::Run(..) | Self::Cell { .. } | Self::Section(_) => {
                &[RevisionScope::Owner, RevisionScope::Text]
            }
        }
    }
}
