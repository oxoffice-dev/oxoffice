use std::collections::HashMap;

use oxoffice_core::OxmlError;
use oxoffice_core::xml::{escape_attribute_value, push_escaped_attribute};
use oxoffice_drawing::namespace::A_NS;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, Writer, XmlVersion};

/// PresentationML main namespace URI.
pub const P_NS: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";

/// Fixed PresentationML prefix used when writing XML.
pub const P_PREFIX: &str = "p";

/// Office document relationships namespace URI.
pub const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// Fixed office document relationships prefix used when writing XML.
pub const R_PREFIX: &str = "r";

/// Markup Compatibility namespace URI.
pub const MC_NS: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";

pub(crate) const FIXED_MODEL_PREFIXES: &[&str] = &["p", "a", "r"];
pub(crate) const FIXED_SHAPE_TREE_PREFIXES: &[&str] = &["p", "a", "r", "mc"];

#[derive(Clone, Debug, Default)]
pub(crate) struct NamespaceBindings {
    default: Option<String>,
    prefixes: HashMap<String, String>,
}

impl NamespaceBindings {
    pub(crate) fn from_entries(entries: &[(String, String)]) -> Self {
        let mut bindings = Self::default();
        for (prefix, uri) in entries {
            if prefix.is_empty() {
                bindings.default = Some(uri.clone());
            } else {
                bindings.prefixes.insert(prefix.clone(), uri.clone());
            }
        }
        bindings
    }

    pub(crate) fn with_start(&self, start: &BytesStart<'_>) -> Result<Self, OxmlError> {
        let mut bindings = self.clone();
        for (name, value) in all_attributes(start)? {
            if name == "xmlns" {
                bindings.default = Some(value);
            } else if let Some(prefix) = name.strip_prefix("xmlns:") {
                bindings.prefixes.insert(prefix.to_owned(), value);
            }
        }
        Ok(bindings)
    }

    pub(crate) fn element_uri<'a>(&'a self, name: &[u8]) -> Option<&'a str> {
        match qname_prefix(name) {
            Some(prefix) => self.prefixes.get(prefix).map(String::as_str),
            None => self.default.as_deref(),
        }
    }

    pub(crate) fn attribute_uri<'a>(&'a self, name: &[u8]) -> Option<&'a str> {
        qname_prefix(name).and_then(|prefix| self.prefixes.get(prefix).map(String::as_str))
    }

    pub(crate) fn entries(&self) -> Vec<(String, String)> {
        let mut entries: Vec<_> = self
            .prefixes
            .iter()
            .map(|(prefix, uri)| (prefix.clone(), uri.clone()))
            .collect();
        sort_namespace_entries(&mut entries);
        if let Some(uri) = &self.default {
            entries.push((String::new(), uri.clone()));
        }
        entries
    }

    pub(crate) fn reject_writer_conflicts(&self, fixed_prefixes: &[&str]) -> Result<(), OxmlError> {
        for prefix in fixed_prefixes {
            let expected = canonical_uri(prefix).expect("fixed prefixes have canonical URIs");
            if let Some(actual) = self.prefixes.get(*prefix)
                && actual != expected
            {
                return Err(OxmlError::InvalidValue(format!(
                    "xmlns:{prefix} conflicts with the fixed writer namespace"
                )));
            }
        }
        Ok(())
    }
}

/// Returns every attribute of `start`, its value normalised as XML requires:
/// a raw tab, line feed or carriage return reads as a space, while one written
/// as a character reference (`&#9;`) is kept.
pub(crate) fn all_attributes(start: &BytesStart<'_>) -> Result<Vec<(String, String)>, OxmlError> {
    let mut attributes = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute?;
        let name = std::str::from_utf8(attribute.key.as_ref())?.to_owned();
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())?
            .into_owned();
        attributes.push((name, value));
    }
    Ok(attributes)
}

pub(crate) fn non_visual_drawing_id(xml: &[u8]) -> Option<u32> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer).ok()? {
            Event::Start(start) | Event::Empty(start) => {
                return all_attributes(&start)
                    .ok()?
                    .into_iter()
                    .find(|(name, _)| name == "id")
                    .and_then(|(_, value)| value.parse().ok());
            }
            Event::Eof => return None,
            _ => {}
        }
        buffer.clear();
    }
}

pub(crate) fn non_visual_drawing_name(start: &BytesStart<'_>) -> Result<Option<String>, OxmlError> {
    Ok(all_attributes(start)?
        .into_iter()
        .find(|(name, _)| name == "name")
        .map(|(_, value)| value))
}

pub(crate) fn set_non_visual_drawing_name(xml: &mut Vec<u8>, name: &str) -> Result<(), OxmlError> {
    oxoffice_core::xml::reject_non_xml_characters("shape name", name)?;
    let mut reader = Reader::from_reader(xml.as_slice());
    let mut buffer = Vec::new();
    loop {
        let event = reader.read_event_into(&mut buffer)?;
        let is_empty = matches!(&event, Event::Empty(_));
        match event {
            Event::Start(start) | Event::Empty(start) => {
                let end = reader.buffer_position() as usize;
                let qualified_name = std::str::from_utf8(start.name().as_ref())?.to_owned();
                let mut replacement = BytesStart::new(qualified_name);
                let mut replaced = false;
                for attribute in start.attributes().with_checks(false) {
                    let attribute = attribute?;
                    if attribute.key.as_ref() == b"name" {
                        push_escaped_attribute(&mut replacement, "name", name);
                        replaced = true;
                    } else {
                        replacement.push_attribute(attribute);
                    }
                }
                if !replaced {
                    push_escaped_attribute(&mut replacement, "name", name);
                }
                let mut writer = Writer::new(Vec::new());
                if is_empty {
                    writer.write_event(Event::Empty(replacement))?;
                } else {
                    writer.write_event(Event::Start(replacement))?;
                    writer.get_mut().extend_from_slice(&xml[end..]);
                }
                *xml = writer.into_inner();
                return Ok(());
            }
            Event::Eof => {
                return Err(OxmlError::MissingElement(
                    "non-visual drawing properties".to_owned(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    }
}

/// Reads an unqualified accessibility attribute from preserved drawing properties.
/// Read only an unqualified hidden attribute on the actual qualified drawing owner.
pub(crate) fn non_visual_drawing_hidden(xml: &[u8], inherited: &NamespaceBindings) -> bool {
    let mut reader = Reader::from_reader(xml);
    let Ok(Event::Start(start) | Event::Empty(start)) = reader.read_event() else {
        return false;
    };
    let Ok(bindings) = inherited.with_start(&start) else {
        return false;
    };
    bindings.element_uri(start.name().as_ref()) == Some(P_NS)
        && start.local_name().as_ref() == b"cNvPr"
        && matches!(
            non_visual_drawing_text(xml, "hidden").as_deref(),
            Some("true" | "1")
        )
}

pub(crate) fn non_visual_drawing_text(xml: &[u8], key: &str) -> Option<String> {
    let mut reader = Reader::from_reader(xml);
    match reader.read_event().ok()? {
        Event::Start(start) | Event::Empty(start) => {
            for attribute in start.attributes() {
                let attribute = attribute.ok()?;
                if attribute.key.as_ref() == key.as_bytes() {
                    return attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
                        .ok()
                        .map(|value| value.into_owned());
                }
            }
            None
        }
        _ => None,
    }
}

pub(crate) fn accessibility_parent_bindings(
    root: &[(String, String)],
    non_visual: &[(String, String)],
) -> NamespaceBindings {
    let mut bindings = NamespaceBindings::from_entries(&[
        ("a".to_owned(), A_NS.to_owned()),
        ("p".to_owned(), P_NS.to_owned()),
    ]);
    for (name, value) in root.iter().chain(non_visual) {
        if name == "xmlns" {
            bindings.default = Some(value.clone());
        } else if let Some(prefix) = name.strip_prefix("xmlns:") {
            bindings.prefixes.insert(prefix.to_owned(), value.clone());
        }
    }
    bindings
}

/// Edits only the owned attribute bytes, retaining other lexical XML verbatim.
pub(crate) fn set_non_visual_drawing_text(
    xml: &mut Vec<u8>,
    key: &str,
    value: Option<&str>,
) -> Result<(), OxmlError> {
    if let Some(value) = value {
        oxoffice_core::xml::reject_non_xml_characters("shape accessibility text", value)?;
    }
    let mut reader = Reader::from_reader(xml.as_slice());
    let event = reader.read_event()?;
    let start = match event {
        Event::Start(start) | Event::Empty(start) => start,
        _ => return Err(OxmlError::MissingElement("p:cNvPr".to_owned())),
    };
    // Validate the complete attribute set before publishing a replacement.
    all_attributes(&start)?;
    let end = reader.buffer_position() as usize;
    let mut cursor = 1;
    while cursor < end && !xml[cursor].is_ascii_whitespace() && !matches!(xml[cursor], b'>' | b'/')
    {
        cursor += 1;
    }
    let mut owned = None;
    while cursor < end {
        let whitespace = cursor;
        while cursor < end && xml[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor == end || matches!(xml[cursor], b'/' | b'>') {
            break;
        }
        let name_start = cursor;
        while cursor < end && !xml[cursor].is_ascii_whitespace() && xml[cursor] != b'=' {
            cursor += 1;
        }
        let name_end = cursor;
        while cursor < end && xml[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if xml.get(cursor) != Some(&b'=') {
            return Err(OxmlError::InvalidValue(
                "invalid drawing-property attribute".to_owned(),
            ));
        }
        cursor += 1;
        while cursor < end && xml[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let quote = *xml
            .get(cursor)
            .ok_or_else(|| OxmlError::InvalidValue("missing attribute quote".to_owned()))?;
        if !matches!(quote, b'\'' | b'"') {
            return Err(OxmlError::InvalidValue(
                "missing attribute quote".to_owned(),
            ));
        }
        cursor += 1;
        while cursor < end && xml[cursor] != quote {
            cursor += 1;
        }
        if cursor == end {
            return Err(OxmlError::InvalidValue("unterminated attribute".to_owned()));
        }
        cursor += 1;
        if &xml[name_start..name_end] == key.as_bytes() {
            owned = Some(whitespace..cursor);
        }
    }
    let replacement = value
        .map(|value| format!(" {key}=\"{}\"", escape_attribute_value(value)).into_bytes())
        .unwrap_or_default();
    let range = owned.unwrap_or_else(|| {
        let insertion = if xml.get(end.wrapping_sub(2)) == Some(&b'/') {
            end - 2
        } else {
            end - 1
        };
        insertion..insertion
    });
    let mut candidate = Vec::with_capacity(xml.len() + replacement.len());
    candidate.extend_from_slice(&xml[..range.start]);
    candidate.extend_from_slice(&replacement);
    candidate.extend_from_slice(&xml[range.end..]);
    *xml = candidate;
    Ok(())
}

const DECORATIVE_URI: &str = "{C183D7F6-B498-43B3-948B-1728B52AA6E4}";
const DECORATIVE_NS: &str = "http://schemas.microsoft.com/office/drawing/2017/decorative";

struct AccessibilityElementParts {
    start: Vec<u8>,
    children: Vec<Vec<u8>>,
    end: Vec<u8>,
}

/// Splits a preserved element without reserializing untouched child bytes.
fn accessibility_element_parts(xml: &[u8]) -> Result<AccessibilityElementParts, OxmlError> {
    let mut reader = Reader::from_reader(xml);
    let event = reader.read_event()?;
    let (name, empty) = match event {
        Event::Start(start) => (start.name().as_ref().to_vec(), false),
        Event::Empty(start) => (start.name().as_ref().to_vec(), true),
        _ => {
            return Err(OxmlError::MissingElement(
                "accessibility element".to_owned(),
            ));
        }
    };
    let start_end = reader.buffer_position() as usize;
    if empty {
        let mut start = xml[..start_end - 2].to_vec();
        start.push(b'>');
        return Ok(AccessibilityElementParts {
            start,
            children: Vec::new(),
            end: [b"</".as_slice(), &name, b">"].concat(),
        });
    }
    let mut children = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        match reader.read_event()? {
            Event::Start(start) => {
                reader.read_to_end(start.name())?;
            }
            Event::End(_) => {
                return Ok(AccessibilityElementParts {
                    start: xml[..start_end].to_vec(),
                    children,
                    end: xml[before..].to_vec(),
                });
            }
            Event::Eof => {
                return Err(OxmlError::MissingElement(
                    "accessibility end tag".to_owned(),
                ));
            }
            _ => {}
        }
        children.push(xml[before..reader.buffer_position() as usize].to_vec());
    }
}

fn accessibility_bindings(
    xml: &[u8],
    inherited: &NamespaceBindings,
) -> Result<NamespaceBindings, OxmlError> {
    let mut reader = Reader::from_reader(xml);
    match reader.read_event()? {
        Event::Start(start) | Event::Empty(start) => inherited.with_start(&start),
        _ => Ok(inherited.clone()),
    }
}

fn accessibility_is(
    xml: &[u8],
    inherited: &NamespaceBindings,
    uri: &str,
    local: &[u8],
) -> Result<bool, OxmlError> {
    let mut reader = Reader::from_reader(xml);
    match reader.read_event()? {
        Event::Start(start) | Event::Empty(start) => {
            let bindings = inherited.with_start(&start)?;
            Ok(bindings.element_uri(start.name().as_ref()) == Some(uri)
                && oxoffice_core::xml::local_name(start.name().as_ref()) == local)
        }
        _ => Ok(false),
    }
}

fn accessibility_join(start: &[u8], children: &[Vec<u8>], end: &[u8]) -> Vec<u8> {
    let mut result = start.to_vec();
    for child in children {
        result.extend_from_slice(child);
    }
    result.extend_from_slice(end);
    result
}

/// Gets the qualified decorative flag without interpreting foreign lookalikes.
pub(crate) fn non_visual_drawing_decorative(xml: &[u8], inherited: &NamespaceBindings) -> bool {
    decorative_value(xml, inherited).unwrap_or(false)
}

fn decorative_value(xml: &[u8], inherited: &NamespaceBindings) -> Result<bool, OxmlError> {
    let bindings = accessibility_bindings(xml, inherited)?;
    let AccessibilityElementParts { children, .. } = accessibility_element_parts(xml)?;
    for list in children {
        if !accessibility_is(&list, &bindings, A_NS, b"extLst")? {
            continue;
        }
        let list_bindings = accessibility_bindings(&list, &bindings)?;
        for extension in accessibility_element_parts(&list)?.children {
            if !accessibility_is(&extension, &list_bindings, A_NS, b"ext")?
                || non_visual_drawing_text(&extension, "uri").as_deref() != Some(DECORATIVE_URI)
            {
                continue;
            }
            let extension_bindings = accessibility_bindings(&extension, &list_bindings)?;
            for flag in accessibility_element_parts(&extension)?.children {
                if accessibility_is(&flag, &extension_bindings, DECORATIVE_NS, b"decorative")?
                    && matches!(
                        non_visual_drawing_text(&flag, "val").as_deref(),
                        Some("1" | "true")
                    )
                {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// Updates the qualified flag in place, preserving unrelated extensions and children.
pub(crate) fn set_non_visual_drawing_decorative(
    xml: &mut Vec<u8>,
    decorative: bool,
    inherited: &NamespaceBindings,
) -> Result<(), OxmlError> {
    let bindings = accessibility_bindings(xml, inherited)?;
    let AccessibilityElementParts {
        start,
        mut children,
        end,
    } = accessibility_element_parts(xml)?;
    let mut lists = Vec::new();
    for (index, child) in children.iter().enumerate() {
        if accessibility_is(child, &bindings, A_NS, b"extLst")? {
            lists.push(index);
        }
    }
    if lists.len() > 1 {
        return Err(OxmlError::InvalidValue(
            "duplicate DrawingML accessibility extension list".to_owned(),
        ));
    }
    let flag = format!(r#"<adec:decorative xmlns:adec="{DECORATIVE_NS}" val="1"/>"#).into_bytes();
    let extension = format!(
        r#"<a:ext xmlns:a="{A_NS}" uri="{DECORATIVE_URI}">{}</a:ext>"#,
        std::str::from_utf8(&flag)?
    )
    .into_bytes();
    if let Some(index) = lists.first().copied() {
        let list_bindings = accessibility_bindings(&children[index], &bindings)?;
        let AccessibilityElementParts {
            start: list_start,
            children: mut extensions,
            end: list_end,
        } = accessibility_element_parts(&children[index])?;
        let mut owned = Vec::new();
        for (slot, ext) in extensions.iter().enumerate() {
            if accessibility_is(ext, &list_bindings, A_NS, b"ext")?
                && non_visual_drawing_text(ext, "uri").as_deref() == Some(DECORATIVE_URI)
            {
                owned.push(slot);
            }
        }
        if owned.len() > 1 {
            return Err(OxmlError::InvalidValue(
                "duplicate decorative extension URI".to_owned(),
            ));
        }
        if let Some(slot) = owned.first().copied() {
            let ext_bindings = accessibility_bindings(&extensions[slot], &list_bindings)?;
            let AccessibilityElementParts {
                start: ext_start,
                children: mut flags,
                end: ext_end,
            } = accessibility_element_parts(&extensions[slot])?;
            let mut owned_flags = Vec::new();
            for (position, child) in flags.iter().enumerate() {
                if accessibility_is(child, &ext_bindings, DECORATIVE_NS, b"decorative")? {
                    owned_flags.push(position);
                }
            }
            if owned_flags.len() > 1 {
                return Err(OxmlError::InvalidValue(
                    "duplicate qualified decorative flag".to_owned(),
                ));
            }
            if let Some(position) = owned_flags.first().copied() {
                // The owned Boolean can be cleared without deleting opaque flag
                // payload or metadata on its extension and list containers.
                let value = if decorative { "1" } else { "0" };
                set_non_visual_drawing_text(&mut flags[position], "val", Some(value))?;
            } else if decorative {
                flags.push(flag);
            } else {
                return Ok(());
            }
            extensions[slot] = accessibility_join(&ext_start, &flags, &ext_end);
        } else if decorative {
            extensions.push(extension);
        } else {
            return Ok(());
        }
        children[index] = accessibility_join(&list_start, &extensions, &list_end);
    } else if decorative {
        children.push(
            format!(
                r#"<a:extLst xmlns:a="{A_NS}">{}</a:extLst>"#,
                std::str::from_utf8(&extension)?
            )
            .into_bytes(),
        );
    } else {
        return Ok(());
    }
    *xml = accessibility_join(&start, &children, &end);
    Ok(())
}

pub(crate) fn root_attributes(
    start: &BytesStart<'_>,
    fixed_prefixes: &[&str],
) -> Result<Vec<(String, String)>, OxmlError> {
    Ok(all_attributes(start)?
        .into_iter()
        .filter(|(name, uri)| {
            !is_fixed_xmlns(name, fixed_prefixes) || (name == "xmlns:r" && uri != R_NS)
        })
        .collect())
}

pub(crate) fn self_contained_attributes(
    start: &BytesStart<'_>,
    fixed_prefixes: &[&str],
    inherited: &[(String, String)],
) -> Result<Vec<(String, String)>, OxmlError> {
    let mut attributes = root_attributes(start, fixed_prefixes)?;
    for (prefix, uri) in inherited {
        let name = if prefix.is_empty() {
            "xmlns".to_owned()
        } else {
            format!("xmlns:{prefix}")
        };
        if (is_fixed_xmlns(&name, fixed_prefixes) && !(prefix == "r" && uri != R_NS))
            || attributes.iter().any(|(existing, _)| existing == &name)
        {
            continue;
        }
        attributes.push((name, uri.clone()));
    }
    Ok(attributes)
}

pub(crate) fn is_fixed_xmlns(name: &str, fixed_prefixes: &[&str]) -> bool {
    name.strip_prefix("xmlns:")
        .is_some_and(|prefix| fixed_prefixes.contains(&prefix))
}

fn canonical_uri(prefix: &str) -> Option<&'static str> {
    match prefix {
        "p" => Some(P_NS),
        "a" => Some(A_NS),
        "r" => Some(R_NS),
        "mc" => Some(MC_NS),
        _ => None,
    }
}

fn qname_prefix(name: &[u8]) -> Option<&str> {
    let position = name.iter().position(|byte| *byte == b':')?;
    std::str::from_utf8(&name[..position]).ok()
}

fn sort_namespace_entries(entries: &mut [(String, String)]) {
    entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
}
