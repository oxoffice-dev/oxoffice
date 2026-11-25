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
    /// Reuse equivalent dependencies and rename different ones. A default
    /// character, table or numbering style under an id the destination lacks
    /// (Word's localized Policepardfaut) takes the destination's default. A
    /// default paragraph style does when it is that default under another id;
    /// a different one is imported under its own id, no longer a default: the
    /// styles based on it and the text that names it keep its look, text that
    /// names no style takes the destination's default paragraph style.
    #[staticmethod]
    fn reuse_equivalent() -> Self {
        Self {
            inner: oxoffice_docx::FragmentConflictPolicy::reuse_equivalent(),
        }
    }
    /// A fresh destination identity for every dependency, except a default
    /// style, which takes the destination's default as reuse_equivalent says.
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
        match (self.is_end, self.index_path.as_slice()) {
            (true, [control]) => Ok(oxoffice_docx::ContentLocation::control_end(story, *control)),
            (true, _) => Ok(oxoffice_docx::ContentLocation::end(story)),
            (false, _) => Ok(oxoffice_docx::ContentLocation::new(
                story,
                story_item_kind_from_name(&self.kind)?,
                self.index_path.clone(),
            )),
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
    /// The boundary after the last child of the content of the block
    /// content control that is story item `control_index` of `story`.
    #[staticmethod]
    fn control_end(story: PyRef<'_, PyStory>, control_index: usize) -> Self {
        Self {
            story: story.clone(),
            kind: "content_control".into(),
            index_path: vec![control_index],
            is_end: true,
        }
    }
    /// Whether this location is an end: of its story or of a control's
    /// content.
    #[getter]
    fn is_end(&self) -> bool {
        self.is_end
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
impl PyDocument {
    /// Insert a paragraph of `text` before the paragraph at `location`, one
    /// that is no direct body paragraph: in a block content control at any
    /// depth, a table cell, a header, footer, note, comment or text box.
    /// `segments` are the path of that paragraph's handle; the new paragraph
    /// takes its place in that order.
    pub(crate) fn insert_paragraph_at(
        slf: Py<Self>,
        py: Python<'_>,
        location: &crate::paragraph::ParagraphLocation,
        segments: smallvec::SmallVec<[PathSeg; 5]>,
        text: &str,
        style: Option<&str>,
    ) -> PyResult<Py<PyParagraph>> {
        let mut document = slf.borrow_mut(py);
        let style = style
            .map(|value| {
                style_id_of_type(&document.inner, value, oxoffice_docx::StyleType::Paragraph)
            })
            .transpose()?;
        document.refuse_non_xml_paragraph_text(py, text)?;
        let native = location
            .native(&document.inner)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let mut inserted = oxoffice_docx::Document::detached_text_paragraph(text);
        if let Some(style) = &style {
            oxoffice_docx::Paragraph::detached(&mut inserted).set_style(style);
        }
        let fragment = oxoffice_docx::ContentFragment::paragraph(inserted)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let mut candidate = document.inner.clone_for_staging();
        let at = candidate
            .block_insertion_location(&native)
            .map_err(|error| docx_to_pyerr(py, error))?;
        candidate
            .insert_content(&native, fragment)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "Paragraph.insert_paragraph_before",
            crate::document::WordPublication::Conservative,
        )?;
        let paragraph = match location {
            // In the body or a cell, the new paragraph takes the place of
            // the one it goes before.
            crate::paragraph::ParagraphLocation::Body(_)
            | crate::paragraph::ParagraphLocation::Cell { .. } => {
                let path = document
                    .revisions
                    .capture_prepared(&prepared.revision, segments)
                    .map_err(revision_to_pyerr)?;
                PyParagraph::new(slf.clone_ref(py), path)
                    .bind_prepared(py, &document, &prepared, &candidate, None)?
            }
            crate::paragraph::ParagraphLocation::Related(_) => {
                let story = candidate
                    .stories()
                    .map_err(|error| docx_to_pyerr(py, error))?
                    .into_iter()
                    .find(|story| {
                        story.kind() == at.story().kind()
                            && story.part_name() == at.story().part_name()
                            && story.owner_index() == at.story().owner_index()
                    })
                    .ok_or_else(|| PyValueError::new_err("the paragraph's story disappeared"))?;
                let at = oxoffice_docx::ContentLocation::new(
                    story,
                    oxoffice_docx::StoryItemKind::Paragraph,
                    at.index_path().to_vec(),
                );
                let path = document
                    .revisions
                    .capture_prepared(
                        &prepared.revision,
                        smallvec![PathSeg::Para(at.index_path()[0])],
                    )
                    .map_err(revision_to_pyerr)?;
                PyParagraph::for_story(slf.clone_ref(py), path, at)
                    .bind_prepared(py, &document, &prepared, &candidate, None)?
            }
        };
        let result = Py::new(py, paragraph)?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        Ok(result)
    }
}

#[pymethods]
impl PyDocumentFragment {
    #[staticmethod]
    #[pyo3(signature = (document, start, end, *, include_final_section_properties=false))]
    fn from_range(
        py: Python<'_>,
        document: &Bound<'_, PyDocument>,
        start: &Bound<'_, PyAny>,
        end: &Bound<'_, PyAny>,
        include_final_section_properties: bool,
    ) -> PyResult<Self> {
        let owner = document.clone().unbind();
        let start = PyDocument::location_argument(&owner, py, start, "start")?;
        let end = PyDocument::location_argument(&owner, py, end, "end")?;
        let document = document.borrow();
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
