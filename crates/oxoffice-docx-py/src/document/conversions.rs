//! Diagnostics and results of imports, exports and validation.

use super::*;

#[pyclass(name = "HtmlDiagnostic", frozen, get_all, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyHtmlDiagnostic {
    pub location: String,
    pub property: Option<String>,
    pub message: String,
}

pub(super) fn html_diagnostics<'py>(
    py: Python<'py>,
    diagnostics: Vec<oxoffice_docx::HtmlDiagnostic>,
) -> PyResult<Bound<'py, PyTuple>> {
    let records = diagnostics
        .into_iter()
        .map(|value| {
            Py::new(
                py,
                PyHtmlDiagnostic {
                    location: value.location,
                    property: value.property,
                    message: value.message,
                },
            )
        })
        .collect::<PyResult<Vec<_>>>()?;
    PyTuple::new(py, records)
}

pub(super) fn read_import_text(path: PathBuf) -> PyResult<String> {
    use std::io::Read;
    const LIMIT: u64 = 64 * 1024 * 1024;
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > LIMIT {
        return Err(PyValueError::new_err(
            "HTML or Markdown input exceeds the 64 MiB limit",
        ));
    }
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LIMIT {
        return Err(PyValueError::new_err(
            "HTML or Markdown input exceeds the 64 MiB limit",
        ));
    }
    String::from_utf8(bytes)
        .map_err(|_| PyValueError::new_err("HTML or Markdown input must be UTF-8"))
}

#[pyclass(name = "OdtDiagnostic", frozen, get_all, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyOdtDiagnostic {
    pub path: String,
    pub message: String,
}

#[pyclass(name = "RtfDiagnostic", frozen, get_all, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyRtfDiagnostic {
    pub offset: usize,
    pub destination: Option<String>,
    pub message: String,
}

#[pyclass(name = "EpubDiagnostic", frozen, get_all, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyEpubDiagnostic {
    pub path: String,
    pub message: String,
}

#[pyclass(name = "TextDiagnostic", frozen, get_all, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyTextDiagnostic {
    pub message: String,
}

#[pyclass(name = "ValidationFinding", frozen, get_all, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyValidationFinding {
    pub severity: String,
    pub message: String,
}
#[pymethods]
impl PyOdtExportResult {
    #[getter]
    fn bytes<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.bytes)
    }
    #[getter]
    fn diagnostics<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, self.diagnostics.iter().cloned())
    }
}
#[pymethods]
impl PyRtfExportResult {
    #[getter]
    fn bytes<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.bytes)
    }
    #[getter]
    fn diagnostics<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, self.diagnostics.iter().cloned())
    }
}
#[pymethods]
impl PyEpubExportResult {
    #[getter]
    fn bytes<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.bytes)
    }
    #[getter]
    fn diagnostics<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, self.diagnostics.iter().cloned())
    }
}

#[pyclass(name = "TextReport", frozen, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyTextReport {
    text: String,
    scope: String,
    revision_view: String,
    diagnostics: Vec<PyTextDiagnostic>,
}
#[pymethods]
impl PyTextReport {
    #[getter]
    fn text(&self) -> &str {
        &self.text
    }
    #[getter]
    fn scope(&self) -> &str {
        &self.scope
    }
    #[getter]
    fn revision_view(&self) -> &str {
        &self.revision_view
    }
    #[getter]
    fn diagnostics<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, self.diagnostics.iter().cloned())
    }
}
impl From<oxoffice_docx::TextReport> for PyTextReport {
    fn from(report: oxoffice_docx::TextReport) -> Self {
        Self {
            text: report.text,
            scope: report.scope.to_owned(),
            revision_view: report.revision_view.to_owned(),
            diagnostics: report
                .diagnostics
                .into_iter()
                .map(|d| PyTextDiagnostic { message: d.message })
                .collect(),
        }
    }
}
#[pyclass(name = "ValidationReport", frozen, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyValidationReport {
    findings: Vec<PyValidationFinding>,
}
impl From<oxoffice_docx::ValidationReport> for PyValidationReport {
    fn from(report: oxoffice_docx::ValidationReport) -> Self {
        Self {
            findings: report
                .findings
                .into_iter()
                .map(|finding| PyValidationFinding {
                    severity: match finding.severity {
                        oxoffice_docx::ValidationSeverity::Error => "error",
                        oxoffice_docx::ValidationSeverity::Warning => "warning",
                    }
                    .to_owned(),
                    message: finding.message,
                })
                .collect(),
        }
    }
}
#[pymethods]
impl PyValidationReport {
    #[getter]
    fn findings<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, self.findings.iter().cloned())
    }
    #[getter]
    fn is_valid(&self) -> bool {
        self.findings
            .iter()
            .all(|finding| finding.severity != "error")
    }
}
#[pyfunction]
pub fn validate_bytes(data: &[u8], py: Python<'_>) -> PyResult<PyValidationReport> {
    py.detach(|| oxoffice_docx::validate_document_bytes(data))
        .map(Into::into)
        .map_err(|error| docx_to_pyerr(py, error))
}
#[pyfunction]
pub fn validate_file(path: PathBuf, py: Python<'_>) -> PyResult<PyValidationReport> {
    py.detach(|| oxoffice_docx::validate_document_file(path))
        .map(Into::into)
        .map_err(|error| docx_to_pyerr(py, error))
}
