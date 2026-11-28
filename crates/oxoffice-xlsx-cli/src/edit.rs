//! The edit map of `oxoffice xlsx edit`: a JSON array of the edits to make,
//! applied in order, all or nothing.
//!
//! One entry does one thing, told by its key: `value`, `formula` (with
//! `stored` and `spill`) or `date` of a `cell`; `style`, `number_format` or
//! `clear` of a `range`; `add_sheet`, `columns` (with `width`), `rows` (with
//! `height`) or `freeze`. An entry names the sheet it edits with `sheet`, a
//! name or a number from 1; without it, it edits the sheet of the entry
//! before it, and the first entry the first visible worksheet. A refusal
//! names the entry, counted from 0.

use oxoffice_xlsx::{
    CellInput, CellRange, Clear, DateValue, FormulaOptions, Spill, StylePatch, Workbook,
};
use serde_json::{Map, Value};

use crate::Result;

/// The sheet an entry names.
#[derive(Debug, PartialEq)]
enum SheetRef {
    Name(String),
    /// A position, from 1.
    Number(usize),
}

/// What one entry does.
#[derive(Debug, PartialEq)]
enum Operation {
    AddSheet(String),
    ColumnWidth {
        columns: String,
        width: f64,
    },
    RowHeight {
        rows: String,
        height: f64,
    },
    Freeze(Option<String>),
    /// A value, a formula or a date written into a cell.
    Write {
        cell: String,
        input: CellInput,
    },
    Style {
        range: String,
        patch: StylePatch,
    },
    Clear {
        range: String,
        what: Clear,
    },
}

/// One entry of the map.
#[derive(Debug, PartialEq)]
pub(crate) struct Edit {
    sheet: Option<SheetRef>,
    operation: Operation,
}

/// The operations of an entry, by the key that names each.
const OPERATIONS: [&str; 10] = [
    "add_sheet",
    "columns",
    "rows",
    "freeze",
    "value",
    "formula",
    "date",
    "style",
    "number_format",
    "clear",
];

pub(crate) fn parse(json: &str) -> Result<Vec<Edit>> {
    let value: Value =
        serde_json::from_str(json).map_err(|error| format!("the edit map is not JSON: {error}"))?;
    let Value::Array(entries) = value else {
        return Err("the edit map is an array of objects, one for each edit".into());
    };
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            parse_entry(entry).map_err(|message| format!("entry {index}: {message}").into())
        })
        .collect()
}

fn parse_entry(entry: &Value) -> std::result::Result<Edit, String> {
    let Value::Object(entry) = entry else {
        return Err("an entry is an object".to_owned());
    };
    let named: Vec<&str> = OPERATIONS
        .into_iter()
        .filter(|key| entry.contains_key(*key))
        .collect();
    let (operation, allowed): (Operation, &[&str]) = match named.as_slice() {
        [] => {
            return Err(
                "an entry names its operation: a cell and its value, formula or date; a range and its style, number_format or clear; add_sheet, columns and width, rows and height, or freeze"
                    .to_owned(),
            );
        }
        [one] => match *one {
            "add_sheet" => (
                Operation::AddSheet(text(entry, "add_sheet")?),
                &["add_sheet"],
            ),
            "columns" => (
                Operation::ColumnWidth {
                    columns: text(entry, "columns")?,
                    width: number(entry, "width")?,
                },
                &["sheet", "columns", "width"],
            ),
            "rows" => (
                Operation::RowHeight {
                    rows: text(entry, "rows")?,
                    height: number(entry, "height")?,
                },
                &["sheet", "rows", "height"],
            ),
            "freeze" => (
                Operation::Freeze(match &entry["freeze"] {
                    Value::Null => None,
                    Value::String(cell) => Some(cell.clone()),
                    _ => return Err("freeze is a cell such as \"B2\", or null".to_owned()),
                }),
                &["sheet", "freeze"],
            ),
            "value" => (
                Operation::Write {
                    cell: text(entry, "cell")?,
                    input: CellInput::Value(match &entry["value"] {
                        Value::Null => oxoffice_xlsx::Value::Empty,
                        Value::Bool(boolean) => oxoffice_xlsx::Value::Boolean(*boolean),
                        Value::Number(number) => oxoffice_xlsx::Value::Number(
                            number.as_f64().ok_or("value is a number Excel holds")?,
                        ),
                        Value::String(text) => oxoffice_xlsx::Value::Text(text.clone()),
                        _ => {
                            return Err(
                                "value is a number, a string, true, false or null".to_owned()
                            );
                        }
                    }),
                },
                &["sheet", "cell", "value"],
            ),
            "formula" => (
                Operation::Write {
                    cell: text(entry, "cell")?,
                    input: CellInput::Formula(
                        text(entry, "formula")?,
                        FormulaOptions {
                            stored: flag(entry, "stored")?.unwrap_or(false),
                            spill: spill(entry)?,
                        },
                    ),
                },
                &["sheet", "cell", "formula", "stored", "spill"],
            ),
            "date" => {
                let date = text(entry, "date")?;
                let Some(value) = DateValue::parse(&date) else {
                    return Err(format!(
                        "date is YYYY-MM-DD, with a time HH:MM[:SS] after a space or a T, or a time alone, not {date:?}"
                    ));
                };
                (
                    Operation::Write {
                        cell: text(entry, "cell")?,
                        input: CellInput::Date(value),
                    },
                    &["sheet", "cell", "date"],
                )
            }
            "style" => (
                Operation::Style {
                    range: text(entry, "range")?,
                    patch: style(&entry["style"])?,
                },
                &["sheet", "range", "style"],
            ),
            "number_format" => (
                Operation::Style {
                    range: text(entry, "range")?,
                    patch: StylePatch {
                        number_format: Some(text(entry, "number_format")?),
                        ..StylePatch::default()
                    },
                },
                &["sheet", "range", "number_format"],
            ),
            _ => (
                Operation::Clear {
                    range: text(entry, "range")?,
                    what: match text(entry, "clear")?.as_str() {
                        "contents" => Clear::Contents,
                        "formats" => Clear::Formats,
                        "all" => Clear::All,
                        other => {
                            return Err(format!(
                                "clear is \"contents\", \"formats\" or \"all\", not {other:?}"
                            ));
                        }
                    },
                },
                &["sheet", "range", "clear"],
            ),
        },
        several => {
            return Err(format!(
                "an entry does one thing, and this one has {}",
                several.join(" and ")
            ));
        }
    };
    if let Some(key) = entry.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("{key} does not go with {}", named[0]));
    }
    let sheet = match entry.get("sheet") {
        None => None,
        Some(Value::String(name)) => Some(SheetRef::Name(name.clone())),
        Some(Value::Number(number)) => match number.as_u64().filter(|number| *number >= 1) {
            Some(number) => Some(SheetRef::Number(number as usize)),
            None => return Err("sheet is a name, or a number from 1".to_owned()),
        },
        Some(_) => return Err("sheet is a name, or a number from 1".to_owned()),
    };
    Ok(Edit { sheet, operation })
}

fn text(entry: &Map<String, Value>, key: &str) -> std::result::Result<String, String> {
    match entry.get(key) {
        Some(Value::String(text)) => Ok(text.clone()),
        _ => Err(format!("{key} is a string")),
    }
}

/// Where `"spill"` says a formula spills: `true` from its cell over its
/// result, a range (`"J2:J5"`) over that range, `false` or nothing not at
/// all.
fn spill(entry: &Map<String, Value>) -> std::result::Result<Option<Spill>, String> {
    match entry.get("spill") {
        None | Some(Value::Bool(false)) => Ok(None),
        Some(Value::Bool(true)) => Ok(Some(Spill::Anchor)),
        Some(Value::String(range)) => CellRange::parse(range)
            .map(|range| Some(Spill::Range(range)))
            .ok_or_else(|| {
                format!("spill is true, false or a range such as \"J2:J5\", not {range:?}")
            }),
        Some(_) => Err("spill is true, false or a range such as \"J2:J5\"".to_owned()),
    }
}

fn flag(entry: &Map<String, Value>, key: &str) -> std::result::Result<Option<bool>, String> {
    match entry.get(key) {
        None => Ok(None),
        Some(Value::Bool(flag)) => Ok(Some(*flag)),
        Some(_) => Err(format!("{key} is true or false")),
    }
}

/// The style of a `style` entry: an object of the parts it changes.
fn style(value: &Value) -> std::result::Result<StylePatch, String> {
    let Value::Object(parts) = value else {
        return Err("style is an object such as {\"bold\": true, \"fill\": \"FFFF00\"}".to_owned());
    };
    let mut patch = StylePatch::default();
    for key in parts.keys() {
        match key.as_str() {
            "bold" => patch.bold = flag(parts, key)?,
            "italic" => patch.italic = flag(parts, key)?,
            "underline" => patch.underline = flag(parts, key)?,
            "strike" => patch.strike = flag(parts, key)?,
            "wrap_text" => patch.wrap_text = flag(parts, key)?,
            "font_size" => patch.font_size = Some(number(parts, key)?),
            "font_name" => patch.font_name = Some(text(parts, key)?),
            "font_color" => patch.font_color = Some(text(parts, key)?),
            "fill" => patch.fill = Some(text(parts, key)?),
            "border" => patch.border = Some(text(parts, key)?),
            "border_color" => patch.border_color = Some(text(parts, key)?),
            "horizontal" => patch.horizontal = Some(text(parts, key)?),
            "vertical" => patch.vertical = Some(text(parts, key)?),
            "number_format" => patch.number_format = Some(text(parts, key)?),
            other => {
                return Err(format!(
                    "{other} is not a part of a style: bold, italic, underline, strike, font_name, font_size, font_color, fill, border, border_color, horizontal, vertical, wrap_text or number_format"
                ));
            }
        }
    }
    Ok(patch)
}

fn number(entry: &Map<String, Value>, key: &str) -> std::result::Result<f64, String> {
    match entry.get(key).and_then(Value::as_f64) {
        Some(number) => Ok(number),
        None => Err(format!("{key} is a number")),
    }
}

/// The 0-based position of the sheet `reference` names.
fn position(workbook: &Workbook, reference: &SheetRef) -> std::result::Result<usize, String> {
    match reference {
        SheetRef::Name(name) => {
            if let Some(sheet) = workbook.sheet(name) {
                return Ok(sheet.position());
            }
            // A name is read first; a number written as text is a position.
            match name.parse::<usize>() {
                Ok(number) => position(workbook, &SheetRef::Number(number)),
                Err(_) => Err(format!("no sheet is named {name:?}")),
            }
        }
        SheetRef::Number(number) => {
            let count = workbook.sheets().len();
            if (1..=count).contains(number) {
                Ok(number - 1)
            } else {
                Err(format!(
                    "the workbook has {count} sheet(s), and none is number {number}"
                ))
            }
        }
    }
}

/// Makes `edits` in `workbook`, in order; the first that is refused ends it,
/// named by its place in the map.
pub(crate) fn apply(workbook: &mut Workbook, edits: &[Edit]) -> Result<()> {
    // The first visible worksheet, as the commands that read cells take it.
    let mut current = workbook
        .sheets()
        .find(|sheet| {
            sheet.kind() == oxoffice_xlsx::SheetKind::Worksheet
                && sheet.state() == oxoffice_xlsx::SheetState::Visible
        })
        .map_or(0, |sheet| sheet.position());
    for (index, edit) in edits.iter().enumerate() {
        let refused = |message: String| format!("entry {index}: {message}");
        if let Some(reference) = &edit.sheet {
            current = position(workbook, reference).map_err(refused)?;
        }
        match &edit.operation {
            Operation::AddSheet(name) => {
                current = workbook
                    .add_sheet(name)
                    .map_err(|e| refused(e.to_string()))?
                    .position();
            }
            operation => {
                let mut sheet = workbook
                    .sheet_at_mut(current)
                    .ok_or_else(|| refused("the workbook has no sheet to edit".to_owned()))?;
                match operation {
                    Operation::ColumnWidth { columns, width } => {
                        sheet.set_column_width(columns, *width)
                    }
                    Operation::RowHeight { rows, height } => sheet.set_row_height(rows, *height),
                    Operation::Freeze(cell) => sheet.freeze_panes(cell.as_deref()),
                    Operation::Write { cell, input } => sheet.set_value(cell, input.clone()),
                    Operation::Style { range, patch } => sheet.set_style(range, patch),
                    Operation::Clear { range, what } => sheet.clear(range, *what),
                    Operation::AddSheet(_) => unreachable!("added above"),
                }
                .map_err(|e| refused(e.to_string()))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_does_one_thing_and_a_refusal_names_it() {
        let parsed = parse(
            r#"[{"add_sheet": "Summary"},
                {"sheet": 2, "columns": "A:C", "width": 20},
                {"rows": "2:5", "height": 30.5},
                {"sheet": "Data", "freeze": "B2"},
                {"freeze": null}]"#,
        )
        .unwrap();
        assert_eq!(parsed.len(), 5);
        assert_eq!(parsed[1].sheet, Some(SheetRef::Number(2)));
        assert_eq!(
            parsed[3].operation,
            Operation::Freeze(Some("B2".to_owned()))
        );
        assert_eq!(parsed[4].operation, Operation::Freeze(None));

        let cells = parse(
            r#"[{"sheet": "Data", "cell": "B2", "value": 3},
                {"cell": "B3", "value": "3"},
                {"cell": "B4", "value": null},
                {"cell": "D2", "formula": "B2*C2", "spill": true},
                {"cell": "F2", "formula": "SORT(A2:A5)", "spill": "F2:F5"},
                {"cell": "E2", "date": "2026-01-05"},
                {"range": "A1:F1", "style": {"bold": true, "fill": "FFFF00", "font_size": 12}},
                {"range": "C2:C8", "number_format": "0.00"},
                {"range": "A20:B30", "clear": "contents"}]"#,
        )
        .unwrap();
        let operations: Vec<&Operation> = cells.iter().map(|edit| &edit.operation).collect();
        let write = |cell: &str, input| Operation::Write {
            cell: cell.to_owned(),
            input,
        };
        assert_eq!(
            operations[..4],
            [
                &write("B2", CellInput::from(3.0)),
                &write("B3", CellInput::from("3")),
                &write("B4", CellInput::Value(oxoffice_xlsx::Value::Empty)),
                &write(
                    "D2",
                    CellInput::Formula(
                        "B2*C2".to_owned(),
                        FormulaOptions {
                            stored: false,
                            spill: Some(Spill::Anchor)
                        }
                    )
                ),
            ]
        );
        assert_eq!(
            operations[4],
            &write(
                "F2",
                CellInput::Formula(
                    "SORT(A2:A5)".to_owned(),
                    FormulaOptions {
                        stored: false,
                        spill: Some(Spill::Range(CellRange::parse("F2:F5").unwrap()))
                    }
                )
            )
        );
        assert!(matches!(
            operations[5],
            Operation::Write {
                input: CellInput::Date(_),
                ..
            }
        ));
        assert_eq!(
            operations[6],
            &Operation::Style {
                range: "A1:F1".to_owned(),
                patch: StylePatch {
                    bold: Some(true),
                    fill: Some("FFFF00".to_owned()),
                    font_size: Some(12.0),
                    ..StylePatch::default()
                }
            }
        );
        assert_eq!(
            operations[8],
            &Operation::Clear {
                range: "A20:B30".to_owned(),
                what: Clear::Contents
            }
        );

        for (case, json, message) in [
            ("not JSON", "[", "not JSON"),
            ("not an array", "{}", "an array of objects"),
            ("not an object", "[1]", "entry 0: an entry is an object"),
            (
                "no operation",
                r#"[{"sheet": 1}]"#,
                "entry 0: an entry names its operation",
            ),
            (
                "two operations",
                r#"[{"add_sheet": "A"}, {"columns": "A", "width": 1, "rows": "1", "height": 1}]"#,
                "entry 1: an entry does one thing, and this one has columns and rows",
            ),
            (
                "a key of another operation",
                r#"[{"freeze": "B2", "width": 3}]"#,
                "entry 0: width does not go with freeze",
            ),
            (
                "no width",
                r#"[{"columns": "A"}]"#,
                "entry 0: width is a number",
            ),
            (
                "a text width",
                r#"[{"columns": "A", "width": "20"}]"#,
                "entry 0: width is a number",
            ),
            (
                "a number as columns",
                r#"[{"columns": 3, "width": 2}]"#,
                "entry 0: columns is a string",
            ),
            (
                "freeze as a number",
                r#"[{"freeze": 2}]"#,
                "entry 0: freeze is a cell",
            ),
            (
                "sheet 0",
                r#"[{"sheet": 0, "freeze": "B2"}]"#,
                "entry 0: sheet is a name, or a number from 1",
            ),
            (
                "sheet true",
                r#"[{"sheet": true, "freeze": "B2"}]"#,
                "entry 0: sheet is a name, or a number from 1",
            ),
            (
                "a value and a formula",
                r#"[{"cell": "A1", "value": 1, "formula": "1"}]"#,
                "entry 0: an entry does one thing, and this one has value and formula",
            ),
            (
                "a value without its cell",
                r#"[{"value": 1}]"#,
                "entry 0: cell is a string",
            ),
            (
                "a value that is a list",
                r#"[{"cell": "A1", "value": [1]}]"#,
                "entry 0: value is a number, a string, true, false or null",
            ),
            (
                "a date that is not one",
                r#"[{"cell": "A1", "date": "5/1/2026"}]"#,
                "entry 0: date is YYYY-MM-DD",
            ),
            (
                "a style part that is not one",
                r#"[{"range": "A1", "style": {"colour": "FF0000"}}]"#,
                "entry 0: colour is not a part of a style",
            ),
            (
                "a clear of something else",
                r#"[{"range": "A1", "clear": "values"}]"#,
                "entry 0: clear is \"contents\", \"formats\" or \"all\"",
            ),
            (
                "a spill that is no range",
                r#"[{"cell": "A1", "formula": "1", "spill": "down"}]"#,
                "entry 0: spill is true, false or a range such as \"J2:J5\", not \"down\"",
            ),
            (
                "stored as text",
                r#"[{"cell": "A1", "formula": "1", "stored": "yes"}]"#,
                "entry 0: stored is true or false",
            ),
        ] {
            let error = parse(json).unwrap_err().to_string();
            assert!(error.contains(message), "{case}: {error}");
        }
    }
}
