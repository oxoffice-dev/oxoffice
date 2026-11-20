//! Format-neutral two-dimensional affine transforms.

use crate::{Point, Rect};

/// A 2x3 affine transform using the PDF matrix coefficient order.
///
/// Points are transformed as `x' = a*x + c*y + e` and
/// `y' = b*x + d*y + f`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Transform {
    /// The affine identity transform.
    pub const IDENTITY: Transform = Transform {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    /// Return a rotation in degrees about `(cx, cy)`.
    pub fn rotate_about(degrees: f64, cx: f64, cy: f64) -> Transform {
        let (sin, cos) = degrees.to_radians().sin_cos();
        Transform {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            e: cx - cos * cx + sin * cy,
            f: cy - sin * cx - cos * cy,
        }
    }

    /// Compose two transforms, applying `self` first and `next` second.
    pub fn then(self, next: Transform) -> Transform {
        Transform {
            a: next.a * self.a + next.c * self.b,
            b: next.b * self.a + next.d * self.b,
            c: next.a * self.c + next.c * self.d,
            d: next.b * self.c + next.d * self.d,
            e: next.a * self.e + next.c * self.f + next.e,
            f: next.b * self.e + next.d * self.f + next.f,
        }
    }

    /// Apply this transform to a point.
    pub fn apply(self, point: Point) -> Point {
        Point {
            x: self.a * point.x + self.c * point.y + self.e,
            y: self.b * point.x + self.d * point.y + self.f,
        }
    }

    /// Return whether all six coefficients exactly equal the identity.
    pub fn is_identity(self) -> bool {
        self == Self::IDENTITY
    }

    /// Return the axis-aligned bounds of the four transformed corners.
    pub fn transform_rect_bbox(self, rect: Rect) -> Rect {
        let corners = [
            self.apply(Point {
                x: rect.x,
                y: rect.y,
            }),
            self.apply(Point {
                x: rect.x + rect.width,
                y: rect.y,
            }),
            self.apply(Point {
                x: rect.x,
                y: rect.y + rect.height,
            }),
            self.apply(Point {
                x: rect.x + rect.width,
                y: rect.y + rect.height,
            }),
        ];

        let mut min_x = corners[0].x;
        let mut min_y = corners[0].y;
        let mut max_x = corners[0].x;
        let mut max_y = corners[0].y;
        for corner in &corners[1..] {
            min_x = min_x.min(corner.x);
            min_y = min_y.min(corner.y);
            max_x = max_x.max(corner.x);
            max_y = max_y.max(corner.y);
        }

        Rect {
            x: min_x,
            y: min_y,
            width: max_x - min_x,
            height: max_y - min_y,
        }
    }
}
