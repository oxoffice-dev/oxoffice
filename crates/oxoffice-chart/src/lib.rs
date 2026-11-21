#![allow(non_camel_case_types)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Write;

use oxoffice_core::OxmlError;
use oxoffice_core::raw_xml::{
    capture_element, capture_empty_element, capture_event, emit_raw, push_attributes,
};
use oxoffice_core::xml::{escape_text, local_name, matches_local_name};
use oxoffice_core::xml_text::resolve_entity;
use oxoffice_drawing::color::{
    ColorChoice, ColorMap, ColorMapSlot, ResolvedColor, ThemeColorSlot, apply_color_transforms,
    resolve_color,
};
use oxoffice_drawing::fill::{Fill, SolidFill};
use oxoffice_drawing::line::CT_LineProperties;
use oxoffice_drawing::order::OrderedRawChildren;
use oxoffice_drawing::shape_props::{CT_ShapeProperties, ShapePropertiesError};
use oxoffice_drawing::text::{CT_TextBody, CT_TextCharacterProperties, TextError};
use oxoffice_drawing::theme::CT_OfficeStyleSheet;
use oxoffice_layout::{
    Color, FillRule, FontManager, GlyphRun, GroupElement, Paint, Path, PathCommand, PathElement,
    Point, PositionedElement, Rect, Stroke, Transform,
};
use oxoffice_sml::{ChartCellValue, ChartRange, ChartRangeUpdate, Column, Workbook};
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer, XmlVersion};

pub use oxoffice_drawing::color::RgbColor;

mod authoring;
mod render;

pub use authoring::{
    authored_chart_parts, chart_legend_settings, chart_plain_title, chart_workbook_relationship_id,
    read_chart_data, read_chart_type, replace_chart_data, set_chart_legend,
};
use authoring::{
    chart_child_node, chart_source_nodes, invalid_authoring_value, legend_settings_at,
};
pub use render::{
    ChartGeometry, render_chart, render_chart_placeholder, render_geometry,
    unsupported_number_formats,
};

/// The transitional ChartML namespace used by OOXML packages.
pub const C_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
/// The fixed ChartML prefix used by writers in this crate.
pub const C_PREFIX: &str = "c";
/// The DrawingML namespace written on a chart-space root.
pub const A_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
/// The office-document relationships namespace written on a chart-space root.
pub const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const MAX_CATEGORY_ANNOTATIONS: usize = 16_384;
const PLOT_MARGIN_LEFT: f64 = 36.0;
const PLOT_MARGIN_RIGHT: f64 = 12.0;
const PLOT_MARGIN_TOP: f64 = 12.0;
const PLOT_MARGIN_BOTTOM: f64 = 28.0;

/// Errors produced while reading or writing the implemented ChartML core.
#[derive(Debug)]
pub enum ChartError {
    Xml(OxmlError),
    ShapeProperties(ShapePropertiesError),
    Text(TextError),
    UnexpectedElement(String),
    MissingElement(String),
    DuplicateElement(String),
    InvalidAttribute {
        element: String,
        attribute: String,
        value: String,
    },
    InvalidValue {
        element: String,
        value: String,
    },
}

impl fmt::Display for ChartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Xml(error) => error.fmt(formatter),
            Self::ShapeProperties(error) => error.fmt(formatter),
            Self::Text(error) => error.fmt(formatter),
            Self::UnexpectedElement(element) => {
                write!(formatter, "unexpected ChartML element: {element}")
            }
            Self::MissingElement(element) => {
                write!(formatter, "ChartML requires {element}")
            }
            Self::DuplicateElement(element) => {
                write!(formatter, "ChartML contains duplicate {element}")
            }
            Self::InvalidAttribute {
                element,
                attribute,
                value,
            } => write!(
                formatter,
                "ChartML {element} has invalid @{attribute}: {value}"
            ),
            Self::InvalidValue { element, value } => {
                write!(formatter, "ChartML {element} has invalid value: {value}")
            }
        }
    }
}

impl std::error::Error for ChartError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Xml(error) => Some(error),
            Self::ShapeProperties(error) => Some(error),
            Self::Text(error) => Some(error),
            _ => None,
        }
    }
}

impl From<OxmlError> for ChartError {
    fn from(error: OxmlError) -> Self {
        Self::Xml(error)
    }
}

impl From<ShapePropertiesError> for ChartError {
    fn from(error: ShapePropertiesError) -> Self {
        Self::ShapeProperties(error)
    }
}

impl From<TextError> for ChartError {
    fn from(error: TextError) -> Self {
        Self::Text(error)
    }
}

pub type Result<T> = std::result::Result<T, ChartError>;

/// One supported two-dimensional chart family.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChartKind {
    Bar,
    Line,
    Pie,
    Doughnut,
    Area,
    Scatter,
    Radar,
}

/// Data used to author one chart and its editable workbook.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChartData {
    pub categories: Vec<String>,
    pub series: Vec<(String, Vec<f64>)>,
    pub number_format: Option<String>,
    pub category_axis_title: Option<String>,
    pub value_axis_title: Option<String>,
    pub palette: Vec<RgbColor>,
}

type NamespaceBindings = Vec<(Vec<u8>, String)>;
type XmlAttributes = Vec<(String, String)>;
type RootAttributes = (XmlAttributes, XmlAttributes);

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ScalarMarkup {
    raw_attributes: XmlAttributes,
    raw_content: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TextMarkup {
    raw_attributes: XmlAttributes,
    raw_children: OrderedRawChildren,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct PointMarkup {
    raw_attributes: XmlAttributes,
    raw_children: OrderedRawChildren,
    value: TextMarkup,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ReferenceMarkup {
    raw_attributes: XmlAttributes,
    raw_children: OrderedRawChildren,
    formula: TextMarkup,
    cache_attributes: XmlAttributes,
    cache_children: OrderedRawChildren,
    format_code: Option<TextMarkup>,
    format_code_omitted: bool,
    point_count: ScalarMarkup,
    declared_point_count: Option<u32>,
    point_indexes: Vec<u32>,
    points: Vec<PointMarkup>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct WrapperMarkup {
    raw_attributes: XmlAttributes,
    raw_children: OrderedRawChildren,
}

/// A formula-backed string cache whose count and point indexes are derived.
#[derive(Clone, Debug, PartialEq)]
pub struct StringRef {
    pub formula: String,
    pub values: Vec<String>,
    markup: ReferenceMarkup,
}

impl StringRef {
    pub fn new(formula: String, values: Vec<String>) -> Result<Self> {
        let reference = Self {
            formula,
            values,
            markup: ReferenceMarkup::default(),
        };
        reference.validate()?;
        Ok(reference)
    }

    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        Self::from_xml_with_namespaces(xml, &chart_namespace_defaults())
    }

    fn from_xml_with_namespaces(xml: &[u8], inherited: &NamespaceBindings) -> Result<Self> {
        let parsed = parse_reference(xml, b"strRef", b"strCache", false, inherited)?;
        let values = parsed.values;
        let reference = Self {
            formula: parsed.formula,
            values,
            markup: parsed.markup,
        };
        reference.validate()?;
        Ok(reference)
    }

    pub fn to_xml(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut writer = Writer::new(Vec::new());
        self.write_xml(&mut writer, true)?;
        Ok(writer.into_inner())
    }

    fn validate(&self) -> Result<()> {
        validate_formula(&self.formula, "c:strRef/c:f")?;
        for value in &self.values {
            validate_xml_text(value, "c:strCache/c:pt/c:v")?;
        }
        validate_point_count(self.values.len(), "c:strCache")
    }

    fn write_xml(&self, writer: &mut Writer<Vec<u8>>, standalone: bool) -> Result<()> {
        self.validate()?;
        let mut start = BytesStart::new("c:strRef");
        if standalone {
            start.push_attribute(("xmlns:c", C_NS));
        }
        push_attributes(&mut start, &self.markup.raw_attributes);
        writer
            .write_event(Event::Start(start))
            .map_err(OxmlError::from)?;
        emit_raw(writer, self.markup.raw_children.at(0))?;
        write_text(writer, "c:f", &self.formula, &self.markup.formula)?;
        emit_raw(writer, self.markup.raw_children.at(1))?;
        write_string_cache(writer, &self.values, &self.markup)?;
        emit_raw(writer, self.markup.raw_children.at(2))?;
        writer
            .write_event(Event::End(BytesEnd::new("c:strRef")))
            .map_err(OxmlError::from)?;
        Ok(())
    }
}

/// A formula-backed numeric cache whose metadata and points are derived.
#[derive(Clone, Debug, PartialEq)]
pub struct NumericData {
    pub formula: String,
    pub format_code: String,
    pub values: Vec<f64>,
    markup: ReferenceMarkup,
}

impl NumericData {
    pub fn new(formula: String, format_code: String, values: Vec<f64>) -> Result<Self> {
        validate_formula(&formula, "c:numRef/c:f")?;
        validate_format_code(&format_code)?;
        validate_numeric_values(&values)?;
        validate_point_count(values.len(), "c:numCache")?;
        Ok(Self {
            formula,
            format_code,
            values,
            markup: ReferenceMarkup::default(),
        })
    }

    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        Self::from_xml_with_namespaces(xml, &chart_namespace_defaults())
    }

    fn from_xml_with_namespaces(xml: &[u8], inherited: &NamespaceBindings) -> Result<Self> {
        let parsed = parse_reference(xml, b"numRef", b"numCache", true, inherited)?;
        let format_code = parsed.format_code.unwrap_or_else(|| "General".to_owned());
        let mut values = Vec::with_capacity(parsed.values.len());
        for value in parsed.values {
            let number = value.parse::<f64>().map_err(|_| ChartError::InvalidValue {
                element: "c:v".to_owned(),
                value: value.clone(),
            })?;
            if !number.is_finite() {
                return Err(ChartError::InvalidValue {
                    element: "c:v".to_owned(),
                    value,
                });
            }
            values.push(number);
        }
        let data = Self {
            formula: parsed.formula,
            format_code,
            values,
            markup: parsed.markup,
        };
        data.validate()?;
        Ok(data)
    }

    pub fn to_xml(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut writer = Writer::new(Vec::new());
        self.write_xml(&mut writer, true)?;
        Ok(writer.into_inner())
    }

    fn validate(&self) -> Result<()> {
        validate_formula(&self.formula, "c:numRef/c:f")?;
        validate_format_code(&self.format_code)?;
        validate_numeric_values(&self.values)?;
        validate_point_count(self.values.len(), "c:numCache")
    }

    fn write_xml(&self, writer: &mut Writer<Vec<u8>>, standalone: bool) -> Result<()> {
        self.validate()?;
        let mut start = BytesStart::new("c:numRef");
        if standalone {
            start.push_attribute(("xmlns:c", C_NS));
        }
        push_attributes(&mut start, &self.markup.raw_attributes);
        writer
            .write_event(Event::Start(start))
            .map_err(OxmlError::from)?;
        emit_raw(writer, self.markup.raw_children.at(0))?;
        write_text(writer, "c:f", &self.formula, &self.markup.formula)?;
        emit_raw(writer, self.markup.raw_children.at(1))?;
        write_numeric_cache(writer, self, &self.markup)?;
        emit_raw(writer, self.markup.raw_children.at(2))?;
        writer
            .write_event(Event::End(BytesEnd::new("c:numRef")))
            .map_err(OxmlError::from)?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum AxisData {
    String(StringRef),
    Numeric(NumericData),
}

#[derive(Clone, Debug, PartialEq)]
struct DataPoint {
    index: u32,
    sp_pr: CT_ShapeProperties,
}

impl DataPoint {
    fn write_xml<W: Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        writer
            .write_event(Event::Start(BytesStart::new("c:dPt")))
            .map_err(OxmlError::from)?;
        write_scalar(writer, "c:idx", &self.index.to_string(), None)?;
        self.sp_pr.write_xml_as(writer, "c:spPr")?;
        writer
            .write_event(Event::End(BytesEnd::new("c:dPt")))
            .map_err(OxmlError::from)?;
        Ok(())
    }
}

/// The common formula-backed payload of one ChartML series.
#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    pub index: u32,
    pub order: u32,
    pub name: Option<StringRef>,
    pub categories: Option<AxisData>,
    pub values: NumericData,
    pub bubble_size: Option<NumericData>,
    pub sp_pr: Option<CT_ShapeProperties>,
    pub data_labels: Option<CT_DLbls>,
    authored_data_points: Vec<DataPoint>,
    index_markup: ScalarMarkup,
    order_markup: ScalarMarkup,
    name_markup: Option<WrapperMarkup>,
    categories_markup: Option<WrapperMarkup>,
    values_markup: WrapperMarkup,
    bubble_size_markup: Option<WrapperMarkup>,
    opaque_name: bool,
    opaque_categories: bool,
    opaque_bubble_size: bool,
    uses_scatter_wrappers: bool,
    parsed_from_xml: bool,
    raw_attributes: XmlAttributes,
    namespace_declarations: XmlAttributes,
    raw_children: OrderedRawChildren,
}

impl Series {
    pub fn new(index: u32, order: u32, values: NumericData) -> Self {
        Self {
            index,
            order,
            name: None,
            categories: None,
            values,
            bubble_size: None,
            sp_pr: None,
            data_labels: None,
            authored_data_points: Vec::new(),
            index_markup: ScalarMarkup::default(),
            order_markup: ScalarMarkup::default(),
            name_markup: None,
            categories_markup: None,
            values_markup: WrapperMarkup::default(),
            bubble_size_markup: None,
            opaque_name: false,
            opaque_categories: false,
            opaque_bubble_size: false,
            uses_scatter_wrappers: false,
            parsed_from_xml: false,
            raw_attributes: Vec::new(),
            namespace_declarations: Vec::new(),
            raw_children: OrderedRawChildren::default(),
        }
    }

    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        Self::from_xml_with_namespaces(xml, &chart_namespace_defaults())
    }

    fn from_xml_with_namespaces(xml: &[u8], inherited: &NamespaceBindings) -> Result<Self> {
        let mut reader = Reader::from_reader(xml);
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element) if matches_local_name(element.name().as_ref(), b"ser") => {
                    chart_root_prefix(&element)?;
                    if !element_is_in_namespace(&element, C_NS, inherited)? {
                        return Err(ChartError::UnexpectedElement(element_name(&element)));
                    }
                    return Self::from_element(&mut reader, &element, inherited);
                }
                Event::Empty(element) if matches_local_name(element.name().as_ref(), b"ser") => {
                    return Err(ChartError::MissingElement("c:idx".to_owned()));
                }
                Event::Start(element) | Event::Empty(element) => {
                    return Err(ChartError::UnexpectedElement(element_name(&element)));
                }
                Event::Eof => return Err(ChartError::MissingElement("c:ser".to_owned())),
                _ => {}
            }
            buffer.clear();
        }
    }

    fn from_element(
        reader: &mut Reader<&[u8]>,
        start: &BytesStart<'_>,
        inherited: &NamespaceBindings,
    ) -> Result<Self> {
        reject_conflicting_prefix(start, b"a", A_NS)?;
        let namespaces = chart_bindings(inherited, start)?;
        require_fixed_namespace(&namespaces, b"c", C_NS, start)?;
        require_fixed_namespace(&namespaces, b"a", A_NS, start)?;
        require_fixed_namespace(&namespaces, b"r", R_NS, start)?;
        let (raw_attributes, _) =
            capture_fixed_root_attributes(start, &["xmlns:c", "xmlns:a", "xmlns:r"])?;
        let namespace_declarations = standalone_namespace_declarations(&namespaces)?;
        let mut state = SeriesParseState::default();
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element) => {
                    let name = chart_child_local(&element, &namespaces)?;
                    let raw = capture_element(reader, &element)?;
                    state.parse_child(name.as_deref().unwrap_or_default(), raw, &namespaces)?;
                }
                Event::Empty(element) => {
                    let name = chart_child_local(&element, &namespaces)?;
                    let raw = capture_empty_element(&element)?;
                    state.parse_child(name.as_deref().unwrap_or_default(), raw, &namespaces)?;
                }
                event @ (Event::Text(_)
                | Event::CData(_)
                | Event::Comment(_)
                | Event::PI(_)
                | Event::GeneralRef(_)) => state.capture_event(capture_event(event)?),
                Event::End(element) if matches_local_name(element.name().as_ref(), b"ser") => {
                    break;
                }
                Event::Eof => return Err(missing_end("c:ser")),
                _ => {}
            }
            buffer.clear();
        }
        state.finish(raw_attributes, namespace_declarations)
    }

    pub fn to_xml(&self) -> Result<Vec<u8>> {
        self.to_xml_for_plot(self.uses_scatter_wrappers)
    }

    fn to_xml_for_plot(&self, scatter: bool) -> Result<Vec<u8>> {
        if self.opaque_name && self.name.is_some() {
            return Err(ChartError::DuplicateElement("c:tx".to_owned()));
        }
        if self.opaque_categories && self.categories.is_some() {
            return Err(ChartError::DuplicateElement("c:cat".to_owned()));
        }
        if self.opaque_bubble_size && self.bubble_size.is_some() {
            return Err(ChartError::DuplicateElement("c:bubbleSize".to_owned()));
        }
        self.values.validate()?;
        if let Some(name) = &self.name {
            name.validate()?;
        }
        if let Some(categories) = &self.categories {
            if scatter && !matches!(categories, AxisData::Numeric(_)) {
                return Err(ChartError::InvalidValue {
                    element: "c:xVal".to_owned(),
                    value: "scatter x values must use a numeric cache".to_owned(),
                });
            }
            match categories {
                AxisData::String(reference) => reference.validate()?,
                AxisData::Numeric(reference) => reference.validate()?,
            }
        }
        if scatter && self.categories.is_none() {
            return Err(ChartError::MissingElement("c:xVal".to_owned()));
        }
        if let Some(size) = &self.bubble_size {
            size.validate()?;
        }
        if let Some(labels) = &self.data_labels {
            labels.validate()?;
        }
        let mut writer = Writer::new(Vec::new());
        let mut start = BytesStart::new("c:ser");
        start.push_attribute(("xmlns:c", C_NS));
        start.push_attribute(("xmlns:a", A_NS));
        start.push_attribute(("xmlns:r", R_NS));
        push_attributes(&mut start, &self.namespace_declarations);
        push_attributes(&mut start, &self.raw_attributes);
        writer
            .write_event(Event::Start(start))
            .map_err(OxmlError::from)?;
        emit_raw(&mut writer, self.raw_children.at(0))?;
        write_scalar(
            &mut writer,
            "c:idx",
            &self.index.to_string(),
            Some(&self.index_markup),
        )?;
        emit_raw(&mut writer, self.raw_children.at(1))?;
        write_scalar(
            &mut writer,
            "c:order",
            &self.order.to_string(),
            Some(&self.order_markup),
        )?;
        emit_raw(&mut writer, self.raw_children.at(2))?;
        if let Some(name) = &self.name {
            write_wrapper_start(
                &mut writer,
                "c:tx",
                self.name_markup
                    .as_ref()
                    .unwrap_or(&WrapperMarkup::default()),
            )?;
            name.write_xml(&mut writer, false)?;
            write_wrapper_end(
                &mut writer,
                "c:tx",
                self.name_markup
                    .as_ref()
                    .unwrap_or(&WrapperMarkup::default()),
            )?;
        }
        emit_raw(&mut writer, self.raw_children.at(3))?;
        if let Some(properties) = &self.sp_pr {
            properties.write_xml_as(&mut writer, "c:spPr")?;
        }
        emit_raw(&mut writer, self.raw_children.at(4))?;
        for point in &self.authored_data_points {
            point.write_xml(&mut writer)?;
        }
        if let Some(labels) = &self.data_labels {
            labels.write_xml(&mut writer, false)?;
        }
        emit_raw(&mut writer, self.raw_children.at(5))?;
        if let Some(categories) = &self.categories {
            let default_markup = WrapperMarkup::default();
            let markup = self.categories_markup.as_ref().unwrap_or(&default_markup);
            write_wrapper_start(
                &mut writer,
                if scatter { "c:xVal" } else { "c:cat" },
                markup,
            )?;
            match categories {
                AxisData::String(reference) => reference.write_xml(&mut writer, false)?,
                AxisData::Numeric(reference) => reference.write_xml(&mut writer, false)?,
            }
            write_wrapper_end(
                &mut writer,
                if scatter { "c:xVal" } else { "c:cat" },
                markup,
            )?;
        }
        emit_raw(&mut writer, self.raw_children.at(6))?;
        write_wrapper_start(
            &mut writer,
            if scatter { "c:yVal" } else { "c:val" },
            &self.values_markup,
        )?;
        self.values.write_xml(&mut writer, false)?;
        write_wrapper_end(
            &mut writer,
            if scatter { "c:yVal" } else { "c:val" },
            &self.values_markup,
        )?;
        emit_raw(&mut writer, self.raw_children.at(7))?;
        if let Some(size) = &self.bubble_size {
            let default_markup = WrapperMarkup::default();
            let markup = self.bubble_size_markup.as_ref().unwrap_or(&default_markup);
            write_wrapper_start(&mut writer, "c:bubbleSize", markup)?;
            size.write_xml(&mut writer, false)?;
            write_wrapper_end(&mut writer, "c:bubbleSize", markup)?;
        }
        emit_raw(&mut writer, self.raw_children.at(8))?;
        writer
            .write_event(Event::End(BytesEnd::new("c:ser")))
            .map_err(OxmlError::from)?;
        Ok(writer.into_inner())
    }

    pub fn raw_children(&self) -> &OrderedRawChildren {
        &self.raw_children
    }
}

#[derive(Default)]
struct SeriesParseState {
    index: Option<(u32, ScalarMarkup)>,
    order: Option<(u32, ScalarMarkup)>,
    name: Option<(StringRef, WrapperMarkup)>,
    categories: Option<(AxisData, WrapperMarkup)>,
    values: Option<(NumericData, WrapperMarkup)>,
    bubble_size: Option<(NumericData, WrapperMarkup)>,
    data_labels: Option<CT_DLbls>,
    name_seen: bool,
    categories_seen: bool,
    bubble_size_seen: bool,
    opaque_name: bool,
    opaque_categories: bool,
    opaque_bubble_size: bool,
    saw_standard_wrapper: bool,
    saw_scatter_wrapper: bool,
    sp_pr: Option<CT_ShapeProperties>,
    raw_children: OrderedRawChildren,
    boundary: usize,
}

impl SeriesParseState {
    fn capture_event(&mut self, raw: Vec<u8>) {
        self.raw_children.push(self.boundary, raw);
    }

    fn parse_child(
        &mut self,
        name: &[u8],
        raw: Vec<u8>,
        namespaces: &NamespaceBindings,
    ) -> Result<()> {
        match name {
            b"idx" => {
                set_once(&mut self.index, parse_u32_scalar(&raw, "idx")?, "c:idx")?;
                self.boundary = self.boundary.max(1);
            }
            b"order" => {
                set_once(&mut self.order, parse_u32_scalar(&raw, "order")?, "c:order")?;
                self.boundary = self.boundary.max(2);
            }
            b"tx" => {
                mark_once(&mut self.name_seen, "c:tx")?;
                let parsed = parse_wrapper(&raw, b"tx", &[b"strRef"], namespaces)?;
                if let Some((_, reference)) = parsed.choice {
                    self.name = Some((
                        StringRef::from_xml_with_namespaces(&reference, &parsed.namespaces)?,
                        parsed.markup,
                    ));
                } else {
                    self.opaque_name = true;
                    self.raw_children.push(2, raw);
                }
                self.boundary = self.boundary.max(3);
            }
            b"spPr" => {
                parse_chart_shape_properties(&raw, namespaces, &mut self.sp_pr)?;
                self.boundary = self.boundary.max(4);
            }
            b"dLbls" => {
                set_once(
                    &mut self.data_labels,
                    CT_DLbls::from_xml_with_namespaces(&raw, namespaces)?,
                    "c:dLbls",
                )?;
                self.boundary = self.boundary.max(5);
            }
            b"cat" => {
                self.saw_standard_wrapper = true;
                mark_once(&mut self.categories_seen, "c:cat")?;
                let parsed = parse_wrapper(&raw, b"cat", &[b"strRef", b"numRef"], namespaces)?;
                if let Some((choice, reference)) = parsed.choice {
                    let categories = if choice == b"strRef" {
                        AxisData::String(StringRef::from_xml_with_namespaces(
                            &reference,
                            &parsed.namespaces,
                        )?)
                    } else {
                        AxisData::Numeric(NumericData::from_xml_with_namespaces(
                            &reference,
                            &parsed.namespaces,
                        )?)
                    };
                    self.categories = Some((categories, parsed.markup));
                } else {
                    self.opaque_categories = true;
                    self.raw_children.push(5, raw);
                }
                self.boundary = self.boundary.max(6);
            }
            b"xVal" => {
                self.saw_scatter_wrapper = true;
                mark_once(&mut self.categories_seen, "c:xVal")?;
                let parsed = parse_wrapper(&raw, b"xVal", &[b"numRef"], namespaces)?;
                let (_, reference) = parsed
                    .choice
                    .ok_or_else(|| ChartError::MissingElement("c:xVal/c:numRef".to_owned()))?;
                self.categories = Some((
                    AxisData::Numeric(NumericData::from_xml_with_namespaces(
                        &reference,
                        &parsed.namespaces,
                    )?),
                    parsed.markup,
                ));
                self.boundary = self.boundary.max(6);
            }
            b"val" | b"yVal" => {
                if name == b"yVal" {
                    self.saw_scatter_wrapper = true;
                } else {
                    self.saw_standard_wrapper = true;
                }
                let wrapper = if name == b"yVal" {
                    b"yVal".as_slice()
                } else {
                    b"val".as_slice()
                };
                let parsed = parse_wrapper(&raw, wrapper, &[b"numRef"], namespaces)?;
                let (_, reference) = parsed.choice.ok_or_else(|| {
                    ChartError::MissingElement(format!(
                        "c:{}/c:numRef",
                        String::from_utf8_lossy(wrapper)
                    ))
                })?;
                set_once(
                    &mut self.values,
                    (
                        NumericData::from_xml_with_namespaces(&reference, &parsed.namespaces)?,
                        parsed.markup,
                    ),
                    "c:val or c:yVal",
                )?;
                self.boundary = self.boundary.max(7);
            }
            b"bubbleSize" => {
                mark_once(&mut self.bubble_size_seen, "c:bubbleSize")?;
                let parsed = parse_wrapper(&raw, b"bubbleSize", &[b"numRef"], namespaces)?;
                if let Some((_, reference)) = parsed.choice {
                    self.bubble_size = Some((
                        NumericData::from_xml_with_namespaces(&reference, &parsed.namespaces)?,
                        parsed.markup,
                    ));
                } else {
                    self.opaque_bubble_size = true;
                    self.raw_children.push(7, raw);
                }
                self.boundary = self.boundary.max(8);
            }
            _ => {
                let boundary = series_raw_boundary(name, self.boundary);
                self.raw_children.push(boundary, raw);
                self.boundary = self.boundary.max(boundary);
            }
        }
        Ok(())
    }

    fn finish(
        self,
        raw_attributes: XmlAttributes,
        namespace_declarations: XmlAttributes,
    ) -> Result<Series> {
        if self.saw_standard_wrapper && self.saw_scatter_wrapper {
            return Err(ChartError::InvalidValue {
                element: "c:ser".to_owned(),
                value: "category/value and x/y wrappers cannot be combined".to_owned(),
            });
        }
        let (index, index_markup) = self
            .index
            .ok_or_else(|| ChartError::MissingElement("c:idx".to_owned()))?;
        let (order, order_markup) = self
            .order
            .ok_or_else(|| ChartError::MissingElement("c:order".to_owned()))?;
        let (values, values_markup) = self
            .values
            .ok_or_else(|| ChartError::MissingElement("c:val".to_owned()))?;
        let (name, name_markup) = self
            .name
            .map(|(value, markup)| (Some(value), Some(markup)))
            .unwrap_or((None, None));
        let (categories, categories_markup) = self
            .categories
            .map(|(value, markup)| (Some(value), Some(markup)))
            .unwrap_or((None, None));
        let (bubble_size, bubble_size_markup) = self
            .bubble_size
            .map(|(value, markup)| (Some(value), Some(markup)))
            .unwrap_or((None, None));
        Ok(Series {
            index,
            order,
            name,
            categories,
            values,
            bubble_size,
            sp_pr: self.sp_pr,
            data_labels: self.data_labels,
            authored_data_points: Vec::new(),
            index_markup,
            order_markup,
            name_markup,
            categories_markup,
            values_markup,
            bubble_size_markup,
            opaque_name: self.opaque_name,
            opaque_categories: self.opaque_categories,
            opaque_bubble_size: self.opaque_bubble_size,
            uses_scatter_wrappers: self.saw_scatter_wrapper,
            parsed_from_xml: true,
            raw_attributes,
            namespace_declarations,
            raw_children: raw_children_in_schema_order(&self.raw_children, 8),
        })
    }
}

fn series_raw_boundary(name: &[u8], current: usize) -> usize {
    match name {
        b"marker" | b"invertIfNegative" | b"pictureOptions" | b"explosion" | b"dPt" => 4,
        b"trendline" | b"errBars" => 5,
        b"shape" | b"smooth" => 7,
        b"extLst" => 8,
        _ => current,
    }
}

struct ParsedReference {
    formula: String,
    format_code: Option<String>,
    values: Vec<String>,
    markup: ReferenceMarkup,
}

fn parse_reference(
    xml: &[u8],
    reference_local: &[u8],
    cache_local: &[u8],
    numeric: bool,
    inherited: &NamespaceBindings,
) -> Result<ParsedReference> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element)
                if matches_local_name(element.name().as_ref(), reference_local) =>
            {
                let namespaces = typed_rewrite_bindings(&element, inherited)?;
                let raw_attributes = capture_fixed_attributes(&element, &["xmlns:c"])?;
                let mut formula: Option<(String, TextMarkup)> = None;
                let mut cache: Option<(Option<String>, Vec<String>, CacheMarkup)> = None;
                let mut raw_children = OrderedRawChildren::default();
                let mut boundary = 0usize;
                let mut inner = Vec::new();
                loop {
                    match reader
                        .read_event_into(&mut inner)
                        .map_err(OxmlError::from)?
                    {
                        Event::Start(child) => {
                            let name = chart_child_local(&child, &namespaces)?;
                            let raw = capture_element(&mut reader, &child)?;
                            match name.as_deref().unwrap_or_default() {
                                b"f" => {
                                    set_once(
                                        &mut formula,
                                        parse_text_element(&raw, b"f", &namespaces)?,
                                        "c:f",
                                    )?;
                                    boundary = boundary.max(1);
                                }
                                name if name == cache_local => {
                                    set_once(
                                        &mut cache,
                                        parse_cache(&raw, cache_local, numeric, &namespaces)?,
                                        &format!("c:{}", String::from_utf8_lossy(cache_local)),
                                    )?;
                                    boundary = boundary.max(2);
                                }
                                _ => raw_children.push(boundary, raw),
                            }
                        }
                        Event::Empty(child) => {
                            let name = chart_child_local(&child, &namespaces)?;
                            let raw = capture_empty_element(&child)?;
                            match name.as_deref().unwrap_or_default() {
                                b"f" => {
                                    set_once(
                                        &mut formula,
                                        parse_text_element(&raw, b"f", &namespaces)?,
                                        "c:f",
                                    )?;
                                    boundary = boundary.max(1);
                                }
                                name if name == cache_local => {
                                    set_once(
                                        &mut cache,
                                        parse_cache(&raw, cache_local, numeric, &namespaces)?,
                                        &format!("c:{}", String::from_utf8_lossy(cache_local)),
                                    )?;
                                    boundary = boundary.max(2);
                                }
                                _ => raw_children.push(boundary, raw),
                            }
                        }
                        event @ (Event::Text(_)
                        | Event::CData(_)
                        | Event::Comment(_)
                        | Event::PI(_)
                        | Event::GeneralRef(_)) => {
                            raw_children.push(boundary, capture_event(event)?);
                        }
                        Event::End(end)
                            if matches_local_name(end.name().as_ref(), reference_local) =>
                        {
                            let (formula, formula_markup) = formula
                                .ok_or_else(|| ChartError::MissingElement("c:f".to_owned()))?;
                            validate_formula(&formula, "c:f")?;
                            let (format_code, values, cache_markup) = cache.ok_or_else(|| {
                                ChartError::MissingElement(format!(
                                    "c:{}",
                                    String::from_utf8_lossy(cache_local)
                                ))
                            })?;
                            let markup = ReferenceMarkup {
                                raw_attributes,
                                raw_children: raw_children_in_schema_order(&raw_children, 2),
                                formula: formula_markup,
                                cache_attributes: cache_markup.raw_attributes,
                                cache_children: cache_markup.raw_children,
                                format_code: cache_markup.format_code,
                                format_code_omitted: cache_markup.format_code_omitted,
                                point_count: cache_markup.point_count,
                                declared_point_count: cache_markup.declared_point_count,
                                point_indexes: cache_markup.point_indexes,
                                points: cache_markup.points,
                            };
                            return Ok(ParsedReference {
                                formula,
                                format_code,
                                values,
                                markup,
                            });
                        }
                        Event::Eof => {
                            return Err(missing_end(&format!(
                                "c:{}",
                                String::from_utf8_lossy(reference_local)
                            )));
                        }
                        _ => {}
                    }
                    inner.clear();
                }
            }
            Event::Empty(element)
                if matches_local_name(element.name().as_ref(), reference_local) =>
            {
                return Err(ChartError::MissingElement("c:f".to_owned()));
            }
            Event::Start(element) | Event::Empty(element) => {
                return Err(ChartError::UnexpectedElement(element_name(&element)));
            }
            Event::Eof => {
                return Err(ChartError::MissingElement(format!(
                    "c:{}",
                    String::from_utf8_lossy(reference_local)
                )));
            }
            _ => {}
        }
        buffer.clear();
    }
}

struct CacheMarkup {
    raw_attributes: XmlAttributes,
    raw_children: OrderedRawChildren,
    format_code: Option<TextMarkup>,
    format_code_omitted: bool,
    point_count: ScalarMarkup,
    declared_point_count: Option<u32>,
    point_indexes: Vec<u32>,
    points: Vec<PointMarkup>,
}

fn parse_cache(
    xml: &[u8],
    cache_local: &[u8],
    numeric: bool,
    inherited: &NamespaceBindings,
) -> Result<(Option<String>, Vec<String>, CacheMarkup)> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) if matches_local_name(element.name().as_ref(), cache_local) => {
                let namespaces = typed_rewrite_bindings(&element, inherited)?;
                let raw_attributes = capture_attributes(&element)?;
                let mut state = CacheParseState::new(numeric, namespaces);
                let mut inner = Vec::new();
                loop {
                    match reader
                        .read_event_into(&mut inner)
                        .map_err(OxmlError::from)?
                    {
                        Event::Start(child) => {
                            let name = chart_child_local(&child, &state.namespaces)?;
                            let raw = capture_element(&mut reader, &child)?;
                            state.parse_child(name.as_deref().unwrap_or_default(), raw)?;
                        }
                        Event::Empty(child) => {
                            let name = chart_child_local(&child, &state.namespaces)?;
                            let raw = capture_empty_element(&child)?;
                            state.parse_child(name.as_deref().unwrap_or_default(), raw)?;
                        }
                        event @ (Event::Text(_)
                        | Event::CData(_)
                        | Event::Comment(_)
                        | Event::PI(_)
                        | Event::GeneralRef(_)) => {
                            state.capture_event(capture_event(event)?);
                        }
                        Event::End(end) if matches_local_name(end.name().as_ref(), cache_local) => {
                            return state.finish(raw_attributes);
                        }
                        Event::Eof => {
                            return Err(missing_end(&format!(
                                "c:{}",
                                String::from_utf8_lossy(cache_local)
                            )));
                        }
                        _ => {}
                    }
                    inner.clear();
                }
            }
            Event::Empty(element) if matches_local_name(element.name().as_ref(), cache_local) => {
                return Err(ChartError::MissingElement("c:ptCount".to_owned()));
            }
            Event::Start(element) | Event::Empty(element) => {
                return Err(ChartError::UnexpectedElement(element_name(&element)));
            }
            Event::Eof => {
                return Err(ChartError::MissingElement(format!(
                    "c:{}",
                    String::from_utf8_lossy(cache_local)
                )));
            }
            _ => {}
        }
        buffer.clear();
    }
}

struct CacheParseState {
    numeric: bool,
    namespaces: NamespaceBindings,
    point_base: usize,
    boundary: usize,
    format_code: Option<(String, TextMarkup)>,
    point_count: Option<(u32, ScalarMarkup)>,
    points: Vec<(u32, String, PointMarkup)>,
    raw_children: OrderedRawChildren,
}

impl CacheParseState {
    fn new(numeric: bool, namespaces: NamespaceBindings) -> Self {
        Self {
            numeric,
            namespaces,
            point_base: usize::from(numeric) + 1,
            boundary: 0,
            format_code: None,
            point_count: None,
            points: Vec::new(),
            raw_children: OrderedRawChildren::default(),
        }
    }

    fn capture_event(&mut self, raw: Vec<u8>) {
        self.raw_children.push(self.boundary, raw);
    }

    fn parse_child(&mut self, name: &[u8], raw: Vec<u8>) -> Result<()> {
        match name {
            b"formatCode" if self.numeric => {
                set_once(
                    &mut self.format_code,
                    parse_text_element(&raw, b"formatCode", &self.namespaces)?,
                    "c:formatCode",
                )?;
                self.boundary = self.boundary.max(1);
            }
            b"ptCount" => {
                set_once(
                    &mut self.point_count,
                    parse_u32_scalar(&raw, "ptCount")?,
                    "c:ptCount",
                )?;
                self.boundary = self.boundary.max(self.point_base);
            }
            b"pt" => {
                self.points.push(parse_point(&raw, &self.namespaces)?);
                self.boundary = self.boundary.max(self.point_base + self.points.len());
            }
            _ => self.raw_children.push(self.boundary, raw),
        }
        Ok(())
    }

    fn finish(
        self,
        raw_attributes: XmlAttributes,
    ) -> Result<(Option<String>, Vec<String>, CacheMarkup)> {
        let (declared, point_count_markup) = self
            .point_count
            .ok_or_else(|| ChartError::MissingElement("c:ptCount".to_owned()))?;
        let actual = u32::try_from(self.points.len()).map_err(|_| ChartError::InvalidValue {
            element: "c:ptCount".to_owned(),
            value: self.points.len().to_string(),
        })?;
        if declared < actual {
            return Err(invalid_attribute("ptCount", "val", declared.to_string()));
        }
        let mut values = Vec::with_capacity(self.points.len());
        let mut point_indexes = Vec::with_capacity(self.points.len());
        let mut point_markup = Vec::with_capacity(self.points.len());
        let mut previous = None;
        for (index, value, markup) in self.points {
            if index >= declared || previous.is_some_and(|last| index <= last) {
                return Err(invalid_attribute("pt", "idx", index.to_string()));
            }
            previous = Some(index);
            values.push(value);
            point_indexes.push(index);
            point_markup.push(markup);
        }
        let (format_code, format_markup) = self
            .format_code
            .map(|(value, markup)| (Some(value), Some(markup)))
            .unwrap_or((None, None));
        let format_code_omitted = self.numeric && format_code.is_none();
        Ok((
            format_code,
            values,
            CacheMarkup {
                raw_attributes,
                raw_children: raw_children_in_schema_order(
                    &self.raw_children,
                    self.point_base + point_markup.len(),
                ),
                format_code: format_markup,
                format_code_omitted,
                point_count: point_count_markup,
                declared_point_count: Some(declared),
                point_indexes,
                points: point_markup,
            },
        ))
    }
}

fn parse_point(xml: &[u8], inherited: &NamespaceBindings) -> Result<(u32, String, PointMarkup)> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) if matches_local_name(element.name().as_ref(), b"pt") => {
                let namespaces = typed_rewrite_bindings(&element, inherited)?;
                let index = required_u32_attribute(&element, "pt", b"idx")?;
                let raw_attributes = capture_attributes_except(&element, b"idx")?;
                let mut value: Option<(String, TextMarkup)> = None;
                let mut raw_children = OrderedRawChildren::default();
                let mut boundary = 0usize;
                let mut inner = Vec::new();
                loop {
                    match reader
                        .read_event_into(&mut inner)
                        .map_err(OxmlError::from)?
                    {
                        Event::Start(child) => {
                            let name = chart_child_local(&child, &namespaces)?;
                            let raw = capture_element(&mut reader, &child)?;
                            if name.as_deref() == Some(b"v") {
                                set_once(
                                    &mut value,
                                    parse_text_element(&raw, b"v", &namespaces)?,
                                    "c:v",
                                )?;
                                boundary = 1;
                            } else {
                                raw_children.push(boundary, raw);
                            }
                        }
                        Event::Empty(child) => {
                            let name = chart_child_local(&child, &namespaces)?;
                            let raw = capture_empty_element(&child)?;
                            if name.as_deref() == Some(b"v") {
                                set_once(
                                    &mut value,
                                    parse_text_element(&raw, b"v", &namespaces)?,
                                    "c:v",
                                )?;
                                boundary = 1;
                            } else {
                                raw_children.push(boundary, raw);
                            }
                        }
                        event @ (Event::Text(_)
                        | Event::CData(_)
                        | Event::Comment(_)
                        | Event::PI(_)
                        | Event::GeneralRef(_)) => {
                            raw_children.push(boundary, capture_event(event)?);
                        }
                        Event::End(end) if matches_local_name(end.name().as_ref(), b"pt") => {
                            let (value, value_markup) = value
                                .ok_or_else(|| ChartError::MissingElement("c:v".to_owned()))?;
                            return Ok((
                                index,
                                value,
                                PointMarkup {
                                    raw_attributes,
                                    raw_children: raw_children_in_schema_order(&raw_children, 1),
                                    value: value_markup,
                                },
                            ));
                        }
                        Event::Eof => return Err(missing_end("c:pt")),
                        _ => {}
                    }
                    inner.clear();
                }
            }
            Event::Empty(element) if matches_local_name(element.name().as_ref(), b"pt") => {
                return Err(ChartError::MissingElement("c:v".to_owned()));
            }
            Event::Start(element) | Event::Empty(element) => {
                return Err(ChartError::UnexpectedElement(element_name(&element)));
            }
            Event::Eof => return Err(ChartError::MissingElement("c:pt".to_owned())),
            _ => {}
        }
        buffer.clear();
    }
}

fn parse_text_element(
    xml: &[u8],
    local: &[u8],
    inherited: &NamespaceBindings,
) -> Result<(String, TextMarkup)> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) if matches_local_name(element.name().as_ref(), local) => {
                typed_rewrite_bindings(&element, inherited)?;
                let raw_attributes = capture_attributes(&element)?;
                let mut value = String::new();
                let mut raw_children = OrderedRawChildren::default();
                let mut seen_text = false;
                let mut inner = Vec::new();
                loop {
                    match reader
                        .read_event_into(&mut inner)
                        .map_err(OxmlError::from)?
                    {
                        Event::Text(text) => {
                            let decoded =
                                text.xml10_content()
                                    .map_err(|error| ChartError::InvalidValue {
                                        element: format!("c:{}", String::from_utf8_lossy(local)),
                                        value: error.to_string(),
                                    })?;
                            value.push_str(&decoded);
                            seen_text = true;
                        }
                        Event::CData(text) => {
                            let decoded =
                                text.xml10_content()
                                    .map_err(|error| ChartError::InvalidValue {
                                        element: format!("c:{}", String::from_utf8_lossy(local)),
                                        value: error.to_string(),
                                    })?;
                            value.push_str(&decoded);
                            seen_text = true;
                        }
                        Event::GeneralRef(reference) => {
                            value.push_str(&resolve_entity(&reference));
                            seen_text = true;
                        }
                        event @ (Event::Comment(_) | Event::PI(_)) => {
                            raw_children.push(usize::from(seen_text), capture_event(event)?);
                        }
                        Event::Start(child) | Event::Empty(child) => {
                            return Err(ChartError::UnexpectedElement(element_name(&child)));
                        }
                        Event::End(end) if matches_local_name(end.name().as_ref(), local) => {
                            return Ok((
                                value,
                                TextMarkup {
                                    raw_attributes,
                                    raw_children: raw_children_in_schema_order(&raw_children, 1),
                                },
                            ));
                        }
                        Event::Eof => {
                            return Err(missing_end(&format!(
                                "c:{}",
                                String::from_utf8_lossy(local)
                            )));
                        }
                        _ => {}
                    }
                    inner.clear();
                }
            }
            Event::Empty(element) if matches_local_name(element.name().as_ref(), local) => {
                typed_rewrite_bindings(&element, inherited)?;
                return Ok((
                    String::new(),
                    TextMarkup {
                        raw_attributes: capture_attributes(&element)?,
                        raw_children: OrderedRawChildren::default(),
                    },
                ));
            }
            Event::Start(element) | Event::Empty(element) => {
                return Err(ChartError::UnexpectedElement(element_name(&element)));
            }
            Event::Eof => {
                return Err(ChartError::MissingElement(format!(
                    "c:{}",
                    String::from_utf8_lossy(local)
                )));
            }
            _ => {}
        }
        buffer.clear();
    }
}

struct ParsedWrapper {
    choice: Option<(Vec<u8>, Vec<u8>)>,
    markup: WrapperMarkup,
    namespaces: NamespaceBindings,
}

fn parse_wrapper(
    xml: &[u8],
    local: &[u8],
    choices: &[&[u8]],
    inherited: &NamespaceBindings,
) -> Result<ParsedWrapper> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) if matches_local_name(element.name().as_ref(), local) => {
                let namespaces = typed_rewrite_bindings(&element, inherited)?;
                let raw_attributes = capture_attributes(&element)?;
                let mut choice = None;
                let mut raw_children = OrderedRawChildren::default();
                let mut boundary = 0usize;
                let mut inner = Vec::new();
                loop {
                    match reader
                        .read_event_into(&mut inner)
                        .map_err(OxmlError::from)?
                    {
                        Event::Start(child) => {
                            let name = chart_child_local(&child, &namespaces)?;
                            let raw = capture_element(&mut reader, &child)?;
                            if let Some(name) =
                                name.filter(|name| choices.contains(&name.as_slice()))
                            {
                                if choice.is_some() {
                                    return Err(ChartError::DuplicateElement(format!(
                                        "c:{} reference",
                                        String::from_utf8_lossy(local)
                                    )));
                                }
                                choice = Some((name, raw));
                                boundary = 1;
                            } else {
                                raw_children.push(boundary, raw);
                            }
                        }
                        Event::Empty(child) => {
                            let name = chart_child_local(&child, &namespaces)?;
                            let raw = capture_empty_element(&child)?;
                            if let Some(name) =
                                name.filter(|name| choices.contains(&name.as_slice()))
                            {
                                if choice.is_some() {
                                    return Err(ChartError::DuplicateElement(format!(
                                        "c:{} reference",
                                        String::from_utf8_lossy(local)
                                    )));
                                }
                                choice = Some((name, raw));
                                boundary = 1;
                            } else {
                                raw_children.push(boundary, raw);
                            }
                        }
                        event @ (Event::Text(_)
                        | Event::CData(_)
                        | Event::Comment(_)
                        | Event::PI(_)
                        | Event::GeneralRef(_)) => {
                            raw_children.push(boundary, capture_event(event)?);
                        }
                        Event::End(end) if matches_local_name(end.name().as_ref(), local) => {
                            return Ok(ParsedWrapper {
                                choice,
                                markup: WrapperMarkup {
                                    raw_attributes,
                                    raw_children: raw_children_in_schema_order(&raw_children, 1),
                                },
                                namespaces,
                            });
                        }
                        Event::Eof => {
                            return Err(missing_end(&format!(
                                "c:{}",
                                String::from_utf8_lossy(local)
                            )));
                        }
                        _ => {}
                    }
                    inner.clear();
                }
            }
            Event::Empty(element) if matches_local_name(element.name().as_ref(), local) => {
                let namespaces = chart_bindings(inherited, &element)?;
                return Ok(ParsedWrapper {
                    choice: None,
                    markup: WrapperMarkup {
                        raw_attributes: capture_attributes(&element)?,
                        raw_children: OrderedRawChildren::default(),
                    },
                    namespaces,
                });
            }
            Event::Start(element) | Event::Empty(element) => {
                return Err(ChartError::UnexpectedElement(element_name(&element)));
            }
            Event::Eof => {
                return Err(ChartError::MissingElement(format!(
                    "c:{}",
                    String::from_utf8_lossy(local)
                )));
            }
            _ => {}
        }
        buffer.clear();
    }
}

fn write_string_cache(
    writer: &mut Writer<Vec<u8>>,
    values: &[String],
    markup: &ReferenceMarkup,
) -> Result<()> {
    let (declared_count, indexes) = cache_layout(markup, values.len())?;
    let mut start = BytesStart::new("c:strCache");
    push_attributes(&mut start, &markup.cache_attributes);
    writer
        .write_event(Event::Start(start))
        .map_err(OxmlError::from)?;
    emit_raw(writer, markup.cache_children.at(0))?;
    write_scalar(
        writer,
        "c:ptCount",
        &declared_count.to_string(),
        Some(&markup.point_count),
    )?;
    emit_cache_point_raw(writer, markup, 1, values.len(), 0)?;
    for (position, value) in values.iter().enumerate() {
        let default_markup = PointMarkup::default();
        write_point(
            writer,
            indexes[position],
            value,
            markup.points.get(position).unwrap_or(&default_markup),
        )?;
        emit_cache_point_raw(writer, markup, 1, values.len(), position + 1)?;
    }
    writer
        .write_event(Event::End(BytesEnd::new("c:strCache")))
        .map_err(OxmlError::from)?;
    Ok(())
}

fn write_numeric_cache(
    writer: &mut Writer<Vec<u8>>,
    data: &NumericData,
    markup: &ReferenceMarkup,
) -> Result<()> {
    let (declared_count, indexes) = cache_layout(markup, data.values.len())?;
    let mut start = BytesStart::new("c:numCache");
    push_attributes(&mut start, &markup.cache_attributes);
    writer
        .write_event(Event::Start(start))
        .map_err(OxmlError::from)?;
    emit_raw(writer, markup.cache_children.at(0))?;
    if !markup.format_code_omitted || data.format_code != "General" {
        let default_format_markup = TextMarkup::default();
        write_text(
            writer,
            "c:formatCode",
            &data.format_code,
            markup
                .format_code
                .as_ref()
                .unwrap_or(&default_format_markup),
        )?;
    }
    emit_raw(writer, markup.cache_children.at(1))?;
    write_scalar(
        writer,
        "c:ptCount",
        &declared_count.to_string(),
        Some(&markup.point_count),
    )?;
    emit_cache_point_raw(writer, markup, 2, data.values.len(), 0)?;
    for (position, value) in data.values.iter().enumerate() {
        let default_markup = PointMarkup::default();
        write_point(
            writer,
            indexes[position],
            &value.to_string(),
            markup.points.get(position).unwrap_or(&default_markup),
        )?;
        emit_cache_point_raw(writer, markup, 2, data.values.len(), position + 1)?;
    }
    writer
        .write_event(Event::End(BytesEnd::new("c:numCache")))
        .map_err(OxmlError::from)?;
    Ok(())
}

fn emit_cache_point_raw(
    writer: &mut Writer<Vec<u8>>,
    markup: &ReferenceMarkup,
    point_base: usize,
    current_count: usize,
    completed_points: usize,
) -> Result<()> {
    let original_count = markup.points.len();
    if current_count == 0 {
        for boundary in point_base..=point_base + original_count {
            emit_raw(writer, markup.cache_children.at(boundary))?;
        }
    } else if completed_points == 0 {
        if original_count > 0 {
            emit_raw(writer, markup.cache_children.at(point_base))?;
        }
    } else if completed_points < current_count {
        if completed_points < original_count {
            emit_raw(
                writer,
                markup.cache_children.at(point_base + completed_points),
            )?;
        }
    } else {
        let first_tail = if original_count == 0 {
            point_base
        } else {
            point_base + current_count.min(original_count)
        };
        for boundary in first_tail..=point_base + original_count {
            emit_raw(writer, markup.cache_children.at(boundary))?;
        }
    }
    Ok(())
}

fn write_point(
    writer: &mut Writer<Vec<u8>>,
    index: u32,
    value: &str,
    markup: &PointMarkup,
) -> Result<()> {
    let index = index.to_string();
    let mut start = BytesStart::new("c:pt");
    start.push_attribute(("idx", index.as_str()));
    push_attributes(&mut start, &markup.raw_attributes);
    writer
        .write_event(Event::Start(start))
        .map_err(OxmlError::from)?;
    emit_raw(writer, markup.raw_children.at(0))?;
    write_text(writer, "c:v", value, &markup.value)?;
    emit_raw(writer, markup.raw_children.at(1))?;
    writer
        .write_event(Event::End(BytesEnd::new("c:pt")))
        .map_err(OxmlError::from)?;
    Ok(())
}

fn cache_layout(markup: &ReferenceMarkup, value_count: usize) -> Result<(u32, Vec<u32>)> {
    if markup.point_indexes.len() == value_count
        && let Some(declared) = markup.declared_point_count
    {
        return Ok((declared, markup.point_indexes.clone()));
    }
    let declared = u32::try_from(value_count).map_err(|_| ChartError::InvalidValue {
        element: "c:ptCount".to_owned(),
        value: value_count.to_string(),
    })?;
    Ok((declared, (0..declared).collect()))
}

fn write_text(
    writer: &mut Writer<Vec<u8>>,
    tag: &str,
    value: &str,
    markup: &TextMarkup,
) -> Result<()> {
    let mut start = BytesStart::new(tag);
    push_attributes(&mut start, &markup.raw_attributes);
    writer
        .write_event(Event::Start(start))
        .map_err(OxmlError::from)?;
    emit_raw(writer, markup.raw_children.at(0))?;
    writer
        .write_event(Event::Text(BytesText::from_escaped(escape_text(value))))
        .map_err(OxmlError::from)?;
    emit_raw(writer, markup.raw_children.at(1))?;
    writer
        .write_event(Event::End(BytesEnd::new(tag)))
        .map_err(OxmlError::from)?;
    Ok(())
}

fn write_wrapper_start(
    writer: &mut Writer<Vec<u8>>,
    tag: &str,
    markup: &WrapperMarkup,
) -> Result<()> {
    let mut start = BytesStart::new(tag);
    push_attributes(&mut start, &markup.raw_attributes);
    writer
        .write_event(Event::Start(start))
        .map_err(OxmlError::from)?;
    Ok(emit_raw(writer, markup.raw_children.at(0))?)
}

fn write_wrapper_end(
    writer: &mut Writer<Vec<u8>>,
    tag: &str,
    markup: &WrapperMarkup,
) -> Result<()> {
    emit_raw(writer, markup.raw_children.at(1))?;
    writer
        .write_event(Event::End(BytesEnd::new(tag)))
        .map_err(OxmlError::from)?;
    Ok(())
}

fn validate_formula(formula: &str, element: &str) -> Result<()> {
    if formula.trim().is_empty() {
        return Err(ChartError::InvalidValue {
            element: element.to_owned(),
            value: formula.to_owned(),
        });
    }
    validate_xml_text(formula, element)
}

fn validate_format_code(format_code: &str) -> Result<()> {
    validate_xml_text(format_code, "c:formatCode")
}

fn validate_xml_text(value: &str, element: &str) -> Result<()> {
    if value.chars().all(|character| {
        matches!(
            character,
            '\u{9}' | '\u{A}' | '\u{D}' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}'
        )
    }) {
        return Ok(());
    }
    Err(ChartError::InvalidValue {
        element: element.to_owned(),
        value: value.to_owned(),
    })
}

fn validate_numeric_values(values: &[f64]) -> Result<()> {
    if let Some(value) = values.iter().find(|value| !value.is_finite()) {
        return Err(ChartError::InvalidValue {
            element: "c:v".to_owned(),
            value: value.to_string(),
        });
    }
    Ok(())
}

fn validate_point_count(count: usize, element: &str) -> Result<()> {
    u32::try_from(count)
        .map(|_| ())
        .map_err(|_| ChartError::InvalidValue {
            element: element.to_owned(),
            value: count.to_string(),
        })
}

fn parse_u32_scalar(xml: &[u8], local: &str) -> Result<(u32, ScalarMarkup)> {
    let (value, markup) = scalar_value(xml, local)?;
    let value = value.ok_or_else(|| invalid_attribute(local, "val", "<missing>".to_owned()))?;
    let parsed = value
        .parse::<u32>()
        .map_err(|_| invalid_attribute(local, "val", value))?;
    Ok((parsed, markup))
}

fn required_u32_attribute(element: &BytesStart<'_>, local: &str, attribute: &[u8]) -> Result<u32> {
    let value = attribute_value(element, attribute)?.ok_or_else(|| {
        invalid_attribute(
            local,
            &String::from_utf8_lossy(attribute),
            "<missing>".to_owned(),
        )
    })?;
    value
        .parse::<u32>()
        .map_err(|_| invalid_attribute(local, &String::from_utf8_lossy(attribute), value))
}

fn chart_root_prefix(element: &BytesStart<'_>) -> Result<Vec<u8>> {
    let name = element.name();
    let qualified = name.as_ref();
    let prefix = qualified
        .iter()
        .position(|byte| *byte == b':')
        .map(|position| qualified[..position].to_vec())
        .unwrap_or_default();
    let declaration = if prefix.is_empty() {
        b"xmlns".to_vec()
    } else {
        let mut declaration = b"xmlns:".to_vec();
        declaration.extend_from_slice(&prefix);
        declaration
    };
    if let Some(namespace) = attribute_value(element, &declaration)?
        && namespace != C_NS
    {
        return Err(invalid_attribute(
            &element_name(element),
            "namespace",
            namespace,
        ));
    }
    reject_conflicting_prefix(element, b"c", C_NS)?;
    Ok(prefix)
}

fn typed_rewrite_bindings(
    element: &BytesStart<'_>,
    inherited: &NamespaceBindings,
) -> Result<NamespaceBindings> {
    chart_root_prefix(element)?;
    if !element_is_in_namespace(element, C_NS, inherited)? {
        return Err(ChartError::UnexpectedElement(element_name(element)));
    }
    let bindings = chart_bindings(inherited, element)?;
    require_fixed_namespace(&bindings, b"c", C_NS, element)?;
    Ok(bindings)
}

fn chart_namespace_defaults() -> NamespaceBindings {
    vec![
        (b"c".to_vec(), C_NS.to_owned()),
        (b"a".to_vec(), A_NS.to_owned()),
        (b"r".to_vec(), R_NS.to_owned()),
    ]
}

fn chart_bindings(
    inherited: &NamespaceBindings,
    element: &BytesStart<'_>,
) -> Result<NamespaceBindings> {
    let mut bindings = chart_namespace_defaults();
    for (prefix, namespace) in inherited
        .iter()
        .cloned()
        .chain(namespace_bindings(element)?)
    {
        upsert_namespace_binding(&mut bindings, prefix, namespace);
    }
    Ok(bindings)
}

fn root_chart_bindings(
    xml: &[u8],
    local: &[u8],
    inherited: &NamespaceBindings,
) -> Result<NamespaceBindings> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) | Event::Empty(element)
                if matches_local_name(element.name().as_ref(), local) =>
            {
                if !element_is_in_namespace(&element, C_NS, inherited)? {
                    return Err(ChartError::UnexpectedElement(element_name(&element)));
                }
                return chart_bindings(inherited, &element);
            }
            Event::Start(element) | Event::Empty(element) => {
                return Err(ChartError::UnexpectedElement(element_name(&element)));
            }
            Event::Eof => {
                return Err(ChartError::MissingElement(format!(
                    "c:{}",
                    String::from_utf8_lossy(local)
                )));
            }
            _ => {}
        }
        buffer.clear();
    }
}

fn chart_child_local(
    element: &BytesStart<'_>,
    inherited: &NamespaceBindings,
) -> Result<Option<Vec<u8>>> {
    Ok(element_is_in_namespace(element, C_NS, inherited)?
        .then(|| local_name(element.name().as_ref()).to_vec()))
}

fn capture_fixed_root_attributes(
    start: &BytesStart<'_>,
    fixed: &[&str],
) -> Result<(XmlAttributes, XmlAttributes)> {
    let mut attributes = Vec::new();
    let mut namespaces = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(OxmlError::from)?;
        let name = std::str::from_utf8(attribute.key.as_ref())
            .map_err(OxmlError::from)?
            .to_owned();
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
            .map_err(OxmlError::from)?
            .into_owned();
        if fixed.contains(&name.as_str()) {
            continue;
        }
        if name == "xmlns" || name.starts_with("xmlns:") {
            namespaces.push((name, value));
        } else {
            attributes.push((name, value));
        }
    }
    Ok((attributes, namespaces))
}

fn require_fixed_namespace(
    bindings: &NamespaceBindings,
    prefix: &[u8],
    expected: &str,
    element: &BytesStart<'_>,
) -> Result<()> {
    let actual = bindings
        .iter()
        .find(|(candidate, _)| candidate.as_slice() == prefix)
        .map(|(_, namespace)| namespace.as_str());
    if actual == Some(expected) {
        return Ok(());
    }
    Err(invalid_attribute(
        &element_name(element),
        &format!("xmlns:{}", String::from_utf8_lossy(prefix)),
        actual.unwrap_or("<missing>").to_owned(),
    ))
}

fn standalone_namespace_declarations(bindings: &NamespaceBindings) -> Result<XmlAttributes> {
    bindings
        .iter()
        .filter(|(prefix, _)| prefix != b"c" && prefix != b"a" && prefix != b"r")
        .map(|(prefix, namespace)| {
            let prefix = std::str::from_utf8(prefix).map_err(OxmlError::from)?;
            let name = if prefix.is_empty() {
                "xmlns".to_owned()
            } else {
                format!("xmlns:{prefix}")
            };
            Ok((name, namespace.clone()))
        })
        .collect()
}

fn capture_fixed_attributes(start: &BytesStart<'_>, fixed: &[&str]) -> Result<XmlAttributes> {
    let mut attributes = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(OxmlError::from)?;
        let name = std::str::from_utf8(attribute.key.as_ref())
            .map_err(OxmlError::from)?
            .to_owned();
        if fixed.contains(&name.as_str()) {
            continue;
        }
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
            .map_err(OxmlError::from)?
            .into_owned();
        attributes.push((name, value));
    }
    Ok(attributes)
}

/// A producer-compatible ChartML axis identifier.
///
/// ECMA-376 names an unsigned value, but PowerPoint emits signed 32-bit
/// lexical values in real chart parts. The accepted domain covers both forms.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AxisId(i64);

impl AxisId {
    pub fn new(value: i64) -> Result<Self> {
        if (i64::from(i32::MIN)..=i64::from(u32::MAX)).contains(&value) {
            Ok(Self(value))
        } else {
            Err(ChartError::InvalidValue {
                element: "c:axId".to_owned(),
                value: value.to_string(),
            })
        }
    }

    pub const fn value(self) -> i64 {
        self.0
    }
}

/// Whether bars extend along the category or value axis.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BarDirection {
    Bar,
    Column,
}

impl BarDirection {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "bar" => Some(Self::Bar),
            "col" => Some(Self::Column),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Bar => "bar",
            Self::Column => "col",
        }
    }
}

/// Grouping modes supported by a two-dimensional bar plot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BarGrouping {
    Clustered,
    PercentStacked,
    Stacked,
    Standard,
}

impl BarGrouping {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "clustered" => Some(Self::Clustered),
            "percentStacked" => Some(Self::PercentStacked),
            "stacked" => Some(Self::Stacked),
            "standard" => Some(Self::Standard),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Clustered => "clustered",
            Self::PercentStacked => "percentStacked",
            Self::Stacked => "stacked",
            Self::Standard => "standard",
        }
    }
}

/// Grouping modes shared by line and later area plots.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Grouping {
    PercentStacked,
    Stacked,
    Standard,
}

impl Grouping {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "percentStacked" => Some(Self::PercentStacked),
            "stacked" => Some(Self::Stacked),
            "standard" => Some(Self::Standard),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::PercentStacked => "percentStacked",
            Self::Stacked => "stacked",
            Self::Standard => "standard",
        }
    }
}

/// Display style for a two-dimensional scatter plot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScatterStyle {
    Line,
    LineMarker,
    Marker,
    None,
    Smooth,
    SmoothMarker,
}

impl ScatterStyle {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "line" => Some(Self::Line),
            "lineMarker" => Some(Self::LineMarker),
            "marker" => Some(Self::Marker),
            "none" => Some(Self::None),
            "smooth" => Some(Self::Smooth),
            "smoothMarker" => Some(Self::SmoothMarker),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Line => "line",
            Self::LineMarker => "lineMarker",
            Self::Marker => "marker",
            Self::None => "none",
            Self::Smooth => "smooth",
            Self::SmoothMarker => "smoothMarker",
        }
    }
}

/// Display style for a two-dimensional radar plot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadarStyle {
    Filled,
    Marker,
    Standard,
}

impl RadarStyle {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "filled" => Some(Self::Filled),
            "marker" => Some(Self::Marker),
            "standard" => Some(Self::Standard),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Filled => "filled",
            Self::Marker => "marker",
            Self::Standard => "standard",
        }
    }
}

/// A supported two-dimensional plot owned by one plot area.
#[derive(Clone, Debug, PartialEq)]
pub enum Plot {
    Bar {
        direction: BarDirection,
        grouping: BarGrouping,
        gap_width: u16,
        overlap: i8,
        series: Vec<Series>,
        data_labels: Option<CT_DLbls>,
        axis_ids: [AxisId; 2],
    },
    Line {
        grouping: Grouping,
        marker: bool,
        smooth: bool,
        series: Vec<Series>,
        data_labels: Option<CT_DLbls>,
        axis_ids: [AxisId; 2],
    },
    Pie {
        first_slice_angle: u16,
        series: Vec<Series>,
        data_labels: Option<CT_DLbls>,
    },
    Doughnut {
        first_slice_angle: u16,
        hole_size: u8,
        series: Vec<Series>,
        data_labels: Option<CT_DLbls>,
    },
    Area {
        grouping: Grouping,
        series: Vec<Series>,
        data_labels: Option<CT_DLbls>,
        axis_ids: [AxisId; 2],
    },
    Scatter {
        style: ScatterStyle,
        series: Vec<Series>,
        data_labels: Option<CT_DLbls>,
        axis_ids: [AxisId; 2],
    },
    Radar {
        style: RadarStyle,
        series: Vec<Series>,
        data_labels: Option<CT_DLbls>,
        axis_ids: [AxisId; 2],
    },
}

impl Plot {
    pub fn bar(
        direction: BarDirection,
        grouping: BarGrouping,
        series: Vec<Series>,
        axis_ids: [AxisId; 2],
    ) -> Result<Self> {
        let plot = Self::Bar {
            direction,
            grouping,
            gap_width: 150,
            overlap: 0,
            series,
            data_labels: None,
            axis_ids,
        };
        plot.validate()?;
        Ok(plot)
    }

    pub fn line(grouping: Grouping, series: Vec<Series>, axis_ids: [AxisId; 2]) -> Result<Self> {
        let plot = Self::Line {
            grouping,
            marker: false,
            smooth: false,
            series,
            data_labels: None,
            axis_ids,
        };
        plot.validate()?;
        Ok(plot)
    }

    pub fn pie(series: Vec<Series>) -> Result<Self> {
        let plot = Self::Pie {
            first_slice_angle: 0,
            series,
            data_labels: None,
        };
        plot.validate()?;
        Ok(plot)
    }

    pub fn doughnut(series: Vec<Series>) -> Result<Self> {
        let plot = Self::Doughnut {
            first_slice_angle: 0,
            hole_size: 50,
            series,
            data_labels: None,
        };
        plot.validate()?;
        Ok(plot)
    }

    pub fn area(grouping: Grouping, series: Vec<Series>, axis_ids: [AxisId; 2]) -> Result<Self> {
        let plot = Self::Area {
            grouping,
            series,
            data_labels: None,
            axis_ids,
        };
        plot.validate()?;
        Ok(plot)
    }

    pub fn scatter(
        style: ScatterStyle,
        series: Vec<Series>,
        axis_ids: [AxisId; 2],
    ) -> Result<Self> {
        let plot = Self::Scatter {
            style,
            series,
            data_labels: None,
            axis_ids,
        };
        plot.validate()?;
        Ok(plot)
    }

    pub fn radar(style: RadarStyle, series: Vec<Series>, axis_ids: [AxisId; 2]) -> Result<Self> {
        let plot = Self::Radar {
            style,
            series,
            data_labels: None,
            axis_ids,
        };
        plot.validate()?;
        Ok(plot)
    }

    fn axis_ids(&self) -> Option<[AxisId; 2]> {
        match self {
            Self::Bar { axis_ids, .. }
            | Self::Line { axis_ids, .. }
            | Self::Area { axis_ids, .. }
            | Self::Scatter { axis_ids, .. }
            | Self::Radar { axis_ids, .. } => Some(*axis_ids),
            Self::Pie { .. } | Self::Doughnut { .. } => None,
        }
    }

    fn validate(&self) -> Result<()> {
        let (series, labels, axis_ids) = match self {
            Self::Bar {
                gap_width,
                overlap,
                series,
                data_labels,
                axis_ids,
                ..
            } => {
                if *gap_width > 500 {
                    return Err(ChartError::InvalidValue {
                        element: "c:gapWidth".to_owned(),
                        value: gap_width.to_string(),
                    });
                }
                if !(-100..=100).contains(overlap) {
                    return Err(ChartError::InvalidValue {
                        element: "c:overlap".to_owned(),
                        value: overlap.to_string(),
                    });
                }
                (series, data_labels, Some(axis_ids))
            }
            Self::Line {
                series,
                data_labels,
                axis_ids,
                ..
            } => (series, data_labels, Some(axis_ids)),
            Self::Pie {
                first_slice_angle,
                series,
                data_labels,
            } => {
                if *first_slice_angle > 360 {
                    return Err(ChartError::InvalidValue {
                        element: "c:firstSliceAng".to_owned(),
                        value: first_slice_angle.to_string(),
                    });
                }
                (series, data_labels, None)
            }
            Self::Doughnut {
                first_slice_angle,
                hole_size,
                series,
                data_labels,
            } => {
                if *first_slice_angle > 360 {
                    return Err(ChartError::InvalidValue {
                        element: "c:firstSliceAng".to_owned(),
                        value: first_slice_angle.to_string(),
                    });
                }
                if !(10..=90).contains(hole_size) {
                    return Err(ChartError::InvalidValue {
                        element: "c:holeSize".to_owned(),
                        value: hole_size.to_string(),
                    });
                }
                (series, data_labels, None)
            }
            Self::Area {
                series,
                data_labels,
                axis_ids,
                ..
            }
            | Self::Radar {
                series,
                data_labels,
                axis_ids,
                ..
            } => (series, data_labels, Some(axis_ids)),
            Self::Scatter {
                series,
                data_labels,
                axis_ids,
                ..
            } => {
                for item in series {
                    if !matches!(item.categories, Some(AxisData::Numeric(_))) {
                        return Err(ChartError::InvalidValue {
                            element: "c:xVal".to_owned(),
                            value: "scatter series requires numeric x values".to_owned(),
                        });
                    }
                }
                (series, data_labels, Some(axis_ids))
            }
        };
        if series.is_empty() {
            return Err(ChartError::MissingElement("c:ser".to_owned()));
        }
        let scatter_plot = matches!(self, Self::Scatter { .. });
        for item in series {
            if item.bubble_size.is_some() || item.opaque_bubble_size {
                return Err(ChartError::InvalidValue {
                    element: "c:bubbleSize".to_owned(),
                    value: "bubble size is only valid in an opaque bubble plot".to_owned(),
                });
            }
            if item.uses_scatter_wrappers && !scatter_plot {
                return Err(ChartError::InvalidValue {
                    element: "c:ser".to_owned(),
                    value: "x/y wrappers are only valid in scatter plots".to_owned(),
                });
            }
            if scatter_plot && item.parsed_from_xml && !item.uses_scatter_wrappers {
                return Err(ChartError::InvalidValue {
                    element: "c:ser".to_owned(),
                    value: "scatter plots require xVal/yVal wrappers".to_owned(),
                });
            }
        }
        if let Some(axis_ids) = axis_ids {
            if axis_ids[0] == axis_ids[1] {
                return Err(ChartError::DuplicateElement(format!(
                    "c:axId {}",
                    axis_ids[0].value()
                )));
            }
            for id in axis_ids {
                AxisId::new(id.value())?;
            }
        }
        if let Some(labels) = labels {
            labels.validate()?;
        }
        for item in series {
            item.values.validate()?;
        }
        Ok(())
    }
}

/// The concrete ChartML root used by an axis.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AxisKind {
    Category,
    Value,
    Date,
    Series,
}

impl AxisKind {
    fn parse(local: &[u8]) -> Option<Self> {
        match local {
            b"catAx" => Some(Self::Category),
            b"valAx" => Some(Self::Value),
            b"dateAx" => Some(Self::Date),
            b"serAx" => Some(Self::Series),
            _ => None,
        }
    }

    const fn local(self) -> &'static str {
        match self {
            Self::Category => "catAx",
            Self::Value => "valAx",
            Self::Date => "dateAx",
            Self::Series => "serAx",
        }
    }
}

/// Direction of values along an axis scale.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Orientation {
    #[default]
    MinMax,
    MaxMin,
}

impl Orientation {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "minMax" => Some(Self::MinMax),
            "maxMin" => Some(Self::MaxMin),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::MinMax => "minMax",
            Self::MaxMin => "maxMin",
        }
    }
}

/// Which side of the plot area carries an axis.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AxisPosition {
    Bottom,
    Left,
    Right,
    Top,
}

impl AxisPosition {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "b" => Some(Self::Bottom),
            "l" => Some(Self::Left),
            "r" => Some(Self::Right),
            "t" => Some(Self::Top),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Bottom => "b",
            Self::Left => "l",
            Self::Right => "r",
            Self::Top => "t",
        }
    }
}

/// Placement of a major or minor tick relative to its axis line.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TickMark {
    Cross,
    Inside,
    #[default]
    None,
    Outside,
}

impl TickMark {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "cross" => Some(Self::Cross),
            "in" => Some(Self::Inside),
            "none" => Some(Self::None),
            "out" => Some(Self::Outside),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Cross => "cross",
            Self::Inside => "in",
            Self::None => "none",
            Self::Outside => "out",
        }
    }
}

/// Placement of tick labels relative to an axis.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TickLabelPosition {
    High,
    Low,
    #[default]
    NextTo,
    None,
}

impl TickLabelPosition {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "high" => Some(Self::High),
            "low" => Some(Self::Low),
            "nextTo" => Some(Self::NextTo),
            "none" => Some(Self::None),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Low => "low",
            Self::NextTo => "nextTo",
            Self::None => "none",
        }
    }
}

/// The modelled values of `c:scaling`.
#[derive(Clone, Debug)]
pub struct Scaling {
    pub log_base: Option<f64>,
    pub orientation: Orientation,
    pub maximum: Option<f64>,
    pub minimum: Option<f64>,
    log_base_markup: Option<ScalarMarkup>,
    orientation_markup: Option<ScalarMarkup>,
    maximum_markup: Option<ScalarMarkup>,
    minimum_markup: Option<ScalarMarkup>,
    raw_attributes: XmlAttributes,
    raw_children: OrderedRawChildren,
}

impl PartialEq for Scaling {
    fn eq(&self, other: &Self) -> bool {
        self.log_base == other.log_base
            && self.orientation == other.orientation
            && self.maximum == other.maximum
            && self.minimum == other.minimum
            && optional_scalar_markup_eq(&self.log_base_markup, &other.log_base_markup)
            && optional_scalar_markup_eq(&self.orientation_markup, &other.orientation_markup)
            && optional_scalar_markup_eq(&self.maximum_markup, &other.maximum_markup)
            && optional_scalar_markup_eq(&self.minimum_markup, &other.minimum_markup)
            && self.raw_attributes == other.raw_attributes
            && self.raw_children == other.raw_children
    }
}

impl Default for Scaling {
    fn default() -> Self {
        Self {
            log_base: None,
            orientation: Orientation::MinMax,
            maximum: None,
            minimum: None,
            log_base_markup: None,
            orientation_markup: None,
            maximum_markup: None,
            minimum_markup: None,
            raw_attributes: Vec::new(),
            raw_children: OrderedRawChildren::default(),
        }
    }
}

/// Shape properties on a major or minor gridline container.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChartLines {
    pub sp_pr: Option<CT_ShapeProperties>,
    raw_attributes: XmlAttributes,
    raw_children: OrderedRawChildren,
}

/// Number format applied to axis values and labels.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NumberFormat {
    pub format_code: String,
    pub source_linked: bool,
    raw_attributes: XmlAttributes,
    raw_content: Vec<Vec<u8>>,
}

/// Placement of a chart data label relative to its data point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataLabelPosition {
    BestFit,
    Bottom,
    Center,
    InsideBase,
    InsideEnd,
    Left,
    OutsideEnd,
    Right,
    Top,
}

impl DataLabelPosition {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "bestFit" => Some(Self::BestFit),
            "b" => Some(Self::Bottom),
            "ctr" => Some(Self::Center),
            "inBase" => Some(Self::InsideBase),
            "inEnd" => Some(Self::InsideEnd),
            "l" => Some(Self::Left),
            "outEnd" => Some(Self::OutsideEnd),
            "r" => Some(Self::Right),
            "t" => Some(Self::Top),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::BestFit => "bestFit",
            Self::Bottom => "b",
            Self::Center => "ctr",
            Self::InsideBase => "inBase",
            Self::InsideEnd => "inEnd",
            Self::Left => "l",
            Self::OutsideEnd => "outEnd",
            Self::Right => "r",
            Self::Top => "t",
        }
    }
}

/// Collection-level data-label defaults for one chart series or plot.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CT_DLbls {
    pub number_format: Option<NumberFormat>,
    pub position: Option<DataLabelPosition>,
    pub separator: Option<String>,
    pub show_legend_key: bool,
    pub show_value: bool,
    pub show_category_name: bool,
    pub show_series_name: bool,
    pub show_percent: bool,
    pub show_bubble_size: bool,
    position_markup: Option<ScalarMarkup>,
    show_legend_key_markup: Option<ScalarMarkup>,
    show_value_markup: Option<ScalarMarkup>,
    show_category_name_markup: Option<ScalarMarkup>,
    show_series_name_markup: Option<ScalarMarkup>,
    show_percent_markup: Option<ScalarMarkup>,
    show_bubble_size_markup: Option<ScalarMarkup>,
    separator_markup: Option<TextMarkup>,
    raw_attributes: XmlAttributes,
    namespace_declarations: XmlAttributes,
    raw_children: OrderedRawChildren,
}

impl CT_DLbls {
    /// Labels that show the percentage and nothing else, with all six flags
    /// written, as PowerPoint writes them: readers disagree on an absent
    /// flag, and LibreOffice reads one as shown.
    fn percentage_only() -> Self {
        let stated = || Some(ScalarMarkup::default());
        Self {
            show_percent: true,
            show_legend_key_markup: stated(),
            show_value_markup: stated(),
            show_category_name_markup: stated(),
            show_series_name_markup: stated(),
            show_percent_markup: stated(),
            show_bubble_size_markup: stated(),
            ..Self::default()
        }
    }
}

/// How a chart displays cells whose cached values are blank.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DispBlanksAs {
    #[default]
    Gap,
    Zero,
    Span,
}

impl DispBlanksAs {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "gap" => Some(Self::Gap),
            "zero" => Some(Self::Zero),
            "span" => Some(Self::Span),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Gap => "gap",
            Self::Zero => "zero",
            Self::Span => "span",
        }
    }
}

/// A chart title with plain-text authoring and preserved unsupported children.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CT_Title {
    text: Option<CT_TextBody>,
    raw_attributes: Vec<(String, String)>,
    raw_children: OrderedRawChildren,
}

/// Preserved lexical and ordered-raw state for one typed plot.
#[derive(Clone, Debug, Default, PartialEq)]
struct PlotMarkup {
    raw_attributes: XmlAttributes,
    raw_children: OrderedRawChildren,
    direction: Option<ScalarMarkup>,
    grouping: Option<ScalarMarkup>,
    gap_width: Option<ScalarMarkup>,
    overlap: Option<ScalarMarkup>,
    marker: Option<ScalarMarkup>,
    smooth: Option<ScalarMarkup>,
    first_slice_angle: Option<ScalarMarkup>,
    hole_size: Option<ScalarMarkup>,
    style: Option<ScalarMarkup>,
    axis_ids: Vec<AxisIdMarkup>,
    original_series_keys: Vec<(u32, u32)>,
    original_axis_ids: Vec<AxisId>,
    parsed_bar: Option<bool>,
    parsed_remaining: Option<&'static str>,
}

/// A plot-area shell. Selected raw slots are replaced with types.
#[derive(Clone, Debug, PartialEq)]
pub struct CT_PlotArea {
    raw_attributes: Vec<(String, String)>,
    raw_children: OrderedRawChildren,
    namespace_bindings: NamespaceBindings,
    plots: Option<Vec<Plot>>,
    plot_markup: Vec<PlotMarkup>,
    axes: Vec<Axis>,
}

/// The five ChartML legend positions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LegendPosition {
    #[default]
    Right,
    Left,
    Top,
    Bottom,
    TopRight,
}

impl LegendPosition {
    /// The ChartML lexical value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Right => "r",
            Self::Left => "l",
            Self::Top => "t",
            Self::Bottom => "b",
            Self::TopRight => "tr",
        }
    }
    /// Parse one of the five supported lexical values.
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "r" => Ok(Self::Right),
            "l" => Ok(Self::Left),
            "t" => Ok(Self::Top),
            "b" => Ok(Self::Bottom),
            "tr" => Ok(Self::TopRight),
            _ => Err(invalid_authoring_value(format!(
                "unknown legend position {value}"
            ))),
        }
    }
}

/// Effective legend settings. An absent legend has right/false defaults.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LegendSettings {
    pub visible: bool,
    pub position: LegendPosition,
    pub overlay: bool,
}

/// A legend retaining opaque children and their effective projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CT_Legend {
    root_name: String,
    settings: LegendSettings,
    raw_attributes: Vec<(String, String)>,
    raw_children: OrderedRawChildren,
}

#[derive(Clone, Debug, PartialEq)]
struct AxisIdMarkup {
    scalar: ScalarMarkup,
    parsed: AxisId,
    lexical: String,
}

/// The common modelled sequence of one ChartML axis.
#[derive(Clone, Debug)]
pub struct Axis {
    pub kind: AxisKind,
    pub id: AxisId,
    pub scaling: Scaling,
    pub deleted: bool,
    pub position: AxisPosition,
    pub major_gridlines: Option<ChartLines>,
    pub minor_gridlines: Option<ChartLines>,
    pub title: Option<CT_Title>,
    pub number_format: Option<NumberFormat>,
    pub major_tick_mark: TickMark,
    pub minor_tick_mark: TickMark,
    pub tick_label_position: TickLabelPosition,
    pub sp_pr: Option<CT_ShapeProperties>,
    pub tx_pr: Option<CT_TextBody>,
    pub cross_axis: AxisId,
    parsed_kind: Option<AxisKind>,
    id_markup: AxisIdMarkup,
    delete_markup: Option<ScalarMarkup>,
    position_markup: ScalarMarkup,
    major_tick_mark_markup: Option<ScalarMarkup>,
    minor_tick_mark_markup: Option<ScalarMarkup>,
    tick_label_position_markup: Option<ScalarMarkup>,
    cross_axis_markup: AxisIdMarkup,
    raw_attributes: XmlAttributes,
    namespace_declarations: XmlAttributes,
    raw_children: OrderedRawChildren,
}

impl PartialEq for Axis {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.id == other.id
            && axis_id_markup_eq(&self.id_markup, &other.id_markup)
            && self.scaling == other.scaling
            && self.deleted == other.deleted
            && optional_scalar_markup_eq(&self.delete_markup, &other.delete_markup)
            && self.position == other.position
            && self.position_markup == other.position_markup
            && self.major_gridlines == other.major_gridlines
            && self.minor_gridlines == other.minor_gridlines
            && self.title == other.title
            && self.number_format == other.number_format
            && self.major_tick_mark == other.major_tick_mark
            && optional_scalar_markup_eq(
                &self.major_tick_mark_markup,
                &other.major_tick_mark_markup,
            )
            && self.minor_tick_mark == other.minor_tick_mark
            && optional_scalar_markup_eq(
                &self.minor_tick_mark_markup,
                &other.minor_tick_mark_markup,
            )
            && self.tick_label_position == other.tick_label_position
            && optional_scalar_markup_eq(
                &self.tick_label_position_markup,
                &other.tick_label_position_markup,
            )
            && self.sp_pr == other.sp_pr
            && self.tx_pr == other.tx_pr
            && self.cross_axis == other.cross_axis
            && axis_id_markup_eq(&self.cross_axis_markup, &other.cross_axis_markup)
            && self.raw_attributes == other.raw_attributes
            && self.namespace_declarations == other.namespace_declarations
            && self.raw_children == other.raw_children
    }
}

impl Axis {
    pub fn new(kind: AxisKind, id: AxisId, position: AxisPosition, cross_axis: AxisId) -> Self {
        Self {
            kind,
            id,
            scaling: Scaling::default(),
            deleted: false,
            position,
            major_gridlines: None,
            minor_gridlines: None,
            title: None,
            number_format: None,
            major_tick_mark: TickMark::None,
            minor_tick_mark: TickMark::None,
            tick_label_position: TickLabelPosition::NextTo,
            sp_pr: None,
            tx_pr: None,
            cross_axis,
            parsed_kind: None,
            id_markup: default_axis_id_markup(id),
            delete_markup: None,
            position_markup: ScalarMarkup::default(),
            major_tick_mark_markup: None,
            minor_tick_mark_markup: None,
            tick_label_position_markup: None,
            cross_axis_markup: default_axis_id_markup(cross_axis),
            raw_attributes: Vec::new(),
            namespace_declarations: Vec::new(),
            raw_children: OrderedRawChildren::default(),
        }
    }

    /// Sets axis visibility and writes the explicit ChartML deletion flag.
    pub fn set_deleted(&mut self, deleted: bool) {
        self.deleted = deleted;
        self.delete_markup.get_or_insert_with(ScalarMarkup::default);
    }

    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        Self::from_xml_with_namespaces(xml, &chart_namespace_defaults())
    }

    fn from_xml_with_namespaces(xml: &[u8], inherited: &NamespaceBindings) -> Result<Self> {
        let mut reader = Reader::from_reader(xml);
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element) => {
                    let name = element.name();
                    let local = local_name(name.as_ref());
                    let Some(kind) = AxisKind::parse(local) else {
                        return Err(ChartError::UnexpectedElement(element_name(&element)));
                    };
                    chart_root_prefix(&element)?;
                    if !element_is_in_namespace(&element, C_NS, inherited)? {
                        return Err(ChartError::UnexpectedElement(element_name(&element)));
                    }
                    return Self::from_element(&mut reader, &element, inherited, kind);
                }
                Event::Empty(element) => {
                    let name = element.name();
                    let local = local_name(name.as_ref());
                    if AxisKind::parse(local).is_some() {
                        return Err(ChartError::MissingElement("c:axId".to_owned()));
                    }
                    return Err(ChartError::UnexpectedElement(element_name(&element)));
                }
                Event::Eof => return Err(ChartError::MissingElement("ChartML axis".to_owned())),
                _ => {}
            }
            buffer.clear();
        }
    }

    fn from_element(
        reader: &mut Reader<&[u8]>,
        start: &BytesStart<'_>,
        inherited: &NamespaceBindings,
        kind: AxisKind,
    ) -> Result<Self> {
        reject_conflicting_prefix(start, b"a", A_NS)?;
        reject_conflicting_prefix(start, b"r", R_NS)?;
        let namespaces = chart_bindings(inherited, start)?;
        require_fixed_namespace(&namespaces, b"c", C_NS, start)?;
        require_fixed_namespace(&namespaces, b"a", A_NS, start)?;
        require_fixed_namespace(&namespaces, b"r", R_NS, start)?;
        let (raw_attributes, _) =
            capture_fixed_root_attributes(start, &["xmlns:c", "xmlns:a", "xmlns:r"])?;
        let namespace_declarations = standalone_namespace_declarations(&namespaces)?;
        let mut state = AxisParseState::default();
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element) => {
                    let name = chart_child_local(&element, &namespaces)?;
                    let raw = capture_element(reader, &element)?;
                    state.parse_child(name.as_deref().unwrap_or_default(), raw, &namespaces)?;
                }
                Event::Empty(element) => {
                    let name = chart_child_local(&element, &namespaces)?;
                    let raw = capture_empty_element(&element)?;
                    state.parse_child(name.as_deref().unwrap_or_default(), raw, &namespaces)?;
                }
                event @ (Event::Text(_)
                | Event::CData(_)
                | Event::Comment(_)
                | Event::PI(_)
                | Event::GeneralRef(_)) => state.capture_event(capture_event(event)?),
                Event::End(element)
                    if matches_local_name(element.name().as_ref(), kind.local().as_bytes()) =>
                {
                    break;
                }
                Event::Eof => return Err(missing_end(&format!("c:{}", kind.local()))),
                _ => {}
            }
            buffer.clear();
        }
        state.finish(kind, raw_attributes, namespace_declarations)
    }

    pub fn to_xml(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut writer = Writer::new(Vec::new());
        let root = format!("c:{}", self.kind.local());
        let mut start = BytesStart::new(&root);
        start.push_attribute(("xmlns:c", C_NS));
        start.push_attribute(("xmlns:a", A_NS));
        start.push_attribute(("xmlns:r", R_NS));
        push_attributes(&mut start, &self.namespace_declarations);
        push_attributes(&mut start, &self.raw_attributes);
        writer
            .write_event(Event::Start(start))
            .map_err(OxmlError::from)?;
        emit_raw(&mut writer, self.raw_children.at(0))?;
        write_axis_id(&mut writer, "c:axId", self.id, &self.id_markup)?;
        emit_raw(&mut writer, self.raw_children.at(1))?;
        self.scaling.write_xml(&mut writer)?;
        emit_raw(&mut writer, self.raw_children.at(2))?;
        if self.deleted || self.delete_markup.is_some() {
            write_scalar(
                &mut writer,
                "c:delete",
                bool_lexical(self.deleted),
                self.delete_markup.as_ref(),
            )?;
        }
        emit_raw(&mut writer, self.raw_children.at(3))?;
        write_scalar(
            &mut writer,
            "c:axPos",
            self.position.as_str(),
            Some(&self.position_markup),
        )?;
        emit_raw(&mut writer, self.raw_children.at(4))?;
        if let Some(lines) = &self.major_gridlines {
            lines.write_xml(&mut writer, "c:majorGridlines")?;
        }
        emit_raw(&mut writer, self.raw_children.at(5))?;
        if let Some(lines) = &self.minor_gridlines {
            lines.write_xml(&mut writer, "c:minorGridlines")?;
        }
        emit_raw(&mut writer, self.raw_children.at(6))?;
        if let Some(title) = &self.title {
            title.write_xml(&mut writer)?;
        }
        emit_raw(&mut writer, self.raw_children.at(7))?;
        if let Some(number_format) = &self.number_format {
            number_format.write_xml(&mut writer)?;
        }
        emit_raw(&mut writer, self.raw_children.at(8))?;
        if self.major_tick_mark != TickMark::None || self.major_tick_mark_markup.is_some() {
            write_scalar(
                &mut writer,
                "c:majorTickMark",
                self.major_tick_mark.as_str(),
                self.major_tick_mark_markup.as_ref(),
            )?;
        }
        emit_raw(&mut writer, self.raw_children.at(9))?;
        if self.minor_tick_mark != TickMark::None || self.minor_tick_mark_markup.is_some() {
            write_scalar(
                &mut writer,
                "c:minorTickMark",
                self.minor_tick_mark.as_str(),
                self.minor_tick_mark_markup.as_ref(),
            )?;
        }
        emit_raw(&mut writer, self.raw_children.at(10))?;
        if self.tick_label_position != TickLabelPosition::NextTo
            || self.tick_label_position_markup.is_some()
        {
            write_scalar(
                &mut writer,
                "c:tickLblPos",
                self.tick_label_position.as_str(),
                self.tick_label_position_markup.as_ref(),
            )?;
        }
        emit_raw(&mut writer, self.raw_children.at(11))?;
        if let Some(properties) = &self.sp_pr {
            properties.write_xml_as(&mut writer, "c:spPr")?;
        }
        emit_raw(&mut writer, self.raw_children.at(12))?;
        if let Some(text) = &self.tx_pr {
            text.write_xml_as(&mut writer, "c:txPr")?;
        }
        emit_raw(&mut writer, self.raw_children.at(13))?;
        write_axis_id(
            &mut writer,
            "c:crossAx",
            self.cross_axis,
            &self.cross_axis_markup,
        )?;
        emit_raw(&mut writer, self.raw_children.at(14))?;
        writer
            .write_event(Event::End(BytesEnd::new(&root)))
            .map_err(OxmlError::from)?;
        Ok(writer.into_inner())
    }

    pub fn raw_children(&self) -> &OrderedRawChildren {
        &self.raw_children
    }

    fn validate(&self) -> Result<()> {
        if let Some(parsed_kind) = self.parsed_kind
            && self.kind != parsed_kind
        {
            return Err(ChartError::InvalidValue {
                element: "ChartML axis root".to_owned(),
                value: format!(
                    "parsed c:{} cannot be relabelled c:{}",
                    parsed_kind.local(),
                    self.kind.local()
                ),
            });
        }
        AxisId::new(self.id.value())?;
        AxisId::new(self.cross_axis.value())?;
        self.scaling.validate()?;
        if let Some(number_format) = &self.number_format {
            number_format.validate()?;
        }
        Ok(())
    }
}

#[derive(Default)]
struct AxisParseState {
    id: Option<(AxisId, AxisIdMarkup)>,
    scaling: Option<Scaling>,
    deleted: Option<(bool, ScalarMarkup)>,
    position: Option<(AxisPosition, ScalarMarkup)>,
    major_gridlines: Option<ChartLines>,
    minor_gridlines: Option<ChartLines>,
    title: Option<CT_Title>,
    number_format: Option<NumberFormat>,
    major_tick_mark: Option<(TickMark, ScalarMarkup)>,
    minor_tick_mark: Option<(TickMark, ScalarMarkup)>,
    tick_label_position: Option<(TickLabelPosition, ScalarMarkup)>,
    sp_pr: Option<CT_ShapeProperties>,
    tx_pr: Option<CT_TextBody>,
    cross_axis: Option<(AxisId, AxisIdMarkup)>,
    raw_children: OrderedRawChildren,
    boundary: usize,
}

impl AxisParseState {
    fn capture_event(&mut self, raw: Vec<u8>) {
        self.raw_children.push(self.boundary, raw);
    }

    fn parse_child(
        &mut self,
        name: &[u8],
        raw: Vec<u8>,
        namespaces: &NamespaceBindings,
    ) -> Result<()> {
        match name {
            b"axId" => {
                set_once(&mut self.id, parse_axis_id_scalar(&raw, "axId")?, "c:axId")?;
                self.boundary = self.boundary.max(1);
            }
            b"scaling" => {
                set_once(
                    &mut self.scaling,
                    Scaling::from_xml(&raw, namespaces)?,
                    "c:scaling",
                )?;
                self.boundary = self.boundary.max(2);
            }
            b"delete" => {
                set_once(
                    &mut self.deleted,
                    parse_bool_value(&raw, "delete")?,
                    "c:delete",
                )?;
                self.boundary = self.boundary.max(3);
            }
            b"axPos" => {
                set_once(&mut self.position, parse_axis_position(&raw)?, "c:axPos")?;
                self.boundary = self.boundary.max(4);
            }
            b"majorGridlines" => {
                set_once(
                    &mut self.major_gridlines,
                    ChartLines::from_xml(&raw, b"majorGridlines", namespaces)?,
                    "c:majorGridlines",
                )?;
                self.boundary = self.boundary.max(5);
            }
            b"minorGridlines" => {
                set_once(
                    &mut self.minor_gridlines,
                    ChartLines::from_xml(&raw, b"minorGridlines", namespaces)?,
                    "c:minorGridlines",
                )?;
                self.boundary = self.boundary.max(6);
            }
            b"title" => {
                set_once(&mut self.title, CT_Title::from_xml(&raw)?, "c:title")?;
                self.boundary = self.boundary.max(7);
            }
            b"numFmt" => {
                set_once(
                    &mut self.number_format,
                    NumberFormat::from_xml(&raw)?,
                    "c:numFmt",
                )?;
                self.boundary = self.boundary.max(8);
            }
            b"majorTickMark" => {
                set_once(
                    &mut self.major_tick_mark,
                    parse_tick_mark(&raw, "majorTickMark")?,
                    "c:majorTickMark",
                )?;
                self.boundary = self.boundary.max(9);
            }
            b"minorTickMark" => {
                set_once(
                    &mut self.minor_tick_mark,
                    parse_tick_mark(&raw, "minorTickMark")?,
                    "c:minorTickMark",
                )?;
                self.boundary = self.boundary.max(10);
            }
            b"tickLblPos" => {
                set_once(
                    &mut self.tick_label_position,
                    parse_tick_label_position(&raw)?,
                    "c:tickLblPos",
                )?;
                self.boundary = self.boundary.max(11);
            }
            b"spPr" => {
                parse_chart_shape_properties(&raw, namespaces, &mut self.sp_pr)?;
                self.boundary = self.boundary.max(12);
            }
            b"txPr" => {
                reject_conflicting_prefix_in_xml(&raw, b"a", A_NS)?;
                let inherited = namespaces
                    .iter()
                    .map(|(prefix, uri)| {
                        Ok((
                            std::str::from_utf8(prefix)
                                .map_err(OxmlError::from)?
                                .to_owned(),
                            uri.clone(),
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let text = CT_TextBody::from_xml_as_with_namespaces(&raw, b"txPr", &inherited)?;
                let mut writer = Writer::new(Vec::new());
                text.write_xml_as(&mut writer, "c:txPr")?;
                reject_rewritten_foreign_elements(&raw, &writer.into_inner(), namespaces, b"txPr")?;
                set_once(&mut self.tx_pr, text, "c:txPr")?;
                self.boundary = self.boundary.max(13);
            }
            b"crossAx" => {
                set_once(
                    &mut self.cross_axis,
                    parse_axis_id_scalar(&raw, "crossAx")?,
                    "c:crossAx",
                )?;
                self.boundary = self.boundary.max(14);
            }
            _ => self.raw_children.push(self.boundary, raw),
        }
        Ok(())
    }

    fn finish(
        self,
        kind: AxisKind,
        raw_attributes: XmlAttributes,
        namespace_declarations: XmlAttributes,
    ) -> Result<Axis> {
        let (id, id_markup) = self
            .id
            .ok_or_else(|| ChartError::MissingElement("c:axId".to_owned()))?;
        let scaling = self
            .scaling
            .ok_or_else(|| ChartError::MissingElement("c:scaling".to_owned()))?;
        let (position, position_markup) = self
            .position
            .ok_or_else(|| ChartError::MissingElement("c:axPos".to_owned()))?;
        let (cross_axis, cross_axis_markup) = self
            .cross_axis
            .ok_or_else(|| ChartError::MissingElement("c:crossAx".to_owned()))?;
        let (deleted, delete_markup) = self
            .deleted
            .map(|(value, markup)| (value, Some(markup)))
            .unwrap_or((false, None));
        let (major_tick_mark, major_tick_mark_markup) = self
            .major_tick_mark
            .map(|(value, markup)| (value, Some(markup)))
            .unwrap_or((TickMark::None, None));
        let (minor_tick_mark, minor_tick_mark_markup) = self
            .minor_tick_mark
            .map(|(value, markup)| (value, Some(markup)))
            .unwrap_or((TickMark::None, None));
        let (tick_label_position, tick_label_position_markup) = self
            .tick_label_position
            .map(|(value, markup)| (value, Some(markup)))
            .unwrap_or((TickLabelPosition::NextTo, None));
        let axis = Axis {
            kind,
            id,
            scaling,
            deleted,
            position,
            major_gridlines: self.major_gridlines,
            minor_gridlines: self.minor_gridlines,
            title: self.title,
            number_format: self.number_format,
            major_tick_mark,
            minor_tick_mark,
            tick_label_position,
            sp_pr: self.sp_pr,
            tx_pr: self.tx_pr,
            cross_axis,
            parsed_kind: Some(kind),
            id_markup,
            delete_markup,
            position_markup,
            major_tick_mark_markup,
            minor_tick_mark_markup,
            tick_label_position_markup,
            cross_axis_markup,
            raw_attributes,
            namespace_declarations,
            raw_children: raw_children_in_schema_order(&self.raw_children, 14),
        };
        axis.validate()?;
        Ok(axis)
    }
}

impl Scaling {
    fn from_xml(xml: &[u8], inherited: &NamespaceBindings) -> Result<Self> {
        let mut reader = Reader::from_reader(xml);
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element)
                    if matches_local_name(element.name().as_ref(), b"scaling") =>
                {
                    let namespaces = typed_rewrite_bindings(&element, inherited)?;
                    let raw_attributes = capture_attributes(&element)?;
                    let mut log_base = None;
                    let mut orientation = None;
                    let mut maximum = None;
                    let mut minimum = None;
                    let mut raw_children = OrderedRawChildren::default();
                    let mut boundary = 0usize;
                    let mut inner = Vec::new();
                    loop {
                        match reader
                            .read_event_into(&mut inner)
                            .map_err(OxmlError::from)?
                        {
                            Event::Start(child) => {
                                let name = chart_child_local(&child, &namespaces)?;
                                let raw = capture_element(&mut reader, &child)?;
                                parse_scaling_child(
                                    name.as_deref().unwrap_or_default(),
                                    raw,
                                    &mut log_base,
                                    &mut orientation,
                                    &mut maximum,
                                    &mut minimum,
                                    &mut raw_children,
                                    &mut boundary,
                                )?;
                            }
                            Event::Empty(child) => {
                                let name = chart_child_local(&child, &namespaces)?;
                                let raw = capture_empty_element(&child)?;
                                parse_scaling_child(
                                    name.as_deref().unwrap_or_default(),
                                    raw,
                                    &mut log_base,
                                    &mut orientation,
                                    &mut maximum,
                                    &mut minimum,
                                    &mut raw_children,
                                    &mut boundary,
                                )?;
                            }
                            event @ (Event::Text(_)
                            | Event::CData(_)
                            | Event::Comment(_)
                            | Event::PI(_)
                            | Event::GeneralRef(_)) => {
                                raw_children.push(boundary, capture_event(event)?);
                            }
                            Event::End(end)
                                if matches_local_name(end.name().as_ref(), b"scaling") =>
                            {
                                let (log_base, log_base_markup) = optional_markup(log_base);
                                let (orientation, orientation_markup) = orientation
                                    .map(|(value, markup)| (value, Some(markup)))
                                    .unwrap_or((Orientation::MinMax, None));
                                let (maximum, maximum_markup) = optional_markup(maximum);
                                let (minimum, minimum_markup) = optional_markup(minimum);
                                let scaling = Self {
                                    log_base,
                                    orientation,
                                    maximum,
                                    minimum,
                                    log_base_markup,
                                    orientation_markup,
                                    maximum_markup,
                                    minimum_markup,
                                    raw_attributes,
                                    raw_children: raw_children_in_schema_order(&raw_children, 4),
                                };
                                scaling.validate()?;
                                return Ok(scaling);
                            }
                            Event::Eof => return Err(missing_end("c:scaling")),
                            _ => {}
                        }
                        inner.clear();
                    }
                }
                Event::Empty(element)
                    if matches_local_name(element.name().as_ref(), b"scaling") =>
                {
                    typed_rewrite_bindings(&element, inherited)?;
                    return Ok(Self {
                        raw_attributes: capture_attributes(&element)?,
                        ..Self::default()
                    });
                }
                Event::Start(element) | Event::Empty(element) => {
                    return Err(ChartError::UnexpectedElement(element_name(&element)));
                }
                Event::Eof => return Err(ChartError::MissingElement("c:scaling".to_owned())),
                _ => {}
            }
            buffer.clear();
        }
    }

    fn write_xml<W: Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.validate()?;
        let mut start = BytesStart::new("c:scaling");
        push_attributes(&mut start, &self.raw_attributes);
        writer
            .write_event(Event::Start(start))
            .map_err(OxmlError::from)?;
        emit_raw(writer, self.raw_children.at(0))?;
        if let Some(value) = self.log_base {
            write_scalar(
                writer,
                "c:logBase",
                &value.to_string(),
                self.log_base_markup.as_ref(),
            )?;
        }
        emit_raw(writer, self.raw_children.at(1))?;
        if self.orientation != Orientation::MinMax || self.orientation_markup.is_some() {
            write_scalar(
                writer,
                "c:orientation",
                self.orientation.as_str(),
                self.orientation_markup.as_ref(),
            )?;
        }
        emit_raw(writer, self.raw_children.at(2))?;
        if let Some(value) = self.maximum {
            write_scalar(
                writer,
                "c:max",
                &value.to_string(),
                self.maximum_markup.as_ref(),
            )?;
        }
        emit_raw(writer, self.raw_children.at(3))?;
        if let Some(value) = self.minimum {
            write_scalar(
                writer,
                "c:min",
                &value.to_string(),
                self.minimum_markup.as_ref(),
            )?;
        }
        emit_raw(writer, self.raw_children.at(4))?;
        writer
            .write_event(Event::End(BytesEnd::new("c:scaling")))
            .map_err(OxmlError::from)?;
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        if let Some(value) = self.log_base
            && (!value.is_finite() || !(2.0..=1000.0).contains(&value))
        {
            return Err(ChartError::InvalidValue {
                element: "c:logBase".to_owned(),
                value: value.to_string(),
            });
        }
        for (element, value) in [("c:max", self.maximum), ("c:min", self.minimum)] {
            if let Some(value) = value
                && !value.is_finite()
            {
                return Err(ChartError::InvalidValue {
                    element: element.to_owned(),
                    value: value.to_string(),
                });
            }
        }
        if let (Some(minimum), Some(maximum)) = (self.minimum, self.maximum)
            && minimum > maximum
        {
            return Err(ChartError::InvalidValue {
                element: "c:scaling".to_owned(),
                value: format!("minimum {minimum} exceeds maximum {maximum}"),
            });
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn parse_scaling_child(
    name: &[u8],
    raw: Vec<u8>,
    log_base: &mut Option<(f64, ScalarMarkup)>,
    orientation: &mut Option<(Orientation, ScalarMarkup)>,
    maximum: &mut Option<(f64, ScalarMarkup)>,
    minimum: &mut Option<(f64, ScalarMarkup)>,
    raw_children: &mut OrderedRawChildren,
    boundary: &mut usize,
) -> Result<()> {
    match name {
        b"logBase" => {
            set_once(log_base, parse_f64_scalar(&raw, "logBase")?, "c:logBase")?;
            *boundary = (*boundary).max(1);
        }
        b"orientation" => {
            let (value, markup) = scalar_value(&raw, "orientation")?;
            let value = value
                .ok_or_else(|| invalid_attribute("orientation", "val", "<missing>".to_owned()))?;
            let parsed = Orientation::parse(&value)
                .ok_or_else(|| invalid_attribute("orientation", "val", value))?;
            set_once(orientation, (parsed, markup), "c:orientation")?;
            *boundary = (*boundary).max(2);
        }
        b"max" => {
            set_once(maximum, parse_f64_scalar(&raw, "max")?, "c:max")?;
            *boundary = (*boundary).max(3);
        }
        b"min" => {
            set_once(minimum, parse_f64_scalar(&raw, "min")?, "c:min")?;
            *boundary = (*boundary).max(4);
        }
        _ => raw_children.push(*boundary, raw),
    }
    Ok(())
}

impl ChartLines {
    fn from_xml(xml: &[u8], local: &[u8], inherited: &NamespaceBindings) -> Result<Self> {
        let mut reader = Reader::from_reader(xml);
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element) if matches_local_name(element.name().as_ref(), local) => {
                    let namespaces = typed_rewrite_bindings(&element, inherited)?;
                    let raw_attributes = capture_attributes(&element)?;
                    let mut sp_pr = None;
                    let mut raw_children = OrderedRawChildren::default();
                    let mut boundary = 0usize;
                    let mut inner = Vec::new();
                    loop {
                        match reader
                            .read_event_into(&mut inner)
                            .map_err(OxmlError::from)?
                        {
                            Event::Start(child) => {
                                let name = chart_child_local(&child, &namespaces)?;
                                let raw = capture_element(&mut reader, &child)?;
                                if name.as_deref() == Some(b"spPr") {
                                    parse_chart_shape_properties(&raw, &namespaces, &mut sp_pr)?;
                                    boundary = 1;
                                } else {
                                    raw_children.push(boundary, raw);
                                }
                            }
                            Event::Empty(child) => {
                                let name = chart_child_local(&child, &namespaces)?;
                                let raw = capture_empty_element(&child)?;
                                if name.as_deref() == Some(b"spPr") {
                                    parse_chart_shape_properties(&raw, &namespaces, &mut sp_pr)?;
                                    boundary = 1;
                                } else {
                                    raw_children.push(boundary, raw);
                                }
                            }
                            event @ (Event::Text(_)
                            | Event::CData(_)
                            | Event::Comment(_)
                            | Event::PI(_)
                            | Event::GeneralRef(_)) => {
                                raw_children.push(boundary, capture_event(event)?);
                            }
                            Event::End(end) if matches_local_name(end.name().as_ref(), local) => {
                                return Ok(Self {
                                    sp_pr,
                                    raw_attributes,
                                    raw_children: raw_children_in_schema_order(&raw_children, 1),
                                });
                            }
                            Event::Eof => {
                                return Err(missing_end(&format!(
                                    "c:{}",
                                    String::from_utf8_lossy(local)
                                )));
                            }
                            _ => {}
                        }
                        inner.clear();
                    }
                }
                Event::Empty(element) if matches_local_name(element.name().as_ref(), local) => {
                    typed_rewrite_bindings(&element, inherited)?;
                    return Ok(Self {
                        sp_pr: None,
                        raw_attributes: capture_attributes(&element)?,
                        raw_children: OrderedRawChildren::default(),
                    });
                }
                Event::Start(element) | Event::Empty(element) => {
                    return Err(ChartError::UnexpectedElement(element_name(&element)));
                }
                Event::Eof => {
                    return Err(ChartError::MissingElement(format!(
                        "c:{}",
                        String::from_utf8_lossy(local)
                    )));
                }
                _ => {}
            }
            buffer.clear();
        }
    }

    fn write_xml<W: Write>(&self, writer: &mut Writer<W>, tag: &str) -> Result<()> {
        let mut start = BytesStart::new(tag);
        push_attributes(&mut start, &self.raw_attributes);
        if self.sp_pr.is_none() && self.raw_children.is_empty() {
            writer
                .write_event(Event::Empty(start))
                .map_err(OxmlError::from)?;
            return Ok(());
        }
        writer
            .write_event(Event::Start(start))
            .map_err(OxmlError::from)?;
        emit_raw(writer, self.raw_children.at(0))?;
        if let Some(properties) = &self.sp_pr {
            properties.write_xml_as(writer, "c:spPr")?;
        }
        emit_raw(writer, self.raw_children.at(1))?;
        writer
            .write_event(Event::End(BytesEnd::new(tag)))
            .map_err(OxmlError::from)?;
        Ok(())
    }
}

fn parse_chart_shape_properties(
    raw: &[u8],
    namespaces: &NamespaceBindings,
    slot: &mut Option<CT_ShapeProperties>,
) -> Result<()> {
    reject_conflicting_prefix_in_xml(raw, b"a", A_NS)?;
    let inherited = namespaces
        .iter()
        .map(|(prefix, uri)| {
            Ok((
                std::str::from_utf8(prefix)
                    .map_err(OxmlError::from)?
                    .to_owned(),
                uri.clone(),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let properties = CT_ShapeProperties::from_xml_with_inherited_namespaces(raw, &inherited)?;
    let mut writer = Writer::new(Vec::new());
    properties.write_xml_as(&mut writer, "c:spPr")?;
    reject_rewritten_foreign_elements(raw, &writer.into_inner(), namespaces, b"spPr")?;
    set_once(slot, properties, "c:spPr")
}

impl NumberFormat {
    pub fn new(format_code: String, source_linked: bool) -> Result<Self> {
        let number_format = Self {
            format_code,
            source_linked,
            raw_attributes: Vec::new(),
            raw_content: Vec::new(),
        };
        number_format.validate()?;
        Ok(number_format)
    }

    fn from_xml(xml: &[u8]) -> Result<Self> {
        let mut reader = Reader::from_reader(xml);
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Empty(element) if matches_local_name(element.name().as_ref(), b"numFmt") => {
                    return Self::from_element(&element, Vec::new());
                }
                Event::Start(element) if matches_local_name(element.name().as_ref(), b"numFmt") => {
                    let mut raw_content = Vec::new();
                    let mut inner = Vec::new();
                    loop {
                        match reader
                            .read_event_into(&mut inner)
                            .map_err(OxmlError::from)?
                        {
                            Event::End(end)
                                if matches_local_name(end.name().as_ref(), b"numFmt") =>
                            {
                                return Self::from_element(&element, raw_content);
                            }
                            event @ (Event::Text(_)
                            | Event::CData(_)
                            | Event::Comment(_)
                            | Event::PI(_)
                            | Event::GeneralRef(_)) => raw_content.push(capture_event(event)?),
                            Event::Eof => return Err(missing_end("c:numFmt")),
                            _ => return Err(ChartError::UnexpectedElement("c:numFmt".to_owned())),
                        }
                        inner.clear();
                    }
                }
                Event::Start(element) | Event::Empty(element) => {
                    return Err(ChartError::UnexpectedElement(element_name(&element)));
                }
                Event::Eof => return Err(ChartError::MissingElement("c:numFmt".to_owned())),
                _ => {}
            }
            buffer.clear();
        }
    }

    fn from_element(element: &BytesStart<'_>, raw_content: Vec<Vec<u8>>) -> Result<Self> {
        reject_conflicting_prefix(element, b"c", C_NS)?;
        let format_code = attribute_value(element, b"formatCode")?
            .ok_or_else(|| invalid_attribute("numFmt", "formatCode", "<missing>".to_owned()))?;
        let source_linked = attribute_value(element, b"sourceLinked")?
            .ok_or_else(|| invalid_attribute("numFmt", "sourceLinked", "<missing>".to_owned()))?;
        let source_linked = parse_bool_lexical("numFmt", "sourceLinked", &source_linked)?;
        let number_format = Self {
            format_code,
            source_linked,
            raw_attributes: capture_attributes_excluding(
                element,
                &[b"formatCode", b"sourceLinked"],
            )?,
            raw_content,
        };
        number_format.validate()?;
        Ok(number_format)
    }

    fn write_xml<W: Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.validate()?;
        let mut start = BytesStart::new("c:numFmt");
        start.push_attribute(("formatCode", self.format_code.as_str()));
        start.push_attribute(("sourceLinked", bool_lexical(self.source_linked)));
        push_attributes(&mut start, &self.raw_attributes);
        if self.raw_content.is_empty() {
            writer
                .write_event(Event::Empty(start))
                .map_err(OxmlError::from)?;
            return Ok(());
        }
        writer
            .write_event(Event::Start(start))
            .map_err(OxmlError::from)?;
        for raw in &self.raw_content {
            writer.get_mut().write_all(raw).map_err(OxmlError::from)?;
        }
        writer
            .write_event(Event::End(BytesEnd::new("c:numFmt")))
            .map_err(OxmlError::from)?;
        Ok(())
    }

    /// Projects one cached value through the supported deterministic subset.
    pub fn format_value(&self, value: f64) -> Result<String> {
        if !value.is_finite() {
            return Err(ChartError::InvalidValue {
                element: "c:numFmt value".to_owned(),
                value: value.to_string(),
            });
        }
        if self.format_code == "General" {
            return Ok(value.to_string());
        }

        let sections =
            numeric_sections(&self.format_code).ok_or_else(|| ChartError::InvalidValue {
                element: "c:numFmt/@formatCode projection".into(),
                value: self.format_code.clone(),
            })?;
        let index = if value < 0.0 && sections.len() >= 2 {
            1
        } else if value == 0.0 && sections.len() >= 3 {
            2
        } else {
            0
        };
        let mut text = sections[index].format(value.abs())?;
        if value < 0.0 && sections.len() == 1 && !text.starts_with('-') {
            text.insert(0, '-');
        }
        Ok(text)
    }

    fn validate(&self) -> Result<()> {
        validate_xml_text(&self.format_code, "c:numFmt/@formatCode")
    }
}

#[derive(Clone, PartialEq)]
enum NumericToken {
    Digit(bool),
    Point,
    Comma,
    Percent,
    Literal(String),
}
struct NumericSection {
    tokens: Vec<NumericToken>,
    group: usize,
    scale: usize,
}
fn numeric_sections(code: &str) -> Option<Vec<NumericSection>> {
    if code.is_empty() {
        return None;
    }
    let mut parts = vec![String::new()];
    let mut chars = code.chars().peekable();
    let mut quoted = false;
    while let Some(c) = chars.next() {
        if c == '\\' || !quoted && matches!(c, '_' | '*') {
            parts.last_mut()?.push(c);
            parts.last_mut()?.push(chars.next()?);
            continue;
        }
        if c == '"' {
            if quoted && chars.peek() == Some(&'"') {
                parts.last_mut()?.push_str("\"\"");
                chars.next();
                continue;
            }
            quoted = !quoted;
        }
        if c == ';' && !quoted {
            parts.push(String::new());
        } else {
            parts.last_mut()?.push(c);
        }
    }
    if quoted || parts.len() > 4 {
        return None;
    }
    parts.truncate(3);
    if parts.len() > 1 && parts.iter().any(|part| part == "General") {
        return None;
    }
    let sections = parts
        .iter()
        .map(|part| numeric_section(part))
        .collect::<Option<Vec<_>>>()?;
    if !sections.iter().any(|section| {
        section
            .tokens
            .iter()
            .any(|t| matches!(t, NumericToken::Digit(_)))
    }) {
        return None;
    }
    Some(sections)
}
fn numeric_section(code: &str) -> Option<NumericSection> {
    let mut chars = code.chars().peekable();
    let mut tokens = Vec::new();
    let mut point = false;
    while let Some(c) = chars.next() {
        let token = match c {
            '"' => {
                let mut literal = String::new();
                loop {
                    match chars.next()? {
                        '"' if chars.peek() == Some(&'"') => {
                            chars.next();
                            literal.push('"');
                        }
                        '"' => break,
                        c => literal.push(c),
                    }
                }
                NumericToken::Literal(literal)
            }
            '\\' => NumericToken::Literal(chars.next()?.to_string()),
            '_' => {
                chars.next()?;
                NumericToken::Literal(" ".into())
            }
            '*' => {
                chars.next()?;
                continue;
            }
            '[' => {
                let mut bracket = String::new();
                loop {
                    let c = chars.next()?;
                    if c == ']' {
                        break;
                    }
                    bracket.push(c);
                }
                if let Some(currency) = bracket.strip_prefix('$') {
                    let symbol = currency
                        .rsplit_once('-')
                        .filter(|(_, suffix)| {
                            !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_hexdigit())
                        })
                        .map_or(currency, |(symbol, _)| symbol);
                    NumericToken::Literal(symbol.into())
                } else if [
                    "red", "blue", "green", "yellow", "black", "white", "cyan", "magenta",
                ]
                .contains(&bracket.to_ascii_lowercase().as_str())
                {
                    continue;
                } else {
                    return None;
                }
            }
            '0' | '#' | '?' => NumericToken::Digit(c == '0'),
            '.' => {
                if point {
                    return None;
                }
                point = true;
                NumericToken::Point
            }
            ',' => NumericToken::Comma,
            '%' => NumericToken::Percent,
            c if matches!(c.to_ascii_lowercase(), 'y' | 'm' | 'd' | 'h' | 's') => return None,
            'A' | 'a'
                if {
                    let mut following = chars.clone();
                    following.next() == Some('/') && matches!(following.next(), Some('P' | 'p'))
                } =>
            {
                return None;
            }
            'E' | 'e' if matches!(chars.peek(), Some('+' | '-')) => return None,
            '/' => {
                if matches!(tokens.last(), Some(NumericToken::Digit(_)))
                    && matches!(chars.peek(), Some('0' | '#' | '?'))
                {
                    return None;
                }
                NumericToken::Literal("/".into())
            }
            c => NumericToken::Literal(c.to_string()),
        };
        tokens.push(token);
    }
    let decimal = tokens
        .iter()
        .position(|t| *t == NumericToken::Point)
        .unwrap_or(tokens.len());
    let integer_digits = tokens[..decimal]
        .iter()
        .enumerate()
        .filter_map(|(i, t)| matches!(t, NumericToken::Digit(_)).then_some(i))
        .collect::<Vec<_>>();
    let mut group = 0;
    let mut scale = 0;
    for (i, token) in tokens[..decimal].iter_mut().enumerate() {
        if *token != NumericToken::Comma {
            continue;
        }
        let left = integer_digits.iter().any(|at| *at < i);
        let right = integer_digits.iter().filter(|at| **at > i).count();
        if left && right > 0 {
            group = right;
            *token = NumericToken::Literal(String::new());
        } else if left {
            scale += 1;
            *token = NumericToken::Literal(String::new());
        } else {
            *token = NumericToken::Literal(",".into());
        }
    }
    // Fractional commas are ordinary literal commas, not scale/group directives.
    for token in &mut tokens[decimal..] {
        if *token == NumericToken::Comma {
            *token = NumericToken::Literal(",".into());
        }
    }
    Some(NumericSection {
        tokens,
        group,
        scale,
    })
}
impl NumericSection {
    fn format(&self, value: f64) -> Result<String> {
        let mut scaled = value;
        for token in &self.tokens {
            if *token == NumericToken::Percent {
                scaled *= 100.0;
            }
        }
        for _ in 0..self.scale {
            scaled /= 1000.0;
        }
        if !scaled.is_finite() {
            return Err(ChartError::InvalidValue {
                element: "c:numFmt value".into(),
                value: scaled.to_string(),
            });
        }
        let point = self
            .tokens
            .iter()
            .position(|t| *t == NumericToken::Point)
            .unwrap_or(self.tokens.len());
        let precision = self.tokens[point..]
            .iter()
            .filter(|t| matches!(t, NumericToken::Digit(_)))
            .count();
        let rounded = format!("{scaled:.precision$}");
        let (integer, fraction) = rounded.split_once('.').unwrap_or((&rounded, ""));
        let integer_places = self.tokens[..point]
            .iter()
            .filter(|t| matches!(t, NumericToken::Digit(_)))
            .count();
        let required = self.tokens[..point]
            .iter()
            .filter(|t| **t == NumericToken::Digit(true))
            .count();
        let integer = if integer == "0" && required == 0 {
            String::new()
        } else {
            format!("{integer:0>required$}")
        };
        let extra = integer.len().saturating_sub(integer_places);
        let mut integer_at = 0;
        let mut fraction_at = 0;
        let mut places = integer_places;
        let mut out = String::new();
        let mut fractional_end = fraction.len();
        for token in self.tokens[point..].iter().rev() {
            if let NumericToken::Digit(required) = token {
                if !required
                    && fractional_end > 0
                    && fraction.as_bytes()[fractional_end - 1] == b'0'
                {
                    fractional_end -= 1;
                } else {
                    break;
                }
            }
        }
        for (at, token) in self.tokens.iter().enumerate() {
            match token {
                NumericToken::Digit(required) if at < point => {
                    let take = if integer_at == 0 { extra + 1 } else { 1 };
                    if integer.len().saturating_sub(integer_at) >= places {
                        for _ in 0..take {
                            if integer_at < integer.len() {
                                out.push(integer.as_bytes()[integer_at] as char);
                                integer_at += 1;
                                let remaining = integer.len() - integer_at;
                                if self.group > 0 && remaining > 0 && remaining % self.group == 0 {
                                    out.push(',');
                                }
                            }
                        }
                    } else if *required {
                        out.push('0');
                    }
                    places = places.saturating_sub(1);
                }
                NumericToken::Digit(required) => {
                    if fraction_at < fractional_end || *required {
                        out.push(
                            fraction
                                .as_bytes()
                                .get(fraction_at)
                                .copied()
                                .unwrap_or(b'0') as char,
                        );
                    }
                    fraction_at += 1;
                }
                NumericToken::Point => {
                    if fractional_end > 0 {
                        out.push('.');
                    }
                }
                NumericToken::Literal(text) => out.push_str(text),
                NumericToken::Percent => out.push('%'),
                NumericToken::Comma => {}
            }
        }
        Ok(out)
    }
}

impl CT_DLbls {
    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        Self::from_xml_with_namespaces(xml, &chart_namespace_defaults())
    }

    fn from_xml_with_namespaces(xml: &[u8], inherited: &NamespaceBindings) -> Result<Self> {
        let mut reader = Reader::from_reader(xml);
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element) if matches_local_name(element.name().as_ref(), b"dLbls") => {
                    chart_root_prefix(&element)?;
                    if !element_is_in_namespace(&element, C_NS, inherited)? {
                        return Err(ChartError::UnexpectedElement(element_name(&element)));
                    }
                    return Self::from_element(&mut reader, &element, inherited);
                }
                Event::Empty(element) if matches_local_name(element.name().as_ref(), b"dLbls") => {
                    chart_root_prefix(&element)?;
                    let namespaces = chart_bindings(inherited, &element)?;
                    if !element_is_in_namespace(&element, C_NS, inherited)? {
                        return Err(ChartError::UnexpectedElement(element_name(&element)));
                    }
                    let (raw_attributes, _) = capture_fixed_root_attributes(
                        &element,
                        &["xmlns:c", "xmlns:a", "xmlns:r"],
                    )?;
                    return Ok(Self {
                        raw_attributes,
                        namespace_declarations: standalone_namespace_declarations(&namespaces)?,
                        ..Self::default()
                    });
                }
                Event::Start(element) | Event::Empty(element) => {
                    return Err(ChartError::UnexpectedElement(element_name(&element)));
                }
                Event::Eof => return Err(ChartError::MissingElement("c:dLbls".to_owned())),
                _ => {}
            }
            buffer.clear();
        }
    }

    fn from_element(
        reader: &mut Reader<&[u8]>,
        start: &BytesStart<'_>,
        inherited: &NamespaceBindings,
    ) -> Result<Self> {
        reject_conflicting_prefix(start, b"a", A_NS)?;
        let namespaces = chart_bindings(inherited, start)?;
        require_fixed_namespace(&namespaces, b"c", C_NS, start)?;
        let (raw_attributes, _) =
            capture_fixed_root_attributes(start, &["xmlns:c", "xmlns:a", "xmlns:r"])?;
        let namespace_declarations = standalone_namespace_declarations(&namespaces)?;
        let mut state = DataLabelsParseState::new(namespaces);
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element) => {
                    let name = chart_child_local(&element, &state.namespaces)?;
                    let raw = capture_element(reader, &element)?;
                    state.parse_child(name.as_deref().unwrap_or_default(), raw)?;
                }
                Event::Empty(element) => {
                    let name = chart_child_local(&element, &state.namespaces)?;
                    let raw = capture_empty_element(&element)?;
                    state.parse_child(name.as_deref().unwrap_or_default(), raw)?;
                }
                event @ (Event::Text(_)
                | Event::CData(_)
                | Event::Comment(_)
                | Event::PI(_)
                | Event::GeneralRef(_)) => state.capture_event(capture_event(event)?),
                Event::End(element) if matches_local_name(element.name().as_ref(), b"dLbls") => {
                    return state.finish(raw_attributes, namespace_declarations);
                }
                Event::Eof => return Err(missing_end("c:dLbls")),
                _ => {}
            }
            buffer.clear();
        }
    }

    pub fn to_xml(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new(Vec::new());
        self.write_xml(&mut writer, true)?;
        Ok(writer.into_inner())
    }

    fn write_xml(&self, writer: &mut Writer<Vec<u8>>, standalone: bool) -> Result<()> {
        self.validate()?;
        let mut start = BytesStart::new("c:dLbls");
        if standalone {
            start.push_attribute(("xmlns:c", C_NS));
            start.push_attribute(("xmlns:a", A_NS));
            start.push_attribute(("xmlns:r", R_NS));
        }
        push_attributes(&mut start, &self.namespace_declarations);
        push_attributes(&mut start, &self.raw_attributes);
        writer
            .write_event(Event::Start(start))
            .map_err(OxmlError::from)?;
        emit_raw(writer, self.raw_children.at(0))?;
        if let Some(number_format) = &self.number_format {
            number_format.write_xml(writer)?;
        }
        emit_raw(writer, self.raw_children.at(1))?;
        if let Some(position) = self.position {
            write_scalar(
                writer,
                "c:dLblPos",
                position.as_str(),
                self.position_markup.as_ref(),
            )?;
        }
        emit_raw(writer, self.raw_children.at(2))?;
        write_optional_bool(
            writer,
            "c:showLegendKey",
            self.show_legend_key,
            self.show_legend_key_markup.as_ref(),
        )?;
        emit_raw(writer, self.raw_children.at(3))?;
        write_optional_bool(
            writer,
            "c:showVal",
            self.show_value,
            self.show_value_markup.as_ref(),
        )?;
        emit_raw(writer, self.raw_children.at(4))?;
        write_optional_bool(
            writer,
            "c:showCatName",
            self.show_category_name,
            self.show_category_name_markup.as_ref(),
        )?;
        emit_raw(writer, self.raw_children.at(5))?;
        write_optional_bool(
            writer,
            "c:showSerName",
            self.show_series_name,
            self.show_series_name_markup.as_ref(),
        )?;
        emit_raw(writer, self.raw_children.at(6))?;
        write_optional_bool(
            writer,
            "c:showPercent",
            self.show_percent,
            self.show_percent_markup.as_ref(),
        )?;
        emit_raw(writer, self.raw_children.at(7))?;
        write_optional_bool(
            writer,
            "c:showBubbleSize",
            self.show_bubble_size,
            self.show_bubble_size_markup.as_ref(),
        )?;
        emit_raw(writer, self.raw_children.at(8))?;
        if let Some(separator) = &self.separator {
            write_text(
                writer,
                "c:separator",
                separator,
                self.separator_markup
                    .as_ref()
                    .unwrap_or(&TextMarkup::default()),
            )?;
        }
        emit_raw(writer, self.raw_children.at(9))?;
        writer
            .write_event(Event::End(BytesEnd::new("c:dLbls")))
            .map_err(OxmlError::from)?;
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        if let Some(number_format) = &self.number_format {
            number_format.validate()?;
        }
        if let Some(separator) = &self.separator {
            validate_xml_text(separator, "c:separator")?;
        }
        Ok(())
    }

    pub fn raw_children(&self) -> &OrderedRawChildren {
        &self.raw_children
    }
}

struct DataLabelsParseState {
    namespaces: NamespaceBindings,
    number_format: Option<NumberFormat>,
    position: Option<(DataLabelPosition, ScalarMarkup)>,
    show_legend_key: Option<(bool, ScalarMarkup)>,
    show_value: Option<(bool, ScalarMarkup)>,
    show_category_name: Option<(bool, ScalarMarkup)>,
    show_series_name: Option<(bool, ScalarMarkup)>,
    show_percent: Option<(bool, ScalarMarkup)>,
    show_bubble_size: Option<(bool, ScalarMarkup)>,
    separator: Option<(String, TextMarkup)>,
    raw_children: OrderedRawChildren,
    boundary: usize,
}

impl DataLabelsParseState {
    fn new(namespaces: NamespaceBindings) -> Self {
        Self {
            namespaces,
            number_format: None,
            position: None,
            show_legend_key: None,
            show_value: None,
            show_category_name: None,
            show_series_name: None,
            show_percent: None,
            show_bubble_size: None,
            separator: None,
            raw_children: OrderedRawChildren::default(),
            boundary: 0,
        }
    }

    fn capture_event(&mut self, raw: Vec<u8>) {
        self.raw_children.push(self.boundary, raw);
    }

    fn parse_child(&mut self, name: &[u8], raw: Vec<u8>) -> Result<()> {
        match name {
            b"numFmt" => {
                set_once(
                    &mut self.number_format,
                    NumberFormat::from_xml(&raw)?,
                    "c:numFmt",
                )?;
                self.boundary = self.boundary.max(1);
            }
            b"dLblPos" => {
                let (value, markup) = scalar_value(&raw, "dLblPos")?;
                let lexical = value
                    .ok_or_else(|| invalid_attribute("dLblPos", "val", "<missing>".to_owned()))?;
                let position = DataLabelPosition::parse(&lexical)
                    .ok_or_else(|| invalid_attribute("dLblPos", "val", lexical))?;
                set_once(&mut self.position, (position, markup), "c:dLblPos")?;
                self.boundary = self.boundary.max(2);
            }
            b"showLegendKey" => {
                set_once(
                    &mut self.show_legend_key,
                    parse_bool_value(&raw, "showLegendKey")?,
                    "c:showLegendKey",
                )?;
                self.boundary = self.boundary.max(3);
            }
            b"showVal" => {
                set_once(
                    &mut self.show_value,
                    parse_bool_value(&raw, "showVal")?,
                    "c:showVal",
                )?;
                self.boundary = self.boundary.max(4);
            }
            b"showCatName" => {
                set_once(
                    &mut self.show_category_name,
                    parse_bool_value(&raw, "showCatName")?,
                    "c:showCatName",
                )?;
                self.boundary = self.boundary.max(5);
            }
            b"showSerName" => {
                set_once(
                    &mut self.show_series_name,
                    parse_bool_value(&raw, "showSerName")?,
                    "c:showSerName",
                )?;
                self.boundary = self.boundary.max(6);
            }
            b"showPercent" => {
                set_once(
                    &mut self.show_percent,
                    parse_bool_value(&raw, "showPercent")?,
                    "c:showPercent",
                )?;
                self.boundary = self.boundary.max(7);
            }
            b"showBubbleSize" => {
                set_once(
                    &mut self.show_bubble_size,
                    parse_bool_value(&raw, "showBubbleSize")?,
                    "c:showBubbleSize",
                )?;
                self.boundary = self.boundary.max(8);
            }
            b"separator" => {
                set_once(
                    &mut self.separator,
                    parse_text_element(&raw, b"separator", &self.namespaces)?,
                    "c:separator",
                )?;
                self.boundary = self.boundary.max(9);
            }
            _ => {
                let boundary = data_labels_raw_boundary(name, self.boundary);
                self.raw_children.push(boundary, raw);
                self.boundary = self.boundary.max(boundary);
            }
        }
        Ok(())
    }

    fn finish(
        self,
        raw_attributes: XmlAttributes,
        namespace_declarations: XmlAttributes,
    ) -> Result<CT_DLbls> {
        let (position, position_markup) = optional_markup(self.position);
        let (show_legend_key, show_legend_key_markup) = optional_bool_markup(self.show_legend_key);
        let (show_value, show_value_markup) = optional_bool_markup(self.show_value);
        let (show_category_name, show_category_name_markup) =
            optional_bool_markup(self.show_category_name);
        let (show_series_name, show_series_name_markup) =
            optional_bool_markup(self.show_series_name);
        let (show_percent, show_percent_markup) = optional_bool_markup(self.show_percent);
        let (show_bubble_size, show_bubble_size_markup) =
            optional_bool_markup(self.show_bubble_size);
        let (separator, separator_markup) = self
            .separator
            .map(|(value, markup)| (Some(value), Some(markup)))
            .unwrap_or((None, None));
        let labels = CT_DLbls {
            number_format: self.number_format,
            position,
            separator,
            show_legend_key,
            show_value,
            show_category_name,
            show_series_name,
            show_percent,
            show_bubble_size,
            position_markup,
            show_legend_key_markup,
            show_value_markup,
            show_category_name_markup,
            show_series_name_markup,
            show_percent_markup,
            show_bubble_size_markup,
            separator_markup,
            raw_attributes,
            namespace_declarations,
            raw_children: raw_children_in_schema_order(&self.raw_children, 9),
        };
        labels.validate()?;
        Ok(labels)
    }
}

fn data_labels_raw_boundary(name: &[u8], current: usize) -> usize {
    match name {
        b"dLbl" | b"delete" => 0,
        b"spPr" | b"txPr" => 1,
        b"showLeaderLines" | b"leaderLines" | b"extLst" => 9,
        _ => current,
    }
}

fn optional_bool_markup(value: Option<(bool, ScalarMarkup)>) -> (bool, Option<ScalarMarkup>) {
    value
        .map(|(value, markup)| (value, Some(markup)))
        .unwrap_or((false, None))
}

fn write_optional_bool(
    writer: &mut Writer<Vec<u8>>,
    tag: &str,
    value: bool,
    markup: Option<&ScalarMarkup>,
) -> Result<()> {
    if value || markup.is_some() {
        write_scalar(writer, tag, bool_lexical(value), markup)?;
    }
    Ok(())
}

fn default_axis_id_markup(id: AxisId) -> AxisIdMarkup {
    AxisIdMarkup {
        scalar: ScalarMarkup::default(),
        parsed: id,
        lexical: id.value().to_string(),
    }
}

fn parse_axis_id_scalar(xml: &[u8], local: &str) -> Result<(AxisId, AxisIdMarkup)> {
    let (value, scalar) = scalar_value(xml, local)?;
    let lexical = value.ok_or_else(|| invalid_attribute(local, "val", "<missing>".to_owned()))?;
    let parsed = lexical
        .parse::<i64>()
        .map_err(|_| invalid_attribute(local, "val", lexical.clone()))?;
    let id = AxisId::new(parsed).map_err(|_| invalid_attribute(local, "val", lexical.clone()))?;
    Ok((
        id,
        AxisIdMarkup {
            scalar,
            parsed: id,
            lexical,
        },
    ))
}

fn write_axis_id<W: Write>(
    writer: &mut Writer<W>,
    tag: &str,
    id: AxisId,
    markup: &AxisIdMarkup,
) -> Result<()> {
    AxisId::new(id.value())?;
    let normalized;
    let value = if id == markup.parsed {
        markup.lexical.as_str()
    } else {
        normalized = id.value().to_string();
        normalized.as_str()
    };
    write_scalar(writer, tag, value, Some(&markup.scalar))
}

fn parse_f64_scalar(xml: &[u8], local: &str) -> Result<(f64, ScalarMarkup)> {
    let (value, markup) = scalar_value(xml, local)?;
    let value = value.ok_or_else(|| invalid_attribute(local, "val", "<missing>".to_owned()))?;
    let parsed = value
        .parse::<f64>()
        .map_err(|_| invalid_attribute(local, "val", value.clone()))?;
    if !parsed.is_finite() {
        return Err(invalid_attribute(local, "val", value));
    }
    Ok((parsed, markup))
}

fn parse_axis_position(xml: &[u8]) -> Result<(AxisPosition, ScalarMarkup)> {
    let (value, markup) = scalar_value(xml, "axPos")?;
    let value = value.ok_or_else(|| invalid_attribute("axPos", "val", "<missing>".to_owned()))?;
    let parsed =
        AxisPosition::parse(&value).ok_or_else(|| invalid_attribute("axPos", "val", value))?;
    Ok((parsed, markup))
}

fn parse_tick_mark(xml: &[u8], local: &str) -> Result<(TickMark, ScalarMarkup)> {
    let (value, markup) = scalar_value(xml, local)?;
    let value = value.ok_or_else(|| invalid_attribute(local, "val", "<missing>".to_owned()))?;
    let parsed = TickMark::parse(&value).ok_or_else(|| invalid_attribute(local, "val", value))?;
    Ok((parsed, markup))
}

fn parse_tick_label_position(xml: &[u8]) -> Result<(TickLabelPosition, ScalarMarkup)> {
    let (value, markup) = scalar_value(xml, "tickLblPos")?;
    let value =
        value.ok_or_else(|| invalid_attribute("tickLblPos", "val", "<missing>".to_owned()))?;
    let parsed = TickLabelPosition::parse(&value)
        .ok_or_else(|| invalid_attribute("tickLblPos", "val", value))?;
    Ok((parsed, markup))
}

fn optional_markup<T>(value: Option<(T, ScalarMarkup)>) -> (Option<T>, Option<ScalarMarkup>) {
    value
        .map(|(value, markup)| (Some(value), Some(markup)))
        .unwrap_or((None, None))
}

fn optional_scalar_markup_eq(left: &Option<ScalarMarkup>, right: &Option<ScalarMarkup>) -> bool {
    let empty = ScalarMarkup::default();
    left.as_ref().unwrap_or(&empty) == right.as_ref().unwrap_or(&empty)
}

fn axis_id_markup_eq(left: &AxisIdMarkup, right: &AxisIdMarkup) -> bool {
    left.scalar == right.scalar
}

const fn bool_lexical(value: bool) -> &'static str {
    if value { "1" } else { "0" }
}

fn parse_bool_lexical(element: &str, attribute: &str, value: &str) -> Result<bool> {
    match value {
        "1" | "true" => Ok(true),
        "0" | "false" => Ok(false),
        _ => Err(invalid_attribute(element, attribute, value.to_owned())),
    }
}

impl CT_Title {
    fn plain_text(text: &str) -> Self {
        let mut body = CT_TextBody::new();
        body.set_text(text);
        let mut raw_children = OrderedRawChildren::default();
        raw_children.push(1, br#"<c:layout/><c:overlay val="0"/>"#.to_vec());
        Self {
            text: Some(body),
            raw_children,
            ..Self::default()
        }
    }

    fn from_xml(xml: &[u8]) -> Result<Self> {
        let (raw_attributes, raw_children) = parse_raw_shell(xml, b"title", "c:title")?;
        Ok(Self {
            text: None,
            raw_attributes,
            raw_children,
        })
    }

    fn write_xml<W: Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        if self.text.is_none() {
            return write_raw_shell(writer, "c:title", &self.raw_attributes, &self.raw_children);
        }
        let mut start = BytesStart::new("c:title");
        push_attributes(&mut start, &self.raw_attributes);
        writer
            .write_event(Event::Start(start))
            .map_err(OxmlError::from)?;
        emit_raw(writer, self.raw_children.at(0))?;
        writer
            .write_event(Event::Start(BytesStart::new("c:tx")))
            .map_err(OxmlError::from)?;
        if let Some(text) = &self.text {
            text.write_xml_as(writer, "c:rich")?;
        }
        writer
            .write_event(Event::End(BytesEnd::new("c:tx")))
            .map_err(OxmlError::from)?;
        emit_raw(writer, self.raw_children.at(1))?;
        writer
            .write_event(Event::End(BytesEnd::new("c:title")))
            .map_err(OxmlError::from)?;
        Ok(())
    }

    pub fn raw_children(&self) -> &OrderedRawChildren {
        &self.raw_children
    }
}

impl CT_PlotArea {
    fn from_xml_with_namespaces(xml: &[u8], inherited: &NamespaceBindings) -> Result<Self> {
        let (raw_attributes, captured_children) = parse_raw_shell(xml, b"plotArea", "c:plotArea")?;
        let namespace_bindings = root_chart_bindings(xml, b"plotArea", inherited)?;
        let direct_roots: Vec<_> = captured_children
            .at(0)
            .filter_map(|raw| chart_root_local(raw, &namespace_bindings).transpose())
            .collect::<Result<_>>()?;
        let plot_roots: Vec<_> = direct_roots
            .iter()
            .filter(|local| is_plot_container(local))
            .cloned()
            .collect();
        let typed_local = (plot_roots.len() == 1
            && matches!(
                plot_roots[0].as_slice(),
                b"barChart"
                    | b"lineChart"
                    | b"pieChart"
                    | b"doughnutChart"
                    | b"areaChart"
                    | b"scatterChart"
                    | b"radarChart"
            ))
        .then(|| plot_roots[0].clone());

        let Some(typed_local) = typed_local else {
            return Ok(Self {
                raw_attributes,
                raw_children: captured_children,
                namespace_bindings,
                plots: None,
                plot_markup: Vec::new(),
                axes: Vec::new(),
            });
        };

        let mut raw_children = OrderedRawChildren::default();
        let mut plots = Vec::new();
        let mut plot_markup = Vec::new();
        let mut axes = Vec::new();
        let mut boundary = 0usize;
        for raw in captured_children.at(0) {
            let local = chart_root_local(raw, &namespace_bindings)?;
            if local.as_deref() == Some(typed_local.as_slice()) {
                let (plot, markup) = parse_plot(raw, &namespace_bindings)?;
                plots.push(plot);
                plot_markup.push(markup);
                boundary = 1;
            } else if local.as_deref().and_then(AxisKind::parse).is_some() {
                axes.push(Axis::from_xml_with_namespaces(raw, &namespace_bindings)?);
                boundary = axes.len() + 1;
            } else {
                raw_children.push(boundary, raw.to_vec());
            }
        }
        let plot_area = Self {
            raw_attributes,
            raw_children,
            namespace_bindings,
            plots: Some(plots),
            plot_markup,
            axes,
        };
        plot_area.validate_typed()?;
        Ok(plot_area)
    }

    fn write_xml<W: Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        let Some(plots) = &self.plots else {
            return write_raw_shell(
                writer,
                "c:plotArea",
                &self.raw_attributes,
                &self.raw_children,
            );
        };
        self.validate_typed()?;
        let mut start = BytesStart::new("c:plotArea");
        push_attributes(&mut start, &self.raw_attributes);
        writer
            .write_event(Event::Start(start))
            .map_err(OxmlError::from)?;
        emit_raw(writer, self.raw_children.at(0))?;
        for (index, plot) in plots.iter().enumerate() {
            write_plot(
                writer,
                plot,
                self.plot_markup
                    .get(index)
                    .unwrap_or(&PlotMarkup::default()),
            )?;
        }
        emit_raw(writer, self.raw_children.at(1))?;
        for (index, axis) in self.axes.iter().enumerate() {
            writer
                .get_mut()
                .write_all(&axis.to_xml()?)
                .map_err(OxmlError::from)?;
            emit_raw(writer, self.raw_children.at(index + 2))?;
        }
        writer
            .write_event(Event::End(BytesEnd::new("c:plotArea")))
            .map_err(OxmlError::from)?;
        Ok(())
    }

    pub fn raw_children(&self) -> &OrderedRawChildren {
        &self.raw_children
    }

    /// Creates a supported single-family plot area with owned axes.
    pub fn new(plots: Vec<Plot>, axes: Vec<Axis>) -> Result<Self> {
        let plot_markup = plots
            .iter()
            .map(|plot| {
                let mut markup = PlotMarkup::default();
                if matches!(plot, Plot::Line { .. }) {
                    markup.marker = Some(ScalarMarkup::default());
                    markup.smooth = Some(ScalarMarkup::default());
                }
                if matches!(plot, Plot::Doughnut { .. }) {
                    markup.hole_size = Some(ScalarMarkup::default());
                }
                markup
            })
            .collect();
        let plot_area = Self {
            raw_attributes: Vec::new(),
            raw_children: OrderedRawChildren::default(),
            namespace_bindings: chart_namespace_defaults(),
            plot_markup,
            plots: Some(plots),
            axes,
        };
        plot_area.validate_typed()?;
        Ok(plot_area)
    }

    /// Returns the owned supported plots, or an error for an opaque choice.
    pub fn plots(&self) -> Result<&[Plot]> {
        self.plots
            .as_deref()
            .ok_or_else(|| ChartError::InvalidValue {
                element: "c:plotArea".to_owned(),
                value: "unsupported or combination plot area is opaque".to_owned(),
            })
    }

    /// Returns mutable supported plots without exposing opaque content to edits.
    pub fn plots_mut(&mut self) -> Result<&mut [Plot]> {
        self.plots
            .as_deref_mut()
            .ok_or_else(|| ChartError::InvalidValue {
                element: "c:plotArea".to_owned(),
                value: "cannot combine typed plots with an opaque plot choice".to_owned(),
            })
    }

    /// Parses the common series payloads nested in category-based plot shells.
    pub fn series(&self) -> Result<Vec<Series>> {
        if let Some(plots) = &self.plots {
            return Ok(plots
                .iter()
                .flat_map(|plot| match plot {
                    Plot::Bar { series, .. }
                    | Plot::Line { series, .. }
                    | Plot::Pie { series, .. }
                    | Plot::Doughnut { series, .. }
                    | Plot::Area { series, .. }
                    | Plot::Scatter { series, .. }
                    | Plot::Radar { series, .. } => series.clone(),
                })
                .collect());
        }
        let mut series = Vec::new();
        for raw in self.raw_children.at(0) {
            parse_plot_series(raw, &self.namespace_bindings, &mut series)?;
        }
        Ok(series)
    }

    /// Parses and validates the direct axis children of this plot area.
    pub fn axes(&self) -> Result<Vec<Axis>> {
        if self.plots.is_some() {
            self.validate_typed()?;
            return Ok(self.axes.clone());
        }
        let mut axes = Vec::new();
        for raw in self.raw_children.at(0) {
            if let Some(axis) = parse_plot_axis(raw, &self.namespace_bindings)? {
                axes.push(axis);
            }
        }
        validate_axis_pairs(&axes)?;
        Ok(axes)
    }

    fn validate_typed(&self) -> Result<()> {
        let plots = self
            .plots
            .as_ref()
            .ok_or_else(|| ChartError::InvalidValue {
                element: "c:plotArea".to_owned(),
                value: "opaque plot area has no typed validation view".to_owned(),
            })?;
        if plots.len() != 1 {
            return Err(ChartError::InvalidValue {
                element: "c:plotArea".to_owned(),
                value: "typed plot area requires exactly one plot family".to_owned(),
            });
        }
        validate_axis_pairs(&self.axes)?;
        for plot in plots {
            plot.validate()?;
            if let Some(axis_ids) = plot.axis_ids() {
                for id in axis_ids {
                    if !self.axes.iter().any(|axis| axis.id == id) {
                        return Err(ChartError::InvalidValue {
                            element: "c:axId".to_owned(),
                            value: format!("plot references missing axis {}", id.value()),
                        });
                    }
                }
            } else if !self.axes.is_empty() {
                return Err(ChartError::InvalidValue {
                    element: "c:plotArea".to_owned(),
                    value: "pie and doughnut plot areas must not own axes".to_owned(),
                });
            }
            if plot.axis_ids().is_some() && self.axes.len() != 2 {
                return Err(ChartError::InvalidValue {
                    element: "c:plotArea".to_owned(),
                    value: format!(
                        "axis-owned plot requires exactly 2 axes, found {}",
                        self.axes.len()
                    ),
                });
            }
        }
        Ok(())
    }
}

impl Default for CT_PlotArea {
    fn default() -> Self {
        Self {
            raw_attributes: Vec::new(),
            raw_children: OrderedRawChildren::default(),
            namespace_bindings: chart_namespace_defaults(),
            plots: Some(Vec::new()),
            plot_markup: Vec::new(),
            axes: Vec::new(),
        }
    }
}

fn chart_root_local(xml: &[u8], inherited: &NamespaceBindings) -> Result<Option<Vec<u8>>> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) | Event::Empty(element) => {
                return Ok(element_is_in_namespace(&element, C_NS, inherited)?
                    .then(|| local_name(element.name().as_ref()).to_vec()));
            }
            Event::Eof => return Ok(None),
            _ => {}
        }
        buffer.clear();
    }
}

fn is_plot_container(local: &[u8]) -> bool {
    matches!(
        local,
        b"areaChart"
            | b"area3DChart"
            | b"barChart"
            | b"bar3DChart"
            | b"bubbleChart"
            | b"doughnutChart"
            | b"lineChart"
            | b"line3DChart"
            | b"ofPieChart"
            | b"pieChart"
            | b"pie3DChart"
            | b"radarChart"
            | b"scatterChart"
            | b"stockChart"
            | b"surfaceChart"
            | b"surface3DChart"
    )
}

fn parse_plot(xml: &[u8], inherited: &NamespaceBindings) -> Result<(Plot, PlotMarkup)> {
    let local = chart_root_local(xml, inherited)?
        .ok_or_else(|| ChartError::MissingElement("c:barChart or c:lineChart".to_owned()))?;
    match local.as_slice() {
        b"barChart" => parse_bar_plot(xml, inherited),
        b"lineChart" => parse_line_plot(xml, inherited),
        b"pieChart" | b"doughnutChart" | b"areaChart" | b"scatterChart" | b"radarChart" => {
            parse_remaining_plot(xml, inherited, &local)
        }
        _ => Err(ChartError::UnexpectedElement(format!(
            "c:{}",
            String::from_utf8_lossy(&local)
        ))),
    }
}

fn parse_remaining_plot(
    xml: &[u8],
    inherited: &NamespaceBindings,
    local: &[u8],
) -> Result<(Plot, PlotMarkup)> {
    let root = String::from_utf8_lossy(local).into_owned();
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) if matches_local_name(element.name().as_ref(), local) => {
                let namespaces = typed_rewrite_bindings(&element, inherited)?;
                let (mut raw_attributes, namespace_declarations) =
                    capture_fixed_root_attributes(&element, &["xmlns:c", "xmlns:a", "xmlns:r"])?;
                raw_attributes.splice(0..0, namespace_declarations);
                let mut grouping = None;
                let mut style = None;
                let mut first_slice_angle = None;
                let mut hole_size = None;
                let mut series = Vec::new();
                let mut data_labels = None;
                let mut axis_ids = Vec::new();
                let mut raw_children = OrderedRawChildren::default();
                let mut boundary = 0usize;
                let mut inner = Vec::new();
                loop {
                    match reader
                        .read_event_into(&mut inner)
                        .map_err(OxmlError::from)?
                    {
                        Event::Start(child) => {
                            let name = chart_child_local(&child, &namespaces)?;
                            let raw = capture_element(&mut reader, &child)?;
                            parse_remaining_plot_child(
                                local,
                                name.as_deref().unwrap_or_default(),
                                raw,
                                &namespaces,
                                &mut grouping,
                                &mut style,
                                &mut first_slice_angle,
                                &mut hole_size,
                                &mut series,
                                &mut data_labels,
                                &mut axis_ids,
                                &mut raw_children,
                                &mut boundary,
                            )?;
                        }
                        Event::Empty(child) => {
                            let name = chart_child_local(&child, &namespaces)?;
                            let raw = capture_empty_element(&child)?;
                            parse_remaining_plot_child(
                                local,
                                name.as_deref().unwrap_or_default(),
                                raw,
                                &namespaces,
                                &mut grouping,
                                &mut style,
                                &mut first_slice_angle,
                                &mut hole_size,
                                &mut series,
                                &mut data_labels,
                                &mut axis_ids,
                                &mut raw_children,
                                &mut boundary,
                            )?;
                        }
                        event @ (Event::Text(_)
                        | Event::CData(_)
                        | Event::Comment(_)
                        | Event::PI(_)
                        | Event::GeneralRef(_)) => {
                            raw_children.push(boundary, capture_event(event)?);
                        }
                        Event::End(end) if matches_local_name(end.name().as_ref(), local) => {
                            let needs_axes =
                                matches!(local, b"areaChart" | b"scatterChart" | b"radarChart");
                            if needs_axes && axis_ids.len() != 2 {
                                return Err(ChartError::InvalidValue {
                                    element: format!("c:{root}/c:axId"),
                                    value: format!("expected 2, found {}", axis_ids.len()),
                                });
                            }
                            if !needs_axes && !axis_ids.is_empty() {
                                return Err(ChartError::InvalidValue {
                                    element: format!("c:{root}/c:axId"),
                                    value: "axis-free plot contains axis references".to_owned(),
                                });
                            }
                            let parsed_ids = needs_axes.then(|| [axis_ids[0].0, axis_ids[1].0]);
                            let original_series_keys = series
                                .iter()
                                .map(|item| (item.index, item.order))
                                .collect::<Vec<_>>();
                            let plot = match local {
                                b"pieChart" => Plot::Pie {
                                    first_slice_angle: first_slice_angle
                                        .as_ref()
                                        .map_or(0, |item| item.0),
                                    series,
                                    data_labels,
                                },
                                b"doughnutChart" => Plot::Doughnut {
                                    first_slice_angle: first_slice_angle
                                        .as_ref()
                                        .map_or(0, |item| item.0),
                                    hole_size: hole_size.as_ref().map_or(50, |item| item.0),
                                    series,
                                    data_labels,
                                },
                                b"areaChart" => Plot::Area {
                                    grouping: grouping
                                        .as_ref()
                                        .ok_or_else(|| {
                                            ChartError::MissingElement("c:grouping".to_owned())
                                        })?
                                        .0,
                                    series,
                                    data_labels,
                                    axis_ids: parsed_ids.ok_or_else(|| {
                                        ChartError::MissingElement("c:areaChart/c:axId".to_owned())
                                    })?,
                                },
                                b"scatterChart" => {
                                    let lexical = style
                                        .as_ref()
                                        .ok_or_else(|| {
                                            ChartError::MissingElement("c:scatterStyle".to_owned())
                                        })?
                                        .0
                                        .as_str();
                                    Plot::Scatter {
                                        style: ScatterStyle::parse(lexical).ok_or_else(|| {
                                            invalid_attribute(
                                                "scatterStyle",
                                                "val",
                                                lexical.to_owned(),
                                            )
                                        })?,
                                        series,
                                        data_labels,
                                        axis_ids: parsed_ids.ok_or_else(|| {
                                            ChartError::MissingElement(
                                                "c:scatterChart/c:axId".to_owned(),
                                            )
                                        })?,
                                    }
                                }
                                b"radarChart" => {
                                    let lexical = style
                                        .as_ref()
                                        .ok_or_else(|| {
                                            ChartError::MissingElement("c:radarStyle".to_owned())
                                        })?
                                        .0
                                        .as_str();
                                    Plot::Radar {
                                        style: RadarStyle::parse(lexical).ok_or_else(|| {
                                            invalid_attribute(
                                                "radarStyle",
                                                "val",
                                                lexical.to_owned(),
                                            )
                                        })?,
                                        series,
                                        data_labels,
                                        axis_ids: parsed_ids.ok_or_else(|| {
                                            ChartError::MissingElement(
                                                "c:radarChart/c:axId".to_owned(),
                                            )
                                        })?,
                                    }
                                }
                                _ => {
                                    return Err(ChartError::UnexpectedElement(format!(
                                        "c:{}",
                                        String::from_utf8_lossy(local)
                                    )));
                                }
                            };
                            plot.validate()?;
                            let final_boundary = boundary.max(original_series_keys.len() + 6);
                            return Ok((
                                plot,
                                PlotMarkup {
                                    raw_attributes,
                                    raw_children: raw_children_in_schema_order(
                                        &raw_children,
                                        final_boundary,
                                    ),
                                    grouping: grouping.map(|item| item.1),
                                    first_slice_angle: first_slice_angle.map(|item| item.1),
                                    hole_size: hole_size.map(|item| item.1),
                                    style: style.map(|item| item.1),
                                    axis_ids: axis_ids.into_iter().map(|item| item.1).collect(),
                                    original_series_keys,
                                    original_axis_ids: parsed_ids
                                        .map_or_else(Vec::new, |ids| ids.to_vec()),
                                    parsed_remaining: match local {
                                        b"pieChart" => Some("pieChart"),
                                        b"doughnutChart" => Some("doughnutChart"),
                                        b"areaChart" => Some("areaChart"),
                                        b"scatterChart" => Some("scatterChart"),
                                        b"radarChart" => Some("radarChart"),
                                        _ => None,
                                    },
                                    ..PlotMarkup::default()
                                },
                            ));
                        }
                        Event::Eof => return Err(missing_end(&format!("c:{root}"))),
                        _ => {}
                    }
                    inner.clear();
                }
            }
            Event::Empty(element) if matches_local_name(element.name().as_ref(), local) => {
                return Err(ChartError::MissingElement("c:ser".to_owned()));
            }
            Event::Start(element) | Event::Empty(element) => {
                return Err(ChartError::UnexpectedElement(element_name(&element)));
            }
            Event::Eof => return Err(ChartError::MissingElement(format!("c:{root}"))),
            _ => {}
        }
        buffer.clear();
    }
}

#[allow(clippy::too_many_arguments)]
fn parse_remaining_plot_child(
    root: &[u8],
    name: &[u8],
    raw: Vec<u8>,
    namespaces: &NamespaceBindings,
    grouping: &mut Option<(Grouping, ScalarMarkup)>,
    style: &mut Option<(String, ScalarMarkup)>,
    first_slice_angle: &mut Option<(u16, ScalarMarkup)>,
    hole_size: &mut Option<(u8, ScalarMarkup)>,
    series: &mut Vec<Series>,
    data_labels: &mut Option<CT_DLbls>,
    axis_ids: &mut Vec<(AxisId, AxisIdMarkup)>,
    raw_children: &mut OrderedRawChildren,
    boundary: &mut usize,
) -> Result<()> {
    let property_first = matches!(root, b"areaChart" | b"scatterChart" | b"radarChart");
    let base = usize::from(property_first);
    match name {
        b"grouping" if root == b"areaChart" => {
            let (value, markup) = required_scalar(&raw, "grouping")?;
            let parsed = Grouping::parse(&value)
                .ok_or_else(|| invalid_attribute("grouping", "val", value))?;
            set_once(grouping, (parsed, markup), "c:grouping")?;
            *boundary = (*boundary).max(1);
        }
        b"scatterStyle" if root == b"scatterChart" => {
            let (value, markup) = required_scalar(&raw, "scatterStyle")?;
            if ScatterStyle::parse(&value).is_none() {
                return Err(invalid_attribute("scatterStyle", "val", value));
            }
            set_once(style, (value, markup), "c:scatterStyle")?;
            *boundary = (*boundary).max(1);
        }
        b"radarStyle" if root == b"radarChart" => {
            let (value, markup) = required_scalar(&raw, "radarStyle")?;
            if RadarStyle::parse(&value).is_none() {
                return Err(invalid_attribute("radarStyle", "val", value));
            }
            set_once(style, (value, markup), "c:radarStyle")?;
            *boundary = (*boundary).max(1);
        }
        b"ser" => {
            series.push(Series::from_xml_with_namespaces(&raw, namespaces)?);
            *boundary = (*boundary).max(base + series.len());
        }
        b"dLbls" => {
            set_once(
                data_labels,
                CT_DLbls::from_xml_with_namespaces(&raw, namespaces)?,
                "c:dLbls",
            )?;
            *boundary = (*boundary).max(base + series.len() + 1);
        }
        b"firstSliceAng" if matches!(root, b"pieChart" | b"doughnutChart") => {
            let (value, markup) = required_scalar(&raw, "firstSliceAng")?;
            let parsed = value
                .parse::<u16>()
                .map_err(|_| invalid_attribute("firstSliceAng", "val", value.clone()))?;
            if parsed > 360 {
                return Err(invalid_attribute("firstSliceAng", "val", value));
            }
            set_once(first_slice_angle, (parsed, markup), "c:firstSliceAng")?;
            *boundary = (*boundary).max(series.len() + 2);
        }
        b"holeSize" if root == b"doughnutChart" => {
            let (value, markup) = required_scalar(&raw, "holeSize")?;
            let parsed = value
                .parse::<u8>()
                .map_err(|_| invalid_attribute("holeSize", "val", value.clone()))?;
            if !(10..=90).contains(&parsed) {
                return Err(invalid_attribute("holeSize", "val", value));
            }
            set_once(hole_size, (parsed, markup), "c:holeSize")?;
            *boundary = (*boundary).max(series.len() + 3);
        }
        b"axId" if property_first => {
            if axis_ids.len() == 2 {
                if root != b"areaChart" {
                    return Err(ChartError::DuplicateElement("c:axId".to_owned()));
                }
                raw_children.push(base + series.len() + 3, raw);
                return Ok(());
            }
            axis_ids.push(parse_axis_id_scalar(&raw, "axId")?);
            *boundary = (*boundary).max(base + series.len() + 1 + axis_ids.len());
        }
        b"axId" => {
            return Err(ChartError::InvalidValue {
                element: "c:axId".to_owned(),
                value: "axis-free plot contains axis reference".to_owned(),
            });
        }
        _ => {
            let raw_boundary = match (root, name) {
                (_, b"varyColors") => base,
                (b"pieChart", b"extLst") => series.len() + 2,
                (b"doughnutChart", b"extLst") => series.len() + 3,
                (b"areaChart" | b"scatterChart" | b"radarChart", b"dropLines") => {
                    base + series.len() + 1
                }
                (b"areaChart" | b"scatterChart" | b"radarChart", b"extLst") => {
                    base + series.len() + 3
                }
                _ => *boundary,
            };
            raw_children.push(raw_boundary, raw);
            *boundary = (*boundary).max(raw_boundary);
        }
    }
    Ok(())
}

fn parse_bar_plot(xml: &[u8], inherited: &NamespaceBindings) -> Result<(Plot, PlotMarkup)> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) if matches_local_name(element.name().as_ref(), b"barChart") => {
                let namespaces = typed_rewrite_bindings(&element, inherited)?;
                let (mut raw_attributes, namespace_declarations) =
                    capture_fixed_root_attributes(&element, &["xmlns:c", "xmlns:a", "xmlns:r"])?;
                raw_attributes.splice(0..0, namespace_declarations);
                let mut direction = None;
                let mut grouping = None;
                let mut gap_width = None;
                let mut overlap = None;
                let mut series = Vec::new();
                let mut data_labels = None;
                let mut axis_ids = Vec::new();
                let mut raw_children = OrderedRawChildren::default();
                let mut boundary = 0usize;
                let mut inner = Vec::new();
                loop {
                    match reader
                        .read_event_into(&mut inner)
                        .map_err(OxmlError::from)?
                    {
                        Event::Start(child) => {
                            let name = chart_child_local(&child, &namespaces)?;
                            let raw = capture_element(&mut reader, &child)?;
                            parse_bar_child(
                                name.as_deref().unwrap_or_default(),
                                raw,
                                &namespaces,
                                &mut direction,
                                &mut grouping,
                                &mut gap_width,
                                &mut overlap,
                                &mut series,
                                &mut data_labels,
                                &mut axis_ids,
                                &mut raw_children,
                                &mut boundary,
                            )?;
                        }
                        Event::Empty(child) => {
                            let name = chart_child_local(&child, &namespaces)?;
                            let raw = capture_empty_element(&child)?;
                            parse_bar_child(
                                name.as_deref().unwrap_or_default(),
                                raw,
                                &namespaces,
                                &mut direction,
                                &mut grouping,
                                &mut gap_width,
                                &mut overlap,
                                &mut series,
                                &mut data_labels,
                                &mut axis_ids,
                                &mut raw_children,
                                &mut boundary,
                            )?;
                        }
                        event @ (Event::Text(_)
                        | Event::CData(_)
                        | Event::Comment(_)
                        | Event::PI(_)
                        | Event::GeneralRef(_)) => {
                            raw_children.push(boundary, capture_event(event)?);
                        }
                        Event::End(end) if matches_local_name(end.name().as_ref(), b"barChart") => {
                            let (direction, direction_markup) = direction
                                .ok_or_else(|| ChartError::MissingElement("c:barDir".to_owned()))?;
                            let (grouping, grouping_markup) = grouping.ok_or_else(|| {
                                ChartError::MissingElement("c:grouping".to_owned())
                            })?;
                            if axis_ids.len() != 2 {
                                return Err(ChartError::InvalidValue {
                                    element: "c:barChart/c:axId".to_owned(),
                                    value: format!("expected 2, found {}", axis_ids.len()),
                                });
                            }
                            let parsed_ids = [axis_ids[0].0, axis_ids[1].0];
                            let original_series_keys: Vec<(u32, u32)> =
                                series.iter().map(|item| (item.index, item.order)).collect();
                            let plot = Plot::Bar {
                                direction,
                                grouping,
                                gap_width: gap_width.as_ref().map_or(150, |item| item.0),
                                overlap: overlap.as_ref().map_or(0, |item| item.0),
                                series,
                                data_labels,
                                axis_ids: parsed_ids,
                            };
                            plot.validate()?;
                            return Ok((
                                plot,
                                PlotMarkup {
                                    raw_attributes,
                                    raw_children: raw_children_in_schema_order(
                                        &raw_children,
                                        original_series_keys.len() + 7,
                                    ),
                                    direction: Some(direction_markup),
                                    grouping: Some(grouping_markup),
                                    gap_width: gap_width.map(|item| item.1),
                                    overlap: overlap.map(|item| item.1),
                                    marker: None,
                                    smooth: None,
                                    first_slice_angle: None,
                                    hole_size: None,
                                    style: None,
                                    axis_ids: axis_ids.into_iter().map(|item| item.1).collect(),
                                    original_series_keys,
                                    original_axis_ids: parsed_ids.to_vec(),
                                    parsed_bar: Some(true),
                                    parsed_remaining: None,
                                },
                            ));
                        }
                        Event::Eof => return Err(missing_end("c:barChart")),
                        _ => {}
                    }
                    inner.clear();
                }
            }
            Event::Empty(element) if matches_local_name(element.name().as_ref(), b"barChart") => {
                return Err(ChartError::MissingElement("c:barDir".to_owned()));
            }
            Event::Start(element) | Event::Empty(element) => {
                return Err(ChartError::UnexpectedElement(element_name(&element)));
            }
            Event::Eof => return Err(ChartError::MissingElement("c:barChart".to_owned())),
            _ => {}
        }
        buffer.clear();
    }
}

#[allow(clippy::too_many_arguments)]
fn parse_bar_child(
    name: &[u8],
    raw: Vec<u8>,
    namespaces: &NamespaceBindings,
    direction: &mut Option<(BarDirection, ScalarMarkup)>,
    grouping: &mut Option<(BarGrouping, ScalarMarkup)>,
    gap_width: &mut Option<(u16, ScalarMarkup)>,
    overlap: &mut Option<(i8, ScalarMarkup)>,
    series: &mut Vec<Series>,
    data_labels: &mut Option<CT_DLbls>,
    axis_ids: &mut Vec<(AxisId, AxisIdMarkup)>,
    raw_children: &mut OrderedRawChildren,
    boundary: &mut usize,
) -> Result<()> {
    match name {
        b"barDir" => {
            let (value, markup) = required_scalar(&raw, "barDir")?;
            let value = BarDirection::parse(&value)
                .ok_or_else(|| invalid_attribute("barDir", "val", value))?;
            set_once(direction, (value, markup), "c:barDir")?;
            *boundary = (*boundary).max(1);
        }
        b"grouping" => {
            let (value, markup) = required_scalar(&raw, "grouping")?;
            let value = BarGrouping::parse(&value)
                .ok_or_else(|| invalid_attribute("grouping", "val", value))?;
            set_once(grouping, (value, markup), "c:grouping")?;
            *boundary = (*boundary).max(2);
        }
        b"ser" => {
            series.push(Series::from_xml_with_namespaces(&raw, namespaces)?);
            *boundary = (*boundary).max(series.len() + 2);
        }
        b"dLbls" => {
            set_once(
                data_labels,
                CT_DLbls::from_xml_with_namespaces(&raw, namespaces)?,
                "c:dLbls",
            )?;
            *boundary = (*boundary).max(series.len() + 3);
        }
        b"gapWidth" => {
            let (value, markup) = required_scalar(&raw, "gapWidth")?;
            let parsed = value
                .parse::<u16>()
                .map_err(|_| invalid_attribute("gapWidth", "val", value.clone()))?;
            if parsed > 500 {
                return Err(invalid_attribute("gapWidth", "val", value));
            }
            set_once(gap_width, (parsed, markup), "c:gapWidth")?;
            *boundary = (*boundary).max(series.len() + 4);
        }
        b"overlap" => {
            let (value, markup) = required_scalar(&raw, "overlap")?;
            let parsed = value
                .parse::<i8>()
                .map_err(|_| invalid_attribute("overlap", "val", value.clone()))?;
            if !(-100..=100).contains(&parsed) {
                return Err(invalid_attribute("overlap", "val", value));
            }
            set_once(overlap, (parsed, markup), "c:overlap")?;
            *boundary = (*boundary).max(series.len() + 5);
        }
        b"axId" => {
            if axis_ids.len() == 2 {
                raw_children.push(series.len() + 7, raw);
                return Ok(());
            }
            axis_ids.push(parse_axis_id_scalar(&raw, "axId")?);
            *boundary = (*boundary).max(series.len() + 5 + axis_ids.len());
        }
        _ => raw_children.push(*boundary, raw),
    }
    Ok(())
}

fn parse_line_plot(xml: &[u8], inherited: &NamespaceBindings) -> Result<(Plot, PlotMarkup)> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) if matches_local_name(element.name().as_ref(), b"lineChart") => {
                let namespaces = typed_rewrite_bindings(&element, inherited)?;
                let (mut raw_attributes, namespace_declarations) =
                    capture_fixed_root_attributes(&element, &["xmlns:c", "xmlns:a", "xmlns:r"])?;
                raw_attributes.splice(0..0, namespace_declarations);
                let mut grouping = None;
                let mut marker = None;
                let mut smooth = None;
                let mut series = Vec::new();
                let mut data_labels = None;
                let mut axis_ids = Vec::new();
                let mut raw_children = OrderedRawChildren::default();
                let mut boundary = 0usize;
                let mut inner = Vec::new();
                loop {
                    match reader
                        .read_event_into(&mut inner)
                        .map_err(OxmlError::from)?
                    {
                        Event::Start(child) => {
                            let name = chart_child_local(&child, &namespaces)?;
                            let raw = capture_element(&mut reader, &child)?;
                            parse_line_child(
                                name.as_deref().unwrap_or_default(),
                                raw,
                                &namespaces,
                                &mut grouping,
                                &mut marker,
                                &mut smooth,
                                &mut series,
                                &mut data_labels,
                                &mut axis_ids,
                                &mut raw_children,
                                &mut boundary,
                            )?;
                        }
                        Event::Empty(child) => {
                            let name = chart_child_local(&child, &namespaces)?;
                            let raw = capture_empty_element(&child)?;
                            parse_line_child(
                                name.as_deref().unwrap_or_default(),
                                raw,
                                &namespaces,
                                &mut grouping,
                                &mut marker,
                                &mut smooth,
                                &mut series,
                                &mut data_labels,
                                &mut axis_ids,
                                &mut raw_children,
                                &mut boundary,
                            )?;
                        }
                        event @ (Event::Text(_)
                        | Event::CData(_)
                        | Event::Comment(_)
                        | Event::PI(_)
                        | Event::GeneralRef(_)) => {
                            raw_children.push(boundary, capture_event(event)?);
                        }
                        Event::End(end)
                            if matches_local_name(end.name().as_ref(), b"lineChart") =>
                        {
                            let (grouping, grouping_markup) = grouping.ok_or_else(|| {
                                ChartError::MissingElement("c:grouping".to_owned())
                            })?;
                            if axis_ids.len() != 2 {
                                return Err(ChartError::InvalidValue {
                                    element: "c:lineChart/c:axId".to_owned(),
                                    value: format!("expected 2, found {}", axis_ids.len()),
                                });
                            }
                            let parsed_ids = [axis_ids[0].0, axis_ids[1].0];
                            let original_series_keys: Vec<(u32, u32)> =
                                series.iter().map(|item| (item.index, item.order)).collect();
                            let plot = Plot::Line {
                                grouping,
                                marker: marker.as_ref().is_some_and(|item| item.0),
                                smooth: smooth.as_ref().is_some_and(|item| item.0),
                                series,
                                data_labels,
                                axis_ids: parsed_ids,
                            };
                            plot.validate()?;
                            return Ok((
                                plot,
                                PlotMarkup {
                                    raw_attributes,
                                    raw_children: raw_children_in_schema_order(
                                        &raw_children,
                                        original_series_keys.len() + 6,
                                    ),
                                    direction: None,
                                    grouping: Some(grouping_markup),
                                    gap_width: None,
                                    overlap: None,
                                    marker: marker.map(|item| item.1),
                                    smooth: smooth.map(|item| item.1),
                                    first_slice_angle: None,
                                    hole_size: None,
                                    style: None,
                                    axis_ids: axis_ids.into_iter().map(|item| item.1).collect(),
                                    original_series_keys,
                                    original_axis_ids: parsed_ids.to_vec(),
                                    parsed_bar: Some(false),
                                    parsed_remaining: None,
                                },
                            ));
                        }
                        Event::Eof => return Err(missing_end("c:lineChart")),
                        _ => {}
                    }
                    inner.clear();
                }
            }
            Event::Empty(element) if matches_local_name(element.name().as_ref(), b"lineChart") => {
                return Err(ChartError::MissingElement("c:grouping".to_owned()));
            }
            Event::Start(element) | Event::Empty(element) => {
                return Err(ChartError::UnexpectedElement(element_name(&element)));
            }
            Event::Eof => return Err(ChartError::MissingElement("c:lineChart".to_owned())),
            _ => {}
        }
        buffer.clear();
    }
}

#[allow(clippy::too_many_arguments)]
fn parse_line_child(
    name: &[u8],
    raw: Vec<u8>,
    namespaces: &NamespaceBindings,
    grouping: &mut Option<(Grouping, ScalarMarkup)>,
    marker: &mut Option<(bool, ScalarMarkup)>,
    smooth: &mut Option<(bool, ScalarMarkup)>,
    series: &mut Vec<Series>,
    data_labels: &mut Option<CT_DLbls>,
    axis_ids: &mut Vec<(AxisId, AxisIdMarkup)>,
    raw_children: &mut OrderedRawChildren,
    boundary: &mut usize,
) -> Result<()> {
    match name {
        b"grouping" => {
            let (value, markup) = required_scalar(&raw, "grouping")?;
            let value = Grouping::parse(&value)
                .ok_or_else(|| invalid_attribute("grouping", "val", value))?;
            set_once(grouping, (value, markup), "c:grouping")?;
            *boundary = (*boundary).max(1);
        }
        b"ser" => {
            series.push(Series::from_xml_with_namespaces(&raw, namespaces)?);
            *boundary = (*boundary).max(series.len() + 1);
        }
        b"dLbls" => {
            set_once(
                data_labels,
                CT_DLbls::from_xml_with_namespaces(&raw, namespaces)?,
                "c:dLbls",
            )?;
            *boundary = (*boundary).max(series.len() + 2);
        }
        b"marker" => {
            set_once(marker, parse_bool_value(&raw, "marker")?, "c:marker")?;
            *boundary = (*boundary).max(series.len() + 3);
        }
        b"smooth" => {
            set_once(smooth, parse_bool_value(&raw, "smooth")?, "c:smooth")?;
            *boundary = (*boundary).max(series.len() + 4);
        }
        b"axId" => {
            if axis_ids.len() == 2 {
                raw_children.push(series.len() + 6, raw);
                return Ok(());
            }
            axis_ids.push(parse_axis_id_scalar(&raw, "axId")?);
            *boundary = (*boundary).max(series.len() + 4 + axis_ids.len());
        }
        _ => raw_children.push(*boundary, raw),
    }
    Ok(())
}

fn required_scalar(xml: &[u8], local: &str) -> Result<(String, ScalarMarkup)> {
    let (value, markup) = scalar_value(xml, local)?;
    let value = value.ok_or_else(|| invalid_attribute(local, "val", "<missing>".to_owned()))?;
    Ok((value, markup))
}

fn write_plot<W: Write>(writer: &mut Writer<W>, plot: &Plot, markup: &PlotMarkup) -> Result<()> {
    plot.validate()?;
    if let Some(parsed_bar) = markup.parsed_bar
        && parsed_bar != matches!(plot, Plot::Bar { .. })
    {
        return Err(ChartError::InvalidValue {
            element: "c:plotArea".to_owned(),
            value: "a parsed plot family cannot be replaced while preserved payload remains"
                .to_owned(),
        });
    }
    let current_remaining = match plot {
        Plot::Pie { .. } => Some("pieChart"),
        Plot::Doughnut { .. } => Some("doughnutChart"),
        Plot::Area { .. } => Some("areaChart"),
        Plot::Scatter { .. } => Some("scatterChart"),
        Plot::Radar { .. } => Some("radarChart"),
        Plot::Bar { .. } | Plot::Line { .. } => None,
    };
    if let Some(parsed) = markup.parsed_remaining
        && current_remaining != Some(parsed)
    {
        return Err(ChartError::InvalidValue {
            element: "c:plotArea".to_owned(),
            value: "a parsed plot family cannot be replaced while preserved payload remains"
                .to_owned(),
        });
    }
    match plot {
        Plot::Bar {
            direction,
            grouping,
            gap_width,
            overlap,
            series,
            data_labels,
            axis_ids,
        } => {
            let mut start = BytesStart::new("c:barChart");
            push_attributes(&mut start, &markup.raw_attributes);
            writer
                .write_event(Event::Start(start))
                .map_err(OxmlError::from)?;
            emit_raw(writer, markup.raw_children.at(0))?;
            write_scalar(
                writer,
                "c:barDir",
                direction.as_str(),
                markup.direction.as_ref(),
            )?;
            emit_raw(writer, markup.raw_children.at(1))?;
            write_scalar(
                writer,
                "c:grouping",
                grouping.as_str(),
                markup.grouping.as_ref(),
            )?;
            let original_to_current =
                series_original_to_current(&markup.original_series_keys, series);
            emit_repeated_raw(
                writer,
                &markup.raw_children,
                2,
                &original_to_current,
                series.len(),
                0,
            )?;
            for (index, item) in series.iter().enumerate() {
                writer
                    .get_mut()
                    .write_all(&item.to_xml()?)
                    .map_err(OxmlError::from)?;
                emit_repeated_raw(
                    writer,
                    &markup.raw_children,
                    2,
                    &original_to_current,
                    series.len(),
                    index + 1,
                )?;
            }
            if let Some(labels) = data_labels {
                writer
                    .get_mut()
                    .write_all(&labels.to_xml()?)
                    .map_err(OxmlError::from)?;
            }
            let trailing = markup.original_series_keys.len();
            emit_raw(writer, markup.raw_children.at(trailing + 3))?;
            if *gap_width != 150 || markup.gap_width.is_some() {
                write_scalar(
                    writer,
                    "c:gapWidth",
                    &gap_width.to_string(),
                    markup.gap_width.as_ref(),
                )?;
            }
            emit_raw(writer, markup.raw_children.at(trailing + 4))?;
            if *overlap != 0 || markup.overlap.is_some() {
                write_scalar(
                    writer,
                    "c:overlap",
                    &overlap.to_string(),
                    markup.overlap.as_ref(),
                )?;
            }
            let axis_original_to_current =
                axis_original_to_current(&markup.original_axis_ids, axis_ids);
            let axis_current_to_original =
                invert_original_to_current(&axis_original_to_current, axis_ids.len());
            emit_repeated_raw(
                writer,
                &markup.raw_children,
                trailing + 5,
                &axis_original_to_current,
                axis_ids.len(),
                0,
            )?;
            for (index, id) in axis_ids.iter().enumerate() {
                let default_markup = default_axis_id_markup(*id);
                let original_markup = axis_current_to_original[index]
                    .and_then(|original| markup.axis_ids.get(original));
                write_axis_id(
                    writer,
                    "c:axId",
                    *id,
                    original_markup.unwrap_or(&default_markup),
                )?;
                emit_repeated_raw(
                    writer,
                    &markup.raw_children,
                    trailing + 5,
                    &axis_original_to_current,
                    axis_ids.len(),
                    index + 1,
                )?;
            }
            writer
                .write_event(Event::End(BytesEnd::new("c:barChart")))
                .map_err(OxmlError::from)?;
        }
        Plot::Line {
            grouping,
            marker,
            smooth,
            series,
            data_labels,
            axis_ids,
        } => {
            let mut start = BytesStart::new("c:lineChart");
            push_attributes(&mut start, &markup.raw_attributes);
            writer
                .write_event(Event::Start(start))
                .map_err(OxmlError::from)?;
            emit_raw(writer, markup.raw_children.at(0))?;
            write_scalar(
                writer,
                "c:grouping",
                grouping.as_str(),
                markup.grouping.as_ref(),
            )?;
            let original_to_current =
                series_original_to_current(&markup.original_series_keys, series);
            emit_repeated_raw(
                writer,
                &markup.raw_children,
                1,
                &original_to_current,
                series.len(),
                0,
            )?;
            for (index, item) in series.iter().enumerate() {
                writer
                    .get_mut()
                    .write_all(&item.to_xml()?)
                    .map_err(OxmlError::from)?;
                emit_repeated_raw(
                    writer,
                    &markup.raw_children,
                    1,
                    &original_to_current,
                    series.len(),
                    index + 1,
                )?;
            }
            if let Some(labels) = data_labels {
                writer
                    .get_mut()
                    .write_all(&labels.to_xml()?)
                    .map_err(OxmlError::from)?;
            }
            let trailing = markup.original_series_keys.len();
            emit_raw(writer, markup.raw_children.at(trailing + 2))?;
            if *marker || markup.marker.is_some() {
                write_scalar(
                    writer,
                    "c:marker",
                    bool_lexical(*marker),
                    markup.marker.as_ref(),
                )?;
            }
            emit_raw(writer, markup.raw_children.at(trailing + 3))?;
            if *smooth || markup.smooth.is_some() {
                write_scalar(
                    writer,
                    "c:smooth",
                    bool_lexical(*smooth),
                    markup.smooth.as_ref(),
                )?;
            }
            let axis_original_to_current =
                axis_original_to_current(&markup.original_axis_ids, axis_ids);
            let axis_current_to_original =
                invert_original_to_current(&axis_original_to_current, axis_ids.len());
            emit_repeated_raw(
                writer,
                &markup.raw_children,
                trailing + 4,
                &axis_original_to_current,
                axis_ids.len(),
                0,
            )?;
            for (index, id) in axis_ids.iter().enumerate() {
                let default_markup = default_axis_id_markup(*id);
                let original_markup = axis_current_to_original[index]
                    .and_then(|original| markup.axis_ids.get(original));
                write_axis_id(
                    writer,
                    "c:axId",
                    *id,
                    original_markup.unwrap_or(&default_markup),
                )?;
                emit_repeated_raw(
                    writer,
                    &markup.raw_children,
                    trailing + 4,
                    &axis_original_to_current,
                    axis_ids.len(),
                    index + 1,
                )?;
            }
            writer
                .write_event(Event::End(BytesEnd::new("c:lineChart")))
                .map_err(OxmlError::from)?;
        }
        remaining => {
            let (
                root,
                property,
                series,
                data_labels,
                axis_ids,
                first_slice_angle,
                hole_size,
                scatter,
            ) = match remaining {
                Plot::Pie {
                    first_slice_angle,
                    series,
                    data_labels,
                } => (
                    "pieChart",
                    None,
                    series,
                    data_labels,
                    None,
                    Some(*first_slice_angle),
                    None,
                    false,
                ),
                Plot::Doughnut {
                    first_slice_angle,
                    hole_size,
                    series,
                    data_labels,
                } => (
                    "doughnutChart",
                    None,
                    series,
                    data_labels,
                    None,
                    Some(*first_slice_angle),
                    Some(*hole_size),
                    false,
                ),
                Plot::Area {
                    grouping,
                    series,
                    data_labels,
                    axis_ids,
                } => (
                    "areaChart",
                    Some(("c:grouping", grouping.as_str(), markup.grouping.as_ref())),
                    series,
                    data_labels,
                    Some(axis_ids),
                    None,
                    None,
                    false,
                ),
                Plot::Scatter {
                    style,
                    series,
                    data_labels,
                    axis_ids,
                } => (
                    "scatterChart",
                    Some(("c:scatterStyle", style.as_str(), markup.style.as_ref())),
                    series,
                    data_labels,
                    Some(axis_ids),
                    None,
                    None,
                    true,
                ),
                Plot::Radar {
                    style,
                    series,
                    data_labels,
                    axis_ids,
                } => (
                    "radarChart",
                    Some(("c:radarStyle", style.as_str(), markup.style.as_ref())),
                    series,
                    data_labels,
                    Some(axis_ids),
                    None,
                    None,
                    false,
                ),
                Plot::Bar { .. } | Plot::Line { .. } => unreachable!("handled above"),
            };
            let tag = format!("c:{root}");
            let mut start = BytesStart::new(&tag);
            push_attributes(&mut start, &markup.raw_attributes);
            writer
                .write_event(Event::Start(start))
                .map_err(OxmlError::from)?;
            let base = usize::from(property.is_some());
            if let Some((name, value, scalar_markup)) = property {
                emit_raw(writer, markup.raw_children.at(0))?;
                write_scalar(writer, name, value, scalar_markup)?;
            }
            let original_to_current =
                series_original_to_current(&markup.original_series_keys, series);
            emit_repeated_raw(
                writer,
                &markup.raw_children,
                base,
                &original_to_current,
                series.len(),
                0,
            )?;
            for (index, item) in series.iter().enumerate() {
                writer
                    .get_mut()
                    .write_all(&item.to_xml_for_plot(scatter)?)
                    .map_err(OxmlError::from)?;
                emit_repeated_raw(
                    writer,
                    &markup.raw_children,
                    base,
                    &original_to_current,
                    series.len(),
                    index + 1,
                )?;
            }
            if let Some(labels) = data_labels {
                writer
                    .get_mut()
                    .write_all(&labels.to_xml()?)
                    .map_err(OxmlError::from)?;
            }
            let trailing = base + markup.original_series_keys.len();
            if let Some(axis_ids) = axis_ids {
                let axis_original_to_current =
                    axis_original_to_current(&markup.original_axis_ids, axis_ids);
                let axis_current_to_original =
                    invert_original_to_current(&axis_original_to_current, axis_ids.len());
                emit_repeated_raw(
                    writer,
                    &markup.raw_children,
                    trailing + 1,
                    &axis_original_to_current,
                    axis_ids.len(),
                    0,
                )?;
                for (index, id) in axis_ids.iter().enumerate() {
                    let default_markup = default_axis_id_markup(*id);
                    let original_markup = axis_current_to_original[index]
                        .and_then(|original| markup.axis_ids.get(original));
                    write_axis_id(
                        writer,
                        "c:axId",
                        *id,
                        original_markup.unwrap_or(&default_markup),
                    )?;
                    emit_repeated_raw(
                        writer,
                        &markup.raw_children,
                        trailing + 1,
                        &axis_original_to_current,
                        axis_ids.len(),
                        index + 1,
                    )?;
                }
            } else {
                emit_raw(writer, markup.raw_children.at(trailing + 1))?;
                if let Some(angle) = first_slice_angle {
                    if angle != 0 || markup.first_slice_angle.is_some() {
                        write_scalar(
                            writer,
                            "c:firstSliceAng",
                            &angle.to_string(),
                            markup.first_slice_angle.as_ref(),
                        )?;
                    }
                    emit_raw(writer, markup.raw_children.at(trailing + 2))?;
                }
                if let Some(size) = hole_size {
                    if size != 50 || markup.hole_size.is_some() {
                        write_scalar(
                            writer,
                            "c:holeSize",
                            &size.to_string(),
                            markup.hole_size.as_ref(),
                        )?;
                    }
                    emit_raw(writer, markup.raw_children.at(trailing + 3))?;
                }
            }
            writer
                .write_event(Event::End(BytesEnd::new(&tag)))
                .map_err(OxmlError::from)?;
        }
    }
    Ok(())
}

fn series_original_to_current(original: &[(u32, u32)], current: &[Series]) -> Vec<Option<usize>> {
    let mut used = vec![false; current.len()];
    let mut matches = vec![None; original.len()];
    for original_index in 0..original.len().min(current.len()) {
        if original[original_index]
            == (current[original_index].index, current[original_index].order)
        {
            matches[original_index] = Some(original_index);
            used[original_index] = true;
        }
    }
    for (original_index, key) in original.iter().enumerate() {
        if matches[original_index].is_some() {
            continue;
        }
        if let Some(current_index) = current
            .iter()
            .enumerate()
            .position(|(index, item)| !used[index] && (item.index, item.order) == *key)
        {
            matches[original_index] = Some(current_index);
            used[current_index] = true;
        }
    }
    match_unidentified_by_position(&mut matches, &mut used);
    matches
}

fn axis_original_to_current(original: &[AxisId], current: &[AxisId]) -> Vec<Option<usize>> {
    let mut used = vec![false; current.len()];
    let mut matches = vec![None; original.len()];
    for (original_index, id) in original.iter().enumerate() {
        if let Some(current_index) = current
            .iter()
            .enumerate()
            .position(|(index, current_id)| !used[index] && current_id == id)
        {
            matches[original_index] = Some(current_index);
            used[current_index] = true;
        }
    }
    match_unidentified_by_position(&mut matches, &mut used);
    matches
}

fn match_unidentified_by_position(matches: &mut [Option<usize>], used: &mut [bool]) {
    for original_index in 0..matches.len() {
        if matches[original_index].is_none() && original_index < used.len() && !used[original_index]
        {
            matches[original_index] = Some(original_index);
            used[original_index] = true;
        }
    }
    for matched in matches.iter_mut().filter(|matched| matched.is_none()) {
        if let Some(current_index) = used.iter().position(|used| !used) {
            *matched = Some(current_index);
            used[current_index] = true;
        }
    }
}

fn invert_original_to_current(
    original_to_current: &[Option<usize>],
    current_len: usize,
) -> Vec<Option<usize>> {
    let mut current_to_original = vec![None; current_len];
    for (original, current) in original_to_current.iter().enumerate() {
        if let Some(current) = current {
            current_to_original[*current] = Some(original);
        }
    }
    current_to_original
}

fn emit_repeated_raw<W: Write>(
    writer: &mut Writer<W>,
    raw_children: &OrderedRawChildren,
    offset: usize,
    original_to_current: &[Option<usize>],
    current_len: usize,
    current_boundary: usize,
) -> Result<()> {
    if current_boundary == 0 {
        emit_raw(writer, raw_children.at(offset))?;
    }
    for original_boundary in 1..=original_to_current.len() {
        let effective = original_to_current
            .iter()
            .skip(original_boundary)
            .flatten()
            .copied()
            .next()
            .unwrap_or(current_len);
        if effective == current_boundary {
            emit_raw(writer, raw_children.at(offset + original_boundary))?;
        }
    }
    Ok(())
}

fn parse_plot_axis(xml: &[u8], inherited: &NamespaceBindings) -> Result<Option<Axis>> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) | Event::Empty(element) => {
                if !element_is_in_namespace(&element, C_NS, inherited)? {
                    return Ok(None);
                }
                let name = element.name();
                if AxisKind::parse(local_name(name.as_ref())).is_none() {
                    return Ok(None);
                }
                return Axis::from_xml_with_namespaces(xml, inherited).map(Some);
            }
            Event::Eof => return Ok(None),
            _ => {}
        }
        buffer.clear();
    }
}

fn validate_axis_pairs(axes: &[Axis]) -> Result<()> {
    for (index, axis) in axes.iter().enumerate() {
        if axes[..index].iter().any(|other| other.id == axis.id) {
            return Err(ChartError::DuplicateElement(format!(
                "c:axId {}",
                axis.id.value()
            )));
        }
    }
    for axis in axes {
        if axis.id == axis.cross_axis {
            return Err(ChartError::InvalidValue {
                element: "c:crossAx".to_owned(),
                value: format!("axis {} crosses itself", axis.id.value()),
            });
        }
        let crossed = axes
            .iter()
            .find(|candidate| candidate.id == axis.cross_axis)
            .ok_or_else(|| ChartError::InvalidValue {
                element: "c:crossAx".to_owned(),
                value: format!(
                    "axis {} references missing axis {}",
                    axis.id.value(),
                    axis.cross_axis.value()
                ),
            })?;
        if crossed.cross_axis != axis.id {
            return Err(ChartError::InvalidValue {
                element: "c:crossAx".to_owned(),
                value: format!(
                    "axis {} references {}, which references {}",
                    axis.id.value(),
                    crossed.id.value(),
                    crossed.cross_axis.value()
                ),
            });
        }
    }
    Ok(())
}

fn parse_plot_series(
    xml: &[u8],
    inherited: &NamespaceBindings,
    series: &mut Vec<Series>,
) -> Result<()> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) => {
                if !element_is_in_namespace(&element, C_NS, inherited)? {
                    return Ok(());
                }
                let local = local_name(element.name().as_ref()).to_vec();
                if !is_supported_series_plot(&local) {
                    return Ok(());
                }
                let namespaces = chart_bindings(inherited, &element)?;
                let mut inner = Vec::new();
                loop {
                    match reader
                        .read_event_into(&mut inner)
                        .map_err(OxmlError::from)?
                    {
                        Event::Start(child) => {
                            let name = chart_child_local(&child, &namespaces)?;
                            let raw = capture_element(&mut reader, &child)?;
                            if name.as_deref() == Some(b"ser") {
                                series.push(Series::from_xml_with_namespaces(&raw, &namespaces)?);
                            }
                        }
                        Event::Empty(child) => {
                            let name = chart_child_local(&child, &namespaces)?;
                            if name.as_deref() == Some(b"ser") {
                                return Err(ChartError::MissingElement("c:ser/c:idx".to_owned()));
                            }
                        }
                        Event::End(end) if matches_local_name(end.name().as_ref(), &local) => {
                            return Ok(());
                        }
                        Event::Eof => {
                            return Err(missing_end(&format!(
                                "c:{}",
                                String::from_utf8_lossy(&local)
                            )));
                        }
                        _ => {}
                    }
                    inner.clear();
                }
            }
            Event::Empty(_) | Event::Eof => return Ok(()),
            _ => {}
        }
        buffer.clear();
    }
}

fn is_supported_series_plot(local: &[u8]) -> bool {
    matches!(
        local,
        b"areaChart"
            | b"area3DChart"
            | b"barChart"
            | b"bar3DChart"
            | b"doughnutChart"
            | b"lineChart"
            | b"line3DChart"
            | b"ofPieChart"
            | b"pieChart"
            | b"pie3DChart"
            | b"radarChart"
            | b"stockChart"
            | b"surfaceChart"
            | b"surface3DChart"
    )
}

impl Default for CT_Legend {
    fn default() -> Self {
        Self {
            root_name: "c:legend".into(),
            settings: LegendSettings {
                visible: true,
                ..LegendSettings::default()
            },
            raw_attributes: Vec::new(),
            raw_children: OrderedRawChildren::default(),
        }
    }
}

impl CT_Legend {
    fn from_xml_with_namespaces(xml: &[u8], namespaces: &NamespaceBindings) -> Result<Self> {
        let mut wrapper = BytesStart::new("scope");
        for (prefix, uri) in namespaces {
            let name = if prefix.is_empty() {
                "xmlns".to_owned()
            } else {
                format!("xmlns:{}", String::from_utf8_lossy(prefix))
            };
            wrapper.push_attribute((name.as_str(), uri.as_str()));
        }
        let mut writer = Writer::new(Vec::new());
        writer
            .write_event(Event::Start(wrapper))
            .map_err(OxmlError::from)?;
        writer.get_mut().extend_from_slice(xml);
        writer
            .write_event(Event::End(BytesEnd::new("scope")))
            .map_err(OxmlError::from)?;
        let wrapped = writer.into_inner();
        let nodes = chart_source_nodes(&wrapped)?;
        let legend = chart_child_node(&nodes, 0, b"legend")?;
        let settings = legend_settings_at(&wrapped, &nodes, legend)?;
        let (raw_attributes, raw_children) = parse_raw_shell(xml, b"legend", "c:legend")?;
        let mut reader = Reader::from_reader(xml);
        let root_name = loop {
            match reader.read_event().map_err(OxmlError::from)? {
                Event::Start(e) | Event::Empty(e) => break element_name(&e),
                Event::Eof => return Err(missing_end("legend")),
                _ => {}
            }
        };
        // Preserve a producer root alias only when a foreign c binding must
        // remain in scope for opaque descendants. Otherwise retain fixed c output.
        let root_name = if raw_attributes
            .iter()
            .any(|(name, uri)| name == "xmlns:c" && uri != C_NS)
        {
            root_name
        } else {
            "c:legend".to_owned()
        };
        Ok(Self {
            root_name,
            settings,
            raw_attributes,
            raw_children,
        })
    }

    fn write_xml<W: Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        write_raw_shell(
            writer,
            &self.root_name,
            &self.raw_attributes,
            &self.raw_children,
        )
    }

    pub fn raw_children(&self) -> &OrderedRawChildren {
        &self.raw_children
    }
}

fn parse_raw_shell(
    xml: &[u8],
    local: &[u8],
    tag: &str,
) -> Result<(XmlAttributes, OrderedRawChildren)> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) if matches_local_name(element.name().as_ref(), local) => {
                if local != b"legend" {
                    reject_conflicting_prefix(&element, b"c", C_NS)?;
                }
                let raw_attributes = capture_attributes(&element)?;
                let mut raw_children = OrderedRawChildren::default();
                let mut child_buffer = Vec::new();
                loop {
                    match reader
                        .read_event_into(&mut child_buffer)
                        .map_err(OxmlError::from)?
                    {
                        Event::Start(child) => {
                            raw_children.push(0, capture_element(&mut reader, &child)?)
                        }
                        Event::Empty(child) => raw_children.push(0, capture_empty_element(&child)?),
                        event @ (Event::Text(_)
                        | Event::CData(_)
                        | Event::Comment(_)
                        | Event::PI(_)
                        | Event::GeneralRef(_)) => {
                            raw_children.push(0, capture_event(event)?);
                        }
                        Event::End(end) if matches_local_name(end.name().as_ref(), local) => break,
                        Event::Eof => return Err(missing_end(tag)),
                        _ => {}
                    }
                    child_buffer.clear();
                }
                return Ok((raw_attributes, raw_children));
            }
            Event::Empty(element) if matches_local_name(element.name().as_ref(), local) => {
                if local != b"legend" {
                    reject_conflicting_prefix(&element, b"c", C_NS)?;
                }
                return Ok((capture_attributes(&element)?, OrderedRawChildren::default()));
            }
            Event::Start(element) | Event::Empty(element) => {
                return Err(ChartError::UnexpectedElement(element_name(&element)));
            }
            Event::Eof => return Err(ChartError::MissingElement(tag.to_owned())),
            _ => {}
        }
        buffer.clear();
    }
}

fn write_raw_shell<W: Write>(
    writer: &mut Writer<W>,
    tag: &str,
    raw_attributes: &XmlAttributes,
    raw_children: &OrderedRawChildren,
) -> Result<()> {
    let mut start = BytesStart::new(tag);
    push_attributes(&mut start, raw_attributes);
    if raw_children.is_empty() {
        writer
            .write_event(Event::Empty(start))
            .map_err(OxmlError::from)?;
        return Ok(());
    }
    writer
        .write_event(Event::Start(start))
        .map_err(OxmlError::from)?;
    emit_raw(writer, raw_children.at(0))?;
    writer
        .write_event(Event::End(BytesEnd::new(tag)))
        .map_err(OxmlError::from)?;
    Ok(())
}

/// The implemented behavior-bearing core of one `c:chart`.
#[derive(Clone, Debug, PartialEq)]
pub struct CT_Chart {
    pub title: Option<CT_Title>,
    pub auto_title_deleted: bool,
    pub plot_area: CT_PlotArea,
    pub legend: Option<CT_Legend>,
    pub plot_vis_only: bool,
    pub disp_blanks_as: DispBlanksAs,
    auto_title_deleted_markup: Option<ScalarMarkup>,
    plot_vis_only_markup: Option<ScalarMarkup>,
    disp_blanks_as_markup: Option<ScalarMarkup>,
    raw_attributes: Vec<(String, String)>,
    raw_children: OrderedRawChildren,
}

impl CT_Chart {
    fn from_xml_with_namespaces(xml: &[u8], inherited: &NamespaceBindings) -> Result<Self> {
        let mut reader = Reader::from_reader(xml);
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element) if matches_local_name(element.name().as_ref(), b"chart") => {
                    reject_conflicting_prefix(&element, b"c", C_NS)?;
                    return Self::from_element(&mut reader, &element, inherited);
                }
                Event::Empty(element) if matches_local_name(element.name().as_ref(), b"chart") => {
                    return Err(ChartError::MissingElement("c:plotArea".to_owned()));
                }
                Event::Start(element) | Event::Empty(element) => {
                    return Err(ChartError::UnexpectedElement(element_name(&element)));
                }
                Event::Eof => return Err(ChartError::MissingElement("c:chart".to_owned())),
                _ => {}
            }
            buffer.clear();
        }
    }

    fn from_element(
        reader: &mut Reader<&[u8]>,
        start: &BytesStart<'_>,
        inherited: &NamespaceBindings,
    ) -> Result<Self> {
        let namespaces = bindings_with_local(inherited, start)?;
        let raw_attributes = capture_attributes(start)?;
        let mut state = ChartParseState::default();
        let mut buffer = Vec::new();

        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element) => {
                    let name = if element_is_in_namespace(&element, C_NS, &namespaces)? {
                        local_name(element.name().as_ref()).to_vec()
                    } else {
                        Vec::new()
                    };
                    let raw = capture_element(reader, &element)?;
                    state.parse_child(&name, raw, &namespaces)?;
                }
                Event::Empty(element) => {
                    let name = if element_is_in_namespace(&element, C_NS, &namespaces)? {
                        local_name(element.name().as_ref()).to_vec()
                    } else {
                        Vec::new()
                    };
                    let raw = capture_empty_element(&element)?;
                    state.parse_child(&name, raw, &namespaces)?;
                }
                event @ (Event::Text(_)
                | Event::CData(_)
                | Event::Comment(_)
                | Event::PI(_)
                | Event::GeneralRef(_)) => {
                    state.capture_event(capture_event(event)?);
                }
                Event::End(element) if matches_local_name(element.name().as_ref(), b"chart") => {
                    break;
                }
                Event::Eof => return Err(missing_end("c:chart")),
                _ => {}
            }
            buffer.clear();
        }

        state.finish(raw_attributes)
    }

    fn write_xml<W: Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        let mut start = BytesStart::new("c:chart");
        push_attributes(&mut start, &self.raw_attributes);
        writer
            .write_event(Event::Start(start))
            .map_err(OxmlError::from)?;
        emit_raw(writer, self.raw_children.at(0))?;
        if let Some(title) = &self.title {
            title.write_xml(writer)?;
        }
        emit_raw(writer, self.raw_children.at(1))?;
        if self.auto_title_deleted_markup.is_some() || self.auto_title_deleted {
            write_scalar(
                writer,
                "c:autoTitleDeleted",
                if self.auto_title_deleted { "1" } else { "0" },
                self.auto_title_deleted_markup.as_ref(),
            )?;
        }
        for raw_boundary in 2..8 {
            emit_raw(writer, self.raw_children.at(raw_boundary))?;
        }
        self.plot_area.write_xml(writer)?;
        emit_raw(writer, self.raw_children.at(8))?;
        if let Some(legend) = &self.legend {
            legend.write_xml(writer)?;
        }
        emit_raw(writer, self.raw_children.at(9))?;
        if self.plot_vis_only_markup.is_some() || !self.plot_vis_only {
            write_scalar(
                writer,
                "c:plotVisOnly",
                if self.plot_vis_only { "1" } else { "0" },
                self.plot_vis_only_markup.as_ref(),
            )?;
        }
        emit_raw(writer, self.raw_children.at(10))?;
        if self.disp_blanks_as_markup.is_some() || self.disp_blanks_as != DispBlanksAs::Gap {
            write_scalar(
                writer,
                "c:dispBlanksAs",
                self.disp_blanks_as.as_str(),
                self.disp_blanks_as_markup.as_ref(),
            )?;
        }
        for raw_boundary in 11..=13 {
            emit_raw(writer, self.raw_children.at(raw_boundary))?;
        }
        writer
            .write_event(Event::End(BytesEnd::new("c:chart")))
            .map_err(OxmlError::from)?;
        Ok(())
    }

    pub fn raw_children(&self) -> &OrderedRawChildren {
        &self.raw_children
    }
}

#[derive(Default)]
struct ChartParseState {
    title: Option<CT_Title>,
    auto_title_deleted: Option<(bool, ScalarMarkup)>,
    plot_area: Option<CT_PlotArea>,
    legend: Option<CT_Legend>,
    plot_vis_only: Option<(bool, ScalarMarkup)>,
    disp_blanks_as: Option<(DispBlanksAs, ScalarMarkup)>,
    raw_children: OrderedRawChildren,
    boundary: usize,
}

impl ChartParseState {
    fn capture_event(&mut self, raw: Vec<u8>) {
        self.raw_children.push(self.boundary, raw);
    }

    fn parse_child(
        &mut self,
        name: &[u8],
        raw: Vec<u8>,
        namespaces: &NamespaceBindings,
    ) -> Result<()> {
        match name {
            b"title" => {
                set_once(&mut self.title, CT_Title::from_xml(&raw)?, "c:title")?;
                self.boundary = self.boundary.max(1);
            }
            b"autoTitleDeleted" => {
                set_once(
                    &mut self.auto_title_deleted,
                    parse_bool_value(&raw, "autoTitleDeleted")?,
                    "c:autoTitleDeleted",
                )?;
                self.boundary = self.boundary.max(2);
            }
            b"plotArea" => {
                set_once(
                    &mut self.plot_area,
                    CT_PlotArea::from_xml_with_namespaces(&raw, namespaces)?,
                    "c:plotArea",
                )?;
                self.boundary = self.boundary.max(8);
            }
            b"legend" => {
                set_once(
                    &mut self.legend,
                    CT_Legend::from_xml_with_namespaces(&raw, namespaces)?,
                    "c:legend",
                )?;
                self.boundary = self.boundary.max(9);
            }
            b"plotVisOnly" => {
                set_once(
                    &mut self.plot_vis_only,
                    parse_bool_value(&raw, "plotVisOnly")?,
                    "c:plotVisOnly",
                )?;
                self.boundary = self.boundary.max(10);
            }
            b"dispBlanksAs" => {
                set_once(
                    &mut self.disp_blanks_as,
                    parse_enum_value(&raw, "dispBlanksAs")?,
                    "c:dispBlanksAs",
                )?;
                self.boundary = self.boundary.max(11);
            }
            _ => {
                self.raw_children
                    .push(chart_raw_slot(name).unwrap_or(self.boundary), raw);
                self.boundary = self.boundary.max(chart_raw_boundary_after(name));
            }
        }
        Ok(())
    }

    fn finish(self, raw_attributes: XmlAttributes) -> Result<CT_Chart> {
        let (auto_title_deleted, auto_title_deleted_markup) = self
            .auto_title_deleted
            .map(|(value, markup)| (value, Some(markup)))
            .unwrap_or((false, None));
        let (plot_vis_only, plot_vis_only_markup) = self
            .plot_vis_only
            .map(|(value, markup)| (value, Some(markup)))
            .unwrap_or((true, None));
        let (disp_blanks_as, disp_blanks_as_markup) = self
            .disp_blanks_as
            .map(|(value, markup)| (value, Some(markup)))
            .unwrap_or((DispBlanksAs::Gap, None));
        Ok(CT_Chart {
            title: self.title,
            auto_title_deleted,
            plot_area: self
                .plot_area
                .ok_or_else(|| ChartError::MissingElement("c:plotArea".to_owned()))?,
            legend: self.legend,
            plot_vis_only,
            disp_blanks_as,
            auto_title_deleted_markup,
            plot_vis_only_markup,
            disp_blanks_as_markup,
            raw_attributes,
            raw_children: raw_children_in_schema_order(&self.raw_children, 13),
        })
    }
}

fn chart_raw_slot(name: &[u8]) -> Option<usize> {
    match name {
        b"pivotFmts" => Some(2),
        b"view3D" => Some(3),
        b"floor" => Some(4),
        b"sideWall" => Some(5),
        b"backWall" => Some(6),
        // Both unmodelled tail children share a slot so producer sibling order
        // remains byte-stable until a later story types either element.
        b"showDLblsOverMax" | b"extLst" => Some(11),
        _ => None,
    }
}

fn chart_raw_boundary_after(name: &[u8]) -> usize {
    match name {
        b"title" => 1,
        b"autoTitleDeleted" => 2,
        b"pivotFmts" => 3,
        b"view3D" => 4,
        b"floor" => 5,
        b"sideWall" => 6,
        b"backWall" => 7,
        b"plotArea" => 8,
        b"legend" => 9,
        b"plotVisOnly" => 10,
        b"dispBlanksAs" => 11,
        b"showDLblsOverMax" | b"extLst" => 11,
        _ => 0,
    }
}

/// One complete `c:chartSpace` part with its core typed.
#[derive(Clone, Debug, PartialEq)]
pub struct CT_ChartSpace {
    pub chart: CT_Chart,
    pub sp_pr: Option<CT_ShapeProperties>,
    pub tx_pr: Option<CT_TextBody>,
    raw_attributes: Vec<(String, String)>,
    namespace_declarations: Vec<(String, String)>,
    raw_children: OrderedRawChildren,
}

impl CT_ChartSpace {
    /// Parses one complete ChartML chart-space part.
    pub fn from_xml(xml: &[u8]) -> Result<Self> {
        let mut reader = Reader::from_reader(xml);
        let mut buffer = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element)
                    if matches_local_name(element.name().as_ref(), b"chartSpace") =>
                {
                    validate_chart_space_namespace(&element)?;
                    return Self::from_element(&mut reader, &element);
                }
                Event::Empty(element)
                    if matches_local_name(element.name().as_ref(), b"chartSpace") =>
                {
                    validate_chart_space_namespace(&element)?;
                    return Err(ChartError::MissingElement("c:chart".to_owned()));
                }
                Event::Start(element) | Event::Empty(element) => {
                    return Err(ChartError::UnexpectedElement(element_name(&element)));
                }
                Event::Eof => return Err(ChartError::MissingElement("c:chartSpace".to_owned())),
                _ => {}
            }
            buffer.clear();
        }
    }

    fn from_element(reader: &mut Reader<&[u8]>, start: &BytesStart<'_>) -> Result<Self> {
        let namespaces = namespace_bindings(start)?;
        let (raw_attributes, namespace_declarations) = capture_root_attributes(start)?;
        let mut state = ChartSpaceParseState::default();
        let mut buffer = Vec::new();

        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element) => {
                    let name = if element_is_in_namespace(&element, C_NS, &namespaces)? {
                        local_name(element.name().as_ref()).to_vec()
                    } else {
                        Vec::new()
                    };
                    let raw = capture_element(reader, &element)?;
                    state.parse_child(&name, raw, &namespaces)?;
                }
                Event::Empty(element) => {
                    let name = if element_is_in_namespace(&element, C_NS, &namespaces)? {
                        local_name(element.name().as_ref()).to_vec()
                    } else {
                        Vec::new()
                    };
                    let raw = capture_empty_element(&element)?;
                    state.parse_child(&name, raw, &namespaces)?;
                }
                event @ (Event::Text(_)
                | Event::CData(_)
                | Event::Comment(_)
                | Event::PI(_)
                | Event::GeneralRef(_)) => {
                    state.capture_event(capture_event(event)?);
                }
                Event::End(element)
                    if matches_local_name(element.name().as_ref(), b"chartSpace") =>
                {
                    break;
                }
                Event::Eof => return Err(missing_end("c:chartSpace")),
                _ => {}
            }
            buffer.clear();
        }

        state.finish(raw_attributes, namespace_declarations)
    }

    /// Writes one chart-space part using fixed OOXML prefixes and schema order.
    pub fn to_xml(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new(Vec::new());
        let mut start = BytesStart::new("c:chartSpace");
        start.push_attribute(("xmlns:c", C_NS));
        start.push_attribute(("xmlns:a", A_NS));
        start.push_attribute(("xmlns:r", R_NS));
        push_attributes(&mut start, &self.namespace_declarations);
        push_attributes(&mut start, &self.raw_attributes);
        writer
            .write_event(Event::Start(start))
            .map_err(OxmlError::from)?;
        for raw_boundary in 0..8 {
            emit_raw(&mut writer, self.raw_children.at(raw_boundary))?;
        }
        self.chart.write_xml(&mut writer)?;
        emit_raw(&mut writer, self.raw_children.at(8))?;
        if let Some(properties) = &self.sp_pr {
            properties.write_xml_as(&mut writer, "c:spPr")?;
        }
        emit_raw(&mut writer, self.raw_children.at(9))?;
        if let Some(text) = &self.tx_pr {
            text.write_xml_as(&mut writer, "c:txPr")?;
        }
        for raw_boundary in 10..=14 {
            emit_raw(&mut writer, self.raw_children.at(raw_boundary))?;
        }
        writer
            .write_event(Event::End(BytesEnd::new("c:chartSpace")))
            .map_err(OxmlError::from)?;
        Ok(writer.into_inner())
    }

    pub fn raw_children(&self) -> &OrderedRawChildren {
        &self.raw_children
    }
}

#[derive(Default)]
struct ChartSpaceParseState {
    chart: Option<CT_Chart>,
    sp_pr: Option<CT_ShapeProperties>,
    tx_pr: Option<CT_TextBody>,
    raw_children: OrderedRawChildren,
    boundary: usize,
}

impl ChartSpaceParseState {
    fn capture_event(&mut self, raw: Vec<u8>) {
        self.raw_children.push(self.boundary, raw);
    }

    fn parse_child(
        &mut self,
        name: &[u8],
        raw: Vec<u8>,
        namespaces: &NamespaceBindings,
    ) -> Result<()> {
        match name {
            b"chart" => {
                set_once(
                    &mut self.chart,
                    CT_Chart::from_xml_with_namespaces(&raw, namespaces)?,
                    "c:chart",
                )?;
                self.boundary = self.boundary.max(8);
            }
            b"spPr" => {
                parse_chart_shape_properties(&raw, namespaces, &mut self.sp_pr)?;
                self.boundary = self.boundary.max(9);
            }
            b"txPr" => {
                reject_conflicting_prefix_in_xml(&raw, b"a", A_NS)?;
                let inherited = namespaces
                    .iter()
                    .map(|(prefix, uri)| {
                        Ok((
                            std::str::from_utf8(prefix)
                                .map_err(OxmlError::from)?
                                .to_owned(),
                            uri.clone(),
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let text = CT_TextBody::from_xml_as_with_namespaces(&raw, b"txPr", &inherited)?;
                let mut writer = Writer::new(Vec::new());
                text.write_xml_as(&mut writer, "c:txPr")?;
                reject_rewritten_foreign_elements(&raw, &writer.into_inner(), namespaces, b"txPr")?;
                set_once(&mut self.tx_pr, text, "c:txPr")?;
                self.boundary = self.boundary.max(10);
            }
            _ => {
                self.raw_children
                    .push(chart_space_raw_slot(name).unwrap_or(self.boundary), raw);
                self.boundary = self.boundary.max(chart_space_raw_boundary_after(name));
            }
        }
        Ok(())
    }

    fn finish(
        self,
        raw_attributes: XmlAttributes,
        namespace_declarations: XmlAttributes,
    ) -> Result<CT_ChartSpace> {
        Ok(CT_ChartSpace {
            chart: self
                .chart
                .ok_or_else(|| ChartError::MissingElement("c:chart".to_owned()))?,
            sp_pr: self.sp_pr,
            tx_pr: self.tx_pr,
            raw_attributes,
            namespace_declarations,
            raw_children: raw_children_in_schema_order(&self.raw_children, 14),
        })
    }
}

fn raw_children_in_schema_order(
    raw_children: &OrderedRawChildren,
    final_boundary: usize,
) -> OrderedRawChildren {
    let mut ordered = OrderedRawChildren::default();
    for boundary in 0..=final_boundary {
        for raw in raw_children.at(boundary) {
            ordered.push(boundary, raw.to_vec());
        }
    }
    ordered
}

fn chart_space_raw_slot(name: &[u8]) -> Option<usize> {
    match name {
        b"date1904" => Some(0),
        b"lang" => Some(1),
        b"roundedCorners" => Some(2),
        b"style" => Some(3),
        b"clrMapOvr" => Some(4),
        b"pivotSource" => Some(5),
        b"protection" => Some(6),
        b"externalData" => Some(10),
        b"printSettings" => Some(11),
        b"userShapes" => Some(12),
        b"extLst" => Some(13),
        _ => None,
    }
}

fn chart_space_raw_boundary_after(name: &[u8]) -> usize {
    match name {
        b"date1904" => 1,
        b"lang" => 2,
        b"roundedCorners" => 3,
        b"style" => 4,
        b"clrMapOvr" => 5,
        b"pivotSource" => 6,
        b"protection" => 7,
        b"chart" => 8,
        b"spPr" => 9,
        b"txPr" => 10,
        b"externalData" => 11,
        b"printSettings" => 12,
        b"userShapes" => 13,
        b"extLst" => 14,
        _ => 0,
    }
}

fn parse_bool_value(xml: &[u8], local: &str) -> Result<(bool, ScalarMarkup)> {
    let (value, markup) = scalar_value(xml, local)?;
    let value = match value.as_deref().unwrap_or("1") {
        "1" | "true" => true,
        "0" | "false" => false,
        invalid => return Err(invalid_attribute(local, "val", invalid.to_owned())),
    };
    Ok((value, markup))
}

fn parse_enum_value(xml: &[u8], local: &str) -> Result<(DispBlanksAs, ScalarMarkup)> {
    let (value, markup) = scalar_value(xml, local)?;
    let value = value.ok_or_else(|| invalid_attribute(local, "val", "<missing>".to_owned()))?;
    let parsed =
        DispBlanksAs::parse(&value).ok_or_else(|| invalid_attribute(local, "val", value))?;
    Ok((parsed, markup))
}

fn scalar_value(xml: &[u8], expected: &str) -> Result<(Option<String>, ScalarMarkup)> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Empty(element)
                if matches_local_name(element.name().as_ref(), expected.as_bytes()) =>
            {
                reject_conflicting_prefix(&element, b"c", C_NS)?;
                return Ok((
                    attribute_value(&element, b"val")?,
                    ScalarMarkup {
                        raw_attributes: capture_attributes_except(&element, b"val")?,
                        raw_content: Vec::new(),
                    },
                ));
            }
            Event::Start(element)
                if matches_local_name(element.name().as_ref(), expected.as_bytes()) =>
            {
                reject_conflicting_prefix(&element, b"c", C_NS)?;
                let value = attribute_value(&element, b"val")?;
                let raw_attributes = capture_attributes_except(&element, b"val")?;
                let mut raw_content = Vec::new();
                let mut inner = Vec::new();
                loop {
                    match reader
                        .read_event_into(&mut inner)
                        .map_err(OxmlError::from)?
                    {
                        Event::End(end)
                            if matches_local_name(end.name().as_ref(), expected.as_bytes()) =>
                        {
                            return Ok((
                                value,
                                ScalarMarkup {
                                    raw_attributes,
                                    raw_content,
                                },
                            ));
                        }
                        event @ (Event::Text(_)
                        | Event::CData(_)
                        | Event::Comment(_)
                        | Event::PI(_)
                        | Event::GeneralRef(_)) => raw_content.push(capture_event(event)?),
                        Event::Eof => return Err(missing_end(expected)),
                        _ => return Err(ChartError::UnexpectedElement(expected.to_owned())),
                    }
                    inner.clear();
                }
            }
            Event::Start(element) | Event::Empty(element) => {
                return Err(ChartError::UnexpectedElement(element_name(&element)));
            }
            Event::Eof => return Err(ChartError::MissingElement(expected.to_owned())),
            _ => {}
        }
        buffer.clear();
    }
}

fn set_once<T>(slot: &mut Option<T>, value: T, name: &str) -> Result<()> {
    if slot.is_some() {
        return Err(ChartError::DuplicateElement(name.to_owned()));
    }
    *slot = Some(value);
    Ok(())
}

fn mark_once(seen: &mut bool, name: &str) -> Result<()> {
    if *seen {
        return Err(ChartError::DuplicateElement(name.to_owned()));
    }
    *seen = true;
    Ok(())
}

fn validate_chart_space_namespace(element: &BytesStart<'_>) -> Result<()> {
    let name = element.name();
    let qualified = name.as_ref();
    let declaration = match qualified.iter().position(|byte| *byte == b':') {
        Some(position) => {
            let mut declaration = b"xmlns:".to_vec();
            declaration.extend_from_slice(&qualified[..position]);
            declaration
        }
        None => b"xmlns".to_vec(),
    };
    let namespace = attribute_value(element, &declaration)?
        .ok_or_else(|| invalid_attribute("chartSpace", "namespace", "<missing>".to_owned()))?;
    if namespace != C_NS {
        return Err(invalid_attribute("chartSpace", "namespace", namespace));
    }
    reject_conflicting_prefix(element, b"c", C_NS)?;
    reject_conflicting_prefix(element, b"a", A_NS)?;
    reject_conflicting_prefix(element, b"r", R_NS)
}

fn reject_conflicting_prefix_in_xml(xml: &[u8], prefix: &[u8], namespace: &str) -> Result<()> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) | Event::Empty(element) => {
                return reject_conflicting_prefix(&element, prefix, namespace);
            }
            Event::Eof => return Err(ChartError::MissingElement("XML root".to_owned())),
            _ => {}
        }
        buffer.clear();
    }
}

fn reject_rewritten_foreign_elements(
    original: &[u8],
    written: &[u8],
    inherited: &NamespaceBindings,
    root_local: &[u8],
) -> Result<()> {
    let original = foreign_element_regions(original, inherited, root_local)?;
    let mut written_namespaces = inherited.clone();
    written_namespaces.retain(|(prefix, _)| prefix != b"a");
    written_namespaces.push((b"a".to_vec(), A_NS.to_owned()));
    let written = foreign_element_regions(written, &written_namespaces, root_local)?;
    if original != written {
        return Err(invalid_attribute(
            &String::from_utf8_lossy(root_local),
            "namespace",
            "foreign DrawingML content would be converted to typed content".to_owned(),
        ));
    }
    Ok(())
}

fn foreign_element_regions(
    xml: &[u8],
    inherited: &NamespaceBindings,
    root_local: &[u8],
) -> Result<Vec<Vec<u8>>> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    let root_namespaces = loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) if matches_local_name(element.name().as_ref(), root_local) => {
                break bindings_with_local(inherited, &element)?;
            }
            Event::Empty(element) if matches_local_name(element.name().as_ref(), root_local) => {
                return Ok(Vec::new());
            }
            Event::Start(element) | Event::Empty(element) => {
                return Err(ChartError::UnexpectedElement(element_name(&element)));
            }
            Event::Eof => {
                return Err(ChartError::MissingElement(
                    String::from_utf8_lossy(root_local).into_owned(),
                ));
            }
            _ => {}
        }
        buffer.clear();
    };

    let mut namespace_stack = vec![root_namespaces];
    let mut foreign = Vec::new();
    let mut child_buffer = Vec::new();
    loop {
        match reader
            .read_event_into(&mut child_buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) => {
                let namespaces = namespace_stack.last().expect("root namespace binding");
                if element_is_in_namespace(&element, A_NS, namespaces)? {
                    namespace_stack.push(bindings_with_local(namespaces, &element)?);
                } else {
                    foreign.push(capture_element(&mut reader, &element)?);
                }
            }
            Event::Empty(element) => {
                let namespaces = namespace_stack.last().expect("root namespace binding");
                if !element_is_in_namespace(&element, A_NS, namespaces)? {
                    foreign.push(capture_empty_element(&element)?);
                }
            }
            Event::End(element) if matches_local_name(element.name().as_ref(), root_local) => {
                return Ok(foreign);
            }
            Event::End(_) => {
                namespace_stack.pop();
            }
            Event::Eof => {
                return Err(missing_end(&String::from_utf8_lossy(root_local)));
            }
            _ => {}
        }
        child_buffer.clear();
    }
}

fn reject_conflicting_prefix(
    element: &BytesStart<'_>,
    prefix: &[u8],
    namespace: &str,
) -> Result<()> {
    let mut key = b"xmlns:".to_vec();
    key.extend_from_slice(prefix);
    if let Some(value) = attribute_value(element, &key)?
        && value != namespace
    {
        return Err(invalid_attribute(
            &element_name(element),
            &String::from_utf8_lossy(&key),
            value,
        ));
    }
    Ok(())
}

fn namespace_bindings(element: &BytesStart<'_>) -> Result<NamespaceBindings> {
    let mut bindings = Vec::new();
    for attribute in element.attributes() {
        let attribute = attribute.map_err(OxmlError::from)?;
        let key = attribute.key.as_ref();
        let prefix = if key == b"xmlns" {
            Vec::new()
        } else if let Some(prefix) = key.strip_prefix(b"xmlns:") {
            prefix.to_vec()
        } else {
            continue;
        };
        let namespace = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
            .map_err(OxmlError::from)?
            .into_owned();
        bindings.push((prefix, namespace));
    }
    Ok(bindings)
}

fn bindings_with_local(
    inherited: &NamespaceBindings,
    element: &BytesStart<'_>,
) -> Result<NamespaceBindings> {
    let mut bindings = inherited.clone();
    for (prefix, namespace) in namespace_bindings(element)? {
        upsert_namespace_binding(&mut bindings, prefix, namespace);
    }
    Ok(bindings)
}

fn upsert_namespace_binding(bindings: &mut NamespaceBindings, prefix: Vec<u8>, namespace: String) {
    if let Some((_, current)) = bindings.iter_mut().find(|(current, _)| current == &prefix) {
        *current = namespace;
    } else {
        bindings.push((prefix, namespace));
    }
}

fn element_is_in_namespace(
    element: &BytesStart<'_>,
    expected: &str,
    inherited: &NamespaceBindings,
) -> Result<bool> {
    let name = element.name();
    let qualified = name.as_ref();
    let prefix = qualified
        .iter()
        .position(|byte| *byte == b':')
        .map(|position| &qualified[..position])
        .unwrap_or_default();
    let local = namespace_bindings(element)?;
    let namespace = local
        .iter()
        .rev()
        .chain(inherited.iter().rev())
        .find(|(candidate, _)| candidate.as_slice() == prefix)
        .map(|(_, namespace)| namespace.as_str());
    Ok(namespace == Some(expected))
}

fn attribute_value(element: &BytesStart<'_>, name: &[u8]) -> Result<Option<String>> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(OxmlError::from)?;
        if attribute.key.as_ref() == name {
            return Ok(Some(
                attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                    .map_err(OxmlError::from)?
                    .into_owned(),
            ));
        }
    }
    Ok(None)
}

fn capture_root_attributes(start: &BytesStart<'_>) -> Result<RootAttributes> {
    let mut attributes = Vec::new();
    let mut namespaces = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(OxmlError::from)?;
        let name = std::str::from_utf8(attribute.key.as_ref())
            .map_err(OxmlError::from)?
            .to_owned();
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
            .map_err(OxmlError::from)?
            .into_owned();
        if name == "xmlns:c" || name == "xmlns:a" || name == "xmlns:r" {
            continue;
        }
        if name == "xmlns" || name.starts_with("xmlns:") {
            namespaces.push((name, value));
        } else {
            attributes.push((name, value));
        }
    }
    Ok((attributes, namespaces))
}

fn capture_attributes(start: &BytesStart<'_>) -> Result<XmlAttributes> {
    let mut attributes = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(OxmlError::from)?;
        attributes.push((
            std::str::from_utf8(attribute.key.as_ref())
                .map_err(OxmlError::from)?
                .to_owned(),
            attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
                .map_err(OxmlError::from)?
                .into_owned(),
        ));
    }
    Ok(attributes)
}

fn capture_attributes_except(start: &BytesStart<'_>, excluded: &[u8]) -> Result<XmlAttributes> {
    let mut attributes = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(OxmlError::from)?;
        if attribute.key.as_ref() == excluded {
            continue;
        }
        attributes.push((
            std::str::from_utf8(attribute.key.as_ref())
                .map_err(OxmlError::from)?
                .to_owned(),
            attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
                .map_err(OxmlError::from)?
                .into_owned(),
        ));
    }
    Ok(attributes)
}

fn capture_attributes_excluding(
    start: &BytesStart<'_>,
    excluded: &[&[u8]],
) -> Result<XmlAttributes> {
    let mut attributes = Vec::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(OxmlError::from)?;
        if excluded.contains(&attribute.key.as_ref()) {
            continue;
        }
        attributes.push((
            std::str::from_utf8(attribute.key.as_ref())
                .map_err(OxmlError::from)?
                .to_owned(),
            attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, start.decoder())
                .map_err(OxmlError::from)?
                .into_owned(),
        ));
    }
    Ok(attributes)
}

fn write_scalar<W: Write>(
    writer: &mut Writer<W>,
    name: &str,
    value: &str,
    markup: Option<&ScalarMarkup>,
) -> Result<()> {
    let mut start = BytesStart::new(name);
    start.push_attribute(("val", value));
    if let Some(markup) = markup {
        push_attributes(&mut start, &markup.raw_attributes);
    }
    let has_content = markup.is_some_and(|markup| !markup.raw_content.is_empty());
    if !has_content {
        writer
            .write_event(Event::Empty(start))
            .map_err(OxmlError::from)?;
        return Ok(());
    }
    writer
        .write_event(Event::Start(start))
        .map_err(OxmlError::from)?;
    if let Some(markup) = markup {
        for raw in &markup.raw_content {
            writer.get_mut().write_all(raw).map_err(OxmlError::from)?;
        }
    }
    writer
        .write_event(Event::End(BytesEnd::new(name)))
        .map_err(OxmlError::from)?;
    Ok(())
}

fn invalid_attribute(element: &str, attribute: &str, value: String) -> ChartError {
    ChartError::InvalidAttribute {
        element: element.to_owned(),
        attribute: attribute.to_owned(),
        value,
    }
}

fn element_name(element: &BytesStart<'_>) -> String {
    String::from_utf8_lossy(element.name().as_ref()).into_owned()
}

fn missing_end(element: &str) -> ChartError {
    ChartError::Xml(OxmlError::MissingElement(format!("closing {element}")))
}
