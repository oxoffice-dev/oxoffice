//! WordprocessingML glossary document parts and building-block entries.

use oxoffice_core::xml::{XmlLexicalError, validate_strict_xml_1_0};
use oxoffice_core::xml_element::start_tag;
use quick_xml::events::{BytesEnd, BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::reader::NsReader;
use quick_xml::{Reader, Writer};
use std::collections::BTreeMap;
use std::ops::Range;

use crate::document::CT_Body;
use crate::error::{OxmlError, Result};
use crate::namespace::W_NS;
use crate::numbering::word_prefixes_at;
use crate::properties::{is_word_attribute, is_word_element};
use crate::raw_xml::{capture_element, capture_empty_element};

/// Bytes and invocation-only companions follow the same existing glossary edit
/// recipe. Legacy callers keep no maps or events.
pub(crate) struct GlossaryXml {
    pub(crate) bytes: Vec<u8>,
    pub(crate) source_map: Option<crate::document::WordSourceMap>,
    pub(crate) events: Vec<crate::text::TemplateWordEvent>,
}

impl GlossaryXml {
    fn generated(bytes: Vec<u8>, tracked: bool) -> Self {
        Self {
            source_map: tracked.then(|| crate::document::WordSourceMap::generated(bytes.len())),
            bytes,
            events: Vec::new(),
        }
    }

    fn retained(raw: &[u8], work: Option<&mut crate::text::WordOperationWork>) -> Result<Self> {
        if let Some(work) = work {
            let (mut output, lease) = work.template_word_record_output();
            std::io::Write::write_all(&mut output, raw)?;
            let (bytes, source_map, events) = output.into_word_parts();
            drop(lease);
            Ok(Self {
                bytes,
                source_map: Some(source_map),
                events,
            })
        } else {
            Ok(Self::generated(raw.to_vec(), false))
        }
    }

    fn append(&mut self, mut other: Self) -> Result<()> {
        let offset = self.bytes.len();
        if let Some(map) = &mut self.source_map {
            map.extend(other.source_map.as_ref().expect("tracked glossary piece"))?;
            for event in &mut other.events {
                event.encoded.start = event.encoded.start.checked_add(offset).ok_or_else(|| {
                    OxmlError::InvalidValue("glossary Word event position overflow".into())
                })?;
                event.encoded.end = event.encoded.end.checked_add(offset).ok_or_else(|| {
                    OxmlError::InvalidValue("glossary Word event position overflow".into())
                })?;
            }
            self.events.extend(other.events);
        }
        self.bytes.extend(other.bytes);
        Ok(())
    }

    fn append_slice(&mut self, other: &Self, range: Range<usize>) -> Result<()> {
        let offset = self.bytes.len();
        if let Some(map) = &mut self.source_map {
            map.extend(
                &other
                    .source_map
                    .as_ref()
                    .expect("tracked glossary piece")
                    .slice(range.clone())?,
            )?;
            self.events.extend(
                other
                    .events
                    .iter()
                    .filter(|event| {
                        event.encoded.start >= range.start && event.encoded.end <= range.end
                    })
                    .cloned()
                    .map(|mut event| {
                        event.encoded.start = offset + event.encoded.start - range.start;
                        event.encoded.end = offset + event.encoded.end - range.start;
                        event
                    }),
            );
        }
        self.bytes.extend_from_slice(&other.bytes[range]);
        Ok(())
    }

    fn append_retained(
        &mut self,
        raw: &[u8],
        work: Option<&crate::text::WordOperationWork>,
    ) -> Result<()> {
        if let Some(map) = &mut self.source_map {
            map.extend(
                &work
                    .expect("tracked glossary writer")
                    .template_word_raw_map(raw)?,
            )?;
        }
        self.bytes.extend_from_slice(raw);
        Ok(())
    }
}

/// The `w:glossaryDocument` root.
#[derive(Debug, Clone)]
pub struct CT_GlossaryDocument {
    pub doc_parts: Vec<CT_DocPart>,
    raw_xml: Vec<u8>,
    source_spans: Vec<Range<usize>>,
}

/// One `w:docPart` building-block entry.
#[derive(Debug, Clone)]
pub struct CT_DocPart {
    pub properties: CT_DocPartPr,
    pub body: CT_Body,
    raw_xml: Vec<u8>,
    body_xml: Vec<u8>,
    source_span: Option<Range<usize>>,
    properties_span: Option<Range<usize>>,
    body_span: Range<usize>,
    original_properties: DocPartPrSnapshot,
    original_body: CT_Body,
    word_prefixes: Vec<String>,
    dirty_raw: bool,
    inherited_namespaces: BTreeMap<String, String>,
}

/// The supported properties of one building block.
#[derive(Debug, Clone)]
pub struct CT_DocPartPr {
    pub name: Option<String>,
    pub style: Option<String>,
    pub category: Option<String>,
    pub gallery: Option<String>,
    pub types: Vec<String>,
    pub behaviors: Vec<String>,
    pub description: Option<String>,
    pub guid: Option<String>,
    source: Option<Box<DocPartPrSource>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DocPartPrSnapshot {
    name: Option<String>,
    style: Option<String>,
    category: Option<String>,
    gallery: Option<String>,
    types: Vec<String>,
    behaviors: Vec<String>,
    description: Option<String>,
    guid: Option<String>,
}

#[derive(Debug, Clone)]
struct DocPartPrSource {
    raw_xml: Vec<u8>,
    root_start_xml: Vec<u8>,
    root_end_xml: Vec<u8>,
    field_xml: Vec<Option<Vec<u8>>>,
    extra_xml: Vec<(usize, Vec<u8>)>,
    unsupported: bool,
    word_prefixes: Vec<String>,
    output_prefix: String,
    declare_output_prefix: bool,
}

impl CT_DocPartPr {
    fn empty() -> Self {
        Self {
            name: None,
            style: None,
            category: None,
            gallery: None,
            types: Vec::new(),
            behaviors: Vec::new(),
            description: None,
            guid: None,
            source: None,
        }
    }

    fn snapshot(&self) -> DocPartPrSnapshot {
        DocPartPrSnapshot {
            name: self.name.clone(),
            style: self.style.clone(),
            category: self.category.clone(),
            gallery: self.gallery.clone(),
            types: self.types.clone(),
            behaviors: self.behaviors.clone(),
            description: self.description.clone(),
            guid: self.guid.clone(),
        }
    }

    pub fn has_unsupported_content(&self) -> bool {
        self.source
            .as_deref()
            .is_some_and(|source| source.unsupported)
    }

    fn to_xml(&self, original: &DocPartPrSnapshot) -> Result<Vec<u8>> {
        let Some(source) = self.source.as_deref() else {
            return self.to_canonical_xml(None);
        };
        if &self.snapshot() == original {
            return Ok(source.raw_xml.clone());
        }
        self.to_canonical_xml(Some((source, original)))
    }

    fn to_canonical_xml(
        &self,
        retained: Option<(&DocPartPrSource, &DocPartPrSnapshot)>,
    ) -> Result<Vec<u8>> {
        if self.category.is_some() != self.gallery.is_some() {
            return Err(OxmlError::MissingElement(
                "w:category requires both name and gallery values".to_owned(),
            ));
        }
        let mut writer = Writer::new(Vec::new());
        if let Some((source, _)) = retained {
            writer
                .get_mut()
                .extend_from_slice(&root_start_for_changed_output(source)?);
        } else {
            let mut root = BytesStart::new("w:docPartPr");
            root.push_attribute(("xmlns:w", W_NS));
            writer.write_event(Event::Start(root))?;
        }
        for slot in 0..=7 {
            if let Some((source, _)) = retained {
                for (_, raw) in source.extra_xml.iter().filter(|(at, _)| *at == slot) {
                    writer.get_mut().extend_from_slice(raw);
                }
            }
            if slot == 7 {
                continue;
            }
            let unchanged =
                retained.is_some_and(|(_, original)| property_slot_matches(self, original, slot));
            if unchanged
                && let Some(raw) = retained
                    .and_then(|(source, _)| source.field_xml.get(slot))
                    .and_then(Option::as_deref)
            {
                writer.get_mut().extend_from_slice(raw);
                continue;
            }
            if let Some((source, _)) = retained
                && let Some(raw) = source.field_xml.get(slot).and_then(Option::as_deref)
            {
                writer.get_mut().extend_from_slice(&rewrite_property_slot(
                    raw,
                    self,
                    slot,
                    &source.word_prefixes,
                    &source.output_prefix,
                )?);
            } else {
                let prefix = retained.map_or("w", |(source, _)| source.output_prefix.as_str());
                write_property_slot(&mut writer, self, slot, prefix)?;
            }
        }
        if let Some((source, _)) = retained {
            writer.get_mut().extend_from_slice(&source.root_end_xml);
        } else {
            writer.write_event(Event::End(BytesEnd::new("w:docPartPr")))?;
        }
        Ok(writer.into_inner())
    }
}

fn property_slot_matches(value: &CT_DocPartPr, original: &DocPartPrSnapshot, slot: usize) -> bool {
    match slot {
        0 => value.name == original.name,
        1 => value.style == original.style,
        2 => value.category == original.category && value.gallery == original.gallery,
        3 => value.types == original.types,
        4 => value.behaviors == original.behaviors,
        5 => value.description == original.description,
        6 => value.guid == original.guid,
        _ => true,
    }
}

fn write_property_slot<W: std::io::Write>(
    writer: &mut Writer<W>,
    value: &CT_DocPartPr,
    slot: usize,
    prefix: &str,
) -> Result<()> {
    match slot {
        0 => write_optional_value(writer, "name", value.name.as_deref(), prefix)?,
        1 => write_optional_value(writer, "style", value.style.as_deref(), prefix)?,
        2 if value.category.is_some() || value.gallery.is_some() => {
            writer.write_event(Event::Start(BytesStart::new(format!("{prefix}:category"))))?;
            write_optional_value(writer, "name", value.category.as_deref(), prefix)?;
            write_optional_value(writer, "gallery", value.gallery.as_deref(), prefix)?;
            writer.write_event(Event::End(BytesEnd::new(format!("{prefix}:category"))))?;
        }
        3 if !value.types.is_empty() => {
            writer.write_event(Event::Start(BytesStart::new(format!("{prefix}:types"))))?;
            for value in &value.types {
                write_value(writer, "type", value, prefix)?;
            }
            writer.write_event(Event::End(BytesEnd::new(format!("{prefix}:types"))))?;
        }
        4 if !value.behaviors.is_empty() => {
            writer.write_event(Event::Start(BytesStart::new(format!("{prefix}:behaviors"))))?;
            for value in &value.behaviors {
                write_value(writer, "behavior", value, prefix)?;
            }
            writer.write_event(Event::End(BytesEnd::new(format!("{prefix}:behaviors"))))?;
        }
        5 => write_optional_value(writer, "description", value.description.as_deref(), prefix)?,
        6 => write_optional_value(writer, "guid", value.guid.as_deref(), prefix)?,
        _ => {}
    }
    Ok(())
}

fn write_optional_value<W: std::io::Write>(
    writer: &mut Writer<W>,
    name: &str,
    value: Option<&str>,
    prefix: &str,
) -> Result<()> {
    if let Some(value) = value {
        write_value(writer, name, value, prefix)?;
    }
    Ok(())
}

fn write_value<W: std::io::Write>(
    writer: &mut Writer<W>,
    name: &str,
    value: &str,
    prefix: &str,
) -> Result<()> {
    let qname = format!("{prefix}:{name}");
    let mut element = BytesStart::new(qname);
    oxoffice_core::xml::push_escaped_attribute(&mut element, &format!("{prefix}:val"), value);
    writer.write_event(Event::Empty(element))?;
    Ok(())
}

impl CT_DocPart {
    /// Borrow the stored record without asserting current publication eligibility.
    #[doc(hidden)]
    pub fn retained_xml(&self) -> &[u8] {
        &self.raw_xml
    }

    fn to_xml(&self) -> Result<Vec<u8>> {
        Ok(self.to_xml_in(None)?.bytes)
    }

    /// Create an entry whose body is owned, namespace-complete XML.
    pub fn from_body_xml(content: &[u8]) -> Result<Self> {
        let mut xml =
            format!(r#"<w:docPart xmlns:w="{W_NS}"><w:docPartPr/><w:docPartBody>"#).into_bytes();
        xml.extend_from_slice(content);
        xml.extend_from_slice(b"</w:docPartBody></w:docPart>");
        validate_document_declarations_and_doctype(&xml)?;
        let mut part = parse_doc_part(xml, &["w".to_owned()], 0..0)?;
        part.source_span = None;
        Ok(part)
    }

    /// Replace owned content while retaining body-wrapper attributes and entry siblings.
    pub fn replace_body_content_xml(&mut self, content: &[u8]) -> Result<()> {
        let raw = self.to_xml()?;
        let current = parse_doc_part(raw, &self.word_prefixes, 0..0)?;
        let (mut body, end) = expanded_root_wrapper(&current.body_xml)?;
        body.extend_from_slice(content);
        body.extend(end);
        let updated = apply_glossary_edits(
            &current.raw_xml,
            vec![(current.body_span, GlossaryXml::generated(body, false))],
            None,
        )?
        .bytes;
        let mut writer = Writer::new(Vec::new());
        let mut wrapper = BytesStart::new("namespaceScope");
        for (name, namespace) in &self.inherited_namespaces {
            wrapper.push_attribute((name.as_str(), namespace.as_str()));
        }
        writer.write_event(Event::Start(wrapper))?;
        writer.get_mut().extend_from_slice(&updated);
        writer.write_event(Event::End(BytesEnd::new("namespaceScope")))?;
        validate_document_declarations_and_doctype(&writer.into_inner())?;
        let mut replacement = parse_doc_part(updated, &self.word_prefixes, 0..0)?;
        replacement.source_span = self.source_span.clone();
        replacement.dirty_raw = true;
        replacement.inherited_namespaces = self.inherited_namespaces.clone();
        *self = replacement;
        Ok(())
    }

    pub fn has_unsupported_content(&self) -> bool {
        self.properties.has_unsupported_content()
            || self
                .body
                .content
                .iter()
                .any(|item| matches!(item, crate::document::BodyContent::RawXml(_)))
    }

    fn is_unchanged(&self) -> bool {
        !self.dirty_raw
            && self.properties.snapshot() == self.original_properties
            && self.body == self.original_body
    }

    fn to_xml_in(
        &self,
        mut work: Option<&mut crate::text::WordOperationWork>,
    ) -> Result<GlossaryXml> {
        let _part_scope = crate::text::raw_root_binding_scope(&self.raw_xml, false)?;
        if self.is_unchanged() {
            return GlossaryXml::retained(&self.raw_xml, work);
        }
        let properties_changed = self.properties.snapshot() != self.original_properties;
        let body_changed = self.body != self.original_body;
        let mut edits = Vec::new();
        if properties_changed {
            let properties = self.properties.to_xml(&self.original_properties)?;
            if let Some(properties_span) = self.properties_span.clone() {
                edits.push((
                    properties_span,
                    GlossaryXml::generated(properties, work.is_some()),
                ));
            } else {
                edits.push((
                    self.body_span.start..self.body_span.start,
                    GlossaryXml::generated(properties, work.is_some()),
                ));
            }
        }
        if body_changed {
            let body = serialize_doc_part_body(&self.body, &self.body_xml, work.as_deref_mut())?;
            edits.push((self.body_span.clone(), body));
        }
        apply_glossary_edits(&self.raw_xml, edits, work)
    }
}

impl CT_GlossaryDocument {
    /// Borrow the stored incoming glossary record without serialization.
    #[doc(hidden)]
    pub fn retained_xml(&self) -> &[u8] {
        &self.raw_xml
    }

    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        crate::document::admit_word_xml(xml, &[], None)?;
        validate_document_declarations_and_doctype(xml)?;
        let mut reader = Reader::from_reader(xml);
        reader.config_mut().trim_text(false);
        let mut buffer = Vec::new();
        let mut word_prefixes = Vec::new();
        let mut prefix_scopes = Vec::new();
        let mut stack = Vec::<Option<String>>::new();
        let mut root_seen = false;
        let mut root_closed = false;
        let mut declaration_allowed = true;
        let mut declaration_seen = false;
        let mut doc_parts_count = 0usize;
        let mut doc_parts = Vec::new();
        let mut inherited_namespaces = BTreeMap::new();

        loop {
            let event_start = reader.buffer_position() as usize;
            match reader.read_event_into(&mut buffer)? {
                Event::Decl(_) => {
                    if !declaration_allowed || declaration_seen {
                        return Err(OxmlError::InvalidValue(
                            "misplaced or duplicate glossary XML declaration".to_owned(),
                        ));
                    }
                    declaration_seen = true;
                    declaration_allowed = false;
                }
                Event::DocType(_) => {
                    return Err(OxmlError::InvalidValue(
                        "glossary XML cannot contain a document type".to_owned(),
                    ));
                }
                Event::Start(element) => {
                    declaration_allowed = false;
                    if root_closed {
                        return Err(OxmlError::InvalidValue(
                            "multiple top-level glossary roots".to_owned(),
                        ));
                    }
                    let local_prefixes = word_prefixes_at(&element, &word_prefixes)?;
                    let local = word_local_name(&element, &local_prefixes);
                    if !root_seen {
                        if local.as_deref() != Some("glossaryDocument") {
                            return Err(OxmlError::MissingElement(
                                "w:glossaryDocument root".to_owned(),
                            ));
                        }
                        root_seen = true;
                        collect_namespace_declarations(&element, &mut inherited_namespaces)?;
                        stack.push(local);
                        let local_prefixes = local_prefixes.into_owned();
                        prefix_scopes.push(std::mem::replace(&mut word_prefixes, local_prefixes));
                    } else if stack.len() == 1
                        && stack[0].as_deref() == Some("glossaryDocument")
                        && local.as_deref() == Some("docParts")
                    {
                        collect_namespace_declarations(&element, &mut inherited_namespaces)?;
                        doc_parts_count += 1;
                        if doc_parts_count > 1 {
                            return Err(OxmlError::InvalidValue(
                                "multiple direct w:docParts containers".to_owned(),
                            ));
                        }
                        stack.push(local);
                        let local_prefixes = local_prefixes.into_owned();
                        prefix_scopes.push(std::mem::replace(&mut word_prefixes, local_prefixes));
                    } else if stack.len() == 2
                        && stack[0].as_deref() == Some("glossaryDocument")
                        && stack[1].as_deref() == Some("docParts")
                        && local.as_deref() == Some("docPart")
                    {
                        let raw = capture_element(&mut reader, &element)?;
                        let source_span = event_start..reader.buffer_position() as usize;
                        let mut part = parse_doc_part(raw, &local_prefixes, source_span)?;
                        part.inherited_namespaces = inherited_namespaces.clone();
                        doc_parts.push(part);
                    } else {
                        stack.push(local);
                        let local_prefixes = local_prefixes.into_owned();
                        prefix_scopes.push(std::mem::replace(&mut word_prefixes, local_prefixes));
                    }
                }
                Event::Empty(element) => {
                    declaration_allowed = false;
                    if root_closed {
                        return Err(OxmlError::InvalidValue(
                            "multiple top-level glossary roots".to_owned(),
                        ));
                    }
                    let local_prefixes = word_prefixes_at(&element, &word_prefixes)?;
                    let local = word_local_name(&element, &local_prefixes);
                    if !root_seen {
                        return Err(OxmlError::MissingElement(
                            "nonempty w:glossaryDocument root".to_owned(),
                        ));
                    }
                    if stack.len() == 1
                        && stack[0].as_deref() == Some("glossaryDocument")
                        && local.as_deref() == Some("docParts")
                    {
                        doc_parts_count += 1;
                        if doc_parts_count > 1 {
                            return Err(OxmlError::InvalidValue(
                                "multiple direct w:docParts containers".to_owned(),
                            ));
                        }
                    } else if stack.len() == 2
                        && stack[1].as_deref() == Some("docParts")
                        && local.as_deref() == Some("docPart")
                    {
                        let source_span = event_start..reader.buffer_position() as usize;
                        let mut part = parse_doc_part(
                            capture_empty_element(&element)?,
                            &local_prefixes,
                            source_span,
                        )?;
                        part.inherited_namespaces = inherited_namespaces.clone();
                        doc_parts.push(part);
                    }
                }
                Event::End(_) => {
                    declaration_allowed = false;
                    if stack.len() == 1 {
                        root_closed = true;
                    }
                    stack.pop();
                    word_prefixes = prefix_scopes.pop().unwrap_or_default();
                }
                Event::Text(text)
                    if text.as_ref().iter().any(|byte| !byte.is_ascii_whitespace())
                        && (stack.is_empty() || is_glossary_element_only_container(&stack)) =>
                {
                    return Err(OxmlError::InvalidValue(
                        "non-whitespace text in glossary element-only content".to_owned(),
                    ));
                }
                Event::CData(text)
                    if stack.is_empty()
                        || (is_glossary_element_only_container(&stack)
                            && text.as_ref().iter().any(|byte| !byte.is_ascii_whitespace())) =>
                {
                    return Err(OxmlError::InvalidValue(
                        "character data in glossary element-only content".to_owned(),
                    ));
                }
                Event::GeneralRef(reference) => {
                    let non_whitespace = match reference.resolve_char_ref() {
                        Ok(Some(character)) => !character.is_ascii_whitespace(),
                        Ok(None) | Err(_) => true,
                    };
                    if stack.is_empty()
                        || (is_glossary_element_only_container(&stack) && non_whitespace)
                    {
                        return Err(OxmlError::InvalidValue(
                            "character reference in glossary element-only content".to_owned(),
                        ));
                    }
                }
                Event::Eof => break,
                _ => declaration_allowed = false,
            }
            buffer.clear();
        }
        if !root_seen || !root_closed || doc_parts_count != 1 {
            return Err(OxmlError::MissingElement(
                "w:glossaryDocument root with one direct w:docParts".to_owned(),
            ));
        }
        let source_spans = doc_parts
            .iter()
            .filter_map(|part| part.source_span.clone())
            .collect();
        Ok(Self {
            doc_parts,
            raw_xml: xml.to_vec(),
            source_spans,
        })
    }

    /// Construct the retained empty glossary used by facade authoring.
    pub fn new() -> Result<Self> {
        Self::from_xml(format!(r#"<w:glossaryDocument xmlns:w="{W_NS}"><w:docParts></w:docParts></w:glossaryDocument>"#).as_bytes())
    }

    /// Absolute source range of one body wrapper in an unchanged parsed root.
    pub fn body_range(&self, ordinal: usize) -> Result<Range<usize>> {
        let part = self
            .doc_parts
            .get(ordinal)
            .ok_or_else(|| OxmlError::InvalidValue("stale glossary ordinal".to_owned()))?;
        let span = part.source_span.as_ref().ok_or_else(|| {
            OxmlError::InvalidValue("new glossary entry has no source range".to_owned())
        })?;
        Ok(span.start + part.body_span.start..span.start + part.body_span.end)
    }

    /// Validate all stored owner models before cloning or serialization.
    #[doc(hidden)]
    pub fn validate_resource_limits(&self) -> Result<()> {
        for part in &self.doc_parts {
            part.body.validate_resource_limits()?;
            part.original_body.validate_resource_limits()?;
        }
        Ok(())
    }

    pub fn to_xml(&self) -> Result<Vec<u8>> {
        Ok(self.to_xml_in(None)?.bytes)
    }

    pub(crate) fn to_xml_in(
        &self,
        mut work: Option<&mut crate::text::WordOperationWork>,
    ) -> Result<GlossaryXml> {
        let _root_scope = crate::text::raw_root_binding_scope(&self.raw_xml, true)?;
        self.validate_resource_limits()?;
        let (container, empty) = doc_parts_close(&self.raw_xml)?;
        if empty {
            if self.doc_parts.is_empty() {
                return GlossaryXml::retained(&self.raw_xml, work);
            }
            if self.doc_parts.iter().any(|part| part.source_span.is_some()) {
                return Err(OxmlError::InvalidValue(
                    "entry source does not belong to empty glossary".to_owned(),
                ));
            }
            let (start, end) = expanded_root_wrapper(&self.raw_xml[container.clone()])?;
            let mut replacement = GlossaryXml::generated(start, work.is_some());
            for part in &self.doc_parts {
                replacement.append(part.to_xml_in(work.as_deref_mut())?)?;
            }
            replacement.append(GlossaryXml::generated(end, work.is_some()))?;
            return apply_glossary_edits(&self.raw_xml, vec![(container, replacement)], work);
        }
        let mut edits = Vec::new();
        let mut retained = Vec::new();
        let mut pending = GlossaryXml::generated(Vec::new(), work.is_some());
        let mut previous = None;
        for part in &self.doc_parts {
            if let Some(span) = &part.source_span {
                let index = self
                    .source_spans
                    .iter()
                    .position(|source| source == span)
                    .ok_or_else(|| {
                        OxmlError::InvalidValue("foreign glossary source span".to_owned())
                    })?;
                if previous.is_some_and(|previous| index <= previous) {
                    return Err(OxmlError::InvalidValue(
                        "reordered or duplicate retained glossary entries".to_owned(),
                    ));
                }
                previous = Some(index);
                retained.push(span.clone());
                if !pending.bytes.is_empty() {
                    edits.push((
                        span.start..span.start,
                        std::mem::replace(
                            &mut pending,
                            GlossaryXml::generated(Vec::new(), work.is_some()),
                        ),
                    ));
                }
                if !part.is_unchanged() {
                    edits.push((span.clone(), part.to_xml_in(work.as_deref_mut())?));
                }
            } else {
                pending.append(part.to_xml_in(work.as_deref_mut())?)?;
            }
        }
        if !pending.bytes.is_empty() {
            edits.push((container.start..container.start, pending));
        }
        for span in &self.source_spans {
            if !retained.contains(span) {
                edits.push((
                    span.clone(),
                    GlossaryXml::generated(Vec::new(), work.is_some()),
                ));
            }
        }
        apply_glossary_edits(&self.raw_xml, edits, work)
    }
}

fn collect_namespace_declarations(
    element: &BytesStart<'_>,
    scope: &mut BTreeMap<String, String>,
) -> Result<()> {
    for attribute in element.attributes() {
        let attribute = attribute?;
        let name = std::str::from_utf8(attribute.key.as_ref())
            .map_err(|error| OxmlError::InvalidValue(error.to_string()))?;
        if name == "xmlns" || name.starts_with("xmlns:") {
            scope.insert(
                name.to_owned(),
                attribute
                    .decoded_and_normalized_value(
                        quick_xml::XmlVersion::Implicit1_0,
                        element.decoder(),
                    )?
                    .into_owned(),
            );
        }
    }
    Ok(())
}

fn doc_parts_close(xml: &[u8]) -> Result<(Range<usize>, bool)> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    loop {
        let before = reader.buffer_position() as usize;
        let (namespace, event) = reader.read_resolved_event_into(&mut buffer)?;
        let is_word = namespace_is(&namespace, W_NS);
        match event {
            Event::Start(_) => depth += 1,
            Event::Empty(element)
                if depth == 1 && is_word && element.local_name().as_ref() == b"docParts" =>
            {
                return Ok((before..reader.buffer_position() as usize, true));
            }
            Event::End(element) => {
                if depth == 2 && is_word && element.local_name().as_ref() == b"docParts" {
                    return Ok((before..before, false));
                }
                depth = depth.saturating_sub(1);
            }
            Event::Eof => return Err(OxmlError::MissingElement("w:docParts".to_owned())),
            _ => {}
        }
        buffer.clear();
    }
}

fn is_glossary_element_only_container(stack: &[Option<String>]) -> bool {
    matches!(stack, [Some(root)] if root == "glossaryDocument")
        || matches!(
            stack,
            [Some(root), Some(container)]
                if root == "glossaryDocument" && container == "docParts"
        )
}

fn validate_document_declarations_and_doctype(xml: &[u8]) -> Result<()> {
    validate_strict_xml_1_0(xml).map_err(glossary_lexical_error)?;
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut declaration_allowed = true;
    let mut declaration_seen = false;
    let mut stack = Vec::<Option<Vec<u8>>>::new();
    loop {
        let (namespace, event) = reader.read_resolved_event_into(&mut buffer)?;
        let is_word = namespace_is(&namespace, W_NS);
        let event = event.into_owned();
        drop(namespace);
        match event {
            Event::Decl(_) => {
                if !declaration_allowed || declaration_seen {
                    return Err(OxmlError::InvalidValue(
                        "misplaced or duplicate glossary XML declaration".to_owned(),
                    ));
                }
                declaration_seen = true;
                declaration_allowed = false;
            }
            Event::DocType(_) => {
                return Err(OxmlError::InvalidValue(
                    "glossary XML cannot contain a document type".to_owned(),
                ));
            }
            Event::Start(element) => {
                declaration_allowed = false;
                let element_name = element.name();
                let local = local_name(element_name.as_ref());
                stack.push(
                    (is_word && glossary_container_is_element_only(local)).then(|| local.to_vec()),
                );
            }
            Event::Empty(_) => {
                declaration_allowed = false;
            }
            Event::End(_) => {
                declaration_allowed = false;
                stack.pop();
            }
            Event::Text(text)
                if stack.last().is_some_and(Option::is_some)
                    && text.iter().any(|byte| !byte.is_ascii_whitespace()) =>
            {
                return Err(OxmlError::InvalidValue(
                    "non-whitespace text in glossary element-only content".to_owned(),
                ));
            }
            Event::CData(text)
                if stack.last().is_some_and(Option::is_some)
                    && text.iter().any(|byte| !byte.is_ascii_whitespace()) =>
            {
                return Err(OxmlError::InvalidValue(
                    "character data in glossary element-only content".to_owned(),
                ));
            }
            Event::GeneralRef(reference) => {
                let non_whitespace = reference
                    .resolve_char_ref()
                    .ok()
                    .flatten()
                    .is_none_or(|character| !character.is_ascii_whitespace());
                if stack.last().is_some_and(Option::is_some) && non_whitespace {
                    return Err(OxmlError::InvalidValue(
                        "character reference in glossary element-only content".to_owned(),
                    ));
                }
            }
            Event::Eof => return Ok(()),
            _ => declaration_allowed = false,
        }
        buffer.clear();
    }
}

fn glossary_lexical_error(error: XmlLexicalError) -> OxmlError {
    let message = match error {
        XmlLexicalError::InvalidUtf8 => "glossary XML is not UTF-8".to_owned(),
        XmlLexicalError::InvalidDeclaration(reason) if reason == "must begin with version" => {
            "glossary XML declaration must begin with version".to_owned()
        }
        XmlLexicalError::InvalidDeclaration(reason)
            if reason == "attributes are invalid, duplicated, or out of order" =>
        {
            "invalid glossary XML declaration".to_owned()
        }
        XmlLexicalError::InvalidDeclaration(reason) => {
            format!("invalid glossary XML declaration: {reason}")
        }
        XmlLexicalError::ForbiddenLiteralCharacter => {
            "glossary XML contains a forbidden literal XML 1.0 character".to_owned()
        }
        XmlLexicalError::InvalidName(reason) if reason.starts_with("qualified name ") => {
            format!("invalid glossary XML {reason}")
        }
        XmlLexicalError::InvalidName(reason) if reason.starts_with("name ") => {
            format!("invalid glossary XML {reason}")
        }
        XmlLexicalError::InvalidName(reason) => format!("invalid glossary XML: {reason}"),
        XmlLexicalError::InvalidNamespace(reason)
            if reason == "element uses the reserved xmlns prefix" =>
        {
            "glossary XML element uses the reserved xmlns prefix".to_owned()
        }
        XmlLexicalError::InvalidNamespace(reason) if reason == "invalid namespace declaration" => {
            "glossary XML contains an invalid namespace declaration".to_owned()
        }
        XmlLexicalError::InvalidNamespace(_) => {
            "glossary XML uses an unbound namespace prefix".to_owned()
        }
        XmlLexicalError::DuplicateExpandedAttribute => {
            "glossary XML element has duplicate expanded-name attributes".to_owned()
        }
        XmlLexicalError::InvalidReference(reason)
            if reason == "attribute contains a literal less-than sign" =>
        {
            "glossary XML attribute contains a literal less-than sign".to_owned()
        }
        XmlLexicalError::InvalidReference(reason)
            if reason == "character reference is not legal in XML 1.0" =>
        {
            "glossary character reference is not legal in XML 1.0".to_owned()
        }
        XmlLexicalError::InvalidReference(reason)
            if reason.starts_with("undeclared entity reference ") =>
        {
            reason.replacen(
                "undeclared entity reference ",
                "undeclared glossary XML entity reference ",
                1,
            )
        }
        XmlLexicalError::InvalidReference(reason) => format!("invalid glossary XML: {reason}"),
        XmlLexicalError::InvalidProcessingInstruction(reason)
            if reason == "reserved XML target" =>
        {
            "reserved glossary XML processing instruction".to_owned()
        }
        XmlLexicalError::InvalidProcessingInstruction(reason) if reason.starts_with("name ") => {
            format!("invalid glossary XML {reason}")
        }
        XmlLexicalError::InvalidProcessingInstruction(reason) => {
            format!("invalid glossary XML processing instruction: {reason}")
        }
        XmlLexicalError::InvalidComment(reason) => {
            format!("invalid glossary XML comment: {reason}")
        }
    };
    OxmlError::InvalidValue(message)
}

fn glossary_container_is_element_only(local: &[u8]) -> bool {
    matches!(
        local,
        b"glossaryDocument"
            | b"docParts"
            | b"docPart"
            | b"docPartPr"
            | b"category"
            | b"types"
            | b"behaviors"
    )
}

fn namespace_is(namespace: &ResolveResult<'_>, expected: &str) -> bool {
    matches!(namespace, ResolveResult::Bound(Namespace(uri)) if *uri == expected.as_bytes())
}

fn local_name(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

fn parse_doc_part(
    raw_xml: Vec<u8>,
    inherited_prefixes: &[String],
    source_span: Range<usize>,
) -> Result<CT_DocPart> {
    let mut reader = Reader::from_reader(raw_xml.as_slice());
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut prefixes = inherited_prefixes.to_vec();
    let mut prefix_scopes = Vec::new();
    let mut owner_prefixes = inherited_prefixes.to_vec();
    let mut depth = 0usize;
    let mut properties_xml = None;
    let mut properties_span = None;
    let mut body_xml = None;
    let mut body_span = None;
    loop {
        let event_start = reader.buffer_position() as usize;
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) => {
                let local_prefixes = word_prefixes_at(&element, &prefixes)?;
                let local = word_local_name(&element, &local_prefixes);
                if depth == 0 {
                    owner_prefixes = local_prefixes.to_vec();
                }
                if depth == 1 && local.as_deref() == Some("docPartPr") {
                    if properties_xml.is_some() {
                        return Err(OxmlError::InvalidValue(
                            "multiple direct w:docPartPr children".to_owned(),
                        ));
                    }
                    if body_xml.is_some() {
                        return Err(OxmlError::InvalidValue(
                            "w:docPartPr must precede w:docPartBody".to_owned(),
                        ));
                    }
                    properties_xml = Some(capture_element(&mut reader, &element)?);
                    properties_span = Some(event_start..reader.buffer_position() as usize);
                } else if depth == 1 && local.as_deref() == Some("docPartBody") {
                    if body_xml.is_some() {
                        return Err(OxmlError::InvalidValue(
                            "multiple direct w:docPartBody children".to_owned(),
                        ));
                    }
                    body_xml = Some(capture_element(&mut reader, &element)?);
                    body_span = Some(event_start..reader.buffer_position() as usize);
                } else {
                    depth += 1;
                    let local_prefixes = local_prefixes.into_owned();
                    prefix_scopes.push(std::mem::replace(&mut prefixes, local_prefixes));
                }
            }
            Event::Empty(element) => {
                let local_prefixes = word_prefixes_at(&element, &prefixes)?;
                let local = word_local_name(&element, &local_prefixes);
                if depth == 1 && local.as_deref() == Some("docPartPr") {
                    if properties_xml.is_some() {
                        return Err(OxmlError::InvalidValue(
                            "multiple direct w:docPartPr children".to_owned(),
                        ));
                    }
                    if body_xml.is_some() {
                        return Err(OxmlError::InvalidValue(
                            "w:docPartPr must precede w:docPartBody".to_owned(),
                        ));
                    }
                    properties_xml = Some(capture_empty_element(&element)?);
                    properties_span = Some(event_start..reader.buffer_position() as usize);
                } else if depth == 1 && local.as_deref() == Some("docPartBody") {
                    if body_xml.is_some() {
                        return Err(OxmlError::InvalidValue(
                            "multiple direct w:docPartBody children".to_owned(),
                        ));
                    }
                    body_xml = Some(capture_empty_element(&element)?);
                    body_span = Some(event_start..reader.buffer_position() as usize);
                }
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                prefixes = prefix_scopes
                    .pop()
                    .unwrap_or_else(|| inherited_prefixes.to_vec());
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let body_xml = body_xml.ok_or_else(|| OxmlError::MissingElement("w:docPartBody".to_owned()))?;
    let body_span = body_span.expect("body span accompanies body XML");
    let properties = properties_xml
        .as_deref()
        .map(|xml| parse_properties(xml, &owner_prefixes))
        .transpose()?
        .unwrap_or_else(CT_DocPartPr::empty);
    let body = parse_doc_part_body(&body_xml, &owner_prefixes)?;
    Ok(CT_DocPart {
        original_properties: properties.snapshot(),
        original_body: body.clone(),
        word_prefixes: owner_prefixes.clone(),
        dirty_raw: false,
        inherited_namespaces: BTreeMap::new(),
        properties,
        body,
        raw_xml,
        body_xml,
        source_span: Some(source_span),
        properties_span,
        body_span,
    })
}

fn parse_properties(raw_xml: &[u8], inherited_prefixes: &[String]) -> Result<CT_DocPartPr> {
    let mut reader = Reader::from_reader(raw_xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut prefixes = inherited_prefixes.to_vec();
    let mut root_seen = false;
    let mut slot = 0usize;
    let mut field_xml = vec![None; 7];
    let mut seen_fields = [false; 7];
    let mut extra_xml = Vec::new();
    let mut unsupported = false;
    let mut root_start_xml = Vec::new();
    let mut root_end_xml = Vec::new();
    let mut source_word_prefixes = inherited_prefixes.to_vec();
    let mut value = CT_DocPartPr {
        name: None,
        style: None,
        category: None,
        gallery: None,
        types: Vec::new(),
        behaviors: Vec::new(),
        description: None,
        guid: None,
        source: None,
    };
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) => {
                let local_prefixes = word_prefixes_at(&element, &prefixes)?;
                if !root_seen {
                    root_seen = true;
                    root_start_xml = standalone_event(Event::Start(element.into_owned()))?;
                    source_word_prefixes = local_prefixes.to_vec();
                    prefixes = local_prefixes.into_owned();
                } else {
                    let raw = capture_element(&mut reader, &element)?;
                    if let Some(field_slot) = property_slot(&element, &local_prefixes) {
                        if std::mem::replace(&mut seen_fields[field_slot], true) {
                            return Err(OxmlError::InvalidValue(format!(
                                "duplicate modeled glossary property in slot {field_slot}"
                            )));
                        }
                        if field_slot < slot {
                            return Err(OxmlError::InvalidValue(format!(
                                "out-of-order modeled glossary property in slot {field_slot}"
                            )));
                        }
                        apply_property_field(&mut value, field_slot, &raw, &local_prefixes)?;
                        field_xml[field_slot] = Some(raw);
                        slot = field_slot + 1;
                    } else {
                        unsupported = true;
                        extra_xml.push((slot, raw));
                    }
                }
            }
            Event::Empty(element) => {
                let local_prefixes = word_prefixes_at(&element, &prefixes)?;
                if !root_seen {
                    root_seen = true;
                    source_word_prefixes = local_prefixes.into_owned();
                    let name = String::from_utf8_lossy(element.name().as_ref()).into_owned();
                    root_start_xml = standalone_event(Event::Start(element.to_owned()))?;
                    root_end_xml = standalone_event(Event::End(BytesEnd::new(name)))?;
                } else {
                    let raw = capture_empty_element(&element)?;
                    if let Some(field_slot) = property_slot(&element, &local_prefixes) {
                        if std::mem::replace(&mut seen_fields[field_slot], true) {
                            return Err(OxmlError::InvalidValue(format!(
                                "duplicate modeled glossary property in slot {field_slot}"
                            )));
                        }
                        if field_slot < slot {
                            return Err(OxmlError::InvalidValue(format!(
                                "out-of-order modeled glossary property in slot {field_slot}"
                            )));
                        }
                        apply_property_field(&mut value, field_slot, &raw, &local_prefixes)?;
                        field_xml[field_slot] = Some(raw);
                        slot = field_slot + 1;
                    } else {
                        unsupported = true;
                        extra_xml.push((slot, raw));
                    }
                }
            }
            Event::End(element) if root_seen => {
                root_end_xml = standalone_event(Event::End(element.into_owned()))?;
                break;
            }
            Event::Eof => break,
            event if root_seen => {
                let raw = standalone_event(event.into_owned())?;
                if !raw.is_empty() {
                    extra_xml.push((slot, raw));
                }
            }
            _ => {}
        }
        buffer.clear();
    }
    let output_prefix = changed_output_prefix(&source_word_prefixes, &root_start_xml);
    value.source = Some(Box::new(DocPartPrSource {
        raw_xml: raw_xml.to_vec(),
        root_start_xml,
        root_end_xml,
        field_xml,
        extra_xml,
        unsupported,
        output_prefix,
        declare_output_prefix: !source_word_prefixes
            .iter()
            .any(|prefix| !prefix.is_empty() && !prefix.starts_with('\0')),
        word_prefixes: source_word_prefixes,
    }));
    Ok(value)
}

fn changed_output_prefix(prefixes: &[String], root_start: &[u8]) -> String {
    if let Some(prefix) = prefixes
        .iter()
        .find(|prefix| !prefix.is_empty() && !prefix.starts_with('\0'))
    {
        return prefix.clone();
    }
    for index in 0usize.. {
        let candidate = if index == 0 {
            "w".to_owned()
        } else {
            format!("w{index}")
        };
        let attribute = format!("xmlns:{candidate}");
        if !start_tag_has_attribute(root_start, attribute.as_bytes()) {
            return candidate;
        }
    }
    unreachable!()
}

fn root_start_for_changed_output(source: &DocPartPrSource) -> Result<Vec<u8>> {
    let mut start = source.root_start_xml.clone();
    if !source.declare_output_prefix {
        return Ok(start);
    }
    let insert_at = start
        .iter()
        .rposition(|byte| *byte == b'>')
        .ok_or_else(|| OxmlError::MissingElement("w:docPartPr start tag".to_owned()))?;
    start.splice(
        insert_at..insert_at,
        format!(r#" xmlns:{}="{W_NS}""#, source.output_prefix).bytes(),
    );
    Ok(start)
}

fn rewrite_property_slot(
    raw: &[u8],
    value: &CT_DocPartPr,
    slot: usize,
    word_prefixes: &[String],
    output_prefix: &str,
) -> Result<Vec<u8>> {
    match slot {
        0 => patch_root_word_value(raw, word_prefixes, value.name.as_deref(), output_prefix),
        1 => patch_root_word_value(raw, word_prefixes, value.style.as_deref(), output_prefix),
        2 if value.category.is_none() && value.gallery.is_none() => Ok(Vec::new()),
        2 => patch_container_values(
            raw,
            word_prefixes,
            &[
                ("name", value.category.as_deref()),
                ("gallery", value.gallery.as_deref()),
            ],
            output_prefix,
        ),
        3 if value.types.is_empty() => Ok(Vec::new()),
        3 => {
            patch_repeated_container_values(raw, word_prefixes, "type", &value.types, output_prefix)
        }
        4 if value.behaviors.is_empty() => Ok(Vec::new()),
        4 => patch_repeated_container_values(
            raw,
            word_prefixes,
            "behavior",
            &value.behaviors,
            output_prefix,
        ),
        5 => patch_root_word_value(
            raw,
            word_prefixes,
            value.description.as_deref(),
            output_prefix,
        ),
        6 => patch_root_word_value(raw, word_prefixes, value.guid.as_deref(), output_prefix),
        _ => Ok(raw.to_vec()),
    }
}

fn patch_root_word_value(
    raw: &[u8],
    inherited_prefixes: &[String],
    value: Option<&str>,
    output_prefix: &str,
) -> Result<Vec<u8>> {
    if value.is_none() {
        return Ok(Vec::new());
    }
    let mut reader = Reader::from_reader(raw);
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            event @ (Event::Start(_) | Event::Empty(_)) => {
                let empty = matches!(event, Event::Empty(_));
                let element = match event {
                    Event::Start(element) | Event::Empty(element) => element,
                    _ => unreachable!(),
                };
                let prefixes = word_prefixes_at(&element, inherited_prefixes)?;
                let mut tag = if empty {
                    let mut writer = Writer::new(Vec::new());
                    writer.write_event(Event::Empty(element.clone()))?;
                    writer.into_inner()
                } else {
                    let mut writer = Writer::new(Vec::new());
                    writer.write_event(Event::Start(element.clone()))?;
                    writer.into_inner()
                };
                let mut attribute_key = None;
                for attribute in element.attributes() {
                    let attribute = attribute?;
                    if is_word_attribute(attribute.key.as_ref(), b"val", &prefixes) {
                        attribute_key = Some(attribute.key.as_ref().to_vec());
                        break;
                    }
                }
                match (attribute_key, value) {
                    (Some(key), Some(value)) => {
                        if let Some(range) = lexical_attribute_value_range(&tag, &key) {
                            let escaped = oxoffice_core::xml::escape_attribute_value(value);
                            tag.splice(range, escaped.as_bytes().iter().copied());
                        }
                    }
                    (Some(_), None) => unreachable!("handled before parsing the element"),
                    (None, Some(value)) => {
                        let (prefix, declare) = usable_word_prefix(&prefixes, &tag, output_prefix);
                        if declare {
                            declare_word_prefix(&mut tag, 0, &prefix)?;
                        }
                        let insert_at = start_tag(&tag)?.attributes_end;
                        let escaped = oxoffice_core::xml::escape_attribute_value(value);
                        tag.splice(
                            insert_at..insert_at,
                            format!(" {prefix}:val=\"{escaped}\"").bytes(),
                        );
                    }
                    (None, None) => unreachable!("handled before parsing the element"),
                }
                let original_tag_len = reader.buffer_position() as usize;
                let mut output = tag;
                output.extend_from_slice(&raw[original_tag_len..]);
                return Ok(output);
            }
            Event::Eof => return Ok(raw.to_vec()),
            _ => {}
        }
        buffer.clear();
    }
}

fn patch_container_values(
    raw: &[u8],
    inherited_prefixes: &[String],
    desired: &[(&str, Option<&str>)],
    output_prefix: &str,
) -> Result<Vec<u8>> {
    let (raw, output_prefix) = prepare_container_output(raw, inherited_prefixes, output_prefix)?;
    let seen = std::cell::RefCell::new(vec![false; desired.len()]);
    rewrite_container(
        &raw,
        inherited_prefixes,
        |name, child, prefixes, output| {
            let Some(index) = desired.iter().position(|(candidate, _)| *candidate == name) else {
                output.extend_from_slice(child);
                return Ok(());
            };
            let mut seen = seen.borrow_mut();
            if seen[index] {
                return Err(OxmlError::InvalidValue(format!(
                    "duplicate direct w:{} child",
                    desired[index].0
                )));
            }
            for prior in 0..index {
                if !seen[prior] {
                    if let Some(value) = desired[prior].1 {
                        write_value_bytes(output, desired[prior].0, value, &output_prefix);
                    }
                    seen[prior] = true;
                }
            }
            output.extend_from_slice(&patch_root_word_value(
                child,
                prefixes,
                desired[index].1,
                &output_prefix,
            )?);
            seen[index] = true;
            Ok(())
        },
        |output| {
            let seen = seen.borrow();
            for (index, (name, value)) in desired.iter().enumerate() {
                if !seen[index]
                    && let Some(value) = value
                {
                    write_value_bytes(output, name, value, &output_prefix);
                }
            }
        },
    )
}

fn patch_repeated_container_values(
    raw: &[u8],
    inherited_prefixes: &[String],
    child_name: &str,
    desired: &[String],
    output_prefix: &str,
) -> Result<Vec<u8>> {
    let (raw, output_prefix) = prepare_container_output(raw, inherited_prefixes, output_prefix)?;
    let next = std::cell::Cell::new(0usize);
    rewrite_container(
        &raw,
        inherited_prefixes,
        |name, child, prefixes, output| {
            if name != child_name {
                output.extend_from_slice(child);
                return Ok(());
            }
            let index = next.get();
            let value = desired.get(index).map(String::as_str);
            output.extend_from_slice(&patch_root_word_value(
                child,
                prefixes,
                value,
                &output_prefix,
            )?);
            next.set(index + usize::from(value.is_some()));
            Ok(())
        },
        |output| {
            for value in &desired[next.get()..] {
                write_value_bytes(output, child_name, value, &output_prefix);
            }
        },
    )
}

fn rewrite_container<Child, Finish>(
    raw: &[u8],
    inherited_prefixes: &[String],
    mut child: Child,
    mut finish: Finish,
) -> Result<Vec<u8>>
where
    Child: FnMut(&str, &[u8], &[String], &mut Vec<u8>) -> Result<()>,
    Finish: FnMut(&mut Vec<u8>),
{
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut output = Vec::with_capacity(raw.len());
    let mut prefixes = inherited_prefixes.to_vec();
    let mut prefix_scopes = Vec::new();
    let mut depth = 0usize;
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) => {
                let local_prefixes = word_prefixes_at(&element, &prefixes)?;
                let local = word_local_name(&element, &local_prefixes);
                if depth == 1
                    && let Some(local) = local
                {
                    let raw_child = capture_element(&mut reader, &element)?;
                    child(&local, &raw_child, &local_prefixes, &mut output)?;
                } else {
                    let mut writer = Writer::new(Vec::new());
                    writer.write_event(Event::Start(element))?;
                    output.extend_from_slice(&writer.into_inner());
                    depth += 1;
                    let local_prefixes = local_prefixes.into_owned();
                    prefix_scopes.push(std::mem::replace(&mut prefixes, local_prefixes));
                }
            }
            Event::Empty(element) => {
                let local_prefixes = word_prefixes_at(&element, &prefixes)?;
                let local = word_local_name(&element, &local_prefixes);
                let raw_child = capture_empty_element(&element)?;
                if depth == 1
                    && let Some(local) = local
                {
                    child(&local, &raw_child, &local_prefixes, &mut output)?;
                } else {
                    output.extend_from_slice(&raw_child);
                }
            }
            Event::End(element) => {
                if depth == 1 {
                    finish(&mut output);
                }
                let mut writer = Writer::new(Vec::new());
                writer.write_event(Event::End(element))?;
                output.extend_from_slice(&writer.into_inner());
                depth = depth.saturating_sub(1);
                prefixes = prefix_scopes
                    .pop()
                    .unwrap_or_else(|| inherited_prefixes.to_vec());
            }
            Event::Eof => break,
            event => {
                let raw_event = standalone_event(event.into_owned())?;
                output.extend_from_slice(&raw_event);
            }
        }
        buffer.clear();
    }
    Ok(output)
}

fn prepare_container_output(
    raw: &[u8],
    inherited_prefixes: &[String],
    preferred: &str,
) -> Result<(Vec<u8>, String)> {
    let mut reader = Reader::from_reader(raw);
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        match reader.read_event_into(&mut buffer)? {
            event @ (Event::Start(_) | Event::Empty(_)) => {
                let empty = matches!(event, Event::Empty(_));
                let element = match event {
                    Event::Start(element) | Event::Empty(element) => element,
                    _ => unreachable!(),
                };
                let tag_end = reader.buffer_position() as usize;
                let mut start = raw[..tag_end].to_vec();
                let prefixes = word_prefixes_at(&element, inherited_prefixes)?;
                let (prefix, declare) = usable_word_prefix(&prefixes, &start, preferred);
                if declare {
                    declare_word_prefix(&mut start, before, &prefix)?;
                }
                let mut output = start;
                if empty {
                    // Opened for content: the start tag up to its close.
                    let opened = before + start_tag(&output[before..])?.attributes_end;
                    output.truncate(opened);
                    output.push(b'>');
                    let name = std::str::from_utf8(element.name().as_ref())?.to_owned();
                    output.extend_from_slice(format!("</{name}>").as_bytes());
                } else {
                    output.extend_from_slice(&raw[tag_end..]);
                }
                return Ok((output, prefix));
            }
            Event::Eof => {
                return Err(OxmlError::MissingElement(
                    "glossary property container".to_owned(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    }
}

fn usable_word_prefix(prefixes: &[String], tag: &[u8], preferred: &str) -> (String, bool) {
    if !preferred.is_empty() && prefixes.iter().any(|prefix| prefix == preferred) {
        return (preferred.to_owned(), false);
    }
    if let Some(prefix) = prefixes
        .iter()
        .find(|prefix| !prefix.is_empty() && !prefix.starts_with('\0'))
    {
        return (prefix.clone(), false);
    }
    (changed_output_prefix(&[], tag), true)
}

/// Declare `prefix` for the Word namespace in the start tag at `at` in `tag`.
fn declare_word_prefix(tag: &mut Vec<u8>, at: usize, prefix: &str) -> Result<()> {
    let insert_at = at + start_tag(&tag[at..])?.attributes_end;
    tag.splice(
        insert_at..insert_at,
        format!(r#" xmlns:{prefix}="{W_NS}""#).bytes(),
    );
    Ok(())
}

fn write_value_bytes(output: &mut Vec<u8>, name: &str, value: &str, prefix: &str) {
    let value = oxoffice_core::xml::escape_attribute_value(value);
    output.extend_from_slice(format!("<{prefix}:{name} {prefix}:val=\"{value}\"/>").as_bytes());
}

fn start_tag_has_attribute(tag: &[u8], wanted: &[u8]) -> bool {
    lexical_attribute_value_range(tag, wanted).is_some()
}

fn lexical_attribute_value_range(tag: &[u8], wanted: &[u8]) -> Option<std::ops::Range<usize>> {
    lexical_attribute_ranges(tag, wanted).map(|(_, value)| value)
}

fn lexical_attribute_ranges(
    tag: &[u8],
    wanted: &[u8],
) -> Option<(std::ops::Range<usize>, std::ops::Range<usize>)> {
    let mut cursor = tag.iter().position(|byte| byte.is_ascii_whitespace())?;
    while cursor < tag.len() {
        let whole_start = cursor;
        while tag.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if matches!(tag.get(cursor), Some(b'>' | b'/')) {
            return None;
        }
        let name_start = cursor;
        while tag
            .get(cursor)
            .is_some_and(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b'=' | b'>' | b'/'))
        {
            cursor += 1;
        }
        let name = &tag[name_start..cursor];
        while tag.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if tag.get(cursor) != Some(&b'=') {
            return None;
        }
        cursor += 1;
        while tag.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        let quote = *tag.get(cursor)?;
        if !matches!(quote, b'\"' | b'\'') {
            return None;
        }
        cursor += 1;
        let value_start = cursor;
        while tag.get(cursor) != Some(&quote) {
            cursor += 1;
            if cursor >= tag.len() {
                return None;
            }
        }
        let value_end = cursor;
        cursor += 1;
        if name == wanted {
            return Some((whole_start..cursor, value_start..value_end));
        }
    }
    None
}

fn property_slot(element: &BytesStart<'_>, prefixes: &[String]) -> Option<usize> {
    [
        b"name".as_slice(),
        b"style".as_slice(),
        b"category".as_slice(),
        b"types".as_slice(),
        b"behaviors".as_slice(),
        b"description".as_slice(),
        b"guid".as_slice(),
    ]
    .iter()
    .position(|local| is_word_element(element.name().as_ref(), local, prefixes))
}

fn apply_property_field(
    value: &mut CT_DocPartPr,
    slot: usize,
    raw: &[u8],
    prefixes: &[String],
) -> Result<()> {
    match slot {
        0 => value.name = root_value(raw, prefixes)?,
        1 => value.style = root_value(raw, prefixes)?,
        2 => {
            let (category, gallery) = category_values(raw, prefixes)?;
            value.category = category;
            value.gallery = gallery;
        }
        3 => value.types = nested_values(raw, b"type", prefixes)?,
        4 => value.behaviors = nested_values(raw, b"behavior", prefixes)?,
        5 => value.description = root_value(raw, prefixes)?,
        6 => {
            let guid = root_value(raw, prefixes)?;
            if guid.as_deref().is_some_and(|guid| !valid_guid(guid)) {
                return Err(OxmlError::InvalidValue(
                    "invalid w:guid lexical form".to_owned(),
                ));
            }
            value.guid = guid;
        }
        _ => {}
    }
    Ok(())
}

fn root_value(raw: &[u8], inherited_prefixes: &[String]) -> Result<Option<String>> {
    let mut reader = Reader::from_reader(raw);
    let mut buffer = Vec::new();
    let mut prefixes = inherited_prefixes.to_vec();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) | Event::Empty(element) => {
                prefixes = word_prefixes_at(&element, &prefixes)?.into_owned();
                return Ok(Some(word_value(&element, &prefixes)?.ok_or_else(|| {
                    OxmlError::MissingElement("modeled glossary w:val".to_owned())
                })?));
            }
            Event::Eof => return Ok(None),
            _ => {}
        }
        buffer.clear();
    }
}

fn nested_values(raw: &[u8], wanted: &[u8], inherited_prefixes: &[String]) -> Result<Vec<String>> {
    let mut reader = Reader::from_reader(raw);
    let mut buffer = Vec::new();
    let mut prefixes = inherited_prefixes.to_vec();
    let mut prefix_scopes = Vec::new();
    let mut depth = 0usize;
    let mut values = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) => {
                let local_prefixes = word_prefixes_at(&element, &prefixes)?;
                if depth == 1 && is_word_element(element.name().as_ref(), wanted, &local_prefixes) {
                    values.push(word_value(&element, &local_prefixes)?.ok_or_else(|| {
                        OxmlError::MissingElement("modeled glossary child w:val".to_owned())
                    })?);
                }
                depth += 1;
                let local_prefixes = local_prefixes.into_owned();
                prefix_scopes.push(std::mem::replace(&mut prefixes, local_prefixes));
            }
            Event::Empty(element) => {
                let local_prefixes = word_prefixes_at(&element, &prefixes)?;
                if depth == 1 && is_word_element(element.name().as_ref(), wanted, &local_prefixes) {
                    values.push(word_value(&element, &local_prefixes)?.ok_or_else(|| {
                        OxmlError::MissingElement("modeled glossary child w:val".to_owned())
                    })?);
                }
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                prefixes = prefix_scopes
                    .pop()
                    .unwrap_or_else(|| inherited_prefixes.to_vec());
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if values.is_empty() {
        return Err(OxmlError::MissingElement(format!(
            "modeled glossary collection requires at least one w:{} child",
            String::from_utf8_lossy(wanted)
        )));
    }
    if wanted == b"type" && values.iter().any(|value| !valid_doc_part_type(value)) {
        return Err(OxmlError::InvalidValue(
            "invalid w:type enumeration".to_owned(),
        ));
    }
    if wanted == b"behavior" && values.iter().any(|value| !valid_doc_part_behavior(value)) {
        return Err(OxmlError::InvalidValue(
            "invalid w:behavior enumeration".to_owned(),
        ));
    }
    Ok(values)
}

fn category_values(
    raw: &[u8],
    inherited_prefixes: &[String],
) -> Result<(Option<String>, Option<String>)> {
    let mut reader = Reader::from_reader(raw);
    let mut buffer = Vec::new();
    let mut prefixes = inherited_prefixes.to_vec();
    let mut prefix_scopes = Vec::new();
    let mut depth = 0usize;
    let mut last_slot = None;
    let mut values = [None, None];
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) => {
                let local_prefixes = word_prefixes_at(&element, &prefixes)?;
                validate_category_child(
                    &element,
                    &local_prefixes,
                    depth,
                    &mut last_slot,
                    &mut values,
                )?;
                depth += 1;
                let local_prefixes = local_prefixes.into_owned();
                prefix_scopes.push(std::mem::replace(&mut prefixes, local_prefixes));
            }
            Event::Empty(element) => {
                let local_prefixes = word_prefixes_at(&element, &prefixes)?;
                validate_category_child(
                    &element,
                    &local_prefixes,
                    depth,
                    &mut last_slot,
                    &mut values,
                )?;
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                prefixes = prefix_scopes
                    .pop()
                    .unwrap_or_else(|| inherited_prefixes.to_vec());
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if values.iter().any(Option::is_none) {
        return Err(OxmlError::MissingElement(
            "w:category requires direct w:name and w:gallery children".to_owned(),
        ));
    }
    if values[1]
        .as_deref()
        .is_some_and(|gallery| !valid_doc_part_gallery(gallery))
    {
        return Err(OxmlError::InvalidValue(
            "invalid w:gallery enumeration".to_owned(),
        ));
    }
    Ok((values[0].take(), values[1].take()))
}

fn valid_doc_part_behavior(value: &str) -> bool {
    matches!(value, "content" | "p" | "pg")
}

fn valid_doc_part_type(value: &str) -> bool {
    matches!(
        value,
        "none" | "normal" | "autoExp" | "toolbar" | "speller" | "formFld" | "bbPlcHdr"
    )
}

/// Whether `value` is a braced upper-case GUID, `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`.
pub fn valid_guid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 38
        && bytes.first() == Some(&b'{')
        && bytes.last() == Some(&b'}')
        && [9, 14, 19, 24]
            .into_iter()
            .all(|index| bytes.get(index) == Some(&b'-'))
        && bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 0 | 9 | 14 | 19 | 24 | 37) || matches!(byte, b'0'..=b'9' | b'A'..=b'F')
        })
}

/// Whether `value` is one of the building-block gallery values (`ST_DocPartGallery`).
pub fn valid_doc_part_gallery(value: &str) -> bool {
    matches!(
        value,
        "placeholder"
            | "any"
            | "default"
            | "docParts"
            | "coverPg"
            | "eq"
            | "ftrs"
            | "hdrs"
            | "pgNum"
            | "tbls"
            | "watermarks"
            | "autoTxt"
            | "txtBox"
            | "pgNumT"
            | "pgNumB"
            | "pgNumMargins"
            | "tblOfContents"
            | "bib"
            | "custQuickParts"
            | "custCoverPg"
            | "custEq"
            | "custFtrs"
            | "custHdrs"
            | "custPgNum"
            | "custTbls"
            | "custWatermarks"
            | "custAutoTxt"
            | "custTxtBox"
            | "custPgNumT"
            | "custPgNumB"
            | "custPgNumMargins"
            | "custTblOfContents"
            | "custBib"
            | "custom1"
            | "custom2"
            | "custom3"
            | "custom4"
            | "custom5"
    )
}

fn validate_category_child(
    element: &BytesStart<'_>,
    prefixes: &[String],
    depth: usize,
    last_slot: &mut Option<usize>,
    values: &mut [Option<String>; 2],
) -> Result<()> {
    if depth != 1 {
        return Ok(());
    }
    let slot = if is_word_element(element.name().as_ref(), b"name", prefixes) {
        Some(0)
    } else if is_word_element(element.name().as_ref(), b"gallery", prefixes) {
        Some(1)
    } else {
        None
    };
    let Some(slot) = slot else {
        return Ok(());
    };
    if values[slot].is_some() {
        return Err(OxmlError::InvalidValue(
            "duplicate modeled w:category child".to_owned(),
        ));
    }
    if last_slot.is_some_and(|last| slot < last) {
        return Err(OxmlError::InvalidValue(
            "out-of-order modeled w:category child".to_owned(),
        ));
    }
    values[slot] = Some(
        word_value(element, prefixes)?
            .ok_or_else(|| OxmlError::MissingElement("modeled glossary child w:val".to_owned()))?,
    );
    *last_slot = Some(slot);
    Ok(())
}

fn word_value(element: &BytesStart<'_>, prefixes: &[String]) -> Result<Option<String>> {
    let mut value = None;
    for attribute in element.attributes() {
        let attribute = attribute?;
        if is_word_attribute(attribute.key.as_ref(), b"val", prefixes) {
            if value.is_some() {
                return Err(OxmlError::InvalidValue(
                    "duplicate modeled glossary w:val attribute".to_owned(),
                ));
            }
            let raw = oxoffice_core::xml::normalize_attribute_white_space(std::str::from_utf8(
                &attribute.value,
            )?);
            value = Some(
                quick_xml::escape::unescape(&raw)
                    .map_err(|error| {
                        OxmlError::InvalidValue(format!(
                            "invalid modeled glossary w:val attribute: {error}"
                        ))
                    })?
                    .into_owned(),
            );
        }
    }
    Ok(value)
}

fn parse_doc_part_body(raw: &[u8], inherited_prefixes: &[String]) -> Result<CT_Body> {
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut prefixes = inherited_prefixes.to_vec();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) => {
                prefixes = word_prefixes_at(&element, &prefixes)?.into_owned();
                if is_word_element(element.name().as_ref(), b"docPartBody", &prefixes) {
                    return CT_Body::from_xml_with_prefixes_and_owner_bindings_until(
                        &mut reader,
                        &prefixes,
                        &[],
                        b"docPartBody",
                        None,
                    );
                }
            }
            Event::Empty(element) => {
                prefixes = word_prefixes_at(&element, &prefixes)?.into_owned();
                if is_word_element(element.name().as_ref(), b"docPartBody", &prefixes) {
                    return Ok(CT_Body {
                        content: Vec::new(),
                        sect_pr: None,
                    });
                }
            }
            Event::Eof => {
                return Err(OxmlError::MissingElement("w:docPartBody".to_owned()));
            }
            _ => {}
        }
        buffer.clear();
    }
}

fn serialize_doc_part_body(
    body: &CT_Body,
    source: &[u8],
    work: Option<&mut crate::text::WordOperationWork>,
) -> Result<GlossaryXml> {
    let _body_scope = crate::text::raw_root_binding_scope(source, false)?;
    let tracked = work.is_some();
    let xml = if let Some(work) = work {
        let (output, lease) = work.template_word_record_output();
        let mut writer = Writer::new(output);
        body.write_marked(
            &mut writer,
            crate::document::PartWrite::Whole,
            &mut |_| {},
            Some(lease.work),
        )?;
        let (bytes, source_map, events) = writer.into_inner().into_word_parts();
        drop(lease);
        GlossaryXml {
            bytes,
            source_map: Some(source_map),
            events,
        }
    } else {
        let mut writer = Writer::new(Vec::new());
        body.to_xml(&mut writer)?;
        GlossaryXml::generated(writer.into_inner(), false)
    };
    let wrapper = oxoffice_core::xml_element::element_span(std::str::from_utf8(&xml.bytes)?)
        .ok()
        .filter(|wrapper| wrapper.name == "w:body" && wrapper.end_tag.end == xml.bytes.len())
        .ok_or_else(|| OxmlError::MissingElement("serialized w:body wrapper".to_owned()))?;
    let (root_start, root_end) = expanded_root_wrapper(source)?;
    let mut output = GlossaryXml::generated(root_start, tracked);
    output.append_slice(&xml, wrapper.content)?;
    output.append(GlossaryXml::generated(root_end, tracked))?;
    Ok(output)
}

fn apply_glossary_edits(
    input: &[u8],
    mut edits: Vec<(Range<usize>, GlossaryXml)>,
    work: Option<&mut crate::text::WordOperationWork>,
) -> Result<GlossaryXml> {
    edits.sort_by_key(|(span, _)| (span.start, span.end));
    let replacement_bytes: usize = edits
        .iter()
        .map(|(_, replacement)| replacement.bytes.len())
        .sum();
    let removed_bytes = edits.iter().map(|(span, _)| span.len()).sum::<usize>();
    let mut output = GlossaryXml::generated(
        Vec::with_capacity(input.len() + replacement_bytes - removed_bytes),
        work.is_some(),
    );
    let mut cursor = 0usize;
    for (span, replacement) in edits {
        if span.start > span.end || span.end > input.len() || span.start < cursor {
            return Err(OxmlError::InvalidValue(
                "overlapping or invalid retained glossary source spans".to_owned(),
            ));
        }
        output.append_retained(&input[cursor..span.start], work.as_deref())?;
        output.append(replacement)?;
        cursor = span.end;
    }
    output.append_retained(&input[cursor..], work.as_deref())?;
    Ok(output)
}

fn expanded_root_wrapper(source: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut reader = Reader::from_reader(source);
    let mut buffer = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        match reader.read_event_into(&mut buffer)? {
            Event::Start(element) => {
                let end = reader.buffer_position() as usize;
                let name = std::str::from_utf8(element.name().as_ref())?.to_owned();
                return Ok((source[..end].to_vec(), format!("</{name}>").into_bytes()));
            }
            Event::Empty(element) => {
                let mut start =
                    source[..before + start_tag(&source[before..])?.attributes_end].to_vec();
                start.push(b'>');
                let name = std::str::from_utf8(element.name().as_ref())?.to_owned();
                return Ok((start, format!("</{name}>").into_bytes()));
            }
            Event::Eof => return Err(OxmlError::MissingElement("root element".to_owned())),
            _ => {}
        }
        buffer.clear();
    }
}

fn word_local_name(element: &BytesStart<'_>, prefixes: &[String]) -> Option<String> {
    let name = element.name();
    let local = name.as_ref().rsplit(|byte| *byte == b':').next()?;
    is_word_element(name.as_ref(), local, prefixes)
        .then(|| String::from_utf8_lossy(local).into_owned())
}

fn standalone_event(event: Event<'static>) -> Result<Vec<u8>> {
    let mut writer = Writer::new(Vec::new());
    if !matches!(event, Event::Decl(_) | Event::DocType(_)) {
        writer.write_event(event)?;
    }
    Ok(writer.into_inner())
}
