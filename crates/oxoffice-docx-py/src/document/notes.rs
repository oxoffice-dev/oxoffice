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
        // A note removed in place retires its handle alone: the document
        // epoch the handle is bound to stays.
        story
            .map_err(|error| docx_to_pyerr(py, error))?
            .ok_or_else(|| {
                crate::public_error(
                    py,
                    "StaleElementError",
                    "note no longer exists. Re-fetch the note after structural mutation."
                        .to_owned(),
                )
            })
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
        let kind = if self.endnote {
            oxoffice_docx::StoryKind::Endnote
        } else {
            oxoffice_docx::StoryKind::Footnote
        };
        if document.remove_stories_in_place(py, "Note.delete", |document| {
            Ok(document
                .remove_note_in_place(kind, self.id)?
                .map(|story| vec![story]))
        })? {
            return Ok(());
        }
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
        // The story's tables, those of its block content controls too.
        for location in document
            .inner
            .story_tables(&story)
            .map_err(|error| docx_to_pyerr(py, error))?
        {
            let path = document
                .revisions
                .capture(smallvec![PathSeg::Body(location.index_path()[0])]);
            tables.push(Py::new(
                py,
                PyTable::for_story(self.document.clone_ref(py), path, location)
                    .bind_current(py, None)?,
            )?);
        }
        PyTuple::new(py, tables)
    }
    #[pyo3(signature=(text=""))]
    fn add_paragraph(&mut self, py: Python<'_>, text: &str) -> PyResult<Py<PyParagraph>> {
        let mut paragraph = oxoffice_docx_oxml::text::CT_P::new();
        if !text.is_empty() {
            paragraph.add_run(text);
        }
        let fragment = oxoffice_docx::ContentFragment::paragraph(paragraph)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let (path, location) = self.append_block(
            py,
            fragment,
            oxoffice_docx::StoryItemKind::Paragraph,
            "Note.add_paragraph",
        )?;
        Py::new(
            py,
            PyParagraph::for_story(self.document.clone_ref(py), path, location)
                .bind_current(py, None)?,
        )
    }
    fn add_table(&mut self, py: Python<'_>, rows: usize, cols: usize) -> PyResult<Py<PyTable>> {
        let fragment = related_table_fragment(rows, cols)?;
        let (path, location) = self.append_block(
            py,
            fragment,
            oxoffice_docx::StoryItemKind::Table,
            "Note.add_table",
        )?;
        Py::new(
            py,
            PyTable::for_story(self.document.clone_ref(py), path, location)
                .bind_current(py, None)?,
        )
    }
}

impl PyNote {
    /// Append `fragment` at the end of this note on the live document and
    /// publish the note's story alone: the other owners keep their handles.
    /// A refused edit or publication leaves the document as it was. The
    /// answer is the new block's path and location, to bind as it is now.
    fn append_block(
        &mut self,
        py: Python<'_>,
        fragment: oxoffice_docx::ContentFragment,
        kind: oxoffice_docx::StoryItemKind,
        operation: &'static str,
    ) -> PyResult<(
        oxoffice_py_support::ContentPath,
        oxoffice_docx::ContentLocation,
    )> {
        let story = self.story(py)?;
        let mut document = self.document.borrow_mut(py);
        document.ensure_word_authority(py)?;
        let index = document
            .inner
            .story_items(&story)
            .map_err(|error| docx_to_pyerr(py, error))?
            .len();
        #[cfg(debug_assertions)]
        let before = document.inner.clone_for_staging();
        document.inner.hold_edit_undo();
        // An insertion that fails or panics leaves the note as it was: the
        // guard undoes it as it drops.
        let held = super::block_edits_in_place::HeldUndo {
            document: &mut document.inner,
            armed: true,
        };
        let inserted = held
            .document
            .insert_content(&oxoffice_docx::ContentLocation::end(story), fragment);
        if let Err(error) = inserted {
            drop(held);
            return Err(docx_to_pyerr(py, error));
        }
        let undo = held.take().expect("a note insertion edits in place");
        // A publication that fails or panics leaves the document as it was:
        // undo the edit before the error or the panic goes on.
        let published = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            super::paragraph_in_place::debug_fail_paragraph_splice()?;
            let story = if self.endnote {
                document.inner.endnote_story(self.id)
            } else {
                document.inner.footnote_story(self.id)
            }
            .map_err(|error| docx_to_pyerr(py, error))?
            .ok_or_else(|| PyValueError::new_err("note disappeared during insertion"))?;
            let spliced = document.splice_story_owners(
                py,
                operation,
                &story,
                #[cfg(debug_assertions)]
                &before,
            )?;
            if !spliced {
                let prepared = document.prepare_story_revision(
                    py,
                    operation,
                    &story,
                    #[cfg(debug_assertions)]
                    &before,
                )?;
                document.commit_story_revision(prepared)?;
            }
            Ok(story)
        }));
        let story = match published {
            Ok(Ok(story)) => story,
            Ok(Err(error)) => {
                document.inner.undo_edit(undo);
                return Err(error);
            }
            Err(panic) => {
                document.inner.undo_edit(undo);
                std::panic::resume_unwind(panic);
            }
        };
        let segment = match kind {
            oxoffice_docx::StoryItemKind::Table => PathSeg::Body(index),
            _ => PathSeg::Para(index),
        };
        let path = document.revisions.capture(smallvec![segment]);
        self.path = document.revisions.capture(smallvec![]);
        Ok((
            path,
            oxoffice_docx::ContentLocation::new(story, kind, vec![index]),
        ))
    }
}
