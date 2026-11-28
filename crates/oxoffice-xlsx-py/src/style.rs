//! The resolved style of a cell, as frozen snapshots.

use pyo3::prelude::*;

use oxoffice_xlsx::{Alignment, Border, BorderSide, Color, Fill, Font, ResolvedStyle};

/// A colour resolved to RGB, with where it comes from.
#[pyclass(
    name = "Color",
    module = "oxoffice.xlsx",
    frozen,
    get_all,
    eq,
    skip_from_py_object
)]
#[derive(Clone, PartialEq)]
pub struct PyColor {
    rgb: Option<String>,
    source: &'static str,
    index: Option<u32>,
    tint: f64,
}

#[pymethods]
impl PyColor {
    fn __repr__(&self) -> String {
        format!(
            "<Color {} from {}>",
            self.rgb.as_deref().unwrap_or("auto"),
            self.source
        )
    }
}

fn color(color: &Option<Color>) -> Option<PyColor> {
    color.as_ref().map(|color| PyColor {
        rgb: color.rgb.clone(),
        source: color.source.kind(),
        index: color.source.index(),
        tint: color.tint,
    })
}

/// The font of a style.
#[pyclass(
    name = "Font",
    module = "oxoffice.xlsx",
    frozen,
    get_all,
    eq,
    skip_from_py_object
)]
#[derive(Clone, PartialEq)]
pub struct PyFont {
    name: Option<String>,
    size: Option<f64>,
    bold: bool,
    italic: bool,
    underline: Option<String>,
    strike: bool,
    vertical_align: Option<String>,
    color: Option<PyColor>,
}

impl From<&Font> for PyFont {
    fn from(font: &Font) -> Self {
        PyFont {
            name: font.name.clone(),
            size: font.size,
            bold: font.bold,
            italic: font.italic,
            underline: font.underline.clone(),
            strike: font.strike,
            vertical_align: font.vertical_align.clone(),
            color: color(&font.color),
        }
    }
}

/// A stop of a gradient fill.
#[pyclass(
    name = "GradientStop",
    module = "oxoffice.xlsx",
    frozen,
    get_all,
    eq,
    skip_from_py_object
)]
#[derive(Clone, PartialEq)]
pub struct PyGradientStop {
    position: f64,
    color: Option<PyColor>,
}

/// The fill of a style: a pattern, or a gradient.
#[pyclass(
    name = "Fill",
    module = "oxoffice.xlsx",
    frozen,
    get_all,
    eq,
    skip_from_py_object
)]
#[derive(Clone, PartialEq)]
pub struct PyFill {
    kind: &'static str,
    pattern: Option<String>,
    foreground: Option<PyColor>,
    background: Option<PyColor>,
    gradient_type: Option<String>,
    degree: f64,
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
    stops: Vec<PyGradientStop>,
}

impl From<&Fill> for PyFill {
    fn from(fill: &Fill) -> Self {
        match fill {
            Fill::Pattern {
                pattern,
                foreground,
                background,
            } => PyFill {
                kind: "pattern",
                pattern: Some(pattern.clone()),
                foreground: color(foreground),
                background: color(background),
                gradient_type: None,
                degree: 0.0,
                left: 0.0,
                right: 0.0,
                top: 0.0,
                bottom: 0.0,
                stops: Vec::new(),
            },
            Fill::Gradient {
                kind,
                degree,
                left,
                right,
                top,
                bottom,
                stops,
            } => PyFill {
                kind: "gradient",
                pattern: None,
                foreground: None,
                background: None,
                gradient_type: Some(kind.clone()),
                degree: *degree,
                left: *left,
                right: *right,
                top: *top,
                bottom: *bottom,
                stops: stops
                    .iter()
                    .map(|stop| PyGradientStop {
                        position: stop.position,
                        color: color(&stop.color),
                    })
                    .collect(),
            },
        }
    }
}

/// A side of a border.
#[pyclass(
    name = "BorderSide",
    module = "oxoffice.xlsx",
    frozen,
    get_all,
    eq,
    skip_from_py_object
)]
#[derive(Clone, PartialEq)]
pub struct PyBorderSide {
    style: String,
    color: Option<PyColor>,
}

impl From<&BorderSide> for PyBorderSide {
    fn from(side: &BorderSide) -> Self {
        PyBorderSide {
            style: side.style.clone(),
            color: color(&side.color),
        }
    }
}

/// The borders of a style.
#[pyclass(
    name = "Border",
    module = "oxoffice.xlsx",
    frozen,
    get_all,
    eq,
    skip_from_py_object
)]
#[derive(Clone, PartialEq)]
pub struct PyBorder {
    left: PyBorderSide,
    right: PyBorderSide,
    top: PyBorderSide,
    bottom: PyBorderSide,
    diagonal: PyBorderSide,
    diagonal_up: bool,
    diagonal_down: bool,
}

impl From<&Border> for PyBorder {
    fn from(border: &Border) -> Self {
        PyBorder {
            left: (&border.left).into(),
            right: (&border.right).into(),
            top: (&border.top).into(),
            bottom: (&border.bottom).into(),
            diagonal: (&border.diagonal).into(),
            diagonal_up: border.diagonal_up,
            diagonal_down: border.diagonal_down,
        }
    }
}

/// The alignment of a style.
#[pyclass(
    name = "Alignment",
    module = "oxoffice.xlsx",
    frozen,
    get_all,
    eq,
    skip_from_py_object
)]
#[derive(Clone, PartialEq)]
pub struct PyAlignment {
    horizontal: String,
    vertical: String,
    wrap_text: bool,
    shrink_to_fit: bool,
    indent: u32,
    text_rotation: u32,
    reading_order: u32,
}

impl From<&Alignment> for PyAlignment {
    fn from(alignment: &Alignment) -> Self {
        PyAlignment {
            horizontal: alignment.horizontal.clone(),
            vertical: alignment.vertical.clone(),
            wrap_text: alignment.wrap_text,
            shrink_to_fit: alignment.shrink_to_fit,
            indent: alignment.indent,
            text_rotation: alignment.text_rotation,
            reading_order: alignment.reading_order,
        }
    }
}

/// The protection of a style.
#[pyclass(
    name = "Protection",
    module = "oxoffice.xlsx",
    frozen,
    get_all,
    eq,
    skip_from_py_object
)]
#[derive(Clone, PartialEq)]
pub struct PyProtection {
    locked: bool,
    hidden: bool,
}

/// The style of a cell, resolved.
#[pyclass(
    name = "Style",
    module = "oxoffice.xlsx",
    frozen,
    get_all,
    eq,
    skip_from_py_object
)]
#[derive(Clone, PartialEq)]
pub struct PyStyle {
    number_format: String,
    font: PyFont,
    fill: PyFill,
    border: PyBorder,
    alignment: PyAlignment,
    protection: PyProtection,
    style_name: Option<String>,
}

#[pymethods]
impl PyStyle {
    fn __repr__(&self) -> String {
        format!(
            "<Style {:?} {} {}>",
            self.style_name.as_deref().unwrap_or(""),
            self.font.name.as_deref().unwrap_or("-"),
            self.number_format
        )
    }
}

impl From<&ResolvedStyle> for PyStyle {
    fn from(style: &ResolvedStyle) -> Self {
        PyStyle {
            number_format: style.number_format.clone(),
            font: (&style.font).into(),
            fill: (&style.fill).into(),
            border: (&style.border).into(),
            alignment: (&style.alignment).into(),
            protection: PyProtection {
                locked: style.protection.locked,
                hidden: style.protection.hidden,
            },
            style_name: style.style_name.clone(),
        }
    }
}

pub fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyColor>()?;
    module.add_class::<PyFont>()?;
    module.add_class::<PyGradientStop>()?;
    module.add_class::<PyFill>()?;
    module.add_class::<PyBorderSide>()?;
    module.add_class::<PyBorder>()?;
    module.add_class::<PyAlignment>()?;
    module.add_class::<PyProtection>()?;
    module.add_class::<PyStyle>()?;
    Ok(())
}
