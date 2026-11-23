//! Data binding: prefix mappings and XPath steps, selecting the bound element of a custom XML part and writing values into it.

use super::*;

pub(super) fn resolve_binding_edit(
    package: &OpcPackage,
    document_part: &str,
    binding: &CT_DataBinding,
    value: &str,
) -> Result<(String, XmlEdit)> {
    let store_item_id = binding
        .store_item_id
        .as_deref()
        .ok_or_else(|| Error::Other("content-control binding has no store item id".to_owned()))?;
    let xpath = binding
        .xpath
        .as_deref()
        .ok_or_else(|| Error::Other("content-control binding has no XPath".to_owned()))?;
    let mappings = parse_prefix_mappings(binding.prefix_mappings.as_deref().unwrap_or(""))?;
    let steps = parse_xpath(xpath, &mappings)?;
    let part_name = resolve_custom_xml_part(package, document_part, store_item_id)?;
    let xml = package
        .get_part(&part_name)
        .ok_or_else(|| Error::Other(format!("custom XML part {part_name} is missing")))?;
    let selection = select_xml_element(xml, &steps)?;
    let replacement = if selection.empty {
        expand_empty_element(xml, &selection, value)?
    } else {
        quick_xml::escape::escape(value).as_bytes().to_vec()
    };
    Ok((
        part_name,
        XmlEdit {
            start: selection.replace_start,
            end: selection.replace_end,
            replacement,
        },
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct XPathStep {
    namespace: Option<Vec<u8>>,
    local_name: Vec<u8>,
    index: Option<usize>,
}

pub(super) fn parse_prefix_mappings(value: &str) -> Result<HashMap<String, Vec<u8>>> {
    let bytes = value.as_bytes();
    let mut mappings = HashMap::new();
    let mut position = 0usize;
    while position < bytes.len() {
        skip_ascii_whitespace(bytes, &mut position);
        if position == bytes.len() {
            break;
        }
        if !bytes[position..].starts_with(b"xmlns") {
            return Err(Error::Other("unsupported prefixMappings syntax".to_owned()));
        }
        position += b"xmlns".len();
        let prefix = if bytes.get(position) == Some(&b':') {
            position += 1;
            let start = position;
            while bytes
                .get(position)
                .is_some_and(|byte| is_xml_name_byte(*byte))
            {
                position += 1;
            }
            if start == position {
                return Err(Error::Other(
                    "prefixMappings has an empty prefix".to_owned(),
                ));
            }
            let prefix = std::str::from_utf8(&bytes[start..position])
                .map_err(|_| Error::Other("prefixMappings prefix is not UTF-8".to_owned()))?
                .to_owned();
            if !is_xml_name(&prefix) {
                return Err(Error::Other(
                    "prefixMappings contains an unsupported prefix".to_owned(),
                ));
            }
            prefix
        } else {
            String::new()
        };
        skip_ascii_whitespace(bytes, &mut position);
        if bytes.get(position) != Some(&b'=') {
            return Err(Error::Other("prefixMappings is missing '='".to_owned()));
        }
        position += 1;
        skip_ascii_whitespace(bytes, &mut position);
        let quote = *bytes
            .get(position)
            .filter(|quote| **quote == b'\'' || **quote == b'"')
            .ok_or_else(|| Error::Other("prefixMappings URI is not quoted".to_owned()))?;
        position += 1;
        let start = position;
        while bytes.get(position).is_some_and(|byte| *byte != quote) {
            position += 1;
        }
        if bytes.get(position) != Some(&quote) {
            return Err(Error::Other(
                "prefixMappings URI has no closing quote".to_owned(),
            ));
        }
        let uri = bytes[start..position].to_vec();
        position += 1;
        if let Some(previous) = mappings.insert(prefix, uri.clone())
            && previous != uri
        {
            return Err(Error::Other(
                "prefixMappings defines one prefix more than once".to_owned(),
            ));
        }
    }
    Ok(mappings)
}

pub(super) fn parse_xpath(
    xpath: &str,
    mappings: &HashMap<String, Vec<u8>>,
) -> Result<Vec<XPathStep>> {
    if !xpath.starts_with('/') || xpath.starts_with("//") {
        return Err(Error::Other(
            "content-control XPath must be an absolute child path".to_owned(),
        ));
    }
    let mut steps = Vec::new();
    for segment in xpath[1..].split('/') {
        if segment.is_empty() {
            return Err(Error::Other(
                "content-control XPath contains a descendant or empty step".to_owned(),
            ));
        }
        let (name, index) = parse_xpath_segment(segment)?;
        let mut names = name.split(':');
        let first = names.next().unwrap_or_default();
        let second = names.next();
        if names.next().is_some() {
            return Err(Error::Other(
                "content-control XPath contains an invalid qualified name".to_owned(),
            ));
        }
        let (namespace, local_name) = match second {
            Some(local) => {
                if !is_xml_name(first) {
                    return Err(Error::Other(
                        "content-control XPath contains an unsupported prefix".to_owned(),
                    ));
                }
                let namespace = mappings.get(first).cloned().ok_or_else(|| {
                    Error::Other(format!(
                        "content-control XPath prefix {first} is not mapped"
                    ))
                })?;
                (Some(namespace), local)
            }
            None => (mappings.get("").cloned(), first),
        };
        if !is_xml_name(local_name) {
            return Err(Error::Other(
                "content-control XPath contains an unsupported name".to_owned(),
            ));
        }
        steps.push(XPathStep {
            namespace,
            local_name: local_name.as_bytes().to_vec(),
            index,
        });
    }
    if steps.is_empty() {
        return Err(Error::Other("content-control XPath is empty".to_owned()));
    }
    Ok(steps)
}

fn parse_xpath_segment(segment: &str) -> Result<(&str, Option<usize>)> {
    let Some(open) = segment.find('[') else {
        if segment.contains(['*', '(', ')', ']']) {
            return Err(Error::Other(
                "content-control XPath uses an unsupported selector".to_owned(),
            ));
        }
        return Ok((segment, None));
    };
    if !segment.ends_with(']') || segment[open + 1..segment.len() - 1].contains(['[', ']']) {
        return Err(Error::Other(
            "content-control XPath uses an unsupported predicate".to_owned(),
        ));
    }
    let name = &segment[..open];
    let index = segment[open + 1..segment.len() - 1]
        .parse::<usize>()
        .ok()
        .filter(|index| *index > 0)
        .ok_or_else(|| Error::Other("content-control XPath index must be one-based".to_owned()))?;
    if name.is_empty() || name.contains(['*', '(', ')', '[', ']']) {
        return Err(Error::Other(
            "content-control XPath uses an unsupported selector".to_owned(),
        ));
    }
    Ok((name, Some(index)))
}

fn skip_ascii_whitespace(bytes: &[u8], position: &mut usize) {
    while bytes
        .get(*position)
        .is_some_and(|byte| byte.is_ascii_whitespace())
    {
        *position += 1;
    }
}

fn is_xml_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')
}

fn is_xml_name(value: &str) -> bool {
    value
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && value.bytes().skip(1).all(is_xml_name_byte)
}

#[derive(Debug, Clone)]
struct XmlNode {
    parent: Option<usize>,
    namespace: Option<Vec<u8>>,
    local_name: Vec<u8>,
    occurrence: usize,
    replace_start: usize,
    replace_end: usize,
    element_start: usize,
    element_end: usize,
    qualified_name: Vec<u8>,
    empty: bool,
    simple_text: bool,
}

pub(super) fn select_xml_element(xml: &[u8], steps: &[XPathStep]) -> Result<XmlSelection> {
    let nodes = parse_xml_nodes(xml)?;
    let mut candidates = Vec::new();
    for (step_index, step) in steps.iter().enumerate() {
        let parents = candidates;
        candidates = nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| {
                let parent_matches = if step_index == 0 {
                    node.parent.is_none()
                } else {
                    node.parent.is_some_and(|parent| parents.contains(&parent))
                };
                parent_matches
                    && node.namespace == step.namespace
                    && node.local_name == step.local_name
                    && step.index.is_none_or(|index| node.occurrence == index)
            })
            .map(|(index, _)| index)
            .collect();
    }
    let [selected_index] = candidates.as_slice() else {
        return if candidates.is_empty() {
            Err(Error::Other(
                "content-control XPath selected no custom XML element".to_owned(),
            ))
        } else {
            Err(Error::Other(
                "content-control XPath selected more than one custom XML element".to_owned(),
            ))
        };
    };
    let selected = &nodes[*selected_index];
    if !selected.simple_text {
        return Err(Error::Other(
            "content-control XPath selected an element with preserved markup".to_owned(),
        ));
    }
    Ok(XmlSelection {
        replace_start: selected.replace_start,
        replace_end: selected.replace_end,
        element_start: selected.element_start,
        element_end: selected.element_end,
        qualified_name: selected.qualified_name.clone(),
        empty: selected.empty,
    })
}

fn parse_xml_nodes(xml: &[u8]) -> Result<Vec<XmlNode>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut nodes: Vec<XmlNode> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    loop {
        let before = usize::try_from(reader.buffer_position())
            .map_err(|_| Error::Other("custom XML position overflow".to_owned()))?;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid custom XML part: {error}")))?;
        let namespace = namespace_bytes(&namespace)?;
        let event = event.into_owned();
        let after = usize::try_from(reader.buffer_position())
            .map_err(|_| Error::Other("custom XML position overflow".to_owned()))?;
        match event {
            Event::Start(element) => {
                if let Some(parent) = stack.last().copied() {
                    nodes[parent].simple_text = false;
                }
                let parent = stack.last().copied();
                let qualified_name = element.name().as_ref().to_vec();
                let local_name = qualified_name
                    .rsplit(|byte| *byte == b':')
                    .next()
                    .unwrap_or(&qualified_name)
                    .to_vec();
                let occurrence = nodes
                    .iter()
                    .filter(|node| {
                        node.parent == parent
                            && node.namespace == namespace
                            && node.local_name == local_name
                    })
                    .count()
                    .checked_add(1)
                    .ok_or_else(|| Error::Other("custom XML sibling count overflow".to_owned()))?;
                let index = nodes.len();
                nodes.push(XmlNode {
                    parent,
                    namespace,
                    local_name,
                    occurrence,
                    replace_start: after,
                    replace_end: 0,
                    element_start: before,
                    element_end: after,
                    qualified_name,
                    empty: false,
                    simple_text: true,
                });
                stack.push(index);
            }
            Event::Empty(element) => {
                if let Some(parent) = stack.last().copied() {
                    nodes[parent].simple_text = false;
                }
                let parent = stack.last().copied();
                let qualified_name = element.name().as_ref().to_vec();
                let local_name = qualified_name
                    .rsplit(|byte| *byte == b':')
                    .next()
                    .unwrap_or(&qualified_name)
                    .to_vec();
                let occurrence = nodes
                    .iter()
                    .filter(|node| {
                        node.parent == parent
                            && node.namespace == namespace
                            && node.local_name == local_name
                    })
                    .count()
                    .checked_add(1)
                    .ok_or_else(|| Error::Other("custom XML sibling count overflow".to_owned()))?;
                nodes.push(XmlNode {
                    parent,
                    namespace,
                    local_name,
                    occurrence,
                    replace_start: before,
                    replace_end: after,
                    element_start: before,
                    element_end: after,
                    qualified_name,
                    empty: true,
                    simple_text: true,
                });
            }
            Event::End(_) => {
                let index = stack.pop().ok_or_else(|| {
                    Error::Other("custom XML contains an unmatched end element".to_owned())
                })?;
                nodes[index].replace_end = before;
                nodes[index].element_end = after;
            }
            Event::Comment(_) | Event::PI(_) | Event::DocType(_) => {
                if let Some(index) = stack.last().copied() {
                    nodes[index].simple_text = false;
                }
            }
            Event::Eof => {
                if !stack.is_empty() {
                    return Err(Error::Other(
                        "custom XML contains an unclosed element".to_owned(),
                    ));
                }
                return Ok(nodes);
            }
            _ => {}
        }
        buffer.clear();
    }
}

fn namespace_bytes(namespace: &ResolveResult<'_>) -> Result<Option<Vec<u8>>> {
    match namespace {
        ResolveResult::Bound(Namespace(uri)) => Ok(Some(uri.to_vec())),
        ResolveResult::Unbound => Ok(None),
        ResolveResult::Unknown(prefix) => Err(Error::Other(format!(
            "custom XML uses unmapped namespace prefix {}",
            String::from_utf8_lossy(prefix)
        ))),
    }
}

pub(super) fn expand_empty_element(
    xml: &[u8],
    selection: &XmlSelection,
    value: &str,
) -> Result<Vec<u8>> {
    let raw = xml
        .get(selection.element_start..selection.element_end)
        .ok_or_else(|| Error::Other("custom XML selection is out of bounds".to_owned()))?;
    let slash = raw
        .iter()
        .rposition(|byte| *byte == b'/')
        .filter(|slash| raw.get(slash + 1) == Some(&b'>'))
        .ok_or_else(|| Error::Other("custom XML empty element is malformed".to_owned()))?;
    let mut expanded = Vec::new();
    expanded.extend_from_slice(&raw[..slash]);
    expanded.push(b'>');
    expanded.extend_from_slice(quick_xml::escape::escape(value).as_bytes());
    expanded.extend_from_slice(b"</");
    expanded.extend_from_slice(&selection.qualified_name);
    expanded.push(b'>');
    Ok(expanded)
}

pub(super) fn apply_xml_edits(xml: &[u8], mut edits: Vec<XmlEdit>) -> Result<Vec<u8>> {
    edits.sort_by_key(|edit| (edit.start, edit.end));
    let mut unique: Vec<XmlEdit> = Vec::new();
    for edit in edits {
        if let Some(previous) = unique.last() {
            if previous.start == edit.start && previous.end == edit.end {
                if previous.replacement == edit.replacement {
                    continue;
                }
                return Err(Error::Other(
                    "content controls assign different values to one custom XML element".to_owned(),
                ));
            }
            if edit.start < previous.end {
                return Err(Error::Other(
                    "content-control custom XML selections overlap".to_owned(),
                ));
            }
        }
        unique.push(edit);
    }
    let mut output = xml.to_vec();
    for edit in unique.into_iter().rev() {
        if edit.start > edit.end || edit.end > output.len() {
            return Err(Error::Other(
                "content-control custom XML selection is out of bounds".to_owned(),
            ));
        }
        output.splice(edit.start..edit.end, edit.replacement);
    }
    validate_xml(&output)?;
    Ok(output)
}

fn validate_xml(xml: &[u8]) -> Result<()> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Eof) => return Ok(()),
            Ok(_) => buffer.clear(),
            Err(error) => {
                return Err(Error::Other(format!(
                    "content-control custom XML edit is invalid: {error}"
                )));
            }
        }
    }
}
