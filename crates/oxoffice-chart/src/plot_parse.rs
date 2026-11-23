//! Parsing one plot: bar, line and the other plot families.

use super::*;

pub(super) fn parse_plot(xml: &[u8], inherited: &NamespaceBindings) -> Result<(Plot, PlotMarkup)> {
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
