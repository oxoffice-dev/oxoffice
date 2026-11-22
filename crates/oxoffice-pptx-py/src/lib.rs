//! Python bindings for the oxoffice-pptx facade, published as `oxoffice.pptx`.

mod dml;
mod layout;
mod presentation;
mod shape;
mod slide;
mod table;
mod text;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::PyType;

use oxoffice_py_support::{ContentPath, PathSeg, StaleElementError};
use presentation::{
    PyComment, PyCommentAuthor, PyCommentReply, PyPresentation, PyPresentationTextExport,
    PyValidationIssue,
};

pub(crate) use oxoffice_py_support::normalize_index;

pub(crate) use oxoffice_py_support::LengthUnit;

const LENGTHS: oxoffice_py_support::LengthClasses = oxoffice_py_support::LengthClasses {
    module: "oxoffice.pptx",
    hint: "Pt",
};

/// Read one optional length argument by the rule every length argument
/// follows: see [`oxoffice_py_support::length_argument`].
pub(crate) fn length_argument(
    value: Option<&Bound<'_, PyAny>>,
    name: &str,
    unit: LengthUnit,
) -> PyResult<Option<i64>> {
    oxoffice_py_support::length_argument(value, LENGTHS, name, unit)
}

/// Read one length argument that takes no `None`, by the same rule.
pub(crate) fn required_length(
    value: &Bound<'_, PyAny>,
    name: &str,
    unit: LengthUnit,
) -> PyResult<i64> {
    oxoffice_py_support::required_length(value, LENGTHS, name, unit)
}

fn public_exception_type<'py>(py: Python<'py>, class_name: &str) -> PyResult<Bound<'py, PyType>> {
    oxoffice_py_support::public_exception_type(py, "oxoffice.pptx", class_name)
}

fn public_error(py: Python<'_>, class_name: &str, message: String) -> PyErr {
    oxoffice_py_support::public_error(py, "oxoffice.pptx", class_name, message)
}

/// A counted replacement that matched a different number of times than the
/// caller expected, worded like `oxoffice pptx replace --expect`.
pub(crate) fn replacement_count_to_pyerr(
    py: Python<'_>,
    placeholder: &str,
    expected: usize,
    found: usize,
) -> PyErr {
    let message = format!("expected {expected} replacement(s) of \"{placeholder}\", found {found}");
    match public_exception_type(py, "ReplacementCountError") {
        Ok(class) => PyErr::from_type(class, (message, expected, found)),
        Err(_) => PyRuntimeError::new_err(message),
    }
}

pub(crate) fn stale_to_pyerr(py: Python<'_>, error: StaleElementError) -> PyErr {
    public_error(py, "StaleElementError", error.to_string())
}

pub(crate) fn recovery_hint(path: &ContentPath, suffix: &str) -> String {
    let mut public_path = String::from("prs");
    let mut pending_row = None;
    for segment in &path.segs {
        match segment {
            PathSeg::Slide(index) => public_path.push_str(&format!(".slides[{index}]")),
            PathSeg::Layout(index) => public_path.push_str(&format!(".slide_layouts[{index}]")),
            PathSeg::TextStyle(index) => public_path.push_str(&format!(
                ".text_styles.{}",
                ["title", "body", "other"].get(*index).unwrap_or(&"invalid")
            )),
            PathSeg::Master(index) => public_path.push_str(&format!(".slide_masters[{index}]")),
            PathSeg::Shape(index) => public_path.push_str(&format!(".shapes[{index}]")),
            PathSeg::Body(index) => public_path.push_str(&format!(".body[{index}]")),
            PathSeg::Row(index) => pending_row = Some(*index),
            PathSeg::Cell(index) => {
                if let Some(row) = pending_row.take() {
                    public_path.push_str(&format!(".table.cell({row}, {index})"));
                } else {
                    public_path.push_str(&format!(".table.columns[{index}]"));
                }
            }
            PathSeg::Para(index) => {
                public_path.push_str(&format!(".text_frame.paragraphs[{index}]"));
            }
            PathSeg::Run(index) => public_path.push_str(&format!(".runs[{index}]")),
        }
    }
    if let Some(row) = pending_row {
        public_path.push_str(&format!(".table.rows[{row}]"));
    }
    public_path.push_str(suffix);
    format!("Re-fetch it with {public_path}.")
}

pub(crate) fn validate_path(
    py: Python<'_>,
    presentation: &PyPresentation,
    path: &ContentPath,
    kind: &str,
    suffix: &str,
) -> PyResult<()> {
    if !path.belongs_to(&presentation.revisions) {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "handle belongs to a different presentation",
        ));
    }
    path.validate_scopes(&presentation.revisions, kind, &recovery_hint(path, suffix))
        .map_err(|error| stale_to_pyerr(py, error))?;
    Ok(())
}

pub(crate) fn pptx_to_pyerr(py: Python<'_>, error: oxoffice_pptx::Error) -> PyErr {
    let message = error.to_string();
    // A refused character XML 1.0 cannot carry is an `XmlError`, also a
    // `ValueError`, wherever the refusal was wrapped.
    if message.contains(oxoffice_core::xml::NON_XML_CHARACTER_REFUSAL) {
        return public_error(py, "XmlError", message);
    }
    let class_name = match &error {
        oxoffice_pptx::Error::MalformedPart { .. } => "XmlError",
        oxoffice_pptx::Error::Package(_)
        | oxoffice_pptx::Error::MissingMainDocument
        | oxoffice_pptx::Error::OverwritesSource { .. }
        | oxoffice_pptx::Error::MissingRelationship { .. }
        | oxoffice_pptx::Error::WrongRelationshipType { .. }
        | oxoffice_pptx::Error::ExternalRelationship { .. }
        | oxoffice_pptx::Error::MissingPart { .. }
        | oxoffice_pptx::Error::CorePropertiesPartCollision { .. }
        | oxoffice_pptx::Error::DuplicateNotesSlides { .. } => "PackageError",
        _ => "PptxError",
    };
    public_error(py, class_name, message)
}

/// A refused value: an `XmlError` for a character XML 1.0 cannot carry, a
/// `ValueError` with `fallback` otherwise.
pub(crate) fn refused_value_to_pyerr(
    py: Python<'_>,
    error: impl std::fmt::Display,
    fallback: &str,
) -> PyErr {
    let message = error.to_string();
    if message.contains(oxoffice_core::xml::NON_XML_CHARACTER_REFUSAL) {
        return public_error(py, "XmlError", message);
    }
    pyo3::exceptions::PyValueError::new_err(fallback.to_owned())
}

pub(crate) fn pptx_value_to_pyerr(py: Python<'_>, message: String) -> PyErr {
    if message.contains(oxoffice_core::xml::NON_XML_CHARACTER_REFUSAL) {
        return public_error(py, "XmlError", message);
    }
    public_error(py, "PptxError", message)
}

/// Register every class and function of the pptx bindings on `module`.
pub fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyPresentation>()?;
    module.add_class::<slide::PyNotesSlide>()?;
    module.add_class::<slide::PyNotesTextFrame>()?;
    module.add_class::<presentation::PyPptElement>()?;
    module.add_class::<presentation::PyFontEmbeddingLicense>()?;
    module.add_class::<presentation::PyEmbeddedFont>()?;
    module.add_class::<presentation::PyEmbeddedFontFace>()?;
    module.add_class::<presentation::PyEmbeddedTypeface>()?;

    module.add_class::<PyPresentationTextExport>()?;
    module.add_class::<presentation::PyCoreProperties>()?;
    module.add_class::<presentation::PyAppProperties>()?;
    module.add_class::<presentation::PyCustomFileTime>()?;
    module.add_class::<presentation::PyCustomProperty>()?;
    module.add_class::<presentation::PyCustomProperties>()?;
    module.add_class::<presentation::PySection>()?;
    module.add_class::<PyCommentAuthor>()?;
    module.add_class::<PyComment>()?;
    module.add_class::<PyCommentReply>()?;
    module.add_class::<PyValidationIssue>()?;
    slide::register(module)?;
    shape::register(module)?;
    dml::register(module)?;
    text::register(module)?;
    table::register(module)?;
    layout::register(module)?;
    Ok(())
}
