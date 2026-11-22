//! Reading story XML: element frames, modeled children, story namespaces and comment ranges.

use super::*;

// Paired source events prove physical correspondence while permitting only
// namespace declarations to differ. Non-namespace source remains exact.
#[allow(clippy::type_complexity)] // Concrete physical map and changed namespace offsets form one proof.
pub(crate) fn comment_namespace_source_boundaries(
    canonical: &[u8],
    restored: &[u8],
) -> Result<Option<(BTreeMap<usize, usize>, Vec<usize>)>> {
    let mut left = NsReader::from_reader(canonical);
    let mut right = NsReader::from_reader(restored);
    let mut boundaries = BTreeMap::new();
    let mut changed_namespace_facts = Vec::new();
    loop {
        let left_start = left.buffer_position() as usize;
        let right_start = right.buffer_position() as usize;
        let a = left.read_event().map_err(|e| Error::Other(e.to_string()))?;
        let b = right
            .read_event()
            .map_err(|e| Error::Other(e.to_string()))?;
        let same = match (&a, &b) {
            (Event::Start(a), Event::Start(b)) | (Event::Empty(a), Event::Empty(b)) => {
                let attributes = |element: &BytesStart<'_>| -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
                    element
                        .attributes()
                        .filter_map(|attribute| match attribute {
                            Ok(attribute)
                                if attribute.key.as_ref() == b"xmlns"
                                    || attribute.key.as_ref().starts_with(b"xmlns:") =>
                            {
                                None
                            }
                            Ok(attribute) => Some(Ok((
                                attribute.key.as_ref().to_vec(),
                                attribute.value.as_ref().to_vec(),
                            ))),
                            Err(error) => Some(Err(Error::Other(error.to_string()))),
                        })
                        .collect()
                };
                let a_attributes = attributes(a)?;
                let b_attributes = attributes(b)?;
                let same_namespaces = left.resolver().resolve_element(a.name()).0
                    == right.resolver().resolve_element(b.name()).0
                    && a.attributes()
                        .filter_map(|attribute| attribute.ok())
                        .filter(|attribute| {
                            attribute.key.as_ref() != b"xmlns"
                                && !attribute.key.as_ref().starts_with(b"xmlns:")
                        })
                        .all(|attribute| {
                            left.resolver().resolve_attribute(attribute.key).0
                                == right.resolver().resolve_attribute(attribute.key).0
                        });
                if !same_namespaces {
                    changed_namespace_facts.push(left_start);
                }
                a.name() == b.name() && a_attributes == b_attributes
            }
            _ => a == b,
        };
        if !same {
            return Ok(None);
        }
        boundaries.insert(left_start, right_start);
        boundaries.insert(
            left.buffer_position() as usize,
            right.buffer_position() as usize,
        );
        if matches!(a, Event::Eof) {
            break;
        }
    }
    Ok(Some((boundaries, changed_namespace_facts)))
}

// Existing checked paragraph paths shared by discovery and rich projection.
// Neither consumer invents a second index axis. Every owner of one source and
// every control in it is scanned from its own excerpt, with each family of
// scopes inventoried in one pass, so no owner or control rereads the source
// from its start. One list per owner, in the order given.
#[allow(clippy::type_complexity)] // One located span list per owner.
pub(super) fn story_range_paragraph_spans(
    xml: &[u8],
    owners: &[(StoryId, StoryOwnerSpan)],
) -> Result<Vec<Vec<(ContentLocation, Range<usize>)>>> {
    let owner_scopes =
        story_namespace_scopes_at(xml, owners.iter().map(|(_, owner)| owner.full.start))?;
    let mut inventories = Vec::with_capacity(owners.len());
    for (_, owner) in owners {
        let scope = owner_scopes.get(&owner.full.start).ok_or_else(|| {
            Error::Other("story owner namespace scope was not inventoried".to_owned())
        })?;
        inventories.push(scan_story_items_with_scope(xml, owner, scope, None)?);
    }
    let control_scopes = story_namespace_scopes_at(
        xml,
        inventories
            .iter()
            .flatten()
            .filter(|item| item.kind == StoryItemKind::ContentControl)
            .map(|item| item.full.start),
    )?;
    story_range_paragraph_spans_from_inventory(xml, owners, &inventories, &control_scopes)
}

#[allow(clippy::type_complexity)] // Story-ordered paragraphs retain checked locations and source spans.
pub(super) fn story_range_paragraph_spans_from_inventory(
    xml: &[u8],
    owners: &[(StoryId, StoryOwnerSpan)],
    inventories: &[Vec<StoryItemSpan>],
    control_scopes: &HashMap<usize, BTreeMap<String, String>>,
) -> Result<Vec<Vec<(ContentLocation, Range<usize>)>>> {
    let mut spans = Vec::with_capacity(owners.len());
    for ((story, _), items) in owners.iter().zip(inventories) {
        let mut paragraphs = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let item_spans = match item.kind {
                StoryItemKind::Paragraph => vec![(vec![index], item.full.clone())],
                StoryItemKind::ContentControl => {
                    let scope = control_scopes.get(&item.full.start).ok_or_else(|| {
                        Error::Other(
                            "content control namespace scope was not inventoried".to_owned(),
                        )
                    })?;
                    scan_story_control_paragraphs_with_scope(xml, item, scope)?
                        .into_iter()
                        .enumerate()
                        .map(|(paragraph, span)| (vec![index, paragraph], span))
                        .collect()
                }
                _ => continue,
            };
            for (path, span) in item_spans {
                paragraphs.push((
                    ContentLocation::new(story.clone(), StoryItemKind::Paragraph, path),
                    span,
                ));
            }
        }
        spans.push(paragraphs);
    }
    Ok(spans)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum StoryNamespace {
    Word,
    Vml,
    Drawing,
    DrawingPicture,
    WordDrawing,
    WordShape,
    WordGroup,
    MarkupCompatibility,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum StorySdtContext {
    Block,
    Table,
    Row,
    Inline,
}

pub(super) fn story_frame_local_name(value: &[u8]) -> Cow<'static, [u8]> {
    match value {
        b"p" => Cow::Borrowed(b"p".as_slice()),
        b"r" => Cow::Borrowed(b"r".as_slice()),
        b"rPr" => Cow::Borrowed(b"rPr".as_slice()),
        b"t" => Cow::Borrowed(b"t".as_slice()),
        _ => Cow::Owned(value.to_vec()),
    }
}

pub(super) fn xml_decodes(xml: &[u8]) -> bool {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(element) | Event::Empty(element)) => {
                for attribute in element.attributes() {
                    let Ok(attribute) = attribute else {
                        return false;
                    };
                    if attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                        .is_err()
                        || attribute
                            .key
                            .as_ref()
                            .strip_prefix(b"xmlns:")
                            .is_some_and(|prefix| std::str::from_utf8(prefix).is_err())
                    {
                        return false;
                    }
                }
            }
            Ok(Event::Eof) => return true,
            Ok(_) => {}
            Err(_) => return false,
        }
        buffer.clear();
    }
}

pub(super) fn scoped_proof_marker(original: &[u8], restored: &[u8]) -> Result<String> {
    let mut identity = 0usize;
    loop {
        let marker = format!("rdocxScopedPositionProof{identity}");
        if !original
            .windows(marker.len())
            .any(|bytes| bytes == marker.as_bytes())
            && !restored
                .windows(marker.len())
                .any(|bytes| bytes == marker.as_bytes())
        {
            return Ok(marker);
        }
        identity = identity
            .checked_add(1)
            .ok_or_else(|| Error::Other("scoped source proof identity exhausted".into()))?;
    }
}

pub(super) fn scoped_proof_sentinel(selected_closed: &[u8], marker: &str) -> Result<String> {
    let mut reader = NsReader::from_reader(selected_closed);
    let (namespace, event) = reader
        .read_resolved_event()
        .map_err(|error| Error::Other(error.to_string()))?;
    let root = match event {
        Event::Start(element) | Event::Empty(element) => element,
        _ => return Err(Error::Other("scoped source proof has no root".into())),
    };
    if !word_element(&namespace) {
        return Err(Error::Other(
            "scoped source proof needs a qualified Word block".into(),
        ));
    }
    let paragraph =
        format!(r#"<w:p xmlns:w="{WORD_NAMESPACE}"><w:r><w:t>{marker}</w:t></w:r></w:p>"#);
    Ok(match root.local_name().as_ref() {
        b"p" => paragraph,
        b"tbl" => format!(
            r#"<w:tbl xmlns:w="{WORD_NAMESPACE}"><w:tr><w:tc>{paragraph}</w:tc></w:tr></w:tbl>"#
        ),
        b"tc" => format!(r#"<w:tc xmlns:w="{WORD_NAMESPACE}">{paragraph}</w:tc>"#),
        b"sdt" => format!(
            r#"<w:sdt xmlns:w="{WORD_NAMESPACE}"><w:sdtPr/><w:sdtContent>{paragraph}</w:sdtContent></w:sdt>"#
        ),
        _ => return Err(Error::Other("unsupported scoped source proof kind".into())),
    })
}

pub(super) fn word_element(namespace: &ResolveResult<'_>) -> bool {
    matches!(
        namespace,
        ResolveResult::Bound(Namespace(uri)) if *uri == WORD_NAMESPACE.as_bytes()
    )
}

pub(super) fn story_namespace(namespace: &ResolveResult<'_>) -> StoryNamespace {
    match namespace {
        ResolveResult::Bound(Namespace(uri)) => match *uri {
            uri if uri == WORD_NAMESPACE.as_bytes() => StoryNamespace::Word,
            b"urn:schemas-microsoft-com:vml" => StoryNamespace::Vml,
            b"http://schemas.openxmlformats.org/drawingml/2006/main" => StoryNamespace::Drawing,
            b"http://schemas.openxmlformats.org/drawingml/2006/picture" => {
                StoryNamespace::DrawingPicture
            }
            b"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" => {
                StoryNamespace::WordDrawing
            }
            b"http://schemas.microsoft.com/office/word/2010/wordprocessingShape" => {
                StoryNamespace::WordShape
            }
            b"http://schemas.microsoft.com/office/word/2010/wordprocessingGroup" => {
                StoryNamespace::WordGroup
            }
            b"http://schemas.openxmlformats.org/markup-compatibility/2006" => {
                StoryNamespace::MarkupCompatibility
            }
            _ => StoryNamespace::Other,
        },
        _ => StoryNamespace::Other,
    }
}

fn modeled_word_child(parent: &[u8], child: &[u8]) -> bool {
    match parent {
        b"document" => child == b"body",
        b"footnotes" => child == b"footnote",
        b"endnotes" => child == b"endnote",
        b"comments" => child == b"comment",
        b"body" | b"hdr" | b"ftr" | b"footnote" | b"endnote" | b"comment" | b"txbxContent" => {
            matches!(child, b"p" | b"tbl" | b"sdt" | b"sectPr")
        }
        b"tc" => matches!(child, b"tcPr" | b"p" | b"tbl" | b"sdt"),
        b"tbl" => matches!(child, b"tblPr" | b"tblGrid" | b"tr" | b"sdt"),
        b"tr" => matches!(child, b"trPr" | b"tc" | b"sdt"),
        b"p" => matches!(
            child,
            b"pPr"
                | b"r"
                | b"sdt"
                | b"hyperlink"
                | b"fldSimple"
                | b"ins"
                | b"del"
                | b"moveFrom"
                | b"moveTo"
                | b"bookmarkStart"
                | b"bookmarkEnd"
                | b"commentRangeStart"
                | b"commentRangeEnd"
                | b"proofErr"
        ),
        b"r" => matches!(
            child,
            b"rPr"
                | b"t"
                | b"delText"
                | b"instrText"
                | b"delInstrText"
                | b"fldChar"
                | b"drawing"
                | b"pict"
                | b"object"
                | b"tab"
                | b"br"
                | b"cr"
                | b"sym"
                | b"noBreakHyphen"
                | b"softHyphen"
                | b"ptab"
                | b"footnoteReference"
                | b"endnoteReference"
                | b"commentReference"
                | b"lastRenderedPageBreak"
        ),
        b"sdt" => matches!(child, b"sdtPr" | b"sdtEndPr" | b"sdtContent"),
        b"sdtContent" => false,
        b"hyperlink" => {
            matches!(
                child,
                b"r" | b"fldSimple" | b"ins" | b"del" | b"moveFrom" | b"moveTo"
            )
        }
        b"ins" | b"del" | b"moveFrom" | b"moveTo" => matches!(
            child,
            b"r" | b"fldSimple" | b"sdt" | b"hyperlink" | b"ins" | b"del" | b"moveFrom" | b"moveTo"
        ),
        b"fldSimple" => child == b"r",
        _ => false,
    }
}

pub(super) fn story_sdt_context(stack: &[XmlElementFrame]) -> Option<StorySdtContext> {
    let parent = stack.last()?;
    if parent.namespace != StoryNamespace::Word {
        return None;
    }
    match parent.local_name.as_ref() {
        b"body" | b"hdr" | b"ftr" | b"footnote" | b"endnote" | b"comment" | b"txbxContent"
        | b"tc" => Some(StorySdtContext::Block),
        b"tbl" => Some(StorySdtContext::Table),
        b"tr" => Some(StorySdtContext::Row),
        b"p" | b"hyperlink" | b"fldSimple" | b"ins" | b"del" | b"moveFrom" | b"moveTo" => {
            Some(StorySdtContext::Inline)
        }
        b"sdtContent" => parent.sdt_context,
        _ => None,
    }
}

pub(super) fn story_element_sdt_context(
    stack: &[XmlElementFrame],
    namespace: StoryNamespace,
    local_name: &[u8],
) -> Option<StorySdtContext> {
    if namespace != StoryNamespace::Word {
        return None;
    }
    match local_name {
        b"sdt" => story_sdt_context(stack),
        b"sdtContent" => stack.last().and_then(|parent| {
            (parent.namespace == StoryNamespace::Word && parent.local_name.as_ref() == b"sdt")
                .then_some(parent.sdt_context)
                .flatten()
        }),
        _ => None,
    }
}

fn modeled_sdt_content_child(context: Option<StorySdtContext>, child: &[u8]) -> bool {
    child == b"sdt"
        || match context {
            Some(StorySdtContext::Block) => matches!(child, b"p" | b"tbl"),
            Some(StorySdtContext::Table) => child == b"tr",
            Some(StorySdtContext::Row) => child == b"tc",
            Some(StorySdtContext::Inline) => {
                matches!(
                    child,
                    b"r" | b"fldSimple" | b"ins" | b"del" | b"moveFrom" | b"moveTo"
                )
            }
            None => false,
        }
}

pub(super) fn modeled_story_child(
    stack: &[XmlElementFrame],
    namespace: StoryNamespace,
    local_name: &[u8],
) -> bool {
    let Some(parent) = stack.last() else {
        return namespace == StoryNamespace::Word;
    };
    match (parent.namespace, namespace) {
        (StoryNamespace::Word, StoryNamespace::Word)
            if parent.local_name.as_ref() == b"sdtContent" =>
        {
            modeled_sdt_content_child(parent.sdt_context, local_name)
        }
        (StoryNamespace::Word, StoryNamespace::Word) => {
            modeled_word_child(&parent.local_name, local_name)
        }
        (StoryNamespace::Word, StoryNamespace::Vml) => {
            matches!(parent.local_name.as_ref(), b"pict" | b"object")
                && matches!(
                    local_name,
                    b"shape"
                        | b"group"
                        | b"rect"
                        | b"roundrect"
                        | b"oval"
                        | b"line"
                        | b"polyline"
                        | b"curve"
                        | b"arc"
                )
        }
        (StoryNamespace::Vml, StoryNamespace::Vml) => match parent.local_name.as_ref() {
            b"group" => matches!(
                local_name,
                b"shape"
                    | b"group"
                    | b"rect"
                    | b"roundrect"
                    | b"oval"
                    | b"line"
                    | b"polyline"
                    | b"curve"
                    | b"arc"
            ),
            b"shape" | b"rect" | b"roundrect" | b"oval" | b"line" | b"polyline" | b"curve"
            | b"arc" => local_name == b"textbox",
            _ => false,
        },
        (StoryNamespace::Vml, StoryNamespace::Word) => {
            parent.local_name.as_ref() == b"textbox" && local_name == b"txbxContent"
        }
        // Word writes a text box twice in a run, as DrawingML in `mc:Choice` and
        // as VML in `mc:Fallback`. One Choice is read, see
        // `modeled_story_event`, and the Fallback stays opaque, so the text box
        // is one story.
        (StoryNamespace::Word, StoryNamespace::MarkupCompatibility) => {
            parent.local_name.as_ref() == b"r" && local_name == b"AlternateContent"
        }
        (StoryNamespace::MarkupCompatibility, StoryNamespace::MarkupCompatibility) => {
            parent.local_name.as_ref() == b"AlternateContent" && local_name == b"Choice"
        }
        (StoryNamespace::MarkupCompatibility, StoryNamespace::Word) => {
            parent.local_name.as_ref() == b"Choice" && local_name == b"drawing"
        }
        (StoryNamespace::Word, StoryNamespace::WordDrawing) => {
            parent.local_name.as_ref() == b"drawing" && matches!(local_name, b"inline" | b"anchor")
        }
        (StoryNamespace::WordDrawing, StoryNamespace::Drawing) => {
            matches!(parent.local_name.as_ref(), b"inline" | b"anchor") && local_name == b"graphic"
        }
        (StoryNamespace::Drawing, StoryNamespace::Drawing) => match parent.local_name.as_ref() {
            b"graphic" => local_name == b"graphicData",
            _ => false,
        },
        (StoryNamespace::Drawing, StoryNamespace::WordShape) => {
            parent.local_name.as_ref() == b"graphicData" && local_name == b"wsp"
        }
        (StoryNamespace::Drawing, StoryNamespace::WordGroup) => {
            parent.local_name.as_ref() == b"graphicData" && local_name == b"wgp"
        }
        (StoryNamespace::WordGroup, StoryNamespace::WordGroup) => {
            matches!(parent.local_name.as_ref(), b"wgp" | b"grpSp") && local_name == b"grpSp"
        }
        (StoryNamespace::WordGroup, StoryNamespace::WordShape) => {
            matches!(parent.local_name.as_ref(), b"wgp" | b"grpSp") && local_name == b"wsp"
        }
        (StoryNamespace::WordShape, StoryNamespace::WordShape) => {
            parent.local_name.as_ref() == b"wsp" && local_name == b"txbx"
        }
        (StoryNamespace::WordShape, StoryNamespace::Word) => {
            parent.local_name.as_ref() == b"txbx" && local_name == b"txbxContent"
        }
        (StoryNamespace::Drawing, StoryNamespace::DrawingPicture) => {
            parent.local_name.as_ref() == b"graphicData" && local_name == b"pic"
        }
        _ => false,
    }
}

pub(super) fn field_instruction_has_name(instruction: &str) -> bool {
    story_field_instruction_has_name(instruction)
}

pub(super) fn story_word_prefixes_at(
    element: &BytesStart<'_>,
    inherited: &Arc<Vec<String>>,
    is_word_element: bool,
) -> Result<Arc<Vec<String>>> {
    let mut prefixes = Arc::clone(inherited);
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| {
            Error::Other(format!("story namespace declaration scan failed: {error}"))
        })?;
        let name = attribute.key.as_ref();
        let prefix = if name == b"xmlns" {
            b"".as_slice()
        } else if let Some(prefix) = name.strip_prefix(b"xmlns:") {
            prefix
        } else {
            continue;
        };
        let prefix = std::str::from_utf8(prefix)
            .map_err(|error| Error::Other(format!("story namespace prefix is invalid: {error}")))?
            .to_owned();
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
            .map_err(|error| {
                Error::Other(format!(
                    "story namespace declaration decode failed: {error}"
                ))
            })?
            .into_owned();
        let prefixes = Arc::make_mut(&mut prefixes);
        prefixes.retain(|candidate| {
            candidate != &prefix
                && candidate
                    .strip_prefix('\0')
                    .and_then(|rest| rest.split_once('\0'))
                    .is_none_or(|(bound, _)| bound != prefix)
        });
        prefixes.push(format!("\0{prefix}\0{value}"));
        if value == WORD_NAMESPACE {
            prefixes.push(prefix);
        }
    }
    if is_word_element {
        let name = element.name();
        let prefix = name
            .as_ref()
            .split(|byte| *byte == b':')
            .next()
            .filter(|_| name.as_ref().contains(&b':'))
            .unwrap_or(b"");
        let prefix = std::str::from_utf8(prefix)
            .map_err(|error| Error::Other(format!("story element prefix is invalid: {error}")))?;
        if !prefixes.iter().any(|value| value == prefix) {
            Arc::make_mut(&mut prefixes).push(prefix.to_owned());
        }
    }
    Ok(prefixes)
}

// Locate the selected reference's actual run without changing raw attributes.
pub(super) fn comment_reference_run_span(
    xml: &[u8],
    owner: &Range<usize>,
    id: i32,
) -> Result<Range<usize>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut stack = Vec::<(bool, Vec<u8>, usize)>::new();
    let mut found = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (ns, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(error.to_string()))?;
        let word = word_element(&ns);
        drop(ns);
        match &event {
            Event::Start(element) | Event::Empty(element) => {
                if word
                    && element.local_name().as_ref() == b"commentReference"
                    && owner.contains(&before)
                {
                    let mut selected = false;
                    for attr in element.attributes() {
                        let attr = attr.map_err(|error| Error::Other(error.to_string()))?;
                        let (ns, local) = reader.resolver().resolve_attribute(attr.key);
                        if word_element(&ns) && local.as_ref() == b"id" {
                            selected = attr
                                .decoded_and_normalized_value(
                                    XmlVersion::Implicit1_0,
                                    reader.decoder(),
                                )
                                .map_err(|error| Error::Other(error.to_string()))?
                                .parse::<i32>()
                                .ok()
                                == Some(id);
                        }
                    }
                    if selected {
                        let start = stack
                            .iter()
                            .rev()
                            .find(|(word, name, _)| *word && name == b"r")
                            .map(|(_, _, start)| *start)
                            .ok_or_else(|| {
                                Error::Other("comment reference has no source run".into())
                            })?;
                        found.push(start..story_element_end(xml, start)?);
                    }
                }
                if matches!(event, Event::Start(_)) {
                    stack.push((word, element.local_name().as_ref().to_vec(), before));
                }
            }
            Event::End(_) => {
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if found.len() != 1 {
        return Err(Error::Other(
            "comment move requires one qualified reference run".into(),
        ));
    }
    Ok(found.remove(0))
}

#[allow(clippy::type_complexity)] // Concrete edit spans, reference bytes and original run span.
pub(super) fn comment_move_source_edits(
    xml: &[u8],
    selected: &[Range<usize>],
) -> Result<(Vec<Range<usize>>, Vec<u8>, Range<usize>)> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut stack = Vec::<(bool, Vec<u8>, usize)>::new();
    let mut reference = None;
    let mut controls = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (ns, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(error.to_string()))?;
        let word = word_element(&ns);
        drop(ns);
        match &event {
            Event::Start(element) | Event::Empty(element) => {
                if selected.iter().any(|span| span.start == before) {
                    for (_, _, start) in stack
                        .iter()
                        .filter(|(word, name, _)| *word && name == b"sdt")
                    {
                        let span = *start..story_element_end(xml, *start)?;
                        if !controls.contains(&span) {
                            controls.push(span);
                        }
                    }
                    if word && element.local_name().as_ref() == b"commentReference" {
                        let start = stack
                            .iter()
                            .rev()
                            .find(|(word, name, _)| *word && name == b"r")
                            .map(|(_, _, start)| *start)
                            .ok_or_else(|| {
                                Error::Other("comment reference has no source run".into())
                            })?;
                        if reference
                            .replace(start..story_element_end(xml, start)?)
                            .is_some()
                        {
                            return Err(Error::Other(
                                "comment move has duplicate reference runs".into(),
                            ));
                        }
                    }
                }
                if matches!(event, Event::Start(_)) {
                    stack.push((word, element.local_name().as_ref().to_vec(), before));
                }
            }
            Event::End(_) => {
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let reference_span = reference
        .ok_or_else(|| Error::Other("comment move has no selected reference run".into()))?;
    let scope = story_namespace_scope_at(xml, reference_span.start)?;
    let reference = close_content_fragment_namespaces(&xml[reference_span.clone()], &scope)?;
    // Only the selected direct reference and run properties travel. Other
    // children retain their exact source sequence, even when visible text and
    // opaque producer payload share this run with the reference.
    let selected_reference = selected
        .iter()
        .find(|span| reference_span.contains(&span.start))
        .ok_or_else(|| Error::Other("comment move reference source was not selected".into()))?;
    let mut original_reader = quick_xml::Reader::from_reader(&xml[reference_span.clone()]);
    original_reader
        .read_event()
        .map_err(|error| Error::Other(error.to_string()))?;
    let original_open_end = original_reader.buffer_position() as usize;
    let mut reader = NsReader::from_reader(reference.as_slice());
    let mut depth = 0usize;
    let mut reference_found = false;
    let mut open_end = 0usize;
    let mut close_start = 0usize;
    let mut kept = Vec::new();
    let mut mixed = false;
    let mut properties_count = 0usize;
    loop {
        let before = reader.buffer_position() as usize;
        let (ns, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(error.to_string()))?;
        let word = word_element(&ns);
        drop(ns);
        let after = reader.buffer_position() as usize;
        match &event {
            Event::Start(element) | Event::Empty(element) => {
                if depth == 0 {
                    open_end = after;
                }
                if depth == 1 {
                    let end = if matches!(event, Event::Empty(_)) {
                        after
                    } else {
                        story_element_end(&reference, before)?
                    };
                    let is_selected = before
                        == selected_reference.start - reference_span.start + open_end
                            - original_open_end;
                    if is_selected && word && element.local_name().as_ref() == b"commentReference" {
                        if reference_found {
                            return Err(Error::Other("comment reference run is ambiguous".into()));
                        }
                        reference_found = true;
                        kept.push(before..end);
                    } else if word && element.local_name().as_ref() == b"rPr" {
                        properties_count += 1;
                        kept.push(before..end);
                    } else {
                        mixed = true;
                    }
                }
                if matches!(event, Event::Start(_)) {
                    depth += 1;
                }
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    close_start = before;
                }
            }
            Event::Text(text) if depth == 1 && !text.iter().all(u8::is_ascii_whitespace) => {
                mixed = true;
            }
            Event::GeneralRef(_) if depth == 1 => {
                mixed = true;
            }
            Event::Comment(_) | Event::PI(_) if depth == 1 => {
                mixed = true;
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if !reference_found || properties_count > 1 || close_start == 0 {
        return Err(Error::Other(
            "comment reference is not a uniquely separable direct run child".into(),
        ));
    }
    let mut transport = reference[..open_end].to_vec();
    for span in kept {
        transport.extend_from_slice(&reference[span]);
    }
    transport.extend_from_slice(&reference[close_start..]);
    let mut edits = selected
        .iter()
        .filter(|span| !reference_span.contains(&span.start))
        .cloned()
        .collect::<Vec<_>>();
    edits.push(if mixed {
        selected_reference.clone()
    } else {
        reference_span.clone()
    });
    controls.sort_by_key(|span| std::cmp::Reverse(span.start));
    for control in controls {
        let mut fragment = xml[control.clone()].to_vec();
        let mut contained = edits
            .iter()
            .filter(|span| control.start <= span.start && span.end <= control.end)
            .cloned()
            .collect::<Vec<_>>();
        contained.sort_by_key(|span| span.start);
        for span in contained.iter().rev() {
            fragment.drain(span.start - control.start..span.end - control.start);
        }
        if pure_empty_google_comment_control(
            &fragment,
            &story_namespace_scope_at(xml, control.start)?,
        )? {
            edits.retain(|span| !(control.start <= span.start && span.end <= control.end));
            edits.push(control);
        }
    }
    edits.sort_by_key(|span| span.start);
    if edits.windows(2).any(|pair| pair[0].end > pair[1].start) {
        return Err(Error::Other("comment move edits overlap".into()));
    }
    Ok((edits, transport, reference_span))
}

fn pure_empty_google_comment_control(xml: &[u8], scope: &BTreeMap<String, String>) -> Result<bool> {
    // An authored namespace declaration or any other root attribute is payload.
    let mut raw_reader = quick_xml::Reader::from_reader(xml);
    if !matches!(raw_reader.read_event(), Ok(Event::Start(ref root)) if root.attributes().next().is_none())
    {
        return Ok(false);
    }
    let closed = close_content_fragment_namespaces(xml, scope)?;
    let mut reader = NsReader::from_reader(closed.as_slice());
    let mut stack = Vec::<Vec<u8>>::new();
    let mut tag = false;
    let mut content = false;
    let mut buffer = Vec::new();
    loop {
        let (ns, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(error.to_string()))?;
        let word = word_element(&ns);
        drop(ns);
        match &event {
            Event::Start(element) | Event::Empty(element) => {
                let name = element.local_name().as_ref().to_vec();
                let parent = stack.last().map(Vec::as_slice);
                let allowed = word
                    && matches!(
                        (parent, name.as_slice()),
                        (None, b"sdt")
                            | (Some(b"sdt"), b"sdtPr" | b"sdtContent")
                            | (Some(b"sdtPr"), b"tag")
                            | (Some(b"sdtContent"), b"p")
                    );
                if !allowed {
                    return Ok(false);
                }
                if name == b"sdtContent" {
                    if content {
                        return Ok(false);
                    }
                    content = true;
                }
                for attr in element.attributes() {
                    let attr = attr.map_err(|error| Error::Other(error.to_string()))?;
                    if stack.is_empty()
                        && (attr.key.as_ref() == b"xmlns"
                            || attr.key.as_ref().starts_with(b"xmlns:"))
                    {
                        continue;
                    }
                    let (ns, local) = reader.resolver().resolve_attribute(attr.key);
                    if name != b"tag" || !word_element(&ns) || local.as_ref() != b"val" || tag {
                        return Ok(false);
                    }
                    tag = attr
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                        .map_err(|error| Error::Other(error.to_string()))?
                        .starts_with("goog_rdk_");
                    if !tag {
                        return Ok(false);
                    }
                }
                if matches!(event, Event::Start(_)) {
                    stack.push(name);
                }
            }
            Event::End(_) => {
                stack.pop();
            }
            Event::Text(text) if text.iter().all(u8::is_ascii_whitespace) => {}
            Event::Eof => break,
            _ => return Ok(false),
        }
        buffer.clear();
    }
    Ok(tag && content)
}

pub(super) fn story_element_end(xml: &[u8], start: usize) -> Result<usize> {
    let fragment = xml
        .get(start..)
        .ok_or_else(|| Error::Other("story element begins outside its source part".to_owned()))?;
    let mut reader = quick_xml::Reader::from_reader(fragment);
    reader.config_mut().trim_text(false);
    let mut depth = 0usize;
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("story element scan failed: {error}")))?
        {
            Event::Start(_) => depth += 1,
            Event::Empty(_) if depth == 0 => {
                return Ok(start + reader.buffer_position() as usize);
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Ok(start + reader.buffer_position() as usize);
                }
            }
            Event::Eof => {
                return Err(Error::Other(
                    "story element has no matching closing element".to_owned(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    }
}

// Most elements inherit an unchanged map. Stop at the first declaration, then
// let the original decoder apply the whole element to a full inherited copy.
// Stopping here preserves namespace-value errors before later attribute errors.
pub(super) fn changed_story_namespace_scope(
    inherited: &BTreeMap<String, String>,
    element: &BytesStart<'_>,
) -> Result<Option<BTreeMap<String, String>>> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| {
            Error::Other(format!("story namespace scope attribute failed: {error}"))
        })?;
        let name = attribute.key.as_ref();
        if name == b"xmlns" || name.starts_with(b"xmlns:") {
            let mut scope = inherited.clone();
            update_story_namespace_scope(&mut scope, element)?;
            return Ok(Some(scope));
        }
    }
    Ok(None)
}

/// A bookmark marker written between blocks of a story source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceBlockMarker {
    /// The offset of the marker element in the source.
    pub(crate) offset: usize,
    pub(crate) start: bool,
    pub(crate) id: i32,
    pub(crate) name: String,
    /// The start offset of the paragraph it is read as.
    pub(crate) paragraph: usize,
    /// Whether it leads that paragraph, or closes it.
    pub(crate) leading: bool,
}

/// The bookmark markers of a story source that sit between blocks, each
/// with the paragraph it is read as.
///
/// A flow is the element whose blocks one story reads: the body, a header
/// or footer, a note, a comment or a text box's content. A marker or a
/// paragraph belongs to a flow when only tables, rows, cells and block
/// content controls stand between them, as Word and Google Docs write
/// block-level bookmarks. A marker under anything else, a paragraph, an
/// `mc:AlternateContent` branch or a `w:customXml` element, is not read
/// here: either a paragraph scan reads it, or no story does.
///
/// Within its flow, a marker is read as the paragraph it wraps, as
/// `BlockBookmarks` reads it: a start leads the next paragraph, an end
/// closes the previous one, an end whose start waits since the last
/// paragraph leads the next one with it; the other side when there is no
/// such paragraph. A table cell's paragraphs are a story of their own, so
/// the paragraph, not the flow, tells which story reads a marker.
pub(crate) fn source_block_bookmarks(xml: &[u8]) -> Result<Vec<SourceBlockMarker>> {
    if !xml
        .windows(b"bookmark".len())
        .any(|window| window == b"bookmark")
    {
        return Ok(Vec::new());
    }
    // Each marker as (offset, start, id, name, flow).
    let mut markers = Vec::<(usize, bool, i32, String, usize)>::new();
    // The paragraph starts of each flow, in source order.
    let mut flows = HashMap::<usize, Vec<usize>>::new();
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    // Each open element: whether it is a Word element, its local name and
    // its start offset.
    let mut stack = Vec::<(bool, Vec<u8>, usize)>::new();
    let flow_of = |stack: &[(bool, Vec<u8>, usize)]| {
        for (word, name, start) in stack.iter().rev() {
            match (*word, name.as_slice()) {
                (true, b"tbl" | b"tr" | b"tc" | b"sdt" | b"sdtContent") => {}
                (
                    true,
                    b"body" | b"hdr" | b"ftr" | b"footnote" | b"endnote" | b"comment"
                    | b"txbxContent",
                ) => return Some(*start),
                _ => return None,
            }
        }
        None
    };
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("block bookmark scan failed: {error}")))?;
        let word = word_element(&namespace);
        drop(namespace);
        match &event {
            Event::Start(element) | Event::Empty(element) => {
                let local = element.local_name();
                let local = local.as_ref();
                if word && local == b"p" {
                    if let Some(flow) = flow_of(&stack) {
                        flows.entry(flow).or_default().push(before);
                    }
                } else if word
                    && matches!(local, b"bookmarkStart" | b"bookmarkEnd")
                    && let Some(flow) = flow_of(&stack)
                {
                    let mut id = None;
                    let mut name = String::new();
                    for attribute in element.attributes() {
                        let attribute =
                            attribute.map_err(|error| Error::Other(error.to_string()))?;
                        let (attribute_namespace, attribute_local) =
                            reader.resolver().resolve_attribute(attribute.key);
                        if !word_element(&attribute_namespace) {
                            continue;
                        }
                        let value = attribute
                            .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                            .map_err(|error| Error::Other(error.to_string()))?;
                        match attribute_local.as_ref() {
                            b"id" => id = value.trim().parse().ok(),
                            b"name" => name = value.into_owned(),
                            _ => {}
                        }
                    }
                    if let Some(id) = id {
                        markers.push((before, local == b"bookmarkStart", id, name, flow));
                    }
                }
                if let Event::Start(_) = &event {
                    stack.push((word, local.to_vec(), before));
                }
            }
            Event::End(_) => {
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let mut attributed = Vec::with_capacity(markers.len());
    // The starts waiting in each flow, since how many of its paragraphs.
    let mut waiting = HashMap::<usize, (usize, Vec<i32>)>::new();
    for (offset, start, id, name, flow) in markers {
        let Some(paragraphs) = flows.get(&flow) else {
            continue;
        };
        let after = paragraphs.partition_point(|paragraph| *paragraph < offset);
        let (since, starts) = waiting.entry(flow).or_default();
        if *since != after {
            *since = after;
            starts.clear();
        }
        let collapsed = !start && starts.contains(&id);
        if start {
            starts.push(id);
        }
        let next = paragraphs.get(after).copied();
        let previous = after.checked_sub(1).map(|at| paragraphs[at]);
        let (paragraph, leading) = match (start || collapsed, next, previous) {
            (true, Some(next), _) | (false, Some(next), None) => (next, true),
            (_, _, Some(previous)) => (previous, false),
            (_, None, None) => continue,
        };
        attributed.push(SourceBlockMarker {
            offset,
            start,
            id,
            name,
            paragraph,
            leading,
        });
    }
    Ok(attributed)
}
