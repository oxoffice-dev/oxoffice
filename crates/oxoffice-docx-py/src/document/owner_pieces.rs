//! Owner pieces: block and part pieces of the inventory, and their rebuild from the document.

use super::*;

pub(super) type CellParagraphRoutes =
    HashMap<(usize, usize, usize), Vec<oxoffice_docx::ContentLocation>>;

impl WordIndexProjections {
    pub(super) fn of(document: &oxoffice_docx::Document) -> Self {
        Self {
            body_paragraphs: document.paragraph_story_locations().ok(),
            cell_paragraphs: document
                .cell_paragraph_story_locations()
                .unwrap_or_default(),
            table_slots: (0..document.table_count())
                .map(|index| document.content_index_of_table(index))
                .collect(),
        }
    }
}

pub(super) fn word_block_owners(
    document: &oxoffice_docx::Document,
    block: &oxoffice_docx_oxml::BodyContent,
) -> oxoffice_docx::Result<Option<WordBlockOwners>> {
    if let oxoffice_docx_oxml::BodyContent::Paragraph(paragraph) = block
        && let Ok(Some((text, runs))) = document.direct_body_paragraph_facts(paragraph)
    {
        let context = document.body_item_context();
        let story =
            oxoffice_docx::StoryId::new(oxoffice_docx::StoryKind::Body, context.part_name(), 0);
        return Ok(Some(plain_paragraph_owners(&story, &text, &runs)));
    }
    let oxoffice_docx::BodyItemOwners {
        owners,
        cell_stories,
        text_boxes,
        paragraph_routes,
        tables,
        cell_paragraphs,
        holds_field,
    } = document.body_item_owners(block)?;
    let body = |snapshot: &&oxoffice_docx::StoryItemSnapshot| {
        snapshot.location().story().kind() == oxoffice_docx::StoryKind::Body
    };
    let direct = owners
        .items
        .iter()
        .filter(body)
        .filter(|snapshot| snapshot.is_direct_child())
        .map(oxoffice_docx::StoryItemSnapshot::direct_body_index)
        .collect::<Vec<_>>();
    if direct.first() != Some(&Some(0)) || direct.len() != 1 {
        return Ok(None);
    }
    let body_items = owners.items.iter().filter(body).count();
    let relationships = link_relationships(&owners.links);
    let Some(facts) = word_owner_facts(owners, |snapshot| word_table_facts(snapshot.xml()))? else {
        return Ok(None);
    };
    Ok(Some(WordBlockOwners {
        local_table_membership: facts.has_complete_table_membership(cell_stories),
        facts,
        relationships,
        body_items,
        cell_stories,
        text_boxes,
        paragraphs: paragraph_routes,
        tables,
        cell_paragraphs,
        holds_field,
    }))
}

fn link_relationships(
    links: &[(oxoffice_docx::ContentLocation, oxoffice_docx::LinkInfo)],
) -> Vec<String> {
    let mut relationships = links
        .iter()
        .filter_map(|(_, link)| link.rel_id.clone())
        .collect::<Vec<_>>();
    relationships.sort_unstable();
    relationships.dedup();
    relationships
}

pub(super) fn plain_paragraph_owners(
    body: &oxoffice_docx::StoryId,
    text: &str,
    runs: &[oxoffice_docx_oxml::text::AcceptedRunPath],
) -> WordBlockOwners {
    let location = oxoffice_docx::ContentLocation::new(
        body.clone(),
        oxoffice_docx::StoryItemKind::Paragraph,
        vec![0],
    );
    WordBlockOwners {
        facts: WordOwnerFacts {
            items: vec![(location.clone(), Some(0), WordReadFacts::None)],
            cells: Vec::new(),
            paragraphs: vec![(location, text.to_owned(), runs.to_vec())],
            links: Vec::new(),
        },
        relationships: Vec::new(),
        body_items: 1,
        cell_stories: 0,
        text_boxes: 0,
        paragraphs: vec![Some(vec![0])],
        tables: 0,
        local_table_membership: false,
        cell_paragraphs: Some((HashMap::new(), true)),
        holds_field: false,
    }
}

pub(super) fn table_owners(facts: WordOwnerFacts, cells: usize) -> Option<WordBlockOwners> {
    let Some((_, _, WordReadFacts::Table(rows))) = facts.items.first() else {
        return None;
    };
    let mut stories = HashMap::new();
    let mut routes = CellParagraphRoutes::new();
    for (_, row, cell, story) in &facts.cells {
        stories.insert(story.owner_index(), (*row, *cell));
        routes.insert((0, *row, *cell), Vec::new());
    }
    for (location, _, _) in &facts.paragraphs {
        if location.story().kind() == oxoffice_docx::StoryKind::TableCell
            && let Some((row, cell)) = stories.get(&location.story().owner_index())
        {
            routes
                .entry((0, *row, *cell))
                .or_default()
                .push(location.clone());
        }
    }
    routes.retain(|(_, row, cell), locations| {
        rows.get(*row).and_then(|row| row.get(*cell)) == Some(&locations.len())
    });
    Some(WordBlockOwners {
        local_table_membership: facts.has_complete_table_membership(cells),
        relationships: Vec::new(),
        body_items: 1,
        cell_stories: cells,
        text_boxes: 0,
        paragraphs: Vec::new(),
        tables: 1,
        cell_paragraphs: Some((routes, true)),
        facts,
        holds_field: false,
    })
}

pub(super) fn placed_route(
    location: &oxoffice_docx::ContentLocation,
    body: &oxoffice_docx::StoryId,
    (items, stories): (usize, (usize, usize)),
) -> oxoffice_docx::ContentLocation {
    let story = location.story();
    if story.kind() == oxoffice_docx::StoryKind::Body {
        let mut placed = oxoffice_docx::ContentLocation::new(
            body.clone(),
            location.item_kind(),
            location.index_path().to_vec(),
        );
        if let Some(head) = location.index_path().first() {
            placed.set_index_path_head(head + items);
        }
        placed
    } else {
        oxoffice_docx::ContentLocation::new(
            placed_story(story, stories),
            location.item_kind(),
            location.index_path().to_vec(),
        )
    }
}

pub(super) fn placed_story(
    story: &oxoffice_docx::StoryId,
    (cells, boxes): (usize, usize),
) -> oxoffice_docx::StoryId {
    let mut placed = story.clone();
    let before = if story.kind() == oxoffice_docx::StoryKind::TextBox {
        boxes
    } else {
        cells
    };
    placed.set_owner_index(story.owner_index() + before);
    placed
}

pub(super) struct WordPartPiece {
    kind: oxoffice_docx::StoryKind,
    part_name: String,
    xml: Vec<u8>,
    owners: Option<(WordOwnerFacts, Vec<String>)>,
}

pub(super) fn aligned_block(
    piece: &WordBlockPiece,
    block: &oxoffice_docx_oxml::BodyContent,
) -> bool {
    piece.block.as_ref() == block
}

fn qualified_block(piece: &WordBlockPiece, block: &oxoffice_docx_oxml::BodyContent) -> bool {
    aligned_block(piece, block)
        && piece
            .owners
            .as_ref()
            .is_some_and(|owners| !owners.holds_field)
        && piece.block.owner_facts_reusable_by_value()
        && block.owner_facts_reusable_by_value()
}

impl WordPieces {
    pub(super) fn assemble(
        &self,
        document: &oxoffice_docx::Document,
    ) -> (Self, Option<(Vec<WordOwnerAddress>, WordIndexProjections)>) {
        let context = document.body_item_context();
        let blocks = document.body_children();
        let Ok(sources) = document.story_part_sources() else {
            return (self.clone(), None);
        };
        // Sources have already resolved dot segments. OPC physical identity is
        // case insensitive even when relationship targets retain their spelling.
        let mut names = std::collections::HashSet::from([context.part_name().to_ascii_lowercase()]);
        if !sources
            .iter()
            .all(|(_, name, _)| names.insert(name.to_ascii_lowercase()))
        {
            return (self.clone(), None);
        }
        let opened_paragraphs = if self.blocks.is_empty() {
            document
                .opened_plain_body_paragraph_facts()
                .ok()
                .flatten()
                .filter(|facts| facts.len() == blocks.len())
        } else {
            None
        };
        let same_context = self
            .context
            .as_ref()
            .is_some_and(|old| old.read_facts_compatible(&context));
        let changed_existing_context = self.context.is_some() && !same_context;
        let old = if same_context {
            self.blocks.as_slice()
        } else {
            &[]
        };
        let prefix = old
            .iter()
            .zip(blocks)
            .take_while(|(piece, block)| aligned_block(piece, block))
            .count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(blocks[prefix..].iter().rev())
            .take_while(|(piece, block)| aligned_block(piece, block))
            .count();
        let mut pieces = Self {
            context: Some(context),
            blocks: Vec::with_capacity(blocks.len()),
            parts: Vec::new(),
        };
        for (piece, block) in old[..prefix].iter().zip(&blocks[..prefix]) {
            if qualified_block(piece, block) {
                pieces.blocks.push(std::sync::Arc::clone(piece));
            } else {
                let owners = word_block_owners(document, block).ok().flatten();
                pieces.blocks.push(std::sync::Arc::new(WordBlockPiece {
                    block: std::sync::Arc::new(block.clone()),
                    owners,
                }));
            }
        }
        for (offset, block) in blocks[prefix..blocks.len() - suffix].iter().enumerate() {
            let owners = opened_paragraphs
                .as_ref()
                .and_then(|facts| facts.get(prefix + offset))
                .map(|(_, text, runs)| {
                    let story = oxoffice_docx::StoryId::new(
                        oxoffice_docx::StoryKind::Body,
                        pieces
                            .context
                            .as_ref()
                            .expect("current body context")
                            .part_name(),
                        0,
                    );
                    plain_paragraph_owners(&story, text, runs)
                })
                .or_else(|| word_block_owners(document, block).ok().flatten());
            pieces.blocks.push(std::sync::Arc::new(WordBlockPiece {
                block: opened_paragraphs
                    .as_ref()
                    .and_then(|facts| facts.get(prefix + offset))
                    .and_then(|(model, _, _)| model.clone())
                    .unwrap_or_else(|| std::sync::Arc::new(block.clone())),
                owners,
            }));
        }
        for (piece, block) in old[old.len() - suffix..]
            .iter()
            .zip(&blocks[blocks.len() - suffix..])
        {
            if qualified_block(piece, block) {
                pieces.blocks.push(std::sync::Arc::clone(piece));
            } else {
                let owners = word_block_owners(document, block).ok().flatten();
                pieces.blocks.push(std::sync::Arc::new(WordBlockPiece {
                    block: std::sync::Arc::new(block.clone()),
                    owners,
                }));
            }
        }
        let old_parts = if same_context {
            self.parts.as_slice()
        } else {
            &[]
        };
        for (kind, part_name, xml) in sources {
            if let Some(piece) = old_parts.iter().find(|piece| {
                piece.owners.is_some()
                    && piece.kind == kind
                    && piece.part_name == part_name
                    && piece.xml == *xml
            }) {
                pieces.parts.push(piece.clone());
                continue;
            }
            let owners = document
                .story_part_owner_snapshots(kind, &part_name, &xml)
                .ok()
                .and_then(|snapshots| {
                    let relationships = link_relationships(&snapshots.links);
                    word_owner_facts(snapshots, |snapshot| word_table_facts(snapshot.xml()))
                        .ok()
                        .flatten()
                        .map(|facts| (facts, relationships))
                });
            pieces.parts.push(std::sync::Arc::new(WordPartPiece {
                kind,
                part_name,
                xml: xml.into_owned(),
                owners,
            }));
        }
        let inventory = if changed_existing_context {
            None
        } else {
            pieces.inventory(document)
        };
        (pieces, inventory)
    }

    /// The position of the related part `part_name` among the pieces, and
    /// the owner addresses its piece holds, as the inventory reads them for
    /// that part alone (parents relative to the result).
    pub(super) fn part_owner_addresses(
        &self,
        part_name: &str,
    ) -> Option<(usize, Vec<WordOwnerAddress>)> {
        let index = self
            .parts
            .iter()
            .position(|piece| piece.part_name == part_name)?;
        let (facts, _) = self.parts[index].owners.as_ref()?;
        Some((index, word_piece_owner_addresses(facts.clone(), 0)))
    }

    /// Read the related part `part_name` of `document` as [`Self::assemble`]
    /// does, for that part alone: its piece and owner addresses, or `None`
    /// when its owners cannot be read or a link names no valid relationship.
    pub(super) fn read_part(
        document: &oxoffice_docx::Document,
        kind: oxoffice_docx::StoryKind,
        part_name: &str,
        xml: &[u8],
    ) -> Option<(std::sync::Arc<WordPartPiece>, Vec<WordOwnerAddress>)> {
        let snapshots = document
            .story_part_owner_snapshots(kind, part_name, xml)
            .ok()?;
        let relationships = link_relationships(&snapshots.links);
        let facts = word_owner_facts(snapshots, |snapshot| word_table_facts(snapshot.xml()))
            .ok()
            .flatten()?;
        if !relationships.is_empty() {
            let valid = document.story_hyperlink_relationships(part_name);
            if !relationships
                .iter()
                .all(|relationship| valid.contains(relationship.as_str()))
            {
                return None;
            }
        }
        let addresses = word_piece_owner_addresses(facts.clone(), 0);
        let piece = std::sync::Arc::new(WordPartPiece {
            kind,
            part_name: part_name.to_owned(),
            xml: xml.to_vec(),
            owners: Some((facts, relationships)),
        });
        Some((piece, addresses))
    }

    /// Put `piece` in place of the related part piece at `index`.
    pub(super) fn replace_part(&mut self, index: usize, piece: std::sync::Arc<WordPartPiece>) {
        self.parts[index] = piece;
    }

    /// The related part pieces: each one's kind, part name and bytes.
    pub(super) fn part_sources(
        &self,
    ) -> impl Iterator<Item = (oxoffice_docx::StoryKind, &str, &[u8])> {
        self.parts
            .iter()
            .map(|piece| (piece.kind, piece.part_name.as_str(), piece.xml.as_slice()))
    }

    /// The related part piece at `index`.
    pub(super) fn part_piece(&self, index: usize) -> std::sync::Arc<WordPartPiece> {
        std::sync::Arc::clone(&self.parts[index])
    }

    /// Put `parts` in place of the related part pieces, in the order of the
    /// document's related story parts.
    pub(super) fn replace_parts(&mut self, parts: Vec<std::sync::Arc<WordPartPiece>>) {
        self.parts = parts;
    }

    /// Keep body pieces aligned with an in-place splice of `document`.
    ///
    /// `removed` pieces at `slot` go. `inserted` is the block now at `slot`.
    /// A count or slot that does not match the document drops the body pieces
    /// and leaves related parts in place. Context is unchanged.
    pub(super) fn splice(
        &mut self,
        document: &oxoffice_docx::Document,
        slot: usize,
        removed: usize,
        inserted: Option<WordBlockOwners>,
    ) {
        let blocks = document.body_children();
        let added = usize::from(inserted.is_some());
        if self.blocks.len() + added != blocks.len() + removed || slot + removed > self.blocks.len()
        {
            self.blocks.clear();
            return;
        }
        let piece = inserted.map(|owners| {
            std::sync::Arc::new(WordBlockPiece {
                block: std::sync::Arc::new(blocks[slot].clone()),
                owners: Some(owners),
            })
        });
        self.blocks.splice(slot..slot + removed, piece);
    }

    /// Keep body pieces aligned with an in-place splice of `document` that
    /// put the blocks of `inserted` in place of `removed` pieces at `slot`.
    /// A count or slot that does not match the document drops the body
    /// pieces, as [`Self::splice`] does.
    pub(super) fn splice_blocks(
        &mut self,
        document: &oxoffice_docx::Document,
        slot: usize,
        removed: usize,
        inserted: Vec<WordBlockOwners>,
    ) {
        let blocks = document.body_children();
        if self.blocks.len() + inserted.len() != blocks.len() + removed
            || slot + removed > self.blocks.len()
        {
            self.blocks.clear();
            return;
        }
        let pieces = inserted
            .into_iter()
            .enumerate()
            .map(|(offset, owners)| {
                std::sync::Arc::new(WordBlockPiece {
                    block: std::sync::Arc::new(blocks[slot + offset].clone()),
                    owners: Some(owners),
                })
            })
            .collect::<Vec<_>>();
        self.blocks.splice(slot..slot + removed, pieces);
    }

    fn inventory(
        &self,
        document: &oxoffice_docx::Document,
    ) -> Option<(Vec<WordOwnerAddress>, WordIndexProjections)> {
        let main = self.context.as_ref()?.part_name();
        let body = oxoffice_docx::StoryId::new(oxoffice_docx::StoryKind::Body, main, 0);
        let mut links = None;
        let mut facts = WordOwnerFacts::default();
        let mut cells = WordOwnerFacts::default();
        let mut projections = WordIndexProjections {
            body_paragraphs: Some(Vec::new()),
            cell_paragraphs: HashMap::new(),
            table_slots: Vec::new(),
        };
        let mut cell_paragraphs = Some(HashMap::new());
        let mut routed = true;
        let (mut items, mut stories, mut tables) = (0, (0, 0), 0);
        for (slot, piece) in self.blocks.iter().enumerate() {
            let owners = piece.owners.as_ref()?;
            if !owners.relationships.is_empty() {
                let valid =
                    links.get_or_insert_with(|| document.story_hyperlink_relationships(main));
                if !owners
                    .relationships
                    .iter()
                    .all(|relationship| valid.contains(relationship.as_str()))
                {
                    return None;
                }
            }
            let place = |location: &oxoffice_docx::ContentLocation| {
                placed_route(location, &body, (items, stories))
            };
            let in_body = |location: &oxoffice_docx::ContentLocation| {
                location.story().kind() == oxoffice_docx::StoryKind::Body
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
            if let Some(paragraphs) = &mut projections.body_paragraphs {
                paragraphs.extend(owners.paragraphs.iter().map(|path| {
                    path.as_ref().map(|path| {
                        let mut location = oxoffice_docx::ContentLocation::new(
                            body.clone(),
                            oxoffice_docx::StoryItemKind::Paragraph,
                            path.clone(),
                        );
                        location.set_index_path_head(path[0] + items);
                        location
                    })
                }));
            }
            if matches!(
                piece.block.as_ref(),
                oxoffice_docx_oxml::BodyContent::Table(_)
            ) {
                projections.table_slots.push(Some(slot));
            } else {
                projections
                    .table_slots
                    .extend(std::iter::repeat_n(None, owners.tables));
            }
            if routed {
                match &owners.cell_paragraphs {
                    Some((routes, complete)) => {
                        if let Some(all) = &mut cell_paragraphs {
                            for ((table, row, cell), locations) in routes {
                                all.insert(
                                    (table + tables, *row, *cell),
                                    locations.iter().map(&place).collect(),
                                );
                            }
                        }
                        routed = *complete;
                    }
                    None => {
                        cell_paragraphs = None;
                        routed = false;
                    }
                }
            }
            items += owners.body_items;
            stories.0 += owners.cell_stories;
            stories.1 += owners.text_boxes;
            tables += owners.tables;
        }
        projections.cell_paragraphs = cell_paragraphs.unwrap_or_default();
        facts.items.append(&mut cells.items);
        facts.paragraphs.append(&mut cells.paragraphs);
        facts.links.append(&mut cells.links);
        for piece in &self.parts {
            let (owners, relationships) = piece.owners.as_ref()?;
            if !relationships.is_empty() {
                let valid = document.story_hyperlink_relationships(&piece.part_name);
                if !relationships
                    .iter()
                    .all(|relationship| valid.contains(relationship.as_str()))
                {
                    return None;
                }
            }
            facts.items.extend_from_slice(&owners.items);
            facts.cells.extend_from_slice(&owners.cells);
            facts.paragraphs.extend_from_slice(&owners.paragraphs);
            facts.links.extend_from_slice(&owners.links);
        }
        Some((
            word_piece_owner_addresses(facts, document.section_count()),
            projections,
        ))
    }
}

pub(super) fn word_owner_locations(
    document: &oxoffice_docx::Document,
    form_sources: Option<&mut FormSources>,
) -> oxoffice_docx::Result<Vec<WordOwnerAddress>> {
    let table_facts = word_table_read_facts(document);
    word_owner_addresses(
        document.story_owner_snapshots()?,
        &table_facts,
        Some(document),
        form_sources,
    )
}

/// The owner inventory of the given snapshots. Sections are those of
/// `document`, and a snapshot of a detached item has none.
pub(super) fn word_owner_addresses(
    snapshots: WordOwnerSnapshots,
    table_facts: &HashMap<oxoffice_docx::ContentLocation, WordReadFacts>,
    document: Option<&oxoffice_docx::Document>,
    mut form_sources: Option<&mut FormSources>,
) -> oxoffice_docx::Result<Vec<WordOwnerAddress>> {
    let entries = with_control_tables(&snapshots).ok_or_else(|| {
        oxoffice_docx::Error::Other("a block control table has no place in its story".into())
    })?;
    let oxoffice_docx::StoryOwnerSnapshots {
        items: snapshots,
        cells: cell_snapshots,
        paragraphs: paragraph_snapshots,
        links: link_snapshots,
        control_tables,
    } = snapshots;
    let mut result = Vec::new();
    let mut table_ordinals = HashMap::new();
    if let Some(sources) = form_sources.as_deref_mut() {
        let tables = snapshots
            .iter()
            .filter(|snapshot| {
                snapshot.location().item_kind() == oxoffice_docx::StoryItemKind::Table
            })
            .map(|snapshot| (snapshot.location(), snapshot.xml()))
            .chain(
                control_tables
                    .iter()
                    .map(|(location, _, xml)| (location, xml.as_slice())),
            );
        for (location, xml) in tables {
            let key = FormOwnerRoute::Table(form_source_key(location));
            sources
                .xml
                .entry(key)
                .and_modify(|xml| *xml = None)
                .or_insert_with(|| Some(xml.to_vec()));
        }
        // Preserve raw multiplicity before the legacy cell-parent map collapses it.
        for (table, row, cell, story) in &cell_snapshots {
            let story_key = (
                story.kind(),
                story.part_name().to_owned(),
                story.owner_index(),
            );
            *sources.cell_stories.entry(story_key.clone()).or_default() += 1;
            *sources
                .cells
                .entry(FormOwnerRoute::Cell(
                    form_source_key(table),
                    *row,
                    *cell,
                    story_key,
                ))
                .or_default() += 1;
        }
    }
    let mut body_slots = HashMap::new();
    let mut cell_parents = HashMap::new();
    let mut table_cells = HashMap::<oxoffice_docx::ContentLocation, Vec<_>>::new();
    for (table, row, cell, story) in cell_snapshots {
        table_cells
            .entry(table)
            .or_default()
            .push((row, cell, story));
    }
    for snapshot in &snapshots {
        let location = snapshot.location();
        if location.story().kind() == oxoffice_docx::StoryKind::Body
            && snapshot.is_direct_child()
            && matches!(
                location.item_kind(),
                oxoffice_docx::StoryItemKind::Paragraph
                    | oxoffice_docx::StoryItemKind::ContentControl
            )
            && let Some(slot) = snapshot.direct_body_index()
        {
            body_slots.insert(location.clone(), slot);
        }
    }
    // Every table outside cells of each story, in document order.
    for entry in entries {
        let (location, direct, read_facts) = match entry {
            StoryEntry::Item(index) => {
                let snapshot = &snapshots[index];
                if snapshot.location().item_kind() != oxoffice_docx::StoryItemKind::Table {
                    continue;
                }
                let location = snapshot.location().clone();
                let read_facts = table_facts.get(&location).cloned().unwrap_or_default();
                let direct = snapshot
                    .direct_body_index()
                    .filter(|_| snapshot.is_direct_child());
                (location, direct, read_facts)
            }
            StoryEntry::ControlTable(index) => {
                let (location, _, xml) = &control_tables[index];
                (
                    location.clone(),
                    None,
                    word_table_facts(xml).unwrap_or_default(),
                )
            }
        };
        let ordinal = table_ordinals
            .entry(location.story().clone())
            .or_insert(0usize);
        let projection_ordinal = Some(*ordinal);
        *ordinal += 1;
        let table_index = result.len();
        result.push(WordOwnerAddress {
            location: WordOwnerLocation::Table(location.clone()),
            parent: None,
            projection_ordinal,
            read_facts,
            body_slot: content_body_slot(&location, direct, &body_slots),
        });
        for (row, cell, story) in table_cells.remove(&location).unwrap_or_default() {
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
    // Join nested table routes through their actual physical cell owner. An
    // unresolved route stays conservative rather than acquiring an ordinal parent.
    for address in &mut result {
        if let WordOwnerLocation::Table(location) = &address.location {
            let story = location.story();
            let parent = cell_parents
                .get(&(
                    story.kind(),
                    story.part_name().to_owned(),
                    story.owner_index(),
                ))
                .copied();
            address.parent = parent;
        }
    }
    let mut paragraph_ordinals = HashMap::new();
    for (location, xml) in paragraph_snapshots {
        if let Some(sources) = form_sources.as_deref_mut() {
            let key = FormOwnerRoute::Paragraph(form_source_key(&location));
            sources
                .xml
                .entry(key)
                .and_modify(|source| *source = None)
                .or_insert_with(|| Some(xml.clone()));
        }
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
        let paragraph = oxoffice_docx_oxml::CT_P::from_xml_fragment(&xml)?;
        // Reuse the containing item's fact from this exact immutable source
        // inventory, matching the native paragraph snapshot's containing route.
        let body_slot =
            content_body_slot(&location, body_slots.get(&location).copied(), &body_slots);
        result.push(WordOwnerAddress {
            location: WordOwnerLocation::Paragraph(location.clone()),
            parent,
            body_slot,
            projection_ordinal,
            read_facts: WordReadFacts::Paragraph(paragraph.accepted_text()),
        });
        for path in paragraph.accepted_run_paths() {
            result.push(WordOwnerAddress {
                location: WordOwnerLocation::Run(location.clone(), path),
                parent: Some(index),
                body_slot: None,
                projection_ordinal: None,
                read_facts: WordReadFacts::None,
            });
        }
    }
    for index in 0..document.map_or(0, oxoffice_docx::Document::section_count) {
        if let (Some(sources), Some(document)) = (form_sources.as_deref_mut(), document) {
            sources.xml.insert(
                FormOwnerRoute::Section(index),
                Some(document.section_properties_xml(index)?),
            );
        }
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
    for (location, _) in link_snapshots {
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
    Ok(result)
}

pub(super) fn validate_word_facts(
    inner: &oxoffice_docx::Document,
    py: Python<'_>,
) -> PyResult<(Vec<WordOwnerAddress>, WordIndexProjections, WordPieces)> {
    let (pieces, inventory) = WordPieces::default().assemble(inner);
    let (locations, projections) = match inventory {
        Some((locations, projections)) => {
            #[cfg(debug_assertions)]
            assert_word_pieces_rebuild(inner, &locations, &projections);
            (locations, Some(projections))
        }
        None => (
            word_owner_locations(inner, None).map_err(|error| docx_to_pyerr(py, error))?,
            None,
        ),
    };
    let projections = projections.unwrap_or_else(|| WordIndexProjections::of(inner));
    Ok((locations, projections, pieces))
}
