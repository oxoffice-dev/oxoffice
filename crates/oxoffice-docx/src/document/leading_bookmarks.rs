//! Bookmark starts written between blocks belong to the block after them.
//!
//! Google Docs writes a heading's bookmark start between blocks, just before
//! the heading paragraph, with its end inside the heading. `rebuild_toc`
//! reads those starts as the heading's own, and so do the block operations:
//! an insertion before a block goes before the bookmark starts that lead it,
//! and a block that moves or goes takes the leading starts whose bookmark
//! ends inside it. A leading start whose bookmark ends further on stays where
//! it is, so no bookmark loses its start or ends before it starts.

use super::*;

/// Whether a resolved name is Word's. A body child kept as raw XML inherits
/// its prefix from the body (`inherited`), so a prefix it leaves undeclared
/// is Word's, as `BlockBookmarks` reads it.
fn is_word(namespace: &ResolveResult<'_>, inherited: bool) -> bool {
    word_element(namespace) || inherited && matches!(namespace, ResolveResult::Unknown(_))
}

/// The Word local name and `w:id` of the root of an XML element, closed or,
/// when `inherited`, a body child kept as raw XML.
fn word_root(xml: &[u8], inherited: bool) -> Result<Option<(Vec<u8>, Option<String>)>> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("block root scan failed: {error}")))?;
        match event {
            Event::Start(element) | Event::Empty(element) => {
                if !is_word(&namespace, inherited) {
                    return Ok(None);
                }
                let local = element.local_name().as_ref().to_vec();
                let id = word_id(&reader, &element, inherited)?;
                return Ok(Some((local, id)));
            }
            Event::Eof => return Ok(None),
            _ => buffer.clear(),
        }
    }
}

fn word_id(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
    inherited: bool,
) -> Result<Option<String>> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        if local.as_ref() == b"id" && is_word(&namespace, inherited) {
            return Ok(Some(
                attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                    .map_err(|error| Error::Other(error.to_string()))?
                    .trim()
                    .to_owned(),
            ));
        }
    }
    Ok(None)
}

/// The `w:id` of a Word bookmark start that is the whole element.
fn bookmark_start_id(xml: &[u8], inherited: bool) -> Result<Option<String>> {
    Ok(match word_root(xml, inherited)? {
        Some((local, id)) if local == b"bookmarkStart" => id,
        _ => None,
    })
}

/// The `w:id` values of the Word bookmark ends anywhere in a closed element.
fn bookmark_end_ids(xml: &[u8]) -> Result<HashSet<String>> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut ids = HashSet::new();
    loop {
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("bookmark end scan failed: {error}")))?;
        match event {
            Event::Start(element) | Event::Empty(element)
                if word_element(&namespace) && element.local_name().as_ref() == b"bookmarkEnd" =>
            {
                if let Some(id) = word_id(&reader, &element, false)? {
                    ids.insert(id);
                }
            }
            Event::Eof => return Ok(ids),
            _ => {}
        }
        buffer.clear();
    }
}

/// The leading bookmark starts of the block at `index` of `blocks`, as
/// `(index, w:id)` in document order: the bookmark starts written just
/// before it. None when the item at `index` is not a block (a paragraph, a
/// table or a block content control), so a start addressed itself, or the
/// end of a story, keeps its exact place.
fn leading_starts<T>(
    blocks: &[T],
    index: usize,
    is_block: impl Fn(&T) -> Result<bool>,
    start_id: impl Fn(&T) -> Result<Option<String>>,
) -> Result<Vec<(usize, String)>> {
    let mut starts = Vec::new();
    let Some(block) = blocks.get(index) else {
        return Ok(starts);
    };
    if !is_block(block)? {
        return Ok(starts);
    }
    let mut first = index;
    while first > 0
        && let Some(id) = start_id(&blocks[first - 1])?
    {
        first -= 1;
        starts.push((first, id));
    }
    starts.reverse();
    Ok(starts)
}

/// The closed XML of one direct story item.
fn item_xml(xml: &[u8], item: &StoryItemSpan) -> Result<Vec<u8>> {
    let scope = story_namespace_scope_at(xml, item.full.start)?;
    close_content_fragment_namespaces(&xml[item.full.clone()], &scope)
}

fn item_is_block(item: &StoryItemSpan) -> bool {
    matches!(
        item.kind,
        StoryItemKind::Paragraph | StoryItemKind::Table | StoryItemKind::ContentControl
    )
}

fn item_start_id(xml: &[u8], item: &StoryItemSpan) -> Result<Option<String>> {
    if item.kind != StoryItemKind::PreservedNode {
        return Ok(None);
    }
    bookmark_start_id(&item_xml(xml, item)?, false)
}

/// The direct index an insertion before direct item `index` of `items` uses:
/// before the bookmark starts that lead the block there.
pub(super) fn leading_bookmark_start_item(
    xml: &[u8],
    items: &[StoryItemSpan],
    index: usize,
) -> Result<usize> {
    let starts = leading_starts(
        items,
        index,
        |item| Ok(item_is_block(item)),
        |item| item_start_id(xml, item),
    )?;
    Ok(starts.first().map_or(index, |(first, _)| *first))
}

/// The leading bookmark starts of direct item `index` of `items` whose
/// bookmark ends inside one of the items `owners`, as direct indices.
pub(super) fn owned_leading_bookmark_start_items(
    xml: &[u8],
    items: &[StoryItemSpan],
    index: usize,
    owners: Range<usize>,
) -> Result<Vec<usize>> {
    let starts = leading_starts(
        items,
        index,
        |item| Ok(item_is_block(item)),
        |item| item_start_id(xml, item),
    )?;
    if starts.is_empty() {
        return Ok(Vec::new());
    }
    let mut ends = HashSet::new();
    for item in &items[owners] {
        ends.extend(bookmark_end_ids(&item_xml(xml, item)?)?);
    }
    Ok(starts
        .into_iter()
        .filter(|(_, id)| ends.contains(id))
        .map(|(index, _)| index)
        .collect())
}

fn body_is_block(content: &BodyContent) -> Result<bool> {
    Ok(match content {
        BodyContent::Paragraph(_) | BodyContent::Table(_) | BodyContent::ContentControl(_) => true,
        BodyContent::RawXml(raw) => {
            matches!(word_root(raw, true)?, Some((local, _)) if local == b"sdt")
        }
    })
}

fn body_start_id(content: &BodyContent) -> Result<Option<String>> {
    match content {
        BodyContent::RawXml(raw) => bookmark_start_id(raw, true),
        _ => Ok(None),
    }
}

impl Document {
    /// The body index an insertion before body item `index` uses: before
    /// the bookmark starts that lead the block there. `index` itself past
    /// the end, at a bookmark start or when nothing leads the block.
    #[doc(hidden)]
    pub fn block_insertion_index(&self, index: usize) -> usize {
        leading_starts(
            &self.document.body.content,
            index,
            body_is_block,
            body_start_id,
        )
        .ok()
        .and_then(|starts| starts.first().map(|(first, _)| *first))
        .unwrap_or(index)
    }

    /// The body indices of the leading bookmark starts of body item `index`
    /// whose bookmark ends inside the body items `owners`, the starts that go
    /// with those items when they move or are removed.
    #[doc(hidden)]
    pub fn owned_leading_bookmark_starts(
        &self,
        index: usize,
        owners: Range<usize>,
    ) -> Result<Vec<usize>> {
        let content = &self.document.body.content;
        let starts = leading_starts(content, index, body_is_block, body_start_id)?;
        if starts.is_empty() {
            return Ok(Vec::new());
        }
        let mut ends = HashSet::new();
        for block in &content[owners] {
            ends.extend(bookmark_end_ids(&serialize_content_fragment(
                block.clone(),
            )?)?);
        }
        Ok(starts
            .into_iter()
            .filter(|(_, id)| ends.contains(id))
            .map(|(index, _)| index)
            .collect())
    }
}
