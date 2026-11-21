//! Scanning stories: owners, complex field markers, semantic projections, items and links.

use super::*;

pub(super) fn opaque_story_event(
    stack: &mut [XmlElementFrame],
    namespace: StoryNamespace,
    local_name: &[u8],
    element: &BytesStart<'_>,
    empty: bool,
    xml: &[u8],
    before: usize,
    after: usize,
) -> Result<bool> {
    Ok(stack.last().is_some_and(|frame| frame.opaque)
        || !modeled_story_event(
            stack, namespace, local_name, element, empty, xml, before, after,
        )?)
}

fn direct_story_item_kind(is_word: bool, local_name: &[u8]) -> StoryItemKind {
    if !is_word {
        return StoryItemKind::PreservedNode;
    }
    match local_name {
        b"p" => StoryItemKind::Paragraph,
        b"tbl" => StoryItemKind::Table,
        b"sdt" => StoryItemKind::ContentControl,
        _ => StoryItemKind::PreservedNode,
    }
}

fn owner_kind(local_name: &[u8]) -> Option<StoryKind> {
    match local_name {
        b"body" => Some(StoryKind::Body),
        b"tc" => Some(StoryKind::TableCell),
        b"hdr" => Some(StoryKind::Header),
        b"ftr" => Some(StoryKind::Footer),
        b"footnote" => Some(StoryKind::Footnote),
        b"endnote" => Some(StoryKind::Endnote),
        b"comment" => Some(StoryKind::Comment),
        b"txbxContent" => Some(StoryKind::TextBox),
        _ => None,
    }
}

pub(super) fn editable_note_owner(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
) -> Result<bool> {
    let mut id = None;
    let mut separator = false;
    let mut normal_type = true;
    for attribute in element.attributes() {
        let attribute = attribute
            .map_err(|error| Error::Other(format!("story owner attribute scan failed: {error}")))?;
        let (namespace, local_name) = reader.resolver().resolve_attribute(attribute.key);
        if !word_element(&namespace) {
            continue;
        }
        match local_name.as_ref() {
            b"type" => {
                let value = attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                    .map_err(|error| {
                        Error::Other(format!("story owner type decode failed: {error}"))
                    })?;
                separator = matches!(
                    value.as_ref(),
                    "separator" | "continuationSeparator" | "continuationNotice"
                );
                normal_type = value == "normal";
            }
            b"id" => {
                id = attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                    .map_err(|error| {
                        Error::Other(format!("story owner id decode failed: {error}"))
                    })?
                    .parse::<i32>()
                    .ok();
            }
            _ => {}
        }
    }
    Ok(!separator && id.is_some_and(|id| normal_type || id > 0))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ComplexFieldMarker {
    Begin,
    Separate,
    End,
}

struct StoryFieldRun {
    range: Range<usize>,
    parent_start: Option<usize>,
}

struct ComplexFieldScan {
    marker_start: usize,
    run_start: usize,
    run_parent_start: Option<usize>,
    ancestors: Vec<usize>,
    separators: usize,
    instruction: String,
    leading_nested: bool,
    valid: bool,
}

impl ComplexFieldScan {
    fn accepted(&self) -> bool {
        self.valid
            && self.separators <= 1
            && !self.leading_nested
            && field_instruction_has_name(&self.instruction)
    }
}

pub(super) fn complex_field_marker(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
) -> Result<Option<ComplexFieldMarker>> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| {
            Error::Other(format!("field marker attribute scan failed: {error}"))
        })?;
        let (namespace, local_name) = reader.resolver().resolve_attribute(attribute.key);
        if !word_element(&namespace) || local_name.as_ref() != b"fldCharType" {
            continue;
        }
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
            .map_err(|error| Error::Other(format!("field marker type decode failed: {error}")))?;
        return Ok(match value.as_ref() {
            "begin" => Some(ComplexFieldMarker::Begin),
            "separate" => Some(ComplexFieldMarker::Separate),
            "end" => Some(ComplexFieldMarker::End),
            _ => None,
        });
    }
    Ok(None)
}

fn story_enclosing_run_source(
    xml: &[u8],
    stack: &[XmlElementFrame],
) -> Result<Option<StoryFieldRun>> {
    let Some((index, frame)) = stack.iter().enumerate().rev().find(|(_, frame)| {
        !frame.opaque
            && frame.namespace == StoryNamespace::Word
            && frame.local_name.as_ref() == b"r"
    }) else {
        return Ok(None);
    };
    Ok(Some(StoryFieldRun {
        range: frame.full_start..story_element_end(xml, frame.full_start)?,
        parent_start: index.checked_sub(1).map(|parent| stack[parent].full_start),
    }))
}

fn complex_field_scan_context(stack: &[XmlElementFrame]) -> bool {
    stack.iter().any(|frame| {
        !frame.opaque
            && frame.namespace == StoryNamespace::Word
            && frame.local_name.as_ref() == b"r"
    }) && !stack.iter().any(|frame| {
        frame.namespace == StoryNamespace::Word && frame.local_name.as_ref() == b"fldSimple"
    })
}

fn scan_complex_field_marker(
    fields: &mut Vec<ComplexFieldScan>,
    items: &mut Vec<StoryItemSpan>,
    marker: Option<ComplexFieldMarker>,
    before: usize,
    after: usize,
    run: StoryFieldRun,
) {
    match marker {
        Some(ComplexFieldMarker::Begin) => {
            let ancestors = fields.iter().map(|field| field.marker_start).collect();
            fields.push(ComplexFieldScan {
                marker_start: before,
                run_start: run.range.start,
                run_parent_start: run.parent_start,
                ancestors,
                separators: 0,
                instruction: String::new(),
                leading_nested: false,
                valid: true,
            });
        }
        Some(ComplexFieldMarker::Separate) => {
            if let Some(field) = fields.last_mut() {
                field.separators += 1;
                if field.separators > 1 {
                    field.valid = false;
                }
            }
        }
        Some(ComplexFieldMarker::End) => {
            let Some(field) = fields.pop() else {
                return;
            };
            let accepted = field.accepted();
            if let Some(parent) = fields.last_mut() {
                if !accepted {
                    parent.valid = false;
                } else if parent.separators == 0 && !field_instruction_has_name(&parent.instruction)
                {
                    parent.leading_nested = true;
                }
            }
            // A first-to-last run excerpt is a balanced sibling sequence only
            // under the same physical parent. This is typed eligibility, not
            // intrinsic field validity or a reason to poison an outer field.
            if accepted && field.run_parent_start == run.parent_start {
                items.push(StoryItemSpan {
                    kind: StoryItemKind::Field,
                    full: field.marker_start..after,
                    scan: field.run_start..run.range.end,
                    direct_owner_child: false,
                    complex_field: true,
                    complex_ancestors: field.ancestors,
                    sdt_context: None,
                });
            }
        }
        None => {}
    }
}

fn push_complex_field_instruction(
    fields: &mut [ComplexFieldScan],
    stack: &[XmlElementFrame],
    text: &str,
) {
    let modeled_instruction = stack.last().is_some_and(|frame| {
        !frame.opaque
            && frame.namespace == StoryNamespace::Word
            // Word writes the code of a deleted field in delInstrText.
            && matches!(frame.local_name.as_ref(), b"instrText" | b"delInstrText")
    });
    if modeled_instruction
        && let Some(field) = fields.last_mut()
        && field.separators == 0
    {
        field.instruction.push_str(text);
    }
}

pub(super) fn update_complex_field_results(
    results: &mut Vec<bool>,
    marker: Option<ComplexFieldMarker>,
) {
    match marker {
        Some(ComplexFieldMarker::Begin) => results.push(false),
        Some(ComplexFieldMarker::Separate) => {
            if let Some(result) = results.last_mut() {
                *result = true;
            }
        }
        Some(ComplexFieldMarker::End) => {
            results.pop();
        }
        None => {}
    }
}

/// Return whether text at this point is visible: no open complex field is
/// still in its instruction part, and a field item is inside its own field.
pub(super) fn complex_field_text_visible(item: &StoryItemSpan, results: &[bool]) -> bool {
    (!item.complex_field || !results.is_empty()) && results.iter().all(|result| *result)
}

/// The fields open where a piece of story text was read, by index into
/// [`StoryFieldTracker`], each with whether it had reached its result.
pub(super) type StoryFieldContext = Vec<(usize, bool)>;

/// The complex fields an item other than a field item opens, in source order.
///
/// The typed model reads a field as its cached result only when the field
/// closes within the paragraph. An open field, such as the first paragraph of
/// a table of contents, reads as plain runs. Whether a field hides text is
/// only known at its end marker, so text records the [`Self::context`] it was
/// read in, and [`Self::hides`] decides once the item is scanned.
#[derive(Default)]
pub(super) struct StoryFieldTracker {
    /// The fields open now, by index, each with whether it has separated.
    open: StoryFieldContext,
    closed: Vec<bool>,
}

impl StoryFieldTracker {
    pub(super) fn mark(&mut self, marker: Option<ComplexFieldMarker>) {
        match marker {
            Some(ComplexFieldMarker::Begin) => {
                self.open.push((self.closed.len(), false));
                self.closed.push(false);
            }
            Some(ComplexFieldMarker::Separate) => {
                if let Some((_, separated)) = self.open.last_mut() {
                    *separated = true;
                }
            }
            Some(ComplexFieldMarker::End) => {
                if let Some((field, _)) = self.open.pop() {
                    self.closed[field] = true;
                }
            }
            None => {}
        }
    }

    /// The open fields, each with whether it has reached its result.
    pub(super) fn context(&self) -> StoryFieldContext {
        self.open.clone()
    }

    /// Whether a closed field was still in its instruction part.
    pub(super) fn hides(&self, context: &[(usize, bool)]) -> bool {
        context
            .iter()
            .any(|(field, separated)| self.closed[*field] && !separated)
    }

    /// Whether the text lies in the result of a closed field.
    pub(super) fn in_result(&self, context: &[(usize, bool)]) -> bool {
        context.iter().any(|(field, _)| self.closed[*field])
    }
}

/// Return whether a field character at `position` opens, separates or closes
/// a field that hides text of this item. A field item tracks the markers of
/// its own field. Any other item tracks every accepted marker of its own
/// story, so a field's instruction part stays out of its text as in
/// `Paragraph::text`.
pub(super) fn story_field_marker_tracked(
    item: &StoryItemSpan,
    position: usize,
    nested_owner_depth: usize,
    stack: &[XmlElementFrame],
) -> bool {
    if item.complex_field {
        item.full.start <= position && position < item.full.end
    } else {
        nested_owner_depth == 0 && accepted_story_text_visible(stack)
    }
}

/// One piece of story text, kept with the fields open where it was read.
pub(super) enum StoryTextPiece {
    Text(String),
    /// A tab or break, as read outside and inside a field result.
    Character(Option<char>, Option<char>),
}

/// The text a visible run `w:br` or `w:cr` adds, as the typed model reads it.
/// Direct runs and flattened simple fields read breaks as line feeds.
/// Inside a closed complex field's cached result, page and column breaks
/// read as form feeds and vertical tabs. CR remains a line feed in both views.
pub(super) fn story_break_text(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
    in_field_result: bool,
) -> Result<Option<char>> {
    if element.local_name().as_ref() == b"cr" {
        return Ok(Some('\n'));
    }
    if !in_field_result {
        return Ok(Some('\n'));
    }
    for attribute in element.attributes() {
        let attribute = attribute
            .map_err(|error| Error::Other(format!("break attribute scan failed: {error}")))?;
        let (namespace, local_name) = reader.resolver().resolve_attribute(attribute.key);
        if !word_element(&namespace) || local_name.as_ref() != b"type" {
            continue;
        }
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
            .map_err(|error| Error::Other(format!("break type decode failed: {error}")))?;
        return Ok(Some(match value.as_ref() {
            "page" => '\u{000c}',
            "column" => '\u{000b}',
            _ => '\n',
        }));
    }
    Ok(Some('\n'))
}

pub(super) fn accepted_story_text_visible(stack: &[XmlElementFrame]) -> bool {
    !stack.iter().any(|frame| {
        !frame.opaque
            && frame.namespace == StoryNamespace::Word
            && matches!(frame.local_name.as_ref(), b"del" | b"moveFrom")
    })
}

pub(crate) fn scan_story_owners(xml: &[u8], root_kind: StoryKind) -> Result<Vec<StoryOwnerSpan>> {
    scan_story_owners_with_special(xml, root_kind, false)
}

// Read-only physical paragraph membership for the existing custody convention.
// These ranges belong only to this input, and confer no edit authority.
pub(crate) fn admitted_story_paragraph_spans(
    xml: &[u8],
    root_kind: StoryKind,
) -> Result<Vec<Range<usize>>> {
    let owners = scan_story_owners_with_special(xml, root_kind, true)?
        .into_iter()
        .map(|owner| {
            let story = StoryId {
                kind: owner.kind,
                part_name: String::new(),
                owner_index: owner.owner_index,
                fingerprint: owner.fingerprint,
            };
            (story, owner)
        })
        .collect::<Vec<_>>();
    let mut paragraphs = story_range_paragraph_spans(xml, &owners)?
        .into_iter()
        .flatten()
        .map(|(_, span)| span)
        .collect::<Vec<_>>();
    paragraphs.sort_by_key(|span| (span.start, span.end));
    paragraphs.dedup();
    Ok(paragraphs)
}

pub(crate) fn scan_story_owners_with_special(
    xml: &[u8],
    root_kind: StoryKind,
    include_special: bool,
) -> Result<Vec<StoryOwnerSpan>> {
    let empty_word_prefixes = Arc::new(Vec::new());
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut stack: Vec<XmlElementFrame> = Vec::new();
    let mut owners = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = match reader.read_resolved_event_into(&mut buffer) {
            Ok(event) => event,
            Err(error) => {
                return Err(Error::Other(format!("story XML scan failed: {error}")));
            }
        };
        let namespace_kind = story_namespace(&namespace);
        let is_word_namespace = namespace_kind == StoryNamespace::Word;
        drop(namespace);
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                let is_word = is_word_namespace;
                let local_name = story_frame_local_name(element.local_name().as_ref());
                let sdt_context = story_element_sdt_context(&stack, namespace_kind, &local_name);
                let direct_owner_child =
                    stack.last().is_some_and(|frame| frame.owner_kind.is_some());
                let mut opaque = opaque_story_event(
                    &mut stack,
                    namespace_kind,
                    &local_name,
                    &element,
                    false,
                    xml,
                    before,
                    after,
                )? || (direct_owner_child
                    && direct_story_item_kind(is_word, &local_name)
                        == StoryItemKind::PreservedNode);
                let mut candidate = (!opaque && is_word)
                    .then(|| owner_kind(&local_name))
                    .flatten();
                if matches!(candidate, Some(StoryKind::Footnote | StoryKind::Endnote))
                    && !include_special
                    && !editable_note_owner(&reader, &element)?
                {
                    candidate = None;
                    opaque = true;
                }
                let inherited = stack
                    .last()
                    .map_or(&empty_word_prefixes, |frame| &frame.word_prefixes);
                let word_prefixes = story_word_prefixes_at(&element, inherited, is_word)?;
                stack.push(XmlElementFrame {
                    namespace: namespace_kind,
                    local_name,
                    is_word,
                    full_start: before,
                    owner_kind: candidate.filter(|kind| {
                        *kind == root_kind
                            || matches!(kind, StoryKind::TableCell | StoryKind::TextBox)
                    }),
                    item_kind: None,
                    direct_owner_child: false,
                    sdt_context,
                    word_prefixes,
                    opaque,
                    typed_sdt_content_seen: false,
                });
            }
            Event::Empty(element) => {
                let direct_owner_child =
                    stack.last().is_some_and(|frame| frame.owner_kind.is_some());
                let local_name = element.local_name();
                let opaque = opaque_story_event(
                    &mut stack,
                    namespace_kind,
                    local_name.as_ref(),
                    &element,
                    true,
                    xml,
                    before,
                    after,
                )? || (direct_owner_child
                    && direct_story_item_kind(is_word_namespace, local_name.as_ref())
                        == StoryItemKind::PreservedNode);
                if is_word_namespace
                    && !opaque
                    && let Some(kind) = owner_kind(local_name.as_ref()).filter(|kind| {
                        *kind == root_kind
                            || matches!(kind, StoryKind::TableCell | StoryKind::TextBox)
                    })
                    && (!matches!(kind, StoryKind::Footnote | StoryKind::Endnote)
                        || include_special
                        || editable_note_owner(&reader, &element)?)
                {
                    owners.push(StoryOwnerSpan {
                        kind,
                        owner_index: 0,
                        full: before..after,
                        fingerprint: structural_fingerprint(&xml[before..after])?,
                    });
                }
            }
            Event::End(_) => {
                let Some(frame) = stack.pop() else {
                    return Err(Error::Other(
                        "story XML contains an unmatched closing element".to_owned(),
                    ));
                };
                if let Some(kind) = frame.owner_kind {
                    owners.push(StoryOwnerSpan {
                        kind,
                        owner_index: 0,
                        full: frame.full_start..after,
                        fingerprint: structural_fingerprint(&xml[frame.full_start..after])?,
                    });
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    owners.sort_by_key(|owner| owner.full.start);
    let mut counts = HashMap::new();
    for owner in &mut owners {
        let count = counts.entry(owner.kind).or_insert(0usize);
        owner.owner_index = *count;
        *count += 1;
    }
    Ok(owners)
}

/// Pair the non-cell story owners of `scanned`, the bytes revision resolution
/// scans, with the identities [`Document::stories`] reports from `reported`,
/// its own bytes for the same part.
///
/// Owners pair in order when both byte streams have the same owner kinds.
/// Otherwise the two disagree, for example because the typed serialization
/// dropped the namespace binding of a text box, so root owners pair by index
/// and text boxes by identical bytes. An owner without a partner is left
/// out, which folds its revisions into the owner around it.
pub(super) fn paired_story_owners(
    part_name: &str,
    root_kind: StoryKind,
    scanned: &[u8],
    reported: Option<&[u8]>,
) -> Result<Vec<(Range<usize>, StoryId)>> {
    let non_cell_owners = |xml: &[u8]| -> Result<Vec<StoryOwnerSpan>> {
        Ok(scan_story_owners(xml, root_kind)?
            .into_iter()
            .filter(|owner| owner.kind != StoryKind::TableCell)
            .collect())
    };
    let scanned_owners = non_cell_owners(scanned)?;
    let reported_owners = match reported {
        Some(xml) if xml == scanned => scanned_owners.clone(),
        Some(xml) => non_cell_owners(xml)?,
        None => Vec::new(),
    };
    let story = |owner: &StoryOwnerSpan| StoryId {
        kind: owner.kind,
        part_name: part_name.to_owned(),
        owner_index: owner.owner_index,
        fingerprint: owner.fingerprint,
    };
    let same_kinds = scanned_owners.len() == reported_owners.len()
        && scanned_owners
            .iter()
            .zip(&reported_owners)
            .all(|(scanned, reported)| scanned.kind == reported.kind);
    if same_kinds {
        return Ok(scanned_owners
            .iter()
            .zip(&reported_owners)
            .map(|(scanned, reported)| (scanned.full.clone(), story(reported)))
            .collect());
    }
    let mut unpaired = reported_owners.iter().collect::<Vec<_>>();
    Ok(scanned_owners
        .iter()
        .filter_map(|scanned| {
            let position = unpaired.iter().position(|reported| {
                reported.kind == scanned.kind
                    && if scanned.kind == root_kind {
                        reported.owner_index == scanned.owner_index
                    } else {
                        reported.fingerprint == scanned.fingerprint
                    }
            })?;
            Some((scanned.full.clone(), story(unpaired.remove(position))))
        })
        .collect())
}

// Anchor capture uses trim_text(true). Apply precisely that parser projection
// to the original span, while retaining element names, attributes and order.
fn drawing_capture_key(xml: &[u8]) -> Result<Vec<u8>> {
    let mut reader = quick_xml::Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut writer = quick_xml::Writer::new(Vec::new());
    let mut buffer = Vec::new();
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("drawing capture scan failed: {error}")))?;
        if matches!(event, Event::Eof) {
            break;
        }
        writer
            .write_event(event)
            .map_err(|error| Error::Other(error.to_string()))?;
        buffer.clear();
    }
    Ok(writer.into_inner())
}

/// Bind modeled selected anchors to actual physical story owners before shaping.
pub(super) fn bind_text_box_source_owners(
    body: &mut oxoffice_docx_oxml::document::CT_Body,
    xml: &[u8],
    root_kind: StoryKind,
    scope: Range<usize>,
) -> Result<()> {
    let owners = scan_story_owners(xml, root_kind)?;
    let mut anchors = Vec::new();
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("text-box owner scan failed: {error}")))?;
        if let Event::Start(element) = event {
            if story_namespace(&namespace) == StoryNamespace::WordDrawing
                && element.local_name().as_ref() == b"anchor"
                && scope.contains(&before)
            {
                let end = story_element_end(xml, before)?;
                let contained = owners
                    .iter()
                    .filter(|owner| {
                        owner.kind == StoryKind::TextBox
                            && before <= owner.full.start
                            && owner.full.end <= end
                    })
                    .collect::<Vec<_>>();
                // Nested anchors own their own boxes. This anchor's direct box
                // is the first selected owner in its captured source span.
                anchors.push((
                    drawing_capture_key(&xml[before..end])?,
                    contained.first().map(|owner| owner.owner_index),
                    false,
                ));
            }
        } else if matches!(event, Event::Eof) {
            break;
        }
        buffer.clear();
    }
    prepare_physical_story_projection(body, &mut anchors)
}

// These are read/layout clones. No raw drawing scan or whole-body clone is
// needed to make already-admitted revision fields available to semantic readers.
pub(super) fn prepare_semantic_paragraph_projection(value: &mut CT_P) -> Result<()> {
    for (_, _, revision) in &mut value.revisions {
        if let Some(paragraph) = revision.physical_content_paragraph_mut()? {
            prepare_semantic_paragraph_projection(paragraph)?;
        }
    }
    for (_, _, _, control) in &mut value.content_controls {
        prepare_semantic_control_projection(control)?;
    }
    for run in &mut value.runs {
        prepare_semantic_run_projection(run)?;
    }
    Ok(())
}

fn prepare_semantic_run_projection(value: &mut CT_R) -> Result<()> {
    for drawing in value
        .content
        .iter_mut()
        .filter_map(|content| {
            if let RunContent::Drawing(drawing) = content {
                Some(drawing)
            } else {
                None
            }
        })
        .chain(value.alt_drawings.iter_mut())
    {
        let shape = drawing
            .anchor
            .as_mut()
            .and_then(|anchor| anchor.shape.as_mut());
        if let Some(shape) = shape {
            if let Some(body) = &mut shape.text_body {
                prepare_semantic_body_projection(&mut body.content)?;
            } else {
                for paragraph in &mut shape.text {
                    prepare_semantic_paragraph_projection(paragraph)?;
                }
            }
        }
    }
    Ok(())
}

fn prepare_semantic_control_projection(value: &mut CT_Sdt) -> Result<()> {
    for (_, revision) in value.physical_revisions_mut() {
        if let Some(paragraph) = revision.physical_content_paragraph_mut()? {
            prepare_semantic_paragraph_projection(paragraph)?;
        }
    }
    for child in &mut value.content {
        match child {
            SdtContent::Paragraph(p) => prepare_semantic_paragraph_projection(p)?,
            SdtContent::Table(t) => prepare_semantic_table_projection(t)?,
            SdtContent::Row(r) => prepare_semantic_row_projection(r)?,
            SdtContent::Cell(c) => prepare_semantic_cell_projection(c)?,
            SdtContent::Run(r) => prepare_semantic_run_projection(r)?,
            SdtContent::ContentControl(c) => prepare_semantic_control_projection(c)?,
            SdtContent::RawXml(_) => {}
        }
    }
    Ok(())
}

fn prepare_semantic_table_projection(value: &mut CT_Tbl) -> Result<()> {
    for (_, _, control) in &mut value.content_controls {
        prepare_semantic_control_projection(control)?;
    }
    for row in &mut value.rows {
        prepare_semantic_row_projection(row)?;
    }
    Ok(())
}

fn prepare_semantic_row_projection(value: &mut CT_Row) -> Result<()> {
    for (_, _, control) in &mut value.content_controls {
        prepare_semantic_control_projection(control)?;
    }
    for cell in &mut value.cells {
        prepare_semantic_cell_projection(cell)?;
    }
    Ok(())
}

fn prepare_semantic_cell_projection(value: &mut CT_Tc) -> Result<()> {
    for content in &mut value.content {
        match content {
            CellContent::Paragraph(p) => prepare_semantic_paragraph_projection(p)?,
            CellContent::Table(t) => prepare_semantic_table_projection(t)?,
            CellContent::ContentControl(c) => prepare_semantic_control_projection(c)?,
        }
    }
    Ok(())
}

pub(crate) fn prepare_semantic_body_projection(values: &mut [BodyContent]) -> Result<()> {
    for value in values {
        match value {
            BodyContent::Paragraph(p) => prepare_semantic_paragraph_projection(p)?,
            BodyContent::Table(t) => prepare_semantic_table_projection(t)?,
            BodyContent::ContentControl(c) => prepare_semantic_control_projection(c)?,
            BodyContent::RawXml(_) => {}
        }
    }
    Ok(())
}

pub(crate) fn prepare_physical_story_projection(
    body: &mut oxoffice_docx_oxml::document::CT_Body,
    anchors: &mut [(Vec<u8>, Option<usize>, bool)],
) -> Result<()> {
    fn consume_raw(raw: &[u8], anchors: &mut [(Vec<u8>, Option<usize>, bool)]) -> Result<()> {
        let captured = drawing_capture_key(raw)?;
        let mut reader = quick_xml::Reader::from_reader(captured.as_slice());
        let mut buffer = Vec::new();
        loop {
            let before = reader.buffer_position() as usize;
            let event = reader
                .read_event_into(&mut buffer)
                .map_err(|error| Error::Other(error.to_string()))?;
            if let Event::Start(element) = &event
                && element.local_name().as_ref() == b"anchor"
            {
                let end = story_element_end(&captured, before)?;
                let key = &captured[before..end];
                if let Some((_, _, used)) = anchors
                    .iter_mut()
                    .find(|(source, _, used)| !*used && source == key)
                {
                    *used = true;
                }
            }
            if matches!(event, Event::Eof) {
                break;
            }
            buffer.clear();
        }
        Ok(())
    }
    fn drawing(
        value: &mut oxoffice_docx_oxml::drawing::CT_Drawing,
        anchors: &mut [(Vec<u8>, Option<usize>, bool)],
    ) -> Result<()> {
        let Some(anchor) = value.anchor.as_mut() else {
            return Ok(());
        };
        let Some(shape) = anchor.shape.as_mut() else {
            return Ok(());
        };
        shape.source_text_box_owner = if let Some(raw) = &anchor.raw_xml {
            let key = drawing_capture_key(raw)?;
            anchors
                .iter_mut()
                .find(|(source, _, used)| !*used && *source == key)
                .and_then(|(_, owner, used)| {
                    *used = true;
                    *owner
                })
        } else {
            None
        };
        shape_content(shape, anchors)
    }
    fn shape_content(
        shape: &mut oxoffice_docx_oxml::drawing::CT_Shape,
        anchors: &mut [(Vec<u8>, Option<usize>, bool)],
    ) -> Result<()> {
        if let Some(body) = &mut shape.text_body {
            body_content(&mut body.content, anchors)?;
        } else {
            for p in &mut shape.text {
                paragraph(p, anchors)?;
            }
        }
        Ok(())
    }
    fn selected_raw(
        raw: &[u8],
        selected: &mut oxoffice_docx_oxml::drawing::CT_Drawing,
        anchors: &mut [(Vec<u8>, Option<usize>, bool)],
    ) -> Result<()> {
        let selected_key = selected
            .anchor
            .as_ref()
            .and_then(|anchor| anchor.raw_xml.as_deref())
            .map(drawing_capture_key)
            .transpose()?;
        let captured = drawing_capture_key(raw)?;
        let mut reader = quick_xml::Reader::from_reader(captured.as_slice());
        let mut buffer = Vec::new();
        let mut bound = false;
        loop {
            let before = reader.buffer_position() as usize;
            let event = reader
                .read_event_into(&mut buffer)
                .map_err(|error| Error::Other(error.to_string()))?;
            if let Event::Start(element) = &event
                && element.local_name().as_ref() == b"anchor"
            {
                let end = story_element_end(&captured, before)?;
                let key = &captured[before..end];
                let owner = anchors
                    .iter_mut()
                    .find(|(source, _, used)| !*used && source == key)
                    .and_then(|(_, owner, used)| {
                        *used = true;
                        *owner
                    });
                if !bound && owner.is_some() && selected_key.as_deref() == Some(key) {
                    if let Some(shape) = selected
                        .anchor
                        .as_mut()
                        .and_then(|anchor| anchor.shape.as_mut())
                    {
                        shape.source_text_box_owner = owner;
                        shape_content(shape, anchors)?;
                    }
                    bound = true;
                    let name = element.name().as_ref().to_vec();
                    reader
                        .read_to_end_into(quick_xml::name::QName(&name), &mut Vec::new())
                        .map_err(|error| Error::Other(error.to_string()))?;
                }
            }
            if matches!(event, Event::Eof) {
                break;
            }
            buffer.clear();
        }
        if !bound
            && let Some(shape) = selected
                .anchor
                .as_mut()
                .and_then(|anchor| anchor.shape.as_mut())
        {
            shape.source_text_box_owner = None;
        }
        Ok(())
    }
    fn run(value: &mut CT_R, anchors: &mut [(Vec<u8>, Option<usize>, bool)]) -> Result<()> {
        let properties = usize::from(value.properties.is_some());
        let mut selected_index = 0;
        let mut children = value
            .extra_xml
            .iter()
            .enumerate()
            .filter_map(|(index, _)| {
                let encoded = value.extra_xml_positions.get(index).copied();
                if encoded.is_some_and(CT_R::raw_child_is_root_attributes) {
                    return None;
                }
                let selected = if encoded.is_some_and(CT_R::raw_child_has_alternate_content_drawing)
                {
                    let index = selected_index;
                    selected_index += 1;
                    Some(index)
                } else {
                    None
                };
                Some((
                    encoded.map_or(properties + value.content.len(), CT_R::raw_child_position),
                    0u8,
                    index,
                    selected,
                ))
            })
            .chain(
                value
                    .content
                    .iter()
                    .enumerate()
                    .filter_map(|(index, content)| {
                        matches!(content, RunContent::Drawing(_)).then_some((
                            properties + index,
                            1u8,
                            index,
                            None,
                        ))
                    }),
            )
            .collect::<Vec<_>>();
        children.sort_unstable();
        for (_, kind, index, selected) in children {
            if kind == 1 {
                if let RunContent::Drawing(value) = &mut value.content[index] {
                    drawing(value, anchors)?;
                }
            } else if let Some(selected) =
                selected.and_then(|index| value.alt_drawings.get_mut(index))
            {
                selected_raw(&value.extra_xml[index], selected, anchors)?;
            } else {
                consume_raw(&value.extra_xml[index], anchors)?;
            }
        }
        Ok(())
    }
    fn paragraph(value: &mut CT_P, anchors: &mut [(Vec<u8>, Option<usize>, bool)]) -> Result<()> {
        // Consume every physical revision owner before selecting accepted runs.
        // Projection metadata is installed in place, never serialized/reparsed.
        for boundary in 0..=value.runs.len() {
            let mut owners = value
                .content_controls
                .iter()
                .enumerate()
                .filter(|(_, (at, _, _, _))| *at == boundary)
                .map(|(index, (_, slot, _, _))| (1u8, *slot, 0u8, index))
                .chain(
                    value
                        .revisions
                        .iter()
                        .enumerate()
                        .filter(|(_, (at, _, _))| *at == boundary)
                        .map(|(index, (_, slot, _))| {
                            let (order, slot) = if let Some(link_index) =
                                oxoffice_docx_oxml::text::hyperlink_revision_index(*slot)
                            {
                                match value.hyperlinks.get(link_index) {
                                    Some(link) if link.preserved_raw_before.is_some() => {
                                        (1, link.preserved_raw_before.unwrap())
                                    }
                                    Some(link) if boundary == link.run_end => (0, 0),
                                    _ => (2, 0),
                                }
                            } else {
                                (1, *slot)
                            };
                            (order, slot, 1u8, index)
                        }),
                )
                .collect::<Vec<_>>();
            for (raw_slot, (index, (at, _))) in value
                .extra_xml
                .iter()
                .enumerate()
                .filter(|(_, (at, _))| *at == boundary)
                .enumerate()
            {
                let projected = value
                    .revisions
                    .iter()
                    .any(|(position, slot, _)| position == at && *slot == raw_slot)
                    || value.hyperlinks.iter().any(|link| {
                        link.run_start == boundary && link.preserved_raw_before == Some(raw_slot)
                    });
                if !projected {
                    owners.push((1, raw_slot, 2, index));
                }
            }
            owners.sort_unstable();
            for (_, _, kind, index) in owners {
                if kind == 1 {
                    if let Some(p) = value.revisions[index].2.physical_content_paragraph_mut()? {
                        paragraph(p, anchors)?;
                    }
                } else if kind == 0 {
                    control(&mut value.content_controls[index].3, anchors)?;
                } else {
                    consume_raw(&value.extra_xml[index].1, anchors)?;
                }
            }
            if let Some(value) = value.runs.get_mut(boundary) {
                run(value, anchors)?;
            }
        }
        Ok(())
    }
    fn control(value: &mut CT_Sdt, anchors: &mut [(Vec<u8>, Option<usize>, bool)]) -> Result<()> {
        for boundary in 0..=value.content.len() {
            for (_, revision) in value
                .physical_revisions_mut()
                .iter_mut()
                .filter(|(at, _)| *at == boundary)
            {
                if let Some(p) = revision.physical_content_paragraph_mut()? {
                    paragraph(p, anchors)?;
                }
            }
            if let Some(content) = value.content.get_mut(boundary) {
                match content {
                    SdtContent::Paragraph(p) => paragraph(p, anchors)?,
                    SdtContent::Run(value) => run(value, anchors)?,
                    SdtContent::Table(value) => table(value, anchors)?,
                    SdtContent::Row(value) => row(value, anchors)?,
                    SdtContent::Cell(value) => cell(value, anchors)?,
                    SdtContent::ContentControl(value) => control(value, anchors)?,
                    SdtContent::RawXml(raw) => consume_raw(raw, anchors)?,
                }
            }
        }
        Ok(())
    }
    fn table(value: &mut CT_Tbl, anchors: &mut [(Vec<u8>, Option<usize>, bool)]) -> Result<()> {
        for boundary in 0..=value.rows.len() {
            for (_, _, value) in value
                .content_controls
                .iter_mut()
                .filter(|(at, _, _)| *at == boundary)
            {
                control(value, anchors)?;
            }
            if let Some(value) = value.rows.get_mut(boundary) {
                row(value, anchors)?;
            }
        }
        Ok(())
    }
    fn row(value: &mut CT_Row, anchors: &mut [(Vec<u8>, Option<usize>, bool)]) -> Result<()> {
        for boundary in 0..=value.cells.len() {
            for (_, _, value) in value
                .content_controls
                .iter_mut()
                .filter(|(at, _, _)| *at == boundary)
            {
                control(value, anchors)?;
            }
            if let Some(value) = value.cells.get_mut(boundary) {
                cell(value, anchors)?;
            }
        }
        Ok(())
    }
    fn cell(value: &mut CT_Tc, anchors: &mut [(Vec<u8>, Option<usize>, bool)]) -> Result<()> {
        for content in &mut value.content {
            match content {
                CellContent::Paragraph(value) => paragraph(value, anchors)?,
                CellContent::Table(value) => table(value, anchors)?,
                CellContent::ContentControl(value) => control(value, anchors)?,
            }
        }
        Ok(())
    }
    fn body_content(
        values: &mut [BodyContent],
        anchors: &mut [(Vec<u8>, Option<usize>, bool)],
    ) -> Result<()> {
        for value in values {
            match value {
                BodyContent::Paragraph(value) => paragraph(value, anchors)?,
                BodyContent::Table(value) => table(value, anchors)?,
                BodyContent::ContentControl(value) => control(value, anchors)?,
                BodyContent::RawXml(raw) => consume_raw(raw, anchors)?,
            }
        }
        Ok(())
    }
    let mut candidate = body.clone();
    body_content(&mut candidate.content, anchors)?;
    *body = candidate;
    Ok(())
}

pub(super) fn structural_fingerprint(xml: &[u8]) -> Result<u64> {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut hash = OFFSET;
    for byte in xml {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    Ok(hash)
}

fn typed_complex_field_candidate_is_admitted(
    xml: &[u8],
    paragraph: &Range<usize>,
    candidate: &StoryItemSpan,
    inherited_word_prefixes: &[String],
) -> Result<bool> {
    let paragraph_xml = &xml[paragraph.clone()];
    let mut reader = quick_xml::Reader::from_reader(paragraph_xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let paragraph_local_name = loop {
        match reader.read_event_into(&mut buffer).map_err(|error| {
            Error::Other(format!("typed story field paragraph scan failed: {error}"))
        })? {
            Event::Start(element) => {
                break element.local_name().as_ref().to_vec();
            }
            Event::Eof => {
                return Err(Error::Other(
                    "typed story field paragraph has no root element".to_owned(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    };
    let scanned = &xml[candidate.scan.clone()];
    let marker_start = candidate
        .full
        .start
        .checked_sub(candidate.scan.start)
        .filter(|start| *start <= scanned.len())
        .ok_or_else(|| {
            Error::Other("complex story field starts outside its run span".to_owned())
        })?;
    let marker_end = candidate
        .full
        .end
        .checked_sub(candidate.scan.start)
        .filter(|end| marker_start <= *end && *end <= scanned.len())
        .ok_or_else(|| Error::Other("complex story field ends outside its run span".to_owned()))?;
    let mut run_reader = quick_xml::Reader::from_reader(scanned);
    run_reader.config_mut().trim_text(false);
    let mut run_buffer = Vec::new();
    let mut run_opening_end = None;
    let mut run_closing_start = None;
    loop {
        let before = run_reader.buffer_position() as usize;
        let event = run_reader
            .read_event_into(&mut run_buffer)
            .map_err(|error| Error::Other(format!("typed story field run scan failed: {error}")))?;
        let after = run_reader.buffer_position() as usize;
        if run_opening_end.is_none() && matches!(event, Event::Start(_)) {
            run_opening_end = Some(after);
        }
        if after == scanned.len() && matches!(event, Event::End(_)) {
            run_closing_start = Some(before);
            break;
        }
        if matches!(event, Event::Eof) {
            break;
        }
        run_buffer.clear();
    }
    let Some(run_opening_end) = run_opening_end.filter(|end| *end <= marker_start) else {
        return Ok(false);
    };
    let Some(run_closing_start) = run_closing_start.filter(|start| marker_end <= *start) else {
        return Ok(false);
    };
    let mut isolated = Vec::new();
    isolated.extend_from_slice(&scanned[..run_opening_end]);
    isolated.extend_from_slice(&scanned[marker_start..marker_end]);
    isolated.extend_from_slice(&scanned[run_closing_start..]);
    let mut scope = story_namespace_scope_at(xml, candidate.scan.start)?;
    let word_prefix = story_word_prefix_for_scope(&scope);
    scope
        .entry(word_prefix.clone())
        .or_insert_with(|| WORD_NAMESPACE.to_owned());
    let mut paragraph_name = Vec::new();
    if !word_prefix.is_empty() {
        paragraph_name.extend_from_slice(word_prefix.as_bytes());
        paragraph_name.push(b':');
    }
    paragraph_name.extend_from_slice(&paragraph_local_name);
    let mut fragment = Vec::new();
    fragment.push(b'<');
    fragment.extend_from_slice(&paragraph_name);
    for (prefix, namespace) in scope {
        fragment.extend_from_slice(b" xmlns");
        if !prefix.is_empty() {
            fragment.push(b':');
            fragment.extend_from_slice(prefix.as_bytes());
        }
        fragment.extend_from_slice(b"=\"");
        fragment.extend_from_slice(quick_xml::escape::escape(&namespace).as_bytes());
        fragment.push(b'"');
    }
    fragment.push(b'>');
    fragment.extend_from_slice(&isolated);
    fragment.extend_from_slice(b"</");
    fragment.extend_from_slice(&paragraph_name);
    fragment.push(b'>');
    let Ok(sources) = CT_P::story_complex_field_sources(&fragment, inherited_word_prefixes) else {
        return Ok(false);
    };
    Ok(sources.iter().any(|source| source == &isolated))
}

pub(super) fn scan_story_control_paragraphs(
    xml: &[u8],
    control: &StoryItemSpan,
) -> Result<Vec<Range<usize>>> {
    let empty_word_prefixes = Arc::new(Vec::new());
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut stack = Vec::<XmlElementFrame>::new();
    let mut paragraphs = Vec::new();
    let mut active = false;
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = match reader.read_resolved_event_into(&mut buffer) {
            Ok(event) => event,
            Err(error) => {
                return Err(Error::Other(format!(
                    "control paragraph scan failed: {error}"
                )));
            }
        };
        let namespace = story_namespace(&namespace);
        let after = reader.buffer_position() as usize;
        if !active {
            if before != control.full.start {
                if matches!(event, Event::Eof) {
                    break;
                }
                buffer.clear();
                continue;
            }
            active = true;
        }
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let local = story_frame_local_name(element.local_name().as_ref());
                let empty = matches!(event, Event::Empty(_));
                let is_word = namespace == StoryNamespace::Word;
                let root = stack.is_empty();
                if root && (!is_word || local.as_ref() != b"sdt") {
                    return Err(Error::Other(
                        "control paragraph source has no qualified selected control".into(),
                    ));
                }
                // Only this control's direct paragraphs belong to this inventory.
                // Descendants of another control cannot contribute, typed or raw.
                let opaque = !root
                    && ((is_word && local.as_ref() == b"sdt")
                        || opaque_story_event(
                            &mut stack, namespace, &local, element, empty, xml, before, after,
                        )?);
                let direct_content = stack.last().is_some_and(|parent| {
                    !parent.opaque
                        && parent.namespace == StoryNamespace::Word
                        && parent.local_name.as_ref() == b"sdtContent"
                });
                if !opaque && is_word && local.as_ref() == b"p" && direct_content {
                    paragraphs.push(
                        before..if empty {
                            after
                        } else {
                            story_element_end(xml, before)?
                        },
                    );
                }
                if !empty {
                    let inherited = if root {
                        Arc::new(
                            story_namespace_scope_at(xml, before)?
                                .into_iter()
                                .filter_map(|(prefix, uri)| {
                                    (uri == WORD_NAMESPACE).then_some(prefix)
                                })
                                .collect::<Vec<_>>(),
                        )
                    } else {
                        stack.last().map_or_else(
                            || Arc::clone(&empty_word_prefixes),
                            |parent| Arc::clone(&parent.word_prefixes),
                        )
                    };
                    let word_prefixes = story_word_prefixes_at(element, &inherited, is_word)?;
                    let sdt_context = if root {
                        control.sdt_context
                    } else {
                        story_element_sdt_context(&stack, namespace, &local)
                    };
                    stack.push(XmlElementFrame {
                        namespace,
                        local_name: local,
                        is_word,
                        full_start: before,
                        owner_kind: None,
                        item_kind: None,
                        direct_owner_child: false,
                        sdt_context,
                        word_prefixes,
                        opaque,
                        typed_sdt_content_seen: false,
                    });
                }
            }
            Event::End(_) => {
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        if after >= control.full.end {
            break;
        }
        buffer.clear();
    }
    Ok(paragraphs)
}

pub(super) fn scan_story_control_paragraphs_with_scope(
    xml: &[u8],
    control: &StoryItemSpan,
    scope: &BTreeMap<String, String>,
) -> Result<Vec<Range<usize>>> {
    let (closed, added) = scoped_story_fragment(xml, control.full.clone(), scope)?;
    let mut local_control = control.clone();
    local_control.full = 0..closed.len();
    let map = |position: usize| -> Result<usize> {
        let mapped = if position == 0 {
            Some(control.full.start)
        } else {
            position
                .checked_sub(added)
                .and_then(|offset| control.full.start.checked_add(offset))
        };
        mapped
            .filter(|offset| *offset <= control.full.end)
            .ok_or_else(|| {
                Error::Other("control paragraph excerpt has an invalid source offset".to_owned())
            })
    };
    scan_story_control_paragraphs(&closed, &local_control)?
        .into_iter()
        .map(|paragraph| Ok(map(paragraph.start)?..map(paragraph.end)?))
        .collect()
}

pub(super) fn scan_story_items(
    xml: &[u8],
    owner: &StoryOwnerSpan,
    mut template_field_ranges: Option<&mut Vec<Range<usize>>>,
) -> Result<Vec<StoryItemSpan>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut stack: Vec<XmlElementFrame> = Vec::new();
    let mut items = Vec::new();
    let mut complex_fields = Vec::<ComplexFieldScan>::new();
    let mut template_open_fields = Vec::<usize>::new();
    let mut paragraphs = Vec::<(Range<usize>, Arc<Vec<String>>)>::new();
    let mut active = false;
    let mut buffer = Vec::new();
    let mut ancestor_prefixes = Arc::new(Vec::new());
    let mut ancestor_frames: Vec<Arc<Vec<String>>> = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = match reader.read_resolved_event_into(&mut buffer) {
            Ok(event) => event,
            Err(error) => {
                return Err(Error::Other(format!("story item scan failed: {error}")));
            }
        };
        let namespace_kind = story_namespace(&namespace);
        let is_word_namespace = namespace_kind == StoryNamespace::Word;
        drop(namespace);
        let after = reader.buffer_position() as usize;
        if !active {
            if before != owner.full.start {
                match &event {
                    Event::Start(element) => {
                        let next = story_word_prefixes_at(element, &ancestor_prefixes, false)?;
                        ancestor_frames.push(std::mem::replace(&mut ancestor_prefixes, next));
                    }
                    Event::End(_) => {
                        if let Some(previous) = ancestor_frames.pop() {
                            ancestor_prefixes = previous;
                        }
                    }
                    Event::Eof => break,
                    _ => {}
                }
                buffer.clear();
                continue;
            }
            active = true;
        }
        match event {
            Event::Start(element) => {
                let is_word = is_word_namespace;
                let local_name = story_frame_local_name(element.local_name().as_ref());
                let sdt_context =
                    if stack.is_empty() && is_word && local_name.as_ref() == b"sdtContent" {
                        Some(StorySdtContext::Block)
                    } else {
                        story_element_sdt_context(&stack, namespace_kind, &local_name)
                    };
                let depth = stack.len();
                let in_nested_owner = stack.iter().skip(1).any(|frame| {
                    frame.is_word && matches!(frame.local_name.as_ref(), b"tc" | b"txbxContent")
                });
                let child_opaque = in_nested_owner
                    || opaque_story_event(
                        &mut stack,
                        namespace_kind,
                        &local_name,
                        &element,
                        false,
                        xml,
                        before,
                        after,
                    )?;
                let item_kind = if depth == 1 && !in_nested_owner {
                    Some(if child_opaque {
                        StoryItemKind::PreservedNode
                    } else {
                        direct_story_item_kind(is_word, &local_name)
                    })
                } else if !in_nested_owner && !child_opaque && is_word {
                    match local_name.as_ref() {
                        b"sdt" => Some(StoryItemKind::ContentControl),
                        b"fldSimple" => Some(StoryItemKind::Field),
                        // A drawing in `mc:Choice` is read only for the text
                        // boxes it holds, and lists no item of its own.
                        b"drawing"
                            if stack.last().is_some_and(|parent| {
                                parent.namespace == StoryNamespace::MarkupCompatibility
                            }) =>
                        {
                            None
                        }
                        b"drawing" | b"pict" => Some(StoryItemKind::Drawing),
                        _ => None,
                    }
                } else {
                    None
                };
                let opaque = child_opaque || item_kind == Some(StoryItemKind::PreservedNode);
                if !opaque
                    && !in_nested_owner
                    && is_word
                    && local_name.as_ref() == b"fldChar"
                    && complex_field_scan_context(&stack)
                    && let Some(run) = story_enclosing_run_source(xml, &stack)?
                {
                    let marker = complex_field_marker(&reader, &element)?;
                    if let Some(ranges) = template_field_ranges.as_mut() {
                        match marker {
                            Some(ComplexFieldMarker::Begin) => template_open_fields.push(before),
                            Some(ComplexFieldMarker::End) => ranges.push(
                                template_open_fields.pop().unwrap_or(owner.full.start)..after,
                            ),
                            Some(ComplexFieldMarker::Separate)
                                if template_open_fields.is_empty() =>
                            {
                                ranges.push(owner.full.start..owner.full.end)
                            }
                            _ => {}
                        }
                    }
                    scan_complex_field_marker(
                        &mut complex_fields,
                        &mut items,
                        marker,
                        before,
                        after,
                        run,
                    );
                }
                let inherited = stack
                    .last()
                    .map(|frame| &frame.word_prefixes)
                    .unwrap_or(&ancestor_prefixes);
                let word_prefixes = story_word_prefixes_at(&element, inherited, is_word_namespace)?;
                stack.push(XmlElementFrame {
                    namespace: namespace_kind,
                    local_name,
                    is_word,
                    full_start: before,
                    owner_kind: None,
                    item_kind,
                    direct_owner_child: depth == 1 && !in_nested_owner,
                    sdt_context,
                    word_prefixes,
                    opaque,
                    typed_sdt_content_seen: false,
                });
            }
            Event::Empty(element) => {
                let is_word = is_word_namespace;
                let local_name = element.local_name();
                let sdt_context =
                    story_element_sdt_context(&stack, namespace_kind, local_name.as_ref());
                let depth = stack.len();
                let in_nested_owner = stack.iter().skip(1).any(|frame| {
                    frame.is_word && matches!(frame.local_name.as_ref(), b"tc" | b"txbxContent")
                });
                let opaque = in_nested_owner
                    || opaque_story_event(
                        &mut stack,
                        namespace_kind,
                        local_name.as_ref(),
                        &element,
                        true,
                        xml,
                        before,
                        after,
                    )?;
                let kind = if depth == 1 && !in_nested_owner {
                    Some(if opaque {
                        StoryItemKind::PreservedNode
                    } else {
                        direct_story_item_kind(is_word, local_name.as_ref())
                    })
                } else if !in_nested_owner && !opaque && is_word {
                    match local_name.as_ref() {
                        b"sdt" => Some(StoryItemKind::ContentControl),
                        b"fldSimple" => Some(StoryItemKind::Field),
                        b"drawing" | b"pict" => Some(StoryItemKind::Drawing),
                        _ => None,
                    }
                } else {
                    None
                };
                if !opaque
                    && !in_nested_owner
                    && is_word
                    && local_name.as_ref() == b"fldChar"
                    && complex_field_scan_context(&stack)
                    && let Some(run) = story_enclosing_run_source(xml, &stack)?
                {
                    let marker = complex_field_marker(&reader, &element)?;
                    if let Some(ranges) = template_field_ranges.as_mut() {
                        match marker {
                            Some(ComplexFieldMarker::Begin) => template_open_fields.push(before),
                            Some(ComplexFieldMarker::End) => ranges.push(
                                template_open_fields.pop().unwrap_or(owner.full.start)..after,
                            ),
                            Some(ComplexFieldMarker::Separate)
                                if template_open_fields.is_empty() =>
                            {
                                ranges.push(owner.full.start..owner.full.end)
                            }
                            _ => {}
                        }
                    }
                    scan_complex_field_marker(
                        &mut complex_fields,
                        &mut items,
                        marker,
                        before,
                        after,
                        run,
                    );
                }
                if let Some(kind) = kind {
                    items.push(StoryItemSpan {
                        kind,
                        full: before..after,
                        scan: before..after,
                        direct_owner_child: depth == 1 && !in_nested_owner,
                        complex_field: false,
                        complex_ancestors: Vec::new(),
                        sdt_context,
                    });
                }
            }
            Event::End(_) => {
                let Some(frame) = stack.pop() else {
                    return Err(Error::Other(
                        "story item XML contains an unmatched closing element".to_owned(),
                    ));
                };
                if !frame.opaque
                    && frame.namespace == StoryNamespace::Word
                    && frame.local_name.as_ref() == b"p"
                {
                    paragraphs.push((frame.full_start..after, frame.word_prefixes.clone()));
                    complex_fields.clear();
                }
                if let Some(kind) = frame.item_kind {
                    items.push(StoryItemSpan {
                        kind,
                        full: frame.full_start..after,
                        scan: frame.full_start..after,
                        direct_owner_child: frame.direct_owner_child,
                        complex_field: false,
                        complex_ancestors: Vec::new(),
                        sdt_context: frame.sdt_context,
                    });
                }
                if after == owner.full.end {
                    break;
                }
            }
            Event::Text(value) => {
                let decoded = value.decode().map_err(|error| {
                    Error::Other(format!("field instruction decode failed: {error}"))
                })?;
                let value = quick_xml::escape::unescape(&decoded).map_err(|error| {
                    Error::Other(format!("field instruction unescape failed: {error}"))
                })?;
                push_complex_field_instruction(&mut complex_fields, &stack, &value);
            }
            Event::CData(value) => {
                let value = value.decode().map_err(|error| {
                    Error::Other(format!("field instruction CDATA decode failed: {error}"))
                })?;
                push_complex_field_instruction(&mut complex_fields, &stack, &value);
            }
            Event::GeneralRef(value) => push_complex_field_instruction(
                &mut complex_fields,
                &stack,
                &oxoffice_core::xml_text::resolve_entity(&value),
            ),
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let mut typed_complex_admission = HashMap::<usize, bool>::new();
    if items.iter().any(|item| item.complex_field) {
        for (paragraph, word_prefixes) in &paragraphs {
            for candidate in items.iter().filter(|item| {
                item.complex_field
                    && paragraph.start <= item.scan.start
                    && item.scan.end <= paragraph.end
            }) {
                let admitted = typed_complex_field_candidate_is_admitted(
                    xml,
                    paragraph,
                    candidate,
                    word_prefixes,
                )?;
                typed_complex_admission.insert(candidate.full.start, admitted);
            }
        }
    }
    items.retain(|item| {
        !item.complex_field
            || (typed_complex_admission
                .get(&item.full.start)
                .copied()
                .unwrap_or(false)
                && item.complex_ancestors.iter().all(|ancestor| {
                    typed_complex_admission
                        .get(ancestor)
                        .copied()
                        .unwrap_or(false)
                }))
    });
    if let Some(ranges) = template_field_ranges {
        ranges.extend(
            template_open_fields
                .into_iter()
                .map(|start| start..owner.full.end),
        );
    }
    items.sort_by_key(|item| (item.full.start, item.full.end));
    items.dedup_by(|left, right| left.kind == right.kind && left.full == right.full);
    Ok(items)
}

pub(super) fn scan_story_item_links(
    xml: &[u8],
    item: &StoryItemSpan,
) -> Result<Vec<StoryLinkSpan>> {
    let empty_word_prefixes = Arc::new(Vec::new());
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut stack: Vec<XmlElementFrame> = Vec::new();
    let mut open_links = Vec::new();
    let mut links = Vec::new();
    let mut nested_owner_depth = 0usize;
    let mut active = false;
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("story hyperlink scan failed: {error}")))?;
        let namespace_kind = story_namespace(&namespace);
        let is_word_namespace = namespace_kind == StoryNamespace::Word;
        drop(namespace);
        let after = reader.buffer_position() as usize;
        if !active {
            if before != item.scan.start {
                if matches!(event, Event::Eof) {
                    break;
                }
                buffer.clear();
                continue;
            }
            active = true;
        }
        match event {
            Event::Start(element) => {
                let depth = stack.len();
                let local_name = story_frame_local_name(element.local_name().as_ref());
                let opaque = if stack.is_empty() && item.kind == StoryItemKind::ContentControl {
                    false
                } else {
                    opaque_story_event(
                        &mut stack,
                        namespace_kind,
                        &local_name,
                        &element,
                        false,
                        xml,
                        before,
                        after,
                    )?
                };
                if !opaque
                    && depth > 0
                    && is_word_namespace
                    && matches!(local_name.as_ref(), b"tc" | b"txbxContent" | b"sdt")
                {
                    nested_owner_depth += 1;
                }
                if !opaque
                    && nested_owner_depth == 0
                    && is_word_namespace
                    && local_name.as_ref() == b"hyperlink"
                {
                    let (rel_id, anchor) = story_hyperlink_attributes(&reader, &element)?;
                    open_links.push((before, rel_id, anchor));
                }
                let inherited = stack
                    .last()
                    .map_or(&empty_word_prefixes, |frame| &frame.word_prefixes);
                let word_prefixes = story_word_prefixes_at(&element, inherited, is_word_namespace)?;
                stack.push(XmlElementFrame {
                    namespace: namespace_kind,
                    local_name,
                    is_word: is_word_namespace,
                    full_start: before,
                    owner_kind: None,
                    item_kind: None,
                    direct_owner_child: false,
                    sdt_context: if depth == 0 && item.kind == StoryItemKind::ContentControl {
                        item.sdt_context
                    } else {
                        story_element_sdt_context(
                            &stack,
                            namespace_kind,
                            element.local_name().as_ref(),
                        )
                    },
                    word_prefixes,
                    opaque,
                    typed_sdt_content_seen: false,
                });
            }
            Event::Empty(element) => {
                let local_name = element.local_name();
                let opaque = opaque_story_event(
                    &mut stack,
                    namespace_kind,
                    local_name.as_ref(),
                    &element,
                    true,
                    xml,
                    before,
                    after,
                )?;
                if !opaque
                    && nested_owner_depth == 0
                    && is_word_namespace
                    && local_name.as_ref() == b"hyperlink"
                {
                    let (rel_id, anchor) = story_hyperlink_attributes(&reader, &element)?;
                    links.push(StoryLinkSpan {
                        full: before..after,
                        rel_id,
                        anchor,
                    });
                }
            }
            Event::End(_) => {
                let Some(frame) = stack.pop() else {
                    return Err(Error::Other(
                        "story hyperlink XML contains an unmatched closing element".to_owned(),
                    ));
                };
                if !frame.opaque
                    && nested_owner_depth == 0
                    && frame.namespace == StoryNamespace::Word
                    && frame.local_name.as_ref() == b"hyperlink"
                {
                    let (start, rel_id, anchor) = open_links.pop().ok_or_else(|| {
                        Error::Other("story hyperlink has no matching start".to_owned())
                    })?;
                    links.push(StoryLinkSpan {
                        full: start..after,
                        rel_id,
                        anchor,
                    });
                }
                if !frame.opaque
                    && !stack.is_empty()
                    && frame.namespace == StoryNamespace::Word
                    && matches!(frame.local_name.as_ref(), b"tc" | b"txbxContent" | b"sdt")
                {
                    nested_owner_depth = nested_owner_depth.saturating_sub(1);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        if after == item.scan.end {
            break;
        }
        buffer.clear();
    }
    if !open_links.is_empty() {
        return Err(Error::Other(
            "story hyperlink has no matching end".to_owned(),
        ));
    }
    Ok(links)
}
