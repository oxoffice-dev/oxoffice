//! Namespace declarations in scope and namespace-qualified attributes: reading them, and rewriting one with a prefix in scope, declared when none is.

use super::*;

pub(super) fn in_scope_xmlns(xml: &[u8], start: usize) -> Result<String> {
    Ok(in_scope_declarations(xml, start)?
        .into_iter()
        .map(|(name, uri)| format!(" {name}=\"{}\"", xml_attr(&uri)))
        .collect())
}

/// The namespace declarations in scope at the element starting at `start`,
/// its own included, as attribute names (`xmlns`, `xmlns:w`) and namespaces.
pub(super) fn in_scope_declarations(xml: &[u8], start: usize) -> Result<Vec<(String, String)>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (_namespace, _event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| form_fault(format!("content control XML is invalid: {error}")))?;
        if before == start {
            let mut declarations = Vec::new();
            for (declaration, namespace) in reader.resolver().bindings() {
                let uri = String::from_utf8_lossy(namespace.0).into_owned();
                match declaration {
                    quick_xml::name::PrefixDeclaration::Default => {
                        declarations.push(("xmlns".to_owned(), uri));
                    }
                    quick_xml::name::PrefixDeclaration::Named(prefix) => {
                        let prefix = String::from_utf8_lossy(prefix);
                        if prefix == "xml" {
                            continue;
                        }
                        declarations.push((format!("xmlns:{prefix}"), uri));
                    }
                }
            }
            return Ok(declarations);
        }
        if before > start {
            return Err(form_fault("content control attribute position was missed"));
        }
        buffer.clear();
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AttrNs {
    Word,
    W14,
    Xml,
}

pub(super) struct BoundAttr {
    namespace: Option<AttrNs>,
    local: Vec<u8>,
    remove_start: usize,
    remove_end: usize,
    value_start: usize,
    value_end: usize,
}

pub(super) fn rewrite_qualified_attribute(
    xml: &mut Vec<u8>,
    start: usize,
    namespace: AttrNs,
    local: &str,
    value: Option<&str>,
) -> Result<()> {
    let (attributes, prefix, declare) = qualified_attributes(xml, start, namespace)?;
    let matches = attributes
        .iter()
        .filter(|attr| attr.namespace == Some(namespace) && attr.local == local.as_bytes())
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        return Err(form_fault("duplicate qualified attribute"));
    }
    if let Some(found) = matches.first() {
        if let Some(value) = value {
            let escaped = xml_attr(value).into_bytes();
            xml.splice(found.value_start..found.value_end, escaped);
        } else {
            xml.splice(found.remove_start..found.remove_end, Vec::new());
        }
        return Ok(());
    }
    let Some(value) = value else {
        return Ok(());
    };
    let elems = index_elements(xml)?;
    let elem = elems
        .iter()
        .find(|elem| elem.range.start == start)
        .ok_or_else(|| form_fault("stale form target"))?;
    let mut tag = xml[elem.range.start..elem.open_end].to_vec();
    let attribute = match declare {
        Some(uri) => format!(
            " xmlns:{prefix}=\"{uri}\" {prefix}:{local}=\"{}\"",
            xml_attr(value)
        ),
        None => format!(" {prefix}:{local}=\"{}\"", xml_attr(value)),
    };
    insert_into_start_tag(&mut tag, &attribute)?;
    let open = elem.open_end;
    xml.splice(elem.range.start..open, tag);
    Ok(())
}

pub(super) fn qualified_attributes(
    xml: &[u8],
    start: usize,
    wanted: AttrNs,
) -> Result<(Vec<BoundAttr>, String, Option<&'static str>)> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (_namespace, event) =
            reader
                .read_resolved_event_into(&mut buffer)
                .map_err(|error| {
                    form_fault(format!("content control attribute is invalid: {error}"))
                })?;
        if before == start {
            let (Event::Start(element) | Event::Empty(element)) = event else {
                return Ok((Vec::new(), String::new(), None));
            };
            let element_bytes: &[u8] = &element;
            let element_ptr = element_bytes.as_ptr() as usize;
            let mut attributes = Vec::new();
            for attribute in element.attributes() {
                let attribute = attribute.map_err(|error| {
                    form_fault(format!("content control attribute is invalid: {error}"))
                })?;
                let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                let std::borrow::Cow::Borrowed(raw_value) = &attribute.value else {
                    return Err(form_fault("content control attribute is invalid"));
                };
                let value_ptr = raw_value.as_ptr() as usize;
                let key_ptr = attribute.key.as_ref().as_ptr() as usize;
                if value_ptr < element_ptr || key_ptr < element_ptr {
                    return Err(form_fault("content control attribute is invalid"));
                }
                let value_start = before + 1 + (value_ptr - element_ptr);
                let value_end = value_start + raw_value.len();
                let key_start = before + 1 + (key_ptr - element_ptr);
                let quote = *xml
                    .get(
                        value_start
                            .checked_sub(1)
                            .ok_or_else(|| form_fault("content control attribute is invalid"))?,
                    )
                    .ok_or_else(|| form_fault("content control attribute is invalid"))?;
                if quote != b'"' && quote != b'\'' {
                    return Err(form_fault("content control attribute is invalid"));
                }
                if xml.get(value_end) != Some(&quote) {
                    return Err(form_fault("content control attribute is invalid"));
                }
                let mut remove_start = key_start;
                while remove_start > before && xml[remove_start - 1].is_ascii_whitespace() {
                    remove_start -= 1;
                }
                attributes.push(BoundAttr {
                    namespace: attr_namespace(&namespace),
                    local: local.as_ref().to_vec(),
                    remove_start,
                    remove_end: value_end + 1,
                    value_start,
                    value_end,
                });
            }
            let (prefix, declare) = attribute_prefix(&reader, &element, wanted)?;
            return Ok((attributes, prefix, declare));
        }
        if before > start {
            return Err(form_fault("content control attribute position was missed"));
        }
        buffer.clear();
    }
}

fn attr_namespace(namespace: &ResolveResult<'_>) -> Option<AttrNs> {
    match namespace {
        ResolveResult::Bound(Namespace(uri)) if *uri == W_NS.as_bytes() => Some(AttrNs::Word),
        ResolveResult::Bound(Namespace(uri)) if *uri == W14_NS.as_bytes() => Some(AttrNs::W14),
        ResolveResult::Bound(Namespace(uri)) if *uri == b"http://www.w3.org/XML/1998/namespace" => {
            Some(AttrNs::Xml)
        }
        _ => None,
    }
}

fn attribute_prefix(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
    namespace: AttrNs,
) -> Result<(String, Option<&'static str>)> {
    if let Some(prefix) = prefix_in_scope(reader, namespace) {
        return Ok((prefix, None));
    }
    let (preferred, uri) = match namespace {
        AttrNs::Word => ("w", W_NS),
        AttrNs::W14 => ("w14", W14_NS),
        AttrNs::Xml => ("xml", "http://www.w3.org/XML/1998/namespace"),
    };
    if !prefix_captured(reader, element, preferred) {
        return Ok((preferred.to_owned(), Some(uri)));
    }
    let mut index = 2u32;
    loop {
        let candidate = format!("{preferred}{index}");
        if !prefix_captured(reader, element, &candidate) {
            return Ok((candidate, Some(uri)));
        }
        index = index
            .checked_add(1)
            .ok_or_else(|| form_fault("content control namespace prefix is exhausted"))?;
    }
}

fn prefix_captured(reader: &NsReader<&[u8]>, element: &BytesStart<'_>, name: &str) -> bool {
    let name_bytes = name.as_bytes();
    if reader.resolver().bindings().any(|(declaration, _)| {
        matches!(
            declaration,
            quick_xml::name::PrefixDeclaration::Named(prefix) if prefix == name_bytes
        )
    }) {
        return true;
    }
    element
        .attributes()
        .with_checks(false)
        .flatten()
        .any(|attribute| {
            let key = attribute.key.as_ref();
            key.starts_with(b"xmlns:") && &key[b"xmlns:".len()..] == name_bytes
                || key.split(|byte| *byte == b':').next() == Some(name_bytes)
        })
}

fn prefix_in_scope(reader: &NsReader<&[u8]>, namespace: AttrNs) -> Option<String> {
    if namespace == AttrNs::Xml {
        return Some("xml".to_owned());
    }
    let wanted: &[u8] = match namespace {
        AttrNs::Word => W_NS.as_bytes(),
        AttrNs::W14 => W14_NS.as_bytes(),
        AttrNs::Xml => b"http://www.w3.org/XML/1998/namespace",
    };
    for (declaration, bound) in reader.resolver().bindings() {
        if bound.0 == wanted
            && let quick_xml::name::PrefixDeclaration::Named(prefix) = declaration
        {
            return Some(String::from_utf8_lossy(prefix).into_owned());
        }
    }
    None
}

pub(super) fn insert_into_start_tag(tag: &mut Vec<u8>, attribute: &str) -> Result<()> {
    let end = tag
        .iter()
        .rposition(|byte| *byte == b'>')
        .ok_or_else(|| form_fault("content control start tag is not closed"))?;
    let at = if end > 0 && tag[end - 1] == b'/' {
        end - 1
    } else {
        end
    };
    tag.splice(at..at, attribute.as_bytes().iter().copied());
    Ok(())
}
