//! Authoring on chart part bytes: new chart parts, reading and replacing their data, the legend.

use super::*;

/// Project the direct chart title from complete current part bytes without rewriting it.
pub fn chart_plain_title(chart_xml: &[u8]) -> Result<Option<String>> {
    CT_ChartSpace::from_xml(chart_xml)?;
    use quick_xml::name::ResolveResult;
    let mut reader = oxoffice_core::xml_reader::NsReader::from_reader(chart_xml);
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
                    Event::Text(text) => {
                        target.push_str(&text.xml10_content().map_err(|error| {
                            ChartError::InvalidValue {
                                element: "c:title".into(),
                                value: error.to_string(),
                            }
                        })?)
                    }
                    Event::CData(text) => {
                        target.push_str(&text.xml10_content().map_err(|error| {
                            ChartError::InvalidValue {
                                element: "c:title".into(),
                                value: error.to_string(),
                            }
                        })?)
                    }
                    Event::GeneralRef(reference) => target.push_str(&resolve_entity(&reference)),
                    _ => {}
                }
            }
        }
    }
    let title = if has_rich { rich } else { cached };
    Ok((!title.is_empty()).then_some(title))
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
        *data_labels = Some(CT_DLbls::percentage_only());
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
    use oxoffice_core::xml_reader::NsReader;
    use quick_xml::name::ResolveResult;
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
pub(super) struct ChartSourceNode {
    local: Vec<u8>,
    qualified: bool,
    parent: Option<usize>,
    start: usize,
    end: usize,
}

pub(super) fn chart_source_nodes(xml: &[u8]) -> Result<Vec<ChartSourceNode>> {
    use oxoffice_core::xml_reader::NsReader;
    use quick_xml::name::ResolveResult;
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

pub(super) fn chart_child_node(
    nodes: &[ChartSourceNode],
    parent: usize,
    local: &[u8],
) -> Result<usize> {
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

pub(super) fn invalid_authoring_value(error: impl fmt::Display) -> ChartError {
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

pub(super) fn legend_settings_at(
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
    if let Event::Empty(_) = event {
        let (replacement, _) =
            oxoffice_core::xml_element::replace_content(&xml[node.start..node.end], bytes)?;
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
            let empty = matches!(event, Event::Empty(_));
            match event {
                Event::Start(element) | Event::Empty(element) => {
                    let mut start = BytesStart::new(element_name(&element));
                    push_attributes(&mut start, &capture_attributes_except(&element, b"val")?);
                    start.push_attribute(("val", value.as_str()));
                    writer
                        .write_event(if empty {
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
