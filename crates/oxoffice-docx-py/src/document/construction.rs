//! Building a Document handle: from a facade document with its owner inventory, and from imported HTML or Markdown.

use super::*;

impl PyDocument {
    pub(super) fn imported_text<'py>(
        py: Python<'py>,
        text: &str,
        markdown: bool,
        template: Option<&Bound<'py, PyAny>>,
        images: &[(String, Py<PyBytes>, String)],
    ) -> PyResult<(Py<Self>, Bound<'py, PyTuple>)> {
        let resources = images
            .iter()
            .map(
                |(source, bytes, filename)| oxoffice_docx::HtmlImageResource {
                    source,
                    bytes: bytes.bind(py).as_bytes(),
                    filename,
                },
            )
            .collect::<Vec<_>>();
        let loaded;
        let borrowed;
        let template = match template {
            None => None,
            Some(value) if value.is_instance_of::<PyDocument>() => {
                borrowed = value.extract::<PyRef<'_, Self>>()?;
                Some(&borrowed.inner)
            }
            Some(value) => {
                let path = value.extract::<PathBuf>()?;
                loaded = oxoffice_docx::Document::open(path)
                    .map_err(|error| docx_to_pyerr(py, error))?;
                Some(&loaded)
            }
        };
        let result = py
            .detach(|| {
                if markdown {
                    oxoffice_docx::Document::from_markdown_with_options(text, template, &resources)
                } else {
                    oxoffice_docx::Document::from_html_with_options(text, template, &resources)
                }
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        let diagnostics = html_diagnostics(py, result.diagnostics)?;
        Ok((
            Py::new(py, Self::from_document(result.document, py)?)?,
            diagnostics,
        ))
    }
}

impl PyDocument {
    pub(super) fn from_document(inner: oxoffice_docx::Document, py: Python<'_>) -> PyResult<Self> {
        // Keep all eager fallible checks, without building dormant read facts.
        let (_, _, paragraphs, _) = inner
            .story_owner_snapshots()
            .map_err(|error| docx_to_pyerr(py, error))?;
        for (_, xml) in paragraphs {
            oxoffice_docx_oxml::CT_P::from_xml_fragment(&xml)
                .map_err(|error| docx_to_pyerr(py, error.into()))?;
        }
        Ok(Self {
            inner,
            revisions: RevisionCounter::new(),
            owners: Vec::new(),
            indexes: WordOwnerIndexes::prepare(
                &[],
                WordBodyItems::default(),
                WordIndexProjections {
                    body_paragraphs: None,
                    cell_paragraphs: HashMap::new(),
                    table_slots: Vec::new(),
                },
            )?,
            snapshot_scope: None,
            pieces: None,
            pending_pieces: None,
        })
    }
}
