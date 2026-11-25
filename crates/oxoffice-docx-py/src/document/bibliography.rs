//! Bibliography sources, style, citations, bibliographies and the update of
//! their results.
//!
//! Styles, kinds, roles and fields are the facade's stable snake-case names
//! (`BibliographyStyle::name` and its siblings). The value classes hold the
//! native values, so they compare as the facade does.

use super::*;

use oxoffice_docx::{
    BibliographyAuthor, BibliographyContributor, BibliographyContributorRole, BibliographyOptions,
    BibliographyPerson, BibliographyProperty, BibliographySource, BibliographySourceField,
    BibliographySourceInfo, BibliographySourceKind, BibliographyStyle, BibliographyStyleInfo,
    BibliographyUpdateReport, CitationOptions, CitationSourceOptions,
};

fn style_from_name(name: &str) -> PyResult<BibliographyStyle> {
    BibliographyStyle::from_name(name).ok_or_else(|| {
        PyValueError::new_err(format!(
            "unknown bibliography style {name:?}, expected one of {}",
            BibliographyStyle::ALL
                .map(BibliographyStyle::name)
                .join(", ")
        ))
    })
}

/// The strings of `value`: one string, or a sequence of them.
fn name_parts(value: &Bound<'_, PyAny>, label: &str) -> PyResult<Vec<String>> {
    if let Ok(part) = value.cast::<PyString>() {
        return Ok(vec![part.to_str()?.to_owned()]);
    }
    require_carrier_sequence(value, label)?;
    value
        .try_iter()?
        .map(|part| {
            part?
                .extract::<String>()
                .map_err(|_| PyTypeError::new_err(format!("{label} must hold strings")))
        })
        .collect()
}

/// The items of a sequence argument that must not be a bare string.
fn sequence_items<'py>(value: &Bound<'py, PyAny>, label: &str) -> PyResult<Vec<Bound<'py, PyAny>>> {
    require_carrier_sequence(value, label)?;
    value.try_iter()?.collect()
}

fn strings(value: &Bound<'_, PyAny>, label: &str) -> PyResult<Vec<String>> {
    sequence_items(value, label)?
        .iter()
        .map(|item| {
            item.extract::<String>()
                .map_err(|_| PyTypeError::new_err(format!("{label} must hold strings")))
        })
        .collect()
}

/// One person's name: last, first and middle parts, each in order.
#[pyclass(name = "BibliographyPerson", frozen, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyBibliographyPerson {
    inner: BibliographyPerson,
}

#[pymethods]
impl PyBibliographyPerson {
    #[new]
    #[pyo3(signature = (last, first = None, middle = None))]
    fn new(
        last: &Bound<'_, PyAny>,
        first: Option<&Bound<'_, PyAny>>,
        middle: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let optional = |value: Option<&Bound<'_, PyAny>>, label| {
            value.map_or(Ok(Vec::new()), |value| name_parts(value, label))
        };
        Ok(Self {
            inner: BibliographyPerson {
                last: name_parts(last, "last")?,
                first: optional(first, "first")?,
                middle: optional(middle, "middle")?,
            },
        })
    }

    #[getter]
    fn last<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, &self.inner.last)
    }

    #[getter]
    fn first<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, &self.inner.first)
    }

    #[getter]
    fn middle<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, &self.inner.middle)
    }
}

/// One contributor in source order: a role and either people or a
/// corporate name.
#[pyclass(name = "BibliographyContributor", frozen, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyBibliographyContributor {
    inner: BibliographyContributor,
}

#[pymethods]
impl PyBibliographyContributor {
    #[new]
    #[pyo3(signature = (role, *, people = None, corporate = None))]
    fn new(
        role: &str,
        people: Option<&Bound<'_, PyAny>>,
        corporate: Option<String>,
    ) -> PyResult<Self> {
        let role = BibliographyContributorRole::from_name(role)
            .ok_or_else(|| PyValueError::new_err(format!("unknown contributor role {role:?}")))?;
        let value = match (people, corporate) {
            (Some(people), None) => BibliographyAuthor::People(
                sequence_items(people, "people")?
                    .iter()
                    .map(|person| {
                        person
                            .cast::<PyBibliographyPerson>()
                            .map(|person| person.get().inner.clone())
                            .map_err(|_| {
                                PyTypeError::new_err("people must hold BibliographyPerson values")
                            })
                    })
                    .collect::<PyResult<_>>()?,
            ),
            (None, Some(name)) => BibliographyAuthor::Corporate(name),
            _ => {
                return Err(PyValueError::new_err(
                    "a contributor takes exactly one of people and corporate",
                ));
            }
        };
        Ok(Self {
            inner: BibliographyContributor { role, value },
        })
    }

    #[getter]
    fn role(&self) -> &'static str {
        self.inner.role.name()
    }

    #[getter]
    fn people<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyTuple>>> {
        let BibliographyAuthor::People(people) = &self.inner.value else {
            return Ok(None);
        };
        let people = people
            .iter()
            .map(|person| {
                Py::new(
                    py,
                    PyBibliographyPerson {
                        inner: person.clone(),
                    },
                )
            })
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, people).map(Some)
    }

    #[getter]
    fn corporate(&self) -> Option<&str> {
        match &self.inner.value {
            BibliographyAuthor::Corporate(name) => Some(name),
            BibliographyAuthor::People(_) => None,
        }
    }
}

/// An editable source: tag, kind, GUID, locale, contributors and fields,
/// each in order.
#[pyclass(name = "BibliographySource", frozen, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyBibliographySource {
    inner: BibliographySource,
}

#[pymethods]
impl PyBibliographySource {
    #[new]
    #[pyo3(signature = (tag, kind, *, guid = String::new(), locale = None, contributors = None, fields = None))]
    fn new(
        tag: String,
        kind: &str,
        guid: String,
        locale: Option<u32>,
        contributors: Option<&Bound<'_, PyAny>>,
        fields: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let kind = BibliographySourceKind::from_name(kind)
            .ok_or_else(|| PyValueError::new_err(format!("unknown source kind {kind:?}")))?;
        let contributors = match contributors {
            None => Vec::new(),
            Some(contributors) => sequence_items(contributors, "contributors")?
                .iter()
                .map(|contributor| {
                    contributor
                        .cast::<PyBibliographyContributor>()
                        .map(|contributor| contributor.get().inner.clone())
                        .map_err(|_| {
                            PyTypeError::new_err(
                                "contributors must hold BibliographyContributor values",
                            )
                        })
                })
                .collect::<PyResult<_>>()?,
        };
        let properties = match fields {
            None => Vec::new(),
            Some(fields) => sequence_items(fields, "fields")?
                .iter()
                .map(|pair| {
                    // Any pair: a tuple, or a list as `json.load` reads `sources --json`.
                    let shape =
                        || PyValueError::new_err("a field is a (field, value) pair of strings");
                    let items = sequence_items(pair, "a field").map_err(|_| shape())?;
                    let [field, value] = items.as_slice() else {
                        return Err(shape());
                    };
                    let (field, value) = (
                        field.extract::<String>().map_err(|_| shape())?,
                        value.extract::<String>().map_err(|_| shape())?,
                    );
                    let field = BibliographySourceField::from_name(&field).ok_or_else(|| {
                        PyValueError::new_err(format!("unknown source field {field:?}"))
                    })?;
                    Ok(BibliographyProperty { field, value })
                })
                .collect::<PyResult<_>>()?,
        };
        Ok(Self {
            inner: BibliographySource {
                tag,
                guid,
                kind,
                locale,
                contributors,
                properties,
            },
        })
    }

    #[getter]
    fn tag(&self) -> &str {
        &self.inner.tag
    }

    #[getter]
    fn kind(&self) -> &'static str {
        self.inner.kind.name()
    }

    #[getter]
    fn guid(&self) -> &str {
        &self.inner.guid
    }

    #[getter]
    fn locale(&self) -> Option<u32> {
        self.inner.locale
    }

    #[getter]
    fn contributors<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let contributors = self
            .inner
            .contributors
            .iter()
            .map(|contributor| {
                Py::new(
                    py,
                    PyBibliographyContributor {
                        inner: contributor.clone(),
                    },
                )
            })
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, contributors)
    }

    #[getter]
    fn fields<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(
            py,
            self.inner
                .properties
                .iter()
                .map(|property| (property.field.name(), property.value.as_str())),
        )
    }
}

/// One cited source and its own switches.
#[pyclass(name = "CitationSource", frozen, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyCitationSource {
    inner: CitationSourceOptions,
}

#[pymethods]
impl PyCitationSource {
    #[new]
    #[pyo3(signature = (tag, *, pages = None, volume = None, prefix = None, suffix = None, suppress_author = false, suppress_year = false, suppress_title = false))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        tag: String,
        pages: Option<String>,
        volume: Option<String>,
        prefix: Option<String>,
        suffix: Option<String>,
        suppress_author: bool,
        suppress_year: bool,
        suppress_title: bool,
    ) -> Self {
        Self {
            inner: CitationSourceOptions {
                tag,
                pages,
                volume,
                prefix,
                suffix,
                suppress_author,
                suppress_year,
                suppress_title,
            },
        }
    }

    #[getter]
    fn tag(&self) -> &str {
        &self.inner.tag
    }

    #[getter]
    fn pages(&self) -> Option<&str> {
        self.inner.pages.as_deref()
    }

    #[getter]
    fn volume(&self) -> Option<&str> {
        self.inner.volume.as_deref()
    }

    #[getter]
    fn prefix(&self) -> Option<&str> {
        self.inner.prefix.as_deref()
    }

    #[getter]
    fn suffix(&self) -> Option<&str> {
        self.inner.suffix.as_deref()
    }

    #[getter]
    fn suppress_author(&self) -> bool {
        self.inner.suppress_author
    }

    #[getter]
    fn suppress_year(&self) -> bool {
        self.inner.suppress_year
    }

    #[getter]
    fn suppress_title(&self) -> bool {
        self.inner.suppress_title
    }
}

/// A source as saved: its identity, the editable source when it has one,
/// and what could not be read.
#[pyclass(name = "BibliographySourceInfo", frozen, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyBibliographySourceInfo {
    inner: BibliographySourceInfo,
}

#[pymethods]
impl PyBibliographySourceInfo {
    #[getter]
    fn tag(&self) -> &str {
        &self.inner.tag
    }

    #[getter]
    fn guid(&self) -> Option<&str> {
        self.inner.guid.as_deref()
    }

    #[getter]
    fn source_type(&self) -> Option<&str> {
        self.inner.source_type.as_deref()
    }

    #[getter]
    fn supported(&self) -> Option<PyBibliographySource> {
        self.inner
            .supported
            .clone()
            .map(|inner| PyBibliographySource { inner })
    }

    #[getter]
    fn diagnostics<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, &self.inner.diagnostics)
    }
}

/// The saved style metadata and the style it names, when it is one of the
/// twelve.
#[pyclass(name = "BibliographyStyleInfo", frozen, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyBibliographyStyleInfo {
    inner: BibliographyStyleInfo,
}

#[pymethods]
impl PyBibliographyStyleInfo {
    #[getter]
    fn style_key(&self) -> Option<&str> {
        self.inner.style_key.as_deref()
    }

    #[getter]
    fn style_path(&self) -> Option<&str> {
        self.inner.style_path.as_deref()
    }

    #[getter]
    fn locale(&self) -> Option<u32> {
        self.inner.locale
    }

    #[getter]
    fn supported_style(&self) -> Option<&'static str> {
        self.inner.supported_style.map(BibliographyStyle::name)
    }
}

/// What an update rebuilt.
#[pyclass(name = "BibliographyUpdateReport", frozen, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyBibliographyUpdateReport {
    inner: BibliographyUpdateReport,
}

#[pymethods]
impl PyBibliographyUpdateReport {
    #[getter]
    fn updated_citations(&self) -> usize {
        self.inner.updated_citations
    }

    #[getter]
    fn rebuilt_bibliographies(&self) -> usize {
        self.inner.rebuilt_bibliographies
    }

    #[getter]
    fn diagnostics<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, &self.inner.diagnostics)
    }
}

fn source_argument(source: &Bound<'_, PyAny>) -> PyResult<BibliographySource> {
    source
        .cast::<PyBibliographySource>()
        .map(|source| source.get().inner.clone())
        .map_err(|_| PyTypeError::new_err("source must be a BibliographySource"))
}

fn citation_options(sources: &Bound<'_, PyAny>, locale: Option<u32>) -> PyResult<CitationOptions> {
    let sources = sequence_items(sources, "sources")?
        .iter()
        .map(|source| {
            if let Ok(tag) = source.cast::<PyString>() {
                return Ok(CitationSourceOptions {
                    tag: tag.to_str()?.to_owned(),
                    pages: None,
                    volume: None,
                    prefix: None,
                    suffix: None,
                    suppress_author: false,
                    suppress_year: false,
                    suppress_title: false,
                });
            }
            source
                .cast::<PyCitationSource>()
                .map(|source| source.get().inner.clone())
                .map_err(|_| {
                    PyTypeError::new_err("sources must hold tags or CitationSource values")
                })
        })
        .collect::<PyResult<_>>()?;
    Ok(CitationOptions { sources, locale })
}

fn bibliography_options(
    style: &str,
    locale: Option<u32>,
    locale_filter: Option<u32>,
    tags: Option<&Bound<'_, PyAny>>,
) -> PyResult<BibliographyOptions> {
    Ok(BibliographyOptions {
        style: style_from_name(style)?,
        locale,
        locale_filter,
        tags: tags.map_or(Ok(Vec::new()), |tags| strings(tags, "tags"))?,
    })
}

impl PyDocument {
    pub(super) fn bibliography_sources_impl<'py>(
        &self,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyTuple>> {
        let sources = self
            .inner
            .bibliography_sources()
            .map_err(|error| docx_to_pyerr(py, error))?
            .into_iter()
            .map(|inner| Py::new(py, PyBibliographySourceInfo { inner }))
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, sources)
    }

    pub(super) fn add_bibliography_source_impl(
        &mut self,
        py: Python<'_>,
        source: &Bound<'_, PyAny>,
    ) -> PyResult<String> {
        let source = source_argument(source)?;
        // The source collection is edited in place: no body content
        // changes, so every handle stays bound.
        self.inner
            .add_bibliography_source(&source)
            .map_err(|error| docx_to_pyerr(py, error))
    }

    pub(super) fn replace_bibliography_source_impl(
        &mut self,
        py: Python<'_>,
        source: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let mut source = source_argument(source)?;
        if source.guid.is_empty() {
            // The refusals `replace-source` gives for the same case.
            let refused = |message: String| {
                docx_to_pyerr(
                    py,
                    oxoffice_docx::Error::Other(format!("bibliography: {message}")),
                )
            };
            let existing = self
                .inner
                .bibliography_sources()
                .map_err(|error| docx_to_pyerr(py, error))?
                .into_iter()
                .find(|info| info.tag == source.tag)
                .ok_or_else(|| {
                    refused(format!("source {:?}: no source has this tag", source.tag))
                })?;
            source.guid = existing.guid.ok_or_else(|| {
                refused(format!("source {:?}: the source has no GUID", source.tag))
            })?;
        }
        // In place, as add_bibliography_source.
        self.inner
            .replace_bibliography_source(&source)
            .map_err(|error| docx_to_pyerr(py, error))
    }

    pub(super) fn remove_bibliography_source_impl(
        &mut self,
        py: Python<'_>,
        tag: &str,
    ) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .remove_bibliography_source(tag)
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.publish_api_candidate(py, candidate, "Document.remove_bibliography_source")
    }

    pub(super) fn bibliography_style_impl(
        &self,
        py: Python<'_>,
    ) -> PyResult<Option<PyBibliographyStyleInfo>> {
        Ok(self
            .inner
            .bibliography_style()
            .map_err(|error| docx_to_pyerr(py, error))?
            .map(|inner| PyBibliographyStyleInfo { inner }))
    }

    pub(super) fn set_bibliography_options_impl(
        &mut self,
        py: Python<'_>,
        style: &str,
        locale: Option<u32>,
        locale_filter: Option<u32>,
        tags: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let options = bibliography_options(style, locale, locale_filter, tags)?;
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .set_bibliography_options(&options)
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.publish_api_candidate(py, candidate, "Document.set_bibliography_options")
    }

    pub(super) fn insert_citation_impl(
        &mut self,
        py: Python<'_>,
        position: &Bound<'_, PyAny>,
        sources: &Bound<'_, PyAny>,
        locale: Option<u32>,
    ) -> PyResult<()> {
        let position = position
            .cast::<PyStoryRunPosition>()
            .map_err(|_| PyTypeError::new_err("position must be a StoryRunPosition"))?
            .borrow();
        let options = citation_options(sources, locale)?;
        let native = oxoffice_docx::StoryRunPosition {
            location: self.native_location(py, &position.item)?,
            run_index: position.run_index,
        };
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .insert_citation(&native, &options)
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.publish_api_candidate(py, candidate, "Document.insert_citation")
    }

    pub(super) fn insert_citation_after_text_impl(
        &mut self,
        py: Python<'_>,
        text: &str,
        sources: &Bound<'_, PyAny>,
        occurrence: usize,
        locale: Option<u32>,
    ) -> PyResult<()> {
        let options = citation_options(sources, locale)?;
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .insert_citation_after_text(text, occurrence, &options)
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.publish_api_candidate(py, candidate, "Document.insert_citation_after_text")
    }

    pub(super) fn insert_bibliography_impl(
        &mut self,
        py: Python<'_>,
        destination: &Bound<'_, PyAny>,
        style: &str,
        locale: Option<u32>,
        locale_filter: Option<u32>,
        tags: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let options = bibliography_options(style, locale, locale_filter, tags)?;
        // A story item names the boundary before it, and a story its end.
        let location = if let Ok(item) = destination.cast::<PyStoryItem>() {
            self.native_location(py, &item.borrow())?
        } else if let Ok(story) = destination.cast::<PyStory>() {
            oxoffice_docx::ContentLocation::end(self.native_story(py, &story.borrow())?)
        } else {
            let destination = body_index_argument(
                destination,
                "destination must be an int, a StoryItem or a Story",
            )?;
            self.body_location(py, destination)?
        };
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .insert_bibliography(&location, &options)
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.publish_api_candidate(py, candidate, "Document.insert_bibliography")
    }

    pub(super) fn update_bibliography_impl(
        &mut self,
        py: Python<'_>,
        default_locale: Option<u32>,
    ) -> PyResult<PyBibliographyUpdateReport> {
        let mut candidate = self.inner.clone_for_staging();
        let report = match default_locale {
            Some(locale) => candidate.update_bibliography_with_default_locale(locale),
            None => candidate.update_bibliography(),
        }
        .map_err(|error| docx_to_pyerr(py, error))?;
        self.publish_api_candidate(py, candidate, "Document.update_bibliography")?;
        Ok(PyBibliographyUpdateReport { inner: report })
    }
}
