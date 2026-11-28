//! What Python writes into cells: values, dates, formulas, and the
//! `Formula` that tells a formula from a text in a bulk write.

use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyFloat, PyInt, PyString};

use oxoffice_xlsx::{CellError, CellInput, DateTime, DateValue, FormulaOptions, Value};

/// A formula, for a bulk write or a value: its text as Excel displays it
/// (an `=` accepted), or as the file stores it with `stored`; `spill` for a
/// dynamic array formula that spills from its cell.
#[pyclass(
    name = "Formula",
    module = "oxoffice.xlsx",
    frozen,
    get_all,
    eq,
    skip_from_py_object
)]
#[derive(Clone, PartialEq)]
pub struct PyFormula {
    text: String,
    stored: bool,
    spill: bool,
}

#[pymethods]
impl PyFormula {
    #[new]
    #[pyo3(signature = (text, stored = false, spill = false))]
    fn new(text: String, stored: bool, spill: bool) -> Self {
        PyFormula {
            text,
            stored,
            spill,
        }
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let text = PyString::new(py, &self.text).repr()?;
        let mut options = String::new();
        if self.stored {
            options.push_str(", stored=True");
        }
        if self.spill {
            options.push_str(", spill=True");
        }
        Ok(format!("Formula({text}{options})"))
    }
}

/// The classes a value is told by, looked up once per call.
pub struct Inputs<'py> {
    cell_error: Bound<'py, PyAny>,
    datetime: Bound<'py, PyAny>,
    date: Bound<'py, PyAny>,
    time: Bound<'py, PyAny>,
    timedelta: Bound<'py, PyAny>,
}

impl<'py> Inputs<'py> {
    pub fn new(py: Python<'py>) -> PyResult<Self> {
        let module = py.import("datetime")?;
        Ok(Inputs {
            cell_error: py.import("oxoffice.xlsx")?.getattr("CellError")?,
            datetime: module.getattr("datetime")?,
            date: module.getattr("date")?,
            time: module.getattr("time")?,
            timedelta: module.getattr("timedelta")?,
        })
    }

    /// What `value` writes: `None`, `bool`, `int` or `float`, `str`,
    /// `CellError`, `datetime`, `date`, `time`, `timedelta` or `Formula`.
    pub fn convert(&self, value: &Bound<'py, PyAny>) -> PyResult<CellInput> {
        let field = |name: &str| -> PyResult<u32> { value.getattr(name)?.extract() };
        if value.is_none() {
            return Ok(CellInput::Value(Value::Empty));
        }
        if let Ok(formula) = value.cast::<PyFormula>() {
            let formula = formula.get();
            return Ok(CellInput::Formula(
                formula.text.clone(),
                FormulaOptions {
                    stored: formula.stored,
                    spill: formula.spill,
                },
            ));
        }
        if value.is_instance_of::<PyBool>() {
            return Ok(CellInput::Value(Value::Boolean(value.extract()?)));
        }
        if value.is_instance(&self.cell_error)? {
            let code: String = value.extract()?;
            return Ok(CellInput::Value(Value::Error(CellError::from_code(&code))));
        }
        if value.is_instance_of::<PyString>() {
            return Ok(CellInput::Value(Value::Text(value.extract()?)));
        }
        if value.is_instance_of::<PyInt>() || value.is_instance_of::<PyFloat>() {
            return Ok(CellInput::Value(Value::Number(value.extract()?)));
        }
        let time_of_day = || -> PyResult<(u32, u32, u32, u32)> {
            Ok((
                field("hour")?,
                field("minute")?,
                field("second")?,
                field("microsecond")? / 1000,
            ))
        };
        if value.is_instance(&self.datetime)? {
            let (hour, minute, second, millisecond) = time_of_day()?;
            return Ok(CellInput::Date(DateValue::DateTime(DateTime {
                year: value.getattr("year")?.extract()?,
                month: field("month")?,
                day: field("day")?,
                hour,
                minute,
                second,
                millisecond,
            })));
        }
        if value.is_instance(&self.date)? {
            return Ok(CellInput::Date(DateValue::Date {
                year: value.getattr("year")?.extract()?,
                month: field("month")?,
                day: field("day")?,
            }));
        }
        if value.is_instance(&self.time)? {
            let (hour, minute, second, millisecond) = time_of_day()?;
            return Ok(CellInput::Date(DateValue::Time {
                hour,
                minute,
                second,
                millisecond,
            }));
        }
        if value.is_instance(&self.timedelta)? {
            let seconds: f64 = value.call_method0("total_seconds")?.extract()?;
            return Ok(CellInput::Date(DateValue::Duration(seconds)));
        }
        // Numbers of other libraries (`decimal.Decimal`, numpy's) that
        // `float()` takes.
        if let Ok(number) = value.extract::<f64>() {
            return Ok(CellInput::Value(Value::Number(number)));
        }
        Err(PyTypeError::new_err(format!(
            "a cell takes a number, a str, a bool, a CellError, None, a datetime, date, time or timedelta, or a Formula, not {}",
            value.get_type().name()?
        )))
    }
}

pub fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyFormula>()?;
    Ok(())
}
