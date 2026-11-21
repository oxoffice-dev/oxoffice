//! Legacy form fields, form field targets, values and rules, and document protection.

use super::*;

/// An owned immutable legacy field inventory entry.
#[pyclass(name = "LegacyFormField", frozen)]
pub struct PyLegacyFormField {
    #[pyo3(get)]
    source_part: String,
    #[pyo3(get)]
    ordinal: usize,
    #[pyo3(get)]
    name: Option<String>,
    #[pyo3(get)]
    enabled: bool,
    #[pyo3(get)]
    calculate_on_exit: bool,
    #[pyo3(get, name = "type")]
    field_type: &'static str,
    #[pyo3(get)]
    value: Py<PyAny>,
    #[pyo3(get)]
    choices: Py<PyTuple>,
    #[pyo3(get)]
    default_value: Py<PyAny>,
}

impl PyLegacyFormField {
    pub(super) fn from_info(
        py: Python<'_>,
        field: oxoffice_docx::LegacyFormFieldInfo,
        default_value: Option<oxoffice_docx::LegacyFormFieldValue>,
    ) -> PyResult<Self> {
        let value = match field.value {
            oxoffice_docx::LegacyFormFieldValue::Text(value) => {
                value.into_pyobject(py)?.into_any().unbind()
            }
            oxoffice_docx::LegacyFormFieldValue::Checked(value) => {
                value.into_pyobject(py)?.to_owned().into_any().unbind()
            }
            oxoffice_docx::LegacyFormFieldValue::SelectedIndex(value) => {
                value.into_pyobject(py)?.into_any().unbind()
            }
        };
        Ok(Self {
            source_part: field.source_part,
            ordinal: field.ordinal,
            name: field.name,
            enabled: field.enabled,
            calculate_on_exit: field.calculate_on_exit,
            field_type: match field.kind {
                oxoffice_docx::LegacyFormFieldKind::TextInput => "text",
                oxoffice_docx::LegacyFormFieldKind::CheckBox => "checkbox",
                oxoffice_docx::LegacyFormFieldKind::DropDownList => "dropdown",
            },
            value,
            choices: PyTuple::new(py, field.choices)?.unbind(),
            default_value: match default_value {
                Some(value) => legacy_value_to_py(py, &value)?,
                None => py.None(),
            },
        })
    }
}

fn legacy_value_to_py(
    py: Python<'_>,
    value: &oxoffice_docx::LegacyFormFieldValue,
) -> PyResult<Py<PyAny>> {
    match value {
        oxoffice_docx::LegacyFormFieldValue::Text(value) => {
            Ok(value.into_pyobject(py)?.into_any().unbind())
        }
        oxoffice_docx::LegacyFormFieldValue::Checked(value) => {
            Ok(value.into_pyobject(py)?.to_owned().into_any().unbind())
        }
        oxoffice_docx::LegacyFormFieldValue::SelectedIndex(value) => {
            Ok(value.into_pyobject(py)?.into_any().unbind())
        }
    }
}

fn strict_flag(value: &Bound<'_, PyAny>) -> PyResult<bool> {
    require_exact_bool(value, "flag")
}

fn strict_index(value: &Bound<'_, PyAny>) -> PyResult<usize> {
    if value.is_instance_of::<PyBool>() || !value.is_instance_of::<PyInt>() {
        return Err(PyTypeError::new_err("index requires an integer"));
    }
    ensure_nonnegative(value, "index cannot be negative")?;
    value.extract()
}

fn require_exact_bool(value: &Bound<'_, PyAny>, label: &str) -> PyResult<bool> {
    if !value.is_instance_of::<PyBool>() {
        return Err(PyTypeError::new_err(format!("{label} requires bool")));
    }
    value.extract()
}

fn optional_info_text(
    value: Option<&Bound<'_, PyAny>>,
) -> PyResult<Option<oxoffice_docx::LegacyFormInfoText>> {
    let Some(value) = value.filter(|value| !value.is_none()) else {
        return Ok(None);
    };
    let (kind, text) = value
        .extract::<(String, String)>()
        .map_err(|_| PyTypeError::new_err("help and status must be a (kind, text) pair or None"))?;
    match kind.as_str() {
        "text" => Ok(Some(oxoffice_docx::LegacyFormInfoText::Text(text))),
        "autoText" => Ok(Some(oxoffice_docx::LegacyFormInfoText::AutoText(text))),
        _ => Err(PyValueError::new_err(
            "help and status kind must be text or autoText",
        )),
    }
}

fn checkbox_size(value: Option<&Bound<'_, PyAny>>) -> PyResult<oxoffice_docx::LegacyCheckBoxSize> {
    let Some(value) = value.filter(|value| !value.is_none()) else {
        return Ok(oxoffice_docx::LegacyCheckBoxSize::Auto);
    };
    if value.is_instance_of::<PyBool>() || !value.is_instance_of::<PyInt>() {
        return Err(PyTypeError::new_err(
            "checkbox size requires an integer half-point count or None",
        ));
    }
    ensure_nonnegative(value, "checkbox size cannot be negative")?;
    Ok(oxoffice_docx::LegacyCheckBoxSize::HalfPoints(
        value.extract::<u64>()?,
    ))
}

fn copied_choices(value: &Bound<'_, PyAny>) -> PyResult<Vec<String>> {
    if value.is_instance_of::<PyString>() || value.is_instance_of::<PyBytes>() {
        return Err(PyTypeError::new_err(
            "choices must be a sequence of str, not str or bytes",
        ));
    }
    let mut choices = Vec::new();
    for item in value.try_iter()? {
        let item = item?;
        if !item.is_instance_of::<PyString>() {
            return Err(PyTypeError::new_err("each drop-down choice must be str"));
        }
        choices.push(item.extract()?);
    }
    Ok(choices)
}

fn text_input_type(value: &str) -> PyResult<oxoffice_docx::LegacyTextInputType> {
    Ok(match value {
        "regular" => oxoffice_docx::LegacyTextInputType::Regular,
        "number" => oxoffice_docx::LegacyTextInputType::Number,
        "date" => oxoffice_docx::LegacyTextInputType::Date,
        "currentTime" => oxoffice_docx::LegacyTextInputType::CurrentTime,
        "currentDate" => oxoffice_docx::LegacyTextInputType::CurrentDate,
        "calculated" => oxoffice_docx::LegacyTextInputType::Calculated,
        _ => {
            return Err(PyValueError::new_err(
                "text input type must be regular, number, date, currentTime, currentDate, or calculated",
            ));
        }
    })
}

fn optional_bounded_length(
    value: Option<&Bound<'_, PyAny>>,
    label: &str,
) -> PyResult<Option<usize>> {
    let Some(value) = value.filter(|value| !value.is_none()) else {
        return Ok(None);
    };
    if value.is_instance_of::<PyBool>() || !value.is_instance_of::<PyInt>() {
        return Err(PyTypeError::new_err(format!(
            "{label} requires an integer or None"
        )));
    }
    ensure_nonnegative(value, &format!("{label} cannot be negative"))?;
    Ok(Some(value.extract::<usize>()?))
}

fn ensure_nonnegative(value: &Bound<'_, PyAny>, message: &str) -> PyResult<()> {
    if value.lt(0)? {
        return Err(PyValueError::new_err(message.to_owned()));
    }
    Ok(())
}

fn optional_exact_bool(value: Option<&Bound<'_, PyAny>>, label: &str) -> PyResult<Option<bool>> {
    let Some(value) = value.filter(|value| !value.is_none()) else {
        return Ok(None);
    };
    Ok(Some(require_exact_bool(value, label)?))
}

fn optional_u32(value: Option<&Bound<'_, PyAny>>, label: &str) -> PyResult<Option<u32>> {
    let Some(value) = value.filter(|value| !value.is_none()) else {
        return Ok(None);
    };
    if value.is_instance_of::<PyBool>() || !value.is_instance_of::<PyInt>() {
        return Err(PyTypeError::new_err(format!(
            "{label} requires an integer or None"
        )));
    }
    ensure_nonnegative(value, &format!("{label} cannot be negative"))?;
    Ok(Some(value.extract()?))
}

/// Frozen authoring payload for one legacy form field.
#[pyclass(name = "LegacyFormFieldDefinition", frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct PyLegacyFormFieldDefinition {
    field: oxoffice_docx::Field,
}

impl PyLegacyFormFieldDefinition {
    pub(crate) fn field(&self) -> oxoffice_docx::Field {
        self.field.clone()
    }

    fn checked(
        py: Python<'_>,
        options: oxoffice_docx::LegacyFormFieldOptions,
        definition: oxoffice_docx::LegacyFormFieldDefinition,
    ) -> PyResult<Self> {
        let field = oxoffice_docx::Field::from_legacy_form(options, definition)
            .map_err(|error| docx_to_pyerr(py, oxoffice_docx::Error::Oxml(error)))?;
        Ok(Self { field })
    }

    fn shared_options(
        name: Option<String>,
        enabled: bool,
        calculate_on_exit: bool,
        help_text: Option<&Bound<'_, PyAny>>,
        status_text: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<oxoffice_docx::LegacyFormFieldOptions> {
        Ok(oxoffice_docx::LegacyFormFieldOptions {
            name,
            enabled,
            calculate_on_exit,
            help_text: optional_info_text(help_text)?,
            status_text: optional_info_text(status_text)?,
        })
    }
}

#[pymethods]
impl PyLegacyFormFieldDefinition {
    #[new]
    fn new(_private: &Bound<'_, PyAny>) -> PyResult<Self> {
        Err(PyTypeError::new_err(
            "LegacyFormFieldDefinition cannot be constructed directly",
        ))
    }

    #[staticmethod]
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (
        *,
        value="",
        default=None,
        input_type="regular",
        max_length=None,
        format=None,
        name=None,
        enabled=true,
        calculate_on_exit=false,
        help_text=None,
        status_text=None
    ))]
    fn text_input(
        py: Python<'_>,
        value: &str,
        default: Option<&Bound<'_, PyAny>>,
        input_type: &str,
        max_length: Option<&Bound<'_, PyAny>>,
        format: Option<String>,
        name: Option<String>,
        #[pyo3(from_py_with = strict_flag)] enabled: bool,
        #[pyo3(from_py_with = strict_flag)] calculate_on_exit: bool,
        help_text: Option<&Bound<'_, PyAny>>,
        status_text: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let default = match default.filter(|value| !value.is_none()) {
            Some(value) if value.is_instance_of::<PyString>() => Some(value.extract::<String>()?),
            Some(_) => return Err(PyTypeError::new_err("text default requires str or None")),
            None => None,
        };
        Self::checked(
            py,
            Self::shared_options(name, enabled, calculate_on_exit, help_text, status_text)?,
            oxoffice_docx::LegacyFormFieldDefinition::TextInput {
                input_type: text_input_type(input_type)?,
                default,
                value: value.to_owned(),
                max_length: optional_bounded_length(max_length, "max_length")?,
                format,
            },
        )
    }

    #[staticmethod]
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (
        *,
        checked=false,
        default=false,
        size_half_points=None,
        name=None,
        enabled=true,
        calculate_on_exit=false,
        help_text=None,
        status_text=None
    ))]
    fn checkbox(
        py: Python<'_>,
        #[pyo3(from_py_with = strict_flag)] checked: bool,
        #[pyo3(from_py_with = strict_flag)] default: bool,
        size_half_points: Option<&Bound<'_, PyAny>>,
        name: Option<String>,
        #[pyo3(from_py_with = strict_flag)] enabled: bool,
        #[pyo3(from_py_with = strict_flag)] calculate_on_exit: bool,
        help_text: Option<&Bound<'_, PyAny>>,
        status_text: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        Self::checked(
            py,
            Self::shared_options(name, enabled, calculate_on_exit, help_text, status_text)?,
            oxoffice_docx::LegacyFormFieldDefinition::CheckBox {
                size: checkbox_size(size_half_points)?,
                default,
                checked,
            },
        )
    }

    #[staticmethod]
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (
        choices,
        *,
        selected_index=0,
        default_index=0,
        name=None,
        enabled=true,
        calculate_on_exit=false,
        help_text=None,
        status_text=None
    ))]
    fn drop_down_list(
        py: Python<'_>,
        choices: &Bound<'_, PyAny>,
        #[pyo3(from_py_with = strict_index)] selected_index: usize,
        #[pyo3(from_py_with = strict_index)] default_index: usize,
        name: Option<String>,
        #[pyo3(from_py_with = strict_flag)] enabled: bool,
        #[pyo3(from_py_with = strict_flag)] calculate_on_exit: bool,
        help_text: Option<&Bound<'_, PyAny>>,
        status_text: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        Self::checked(
            py,
            Self::shared_options(name, enabled, calculate_on_exit, help_text, status_text)?,
            oxoffice_docx::LegacyFormFieldDefinition::DropDownList {
                choices: copied_choices(choices)?,
                default_index,
                selected_index,
            },
        )
    }
}

fn refuse_direct(class_name: &str) -> PyErr {
    PyTypeError::new_err(format!("{class_name} cannot be constructed directly"))
}

fn protection_mode(value: &str) -> PyResult<oxoffice_docx::ProtectionMode> {
    match value {
        "readOnly" => Ok(oxoffice_docx::ProtectionMode::ReadOnly),
        "comments" => Ok(oxoffice_docx::ProtectionMode::Comments),
        "trackedChanges" => Ok(oxoffice_docx::ProtectionMode::TrackedChanges),
        "forms" => Ok(oxoffice_docx::ProtectionMode::Forms),
        _ => Err(PyValueError::new_err(
            "document protection mode must be readOnly, comments, trackedChanges, or forms",
        )),
    }
}

fn crypt_provider(value: &str) -> PyResult<oxoffice_docx::CryptProviderType> {
    match value {
        "rsaAES" => Ok(oxoffice_docx::CryptProviderType::RsaAes),
        "rsaFull" => Ok(oxoffice_docx::CryptProviderType::RsaFull),
        "custom" => Ok(oxoffice_docx::CryptProviderType::Custom),
        _ => Err(PyValueError::new_err(
            "document protection provider_type must be rsaAES, rsaFull, or custom",
        )),
    }
}

fn crypt_algorithm_class(value: &str) -> PyResult<oxoffice_docx::CryptAlgorithmClass> {
    match value {
        "hash" => Ok(oxoffice_docx::CryptAlgorithmClass::Hash),
        "custom" => Ok(oxoffice_docx::CryptAlgorithmClass::Custom),
        _ => Err(PyValueError::new_err(
            "document protection algorithm_class must be hash or custom",
        )),
    }
}

fn crypt_algorithm_type(value: &str) -> PyResult<oxoffice_docx::CryptAlgorithmType> {
    match value {
        "typeAny" => Ok(oxoffice_docx::CryptAlgorithmType::Any),
        "custom" => Ok(oxoffice_docx::CryptAlgorithmType::Custom),
        _ => Err(PyValueError::new_err(
            "document protection algorithm_type must be typeAny or custom",
        )),
    }
}

/// Frozen tag or legacy identity for one form field.
#[pyclass(name = "FormFieldTarget", frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct PyFormFieldTarget {
    inner: oxoffice_docx::FormFieldTarget,
}

#[pymethods]
impl PyFormFieldTarget {
    #[new]
    fn new(_private: &Bound<'_, PyAny>) -> PyResult<Self> {
        Err(refuse_direct("FormFieldTarget"))
    }

    #[staticmethod]
    fn content_control_tag(tag: String) -> Self {
        Self {
            inner: oxoffice_docx::FormFieldTarget::ContentControlTag(tag),
        }
    }

    #[staticmethod]
    fn legacy(source_part: String, #[pyo3(from_py_with = strict_index)] ordinal: usize) -> Self {
        Self {
            inner: oxoffice_docx::FormFieldTarget::Legacy {
                source_part,
                ordinal,
            },
        }
    }
}

/// Frozen typed value for one form field.
#[pyclass(name = "FormFieldValue", frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct PyFormFieldValue {
    inner: oxoffice_docx::FormFieldValue,
}

#[pymethods]
impl PyFormFieldValue {
    #[new]
    fn new(_private: &Bound<'_, PyAny>) -> PyResult<Self> {
        Err(refuse_direct("FormFieldValue"))
    }

    #[staticmethod]
    fn text(py: Python<'_>, value: String) -> PyResult<Self> {
        Ok(Self {
            inner: oxoffice_docx::FormFieldValue::Text(checked_form_text(py, value)?),
        })
    }

    #[staticmethod]
    fn checked(#[pyo3(from_py_with = strict_flag)] value: bool) -> Self {
        Self {
            inner: oxoffice_docx::FormFieldValue::Checked(value),
        }
    }

    #[staticmethod]
    fn selected_index(#[pyo3(from_py_with = strict_index)] value: usize) -> Self {
        Self {
            inner: oxoffice_docx::FormFieldValue::SelectedIndex(value),
        }
    }
}

/// Frozen reset payload. A value and a placeholder are separate factories.
#[pyclass(name = "FormResetValue", frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct PyFormResetValue {
    inner: oxoffice_docx::FormResetValue,
}

#[pymethods]
impl PyFormResetValue {
    #[new]
    fn new(_private: &Bound<'_, PyAny>) -> PyResult<Self> {
        Err(refuse_direct("FormResetValue"))
    }

    #[staticmethod]
    fn value(value: &Bound<'_, PyAny>) -> PyResult<Self> {
        let value = value
            .cast::<PyFormFieldValue>()
            .map_err(|_| PyTypeError::new_err("reset value requires FormFieldValue"))?
            .get()
            .inner
            .clone();
        Ok(Self {
            inner: oxoffice_docx::FormResetValue::Value(value),
        })
    }

    #[staticmethod]
    fn placeholder(py: Python<'_>, text: String) -> PyResult<Self> {
        Ok(Self {
            inner: oxoffice_docx::FormResetValue::Placeholder {
                text: checked_form_text(py, text)?,
            },
        })
    }
}

/// One transient validation and reset rule.
#[pyclass(name = "FormFieldRule", frozen, skip_from_py_object)]
pub struct PyFormFieldRule {
    #[pyo3(get)]
    target: Py<PyFormFieldTarget>,
    #[pyo3(get)]
    reset: Py<PyFormResetValue>,
    #[pyo3(get)]
    required: bool,
}

#[pymethods]
impl PyFormFieldRule {
    #[new]
    #[pyo3(
        signature = (*, target, reset, required = false),
        text_signature = "(*, target, reset, required=False)"
    )]
    fn new(
        py: Python<'_>,
        target: &Bound<'_, PyAny>,
        reset: &Bound<'_, PyAny>,
        #[pyo3(from_py_with = strict_flag)] required: bool,
    ) -> PyResult<Self> {
        let target = copied_target(py, target)?;
        let reset = copied_reset(py, reset)?;
        Ok(Self {
            target,
            reset,
            required,
        })
    }
}

/// One value to publish for a target that a rule already names.
#[pyclass(name = "FormFieldAssignment", frozen, skip_from_py_object)]
pub struct PyFormFieldAssignment {
    #[pyo3(get)]
    target: Py<PyFormFieldTarget>,
    #[pyo3(get)]
    value: Py<PyFormFieldValue>,
}

#[pymethods]
impl PyFormFieldAssignment {
    #[new]
    #[pyo3(
        signature = (*, target, value),
        text_signature = "(*, target, value)"
    )]
    fn new(py: Python<'_>, target: &Bound<'_, PyAny>, value: &Bound<'_, PyAny>) -> PyResult<Self> {
        Ok(Self {
            target: copied_target(py, target)?,
            value: copied_value(py, value)?,
        })
    }
}

/// One current-state problem for one physical control or legacy field.
#[pyclass(name = "FormValidationIssue", frozen, skip_from_py_object)]
pub struct PyFormValidationIssue {
    #[pyo3(get)]
    target: Py<PyFormFieldTarget>,
    #[pyo3(get)]
    source_part: String,
    #[pyo3(get)]
    ordinal: usize,
    #[pyo3(get)]
    message: String,
}

#[pymethods]
impl PyFormValidationIssue {
    #[new]
    fn new(_private: &Bound<'_, PyAny>) -> PyResult<Self> {
        Err(refuse_direct("FormValidationIssue"))
    }
}

#[pymethods]
impl PyDocumentProtection {
    #[new]
    #[allow(clippy::too_many_arguments)]
    #[pyo3(
        signature = (
            *,
            mode,
            enforcement=None,
            formatting=None,
            provider_type=None,
            algorithm_class=None,
            algorithm_type=None,
            algorithm_sid=None,
            spin_count=None,
            hash=None,
            salt=None
        ),
        text_signature = "(*, mode, enforcement=None, formatting=None, provider_type=None, algorithm_class=None, algorithm_type=None, algorithm_sid=None, spin_count=None, hash=None, salt=None)"
    )]
    fn new(
        mode: &str,
        enforcement: Option<&Bound<'_, PyAny>>,
        formatting: Option<&Bound<'_, PyAny>>,
        provider_type: Option<&str>,
        algorithm_class: Option<&str>,
        algorithm_type: Option<&str>,
        algorithm_sid: Option<&Bound<'_, PyAny>>,
        spin_count: Option<&Bound<'_, PyAny>>,
        hash: Option<String>,
        salt: Option<String>,
    ) -> PyResult<Self> {
        Ok(Self {
            inner: oxoffice_docx::DocumentProtection {
                mode: protection_mode(mode)?,
                enforcement: optional_exact_bool(enforcement, "enforcement")?,
                formatting: optional_exact_bool(formatting, "formatting")?,
                provider_type: provider_type.map(crypt_provider).transpose()?,
                algorithm_class: algorithm_class.map(crypt_algorithm_class).transpose()?,
                algorithm_type: algorithm_type.map(crypt_algorithm_type).transpose()?,
                algorithm_sid: optional_u32(algorithm_sid, "algorithm_sid")?,
                spin_count: optional_u32(spin_count, "spin_count")?,
                hash,
                salt,
            },
        })
    }

    #[getter]
    fn mode(&self) -> &'static str {
        match self.inner.mode {
            oxoffice_docx::ProtectionMode::ReadOnly => "readOnly",
            oxoffice_docx::ProtectionMode::Comments => "comments",
            oxoffice_docx::ProtectionMode::TrackedChanges => "trackedChanges",
            oxoffice_docx::ProtectionMode::Forms => "forms",
        }
    }

    #[getter]
    fn enforcement(&self) -> Option<bool> {
        self.inner.enforcement
    }

    #[getter]
    fn formatting(&self) -> Option<bool> {
        self.inner.formatting
    }

    #[getter]
    fn provider_type(&self) -> Option<&'static str> {
        self.inner.provider_type.map(|value| match value {
            oxoffice_docx::CryptProviderType::RsaAes => "rsaAES",
            oxoffice_docx::CryptProviderType::RsaFull => "rsaFull",
            oxoffice_docx::CryptProviderType::Custom => "custom",
        })
    }

    #[getter]
    fn algorithm_class(&self) -> Option<&'static str> {
        self.inner.algorithm_class.map(|value| match value {
            oxoffice_docx::CryptAlgorithmClass::Hash => "hash",
            oxoffice_docx::CryptAlgorithmClass::Custom => "custom",
        })
    }

    #[getter]
    fn algorithm_type(&self) -> Option<&'static str> {
        self.inner.algorithm_type.map(|value| match value {
            oxoffice_docx::CryptAlgorithmType::Any => "typeAny",
            oxoffice_docx::CryptAlgorithmType::Custom => "custom",
        })
    }

    #[getter]
    fn algorithm_sid(&self) -> Option<u32> {
        self.inner.algorithm_sid
    }

    #[getter]
    fn spin_count(&self) -> Option<u32> {
        self.inner.spin_count
    }

    #[getter]
    fn hash(&self) -> Option<&str> {
        self.inner.hash.as_deref()
    }

    #[getter]
    fn salt(&self) -> Option<&str> {
        self.inner.salt.as_deref()
    }
}

fn checked_form_text(py: Python<'_>, value: String) -> PyResult<String> {
    oxoffice_docx_oxml::CT_SdtPr::default()
        .prepare_text_value(&value)
        .map_err(|error| docx_to_pyerr(py, oxoffice_docx::Error::Oxml(error)))?;
    Ok(value)
}

pub(super) fn require_carrier_sequence(value: &Bound<'_, PyAny>, label: &str) -> PyResult<()> {
    let abc = value.py().import("collections.abc")?;
    if value.is_instance_of::<PyString>()
        || value.is_instance_of::<PyBytes>()
        || value.is_instance(&abc.getattr("Mapping")?)?
        || value.is_instance(&abc.getattr("Set")?)?
    {
        return Err(PyTypeError::new_err(format!(
            "{label} must be a sequence, not str, bytes, mapping, or set"
        )));
    }
    if !value.is_instance(&abc.getattr("Sequence")?)? {
        return Err(PyTypeError::new_err(format!("{label} must be a sequence")));
    }
    Ok(())
}

pub(super) fn copied_rules(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
) -> PyResult<Vec<oxoffice_docx::FormFieldRule>> {
    require_carrier_sequence(value, "rules")?;
    let mut rules = Vec::new();
    for item in value.try_iter()? {
        let item = item?;
        let rule = item
            .cast::<PyFormFieldRule>()
            .map_err(|_| PyTypeError::new_err("rules must be a sequence of FormFieldRule"))?;
        let rule = rule.borrow();
        rules.push(oxoffice_docx::FormFieldRule {
            target: rule.target.bind(py).borrow().inner.clone(),
            required: rule.required,
            reset: rule.reset.bind(py).borrow().inner.clone(),
        });
    }
    Ok(rules)
}

pub(super) fn copied_assignments(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
) -> PyResult<Vec<oxoffice_docx::FormFieldAssignment>> {
    require_carrier_sequence(value, "assignments")?;
    let mut assignments = Vec::new();
    for item in value.try_iter()? {
        let item = item?;
        let assignment = item.cast::<PyFormFieldAssignment>().map_err(|_| {
            PyTypeError::new_err("assignments must be a sequence of FormFieldAssignment")
        })?;
        let assignment = assignment.borrow();
        assignments.push(oxoffice_docx::FormFieldAssignment {
            target: assignment.target.bind(py).borrow().inner.clone(),
            value: assignment.value.bind(py).borrow().inner.clone(),
        });
    }
    Ok(assignments)
}

pub(super) fn issue_to_py(
    py: Python<'_>,
    issue: oxoffice_docx::FormValidationIssue,
) -> PyResult<PyFormValidationIssue> {
    Ok(PyFormValidationIssue {
        target: Py::new(
            py,
            PyFormFieldTarget {
                inner: issue.target,
            },
        )?,
        source_part: issue.source_part,
        ordinal: issue.ordinal,
        message: issue.message,
    })
}

fn copied_target(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<Py<PyFormFieldTarget>> {
    let inner = value
        .cast::<PyFormFieldTarget>()
        .map_err(|_| PyTypeError::new_err("target requires FormFieldTarget"))?
        .get()
        .inner
        .clone();
    Py::new(py, PyFormFieldTarget { inner })
}

fn copied_value(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<Py<PyFormFieldValue>> {
    let inner = value
        .cast::<PyFormFieldValue>()
        .map_err(|_| PyTypeError::new_err("value requires FormFieldValue"))?
        .get()
        .inner
        .clone();
    Py::new(py, PyFormFieldValue { inner })
}

fn copied_reset(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<Py<PyFormResetValue>> {
    let inner = value
        .cast::<PyFormResetValue>()
        .map_err(|_| PyTypeError::new_err("reset requires FormResetValue"))?
        .get()
        .inner
        .clone();
    Py::new(py, PyFormResetValue { inner })
}
