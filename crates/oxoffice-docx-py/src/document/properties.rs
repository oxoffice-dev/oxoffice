//! Core, application and custom document properties, and the default paragraph format.

use super::*;

/// One text field of the native core-properties model.

#[pyclass(name = "CustomFileTime", frozen, get_all, skip_from_py_object)]
#[derive(Clone)]
pub struct PyCustomFileTime {
    value: String,
}

fn valid_custom_filetime(value: &str) -> bool {
    let b = value.as_bytes();
    if b.len() < 20
        || !b.is_ascii()
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return false;
    }
    let number = |start: usize, end: usize| -> Option<u32> {
        b[start..end]
            .iter()
            .all(u8::is_ascii_digit)
            .then(|| value[start..end].parse().ok())
            .flatten()
    };
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        number(0, 4),
        number(5, 7),
        number(8, 10),
        number(11, 13),
        number(14, 16),
        number(17, 19),
    ) else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => return false,
    };
    if year == 0 || day == 0 || day > days || hour > 23 || minute > 59 || second > 59 {
        return false;
    }
    let mut at = 19;
    if b.get(at) == Some(&b'.') {
        at += 1;
        let start = at;
        while b.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        if at == start {
            return false;
        }
    }
    if b.get(at) == Some(&b'Z') {
        return at + 1 == b.len();
    }
    if !matches!(b.get(at), Some(b'+') | Some(b'-')) || at + 6 != b.len() || b[at + 3] != b':' {
        return false;
    }
    number(at + 1, at + 3).is_some_and(|n| n <= 23)
        && number(at + 4, at + 6).is_some_and(|n| n <= 59)
}

#[pymethods]
impl PyCustomFileTime {
    #[new]
    fn new(value: String) -> PyResult<Self> {
        if !valid_custom_filetime(&value) {
            return Err(PyValueError::new_err(
                "CustomFileTime requires a valid Gregorian RFC3339 date with explicit timezone and no leap second",
            ));
        }
        Ok(Self { value })
    }
}

#[pyclass(name = "CustomProperty", frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct PyCustomProperty {
    property: oxoffice_docx::CustomProperty,
}

#[pymethods]
impl PyCustomProperty {
    #[getter]
    fn name(&self) -> Option<String> {
        self.property.name.clone()
    }
    #[getter]
    fn pid(&self) -> i32 {
        self.property.pid
    }
    #[getter]
    fn fmtid(&self) -> String {
        self.property.fmtid.clone()
    }
    #[getter]
    fn r#type(&self) -> &'static str {
        use oxoffice_docx::CustomPropertyValue as V;
        match &self.property.value {
            V::Lpstr(_) => "lpstr",
            V::Lpwstr(_) => "lpwstr",
            V::I4(_) => "i4",
            V::R8(_) => "r8",
            V::Bool(_) => "bool",
            V::FileTime(_) => "filetime",
            V::Empty => "empty",
            V::Raw(_) => "raw",
        }
    }
    #[getter]
    fn value(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        use oxoffice_docx::CustomPropertyValue as V;
        Ok(match &self.property.value {
            V::Lpstr(v) | V::Lpwstr(v) | V::FileTime(v) => v.into_pyobject(py)?.into_any().unbind(),
            V::I4(v) => v.into_pyobject(py)?.into_any().unbind(),
            V::R8(v) => v.into_pyobject(py)?.into_any().unbind(),
            V::Bool(v) => v.into_pyobject(py)?.to_owned().into_any().unbind(),
            V::Empty => py.None(),
            V::Raw(v) => PyBytes::new(py, v).into_any().unbind(),
        })
    }
}

#[pyclass(name = "DefaultParagraphFormat", frozen, get_all, skip_from_py_object)]
pub struct PyDefaultParagraphFormat {
    pub space_before: Option<Py<PyAny>>,
    pub space_after: Option<Py<PyAny>>,
    pub line_spacing: Option<Py<PyAny>>,
}

fn length_from_twips(
    py: Python<'_>,
    twips: Option<oxoffice_docx::Twips>,
) -> PyResult<Option<Py<PyAny>>> {
    twips
        .map(|twips| crate::length_object(py, oxoffice_docx::Length::twips(twips.0)))
        .transpose()
}

fn line_spacing_value(
    py: Python<'_>,
    properties: &oxoffice_docx::CT_PPr,
) -> PyResult<Option<Py<PyAny>>> {
    let Some(line) = properties.line_spacing else {
        return Ok(None);
    };
    match properties.line_rule.as_deref() {
        Some("exact") | Some("atLeast") => {
            crate::length_object(py, oxoffice_docx::Length::twips(line.0)).map(Some)
        }
        None | Some("auto") => Ok(Some(
            (line.0 as f64 / 240.0)
                .into_pyobject(py)?
                .to_owned()
                .unbind()
                .into_any(),
        )),
        _ => Ok(None),
    }
}

pub(super) fn default_paragraph_format_snapshot(
    py: Python<'_>,
    properties: Option<&oxoffice_docx::CT_PPr>,
) -> PyResult<PyDefaultParagraphFormat> {
    let Some(properties) = properties else {
        return Ok(PyDefaultParagraphFormat {
            space_before: None,
            space_after: None,
            line_spacing: None,
        });
    };
    Ok(PyDefaultParagraphFormat {
        space_before: length_from_twips(py, properties.space_before)?,
        space_after: length_from_twips(py, properties.space_after)?,
        line_spacing: line_spacing_value(py, properties)?,
    })
}

pub(super) fn length_keyword(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
    name: &str,
) -> PyResult<Option<oxoffice_docx::Twips>> {
    if value.is_none() {
        return Ok(None);
    }
    let message = format!("{name} requires a Length or None");
    let length_type = py.import("oxoffice.docx")?.getattr("Length")?;
    if value.is_instance_of::<PyBool>()
        || value.is_instance_of::<PyFloat>()
        || !value.is_instance(&length_type)?
    {
        return Err(PyTypeError::new_err(message));
    }
    let emu = value
        .extract::<i64>()
        .map_err(|_| PyValueError::new_err(format!("{name} must be a representable Length")))?;
    let twips = i32::try_from(emu / 635)
        .map_err(|_| PyValueError::new_err(format!("{name} must fit signed twips")))?;
    Ok(Some(oxoffice_docx::Twips(twips)))
}

/// A default `line_spacing` keyword. `Length` is an exact pitch. A non-bool
/// int or float is a line multiple. `None` clears the line and its rule.
pub(super) fn line_spacing_keyword(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
) -> PyResult<Option<(oxoffice_docx::Twips, &'static str)>> {
    if value.is_none() {
        return Ok(None);
    }
    let message = "line_spacing requires a numeric line multiple, a Length or None";
    if value.is_instance_of::<PyBool>() {
        return Err(PyTypeError::new_err(message));
    }
    if value.is_instance(&py.import("oxoffice.docx")?.getattr("Length")?)? {
        return Ok(length_keyword(py, value, "line_spacing")?.map(|twips| (twips, "exact")));
    }
    if !value.is_instance_of::<PyInt>() && !value.is_instance_of::<PyFloat>() {
        return Err(PyTypeError::new_err(message));
    }
    Ok(Some((
        oxoffice_docx::auto_line_spacing(crate::formatting::require_auto_line_spacing(value)?),
        "auto",
    )))
}

#[pymethods]
impl PyCustomProperties {
    fn __len__(&self, py: Python<'_>) -> usize {
        self.document.borrow(py).inner.custom_properties().len()
    }

    fn __iter__(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let properties = self
            .document
            .borrow(py)
            .inner
            .custom_properties()
            .iter()
            .cloned()
            .map(|property| PyCustomProperty { property })
            .collect::<Vec<_>>();
        PyTuple::new(py, properties)?
            .call_method0("__iter__")
            .map(Bound::unbind)
    }

    fn __getitem__(&self, py: Python<'_>, name: &str) -> PyResult<PyCustomProperty> {
        self.get(py, name)
            .ok_or_else(|| PyKeyError::new_err(name.to_owned()))
    }

    fn items<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let pairs = self
            .document
            .borrow(py)
            .inner
            .custom_properties()
            .iter()
            .map(|property| {
                (
                    property.name.clone().unwrap_or_default(),
                    PyCustomProperty {
                        property: property.clone(),
                    },
                )
            })
            .collect::<Vec<_>>();
        PyTuple::new(py, pairs)
    }

    fn __setitem__(&self, py: Python<'_>, name: &str, value: &Bound<'_, PyAny>) -> PyResult<()> {
        self.set(py, name, value)
    }
    fn __delitem__(&self, py: Python<'_>, name: &str) -> PyResult<()> {
        if name.is_empty() || !self.delete(py, name)? {
            Err(pyo3::exceptions::PyKeyError::new_err(name.to_owned()))
        } else {
            Ok(())
        }
    }
    fn __contains__(&self, py: Python<'_>, name: &str) -> bool {
        !name.is_empty() && self.get(py, name).is_some()
    }
    fn get(&self, py: Python<'_>, name: &str) -> Option<PyCustomProperty> {
        self.document
            .borrow(py)
            .inner
            .custom_property(name)
            .cloned()
            .map(|property| PyCustomProperty { property })
    }
    fn set(&self, py: Python<'_>, name: &str, value: &Bound<'_, PyAny>) -> PyResult<()> {
        use oxoffice_docx::CustomPropertyValue as V;
        if name.is_empty() {
            return Err(PyValueError::new_err(
                "custom property name must not be empty",
            ));
        }
        let authored = if value.is_instance_of::<PyBool>() {
            V::Bool(value.extract()?)
        } else if value.is_instance_of::<PyInt>() {
            V::I4(
                value
                    .extract()
                    .map_err(|_| PyOverflowError::new_err("custom integer must fit signed32"))?,
            )
        } else if value.is_instance_of::<PyFloat>() {
            let n: f64 = value.extract()?;
            if !n.is_finite() {
                return Err(PyValueError::new_err("custom float must be finite"));
            }
            V::R8(n)
        } else if value.is_instance_of::<PyString>() {
            V::Lpwstr(value.extract()?)
        } else if let Ok(v) = value.extract::<PyRef<'_, PyCustomFileTime>>() {
            V::FileTime(v.value.clone())
        } else {
            return Err(PyTypeError::new_err(
                "custom property requires str, bool, signed32 int, finite float or CustomFileTime",
            ));
        };
        let mut document = self.document.borrow_mut(py);
        let mut property = if let Some(existing) = document.inner.custom_property(name) {
            existing.clone()
        } else {
            let used: std::collections::BTreeSet<i32> = document
                .inner
                .custom_properties()
                .iter()
                .map(|p| p.pid)
                .collect();
            let pid = (2..=i32::MAX)
                .find(|pid| !used.contains(pid))
                .ok_or_else(|| PyOverflowError::new_err("no custom property PID available"))?;
            oxoffice_docx::CustomProperty {
                fmtid: "{D5CDD505-2E9C-101B-9397-08002B2CF9AE}".into(),
                pid,
                name: Some(name.into()),
                value: V::Empty,
            }
        };
        let authored = match (&property.value, authored) {
            (V::Lpstr(_), V::Lpwstr(v)) => V::Lpstr(v),
            (_, v) => v,
        };
        if property.value == authored {
            return Ok(());
        }
        property.value = authored;
        document
            .inner
            .set_custom_property(property)
            .map_err(|e| docx_to_pyerr(py, e))
    }
    fn delete(&self, py: Python<'_>, name: &str) -> PyResult<bool> {
        self.document
            .borrow_mut(py)
            .inner
            .remove_custom_property(name)
            .map(|p| p.is_some())
            .map_err(|e| docx_to_pyerr(py, e))
    }
}

// These concrete field selectors update the latest owned model, preserving its
// opaque child order and namespace metadata. Reading never creates a part.
type AppTextField = fn(&mut oxoffice_docx::AppProperties) -> &mut Option<String>;
impl PyAppProperties {
    fn text(&self, py: Python<'_>, field: AppTextField) -> Option<String> {
        let mut p = self
            .document
            .borrow(py)
            .inner
            .application_properties()?
            .clone();
        field(&mut p).clone()
    }
    fn set_text(&self, py: Python<'_>, field: AppTextField, value: Option<String>) -> PyResult<()> {
        let mut document = self.document.borrow_mut(py);
        let mut p = document
            .inner
            .application_properties()
            .cloned()
            .unwrap_or_default();
        if *field(&mut p) == value {
            return Ok(());
        }
        *field(&mut p) = value;
        document
            .inner
            .set_application_properties(p)
            .map_err(|e| docx_to_pyerr(py, e))
    }
}
#[pymethods]
impl PyAppProperties {
    #[getter]
    fn company(&self, py: Python<'_>) -> Option<String> {
        self.text(py, |p| &mut p.company)
    }
    #[setter]
    fn set_company(&self, py: Python<'_>, value: Option<String>) -> PyResult<()> {
        self.set_text(py, |p| &mut p.company, value)
    }
    #[getter]
    fn manager(&self, py: Python<'_>) -> Option<String> {
        self.text(py, |p| &mut p.manager)
    }
    #[setter]
    fn set_manager(&self, py: Python<'_>, value: Option<String>) -> PyResult<()> {
        self.set_text(py, |p| &mut p.manager, value)
    }
    #[getter]
    fn template(&self, py: Python<'_>) -> Option<String> {
        self.text(py, |p| &mut p.template)
    }
    #[setter]
    fn set_template(&self, py: Python<'_>, value: Option<String>) -> PyResult<()> {
        self.set_text(py, |p| &mut p.template, value)
    }
    #[getter]
    fn application(&self, py: Python<'_>) -> Option<String> {
        self.text(py, |p| &mut p.application)
    }
    #[setter]
    fn set_application(&self, py: Python<'_>, value: Option<String>) -> PyResult<()> {
        self.set_text(py, |p| &mut p.application, value)
    }
    #[getter]
    fn application_version(&self, py: Python<'_>) -> Option<String> {
        self.text(py, |p| &mut p.application_version)
    }
    #[setter]
    fn set_application_version(&self, py: Python<'_>, value: Option<String>) -> PyResult<()> {
        self.set_text(py, |p| &mut p.application_version, value)
    }
    #[getter]
    fn presentation_format(&self, py: Python<'_>) -> Option<String> {
        self.text(py, |p| &mut p.presentation_format)
    }
    #[setter]
    fn set_presentation_format(&self, py: Python<'_>, value: Option<String>) -> PyResult<()> {
        self.set_text(py, |p| &mut p.presentation_format, value)
    }
    #[getter]
    fn pages(&self, py: Python<'_>) -> Option<i32> {
        self.document
            .borrow(py)
            .inner
            .application_properties()
            .and_then(|p| p.pages)
    }
    #[getter]
    fn words(&self, py: Python<'_>) -> Option<i32> {
        self.document
            .borrow(py)
            .inner
            .application_properties()
            .and_then(|p| p.words)
    }
    #[getter]
    fn characters(&self, py: Python<'_>) -> Option<i32> {
        self.document
            .borrow(py)
            .inner
            .application_properties()
            .and_then(|p| p.characters)
    }
    #[getter]
    fn characters_with_spaces(&self, py: Python<'_>) -> Option<i32> {
        self.document
            .borrow(py)
            .inner
            .application_properties()
            .and_then(|p| p.characters_with_spaces)
    }
    #[getter]
    fn lines(&self, py: Python<'_>) -> Option<i32> {
        self.document
            .borrow(py)
            .inner
            .application_properties()
            .and_then(|p| p.lines)
    }
    #[getter]
    fn paragraphs(&self, py: Python<'_>) -> Option<i32> {
        self.document
            .borrow(py)
            .inner
            .application_properties()
            .and_then(|p| p.paragraphs)
    }
    #[getter]
    fn slides(&self, py: Python<'_>) -> Option<i32> {
        self.document
            .borrow(py)
            .inner
            .application_properties()
            .and_then(|p| p.slides)
    }
    #[getter]
    fn notes(&self, py: Python<'_>) -> Option<i32> {
        self.document
            .borrow(py)
            .inner
            .application_properties()
            .and_then(|p| p.notes)
    }
    #[getter]
    fn total_time(&self, py: Python<'_>) -> Option<i32> {
        self.document
            .borrow(py)
            .inner
            .application_properties()
            .and_then(|p| p.total_time)
    }
    #[getter]
    fn hidden_slides(&self, py: Python<'_>) -> Option<i32> {
        self.document
            .borrow(py)
            .inner
            .application_properties()
            .and_then(|p| p.hidden_slides)
    }
    #[getter]
    fn multimedia_clips(&self, py: Python<'_>) -> Option<i32> {
        self.document
            .borrow(py)
            .inner
            .application_properties()
            .and_then(|p| p.multimedia_clips)
    }
    #[getter]
    fn scale_crop(&self, py: Python<'_>) -> Option<bool> {
        self.document
            .borrow(py)
            .inner
            .application_properties()
            .and_then(|p| p.scale_crop)
    }
    #[getter]
    fn links_up_to_date(&self, py: Python<'_>) -> Option<bool> {
        self.document
            .borrow(py)
            .inner
            .application_properties()
            .and_then(|p| p.links_up_to_date)
    }
    #[getter]
    fn shared_document(&self, py: Python<'_>) -> Option<bool> {
        self.document
            .borrow(py)
            .inner
            .application_properties()
            .and_then(|p| p.shared_document)
    }
    #[getter]
    fn hyperlinks_changed(&self, py: Python<'_>) -> Option<bool> {
        self.document
            .borrow(py)
            .inner
            .application_properties()
            .and_then(|p| p.hyperlinks_changed)
    }
}

type CoreField = fn(&mut oxoffice_docx::CoreProperties) -> &mut Option<String>;

/// The longest text python-docx accepts for a core property.
const CORE_TEXT_LIMIT: usize = 255;

/// Read a W3CDTF date as python-docx does: a date and time, a date, a year and
/// month, or a year in the first nineteen characters, then an optional
/// `+hh:mm` or `-hh:mm` offset. Anything after the time that is not an offset,
/// such as `Z` or fractional seconds, is ignored.
fn w3cdtf_fields(value: &str) -> Option<([u32; 6], i32)> {
    let split = value
        .char_indices()
        .nth(19)
        .map_or(value.len(), |(index, _)| index);
    let (stamp, offset) = value.split_at(split);
    let bytes = stamp.as_bytes();
    let number = |start: usize, end: usize| {
        let digits = bytes.get(start..end)?;
        digits
            .iter()
            .all(u8::is_ascii_digit)
            .then(|| stamp[start..end].parse().ok())
            .flatten()
    };
    let separators = |expected: &[(usize, u8)]| {
        expected
            .iter()
            .all(|(index, byte)| bytes.get(*index) == Some(byte))
    };
    let fields = match bytes.len() {
        19 if separators(&[(4, b'-'), (7, b'-'), (10, b'T'), (13, b':'), (16, b':')]) => [
            number(0, 4)?,
            number(5, 7)?,
            number(8, 10)?,
            number(11, 13)?,
            number(14, 16)?,
            number(17, 19)?,
        ],
        10 if separators(&[(4, b'-'), (7, b'-')]) => {
            [number(0, 4)?, number(5, 7)?, number(8, 10)?, 0, 0, 0]
        }
        7 if separators(&[(4, b'-')]) => [number(0, 4)?, number(5, 7)?, 1, 0, 0, 0],
        4 => [number(0, 4)?, 1, 1, 0, 0, 0],
        _ => return None,
    };
    let minutes = if offset.len() == 6 {
        let offset = offset.as_bytes();
        let sign = match offset[0] {
            b'+' => 1,
            b'-' => -1,
            _ => return None,
        };
        if offset[3] != b':'
            || ![1, 2, 4, 5]
                .iter()
                .all(|index| offset[*index].is_ascii_digit())
        {
            return None;
        }
        let digit = |index: usize| i32::from(offset[index] - b'0');
        sign * ((digit(1) * 10 + digit(2)) * 60 + digit(4) * 10 + digit(5))
    } else {
        0
    };
    Some((fields, minutes))
}

/// Convert a stored W3CDTF date to an aware UTC `datetime`, or `None` when it
/// cannot be read, has an offset of a day or more, or falls outside the
/// `datetime` range once converted to UTC.
fn w3cdtf_to_datetime(py: Python<'_>, value: &str) -> PyResult<Option<Py<PyAny>>> {
    let Some(([year, month, day, hour, minute, second], offset)) = w3cdtf_fields(value) else {
        return Ok(None);
    };
    let datetime = py.import("datetime")?;
    let timezone = datetime.getattr("timezone")?;
    let utc = timezone.getattr("utc")?;
    let delta = datetime
        .getattr("timedelta")?
        .call((0, i64::from(offset) * 60), None)?;
    let constructor = datetime.getattr("datetime")?;
    let stamp = timezone
        .call1((delta,))
        .and_then(|zone| constructor.call1((year, month, day, hour, minute, second, 0, zone)))
        .and_then(|stamp| stamp.call_method1("astimezone", (utc,)));
    match stamp {
        Ok(stamp) => Ok(Some(stamp.unbind())),
        Err(error)
            if error.is_instance_of::<PyValueError>(py)
                || error.is_instance_of::<PyOverflowError>(py) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// Write a `datetime` as a UTC W3CDTF date. A naive value is taken as UTC, as
/// python-docx does, and an aware one is converted to UTC first.
fn datetime_to_w3cdtf(value: &Bound<'_, PyAny>) -> PyResult<String> {
    let datetime = value.py().import("datetime")?;
    if !value.is_instance(&datetime.getattr("datetime")?)? {
        return Err(PyTypeError::new_err(
            "a core property date must be a datetime.datetime",
        ));
    }
    let value = if value.call_method0("utcoffset")?.is_none() {
        value.clone()
    } else {
        value.call_method1(
            "astimezone",
            (datetime.getattr("timezone")?.getattr("utc")?,),
        )?
    };
    let field = |name: &str| value.getattr(name)?.extract::<u32>();
    Ok(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        field("year")?,
        field("month")?,
        field("day")?,
        field("hour")?,
        field("minute")?,
        field("second")?,
    ))
}

impl PyCoreProperties {
    fn read(&self, py: Python<'_>, field: CoreField) -> Option<String> {
        let document = self.document.borrow(py);
        let mut properties = document.inner.core_properties()?.clone();
        field(&mut properties).take()
    }

    fn write(&self, py: Python<'_>, field: CoreField, value: Option<String>) -> PyResult<()> {
        let mut document = self.document.borrow_mut(py);
        let mut properties = document
            .inner
            .core_properties()
            .cloned()
            .unwrap_or_default();
        let value = value.filter(|value| !value.is_empty());
        if *field(&mut properties) == value {
            return Ok(());
        }
        *field(&mut properties) = value;
        document
            .inner
            .set_core_properties(properties)
            .map_err(|error| docx_to_pyerr(py, error))
    }

    fn text(&self, py: Python<'_>, field: CoreField) -> String {
        self.read(py, field).unwrap_or_default()
    }

    fn set_text(&self, py: Python<'_>, field: CoreField, value: Option<String>) -> PyResult<()> {
        if value
            .as_deref()
            .is_some_and(|value| value.chars().count() > CORE_TEXT_LIMIT)
        {
            return Err(PyValueError::new_err(format!(
                "a core property holds at most {CORE_TEXT_LIMIT} characters"
            )));
        }
        self.write(py, field, value)
    }

    fn date(&self, py: Python<'_>, field: CoreField) -> PyResult<Option<Py<PyAny>>> {
        match self.read(py, field) {
            Some(value) => w3cdtf_to_datetime(py, &value),
            None => Ok(None),
        }
    }

    fn set_date(
        &self,
        py: Python<'_>,
        field: CoreField,
        value: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let value = value
            .filter(|value| !value.is_none())
            .map(datetime_to_w3cdtf)
            .transpose()?;
        self.write(py, field, value)
    }
}

#[pymethods]
impl PyCoreProperties {
    #[getter]
    fn author(&self, py: Python<'_>) -> String {
        self.text(py, |properties| &mut properties.creator)
    }
    #[setter]
    fn set_author(&self, py: Python<'_>, value: Option<String>) -> PyResult<()> {
        self.set_text(py, |properties| &mut properties.creator, value)
    }
    #[getter]
    fn category(&self, py: Python<'_>) -> String {
        self.text(py, |properties| &mut properties.category)
    }
    #[setter]
    fn set_category(&self, py: Python<'_>, value: Option<String>) -> PyResult<()> {
        self.set_text(py, |properties| &mut properties.category, value)
    }
    #[getter]
    fn comments(&self, py: Python<'_>) -> String {
        self.text(py, |properties| &mut properties.description)
    }
    #[setter]
    fn set_comments(&self, py: Python<'_>, value: Option<String>) -> PyResult<()> {
        self.set_text(py, |properties| &mut properties.description, value)
    }
    #[getter]
    fn content_status(&self, py: Python<'_>) -> String {
        self.text(py, |properties| &mut properties.content_status)
    }
    #[setter]
    fn set_content_status(&self, py: Python<'_>, value: Option<String>) -> PyResult<()> {
        self.set_text(py, |properties| &mut properties.content_status, value)
    }
    #[getter]
    fn created(&self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        self.date(py, |properties| &mut properties.created)
    }
    #[setter]
    fn set_created(&self, py: Python<'_>, value: Option<&Bound<'_, PyAny>>) -> PyResult<()> {
        self.set_date(py, |properties| &mut properties.created, value)
    }
    #[getter]
    fn identifier(&self, py: Python<'_>) -> String {
        self.text(py, |properties| &mut properties.identifier)
    }
    #[setter]
    fn set_identifier(&self, py: Python<'_>, value: Option<String>) -> PyResult<()> {
        self.set_text(py, |properties| &mut properties.identifier, value)
    }
    #[getter]
    fn keywords(&self, py: Python<'_>) -> String {
        self.text(py, |properties| &mut properties.keywords)
    }
    #[setter]
    fn set_keywords(&self, py: Python<'_>, value: Option<String>) -> PyResult<()> {
        self.set_text(py, |properties| &mut properties.keywords, value)
    }
    #[getter]
    fn language(&self, py: Python<'_>) -> String {
        self.text(py, |properties| &mut properties.language)
    }
    #[setter]
    fn set_language(&self, py: Python<'_>, value: Option<String>) -> PyResult<()> {
        self.set_text(py, |properties| &mut properties.language, value)
    }
    #[getter]
    fn last_modified_by(&self, py: Python<'_>) -> String {
        self.text(py, |properties| &mut properties.last_modified_by)
    }
    #[setter]
    fn set_last_modified_by(&self, py: Python<'_>, value: Option<String>) -> PyResult<()> {
        self.set_text(py, |properties| &mut properties.last_modified_by, value)
    }
    #[getter]
    fn last_printed(&self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        self.date(py, |properties| &mut properties.last_printed)
    }
    #[setter]
    fn set_last_printed(&self, py: Python<'_>, value: Option<&Bound<'_, PyAny>>) -> PyResult<()> {
        self.set_date(py, |properties| &mut properties.last_printed, value)
    }
    #[getter]
    fn modified(&self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        self.date(py, |properties| &mut properties.modified)
    }
    #[setter]
    fn set_modified(&self, py: Python<'_>, value: Option<&Bound<'_, PyAny>>) -> PyResult<()> {
        self.set_date(py, |properties| &mut properties.modified, value)
    }
    #[getter]
    fn revision(&self, py: Python<'_>) -> i64 {
        self.read(py, |properties| &mut properties.revision)
            .and_then(|value| value.trim().parse::<i64>().ok())
            .map_or(0, |value| value.max(0))
    }
    #[setter]
    fn set_revision(&self, py: Python<'_>, value: Option<i64>) -> PyResult<()> {
        if value.is_some_and(|value| value < 1) {
            return Err(PyValueError::new_err("revision must be a positive integer"));
        }
        self.write(
            py,
            |properties| &mut properties.revision,
            value.map(|value| value.to_string()),
        )
    }
    #[getter]
    fn subject(&self, py: Python<'_>) -> String {
        self.text(py, |properties| &mut properties.subject)
    }
    #[setter]
    fn set_subject(&self, py: Python<'_>, value: Option<String>) -> PyResult<()> {
        self.set_text(py, |properties| &mut properties.subject, value)
    }
    #[getter]
    fn title(&self, py: Python<'_>) -> String {
        self.text(py, |properties| &mut properties.title)
    }
    #[setter]
    fn set_title(&self, py: Python<'_>, value: Option<String>) -> PyResult<()> {
        self.set_text(py, |properties| &mut properties.title, value)
    }
    #[getter]
    fn version(&self, py: Python<'_>) -> String {
        self.text(py, |properties| &mut properties.version)
    }
    #[setter]
    fn set_version(&self, py: Python<'_>, value: Option<String>) -> PyResult<()> {
        self.set_text(py, |properties| &mut properties.version, value)
    }
}
