//! What a [`ContentLocation`] names for the operations that insert, copy,
//! move or remove whole blocks, read by one resolver.
//!
//! A block location has four shapes, the same in every story:
//!
//! - `[i]`: the block that is story item `i`, a direct child of the story
//!   owner, or a block content control written directly in the content of
//!   another block control;
//! - `[c, k]`: paragraph `k` among the paragraphs written directly in the
//!   content of block control `c`, or with the kind table, table `k` among
//!   its tables;
//! - `end(story)`: after the story's last block, before the body's section
//!   properties;
//! - `control_end(story, c)`: after the last child of control `c`'s content.
//!
//! The container of a location is the story owner, or the `w:sdtContent` of
//! the block control the location is in, and the rules of the story level
//! (bookmark starts leading a block, owned markers) apply among the
//! container's direct children.

use super::*;

/// The container a block location counts in and the place it names there.
#[derive(Clone)]
pub(super) struct BlockSlot {
    /// The story owner, or the `w:sdtContent` of a block content control as
    /// an owner span of the story's kind.
    pub(super) owner: StoryOwnerSpan,
    /// The story item index of the control whose content is the container.
    pub(super) control: Option<usize>,
    /// What the container takes.
    takes: ContainerTakes,
    /// The container's direct children, the body's section properties
    /// excluded.
    pub(super) items: Vec<StoryItemSpan>,
    /// The index in `items` of the block named, `items.len()` at an end.
    pub(super) index: usize,
    /// The start of the block named, or the end of the container's content.
    pub(super) boundary: usize,
    /// Whether the location is an end.
    end: bool,
}

/// What a container takes, from its control's type.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ContainerTakes {
    /// Any block: a story owner, a rich text, gallery, group, repeating
    /// section item or untyped control.
    Blocks,
    /// One block: a control whose value Word keeps in one paragraph.
    OneBlock(&'static str),
    /// Repeating section items only.
    Items,
}

fn refused(message: String) -> Error {
    Error::Other(message)
}

/// The block control at story item `control`, or why that item is none.
fn block_control_item<'a>(
    items: &'a [StoryItemSpan],
    control: usize,
    path: &[usize],
) -> Result<&'a StoryItemSpan> {
    let item = items.get(control).ok_or_else(|| {
        refused(format!(
            "story item {control} is past the {} items of the story: {path:?} names nothing",
            items.len()
        ))
    })?;
    if item.kind != StoryItemKind::ContentControl {
        return Err(refused(format!(
            "story item {control} is a {}, not a block content control: [control, k] names a \
             paragraph or a table of a block content control, and [{control}] names the item \
             itself",
            item_kind_words(item.kind)
        )));
    }
    match item.sdt_context {
        Some(StorySdtContext::Block) => Ok(item),
        Some(StorySdtContext::Inline) => Err(refused(format!(
            "content control {control} is inside a paragraph and holds no block: name the \
             paragraph that holds it"
        ))),
        Some(StorySdtContext::Table | StorySdtContext::Row) => Err(refused(format!(
            "content control {control} is around table rows or cells and holds no block: its \
             paragraphs are in the stories of its cells"
        ))),
        None => Err(refused(format!(
            "content control {control} is not among the blocks of its story"
        ))),
    }
}

fn item_kind_words(kind: StoryItemKind) -> &'static str {
    match kind {
        StoryItemKind::Paragraph => "paragraph",
        StoryItemKind::Table => "table",
        StoryItemKind::ContentControl => "content control",
        StoryItemKind::Field => "field",
        StoryItemKind::Drawing => "drawing",
        StoryItemKind::PreservedNode => "preserved node",
    }
}

/// The `w:sdtContent` of the block control `control` as a container, what
/// it takes, and whether it is an empty element.
fn block_control_container(
    xml: &[u8],
    story: &StoryOwnerSpan,
    control: &StoryItemSpan,
) -> Result<(StoryOwnerSpan, ContainerTakes, bool)> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    // Depth below the control's `w:sdt` once reached, and whether the
    // element at depth 1 is its `w:sdtPr`.
    let mut depth: Option<usize> = None;
    let mut in_properties = false;
    let mut takes = ContainerTakes::Blocks;
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("block control scan failed: {error}")))?;
        let word = word_element(&namespace);
        let uri = match &namespace {
            ResolveResult::Bound(namespace) => namespace.as_ref().to_vec(),
            _ => Vec::new(),
        };
        drop(namespace);
        let after = reader.buffer_position() as usize;
        match (&event, depth) {
            (Event::Start(_), None) if before == control.full.start => depth = Some(0),
            (Event::Start(element) | Event::Empty(element), Some(level)) => {
                let local = element.local_name().as_ref().to_vec();
                let empty = matches!(event, Event::Empty(_));
                if level == 0 && word && local == b"sdtContent" {
                    let full = if empty {
                        before..after
                    } else {
                        before..story_element_end(xml, before)?
                    };
                    let owner = StoryOwnerSpan {
                        full,
                        ..story.clone()
                    };
                    return Ok((owner, takes, empty));
                }
                if level == 0 && word && local == b"sdtPr" {
                    in_properties = !empty;
                } else if level == 1 && in_properties {
                    takes = control_type_takes(&uri, &local).unwrap_or(takes);
                }
                if !empty {
                    depth = Some(level + 1);
                }
            }
            (Event::End(_), Some(level)) => {
                if level == 0 {
                    break;
                }
                if level == 1 {
                    in_properties = false;
                }
                depth = Some(level - 1);
            }
            (Event::Eof, _) => break,
            _ => {}
        }
        buffer.clear();
    }
    Err(refused(
        "the block content control has no content element".to_owned(),
    ))
}

/// What a control whose `w:sdtPr` holds `uri:local` takes, when that child
/// names a type that limits it.
fn control_type_takes(uri: &[u8], local: &[u8]) -> Option<ContainerTakes> {
    const W14: &[u8] = b"http://schemas.microsoft.com/office/word/2010/wordml";
    const W15: &[u8] = b"http://schemas.microsoft.com/office/word/2012/wordml";
    if uri == WORD_NAMESPACE.as_bytes() {
        return Some(ContainerTakes::OneBlock(match local {
            b"text" => "plain text",
            b"date" => "date",
            b"dropDownList" => "drop-down list",
            b"comboBox" => "combo box",
            b"picture" => "picture",
            _ => return None,
        }));
    }
    if uri == W14 && local == b"checkbox" {
        return Some(ContainerTakes::OneBlock("check box"));
    }
    if uri == W15 && local == b"repeatingSection" {
        return Some(ContainerTakes::Items);
    }
    None
}

/// Resolve `location` in the story owner `story` of `xml`.
pub(super) fn resolve_block_location(
    xml: &[u8],
    story: &StoryOwnerSpan,
    location: &ContentLocation,
) -> Result<BlockSlot> {
    let path = location.index_path.as_slice();
    let story_items = scan_story_items(xml, story, None)?;
    let story_slot = |index: Option<&StoryItemSpan>| -> Result<BlockSlot> {
        let items = direct_story_content_items_in(xml, story, &story_items)?;
        let (index, boundary) = match index {
            Some(item) => (
                items
                    .iter()
                    .position(|candidate| candidate.full == item.full)
                    .ok_or_else(|| Error::Other("direct story block was not found".to_owned()))?,
                item.full.start,
            ),
            None => (
                items.len(),
                story_owner_content_end_in(xml, story, &story_items)?,
            ),
        };
        Ok(BlockSlot {
            owner: story.clone(),
            control: None,
            takes: ContainerTakes::Blocks,
            items,
            index,
            boundary,
            end: location.is_end,
        })
    };
    // The slot of `block` (or the end) in the content of block control
    // `control`.
    let control_slot = |control: usize, block: Option<&Range<usize>>| -> Result<BlockSlot> {
        let item = block_control_item(&story_items, control, path)?;
        let (owner, takes, empty) = block_control_container(xml, story, item)?;
        let items = if empty {
            Vec::new()
        } else {
            direct_story_content_items(xml, &owner)?
        };
        let (index, boundary) = match block {
            Some(block) => (
                items
                    .iter()
                    .position(|candidate| candidate.full == *block)
                    .ok_or_else(|| {
                        refused(format!(
                            "the block at {path:?} is not written directly in the content of \
                             content control {control}"
                        ))
                    })?,
                block.start,
            ),
            // The content of an empty owner goes where its attributes end.
            None if empty => (
                0,
                owner.full.start
                    + oxoffice_core::xml_element::start_tag(&xml[owner.full.start..])?
                        .attributes_end,
            ),
            None => (items.len(), story_owner_content_end(xml, &owner)?),
        };
        Ok(BlockSlot {
            owner,
            control: Some(control),
            takes,
            items,
            index,
            boundary,
            end: location.is_end,
        })
    };
    match (location.is_end, path) {
        (true, []) => story_slot(None),
        (true, [control]) => control_slot(*control, None),
        (false, [index]) => {
            let item = story_items.get(*index).ok_or_else(|| {
                refused(format!(
                    "story item {index} is past the {} items of the story: the location after \
                     its last block is end(story)",
                    story_items.len()
                ))
            })?;
            if item.kind != location.item_kind {
                return Err(StoryError::KindMismatch {
                    expected: location.item_kind,
                    actual: item.kind,
                }
                .into());
            }
            if item.direct_owner_child {
                if story.kind == StoryKind::Body
                    && content_fragment_root_is_section_properties(xml, item)?
                {
                    return Err(refused(format!(
                        "story item {index} is the body's section properties, not a block: the \
                         location before them is end(story)"
                    )));
                }
                return story_slot(Some(item));
            }
            if item.kind == StoryItemKind::ContentControl
                && item.sdt_context == Some(StorySdtContext::Block)
            {
                // The innermost block control around it holds it.
                let parent = story_items
                    .iter()
                    .enumerate()
                    .filter(|(_, candidate)| {
                        candidate.kind == StoryItemKind::ContentControl
                            && candidate.sdt_context == Some(StorySdtContext::Block)
                            && candidate.full.start < item.full.start
                            && item.full.end <= candidate.full.end
                    })
                    .max_by_key(|(_, candidate)| candidate.full.start)
                    .map(|(parent, _)| parent)
                    .ok_or_else(|| {
                        refused(format!(
                            "content control {index} is not among the blocks of its story"
                        ))
                    })?;
                return control_slot(parent, Some(&item.full));
            }
            Err(refused(format!(
                "story item {index} is a {} inside a paragraph or a table, not a block: name the \
                 block that holds it",
                item_kind_words(item.kind)
            )))
        }
        (false, [control, ordinal]) => {
            let Some(block) = control_block_name(location.item_kind) else {
                return Err(refused(format!(
                    "[{control}, {ordinal}] names a paragraph or a table of a block content \
                     control, not a {}: a nested block control is its own story item [i]",
                    item_kind_words(location.item_kind)
                )));
            };
            let (noun, what) = if block == b"p" {
                ("paragraph", "paragraphs")
            } else {
                ("table", "tables")
            };
            let item = block_control_item(&story_items, *control, path)?;
            let blocks = scan_story_control_blocks(xml, item, block)?;
            let Some(span) = blocks.get(*ordinal) else {
                if block_control_container(xml, story, item)?.1 == ContainerTakes::Items {
                    return Err(refused(format!(
                        "content control {control} is a repeating section, which holds its \
                         items and no {what}: name a {noun} of an item ([item, k]) or an \
                         item's end (control_end(story, item)), or add an item with \
                         repeat_content_control"
                    )));
                }
                return Err(refused(format!(
                    "content control {control} has {} {what}, so [{control}, {ordinal}] is \
                     past them: the location after its last block is control_end(story, \
                     {control})",
                    blocks.len()
                )));
            };
            control_slot(*control, Some(span))
        }
        _ => Err(refused(format!(
            "a block location is [i], [control, k], end(story) or control_end(story, control); \
             {}{path:?} is none of them",
            if location.is_end { "the end " } else { "" }
        ))),
    }
}

impl BlockSlot {
    /// The block the location names; an end names none.
    pub(super) fn block(&self) -> Result<&StoryItemSpan> {
        if self.end {
            return Err(refused(
                "an end is a boundary between blocks, not a block: name the block itself (a \
                 location [i] or [control, k])"
                    .to_owned(),
            ));
        }
        self.items
            .get(self.index)
            .ok_or_else(|| Error::Other("located block disappeared".to_owned()))
    }

    /// The boundary and index before the bookmark starts that lead the
    /// block named, or the end, without asking whether the container takes a
    /// block there (the end of a range).
    pub(super) fn leading_boundary(&self, xml: &[u8]) -> Result<(usize, usize)> {
        let first = leading_bookmark_start_item(xml, &self.items, self.index)?;
        Ok(if first == self.index {
            (self.boundary, self.index)
        } else {
            (self.items[first].full.start, first)
        })
    }

    /// Refuse writing `content`, the closed XML of the blocks an operation
    /// writes, where the container does not take it: a repeating section
    /// takes its items only, and an item goes nowhere else; a typed control
    /// that holds a block takes no other.
    pub(super) fn check_takes(&self, content: &[u8]) -> Result<()> {
        let control = self.control.unwrap_or_default();
        let (items, others) = block_kinds(content)?;
        match self.takes {
            ContainerTakes::Items if others > 0 => Err(refused(format!(
                "content control {control} is a repeating section, whose content holds only its \
                 items: add an item with repeat_content_control, or put the block in an item \
                 (control_end(story, item))"
            ))),
            ContainerTakes::Items => Ok(()),
            _ if items > 0 => Err(refused(
                "a repeating section item stays directly in the content of a repeating section: \
                 copy or move it before another item or to control_end(story, section), or add \
                 one with repeat_content_control"
                    .to_owned(),
            )),
            ContainerTakes::OneBlock(kind) if self.items.iter().any(is_block) => {
                Err(refused(format!(
                    "content control {control} is a {kind} control, which holds its value in one \
                     block: set the control's value, or remove or unwrap the control"
                )))
            }
            ContainerTakes::Blocks | ContainerTakes::OneBlock(_) => Ok(()),
        }
    }

    /// [`Self::leading_boundary`] for an insertion of `content`: refused
    /// where the container does not take it ([`Self::check_takes`]).
    pub(super) fn insertion(&self, xml: &[u8], content: &[u8]) -> Result<(usize, usize)> {
        self.check_takes(content)?;
        self.leading_boundary(xml)
    }

    /// Refuse taking the block named out of its container when the
    /// container needs it: the only block of a block control, or of a
    /// header, footer, note, comment or text box, whose schema asks for one;
    /// the last paragraph of a table cell, which needs one (in another block
    /// of the cell, a control's included). The body may be empty.
    pub(super) fn check_leaves_a_block(&self, xml: &[u8]) -> Result<()> {
        let others = |keep: fn(&StoryItemSpan) -> bool| {
            self.items
                .iter()
                .enumerate()
                .any(|(index, item)| index != self.index && keep(item))
        };
        let control_block = self
            .items
            .get(self.index)
            .is_some_and(|item| item.kind == StoryItemKind::ContentControl);
        let unwrap = if control_block {
            ", or unwrap the control (unwrap_content_control), which keeps its content"
        } else {
            ""
        };
        if let Some(control) = self.control {
            if others(is_block) {
                return Ok(());
            }
            return Err(refused(format!(
                "this is the only block of content control {control}: remove the control itself \
                 (remove_content_control), or insert its new content first{unwrap}"
            )));
        }
        if self.owner.kind == StoryKind::TableCell {
            // A paragraph anywhere in another block of the cell counts, as
            // `remove_content_control` counts it.
            let holds_paragraph = |item: &StoryItemSpan| -> Result<bool> {
                if item.kind == StoryItemKind::Paragraph || !is_block(item) {
                    return Ok(item.kind == StoryItemKind::Paragraph);
                }
                let scope = story_namespace_scope_at(xml, item.full.start)?;
                let closed = close_content_fragment_namespaces(&xml[item.full.clone()], &scope)?;
                let mut reader = NsReader::from_reader(closed.as_slice());
                let mut buffer = Vec::new();
                loop {
                    match reader.read_resolved_event_into(&mut buffer) {
                        Ok((namespace, Event::Start(element) | Event::Empty(element)))
                            if word_element(&namespace)
                                && element.local_name().as_ref() == b"p" =>
                        {
                            return Ok(true);
                        }
                        Ok((_, Event::Eof)) | Err(_) => return Ok(false),
                        _ => buffer.clear(),
                    }
                }
            };
            for (index, item) in self.items.iter().enumerate() {
                if index != self.index && holds_paragraph(item)? {
                    return Ok(());
                }
            }
            return Err(refused(format!(
                "this is the only paragraph of its table cell, which ends with one: clear its \
                 text, or remove its row{unwrap}"
            )));
        }
        if others(is_block) {
            return Ok(());
        }
        let (what, route) = match self.owner.kind {
            StoryKind::Header => ("header", ""),
            StoryKind::Footer => ("footer", ""),
            StoryKind::Footnote => ("footnote", ", or remove the footnote (remove_footnote)"),
            StoryKind::Endnote => ("endnote", ", or delete the endnote with its reference"),
            StoryKind::Comment => ("comment", ", or remove the comment (remove_comment)"),
            StoryKind::TextBox => ("text box", ", or clear its text"),
            _ => return Ok(()),
        };
        Err(refused(format!(
            "this is the only block of its {what}, which holds at least one: insert its new \
             content before removing this block{route}{unwrap}"
        )))
    }

    /// The index path a paragraph written at `boundary` of this container
    /// takes: `[j]` in the story, `[c, k]` in control `c`.
    pub(super) fn paragraph_path_at(
        &self,
        xml: &[u8],
        story: &StoryOwnerSpan,
        boundary: usize,
    ) -> Result<Vec<usize>> {
        match self.control {
            None => Ok(vec![
                scan_story_items(xml, story, None)?
                    .iter()
                    .filter(|item| item.full.start < boundary)
                    .count(),
            ]),
            Some(control) => {
                let item = scan_story_items(xml, story, None)?
                    .get(control)
                    .cloned()
                    .ok_or_else(|| Error::Other("block control disappeared".to_owned()))?;
                let before = scan_story_control_paragraphs(xml, &item)?
                    .iter()
                    .filter(|paragraph| paragraph.start < boundary)
                    .count();
                Ok(vec![control, before])
            }
        }
    }
}

/// How many repeating section items and how many other blocks the closed
/// XML `content` holds at its top level.
fn block_kinds(content: &[u8]) -> Result<(usize, usize)> {
    const W15: &str = "http://schemas.microsoft.com/office/word/2012/wordml";
    let mut wrapped =
        format!("<root xmlns:w=\"{WORD_NAMESPACE}\" xmlns:w15=\"{W15}\">").into_bytes();
    wrapped.extend_from_slice(content);
    wrapped.extend_from_slice(b"</root>");
    let mut reader = NsReader::from_reader(wrapped.as_slice());
    let mut buffer = Vec::new();
    let (mut items, mut others) = (0, 0);
    // Open elements; the top-level control being read and whether its
    // properties name it an item.
    let mut depth = 0usize;
    let mut control: Option<bool> = None;
    loop {
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("block content scan failed: {error}")))?;
        let word = word_element(&namespace);
        let w15 = matches!(&namespace, ResolveResult::Bound(uri) if uri.as_ref() == W15.as_bytes());
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let local = element.local_name();
                let empty = matches!(event, Event::Empty(_));
                if depth == 1 && word && matches!(local.as_ref(), b"p" | b"tbl") {
                    others += 1;
                } else if depth == 1 && word && local.as_ref() == b"sdt" {
                    if empty {
                        others += 1;
                    } else {
                        control = Some(false);
                    }
                } else if depth == 3 && w15 && local.as_ref() == b"repeatingSectionItem" {
                    control = control.map(|_| true);
                }
                if !empty {
                    depth += 1;
                }
            }
            Event::End(_) => {
                depth -= 1;
                if depth == 1
                    && let Some(item) = control.take()
                {
                    if item {
                        items += 1;
                    } else {
                        others += 1;
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok((items, others))
}

fn is_block(item: &StoryItemSpan) -> bool {
    matches!(
        item.kind,
        StoryItemKind::Paragraph | StoryItemKind::Table | StoryItemKind::ContentControl
    )
}

/// The complex field marker the bytes `ranges` of `xml` hold without its
/// pair: an end whose begin is before them, or a begin whose end is after
/// them. `None` when they hold whole fields only.
pub(super) fn unpaired_complex_field_marker(
    xml: &[u8],
    ranges: &[Range<usize>],
) -> Result<Option<ComplexFieldMarker>> {
    let Some(last) = ranges.iter().map(|range| range.end).max() else {
        return Ok(None);
    };
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut open = 0usize;
    loop {
        let before = reader.buffer_position() as usize;
        if before >= last {
            break;
        }
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("complex field scan failed: {error}")))?;
        match event {
            Event::Start(element) | Event::Empty(element)
                if word_element(&namespace)
                    && element.local_name().as_ref() == b"fldChar"
                    && ranges.iter().any(|range| range.contains(&before)) =>
            {
                match complex_field_marker(&reader, &element)? {
                    Some(ComplexFieldMarker::Begin) => open += 1,
                    Some(ComplexFieldMarker::End) if open == 0 => {
                        return Ok(Some(ComplexFieldMarker::End));
                    }
                    Some(ComplexFieldMarker::End) => open -= 1,
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok((open > 0).then_some(ComplexFieldMarker::Begin))
}

/// Refuse copying `what`, the bytes `ranges` of `xml`, when it holds part of
/// a complex field.
pub(super) fn refuse_partial_complex_field(
    xml: &[u8],
    ranges: &[Range<usize>],
    what: &str,
) -> Result<()> {
    let (held, missing) = match unpaired_complex_field_marker(xml, ranges)? {
        None => return Ok(()),
        Some(ComplexFieldMarker::End) => ("end", "begin"),
        Some(_) => ("begin", "end"),
    };
    Err(refused(format!(
        "{what} holds the {held} of a complex field without its {missing}, which a copy would \
         leave unpaired: copy blocks that hold whole fields, or a range from the field's begin \
         to its end (from_range)"
    )))
}

impl Document {
    /// The location the paragraph an insertion at `destination` writes
    /// takes, read before the insertion: `[j]` in the story, `[c, k]` in a
    /// block control.
    #[doc(hidden)]
    pub fn block_insertion_location(
        &self,
        destination: &ContentLocation,
    ) -> Result<ContentLocation> {
        let (source, owner) = self.story_source_and_owner(&destination.story)?;
        let xml = source.xml.as_ref();
        let slot = resolve_block_location(xml, &owner, destination)?;
        let (boundary, _) = slot.insertion(xml, b"<w:p/>")?;
        Ok(ContentLocation::new(
            destination.story.clone(),
            StoryItemKind::Paragraph,
            slot.paragraph_path_at(xml, &owner, boundary)?,
        ))
    }

    /// The tables of `story` outside its cells, in document order: a table
    /// that is a story item, `[i]`, and a table written in a block content
    /// control at any depth, `[c, k]`.
    pub fn story_tables(&self, story: &StoryId) -> Result<Vec<ContentLocation>> {
        let (source, owner) = self.story_source_and_owner(story)?;
        let xml = source.xml.as_ref();
        let mut tables = Vec::new();
        for (index, item) in scan_story_items(xml, &owner, None)?.iter().enumerate() {
            match item.kind {
                StoryItemKind::Table => tables.push((item.full.start, vec![index])),
                StoryItemKind::ContentControl
                    if item.sdt_context == Some(StorySdtContext::Block) =>
                {
                    for (k, table) in scan_story_control_blocks(xml, item, b"tbl")?
                        .into_iter()
                        .enumerate()
                    {
                        tables.push((table.start, vec![index, k]));
                    }
                }
                _ => {}
            }
        }
        tables.sort_by_key(|(start, _)| *start);
        Ok(tables
            .into_iter()
            .map(|(_, path)| ContentLocation::new(story.clone(), StoryItemKind::Table, path))
            .collect())
    }

    /// The location of body table `table_index`, the index
    /// [`Self::table`] takes: entry `table_index` of the body's
    /// [`Self::story_tables`]. `None` past the tables, or when the model
    /// counts another number of body tables than the source lists.
    pub fn table_story_location(&self, table_index: usize) -> Result<Option<ContentLocation>> {
        let body = self
            .stories()?
            .into_iter()
            .find(|story| story.kind == StoryKind::Body)
            .ok_or_else(|| Error::Other("document body story is missing".to_owned()))?;
        let tables = self.story_tables(&body)?;
        if tables.len() != self.document.body.tables().count() {
            return Ok(None);
        }
        Ok(tables.into_iter().nth(table_index))
    }

    /// Whether `location` names a block, as the block operations read it.
    pub(crate) fn names_a_block(&self, location: &ContentLocation) -> bool {
        self.story_source_and_owner(&location.story)
            .and_then(|(source, owner)| {
                resolve_block_location(source.xml.as_ref(), &owner, location)
                    .and_then(|slot| slot.block().map(|_| ()))
            })
            .is_ok()
    }
}
