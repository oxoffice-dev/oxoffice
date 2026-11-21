//! Finding complex fields and their spans in raw Word XML.

use super::*;

#[derive(Debug)]
struct DynamicFieldScan {
    instruction: String,
    field_start: usize,
    begin_paragraph: usize,
    begin_paragraph_start: usize,
    begin_run_start: usize,
    separator_paragraph: Option<usize>,
    separator_run_start: Option<usize>,
    result_start: Option<usize>,
    result_start_position: Option<TocRunPosition>,
    start_paragraph_name: Option<String>,
    start_paragraph_namespaces: BTreeMap<String, String>,
    separator_wrapper_names: Vec<String>,
    instruction_runs: Vec<DynamicInstructionRun>,
}

#[derive(Debug)]
pub(super) struct DynamicHyperlinkPlan {
    revision_orders: Vec<TocRawOrder>,
    next_revision: usize,
    preserved_raw: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TypedBlockOwner {
    Document,
    Body,
    Table,
    Row,
    Cell,
    ContentControl(LegacyBlockOwner),
    Content(LegacyBlockOwner),
    Paragraph,
}

const MAX_DYNAMIC_REVISION_NESTING_DEPTH: usize = 32;

pub(super) fn dynamic_namespace_bindings(
    element: &BytesStart<'_>,
    elements: &[DynamicXmlElement],
) -> Result<(BTreeMap<String, String>, BTreeMap<String, String>)> {
    let inherited = elements
        .last()
        .map_or_else(BTreeMap::new, |parent| parent.namespace_bindings.clone());
    let mut bindings = inherited.clone();
    for attribute in element.attributes() {
        let attribute = attribute
            .map_err(|error| Error::Other(format!("invalid XML namespace attribute: {error}")))?;
        let key = attribute.key.as_ref();
        let prefix = if key == b"xmlns" {
            Some(String::new())
        } else {
            key.strip_prefix(b"xmlns:")
                .map(|prefix| String::from_utf8_lossy(prefix).into_owned())
        };
        if let Some(prefix) = prefix {
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                .map_err(|error| Error::Other(format!("invalid XML namespace attribute: {error}")))?
                .into_owned();
            if value.is_empty() {
                bindings.remove(&prefix);
            } else {
                bindings.insert(prefix, value);
            }
        }
    }
    Ok((bindings, inherited))
}

#[derive(Clone, Copy)]
enum DynamicContentControlOwner {
    Block(LegacyBlockOwner),
    Inline,
}

fn dynamic_element_end(xml: &[u8], start: usize) -> Result<usize> {
    let mut reader = quick_xml::Reader::from_reader(&xml[start..]);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    loop {
        match reader.read_event_into(&mut buffer).map_err(|error| {
            Error::Other(format!("invalid table of contents XML element: {error}"))
        })? {
            Event::Start(_) => depth += 1,
            Event::Empty(_) if depth == 0 => {
                return Ok(start + reader.buffer_position() as usize);
            }
            Event::End(_) => {
                depth = depth.checked_sub(1).ok_or_else(|| {
                    Error::Other("table of contents XML element has an unmatched end".to_owned())
                })?;
                if depth == 0 {
                    return Ok(start + reader.buffer_position() as usize);
                }
            }
            Event::Eof => {
                return Err(Error::Other(
                    "table of contents XML element is unclosed".to_owned(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    }
}

pub(crate) fn xml_fragment_with_namespaces(
    raw: &[u8],
    bindings: &BTreeMap<String, String>,
    description: &str,
) -> Result<Vec<u8>> {
    let mut reader = quick_xml::Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let (insertion, local_namespaces) = match reader
        .read_event_into(&mut buffer)
        .map_err(|error| Error::Other(format!("invalid {description}: {error}")))?
    {
        Event::Start(start) | Event::Empty(start) => {
            let mut local_namespaces = HashSet::new();
            for attribute in start.attributes() {
                let attribute = attribute
                    .map_err(|error| Error::Other(format!("invalid {description}: {error}")))?;
                let key = attribute.key.as_ref();
                if key == b"xmlns" {
                    local_namespaces.insert(String::new());
                } else if let Some(prefix) = key.strip_prefix(b"xmlns:") {
                    local_namespaces.insert(String::from_utf8_lossy(prefix).into_owned());
                }
            }
            let tag_end = reader.buffer_position() as usize;
            let insertion = if tag_end >= 2 && raw[tag_end - 2] == b'/' {
                tag_end - 2
            } else {
                tag_end - 1
            };
            (insertion, local_namespaces)
        }
        _ => return Err(Error::Other(format!("{description} has no start tag"))),
    };
    let mut output = Vec::with_capacity(raw.len() + bindings.len() * 32);
    output.extend_from_slice(&raw[..insertion]);
    for (prefix, namespace) in bindings {
        if prefix == "xml" || local_namespaces.contains(prefix) {
            continue;
        }
        if prefix.is_empty() {
            output.extend_from_slice(b" xmlns=\"");
        } else {
            output.extend_from_slice(b" xmlns:");
            output.extend_from_slice(prefix.as_bytes());
            output.extend_from_slice(b"=\"");
        }
        output.extend_from_slice(xml_escape_attribute(namespace).as_bytes());
        output.push(b'"');
    }
    output.extend_from_slice(&raw[insertion..]);
    Ok(output)
}

fn dynamic_content_control_is_typed(
    xml: &[u8],
    start: usize,
    bindings: &BTreeMap<String, String>,
    owner: DynamicContentControlOwner,
) -> Result<bool> {
    let end = dynamic_element_end(xml, start)?;
    let control = xml_fragment_with_namespaces(
        &xml[start..end],
        bindings,
        "table of contents content control",
    )?;
    let mut candidate = Vec::with_capacity(control.len() + 256);
    candidate.extend_from_slice(format!(r#"<w:document xmlns:w="{W_NS}"><w:body>"#).as_bytes());
    let (prefix, suffix): (&[u8], &[u8]) = match owner {
        DynamicContentControlOwner::Block(LegacyBlockOwner::Root) => (b"", b""),
        DynamicContentControlOwner::Block(LegacyBlockOwner::Table) => {
            (b"<w:tbl>", b"<w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>")
        }
        DynamicContentControlOwner::Block(LegacyBlockOwner::Row) => {
            (b"<w:tbl><w:tr>", b"<w:tc><w:p/></w:tc></w:tr></w:tbl>")
        }
        DynamicContentControlOwner::Block(LegacyBlockOwner::Cell) => {
            (b"<w:tbl><w:tr><w:tc>", b"<w:p/></w:tc></w:tr></w:tbl>")
        }
        DynamicContentControlOwner::Inline => (b"<w:p>", b"</w:p>"),
    };
    candidate.extend_from_slice(prefix);
    candidate.extend_from_slice(&control);
    candidate.extend_from_slice(suffix);
    candidate.extend_from_slice(b"</w:body></w:document>");
    let Ok(document) = CT_Document::from_xml(&candidate) else {
        return Ok(false);
    };
    let accepted = match owner {
        DynamicContentControlOwner::Block(LegacyBlockOwner::Root) => matches!(
            document.body.content.first(),
            Some(BodyContent::ContentControl(_))
        ),
        DynamicContentControlOwner::Block(LegacyBlockOwner::Table) => document
            .body
            .tables()
            .next()
            .is_some_and(|table| table.content_controls.len() == 1),
        DynamicContentControlOwner::Block(LegacyBlockOwner::Row) => document
            .body
            .tables()
            .next()
            .and_then(|table| table.rows.first())
            .is_some_and(|row| row.content_controls.len() == 1),
        DynamicContentControlOwner::Block(LegacyBlockOwner::Cell) => document
            .body
            .tables()
            .next()
            .and_then(|table| table.rows.first())
            .and_then(|row| row.cells.first())
            .is_some_and(|cell| {
                matches!(cell.content.first(), Some(CellContent::ContentControl(_)))
            }),
        DynamicContentControlOwner::Inline => document
            .body
            .paragraphs()
            .next()
            .is_some_and(|paragraph| paragraph.content_controls.len() == 1),
    };
    Ok(accepted)
}

pub(super) fn validate_dynamic_content_control(
    xml: &[u8],
    start: usize,
    word: bool,
    local: &[u8],
    bindings: &BTreeMap<String, String>,
    typed_block_owner: &mut Option<TypedBlockOwner>,
    is_typed_inline_owner: &mut bool,
) -> Result<()> {
    if !word || local != b"sdt" {
        return Ok(());
    }
    let owner = match *typed_block_owner {
        Some(TypedBlockOwner::ContentControl(owner)) => {
            Some(DynamicContentControlOwner::Block(owner))
        }
        _ if *is_typed_inline_owner => Some(DynamicContentControlOwner::Inline),
        _ => None,
    };
    if let Some(owner) = owner
        && !dynamic_content_control_is_typed(xml, start, bindings, owner)?
    {
        *typed_block_owner = None;
        *is_typed_inline_owner = false;
    }
    Ok(())
}

fn is_dynamic_revision_element(local: &[u8]) -> bool {
    matches!(
        local,
        b"ins"
            | b"del"
            | b"moveFrom"
            | b"moveTo"
            | b"rPrChange"
            | b"pPrChange"
            | b"tblPrChange"
            | b"sectPrChange"
    )
}

pub(super) fn invalidate_overdeep_revision_owner(
    word: bool,
    local: &[u8],
    elements: &mut [DynamicXmlElement],
) -> Option<usize> {
    if !word
        || !is_dynamic_revision_element(local)
        || elements
            .iter()
            .filter(|element| element.is_word && is_dynamic_revision_element(&element.local_name))
            .count()
            < MAX_DYNAMIC_REVISION_NESTING_DEPTH
    {
        return None;
    }
    let owner = elements.iter().position(|element| {
        element.is_typed_inline_owner && matches!(element.local_name.as_slice(), b"ins" | b"moveTo")
    })?;
    let start = elements[owner].start;
    for element in &mut elements[owner..] {
        element.is_typed_inline_owner = false;
    }
    Some(start)
}

pub(super) fn mark_typed_sdt_content(
    elements: &mut [DynamicXmlElement],
    local: &[u8],
    typed_block_owner: Option<TypedBlockOwner>,
    is_typed_inline_owner: bool,
) {
    if local == b"sdtContent"
        && (matches!(typed_block_owner, Some(TypedBlockOwner::Content(_))) || is_typed_inline_owner)
        && let Some(parent) = elements.last_mut()
    {
        parent.sdt_content_seen = true;
    }
}

pub(super) fn scan_dynamic_table_spans(
    xml: &[u8],
    generated: DynamicOwnerPolicy,
) -> Result<Vec<DynamicTocSpan>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut elements = Vec::<DynamicXmlElement>::new();
    let mut fields = Vec::<DynamicFieldScan>::new();
    let mut spans = Vec::<DynamicTocSpan>::new();
    let mut paragraph_count = 0usize;
    let mut paragraph_run_boundaries = Vec::<usize>::new();
    let mut paragraph_nested_run_orders = Vec::<HashMap<(usize, TocRawOrder), usize>>::new();
    let mut paragraph_raw_before = Vec::<usize>::new();
    let mut instruction_depth = None;

    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid table of contents XML: {error}")))?;
        let word = namespace_is_word(&namespace);
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                let local = local_name(element.name().as_ref()).to_vec();
                if let Some(start) = invalidate_overdeep_revision_owner(word, &local, &mut elements)
                {
                    fields.retain(|field| field.field_start < start);
                    spans.retain(|span| span.field_start < start);
                    instruction_depth = None;
                }
                let (namespace_bindings, inherited_namespaces) =
                    dynamic_namespace_bindings(&element, &elements)?;
                let mut typed_block_owner = classify_typed_block_owner(word, &local, &elements);
                let is_typed_paragraph = typed_block_owner == Some(TypedBlockOwner::Paragraph);
                let paragraph = if is_typed_paragraph {
                    let index = paragraph_count;
                    paragraph_count += 1;
                    paragraph_run_boundaries.push(0);
                    paragraph_nested_run_orders.push(HashMap::new());
                    paragraph_raw_before.push(0);
                    Some(index)
                } else {
                    elements.last().and_then(|element| element.paragraph)
                };
                let mut is_typed_inline_owner = typed_inline_owner(
                    &element,
                    reader.resolver(),
                    word,
                    &local,
                    &elements,
                    paragraph,
                )?;
                validate_dynamic_content_control(
                    xml,
                    before,
                    word,
                    &local,
                    &namespace_bindings,
                    &mut typed_block_owner,
                    &mut is_typed_inline_owner,
                )?;
                let revision_depth = if is_typed_inline_owner {
                    elements.last().map_or(0, |parent| parent.revision_depth)
                        + usize::from(matches!(local.as_slice(), b"ins" | b"moveTo"))
                } else {
                    0
                };
                let modeled_simple_field = if word && local == b"fldSimple" {
                    resolved_element_attribute(
                        &element,
                        reader.resolver(),
                        b"instr",
                        AttributeNamespace::Word,
                    )?
                    .is_some_and(|(_, instruction)| {
                        !Field::new(&instruction, "").instruction.name.is_empty()
                    })
                } else {
                    false
                };
                let run_position = dynamic_toc_run_position(
                    word,
                    &local,
                    is_typed_inline_owner,
                    modeled_simple_field,
                    &mut elements,
                    paragraph,
                    &mut paragraph_run_boundaries,
                    &mut paragraph_nested_run_orders,
                    &mut paragraph_raw_before,
                );
                let direct_paragraph_child = direct_typed_paragraph_parent(&elements, paragraph);
                let hyperlink_plan = if word && local == b"hyperlink" && direct_paragraph_child {
                    Some(dynamic_hyperlink_plan(
                        xml,
                        before,
                        &namespace_bindings,
                        paragraph
                            .and_then(|index| paragraph_raw_before.get(index).copied())
                            .ok_or_else(|| {
                                Error::Other(
                                    "table of contents hyperlink has no paragraph position"
                                        .to_owned(),
                                )
                            })?,
                    )?)
                } else {
                    None
                };
                advance_direct_paragraph_raw_child(
                    word,
                    &local,
                    is_typed_inline_owner,
                    modeled_simple_field,
                    direct_paragraph_child,
                    paragraph,
                    hyperlink_plan.as_ref(),
                    &mut paragraph_raw_before,
                );
                if word && local == b"fldChar" && direct_word_run_parent(&elements, paragraph) {
                    update_dynamic_field_stack(
                        &element,
                        reader.resolver(),
                        before,
                        paragraph,
                        &elements,
                        &mut fields,
                        &mut spans,
                        generated,
                    )?;
                }
                if word
                    && local == b"instrText"
                    && !fields.is_empty()
                    && direct_word_run_parent(&elements, paragraph)
                {
                    instruction_depth = Some(elements.len());
                }
                mark_typed_sdt_content(
                    &mut elements,
                    &local,
                    typed_block_owner,
                    is_typed_inline_owner,
                );
                elements.push(DynamicXmlElement {
                    local_name: local,
                    qualified_name: String::from_utf8_lossy(element.name().as_ref()).into_owned(),
                    typed_block_owner,
                    is_word: word,
                    is_typed_paragraph,
                    is_typed_inline_owner,
                    revision_depth,
                    sdt_content_seen: false,
                    namespace_bindings,
                    inherited_namespaces,
                    run_position,
                    hyperlink_plan,
                    start: before,
                    start_tag_end: after,
                    paragraph,
                });
            }
            Event::Empty(element) => {
                let local = local_name(element.name().as_ref()).to_vec();
                // Empty content is a retained raw slot, not the typed Start slot.
                let word = word && &local[..] != b"sdtContent";
                if let Some(start) = invalidate_overdeep_revision_owner(word, &local, &mut elements)
                {
                    fields.retain(|field| field.field_start < start);
                    spans.retain(|span| span.field_start < start);
                    instruction_depth = None;
                }
                let typed_block_owner = classify_typed_block_owner(word, &local, &elements);
                let paragraph = if typed_block_owner == Some(TypedBlockOwner::Paragraph) {
                    let index = paragraph_count;
                    paragraph_count += 1;
                    paragraph_run_boundaries.push(0);
                    paragraph_nested_run_orders.push(HashMap::new());
                    paragraph_raw_before.push(0);
                    Some(index)
                } else {
                    elements.last().and_then(|element| element.paragraph)
                };
                let is_typed_inline_owner = typed_inline_owner(
                    &element,
                    reader.resolver(),
                    word,
                    &local,
                    &elements,
                    paragraph,
                )?;
                let (namespace_bindings, inherited_namespaces) =
                    dynamic_namespace_bindings(&element, &elements)?;
                let modeled_simple_field = word
                    && local == b"fldSimple"
                    && resolved_element_attribute(
                        &element,
                        reader.resolver(),
                        b"instr",
                        AttributeNamespace::Word,
                    )?
                    .is_some_and(|(_, instruction)| {
                        !Field::new(&instruction, "").instruction.name.is_empty()
                    });
                let run_position = dynamic_toc_run_position(
                    word,
                    &local,
                    is_typed_inline_owner,
                    modeled_simple_field,
                    &mut elements,
                    paragraph,
                    &mut paragraph_run_boundaries,
                    &mut paragraph_nested_run_orders,
                    &mut paragraph_raw_before,
                );
                let direct_paragraph_child = direct_typed_paragraph_parent(&elements, paragraph);
                advance_direct_paragraph_raw_child(
                    word,
                    &local,
                    is_typed_inline_owner,
                    modeled_simple_field,
                    direct_paragraph_child,
                    paragraph,
                    None,
                    &mut paragraph_raw_before,
                );
                mark_typed_sdt_content(
                    &mut elements,
                    &local,
                    typed_block_owner,
                    is_typed_inline_owner,
                );
                if generated != DynamicOwnerPolicy::Toc && modeled_simple_field {
                    let closed = DynamicXmlElement {
                        local_name: local.clone(),
                        qualified_name: String::from_utf8_lossy(element.name().as_ref())
                            .into_owned(),
                        typed_block_owner,
                        is_word: word,
                        is_typed_paragraph: false,
                        is_typed_inline_owner,
                        revision_depth: 0,
                        sdt_content_seen: false,
                        namespace_bindings,
                        inherited_namespaces,
                        run_position,
                        hyperlink_plan: None,
                        start: before,
                        start_tag_end: after,
                        paragraph,
                    };
                    if let Some(span) =
                        generated_simple_span(xml, &closed, &elements, after, after, generated)?
                    {
                        spans.push(span);
                    }
                }
                if word
                    && matches_local_name(element.name().as_ref(), b"fldChar")
                    && direct_word_run_parent(&elements, paragraph)
                {
                    update_dynamic_field_stack(
                        &element,
                        reader.resolver(),
                        before,
                        paragraph,
                        &elements,
                        &mut fields,
                        &mut spans,
                        generated,
                    )?;
                }
            }
            Event::Text(text) if instruction_depth.is_some() => {
                if let Some(field) = fields.last_mut()
                    && field.separator_paragraph.is_none()
                {
                    let decoded = text.decode().map_err(|error| {
                        Error::Other(format!("invalid table of contents instruction: {error}"))
                    })?;
                    let unescaped = quick_xml::escape::unescape(&decoded).map_err(|error| {
                        Error::Other(format!("invalid table of contents instruction: {error}"))
                    })?;
                    field.instruction.push_str(&unescaped);
                }
            }
            Event::GeneralRef(_)
                if generated == DynamicOwnerPolicy::Bibliography && instruction_depth.is_some() =>
            {
                if let Some(field) = fields.last_mut()
                    && field.separator_paragraph.is_none()
                {
                    let reference = std::str::from_utf8(&xml[before..after]).map_err(|error| {
                        Error::Other(format!(
                            "invalid bibliography instruction reference: {error}"
                        ))
                    })?;
                    let decoded = quick_xml::escape::unescape(reference).map_err(|error| {
                        Error::Other(format!(
                            "invalid bibliography instruction reference: {error}"
                        ))
                    })?;
                    field.instruction.push_str(&decoded);
                }
            }
            Event::CData(text) if instruction_depth.is_some() => {
                if let Some(field) = fields.last_mut()
                    && field.separator_paragraph.is_none()
                {
                    field.instruction.push_str(&text.decode().map_err(|error| {
                        Error::Other(format!("invalid table of contents instruction: {error}"))
                    })?);
                }
            }
            Event::Comment(_) | Event::PI(_) => {
                if let Some(paragraph) = elements.last().and_then(|element| {
                    element
                        .is_typed_paragraph
                        .then_some(element.paragraph)
                        .flatten()
                }) && let Some(raw_before) = paragraph_raw_before.get_mut(paragraph)
                {
                    *raw_before += 1;
                }
            }
            Event::End(element) => {
                let Some(closed) = elements.pop() else {
                    return Err(Error::Other(
                        "table of contents XML has an unmatched end element".to_owned(),
                    ));
                };
                if generated != DynamicOwnerPolicy::Toc
                    && let Some(span) =
                        generated_simple_span(xml, &closed, &elements, before, after, generated)?
                {
                    spans.push(span);
                }
                if closed
                    .hyperlink_plan
                    .as_ref()
                    .is_some_and(|plan| plan.preserved_raw)
                    && let Some(paragraph) = closed.paragraph
                    && let Some(raw_before) = paragraph_raw_before.get_mut(paragraph)
                {
                    *raw_before += 1;
                }
                if word && matches_local_name(element.name().as_ref(), b"instrText") {
                    instruction_depth = None;
                }
                if word && matches_local_name(element.name().as_ref(), b"r") {
                    for field in &mut fields {
                        if closed.is_typed_inline_owner && field.result_start.is_none() {
                            field.instruction_runs.push(DynamicInstructionRun {
                                start: closed.start,
                                end: after,
                                inherited_namespaces: closed.inherited_namespaces.clone(),
                            });
                        }
                        if field.separator_run_start == Some(closed.start)
                            && field.result_start.is_none()
                        {
                            field.result_start = Some(after);
                        }
                    }
                    if let Some(span) = spans
                        .iter_mut()
                        .rev()
                        .find(|span| span.result_end == closed.start && span.end_run_end == 0)
                    {
                        span.end_run_end = after;
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if !elements.is_empty() {
        return Err(Error::Other(
            "table of contents XML has an unclosed element".to_owned(),
        ));
    }
    if fields.iter().any(|field| {
        generated_table_opcode(
            &Field::new(&field.instruction, "").instruction.name,
            generated,
        )
    }) {
        return Err(Error::Other(
            "table of contents field is missing its end marker".to_owned(),
        ));
    }
    spans.sort_by_key(|span| span.field_start);
    if spans
        .windows(2)
        .any(|pair| pair[1].field_start < pair[0].field_end)
    {
        return Err(Error::Other(
            "nested table of contents fields have ambiguous ownership".to_owned(),
        ));
    }
    let paragraphs = toc_paragraph_insertions(xml)?;
    for span in &mut spans {
        span.end_paragraph_content_start = paragraphs
            .get(span.end_paragraph)
            .map(|paragraph| paragraph.content_start)
            .ok_or_else(|| {
                Error::Other(
                    "table of contents end paragraph was not found in package XML".to_owned(),
                )
            })?;
    }
    Ok(spans)
}

pub(super) fn collect_simple_toc_diagnostics(xml: &[u8]) -> Result<Vec<(usize, String)>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut elements = Vec::<DynamicXmlElement>::new();
    let mut simple_toc_starts = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid table of contents XML: {error}")))?;
        let word = namespace_is_word(&namespace);
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                let local = local_name(element.name().as_ref()).to_vec();
                if let Some(start) = invalidate_overdeep_revision_owner(word, &local, &mut elements)
                {
                    simple_toc_starts.retain(|field_start| *field_start < start);
                }
                let (namespace_bindings, inherited_namespaces) =
                    dynamic_namespace_bindings(&element, &elements)?;
                let mut typed_block_owner = classify_typed_block_owner(word, &local, &elements);
                let is_typed_paragraph = typed_block_owner == Some(TypedBlockOwner::Paragraph);
                let paragraph = if is_typed_paragraph {
                    Some(0)
                } else {
                    elements.last().and_then(|element| element.paragraph)
                };
                let mut is_typed_inline_owner = typed_inline_owner(
                    &element,
                    reader.resolver(),
                    word,
                    &local,
                    &elements,
                    paragraph,
                )?;
                validate_dynamic_content_control(
                    xml,
                    before,
                    word,
                    &local,
                    &namespace_bindings,
                    &mut typed_block_owner,
                    &mut is_typed_inline_owner,
                )?;
                let revision_depth = if is_typed_inline_owner {
                    elements.last().map_or(0, |parent| parent.revision_depth)
                        + usize::from(matches!(local.as_slice(), b"ins" | b"moveTo"))
                } else {
                    0
                };
                if word
                    && local == b"fldSimple"
                    && paragraph.is_some()
                    && accepted_simple_toc_parent(&elements, paragraph)
                    && resolved_element_attribute(
                        &element,
                        reader.resolver(),
                        b"instr",
                        AttributeNamespace::Word,
                    )?
                    .is_some_and(|(_, instruction)| {
                        Field::new(&instruction, "").instruction.name == "TOC"
                    })
                {
                    simple_toc_starts.push(before);
                }
                mark_typed_sdt_content(
                    &mut elements,
                    &local,
                    typed_block_owner,
                    is_typed_inline_owner,
                );
                elements.push(DynamicXmlElement {
                    local_name: local,
                    qualified_name: String::from_utf8_lossy(element.name().as_ref()).into_owned(),
                    typed_block_owner,
                    is_word: word,
                    is_typed_paragraph,
                    is_typed_inline_owner,
                    revision_depth,
                    sdt_content_seen: false,
                    namespace_bindings,
                    inherited_namespaces,
                    run_position: None,
                    hyperlink_plan: None,
                    start: before,
                    start_tag_end: after,
                    paragraph,
                });
            }
            Event::Empty(element) => {
                let local = local_name(element.name().as_ref()).to_vec();
                // Empty content is a retained raw slot, not the typed Start slot.
                let word = word && &local[..] != b"sdtContent";
                if let Some(start) = invalidate_overdeep_revision_owner(word, &local, &mut elements)
                {
                    simple_toc_starts.retain(|field_start| *field_start < start);
                }
                let paragraph = elements.last().and_then(|element| element.paragraph);
                let typed_block_owner = classify_typed_block_owner(word, &local, &elements);
                let is_typed_inline_owner = typed_inline_owner(
                    &element,
                    reader.resolver(),
                    word,
                    &local,
                    &elements,
                    paragraph,
                )?;
                mark_typed_sdt_content(
                    &mut elements,
                    &local,
                    typed_block_owner,
                    is_typed_inline_owner,
                );
                if word
                    && matches_local_name(element.name().as_ref(), b"fldSimple")
                    && elements
                        .last()
                        .and_then(|element| element.paragraph)
                        .is_some()
                    && accepted_simple_toc_parent(
                        &elements,
                        elements.last().and_then(|element| element.paragraph),
                    )
                    && resolved_element_attribute(
                        &element,
                        reader.resolver(),
                        b"instr",
                        AttributeNamespace::Word,
                    )?
                    .is_some_and(|(_, instruction)| {
                        Field::new(&instruction, "").instruction.name == "TOC"
                    })
                {
                    simple_toc_starts.push(before);
                }
            }
            Event::End(_) => {
                elements.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(simple_toc_starts
        .into_iter()
        .map(|offset| {
            (
                offset,
                "simple table of contents fields are not rebuilt, stored display retained"
                    .to_owned(),
            )
        })
        .collect())
}

pub(super) fn classify_typed_block_owner(
    word: bool,
    local: &[u8],
    elements: &[DynamicXmlElement],
) -> Option<TypedBlockOwner> {
    if !word {
        return None;
    }
    let parent = elements
        .last()
        .and_then(|element| element.typed_block_owner);
    let content_owner = match parent {
        Some(TypedBlockOwner::Body | TypedBlockOwner::Content(LegacyBlockOwner::Root)) => {
            Some(LegacyBlockOwner::Root)
        }
        Some(TypedBlockOwner::Table | TypedBlockOwner::Content(LegacyBlockOwner::Table)) => {
            Some(LegacyBlockOwner::Table)
        }
        Some(TypedBlockOwner::Row | TypedBlockOwner::Content(LegacyBlockOwner::Row)) => {
            Some(LegacyBlockOwner::Row)
        }
        Some(TypedBlockOwner::Cell | TypedBlockOwner::Content(LegacyBlockOwner::Cell)) => {
            Some(LegacyBlockOwner::Cell)
        }
        _ => None,
    };
    match local {
        b"document" if elements.is_empty() => Some(TypedBlockOwner::Document),
        b"body" if parent == Some(TypedBlockOwner::Document) => Some(TypedBlockOwner::Body),
        b"p" if matches!(
            content_owner,
            Some(LegacyBlockOwner::Root | LegacyBlockOwner::Cell)
        ) =>
        {
            Some(TypedBlockOwner::Paragraph)
        }
        b"tbl"
            if matches!(
                content_owner,
                Some(LegacyBlockOwner::Root | LegacyBlockOwner::Cell)
            ) =>
        {
            Some(TypedBlockOwner::Table)
        }
        b"tr" if content_owner == Some(LegacyBlockOwner::Table) => Some(TypedBlockOwner::Row),
        b"tc" if content_owner == Some(LegacyBlockOwner::Row) => Some(TypedBlockOwner::Cell),
        b"sdt" if content_owner.is_some() => {
            Some(TypedBlockOwner::ContentControl(content_owner.unwrap()))
        }
        b"sdtContent"
            if matches!(parent, Some(TypedBlockOwner::ContentControl(_)))
                && elements
                    .last()
                    .is_some_and(|control| !control.sdt_content_seen) =>
        {
            let Some(TypedBlockOwner::ContentControl(owner)) = parent else {
                unreachable!();
            };
            Some(TypedBlockOwner::Content(owner))
        }
        _ => None,
    }
}

fn direct_word_run_parent(elements: &[DynamicXmlElement], paragraph: Option<usize>) -> bool {
    paragraph.is_some()
        && elements.last().is_some_and(|parent| {
            parent.is_typed_inline_owner
                && parent.local_name == b"r"
                && parent.paragraph == paragraph
        })
}

fn dynamic_toc_run_position(
    word: bool,
    local: &[u8],
    is_typed_inline_owner: bool,
    modeled_simple_field: bool,
    elements: &mut [DynamicXmlElement],
    paragraph: Option<usize>,
    next_boundaries: &mut [usize],
    nested_run_orders: &mut [HashMap<(usize, TocRawOrder), usize>],
    paragraph_raw_before: &mut [usize],
) -> Option<TocRunPosition> {
    let paragraph = paragraph?;
    let next = next_boundaries.get_mut(paragraph)?;
    let inherited = elements.last().and_then(|parent| parent.run_position);
    if word && local == b"r" && is_typed_inline_owner {
        let nested = elements.iter().rev().take_while(|element| {
            !element.is_typed_paragraph || element.paragraph != Some(paragraph)
        });
        if nested.into_iter().any(|element| {
            matches!(
                element.local_name.as_slice(),
                b"sdt" | b"sdtContent" | b"ins" | b"moveTo"
            )
        }) {
            let outer = inherited.unwrap_or(TocRunPosition {
                run_boundary: *next,
                raw_order: TocRawOrder::Raw(0),
                nested_order: 0,
            });
            return nested_leaf_position(paragraph, outer, nested_run_orders);
        }
        let boundary = *next;
        let position = nested_leaf_position(
            paragraph,
            TocRunPosition {
                run_boundary: boundary,
                raw_order: TocRawOrder::AfterRaw,
                nested_order: 0,
            },
            nested_run_orders,
        );
        *next += 1;
        *paragraph_raw_before.get_mut(paragraph)? = 0;
        return position;
    }
    if word
        && local == b"fldSimple"
        && modeled_simple_field
        && accepted_simple_field_parent(elements, paragraph)
    {
        if elements.last().is_some_and(|parent| {
            parent.is_typed_inline_owner
                && matches!(parent.local_name.as_slice(), b"ins" | b"moveTo")
        }) {
            let outer = inherited.unwrap_or(TocRunPosition {
                run_boundary: *next,
                raw_order: TocRawOrder::Raw(0),
                nested_order: 0,
            });
            return nested_leaf_position(paragraph, outer, nested_run_orders);
        }
        let boundary = *next;
        let position = nested_leaf_position(
            paragraph,
            TocRunPosition {
                run_boundary: boundary,
                raw_order: TocRawOrder::AfterRaw,
                nested_order: 0,
            },
            nested_run_orders,
        );
        *next += 1;
        *paragraph_raw_before.get_mut(paragraph)? = 0;
        return position;
    }
    if is_typed_inline_owner && local == b"hyperlink" {
        return inherited;
    }
    if is_typed_inline_owner && matches!(local, b"sdt" | b"sdtContent" | b"ins" | b"moveTo") {
        if let Some(inherited) = inherited {
            return Some(inherited);
        }
        let raw_order = if elements
            .last()
            .is_some_and(|parent| parent.is_typed_inline_owner && parent.local_name == b"hyperlink")
        {
            let parent = elements.last_mut()?;
            let plan = parent.hyperlink_plan.as_mut()?;
            let order = *plan.revision_orders.get(plan.next_revision)?;
            plan.next_revision += 1;
            order
        } else {
            TocRawOrder::Raw(*paragraph_raw_before.get(paragraph)?)
        };
        return Some(TocRunPosition {
            run_boundary: *next,
            raw_order,
            nested_order: 0,
        });
    }
    None
}

fn advance_direct_paragraph_raw_child(
    word: bool,
    local: &[u8],
    is_typed_inline_owner: bool,
    modeled_simple_field: bool,
    direct_paragraph_child: bool,
    paragraph: Option<usize>,
    hyperlink_plan: Option<&DynamicHyperlinkPlan>,
    paragraph_raw_before: &mut [usize],
) {
    if !direct_paragraph_child {
        return;
    }
    let modeled_without_raw = word
        && (local == b"pPr"
            || local == b"r" && is_typed_inline_owner
            || local == b"fldSimple" && modeled_simple_field
            || local == b"sdt" && is_typed_inline_owner
            || local == b"hyperlink");
    if modeled_without_raw
        || hyperlink_plan.is_some_and(|plan| plan.preserved_raw)
        || local == b"hyperlink"
    {
        return;
    }
    if let Some(raw_before) = paragraph.and_then(|index| paragraph_raw_before.get_mut(index)) {
        *raw_before += 1;
    }
}

fn dynamic_hyperlink_plan(
    xml: &[u8],
    start: usize,
    bindings: &BTreeMap<String, String>,
    raw_before: usize,
) -> Result<DynamicHyperlinkPlan> {
    let raw = dynamic_element_slice(xml, start)?;
    let mut scoped = Vec::new();
    scoped.extend_from_slice(b"<rdocx-scope");
    for (prefix, namespace) in bindings {
        if prefix == "xml" {
            continue;
        }
        if prefix.is_empty() {
            scoped.extend_from_slice(b" xmlns=\"");
        } else {
            scoped.extend_from_slice(b" xmlns:");
            scoped.extend_from_slice(prefix.as_bytes());
            scoped.extend_from_slice(b"=\"");
        }
        scoped.extend_from_slice(xml_escape_attribute(namespace).as_bytes());
        scoped.push(b'"');
    }
    scoped.push(b'>');
    scoped.extend_from_slice(raw);
    scoped.extend_from_slice(b"</rdocx-scope>");

    let mut reader = NsReader::from_reader(scoped.as_slice());
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut direct_runs = 0usize;
    let mut revision_boundaries = Vec::new();
    loop {
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid hyperlink XML: {error}")))?;
        let word = namespace_is_word(&namespace);
        match event {
            Event::Start(element) => {
                if depth == 2 && word && local_name(element.name().as_ref()) == b"r" {
                    direct_runs += 1;
                } else if depth == 2
                    && word
                    && matches!(local_name(element.name().as_ref()), b"ins" | b"moveTo")
                {
                    let valid_id = resolved_element_attribute(
                        &element,
                        reader.resolver(),
                        b"id",
                        AttributeNamespace::Word,
                    )?
                    .is_some_and(|(_, id)| id.parse::<i32>().is_ok());
                    let has_author = resolved_element_attribute(
                        &element,
                        reader.resolver(),
                        b"author",
                        AttributeNamespace::Word,
                    )?
                    .is_some();
                    if valid_id && has_author {
                        revision_boundaries.push(direct_runs);
                    }
                }
                depth += 1;
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            Event::Eof => {
                return Err(Error::Other(
                    "table of contents hyperlink is not balanced".to_owned(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    }
    let preserved_raw = direct_runs == 0;
    let revision_orders = revision_boundaries
        .into_iter()
        .map(|boundary| {
            if preserved_raw {
                TocRawOrder::Raw(raw_before)
            } else if boundary == direct_runs {
                TocRawOrder::BeforeRaw
            } else {
                TocRawOrder::AfterRaw
            }
        })
        .collect();
    Ok(DynamicHyperlinkPlan {
        revision_orders,
        next_revision: 0,
        preserved_raw,
    })
}

fn dynamic_element_slice(xml: &[u8], start: usize) -> Result<&[u8]> {
    let tail = xml.get(start..).ok_or_else(|| {
        Error::Other("table of contents element offset is outside the document".to_owned())
    })?;
    let mut reader = quick_xml::Reader::from_reader(tail);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid table of contents XML: {error}")))?
        {
            Event::Start(_) => depth += 1,
            Event::Empty(_) if depth == 0 => {
                return Ok(&tail[..reader.buffer_position() as usize]);
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Ok(&tail[..reader.buffer_position() as usize]);
                }
            }
            Event::Eof => {
                return Err(Error::Other(
                    "table of contents element is not balanced".to_owned(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    }
}

fn nested_leaf_position(
    paragraph: usize,
    outer: TocRunPosition,
    nested_run_orders: &mut [HashMap<(usize, TocRawOrder), usize>],
) -> Option<TocRunPosition> {
    let next = nested_run_orders
        .get_mut(paragraph)?
        .entry((outer.run_boundary, outer.raw_order))
        .or_default();
    let nested_order = *next;
    *next += 1;
    Some(TocRunPosition {
        nested_order,
        ..outer
    })
}

pub(super) fn accepted_simple_field_parent(
    elements: &[DynamicXmlElement],
    paragraph: usize,
) -> bool {
    direct_typed_paragraph_parent(elements, Some(paragraph))
        || elements.last().is_some_and(|parent| {
            parent.is_typed_inline_owner
                && matches!(parent.local_name.as_slice(), b"ins" | b"moveTo")
                && parent.paragraph == Some(paragraph)
        })
}

pub(super) fn typed_inline_owner(
    element: &BytesStart<'_>,
    resolver: &NamespaceResolver,
    word: bool,
    local: &[u8],
    elements: &[DynamicXmlElement],
    paragraph: Option<usize>,
) -> Result<bool> {
    if !word || paragraph.is_none() {
        return Ok(false);
    }
    let Some(parent) = elements.last() else {
        return Ok(false);
    };
    let parent_is_paragraph = parent.is_typed_paragraph && parent.paragraph == paragraph;
    let parent_is_inline = parent.is_typed_inline_owner && parent.paragraph == paragraph;
    let parent_local = parent.local_name.as_slice();
    let valid = match local {
        b"r" => {
            parent_is_paragraph
                || (parent_is_inline
                    && matches!(
                        parent_local,
                        b"hyperlink" | b"sdtContent" | b"ins" | b"moveTo"
                    ))
        }
        b"hyperlink" => {
            parent_is_paragraph || (parent_is_inline && matches!(parent_local, b"ins" | b"moveTo"))
        }
        b"sdt" => {
            parent_is_paragraph
                || (parent_is_inline && matches!(parent_local, b"sdtContent" | b"ins" | b"moveTo"))
        }
        b"sdtContent" => parent_is_inline && parent_local == b"sdt" && !parent.sdt_content_seen,
        b"ins" | b"moveTo" => {
            let valid_parent = parent_is_paragraph
                || (parent_is_inline
                    && matches!(
                        parent_local,
                        b"hyperlink" | b"sdtContent" | b"ins" | b"moveTo"
                    ));
            let valid_id =
                resolved_element_attribute(element, resolver, b"id", AttributeNamespace::Word)?
                    .is_some_and(|(_, id)| id.parse::<i32>().is_ok());
            let has_author =
                resolved_element_attribute(element, resolver, b"author", AttributeNamespace::Word)?
                    .is_some();
            valid_parent
                && parent.revision_depth < MAX_DYNAMIC_REVISION_NESTING_DEPTH
                && valid_id
                && has_author
        }
        _ => false,
    };
    Ok(valid)
}

fn direct_typed_paragraph_parent(elements: &[DynamicXmlElement], paragraph: Option<usize>) -> bool {
    let Some(paragraph) = paragraph else {
        return false;
    };
    elements
        .last()
        .is_some_and(|parent| parent.is_typed_paragraph && parent.paragraph == Some(paragraph))
}

pub(super) fn accepted_bookmark_parent(
    elements: &[DynamicXmlElement],
    paragraph: Option<usize>,
) -> bool {
    direct_typed_paragraph_parent(elements, paragraph)
        || paragraph.is_some()
            && elements.last().is_some_and(|parent| {
                parent.is_typed_inline_owner
                    && matches!(
                        parent.local_name.as_slice(),
                        b"hyperlink" | b"sdtContent" | b"ins" | b"moveTo"
                    )
                    && parent.paragraph == paragraph
            })
}

fn accepted_simple_toc_parent(elements: &[DynamicXmlElement], paragraph: Option<usize>) -> bool {
    direct_typed_paragraph_parent(elements, paragraph)
        || paragraph.is_some()
            && elements.last().is_some_and(|parent| {
                parent.is_typed_inline_owner
                    && matches!(parent.local_name.as_slice(), b"ins" | b"moveTo")
                    && parent.paragraph == paragraph
            })
}

fn update_dynamic_field_stack(
    element: &BytesStart<'_>,
    resolver: &NamespaceResolver,
    event_start: usize,
    paragraph: Option<usize>,
    elements: &[DynamicXmlElement],
    fields: &mut Vec<DynamicFieldScan>,
    spans: &mut Vec<DynamicTocSpan>,
    generated: DynamicOwnerPolicy,
) -> Result<()> {
    let Some(paragraph) = paragraph else {
        return Ok(());
    };
    let kind =
        resolved_element_attribute(element, resolver, b"fldCharType", AttributeNamespace::Word)?
            .map(|(_, value)| value);
    match kind.as_deref() {
        Some("begin") => fields.push(DynamicFieldScan {
            instruction: String::new(),
            field_start: event_start,
            begin_paragraph: paragraph,
            begin_paragraph_start: elements
                .iter()
                .rev()
                .find(|element| element.is_typed_paragraph && element.paragraph == Some(paragraph))
                .map(|element| element.start)
                .ok_or_else(|| {
                    Error::Other(
                        "table of contents begin marker is not contained by a paragraph".to_owned(),
                    )
                })?,
            begin_run_start: elements
                .iter()
                .rev()
                .find(|element| {
                    element.is_word
                        && element.local_name == b"r"
                        && element.paragraph == Some(paragraph)
                })
                .map(|element| element.start)
                .ok_or_else(|| {
                    Error::Other(
                        "table of contents begin marker is not contained by a run".to_owned(),
                    )
                })?,
            separator_paragraph: None,
            separator_run_start: None,
            result_start: None,
            result_start_position: None,
            start_paragraph_name: None,
            start_paragraph_namespaces: BTreeMap::new(),
            separator_wrapper_names: Vec::new(),
            instruction_runs: Vec::new(),
        }),
        Some("separate") => {
            let Some(field) = fields.last_mut() else {
                return Ok(());
            };
            if field.separator_paragraph.is_some() {
                return Err(Error::Other(
                    "table of contents field has more than one separator".to_owned(),
                ));
            }
            let run = elements
                .iter()
                .rev()
                .find(|element| {
                    element.is_word
                        && element.local_name == b"r"
                        && element.paragraph == Some(paragraph)
                })
                .ok_or_else(|| {
                    Error::Other("table of contents separator is not contained by a run".to_owned())
                })?;
            let para = elements
                .iter()
                .rev()
                .find(|element| element.is_typed_paragraph && element.paragraph == Some(paragraph))
                .ok_or_else(|| {
                    Error::Other(
                        "table of contents separator is not contained by a paragraph".to_owned(),
                    )
                })?;
            field.separator_paragraph = Some(paragraph);
            field.separator_run_start = Some(run.start);
            field.result_start_position = run.run_position.map(|position| {
                if position.raw_order == TocRawOrder::AfterRaw {
                    TocRunPosition {
                        run_boundary: position.run_boundary + 1,
                        raw_order: TocRawOrder::BeforeRaw,
                        nested_order: 0,
                    }
                } else {
                    TocRunPosition {
                        nested_order: position.nested_order + 1,
                        ..position
                    }
                }
            });
            field.start_paragraph_name = Some(para.qualified_name.clone());
            field.start_paragraph_namespaces = para.inherited_namespaces.clone();
            let paragraph_position = elements
                .iter()
                .position(|element| std::ptr::eq(element, para))
                .expect("paragraph was borrowed from the element stack");
            let run_position = elements
                .iter()
                .rposition(|element| std::ptr::eq(element, run))
                .expect("run was borrowed from the element stack");
            field.separator_wrapper_names = elements[paragraph_position + 1..run_position]
                .iter()
                .filter(|element| element.is_typed_inline_owner)
                .map(|element| element.qualified_name.clone())
                .collect();
        }
        Some("end") => {
            let Some(field) = fields.pop() else {
                return Ok(());
            };
            if !generated_table_opcode(
                &Field::new(&field.instruction, "").instruction.name,
                generated,
            ) {
                return Ok(());
            }
            let result_start = field.result_start.ok_or_else(|| {
                Error::Other("table of contents field is missing its separator".to_owned())
            })?;
            let separator_paragraph = field.separator_paragraph.ok_or_else(|| {
                Error::Other("table of contents field is missing its separator".to_owned())
            })?;
            if separator_paragraph != field.begin_paragraph {
                return Err(Error::Other(
                    "table of contents instruction crosses a paragraph boundary".to_owned(),
                ));
            }
            let end_run = elements
                .iter()
                .rev()
                .find(|element| {
                    element.is_word
                        && element.local_name == b"r"
                        && element.paragraph == Some(paragraph)
                })
                .ok_or_else(|| {
                    Error::Other(
                        "table of contents end marker is not contained by a run".to_owned(),
                    )
                })?;
            let end_para = elements
                .iter()
                .rev()
                .find(|element| element.is_typed_paragraph && element.paragraph == Some(paragraph))
                .ok_or_else(|| {
                    Error::Other(
                        "table of contents end marker is not contained by a paragraph".to_owned(),
                    )
                })?;
            if event_start < result_start || end_run.start < result_start {
                return Err(Error::Other(
                    "table of contents result boundaries are reversed".to_owned(),
                ));
            }
            let paragraph_position = elements
                .iter()
                .position(|element| std::ptr::eq(element, end_para))
                .expect("end paragraph was borrowed from the element stack");
            let run_position = elements
                .iter()
                .rposition(|element| std::ptr::eq(element, end_run))
                .expect("end run was borrowed from the element stack");
            let wrapper_chain = &elements[paragraph_position + 1..run_position];
            let mut end_wrapper_prefixes = Vec::new();
            let mut wrapper_index = 0usize;
            while wrapper_index < wrapper_chain.len() {
                let wrapper = &wrapper_chain[wrapper_index];
                if !wrapper.is_typed_inline_owner {
                    wrapper_index += 1;
                    continue;
                }
                if wrapper.local_name == b"sdt"
                    && let Some(content) = wrapper_chain.get(wrapper_index + 1)
                    && content.is_typed_inline_owner
                    && content.local_name == b"sdtContent"
                {
                    end_wrapper_prefixes.push((wrapper.start, content.start_tag_end));
                    wrapper_index += 2;
                } else {
                    end_wrapper_prefixes.push((wrapper.start, wrapper.start_tag_end));
                    wrapper_index += 1;
                }
            }
            spans.push(DynamicTocSpan {
                simple_field: None,
                instruction: field.instruction,
                field_start: field.field_start,
                field_end: event_start,
                begin_paragraph: field.begin_paragraph,
                end_paragraph: paragraph,
                begin_run_start: field.begin_run_start,
                instruction_paragraph_start: field.begin_paragraph_start,
                result_start,
                result_end: end_run.start,
                result_start_position: field.result_start_position.ok_or_else(|| {
                    Error::Other(
                        "table of contents separator run position was not retained".to_owned(),
                    )
                })?,
                result_end_position: end_run.run_position.ok_or_else(|| {
                    Error::Other("table of contents end run position was not retained".to_owned())
                })?,
                end_run_end: 0,
                start_paragraph_name: field.start_paragraph_name.ok_or_else(|| {
                    Error::Other("table of contents field is missing its separator".to_owned())
                })?,
                start_paragraph_namespaces: field.start_paragraph_namespaces,
                separator_wrapper_names: field.separator_wrapper_names,
                instruction_runs: field.instruction_runs,
                end_paragraph_start: end_para.start,
                end_paragraph_content_start: 0,
                end_wrapper_prefixes,
            });
        }
        _ => {}
    }
    Ok(())
}

type ParsedDynamicTocFields = (Vec<Option<TocField>>, Vec<(usize, String)>);

pub(super) fn parse_dynamic_toc_fields(
    document: &Document,
    xml: &[u8],
    spans: &[DynamicTocSpan],
) -> Result<ParsedDynamicTocFields> {
    let mut projected = document.clone_for_staging();
    prepare_physical_story_projection(&mut projected.document.body, &mut [])?;
    let document = &projected;
    let mut paragraphs = Vec::new();
    collect_body_paragraphs(&document.document.body, &mut paragraphs, None);
    let context = FieldEvaluationContext::default();
    let mut output = Vec::with_capacity(spans.len());
    let mut diagnostics = Vec::new();
    for span in spans {
        let field = parse_dynamic_toc_field(xml, span)?;
        let mut evaluator = Evaluator::new(document, &context);
        evaluator.ensure_numbering_layout_for_field(&field)?;
        match evaluator.evaluate_field(&field, "main", &paragraphs, span.begin_paragraph, None) {
            FieldOutcome::TableOfContents(toc) => output.push(Some(toc)),
            FieldOutcome::KeepStored { diagnostic } => {
                diagnostics.push((span.field_start, diagnostic));
                output.push(None);
            }
            _ => {
                return Err(Error::Other(
                    "table of contents instruction was not recognized".to_owned(),
                ));
            }
        }
    }
    Ok((output, diagnostics))
}

pub(super) fn parse_dynamic_toc_field(xml: &[u8], span: &DynamicTocSpan) -> Result<Field> {
    if let Some(field) = &span.simple_field {
        return Ok(field.clone());
    }
    let prefix = span
        .start_paragraph_name
        .split_once(':')
        .map_or("w", |(prefix, _)| prefix);
    // The instruction paragraph is cut out of its part, so its start tag gets
    // the declarations it inherits there. Every run in it then resolves the
    // prefixes it resolved when the document was read.
    let mut source = Vec::new();
    append_with_inherited_namespaces(
        &mut source,
        &xml[span.instruction_paragraph_start..span.result_start],
        &span.start_paragraph_namespaces,
    )?;
    source.extend_from_slice(
        format!("<{prefix}:r><{prefix}:fldChar {prefix}:fldCharType=\"end\"/></{prefix}:r>")
            .as_bytes(),
    );
    append_toc_wrapper_closures(&mut source, &span.separator_wrapper_names);
    source.extend_from_slice(b"</");
    source.extend_from_slice(span.start_paragraph_name.as_bytes());
    source.push(b'>');
    let parse_paragraph = |source: &[u8]| -> Result<CT_P> {
        let mut reader = quick_xml::Reader::from_reader(source);
        reader.config_mut().trim_text(false);
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer).map_err(|error| {
                Error::Other(format!("invalid table of contents field: {error}"))
            })? {
                Event::Start(element) if matches_local_name(element.name().as_ref(), b"p") => {
                    return Ok(CT_P::from_xml(&mut reader)?);
                }
                Event::Eof => {
                    return Err(Error::Other(
                        "table of contents instruction paragraph was not found".to_owned(),
                    ));
                }
                _ => {}
            }
            buffer.clear();
        }
    };
    CT_P::from_xml_fragment(&source)?;

    let mut projected = format!("<w:p xmlns:w=\"{W_NS}\">").into_bytes();
    for run in &span.instruction_runs {
        append_with_inherited_namespaces(
            &mut projected,
            &xml[run.start..run.end],
            &run.inherited_namespaces,
        )?;
    }
    projected.extend_from_slice(b"<w:r><w:fldChar w:fldCharType=\"end\"/></w:r></w:p>");
    let paragraph = parse_paragraph(&projected)?;
    accepted_toc_runs(&paragraph)
        .into_iter()
        .flat_map(|run| &run.run.content)
        .find_map(|content| match content {
            RunContent::Field(field)
                if field.effective_instruction().name
                    == Field::new(&span.instruction, "").instruction.name =>
            {
                Some(field.clone())
            }
            _ => None,
        })
        .ok_or_else(|| {
            Error::Other(format!(
                "table of contents instruction could not be parsed: {}",
                span.instruction.trim()
            ))
        })
}

fn append_with_inherited_namespaces(
    output: &mut Vec<u8>,
    raw: &[u8],
    inherited_namespaces: &BTreeMap<String, String>,
) -> Result<()> {
    let mut reader = quick_xml::Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let (insertion, local_namespaces) =
        match reader.read_event_into(&mut buffer).map_err(|error| {
            Error::Other(format!(
                "invalid table of contents instruction XML: {error}"
            ))
        })? {
            Event::Start(start) | Event::Empty(start) => {
                let mut local_namespaces = HashSet::new();
                for attribute in start.attributes() {
                    let attribute = attribute.map_err(|error| {
                        Error::Other(format!(
                            "invalid table of contents instruction XML: {error}"
                        ))
                    })?;
                    let key = attribute.key.as_ref();
                    if key == b"xmlns" {
                        local_namespaces.insert(String::new());
                    } else if let Some(prefix) = key.strip_prefix(b"xmlns:") {
                        local_namespaces.insert(String::from_utf8_lossy(prefix).into_owned());
                    }
                }
                let tag_end = reader.buffer_position() as usize;
                let insertion = if tag_end >= 2 && raw[tag_end - 2] == b'/' {
                    tag_end - 2
                } else {
                    tag_end - 1
                };
                (insertion, local_namespaces)
            }
            _ => {
                return Err(Error::Other(
                    "table of contents instruction XML has no start tag".to_owned(),
                ));
            }
        };
    output.extend_from_slice(&raw[..insertion]);
    for (prefix, namespace) in inherited_namespaces {
        if prefix == "xml" || local_namespaces.contains(prefix) {
            continue;
        }
        if prefix.is_empty() {
            output.extend_from_slice(b" xmlns=\"");
        } else {
            output.extend_from_slice(b" xmlns:");
            output.extend_from_slice(prefix.as_bytes());
            output.extend_from_slice(b"=\"");
        }
        output.extend_from_slice(xml_escape_attribute(namespace).as_bytes());
        output.push(b'"');
    }
    output.extend_from_slice(&raw[insertion..]);
    Ok(())
}
