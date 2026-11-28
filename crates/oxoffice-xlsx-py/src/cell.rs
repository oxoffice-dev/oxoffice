//! Cells and comments, as frozen snapshots read by address.

use pyo3::prelude::*;

use oxoffice_xlsx::{Cell, Comment, Value};

use crate::style::PyStyle;

/// What turns cell values into Python objects: `float`, `str`, `bool`,
/// `CellError` or `None`. The `CellError` class is looked up once.
pub struct Values<'py> {
    cell_error: Bound<'py, PyAny>,
}

impl<'py> Values<'py> {
    pub fn new(py: Python<'py>) -> PyResult<Self> {
        Ok(Values {
            cell_error: py.import("oxoffice.xlsx")?.getattr("CellError")?,
        })
    }

    pub fn convert(&self, value: &Value) -> PyResult<Bound<'py, PyAny>> {
        let py = self.cell_error.py();
        Ok(match value {
            Value::Empty => py.None().into_bound(py),
            Value::Number(number) => number.into_pyobject(py)?.into_any(),
            Value::Text(text) => text.into_pyobject(py)?.into_any(),
            Value::Boolean(boolean) => boolean.into_pyobject(py)?.to_owned().into_any(),
            Value::Error(error) => self.cell_error.call1((error.code(),))?,
        })
    }
}

/// A cell, read by its address: a snapshot an edit does not change.
#[pyclass(
    name = "Cell",
    module = "oxoffice.xlsx",
    frozen,
    eq,
    skip_from_py_object
)]
#[derive(Clone, PartialEq)]
pub struct PyCell {
    inner: Cell,
}

impl From<Cell> for PyCell {
    fn from(inner: Cell) -> Self {
        PyCell { inner }
    }
}

#[pymethods]
impl PyCell {
    /// The A1 reference, `B3`.
    #[getter(r#ref)]
    fn reference(&self) -> String {
        self.inner.address.to_string()
    }

    /// The 0-based row.
    #[getter]
    fn row(&self) -> u32 {
        self.inner.row()
    }

    /// The 0-based column.
    #[getter]
    fn column(&self) -> u32 {
        self.inner.column()
    }

    /// The value: `float`, `str`, `bool`, `CellError` or `None`; for a
    /// formula, the value its producer cached.
    #[getter]
    fn value<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        Values::new(py)?.convert(&self.inner.value)
    }

    /// The formula as Excel displays it after its `=`, or `None`.
    #[getter]
    fn formula(&self) -> Option<String> {
        self.inner.formula.clone()
    }

    /// The formula as the file stores it.
    #[getter]
    fn stored_formula(&self) -> Option<String> {
        self.inner.stored_formula.clone()
    }

    /// `"shared"`, `"array"`, `"data_table"` or `None`.
    #[getter]
    fn formula_kind(&self) -> Option<&'static str> {
        self.inner.formula_kind.map(|kind| kind.as_str())
    }

    /// The array formula's or data table's range the cell is in.
    #[getter]
    fn array_range(&self) -> Option<String> {
        self.inner.array_range.map(|range| range.to_string())
    }

    /// Whether the cached value of the formula may be out of date.
    #[getter]
    fn stale(&self) -> bool {
        self.inner.stale
    }

    /// The number format code.
    #[getter]
    fn number_format(&self) -> String {
        self.inner.number_format.clone()
    }

    /// The value as Excel displays it.
    #[getter]
    fn text(&self) -> String {
        self.inner.text.clone()
    }

    /// The resolved style.
    #[getter]
    fn style(&self) -> PyStyle {
        PyStyle::from(self.inner.style.as_ref())
    }

    /// The number as a `datetime.datetime` when the number format shows a
    /// date, a time or both, else `None`.
    #[getter]
    fn datetime<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyAny>>> {
        let Some(at) = self.inner.datetime else {
            return Ok(None);
        };
        py.import("datetime")?
            .getattr("datetime")?
            .call1((
                at.year,
                at.month,
                at.day,
                at.hour,
                at.minute,
                at.second,
                at.millisecond * 1000,
            ))
            .map(Some)
    }

    /// The number as a `datetime.timedelta` when the number format shows a
    /// duration (`[h]:mm`, `[mm]:ss`), else `None`.
    #[getter]
    fn duration<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyAny>>> {
        let Some(duration) = self.inner.duration else {
            return Ok(None);
        };
        let kwargs = pyo3::types::PyDict::new(py);
        kwargs.set_item("milliseconds", duration.as_millis() as u64)?;
        py.import("datetime")?
            .getattr("timedelta")?
            .call((), Some(&kwargs))
            .map(Some)
    }

    fn __repr__(&self) -> String {
        match &self.inner.formula {
            Some(formula) => format!(
                "<Cell {} ={formula} {:?}>",
                self.inner.address, self.inner.text
            ),
            None => format!("<Cell {} {:?}>", self.inner.address, self.inner.text),
        }
    }
}

/// A reply in a comment thread.
#[pyclass(
    name = "Reply",
    module = "oxoffice.xlsx",
    frozen,
    get_all,
    eq,
    skip_from_py_object
)]
#[derive(Clone, PartialEq)]
pub struct PyReply {
    author: Option<String>,
    date: Option<String>,
    text: String,
}

/// A note or a comment thread on a cell.
#[pyclass(
    name = "Comment",
    module = "oxoffice.xlsx",
    frozen,
    get_all,
    eq,
    skip_from_py_object
)]
#[derive(Clone, PartialEq)]
pub struct PyComment {
    #[pyo3(name = "ref")]
    reference: String,
    kind: &'static str,
    author: Option<String>,
    date: Option<String>,
    text: String,
    resolved: bool,
    replies: Vec<PyReply>,
}

#[pymethods]
impl PyComment {
    fn __repr__(&self) -> String {
        format!("<Comment {} {} {:?}>", self.reference, self.kind, self.text)
    }
}

impl From<&Comment> for PyComment {
    fn from(comment: &Comment) -> Self {
        PyComment {
            reference: comment.address.to_string(),
            kind: comment.kind.as_str(),
            author: comment.author.clone(),
            date: comment.date.clone(),
            text: comment.text.clone(),
            resolved: comment.resolved,
            replies: comment
                .replies
                .iter()
                .map(|reply| PyReply {
                    author: reply.author.clone(),
                    date: reply.date.clone(),
                    text: reply.text.clone(),
                })
                .collect(),
        }
    }
}

pub fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyCell>()?;
    module.add_class::<PyReply>()?;
    module.add_class::<PyComment>()?;
    Ok(())
}
