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
        for (index, item) in items.into_iter().enumerate() {
            let BlockItem::Marker(start, id, name) = item else {
                continue;
            };
            let next = paragraphs.iter().find(|(at, _)| *at > index);
            let previous = paragraphs.iter().rev().find(|(at, _)| *at < index);
            let (paragraph, leading) = match (start, next, previous) {
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
    for (index, row) in table.rows.iter().enumerate() {
        for (_, raw) in table.extra_xml.iter().filter(|(at, _)| *at == index) {
            push_raw(raw, items);
        }
        for (cell_index, cell) in row.cells.iter().enumerate() {
            for (_, raw) in row.extra_xml.iter().filter(|(at, _)| *at == cell_index) {
                push_raw(raw, items);
            }
            push_cell(cell, items);
        }
        for (_, raw) in row
            .extra_xml
            .iter()
            .filter(|(at, _)| *at >= row.cells.len())
        {
            push_raw(raw, items);
        }
    }
    for (_, raw) in table
        .extra_xml
        .iter()
        .filter(|(at, _)| *at >= table.rows.len())
    {
        push_raw(raw, items);
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
            SdtContent::Row(row) => {
                for cell in &row.cells {
                    push_cell(cell, items);
                }
            }
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
                .then(|| String::from_utf8_lossy(&attribute.value).into_owned())
        })
    };
    items.push(BlockItem::Marker(
        start,
        attribute(b"id").and_then(|id| id.parse().ok()),
        attribute(b"name"),
    ));
}
