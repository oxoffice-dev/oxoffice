//! Length unit type with convenient constructors.

use crate::units::{Emu, HalfPoint, Twips};

/// A length measurement that can be expressed in various units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Length {
    emu: i64,
}

impl Length {
    /// Create a length from inches.
    pub fn inches(val: f64) -> Self {
        Length {
            emu: (val * 914400.0) as i64,
        }
    }

    /// Create a length from centimeters.
    pub fn cm(val: f64) -> Self {
        Length {
            emu: (val * 360000.0) as i64,
        }
    }

    /// Create a length from millimetres.
    pub fn mm(val: f64) -> Self {
        Length {
            emu: (val * 36000.0) as i64,
        }
    }

    /// Create a length from points.
    pub fn pt(val: f64) -> Self {
        Length {
            emu: (val * 12700.0) as i64,
        }
    }

    /// Create a length from EMUs.
    pub fn emu(val: i64) -> Self {
        Length { emu: val }
    }

    /// Create a length from twips.
    pub fn twips(val: i32) -> Self {
        Length {
            emu: val as i64 * 635,
        }
    }

    /// Get the value in inches.
    pub fn to_inches(self) -> f64 {
        self.emu as f64 / 914400.0
    }

    /// Get the value in centimeters.
    pub fn to_cm(self) -> f64 {
        self.emu as f64 / 360000.0
    }

    /// Get the value in points.
    pub fn to_pt(self) -> f64 {
        self.emu as f64 / 12700.0
    }

    /// Get the value in EMUs.
    pub fn to_emu(self) -> i64 {
        self.emu
    }

    /// Get the value in twips.
    pub fn to_twips(self) -> i32 {
        (self.emu / 635) as i32
    }

    /// Convert to the OXML Twips type.
    pub fn as_twips(self) -> Twips {
        Twips(self.to_twips())
    }

    /// Convert to the OXML Emu type.
    pub fn as_emu(self) -> Emu {
        Emu(self.emu)
    }

    /// Convert to the OXML HalfPoint type (for font sizes).
    pub fn as_half_points(self) -> HalfPoint {
        HalfPoint((self.to_pt() * 2.0) as u32)
    }
}
