//! Content and document fragments, their conflict policy and content locations.

use super::*;

#[pymethods]
impl PyContentFragment {
    #[getter]
    fn kind(&self) -> &'static str {
        story_item_kind_name(self.inner.kind())
    }
}
#[pymethods]
impl PyFragmentConflictPolicy {
    #[staticmethod]
    fn reuse_equivalent() -> Self {
        Self {
            inner: oxoffice_docx::FragmentConflictPolicy::reuse_equivalent(),
        }
    }
    #[staticmethod]
    fn rename_all() -> Self {
        Self {
            inner: oxoffice_docx::FragmentConflictPolicy::rename_all(),
        }
    }
    fn with_style_reuse(&self, reuse: bool) -> Self {
        Self {
            inner: self.inner.with_style_reuse(reuse),
        }
    }
    fn with_numbering_reuse(&self, reuse: bool) -> Self {
        Self {
            inner: self.inner.with_numbering_reuse(reuse),
        }
    }
    fn with_related_part_reuse(&self, reuse: bool) -> Self {
        Self {
            inner: self.inner.with_related_part_reuse(reuse),
        }
    }
    /// Select whether imported content takes the destination's look: a style
    /// the destination defines under the same id and type keeps the destination's
    /// definition, and the source's document defaults are not carried.
    fn with_destination_formatting(&self, keep: bool) -> Self {
        Self {
            inner: self.inner.with_destination_formatting(keep),
        }
    }
}
impl PyContentLocation {
    pub(super) fn native(
        &self,
        py: Python<'_>,
        document: &PyDocument,
    ) -> PyResult<oxoffice_docx::ContentLocation> {
        let story = document.native_story(py, &self.story)?;
        if self.is_end {
            Ok(oxoffice_docx::ContentLocation::end(story))
        } else {
            Ok(oxoffice_docx::ContentLocation::new(
                story,
                story_item_kind_from_name(&self.kind)?,
                self.index_path.clone(),
            ))
        }
    }
}
#[pymethods]
impl PyContentLocation {
    #[new]
    fn new(story: PyRef<'_, PyStory>, item_kind: String, index_path: Vec<usize>) -> PyResult<Self> {
        story_item_kind_from_name(&item_kind)?;
        Ok(Self {
            story: story.clone(),
            kind: item_kind,
            index_path,
            is_end: false,
        })
    }
    #[staticmethod]
    fn end(story: PyRef<'_, PyStory>) -> Self {
        Self {
            story: story.clone(),
            kind: "preserved_node".into(),
            index_path: Vec::new(),
            is_end: true,
        }
    }
    #[getter]
    fn story(&self) -> PyStory {
        self.story.clone()
    }
    #[getter]
    fn item_kind(&self) -> &str {
        &self.kind
    }
    #[getter]
    fn index_path<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, &self.index_path)
    }
}
#[pymethods]
impl PyDocumentFragment {
    #[staticmethod]
    #[pyo3(signature = (document, start, end, *, include_final_section_properties=false))]
    fn from_range(
        py: Python<'_>,
        document: PyRef<'_, PyDocument>,
        start: PyRef<'_, PyContentLocation>,
        end: PyRef<'_, PyContentLocation>,
        include_final_section_properties: bool,
    ) -> PyResult<Self> {
        let start = start.native(py, &document)?;
        let end = end.native(py, &document)?;
        oxoffice_docx::DocumentFragment::from_range(
            &document.inner,
            &start,
            &end,
            include_final_section_properties,
        )
        .map(|inner| Self { inner })
        .map_err(|e| docx_to_pyerr(py, e))
    }
}
