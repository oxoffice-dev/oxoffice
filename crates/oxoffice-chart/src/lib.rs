#![allow(non_camel_case_types)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Write;

use oxoffice_core::OxmlError;
use oxoffice_core::raw_xml::{capture_element, capture_empty_element};
use oxoffice_core::xml::{local_name, matches_local_name};
use oxoffice_core::xml_text::{decode_plain, resolve_entity};
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

/// Project the direct chart title from complete current part bytes without rewriting it.
pub fn chart_plain_title(chart_xml: &[u8]) -> Result<Option<String>> {
    CT_ChartSpace::from_xml(chart_xml)?;
    use quick_xml::name::ResolveResult;
    let mut reader = quick_xml::reader::NsReader::from_reader(chart_xml);
    let mut path: Vec<(u8, Vec<u8>)> = Vec::new();
    let mut rich = String::new();
    let mut cached = String::new();
    let mut has_rich = false;
    let title_prefix = |path: &[(u8, Vec<u8>)]| {
        path.len() >= 4
            && path[..4]
                .iter()
                .map(|(ns, name)| (*ns, name.as_slice()))
                .eq([
                    (1, b"chartSpace".as_slice()),
                    (1, b"chart"),
                    (1, b"title"),
                    (1, b"tx"),
                ])
    };
    loop {
        let (ns, event) = reader.read_resolved_event().map_err(OxmlError::from)?;
        let ns = match ns {
            ResolveResult::Bound(uri) if uri.as_ref() == C_NS.as_bytes() => 1,
            ResolveResult::Bound(uri) if uri.as_ref() == A_NS.as_bytes() => 2,
            _ => 0,
        };
        match event {
            event @ (Event::Start(_) | Event::Empty(_)) => {
                let empty = matches!(&event, Event::Empty(_));
                let start = match event {
                    Event::Start(start) | Event::Empty(start) => start,
                    _ => unreachable!(),
                };
                path.push((ns, start.local_name().as_ref().to_vec()));
                if path.len() == 5 && title_prefix(&path) && path[4] == (1, b"rich".to_vec()) {
                    has_rich = true;
                }
                if empty {
                    path.pop();
                }
            }
            Event::End(_) => {
                path.pop();
            }
            Event::Eof => break,
            event => {
                let rich_path = path.len() == 8
                    && title_prefix(&path)
                    && path[4] == (1, b"rich".to_vec())
                    && path[5] == (2, b"p".to_vec())
                    && path[6].0 == 2
                    && matches!(path[6].1.as_slice(), b"r" | b"fld")
                    && path[7] == (2, b"t".to_vec());
                let cache_path = path.len() == 8
                    && title_prefix(&path)
                    && path[4..]
                        .iter()
                        .map(|(ns, name)| (*ns, name.as_slice()))
                        .eq([
                            (1, b"strRef".as_slice()),
                            (1, b"strCache"),
                            (1, b"pt"),
                            (1, b"v"),
                        ]);
                let target = if rich_path {
                    &mut rich
                } else if cache_path {
                    &mut cached
                } else {
                    continue;
                };
                match event {
                    Event::Text(text) => target.push_str(&decode_plain(&text)),
                    Event::CData(text) => target.push_str(&text.decode().map_err(|error| {
                        ChartError::InvalidValue {
                            element: "c:title".into(),
                            value: error.to_string(),
                        }
                    })?),
                    Event::GeneralRef(reference) => target.push_str(&resolve_entity(&reference)),
                    _ => {}
                }
            }
        }
    }
    let title = if has_rich { rich } else { cached };
    Ok((!title.is_empty()).then_some(title))
}

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

/// Authors one ChartML part and its editable workbook from one validated source.
pub fn authored_chart_parts(
    kind: ChartKind,
    data: &ChartData,
    workbook_relationship_id: &str,
) -> Result<(Vec<u8>, Vec<u8>)> {
    validate_chart_data(kind, data)?;
    let category_column = if kind == ChartKind::Scatter {
        Column::Number {
            header: "Category".to_owned(),
            values: data
                .categories
                .iter()
                .map(|value| value.parse::<f64>().expect("validated scatter category"))
                .collect(),
            number_format: None,
        }
    } else {
        Column::Text {
            header: "Category".to_owned(),
            values: data.categories.clone(),
        }
    };
    let mut columns = Vec::with_capacity(data.series.len() + 1);
    columns.push(category_column);
    columns.extend(data.series.iter().map(|(name, values)| Column::Number {
        header: name.clone(),
        values: values.clone(),
        number_format: data.number_format.clone(),
    }));
    let workbook = Workbook::new("Sheet1", columns).map_err(invalid_authoring_value)?;
    let category_formula = workbook
        .formula_range(0)
        .expect("validated categories produce a formula range");
    let number_format = data.number_format.as_deref().unwrap_or("General");
    let mut chart_series = Vec::with_capacity(data.series.len());
    for (index, (name, values)) in data.series.iter().enumerate() {
        let values_formula = workbook
            .formula_range(index + 1)
            .expect("validated series produces a formula range");
        let values = NumericData::new(values_formula, number_format.to_owned(), values.clone())
            .map_err(invalid_authoring_value)?;
        let mut series = Series::new(index as u32, index as u32, values);
        series.name = Some(
            StringRef::new(
                format!("Sheet1!${}$1", spreadsheet_column_name(index + 1)),
                vec![name.clone()],
            )
            .map_err(invalid_authoring_value)?,
        );
        series.categories = Some(if kind == ChartKind::Scatter {
            AxisData::Numeric(
                NumericData::new(
                    category_formula.clone(),
                    "General".to_owned(),
                    data.categories
                        .iter()
                        .map(|value| value.parse::<f64>().expect("validated scatter category"))
                        .collect(),
                )
                .map_err(invalid_authoring_value)?,
            )
        } else {
            AxisData::String(
                StringRef::new(category_formula.clone(), data.categories.clone())
                    .map_err(invalid_authoring_value)?,
            )
        });
        chart_series.push(series);
    }
    apply_palette(kind, data, &mut chart_series);

    let category_axis_id = AxisId::new(48_650_112).map_err(invalid_authoring_value)?;
    let value_axis_id = AxisId::new(48_672_768).map_err(invalid_authoring_value)?;
    let axis_ids = [category_axis_id, value_axis_id];
    let (plot, axes) = match kind {
        ChartKind::Bar => (
            Plot::bar(
                BarDirection::Column,
                BarGrouping::Clustered,
                chart_series,
                axis_ids,
            ),
            category_value_axes(category_axis_id, value_axis_id, data)?,
        ),
        ChartKind::Line => (
            Plot::line(Grouping::Standard, chart_series, axis_ids),
            category_value_axes(category_axis_id, value_axis_id, data)?,
        ),
        ChartKind::Pie => (Plot::pie(chart_series), Vec::new()),
        ChartKind::Doughnut => (Plot::doughnut(chart_series), Vec::new()),
        ChartKind::Area => (
            Plot::area(Grouping::Standard, chart_series, axis_ids),
            category_value_axes(category_axis_id, value_axis_id, data)?,
        ),
        ChartKind::Scatter => (
            Plot::scatter(ScatterStyle::LineMarker, chart_series, axis_ids),
            authored_axes(
                [
                    Axis::new(
                        AxisKind::Value,
                        category_axis_id,
                        AxisPosition::Bottom,
                        value_axis_id,
                    ),
                    Axis::new(
                        AxisKind::Value,
                        value_axis_id,
                        AxisPosition::Left,
                        category_axis_id,
                    ),
                ],
                data,
            )?,
        ),
        ChartKind::Radar => (
            Plot::radar(RadarStyle::Standard, chart_series, axis_ids),
            category_value_axes(category_axis_id, value_axis_id, data)?,
        ),
    };
    let mut plot = plot.map_err(invalid_authoring_value)?;
    if let Plot::Pie { data_labels, .. } | Plot::Doughnut { data_labels, .. } = &mut plot {
        *data_labels = Some(CT_DLbls {
            show_percent: true,
            ..CT_DLbls::default()
        });
    }
    let plot_area = CT_PlotArea::new(vec![plot], axes).map_err(invalid_authoring_value)?;
    let chart_shell = format!(
        r#"<c:chartSpace xmlns:c="{C_NS}" xmlns:a="{A_NS}" xmlns:r="{R_NS}"><c:chart><c:plotArea/></c:chart><c:externalData r:id="{workbook_relationship_id}"><c:autoUpdate val="0"/></c:externalData></c:chartSpace>"#,
    );
    let mut chart_space = CT_ChartSpace::from_xml(chart_shell.as_bytes())?;
    chart_space.chart.auto_title_deleted = true;
    chart_space.chart.plot_area = plot_area;
    chart_space.chart.legend = (data.series.len() > 1
        || matches!(kind, ChartKind::Pie | ChartKind::Doughnut))
    .then(CT_Legend::default);
    let chart_xml = chart_space.to_xml()?;
    let workbook_bytes = workbook.to_xlsx_bytes().map_err(invalid_authoring_value)?;
    Ok((chart_xml, workbook_bytes))
}

/// Project one supported chart family and its cached data without changing source.
/// Sparse numeric cache entries remain NaN, and sparse labels remain empty.
/// Unsupported plots and incompatible category vectors return contextual errors.
pub fn read_chart_data(chart_xml: &[u8]) -> Result<(ChartKind, ChartData)> {
    let space = CT_ChartSpace::from_xml(chart_xml)?;
    let [plot] = space.chart.plot_area.plots()? else {
        return Err(invalid_authoring_value(
            "chart data projection requires one supported plot",
        ));
    };
    let (kind, series) = match plot {
        Plot::Bar { series, .. } => (ChartKind::Bar, series),
        Plot::Line { series, .. } => (ChartKind::Line, series),
        Plot::Pie { series, .. } => (ChartKind::Pie, series),
        Plot::Doughnut { series, .. } => (ChartKind::Doughnut, series),
        Plot::Area { series, .. } => (ChartKind::Area, series),
        Plot::Scatter { series, .. } => (ChartKind::Scatter, series),
        Plot::Radar { series, .. } => (ChartKind::Radar, series),
    };
    let first = series
        .first()
        .ok_or_else(|| invalid_authoring_value("chart has no series"))?;
    let categories = chart_category_labels(first)?;
    let mut projected = Vec::with_capacity(series.len());
    for item in series {
        if chart_category_labels(item)? != categories {
            return Err(invalid_authoring_value(
                "chart series use different category or X vectors",
            ));
        }
        let name = match &item.name {
            Some(name) => name.values.first().cloned().unwrap_or_default(),
            None if item.opaque_name => chart_literal_name(item)?,
            None => String::new(),
        };
        projected.push((name, chart_dense_numbers(&item.values)?));
    }
    Ok((
        kind,
        ChartData {
            categories,
            series: projected,
            number_format: (first.values.format_code != "General")
                .then(|| first.values.format_code.clone()),
            ..ChartData::default()
        },
    ))
}

/// Reads a bounded XL_CHART_TYPE value from one owned ChartML plot.
/// Valid unsupported or combination plots return None. Reading does not project
/// dense caches, resolve an embedded workbook, or rewrite retained source.
pub fn read_chart_type(chart_xml: &[u8]) -> Result<Option<i64>> {
    oxoffice_core::xml::validate_strict_xml_1_0(chart_xml)
        .map_err(|error| invalid_authoring_value(format!("invalid chart XML: {error:?}")))?;
    let nodes = chart_source_nodes(chart_xml)?;
    if !nodes
        .first()
        .is_some_and(|node| node.qualified && node.local == b"chartSpace")
        || nodes.iter().filter(|node| node.parent.is_none()).count() != 1
    {
        return Err(invalid_authoring_value("owned chartSpace required"));
    }
    let chart = chart_child_node(&nodes, 0, b"chart")?;
    let area = chart_child_node(&nodes, chart, b"plotArea")?;
    let mut plots = nodes.iter().enumerate().filter(|(_, node)| {
        node.parent == Some(area) && node.qualified && node.local.ends_with(b"Chart")
    });
    let Some((source_plot, plot)) = plots.next() else {
        return Ok(None);
    };
    if plots.next().is_some() {
        return Ok(None);
    }
    // Index physical children once. Each direct child is visited at most twice
    // for series metadata and twice for plot scalars, independently of series count.
    let mut children = vec![Vec::new(); nodes.len()];
    for (index, node) in nodes.iter().enumerate() {
        if let Some(parent) = node.parent {
            children[parent].push(index);
        }
    }
    let mut remaining_work = nodes.len().saturating_mul(4);
    let mut marker_none = false;
    let mut exploded = false;
    for (index, _) in nodes.iter().enumerate().filter(|(_, node)| {
        node.parent == Some(source_plot) && node.qualified && node.local == b"ser"
    }) {
        if let Some(marker) =
            chart_optional_child_node(&nodes, &children, index, b"marker", &mut remaining_work)?
            && let Some(symbol) = chart_optional_child_node(
                &nodes,
                &children,
                marker,
                b"symbol",
                &mut remaining_work,
            )?
        {
            let symbol = chart_type_scalar(chart_xml, &nodes[symbol])?;
            if !matches!(
                symbol.as_str(),
                "auto"
                    | "circle"
                    | "dash"
                    | "diamond"
                    | "dot"
                    | "none"
                    | "picture"
                    | "plus"
                    | "square"
                    | "star"
                    | "triangle"
                    | "x"
            ) {
                return Err(invalid_authoring_value("invalid marker symbol"));
            }
            marker_none |= symbol == "none";
        }
        if let Some(explosion) =
            chart_optional_child_node(&nodes, &children, index, b"explosion", &mut remaining_work)?
        {
            chart_type_scalar(chart_xml, &nodes[explosion])?
                .parse::<u32>()
                .map_err(|_| invalid_authoring_value("invalid series explosion"))?;
            exploded = true;
        }
    }
    let mut scalar = |local: &[u8]| -> Result<Option<String>> {
        chart_optional_child_node(&nodes, &children, source_plot, local, &mut remaining_work)?
            .map(|index| chart_type_scalar(chart_xml, &nodes[index]))
            .transpose()
    };
    Ok(match plot.local.as_slice() {
        b"barChart" => {
            let direction =
                scalar(b"barDir")?.ok_or_else(|| ChartError::MissingElement("c:barDir".into()))?;
            let direction = BarDirection::parse(&direction)
                .ok_or_else(|| invalid_authoring_value("invalid bar direction"))?;
            let grouping = scalar(b"grouping")?.unwrap_or_else(|| "standard".into());
            let grouping = BarGrouping::parse(&grouping)
                .ok_or_else(|| invalid_authoring_value("invalid bar grouping"))?;
            match (direction, grouping) {
                (BarDirection::Bar, BarGrouping::Clustered) => Some(57),
                (BarDirection::Bar, BarGrouping::Stacked) => Some(58),
                (BarDirection::Bar, BarGrouping::PercentStacked) => Some(59),
                (BarDirection::Column, BarGrouping::Clustered) => Some(51),
                (BarDirection::Column, BarGrouping::Stacked) => Some(52),
                (BarDirection::Column, BarGrouping::PercentStacked) => Some(53),
                (_, BarGrouping::Standard) => None,
            }
        }
        b"lineChart" | b"areaChart" => {
            let grouping = scalar(b"grouping")?.unwrap_or_else(|| "standard".into());
            let grouping = Grouping::parse(&grouping)
                .ok_or_else(|| invalid_authoring_value("invalid chart grouping"))?;
            Some(if plot.local == b"areaChart" {
                match grouping {
                    Grouping::Standard => 1,
                    Grouping::Stacked => 76,
                    Grouping::PercentStacked => 77,
                }
            } else {
                match (grouping, marker_none) {
                    (Grouping::Standard, true) => 4,
                    (Grouping::Stacked, true) => 63,
                    (Grouping::PercentStacked, true) => 64,
                    (Grouping::Standard, false) => 65,
                    (Grouping::Stacked, false) => 66,
                    (Grouping::PercentStacked, false) => 67,
                }
            })
        }
        b"pieChart" => Some(if exploded { 69 } else { 5 }),
        b"doughnutChart" => Some(if exploded { 80 } else { -4120 }),
        b"scatterChart" => {
            let value = scalar(b"scatterStyle")?
                .ok_or_else(|| ChartError::MissingElement("c:scatterStyle".into()))?;
            let style = ScatterStyle::parse(&value)
                .ok_or_else(|| invalid_authoring_value("invalid scatter style"))?;
            (style == ScatterStyle::LineMarker && !marker_none).then_some(74)
        }
        b"radarChart" => {
            let value = scalar(b"radarStyle")?
                .ok_or_else(|| ChartError::MissingElement("c:radarStyle".into()))?;
            let style = RadarStyle::parse(&value)
                .ok_or_else(|| invalid_authoring_value("invalid radar style"))?;
            (style == RadarStyle::Standard && !marker_none).then_some(81)
        }
        _ => None,
    })
}

fn chart_optional_child_node(
    nodes: &[ChartSourceNode],
    children: &[Vec<usize>],
    parent: usize,
    local: &[u8],
    remaining_work: &mut usize,
) -> Result<Option<usize>> {
    let mut found = None;
    for &index in &children[parent] {
        *remaining_work = remaining_work
            .checked_sub(1)
            .ok_or_else(|| invalid_authoring_value("chart metadata work limit"))?;
        let node = &nodes[index];
        if node.qualified && node.local == local && found.replace(index).is_some() {
            return Err(ChartError::DuplicateElement(format!(
                "c:{}",
                String::from_utf8_lossy(local)
            )));
        }
    }
    Ok(found)
}

fn chart_type_scalar(xml: &[u8], node: &ChartSourceNode) -> Result<String> {
    let mut reader = Reader::from_reader(&xml[node.start..node.end]);
    let value = match reader.read_event().map_err(OxmlError::from)? {
        Event::Start(element) | Event::Empty(element) => attribute_value(&element, b"val")?
            .ok_or_else(|| invalid_authoring_value("chart type scalar requires val"))?,
        _ => return Err(invalid_authoring_value("chart type scalar root")),
    };
    loop {
        match reader.read_event().map_err(OxmlError::from)? {
            Event::End(_) | Event::Comment(_) | Event::PI(_) => {}
            Event::Text(text) if text.iter().all(u8::is_ascii_whitespace) => {}
            Event::Eof => return Ok(value),
            _ => {
                return Err(invalid_authoring_value(
                    "chart type scalar has non-scalar content",
                ));
            }
        }
    }
}

fn chart_dense_numbers(data: &NumericData) -> Result<Vec<f64>> {
    let (count, indexes) = cache_layout(&data.markup, data.values.len())?;
    let count = chart_projection_count(count)?;
    let mut values = vec![f64::NAN; count];
    for (index, value) in indexes.into_iter().zip(&data.values) {
        values[index as usize] = *value;
    }
    Ok(values)
}

fn chart_projection_count(count: u32) -> Result<usize> {
    if count > 1_048_576 {
        return Err(invalid_attribute("ptCount", "val", count.to_string()));
    }
    Ok(count as usize)
}

fn chart_category_labels(series: &Series) -> Result<Vec<String>> {
    let (markup, labels) = match &series.categories {
        Some(AxisData::String(data)) => (&data.markup, data.values.clone()),
        Some(AxisData::Numeric(data)) => (
            &data.markup,
            data.values.iter().map(|value| value.to_string()).collect(),
        ),
        None if series.opaque_categories => {
            return Err(invalid_authoring_value(
                "unsupported literal or multilevel category projection",
            ));
        }
        None => return Ok(Vec::new()),
    };
    let (count, indexes) = cache_layout(markup, labels.len())?;
    let mut values = vec![String::new(); chart_projection_count(count)?];
    for (index, value) in indexes.into_iter().zip(labels) {
        values[index as usize] = value;
    }
    Ok(values)
}

fn chart_literal_name(series: &Series) -> Result<String> {
    for raw in series.raw_children.at(2) {
        let inherited = series
            .namespace_declarations
            .iter()
            .filter_map(|(name, value)| {
                let prefix = if name == "xmlns" {
                    ""
                } else {
                    name.strip_prefix("xmlns:")?
                };
                Some((prefix.as_bytes().to_vec(), value.clone()))
            })
            .collect::<NamespaceBindings>();
        let mut reader = Reader::from_reader(raw);
        let mut buffer = Vec::new();
        let mut namespaces = inherited;
        let mut depth = 0usize;
        loop {
            match reader
                .read_event_into(&mut buffer)
                .map_err(OxmlError::from)?
            {
                Event::Start(element) => {
                    if depth == 0 {
                        if chart_child_local(&element, &namespaces)?.as_deref() != Some(b"tx") {
                            break;
                        }
                        namespaces = chart_bindings(&namespaces, &element)?;
                    }
                    if depth == 1
                        && chart_child_local(&element, &namespaces)?.as_deref() == Some(b"v")
                    {
                        let value = capture_element(&mut reader, &element)?;
                        return Ok(parse_text_element(&value, b"v", &namespaces)?.0);
                    }
                    depth += 1;
                }
                Event::End(_) => {
                    depth = depth.saturating_sub(1);
                }
                Event::Eof => break,
                _ => {}
            }
            buffer.clear();
        }
    }
    Err(invalid_authoring_value(
        "unsupported series name projection",
    ))
}

/// Resolve only the root-owned ChartML externalData relationships attribute.
pub fn chart_workbook_relationship_id(chart_xml: &[u8]) -> Result<Option<String>> {
    use quick_xml::name::ResolveResult;
    use quick_xml::reader::NsReader;
    CT_ChartSpace::from_xml(chart_xml)?;
    let mut reader = NsReader::from_reader(chart_xml);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut result = None;
    loop {
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(OxmlError::from)?;
        let empty = matches!(&event, Event::Empty(_));
        match (namespace, event) {
            (namespace, Event::Start(element) | Event::Empty(element)) => {
                if depth == 1
                    && matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == C_NS.as_bytes())
                    && element.local_name().as_ref() == b"externalData"
                {
                    if result.is_some() {
                        return Err(ChartError::DuplicateElement("c:externalData".into()));
                    }
                    for attribute in element.attributes() {
                        let attribute = attribute.map_err(OxmlError::from)?;
                        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                        if matches!(namespace, ResolveResult::Bound(value) if value.as_ref() == R_NS.as_bytes())
                            && local.as_ref() == b"id"
                        {
                            result = Some(
                                attribute
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        reader.decoder(),
                                    )
                                    .map_err(OxmlError::from)?
                                    .into_owned(),
                            );
                        }
                    }
                    if result.is_none() {
                        return Err(ChartError::MissingElement("c:externalData/@r:id".into()));
                    }
                }
                if !empty {
                    depth += 1;
                }
            }
            (_, Event::End(_)) => {
                depth = depth.saturating_sub(1);
            }
            (_, Event::DocType(_)) => {
                return Err(invalid_authoring_value(
                    "DOCTYPE is unsupported in chart data",
                ));
            }
            (_, Event::Eof) => break,
            _ => {}
        }
        buffer.clear();
    }
    Ok(result)
}

#[derive(Debug)]
struct ChartSourceNode {
    local: Vec<u8>,
    qualified: bool,
    parent: Option<usize>,
    start: usize,
    end: usize,
}

fn chart_source_nodes(xml: &[u8]) -> Result<Vec<ChartSourceNode>> {
    use quick_xml::name::ResolveResult;
    use quick_xml::reader::NsReader;
    let mut reader = NsReader::from_reader(xml);
    let mut nodes: Vec<ChartSourceNode> = Vec::new();
    let mut stack = Vec::new();
    let mut buffer = Vec::new();
    loop {
        let start = reader.buffer_position() as usize;
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?;
        let end = reader.buffer_position() as usize;
        let empty = matches!(&event, Event::Empty(_));
        match event {
            Event::Start(element) | Event::Empty(element) => {
                let (namespace, local) = reader.resolver().resolve_element(element.name());
                let qualified = matches!(namespace,ResolveResult::Bound(value) if value.as_ref()==C_NS.as_bytes());
                let local = local.as_ref().to_vec();
                let index = nodes.len();
                if index >= 1_000_000 {
                    return Err(invalid_authoring_value("chart XML node limit"));
                }
                nodes.push(ChartSourceNode {
                    local,
                    qualified,
                    parent: stack.last().copied(),
                    start,
                    end,
                });
                if !empty {
                    stack.push(index);
                }
            }
            Event::End(_) => {
                let index = stack
                    .pop()
                    .ok_or_else(|| invalid_authoring_value("unbalanced chart source"))?;
                nodes[index].end = end;
            }
            Event::Text(text) if stack.is_empty() && !text.iter().all(u8::is_ascii_whitespace) => {
                return Err(invalid_authoring_value("text outside chart XML root"));
            }
            Event::CData(_) | Event::GeneralRef(_) if stack.is_empty() => {
                return Err(invalid_authoring_value("content outside chart XML root"));
            }
            Event::Decl(_) if start != 0 && !(start == 3 && xml.starts_with(b"\xef\xbb\xbf")) => {
                return Err(invalid_authoring_value("misplaced chart XML declaration"));
            }
            Event::DocType(_) => {
                return Err(invalid_authoring_value(
                    "DOCTYPE is unsupported in chart source",
                ));
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if !stack.is_empty() {
        return Err(invalid_authoring_value("truncated chart source"));
    }
    Ok(nodes)
}

fn chart_child_node(nodes: &[ChartSourceNode], parent: usize, local: &[u8]) -> Result<usize> {
    let found = nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.parent == Some(parent) && node.qualified && node.local == local)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let [index] = found.as_slice() else {
        return Err(invalid_authoring_value(format!(
            "chart data requires one owned {}",
            String::from_utf8_lossy(local)
        )));
    };
    Ok(*index)
}

fn chart_source_series(xml: &[u8]) -> Result<Vec<Series>> {
    CT_ChartSpace::from_xml(xml)?.chart.plot_area.series()
}

fn resized_chart_range(original: &ChartRange, count: usize) -> Result<ChartRange> {
    let mut range = original.clone();
    range.last_row = range
        .first_row
        .checked_add(
            count
                .checked_sub(1)
                .ok_or_else(|| invalid_authoring_value("empty chart data range"))?,
        )
        .ok_or_else(|| invalid_authoring_value("chart data row overflow"))?;
    range.formula().map_err(invalid_authoring_value)?;
    Ok(range)
}

fn replaced_string_reference(
    original: &StringRef,
    formula: String,
    values: Vec<String>,
) -> StringRef {
    let mut result = original.clone();
    result.formula = formula;
    result.values = values;
    result.markup.declared_point_count = None;
    result.markup.point_indexes.clear();
    result
}

fn replaced_numeric_reference(
    original: &NumericData,
    formula: String,
    values: Vec<f64>,
    number_format: Option<&str>,
) -> NumericData {
    let mut result = original.clone();
    result.formula = formula;
    result.values = values;
    if let Some(format) = number_format {
        result.format_code = format.to_owned();
    }
    result.markup.declared_point_count = None;
    result.markup.point_indexes.clear();
    result
}

/// Stage replacement chart caches and only their formula-owned workbook cells.
/// The caller publishes both returned parts together after proving package owners.
/// Original root, plot, axes, legend and surviving series formatting stay exact.
pub fn replace_chart_data(
    chart_xml: &[u8],
    workbook: &[u8],
    data: &ChartData,
) -> Result<(Vec<u8>, Vec<u8>)> {
    let (kind, current) = read_chart_data(chart_xml)?;
    validate_chart_data(kind, data)?;
    if data.category_axis_title.is_some()
        || data.value_axis_title.is_some()
        || !data.palette.is_empty()
    {
        return Err(invalid_authoring_value(
            "replace_data changes chart data, not axis titles or palette",
        ));
    }
    let workbook_id = chart_workbook_relationship_id(chart_xml)?
        .ok_or_else(|| invalid_authoring_value("chart has no embedded workbook reference"))?;
    let old_series = chart_source_series(chart_xml)?;
    let first = old_series
        .first()
        .ok_or_else(|| invalid_authoring_value("chart has no editable series"))?;
    let first_name = first
        .name
        .as_ref()
        .ok_or_else(|| invalid_authoring_value("series title needs an owned workbook formula"))?;
    let name_origin =
        ChartRange::from_formula(&first_name.formula).map_err(invalid_authoring_value)?;
    let values_origin =
        ChartRange::from_formula(&first.values.formula).map_err(invalid_authoring_value)?;
    if name_origin.first_row != name_origin.last_row || name_origin.sheet != values_origin.sheet {
        return Err(invalid_authoring_value(
            "series title and values must own one embedded worksheet",
        ));
    }
    let category_origin = match first.categories.as_ref() {
        Some(AxisData::String(reference)) => ChartRange::from_formula(&reference.formula),
        Some(AxisData::Numeric(reference)) => ChartRange::from_formula(&reference.formula),
        None => {
            return Err(invalid_authoring_value(
                "categories need an owned workbook formula",
            ));
        }
    }
    .map_err(invalid_authoring_value)?;
    if category_origin.sheet != values_origin.sheet {
        return Err(invalid_authoring_value(
            "chart data must own one embedded worksheet",
        ));
    }
    // Validate every formula boundary before an unchanged-data fast path.
    for series in &old_series {
        for formula in [
            series.name.as_ref().map(|r| r.formula.as_str()),
            Some(series.values.formula.as_str()),
            series.categories.as_ref().map(|r| match r {
                AxisData::String(r) => r.formula.as_str(),
                AxisData::Numeric(r) => r.formula.as_str(),
            }),
        ] {
            let formula = formula.ok_or_else(|| {
                invalid_authoring_value("series needs ordinary formula-backed data")
            })?;
            let range = ChartRange::from_formula(formula).map_err(invalid_authoring_value)?;
            if range.sheet != values_origin.sheet {
                return Err(invalid_authoring_value(
                    "series formula refers to a different worksheet",
                ));
            }
        }
    }
    if data.categories == current.categories
        && data.series == current.series
        && data.number_format.as_ref().is_none_or(|format| {
            Some(format) == current.number_format.as_ref()
                || (format == "General" && current.number_format.is_none())
        })
    {
        oxoffice_sml::chart_unused_columns(workbook, &values_origin.sheet, 0)
            .map_err(invalid_authoring_value)?;
        return Ok((chart_xml.to_vec(), workbook.to_vec()));
    }
    let nodes = chart_source_nodes(chart_xml)?;
    let chart = chart_child_node(&nodes, 0, b"chart")?;
    let plot_area = chart_child_node(&nodes, chart, b"plotArea")?;
    let plot_local = match kind {
        ChartKind::Bar => b"barChart".as_slice(),
        ChartKind::Line => b"lineChart",
        ChartKind::Pie => b"pieChart",
        ChartKind::Doughnut => b"doughnutChart",
        ChartKind::Area => b"areaChart",
        ChartKind::Scatter => b"scatterChart",
        ChartKind::Radar => b"radarChart",
    };
    let plot = chart_child_node(&nodes, plot_area, plot_local)?;
    let series_nodes = nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.parent == Some(plot) && node.qualified && node.local == b"ser")
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if series_nodes.len() != old_series.len() {
        return Err(invalid_authoring_value(
            "editable series source inventory disagrees",
        ));
    }
    let mut updates = Vec::new();
    let mut patches = Vec::new();
    for (ordinal, (old, &source_index)) in old_series.iter().zip(&series_nodes).enumerate() {
        let old_name = old
            .name
            .as_ref()
            .ok_or_else(|| invalid_authoring_value("series title needs formula-backed data"))?;
        let old_name_range =
            ChartRange::from_formula(&old_name.formula).map_err(invalid_authoring_value)?;
        if old_name_range.first_row != old_name_range.last_row {
            return Err(invalid_authoring_value(
                "series title formula must own one cell",
            ));
        }
        let old_value_range =
            ChartRange::from_formula(&old.values.formula).map_err(invalid_authoring_value)?;
        let categories = old
            .categories
            .as_ref()
            .ok_or_else(|| invalid_authoring_value("series categories are missing"))?;
        let old_category_range = ChartRange::from_formula(match categories {
            AxisData::String(r) => &r.formula,
            AxisData::Numeric(r) => &r.formula,
        })
        .map_err(invalid_authoring_value)?;
        let Some((name, values)) = data.series.get(ordinal) else {
            for original in [old_name_range, old_value_range, old_category_range] {
                updates.push(ChartRangeUpdate {
                    number_format: None,
                    original: Some(original),
                    target: None,
                    values: Vec::new(),
                });
            }
            patches.push((
                nodes[source_index].start,
                nodes[source_index].end,
                Vec::new(),
            ));
            continue;
        };
        let new_value_range = resized_chart_range(&old_value_range, values.len())?;
        let new_category_range = resized_chart_range(&old_category_range, data.categories.len())?;
        updates.push(ChartRangeUpdate {
            number_format: None,
            original: Some(old_name_range.clone()),
            target: Some(old_name_range.clone()),
            values: vec![ChartCellValue::Text(name.clone())],
        });
        updates.push(ChartRangeUpdate {
            number_format: Some(
                data.number_format
                    .as_deref()
                    .unwrap_or(&old.values.format_code)
                    .to_owned(),
            ),
            original: Some(old_value_range),
            target: Some(new_value_range.clone()),
            values: values.iter().copied().map(ChartCellValue::Number).collect(),
        });
        let category_cells = if matches!(categories, AxisData::Numeric(_)) {
            data.categories
                .iter()
                .map(|value| {
                    value
                        .parse::<f64>()
                        .map(ChartCellValue::Number)
                        .map_err(invalid_authoring_value)
                })
                .collect::<Result<Vec<_>>>()?
        } else {
            data.categories
                .iter()
                .cloned()
                .map(ChartCellValue::Text)
                .collect()
        };
        updates.push(ChartRangeUpdate {
            number_format: None,
            original: Some(old_category_range),
            target: Some(new_category_range.clone()),
            values: category_cells,
        });
        let tx = chart_child_node(&nodes, source_index, b"tx")?;
        let name_ref = chart_child_node(&nodes, tx, b"strRef")?;
        let name_reference = replaced_string_reference(
            old_name,
            old_name_range.formula().map_err(invalid_authoring_value)?,
            vec![name.clone()],
        );
        patches.push((
            nodes[name_ref].start,
            nodes[name_ref].end,
            name_reference.to_xml()?,
        ));
        let value_wrapper = chart_child_node(
            &nodes,
            source_index,
            if kind == ChartKind::Scatter {
                b"yVal"
            } else {
                b"val"
            },
        )?;
        let value_ref = chart_child_node(&nodes, value_wrapper, b"numRef")?;
        let value_reference = replaced_numeric_reference(
            &old.values,
            new_value_range.formula().map_err(invalid_authoring_value)?,
            values.clone(),
            data.number_format.as_deref(),
        );
        patches.push((
            nodes[value_ref].start,
            nodes[value_ref].end,
            value_reference.to_xml()?,
        ));
        let category_wrapper = chart_child_node(
            &nodes,
            source_index,
            if kind == ChartKind::Scatter {
                b"xVal"
            } else {
                b"cat"
            },
        )?;
        let (local, bytes) = match categories {
            AxisData::String(reference) => (
                b"strRef".as_slice(),
                replaced_string_reference(
                    reference,
                    new_category_range
                        .formula()
                        .map_err(invalid_authoring_value)?,
                    data.categories.clone(),
                )
                .to_xml()?,
            ),
            AxisData::Numeric(reference) => (
                b"numRef".as_slice(),
                replaced_numeric_reference(
                    reference,
                    new_category_range
                        .formula()
                        .map_err(invalid_authoring_value)?,
                    data.categories
                        .iter()
                        .map(|v| v.parse::<f64>().map_err(invalid_authoring_value))
                        .collect::<Result<Vec<_>>>()?,
                    None,
                )
                .to_xml()?,
            ),
        };
        let category_ref = chart_child_node(&nodes, category_wrapper, local)?;
        patches.push((nodes[category_ref].start, nodes[category_ref].end, bytes));
    }
    let added = data.series.len().saturating_sub(old_series.len());
    if added > 0 {
        let free = oxoffice_sml::chart_unused_columns(workbook, &values_origin.sheet, added)
            .map_err(invalid_authoring_value)?;
        let (default_xml, _) = authored_chart_parts(kind, data, &workbook_id)?;
        let defaults = chart_source_series(&default_xml)?;
        let max_index = old_series.iter().map(|s| s.index).max().unwrap();
        let max_order = old_series.iter().map(|s| s.order).max().unwrap();
        let mut bytes = Vec::new();
        for (offset, column) in free.into_iter().enumerate() {
            let ordinal = old_series.len() + offset;
            let (name, values) = &data.series[ordinal];
            let mut series = defaults[ordinal].clone();
            let step = u32::try_from(offset + 1).map_err(invalid_authoring_value)?;
            series.index = max_index
                .checked_add(step)
                .ok_or_else(|| invalid_authoring_value("chart series index overflow"))?;
            series.order = max_order
                .checked_add(step)
                .ok_or_else(|| invalid_authoring_value("chart series order overflow"))?;
            let mut title = name_origin.clone();
            title.column = column;
            let mut numbers = resized_chart_range(&values_origin, values.len())?;
            numbers.column = column;
            let category = resized_chart_range(&category_origin, data.categories.len())?;
            series.name = Some(StringRef::new(
                title.formula().map_err(invalid_authoring_value)?,
                vec![name.clone()],
            )?);
            series.values = NumericData::new(
                numbers.formula().map_err(invalid_authoring_value)?,
                data.number_format
                    .as_deref()
                    .unwrap_or(&first.values.format_code)
                    .to_owned(),
                values.clone(),
            )?;
            series.categories = Some(if kind == ChartKind::Scatter {
                AxisData::Numeric(NumericData::new(
                    category.formula().map_err(invalid_authoring_value)?,
                    "General".into(),
                    data.categories
                        .iter()
                        .map(|v| v.parse::<f64>().map_err(invalid_authoring_value))
                        .collect::<Result<Vec<_>>>()?,
                )?)
            } else {
                AxisData::String(StringRef::new(
                    category.formula().map_err(invalid_authoring_value)?,
                    data.categories.clone(),
                )?)
            });
            updates.push(ChartRangeUpdate {
                number_format: None,
                original: None,
                target: Some(title),
                values: vec![ChartCellValue::Text(name.clone())],
            });
            updates.push(ChartRangeUpdate {
                number_format: Some(
                    data.number_format
                        .as_deref()
                        .unwrap_or(&first.values.format_code)
                        .to_owned(),
                ),
                original: None,
                target: Some(numbers),
                values: values.iter().copied().map(ChartCellValue::Number).collect(),
            });
            bytes.extend(series.to_xml_for_plot(kind == ChartKind::Scatter)?);
        }
        let position = nodes[*series_nodes.last().unwrap()].end;
        patches.push((position, position, bytes));
    }
    let updated_workbook =
        oxoffice_sml::merge_chart_ranges(workbook, &updates).map_err(invalid_authoring_value)?;
    patches.sort_by_key(|(start, end, _)| (*start, *end));
    for pair in patches.windows(2) {
        if pair[0].1 > pair[1].0 {
            return Err(invalid_authoring_value(
                "overlapping chart data source edits",
            ));
        }
    }
    let mut updated_chart = chart_xml.to_vec();
    for (start, end, bytes) in patches.into_iter().rev() {
        updated_chart.splice(start..end, bytes);
    }
    let (_, projected) = read_chart_data(&updated_chart)?;
    if projected.categories != data.categories || projected.series != data.series {
        return Err(invalid_authoring_value(
            "replacement chart cache projection disagrees with requested data",
        ));
    }
    Ok((updated_chart, updated_workbook))
}

fn validate_chart_data(kind: ChartKind, data: &ChartData) -> Result<()> {
    if data.categories.is_empty() {
        return Err(invalid_authoring_value("at least one category is required"));
    }
    if data.series.is_empty() {
        return Err(invalid_authoring_value("at least one series is required"));
    }
    if matches!(kind, ChartKind::Pie | ChartKind::Doughnut) && data.series.len() != 1 {
        return Err(invalid_authoring_value(
            "pie and doughnut charts require exactly one series",
        ));
    }
    for (name, values) in &data.series {
        if values.len() != data.categories.len() {
            return Err(invalid_authoring_value(format!(
                "series {name:?} has {} values for {} categories",
                values.len(),
                data.categories.len()
            )));
        }
        if values.iter().any(|value| !value.is_finite()) {
            return Err(invalid_authoring_value(format!(
                "series {name:?} contains a nonfinite value"
            )));
        }
    }
    if kind == ChartKind::Scatter {
        for category in &data.categories {
            if category
                .parse::<f64>()
                .ok()
                .is_none_or(|value| !value.is_finite())
            {
                return Err(invalid_authoring_value(format!(
                    "scatter category {category:?} is not a finite number"
                )));
            }
        }
    }
    if let Some(format) = &data.number_format {
        if format.is_empty() {
            return Err(invalid_authoring_value("number format must not be empty"));
        }
        NumericData::new("Sheet1!$A$2".to_owned(), format.clone(), vec![0.0])?;
    }
    Ok(())
}

fn invalid_authoring_value(error: impl fmt::Display) -> ChartError {
    ChartError::InvalidValue {
        element: "chart authoring".to_owned(),
        value: error.to_string(),
    }
}

fn category_value_axes(category: AxisId, value: AxisId, data: &ChartData) -> Result<Vec<Axis>> {
    authored_axes(
        [
            Axis::new(AxisKind::Category, category, AxisPosition::Bottom, value),
            Axis::new(AxisKind::Value, value, AxisPosition::Left, category),
        ],
        data,
    )
}

fn authored_axes(mut axes: [Axis; 2], data: &ChartData) -> Result<Vec<Axis>> {
    for axis in &mut axes {
        axis.set_deleted(false);
    }
    axes[0].title = data
        .category_axis_title
        .as_deref()
        .map(CT_Title::plain_text);
    axes[1].title = data.value_axis_title.as_deref().map(CT_Title::plain_text);
    axes[1].number_format = data
        .number_format
        .as_ref()
        .map(|format| NumberFormat::new(format.clone(), false))
        .transpose()?;
    Ok(axes.into())
}

fn apply_palette(kind: ChartKind, data: &ChartData, series: &mut [Series]) {
    if data.palette.is_empty() {
        return;
    }
    for (index, item) in series.iter_mut().enumerate() {
        item.sp_pr = Some(series_shape(kind, data.palette[index % data.palette.len()]));
    }
    if matches!(kind, ChartKind::Bar | ChartKind::Pie | ChartKind::Doughnut) && series.len() == 1 {
        series[0].authored_data_points = data
            .categories
            .iter()
            .enumerate()
            .map(|(index, _)| DataPoint {
                index: index as u32,
                sp_pr: series_shape(kind, data.palette[index % data.palette.len()]),
            })
            .collect();
    }
}

fn series_shape(kind: ChartKind, color: RgbColor) -> CT_ShapeProperties {
    let mut solid = SolidFill::default();
    solid.color = Some(ColorChoice::srgb(color));
    let fill = Fill::Solid(solid);
    let mut properties = CT_ShapeProperties::default();
    if matches!(
        kind,
        ChartKind::Line | ChartKind::Scatter | ChartKind::Radar
    ) {
        let mut line = CT_LineProperties::default();
        line.fill = Some(fill);
        properties.line = Some(line);
    } else {
        properties.fill = Some(fill);
    }
    properties
}

fn spreadsheet_column_name(mut index: usize) -> String {
    let mut name = Vec::new();
    index += 1;
    while index != 0 {
        let remainder = (index - 1) % 26;
        name.push((b'A' + remainder as u8) as char);
        index = (index - 1) / 26;
    }
    name.iter().rev().collect()
}

/// Backend-neutral plot geometry and the plot rectangle reserved inside a chart.
#[derive(Clone, Debug)]
pub struct ChartGeometry {
    pub plot_bounds: Rect,
    pub elements: Vec<PositionedElement>,
}

#[derive(Clone, Debug)]
struct AxisScale {
    domain: Domain,
    ticks: Vec<f64>,
}

#[derive(Clone, Copy, Debug)]
struct AxisMapping {
    domain: Domain,
    orientation: Orientation,
}

#[derive(Clone, Copy, Debug, Default)]
struct GeometryOptions {
    category_orientation: Orientation,
    value: Option<AxisMapping>,
    scatter_x: Option<AxisMapping>,
    scatter_y: Option<AxisMapping>,
}

#[derive(Default)]
struct ChartAnnotations {
    gridlines: Vec<PositionedElement>,
    axis_lines: Vec<PositionedElement>,
    ticks: Vec<PositionedElement>,
    labels: Vec<PositionedElement>,
}

/// Converts one supported typed chart into a labelled backend-neutral group.
pub fn render_chart(
    chart: &CT_Chart,
    bounds: Rect,
    theme: &CT_OfficeStyleSheet,
    color_map: &ColorMap,
    fonts: &mut FontManager,
) -> Result<GroupElement> {
    let plots = chart.plot_area.plots()?;
    let plot = plots
        .first()
        .ok_or_else(|| ChartError::MissingElement("c:plot".to_owned()))?;
    plot.validate()?;
    let legend_layout = layout_legend(chart, plot, bounds, fonts)?;
    let plot_bounds = legend_layout.plot;
    let series_colours = resolve_series_colours(plot, theme, color_map)?;
    let axes = chart.plot_area.axes()?;
    let options = geometry_options(plot, &axes, chart.disp_blanks_as)?;
    let plot_children = render_plot_geometry(
        plot,
        plot_bounds,
        chart.disp_blanks_as,
        options,
        &series_colours,
    )?;
    if plot_children.is_empty() && !plot_can_render_without_marks(plot) {
        return Err(ChartError::InvalidValue {
            element: "c:plotArea".to_owned(),
            value: "plot has no renderable cached data".to_owned(),
        });
    }

    let mut chart_annotations =
        render_axes_and_labels(plot, &axes, plot_bounds, chart.disp_blanks_as, fonts)?;
    let mut annotations = Vec::new();
    render_data_labels(
        plot,
        plot_bounds,
        chart.disp_blanks_as,
        options,
        fonts,
        &mut chart_annotations.labels,
    )?;
    render_legend(
        legend_layout,
        &series_colours,
        &mut annotations,
        &mut chart_annotations.labels,
    );

    validate_geometry_coordinates(&chart_annotations.gridlines)?;
    validate_geometry_coordinates(&plot_children)?;
    validate_geometry_coordinates(&chart_annotations.axis_lines)?;
    validate_geometry_coordinates(&chart_annotations.ticks)?;
    validate_geometry_coordinates(&annotations)?;
    let mut children = chart_annotations.gridlines;
    children.push(PositionedElement::Group(GroupElement {
        transform: Transform::IDENTITY,
        clip: Some(Path::rect(plot_bounds)),
        opacity: 1.0,
        effects: Vec::new(),
        children: plot_children,
    }));
    children.extend(chart_annotations.axis_lines);
    children.extend(chart_annotations.ticks);
    children.extend(annotations);
    children.extend(chart_annotations.labels);
    Ok(GroupElement {
        transform: Transform::IDENTITY,
        clip: None,
        opacity: 1.0,
        effects: Vec::new(),
        children,
    })
}

/// Shapes the stable label used inside a visible unsupported-chart fallback.
pub fn render_chart_placeholder(bounds: Rect, fonts: &mut FontManager) -> Result<GroupElement> {
    if !bounds.width.is_finite()
        || !bounds.height.is_finite()
        || bounds.width <= 0.0
        || bounds.height <= 0.0
    {
        return Err(ChartError::InvalidValue {
            element: "chart fallback bounds".to_owned(),
            value: "width and height must be finite and positive".to_owned(),
        });
    }
    let label = shape_label(
        fonts,
        "Unsupported chart",
        Point {
            x: 4.0,
            y: (bounds.height / 2.0).max(9.0),
        },
    )?;
    Ok(GroupElement {
        transform: Transform::IDENTITY,
        clip: Some(Path::rect(Rect {
            x: 0.0,
            y: 0.0,
            width: bounds.width,
            height: bounds.height,
        })),
        opacity: 1.0,
        effects: Vec::new(),
        children: vec![PositionedElement::Text(label)],
    })
}

/// Converts one supported typed chart into chart-local backend-neutral paths.
pub fn render_geometry(
    chart: &CT_Chart,
    bounds: Rect,
    theme: &CT_OfficeStyleSheet,
    color_map: &ColorMap,
) -> Result<ChartGeometry> {
    let plot_bounds = geometry_plot_bounds(bounds)?;
    let plots = chart.plot_area.plots()?;
    let plot = plots
        .first()
        .ok_or_else(|| ChartError::MissingElement("c:plot".to_owned()))?;
    plot.validate()?;
    let series_colours = resolve_series_colours(plot, theme, color_map)?;
    let children = render_plot_geometry(
        plot,
        plot_bounds,
        chart.disp_blanks_as,
        geometry_options(plot, &[], chart.disp_blanks_as)?,
        &series_colours,
    )?;
    if children.is_empty() && !plot_can_render_without_marks(plot) {
        return Err(ChartError::InvalidValue {
            element: "c:plotArea".to_owned(),
            value: "plot has no renderable cached data".to_owned(),
        });
    }
    validate_geometry_coordinates(&children)?;
    Ok(ChartGeometry {
        plot_bounds,
        elements: vec![PositionedElement::Group(GroupElement {
            transform: Transform::IDENTITY,
            clip: None,
            opacity: 1.0,
            effects: Vec::new(),
            children,
        })],
    })
}

fn plot_can_render_without_marks(plot: &Plot) -> bool {
    matches!(
        plot,
        Plot::Radar { series, .. }
            if series
                .iter()
                .any(|series| !series.values.values.is_empty())
    )
}

fn geometry_plot_bounds(bounds: Rect) -> Result<Rect> {
    if ![bounds.x, bounds.y, bounds.width, bounds.height]
        .into_iter()
        .all(f64::is_finite)
        || bounds.width <= 0.0
        || bounds.height <= 0.0
        || !(bounds.x + bounds.width).is_finite()
        || !(bounds.y + bounds.height).is_finite()
    {
        return Err(ChartError::InvalidValue {
            element: "chart geometry bounds".to_owned(),
            value: format!(
                "x={}, y={}, width={}, height={}",
                bounds.x, bounds.y, bounds.width, bounds.height
            ),
        });
    }
    let plot_bounds = Rect {
        x: bounds.x + PLOT_MARGIN_LEFT,
        y: bounds.y + PLOT_MARGIN_TOP,
        width: bounds.width - PLOT_MARGIN_LEFT - PLOT_MARGIN_RIGHT,
        height: bounds.height - PLOT_MARGIN_TOP - PLOT_MARGIN_BOTTOM,
    };
    if ![
        plot_bounds.x,
        plot_bounds.y,
        plot_bounds.width,
        plot_bounds.height,
        plot_bounds.x + plot_bounds.width,
        plot_bounds.y + plot_bounds.height,
    ]
    .into_iter()
    .all(f64::is_finite)
        || plot_bounds.width <= 0.0
        || plot_bounds.height <= 0.0
    {
        return Err(ChartError::InvalidValue {
            element: "chart geometry bounds".to_owned(),
            value: "chart is too small for the fixed plot margins".to_owned(),
        });
    }
    Ok(plot_bounds)
}

fn geometry_options(plot: &Plot, axes: &[Axis], blanks: DispBlanksAs) -> Result<GeometryOptions> {
    let mut options = GeometryOptions::default();
    if let Some(category_axis) = axes.iter().find(|axis| axis.kind != AxisKind::Value) {
        options.category_orientation = category_axis.scaling.orientation;
    }

    if let Plot::Scatter { axis_ids, .. } = plot {
        let (x_domain, y_domain) = scatter_domains(plot, blanks)?;
        if let Some(axis) = axes.iter().find(|axis| axis.id == axis_ids[0]) {
            options.scatter_x = Some(axis_mapping(axis, x_domain)?);
        }
        if let Some(axis) = axes.iter().find(|axis| axis.id == axis_ids[1]) {
            options.scatter_y = Some(axis_mapping(axis, y_domain)?);
        }
    } else if let Some(domain) = plot_value_domain(plot, blanks)? {
        options.value = Some(
            axes.iter()
                .find(|axis| axis.kind == AxisKind::Value)
                .map(|axis| axis_mapping(axis, domain))
                .transpose()?
                .unwrap_or(AxisMapping {
                    domain,
                    orientation: Orientation::MinMax,
                }),
        );
    }
    Ok(options)
}

fn axis_mapping(axis: &Axis, extent: Domain) -> Result<AxisMapping> {
    if axis.scaling.log_base.is_some() {
        return Err(ChartError::InvalidValue {
            element: "c:scaling/c:logBase".to_owned(),
            value: "native chart rendering currently supports linear axes".to_owned(),
        });
    }
    let scale = nice_number_scale(extent, axis.scaling.minimum, axis.scaling.maximum, 6)?;
    Ok(AxisMapping {
        domain: scale.domain,
        orientation: axis.scaling.orientation,
    })
}

fn plot_value_domain(plot: &Plot, blanks: DispBlanksAs) -> Result<Option<Domain>> {
    let domain = match plot {
        Plot::Bar {
            grouping, series, ..
        } => {
            let logical = logical_series_values(series)?;
            let stacked = matches!(grouping, BarGrouping::Stacked | BarGrouping::PercentStacked);
            let layers =
                stacked_series_bounds(&logical, stacked, *grouping == BarGrouping::PercentStacked)?;
            domain_from_layers(&layers)?
        }
        Plot::Line {
            grouping, series, ..
        } => {
            let logical = logical_series_values(series)?;
            if *grouping == Grouping::Standard {
                let values = if blanks == DispBlanksAs::Zero {
                    zero_filled_series(&logical, series_category_count(series)?)
                        .into_iter()
                        .flat_map(|(_, points)| points.into_iter().map(|(_, value)| value))
                        .collect::<Vec<_>>()
                } else {
                    logical
                        .into_iter()
                        .flat_map(|(_, points)| points.into_iter().map(|(_, value)| value))
                        .collect::<Vec<_>>()
                };
                data_domain(&values, false)?
            } else {
                let layers =
                    stacked_series_bounds(&logical, true, *grouping == Grouping::PercentStacked)?;
                domain_from_layers(&layers)?
            }
        }
        Plot::Area {
            grouping, series, ..
        } => {
            let logical = logical_series_values(series)?;
            let layers = stacked_series_bounds(
                &logical,
                *grouping != Grouping::Standard,
                *grouping == Grouping::PercentStacked,
            )?;
            domain_from_layers(&layers)?
        }
        Plot::Radar { series, .. } => {
            let values = series
                .iter()
                .flat_map(|series| series.values.values.iter().copied())
                .collect::<Vec<_>>();
            data_domain(&values, true)?
        }
        Plot::Pie { .. } | Plot::Doughnut { .. } | Plot::Scatter { .. } => return Ok(None),
    };
    Ok(Some(domain))
}

fn scatter_domains(plot: &Plot, blanks: DispBlanksAs) -> Result<(Domain, Domain)> {
    let Plot::Scatter { series, .. } = plot else {
        return Err(ChartError::UnexpectedElement("non-scatter plot".to_owned()));
    };
    let mut x_values = Vec::new();
    let mut y_values = Vec::new();
    for series in series {
        for (_, x, y) in scatter_render_points(series, blanks)? {
            x_values.push(x);
            y_values.push(y);
        }
    }
    Ok((
        data_domain(&x_values, false)?,
        data_domain(&y_values, false)?,
    ))
}

fn render_axes_and_labels(
    plot: &Plot,
    axes: &[Axis],
    bounds: Rect,
    blanks: DispBlanksAs,
    fonts: &mut FontManager,
) -> Result<ChartAnnotations> {
    if matches!(plot, Plot::Radar { .. }) {
        return render_radar_axes_and_labels(plot, axes, bounds, blanks, fonts);
    }
    let mut annotations = ChartAnnotations::default();
    let series = plot_series(plot);
    let category_count = if series.is_empty() {
        1
    } else {
        series_category_count(series)?
    };
    for axis in axes {
        if axis.deleted {
            continue;
        }
        axis_line(&mut annotations.axis_lines, bounds, axis.position);
        if axis.kind == AxisKind::Value {
            let extent = axis_extent(plot, axis, blanks)?;
            let scale = nice_number_scale(extent, axis.scaling.minimum, axis.scaling.maximum, 6)?;
            for tick in &scale.ticks {
                let coordinate = axis_tick_coordinate(
                    *tick,
                    scale.domain,
                    bounds,
                    axis.position,
                    axis.scaling.orientation,
                )?;
                if axis.major_gridlines.is_some() {
                    major_gridline(
                        &mut annotations.gridlines,
                        bounds,
                        axis.position,
                        coordinate,
                    );
                }
                axis_tick(
                    &mut annotations.ticks,
                    bounds,
                    axis.position,
                    coordinate,
                    axis.major_tick_mark,
                );
                if axis.tick_label_position != TickLabelPosition::None {
                    let text = format_axis_value(axis.number_format.as_ref(), *tick)?;
                    let origin = tick_label_origin(
                        bounds,
                        axis.position,
                        axis.tick_label_position,
                        coordinate,
                    );
                    annotations
                        .labels
                        .push(PositionedElement::Text(shape_label_with_properties(
                            fonts,
                            &text,
                            origin,
                            axis.tx_pr.as_ref(),
                        )?));
                }
            }
        } else {
            let count = category_count;
            if count > MAX_CATEGORY_ANNOTATIONS {
                return Err(ChartError::InvalidValue {
                    element: "c:catAx annotations".to_owned(),
                    value: format!(
                        "logical category count {count} exceeds {MAX_CATEGORY_ANNOTATIONS}"
                    ),
                });
            }
            let category_labels: BTreeMap<_, _> =
                if axis.tick_label_position != TickLabelPosition::None {
                    series
                        .first()
                        .map(|series| series_categories(series, axis.number_format.as_ref()))
                        .transpose()?
                        .unwrap_or_default()
                        .into_iter()
                        .collect()
                } else {
                    BTreeMap::new()
                };
            for logical_index in 0..count {
                let index = oriented_category_index(logical_index, count, axis.scaling.orientation);
                let fraction = (index as f64 + 0.5) / count as f64;
                let coordinate =
                    if matches!(axis.position, AxisPosition::Bottom | AxisPosition::Top) {
                        bounds.x + fraction * bounds.width
                    } else {
                        bounds.y + fraction * bounds.height
                    };
                if axis.major_gridlines.is_some() {
                    major_gridline(
                        &mut annotations.gridlines,
                        bounds,
                        axis.position,
                        coordinate,
                    );
                }
                axis_tick(
                    &mut annotations.ticks,
                    bounds,
                    axis.position,
                    coordinate,
                    axis.major_tick_mark,
                );
                if axis.tick_label_position != TickLabelPosition::None
                    && let Some(text) = category_labels.get(&logical_index)
                {
                    let origin = tick_label_origin(
                        bounds,
                        axis.position,
                        axis.tick_label_position,
                        coordinate,
                    );
                    annotations
                        .labels
                        .push(PositionedElement::Text(shape_label_with_properties(
                            fonts,
                            text,
                            origin,
                            axis.tx_pr.as_ref(),
                        )?));
                }
            }
        }
    }
    Ok(annotations)
}

fn render_radar_axes_and_labels(
    plot: &Plot,
    axes: &[Axis],
    bounds: Rect,
    blanks: DispBlanksAs,
    fonts: &mut FontManager,
) -> Result<ChartAnnotations> {
    let mut annotations = ChartAnnotations::default();
    let series = plot_series(plot);
    let count = series_category_count(series)?;
    if count > MAX_CATEGORY_ANNOTATIONS {
        return Err(ChartError::InvalidValue {
            element: "c:radarChart annotations".to_owned(),
            value: format!("logical category count {count} exceeds {MAX_CATEGORY_ANNOTATIONS}"),
        });
    }
    let center = Point {
        x: bounds.x + bounds.width / 2.0,
        y: bounds.y + bounds.height / 2.0,
    };
    let radius = bounds.width.min(bounds.height) / 2.0;
    for axis in axes {
        if axis.deleted {
            continue;
        }
        if axis.kind == AxisKind::Value {
            let scale = nice_number_scale(
                axis_extent(plot, axis, blanks)?,
                axis.scaling.minimum,
                axis.scaling.maximum,
                6,
            )?;
            annotations.axis_lines.push(stroked_segment(
                center,
                radial_point(center, radius, 0, count),
                Color::BLACK,
                1.0,
            ));
            for tick in &scale.ticks {
                let distance =
                    normalized_value_oriented(*tick, scale.domain, axis.scaling.orientation)?
                        * radius;
                if axis.major_gridlines.is_some() && distance > 0.0 {
                    let points: Vec<_> = (0..count)
                        .map(|index| radial_point(center, distance, index, count))
                        .collect();
                    annotations
                        .gridlines
                        .push(PositionedElement::Path(PathElement {
                            path: polyline_path(&points, true),
                            fill: None,
                            stroke: Some(Stroke::new(
                                Paint::Solid(Color {
                                    r: 0.82,
                                    g: 0.82,
                                    b: 0.82,
                                    a: 1.0,
                                }),
                                0.5,
                            )),
                        }));
                }
                radar_value_tick(
                    &mut annotations.ticks,
                    center,
                    distance,
                    axis.major_tick_mark,
                );
                let origin = match axis.tick_label_position {
                    TickLabelPosition::High => Some(Point {
                        x: center.x + radius + 4.0,
                        y: center.y - distance + 3.0,
                    }),
                    TickLabelPosition::Low => Some(Point {
                        x: center.x - radius - 30.0,
                        y: center.y - distance + 3.0,
                    }),
                    TickLabelPosition::NextTo => Some(Point {
                        x: center.x + 4.0,
                        y: center.y - distance + 3.0,
                    }),
                    TickLabelPosition::None => None,
                };
                if let Some(origin) = origin {
                    let text = format_axis_value(axis.number_format.as_ref(), *tick)?;
                    annotations
                        .labels
                        .push(PositionedElement::Text(shape_label_with_properties(
                            fonts,
                            &text,
                            origin,
                            axis.tx_pr.as_ref(),
                        )?));
                }
            }
        } else {
            let category_labels: BTreeMap<_, _> =
                if axis.tick_label_position != TickLabelPosition::None {
                    series
                        .first()
                        .map(|series| series_categories(series, axis.number_format.as_ref()))
                        .transpose()?
                        .unwrap_or_default()
                        .into_iter()
                        .collect()
                } else {
                    BTreeMap::new()
                };
            for logical_index in 0..count {
                let index = oriented_category_index(logical_index, count, axis.scaling.orientation);
                let end = radial_point(center, radius, index, count);
                annotations
                    .axis_lines
                    .push(stroked_segment(center, end, Color::BLACK, 1.0));
                radar_category_tick(
                    &mut annotations.ticks,
                    center,
                    radius,
                    index,
                    count,
                    axis.major_tick_mark,
                );
                if let Some(text) = category_labels.get(&logical_index) {
                    let Some(origin) = radar_category_label_origin(
                        bounds,
                        center,
                        radius,
                        index,
                        count,
                        axis.tick_label_position,
                    ) else {
                        continue;
                    };
                    annotations
                        .labels
                        .push(PositionedElement::Text(shape_label_with_properties(
                            fonts,
                            text,
                            origin,
                            axis.tx_pr.as_ref(),
                        )?));
                }
            }
        }
    }
    Ok(annotations)
}

fn radar_category_label_origin(
    bounds: Rect,
    center: Point,
    radius: f64,
    index: usize,
    count: usize,
    position: TickLabelPosition,
) -> Option<Point> {
    let label_radius = match position {
        TickLabelPosition::High => radius + 18.0,
        TickLabelPosition::Low => (radius - 10.0).max(0.0),
        TickLabelPosition::NextTo => radius + 10.0,
        TickLabelPosition::None => return None,
    };
    let label = radial_point(center, label_radius, index, count);
    let mut origin = Point {
        x: label.x - 4.0,
        y: label.y + 3.0,
    };
    if position == TickLabelPosition::High {
        origin.x = origin.x.clamp(
            bounds.x - PLOT_MARGIN_LEFT,
            bounds.x + bounds.width + PLOT_MARGIN_RIGHT,
        );
        origin.y = origin.y.clamp(
            bounds.y - PLOT_MARGIN_TOP,
            bounds.y + bounds.height + PLOT_MARGIN_BOTTOM,
        );
    }
    Some(origin)
}

fn radar_value_tick(
    elements: &mut Vec<PositionedElement>,
    center: Point,
    distance: f64,
    mark: TickMark,
) {
    let (left, right) = match mark {
        TickMark::None => return,
        TickMark::Inside => (4.0, 0.0),
        TickMark::Outside => (0.0, 4.0),
        TickMark::Cross => (2.0, 2.0),
    };
    let y = center.y - distance;
    elements.push(stroked_segment(
        Point {
            x: center.x - left,
            y,
        },
        Point {
            x: center.x + right,
            y,
        },
        Color::BLACK,
        1.0,
    ));
}

fn radar_category_tick(
    elements: &mut Vec<PositionedElement>,
    center: Point,
    radius: f64,
    index: usize,
    count: usize,
    mark: TickMark,
) {
    let (inside, outside) = match mark {
        TickMark::None => return,
        TickMark::Inside => (4.0, 0.0),
        TickMark::Outside => (0.0, 4.0),
        TickMark::Cross => (2.0, 2.0),
    };
    elements.push(stroked_segment(
        radial_point(center, radius - inside, index, count),
        radial_point(center, radius + outside, index, count),
        Color::BLACK,
        1.0,
    ));
}

fn axis_extent(plot: &Plot, axis: &Axis, blanks: DispBlanksAs) -> Result<Domain> {
    if let Plot::Scatter { axis_ids, .. } = plot {
        let (x, y) = scatter_domains(plot, blanks)?;
        return Ok(if axis.id == axis_ids[0] { x } else { y });
    }
    plot_value_domain(plot, blanks)?.ok_or_else(|| ChartError::InvalidValue {
        element: "chart axis".to_owned(),
        value: "axis-free plot supplied an axis".to_owned(),
    })
}

fn axis_tick_coordinate(
    value: f64,
    domain: Domain,
    bounds: Rect,
    position: AxisPosition,
    orientation: Orientation,
) -> Result<f64> {
    if matches!(position, AxisPosition::Bottom | AxisPosition::Top) {
        map_x_oriented(value, domain, bounds, orientation)
    } else {
        map_y_oriented(value, domain, bounds, orientation)
    }
}

fn axis_line(elements: &mut Vec<PositionedElement>, bounds: Rect, position: AxisPosition) {
    let (start, end) = match position {
        AxisPosition::Bottom => (
            Point {
                x: bounds.x,
                y: bounds.y + bounds.height,
            },
            Point {
                x: bounds.x + bounds.width,
                y: bounds.y + bounds.height,
            },
        ),
        AxisPosition::Top => (
            Point {
                x: bounds.x,
                y: bounds.y,
            },
            Point {
                x: bounds.x + bounds.width,
                y: bounds.y,
            },
        ),
        AxisPosition::Left => (
            Point {
                x: bounds.x,
                y: bounds.y,
            },
            Point {
                x: bounds.x,
                y: bounds.y + bounds.height,
            },
        ),
        AxisPosition::Right => (
            Point {
                x: bounds.x + bounds.width,
                y: bounds.y,
            },
            Point {
                x: bounds.x + bounds.width,
                y: bounds.y + bounds.height,
            },
        ),
    };
    elements.push(stroked_segment(start, end, Color::BLACK, 1.0));
}

fn major_gridline(
    elements: &mut Vec<PositionedElement>,
    bounds: Rect,
    position: AxisPosition,
    coordinate: f64,
) {
    let (start, end) = if matches!(position, AxisPosition::Bottom | AxisPosition::Top) {
        (
            Point {
                x: coordinate,
                y: bounds.y,
            },
            Point {
                x: coordinate,
                y: bounds.y + bounds.height,
            },
        )
    } else {
        (
            Point {
                x: bounds.x,
                y: coordinate,
            },
            Point {
                x: bounds.x + bounds.width,
                y: coordinate,
            },
        )
    };
    elements.push(stroked_segment(
        start,
        end,
        Color {
            r: 0.82,
            g: 0.82,
            b: 0.82,
            a: 1.0,
        },
        0.5,
    ));
}

fn axis_tick(
    elements: &mut Vec<PositionedElement>,
    bounds: Rect,
    position: AxisPosition,
    coordinate: f64,
    mark: TickMark,
) {
    let (inside, outside) = match mark {
        TickMark::None => return,
        TickMark::Inside => (4.0, 0.0),
        TickMark::Outside => (0.0, 4.0),
        TickMark::Cross => (2.0, 2.0),
    };
    let (start, end) = match position {
        AxisPosition::Bottom => (
            Point {
                x: coordinate,
                y: bounds.y + bounds.height - inside,
            },
            Point {
                x: coordinate,
                y: bounds.y + bounds.height + outside,
            },
        ),
        AxisPosition::Top => (
            Point {
                x: coordinate,
                y: bounds.y + inside,
            },
            Point {
                x: coordinate,
                y: bounds.y - outside,
            },
        ),
        AxisPosition::Left => (
            Point {
                x: bounds.x + inside,
                y: coordinate,
            },
            Point {
                x: bounds.x - outside,
                y: coordinate,
            },
        ),
        AxisPosition::Right => (
            Point {
                x: bounds.x + bounds.width - inside,
                y: coordinate,
            },
            Point {
                x: bounds.x + bounds.width + outside,
                y: coordinate,
            },
        ),
    };
    elements.push(stroked_segment(start, end, Color::BLACK, 1.0));
}

fn tick_label_origin(
    bounds: Rect,
    axis_position: AxisPosition,
    label_position: TickLabelPosition,
    coordinate: f64,
) -> Point {
    let side = match label_position {
        TickLabelPosition::NextTo => axis_position,
        TickLabelPosition::High => {
            if matches!(axis_position, AxisPosition::Bottom | AxisPosition::Top) {
                AxisPosition::Top
            } else {
                AxisPosition::Right
            }
        }
        TickLabelPosition::Low => {
            if matches!(axis_position, AxisPosition::Bottom | AxisPosition::Top) {
                AxisPosition::Bottom
            } else {
                AxisPosition::Left
            }
        }
        TickLabelPosition::None => axis_position,
    };
    match side {
        AxisPosition::Bottom => Point {
            x: coordinate - 4.0,
            y: bounds.y + bounds.height + 12.0,
        },
        AxisPosition::Top => Point {
            x: coordinate - 4.0,
            y: bounds.y - 4.0,
        },
        AxisPosition::Left => Point {
            x: bounds.x - 30.0,
            y: coordinate + 3.0,
        },
        AxisPosition::Right => Point {
            x: bounds.x + bounds.width + 4.0,
            y: coordinate + 3.0,
        },
    }
}

fn format_axis_value(format: Option<&NumberFormat>, value: f64) -> Result<String> {
    if let Some(format) = format {
        format_or_general(format, value)
    } else {
        NumberFormat::new("General".to_owned(), false)?.format_value(value)
    }
}

fn format_or_general(format: &NumberFormat, value: f64) -> Result<String> {
    match format.format_value(value) {
        Err(ChartError::InvalidValue { element, .. })
            if element == "c:numFmt/@formatCode projection" && value.is_finite() =>
        {
            Ok(value.to_string())
        }
        result => result,
    }
}
/// Refused numeric label codes, deduplicated in plot, point and axis source order.
pub fn unsupported_number_formats(chart: &CT_Chart) -> Result<Vec<String>> {
    let mut codes = Vec::new();
    let mut note = |format: &NumberFormat| {
        if format.format_code != "General"
            && numeric_sections(&format.format_code).is_none()
            && !codes.contains(&format.format_code)
        {
            codes.push(format.format_code.clone());
        }
    };
    for plot in chart.plot_area.plots()? {
        let mut sets = plot_data_labels(plot).into_iter().collect::<Vec<_>>();
        for series in plot_series(plot) {
            sets.extend(series.data_labels.as_ref());
        }
        for labels in &sets {
            labels.number_format.iter().for_each(&mut note);
        }
        for series in plot_series(plot) {
            if let Some(AxisData::Numeric(data)) = &series.categories {
                note(&NumberFormat::new(data.format_code.clone(), false)?);
            }
        }
        for labels in sets {
            // Validate existing duplicate/index policy, then walk physical raw order.
            point_label_overrides(labels)?;
            let mut namespaces = chart_namespace_defaults();
            for (name, uri) in &labels.namespace_declarations {
                if name == "xmlns" {
                    upsert_namespace_binding(&mut namespaces, Vec::new(), uri.clone());
                } else if let Some(prefix) = name.strip_prefix("xmlns:") {
                    upsert_namespace_binding(
                        &mut namespaces,
                        prefix.as_bytes().to_vec(),
                        uri.clone(),
                    );
                }
            }
            for raw in labels.raw_children.at(0) {
                if chart_root_local(raw, &namespaces)?.as_deref() == Some(b"dLbl") {
                    parse_point_label_override(raw, &namespaces)?
                        .number_format
                        .iter()
                        .for_each(&mut note);
                }
            }
        }
    }
    for axis in chart.plot_area.axes()? {
        axis.number_format.iter().for_each(&mut note);
    }
    Ok(codes)
}

fn shape_label(fonts: &mut FontManager, text: &str, origin: Point) -> Result<GlyphRun> {
    shape_label_with_properties(fonts, text, origin, None)
}

fn shape_label_with_properties(
    fonts: &mut FontManager,
    text: &str,
    origin: Point,
    properties: Option<&CT_TextBody>,
) -> Result<GlyphRun> {
    const FAMILY: &str = "Carlito";
    const SIZE: f64 = 9.0;
    let properties = properties.and_then(chart_text_properties);
    let family = properties
        .and_then(|properties| properties.latin.as_ref())
        .map(|font| font.typeface.as_str())
        .unwrap_or(FAMILY);
    let size = properties
        .and_then(|properties| properties.font_size)
        .map(|size| f64::from(size) / 100.0)
        .unwrap_or(SIZE);
    let bold = properties
        .and_then(|properties| properties.bold)
        .unwrap_or(false);
    let italic = properties
        .and_then(|properties| properties.italic)
        .unwrap_or(false);
    let font_id = fonts
        .resolve_font_for_text(Some(family), bold, italic, text)
        .map_err(|error| ChartError::InvalidValue {
            element: "chart label font".to_owned(),
            value: error.to_string(),
        })?;
    let shaped =
        fonts
            .shape_text(font_id, text, size)
            .map_err(|error| ChartError::InvalidValue {
                element: "chart label shaping".to_owned(),
                value: error.to_string(),
            })?;
    Ok(GlyphRun {
        origin,
        font_id,
        font_size: size,
        glyph_ids: shaped.glyph_ids,
        advances: shaped.advances,
        text: text.to_owned(),
        source: None,
        color: Color::BLACK,
        bold,
        italic,
        field_kind: None,
        field_source: None,
        note: None,
        note_reference_source: None,
        tab_aligned: None,
    })
}

fn chart_text_properties(body: &CT_TextBody) -> Option<&CT_TextCharacterProperties> {
    body.paragraphs()
        .first()
        .and_then(|paragraph| paragraph.properties.as_ref())
        .and_then(|properties| properties.default_run_properties.as_ref())
}

fn stroked_segment(start: Point, end: Point, color: Color, width: f64) -> PositionedElement {
    PositionedElement::Path(PathElement {
        path: Path {
            commands: vec![PathCommand::MoveTo(start), PathCommand::LineTo(end)],
            fill_rule: FillRule::NonZero,
        },
        fill: None,
        stroke: Some(Stroke::new(Paint::Solid(color), width)),
    })
}

fn plot_series(plot: &Plot) -> &[Series] {
    match plot {
        Plot::Bar { series, .. }
        | Plot::Line { series, .. }
        | Plot::Pie { series, .. }
        | Plot::Doughnut { series, .. }
        | Plot::Area { series, .. }
        | Plot::Scatter { series, .. }
        | Plot::Radar { series, .. } => series,
    }
}

fn plot_data_labels(plot: &Plot) -> Option<&CT_DLbls> {
    match plot {
        Plot::Bar { data_labels, .. }
        | Plot::Line { data_labels, .. }
        | Plot::Pie { data_labels, .. }
        | Plot::Doughnut { data_labels, .. }
        | Plot::Area { data_labels, .. }
        | Plot::Scatter { data_labels, .. }
        | Plot::Radar { data_labels, .. } => data_labels.as_ref(),
    }
}

fn series_categories(
    series: &Series,
    number_format: Option<&NumberFormat>,
) -> Result<Vec<(usize, String)>> {
    let Some(categories) = &series.categories else {
        return Ok(Vec::new());
    };
    match categories {
        AxisData::String(data) => {
            let (_, indexes) = cache_layout(&data.markup, data.values.len())?;
            indexes
                .into_iter()
                .zip(&data.values)
                .map(|(index, value)| {
                    usize::try_from(index)
                        .map(|index| (index, value.clone()))
                        .map_err(|_| ChartError::InvalidValue {
                            element: "c:cat/c:pt/@idx".to_owned(),
                            value: index.to_string(),
                        })
                })
                .collect()
        }
        AxisData::Numeric(data) => {
            let (_, indexes) = cache_layout(&data.markup, data.values.len())?;
            let cache_format;
            let format = if let Some(format) = number_format {
                format
            } else {
                cache_format = NumberFormat::new(data.format_code.clone(), false)?;
                &cache_format
            };
            indexes
                .into_iter()
                .zip(&data.values)
                .map(|(index, value)| {
                    let index = usize::try_from(index).map_err(|_| ChartError::InvalidValue {
                        element: "c:cat/c:pt/@idx".to_owned(),
                        value: index.to_string(),
                    })?;
                    let text = format_or_general(format, *value)?;
                    Ok((index, text))
                })
                .collect()
        }
    }
}

fn series_name(series: &Series, index: usize) -> String {
    series
        .name
        .as_ref()
        .and_then(|name| name.values.first())
        .cloned()
        .unwrap_or_else(|| format!("Series {}", index + 1))
}

fn render_data_labels(
    plot: &Plot,
    bounds: Rect,
    blanks: DispBlanksAs,
    options: GeometryOptions,
    fonts: &mut FontManager,
    elements: &mut Vec<PositionedElement>,
) -> Result<()> {
    let plot_labels = plot_data_labels(plot);
    let anchors = plot_label_anchors(plot, bounds, blanks, options)?;
    for (series_index, series) in plot_series(plot).iter().enumerate() {
        let Some(labels) = series.data_labels.as_ref().or(plot_labels) else {
            continue;
        };
        let overrides = point_label_overrides(labels)?;
        let mut categories = None;
        let mut bubble_sizes = None;
        let (_, values) = logical_numeric_values(&series.values)?;
        let mut percentage_total = None;
        for (logical_index, value) in values {
            let point_override = overrides.get(&logical_index);
            if point_override.is_some_and(|point| point.deleted == Some(true)) {
                continue;
            }
            let effective_labels = point_override
                .map(|point| point.apply_to(labels))
                .unwrap_or_else(|| labels.clone());
            let labels = &effective_labels;
            let Some(anchor) = anchors.get(&(series_index, logical_index)).copied() else {
                continue;
            };
            let mut fields = Vec::new();
            if labels.show_series_name {
                fields.push(series_name(series, series_index));
            }
            if labels.show_category_name {
                if categories.is_none() {
                    categories = Some(
                        series_categories(series, None)?
                            .into_iter()
                            .collect::<BTreeMap<_, _>>(),
                    );
                }
                if let Some(category) = categories
                    .as_ref()
                    .and_then(|categories| categories.get(&logical_index))
                {
                    fields.push(category.clone());
                }
            }
            if labels.show_value {
                fields.push(format_axis_value(labels.number_format.as_ref(), value)?);
            }
            if labels.show_percent {
                let total = if let Some(total) = percentage_total {
                    total
                } else {
                    let total = series
                        .values
                        .values
                        .iter()
                        .copied()
                        .filter(|value| *value > 0.0)
                        .try_fold(0.0, |total, value| {
                            checked_geometry_sum(total, value, "data-label percentage total")
                        })?;
                    percentage_total = Some(total);
                    total
                };
                if total > 0.0 {
                    let text = if let Some(format) = &labels.number_format {
                        format_or_general(format, value / total)?
                    } else {
                        NumberFormat::new("0%".to_owned(), false)?.format_value(value / total)?
                    };
                    fields.push(text);
                }
            }
            if labels.show_bubble_size {
                if bubble_sizes.is_none() {
                    bubble_sizes = Some(
                        series
                            .bubble_size
                            .as_ref()
                            .map(logical_numeric_values)
                            .transpose()?
                            .map(|(_, values)| values.into_iter().collect::<BTreeMap<_, _>>())
                            .unwrap_or_default(),
                    );
                }
                if let Some(value) = bubble_sizes
                    .as_ref()
                    .and_then(|sizes| sizes.get(&logical_index))
                {
                    fields.push(format_axis_value(labels.number_format.as_ref(), *value)?);
                }
            }
            if fields.is_empty() {
                continue;
            }
            let separator = labels.separator.as_deref().unwrap_or(", ");
            let text = fields.join(separator);
            let origin = anchor.origin(labels.position);
            elements.push(PositionedElement::Text(shape_label(fonts, &text, origin)?));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Default)]
struct PointLabelOverride {
    index: Option<usize>,
    deleted: Option<bool>,
    number_format: Option<NumberFormat>,
    position: Option<DataLabelPosition>,
    separator: Option<String>,
    show_legend_key: Option<bool>,
    show_value: Option<bool>,
    show_category_name: Option<bool>,
    show_series_name: Option<bool>,
    show_percent: Option<bool>,
    show_bubble_size: Option<bool>,
}

impl PointLabelOverride {
    fn apply_to(&self, labels: &CT_DLbls) -> CT_DLbls {
        let mut effective = labels.clone();
        if let Some(value) = &self.number_format {
            effective.number_format = Some(value.clone());
        }
        if let Some(value) = self.position {
            effective.position = Some(value);
        }
        if let Some(value) = &self.separator {
            effective.separator = Some(value.clone());
        }
        if let Some(value) = self.show_legend_key {
            effective.show_legend_key = value;
        }
        if let Some(value) = self.show_value {
            effective.show_value = value;
        }
        if let Some(value) = self.show_category_name {
            effective.show_category_name = value;
        }
        if let Some(value) = self.show_series_name {
            effective.show_series_name = value;
        }
        if let Some(value) = self.show_percent {
            effective.show_percent = value;
        }
        if let Some(value) = self.show_bubble_size {
            effective.show_bubble_size = value;
        }
        effective
    }
}

fn point_label_overrides(labels: &CT_DLbls) -> Result<BTreeMap<usize, PointLabelOverride>> {
    let mut namespaces = chart_namespace_defaults();
    for (name, namespace) in &labels.namespace_declarations {
        let prefix = if name == "xmlns" {
            Vec::new()
        } else if let Some(prefix) = name.strip_prefix("xmlns:") {
            prefix.as_bytes().to_vec()
        } else {
            continue;
        };
        upsert_namespace_binding(&mut namespaces, prefix, namespace.clone());
    }
    let mut overrides = BTreeMap::new();
    for raw in labels.raw_children.at(0) {
        if chart_root_local(raw, &namespaces)?.as_deref() != Some(b"dLbl") {
            continue;
        }
        let point = parse_point_label_override(raw, &namespaces)?;
        let index = point
            .index
            .ok_or_else(|| ChartError::MissingElement("c:dLbl/c:idx".to_owned()))?;
        if overrides.insert(index, point).is_some() {
            return Err(ChartError::DuplicateElement(format!(
                "c:dLbl[c:idx={index}]"
            )));
        }
    }
    Ok(overrides)
}

fn parse_point_label_override(
    xml: &[u8],
    inherited: &NamespaceBindings,
) -> Result<PointLabelOverride> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut namespaces = None;
    let mut state = PointLabelOverride::default();
    let mut order = 0;
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(OxmlError::from)?
        {
            Event::Start(element) if namespaces.is_none() => {
                if !matches_local_name(element.name().as_ref(), b"dLbl")
                    || !element_is_in_namespace(&element, C_NS, inherited)?
                {
                    return Err(ChartError::UnexpectedElement(element_name(&element)));
                }
                namespaces = Some(chart_bindings(inherited, &element)?);
            }
            Event::Start(element) => {
                let bindings = namespaces
                    .as_ref()
                    .ok_or_else(|| ChartError::UnexpectedElement(element_name(&element)))?;
                let name = chart_child_local(&element, bindings)?;
                let raw = capture_element(&mut reader, &element)?;
                parse_point_label_child(&mut state, name.as_deref(), &raw, bindings, &mut order)?;
            }
            Event::Empty(element) => {
                let bindings = namespaces
                    .as_ref()
                    .ok_or_else(|| ChartError::UnexpectedElement(element_name(&element)))?;
                let name = chart_child_local(&element, bindings)?;
                let raw = capture_empty_element(&element)?;
                parse_point_label_child(&mut state, name.as_deref(), &raw, bindings, &mut order)?;
            }
            Event::End(element) if matches_local_name(element.name().as_ref(), b"dLbl") => {
                if state.index.is_none() {
                    return Err(ChartError::MissingElement("c:dLbl/c:idx".to_owned()));
                }
                return Ok(state);
            }
            Event::Eof => return Err(missing_end("c:dLbl")),
            _ => {}
        }
        buffer.clear();
    }
}

fn parse_point_label_child(
    state: &mut PointLabelOverride,
    name: Option<&[u8]>,
    raw: &[u8],
    namespaces: &NamespaceBindings,
    order: &mut usize,
) -> Result<()> {
    let Some(name) = name else {
        return Ok(());
    };
    let rank = match name {
        b"idx" => 0,
        b"delete" => 1,
        b"layout" => 2,
        b"tx" => 3,
        b"numFmt" => 4,
        b"spPr" => 5,
        b"txPr" => 6,
        b"dLblPos" => 7,
        b"showLegendKey" => 8,
        b"showVal" => 9,
        b"showCatName" => 10,
        b"showSerName" => 11,
        b"showPercent" => 12,
        b"showBubbleSize" => 13,
        b"separator" => 14,
        b"showLeaderLines" => 15,
        b"leaderLines" => 16,
        b"extLst" => 17,
        _ => return Ok(()),
    };
    if rank < *order {
        return Err(ChartError::InvalidValue {
            element: "c:dLbl".to_owned(),
            value: format!("c:{} is out of schema order", String::from_utf8_lossy(name)),
        });
    }
    *order = rank;
    match name {
        b"idx" => {
            let (index, _) = parse_u32_scalar(raw, "idx")?;
            let index = usize::try_from(index).map_err(|_| ChartError::InvalidValue {
                element: "c:dLbl/c:idx".to_owned(),
                value: index.to_string(),
            })?;
            set_once(&mut state.index, index, "c:dLbl/c:idx")?;
        }
        b"delete" => set_once(
            &mut state.deleted,
            parse_bool_value(raw, "delete")?.0,
            "c:dLbl/c:delete",
        )?,
        b"numFmt" => set_once(
            &mut state.number_format,
            NumberFormat::from_xml(raw)?,
            "c:dLbl/c:numFmt",
        )?,
        b"dLblPos" => {
            let (value, _) = scalar_value(raw, "dLblPos")?;
            let value =
                value.ok_or_else(|| invalid_attribute("dLblPos", "val", "<missing>".to_owned()))?;
            let position = DataLabelPosition::parse(&value)
                .ok_or_else(|| invalid_attribute("dLblPos", "val", value))?;
            set_once(&mut state.position, position, "c:dLbl/c:dLblPos")?;
        }
        b"showLegendKey" => set_once(
            &mut state.show_legend_key,
            parse_bool_value(raw, "showLegendKey")?.0,
            "c:dLbl/c:showLegendKey",
        )?,
        b"showVal" => set_once(
            &mut state.show_value,
            parse_bool_value(raw, "showVal")?.0,
            "c:dLbl/c:showVal",
        )?,
        b"showCatName" => set_once(
            &mut state.show_category_name,
            parse_bool_value(raw, "showCatName")?.0,
            "c:dLbl/c:showCatName",
        )?,
        b"showSerName" => set_once(
            &mut state.show_series_name,
            parse_bool_value(raw, "showSerName")?.0,
            "c:dLbl/c:showSerName",
        )?,
        b"showPercent" => set_once(
            &mut state.show_percent,
            parse_bool_value(raw, "showPercent")?.0,
            "c:dLbl/c:showPercent",
        )?,
        b"showBubbleSize" => set_once(
            &mut state.show_bubble_size,
            parse_bool_value(raw, "showBubbleSize")?.0,
            "c:dLbl/c:showBubbleSize",
        )?,
        b"separator" => set_once(
            &mut state.separator,
            parse_text_element(raw, b"separator", namespaces)?.0,
            "c:dLbl/c:separator",
        )?,
        _ => {}
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
struct LabelAnchor {
    center: Point,
    base: Point,
    end: Point,
    direction: Point,
}

impl LabelAnchor {
    const fn point(center: Point) -> Self {
        Self {
            center,
            base: center,
            end: center,
            direction: Point { x: 0.0, y: 0.0 },
        }
    }

    const fn segment(center: Point, base: Point, end: Point) -> Self {
        Self {
            center,
            base,
            end,
            direction: Point {
                x: end.x - base.x,
                y: end.y - base.y,
            },
        }
    }

    const fn directed_segment(center: Point, base: Point, end: Point, direction: Point) -> Self {
        Self {
            center,
            base,
            end,
            direction,
        }
    }

    fn origin(self, position: Option<DataLabelPosition>) -> Point {
        let mut origin = self.center;
        match position.unwrap_or(DataLabelPosition::BestFit) {
            DataLabelPosition::Bottom => origin.y += 8.0,
            DataLabelPosition::Top => origin.y -= 5.0,
            DataLabelPosition::Left => origin.x -= 8.0,
            DataLabelPosition::Right => origin.x += 8.0,
            DataLabelPosition::InsideBase => {
                origin = self.along_segment(self.base, 5.0, true).unwrap_or(Point {
                    x: origin.x,
                    y: origin.y + 8.0,
                });
            }
            DataLabelPosition::InsideEnd => {
                origin = self.along_segment(self.end, -5.0, true).unwrap_or(origin);
            }
            DataLabelPosition::OutsideEnd => {
                origin = self.along_segment(self.end, 5.0, false).unwrap_or(Point {
                    x: origin.x,
                    y: origin.y - 5.0,
                });
            }
            DataLabelPosition::BestFit | DataLabelPosition::Center => {}
        }
        origin
    }

    fn along_segment(self, origin: Point, distance: f64, clamp_inside: bool) -> Option<Point> {
        let dx = self.end.x - self.base.x;
        let dy = self.end.y - self.base.y;
        let length = dx.hypot(dy);
        let (dx, dy, direction_length) = if length.is_finite() && length > 0.0 {
            (dx, dy, length)
        } else {
            let direction_length = self.direction.x.hypot(self.direction.y);
            if !direction_length.is_finite() || direction_length == 0.0 {
                return None;
            }
            (self.direction.x, self.direction.y, direction_length)
        };
        let distance = if clamp_inside {
            distance.clamp(-length / 2.0, length / 2.0)
        } else {
            distance
        };
        Some(Point {
            x: origin.x + dx / direction_length * distance,
            y: origin.y + dy / direction_length * distance,
        })
    }
}

fn plot_label_anchors(
    plot: &Plot,
    bounds: Rect,
    blanks: DispBlanksAs,
    options: GeometryOptions,
) -> Result<BTreeMap<(usize, usize), LabelAnchor>> {
    let mut anchors = BTreeMap::new();
    match plot {
        Plot::Bar {
            direction,
            grouping,
            gap_width,
            overlap,
            series,
            ..
        } => {
            for bar in bar_rectangles(
                *direction, *grouping, *gap_width, *overlap, series, bounds, options,
            )? {
                if !point_in_rect(bar.end, bounds) {
                    continue;
                }
                let visible_base = Point {
                    x: bar.base.x.clamp(bounds.x, bounds.x + bounds.width),
                    y: bar.base.y.clamp(bounds.y, bounds.y + bounds.height),
                };
                let visible_end = Point {
                    x: bar.end.x.clamp(bounds.x, bounds.x + bounds.width),
                    y: bar.end.y.clamp(bounds.y, bounds.y + bounds.height),
                };
                anchors.insert(
                    (bar.series_index, bar.logical_index),
                    LabelAnchor::directed_segment(
                        Point {
                            x: (visible_base.x + visible_end.x) / 2.0,
                            y: (visible_base.y + visible_end.y) / 2.0,
                        },
                        visible_base,
                        visible_end,
                        Point {
                            x: bar.end.x - bar.base.x,
                            y: bar.end.y - bar.base.y,
                        },
                    ),
                );
            }
        }
        Plot::Line {
            grouping, series, ..
        }
        | Plot::Area {
            grouping, series, ..
        } => {
            for (series_index, points) in
                category_label_points(*grouping, series, bounds, blanks, options)?
                    .into_iter()
                    .enumerate()
            {
                for (logical_index, point) in points {
                    anchors.insert((series_index, logical_index), LabelAnchor::point(point));
                }
            }
        }
        Plot::Scatter { series, .. } => {
            let (x_domain, y_domain) = scatter_domains(plot, blanks)?;
            let x_mapping = options.scatter_x.unwrap_or(AxisMapping {
                domain: x_domain,
                orientation: Orientation::MinMax,
            });
            let y_mapping = options.scatter_y.unwrap_or(AxisMapping {
                domain: y_domain,
                orientation: Orientation::MinMax,
            });
            for (series_index, series) in series.iter().enumerate() {
                let indexed = matched_scatter_points(series)?;
                let points = scatter_points(
                    &indexed,
                    x_mapping.domain,
                    y_mapping.domain,
                    bounds,
                    x_mapping.orientation,
                    y_mapping.orientation,
                )?;
                for ((logical_index, _, _), point) in indexed.into_iter().zip(points) {
                    anchors.insert((series_index, logical_index), LabelAnchor::point(point));
                }
            }
        }
        Plot::Radar { series, .. } => {
            let center = Point {
                x: bounds.x + bounds.width / 2.0,
                y: bounds.y + bounds.height / 2.0,
            };
            let category_count = series_category_count(series)?;
            for (series_index, points) in radar_series_points(series, bounds, options)?
                .into_iter()
                .enumerate()
            {
                for (logical_index, point) in points {
                    let spoke = radial_point(
                        center,
                        1.0,
                        oriented_category_index(
                            logical_index,
                            category_count,
                            options.category_orientation,
                        ),
                        category_count,
                    );
                    anchors.insert(
                        (series_index, logical_index),
                        LabelAnchor::directed_segment(
                            point,
                            center,
                            point,
                            Point {
                                x: spoke.x - center.x,
                                y: spoke.y - center.y,
                            },
                        ),
                    );
                }
            }
        }
        Plot::Pie {
            first_slice_angle,
            series,
            ..
        } => {
            for slice in pie_slices(*first_slice_angle, None, series, bounds)? {
                anchors.insert(
                    (slice.series_index, slice.logical_index),
                    LabelAnchor::segment(slice.anchor, slice.base, slice.end),
                );
            }
        }
        Plot::Doughnut {
            first_slice_angle,
            hole_size,
            series,
            ..
        } => {
            for slice in pie_slices(*first_slice_angle, Some(*hole_size), series, bounds)? {
                anchors.insert(
                    (slice.series_index, slice.logical_index),
                    LabelAnchor::segment(slice.anchor, slice.base, slice.end),
                );
            }
        }
    }
    anchors.retain(|_, anchor| point_in_rect(anchor.end, bounds));
    Ok(anchors)
}

fn category_label_points(
    grouping: Grouping,
    series: &[Series],
    bounds: Rect,
    blanks: DispBlanksAs,
    options: GeometryOptions,
) -> Result<Vec<Vec<(usize, Point)>>> {
    validate_series_values(series)?;
    let logical_series = logical_series_values(series)?;
    let category_count = series_category_count(series)?;
    let calculated_series = if blanks == DispBlanksAs::Zero {
        zero_filled_series(&logical_series, category_count)
    } else {
        logical_series.clone()
    };
    let layers = stacked_series_bounds(
        &calculated_series,
        grouping != Grouping::Standard,
        grouping == Grouping::PercentStacked,
    )?;
    let domain = options
        .value
        .map(|mapping| mapping.domain)
        .unwrap_or(domain_from_layers(&layers)?);
    let value_orientation = options
        .value
        .map(|mapping| mapping.orientation)
        .unwrap_or(Orientation::MinMax);
    let mut point_series = Vec::with_capacity(layers.len());
    for (series_index, layer) in layers.iter().enumerate() {
        let present: BTreeSet<_> = logical_series[series_index]
            .1
            .iter()
            .map(|(index, _)| *index)
            .collect();
        let indexed: Vec<_> = layer
            .iter()
            .filter(|(index, _, _)| present.contains(index))
            .map(|(index, _, upper)| (*index, *upper))
            .collect();
        let points = category_points(
            &indexed,
            category_count,
            domain,
            bounds,
            options.category_orientation,
            value_orientation,
        )?;
        point_series.push(
            indexed
                .into_iter()
                .map(|(index, _)| index)
                .zip(points)
                .collect(),
        );
    }
    Ok(point_series)
}

fn point_in_rect(point: Point, bounds: Rect) -> bool {
    const EPSILON: f64 = 1.0e-9;
    point.x >= bounds.x - EPSILON
        && point.x <= bounds.x + bounds.width + EPSILON
        && point.y >= bounds.y - EPSILON
        && point.y <= bounds.y + bounds.height + EPSILON
}

fn resolve_series_colours(
    plot: &Plot,
    theme: &CT_OfficeStyleSheet,
    color_map: &ColorMap,
) -> Result<Vec<Color>> {
    let lookup = theme_colour_lookup(theme);
    plot_series(plot)
        .iter()
        .enumerate()
        .map(|(index, series)| {
            if let Some(properties) = series.sp_pr.as_ref() {
                if let Some(fill) = properties.fill.as_ref() {
                    return resolve_series_paint(fill, index, "fill", theme, color_map, &lookup);
                }
                if let Some(fill) = properties.line.as_ref().and_then(|line| line.fill.as_ref()) {
                    return resolve_series_paint(fill, index, "line", theme, color_map, &lookup);
                }
            }
            resolve_series_accent(index, theme, color_map, &lookup)
        })
        .collect()
}

fn resolve_series_paint(
    fill: &Fill,
    series_index: usize,
    source: &str,
    theme: &CT_OfficeStyleSheet,
    color_map: &ColorMap,
    lookup: &[(&str, RgbColor)],
) -> Result<Color> {
    match fill {
        Fill::NoFill(_) => Ok(Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        }),
        Fill::Solid(solid) => {
            let choice = solid
                .color
                .as_ref()
                .ok_or_else(|| ChartError::InvalidValue {
                    element: format!("c:ser[{series_index}] direct {source} paint"),
                    value: "solid fill has no colour choice".to_owned(),
                })?;
            resolve_direct_chart_colour(choice, theme, color_map, lookup, || {
                format!("c:ser[{series_index}] direct {source} paint")
            })
        }
        Fill::Gradient(_) => unsupported_series_paint(series_index, source, "gradient"),
        Fill::Pattern(_) => unsupported_series_paint(series_index, source, "pattern"),
        Fill::Blip(_) => unsupported_series_paint(series_index, source, "picture"),
    }
}

fn resolve_direct_chart_colour(
    choice: &ColorChoice,
    theme: &CT_OfficeStyleSheet,
    color_map: &ColorMap,
    lookup: &[(&str, RgbColor)],
    element: impl FnOnce() -> String,
) -> Result<Color> {
    let ColorChoice::Scheme {
        value, transforms, ..
    } = choice
    else {
        return resolve_chart_colour(choice, color_map, lookup, element);
    };
    let theme_slot =
        mapped_theme_slot(value, color_map).ok_or_else(|| ChartError::InvalidValue {
            element: element(),
            value: format!("no concrete theme slot is available for scheme colour {value}"),
        })?;
    let theme_choice = theme.theme_elements.color_scheme.color(theme_slot);
    let base = concrete_theme_colour(theme_choice).ok_or_else(|| ChartError::InvalidValue {
        element: format!("a:clrScheme/a:{}", theme_slot.as_str()),
        value: "selected theme slot has no concrete sRGB or system fallback colour".to_owned(),
    })?;
    let mut combined = theme_choice.transforms().to_vec();
    combined.extend_from_slice(transforms);
    Ok(resolved_layout_colour(apply_color_transforms(
        base, &combined,
    )))
}

fn mapped_theme_slot(value: &str, color_map: &ColorMap) -> Option<ThemeColorSlot> {
    let mapped = match value {
        "bg1" => color_map.theme_slot(ColorMapSlot::Background1),
        "tx1" => color_map.theme_slot(ColorMapSlot::Text1),
        "bg2" => color_map.theme_slot(ColorMapSlot::Background2),
        "tx2" => color_map.theme_slot(ColorMapSlot::Text2),
        "accent1" => color_map.theme_slot(ColorMapSlot::Accent1),
        "accent2" => color_map.theme_slot(ColorMapSlot::Accent2),
        "accent3" => color_map.theme_slot(ColorMapSlot::Accent3),
        "accent4" => color_map.theme_slot(ColorMapSlot::Accent4),
        "accent5" => color_map.theme_slot(ColorMapSlot::Accent5),
        "accent6" => color_map.theme_slot(ColorMapSlot::Accent6),
        "hlink" => color_map.theme_slot(ColorMapSlot::Hyperlink),
        "folHlink" => color_map.theme_slot(ColorMapSlot::FollowedHyperlink),
        "dk1" => ThemeColorSlot::Dark1,
        "lt1" => ThemeColorSlot::Light1,
        "dk2" => ThemeColorSlot::Dark2,
        "lt2" => ThemeColorSlot::Light2,
        _ => return None,
    };
    Some(mapped)
}

fn unsupported_series_paint(series_index: usize, source: &str, kind: &str) -> Result<Color> {
    Err(ChartError::InvalidValue {
        element: format!("c:ser[{series_index}] direct {source} paint"),
        value: format!("unsupported {kind} paint"),
    })
}

fn resolve_series_accent(
    series_index: usize,
    theme: &CT_OfficeStyleSheet,
    color_map: &ColorMap,
    lookup: &[(&str, RgbColor)],
) -> Result<Color> {
    const ACCENTS: [ColorMapSlot; 6] = [
        ColorMapSlot::Accent1,
        ColorMapSlot::Accent2,
        ColorMapSlot::Accent3,
        ColorMapSlot::Accent4,
        ColorMapSlot::Accent5,
        ColorMapSlot::Accent6,
    ];
    let semantic_slot = ACCENTS[series_index % ACCENTS.len()];
    let theme_slot = color_map.theme_slot(semantic_slot);
    let choice = theme.theme_elements.color_scheme.color(theme_slot);
    resolve_chart_colour(choice, color_map, lookup, || {
        format!("c:ser[{series_index}] theme accent")
    })
}

fn resolve_chart_colour(
    choice: &ColorChoice,
    color_map: &ColorMap,
    lookup: &[(&str, RgbColor)],
    element: impl FnOnce() -> String,
) -> Result<Color> {
    resolve_color(choice, color_map, lookup)
        .map(resolved_layout_colour)
        .map_err(|error| ChartError::InvalidValue {
            element: element(),
            value: error.to_string(),
        })
}

fn theme_colour_lookup(theme: &CT_OfficeStyleSheet) -> Vec<(&'static str, RgbColor)> {
    theme
        .theme_elements
        .color_scheme
        .iter()
        .filter_map(|(slot, choice)| {
            concrete_theme_colour(choice).map(|colour| (slot.as_str(), colour))
        })
        .collect()
}

fn concrete_theme_colour(choice: &ColorChoice) -> Option<RgbColor> {
    match choice {
        ColorChoice::Srgb { value, .. } => Some(*value),
        ColorChoice::System { last_color, .. } => *last_color,
        ColorChoice::Scheme { .. } | ColorChoice::Preset { .. } => None,
    }
}

fn resolved_layout_colour(colour: ResolvedColor) -> Color {
    Color {
        r: f64::from(colour.red) / 255.0,
        g: f64::from(colour.green) / 255.0,
        b: f64::from(colour.blue) / 255.0,
        a: f64::from(colour.alpha) / 255.0,
    }
}

struct LegendLayout {
    plot: Rect,
    labels: Vec<GlyphRun>,
    keys: Vec<Rect>,
}

fn layout_legend(
    chart: &CT_Chart,
    plot: &Plot,
    bounds: Rect,
    fonts: &mut FontManager,
) -> Result<LegendLayout> {
    let full_plot = geometry_plot_bounds(bounds)?;
    let Some(legend) = &chart.legend else {
        return Ok(LegendLayout {
            plot: full_plot,
            labels: Vec::new(),
            keys: Vec::new(),
        });
    };
    let settings = legend.settings;
    let horizontal = matches!(
        settings.position,
        LegendPosition::Top | LegendPosition::Bottom
    );
    if plot_series(plot).len() > MAX_CATEGORY_ANNOTATIONS {
        return Err(invalid_authoring_value("legend entry limit"));
    }
    let mut labels = plot_series(plot)
        .iter()
        .enumerate()
        .map(|(index, series)| {
            shape_label(fonts, &series_name(series, index), Point { x: 0.0, y: 0.0 })
        })
        .collect::<Result<Vec<_>>>()?;
    let available = if settings.overlay { full_plot } else { bounds };
    let capacity = available.width - 12.0;
    // Frozen native Word/PowerPoint 9 point legend controls qualify these
    // nominal-font metrics within 1 point and the unchanged cropped paint gate.
    let row_height = labels.first().map_or(0.0, |label| label.font_size * 1.875);
    let mut positions = Vec::with_capacity(labels.len());
    let mut width: f64 = 0.0;
    let mut x = 0.0;
    let mut y = 0.0;
    for label in &labels {
        let entry_width = label.advances.iter().sum::<f64>() + label.font_size * 7.0 / 9.0 + 4.0;
        if !entry_width.is_finite() || entry_width > capacity {
            return Err(invalid_authoring_value("legend label exceeds chart width"));
        }
        if horizontal && x > 0.0 && x + entry_width > capacity {
            x = 0.0;
            y += row_height;
        }
        positions.push(Point { x, y });
        width = width.max(x + entry_width);
        if horizontal {
            x += entry_width + 4.0;
        } else {
            y += row_height;
        }
    }
    let height = if labels.is_empty() {
        0.0
    } else if horizontal {
        y + row_height
    } else {
        y
    };
    if height > available.height - 12.0 {
        return Err(invalid_authoring_value(
            "legend entries exceed chart height",
        ));
    }
    let mut content = bounds;
    if !settings.overlay && !labels.is_empty() {
        match settings.position {
            LegendPosition::Right | LegendPosition::TopRight => content.width -= width + 10.0,
            LegendPosition::Left => {
                content.x += width + 10.0;
                content.width -= width + 10.0;
            }
            LegendPosition::Top => {
                content.y += height + 10.0;
                content.height -= height + 10.0;
            }
            LegendPosition::Bottom => content.height -= height + 10.0,
        }
    }
    let plot_bounds = geometry_plot_bounds(content)?;
    let area = if settings.overlay {
        plot_bounds
    } else {
        bounds
    };
    let origin = match settings.position {
        LegendPosition::Right => Point {
            x: area.x + area.width - width - 6.0,
            y: area.y + (area.height - height) / 2.0,
        },
        LegendPosition::TopRight => Point {
            x: area.x + area.width - width - 6.0,
            y: area.y + 6.0,
        },
        LegendPosition::Left => Point {
            x: area.x + 6.0,
            y: area.y + (area.height - height) / 2.0,
        },
        LegendPosition::Top => Point {
            x: area.x + (area.width - width) / 2.0,
            y: area.y + 6.0,
        },
        LegendPosition::Bottom => Point {
            x: area.x + (area.width - width) / 2.0,
            y: area.y + area.height - height - 6.0,
        },
    };
    let mut keys = Vec::with_capacity(labels.len());
    for (label, offset) in labels.iter_mut().zip(positions) {
        let x = origin.x + offset.x;
        let y = origin.y + offset.y;
        keys.push(Rect {
            x,
            y: y + (row_height - label.font_size * 5.0 / 9.0) / 2.0,
            width: label.font_size * 5.0 / 9.0,
            height: label.font_size * 5.0 / 9.0,
        });
        label.origin = Point {
            x: x + label.font_size * 7.0 / 9.0,
            y: y + label.font_size * 7.0 / 6.0,
        };
    }
    Ok(LegendLayout {
        plot: plot_bounds,
        labels,
        keys,
    })
}

fn render_legend(
    layout: LegendLayout,
    series_colours: &[Color],
    paths: &mut Vec<PositionedElement>,
    labels: &mut Vec<PositionedElement>,
) {
    for (key, colour) in layout.keys.into_iter().zip(series_colours) {
        paths.push(filled_path(Path::rect(key), *colour));
    }
    labels.extend(layout.labels.into_iter().map(PositionedElement::Text));
}

fn validate_geometry_coordinates(elements: &[PositionedElement]) -> Result<()> {
    fn finite_point(point: Point) -> bool {
        point.x.is_finite() && point.y.is_finite()
    }

    for element in elements {
        match element {
            PositionedElement::Path(path) => {
                for command in &path.path.commands {
                    let finite = match command {
                        PathCommand::MoveTo(point) | PathCommand::LineTo(point) => {
                            finite_point(*point)
                        }
                        PathCommand::CurveTo { c1, c2, to } => {
                            finite_point(*c1) && finite_point(*c2) && finite_point(*to)
                        }
                        PathCommand::Close => true,
                    };
                    if !finite {
                        return Err(ChartError::InvalidValue {
                            element: "chart geometry coordinate".to_owned(),
                            value: "generated path contains a nonfinite point".to_owned(),
                        });
                    }
                }
            }
            PositionedElement::Group(group) => validate_geometry_coordinates(&group.children)?,
            _ => {}
        }
    }
    Ok(())
}

fn render_plot_geometry(
    plot: &Plot,
    bounds: Rect,
    blanks: DispBlanksAs,
    options: GeometryOptions,
    series_colours: &[Color],
) -> Result<Vec<PositionedElement>> {
    match plot {
        Plot::Bar {
            direction,
            grouping,
            gap_width,
            overlap,
            series,
            ..
        } => render_bar_geometry(
            *direction,
            *grouping,
            *gap_width,
            *overlap,
            series,
            bounds,
            options,
            series_colours,
        ),
        Plot::Line {
            grouping,
            marker,
            series,
            ..
        } => render_line_geometry(
            *grouping,
            *marker,
            series,
            bounds,
            blanks,
            options,
            series_colours,
        ),
        Plot::Pie {
            first_slice_angle,
            series,
            ..
        } => render_pie_geometry(*first_slice_angle, None, series, bounds, series_colours),
        Plot::Doughnut {
            first_slice_angle,
            hole_size,
            series,
            ..
        } => render_pie_geometry(
            *first_slice_angle,
            Some(*hole_size),
            series,
            bounds,
            series_colours,
        ),
        Plot::Area {
            grouping, series, ..
        } => render_area_geometry(*grouping, series, bounds, blanks, options, series_colours),
        Plot::Scatter { style, series, .. } => {
            render_scatter_geometry(*style, series, bounds, blanks, options, series_colours)
        }
        Plot::Radar { style, series, .. } => {
            render_radar_geometry(*style, series, bounds, options, series_colours)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_bar_geometry(
    direction: BarDirection,
    grouping: BarGrouping,
    gap_width: u16,
    overlap: i8,
    series: &[Series],
    bounds: Rect,
    options: GeometryOptions,
    series_colours: &[Color],
) -> Result<Vec<PositionedElement>> {
    let bars = bar_rectangles(
        direction, grouping, gap_width, overlap, series, bounds, options,
    )?;
    bars.into_iter()
        .map(|bar| {
            Ok(filled_path(
                Path::rect(bar.rect),
                series_colour(series_colours, bar.series_index)?,
            ))
        })
        .collect()
}

#[derive(Clone, Copy, Debug)]
struct BarRectangle {
    series_index: usize,
    logical_index: usize,
    rect: Rect,
    base: Point,
    end: Point,
}

fn bar_rectangles(
    direction: BarDirection,
    grouping: BarGrouping,
    gap_width: u16,
    overlap: i8,
    series: &[Series],
    bounds: Rect,
    options: GeometryOptions,
) -> Result<Vec<BarRectangle>> {
    validate_series_values(series)?;
    let logical_series = logical_series_values(series)?;
    let category_count = series_category_count(series)?;
    let stacked = matches!(grouping, BarGrouping::Stacked | BarGrouping::PercentStacked);
    let percent = grouping == BarGrouping::PercentStacked;
    let layers = stacked_series_bounds(&logical_series, stacked, percent)?;
    let domain = options
        .value
        .map(|mapping| mapping.domain)
        .unwrap_or(domain_from_layers(&layers)?);
    let value_orientation = options
        .value
        .map(|mapping| mapping.orientation)
        .unwrap_or(Orientation::MinMax);
    let category_extent = if direction == BarDirection::Column {
        bounds.width
    } else {
        bounds.height
    };
    let slot = category_extent / category_count as f64;
    let cluster_extent = slot * 100.0 / (100.0 + f64::from(gap_width));
    let bars_per_cluster = if stacked { 1 } else { series.len() };
    let overlap_fraction = if stacked {
        0.0
    } else {
        f64::from(overlap) / 100.0
    };
    let divisor =
        bars_per_cluster as f64 - (bars_per_cluster.saturating_sub(1)) as f64 * overlap_fraction;
    let bar_extent = cluster_extent / divisor;
    let advance = bar_extent * (1.0 - overlap_fraction);
    let mut bars = Vec::new();
    for (series_index, layer) in layers.iter().enumerate() {
        for &(logical_index, start, end) in layer {
            let category = oriented_category_index(
                logical_index,
                category_count,
                options.category_orientation,
            );
            let cluster_start = category as f64 / category_count as f64 * category_extent
                + (slot - cluster_extent) / 2.0;
            let category_start = if stacked {
                cluster_start
            } else {
                cluster_start + series_index as f64 * advance
            };
            let (rect, base, end) = if direction == BarDirection::Column {
                let y0 = map_y_oriented(start, domain, bounds, value_orientation)?;
                let y1 = map_y_oriented(end, domain, bounds, value_orientation)?;
                let center_x = bounds.x + category_start + bar_extent / 2.0;
                (
                    Rect {
                        x: bounds.x + category_start,
                        y: y0.min(y1),
                        width: bar_extent,
                        height: (y1 - y0).abs(),
                    },
                    Point { x: center_x, y: y0 },
                    Point { x: center_x, y: y1 },
                )
            } else {
                let x0 = map_x_oriented(start, domain, bounds, value_orientation)?;
                let x1 = map_x_oriented(end, domain, bounds, value_orientation)?;
                let center_y = bounds.y + category_start + bar_extent / 2.0;
                (
                    Rect {
                        x: x0.min(x1),
                        y: bounds.y + category_start,
                        width: (x1 - x0).abs(),
                        height: bar_extent,
                    },
                    Point { x: x0, y: center_y },
                    Point { x: x1, y: center_y },
                )
            };
            bars.push(BarRectangle {
                series_index,
                logical_index,
                rect,
                base,
                end,
            });
        }
    }
    Ok(bars)
}

fn render_line_geometry(
    grouping: Grouping,
    marker: bool,
    series: &[Series],
    bounds: Rect,
    blanks: DispBlanksAs,
    options: GeometryOptions,
    series_colours: &[Color],
) -> Result<Vec<PositionedElement>> {
    validate_series_values(series)?;
    let logical_series = logical_series_values(series)?;
    let category_count = series_category_count(series)?;
    let calculated_series = if blanks == DispBlanksAs::Zero {
        zero_filled_series(&logical_series, category_count)
    } else {
        logical_series.clone()
    };
    let stacked = grouping != Grouping::Standard;
    let percent = grouping == Grouping::PercentStacked;
    let layers = stacked_series_bounds(&calculated_series, stacked, percent)?;
    let domain = options
        .value
        .map(|mapping| mapping.domain)
        .unwrap_or(domain_from_layers(&layers)?);
    let value_orientation = options
        .value
        .map(|mapping| mapping.orientation)
        .unwrap_or(Orientation::MinMax);
    let mut elements = Vec::new();
    for (series_index, layer) in layers.iter().enumerate() {
        let colour = series_colour(series_colours, series_index)?;
        let indexed: Vec<_> = layer
            .iter()
            .map(|(index, _, upper)| (*index, *upper))
            .collect();
        let indexes: Vec<_> = indexed.iter().map(|(index, _)| *index).collect();
        for (start, end) in contiguous_ranges(&indexes, blanks) {
            let points = category_points(
                &indexed[start..end],
                category_count,
                domain,
                bounds,
                options.category_orientation,
                value_orientation,
            )?;
            elements.push(stroked_path(polyline_path(&points, false), colour));
        }
        if marker {
            let present: BTreeSet<_> = logical_series[series_index]
                .1
                .iter()
                .map(|(index, _)| *index)
                .collect();
            let marker_values: Vec<_> = indexed
                .iter()
                .copied()
                .filter(|(index, _)| present.contains(index))
                .collect();
            let marker_points = category_points(
                &marker_values,
                category_count,
                domain,
                bounds,
                options.category_orientation,
                value_orientation,
            )?;
            push_markers(&mut elements, &marker_points, colour);
        }
    }
    Ok(elements)
}

fn render_area_geometry(
    grouping: Grouping,
    series: &[Series],
    bounds: Rect,
    blanks: DispBlanksAs,
    options: GeometryOptions,
    series_colours: &[Color],
) -> Result<Vec<PositionedElement>> {
    validate_series_values(series)?;
    let logical_series = logical_series_values(series)?;
    let category_count = series_category_count(series)?;
    let calculated_series = if blanks == DispBlanksAs::Zero {
        zero_filled_series(&logical_series, category_count)
    } else {
        logical_series
    };
    let stacked = grouping != Grouping::Standard;
    let percent = grouping == Grouping::PercentStacked;
    let layers = stacked_series_bounds(&calculated_series, stacked, percent)?;
    let domain = options
        .value
        .map(|mapping| mapping.domain)
        .unwrap_or(domain_from_layers(&layers)?);
    let value_orientation = options
        .value
        .map(|mapping| mapping.orientation)
        .unwrap_or(Orientation::MinMax);
    let mut elements = Vec::new();
    for (series_index, layer) in layers.iter().enumerate() {
        let colour = series_colour(series_colours, series_index)?;
        let indexes: Vec<_> = layer.iter().map(|(index, _, _)| *index).collect();
        for (start, end) in contiguous_ranges(&indexes, blanks) {
            let segment = &layer[start..end];
            let top_values: Vec<_> = segment
                .iter()
                .map(|(index, _, upper)| (*index, *upper))
                .collect();
            let lower_values: Vec<_> = segment
                .iter()
                .map(|(index, lower, _)| (*index, *lower))
                .collect();
            let top = category_points(
                &top_values,
                category_count,
                domain,
                bounds,
                options.category_orientation,
                value_orientation,
            )?;
            let mut bottom = category_points(
                &lower_values,
                category_count,
                domain,
                bounds,
                options.category_orientation,
                value_orientation,
            )?;
            bottom.reverse();
            let mut commands = Vec::with_capacity(top.len() + bottom.len() + 2);
            commands.push(PathCommand::MoveTo(top[0]));
            commands.extend(top.iter().skip(1).copied().map(PathCommand::LineTo));
            commands.extend(bottom.into_iter().map(PathCommand::LineTo));
            commands.push(PathCommand::Close);
            elements.push(filled_and_stroked_path(
                Path {
                    commands,
                    fill_rule: FillRule::NonZero,
                },
                colour,
            ));
        }
    }
    Ok(elements)
}

fn render_scatter_geometry(
    style: ScatterStyle,
    series: &[Series],
    bounds: Rect,
    blanks: DispBlanksAs,
    options: GeometryOptions,
    series_colours: &[Color],
) -> Result<Vec<PositionedElement>> {
    validate_series_values(series)?;
    let mut paired_series = Vec::with_capacity(series.len());
    let mut marker_series = Vec::with_capacity(series.len());
    let mut all_x = Vec::new();
    let mut all_y = Vec::new();
    for item in series {
        let markers = matched_scatter_points(item)?;
        let paired = scatter_render_points(item, blanks)?;
        all_x.extend(paired.iter().map(|(_, x, _)| *x));
        all_y.extend(paired.iter().map(|(_, _, y)| *y));
        paired_series.push(paired);
        marker_series.push(markers);
    }
    let x_domain = options
        .scatter_x
        .map(|mapping| mapping.domain)
        .unwrap_or(data_domain(&all_x, false)?);
    let y_domain = options
        .scatter_y
        .map(|mapping| mapping.domain)
        .unwrap_or(data_domain(&all_y, false)?);
    let x_orientation = options
        .scatter_x
        .map(|mapping| mapping.orientation)
        .unwrap_or(Orientation::MinMax);
    let y_orientation = options
        .scatter_y
        .map(|mapping| mapping.orientation)
        .unwrap_or(Orientation::MinMax);
    let draw_line = matches!(
        style,
        ScatterStyle::Line
            | ScatterStyle::LineMarker
            | ScatterStyle::Smooth
            | ScatterStyle::SmoothMarker
    );
    let draw_marker = matches!(
        style,
        ScatterStyle::Marker | ScatterStyle::LineMarker | ScatterStyle::SmoothMarker
    );
    let mut elements = Vec::new();
    for (series_index, paired) in paired_series.iter().enumerate() {
        let colour = series_colour(series_colours, series_index)?;
        if draw_line {
            let indexes: Vec<_> = paired.iter().map(|(index, _, _)| *index).collect();
            for (start, end) in contiguous_ranges(&indexes, blanks) {
                let points = scatter_points(
                    &paired[start..end],
                    x_domain,
                    y_domain,
                    bounds,
                    x_orientation,
                    y_orientation,
                )?;
                elements.push(stroked_path(polyline_path(&points, false), colour));
            }
        }
        if draw_marker {
            let marker_points = scatter_points(
                &marker_series[series_index],
                x_domain,
                y_domain,
                bounds,
                x_orientation,
                y_orientation,
            )?;
            push_markers(&mut elements, &marker_points, colour);
        }
    }
    Ok(elements)
}

type ScatterPoint = (usize, f64, f64);

struct ScatterPointMaps {
    x_count: usize,
    y_count: usize,
    x_by_index: BTreeMap<usize, f64>,
    y_by_index: BTreeMap<usize, f64>,
}

fn scatter_point_maps(series: &Series) -> Result<ScatterPointMaps> {
    let Some(AxisData::Numeric(x_values)) = &series.categories else {
        return Err(ChartError::InvalidValue {
            element: "c:xVal".to_owned(),
            value: "scatter series requires numeric cached x values".to_owned(),
        });
    };
    validate_finite(&x_values.values, "c:xVal/c:numCache")?;
    let (x_count, x_points) = logical_numeric_values(x_values)?;
    let (y_count, y_points) = logical_numeric_values(&series.values)?;
    Ok(ScatterPointMaps {
        x_count,
        y_count,
        x_by_index: x_points.into_iter().collect(),
        y_by_index: y_points.into_iter().collect(),
    })
}

fn matched_scatter_points(series: &Series) -> Result<Vec<ScatterPoint>> {
    let points = scatter_point_maps(series)?;
    Ok(points
        .y_by_index
        .iter()
        .filter_map(|(index, y)| points.x_by_index.get(index).map(|x| (*index, *x, *y)))
        .collect())
}

fn scatter_render_points(series: &Series, blanks: DispBlanksAs) -> Result<Vec<ScatterPoint>> {
    let points = scatter_point_maps(series)?;
    if blanks != DispBlanksAs::Zero {
        return Ok(points
            .y_by_index
            .iter()
            .filter_map(|(index, y)| points.x_by_index.get(index).map(|x| (*index, *x, *y)))
            .collect());
    }
    let count = points.x_count.max(points.y_count);
    let present = points
        .x_by_index
        .keys()
        .chain(points.y_by_index.keys())
        .copied()
        .collect();
    Ok(zero_control_indexes(count, &present)
        .into_iter()
        .map(|index| {
            (
                index,
                points.x_by_index.get(&index).copied().unwrap_or(0.0),
                points.y_by_index.get(&index).copied().unwrap_or(0.0),
            )
        })
        .collect())
}

fn scatter_points(
    values: &[(usize, f64, f64)],
    x_domain: Domain,
    y_domain: Domain,
    bounds: Rect,
    x_orientation: Orientation,
    y_orientation: Orientation,
) -> Result<Vec<Point>> {
    values
        .iter()
        .map(|(_, x, y)| {
            Ok(Point {
                x: map_x_oriented(*x, x_domain, bounds, x_orientation)?,
                y: map_y_oriented(*y, y_domain, bounds, y_orientation)?,
            })
        })
        .collect()
}

fn render_radar_geometry(
    style: RadarStyle,
    series: &[Series],
    bounds: Rect,
    options: GeometryOptions,
    series_colours: &[Color],
) -> Result<Vec<PositionedElement>> {
    let point_series = radar_series_points(series, bounds, options)?;
    let mut elements = Vec::new();
    for (series_index, points) in point_series.iter().enumerate() {
        let colour = series_colour(series_colours, series_index)?;
        let plain_points: Vec<_> = points.iter().map(|(_, point)| *point).collect();
        if plain_points.is_empty() {
            continue;
        }
        let path = polyline_path(&plain_points, true);
        if style == RadarStyle::Filled {
            elements.push(filled_and_stroked_path(path, colour));
        } else {
            elements.push(stroked_path(path, colour));
        }
        if style == RadarStyle::Marker {
            push_markers(&mut elements, &plain_points, colour);
        }
    }
    Ok(elements)
}

fn radar_series_points(
    series: &[Series],
    bounds: Rect,
    options: GeometryOptions,
) -> Result<Vec<Vec<(usize, Point)>>> {
    validate_series_values(series)?;
    let logical_series = logical_series_values(series)?;
    let category_count = series_category_count(series)?;
    let values = logical_series
        .iter()
        .flat_map(|(_, points)| points.iter().map(|(_, value)| *value))
        .collect::<Vec<_>>();
    let mapping = options.value.unwrap_or(AxisMapping {
        domain: data_domain(&values, true)?,
        orientation: Orientation::MinMax,
    });
    let center = Point {
        x: bounds.x + bounds.width / 2.0,
        y: bounds.y + bounds.height / 2.0,
    };
    let radius = bounds.width.min(bounds.height) / 2.0;
    let mut point_series = Vec::with_capacity(logical_series.len());
    for (_, logical) in &logical_series {
        let mut points = Vec::with_capacity(logical.len());
        for (category, value) in logical {
            if *value < mapping.domain.min || *value > mapping.domain.max {
                continue;
            }
            points.push((
                *category,
                radial_point(
                    center,
                    normalized_value_oriented(*value, mapping.domain, mapping.orientation)?
                        * radius,
                    oriented_category_index(
                        *category,
                        category_count,
                        options.category_orientation,
                    ),
                    category_count,
                ),
            ));
        }
        point_series.push(points);
    }
    Ok(point_series)
}

fn render_pie_geometry(
    first_slice_angle: u16,
    hole_size: Option<u8>,
    series: &[Series],
    bounds: Rect,
    series_colours: &[Color],
) -> Result<Vec<PositionedElement>> {
    pie_slices(first_slice_angle, hole_size, series, bounds)?
        .into_iter()
        .map(|slice| {
            Ok(filled_path(
                slice.path,
                series_colour(series_colours, slice.series_index)?,
            ))
        })
        .collect()
}

struct PieSliceGeometry {
    series_index: usize,
    logical_index: usize,
    path: Path,
    anchor: Point,
    base: Point,
    end: Point,
}

fn pie_slices(
    first_slice_angle: u16,
    hole_size: Option<u8>,
    series: &[Series],
    bounds: Rect,
) -> Result<Vec<PieSliceGeometry>> {
    validate_series_values(series)?;
    let center = Point {
        x: bounds.x + bounds.width / 2.0,
        y: bounds.y + bounds.height / 2.0,
    };
    let radius = bounds.width.min(bounds.height) / 2.0;
    let mut elements = Vec::new();
    for (series_index, item) in series.iter().enumerate() {
        let (_, logical) = logical_numeric_values(&item.values)?;
        let total = logical
            .iter()
            .map(|(_, value)| *value)
            .filter(|value| *value > 0.0)
            .try_fold(0.0, |total, value| {
                checked_geometry_sum(total, value, "pie value total")
            })?;
        if total <= 0.0 {
            continue;
        }
        let mut angle = f64::from(first_slice_angle).to_radians() - std::f64::consts::FRAC_PI_2;
        for (logical_index, value) in logical {
            if value <= 0.0 {
                continue;
            }
            let next = angle + value / total * std::f64::consts::TAU;
            let path = if let Some(hole_size) = hole_size {
                doughnut_wedge(
                    center,
                    radius,
                    radius * f64::from(hole_size) / 100.0,
                    angle,
                    next,
                )
            } else {
                pie_wedge(center, radius, angle, next)
            };
            let inner_radius = hole_size
                .map(|hole| radius * f64::from(hole) / 100.0)
                .unwrap_or(0.0);
            let anchor_radius = hole_size
                .map(|_| (radius + inner_radius) / 2.0)
                .unwrap_or(radius * 2.0 / 3.0);
            let midpoint = (angle + next) / 2.0;
            elements.push(PieSliceGeometry {
                series_index,
                logical_index,
                path,
                anchor: circle_point(center, anchor_radius, midpoint),
                base: circle_point(center, inner_radius, midpoint),
                end: circle_point(center, radius, midpoint),
            });
            angle = next;
        }
    }
    Ok(elements)
}

fn validate_series_values(series: &[Series]) -> Result<()> {
    if series.is_empty() {
        return Err(ChartError::MissingElement("c:ser".to_owned()));
    }
    for item in series {
        validate_finite(&item.values.values, "c:val/c:numCache")?;
    }
    Ok(())
}

fn validate_finite(values: &[f64], element: &str) -> Result<()> {
    if let Some(value) = values.iter().find(|value| !value.is_finite()) {
        return Err(ChartError::InvalidValue {
            element: element.to_owned(),
            value: value.to_string(),
        });
    }
    Ok(())
}

type IndexedValues = Vec<(usize, f64)>;
type LogicalSeries = (usize, IndexedValues);
type GeometryLayer = Vec<(usize, f64, f64)>;

fn zero_control_indexes(count: usize, present: &BTreeSet<usize>) -> BTreeSet<usize> {
    let mut controls = present.clone();
    if count == 0 {
        return controls;
    }
    controls.insert(0);
    controls.insert(count - 1);
    for index in present {
        if *index > 0 {
            controls.insert(*index - 1);
        }
        if index + 1 < count {
            controls.insert(index + 1);
        }
    }
    controls
}

fn zero_filled_series(series: &[LogicalSeries], count: usize) -> Vec<LogicalSeries> {
    let present = series
        .iter()
        .flat_map(|(_, points)| points.iter().map(|(index, _)| *index))
        .collect();
    let controls = zero_control_indexes(count, &present);
    series
        .iter()
        .map(|(declared, points)| {
            let values: BTreeMap<_, _> = points.iter().copied().collect();
            let filled = controls
                .iter()
                .map(|index| (*index, values.get(index).copied().unwrap_or(0.0)))
                .collect();
            (*declared, filled)
        })
        .collect()
}

fn contiguous_ranges(indexes: &[usize], blanks: DispBlanksAs) -> Vec<(usize, usize)> {
    if indexes.is_empty() {
        return Vec::new();
    }
    if blanks != DispBlanksAs::Gap {
        return vec![(0, indexes.len())];
    }
    let mut ranges = Vec::new();
    let mut start = 0usize;
    for position in 1..indexes.len() {
        if indexes[position - 1].checked_add(1) != Some(indexes[position]) {
            ranges.push((start, position));
            start = position;
        }
    }
    ranges.push((start, indexes.len()));
    ranges
}

fn logical_numeric_values(data: &NumericData) -> Result<LogicalSeries> {
    validate_finite(&data.values, "c:numCache")?;
    let (declared, indexes) = cache_layout(&data.markup, data.values.len())?;
    let declared = usize::try_from(declared).map_err(|_| ChartError::InvalidValue {
        element: "c:ptCount".to_owned(),
        value: declared.to_string(),
    })?;
    let points = indexes
        .into_iter()
        .zip(data.values.iter().copied())
        .map(|(index, value)| {
            usize::try_from(index)
                .map(|index| (index, value))
                .map_err(|_| ChartError::InvalidValue {
                    element: "c:pt/@idx".to_owned(),
                    value: index.to_string(),
                })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((declared, points))
}

fn logical_series_values(series: &[Series]) -> Result<Vec<LogicalSeries>> {
    series
        .iter()
        .map(|item| logical_numeric_values(&item.values))
        .collect()
}

fn reference_logical_count(markup: &ReferenceMarkup, value_count: usize) -> Result<usize> {
    let (declared, _) = cache_layout(markup, value_count)?;
    usize::try_from(declared).map_err(|_| ChartError::InvalidValue {
        element: "c:ptCount".to_owned(),
        value: declared.to_string(),
    })
}

fn series_category_count(series: &[Series]) -> Result<usize> {
    let mut count = 0usize;
    for item in series {
        count = count.max(reference_logical_count(
            &item.values.markup,
            item.values.values.len(),
        )?);
        if let Some(categories) = &item.categories {
            let category_count = match categories {
                AxisData::String(data) => reference_logical_count(&data.markup, data.values.len())?,
                AxisData::Numeric(data) => {
                    reference_logical_count(&data.markup, data.values.len())?
                }
            };
            count = count.max(category_count);
        }
    }
    if count == 0 {
        return Err(ChartError::InvalidValue {
            element: "c:ser".to_owned(),
            value: "cached series contain no points".to_owned(),
        });
    }
    Ok(count)
}

fn stacked_series_bounds(
    series: &[LogicalSeries],
    stacked: bool,
    percent: bool,
) -> Result<Vec<GeometryLayer>> {
    let mut positive_totals = BTreeMap::<usize, f64>::new();
    let mut negative_totals = BTreeMap::<usize, f64>::new();
    if percent {
        for (_, points) in series {
            for &(index, value) in points {
                let totals = if value >= 0.0 {
                    &mut positive_totals
                } else {
                    &mut negative_totals
                };
                let total = totals.entry(index).or_default();
                *total = checked_geometry_sum(*total, value.abs(), "stacked percentage total")?;
            }
        }
    }
    let mut positive = BTreeMap::<usize, f64>::new();
    let mut negative = BTreeMap::<usize, f64>::new();
    let mut layers = Vec::with_capacity(series.len());
    for (_, points) in series {
        let mut layer = Vec::with_capacity(points.len());
        for &(index, original) in points {
            let total = if original >= 0.0 {
                positive_totals.get(&index).copied().unwrap_or(0.0)
            } else {
                negative_totals.get(&index).copied().unwrap_or(0.0)
            };
            let value = if percent && total != 0.0 {
                original / total
            } else {
                original
            };
            if stacked {
                let accumulators = if value >= 0.0 {
                    &mut positive
                } else {
                    &mut negative
                };
                let accumulator = accumulators.entry(index).or_default();
                let lower = *accumulator;
                *accumulator = checked_geometry_sum(*accumulator, value, "stacked value total")?;
                layer.push((index, lower, *accumulator));
            } else {
                layer.push((index, 0.0, value));
            }
        }
        layers.push(layer);
    }
    Ok(layers)
}

fn checked_geometry_sum(current: f64, value: f64, context: &str) -> Result<f64> {
    let sum = current + value;
    if sum.is_finite() {
        Ok(sum)
    } else {
        Err(ChartError::InvalidValue {
            element: "chart geometry aggregate".to_owned(),
            value: format!("{context} is nonfinite"),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Domain {
    min: f64,
    max: f64,
}

fn nice_number_scale(
    extent: Domain,
    pinned_minimum: Option<f64>,
    pinned_maximum: Option<f64>,
    target_tick_count: usize,
) -> Result<AxisScale> {
    if target_tick_count < 2
        || !extent.min.is_finite()
        || !extent.max.is_finite()
        || pinned_minimum.is_some_and(|value| !value.is_finite())
        || pinned_maximum.is_some_and(|value| !value.is_finite())
    {
        return Err(ChartError::InvalidValue {
            element: "chart axis scale".to_owned(),
            value: "scale bounds and target tick count must be finite and valid".to_owned(),
        });
    }

    let mut data_min = extent.min.min(extent.max);
    let mut data_max = extent.min.max(extent.max);
    if data_min == data_max {
        let expanded = expand_constant_domain(data_min)?;
        data_min = expanded.min;
        data_max = expanded.max;
    }
    let requested_min = match (pinned_minimum, pinned_maximum) {
        (Some(minimum), _) => minimum,
        (None, Some(maximum)) if data_min >= maximum => {
            extend_bound(maximum, stable_positive_step(data_min, data_max, 3)?, false)?
        }
        (None, _) => data_min,
    };
    let requested_max = match (pinned_minimum, pinned_maximum) {
        (_, Some(maximum)) => maximum,
        (Some(minimum), None) if data_max <= minimum => {
            extend_bound(minimum, stable_positive_step(data_min, data_max, 3)?, true)?
        }
        (_, None) => data_max,
    };
    if requested_min >= requested_max {
        return Err(ChartError::InvalidValue {
            element: "c:scaling".to_owned(),
            value: format!("minimum {requested_min} is not less than maximum {requested_max}"),
        });
    }

    let raw_step = stable_positive_step(requested_min, requested_max, target_tick_count)?;
    let step = nice_step(raw_step)?;
    let rounded_minimum = (requested_min / step).floor() * step;
    let rounded_maximum = (requested_max / step).ceil() * step;
    let lower_fallback = pinned_minimum.is_none() && !rounded_minimum.is_finite();
    let upper_fallback = pinned_maximum.is_none() && !rounded_maximum.is_finite();
    let minimum = pinned_minimum.unwrap_or(if rounded_minimum.is_finite() {
        rounded_minimum
    } else {
        requested_min
    });
    let maximum = pinned_maximum.unwrap_or(if rounded_maximum.is_finite() {
        rounded_maximum
    } else {
        requested_max
    });
    if !minimum.is_finite() || !maximum.is_finite() || minimum >= maximum {
        return Err(ChartError::InvalidValue {
            element: "chart axis scale".to_owned(),
            value: "nice-number bounds are nonfinite or empty".to_owned(),
        });
    }

    let first = (minimum / step).ceil();
    let last = (maximum / step).floor();
    let tick_count = if last >= first {
        (last - first).min(1024.0) as usize + 1
    } else {
        0
    };
    let mut ticks = Vec::with_capacity(tick_count);
    for index in 0..tick_count {
        let tick = (first + index as f64) * step;
        if tick.is_finite() && tick >= minimum && tick <= maximum {
            ticks.push(clean_tick(tick, step));
        }
    }
    if lower_fallback && ticks.first().is_none_or(|tick| *tick > minimum) {
        ticks.insert(0, minimum);
    }
    if upper_fallback && ticks.last().is_none_or(|tick| *tick < maximum) {
        ticks.push(maximum);
    }
    if ticks.len() < 2 {
        ticks.extend([minimum, maximum]);
        ticks.sort_by(f64::total_cmp);
    }
    ticks.dedup_by(|left, right| *left == *right);
    if ticks.len() < 2 || !ticks.windows(2).all(|pair| pair[0] < pair[1]) {
        return Err(ChartError::InvalidValue {
            element: "chart axis scale".to_owned(),
            value: "scale cannot produce two increasing finite ticks".to_owned(),
        });
    }
    Ok(AxisScale {
        domain: Domain {
            min: minimum,
            max: maximum,
        },
        ticks,
    })
}

const MIN_POSITIVE_SUBNORMAL: f64 = f64::from_bits(1);

fn stable_positive_step(minimum: f64, maximum: f64, target_tick_count: usize) -> Result<f64> {
    let scale = minimum.abs().max(maximum.abs());
    let scale = if scale == 0.0 {
        MIN_POSITIVE_SUBNORMAL
    } else {
        scale
    };
    let normalized =
        (maximum / scale - minimum / scale) / target_tick_count.saturating_sub(1) as f64;
    let step = normalized * scale;
    if normalized.is_finite() && normalized > 0.0 && step.is_finite() {
        Ok(step.max(MIN_POSITIVE_SUBNORMAL))
    } else {
        Err(ChartError::InvalidValue {
            element: "chart axis scale".to_owned(),
            value: "scale step is nonfinite or empty".to_owned(),
        })
    }
}

fn nice_step(raw_step: f64) -> Result<f64> {
    if !raw_step.is_finite() || raw_step <= 0.0 {
        return Err(ChartError::InvalidValue {
            element: "chart axis scale".to_owned(),
            value: format!("invalid raw tick step {raw_step}"),
        });
    }
    let magnitude = 10.0_f64
        .powf(raw_step.log10().floor())
        .max(MIN_POSITIVE_SUBNORMAL);
    let fraction = raw_step / magnitude;
    let multiplier = if fraction <= 1.0 {
        1.0
    } else if fraction <= 2.0 {
        2.0
    } else if fraction <= 5.0 {
        5.0
    } else {
        10.0
    };
    let step = (multiplier * magnitude).max(MIN_POSITIVE_SUBNORMAL);
    if step.is_finite() && step > 0.0 {
        Ok(step)
    } else {
        Err(ChartError::InvalidValue {
            element: "chart axis scale".to_owned(),
            value: "nice-number step is nonfinite".to_owned(),
        })
    }
}

fn expand_constant_domain(value: f64) -> Result<Domain> {
    if value == 0.0 {
        return Ok(Domain {
            min: -1.0,
            max: 1.0,
        });
    }
    let padding = (value.abs() * 0.5).max(MIN_POSITIVE_SUBNORMAL);
    let minimum = value - padding;
    let maximum = value + padding;
    if minimum.is_finite() && maximum.is_finite() && minimum < maximum {
        return Ok(Domain {
            min: minimum,
            max: maximum,
        });
    }
    let domain = if value.is_sign_positive() {
        Domain {
            min: 0.0,
            max: value,
        }
    } else {
        Domain {
            min: value,
            max: 0.0,
        }
    };
    if domain.min < domain.max {
        Ok(domain)
    } else {
        Err(ChartError::InvalidValue {
            element: "chart axis scale".to_owned(),
            value: format!("constant value {value} has no finite renderable extent"),
        })
    }
}

fn extend_bound(bound: f64, span: f64, positive: bool) -> Result<f64> {
    let candidate = if positive { bound + span } else { bound - span };
    let extended = if candidate.is_finite()
        && if positive {
            candidate > bound
        } else {
            candidate < bound
        } {
        candidate
    } else if positive {
        bound.next_up()
    } else {
        bound.next_down()
    };
    if extended.is_finite()
        && if positive {
            extended > bound
        } else {
            extended < bound
        }
    {
        Ok(extended)
    } else {
        Err(ChartError::InvalidValue {
            element: "c:scaling".to_owned(),
            value: format!("one-sided bound {bound} leaves no finite automatic extent"),
        })
    }
}

fn clean_tick(value: f64, step: f64) -> f64 {
    if value.abs() <= step.abs() * 1.0e-12 {
        0.0
    } else {
        value
    }
}

fn domain_from_layers(layers: &[GeometryLayer]) -> Result<Domain> {
    let values = layers
        .iter()
        .flatten()
        .flat_map(|(_, lower, upper)| [*lower, *upper]);
    let mut min = 0.0_f64;
    let mut max = 0.0_f64;
    for value in values {
        if !value.is_finite() {
            return Err(ChartError::InvalidValue {
                element: "chart geometry domain".to_owned(),
                value: "aggregate produced a nonfinite value".to_owned(),
            });
        }
        min = min.min(value);
        max = max.max(value);
    }
    if min == max {
        max = 1.0;
    }
    Ok(Domain { min, max })
}

fn data_domain(values: &[f64], include_zero: bool) -> Result<Domain> {
    validate_finite(values, "chart geometry cache")?;
    let Some(first) = values.first().copied() else {
        return Err(ChartError::InvalidValue {
            element: "c:numCache".to_owned(),
            value: "cached series contain no points".to_owned(),
        });
    };
    let mut min = first;
    let mut max = first;
    for value in values.iter().copied() {
        min = min.min(value);
        max = max.max(value);
    }
    if include_zero {
        min = min.min(0.0);
        max = max.max(0.0);
    }
    if min == max {
        if !include_zero {
            return expand_constant_domain(min);
        }
        if min > 0.0 {
            min = 0.0;
        } else if max < 0.0 {
            max = 0.0;
        } else {
            max = 1.0;
        }
    }
    Ok(Domain { min, max })
}

fn normalized_value(value: f64, domain: Domain) -> Result<f64> {
    let scale = value
        .abs()
        .max(domain.min.abs())
        .max(domain.max.abs())
        .max(1.0);
    let value = value / scale;
    let min = domain.min / scale;
    let max = domain.max / scale;
    let range = max - min;
    let normalized = (value - min) / range;
    if !range.is_finite() || range <= 0.0 || !normalized.is_finite() {
        return Err(ChartError::InvalidValue {
            element: "chart geometry domain".to_owned(),
            value: format!("cannot map finite domain {} to {}", domain.min, domain.max),
        });
    }
    Ok(normalized)
}

fn normalized_value_oriented(value: f64, domain: Domain, orientation: Orientation) -> Result<f64> {
    let normalized = normalized_value(value, domain)?;
    Ok(if orientation == Orientation::MaxMin {
        1.0 - normalized
    } else {
        normalized
    })
}

fn map_x_oriented(
    value: f64,
    domain: Domain,
    bounds: Rect,
    orientation: Orientation,
) -> Result<f64> {
    let coordinate =
        bounds.x + normalized_value_oriented(value, domain, orientation)? * bounds.width;
    if coordinate.is_finite() {
        Ok(coordinate)
    } else {
        Err(ChartError::InvalidValue {
            element: "chart geometry coordinate".to_owned(),
            value: "x coordinate is nonfinite".to_owned(),
        })
    }
}

fn map_y_oriented(
    value: f64,
    domain: Domain,
    bounds: Rect,
    orientation: Orientation,
) -> Result<f64> {
    let coordinate = bounds.y + bounds.height
        - normalized_value_oriented(value, domain, orientation)? * bounds.height;
    if coordinate.is_finite() {
        Ok(coordinate)
    } else {
        Err(ChartError::InvalidValue {
            element: "chart geometry coordinate".to_owned(),
            value: "y coordinate is nonfinite".to_owned(),
        })
    }
}

fn oriented_category_index(index: usize, count: usize, orientation: Orientation) -> usize {
    if orientation == Orientation::MaxMin && index < count {
        count - index - 1
    } else {
        index
    }
}

fn category_points(
    values: &[(usize, f64)],
    count: usize,
    domain: Domain,
    bounds: Rect,
    category_orientation: Orientation,
    value_orientation: Orientation,
) -> Result<Vec<Point>> {
    values
        .iter()
        .map(|(index, value)| {
            let index = oriented_category_index(*index, count, category_orientation);
            let x = bounds.x + (index as f64 + 0.5) / count as f64 * bounds.width;
            let y = map_y_oriented(*value, domain, bounds, value_orientation)?;
            if !x.is_finite() {
                return Err(ChartError::InvalidValue {
                    element: "chart geometry coordinate".to_owned(),
                    value: "category x coordinate is nonfinite".to_owned(),
                });
            }
            Ok(Point { x, y })
        })
        .collect()
}

fn polyline_path(points: &[Point], close: bool) -> Path {
    let mut commands = Vec::with_capacity(points.len() + usize::from(close));
    if let Some(first) = points.first() {
        commands.push(PathCommand::MoveTo(*first));
        commands.extend(points.iter().skip(1).copied().map(PathCommand::LineTo));
        if close {
            commands.push(PathCommand::Close);
        }
    }
    Path {
        commands,
        fill_rule: FillRule::NonZero,
    }
}

fn pie_wedge(center: Point, radius: f64, start: f64, end: f64) -> Path {
    let first = circle_point(center, radius, start);
    let mut commands = vec![PathCommand::MoveTo(center), PathCommand::LineTo(first)];
    append_arc(&mut commands, center, radius, start, end);
    commands.push(PathCommand::Close);
    Path {
        commands,
        fill_rule: FillRule::NonZero,
    }
}

fn doughnut_wedge(
    center: Point,
    outer_radius: f64,
    inner_radius: f64,
    start: f64,
    end: f64,
) -> Path {
    let outer_start = circle_point(center, outer_radius, start);
    let inner_end = circle_point(center, inner_radius, end);
    let mut commands = vec![PathCommand::MoveTo(outer_start)];
    append_arc(&mut commands, center, outer_radius, start, end);
    commands.push(PathCommand::LineTo(inner_end));
    append_arc(&mut commands, center, inner_radius, end, start);
    commands.push(PathCommand::Close);
    Path {
        commands,
        fill_rule: FillRule::NonZero,
    }
}

fn append_arc(commands: &mut Vec<PathCommand>, center: Point, radius: f64, start: f64, end: f64) {
    let segments = ((end - start).abs() / std::f64::consts::FRAC_PI_2)
        .ceil()
        .max(1.0) as usize;
    let sweep = (end - start) / segments as f64;
    for segment in 0..segments {
        let angle = start + segment as f64 * sweep;
        let next = angle + sweep;
        let factor = 4.0 / 3.0 * (sweep / 4.0).tan();
        let from = circle_point(center, radius, angle);
        let to = circle_point(center, radius, next);
        commands.push(PathCommand::CurveTo {
            c1: Point {
                x: from.x - factor * radius * angle.sin(),
                y: from.y + factor * radius * angle.cos(),
            },
            c2: Point {
                x: to.x + factor * radius * next.sin(),
                y: to.y - factor * radius * next.cos(),
            },
            to,
        });
    }
}

fn circle_point(center: Point, radius: f64, angle: f64) -> Point {
    Point {
        x: center.x + radius * angle.cos(),
        y: center.y + radius * angle.sin(),
    }
}

fn radial_point(center: Point, radius: f64, index: usize, count: usize) -> Point {
    circle_point(
        center,
        radius,
        index as f64 * std::f64::consts::TAU / count as f64 - std::f64::consts::FRAC_PI_2,
    )
}

fn marker_path(center: Point) -> Path {
    const RADIUS: f64 = 2.5;
    const KAPPA: f64 = 0.552_284_749_830_793_6;
    let control = RADIUS * KAPPA;
    Path {
        commands: vec![
            PathCommand::MoveTo(Point {
                x: center.x + RADIUS,
                y: center.y,
            }),
            PathCommand::CurveTo {
                c1: Point {
                    x: center.x + RADIUS,
                    y: center.y + control,
                },
                c2: Point {
                    x: center.x + control,
                    y: center.y + RADIUS,
                },
                to: Point {
                    x: center.x,
                    y: center.y + RADIUS,
                },
            },
            PathCommand::CurveTo {
                c1: Point {
                    x: center.x - control,
                    y: center.y + RADIUS,
                },
                c2: Point {
                    x: center.x - RADIUS,
                    y: center.y + control,
                },
                to: Point {
                    x: center.x - RADIUS,
                    y: center.y,
                },
            },
            PathCommand::CurveTo {
                c1: Point {
                    x: center.x - RADIUS,
                    y: center.y - control,
                },
                c2: Point {
                    x: center.x - control,
                    y: center.y - RADIUS,
                },
                to: Point {
                    x: center.x,
                    y: center.y - RADIUS,
                },
            },
            PathCommand::CurveTo {
                c1: Point {
                    x: center.x + control,
                    y: center.y - RADIUS,
                },
                c2: Point {
                    x: center.x + RADIUS,
                    y: center.y - control,
                },
                to: Point {
                    x: center.x + RADIUS,
                    y: center.y,
                },
            },
            PathCommand::Close,
        ],
        fill_rule: FillRule::NonZero,
    }
}

fn push_markers(elements: &mut Vec<PositionedElement>, points: &[Point], colour: Color) {
    for point in points {
        elements.push(filled_path(marker_path(*point), colour));
    }
}

fn series_colour(series_colours: &[Color], index: usize) -> Result<Color> {
    series_colours
        .get(index)
        .copied()
        .ok_or_else(|| ChartError::InvalidValue {
            element: "chart series palette".to_owned(),
            value: format!("missing resolved colour for series {index}"),
        })
}

fn filled_path(path: Path, color: Color) -> PositionedElement {
    PositionedElement::Path(PathElement {
        path,
        fill: Some(Paint::Solid(color)),
        stroke: None,
    })
}

fn stroked_path(path: Path, color: Color) -> PositionedElement {
    PositionedElement::Path(PathElement {
        path,
        fill: None,
        stroke: Some(Stroke::new(Paint::Solid(color), 1.5)),
    })
}

fn filled_and_stroked_path(path: Path, color: Color) -> PositionedElement {
    PositionedElement::Path(PathElement {
        path,
        fill: Some(Paint::Solid(Color {
            a: color.a * 0.55,
            ..color
        })),
        stroke: Some(Stroke::new(Paint::Solid(color), 1.0)),
    })
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
                            value.push_str(&decode_plain(&text));
                            seen_text = true;
                        }
                        Event::CData(text) => {
                            let decoded =
                                text.decode().map_err(|error| ChartError::InvalidValue {
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
        .write_event(Event::Text(BytesText::new(value)))
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
    emit_raw(writer, markup.raw_children.at(0))
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

fn legend_scalar_value(xml: &[u8], node: &ChartSourceNode) -> Result<String> {
    let mut reader = Reader::from_reader(&xml[node.start..node.end]);
    let value = match reader.read_event().map_err(OxmlError::from)? {
        Event::Start(element) | Event::Empty(element) => attribute_value(&element, b"val")?
            .unwrap_or_else(|| {
                if node.local == b"legendPos" {
                    "r".into()
                } else {
                    "1".into()
                }
            }),
        _ => return Err(invalid_authoring_value("legend scalar root")),
    };
    loop {
        match reader.read_event().map_err(OxmlError::from)? {
            Event::End(_) | Event::Comment(_) | Event::PI(_) => {}
            Event::Text(text) if text.iter().all(u8::is_ascii_whitespace) => {}
            Event::Eof => return Ok(value),
            _ => {
                return Err(invalid_authoring_value(
                    "legend scalar has non-scalar content",
                ));
            }
        }
    }
}

fn legend_settings_at(
    xml: &[u8],
    nodes: &[ChartSourceNode],
    legend: usize,
) -> Result<LegendSettings> {
    let mut settings = LegendSettings {
        visible: true,
        ..LegendSettings::default()
    };
    let mut position = None;
    let mut overlay = None;
    for (index, node) in nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.parent == Some(legend) && n.qualified)
    {
        if node.local != b"legendPos" && node.local != b"overlay" {
            continue;
        }
        if nodes.iter().any(|child| child.parent == Some(index)) {
            return Err(invalid_authoring_value("legend scalar has child elements"));
        }
        let value = legend_scalar_value(xml, node)?;
        if node.local == b"legendPos" {
            set_once(&mut position, LegendPosition::parse(&value)?, "c:legendPos")?;
        } else {
            set_once(
                &mut overlay,
                parse_bool_lexical("c:overlay", "val", &value)?,
                "c:overlay",
            )?;
        }
    }
    settings.position = position.unwrap_or_default();
    settings.overlay = overlay.unwrap_or(false);
    Ok(settings)
}

/// Read only the namespace-owned legend without rewriting producer bytes.
pub fn chart_legend_settings(xml: &[u8]) -> Result<LegendSettings> {
    oxoffice_core::xml::validate_strict_xml_1_0(xml)
        .map_err(|error| invalid_authoring_value(format!("invalid chart XML: {error:?}")))?;
    let nodes = chart_source_nodes(xml)?;
    if !nodes
        .first()
        .is_some_and(|n| n.qualified && n.local == b"chartSpace")
        || nodes.iter().filter(|n| n.parent.is_none()).count() != 1
    {
        return Err(invalid_authoring_value("owned chartSpace required"));
    }
    let chart = chart_child_node(&nodes, 0, b"chart")?;
    let legends = nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.parent == Some(chart) && n.qualified && n.local == b"legend")
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    match legends.as_slice() {
        [] => Ok(LegendSettings::default()),
        [index] => legend_settings_at(xml, &nodes, *index),
        _ => Err(ChartError::DuplicateElement("c:legend".into())),
    }
}

fn legend_child_rank(local: &[u8]) -> Option<u8> {
    match local {
        b"legendPos" => Some(0),
        b"legendEntry" => Some(1),
        b"layout" => Some(2),
        b"overlay" => Some(3),
        b"spPr" => Some(4),
        b"txPr" => Some(5),
        b"extLst" => Some(6),
        _ => None,
    }
}

fn insert_chart_child(
    xml: &[u8],
    node: &ChartSourceNode,
    position: usize,
    bytes: &[u8],
) -> Result<Vec<u8>> {
    let mut reader = Reader::from_reader(&xml[node.start..node.end]);
    let event = reader.read_event().map_err(OxmlError::from)?;
    let mut result = xml.to_vec();
    if let Event::Empty(element) = event {
        let end = node.start + reader.buffer_position() as usize;
        let name = element_name(&element);
        let mut replacement = xml[node.start..end - 2].to_vec();
        replacement.push(b'>');
        replacement.extend_from_slice(bytes);
        replacement.extend_from_slice(format!("</{name}>").as_bytes());
        result.splice(node.start..node.end, replacement);
    } else {
        result.splice(position..position, bytes.iter().copied());
    }
    Ok(result)
}

/// Stage a legend edit, preserving all other source bytes. None leaves a property unchanged.
/// Position and overlay edits of an absent legend are no-ops. Hiding drops its settings.
pub fn set_chart_legend(
    xml: &[u8],
    visible: Option<bool>,
    position: Option<LegendPosition>,
    overlay: Option<bool>,
) -> Result<Vec<u8>> {
    let current = chart_legend_settings(xml)?;
    if visible == Some(false) {
        if !current.visible {
            return Ok(xml.to_vec());
        }
        let nodes = chart_source_nodes(xml)?;
        let chart = chart_child_node(&nodes, 0, b"chart")?;
        let legend = chart_child_node(&nodes, chart, b"legend")?;
        let mut result = xml.to_vec();
        result.drain(nodes[legend].start..nodes[legend].end);
        return Ok(result);
    }
    if !current.visible && visible != Some(true) {
        return Ok(xml.to_vec());
    }
    if !current.visible {
        let nodes = chart_source_nodes(xml)?;
        let chart = chart_child_node(&nodes, 0, b"chart")?;
        let after = chart_child_node(&nodes, chart, b"plotArea")?;
        let bytes = format!(
            "<c:legend xmlns:c=\"{C_NS}\"><c:legendPos val=\"{}\"/><c:overlay val=\"{}\"/></c:legend>",
            position.unwrap_or_default().as_str(),
            u8::from(overlay.unwrap_or(false))
        );
        return insert_chart_child(xml, &nodes[chart], nodes[after].end, bytes.as_bytes());
    }
    let mut result = xml.to_vec();
    for (local, value, changed) in [
        (
            "legendPos",
            position.unwrap_or(current.position).as_str().to_owned(),
            position.is_some_and(|v| v != current.position),
        ),
        (
            "overlay",
            u8::from(overlay.unwrap_or(current.overlay)).to_string(),
            overlay.is_some_and(|v| v != current.overlay),
        ),
    ] {
        if !changed {
            continue;
        }
        let nodes = chart_source_nodes(&result)?;
        let chart = chart_child_node(&nodes, 0, b"chart")?;
        let legend = chart_child_node(&nodes, chart, b"legend")?;
        if let Some(node) = nodes
            .iter()
            .find(|n| n.parent == Some(legend) && n.qualified && n.local == local.as_bytes())
        {
            let mut reader = Reader::from_reader(&result[node.start..node.end]);
            let event = reader.read_event().map_err(OxmlError::from)?;
            let end = node.start + reader.buffer_position() as usize;
            let mut writer = Writer::new(Vec::new());
            match event {
                Event::Start(element) | Event::Empty(element) => {
                    let mut start = BytesStart::new(element_name(&element));
                    push_attributes(&mut start, &capture_attributes_except(&element, b"val")?);
                    start.push_attribute(("val", value.as_str()));
                    writer
                        .write_event(if result[end - 2..end] == *b"/>" {
                            Event::Empty(start)
                        } else {
                            Event::Start(start)
                        })
                        .map_err(OxmlError::from)?;
                }
                _ => return Err(invalid_authoring_value("legend scalar root")),
            }
            result.splice(node.start..end, writer.into_inner());
        } else {
            let rank = legend_child_rank(local.as_bytes()).expect("owned legend scalar");
            let insert = nodes
                .iter()
                .find(|n| {
                    n.parent == Some(legend)
                        && n.qualified
                        && legend_child_rank(&n.local).is_some_and(|r| r > rank)
                })
                .map(|n| n.start)
                .unwrap_or_else(|| {
                    result[..nodes[legend].end]
                        .iter()
                        .rposition(|v| *v == b'<')
                        .unwrap_or(nodes[legend].start)
                });
            let bytes = format!("<c:{local} xmlns:c=\"{C_NS}\" val=\"{value}\"/>");
            result = insert_chart_child(&result, &nodes[legend], insert, bytes.as_bytes())?;
        }
    }
    let expected = LegendSettings {
        visible: true,
        position: position.unwrap_or(current.position),
        overlay: overlay.unwrap_or(current.overlay),
    };
    if chart_legend_settings(&result)? != expected {
        return Err(invalid_authoring_value("legend edit projection mismatch"));
    }
    Ok(result)
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

fn push_attributes(start: &mut BytesStart<'_>, attributes: &[(String, String)]) {
    for (name, value) in attributes {
        start.push_attribute((name.as_str(), value.as_str()));
    }
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

fn capture_event(event: Event<'_>) -> Result<Vec<u8>> {
    let mut writer = Writer::new(Vec::new());
    writer
        .write_event(event.into_owned())
        .map_err(OxmlError::from)?;
    Ok(writer.into_inner())
}

fn emit_raw<'a, W: Write>(
    writer: &mut Writer<W>,
    children: impl Iterator<Item = &'a [u8]>,
) -> Result<()> {
    for child in children {
        writer.get_mut().write_all(child).map_err(OxmlError::from)?;
    }
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
