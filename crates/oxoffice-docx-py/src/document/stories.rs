//! Stories and their items, paragraphs, run positions and run ranges.

use super::*;

#[pymethods]
impl PyStory {
    #[getter]
    fn kind(&self) -> &str {
        &self.kind
    }
    #[getter]
    fn part_name(&self) -> &str {
        &self.part_name
    }
    #[getter]
    fn owner_index(&self) -> usize {
        self.owner_index
    }
    #[new]
    #[pyo3(signature = (*, kind, part_name, owner_index))]
    fn new(kind: String, part_name: String, owner_index: usize) -> Self {
        Self {
            kind,
            part_name,
            owner_index,
            binding: None,
        }
    }
}

#[pymethods]
impl PyStoryItem {
    #[new]
    #[pyo3(signature = (*, story, kind, index_path, text, xml=None, direct_body_index=None, revision=0))]
    fn new(
        story: PyRef<'_, PyStory>,
        kind: String,
        index_path: Vec<usize>,
        text: Option<String>,
        xml: Option<&[u8]>,
        direct_body_index: Option<usize>,
        revision: u64,
    ) -> Self {
        Self {
            story: story.clone(),
            kind,
            index_path,
            direct_body_index,
            text,
            xml: xml.unwrap_or_default().to_vec(),
            revision,
            binding: None,
            document: None,
        }
    }

    #[getter]
    fn story(&self) -> PyStory {
        self.story.clone()
    }

    #[getter]
    fn kind(&self) -> &str {
        &self.kind
    }

    #[getter]
    fn index_path<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, self.index_path.iter().copied())
    }

    #[getter]
    fn direct_body_index(&self) -> Option<usize> {
        self.direct_body_index
    }

    #[getter]
    fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    #[getter]
    fn xml<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.xml)
    }

    #[getter]
    fn revision(&self) -> u64 {
        self.revision
    }

    /// A paragraph handle for this paragraph item, or `None` for another kind
    /// or an item that was not read from a document.
    #[getter]
    fn paragraph(&self, py: Python<'_>) -> PyResult<Option<Py<PyParagraph>>> {
        let (Some(document), "paragraph", [index, ..]) = (
            &self.document,
            self.kind.as_str(),
            self.index_path.as_slice(),
        ) else {
            return Ok(None);
        };
        let document = document.clone_ref(py);
        let (location, path) = {
            let owner = document.borrow(py);
            let location = owner.native_location(py, self)?;
            let path = owner.revisions.capture(smallvec![PathSeg::Para(*index)]);
            (location, path)
        };
        Py::new(
            py,
            PyParagraph::for_story(document, path, location).bind_current(py, None)?,
        )
        .map(Some)
    }
}

impl PyStoryParagraph {
    /// A paragraph whose story is not bound to a document, as a refusal
    /// reports it after the candidate it was read from is dropped.
    pub(crate) fn unbound(paragraph: oxoffice_docx::StoryParagraph) -> Self {
        Self {
            kind: story_kind_name(paragraph.kind).to_owned(),
            story: story_snapshot(paragraph.location.story()),
            index_path: paragraph.location.index_path().to_vec(),
            style: paragraph.style,
            text: paragraph.text,
        }
    }
}

#[pymethods]
impl PyStoryParagraph {
    /// The story of the part: body, header, footer, footnote, endnote or
    /// comment. A table cell or a text box takes the kind of the story around it.
    #[getter]
    fn kind(&self) -> &str {
        &self.kind
    }

    #[getter]
    fn part_name(&self) -> &str {
        &self.story.part_name
    }

    /// The story that holds the paragraph: the part's story, a table cell or
    /// a text box.
    #[getter]
    fn story(&self) -> PyStory {
        self.story.clone()
    }

    #[getter]
    fn index_path<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, self.index_path.iter().copied())
    }

    #[getter]
    fn style(&self) -> Option<&str> {
        self.style.as_deref()
    }

    #[getter]
    fn text(&self) -> &str {
        &self.text
    }

    fn __repr__(&self) -> String {
        format!(
            "StoryParagraph(kind={:?}, part_name={:?}, style={}, text={:?})",
            self.kind,
            self.story.part_name,
            self.style
                .as_ref()
                .map_or_else(|| "None".to_owned(), |style| format!("{style:?}")),
            self.text
        )
    }
}

#[pymethods]
impl PyStoryRunPosition {
    /// Take a `StoryItem`, or a `Paragraph` handle, which also reaches a
    /// paragraph inside a block content control.
    #[new]
    #[pyo3(signature = (*, item = None, run_index, paragraph = None))]
    fn new(
        py: Python<'_>,
        item: Option<PyRef<'_, PyStoryItem>>,
        run_index: usize,
        paragraph: Option<PyRef<'_, PyParagraph>>,
    ) -> PyResult<Self> {
        let item = match (item, paragraph) {
            (Some(item), None) => item.clone(),
            (None, Some(paragraph)) => PyDocument::paragraph_story_item(py, &paragraph)?,
            _ => {
                return Err(PyTypeError::new_err(
                    "StoryRunPosition takes exactly one of item and paragraph",
                ));
            }
        };
        Ok(Self { item, run_index })
    }

    #[getter]
    fn item(&self) -> PyStoryItem {
        self.item.clone()
    }

    #[getter]
    fn run_index(&self) -> usize {
        self.run_index
    }
}

#[pymethods]
impl PyStoryRunRange {
    #[new]
    #[pyo3(signature = (*, start, end))]
    fn new(start: PyRef<'_, PyStoryRunPosition>, end: PyRef<'_, PyStoryRunPosition>) -> Self {
        Self {
            start: start.clone(),
            end: end.clone(),
        }
    }

    #[getter]
    fn start(&self) -> PyStoryRunPosition {
        self.start.clone()
    }

    #[getter]
    fn end(&self) -> PyStoryRunPosition {
        self.end.clone()
    }
}
