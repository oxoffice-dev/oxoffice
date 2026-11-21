//! Template data: Python values checked and converted for a template render.

use super::*;

fn type_name(value: &Bound<'_, PyAny>) -> String {
    value
        .get_type()
        .name()
        .map(|name| name.to_string())
        .unwrap_or_else(|_| "object".to_owned())
}

fn is_mapping(value: &Bound<'_, PyAny>) -> PyResult<bool> {
    Ok(value.is_instance_of::<PyDict>())
}

/// The deepest nesting of dicts and lists template data may use.
const MAX_TEMPLATE_DATA_DEPTH: usize = 256;

/// Caller-facing template refusals are `ValueError`. Other native failures
/// keep the shared error class.
pub(super) fn template_render_pyerr(py: Python<'_>, error: oxoffice_docx::Error) -> PyErr {
    if let oxoffice_docx::Error::Other(message) = &error
        && (message.starts_with("invalid template:")
            || message.starts_with("unsupported template grammar version "))
    {
        return PyValueError::new_err(message.clone());
    }
    docx_to_pyerr(py, error)
}

/// Convert template data to JSON. `path` names the value in error messages,
/// and `open` holds the containers being converted, to refuse a cycle and
/// bound the depth.
pub(super) fn template_value(
    value: &Bound<'_, PyAny>,
    path: &str,
    open: &mut Vec<usize>,
) -> PyResult<serde_json::Value> {
    use serde_json::Value;

    if value.is_none() {
        return Ok(Value::Null);
    }
    if let Ok(flag) = value.cast::<PyBool>() {
        return Ok(Value::Bool(flag.is_true()));
    }
    if value.is_instance_of::<PyInt>() {
        if let Ok(number) = value.extract::<i64>() {
            return Ok(Value::from(number));
        }
        if let Ok(number) = value.extract::<u64>() {
            return Ok(Value::from(number));
        }
        return Err(PyValueError::new_err(format!(
            "template data {path} is an integer outside the 64-bit range; pass it as a str"
        )));
    }
    if let Ok(number) = value.cast::<PyFloat>() {
        return serde_json::Number::from_f64(number.value())
            .map(Value::Number)
            .ok_or_else(|| {
                PyValueError::new_err(format!(
                    "template data {path} is {}, which has no text form; pass a str instead",
                    number.value()
                ))
            });
    }
    if let Ok(text) = value.cast::<PyString>() {
        return Ok(Value::String(text.to_cow()?.into_owned()));
    }
    let container =
        is_mapping(value)? || value.is_instance_of::<PyList>() || value.is_instance_of::<PyTuple>();
    if container {
        let identity = value.as_ptr() as usize;
        if open.contains(&identity) {
            return Err(PyValueError::new_err(format!(
                "template data {path} contains itself; template data must be a tree"
            )));
        }
        if open.len() == MAX_TEMPLATE_DATA_DEPTH {
            return Err(PyValueError::new_err(format!(
                "template data {path} nests deeper than {MAX_TEMPLATE_DATA_DEPTH} dicts and lists"
            )));
        }
        open.push(identity);
        let converted = template_container(value, path, open);
        open.pop();
        return converted;
    }
    let datetime = value.py().import("datetime")?;
    let hint = if value.is_instance(&datetime.getattr("date")?)?
        || value.is_instance(&datetime.getattr("time")?)?
    {
        "format it first, for example with value.strftime(\"%d %B %Y\") or value.isoformat()"
    } else {
        "convert it to a str, int, float, bool, None, dict or list first"
    };
    Err(PyTypeError::new_err(format!(
        "template data {path} is a {}, which a template cannot render; {hint}",
        type_name(value)
    )))
}

/// Convert one dict or list of template data.
fn template_container(
    value: &Bound<'_, PyAny>,
    path: &str,
    open: &mut Vec<usize>,
) -> PyResult<serde_json::Value> {
    use serde_json::Value;

    if is_mapping(value)? {
        let mut object = serde_json::Map::new();
        for (key, item) in value.cast::<PyDict>()?.iter() {
            let Ok(key) = key.extract::<String>() else {
                return Err(PyTypeError::new_err(format!(
                    "template data {path} has a {} key; tag paths need str keys",
                    type_name(&key)
                )));
            };
            let value = template_value(&item, &format!("{path}[{key:?}]"), open)?;
            object.insert(key, value);
        }
        return Ok(Value::Object(object));
    }
    let mut array = Vec::new();
    let items = if let Ok(list) = value.cast::<PyList>() {
        list.iter().collect::<Vec<_>>()
    } else {
        value.cast::<PyTuple>()?.iter().collect::<Vec<_>>()
    };
    for (index, item) in items.iter().enumerate() {
        array.push(template_value(item, &format!("{path}[{index}]"), open)?);
    }
    Ok(Value::Array(array))
}
