//! Backend-neutral fill and stroke paint.

use crate::{Color, MediaId, Point, Rect, Transform};

/// One color stop in a gradient.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradientStop {
    pub offset: f64,
    pub color: Color,
}

/// Fill or stroke paint independent of a rendering backend.
#[derive(Debug, Clone, PartialEq)]
pub enum Paint {
    Solid(Color),
    Linear {
        start: Point,
        end: Point,
        stops: Vec<GradientStop>,
        extend: (bool, bool),
    },
    Radial {
        center: Point,
        radius: f64,
        focal: Point,
        stops: Vec<GradientStop>,
        extend: (bool, bool),
    },
    /// A source-derived colour grid, repeated over its physical tile bounds.
    /// Path gradients use one tile covering the complete painted owner.
    Grid {
        bounds: Rect,
        columns: u16,
        rows: u16,
        colors: Vec<Color>,
    },
    Tile {
        image: MediaId,
        tile: Rect,
        transform: Transform,
    },
}

impl Paint {
    /// Validate a bounded colour grid before publishing backend-neutral paint.
    pub fn grid(bounds: Rect, columns: u16, rows: u16, colors: Vec<Color>) -> Option<Self> {
        let count = usize::from(columns) * usize::from(rows);
        (columns > 0
            && rows > 0
            && count <= 65_536
            && colors.len() == count
            && [bounds.x, bounds.y, bounds.width, bounds.height]
                .into_iter()
                .all(f64::is_finite)
            && bounds.width > 0.0
            && bounds.height > 0.0
            && colors.iter().all(|c| {
                [c.r, c.g, c.b, c.a]
                    .into_iter()
                    .all(|v| v.is_finite() && (0.0..=1.0).contains(&v))
            }))
        .then_some(Self::Grid {
            bounds,
            columns,
            rows,
            colors,
        })
    }

    /// Construct a linear gradient, degrading one stop to a solid paint.
    pub fn linear(
        start: Point,
        end: Point,
        stops: Vec<GradientStop>,
        extend: (bool, bool),
    ) -> Self {
        if let [stop] = stops.as_slice() {
            Self::Solid(stop.color)
        } else {
            Self::Linear {
                start,
                end,
                stops,
                extend,
            }
        }
    }

    /// Construct a radial gradient, degrading one stop to a solid paint.
    pub fn radial(
        center: Point,
        radius: f64,
        focal: Point,
        stops: Vec<GradientStop>,
        extend: (bool, bool),
    ) -> Self {
        if let [stop] = stops.as_slice() {
            Self::Solid(stop.color)
        } else {
            Self::Radial {
                center,
                radius,
                focal,
                stops,
                extend,
            }
        }
    }
}

/// Stroke endpoint style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineCap {
    #[default]
    Butt,
    Round,
    Square,
}

/// Stroke segment join style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineJoin {
    #[default]
    Miter,
    Round,
    Bevel,
}

/// Paint and geometry used to stroke a path.
#[derive(Debug, Clone, PartialEq)]
pub struct Stroke {
    pub paint: Paint,
    pub width: f64,
    pub cap: LineCap,
    pub join: LineJoin,
    pub dash: Option<Vec<f64>>,
}

impl Stroke {
    /// Construct a stroke with butt caps, miter joins, and no dash.
    pub fn new(paint: Paint, width: f64) -> Self {
        Self {
            paint,
            width,
            cap: LineCap::Butt,
            join: LineJoin::Miter,
            dash: None,
        }
    }
}
