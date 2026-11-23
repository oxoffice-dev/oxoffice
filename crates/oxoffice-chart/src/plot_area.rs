//! The plot area: its plots and axes, the plot containers and the checks on axis pairs.

use super::*;

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

impl CT_PlotArea {
    pub(super) fn from_xml_with_namespaces(
        xml: &[u8],
        inherited: &NamespaceBindings,
    ) -> Result<Self> {
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

    pub(super) fn write_xml<W: Write>(&self, writer: &mut Writer<W>) -> Result<()> {
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
