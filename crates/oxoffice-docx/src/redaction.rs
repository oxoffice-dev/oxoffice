use std::collections::HashSet;
use std::io::Cursor;

use oxoffice_core::xml::is_xml_1_0_character;
use oxoffice_opc::relationship::rel_types;
use oxoffice_opc::{OpcPackage, PackageReadLimits};
use quick_xml::events::{BytesDecl, BytesRef, BytesStart, BytesText, Event};
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::reader::NsReader;
use quick_xml::{Writer, XmlVersion};

use crate::{Document, Error, Result};

const WORD_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const STRICT_WORD_NS: &str = "http://purl.oclc.org/ooxml/wordprocessingml/main";
const DRAWING_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const STRICT_DRAWING_NS: &str = "http://purl.oclc.org/ooxml/drawingml/main";
const WORDPROCESSING_DRAWING_NS: &str =
    "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
const STRICT_WORDPROCESSING_DRAWING_NS: &str =
    "http://purl.oclc.org/ooxml/drawingml/wordprocessingDrawing";
const PICTURE_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/picture";
const STRICT_PICTURE_NS: &str = "http://purl.oclc.org/ooxml/drawingml/picture";
const WORDPROCESSING_SHAPE_NS: &str =
    "http://schemas.microsoft.com/office/word/2010/wordprocessingShape";
const WORDPROCESSING_GROUP_NS: &str =
    "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup";
const CHART_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
const STRICT_CHART_NS: &str = "http://purl.oclc.org/ooxml/drawingml/chart";
const SPREADSHEET_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const STRICT_SPREADSHEET_NS: &str = "http://purl.oclc.org/ooxml/spreadsheetml/main";
const CORE_PROPERTIES_NS: &str =
    "http://schemas.openxmlformats.org/package/2006/metadata/core-properties";
const DUBLIN_CORE_NS: &str = "http://purl.org/dc/elements/1.1/";
const DUBLIN_TERMS_NS: &str = "http://purl.org/dc/terms/";
const CUSTOM_PROPERTIES_NS: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/custom-properties";
const VARIANT_TYPES_NS: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes";
const MARKUP_COMPATIBILITY_NS: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";

const OUTER_LIMITS: PackageReadLimits = PackageReadLimits {
    max_entries: 4_096,
    max_part_uncompressed_bytes: 256 * 1024 * 1024,
    max_total_uncompressed_bytes: 1024 * 1024 * 1024,
};
const NESTED_LIMITS: PackageReadLimits = PackageReadLimits {
    max_entries: 1_024,
    max_part_uncompressed_bytes: 64 * 1024 * 1024,
    max_total_uncompressed_bytes: 256 * 1024 * 1024,
};

/// Counts of exact literal replacements made by [`Document::redact_text`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RedactionReport {
    /// WordprocessingML stories, comments, revisions, notes, and drawing text.
    pub word_text: usize,
    /// Core and custom document property values.
    pub metadata: usize,
    /// DrawingML chart labels and cached values.
    pub chart_caches: usize,
    /// Shared, inline, and direct cell values in embedded workbooks.
    pub embedded_workbooks: usize,
}

impl RedactionReport {
    /// Total exact literal replacements across every redacted surface.
    pub fn total(&self) -> usize {
        self.word_text
            .saturating_add(self.metadata)
            .saturating_add(self.chart_caches)
            .saturating_add(self.embedded_workbooks)
    }
}

fn add_replacements(total: &mut usize, replacements: usize) {
    *total = total.saturating_add(replacements);
}

#[derive(Clone, Copy)]
enum XmlSurface {
    Word,
    CoreProperties,
    CustomProperties,
    Chart,
    Workbook,
}

#[derive(Clone)]
struct ElementContext {
    namespace: Option<Vec<u8>>,
    local_name: Vec<u8>,
    shared_string_cell: bool,
}

impl Document {
    /// Remove every exact occurrence of one non-empty literal from sensitive
    /// Word and related package surfaces.
    ///
    /// The operation is native-only and atomic. It stages the complete
    /// document, reopens and validates the candidate package, and scans every
    /// inflated outer and embedded-workbook entry before replacing live state.
    pub fn redact_text(&mut self, selector: &str) -> Result<RedactionReport> {
        if selector.is_empty() {
            return Err(Error::Other(
                "redaction selector must not be empty".to_owned(),
            ));
        }

        let mut candidate = self.clone_for_staging();
        let mut report = RedactionReport::default();
        candidate.prepare_staged_package()?;
        redact_package(&mut candidate.package, selector, &mut report)?;
        validate_package_integrity(&candidate.package)?;

        let bytes = package_bytes(&candidate.package)?;
        scan_serialized_package(&bytes, selector, OUTER_LIMITS, 0)?;
        let reopened = candidate.reopen_prepared_staged_with_limits(OUTER_LIMITS)?;
        validate_package_integrity(&reopened.package)?;
        self.commit_staged_mutation(reopened);
        Ok(report)
    }
}

fn redact_package(
    package: &mut OpcPackage,
    selector: &str,
    report: &mut RedactionReport,
) -> Result<()> {
    let word_parts = word_story_parts(package)?;
    for part_name in word_parts {
        let Some(xml) = package.get_part(&part_name).map(<[u8]>::to_vec) else {
            continue;
        };
        let (rewritten, replacements) = rewrite_sensitive_xml(&xml, selector, XmlSurface::Word)?;
        if replacements > 0 {
            package.set_part(&part_name, rewritten);
            add_replacements(&mut report.word_text, replacements);
        }
    }

    for (relationship_type, surface) in [
        (rel_types::CORE_PROPERTIES, XmlSurface::CoreProperties),
        (rel_types::CUSTOM_PROPERTIES, XmlSurface::CustomProperties),
    ] {
        let targets = package
            .package_rels
            .get_all_by_type(relationship_type)
            .into_iter()
            .filter(|relationship| crate::document::relationship_is_internal(relationship))
            .map(|relationship| OpcPackage::resolve_rel_target("/", &relationship.target))
            .collect::<Vec<_>>();
        for part_name in targets {
            let xml = package
                .get_part(&part_name)
                .ok_or_else(|| Error::Other(format!("missing property part {part_name}")))?
                .to_vec();
            let (rewritten, replacements) = rewrite_sensitive_xml(&xml, selector, surface)?;
            if replacements > 0 {
                package.set_part(&part_name, rewritten);
                add_replacements(&mut report.metadata, replacements);
            }
        }
    }

    let chart_parts = chart_parts(package)?;
    let mut workbook_parts = HashSet::new();
    for chart_part in chart_parts {
        let xml = package
            .get_part(&chart_part)
            .ok_or_else(|| Error::Other(format!("missing chart part {chart_part}")))?
            .to_vec();
        let (rewritten, replacements) = rewrite_sensitive_xml(&xml, selector, XmlSurface::Chart)?;
        if replacements > 0 {
            package.set_part(&chart_part, rewritten);
            add_replacements(&mut report.chart_caches, replacements);
        }

        if let Some(relationships) = package.get_part_rels(&chart_part) {
            for relationship in relationships.get_all_by_type(rel_types::PACKAGE) {
                if !crate::document::relationship_is_internal(relationship) {
                    continue;
                }
                workbook_parts.insert(OpcPackage::resolve_rel_target(
                    &chart_part,
                    &relationship.target,
                ));
            }
        }
    }

    for workbook_part in workbook_parts {
        let workbook_bytes = package
            .get_part(&workbook_part)
            .ok_or_else(|| Error::Other(format!("missing embedded workbook {workbook_part}")))?;
        let (rewritten, replacements) = redact_workbook(workbook_bytes, selector)?;
        if replacements > 0 {
            package.set_part(&workbook_part, rewritten);
            add_replacements(&mut report.embedded_workbooks, replacements);
        }
    }
    Ok(())
}

fn word_story_parts(package: &OpcPackage) -> Result<Vec<String>> {
    let document_part = package.main_document_part().ok_or(Error::NoDocumentPart)?;
    let mut parts = vec![document_part.clone()];
    let mut seen = HashSet::from([document_part.clone()]);
    if let Some(relationships) = package.get_part_rels(&document_part) {
        for relationship in &relationships.items {
            if !matches!(
                relationship.rel_type.as_str(),
                rel_types::HEADER
                    | rel_types::FOOTER
                    | rel_types::FOOTNOTES
                    | rel_types::ENDNOTES
                    | rel_types::COMMENTS
            ) {
                continue;
            }
            if !crate::document::relationship_is_internal(relationship) {
                continue;
            }
            let target = OpcPackage::resolve_rel_target(&document_part, &relationship.target);
            if seen.insert(target.clone()) {
                parts.push(target);
            }
        }
    }
    Ok(parts)
}

fn chart_parts(package: &OpcPackage) -> Result<Vec<String>> {
    let mut parts = HashSet::new();
    for (source, relationships) in &package.part_rels {
        for relationship in relationships.get_all_by_type(rel_types::CHART) {
            if !crate::document::relationship_is_internal(relationship) {
                continue;
            }
            parts.insert(OpcPackage::resolve_rel_target(source, &relationship.target));
        }
    }
    let mut parts = parts.into_iter().collect::<Vec<_>>();
    parts.sort_unstable();
    Ok(parts)
}

fn redact_workbook(bytes: &[u8], selector: &str) -> Result<(Vec<u8>, usize)> {
    let mut workbook = OpcPackage::from_reader_with_limits(Cursor::new(bytes), NESTED_LIMITS)
        .map_err(|error| Error::Other(format!("invalid embedded workbook: {error}")))?;
    validate_package_integrity(&workbook)?;
    let part_names = workbook
        .parts
        .keys()
        .filter(|part_name| {
            matches!(
                workbook.content_types.content_type_for(part_name),
                Some(
                    oxoffice_opc::content_types::SHARED_STRINGS
                        | oxoffice_opc::content_types::WORKSHEET
                )
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut count = 0;
    for part_name in part_names {
        let xml = workbook
            .get_part(&part_name)
            .ok_or_else(|| Error::Other(format!("missing workbook XML part {part_name}")))?
            .to_vec();
        let (rewritten, replacements) =
            rewrite_sensitive_xml(&xml, selector, XmlSurface::Workbook)?;
        if replacements > 0 {
            workbook.set_part(&part_name, rewritten);
            count += replacements;
        }
    }
    validate_package_integrity(&workbook)?;
    let rewritten = package_bytes(&workbook)?;
    scan_serialized_package(&rewritten, selector, NESTED_LIMITS, 1)?;
    Ok((rewritten, count))
}

fn rewrite_sensitive_xml(
    xml: &[u8],
    selector: &str,
    surface: XmlSurface,
) -> Result<(Vec<u8>, usize)> {
    if let Some(little_endian) = match xml.get(..2) {
        Some([0xff, 0xfe]) => Some(true),
        Some([0xfe, 0xff]) => Some(false),
        _ => None,
    } {
        return rewrite_utf16_sensitive_xml(xml, selector, surface, little_endian);
    }

    rewrite_utf8_sensitive_xml(xml, selector, surface)
}

fn rewrite_utf16_sensitive_xml(
    xml: &[u8],
    selector: &str,
    surface: XmlSurface,
    little_endian: bool,
) -> Result<(Vec<u8>, usize)> {
    let encoded = xml
        .get(2..)
        .ok_or_else(|| Error::Other("invalid UTF-16 sensitive XML byte order mark".to_owned()))?;
    if encoded.len() % 2 != 0 {
        return Err(Error::Other(
            "invalid UTF-16 sensitive XML byte length".to_owned(),
        ));
    }
    let code_units = encoded
        .chunks_exact(2)
        .map(|bytes| {
            if little_endian {
                u16::from_le_bytes([bytes[0], bytes[1]])
            } else {
                u16::from_be_bytes([bytes[0], bytes[1]])
            }
        })
        .collect::<Vec<_>>();
    let decoded = String::from_utf16(&code_units)
        .map_err(|error| Error::Other(format!("invalid UTF-16 sensitive XML: {error}")))?;
    let (declaration, body) = if decoded.starts_with("<?xml") {
        let declaration_end = decoded
            .find("?>")
            .ok_or_else(|| Error::Other("invalid UTF-16 sensitive XML declaration".to_owned()))?
            + 2;
        decoded.split_at(declaration_end)
    } else {
        ("", decoded.as_str())
    };
    if !declaration.is_empty() {
        validate_declaration_document(
            declaration.as_bytes(),
            if little_endian {
                XmlEncoding::Utf16Le
            } else {
                XmlEncoding::Utf16Be
            },
        )?;
    }
    let (rewritten, replacements) = rewrite_utf8_sensitive_xml(body.as_bytes(), selector, surface)?;
    let rewritten = String::from_utf8(rewritten)
        .map_err(|error| Error::Other(format!("invalid rewritten UTF-16 XML: {error}")))?;
    let combined = format!("{declaration}{rewritten}");
    let mut output = Vec::with_capacity(xml.len());
    if little_endian {
        output.extend_from_slice(&[0xff, 0xfe]);
    } else {
        output.extend_from_slice(&[0xfe, 0xff]);
    }
    for code_unit in combined.encode_utf16() {
        let bytes = if little_endian {
            code_unit.to_le_bytes()
        } else {
            code_unit.to_be_bytes()
        };
        output.extend_from_slice(&bytes);
    }
    Ok((output, replacements))
}

fn rewrite_utf8_sensitive_xml(
    xml: &[u8],
    selector: &str,
    surface: XmlSurface,
) -> Result<(Vec<u8>, usize)> {
    let (cross_run_xml, mut replacements) = match surface {
        XmlSurface::Word => {
            let mut rewritten = xml.to_vec();
            let mut count = 0usize;
            loop {
                let (accepted, accepted_count) = rewrite_cross_node_text(
                    &rewritten,
                    selector,
                    surface,
                    TextProjection::Accepted,
                )?;
                let (rejected, rejected_count) = rewrite_cross_node_text(
                    &accepted,
                    selector,
                    surface,
                    TextProjection::Rejected,
                )?;
                let pass_count = accepted_count.saturating_add(rejected_count);
                count = count.saturating_add(pass_count);
                rewritten = rejected;
                if pass_count == 0 {
                    break;
                }
            }
            (rewritten, count)
        }
        XmlSurface::CoreProperties
        | XmlSurface::CustomProperties
        | XmlSurface::Chart
        | XmlSurface::Workbook => {
            rewrite_cross_node_text(xml, selector, surface, TextProjection::All)?
        }
    };
    let xml = cross_run_xml.as_slice();
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut output = Vec::with_capacity(xml.len());
    let mut stack = Vec::<ElementContext>::new();
    let mut version = XmlVersion::Implicit1_0;

    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid sensitive XML: {error}")))?;
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                let namespace = reader.resolver().resolve_element(element.name()).0;
                let namespace = resolved_namespace(&namespace)?;
                let context = element_context(&reader, namespace, &element)?;
                let (serialized, count) = rewrite_sensitive_attributes(
                    &reader,
                    &element,
                    &xml[before..after],
                    selector,
                    surface,
                    &context,
                )?;
                if count == 0 {
                    output.extend_from_slice(&xml[before..after]);
                } else {
                    output.extend_from_slice(&serialized);
                    replacements += count;
                }
                stack.push(context);
            }
            Event::Empty(element) => {
                let namespace = reader.resolver().resolve_element(element.name()).0;
                let namespace = resolved_namespace(&namespace)?;
                let context = element_context(&reader, namespace, &element)?;
                let (serialized, count) = rewrite_sensitive_attributes(
                    &reader,
                    &element,
                    &xml[before..after],
                    selector,
                    surface,
                    &context,
                )?;
                if count == 0 {
                    output.extend_from_slice(&xml[before..after]);
                } else {
                    output.extend_from_slice(&serialized);
                    replacements += count;
                }
            }
            Event::Text(text) if text_is_sensitive(surface, &stack) => {
                let decoded = text.xml_content(version).map_err(|error| {
                    Error::Other(format!("invalid sensitive XML text: {error}"))
                })?;
                let (value, count) = redact_string_to_fixed_point(&decoded, selector);
                if count == 0 {
                    output.extend_from_slice(&xml[before..after]);
                } else {
                    let mut writer = Writer::new(Vec::new());
                    writer.write_event(Event::Text(BytesText::new(&value)))?;
                    output.extend_from_slice(&writer.into_inner());
                    replacements += count;
                }
            }
            Event::CData(text) if text_is_sensitive(surface, &stack) => {
                let decoded = text
                    .xml_content(version)
                    .map_err(|error| Error::Other(format!("invalid sensitive CDATA: {error}")))?;
                let (value, count) = redact_string_to_fixed_point(&decoded, selector);
                if count == 0 {
                    output.extend_from_slice(&xml[before..after]);
                } else {
                    let mut writer = Writer::new(Vec::new());
                    writer.write_event(Event::Text(BytesText::new(&value)))?;
                    output.extend_from_slice(&writer.into_inner());
                    replacements += count;
                }
            }
            Event::GeneralRef(reference) if text_is_sensitive(surface, &stack) => {
                let decoded = decode_xml_reference(&reference, version)?;
                let (value, count) = redact_string_to_fixed_point(&decoded, selector);
                if count == 0 {
                    output.extend_from_slice(&xml[before..after]);
                } else {
                    let mut writer = Writer::new(Vec::new());
                    writer.write_event(Event::Text(BytesText::new(&value)))?;
                    output.extend_from_slice(&writer.into_inner());
                    replacements = replacements.saturating_add(count);
                }
            }
            Event::Decl(declaration) => {
                version = declaration.xml_version().map_err(|error| {
                    Error::Other(format!("invalid sensitive XML declaration: {error}"))
                })?;
                output.extend_from_slice(&xml[before..after]);
            }
            Event::End(_) => {
                output.extend_from_slice(&xml[before..after]);
                if stack.pop().is_none() {
                    return Err(Error::Other(
                        "sensitive XML has an unmatched end element".to_owned(),
                    ));
                }
            }
            Event::Eof => {
                if !stack.is_empty() {
                    return Err(Error::Other(
                        "sensitive XML has an unclosed element".to_owned(),
                    ));
                }
                break;
            }
            _ => output.extend_from_slice(&xml[before..after]),
        }
        buffer.clear();
    }
    validate_rewritten_xml(&output)?;
    Ok((output, replacements))
}

struct TextSpan {
    start: usize,
    end: usize,
    value: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TextFlow {
    WordText,
    WordInstruction,
    ChartText,
    WorkbookText,
    SensitiveValue,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TextProjection {
    All,
    Accepted,
    Rejected,
}

fn rewrite_cross_node_text(
    xml: &[u8],
    selector: &str,
    surface: XmlSurface,
    projection: TextProjection,
) -> Result<(Vec<u8>, usize)> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut stack = Vec::<ElementContext>::new();
    let mut spans = Vec::<TextSpan>::new();
    let mut edits = Vec::<TextSpan>::new();
    let mut flow = None;
    let mut replacements = 0usize;
    let mut version = XmlVersion::Implicit1_0;

    loop {
        let before = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid sensitive XML: {error}")))?;
        let after = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                let namespace = reader.resolver().resolve_element(element.name()).0;
                let namespace = resolved_namespace(&namespace)?;
                let context = element_context(&reader, namespace, &element)?;
                if !projection_hidden_by_ancestor(surface, projection, &stack)
                    && cross_node_boundary(surface, projection, &context, false)
                {
                    replacements += flush_text_spans(&mut spans, selector, &mut edits);
                    flow = None;
                }
                stack.push(context);
            }
            Event::Empty(element) => {
                let namespace = reader.resolver().resolve_element(element.name()).0;
                let namespace = resolved_namespace(&namespace)?;
                let context = element_context(&reader, namespace, &element)?;
                if !projection_hidden_by_ancestor(surface, projection, &stack)
                    && cross_node_boundary(surface, projection, &context, true)
                {
                    replacements += flush_text_spans(&mut spans, selector, &mut edits);
                    flow = None;
                }
            }
            Event::Text(text) => {
                let next_flow = cross_node_flow(surface, projection, &stack);
                if next_flow.is_some() && next_flow != flow {
                    replacements += flush_text_spans(&mut spans, selector, &mut edits);
                }
                if let Some(next_flow) = next_flow {
                    flow = Some(next_flow);
                    let decoded = text.xml_content(version).map_err(|error| {
                        Error::Other(format!("invalid sensitive XML text: {error}"))
                    })?;
                    spans.push(TextSpan {
                        start: before,
                        end: after,
                        value: decoded.into_owned(),
                    });
                }
            }
            Event::CData(text) => {
                let next_flow = cross_node_flow(surface, projection, &stack);
                if next_flow.is_some() && next_flow != flow {
                    replacements += flush_text_spans(&mut spans, selector, &mut edits);
                }
                if let Some(next_flow) = next_flow {
                    flow = Some(next_flow);
                    let decoded = text.xml_content(version).map_err(|error| {
                        Error::Other(format!("invalid sensitive XML CDATA: {error}"))
                    })?;
                    spans.push(TextSpan {
                        start: before,
                        end: after,
                        value: decoded.into_owned(),
                    });
                }
            }
            Event::GeneralRef(reference) => {
                let next_flow = cross_node_flow(surface, projection, &stack);
                if next_flow.is_some() && next_flow != flow {
                    replacements += flush_text_spans(&mut spans, selector, &mut edits);
                }
                if let Some(next_flow) = next_flow {
                    flow = Some(next_flow);
                    spans.push(TextSpan {
                        start: before,
                        end: after,
                        value: decode_xml_reference(&reference, version)?,
                    });
                }
            }
            Event::Decl(declaration) => {
                version = declaration.xml_version().map_err(|error| {
                    Error::Other(format!("invalid sensitive XML declaration: {error}"))
                })?;
            }
            Event::End(_) => {
                let Some(context) = stack.last() else {
                    return Err(Error::Other(
                        "sensitive XML has an unmatched end element".to_owned(),
                    ));
                };
                if !projection_hidden_by_ancestor(surface, projection, &stack)
                    && cross_node_boundary(surface, projection, context, false)
                {
                    replacements += flush_text_spans(&mut spans, selector, &mut edits);
                    flow = None;
                }
                stack.pop();
            }
            Event::Eof => {
                if !stack.is_empty() {
                    return Err(Error::Other(
                        "sensitive XML has an unclosed element".to_owned(),
                    ));
                }
                replacements += flush_text_spans(&mut spans, selector, &mut edits);
                break;
            }
            _ => {}
        }
        buffer.clear();
    }

    if edits.is_empty() {
        return Ok((xml.to_vec(), replacements));
    }
    let mut output = xml.to_vec();
    for edit in edits.into_iter().rev() {
        let escaped = quick_xml::escape::escape(&edit.value);
        output.splice(edit.start..edit.end, escaped.as_bytes().iter().copied());
    }
    Ok((output, replacements))
}

fn cross_node_flow(
    surface: XmlSurface,
    projection: TextProjection,
    stack: &[ElementContext],
) -> Option<TextFlow> {
    let current = stack.last()?;
    match surface {
        XmlSurface::Word
            if stack.iter().any(|element| {
                namespace_matches(element.namespace.as_deref(), WORD_NS, STRICT_WORD_NS)
                    && element.local_name == b"p"
            }) && !projection_hidden_by_ancestor(surface, projection, stack)
                && namespace_matches(current.namespace.as_deref(), WORD_NS, STRICT_WORD_NS) =>
        {
            match current.local_name.as_slice() {
                b"t" | b"delText" => Some(TextFlow::WordText),
                b"instrText" | b"delInstrText" => Some(TextFlow::WordInstruction),
                _ => None,
            }
        }
        XmlSurface::Chart
            if stack.iter().any(|element| {
                namespace_matches(element.namespace.as_deref(), DRAWING_NS, STRICT_DRAWING_NS)
                    && element.local_name == b"p"
            }) && namespace_matches(
                current.namespace.as_deref(),
                DRAWING_NS,
                STRICT_DRAWING_NS,
            ) && current.local_name == b"t" =>
        {
            Some(TextFlow::ChartText)
        }
        XmlSurface::Workbook
            if stack.iter().any(|element| {
                namespace_matches(
                    element.namespace.as_deref(),
                    SPREADSHEET_NS,
                    STRICT_SPREADSHEET_NS,
                ) && matches!(element.local_name.as_slice(), b"si" | b"is")
            }) && namespace_matches(
                current.namespace.as_deref(),
                SPREADSHEET_NS,
                STRICT_SPREADSHEET_NS,
            ) && current.local_name == b"t" =>
        {
            Some(TextFlow::WorkbookText)
        }
        _ if !matches!(surface, XmlSurface::Word) && text_is_sensitive(surface, stack) => {
            Some(TextFlow::SensitiveValue)
        }
        _ => None,
    }
}

fn projection_hidden_by_ancestor(
    surface: XmlSurface,
    projection: TextProjection,
    stack: &[ElementContext],
) -> bool {
    matches!(surface, XmlSurface::Word)
        && stack.iter().any(|element| {
            namespace_matches(element.namespace.as_deref(), WORD_NS, STRICT_WORD_NS)
                && word_revision_visibility(&element.local_name, projection) == Some(false)
        })
}

fn cross_node_boundary(
    surface: XmlSurface,
    projection: TextProjection,
    element: &ElementContext,
    _empty: bool,
) -> bool {
    if element.namespace.as_deref() == Some(MARKUP_COMPATIBILITY_NS.as_bytes())
        && matches!(
            element.local_name.as_slice(),
            b"AlternateContent" | b"Choice" | b"Fallback"
        )
    {
        return true;
    }
    match surface {
        XmlSurface::Word
            if namespace_matches(element.namespace.as_deref(), WORD_NS, STRICT_WORD_NS) =>
        {
            element.local_name == b"p"
                || (is_word_revision_container(&element.local_name)
                    && word_revision_visibility(&element.local_name, projection).is_none())
                || matches!(
                    element.local_name.as_slice(),
                    b"br"
                        | b"cr"
                        | b"tab"
                        | b"ptab"
                        | b"sym"
                        | b"fldChar"
                        | b"noBreakHyphen"
                        | b"softHyphen"
                        | b"lastRenderedPageBreak"
                        | b"drawing"
                        | b"object"
                        | b"pict"
                        | b"footnoteReference"
                        | b"endnoteReference"
                        | b"footnoteRef"
                        | b"endnoteRef"
                        | b"annotationRef"
                        | b"pgNum"
                        | b"dayShort"
                        | b"monthShort"
                        | b"yearShort"
                        | b"dayLong"
                        | b"monthLong"
                        | b"yearLong"
                        | b"commentReference"
                        | b"separator"
                        | b"continuationSeparator"
                        | b"contentPart"
                        | b"ruby"
                        | b"rt"
                        | b"rubyBase"
                        | b"control"
                )
        }
        XmlSurface::Chart
            if namespace_matches(element.namespace.as_deref(), DRAWING_NS, STRICT_DRAWING_NS) =>
        {
            element.local_name == b"p" || element.local_name == b"br"
        }
        XmlSurface::Chart
            if namespace_matches(element.namespace.as_deref(), CHART_NS, STRICT_CHART_NS) =>
        {
            element.local_name == b"v"
        }
        XmlSurface::Workbook
            if namespace_matches(
                element.namespace.as_deref(),
                SPREADSHEET_NS,
                STRICT_SPREADSHEET_NS,
            ) =>
        {
            matches!(element.local_name.as_slice(), b"si" | b"is" | b"rPh" | b"v")
        }
        XmlSurface::CoreProperties => {
            matches!(
                element.namespace.as_deref(),
                Some(namespace)
                    if (namespace == CORE_PROPERTIES_NS.as_bytes()
                        && element.local_name != b"coreProperties")
                        || namespace == DUBLIN_CORE_NS.as_bytes()
                        || namespace == DUBLIN_TERMS_NS.as_bytes()
            )
        }
        XmlSurface::CustomProperties => {
            element.namespace.as_deref() == Some(VARIANT_TYPES_NS.as_bytes())
                && matches!(
                    element.local_name.as_slice(),
                    b"lpstr" | b"lpwstr" | b"bstr"
                )
        }
        _ => false,
    }
}

fn word_revision_visibility(local_name: &[u8], projection: TextProjection) -> Option<bool> {
    match (local_name, projection) {
        (b"ins" | b"moveTo", TextProjection::Accepted) => Some(true),
        (b"del" | b"moveFrom", TextProjection::Accepted) => Some(false),
        (b"ins" | b"moveTo", TextProjection::Rejected) => Some(false),
        (b"del" | b"moveFrom", TextProjection::Rejected) => Some(true),
        (b"ins" | b"del" | b"moveTo" | b"moveFrom", TextProjection::All) => Some(true),
        _ => None,
    }
}

fn is_word_revision_container(local_name: &[u8]) -> bool {
    matches!(
        local_name,
        b"ins"
            | b"del"
            | b"moveFrom"
            | b"moveTo"
            | b"rPrChange"
            | b"pPrChange"
            | b"tblPrChange"
            | b"tblPrExChange"
            | b"tblGridChange"
            | b"trPrChange"
            | b"tcPrChange"
            | b"sectPrChange"
            | b"numberingChange"
            | b"cellIns"
            | b"cellDel"
            | b"cellMerge"
    )
}

fn flush_text_spans(spans: &mut Vec<TextSpan>, selector: &str, edits: &mut Vec<TextSpan>) -> usize {
    let replacements = redact_text_spans(spans, selector, edits);
    spans.clear();
    replacements
}

fn redact_text_spans(spans: &mut [TextSpan], selector: &str, edits: &mut Vec<TextSpan>) -> usize {
    let originals = spans
        .iter()
        .map(|span| span.value.clone())
        .collect::<Vec<_>>();
    let mut replacement_count = 0usize;
    loop {
        let combined = spans
            .iter()
            .map(|span| span.value.as_str())
            .collect::<String>();
        let matches = combined
            .match_indices(selector)
            .map(|(start, value)| {
                let start = combined[..start].chars().count();
                (start, start + value.chars().count())
            })
            .collect::<Vec<_>>();
        if matches.is_empty() {
            break;
        }
        replacement_count = replacement_count.saturating_add(matches.len());

        let mut char_start = 0usize;
        for span in &mut *spans {
            let char_end = char_start + span.value.chars().count();
            let mut removals = matches
                .iter()
                .filter_map(|(match_start, match_end)| {
                    let start = (*match_start).max(char_start);
                    let end = (*match_end).min(char_end);
                    if start < end {
                        Some((start - char_start, end - char_start))
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            for (start, end) in removals.drain(..).rev() {
                let byte_start = char_to_byte(&span.value, start);
                let byte_end = char_to_byte(&span.value, end);
                span.value.replace_range(byte_start..byte_end, "");
            }
            char_start = char_end;
        }
    }

    for (span, original) in spans.iter().zip(originals) {
        if span.value != original {
            edits.push(TextSpan {
                start: span.start,
                end: span.end,
                value: span.value.clone(),
            });
        }
    }
    replacement_count
}

fn char_to_byte(value: &str, index: usize) -> usize {
    value
        .char_indices()
        .nth(index)
        .map(|(offset, _)| offset)
        .unwrap_or(value.len())
}

fn decode_xml_reference(reference: &BytesRef<'_>, version: XmlVersion) -> Result<String> {
    if let Some(character) = reference
        .resolve_char_ref()
        .map_err(|error| Error::Other(format!("invalid sensitive XML reference: {error}")))?
    {
        validate_xml_characters(&character.to_string(), version)?;
        return Ok(character.to_string());
    }
    let name = reference
        .decode()
        .map_err(|error| Error::Other(format!("invalid sensitive XML reference: {error}")))?;
    match name.as_ref() {
        "lt" => Ok("<".to_owned()),
        "gt" => Ok(">".to_owned()),
        "amp" => Ok("&".to_owned()),
        "apos" => Ok("'".to_owned()),
        "quot" => Ok("\"".to_owned()),
        _ => Err(Error::Other(format!(
            "sensitive XML has an undefined entity {name}"
        ))),
    }
}

fn element_context(
    reader: &NsReader<&[u8]>,
    namespace: Option<Vec<u8>>,
    element: &BytesStart<'_>,
) -> Result<ElementContext> {
    let local_name = local_name(element.name().as_ref()).to_vec();
    let shared_string_cell =
        namespace_matches(namespace.as_deref(), SPREADSHEET_NS, STRICT_SPREADSHEET_NS)
            && local_name == b"c"
            && element.attributes().try_fold(false, |shared, attribute| {
                let attribute = attribute.map_err(|error| {
                    Error::Other(format!("invalid sensitive XML attribute: {error}"))
                })?;
                let (attribute_namespace, attribute_local) =
                    reader.resolver().resolve_attribute(attribute.key);
                let attribute_namespace = resolved_namespace(&attribute_namespace)?;
                if attribute_namespace.is_none() && attribute_local.as_ref() == b"t" {
                    let value = attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                        .map_err(|error| {
                            Error::Other(format!("invalid sensitive XML attribute: {error}"))
                        })?;
                    Ok::<bool, Error>(shared || value == "s")
                } else {
                    Ok(shared)
                }
            })?;
    Ok(ElementContext {
        namespace,
        local_name,
        shared_string_cell,
    })
}

fn rewrite_sensitive_attributes(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
    raw_tag: &[u8],
    selector: &str,
    surface: XmlSurface,
    context: &ElementContext,
) -> Result<(Vec<u8>, usize)> {
    let raw_attributes = raw_attribute_spans(raw_tag)?;
    let mut edits = Vec::<(usize, usize, Vec<u8>)>::new();
    let mut replacements = 0usize;
    for (index, attribute) in element.attributes().enumerate() {
        let attribute = attribute
            .map_err(|error| Error::Other(format!("invalid sensitive XML attribute: {error}")))?;
        let raw_attribute = raw_attributes.get(index).ok_or_else(|| {
            Error::Other("sensitive XML attribute positions do not match the parser".to_owned())
        })?;
        if &raw_tag[raw_attribute.name_start..raw_attribute.name_end] != attribute.key.as_ref() {
            return Err(Error::Other(
                "sensitive XML attribute names do not match the parser".to_owned(),
            ));
        }
        let (attribute_namespace, attribute_local) =
            reader.resolver().resolve_attribute(attribute.key);
        let attribute_namespace = resolved_namespace(&attribute_namespace)?;
        let sensitive = attribute_is_sensitive(
            surface,
            context,
            attribute_namespace.as_deref(),
            attribute_local.as_ref(),
        );
        if sensitive {
            let decoded = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                .map_err(|error| {
                    Error::Other(format!("invalid sensitive XML attribute: {error}"))
                })?;
            let (rewritten, count) = redact_string_to_fixed_point(&decoded, selector);
            replacements += count;
            if count > 0 {
                edits.push((
                    raw_attribute.value_start,
                    raw_attribute.value_end,
                    escape_attribute_value(&rewritten, raw_attribute.quote),
                ));
            }
        }
    }
    if raw_attributes.len() != element.attributes().count() {
        return Err(Error::Other(
            "sensitive XML attribute positions do not match the parser".to_owned(),
        ));
    }
    if replacements == 0 {
        return Ok((Vec::new(), 0));
    }

    let mut output = raw_tag.to_vec();
    for (start, end, replacement) in edits.into_iter().rev() {
        output.splice(start..end, replacement);
    }
    Ok((output, replacements))
}

fn redact_string_to_fixed_point(value: &str, selector: &str) -> (String, usize) {
    let mut value = value.to_owned();
    let mut replacements = 0usize;
    loop {
        let count = value.matches(selector).count();
        if count == 0 {
            return (value, replacements);
        }
        replacements = replacements.saturating_add(count);
        value = value.replace(selector, "");
    }
}

struct RawAttributeSpan {
    name_start: usize,
    name_end: usize,
    value_start: usize,
    value_end: usize,
    quote: u8,
}

fn raw_attribute_spans(raw_tag: &[u8]) -> Result<Vec<RawAttributeSpan>> {
    let mut index = 1usize;
    while index < raw_tag.len()
        && !raw_tag[index].is_ascii_whitespace()
        && !matches!(raw_tag[index], b'/' | b'>')
    {
        index += 1;
    }
    let mut attributes = Vec::new();
    loop {
        while index < raw_tag.len() && raw_tag[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= raw_tag.len() || matches!(raw_tag[index], b'/' | b'>') {
            break;
        }
        let name_start = index;
        while index < raw_tag.len()
            && !raw_tag[index].is_ascii_whitespace()
            && !matches!(raw_tag[index], b'=' | b'/' | b'>')
        {
            index += 1;
        }
        let name_end = index;
        while index < raw_tag.len() && raw_tag[index].is_ascii_whitespace() {
            index += 1;
        }
        if raw_tag.get(index) != Some(&b'=') {
            return Err(Error::Other(
                "sensitive XML attribute has no equals sign".to_owned(),
            ));
        }
        index += 1;
        while index < raw_tag.len() && raw_tag[index].is_ascii_whitespace() {
            index += 1;
        }
        let quote = *raw_tag.get(index).ok_or_else(|| {
            Error::Other("sensitive XML attribute has no quoted value".to_owned())
        })?;
        if !matches!(quote, b'\'' | b'"') {
            return Err(Error::Other(
                "sensitive XML attribute value is not quoted".to_owned(),
            ));
        }
        index += 1;
        let value_start = index;
        while index < raw_tag.len() && raw_tag[index] != quote {
            index += 1;
        }
        if index >= raw_tag.len() {
            return Err(Error::Other(
                "sensitive XML attribute value is not closed".to_owned(),
            ));
        }
        attributes.push(RawAttributeSpan {
            name_start,
            name_end,
            value_start,
            value_end: index,
            quote,
        });
        index += 1;
    }
    Ok(attributes)
}

fn escape_attribute_value(value: &str, quote: u8) -> Vec<u8> {
    let mut escaped = Vec::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.extend_from_slice(b"&amp;"),
            '<' => escaped.extend_from_slice(b"&lt;"),
            '"' if quote == b'"' => escaped.extend_from_slice(b"&quot;"),
            '\'' if quote == b'\'' => escaped.extend_from_slice(b"&apos;"),
            _ => {
                let mut bytes = [0; 4];
                escaped.extend_from_slice(character.encode_utf8(&mut bytes).as_bytes());
            }
        }
    }
    escaped
}

fn text_is_sensitive(surface: XmlSurface, stack: &[ElementContext]) -> bool {
    let Some(current) = stack.last() else {
        return false;
    };
    match surface {
        XmlSurface::Word => {
            namespace_matches(current.namespace.as_deref(), WORD_NS, STRICT_WORD_NS)
                && matches!(
                    current.local_name.as_slice(),
                    b"t" | b"delText" | b"instrText" | b"delInstrText"
                )
        }
        XmlSurface::CoreProperties => matches!(
            current.namespace.as_deref(),
            Some(namespace)
                if (namespace == CORE_PROPERTIES_NS.as_bytes()
                    && current.local_name != b"coreProperties")
                    || namespace == DUBLIN_CORE_NS.as_bytes()
                    || namespace == DUBLIN_TERMS_NS.as_bytes()
        ),
        XmlSurface::CustomProperties => {
            current.namespace.as_deref() == Some(VARIANT_TYPES_NS.as_bytes())
                && matches!(
                    current.local_name.as_slice(),
                    b"lpstr" | b"lpwstr" | b"bstr"
                )
        }
        XmlSurface::Chart => {
            (namespace_matches(current.namespace.as_deref(), DRAWING_NS, STRICT_DRAWING_NS)
                && current.local_name == b"t")
                || (namespace_matches(current.namespace.as_deref(), CHART_NS, STRICT_CHART_NS)
                    && current.local_name == b"v"
                    && stack.iter().rev().any(|ancestor| {
                        namespace_matches(ancestor.namespace.as_deref(), CHART_NS, STRICT_CHART_NS)
                            && matches!(
                                ancestor.local_name.as_slice(),
                                b"strCache" | b"numCache" | b"multiLvlStrCache"
                            )
                    }))
        }
        XmlSurface::Workbook => {
            if !namespace_matches(
                current.namespace.as_deref(),
                SPREADSHEET_NS,
                STRICT_SPREADSHEET_NS,
            ) {
                return false;
            }
            if current.local_name == b"t" {
                return true;
            }
            current.local_name == b"v"
                && !stack.iter().rev().any(|ancestor| {
                    namespace_matches(
                        ancestor.namespace.as_deref(),
                        SPREADSHEET_NS,
                        STRICT_SPREADSHEET_NS,
                    ) && ancestor.local_name == b"c"
                        && ancestor.shared_string_cell
                })
        }
    }
}

fn attribute_is_sensitive(
    surface: XmlSurface,
    element: &ElementContext,
    attribute_namespace: Option<&[u8]>,
    attribute_local: &[u8],
) -> bool {
    match surface {
        XmlSurface::Word => {
            let word_element =
                namespace_matches(element.namespace.as_deref(), WORD_NS, STRICT_WORD_NS);
            let word_attribute = namespace_matches(attribute_namespace, WORD_NS, STRICT_WORD_NS);
            (word_element
                && word_attribute
                && (matches!(
                    (element.local_name.as_slice(), attribute_local),
                    (b"comment", b"author")
                        | (b"comment", b"initials")
                        | (b"docVar", b"name")
                        | (b"docVar", b"val")
                ) || (attribute_local == b"author"
                    && is_word_revision_author_element(&element.local_name))
                    || (element.local_name == b"fldSimple" && attribute_local == b"instr")))
                || (attribute_namespace.is_none()
                    && drawing_nonvisual_element(element)
                    && matches!(attribute_local, b"name" | b"descr" | b"title"))
        }
        XmlSurface::CustomProperties => {
            element.namespace.as_deref() == Some(CUSTOM_PROPERTIES_NS.as_bytes())
                && element.local_name == b"property"
                && attribute_namespace.is_none()
                && attribute_local == b"name"
        }
        XmlSurface::CoreProperties | XmlSurface::Chart | XmlSurface::Workbook => false,
    }
}

fn is_word_revision_author_element(local_name: &[u8]) -> bool {
    is_word_revision_container(local_name)
        || matches!(
            local_name,
            b"moveFromRangeStart"
                | b"moveToRangeStart"
                | b"customXmlInsRangeStart"
                | b"customXmlDelRangeStart"
                | b"customXmlMoveFromRangeStart"
                | b"customXmlMoveToRangeStart"
        )
}

fn drawing_nonvisual_element(element: &ElementContext) -> bool {
    let namespace = element.namespace.as_deref();
    (element.local_name == b"docPr"
        && namespace_matches(
            namespace,
            WORDPROCESSING_DRAWING_NS,
            STRICT_WORDPROCESSING_DRAWING_NS,
        ))
        || (element.local_name == b"cNvPr"
            && (namespace_matches(namespace, PICTURE_NS, STRICT_PICTURE_NS)
                || matches!(
                    namespace,
                    Some(uri)
                        if uri == WORDPROCESSING_SHAPE_NS.as_bytes()
                            || uri == WORDPROCESSING_GROUP_NS.as_bytes()
                )))
}

#[derive(Clone, Copy)]
enum XmlEncoding {
    Utf8,
    Utf16Le,
    Utf16Be,
}

fn validate_declaration_document(xml: &[u8], encoding: XmlEncoding) -> Result<()> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().enable_all_checks(true);
    let mut buffer = Vec::new();
    let declaration = reader
        .read_event_into(&mut buffer)
        .map_err(|error| Error::Other(format!("invalid XML declaration: {error}")))?;
    let Event::Decl(declaration) = declaration else {
        return Err(Error::Other("invalid XML declaration".to_owned()));
    };
    validate_xml_declaration(&declaration, encoding)?;
    buffer.clear();
    if !matches!(
        reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid XML declaration: {error}")))?,
        Event::Eof
    ) {
        return Err(Error::Other(
            "XML declaration contains trailing content".to_owned(),
        ));
    }
    Ok(())
}

fn validate_xml_declaration(
    declaration: &BytesDecl<'_>,
    encoding: XmlEncoding,
) -> Result<XmlVersion> {
    let version = declaration
        .xml_version()
        .map_err(|error| Error::Other(format!("invalid XML declaration version: {error}")))?;
    if version == XmlVersion::Explicit1_1 {
        return Err(Error::Other(
            "XML 1.1 sensitive parts are not supported".to_owned(),
        ));
    }
    let declaration_text = std::str::from_utf8(declaration.as_ref())
        .map_err(|error| Error::Other(format!("invalid XML declaration: {error}")))?;
    let start = BytesStart::from_content(declaration_text, 3);
    let mut position = 0usize;
    for attribute in start.attributes() {
        let attribute =
            attribute.map_err(|error| Error::Other(format!("invalid XML declaration: {error}")))?;
        let name = attribute.key.as_ref();
        let value = std::str::from_utf8(attribute.value.as_ref())
            .map_err(|error| Error::Other(format!("invalid XML declaration: {error}")))?;
        validate_xml_characters(value, version)?;
        match (position, name) {
            (0, b"version") if matches!(value, "1.0" | "1.1") => {}
            (1, b"encoding") if xml_encoding_matches(value, encoding) => {}
            (1, b"standalone") if matches!(value, "yes" | "no") => {}
            (2, b"standalone") if matches!(value, "yes" | "no") => {}
            (_, b"encoding") => {
                return Err(Error::Other(
                    "XML declaration encoding is invalid or inconsistent".to_owned(),
                ));
            }
            (_, b"standalone") => {
                return Err(Error::Other(
                    "XML declaration standalone value or order is invalid".to_owned(),
                ));
            }
            _ => {
                return Err(Error::Other(
                    "XML declaration attributes are invalid or out of order".to_owned(),
                ));
            }
        }
        position += 1;
    }
    if position == 0 {
        return Err(Error::Other("XML declaration has no version".to_owned()));
    }
    Ok(version)
}

fn xml_encoding_matches(value: &str, encoding: XmlEncoding) -> bool {
    match encoding {
        XmlEncoding::Utf8 => value.eq_ignore_ascii_case("UTF-8"),
        XmlEncoding::Utf16Le => {
            value.eq_ignore_ascii_case("UTF-16") || value.eq_ignore_ascii_case("UTF-16LE")
        }
        XmlEncoding::Utf16Be => {
            value.eq_ignore_ascii_case("UTF-16") || value.eq_ignore_ascii_case("UTF-16BE")
        }
    }
}

fn validate_xml_characters(value: &str, version: XmlVersion) -> Result<()> {
    let valid = value.chars().all(|character| match version {
        XmlVersion::Explicit1_1 => {
            let value = character as u32;
            (0x1..=0xd7ff).contains(&value)
                || (0xe000..=0xfffd).contains(&value)
                || (0x10000..=0x10ffff).contains(&value)
        }
        XmlVersion::Implicit1_0 | XmlVersion::Explicit1_0 => is_xml_1_0_character(character),
    });
    if !valid {
        return Err(Error::Other(
            "rewritten sensitive XML contains a forbidden character".to_owned(),
        ));
    }
    Ok(())
}

fn validate_xml_name(name: &[u8]) -> Result<()> {
    let name = std::str::from_utf8(name)
        .map_err(|error| Error::Other(format!("invalid XML name: {error}")))?;
    let mut characters = name.chars();
    let first = characters
        .next()
        .ok_or_else(|| Error::Other("rewritten sensitive XML has an empty name".to_owned()))?;
    if !xml_name_start_character(first) || !characters.all(xml_name_character) {
        return Err(Error::Other(format!(
            "rewritten sensitive XML has an invalid name {name}"
        )));
    }
    Ok(())
}

fn validate_xml_qname(name: &[u8]) -> Result<()> {
    let mut parts = name.split(|byte| *byte == b':');
    let first = parts.next().unwrap_or_default();
    validate_xml_name(first)?;
    if let Some(second) = parts.next() {
        validate_xml_name(second)?;
    }
    if parts.next().is_some() {
        return Err(Error::Other(
            "rewritten sensitive XML has an invalid qualified name".to_owned(),
        ));
    }
    Ok(())
}

fn xml_name_start_character(character: char) -> bool {
    matches!(character, ':' | 'A'..='Z' | '_' | 'a'..='z')
        || ('\u{c0}'..='\u{d6}').contains(&character)
        || ('\u{d8}'..='\u{f6}').contains(&character)
        || ('\u{f8}'..='\u{2ff}').contains(&character)
        || ('\u{370}'..='\u{37d}').contains(&character)
        || ('\u{37f}'..='\u{1fff}').contains(&character)
        || ('\u{200c}'..='\u{200d}').contains(&character)
        || ('\u{2070}'..='\u{218f}').contains(&character)
        || ('\u{2c00}'..='\u{2fef}').contains(&character)
        || ('\u{3001}'..='\u{d7ff}').contains(&character)
        || ('\u{f900}'..='\u{fdcf}').contains(&character)
        || ('\u{fdf0}'..='\u{fffd}').contains(&character)
        || ('\u{10000}'..='\u{effff}').contains(&character)
}

fn xml_name_character(character: char) -> bool {
    xml_name_start_character(character)
        || matches!(character, '-' | '.' | '0'..='9' | '\u{b7}')
        || ('\u{300}'..='\u{36f}').contains(&character)
        || ('\u{203f}'..='\u{2040}').contains(&character)
}

fn validate_element_start(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
    version: XmlVersion,
) -> Result<()> {
    validate_xml_qname(element.name().as_ref())?;
    let namespace = reader.resolver().resolve_element(element.name()).0;
    resolved_namespace(&namespace)?;
    let mut expanded_names = HashSet::new();
    for attribute in element.attributes() {
        let attribute = attribute
            .map_err(|error| Error::Other(format!("invalid rewritten sensitive XML: {error}")))?;
        validate_xml_qname(attribute.key.as_ref())?;
        if attribute.value.contains(&b'<') {
            return Err(Error::Other(
                "rewritten sensitive XML has a less-than sign in an attribute".to_owned(),
            ));
        }
        let value = attribute
            .decoded_and_normalized_value(version, reader.decoder())
            .map_err(|error| Error::Other(format!("invalid rewritten sensitive XML: {error}")))?;
        validate_xml_characters(&value, version)?;
        let (namespace, local_name) = reader.resolver().resolve_attribute(attribute.key);
        let namespace = resolved_namespace(&namespace)?;
        if !expanded_names.insert((namespace, local_name.as_ref().to_vec())) {
            return Err(Error::Other(
                "rewritten sensitive XML has duplicate expanded-name attributes".to_owned(),
            ));
        }
    }
    Ok(())
}

fn validate_rewritten_xml(xml: &[u8]) -> Result<()> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().enable_all_checks(true);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut roots = 0usize;
    let mut declaration_allowed = true;
    let mut seen_declaration = false;
    let mut version = XmlVersion::Implicit1_0;
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| Error::Other(format!("invalid rewritten sensitive XML: {error}")))?;
        if !matches!(&event, Event::Decl(_)) {
            declaration_allowed = false;
        }
        match event {
            Event::Decl(declaration) => {
                if !declaration_allowed || seen_declaration || depth != 0 || roots != 0 {
                    return Err(Error::Other(
                        "rewritten sensitive XML has a misplaced declaration".to_owned(),
                    ));
                }
                version = validate_xml_declaration(&declaration, XmlEncoding::Utf8)?;
                seen_declaration = true;
                declaration_allowed = false;
            }
            Event::DocType(_) => {
                return Err(Error::Other(
                    "rewritten sensitive XML document types are not supported".to_owned(),
                ));
            }
            Event::Start(element) => {
                if depth == 0 {
                    roots += 1;
                }
                depth += 1;
                validate_element_start(&reader, &element, version)?;
            }
            Event::Empty(element) => {
                if depth == 0 {
                    roots += 1;
                }
                validate_element_start(&reader, &element, version)?;
            }
            Event::End(_) => {
                depth = depth.checked_sub(1).ok_or_else(|| {
                    Error::Other("rewritten sensitive XML has an unmatched end".to_owned())
                })?;
            }
            Event::Text(text) => {
                let decoded = text.xml_content(version).map_err(|error| {
                    Error::Other(format!("invalid rewritten sensitive XML: {error}"))
                })?;
                validate_xml_characters(&decoded, version)?;
                if decoded.contains("]]>") {
                    return Err(Error::Other(
                        "rewritten sensitive XML text contains a CDATA terminator".to_owned(),
                    ));
                }
                if depth == 0 && !decoded.trim().is_empty() {
                    return Err(Error::Other(
                        "rewritten sensitive XML has text outside its root".to_owned(),
                    ));
                }
            }
            Event::CData(text) => {
                let decoded = text.xml_content(version).map_err(|error| {
                    Error::Other(format!("invalid rewritten sensitive XML: {error}"))
                })?;
                validate_xml_characters(&decoded, version)?;
                if depth == 0 {
                    return Err(Error::Other(
                        "rewritten sensitive XML has CDATA outside its root".to_owned(),
                    ));
                }
            }
            Event::Comment(text) => {
                let decoded = text.xml_content(version).map_err(|error| {
                    Error::Other(format!("invalid rewritten sensitive XML: {error}"))
                })?;
                validate_xml_characters(&decoded, version)?;
            }
            Event::PI(instruction) => {
                validate_xml_name(instruction.target())?;
                if instruction.target().eq_ignore_ascii_case(b"xml") {
                    return Err(Error::Other(
                        "rewritten sensitive XML has a reserved processing instruction".to_owned(),
                    ));
                }
                let decoded = std::str::from_utf8(instruction.as_ref()).map_err(|error| {
                    Error::Other(format!("invalid rewritten sensitive XML: {error}"))
                })?;
                validate_xml_characters(decoded, version)?;
            }
            Event::GeneralRef(reference) => {
                if let Some(character) = reference.resolve_char_ref().map_err(|error| {
                    Error::Other(format!("invalid rewritten sensitive XML: {error}"))
                })? {
                    validate_xml_characters(&character.to_string(), version)?;
                } else {
                    let name = reference.decode().map_err(|error| {
                        Error::Other(format!("invalid rewritten sensitive XML: {error}"))
                    })?;
                    if !matches!(name.as_ref(), "lt" | "gt" | "amp" | "apos" | "quot") {
                        return Err(Error::Other(format!(
                            "rewritten sensitive XML has an undefined entity {name}"
                        )));
                    }
                }
            }
            Event::Eof => {
                if depth != 0 || roots != 1 {
                    return Err(Error::Other(
                        "rewritten sensitive XML must have exactly one root".to_owned(),
                    ));
                }
                break;
            }
        }
        buffer.clear();
    }
    Ok(())
}

fn resolved_namespace(namespace: &ResolveResult<'_>) -> Result<Option<Vec<u8>>> {
    match namespace {
        ResolveResult::Bound(Namespace(uri)) => {
            let uri = std::str::from_utf8(uri)
                .map_err(|error| Error::Other(format!("invalid XML namespace URI: {error}")))?;
            let uri = quick_xml::escape::unescape(uri)
                .map_err(|error| Error::Other(format!("invalid XML namespace URI: {error}")))?;
            validate_xml_characters(&uri, XmlVersion::Implicit1_0)?;
            Ok(Some(uri.as_bytes().to_vec()))
        }
        ResolveResult::Unbound => Ok(None),
        ResolveResult::Unknown(prefix) => Err(Error::Other(format!(
            "sensitive XML uses unbound namespace prefix {}",
            String::from_utf8_lossy(prefix)
        ))),
    }
}

fn namespace_matches(namespace: Option<&[u8]>, transitional: &str, strict: &str) -> bool {
    matches!(namespace, Some(uri) if uri == transitional.as_bytes() || uri == strict.as_bytes())
}

fn local_name(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

fn validate_package_integrity(package: &OpcPackage) -> Result<()> {
    for part_name in package.parts.keys() {
        if package.content_types.content_type_for(part_name).is_none() {
            return Err(Error::Other(format!(
                "redaction candidate has no content type for {part_name}"
            )));
        }
    }
    validate_relationship_targets(package, "/", &package.package_rels)?;
    for (source, relationships) in &package.part_rels {
        if !package.contains_part(source) {
            return Err(Error::Other(format!(
                "redaction candidate has relationships for missing part {source}"
            )));
        }
        validate_relationship_targets(package, source, relationships)?;
    }
    Ok(())
}

fn validate_relationship_targets(
    package: &OpcPackage,
    source: &str,
    relationships: &oxoffice_opc::relationship::Relationships,
) -> Result<()> {
    let mut ids = HashSet::new();
    for relationship in &relationships.items {
        if !ids.insert(relationship.id.as_str()) {
            return Err(Error::Other(format!(
                "redaction candidate has duplicate relationship id {} in {source}",
                relationship.id
            )));
        }
        if !crate::document::relationship_is_internal(relationship) {
            continue;
        }
        let target = OpcPackage::resolve_rel_target(source, &relationship.target);
        if !package.contains_part(&target) {
            return Err(Error::Other(format!(
                "redaction candidate relationship {} from {source} targets missing part {target}",
                relationship.id
            )));
        }
    }
    Ok(())
}

fn package_bytes(package: &OpcPackage) -> Result<Vec<u8>> {
    let mut output = Cursor::new(Vec::new());
    package.write_to(&mut output)?;
    Ok(output.into_inner())
}

fn scan_serialized_package(
    bytes: &[u8],
    selector: &str,
    limits: PackageReadLimits,
    depth: usize,
) -> Result<()> {
    let package =
        OpcPackage::from_reader_with_limits(Cursor::new(bytes), limits).map_err(|error| {
            Error::Other(format!("redaction post-scan package is invalid: {error}"))
        })?;
    scan_bytes(
        package.content_types.to_xml()?,
        selector,
        "[Content_Types].xml",
    )?;
    scan_bytes(package.package_rels.to_xml()?, selector, "_rels/.rels")?;
    for (source, relationships) in &package.part_rels {
        scan_bytes(
            relationships.to_xml()?,
            selector,
            &format!("relationships for {source}"),
        )?;
    }
    for (part_name, part) in &package.parts {
        scan_bytes(part.as_slice(), selector, part_name)?;
        if part.starts_with(b"PK\x03\x04") {
            if depth >= 1 {
                return Err(Error::Other(format!(
                    "redaction post-scan rejects nested ZIP depth beyond one at {part_name}"
                )));
            }
            scan_serialized_package(part, selector, NESTED_LIMITS, depth + 1)?;
        }
    }
    Ok(())
}

fn scan_bytes(bytes: impl AsRef<[u8]>, selector: &str, location: &str) -> Result<()> {
    let bytes = bytes.as_ref();
    let utf8 = selector.as_bytes();
    let utf16le = selector
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    if contains_bytes(bytes, utf8) || contains_bytes(bytes, &utf16le) {
        return Err(Error::Other(format!(
            "redaction post-scan found the selector in {location}"
        )));
    }
    Ok(())
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}
