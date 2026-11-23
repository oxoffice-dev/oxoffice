//! Section handles and snapshots, and the note policy a section takes.

use super::*;

impl PySectionHandle {
    fn validate(&self, py: Python<'_>) -> PyResult<()> {
        let document = self.document.borrow(py);
        self.path
            .validate_scopes(
                &document.revisions,
                "section",
                "Re-fetch it with document.section(index).",
            )
            .map_err(|error| crate::stale_to_pyerr(py, error))?;
        if !document.owners.iter().any(|owner| {
            owner.owner.same_state(&self.owner)
                && matches!(owner.location,WordOwnerLocation::Section(index) if index==self.index)
        }) {
            return Err(PyValueError::new_err(
                "section physical owner is no longer current",
            ));
        }
        if self.index >= document.inner.section_count() {
            return Err(PyIndexError::new_err("section index out of range"));
        }
        Ok(())
    }

    fn snapshot(&self, py: Python<'_>) -> PyResult<PySection> {
        self.validate(py)?;
        Ok(section_snapshot(
            self.document
                .borrow(py)
                .inner
                .section(self.index)
                .expect("validated section"),
        ))
    }

    fn geometry(&mut self, py: Python<'_>, name: &str, value: Bound<'_, PyAny>) -> PyResult<()> {
        self.validate(py)?;
        crate::required_length(
            &value,
            &format!("SectionHandle.{name}"),
            crate::LengthUnit::Twips,
        )?;
        // python-docx's `top_margin` and the others name the same margins.
        let name = match name {
            "top_margin" => "margin_top",
            "right_margin" => "margin_right",
            "bottom_margin" => "margin_bottom",
            "left_margin" => "margin_left",
            name => name,
        };
        let value = Some(value.unbind());
        let mut document = self.document.borrow_mut(py);
        document.update_section(
            py,
            self.index,
            None,
            if name == "page_width" {
                value.as_ref().map(|value| value.bind(py).clone())
            } else {
                None
            },
            if name == "page_height" {
                value.as_ref().map(|value| value.bind(py).clone())
            } else {
                None
            },
            if name == "margin_top" {
                value.as_ref().map(|value| value.bind(py).clone())
            } else {
                None
            },
            if name == "margin_right" {
                value.as_ref().map(|value| value.bind(py).clone())
            } else {
                None
            },
            if name == "margin_bottom" {
                value.as_ref().map(|value| value.bind(py).clone())
            } else {
                None
            },
            if name == "margin_left" {
                value.as_ref().map(|value| value.bind(py).clone())
            } else {
                None
            },
            if name == "gutter" {
                value.as_ref().map(|value| value.bind(py).clone())
            } else {
                None
            },
            None,
            None,
            None,
            if name == "header_distance" {
                value.as_ref().map(|value| value.bind(py).clone())
            } else {
                None
            },
            if name == "footer_distance" {
                value.as_ref().map(|value| value.bind(py).clone())
            } else {
                None
            },
            None,
            None,
        )?;
        Ok(())
    }

    fn furniture(
        &self,
        py: Python<'_>,
        kind: oxoffice_docx::HeaderFooterKind,
        variant: oxoffice_docx::HdrFtrType,
    ) -> PyResult<PyHeaderFooter> {
        self.validate(py)?;
        Ok(PyHeaderFooter {
            document: self.document.clone_ref(py),
            index: self.index,
            kind,
            variant,
            path: self
                .document
                .borrow(py)
                .capture_owner(
                    &WordOwnerLocation::Section(self.index),
                    self.path.segs.clone(),
                    WordView::Content,
                    None,
                )?
                .0,
        })
    }
}

#[pymethods]
impl PySectionHandle {
    #[getter]
    fn element<'py>(slf: &Bound<'py, Self>) -> Bound<'py, Self> {
        slf.clone()
    }

    #[getter]
    fn xml(&self, py: Python<'_>) -> PyResult<String> {
        self.validate(py)?;
        let bytes = self
            .document
            .borrow(py)
            .inner
            .section_properties_xml(self.index)
            .map_err(|error| docx_to_pyerr(py, error))?;
        String::from_utf8(bytes).map_err(|error| PyValueError::new_err(error.to_string()))
    }

    fn replace_xml(&self, py: Python<'_>, xml: &str) -> PyResult<()> {
        self.validate(py)?;
        let mut document = self.document.borrow_mut(py);
        let mut candidate = document.inner.clone_for_staging();
        candidate
            .replace_section_properties_xml(self.index, xml)
            .map_err(|error| docx_to_pyerr(py, error))?;
        document.publish_owner_candidate(
            py,
            candidate,
            "Section.replace_xml",
            WordPublication::ElementXml(WordOwnerLocation::Section(self.index)),
        )
    }

    #[getter]
    fn page_width(&self, py: Python<'_>) -> PyResult<Option<i64>> {
        Ok(self.snapshot(py)?.page_width)
    }
    #[setter]
    fn set_page_width(&mut self, py: Python<'_>, value: Bound<'_, PyAny>) -> PyResult<()> {
        self.geometry(py, "page_width", value)
    }
    #[getter]
    fn page_height(&self, py: Python<'_>) -> PyResult<Option<i64>> {
        Ok(self.snapshot(py)?.page_height)
    }
    #[setter]
    fn set_page_height(&mut self, py: Python<'_>, value: Bound<'_, PyAny>) -> PyResult<()> {
        self.geometry(py, "page_height", value)
    }
    #[getter]
    fn margin_top(&self, py: Python<'_>) -> PyResult<Option<i64>> {
        Ok(self.snapshot(py)?.margin_top)
    }
    #[setter]
    fn set_margin_top(&mut self, py: Python<'_>, value: Bound<'_, PyAny>) -> PyResult<()> {
        self.geometry(py, "margin_top", value)
    }
    #[getter]
    fn margin_right(&self, py: Python<'_>) -> PyResult<Option<i64>> {
        Ok(self.snapshot(py)?.margin_right)
    }
    #[setter]
    fn set_margin_right(&mut self, py: Python<'_>, value: Bound<'_, PyAny>) -> PyResult<()> {
        self.geometry(py, "margin_right", value)
    }
    #[getter]
    fn margin_bottom(&self, py: Python<'_>) -> PyResult<Option<i64>> {
        Ok(self.snapshot(py)?.margin_bottom)
    }
    #[setter]
    fn set_margin_bottom(&mut self, py: Python<'_>, value: Bound<'_, PyAny>) -> PyResult<()> {
        self.geometry(py, "margin_bottom", value)
    }
    #[getter]
    fn margin_left(&self, py: Python<'_>) -> PyResult<Option<i64>> {
        Ok(self.snapshot(py)?.margin_left)
    }
    #[setter]
    fn set_margin_left(&mut self, py: Python<'_>, value: Bound<'_, PyAny>) -> PyResult<()> {
        self.geometry(py, "margin_left", value)
    }
    #[getter]
    fn top_margin(&self, py: Python<'_>) -> PyResult<Option<i64>> {
        Ok(self.snapshot(py)?.margin_top)
    }
    #[setter]
    fn set_top_margin(&mut self, py: Python<'_>, value: Bound<'_, PyAny>) -> PyResult<()> {
        self.geometry(py, "top_margin", value)
    }
    #[getter]
    fn right_margin(&self, py: Python<'_>) -> PyResult<Option<i64>> {
        Ok(self.snapshot(py)?.margin_right)
    }
    #[setter]
    fn set_right_margin(&mut self, py: Python<'_>, value: Bound<'_, PyAny>) -> PyResult<()> {
        self.geometry(py, "right_margin", value)
    }
    #[getter]
    fn bottom_margin(&self, py: Python<'_>) -> PyResult<Option<i64>> {
        Ok(self.snapshot(py)?.margin_bottom)
    }
    #[setter]
    fn set_bottom_margin(&mut self, py: Python<'_>, value: Bound<'_, PyAny>) -> PyResult<()> {
        self.geometry(py, "bottom_margin", value)
    }
    #[getter]
    fn left_margin(&self, py: Python<'_>) -> PyResult<Option<i64>> {
        Ok(self.snapshot(py)?.margin_left)
    }
    #[setter]
    fn set_left_margin(&mut self, py: Python<'_>, value: Bound<'_, PyAny>) -> PyResult<()> {
        self.geometry(py, "left_margin", value)
    }
    #[getter]
    fn gutter(&self, py: Python<'_>) -> PyResult<Option<i64>> {
        Ok(self.snapshot(py)?.gutter)
    }
    #[setter]
    fn set_gutter(&mut self, py: Python<'_>, value: Bound<'_, PyAny>) -> PyResult<()> {
        self.geometry(py, "gutter", value)
    }
    #[getter]
    fn header_distance(&self, py: Python<'_>) -> PyResult<Option<i64>> {
        Ok(self.snapshot(py)?.header_distance)
    }
    #[setter]
    fn set_header_distance(&mut self, py: Python<'_>, value: Bound<'_, PyAny>) -> PyResult<()> {
        self.geometry(py, "header_distance", value)
    }
    #[getter]
    fn footer_distance(&self, py: Python<'_>) -> PyResult<Option<i64>> {
        Ok(self.snapshot(py)?.footer_distance)
    }
    #[setter]
    fn set_footer_distance(&mut self, py: Python<'_>, value: Bound<'_, PyAny>) -> PyResult<()> {
        self.geometry(py, "footer_distance", value)
    }
    #[getter]
    fn orientation(&self, py: Python<'_>) -> PyResult<Option<String>> {
        Ok(self.snapshot(py)?.orientation)
    }
    #[setter]
    fn set_orientation(&mut self, py: Python<'_>, value: &str) -> PyResult<()> {
        self.validate(py)?;
        self.document.borrow_mut(py).update_section(
            py,
            self.index,
            Some(value),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )?;
        Ok(())
    }
    #[getter]
    fn different_first_page_header_footer(&self, py: Python<'_>) -> PyResult<bool> {
        Ok(self.snapshot(py)?.different_first_page.unwrap_or(false))
    }
    #[setter]
    fn set_different_first_page_header_footer(
        &mut self,
        py: Python<'_>,
        value: bool,
    ) -> PyResult<()> {
        self.validate(py)?;
        self.document.borrow_mut(py).update_section(
            py,
            self.index,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(value),
            None,
        )?;
        Ok(())
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
        self.validate(py)?;
        let (family, policy) = checked_note_policy(family, format, start, restart, placement)?;
        let mut document = self.document.borrow_mut(py);
        let mut candidate = document.inner.clone_for_staging();
        candidate
            .section_mut(self.index)
            .ok_or_else(|| PyIndexError::new_err("section index out of range"))?
            .set_note_policy(family, policy)
            .map_err(|error| docx_to_pyerr(py, error))?;
        candidate
            .to_bytes()
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "SectionHandle.set_note_policy",
            crate::document::WordPublication::Conservative,
        )?;
        let (next, next_owner) = document.capture_prepared_owner(
            &prepared,
            &WordOwnerLocation::Section(self.index),
            smallvec![],
            WordView::Owner,
            None,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        self.path = next;
        self.owner = next_owner;
        Ok(())
    }

    #[pyo3(signature = (edge, *, style="single", width=8, space=0, color="000000"))]
    fn set_page_border(
        &mut self,
        py: Python<'_>,
        edge: &str,
        style: &str,
        width: u32,
        space: u32,
        color: &str,
    ) -> PyResult<()> {
        self.validate(py)?;
        let mut document = self.document.borrow_mut(py);
        let mut candidate = document.inner.clone_for_staging();
        candidate
            .section_mut(self.index)
            .ok_or_else(|| PyIndexError::new_err("section index out of range"))?
            .set_page_border(edge, style, width, space, color)
            .map_err(|error| docx_to_pyerr(py, error))?;
        candidate
            .saves_as(&candidate)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "SectionHandle.set_page_border",
            crate::document::WordPublication::Conservative,
        )?;
        let (next, next_owner) = document.capture_prepared_owner(
            &prepared,
            &WordOwnerLocation::Section(self.index),
            smallvec![],
            WordView::Owner,
            None,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        self.path = next;
        self.owner = next_owner;
        Ok(())
    }

    /// Author actual DrawingML text, with styled real text in both representations.
    #[allow(clippy::too_many_arguments)] // Keep explicit checked public watermark operands.
    #[pyo3(signature = (text, *, width, height, font_size, variant="default", rotation=315.0, color="D9D9D9", font_family="Calibri", opacity=0.5))]
    fn set_text_watermark(
        &mut self,
        py: Python<'_>,
        text: &str,
        width: &Bound<'_, PyAny>,
        height: &Bound<'_, PyAny>,
        font_size: &Bound<'_, PyAny>,
        variant: &str,
        rotation: f64,
        color: &str,
        font_family: &str,
        opacity: f64,
    ) -> PyResult<()> {
        self.validate(py)?;
        let variant = match variant {
            "default" => oxoffice_docx::HdrFtrType::Default,
            "first" => oxoffice_docx::HdrFtrType::First,
            "even" => oxoffice_docx::HdrFtrType::Even,
            _ => {
                return Err(PyValueError::new_err(
                    "watermark variant must be default, first or even",
                ));
            }
        };
        let width = crate::required_length(
            width,
            "SectionHandle.set_text_watermark(width)",
            crate::LengthUnit::Emu,
        )?;
        let height = crate::required_length(
            height,
            "SectionHandle.set_text_watermark(height)",
            crate::LengthUnit::Emu,
        )?;
        let font_size = crate::required_length(
            font_size,
            "SectionHandle.set_text_watermark(font_size)",
            crate::LengthUnit::HalfPoints,
        )?;
        let mut document = self.document.borrow_mut(py);
        let mut candidate = document.inner.clone_for_staging();
        candidate
            .set_drawingml_text_watermark_for(
                self.index,
                variant,
                text,
                oxoffice_docx::TextWatermarkOptions {
                    width: oxoffice_docx::Length::emu(width),
                    height: oxoffice_docx::Length::emu(height),
                    rotation_degrees: rotation,
                    color: color.to_owned(),
                    font_family: Some(font_family.to_owned()),
                    opacity,
                },
                oxoffice_docx::Length::emu(font_size),
            )
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "SectionHandle.set_text_watermark",
            crate::document::WordPublication::Conservative,
        )?;
        let (next, next_owner) = document.capture_prepared_owner(
            &prepared,
            &WordOwnerLocation::Section(self.index),
            smallvec![],
            WordView::Owner,
            None,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        self.path = next;
        self.owner = next_owner;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)] // Explicit checked watermark operands.
    #[pyo3(signature = (data, filename, *, width, height, variant="default", rotation=0.0, opacity=0.5))]
    fn set_image_watermark(
        &mut self,
        py: Python<'_>,
        data: &[u8],
        filename: &str,
        width: &Bound<'_, PyAny>,
        height: &Bound<'_, PyAny>,
        variant: &str,
        rotation: f64,
        opacity: f64,
    ) -> PyResult<()> {
        self.validate(py)?;
        let variant = match variant {
            "default" => oxoffice_docx::HdrFtrType::Default,
            "first" => oxoffice_docx::HdrFtrType::First,
            "even" => oxoffice_docx::HdrFtrType::Even,
            _ => {
                return Err(PyValueError::new_err(
                    "watermark variant must be default, first or even",
                ));
            }
        };
        let width = crate::required_length(
            width,
            "SectionHandle.set_image_watermark(width)",
            crate::LengthUnit::Emu,
        )?;
        let height = crate::required_length(
            height,
            "SectionHandle.set_image_watermark(height)",
            crate::LengthUnit::Emu,
        )?;
        let mut document = self.document.borrow_mut(py);
        let mut candidate = document.inner.clone_for_staging();
        candidate
            .set_drawingml_image_watermark_for(
                self.index,
                variant,
                data,
                filename,
                oxoffice_docx::ImageWatermarkOptions {
                    width: oxoffice_docx::Length::emu(width),
                    height: oxoffice_docx::Length::emu(height),
                    rotation_degrees: rotation,
                    opacity,
                },
            )
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "SectionHandle.set_image_watermark",
            crate::document::WordPublication::Conservative,
        )?;
        let (next, next_owner) = document.capture_prepared_owner(
            &prepared,
            &WordOwnerLocation::Section(self.index),
            smallvec![],
            WordView::Owner,
            None,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        self.path = next;
        self.owner = next_owner;
        Ok(())
    }

    #[pyo3(signature = (*, variant="default"))]
    fn remove_watermark(&mut self, py: Python<'_>, variant: &str) -> PyResult<()> {
        self.validate(py)?;
        let variant = match variant {
            "default" => oxoffice_docx::HdrFtrType::Default,
            "first" => oxoffice_docx::HdrFtrType::First,
            "even" => oxoffice_docx::HdrFtrType::Even,
            _ => {
                return Err(PyValueError::new_err(
                    "watermark variant must be default, first or even",
                ));
            }
        };
        let mut document = self.document.borrow_mut(py);
        let mut candidate = document.inner.clone_for_staging();
        candidate
            .remove_drawingml_watermark_for(self.index, variant)
            .map_err(|error| docx_to_pyerr(py, error))?;
        let prepared = document.prepare_word_revision(
            py,
            &candidate,
            "SectionHandle.remove_watermark",
            crate::document::WordPublication::Conservative,
        )?;
        let (next, next_owner) = document.capture_prepared_owner(
            &prepared,
            &WordOwnerLocation::Section(self.index),
            smallvec![],
            WordView::Owner,
            None,
        )?;
        document.commit_word_revision(prepared)?;
        document.install_inner(candidate);
        self.path = next;
        self.owner = next_owner;
        Ok(())
    }

    #[getter]
    fn header(&self, py: Python<'_>) -> PyResult<PyHeaderFooter> {
        self.furniture(
            py,
            oxoffice_docx::HeaderFooterKind::Header,
            oxoffice_docx::HdrFtrType::Default,
        )
    }
    #[getter]
    fn first_page_header(&self, py: Python<'_>) -> PyResult<PyHeaderFooter> {
        self.furniture(
            py,
            oxoffice_docx::HeaderFooterKind::Header,
            oxoffice_docx::HdrFtrType::First,
        )
    }
    #[getter]
    fn even_page_header(&self, py: Python<'_>) -> PyResult<PyHeaderFooter> {
        self.furniture(
            py,
            oxoffice_docx::HeaderFooterKind::Header,
            oxoffice_docx::HdrFtrType::Even,
        )
    }
    #[getter]
    fn footer(&self, py: Python<'_>) -> PyResult<PyHeaderFooter> {
        self.furniture(
            py,
            oxoffice_docx::HeaderFooterKind::Footer,
            oxoffice_docx::HdrFtrType::Default,
        )
    }
    #[getter]
    fn first_page_footer(&self, py: Python<'_>) -> PyResult<PyHeaderFooter> {
        self.furniture(
            py,
            oxoffice_docx::HeaderFooterKind::Footer,
            oxoffice_docx::HdrFtrType::First,
        )
    }
    #[getter]
    fn even_page_footer(&self, py: Python<'_>) -> PyResult<PyHeaderFooter> {
        self.furniture(
            py,
            oxoffice_docx::HeaderFooterKind::Footer,
            oxoffice_docx::HdrFtrType::Even,
        )
    }
}

pub(super) fn checked_note_policy(
    family: &str,
    format: Option<&str>,
    start: Option<&Bound<'_, PyAny>>,
    restart: &str,
    placement: Option<&str>,
) -> PyResult<(oxoffice_docx::NoteFamily, oxoffice_docx::NotePolicy)> {
    let family = match family {
        "footnote" => oxoffice_docx::NoteFamily::Footnote,
        "endnote" => oxoffice_docx::NoteFamily::Endnote,
        _ => {
            return Err(PyValueError::new_err(
                "note family must be footnote or endnote",
            ));
        }
    };
    // Without a format Word numbers footnotes 1, 2, 3 and endnotes i, ii, iii.
    let format = match format {
        None if family == oxoffice_docx::NoteFamily::Endnote => {
            oxoffice_docx::NoteNumberFormat::LowerRoman
        }
        None => oxoffice_docx::NoteNumberFormat::Decimal,
        Some(format) => match format {
            "decimal" => oxoffice_docx::NoteNumberFormat::Decimal,
            "upperRoman" => oxoffice_docx::NoteNumberFormat::UpperRoman,
            "lowerRoman" => oxoffice_docx::NoteNumberFormat::LowerRoman,
            "upperLetter" => oxoffice_docx::NoteNumberFormat::UpperLetter,
            "lowerLetter" => oxoffice_docx::NoteNumberFormat::LowerLetter,
            "chicago" => oxoffice_docx::NoteNumberFormat::Chicago,
            _ => return Err(PyValueError::new_err("unsupported note number format")),
        },
    };
    let start = match start {
        None => 1,
        Some(value) if value.is_instance_of::<PyBool>() => {
            return Err(PyTypeError::new_err(
                "note numbering start must be an integer, not bool",
            ));
        }
        Some(value) => value.extract::<u32>()?,
    };
    let restart = match restart {
        "continuous" => oxoffice_docx::NoteRestart::Continuous,
        "eachSect" => oxoffice_docx::NoteRestart::EachSection,
        "eachPage" => oxoffice_docx::NoteRestart::EachPage,
        _ => return Err(PyValueError::new_err("unsupported note restart")),
    };
    let placement = match placement {
        None if family == oxoffice_docx::NoteFamily::Footnote => {
            oxoffice_docx::NotePlacement::PageBottom
        }
        None => oxoffice_docx::NotePlacement::DocumentEnd,
        Some("pageBottom") => oxoffice_docx::NotePlacement::PageBottom,
        Some("beneathText") => oxoffice_docx::NotePlacement::BeneathText,
        Some("sectEnd") => oxoffice_docx::NotePlacement::SectionEnd,
        Some("docEnd") => oxoffice_docx::NotePlacement::DocumentEnd,
        _ => return Err(PyValueError::new_err("unsupported note placement")),
    };
    Ok((
        family,
        oxoffice_docx::NotePolicy {
            format,
            start,
            restart,
            placement,
        },
    ))
}

#[pyclass(name = "Section", frozen, get_all, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PySection {
    pub ordinal: usize,
    pub is_final: bool,
    pub orientation: Option<String>,
    pub page_width: Option<i64>,
    pub page_height: Option<i64>,
    pub margin_top: Option<i64>,
    pub margin_right: Option<i64>,
    pub margin_bottom: Option<i64>,
    pub margin_left: Option<i64>,
    pub gutter: Option<i64>,
    pub column_count: Option<u32>,
    pub column_spacing: Option<i64>,
    pub page_number_start: Option<u32>,
    pub header_distance: Option<i64>,
    pub footer_distance: Option<i64>,
    pub different_first_page: Option<bool>,
    pub break_type: Option<String>,
}

impl PySection {
    fn live_section_alternative(&self, property: &str) -> PyResult<()> {
        Err(pyo3::exceptions::PyNotImplementedError::new_err(format!(
            "Section is a snapshot; use document.section({}).{}",
            self.ordinal, property,
        )))
    }
}

#[pymethods]
impl PySection {
    #[getter]
    fn left_margin(&self) -> PyResult<()> {
        self.live_section_alternative("left_margin")
    }
    #[setter]
    fn set_left_margin(&self, _value: &Bound<'_, PyAny>) -> PyResult<()> {
        self.live_section_alternative("left_margin")
    }
    #[getter]
    fn header(&self) -> PyResult<()> {
        self.live_section_alternative("header")
    }
    #[getter]
    fn footer(&self) -> PyResult<()> {
        self.live_section_alternative("footer")
    }
    #[getter]
    fn first_page_header(&self) -> PyResult<()> {
        self.live_section_alternative("first_page_header")
    }
    #[getter]
    fn different_first_page_header_footer(&self) -> PyResult<()> {
        self.live_section_alternative("different_first_page_header_footer")
    }
    #[setter]
    fn set_different_first_page_header_footer(&self, _value: &Bound<'_, PyAny>) -> PyResult<()> {
        self.live_section_alternative("different_first_page_header_footer")
    }

    #[new]
    #[pyo3(signature = (*, ordinal, is_final, orientation, page_width, page_height, margin_top, margin_right, margin_bottom, margin_left, gutter, column_count, column_spacing, page_number_start, header_distance, footer_distance, different_first_page, break_type))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        ordinal: usize,
        is_final: bool,
        orientation: Option<String>,
        page_width: Option<i64>,
        page_height: Option<i64>,
        margin_top: Option<i64>,
        margin_right: Option<i64>,
        margin_bottom: Option<i64>,
        margin_left: Option<i64>,
        gutter: Option<i64>,
        column_count: Option<u32>,
        column_spacing: Option<i64>,
        page_number_start: Option<u32>,
        header_distance: Option<i64>,
        footer_distance: Option<i64>,
        different_first_page: Option<bool>,
        break_type: Option<String>,
    ) -> Self {
        Self {
            ordinal,
            is_final,
            orientation,
            page_width,
            page_height,
            margin_top,
            margin_right,
            margin_bottom,
            margin_left,
            gutter,
            column_count,
            column_spacing,
            page_number_start,
            header_distance,
            footer_distance,
            different_first_page,
            break_type,
        }
    }
}
