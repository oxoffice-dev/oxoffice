//! Rendering a chart to positioned elements: geometry, axes, labels, legend and plots.

use super::*;

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
/// Dates on axes and in labels are counted in the chart space's date system
/// (`c:date1904`).
pub fn render_chart(
    chart_space: &CT_ChartSpace,
    bounds: Rect,
    theme: &CT_OfficeStyleSheet,
    color_map: &ColorMap,
    fonts: &mut FontManager,
) -> Result<GroupElement> {
    let chart = &chart_space.chart;
    let numbers = FormatOptions::default().with_date_system(chart_space.date_system()?);
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

    let mut chart_annotations = render_axes_and_labels(
        plot,
        &axes,
        plot_bounds,
        chart.disp_blanks_as,
        &numbers,
        fonts,
    )?;
    let mut annotations = Vec::new();
    render_data_labels(
        plot,
        plot_bounds,
        chart.disp_blanks_as,
        options,
        &numbers,
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
    numbers: &FormatOptions,
    fonts: &mut FontManager,
) -> Result<ChartAnnotations> {
    if matches!(plot, Plot::Radar { .. }) {
        return render_radar_axes_and_labels(plot, axes, bounds, blanks, numbers, fonts);
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
                    let text = format_axis_value(axis.number_format.as_ref(), *tick, numbers)?;
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
            let category_labels: BTreeMap<_, _> = if axis.tick_label_position
                != TickLabelPosition::None
            {
                series
                    .first()
                    .map(|series| series_categories(series, axis.number_format.as_ref(), numbers))
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
    numbers: &FormatOptions,
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
                    let text = format_axis_value(axis.number_format.as_ref(), *tick, numbers)?;
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
            let category_labels: BTreeMap<_, _> = if axis.tick_label_position
                != TickLabelPosition::None
            {
                series
                    .first()
                    .map(|series| series_categories(series, axis.number_format.as_ref(), numbers))
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

fn format_axis_value(
    format: Option<&NumberFormat>,
    value: f64,
    numbers: &FormatOptions,
) -> Result<String> {
    match format {
        Some(format) => format_or_general(format, value, numbers),
        None => Ok(general(value, numbers)),
    }
}

fn general(value: f64, numbers: &FormatOptions) -> String {
    oxoffice_numfmt::NumberFormat::general()
        .format_number(value, numbers)
        .text
}

/// A code Excel cannot read shows its values as General, as in Excel.
fn format_or_general(format: &NumberFormat, value: f64, numbers: &FormatOptions) -> Result<String> {
    match format.format_value(value, numbers) {
        Err(ChartError::InvalidValue { element, .. })
            if element == "c:numFmt/@formatCode projection" && value.is_finite() =>
        {
            Ok(general(value, numbers))
        }
        result => result,
    }
}
/// The label and axis codes the chart does not support
/// ([`NumberFormat::is_supported`]), whose values show as General,
/// deduplicated in plot, point and axis source order.
pub fn unsupported_number_formats(chart: &CT_Chart) -> Result<Vec<String>> {
    let mut codes = Vec::new();
    let mut note = |format: &NumberFormat| {
        if !format.is_supported() && !codes.contains(&format.format_code) {
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
    numbers: &FormatOptions,
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
                    let text = format_or_general(format, *value, numbers)?;
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
    numbers: &FormatOptions,
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
                        series_categories(series, None, numbers)?
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
                fields.push(format_axis_value(
                    labels.number_format.as_ref(),
                    value,
                    numbers,
                )?);
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
                        format_or_general(format, value / total, numbers)?
                    } else {
                        NumberFormat::new("0%".to_owned(), false)?
                            .format_value(value / total, numbers)?
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
                    fields.push(format_axis_value(
                        labels.number_format.as_ref(),
                        *value,
                        numbers,
                    )?);
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
                let raw = capture_empty(&element, reader.empty_writing())?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn space(xml: &str) -> CT_ChartSpace {
        CT_ChartSpace::from_xml(xml.as_bytes()).unwrap()
    }

    /// A scatter chart whose value axes show the year of their ticks, with
    /// `extra` written first in the chart space.
    fn yearly_chart(extra: &str) -> String {
        let data = ChartData {
            categories: vec!["43844".into(), "43874".into()],
            series: vec![("Sales".into(), vec![43844.0, 43874.0])],
            ..ChartData::default()
        };
        let (xml, _) = authored_chart_parts(ChartKind::Scatter, &data, "rId1").unwrap();
        let xml = String::from_utf8(xml).unwrap();
        let root = xml.find("<c:chartSpace").unwrap();
        let root_end = root + xml[root..].find('>').unwrap() + 1;
        let xml = format!("{}{extra}{}", &xml[..root_end], &xml[root_end..]);
        xml.replace(
            "<c:crossAx",
            "<c:numFmt formatCode=\"yyyy\" sourceLinked=\"0\"/><c:crossAx",
        )
    }

    fn label_texts(space: &CT_ChartSpace) -> BTreeSet<String> {
        let mut fonts = FontManager::new_deterministic().unwrap();
        let bounds = Rect {
            x: 0.0,
            y: 0.0,
            width: 400.0,
            height: 300.0,
        };
        let group = render_chart(
            space,
            bounds,
            &CT_OfficeStyleSheet::office_default(),
            &ColorMap::default(),
            &mut fonts,
        )
        .unwrap();
        let mut texts = BTreeSet::new();
        let mut stack = vec![&group];
        while let Some(group) = stack.pop() {
            for child in &group.children {
                match child {
                    PositionedElement::Text(run) => {
                        texts.insert(run.text.clone());
                    }
                    PositionedElement::Group(inner) => stack.push(inner),
                    _ => {}
                }
            }
        }
        texts
    }

    #[test]
    fn a_chart_counts_its_dates_in_the_date_system_of_its_chart_space() {
        let in_1900 = label_texts(&space(&yearly_chart("")));
        let in_1904 = label_texts(&space(&yearly_chart("<c:date1904 val=\"1\"/>")));
        assert!(in_1900.contains("2020"), "{in_1900:?}");
        assert!(in_1904.contains("2024"), "{in_1904:?}");
        assert!(!in_1904.contains("2020"), "{in_1904:?}");
        for (date1904, system) in [
            ("", DateSystem::Date1900),
            ("<c:date1904/>", DateSystem::Date1904),
            ("<c:date1904 val=\"0\"/>", DateSystem::Date1900),
        ] {
            let read = space(&yearly_chart(date1904)).date_system().unwrap();
            assert_eq!(read, system, "{date1904}");
        }
    }

    #[test]
    fn axis_and_label_values_show_as_excel_shows_them() {
        let format = |code: &str| NumberFormat::new(code.to_owned(), false).unwrap();
        let in_1900 = FormatOptions::default();
        let shown = |code: Option<&str>, value| {
            format_axis_value(code.map(format).as_ref(), value, &in_1900).unwrap()
        };
        assert_eq!(shown(Some("d-mmm-yy"), 45306.0), "15-Jan-24");
        assert_eq!(shown(Some("[$-1009]yyyy/mm/dd"), 45306.0), "2024/01/15");
        assert_eq!(shown(Some("# ?/?"), 1.5), "1 1/2");
        assert_eq!(shown(Some("0.0;[Red](0.0)"), -2.0), "(2.0)");
        assert_eq!(shown(None, 0.1 + 0.2), "0.3");
        // Names in the language of the tag, as Excel writes them.
        assert_eq!(shown(Some("[$-40C]mmmm yyyy"), 45306.0), "janvier 2024");
        assert_eq!(shown(Some("[$-407]ddd d"), 45306.0), "Mo 15");
        // A code Excel cannot read, and month names of a language without
        // a table, show as General and are reported.
        for code in ["0.0e+00", "[$-411]mmmm yyyy"] {
            assert_eq!(shown(Some(code), 45306.0), "45306", "{code}");
            assert!(!format(code).is_supported(), "{code}");
        }
        assert!(format("[$€-40C]#,##0.00").is_supported());
        assert!(format("[$-419]h:mm AM/PM").is_supported());
        let reported =
            space(&yearly_chart("").replace("formatCode=\"yyyy\"", "formatCode=\"[$-411]mmmm\""));
        assert_eq!(
            unsupported_number_formats(&reported.chart).unwrap(),
            vec!["[$-411]mmmm".to_owned()]
        );
    }
}
