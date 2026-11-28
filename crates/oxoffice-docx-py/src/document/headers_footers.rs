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
        let (index, kind, variant) = (self.index, self.kind, self.variant);
        let mut document = self.document.borrow_mut(py);
        document.edit_story_parts(py, "HeaderFooter.is_linked_to_previous", |document| {
            if value {
                document.inherit_section_story(index, kind, variant)
            } else if document.section_story(index, kind, variant)?.is_none() {
                document
                    .create_section_story(index, kind, variant)
                    .map(|_| ())
            } else {
                document
                    .unlink_section_story(index, kind, variant)
                    .map(|_| ())
            }
        })?;
        self.path = document
            .capture_owner(
                &WordOwnerLocation::Section(self.index),
                smallvec![],
                WordView::Content,
                None,
            )?
            .0;
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
        let (path, location, next) = {
            let mut document = self.document.borrow_mut(py);
            let style = match style {
                Some(value) => Some(style_id_of_type(
                    &document.inner,
                    value,
                    oxoffice_docx::StyleType::Paragraph,
                )?),
                None => {
                    let wanted = if self.kind == oxoffice_docx::HeaderFooterKind::Header {
                        "Header"
                    } else {
                        "Footer"
                    };
                    document
                        .inner
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
            let fragment = oxoffice_docx::ContentFragment::paragraph(paragraph)
                .map_err(|error| docx_to_pyerr(py, error))?;
            let (location, next) = self.append_block(
                py,
                &mut document,
                "HeaderFooter.add_paragraph",
                fragment,
                oxoffice_docx::StoryItemKind::Paragraph,
            )?;
            let path = document
                .revisions
                .capture(smallvec![PathSeg::Para(location.index_path()[0])]);
            (path, location, next)
        };
        self.path = next;
        let scope = self.document.borrow(py).section_child_scope(self.index)?;
        Py::new(
            py,
            PyParagraph::for_story(self.document.clone_ref(py), path, location)
                .bind_current(py, Some(scope))?,
        )
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
            // The story's tables, those of its block content controls too.
            for location in document
                .inner
                .story_tables(story.story())
                .map_err(|error| docx_to_pyerr(py, error))?
            {
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
        PyTuple::new(py, tables)
    }
    fn add_table(&mut self, py: Python<'_>, rows: usize, cols: usize) -> PyResult<Py<PyTable>> {
        self.validate(py)?;
        let fragment = related_table_fragment(rows, cols)?;
        let (path, location, next) = {
            let mut document = self.document.borrow_mut(py);
            let (location, next) = self.append_block(
                py,
                &mut document,
                "HeaderFooter.add_table",
                fragment,
                oxoffice_docx::StoryItemKind::Table,
            )?;
            let path = document
                .revisions
                .capture(smallvec![PathSeg::Body(location.index_path()[0])]);
            (path, location, next)
        };
        self.path = next;
        let scope = self.document.borrow(py).section_child_scope(self.index)?;
        Py::new(
            py,
            PyTable::for_story(self.document.clone_ref(py), path, location)
                .bind_current(py, Some(scope))?,
        )
    }
}

impl PyHeaderFooter {
    /// Append `fragment` to this header or footer, creating its story when
    /// the section has none of its own, on the live document; publish the
    /// part it wrote or added (`edit_story_parts`), and answer the location
    /// of the new block and this handle's refreshed path.
    fn append_block(
        &self,
        py: Python<'_>,
        document: &mut PyDocument,
        operation: &'static str,
        fragment: oxoffice_docx::ContentFragment,
        kind: oxoffice_docx::StoryItemKind,
    ) -> PyResult<(oxoffice_docx::ContentLocation, ContentPath)> {
        let (index, header_kind, variant) = (self.index, self.kind, self.variant);
        let insert = move |document: &mut oxoffice_docx::Document| {
            let story = match document.section_story(index, header_kind, variant)? {
                Some(story) => story.story().clone(),
                None => document.create_section_story(index, header_kind, variant)?,
            };
            let position = document.story_items(&story)?.len();
            document.insert_content(&oxoffice_docx::ContentLocation::end(story), fragment)?;
            let current = document
                .section_story(index, header_kind, variant)?
                .ok_or_else(|| {
                    oxoffice_docx::Error::Other(
                        "header/footer disappeared during insertion".to_owned(),
                    )
                })?;
            Ok(oxoffice_docx::ContentLocation::new(
                current.story().clone(),
                kind,
                vec![position],
            ))
        };
        // Under a session, the block is written as inserted on a staged copy.
        let location = match document.inner.tracks_changes() {
            true => document.staged_story_insertion(py, operation, insert)?,
            false => document.edit_story_parts(py, operation, insert)?.0,
        };
        let next = document
            .capture_owner(
                &WordOwnerLocation::Section(self.index),
                smallvec![],
                WordView::Content,
                None,
            )?
            .0;
        Ok((location, next))
    }
}
