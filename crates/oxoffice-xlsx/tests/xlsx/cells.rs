//! Cells read their value, formula, displayed text, kind, staleness and
//! date; every cell and comment of every corpus copy reads, and every
//! producer's copy of a family reads the cells of its authored
//! source (the same values, formulas as Excel displays them and displayed
//! texts at the cells of `cells.tsv`, the same used ranges and merges), what
//! a producer writes otherwise named with why in `cell-differences.tsv`, a
//! named difference that no longer holds failing. Bulk reads, CSV, Markdown
//! and refusals.

use std::collections::BTreeSet;

use oxoffice_xlsx::{CellRef, Error, Value, Workbook};

use crate::support::{corpus_bytes, corpus_workbooks, package_parts, zip_of};

/// The rows of a table (`tests/xlsx/*.tsv`): its lines but comments,
/// split at tabs, `\r`, `\n` and `\u{...}` read as their characters.
fn rows(table: &str) -> Vec<Vec<String>> {
    let unescape = |field: &str| {
        let mut text = String::new();
        let mut rest = field;
        while let Some(at) = rest.find('\\') {
            text.push_str(&rest[..at]);
            rest = &rest[at..];
            if let Some(after) = rest.strip_prefix("\\r") {
                text.push('\r');
                rest = after;
            } else if let Some(after) = rest.strip_prefix("\\n") {
                text.push('\n');
                rest = after;
            } else {
                let (code, after) = rest["\\u{".len()..].split_once('}').expect("\\u{...}");
                text.push(char::from_u32(u32::from_str_radix(code, 16).unwrap()).unwrap());
                rest = after;
            }
        }
        text.push_str(rest);
        text
    };
    table
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| line.split('\t').map(unescape).collect())
        .collect()
}

/// The family a corpus copy belongs to: its name, the threads and Strict
/// copies being Excel's and Google's `review`.
fn family(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap().trim_end_matches(".xlsx");
    match name {
        "threads" | "strict" => "review",
        name => name,
    }
}

fn value_text(value: &Value) -> String {
    match value {
        Value::Empty => "-".to_owned(),
        Value::Number(number) => format!("n:{number}"),
        Value::Text(text) => format!("s:{text}"),
        Value::Boolean(boolean) => format!("b:{boolean}"),
        Value::Error(error) => format!("e:{error}"),
    }
}

/// Whether a value reads as `expected`, numbers to the last digits a
/// producer writes.
fn same_value(value: &Value, expected: &str) -> bool {
    match (value, expected.strip_prefix("n:")) {
        (Value::Number(number), Some(expected)) => {
            let expected: f64 = expected.parse().unwrap();
            (number - expected).abs() <= 1e-9 * expected.abs().max(1.0)
        }
        _ => value_text(value) == expected,
    }
}

/// Every copy reads the cells of `cells.tsv`, as its family's source holds
/// them, but for the differences of `cell-differences.tsv`.
#[test]
fn every_copy_reads_the_cells_of_its_source() {
    let source_cells = rows(include_str!("cells.tsv"));
    let differences = rows(include_str!("cell-differences.tsv"));
    let mut failures = Vec::new();
    let mut named = BTreeSet::new();
    for file in corpus_workbooks() {
        let family = family(&file.path);
        let workbook = Workbook::from_bytes(&file.bytes()).unwrap();
        let source =
            Workbook::from_bytes(&corpus_bytes(&format!("authored/{family}.xlsx"))).unwrap();
        // Every cell and comment of every sheet reads.
        for sheet in workbook.sheets() {
            let read = sheet.cells("1:1048576").and(sheet.comments());
            if let Err(error) = read {
                failures.push(format!("{} {}: {error}", file.path, sheet.name()));
            }
        }
        for sheet in source.sheets() {
            let copy = workbook.sheet(sheet.name()).unwrap();
            let expected = (sheet.used_range().unwrap(), sheet.merged_ranges().unwrap());
            if (copy.used_range().unwrap(), copy.merged_ranges().unwrap()) != expected {
                failures.push(format!(
                    "{} {}: used range or merges",
                    file.path,
                    sheet.name()
                ));
            }
        }
        for row in source_cells.iter().filter(|row| row[0] == family) {
            let at = &row[1];
            let (sheet, cell) = at.split_once('!').unwrap();
            let read = workbook.sheet(sheet).unwrap().cell(cell).unwrap();
            let formula = read.formula.clone().unwrap_or_default();
            for (field, column, holds) in [
                ("value", 2, same_value(&read.value, &row[2])),
                ("formula", 3, formula == row[3]),
                ("text", 4, read.text == row[4]),
            ] {
                let difference = differences.iter().position(|other| {
                    other[0] == file.path && other[1] == *at && other[2] == field
                });
                let holds = match difference {
                    Some(index) => {
                        named.insert(index);
                        let expected = &differences[index][3];
                        match field {
                            "value" => same_value(&read.value, expected),
                            "formula" => formula == *expected,
                            _ => read.text == *expected,
                        }
                    }
                    None => holds,
                };
                if !holds {
                    let got = match field {
                        "value" => value_text(&read.value),
                        "formula" => formula.clone(),
                        _ => read.text.clone(),
                    };
                    failures.push(format!(
                        "{} {at} {field}: {got:?}, the source {:?}",
                        file.path, row[column]
                    ));
                }
            }
        }
    }
    for (index, row) in differences.iter().enumerate() {
        if !named.contains(&index) {
            failures.push(format!(
                "{} {} {}: names no cell read",
                row[0], row[1], row[2]
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    let budget = Workbook::from_bytes(&corpus_bytes("authored/budget.xlsx")).unwrap();
    let merges = budget.sheet("Budget").unwrap().merged_ranges().unwrap();
    let merges: Vec<String> = merges.iter().map(ToString::to_string).collect();
    assert_eq!(merges, ["A1:J1", "A17:J19"]);
}

/// `copy | sheet!cell | ` then what a test reads there, `-` for nothing.
fn fields(row: &str) -> (Workbook, String, Vec<Option<&str>>) {
    let mut fields = row.split(" | ");
    let workbook = Workbook::from_bytes(&corpus_bytes(fields.next().unwrap())).unwrap();
    let at = fields.next().unwrap().to_owned();
    let rest = fields
        .map(|field| (field != "-").then_some(field))
        .collect();
    (workbook, at, rest)
}

fn cell_at(workbook: &Workbook, at: &str) -> oxoffice_xlsx::Cell {
    let (sheet, cell) = at.split_once('!').unwrap();
    workbook.sheet(sheet).unwrap().cell(cell).unwrap()
}

#[test]
fn formulas_read_their_kind_range_stored_form_and_staleness() {
    // copy | sheet!cell | kind | array range | stored formula | stale
    for row in [
        "excel/review.xlsx | Orders!F2 | shared | - | D2*E2 | false",
        "excel/review.xlsx | Orders!F3 | shared | - | D3*E3 | false",
        "libreoffice/review.xlsx | Orders!F3 | - | - | D3*E3 | false",
        "authored/review.xlsx | Orders!F3 | shared | - | D3*E3 | true",
        "authored/review.xlsx | Orders!A1 | - | - | - | false",
        "excel/multi.xlsx | Q1 Forecast!D2 | array | D2:D7 | B2:B7*C2:C7 | false",
        "excel/multi.xlsx | Q1 Forecast!D3 | - | D2:D7 | - | false",
        "excel/multi.xlsx | Q1 Forecast!F2 | array | F2:F7 | _xlfn._xlws.SORT(B2:B7,,-1) | false",
        "authored/multi.xlsx | Q1 Forecast!F3 | - | F2:F7 | - | true",
        "excel/stream.xlsx | Log!B28 | - | - | TODAY() | true",
        "excel/stream.xlsx | Log!B30 | - | - | SUM(OFFSET(E2,0,0,5,1)) | true",
        "libreoffice/stream.xlsx | Log!B28 | - | - | TODAY() | false",
    ] {
        let (workbook, at, expected) = fields(row);
        let read = cell_at(&workbook, &at);
        let array_range = read.array_range.map(|range| range.to_string());
        let stale = read.stale.to_string();
        let read = [
            read.formula_kind.map(|kind| kind.as_str()),
            array_range.as_deref(),
            read.stored_formula.as_deref(),
            Some(stale.as_str()),
        ];
        assert_eq!(read.as_slice(), expected, "{row}");
    }
    let recalculation = |path| {
        Workbook::from_bytes(&corpus_bytes(path))
            .unwrap()
            .recalculation_needed()
    };
    assert!(recalculation("authored/review.xlsx") && !recalculation("excel/review.xlsx"));
}

#[test]
fn a_date_or_time_format_gives_a_datetime() {
    // copy | sheet!cell | date and time
    for row in [
        "excel/review.xlsx | Orders!C2 | 2026-09-01 00:00:00.000",
        "excel/strict.xlsx | Orders!C2 | 2026-09-01 00:00:00.000",
        "excel/dates1904.xlsx | Dates!B2 | 1904-01-01 00:00:00.000",
        "excel/dates1904.xlsx | Dates!B5 | 1904-01-01 13:45:30.000",
        "excel/dates1904.xlsx | Dates!B7 | -",
        "excel/budget.xlsx | Formats!C25 | -",
        "excel/budget.xlsx | Formats!C26 | 2026-11-21 14:35:00.000",
    ] {
        let (workbook, at, expected) = fields(row);
        let datetime = cell_at(&workbook, &at).datetime.map(|d| {
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
                d.year, d.month, d.day, d.hour, d.minute, d.second, d.millisecond
            )
        });
        assert_eq!(datetime.as_deref(), expected[0], "{row}");
    }
}

#[test]
fn a_bulk_read_gives_rows_of_values_and_the_cells_that_hold_one() {
    let workbook = Workbook::from_bytes(&corpus_bytes("excel/review.xlsx")).unwrap();
    let sheet = workbook.sheet("Orders").unwrap();
    let rows = sheet.values("A1:B2").unwrap();
    assert_eq!(
        rows,
        [
            vec![Value::Text("Item".into()), Value::Text("Supplier".into())],
            vec![
                Value::Text("Laptop dock".into()),
                Value::Text("Kestrel Tech".into())
            ],
        ]
    );
    // Whole columns and rows are clipped to the used range, A1:G40.
    let column = sheet.values("$F:F").unwrap();
    assert_eq!((column.len(), column[0].len()), (40, 1));
    assert_eq!(sheet.values("39:39").unwrap(), [vec![Value::Empty; 7]]);
    let cells = sheet.cells("38:40").unwrap();
    let addresses: Vec<String> = cells.iter().map(|cell| cell.address.to_string()).collect();
    assert_eq!(addresses, ["A38", "D38", "F38", "A40"]);
    let empty = Workbook::from_bytes(&corpus_bytes("excel/dates1904.xlsx")).unwrap();
    let dates = empty.sheet("Dates").unwrap();
    assert_eq!(
        dates.cell_at(1, 1).unwrap().address,
        CellRef::parse("B2").unwrap()
    );
    assert!(dates.cells("H:H").unwrap().is_empty());
}

#[test]
fn a_sheet_reads_as_csv_and_the_workbook_as_markdown() {
    let workbook = Workbook::from_bytes(&corpus_bytes("excel/dates1904.xlsx")).unwrap();
    let sheet = workbook.sheet("Dates").unwrap();
    assert_eq!(
        sheet.to_csv(&Default::default()).unwrap(),
        concat!(
            "Description,Value,Plus 30 days\r\n",
            "Serial zero,1904-01-01,1904-01-31\r\n",
            "A leap day,2000-02-29,2000-03-30\r\n",
            "Review date,2026-11-21,2026-12-21\r\n",
            "Time of day,13:45:30,\r\n",
            "Date and time,2026-11-21 14:35,\r\n",
            "Days between,44885,\r\n",
        )
    );
    let raw = oxoffice_xlsx::CsvOptions {
        raw: true,
        delimiter: ';',
        bom: true,
    };
    let csv = sheet.to_csv(&raw).unwrap();
    assert!(
        csv.starts_with("\u{feff}Description;Value;Plus 30 days\r\nSerial zero;0;30\r\n"),
        "{csv}"
    );
    assert!(csv.contains("Time of day;0.5732638888888889;\r\n"), "{csv}");
    assert_eq!(
        workbook.to_markdown().unwrap(),
        concat!(
            "## Dates\n\n",
            "|   | A | B | C |\n",
            "| --- | --- | --- | --- |\n",
            "| 1 | Description | Value | Plus 30 days |\n",
            "| 2 | Serial zero | 1904-01-01 | 1904-01-31 |\n",
            "| 3 | A leap day | 2000-02-29 | 2000-03-30 |\n",
            "| 4 | Review date | 2026-11-21 | 2026-12-21 |\n",
            "| 5 | Time of day | 13:45:30 |  |\n",
            "| 6 | Date and time | 2026-11-21 14:35 |  |\n",
            "| 7 | Days between | 44885 |  |\n",
        )
    );
    let stream = Workbook::from_bytes(&corpus_bytes("excel/stream.xlsx")).unwrap();
    let markdown = stream.to_markdown().unwrap();
    assert!(
        markdown.contains("| 33 | Line break |  |  | First line<br>second line |  |\n"),
        "{markdown}"
    );
    let csv = stream
        .sheet("Log")
        .unwrap()
        .to_csv(&Default::default())
        .unwrap();
    assert!(
        csv.contains("\r\nLine break,,,\"First line\r\nsecond line\",\r\n"),
        "{csv}"
    );
}

#[test]
fn a_sheet_without_cells_and_a_bad_reference_are_refused() {
    let review = corpus_bytes("excel/review.xlsx");
    let mut parts = package_parts(&review);
    let rels = parts.get_mut("xl/_rels/workbook.xml.rels").unwrap();
    *rels = String::from_utf8(rels.clone())
        .unwrap()
        .replace("relationships/worksheet", "relationships/chartsheet")
        .into_bytes();
    let entries: Vec<(&str, &[u8])> = parts
        .iter()
        .map(|(name, data)| (name.as_str(), data.as_slice()))
        .collect();
    let chart = Workbook::from_bytes(&zip_of(&entries)).unwrap();
    let chartsheet = chart.sheet("Orders").unwrap();
    assert_eq!(chartsheet.used_range().unwrap(), None);
    let workbook = Workbook::from_bytes(&review).unwrap();
    let orders = workbook.sheet("Orders").unwrap();
    let cases: [(&str, Error, &str); 4] = [
        (
            "a chartsheet",
            chartsheet.cell("A1").unwrap_err(),
            "sheet Orders is a chartsheet: it holds no cells to read",
        ),
        (
            "not a reference",
            orders.values("A0:B2").unwrap_err(),
            "`A0:B2` is not a cell or a range: write B3, A1:C10, A:C or 2:5",
        ),
        (
            "a range for a cell",
            orders.cell("A1:B2").unwrap_err(),
            "`A1:B2` is not a cell: write B3 or $B$3",
        ),
        (
            "a range too large",
            orders.values("A1:XFD1048576").unwrap_err(),
            "A1:XFD1048576 holds more than 10000000 cells: read a smaller range",
        ),
    ];
    for (case, error, message) in cases {
        assert_eq!(error.to_string(), message, "{case}");
    }
}
