//! Walking the body model: nth paragraph, table and cell, counts, visitors, export items and drawings.

use super::*;

fn take_paragraph<'a>(paragraph: &'a mut CT_P, index: &mut usize) -> Option<&'a mut CT_P> {
    if *index == 0 {
        Some(paragraph)
    } else {
        *index -= 1;
        None
    }
}

pub(super) fn nth_paragraph_in_body<'a>(
    content: &'a mut [BodyContent],
    index: &mut usize,
) -> Option<&'a mut CT_P> {
    for child in content {
        let paragraph = match child {
            BodyContent::Paragraph(paragraph) => take_paragraph(paragraph, index),
            BodyContent::ContentControl(control) => nth_paragraph_in_control(control, index),
            BodyContent::Table(_) | BodyContent::RawXml(_) => None,
        };
        if paragraph.is_some() {
            return paragraph;
        }
    }
    None
}

fn nth_paragraph_in_control<'a>(
    control: &'a mut CT_Sdt,
    index: &mut usize,
) -> Option<&'a mut CT_P> {
    for child in &mut control.content {
        let paragraph = match child {
            SdtContent::Paragraph(paragraph) => take_paragraph(paragraph, index),
            SdtContent::Table(table) => nth_paragraph_in_table(table, index),
            SdtContent::Row(row) => nth_paragraph_in_row(row, index),
            SdtContent::Cell(cell) => nth_paragraph_in_cell(cell, index),
            SdtContent::ContentControl(control) => nth_paragraph_in_control(control, index),
            SdtContent::Run(_) | SdtContent::RawXml(_) => None,
        };
        if paragraph.is_some() {
            return paragraph;
        }
    }
    None
}

fn paragraph_count_in_control(control: &CT_Sdt) -> usize {
    control
        .content
        .iter()
        .map(|child| match child {
            SdtContent::Paragraph(_) => 1,
            SdtContent::Table(table) => paragraph_count_in_table(table),
            SdtContent::Row(row) => paragraph_count_in_row(row),
            SdtContent::Cell(cell) => paragraph_count_in_cell(cell),
            SdtContent::ContentControl(control) => paragraph_count_in_control(control),
            SdtContent::Run(_) | SdtContent::RawXml(_) => 0,
        })
        .sum()
}

fn paragraph_count_in_table(table: &CT_Tbl) -> usize {
    table
        .content_controls
        .iter()
        .map(|(_, _, control)| paragraph_count_in_control(control))
        .sum::<usize>()
        + table.rows.iter().map(paragraph_count_in_row).sum::<usize>()
}

fn paragraph_count_in_row(row: &CT_Row) -> usize {
    row.content_controls
        .iter()
        .map(|(_, _, control)| paragraph_count_in_control(control))
        .sum::<usize>()
        + row.cells.iter().map(paragraph_count_in_cell).sum::<usize>()
}

fn paragraph_count_in_cell(cell: &CT_Tc) -> usize {
    cell.content
        .iter()
        .map(|child| match child {
            CellContent::Paragraph(_) => 1,
            CellContent::ContentControl(control) => paragraph_count_in_control(control),
            CellContent::Table(_) => 0,
        })
        .sum()
}

fn nth_paragraph_in_table<'a>(table: &'a mut CT_Tbl, index: &mut usize) -> Option<&'a mut CT_P> {
    let CT_Tbl {
        rows,
        content_controls,
        ..
    } = table;
    let mut selected_control = None;
    let mut selected_row = None;
    for boundary in 0..=rows.len() {
        for (control_index, (_, _, control)) in content_controls
            .iter()
            .enumerate()
            .filter(|(_, (at, _, _))| *at == boundary)
        {
            let count = paragraph_count_in_control(control);
            if *index < count {
                selected_control = Some(control_index);
                break;
            }
            *index -= count;
        }
        if selected_control.is_some() {
            break;
        }
        if let Some(row) = rows.get(boundary) {
            let count = paragraph_count_in_row(row);
            if *index < count {
                selected_row = Some(boundary);
                break;
            }
            *index -= count;
        }
    }
    if let Some(control_index) = selected_control {
        nth_paragraph_in_control(&mut content_controls[control_index].2, index)
    } else if let Some(row_index) = selected_row {
        nth_paragraph_in_row(&mut rows[row_index], index)
    } else {
        None
    }
}

fn nth_paragraph_in_row<'a>(row: &'a mut CT_Row, index: &mut usize) -> Option<&'a mut CT_P> {
    let CT_Row {
        cells,
        content_controls,
        ..
    } = row;
    let mut selected_control = None;
    let mut selected_cell = None;
    for boundary in 0..=cells.len() {
        for (control_index, (_, _, control)) in content_controls
            .iter()
            .enumerate()
            .filter(|(_, (at, _, _))| *at == boundary)
        {
            let count = paragraph_count_in_control(control);
            if *index < count {
                selected_control = Some(control_index);
                break;
            }
            *index -= count;
        }
        if selected_control.is_some() {
            break;
        }
        if let Some(cell) = cells.get(boundary) {
            let count = paragraph_count_in_cell(cell);
            if *index < count {
                selected_cell = Some(boundary);
                break;
            }
            *index -= count;
        }
    }
    if let Some(control_index) = selected_control {
        nth_paragraph_in_control(&mut content_controls[control_index].2, index)
    } else if let Some(cell_index) = selected_cell {
        nth_paragraph_in_cell(&mut cells[cell_index], index)
    } else {
        None
    }
}

fn nth_paragraph_in_cell<'a>(cell: &'a mut CT_Tc, index: &mut usize) -> Option<&'a mut CT_P> {
    for child in &mut cell.content {
        let paragraph = match child {
            CellContent::Paragraph(paragraph) => take_paragraph(paragraph, index),
            CellContent::ContentControl(control) => nth_paragraph_in_control(control, index),
            CellContent::Table(_) => None,
        };
        if paragraph.is_some() {
            return paragraph;
        }
    }
    None
}

fn take_table<'a>(table: &'a mut CT_Tbl, index: &mut usize) -> Option<&'a mut CT_Tbl> {
    if *index == 0 {
        Some(table)
    } else {
        *index -= 1;
        None
    }
}

pub(super) fn nth_table_in_body_ref<'a>(
    content: &'a [BodyContent],
    index: &mut usize,
) -> Option<&'a CT_Tbl> {
    fn take<'a>(table: &'a CT_Tbl, index: &mut usize) -> Option<&'a CT_Tbl> {
        if *index == 0 {
            Some(table)
        } else {
            *index -= 1;
            None
        }
    }
    fn in_cell<'a>(cell: &'a CT_Tc, index: &mut usize) -> Option<&'a CT_Tbl> {
        for child in &cell.content {
            let found = match child {
                CellContent::Table(table) => take(table, index),
                CellContent::ContentControl(control) => in_control(control, index),
                CellContent::Paragraph(_) => None,
            };
            if found.is_some() {
                return found;
            }
        }
        None
    }
    fn in_control<'a>(control: &'a CT_Sdt, index: &mut usize) -> Option<&'a CT_Tbl> {
        for child in &control.content {
            let found = match child {
                SdtContent::Table(table) => take(table, index),
                SdtContent::Cell(cell) => in_cell(cell, index),
                SdtContent::ContentControl(control) => in_control(control, index),
                _ => None,
            };
            if found.is_some() {
                return found;
            }
        }
        None
    }
    for child in content {
        let found = match child {
            BodyContent::Table(table) => take(table, index),
            BodyContent::ContentControl(control) => in_control(control, index),
            BodyContent::Paragraph(_) | BodyContent::RawXml(_) => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

pub(super) fn nth_table_in_body<'a>(
    content: &'a mut [BodyContent],
    index: &mut usize,
) -> Option<&'a mut CT_Tbl> {
    for child in content {
        let table = match child {
            BodyContent::Table(table) => take_table(table, index),
            BodyContent::ContentControl(control) => nth_table_in_control(control, index),
            BodyContent::Paragraph(_) | BodyContent::RawXml(_) => None,
        };
        if table.is_some() {
            return table;
        }
    }
    None
}

fn nth_table_in_control<'a>(control: &'a mut CT_Sdt, index: &mut usize) -> Option<&'a mut CT_Tbl> {
    for child in &mut control.content {
        let table = match child {
            SdtContent::Table(table) => take_table(table, index),
            SdtContent::Cell(cell) => nth_table_in_cell(cell, index),
            SdtContent::ContentControl(control) => nth_table_in_control(control, index),
            SdtContent::Paragraph(_)
            | SdtContent::Row(_)
            | SdtContent::Run(_)
            | SdtContent::RawXml(_) => None,
        };
        if table.is_some() {
            return table;
        }
    }
    None
}

fn nth_table_in_cell<'a>(cell: &'a mut CT_Tc, index: &mut usize) -> Option<&'a mut CT_Tbl> {
    for child in &mut cell.content {
        let table = match child {
            CellContent::Table(table) => take_table(table, index),
            CellContent::ContentControl(control) => nth_table_in_control(control, index),
            CellContent::Paragraph(_) => None,
        };
        if table.is_some() {
            return table;
        }
    }
    None
}

// Optional routes from one checked physical inventory. Both source consumers
// keep their original lookup if ordering or containment cannot be qualified.
pub(super) fn modeled_main_cell_routes(
    owners: &[StoryOwnerSpan],
    direct_items: &[StoryItemSpan],
) -> Option<HashMap<usize, (usize, usize)>> {
    if direct_items
        .windows(2)
        .any(|pair| pair[0].full.end > pair[1].full.start)
    {
        return None;
    }
    let mut routes = HashMap::new();
    let mut ordinals = HashMap::<usize, usize>::new();
    let mut direct = 0;
    let mut previous = None;
    for owner in owners {
        if owner.kind != StoryKind::TableCell {
            continue;
        }
        if previous.is_some_and(|start| start > owner.full.start) {
            return None;
        }
        previous = Some(owner.full.start);
        loop {
            let item = direct_items.get(direct)?;
            if item.full.end <= owner.full.start {
                direct += 1;
                continue;
            }
            if item.full.start > owner.full.start
                || owner.full.end > item.full.end
                || !matches!(
                    item.kind,
                    StoryItemKind::Table | StoryItemKind::ContentControl
                )
            {
                return None;
            }
            let ordinal = ordinals.entry(direct).or_default();
            if routes
                .insert(owner.owner_index, (direct, *ordinal))
                .is_some()
            {
                return None;
            }
            *ordinal = ordinal.checked_add(1)?;
            break;
        }
    }
    Some(routes)
}

pub(super) fn modeled_main_cell_route(
    xml: &[u8],
    target: &StoryOwnerSpan,
) -> Result<(usize, usize)> {
    let body = scan_story_owners(xml, StoryKind::Body)?
        .into_iter()
        .find(|owner| owner.kind == StoryKind::Body)
        .ok_or_else(|| Error::Other("main document body owner was not found".to_owned()))?;
    let direct_items = direct_story_content_items(xml, &body)?;
    let (content_index, direct_item) = direct_items
        .iter()
        .enumerate()
        .find(|(_, item)| item.full.start <= target.full.start && target.full.end <= item.full.end)
        .ok_or_else(|| Error::Other("table-cell owner has no direct body container".to_owned()))?;
    if !matches!(
        direct_item.kind,
        StoryItemKind::Table | StoryItemKind::ContentControl
    ) {
        return Err(Error::Other(
            "HTML fragments require a modeled main-document table-cell owner".to_owned(),
        ));
    }
    let cell_index = scan_story_owners(xml, StoryKind::Body)?
        .into_iter()
        .filter(|owner| {
            owner.kind == StoryKind::TableCell
                && direct_item.full.start <= owner.full.start
                && owner.full.end <= direct_item.full.end
        })
        .position(|owner| owner.full == target.full)
        .ok_or_else(|| Error::Other("modeled table-cell owner was not found".to_owned()))?;
    Ok((content_index, cell_index))
}

/// [`Document::cell_paragraph_story_locations`] of `body`, serialized as
/// `xml` with the story owners `owners` in the main part `part_name`, and
/// whether it routed every table cell owner. Like the single lookup, which
/// fails on the first owner it cannot route, the inventory stops there.
#[allow(clippy::type_complexity)] // The located paragraphs of each cell and one fact.
pub(super) fn cell_paragraph_routes(
    body: &mut CT_Body,
    xml: &[u8],
    owners: &[StoryOwnerSpan],
    part_name: &str,
) -> Result<(HashMap<(usize, usize, usize), Vec<ContentLocation>>, bool)> {
    let addresses = crate::table::cell_addresses(body.tables());
    let body_owner = owners
        .iter()
        .find(|owner| owner.kind == StoryKind::Body)
        .ok_or_else(|| Error::Other("main document body owner was not found".to_owned()))?;
    let direct_items = direct_story_content_items(xml, body_owner)?;
    let routes = modeled_main_cell_routes(owners, &direct_items);
    let mut ordinals = HashMap::<usize, usize>::new();
    let mut selected = Vec::new();
    let mut seen = HashSet::new();
    let mut complete = true;
    for owner in owners
        .iter()
        .filter(|owner| owner.kind == StoryKind::TableCell)
    {
        // The single lookup fails on the first owner it cannot route,
        // whatever its target, so the inventory stops there too.
        let Some((content, direct_item)) = routes
            .as_ref()
            .and_then(|routes| routes.get(&owner.owner_index))
            .and_then(|(content, _)| direct_items.get(*content).map(|item| (*content, item)))
            .or_else(|| {
                direct_items.iter().enumerate().find(|(_, item)| {
                    item.full.start <= owner.full.start && owner.full.end <= item.full.end
                })
            })
        else {
            complete = false;
            break;
        };
        if !matches!(
            direct_item.kind,
            StoryItemKind::Table | StoryItemKind::ContentControl
        ) {
            complete = false;
            break;
        }
        let counter = ordinals.entry(content).or_insert(0);
        let mut ordinal = *counter;
        *counter += 1;
        let modeled = match body.content.get_mut(content) {
            Some(BodyContent::Table(table)) => nth_cell_in_table(table.as_mut(), &mut ordinal),
            Some(BodyContent::ContentControl(control)) => {
                nth_cell_in_control(control.as_mut(), &mut ordinal)
            }
            _ => None,
        };
        let Some((pointer, count)) =
            modeled.map(|cell| (&*cell as *const CT_Tc, cell.paragraphs().len()))
        else {
            continue;
        };
        let Some(address) = addresses.get(&pointer) else {
            continue;
        };
        if !seen.insert(*address) {
            continue;
        }
        let story = StoryId {
            kind: owner.kind,
            part_name: part_name.to_owned(),
            owner_index: owner.owner_index,
            fingerprint: owner.fingerprint,
        };
        selected.push((*address, count, (story, owner.clone())));
    }
    let pairs = selected
        .iter()
        .map(|(_, _, pair)| pair.clone())
        .collect::<Vec<_>>();
    let spans = story_range_paragraph_spans(xml, &pairs)?;
    let mut result = HashMap::with_capacity(selected.len());
    for ((address, count, _), mut paragraphs) in selected.into_iter().zip(spans) {
        paragraphs.sort_by_key(|(_, span)| span.start);
        if paragraphs.len() == count {
            result.insert(
                address,
                paragraphs
                    .into_iter()
                    .map(|(location, _)| location)
                    .collect(),
            );
        }
    }
    Ok((result, complete))
}

pub(super) fn nth_cell_in_control<'a>(
    control: &'a mut CT_Sdt,
    index: &mut usize,
) -> Option<&'a mut CT_Tc> {
    for child in &mut control.content {
        let cell = match child {
            SdtContent::Table(table) => nth_cell_in_table(table, index),
            SdtContent::Row(row) => nth_cell_in_row(row, index),
            SdtContent::Cell(cell) => take_cell(cell, index),
            SdtContent::ContentControl(control) => nth_cell_in_control(control, index),
            SdtContent::Paragraph(_) | SdtContent::Run(_) | SdtContent::RawXml(_) => None,
        };
        if cell.is_some() {
            return cell;
        }
    }
    None
}

pub(super) fn nth_cell_in_table<'a>(
    table: &'a mut CT_Tbl,
    index: &mut usize,
) -> Option<&'a mut CT_Tc> {
    let CT_Tbl {
        rows,
        content_controls,
        ..
    } = table;
    let mut selected_control = None;
    let mut selected_row = None;
    for boundary in 0..=rows.len() {
        for (control_index, (_, _, control)) in content_controls
            .iter()
            .enumerate()
            .filter(|(_, (at, _, _))| *at == boundary)
        {
            let count = cell_count_in_control(control);
            if *index < count {
                selected_control = Some(control_index);
                break;
            }
            *index -= count;
        }
        if selected_control.is_some() {
            break;
        }
        if let Some(row) = rows.get(boundary) {
            let count = cell_count_in_row(row);
            if *index < count {
                selected_row = Some(boundary);
                break;
            }
            *index -= count;
        }
    }
    if let Some(control_index) = selected_control {
        nth_cell_in_control(&mut content_controls[control_index].2, index)
    } else if let Some(row_index) = selected_row {
        nth_cell_in_row(&mut rows[row_index], index)
    } else {
        None
    }
}

fn nth_cell_in_row<'a>(row: &'a mut CT_Row, index: &mut usize) -> Option<&'a mut CT_Tc> {
    let CT_Row {
        cells,
        content_controls,
        ..
    } = row;
    let mut selected_control = None;
    let mut selected_cell = None;
    for boundary in 0..=cells.len() {
        for (control_index, (_, _, control)) in content_controls
            .iter()
            .enumerate()
            .filter(|(_, (at, _, _))| *at == boundary)
        {
            let count = cell_count_in_control(control);
            if *index < count {
                selected_control = Some(control_index);
                break;
            }
            *index -= count;
        }
        if selected_control.is_some() {
            break;
        }
        if let Some(cell) = cells.get(boundary) {
            let count = 1 + cell_count_in_cell(cell);
            if *index < count {
                selected_cell = Some(boundary);
                break;
            }
            *index -= count;
        }
    }
    if let Some(control_index) = selected_control {
        nth_cell_in_control(&mut content_controls[control_index].2, index)
    } else if let Some(cell_index) = selected_cell {
        take_cell(&mut cells[cell_index], index)
    } else {
        None
    }
}

fn take_cell<'a>(cell: &'a mut CT_Tc, index: &mut usize) -> Option<&'a mut CT_Tc> {
    if *index == 0 {
        Some(cell)
    } else {
        *index -= 1;
        for child in &mut cell.content {
            let nested = match child {
                CellContent::Table(table) => nth_cell_in_table(table, index),
                CellContent::ContentControl(control) => nth_cell_in_control(control, index),
                CellContent::Paragraph(_) => None,
            };
            if nested.is_some() {
                return nested;
            }
        }
        None
    }
}

// Traverse the parsed physical tree once, in document order. Coordinates
// count rows and cells as `TableRef::cell` does: the rows of row controls
// and the cells of cell controls among the table's own, so a coordinate is
// numbered when its node is visited.
enum PhysicalCellNode<'a> {
    Table(&'a CT_Tbl),
    /// A row of table `.1`; `.2` when the row is one of its rows.
    Row(&'a CT_Row, usize, bool),
    /// A cell of table `.1`; `.2` is its row's coordinate and counter.
    Cell(&'a CT_Tc, usize, Option<(usize, usize)>),
    /// A control of table `.1`: `.2` names what it holds rows or cells of.
    Control(&'a CT_Sdt, usize, ControlSlot),
}

#[derive(Clone, Copy)]
enum ControlSlot {
    /// Between rows: its rows are rows of the table.
    Rows,
    /// Between the cells of a row, by its coordinate and counter.
    Cells(usize, usize),
    /// In a cell or another block: it holds no row or cell of the table.
    Block,
}

#[allow(clippy::type_complexity)]
pub(super) fn physical_table_cells(
    table: &CT_Tbl,
) -> (usize, Vec<(usize, Option<(usize, usize)>)>) {
    let mut stack = vec![PhysicalCellNode::Table(table)];
    let mut tables = 0;
    let mut next_row = Vec::new();
    let mut next_cell = Vec::new();
    let mut cells = Vec::new();
    while let Some(node) = stack.pop() {
        let mut children = Vec::new();
        match node {
            PhysicalCellNode::Table(table) => {
                let index = tables;
                tables += 1;
                next_row.push(0usize);
                let mut controls = table.content_controls.iter().peekable();
                for boundary in 0..=table.rows.len() {
                    while controls.peek().is_some_and(|(at, _, _)| *at == boundary) {
                        children.push(PhysicalCellNode::Control(
                            &controls.next().unwrap().2,
                            index,
                            ControlSlot::Rows,
                        ));
                    }
                    if let Some(row) = table.rows.get(boundary) {
                        children.push(PhysicalCellNode::Row(row, index, true));
                    }
                }
            }
            PhysicalCellNode::Row(row, table, counted) => {
                let ordinal = counted.then(|| {
                    let ordinal = next_row[table];
                    next_row[table] += 1;
                    next_cell.push(0usize);
                    (ordinal, next_cell.len() - 1)
                });
                let mut controls = row.content_controls.iter().peekable();
                for boundary in 0..=row.cells.len() {
                    while controls.peek().is_some_and(|(at, _, _)| *at == boundary) {
                        children.push(PhysicalCellNode::Control(
                            &controls.next().unwrap().2,
                            table,
                            ordinal.map_or(ControlSlot::Block, |(row, counter)| {
                                ControlSlot::Cells(row, counter)
                            }),
                        ));
                    }
                    if let Some(cell) = row.cells.get(boundary) {
                        children.push(PhysicalCellNode::Cell(cell, table, ordinal));
                    }
                }
            }
            PhysicalCellNode::Cell(cell, table, ordinal) => {
                let coordinate = ordinal.map(|(row, counter)| {
                    let index = next_cell[counter];
                    next_cell[counter] += 1;
                    (row, index)
                });
                cells.push((table, coordinate));
                for child in &cell.content {
                    match child {
                        CellContent::Table(table) => children.push(PhysicalCellNode::Table(table)),
                        CellContent::ContentControl(control) => children.push(
                            PhysicalCellNode::Control(control, table, ControlSlot::Block),
                        ),
                        CellContent::Paragraph(_) => {}
                    }
                }
            }
            PhysicalCellNode::Control(control, table, slot) => {
                for child in &control.content {
                    match child {
                        SdtContent::Table(table) => children.push(PhysicalCellNode::Table(table)),
                        SdtContent::Row(row) => children.push(PhysicalCellNode::Row(
                            row,
                            table,
                            matches!(slot, ControlSlot::Rows),
                        )),
                        SdtContent::Cell(cell) => children.push(PhysicalCellNode::Cell(
                            cell,
                            table,
                            match slot {
                                ControlSlot::Cells(row, counter) => Some((row, counter)),
                                _ => None,
                            },
                        )),
                        SdtContent::ContentControl(control) => {
                            children.push(PhysicalCellNode::Control(control, table, slot))
                        }
                        _ => {}
                    }
                }
            }
        }
        stack.extend(children.into_iter().rev());
    }
    (tables, cells)
}

fn cell_count_in_control(control: &CT_Sdt) -> usize {
    control
        .content
        .iter()
        .map(|child| match child {
            SdtContent::Table(table) => cell_count_in_table(table),
            SdtContent::Row(row) => cell_count_in_row(row),
            SdtContent::Cell(cell) => 1 + cell_count_in_cell(cell),
            SdtContent::ContentControl(control) => cell_count_in_control(control),
            SdtContent::Paragraph(_) | SdtContent::Run(_) | SdtContent::RawXml(_) => 0,
        })
        .sum()
}

fn cell_count_in_table(table: &CT_Tbl) -> usize {
    table
        .content_controls
        .iter()
        .map(|(_, _, control)| cell_count_in_control(control))
        .sum::<usize>()
        + table.rows.iter().map(cell_count_in_row).sum::<usize>()
}

fn cell_count_in_row(row: &CT_Row) -> usize {
    row.content_controls
        .iter()
        .map(|(_, _, control)| cell_count_in_control(control))
        .sum::<usize>()
        + row
            .cells
            .iter()
            .map(|cell| 1 + cell_count_in_cell(cell))
            .sum::<usize>()
}

fn cell_count_in_cell(cell: &CT_Tc) -> usize {
    cell.content
        .iter()
        .map(|child| match child {
            CellContent::Table(table) => cell_count_in_table(table),
            CellContent::ContentControl(control) => cell_count_in_control(control),
            CellContent::Paragraph(_) => 0,
        })
        .sum()
}

pub(super) fn insert_html_content_into_cell(
    cell: &mut CT_Tc,
    direct_index: usize,
    content: Vec<BodyContent>,
) -> Result<()> {
    let inserted = content
        .into_iter()
        .map(|item| match item {
            BodyContent::Paragraph(paragraph) => Ok(CellContent::Paragraph(*paragraph)),
            BodyContent::Table(table) => Ok(CellContent::Table(*table)),
            BodyContent::ContentControl(control) => Ok(CellContent::ContentControl(*control)),
            BodyContent::RawXml(_) => Err(Error::Other(
                "HTML projection produced unsupported raw cell content".to_owned(),
            )),
        })
        .collect::<Result<Vec<_>>>()?;
    let inserted_len = inserted.len();
    let mut virtual_index = 0_usize;
    let mut insertion = None;
    for content_index in 0..=cell.content.len() {
        for (raw_index, (position, _)) in cell.extra_xml.iter().enumerate() {
            if *position != content_index {
                continue;
            }
            if virtual_index == direct_index {
                insertion = Some((content_index, raw_index));
                break;
            }
            virtual_index += 1;
        }
        if insertion.is_some() {
            break;
        }
        if content_index < cell.content.len() {
            if virtual_index == direct_index {
                let raw_index = cell
                    .extra_xml
                    .iter()
                    .position(|(position, _)| *position > content_index)
                    .unwrap_or(cell.extra_xml.len());
                insertion = Some((content_index, raw_index));
                break;
            }
            virtual_index += 1;
        }
    }
    if virtual_index == direct_index && insertion.is_none() {
        insertion = Some((cell.content.len(), cell.extra_xml.len()));
    }
    let Some((content_index, raw_shift_start)) = insertion else {
        return Err(StoryError::OutOfBounds {
            index: direct_index,
            len: virtual_index,
        }
        .into());
    };
    for (position, _) in cell.extra_xml.iter_mut().skip(raw_shift_start) {
        *position = position
            .checked_add(inserted_len)
            .ok_or_else(|| Error::Other("cell raw-content position overflowed".to_owned()))?;
    }
    cell.content.splice(content_index..content_index, inserted);
    Ok(())
}

pub(crate) fn visit_body_paragraphs<'a>(
    content: &'a [BodyContent],
    accepted: bool,
    visitor: &mut impl FnMut(&'a CT_P),
) {
    for item in content {
        match item {
            BodyContent::Paragraph(paragraph) => visit_paragraph(paragraph, accepted, visitor),
            BodyContent::Table(table) => visit_table(table, accepted, visitor),
            BodyContent::ContentControl(control) => visit_sdt(control, accepted, visitor),
            BodyContent::RawXml(_) => {}
        }
    }
}

// With `accepted`, the visitors leave out the rows that accepting every
// tracked change removes, as `CT_Row::accepted_view_removes` reports them.
// `visit_row` itself visits the row it is given.
pub(crate) fn visit_paragraph<'a>(
    paragraph: &'a CT_P,
    accepted: bool,
    visitor: &mut impl FnMut(&'a CT_P),
) {
    visitor(paragraph);
    for (_, _, _, control) in &paragraph.content_controls {
        visit_sdt(control, accepted, visitor);
    }
}

fn visit_table<'a>(table: &'a CT_Tbl, accepted: bool, visitor: &mut impl FnMut(&'a CT_P)) {
    for index in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == index)
        {
            visit_sdt(control, accepted, visitor);
        }
        if let Some(row) = table.rows.get(index)
            && !(accepted && row.accepted_view_removes())
        {
            visit_row(row, accepted, visitor);
        }
    }
}

fn visit_row<'a>(row: &'a CT_Row, accepted: bool, visitor: &mut impl FnMut(&'a CT_P)) {
    for index in 0..=row.cells.len() {
        for (_, _, control) in row
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == index)
        {
            visit_sdt(control, accepted, visitor);
        }
        if let Some(cell) = row.cells.get(index) {
            visit_cell(cell, accepted, visitor);
        }
    }
}

pub(super) fn visit_cell<'a>(cell: &'a CT_Tc, accepted: bool, visitor: &mut impl FnMut(&'a CT_P)) {
    for item in &cell.content {
        match item {
            CellContent::Paragraph(paragraph) => visit_paragraph(paragraph, accepted, visitor),
            CellContent::Table(table) => visit_table(table, accepted, visitor),
            CellContent::ContentControl(control) => visit_sdt(control, accepted, visitor),
        }
    }
}

fn visit_sdt<'a>(control: &'a CT_Sdt, accepted: bool, visitor: &mut impl FnMut(&'a CT_P)) {
    for item in &control.content {
        match item {
            SdtContent::Paragraph(paragraph) => visit_paragraph(paragraph, accepted, visitor),
            SdtContent::Table(table) => visit_table(table, accepted, visitor),
            SdtContent::Row(row) if accepted && row.accepted_view_removes() => {}
            SdtContent::Row(row) => visit_row(row, accepted, visitor),
            SdtContent::Cell(cell) => visit_cell(cell, accepted, visitor),
            SdtContent::Run(_) | SdtContent::RawXml(_) => {}
            SdtContent::ContentControl(nested) => visit_sdt(nested, accepted, visitor),
        }
    }
}

/// Collect the body paragraphs in document order, those that body-level
/// content controls wrap included. Tables are not entered, with or without a
/// control around them. This is what [`Document::headings`] and
/// [`Document::links`] read.
pub(super) fn block_paragraphs(content: &[BodyContent]) -> Vec<&CT_P> {
    let mut paragraphs = Vec::new();
    for item in content {
        match item {
            BodyContent::Paragraph(paragraph) => paragraphs.push(paragraph.as_ref()),
            BodyContent::ContentControl(control) => {
                collect_block_control_paragraphs(control, &mut paragraphs);
            }
            BodyContent::Table(_) | BodyContent::RawXml(_) => {}
        }
    }
    paragraphs
}

fn collect_block_control_paragraphs<'a>(control: &'a CT_Sdt, paragraphs: &mut Vec<&'a CT_P>) {
    for item in &control.content {
        match item {
            SdtContent::Paragraph(paragraph) => paragraphs.push(paragraph),
            SdtContent::ContentControl(nested) => {
                collect_block_control_paragraphs(nested, paragraphs);
            }
            SdtContent::Table(_)
            | SdtContent::Row(_)
            | SdtContent::Cell(_)
            | SdtContent::Run(_)
            | SdtContent::RawXml(_) => {}
        }
    }
}

/// Append a block-level paragraph to [`Document::text`] as one line.
pub(super) fn push_paragraph_line(paragraph: &CT_P, text: &mut String) {
    text.push_str(&ParagraphRef { inner: paragraph }.text());
    text.push('\n');
}

/// Append a table to [`Document::text`] as one line per row, rows wrapped by
/// table-level content controls included.
pub(super) fn push_table_text(table: &CT_Tbl, text: &mut String) {
    for index in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == index)
        {
            push_control_text(control, text);
        }
        if let Some(row) = table.rows.get(index) {
            push_row_text(row, text);
        }
    }
}

/// Append a row to [`Document::text`] as one line in which every paragraph of
/// its cells ends with a tab, cell controls and nested tables included.
fn push_row_text(row: &CT_Row, text: &mut String) {
    if row.accepted_view_removes() {
        return;
    }
    visit_row(row, true, &mut |paragraph| {
        push_cell_paragraph(paragraph, text)
    });
    text.push('\n');
}

fn push_cell_paragraph(paragraph: &CT_P, text: &mut String) {
    text.push_str(&ParagraphRef { inner: paragraph }.text());
    text.push('\t');
}

/// Append what a block, table or row content control wraps to [`Document::text`].
pub(super) fn push_control_text(control: &CT_Sdt, text: &mut String) {
    for item in &control.content {
        match item {
            SdtContent::Paragraph(paragraph) => push_paragraph_line(paragraph, text),
            SdtContent::Table(table) => push_table_text(table, text),
            SdtContent::Row(row) => push_row_text(row, text),
            SdtContent::Cell(cell) => {
                visit_cell(cell, true, &mut |paragraph| {
                    push_cell_paragraph(paragraph, text)
                });
            }
            SdtContent::Run(_) | SdtContent::RawXml(_) => {}
            SdtContent::ContentControl(nested) => push_control_text(nested, text),
        }
    }
}

pub(crate) fn visit_body_paragraphs_mut(
    content: &mut [BodyContent],
    visitor: &mut impl FnMut(&mut CT_P),
) {
    for item in content {
        match item {
            BodyContent::Paragraph(paragraph) => visit_paragraph_mut(paragraph, visitor),
            BodyContent::Table(table) => visit_table_mut(table, visitor),
            BodyContent::ContentControl(control) => visit_sdt_mut(control, visitor),
            BodyContent::RawXml(_) => {}
        }
    }
}

/// Visit the paragraphs of the body's table cells in document order, nested
/// tables, tables inside block controls and controls around rows or cells
/// included, as [`visit_body_paragraphs_mut`] reaches them.
pub(crate) fn visit_body_table_paragraphs_mut(
    content: &mut [BodyContent],
    visitor: &mut impl FnMut(&mut CT_P),
) {
    for item in content {
        match item {
            BodyContent::Table(table) => visit_table_mut(table, visitor),
            BodyContent::ContentControl(control) => visit_sdt_tables_mut(control, visitor),
            BodyContent::Paragraph(_) | BodyContent::RawXml(_) => {}
        }
    }
}

fn visit_sdt_tables_mut(control: &mut CT_Sdt, visitor: &mut impl FnMut(&mut CT_P)) {
    for item in &mut control.content {
        match item {
            SdtContent::Table(table) => visit_table_mut(table, visitor),
            SdtContent::Row(row) => visit_row_mut(row, visitor),
            SdtContent::Cell(cell) => visit_cell_mut(cell, visitor),
            SdtContent::ContentControl(nested) => visit_sdt_tables_mut(nested, visitor),
            SdtContent::Paragraph(_) | SdtContent::Run(_) | SdtContent::RawXml(_) => {}
        }
    }
}

fn visit_paragraph_mut(paragraph: &mut CT_P, visitor: &mut impl FnMut(&mut CT_P)) {
    visitor(paragraph);
    for (_, _, _, control) in &mut paragraph.content_controls {
        visit_sdt_mut(control, visitor);
    }
}

fn visit_table_mut(table: &mut CT_Tbl, visitor: &mut impl FnMut(&mut CT_P)) {
    for index in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter_mut()
            .filter(|(at, _, _)| *at == index)
        {
            visit_sdt_mut(control, visitor);
        }
        if let Some(row) = table.rows.get_mut(index) {
            visit_row_mut(row, visitor);
        }
    }
}

fn visit_row_mut(row: &mut CT_Row, visitor: &mut impl FnMut(&mut CT_P)) {
    for index in 0..=row.cells.len() {
        for (_, _, control) in row
            .content_controls
            .iter_mut()
            .filter(|(at, _, _)| *at == index)
        {
            visit_sdt_mut(control, visitor);
        }
        if let Some(cell) = row.cells.get_mut(index) {
            visit_cell_mut(cell, visitor);
        }
    }
}

fn visit_cell_mut(cell: &mut CT_Tc, visitor: &mut impl FnMut(&mut CT_P)) {
    for item in &mut cell.content {
        match item {
            CellContent::Paragraph(paragraph) => visit_paragraph_mut(paragraph, visitor),
            CellContent::Table(table) => visit_table_mut(table, visitor),
            CellContent::ContentControl(control) => visit_sdt_mut(control, visitor),
        }
    }
}

fn visit_sdt_mut(control: &mut CT_Sdt, visitor: &mut impl FnMut(&mut CT_P)) {
    for item in &mut control.content {
        match item {
            SdtContent::Paragraph(paragraph) => visit_paragraph_mut(paragraph, visitor),
            SdtContent::Table(table) => visit_table_mut(table, visitor),
            SdtContent::Row(row) => visit_row_mut(row, visitor),
            SdtContent::Cell(cell) => visit_cell_mut(cell, visitor),
            SdtContent::Run(_) | SdtContent::RawXml(_) => {}
            SdtContent::ContentControl(nested) => visit_sdt_mut(nested, visitor),
        }
    }
}

pub(super) fn collect_relationship_ids(content: &[BodyContent], output: &mut Vec<String>) {
    for item in content {
        match item {
            BodyContent::Paragraph(paragraph) => {
                collect_paragraph_relationship_ids(paragraph, output)
            }
            BodyContent::Table(table) => collect_table_relationship_ids(table, output),
            BodyContent::ContentControl(control) => collect_sdt_relationship_ids(control, output),
            BodyContent::RawXml(_) => {}
        }
    }
}

fn collect_paragraph_relationship_ids(paragraph: &CT_P, output: &mut Vec<String>) {
    if let Some(section) = paragraph
        .properties
        .as_ref()
        .and_then(|properties| properties.sect_pr.as_ref())
    {
        for reference in section.header_refs.iter().chain(&section.footer_refs) {
            if !output.contains(&reference.rel_id) {
                output.push(reference.rel_id.clone());
            }
        }
    }
    for index in 0..=paragraph.runs.len() {
        for (_, _, _, control) in paragraph
            .content_controls
            .iter()
            .filter(|(at, _, _, _)| *at == index)
        {
            collect_sdt_relationship_ids(control, output);
        }
        for hyperlink in paragraph
            .hyperlinks
            .iter()
            .filter(|hyperlink| hyperlink.run_start == index)
        {
            if let Some(id) = &hyperlink.rel_id
                && !output.contains(id)
            {
                output.push(id.clone());
            }
        }
        let Some(run) = paragraph.runs.get(index) else {
            continue;
        };
        for content in &run.content {
            let RunContent::Drawing(drawing) = content else {
                continue;
            };
            let id = drawing
                .inline
                .as_ref()
                .filter(|drawing| drawing.raw_xml.is_none())
                .map(|drawing| drawing.chart_rel_id.as_ref().unwrap_or(&drawing.embed_id))
                .or_else(|| {
                    drawing
                        .anchor
                        .as_ref()
                        .filter(|drawing| drawing.raw_xml.is_none())
                        .map(|drawing| drawing.chart_rel_id.as_ref().unwrap_or(&drawing.embed_id))
                });
            if let Some(id) = id
                && !output.contains(id)
            {
                output.push(id.clone());
            }
        }
    }
}

fn collect_table_relationship_ids(table: &CT_Tbl, output: &mut Vec<String>) {
    for index in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == index)
        {
            collect_sdt_relationship_ids(control, output);
        }
        if let Some(row) = table.rows.get(index) {
            for cell_index in 0..=row.cells.len() {
                for (_, _, control) in row
                    .content_controls
                    .iter()
                    .filter(|(at, _, _)| *at == cell_index)
                {
                    collect_sdt_relationship_ids(control, output);
                }
                if let Some(cell) = row.cells.get(cell_index) {
                    for item in &cell.content {
                        match item {
                            CellContent::Paragraph(paragraph) => {
                                collect_paragraph_relationship_ids(paragraph, output)
                            }
                            CellContent::Table(table) => {
                                collect_table_relationship_ids(table, output)
                            }
                            CellContent::ContentControl(control) => {
                                collect_sdt_relationship_ids(control, output)
                            }
                        }
                    }
                }
            }
        }
    }
}

fn collect_sdt_relationship_ids(control: &CT_Sdt, output: &mut Vec<String>) {
    for item in &control.content {
        match item {
            SdtContent::Paragraph(paragraph) => {
                collect_paragraph_relationship_ids(paragraph, output)
            }
            SdtContent::Table(table) => collect_table_relationship_ids(table, output),
            SdtContent::Row(row) => {
                let table = CT_Tbl {
                    properties: None,
                    grid: None,
                    rows: vec![row.clone()],
                    extra_xml: Vec::new(),
                    content_controls: Vec::new(),
                };
                collect_table_relationship_ids(&table, output);
            }
            SdtContent::Cell(cell) => {
                for item in &cell.content {
                    match item {
                        CellContent::Paragraph(paragraph) => {
                            collect_paragraph_relationship_ids(paragraph, output)
                        }
                        CellContent::Table(table) => collect_table_relationship_ids(table, output),
                        CellContent::ContentControl(control) => {
                            collect_sdt_relationship_ids(control, output)
                        }
                    }
                }
            }
            SdtContent::ContentControl(nested) => collect_sdt_relationship_ids(nested, output),
            SdtContent::Run(run) => {
                for content in &run.content {
                    if let RunContent::Drawing(drawing) = content
                        && let Some(id) = drawing
                            .inline
                            .as_ref()
                            .filter(|drawing| drawing.raw_xml.is_none())
                            .map(|drawing| {
                                drawing.chart_rel_id.as_ref().unwrap_or(&drawing.embed_id)
                            })
                            .or_else(|| {
                                drawing
                                    .anchor
                                    .as_ref()
                                    .filter(|drawing| drawing.raw_xml.is_none())
                                    .map(|drawing| {
                                        drawing.chart_rel_id.as_ref().unwrap_or(&drawing.embed_id)
                                    })
                            })
                        && !output.contains(id)
                    {
                        output.push(id.clone());
                    }
                }
            }
            SdtContent::RawXml(_) => {}
        }
    }
}

/// A body child as the ODT and RTF writers export it.
pub(crate) enum ExportItem<'a> {
    Paragraph(&'a CT_P),
    /// A table, with the rows, cells and cell content that content controls
    /// wrap in place, see [`unwrap_table_controls`].
    Table(Box<Cow<'a, CT_Tbl>>),
    /// A block content control. What it wraps follows it.
    ContentControl,
    /// A bookmark start or end between blocks.
    BookmarkMarker,
    RawXml,
}

/// The body children in document order with their source paths, `body[i]`.
/// A block content control is transparent: what it wraps follows it at
/// `{its path}/content[k]`, nested controls included.
pub(crate) fn body_export_items(content: &[BodyContent]) -> Vec<(ExportItem<'_>, String)> {
    let mut items = Vec::new();
    for (index, item) in content.iter().enumerate() {
        let path = format!("body[{index}]");
        match item {
            BodyContent::Paragraph(paragraph) => {
                items.push((ExportItem::Paragraph(paragraph), path))
            }
            BodyContent::Table(table) => items.push((
                ExportItem::Table(Box::new(unwrap_table_controls(table))),
                path,
            )),
            BodyContent::ContentControl(control) => {
                push_control_export_items(control, path, &mut items)
            }
            BodyContent::RawXml(raw) => items.push((raw_export_item(raw), path)),
        }
    }
    items
}

fn raw_export_item<'a>(raw: &[u8]) -> ExportItem<'a> {
    if super::is_block_bookmark_marker(raw) {
        ExportItem::BookmarkMarker
    } else {
        ExportItem::RawXml
    }
}

fn push_control_export_items<'a>(
    control: &'a CT_Sdt,
    path: String,
    items: &mut Vec<(ExportItem<'a>, String)>,
) {
    items.push((ExportItem::ContentControl, path.clone()));
    for (index, item) in control.content.iter().enumerate() {
        let path = format!("{path}/content[{index}]");
        match item {
            SdtContent::Paragraph(paragraph) => {
                items.push((ExportItem::Paragraph(paragraph), path))
            }
            SdtContent::Table(table) => items.push((
                ExportItem::Table(Box::new(unwrap_table_controls(table))),
                path,
            )),
            SdtContent::ContentControl(nested) => push_control_export_items(nested, path, items),
            // A control around a paragraph or a table holds no rows, cells or
            // runs of its own.
            SdtContent::Row(_) | SdtContent::Cell(_) | SdtContent::Run(_) => {}
            SdtContent::RawXml(raw) => items.push((raw_export_item(raw), path)),
        }
    }
}

/// Whether a table, not counting the tables nested in its cells, holds a
/// content control around a row, a cell or cell content.
fn table_has_content_controls(table: &CT_Tbl) -> bool {
    !table.content_controls.is_empty()
        || table.rows.iter().any(|row| {
            !row.content_controls.is_empty()
                || row.cells.iter().any(|cell| {
                    cell.content
                        .iter()
                        .any(|content| matches!(content, CellContent::ContentControl(_)))
                })
        })
}

/// The table with the rows, cells and cell content that its content controls
/// wrap in place of the controls, nested ones included. Borrowed when the
/// table has no such control.
pub(crate) fn unwrap_table_controls(table: &CT_Tbl) -> Cow<'_, CT_Tbl> {
    if !table_has_content_controls(table) {
        return Cow::Borrowed(table);
    }
    let rows = table
        .rows()
        .into_iter()
        .map(|row| CT_Row {
            table_property_exception: row.table_property_exception.clone(),
            properties: row.properties.clone(),
            cells: row
                .cells()
                .into_iter()
                .map(|cell| {
                    let mut content = Vec::with_capacity(cell.content.len());
                    let mut extra_xml = cell.extra_xml.clone();
                    for item in &cell.content {
                        match item {
                            CellContent::ContentControl(control) => {
                                push_control_cell_content(control, &mut content, &mut extra_xml)
                            }
                            item => content.push(item.clone()),
                        }
                    }
                    CT_Tc {
                        properties: cell.properties.clone(),
                        content,
                        extra_xml,
                    }
                })
                .collect(),
            extra_xml: row.extra_xml.clone(),
            content_controls: Vec::new(),
        })
        .collect();
    Cow::Owned(CT_Tbl {
        properties: table.properties.clone(),
        grid: table.grid.clone(),
        rows,
        extra_xml: table.extra_xml.clone(),
        content_controls: Vec::new(),
    })
}

/// The content a cell's control wraps, in its place; the control's raw
/// children go with the cell's, so the exports still report them.
fn push_control_cell_content(
    control: &CT_Sdt,
    content: &mut Vec<CellContent>,
    extra_xml: &mut Vec<(usize, Vec<u8>)>,
) {
    for item in &control.content {
        match item {
            SdtContent::Paragraph(paragraph) => {
                content.push(CellContent::Paragraph(paragraph.clone()))
            }
            SdtContent::Table(table) => content.push(CellContent::Table(table.clone())),
            SdtContent::ContentControl(nested) => {
                push_control_cell_content(nested, content, extra_xml)
            }
            SdtContent::RawXml(raw) => extra_xml.push((content.len(), raw.clone())),
            SdtContent::Row(_) | SdtContent::Cell(_) | SdtContent::Run(_) => {}
        }
    }
}

/// Visit the drawings the exporters write, in the order they write them: those
/// of the accepted view of every paragraph (see `CT_P::accepted_view`), in
/// the paragraphs [`visit_body_paragraphs`] reaches.
pub(crate) fn visit_accepted_drawings(
    content: &[BodyContent],
    visitor: &mut impl FnMut(&CT_Drawing),
) {
    visit_body_paragraphs(content, true, &mut |paragraph| {
        for run in &paragraph.accepted_view().runs {
            visit_run_drawings(run, visitor);
        }
    });
}

pub(crate) fn visit_all_drawings(content: &[BodyContent], visitor: &mut impl FnMut(&CT_Drawing)) {
    for item in content {
        match item {
            BodyContent::Paragraph(paragraph) => visit_paragraph_drawings(paragraph, visitor),
            BodyContent::Table(table) => visit_table_drawings(table, visitor),
            BodyContent::ContentControl(control) => visit_sdt_drawings(control, visitor),
            BodyContent::RawXml(_) => {}
        }
    }
}

pub(crate) fn visit_all_drawings_mut(
    content: &mut [BodyContent],
    visitor: &mut impl FnMut(&mut CT_Drawing),
) {
    for item in content {
        match item {
            BodyContent::Paragraph(paragraph) => visit_paragraph_drawings_mut(paragraph, visitor),
            BodyContent::Table(table) => visit_table_drawings_mut(table, visitor),
            BodyContent::ContentControl(control) => visit_sdt_drawings_mut(control, visitor),
            BodyContent::RawXml(_) => {}
        }
    }
}

pub(super) fn visit_authored_drawings_mut(
    content: &mut [BodyContent],
    visitor: &mut impl FnMut(&mut CT_Drawing),
) {
    visit_all_drawings_mut(content, &mut |drawing| {
        if authored_drawing(drawing) {
            visitor(drawing);
        }
    });
}

pub(super) fn close_typed_story_drawing_namespaces(paragraph: &mut CT_P) -> Result<()> {
    let scope = BTreeMap::from([
        ("r".to_owned(), drawing_ns::R.to_owned()),
        ("wp".to_owned(), drawing_ns::WP.to_owned()),
    ]);
    let mut error = None;
    visit_paragraph_drawings_mut(paragraph, &mut |drawing| {
        if error.is_some() || !authored_drawing(drawing) {
            return;
        }
        if let Some(inline) = &mut drawing.inline {
            let mut writer = Writer::new(Vec::new());
            if let Err(current) = inline.to_xml(&mut writer).map_err(Error::from) {
                error = Some(current);
                return;
            }
            match close_content_fragment_namespaces(&writer.into_inner(), &scope) {
                Ok(xml) => inline.raw_xml = Some(xml),
                Err(current) => error = Some(current),
            }
        } else if let Some(anchor) = &mut drawing.anchor {
            let mut writer = Writer::new(Vec::new());
            if let Err(current) = anchor.to_xml(&mut writer).map_err(Error::from) {
                error = Some(current);
                return;
            }
            match close_content_fragment_namespaces(&writer.into_inner(), &scope) {
                Ok(xml) => anchor.raw_xml = Some(xml),
                Err(current) => error = Some(current),
            }
        }
    });
    error.map_or(Ok(()), Err)
}

fn authored_drawing(drawing: &CT_Drawing) -> bool {
    drawing
        .inline
        .as_ref()
        .is_some_and(|value| value.raw_xml.is_none())
        || drawing
            .anchor
            .as_ref()
            .is_some_and(|value| value.raw_xml.is_none())
}

fn visit_run_drawings(run: &CT_R, visitor: &mut impl FnMut(&CT_Drawing)) {
    for content in &run.content {
        if let RunContent::Drawing(drawing) = content {
            visitor(drawing);
        }
    }
}

fn visit_run_drawings_mut(run: &mut CT_R, visitor: &mut impl FnMut(&mut CT_Drawing)) {
    for content in &mut run.content {
        if let RunContent::Drawing(drawing) = content {
            visitor(drawing);
        }
    }
}

fn visit_paragraph_drawings(paragraph: &CT_P, visitor: &mut impl FnMut(&CT_Drawing)) {
    for index in 0..=paragraph.runs.len() {
        for (_, _, _, control) in paragraph
            .content_controls
            .iter()
            .filter(|(at, _, _, _)| *at == index)
        {
            visit_sdt_drawings(control, visitor);
        }
        if let Some(run) = paragraph.runs.get(index) {
            visit_run_drawings(run, visitor);
        }
    }
}

fn visit_paragraph_drawings_mut(paragraph: &mut CT_P, visitor: &mut impl FnMut(&mut CT_Drawing)) {
    for index in 0..=paragraph.runs.len() {
        for (_, _, _, control) in paragraph
            .content_controls
            .iter_mut()
            .filter(|(at, _, _, _)| *at == index)
        {
            visit_sdt_drawings_mut(control, visitor);
        }
        if let Some(run) = paragraph.runs.get_mut(index) {
            visit_run_drawings_mut(run, visitor);
        }
    }
}

fn visit_table_drawings(table: &CT_Tbl, visitor: &mut impl FnMut(&CT_Drawing)) {
    for index in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == index)
        {
            visit_sdt_drawings(control, visitor);
        }
        if let Some(row) = table.rows.get(index) {
            visit_row_drawings(row, visitor);
        }
    }
}

fn visit_table_drawings_mut(table: &mut CT_Tbl, visitor: &mut impl FnMut(&mut CT_Drawing)) {
    for index in 0..=table.rows.len() {
        for (_, _, control) in table
            .content_controls
            .iter_mut()
            .filter(|(at, _, _)| *at == index)
        {
            visit_sdt_drawings_mut(control, visitor);
        }
        if let Some(row) = table.rows.get_mut(index) {
            visit_row_drawings_mut(row, visitor);
        }
    }
}

fn visit_row_drawings(row: &CT_Row, visitor: &mut impl FnMut(&CT_Drawing)) {
    for index in 0..=row.cells.len() {
        for (_, _, control) in row
            .content_controls
            .iter()
            .filter(|(at, _, _)| *at == index)
        {
            visit_sdt_drawings(control, visitor);
        }
        if let Some(cell) = row.cells.get(index) {
            visit_cell_drawings(cell, visitor);
        }
    }
}

fn visit_row_drawings_mut(row: &mut CT_Row, visitor: &mut impl FnMut(&mut CT_Drawing)) {
    for index in 0..=row.cells.len() {
        for (_, _, control) in row
            .content_controls
            .iter_mut()
            .filter(|(at, _, _)| *at == index)
        {
            visit_sdt_drawings_mut(control, visitor);
        }
        if let Some(cell) = row.cells.get_mut(index) {
            visit_cell_drawings_mut(cell, visitor);
        }
    }
}

fn visit_cell_drawings(cell: &CT_Tc, visitor: &mut impl FnMut(&CT_Drawing)) {
    for item in &cell.content {
        match item {
            CellContent::Paragraph(paragraph) => visit_paragraph_drawings(paragraph, visitor),
            CellContent::Table(table) => visit_table_drawings(table, visitor),
            CellContent::ContentControl(control) => visit_sdt_drawings(control, visitor),
        }
    }
}

fn visit_cell_drawings_mut(cell: &mut CT_Tc, visitor: &mut impl FnMut(&mut CT_Drawing)) {
    for item in &mut cell.content {
        match item {
            CellContent::Paragraph(paragraph) => visit_paragraph_drawings_mut(paragraph, visitor),
            CellContent::Table(table) => visit_table_drawings_mut(table, visitor),
            CellContent::ContentControl(control) => visit_sdt_drawings_mut(control, visitor),
        }
    }
}

fn visit_sdt_drawings(control: &CT_Sdt, visitor: &mut impl FnMut(&CT_Drawing)) {
    for item in &control.content {
        match item {
            SdtContent::Paragraph(paragraph) => visit_paragraph_drawings(paragraph, visitor),
            SdtContent::Table(table) => visit_table_drawings(table, visitor),
            SdtContent::Row(row) => visit_row_drawings(row, visitor),
            SdtContent::Cell(cell) => visit_cell_drawings(cell, visitor),
            SdtContent::Run(run) => visit_run_drawings(run, visitor),
            SdtContent::ContentControl(nested) => visit_sdt_drawings(nested, visitor),
            SdtContent::RawXml(_) => {}
        }
    }
}

fn visit_sdt_drawings_mut(control: &mut CT_Sdt, visitor: &mut impl FnMut(&mut CT_Drawing)) {
    for item in &mut control.content {
        match item {
            SdtContent::Paragraph(paragraph) => visit_paragraph_drawings_mut(paragraph, visitor),
            SdtContent::Table(table) => visit_table_drawings_mut(table, visitor),
            SdtContent::Row(row) => visit_row_drawings_mut(row, visitor),
            SdtContent::Cell(cell) => visit_cell_drawings_mut(cell, visitor),
            SdtContent::Run(run) => visit_run_drawings_mut(run, visitor),
            SdtContent::ContentControl(nested) => visit_sdt_drawings_mut(nested, visitor),
            SdtContent::RawXml(_) => {}
        }
    }
}
