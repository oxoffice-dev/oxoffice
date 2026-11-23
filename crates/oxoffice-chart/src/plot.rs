//! Plot families and their settings: bar direction and grouping, line grouping, scatter and radar styles, the plot enum.

use super::*;

/// Whether bars extend along the category or value axis.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BarDirection {
    Bar,
    Column,
}

impl BarDirection {
    pub(super) fn parse(value: &str) -> Option<Self> {
        match value {
            "bar" => Some(Self::Bar),
            "col" => Some(Self::Column),
            _ => None,
        }
    }

    pub(super) const fn as_str(self) -> &'static str {
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
    pub(super) fn parse(value: &str) -> Option<Self> {
        match value {
            "clustered" => Some(Self::Clustered),
            "percentStacked" => Some(Self::PercentStacked),
            "stacked" => Some(Self::Stacked),
            "standard" => Some(Self::Standard),
            _ => None,
        }
    }

    pub(super) const fn as_str(self) -> &'static str {
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
    pub(super) fn parse(value: &str) -> Option<Self> {
        match value {
            "percentStacked" => Some(Self::PercentStacked),
            "stacked" => Some(Self::Stacked),
            "standard" => Some(Self::Standard),
            _ => None,
        }
    }

    pub(super) const fn as_str(self) -> &'static str {
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
    pub(super) fn parse(value: &str) -> Option<Self> {
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

    pub(super) const fn as_str(self) -> &'static str {
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
    pub(super) fn parse(value: &str) -> Option<Self> {
        match value {
            "filled" => Some(Self::Filled),
            "marker" => Some(Self::Marker),
            "standard" => Some(Self::Standard),
            _ => None,
        }
    }

    pub(super) const fn as_str(self) -> &'static str {
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

    pub(super) fn axis_ids(&self) -> Option<[AxisId; 2]> {
        match self {
            Self::Bar { axis_ids, .. }
            | Self::Line { axis_ids, .. }
            | Self::Area { axis_ids, .. }
            | Self::Scatter { axis_ids, .. }
            | Self::Radar { axis_ids, .. } => Some(*axis_ids),
            Self::Pie { .. } | Self::Doughnut { .. } => None,
        }
    }

    pub(super) fn validate(&self) -> Result<()> {
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
