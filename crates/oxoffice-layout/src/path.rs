//! Backend-neutral path geometry.

use crate::{Point, Rect};

/// One command in a path.
#[derive(Debug, Clone, PartialEq)]
pub enum PathCommand {
    MoveTo(Point),
    LineTo(Point),
    CurveTo { c1: Point, c2: Point, to: Point },
    Close,
}

/// The rule used to determine which regions a path fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillRule {
    NonZero,
    EvenOdd,
}

/// A backend-neutral path and its fill rule.
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    pub commands: Vec<PathCommand>,
    pub fill_rule: FillRule,
}

impl Path {
    /// Return conservative bounds containing every endpoint and cubic control.
    pub fn bounds(&self) -> Option<Rect> {
        let mut points = self.commands.iter().flat_map(|command| match command {
            PathCommand::MoveTo(point) | PathCommand::LineTo(point) => [Some(*point), None, None],
            PathCommand::CurveTo { c1, c2, to } => [Some(*c1), Some(*c2), Some(*to)],
            PathCommand::Close => [None, None, None],
        });
        let first = points.find_map(|point| point)?;
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (first.x, first.y, first.x, first.y);

        for point in points.flatten() {
            min_x = min_x.min(point.x);
            min_y = min_y.min(point.y);
            max_x = max_x.max(point.x);
            max_y = max_y.max(point.y);
        }

        Some(Rect {
            x: min_x,
            y: min_y,
            width: max_x - min_x,
            height: max_y - min_y,
        })
    }

    /// Construct a closed rectangular path.
    pub fn rect(rect: Rect) -> Path {
        let right = rect.x + rect.width;
        let bottom = rect.y + rect.height;
        Path {
            commands: vec![
                PathCommand::MoveTo(Point {
                    x: rect.x,
                    y: rect.y,
                }),
                PathCommand::LineTo(Point {
                    x: right,
                    y: rect.y,
                }),
                PathCommand::LineTo(Point {
                    x: right,
                    y: bottom,
                }),
                PathCommand::LineTo(Point {
                    x: rect.x,
                    y: bottom,
                }),
                PathCommand::Close,
            ],
            fill_rule: FillRule::NonZero,
        }
    }

    /// Construct a closed rectangle with one circular corner radius.
    pub fn round_rect(rect: Rect, radius: f64) -> Path {
        let radius = radius
            .max(0.0)
            .min(rect.width.abs() / 2.0)
            .min(rect.height.abs() / 2.0);
        if radius == 0.0 {
            return Self::rect(rect);
        }

        const KAPPA: f64 = 0.552_284_749_830_793_6;
        let right = rect.x + rect.width;
        let bottom = rect.y + rect.height;
        let control = radius * KAPPA;

        Path {
            commands: vec![
                PathCommand::MoveTo(Point {
                    x: rect.x + radius,
                    y: rect.y,
                }),
                PathCommand::LineTo(Point {
                    x: right - radius,
                    y: rect.y,
                }),
                PathCommand::CurveTo {
                    c1: Point {
                        x: right - radius + control,
                        y: rect.y,
                    },
                    c2: Point {
                        x: right,
                        y: rect.y + radius - control,
                    },
                    to: Point {
                        x: right,
                        y: rect.y + radius,
                    },
                },
                PathCommand::LineTo(Point {
                    x: right,
                    y: bottom - radius,
                }),
                PathCommand::CurveTo {
                    c1: Point {
                        x: right,
                        y: bottom - radius + control,
                    },
                    c2: Point {
                        x: right - radius + control,
                        y: bottom,
                    },
                    to: Point {
                        x: right - radius,
                        y: bottom,
                    },
                },
                PathCommand::LineTo(Point {
                    x: rect.x + radius,
                    y: bottom,
                }),
                PathCommand::CurveTo {
                    c1: Point {
                        x: rect.x + radius - control,
                        y: bottom,
                    },
                    c2: Point {
                        x: rect.x,
                        y: bottom - radius + control,
                    },
                    to: Point {
                        x: rect.x,
                        y: bottom - radius,
                    },
                },
                PathCommand::LineTo(Point {
                    x: rect.x,
                    y: rect.y + radius,
                }),
                PathCommand::CurveTo {
                    c1: Point {
                        x: rect.x,
                        y: rect.y + radius - control,
                    },
                    c2: Point {
                        x: rect.x + radius - control,
                        y: rect.y,
                    },
                    to: Point {
                        x: rect.x + radius,
                        y: rect.y,
                    },
                },
                PathCommand::Close,
            ],
            fill_rule: FillRule::NonZero,
        }
    }

    /// Construct a closed four-cubic approximation of an ellipse.
    pub fn ellipse(rect: Rect) -> Path {
        const KAPPA: f64 = 0.552_284_749_830_793_6;
        let cx = rect.x + rect.width / 2.0;
        let cy = rect.y + rect.height / 2.0;
        let rx = rect.width / 2.0;
        let ry = rect.height / 2.0;
        let dx = rx * KAPPA;
        let dy = ry * KAPPA;

        Path {
            commands: vec![
                PathCommand::MoveTo(Point { x: cx + rx, y: cy }),
                PathCommand::CurveTo {
                    c1: Point {
                        x: cx + rx,
                        y: cy + dy,
                    },
                    c2: Point {
                        x: cx + dx,
                        y: cy + ry,
                    },
                    to: Point { x: cx, y: cy + ry },
                },
                PathCommand::CurveTo {
                    c1: Point {
                        x: cx - dx,
                        y: cy + ry,
                    },
                    c2: Point {
                        x: cx - rx,
                        y: cy + dy,
                    },
                    to: Point { x: cx - rx, y: cy },
                },
                PathCommand::CurveTo {
                    c1: Point {
                        x: cx - rx,
                        y: cy - dy,
                    },
                    c2: Point {
                        x: cx - dx,
                        y: cy - ry,
                    },
                    to: Point { x: cx, y: cy - ry },
                },
                PathCommand::CurveTo {
                    c1: Point {
                        x: cx + dx,
                        y: cy - ry,
                    },
                    c2: Point {
                        x: cx + rx,
                        y: cy - dy,
                    },
                    to: Point { x: cx + rx, y: cy },
                },
                PathCommand::Close,
            ],
            fill_rule: FillRule::NonZero,
        }
    }
}
