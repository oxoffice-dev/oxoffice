//! The Document class's Python methods: one `#[pymethods]` block, as PyO3 allows one per class.

use super::*;

#[pymethods]
impl PyDocument {
    #[getter]
    fn default_font_name(&self) -> Option<String> {
        self.inner.default_font_name().map(str::to_owned)
    }

    #[setter]
    fn set_default_font_name(&mut self, py: Python<'_>, value: Option<&str>) -> PyResult<()> {
        self.inner
            .set_default_font_name_value(value)
            .map_err(|error| docx_to_pyerr(py, error))
    }

    #[getter]
    fn default_font_size(&self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        self.inner
            .default_font_size()
            .map(|value| crate::length_object(py, value))
            .transpose()
    }

    #[setter]
    fn set_default_font_size(
        &mut self,
        py: Python<'_>,
        value: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let value = crate::length_argument(
            value,
            "Document.default_font_size",
            crate::LengthUnit::HalfPoints,
        )?
        .map(oxoffice_docx::Length::emu);
        if value.is_some_and(|value| {
            value.to_emu() < 0 || u32::try_from(value.to_emu() / 6350).is_err()
        }) {
            return Err(PyValueError::new_err(
                "Document.default_font_size must fit nonnegative half-points",
            ));
        }
        self.inner
            .set_default_font_size_value(value)
            .map_err(|error| docx_to_pyerr(py, error))
    }

    #[new]
    #[pyo3(signature = (path = None))]
    fn new(path: Option<&Bound<'_, PyAny>>, py: Python<'_>) -> PyResult<Self> {
        let inner = match path {
            Some(path) => open_document_input(py, path)?,
            None => oxoffice_docx::Document::new(),
        };
        Self::from_document(inner, py)
    }

    #[staticmethod]
    fn open(path: &Bound<'_, PyAny>, py: Python<'_>) -> PyResult<Self> {
        Self::from_document(open_document_input(py, path)?, py)
    }

    #[staticmethod]
    fn from_bytes(bytes: &[u8], py: Python<'_>) -> PyResult<Self> {
        oxoffice_docx::Document::from_bytes(bytes)
            .map_err(|error| docx_to_pyerr(py, error))
            .and_then(|inner| Self::from_document(inner, py))
    }

    #[staticmethod]
    #[pyo3(signature = (html, *, images=Vec::new()))]
    fn from_html<'py>(
        py: Python<'py>,
        html: &str,
        images: Vec<(String, Py<PyBytes>, String)>,
    ) -> PyResult<(Py<Self>, Bound<'py, PyTuple>)> {
        Self::imported_text(py, html, false, None, &images)
    }

    #[staticmethod]
    #[pyo3(signature = (path, *, images=Vec::new()))]
    fn open_html<'py>(
        py: Python<'py>,
        path: PathBuf,
        images: Vec<(String, Py<PyBytes>, String)>,
    ) -> PyResult<(Py<Self>, Bound<'py, PyTuple>)> {
        Self::imported_text(py, &read_import_text(path)?, false, None, &images)
    }

    #[staticmethod]
    #[pyo3(signature = (markdown, *, template=None, images=Vec::new()))]
    fn from_markdown<'py>(
        py: Python<'py>,
        markdown: &str,
        template: Option<&Bound<'py, PyAny>>,
        images: Vec<(String, Py<PyBytes>, String)>,
    ) -> PyResult<(Py<Self>, Bound<'py, PyTuple>)> {
        Self::imported_text(py, markdown, true, template, &images)
    }

    #[staticmethod]
    #[pyo3(signature = (path, *, template=None, images=Vec::new()))]
    fn open_markdown<'py>(
        py: Python<'py>,
        path: PathBuf,
        template: Option<&Bound<'py, PyAny>>,
        images: Vec<(String, Py<PyBytes>, String)>,
    ) -> PyResult<(Py<Self>, Bound<'py, PyTuple>)> {
        Self::imported_text(py, &read_import_text(path)?, true, template, &images)
    }

    #[pyo3(signature = (html, *, after, images=Vec::new()))]
    fn insert_html<'py>(
        slf: Py<Self>,
        py: Python<'py>,
        html: &str,
        after: PyRef<'_, PyParagraph>,
        images: Vec<(String, Py<PyBytes>, String)>,
    ) -> PyResult<Bound<'py, PyTuple>> {
        if !after.belongs_to(py, &slf) {
            return Err(PyValueError::new_err(
                "after must be a live paragraph of this document",
            ));
        }
        let paragraph = after.validate(py)?;
        let mut document = slf.borrow_mut(py);
        let location = paragraph
            .native(&document.inner)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let snapshots = document
            .inner
            .story_item_snapshots()
            .map_err(|error| docx_to_pyerr(py, error))?;
        let direct = snapshots
            .iter()
            .filter(|item| {
                item.location().story() == location.story()
                    && item.is_direct_child()
                    && (location.story().kind() != oxoffice_docx::StoryKind::Body
                        || item.direct_body_index().is_some())
            })
            .collect::<Vec<_>>();
        let index = direct
            .iter()
            .position(|item| item.location() == &location)
            .ok_or_else(|| {
                PyValueError::new_err("after must be a supported direct-child paragraph")
            })?;
        let destination = direct.get(index + 1).map_or_else(
            || oxoffice_docx::ContentLocation::end(location.story().clone()),
            |item| item.location().clone(),
        );
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
        let mut candidate = document.inner.clone_for_staging();
        let result = py
            .detach(|| candidate.insert_html_fragment(&destination, html, &resources))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let diagnostics = html_diagnostics(py, result.diagnostics)?;
        document.publish_api_candidate(py, candidate, "Document.insert_html")?;
        Ok(diagnostics)
    }

    /// Sign current typed content with DER or one unencrypted PKCS#8/X.509 PEM block.
    fn sign(
        &mut self,
        private_key: &[u8],
        certificate: &[u8],
        py: Python<'_>,
    ) -> PyResult<Py<pyo3::types::PyDict>> {
        let mut candidate = self.inner.clone_for_staging();
        let report = py
            .detach(|| candidate.sign(private_key, certificate))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let result = oxoffice_py_support::signature_report_to_py(py, &report)?;
        let prepared =
            self.prepare_word_revision(py, &candidate, "Document.sign", WordPublication::Signing)?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(result)
    }

    /// Verify serialized current content, without publishing state or asserting chain trust.
    fn verify_signatures(&self, py: Python<'_>) -> PyResult<Py<PyTuple>> {
        let reports = py
            .detach(|| {
                let mut current = self.inner.clone_for_staging();
                let bytes = current.to_bytes()?;
                oxoffice_docx::Document::from_bytes(&bytes)?
                    .verify_signatures()
                    .map_err(oxoffice_docx::Error::from)
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        let reports = reports
            .iter()
            .map(|report| oxoffice_py_support::signature_report_to_py(py, report))
            .collect::<PyResult<Vec<_>>>()?;
        Ok(PyTuple::new(py, reports)?.unbind())
    }

    /// Save to a path or a binary stream. A document opened from a file
    /// refuses to replace that file unless `overwrite_source` is true, unlike
    /// python-docx.
    #[pyo3(signature = (path, *, overwrite_source = false))]
    fn save(
        &mut self,
        path: &Bound<'_, PyAny>,
        overwrite_source: bool,
        py: Python<'_>,
    ) -> PyResult<()> {
        if let Ok(path) = path.extract::<PathBuf>() {
            if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
            {
                return Err(PyValueError::new_err(
                    "use to_pdf() to export a PDF instead of save()",
                ));
            }
            let options = oxoffice_docx::SaveOptions { overwrite_source };
            return py
                .detach(|| self.inner.save_with_options(path, options))
                .map_err(|error| docx_to_pyerr(py, error));
        }
        let write = path.getattr("write").map_err(|_| PyTypeError::new_err(
            "save requires a filesystem path or binary writable stream; use stream.write(document.to_bytes())",
        ))?;
        let bytes = self
            .inner
            .to_bytes()
            .map_err(|error| docx_to_pyerr(py, error))?;
        let written = write.call1((PyBytes::new(py, &bytes),))?;
        if written.extract::<usize>().ok() != Some(bytes.len()) {
            return Err(pyo3::exceptions::PyOSError::new_err(
                "binary stream write returned a short or invalid byte count",
            ));
        }
        Ok(())
    }

    #[pyo3(name = "to_bytes")]
    fn serialize<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        py.detach(|| self.inner.to_bytes())
            .map(|bytes| PyBytes::new(py, &bytes))
            .map_err(|error| docx_to_pyerr(py, error))
    }

    /// The body as a complete HTML document: comments, notes, headers and footers are left out
    /// (`oxoffice docx convert` appends them); equations are written as MathML.
    fn to_html(&self, py: Python<'_>) -> String {
        py.detach(|| self.inner.to_html())
    }
    /// The body as Markdown: comments, notes, headers and footers are left out
    /// (`oxoffice docx convert` appends them); equations are written as LaTeX.
    fn to_markdown(&self, py: Python<'_>) -> String {
        py.detach(|| self.inner.to_markdown())
    }
    fn text(&self, py: Python<'_>) -> String {
        py.detach(|| self.inner.text_report().text)
    }
    fn text_report(&self, py: Python<'_>) -> PyTextReport {
        py.detach(|| self.inner.text_report()).into()
    }
    fn word_count(&self) -> usize {
        self.inner.word_count()
    }
    #[pyo3(signature = (*, include_spaces = true))]
    fn character_count(&self, include_spaces: bool) -> usize {
        self.inner.character_count(include_spaces)
    }
    fn validate(&self, py: Python<'_>) -> PyResult<PyValidationReport> {
        py.detach(|| self.inner.validate())
            .map(Into::into)
            .map_err(|error| docx_to_pyerr(py, error))
    }
    fn to_odt(&self, py: Python<'_>) -> PyResult<PyOdtExportResult> {
        let result = py
            .detach(|| self.inner.to_odt_bytes())
            .map_err(|error| docx_to_pyerr(py, error))?;
        Ok(PyOdtExportResult {
            bytes: result.bytes,
            diagnostics: result
                .diagnostics
                .into_iter()
                .map(|d| PyOdtDiagnostic {
                    path: d.path,
                    message: d.message,
                })
                .collect(),
        })
    }
    fn to_rtf(&self, py: Python<'_>) -> PyResult<PyRtfExportResult> {
        let result = py
            .detach(|| self.inner.to_rtf_bytes())
            .map_err(|error| docx_to_pyerr(py, error))?;
        Ok(PyRtfExportResult {
            bytes: result.bytes,
            diagnostics: result
                .diagnostics
                .into_iter()
                .map(|d| PyRtfDiagnostic {
                    offset: d.offset,
                    destination: d.destination,
                    message: d.message,
                })
                .collect(),
        })
    }
    fn to_epub(&self, py: Python<'_>) -> PyResult<PyEpubExportResult> {
        let result = py
            .detach(|| self.inner.to_epub_bytes())
            .map_err(|error| docx_to_pyerr(py, error))?;
        Ok(PyEpubExportResult {
            bytes: result.bytes,
            diagnostics: result
                .diagnostics
                .into_iter()
                .map(|d| PyEpubDiagnostic {
                    path: d.path,
                    message: d.message,
                })
                .collect(),
        })
    }
    #[pyo3(signature = (*, fonts = None, font_dir = None, font_dirs = None, system_fonts = false, bundled_fonts_only = false))]
    fn page_count<'py>(
        &self,
        py: Python<'py>,
        fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
        font_dir: Option<PathBuf>,
        font_dirs: Option<Bound<'py, PyAny>>,
        system_fonts: bool,
        bundled_fonts_only: bool,
    ) -> PyResult<usize> {
        let caller_fonts = collect_caller_fonts(py, fonts, font_dir, font_dirs)?;
        let refs = caller_fonts.as_ref().map(|files| {
            files
                .iter()
                .map(|font| (font.family.as_str(), font.data.as_slice()))
                .collect::<Vec<_>>()
        });
        let options = with_font_switches(
            oxoffice_docx::RenderOptions::default(),
            system_fonts,
            bundled_fonts_only,
        )?;
        py.detach(|| {
            self.inner
                .layout_for_render(refs.as_deref().unwrap_or(&[]), options)
                .map(|layout| layout.layout.pages.len())
        })
        .map_err(|error| docx_to_pyerr(py, error))
    }
    /// Render `{{ path }}` tags from data; comments are left as written.
    #[pyo3(signature = (data, *, grammar_version=None))]
    fn render_template(
        &mut self,
        py: Python<'_>,
        data: &Bound<'_, PyAny>,
        grammar_version: Option<u32>,
    ) -> PyResult<usize> {
        if !data.is_instance_of::<PyDict>() {
            return Err(PyTypeError::new_err(
                "render_template data must be a dict with string keys",
            ));
        }
        let data = template_value(data, "data", &mut Vec::new())?;
        let before = self
            .inner
            .to_bytes()
            .map_err(|error| docx_to_pyerr(py, error))?;
        let mut candidate = self.inner.clone_for_staging();
        let count = py
            .detach(|| match grammar_version {
                None => candidate.render_template(&data),
                Some(version) => candidate.render_template_versioned(&data, version),
            })
            .map_err(|error| template_render_pyerr(py, error))?;
        let after = candidate
            .to_bytes()
            .map_err(|error| docx_to_pyerr(py, error))?;
        if after != before {
            let prepared = self.prepare_word_revision(
                py,
                &candidate,
                "Document.render_template",
                crate::document::WordPublication::Conservative,
            )?;
            self.commit_word_revision(prepared)?;
            self.install_inner(candidate);
        }
        Ok(count)
    }

    fn validate_form<'py>(
        &self,
        py: Python<'py>,
        rules: &Bound<'_, PyAny>,
    ) -> PyResult<Bound<'py, PyTuple>> {
        let rules = copied_rules(py, rules)?;
        let issues = self
            .inner
            .validate_form(&rules)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let issues = issues
            .into_iter()
            .map(|issue| issue_to_py(py, issue))
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, issues)
    }

    fn fill_form(
        &mut self,
        py: Python<'_>,
        rules: &Bound<'_, PyAny>,
        assignments: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyTuple>> {
        let rules = copied_rules(py, rules)?;
        let assignments = copied_assignments(py, assignments)?;
        let mut candidate = self.inner.clone_for_staging();
        let counts = candidate
            .fill_form(&rules, &assignments)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let result = PyTuple::new(py, counts)?.unbind();
        self.publish_form_candidate(
            py,
            candidate,
            "Document.fill_form",
            WordPublication::ModernFormValues,
        )?;
        Ok(result)
    }

    fn reset_form(&mut self, py: Python<'_>, rules: &Bound<'_, PyAny>) -> PyResult<Py<PyTuple>> {
        let rules = copied_rules(py, rules)?;
        let mut candidate = self.inner.clone_for_staging();
        let counts = candidate
            .reset_form(&rules)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let result = PyTuple::new(py, counts)?.unbind();
        self.publish_form_candidate(
            py,
            candidate,
            "Document.reset_form",
            WordPublication::ModernFormValues,
        )?;
        Ok(result)
    }

    fn protect_form(
        &mut self,
        py: Python<'_>,
        rules: &Bound<'_, PyAny>,
        protection: &PyDocumentProtection,
    ) -> PyResult<()> {
        let rules = copied_rules(py, rules)?;
        let protection = protection.inner.clone();
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .protect_form(&rules, protection)
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.publish_form_candidate(
            py,
            candidate,
            "Document.protect_form",
            WordPublication::ModernFormValues,
        )?;
        Ok(())
    }

    #[getter]
    fn document_protection(&self) -> Option<PyDocumentProtection> {
        self.inner
            .document_protection()
            .cloned()
            .map(|inner| PyDocumentProtection { inner })
    }

    fn set_document_protection(
        &mut self,
        py: Python<'_>,
        value: &PyDocumentProtection,
    ) -> PyResult<()> {
        let value = value.inner.clone();
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .set_document_protection(value)
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.publish_form_candidate(
            py,
            candidate,
            "Document.set_document_protection",
            WordPublication::ModernFormValues,
        )?;
        Ok(())
    }

    fn remove_document_protection(
        &mut self,
        py: Python<'_>,
    ) -> PyResult<Option<Py<PyDocumentProtection>>> {
        // Py::new runs before publication. Allocation failure here is a source
        // ordering counterfactual, not an observed allocation RED.
        let snapshot = self
            .inner
            .document_protection()
            .cloned()
            .map(|inner| Py::new(py, PyDocumentProtection { inner }))
            .transpose()?;
        let mut candidate = self.inner.clone_for_staging();
        let removed = candidate
            .remove_document_protection()
            .map_err(|error| docx_to_pyerr(py, error))?;
        if removed.is_none() {
            return Ok(None);
        }
        self.publish_form_candidate(
            py,
            candidate,
            "Document.remove_document_protection",
            WordPublication::ModernFormValues,
        )?;
        Ok(snapshot)
    }

    #[pyo3(
        signature = (range, *, editor=None, group=None),
        text_signature = "($self, range, *, editor=None, group=None)"
    )]
    fn add_story_permission_range(
        &mut self,
        py: Python<'_>,
        range: &Bound<'_, PyAny>,
        editor: Option<String>,
        group: Option<String>,
    ) -> PyResult<Py<PyInt>> {
        let range = range
            .cast::<PyStoryRunRange>()
            .map_err(|_| PyTypeError::new_err("range must be a StoryRunRange"))?;
        let range = range.borrow();
        let start = oxoffice_docx::StoryRunPosition {
            location: self.native_location(py, &range.start.item)?,
            run_index: range.start.run_index,
        };
        let end = oxoffice_docx::StoryRunPosition {
            location: self.native_location(py, &range.end.item)?,
            run_index: range.end.run_index,
        };
        drop(range);
        let mut candidate = self.inner.clone_for_staging();
        let id = candidate
            .add_story_permission_range(
                editor.as_deref(),
                group.as_deref(),
                oxoffice_docx::StoryRunRange { start, end },
            )
            .map_err(|error| docx_to_pyerr(py, error))?;
        let id = id.into_pyobject(py)?.unbind();
        self.publish_api_candidate(py, candidate, "Document.add_story_permission_range")?;
        Ok(id)
    }

    fn bind_content_controls(
        &mut self,
        py: Python<'_>,
        values: &Bound<'_, PyDict>,
    ) -> PyResult<Py<PyInt>> {
        let values = values
            .iter()
            .map(|(key, value)| Ok((key.extract::<String>()?, value.extract::<String>()?)))
            .collect::<PyResult<HashMap<_, _>>>()?;
        let mut candidate = self.inner.clone_for_staging();
        let count = py
            .detach(|| candidate.bind_content_controls(&values))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let result = count.into_pyobject(py)?.unbind();
        self.publish_form_candidate(
            py,
            candidate,
            "Document.bind_content_controls",
            WordPublication::FormValues,
        )?;
        Ok(result)
    }

    fn legacy_form_fields<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let fields = self
            .inner
            .legacy_form_fields_with_defaults()
            .map_err(|error| docx_to_pyerr(py, error))?;
        let fields = fields
            .into_iter()
            .map(|(field, default_value)| {
                Py::new(py, PyLegacyFormField::from_info(py, field, default_value)?)
            })
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, fields)
    }

    // Bibliography; the bodies are in `bibliography.rs`.

    fn bibliography_sources<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        self.bibliography_sources_impl(py)
    }

    /// Add a source and return its GUID: the one it holds, or one derived
    /// from its content when it holds none.
    fn add_bibliography_source(
        &mut self,
        py: Python<'_>,
        source: &Bound<'_, PyAny>,
    ) -> PyResult<String> {
        self.add_bibliography_source_impl(py, source)
    }

    /// Replace the source with the same tag, keeping its GUID: an empty
    /// `guid` is the one the source has.
    fn replace_bibliography_source(
        &mut self,
        py: Python<'_>,
        source: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        self.replace_bibliography_source_impl(py, source)
    }

    fn remove_bibliography_source(&mut self, py: Python<'_>, tag: &str) -> PyResult<()> {
        self.remove_bibliography_source_impl(py, tag)
    }

    fn bibliography_style(&self, py: Python<'_>) -> PyResult<Option<PyBibliographyStyleInfo>> {
        self.bibliography_style_impl(py)
    }

    /// Set the style, and the selectors of every bibliography in the document.
    #[pyo3(signature = (style, *, locale = None, locale_filter = None, tags = None))]
    fn set_bibliography_options(
        &mut self,
        py: Python<'_>,
        style: &str,
        locale: Option<u32>,
        locale_filter: Option<u32>,
        tags: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        self.set_bibliography_options_impl(py, style, locale, locale_filter, tags)
    }

    #[pyo3(signature = (position, sources, *, locale = None))]
    fn insert_citation(
        &mut self,
        py: Python<'_>,
        position: &Bound<'_, PyAny>,
        sources: &Bound<'_, PyAny>,
        locale: Option<u32>,
    ) -> PyResult<()> {
        self.insert_citation_impl(py, position, sources, locale)
    }

    /// Cite right after the `occurrence`-th match of `text` in the main
    /// story, found as `add_comment_on_text` finds its anchor.
    #[pyo3(signature = (text, sources, *, occurrence = 0, locale = None))]
    fn insert_citation_after_text(
        &mut self,
        py: Python<'_>,
        text: &str,
        sources: &Bound<'_, PyAny>,
        occurrence: usize,
        locale: Option<u32>,
    ) -> PyResult<()> {
        self.insert_citation_after_text_impl(py, text, sources, occurrence, locale)
    }

    /// Insert a bibliography where `insert_content` would put content, and
    /// set the style.
    #[pyo3(signature = (destination, style, *, locale = None, locale_filter = None, tags = None))]
    #[allow(clippy::too_many_arguments)]
    fn insert_bibliography(
        &mut self,
        py: Python<'_>,
        destination: &Bound<'_, PyAny>,
        style: &str,
        locale: Option<u32>,
        locale_filter: Option<u32>,
        tags: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        self.insert_bibliography_impl(py, destination, style, locale, locale_filter, tags)
    }

    /// Rebuild every citation and bibliography result. A field not covered
    /// yet fails the whole update and changes nothing.
    #[pyo3(signature = (*, default_locale = None))]
    fn update_bibliography(
        &mut self,
        py: Python<'_>,
        default_locale: Option<u32>,
    ) -> PyResult<PyBibliographyUpdateReport> {
        self.update_bibliography_impl(py, default_locale)
    }

    fn insert_legacy_form_field(
        &mut self,
        py: Python<'_>,
        position: &Bound<'_, PyAny>,
        definition: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let definition = definition.cast::<PyLegacyFormFieldDefinition>()?.borrow();
        let position = position.cast::<PyStoryRunPosition>()?.borrow();
        let field = definition.field();
        let mut candidate = self.inner.clone_for_staging();
        let location = self.native_location(py, &position.item)?;
        let native = oxoffice_docx::StoryRunPosition {
            location,
            run_index: position.run_index,
        };
        py.detach(|| candidate.insert_field(&native, field))
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.publish_api_candidate(py, candidate, "Document.insert_legacy_form_field")?;
        Ok(())
    }

    #[pyo3(signature = (name, value, *, source_part=None, ordinal=None))]
    fn set_legacy_form_field_value(
        &mut self,
        py: Python<'_>,
        name: Option<&str>,
        value: &Bound<'_, PyAny>,
        source_part: Option<&str>,
        ordinal: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyLegacyFormField>> {
        let ordinal = ordinal
            .map(|value| {
                if !value.is_instance_of::<PyInt>() || value.is_instance_of::<PyBool>() {
                    return Err(PyTypeError::new_err(
                        "legacy field ordinal must be a nonnegative int, not bool",
                    ));
                }
                value.extract::<usize>()
            })
            .transpose()?;
        if source_part.is_some() != ordinal.is_some() {
            return Err(PyValueError::new_err(
                "source_part and ordinal must be supplied together",
            ));
        }
        if source_part.is_none() && name.is_none() {
            return Err(PyValueError::new_err(
                "name is required without source_part and ordinal",
            ));
        }
        let fields = self
            .inner
            .legacy_form_fields()
            .map_err(|error| docx_to_pyerr(py, error))?;
        let mut matches = fields.into_iter().filter(|field| {
            if let (Some(part), Some(ordinal)) = (source_part, ordinal) {
                field.source_part == part
                    && field.ordinal == ordinal
                    && name.is_none_or(|name| field.name.as_deref() == Some(name))
            } else {
                field.name.as_deref() == name
            }
        });
        let field = matches
            .next()
            .ok_or_else(|| PyValueError::new_err("unknown or stale legacy form field identity"))?;
        if matches.next().is_some() {
            return Err(PyValueError::new_err(
                "ambiguous legacy form field name; supply source_part and ordinal",
            ));
        }
        let value = match field.kind {
            oxoffice_docx::LegacyFormFieldKind::TextInput => {
                oxoffice_docx::LegacyFormFieldValue::Text(value.extract::<String>()?)
            }
            oxoffice_docx::LegacyFormFieldKind::CheckBox => {
                if !value.is_instance_of::<PyBool>() {
                    return Err(PyTypeError::new_err("legacy checkbox requires bool"));
                }
                oxoffice_docx::LegacyFormFieldValue::Checked(value.extract::<bool>()?)
            }
            oxoffice_docx::LegacyFormFieldKind::DropDownList => {
                if !value.is_instance_of::<PyInt>() || value.is_instance_of::<PyBool>() {
                    return Err(PyTypeError::new_err(format!(
                        "legacy dropdown received {}. Use a nonnegative integer index, not bool, from document.legacy_form_fields() snapshot choices",
                        value.get_type().name()?,
                    )));
                }
                oxoffice_docx::LegacyFormFieldValue::SelectedIndex(value.extract::<usize>()?)
            }
        };
        let mut candidate = self.inner.clone_for_staging();
        let reopened = py
            .detach(|| {
                candidate.set_legacy_form_field_value(&field.source_part, field.ordinal, value)
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        let default_value = candidate
            .legacy_form_fields_with_defaults()
            .map_err(|error| docx_to_pyerr(py, error))?
            .into_iter()
            .find(|(info, _)| {
                info.source_part == reopened.source_part && info.ordinal == reopened.ordinal
            })
            .map(|(_, default_value)| default_value)
            .ok_or_else(|| PyValueError::new_err("legacy form identity did not survive reopen"))?;
        let prepared = Py::new(
            py,
            PyLegacyFormField::from_info(py, reopened, default_value)?,
        )?;
        self.publish_form_candidate(
            py,
            candidate,
            "Document.set_legacy_form_field_value",
            WordPublication::FormValues,
        )?;
        Ok(prepared)
    }

    fn content_controls<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(
            py,
            self.inner
                .content_controls()
                .into_iter()
                .map(PyContentControl::from),
        )
    }
    fn set_content_control_value_by_tag(
        &mut self,
        py: Python<'_>,
        tag: &str,
        value: &str,
    ) -> PyResult<usize> {
        self.set_control_value(py, tag, value, true)
    }
    fn fill_content_control_tags<'py>(
        &mut self,
        py: Python<'py>,
        pairs: Vec<(String, String)>,
    ) -> PyResult<Bound<'py, PyTuple>> {
        let pairs = pairs
            .iter()
            .map(|(tag, value)| (tag.as_str(), value.as_str()))
            .collect::<Vec<_>>();
        let mut candidate = self.inner.clone_for_staging();
        let counts = py
            .detach(|| candidate.fill_content_control_tags(&pairs))
            .map_err(|error| docx_to_pyerr(py, error))?;

        let result = PyTuple::new(py, counts)?;
        self.publish_api_candidate(py, candidate, "Document.fill_content_control_tags")?;

        Ok(result)
    }

    #[allow(clippy::too_many_arguments)] // Q21 keeps each public operand explicit.
    #[pyo3(signature = (destination, placement, kind, tag=None, alias=None, lock=None, appearance=None, placeholder=None))]
    fn insert_content_control(
        &mut self,
        py: Python<'_>,
        destination: PyRef<'_, PyContentLocation>,
        placement: &str,
        kind: &str,
        tag: Option<String>,
        alias: Option<String>,
        lock: Option<String>,
        appearance: Option<String>,
        placeholder: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let destination = destination.native(py, self)?;
        let spec = oxoffice_docx::ContentControlSpec {
            placement: content_control_placement_from_name(placement)?,
            kind: content_control_kind_from_name(kind)?,
            tag,
            alias,
            lock: lock
                .as_deref()
                .map(content_control_lock_from_name)
                .transpose()?,
            appearance: appearance
                .as_deref()
                .map(content_control_appearance_from_name)
                .transpose()?,
            placeholder: optional_python_bool(placeholder, "placeholder")?,
        };
        let mut candidate = self.inner.clone_for_staging();
        py.detach(|| candidate.insert_content_control(&destination, spec))
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.publish_api_candidate(py, candidate, "Document.insert_content_control")
    }

    #[pyo3(signature = (location, tag=None))]
    fn set_content_control_tag(
        &mut self,
        py: Python<'_>,
        location: PyRef<'_, PyContentLocation>,
        tag: Option<String>,
    ) -> PyResult<()> {
        self.edit_content_control(
            py,
            &location,
            "Document.set_content_control_tag",
            |document, location| document.set_content_control_tag(location, tag.as_deref()),
        )
    }

    #[pyo3(signature = (location, alias=None))]
    fn set_content_control_alias(
        &mut self,
        py: Python<'_>,
        location: PyRef<'_, PyContentLocation>,
        alias: Option<String>,
    ) -> PyResult<()> {
        self.edit_content_control(
            py,
            &location,
            "Document.set_content_control_alias",
            |document, location| document.set_content_control_alias(location, alias.as_deref()),
        )
    }

    #[pyo3(signature = (location, lock=None))]
    fn set_content_control_lock(
        &mut self,
        py: Python<'_>,
        location: PyRef<'_, PyContentLocation>,
        lock: Option<String>,
    ) -> PyResult<()> {
        let lock = lock
            .as_deref()
            .map(content_control_lock_from_name)
            .transpose()?;
        self.edit_content_control(
            py,
            &location,
            "Document.set_content_control_lock",
            |document, location| document.set_content_control_lock(location, lock),
        )
    }

    #[pyo3(signature = (location, appearance=None))]
    fn set_content_control_appearance(
        &mut self,
        py: Python<'_>,
        location: PyRef<'_, PyContentLocation>,
        appearance: Option<String>,
    ) -> PyResult<()> {
        let appearance = appearance
            .as_deref()
            .map(content_control_appearance_from_name)
            .transpose()?;
        self.edit_content_control(
            py,
            &location,
            "Document.set_content_control_appearance",
            |document, location| document.set_content_control_appearance(location, appearance),
        )
    }

    #[pyo3(signature = (location, placeholder=None))]
    fn set_content_control_placeholder(
        &mut self,
        py: Python<'_>,
        location: PyRef<'_, PyContentLocation>,
        placeholder: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let placeholder = optional_python_bool(placeholder, "placeholder")?;
        self.edit_content_control(
            py,
            &location,
            "Document.set_content_control_placeholder",
            |document, location| document.set_content_control_placeholder(location, placeholder),
        )
    }

    fn remove_content_control(
        &mut self,
        py: Python<'_>,
        location: PyRef<'_, PyContentLocation>,
    ) -> PyResult<()> {
        self.edit_content_control(
            py,
            &location,
            "Document.remove_content_control",
            |document, location| document.remove_content_control(location),
        )
    }

    fn unwrap_content_control(
        &mut self,
        py: Python<'_>,
        location: PyRef<'_, PyContentLocation>,
    ) -> PyResult<()> {
        self.edit_content_control(
            py,
            &location,
            "Document.unwrap_content_control",
            |document, location| document.unwrap_content_control(location),
        )
    }

    fn move_content_control(
        &mut self,
        py: Python<'_>,
        source: PyRef<'_, PyContentLocation>,
        destination: PyRef<'_, PyContentLocation>,
    ) -> PyResult<()> {
        let source = source.native(py, self)?;
        let destination = destination.native(py, self)?;
        let mut candidate = self.inner.clone_for_staging();
        py.detach(|| candidate.move_content_control(&source, &destination))
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.publish_api_candidate(py, candidate, "Document.move_content_control")
    }

    fn clone_content_control(
        &mut self,
        py: Python<'_>,
        source: PyRef<'_, PyContentLocation>,
        destination: PyRef<'_, PyContentLocation>,
    ) -> PyResult<()> {
        let source = source.native(py, self)?;
        let destination = destination.native(py, self)?;
        let mut candidate = self.inner.clone_for_staging();
        py.detach(|| candidate.clone_content_control(&source, &destination))
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.publish_api_candidate(py, candidate, "Document.clone_content_control")
    }

    fn set_content_control_type(
        &mut self,
        py: Python<'_>,
        location: PyRef<'_, PyContentLocation>,
        payload: PyRef<'_, PyContentControlTypePayload>,
    ) -> PyResult<()> {
        let payload = payload.inner.clone();
        self.edit_content_control(
            py,
            &location,
            "Document.set_content_control_type",
            move |document, location| document.set_content_control_type(location, payload),
        )
    }

    fn repeat_content_control(
        &mut self,
        py: Python<'_>,
        location: PyRef<'_, PyContentLocation>,
    ) -> PyResult<()> {
        self.edit_content_control(
            py,
            &location,
            "Document.repeat_content_control",
            |document, location| document.repeat_content_control(location),
        )
    }

    #[pyo3(signature = (item_xml, schema_uris=Vec::new()), text_signature = "($self, item_xml, schema_uris=())")]
    fn add_custom_xml_store(
        &mut self,
        py: Python<'_>,
        item_xml: &[u8],
        schema_uris: Vec<String>,
    ) -> PyResult<String> {
        let mut candidate = self.inner.clone_for_staging();
        let item_id = py
            .detach(|| {
                let uris = schema_uris.iter().map(String::as_str).collect::<Vec<_>>();
                candidate.add_custom_xml_store(item_xml, &uris)
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.publish_api_candidate(py, candidate, "Document.add_custom_xml_store")?;
        Ok(item_id)
    }

    #[pyo3(signature = (location, store_item_id, xpath, prefix_mappings=None))]
    fn set_content_control_binding(
        &mut self,
        py: Python<'_>,
        location: PyRef<'_, PyContentLocation>,
        store_item_id: String,
        xpath: String,
        prefix_mappings: Option<String>,
    ) -> PyResult<()> {
        self.edit_content_control(
            py,
            &location,
            "Document.set_content_control_binding",
            move |document, location| {
                document.set_content_control_binding(
                    location,
                    &store_item_id,
                    &xpath,
                    prefix_mappings.as_deref(),
                )
            },
        )
    }

    fn set_content_control_value_by_alias(
        &mut self,
        py: Python<'_>,
        alias: &str,
        value: &str,
    ) -> PyResult<usize> {
        self.set_control_value(py, alias, value, false)
    }
    #[pyo3(signature = (destination, fragment, *, policy=None))]
    fn import_fragment(
        &mut self,
        py: Python<'_>,
        destination: PyRef<'_, PyContentLocation>,
        fragment: PyRef<'_, PyDocumentFragment>,
        policy: Option<PyRef<'_, PyFragmentConflictPolicy>>,
    ) -> PyResult<()> {
        let destination = destination.native(py, self)?;
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .import_fragment(
                &destination,
                &fragment.inner,
                policy.map(|p| p.inner).unwrap_or_default(),
            )
            .map_err(|e| docx_to_pyerr(py, e))?;
        self.publish_api_candidate(py, candidate, "Document.import_fragment")
    }
    #[pyo3(signature = (other, *, at, policy=None))]
    fn insert_document(
        &mut self,
        py: Python<'_>,
        other: PyRef<'_, PyDocument>,
        at: &Bound<'_, PyAny>,
        policy: Option<PyRef<'_, PyFragmentConflictPolicy>>,
    ) -> PyResult<()> {
        if at.is_instance_of::<PyBool>() {
            return Err(PyTypeError::new_err(
                "at must be a physical body index, not bool",
            ));
        }
        let at = at.extract::<usize>()?;
        let destination = self.body_location(py, at)?;
        if other.inner.content_count() == 0 {
            return Ok(());
        }
        let start = other.body_location(py, 0)?;
        let end = oxoffice_docx::ContentLocation::end(other.body_story(py)?);
        let fragment =
            oxoffice_docx::DocumentFragment::from_range(&other.inner, &start, &end, false)
                .map_err(|e| docx_to_pyerr(py, e))?;
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .import_fragment(
                &destination,
                &fragment,
                oxoffice_docx::FragmentConflictPolicy::for_insert_document(policy.map(|p| p.inner)),
            )
            .map_err(|e| docx_to_pyerr(py, e))?;
        self.publish_api_candidate(py, candidate, "Document.insert_document")
    }
    #[getter]
    fn custom_properties(slf: Py<Self>) -> PyCustomProperties {
        PyCustomProperties { document: slf }
    }
    #[getter]
    fn app_properties(slf: Py<Self>) -> PyAppProperties {
        PyAppProperties { document: slf }
    }

    #[getter]
    fn core_properties(slf: Py<Self>, py: Python<'_>) -> PyResult<Py<PyCoreProperties>> {
        Py::new(py, PyCoreProperties { document: slf })
    }

    #[getter]
    fn update_fields_on_open(&self) -> Option<bool> {
        self.inner.update_fields_on_open()
    }

    #[setter]
    fn set_update_fields_on_open(&mut self, py: Python<'_>, value: Option<bool>) -> PyResult<()> {
        self.inner
            .set_update_fields_on_open(value)
            .map_err(|error| docx_to_pyerr(py, error))
    }

    fn image_data<'py>(
        &self,
        py: Python<'py>,
        relationship_id: &str,
    ) -> Option<Bound<'py, PyBytes>> {
        self.inner
            .image_data(relationship_id)
            .map(|bytes| PyBytes::new(py, &bytes))
    }

    fn replace_image(
        &mut self,
        py: Python<'_>,
        relationship_id: &str,
        data: &[u8],
    ) -> PyResult<()> {
        // No content moves, so live handles stay valid.
        self.inner
            .replace_image(relationship_id, data)
            .map_err(|error| docx_to_pyerr(py, error))
    }

    fn replace_image_for_story(
        &mut self,
        py: Python<'_>,
        story: PyRef<'_, PyStory>,
        relationship_id: &str,
        data: &[u8],
    ) -> PyResult<()> {
        let story = self.native_story(py, &story)?;
        self.inner
            .replace_image_for_story(&story, relationship_id, data)
            .map_err(|error| docx_to_pyerr(py, error))
    }

    #[pyo3(signature = (paragraph, heading, text, *, tooltip=None))]
    fn add_heading_hyperlink(
        slf: Py<Self>,
        py: Python<'_>,
        paragraph: PyRef<'_, PyParagraph>,
        heading: PyRef<'_, PyParagraph>,
        text: &str,
        tooltip: Option<&str>,
    ) -> PyResult<Py<crate::run::PyRun>> {
        if !paragraph.belongs_to(py, &slf) || !heading.belongs_to(py, &slf) {
            return Err(PyValueError::new_err(
                "heading hyperlink paragraphs belong to a different document",
            ));
        }
        let source = paragraph.validate(py)?;
        let destination = heading.validate(py)?;
        let mut document = slf.borrow_mut(py);
        let source = source
            .native(&document.inner)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let destination = destination
            .native(&document.inner)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let mut candidate = document.inner.clone_for_staging();
        let position = candidate
            .add_heading_hyperlink_at(&source, &destination, text, tooltip)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let run_path = candidate
            .read_story_paragraph(&position.location, |p| p.run_path(position.run_index))
            .map_err(|error| docx_to_pyerr(py, error))?
            .ok_or_else(|| PyIndexError::new_err("heading link run was not found"))?;
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "Document.add_heading_hyperlink",
            crate::document::WordPublication::Conservative,
        )?;
        let path = document
            .revisions
            .capture_prepared(
                &prepared.revision,
                smallvec![
                    PathSeg::Para(*position.location.index_path().last().unwrap_or(&0)),
                    PathSeg::Run(position.run_index)
                ],
            )
            .map_err(revision_to_pyerr)?;
        let result = Py::new(
            py,
            crate::run::PyRun::new(slf.clone_ref(py), path, run_path, Some(position.location))
                .bind_prepared(py, &document, &prepared, &candidate, None)?,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        Ok(result)
    }

    /// Frozen snapshots distinguish physical drawings of the same media.
    fn pictures<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        self.ensure_word_authority(py)?;
        let values = self
            .inner
            .pictures()
            .map_err(|error| docx_to_pyerr(py, error))?
            .into_iter()
            .map(|inner| {
                Py::new(
                    py,
                    PyPicture {
                        inner,
                        binding: self.capture_snapshot(smallvec![])?,
                    },
                )
            })
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, values)
    }

    fn set_picture_size(
        &mut self,
        py: Python<'_>,
        picture: PyRef<'_, PyPicture>,
        width: &Bound<'_, PyAny>,
        height: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        picture
            .binding
            .validate_scopes(
                &self.revisions,
                "picture",
                "Re-fetch it with document.pictures().",
            )
            .map_err(|error| crate::stale_to_pyerr(py, error))?;
        if picture.binding.revision != self.revisions.current() {
            return Err(crate::stale_to_pyerr(
                py,
                StaleElementError {
                    element_kind: "picture".into(),
                    captured_revision: picture.binding.revision,
                    current_revision: self.revisions.current(),
                    recovery_hint: "Re-fetch it with document.pictures().".into(),
                },
            ));
        }
        let width = crate::required_length(
            width,
            "Document.set_picture_size(width)",
            crate::LengthUnit::Emu,
        )?;
        let height = crate::required_length(
            height,
            "Document.set_picture_size(height)",
            crate::LengthUnit::Emu,
        )?;
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .set_picture_size_at(
                &picture.inner,
                oxoffice_docx::Length::emu(width),
                oxoffice_docx::Length::emu(height),
            )
            .map_err(|error| docx_to_pyerr(py, error))?;
        if (width, height) == (picture.inner.width, picture.inner.height) {
            return Ok(());
        }
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.set_picture_size",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    fn set_picture_sizes_for_relationship(
        &mut self,
        py: Python<'_>,
        relationship_id: &str,
        width: &Bound<'_, PyAny>,
        height: &Bound<'_, PyAny>,
    ) -> PyResult<usize> {
        let width = crate::required_length(
            width,
            "Document.set_picture_sizes_for_relationship(width)",
            crate::LengthUnit::Emu,
        )?;
        let height = crate::required_length(
            height,
            "Document.set_picture_sizes_for_relationship(height)",
            crate::LengthUnit::Emu,
        )?;
        // No content moves, so live handles stay valid.
        py.detach(|| {
            self.inner.set_picture_size(
                relationship_id,
                oxoffice_docx::Length::emu(width),
                oxoffice_docx::Length::emu(height),
            )
        })
        .map_err(|error| docx_to_pyerr(py, error))
    }

    /// Split runs so that `span`, from `Paragraph.find_text`, covers whole
    /// runs, and return the indexes `(first, last)` of those runs, the last
    /// excluded, as `Paragraph.runs` counts them. The text does not change,
    /// and a tab, a break or another text-free item right before or right
    /// after the span stays out of those runs, so formatting them or setting
    /// their text leaves it alone.
    /// A paragraph of the body, a table cell, a header, a footer, a note or a
    /// comment is accepted. A change stales the document's earlier handles,
    /// the paragraphs of other spans included: find the next span after.
    fn isolate_span(
        slf: Py<Self>,
        py: Python<'_>,
        span: PyRef<'_, crate::paragraph::PyTextSpan>,
    ) -> PyResult<(usize, usize)> {
        let paragraph = span.paragraph.bind(py).borrow();
        if !paragraph.belongs_to(py, &slf) {
            return Err(PyValueError::new_err(
                "span belongs to a different document",
            ));
        }
        let location = paragraph.validate(py)?;
        let mut document = slf.borrow_mut(py);
        let run_count = |document: &oxoffice_docx::Document| {
            location.read(document, |paragraph| paragraph.run_count())
        };
        let before = run_count(&document.inner).map_err(|error| docx_to_pyerr(py, error))?;
        let mut candidate = document.inner.clone_for_staging();
        let runs = location
            .edit(&mut candidate, |paragraph| {
                paragraph.isolate_span(oxoffice_docx::TextSpan {
                    start: span.start,
                    end: span.end,
                })
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        if run_count(&candidate).map_err(|error| docx_to_pyerr(py, error))? != before {
            let prepared = document.prepare_word_revision(
                py,
                &candidate,
                "Document.isolate_span",
                crate::document::WordPublication::Conservative,
            )?;
            document.commit_word_revision(prepared)?;
            document.install_inner(candidate);
        }
        Ok((runs.start, runs.end))
    }

    fn split_run(
        slf: Py<Self>,
        py: Python<'_>,
        body_index: &Bound<'_, PyAny>,
        run_index: usize,
        character_offset: usize,
    ) -> PyResult<usize> {
        let Ok(paragraph) = body_index.cast::<PyParagraph>() else {
            // An integer is the direct body index, as in `RunPosition`.
            let body_index = body_index.extract::<usize>().map_err(|error| {
                if error.is_instance_of::<PyTypeError>(py) {
                    PyTypeError::new_err("body_index must be an int or a Paragraph handle")
                } else {
                    error
                }
            })?;
            return slf.borrow_mut(py).split_run_in_place_or_staged(
                py,
                body_index,
                run_index,
                character_offset,
            );
        };
        let paragraph = paragraph.borrow();
        if !paragraph.belongs_to(py, &slf) {
            return Err(PyValueError::new_err(
                "paragraph handle belongs to a different document",
            ));
        }
        // A cell handle counts paragraphs inside cell content controls, which
        // `Cell::paragraph_mut` does not, so it could name another paragraph.
        let ParagraphLocation::Body(paragraph_index) = paragraph.validate(py)? else {
            return Err(PyValueError::new_err(
                "split_run does not accept a table cell paragraph handle",
            ));
        };
        let mut document = slf.borrow_mut(py);
        if let Some(body_index) = document.inner.content_index_of_paragraph(paragraph_index) {
            return document.split_run_in_place_or_staged(
                py,
                body_index,
                run_index,
                character_offset,
            );
        }
        // A paragraph inside a block content control has no direct body index.
        let run_count = |document: &oxoffice_docx::Document| {
            document
                .paragraph(paragraph_index)
                .map(|paragraph| paragraph.run_count())
        };
        let before = run_count(&document.inner);
        let mut candidate = document.inner.clone_for_staging();
        let boundary = candidate
            .paragraph_mut(paragraph_index)
            .ok_or_else(|| PyIndexError::new_err("paragraph index out of range"))?
            .split_run(run_index, character_offset)
            .map_err(|error| docx_to_pyerr(py, error))?;
        if run_count(&candidate) != before {
            let publication = document
                .paragraph_route(&ParagraphLocation::Body(paragraph_index), None)
                .map_or(
                    WordPublication::Conservative,
                    WordPublication::ParagraphEdit,
                );
            document.publish_block_edit(py, candidate, "Document.split_run", publication)?;
        }
        Ok(boundary)
    }

    #[pyo3(signature = (*, fonts = None, font_dir = None, font_dirs = None, system_fonts = false, bundled_fonts_only = false, revision_view = "accepted", revision_style = "plain", fillable_forms = false))]
    #[allow(clippy::too_many_arguments)]
    fn to_pdf<'py>(
        &self,
        py: Python<'py>,
        fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
        font_dir: Option<PathBuf>,
        font_dirs: Option<Bound<'py, PyAny>>,
        system_fonts: bool,
        bundled_fonts_only: bool,
        revision_view: &str,
        revision_style: &str,
        fillable_forms: bool,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let mut options = parse_render_options(revision_view, revision_style)?;
        options = with_font_switches(options, system_fonts, bundled_fonts_only)?;
        options.fillable_forms = check_fillable_forms(fillable_forms, revision_view)?;
        let caller_fonts = collect_caller_fonts(py, fonts, font_dir, font_dirs)?;
        let font_refs = caller_fonts.as_ref().map(|files| {
            files
                .iter()
                .map(|font| (font.family.as_str(), font.data.as_slice()))
                .collect::<Vec<_>>()
        });
        py.detach(|| {
            self.inner
                .to_pdf_with_diagnostics(font_refs.as_deref().unwrap_or(&[]), options)
                .map(|(bytes, _)| bytes)
        })
        .map(|bytes| PyBytes::new(py, &bytes))
        .map_err(|error| docx_to_pyerr(py, error))
    }

    #[pyo3(signature = (*, fonts = None, font_dir = None, font_dirs = None, system_fonts = false, bundled_fonts_only = false, revision_view = "accepted", revision_style = "plain", fillable_forms = false))]
    #[allow(clippy::too_many_arguments)]
    fn to_pdf_with_diagnostics<'py>(
        &self,
        py: Python<'py>,
        fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
        font_dir: Option<PathBuf>,
        font_dirs: Option<Bound<'py, PyAny>>,
        system_fonts: bool,
        bundled_fonts_only: bool,
        revision_view: &str,
        revision_style: &str,
        fillable_forms: bool,
    ) -> PyResult<(Bound<'py, PyBytes>, Bound<'py, PyTuple>)> {
        let mut options = parse_render_options(revision_view, revision_style)?;
        options = with_font_switches(options, system_fonts, bundled_fonts_only)?;
        options.fillable_forms = check_fillable_forms(fillable_forms, revision_view)?;
        let caller_fonts = collect_caller_fonts(py, fonts, font_dir, font_dirs)?;
        let font_refs = caller_fonts.as_ref().map(|files| {
            files
                .iter()
                .map(|font| (font.family.as_str(), font.data.as_slice()))
                .collect::<Vec<_>>()
        });
        let (payload, diagnostics) = py
            .detach(|| {
                self.inner
                    .to_pdf_with_diagnostics(font_refs.as_deref().unwrap_or(&[]), options)
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        let messages = PyTuple::new(py, diagnostics.iter().map(|d| d.message.as_str()))?;
        Ok((PyBytes::new(py, &payload), messages))
    }
    #[pyo3(signature = (profile = "pdfa-2b", *, fillable_forms = false))]
    fn to_pdfa_deterministic<'py>(
        &self,
        py: Python<'py>,
        profile: &str,
        fillable_forms: bool,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let profile = match profile {
            "pdfa-2b" => oxoffice_docx::PdfConformance::PdfA2b,
            "pdfa-3b" => oxoffice_docx::PdfConformance::PdfA3b,
            _ => {
                return Err(PyValueError::new_err(format!(
                    "profile must be pdfa-2b or pdfa-3b, not {profile:?}"
                )));
            }
        };
        let options = oxoffice_docx::RenderOptions::default().with_fillable_forms(fillable_forms);
        py.detach(|| {
            self.inner
                .to_pdfa_deterministic_with_options(profile, options)
        })
        .map(|bytes| PyBytes::new(py, &bytes))
        .map_err(|error| docx_to_pyerr(py, error))
    }

    #[pyo3(signature = (page_index, dpi = 150.0, *, fonts = None, font_dir = None, font_dirs = None, system_fonts = false, bundled_fonts_only = false, revision_view = "accepted", revision_style = "plain"))]
    #[allow(clippy::too_many_arguments)]
    fn render_page_to_png<'py>(
        &self,
        py: Python<'py>,
        page_index: usize,
        dpi: f64,
        fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
        font_dir: Option<PathBuf>,
        font_dirs: Option<Bound<'py, PyAny>>,
        system_fonts: bool,
        bundled_fonts_only: bool,
        revision_view: &str,
        revision_style: &str,
    ) -> PyResult<Option<Bound<'py, PyBytes>>> {
        let mut options = parse_render_options(revision_view, revision_style)?;
        options = with_font_switches(options, system_fonts, bundled_fonts_only)?;
        let caller_fonts = collect_caller_fonts(py, fonts, font_dir, font_dirs)?;
        let font_refs = caller_fonts.as_ref().map(|files| {
            files
                .iter()
                .map(|font| (font.family.as_str(), font.data.as_slice()))
                .collect::<Vec<_>>()
        });
        py.detach(|| {
            self.inner
                .render_page_to_png_with_diagnostics(
                    page_index,
                    dpi,
                    font_refs.as_deref().unwrap_or(&[]),
                    options,
                )
                .map(|(bytes, _)| bytes)
        })
        .map(|bytes| bytes.map(|bytes| PyBytes::new(py, &bytes)))
        .map_err(|error| docx_to_pyerr(py, error))
    }

    #[pyo3(signature = (page_index, dpi = 150.0, *, fonts = None, font_dir = None, font_dirs = None, system_fonts = false, bundled_fonts_only = false, revision_view = "accepted", revision_style = "plain"))]
    #[allow(clippy::too_many_arguments)]
    fn render_page_to_png_with_diagnostics<'py>(
        &self,
        py: Python<'py>,
        page_index: usize,
        dpi: f64,
        fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
        font_dir: Option<PathBuf>,
        font_dirs: Option<Bound<'py, PyAny>>,
        system_fonts: bool,
        bundled_fonts_only: bool,
        revision_view: &str,
        revision_style: &str,
    ) -> PyResult<(Option<Bound<'py, PyBytes>>, Bound<'py, PyTuple>)> {
        let mut options = parse_render_options(revision_view, revision_style)?;
        options = with_font_switches(options, system_fonts, bundled_fonts_only)?;
        let caller_fonts = collect_caller_fonts(py, fonts, font_dir, font_dirs)?;
        let font_refs = caller_fonts.as_ref().map(|files| {
            files
                .iter()
                .map(|font| (font.family.as_str(), font.data.as_slice()))
                .collect::<Vec<_>>()
        });
        let (payload, diagnostics) = py
            .detach(|| {
                self.inner.render_page_to_png_with_diagnostics(
                    page_index,
                    dpi,
                    font_refs.as_deref().unwrap_or(&[]),
                    options,
                )
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        let messages = PyTuple::new(py, diagnostics.iter().map(|d| d.message.as_str()))?;
        Ok((payload.map(|bytes| PyBytes::new(py, &bytes)), messages))
    }
    #[pyo3(signature = (page_index, *, fonts = None, font_dir = None, font_dirs = None, system_fonts = false, bundled_fonts_only = false))]
    #[allow(clippy::too_many_arguments)]
    fn render_page_to_svg<'py>(
        &self,
        py: Python<'py>,
        page_index: usize,
        fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
        font_dir: Option<PathBuf>,
        font_dirs: Option<Bound<'py, PyAny>>,
        system_fonts: bool,
        bundled_fonts_only: bool,
    ) -> PyResult<Option<PySvgRenderResult>> {
        let options = with_font_switches(
            oxoffice_docx::RenderOptions::default(),
            system_fonts,
            bundled_fonts_only,
        )?;
        let caller_fonts = collect_caller_fonts(py, fonts, font_dir, font_dirs)?;
        let font_refs = caller_fonts.as_ref().map(|files| {
            files
                .iter()
                .map(|font| (font.family.as_str(), font.data.as_slice()))
                .collect::<Vec<_>>()
        });
        let rendered = py
            .detach(|| {
                self.inner
                    .layout_for_render(font_refs.as_deref().unwrap_or(&[]), options)
                    .map(|layout| oxoffice_docx::render_page(&layout.layout, page_index))
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        Ok(rendered.map(|result| PySvgRenderResult {
            svg: result.svg,
            diagnostics: result
                .diagnostics
                .into_iter()
                .map(|diagnostic| PySvgDiagnostic {
                    path: diagnostic.path,
                    message: diagnostic.message,
                })
                .collect(),
        }))
    }

    #[pyo3(signature = (dpi = 150.0, *, fonts = None, font_dir = None, font_dirs = None, system_fonts = false, bundled_fonts_only = false, revision_view = "accepted", revision_style = "plain"))]
    #[allow(clippy::too_many_arguments)] // Explicit public rendering keyword operands.
    fn render_all_pages<'py>(
        &self,
        py: Python<'py>,
        dpi: f64,
        fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
        font_dir: Option<PathBuf>,
        font_dirs: Option<Bound<'py, PyAny>>,
        system_fonts: bool,
        bundled_fonts_only: bool,
        revision_view: &str,
        revision_style: &str,
    ) -> PyResult<Bound<'py, PyList>> {
        let mut options = parse_render_options(revision_view, revision_style)?;
        options = with_font_switches(options, system_fonts, bundled_fonts_only)?;
        let caller_fonts = collect_caller_fonts(py, fonts, font_dir, font_dirs)?;
        let font_refs = caller_fonts.as_ref().map(|files| {
            files
                .iter()
                .map(|font| (font.family.as_str(), font.data.as_slice()))
                .collect::<Vec<_>>()
        });
        let pages = py
            .detach(|| {
                self.inner
                    .render_all_pages_with_diagnostics(
                        dpi,
                        font_refs.as_deref().unwrap_or(&[]),
                        options,
                    )
                    .map(|(pages, _)| pages)
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        PyList::new(py, pages.iter().map(|page| PyBytes::new(py, page)))
    }

    #[pyo3(signature = (dpi = 150.0, *, fonts = None, font_dir = None, font_dirs = None, system_fonts = false, bundled_fonts_only = false, revision_view = "accepted", revision_style = "plain"))]
    #[allow(clippy::too_many_arguments)] // Explicit public rendering keyword operands.
    fn render_all_pages_with_diagnostics<'py>(
        &self,
        py: Python<'py>,
        dpi: f64,
        fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
        font_dir: Option<PathBuf>,
        font_dirs: Option<Bound<'py, PyAny>>,
        system_fonts: bool,
        bundled_fonts_only: bool,
        revision_view: &str,
        revision_style: &str,
    ) -> PyResult<(Bound<'py, PyList>, Bound<'py, PyTuple>)> {
        let mut options = parse_render_options(revision_view, revision_style)?;
        options = with_font_switches(options, system_fonts, bundled_fonts_only)?;
        let caller_fonts = collect_caller_fonts(py, fonts, font_dir, font_dirs)?;
        let font_refs = caller_fonts.as_ref().map(|files| {
            files
                .iter()
                .map(|font| (font.family.as_str(), font.data.as_slice()))
                .collect::<Vec<_>>()
        });
        let (payload, diagnostics) = py
            .detach(|| {
                self.inner.render_all_pages_with_diagnostics(
                    dpi,
                    font_refs.as_deref().unwrap_or(&[]),
                    options,
                )
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        let messages = PyTuple::new(py, diagnostics.iter().map(|d| d.message.as_str()))?;
        Ok((
            PyList::new(py, payload.iter().map(|bytes| PyBytes::new(py, bytes)))?,
            messages,
        ))
    }
    #[pyo3(signature = (*, dpi = 150.0, format = "png", quality = 90, transparent = false, pages = None, fonts = None, font_dir = None, font_dirs = None, system_fonts = false, bundled_fonts_only = false, revision_view = "accepted", revision_style = "plain"))]
    #[allow(clippy::too_many_arguments)]
    fn render_pages<'py>(
        &self,
        py: Python<'py>,
        dpi: f64,
        format: &str,
        quality: u8,
        transparent: bool,
        pages: Option<Vec<usize>>,
        fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
        font_dir: Option<PathBuf>,
        font_dirs: Option<Bound<'py, PyAny>>,
        system_fonts: bool,
        bundled_fonts_only: bool,
        revision_view: &str,
        revision_style: &str,
    ) -> PyResult<Py<PyAny>> {
        // The twin selects page indices from the exact layout it paints.
        // Reuse that path so caller fonts cannot leave stale page bounds.
        self.render_pages_with_diagnostics(
            py,
            dpi,
            format,
            quality,
            transparent,
            pages,
            fonts,
            font_dir,
            font_dirs,
            system_fonts,
            bundled_fonts_only,
            revision_view,
            revision_style,
        )
        .map(|(payload, _)| payload)
    }

    #[pyo3(signature = (*, dpi = 150.0, format = "png", quality = 90, transparent = false, pages = None, fonts = None, font_dir = None, font_dirs = None, system_fonts = false, bundled_fonts_only = false, revision_view = "accepted", revision_style = "plain"))]
    #[allow(clippy::too_many_arguments)]
    fn render_pages_with_diagnostics<'py>(
        &self,
        py: Python<'py>,
        dpi: f64,
        format: &str,
        quality: u8,
        transparent: bool,
        pages: Option<Vec<usize>>,
        fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
        font_dir: Option<PathBuf>,
        font_dirs: Option<Bound<'py, PyAny>>,
        system_fonts: bool,
        bundled_fonts_only: bool,
        revision_view: &str,
        revision_style: &str,
    ) -> PyResult<(Py<PyAny>, Bound<'py, PyTuple>)> {
        let mut options = parse_render_options(revision_view, revision_style)?;
        options = with_font_switches(options, system_fonts, bundled_fonts_only)?;
        let caller_fonts = collect_caller_fonts(py, fonts, font_dir, font_dirs)?;
        let font_refs = caller_fonts.as_ref().map(|files| {
            files
                .iter()
                .map(|font| (font.family.as_str(), font.data.as_slice()))
                .collect::<Vec<_>>()
        });
        let (rendered, diagnostics) = py
            .detach(|| {
                let format = parse_raster_format(format, quality, transparent)?;
                let raster = oxoffice_docx::RasterOptions { dpi, format };
                self.inner.render_pages_with_diagnostics(
                    pages.as_deref(),
                    raster,
                    font_refs.as_deref().unwrap_or(&[]),
                    options,
                )
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        let payload = match rendered {
            oxoffice_docx::RasterOutput::SeparatePages(pages) => {
                PyList::new(py, pages.iter().map(|bytes| PyBytes::new(py, bytes)))?
                    .into_any()
                    .unbind()
            }
            oxoffice_docx::RasterOutput::MultiPageTiff(bytes) => {
                PyBytes::new(py, &bytes).into_any().unbind()
            }
        };
        let messages = PyTuple::new(py, diagnostics.iter().map(|d| d.message.as_str()))?;
        Ok((payload, messages))
    }
    #[pyo3(signature = (
        edited,
        author,
        timestamp,
        *,
        granularity = "run",
        ignore_formatting = false,
        ignore_whitespace = false,
        ignore_fields = false,
        ignore_comments = false,
        ignored_stories = None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn compare<'py>(
        &mut self,
        edited: &PyDocument,
        author: &str,
        timestamp: &str,
        py: Python<'py>,
        granularity: &str,
        ignore_formatting: bool,
        ignore_whitespace: bool,
        ignore_fields: bool,
        ignore_comments: bool,
        ignored_stories: Option<Vec<String>>,
    ) -> PyResult<Bound<'py, PyTuple>> {
        let mut candidate = self.inner.clone_for_staging();
        let (diagnostics, changed) = py
            .detach(|| {
                let options = oxoffice_docx::ComparisonOptions {
                    granularity: parse_comparison_granularity(granularity)?,
                    ignore_formatting,
                    ignore_whitespace,
                    ignore_fields,
                    ignore_comments,
                    ignored_stories: ignored_stories
                        .iter()
                        .flatten()
                        .map(|name| parse_comparison_story(name))
                        .collect::<oxoffice_docx::Result<_>>()?,
                };
                let before = candidate.to_bytes()?;
                let diagnostics =
                    candidate.compare_with_options(&edited.inner, author, timestamp, &options)?;
                let changed = candidate.to_bytes()? != before;
                Ok::<_, oxoffice_docx::Error>((diagnostics, changed))
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        let result = PyTuple::new(
            py,
            diagnostics.into_iter().map(|item| PyComparisonDiagnostic {
                location: item.location,
                message: item.message,
            }),
        )?;
        if changed {
            let prepared = self.prepare_word_revision(
                py,
                &candidate,
                "Document.compare",
                crate::document::WordPublication::Conservative,
            )?;
            self.commit_word_revision(prepared)?;
            self.install_inner(candidate);
        }
        Ok(result)
    }

    #[getter]
    fn comments<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyTuple>> {
        let py = slf.py();
        let owner = std::sync::Arc::new(slf.clone().unbind());
        let mut this = slf.borrow_mut();
        this.ensure_word_authority(py)?;
        let mut anchors = this
            .inner
            .comment_anchor_snapshot_results()
            .map_err(|error| docx_to_pyerr(py, error))?;
        let mut items = this
            .inner
            .standalone_story_item_snapshots()
            .map_err(|error| docx_to_pyerr(py, error))?
            .into_iter()
            .map(|item| (item.location().clone(), item))
            .collect::<HashMap<_, _>>();
        let records = this
            .inner
            .comments()
            .into_iter()
            .map(|comment| {
                let snapshot = anchors.remove(&comment.id()).ok_or_else(|| {
                    PyValueError::new_err(
                        "comment snapshot is missing its checked ownership record",
                    )
                })?;
                // A comment whose anchor cannot be read or placed keeps its
                // record. The reason is that comment's issue.
                let (anchor, anchor_text, anchor_issue) = match snapshot
                    .map_err(|error| error.to_string())
                    .and_then(|(range, text)| {
                        range
                            .map(|range| -> PyResult<PyStoryRunRange> {
                                Ok(PyStoryRunRange {
                                    start: this.comment_story_position(
                                        py,
                                        &range.start,
                                        &mut items,
                                        &owner,
                                    )?,
                                    end: this.comment_story_position(
                                        py, &range.end, &mut items, &owner,
                                    )?,
                                })
                            })
                            .transpose()
                            .map(|anchor| (anchor, text))
                            .map_err(|error| error.to_string())
                    }) {
                    Ok((anchor, text)) => (anchor, text, None),
                    Err(issue) => (None, None, Some(issue)),
                };
                Ok(PyComment {
                    id: comment.id(),
                    author: comment.author().map(str::to_owned),
                    initials: comment.initials().map(str::to_owned),
                    date: comment.date().map(str::to_owned),
                    text: comment.text(),
                    parent_id: comment.parent_id(),
                    resolved: comment.resolved(),
                    anchor_text,
                    anchor,
                    anchor_issue,
                })
            })
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, records)
    }

    #[getter]
    fn sections<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let sections = self
            .inner
            .sections()
            .map(section_snapshot)
            .collect::<Vec<_>>();
        PyTuple::new(py, sections)
    }

    #[pyo3(signature = (
        index,
        *,
        orientation = None,
        page_width = None,
        page_height = None,
        margin_top = None,
        margin_right = None,
        margin_bottom = None,
        margin_left = None,
        gutter = None,
        column_count = None,
        column_spacing = None,
        page_number_start = None,
        header_distance = None,
        footer_distance = None,
        different_first_page = None,
        break_type = None,
    ))]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn update_section(
        &mut self,
        py: Python<'_>,
        index: usize,
        orientation: Option<&str>,
        page_width: Option<Bound<'_, PyAny>>,
        page_height: Option<Bound<'_, PyAny>>,
        margin_top: Option<Bound<'_, PyAny>>,
        margin_right: Option<Bound<'_, PyAny>>,
        margin_bottom: Option<Bound<'_, PyAny>>,
        margin_left: Option<Bound<'_, PyAny>>,
        gutter: Option<Bound<'_, PyAny>>,
        column_count: Option<u32>,
        column_spacing: Option<Bound<'_, PyAny>>,
        page_number_start: Option<u32>,
        header_distance: Option<Bound<'_, PyAny>>,
        footer_distance: Option<Bound<'_, PyAny>>,
        different_first_page: Option<bool>,
        break_type: Option<&str>,
    ) -> PyResult<PySection> {
        let length = |value: Option<Bound<'_, PyAny>>, name: &str| {
            crate::length_argument(
                value.as_ref(),
                &format!("Document.update_section({name})"),
                crate::LengthUnit::Twips,
            )
        };
        let page_width = length(page_width, "page_width")?;
        let page_height = length(page_height, "page_height")?;
        let margin_top = length(margin_top, "margin_top")?;
        let margin_right = length(margin_right, "margin_right")?;
        let margin_bottom = length(margin_bottom, "margin_bottom")?;
        let margin_left = length(margin_left, "margin_left")?;
        let gutter = length(gutter, "gutter")?;
        let column_spacing = length(column_spacing, "column_spacing")?;
        let header_distance = length(header_distance, "header_distance")?;
        let footer_distance = length(footer_distance, "footer_distance")?;
        let orientation = orientation
            .map(|value| {
                ST_PageOrientation::from_str(value)
                    .map_err(|_| PyValueError::new_err("orientation must be portrait or landscape"))
            })
            .transpose()?;
        let break_type = break_type
            .map(|value| {
                ST_SectionType::from_str(value).map_err(|_| {
                    PyValueError::new_err(
                        "break type must be nextPage, continuous, evenPage, oddPage or nextColumn",
                    )
                })
            })
            .transpose()?;
        let mut section = self
            .inner
            .section_mut(index)
            .ok_or_else(|| PyIndexError::new_err("section index out of range"))?;

        let current = section.properties();
        let defaults = oxoffice_docx_oxml::CT_SectPr::default_letter();
        let page_size = if page_width.is_some() || page_height.is_some() {
            Some((
                given_or_current_length(
                    page_width,
                    current.page_width,
                    defaults.page_width.unwrap(),
                ),
                given_or_current_length(
                    page_height,
                    current.page_height,
                    defaults.page_height.unwrap(),
                ),
            ))
        } else {
            None
        };
        let margins = if [margin_top, margin_right, margin_bottom, margin_left]
            .iter()
            .any(Option::is_some)
        {
            Some((
                given_or_current_length(
                    margin_top,
                    current.margin_top,
                    defaults.margin_top.unwrap(),
                ),
                given_or_current_length(
                    margin_right,
                    current.margin_right,
                    defaults.margin_right.unwrap(),
                ),
                given_or_current_length(
                    margin_bottom,
                    current.margin_bottom,
                    defaults.margin_bottom.unwrap(),
                ),
                given_or_current_length(
                    margin_left,
                    current.margin_left,
                    defaults.margin_left.unwrap(),
                ),
            ))
        } else {
            None
        };
        let columns = if column_count.is_some() || column_spacing.is_some() {
            let current = section.columns();
            if current.is_none()
                && section.properties().columns.is_some()
                && (column_count.is_none() || column_spacing.is_none())
            {
                return Err(PyValueError::new_err(
                    "unequal-width columns require both column_count and column_spacing",
                ));
            }
            let count = column_count
                .or(current.map(|(count, _)| count))
                .unwrap_or(1);
            let spacing = given_or_current_length(
                column_spacing,
                current.map(|(_, spacing)| spacing.as_twips()),
                oxoffice_docx::Twips(720),
            );
            Some((count, spacing))
        } else {
            None
        };
        let distances = if header_distance.is_some() || footer_distance.is_some() {
            Some((
                given_or_current_length(
                    header_distance,
                    current.header_distance,
                    defaults.header_distance.unwrap(),
                ),
                given_or_current_length(
                    footer_distance,
                    current.footer_distance,
                    defaults.footer_distance.unwrap(),
                ),
            ))
        } else {
            None
        };

        // The native setters check one value at a time, so restore the
        // section if a later value is rejected after an earlier one applied.
        let saved = section.properties().clone();
        let applied = (|| -> oxoffice_docx::Result<()> {
            if let Some((width, height)) = page_size {
                section.set_page_size(width, height)?;
            }
            if let Some(orientation) = orientation {
                section.set_orientation(orientation);
            }
            if let Some((top, right, bottom, left)) = margins {
                section.set_margins(top, right, bottom, left)?;
            }
            if let Some(gutter) = gutter {
                section.set_gutter(oxoffice_docx::Length::emu(gutter))?;
            }
            if let Some((count, spacing)) = columns {
                section.set_columns(count, spacing)?;
            }
            if let Some(start) = page_number_start {
                section.set_page_number_start(start)?;
            }
            if let Some((header, footer)) = distances {
                section.set_header_footer_distance(header, footer)?;
            }
            if let Some(enabled) = different_first_page {
                section.set_different_first_page(enabled);
            }
            if let Some(break_type) = break_type {
                section.set_break_type(break_type);
            }
            Ok(())
        })();
        if let Err(error) = applied {
            *section.properties_mut() = saved;
            return Err(docx_to_pyerr(py, error));
        }
        Ok(self
            .inner
            .sections()
            .nth(index)
            .map(section_snapshot)
            .expect("the updated section exists"))
    }

    #[pyo3(signature = (reference, text = ""))]
    fn add_footnote(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        reference: &Bound<'_, PyAny>,
        text: &str,
    ) -> PyResult<Py<PyNote>> {
        Self::create_note(slf, py, reference, text, false)
    }
    #[pyo3(signature = (reference, text = ""))]
    fn add_endnote(
        slf: &Bound<'_, Self>,
        py: Python<'_>,
        reference: &Bound<'_, PyAny>,
        text: &str,
    ) -> PyResult<Py<PyNote>> {
        Self::create_note(slf, py, reference, text, true)
    }

    #[pyo3(signature = (family, *, format=None, start=None, restart="continuous", placement=None))]
    fn set_note_policy(
        &mut self,
        py: Python<'_>,
        family: &str,
        format: Option<&str>,
        start: Option<&Bound<'_, PyAny>>,
        restart: &str,
        placement: Option<&str>,
    ) -> PyResult<()> {
        let (family, policy) = checked_note_policy(family, format, start, restart, placement)?;
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .set_note_policy(family, policy)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.set_note_policy",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    fn set_page_color(&mut self, py: Python<'_>, color: &str) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .set_page_color(color)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.set_page_color",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    /// Author the existing native VML compatibility watermark.
    fn set_text_watermark(&mut self, py: Python<'_>, text: &str) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .set_text_watermark(text)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.set_text_watermark",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    /// Author the existing native VML image watermark, with checked EMU dimensions.
    fn set_image_watermark(
        &mut self,
        py: Python<'_>,
        data: &[u8],
        filename: &str,
        width: &Bound<'_, PyAny>,
        height: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let width = crate::required_length(
            width,
            "Document.set_image_watermark(width)",
            crate::LengthUnit::Emu,
        )?;
        let height = crate::required_length(
            height,
            "Document.set_image_watermark(height)",
            crate::LengthUnit::Emu,
        )?;
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .set_image_watermark(
                data,
                filename,
                oxoffice_docx::Length::emu(width),
                oxoffice_docx::Length::emu(height),
            )
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.set_image_watermark",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    fn section(slf: &Bound<'_, Self>, index: usize) -> PyResult<PySectionHandle> {
        let mut document = slf.borrow_mut();
        if index >= document.inner.section_count() {
            return Err(PyIndexError::new_err("section index out of range"));
        }
        document.ensure_word_authority(slf.py())?;
        let (path, owner) = document.capture_owner(
            &WordOwnerLocation::Section(index),
            smallvec![],
            WordView::Owner,
            None,
        )?;
        Ok(PySectionHandle {
            document: slf.clone().unbind(),
            index,
            path,
            owner,
        })
    }
    #[getter]
    fn settings(slf: &Bound<'_, Self>) -> PySettings {
        PySettings {
            document: slf.clone().unbind(),
        }
    }

    fn insert_section(&mut self, py: Python<'_>, index: usize) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        if index > candidate.section_count() {
            return Err(PyIndexError::new_err("section index out of range"));
        }
        candidate
            .insert_section(index)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.insert_section",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    fn remove_section(&mut self, py: Python<'_>, index: usize) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        if index >= candidate.section_count() {
            return Err(PyIndexError::new_err("section index out of range"));
        }
        candidate
            .remove_section(index)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.remove_section",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    #[getter]
    fn styles<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let snapshots = PyTuple::new(py, self.inner.styles().into_iter().map(style_snapshot))?;
        py.import("oxoffice.docx")?
            .getattr("_StyleSnapshots")?
            .call1((snapshots,))?
            .cast_into::<PyTuple>()
            .map_err(Into::into)
    }

    // Create a style as python-docx's `styles.add_style` does, deriving the
    // ID from the name unless one is given, with the formatting given as
    // keywords. Styles change no content, so handles stay valid.
    #[pyo3(signature = (
        name,
        style_type = "paragraph",
        *,
        style_id = None,
        based_on = None,
        next_style = None,
        font_name = None,
        font_size = None,
        bold = None,
        italic = None,
        color = None,
        space_before = None,
        space_after = None,
        left_indent = None,
        right_indent = None,
        first_line_indent = None,
        alignment = None,
        line_spacing = None,
        underline = None,
        keep_with_next = None,
        keep_together = None,
        page_break_before = None,
        borders = None,
        shading = None,
        tab_stops = None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn add_style(
        &mut self,
        py: Python<'_>,
        name: &str,
        style_type: &str,
        style_id: Option<&str>,
        based_on: Option<&str>,
        next_style: Option<&str>,
        font_name: Option<String>,
        font_size: Option<&Bound<'_, PyAny>>,
        bold: Option<bool>,
        italic: Option<bool>,
        color: Option<&Bound<'_, PyAny>>,
        space_before: Option<&Bound<'_, PyAny>>,
        space_after: Option<&Bound<'_, PyAny>>,
        left_indent: Option<&Bound<'_, PyAny>>,
        right_indent: Option<&Bound<'_, PyAny>>,
        first_line_indent: Option<&Bound<'_, PyAny>>,
        alignment: Option<i32>,
        line_spacing: Option<&Bound<'_, PyAny>>,
        underline: Option<&Bound<'_, PyAny>>,
        keep_with_next: Option<bool>,
        keep_together: Option<bool>,
        page_break_before: Option<bool>,
        borders: Option<&Bound<'_, PyAny>>,
        shading: Option<&Bound<'_, PyAny>>,
        tab_stops: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyStyle> {
        let color = color
            .filter(|value| !value.is_none())
            .map(|value| oxoffice_py_support::rgb_channels(value, "Document.add_style(color)"))
            .transpose()?;
        let length = |value, name: &str, unit| {
            crate::length_argument(value, &format!("Document.add_style({name})"), unit)
        };
        let font_size = length(font_size, "font_size", crate::LengthUnit::HalfPoints)?;
        let space_before = length(space_before, "space_before", crate::LengthUnit::Twips)?;
        let space_after = length(space_after, "space_after", crate::LengthUnit::Twips)?;
        let left_indent = length(left_indent, "left_indent", crate::LengthUnit::Twips)?;
        let right_indent = length(right_indent, "right_indent", crate::LengthUnit::Twips)?;
        let first_line_indent = length(
            first_line_indent,
            "first_line_indent",
            crate::LengthUnit::Twips,
        )?;
        type NewStyle = fn(&str, &str) -> oxoffice_docx::StyleBuilder;
        let (native_type, new_style): (oxoffice_docx::StyleType, NewStyle) = match style_type {
            "paragraph" => (
                oxoffice_docx::StyleType::Paragraph,
                oxoffice_docx::StyleBuilder::paragraph,
            ),
            "character" => (
                oxoffice_docx::StyleType::Character,
                oxoffice_docx::StyleBuilder::character,
            ),
            "table" => (
                oxoffice_docx::StyleType::Table,
                oxoffice_docx::StyleBuilder::table,
            ),
            _ => {
                return Err(PyValueError::new_err(
                    "style_type must be 'paragraph', 'character' or 'table'",
                ));
            }
        };
        let style_id =
            style_id.map_or_else(|| style_id_from_name(&self.inner, name), str::to_owned);
        if name.trim().is_empty() || style_id.trim().is_empty() {
            return Err(PyValueError::new_err(
                "a style name and style ID cannot be blank",
            ));
        }
        if self.inner.style(&style_id).is_some() {
            return Err(PyValueError::new_err(format!(
                "a style with the ID '{style_id}' already exists"
            )));
        }
        let lowered = name.to_lowercase();
        if self.inner.styles().iter().any(|style| {
            style
                .name()
                .is_some_and(|name| name.to_lowercase() == lowered)
        }) {
            return Err(PyValueError::new_err(format!(
                "a style named '{name}' already exists"
            )));
        }
        let formatting = StyleFormatting {
            method: "Document.add_style",
            font_name,
            font_size,
            bold,
            italic,
            color,
            space_before,
            space_after,
            left_indent,
            right_indent,
            first_line_indent,
            alignment,
            line_spacing: line_spacing
                .filter(|v| !v.is_none())
                .map(|v| Some(v.clone().unbind())),
            underline: underline
                .filter(|v| !v.is_none())
                .map(|v| v.clone().unbind()),
            keep_with_next,
            keep_together,
            page_break_before,
            borders: borders.filter(|v| !v.is_none()).map(|v| v.clone().unbind()),
            shading: shading.filter(|v| !v.is_none()).map(|v| v.clone().unbind()),
            tab_stops: tab_stops
                .filter(|v| !v.is_none())
                .map(|v| v.clone().unbind()),
        };
        let mut builder = new_style(&style_id, name);
        if let Some(parent) = based_on {
            builder = builder.based_on(&style_id_of_type(&self.inner, parent, native_type)?);
        }
        if let Some(next) = next_style {
            if native_type != oxoffice_docx::StyleType::Paragraph {
                return Err(PyValueError::new_err(
                    "only a paragraph style has a next style",
                ));
            }
            builder = builder.next_style(&style_id_of_type(
                &self.inner,
                next,
                oxoffice_docx::StyleType::Paragraph,
            )?);
        }
        if let Some(properties) = formatting.paragraph_properties(py, None)? {
            if native_type == oxoffice_docx::StyleType::Character {
                return Err(PyValueError::new_err(
                    "a character style takes no paragraph formatting",
                ));
            }
            builder = builder.replace_paragraph_properties(properties);
        }
        if let Some(properties) = formatting.run_properties(py, None)? {
            builder = builder.run_properties(properties);
            if let Some(name) = &formatting.font_name {
                builder = builder.font(name);
            }
            if let Some((r, g, b)) = formatting.color {
                builder = builder.color(&format!("{r:02X}{g:02X}{b:02X}"));
            }
        }
        self.inner
            .add_style(builder)
            .map_err(|error| docx_to_pyerr(py, error))?;
        Ok(style_snapshot(
            self.inner
                .style(&style_id)
                .expect("the style was just added"),
        ))
    }

    // Update only the supplied formatting on a defined style. The native
    // setter stages and validates the complete style graph before publishing.
    #[pyo3(signature = (
        style,
        *,
        based_on = None,
        next_style = None,
        font_name = None,
        font_size = None,
        bold = None,
        italic = None,
        color = None,
        space_before = None,
        space_after = None,
        left_indent = None,
        right_indent = None,
        first_line_indent = None,
        alignment = None,
        line_spacing = StyleLineSpacingArg::Omitted,
        underline = None,
        keep_with_next = None,
        keep_together = None,
        page_break_before = None,
        borders = None,
        shading = None,
        tab_stops = None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn set_style(
        &mut self,
        py: Python<'_>,
        style: &str,
        based_on: Option<&str>,
        next_style: Option<&str>,
        font_name: Option<String>,
        font_size: Option<&Bound<'_, PyAny>>,
        bold: Option<bool>,
        italic: Option<bool>,
        color: Option<&Bound<'_, PyAny>>,
        space_before: Option<&Bound<'_, PyAny>>,
        space_after: Option<&Bound<'_, PyAny>>,
        left_indent: Option<&Bound<'_, PyAny>>,
        right_indent: Option<&Bound<'_, PyAny>>,
        first_line_indent: Option<&Bound<'_, PyAny>>,
        alignment: Option<i32>,
        line_spacing: StyleLineSpacingArg,
        underline: Option<&Bound<'_, PyAny>>,
        keep_with_next: Option<bool>,
        keep_together: Option<bool>,
        page_break_before: Option<bool>,
        borders: Option<&Bound<'_, PyAny>>,
        shading: Option<&Bound<'_, PyAny>>,
        tab_stops: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyStyle> {
        let color = color
            .filter(|value| !value.is_none())
            .map(|value| oxoffice_py_support::rgb_channels(value, "Document.set_style(color)"))
            .transpose()?;
        let length = |value, name: &str, unit| {
            crate::length_argument(value, &format!("Document.set_style({name})"), unit)
        };
        let font_size = length(font_size, "font_size", crate::LengthUnit::HalfPoints)?;
        let space_before = length(space_before, "space_before", crate::LengthUnit::Twips)?;
        let space_after = length(space_after, "space_after", crate::LengthUnit::Twips)?;
        let left_indent = length(left_indent, "left_indent", crate::LengthUnit::Twips)?;
        let right_indent = length(right_indent, "right_indent", crate::LengthUnit::Twips)?;
        let first_line_indent = length(
            first_line_indent,
            "first_line_indent",
            crate::LengthUnit::Twips,
        )?;
        let (style_type, style_id) = defined_style(&self.inner, style)?;
        let existing = self.inner.style(&style_id).expect("the style was found");
        let name = existing.name().unwrap_or(&style_id).to_owned();
        let mut builder = match style_type {
            oxoffice_docx::StyleType::Paragraph => {
                oxoffice_docx::StyleBuilder::paragraph(&style_id, &name)
            }
            oxoffice_docx::StyleType::Character => {
                oxoffice_docx::StyleBuilder::character(&style_id, &name)
            }
            oxoffice_docx::StyleType::Table => oxoffice_docx::StyleBuilder::table(&style_id, &name),
            _ => return Err(PyValueError::new_err("unsupported style type")),
        };
        if let Some(parent) = based_on {
            builder = builder.based_on(&style_id_of_type(&self.inner, parent, style_type)?);
        }
        if let Some(next) = next_style {
            if style_type != oxoffice_docx::StyleType::Paragraph {
                return Err(PyValueError::new_err(
                    "only a paragraph style has a next style",
                ));
            }
            builder = builder.next_style(&style_id_of_type(
                &self.inner,
                next,
                oxoffice_docx::StyleType::Paragraph,
            )?);
        }
        let formatting = StyleFormatting {
            method: "Document.set_style",
            font_name,
            font_size,
            bold,
            italic,
            color,
            space_before,
            space_after,
            left_indent,
            right_indent,
            first_line_indent,
            alignment,
            line_spacing: match line_spacing {
                StyleLineSpacingArg::Omitted => None,
                StyleLineSpacingArg::Given(value) => Some(value),
            },
            underline: underline
                .filter(|v| !v.is_none())
                .map(|v| v.clone().unbind()),
            keep_with_next,
            keep_together,
            page_break_before,
            borders: borders.filter(|v| !v.is_none()).map(|v| v.clone().unbind()),
            shading: shading.filter(|v| !v.is_none()).map(|v| v.clone().unbind()),
            tab_stops: tab_stops
                .filter(|v| !v.is_none())
                .map(|v| v.clone().unbind()),
        };
        if let Some(properties) =
            formatting.paragraph_properties(py, existing.paragraph_properties())?
        {
            if style_type == oxoffice_docx::StyleType::Character {
                return Err(PyValueError::new_err(
                    "a character style takes no paragraph formatting",
                ));
            }
            builder = builder.replace_paragraph_properties(properties);
        }
        if let Some(properties) = formatting.run_properties(py, existing.run_properties())? {
            builder = builder.run_properties(properties);
            if let Some(name) = &formatting.font_name {
                builder = builder.font(name);
            }
            if let Some((r, g, b)) = formatting.color {
                builder = builder.color(&format!("{r:02X}{g:02X}{b:02X}"));
            }
        }
        self.inner
            .set_style(builder)
            .map_err(|error| docx_to_pyerr(py, error))?;
        Ok(style_snapshot(
            self.inner
                .style(&style_id)
                .expect("the style was just updated"),
        ))
    }

    // Return false when no style has the ID or name. The native call removes
    // the linked style that links back too, releases a numbering level the
    // style points at and that names it back, and refuses a style that
    // content, another style or a numbering level still uses.
    fn remove_style(&mut self, py: Python<'_>, style: &str) -> PyResult<bool> {
        let Ok((_, style_id)) = defined_style(&self.inner, style) else {
            return Ok(false);
        };
        self.inner
            .remove_style(&style_id)
            .map_err(|error| docx_to_pyerr(py, error))
    }

    fn set_default_style(&mut self, py: Python<'_>, style: &str) -> PyResult<()> {
        let (style_type, style_id) = defined_style(&self.inner, style)?;
        self.inner
            .set_default_style(style_type, &style_id)
            .map_err(|error| docx_to_pyerr(py, error))
    }

    #[getter]
    fn default_paragraph_format(&self, py: Python<'_>) -> PyResult<PyDefaultParagraphFormat> {
        default_paragraph_format_snapshot(py, self.inner.default_paragraph_properties())
    }

    #[pyo3(signature = (**kwargs))]
    fn set_default_paragraph_format(
        &mut self,
        py: Python<'_>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<()> {
        let Some(kwargs) = kwargs else {
            return Ok(());
        };
        for (key, _) in kwargs.iter() {
            let key = key.extract::<String>()?;
            if !matches!(
                key.as_str(),
                "space_before" | "space_after" | "line_spacing"
            ) {
                return Err(PyTypeError::new_err(format!(
                    "set_default_paragraph_format() got an unexpected keyword argument '{key}'"
                )));
            }
        }
        let mut next = self
            .inner
            .default_paragraph_properties()
            .cloned()
            .unwrap_or_default();
        for name in ["space_before", "space_after", "line_spacing"] {
            let Some(value) = kwargs.get_item(name)? else {
                continue;
            };
            match name {
                "space_before" => {
                    next.space_before = length_keyword(
                        &value,
                        "Document.set_default_paragraph_format(space_before)",
                    )?;
                }
                "space_after" => {
                    next.space_after = length_keyword(
                        &value,
                        "Document.set_default_paragraph_format(space_after)",
                    )?;
                }
                "line_spacing" => {
                    let line = line_spacing_keyword(py, &value)?;
                    // A missing rule already means auto, so the same multiple
                    // writes back without adding one.
                    let kept = line.is_some_and(|(twips, rule)| {
                        rule == "auto"
                            && next.line_rule.is_none()
                            && next.line_spacing == Some(twips)
                    });
                    next.line_spacing = line.map(|(twips, _)| twips);
                    if !kept {
                        next.line_rule = line.map(|(_, rule)| rule.to_owned());
                    }
                }
                _ => unreachable!("keyword was checked above"),
            }
        }
        let current = self.inner.default_paragraph_properties().cloned();
        if current.as_ref() == Some(&next)
            || (current.is_none() && next == oxoffice_docx::CT_PPr::default())
        {
            return Ok(());
        }
        self.inner
            .set_default_paragraph_properties(Some(next))
            .map_err(|error| docx_to_pyerr(py, error))
    }

    fn part_names<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(
            py,
            self.inner
                .part_names()
                .map_err(|error| docx_to_pyerr(py, error))?,
        )
    }

    fn part_xml(&self, py: Python<'_>, name: &str) -> PyResult<String> {
        self.inner
            .part_xml(name)
            .map_err(|error| docx_to_pyerr(py, error))
    }

    fn add_numbering_definition(
        &mut self,
        py: Python<'_>,
        levels: Vec<PyRef<'_, PyListLevel>>,
    ) -> PyResult<u32> {
        let levels = levels
            .iter()
            .map(|level| level.native())
            .collect::<Vec<_>>();
        self.inner
            .add_numbering_definition(&levels)
            .map_err(|error| docx_to_pyerr(py, error))
    }

    #[pyo3(signature = (definition_id, *, start = None, level = 0))]
    fn add_numbering_instance(
        &mut self,
        py: Python<'_>,
        definition_id: u32,
        start: Option<i64>,
        level: i64,
    ) -> PyResult<u32> {
        if !(0..=8).contains(&level) {
            return Err(PyValueError::new_err("level must be between 0 and 8"));
        }
        if start.is_none() && level != 0 {
            return Err(PyValueError::new_err("nonzero level requires start"));
        }
        let overrides = start
            .map(|start| {
                let start = u32::try_from(start)
                    .map_err(|_| PyValueError::new_err("start must be between 0 and 4294967295"))?;
                Ok::<_, PyErr>(
                    oxoffice_docx::NumberingLevelOverride::new(level as u32).start(start),
                )
            })
            .transpose()?
            .into_iter()
            .collect::<Vec<_>>();
        self.inner
            .add_numbering_instance(definition_id, &overrides)
            .map_err(|error| docx_to_pyerr(py, error))
    }

    #[getter]
    fn charts<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let values = self
            .inner
            .charts()
            .map_err(|e| docx_to_pyerr(py, e))?
            .into_iter()
            .map(|(chart_type, data)| Py::new(py, PyChart { chart_type, data }))
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, values)
    }

    #[pyo3(signature = (paragraph, *, start=Python::attach(|py| PyInt::new(py, 1).into_any().unbind())), text_signature = "($self, paragraph, *, start=1)")]
    fn restart_numbering(
        slf: Py<Self>,
        py: Python<'_>,
        paragraph: PyRef<'_, PyParagraph>,
        start: Py<PyAny>,
    ) -> PyResult<u32> {
        let start = start.bind(py);
        if !start.is_instance_of::<PyInt>() || start.is_instance_of::<PyBool>() {
            return Err(PyTypeError::new_err("start must be a non-bool integer"));
        }
        let start = start.extract::<u32>()?;
        if !paragraph.belongs_to(py, &slf) {
            return Err(PyValueError::new_err(
                "paragraph belongs to a different document",
            ));
        }
        let ParagraphLocation::Body(index) = paragraph.validate(py)? else {
            return Err(PyValueError::new_err(
                "numbering restart requires a direct body paragraph",
            ));
        };
        let mut document = slf.borrow_mut(py);
        let mut candidate = document.inner.clone_for_staging();
        let id = candidate
            .restart_numbering(index, start)
            .map_err(|e| docx_to_pyerr(py, e))?;
        document.publish_api_candidate(py, candidate, "Document.restart_numbering")?;
        Ok(id)
    }

    fn remove_footnote(&mut self, py: Python<'_>, id: &Bound<'_, PyAny>) -> PyResult<()> {
        self.remove_note_by_id(py, id, false)
    }

    fn remove_endnote(&mut self, py: Python<'_>, id: &Bound<'_, PyAny>) -> PyResult<()> {
        self.remove_note_by_id(py, id, true)
    }

    /// Effective legend properties of the live direct-body chart paragraph.
    fn chart_legend(
        slf: Py<Self>,
        py: Python<'_>,
        paragraph: PyRef<'_, PyParagraph>,
    ) -> PyResult<Py<PyDict>> {
        if !paragraph.belongs_to(py, &slf) {
            return Err(PyValueError::new_err(
                "paragraph belongs to a different document",
            ));
        }
        let ParagraphLocation::Body(index) = paragraph.validate(py)? else {
            return Err(PyValueError::new_err(
                "chart legend requires a direct body paragraph",
            ));
        };
        let settings = slf
            .borrow(py)
            .inner
            .chart_legend(index)
            .map_err(|e| docx_to_pyerr(py, e))?;
        let result = PyDict::new(py);
        result.set_item("visible", settings.visible)?;
        result.set_item("position", settings.position.as_str())?;
        result.set_item("overlay", settings.overlay)?;
        Ok(result.unbind())
    }

    /// None leaves a property unchanged. Hidden legends discard their settings.
    #[pyo3(signature=(paragraph, *, visible=None, position=None, overlay=None))]
    fn set_chart_legend(
        slf: Py<Self>,
        py: Python<'_>,
        paragraph: PyRef<'_, PyParagraph>,
        visible: Option<&Bound<'_, PyAny>>,
        position: Option<&str>,
        overlay: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let checked_bool = |value: Option<&Bound<'_, PyAny>>| -> PyResult<Option<bool>> {
            value
                .map(|v| {
                    if !v.is_instance_of::<PyBool>() {
                        return Err(PyTypeError::new_err(
                            "legend visibility and overlay require bool",
                        ));
                    }
                    v.extract()
                })
                .transpose()
        };
        let visible = checked_bool(visible)?;
        let overlay = checked_bool(overlay)?;
        let position = position
            .map(oxoffice_docx::LegendPosition::parse)
            .transpose()
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        if !paragraph.belongs_to(py, &slf) {
            return Err(PyValueError::new_err(
                "paragraph belongs to a different document",
            ));
        }
        let ParagraphLocation::Body(index) = paragraph.validate(py)? else {
            return Err(PyValueError::new_err(
                "chart legend requires a direct body paragraph",
            ));
        };
        let mut document = slf.borrow_mut(py);
        let location = document
            .paragraph_route(&ParagraphLocation::Body(index), None)
            .map_err(|e| docx_to_pyerr(py, e))?;
        let mut candidate = document.inner.clone_for_staging();
        candidate
            .set_chart_legend(index, visible, position, overlay)
            .map_err(|e| docx_to_pyerr(py, e))?;
        document.publish_owner_candidate(
            py,
            candidate,
            "Document.set_chart_legend",
            WordPublication::ElementXml(WordOwnerLocation::Paragraph(location)),
        )
    }

    #[pyo3(signature = (chart_type, chart_data, width, height, *, after=None))]
    fn add_chart(
        slf: Py<Self>,
        py: Python<'_>,
        chart_type: &Bound<'_, PyAny>,
        chart_data: &Bound<'_, PyAny>,
        width: &Bound<'_, PyAny>,
        height: &Bound<'_, PyAny>,
        after: Option<PyRef<'_, PyParagraph>>,
    ) -> PyResult<Py<PyParagraph>> {
        let (kind, data) = checked_chart_inputs(chart_type, chart_data)?;
        let width = oxoffice_docx::Length::emu(crate::required_length(
            width,
            "Document.add_chart(width)",
            crate::LengthUnit::Emu,
        )?);
        let height = oxoffice_docx::Length::emu(crate::required_length(
            height,
            "Document.add_chart(height)",
            crate::LengthUnit::Emu,
        )?);
        let after = after
            .map(|paragraph| {
                if !paragraph.belongs_to(py, &slf) {
                    return Err(PyValueError::new_err(
                        "paragraph belongs to a different document",
                    ));
                }
                let ParagraphLocation::Body(index) = paragraph.validate(py)? else {
                    return Err(PyValueError::new_err(
                        "chart insertion requires a direct body paragraph",
                    ));
                };
                Ok(index)
            })
            .transpose()?;
        let mut document = slf.borrow_mut(py);
        let index = after.map_or(document.inner.paragraph_count(), |index| index + 1);
        let mut candidate = document.inner.clone_for_staging();
        match after {
            Some(after) => candidate.add_chart_after(after, kind, width, height, &data),
            None => candidate.add_chart(kind, width, height, &data),
        }
        .map(drop)
        .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "Document.add_chart",
            if after.is_none() {
                WordPublication::AppendBody
            } else {
                WordPublication::Conservative
            },
        )?;
        let path = document
            .revisions
            .capture_prepared(
                &prepared.revision,
                smallvec![PathSeg::Body(0), PathSeg::Para(index)],
            )
            .map_err(revision_to_pyerr)?;
        let result = Py::new(
            py,
            PyParagraph::new(slf.clone_ref(py), path)
                .bind_prepared(py, &document, &prepared, &candidate, None)?,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        Ok(result)
    }

    #[pyo3(signature = (text, level = 0))]
    fn add_bullet_list_item(
        slf: Py<Self>,
        py: Python<'_>,
        text: &str,
        level: i64,
    ) -> PyResult<Py<PyParagraph>> {
        if !(0..=8).contains(&level) {
            return Err(PyValueError::new_err("level must be between 0 and 8"));
        }
        Self::append_list_item(
            slf,
            py,
            oxoffice_docx::ListItemKind::Bullet,
            text,
            level as u32,
            "Document.add_bullet_list_item",
        )
    }

    #[pyo3(signature = (text, level = 0, *, restart = false))]
    fn add_numbered_list_item(
        slf: Py<Self>,
        py: Python<'_>,
        text: &str,
        level: i64,
        restart: bool,
    ) -> PyResult<Py<PyParagraph>> {
        if !(0..=8).contains(&level) {
            return Err(PyValueError::new_err("level must be between 0 and 8"));
        }
        if !restart {
            return Self::append_list_item(
                slf,
                py,
                oxoffice_docx::ListItemKind::Numbered,
                text,
                level as u32,
                "Document.add_numbered_list_item",
            );
        }
        // A restart gives the item an instance of its own through the
        // facade's staged restart.
        let mut document = slf.borrow_mut(py);
        let index = document.inner.paragraph_count();
        let mut candidate = document.inner.clone_for_staging();
        candidate
            .try_add_numbered_list_item_with_restart(text, level as u32, restart)
            .map(|_| ())
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "Document.add_numbered_list_item",
            crate::document::WordPublication::Conservative,
        )?;
        let path = document
            .revisions
            .capture_prepared(
                &prepared.revision,
                smallvec![PathSeg::Body(0), PathSeg::Para(index)],
            )
            .map_err(oxoffice_py_support::revision_to_pyerr)?;
        let result = Py::new(
            py,
            PyParagraph::new(slf.clone_ref(py), path)
                .bind_prepared(py, &document, &prepared, &candidate, None)?,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        Ok(result)
    }

    fn link_style_to_numbering(
        &mut self,
        py: Python<'_>,
        style: &str,
        num_id: u32,
        level: u32,
    ) -> PyResult<()> {
        let style_id = style_id_of_type(&self.inner, style, oxoffice_docx::StyleType::Paragraph)?;
        self.inner
            .link_style_to_numbering(&style_id, num_id, level)
            .map_err(|error| docx_to_pyerr(py, error))
    }

    #[getter]
    fn stories<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        self.ensure_word_authority(py)?;
        let stories = self
            .inner
            .stories()
            .map_err(|error| docx_to_pyerr(py, error))?;
        PyTuple::new(
            py,
            stories
                .iter()
                .map(|story| self.owned_story_snapshot(story))
                .collect::<PyResult<Vec<_>>>()?,
        )
    }

    #[getter]
    fn story_items<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyTuple>> {
        let py = slf.py();
        let owner = std::sync::Arc::new(slf.clone().unbind());
        let mut this = slf.borrow_mut();
        this.ensure_word_authority(py)?;
        let items = this
            .inner
            .standalone_story_item_snapshots()
            .map_err(|error| docx_to_pyerr(py, error))?;
        let snapshots = items
            .into_iter()
            .map(|item| {
                Ok(PyStoryItem {
                    story: this.owned_story_snapshot(item.location().story())?,
                    kind: story_item_kind_name(item.location().item_kind()).to_owned(),
                    index_path: item.location().index_path().to_vec(),
                    direct_body_index: item.direct_body_index(),
                    text: item.text().map(str::to_owned),
                    xml: item.xml().to_vec(),
                    revision: this.revisions.current(),
                    binding: Some(this.capture_snapshot(smallvec![])?),
                    document: Some(owner.clone()),
                })
            })
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, snapshots)
    }

    #[getter]
    fn story_paragraphs<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        self.ensure_word_authority(py)?;
        let paragraphs = py
            .detach(|| self.inner.story_paragraphs())
            .map_err(|error| docx_to_pyerr(py, error))?;
        let snapshots = paragraphs
            .into_iter()
            .map(|paragraph| {
                Ok(PyStoryParagraph {
                    kind: story_kind_name(paragraph.kind).to_owned(),
                    story: self.owned_story_snapshot(paragraph.location.story())?,
                    index_path: paragraph.location.index_path().to_vec(),
                    style: paragraph.style,
                    text: paragraph.text,
                })
            })
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, snapshots)
    }

    fn count_text<'py>(&self, py: Python<'py>, text: &str) -> PyResult<Bound<'py, PyDict>> {
        if text.is_empty() {
            return Err(PyValueError::new_err("count_text needs a text to count"));
        }
        let counts = py
            .detach(|| self.inner.count_text(text))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let by_kind = PyDict::new(py);
        for (kind, count) in counts {
            by_kind.set_item(story_kind_name(kind), count)?;
        }
        Ok(by_kind)
    }

    #[getter]
    fn header_footer_variants<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        self.ensure_word_authority(py)?;
        let mut snapshots = Vec::new();
        for section_index in 0..self.inner.section_count() {
            for (kind, kind_name) in [
                (oxoffice_docx::HeaderFooterKind::Header, "header"),
                (oxoffice_docx::HeaderFooterKind::Footer, "footer"),
            ] {
                for variant in [
                    oxoffice_docx::HdrFtrType::Default,
                    oxoffice_docx::HdrFtrType::First,
                    oxoffice_docx::HdrFtrType::Even,
                ] {
                    let resolved = self
                        .inner
                        .section_story(section_index, kind, variant)
                        .map_err(|error| docx_to_pyerr(py, error))?;
                    snapshots.push(PyHeaderFooterVariant {
                        section_index,
                        kind: kind_name.to_owned(),
                        variant: variant.to_str().to_owned(),
                        story: resolved
                            .as_ref()
                            .map(|value| self.owned_story_snapshot(value.story()))
                            .transpose()?,
                        source_section: resolved
                            .as_ref()
                            .map(oxoffice_docx::SectionStory::source_section),
                        inherited: resolved.is_some_and(|value| value.is_inherited()),
                    });
                }
            }
        }
        PyTuple::new(py, snapshots)
    }

    // The section story operations publish a reopened package, so each one
    // advances the revision once, even when the variant already had a story.
    fn create_section_story(
        &mut self,
        py: Python<'_>,
        section_index: usize,
        kind: &str,
        variant: &str,
    ) -> PyResult<Py<PyStory>> {
        let (kind, variant) = self.section_story_target(section_index, kind, variant)?;
        let mut candidate = self.inner.clone_for_staging();
        let story = py
            .detach(|| candidate.create_section_story(section_index, kind, variant))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.create_section_story",
            crate::document::WordPublication::Conservative,
        )?;
        let mut snapshot = story_snapshot(&story);
        snapshot.binding = Some(self.capture_prepared_snapshot(&prepared, smallvec![])?);
        let result = Py::new(py, snapshot)?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(result)
    }

    fn link_section_story(
        &mut self,
        py: Python<'_>,
        section_index: usize,
        kind: &str,
        variant: &str,
        story: PyRef<'_, PyStory>,
    ) -> PyResult<Py<PyStory>> {
        let (kind, variant) = self.section_story_target(section_index, kind, variant)?;
        let story = self.native_story(py, &story)?;
        let mut candidate = self.inner.clone_for_staging();
        let linked = py
            .detach(|| candidate.link_section_story(section_index, kind, variant, &story))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.link_section_story",
            crate::document::WordPublication::Conservative,
        )?;
        let mut snapshot = story_snapshot(&linked);
        snapshot.binding = Some(self.capture_prepared_snapshot(&prepared, smallvec![])?);
        let result = Py::new(py, snapshot)?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(result)
    }

    fn unlink_section_story(
        &mut self,
        py: Python<'_>,
        section_index: usize,
        kind: &str,
        variant: &str,
    ) -> PyResult<Py<PyStory>> {
        let (kind, variant) = self.section_story_target(section_index, kind, variant)?;
        let mut candidate = self.inner.clone_for_staging();
        let story = py
            .detach(|| candidate.unlink_section_story(section_index, kind, variant))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.unlink_section_story",
            crate::document::WordPublication::Conservative,
        )?;
        let mut snapshot = story_snapshot(&story);
        snapshot.binding = Some(self.capture_prepared_snapshot(&prepared, smallvec![])?);
        let result = Py::new(py, snapshot)?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(result)
    }

    #[getter]
    fn hyperlinks<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        self.ensure_word_authority(py)?;
        let links = self
            .inner
            .story_link_snapshots()
            .map_err(|error| docx_to_pyerr(py, error))?;
        let mut story_links = HashMap::<_, Vec<_>>::new();
        for (location, link) in &links {
            story_links
                .entry(location.story().clone())
                .or_default()
                .push((location.index_path(), link.text.as_str()));
        }
        let mut layouts = story_links
            .into_iter()
            .map(|(story, links)| (story, (0usize, story_link_layout(links))))
            .collect::<HashMap<_, _>>();
        let snapshots = links
            .into_iter()
            .map(|(location, link)| {
                let (next, layout) = layouts
                    .get_mut(location.story())
                    .expect("every snapshot story has a layout");
                let position = Some((*next, *layout));
                let owner = self
                    .owners
                    .iter()
                    .find(|owner| {
                        matches!(&*owner.location(), WordOwnerLocation::Hyperlink(current, _)
                        if same_content_owner(current, &location))
                            && owner.projection_ordinal() == Some(*next)
                    })
                    .ok_or_else(|| {
                        PyValueError::new_err("hyperlink has no checked physical source owner")
                    })?;
                let binding = self
                    .revisions
                    .capture_scopes(smallvec![], std::slice::from_ref(&owner.owner))
                    .map_err(revision_to_pyerr)?;
                let owner = Some((binding, owner.owner.clone()));
                *next += 1;
                Ok(PyHyperlink {
                    story: self.owned_story_snapshot(location.story())?,
                    index_path: location.index_path().to_vec(),
                    text: link.text,
                    url: link.url,
                    anchor: link.anchor,
                    relationship_id: link.rel_id,
                    position,
                    owner,
                })
            })
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, snapshots)
    }

    fn set_hyperlink_url(
        &mut self,
        py: Python<'_>,
        hyperlink: PyRef<'_, PyHyperlink>,
        url: &str,
    ) -> PyResult<()> {
        let (story, link_index) = self.native_hyperlink(py, &hyperlink)?;
        // No content moves, so live handles stay valid.
        py.detach(|| self.inner.set_hyperlink_url(&story, link_index, url))
            .map_err(|error| docx_to_pyerr(py, error))
    }

    fn remove_hyperlink(
        &mut self,
        py: Python<'_>,
        hyperlink: PyRef<'_, PyHyperlink>,
    ) -> PyResult<()> {
        let (story, link_index) = self.native_hyperlink(py, &hyperlink)?;
        let mut candidate = self.inner.clone_for_staging();
        py.detach(|| candidate.remove_hyperlink(&story, link_index))
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.ensure_word_authority(py)?;
        let selected = self
            .owners
            .iter()
            .find_map(|owner| match &*owner.location() {
                WordOwnerLocation::Hyperlink(location, _)
                    if same_story_owner(location.story(), &story)
                        && owner.projection_ordinal() == Some(link_index) =>
                {
                    Some(owner.location().into_owned())
                }
                _ => None,
            })
            .ok_or_else(|| {
                PyValueError::new_err("hyperlink has no checked physical source owner")
            })?;
        self.publish_owner_candidate(
            py,
            candidate,
            "Document.remove_hyperlink",
            WordPublication::HyperlinkRemove(selected),
        )
    }

    fn set_header(&mut self, py: Python<'_>, text: &str) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        py.detach(|| candidate.try_set_header(text))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.set_header",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    fn set_footer(&mut self, py: Python<'_>, text: &str) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        py.detach(|| candidate.try_set_footer(text))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.set_footer",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    fn set_story_text(
        &mut self,
        py: Python<'_>,
        item: PyRef<'_, PyStoryItem>,
        text: &str,
    ) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        let location = self.native_location(py, &item)?;
        py.detach(|| candidate.set_story_text(&location, text))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.set_story_text",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    fn add_hyperlink_to_story(
        &mut self,
        py: Python<'_>,
        story: PyRef<'_, PyStory>,
        text: &str,
        url: &str,
    ) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        let story = self.native_story(py, &story)?;
        py.detach(|| candidate.add_hyperlink_to_story(&story, text, url))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.add_hyperlink_to_story",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)] // Preserve explicit checked public picture operands.
    #[pyo3(signature = (data, filename=None, width=None, height=None, *, after=None, options=None))]
    fn add_picture(
        &mut self,
        py: Python<'_>,
        data: &Bound<'_, PyAny>,
        filename: Option<&str>,
        width: Option<&Bound<'_, PyAny>>,
        height: Option<&Bound<'_, PyAny>>,
        after: Option<&Bound<'_, PyAny>>,
        options: Option<PyRef<'_, PyPictureOptions>>,
    ) -> PyResult<Py<PyPicture>> {
        let (data, filename) = picture_input(data, filename)?;
        let width =
            crate::length_argument(width, "Document.add_picture(width)", crate::LengthUnit::Emu)?
                .map(oxoffice_docx::Length::emu);
        let height = crate::length_argument(
            height,
            "Document.add_picture(height)",
            crate::LengthUnit::Emu,
        )?
        .map(oxoffice_docx::Length::emu);
        if width.is_some() != height.is_some() {
            return Err(crate::docx_to_pyerr(
                py,
                oxoffice_docx::Error::Other(
                    "use add_picture(..., width=..., height=...) with both dimensions; omit both for the native image size".to_owned(),
                ),
            ));
        }
        let after = after
            .map(|value| -> PyResult<oxoffice_docx::ContentLocation> {
                if let Ok(item) = value.extract::<PyRef<'_, PyStoryItem>>() {
                    return self.native_location(py, &item);
                }
                if let Ok(picture) = value.extract::<PyRef<'_, PyPicture>>() {
                    picture
                        .binding
                        .validate_scopes(
                            &self.revisions,
                            "picture",
                            "Re-fetch it with document.pictures().",
                        )
                        .map_err(|error| crate::stale_to_pyerr(py, error))?;
                    let current = self
                        .inner
                        .pictures()
                        .map_err(|error| docx_to_pyerr(py, error))?;
                    if !current.contains(&picture.inner) {
                        return Err(PyValueError::new_err("picture placement source changed"));
                    }
                    return Ok(picture.inner.location.clone());
                }
                Err(PyTypeError::new_err("after must be a StoryItem or Picture"))
            })
            .transpose()?;
        let story = match &after {
            Some(location) => location.story().clone(),
            None => self.body_story(py)?,
        };
        let candidate = self.inner.clone_for_staging();
        let (candidate, location, pictures) = candidate
            .insert_picture_to_story_with_options_and_inventory_owned(
                &story,
                after.as_ref(),
                &data,
                &filename,
                width,
                height,
                options.map(|options| options.inner.clone()),
            )
            .map_err(|error| docx_to_pyerr(py, error))?;
        let picture = pictures
            .into_iter()
            .find(|picture| picture.location == location)
            .ok_or_else(|| PyIndexError::new_err("inserted picture was not inventoried"))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.add_picture",
            crate::document::WordPublication::Conservative,
        )?;
        let result = Py::new(
            py,
            PyPicture {
                inner: picture,
                binding: self.capture_prepared_snapshot(&prepared, smallvec![])?,
            },
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(result)
    }

    #[pyo3(signature = (range, *, author, text, initials = None, date = None, undated = false))]
    #[allow(clippy::too_many_arguments)]
    fn add_comment(
        &mut self,
        range: &Bound<'_, PyAny>,
        author: &str,
        text: &str,
        initials: Option<&str>,
        date: Option<&str>,
        undated: bool,
        py: Python<'_>,
    ) -> PyResult<i32> {
        let date = comment_date(date, undated)?;
        let mut candidate = self.inner.clone_for_staging();
        let id = if let Ok(range) = range.cast::<PyRunRange>() {
            candidate.add_comment_with_date((*range.borrow()).into(), author, initials, text, date)
        } else if let Ok(range) = range.cast::<PyStoryRunRange>() {
            let range = range.borrow();
            let start = oxoffice_docx::StoryRunPosition {
                location: self.native_location(py, &range.start.item)?,
                run_index: range.start.run_index,
            };
            let end = oxoffice_docx::StoryRunPosition {
                location: self.native_location(py, &range.end.item)?,
                run_index: range.end.run_index,
            };
            candidate.add_story_comment_with_date(
                oxoffice_docx::StoryRunRange { start, end },
                author,
                initials,
                text,
                date,
            )
        } else {
            return Err(PyTypeError::new_err(
                "range must be a RunRange or StoryRunRange",
            ));
        }
        .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.add_comment",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(id)
    }

    /// Comment on the `occurrence`-th match of `anchor`, counted from zero,
    /// in the main story, or among the matches in table cells only with
    /// `tables_only`. Matching is case-sensitive, non-overlapping and within
    /// one paragraph. A match whose range would also show other text, such as
    /// a field result, raises.
    #[pyo3(signature = (anchor, *, author, text, occurrence = 0, tables_only = false, initials = None, date = None, undated = false))]
    #[allow(clippy::too_many_arguments)]
    fn add_comment_on_text(
        &mut self,
        anchor: &str,
        author: &str,
        text: &str,
        occurrence: usize,
        tables_only: bool,
        initials: Option<&str>,
        date: Option<&str>,
        undated: bool,
        py: Python<'_>,
    ) -> PyResult<i32> {
        let date = comment_date(date, undated)?;
        let mut candidate = self.inner.clone_for_staging();
        let id = candidate
            .add_comment_on_text(
                anchor,
                author,
                text,
                oxoffice_docx::TextCommentOptions {
                    occurrence,
                    tables_only,
                    initials,
                    date,
                },
            )
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.add_comment_on_text",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(id)
    }

    /// Move a root comment and its replies to a checked current story range
    /// atomically.
    fn move_comment(
        &mut self,
        id: i32,
        range: PyRef<'_, PyStoryRunRange>,
        py: Python<'_>,
    ) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        let start = oxoffice_docx::StoryRunPosition {
            location: self.native_location(py, &range.start.item)?,
            run_index: range.start.run_index,
        };
        let end = oxoffice_docx::StoryRunPosition {
            location: self.native_location(py, &range.end.item)?,
            run_index: range.end.run_index,
        };
        py.detach(|| candidate.move_comment(id, oxoffice_docx::StoryRunRange { start, end }))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.move_comment",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    /// Move a root comment and its replies onto the selected literal
    /// main-story occurrence.
    #[pyo3(signature = (id, anchor, *, occurrence = 0))]
    fn move_comment_to_text(
        &mut self,
        id: i32,
        anchor: &str,
        occurrence: usize,
        py: Python<'_>,
    ) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        py.detach(|| candidate.move_comment_to_text(id, anchor, occurrence))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.move_comment_to_text",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    #[pyo3(signature = (parent_id, *, author, text, date = None, initials = None, undated = false), text_signature = "($self, parent_id, *, author, text, date=None, initials=None, undated=False)")]
    #[allow(clippy::too_many_arguments)]
    fn reply_to(
        &mut self,
        parent_id: i32,
        author: &str,
        text: &str,
        date: Option<&str>,
        initials: Option<&str>,
        undated: bool,
        py: Python<'_>,
    ) -> PyResult<i32> {
        let date = comment_date(date, undated)?;
        let mut candidate = self.inner.clone_for_staging();
        let id = candidate
            .reply_to_with_initials(parent_id, author, initials, text, date)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.reply_to",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(id)
    }

    #[pyo3(signature = (id, *, resolved = true))]
    fn resolve_comment(&mut self, id: i32, resolved: bool, py: Python<'_>) -> PyResult<bool> {
        let mut candidate = self.inner.clone_for_staging();
        let updated = candidate
            .resolve_comment(id, resolved)
            .map_err(|error| docx_to_pyerr(py, error))?;
        if updated {
            let prepared = self.prepare_word_revision(
                py,
                &candidate,
                "Document.resolve_comment",
                crate::document::WordPublication::Conservative,
            )?;
            self.commit_word_revision(prepared)?;
            self.install_inner(candidate);
        }
        Ok(updated)
    }

    fn remove_comment(&mut self, id: i32, py: Python<'_>) -> PyResult<bool> {
        let mut candidate = self.inner.clone_for_staging();
        let removed = candidate
            .remove_comment(id)
            .map_err(|error| docx_to_pyerr(py, error))?;
        if removed {
            let prepared = self.prepare_word_revision(
                py,
                &candidate,
                "Document.remove_comment",
                crate::document::WordPublication::Conservative,
            )?;
            self.commit_word_revision(prepared)?;
            self.install_inner(candidate);
        }
        Ok(removed)
    }

    #[getter]
    fn bookmarks<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(
            py,
            self.inner
                .bookmarks()
                .into_iter()
                .map(|bookmark| PyBookmark {
                    id: bookmark.id(),
                    name: bookmark.name().map(str::to_owned),
                    range: bookmark.range().map(PyRunRange::from),
                    direct_range: bookmark.direct_range().map(PyRunRange::from),
                    text: bookmark.text().to_owned(),
                    issue: bookmark.issue().map(str::to_owned),
                }),
        )
    }

    fn add_bookmark(
        &mut self,
        py: Python<'_>,
        name: &str,
        range: PyRef<'_, PyRunRange>,
    ) -> PyResult<i32> {
        // Bookmark markers sit between runs, so no content moves and live
        // handles stay valid.
        self.inner
            .add_bookmark(name, (*range).into())
            .map_err(|error| docx_to_pyerr(py, error))
    }

    #[pyo3(signature = (*, fonts = None, font_dir = None, font_dirs = None, system_fonts = false, bundled_fonts_only = false))]
    fn layout<'py>(
        &self,
        py: Python<'py>,
        fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
        font_dir: Option<PathBuf>,
        font_dirs: Option<Bound<'py, PyAny>>,
        system_fonts: bool,
        bundled_fonts_only: bool,
    ) -> PyResult<Bound<'py, PyTuple>> {
        let options = with_font_switches(
            oxoffice_docx::RenderOptions::default(),
            system_fonts,
            bundled_fonts_only,
        )?;
        let caller_fonts = collect_caller_fonts(py, fonts, font_dir, font_dirs)?;
        let font_refs = caller_fonts.as_ref().map(|files| {
            files
                .iter()
                .map(|font| (font.family.as_str(), font.data.as_slice()))
                .collect::<Vec<_>>()
        });
        let fragments = py
            .detach(|| {
                let layout = self
                    .inner
                    .layout_for_render(font_refs.as_deref().unwrap_or(&[]), options)?;
                let mut fragments = Vec::new();
                for body_index in 0..self.inner.content_count() {
                    let Some(body_fragments) = layout.body_layout_fragments(body_index) else {
                        continue;
                    };
                    fragments.extend(body_fragments.iter().map(|fragment| PyLayoutFragment {
                        body_index,
                        physical_page: fragment.physical_page,
                        displayed_page: fragment.displayed_page,
                        bounds: PyBoundingBox {
                            x: fragment.x,
                            y: fragment.y,
                            width: fragment.width,
                            height: fragment.height,
                        },
                    }));
                }
                Ok::<_, oxoffice_docx::Error>(fragments)
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        PyTuple::new(py, fragments)
    }

    #[pyo3(signature = (page_index, *, fonts = None, font_dir = None, font_dirs = None, system_fonts = false, bundled_fonts_only = false))]
    #[allow(clippy::too_many_arguments)]
    fn layout_page<'py>(
        &self,
        page_index: usize,
        py: Python<'py>,
        fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
        font_dir: Option<PathBuf>,
        font_dirs: Option<Bound<'py, PyAny>>,
        system_fonts: bool,
        bundled_fonts_only: bool,
    ) -> PyResult<Option<PyLayoutPage>> {
        let options = with_font_switches(
            oxoffice_docx::RenderOptions::default(),
            system_fonts,
            bundled_fonts_only,
        )?;
        let caller_fonts = collect_caller_fonts(py, fonts, font_dir, font_dirs)?;
        let font_refs = caller_fonts.as_ref().map(|files| {
            files
                .iter()
                .map(|font| (font.family.as_str(), font.data.as_slice()))
                .collect::<Vec<_>>()
        });
        py.detach(|| {
            let layout = self
                .inner
                .layout_for_render(font_refs.as_deref().unwrap_or(&[]), options)?;
            Ok(layout
                .layout
                .pages
                .get(page_index)
                .map(|page| PyLayoutPage {
                    page_number: page.page_number,
                    displayed_page_number: page.displayed_page_number,
                    width: page.width,
                    height: page.height,
                }))
        })
        .map_err(|error| docx_to_pyerr(py, error))
    }

    fn rebuild_toc(&mut self, py: Python<'_>) -> PyResult<Py<PyTocRebuildReport>> {
        let mut candidate = self.inner.clone_for_staging();
        let (report, changed) = py
            .detach(|| {
                let before = candidate.to_bytes()?;
                let report = candidate.rebuild_toc()?;
                let changed = candidate.to_bytes()? != before;
                Ok::<_, oxoffice_docx::Error>((report, changed))
            })
            .map_err(|error| docx_to_pyerr(py, error))?;
        let result = Py::new(
            py,
            PyTocRebuildReport {
                entry_count: report.entry_count,
                bookmark_count: report.bookmark_count,
                diagnostics: report.diagnostics,
            },
        )?;
        if changed {
            let prepared = self.prepare_word_revision(
                py,
                &candidate,
                "Document.rebuild_toc",
                crate::document::WordPublication::Conservative,
            )?;
            self.commit_word_revision(prepared)?;
            self.install_inner(candidate);
        }
        Ok(result)
    }

    #[getter(revisions)]
    fn revision_snapshots<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        self.ensure_word_authority(py)?;
        let revisions = self
            .inner
            .story_revisions()
            .map_err(|error| docx_to_pyerr(py, error))?;
        PyTuple::new(
            py,
            revisions
                .iter()
                .map(|revision| {
                    Ok(PyRevision {
                        id: revision.id(),
                        author: revision.author().to_owned(),
                        timestamp: revision.timestamp().map(str::to_owned),
                        kind: revision_kind_name(revision.kind()).to_owned(),
                        story: Some(self.owned_story_snapshot(revision.story())?),
                        text: revision.text().to_owned(),
                    })
                })
                .collect::<PyResult<Vec<_>>>()?,
        )
    }

    fn accept_all(&mut self, py: Python<'_>) -> PyResult<usize> {
        self.counted_mutation(
            py,
            "Document.accept_all",
            oxoffice_docx::Document::accept_all,
        )
    }

    fn reject_all(&mut self, py: Python<'_>) -> PyResult<usize> {
        self.counted_mutation(
            py,
            "Document.reject_all",
            oxoffice_docx::Document::reject_all,
        )
    }

    fn accept_revisions_by_author(&mut self, py: Python<'_>, author: &str) -> PyResult<usize> {
        self.counted_mutation(py, "Document.accept_revisions_by_author", |document| {
            document.accept_revisions_by_author(author)
        })
    }

    fn reject_revisions_by_author(&mut self, py: Python<'_>, author: &str) -> PyResult<usize> {
        self.counted_mutation(py, "Document.reject_revisions_by_author", |document| {
            document.reject_revisions_by_author(author)
        })
    }

    #[pyo3(signature = (*, start, end))]
    fn accept_revisions_in_date_range(
        &mut self,
        py: Python<'_>,
        start: &str,
        end: &str,
    ) -> PyResult<usize> {
        self.counted_mutation(py, "Document.accept_revisions_in_date_range", |document| {
            document.accept_revisions_in_date_range(start, end)
        })
    }

    #[pyo3(signature = (*, start, end))]
    fn reject_revisions_in_date_range(
        &mut self,
        py: Python<'_>,
        start: &str,
        end: &str,
    ) -> PyResult<usize> {
        self.counted_mutation(py, "Document.reject_revisions_in_date_range", |document| {
            document.reject_revisions_in_date_range(start, end)
        })
    }

    fn accept_revision_id(&mut self, py: Python<'_>, id: i32) -> PyResult<usize> {
        self.counted_mutation(py, "Document.accept_revision_id", |document| {
            document.accept_revision_id(id)
        })
    }

    fn reject_revision_id(&mut self, py: Python<'_>, id: i32) -> PyResult<usize> {
        self.counted_mutation(py, "Document.reject_revision_id", |document| {
            document.reject_revision_id(id)
        })
    }

    /// Replace literal text only inside a checked paragraph, table or control item.
    #[pyo3(signature = (item, old, new, *, expect = None))]
    fn replace_text_at(
        &mut self,
        py: Python<'_>,
        item: &PyStoryItem,
        old: &str,
        new: &str,
        expect: Option<usize>,
    ) -> PyResult<usize> {
        // Constructed/numeric/unknown/nested-control selectors retain the
        // original resolver and its exact validation/error order.
        let story_kind = match item.story.kind.as_str() {
            "body" => Some(oxoffice_docx::StoryKind::Body),
            "table_cell" => Some(oxoffice_docx::StoryKind::TableCell),
            "header" => Some(oxoffice_docx::StoryKind::Header),
            "footer" => Some(oxoffice_docx::StoryKind::Footer),
            "footnote" => Some(oxoffice_docx::StoryKind::Footnote),
            "endnote" => Some(oxoffice_docx::StoryKind::Endnote),
            "comment" => Some(oxoffice_docx::StoryKind::Comment),
            "text_box" => Some(oxoffice_docx::StoryKind::TextBox),
            _ => None,
        };
        let item_kind = match item.kind.as_str() {
            "paragraph" => Some(oxoffice_docx::StoryItemKind::Paragraph),
            "table" => Some(oxoffice_docx::StoryItemKind::Table),
            "content_control" => Some(oxoffice_docx::StoryItemKind::ContentControl),
            _ => None,
        };
        if item.binding.is_some()
            && item.index_path.len() == 1
            && let (Some(story_kind), Some(item_kind)) = (story_kind, item_kind)
        {
            self.validate_owned_item(py, item)?;
            let part_name = item.story.part_name.clone();
            let owner_index = item.story.owner_index;
            let index_path = item.index_path.clone();
            return self.scoped_replacement(py, "Document.replace_text_at", |document| {
                document.try_replace_text_at_selector_owned(
                    story_kind,
                    &part_name,
                    owner_index,
                    item_kind,
                    &index_path,
                    old,
                    new,
                    expect,
                )
            });
        }
        let location = self.native_location(py, item)?;
        self.scoped_replacement(py, "Document.replace_text_at", |document| {
            document.try_replace_text_at_owned(&location, old, new, expect)
        })
    }

    #[pyo3(signature = (placeholder, replacement, *, expect = None))]
    fn try_replace_text<'py>(
        &mut self,
        py: Python<'py>,
        placeholder: &str,
        replacement: &str,
        expect: Option<usize>,
    ) -> PyResult<Bound<'py, PyInt>> {
        if self.snapshot_scope.is_none() {
            if expect.is_none() && replacement.chars().any(|character| {
                !matches!(character as u32, 0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF)
            }) {
                // The native singular API owns this exact upfront error and ordering.
                oxoffice_docx::Document::new().try_replace_text(placeholder, replacement)
                    .map_err(|error| docx_to_pyerr(py, error))?;
            }
            let Some((candidate, counts)) =
                self.dormant_replacements(py, &[(placeholder, replacement, expect)], false, None)?
            else {
                return Ok(PyInt::new(py, 0));
            };
            let result = PyInt::new(py, counts[0]);
            self.commit_dormant_replacement(candidate, "Document.try_replace_text")?;
            return Ok(result);
        }
        if expect.is_none() {
            return self
                .counted_mutation(py, "Document.try_replace_text", |document| {
                    document.try_replace_text(placeholder, replacement)
                })
                .map(|count| PyInt::new(py, count));
        }
        let (candidate, counts) =
            self.expected_replacements(py, &[(placeholder, replacement, expect)], false, None)?;
        if counts[0] > 0 {
            let prepared = self.prepare_word_revision(
                py,
                &candidate,
                "Document.try_replace_text",
                crate::document::WordPublication::Conservative,
            )?;
            self.commit_word_revision(prepared)?;
            self.install_inner(candidate);
        }
        Ok(PyInt::new(py, counts[0]))
    }

    #[pyo3(signature = (pairs, *, allow_unreached = false))]
    fn replace_all<'py>(
        &mut self,
        py: Python<'py>,
        pairs: &Bound<'py, PyAny>,
        allow_unreached: bool,
    ) -> PyResult<Bound<'py, PyTuple>> {
        let check = Some(oxoffice_docx::ReplaceOptions { allow_unreached });
        let mut owned = Vec::new();
        for (index, pair) in pairs.try_iter()?.enumerate() {
            let pair = pair?;
            let parsed = match pair.extract::<(String, String)>() {
                Ok((placeholder, replacement)) => (placeholder, replacement, None),
                Err(_) => pair
                    .extract::<(String, String, Option<usize>)>()
                    .map_err(|_| {
                        PyTypeError::new_err(format!(
                            "pair {index} must be an (old, new) or (old, new, expected) tuple, \
                         where expected is a nonnegative int or None"
                        ))
                    })?,
            };
            owned.push(parsed);
        }
        let pairs = owned
            .iter()
            .map(|(placeholder, replacement, expected)| {
                (placeholder.as_str(), replacement.as_str(), *expected)
            })
            .collect::<Vec<_>>();
        if self.snapshot_scope.is_none() {
            let changed = self.dormant_replacements(py, &pairs, true, check)?;
            let (candidate, counts) = match changed {
                Some((candidate, counts)) => (Some(candidate), counts),
                None => (None, vec![0; pairs.len()]),
            };
            let result = PyTuple::new(py, counts)?;
            if let Some(candidate) = candidate {
                self.commit_dormant_replacement(candidate, "Document.replace_all")?;
            }
            return Ok(result);
        }
        let (candidate, counts) = self.expected_replacements(py, &pairs, true, check)?;
        let changed = counts.iter().any(|count| *count > 0);
        let result = PyTuple::new(py, counts)?;
        if changed {
            let prepared = self.prepare_word_revision(
                py,
                &candidate,
                "Document.replace_all",
                crate::document::WordPublication::Conservative,
            )?;
            self.commit_word_revision(prepared)?;
            self.install_inner(candidate);
        }
        Ok(result)
    }

    fn replace_all_regex_expected<'py>(
        &mut self,
        py: Python<'py>,
        pairs: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyTuple>> {
        let mut owned = Vec::new();
        for (index, pair) in pairs.try_iter()?.enumerate() {
            let pair = pair?;
            let parsed = match pair.extract::<(String, String)>() {
                Ok((placeholder, replacement)) => (placeholder, replacement, None),
                Err(_) => pair
                    .extract::<(String, String, Option<usize>)>()
                    .map_err(|_| {
                        PyTypeError::new_err(format!(
                            "pair {index} must be an (old, new) or (old, new, expected) tuple, \
                         where expected is a nonnegative int or None"
                        ))
                    })?,
            };
            owned.push(parsed);
        }
        let pairs = owned
            .iter()
            .map(|(placeholder, replacement, expected)| {
                (placeholder.as_str(), replacement.as_str(), *expected)
            })
            .collect::<Vec<_>>();
        let (candidate, counts) = {
            let mut candidate = self.inner.clone_for_staging();
            let counts = py
                .detach(|| candidate.try_replace_all_regex_expected(&pairs))
                .map_err(|error| docx_to_pyerr(py, error))?
                .map_err(|mismatch| crate::replacement_count_to_pyerr(py, &mismatch, true))?;
            (candidate, counts)
        };

        let result = PyTuple::new(py, counts)?;
        self.publish_api_candidate(py, candidate, "Document.replace_all_regex_expected")?;

        Ok(result)
    }

    fn replace_all_regex(
        &mut self,
        py: Python<'_>,
        patterns: Vec<(String, String)>,
    ) -> PyResult<usize> {
        self.counted_mutation(py, "Document.replace_all_regex", |document| {
            document.replace_all_regex(&patterns)
        })
    }

    #[pyo3(signature = (
        *,
        now = None,
        file_name = None,
        file_path = None,
        merge_fields = None,
        included_text = None,
        merge_record_number = None,
        merge_sequence_number = None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn update_fields(
        &mut self,
        py: Python<'_>,
        now: Option<&Bound<'_, PyAny>>,
        file_name: Option<String>,
        file_path: Option<String>,
        merge_fields: Option<BTreeMap<String, String>>,
        included_text: Option<BTreeMap<String, String>>,
        merge_record_number: Option<u32>,
        merge_sequence_number: Option<u32>,
    ) -> PyResult<usize> {
        // Read field by field: the abi3 build has no datetime accessors, and
        // the wall-clock values are used as given.
        let now = now
            .map(|now| {
                Ok::<_, PyErr>(oxoffice_docx::FieldDateTime {
                    year: now.getattr("year")?.extract()?,
                    month: now.getattr("month")?.extract()?,
                    day: now.getattr("day")?.extract()?,
                    hour: now.getattr("hour")?.extract()?,
                    minute: now.getattr("minute")?.extract()?,
                    second: now.getattr("second")?.extract()?,
                })
            })
            .transpose()?;
        let context = oxoffice_docx::FieldEvaluationContext {
            now,
            file_name,
            file_path,
            merge_fields: merge_fields.unwrap_or_default(),
            included_text: included_text.unwrap_or_default(),
            merge_record_number,
            merge_sequence_number,
        };
        self.counted_mutation(py, "Document.update_fields", |document| {
            document.update_fields(&context)
        })
    }

    #[pyo3(signature = (
        *,
        now = None,
        file_name = None,
        file_path = None,
        merge_fields = None,
        included_text = None,
        merge_record_number = None,
        merge_sequence_number = None,
    ))]
    #[allow(clippy::too_many_arguments)]
    fn update_all_fields(
        &mut self,
        py: Python<'_>,
        now: Option<&Bound<'_, PyAny>>,
        file_name: Option<String>,
        file_path: Option<String>,
        merge_fields: Option<BTreeMap<String, String>>,
        included_text: Option<BTreeMap<String, String>>,
        merge_record_number: Option<u32>,
        merge_sequence_number: Option<u32>,
    ) -> PyResult<Py<PyFieldsUpdateReport>> {
        // Read field by field: the abi3 build has no datetime accessors, and
        // the wall-clock values are used as given.
        let now = now
            .map(|now| {
                Ok::<_, PyErr>(oxoffice_docx::FieldDateTime {
                    year: now.getattr("year")?.extract()?,
                    month: now.getattr("month")?.extract()?,
                    day: now.getattr("day")?.extract()?,
                    hour: now.getattr("hour")?.extract()?,
                    minute: now.getattr("minute")?.extract()?,
                    second: now.getattr("second")?.extract()?,
                })
            })
            .transpose()?;
        let context = oxoffice_docx::FieldEvaluationContext {
            now,
            file_name,
            file_path,
            merge_fields: merge_fields.unwrap_or_default(),
            included_text: included_text.unwrap_or_default(),
            merge_record_number,
            merge_sequence_number,
        };
        let mut candidate = self.inner.clone_for_staging();
        let report = py
            .detach(|| candidate.update_all_fields(&context))
            .map_err(|error| docx_to_pyerr(py, error))?;

        // Every returned object, including nested outcomes and tuples, exists
        // before publishing. Returning this owned Py object needs no allocation.
        let json_loads = py.import("json")?.getattr("loads")?;
        let evaluations = report
            .evaluations
            .iter()
            .map(|evaluation| {
                let outcome = json_loads
                    .call1((evaluation.to_json_value()["outcome"].to_string(),))?
                    .cast_into::<PyDict>()?
                    .unbind();
                Py::new(
                    py,
                    PyFieldEvaluation {
                        field_index: evaluation.field_index,
                        instruction: evaluation.instruction.clone(),
                        cached_result: evaluation.cached_result.clone(),
                        outcome,
                    },
                )
            })
            .collect::<PyResult<Vec<_>>>()?;
        let evaluations = PyTuple::new(py, evaluations)?.unbind();
        let diagnostics = PyTuple::new(py, &report.diagnostics)?.unbind();
        let layout = Py::new(
            py,
            PyLayoutBackedFieldUpdateReport {
                page_fields: report.layout.page_fields,
                num_pages_fields: report.layout.num_pages_fields,
                page_reference_fields: report.layout.page_reference_fields,
                section_fields: report.layout.section_fields,
                section_pages_fields: report.layout.section_pages_fields,
                diagnostics: report.layout.diagnostics,
            },
        )?;
        let result = Py::new(
            py,
            PyFieldsUpdateReport {
                ordinary_updated: report.ordinary_updated,
                layout,
                evaluations,
                diagnostics,
                total_updated: report.total_updated,
                saved_cache_scope: report.saved_cache_scope.to_owned(),
            },
        )?;
        self.publish_api_candidate(py, candidate, "Document.update_all_fields")?;

        Ok(result)
    }

    fn update_page_fields(&mut self, py: Python<'_>) -> PyResult<usize> {
        let mut candidate = self.inner.clone_for_staging();
        let updated = py
            .detach(|| candidate.update_page_fields())
            .map_err(|error| docx_to_pyerr(py, error))?;
        if updated != 0 {
            let prepared = self.prepare_word_revision(
                py,
                &candidate,
                "Document.update_page_fields",
                crate::document::WordPublication::Conservative,
            )?;
            self.commit_word_revision(prepared)?;
            self.install_inner(candidate);
        }
        Ok(updated)
    }

    fn update_layout_backed_fields(
        &mut self,
        py: Python<'_>,
    ) -> PyResult<Py<PyLayoutBackedFieldUpdateReport>> {
        let mut candidate = self.inner.clone_for_staging();
        let report = py
            .detach(|| candidate.update_layout_backed_fields())
            .map_err(|error| docx_to_pyerr(py, error))?;
        let changed = report.updated_count() != 0;
        let result = Py::new(
            py,
            PyLayoutBackedFieldUpdateReport {
                page_fields: report.page_fields,
                num_pages_fields: report.num_pages_fields,
                page_reference_fields: report.page_reference_fields,
                section_fields: report.section_fields,
                section_pages_fields: report.section_pages_fields,
                diagnostics: report.diagnostics,
            },
        )?;
        if changed {
            let prepared = self.prepare_word_revision(
                py,
                &candidate,
                "Document.update_layout_backed_fields",
                crate::document::WordPublication::Conservative,
            )?;
            self.commit_word_revision(prepared)?;
            self.install_inner(candidate);
        }
        Ok(result)
    }

    #[pyo3(signature = (index, max_level = 3))]
    fn insert_toc(&mut self, py: Python<'_>, index: usize, max_level: u32) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        if index > candidate.content_count() {
            return Err(PyIndexError::new_err("content index out of range"));
        }
        if !(1..=9).contains(&max_level) {
            return Err(PyValueError::new_err("max_level must be between 1 and 9"));
        }
        candidate
            .insert_toc(index, max_level)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.insert_toc",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(())
    }

    #[getter]
    fn paragraphs(slf: Py<Self>, py: Python<'_>) -> PyResult<Py<PyParagraphCollection>> {
        slf.borrow_mut(py).ensure_word_authority(py)?;
        Py::new(py, PyParagraphCollection::new(slf))
    }

    #[getter]
    fn tables(slf: Py<Self>, py: Python<'_>) -> PyResult<Py<PyTableCollection>> {
        slf.borrow_mut(py).ensure_word_authority(py)?;
        Py::new(py, PyTableCollection::new(slf))
    }

    #[pyo3(signature = (text = "", style = None))]
    fn add_paragraph(
        slf: Py<Self>,
        py: Python<'_>,
        text: &str,
        style: Option<&str>,
    ) -> PyResult<Py<PyParagraph>> {
        Self::append_paragraph_candidate(slf, py, text, style, false)
    }

    #[pyo3(signature = (text = "", level = 1))]
    fn add_heading(
        slf: Py<Self>,
        py: Python<'_>,
        text: &str,
        #[pyo3(from_py_with = checked_heading_level)] level: i64,
    ) -> PyResult<Py<PyParagraph>> {
        if !(0..=9).contains(&level) {
            return Err(PyValueError::new_err(
                "heading level must be from 0 through 9",
            ));
        }
        let style = if level == 0 {
            "Title".to_owned()
        } else {
            format!("Heading{level}")
        };
        Self::append_paragraph_candidate(slf, py, text, Some(&style), false)
    }

    fn add_page_break(slf: Py<Self>, py: Python<'_>) -> PyResult<Py<PyParagraph>> {
        Self::append_paragraph_candidate(slf, py, "", None, true)
    }

    #[pyo3(signature = (start_type = None))]
    fn add_section(&self, start_type: Option<&Bound<'_, PyAny>>) -> PyResult<()> {
        let _ = start_type;
        Err(pyo3::exceptions::PyNotImplementedError::new_err(
            "use document.insert_section(len(document.sections)) and document.update_section(index, break_type=...); existing native insertion copies geometry only",
        ))
    }

    #[pyo3(signature = (rows, cols, style = None))]
    fn add_table(
        slf: Py<Self>,
        py: Python<'_>,
        rows: usize,
        cols: usize,
        style: Option<&str>,
    ) -> PyResult<Py<PyTable>> {
        let mut document = slf.borrow_mut(py);
        let index = document.inner.table_count();
        document.refuse_unsavable_body_edit(py)?;
        // A style may add its definition, so only a plain table splices.
        if style.is_none() {
            let appended = oxoffice_docx::Document::detached_table(rows, cols);
            let slot = document.inner.content_count();
            if let Ok((xml, snapshots)) = document.inner.detached_table_owner_snapshots(&appended)
                && let Some(plan) = document.plan_body_splice(
                    py,
                    BodySplice {
                        slot,
                        removed: 0,
                        inserted: Some(SplicedBodyItem::Table(xml, snapshots)),
                    },
                )?
            {
                // Every fallible step precedes the in-place native append.
                let publication = crate::document::WordPublication::AppendBody;
                let prepared =
                    document.prepare_body_splice(plan, "Document.add_table", &publication)?;
                let path = document
                    .revisions
                    .capture_prepared(&prepared.revision, smallvec![PathSeg::Body(index)])
                    .map_err(revision_to_pyerr)?;
                let result = Py::new(
                    py,
                    PyTable::new(slf.clone_ref(py), path)
                        .bind_prepared_splice(&document, &prepared)?,
                )?;
                let savable = document.inner.unsavable_body_edit().is_none();
                document.inner.add_table(rows, cols);
                debug_assert_eq!(prepared.plan.table, index);
                document.commit_body_splice(prepared, &publication)?;
                document
                    .inner
                    .keep_body_savable_after_typed_insert(slot, savable);
                return Ok(result);
            }
        }
        let mut candidate = document.inner.clone_for_staging();
        let style = style
            .map(|value| crate::table::checked_table_style(py, &mut candidate, value))
            .transpose()?;
        let mut table = candidate.add_table(rows, cols);
        if let Some(style) = style {
            table.set_style(&style);
        }
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "Document.add_table",
            crate::document::WordPublication::AppendBody,
        )?;
        let path = document
            .revisions
            .capture_prepared(&prepared.revision, smallvec![PathSeg::Body(index)])
            .map_err(revision_to_pyerr)?;
        let result = Py::new(
            py,
            PyTable::new(slf.clone_ref(py), path)
                .bind_prepared(py, &document, &prepared, &candidate, None)?,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        Ok(result)
    }

    fn remove_content(&mut self, py: Python<'_>, index: usize) -> PyResult<bool> {
        // A paragraph that ends a section is not removable in place: its
        // removal merges two sections, which the kept section owners would
        // not follow.
        if self.inner.plain_direct_paragraph(index)
            && self.inner.plain_removable_paragraph(index)
            && self.inner.block_insertion_index(index) == index
            && let Some((range, revision)) = self.prepare_tail_body_removal(index)?
        {
            #[cfg(debug_assertions)]
            let before = (self.inner.clone_for_staging(), self.owners.clone());
            let removed = py
                .detach(|| self.inner.try_remove_content(index))
                .map_err(|error| docx_to_pyerr(py, error))?;
            if removed {
                self.commit_tail_body_removal(index, range, revision)?;
                // A run born locally sits after the owners of later blocks,
                // so the inventory is compared in any order.
                #[cfg(debug_assertions)]
                {
                    self.assert_word_inventory_rebuilds(false);
                    assert_kept_owners_name_their_elements(
                        (&before.0, &before.1),
                        (&self.inner, &self.owners),
                        None,
                    );
                }
            }
            return Ok(removed);
        }
        if let Some(plan) = self.plan_body_splice(
            py,
            BodySplice {
                slot: index,
                removed: 1,
                inserted: None,
            },
        )? {
            let publication = crate::document::WordPublication::BodySplice;
            let prepared =
                self.prepare_body_splice(plan, "Document.remove_content", &publication)?;
            let removed = py
                .detach(|| self.inner.try_remove_content(index))
                .map_err(|error| docx_to_pyerr(py, error))?;
            if removed {
                self.commit_body_splice(prepared, &publication)?;
            }
            return Ok(removed);
        }
        let mut candidate = self.inner.clone_for_staging();
        let removed = py
            .detach(|| candidate.try_remove_content(index))
            .map_err(|error| docx_to_pyerr(py, error))?;
        if removed {
            let prepared = self.prepare_word_revision(
                py,
                &candidate,
                "Document.remove_content",
                crate::document::WordPublication::Conservative,
            )?;
            self.commit_word_revision(prepared)?;
            self.install_inner(candidate);
        }
        Ok(removed)
    }

    fn remove_content_range(
        &mut self,
        py: Python<'_>,
        start: usize,
        end: usize,
    ) -> PyResult<usize> {
        let len = self.inner.content_count();
        if start > end || end > len {
            return Err(docx_to_pyerr(
                py,
                oxoffice_docx::Error::Other("content range is reversed or out of range".to_owned()),
            ));
        }
        if start == end {
            return Ok(0);
        }
        if let Some(plan) = self.plan_body_splice(
            py,
            BodySplice {
                slot: start,
                removed: end - start,
                inserted: None,
            },
        )? {
            let publication = crate::document::WordPublication::BodySplice;
            let prepared =
                self.prepare_body_splice(plan, "Document.remove_content_range", &publication)?;
            let removed = py
                .detach(|| self.inner.remove_content_range(start, end))
                .map_err(|error| docx_to_pyerr(py, error))?;
            self.commit_body_splice(prepared, &publication)?;
            return Ok(removed);
        }
        let mut candidate = self.inner.clone_for_staging();
        let removed = py
            .detach(|| candidate.remove_content_range(start, end))
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = self.prepare_word_revision(
            py,
            &candidate,
            "Document.remove_content_range",
            crate::document::WordPublication::Conservative,
        )?;
        self.commit_word_revision(prepared)?;
        self.install_inner(candidate);
        Ok(removed)
    }

    fn find_content_index(
        slf: Py<Self>,
        py: Python<'_>,
        content: &Bound<'_, PyAny>,
    ) -> PyResult<usize> {
        if let Ok(text) = content.extract::<String>() {
            return slf
                .borrow(py)
                .inner
                .find_content_index(&text)
                .ok_or_else(|| PyValueError::new_err("text was not found in body content"));
        }
        Self::direct_content_index(&slf, py, content, "content")
    }

    fn find_content_indices<'py>(
        &self,
        py: Python<'py>,
        text: &str,
    ) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, self.inner.find_content_indices(text))
    }

    #[pyo3(signature = (index, text, style = None))]
    pub(crate) fn insert_paragraph(
        slf: Py<Self>,
        py: Python<'_>,
        index: usize,
        text: &str,
        style: Option<&str>,
    ) -> PyResult<Py<PyParagraph>> {
        let result = {
            let mut document = slf.borrow_mut(py);
            if index > document.inner.content_count() {
                return Err(PyIndexError::new_err("content index out of range"));
            }
            // The paragraph goes before the bookmark starts that lead the
            // block at `index`, as the native insertion places it.
            let index = document.inner.block_insertion_index(index);
            let style = style
                .map(|value| {
                    style_id_of_type(&document.inner, value, oxoffice_docx::StyleType::Paragraph)
                })
                .transpose()?;
            document.refuse_non_xml_paragraph_text(py, text)?;
            document.refuse_unsavable_body_edit(py)?;
            // The same edit builds the paragraph read for the inventory and
            // the paragraph placed.
            let edit = |paragraph: &mut oxoffice_docx::Paragraph<'_>| {
                if let Some(style) = &style {
                    paragraph.set_style(style);
                }
            };
            let mut inserted = oxoffice_docx::Document::detached_text_paragraph(text);
            edit(&mut oxoffice_docx::Paragraph::detached(&mut inserted));
            let splice = document.inner.direct_body_paragraph_facts(&inserted);
            if let Ok(Some(facts)) = splice
                && let Some(plan) = document.plan_body_splice(
                    py,
                    BodySplice {
                        slot: index,
                        removed: 0,
                        inserted: Some(SplicedBodyItem::Paragraph(facts.0, facts.1)),
                    },
                )?
            {
                // Every fallible step precedes the in-place native insertion.
                let publication = crate::document::WordPublication::BodySplice;
                let prepared = document.prepare_body_splice(
                    plan,
                    "Document.insert_paragraph",
                    &publication,
                )?;
                let path = document
                    .revisions
                    .capture_prepared(
                        &prepared.revision,
                        smallvec![PathSeg::Body(0), PathSeg::Para(prepared.plan.paragraph)],
                    )
                    .map_err(revision_to_pyerr)?;
                let result = Py::new(
                    py,
                    PyParagraph::new(slf.clone_ref(py), path)
                        .bind_prepared_splice(&document, &prepared)?,
                )?;
                let savable = document.inner.unsavable_body_edit().is_none();
                edit(&mut document.inner.insert_paragraph(index, text));
                debug_assert_eq!(
                    document.inner.paragraph_index_of_content(index),
                    Some(prepared.plan.paragraph)
                );
                document.commit_body_splice(prepared, &publication)?;
                document
                    .inner
                    .keep_body_savable_after_typed_insert(index, savable);
                return Ok(result);
            }
            let mut candidate = document.inner.clone_for_staging();
            edit(&mut candidate.insert_paragraph(index, text));
            let paragraph = candidate
                .paragraph_index_of_content(index)
                .expect("an inserted body paragraph has a paragraph index");
            let prepared = document.prepare_word_revision(
                py,
                &candidate,
                "Document.insert_paragraph",
                crate::document::WordPublication::Conservative,
            )?;
            let path = document
                .revisions
                .capture_prepared(
                    &prepared.revision,
                    smallvec![PathSeg::Body(0), PathSeg::Para(paragraph)],
                )
                .map_err(revision_to_pyerr)?;
            let result = Py::new(
                py,
                PyParagraph::new(slf.clone_ref(py), path)
                    .bind_prepared(py, &document, &prepared, &candidate, None)?,
            )?;
            document.commit_word_revision(prepared)?;
            document.install_inner(candidate);
            result
        };
        Ok(result)
    }

    #[pyo3(signature = (index, rows, cols))]
    fn insert_table(
        slf: Py<Self>,
        py: Python<'_>,
        index: usize,
        rows: usize,
        cols: usize,
    ) -> PyResult<Py<PyTable>> {
        let result = {
            let mut document = slf.borrow_mut(py);
            if index > document.inner.content_count() {
                return Err(PyIndexError::new_err("content index out of range"));
            }
            // The table goes before the bookmark starts that lead the block
            // at `index`, as the native insertion places it.
            let index = document.inner.block_insertion_index(index);
            document.refuse_unsavable_body_edit(py)?;
            let inserted = oxoffice_docx::Document::detached_table(rows, cols);
            if let Ok((xml, snapshots)) = document.inner.detached_table_owner_snapshots(&inserted)
                && let Some(plan) = document.plan_body_splice(
                    py,
                    BodySplice {
                        slot: index,
                        removed: 0,
                        inserted: Some(SplicedBodyItem::Table(xml, snapshots)),
                    },
                )?
            {
                // Every fallible step precedes the in-place native insertion.
                let publication = crate::document::WordPublication::BodySplice;
                let prepared =
                    document.prepare_body_splice(plan, "Document.insert_table", &publication)?;
                let path = document
                    .revisions
                    .capture_prepared(
                        &prepared.revision,
                        smallvec![PathSeg::Body(prepared.plan.table)],
                    )
                    .map_err(revision_to_pyerr)?;
                let result = Py::new(
                    py,
                    PyTable::new(slf.clone_ref(py), path)
                        .bind_prepared_splice(&document, &prepared)?,
                )?;
                let savable = document.inner.unsavable_body_edit().is_none();
                document.inner.insert_table(index, rows, cols);
                debug_assert_eq!(
                    document.inner.content_index_of_table(prepared.plan.table),
                    Some(index)
                );
                document.commit_body_splice(prepared, &publication)?;
                document
                    .inner
                    .keep_body_savable_after_typed_insert(index, savable);
                return Ok(result);
            }
            let mut candidate = document.inner.clone_for_staging();
            candidate.insert_table(index, rows, cols);
            // Preserve the direct body table ordinal while capturing its physical owner.
            let table = (0..candidate.table_count())
                .find(|table| candidate.content_index_of_table(*table) == Some(index))
                .expect("an inserted body table has a table index");
            let prepared = document.prepare_word_revision(
                py,
                &candidate,
                "Document.insert_table",
                crate::document::WordPublication::Conservative,
            )?;
            let path = document
                .revisions
                .capture_prepared(&prepared.revision, smallvec![PathSeg::Body(table)])
                .map_err(revision_to_pyerr)?;
            let result = Py::new(
                py,
                PyTable::new(slf.clone_ref(py), path)
                    .bind_prepared(py, &document, &prepared, &candidate, None)?,
            )?;
            document.commit_word_revision(prepared)?;
            document.install_inner(candidate);
            result
        };
        Ok(result)
    }

    fn pop_content(
        slf: Py<Self>,
        py: Python<'_>,
        index: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyContentFragment>> {
        let location = if let Ok(item) = index.cast::<PyStoryItem>() {
            slf.borrow(py).native_location(py, &item.borrow())?
        } else {
            let index = body_index_argument(index, "index must be an int or a StoryItem")?;
            let location = slf.borrow(py).body_location(py, index)?;
            if index == slf.borrow(py).inner.content_count() {
                return Err(PyIndexError::new_err("content index out of range"));
            }
            location
        };
        let mut document = slf.borrow_mut(py);
        let mut candidate = document.inner.clone_for_staging();
        let fragment = candidate
            .remove_content_at(&location)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "Document.pop_content",
            crate::document::WordPublication::Conservative,
        )?;
        let result = Py::new(py, PyContentFragment { inner: fragment })?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        Ok(result)
    }

    fn insert_content(
        slf: Py<Self>,
        py: Python<'_>,
        destination: &Bound<'_, PyAny>,
        fragment: PyRef<'_, PyContentFragment>,
    ) -> PyResult<()> {
        // A story item names the boundary before it, and a story its end.
        let location = if let Ok(item) = destination.cast::<PyStoryItem>() {
            slf.borrow(py).native_location(py, &item.borrow())?
        } else if let Ok(story) = destination.cast::<PyStory>() {
            oxoffice_docx::ContentLocation::end(slf.borrow(py).native_story(py, &story.borrow())?)
        } else {
            let destination = body_index_argument(
                destination,
                "destination must be an int, a StoryItem or a Story",
            )?;
            slf.borrow(py).body_location(py, destination)?
        };
        let fragment = fragment.inner.clone();
        let mut document = slf.borrow_mut(py);
        let mut candidate = document.inner.clone_for_staging();
        candidate
            .insert_content(&location, fragment)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "Document.insert_content",
            crate::document::WordPublication::Conservative,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        Ok(())
    }

    fn clone_content(
        slf: Py<Self>,
        py: Python<'_>,
        source: &Bound<'_, PyAny>,
        destination: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let source_index = Self::direct_content_index(&slf, py, source, "source")?;
        let destination = destination
            .extract::<usize>()
            .map_err(|_| PyTypeError::new_err("destination must be a direct body index integer"))?;
        let (source, destination) = {
            let document = slf.borrow(py);
            let mut locations = document.body_locations(py, &[source_index, destination])?;
            let destination = locations.pop().expect("two requested body locations");
            let source = locations.pop().expect("two requested body locations");
            (source, destination)
        };
        let mut document = slf.borrow_mut(py);
        let mut candidate = document.inner.clone_for_staging();
        candidate
            .clone_content(&source, &destination)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "Document.clone_content",
            crate::document::WordPublication::Conservative,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        Ok(())
    }

    fn move_content(
        slf: Py<Self>,
        py: Python<'_>,
        source: &Bound<'_, PyAny>,
        destination: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        let source_index = Self::direct_content_index(&slf, py, source, "source")?;
        let destination = destination
            .extract::<usize>()
            .map_err(|_| PyTypeError::new_err("destination must be a direct body index integer"))?;
        let (source, destination) = {
            let document = slf.borrow(py);
            let mut locations = document.body_locations(py, &[source_index, destination])?;
            let destination = locations.pop().expect("two requested body locations");
            let source = locations.pop().expect("two requested body locations");
            (source, destination)
        };
        let mut document = slf.borrow_mut(py);
        let mut candidate = document.inner.clone_for_staging();
        candidate
            .move_content(&source, &destination)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "Document.move_content",
            crate::document::WordPublication::Conservative,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        Ok(())
    }

    #[getter]
    fn fonts(&self, py: Python<'_>) -> PyResult<Py<PyTuple>> {
        let fonts = self.inner.try_fonts().map_err(|e| docx_to_pyerr(py, e))?;
        let mut snapshots = Vec::new();
        snapshots.try_reserve_exact(fonts.len()).map_err(|_| {
            pyo3::exceptions::PyMemoryError::new_err("font listing allocation failed")
        })?;
        for font in fonts {
            snapshots.push(PyFontDefinition::snapshot(py, font)?);
        }
        Ok(PyTuple::new(py, snapshots)?.unbind())
    }
    fn set_font(&mut self, py: Python<'_>, font: PyRef<'_, PyFontDefinition>) -> PyResult<()> {
        let native = descriptive_font(py, &font, "set_font")?;
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .set_font(native)
            .map_err(|e| docx_to_pyerr(py, e))?;
        self.publish_api_candidate(py, candidate, "Document.set_font")
    }
    fn set_font_at(
        &mut self,
        py: Python<'_>,
        index: usize,
        font: PyRef<'_, PyFontDefinition>,
    ) -> PyResult<()> {
        let native = descriptive_font(py, &font, "set_font_at")?;
        self.check_font_index(py, index)?;
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .set_font_at(index, native)
            .map_err(|e| docx_to_pyerr(py, e))?;
        self.publish_api_candidate(py, candidate, "Document.set_font_at")
    }
    fn remove_font(
        &mut self,
        py: Python<'_>,
        name: &str,
    ) -> PyResult<Option<Py<PyFontDefinition>>> {
        let mut candidate = self.inner.clone_for_staging();
        let removed = candidate
            .remove_font(name)
            .map_err(|e| docx_to_pyerr(py, e))?
            .map(|font| PyFontDefinition::snapshot(py, font))
            .transpose()?;
        self.publish_api_candidate(py, candidate, "Document.remove_font")?;
        Ok(removed)
    }
    fn remove_font_at(&mut self, py: Python<'_>, index: usize) -> PyResult<Py<PyFontDefinition>> {
        self.check_font_index(py, index)?;
        let mut candidate = self.inner.clone_for_staging();
        let removed = candidate
            .remove_font_at(index)
            .map_err(|e| docx_to_pyerr(py, e))?;
        let removed = PyFontDefinition::snapshot(py, removed)?;
        self.publish_api_candidate(py, candidate, "Document.remove_font_at")?;
        Ok(removed)
    }
    #[pyo3(signature = (name, kind="regular"))]
    fn remove_embedded_font(
        &mut self,
        py: Python<'_>,
        name: &str,
        kind: &str,
    ) -> PyResult<Option<Py<PyEmbeddedFont>>> {
        let kind = checked_font_kind(kind)?;
        let mut candidate = self.inner.clone_for_staging();
        let removed = candidate
            .remove_embedded_font(name, kind)
            .map_err(|e| docx_to_pyerr(py, e))?
            .map(|font| PyEmbeddedFont::snapshot(py, font))
            .transpose()?;
        self.publish_api_candidate(py, candidate, "Document.remove_embedded_font")?;
        Ok(removed)
    }
    #[pyo3(signature = (index, kind="regular"))]
    fn remove_embedded_font_at(
        &mut self,
        py: Python<'_>,
        index: usize,
        kind: &str,
    ) -> PyResult<Option<Py<PyEmbeddedFont>>> {
        let kind = checked_font_kind(kind)?;
        self.check_font_index(py, index)?;
        let mut candidate = self.inner.clone_for_staging();
        let removed = candidate
            .remove_embedded_font_at(index, kind)
            .map_err(|e| docx_to_pyerr(py, e))?
            .map(|font| PyEmbeddedFont::snapshot(py, font))
            .transpose()?;
        self.publish_api_candidate(py, candidate, "Document.remove_embedded_font_at")?;
        Ok(removed)
    }
    #[pyo3(signature = (name, font, kind="regular", *, license, font_key=None, subsetted=false))]
    #[allow(clippy::too_many_arguments)] // Explicit operands match the native font authoring contract.
    fn embed_font(
        &mut self,
        py: Python<'_>,
        name: &str,
        font: &Bound<'_, PyAny>,
        kind: &str,
        license: PyRef<'_, PyFontEmbeddingLicense>,
        font_key: Option<String>,
        subsetted: bool,
    ) -> PyResult<()> {
        let font = embedded_face(py, font, kind, &license, font_key, subsetted)?;
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .embed_font(name, font)
            .map_err(|e| docx_to_pyerr(py, e))?;
        self.publish_api_candidate(py, candidate, "Document.embed_font")
    }
    #[pyo3(signature = (index, font, kind="regular", *, license, font_key=None, subsetted=false))]
    #[allow(clippy::too_many_arguments)] // Explicit operands match the native font authoring contract.
    fn embed_font_at(
        &mut self,
        py: Python<'_>,
        index: usize,
        font: &Bound<'_, PyAny>,
        kind: &str,
        license: PyRef<'_, PyFontEmbeddingLicense>,
        font_key: Option<String>,
        subsetted: bool,
    ) -> PyResult<()> {
        let font = embedded_face(py, font, kind, &license, font_key, subsetted)?;
        self.check_font_index(py, index)?;
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .embed_font_at(index, font)
            .map_err(|e| docx_to_pyerr(py, e))?;
        self.publish_api_candidate(py, candidate, "Document.embed_font_at")
    }

    #[getter]
    fn embed_true_type_fonts(&self, py: Python<'_>) -> PyResult<Option<bool>> {
        self.inner
            .embed_true_type_fonts()
            .map_err(|e| docx_to_pyerr(py, e))
    }
    #[setter]
    fn set_embed_true_type_fonts(&mut self, py: Python<'_>, value: Option<bool>) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .set_embed_true_type_fonts(value)
            .map_err(|e| docx_to_pyerr(py, e))?;
        self.publish_api_candidate(py, candidate, "Document.set_embed_true_type_fonts")
    }
    #[getter]
    fn embed_system_fonts(&self, py: Python<'_>) -> PyResult<Option<bool>> {
        self.inner
            .embed_system_fonts()
            .map_err(|e| docx_to_pyerr(py, e))
    }
    #[setter]
    fn set_embed_system_fonts(&mut self, py: Python<'_>, value: Option<bool>) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .set_embed_system_fonts(value)
            .map_err(|e| docx_to_pyerr(py, e))?;
        self.publish_api_candidate(py, candidate, "Document.set_embed_system_fonts")
    }
    #[getter]
    fn save_subset_fonts(&self, py: Python<'_>) -> PyResult<Option<bool>> {
        self.inner
            .save_subset_fonts()
            .map_err(|e| docx_to_pyerr(py, e))
    }
    #[setter]
    fn set_save_subset_fonts(&mut self, py: Python<'_>, value: Option<bool>) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .set_save_subset_fonts(value)
            .map_err(|e| docx_to_pyerr(py, e))?;
        self.publish_api_candidate(py, candidate, "Document.set_save_subset_fonts")
    }

    fn set_mail_merge_package(
        &mut self,
        py: Python<'_>,
        options: &Bound<'_, PyMailMergePackageOptions>,
    ) -> PyResult<()> {
        let mut candidate = self.inner.clone_for_staging();
        candidate
            .set_mail_merge_package(options.borrow().inner.clone())
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.publish_api_candidate(py, candidate, "Document.set_mail_merge_package")
    }

    fn mail_merge_package(&self, py: Python<'_>) -> PyResult<Option<PyMailMergePackageOptions>> {
        Ok(self
            .inner
            .mail_merge_package()
            .map_err(|error| docx_to_pyerr(py, error))?
            .map(|inner| PyMailMergePackageOptions { inner }))
    }

    fn remove_mail_merge_package(&mut self, py: Python<'_>) -> PyResult<bool> {
        let mut candidate = self.inner.clone_for_staging();
        let removed = candidate
            .remove_mail_merge_package()
            .map_err(|error| docx_to_pyerr(py, error))?;
        self.publish_api_candidate(py, candidate, "Document.remove_mail_merge_package")?;
        Ok(removed)
    }

    #[pyo3(
        signature = (*, policy = "offline", external_records = None),
        text_signature = "($self, *, policy='offline', external_records=None)"
    )]
    fn prepare_mail_merge_data(
        &self,
        py: Python<'_>,
        policy: &str,
        external_records: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyMailMergePreparedData> {
        let native_policy = match policy {
            "offline" => oxoffice_docx::MailMergeSourcePolicy::Offline,
            "caller_supplied" => oxoffice_docx::MailMergeSourcePolicy::CallerSupplied,
            _ => {
                return Err(PyValueError::new_err(
                    "policy must be 'offline' or 'caller_supplied'",
                ));
            }
        };
        let prepared = if let Some(external_records) = external_records {
            require_carrier_sequence(external_records, "external_records")?;
            let probe = self
                .inner
                .prepare_mail_merge_data(native_policy, Some(&[]))
                .map_err(|error| docx_to_pyerr(py, error))?;
            if !probe.diagnostics.is_empty() {
                probe
            } else {
                let rows = copied_external_rows(py, external_records)?;
                self.inner
                    .prepare_mail_merge_data(native_policy, Some(&rows))
                    .map_err(|error| docx_to_pyerr(py, error))?
            }
        } else {
            self.inner
                .prepare_mail_merge_data(native_policy, None)
                .map_err(|error| docx_to_pyerr(py, error))?
        };
        Ok(PyMailMergePreparedData { inner: prepared })
    }
}
