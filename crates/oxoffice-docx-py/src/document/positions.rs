//! Run positions and run ranges in the body.

use super::*;

#[pyclass(name = "RunPosition", frozen, get_all, eq, skip_from_py_object)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PyRunPosition {
    pub body_index: usize,
    pub run_index: usize,
}

#[pymethods]
impl PyRunPosition {
    #[new]
    #[pyo3(signature = (*, body_index, run_index))]
    fn new(body_index: usize, run_index: usize) -> Self {
        Self {
            body_index,
            run_index,
        }
    }
}

impl From<PyRunPosition> for oxoffice_docx::RunPosition {
    fn from(value: PyRunPosition) -> Self {
        Self {
            body_index: value.body_index,
            run_index: value.run_index,
        }
    }
}

#[pyclass(name = "RunRange", frozen, eq, skip_from_py_object)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PyRunRange {
    start: PyRunPosition,
    end: PyRunPosition,
}

#[pymethods]
impl PyRunRange {
    #[new]
    #[pyo3(signature = (*, start, end))]
    fn new(start: PyRef<'_, PyRunPosition>, end: PyRef<'_, PyRunPosition>) -> Self {
        Self {
            start: *start,
            end: *end,
        }
    }

    #[getter]
    fn start(&self) -> PyRunPosition {
        self.start
    }

    #[getter]
    fn end(&self) -> PyRunPosition {
        self.end
    }
}

impl From<PyRunRange> for oxoffice_docx::RunRange {
    fn from(value: PyRunRange) -> Self {
        Self {
            start: value.start.into(),
            end: value.end.into(),
        }
    }
}

impl From<oxoffice_docx::RunRange> for PyRunRange {
    fn from(value: oxoffice_docx::RunRange) -> Self {
        let position = |position: oxoffice_docx::RunPosition| PyRunPosition {
            body_index: position.body_index,
            run_index: position.run_index,
        };
        Self {
            start: position(value.start),
            end: position(value.end),
        }
    }
}
