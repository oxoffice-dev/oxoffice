//! Bookmark markers written between blocks, read as the paragraph they wrap.

use std::collections::HashMap;

use super::*;
use crate::document::BodyContent;

/// Bookmark markers that sit between blocks, a direct child of the body, a
/// table cell or a block content control, as Google Docs writes its heading
/// bookmarks. Each is read as the paragraph it wraps reads it: a start at
/// the start of the next paragraph, an end at the end of the previous one
/// (the other side when there is no such paragraph).
///
/// Paragraphs are identified by address: build it from the blocks the
/// paragraphs are read from, and use it while they are borrowed.
#[doc(hidden)]
#[derive(Debug, Default, Clone)]
pub struct BlockBookmarks {
    paragraphs: HashMap<usize, (Vec<BookmarkMarker>, Vec<BookmarkMarker>)>,
}

enum BlockItem<'a> {
    Paragraph(&'a CT_P),
    Marker(bool, Option<i32>, Option<String>),
}

impl BlockBookmarks {
    /// The block markers of these blocks, tables and controls included.
    pub fn of_blocks(content: &[BodyContent]) -> Self {
        let mut output = Self::default();
        output.add_blocks(content);
        output
    }

    /// Add the block markers of another story's blocks.
    pub fn add_blocks(&mut self, content: &[BodyContent]) {
        let mut items = Vec::new();
        for block in content {
            match block {
                BodyContent::Paragraph(paragraph) => items.push(BlockItem::Paragraph(paragraph)),
                BodyContent::Table(table) => push_table(table, &mut items),
                BodyContent::ContentControl(control) => push_control(control, &mut items),
                BodyContent::RawXml(raw) => push_raw(raw, &mut items),
            }
        }
        self.paragraphs.extend(Self::attribute(items).paragraphs);
    }

    /// The names of the bookmark starts between blocks.
    pub fn start_names(&self) -> impl Iterator<Item = &str> {
        self.paragraphs
            .values()
            .flat_map(|(before, after)| before.iter().chain(after))
            .filter(|marker| marker.is_start())
            .filter_map(BookmarkMarker::name)
    }

    /// Whether no marker sits between blocks.
    pub fn is_empty(&self) -> bool {
        self.paragraphs.is_empty()
    }

    /// The block markers a paragraph takes: those before its own markers,
    /// and those after them.
    pub fn markers(&self, paragraph: &CT_P) -> (&[BookmarkMarker], &[BookmarkMarker]) {
        self.paragraphs
            .get(&std::ptr::from_ref(paragraph).addr())
            .map_or((&[], &[]), |(before, after)| {
                (before.as_slice(), after.as_slice())
            })
    }

    /// A paragraph's current bookmark projection with the block markers it
    /// takes, in document order.
    pub fn paragraph_projection(&self, paragraph: &CT_P) -> Vec<BookmarkMarker> {
        let (before, after) = self.markers(paragraph);
        let mut markers = before.to_vec();
        markers.extend(paragraph.current_bookmark_projection());
        markers.extend_from_slice(after);
        markers
    }

    /// Write the block markers the paragraphs of `source` take into the
    /// same paragraphs of `copy`, a clone of it, whose addresses the markers
    /// were not read for ([`CT_P::take_block_markers`]).
    pub fn carry_into_table_copy(&self, source: &CT_Tbl, copy: &mut CT_Tbl) -> bool {
        self.is_empty() || self.carry_table(source, copy)
    }

    fn carry_paragraph(&self, source: &CT_P, copy: &mut CT_P) -> bool {
        let (before, after) = self.markers(source);
        (before.is_empty() && after.is_empty()) || copy.take_block_markers(before, after)
    }

    fn carry_table(&self, source: &CT_Tbl, copy: &mut CT_Tbl) -> bool {
        source.rows.len() == copy.rows.len()
            && source.content_controls.len() == copy.content_controls.len()
            && source
                .rows
                .iter()
                .zip(&mut copy.rows)
                .all(|(source, copy)| self.carry_row(source, copy))
            && source
                .content_controls
                .iter()
                .zip(&mut copy.content_controls)
                .all(|((_, _, source), (_, _, copy))| self.carry_control(source, copy))
    }

    fn carry_row(&self, source: &CT_Row, copy: &mut CT_Row) -> bool {
        source.cells.len() == copy.cells.len()
            && source.content_controls.len() == copy.content_controls.len()
            && source
                .cells
                .iter()
                .zip(&mut copy.cells)
                .all(|(source, copy)| self.carry_cell(source, copy))
            && source
                .content_controls
                .iter()
                .zip(&mut copy.content_controls)
                .all(|((_, _, source), (_, _, copy))| self.carry_control(source, copy))
    }

    fn carry_cell(&self, source: &CT_Tc, copy: &mut CT_Tc) -> bool {
        source.content.len() == copy.content.len()
            && source
                .content
                .iter()
                .zip(&mut copy.content)
                .all(|pair| match pair {
                    (CellContent::Paragraph(source), CellContent::Paragraph(copy)) => {
                        self.carry_paragraph(source, copy)
                    }
                    (CellContent::Table(source), CellContent::Table(copy)) => {
                        self.carry_table(source, copy)
                    }
                    (CellContent::ContentControl(source), CellContent::ContentControl(copy)) => {
                        self.carry_control(source, copy)
                    }
                    _ => false,
                })
    }

    fn carry_control(&self, source: &CT_Sdt, copy: &mut CT_Sdt) -> bool {
        source.content.len() == copy.content.len()
            && source
                .content
                .iter()
                .zip(&mut copy.content)
                .all(|pair| match pair {
                    (SdtContent::Paragraph(source), SdtContent::Paragraph(copy)) => {
                        self.carry_paragraph(source, copy)
                    }
                    (SdtContent::Table(source), SdtContent::Table(copy)) => {
                        self.carry_table(source, copy)
                    }
                    (SdtContent::Cell(source), SdtContent::Cell(copy)) => {
                        self.carry_cell(source, copy)
                    }
                    (SdtContent::Row(source), SdtContent::Row(copy)) => {
                        self.carry_row(source, copy)
                    }
                    (SdtContent::ContentControl(source), SdtContent::ContentControl(copy)) => {
                        self.carry_control(source, copy)
                    }
                    (SdtContent::RawXml(_), SdtContent::RawXml(_))
                    | (SdtContent::Run(_), SdtContent::Run(_)) => true,
                    _ => false,
                })
    }

    fn attribute(items: Vec<BlockItem<'_>>) -> Self {
        let paragraphs = items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| match item {
                BlockItem::Paragraph(paragraph) => Some((index, *paragraph)),
                BlockItem::Marker(..) => None,
            })
            .collect::<Vec<_>>();
        let mut output = Self::default();
        // Starts still waiting for a paragraph: an end of one of them, with
        // no paragraph in between, is a collapsed bookmark at that start.
        let mut waiting = Vec::new();
        for (index, item) in items.into_iter().enumerate() {
            let BlockItem::Marker(start, id, name) = item else {
                waiting.clear();
                continue;
            };
            let after = paragraphs.partition_point(|(at, _)| *at < index);
            let next = paragraphs.get(after);
            let previous = after.checked_sub(1).and_then(|at| paragraphs.get(at));
            let collapsed = !start && id.is_some() && waiting.contains(&id);
            if start {
                waiting.push(id);
            }
            let (paragraph, leading) = match (start || collapsed, next, previous) {
                (true, Some((_, next)), _) | (false, Some((_, next)), None) => (*next, true),
                (_, _, Some((_, previous))) => (*previous, false),
                (_, None, None) => continue,
            };
            let entry = output
                .paragraphs
                .entry(std::ptr::from_ref(paragraph).addr())
                .or_default();
            if leading {
                entry
                    .0
                    .push(BookmarkMarker::between_blocks(start, id, name, 0, 0, 0));
            } else {
                let counts = current_paragraph_bookmark_projection(paragraph);
                entry.1.push(BookmarkMarker::between_blocks(
                    start,
                    id,
                    name,
                    paragraph.runs.len(),
                    counts.projected_run_count,
                    counts.tracked_run_count,
                ));
            }
        }
        output
    }
}

impl CT_P {
    /// Write the block markers this paragraph takes ([`BlockBookmarks::markers`])
    /// into the paragraph itself: those before at its first run boundary,
    /// those after at its last. For a copy whose address the markers
    /// were not read for, such as the paragraph the accepted view joins.
    #[doc(hidden)]
    pub fn take_block_markers(
        &mut self,
        before: &[BookmarkMarker],
        after: &[BookmarkMarker],
    ) -> bool {
        let saved = self.extra_xml.clone();
        let end = self.runs.len();
        let leading = before
            .iter()
            .map(|marker| canonical_block_marker(marker).map(|raw| (0, raw)))
            .collect::<Option<Vec<_>>>();
        let trailing = after
            .iter()
            .map(|marker| canonical_block_marker(marker).map(|raw| (end, raw)))
            .collect::<Option<Vec<_>>>();
        let (Some(leading), Some(trailing)) = (leading, trailing) else {
            return false;
        };
        // Appended, as `insert_bookmark_start` does: the owners placed by a
        // count of the raw children before them (revisions, equations,
        // controls, comment ranges) keep their slots.
        self.extra_xml.extend(leading);
        self.extra_xml.extend(trailing);
        if self.refresh_bookmark_projection() {
            true
        } else {
            self.extra_xml = saved;
            let _ = self.refresh_bookmark_projection();
            false
        }
    }
}

/// A block marker as a canonical `w:bookmarkStart` or `w:bookmarkEnd`.
fn canonical_block_marker(marker: &BookmarkMarker) -> Option<Vec<u8>> {
    let mut value = itoa::Buffer::new();
    let mut element = BytesStart::new(if marker.start {
        "w:bookmarkStart"
    } else {
        "w:bookmarkEnd"
    });
    if let Some(id) = marker.id {
        element.push_attribute(("w:id", value.format(id)));
    }
    if let Some(name) = marker.name.as_deref().filter(|_| marker.start) {
        element.push_attribute(("w:name", name));
    }
    let mut raw = Vec::new();
    Writer::new(&mut raw)
        .write_event(Event::Empty(element))
        .ok()?;
    Some(raw)
}

impl BookmarkMarker {
    fn between_blocks(
        start: bool,
        id: Option<i32>,
        name: Option<String>,
        run_index: usize,
        projected_run_index: usize,
        tracked_run_index: usize,
    ) -> Self {
        Self {
            start,
            id,
            name: if start { name } else { None },
            run_index,
            raw_before: 0,
            projected_run_index,
            tracked_run_index,
            word_prefixes: vec!["w".to_owned()],
            has_child_content: false,
        }
    }
}

fn push_table<'a>(table: &'a CT_Tbl, items: &mut Vec<BlockItem<'a>>) {
    for boundary in 0..=table.rows.len() {
        push_slot(&table.extra_xml, &table.content_controls, boundary, items);
        if let Some(row) = table.rows.get(boundary) {
            push_row(row, items);
        }
    }
}

/// A row's cells with the raw children and controls between them, in a
/// table or in a row-level content control alike.
fn push_row<'a>(row: &'a CT_Row, items: &mut Vec<BlockItem<'a>>) {
    for cell_boundary in 0..=row.cells.len() {
        push_slot(&row.extra_xml, &row.content_controls, cell_boundary, items);
        if let Some(cell) = row.cells.get(cell_boundary) {
            push_cell(cell, items);
        }
    }
}

/// The raw children and controls of a table or row before one boundary, in
/// order: a control after the raw children it counts before it.
fn push_slot<'a>(
    raw: &'a [(usize, Vec<u8>)],
    controls: &'a [(usize, usize, CT_Sdt)],
    boundary: usize,
    items: &mut Vec<BlockItem<'a>>,
) {
    let raws = raw
        .iter()
        .filter(|(at, _)| *at == boundary)
        .map(|(_, raw)| raw)
        .collect::<Vec<_>>();
    let mut controls = controls
        .iter()
        .filter(|(at, _, _)| *at == boundary)
        .peekable();
    for (index, raw) in raws.iter().enumerate() {
        while let Some((_, _, control)) = controls.next_if(|(_, before, _)| *before <= index) {
            push_control(control, items);
        }
        push_raw(raw, items);
    }
    for (_, _, control) in controls {
        push_control(control, items);
    }
}

fn push_cell<'a>(cell: &'a CT_Tc, items: &mut Vec<BlockItem<'a>>) {
    for (index, content) in cell.content.iter().enumerate() {
        for (_, raw) in cell.extra_xml.iter().filter(|(at, _)| *at == index) {
            push_raw(raw, items);
        }
        match content {
            CellContent::Paragraph(paragraph) => items.push(BlockItem::Paragraph(paragraph)),
            CellContent::Table(table) => push_table(table, items),
            CellContent::ContentControl(control) => push_control(control, items),
        }
    }
    for (_, raw) in cell
        .extra_xml
        .iter()
        .filter(|(at, _)| *at >= cell.content.len())
    {
        push_raw(raw, items);
    }
}

fn push_control<'a>(control: &'a CT_Sdt, items: &mut Vec<BlockItem<'a>>) {
    for content in &control.content {
        match content {
            SdtContent::Paragraph(paragraph) => items.push(BlockItem::Paragraph(paragraph)),
            SdtContent::Table(table) => push_table(table, items),
            SdtContent::Cell(cell) => push_cell(cell, items),
            SdtContent::Row(row) => push_row(row, items),
            SdtContent::ContentControl(control) => push_control(control, items),
            SdtContent::RawXml(raw) => push_raw(raw, items),
            SdtContent::Run(_) => {}
        }
    }
}

/// A raw block child that is a Word bookmark start or end.
fn push_raw(raw: &[u8], items: &mut Vec<BlockItem<'_>>) {
    let mut reader = Reader::from_reader(raw);
    let element = loop {
        match reader.read_event() {
            Ok(Event::Start(element) | Event::Empty(element)) => break element,
            Ok(Event::Text(text)) if is_xml_whitespace(text.as_ref()) => {}
            _ => return,
        }
    };
    let name = element.name();
    let start = match name.local_name().as_ref() {
        b"bookmarkStart" => true,
        b"bookmarkEnd" => false,
        _ => return,
    };
    // A block child inherits its prefix from the body; one bound in the raw
    // child itself must be Word's.
    let prefix = name.prefix().map(|prefix| prefix.as_ref().to_vec());
    let declared = element.attributes().flatten().find_map(|attribute| {
        let key = attribute.key.as_ref();
        let declares = match &prefix {
            Some(prefix) => key.strip_prefix(b"xmlns:") == Some(prefix.as_slice()),
            None => key == b"xmlns",
        };
        declares.then(|| attribute.value.into_owned())
    });
    if declared.is_some_and(|uri| uri != crate::namespace::W_NS.as_bytes()) {
        return;
    }
    let attribute = |local: &[u8]| {
        element.attributes().flatten().find_map(|attribute| {
            let key = attribute.key;
            (key.local_name().as_ref() == local
                && key.prefix().map(|p| p.as_ref().to_vec()) == prefix)
                .then(|| {
                    oxoffice_core::xml::attribute_text(&attribute)
                        .map(std::borrow::Cow::into_owned)
                        .ok()
                })
                .flatten()
        })
    };
    items.push(BlockItem::Marker(
        start,
        attribute(b"id").and_then(|id| id.parse().ok()),
        attribute(b"name"),
    ));
}
