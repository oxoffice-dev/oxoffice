//! Footnote and endnote handles.

use super::*;

impl PyNote {
    fn story(&self, py: Python<'_>) -> PyResult<oxoffice_docx::StoryId> {
        let document = self.document.borrow(py);
        self.path
            .validate_scopes(
                &document.revisions,
                "note",
                "Re-fetch the note after structural mutation.",
            )
            .map_err(|error| crate::stale_to_pyerr(py, error))?;
        let story = if self.endnote {
            document.inner.endnote_story(self.id)
        } else {
            document.inner.footnote_story(self.id)
        };
        story
            .map_err(|error| docx_to_pyerr(py, error))?
            .ok_or_else(|| PyValueError::new_err("note owner no longer exists"))
    }
}
#[pymethods]
impl PyNote {
    #[getter]
    fn id(&self) -> i32 {
        self.id
    }
    #[getter]
    fn paragraphs<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let story = self.story(py)?;
        let document = self.document.borrow(py);
        let mut paragraphs = Vec::new();
        for item in document
            .inner
            .story_items(&story)
            .map_err(|error| docx_to_pyerr(py, error))?
        {
            if item.kind() == oxoffice_docx::StoryItemKind::Paragraph {
                let location = item.location().clone();
                let path = document
                    .revisions
                    .capture(smallvec![PathSeg::Para(location.index_path()[0])]);
                paragraphs.push(Py::new(
                    py,
                    PyParagraph::for_story(self.document.clone_ref(py), path, location)
                        .bind_current(py, None)?,
                )?);
            }
        }
        PyTuple::new(py, paragraphs)
    }
    fn delete(&self, py: Python<'_>) -> PyResult<()> {
        self.story(py)?;
        let mut document = self.document.borrow_mut(py);
        let mut candidate = document.inner.clone_for_staging();
        if self.endnote {
            candidate.remove_endnote(self.id)
        } else {
            candidate.remove_footnote(self.id)
        }
        .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "Note.delete",
            crate::document::WordPublication::Conservative,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        Ok(())
    }
    #[getter]
    fn tables<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let story = self.story(py)?;
        let document = self.document.borrow(py);
        let mut tables = Vec::new();
        for item in document
            .inner
            .story_items(&story)
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
                        .bind_current(py, None)?,
                )?);
            }
        }
        PyTuple::new(py, tables)
    }
    #[pyo3(signature=(text=""))]
    fn add_paragraph(&mut self, py: Python<'_>, text: &str) -> PyResult<Py<PyParagraph>> {
        let story = self.story(py)?;
        let mut paragraph = oxoffice_docx_oxml::text::CT_P::new();
        if !text.is_empty() {
            paragraph.add_run(text);
        }
        let mut document = self.document.borrow_mut(py);
        let mut candidate = document.inner.clone_for_staging();
        let index = candidate
            .story_items(&story)
            .map_err(|error| docx_to_pyerr(py, error))?
            .len();
        candidate
            .insert_content(
                &oxoffice_docx::ContentLocation::end(story),
                oxoffice_docx::ContentFragment::paragraph(paragraph)
                    .map_err(|error| docx_to_pyerr(py, error))?,
            )
            .map_err(|error| docx_to_pyerr(py, error))?;
        let story = if self.endnote {
            candidate.endnote_story(self.id)
        } else {
            candidate.footnote_story(self.id)
        }
        .map_err(|error| docx_to_pyerr(py, error))?
        .ok_or_else(|| PyValueError::new_err("note disappeared during insertion"))?;
        let location = oxoffice_docx::ContentLocation::new(
            story,
            oxoffice_docx::StoryItemKind::Paragraph,
            vec![index],
        );
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "Note.add_paragraph",
            crate::document::WordPublication::Conservative,
        )?;
        let path = document
            .revisions
            .capture_prepared(&prepared.revision, smallvec![PathSeg::Para(index)])
            .map_err(revision_to_pyerr)?;
        let next = document
            .revisions
            .capture_prepared(&prepared.revision, smallvec![])
            .map_err(revision_to_pyerr)?;
        let result = Py::new(
            py,
            PyParagraph::for_story(self.document.clone_ref(py), path, location)
                .bind_prepared(py, &document, &prepared, &candidate, None)?,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        self.path = next;
        Ok(result)
    }
    fn add_table(&mut self, py: Python<'_>, rows: usize, cols: usize) -> PyResult<Py<PyTable>> {
        let story = self.story(py)?;
        let fragment = related_table_fragment(rows, cols)?;
        let mut document = self.document.borrow_mut(py);
        let mut candidate = document.inner.clone_for_staging();
        let index = candidate
            .story_items(&story)
            .map_err(|error| docx_to_pyerr(py, error))?
            .len();
        candidate
            .insert_content(&oxoffice_docx::ContentLocation::end(story), fragment)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let story = if self.endnote {
            candidate.endnote_story(self.id)
        } else {
            candidate.footnote_story(self.id)
        }
        .map_err(|error| docx_to_pyerr(py, error))?
        .ok_or_else(|| PyValueError::new_err("note disappeared during insertion"))?;
        let location = oxoffice_docx::ContentLocation::new(
            story,
            oxoffice_docx::StoryItemKind::Table,
            vec![index],
        );
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "Note.add_table",
            crate::document::WordPublication::Conservative,
        )?;
        let path = document
            .revisions
            .capture_prepared(&prepared.revision, smallvec![PathSeg::Body(index)])
            .map_err(revision_to_pyerr)?;
        let next = document
            .revisions
            .capture_prepared(&prepared.revision, smallvec![])
            .map_err(revision_to_pyerr)?;
        let result = Py::new(
            py,
            PyTable::for_story(self.document.clone_ref(py), path, location)
                .bind_prepared(py, &document, &prepared, &candidate, None)?,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        self.path = next;
        Ok(result)
    }
}
