//! Tracked changes sessions: `Document.track_changes` and the gate that
//! keeps a session's document to the edits it records (spec of 2026-11-27,
//! writing revisions and comments).

use super::*;

/// The publications a session lets through: the edits it writes as tracked
/// changes, and those that change what Word never tracks (comments,
/// permission ranges, styles, signing, the resolution of revisions). An edit
/// that publishes nothing needs no entry: it runs untracked (a property, a
/// setting) or refuses itself under a session. Every other publication is
/// refused before it changes the document; `test_docx_tracked_changes.py`
/// runs every mutating call under a session and checks its class.
const SESSION_OPERATIONS: &[&str] = &[
    // Tracked.
    "Cell.add_paragraph",
    "Cell.add_table",
    "Cell.replace_text",
    "Cell.text",
    "Document.add_bullet_list_item",
    "Document.add_note",
    "Document.add_numbered_list_item",
    "Document.add_paragraph",
    "Document.add_table",
    "Document.clone_content",
    "Document.import_fragment",
    "Document.insert_content",
    "Document.insert_document",
    "Document.insert_html",
    "Document.insert_paragraph",
    "Document.insert_table",
    "Document.pop_content",
    "Document.remove_content",
    "Document.remove_content_range",
    "Document.replace_all",
    "Document.replace_all_regex",
    "Document.replace_all_regex_expected",
    "Document.replace_text_at",
    "Document.set_story_text",
    "Document.try_replace_text",
    "HeaderFooter.add_paragraph",
    "HeaderFooter.add_table",
    "Note.add_paragraph",
    "Note.add_table",
    "Paragraph.add_hyperlink",
    "Paragraph.add_run",
    "Paragraph.insert_paragraph_before",
    "Paragraph.replace_text",
    "Paragraph.replace_xml",
    "Paragraph.text",
    "Run.add_field",
    "Run.add_picture",
    "Run.remove",
    "Table.add_row",
    "Table.clone_row",
    "Table.remove_row",
    "Run.text",
    // Untracked, as in Word.
    "Document.accept_all",
    "Document.accept_revision_id",
    "Document.accept_revisions_by_author",
    "Document.accept_revisions_in_date_range",
    "Document.add_comment",
    "Document.add_comment_on_text",
    "Document.add_story_permission_range",
    "Document.add_style",
    "Document.move_comment",
    "Document.move_comment_to_text",
    "Document.reject_all",
    "Document.reject_revision_id",
    "Document.reject_revisions_by_author",
    "Document.reject_revisions_in_date_range",
    "Document.remove_bibliography_source",
    "Document.remove_comment",
    "Document.reply_to",
    "Document.resolve_comment",
    "Document.set_style",
    "Document.sign",
    // A run split: no content changes, so nothing is written.
    "Document.split_run",
];

fn session_gate() -> oxoffice_py_support::RevisionGate {
    oxoffice_py_support::RevisionGate {
        allows: std::sync::Arc::new(|operation| {
            operation.is_some_and(|operation| SESSION_OPERATIONS.contains(&operation))
        }),
        module: "oxoffice.docx",
        class: "DocxError",
    }
}

/// A tracked changes session of a document: its author, its date and the
/// revisions it wrote. A context manager that ends the session on exit.
#[pyclass(name = "TrackedChanges", frozen)]
pub struct PyTrackedChanges {
    document: Py<PyDocument>,
    session: oxoffice_docx::TrackedSession,
}

#[pymethods]
impl PyTrackedChanges {
    #[getter]
    fn author(&self) -> &str {
        self.session.changes().author()
    }

    #[getter]
    fn timestamp(&self) -> Option<&str> {
        self.session.changes().timestamp()
    }

    /// The ids of the revisions the session wrote, in the order it wrote them.
    #[getter]
    fn revision_ids<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let ids = self
            .session
            .revision_ids(&self.document.borrow(py).inner)
            .map_err(|error| docx_to_pyerr(py, error))?;
        PyTuple::new(py, ids)
    }

    /// Whether the document's edits still run in this session.
    #[getter]
    fn active(&self, py: Python<'_>) -> bool {
        self.document
            .borrow(py)
            .inner
            .tracked_session()
            .is_some_and(|current| current.same(&self.session))
    }

    /// End the session. Ending one that has ended does nothing.
    fn stop(&self, py: Python<'_>) {
        let mut document = self.document.borrow_mut(py);
        if document
            .inner
            .tracked_session()
            .is_some_and(|current| current.same(&self.session))
        {
            document.inner.stop_tracked_changes();
            document.revisions.set_gate(None);
        }
    }

    /// The session itself; one that has stopped refuses, since the edits of
    /// the block would run untracked.
    fn __enter__(slf: Py<Self>, py: Python<'_>) -> PyResult<Py<Self>> {
        if !slf.borrow(py).active(py) {
            return Err(docx_to_pyerr(
                py,
                oxoffice_docx::Error::Other(
                    "this tracked changes session has stopped: start another with \
                     Document.track_changes"
                        .to_owned(),
                ),
            ));
        }
        Ok(slf)
    }

    #[pyo3(signature = (*_exception))]
    fn __exit__(&self, py: Python<'_>, _exception: &Bound<'_, PyTuple>) -> bool {
        self.stop(py);
        false
    }
}

/// `Document.track_changes`: start a tracked changes session by `author`.
pub(super) fn track_changes(
    slf: &Bound<'_, PyDocument>,
    author: &str,
    timestamp: Option<&str>,
    undated: bool,
) -> PyResult<PyTrackedChanges> {
    let py = slf.py();
    let date = match (timestamp, undated) {
        (Some(_), true) => {
            return Err(PyValueError::new_err(
                "pass a timestamp or undated=True, not both",
            ));
        }
        (Some(timestamp), false) => oxoffice_docx::RevisionDate::At(timestamp),
        (None, true) => oxoffice_docx::RevisionDate::Undated,
        (None, false) => oxoffice_docx::RevisionDate::Now,
    };
    let changes = oxoffice_docx::TrackedChanges::new(author, date)
        .map_err(|error| docx_to_pyerr(py, error))?;
    let mut document = slf.borrow_mut();
    let session = document
        .inner
        .start_tracked_changes(&changes)
        .map_err(|error| docx_to_pyerr(py, error))?;
    document.revisions.set_gate(Some(session_gate()));
    Ok(PyTrackedChanges {
        document: slf.clone().unbind(),
        session,
    })
}

/// `Document.tracked_changes`: the session the document's edits run in.
pub(super) fn tracked_changes(slf: &Bound<'_, PyDocument>) -> Option<PyTrackedChanges> {
    let session = slf.borrow().inner.tracked_session()?;
    Some(PyTrackedChanges {
        document: slf.clone().unbind(),
        session,
    })
}
