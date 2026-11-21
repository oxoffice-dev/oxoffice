//! List levels a caller builds for Document.add_numbering_definition.

use super::*;

/// One level of a numbering definition for `Document.add_numbering_definition`.
///
/// `format` is a Word `w:numFmt` name such as `decimal`, `lowerLetter` or
/// `bullet`. `text` is the level text, `%1.` or a bullet glyph by default.
/// The indents are EMU, as `Length` values are, and default to half an inch
/// per level with a quarter-inch hanging indent.
#[pyclass(name = "ListLevel", frozen, get_all, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyListLevel {
    pub format: String,
    pub font: Option<String>,
    pub text: Option<String>,
    pub start: Option<u32>,
    pub left_indent: Option<i64>,
    pub hanging_indent: Option<i64>,
}

#[pymethods]
impl PyListLevel {
    #[staticmethod]
    #[pyo3(signature = (*, checked = false))]
    fn checklist(checked: bool) -> Self {
        Self {
            format: "bullet".to_owned(),
            font: None,
            text: Some(if checked { "☑" } else { "☐" }.to_owned()),
            start: None,
            left_indent: None,
            hanging_indent: None,
        }
    }

    #[new]
    #[pyo3(signature = (*, format = "decimal", text = None, start = None, left_indent = None, hanging_indent = None, font = None))]
    fn new(
        format: &str,
        text: Option<String>,
        start: Option<u32>,
        left_indent: Option<i64>,
        hanging_indent: Option<i64>,
        font: Option<String>,
    ) -> PyResult<Self> {
        if let oxoffice_docx::ListNumberFormat::Other(_) =
            oxoffice_docx::ListNumberFormat::from_name(format)
        {
            return Err(PyValueError::new_err(format!(
                "'{format}' is not a Word numbering format"
            )));
        }
        if hanging_indent.is_some_and(|value| value < 0) {
            return Err(PyValueError::new_err("hanging_indent cannot be negative"));
        }
        if let Some(name) = &font {
            if name.trim().is_empty() {
                return Err(PyValueError::new_err("font must be nonempty"));
            }
            if name.chars().any(|c| {
                (c < ' ' && !matches!(c, '\t' | '\n' | '\r'))
                    || matches!(c, '\u{fffe}' | '\u{ffff}')
            }) {
                return Err(PyValueError::new_err("font contains a non-XML character"));
            }
        }
        Ok(Self {
            format: format.to_owned(),
            font,
            text,
            start,
            left_indent,
            hanging_indent,
        })
    }
}

impl PyListLevel {
    pub(super) fn native(&self) -> oxoffice_docx::ListLevel {
        let twips =
            |value: Option<i64>| value.map(|emu| oxoffice_docx::Length::emu(emu).as_twips());
        let mut level =
            oxoffice_docx::ListLevel::new(oxoffice_docx::ListNumberFormat::from_name(&self.format))
                .indentation(twips(self.left_indent), twips(self.hanging_indent), None);
        level.start = self.start;
        if let Some(name) = &self.font {
            level = level.marker_properties(oxoffice_docx_oxml::properties::CT_RPr {
                font_ascii: Some(name.clone()),
                font_hansi: Some(name.clone()),
                ..Default::default()
            });
        }
        match &self.text {
            Some(text) => level.level_text(text.as_str()),
            None => level,
        }
    }
}
