//! XML the model keeps verbatim: where it sits and what its raw elements declare.

use super::*;

enum UnsupportedXmlSource<'a> {
    Raw { raw: &'a [u8] },
    Modeled,
}

/// A borrowed fact for content the compatibility reader does not model.
pub struct UnsupportedXmlRef<'a> {
    source: UnsupportedXmlSource<'a>,
    qualified_name: Option<Cow<'a, str>>,
    local_name: Cow<'a, str>,
    namespace_uri: Option<Cow<'a, str>>,
    has_child_content: bool,
}

impl<'a> UnsupportedXmlRef<'a> {
    /// Inspect preserved reader-item bytes using the same unsupported XML
    /// projection as body compatibility items.
    pub fn from_bytes(raw: &'a [u8]) -> Self {
        Self::raw(raw, &[])
    }

    pub(super) fn raw(raw: &'a [u8], inherited_namespaces: &'a [(String, String)]) -> Self {
        let names = raw_element_names(raw);
        let namespace_uri = names.as_ref().and_then(|(name, _)| {
            let prefix = name.split_once(':').map_or("", |(prefix, _)| prefix);
            raw_namespace_declaration(raw, prefix)
                .map(Cow::Owned)
                .or_else(|| inherited_namespace(inherited_namespaces, prefix).map(Cow::Borrowed))
        });
        let (qualified_name, local_name) = names.map_or_else(
            || (None, Cow::Borrowed("")),
            |(qualified_name, local_name)| {
                (Some(Cow::Owned(qualified_name)), Cow::Owned(local_name))
            },
        );
        Self {
            source: UnsupportedXmlSource::Raw { raw },
            qualified_name,
            local_name,
            namespace_uri,
            has_child_content: raw_has_child_content(raw),
        }
    }

    pub(super) fn modeled(
        qualified_name: &'static str,
        namespace_uri: &'static str,
        local_name: &'static str,
        has_child_content: bool,
    ) -> Self {
        Self {
            source: UnsupportedXmlSource::Modeled,
            qualified_name: Some(Cow::Borrowed(qualified_name)),
            local_name: Cow::Borrowed(local_name),
            namespace_uri: Some(Cow::Borrowed(namespace_uri)),
            has_child_content,
        }
    }

    /// Original subtree bytes, or `None` for a modeled compatibility fact.
    pub fn raw_xml(&self) -> Option<&'a [u8]> {
        match self.source {
            UnsupportedXmlSource::Raw { raw } => Some(raw),
            UnsupportedXmlSource::Modeled => None,
        }
    }

    /// Qualified element name, when the retained XML has a valid root element.
    pub fn qualified_name(&self) -> Option<&str> {
        self.qualified_name.as_deref()
    }

    /// Local element name.
    pub fn local_name(&self) -> &str {
        &self.local_name
    }

    /// Namespace URI resolved from local or inherited declarations.
    pub fn namespace_uri(&self) -> Option<&str> {
        self.namespace_uri.as_deref()
    }

    /// Whether the retained element has a child element or visible text.
    pub fn has_child_content(&self) -> bool {
        self.has_child_content
    }
}

fn raw_element_names(raw: &[u8]) -> Option<(String, String)> {
    let mut reader = oxoffice_core::xml_reader::Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer).ok()? {
            Event::Start(element) | Event::Empty(element) => {
                let qualified_name = std::str::from_utf8(element.name().as_ref())
                    .ok()?
                    .to_owned();
                let local_name = std::str::from_utf8(element.local_name().as_ref())
                    .ok()?
                    .to_owned();
                return Some((qualified_name, local_name));
            }
            Event::Eof => return None,
            _ => buffer.clear(),
        }
    }
}

fn raw_namespace_declaration(raw: &[u8], prefix: &str) -> Option<String> {
    let mut reader = oxoffice_core::xml_reader::Reader::from_reader(raw);
    let mut buffer = Vec::new();
    let declaration = if prefix.is_empty() {
        "xmlns".to_owned()
    } else {
        format!("xmlns:{prefix}")
    };
    loop {
        match reader.read_event_into(&mut buffer).ok()? {
            Event::Start(element) | Event::Empty(element) => {
                let attribute = element
                    .attributes()
                    .flatten()
                    .find(|attribute| attribute.key.as_ref() == declaration.as_bytes())?;
                return attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                    .ok()
                    .map(Cow::into_owned);
            }
            Event::Eof => return None,
            _ => buffer.clear(),
        }
    }
}

pub(super) fn inherited_namespace<'a>(
    namespaces: &'a [(String, String)],
    prefix: &str,
) -> Option<&'a str> {
    if prefix == "xml" {
        return Some(XML_NAMESPACE);
    }
    let declaration = if prefix.is_empty() {
        "xmlns".to_owned()
    } else {
        format!("xmlns:{prefix}")
    };
    namespaces
        .iter()
        .rev()
        .find(|(name, _)| name == &declaration)
        .map(|(_, uri)| uri.as_str())
}

fn raw_has_child_content(raw: &[u8]) -> bool {
    let mut reader = oxoffice_core::xml_reader::Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(_)) => {
                if depth > 0 {
                    return true;
                }
                depth += 1;
            }
            Ok(Event::Empty(_)) if depth > 0 => return true,
            Ok(Event::Text(text)) if depth > 0 && !xml_text_is_whitespace(&text) => {
                return true;
            }
            Ok(Event::CData(text)) if depth > 0 && !xml_bytes_are_whitespace(text.as_ref()) => {
                return true;
            }
            Ok(Event::GeneralRef(reference)) if depth > 0 => match reference.resolve_char_ref() {
                Ok(Some(character)) if !character.is_ascii_whitespace() => return true,
                Ok(Some(_)) => {}
                Ok(None) => return true,
                Err(_) => return false,
            },
            Ok(Event::End(_)) => depth = depth.saturating_sub(1),
            Ok(Event::Eof) | Err(_) => return false,
            _ => {}
        }
        buffer.clear();
    }
}

fn xml_text_is_whitespace(text: &quick_xml::events::BytesText<'_>) -> bool {
    xml_bytes_are_whitespace(text.as_ref())
}

pub(super) fn xml_bytes_are_whitespace(bytes: &[u8]) -> bool {
    bytes.iter().all(|byte| byte.is_ascii_whitespace())
}

pub(super) fn typed_numbering_leaf_has_unmodeled(raw: &[u8], word_prefixes: &[String]) -> bool {
    if raw_has_child_content(raw) {
        return true;
    }
    let mut reader = oxoffice_core::xml_reader::Reader::from_reader(raw);
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(element)) | Ok(Event::Empty(element)) => {
                let mut value_count = 0usize;
                for attribute in element.attributes() {
                    let Ok(attribute) = attribute else {
                        return true;
                    };
                    let name = attribute.key.as_ref();
                    if name == b"xmlns" || name.starts_with(b"xmlns:") {
                        continue;
                    }
                    let Some(separator) = name.iter().position(|byte| *byte == b':') else {
                        return true;
                    };
                    let prefix = &name[..separator];
                    let local = &name[separator + 1..];
                    if local != b"val"
                        || !word_prefixes
                            .iter()
                            .any(|candidate| candidate.as_bytes() == prefix)
                    {
                        return true;
                    }
                    value_count += 1;
                }
                return value_count != 1;
            }
            Ok(Event::Eof) | Err(_) => return true,
            _ => buffer.clear(),
        }
    }
}
