//! Header and footer handles and their variants.

use super::*;

#[pymethods]
impl PyHeaderFooterVariant {
    #[new]
    #[pyo3(signature = (*, section_index, kind, variant, story, source_section, inherited))]
    fn new(
        section_index: usize,
        kind: String,
        variant: String,
        story: Option<PyRef<'_, PyStory>>,
        source_section: Option<usize>,
        inherited: bool,
    ) -> Self {
        Self {
            section_index,
            kind,
            variant,
            story: story.map(|value| value.clone()),
            source_section,
            inherited,
        }
    }

    #[getter]
    fn section_index(&self) -> usize {
        self.section_index
    }

    #[getter]
    fn kind(&self) -> &str {
        &self.kind
    }

    #[getter]
    fn variant(&self) -> &str {
        &self.variant
    }

    #[getter]
    fn story(&self) -> Option<PyStory> {
        self.story.clone()
    }

    #[getter]
    fn source_section(&self) -> Option<usize> {
        self.source_section
    }

    #[getter]
    fn inherited(&self) -> bool {
        self.inherited
    }
}

impl PyHeaderFooter {
    fn validate(&self, py: Python<'_>) -> PyResult<()> {
        self.path
            .validate_scopes(
                &self.document.borrow(py).revisions,
                "header/footer",
                "Re-fetch it from document.section(index).",
            )
            .map_err(|error| crate::stale_to_pyerr(py, error))?;
        Ok(())
    }
}

#[pymethods]
impl PyHeaderFooter {
    #[getter]
    fn is_linked_to_previous(&self, py: Python<'_>) -> PyResult<bool> {
        self.validate(py)?;
        Ok(self
            .document
            .borrow(py)
            .inner
            .section_story(self.index, self.kind, self.variant)
            .map_err(|error| docx_to_pyerr(py, error))?
            .is_none_or(|story| story.is_inherited()))
    }
    #[setter]
    fn set_is_linked_to_previous(&mut self, py: Python<'_>, value: bool) -> PyResult<()> {
        self.validate(py)?;
        if self.is_linked_to_previous(py)? == value {
            return Ok(());
        }
        let mut document = self.document.borrow_mut(py);
        let mut candidate = document.inner.clone_for_staging();
        if value {
            candidate.inherit_section_story(self.index, self.kind, self.variant)
        } else if candidate
            .section_story(self.index, self.kind, self.variant)
            .map_err(|error| docx_to_pyerr(py, error))?
            .is_none()
        {
            candidate
                .create_section_story(self.index, self.kind, self.variant)
                .map(|_| ())
        } else {
            candidate
                .unlink_section_story(self.index, self.kind, self.variant)
                .map(|_| ())
        }
        .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "HeaderFooter.is_linked_to_previous",
            crate::document::WordPublication::Conservative,
        )?;
        let next = document
            .capture_prepared_owner(
                &prepared,
                &WordOwnerLocation::Section(self.index),
                smallvec![],
                WordView::Content,
                None,
            )?
            .0;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        self.path = next;
        Ok(())
    }
    #[getter]
    fn paragraphs<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        self.validate(py)?;
        let document = self.document.borrow(py);
        let mut result = Vec::new();
        if let Some(story) = document
            .inner
            .section_story(self.index, self.kind, self.variant)
            .map_err(|error| docx_to_pyerr(py, error))?
        {
            for item in document
                .inner
                .story_items(story.story())
                .map_err(|error| docx_to_pyerr(py, error))?
            {
                if item.kind() == oxoffice_docx::StoryItemKind::Paragraph {
                    let location = item.location().clone();
                    let path = document
                        .revisions
                        .capture(smallvec![PathSeg::Para(location.index_path()[0])]);
                    result.push(Py::new(
                        py,
                        PyParagraph::for_story(self.document.clone_ref(py), path, location)
                            .bind_current(py, Some(document.section_child_scope(self.index)?))?,
                    )?);
                }
            }
        }
        PyTuple::new(py, result)
    }
    #[pyo3(signature = (text = "", style = None))]
    fn add_paragraph(
        &mut self,
        py: Python<'_>,
        text: &str,
        style: Option<&str>,
    ) -> PyResult<Py<PyParagraph>> {
        self.validate(py)?;
        let mut document = self.document.borrow_mut(py);
        let mut candidate = document.inner.clone_for_staging();
        let style = match style {
            Some(value) => Some(style_id_of_type(
                &candidate,
                value,
                oxoffice_docx::StyleType::Paragraph,
            )?),
            None => {
                let wanted = if self.kind == oxoffice_docx::HeaderFooterKind::Header {
                    "Header"
                } else {
                    "Footer"
                };
                candidate
                    .styles()
                    .into_iter()
                    .find(|style| {
                        style.style_type() == oxoffice_docx::StyleType::Paragraph
                            && (style.style_id() == wanted
                                || style
                                    .name()
                                    .is_some_and(|name| name.eq_ignore_ascii_case(wanted)))
                    })
                    .map(|style| style.style_id().to_owned())
            }
        };
        let story = match candidate
            .section_story(self.index, self.kind, self.variant)
            .map_err(|error| docx_to_pyerr(py, error))?
        {
            Some(story) => story.story().clone(),
            None => candidate
                .create_section_story(self.index, self.kind, self.variant)
                .map_err(|error| docx_to_pyerr(py, error))?,
        };
        let mut paragraph = oxoffice_docx_oxml::text::CT_P::new();
        if !text.is_empty() {
            paragraph.add_run(text);
        }
        if let Some(style) = style {
            paragraph.properties = Some(oxoffice_docx::CT_PPr {
                style_id: Some(style),
                ..Default::default()
            });
        }
        let index = candidate
            .story_items(&story)
            .map_err(|error| docx_to_pyerr(py, error))?
            .len();
        candidate
            .insert_content(
                &oxoffice_docx::ContentLocation::end(story.clone()),
                oxoffice_docx::ContentFragment::paragraph(paragraph)
                    .map_err(|error| docx_to_pyerr(py, error))?,
            )
            .map_err(|error| docx_to_pyerr(py, error))?;
        let current = candidate
            .section_story(self.index, self.kind, self.variant)
            .map_err(|error| docx_to_pyerr(py, error))?
            .ok_or_else(|| PyValueError::new_err("header/footer disappeared during insertion"))?;
        let location = oxoffice_docx::ContentLocation::new(
            current.story().clone(),
            oxoffice_docx::StoryItemKind::Paragraph,
            vec![index],
        );
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "HeaderFooter.add_paragraph",
            crate::document::WordPublication::HeaderFooterAppend(self.index),
        )?;
        let path = document
            .revisions
            .capture_prepared(&prepared.revision, smallvec![PathSeg::Para(index)])
            .map_err(revision_to_pyerr)?;
        let next = document
            .capture_prepared_owner(
                &prepared,
                &WordOwnerLocation::Section(self.index),
                smallvec![],
                WordView::Content,
                None,
            )?
            .0;
        let result = Py::new(
            py,
            PyParagraph::for_story(self.document.clone_ref(py), path, location).bind_prepared(
                py,
                &document,
                &prepared,
                &candidate,
                Some(document.section_child_scope(self.index)?),
            )?,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        self.path = next;
        Ok(result)
    }
    #[getter]
    fn tables<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        self.validate(py)?;
        let document = self.document.borrow(py);
        let mut tables = Vec::new();
        if let Some(story) = document
            .inner
            .section_story(self.index, self.kind, self.variant)
            .map_err(|error| docx_to_pyerr(py, error))?
        {
            for item in document
                .inner
                .story_items(story.story())
                .map_err(|error| docx_to_pyerr(py, error))?
            {
                if item.kind() == oxoffice_docx::StoryItemKind::Table {
                    let location = item.location().clone();
                    let path = document
                        .revisions
                        .capture(smallvec![PathSeg::Body(location.index_path()[0])]);
                    tables.push(Py::new(
                        py,
                        PyTable::for_story(self.document.clone_ref(py), path, location)
                            .bind_current(py, Some(document.section_child_scope(self.index)?))?,
                    )?);
                }
            }
        }
        PyTuple::new(py, tables)
    }
    fn add_table(&mut self, py: Python<'_>, rows: usize, cols: usize) -> PyResult<Py<PyTable>> {
        self.validate(py)?;
        let fragment = related_table_fragment(rows, cols)?;
        let mut document = self.document.borrow_mut(py);
        let mut candidate = document.inner.clone_for_staging();
        let story = match candidate
            .section_story(self.index, self.kind, self.variant)
            .map_err(|error| docx_to_pyerr(py, error))?
        {
            Some(story) => story.story().clone(),
            None => candidate
                .create_section_story(self.index, self.kind, self.variant)
                .map_err(|error| docx_to_pyerr(py, error))?,
        };
        let index = candidate
            .story_items(&story)
            .map_err(|error| docx_to_pyerr(py, error))?
            .len();
        candidate
            .insert_content(&oxoffice_docx::ContentLocation::end(story), fragment)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let story = candidate
            .section_story(self.index, self.kind, self.variant)
            .map_err(|error| docx_to_pyerr(py, error))?
            .ok_or_else(|| PyValueError::new_err("header/footer disappeared during insertion"))?;
        let location = oxoffice_docx::ContentLocation::new(
            story.story().clone(),
            oxoffice_docx::StoryItemKind::Table,
            vec![index],
        );
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "HeaderFooter.add_table",
            crate::document::WordPublication::Conservative,
        )?;
        let path = document
            .revisions
            .capture_prepared(&prepared.revision, smallvec![PathSeg::Body(index)])
            .map_err(revision_to_pyerr)?;
        let next = document
            .capture_prepared_owner(
                &prepared,
                &WordOwnerLocation::Section(self.index),
                smallvec![],
                WordView::Content,
                None,
            )?
            .0;
        let result = Py::new(
            py,
            PyTable::for_story(self.document.clone_ref(py), path, location).bind_prepared(
                py,
                &document,
                &prepared,
                &candidate,
                Some(document.section_child_scope(self.index)?),
            )?,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        self.path = next;
        Ok(result)
    }
}
