//! Python bindings for the oxoffice-xlsx facade, published as
//! `oxoffice.xlsx`.

mod cell;
mod page_setup;
mod style;
mod write;

use std::path::PathBuf;

use pyo3::exceptions::{PyIndexError, PyKeyError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyList, PyTuple};

use oxoffice_xlsx::{Clear, CsvImport, CsvOptions, Error, MAX_BULK_CELLS, SaveOptions, StylePatch};

use crate::cell::{PyCell, PyComment, Values};
use crate::page_setup::PyPageSetup;
use crate::write::Inputs;

const MODULE: &str = "oxoffice.xlsx";

/// The exception an error of the facade raises: a `ValueError` for an
/// argument that is not valid (a reference that names no cell or range
/// among them), else the public exception of `oxoffice.xlsx`: `XmlError`
/// (also a `ValueError`) for a part that is no well-formed SpreadsheetML or a
/// character XML cannot carry, `PackageError` for a file that does not open
/// or cannot be written, `XlsxError` for the rest (a refused edit or read).
fn xlsx_to_pyerr(py: Python<'_>, error: Error) -> PyErr {
    let message = error.to_string();
    let class = if message.contains(oxoffice_core::xml::NON_XML_CHARACTER_REFUSAL) {
        "XmlError"
    } else {
        match error {
            Error::InvalidArgument(_) => return PyValueError::new_err(message),
            Error::MalformedPart { .. } => "XmlError",
            Error::InvalidReference { .. }
            | Error::NotACell { .. }
            | Error::RangeTooLarge { .. } => return PyValueError::new_err(message),
            Error::MacroFreeExtension { .. }
            | Error::OtherFormatExtension { .. }
            | Error::Refused(_)
            | Error::NotAWorksheet { .. } => "XlsxError",
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

    /// Whether the cached values of formulas may be out of date: the file
    /// asks for a recalculation on open, or an edit changed a workbook that
    /// holds a formula.
    #[getter]
    fn recalculation_needed(&self) -> bool {
        self.inner.recalculation_needed()
    }

    /// The workbook as Markdown: `## <name>` and a table per worksheet,
    /// headed by the column letters, each row led by its number.
    fn to_markdown(&self, py: Python<'_>) -> PyResult<String> {
        py.detach(|| self.inner.to_markdown())
            .map_err(|error| xlsx_to_pyerr(py, error))
    }

    /// Saves to `path`. A workbook opened from a file refuses to replace that
    /// file unless `overwrite_source` is true; `drop_cached_values` leaves
    /// every formula to be calculated on open.
    #[pyo3(signature = (path, *, overwrite_source = false, drop_cached_values = false))]
    fn save(
        &self,
        py: Python<'_>,
        path: PathBuf,
        overwrite_source: bool,
        drop_cached_values: bool,
    ) -> PyResult<()> {
        let options = SaveOptions {
            overwrite_source,
            drop_cached_values,
        };
        py.detach(|| self.inner.save_with_options(path, options))
            .map_err(|error| xlsx_to_pyerr(py, error))
    }

    /// The bytes of the workbook as it stands.
    #[pyo3(signature = (*, drop_cached_values = false))]
    fn to_bytes<'py>(
        &self,
        py: Python<'py>,
        drop_cached_values: bool,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let options = SaveOptions {
            drop_cached_values,
            ..SaveOptions::default()
        };
        let bytes = py
            .detach(|| self.inner.to_bytes_with_options(options))
            .map_err(|error| xlsx_to_pyerr(py, error))?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Saves to `path` protected by `password`, refusing the file the
    /// workbook was opened from as `save` does.
    #[pyo3(signature = (path, password, *, overwrite_source = false, drop_cached_values = false))]
    fn save_encrypted(
        &self,
        py: Python<'_>,
        path: PathBuf,
        password: &str,
        overwrite_source: bool,
        drop_cached_values: bool,
    ) -> PyResult<()> {
        let options = SaveOptions {
            overwrite_source,
            drop_cached_values,
        };
        py.detach(|| {
            self.inner
                .save_encrypted_with_options(path, password, options)
        })
        .map_err(|error| xlsx_to_pyerr(py, error))
    }

    /// The bytes of the workbook protected by `password`.
    #[pyo3(signature = (password, *, drop_cached_values = false))]
    fn to_encrypted_bytes<'py>(
        &self,
        py: Python<'py>,
        password: &str,
        drop_cached_values: bool,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let options = SaveOptions {
            drop_cached_values,
            ..SaveOptions::default()
        };
        let bytes = py
            .detach(|| {
                self.inner
                    .to_encrypted_bytes_with_options(password, options)
            })
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
    /// `read` of this sheet of the workbook, its error raised.
    fn read<T>(
        &self,
        py: Python<'_>,
        read: impl FnOnce(oxoffice_xlsx::Worksheet<'_>) -> Result<T, Error>,
    ) -> PyResult<T> {
        self.with(py, read)?
            .map_err(|error| xlsx_to_pyerr(py, error))
    }

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

    /// Freezes the panes above and left of `cell`, or removes them (`None`): the
    /// argument is required, so that a call without one never removes them.
    #[pyo3(signature = (cell))]
    fn freeze_panes(&self, py: Python<'_>, cell: Option<&Bound<'_, PyAny>>) -> PyResult<()> {
        let cell = cell
            .filter(|cell| !cell.is_none())
            .map(cell_argument)
            .transpose()?;
        self.edit(py, |sheet| sheet.freeze_panes(cell.as_deref()))
    }

    /// Writes `value` into `cell`: a number, a str (never a formula), a bool,
    /// a `CellError`, `None` (the content cleared), a `datetime`, `date`,
    /// `time` or `timedelta`, or a `Formula`.
    fn set_value(
        &self,
        py: Python<'_>,
        cell: &Bound<'_, PyAny>,
        value: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let cell = cell_argument(cell)?;
        let input = Inputs::new(py)?.convert(value)?;
        self.edit(py, |sheet| sheet.set_value(&cell, input))
    }

    /// Writes a formula into `cell`, as Excel displays it (an `=` accepted),
    /// or as the file stores it with `stored`; `spill` for a dynamic array
    /// formula: `True` (over its result's range, or the most the data can
    /// fill, when the formula gives it), or its range (`"J2:J5"`).
    #[pyo3(signature = (cell, formula, *, stored = false, spill = None))]
    fn set_formula(
        &self,
        py: Python<'_>,
        cell: &Bound<'_, PyAny>,
        formula: &str,
        stored: bool,
        spill: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let cell = cell_argument(cell)?;
        let spill = spill.map(write::spill_argument).transpose()?.flatten();
        let options = oxoffice_xlsx::FormulaOptions { stored, spill };
        self.edit(py, |sheet| sheet.set_formula(&cell, formula, options))
    }

    /// Writes `rows` from `cell` down and right, one sequence per row; a
    /// formula is a `Formula`, so a str is never one.
    fn write(
        &self,
        py: Python<'_>,
        cell: &Bound<'_, PyAny>,
        rows: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let cell = cell_argument(cell)?;
        if rows.is_instance_of::<pyo3::types::PyString>() {
            return Err(PyTypeError::new_err(
                "rows is a sequence of rows, each a sequence of values",
            ));
        }
        let inputs = Inputs::new(py)?;
        let mut converted = Vec::new();
        for row in rows.try_iter()? {
            let row = row?;
            if row.is_instance_of::<pyo3::types::PyString>() {
                return Err(PyTypeError::new_err(
                    "each row is a sequence of values, not a str",
                ));
            }
            let values = row
                .try_iter()
                .map_err(|_| PyTypeError::new_err("each row is a sequence of values"))?
                .map(|value| inputs.convert(&value?))
                .collect::<PyResult<Vec<_>>>()?;
            converted.push(values);
        }
        self.edit(py, |sheet| sheet.write(&cell, converted))
    }

    /// Clears `range`: its contents (values and formulas), its formats, or
    /// both.
    #[pyo3(signature = (range, *, contents = true, formats = false))]
    fn clear(&self, py: Python<'_>, range: &str, contents: bool, formats: bool) -> PyResult<()> {
        let what = match (contents, formats) {
            (true, true) => Clear::All,
            (true, false) => Clear::Contents,
            (false, true) => Clear::Formats,
            (false, false) => return Ok(()),
        };
        self.edit(py, |sheet| sheet.clear(range, what))
    }

    /// Sets the number format code of every cell of `range`.
    fn set_number_format(&self, py: Python<'_>, range: &str, code: &str) -> PyResult<()> {
        self.edit(py, |sheet| sheet.set_number_format(range, code))
    }

    /// Changes the parts of the style of every cell of `range` that are
    /// given; `None` leaves a part as it is. Whole columns or rows take the
    /// style themselves.
    #[pyo3(signature = (
        range, *, bold = None, italic = None, underline = None, strike = None,
        font_name = None, font_size = None, font_color = None, fill = None,
        border = None, border_color = None, horizontal = None, vertical = None,
        wrap_text = None, number_format = None
    ))]
    #[allow(clippy::too_many_arguments)]
    fn set_style(
        &self,
        py: Python<'_>,
        range: &str,
        bold: Option<bool>,
        italic: Option<bool>,
        underline: Option<bool>,
        strike: Option<bool>,
        font_name: Option<String>,
        font_size: Option<&Bound<'_, PyAny>>,
        font_color: Option<String>,
        fill: Option<String>,
        border: Option<String>,
        border_color: Option<String>,
        horizontal: Option<String>,
        vertical: Option<String>,
        wrap_text: Option<bool>,
        number_format: Option<String>,
    ) -> PyResult<()> {
        let font_size = font_size
            .map(|size| number_argument(size, "font_size", "points"))
            .transpose()?;
        let patch = StylePatch {
            bold,
            italic,
            underline,
            strike,
            font_name,
            font_size,
            font_color,
            fill,
            border,
            border_color,
            horizontal,
            vertical,
            wrap_text,
            number_format,
        };
        self.edit(py, |sheet| sheet.set_style(range, &patch))
    }

    /// Writes a CSV from the cell `at` (`"B3"` or a 0-based `(row,
    /// column)`, `A1` by default): `source` is its text (a str), its bytes in
    /// UTF-8, or the path of its file.
    #[pyo3(signature = (source, *, at = None, infer = true))]
    fn import_csv(
        &self,
        py: Python<'_>,
        source: &Bound<'_, PyAny>,
        at: Option<&Bound<'_, PyAny>>,
        infer: bool,
    ) -> PyResult<()> {
        let at = match at {
            Some(at) => cell_argument(at)?,
            None => "A1".to_owned(),
        };
        let bytes: Vec<u8> = if let Ok(text) = source.cast::<pyo3::types::PyString>() {
            text.to_str()?.as_bytes().to_vec()
        } else if let Ok(bytes) = source.cast::<PyBytes>() {
            bytes.as_bytes().to_vec()
        } else if let Ok(path) = source.extract::<PathBuf>() {
            std::fs::read(&path).map_err(|error| {
                oxoffice_py_support::public_error(
                    py,
                    MODULE,
                    "PackageError",
                    format!("{}: {error}", path.display()),
                )
            })?
        } else {
            return Err(PyTypeError::new_err(
                "source is the CSV as a str, its UTF-8 bytes, or the path of its file",
            ));
        };
        self.edit(py, |sheet| {
            sheet.import_csv(&at, &bytes, CsvImport { infer })
        })
    }

    /// The smallest range holding every cell with a value or a formula
    /// (`"A1:F16"`), `None` for a sheet without one.
    #[getter]
    fn used_range(&self, py: Python<'_>) -> PyResult<Option<String>> {
        self.read(py, |sheet| {
            sheet
                .used_range()
                .map(|range| range.map(|range| range.to_string()))
        })
    }

    /// The cell at `cell`: an A1 reference (`"B3"`) or a 0-based `(row,
    /// column)`.
    fn cell(&self, py: Python<'_>, cell: &Bound<'_, PyAny>) -> PyResult<PyCell> {
        if let Ok(reference) = cell.extract::<String>() {
            return self.read(py, |sheet| sheet.cell(&reference).map(PyCell::from));
        }
        let Ok((row, column)) = cell.extract::<(i64, i64)>() else {
            return Err(PyTypeError::new_err(
                "a cell is an A1 reference such as \"B3\" or a 0-based (row, column)",
            ));
        };
        if !(0..MAX_ROWS).contains(&row) || !(0..MAX_COLUMNS).contains(&column) {
            return Err(PyIndexError::new_err(format!(
                "({row}, {column}) is outside the sheet"
            )));
        }
        let (row, column) = (row as u32, column as u32);
        self.read(py, |sheet| sheet.cell_at(row, column).map(PyCell::from))
    }

    /// The values of `range` (`"A1:C10"`, `"B3"`, whole columns `"A:C"` or
    /// rows `"2:5"` clipped to the used range) as one list per row; with
    /// `text`, the displayed texts.
    #[pyo3(signature = (range, text = false))]
    fn values<'py>(
        &self,
        py: Python<'py>,
        range: &str,
        text: bool,
    ) -> PyResult<Bound<'py, PyList>> {
        if text {
            let rows = self.read(py, |sheet| displayed_texts(sheet, range))?;
            return PyList::new(py, rows);
        }
        let rows = self.read(py, |sheet| sheet.values(range))?;
        let values = Values::new(py)?;
        let list = PyList::empty(py);
        for row in &rows {
            let converted = row
                .iter()
                .map(|value| values.convert(value))
                .collect::<PyResult<Vec<_>>>()?;
            list.append(PyList::new(py, converted)?)?;
        }
        Ok(list)
    }

    /// The cells of `range` that hold a value or a formula, row by row.
    fn cells(&self, py: Python<'_>, range: &str) -> PyResult<Vec<PyCell>> {
        self.read(py, |sheet| {
            sheet
                .cells(range)
                .map(|cells| cells.into_iter().map(PyCell::from).collect())
        })
    }

    /// The merged ranges, `["A1:J1", ...]`.
    #[getter]
    fn merged_ranges(&self, py: Python<'_>) -> PyResult<Vec<String>> {
        self.read(py, |sheet| {
            sheet
                .merged_ranges()
                .map(|ranges| ranges.iter().map(ToString::to_string).collect())
        })
    }

    /// The notes and comment threads, by cell, row by row.
    #[getter]
    fn comments(&self, py: Python<'_>) -> PyResult<Vec<PyComment>> {
        self.read(py, |sheet| {
            sheet
                .comments()
                .map(|comments| comments.iter().map(PyComment::from).collect())
        })
    }

    /// The page setup.
    #[getter]
    fn page_setup(&self, py: Python<'_>) -> PyResult<PyPageSetup> {
        self.read(py, |sheet| {
            sheet.page_setup().map(|setup| PyPageSetup::from(&setup))
        })
    }

    /// The sheet as CSV: displayed texts (or stored values with `raw`) from
    /// A1 to the end of the used range, CRLF line ends.
    #[pyo3(signature = (*, raw = false, delimiter = ",", bom = false))]
    fn to_csv(&self, py: Python<'_>, raw: bool, delimiter: &str, bom: bool) -> PyResult<String> {
        let mut characters = delimiter.chars();
        let delimiter = match (characters.next(), characters.next()) {
            (Some(character), None) if !matches!(character, '"' | '\r' | '\n') => character,
            _ => {
                return Err(PyValueError::new_err(format!(
                    "delimiter is one character other than a quote or a line break, not {delimiter:?}"
                )));
            }
        };
        let options = CsvOptions {
            delimiter,
            raw,
            bom,
        };
        self.read(py, |sheet| sheet.to_csv(&options))
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        self.with(py, |sheet| format!("<Worksheet {:?}>", sheet.name()))
    }
}

/// The displayed texts of `range`, one list per row, `""` where the sheet
/// has no value.
fn displayed_texts(
    sheet: oxoffice_xlsx::Worksheet<'_>,
    range: &str,
) -> Result<Vec<Vec<String>>, Error> {
    let Some(rectangle) = sheet.range(range)? else {
        return Ok(Vec::new());
    };
    let height = rectangle.last.row - rectangle.first.row + 1;
    let width = rectangle.last.column - rectangle.first.column + 1;
    if u64::from(height) * u64::from(width) > MAX_BULK_CELLS {
        return Err(Error::RangeTooLarge {
            range: rectangle.to_string(),
        });
    }
    let mut rows = vec![vec![String::new(); width as usize]; height as usize];
    for cell in sheet.cells(range)? {
        let (row, column) = (cell.row(), cell.column());
        rows[(row - rectangle.first.row) as usize][(column - rectangle.first.column) as usize] =
            cell.text;
    }
    Ok(rows)
}

/// Register every class of the xlsx bindings on `module`.
pub fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyWorkbook>()?;
    module.add_class::<PyWorksheet>()?;
    cell::register(module)?;
    style::register(module)?;
    write::register(module)?;
    page_setup::register(module)?;
    Ok(())
}
