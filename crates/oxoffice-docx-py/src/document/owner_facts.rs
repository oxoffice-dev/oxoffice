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

/// The facade paragraph count of each modeled cell of a standalone table.
pub(super) fn word_table_facts(xml: &[u8]) -> Option<WordReadFacts> {
    let table = oxoffice_docx_oxml::table::CT_Tbl::from_xml_fragment_checked(xml).ok()?;
    let rows = table
        .rows
        .iter()
        .map(|row| {
            row.cells
                .iter()
                .map(|cell| cell.paragraphs().len())
                .collect()
        })
        .collect();
    Some(WordReadFacts::Table(rows))
}

/// The four owner snapshot families of [`oxoffice_docx::Document::story_owner_snapshots`].
pub(super) type WordOwnerSnapshots = (
    Vec<oxoffice_docx::StoryItemSnapshot>,
    Vec<(
        oxoffice_docx::ContentLocation,
        usize,
        usize,
        oxoffice_docx::StoryId,
    )>,
    Vec<(oxoffice_docx::ContentLocation, Vec<u8>)>,
    Vec<(oxoffice_docx::ContentLocation, oxoffice_docx::LinkInfo)>,
);

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
    (snapshots, cells, paragraphs, links): WordOwnerSnapshots,
    table_facts: impl Fn(&oxoffice_docx::StoryItemSnapshot) -> Option<WordReadFacts>,
) -> oxoffice_docx::Result<Option<WordOwnerFacts>> {
    let mut items = Vec::new();
    for snapshot in snapshots.iter().filter(|snapshot| {
        matches!(
            snapshot.location().item_kind(),
            oxoffice_docx::StoryItemKind::Table
                | oxoffice_docx::StoryItemKind::Paragraph
                | oxoffice_docx::StoryItemKind::ContentControl
        )
    }) {
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
        result.push(WordOwnerAddress {
            location: WordOwnerLocation::Table(location.clone()),
            parent: None,
            projection_ordinal,
            read_facts,
            body_slot: slot,
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
        let body_slot = if location.story().kind() == oxoffice_docx::StoryKind::Body {
            match location.index_path() {
                [_] => body_slots.get(&location).copied(),
                [control, _] => body_slots
                    .get(&oxoffice_docx::ContentLocation::new(
                        location.story().clone(),
                        oxoffice_docx::StoryItemKind::ContentControl,
                        vec![*control],
                    ))
                    .copied(),
                _ => None,
            }
        } else {
            None
        };
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
