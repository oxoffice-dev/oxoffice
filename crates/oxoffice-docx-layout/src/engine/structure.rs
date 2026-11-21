//! Document structure: the tagging tree, lists, headings and navigation.

use super::*;

struct StructureBuilder {
    nodes: Vec<StructureNode>,
}

impl StructureBuilder {
    fn add(&mut self, role: StructureRole, parent: Option<StructureId>) -> StructureId {
        let id = StructureId::new(self.nodes.len() as u32 + 1)
            .expect("a structure node index is always non-zero");
        self.nodes.push(StructureNode {
            id,
            role,
            children: Vec::new(),
            alternate_text: None,
        });
        if let Some(parent) = parent
            && let Some(node) = self.nodes.get_mut(parent.get() as usize - 1)
        {
            node.children.push(id);
        }
        id
    }

    fn set_alternate_text(&mut self, id: StructureId, text: String) {
        if let Some(node) = self.nodes.get_mut(id.get() as usize - 1) {
            node.alternate_text = Some(text);
        }
    }
}

#[derive(Clone, Copy)]
struct ListFrame {
    num_id: u32,
    level: u8,
    list: StructureId,
    last_item: Option<StructureId>,
}

pub(super) fn assign_shared_document_structure(
    sections: &mut [paginator::SharedSection],
) -> DocumentStructure {
    let mut builder = StructureBuilder { nodes: Vec::new() };
    let root = builder.add(StructureRole::Document, None);

    for section in sections {
        let mut lists: Vec<ListFrame> = Vec::new();
        for block in &mut section.blocks {
            match block {
                SharedLayoutBlock::Paragraph {
                    block, semantics, ..
                } => {
                    lists.clear();
                    let role = block
                        .heading_level
                        .map(|level| StructureRole::Heading(level.min(6) as u8))
                        .unwrap_or(StructureRole::Paragraph);
                    let paragraph_id = builder.add(role, Some(root));
                    semantics.structure_id = Some(paragraph_id);
                    debug_assert!(block.list.is_none());
                    debug_assert!(block.anchored.is_empty());
                }
                SharedLayoutBlock::Table {
                    block, semantics, ..
                } => {
                    lists.clear();
                    assign_shared_table_structure(&mut builder, block, semantics, root);
                }
                SharedLayoutBlock::Owned {
                    block, semantics, ..
                } => match block.as_mut() {
                    LayoutBlock::Paragraph(paragraph) => {
                        assign_owned_paragraph_structure(&mut builder, paragraph, root, &mut lists);
                        if let Some(semantics) = semantics {
                            semantics.structure_id = paragraph.structure_id;
                        }
                    }
                    LayoutBlock::Table(table) => {
                        lists.clear();
                        assign_owned_table_structure(&mut builder, table, root);
                    }
                },
            }
        }
    }

    DocumentStructure {
        root,
        nodes: builder.nodes,
    }
}

fn assign_owned_paragraph_structure(
    builder: &mut StructureBuilder,
    paragraph: &mut ParagraphBlock,
    root: StructureId,
    lists: &mut Vec<ListFrame>,
) {
    if let Some((num_id, level)) = paragraph.list {
        if lists.last().is_some_and(|frame| frame.num_id != num_id) {
            lists.clear();
        }
        while lists.last().is_some_and(|frame| frame.level > level) {
            lists.pop();
        }
        if lists.is_empty() || lists.last().is_some_and(|frame| frame.level < level) {
            let parent = lists
                .last()
                .and_then(|frame| frame.last_item)
                .unwrap_or(root);
            let list = builder.add(StructureRole::List, Some(parent));
            lists.push(ListFrame {
                num_id,
                level,
                list,
                last_item: None,
            });
        }
        let frame = lists
            .last_mut()
            .expect("the requested list level has been allocated");
        let item = builder.add(StructureRole::ListItem, Some(frame.list));
        frame.last_item = Some(item);
        let paragraph_id = builder.add(StructureRole::Paragraph, Some(item));
        paragraph.structure_id = Some(paragraph_id);
        assign_paragraph_figures(builder, paragraph, paragraph_id, item);
    } else {
        lists.clear();
        let role = paragraph
            .heading_level
            .map(|level| StructureRole::Heading(level.min(6) as u8))
            .unwrap_or(StructureRole::Paragraph);
        let paragraph_id = builder.add(role, Some(root));
        paragraph.structure_id = Some(paragraph_id);
        assign_paragraph_figures(builder, paragraph, paragraph_id, root);
    }
}

fn assign_owned_table_structure(
    builder: &mut StructureBuilder,
    table: &mut table::TableBlock,
    parent: StructureId,
) {
    let table_id = builder.add(StructureRole::Table, Some(parent));
    table.structure_id = Some(table_id);
    for row in &mut table.rows {
        let row_id = builder.add(StructureRole::TableRow, Some(table_id));
        row.structure_id = Some(row_id);
        for cell in &mut row.cells {
            let role = if row.is_header {
                StructureRole::TableHeaderCell
            } else {
                StructureRole::TableCell
            };
            let cell_id = builder.add(role, Some(row_id));
            cell.structure_id = Some(cell_id);
            for block in &mut cell.blocks {
                assign_cell_block_structure(builder, block, cell_id);
            }
        }
    }
}

fn assign_shared_table_structure(
    builder: &mut StructureBuilder,
    table: &table::TableBlock,
    semantics: &mut TableSemantics,
    parent: StructureId,
) {
    let table_id = builder.add(StructureRole::Table, Some(parent));
    for (row, row_semantics) in table.rows.iter().zip(&mut semantics.rows) {
        let row_id = builder.add(StructureRole::TableRow, Some(table_id));
        for (cell, cell_semantics) in row.cells.iter().zip(&mut row_semantics.cells) {
            let role = if row.is_header {
                StructureRole::TableHeaderCell
            } else {
                StructureRole::TableCell
            };
            let cell_id = builder.add(role, Some(row_id));
            for (block, block_semantics) in cell.blocks.iter().zip(&mut cell_semantics.blocks) {
                match (block, block_semantics) {
                    (
                        table::CellBlock::Paragraph(paragraph),
                        CellBlockSemantics::Paragraph(semantics),
                    ) => {
                        let role = paragraph
                            .heading_level
                            .map(|level| StructureRole::Heading(level.min(6) as u8))
                            .unwrap_or(StructureRole::Paragraph);
                        semantics.structure_id = Some(builder.add(role, Some(cell_id)));
                    }
                    (table::CellBlock::Table(table), CellBlockSemantics::Table(semantics)) => {
                        assign_shared_table_structure(builder, table, semantics, cell_id)
                    }
                    _ => unreachable!("shared table semantics stay aligned"),
                }
            }
        }
    }
}

fn assign_cell_block_structure(
    builder: &mut StructureBuilder,
    block: &mut table::CellBlock,
    parent: StructureId,
) {
    match block {
        table::CellBlock::Paragraph(paragraph) => {
            let role = paragraph
                .heading_level
                .map(|level| StructureRole::Heading(level.min(6) as u8))
                .unwrap_or(StructureRole::Paragraph);
            let paragraph_id = builder.add(role, Some(parent));
            paragraph.structure_id = Some(paragraph_id);
            assign_paragraph_figures(builder, paragraph, paragraph_id, parent);
        }
        table::CellBlock::Table(table) => {
            let table_id = builder.add(StructureRole::Table, Some(parent));
            table.structure_id = Some(table_id);
            for row in &mut table.rows {
                let row_id = builder.add(StructureRole::TableRow, Some(table_id));
                row.structure_id = Some(row_id);
                for cell in &mut row.cells {
                    let role = if row.is_header {
                        StructureRole::TableHeaderCell
                    } else {
                        StructureRole::TableCell
                    };
                    let cell_id = builder.add(role, Some(row_id));
                    cell.structure_id = Some(cell_id);
                    for child in &mut cell.blocks {
                        assign_cell_block_structure(builder, child, cell_id);
                    }
                }
            }
        }
    }
}

fn assign_paragraph_figures(
    builder: &mut StructureBuilder,
    paragraph: &mut ParagraphBlock,
    inline_parent: StructureId,
    anchored_parent: StructureId,
) {
    for line in &mut paragraph.lines {
        for item in &mut line.items {
            let (alternate_text, structure_id) = match item {
                LineItem::Figure {
                    alternate_text,
                    structure_id,
                    ..
                } => (alternate_text, structure_id),
                _ => continue,
            };
            let figure = builder.add(StructureRole::Figure, Some(inline_parent));
            builder.set_alternate_text(figure, alternate_text.clone());
            *structure_id = Some(figure);
        }
    }
    for drawing in &mut paragraph.anchored {
        if let Some(text) = drawing
            .alternate_text
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            let figure = builder.add(StructureRole::Figure, Some(anchored_parent));
            builder.set_alternate_text(figure, text.to_owned());
            drawing.structure_id = Some(figure);
        }
    }
}

pub(super) fn excluded_owner_for_source(input: &LayoutInput, source: SourceNodeId) -> bool {
    input.sequence_snapshot.as_ref().is_some_and(|snapshot| {
        snapshot
            .source_node(source)
            .is_some_and(|path| snapshot.navigation_exclusions.contains_key(path))
    })
}

// Cache-safe tables have direct rows/cells and no removed revisions or controls.
// Replay the returned layout's actual omission decisions against current source paths.
pub(super) fn record_cached_table_omissions(
    source: &CT_Tbl,
    block: &table::TableBlock,
    sources: &SourceRegistry,
    story: &WordStory,
    path: &[usize],
    state: &mut NumberingState,
) {
    for (ri, (row, laid_out)) in source.rows.iter().zip(&block.rows).enumerate() {
        for (ci, (cell, placed)) in row.cells.iter().zip(&laid_out.cells).enumerate() {
            let mut cell_path = path.to_vec();
            cell_path.extend([ri, ci]);
            if placed.is_vmerge_continue {
                sources.record_table_omission(story, &cell_path, state);
                continue;
            }
            for (index, (content, block)) in cell.content.iter().zip(&placed.blocks).enumerate() {
                if let (CellContent::Table(source), table::CellBlock::Table(block)) =
                    (content, block)
                {
                    let mut nested = cell_path.clone();
                    nested.push(index);
                    record_cached_table_omissions(source, block, sources, story, &nested, state);
                }
            }
        }
    }
}

pub(super) fn record_table_navigation(
    table: &crate::table::TableBlock,
    semantics: &TableSemantics,
    sources: &SourceRegistry,
    state: &mut NumberingState,
) {
    for (row, row_semantics) in table.rows.iter().zip(&semantics.rows) {
        for (cell, cell_semantics) in row.cells.iter().zip(&row_semantics.cells) {
            for (block, semantics) in cell.blocks.iter().zip(&cell_semantics.blocks) {
                match (block, semantics) {
                    (
                        crate::table::CellBlock::Paragraph(paragraph),
                        CellBlockSemantics::Paragraph(semantics),
                    ) => {
                        if let Some((source, original)) = semantics.source_node.and_then(|node| {
                            sources
                                .navigation_sources
                                .get(&node)
                                .map(|facts| (node, facts))
                        }) {
                            let mut facts = state
                                .remove_navigation(source)
                                .unwrap_or_else(|| original.clone());
                            facts.level = paragraph.heading_level;
                            state.record_navigation(source, facts);
                        }
                    }
                    (
                        crate::table::CellBlock::Table(table),
                        CellBlockSemantics::Table(semantics),
                    ) => {
                        record_table_navigation(table, semantics, sources, state);
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Detect if a paragraph has a heading style, returning the level (1-9).
pub(super) fn detect_heading_level(para: &CT_P, styles: &CT_Styles) -> Option<u32> {
    let style_id = para.properties.as_ref()?.style_id.as_deref()?;
    // Check if style ID matches "Heading1" .. "Heading9"
    if let Some(rest) = style_id.strip_prefix("Heading") {
        return rest.parse::<u32>().ok().filter(|n| (1..=9).contains(n));
    }
    // Also check style name in the styles definitions
    if let Some(style_def) = styles.get_by_id(style_id)
        && let Some(ref name) = style_def.name
        && let Some(rest) = name.strip_prefix("heading ")
    {
        return rest.parse::<u32>().ok().filter(|n| (1..=9).contains(n));
    }
    None
}

/// Whether `style_id` names a table of contents level, "toc 1" to "toc 9".
pub(super) fn is_toc_paragraph_style(style_id: &str, styles: &CT_Styles) -> bool {
    let id_level = |id: &str| {
        id.get(..3)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("toc"))
            && id[3..]
                .trim()
                .parse::<u32>()
                .is_ok_and(|n| (1..=9).contains(&n))
    };
    // Match the existing TOC style-name lookup: the whole name is trimmed and
    // compared to "toc N". A producer name such as "toc 1 " therefore matches.
    let name_level =
        |name: &str| (1..=9).any(|n| name.trim().eq_ignore_ascii_case(&format!("toc {n}")));
    id_level(style_id)
        || styles
            .get_by_id(style_id)
            .and_then(|style| style.name.as_deref())
            .is_some_and(name_level)
}

/// Whether `style_id` is the Hyperlink character style.
pub(super) fn is_hyperlink_character_style(style_id: &str, styles: &CT_Styles) -> bool {
    style_id == "Hyperlink"
        || styles
            .get_by_id(style_id)
            .and_then(|style| style.name.as_deref())
            .is_some_and(|name| name.eq_ignore_ascii_case("Hyperlink"))
}
