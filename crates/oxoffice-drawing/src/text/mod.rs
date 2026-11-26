use std::io::Write;

use oxoffice_core::OxmlError;
use oxoffice_core::raw_xml::{capture_element, capture_empty, emit_raw};
use oxoffice_core::xml::matches_local_name;
use oxoffice_core::xml_reader::Reader;
use quick_xml::Writer;
use quick_xml::events::{BytesEnd, BytesStart, Event};

use crate::order::OrderedRawChildren;

pub mod body;
pub mod bullet;
pub mod list_style;
pub mod paragraph;

pub use body::{
    CT_TextBodyProperties, Coordinate32Value, NormalAutofit, TextAnchor, TextAutofit, TextError,
    TextVertical, TextWrap,
};
pub use bullet::{
    TextAutoNumber, TextAutoNumberScheme, TextBullet, TextBulletCharacter, TextBulletChoice,
    TextBulletColor, TextBulletSize, TextBulletSizeValue, TextNoBullet,
};
pub use list_style::CT_TextListStyle;
pub use paragraph::{
    CT_RegularTextRun, CT_TextCharacterProperties, CT_TextField, CT_TextLineBreak,
    CT_TextParagraph, CT_TextParagraphProperties, TextAlignment, TextFont, TextHyperlink,
    TextPointValue, TextRun, TextSpace, TextSpacing, TextStrike, TextUnderline, TextValue,
    escape_invalid_xml_characters,
};

use body::{Result, missing_end};
use paragraph::{
    TextNamespaceScope, text_capture_opaque_event, text_close_generated_child, text_fragment_scope,
    text_namespace_scope, text_owned_name, text_reject_trailing, text_root_spellings_scoped,
};

#[derive(Clone, Debug)]
struct SourceBodyProperties {
    raw: Vec<u8>,
    original: CT_TextBodyProperties,
    _namespaces: Vec<(String, String)>,
}

/// The `a:txBody` shell with typed body properties and opaque later text stages.
#[allow(non_camel_case_types)]
#[derive(Clone, Debug)]
pub struct CT_TextBody {
    pub body_properties: CT_TextBodyProperties,
    list_style: Option<Box<CT_TextListStyle>>,
    paragraphs: Vec<CT_TextParagraph>,
    raw_children: OrderedRawChildren,
    source_root: Vec<String>,
    source_tag: String,
    source_body_properties: Option<SourceBodyProperties>,
}
impl PartialEq for CT_TextBody {
    fn eq(&self, other: &Self) -> bool {
        self.body_properties == other.body_properties
            && self.list_style == other.list_style
            && self.paragraphs == other.paragraphs
            && self.raw_children == other.raw_children
    }
}
impl Eq for CT_TextBody {}

impl Default for CT_TextBody {
    fn default() -> Self {
        Self::new()
    }
}

impl CT_TextBody {
    /// Creates a minimal valid text body with one empty paragraph.
    pub fn new() -> Self {
        Self {
            body_properties: CT_TextBodyProperties::default(),
            list_style: None,
            paragraphs: vec![CT_TextParagraph::default()],
            raw_children: OrderedRawChildren::default(),
            source_root: Vec::new(),
            source_tag: String::new(),
            source_body_properties: None,
        }
    }

    /// Parses a complete `a:txBody` while retaining later text stages.
    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        Self::from_xml_with_inherited_namespaces(xml, &[])
    }

    /// Parses a complete text body under a caller-selected root local name.
    pub fn from_xml_as(xml: &[u8], root_local_name: &[u8]) -> Result<Self> {
        Self::from_xml_as_with_namespaces(xml, root_local_name, &[])
    }

    /// Parses a text-body fragment in its actual enclosing namespace scope.
    pub fn from_xml_with_inherited_namespaces(
        xml: &[u8],
        inherited: &[(String, String)],
    ) -> Result<Self> {
        Self::from_xml_as_with_namespaces(xml, b"txBody", inherited)
    }

    /// Parses a caller-selected text-body root in its actual enclosing namespace scope.
    /// Retained source declarations stay separate from inherited lookup bindings.
    pub fn from_xml_as_with_namespaces(
        xml: &[u8],
        root_local_name: &[u8],
        inherited: &[(String, String)],
    ) -> Result<Self> {
        let mut reader = Reader::from_reader(xml);
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element)
                    if matches_local_name(element.name().as_ref(), root_local_name) =>
                {
                    let scope = text_namespace_scope(&element, inherited)?;
                    let name = element.name();
                    let spelling = std::str::from_utf8(name.as_ref()).map_err(OxmlError::from)?;
                    let prefix = spelling.split_once(':').map_or("", |(prefix, _)| prefix);
                    if scope.iter().any(|(key, uri)| {
                        key == prefix
                            && uri != crate::namespace::A_NS
                            && uri != "http://schemas.openxmlformats.org/presentationml/2006/main"
                            && !(root_local_name == b"t"
                                && uri
                                    == "http://schemas.openxmlformats.org/drawingml/2006/diagram")
                            && !(root_local_name == b"txPr"
                                && uri == "http://schemas.openxmlformats.org/drawingml/2006/chart")
                            && !(root_local_name == b"txBody"
                                && uri
                                    == "http://schemas.microsoft.com/office/powerpoint/2018/8/main")
                    }) {
                        return Err(TextError::UnexpectedElement(
                            "foreign text body namespace".to_owned(),
                        ));
                    }
                    let parsed = Self::from_element(
                        &mut reader,
                        root_local_name,
                        &element,
                        &TextNamespaceScope::inherited(inherited),
                    )?;
                    text_reject_trailing(&mut reader)?;
                    return Ok(parsed);
                }
                Event::Empty(element)
                    if matches_local_name(element.name().as_ref(), root_local_name) =>
                {
                    return Err(TextError::MissingBodyProperties);
                }
                Event::Start(element) | Event::Empty(element) => {
                    return Err(TextError::UnexpectedElement(element_name(&element)));
                }
                Event::Eof => {
                    return Err(TextError::Xml(OxmlError::MissingElement(
                        "DrawingML text body".to_owned(),
                    )));
                }
                _ => {}
            }
            buffer.clear();
        }
    }

    fn from_element(
        reader: &mut Reader<&[u8]>,
        root_local_name: &[u8],
        start: &BytesStart<'_>,
        inherited: &TextNamespaceScope,
    ) -> Result<Self> {
        let namespaces = text_fragment_scope(start, inherited)?;
        let source_root = text_root_spellings_scoped(
            start,
            inherited,
            &capture_element(&mut reader.clone(), start)?,
        )?;
        let mut body_properties = None;
        let mut source_body_properties = None;
        let mut list_style = None;
        let mut paragraphs = Vec::new();
        let mut raw_children = OrderedRawChildren::default();
        let mut boundary = 0;
        let mut buffer = Vec::new();

        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element) if text_owned_name(&element, &namespaces)? == b"bodyPr" => {
                    if body_properties.is_some() {
                        return Err(TextError::DuplicateElement("bodyPr".to_owned()));
                    }
                    let raw = capture_element(reader, &element)?;
                    let parsed = CT_TextBodyProperties::from_xml(&raw)?;
                    source_body_properties = Some(SourceBodyProperties {
                        raw,
                        original: parsed.clone(),
                        _namespaces: namespaces.physical.clone(),
                    });
                    body_properties = Some(parsed);
                    boundary = boundary.max(1);
                }
                Event::Empty(element) if text_owned_name(&element, &namespaces)? == b"bodyPr" => {
                    if body_properties.is_some() {
                        return Err(TextError::DuplicateElement("bodyPr".to_owned()));
                    }
                    let raw = capture_empty(&element, reader.empty_writing())?;
                    let parsed = CT_TextBodyProperties::from_xml(&raw)?;
                    source_body_properties = Some(SourceBodyProperties {
                        raw,
                        original: parsed.clone(),
                        _namespaces: namespaces.physical.clone(),
                    });
                    body_properties = Some(parsed);
                    boundary = boundary.max(1);
                }
                Event::Start(element) if text_owned_name(&element, &namespaces)? == b"lstStyle" => {
                    if list_style.is_some() {
                        return Err(TextError::DuplicateElement("lstStyle".to_owned()));
                    }
                    let raw = capture_element(reader, &element)?;
                    list_style = Some(CT_TextListStyle::from_xml_boxed_with_namespaces(
                        &raw,
                        &namespaces,
                    )?);
                    boundary = boundary.max(2);
                }
                Event::Empty(element) if text_owned_name(&element, &namespaces)? == b"lstStyle" => {
                    if list_style.is_some() {
                        return Err(TextError::DuplicateElement("lstStyle".to_owned()));
                    }
                    let raw = capture_empty(&element, reader.empty_writing())?;
                    list_style = Some(CT_TextListStyle::from_xml_boxed_with_namespaces(
                        &raw,
                        &namespaces,
                    )?);
                    boundary = boundary.max(2);
                }
                Event::Start(element) if text_owned_name(&element, &namespaces)? == b"p" => {
                    let raw = capture_element(reader, &element)?;
                    paragraphs.push(CT_TextParagraph::from_xml_with_namespaces(
                        &raw,
                        &namespaces,
                    )?);
                    boundary = boundary.max(2 + paragraphs.len());
                }
                Event::Empty(element) if text_owned_name(&element, &namespaces)? == b"p" => {
                    let raw = capture_empty(&element, reader.empty_writing())?;
                    paragraphs.push(CT_TextParagraph::from_xml_with_namespaces(
                        &raw,
                        &namespaces,
                    )?);
                    boundary = boundary.max(2 + paragraphs.len());
                }
                Event::Start(element) => {
                    raw_children.push(boundary, capture_element(reader, &element)?)
                }
                Event::Empty(element) => {
                    raw_children.push(boundary, capture_empty(&element, reader.empty_writing())?)
                }
                Event::End(element)
                    if matches_local_name(element.name().as_ref(), root_local_name) =>
                {
                    break;
                }
                Event::Eof => {
                    return Err(missing_end(&String::from_utf8_lossy(root_local_name)));
                }
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

        let body_properties = body_properties.ok_or(TextError::MissingBodyProperties)?;
        Ok(Self {
            body_properties,
            list_style,
            paragraphs,
            raw_children,
            source_root,
            source_tag: std::str::from_utf8(start.name().as_ref())
                .map_err(OxmlError::from)?
                .to_owned(),
            source_body_properties,
        })
    }

    /// Writes the shell with canonical outer prefixes and schema order.
    pub fn to_xml(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new(Vec::new());
        self.write_xml(&mut writer)?;
        Ok(writer.into_inner())
    }

    /// Writes the shell into an existing XML writer.
    pub fn write_xml<W: Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.write_xml_as(writer, "a:txBody")
    }

    /// Writes the text body using a caller-selected OOXML wrapper tag.
    pub fn write_xml_as<W: Write>(&self, writer: &mut Writer<W>, tag: &str) -> Result<()> {
        let start = paragraph::text_wrapper_start(tag, &self.source_tag, &self.source_root)?;
        let owned_tag = std::str::from_utf8(start.name().as_ref())
            .map_err(OxmlError::from)?
            .to_owned();
        let tag = owned_tag.as_str();
        let namespaces = paragraph::text_namespace_scope(&start, &[])?;
        writer
            .write_event(Event::Start(start))
            .map_err(OxmlError::from)?;
        emit_raw(writer, self.raw_children.at(0))?;
        if let Some(SourceBodyProperties {
            raw,
            original,
            _namespaces,
        }) = &self.source_body_properties
        {
            if &self.body_properties == original && !_namespaces.is_empty() {
                writer.get_mut().write_all(raw).map_err(OxmlError::from)?;
            } else {
                {
                    let mut child = Writer::new(Vec::new());
                    self.body_properties.write_xml(&mut child)?;
                    writer
                        .get_mut()
                        .write_all(&text_close_generated_child(
                            &child.into_inner(),
                            &namespaces,
                        )?)
                        .map_err(OxmlError::from)?;
                }
            }
        } else {
            {
                let mut child = Writer::new(Vec::new());
                self.body_properties.write_xml(&mut child)?;
                writer
                    .get_mut()
                    .write_all(&text_close_generated_child(
                        &child.into_inner(),
                        &namespaces,
                    )?)
                    .map_err(OxmlError::from)?;
            }
        }
        emit_raw(writer, self.raw_children.at(1))?;
        if let Some(list_style) = &self.list_style {
            {
                let mut child = Writer::new(Vec::new());
                list_style.write_xml(&mut child)?;
                writer
                    .get_mut()
                    .write_all(&text_close_generated_child(
                        &child.into_inner(),
                        &namespaces,
                    )?)
                    .map_err(OxmlError::from)?;
            }
        }
        emit_raw(writer, self.raw_children.at(2))?;
        for (index, paragraph) in self.paragraphs.iter().enumerate() {
            {
                let mut child = Writer::new(Vec::new());
                paragraph.write_xml(&mut child)?;
                writer
                    .get_mut()
                    .write_all(&text_close_generated_child(
                        &child.into_inner(),
                        &namespaces,
                    )?)
                    .map_err(OxmlError::from)?;
            }
            emit_raw(writer, self.raw_children.at(3 + index))?;
        }
        writer
            .write_event(Event::End(BytesEnd::new(tag)))
            .map_err(OxmlError::from)?;
        Ok(())
    }

    /// Returns plain text in paragraph and text-content order.
    pub fn plain_text(&self) -> String {
        self.paragraphs
            .iter()
            .map(|paragraph| {
                let mut text = String::new();
                for run in &paragraph.runs {
                    match run {
                        TextRun::Run(run) => text.push_str(&run.text.value),
                        TextRun::Break(_) => text.push('\n'),
                        TextRun::Field(field) => {
                            if let Some(value) = &field.text {
                                text.push_str(&value.value);
                            }
                        }
                    }
                }
                text
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn has_list_style(&self) -> bool {
        self.list_style.is_some()
    }

    pub fn list_style(&self) -> Option<&CT_TextListStyle> {
        self.list_style.as_deref()
    }

    pub fn paragraph_count(&self) -> usize {
        self.paragraphs.len()
    }

    pub fn paragraphs(&self) -> &[CT_TextParagraph] {
        &self.paragraphs
    }

    /// Replaces all text content while retaining body-level state.
    ///
    /// Each line feed or CRLF pair starts a paragraph, and a vertical tab
    /// creates an `a:br`. New paragraphs copy the first paragraph's formatting.
    pub fn set_text(&mut self, text: &str) {
        let old_paragraph_count = self.paragraphs.len();
        let mut first = self.paragraphs.drain(..).next().unwrap_or_default();
        let mut lines = text.split("\r\n").flat_map(|part| part.split('\n'));
        first.set_text(lines.next().unwrap_or_default());
        let mut template = CT_TextParagraph::default();
        template.properties = first.properties.clone();
        template.runs = first.runs.iter().take(1).cloned().collect();
        template.end_properties = first.end_properties.clone();
        self.paragraphs.push(first);
        for line in lines {
            let mut paragraph = template.clone();
            paragraph.set_text(line);
            self.paragraphs.push(paragraph);
        }
        self.reconcile_paragraph_raw_children(old_paragraph_count, self.paragraphs.len());
    }

    /// Returns an empty body that keeps this body's properties and list style
    /// and the formatting of its first paragraph, as PowerPoint fills the
    /// cells of a new table row or column.
    ///
    /// The one paragraph keeps the first paragraph's properties. The
    /// character properties of that paragraph's first regular run, or its end
    /// properties when the run is missing or has none, become the new end
    /// properties without their hyperlinks. Unmodelled body and paragraph
    /// children are not copied.
    pub fn empty_like(&self) -> Self {
        let mut paragraph = CT_TextParagraph::default();
        if let Some(first) = self.paragraphs.first() {
            let first_run = first.runs.iter().find_map(|run| match run {
                TextRun::Run(run) => Some(run),
                TextRun::Break(_) | TextRun::Field(_) => None,
            });
            paragraph.properties = first.properties.clone();
            paragraph.end_properties = first_run
                .and_then(|run| run.properties.clone())
                .or_else(|| first.end_properties.clone())
                .map(CT_TextCharacterProperties::without_hyperlinks);
        }
        Self {
            body_properties: self.body_properties.clone(),
            list_style: self.list_style.clone(),
            paragraphs: vec![paragraph],
            raw_children: OrderedRawChildren::default(),
            source_root: if self.body_properties.autofit_source.is_some() {
                self.source_root.clone()
            } else {
                Vec::new()
            },
            source_tag: if self.body_properties.autofit_source.is_some() {
                self.source_tag.clone()
            } else {
                String::new()
            },
            source_body_properties: if self.body_properties.autofit_source.is_some() {
                self.source_body_properties.clone()
            } else {
                None
            },
        }
    }

    /// Returns one paragraph for in-place mutation.
    pub fn paragraph_mut(&mut self, index: usize) -> Option<&mut CT_TextParagraph> {
        self.paragraphs.get_mut(index)
    }

    /// Appends one empty paragraph after all existing paragraphs.
    pub fn add_paragraph(&mut self) -> &mut CT_TextParagraph {
        self.raw_children
            .shift_boundaries_from(2 + self.paragraphs.len());
        self.paragraphs.push(CT_TextParagraph::default());
        self.paragraphs.last_mut().expect("paragraph was appended")
    }

    /// Moves visible typed paragraphs to `destination`, retaining body-level
    /// state and leaving one empty paragraph in this body.
    pub fn move_content_to(&mut self, destination: &mut Self) {
        if self.plain_text().is_empty() {
            return;
        }

        let source_paragraph_count = self.paragraphs.len();
        let moved = std::mem::take(&mut self.paragraphs);
        self.paragraphs.push(CT_TextParagraph::default());
        self.reconcile_paragraph_raw_children(source_paragraph_count, 1);

        if destination.plain_text().is_empty() {
            let destination_paragraph_count = destination.paragraphs.len();
            destination.paragraphs = moved;
            destination.reconcile_paragraph_raw_children(
                destination_paragraph_count,
                destination.paragraphs.len(),
            );
            return;
        }

        for paragraph in moved {
            destination
                .raw_children
                .shift_boundaries_from(2 + destination.paragraphs.len());
            destination.paragraphs.push(paragraph);
        }
    }

    fn reconcile_paragraph_raw_children(
        &mut self,
        old_paragraph_count: usize,
        new_paragraph_count: usize,
    ) {
        let mut raw_children = OrderedRawChildren::default();
        for boundary in 0..=2 + old_paragraph_count {
            let new_boundary = if boundary <= 2 {
                boundary
            } else {
                2 + new_paragraph_count
            };
            for child in self.raw_children.at(boundary) {
                raw_children.push(new_boundary, child.to_vec());
            }
        }
        self.raw_children = raw_children;
    }

    pub fn raw_children(&self) -> &OrderedRawChildren {
        &self.raw_children
    }
}

fn element_name(element: &BytesStart<'_>) -> String {
    String::from_utf8_lossy(element.name().as_ref()).into_owned()
}
