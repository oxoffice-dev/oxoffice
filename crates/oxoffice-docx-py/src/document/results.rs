//! Results of rendering, layout, comparison and field updates.

use super::*;

#[pyclass(
    name = "ComparisonDiagnostic",
    frozen,
    get_all,
    eq,
    skip_from_py_object
)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyComparisonDiagnostic {
    pub location: String,
    pub message: String,
}

#[pymethods]
impl PyComparisonDiagnostic {
    #[new]
    #[pyo3(signature = (*, location, message))]
    fn new(location: String, message: String) -> Self {
        Self { location, message }
    }
}

#[pyclass(name = "SvgDiagnostic", frozen, get_all, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PySvgDiagnostic {
    pub path: String,
    pub message: String,
}

#[pymethods]
impl PySvgDiagnostic {
    #[new]
    #[pyo3(signature = (*, path, message))]
    fn new(path: String, message: String) -> Self {
        Self { path, message }
    }
}

#[pymethods]
impl PySvgRenderResult {
    #[new]
    #[pyo3(signature = (*, svg, diagnostics))]
    fn new(svg: String, diagnostics: Vec<PyRef<'_, PySvgDiagnostic>>) -> Self {
        Self {
            svg,
            diagnostics: diagnostics
                .iter()
                .map(|diagnostic| (**diagnostic).clone())
                .collect(),
        }
    }

    #[getter]
    fn svg(&self) -> &str {
        &self.svg
    }

    #[getter]
    fn diagnostics<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, self.diagnostics.iter().cloned())
    }
}

#[pyclass(name = "BoundingBox", frozen, get_all, eq, skip_from_py_object)]
#[derive(Clone, Copy, PartialEq)]
pub struct PyBoundingBox {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[pymethods]
impl PyBoundingBox {
    #[new]
    #[pyo3(signature = (*, x, y, width, height))]
    fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

#[pymethods]
impl PyLayoutFragment {
    #[new]
    #[pyo3(signature = (*, body_index, physical_page, displayed_page, bounds))]
    fn new(
        body_index: usize,
        physical_page: usize,
        displayed_page: usize,
        bounds: PyRef<'_, PyBoundingBox>,
    ) -> Self {
        Self {
            body_index,
            physical_page,
            displayed_page,
            bounds: *bounds,
        }
    }

    #[getter]
    fn body_index(&self) -> usize {
        self.body_index
    }

    #[getter]
    fn physical_page(&self) -> usize {
        self.physical_page
    }

    #[getter]
    fn displayed_page(&self) -> usize {
        self.displayed_page
    }

    #[getter]
    fn bounds(&self) -> PyBoundingBox {
        self.bounds
    }
}

#[pyclass(name = "LayoutPage", frozen, get_all, eq, skip_from_py_object)]
#[derive(Clone, Copy, PartialEq)]
pub struct PyLayoutPage {
    pub page_number: usize,
    pub displayed_page_number: usize,
    pub width: f64,
    pub height: f64,
}

#[pymethods]
impl PyLayoutPage {
    #[new]
    #[pyo3(signature = (*, page_number, displayed_page_number, width, height))]
    fn new(page_number: usize, displayed_page_number: usize, width: f64, height: f64) -> Self {
        Self {
            page_number,
            displayed_page_number,
            width,
            height,
        }
    }
}

#[pymethods]
impl PyTocRebuildReport {
    #[new]
    #[pyo3(signature = (*, entry_count, bookmark_count, diagnostics))]
    fn new(entry_count: usize, bookmark_count: usize, diagnostics: Vec<String>) -> Self {
        Self {
            entry_count,
            bookmark_count,
            diagnostics,
        }
    }

    #[getter]
    fn entry_count(&self) -> usize {
        self.entry_count
    }

    #[getter]
    fn bookmark_count(&self) -> usize {
        self.bookmark_count
    }

    #[getter]
    fn diagnostics<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, &self.diagnostics)
    }

    #[getter]
    fn diagnostic_count(&self) -> usize {
        self.diagnostics.len()
    }
}

#[pymethods]
impl PyLayoutBackedFieldUpdateReport {
    #[new]
    #[pyo3(signature = (*, page_fields, num_pages_fields, page_reference_fields, diagnostics, section_fields=0, section_pages_fields=0))]
    fn new(
        page_fields: usize,
        num_pages_fields: usize,
        page_reference_fields: usize,
        diagnostics: Vec<String>,
        section_fields: usize,
        section_pages_fields: usize,
    ) -> Self {
        Self {
            page_fields,
            num_pages_fields,
            page_reference_fields,
            section_fields,
            section_pages_fields,
            diagnostics,
        }
    }

    #[getter]
    fn page_fields(&self) -> usize {
        self.page_fields
    }

    #[getter]
    fn num_pages_fields(&self) -> usize {
        self.num_pages_fields
    }

    #[getter]
    fn page_reference_fields(&self) -> usize {
        self.page_reference_fields
    }

    #[getter]
    fn section_fields(&self) -> usize {
        self.section_fields
    }

    #[getter]
    fn section_pages_fields(&self) -> usize {
        self.section_pages_fields
    }

    #[getter]
    fn updated_count(&self) -> usize {
        self.page_fields
            + self.num_pages_fields
            + self.page_reference_fields
            + self.section_fields
            + self.section_pages_fields
    }

    #[getter]
    fn diagnostics<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, &self.diagnostics)
    }

    #[getter]
    fn diagnostic_count(&self) -> usize {
        self.diagnostics.len()
    }
}
