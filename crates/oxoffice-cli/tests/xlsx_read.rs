//! `oxoffice xlsx text`, `csv`, `cells` and `comments` read what the facade
//! reads: displayed texts as tab-separated rows, Markdown tables or schema-1
//! records; a sheet as CSV; the cells of a range with their formulas and
//! styles; notes and threads. Sheets are named or numbered from 1.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/files")
        .join(name)
}

fn oxoffice(args: &[&str]) -> Output {
    let output = Command::new(env!("CARGO_BIN_EXE_oxoffice"))
        .args(args)
        .output()
        .expect("oxoffice runs");
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn stdout(args: &[&str]) -> String {
    String::from_utf8(oxoffice(args).stdout).unwrap()
}

#[test]
fn text_prints_displayed_texts_as_rows_markdown_or_records() {
    let dates = corpus("excel/dates1904.xlsx");
    let dates = dates.to_str().unwrap();
    assert!(stdout(&["xlsx", "text", dates]).starts_with(
        "# Dates\nDescription\tValue\tPlus 30 days\nSerial zero\t1904-01-01\t1904-01-31\n"
    ));
    let markdown = stdout(&["xlsx", "text", "--markdown", "--sheet", "1", dates]);
    assert!(
        markdown.starts_with("## Dates\n\n|   | A | B | C |\n| --- | --- | --- | --- |\n| 1 | Description | Value | Plus 30 days |\n"),
        "{markdown}"
    );
    let threads = corpus("excel/threads.xlsx");
    let record: Value = serde_json::from_str(&stdout(&[
        "xlsx",
        "text",
        "--json",
        threads.to_str().unwrap(),
    ]))
    .unwrap();
    let orders = &record["sheets"][0];
    assert_eq!(
        (
            &record["schema"],
            &record["date1904"],
            &orders["sheet"],
            &orders["used_range"]
        ),
        (&json!(1), &json!(false), &json!(1), &json!("A1:G40"))
    );
    let cells = orders["cells"].as_array().unwrap();
    assert_eq!(
        cells.iter().find(|cell| cell["ref"] == "F3").unwrap(),
        &json!({"ref": "F3", "type": "number", "value": 12816.21, "text": "12,816.21",
                "formula": "D3*E3", "stale": false})
    );
    assert_eq!(
        cells[0],
        json!({"ref": "A1", "type": "text", "value": "Item", "text": "Item"})
    );
    assert_eq!(orders["comments"][1]["author"], "Reviewer");
    // A cell of an array formula's range is stale with its formula.
    let multi = corpus("authored/multi.xlsx");
    let record: Value = serde_json::from_str(&stdout(&[
        "xlsx",
        "text",
        "--json",
        "--sheet",
        "Q1 Forecast",
        multi.to_str().unwrap(),
    ]))
    .unwrap();
    let cells = record["sheets"][0]["cells"].as_array().unwrap();
    assert_eq!(
        cells.iter().find(|cell| cell["ref"] == "D3").unwrap(),
        &json!({"ref": "D3", "type": "number", "value": 11960.0, "text": "11,960.00", "stale": true})
    );
    assert_eq!(
        orders["comments"][0]["replies"][1]["text"],
        "Done, see the updated list."
    );
}

#[test]
fn csv_writes_one_sheet_to_the_standard_output_or_a_file() {
    let multi = corpus("excel/multi.xlsx");
    let multi = multi.to_str().unwrap();
    // The first visible worksheet by default.
    assert!(stdout(&["xlsx", "csv", multi]).starts_with("Regional sales summary,,,,,,\r\n"));
    let raw = stdout(&[
        "xlsx",
        "csv",
        multi,
        "--sheet",
        "Sales",
        "--raw",
        "--delimiter",
        "tab",
    ]);
    assert!(
        raw.starts_with("Region\tMonth\tUnits\tRevenue\tShipped\r\nNorth\t46023\t"),
        "{raw}"
    );
    let folder = std::env::temp_dir().join(format!("oxoffice-xlsx-csv-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let out = folder.join("sales.csv");
    let out = out.to_str().unwrap();
    oxoffice(&["xlsx", "csv", multi, "--sheet", "2", "--bom", "-o", out]);
    let written = std::fs::read_to_string(out).unwrap();
    assert!(
        written.starts_with("\u{feff}Region,Month,Units"),
        "{written}"
    );
    let again = Command::new(env!("CARGO_BIN_EXE_oxoffice"))
        .args(["xlsx", "csv", multi, "-o", out])
        .output()
        .unwrap();
    assert_eq!(again.status.code(), Some(1));
    oxoffice(&["xlsx", "csv", multi, "-o", out, "--force"]);
    std::fs::remove_dir_all(&folder).unwrap();
}

#[test]
fn cells_prints_a_range_with_formulas_and_styles() {
    let multi = corpus("excel/multi.xlsx");
    let multi = multi.to_str().unwrap();
    assert_eq!(
        stdout(&["xlsx", "cells", multi, "'Q1 Forecast'!D2:F3"]),
        "D2\t10,680.00\t=B2:B7*C2:C7\nF2\t640\t=SORT(B2:B7,,-1)\nD3\t11,960.00\nF3\t120\n"
    );
    let record: Value = serde_json::from_str(&stdout(&[
        "xlsx", "cells", "--json", multi, "F2", "--sheet", "3",
    ]))
    .unwrap();
    let cell = &record["cells"][0];
    assert_eq!(
        (
            &cell["stored_formula"],
            &cell["formula_kind"],
            &cell["array_range"],
            &cell["number_format"]
        ),
        (
            &json!("_xlfn._xlws.SORT(B2:B7,,-1)"),
            &json!("array"),
            &json!("F2:F7"),
            &json!("General")
        )
    );
    let budget = corpus("excel/budget.xlsx");
    let record: Value = serde_json::from_str(&stdout(&[
        "xlsx",
        "cells",
        "--json",
        budget.to_str().unwrap(),
        "Budget!A1:B2",
    ]))
    .unwrap();
    let style = &record["cells"][0]["style"];
    assert_eq!(
        (
            &style["font"]["name"],
            &style["fill"]["foreground"]["rgb"],
            &record["merged"]
        ),
        (&json!("Cambria"), &json!("0F766E"), &json!(["A1:J1"]))
    );
}

#[test]
fn comments_print_notes_and_threads() {
    let threads = corpus("google/threads.xlsx");
    let threads = threads.to_str().unwrap();
    let text = stdout(&["xlsx", "comments", threads]);
    assert!(
        text.starts_with("Orders!C3 (thread) Corpus Author: The dates should follow the delivery calendar.\n  reply Corpus Author: Agreed"),
        "{text}"
    );
    assert!(
        text.contains("Orders!E5 (note) unknown: The price differs"),
        "{text}"
    );
    let record: Value =
        serde_json::from_str(&stdout(&["xlsx", "comments", "--json", threads])).unwrap();
    assert_eq!(
        record["sheets"][0]["comments"][4],
        json!({"ref": "F10", "kind": "thread", "author": "Corpus Author", "date": "2026-11-27T19:42:56.00",
               "text": "The total looks too high.", "resolved": true,
               "replies": [{"author": "Corpus Author", "date": "2026-11-27T19:43:08.00", "text": "It includes the freight."}]})
    );
}

#[test]
fn a_chartsheet_is_listed_by_its_kind_and_refused_for_its_cells() {
    use std::io::{Read, Write};

    // excel/review.xlsx with its only sheet related as a chartsheet.
    let mut source =
        zip::ZipArchive::new(std::fs::File::open(corpus("excel/review.xlsx")).unwrap()).unwrap();
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for index in 0..source.len() {
        let mut entry = source.by_index(index).unwrap();
        let name = entry.name().unwrap().into_owned();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).unwrap();
        if name == "xl/_rels/workbook.xml.rels" {
            data = String::from_utf8(data)
                .unwrap()
                .replace("relationships/worksheet", "relationships/chartsheet")
                .into_bytes();
        }
        out.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        out.write_all(&data).unwrap();
    }
    let folder = std::env::temp_dir().join(format!("oxoffice-xlsx-chart-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let chart = folder.join("chart.xlsx");
    std::fs::write(&chart, out.finish().unwrap().into_inner()).unwrap();
    let chart = chart.to_str().unwrap();
    // A read of its cells is refused.
    for args in [
        vec!["xlsx", "csv", chart],
        vec!["xlsx", "csv", chart, "--sheet", "1"],
        vec!["xlsx", "cells", chart, "A1", "--sheet", "1"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_oxoffice"))
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains("is a chartsheet: it holds no cells to read")
                || error.contains("the workbook has no visible worksheet"),
            "{args:?}: {error}"
        );
    }
    // The workbook's text lists it with its kind and no cells.
    assert_eq!(stdout(&["xlsx", "text", chart]), "# Orders (chartsheet)\n");
    assert_eq!(
        stdout(&["xlsx", "text", "--markdown", "--sheet", "1", chart]),
        "## Orders (chartsheet)\n"
    );
    let record: Value = serde_json::from_str(&stdout(&["xlsx", "text", "--json", chart])).unwrap();
    let sheet = &record["sheets"][0];
    assert_eq!(
        (
            &sheet["kind"],
            &sheet["used_range"],
            &sheet["cells"],
            &sheet["comments"]
        ),
        (&json!("chartsheet"), &Value::Null, &json!([]), &json!([]))
    );
    assert_eq!(stdout(&["xlsx", "comments", chart]), "");
    // inspect lists it, with no used range.
    let record: Value =
        serde_json::from_str(&stdout(&["xlsx", "inspect", "--json", chart])).unwrap();
    assert_eq!(
        (
            &record["sheets"][0]["kind"],
            &record["sheets"][0]["used_range"]
        ),
        (&json!("chartsheet"), &Value::Null)
    );
    std::fs::remove_dir_all(&folder).unwrap();
}
