//! Namespace prefixes and bindings a compared fragment needs from its root.

use super::*;

/// The prefix the comparison writes new WordprocessingML markup of the
/// `local` root's part under: the root's own; for a root written without a
/// prefix (WordprocessingML as the default namespace, #663), the first
/// prefix the root binds to WordprocessingML, else `w`, which the writer
/// declares where it writes it. A root without a prefix that binds `w` to
/// another namespace and no prefix to WordprocessingML has none.
pub(super) fn root_prefix(xml: &str, local: &str) -> Result<String> {
    let bindings = root_namespace_bindings(xml, local, &[])?;
    let mut reader = Reader::from_str(xml);
    let name = loop {
        match reader
            .read_event()
            .map_err(|error| Error::Other(format!("comparison {local} root: {error}")))?
        {
            Event::Start(element) | Event::Empty(element) => {
                break std::str::from_utf8(element.name().as_ref())
                    .map_err(utf8_error)?
                    .to_owned();
            }
            Event::Eof => return Err(Error::Other(format!("comparison {local} root is missing"))),
            _ => {}
        }
    };
    if let Some((prefix, _)) = name.split_once(':') {
        return Ok(prefix.to_owned());
    }
    let bound = bindings.iter().find_map(|(key, uri)| {
        key.strip_prefix("xmlns:")
            .filter(|_| uri == W_NS)
            .map(str::to_owned)
    });
    match bound {
        Some(prefix) => Ok(prefix),
        None if !bindings.iter().any(|(key, _)| key == "xmlns:w") => Ok("w".to_owned()),
        None => Err(Error::Other(format!(
            "comparison {local} root binds no prefix to WordprocessingML and `w` to another namespace"
        ))),
    }
}

/// The start tag of the root a fragment of a Word part is scanned under:
/// WordprocessingML bound to `word_prefix` and as the default namespace, so
/// that an element its part writes without a prefix under a default
/// namespace (#663) is found as one written with it. The scans look for
/// WordprocessingML elements by their local name (`txbxContent`, `r`,
/// `fldSimple`) and the comparison's private markers, which no other
/// vocabulary of a Word part writes there.
pub(super) fn scan_root_start(word_prefix: &str) -> String {
    format!(
        r#"<rdocxcmp:root xmlns:rdocxcmp="urn:rdocx-compare" xmlns:{word_prefix}="{W_NS}" xmlns="{W_NS}">"#
    )
}

/// The range of the content of the `local` owner that `xml` starts with,
/// whatever its prefix. An owner written self-closing has no content: its
/// range is empty, at the end of its tag, so a reader reads it as empty. A
/// writer gives it an end tag before writing content in it
/// ([`replace_owner_inner`]).
pub(super) fn element_inner_range_any_prefix(xml: &str, local: &str) -> Result<Range<usize>> {
    let span = oxoffice_core::xml_element::element_span(xml)
        .map_err(|error| Error::Other(format!("{local} source: {error}")))?;
    if span.local_name() != local {
        return Err(Error::Other(format!(
            "{local} source has a different owner name"
        )));
    }
    Ok(span.content)
}

pub(super) fn drawing_signature(
    drawing: &oxoffice_docx_oxml::drawing::CT_Drawing,
) -> oxoffice_docx_oxml::drawing::CT_Drawing {
    let mut signature = drawing.clone();
    if let Some(inline) = &mut signature.inline {
        inline.raw_xml = None;
    }
    if let Some(anchor) = &mut signature.anchor {
        // The bindings the anchor reads, wherever they are declared: out of
        // the anchor (its inherited scope) or in it, as a writer that
        // declares a prefix on the anchor rather than on the drawing does.
        for (prefix, uri) in anchor
            .raw_xml
            .as_deref()
            .map(declared_bindings)
            .unwrap_or_default()
        {
            if !anchor
                .source_namespace_bindings
                .iter()
                .any(|(known, _)| *known == prefix)
            {
                anchor.source_namespace_bindings.push((prefix, uri));
            }
        }
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

/// Each prefix `xml` declares, on any element, with the first namespace it
/// is bound to.
fn declared_bindings(xml: &[u8]) -> Vec<(String, String)> {
    let mut reader = Reader::from_reader(xml);
    let mut bindings = Vec::<(String, String)>::new();
    let mut buffer = Vec::new();
    while let Ok(event) = reader.read_event_into(&mut buffer) {
        match event {
            Event::Start(element) | Event::Empty(element) => {
                for attribute in element.attributes().flatten() {
                    let name = attribute.key.as_ref();
                    let prefix = if name == b"xmlns" {
                        Some(&b""[..])
                    } else {
                        name.strip_prefix(b"xmlns:")
                    };
                    if let (Some(prefix), Ok(uri)) =
                        (prefix, oxoffice_core::xml::attribute_text(&attribute))
                        && let Ok(prefix) = std::str::from_utf8(prefix)
                        && !bindings.iter().any(|(known, _)| known == prefix)
                    {
                        bindings.push((prefix.to_owned(), uri.into_owned()));
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    bindings
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

#[cfg(test)]
mod tests {
    use super::*;

    // New markup goes under the root's prefix, under the first prefix a root
    // written with the default namespace binds to WordprocessingML, else
    // under `w`, which the writer declares; never under a `w` bound to
    // another namespace.
    #[test]
    fn new_markup_takes_the_prefix_the_root_binds_to_wordprocessingml() {
        let w = W_NS;
        for (root, expected) in [
            (format!(r#"<w:hdr xmlns:w="{w}"/>"#), Ok("w")),
            (format!(r#"<ox0:hdr xmlns:ox0="{w}"/>"#), Ok("ox0")),
            (format!(r#"<hdr xmlns="{w}" xmlns:w="{w}"/>"#), Ok("w")),
            (format!(r#"<hdr xmlns="{w}" xmlns:p1="{w}"/>"#), Ok("p1")),
            (format!(r#"<hdr xmlns="{w}"/>"#), Ok("w")),
            (
                format!(r#"<hdr xmlns="{w}" xmlns:w="urn:x"/>"#),
                Err("binds no prefix"),
            ),
        ] {
            let found = root_prefix(&root, "hdr");
            match expected {
                Ok(prefix) => assert_eq!(found.ok().as_deref(), Some(prefix), "{root}"),
                Err(message) => assert!(
                    found.is_err_and(|error| error.to_string().contains(message)),
                    "{root}"
                ),
            }
        }
    }
}
