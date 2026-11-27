//! Owner facts read from the document: form sources, table read facts and owner addresses.

use super::*;

// Source keys include the complete physical route, not projected text ordinals.
pub(super) type FormSourceKey = (
    oxoffice_docx::StoryKind,
    String,
    usize,
    oxoffice_docx::StoryItemKind,
    Vec<usize>,
);
pub(super) type FormStoryKey = (oxoffice_docx::StoryKind, String, usize);

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) enum FormOwnerRoute {
    Paragraph(FormSourceKey),
    Table(FormSourceKey),
    Cell(FormSourceKey, usize, usize, FormStoryKey),
    Section(usize),
}

pub(super) fn form_source_key(location: &oxoffice_docx::ContentLocation) -> FormSourceKey {
    let story = location.story();
    (
        story.kind(),
        story.part_name().to_owned(),
        story.owner_index(),
        location.item_kind(),
        location.index_path().to_vec(),
    )
}

pub(super) fn form_owner_route(location: &WordOwnerLocation) -> Option<FormOwnerRoute> {
    match location {
        WordOwnerLocation::Paragraph(location) => {
            Some(FormOwnerRoute::Paragraph(form_source_key(location)))
        }
        WordOwnerLocation::Table(location) => {
            Some(FormOwnerRoute::Table(form_source_key(location)))
        }
        WordOwnerLocation::Cell {
            table,
            row,
            cell,
            story,
        } => Some(FormOwnerRoute::Cell(
            form_source_key(table),
            *row,
            *cell,
            (
                story.kind(),
                story.part_name().to_owned(),
                story.owner_index(),
            ),
        )),
        WordOwnerLocation::Section(index) => Some(FormOwnerRoute::Section(*index)),
        WordOwnerLocation::Run(..) | WordOwnerLocation::Hyperlink(..) => None,
    }
}

pub(super) fn word_table_read_facts(
    document: &oxoffice_docx::Document,
) -> HashMap<oxoffice_docx::ContentLocation, WordReadFacts> {
    // Optional acceleration only. The checked native reader remains the fallback
    // for source that the standalone checked table parser cannot project.
    document
        .standalone_story_item_snapshots()
        .unwrap_or_default()
        .into_iter()
        .filter(|item| item.location().item_kind() == oxoffice_docx::StoryItemKind::Table)
        .filter_map(|item| Some((item.location().clone(), word_table_facts(item.xml())?)))
        .collect()
}

/// The facade paragraph count of each modeled cell of a standalone table,
/// rows and cells counted as the facade counts them: those of row and cell
/// content controls included.
pub(super) fn word_table_facts(xml: &[u8]) -> Option<WordReadFacts> {
    let table = oxoffice_docx_oxml::table::CT_Tbl::from_xml_fragment_checked(xml).ok()?;
    let table = oxoffice_docx::TableRef::from_model(&table);
    let rows = (0..table.row_count())
        .map(|row| {
            let row = table.row(row)?;
            (0..row.cell_count())
                .map(|cell| Some(row.cell(cell)?.paragraph_count()))
                .collect::<Option<Vec<_>>>()
        })
        .collect::<Option<Vec<_>>>()?;
    Some(WordReadFacts::Table(rows))
}

/// The owner snapshot families of [`oxoffice_docx::Document::story_owner_snapshots`].
pub(super) type WordOwnerSnapshots = oxoffice_docx::StoryOwnerSnapshots;

/// One entry of [`with_control_tables`]: a story item, or a table of a block
/// control, by its index in its family.
#[derive(Clone, Copy)]
pub(super) enum StoryEntry {
    Item(usize),
    ControlTable(usize),
}

/// The story items of `snapshots` with the tables of its block controls
/// placed among them, in story order: a control table right before the first
/// story-item table of its story that it comes before, else after the last
/// item of its story. The tables of each story are then those of
/// [`oxoffice_docx::Document::story_tables`], in order. `None` when a control
/// table is not placed: the producer lists both families in one story order.
pub(super) fn with_control_tables(snapshots: &WordOwnerSnapshots) -> Option<Vec<StoryEntry>> {
    let items = &snapshots.items;
    let control = &snapshots.control_tables;
    let mut entries = Vec::with_capacity(items.len() + control.len());
    let mut next = 0;
    // Place the control tables of `story` that come before its `tables`-th
    // story-item table (all of them with `None`).
    let mut place =
        |entries: &mut Vec<StoryEntry>, story: &oxoffice_docx::StoryId, tables: Option<usize>| {
            while control.get(next).is_some_and(|(location, before, _)| {
                same_story_owner(location.story(), story)
                    && tables.is_none_or(|tables| *before <= tables)
            }) {
                entries.push(StoryEntry::ControlTable(next));
                next += 1;
            }
        };
    let mut tables = 0;
    for (index, item) in items.iter().enumerate() {
        let story = item.location().story();
        if let Some(previous) = index
            .checked_sub(1)
            .map(|previous| items[previous].location().story())
            && !same_story_owner(previous, story)
        {
            place(&mut entries, previous, None);
            tables = 0;
        }
        if item.location().item_kind() == oxoffice_docx::StoryItemKind::Table {
            place(&mut entries, story, Some(tables));
            tables += 1;
        }
        entries.push(StoryEntry::Item(index));
    }
    if let Some(last) = items.last() {
        place(&mut entries, last.location().story(), None);
    }
    (next == control.len()).then_some(entries)
}

/// The direct body slot of a body paragraph or table at `location`: the
/// slot of the item for one at `[i]` (`direct`), the slot of its control for
/// one at `[c, k]` when that control is a direct body child.
pub(super) fn content_body_slot(
    location: &oxoffice_docx::ContentLocation,
    direct: Option<usize>,
    body_slots: &HashMap<oxoffice_docx::ContentLocation, usize>,
) -> Option<usize> {
    if location.story().kind() != oxoffice_docx::StoryKind::Body {
        return None;
    }
    match location.index_path() {
        [_] => direct,
        [control, _] => body_slots
            .get(&oxoffice_docx::ContentLocation::new(
                location.story().clone(),
                oxoffice_docx::StoryItemKind::ContentControl,
                vec![*control],
            ))
            .copied(),
        _ => None,
    }
}

pub(super) type WordParagraphFacts = (
    oxoffice_docx::ContentLocation,
    String,
    Vec<oxoffice_docx_oxml::text::AcceptedRunPath>,
);

impl WordOwnerFacts {
    /// Prove every native physical cell has one indexed direct coordinate.
    /// Compute once for immutable standalone-piece facts, never per setter.
    pub(super) fn has_complete_table_membership(&self, physical_cells: usize) -> bool {
        let Some((root, Some(0), WordReadFacts::Table(rows))) = self.items.first() else {
            return false;
        };
        if root.story().kind() != oxoffice_docx::StoryKind::Body
            || root.index_path() != [0]
            || self
                .items
                .iter()
                .filter(|(location, _, _)| {
                    location.item_kind() == oxoffice_docx::StoryItemKind::Table
                })
                .count()
                != 1
            || self.cells.len() != physical_cells
        {
            return false;
        }
        let mut coordinates = std::collections::HashSet::new();
        for (ordinal, (table, row, cell, story)) in self.cells.iter().enumerate() {
            if table != root
                || story.kind() != oxoffice_docx::StoryKind::TableCell
                || story.part_name() != root.story().part_name()
                || story.owner_index() != ordinal
                || rows.get(*row).and_then(|cells| cells.get(*cell)).is_none()
                || !coordinates.insert((*row, *cell))
            {
                return false;
            }
        }
        // Native cell_stories includes controlled and empty physical cells.
        // A projected-cell count alone cannot certify their membership/base.
        self.paragraphs.iter().all(|(location, _, _)| {
            let story = location.story();
            story.kind() == oxoffice_docx::StoryKind::TableCell
                && story.part_name() == root.story().part_name()
                && story.owner_index() < physical_cells
        })
    }
}

pub(super) fn word_owner_facts(
    snapshots: WordOwnerSnapshots,
    table_facts: impl Fn(&oxoffice_docx::StoryItemSnapshot) -> Option<WordReadFacts>,
) -> oxoffice_docx::Result<Option<WordOwnerFacts>> {
    let Some(entries) = with_control_tables(&snapshots) else {
        return Ok(None);
    };
    let mut items = Vec::new();
    for entry in entries {
        let snapshot = match entry {
            StoryEntry::Item(index) => &snapshots.items[index],
            StoryEntry::ControlTable(index) => {
                let (location, _, xml) = &snapshots.control_tables[index];
                let Some(facts) = word_table_facts(xml) else {
                    return Ok(None);
                };
                items.push((location.clone(), None, facts));
                continue;
            }
        };
        if !matches!(
            snapshot.location().item_kind(),
            oxoffice_docx::StoryItemKind::Table
                | oxoffice_docx::StoryItemKind::Paragraph
                | oxoffice_docx::StoryItemKind::ContentControl
        ) {
            continue;
        }
        let read_facts = if snapshot.location().item_kind() == oxoffice_docx::StoryItemKind::Table {
            match table_facts(snapshot) {
                Some(facts) => facts,
                None => return Ok(None),
            }
        } else {
            WordReadFacts::None
        };
        let slot = snapshot
            .direct_body_index()
            .filter(|_| snapshot.is_direct_child());
        items.push((snapshot.location().clone(), slot, read_facts));
    }
    let oxoffice_docx::StoryOwnerSnapshots {
        cells,
        paragraphs,
        links,
        ..
    } = snapshots;
    let mut parsed = Vec::new();
    for (location, xml) in paragraphs {
        let paragraph = oxoffice_docx_oxml::CT_P::from_xml_fragment(&xml)?;
        parsed.push((
            location,
            paragraph.accepted_text(),
            paragraph.accepted_run_paths(),
        ));
    }
    Ok(Some(WordOwnerFacts {
        items,
        cells,
        paragraphs: parsed,
        links: links.into_iter().map(|(location, _)| location).collect(),
    }))
}

/// Addresses from piece facts. The full path stays [`word_owner_addresses`].
pub(super) fn word_piece_owner_addresses(
    facts: WordOwnerFacts,
    sections: usize,
) -> Vec<WordOwnerAddress> {
    let WordOwnerFacts {
        items,
        cells,
        paragraphs,
        links,
    } = facts;
    let mut result = Vec::new();
    let mut table_ordinals = HashMap::new();
    let mut body_slots = HashMap::new();
    let mut cell_parents = HashMap::new();
    let mut table_cells = HashMap::<oxoffice_docx::ContentLocation, Vec<_>>::new();
    for (table, row, cell, story) in cells {
        table_cells
            .entry(table)
            .or_default()
            .push((row, cell, story));
    }
    for (location, slot, _) in &items {
        if location.story().kind() == oxoffice_docx::StoryKind::Body
            && matches!(
                location.item_kind(),
                oxoffice_docx::StoryItemKind::Paragraph
                    | oxoffice_docx::StoryItemKind::ContentControl
            )
            && let Some(slot) = slot
        {
            body_slots.insert(location.clone(), *slot);
        }
    }
    for (location, slot, read_facts) in items {
        if location.item_kind() != oxoffice_docx::StoryItemKind::Table {
            continue;
        }
        let ordinal = table_ordinals
            .entry(location.story().clone())
            .or_insert(0usize);
        let projection_ordinal = Some(*ordinal);
        *ordinal += 1;
        let table_index = result.len();
        let cells = table_cells.remove(&location).unwrap_or_default();
        let body_slot = content_body_slot(&location, slot, &body_slots);
        result.push(WordOwnerAddress {
            location: WordOwnerLocation::Table(location.clone()),
            parent: None,
            projection_ordinal,
            read_facts,
            body_slot,
        });
        for (row, cell, story) in cells {
            result.push(WordOwnerAddress {
                location: WordOwnerLocation::Cell {
                    table: location.clone(),
                    row,
                    cell,
                    story,
                },
                parent: Some(table_index),
                body_slot: None,
                projection_ordinal: None,
                read_facts: WordReadFacts::None,
            });
        }
    }
    for (index, address) in result.iter().enumerate() {
        if let WordOwnerLocation::Cell { story, .. } = &address.location {
            cell_parents
                .entry((
                    story.kind(),
                    story.part_name().to_owned(),
                    story.owner_index(),
                ))
                .or_insert(index);
        }
    }
    for address in &mut result {
        if let WordOwnerLocation::Table(location) = &address.location {
            let story = location.story();
            address.parent = cell_parents
                .get(&(
                    story.kind(),
                    story.part_name().to_owned(),
                    story.owner_index(),
                ))
                .copied();
        }
    }
    let mut paragraph_ordinals = HashMap::new();
    for (location, text, runs) in paragraphs {
        let story = location.story();
        let parent = cell_parents
            .get(&(
                story.kind(),
                story.part_name().to_owned(),
                story.owner_index(),
            ))
            .copied();
        let ordinal = paragraph_ordinals
            .entry(location.story().clone())
            .or_insert(0usize);
        let projection_ordinal = Some(*ordinal);
        *ordinal += 1;
        let index = result.len();
        let body_slot =
            content_body_slot(&location, body_slots.get(&location).copied(), &body_slots);
        result.push(WordOwnerAddress {
            location: WordOwnerLocation::Paragraph(location.clone()),
            parent,
            body_slot,
            projection_ordinal,
            read_facts: WordReadFacts::Paragraph(text),
        });
        for path in runs {
            result.push(WordOwnerAddress {
                location: WordOwnerLocation::Run(location.clone(), path),
                parent: Some(index),
                body_slot: None,
                projection_ordinal: None,
                read_facts: WordReadFacts::None,
            });
        }
    }
    for index in 0..sections {
        result.push(WordOwnerAddress {
            location: WordOwnerLocation::Section(index),
            parent: None,
            body_slot: None,
            projection_ordinal: None,
            read_facts: WordReadFacts::None,
        });
    }
    let mut local_indexes = HashMap::<oxoffice_docx::ContentLocation, usize>::new();
    let mut story_indexes = HashMap::<oxoffice_docx::StoryId, usize>::new();
    for location in links {
        let local = local_indexes.entry(location.clone()).or_default();
        let position = story_indexes.entry(location.story().clone()).or_default();
        result.push(WordOwnerAddress {
            location: WordOwnerLocation::Hyperlink(location.clone(), *local),
            parent: None,
            body_slot: None,
            projection_ordinal: Some(*position),
            read_facts: WordReadFacts::None,
        });
        *local += 1;
        *position += 1;
    }
    result
}
