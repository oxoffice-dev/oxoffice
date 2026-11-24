//! Matching an old inventory to a new one: route keys, property owners, and owners with their states.

use super::*;

fn same_inventory_route(
    old: &[WordOwner],
    mut before: usize,
    new: &[WordOwnerAddress],
    mut after: usize,
) -> bool {
    loop {
        let record = &old[before];
        let address = &new[after];
        let b = &address.location;
        let parent = address.parent;
        let equal = match (&*record.location(), b) {
            (WordOwnerLocation::Paragraph(a), WordOwnerLocation::Paragraph(b))
                if a.story().kind() == oxoffice_docx::StoryKind::Body
                    && b.story().kind() == oxoffice_docx::StoryKind::Body
                    && record.body_slot().is_some()
                    && address.body_slot.is_some() =>
            {
                record.body_slot() == address.body_slot
                    && a.index_path().len() <= 2
                    && b.index_path().len() <= 2
                    && a.index_path().get(1..) == b.index_path().get(1..)
            }
            (WordOwnerLocation::Table(a), WordOwnerLocation::Table(b))
                if a.story().kind() == oxoffice_docx::StoryKind::Body
                    && b.story().kind() == oxoffice_docx::StoryKind::Body
                    && record.body_slot().is_some()
                    && address.body_slot.is_some() =>
            {
                record.body_slot() == address.body_slot
            }
            (WordOwnerLocation::Paragraph(a), WordOwnerLocation::Paragraph(b))
            | (WordOwnerLocation::Table(a), WordOwnerLocation::Table(b))
                if record.parent.is_some_and(|index| {
                    matches!(old[index].route_shape(), WordOwnerLocation::Cell { .. })
                }) && parent.is_some_and(|index| {
                    matches!(new[index].location, WordOwnerLocation::Cell { .. })
                }) =>
            {
                a.item_kind() == b.item_kind()
                    && record.projection_ordinal() == address.projection_ordinal
            }
            (WordOwnerLocation::Paragraph(a), WordOwnerLocation::Paragraph(b))
            | (WordOwnerLocation::Table(a), WordOwnerLocation::Table(b)) => {
                same_story_owner(a.story(), b.story())
                    && a.item_kind() == b.item_kind()
                    && record.projection_ordinal() == address.projection_ordinal
            }
            (
                WordOwnerLocation::Cell {
                    row: a, cell: ac, ..
                },
                WordOwnerLocation::Cell {
                    row: b, cell: bc, ..
                },
            ) => a == b && ac == bc,
            (WordOwnerLocation::Run(_, a), WordOwnerLocation::Run(_, b)) => a == b,
            (a, b) => a.same_route(b),
        };
        if !equal {
            return false;
        }
        match (record.parent, parent) {
            (Some(a), Some(b)) => {
                before = a;
                after = b;
            }
            (None, None) => return true,
            _ => return false,
        }
    }
}

// The legacy relation is pairwise, not transitive. Index both slot and
// projection routes, partitioning the old operands before selecting a branch.
#[derive(Clone, PartialEq, Eq, Hash)]
enum WordRouteKey {
    Slot(oxoffice_docx::StoryItemKind, usize, Vec<usize>),
    Projection(
        oxoffice_docx::StoryKind,
        String,
        usize,
        oxoffice_docx::StoryItemKind,
        Option<usize>,
    ),
    CellProjection(oxoffice_docx::StoryItemKind, Option<usize>),
    Cell(usize, usize),
    Run(oxoffice_docx_oxml::text::AcceptedRunPath),
    Section(usize),
    Hyperlink(
        oxoffice_docx::StoryKind,
        String,
        usize,
        oxoffice_docx::StoryItemKind,
        Vec<usize>,
        usize,
    ),
}

fn word_route_key(
    address: &WordOwnerAddress,
    class: usize,
    parent_cell: bool,
) -> Option<WordRouteKey> {
    match &address.location {
        WordOwnerLocation::Paragraph(location) | WordOwnerLocation::Table(location) => {
            let body = location.story().kind() == oxoffice_docx::StoryKind::Body;
            if body
                && class & 1 != 0
                && let Some(slot) = address.body_slot
            {
                let suffix = if matches!(address.location, WordOwnerLocation::Paragraph(_)) {
                    if location.index_path().len() > 2 {
                        return None;
                    }
                    location.index_path().get(1..).unwrap_or_default().to_vec()
                } else {
                    Vec::new()
                };
                let kind = if matches!(address.location, WordOwnerLocation::Paragraph(_)) {
                    oxoffice_docx::StoryItemKind::Paragraph
                } else {
                    oxoffice_docx::StoryItemKind::Table
                };
                Some(WordRouteKey::Slot(kind, slot, suffix))
            } else if parent_cell && class & 2 != 0 {
                Some(WordRouteKey::CellProjection(
                    location.item_kind(),
                    address.projection_ordinal,
                ))
            } else {
                let story = location.story();
                Some(WordRouteKey::Projection(
                    story.kind(),
                    story.part_name().to_owned(),
                    story.owner_index(),
                    location.item_kind(),
                    address.projection_ordinal,
                ))
            }
        }
        WordOwnerLocation::Cell { row, cell, .. } => Some(WordRouteKey::Cell(*row, *cell)),
        WordOwnerLocation::Run(_, path) => Some(WordRouteKey::Run(path.clone())),
        WordOwnerLocation::Section(index) => Some(WordRouteKey::Section(*index)),
        WordOwnerLocation::Hyperlink(location, index) => {
            let story = location.story();
            Some(WordRouteKey::Hyperlink(
                story.kind(),
                story.part_name().to_owned(),
                story.owner_index(),
                location.item_kind(),
                location.index_path().to_vec(),
                *index,
            ))
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) enum OwnerPath {
    Items(Vec<usize>),
    Depth(usize),
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) enum PropertyOwnerKey {
    Table {
        story_kind: oxoffice_docx::StoryKind,
        part_name: String,
        path: OwnerPath,
        cell: Option<(usize, usize)>,
    },
    Cell {
        story_kind: oxoffice_docx::StoryKind,
        part_name: String,
        path: OwnerPath,
        row: usize,
        column: usize,
        parent_cell: Option<(usize, usize)>,
    },
    InCell {
        story_kind: oxoffice_docx::StoryKind,
        part_name: String,
        depth: usize,
        cell: (usize, usize),
        run: Option<oxoffice_docx_oxml::text::AcceptedRunPath>,
    },
    BodyItem {
        story_kind: oxoffice_docx::StoryKind,
        part_name: String,
        kind: oxoffice_docx::StoryItemKind,
        path: Vec<usize>,
    },
    BodyRun {
        story_kind: oxoffice_docx::StoryKind,
        part_name: String,
        path: Vec<usize>,
        run: oxoffice_docx_oxml::text::AcceptedRunPath,
    },
    Section(usize),
    Hyperlink {
        story_kind: oxoffice_docx::StoryKind,
        part_name: String,
        path: OwnerPath,
        index: usize,
    },
}

fn containing_cell(
    addresses: &[WordOwnerAddress],
    mut parent: Option<usize>,
) -> Option<(usize, usize)> {
    while let Some(index) = parent {
        if let WordOwnerLocation::Cell { row, cell, .. } = &addresses[index].location {
            return Some((*row, *cell));
        }
        parent = addresses[index].parent;
    }
    None
}

/// The path of an owner in its property key. In a cell story, its depth
/// only, as for the cell's paragraphs and runs: a first `w:tcPr` written in
/// the cell shifts the raw item indices of what the cell holds (a nested
/// table, the paragraph of a hyperlink), and the owners keep their order.
fn owner_path(location: &oxoffice_docx::ContentLocation) -> OwnerPath {
    if location.story().kind() == oxoffice_docx::StoryKind::TableCell {
        OwnerPath::Depth(location.index_path().len())
    } else {
        OwnerPath::Items(location.index_path().to_vec())
    }
}

fn story_part(location: &oxoffice_docx::ContentLocation) -> (oxoffice_docx::StoryKind, String) {
    let story = location.story();
    (story.kind(), story.part_name().to_owned())
}

pub(super) fn property_owner_key(addresses: &[WordOwnerAddress], index: usize) -> PropertyOwnerKey {
    let address = &addresses[index];
    let cell = containing_cell(addresses, address.parent);
    match &address.location {
        WordOwnerLocation::Table(location) => {
            let (story_kind, part_name) = story_part(location);
            PropertyOwnerKey::Table {
                story_kind,
                part_name,
                path: owner_path(location),
                cell,
            }
        }
        WordOwnerLocation::Cell {
            table,
            row,
            cell: column,
            ..
        } => {
            let (story_kind, part_name) = story_part(table);
            PropertyOwnerKey::Cell {
                story_kind,
                part_name,
                path: owner_path(table),
                row: *row,
                column: *column,
                parent_cell: cell,
            }
        }
        WordOwnerLocation::Paragraph(location) => {
            let (story_kind, part_name) = story_part(location);
            if let Some(cell) = cell {
                PropertyOwnerKey::InCell {
                    story_kind,
                    part_name,
                    depth: location.index_path().len(),
                    cell,
                    run: None,
                }
            } else {
                PropertyOwnerKey::BodyItem {
                    story_kind,
                    part_name,
                    kind: location.item_kind(),
                    path: location.index_path().to_vec(),
                }
            }
        }
        WordOwnerLocation::Run(location, path) => {
            let (story_kind, part_name) = story_part(location);
            if let Some(cell) = cell {
                PropertyOwnerKey::InCell {
                    story_kind,
                    part_name,
                    depth: location.index_path().len(),
                    cell,
                    run: Some(path.clone()),
                }
            } else {
                PropertyOwnerKey::BodyRun {
                    story_kind,
                    part_name,
                    path: location.index_path().to_vec(),
                    run: path.clone(),
                }
            }
        }
        WordOwnerLocation::Section(index) => PropertyOwnerKey::Section(*index),
        WordOwnerLocation::Hyperlink(location, index) => {
            let (story_kind, part_name) = story_part(location);
            PropertyOwnerKey::Hyperlink {
                story_kind,
                part_name,
                path: owner_path(location),
                index: *index,
            }
        }
    }
}

pub(super) fn relocated_property_owners(
    old: &[WordOwner],
    new_addresses: &[WordOwnerAddress],
    items: &WordBodyItems,
) -> Option<Vec<WordOwner>> {
    if old.len() != new_addresses.len() {
        return None;
    }
    let old_addresses = old.iter().map(WordOwner::address).collect::<Vec<_>>();
    let mut buckets: HashMap<PropertyOwnerKey, Vec<usize>> = HashMap::new();
    for index in 0..old_addresses.len() {
        buckets
            .entry(property_owner_key(&old_addresses, index))
            .or_default()
            .push(index);
    }
    let mut cursors: HashMap<PropertyOwnerKey, usize> = HashMap::new();
    let mut owners = Vec::with_capacity(new_addresses.len());
    for index in 0..new_addresses.len() {
        let key = property_owner_key(new_addresses, index);
        let bucket = buckets.get(&key)?;
        let cursor = cursors.entry(key).or_insert(0);
        if *cursor >= bucket.len() {
            return None;
        }
        let previous = &old[bucket[*cursor]];
        *cursor += 1;
        owners.push(WordOwner::with_states_of(
            new_addresses[index].clone(),
            previous,
            items,
        ));
    }
    if cursors.len() != buckets.len()
        || cursors
            .iter()
            .any(|(key, cursor)| buckets.get(key).map(Vec::len) != Some(*cursor))
    {
        return None;
    }
    Some(owners)
}

pub(super) fn indexed_inventory_matches(
    old: &[WordOwner],
    new: &[WordOwnerAddress],
    publication: &WordPublication,
) -> Vec<Option<usize>> {
    let mut index = HashMap::<(Option<usize>, usize, WordRouteKey), Vec<usize>>::new();
    let mut links = HashMap::<WordRouteKey, Vec<usize>>::new();
    for (before, owner) in old.iter().enumerate() {
        let address = owner.address();
        let parent_cell = owner.parent.is_some_and(|parent| {
            matches!(old[parent].route_shape(), WordOwnerLocation::Cell { .. })
        });
        let body_slot = match &address.location {
            WordOwnerLocation::Paragraph(location) | WordOwnerLocation::Table(location) => {
                location.story().kind() == oxoffice_docx::StoryKind::Body
                    && address.body_slot.is_some()
            }
            _ => false,
        };
        let class = usize::from(body_slot) | (usize::from(parent_cell) << 1);
        // The candidate determines which pairwise branch applies. Supply each
        // branch's old key in its own namespace, without merging parent routes.
        let mut keys = Vec::new();
        for candidate_class in 0..4 {
            if let Some(key) = word_route_key(&address, candidate_class, candidate_class & 2 != 0)
                && !keys.contains(&key)
            {
                keys.push(key);
            }
        }
        for key in keys {
            index
                .entry((owner.parent, class, key.clone()))
                .or_default()
                .push(before);
            if matches!(owner.route_shape(), WordOwnerLocation::Hyperlink(..)) {
                links.entry(key).or_default().push(before);
            }
        }
    }
    let mut matches = vec![None::<Vec<usize>>; new.len()];
    for target in 0..new.len() {
        let mut stack = vec![(target, false)];
        while let Some((after, ready)) = stack.pop() {
            if matches[after].is_some() {
                continue;
            }
            if !ready && let Some(parent) = new[after].parent {
                stack.push((after, true));
                if matches[parent].is_none() {
                    stack.push((parent, false));
                }
                continue;
            }
            let address = &new[after];
            let mut found = Vec::new();
            // Removal's original override uses the source content route alone,
            // independently of the normal parent-chain matcher.
            let removed_link = if let (
                WordPublication::HyperlinkRemove(WordOwnerLocation::Hyperlink(
                    removed,
                    removed_index,
                )),
                WordOwnerLocation::Hyperlink(location, position),
            ) = (publication, &address.location)
            {
                if same_content_owner(removed, location) {
                    let previous = if position >= removed_index {
                        position.checked_add(1)
                    } else {
                        Some(*position)
                    };
                    if let Some(previous) = previous {
                        let shifted = WordOwnerAddress {
                            location: WordOwnerLocation::Hyperlink(location.clone(), previous),
                            parent: None,
                            body_slot: None,
                            projection_ordinal: None,
                            read_facts: WordReadFacts::None,
                        };
                        if let Some(key) = word_route_key(&shifted, 0, false) {
                            found.extend(links.get(&key).into_iter().flatten().copied());
                        }
                    }
                    true
                } else {
                    false
                }
            } else {
                false
            };
            if !removed_link {
                let parents = address.parent.map_or_else(
                    || vec![None],
                    |parent| {
                        matches[parent]
                            .as_ref()
                            .unwrap()
                            .iter()
                            .copied()
                            .map(Some)
                            .collect()
                    },
                );
                let parent_cell = address.parent.is_some_and(|parent| {
                    matches!(new[parent].location, WordOwnerLocation::Cell { .. })
                });
                for parent in parents {
                    for class in 0..4 {
                        if let Some(key) = word_route_key(address, class, parent_cell)
                            && let Some(candidates) = index.get(&(parent, class, key))
                        {
                            for &before in candidates {
                                if same_inventory_route(old, before, new, after) {
                                    found.push(before);
                                }
                            }
                        }
                    }
                }
            }
            found.sort_unstable();
            found.dedup();
            matches[after] = Some(found);
        }
    }
    matches
        .into_iter()
        .map(|matched| matched.unwrap().into_iter().next())
        .collect()
}

impl WordOwner {
    pub(super) fn states(&self) -> Vec<RevisionState> {
        self.scope_states().cloned().collect()
    }

    /// The owner's scope states, in [`WordOwnerLocation::scopes`] order.
    pub(super) fn scope_states(&self) -> impl Iterator<Item = &RevisionState> {
        std::iter::once(&self.owner)
            .chain(&self.content)
            .chain(&self.properties)
            .chain(&self.grid)
    }
}

/// The owners of `locations` with their scope states, in order, stored
/// relative to the body items of `items`.
pub(super) fn word_owners_with_states(
    locations: Vec<WordOwnerAddress>,
    states: &[RevisionState],
    items: &WordBodyItems,
) -> Vec<WordOwner> {
    let mut result = Vec::with_capacity(locations.len());
    append_word_owners_with_states(&mut result, locations, states, items);
    result
}

pub(super) fn append_word_owners_with_states(
    result: &mut Vec<WordOwner>,
    locations: Vec<WordOwnerAddress>,
    states: &[RevisionState],
    items: &WordBodyItems,
) {
    let mut offset = 0;
    result.extend(locations.into_iter().map(|address| {
        let scopes = &states[offset..offset + address.location.scopes().len()];
        offset += scopes.len();
        WordOwner::new(address, scopes, items)
    }));
}
