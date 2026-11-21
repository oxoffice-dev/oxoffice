//! Bookmarks, comments, revisions and hyperlinks.

use super::*;

#[pymethods]
impl PyBookmark {
    #[new]
    #[pyo3(signature = (*, id, name, range, direct_range, text, issue))]
    fn new(
        id: Option<i32>,
        name: Option<String>,
        range: Option<PyRef<'_, PyRunRange>>,
        direct_range: Option<PyRef<'_, PyRunRange>>,
        text: String,
        issue: Option<String>,
    ) -> Self {
        Self {
            id,
            name,
            range: range.map(|range| *range),
            direct_range: direct_range.map(|range| *range),
            text,
            issue,
        }
    }

    #[getter]
    fn id(&self) -> Option<i32> {
        self.id
    }

    #[getter]
    fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    #[getter]
    fn range(&self) -> Option<PyRunRange> {
        self.range
    }

    #[getter]
    fn direct_range(&self) -> Option<PyRunRange> {
        self.direct_range
    }

    #[getter]
    fn text(&self) -> &str {
        &self.text
    }

    #[getter]
    fn issue(&self) -> Option<&str> {
        self.issue.as_deref()
    }
}

#[pyclass(name = "Comment", frozen, get_all, eq, skip_from_py_object)]
#[derive(Clone)]
pub struct PyComment {
    pub id: i32,
    pub author: Option<String>,
    pub initials: Option<String>,
    pub date: Option<String>,
    pub text: String,
    pub parent_id: Option<i32>,
    pub resolved: Option<bool>,
    pub anchor_text: Option<String>,
    pub anchor: Option<PyStoryRunRange>,
    pub anchor_issue: Option<String>,
}

// Preserve the established metadata value equality. Derived anchor snapshots
// carry document revisions and do not change the identity of a comment record.
impl PartialEq for PyComment {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.author == other.author
            && self.initials == other.initials
            && self.date == other.date
            && self.text == other.text
            && self.parent_id == other.parent_id
            && self.resolved == other.resolved
    }
}

impl Eq for PyComment {}

#[pymethods]
impl PyComment {
    #[new]
    #[pyo3(signature = (*, id, author, initials, date, text, parent_id, resolved, anchor_text=None, anchor=None, anchor_issue=None))]
    #[allow(clippy::too_many_arguments)] // Preserve seven original fields plus optional anchor snapshots.
    fn new(
        id: i32,
        author: Option<String>,
        initials: Option<String>,
        date: Option<String>,
        text: String,
        parent_id: Option<i32>,
        resolved: Option<bool>,
        anchor_text: Option<String>,
        anchor: Option<PyRef<'_, PyStoryRunRange>>,
        anchor_issue: Option<String>,
    ) -> Self {
        Self {
            anchor_text,
            anchor: anchor.map(|range| (*range).clone()),
            anchor_issue,
            id,
            author,
            initials,
            date,
            text,
            parent_id,
            resolved,
        }
    }
}

#[pyclass(name = "Revision", frozen, get_all, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyRevision {
    pub id: i32,
    pub author: String,
    pub timestamp: Option<String>,
    pub kind: String,
    pub story: Option<PyStory>,
    pub text: String,
}

#[pymethods]
impl PyRevision {
    #[new]
    #[pyo3(signature = (*, id, author, timestamp, kind, story=None, text=String::new()), text_signature = "(*, id, author, timestamp, kind, story=None, text='')")]
    fn new(
        id: i32,
        author: String,
        timestamp: Option<String>,
        kind: String,
        story: Option<PyRef<'_, PyStory>>,
        text: String,
    ) -> Self {
        Self {
            id,
            author,
            timestamp,
            kind,
            story: story.map(|story| story.clone()),
            text,
        }
    }
}

pub(super) fn revision_kind_name(kind: oxoffice_docx::RevisionKind) -> &'static str {
    match kind {
        oxoffice_docx::RevisionKind::Insertion => "insertion",
        oxoffice_docx::RevisionKind::Deletion => "deletion",
        oxoffice_docx::RevisionKind::MoveFrom => "move_from",
        oxoffice_docx::RevisionKind::MoveTo => "move_to",
        oxoffice_docx::RevisionKind::RunPropertyChange => "run_property_change",
        oxoffice_docx::RevisionKind::ParagraphPropertyChange => "paragraph_property_change",
        oxoffice_docx::RevisionKind::TablePropertyChange => "table_property_change",
        oxoffice_docx::RevisionKind::SectionPropertyChange => "section_property_change",
    }
}

/// Equality covers the public fields only, so a snapshot equals a record
/// rebuilt from them.
impl PartialEq for PyHyperlink {
    fn eq(&self, other: &Self) -> bool {
        self.story == other.story
            && self.index_path == other.index_path
            && self.text == other.text
            && self.url == other.url
            && self.anchor == other.anchor
            && self.relationship_id == other.relationship_id
    }
}

impl Eq for PyHyperlink {}

/// A digest of one story's link layout: the item path and text of every link
/// in order. Retargeting a link keeps it, and adding or removing one changes it.
pub(super) fn story_link_layout<'a>(
    links: impl IntoIterator<Item = (&'a [usize], &'a str)>,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    for (index_path, text) in links {
        index_path.hash(&mut hasher);
        text.hash(&mut hasher);
    }
    hasher.finish()
}

#[pymethods]
impl PyHyperlink {
    #[new]
    #[pyo3(signature = (*, story, index_path, text, url, anchor, relationship_id))]
    fn new(
        story: PyRef<'_, PyStory>,
        index_path: Vec<usize>,
        text: String,
        url: Option<String>,
        anchor: Option<String>,
        relationship_id: Option<String>,
    ) -> Self {
        Self {
            story: story.clone(),
            index_path,
            text,
            url,
            anchor,
            relationship_id,
            position: None,
            owner: None,
        }
    }

    #[getter]
    fn story(&self) -> PyStory {
        self.story.clone()
    }

    #[getter]
    fn index_path<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, self.index_path.iter().copied())
    }

    #[getter]
    fn text(&self) -> &str {
        &self.text
    }

    #[getter]
    fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    #[getter]
    fn anchor(&self) -> Option<&str> {
        self.anchor.as_deref()
    }

    #[getter]
    fn relationship_id(&self) -> Option<&str> {
        self.relationship_id.as_deref()
    }
}
