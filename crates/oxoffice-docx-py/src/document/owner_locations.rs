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

impl WordOwnerIndexes {
    pub(super) fn prepare(
        owners: &[WordOwner],
        projections: WordIndexProjections,
    ) -> PyResult<Self> {
        let WordIndexProjections {
            body_paragraphs,
            cell_paragraphs,
            table_slots,
        } = projections;
        let mut indexes = Self {
            by_generation: IndexMap::default(),
            by_route: IndexMap::default(),
            direct_body_paragraphs: IndexMap::default(),
            body_paragraphs,
            body_tables: Vec::new(),
            cell_paragraphs,
            paragraphs_by_parent: IndexMap::default(),
            tables_by_parent: IndexMap::default(),
            cells_by_parent: IndexMap::default(),
            runs_by_parent: IndexMap::default(),
        };
        let fail =
            |_| pyo3::exceptions::PyMemoryError::new_err("Word owner index allocation failed");
        indexes
            .by_generation
            .try_reserve(owners.len())
            .map_err(fail)?;
        indexes.by_route.try_reserve(owners.len()).map_err(fail)?;
        indexes
            .direct_body_paragraphs
            .try_reserve(owners.len())
            .map_err(fail)?;
        let body_tables = indexes.index_owners(owners);
        indexes.body_tables = table_slots
            .into_iter()
            .map(|slot| slot.and_then(|slot| body_tables.get(&slot).cloned()))
            .collect();
        Ok(indexes)
    }

    pub(super) fn prepare_addresses(
        locations: &[WordOwnerAddress],
        projections: WordIndexProjections,
    ) -> PyResult<Self> {
        let WordIndexProjections {
            body_paragraphs,
            cell_paragraphs,
            table_slots,
        } = projections;
        let mut indexes = Self::prepare(
            &[],
            WordIndexProjections {
                body_paragraphs,
                cell_paragraphs,
                table_slots: Vec::new(),
            },
        )?;
        let fail =
            |_| pyo3::exceptions::PyMemoryError::new_err("Word owner index allocation failed");
        indexes
            .by_generation
            .try_reserve(locations.len())
            .map_err(fail)?;
        indexes
            .by_route
            .try_reserve(locations.len())
            .map_err(fail)?;
        let mut tables = HashMap::new();
        for (index, address) in locations.iter().enumerate() {
            indexes.index_location(
                index,
                &address.location,
                address.parent,
                address.body_slot,
                &mut tables,
            );
        }
        indexes.body_tables = table_slots
            .into_iter()
            .map(|slot| slot.and_then(|slot| tables.get(&slot).cloned()))
            .collect();
        Ok(indexes)
    }

    /// Index `owners` again after they moved in place, keeping the document
    /// projections, which the caller moves with them.
    pub(super) fn index_owners_again(&mut self, owners: &[WordOwner]) {
        self.by_generation.clear();
        self.by_generation.reserve(owners.len());
        // Route buckets keep their allocations, and those left empty go.
        self.by_route.values_mut().for_each(Vec::clear);
        self.by_route
            .reserve(owners.len().saturating_sub(self.by_route.len()));
        self.direct_body_paragraphs.clear();
        self.paragraphs_by_parent.clear();
        self.tables_by_parent.clear();
        self.cells_by_parent.clear();
        self.runs_by_parent.clear();
        self.index_owners(owners);
        self.by_route.retain(|_, bucket| !bucket.is_empty());
    }

    /// Index every owner, and return the first body table owner of each
    /// direct body slot.
    fn index_owners(
        &mut self,
        owners: &[WordOwner],
    ) -> HashMap<usize, oxoffice_docx::ContentLocation> {
        let indexes = self;
        let mut body_tables = HashMap::new();
        for (index, owner) in owners.iter().enumerate() {
            indexes
                .by_generation
                .insert(owner.owner.generation(), index);
            indexes.index_location(
                index,
                &owner.location,
                owner.parent,
                owner.body_slot,
                &mut body_tables,
            );
        }
        body_tables
    }

    /// The same route/parent classification is used before and after scope initialization.
    fn index_location(
        &mut self,
        index: usize,
        location: &WordOwnerLocation,
        parent: Option<usize>,
        body_slot: Option<usize>,
        body_tables: &mut HashMap<usize, oxoffice_docx::ContentLocation>,
    ) {
        self.by_route
            .entry(location.route_hash())
            .or_default()
            .push(index);
        match location {
            WordOwnerLocation::Paragraph(location) => {
                if location.story().kind() == oxoffice_docx::StoryKind::Body
                    && location.index_path().len() == 1
                    && let Some(slot) = body_slot
                {
                    self.direct_body_paragraphs.entry(slot).or_insert(index);
                }
                if let Some(parent) = parent {
                    self.paragraphs_by_parent
                        .entry(parent)
                        .or_default()
                        .push(index);
                }
            }
            WordOwnerLocation::Table(location) => {
                if location.story().kind() == oxoffice_docx::StoryKind::Body
                    && let Some(slot) = body_slot
                {
                    body_tables.entry(slot).or_insert_with(|| location.clone());
                }
                if let Some(parent) = parent {
                    self.tables_by_parent.entry(parent).or_default().push(index);
                }
            }
            WordOwnerLocation::Cell { .. } => {
                if let Some(parent) = parent {
                    self.cells_by_parent.entry(parent).or_default().push(index);
                }
            }
            WordOwnerLocation::Run(..) => {
                if let Some(parent) = parent {
                    self.runs_by_parent.entry(parent).or_default().push(index);
                }
            }
            _ => {}
        }
    }

    pub(super) fn find(&self, owners: &[WordOwner], location: &WordOwnerLocation) -> Option<usize> {
        self.by_route
            .get(&location.route_hash())?
            .iter()
            .copied()
            .find(|index| owners[*index].location.same_route(location))
    }
}

pub(super) fn word_owner_capture(
    counter: &RevisionCounter,
    owners: &[WordOwner],
    document_scopes: (RevisionState, &RevisionState),
    index: usize,
    segments: smallvec::SmallVec<[PathSeg; 5]>,
    view: WordView,
    furniture_scope: Option<&RevisionState>,
) -> PyResult<(ContentPath, RevisionState)> {
    let (document_scope, removal_scope) = document_scopes;
    let target = &owners[index];
    // Every structural publisher retires unproved current-owner states. Actual
    // Run handles retain the conservative document epoch and its first cause.
    let mut states = vec![removal_scope.clone(), target.owner.clone()];
    if matches!(&target.location, WordOwnerLocation::Run(..)) {
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
