//! Python bindings for the oxoffice-xlsx facade, published as
//! `oxoffice.xlsx`.

use std::path::PathBuf;

use pyo3::exceptions::{PyIndexError, PyKeyError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyTuple};

use oxoffice_xlsx::{Error, SaveOptions};

const MODULE: &str = "oxoffice.xlsx";

/// The exception an error of the facade raises: a `ValueError` for an
/// argument that is not valid, else the public exception of `oxoffice.xlsx`:
/// `XmlError` (also a `ValueError`) for a part that is no well-formed
/// SpreadsheetML or a character XML cannot carry, `PackageError` for a file
/// that does not open or cannot be written, `XlsxError` for the rest.
fn xlsx_to_pyerr(py: Python<'_>, error: Error) -> PyErr {
    let message = error.to_string();
    let class = if message.contains(oxoffice_core::xml::NON_XML_CHARACTER_REFUSAL) {
        "XmlError"
    } else {
        match error {
            Error::InvalidArgument(_) => return PyValueError::new_err(message),
            Error::MalformedPart { .. } => "XmlError",
            Error::MacroFreeExtension { .. } | Error::Refused(_) => "XlsxError",
            _ => "PackageError",
        }
    };
    oxoffice_py_support::public_error(py, MODULE, class, message)
}

/// A number of the units of the argument: characters for a column width,
/// points for a row height. A `bool` or a `Length` of another package
/// (`oxoffice.docx.Pt(12)` is an EMU count, not a width) is refused.
fn number_argument(value: &Bound<'_, PyAny>, name: &str, unit: &str) -> PyResult<f64> {
    let is_length = value
        .get_type()
        .mro()
        .iter()
        .any(|class| {
            let named = class.getattr("__name__").and_then(|name| name.extract::<String>());
            let module = class.getattr("__module__").and_then(|name| name.extract::<String>());
            matches!((named, module), (Ok(name), Ok(module)) if name == "Length" && module.starts_with("oxoffice."))
        });
    if value.is_instance_of::<pyo3::types::PyBool>() || is_length {
        return Err(PyTypeError::new_err(format!(
            "{name} is a number of {unit}, not a Length or a bool"
        )));
    }
    value
        .extract::<f64>()
        .map_err(|_| PyTypeError::new_err(format!("{name} is a number of {unit}")))
}

/// The A1 text of a cell given as text or as a 0-based `(row, column)`.
fn cell_argument(value: &Bound<'_, PyAny>) -> PyResult<String> {
    if let Ok(text) = value.extract::<String>() {
        return Ok(text);
    }
    let Ok((row, column)) = value.extract::<(i64, i64)>() else {
        return Err(PyTypeError::new_err(
            "a cell is a string such as \"B2\", or a (row, column) tuple counted from 0",
        ));
    };
    if !(0..MAX_ROWS).contains(&row) || !(0..MAX_COLUMNS).contains(&column) {
        return Err(PyIndexError::new_err(format!(
            "({row}, {column}) is outside the sheet: rows 0 to {}, columns 0 to {}",
            MAX_ROWS - 1,
            MAX_COLUMNS - 1
        )));
    }
    let mut letters = Vec::new();
    let mut rest = column + 1;
    while rest > 0 {
        letters.push(b'A' + ((rest - 1) % 26) as u8);
        rest = (rest - 1) / 26;
    }
    letters.reverse();
    Ok(format!(
        "{}{}",
        String::from_utf8(letters).expect("ASCII letters"),
        row + 1
    ))
}

/// The rows and columns of a sheet.
const MAX_ROWS: i64 = 1_048_576;
const MAX_COLUMNS: i64 = 16_384;

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
    /// Opens the workbook at `path`, or makes a new one: with one empty sheet
    /// `Sheet1`, or with an empty sheet for each name of `sheets`.
    #[new]
    #[pyo3(signature = (path = None, *, sheets = None))]
    fn new(py: Python<'_>, path: Option<PathBuf>, sheets: Option<Vec<String>>) -> PyResult<Self> {
        match (path, sheets) {
            (Some(path), None) => workbook(py, py.detach(|| oxoffice_xlsx::Workbook::open(path))),
            (None, None) => Ok(PyWorkbook {
                inner: oxoffice_xlsx::Workbook::new(),
            }),
            (None, Some(names)) => workbook(py, oxoffice_xlsx::Workbook::with_sheet_names(&names)),
            (Some(_), Some(_)) => Err(PyTypeError::new_err(
                "sheets names the sheets of a new workbook: give a path or sheets, not both",
            )),
        }
    }

    /// Opens the workbook at `path`.
    #[staticmethod]
    fn open(py: Python<'_>, path: PathBuf) -> PyResult<Self> {
        Self::new(py, Some(path), None)
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

    /// Adds an empty sheet after the others and gives it.
    fn add_sheet(slf: &Bound<'_, Self>, name: &str) -> PyResult<PyWorksheet> {
        let py = slf.py();
        let relationship_id = slf
            .borrow_mut()
            .inner
            .add_sheet(name)
            .map(|sheet| sheet.relationship_id().to_owned())
            .map_err(|error| xlsx_to_pyerr(py, error))?;
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

impl PyWorksheet {
    /// `edit` of this sheet of the workbook, which changes it.
    fn edit<T>(
        &self,
        py: Python<'_>,
        edit: impl FnOnce(&mut oxoffice_xlsx::WorksheetMut<'_>) -> Result<T, Error>,
    ) -> PyResult<T> {
        let mut workbook = self.workbook.borrow_mut(py);
        let position = workbook
            .inner
            .sheets()
            .find(|sheet| sheet.relationship_id() == self.relationship_id)
            .map(|sheet| sheet.position())
            .ok_or_else(|| {
                oxoffice_py_support::public_error(
                    py,
                    MODULE,
                    "XlsxError",
                    "the sheet is no longer in the workbook".to_owned(),
                )
            })?;
        let mut sheet = workbook
            .inner
            .sheet_at_mut(position)
            .expect("the position was just found");
        edit(&mut sheet).map_err(|error| xlsx_to_pyerr(py, error))
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

    /// Sets the width of columns, in characters: `"B"`, or `"A:C"` for several.
    fn set_column_width(
        &self,
        py: Python<'_>,
        columns: &str,
        width: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let width = number_argument(width, "width", "characters")?;
        self.edit(py, |sheet| sheet.set_column_width(columns, width))
    }

    /// Sets the height of rows, in points: `"3"`, or `"2:5"` for several.
    fn set_row_height(
        &self,
        py: Python<'_>,
        rows: &str,
        height: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let height = number_argument(height, "height", "points")?;
        self.edit(py, |sheet| sheet.set_row_height(rows, height))
    }

    /// Freezes the panes above and left of `cell`, or removes them (`None`).
    #[pyo3(signature = (cell = None))]
    fn freeze_panes(&self, py: Python<'_>, cell: Option<&Bound<'_, PyAny>>) -> PyResult<()> {
        let cell = cell
            .filter(|cell| !cell.is_none())
            .map(cell_argument)
            .transpose()?;
        self.edit(py, |sheet| sheet.freeze_panes(cell.as_deref()))
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
