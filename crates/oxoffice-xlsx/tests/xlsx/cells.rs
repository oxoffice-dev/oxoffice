//! Every producer's copy of a family reads the cells of its authored source:
//! the same values, formulas (as Excel displays them) and displayed texts at
//! the listed cells, and the same used ranges and merges. What a producer
//! writes otherwise is named, with why; a named difference that no longer
//! holds fails.

use std::collections::BTreeSet;

use oxoffice_xlsx::{CellRef, Error, Value, Workbook};

use crate::support::{corpus_bytes, corpus_workbooks, package_parts, zip_of};

/// `(family, sheet!cell, value, formula, text)`, as the authored source
/// holds them. A value is `n:` a number, `s:` a text, `b:` a boolean, `e:` an
/// error or `-` nothing; an empty formula is none.
const SOURCE: &[(&str, &str, &str, &str, &str)] = &[
    ("review", "Orders!A1", "s:Item", "", "Item"),
    ("review", "Orders!C2", "n:46266", "", "2026-09-01"),
    ("review", "Orders!F2", "n:305.32", "D2*E2", "305.32"),
    ("review", "Orders!F3", "n:12816.21", "D3*E3", "12,816.21"),
    ("review", "Orders!F37", "n:4204.8", "D37*E37", "4,204.80"),
    ("review", "Orders!D38", "n:747", "SUM(D2:D37)", "747"),
    (
        "review",
        "Orders!F38",
        "n:206459.11",
        "SUM(F2:F37)",
        "206,459.11",
    ),
    ("review", "Orders!B39", "-", "", ""),
    (
        "review",
        "Orders!A40",
        "s:Supplier price list",
        "",
        "Supplier price list",
    ),
    (
        "budget",
        "Budget!A1",
        "s:Operating budget 2027",
        "",
        "Operating budget 2027",
    ),
    (
        "budget",
        "Budget!F4",
        "n:37922.35",
        "SUM(B4:E4)",
        "$37,922.35",
    ),
    (
        "budget",
        "Budget!G4",
        "n:0.047925559132576366",
        "F4/$F$12",
        "4.8%",
    ),
    ("budget", "Budget!H7", "n:-3800", "", "-3,800"),
    (
        "budget",
        "Budget!F12",
        "n:791276.11",
        "SUM(Totals)",
        "$791,276.11",
    ),
    (
        "budget",
        "Budget!F13",
        "n:949531.33",
        "F12*(1+TaxRate)",
        "$949,531.33",
    ),
    ("budget", "Budget!B15", "n:0.2", "", "20%"),
    ("budget", "Ledger!A5", "n:46297", "", "2026-10-02"),
    (
        "budget",
        "Ledger!F5",
        "n:329.4",
        "Expenses[[#This Row],[Quantity]]*Expenses[[#This Row],[Unit price]]",
        "$329.40",
    ),
    (
        "budget",
        "Ledger!D17",
        "n:562",
        "SUBTOTAL(109,[Quantity])",
        "562",
    ),
    ("budget", "Formats!C3", "n:1234567.891", "", "1,234,567.89"),
    ("budget", "Formats!C4", "n:0.256", "", "26%"),
    ("budget", "Formats!C7", "n:123456.789", "", "1.23E+05"),
    ("budget", "Formats!C8", "n:3.75", "", "3 3/4"),
    ("budget", "Formats!C11", "n:-1234.5", "", "-$1,234.50"),
    ("budget", "Formats!C12", "n:1234.5", "", " $1,234.50 "),
    ("budget", "Formats!C13", "n:1234567", "", "1,235k"),
    ("budget", "Formats!C15", "n:2500000", "", "2.5M"),
    ("budget", "Formats!C17", "n:482", "", "00482"),
    ("budget", "Formats!C18", "s:00123", "", "00123"),
    ("budget", "Formats!C19", "n:46347", "", "11/21/26"),
    (
        "budget",
        "Formats!C21",
        "n:46347",
        "",
        "Saturday 21 November 2026",
    ),
    (
        "budget",
        "Formats!C24",
        "n:0.6076388888888888",
        "",
        "2:35 PM",
    ),
    ("budget", "Formats!C25", "n:1.0625", "", "25:30"),
    (
        "budget",
        "Formats!C26",
        "n:46347.60763888889",
        "",
        "11/21/26 14:35",
    ),
    (
        "multi",
        "Summary!B4",
        "n:1165",
        "SUMIF(Sales!$A$2:$A$25,A4,Sales!$C$2:$C$25)",
        "1165",
    ),
    (
        "multi",
        "Summary!D4",
        "n:0.05",
        "VLOOKUP(A4,Rates!$A$2:$B$5,2,FALSE)",
        "5.0%",
    ),
    (
        "multi",
        "Summary!F4",
        "b:true",
        "C4>='Q1 Forecast'!$H$2",
        "TRUE",
    ),
    (
        "multi",
        "Summary!G4",
        "s:North: 45,796",
        "A4&\": \"&TEXT(C4,\"#,##0\")",
        "North: 45,796",
    ),
    ("multi", "Sales!B2", "n:46023", "", "Jan 2026"),
    ("multi", "Sales!E2", "b:true", "", "TRUE"),
    (
        "multi",
        "Q1 Forecast!D2",
        "n:10680",
        "B2:B7*C2:C7",
        "10,680.00",
    ),
    ("multi", "Q1 Forecast!D3", "n:11960", "", "11,960.00"),
    ("multi", "Q1 Forecast!F2", "n:640", "SORT(B2:B7,,-1)", "640"),
    ("multi", "Q1 Forecast!F3", "n:120", "", "120"),
    ("multi", "Values!B2", "e:#DIV/0!", "1/0", "#DIV/0!"),
    ("multi", "Values!B5", "e:#REF!", "#REF!", "#REF!"),
    ("multi", "Values!B6", "e:#NAME?", "MissingName", "#NAME?"),
    ("multi", "Values!B7", "e:#NUM!", "SQRT(-1)", "#NUM!"),
    (
        "multi",
        "Values!B8",
        "e:#NULL!",
        "SUM(A2:A3 B2:B3)",
        "#NULL!",
    ),
    ("multi", "Values!B9", "b:true", "", "TRUE"),
    ("multi", "Values!B11", "b:true", "1<2", "TRUE"),
    (
        "multi",
        "Values!B12",
        "s:Plain, bold and italic, then red and large",
        "",
        "Plain, bold and italic, then red and large",
    ),
    (
        "multi",
        "Values!B15",
        "s:Tom & Jerry <\"fun\"> it's 5 > 3",
        "",
        "Tom & Jerry <\"fun\"> it's 5 > 3",
    ),
    (
        "multi",
        "Values!B19",
        "s:Launch \u{1f680} done \u{2705}",
        "",
        "Launch \u{1f680} done \u{2705}",
    ),
    ("dates1904", "Dates!B2", "n:0", "", "1904-01-01"),
    ("dates1904", "Dates!B3", "n:35123", "", "2000-02-29"),
    ("dates1904", "Dates!C4", "n:44915", "B4+30", "2026-12-21"),
    (
        "dates1904",
        "Dates!B5",
        "n:0.5732638888888889",
        "",
        "13:45:30",
    ),
    (
        "dates1904",
        "Dates!B6",
        "n:44885.60763888889",
        "",
        "2026-11-21 14:35",
    ),
    ("dates1904", "Dates!B7", "n:44885", "B4-B2", "44885"),
    ("stream", "Log!A1", "s:Time", "", "Time"),
    (
        "stream",
        "Log!A2",
        "n:46346.333333333336",
        "",
        "2026-11-20 08:00",
    ),
    (
        "stream",
        "Log!D2",
        "s:Started the nightly import. The product team extended the escalation path after a short consultation.",
        "",
        "Started the nightly import. The product team extended the escalation path after a short consultation.",
    ),
    ("stream", "Log!B28", "n:46347", "TODAY()", "2026-11-21"),
    (
        "stream",
        "Log!B30",
        "n:8746.58",
        "SUM(OFFSET(E2,0,0,5,1))",
        "8,746.58",
    ),
    (
        "stream",
        "Log!D33",
        "s:First line\r\nsecond line",
        "",
        "First line\r\nsecond line",
    ),
    (
        "stream",
        "Log!D34",
        "s:The text _x0041_ is not the letter A",
        "",
        "The text _x0041_ is not the letter A",
    ),
];

/// `(copy, sheet!cell, field, what the copy reads, why)`.
const DIFFERENCES: &[(&str, &str, &str, &str, &str)] = &[
    (
        "libreoffice/review.xlsx",
        "Orders!C2",
        "text",
        "2026/09/01",
        "LibreOffice respells yyyy-mm-dd as yyyy/mm/dd",
    ),
    (
        "excel/budget.xlsx",
        "Budget!F13",
        "value",
        "n:949531.332",
        "recalculated: the source caches it rounded",
    ),
    (
        "google/budget.xlsx",
        "Budget!F13",
        "value",
        "n:949531.332",
        "recalculated: the source caches it rounded",
    ),
    (
        "google/budget.xlsx",
        "Budget!F12",
        "formula",
        "SUM(Budget!Totals)",
        "Google qualifies a sheet-scoped name",
    ),
    (
        "google/budget.xlsx",
        "Ledger!F5",
        "formula",
        "Ledger!$D5*Ledger!$E5",
        "Google writes table references as A1 references",
    ),
    (
        "excel/budget.xlsx",
        "Ledger!D17",
        "formula",
        "SUBTOTAL(109,Expenses[Quantity])",
        "Excel names the table",
    ),
    (
        "google/budget.xlsx",
        "Ledger!D17",
        "formula",
        "SUBTOTAL(109,Expenses[Quantity])",
        "Google names the table",
    ),
    (
        "libreoffice/budget.xlsx",
        "Ledger!D17",
        "formula",
        "SUBTOTAL(109,[quantity])",
        "LibreOffice lowercases the column",
    ),
    (
        "libreoffice/budget.xlsx",
        "Ledger!A5",
        "text",
        "2026/10/02",
        "LibreOffice respells yyyy-mm-dd as yyyy/mm/dd",
    ),
    (
        "excel/budget.xlsx",
        "Formats!C12",
        "text",
        " 1,234.50  \u{20ac} ",
        "Excel in French rewrites built-in 44 in euros",
    ),
    (
        "google/budget.xlsx",
        "Formats!C19",
        "text",
        "21/11/2026",
        "Google writes D/M/YYYY for built-in 14",
    ),
    (
        "libreoffice/budget.xlsx",
        "Formats!C19",
        "text",
        "21/11/2026",
        "LibreOffice writes dd/mm/yyyy for built-in 14",
    ),
    (
        "libreoffice/budget.xlsx",
        "Formats!C26",
        "text",
        "21/11/2026 14:35",
        "LibreOffice writes dd/mm/yyyy h:mm for built-in 22",
    ),
    (
        "excel/multi.xlsx",
        "Summary!G4",
        "value",
        "s:North: 45796,150",
        "Excel in French computes TEXT with its separators",
    ),
    (
        "excel/multi.xlsx",
        "Summary!G4",
        "text",
        "North: 45796,150",
        "Excel in French computes TEXT with its separators",
    ),
    (
        "google/multi.xlsx",
        "Summary!G4",
        "value",
        "s:North: 45\u{202f}796",
        "Google computes TEXT with a narrow no-break space",
    ),
    (
        "google/multi.xlsx",
        "Summary!G4",
        "text",
        "North: 45\u{202f}796",
        "Google computes TEXT with a narrow no-break space",
    ),
    (
        "libreoffice/multi.xlsx",
        "Summary!D4",
        "formula",
        "VLOOKUP(A4,Rates!$A$2:$B$5,2,FALSE())",
        "LibreOffice writes FALSE as a call",
    ),
    (
        "libreoffice/multi.xlsx",
        "Sales!E2",
        "formula",
        "TRUE()",
        "LibreOffice writes booleans as formulas",
    ),
    (
        "libreoffice/multi.xlsx",
        "Values!B9",
        "formula",
        "TRUE()",
        "LibreOffice writes booleans as formulas",
    ),
    (
        "google/multi.xlsx",
        "Q1 Forecast!F2",
        "value",
        "s:#NAME?",
        "Google drops the dynamic array",
    ),
    (
        "google/multi.xlsx",
        "Q1 Forecast!F2",
        "formula",
        "_xlws.SORT(B2:B7,,-1)",
        "Google drops the dynamic array",
    ),
    (
        "google/multi.xlsx",
        "Q1 Forecast!F2",
        "text",
        "#NAME?",
        "Google drops the dynamic array",
    ),
    (
        "google/multi.xlsx",
        "Q1 Forecast!F3",
        "value",
        "-",
        "Google drops the spilled cells",
    ),
    (
        "google/multi.xlsx",
        "Q1 Forecast!F3",
        "text",
        "",
        "Google drops the spilled cells",
    ),
    (
        "google/multi.xlsx",
        "Values!B2",
        "value",
        "s:#DIV/0!",
        "Google writes formula errors as text",
    ),
    (
        "google/multi.xlsx",
        "Values!B5",
        "value",
        "s:#REF!",
        "Google writes formula errors as text",
    ),
    (
        "google/multi.xlsx",
        "Values!B6",
        "value",
        "s:#NAME?",
        "Google writes formula errors as text",
    ),
    (
        "google/multi.xlsx",
        "Values!B7",
        "value",
        "s:#NUM!",
        "Google writes formula errors as text",
    ),
    (
        "google/multi.xlsx",
        "Values!B8",
        "value",
        "s:#ERROR!",
        "Google writes #NULL! as the text #ERROR!",
    ),
    (
        "google/multi.xlsx",
        "Values!B8",
        "text",
        "#ERROR!",
        "Google writes #NULL! as the text #ERROR!",
    ),
    (
        "libreoffice/multi.xlsx",
        "Values!B5",
        "value",
        "e:#NAME?",
        "LibreOffice recalculates a lowercased #REF!",
    ),
    (
        "libreoffice/multi.xlsx",
        "Values!B5",
        "formula",
        "#ref!",
        "LibreOffice lowercases what it cannot read",
    ),
    (
        "libreoffice/multi.xlsx",
        "Values!B5",
        "text",
        "#NAME?",
        "LibreOffice recalculates a lowercased #REF!",
    ),
    (
        "libreoffice/multi.xlsx",
        "Values!B6",
        "formula",
        "missingname",
        "LibreOffice lowercases what it cannot read",
    ),
    (
        "libreoffice/multi.xlsx",
        "Values!B7",
        "value",
        "e:#VALUE!",
        "LibreOffice gives #VALUE! for SQRT(-1)",
    ),
    (
        "libreoffice/multi.xlsx",
        "Values!B7",
        "text",
        "#VALUE!",
        "LibreOffice gives #VALUE! for SQRT(-1)",
    ),
    (
        "google/dates1904.xlsx",
        "Dates!B2",
        "value",
        "n:1462",
        "Google drops date1904: serials of the 1900 system",
    ),
    (
        "google/dates1904.xlsx",
        "Dates!B3",
        "value",
        "n:36585",
        "Google drops date1904: serials of the 1900 system",
    ),
    (
        "google/dates1904.xlsx",
        "Dates!C4",
        "value",
        "n:46377",
        "Google drops date1904: serials of the 1900 system",
    ),
    (
        "google/dates1904.xlsx",
        "Dates!B6",
        "value",
        "n:46347.60763888889",
        "Google drops date1904: serials of the 1900 system",
    ),
    (
        "libreoffice/dates1904.xlsx",
        "Dates!B2",
        "text",
        "1904/01/01",
        "LibreOffice respells yyyy-mm-dd as yyyy/mm/dd",
    ),
    (
        "libreoffice/dates1904.xlsx",
        "Dates!B3",
        "text",
        "2000/02/29",
        "LibreOffice respells yyyy-mm-dd as yyyy/mm/dd",
    ),
    (
        "libreoffice/dates1904.xlsx",
        "Dates!C4",
        "text",
        "2026/12/21",
        "LibreOffice respells yyyy-mm-dd as yyyy/mm/dd",
    ),
    (
        "libreoffice/dates1904.xlsx",
        "Dates!B6",
        "text",
        "2026/11/21 14:35",
        "LibreOffice respells yyyy-mm-dd as yyyy/mm/dd",
    ),
    (
        "excel/stream.xlsx",
        "Log!B28",
        "value",
        "n:46353",
        "TODAY recalculated on the day of the save",
    ),
    (
        "excel/stream.xlsx",
        "Log!B28",
        "text",
        "2026-11-27",
        "TODAY recalculated on the day of the save",
    ),
    (
        "libreoffice/stream.xlsx",
        "Log!B28",
        "value",
        "n:46353",
        "TODAY recalculated on the day of the save",
    ),
    (
        "libreoffice/stream.xlsx",
        "Log!B28",
        "text",
        "2026/11/27",
        "TODAY recalculated, yyyy/mm/dd",
    ),
    (
        "libreoffice/stream.xlsx",
        "Log!A2",
        "text",
        "2026/11/20 08:00",
        "LibreOffice respells yyyy-mm-dd as yyyy/mm/dd",
    ),
    (
        "libreoffice/stream.xlsx",
        "Log!D33",
        "value",
        "s:First line\nsecond line",
        "LibreOffice drops the carriage return",
    ),
    (
        "libreoffice/stream.xlsx",
        "Log!D33",
        "text",
        "First line\nsecond line",
        "LibreOffice drops the carriage return",
    ),
];

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

#[test]
fn every_copy_reads_the_cells_of_its_source() {
    let mut failures = Vec::new();
    let mut named = BTreeSet::new();
    for file in corpus_workbooks() {
        let family = family(&file.path);
        let workbook = Workbook::from_bytes(&file.bytes()).unwrap();
        let source =
            Workbook::from_bytes(&corpus_bytes(&format!("authored/{family}.xlsx"))).unwrap();
        for sheet in source.sheets() {
            let copy = workbook.sheet(sheet.name()).unwrap();
            let (used, merges) = (sheet.used_range().unwrap(), sheet.merged_ranges().unwrap());
            if (copy.used_range().unwrap(), copy.merged_ranges().unwrap()) != (used, merges.clone())
            {
                failures.push(format!(
                    "{} {}: used range or merges",
                    file.path,
                    sheet.name()
                ));
            }
        }
        for (_, at, value, formula, text) in SOURCE.iter().filter(|row| row.0 == family) {
            let (sheet, cell) = at.split_once('!').unwrap();
            let read = workbook.sheet(sheet).unwrap().cell(cell).unwrap();
            let mut expected = |field: &str, source: &'static str| {
                DIFFERENCES
                    .iter()
                    .find(|row| row.0 == file.path && row.1 == *at && row.2 == field)
                    .map_or(source, |row| {
                        named.insert((row.0, row.1, row.2));
                        row.3
                    })
            };
            let formula_read = read.formula.clone().unwrap_or_default();
            if !same_value(&read.value, expected("value", value)) {
                failures.push(format!(
                    "{} {at}: value {}",
                    file.path,
                    value_text(&read.value)
                ));
            }
            if formula_read != expected("formula", formula) {
                failures.push(format!("{} {at}: formula {formula_read:?}", file.path));
            }
            if read.text != expected("text", text) {
                failures.push(format!("{} {at}: text {:?}", file.path, read.text));
            }
        }
    }
    for row in DIFFERENCES {
        if !named.contains(&(row.0, row.1, row.2)) {
            failures.push(format!("{} {} {}: names no cell read", row.0, row.1, row.2));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    let budget = Workbook::from_bytes(&corpus_bytes("authored/budget.xlsx")).unwrap();
    let merges: Vec<String> = budget
        .sheet("Budget")
        .unwrap()
        .merged_ranges()
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(merges, ["A1:J1", "A17:J19"]);
}

#[test]
fn formulas_read_their_kind_range_stored_form_and_staleness() {
    // (copy, sheet!cell, kind, array range, stored formula, stale)
    for (path, at, kind, range, stored, stale) in [
        (
            "excel/review.xlsx",
            "Orders!F2",
            Some("shared"),
            None,
            Some("D2*E2"),
            false,
        ),
        (
            "excel/review.xlsx",
            "Orders!F3",
            Some("shared"),
            None,
            Some("D3*E3"),
            false,
        ),
        (
            "libreoffice/review.xlsx",
            "Orders!F3",
            None,
            None,
            Some("D3*E3"),
            false,
        ),
        (
            "authored/review.xlsx",
            "Orders!F3",
            Some("shared"),
            None,
            Some("D3*E3"),
            true,
        ),
        ("authored/review.xlsx", "Orders!A1", None, None, None, false),
        (
            "excel/multi.xlsx",
            "Q1 Forecast!D2",
            Some("array"),
            Some("D2:D7"),
            Some("B2:B7*C2:C7"),
            false,
        ),
        (
            "excel/multi.xlsx",
            "Q1 Forecast!D3",
            None,
            Some("D2:D7"),
            None,
            false,
        ),
        (
            "excel/multi.xlsx",
            "Q1 Forecast!F2",
            Some("array"),
            Some("F2:F7"),
            Some("_xlfn._xlws.SORT(B2:B7,,-1)"),
            false,
        ),
        (
            "authored/multi.xlsx",
            "Q1 Forecast!F3",
            None,
            Some("F2:F7"),
            None,
            true,
        ),
        (
            "excel/stream.xlsx",
            "Log!B28",
            None,
            None,
            Some("TODAY()"),
            true,
        ),
        (
            "excel/stream.xlsx",
            "Log!B30",
            None,
            None,
            Some("SUM(OFFSET(E2,0,0,5,1))"),
            true,
        ),
        (
            "libreoffice/stream.xlsx",
            "Log!B28",
            None,
            None,
            Some("TODAY()"),
            false,
        ),
    ] {
        let workbook = Workbook::from_bytes(&corpus_bytes(path)).unwrap();
        let (sheet, cell) = at.split_once('!').unwrap();
        let read = workbook.sheet(sheet).unwrap().cell(cell).unwrap();
        assert_eq!(
            (
                read.formula_kind.map(|kind| kind.as_str()),
                read.array_range.map(|range| range.to_string()),
                read.stored_formula.as_deref(),
                read.stale,
            ),
            (kind, range.map(str::to_owned), stored, stale),
            "{path} {at}"
        );
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
    for (path, at, expected) in [
        (
            "excel/review.xlsx",
            "Orders!C2",
            Some("2026-09-01 00:00:00.000"),
        ),
        (
            "excel/strict.xlsx",
            "Orders!C2",
            Some("2026-09-01 00:00:00.000"),
        ),
        (
            "excel/dates1904.xlsx",
            "Dates!B2",
            Some("1904-01-01 00:00:00.000"),
        ),
        (
            "excel/dates1904.xlsx",
            "Dates!B5",
            Some("1904-01-01 13:45:30.000"),
        ),
        ("excel/dates1904.xlsx", "Dates!B7", None),
        ("excel/budget.xlsx", "Formats!C25", None),
        (
            "excel/budget.xlsx",
            "Formats!C26",
            Some("2026-11-21 14:35:00.000"),
        ),
    ] {
        let workbook = Workbook::from_bytes(&corpus_bytes(path)).unwrap();
        let (sheet, cell) = at.split_once('!').unwrap();
        let datetime = workbook
            .sheet(sheet)
            .unwrap()
            .cell(cell)
            .unwrap()
            .datetime
            .map(|d| {
                format!(
                    "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
                    d.year, d.month, d.day, d.hour, d.minute, d.second, d.millisecond
                )
            });
        assert_eq!(datetime.as_deref(), expected, "{path} {at}");
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
