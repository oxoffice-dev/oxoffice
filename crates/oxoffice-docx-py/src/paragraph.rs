use oxoffice_py_support::{ContentPath, PathSeg};
use pyo3::exceptions::{PyIndexError, PyKeyError, PyRuntimeError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyList, PySlice, PyTuple};
use smallvec::smallvec;

use crate::document::PyDocument;
use crate::run::{PyRun, PyRunCollection};
use crate::{normalize_index, stale_to_pyerr};

#[pyclass(
    name = "MathConversionDiagnostic",
    frozen,
    get_all,
    skip_from_py_object
)]
#[derive(Clone)]
pub struct PyMathConversionDiagnostic {
    path: String,
    message: String,
}
impl From<oxoffice_docx::MathConversionDiagnostic> for PyMathConversionDiagnostic {
    fn from(value: oxoffice_docx::MathConversionDiagnostic) -> Self {
        Self {
            path: value.path,
            message: value.message,
        }
    }
}
#[pyclass(name = "MathConversionResult", frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct PyMathConversionResult {
    value: String,
    diagnostics: Vec<PyMathConversionDiagnostic>,
}
#[pymethods]
impl PyMathConversionResult {
    #[getter]
    fn value(&self) -> &str {
        &self.value
    }
    #[getter]
    fn diagnostics<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let values = self
            .diagnostics
            .iter()
            .cloned()
            .map(|v| Py::new(py, v))
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, values)
    }
}
#[pyclass(name = "EquationConversionResult", frozen, skip_from_py_object)]
pub struct PyEquationConversionResult {
    is_display: bool,
    equations: Vec<PyMathConversionResult>,
    diagnostics: Vec<PyMathConversionDiagnostic>,
}
#[pymethods]
impl PyEquationConversionResult {
    #[getter]
    fn is_display(&self) -> bool {
        self.is_display
    }
    #[getter]
    fn equations<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let values = self
            .equations
            .iter()
            .cloned()
            .map(|v| Py::new(py, v))
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, values)
    }
    #[getter]
    fn diagnostics<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let values = self
            .diagnostics
            .iter()
            .cloned()
            .map(|v| Py::new(py, v))
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, values)
    }
}
#[pyclass(name = "Equation", frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct PyEquation {
    inner: oxoffice_docx::OfficeMath,
}
impl PyEquation {
    fn conversion(&self, mathml: bool) -> PyEquationConversionResult {
        let mut diagnostics = Vec::new();
        let children: Vec<&oxoffice_docx::CT_OMath> = match &self.inner {
            oxoffice_docx::OfficeMath::Inline(equation) => vec![equation],
            oxoffice_docx::OfficeMath::Display(display) => {
                let mut remainder = display.clone();
                remainder.equations.clear();
                if remainder.has_unsupported_content() {
                    diagnostics.push(PyMathConversionDiagnostic { path: "/oMathPara".into(), message: "display wrapper preserved content is not represented by per-equation conversion".into() });
                }
                if display.properties.justification.is_some() {
                    diagnostics.push(PyMathConversionDiagnostic { path: "/oMathPara/oMathParaPr/jc".into(), message: "display group justification is not represented by per-equation conversion".into() });
                }
                display.equations.iter().collect()
            }
        };
        let equations = children.into_iter().map(|child| {
            let argument = oxoffice_docx::MathArgument::new(child.expressions.clone());
            let converted = if mathml { oxoffice_docx::equation_to_mathml(&argument) } else { oxoffice_docx::equation_to_latex(&argument) };
            let mut diagnostics: Vec<_> = converted.diagnostics.into_iter().map(PyMathConversionDiagnostic::from).collect();
            let mut remainder = child.clone();
            remainder.expressions.clear();
            if remainder.has_unsupported_content() {
                diagnostics.push(PyMathConversionDiagnostic { path: "/oMath".into(), message: "equation container preserved content is not represented by expression conversion".into() });
            }
            PyMathConversionResult { value: converted.value, diagnostics }
        }).collect();
        PyEquationConversionResult {
            is_display: matches!(self.inner, oxoffice_docx::OfficeMath::Display(_)),
            equations,
            diagnostics,
        }
    }
}
#[pymethods]
impl PyEquation {
    #[getter]
    fn is_display(&self) -> bool {
        matches!(self.inner, oxoffice_docx::OfficeMath::Display(_))
    }
    fn to_latex(&self) -> PyEquationConversionResult {
        self.conversion(false)
    }
    fn to_mathml(&self) -> PyEquationConversionResult {
        self.conversion(true)
    }
}
#[pyclass(name = "EquationInsertResult", frozen, skip_from_py_object)]
pub struct PyEquationInsertResult {
    equation: PyEquation,
    diagnostics: Vec<PyMathConversionDiagnostic>,
}
#[pymethods]
impl PyEquationInsertResult {
    #[getter]
    fn equation(&self) -> PyEquation {
        self.equation.clone()
    }
    #[getter]
    fn diagnostics<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let values = self
            .diagnostics
            .iter()
            .cloned()
            .map(|v| Py::new(py, v))
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, values)
    }
}

#[derive(Clone)]
pub(crate) enum ParagraphLocation {
    Related(oxoffice_docx::ContentLocation),
    Body(usize),
    Cell {
        table: usize,
        row: usize,
        cell: usize,
        paragraph: usize,
    },
}

impl ParagraphLocation {
    fn refreshed(
        document: &oxoffice_docx::Document,
        location: &oxoffice_docx::ContentLocation,
    ) -> oxoffice_docx::Result<oxoffice_docx::ContentLocation> {
        let owner = location.story();
        let story = document
            .stories()?
            .into_iter()
            .find(|s| {
                s.kind() == owner.kind()
                    && s.part_name() == owner.part_name()
                    && s.owner_index() == owner.owner_index()
            })
            .ok_or_else(|| {
                oxoffice_docx::Error::Other("rich paragraph owner disappeared".to_owned())
            })?;
        Ok(oxoffice_docx::ContentLocation::new(
            story,
            location.item_kind(),
            location.index_path().to_vec(),
        ))
    }

    pub(crate) fn native(
        &self,
        document: &oxoffice_docx::Document,
    ) -> oxoffice_docx::Result<oxoffice_docx::ContentLocation> {
        match self {
            Self::Body(index) => document.paragraph_story_location(*index)?.ok_or_else(|| {
                oxoffice_docx::Error::Other("paragraph has no checked physical owner".to_owned())
            }),
            Self::Cell {
                table,
                row,
                cell,
                paragraph,
            } => document.cell_paragraph_story_location(*table, *row, *cell, *paragraph),
            Self::Related(location) => Self::refreshed(document, location),
        }
    }

    pub(crate) fn read<T>(
        &self,
        document: &oxoffice_docx::Document,
        read: impl FnOnce(oxoffice_docx::ParagraphRef<'_>) -> T,
    ) -> oxoffice_docx::Result<T> {
        match self {
            Self::Body(index) => document.paragraph(*index).map(read).ok_or_else(|| {
                oxoffice_docx::Error::Other("paragraph index out of range".to_owned())
            }),
            Self::Cell {
                table,
                row,
                cell,
                paragraph,
            } => {
                let table = document.table(*table).ok_or_else(|| {
                    oxoffice_docx::Error::Other("table index out of range".to_owned())
                })?;
                let cell = table.cell(*row, *cell).ok_or_else(|| {
                    oxoffice_docx::Error::Other("cell index out of range".to_owned())
                })?;
                let paragraph = cell.paragraph(*paragraph).ok_or_else(|| {
                    oxoffice_docx::Error::Other("cell paragraph index out of range".to_owned())
                })?;
                Ok(read(paragraph))
            }
            Self::Related(location) => {
                document.read_story_paragraph(&Self::refreshed(document, location)?, read)
            }
        }
    }

    pub(crate) fn edit<T>(
        &self,
        document: &mut oxoffice_docx::Document,
        edit: impl FnOnce(&mut oxoffice_docx::Paragraph<'_>) -> oxoffice_docx::Result<T>,
    ) -> oxoffice_docx::Result<T> {
        match self {
            Self::Body(index) => edit(&mut document.paragraph_mut(*index).ok_or_else(|| {
                oxoffice_docx::Error::Other("paragraph index out of range".to_owned())
            })?),
            Self::Cell {
                table,
                row,
                cell,
                paragraph,
            } => document.edit_cell_paragraph(*table, *row, *cell, *paragraph, edit),
            Self::Related(location) => {
                document.edit_story_paragraph(&Self::refreshed(document, location)?, edit)
            }
        }
    }
}

pub(crate) fn paragraph_location(path: &ContentPath) -> PyResult<ParagraphLocation> {
    let paragraph = path
        .segs
        .iter()
        .rev()
        .find_map(|segment| match segment {
            PathSeg::Para(index) => Some(*index),
            _ => None,
        })
        .ok_or_else(|| PyRuntimeError::new_err("paragraph path has no paragraph index"))?;
    let table = path.segs.iter().find_map(|segment| match segment {
        PathSeg::Body(index) => Some(*index),
        _ => None,
    });
    let row = path.segs.iter().find_map(|segment| match segment {
        PathSeg::Row(index) => Some(*index),
        _ => None,
    });
    let cell = path.segs.iter().find_map(|segment| match segment {
        PathSeg::Cell(index) => Some(*index),
        _ => None,
    });
    match (table, row, cell) {
        (Some(table), Some(row), Some(cell)) => Ok(ParagraphLocation::Cell {
            table,
            row,
            cell,
            paragraph,
        }),
        (_, None, None) => Ok(ParagraphLocation::Body(paragraph)),
        _ => Err(PyRuntimeError::new_err("paragraph path is incomplete")),
    }
}

/// Find the style a value names among `styles` of `style_type`, or among
/// every style when it is `None`: a style ID or, as python-docx accepts, a
/// style name.
///
/// The ID is tried first, so a value read from `Paragraph.style` always
/// finds the same style. A name then matches exactly before it matches
/// regardless of case, which is how "Heading 1" finds Word's "heading 1".
fn find_style<'s, 'a>(
    styles: &'s [oxoffice_docx::Style<'a>],
    value: &str,
    style_type: Option<oxoffice_docx::StyleType>,
) -> Option<&'s oxoffice_docx::Style<'a>> {
    let lowered = value.to_lowercase();
    let candidates = || {
        styles
            .iter()
            .filter(move |style| style_type.is_none_or(|wanted| style.style_type() == wanted))
    };
    candidates()
        .find(|style| style.style_id() == value)
        .or_else(|| candidates().find(|style| style.name() == Some(value)))
        .or_else(|| {
            candidates().find(|style| {
                style
                    .name()
                    .is_some_and(|name| name.to_lowercase() == lowered)
            })
        })
}

fn no_style_error(value: &str) -> PyErr {
    PyKeyError::new_err(format!(
        "no style with ID or name '{value}'; use document.add_style/set_style; lists use document.add_numbering_definition and add_numbering_instance",
    ))
}

/// Resolve a style of `style_type`, given by ID or name, to the ID the
/// package defines.
///
/// Each step of [`find_style`] looks at styles of that type only, so a
/// character style cannot hide a paragraph style of that name. As in
/// python-docx, a value naming no style raises `KeyError` and one naming only
/// a style of another type raises `ValueError`.
pub(crate) fn style_id_of_type(
    document: &oxoffice_docx::Document,
    value: &str,
    style_type: oxoffice_docx::StyleType,
) -> PyResult<String> {
    let styles = document.styles();
    if let Some(style) = find_style(&styles, value, Some(style_type)) {
        return Ok(style.style_id().to_owned());
    }
    match find_style(&styles, value, None) {
        Some(style) => Err(PyValueError::new_err(format!(
            "style '{value}' is a {} style, not a {} style",
            style.style_type().to_str(),
            style_type.to_str()
        ))),
        None => Err(no_style_error(value)),
    }
}

/// The type and ID of the style of any type a value names, as [`find_style`]
/// finds it, or `KeyError`.
pub(crate) fn defined_style(
    document: &oxoffice_docx::Document,
    value: &str,
) -> PyResult<(oxoffice_docx::StyleType, String)> {
    let styles = document.styles();
    find_style(&styles, value, None)
        .map(|style| (style.style_type(), style.style_id().to_owned()))
        .ok_or_else(|| no_style_error(value))
}

#[pyclass(name = "Paragraph")]
pub struct PyParagraph {
    pub(crate) document: Py<PyDocument>,
    path: ContentPath,
    related: Option<oxoffice_docx::ContentLocation>,
    owner: Option<oxoffice_py_support::RevisionState>,
    furniture_scope: Option<oxoffice_py_support::RevisionState>,
}

impl PyParagraph {
    pub(crate) fn new(document: Py<PyDocument>, path: ContentPath) -> Self {
        Self {
            document,
            path,
            related: None,
            owner: None,
            furniture_scope: None,
        }
    }

    pub(crate) fn for_story(
        document: Py<PyDocument>,
        path: ContentPath,
        location: oxoffice_docx::ContentLocation,
    ) -> Self {
        Self {
            document,
            path,
            related: Some(location),
            owner: None,
            furniture_scope: None,
        }
    }

    pub(crate) fn bind_current(
        mut self,
        py: Python<'_>,
        furniture_scope: Option<oxoffice_py_support::RevisionState>,
    ) -> PyResult<Self> {
        self.furniture_scope = furniture_scope;
        let location = self.related.as_ref().map_or_else(
            || paragraph_location(&self.path),
            |location| Ok(ParagraphLocation::Related(location.clone())),
        )?;
        {
            let document = self.document.borrow(py);
            match document.paragraph_route(&location, None) {
                Ok(location) => {
                    let (path, owner) = document.capture_owner(
                        &crate::document::WordOwnerLocation::Paragraph(location),
                        self.path.segs.clone(),
                        crate::document::WordView::Owner,
                        self.furniture_scope.as_ref(),
                    )?;
                    self.path = path;
                    self.owner = Some(owner);
                }
                Err(_) => {
                    // Ordinary facade reads retain unsupported legacy routes.
                    // Their snapshot authority retires on every XML publication.
                    self.path = document.capture_snapshot(self.path.segs.clone())?;
                }
            }
        }
        Ok(self)
    }

    pub(crate) fn bind_prepared(
        mut self,
        py: Python<'_>,
        document: &PyDocument,
        prepared: &crate::document::PreparedWordRevision,
        candidate: &oxoffice_docx::Document,
        furniture_scope: Option<oxoffice_py_support::RevisionState>,
    ) -> PyResult<Self> {
        self.furniture_scope =
            document.prepared_furniture_scope(prepared, furniture_scope.as_ref())?;
        let location = self.related.as_ref().map_or_else(
            || paragraph_location(&self.path),
            |location| Ok(ParagraphLocation::Related(location.clone())),
        )?;
        let location = document
            .paragraph_route(&location, Some((prepared, candidate)))
            .map_err(|error| crate::docx_to_pyerr(py, error))?;
        let (path, owner) = document.capture_prepared_owner(
            prepared,
            &crate::document::WordOwnerLocation::Paragraph(location),
            self.path.segs.clone(),
            crate::document::WordView::Owner,
            self.furniture_scope.as_ref(),
        )?;
        self.path = path;
        self.owner = Some(owner);
        Ok(self)
    }

    /// Bind the paragraph a prepared plain body splice inserts, before the
    /// native edit places it.
    pub(crate) fn bind_prepared_splice(
        mut self,
        document: &PyDocument,
        prepared: &crate::document::PreparedBodySplice,
    ) -> PyResult<Self> {
        self.furniture_scope = None;
        let (path, owner) =
            document.capture_prepared_splice_owner(prepared, self.path.segs.clone())?;
        self.path = path;
        self.owner = Some(owner);
        Ok(self)
    }

    /// This handle's path, its body block followed to where it sits now
    /// ([`PyDocument::followed_segments`]).
    pub(crate) fn current_path(&self, document: &PyDocument) -> ContentPath {
        let mut path = self.path.clone();
        if self.related.is_none() {
            path.segs = document.followed_segments(self.owner.as_ref(), &self.path.segs);
        }
        path
    }

    pub(crate) fn validate(&self, py: Python<'_>) -> PyResult<ParagraphLocation> {
        let document = self.document.borrow(py);
        let location = match &self.related {
            Some(location) => ParagraphLocation::Related(location.clone()),
            None => paragraph_location(&self.current_path(&document))?,
        };
        let recovery_hint = match &location {
            ParagraphLocation::Related(_) => {
                "Re-fetch it from the current rich story owner.".to_owned()
            }
            ParagraphLocation::Body(_) => "Re-fetch it with doc.paragraphs[i].".to_owned(),
            ParagraphLocation::Cell {
                table,
                row,
                cell,
                paragraph,
            } => format!(
                "Re-fetch it with doc.tables[{table}].rows[{row}].cells[{cell}].paragraphs[{paragraph}]."
            ),
        };
        self.path
            .validate_scopes(&document.revisions, "paragraph", &recovery_hint)
            .map_err(|error| stale_to_pyerr(py, error))?;
        if let (Some(owner), ParagraphLocation::Related(_)) = (&self.owner, &location) {
            return document
                .current_paragraph_owner(owner)
                .map(ParagraphLocation::Related);
        }
        Ok(location)
    }

    pub(crate) fn belongs_to(&self, py: Python<'_>, document: &Py<PyDocument>) -> bool {
        self.document.bind(py).is(document.bind(py))
    }

    fn append_internal_run(
        &self,
        py: Python<'_>,
        text: &str,
        bookmark: &str,
        tooltip: Option<&str>,
    ) -> PyResult<Py<PyRun>> {
        let location = self.validate(py)?;
        let mut document = self.document.borrow_mut(py);
        let native = location
            .native(&document.inner)
            .map_err(|error| crate::docx_to_pyerr(py, error))?;
        let mut candidate = document.inner.clone_for_staging();
        let position = candidate
            .add_internal_hyperlink_at(&native, text, bookmark, tooltip)
            .map_err(|error| crate::docx_to_pyerr(py, error))?;
        let run_path = candidate
            .read_story_paragraph(&position.location, |p| p.run_path(position.run_index))
            .map_err(|error| crate::docx_to_pyerr(py, error))?
            .ok_or_else(|| PyIndexError::new_err("inserted link run was not found"))?;
        let publication = crate::document::WordPublication::ParagraphAppend(
            document
                .paragraph_route(&location, None)
                .map_err(|error| crate::docx_to_pyerr(py, error))?,
        );
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "Paragraph.add_internal_hyperlink",
            publication,
        )?;
        let mut segments = self.current_path(&document).segs;
        segments.push(PathSeg::Run(position.run_index));
        let path = document
            .revisions
            .capture_prepared(&prepared.revision, segments)
            .map_err(oxoffice_py_support::revision_to_pyerr)?;
        let result = Py::new(
            py,
            PyRun::new(
                self.document.clone_ref(py),
                path,
                run_path,
                Some(position.location),
            )
            .bind_prepared(
                py,
                &document,
                &prepared,
                &candidate,
                self.furniture_scope.clone(),
            )?,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        Ok(result)
    }

    /// Append one run, or one external hyperlink run when `url` is given, and
    /// return its handle.
    fn append_run(
        &self,
        py: Python<'_>,
        text: &str,
        url: Option<&str>,
        tooltip: Option<&str>,
    ) -> PyResult<Py<PyRun>> {
        let location = self.validate(py)?;
        crate::refuse_non_xml_characters(py, "run text", text)?;
        if let Some(url) = url {
            crate::refuse_non_xml_characters(py, "hyperlink address", url)?;
        }
        if url.is_none()
            && let ParagraphLocation::Body(index) = location
        {
            let appended = {
                let mut document = self.document.borrow_mut(py);
                document
                    .edit_body_paragraph_in_place(py, index, "Paragraph.add_run", |paragraph| {
                        let run_index = paragraph.run_count();
                        paragraph.add_run(text);
                        let run_path = paragraph
                            .run_path(run_index)
                            .expect("the appended run has an accepted source path");
                        Ok((run_index, run_path))
                    })?
                    .map(|appended| {
                        let mut segments = self.current_path(&document).segs;
                        segments.push(PathSeg::Run(appended.0));
                        document
                            .capture_snapshot(segments)
                            .map(|path| (path, appended.1))
                    })
                    .transpose()?
            };
            if let Some((path, run_path)) = appended {
                return Py::new(
                    py,
                    PyRun::new(self.document.clone_ref(py), path, run_path, None)
                        .bind_current(py, self.furniture_scope.clone())?,
                );
            }
        }
        let result = {
            let mut document = self.document.borrow_mut(py);
            let mut candidate = document.inner.clone_for_staging();
            let relationship_id = match url {
                Some(url) => {
                    // Check the paragraph first, so a failure leaves no
                    // orphaned relationship behind.
                    location
                        .read(&candidate, |_| ())
                        .map_err(|error| crate::docx_to_pyerr(py, error))?;
                    Some(match &location {
                        ParagraphLocation::Related(location) => {
                            let location = ParagraphLocation::refreshed(&candidate, location)
                                .map_err(|error| crate::docx_to_pyerr(py, error))?;
                            candidate
                                .add_hyperlink_relationship_to_story(location.story(), url)
                                .map_err(|error| crate::docx_to_pyerr(py, error))?
                        }
                        _ => candidate.add_hyperlink_relationship(url),
                    })
                }
                None => None,
            };
            let append = |paragraph: &mut oxoffice_docx::Paragraph<'_>| {
                let run_index = paragraph.run_count();
                match &relationship_id {
                    Some(relationship_id) => {
                        paragraph.add_hyperlink_with_tooltip(text, relationship_id, tooltip)?;
                    }
                    None => {
                        paragraph.add_run(text);
                    }
                }
                let run_path = paragraph
                    .run_path(run_index)
                    .expect("the appended run has an accepted source path");
                Ok((run_index, run_path))
            };
            let (run_index, run_path) = location
                .edit(&mut candidate, append)
                .map_err(|error| crate::docx_to_pyerr(py, error))?;
            let route = document
                .paragraph_route(&location, None)
                .map_err(|error| crate::docx_to_pyerr(py, error))?;
            // A hyperlink also adds a relationship to the part, which its
            // own publication accounts for.
            let (operation, publication) = if url.is_some() {
                (
                    "Paragraph.add_hyperlink",
                    crate::document::WordPublication::ParagraphAppend(route),
                )
            } else {
                (
                    "Paragraph.add_run",
                    crate::document::WordPublication::ParagraphEdit(route),
                )
            };
            let prepared =
                document.prepare_word_revision(py, &candidate, operation, publication)?;
            let mut segments = self.current_path(&document).segs;
            segments.push(PathSeg::Run(run_index));
            let path = document
                .revisions
                .capture_prepared(&prepared.revision, segments)
                .map_err(oxoffice_py_support::revision_to_pyerr)?;
            let result = Py::new(
                py,
                PyRun::new(
                    self.document.clone_ref(py),
                    path,
                    run_path,
                    self.related.clone(),
                )
                .bind_prepared(
                    py,
                    &document,
                    &prepared,
                    &candidate,
                    self.furniture_scope.clone(),
                )?,
            )?;
            document.commit_word_revision(prepared)?;
            document.install_inner(candidate);
            result
        };
        Ok(result)
    }
}

/// A stretch of a paragraph's literal run text, as `Paragraph.find_text`
/// finds it, for `Document.isolate_span`.
#[pyclass(name = "TextSpan", frozen, skip_from_py_object)]
pub struct PyTextSpan {
    pub(crate) paragraph: Py<PyParagraph>,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

#[pymethods]
impl PyTextSpan {
    #[getter]
    fn paragraph(&self, py: Python<'_>) -> Py<PyParagraph> {
        self.paragraph.clone_ref(py)
    }

    #[getter]
    fn start(&self) -> usize {
        self.start
    }

    #[getter]
    fn end(&self) -> usize {
        self.end
    }

    fn __repr__(&self) -> String {
        format!("TextSpan(start={}, end={})", self.start, self.end)
    }
}

#[pymethods]
impl PyParagraph {
    #[getter]
    fn element<'py>(slf: &Bound<'py, Self>) -> Bound<'py, Self> {
        slf.clone()
    }

    /// The `occurrence`-th match of `text`, counted from zero, in the
    /// paragraph's literal run text, or `None`. Tabs, breaks, field results,
    /// drawings, symbols, smart tags and custom XML are not searched, and a
    /// match never crosses a tab, a break, a field, a drawing, a symbol, a
    /// non-breaking hyphen, or a picture, shape or object kept as raw XML.
    /// A footnote, endnote or comment reference mark is not a barrier.
    #[pyo3(signature = (text, occurrence = 0))]
    fn find_text(
        slf: &Bound<'_, Self>,
        text: &str,
        occurrence: usize,
    ) -> PyResult<Option<PyTextSpan>> {
        let py = slf.py();
        let this = slf.borrow();
        let location = this.validate(py)?;
        let span = {
            let document = this.document.borrow(py);
            location
                .read(&document.inner, |paragraph| {
                    paragraph.find_text(text, occurrence)
                })
                .map_err(|error| crate::docx_to_pyerr(py, error))?
        };
        Ok(span.map(|span| PyTextSpan {
            paragraph: slf.clone().unbind(),
            start: span.start,
            end: span.end,
        }))
    }

    #[getter]
    pub(crate) fn xml(&self, py: Python<'_>) -> PyResult<String> {
        self.validate(py)?;
        let document = self.document.borrow(py);
        let owner = self
            .owner
            .as_ref()
            .ok_or_else(|| PyValueError::new_err("paragraph XML owner route is unsupported"))?;
        let current = || -> PyResult<Vec<u8>> {
            let location = document.current_paragraph_owner(owner)?;
            document
                .inner
                .paragraph_element_xml(&location)
                .map_err(|error| crate::docx_to_pyerr(py, error))
        };
        // A direct body paragraph serializes alone, without the `w14`
        // declaration that only the rest of the body can bring. Any other
        // owner reads the whole story source.
        let bytes = match document.direct_body_paragraph_slot(owner).and_then(|slot| {
            document
                .inner
                .direct_body_paragraph_xml(slot)
                .ok()
                .flatten()
        }) {
            Some(bytes) => {
                #[cfg(debug_assertions)]
                crate::document::assert_direct_read(&bytes, current, "paragraph");
                bytes
            }
            None => current()?,
        };
        String::from_utf8(bytes).map_err(|error| PyValueError::new_err(error.to_string()))
    }

    #[getter]
    fn _p(&self, py: Python<'_>) -> PyResult<()> {
        self.validate(py)?;
        Err(pyo3::exceptions::PyNotImplementedError::new_err(
            "read paragraph.xml and write paragraph.replace_xml(xml) instead of lxml",
        ))
    }

    #[getter]
    fn _element(&self, py: Python<'_>) -> PyResult<()> {
        self.validate(py)?;
        Err(pyo3::exceptions::PyNotImplementedError::new_err(
            "use document.remove_content(document.find_content_index(paragraph)); read paragraph.xml and write paragraph.replace_xml(xml) instead of lxml",
        ))
    }

    #[pyo3(signature = (text = "", style = None))]
    fn insert_paragraph_before(
        &self,
        py: Python<'_>,
        text: &str,
        style: Option<&str>,
    ) -> PyResult<Py<PyParagraph>> {
        let location = self.validate(py)?;
        // A direct body paragraph takes the body splice, which keeps the
        // handles; any other inserts at its location.
        let content = match location {
            ParagraphLocation::Body(index) => self
                .document
                .borrow(py)
                .inner
                .content_index_of_paragraph(index),
            _ => None,
        };
        if let Some(content) = content {
            return PyDocument::insert_paragraph(
                self.document.clone_ref(py),
                py,
                content,
                text,
                style,
            );
        }
        let segments = self.current_path(&self.document.borrow(py)).segs;
        PyDocument::insert_paragraph_at(
            self.document.clone_ref(py),
            py,
            &location,
            segments,
            text,
            style,
        )
    }

    pub(crate) fn replace_xml(&self, py: Python<'_>, xml: &str) -> PyResult<()> {
        {
            let mut document = self.document.borrow_mut(py);
            self.path
                .validate_scopes(
                    &document.revisions,
                    "paragraph",
                    "Re-fetch the current paragraph.",
                )
                .map_err(|error| stale_to_pyerr(py, error))?;
            let owner = self
                .owner
                .as_ref()
                .ok_or_else(|| PyValueError::new_err("paragraph XML owner route is unsupported"))?;
            if let Some((selected, slot)) = document.plain_paragraph_owner(owner)
                && document.inner.plain_direct_paragraph(slot)
                && let Ok(parsed) = oxoffice_docx_oxml::CT_P::from_xml_fragment(xml.as_bytes())
            {
                let text = parsed.accepted_text();
                let prepared = document.prepare_plain_paragraph_xml(selected, &parsed)?;
                match document
                    .inner
                    .try_replace_plain_body_paragraph(slot, xml)
                    .map_err(|error| crate::docx_to_pyerr(py, error))?
                {
                    Some(false) => return Ok(()),
                    Some(true) => {
                        document.commit_plain_paragraph_xml(selected, prepared, text)?;
                        return Ok(());
                    }
                    None => {}
                }
            }
        }
        self.validate(py)?;
        let mut document = self.document.borrow_mut(py);
        let owner = self
            .owner
            .as_ref()
            .ok_or_else(|| PyValueError::new_err("paragraph XML owner route is unsupported"))?;
        let location = document.current_paragraph_owner(owner)?;
        let mut candidate = document.inner.clone_for_staging();
        candidate
            .replace_paragraph_xml(&location, xml)
            .map_err(|error| crate::docx_to_pyerr(py, error))?;
        document.publish_owner_candidate(
            py,
            candidate,
            "Paragraph.replace_xml",
            crate::document::WordPublication::ElementXml(
                crate::document::WordOwnerLocation::Paragraph(location),
            ),
        )
    }

    fn equations<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let location = self.validate(py)?;
        let document = self.document.borrow(py);
        let equations = location
            .read(&document.inner, |p| {
                p.equations()
                    .cloned()
                    .map(|inner| PyEquation { inner })
                    .collect::<Vec<_>>()
            })
            .map_err(|e| crate::docx_to_pyerr(py, e))?;
        let values = equations
            .into_iter()
            .map(|v| Py::new(py, v))
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, values)
    }
    #[pyo3(signature = (source, *, format, display=false))]
    fn add_equation(
        &self,
        py: Python<'_>,
        source: &str,
        format: &str,
        display: bool,
    ) -> PyResult<Py<PyEquationInsertResult>> {
        let converted = match format {
            "latex" => oxoffice_docx::equation_from_latex(source),
            "mathml" => oxoffice_docx::equation_from_mathml(source),
            _ => {
                return Err(PyValueError::new_err(
                    "equation format must be latex or mathml",
                ));
            }
        }
        .map_err(|e| crate::docx_to_pyerr(py, e))?;
        let equation = if display {
            oxoffice_docx::OfficeMath::display(vec![oxoffice_docx::CT_OMath::new(
                converted.value.expressions,
            )])
        } else {
            oxoffice_docx::OfficeMath::inline(converted.value.expressions)
        };
        let result = Py::new(
            py,
            PyEquationInsertResult {
                equation: PyEquation {
                    inner: equation.clone(),
                },
                diagnostics: converted
                    .diagnostics
                    .into_iter()
                    .map(PyMathConversionDiagnostic::from)
                    .collect(),
            },
        )?;
        let location = self.validate(py)?;
        let mut document = self.document.borrow_mut(py);
        if let ParagraphLocation::Body(index) = location
            && document
                .edit_body_paragraph_in_place(py, index, "Paragraph.add_equation", |p| {
                    p.add_equation(equation.clone())
                })?
                .is_some()
        {
            return Ok(result);
        }
        let mut candidate = document.inner.clone_for_staging();
        location
            .edit(&mut candidate, |p| p.add_equation(equation))
            .map_err(|e| crate::docx_to_pyerr(py, e))?;
        let publication = crate::document::WordPublication::ParagraphEdit(
            document
                .paragraph_route(&location, None)
                .map_err(|error| crate::docx_to_pyerr(py, error))?,
        );
        document.publish_block_edit(py, candidate, "Paragraph.add_equation", publication)?;
        Ok(result)
    }

    /// Replace literal text within this paragraph, retaining run formatting.
    #[pyo3(signature = (old, new, *, expect = None))]
    fn replace_text(
        &self,
        py: Python<'_>,
        old: &str,
        new: &str,
        expect: Option<usize>,
    ) -> PyResult<usize> {
        let location = self.validate(py)?;
        let mut document = self.document.borrow_mut(py);
        match location {
            ParagraphLocation::Related(location) => {
                let location = ParagraphLocation::refreshed(&document.inner, &location)
                    .map_err(|error| crate::docx_to_pyerr(py, error))?;
                document.scoped_replacement(py, "Paragraph.replace_text", |document| {
                    document.try_replace_text_at_owned(&location, old, new, expect)
                })
            }
            ParagraphLocation::Body(index) => {
                let location = document
                    .inner
                    .paragraph_story_location(index)
                    .map_err(|error| crate::docx_to_pyerr(py, error))?
                    .ok_or_else(|| PyIndexError::new_err("paragraph index out of range"))?;
                document.scoped_replacement(py, "Paragraph.replace_text", |document| {
                    document.try_replace_text_at_owned(&location, old, new, expect)
                })
            }
            ParagraphLocation::Cell {
                table,
                row,
                cell,
                paragraph,
            } => document.scoped_replacement(py, "Paragraph.replace_text", |document| {
                document.try_replace_text_in_cell_owned(
                    (table, row, cell),
                    Some(paragraph),
                    old,
                    new,
                    expect,
                )
            }),
        }
    }

    #[getter]
    fn text(&self, py: Python<'_>) -> PyResult<String> {
        {
            let document = self.document.borrow(py);
            self.path
                .validate_scopes(
                    &document.revisions,
                    "paragraph",
                    "Re-fetch it from its current physical owner.",
                )
                .map_err(|error| stale_to_pyerr(py, error))?;
            if let Some(owner) = &self.owner
                && let Some(text) = document.cached_paragraph_text(owner)
            {
                return Ok(text.to_owned());
            }
        }
        let location = self.validate(py)?;
        let document = self.document.borrow(py);
        location
            .read(&document.inner, |paragraph| paragraph.text())
            .map_err(|error| crate::docx_to_pyerr(py, error))
    }

    // Replace the content with one run holding `text`, keeping the paragraph
    // properties, comments and bookmarks. `None` is empty text. A success
    // retires the paragraph's runs and what the edit renumbers; this handle
    // and the handles of other paragraphs stay. Text the paragraph already
    // shows as its only run changes nothing and keeps every handle.
    #[setter]
    fn set_text(&self, py: Python<'_>, text: Option<&str>) -> PyResult<()> {
        let location = self.validate(py)?;
        let text = text.unwrap_or_default();
        let mut document = self.document.borrow_mut(py);
        // A paragraph without a checked route (one in a nested block content
        // control) retires every handle.
        let native = document.paragraph_route(&location, None).ok();
        if let Some(native) = &native
            && document.try_local_cell_paragraph_text(py, native, text)?
        {
            return Ok(());
        }
        if let ParagraphLocation::Body(index) = location
            && document
                .edit_body_paragraph_in_place(py, index, "Paragraph.text", |paragraph| {
                    paragraph.set_text(text)
                })?
                .is_some()
        {
            return Ok(());
        }
        let mut candidate = document.inner.clone_for_staging();
        location
            .edit(&mut candidate, |paragraph| paragraph.set_text(text))
            .map_err(|error| crate::docx_to_pyerr(py, error))?;
        let publication = native.map_or(
            crate::document::WordPublication::Conservative,
            crate::document::WordPublication::ParagraphEdit,
        );
        document.publish_block_edit(py, candidate, "Paragraph.text", publication)
    }

    #[getter]
    fn runs(&self, py: Python<'_>) -> PyResult<Py<PyRunCollection>> {
        self.validate(py)?;
        let path = match &self.owner {
            Some(owner) => {
                let document = self.document.borrow(py);
                document.capture_current_view(
                    owner,
                    self.current_path(&document).segs,
                    crate::document::WordView::Content,
                    self.furniture_scope.as_ref(),
                )?
            }
            None => self.path.clone(),
        };
        Py::new(
            py,
            PyRunCollection::new(
                self.document.clone_ref(py),
                path,
                self.related.clone(),
                self.owner.clone(),
                self.furniture_scope.clone(),
            ),
        )
    }

    #[getter]
    fn alignment(&self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        let location = self.validate(py)?;
        let document = self.document.borrow(py);
        let alignment = location
            .read(&document.inner, |paragraph| paragraph.alignment())
            .map_err(|error| crate::docx_to_pyerr(py, error))?;
        alignment
            .map(|value| {
                crate::enum_object(
                    py,
                    "WD_ALIGN_PARAGRAPH",
                    crate::formatting::alignment_to_int(value),
                )
            })
            .transpose()
    }

    #[setter]
    fn set_alignment(&self, py: Python<'_>, value: Option<i32>) -> PyResult<()> {
        let location = self.validate(py)?;
        let value = value
            .map(crate::formatting::alignment_from_int)
            .transpose()?;
        let mut document = self.document.borrow_mut(py);
        location
            .edit(&mut document.inner, |paragraph| {
                paragraph.set_alignment_value(value);
                Ok(())
            })
            .map_err(|error| crate::docx_to_pyerr(py, error))?;
        Ok(())
    }

    fn add_run(&self, py: Python<'_>, text: &str) -> PyResult<Py<PyRun>> {
        self.append_run(py, text, None, None)
    }

    #[pyo3(signature = (text, url=None, *, tooltip=None, anchor=None))]
    fn add_hyperlink(
        &self,
        py: Python<'_>,
        text: &str,
        url: Option<&str>,
        tooltip: Option<&str>,
        anchor: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyRun>> {
        self.validate(py)?;
        if anchor.is_some() {
            return Err(pyo3::exceptions::PyNotImplementedError::new_err(
                "use document.add_bookmark(...) then paragraph.add_internal_hyperlink(text, bookmark); anchor=Paragraph does not infer bookmarks",
            ));
        }
        let url = url.ok_or_else(|| {
            PyTypeError::new_err(
                "use add_hyperlink(text, url) or add_internal_hyperlink(text, bookmark)",
            )
        })?;
        if let Some(bookmark) = url.strip_prefix('#') {
            self.append_internal_run(py, text, bookmark, tooltip)
        } else {
            self.append_run(py, text, Some(url), tooltip)
        }
    }

    #[pyo3(signature = (text, bookmark, *, tooltip=None))]
    fn add_internal_hyperlink(
        &self,
        py: Python<'_>,
        text: &str,
        bookmark: &str,
        tooltip: Option<&str>,
    ) -> PyResult<Py<PyRun>> {
        self.append_internal_run(py, text, bookmark, tooltip)
    }

    #[getter]
    fn paragraph_format(
        &self,
        py: Python<'_>,
    ) -> PyResult<Py<crate::formatting::PyParagraphFormat>> {
        self.validate(py)?;
        let path = match &self.owner {
            Some(owner) => {
                let document = self.document.borrow(py);
                document
                    .capture_owner(
                        &crate::document::WordOwnerLocation::Paragraph(
                            document.current_paragraph_owner(owner)?,
                        ),
                        self.current_path(&document).segs,
                        crate::document::WordView::Properties,
                        self.furniture_scope.as_ref(),
                    )?
                    .0
            }
            None => self.path.clone(),
        };
        Py::new(
            py,
            crate::formatting::PyParagraphFormat::new(
                self.document.clone_ref(py),
                path,
                self.related.clone(),
                self.owner.clone(),
            ),
        )
    }

    #[getter]
    fn style(&self, py: Python<'_>) -> PyResult<Option<String>> {
        let location = self.validate(py)?;
        Ok(crate::formatting::paragraph_snapshot(py, &self.document, location)?.style_id)
    }

    #[setter]
    fn set_style(&self, py: Python<'_>, value: Option<String>) -> PyResult<()> {
        let location = self.validate(py)?;
        let value = value
            .map(|value| {
                style_id_of_type(
                    &self.document.borrow(py).inner,
                    &value,
                    oxoffice_docx::StyleType::Paragraph,
                )
            })
            .transpose()?;
        crate::formatting::apply_paragraph_update(
            py,
            &self.document,
            location,
            crate::formatting::ParagraphUpdate::Style(value),
        )
    }

    #[getter]
    fn numbering(&self, py: Python<'_>) -> PyResult<Option<(u32, u32)>> {
        let location = self.validate(py)?;
        Ok(crate::formatting::paragraph_snapshot(py, &self.document, location)?.numbering)
    }

    #[setter]
    fn set_numbering(&self, py: Python<'_>, value: Option<(u32, u32)>) -> PyResult<()> {
        let location = self.validate(py)?;
        if value.is_some_and(|(_, level)| level > 8) {
            return Err(PyValueError::new_err(
                "numbering level must be between 0 and 8",
            ));
        }
        crate::formatting::apply_paragraph_update(
            py,
            &self.document,
            location,
            crate::formatting::ParagraphUpdate::Numbering(value),
        )
    }
}

#[pyclass(name = "ParagraphCollection")]
pub struct PyParagraphCollection {
    document: Py<PyDocument>,
}

impl PyParagraphCollection {
    pub(crate) fn new(document: Py<PyDocument>) -> Self {
        Self { document }
    }

    fn item(&self, py: Python<'_>, index: usize) -> PyResult<Py<PyParagraph>> {
        let path = {
            let document = self.document.borrow(py);
            document
                .revisions
                .capture(smallvec![PathSeg::Body(0), PathSeg::Para(index)])
        };
        Py::new(
            py,
            PyParagraph::new(self.document.clone_ref(py), path).bind_current(py, None)?,
        )
    }
}

#[pymethods]
impl PyParagraphCollection {
    fn __len__(&self, py: Python<'_>) -> usize {
        self.document
            .borrow(py)
            .cached_body_paragraph_count()
            .unwrap_or_else(|| self.document.borrow(py).inner.paragraph_count())
    }

    fn __getitem__(&self, py: Python<'_>, key: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let len = self.__len__(py);
        if let Ok(index) = key.extract::<isize>() {
            return Ok(self
                .item(py, normalize_index(index, len, "paragraph")?)?
                .into_any());
        }
        if key.is_instance_of::<PySlice>() {
            let (start, stop, step): (isize, isize, isize) =
                key.call_method1("indices", (len,))?.extract()?;
            let items = PyList::empty(py);
            let mut index = start;
            while if step > 0 { index < stop } else { index > stop } {
                items.append(self.item(py, index as usize)?)?;
                index += step;
            }
            return Ok(items.into_any().unbind());
        }
        Err(PyTypeError::new_err(
            "paragraph indices must be integers or slices",
        ))
    }

    fn __iter__(&self, py: Python<'_>) -> PyResult<Py<PyParagraphIterator>> {
        Py::new(
            py,
            PyParagraphIterator {
                document: self.document.clone_ref(py),
                index: 0,
            },
        )
    }
}

#[pyclass]
struct PyParagraphIterator {
    document: Py<PyDocument>,
    index: usize,
}

#[pymethods]
impl PyParagraphIterator {
    fn __iter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    fn __next__(&mut self, py: Python<'_>) -> PyResult<Option<Py<PyParagraph>>> {
        let len = self
            .document
            .borrow(py)
            .cached_body_paragraph_count()
            .unwrap_or_else(|| self.document.borrow(py).inner.paragraph_count());
        if self.index >= len {
            return Ok(None);
        }
        let index = self.index;
        self.index += 1;
        let path = self
            .document
            .borrow(py)
            .revisions
            .capture(smallvec![PathSeg::Body(0), PathSeg::Para(index)]);
        Py::new(
            py,
            PyParagraph::new(self.document.clone_ref(py), path).bind_current(py, None)?,
        )
        .map(Some)
    }
}
