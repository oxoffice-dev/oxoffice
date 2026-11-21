//! Content controls: their type payloads, snapshots and the names Python uses.

use super::*;

#[pymethods]
impl PyContentControlTypePayload {
    #[staticmethod]
    fn rich_text() -> Self {
        Self {
            inner: oxoffice_docx::ContentControlTypePayload::RichText,
        }
    }

    #[staticmethod]
    fn plain_text() -> Self {
        Self {
            inner: oxoffice_docx::ContentControlTypePayload::PlainText,
        }
    }

    #[staticmethod]
    fn date(format: String) -> Self {
        Self {
            inner: oxoffice_docx::ContentControlTypePayload::Date { format },
        }
    }

    #[staticmethod]
    fn checkbox() -> Self {
        Self {
            inner: oxoffice_docx::ContentControlTypePayload::CheckBox,
        }
    }

    #[staticmethod]
    fn combo_box(items: Vec<(String, String)>) -> Self {
        Self {
            inner: oxoffice_docx::ContentControlTypePayload::ComboBox { items },
        }
    }

    #[staticmethod]
    fn drop_down_list(items: Vec<(String, String)>) -> Self {
        Self {
            inner: oxoffice_docx::ContentControlTypePayload::DropDownList { items },
        }
    }

    #[staticmethod]
    fn picture(relationship_id: String) -> Self {
        Self {
            inner: oxoffice_docx::ContentControlTypePayload::Picture { relationship_id },
        }
    }

    #[staticmethod]
    fn group() -> Self {
        Self {
            inner: oxoffice_docx::ContentControlTypePayload::Group,
        }
    }

    #[staticmethod]
    fn repeating_section() -> Self {
        Self {
            inner: oxoffice_docx::ContentControlTypePayload::RepeatingSection,
        }
    }

    #[staticmethod]
    fn repeating_section_item() -> Self {
        Self {
            inner: oxoffice_docx::ContentControlTypePayload::RepeatingSectionItem,
        }
    }
}

/// One content control of the document body.
#[pyclass(name = "ContentControl", frozen, eq, skip_from_py_object)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyContentControl {
    #[pyo3(get)]
    tag: Option<String>,
    #[pyo3(get)]
    alias: Option<String>,
    #[pyo3(get)]
    id: Option<i32>,
    #[pyo3(get, name = "type")]
    control_type: &'static str,
    #[pyo3(get)]
    text: String,
}

impl From<oxoffice_docx::ContentControlRef<'_>> for PyContentControl {
    fn from(control: oxoffice_docx::ContentControlRef<'_>) -> Self {
        Self {
            tag: control.tag().map(str::to_owned),
            alias: control.alias().map(str::to_owned),
            id: control.id(),
            control_type: content_control_type_name(control.control_type()),
            text: control.text(),
        }
    }
}

#[pymethods]
impl PyContentControl {
    fn __repr__(&self) -> String {
        let optional = |value: &Option<String>| match value {
            Some(value) => format!("{value:?}"),
            None => "None".to_owned(),
        };
        format!(
            "ContentControl(tag={}, alias={}, type={:?}, text={:?})",
            optional(&self.tag),
            optional(&self.alias),
            self.control_type,
            self.text
        )
    }
}

/// A control without a type element is a rich text control.
fn content_control_type_name(
    control_type: Option<oxoffice_docx_oxml::content_control::SdtType>,
) -> &'static str {
    use oxoffice_docx_oxml::content_control::SdtType;
    match control_type {
        None | Some(SdtType::RichText) => "rich_text",
        Some(SdtType::PlainText) => "plain_text",
        Some(SdtType::Picture) => "picture",
        Some(SdtType::CheckBox) => "checkbox",
        Some(SdtType::ComboBox) => "combo_box",
        Some(SdtType::DropDownList) => "dropdown_list",
        Some(SdtType::Date) => "date",
        Some(SdtType::DocumentPartList) => "document_part_list",
        Some(SdtType::DocumentPartObject) => "document_part_object",
        Some(SdtType::Group) => "group",
        Some(SdtType::RepeatingSection) => "repeating_section",
        Some(SdtType::RepeatingSectionItem) => "repeating_section_item",
        Some(SdtType::Citation) => "citation",
        Some(SdtType::Equation) => "equation",
        Some(SdtType::Bibliography) => "bibliography",
    }
}

pub(super) fn content_control_kind_from_name(
    name: &str,
) -> PyResult<oxoffice_docx_oxml::content_control::SdtType> {
    use oxoffice_docx_oxml::content_control::SdtType;
    match name {
        "rich_text" => Ok(SdtType::RichText),
        "plain_text" => Ok(SdtType::PlainText),
        "picture" => Ok(SdtType::Picture),
        "checkbox" => Ok(SdtType::CheckBox),
        "combo_box" => Ok(SdtType::ComboBox),
        "dropdown_list" => Ok(SdtType::DropDownList),
        "date" => Ok(SdtType::Date),
        "document_part_list" => Ok(SdtType::DocumentPartList),
        "document_part_object" => Ok(SdtType::DocumentPartObject),
        "group" => Ok(SdtType::Group),
        "repeating_section" => Ok(SdtType::RepeatingSection),
        "repeating_section_item" => Ok(SdtType::RepeatingSectionItem),
        "citation" => Ok(SdtType::Citation),
        "equation" => Ok(SdtType::Equation),
        "bibliography" => Ok(SdtType::Bibliography),
        _ => Err(PyValueError::new_err(format!(
            "unknown content control kind {name:?}"
        ))),
    }
}

pub(super) fn content_control_placement_from_name(
    name: &str,
) -> PyResult<oxoffice_docx::ContentControlPlacement> {
    match name {
        "block" => Ok(oxoffice_docx::ContentControlPlacement::Block),
        "row" => Ok(oxoffice_docx::ContentControlPlacement::Row),
        "cell" => Ok(oxoffice_docx::ContentControlPlacement::Cell),
        "paragraph" => Ok(oxoffice_docx::ContentControlPlacement::Paragraph),
        "inline" => Ok(oxoffice_docx::ContentControlPlacement::Inline),
        _ => Err(PyValueError::new_err(format!(
            "content control placement must be block, row, cell, paragraph, or inline, got {name:?}"
        ))),
    }
}

pub(super) fn content_control_lock_from_name(
    name: &str,
) -> PyResult<oxoffice_docx::ContentControlLock> {
    match name {
        "unlocked" => Ok(oxoffice_docx::ContentControlLock::Unlocked),
        "sdtLocked" => Ok(oxoffice_docx::ContentControlLock::SdtLocked),
        "contentLocked" => Ok(oxoffice_docx::ContentControlLock::ContentLocked),
        "sdtContentLocked" => Ok(oxoffice_docx::ContentControlLock::SdtContentLocked),
        _ => Err(PyValueError::new_err(format!(
            "unknown content control lock {name:?}"
        ))),
    }
}

pub(super) fn content_control_appearance_from_name(
    name: &str,
) -> PyResult<oxoffice_docx::ContentControlAppearance> {
    match name {
        "boundingBox" => Ok(oxoffice_docx::ContentControlAppearance::BoundingBox),
        "tags" => Ok(oxoffice_docx::ContentControlAppearance::Tags),
        "hidden" => Ok(oxoffice_docx::ContentControlAppearance::Hidden),
        _ => Err(PyValueError::new_err(format!(
            "unknown content control appearance {name:?}"
        ))),
    }
}

pub(super) fn optional_python_bool(
    value: Option<&Bound<'_, PyAny>>,
    name: &str,
) -> PyResult<Option<bool>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_none() {
        return Ok(None);
    }
    if !value.is_instance_of::<PyBool>() {
        return Err(PyTypeError::new_err(format!(
            "{name} must be a bool or None"
        )));
    }
    value.extract::<bool>().map(Some)
}
