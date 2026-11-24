//! The settings handle.

use super::*;

#[pymethods]
impl PySettings {
    #[getter]
    fn track_revisions(&self, py: Python<'_>) -> Option<bool> {
        self.document.borrow(py).inner.track_revisions()
    }
    #[setter]
    fn set_track_revisions(&self, py: Python<'_>, value: Option<bool>) -> PyResult<()> {
        let mut document = self.document.borrow_mut(py);
        if document.inner.track_revisions() == value {
            return Ok(());
        }
        match value {
            Some(value) => document.inner.set_track_revisions(value),
            None => document.inner.remove_track_revisions().map(|_| ()),
        }
        .map_err(|error| docx_to_pyerr(py, error))
    }
    #[getter]
    fn odd_and_even_pages_header_footer(&self, py: Python<'_>) -> bool {
        self.document.borrow(py).inner.even_and_odd_headers()
    }
    #[setter]
    fn set_odd_and_even_pages_header_footer(&self, py: Python<'_>, value: bool) -> PyResult<()> {
        let mut document = self.document.borrow_mut(py);
        if document.inner.even_and_odd_headers() == value {
            return Ok(());
        }
        // The flag selects which header a page shows; the header stories and
        // their handles stay, so the edit publishes nothing.
        document
            .inner
            .set_even_and_odd_headers(value)
            .map_err(|error| docx_to_pyerr(py, error))
    }
}
