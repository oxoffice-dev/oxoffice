#![allow(non_camel_case_types)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Write;

use oxoffice_core::OxmlError;
use oxoffice_core::raw_xml::{
    capture_element, capture_empty, capture_event, emit_raw, push_attributes,
};
use oxoffice_core::xml::{escape_text, local_name, matches_local_name};
use oxoffice_core::xml_reader::Reader;
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
use quick_xml::{Writer, XmlVersion};

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

mod axis;
mod chart;
mod data_labels;
mod legend;
mod number_format;
mod plot;
mod plot_area;
mod plot_parse;
mod plot_write;
mod references;
mod scalars;
mod series;
mod title;
mod xml;

pub use axis::{
    Axis, AxisId, AxisKind, AxisPosition, ChartLines, Orientation, Scaling, TickLabelPosition,
    TickMark,
};
use axis::{AxisIdMarkup, default_axis_id_markup, parse_axis_id_scalar, write_axis_id};
pub use chart::{CT_Chart, CT_ChartSpace, DispBlanksAs};
pub use data_labels::DataLabelPosition;
pub use legend::{LegendPosition, LegendSettings};
pub use number_format::NumberFormat;
pub use oxoffice_numfmt::{DateSystem, FormatOptions};
pub use plot::{BarDirection, BarGrouping, Grouping, Plot, RadarStyle, ScatterStyle};
pub use plot_area::CT_PlotArea;
use plot_parse::parse_plot;
use plot_write::write_plot;
use references::{
    cache_layout, parse_reference, parse_text_element, parse_wrapper, validate_format_code,
    validate_formula, validate_numeric_values, validate_point_count, validate_xml_text,
    write_numeric_cache, write_string_cache, write_text, write_wrapper_end, write_wrapper_start,
};
use scalars::{
    bool_lexical, element_name, invalid_attribute, mark_once, missing_end, optional_markup,
    optional_scalar_markup_eq, parse_bool_lexical, parse_bool_value, parse_enum_value,
    parse_u32_scalar, required_u32_attribute, scalar_value, set_once, write_scalar,
};
pub use series::AxisData;
pub use title::CT_Title;
use xml::{
    attribute_value, bindings_with_local, capture_attributes, capture_attributes_except,
    capture_attributes_excluding, capture_fixed_attributes, capture_fixed_root_attributes,
    capture_root_attributes, chart_bindings, chart_child_local, chart_namespace_defaults,
    chart_root_local, chart_root_prefix, element_is_in_namespace, namespace_bindings,
    parse_chart_shape_properties, parse_raw_shell, raw_children_in_schema_order,
    reject_conflicting_prefix, reject_conflicting_prefix_in_xml, reject_rewritten_foreign_elements,
    require_fixed_namespace, root_chart_bindings, standalone_namespace_declarations,
    typed_rewrite_bindings, upsert_namespace_binding, validate_chart_space_namespace,
    write_raw_shell,
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

/// A formula-backed numeric cache whose metadata and points are derived.
#[derive(Clone, Debug, PartialEq)]
pub struct NumericData {
    pub formula: String,
    pub format_code: String,
    pub values: Vec<f64>,
    markup: ReferenceMarkup,
}

#[derive(Clone, Debug, PartialEq)]
struct DataPoint {
    index: u32,
    sp_pr: CT_ShapeProperties,
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

struct ParsedReference {
    formula: String,
    format_code: Option<String>,
    values: Vec<String>,
    markup: ReferenceMarkup,
}

struct ParsedWrapper {
    choice: Option<(Vec<u8>, Vec<u8>)>,
    markup: WrapperMarkup,
    namespaces: NamespaceBindings,
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

/// A legend retaining opaque children and their effective projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CT_Legend {
    root_name: String,
    settings: LegendSettings,
    raw_attributes: Vec<(String, String)>,
    raw_children: OrderedRawChildren,
}
