//! The edit map of `oxoffice xlsx edit`: a JSON array of the edits to make,
//! applied in order, all or nothing.
//!
//! One entry does one thing, told by its key: `add_sheet`, `columns` (with
//! `width`), `rows` (with `height`) or `freeze`. An entry names the sheet it
//! edits with `sheet`, a name or a number from 1; without it, it edits the
//! sheet of the entry before it, and the first entry the first sheet. A
//! refusal names the entry, counted from 0.

use oxoffice_xlsx::Workbook;
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
    ColumnWidth { columns: String, width: f64 },
    RowHeight { rows: String, height: f64 },
    Freeze(Option<String>),
}

/// One entry of the map.
#[derive(Debug, PartialEq)]
pub(crate) struct Edit {
    sheet: Option<SheetRef>,
    operation: Operation,
}

/// The operations of an entry, by the key that names each.
const OPERATIONS: [&str; 4] = ["add_sheet", "columns", "rows", "freeze"];

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
                "an entry names its operation: add_sheet, columns and width, rows and height, or freeze"
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
            _ => (
                Operation::Freeze(match &entry["freeze"] {
                    Value::Null => None,
                    Value::String(cell) => Some(cell.clone()),
                    _ => return Err("freeze is a cell such as \"B2\", or null".to_owned()),
                }),
                &["sheet", "freeze"],
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
    let mut current = 0;
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
        ] {
            let error = parse(json).unwrap_err().to_string();
            assert!(error.contains(message), "{case}: {error}");
        }
    }
}
