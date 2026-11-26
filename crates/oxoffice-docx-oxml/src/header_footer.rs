//! Header and footer elements: `CT_HdrFtr`.

use oxoffice_core::xml_reader::NsReader;
use oxoffice_core::xml_reader::Reader;
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::{Writer, XmlVersion};

use crate::error::{OxmlError, Result};
use crate::namespace::{W_NS, matches_local_name};
use crate::numbering::{namespace_bindings, word_prefixes_at};
use crate::properties::is_word_element;
use crate::raw_xml::{capture_element, capture_empty};
use crate::text::{CT_P, ROOT_R_BINDING, ROOT_WP_BINDING, declare_w14_on_part_root};

const VML_NS: &str = "urn:schemas-microsoft-com:vml";
const OFFICE_NS: &str = "urn:schemas-microsoft-com:office:office";
const RELATIONSHIPS_NS: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const TEXT_WATERMARK_SHAPE_TYPE_ID: &str = "rdocx-watermark-type-text";
const IMAGE_WATERMARK_SHAPE_TYPE_ID: &str = "rdocx-watermark-type-image";
const WATERMARK_SHAPE_TYPE_PREFIX: &str = "rdocx-watermark-type-";

/// A conservative layout projection of one VML watermark shape.
#[derive(Debug, Clone, PartialEq)]
pub enum VmlWatermark {
    Text {
        text: String,
        width_pt: f64,
        height_pt: f64,
        rotation_degrees: f64,
        color: String,
        font_family: Option<String>,
        opacity: f64,
    },
    Image {
        relationship_id: String,
        width_pt: f64,
        height_pt: f64,
        rotation_degrees: f64,
        opacity: f64,
    },
}

impl VmlWatermark {
    /// Write the canonical VML subtree used by the native authoring facade.
    #[doc(hidden)]
    pub fn to_pict_xml(&self) -> Vec<u8> {
        let mut writer = Writer::new(Vec::new());
        let mut pict = BytesStart::new("w:pict");
        pict.push_attribute(("xmlns:w", W_NS));
        pict.push_attribute(("xmlns:v", VML_NS));
        pict.push_attribute(("xmlns:o", OFFICE_NS));
        pict.push_attribute(("xmlns:r", RELATIONSHIPS_NS));
        writer
            .write_event(Event::Start(pict))
            .expect("writing VML to memory cannot fail");

        let (width, height, rotation, opacity) = match self {
            Self::Text {
                width_pt,
                height_pt,
                rotation_degrees,
                opacity,
                ..
            }
            | Self::Image {
                width_pt,
                height_pt,
                rotation_degrees,
                opacity,
                ..
            } => (*width_pt, *height_pt, *rotation_degrees, *opacity),
        };
        let style = format!(
            "position:absolute;width:{}pt;height:{}pt;rotation:{};z-index:-251654144;mso-position-horizontal:center;mso-position-horizontal-relative:margin;mso-position-vertical:center;mso-position-vertical-relative:margin",
            compact_number(width),
            compact_number(height),
            compact_number(rotation)
        );
        let (shape_type_id, shape_type_number, text_path) = match self {
            Self::Text { .. } => (TEXT_WATERMARK_SHAPE_TYPE_ID, "136", true),
            Self::Image { .. } => (IMAGE_WATERMARK_SHAPE_TYPE_ID, "75", false),
        };
        let mut shape_type = BytesStart::new("v:shapetype");
        shape_type.push_attribute(("id", shape_type_id));
        shape_type.push_attribute(("coordsize", "21600,21600"));
        shape_type.push_attribute(("o:spt", shape_type_number));
        shape_type.push_attribute(("path", "m0,0l21600,0,21600,21600,0,21600xe"));
        if !text_path {
            shape_type.push_attribute(("filled", "f"));
            shape_type.push_attribute(("stroked", "f"));
        }
        writer
            .write_event(Event::Start(shape_type))
            .expect("writing VML to memory cannot fail");
        let mut shape_path = BytesStart::new("v:path");
        if text_path {
            shape_path.push_attribute(("textpathok", "t"));
        } else {
            shape_path.push_attribute(("o:extrusionok", "f"));
            shape_path.push_attribute(("o:connecttype", "rect"));
        }
        writer
            .write_event(Event::Empty(shape_path))
            .expect("writing VML to memory cannot fail");
        if text_path {
            let mut template_text_path = BytesStart::new("v:textpath");
            template_text_path.push_attribute(("on", "t"));
            template_text_path.push_attribute(("fitshape", "t"));
            writer
                .write_event(Event::Empty(template_text_path))
                .expect("writing VML to memory cannot fail");
        }
        writer
            .write_event(Event::End(BytesEnd::new("v:shapetype")))
            .expect("writing VML to memory cannot fail");

        let mut shape = BytesStart::new("v:shape");
        shape.push_attribute(("id", "rdocx-watermark"));
        shape.push_attribute(("o:spid", "_x0000_s1025"));
        shape.push_attribute((
            "type",
            match self {
                Self::Text { .. } => "#rdocx-watermark-type-text",
                Self::Image { .. } => "#rdocx-watermark-type-image",
            },
        ));
        shape.push_attribute(("style", style.as_str()));
        shape.push_attribute(("stroked", "f"));
        if let Self::Text { color, .. } = self {
            shape.push_attribute(("fillcolor", color.as_str()));
        }
        writer
            .write_event(Event::Start(shape))
            .expect("writing VML to memory cannot fail");

        let mut fill = BytesStart::new("v:fill");
        let opacity = compact_number(opacity);
        fill.push_attribute(("opacity", opacity.as_str()));
        writer
            .write_event(Event::Empty(fill))
            .expect("writing VML to memory cannot fail");

        match self {
            Self::Text {
                text, font_family, ..
            } => {
                let family = font_family.as_deref().unwrap_or("Calibri");
                let text_style = format!("font-family:\"{family}\";font-size:1pt");
                let mut textpath = BytesStart::new("v:textpath");
                textpath.push_attribute(("on", "t"));
                textpath.push_attribute(("fitshape", "t"));
                textpath.push_attribute(("style", text_style.as_str()));
                textpath.push_attribute(("string", text.as_str()));
                writer
                    .write_event(Event::Empty(textpath))
                    .expect("writing VML to memory cannot fail");
            }
            Self::Image {
                relationship_id, ..
            } => {
                let mut image = BytesStart::new("v:imagedata");
                image.push_attribute(("r:id", relationship_id.as_str()));
                image.push_attribute(("o:title", ""));
                writer
                    .write_event(Event::Empty(image))
                    .expect("writing VML to memory cannot fail");
            }
        }

        writer
            .write_event(Event::End(BytesEnd::new("v:shape")))
            .expect("writing VML to memory cannot fail");
        writer
            .write_event(Event::End(BytesEnd::new("w:pict")))
            .expect("writing VML to memory cannot fail");
        writer.into_inner()
    }
}

/// `CT_HdrFtr` — Content of a header or footer part.
///
/// Contains paragraphs (and potentially tables, same as a document body).
#[derive(Debug, Clone, PartialEq)]
pub struct CT_HdrFtr {
    pub paragraphs: Vec<CT_P>,
    /// Supported VML watermark shapes projected from the original part bytes.
    watermarks: Vec<VmlWatermark>,
    /// Extra namespace declarations captured from the root element.
    pub extra_namespaces: Vec<(String, String)>,
    /// Non-namespace attributes of the root element, such as `mc:Ignorable`,
    /// in source order.
    root_attributes: Vec<(String, String)>,
    /// Unknown child elements captured as raw XML.
    pub extra_xml: Vec<Vec<u8>>,
    /// How many paragraphs precede each entry of `extra_xml`, so that a
    /// rewrite puts a table or a content control back where it was. An entry
    /// without a position is written after the last paragraph.
    extra_xml_positions: Vec<usize>,
    /// The namespace bindings of the root element, which a raw child is
    /// parsed with when replacement reaches into it.
    pub(crate) word_prefixes: Vec<String>,
}

#[allow(non_snake_case)]
impl CT_HdrFtr {
    pub fn new() -> Self {
        CT_HdrFtr {
            paragraphs: Vec::new(),
            watermarks: Vec::new(),
            extra_namespaces: Vec::new(),
            root_attributes: Vec::new(),
            extra_xml: Vec::new(),
            extra_xml_positions: Vec::new(),
            word_prefixes: vec!["w".to_owned()],
        }
    }

    /// Get the combined text of all paragraphs.
    pub fn text(&self) -> String {
        self.paragraphs
            .iter()
            .map(|p| p.text())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Return supported watermark projections in document order.
    pub fn watermarks(&self) -> &[VmlWatermark] {
        &self.watermarks
    }

    /// Parse from XML bytes (the content of header*.xml or footer*.xml).
    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        let admitted = crate::document::admit_reader_source(
            xml,
            crate::document::WordSourceReader::Part(crate::document::PartReaderKind::HeaderFooter),
        )?;
        let mut controls = admitted.controls_for(xml);
        let watermarks = parse_vml_watermarks(xml);
        let mut reader = Reader::from_reader(xml);
        // A raw child is captured with the text events of this reader, so
        // trimming them would drop the edge spaces of its text, such as the
        // one of "Page " before a page number. The text between the children
        // of the root is skipped below.
        reader.config_mut().trim_text(false);

        let mut paragraphs = Vec::new();
        let mut extra_namespaces = Vec::new();
        let mut root_attributes = Vec::new();
        let mut extra_xml = Vec::new();
        let mut extra_xml_positions = Vec::new();
        let mut buf = Vec::new();
        let mut word_prefixes = Vec::new();

        // The declarations the root writer always writes itself; a default
        // namespace is kept, as the main part's root keeps it.
        let known_ns: &[&[u8]] = &[b"xmlns:w", b"xmlns:r"];

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, &word_prefixes)?;
                    if is_word_element(name.as_ref(), b"p", &prefixes) {
                        paragraphs.push(CT_P::from_xml_with_revision_facts(
                            &mut reader,
                            &prefixes,
                            Some(e),
                            None,
                            Some(&mut controls),
                            None,
                            None,
                        )?);
                    } else if is_word_element(name.as_ref(), b"hdr", &prefixes)
                        || is_word_element(name.as_ref(), b"ftr", &prefixes)
                    {
                        // Capture extra namespace declarations and the other
                        // attributes from root element
                        for attr in e.attributes().flatten() {
                            let key = attr.key.as_ref();
                            let is_namespace = key.starts_with(b"xmlns:") || key == b"xmlns";
                            if is_namespace && !known_ns.contains(&key) {
                                let key_str = std::str::from_utf8(key).unwrap_or("").to_string();
                                let val_str = attr
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        e.decoder(),
                                    )?
                                    .into_owned();
                                extra_namespaces.push((key_str, val_str));
                            } else if !is_namespace {
                                root_attributes.push((
                                    std::str::from_utf8(key)?.to_owned(),
                                    attr.decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        e.decoder(),
                                    )?
                                    .into_owned(),
                                ));
                            }
                        }
                        word_prefixes = prefixes.into_owned();
                    } else {
                        // Capture unknown elements as raw XML
                        extra_xml.push(capture_element(&mut reader, e)?);
                        extra_xml_positions.push(paragraphs.len());
                    }
                }
                Ok(Event::Empty(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, &word_prefixes)?;
                    if is_word_element(name.as_ref(), b"p", &prefixes) {
                        paragraphs.push(CT_P::from_empty_root(e, &prefixes)?);
                    } else if !matches_local_name(name.as_ref(), b"hdr")
                        && !matches_local_name(name.as_ref(), b"ftr")
                    {
                        extra_xml.push(capture_empty(e, reader.empty_writing())?);
                        extra_xml_positions.push(paragraphs.len());
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        Ok(CT_HdrFtr {
            paragraphs,
            watermarks,
            extra_namespaces,
            root_attributes,
            extra_xml,
            extra_xml_positions,
            word_prefixes,
        })
    }

    /// Serialize to XML bytes as a header.
    pub fn to_xml_header(&self) -> Result<Vec<u8>> {
        self.to_xml_root("w:hdr")
    }

    /// Serialize to XML bytes as a footer.
    pub fn to_xml_footer(&self) -> Result<Vec<u8>> {
        self.to_xml_root("w:ftr")
    }

    fn to_xml_root(&self, root_tag: &str) -> Result<Vec<u8>> {
        for paragraph in &self.paragraphs {
            crate::document::admit_word_model(crate::document::WordOwner::Paragraph(paragraph))?;
        }
        let wp_ns = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
        let mut writer = Writer::new(Vec::new());

        writer.write_event(Event::Decl(BytesDecl::new(
            "1.0",
            Some("UTF-8"),
            Some("yes"),
        )))?;

        let mut start = BytesStart::new(root_tag);
        start.push_attribute(("xmlns:w", W_NS));
        start.push_attribute((
            "xmlns:r",
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
        ));

        // Always emit xmlns:wp for drawing elements
        let mut has_wp = false;
        for (key, _) in &self.extra_namespaces {
            if key == "xmlns:wp" {
                has_wp = true;
                break;
            }
        }
        if !has_wp {
            start.push_attribute(("xmlns:wp", wp_ns));
        }

        // Replay captured extra namespaces, then the root attributes that
        // may name their prefixes, such as `mc:Ignorable`.
        for (key, val) in self.extra_namespaces.iter().chain(&self.root_attributes) {
            start.push_attribute((key.as_str(), val.as_str()));
        }

        let _binding_scope = crate::text::emitted_root_binding_scope(
            &start,
            ROOT_R_BINDING | ROOT_WP_BINDING,
            true,
        )?;
        writer.write_event(Event::Start(start))?;

        // Write each captured unknown element before the paragraph it
        // preceded, and the rest after the last paragraph.
        let mut raw_children = self
            .extra_xml
            .iter()
            .enumerate()
            .map(|(index, raw)| {
                let position = self.extra_xml_positions.get(index).copied();
                (position.unwrap_or(usize::MAX), raw)
            })
            .peekable();
        for (index, p) in self.paragraphs.iter().enumerate() {
            while let Some((_, raw)) = raw_children.next_if(|(position, _)| *position <= index) {
                writer.get_mut().extend_from_slice(raw);
            }
            p.to_xml(&mut writer)?;
        }
        for (_, raw) in raw_children {
            writer.get_mut().extend_from_slice(raw);
        }

        writer.write_event(Event::End(BytesEnd::new(root_tag)))?;

        let mut xml = writer.into_inner();
        declare_w14_on_part_root(&mut xml)?;
        Ok(xml)
    }
}

/// Replace the exact API-owned VML shape without reconstructing its header.
#[doc(hidden)]
pub fn replace_authored_watermark(xml: &[u8], watermark: &VmlWatermark) -> Result<Vec<u8>> {
    let mut replacement_pict = watermark.to_pict_xml();
    let replacement_shape_type_id = match watermark {
        VmlWatermark::Text { .. } => TEXT_WATERMARK_SHAPE_TYPE_ID,
        VmlWatermark::Image { .. } => IMAGE_WATERMARK_SHAPE_TYPE_ID,
    };
    if contains_vml_shapetype(xml, replacement_shape_type_id)?
        && let Some(range) = vml_shape_type_range(&replacement_pict, replacement_shape_type_id)?
    {
        replacement_pict.drain(range);
    }
    let normalized_xml =
        prepare_owned_shape_type(xml, &replacement_pict, replacement_shape_type_id)?;
    let xml = normalized_xml.as_deref().unwrap_or(xml);
    let replacement_range = api_owned_shape_ranges(&replacement_pict)?
        .into_iter()
        .next()
        .ok_or_else(|| OxmlError::MissingElement("generated watermark shape".to_owned()))?;
    let mut replacement_shape = replacement_pict[replacement_range].to_vec();

    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut pict_depth = None;
    let mut owned_start = None;
    let mut owned_ranges = Vec::new();
    let mut header_end = None;
    let mut empty_header = None;

    loop {
        let event_start = reader.buffer_position() as usize;
        let (namespace, event) = reader.read_resolved_event_into(&mut buffer)?;
        let is_word = namespace_is(&namespace, W_NS);
        let is_vml = namespace_is(&namespace, VML_NS);
        let mut completed_owner = None;
        let mut empty_owner = false;
        let mut completed_header = false;
        let mut empty_root = false;
        match event {
            Event::Start(ref element) => {
                depth += 1;
                let name = element.name();
                let local = local_name(name.as_ref());
                if is_word && local == b"pict" {
                    pict_depth = Some(depth);
                } else if pict_depth.is_some()
                    && is_vml
                    && local == b"shape"
                    && raw_unqualified_attribute(element, b"id").as_deref()
                        == Some("rdocx-watermark")
                {
                    owned_start = Some((depth, event_start));
                }
            }
            Event::Empty(ref element) => {
                let name = element.name();
                let local = local_name(name.as_ref());
                if depth == 0 && is_word && local == b"hdr" {
                    empty_root = true;
                } else if pict_depth.is_some()
                    && is_vml
                    && local == b"shape"
                    && raw_unqualified_attribute(element, b"id").as_deref()
                        == Some("rdocx-watermark")
                {
                    empty_owner = true;
                }
            }
            Event::End(ref element) => {
                let name = element.name();
                let local = local_name(name.as_ref());
                if owned_start.is_some_and(|(owner_depth, _)| owner_depth == depth)
                    && is_vml
                    && local == b"shape"
                    && let Some((_, start)) = owned_start.take()
                {
                    completed_owner = Some(start);
                }
                if pict_depth == Some(depth) && is_word && local == b"pict" {
                    pict_depth = None;
                }
                if depth == 1 && is_word && local == b"hdr" {
                    completed_header = true;
                }
                depth = depth.saturating_sub(1);
            }
            Event::Eof => break,
            _ => {}
        }
        drop(event);
        drop(namespace);
        let event_end = reader.buffer_position() as usize;
        if let Some(start) = completed_owner {
            owned_ranges.push(start..event_end);
        }
        if empty_owner {
            owned_ranges.push(event_start..event_end);
        }
        if completed_header {
            header_end = Some(event_start);
        }
        if empty_root {
            empty_header = Some(event_start..event_end);
        }
        buffer.clear();
    }

    if !owned_ranges.is_empty() {
        if !contains_vml_shapetype(xml, replacement_shape_type_id)? {
            let attribute = format!(" type=\"#{replacement_shape_type_id}\"");
            if let Some(position) = replacement_shape
                .windows(attribute.len())
                .position(|window| window == attribute.as_bytes())
            {
                replacement_shape.drain(position..position + attribute.len());
            }
        }
        replacement_shape = ensure_fixed_prefix_bindings(
            replacement_shape,
            xml,
            owned_ranges[0].start,
            matches!(watermark, VmlWatermark::Image { .. }),
        )?;
        let mut output = Vec::with_capacity(xml.len() + replacement_shape.len());
        let mut copied = 0usize;
        for (index, range) in owned_ranges.into_iter().enumerate() {
            output.extend_from_slice(&xml[copied..range.start]);
            if index == 0 {
                output.extend_from_slice(&replacement_shape);
            }
            copied = range.end;
        }
        output.extend_from_slice(&xml[copied..]);
        return Ok(output);
    }

    let mut paragraph = format!("<w:p xmlns:w=\"{W_NS}\"><w:r>").into_bytes();
    paragraph.extend_from_slice(&replacement_pict);
    paragraph.extend_from_slice(b"</w:r></w:p>");
    if let Some(position) = header_end {
        let mut output = Vec::with_capacity(xml.len() + paragraph.len());
        output.extend_from_slice(&xml[..position]);
        output.extend_from_slice(&paragraph);
        output.extend_from_slice(&xml[position..]);
        return Ok(output);
    }
    if let Some(range) = empty_header {
        let (root, _) =
            oxoffice_core::xml_element::replace_content(&xml[range.clone()], &paragraph)?;
        let mut output = Vec::with_capacity(xml.len() + paragraph.len() + root.len());
        output.extend_from_slice(&xml[..range.start]);
        output.extend_from_slice(&root);
        output.extend_from_slice(&xml[range.end..]);
        return Ok(output);
    }
    Err(OxmlError::MissingElement("header root".to_owned()))
}

fn api_owned_shape_ranges(xml: &[u8]) -> Result<Vec<std::ops::Range<usize>>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut pict_depth = None;
    let mut owned_start = None;
    let mut ranges = Vec::new();
    loop {
        let event_start = reader.buffer_position() as usize;
        let (namespace, event) = reader.read_resolved_event_into(&mut buffer)?;
        let is_word = namespace_is(&namespace, W_NS);
        let is_vml = namespace_is(&namespace, VML_NS);
        let mut completed_owner = None;
        let mut empty_owner = false;
        match event {
            Event::Start(ref element) => {
                depth += 1;
                let name = element.name();
                let local = local_name(name.as_ref());
                if is_word && local == b"pict" {
                    pict_depth = Some(depth);
                } else if pict_depth.is_some()
                    && is_vml
                    && local == b"shape"
                    && raw_unqualified_attribute(element, b"id").as_deref()
                        == Some("rdocx-watermark")
                {
                    owned_start = Some((depth, event_start));
                }
            }
            Event::Empty(ref element)
                if pict_depth.is_some()
                    && is_vml
                    && local_name(element.name().as_ref()) == b"shape"
                    && raw_unqualified_attribute(element, b"id").as_deref()
                        == Some("rdocx-watermark") =>
            {
                empty_owner = true;
            }
            Event::End(ref element) => {
                let name = element.name();
                let local = local_name(name.as_ref());
                if owned_start.is_some_and(|(owner_depth, _)| owner_depth == depth)
                    && is_vml
                    && local == b"shape"
                    && let Some((_, start)) = owned_start.take()
                {
                    completed_owner = Some(start);
                }
                if pict_depth == Some(depth) && is_word && local == b"pict" {
                    pict_depth = None;
                }
                depth = depth.saturating_sub(1);
            }
            Event::Eof => break,
            _ => {}
        }
        drop(event);
        drop(namespace);
        let event_end = reader.buffer_position() as usize;
        if let Some(start) = completed_owner {
            ranges.push(start..event_end);
        }
        if empty_owner {
            ranges.push(event_start..event_end);
        }
        buffer.clear();
    }
    Ok(ranges)
}

fn contains_vml_shapetype(xml: &[u8], expected_id: &str) -> Result<bool> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    loop {
        let (namespace, event) = reader.read_resolved_event_into(&mut buffer)?;
        match event {
            Event::Start(ref element) | Event::Empty(ref element)
                if namespace_is(&namespace, VML_NS)
                    && local_name(element.name().as_ref()) == b"shapetype"
                    && raw_unqualified_attribute(element, b"id").as_deref()
                        == Some(expected_id) =>
            {
                return Ok(true);
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
        buffer.clear();
    }
}

fn prepare_owned_shape_type(
    xml: &[u8],
    replacement_pict: &[u8],
    replacement_id: &str,
) -> Result<Option<Vec<u8>>> {
    let Some(owned_type) = api_owned_shape_type(xml)? else {
        return Ok(None);
    };
    if owned_type.as_deref() == Some(replacement_id) && contains_vml_shapetype(xml, replacement_id)?
    {
        return Ok(None);
    }
    let replacement = if contains_vml_shapetype(xml, replacement_id)? {
        None
    } else {
        let replacement_range = vml_shape_type_range(replacement_pict, replacement_id)?
            .ok_or_else(|| OxmlError::MissingElement("generated watermark shapetype".to_owned()))?;
        Some(replacement_pict[replacement_range].to_vec())
    };
    let old_definition_is_owned = owned_type
        .as_deref()
        .is_some_and(|owned_id| owned_id.starts_with(WATERMARK_SHAPE_TYPE_PREFIX));
    let old_definition_is_shared = match owned_type.as_deref() {
        Some(owned_id) => shape_type_is_used_by_producer_shape(xml, owned_id)?,
        None => false,
    };
    if !old_definition_is_owned || old_definition_is_shared {
        let Some(replacement) = replacement else {
            return Ok(None);
        };
        let insertion = api_owned_shape_ranges(xml)?
            .into_iter()
            .next()
            .ok_or_else(|| OxmlError::MissingElement("owned watermark shape".to_owned()))?
            .start;
        let replacement = ensure_fixed_prefix_bindings(replacement, xml, insertion, false)?;
        let mut output = Vec::with_capacity(xml.len() + replacement.len());
        output.extend_from_slice(&xml[..insertion]);
        output.extend_from_slice(&replacement);
        output.extend_from_slice(&xml[insertion..]);
        return Ok(Some(output));
    }
    let Some(source_range) = vml_shape_type_range(
        xml,
        owned_type.as_deref().expect("owned definition id exists"),
    )?
    else {
        let Some(replacement) = replacement else {
            return Ok(None);
        };
        let insertion = api_owned_shape_ranges(xml)?
            .into_iter()
            .next()
            .ok_or_else(|| OxmlError::MissingElement("owned watermark shape".to_owned()))?
            .start;
        let replacement = ensure_fixed_prefix_bindings(replacement, xml, insertion, false)?;
        let mut output = Vec::with_capacity(xml.len() + replacement.len());
        output.extend_from_slice(&xml[..insertion]);
        output.extend_from_slice(&replacement);
        output.extend_from_slice(&xml[insertion..]);
        return Ok(Some(output));
    };
    let replacement = replacement
        .map(|replacement| {
            ensure_fixed_prefix_bindings(replacement, xml, source_range.start, false)
        })
        .transpose()?
        .unwrap_or_default();
    let mut output = Vec::with_capacity(xml.len() + replacement.len());
    output.extend_from_slice(&xml[..source_range.start]);
    output.extend_from_slice(&replacement);
    output.extend_from_slice(&xml[source_range.end..]);
    Ok(Some(output))
}

fn ensure_fixed_prefix_bindings(
    mut fragment: Vec<u8>,
    source: &[u8],
    offset: usize,
    needs_relationships: bool,
) -> Result<Vec<u8>> {
    let bindings = ancestor_namespace_bindings(source, offset)?;
    let mut declarations = Vec::new();
    for (prefix, namespace) in [("v", VML_NS), ("o", OFFICE_NS)] {
        if !bindings.iter().any(|(bound_prefix, bound_namespace)| {
            bound_prefix == prefix && bound_namespace == namespace
        }) {
            declarations.push((prefix, namespace));
        }
    }
    if needs_relationships
        && !bindings
            .iter()
            .any(|(prefix, namespace)| prefix == "r" && namespace == RELATIONSHIPS_NS)
    {
        declarations.push(("r", RELATIONSHIPS_NS));
    }
    if declarations.is_empty() {
        return Ok(fragment);
    }
    let name_end = fragment
        .iter()
        .position(|byte| byte.is_ascii_whitespace() || matches!(*byte, b'>' | b'/'))
        .ok_or_else(|| OxmlError::InvalidValue("watermark XML fragment".to_owned()))?;
    let mut attributes = Vec::new();
    for (prefix, namespace) in declarations {
        attributes.extend_from_slice(format!(" xmlns:{prefix}=\"{namespace}\"").as_bytes());
    }
    fragment.splice(name_end..name_end, attributes);
    Ok(fragment)
}

fn ancestor_namespace_bindings(source: &[u8], offset: usize) -> Result<Vec<(String, String)>> {
    let mut reader = Reader::from_reader(source);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut scopes = vec![Vec::new()];
    loop {
        if reader.buffer_position() as usize == offset {
            return Ok(namespace_bindings(
                scopes.last().expect("namespace root scope exists"),
            ));
        }
        match reader.read_event_into(&mut buffer)? {
            Event::Start(start) => {
                let inherited = scopes.last().expect("namespace root scope exists");
                scopes.push(word_prefixes_at(&start, inherited)?.into_owned());
            }
            Event::End(_) => {
                scopes.pop().ok_or_else(|| {
                    OxmlError::InvalidValue("unbalanced watermark XML namespace scope".to_owned())
                })?;
            }
            Event::Eof => {
                return Err(OxmlError::MissingElement(
                    "watermark XML splice point".to_owned(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    }
}

fn api_owned_shape_type(xml: &[u8]) -> Result<Option<Option<String>>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut pict_depth = None;
    loop {
        let (namespace, event) = reader.read_resolved_event_into(&mut buffer)?;
        match event {
            Event::Start(ref element) => {
                depth += 1;
                if namespace_is(&namespace, W_NS) && local_name(element.name().as_ref()) == b"pict"
                {
                    pict_depth = Some(depth);
                } else if pict_depth.is_some()
                    && namespace_is(&namespace, VML_NS)
                    && local_name(element.name().as_ref()) == b"shape"
                    && raw_unqualified_attribute(element, b"id").as_deref()
                        == Some("rdocx-watermark")
                {
                    return Ok(Some(
                        raw_unqualified_attribute(element, b"type")
                            .and_then(|value| value.strip_prefix('#').map(str::to_owned)),
                    ));
                }
            }
            Event::Empty(ref element)
                if pict_depth.is_some()
                    && namespace_is(&namespace, VML_NS)
                    && local_name(element.name().as_ref()) == b"shape"
                    && raw_unqualified_attribute(element, b"id").as_deref()
                        == Some("rdocx-watermark") =>
            {
                return Ok(Some(
                    raw_unqualified_attribute(element, b"type")
                        .and_then(|value| value.strip_prefix('#').map(str::to_owned)),
                ));
            }
            Event::End(ref element) => {
                if pict_depth == Some(depth)
                    && namespace_is(&namespace, W_NS)
                    && local_name(element.name().as_ref()) == b"pict"
                {
                    pict_depth = None;
                }
                depth = depth.saturating_sub(1);
            }
            Event::Eof => return Ok(None),
            _ => {}
        }
        buffer.clear();
    }
}

fn shape_type_is_used_by_producer_shape(xml: &[u8], expected_id: &str) -> Result<bool> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut pict_depth = None;
    loop {
        let (namespace, event) = reader.read_resolved_event_into(&mut buffer)?;
        match event {
            Event::Start(ref element) => {
                depth += 1;
                if namespace_is(&namespace, W_NS) && local_name(element.name().as_ref()) == b"pict"
                {
                    pict_depth = Some(depth);
                } else if namespace_is(&namespace, VML_NS)
                    && local_name(element.name().as_ref()) == b"shape"
                    && raw_unqualified_attribute(element, b"type")
                        .as_deref()
                        .and_then(|value| value.strip_prefix('#'))
                        == Some(expected_id)
                    && !(pict_depth.is_some()
                        && raw_unqualified_attribute(element, b"id").as_deref()
                            == Some("rdocx-watermark"))
                {
                    return Ok(true);
                }
            }
            Event::Empty(ref element)
                if namespace_is(&namespace, VML_NS)
                    && local_name(element.name().as_ref()) == b"shape"
                    && raw_unqualified_attribute(element, b"type")
                        .as_deref()
                        .and_then(|value| value.strip_prefix('#'))
                        == Some(expected_id)
                    && !(pict_depth.is_some()
                        && raw_unqualified_attribute(element, b"id").as_deref()
                            == Some("rdocx-watermark")) =>
            {
                return Ok(true);
            }
            Event::End(ref element) => {
                if pict_depth == Some(depth)
                    && namespace_is(&namespace, W_NS)
                    && local_name(element.name().as_ref()) == b"pict"
                {
                    pict_depth = None;
                }
                depth = depth.saturating_sub(1);
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
        buffer.clear();
    }
}

fn vml_shape_type_range(xml: &[u8], expected_id: &str) -> Result<Option<std::ops::Range<usize>>> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    loop {
        let event_start = reader.buffer_position() as usize;
        let (namespace, event) = reader.read_resolved_event_into(&mut buffer)?;
        let mut completed = None;
        match event {
            Event::Start(ref element) => {
                depth += 1;
                if namespace_is(&namespace, VML_NS)
                    && local_name(element.name().as_ref()) == b"shapetype"
                    && raw_unqualified_attribute(element, b"id").as_deref() == Some(expected_id)
                {
                    start = Some((depth, event_start));
                }
            }
            Event::Empty(ref element)
                if namespace_is(&namespace, VML_NS)
                    && local_name(element.name().as_ref()) == b"shapetype"
                    && raw_unqualified_attribute(element, b"id").as_deref()
                        == Some(expected_id) =>
            {
                return Ok(Some(event_start..reader.buffer_position() as usize));
            }
            Event::End(ref element) => {
                if start.is_some_and(|(start_depth, _)| start_depth == depth)
                    && namespace_is(&namespace, VML_NS)
                    && local_name(element.name().as_ref()) == b"shapetype"
                    && let Some((_, range_start)) = start.take()
                {
                    completed = Some(range_start..reader.buffer_position() as usize);
                }
                depth = depth.saturating_sub(1);
            }
            Event::Eof => return Ok(None),
            _ => {}
        }
        if let Some(range) = completed {
            return Ok(Some(range));
        }
        buffer.clear();
    }
}

impl Default for CT_HdrFtr {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Default)]
struct PendingWatermark {
    width: Option<f64>,
    height: Option<f64>,
    rotation: f64,
    color: Option<String>,
    opacity: f64,
    text: Option<String>,
    font_family: Option<String>,
    relationship_id: Option<String>,
}

impl PendingWatermark {
    fn finish(self) -> Option<VmlWatermark> {
        let width_pt = self.width?;
        let height_pt = self.height?;
        if !width_pt.is_finite()
            || !height_pt.is_finite()
            || !self.rotation.is_finite()
            || !self.opacity.is_finite()
            || width_pt <= 0.0
            || height_pt <= 0.0
        {
            return None;
        }
        let opacity = self.opacity.clamp(0.0, 1.0);
        match (self.text, self.relationship_id) {
            (Some(text), None) => Some(VmlWatermark::Text {
                text,
                width_pt,
                height_pt,
                rotation_degrees: self.rotation,
                color: self.color.unwrap_or_else(|| "D9D9D9".to_owned()),
                font_family: self.font_family,
                opacity,
            }),
            (None, Some(relationship_id)) => Some(VmlWatermark::Image {
                relationship_id,
                width_pt,
                height_pt,
                rotation_degrees: self.rotation,
                opacity,
            }),
            _ => None,
        }
    }
}

// Match the existing DrawingML Choice projection, while keeping inactive VML
// source physical and exact. An admitted choice must not paint its fallback too.
fn alternate_content_watermarks(xml: &[u8]) -> Vec<VmlWatermark> {
    // Use the same qualified selection as CT_R::alt_drawings. Distinct selectors
    // can otherwise paint both a rejected modern branch and its fallback.
    if crate::drawing::parse_alternate_content(xml, &[]).is_some() {
        return Vec::new();
    }
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut depth = 0_usize;
    loop {
        let offset = reader.buffer_position() as usize;
        let Ok((namespace, event)) = reader.read_resolved_event_into(&mut buffer) else {
            return Vec::new();
        };
        match event {
            Event::Start(ref element)
                if depth == 1
                    && namespace_is(&namespace, crate::namespace::MC_NS)
                    && local_name(element.name().as_ref()) == b"Fallback" =>
            {
                let name = element.name().as_ref().to_vec();
                if reader
                    .read_to_end_into(quick_xml::name::QName(&name), &mut Vec::new())
                    .is_err()
                {
                    return Vec::new();
                }
                let end = reader.buffer_position() as usize;
                let Ok(bindings) = ancestor_namespace_bindings(xml, offset) else {
                    return Vec::new();
                };
                let Ok(scoped) =
                    crate::text::raw_with_external_bindings(&xml[offset..end], &bindings)
                else {
                    return Vec::new();
                };
                return parse_vml_watermarks(&scoped);
            }
            Event::Start(_) => depth += 1,
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Vec::new()
}

fn parse_vml_watermarks(xml: &[u8]) -> Vec<VmlWatermark> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut in_pict = false;
    let mut pending = None;
    let mut watermarks = Vec::new();

    loop {
        let offset = reader.buffer_position() as usize;
        let Ok((namespace, event)) = reader.read_resolved_event_into(&mut buffer) else {
            return Vec::new();
        };
        match event {
            Event::Start(ref element)
                if namespace_is(
                    &namespace,
                    "http://schemas.openxmlformats.org/markup-compatibility/2006",
                ) && local_name(element.name().as_ref()) == b"AlternateContent" =>
            {
                let name = element.name().as_ref().to_vec();
                if reader
                    .read_to_end_into(quick_xml::name::QName(&name), &mut Vec::new())
                    .is_err()
                {
                    return Vec::new();
                }
                let end = reader.buffer_position() as usize;
                let Ok(bindings) = ancestor_namespace_bindings(xml, offset) else {
                    return Vec::new();
                };
                let Ok(scoped) =
                    crate::text::raw_with_external_bindings(&xml[offset..end], &bindings)
                else {
                    return Vec::new();
                };
                watermarks.extend(alternate_content_watermarks(&scoped));
            }
            Event::Start(ref element) | Event::Empty(ref element) => {
                let name = element.name();
                let local = local_name(name.as_ref());
                if namespace_is(&namespace, W_NS) && local == b"pict" {
                    in_pict = true;
                } else if in_pict && namespace_is(&namespace, VML_NS) && local == b"shape" {
                    let Some(style) = unqualified_attribute(&reader, element, b"style") else {
                        buffer.clear();
                        continue;
                    };
                    pending = Some(PendingWatermark {
                        width: style_number(&style, "width", "pt"),
                        height: style_number(&style, "height", "pt"),
                        rotation: style_number(&style, "rotation", "").unwrap_or(0.0),
                        color: unqualified_attribute(&reader, element, b"fillcolor")
                            .map(|value| value.trim_start_matches('#').to_owned()),
                        opacity: style_number(&style, "opacity", "").unwrap_or(1.0),
                        ..PendingWatermark::default()
                    });
                } else if let Some(shape) = pending.as_mut()
                    && namespace_is(&namespace, VML_NS)
                    && local == b"fill"
                {
                    shape.opacity = unqualified_attribute(&reader, element, b"opacity")
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(shape.opacity);
                } else if let Some(shape) = pending.as_mut()
                    && namespace_is(&namespace, VML_NS)
                    && local == b"textpath"
                {
                    shape.text = unqualified_attribute(&reader, element, b"string");
                    shape.font_family = unqualified_attribute(&reader, element, b"style")
                        .and_then(|style| style_value(&style, "font-family"))
                        .map(|family| family.trim_matches(['\'', '"']).to_owned());
                } else if let Some(shape) = pending.as_mut()
                    && namespace_is(&namespace, VML_NS)
                    && local == b"imagedata"
                {
                    shape.relationship_id =
                        namespaced_attribute(&reader, element, RELATIONSHIPS_NS, b"id");
                }
            }
            Event::End(ref element)
                if namespace_is(&namespace, VML_NS)
                    && local_name(element.name().as_ref()) == b"shape" =>
            {
                if let Some(watermark) = pending.take().and_then(PendingWatermark::finish) {
                    watermarks.push(watermark);
                }
            }
            Event::End(ref element)
                if namespace_is(&namespace, W_NS)
                    && local_name(element.name().as_ref()) == b"pict" =>
            {
                in_pict = false;
                pending = None;
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }

    watermarks
}

fn local_name(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

fn namespace_is(namespace: &ResolveResult<'_>, expected: &str) -> bool {
    matches!(namespace, ResolveResult::Bound(Namespace(uri)) if *uri == expected.as_bytes())
}

fn unqualified_attribute(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
    expected: &[u8],
) -> Option<String> {
    element.attributes().flatten().find_map(|attribute| {
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        if matches!(namespace, ResolveResult::Unbound) && local.as_ref() == expected {
            attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                .ok()
                .map(|value| value.into_owned())
        } else {
            None
        }
    })
}

fn raw_unqualified_attribute(element: &BytesStart<'_>, expected: &[u8]) -> Option<String> {
    element.attributes().flatten().find_map(|attribute| {
        (attribute.key.as_ref() == expected)
            .then(|| {
                attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                    .ok()
                    .map(|value| value.into_owned())
            })
            .flatten()
    })
}

fn namespaced_attribute(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
    expected_namespace: &str,
    expected_local: &[u8],
) -> Option<String> {
    element.attributes().flatten().find_map(|attribute| {
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        if namespace_is(&namespace, expected_namespace) && local.as_ref() == expected_local {
            attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                .ok()
                .map(|value| value.into_owned())
        } else {
            None
        }
    })
}

fn style_value(style: &str, expected: &str) -> Option<String> {
    style.split(';').find_map(|declaration| {
        let (name, value) = declaration.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case(expected)
            .then(|| value.trim().to_owned())
    })
}

fn style_number(style: &str, name: &str, suffix: &str) -> Option<f64> {
    style_value(style, name)?
        .strip_suffix(suffix)?
        .trim()
        .parse()
        .ok()
}

fn compact_number(value: f64) -> String {
    let mut value = format!("{value:.6}");
    while value.ends_with('0') {
        value.pop();
    }
    if value.ends_with('.') {
        value.pop();
    }
    value
}

/// Header/footer reference type in section properties.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdrFtrType {
    /// Default header/footer
    Default,
    /// First page header/footer
    First,
    /// Even page header/footer
    Even,
}

impl HdrFtrType {
    pub fn from_str(s: &str) -> Self {
        match s {
            "first" => Self::First,
            "even" => Self::Even,
            _ => Self::Default,
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::First => "first",
            Self::Even => "even",
        }
    }
}

/// A header or footer reference (stored in section properties).
///
/// Two references are equal when they have the same type and relationship,
/// wherever they were read.
#[derive(Debug, Clone)]
pub struct HdrFtrRef {
    /// The type (default, first, even)
    pub hdr_ftr_type: HdrFtrType,
    /// Relationship ID
    pub rel_id: String,
    /// Its place among all the header and footer references of the section
    /// it was read in, which interleaves the two lists of `CT_SectPr` back
    /// in that order. `None` for a reference new to its section.
    pub(crate) source_index: Option<usize>,
}

impl HdrFtrRef {
    /// A reference of `hdr_ftr_type` to the story under relationship
    /// `rel_id`, new to its section.
    pub fn new(hdr_ftr_type: HdrFtrType, rel_id: impl Into<String>) -> Self {
        Self {
            hdr_ftr_type,
            rel_id: rel_id.into(),
            source_index: None,
        }
    }

    /// Take the place `replaced` was read at, for a reference that replaces
    /// it in its section.
    #[doc(hidden)]
    pub fn take_place_of(&mut self, replaced: &HdrFtrRef) {
        self.source_index = replaced.source_index;
    }
}

impl PartialEq for HdrFtrRef {
    fn eq(&self, other: &Self) -> bool {
        self.hdr_ftr_type == other.hdr_ftr_type && self.rel_id == other.rel_id
    }
}
