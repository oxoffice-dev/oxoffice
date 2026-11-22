use std::borrow::Cow;
use std::io::Write;

use oxoffice_core::OxmlError;
use oxoffice_core::raw_xml::{capture_element, capture_empty_element, emit_raw, push_attributes};
use oxoffice_core::xml::{
    escape_attribute_value, escape_text, local_name, matches_local_name, push_escaped_attribute,
};
use oxoffice_core::xml_text::resolve_entity;
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::{NsReader, Reader, Writer, XmlVersion};

use crate::color::{ColorChoice, ColorTransform};
use crate::fill::Fill;
use crate::namespace::A_NS;
use crate::order::OrderedRawChildren;

use super::body::{Result, TextError, missing_end};
use super::bullet::TextBullet;

const TEXT_REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

pub(crate) fn text_validate_root(start: &BytesStart<'_>) -> Result<()> {
    let name = start.name();
    let raw = std::str::from_utf8(name.as_ref()).map_err(OxmlError::from)?;
    let prefix = raw.split_once(':').map_or("", |(prefix, _)| prefix);
    let scope = text_namespace_scope(start, &[])?;
    if scope.iter().any(|(key, uri)| key == prefix && uri != A_NS) {
        return Err(invalid_attribute(
            "DrawingML text",
            "namespace",
            "foreign root",
        ));
    }
    Ok(())
}

pub(crate) fn text_source_tag(tag: &str, original: &str, physical: &[(String, String)]) -> String {
    let requested_prefix = tag.split_once(':').map_or("", |(prefix, _)| prefix);
    let expected = match requested_prefix {
        "a" => Some(A_NS),
        "p" if matches!(
            tag.split_once(':').map_or(tag, |(_, local)| local),
            "defaultTextStyle"
                | "titleStyle"
                | "bodyStyle"
                | "otherStyle"
                | "notesStyle"
                | "txBody"
        ) =>
        {
            Some("http://schemas.openxmlformats.org/presentationml/2006/main")
        }
        "dgm" if tag == "dgm:t" => Some("http://schemas.openxmlformats.org/drawingml/2006/diagram"),
        _ => None,
    };
    let prefix = original.split_once(':').map_or("", |(prefix, _)| prefix);
    let owned = expected.is_some_and(|expected| {
        physical
            .iter()
            .any(|(name, uri)| name == prefix && uri == expected)
    });
    if original.is_empty() || !owned {
        return tag.to_owned();
    }
    // An imported alias/default owner is safer than promoting a canonical
    // spelling absent from, or foreign-bound in, its actual source scope.
    let local = tag.split_once(':').map_or(tag, |(_, local)| local);
    original.split_once(':').map_or_else(
        || local.to_owned(),
        |(prefix, _)| format!("{prefix}:{local}"),
    )
}

// A converted wrapper needs its destination namespace without rebinding any
// imported prefix used by opaque descendants. Empty authored shells rely on
// their surrounding owner's declarations as before.
pub(crate) fn text_wrapper_start(
    tag: &str,
    original: &str,
    spellings: &[String],
) -> Result<BytesStart<'static>> {
    let physical = text_namespace_scope(&text_source_start(tag, spellings), &[])?;
    let owned = text_source_tag(tag, original, &physical);
    let mut start = text_source_start(&owned, spellings);
    if original.is_empty() || owned != tag {
        return Ok(start);
    }
    let Some((prefix, local)) = tag.split_once(':') else {
        return Ok(start);
    };
    let expected = match prefix {
        "a" => A_NS,
        "p" => "http://schemas.openxmlformats.org/presentationml/2006/main",
        "dgm" => "http://schemas.openxmlformats.org/drawingml/2006/diagram",
        _ => return Ok(start),
    };
    let original_prefix = original.split_once(':').map_or("", |(prefix, _)| prefix);
    if !physical.iter().any(|(name, _)| name == original_prefix) {
        // Legacy standalone fragments have no physical owner declaration.
        // Preserve their existing surrounding-scope serialization contract.
        return Ok(start);
    }
    if physical
        .iter()
        .any(|(name, uri)| name == prefix && uri == expected)
    {
        return Ok(start);
    }
    let mut destination = prefix.to_owned();
    let mut suffix = 1;
    while physical.iter().any(|(name, _)| name == &destination) {
        destination = format!("{prefix}{suffix}");
        suffix += 1;
    }
    start = text_source_start(&format!("{destination}:{local}"), spellings);
    start.push_attribute((format!("xmlns:{destination}").as_str(), expected));
    Ok(start)
}

fn text_has_physical_owner(tag: &str, physical: &[(String, String)]) -> bool {
    let prefix = tag.split_once(':').map_or("", |(prefix, _)| prefix);
    physical
        .iter()
        .any(|(name, uri)| name == prefix && uri == A_NS)
}

// Newly generated canonical descendants must declare their own URI when an
// imported parent uses that spelling for a foreign namespace.
pub(crate) fn text_close_generated_child(
    raw: &[u8],
    inherited: &[(String, String)],
) -> Result<Vec<u8>> {
    if !inherited
        .iter()
        .any(|(prefix, uri)| prefix == "a" && uri != A_NS)
    {
        return Ok(raw.to_vec());
    }
    let mut reader = Reader::from_reader(raw);
    let event = reader.read_event().map_err(OxmlError::from)?;
    let (start, empty) = match event {
        Event::Start(start) => (start, false),
        Event::Empty(start) => (start, true),
        _ => return Ok(raw.to_vec()),
    };
    if !start.name().as_ref().starts_with(b"a:")
        || start
            .attributes()
            .any(|attribute| attribute.is_ok_and(|attribute| attribute.key.as_ref() == b"xmlns:a"))
    {
        return Ok(raw.to_vec());
    }
    let name = std::str::from_utf8(start.name().as_ref())
        .map_err(OxmlError::from)?
        .to_owned();
    let content = format!(
        "{} xmlns:a=\"{A_NS}\"",
        std::str::from_utf8(start.as_ref()).map_err(OxmlError::from)?
    );
    let replacement = BytesStart::from_content(content, name.len());
    let mut writer = Writer::new(Vec::new());
    if empty {
        write_empty(&mut writer, replacement)?;
    } else {
        write_start(&mut writer, replacement)?;
        writer
            .get_mut()
            .write_all(&raw[reader.buffer_position() as usize..])
            .map_err(OxmlError::from)?;
    }
    Ok(writer.into_inner())
}

pub(crate) fn text_namespace_scope(
    start: &BytesStart<'_>,
    inherited: &[(String, String)],
) -> Result<Vec<(String, String)>> {
    let mut scope = inherited.to_vec();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(OxmlError::from)?;
        let key = attribute.key.as_ref();
        let prefix = if key == b"xmlns" {
            Some("")
        } else if let Some(prefix) = key.strip_prefix(b"xmlns:") {
            Some(std::str::from_utf8(prefix).map_err(OxmlError::from)?)
        } else {
            None
        };
        if let Some(prefix) = prefix {
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
                .map_err(OxmlError::from)?
                .into_owned();
            if let Some((_, existing)) = scope.iter_mut().find(|(name, _)| name == prefix) {
                *existing = value;
            } else {
                scope.push((prefix.to_owned(), value));
            }
        }
    }
    Ok(scope)
}

/// Physical bindings are preserved source facts. Lookup fallbacks exist only
/// for the established standalone, undeclared-prefix fragment API.
#[derive(Clone, Debug, Default)]
pub(crate) struct TextNamespaceScope {
    pub(crate) physical: Vec<(String, String)>,
    lookup: Vec<(String, String)>,
    pub(crate) fixed_list_style_children: bool,
}

impl TextNamespaceScope {
    pub(crate) fn inherited(bindings: &[(String, String)]) -> Self {
        Self {
            physical: bindings.to_vec(),
            lookup: bindings.to_vec(),
            fixed_list_style_children: false,
        }
    }
}

pub(crate) fn text_fragment_scope(
    start: &BytesStart<'_>,
    inherited: &TextNamespaceScope,
) -> Result<TextNamespaceScope> {
    let physical = text_namespace_scope(start, &inherited.physical)?;
    let mut lookup = inherited.lookup.clone();
    if lookup.is_empty() {
        lookup.push(("a".to_owned(), A_NS.to_owned()));
        lookup.push(("r".to_owned(), TEXT_REL_NS.to_owned()));
        let name = start.name();
        let root = std::str::from_utf8(name.as_ref()).map_err(OxmlError::from)?;
        let prefix = root.split_once(':').map_or("", |(prefix, _)| prefix);
        if !lookup.iter().any(|(existing, _)| existing == prefix) {
            lookup.push((prefix.to_owned(), A_NS.to_owned()));
        }
    }
    let lookup = text_namespace_scope(start, &lookup)?;
    Ok(TextNamespaceScope {
        physical,
        lookup,
        fixed_list_style_children: inherited.fixed_list_style_children,
    })
}

pub(crate) fn text_owned_name(
    start: &BytesStart<'_>,
    inherited: &TextNamespaceScope,
) -> Result<Vec<u8>> {
    let scope = text_namespace_scope(start, &inherited.lookup)?;
    let name = start.name();
    let raw = std::str::from_utf8(name.as_ref()).map_err(OxmlError::from)?;
    let (prefix, local) = raw.split_once(':').unwrap_or(("", raw));
    let owned = scope
        .iter()
        .any(|(name, uri)| name == prefix && uri == A_NS);
    // List-style typed descendants use fixed-prefix lower-level writers.
    // Opaque nodes and ordinary paragraph/run alias preservation are unchanged.
    if owned
        && inherited.fixed_list_style_children
        && (is_paragraph_property_tag(local.as_bytes())
            || is_character_property_tag(local.as_bytes())
            || matches!(local, "lnSpc" | "spcBef" | "spcAft"))
    {
        crate::namespace::reject_conflicting_a_prefix(start)?;
    }
    Ok(if owned {
        local.as_bytes().to_vec()
    } else {
        Vec::new()
    })
}

// Attribute token spelling includes preceding whitespace, original quote and
// entities. The normal XML attribute reader validates names/duplicates first.
fn text_attribute_tokens(start: &BytesStart<'_>) -> Result<Vec<(String, String)>> {
    for attribute in start.attributes() {
        attribute.map_err(OxmlError::from)?;
    }
    let raw: &[u8] = start.as_ref();
    let mut index = start.name().as_ref().len();
    let mut output = Vec::new();
    while index < raw.len() {
        let token_start = index;
        while index < raw.len() && raw[index].is_ascii_whitespace() {
            index += 1;
        }
        if index == raw.len() {
            break;
        }
        let name_start = index;
        while index < raw.len() && !raw[index].is_ascii_whitespace() && raw[index] != b'=' {
            index += 1;
        }
        let name = std::str::from_utf8(&raw[name_start..index])
            .map_err(OxmlError::from)?
            .to_owned();
        while index < raw.len() && raw[index].is_ascii_whitespace() {
            index += 1;
        }
        if raw.get(index) != Some(&b'=') {
            return Err(invalid_attribute("text source", "token", "missing equals"));
        }
        index += 1;
        while index < raw.len() && raw[index].is_ascii_whitespace() {
            index += 1;
        }
        let quote = *raw
            .get(index)
            .ok_or_else(|| invalid_attribute("text source", "token", "missing quote"))?;
        if quote != b'\'' && quote != b'"' {
            return Err(invalid_attribute("text source", "token", "invalid quote"));
        }
        index += 1;
        while index < raw.len() && raw[index] != quote {
            index += 1;
        }
        if index == raw.len() {
            return Err(invalid_attribute(
                "text source",
                "token",
                "unterminated quote",
            ));
        }
        index += 1;
        output.push((
            name,
            std::str::from_utf8(&raw[token_start..index])
                .map_err(OxmlError::from)?
                .to_owned(),
        ));
    }
    Ok(output)
}

pub(crate) fn text_root_spellings(start: &BytesStart<'_>) -> Result<Vec<String>> {
    Ok(text_attribute_tokens(start)?
        .into_iter()
        .map(|(_, token)| token)
        .collect())
}

pub(crate) fn text_root_spellings_scoped(
    start: &BytesStart<'_>,
    inherited: &TextNamespaceScope,
    raw: &[u8],
) -> Result<Vec<String>> {
    let mut tokens = text_attribute_tokens(start)?;
    for (prefix, uri) in text_owner_closure(raw, &text_fragment_scope(start, inherited)?.physical)?
    {
        let key = if prefix.is_empty() {
            "xmlns".to_owned()
        } else {
            format!("xmlns:{prefix}")
        };
        if !tokens.iter().any(|(name, _)| name == &key) {
            tokens.push((
                key.clone(),
                format!(" {key}=\"{}\"", quick_xml::escape::escape(&uri)),
            ));
        }
    }
    Ok(tokens.into_iter().map(|(_, token)| token).collect())
}

fn text_field_root_spellings(
    start: &BytesStart<'_>,
    inherited: &TextNamespaceScope,
) -> Result<Vec<String>> {
    let mut tokens = text_attribute_tokens(start)?;
    for (prefix, uri) in text_fragment_scope(start, inherited)?.physical {
        let key = if prefix.is_empty() {
            "xmlns".to_owned()
        } else {
            format!("xmlns:{prefix}")
        };
        if !tokens.iter().any(|(name, _)| name == &key) {
            tokens.push((
                key.clone(),
                format!(" {key}=\"{}\"", quick_xml::escape::escape(&uri)),
            ));
        }
    }
    Ok(tokens
        .into_iter()
        .filter(|(key, _)| key != "id" && key != "type")
        .map(|(_, token)| token)
        .collect())
}

pub(crate) fn text_source_start(tag: &str, spellings: &[String]) -> BytesStart<'static> {
    BytesStart::from_content(format!("{tag}{}", spellings.concat()), tag.len())
}

pub(crate) fn text_capture_opaque_event(event: Event<'_>) -> Result<Vec<u8>> {
    let mut writer = Writer::new(Vec::new());
    writer.write_event(event).map_err(OxmlError::from)?;
    Ok(writer.into_inner())
}

fn text_referenced_prefixes(raw: &[u8]) -> Result<Vec<String>> {
    let mut reader = Reader::from_reader(raw);
    let mut prefixes = Vec::new();
    loop {
        match reader.read_event().map_err(OxmlError::from)? {
            Event::Start(start) | Event::Empty(start) => {
                let name = start.name();
                let name = std::str::from_utf8(name.as_ref()).map_err(OxmlError::from)?;
                let prefix = name.split_once(':').map_or("", |(prefix, _)| prefix);
                if !prefixes.iter().any(|p| p == prefix) {
                    prefixes.push(prefix.to_owned());
                }
                for attribute in start.attributes() {
                    let attribute = attribute.map_err(OxmlError::from)?;
                    let key =
                        std::str::from_utf8(attribute.key.as_ref()).map_err(OxmlError::from)?;
                    if let Some((prefix, _)) = key.split_once(':')
                        && prefix != "xmlns"
                        && !prefixes.iter().any(|p| p == prefix)
                    {
                        prefixes.push(prefix.to_owned());
                    }
                }
            }
            Event::Eof => return Ok(prefixes),
            _ => {}
        }
    }
}

fn text_owner_closure(raw: &[u8], physical: &[(String, String)]) -> Result<Vec<(String, String)>> {
    let mut reader = Reader::from_reader(raw);
    let start = match reader.read_event().map_err(OxmlError::from)? {
        Event::Start(start) | Event::Empty(start) => start,
        _ => return Ok(Vec::new()),
    };
    let mut referenced = text_referenced_prefixes(raw)?;
    // A canonical fragment's conventional prefix remains supplied by its
    // enclosing owner. Actual local declarations stay in the original tokens.
    // Aliases and foreign payload prefixes still need source-bound closure.
    if start.name().as_ref().starts_with(b"a:")
        && !start
            .attributes()
            .any(|attribute| attribute.is_ok_and(|attribute| attribute.key.as_ref() == b"xmlns:a"))
    {
        referenced.retain(|prefix| prefix != "a");
    }
    Ok(physical
        .iter()
        .filter(|(prefix, _)| referenced.contains(prefix))
        .cloned()
        .collect())
}

pub(crate) fn text_close_source_owner(
    raw: &[u8],
    inherited: &[(String, String)],
    tag: &str,
) -> Result<Vec<u8>> {
    let mut reader = Reader::from_reader(raw);
    let event = reader.read_event().map_err(OxmlError::from)?;
    let (start, empty) = match event {
        Event::Start(start) => (start, false),
        Event::Empty(start) => (start, true),
        _ => return Err(invalid_attribute("text source", "root", "missing owner")),
    };
    text_validate_root(&start)?;
    let scope = text_namespace_scope(&start, inherited)?;
    let closure = text_owner_closure(raw, &scope)?;
    let mut referenced = closure
        .iter()
        .map(|(prefix, _)| prefix.clone())
        .collect::<Vec<_>>();
    if !start.name().as_ref().starts_with(b"a:") {
        referenced.push(
            tag.split_once(':')
                .map_or("", |(prefix, _)| prefix)
                .to_owned(),
        );
    }
    let tokens = text_attribute_tokens(&start)?;
    let mut body = format!(
        "{tag}{}",
        tokens
            .iter()
            .map(|(_, token)| token.as_str())
            .collect::<String>()
    );
    for (prefix, uri) in &scope {
        if !referenced.contains(prefix) {
            continue;
        }
        let key = if prefix.is_empty() {
            "xmlns".to_owned()
        } else {
            format!("xmlns:{prefix}")
        };
        if !tokens.iter().any(|(name, _)| name == &key) {
            body.push_str(&format!(
                " {key}=\"{}\"",
                quick_xml::escape::escape(uri.as_str())
            ));
        }
    }
    let original_name = std::str::from_utf8(start.name().as_ref())
        .map_err(OxmlError::from)?
        .to_owned();
    let owned_tag = text_source_tag(tag, &original_name, &scope);
    let tag = owned_tag.as_str();
    let suffix = body.strip_prefix(tag).map(str::to_owned);
    let original_requested = body.find(char::is_whitespace).unwrap_or(body.len());
    let body = format!(
        "{tag}{}",
        suffix.unwrap_or_else(|| body[original_requested..].to_owned())
    );
    let replacement = BytesStart::from_content(body, tag.len());
    let mut writer = Writer::new(Vec::new());
    if empty {
        write_empty(&mut writer, replacement)?;
    } else {
        write_start(&mut writer, replacement)?;
        let end = raw
            .iter()
            .rposition(|byte| *byte == b'<')
            .ok_or_else(|| missing_end("text source"))?;
        let begin = reader.buffer_position() as usize;
        if end < begin {
            return Err(missing_end("text source"));
        }
        writer
            .get_mut()
            .write_all(&raw[begin..end])
            .map_err(OxmlError::from)?;
        write_end(&mut writer, tag)?;
    }
    Ok(writer.into_inner())
}

const MAX_TEXT_MARGIN: i32 = 51_206_400;
const MAX_TEXT_POINT: i32 = 400_000;
const MAX_TEXT_SPACING_POINT: i32 = 158_400;
const MAX_TRANSITIONAL_SPACING_PERCENT: i32 = 13_200_000;

/// Whether source text explicitly requested XML whitespace preservation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TextSpace {
    #[default]
    Default,
    Preserve,
}

/// One DrawingML `a:t` value and its source whitespace intent.
///
/// `value` is the text as the part stores it and PowerPoint shows it. The
/// text setters store a character XML 1.0 cannot carry, such as U+0001, as
/// `_xHHHH_` with four uppercase hexadecimal digits, as python-pptx does, and
/// reading never decodes that form, as neither python-pptx nor PowerPoint
/// does.
#[derive(Clone, Debug, Default)]
pub struct TextValue {
    pub value: String,
    pub space: TextSpace,
    raw_attributes: Vec<(String, String)>,
    source_xml: Option<Vec<u8>>,
    source_value: String,
    source_space: TextSpace,
    source_namespaces: Vec<(String, String)>,
    source_attributes: Vec<(String, String)>,
    source_tag: String,
}
impl PartialEq for TextValue {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
            && self.space == other.space
            && self.raw_attributes == other.raw_attributes
    }
}
impl Eq for TextValue {}

impl TextValue {
    fn from_start(start: &BytesStart<'_>) -> Result<Self> {
        if !matches_local_name(start.name().as_ref(), b"t") {
            return Err(unexpected(start));
        }
        let space_value = text_attr(start, b"xml:space")?;
        let space = match space_value.as_deref() {
            None | Some("default") => TextSpace::Default,
            Some("preserve") => TextSpace::Preserve,
            Some(value) => return Err(invalid_attribute("t", "xml:space", value)),
        };
        Ok(Self {
            value: String::new(),
            space,
            raw_attributes: capture_raw_attributes(start, &[b"xml:space"])?,
            source_xml: Some(capture_empty_element(start)?),
            source_value: String::new(),
            source_space: space,
            source_namespaces: text_fragment_scope(start, &TextNamespaceScope::default())?.physical,
            source_attributes: text_attribute_tokens(start)?,
            source_tag: std::str::from_utf8(start.name().as_ref())
                .map_err(OxmlError::from)?
                .to_owned(),
        })
    }

    fn from_start_with_namespaces(
        start: &BytesStart<'_>,
        inherited: &TextNamespaceScope,
    ) -> Result<Self> {
        let mut value = Self::from_start(start)?;
        value.source_namespaces = text_fragment_scope(start, inherited)?.physical;
        Ok(value)
    }

    fn from_element_with_namespaces(
        reader: &mut Reader<&[u8]>,
        start: &BytesStart<'_>,
        inherited: &TextNamespaceScope,
    ) -> Result<Self> {
        let raw = capture_element(reader, start)?;
        let mut parsed = parse_complete(&raw, b"t", Self::from_element_model, Self::from_start)?;
        parsed.source_namespaces = text_fragment_scope(start, inherited)?.physical;
        parsed.source_value = parsed.value.clone();
        parsed.source_space = parsed.space;
        // Keep the established explicit whitespace-preserve normalization.
        let had_explicit_space = text_attr(start, b"xml:space")?.is_some();
        if parsed.space != TextSpace::Preserve || had_explicit_space {
            parsed.source_xml = Some(raw);
        } else {
            parsed.source_xml = None;
        }
        Ok(parsed)
    }

    fn from_element_model(reader: &mut Reader<&[u8]>, start: &BytesStart<'_>) -> Result<Self> {
        let mut text = Self::from_start(start)?;
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                // XML end-of-line handling: a raw CR or CR LF reads as a line
                // feed, as in every other reader; `&#13;` stays a CR.
                Event::Text(value) => text.value.push_str(
                    &value
                        .xml10_content()
                        .map_err(|error| OxmlError::InvalidValue(error.to_string()))?,
                ),
                Event::CData(value) => text.value.push_str(
                    &value
                        .xml10_content()
                        .map_err(|error| OxmlError::InvalidValue(error.to_string()))?,
                ),
                Event::GeneralRef(value) => text.value.push_str(&resolve_entity(&value)),
                Event::End(element) if matches_local_name(element.name().as_ref(), b"t") => {
                    if text.space == TextSpace::Default
                        && (text.value.chars().next().is_some_and(char::is_whitespace)
                            || text
                                .value
                                .chars()
                                .next_back()
                                .is_some_and(char::is_whitespace))
                    {
                        text.space = TextSpace::Preserve;
                    }
                    return Ok(text);
                }
                Event::Start(element) | Event::Empty(element) => {
                    return Err(unexpected(&element));
                }
                Event::Eof => return Err(missing_end("t")),
                _ => {}
            }
            buffer.clear();
        }
    }

    fn write_xml<W: Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        if text_has_physical_owner(&self.source_tag, &self.source_namespaces)
            && self.value == self.source_value
            && self.space == self.source_space
            && let Some(raw) = &self.source_xml
        {
            writer
                .get_mut()
                .write_all(&text_close_source_owner(
                    raw,
                    &self.source_namespaces,
                    "a:t",
                )?)
                .map_err(OxmlError::from)?;
            return Ok(());
        }
        let preserve = self.space == TextSpace::Preserve
            || self.value.chars().next().is_some_and(char::is_whitespace)
            || self
                .value
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        let owned_tag = text_source_tag("a:t", &self.source_tag, &self.source_namespaces);
        let tag = owned_tag.as_str();
        let mut start = BytesStart::new(tag);
        if preserve {
            start.push_attribute(("xml:space", "preserve"));
        }
        if self.source_attributes.is_empty() {
            push_attributes(&mut start, &self.raw_attributes);
        } else {
            let unknown: String = self
                .source_attributes
                .iter()
                .filter(|(key, _)| key != "xml:space")
                .map(|(_, token)| token.as_str())
                .collect();
            let generated = std::str::from_utf8(start.as_ref())
                .map_err(OxmlError::from)?
                .to_owned();
            start = BytesStart::from_content(format!("{generated}{unknown}"), tag.len());
        }
        let existing_keys: Vec<String> = start
            .attributes()
            .map(|attribute| {
                attribute.map_err(OxmlError::from).and_then(|attribute| {
                    std::str::from_utf8(attribute.key.as_ref())
                        .map(str::to_owned)
                        .map_err(OxmlError::from)
                })
            })
            .collect::<std::result::Result<_, OxmlError>>()?;
        let source = self
            .source_xml
            .clone()
            .unwrap_or(capture_empty_element(&start)?);
        for (prefix, uri) in text_owner_closure(&source, &self.source_namespaces)? {
            let key = if prefix.is_empty() {
                "xmlns".to_owned()
            } else {
                format!("xmlns:{prefix}")
            };
            if !existing_keys.contains(&key) {
                start.push_attribute((key.as_str(), uri.as_str()));
            }
        }
        if self.value.is_empty() {
            return write_empty(writer, start);
        }
        write_start(writer, start)?;
        writer
            .write_event(Event::Text(BytesText::from_escaped(escape_text(
                &self.value,
            ))))
            .map_err(OxmlError::from)?;
        write_end(writer, tag)
    }
}

/// Spells each character XML 1.0 cannot carry as `_xHHHH_`, as python-pptx's
/// run text setter does, so text holding one can still be saved.
pub fn escape_invalid_xml_characters(text: &str) -> Cow<'_, str> {
    let is_invalid = |character: char| !oxoffice_core::xml::is_xml_1_0_character(character);
    if !text.chars().any(is_invalid) {
        return Cow::Borrowed(text);
    }
    let mut escaped = String::with_capacity(text.len() + 6);
    for character in text.chars() {
        if is_invalid(character) {
            escaped.push_str(&format!("_x{:04X}_", u32::from(character)));
        } else {
            escaped.push(character);
        }
    }
    Cow::Owned(escaped)
}

/// The seven members of `ST_TextAlignType`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextAlignment {
    Left,
    Center,
    Right,
    Justified,
    JustifiedLow,
    Distributed,
    ThaiDistributed,
}

impl TextAlignment {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "l" => Some(Self::Left),
            "ctr" => Some(Self::Center),
            "r" => Some(Self::Right),
            "just" => Some(Self::Justified),
            "justLow" => Some(Self::JustifiedLow),
            "dist" => Some(Self::Distributed),
            "thaiDist" => Some(Self::ThaiDistributed),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Left => "l",
            Self::Center => "ctr",
            Self::Right => "r",
            Self::Justified => "just",
            Self::JustifiedLow => "justLow",
            Self::Distributed => "dist",
            Self::ThaiDistributed => "thaiDist",
        }
    }
}

/// DrawingML paragraph spacing in percentage text or centipoints.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TextSpacing {
    Percent(String),
    Points(i32),
}

impl TextSpacing {
    fn from_xml(xml: &[u8], wrapper: &[u8]) -> Result<Self> {
        parse_complete(
            xml,
            wrapper,
            |reader, start| Self::from_element(reader, start, wrapper),
            |_| {
                Err(TextError::UnexpectedElement(
                    String::from_utf8_lossy(wrapper).into_owned(),
                ))
            },
        )
    }

    fn from_element(
        reader: &mut Reader<&[u8]>,
        start: &BytesStart<'_>,
        wrapper: &[u8],
    ) -> Result<Self> {
        text_validate_root(start)?;
        let mut spacing = None;
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Empty(element) if matches_local_name(element.name().as_ref(), b"spcPct") => {
                    text_validate_root(&element)?;
                    if spacing.is_some() {
                        return Err(duplicate("text spacing choice"));
                    }
                    let value = required_attr(&element, b"val")?;
                    validate_spacing_percent(&value)?;
                    spacing = Some(Self::Percent(value));
                }
                Event::Empty(element) if matches_local_name(element.name().as_ref(), b"spcPts") => {
                    text_validate_root(&element)?;
                    if spacing.is_some() {
                        return Err(duplicate("text spacing choice"));
                    }
                    let value = parse_i32_attr(&element, b"val", 0, MAX_TEXT_SPACING_POINT)?;
                    spacing = Some(Self::Points(value));
                }
                Event::Start(element)
                    if matches_local_name(element.name().as_ref(), b"spcPct")
                        || matches_local_name(element.name().as_ref(), b"spcPts") =>
                {
                    text_validate_root(&element)?;
                    if spacing.is_some() {
                        return Err(duplicate("text spacing choice"));
                    }
                    let name = local_name(element.name().as_ref()).to_vec();
                    let value = if name == b"spcPct" {
                        let value = required_attr(&element, b"val")?;
                        validate_spacing_percent(&value)?;
                        Self::Percent(value)
                    } else {
                        Self::Points(parse_i32_attr(&element, b"val", 0, MAX_TEXT_SPACING_POINT)?)
                    };
                    ensure_empty(reader, &name)?;
                    spacing = Some(value);
                }
                Event::Start(element) | Event::Empty(element) => return Err(unexpected(&element)),
                Event::End(element) if matches_local_name(element.name().as_ref(), wrapper) => {
                    return spacing.ok_or_else(|| {
                        TextError::UnexpectedElement(String::from_utf8_lossy(wrapper).into_owned())
                    });
                }
                Event::Eof => return Err(missing_end(&String::from_utf8_lossy(wrapper))),
                _ => {}
            }
            buffer.clear();
        }
    }

    fn write_xml<W: Write>(&self, writer: &mut Writer<W>, wrapper: &str) -> Result<()> {
        write_start(writer, BytesStart::new(wrapper))?;
        match self {
            Self::Percent(value) => {
                validate_spacing_percent(value)?;
                let mut start = BytesStart::new("a:spcPct");
                start.push_attribute(("val", value.as_str()));
                write_empty(writer, start)?;
            }
            Self::Points(value) if (0..=MAX_TEXT_SPACING_POINT).contains(value) => {
                let value = value.to_string();
                let mut start = BytesStart::new("a:spcPts");
                start.push_attribute(("val", value.as_str()));
                write_empty(writer, start)?;
            }
            Self::Points(value) => {
                return Err(invalid_attribute("spcPts", "val", &value.to_string()));
            }
        }
        write_end(writer, wrapper)
    }
}

/// Character spacing in centipoints or a strict universal measure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TextPointValue {
    Centipoints(i32),
    UniversalMeasure(String),
}

impl TextPointValue {
    fn parse(value: String) -> Result<Self> {
        if let Ok(point) = value.parse::<i32>() {
            if (-MAX_TEXT_POINT..=MAX_TEXT_POINT).contains(&point) {
                return Ok(Self::Centipoints(point));
            }
            return Err(invalid_attribute("rPr", "spc", &value));
        }
        if is_universal_measure(&value) {
            Ok(Self::UniversalMeasure(value))
        } else {
            Err(invalid_attribute("rPr", "spc", &value))
        }
    }

    fn as_xml(&self) -> Result<String> {
        match self {
            Self::Centipoints(value) if (-MAX_TEXT_POINT..=MAX_TEXT_POINT).contains(value) => {
                Ok(value.to_string())
            }
            Self::Centipoints(value) => Err(invalid_attribute("rPr", "spc", &value.to_string())),
            Self::UniversalMeasure(value) if is_universal_measure(value) => Ok(value.clone()),
            Self::UniversalMeasure(value) => Err(invalid_attribute("rPr", "spc", value)),
        }
    }
}

/// DrawingML underline styles.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextUnderline {
    None,
    Words,
    Single,
    Double,
    Heavy,
    Dotted,
    DottedHeavy,
    Dash,
    DashHeavy,
    DashLong,
    DashLongHeavy,
    DotDash,
    DotDashHeavy,
    DotDotDash,
    DotDotDashHeavy,
    Wavy,
    WavyHeavy,
    WavyDouble,
}

impl TextUnderline {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "none" => Self::None,
            "words" => Self::Words,
            "sng" => Self::Single,
            "dbl" => Self::Double,
            "heavy" => Self::Heavy,
            "dotted" => Self::Dotted,
            "dottedHeavy" => Self::DottedHeavy,
            "dash" => Self::Dash,
            "dashHeavy" => Self::DashHeavy,
            "dashLong" => Self::DashLong,
            "dashLongHeavy" => Self::DashLongHeavy,
            "dotDash" => Self::DotDash,
            "dotDashHeavy" => Self::DotDashHeavy,
            "dotDotDash" => Self::DotDotDash,
            "dotDotDashHeavy" => Self::DotDotDashHeavy,
            "wavy" => Self::Wavy,
            "wavyHeavy" => Self::WavyHeavy,
            "wavyDbl" => Self::WavyDouble,
            _ => return None,
        })
    }

    /// Returns the `ST_TextUnderlineType` token as written in the file.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Words => "words",
            Self::Single => "sng",
            Self::Double => "dbl",
            Self::Heavy => "heavy",
            Self::Dotted => "dotted",
            Self::DottedHeavy => "dottedHeavy",
            Self::Dash => "dash",
            Self::DashHeavy => "dashHeavy",
            Self::DashLong => "dashLong",
            Self::DashLongHeavy => "dashLongHeavy",
            Self::DotDash => "dotDash",
            Self::DotDashHeavy => "dotDashHeavy",
            Self::DotDotDash => "dotDotDash",
            Self::DotDotDashHeavy => "dotDotDashHeavy",
            Self::Wavy => "wavy",
            Self::WavyHeavy => "wavyHeavy",
            Self::WavyDouble => "wavyDbl",
        }
    }
}

/// DrawingML strike styles.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextStrike {
    None,
    Single,
    Double,
}

impl TextStrike {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "noStrike" => Some(Self::None),
            "sngStrike" => Some(Self::Single),
            "dblStrike" => Some(Self::Double),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::None => "noStrike",
            Self::Single => "sngStrike",
            Self::Double => "dblStrike",
        }
    }
}

/// A DrawingML text typeface reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextFont {
    pub typeface: String,
    raw_attributes: Vec<(String, String)>,
}

impl TextFont {
    pub fn new(typeface: impl Into<String>) -> Result<Self> {
        let typeface = typeface.into();
        oxoffice_core::xml::reject_non_xml_characters("font typeface", &typeface)?;
        if typeface.is_empty() {
            return Err(invalid_attribute("font", "typeface", ""));
        }
        Ok(Self {
            typeface,
            raw_attributes: Vec::new(),
        })
    }

    pub(crate) fn from_xml(xml: &[u8], expected: &[u8]) -> Result<Self> {
        parse_complete(
            xml,
            expected,
            |reader, start| {
                let font = Self::from_start(start)?;
                ensure_empty(reader, expected)?;
                Ok(font)
            },
            Self::from_start,
        )
    }

    fn from_start(start: &BytesStart<'_>) -> Result<Self> {
        text_validate_root(start)?;
        Ok(Self {
            typeface: required_attr(start, b"typeface")?,
            raw_attributes: capture_raw_attributes(start, &[b"typeface"])?,
        })
    }

    pub(crate) fn write_xml<W: Write>(&self, writer: &mut Writer<W>, tag: &str) -> Result<()> {
        if self.typeface.is_empty() {
            return Err(invalid_attribute(tag, "typeface", ""));
        }
        let mut start = BytesStart::new(tag);
        start.push_attribute(("typeface", self.typeface.as_str()));
        push_attributes(&mut start, &self.raw_attributes);
        write_empty(writer, start)
    }
}

/// Hyperlink attributes retained on text character properties.
#[derive(Clone, Debug, Default)]
pub struct TextHyperlink {
    pub relationship_id: Option<String>,
    pub invalid_url: Option<String>,
    pub action: Option<String>,
    pub target_frame: Option<String>,
    pub tooltip: Option<String>,
    pub history: Option<bool>,
    pub highlight_click: Option<bool>,
    pub end_sound: Option<bool>,
    raw_attributes: Vec<(String, String)>,
    raw_children: OrderedRawChildren,
    source_xml: Option<Vec<u8>>,
    source_namespaces: Vec<(String, String)>,
    source_values: Vec<(String, Option<String>)>,
    source_id_key: Option<String>,
}
impl PartialEq for TextHyperlink {
    fn eq(&self, other: &Self) -> bool {
        self.relationship_id == other.relationship_id
            && self.invalid_url == other.invalid_url
            && self.action == other.action
            && self.target_frame == other.target_frame
            && self.tooltip == other.tooltip
            && self.history == other.history
            && self.highlight_click == other.highlight_click
            && self.end_sound == other.end_sound
            && self
                .raw_attributes
                .iter()
                .filter(|(name, value)| {
                    !((name == "xmlns:a" && value == A_NS)
                        || (name == "xmlns:r" && value == TEXT_REL_NS))
                })
                .eq(other.raw_attributes.iter().filter(|(name, value)| {
                    !((name == "xmlns:a" && value == A_NS)
                        || (name == "xmlns:r" && value == TEXT_REL_NS))
                }))
            && self.raw_children == other.raw_children
    }
}
impl Eq for TextHyperlink {}

impl TextHyperlink {
    fn from_xml_with_namespaces(
        xml: &[u8],
        expected: &[u8],
        inherited: &TextNamespaceScope,
    ) -> Result<Self> {
        let mut result = parse_complete(
            xml,
            expected,
            |reader, start| Self::from_element_with_namespaces(reader, start, expected, inherited),
            |start| Self::from_start_with_namespaces(start, inherited),
        )?;
        result.source_xml = Some(xml.to_vec());
        result.source_values = result.attribute_values();
        Ok(result)
    }

    fn from_start_with_namespaces(
        start: &BytesStart<'_>,
        inherited: &TextNamespaceScope,
    ) -> Result<Self> {
        text_validate_root(start)?;
        let namespaces = text_fragment_scope(start, inherited)?;
        if text_owned_name(start, &namespaces)?.is_empty() {
            return Err(invalid_attribute("hyperlink", "namespace", "foreign owner"));
        }
        let mut relationship_id = None;
        let mut source_id_key = None;
        for attribute in start.attributes() {
            let attribute = attribute.map_err(OxmlError::from)?;
            let key = std::str::from_utf8(attribute.key.as_ref()).map_err(OxmlError::from)?;
            if let Some((prefix, "id")) = key.split_once(':')
                && namespaces
                    .lookup
                    .iter()
                    .any(|(name, uri)| name == prefix && uri == TEXT_REL_NS)
            {
                if source_id_key.is_some() {
                    return Err(duplicate("hyperlink relationship id"));
                }
                relationship_id = Some(
                    attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
                        .map_err(OxmlError::from)?
                        .into_owned(),
                );
                source_id_key = Some(key.to_owned());
            }
        }
        let mut raw_attributes = capture_raw_attributes(
            start,
            &[
                b"invalidUrl",
                b"action",
                b"tgtFrame",
                b"tooltip",
                b"history",
                b"highlightClick",
                b"endSnd",
            ],
        )?;
        raw_attributes.retain(|(key, _)| Some(key) != source_id_key.as_ref());
        Ok(Self {
            relationship_id,
            invalid_url: text_attr(start, b"invalidUrl")?,
            action: text_attr(start, b"action")?,
            target_frame: text_attr(start, b"tgtFrame")?,
            tooltip: text_attr(start, b"tooltip")?,
            history: parse_optional_bool(start, b"history")?,
            highlight_click: parse_optional_bool(start, b"highlightClick")?,
            end_sound: parse_optional_bool(start, b"endSnd")?,
            raw_attributes,
            raw_children: OrderedRawChildren::default(),
            source_xml: None,
            source_namespaces: namespaces.physical,
            source_values: Vec::new(),
            source_id_key,
        })
    }

    fn from_element_with_namespaces(
        reader: &mut Reader<&[u8]>,
        start: &BytesStart<'_>,
        expected: &[u8],
        inherited: &TextNamespaceScope,
    ) -> Result<Self> {
        let mut hyperlink = Self::from_start_with_namespaces(start, inherited)?;
        let namespaces = text_fragment_scope(start, inherited)?;
        let mut boundary = 0;
        let mut seen = [false; 2];
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element) => {
                    let name = text_owned_name(&element, &namespaces)?;
                    let raw = capture_element(reader, &element)?;
                    capture_hyperlink_raw(&mut hyperlink, &name, raw, &mut boundary, &mut seen)?;
                }
                Event::Empty(element) => {
                    let name = text_owned_name(&element, &namespaces)?;
                    let raw = capture_empty_element(&element)?;
                    capture_hyperlink_raw(&mut hyperlink, &name, raw, &mut boundary, &mut seen)?;
                }
                Event::End(element) if matches_local_name(element.name().as_ref(), expected) => {
                    return Ok(hyperlink);
                }
                Event::Eof => return Err(missing_end(&String::from_utf8_lossy(expected))),
                _ => {}
            }
            buffer.clear();
        }
    }

    fn write_xml<W: Write>(&self, writer: &mut Writer<W>, tag: &str) -> Result<()> {
        if self.source_namespaces.iter().any(|(_, uri)| uri == A_NS)
            && let Some(source) = &self.source_xml
        {
            return self.write_source_xml(writer, tag, source);
        }
        let mut start = BytesStart::new(tag);
        // A newly authored click must own its relationship prefix locally.
        // Preserve opaque raw r:* attributes by selecting a different prefix.
        if let Some(id) = self.relationship_id.as_deref() {
            let occupied = |prefix: &str| {
                self.raw_attributes.iter().any(|(key, value)| {
                    key.starts_with(&format!("{prefix}:"))
                        || (key == &format!("xmlns:{prefix}") && value != TEXT_REL_NS)
                })
            };
            let prefix = (0..=self.raw_attributes.len() + 1)
                .map(|index| {
                    if index == 0 {
                        "r".to_owned()
                    } else {
                        format!("rLink{index}")
                    }
                })
                .find(|prefix| !occupied(prefix))
                .ok_or_else(|| {
                    invalid_attribute("hyperlink", "namespace", "no relationship prefix")
                })?;
            let declaration = format!("xmlns:{prefix}");
            if self.source_xml.is_none()
                && !self
                    .raw_attributes
                    .iter()
                    .any(|(key, value)| key == &declaration && value == TEXT_REL_NS)
            {
                start.push_attribute((declaration.as_str(), TEXT_REL_NS));
            }
            let key = format!("{prefix}:id");
            start.push_attribute((key.as_str(), id));
        }
        push_optional_attr(&mut start, "invalidUrl", self.invalid_url.as_deref());
        push_optional_attr(&mut start, "action", self.action.as_deref());
        push_optional_attr(&mut start, "tgtFrame", self.target_frame.as_deref());
        if let Some(tooltip) = self.tooltip.as_deref() {
            push_escaped_attribute(&mut start, "tooltip", tooltip);
        }
        push_optional_bool(&mut start, "history", self.history);
        push_optional_bool(&mut start, "highlightClick", self.highlight_click);
        push_optional_bool(&mut start, "endSnd", self.end_sound);
        push_attributes(&mut start, &self.raw_attributes);
        if self.raw_children.is_empty() {
            return write_empty(writer, start);
        }
        write_start(writer, start)?;
        for boundary in 0..=2 {
            emit_raw(writer, self.raw_children.at(boundary))?;
        }
        write_end(writer, tag)
    }

    fn attribute_values(&self) -> Vec<(String, Option<String>)> {
        vec![
            ("id".to_owned(), self.relationship_id.clone()),
            ("invalidUrl".to_owned(), self.invalid_url.clone()),
            ("action".to_owned(), self.action.clone()),
            ("tgtFrame".to_owned(), self.target_frame.clone()),
            ("tooltip".to_owned(), self.tooltip.clone()),
            (
                "history".to_owned(),
                self.history.map(|v| if v { "1" } else { "0" }.to_owned()),
            ),
            (
                "highlightClick".to_owned(),
                self.highlight_click
                    .map(|v| if v { "1" } else { "0" }.to_owned()),
            ),
            (
                "endSnd".to_owned(),
                self.end_sound.map(|v| if v { "1" } else { "0" }.to_owned()),
            ),
        ]
    }

    fn write_source_xml<W: Write>(
        &self,
        writer: &mut Writer<W>,
        tag: &str,
        source: &[u8],
    ) -> Result<()> {
        let mut reader = Reader::from_reader(source);
        let mut buffer = Vec::new();
        let (start, empty) = match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(start) => (start, false),
            Event::Empty(start) => (start, true),
            _ => return Err(invalid_attribute("hyperlink", "source", "missing owner")),
        };
        let head_end = reader.buffer_position() as usize;
        let tokens = text_attribute_tokens(&start)?;
        let current = self.attribute_values();
        let changed: Vec<_> = current
            .iter()
            .filter(|(name, value)| {
                self.source_values
                    .iter()
                    .find(|(old, _)| old == name)
                    .map(|(_, original)| original)
                    != Some(value)
            })
            .collect();
        let original_name = std::str::from_utf8(start.name().as_ref())
            .map_err(OxmlError::from)?
            .to_owned();
        let owned_tag = text_source_tag(tag, &original_name, &self.source_namespaces);
        let tag = owned_tag.as_str();
        let mut body = tag.to_owned();
        let mut emitted = Vec::new();
        let mut id_key = self.source_id_key.clone();
        let mut namespace_additions = Vec::new();
        let mut referenced = text_referenced_prefixes(source)?;
        referenced.push(
            tag.split_once(':')
                .map_or("", |(prefix, _)| prefix)
                .to_owned(),
        );
        if self.relationship_id.is_some() && id_key.is_none() {
            let prefix = self
                .source_namespaces
                .iter()
                .find(|(prefix, uri)| !prefix.is_empty() && uri == TEXT_REL_NS)
                .map(|(prefix, _)| prefix.clone())
                .or_else(|| {
                    (0..=self.source_namespaces.len())
                        .map(|index| {
                            if index == 0 {
                                "r".to_owned()
                            } else {
                                format!("rLink{index}")
                            }
                        })
                        .find(|prefix| {
                            !self
                                .source_namespaces
                                .iter()
                                .any(|(name, uri)| name == prefix && uri != TEXT_REL_NS)
                        })
                })
                .ok_or_else(|| {
                    invalid_attribute("hyperlink", "namespace", "no relationship prefix")
                })?;
            if !self
                .source_namespaces
                .iter()
                .any(|(name, uri)| name == &prefix && uri == TEXT_REL_NS)
            {
                namespace_additions.push((prefix.clone(), TEXT_REL_NS.to_owned()));
            }
            referenced.push(prefix.clone());
            id_key = Some(format!("{prefix}:id"));
        }
        for (key, token) in &tokens {
            // Only the URI-qualified source token represents relationship_id.
            // An unqualified lexical id is opaque, even when no owned id exists.
            let owned = if self.source_id_key.as_ref() == Some(key) {
                Some("id")
            } else if key == "id" {
                None
            } else {
                Some(key.as_str())
            };
            if let Some((name, value)) = changed
                .iter()
                .find(|(name, _)| Some(name.as_str()) == owned)
            {
                emitted.push(name.as_str());
                if let Some(value) = value {
                    let key = if owned == Some("id") {
                        id_key.as_deref().unwrap_or("r:id")
                    } else {
                        key.as_str()
                    };
                    body.push_str(&format!(" {key}=\"{}\"", escape_attribute_value(value)));
                }
            } else {
                body.push_str(token);
            }
        }
        for (name, value) in &changed {
            if emitted.contains(&name.as_str()) {
                continue;
            }
            if let Some(value) = value {
                let key = if name == "id" {
                    id_key.as_deref().unwrap_or("r:id")
                } else {
                    name.as_str()
                };
                body.push_str(&format!(" {key}=\"{}\"", escape_attribute_value(value)));
            }
        }
        for (prefix, uri) in self
            .source_namespaces
            .iter()
            .chain(namespace_additions.iter())
        {
            if !referenced.contains(prefix) {
                continue;
            }
            let key = if prefix.is_empty() {
                "xmlns".to_owned()
            } else {
                format!("xmlns:{prefix}")
            };
            if tokens.iter().any(|(name, _)| name == &key) {
                continue;
            }
            body.push_str(&format!(" {key}=\"{}\"", quick_xml::escape::escape(uri)));
        }
        let replacement = BytesStart::from_content(body, tag.len());
        if empty {
            return write_empty(writer, replacement);
        }
        write_start(writer, replacement)?;
        let end_start = source
            .iter()
            .rposition(|byte| *byte == b'<')
            .ok_or_else(|| missing_end("hyperlink"))?;
        if end_start < head_end {
            return Err(missing_end("hyperlink"));
        }
        writer
            .get_mut()
            .write_all(&source[head_end..end_start])
            .map_err(OxmlError::from)?;
        write_end(writer, tag)
    }
}

fn capture_hyperlink_raw(
    hyperlink: &mut TextHyperlink,
    name: &[u8],
    raw: Vec<u8>,
    boundary: &mut usize,
    seen: &mut [bool; 2],
) -> Result<()> {
    let slot = match name {
        b"snd" => Some(0),
        b"extLst" => Some(1),
        _ => None,
    };
    if let Some(slot) = slot {
        if seen[slot] {
            return Err(duplicate(&String::from_utf8_lossy(name)));
        }
        seen[slot] = true;
        hyperlink.raw_children.push((*boundary).max(slot), raw);
        *boundary = (*boundary).max(slot + 1);
    } else {
        hyperlink.raw_children.push(*boundary, raw);
    }
    Ok(())
}

/// DrawingML character properties shared by runs, breaks, fields, and defaults.
#[allow(non_camel_case_types)]
#[derive(Clone, Debug, Default)]
pub struct CT_TextCharacterProperties {
    pub font_size: Option<i32>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub all_caps: Option<bool>,
    pub small_caps: Option<bool>,
    pub highlight: Option<ColorChoice>,
    highlight_source: Option<(Vec<u8>, std::ops::Range<usize>, ColorChoice)>,
    pub underline: Option<TextUnderline>,
    pub strike: Option<TextStrike>,
    pub spacing: Option<TextPointValue>,
    pub baseline: Option<String>,
    pub fill: Option<Fill>,
    pub latin: Option<TextFont>,
    pub east_asian: Option<TextFont>,
    pub complex_script: Option<TextFont>,
    pub symbol: Option<TextFont>,
    pub hyperlink_click: Option<TextHyperlink>,
    pub hyperlink_mouse_over: Option<TextHyperlink>,
    raw_attributes: Vec<(String, String)>,
    source_attributes: Vec<(String, String)>,
    source_tag: String,
    source_namespaces: Vec<(String, String)>,
    source_closure: Vec<(String, String)>,
    source_fill: Option<(Vec<u8>, Fill)>,
    source_latin: Option<(Vec<u8>, TextFont)>,
    source_east_asian: Option<(Vec<u8>, TextFont)>,
    source_complex_script: Option<(Vec<u8>, TextFont)>,
    source_symbol: Option<(Vec<u8>, TextFont)>,
    source_cap_modeled: bool,
    raw_children: OrderedRawChildren,
}
impl PartialEq for CT_TextCharacterProperties {
    fn eq(&self, other: &Self) -> bool {
        self.font_size == other.font_size
            && self.bold == other.bold
            && self.italic == other.italic
            && self.all_caps == other.all_caps
            && self.small_caps == other.small_caps
            && self.highlight == other.highlight
            && self.highlight_source == other.highlight_source
            && self.underline == other.underline
            && self.strike == other.strike
            && self.spacing == other.spacing
            && self.baseline == other.baseline
            && self.fill == other.fill
            && self.latin == other.latin
            && self.east_asian == other.east_asian
            && self.complex_script == other.complex_script
            && self.symbol == other.symbol
            && self.hyperlink_click == other.hyperlink_click
            && self.hyperlink_mouse_over == other.hyperlink_mouse_over
            && self
                .raw_attributes
                .iter()
                .filter(|(name, value)| {
                    !((name == "xmlns:a" && value == A_NS)
                        || (name == "xmlns:r" && value == TEXT_REL_NS))
                })
                .eq(other.raw_attributes.iter().filter(|(name, value)| {
                    !((name == "xmlns:a" && value == A_NS)
                        || (name == "xmlns:r" && value == TEXT_REL_NS))
                }))
            && self.raw_children == other.raw_children
    }
}
impl Eq for CT_TextCharacterProperties {}

impl CT_TextCharacterProperties {
    /// Returns local character properties with absent groups inherited from defaults.
    pub fn with_inherited_defaults(&self, defaults: &Self) -> Result<Self> {
        let mut local = Writer::new(Vec::new());
        self.write_xml(&mut local, "a:rPr")?;
        let mut inherited = Writer::new(Vec::new());
        defaults.write_xml(&mut inherited, "a:rPr")?;
        let xml = text_inherit_property_xml(
            &local.into_inner(),
            &self.source_namespaces,
            &inherited.into_inner(),
            &defaults.source_namespaces,
        )?;
        Self::from_xml_with_namespaces(
            &xml,
            &TextNamespaceScope::inherited(&self.source_namespaces),
        )
    }

    fn highlight_scope_attributes(&self) -> Vec<(String, String)> {
        let mut attributes = self.raw_attributes.clone();
        for (prefix, uri) in &self.source_namespaces {
            let key = if prefix.is_empty() {
                "xmlns".to_owned()
            } else {
                format!("xmlns:{prefix}")
            };
            if !attributes.iter().any(|(name, _)| name == &key) {
                attributes.push((key, uri.clone()));
            }
        }
        attributes
    }

    /// Reads the unqualified DrawingML language without promoting or rewriting source.
    pub fn language(&self) -> Option<&str> {
        self.raw_attributes
            .iter()
            .find(|(name, _)| name == "lang")
            .map(|(_, value)| value.as_str())
    }

    /// Replaces or removes the `lang` language tag, keeping its position
    /// among the preserved attributes.
    pub fn set_language(&mut self, language: Option<&str>) {
        if self.language() == language {
            return;
        }
        let position = self
            .raw_attributes
            .iter()
            .position(|(name, _)| name == "lang");
        match (position, language) {
            (Some(index), Some(value)) => self.raw_attributes[index].1 = value.to_owned(),
            (Some(index), None) => {
                self.raw_attributes.remove(index);
            }
            (None, Some(value)) => self
                .raw_attributes
                .push(("lang".to_owned(), value.to_owned())),
            (None, None) => {}
        }
        if !self.source_attributes.is_empty() {
            let position = self
                .source_attributes
                .iter()
                .position(|(name, _)| name == "lang");
            let token =
                language.map(|value| format!(" lang=\"{}\"", quick_xml::escape::escape(value)));
            match (position, token) {
                (Some(index), Some(token)) => self.source_attributes[index].1 = token,
                (Some(index), None) => {
                    self.source_attributes.remove(index);
                }
                (None, Some(token)) => self.source_attributes.push(("lang".to_owned(), token)),
                (None, None) => {}
            }
        }
    }

    /// Parses one complete character-property element with any prefix.
    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        let expected = root_local_name(xml)?;
        if !is_character_property_tag(&expected) {
            return Err(TextError::UnexpectedElement(
                String::from_utf8_lossy(&expected).into_owned(),
            ));
        }
        Self::from_xml_with_namespaces(xml, &TextNamespaceScope::default())
    }

    pub(crate) fn from_xml_with_namespaces(
        xml: &[u8],
        inherited: &TextNamespaceScope,
    ) -> Result<Self> {
        let expected = root_local_name(xml)?;
        parse_complete(
            xml,
            &expected,
            |reader, start| Self::from_element_with_namespaces(reader, start, inherited),
            |start| Self::from_start_with_namespaces(start, inherited),
        )
    }

    fn from_start_with_namespaces(
        start: &BytesStart<'_>,
        inherited: &TextNamespaceScope,
    ) -> Result<Self> {
        let mut parsed = Self::from_start(start)?;
        parsed.source_namespaces = text_fragment_scope(start, inherited)?.physical;
        parsed.source_closure =
            text_owner_closure(&capture_empty_element(start)?, &parsed.source_namespaces)?;
        Ok(parsed)
    }

    fn from_start(start: &BytesStart<'_>) -> Result<Self> {
        text_validate_root(start)?;
        let element = String::from_utf8_lossy(local_name(start.name().as_ref())).into_owned();
        let font_size = text_attr(start, b"sz")?
            .map(|value| parse_i32_value(&element, "sz", &value, 100, MAX_TEXT_POINT))
            .transpose()?;
        let spacing = text_attr(start, b"spc")?
            .map(TextPointValue::parse)
            .transpose()?;
        let baseline = text_attr(start, b"baseline")?;
        if let Some(value) = baseline.as_deref() {
            validate_baseline(value)?;
        }
        let all_caps = match text_attr(start, b"cap")?.as_deref() {
            Some("all") => Some(true),
            Some("none") => Some(false),
            _ => None,
        };
        let small_caps = match text_attr(start, b"cap")?.as_deref() {
            Some("small") => Some(true),
            Some("none") => Some(false),
            _ => None,
        };
        let mut raw_attributes = capture_raw_attributes(
            start,
            &[b"sz", b"b", b"i", b"u", b"strike", b"spc", b"baseline"],
        )?;
        if all_caps.is_some() || small_caps.is_some() {
            raw_attributes.retain(|(name, _)| name != "cap");
        }
        Ok(Self {
            font_size,
            bold: parse_optional_bool(start, b"b")?,
            italic: parse_optional_bool(start, b"i")?,
            all_caps,
            small_caps,
            underline: parse_optional_enum(start, b"u", TextUnderline::parse)?,
            strike: parse_optional_enum(start, b"strike", TextStrike::parse)?,
            spacing,
            baseline,
            source_attributes: text_attribute_tokens(start)?,
            source_tag: std::str::from_utf8(start.name().as_ref())
                .map_err(OxmlError::from)?
                .to_owned(),
            source_namespaces: text_fragment_scope(start, &TextNamespaceScope::default())?.physical,
            source_closure: text_owner_closure(
                &capture_empty_element(start)?,
                &text_fragment_scope(start, &TextNamespaceScope::default())?.physical,
            )?,
            source_cap_modeled: matches!(
                text_attr(start, b"cap")?.as_deref(),
                Some("all" | "small" | "none")
            ),
            raw_attributes,
            ..Self::default()
        })
    }

    fn from_element_with_namespaces(
        reader: &mut Reader<&[u8]>,
        start: &BytesStart<'_>,
        inherited: &TextNamespaceScope,
    ) -> Result<Self> {
        let namespaces = text_fragment_scope(start, inherited)?;
        let expected = local_name(start.name().as_ref()).to_vec();
        let mut properties = Self::from_start(start)?;
        properties.source_namespaces = namespaces.physical.clone();
        properties.source_closure = text_owner_closure(
            &capture_element(&mut reader.clone(), start)?,
            &properties.source_namespaces,
        )?;
        let mut boundary = 0;
        let mut seen = [false; 14];
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element) => {
                    let name = text_owned_name(&element, &namespaces)?;
                    let raw = capture_element(reader, &element)?;
                    properties.capture_child(&name, raw, &mut boundary, &mut seen, &namespaces)?;
                }
                Event::Empty(element) => {
                    let name = text_owned_name(&element, &namespaces)?;
                    let raw = capture_empty_element(&element)?;
                    properties.capture_child(&name, raw, &mut boundary, &mut seen, &namespaces)?;
                }
                Event::End(element) if matches_local_name(element.name().as_ref(), &expected) => {
                    return Ok(properties);
                }
                Event::Eof => return Err(missing_end(&String::from_utf8_lossy(&expected))),
                event @ (Event::Comment(_)
                | Event::PI(_)
                | Event::Text(_)
                | Event::GeneralRef(_)
                | Event::CData(_)) => {
                    properties
                        .raw_children
                        .push(boundary, text_capture_opaque_event(event)?);
                }
                _ => {}
            }
            buffer.clear();
        }
    }

    fn capture_child(
        &mut self,
        name: &[u8],
        raw: Vec<u8>,
        boundary: &mut usize,
        seen: &mut [bool; 14],
        namespaces: &TextNamespaceScope,
    ) -> Result<()> {
        let mut source_reader = Reader::from_reader(raw.as_slice());
        let physical_owner = match source_reader.read_event().map_err(OxmlError::from)? {
            Event::Start(start) | Event::Empty(start) => {
                let name = start.name();
                text_has_physical_owner(
                    std::str::from_utf8(name.as_ref()).map_err(OxmlError::from)?,
                    &text_namespace_scope(&start, &namespaces.physical)?,
                )
            }
            _ => false,
        };
        // Namespace lookalikes do not consume the owned highlight schema slot.
        let name = if name == b"highlight"
            && !character_highlight_is_drawingml(&raw, &self.highlight_scope_attributes())?
        {
            b"".as_slice()
        } else {
            name
        };
        let slot = character_property_slot(name);
        if let Some(slot) = slot {
            if seen[slot - 1] {
                return Err(duplicate(&String::from_utf8_lossy(name)));
            }
            seen[slot - 1] = true;
        }
        match name {
            name if is_fill(name) => {
                let value = Fill::from_xml(&raw)?;
                self.source_fill = physical_owner.then(|| (raw, value.clone()));
                self.fill = Some(value);
            }
            b"highlight" => {
                let (colour, range) =
                    character_highlight_colour(&raw, &self.highlight_scope_attributes())?;
                self.highlight = Some(colour.clone());
                self.highlight_source = Some((raw, range, colour));
            }
            b"latin" => {
                let value = TextFont::from_xml(&raw, b"latin")?;
                self.source_latin = physical_owner.then(|| (raw, value.clone()));
                self.latin = Some(value);
            }
            b"ea" => {
                let value = TextFont::from_xml(&raw, b"ea")?;
                self.source_east_asian = physical_owner.then(|| (raw, value.clone()));
                self.east_asian = Some(value);
            }
            b"cs" => {
                let value = TextFont::from_xml(&raw, b"cs")?;
                self.source_complex_script = physical_owner.then(|| (raw, value.clone()));
                self.complex_script = Some(value);
            }
            b"sym" => {
                let value = TextFont::from_xml(&raw, b"sym")?;
                self.source_symbol = physical_owner.then(|| (raw, value.clone()));
                self.symbol = Some(value);
            }
            b"hlinkClick" => {
                self.hyperlink_click = Some(TextHyperlink::from_xml_with_namespaces(
                    &raw,
                    b"hlinkClick",
                    namespaces,
                )?)
            }
            b"hlinkMouseOver" => {
                self.hyperlink_mouse_over = Some(TextHyperlink::from_xml_with_namespaces(
                    &raw,
                    b"hlinkMouseOver",
                    namespaces,
                )?)
            }
            _ => self.raw_children.push(
                slot.map_or(*boundary, |slot| (*boundary).max(slot - 1)),
                raw,
            ),
        }
        if let Some(slot) = slot {
            *boundary = (*boundary).max(slot);
        }
        Ok(())
    }

    /// Writes character properties with a caller-selected fixed `a:` tag.
    pub fn write_xml<W: Write>(&self, writer: &mut Writer<W>, tag: &str) -> Result<()> {
        let owned_tag = text_source_tag(tag, &self.source_tag, &self.source_namespaces);
        let tag = owned_tag.as_str();
        if self.all_caps == Some(true) && self.small_caps == Some(true) {
            return Err(invalid_attribute(tag, "cap", "conflicting all and small"));
        }
        self.write_highlight(&mut Writer::new(Vec::new()))?;
        let mut start = BytesStart::new(tag);
        if let Some(value) = self.font_size {
            parse_i32_value(tag, "sz", &value.to_string(), 100, MAX_TEXT_POINT)?;
            push_owned_attr(&mut start, "sz", value.to_string());
        }
        push_optional_bool(&mut start, "b", self.bold);
        push_optional_bool(&mut start, "i", self.italic);
        let caps = if self.all_caps == Some(true) {
            Some("all")
        } else if self.small_caps == Some(true) {
            Some("small")
        } else if self.all_caps == Some(false) || self.small_caps == Some(false) {
            Some("none")
        } else {
            None
        };
        if let Some(value) = caps {
            start.push_attribute(("cap", value));
        }
        if let Some(value) = self.underline {
            start.push_attribute(("u", value.as_str()));
        }
        if let Some(value) = self.strike {
            start.push_attribute(("strike", value.as_str()));
        }
        if let Some(value) = &self.spacing {
            push_owned_attr(&mut start, "spc", value.as_xml()?);
        }
        if let Some(value) = self.baseline.as_deref() {
            validate_baseline(value)?;
            start.push_attribute(("baseline", value));
        }
        if self.source_attributes.is_empty() {
            push_attributes(&mut start, &self.raw_attributes);
        } else {
            let modeled = ["sz", "b", "i", "u", "strike", "spc", "baseline"];
            let preserved: Vec<String> = self
                .source_attributes
                .iter()
                .filter(|(key, _)| {
                    !modeled.contains(&key.as_str())
                        && !(key == "cap"
                            && (self.source_cap_modeled
                                || self.all_caps.is_some()
                                || self.small_caps.is_some()))
                })
                .map(|(_, token)| token.clone())
                .collect();
            let generated = std::str::from_utf8(start.as_ref())
                .map_err(OxmlError::from)?
                .to_owned();
            start =
                BytesStart::from_content(format!("{generated}{}", preserved.concat()), tag.len());
        }

        let existing_keys: Vec<String> = start
            .attributes()
            .map(|attribute| {
                attribute.map_err(OxmlError::from).and_then(|attribute| {
                    std::str::from_utf8(attribute.key.as_ref())
                        .map(str::to_owned)
                        .map_err(OxmlError::from)
                })
            })
            .collect::<std::result::Result<_, OxmlError>>()?;
        for (prefix, uri) in &self.source_closure {
            let key = if prefix.is_empty() {
                "xmlns".to_owned()
            } else {
                format!("xmlns:{prefix}")
            };
            if !existing_keys.contains(&key) {
                start.push_attribute((key.as_str(), uri.as_str()));
            }
        }
        let has_modelled_children = self.highlight.is_some()
            || self.highlight_source.is_some()
            || self.fill.is_some()
            || self.latin.is_some()
            || self.east_asian.is_some()
            || self.complex_script.is_some()
            || self.symbol.is_some()
            || self.hyperlink_click.is_some()
            || self.hyperlink_mouse_over.is_some();
        if !has_modelled_children && self.raw_children.is_empty() {
            return write_empty(writer, start);
        }
        write_start(writer, start)?;
        emit_raw(writer, self.raw_children.at(0))?;
        emit_raw(writer, self.raw_children.at(1))?;
        if let Some(fill) = &self.fill {
            let mut child = Writer::new(Vec::new());
            {
                let writer = &mut child;
                if let Some((raw, original)) = &self.source_fill {
                    if fill == original {
                        writer.get_mut().write_all(raw).map_err(OxmlError::from)?;
                    } else {
                        fill.write_xml(writer)?;
                    }
                } else {
                    fill.write_xml(writer)?;
                }
            }
            writer
                .get_mut()
                .write_all(&text_close_generated_child(
                    &child.into_inner(),
                    &self.source_namespaces,
                )?)
                .map_err(OxmlError::from)?;
        }
        for boundary in 2..=6 {
            if boundary == 4 {
                let mut child = Writer::new(Vec::new());
                self.write_highlight(&mut child)?;
                writer
                    .get_mut()
                    .write_all(&text_close_generated_child(
                        &child.into_inner(),
                        &self.source_namespaces,
                    )?)
                    .map_err(OxmlError::from)?;
            }
            emit_raw(writer, self.raw_children.at(boundary))?;
        }
        if let Some(font) = &self.latin {
            let mut child = Writer::new(Vec::new());
            {
                let writer = &mut child;
                if let Some((raw, original)) = &self.source_latin {
                    if font == original {
                        writer.get_mut().write_all(raw).map_err(OxmlError::from)?;
                    } else {
                        font.write_xml(writer, "a:latin")?;
                    }
                } else {
                    font.write_xml(writer, "a:latin")?;
                }
            }
            writer
                .get_mut()
                .write_all(&text_close_generated_child(
                    &child.into_inner(),
                    &self.source_namespaces,
                )?)
                .map_err(OxmlError::from)?;
        }
        emit_raw(writer, self.raw_children.at(7))?;
        if let Some(font) = &self.east_asian {
            let mut child = Writer::new(Vec::new());
            {
                let writer = &mut child;
                if let Some((raw, original)) = &self.source_east_asian {
                    if font == original {
                        writer.get_mut().write_all(raw).map_err(OxmlError::from)?;
                    } else {
                        font.write_xml(writer, "a:ea")?;
                    }
                } else {
                    font.write_xml(writer, "a:ea")?;
                }
            }
            writer
                .get_mut()
                .write_all(&text_close_generated_child(
                    &child.into_inner(),
                    &self.source_namespaces,
                )?)
                .map_err(OxmlError::from)?;
        }
        emit_raw(writer, self.raw_children.at(8))?;
        if let Some(font) = &self.complex_script {
            let mut child = Writer::new(Vec::new());
            {
                let writer = &mut child;
                if let Some((raw, original)) = &self.source_complex_script {
                    if font == original {
                        writer.get_mut().write_all(raw).map_err(OxmlError::from)?;
                    } else {
                        font.write_xml(writer, "a:cs")?;
                    }
                } else {
                    font.write_xml(writer, "a:cs")?;
                }
            }
            writer
                .get_mut()
                .write_all(&text_close_generated_child(
                    &child.into_inner(),
                    &self.source_namespaces,
                )?)
                .map_err(OxmlError::from)?;
        }
        emit_raw(writer, self.raw_children.at(9))?;
        if let Some(font) = &self.symbol {
            let mut child = Writer::new(Vec::new());
            {
                let writer = &mut child;
                if let Some((raw, original)) = &self.source_symbol {
                    if font == original {
                        writer.get_mut().write_all(raw).map_err(OxmlError::from)?;
                    } else {
                        font.write_xml(writer, "a:sym")?;
                    }
                } else {
                    font.write_xml(writer, "a:sym")?;
                }
            }
            writer
                .get_mut()
                .write_all(&text_close_generated_child(
                    &child.into_inner(),
                    &self.source_namespaces,
                )?)
                .map_err(OxmlError::from)?;
        }
        emit_raw(writer, self.raw_children.at(10))?;
        if let Some(hyperlink) = &self.hyperlink_click {
            let mut child = Writer::new(Vec::new());
            {
                let writer = &mut child;
                hyperlink.write_xml(writer, "a:hlinkClick")?;
            }
            writer
                .get_mut()
                .write_all(&text_close_generated_child(
                    &child.into_inner(),
                    &self.source_namespaces,
                )?)
                .map_err(OxmlError::from)?;
        }
        emit_raw(writer, self.raw_children.at(11))?;
        if let Some(hyperlink) = &self.hyperlink_mouse_over {
            let mut child = Writer::new(Vec::new());
            {
                let writer = &mut child;
                hyperlink.write_xml(writer, "a:hlinkMouseOver")?;
            }
            writer
                .get_mut()
                .write_all(&text_close_generated_child(
                    &child.into_inner(),
                    &self.source_namespaces,
                )?)
                .map_err(OxmlError::from)?;
        }
        for boundary in 12..=14 {
            emit_raw(writer, self.raw_children.at(boundary))?;
        }
        write_end(writer, tag)
    }

    /// Changes only the owned highlight colour after checking opaque-source preservation.
    pub fn set_highlight(&mut self, value: Option<ColorChoice>) -> Result<()> {
        let mut candidate = self.clone();
        candidate.highlight = value;
        candidate.write_highlight(&mut Writer::new(Vec::new()))?;
        *self = candidate;
        Ok(())
    }

    fn write_highlight<W: Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        if let Some((raw, _, original)) = &self.highlight_source
            && self.highlight.as_ref() != Some(original)
            && highlight_has_opaque_transform_attributes(raw, &self.highlight_scope_attributes())?
        {
            return Err(invalid_attribute(
                "highlight",
                "transform",
                "opaque transform attributes cannot be replaced or cleared",
            ));
        }
        match (&self.highlight_source, &self.highlight) {
            (Some((raw, _, original)), Some(value)) if original == value => {
                writer.get_mut().write_all(raw).map_err(OxmlError::from)?;
            }
            (Some((raw, range, original)), Some(value)) => {
                if !original.raw_children().is_empty()
                    && original.raw_children() != value.raw_children()
                {
                    return Err(invalid_attribute(
                        "highlight",
                        "colour",
                        "opaque colour children cannot be replaced",
                    ));
                }
                let mut reader = Reader::from_reader(&raw[range.clone()]);
                let event = reader.read_event().map_err(OxmlError::from)?;
                let start = match event {
                    Event::Start(start) | Event::Empty(start) => start,
                    _ => return Err(missing_end("highlight colour")),
                };
                if start.attributes().with_checks(false).any(|attribute| {
                    attribute.map_or(true, |attribute| {
                        let name = attribute.key.as_ref();
                        !matches!(name, b"val" | b"lastClr" | b"xmlns")
                            && !name.starts_with(b"xmlns:")
                    })
                }) {
                    return Err(invalid_attribute(
                        "highlight",
                        "colour",
                        "opaque colour attributes cannot be replaced",
                    ));
                }
                writer
                    .get_mut()
                    .write_all(&raw[..range.start])
                    .map_err(OxmlError::from)?;
                let namespaces = start
                    .attributes()
                    .with_checks(false)
                    .filter_map(|attribute| attribute.ok())
                    .filter(|attribute| {
                        attribute.key.as_ref() == b"xmlns"
                            || attribute.key.as_ref().starts_with(b"xmlns:")
                    })
                    .map(|attribute| {
                        Ok((
                            String::from_utf8_lossy(attribute.key.as_ref()).into_owned(),
                            attribute
                                .normalized_value(XmlVersion::Implicit1_0)
                                .map_err(OxmlError::from)?
                                .into_owned(),
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?;
                if namespaces
                    .iter()
                    .any(|(name, uri)| name == "xmlns:a" && uri != A_NS)
                {
                    return Err(invalid_attribute(
                        "highlight",
                        "colour",
                        "rebound canonical namespace cannot be replaced",
                    ));
                }
                write_namespace_closed_highlight_colour(writer, value, &namespaces)?;
                writer
                    .get_mut()
                    .write_all(&raw[range.end..])
                    .map_err(OxmlError::from)?;
            }
            (Some((raw, range, original)), None) => {
                if !original.raw_children().is_empty() {
                    return Err(invalid_attribute(
                        "highlight",
                        "colour",
                        "opaque colour children cannot be cleared",
                    ));
                }
                let mut colour_reader = Reader::from_reader(&raw[range.clone()]);
                let colour_event = colour_reader.read_event().map_err(OxmlError::from)?;
                let colour_start = match colour_event {
                    Event::Start(start) | Event::Empty(start) => start,
                    _ => return Err(missing_end("highlight colour")),
                };
                if colour_start
                    .attributes()
                    .with_checks(false)
                    .any(|attribute| {
                        attribute.map_or(true, |attribute| {
                            let name = attribute.key.as_ref();
                            !matches!(name, b"val" | b"lastClr" | b"xmlns")
                                && !name.starts_with(b"xmlns:")
                        })
                    })
                {
                    return Err(invalid_attribute(
                        "highlight",
                        "colour",
                        "opaque colour attributes cannot be cleared",
                    ));
                }
                // A colourless wrapper cannot preserve opaque payload and remain valid.
                let mut reader = Reader::from_reader(raw.as_slice());
                let first = reader.read_event().map_err(OxmlError::from)?;
                let Event::Start(start) = first else {
                    return Err(invalid_attribute("highlight", "source", "not a wrapper"));
                };
                if start.attributes().with_checks(false).any(|item| {
                    item.map_or(true, |attribute| {
                        let name = attribute.key.as_ref();
                        name != b"xmlns" && !name.starts_with(b"xmlns:")
                    })
                }) {
                    return Err(invalid_attribute(
                        "highlight",
                        "source",
                        "opaque wrapper attributes cannot be cleared",
                    ));
                }
                let prefix_end = reader.buffer_position() as usize;
                let suffix_start = raw[range.end..]
                    .iter()
                    .position(|byte| *byte == b'<')
                    .map(|offset| range.end + offset)
                    .ok_or_else(|| missing_end("highlight"))?;
                if !raw[prefix_end..range.start]
                    .iter()
                    .all(u8::is_ascii_whitespace)
                    || !raw[range.end..suffix_start]
                        .iter()
                        .all(u8::is_ascii_whitespace)
                    || !raw[suffix_start..].starts_with(b"</")
                {
                    return Err(invalid_attribute(
                        "highlight",
                        "source",
                        "opaque wrapper children cannot be cleared",
                    ));
                }
            }
            (None, Some(value)) => {
                let mut start = BytesStart::new("a:highlight");
                start.push_attribute(("xmlns:a", A_NS));
                write_start(writer, start)?;
                value.to_xml(writer)?;
                write_end(writer, "a:highlight")?;
            }
            (None, None) => {}
        }
        Ok(())
    }

    /// Updates direct all-caps without leaving a conflicting small-caps mode.
    pub fn set_all_caps(&mut self, value: Option<bool>) {
        self.all_caps = value;
        if value.is_some() {
            self.small_caps = None;
        }
    }

    /// Updates direct small-caps without leaving a conflicting all-caps mode.
    pub fn set_small_caps(&mut self, value: Option<bool>) {
        self.small_caps = value;
        if value.is_some() {
            self.all_caps = None;
        }
    }

    pub fn raw_children(&self) -> &OrderedRawChildren {
        &self.raw_children
    }

    /// Returns these properties as formatting for new text, without the click
    /// and mouse-over hyperlinks, which name relationships rather than format.
    pub(crate) fn without_hyperlinks(mut self) -> Self {
        self.hyperlink_click = None;
        self.hyperlink_mouse_over = None;
        self
    }
}

fn write_namespace_closed_highlight_colour<W: Write>(
    writer: &mut Writer<W>,
    colour: &ColorChoice,
    namespaces: &[(String, String)],
) -> Result<()> {
    let mut canonical = Writer::new(Vec::new());
    colour.to_xml(&mut canonical)?;
    let bytes = canonical.into_inner();
    let mut reader = Reader::from_reader(bytes.as_slice());
    match reader.read_event().map_err(OxmlError::from)? {
        Event::Start(start) => {
            let mut start = start.into_owned();
            start.push_attribute(("xmlns:a", A_NS));
            for (name, value) in namespaces {
                if name != "xmlns:a" {
                    start.push_attribute((name.as_str(), value.as_str()));
                }
            }
            write_start(writer, start)?;
        }
        Event::Empty(start) => {
            let mut start = start.into_owned();
            start.push_attribute(("xmlns:a", A_NS));
            for (name, value) in namespaces {
                if name != "xmlns:a" {
                    start.push_attribute((name.as_str(), value.as_str()));
                }
            }
            write_empty(writer, start)?;
        }
        _ => return Err(invalid_attribute("highlight", "colour", "missing colour")),
    }
    writer
        .get_mut()
        .write_all(&bytes[reader.buffer_position() as usize..])
        .map_err(OxmlError::from)?;
    Ok(())
}

// Imported modeled transforms have no attribute-source carrier. Refuse an edit
// that would regenerate one and discard opaque attributes. Unchanged source
// remains verbatim, and namespace-foreign transform lookalikes remain raw.
fn highlight_has_opaque_transform_attributes(
    raw: &[u8],
    attributes: &[(String, String)],
) -> Result<bool> {
    let mut scope = BytesStart::new("scope");
    if !attributes.iter().any(|(name, _)| name == "xmlns:a") {
        scope.push_attribute(("xmlns:a", A_NS));
    }
    for (name, value) in attributes {
        if name == "xmlns" || name.starts_with("xmlns:") {
            scope.push_attribute((name.as_str(), value.as_str()));
        }
    }
    let mut writer = Writer::new(Vec::new());
    write_start(&mut writer, scope)?;
    writer.get_mut().extend_from_slice(raw);
    write_end(&mut writer, "scope")?;
    let xml = writer.into_inner();
    let mut reader = NsReader::from_reader(xml.as_slice());
    let mut depth = 0usize;
    let mut buffer = Vec::new();
    let mut started_transform = None;
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(OxmlError::from)?;
        match event {
            Event::Start(start) => {
                if depth == 3
                    && matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == A_NS.as_bytes())
                    && ColorTransform::from_xml(&start)?.is_some()
                    && start.attributes().with_checks(false).any(|attribute| {
                        attribute.map_or(true, |attribute| {
                            let name = attribute.key.as_ref();
                            name != b"val" && name != b"xmlns" && !name.starts_with(b"xmlns:")
                        })
                    })
                {
                    started_transform = Some(reader.buffer_position() as usize);
                }
                depth += 1;
            }
            Event::Empty(start) => {
                if depth == 3
                    && matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == A_NS.as_bytes())
                    && ColorTransform::from_xml(&start)?.is_some()
                    && start.attributes().with_checks(false).any(|attribute| {
                        attribute.map_or(true, |attribute| {
                            let name = attribute.key.as_ref();
                            name != b"val" && name != b"xmlns" && !name.starts_with(b"xmlns:")
                        })
                    })
                {
                    return Ok(true);
                }
            }
            Event::End(_) => {
                if depth == 4
                    && let Some(content_start) = started_transform.take()
                    && xml[content_start..before]
                        .iter()
                        .all(u8::is_ascii_whitespace)
                {
                    return Ok(true);
                }
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| missing_end("highlight"))?;
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
        buffer.clear();
    }
}

fn character_highlight_colour(
    raw: &[u8],
    attributes: &[(String, String)],
) -> Result<(ColorChoice, std::ops::Range<usize>)> {
    let mut scope = BytesStart::new("scope");
    if !attributes.iter().any(|(name, _)| name == "xmlns:a") {
        scope.push_attribute(("xmlns:a", A_NS));
    }
    for (name, value) in attributes {
        if name == "xmlns" || name.starts_with("xmlns:") {
            scope.push_attribute((name.as_str(), value.as_str()));
        }
    }
    let mut writer = Writer::new(Vec::new());
    write_start(&mut writer, scope)?;
    let prefix = writer.get_ref().len();
    writer.get_mut().extend_from_slice(raw);
    write_end(&mut writer, "scope")?;
    let xml = writer.into_inner();
    let mut reader = NsReader::from_reader(xml.as_slice());
    let mut depth = 0usize;
    let mut selected = None;
    let mut colour_start = None;
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(OxmlError::from)?;
        match event {
            Event::Start(start) => {
                if depth == 2
                    && matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == A_NS.as_bytes())
                    && matches!(
                        local_name(start.name().as_ref()),
                        b"scrgbClr"
                            | b"srgbClr"
                            | b"hslClr"
                            | b"sysClr"
                            | b"schemeClr"
                            | b"prstClr"
                    )
                {
                    if selected.is_some() || colour_start.is_some() {
                        return Err(duplicate("highlight colour"));
                    }
                    colour_start = Some(before);
                }
                depth += 1;
            }
            Event::Empty(start)
                if depth == 2
                    && matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == A_NS.as_bytes())
                    && matches!(
                        local_name(start.name().as_ref()),
                        b"scrgbClr"
                            | b"srgbClr"
                            | b"hslClr"
                            | b"sysClr"
                            | b"schemeClr"
                            | b"prstClr"
                    ) =>
            {
                if selected.is_some() {
                    return Err(duplicate("highlight colour"));
                }
                selected = Some(before..reader.buffer_position() as usize);
            }
            Event::End(_) => {
                if depth == 3
                    && let Some(start) = colour_start.take()
                {
                    selected = Some(start..reader.buffer_position() as usize);
                }
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| missing_end("highlight"))?;
                if depth == 0 {
                    break;
                }
            }
            Event::Eof => return Err(missing_end("highlight")),
            _ => {}
        }
        buffer.clear();
    }
    let range = selected
        .ok_or_else(|| invalid_attribute("highlight", "colour", "missing qualified colour"))?;
    let colour = qualified_highlight_colour(&xml, range.clone())?;
    Ok((colour, range.start - prefix..range.end - prefix))
}

fn qualified_highlight_colour(xml: &[u8], range: std::ops::Range<usize>) -> Result<ColorChoice> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut selected_start = None;
    let mut root_depth = None;
    let mut depth = 0usize;
    let mut transforms = Vec::new();
    let mut raw_children = OrderedRawChildren::default();
    let mut child_start = None;
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(OxmlError::from)?;
        match event {
            Event::Start(start) => {
                if before == range.start {
                    selected_start = Some(start.into_owned());
                    root_depth = Some(depth);
                } else if root_depth.is_some_and(|root| depth == root + 1) {
                    let transform = if matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == A_NS.as_bytes())
                    {
                        ColorTransform::from_xml(&start)?
                    } else {
                        None
                    };
                    child_start = Some((before, reader.buffer_position() as usize, transform));
                }
                depth += 1;
            }
            Event::Empty(start) if before == range.start => {
                return Ok(ColorChoice::from_parts(&start, transforms, raw_children)?);
            }
            Event::Empty(start) if root_depth.is_some_and(|root| depth == root + 1) => {
                let qualified = matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == A_NS.as_bytes());
                let transform = if qualified {
                    ColorTransform::from_xml(&start)?
                } else {
                    None
                };
                if let Some(transform) = transform {
                    transforms.push(transform);
                } else {
                    raw_children.push(
                        transforms.len(),
                        xml[before..reader.buffer_position() as usize].to_vec(),
                    );
                }
            }
            Event::End(_) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| missing_end("highlight colour"))?;
                if root_depth.is_some_and(|root| depth == root + 1) {
                    let (start, content_start, transform) = child_start
                        .take()
                        .ok_or_else(|| missing_end("highlight colour child"))?;
                    if let Some(transform) = transform.filter(|_| {
                        xml[content_start..before]
                            .iter()
                            .all(u8::is_ascii_whitespace)
                    }) {
                        transforms.push(transform);
                    } else {
                        raw_children.push(
                            transforms.len(),
                            xml[start..reader.buffer_position() as usize].to_vec(),
                        );
                    }
                }
                if root_depth == Some(depth) {
                    return Ok(ColorChoice::from_parts(
                        &selected_start.ok_or_else(|| missing_end("highlight colour"))?,
                        transforms,
                        raw_children,
                    )?);
                }
            }
            Event::Eof => return Err(missing_end("highlight colour")),
            _ if root_depth.is_some_and(|root| depth == root + 1) => {
                raw_children.push(
                    transforms.len(),
                    xml[before..reader.buffer_position() as usize].to_vec(),
                );
            }
            _ => {}
        }
        buffer.clear();
    }
}

fn character_highlight_is_drawingml(raw: &[u8], attributes: &[(String, String)]) -> Result<bool> {
    let mut scope = BytesStart::new("scope");
    if !attributes.iter().any(|(name, _)| name == "xmlns:a") {
        scope.push_attribute(("xmlns:a", A_NS));
    }
    for (name, value) in attributes {
        if name == "xmlns" || name.starts_with("xmlns:") {
            scope.push_attribute((name.as_str(), value.as_str()));
        }
    }
    let mut writer = Writer::new(Vec::new());
    write_start(&mut writer, scope)?;
    writer.get_mut().extend_from_slice(raw);
    write_end(&mut writer, "scope")?;
    let xml = writer.into_inner();
    let mut reader = NsReader::from_reader(xml.as_slice());
    let mut buffer = Vec::new();
    let mut in_scope = false;
    loop {
        match reader
            .read_resolved_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            (_, Event::Start(_)) if !in_scope => in_scope = true,
            (namespace, Event::Start(_) | Event::Empty(_)) => {
                return Ok(
                    matches!(namespace, ResolveResult::Bound(Namespace(uri)) if uri == A_NS.as_bytes()),
                );
            }
            (_, Event::Eof) => return Err(missing_end("highlight")),
            _ => {}
        }
        buffer.clear();
    }
}

fn character_property_slot(name: &[u8]) -> Option<usize> {
    match name {
        b"ln" => Some(1),
        name if is_fill(name) => Some(2),
        b"effectLst" | b"effectDag" => Some(3),
        b"highlight" => Some(4),
        b"uLnTx" | b"uLn" => Some(5),
        b"uFillTx" | b"uFill" => Some(6),
        b"latin" => Some(7),
        b"ea" => Some(8),
        b"cs" => Some(9),
        b"sym" => Some(10),
        b"hlinkClick" => Some(11),
        b"hlinkMouseOver" => Some(12),
        b"rtl" => Some(13),
        b"extLst" => Some(14),
        _ => None,
    }
}

fn is_character_property_tag(name: &[u8]) -> bool {
    matches!(name, b"rPr" | b"defRPr" | b"endParaRPr")
}

fn is_fill(name: &[u8]) -> bool {
    matches!(
        name,
        b"noFill" | b"solidFill" | b"gradFill" | b"pattFill" | b"blipFill"
    )
}

/// Paragraph attributes and the story-owned spacing and default-run children.
#[allow(non_camel_case_types)]
#[derive(Clone, Debug, Default)]
pub struct CT_TextParagraphProperties {
    pub left_margin: Option<i32>,
    pub right_margin: Option<i32>,
    pub level: Option<u8>,
    pub indent: Option<i32>,
    pub alignment: Option<TextAlignment>,
    pub right_to_left: Option<bool>,
    pub line_spacing: Option<TextSpacing>,
    pub space_before: Option<TextSpacing>,
    pub space_after: Option<TextSpacing>,
    pub bullet: Option<TextBullet>,
    pub default_run_properties: Option<CT_TextCharacterProperties>,
    raw_attributes: Vec<(String, String)>,
    source_attributes: Vec<(String, String)>,
    source_tag: String,
    source_xml: Option<Vec<u8>>,
    source_projection: Vec<u8>,
    source_namespaces: Vec<(String, String)>,
    raw_children: OrderedRawChildren,
}
impl PartialEq for CT_TextParagraphProperties {
    fn eq(&self, other: &Self) -> bool {
        self.left_margin == other.left_margin
            && self.right_margin == other.right_margin
            && self.level == other.level
            && self.indent == other.indent
            && self.alignment == other.alignment
            && self.right_to_left == other.right_to_left
            && self.line_spacing == other.line_spacing
            && self.space_before == other.space_before
            && self.space_after == other.space_after
            && self.bullet == other.bullet
            && self.default_run_properties == other.default_run_properties
            && {
                let mut left = self.raw_attributes.clone();
                let mut right = other.raw_attributes.clone();
                let inherited_binding =
                    |key: &str, value: &str, namespaces: &[(String, String)]| {
                        let prefix = if key == "xmlns" {
                            Some("")
                        } else {
                            key.strip_prefix("xmlns:")
                        };
                        prefix.is_some_and(|prefix| {
                            namespaces
                                .iter()
                                .any(|(name, uri)| name == prefix && uri == value)
                        })
                    };
                left.retain(|(key, value)| {
                    !(key == "xmlns:r" && value == TEXT_REL_NS)
                        && !inherited_binding(key, value, &other.source_namespaces)
                });
                right.retain(|(key, value)| {
                    !(key == "xmlns:r" && value == TEXT_REL_NS)
                        && !inherited_binding(key, value, &self.source_namespaces)
                });
                left.sort();
                right.sort();
                left == right
            }
            && matches!(
                (
                    text_owner_closure(self.source_xml.as_deref().unwrap_or_default(), &self.source_namespaces),
                    text_owner_closure(other.source_xml.as_deref().unwrap_or_default(), &other.source_namespaces),
                ),
                (Ok(left), Ok(right)) if left == right
            )
            && self.raw_children == other.raw_children
    }
}
impl Eq for CT_TextParagraphProperties {}

impl CT_TextParagraphProperties {
    /// Returns local paragraph properties with absent groups inherited from defaults.
    pub fn with_inherited_defaults(&self, defaults: &Self) -> Result<Self> {
        let xml = text_inherit_property_xml(
            &self.to_xml()?,
            &self.source_namespaces,
            &defaults.to_xml()?,
            &defaults.source_namespaces,
        )?;
        Self::from_xml_with_namespaces(
            &xml,
            &TextNamespaceScope::inherited(&self.source_namespaces),
        )
    }

    /// Parses one complete paragraph-property element with any prefix.
    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        let expected = root_local_name(xml)?;
        if !is_paragraph_property_tag(&expected) {
            return Err(TextError::UnexpectedElement(
                String::from_utf8_lossy(&expected).into_owned(),
            ));
        }
        Self::from_xml_with_namespaces(xml, &TextNamespaceScope::default())
    }

    pub(crate) fn from_xml_with_namespaces(
        xml: &[u8],
        inherited: &TextNamespaceScope,
    ) -> Result<Self> {
        let expected = root_local_name(xml)?;
        let mut parsed = parse_complete(
            xml,
            &expected,
            |reader, start| Self::from_element_model(reader, start, inherited),
            |start| Self::from_start_with_namespaces(start, inherited),
        )?;
        let mut writer = Writer::new(Vec::new());
        parsed.write_xml(&mut writer, "a:pPr")?;
        parsed.source_projection = writer.into_inner();
        parsed.source_xml = Some(xml.to_vec());
        Ok(parsed)
    }

    fn from_start_with_namespaces(
        start: &BytesStart<'_>,
        inherited: &TextNamespaceScope,
    ) -> Result<Self> {
        let mut parsed = Self::from_start(start)?;
        parsed.source_namespaces = text_fragment_scope(start, inherited)?.physical;
        Ok(parsed)
    }

    fn from_start(start: &BytesStart<'_>) -> Result<Self> {
        text_validate_root(start)?;
        let element = String::from_utf8_lossy(local_name(start.name().as_ref())).into_owned();
        let level = text_attr(start, b"lvl")?
            .map(|value| parse_i32_value(&element, "lvl", &value, 0, 8).map(|v| v as u8))
            .transpose()?;
        Ok(Self {
            left_margin: parse_optional_i32(start, b"marL", 0, MAX_TEXT_MARGIN)?,
            right_margin: parse_optional_i32(start, b"marR", 0, MAX_TEXT_MARGIN)?,
            level,
            indent: parse_optional_i32(start, b"indent", -MAX_TEXT_MARGIN, MAX_TEXT_MARGIN)?,
            alignment: parse_optional_enum(start, b"algn", TextAlignment::parse)?,
            right_to_left: parse_optional_bool(start, b"rtl")?,
            source_attributes: text_attribute_tokens(start)?,
            source_tag: std::str::from_utf8(start.name().as_ref())
                .map_err(OxmlError::from)?
                .to_owned(),
            raw_attributes: capture_raw_attributes(
                start,
                &[b"marL", b"marR", b"lvl", b"indent", b"algn"],
            )?,
            ..Self::default()
        })
    }

    fn from_element_with_namespaces(
        reader: &mut Reader<&[u8]>,
        start: &BytesStart<'_>,
        inherited: &TextNamespaceScope,
    ) -> Result<Self> {
        let raw = capture_element(reader, start)?;
        Self::from_xml_with_namespaces(&raw, inherited)
    }

    fn from_element_model(
        reader: &mut Reader<&[u8]>,
        start: &BytesStart<'_>,
        inherited: &TextNamespaceScope,
    ) -> Result<Self> {
        let namespaces = text_fragment_scope(start, inherited)?;
        let expected = local_name(start.name().as_ref()).to_vec();
        let mut properties = Self::from_start(start)?;
        properties.source_namespaces = namespaces.physical.clone();
        let mut boundary = 0;
        let mut seen = [false; 10];
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element) => {
                    let name = text_owned_name(&element, &namespaces)?;
                    let raw = capture_element(reader, &element)?;
                    properties.capture_child(&name, raw, &mut boundary, &mut seen, &namespaces)?;
                }
                Event::Empty(element) => {
                    let name = text_owned_name(&element, &namespaces)?;
                    let raw = capture_empty_element(&element)?;
                    properties.capture_child(&name, raw, &mut boundary, &mut seen, &namespaces)?;
                }
                Event::End(element) if matches_local_name(element.name().as_ref(), &expected) => {
                    return Ok(properties);
                }
                Event::Eof => return Err(missing_end(&String::from_utf8_lossy(&expected))),
                event @ (Event::Comment(_)
                | Event::PI(_)
                | Event::Text(_)
                | Event::GeneralRef(_)
                | Event::CData(_)) => {
                    properties
                        .raw_children
                        .push(boundary, text_capture_opaque_event(event)?);
                }
                _ => {}
            }
            buffer.clear();
        }
    }

    fn capture_child(
        &mut self,
        name: &[u8],
        raw: Vec<u8>,
        boundary: &mut usize,
        seen: &mut [bool; 10],
        namespaces: &TextNamespaceScope,
    ) -> Result<()> {
        if namespaces.fixed_list_style_children && matches!(name, b"lnSpc" | b"spcBef" | b"spcAft")
        {
            let mut spacing = Reader::from_reader(raw.as_slice());
            loop {
                match spacing.read_event().map_err(OxmlError::from)? {
                    Event::Start(start) | Event::Empty(start) => {
                        crate::namespace::reject_conflicting_a_prefix(&start)?;
                    }
                    Event::Eof => break,
                    _ => {}
                }
            }
        }
        let slot = paragraph_property_slot(name);
        if let Some(slot) = slot {
            if seen[slot - 1] {
                return Err(duplicate(&String::from_utf8_lossy(name)));
            }
            seen[slot - 1] = true;
        }
        match name {
            b"lnSpc" => self.line_spacing = Some(TextSpacing::from_xml(&raw, b"lnSpc")?),
            b"spcBef" => self.space_before = Some(TextSpacing::from_xml(&raw, b"spcBef")?),
            b"spcAft" => self.space_after = Some(TextSpacing::from_xml(&raw, b"spcAft")?),
            b"defRPr" => {
                self.default_run_properties = Some(
                    CT_TextCharacterProperties::from_xml_with_namespaces(&raw, namespaces)?,
                )
            }
            _ => {
                let mut bullet = self.bullet.take().unwrap_or_default();
                if bullet.capture_component(name, &raw)? {
                    self.bullet = Some(bullet);
                } else {
                    if !bullet.is_empty() {
                        self.bullet = Some(bullet);
                    }
                    self.raw_children.push(
                        slot.map_or(*boundary, |slot| (*boundary).max(slot - 1)),
                        raw,
                    );
                }
            }
        }
        if let Some(slot) = slot {
            *boundary = (*boundary).max(slot);
        }
        Ok(())
    }

    /// Writes paragraph properties with a caller-selected fixed `a:` tag.
    pub fn write_xml<W: Write>(&self, writer: &mut Writer<W>, tag: &str) -> Result<()> {
        let owned_tag = text_source_tag(tag, &self.source_tag, &self.source_namespaces);
        let tag = owned_tag.as_str();
        if text_has_physical_owner(&self.source_tag, &self.source_namespaces)
            && let Some(raw) = &self.source_xml
        {
            let mut candidate = self.clone();
            candidate.source_xml = None;
            let mut projection = Writer::new(Vec::new());
            candidate.write_xml(&mut projection, "a:pPr")?;
            if projection.into_inner() == self.source_projection {
                writer
                    .get_mut()
                    .write_all(&text_close_source_owner(raw, &self.source_namespaces, tag)?)
                    .map_err(OxmlError::from)?;
                return Ok(());
            }
        }
        let mut start = BytesStart::new(tag);
        if let Some(value) = self.left_margin {
            parse_i32_value(tag, "marL", &value.to_string(), 0, MAX_TEXT_MARGIN)?;
            push_owned_attr(&mut start, "marL", value.to_string());
        }
        if let Some(value) = self.right_margin {
            parse_i32_value(tag, "marR", &value.to_string(), 0, MAX_TEXT_MARGIN)?;
            push_owned_attr(&mut start, "marR", value.to_string());
        }
        if let Some(value) = self.level {
            if value > 8 {
                return Err(invalid_attribute(tag, "lvl", &value.to_string()));
            }
            push_owned_attr(&mut start, "lvl", value.to_string());
        }
        if let Some(value) = self.indent {
            parse_i32_value(
                tag,
                "indent",
                &value.to_string(),
                -MAX_TEXT_MARGIN,
                MAX_TEXT_MARGIN,
            )?;
            push_owned_attr(&mut start, "indent", value.to_string());
        }
        if let Some(value) = self.alignment {
            start.push_attribute(("algn", value.as_str()));
        }
        push_optional_bool(&mut start, "rtl", self.right_to_left);
        if self.source_attributes.is_empty() {
            for (name, value) in &self.raw_attributes {
                if name != "rtl" {
                    start.push_attribute((name.as_str(), value.as_str()));
                }
            }
        } else {
            let modeled = ["marL", "marR", "lvl", "indent", "algn", "rtl"];
            let preserved: Vec<String> = self
                .source_attributes
                .iter()
                .filter(|(key, _)| !modeled.contains(&key.as_str()))
                .map(|(_, token)| token.clone())
                .collect();
            let generated = std::str::from_utf8(start.as_ref())
                .map_err(OxmlError::from)?
                .to_owned();
            start =
                BytesStart::from_content(format!("{generated}{}", preserved.concat()), tag.len());
        }

        let existing_keys: Vec<String> = start
            .attributes()
            .map(|attribute| {
                attribute.map_err(OxmlError::from).and_then(|attribute| {
                    std::str::from_utf8(attribute.key.as_ref())
                        .map(str::to_owned)
                        .map_err(OxmlError::from)
                })
            })
            .collect::<std::result::Result<_, OxmlError>>()?;
        let source = self
            .source_xml
            .clone()
            .unwrap_or(capture_empty_element(&start)?);
        for (prefix, uri) in text_owner_closure(&source, &self.source_namespaces)? {
            let key = if prefix.is_empty() {
                "xmlns".to_owned()
            } else {
                format!("xmlns:{prefix}")
            };
            if !existing_keys.contains(&key) {
                start.push_attribute((key.as_str(), uri.as_str()));
            }
        }

        let modelled = self.line_spacing.is_some()
            || self.space_before.is_some()
            || self.space_after.is_some()
            || self.bullet.is_some()
            || self.default_run_properties.is_some();
        if !modelled && self.raw_children.is_empty() {
            return write_empty(writer, start);
        }
        write_start(writer, start)?;
        emit_raw(writer, self.raw_children.at(0))?;
        if let Some(spacing) = &self.line_spacing {
            {
                let mut child = Writer::new(Vec::new());
                spacing.write_xml(&mut child, "a:lnSpc")?;
                writer
                    .get_mut()
                    .write_all(&text_close_generated_child(
                        &child.into_inner(),
                        &self.source_namespaces,
                    )?)
                    .map_err(OxmlError::from)?;
            }
        }
        emit_raw(writer, self.raw_children.at(1))?;
        if let Some(spacing) = &self.space_before {
            {
                let mut child = Writer::new(Vec::new());
                spacing.write_xml(&mut child, "a:spcBef")?;
                writer
                    .get_mut()
                    .write_all(&text_close_generated_child(
                        &child.into_inner(),
                        &self.source_namespaces,
                    )?)
                    .map_err(OxmlError::from)?;
            }
        }
        emit_raw(writer, self.raw_children.at(2))?;
        if let Some(spacing) = &self.space_after {
            {
                let mut child = Writer::new(Vec::new());
                spacing.write_xml(&mut child, "a:spcAft")?;
                writer
                    .get_mut()
                    .write_all(&text_close_generated_child(
                        &child.into_inner(),
                        &self.source_namespaces,
                    )?)
                    .map_err(OxmlError::from)?;
            }
        }
        emit_raw(writer, self.raw_children.at(3))?;
        if let Some(bullet) = &self.bullet {
            {
                let mut child = Writer::new(Vec::new());
                bullet.write_color(&mut child)?;
                writer
                    .get_mut()
                    .write_all(&text_close_generated_child(
                        &child.into_inner(),
                        &self.source_namespaces,
                    )?)
                    .map_err(OxmlError::from)?;
            }
        }
        emit_raw(writer, self.raw_children.at(4))?;
        if let Some(bullet) = &self.bullet {
            {
                let mut child = Writer::new(Vec::new());
                bullet.write_size(&mut child)?;
                writer
                    .get_mut()
                    .write_all(&text_close_generated_child(
                        &child.into_inner(),
                        &self.source_namespaces,
                    )?)
                    .map_err(OxmlError::from)?;
            }
        }
        emit_raw(writer, self.raw_children.at(5))?;
        if let Some(bullet) = &self.bullet {
            {
                let mut child = Writer::new(Vec::new());
                bullet.write_font(&mut child)?;
                writer
                    .get_mut()
                    .write_all(&text_close_generated_child(
                        &child.into_inner(),
                        &self.source_namespaces,
                    )?)
                    .map_err(OxmlError::from)?;
            }
        }
        emit_raw(writer, self.raw_children.at(6))?;
        if let Some(bullet) = &self.bullet {
            {
                let mut child = Writer::new(Vec::new());
                bullet.write_choice(&mut child)?;
                writer
                    .get_mut()
                    .write_all(&text_close_generated_child(
                        &child.into_inner(),
                        &self.source_namespaces,
                    )?)
                    .map_err(OxmlError::from)?;
            }
        }
        emit_raw(writer, self.raw_children.at(7))?;
        emit_raw(writer, self.raw_children.at(8))?;
        if let Some(properties) = &self.default_run_properties {
            {
                let mut child = Writer::new(Vec::new());
                properties.write_xml(&mut child, "a:defRPr")?;
                writer
                    .get_mut()
                    .write_all(&text_close_generated_child(
                        &child.into_inner(),
                        &self.source_namespaces,
                    )?)
                    .map_err(OxmlError::from)?;
            }
        }
        emit_raw(writer, self.raw_children.at(9))?;
        emit_raw(writer, self.raw_children.at(10))?;
        write_end(writer, tag)
    }

    pub fn raw_children(&self) -> &OrderedRawChildren {
        &self.raw_children
    }

    /// Returns whether the paragraph keeps a preserved `a:buBlip` picture bullet.
    pub fn has_picture_bullet(&self) -> bool {
        (0..=10).any(|boundary| {
            self.raw_children
                .at(boundary)
                .any(|raw| raw_local_name(raw) == b"buBlip")
        })
    }

    /// Replaces the direct bullet.
    ///
    /// Each bullet group holds one member, so a colour, size, font, or choice
    /// set here removes its preserved `a:buClrTx`, `a:buSzTx`, `a:buFontTx`,
    /// or `a:buBlip` alternative. Clearing the bullet removes all four.
    pub fn set_bullet(&mut self, bullet: Option<TextBullet>) {
        let replaces = |name: &[u8]| {
            let Some(bullet) = bullet.as_ref() else {
                return matches!(name, b"buClrTx" | b"buSzTx" | b"buFontTx" | b"buBlip");
            };
            match name {
                b"buClrTx" => bullet.color.is_some(),
                b"buSzTx" => bullet.size.is_some(),
                b"buFontTx" => bullet.font.is_some(),
                b"buBlip" => bullet.choice.is_some(),
                _ => false,
            }
        };
        self.raw_children
            .retain(|raw| !replaces(raw_local_name(raw)));
        self.bullet = bullet;
    }

    /// Serialises a complete paragraph-property fragment.
    pub fn to_xml(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new(Vec::new());
        self.write_xml(&mut writer, "a:pPr")?;
        Ok(writer.into_inner())
    }
}

fn paragraph_property_slot(name: &[u8]) -> Option<usize> {
    match name {
        b"lnSpc" => Some(1),
        b"spcBef" => Some(2),
        b"spcAft" => Some(3),
        b"buClrTx" | b"buClr" => Some(4),
        b"buSzTx" | b"buSzPct" | b"buSzPts" => Some(5),
        b"buFontTx" | b"buFont" => Some(6),
        b"buNone" | b"buAutoNum" | b"buChar" | b"buBlip" => Some(7),
        b"tabLst" => Some(8),
        b"defRPr" => Some(9),
        b"extLst" => Some(10),
        _ => None,
    }
}

/// Returns the local name of a captured raw element.
fn raw_local_name(raw: &[u8]) -> &[u8] {
    let tag = raw.strip_prefix(b"<").unwrap_or(raw);
    let end = tag
        .iter()
        .position(|byte| byte.is_ascii_whitespace() || matches!(byte, b'/' | b'>'))
        .unwrap_or(tag.len());
    local_name(&tag[..end])
}

fn is_paragraph_property_tag(name: &[u8]) -> bool {
    matches!(
        name,
        b"pPr"
            | b"defPPr"
            | b"lvl1pPr"
            | b"lvl2pPr"
            | b"lvl3pPr"
            | b"lvl4pPr"
            | b"lvl5pPr"
            | b"lvl6pPr"
            | b"lvl7pPr"
            | b"lvl8pPr"
            | b"lvl9pPr"
    )
}

/// A regular DrawingML run containing optional properties and required text.
#[allow(non_camel_case_types)]
#[derive(Clone, Debug)]
pub struct CT_RegularTextRun {
    pub properties: Option<CT_TextCharacterProperties>,
    pub text: TextValue,
    raw_children: OrderedRawChildren,
    source_root: Vec<String>,
    source_tag: String,
}
impl PartialEq for CT_RegularTextRun {
    fn eq(&self, other: &Self) -> bool {
        self.properties == other.properties
            && self.text == other.text
            && self.raw_children == other.raw_children
    }
}
impl Eq for CT_RegularTextRun {}

impl CT_RegularTextRun {
    /// Creates a regular text run without direct character formatting.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            properties: None,
            text: TextValue {
                value: escape_invalid_xml_characters(&text.into()).into_owned(),
                ..TextValue::default()
            },
            raw_children: OrderedRawChildren::default(),
            source_root: Vec::new(),
            source_tag: String::new(),
        }
    }

    /// Replaces the run text while retaining properties and unmodelled XML.
    ///
    /// A character XML 1.0 cannot carry is stored as `_xHHHH_`, as
    /// python-pptx stores it.
    pub fn set_text(&mut self, text: &str) {
        self.text.value = escape_invalid_xml_characters(text).into_owned();
    }

    /// Serializes this regular run as a standalone DrawingML element.
    pub fn to_xml(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new(Vec::new());
        self.write_xml(&mut writer)?;
        text_close_generated_child(&writer.into_inner(), &[])
    }

    /// Parses one complete `a:r` element with any prefix.
    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        Self::from_xml_with_namespaces(xml, &TextNamespaceScope::default())
    }

    pub(crate) fn from_xml_with_namespaces(
        xml: &[u8],
        inherited: &TextNamespaceScope,
    ) -> Result<Self> {
        parse_complete(
            xml,
            b"r",
            |reader, start| Self::from_element_with_namespaces(reader, start, inherited),
            |_| Err(TextError::UnexpectedElement("r".to_owned())),
        )
    }

    fn from_element_with_namespaces(
        reader: &mut Reader<&[u8]>,
        start: &BytesStart<'_>,
        inherited: &TextNamespaceScope,
    ) -> Result<Self> {
        let source_root = text_root_spellings_scoped(
            start,
            inherited,
            &capture_element(&mut reader.clone(), start)?,
        )?;
        let namespaces = text_fragment_scope(start, inherited)?;
        let mut properties = None;
        let mut text = None;
        let mut raw_children = OrderedRawChildren::default();
        let mut boundary = 0;
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element) if text_owned_name(&element, &namespaces)? == b"rPr" => {
                    if properties.is_some() {
                        return Err(duplicate("rPr"));
                    }
                    properties = Some(CT_TextCharacterProperties::from_element_with_namespaces(
                        reader,
                        &element,
                        &namespaces,
                    )?);
                    boundary = boundary.max(1);
                }
                Event::Empty(element) if text_owned_name(&element, &namespaces)? == b"rPr" => {
                    if properties.is_some() {
                        return Err(duplicate("rPr"));
                    }
                    properties = Some(CT_TextCharacterProperties::from_start_with_namespaces(
                        &element,
                        &namespaces,
                    )?);
                    boundary = boundary.max(1);
                }
                Event::Start(element) if text_owned_name(&element, &namespaces)? == b"t" => {
                    if text.is_some() {
                        return Err(duplicate("t"));
                    }
                    text = Some(TextValue::from_element_with_namespaces(
                        reader,
                        &element,
                        &namespaces,
                    )?);
                    boundary = boundary.max(2);
                }
                Event::Empty(element) if text_owned_name(&element, &namespaces)? == b"t" => {
                    if text.is_some() {
                        return Err(duplicate("t"));
                    }
                    text = Some(TextValue::from_start_with_namespaces(
                        &element,
                        &namespaces,
                    )?);
                    boundary = boundary.max(2);
                }
                Event::Start(element) => {
                    raw_children.push(boundary, capture_element(reader, &element)?)
                }
                Event::Empty(element) => {
                    raw_children.push(boundary, capture_empty_element(&element)?)
                }
                Event::End(element) if matches_local_name(element.name().as_ref(), b"r") => break,
                Event::Eof => return Err(missing_end("r")),
                event @ (Event::Comment(_)
                | Event::PI(_)
                | Event::Text(_)
                | Event::GeneralRef(_)
                | Event::CData(_)) => {
                    raw_children.push(boundary, text_capture_opaque_event(event)?);
                }
                _ => {}
            }
            buffer.clear();
        }
        Ok(Self {
            properties,
            text: text.ok_or_else(|| {
                TextError::Xml(OxmlError::MissingElement("DrawingML run text".to_owned()))
            })?,
            raw_children,
            source_root,
            source_tag: std::str::from_utf8(start.name().as_ref())
                .map_err(OxmlError::from)?
                .to_owned(),
        })
    }

    fn write_xml<W: Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        let owned_tag = text_source_tag(
            "a:r",
            &self.source_tag,
            &text_namespace_scope(&text_source_start("a:r", &self.source_root), &[])?,
        );
        let tag = owned_tag.as_str();
        write_start(writer, text_source_start(tag, &self.source_root))?;
        emit_raw(writer, self.raw_children.at(0))?;
        if let Some(properties) = &self.properties {
            {
                let mut child = Writer::new(Vec::new());
                properties.write_xml(&mut child, "a:rPr")?;
                writer
                    .get_mut()
                    .write_all(&text_close_generated_child(
                        &child.into_inner(),
                        &text_namespace_scope(&text_source_start(tag, &self.source_root), &[])?,
                    )?)
                    .map_err(OxmlError::from)?;
            }
        }
        emit_raw(writer, self.raw_children.at(1))?;
        self.text.write_xml(writer)?;
        emit_raw(writer, self.raw_children.at(2))?;
        write_end(writer, tag)
    }
}

/// A DrawingML explicit line break and its optional run properties.
#[allow(non_camel_case_types)]
#[derive(Clone, Debug, Default)]
pub struct CT_TextLineBreak {
    pub properties: Option<CT_TextCharacterProperties>,
    raw_children: OrderedRawChildren,
    source_root: Vec<String>,
    source_tag: String,
}
impl PartialEq for CT_TextLineBreak {
    fn eq(&self, other: &Self) -> bool {
        self.properties == other.properties && self.raw_children == other.raw_children
    }
}
impl Eq for CT_TextLineBreak {}

impl CT_TextLineBreak {
    /// Parses one complete `a:br` element with any prefix.
    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        Self::from_xml_with_namespaces(xml, &TextNamespaceScope::default())
    }

    pub(crate) fn from_xml_with_namespaces(
        xml: &[u8],
        inherited: &TextNamespaceScope,
    ) -> Result<Self> {
        parse_complete(
            xml,
            b"br",
            |reader, start| Self::from_element_with_namespaces(reader, start, inherited),
            |start| {
                Ok(Self {
                    source_root: text_root_spellings_scoped(
                        start,
                        inherited,
                        &capture_empty_element(start)?,
                    )?,
                    source_tag: std::str::from_utf8(start.name().as_ref())
                        .map_err(OxmlError::from)?
                        .to_owned(),
                    ..Self::default()
                })
            },
        )
    }

    fn from_element_with_namespaces(
        reader: &mut Reader<&[u8]>,
        start: &BytesStart<'_>,
        inherited: &TextNamespaceScope,
    ) -> Result<Self> {
        let namespaces = text_fragment_scope(start, inherited)?;
        let mut line_break = Self {
            source_root: text_root_spellings_scoped(
                start,
                inherited,
                &capture_element(&mut reader.clone(), start)?,
            )?,
            source_tag: std::str::from_utf8(start.name().as_ref())
                .map_err(OxmlError::from)?
                .to_owned(),
            ..Self::default()
        };
        let mut boundary = 0;
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element) if text_owned_name(&element, &namespaces)? == b"rPr" => {
                    if line_break.properties.is_some() {
                        return Err(duplicate("rPr"));
                    }
                    line_break.properties =
                        Some(CT_TextCharacterProperties::from_element_with_namespaces(
                            reader,
                            &element,
                            &namespaces,
                        )?);
                    boundary = 1;
                }
                Event::Empty(element) if text_owned_name(&element, &namespaces)? == b"rPr" => {
                    if line_break.properties.is_some() {
                        return Err(duplicate("rPr"));
                    }
                    line_break.properties =
                        Some(CT_TextCharacterProperties::from_start_with_namespaces(
                            &element,
                            &namespaces,
                        )?);
                    boundary = 1;
                }
                Event::Start(element) => line_break
                    .raw_children
                    .push(boundary, capture_element(reader, &element)?),
                Event::Empty(element) => line_break
                    .raw_children
                    .push(boundary, capture_empty_element(&element)?),
                Event::End(element) if matches_local_name(element.name().as_ref(), b"br") => {
                    return Ok(line_break);
                }
                Event::Eof => return Err(missing_end("br")),
                event @ (Event::Comment(_)
                | Event::PI(_)
                | Event::Text(_)
                | Event::GeneralRef(_)
                | Event::CData(_)) => {
                    line_break
                        .raw_children
                        .push(boundary, text_capture_opaque_event(event)?);
                }
                _ => {}
            }
            buffer.clear();
        }
    }

    fn write_xml<W: Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        let owned_tag = text_source_tag(
            "a:br",
            &self.source_tag,
            &text_namespace_scope(&text_source_start("a:br", &self.source_root), &[])?,
        );
        let tag = owned_tag.as_str();
        if self.properties.is_none() && self.raw_children.is_empty() {
            return write_empty(writer, text_source_start(tag, &self.source_root));
        }
        write_start(writer, text_source_start(tag, &self.source_root))?;
        emit_raw(writer, self.raw_children.at(0))?;
        if let Some(properties) = &self.properties {
            {
                let mut child = Writer::new(Vec::new());
                properties.write_xml(&mut child, "a:rPr")?;
                writer
                    .get_mut()
                    .write_all(&text_close_generated_child(
                        &child.into_inner(),
                        &text_namespace_scope(&text_source_start(tag, &self.source_root), &[])?,
                    )?)
                    .map_err(OxmlError::from)?;
            }
        }
        emit_raw(writer, self.raw_children.at(1))?;
        write_end(writer, tag)
    }
}

/// A DrawingML field with required GUID, optional type, properties, and text.
#[allow(non_camel_case_types)]
#[derive(Clone, Debug)]
pub struct CT_TextField {
    pub id: String,
    pub field_type: Option<String>,
    pub run_properties: Option<CT_TextCharacterProperties>,
    pub paragraph_properties: Option<CT_TextParagraphProperties>,
    pub text: Option<TextValue>,
    raw_children: OrderedRawChildren,
    source_root: Vec<String>,
    source_tag: String,
}
impl PartialEq for CT_TextField {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.field_type == other.field_type
            && self.run_properties == other.run_properties
            && self.paragraph_properties == other.paragraph_properties
            && self.text == other.text
            && self.raw_children == other.raw_children
    }
}
impl Eq for CT_TextField {}

impl CT_TextField {
    /// Parses one complete `a:fld` element with any prefix.
    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        Self::from_xml_with_namespaces(xml, &TextNamespaceScope::default())
    }

    pub(crate) fn from_xml_with_namespaces(
        xml: &[u8],
        inherited: &TextNamespaceScope,
    ) -> Result<Self> {
        parse_complete(
            xml,
            b"fld",
            |reader, start| Self::from_element_with_namespaces(reader, start, inherited),
            |start| {
                let mut field = Self::from_empty(start)?;
                field.source_root = text_field_root_spellings(start, inherited)?;
                Ok(field)
            },
        )
    }

    fn from_start(start: &BytesStart<'_>) -> Result<Self> {
        let id = required_attr(start, b"id")?;
        if !is_guid(&id) {
            return Err(invalid_attribute("fld", "id", &id));
        }
        Ok(Self {
            id,
            field_type: text_attr(start, b"type")?,
            run_properties: None,
            paragraph_properties: None,
            text: None,
            raw_children: OrderedRawChildren::default(),
            source_root: text_attribute_tokens(start)?
                .into_iter()
                .filter(|(key, _)| key != "id" && key != "type")
                .map(|(_, token)| token)
                .collect(),
            source_tag: std::str::from_utf8(start.name().as_ref())
                .map_err(OxmlError::from)?
                .to_owned(),
        })
    }

    fn from_empty(start: &BytesStart<'_>) -> Result<Self> {
        Self::from_start(start)
    }

    fn from_element_with_namespaces(
        reader: &mut Reader<&[u8]>,
        start: &BytesStart<'_>,
        inherited: &TextNamespaceScope,
    ) -> Result<Self> {
        let namespaces = text_fragment_scope(start, inherited)?;
        let mut field = Self::from_start(start)?;
        field.source_root = text_field_root_spellings(start, inherited)?;
        let mut boundary = 0;
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element) if text_owned_name(&element, &namespaces)? == b"rPr" => {
                    if field.run_properties.is_some() {
                        return Err(duplicate("rPr"));
                    }
                    field.run_properties =
                        Some(CT_TextCharacterProperties::from_element_with_namespaces(
                            reader,
                            &element,
                            &namespaces,
                        )?);
                    boundary = boundary.max(1);
                }
                Event::Empty(element) if text_owned_name(&element, &namespaces)? == b"rPr" => {
                    if field.run_properties.is_some() {
                        return Err(duplicate("rPr"));
                    }
                    field.run_properties =
                        Some(CT_TextCharacterProperties::from_start_with_namespaces(
                            &element,
                            &namespaces,
                        )?);
                    boundary = boundary.max(1);
                }
                Event::Start(element) if text_owned_name(&element, &namespaces)? == b"pPr" => {
                    if field.paragraph_properties.is_some() {
                        return Err(duplicate("pPr"));
                    }
                    field.paragraph_properties =
                        Some(CT_TextParagraphProperties::from_element_with_namespaces(
                            reader,
                            &element,
                            &namespaces,
                        )?);
                    boundary = boundary.max(2);
                }
                Event::Empty(element) if text_owned_name(&element, &namespaces)? == b"pPr" => {
                    if field.paragraph_properties.is_some() {
                        return Err(duplicate("pPr"));
                    }
                    field.paragraph_properties =
                        Some(CT_TextParagraphProperties::from_start_with_namespaces(
                            &element,
                            &namespaces,
                        )?);
                    boundary = boundary.max(2);
                }
                Event::Start(element) if text_owned_name(&element, &namespaces)? == b"t" => {
                    if field.text.is_some() {
                        return Err(duplicate("t"));
                    }
                    field.text = Some(TextValue::from_element_with_namespaces(
                        reader,
                        &element,
                        &namespaces,
                    )?);
                    boundary = boundary.max(3);
                }
                Event::Empty(element) if text_owned_name(&element, &namespaces)? == b"t" => {
                    if field.text.is_some() {
                        return Err(duplicate("t"));
                    }
                    field.text = Some(TextValue::from_start_with_namespaces(
                        &element,
                        &namespaces,
                    )?);
                    boundary = boundary.max(3);
                }
                Event::Start(element) => field
                    .raw_children
                    .push(boundary, capture_element(reader, &element)?),
                Event::Empty(element) => field
                    .raw_children
                    .push(boundary, capture_empty_element(&element)?),
                Event::End(element) if matches_local_name(element.name().as_ref(), b"fld") => {
                    return Ok(field);
                }
                Event::Eof => return Err(missing_end("fld")),
                event @ (Event::Comment(_)
                | Event::PI(_)
                | Event::Text(_)
                | Event::GeneralRef(_)
                | Event::CData(_)) => {
                    field
                        .raw_children
                        .push(boundary, text_capture_opaque_event(event)?);
                }
                _ => {}
            }
            buffer.clear();
        }
    }

    fn write_xml<W: Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        let owned_tag = text_source_tag(
            "a:fld",
            &self.source_tag,
            &text_namespace_scope(&text_source_start("a:fld", &self.source_root), &[])?,
        );
        let tag = owned_tag.as_str();
        if !is_guid(&self.id) {
            return Err(invalid_attribute("fld", "id", &self.id));
        }
        let mut start = text_source_start(tag, &self.source_root);
        start.push_attribute(("id", self.id.as_str()));
        push_optional_attr(&mut start, "type", self.field_type.as_deref());
        let modelled = self.run_properties.is_some()
            || self.paragraph_properties.is_some()
            || self.text.is_some();
        if !modelled && self.raw_children.is_empty() {
            return write_empty(writer, start);
        }
        write_start(writer, start)?;
        emit_raw(writer, self.raw_children.at(0))?;
        if let Some(properties) = &self.run_properties {
            {
                let mut child = Writer::new(Vec::new());
                properties.write_xml(&mut child, "a:rPr")?;
                writer
                    .get_mut()
                    .write_all(&text_close_generated_child(
                        &child.into_inner(),
                        &text_namespace_scope(&text_source_start(tag, &self.source_root), &[])?,
                    )?)
                    .map_err(OxmlError::from)?;
            }
        }
        emit_raw(writer, self.raw_children.at(1))?;
        if let Some(properties) = &self.paragraph_properties {
            {
                let mut child = Writer::new(Vec::new());
                properties.write_xml(&mut child, "a:pPr")?;
                writer
                    .get_mut()
                    .write_all(&text_close_generated_child(
                        &child.into_inner(),
                        &text_namespace_scope(&text_source_start(tag, &self.source_root), &[])?,
                    )?)
                    .map_err(OxmlError::from)?;
            }
        }
        emit_raw(writer, self.raw_children.at(2))?;
        if let Some(text) = &self.text {
            text.write_xml(writer)?;
        }
        emit_raw(writer, self.raw_children.at(3))?;
        write_end(writer, tag)
    }
}

/// One member of the paragraph text-content choice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TextRun {
    Run(CT_RegularTextRun),
    Break(CT_TextLineBreak),
    Field(Box<CT_TextField>),
}

impl TextRun {
    fn from_xml_with_namespaces(
        xml: &[u8],
        name: &[u8],
        namespaces: &TextNamespaceScope,
    ) -> Result<Self> {
        match name {
            b"r" => Ok(Self::Run(CT_RegularTextRun::from_xml_with_namespaces(
                xml, namespaces,
            )?)),
            b"br" => Ok(Self::Break(CT_TextLineBreak::from_xml_with_namespaces(
                xml, namespaces,
            )?)),
            b"fld" => Ok(Self::Field(Box::new(
                CT_TextField::from_xml_with_namespaces(xml, namespaces)?,
            ))),
            _ => Err(TextError::UnexpectedElement(
                String::from_utf8_lossy(name).into_owned(),
            )),
        }
    }

    fn write_xml<W: Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        match self {
            Self::Run(run) => run.write_xml(writer),
            Self::Break(line_break) => line_break.write_xml(writer),
            Self::Field(field) => field.write_xml(writer),
        }
    }
}

/// A DrawingML paragraph with ordered runs, breaks, and fields.
#[allow(non_camel_case_types)]
#[derive(Clone, Debug, Default)]
pub struct CT_TextParagraph {
    pub properties: Option<CT_TextParagraphProperties>,
    pub runs: Vec<TextRun>,
    pub end_properties: Option<CT_TextCharacterProperties>,
    raw_children: OrderedRawChildren,
    source_root: Vec<String>,
    source_tag: String,
}
impl PartialEq for CT_TextParagraph {
    fn eq(&self, other: &Self) -> bool {
        self.properties == other.properties
            && self.runs == other.runs
            && self.end_properties == other.end_properties
            && self.raw_children == other.raw_children
    }
}
impl Eq for CT_TextParagraph {}

impl CT_TextParagraph {
    /// Returns direct properties, inserting them before preserved content.
    pub fn properties_mut(&mut self) -> &mut CT_TextParagraphProperties {
        if self.properties.is_none() {
            let mut raw_children = OrderedRawChildren::default();
            for boundary in 0..=2 + self.runs.len() {
                let new_boundary = if boundary == 0 { 1 } else { boundary };
                for child in self.raw_children.at(boundary) {
                    raw_children.push(new_boundary, child.to_vec());
                }
            }
            self.raw_children = raw_children;
            self.properties = Some(CT_TextParagraphProperties::default());
        }
        self.properties
            .as_mut()
            .expect("paragraph properties were inserted")
    }

    /// Replaces ordered text choices with one regular run, split at each
    /// vertical tab by an `a:br` with the run's formatting, since a vertical
    /// tab is how the paragraph text reports a line break.
    ///
    /// The first existing regular run supplies direct formatting and
    /// unmodelled run content for the replacement. A paragraph without one
    /// formats the new run with its end-of-paragraph properties, without
    /// their hyperlinks, as PowerPoint formats text typed into an empty
    /// paragraph. Empty text leaves the paragraph properties and drops every
    /// run, including run-local preserved children.
    pub fn set_text(&mut self, text: &str) {
        let old_run_count = self.runs.len();
        if text.is_empty() {
            let mut raw_children = OrderedRawChildren::default();
            for boundary in 0..=2 + old_run_count {
                if boundary <= 1 {
                    for child in self.raw_children.at(boundary) {
                        raw_children.push(boundary, child.to_vec());
                    }
                } else if boundary > 1 + old_run_count {
                    for child in self.raw_children.at(boundary) {
                        raw_children.push(2, child.to_vec());
                    }
                }
            }
            self.raw_children = raw_children;
            self.runs.clear();
            return;
        }
        let retained_run = self.runs.iter().find_map(|run| match run {
            TextRun::Run(run) => Some(run.clone()),
            TextRun::Break(_) | TextRun::Field(_) => None,
        });
        let mut raw_children = OrderedRawChildren::default();
        for boundary in 0..=2 + old_run_count {
            let new_boundary = if boundary <= 1 {
                boundary
            } else if boundary <= 1 + old_run_count {
                2
            } else {
                3
            };
            for child in self.raw_children.at(boundary) {
                raw_children.push(new_boundary, child.to_vec());
            }
        }
        self.raw_children = raw_children;
        let mut run = retained_run.unwrap_or_else(|| CT_RegularTextRun {
            properties: self
                .end_properties
                .clone()
                .map(CT_TextCharacterProperties::without_hyperlinks),
            ..CT_RegularTextRun::new(text)
        });
        run.set_text(text);
        self.runs = vec![TextRun::Run(run)];
        self.split_line_breaks(text);
    }

    /// Splits the one run `set_text` leaves at each vertical tab of `text`.
    /// A later run that would be empty is left out, as python-pptx leaves it
    /// out, and content preserved after the old runs stays after the new ones.
    fn split_line_breaks(&mut self, text: &str) {
        let [TextRun::Run(first)] = self.runs.as_mut_slice() else {
            return;
        };
        if !text.contains('\u{b}') {
            return;
        }
        let mut lines = text.split('\u{b}');
        first.set_text(lines.next().unwrap_or_default());
        let properties = first.properties.clone();
        for line in lines {
            self.raw_children.shift_boundaries_from(2);
            self.runs.push(TextRun::Break(CT_TextLineBreak {
                properties: properties.clone(),
                ..CT_TextLineBreak::default()
            }));
            if !line.is_empty() {
                self.raw_children.shift_boundaries_from(2);
                self.runs.push(TextRun::Run(CT_RegularTextRun {
                    properties: properties.clone(),
                    ..CT_RegularTextRun::new(line)
                }));
            }
        }
    }

    /// Appends one regular run after the existing ordered text choices.
    pub fn add_run(&mut self, text: &str) -> &mut CT_RegularTextRun {
        self.raw_children.shift_boundaries_from(2 + self.runs.len());
        self.runs.push(TextRun::Run(CT_RegularTextRun::new(text)));
        let Some(TextRun::Run(run)) = self.runs.last_mut() else {
            unreachable!("the appended text choice is a regular run")
        };
        run
    }

    /// Parses one complete `a:p` element with any prefix.
    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        Self::from_xml_with_namespaces(xml, &TextNamespaceScope::default())
    }

    pub(crate) fn from_xml_with_namespaces(
        xml: &[u8],
        inherited: &TextNamespaceScope,
    ) -> Result<Self> {
        parse_complete(
            xml,
            b"p",
            |reader, start| Self::from_element_with_namespaces(reader, start, inherited),
            |start| {
                Ok(Self {
                    source_root: text_root_spellings_scoped(
                        start,
                        inherited,
                        &capture_empty_element(start)?,
                    )?,
                    source_tag: std::str::from_utf8(start.name().as_ref())
                        .map_err(OxmlError::from)?
                        .to_owned(),
                    ..Self::default()
                })
            },
        )
    }

    fn from_element_with_namespaces(
        reader: &mut Reader<&[u8]>,
        start: &BytesStart<'_>,
        inherited: &TextNamespaceScope,
    ) -> Result<Self> {
        let namespaces = text_fragment_scope(start, inherited)?;
        let mut paragraph = Self {
            source_root: text_root_spellings_scoped(
                start,
                inherited,
                &capture_element(&mut reader.clone(), start)?,
            )?,
            source_tag: std::str::from_utf8(start.name().as_ref())
                .map_err(OxmlError::from)?
                .to_owned(),
            ..Self::default()
        };
        let mut boundary = 0;
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                // The schema allows one leading pPr, but real decks carry a
                // later one between runs. The first governs the paragraph and
                // every later one falls through to verbatim preservation at
                // its position, which is also how python-pptx treats it.
                Event::Start(element)
                    if text_owned_name(&element, &namespaces)? == b"pPr"
                        && paragraph.properties.is_none() =>
                {
                    paragraph.properties =
                        Some(CT_TextParagraphProperties::from_element_with_namespaces(
                            reader,
                            &element,
                            &namespaces,
                        )?);
                    boundary = boundary.max(1);
                }
                Event::Empty(element)
                    if text_owned_name(&element, &namespaces)? == b"pPr"
                        && paragraph.properties.is_none() =>
                {
                    paragraph.properties =
                        Some(CT_TextParagraphProperties::from_start_with_namespaces(
                            &element,
                            &namespaces,
                        )?);
                    boundary = boundary.max(1);
                }
                Event::Start(element) if is_text_run(&text_owned_name(&element, &namespaces)?) => {
                    let name = local_name(element.name().as_ref()).to_vec();
                    let raw = capture_element(reader, &element)?;
                    paragraph.runs.push(TextRun::from_xml_with_namespaces(
                        &raw,
                        &name,
                        &namespaces,
                    )?);
                    boundary = boundary.max(1 + paragraph.runs.len());
                }
                Event::Empty(element) if is_text_run(&text_owned_name(&element, &namespaces)?) => {
                    let name = local_name(element.name().as_ref()).to_vec();
                    let raw = capture_empty_element(&element)?;
                    paragraph.runs.push(TextRun::from_xml_with_namespaces(
                        &raw,
                        &name,
                        &namespaces,
                    )?);
                    boundary = boundary.max(1 + paragraph.runs.len());
                }
                Event::Start(element)
                    if text_owned_name(&element, &namespaces)? == b"endParaRPr" =>
                {
                    if paragraph.end_properties.is_some() {
                        return Err(duplicate("endParaRPr"));
                    }
                    paragraph.end_properties =
                        Some(CT_TextCharacterProperties::from_element_with_namespaces(
                            reader,
                            &element,
                            &namespaces,
                        )?);
                    boundary = boundary.max(2 + paragraph.runs.len());
                }
                Event::Empty(element)
                    if text_owned_name(&element, &namespaces)? == b"endParaRPr" =>
                {
                    if paragraph.end_properties.is_some() {
                        return Err(duplicate("endParaRPr"));
                    }
                    paragraph.end_properties =
                        Some(CT_TextCharacterProperties::from_start_with_namespaces(
                            &element,
                            &namespaces,
                        )?);
                    boundary = boundary.max(2 + paragraph.runs.len());
                }
                Event::Start(element) => paragraph
                    .raw_children
                    .push(boundary, capture_element(reader, &element)?),
                Event::Empty(element) => paragraph
                    .raw_children
                    .push(boundary, capture_empty_element(&element)?),
                Event::End(element) if matches_local_name(element.name().as_ref(), b"p") => {
                    return Ok(paragraph);
                }
                Event::Eof => return Err(missing_end("p")),
                event @ (Event::Comment(_)
                | Event::PI(_)
                | Event::Text(_)
                | Event::GeneralRef(_)
                | Event::CData(_)) => {
                    paragraph
                        .raw_children
                        .push(boundary, text_capture_opaque_event(event)?);
                }
                _ => {}
            }
            buffer.clear();
        }
    }

    /// Writes a paragraph with fixed prefixes and schema child order.
    pub fn write_xml<W: Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        let owned_tag = text_source_tag(
            "a:p",
            &self.source_tag,
            &text_namespace_scope(&text_source_start("a:p", &self.source_root), &[])?,
        );
        let tag = owned_tag.as_str();
        if self.properties.is_none()
            && self.runs.is_empty()
            && self.end_properties.is_none()
            && self.raw_children.is_empty()
        {
            return write_empty(writer, text_source_start(tag, &self.source_root));
        }
        write_start(writer, text_source_start(tag, &self.source_root))?;
        emit_raw(writer, self.raw_children.at(0))?;
        if let Some(properties) = &self.properties {
            {
                let mut child = Writer::new(Vec::new());
                properties.write_xml(&mut child, "a:pPr")?;
                writer
                    .get_mut()
                    .write_all(&text_close_generated_child(
                        &child.into_inner(),
                        &text_namespace_scope(&text_source_start(tag, &self.source_root), &[])?,
                    )?)
                    .map_err(OxmlError::from)?;
            }
        }
        emit_raw(writer, self.raw_children.at(1))?;
        for (index, run) in self.runs.iter().enumerate() {
            run.write_xml(writer)?;
            emit_raw(writer, self.raw_children.at(2 + index))?;
        }
        if let Some(properties) = &self.end_properties {
            {
                let mut child = Writer::new(Vec::new());
                properties.write_xml(&mut child, "a:endParaRPr")?;
                writer
                    .get_mut()
                    .write_all(&text_close_generated_child(
                        &child.into_inner(),
                        &text_namespace_scope(&text_source_start(tag, &self.source_root), &[])?,
                    )?)
                    .map_err(OxmlError::from)?;
            }
        }
        emit_raw(writer, self.raw_children.at(2 + self.runs.len()))?;
        write_end(writer, tag)
    }

    /// Serialises a complete paragraph fragment.
    pub fn to_xml(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new(Vec::new());
        self.write_xml(&mut writer)?;
        Ok(writer.into_inner())
    }

    pub fn raw_children(&self) -> &OrderedRawChildren {
        &self.raw_children
    }
}

fn is_text_run(name: &[u8]) -> bool {
    matches!(local_name(name), b"r" | b"br" | b"fld")
}

fn parse_complete<T>(
    xml: &[u8],
    expected: &[u8],
    parse_start: impl FnOnce(&mut Reader<&[u8]>, &BytesStart<'_>) -> Result<T>,
    parse_empty: impl FnOnce(&BytesStart<'_>) -> Result<T>,
) -> Result<T> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) if matches_local_name(element.name().as_ref(), expected) => {
                text_validate_root(&element)?;
                let parsed = parse_start(&mut reader, &element)?;
                text_reject_trailing(&mut reader)?;
                return Ok(parsed);
            }
            Event::Empty(element) if matches_local_name(element.name().as_ref(), expected) => {
                text_validate_root(&element)?;
                let parsed = parse_empty(&element)?;
                text_reject_trailing(&mut reader)?;
                return Ok(parsed);
            }
            Event::Start(element) | Event::Empty(element) => return Err(unexpected(&element)),
            Event::Eof => {
                return Err(TextError::Xml(OxmlError::MissingElement(
                    String::from_utf8_lossy(expected).into_owned(),
                )));
            }
            _ => {}
        }
        buffer.clear();
    }
}

pub(crate) fn text_reject_trailing(reader: &mut Reader<&[u8]>) -> Result<()> {
    loop {
        match reader.read_event().map_err(OxmlError::from)? {
            Event::Eof => return Ok(()),
            Event::Comment(_) | Event::PI(_) => {}
            Event::Text(text) if text.iter().all(u8::is_ascii_whitespace) => {}
            _ => {
                return Err(TextError::UnexpectedElement(
                    "trailing XML content".to_owned(),
                ));
            }
        }
    }
}

fn root_local_name(xml: &[u8]) -> Result<Vec<u8>> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) | Event::Empty(element) => {
                return Ok(local_name(element.name().as_ref()).to_vec());
            }
            Event::Eof => {
                return Err(TextError::Xml(OxmlError::MissingElement(
                    "DrawingML text properties".to_owned(),
                )));
            }
            _ => {}
        }
        buffer.clear();
    }
}

fn ensure_empty(reader: &mut Reader<&[u8]>, expected: &[u8]) -> Result<()> {
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::End(element) if matches_local_name(element.name().as_ref(), expected) => {
                return Ok(());
            }
            Event::Text(text) if text.iter().all(u8::is_ascii_whitespace) => {}
            Event::Comment(_) => {}
            Event::Start(element) | Event::Empty(element) => return Err(unexpected(&element)),
            Event::Eof => return Err(missing_end(&String::from_utf8_lossy(expected))),
            _ => {
                return Err(TextError::UnexpectedElement(
                    String::from_utf8_lossy(expected).into_owned(),
                ));
            }
        }
        buffer.clear();
    }
}

fn text_attr(start: &BytesStart<'_>, name: &[u8]) -> Result<Option<String>> {
    for attribute in start.attributes() {
        let attribute = attribute.map_err(OxmlError::from)?;
        if attribute.key.as_ref() == name {
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
                .map_err(OxmlError::from)?;
            return Ok(Some(value.into_owned()));
        }
    }
    Ok(None)
}

fn capture_raw_attributes(
    start: &BytesStart<'_>,
    modelled: &[&[u8]],
) -> Result<Vec<(String, String)>> {
    let mut raw = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(OxmlError::from)?;
        if modelled.iter().any(|name| attribute.key.as_ref() == *name) {
            continue;
        }
        let name = std::str::from_utf8(attribute.key.as_ref())
            .map_err(OxmlError::from)?
            .to_owned();
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
            .map_err(OxmlError::from)?
            .into_owned();
        raw.push((name, value));
    }
    Ok(raw)
}

fn required_attr(start: &BytesStart<'_>, name: &[u8]) -> Result<String> {
    text_attr(start, name)?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            missing_attribute(
                &String::from_utf8_lossy(local_name(start.name().as_ref())),
                &String::from_utf8_lossy(name),
            )
        })
}

fn parse_optional_i32(
    start: &BytesStart<'_>,
    name: &[u8],
    min: i32,
    max: i32,
) -> Result<Option<i32>> {
    text_attr(start, name)?
        .map(|value| {
            parse_i32_value(
                &String::from_utf8_lossy(local_name(start.name().as_ref())),
                &String::from_utf8_lossy(name),
                &value,
                min,
                max,
            )
        })
        .transpose()
}

fn parse_i32_attr(start: &BytesStart<'_>, name: &[u8], min: i32, max: i32) -> Result<i32> {
    let value = required_attr(start, name)?;
    parse_i32_value(
        &String::from_utf8_lossy(local_name(start.name().as_ref())),
        &String::from_utf8_lossy(name),
        &value,
        min,
        max,
    )
}

fn parse_i32_value(element: &str, attribute: &str, value: &str, min: i32, max: i32) -> Result<i32> {
    let parsed = value
        .parse::<i32>()
        .map_err(|_| invalid_attribute(element, attribute, value))?;
    if (min..=max).contains(&parsed) {
        Ok(parsed)
    } else {
        Err(invalid_attribute(element, attribute, value))
    }
}

fn parse_optional_bool(start: &BytesStart<'_>, name: &[u8]) -> Result<Option<bool>> {
    text_attr(start, name)?
        .map(|value| match value.as_str() {
            "true" | "1" => Ok(true),
            "false" | "0" => Ok(false),
            _ => Err(invalid_attribute(
                &String::from_utf8_lossy(local_name(start.name().as_ref())),
                &String::from_utf8_lossy(name),
                &value,
            )),
        })
        .transpose()
}

fn parse_optional_enum<T>(
    start: &BytesStart<'_>,
    name: &[u8],
    parse: impl FnOnce(&str) -> Option<T>,
) -> Result<Option<T>> {
    let Some(value) = text_attr(start, name)? else {
        return Ok(None);
    };
    parse(&value).map(Some).ok_or_else(|| {
        invalid_attribute(
            &String::from_utf8_lossy(local_name(start.name().as_ref())),
            &String::from_utf8_lossy(name),
            &value,
        )
    })
}

fn validate_spacing_percent(value: &str) -> Result<()> {
    if is_percentage_string(value) {
        return Ok(());
    }
    parse_i32_value("spcPct", "val", value, 0, MAX_TRANSITIONAL_SPACING_PERCENT)?;
    Ok(())
}

fn validate_baseline(value: &str) -> Result<()> {
    if is_percentage_string(value) {
        return Ok(());
    }
    parse_i32_value("rPr", "baseline", value, -100_000, 100_000)?;
    Ok(())
}

fn is_percentage_string(value: &str) -> bool {
    value.strip_suffix('%').is_some_and(is_signed_decimal)
}

fn is_universal_measure(value: &str) -> bool {
    ["mm", "cm", "in", "pt", "pc", "pi"]
        .iter()
        .find_map(|unit| value.strip_suffix(unit))
        .is_some_and(is_signed_decimal)
}

fn is_signed_decimal(value: &str) -> bool {
    let unsigned = value.strip_prefix('-').unwrap_or(value);
    let mut parts = unsigned.split('.');
    let Some(integer) = parts.next() else {
        return false;
    };
    if integer.is_empty() || !integer.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    if let Some(fraction) = parts.next()
        && (fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return false;
    }
    parts.next().is_none()
}

fn is_guid(value: &str) -> bool {
    if value.len() != 38 || !value.starts_with('{') || !value.ends_with('}') {
        return false;
    }
    value.bytes().enumerate().all(|(index, byte)| match index {
        0 => byte == b'{',
        37 => byte == b'}',
        9 | 14 | 19 | 24 => byte == b'-',
        _ => byte.is_ascii_hexdigit(),
    })
}

fn push_optional_attr<'a>(start: &mut BytesStart<'a>, name: &'a str, value: Option<&'a str>) {
    if let Some(value) = value {
        start.push_attribute((name, value));
    }
}

fn push_optional_bool(start: &mut BytesStart<'_>, name: &'static str, value: Option<bool>) {
    if let Some(value) = value {
        start.push_attribute((name, if value { "1" } else { "0" }));
    }
}

fn push_owned_attr(start: &mut BytesStart<'_>, name: &'static str, value: String) {
    start.push_attribute((name, value.as_str()));
}

fn write_start<W: Write>(writer: &mut Writer<W>, start: BytesStart<'_>) -> Result<()> {
    writer
        .write_event(Event::Start(start))
        .map_err(OxmlError::from)?;
    Ok(())
}

fn write_empty<W: Write>(writer: &mut Writer<W>, start: BytesStart<'_>) -> Result<()> {
    writer
        .write_event(Event::Empty(start))
        .map_err(OxmlError::from)?;
    Ok(())
}

fn write_end<W: Write>(writer: &mut Writer<W>, tag: &str) -> Result<()> {
    writer
        .write_event(Event::End(BytesEnd::new(tag)))
        .map_err(OxmlError::from)?;
    Ok(())
}

fn unexpected(element: &BytesStart<'_>) -> TextError {
    TextError::UnexpectedElement(String::from_utf8_lossy(element.name().as_ref()).into_owned())
}

fn duplicate(element: &str) -> TextError {
    TextError::DuplicateElement(element.to_owned())
}

fn missing_attribute(element: &str, attribute: &str) -> TextError {
    TextError::MissingAttribute {
        element: element.to_owned(),
        attribute: attribute.to_owned(),
    }
}

fn invalid_attribute(element: &str, attribute: &str, value: &str) -> TextError {
    TextError::InvalidAttribute {
        element: element.to_owned(),
        attribute: attribute.to_owned(),
        value: value.to_owned(),
    }
}

/// Close an inherited property at its original owner scope without guessing
/// which opaque attribute or text values might contain producer QNames.
fn text_close_inherited_property(raw: &[u8], inherited: &[(String, String)]) -> Result<Vec<u8>> {
    let mut reader = Reader::from_reader(raw);
    let event = reader.read_event().map_err(OxmlError::from)?;
    let empty = matches!(&event, Event::Empty(_));
    let start = match event {
        Event::Start(start) | Event::Empty(start) => start,
        _ => return Ok(raw.to_vec()),
    };
    let mut content = std::str::from_utf8(start.as_ref())
        .map_err(OxmlError::from)?
        .to_owned();
    let declared = start
        .attributes()
        .map(|attribute| attribute.map(|attribute| attribute.key.as_ref().to_vec()))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(OxmlError::from)?;
    for (prefix, uri) in inherited {
        let key = if prefix.is_empty() {
            "xmlns".to_owned()
        } else {
            format!("xmlns:{prefix}")
        };
        if !declared.iter().any(|old| old == key.as_bytes()) {
            content.push_str(&format!(" {key}=\"{}\"", quick_xml::escape::escape(uri)));
        }
    }
    if !inherited.iter().any(|(prefix, _)| prefix.is_empty())
        && !declared.iter().any(|key| key == b"xmlns")
    {
        content.push_str(" xmlns=\"\"");
    }
    let start = BytesStart::from_content(content, start.name().as_ref().len());
    let mut writer = Writer::new(Vec::new());
    if empty {
        write_empty(&mut writer, start)?;
    } else {
        write_start(&mut writer, start)?;
    }
    let mut closed = writer.into_inner();
    closed.extend_from_slice(&raw[reader.buffer_position() as usize..]);
    Ok(closed)
}

/// Combine only the known DrawingML inheritance groups. Every inherited opaque
/// child carries its original namespace closure, and local groups stay dominant.
pub(crate) fn text_inherit_property_xml(
    local: &[u8],
    local_scope: &[(String, String)],
    inherited: &[u8],
    inherited_scope: &[(String, String)],
) -> Result<Vec<u8>> {
    let read = |xml: &[u8], scope: &[(String, String)]| -> Result<_> {
        let mut reader = Reader::from_reader(xml);
        let start = match reader.read_event().map_err(OxmlError::from)? {
            Event::Start(start) | Event::Empty(start) => start.into_owned(),
            _ => {
                return Err(invalid_attribute(
                    "inherit text properties",
                    "root",
                    "missing owner",
                ));
            }
        };
        let empty = xml[reader.buffer_position() as usize - 2] == b'/';
        let scope = text_fragment_scope(&start, &TextNamespaceScope::inherited(scope))?;
        let root_name = local_name(start.name().as_ref()).to_vec();
        let mut children = Vec::new();
        let mut boundary = 0;
        if !empty {
            loop {
                let event = reader.read_event().map_err(OxmlError::from)?;
                let raw = match event {
                    Event::Start(child) | Event::Empty(child) => {
                        let child_empty = xml[reader.buffer_position() as usize - 2] == b'/';
                        let name = text_owned_name(&child, &scope)?;
                        let slot = if is_character_property_tag(&root_name) {
                            character_property_slot(&name)
                        } else if is_paragraph_property_tag(&root_name) {
                            paragraph_property_slot(&name)
                        } else if name == b"defPPr" {
                            Some(1)
                        } else if root_name != b"extLst" && name == b"extLst" {
                            Some(11)
                        } else if root_name != b"extLst" {
                            name.strip_prefix(b"lvl")
                                .and_then(|tail| tail.strip_suffix(b"pPr"))
                                .and_then(|level| {
                                    std::str::from_utf8(level).ok()?.parse::<usize>().ok()
                                })
                                .filter(|level| (1..=9).contains(level))
                                .map(|level| level + 1)
                        } else {
                            None
                        };
                        if let Some(slot) = slot {
                            boundary = boundary.max(slot);
                        }
                        let raw = if child_empty {
                            capture_empty_element(&child)?
                        } else {
                            capture_element(&mut reader, &child)?
                        };
                        children.push((boundary, slot, raw));
                        continue;
                    }
                    Event::End(_) => break,
                    Event::Eof => return Err(missing_end("inherited property owner")),
                    event @ (Event::Comment(_)
                    | Event::PI(_)
                    | Event::Text(_)
                    | Event::GeneralRef(_)
                    | Event::CData(_)) => text_capture_opaque_event(event)?,
                    _ => continue,
                };
                children.push((boundary, None, raw));
            }
        }
        Ok((start, scope, children))
    };
    let (mut start, local_scope, mut children) = read(local, local_scope)?;
    let (parent, inherited_scope, inherited_children) = read(inherited, inherited_scope)?;
    let root_name = local_name(start.name().as_ref()).to_vec();
    let attribute_identity = |key: &[u8], scope: &TextNamespaceScope| -> Result<_> {
        let key = std::str::from_utf8(key).map_err(OxmlError::from)?;
        let (prefix, name) = key.split_once(':').unwrap_or(("", key));
        let uri = if prefix.is_empty() {
            None
        } else if prefix == "xml" {
            Some("http://www.w3.org/XML/1998/namespace".to_owned())
        } else {
            scope
                .physical
                .iter()
                .find(|(p, _)| p == prefix)
                .map(|(_, uri)| uri.clone())
        };
        if !prefix.is_empty() && uri.is_none() {
            return Err(invalid_attribute(
                "inherit text properties",
                "namespace",
                "unbound attribute prefix",
            ));
        }
        Ok((uri, name.to_owned()))
    };
    let known_attribute = |key: &[u8]| {
        if is_character_property_tag(&root_name) {
            matches!(
                key,
                b"sz"
                    | b"b"
                    | b"i"
                    | b"cap"
                    | b"u"
                    | b"strike"
                    | b"spc"
                    | b"baseline"
                    | b"lang"
                    | b"altLang"
                    | b"kern"
                    | b"kumimoji"
                    | b"normalizeH"
                    | b"noProof"
                    | b"dirty"
                    | b"err"
                    | b"smtClean"
                    | b"smtId"
                    | b"bmk"
            )
        } else if is_paragraph_property_tag(&root_name) {
            matches!(
                key,
                b"marL"
                    | b"marR"
                    | b"lvl"
                    | b"indent"
                    | b"algn"
                    | b"rtl"
                    | b"defTabSz"
                    | b"eaLnBrk"
                    | b"latinLnBrk"
                    | b"hangingPunct"
                    | b"fontAlgn"
            )
        } else {
            false
        }
    };
    let namespaces = |scope: &TextNamespaceScope| {
        scope
            .physical
            .iter()
            .filter(|(prefix, uri)| !prefix.is_empty() || !uri.is_empty())
            .cloned()
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let local_tokens = text_attribute_tokens(&start)?;
    let parent_tokens = text_attribute_tokens(&parent)?;
    let mut additions = String::new();
    for attribute in parent.attributes() {
        let attribute = attribute.map_err(OxmlError::from)?;
        let key = attribute.key.as_ref();
        if key == b"xmlns" || key.starts_with(b"xmlns:") {
            continue;
        }
        let identity = attribute_identity(key, &inherited_scope)?;
        let mut existing = None;
        for old in start.attributes() {
            let old = old.map_err(OxmlError::from)?;
            if old.key.as_ref() != b"xmlns"
                && !old.key.as_ref().starts_with(b"xmlns:")
                && attribute_identity(old.key.as_ref(), &local_scope)? == identity
            {
                existing = Some(old);
                break;
            }
        }
        if let Some(old) = existing {
            if !known_attribute(key)
                && (old
                    .decoded_and_normalized_value(
                        quick_xml::XmlVersion::Implicit1_0,
                        start.decoder(),
                    )
                    .map_err(OxmlError::from)?
                    != attribute
                        .decoded_and_normalized_value(
                            quick_xml::XmlVersion::Implicit1_0,
                            parent.decoder(),
                        )
                        .map_err(OxmlError::from)?
                    || namespaces(&local_scope) != namespaces(&inherited_scope))
            {
                return Err(invalid_attribute(
                    "inherit text properties",
                    "metadata",
                    "conflicting opaque attribute authority",
                ));
            }
            continue;
        }
        if !known_attribute(key)
            && (!children.is_empty()
                || local_tokens
                    .iter()
                    .any(|(key, _)| !key.starts_with("xmlns") && !known_attribute(key.as_bytes())))
            && namespaces(&local_scope) != namespaces(&inherited_scope)
        {
            return Err(invalid_attribute(
                "inherit text properties",
                "metadata",
                "unrepresentable opaque namespace contexts",
            ));
        }
        if !known_attribute(key) {
            for (prefix, uri) in &inherited_scope.physical {
                if let Some((_, old)) = local_scope.physical.iter().find(|(old, _)| old == prefix) {
                    if old != uri {
                        return Err(invalid_attribute(
                            "inherit text properties",
                            "namespace",
                            "conflicting inherited root binding",
                        ));
                    }
                } else {
                    let key = if prefix.is_empty() {
                        "xmlns".to_owned()
                    } else {
                        format!("xmlns:{prefix}")
                    };
                    if !additions.contains(&format!(" {key}=")) {
                        additions
                            .push_str(&format!(" {key}=\"{}\"", quick_xml::escape::escape(uri)));
                    }
                }
            }
        }
        let (_, token) = parent_tokens
            .iter()
            .find(|(old, _)| old.as_bytes() == key)
            .ok_or_else(|| {
                invalid_attribute(
                    "inherit text properties",
                    "attribute",
                    "missing source token",
                )
            })?;
        additions.push_str(token);
    }
    for (boundary, slot, raw) in inherited_children {
        if let Some(slot) = slot
            && let Some(index) = children.iter().position(|(_, old, _)| *old == Some(slot))
        {
            let name = raw_local_name(&raw);
            if matches!(name, b"defRPr" | b"defPPr" | b"extLst") || name.starts_with(b"lvl") {
                children[index].2 = text_inherit_property_xml(
                    &children[index].2,
                    &local_scope.physical,
                    &raw,
                    &inherited_scope.physical,
                )?;
            }
            continue;
        }
        let raw = text_close_inherited_property(&raw, &inherited_scope.physical)?;
        if children.iter().any(|(_, _, old)| old == &raw) {
            continue;
        }
        let position = children
            .iter()
            .position(|(old_boundary, _, _)| *old_boundary > boundary)
            .unwrap_or(children.len());
        children.insert(position, (boundary, slot, raw));
    }
    if !additions.is_empty() {
        let name_len = start.name().as_ref().len();
        let content = format!(
            "{}{additions}",
            std::str::from_utf8(start.as_ref()).map_err(OxmlError::from)?
        );
        start = BytesStart::from_content(content, name_len);
    }
    let name = std::str::from_utf8(start.name().as_ref())
        .map_err(OxmlError::from)?
        .to_owned();
    let mut writer = Writer::new(Vec::new());
    if children.is_empty() {
        write_empty(&mut writer, start)?;
    } else {
        write_start(&mut writer, start)?;
        for (_, _, raw) in children {
            writer.get_mut().write_all(&raw).map_err(OxmlError::from)?;
        }
        write_end(&mut writer, &name)?;
    }
    Ok(writer.into_inner())
}
