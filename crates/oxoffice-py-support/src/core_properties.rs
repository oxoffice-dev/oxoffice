//! Core property values shared by the Python bindings: the text limit and W3CDTF dates as `datetime`.

use pyo3::exceptions::{PyOverflowError, PyTypeError, PyValueError};
use pyo3::prelude::*;

/// The longest text python-docx accepts for a core property.
pub const CORE_TEXT_LIMIT: usize = 255;

/// Read a W3CDTF date, after trimming XML whitespace: a date and time, a date,
/// a year and month, or a year in the first nineteen characters. A date and
/// time may go on with fractional seconds, which are dropped, then `Z` or a
/// `+hh:mm` or `-hh:mm` offset (hours 00-23, minutes 00-59). Without either, it
/// is taken as UTC, as python-docx does. Any other ending makes the date
/// unreadable, so it is never read at a shifted instant.
fn w3cdtf_fields(value: &str) -> Option<([u32; 6], i32)> {
    let value = value.trim_matches(|character| matches!(character, ' ' | '\t' | '\r' | '\n'));
    let split = value
        .char_indices()
        .nth(19)
        .map_or(value.len(), |(index, _)| index);
    let (stamp, rest) = value.split_at(split);
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
    let offset = match rest.strip_prefix('.') {
        Some(fraction) => {
            let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return None;
            }
            &fraction[digits..]
        }
        None => rest,
    };
    let minutes = match offset.as_bytes() {
        [] | [b'Z' | b'z'] => 0,
        [sign @ (b'+' | b'-'), h1, h2, b':', m1, m2]
            if [h1, h2, m1, m2].iter().all(|digit| digit.is_ascii_digit()) =>
        {
            let digit = |byte: &u8| i32::from(byte - b'0');
            let (hours, minutes) = (digit(h1) * 10 + digit(h2), digit(m1) * 10 + digit(m2));
            if hours > 23 || minutes > 59 {
                return None;
            }
            let minutes = hours * 60 + minutes;
            if *sign == b'-' { -minutes } else { minutes }
        }
        _ => return None,
    };
    Some((fields, minutes))
}

/// Convert a stored W3CDTF date to an aware UTC `datetime`, or `None` when it
/// cannot be read, has an offset of a day or more, or falls outside the
/// `datetime` range once converted to UTC.
pub fn w3cdtf_to_datetime(py: Python<'_>, value: &str) -> PyResult<Option<Py<PyAny>>> {
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
pub fn datetime_to_w3cdtf(value: &Bound<'_, PyAny>) -> PyResult<String> {
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
