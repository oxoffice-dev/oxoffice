//! Python bindings for the oxoffice-docx facade, published as `oxoffice.docx`.

mod document;
mod formatting;
mod paragraph;
mod run;
mod table;

use pyo3::exceptions::{PyIndexError, PyRuntimeError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyType};

use oxoffice_py_support::StaleElementError;

use document::{
    PyBookmark, PyBoundingBox, PyChart, PyComment, PyComparisonDiagnostic, PyContentFragment,
    PyCoreProperties, PyDefaultParagraphFormat, PyDocument, PyFieldEvaluation,
    PyFieldsUpdateReport, PyHeaderFooter, PyHeaderFooterVariant, PyHyperlink,
    PyLayoutBackedFieldUpdateReport, PyLayoutFragment, PyLayoutPage, PyListLevel, PyNote,
    PyRevision, PyRunPosition, PyRunRange, PySection, PySectionHandle, PySettings, PyStory,
    PyStoryItem, PyStoryRunPosition, PyStoryRunRange, PyStyle, PySvgDiagnostic, PySvgRenderResult,
    PyTocRebuildReport,
};
use formatting::{PyColorFormat, PyFont, PyParagraphFormat};
use paragraph::{PyParagraph, PyParagraphCollection};
use run::{PyRun, PyRunCollection};
use table::{
    PyCell, PyCellCollection, PyCellParagraphCollection, PyRow, PyRowCollection, PyTable,
    PyTableCollection,
};

pub(crate) fn normalize_index(index: isize, len: usize, kind: &str) -> PyResult<usize> {
    let normalized = if index < 0 {
        len as isize + index
    } else {
        index
    };
    if normalized < 0 || normalized >= len as isize {
        return Err(PyIndexError::new_err(format!("{kind} index out of range")));
    }
    Ok(normalized as usize)
}

pub(crate) fn length_object(py: Python<'_>, value: oxoffice_docx::Length) -> PyResult<Py<PyAny>> {
    py.import("oxoffice.docx")?
        .getattr("Length")?
        .call1((value.to_emu(),))
        .map(Bound::unbind)
}

pub(crate) fn enum_object(py: Python<'_>, name: &str, value: i32) -> PyResult<Py<PyAny>> {
    py.import("oxoffice.docx")?
        .getattr(name)?
        .call1((value,))
        .map(Bound::unbind)
}

/// Normalize a validated scalar RGB input for existing native Word setters.
pub(crate) fn color_hex(value: &Bound<'_, PyAny>, parameter: &str) -> PyResult<String> {
    let (red, green, blue) = oxoffice_py_support::rgb_channels(value, parameter)?;
    Ok(format!("{red:02X}{green:02X}{blue:02X}"))
}

/// Return the stored scalar colour, preserving Word's automatic sentinel.
pub(crate) fn color_object(py: Python<'_>, value: &str) -> PyResult<Py<PyAny>> {
    if value.eq_ignore_ascii_case("auto") {
        return Ok(pyo3::types::PyString::new(py, "auto").into_any().unbind());
    }
    py.import("oxoffice.docx")?
        .getattr("RGBColor")?
        .call_method1("from_string", (value,))
        .map(Bound::unbind)
}

fn public_exception_type<'py>(py: Python<'py>, class_name: &str) -> PyResult<Bound<'py, PyType>> {
    py.import("oxoffice.docx")
        .and_then(|module| module.getattr(class_name))
        .and_then(|class| class.cast_into::<PyType>().map_err(Into::into))
}

fn public_error(py: Python<'_>, class_name: &str, message: String) -> PyErr {
    match public_exception_type(py, class_name) {
        Ok(class) => PyErr::from_type(class, (message,)),
        Err(_) => PyRuntimeError::new_err(message),
    }
}

/// A counted replacement that matched a different number of times than the
/// caller expected. A single replacement uses the wording of
/// `oxoffice docx replace --expect`, and a batch names the failing pair.
pub(crate) fn replacement_count_to_pyerr(
    py: Python<'_>,
    mismatch: &oxoffice_docx::ReplacementCountMismatch,
    batch: bool,
) -> PyErr {
    let (message, index) = if batch {
        (mismatch.to_string(), Some(mismatch.index))
    } else {
        (
            format!(
                "expected {} replacement(s) of \"{}\", found {}",
                mismatch.expected, mismatch.placeholder, mismatch.found
            ),
            None,
        )
    };
    match public_exception_type(py, "ReplacementCountError") {
        Ok(class) => PyErr::from_type(class, (message, mismatch.expected, mismatch.found, index)),
        Err(_) => PyRuntimeError::new_err(message),
    }
}

pub(crate) fn stale_to_pyerr(py: Python<'_>, error: StaleElementError) -> PyErr {
    public_error(py, "StaleElementError", error.to_string())
}

pub(crate) fn docx_to_pyerr(py: Python<'_>, error: oxoffice_docx::Error) -> PyErr {
    let class_name = match &error {
        oxoffice_docx::Error::Opc(_)
        | oxoffice_docx::Error::Io(_)
        | oxoffice_docx::Error::NoDocumentPart
        | oxoffice_docx::Error::OverwritesSource { .. }
        | oxoffice_docx::Error::UnavailableImageDimensions { .. } => "PackageError",
        oxoffice_docx::Error::Oxml(_) => "XmlError",
        oxoffice_docx::Error::Layout(_)
        | oxoffice_docx::Error::Pdf(_)
        | oxoffice_docx::Error::Raster(_) => "LayoutError",
        oxoffice_docx::Error::Rtf { .. }
        | oxoffice_docx::Error::Html { .. }
        | oxoffice_docx::Error::Mhtml { .. }
        | oxoffice_docx::Error::Odt { .. }
        | oxoffice_docx::Error::InvalidEmbeddedMutation { .. }
        | oxoffice_docx::Error::Story(_)
        | oxoffice_docx::Error::Other(_) => "DocxError",
    };
    public_error(py, class_name, error.to_string())
}

/// Register every class and function of the docx bindings on `module`.
pub fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyDocument>()?;
    #[cfg(debug_assertions)]
    module.add_function(wrap_pyfunction!(document::word_whole_body_work, module)?)?;
    #[cfg(debug_assertions)]
    module.add_function(wrap_pyfunction!(document::word_publication_work, module)?)?;
    #[cfg(debug_assertions)]
    module.add_function(wrap_pyfunction!(
        document::word_force_checked_route,
        module
    )?)?;
    #[cfg(debug_assertions)]
    module.add_function(wrap_pyfunction!(document::word_splice_work, module)?)?;
    #[cfg(debug_assertions)]
    module.add_function(wrap_pyfunction!(document::word_transaction_work, module)?)?;
    #[cfg(debug_assertions)]
    module.add_function(wrap_pyfunction!(
        document::scoped_normalization_work,
        module
    )?)?;
    #[cfg(debug_assertions)]
    module.add_function(wrap_pyfunction!(
        document::word_transaction_outputs,
        module
    )?)?;
    #[cfg(debug_assertions)]
    module.add_function(wrap_pyfunction!(document::word_piece_work, module)?)?;
    #[cfg(debug_assertions)]
    module.add_function(wrap_pyfunction!(document::word_piece_alignments, module)?)?;
    #[cfg(debug_assertions)]
    module.add_function(wrap_pyfunction!(document::word_piece_bytes, module)?)?;
    #[cfg(debug_assertions)]
    module.add_function(wrap_pyfunction!(document::staging_work, module)?)?;
    #[cfg(debug_assertions)]
    module.add_function(wrap_pyfunction!(document::reopen_work, module)?)?;
    #[cfg(debug_assertions)]
    module.add_function(wrap_pyfunction!(document::main_body_block_scans, module)?)?;
    module.add_class::<document::PyFontEmbeddingLicense>()?;
    module.add_class::<document::PyEmbeddedFont>()?;
    module.add_class::<document::PyFontDefinition>()?;

    module.add_class::<document::PyPicture>()?;
    module.add_class::<document::PyPictureOptions>()?;
    module.add_class::<document::PyHtmlDiagnostic>()?;
    module.add_class::<document::PyOdtDiagnostic>()?;
    module.add_class::<document::PyRtfDiagnostic>()?;
    module.add_class::<document::PyEpubDiagnostic>()?;
    module.add_class::<document::PyTextDiagnostic>()?;
    module.add_class::<document::PyValidationFinding>()?;
    module.add_class::<document::PyOdtExportResult>()?;
    module.add_class::<document::PyRtfExportResult>()?;
    module.add_class::<document::PyEpubExportResult>()?;
    module.add_class::<document::PyTextReport>()?;
    module.add_class::<paragraph::PyMathConversionDiagnostic>()?;
    module.add_class::<paragraph::PyMathConversionResult>()?;
    module.add_class::<paragraph::PyEquationConversionResult>()?;
    module.add_class::<paragraph::PyEquation>()?;
    module.add_class::<paragraph::PyEquationInsertResult>()?;
    module.add_class::<document::PyFragmentConflictPolicy>()?;
    module.add_class::<document::PyContentLocation>()?;
    module.add_class::<document::PyDocumentFragment>()?;
    module.add_class::<document::PyContentControl>()?;
    module.add_class::<document::PyContentControlTypePayload>()?;
    module.add_class::<document::PyMailMergePackageSource>()?;
    module.add_class::<document::PyMailMergeSettings>()?;
    module.add_class::<document::PyMailMergeFieldMapping>()?;
    module.add_class::<document::PyMailMergeRecipient>()?;
    module.add_class::<document::PyMailMergePackageOptions>()?;
    module.add_class::<document::PyMailMergePackageDiagnostic>()?;
    module.add_class::<document::PyMailMergePreparedData>()?;
    module.add_class::<document::PyLegacyFormField>()?;
    module.add_class::<document::PyLegacyFormFieldDefinition>()?;
    module.add_class::<document::PyFormFieldTarget>()?;
    module.add_class::<document::PyFormFieldValue>()?;
    module.add_class::<document::PyFormResetValue>()?;
    module.add_class::<document::PyFormFieldRule>()?;
    module.add_class::<document::PyFormFieldAssignment>()?;
    module.add_class::<document::PyFormValidationIssue>()?;
    module.add_class::<document::PyDocumentProtection>()?;
    module.add_class::<document::PyCustomFileTime>()?;
    module.add_class::<document::PyCustomProperty>()?;
    module.add_class::<document::PyCustomProperties>()?;
    module.add_class::<document::PyAppProperties>()?;
    module.add_class::<document::PyValidationReport>()?;
    module.add_function(wrap_pyfunction!(document::validate_bytes, module)?)?;
    module.add_function(wrap_pyfunction!(document::validate_file, module)?)?;

    module.add_class::<PyRunPosition>()?;
    module.add_class::<PyRunRange>()?;
    module.add_class::<PyBookmark>()?;
    module.add_class::<PyStoryRunPosition>()?;
    module.add_class::<PyStoryRunRange>()?;
    module.add_class::<PyComment>()?;
    module.add_class::<PyComparisonDiagnostic>()?;
    module.add_class::<PySvgDiagnostic>()?;
    module.add_class::<PySvgRenderResult>()?;
    module.add_class::<PyBoundingBox>()?;
    module.add_class::<PyLayoutFragment>()?;
    module.add_class::<PyLayoutPage>()?;
    module.add_class::<PyLayoutBackedFieldUpdateReport>()?;
    module.add_class::<PyFieldEvaluation>()?;
    module.add_class::<PyFieldsUpdateReport>()?;
    module.add_class::<PyTocRebuildReport>()?;
    module.add_class::<PyRevision>()?;
    module.add_class::<PyStory>()?;
    module.add_class::<PyStoryItem>()?;
    module.add_class::<PyContentFragment>()?;
    module.add_class::<PyCoreProperties>()?;
    module.add_class::<PyHyperlink>()?;
    module.add_class::<PyHeaderFooterVariant>()?;
    module.add_class::<PySection>()?;
    module.add_class::<PySectionHandle>()?;
    module.add_class::<PyHeaderFooter>()?;
    module.add_class::<PySettings>()?;
    module.add_class::<PyNote>()?;
    module.add_class::<PyStyle>()?;
    module.add_class::<PyDefaultParagraphFormat>()?;
    module.add_class::<PyListLevel>()?;
    module.add_class::<PyChart>()?;
    module.add_class::<PyParagraph>()?;
    module.add_class::<PyParagraphCollection>()?;
    module.add_class::<PyRun>()?;
    module.add_class::<PyRunCollection>()?;
    module.add_class::<PyFont>()?;
    module.add_class::<PyColorFormat>()?;
    module.add_class::<PyParagraphFormat>()?;
    module.add_class::<formatting::PyTabStops>()?;
    module.add_class::<formatting::PyTabStop>()?;
    module.add_class::<PyTable>()?;
    module.add_class::<PyTableCollection>()?;
    module.add_class::<PyRow>()?;
    module.add_class::<PyRowCollection>()?;
    module.add_class::<PyCell>()?;
    module.add_class::<PyCellCollection>()?;
    module.add_class::<PyCellParagraphCollection>()?;
    Ok(())
}
