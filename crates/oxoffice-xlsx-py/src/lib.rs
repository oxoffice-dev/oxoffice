//! Python bindings for the oxoffice-xlsx facade, published as
//! `oxoffice.xlsx`.

use std::path::PathBuf;

use pyo3::exceptions::PyKeyError;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyTuple};

use oxoffice_xlsx::{Error, SaveOptions};

const MODULE: &str = "oxoffice.xlsx";

/// The public exception of `oxoffice.xlsx` an error of the facade raises:
/// `XmlError` (also a `ValueError`) for a part that is no well-formed
/// SpreadsheetML or a character XML cannot carry, `PackageError` for a file
/// that does not open or cannot be written, `XlsxError` for the rest.
fn xlsx_to_pyerr(py: Python<'_>, error: Error) -> PyErr {
    let message = error.to_string();
    let class = if message.contains(oxoffice_core::xml::NON_XML_CHARACTER_REFUSAL) {
        "XmlError"
    } else {
        match error {
            Error::MalformedPart { .. } => "XmlError",
            Error::MacroFreeExtension { .. } => "XlsxError",
            _ => "PackageError",
        }
    };
    oxoffice_py_support::public_error(py, MODULE, class, message)
}

/// An Excel-compatible workbook.
#[pyclass(name = "Workbook", module = "oxoffice.xlsx")]
pub struct PyWorkbook {
    inner: oxoffice_xlsx::Workbook,
}

fn workbook(
    py: Python<'_>,
    opened: Result<oxoffice_xlsx::Workbook, Error>,
) -> PyResult<PyWorkbook> {
    opened
        .map(|inner| PyWorkbook { inner })
        .map_err(|error| xlsx_to_pyerr(py, error))
}

#[pymethods]
impl PyWorkbook {
    /// Opens the workbook at `path`.
    #[new]
    fn new(py: Python<'_>, path: PathBuf) -> PyResult<Self> {
        workbook(py, py.detach(|| oxoffice_xlsx::Workbook::open(path)))
    }

    /// Opens the workbook at `path`.
    #[staticmethod]
    fn open(py: Python<'_>, path: PathBuf) -> PyResult<Self> {
        Self::new(py, path)
    }

    /// Opens a workbook from the bytes of a file.
    #[staticmethod]
    fn from_bytes(py: Python<'_>, data: &[u8]) -> PyResult<Self> {
        workbook(py, py.detach(|| oxoffice_xlsx::Workbook::from_bytes(data)))
    }

    /// Opens the password-protected workbook at `path`.
    #[staticmethod]
    fn open_encrypted(py: Python<'_>, path: PathBuf, password: &str) -> PyResult<Self> {
        workbook(
            py,
            py.detach(|| oxoffice_xlsx::Workbook::open_encrypted(path, password)),
        )
    }

    /// Opens a password-protected workbook from the bytes of a file.
    #[staticmethod]
    fn from_encrypted_bytes(py: Python<'_>, data: &[u8], password: &str) -> PyResult<Self> {
        workbook(
            py,
            py.detach(|| oxoffice_xlsx::Workbook::from_encrypted_bytes(data, password)),
        )
    }

    /// The sheets, in the order of the workbook's tabs.
    #[getter]
    fn sheets<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyTuple>> {
        let py = slf.py();
        let ids: Vec<String> = slf
            .borrow()
            .inner
            .sheets()
            .map(|sheet| sheet.relationship_id().to_owned())
            .collect();
        let sheets = ids
            .into_iter()
            .map(|relationship_id| {
                Py::new(
                    py,
                    PyWorksheet {
                        workbook: slf.clone().unbind(),
                        relationship_id,
                    },
                )
            })
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, sheets)
    }

    /// The sheet named `name`: the exact name first, then the name in any
    /// case, as Excel compares sheet names.
    fn sheet(slf: &Bound<'_, Self>, name: &str) -> PyResult<PyWorksheet> {
        let relationship_id = slf
            .borrow()
            .inner
            .sheet(name)
            .map(|sheet| sheet.relationship_id().to_owned())
            .ok_or_else(|| PyKeyError::new_err(format!("no sheet named {name:?}")))?;
        Ok(PyWorksheet {
            workbook: slf.clone().unbind(),
            relationship_id,
        })
    }

    /// Whether dates count from 1904 rather than 1900.
    #[getter]
    fn date1904(&self) -> bool {
        self.inner.date_system() == oxoffice_xlsx::DateSystem::Date1904
    }

    /// Saves to `path`. A workbook opened from a file refuses to replace that
    /// file unless `overwrite_source` is true.
    #[pyo3(signature = (path, *, overwrite_source = false))]
    fn save(&self, py: Python<'_>, path: PathBuf, overwrite_source: bool) -> PyResult<()> {
        let options = SaveOptions { overwrite_source };
        py.detach(|| self.inner.save_with_options(path, options))
            .map_err(|error| xlsx_to_pyerr(py, error))
    }

    /// The bytes of the workbook as it stands.
    fn to_bytes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let bytes = py
            .detach(|| self.inner.to_bytes())
            .map_err(|error| xlsx_to_pyerr(py, error))?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Saves to `path` protected by `password`, refusing the file the
    /// workbook was opened from as `save` does.
    #[pyo3(signature = (path, password, *, overwrite_source = false))]
    fn save_encrypted(
        &self,
        py: Python<'_>,
        path: PathBuf,
        password: &str,
        overwrite_source: bool,
    ) -> PyResult<()> {
        let options = SaveOptions { overwrite_source };
        py.detach(|| {
            self.inner
                .save_encrypted_with_options(path, password, options)
        })
        .map_err(|error| xlsx_to_pyerr(py, error))
    }

    /// The bytes of the workbook protected by `password`.
    fn to_encrypted_bytes<'py>(
        &self,
        py: Python<'py>,
        password: &str,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let bytes = py
            .detach(|| self.inner.to_encrypted_bytes(password))
            .map_err(|error| xlsx_to_pyerr(py, error))?;
        Ok(PyBytes::new(py, &bytes))
    }

    fn __repr__(&self) -> String {
        format!("<Workbook with {} sheets>", self.inner.sheets().len())
    }
}

/// A sheet of a workbook. It names its sheet by the relationship from the
/// workbook part, so it stays valid through every edit of the workbook.
#[pyclass(name = "Worksheet", module = "oxoffice.xlsx", frozen)]
pub struct PyWorksheet {
    workbook: Py<PyWorkbook>,
    relationship_id: String,
}

impl PyWorksheet {
    /// `read` of this sheet of the workbook.
    fn with<T>(
        &self,
        py: Python<'_>,
        read: impl FnOnce(oxoffice_xlsx::Worksheet<'_>) -> T,
    ) -> PyResult<T> {
        let workbook = self.workbook.borrow(py);
        let sheet = workbook
            .inner
            .sheets()
            .find(|sheet| sheet.relationship_id() == self.relationship_id)
            .ok_or_else(|| {
                oxoffice_py_support::public_error(
                    py,
                    MODULE,
                    "XlsxError",
                    "the sheet is no longer in the workbook".to_owned(),
                )
            })?;
        Ok(read(sheet))
    }
}

#[pymethods]
impl PyWorksheet {
    /// The name, as its tab shows it.
    #[getter]
    fn name(&self, py: Python<'_>) -> PyResult<String> {
        self.with(py, |sheet| sheet.name().to_owned())
    }

    /// The 0-based position among the workbook's tabs.
    #[getter]
    fn index(&self, py: Python<'_>) -> PyResult<usize> {
        self.with(py, |sheet| sheet.position())
    }

    /// `"visible"`, `"hidden"` or `"veryHidden"`.
    #[getter]
    fn state(&self, py: Python<'_>) -> PyResult<&'static str> {
        self.with(py, |sheet| sheet.state().as_str())
    }

    /// `"worksheet"`, `"chartsheet"`, `"dialogsheet"` or `"macrosheet"`.
    #[getter]
    fn kind(&self, py: Python<'_>) -> PyResult<&'static str> {
        self.with(py, |sheet| sheet.kind().as_str())
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        self.with(py, |sheet| format!("<Worksheet {:?}>", sheet.name()))
    }
}

/// Register every class of the xlsx bindings on `module`.
pub fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyWorkbook>()?;
    module.add_class::<PyWorksheet>()?;
    Ok(())
}
