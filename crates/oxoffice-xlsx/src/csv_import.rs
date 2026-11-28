//! A CSV written into a sheet: the rectangle it covers from a cell, each
//! field typed strictly ([`CellInput::infer`]) or kept as text, an empty
//! field clearing its cell. The CSV is read by hand (RFC 4180: fields
//! between `,`, quoted with `"` when they hold one, a quote doubled, lines
//! ended by CRLF, LF or CR), from UTF-8.

use oxoffice_xlsx_oxml::CellRef;
use oxoffice_xlsx_oxml::reference::{MAX_COLUMNS, MAX_ROWS};

use crate::cell::{Reference, Value};
use crate::write::CellInput;
use crate::{Error, Result, WorksheetMut};

/// How a CSV is imported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CsvImport {
    /// Type each field: numbers, booleans and dates by
    /// [`CellInput::infer`]'s strict rules, the rest text. Off, every field
    /// is text.
    pub infer: bool,
}

impl Default for CsvImport {
    fn default() -> Self {
        CsvImport { infer: true }
    }
}

/// The fields of each line of `text`.
fn records(text: &str) -> Result<Vec<Vec<String>>> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut line = 1;
    let mut opened_at = 0;
    let mut characters = text.chars().peekable();
    // Whether the line holds anything yet: an empty last line is no record.
    let mut started = false;
    while let Some(character) = characters.next() {
        if quoted {
            match character {
                '"' if characters.peek() == Some(&'"') => {
                    characters.next();
                    field.push('"');
                }
                '"' => quoted = false,
                other => {
                    if other == '\n' {
                        line += 1;
                    }
                    field.push(other);
                }
            }
            continue;
        }
        match character {
            '"' if field.is_empty() => {
                quoted = true;
                started = true;
                opened_at = line;
            }
            ',' => {
                record.push(std::mem::take(&mut field));
                started = true;
            }
            '\r' | '\n' => {
                if character == '\r' && characters.peek() == Some(&'\n') {
                    characters.next();
                }
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
                started = false;
                line += 1;
            }
            other => {
                field.push(other);
                started = true;
            }
        }
    }
    if quoted {
        return Err(Error::InvalidArgument(format!(
            "the quoted field opened on line {opened_at} of the CSV has no closing quote"
        )));
    }
    if started || !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    Ok(records)
}

/// The text of `bytes`, UTF-8 with or without a byte order mark.
fn utf8(bytes: &[u8]) -> Result<&str> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    std::str::from_utf8(bytes).map_err(|error| {
        let at = error.valid_up_to();
        Error::InvalidArgument(format!(
            "the CSV is not UTF-8: byte {at} (0x{:02X}) starts no UTF-8 character; save it as UTF-8 first",
            bytes[at]
        ))
    })
}

impl WorksheetMut<'_> {
    /// Writes the CSV `csv` (UTF-8, a byte order mark skipped) from the
    /// cell `at` down and right: the rectangle it covers, a line shorter
    /// than the longest and an empty field clearing their cells. Each field
    /// typed by [`CellInput::infer`] (a leading `=` stays text: a CSV holds
    /// no formula), or kept as text without [`CsvImport::infer`].
    pub fn import_csv(&mut self, at: &str, csv: &[u8], options: CsvImport) -> Result<()> {
        let at = match Reference::parse(at)? {
            Reference::Cell(cell) => cell,
            _ => {
                return Err(Error::NotACell {
                    reference: at.to_owned(),
                });
            }
        };
        let records = records(utf8(csv)?)?;
        let height = records.len() as u64;
        let width = records.iter().map(Vec::len).max().unwrap_or(0) as u64;
        if u64::from(at.row) + height > u64::from(MAX_ROWS)
            || u64::from(at.column) + width > u64::from(MAX_COLUMNS)
        {
            return Err(Error::InvalidArgument(format!(
                "the CSV's {height} lines of {width} fields from {at} go past the last row or column of the sheet"
            )));
        }
        let mut inputs = Vec::new();
        for (row, record) in (at.row..).zip(&records) {
            for column in 0..width as u32 {
                let field = record.get(column as usize).map_or("", String::as_str);
                let input = if options.infer {
                    CellInput::infer(field)
                } else if field.is_empty() {
                    CellInput::Value(Value::Empty)
                } else {
                    CellInput::Value(Value::Text(field.to_owned()))
                };
                inputs.push((CellRef::new(row, at.column + column), input));
            }
        }
        self.write_inputs(inputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::write::DateValue;

    #[test]
    fn a_csv_reads_as_rfc_4180_writes_it() {
        for (text, expected) in [
            ("a,b\r\nc,d\r\n", vec![vec!["a", "b"], vec!["c", "d"]]),
            ("a,b\nc", vec![vec!["a", "b"], vec!["c"]]),
            ("\"a,\"\"b\"\"\nc\",d\r", vec![vec!["a,\"b\"\nc", "d"]]),
            ("a,,\n\nb", vec![vec!["a", "", ""], vec![""], vec!["b"]]),
            ("x\"y,z", vec![vec!["x\"y", "z"]]),
            ("", vec![]),
        ] {
            let read = records(text).unwrap();
            assert_eq!(read, expected, "{text:?}");
        }
        assert!(
            records("a,\"b\nc")
                .unwrap_err()
                .to_string()
                .contains("opened on line 1")
        );
        assert_eq!(utf8(b"\xEF\xBB\xBFa").unwrap(), "a");
        let error = utf8(b"ab\xE9c").unwrap_err().to_string();
        assert!(error.contains("byte 2 (0xE9)"), "{error}");
    }

    #[test]
    fn a_field_is_typed_strictly() {
        let number = |n: f64| CellInput::Value(Value::Number(n));
        let text = |t: &str| CellInput::Value(Value::Text(t.to_owned()));
        for (field, expected) in [
            ("42", number(42.0)),
            ("-0.5", number(-0.5)),
            ("0", number(0.0)),
            ("0.25", number(0.25)),
            ("1.5E+3", number(1500.0)),
            ("123456789012345", number(123_456_789_012_345.0)),
            ("1234567890123456", text("1234567890123456")),
            ("007", text("007")),
            (".5", text(".5")),
            ("1.", text("1.")),
            ("+1", text("+1")),
            ("1,000", text("1,000")),
            (" 1", text(" 1")),
            ("TRUE", CellInput::Value(Value::Boolean(true))),
            ("false", CellInput::Value(Value::Boolean(false))),
            (
                "2026-01-05",
                CellInput::Date(DateValue::Date {
                    year: 2026,
                    month: 1,
                    day: 5,
                }),
            ),
            (
                "2026-01-05T08:30",
                CellInput::Date(DateValue::DateTime(oxoffice_numfmt::DateTime {
                    year: 2026,
                    month: 1,
                    day: 5,
                    hour: 8,
                    minute: 30,
                    second: 0,
                    millisecond: 0,
                })),
            ),
            ("2026-02-30", text("2026-02-30")),
            (
                "1902-06-15",
                CellInput::Date(DateValue::Date {
                    year: 1902,
                    month: 6,
                    day: 15,
                }),
            ),
            ("08:30", text("08:30")),
            ("08:30:15", text("08:30:15")),
            ("2026-1-5", text("2026-1-5")),
            ("=SUM(A1:A3)", text("=SUM(A1:A3)")),
            ("", CellInput::Value(Value::Empty)),
        ] {
            assert_eq!(CellInput::infer(field), expected, "{field:?}");
        }
        // A date, a date and time, or a time alone (the edit map's `date`).
        for (text, parsed) in [
            (
                "08:30",
                Some("Time { hour: 8, minute: 30, second: 0, millisecond: 0 }"),
            ),
            ("2026-01-05", Some("Date { year: 2026, month: 1, day: 5 }")),
            (
                "1902-06-15T08:30",
                Some(
                    "DateTime(DateTime { year: 1902, month: 6, day: 15, hour: 8, minute: 30, second: 0, millisecond: 0 })",
                ),
            ),
            ("25:00", None),
            ("noon", None),
        ] {
            assert_eq!(
                DateValue::parse(text)
                    .map(|value| format!("{value:?}"))
                    .as_deref(),
                parsed,
                "{text}"
            );
        }
    }
}
