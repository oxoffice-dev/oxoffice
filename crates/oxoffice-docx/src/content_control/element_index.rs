//! An index of a part's elements by byte range, with their namespaces, parents and `w:val`, tree queries, and inserting into an element.

use super::*;

pub(super) fn index_elements(xml: &[u8]) -> Result<Vec<XmlElem>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut stack: Vec<(usize, usize, usize)> = Vec::new();
    let mut elems = Vec::new();
    let mut next_id = 0usize;
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = match reader.read_resolved_event_into(&mut buffer) {
            Ok(event) => event,
            Err(error) => {
                return Err(Error::Other(format!(
                    "content control XML is invalid: {error}"
                )));
            }
        };
        let word = is_word(&namespace);
        let w14 = is_w14(&namespace);
        let w15 = is_w15(&namespace);
        drop(namespace);
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                let id = next_id;
                next_id += 1;
                let parent = stack.last().map(|frame| frame.0);
                let val = word_val(&reader, &element)?;
                stack.push((id, before, after));
                elems.push(XmlElem {
                    id,
                    parent,
                    local: element.local_name().as_ref().to_vec(),
                    word,
                    w14,
                    w15,
                    range: before..before,
                    open_end: after,
                    inner_end: None,
                    val,
                });
            }
            Event::Empty(element) => {
                let id = next_id;
                next_id += 1;
                elems.push(XmlElem {
                    id,
                    parent: stack.last().map(|frame| frame.0),
                    local: element.local_name().as_ref().to_vec(),
                    word,
                    w14,
                    w15,
                    range: before..after,
                    open_end: after,
                    inner_end: None,
                    val: word_val(&reader, &element)?,
                });
            }
            Event::End(_) => {
                let Some((id, start, open_end)) = stack.pop() else {
                    return Err(Error::Other(
                        "content control XML has an unmatched end element".to_owned(),
                    ));
                };
                // IDs are assigned exactly once at the append position for
                // both start and empty elements. Closing never reorders them.
                let Some(elem) = elems.get_mut(id).filter(|elem| elem.id == id) else {
                    return Err(Error::Other(
                        "content control XML lost an open element".to_owned(),
                    ));
                };
                elem.range = start..after;
                elem.open_end = open_end;
                elem.inner_end = Some(before);
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if !stack.is_empty() {
        return Err(Error::Other(
            "content control XML ended inside an element".to_owned(),
        ));
    }
    Ok(elems)
}

pub(super) fn is_word(namespace: &ResolveResult<'_>) -> bool {
    matches!(namespace, ResolveResult::Bound(Namespace(uri)) if *uri == W_NS.as_bytes())
}

pub(super) fn is_w14(namespace: &ResolveResult<'_>) -> bool {
    matches!(namespace, ResolveResult::Bound(Namespace(uri)) if *uri == W14_NS.as_bytes())
}

fn is_w15(namespace: &ResolveResult<'_>) -> bool {
    matches!(namespace, ResolveResult::Bound(Namespace(uri)) if *uri == W15_NS.as_bytes())
}

fn word_val(reader: &NsReader<&[u8]>, element: &BytesStart<'_>) -> Result<Option<String>> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| {
            Error::Other(format!("content control attribute is invalid: {error}"))
        })?;
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        if is_word(&namespace) && local.as_ref() == b"val" {
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                .map_err(|error| {
                    Error::Other(format!(
                        "content control attribute value is invalid: {error}"
                    ))
                })?;
            return Ok(Some(value.into_owned()));
        }
    }
    Ok(None)
}

pub(super) fn element_at<'a>(elems: &'a [XmlElem], range: &Range<usize>) -> Result<&'a XmlElem> {
    elems
        .iter()
        .find(|elem| elem.range.start == range.start)
        .ok_or_else(|| Error::Other("content control bytes are missing".to_owned()))
}

pub(super) fn nth_direct_child(
    xml: &[u8],
    parent: &Range<usize>,
    local: &[u8],
    index: usize,
) -> Result<Range<usize>> {
    let elems = index_elements(xml)?;
    let parent = element_at(&elems, parent)?;
    elems
        .iter()
        .filter(|elem| elem.parent == Some(parent.id) && elem.word && elem.local == local)
        .nth(index)
        .map(|elem| elem.range.clone())
        .ok_or_else(|| {
            Error::Other("content control placement does not match its parent".to_owned())
        })
}

pub(super) fn table_column_count(xml: &[u8], table_range: &Range<usize>) -> Result<usize> {
    let elems = index_elements(xml)?;
    let table = element_at(&elems, table_range)?;
    if let Some(row) = elems
        .iter()
        .find(|elem| elem.parent == Some(table.id) && elem.word && elem.local == b"tr")
    {
        let count = elems
            .iter()
            .filter(|elem| elem.parent == Some(row.id) && elem.word && elem.local == b"tc")
            .count();
        return Ok(count.max(1));
    }
    if let Some(grid) = elems
        .iter()
        .find(|elem| elem.parent == Some(table.id) && elem.word && elem.local == b"tblGrid")
    {
        let count = elems
            .iter()
            .filter(|elem| elem.parent == Some(grid.id) && elem.word && elem.local == b"gridCol")
            .count();
        if count > 0 {
            return Ok(count);
        }
    }
    Ok(1)
}

pub(super) fn insert_into_element(
    xml: &[u8],
    range: &Range<usize>,
    payload: &[u8],
) -> Result<Vec<u8>> {
    let elems = index_elements(xml)?;
    let elem = element_at(&elems, range)?;
    if elem.inner_end.is_none() {
        return expand_empty(&xml[elem.range.clone()], payload);
    }
    let inner_end = elem.inner_end.unwrap_or(elem.range.end);
    let mut updated = xml[elem.range.start..inner_end].to_vec();
    updated.extend_from_slice(payload);
    updated.extend_from_slice(&xml[inner_end..elem.range.end]);
    Ok(updated)
}

pub(super) fn expand_empty(element: &[u8], payload: &[u8]) -> Result<Vec<u8>> {
    let qname = element_qname(element)?;
    let attributes_end = oxoffice_core::xml_element::start_tag(element)
        .map_err(|_| Error::Other("content control empty element is not closed".to_owned()))?
        .attributes_end;
    let mut updated = element[..attributes_end].to_vec();
    while updated.last().is_some_and(u8::is_ascii_whitespace) {
        updated.pop();
    }
    updated.push(b'>');
    updated.extend_from_slice(payload);
    updated.extend_from_slice(b"</");
    updated.extend_from_slice(qname.as_bytes());
    updated.push(b'>');
    Ok(updated)
}

pub(super) fn element_qname(bytes: &[u8]) -> Result<&str> {
    let rest = bytes
        .strip_prefix(b"<")
        .ok_or_else(|| Error::Other("content control element is not a start tag".to_owned()))?;
    let end = rest
        .iter()
        .position(|byte| byte.is_ascii_whitespace() || *byte == b'/' || *byte == b'>')
        .ok_or_else(|| Error::Other("content control element name is missing".to_owned()))?;
    std::str::from_utf8(&rest[..end])
        .map_err(|error| Error::Other(format!("content control element name is invalid: {error}")))
}

pub(super) fn direct_children(elems: &[XmlElem], parent: usize) -> impl Iterator<Item = &XmlElem> {
    elems.iter().filter(move |elem| elem.parent == Some(parent))
}

pub(super) fn direct_child<'a>(
    elems: &'a [XmlElem],
    parent: usize,
    word: bool,
    w14: bool,
    local: &[u8],
) -> Option<&'a XmlElem> {
    direct_children(elems, parent).find(|elem| {
        elem.local.as_slice() == local && if w14 { elem.w14 } else { !word || elem.word }
    })
}

pub(super) fn ancestor_local(elems: &[XmlElem], mut parent: Option<usize>, local: &[u8]) -> bool {
    while let Some(id) = parent {
        let Some(current) = elems.iter().find(|elem| elem.id == id) else {
            return false;
        };
        if current.local.as_slice() == local {
            return true;
        }
        parent = current.parent;
    }
    false
}
