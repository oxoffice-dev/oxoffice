//! Namespace prefixes and bindings a compared fragment needs from its root.

use super::*;

pub(super) fn root_prefix(xml: &str, local: &str) -> Result<String> {
    let suffix = format!(":{local}");
    let suffix_at = xml
        .find(&suffix)
        .ok_or_else(|| Error::Other(format!("comparison {local} root has no prefix")))?;
    let name_start = xml[..suffix_at]
        .rfind('<')
        .map(|at| at + 1)
        .ok_or_else(|| Error::Other(format!("comparison {local} root has no start")))?;
    Ok(xml[name_start..suffix_at].to_owned())
}

/// The range of the content of the `local` owner that `xml` starts with,
/// whatever its prefix. An owner written self-closing has no content: its
/// range is empty, at the end of its tag, so a reader reads it as empty. A
/// writer gives it an end tag before writing content in it
/// ([`replace_owner_inner`]).
pub(super) fn element_inner_range_any_prefix(xml: &str, local: &str) -> Result<Range<usize>> {
    let (open_end, empty, name) = element_opening(xml)?;
    if name.rsplit(':').next() != Some(local) {
        return Err(Error::Other(format!(
            "{local} source has a different owner name"
        )));
    }
    if empty {
        return Ok(open_end..open_end);
    }
    let close = format!("</{name}>");
    let close_start = xml
        .rfind(&close)
        .ok_or_else(|| Error::Other(format!("{local} source has no end tag")))?;
    Ok(open_end..close_start)
}

pub(super) fn drawing_signature(
    drawing: &oxoffice_docx_oxml::drawing::CT_Drawing,
) -> oxoffice_docx_oxml::drawing::CT_Drawing {
    let mut signature = drawing.clone();
    if let Some(inline) = &mut signature.inline {
        inline.raw_xml = None;
    }
    if let Some(anchor) = &mut signature.anchor {
        anchor.raw_xml = None;
        let mut prefixes = HashSet::new();
        if anchor
            .source_namespace_bindings
            .iter()
            .all(|(prefix, _)| prefixes.insert(prefix))
        {
            anchor.source_namespace_bindings.sort();
        }
    }
    signature
}

pub(super) fn required_external_bindings(
    xml: &[u8],
    external: &std::collections::BTreeMap<String, String>,
) -> Result<std::collections::BTreeMap<String, String>> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut scopes = Vec::<HashSet<String>>::new();
    let mut required = std::collections::BTreeMap::new();
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer).map_err(|error| {
            Error::Other(format!("comparison drawing binding scan failed: {error}"))
        })? {
            Event::Start(element) => {
                let declarations = declared_prefixes(&element)?;
                record_required_bindings(&element, &scopes, &declarations, external, &mut required);
                scopes.push(declarations);
            }
            Event::Empty(element) => {
                let declarations = declared_prefixes(&element)?;
                record_required_bindings(&element, &scopes, &declarations, external, &mut required);
            }
            Event::End(_) => {
                scopes.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(required)
}

pub(super) fn declared_prefixes(element: &BytesStart<'_>) -> Result<HashSet<String>> {
    let mut declarations = HashSet::new();
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| {
            Error::Other(format!("comparison drawing namespace failed: {error}"))
        })?;
        let name = attribute.key.as_ref();
        if name == b"xmlns" {
            declarations.insert(String::new());
        } else if let Some(prefix) = name.strip_prefix(b"xmlns:") {
            declarations.insert(std::str::from_utf8(prefix).map_err(utf8_error)?.to_owned());
        }
    }
    Ok(declarations)
}

fn record_required_bindings(
    element: &BytesStart<'_>,
    scopes: &[HashSet<String>],
    declarations: &HashSet<String>,
    external: &std::collections::BTreeMap<String, String>,
    required: &mut std::collections::BTreeMap<String, String>,
) {
    record_required_prefix(
        qualified_prefix(element.name().as_ref()).unwrap_or(""),
        scopes,
        declarations,
        external,
        required,
    );
    for attribute in element.attributes().filter_map(|attribute| attribute.ok()) {
        let name = attribute.key.as_ref();
        if name == b"xmlns" || name.starts_with(b"xmlns:") {
            continue;
        }
        if let Some(prefix) = qualified_prefix(name) {
            record_required_prefix(prefix, scopes, declarations, external, required);
        }
    }
}

fn record_required_prefix(
    prefix: &str,
    scopes: &[HashSet<String>],
    declarations: &HashSet<String>,
    external: &std::collections::BTreeMap<String, String>,
    required: &mut std::collections::BTreeMap<String, String>,
) {
    let internally_bound =
        declarations.contains(prefix) || scopes.iter().rev().any(|scope| scope.contains(prefix));
    if !internally_bound && let Some(namespace) = external.get(prefix) {
        required
            .entry(prefix.to_owned())
            .or_insert_with(|| namespace.clone());
    }
}

fn qualified_prefix(name: &[u8]) -> Option<&str> {
    let separator = name.iter().position(|byte| *byte == b':')?;
    std::str::from_utf8(&name[..separator]).ok()
}

pub(super) fn foreign_word_binding(xml: &[u8]) -> Result<bool> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("comparison namespace scan failed: {error}")))?
        {
            Event::Start(element) | Event::Empty(element) => {
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|error| {
                        Error::Other(format!("comparison namespace attribute failed: {error}"))
                    })?;
                    if attribute.key.as_ref() == b"xmlns:w"
                        && attribute
                            .decoded_and_normalized_value(
                                XmlVersion::Implicit1_0,
                                element.decoder(),
                            )
                            .map_err(|error| {
                                Error::Other(format!("comparison namespace value failed: {error}"))
                            })?
                            .as_ref()
                            != W_NS
                    {
                        return Ok(true);
                    }
                }
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
        buffer.clear();
    }
}

/// Read actual inherited/root bindings without inventing a serializer prefix.
pub(super) fn actual_root_bindings(
    xml: &str,
    inherited: &NamespaceBindings,
) -> Result<NamespaceBindings> {
    let mut root = BytesStart::new("scope");
    for (name, uri) in inherited {
        root.push_attribute((name.as_str(), uri.as_str()));
    }
    let mut writer = Writer::new(Vec::new());
    writer
        .write_event(Event::Start(root))
        .map_err(|error| Error::Other(error.to_string()))?;
    let offset = writer.get_ref().len();
    writer.get_mut().extend_from_slice(xml.as_bytes());
    writer.get_mut().extend_from_slice(b"</scope>");
    let source = writer.into_inner();
    let mut reader = NsReader::from_reader(source.as_slice());
    loop {
        let before = reader.buffer_position() as usize;
        let (_, event) = reader
            .read_resolved_event()
            .map_err(|error| Error::Other(error.to_string()))?;
        if before == offset && matches!(event, Event::Start(_) | Event::Empty(_)) {
            return resolved_source_bindings(&reader);
        }
        if matches!(event, Event::Eof) {
            return Err(Error::Other("actual source root is absent".into()));
        }
    }
}
