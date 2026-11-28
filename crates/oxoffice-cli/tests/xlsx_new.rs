//! `oxoffice xlsx new` makes the same workbook every time, never replaces a
//! file unless asked, `oxoffice xlsx edit` applies an edit map in order,
//! all or nothing, refusing with the entry it cannot make, and `set` and
//! `import-csv` write cells read by the strict rules of a CSV import.

use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "oxoffice-cli-xlsx-new-{}-{name}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch folder");
    dir
}

fn oxoffice(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oxoffice"))
        .arg("xlsx")
        .args(args)
        .output()
        .expect("oxoffice runs")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn part(workbook: &Path, name: &str) -> String {
    let mut zip = zip::ZipArchive::new(Cursor::new(fs::read(workbook).unwrap())).unwrap();
    let mut text = String::new();
    zip.by_name(name)
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    text
}

fn sheets(workbook: &Path) -> Vec<String> {
    let output = oxoffice(&["inspect", "--json", workbook.to_str().unwrap()]);
    assert!(output.status.success(), "{}", stderr(&output));
    let record: Value = serde_json::from_slice(&output.stdout).unwrap();
    record["sheets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|sheet| sheet["name"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn new_makes_the_same_workbook_every_time_and_replaces_nothing_unless_asked() {
    let dir = scratch("new");
    let path = |name: &str| dir.join(name);
    let run = |args: &[&str], name: &str| {
        let output = path(name);
        let mut all = vec!["new"];
        all.extend_from_slice(args);
        all.extend(["-o", output.to_str().unwrap()]);
        oxoffice(&all)
    };
    assert!(run(&[], "a.xlsx").status.success());
    assert!(run(&[], "b.xlsx").status.success());
    assert_eq!(
        fs::read(path("a.xlsx")).unwrap(),
        fs::read(path("b.xlsx")).unwrap()
    );
    assert_eq!(sheets(&path("a.xlsx")), ["Sheet1"]);

    assert!(
        run(&["--sheet", "Data", "--sheet", "Summary"], "named.xlsx")
            .status
            .success()
    );
    assert_eq!(sheets(&path("named.xlsx")), ["Data", "Summary"]);

    // A file is not replaced unless asked, and what is there stays.
    let before = fs::read(path("named.xlsx")).unwrap();
    let output = run(&[], "named.xlsx");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("output already exists"),
        "{}",
        stderr(&output)
    );
    assert_eq!(fs::read(path("named.xlsx")).unwrap(), before);
    assert!(run(&["--force"], "named.xlsx").status.success());
    assert_eq!(sheets(&path("named.xlsx")), ["Sheet1"]);

    // A refused name writes nothing.
    let output = run(&["--sheet", "A", "--sheet", "a"], "refused.xlsx");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("exists already"),
        "{}",
        stderr(&output)
    );
    assert!(!path("refused.xlsx").exists());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn edit_applies_its_map_in_order_and_all_or_nothing() {
    let dir = scratch("edit");
    let path = |name: &str| dir.join(name);
    let source = path("source.xlsx");
    assert!(
        oxoffice(&["new", "-o", source.to_str().unwrap()])
            .status
            .success()
    );
    let edit = |map: Value, name: &str, extra: &[&str]| {
        let map_file = path("map.json");
        fs::write(&map_file, map.to_string()).unwrap();
        let output = path(name);
        let mut args = vec![
            "edit",
            source.to_str().unwrap(),
            "--map",
            map_file.to_str().unwrap(),
        ];
        args.extend(["-o", output.to_str().unwrap()]);
        args.extend_from_slice(extra);
        oxoffice(&args)
    };

    let output = edit(
        json!([
            {"columns": "A:C", "width": 20},
            {"rows": "2:3", "height": 30},
            {"freeze": "B2"},
            {"add_sheet": "Data"},
            {"columns": "B", "width": 12.5},
            {"sheet": 1, "freeze": null},
            {"sheet": "sheet1", "freeze": "A2"},
            {"add_sheet": "R&D <\"2026\"> it's"},
        ]),
        "edited.xlsx",
        &["--json"],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let record: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        (record["operation"].as_str(), record["edits"].as_u64()),
        (Some("edit"), Some(8))
    );
    // A name with markup characters is written once and read back as given.
    assert_eq!(
        sheets(&path("edited.xlsx")),
        ["Sheet1", "Data", "R&D <\"2026\"> it's"]
    );
    let first = part(&path("edited.xlsx"), "xl/worksheets/sheet1.xml");
    assert!(
        first.contains(r#"<col min="1" max="3" width="20.7109375" customWidth="1"/>"#),
        "{first}"
    );
    assert!(
        first.contains(r#"<row r="2" ht="30" customHeight="1"/>"#),
        "{first}"
    );
    // The freeze at B2 was replaced by the one at A2, in the first sheet.
    assert!(
        first.contains(r#"<pane ySplit="1" topLeftCell="A2""#),
        "{first}"
    );
    // The entry after an added sheet edits that sheet.
    let second = part(&path("edited.xlsx"), "xl/worksheets/sheet2.xml");
    assert!(
        second.contains(r#"<col min="2" max="2" width="13.28515625" customWidth="1"/>"#),
        "{second}"
    );

    // A refusal names the entry, counted from 0, and writes nothing.
    for (map, message) in [
        (
            json!([{"columns": "A", "width": 5}, {"freeze": "A1"}]),
            "entry 1: A1 has no rows above it",
        ),
        (
            json!([{"add_sheet": "A"}, {"add_sheet": "a"}]),
            "entry 1: a sheet named",
        ),
        (
            json!([{"sheet": "Missing", "freeze": "B2"}]),
            "entry 0: no sheet is named \"Missing\"",
        ),
        (
            json!([{"sheet": 3, "freeze": "B2"}]),
            "entry 0: the workbook has 1 sheet(s), and none is number 3",
        ),
        (
            json!([{"columns": "A", "width": 0}]),
            "entry 0: a width of 0 would hide the columns",
        ),
        (json!([{"columns": "A"}]), "entry 0: width is a number"),
        (
            json!([{"widht": 1}]),
            "entry 0: an entry names its operation",
        ),
    ] {
        let output = edit(map.clone(), "refused.xlsx", &[]);
        assert_eq!(output.status.code(), Some(1), "{map}");
        assert!(
            stderr(&output).contains(message),
            "{map}: {}",
            stderr(&output)
        );
        assert!(!path("refused.xlsx").exists(), "{map}");
    }

    // Never the input, and a file that is there only with --force.
    let output = oxoffice(&[
        "edit",
        source.to_str().unwrap(),
        "--map",
        path("map.json").to_str().unwrap(),
        "-o",
        source.to_str().unwrap(),
        "--force",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("output is the input file"),
        "{}",
        stderr(&output)
    );
    let output = edit(json!([]), "edited.xlsx", &[]);
    assert!(
        stderr(&output).contains("output already exists"),
        "{}",
        stderr(&output)
    );
    assert!(
        edit(json!([]), "edited.xlsx", &["--force"])
            .status
            .success()
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The cells of a range of a workbook, as `cells --json` prints them.
fn cells(workbook: &Path, range: &str) -> Vec<Value> {
    let output = oxoffice(&["cells", "--json", workbook.to_str().unwrap(), range]);
    assert!(output.status.success(), "{}", stderr(&output));
    let record: Value = serde_json::from_slice(&output.stdout).unwrap();
    record["cells"].as_array().unwrap().clone()
}

#[test]
fn set_import_csv_and_the_edit_map_write_cells_values_formulas_and_styles() {
    let dir = scratch("set");
    let path = |name: &str| dir.join(name);
    let source = path("source.xlsx");
    assert!(
        oxoffice(&[
            "new",
            "--sheet",
            "Data",
            "--sheet",
            "Q1 data",
            "-o",
            source.to_str().unwrap()
        ])
        .status
        .success()
    );
    let run = |args: &[&str], input: &Path, name: &str| {
        let output = path(name);
        // The options before the positional arguments, as a shell user may
        // write them.
        let mut all = vec![args[0], input.to_str().unwrap()];
        all.extend(["-o", output.to_str().unwrap(), "--force"]);
        all.extend_from_slice(&args[1..]);
        let result = oxoffice(&all);
        assert!(result.status.success(), "{args:?}: {}", stderr(&result));
        output
    };
    // VALUE read as a CSV field, or as text, or a formula; a sheet prefix.
    let mut current = source.clone();
    for (index, args) in [
        &["set", "A1", "007"][..],
        &["set", "A2", "1.5"],
        &["set", "A3", "TRUE", "--text"],
        &["set", "A4", "2026-01-05"],
        &["set", "A5", "=SUM(A2:A2)*2", "--formula"],
        &["set", "'Q1 data'!B2", "x"],
        &["set", "B1", "--formula", "--spill", "SORT(A1:A3)"],
        &[
            "set",
            "H1",
            "SORT(A1:A3)",
            "--formula",
            "--spill-range",
            "H1:H3",
        ],
        &["set", "A6", "-5"],
        &["set", "--sheet", "Q1 data", "B5", "-7"],
        &["set", "A7", "--formula", "--", "-A2*2"],
    ]
    .iter()
    .enumerate()
    {
        current = run(args, &current, &format!("step{index}.xlsx"));
    }
    let read = cells(&current, "A1:B5");
    let field = |cell: &Value, key: &str| cell[key].clone();
    let by_ref = |reference: &str| {
        read.iter()
            .find(|cell| cell["ref"] == reference)
            .unwrap_or_else(|| panic!("{reference} in {read:?}"))
            .clone()
    };
    assert_eq!(field(&by_ref("A1"), "value"), json!("007"));
    assert_eq!(field(&by_ref("A2"), "value"), json!(1.5));
    assert_eq!(field(&by_ref("A3"), "value"), json!("TRUE"));
    assert_eq!(field(&by_ref("A4"), "text"), json!("2026-01-05"));
    assert_eq!(field(&by_ref("A5"), "formula"), json!("SUM(A2:A2)*2"));
    assert_eq!(field(&by_ref("B1"), "formula_kind"), json!("array"));
    assert_eq!(field(&by_ref("B1"), "array_range"), json!("B1"));
    let negative = cells(&current, "A6:A7");
    assert_eq!(negative[0]["value"], json!(-5.0));
    assert_eq!(negative[1]["formula"], json!("-A2*2"));
    let spilled = cells(&current, "H1");
    assert_eq!(spilled[0]["array_range"], json!("H1:H3"));
    let other = cells(&current, "'Q1 data'!B2:B5");
    assert_eq!(other[0]["value"], json!("x"));
    assert_eq!(other[1]["value"], json!(-7.0));

    // A refused formula writes nothing.
    let output = oxoffice(&[
        "set",
        current.to_str().unwrap(),
        "C1",
        "IF(A2>0;1;0)",
        "--formula",
        "-o",
        path("refused.xlsx").to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("write `,` between arguments"),
        "{}",
        stderr(&output)
    );
    assert!(!path("refused.xlsx").exists());

    // A CSV from a cell, typed or as text.
    let csv = path("data.csv");
    fs::write(&csv, "\u{feff}north,12\r\nsouth,007\r\n").unwrap();
    let imported = run(
        &["import-csv", csv.to_str().unwrap(), "--at", "D1"],
        &current,
        "imported.xlsx",
    );
    let read = cells(&imported, "D1:E2");
    let values: Vec<Value> = read.iter().map(|cell| cell["value"].clone()).collect();
    assert_eq!(
        values,
        [json!("north"), json!(12.0), json!("south"), json!("007")]
    );
    let as_text = run(
        &["import-csv", csv.to_str().unwrap(), "--text"],
        &current,
        "text.xlsx",
    );
    assert_eq!(cells(&as_text, "B1")[0]["value"], json!("12"));

    // The edit map's cell entries, and the cached values dropped.
    let map = path("cells.json");
    fs::write(
        &map,
        json!([
            {"sheet": "Data", "cell": "C1", "value": 3},
            {"cell": "C2", "formula": "C1*2"},
            {"cell": "C3", "date": "2026-01-05T08:30"},
            {"range": "C1:C3", "style": {"bold": true, "fill": "FFFF00"}},
            {"range": "C1", "number_format": "0.00"},
            {"range": "A1:A2", "clear": "contents"},
        ])
        .to_string(),
    )
    .unwrap();
    let edited = run(
        &[
            "edit",
            "--map",
            map.to_str().unwrap(),
            "--drop-cached-values",
        ],
        &current,
        "edited.xlsx",
    );
    let read = cells(&edited, "A1:C3");
    let c1 = read.iter().find(|cell| cell["ref"] == "C1").unwrap();
    assert_eq!(
        (c1["text"].clone(), c1["style"]["font"]["bold"].clone()),
        (json!("3.00"), json!(true))
    );
    let c3 = read.iter().find(|cell| cell["ref"] == "C3").unwrap();
    assert_eq!(c3["text"], json!("2026-01-05 08:30:00"));
    assert!(
        read.iter()
            .all(|cell| cell["ref"] != "A1" && cell["ref"] != "A2")
    );
    let sheet = part(&edited, "xl/worksheets/sheet1.xml");
    assert!(sheet.contains("<f>SUM(A2:A2)*2</f></c>"), "{sheet}");
    assert!(part(&edited, "xl/workbook.xml").contains(r#"fullCalcOnLoad="1""#));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_edit_map_writes_into_the_first_visible_worksheet_when_no_sheet_is_named() {
    use std::io::Write;

    let dir = scratch("visible");
    let source = dir.join("source.xlsx");
    let made = oxoffice(&[
        "new",
        "--sheet",
        "Config",
        "--sheet",
        "Data",
        "-o",
        source.to_str().unwrap(),
    ]);
    assert!(made.status.success(), "{}", stderr(&made));
    // The first sheet hidden, as a workbook keeps its settings.
    let mut zip = zip::ZipArchive::new(Cursor::new(fs::read(&source).unwrap())).unwrap();
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).unwrap();
        let name = entry.name().unwrap().into_owned();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).unwrap();
        if name == "xl/workbook.xml" {
            data = String::from_utf8(data)
                .unwrap()
                .replace(
                    r#"<sheet name="Config""#,
                    r#"<sheet state="hidden" name="Config""#,
                )
                .into_bytes();
        }
        out.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        out.write_all(&data).unwrap();
    }
    fs::write(&source, out.finish().unwrap().into_inner()).unwrap();
    let map = dir.join("map.json");
    fs::write(&map, json!([{"cell": "B2", "value": 3}]).to_string()).unwrap();
    let edited = dir.join("edited.xlsx");
    let output = oxoffice(&[
        "edit",
        source.to_str().unwrap(),
        "--map",
        map.to_str().unwrap(),
        "-o",
        edited.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(part(&edited, "xl/worksheets/sheet2.xml").contains(r#"<c r="B2"><v>3</v></c>"#));
    assert!(!part(&edited, "xl/worksheets/sheet1.xml").contains("B2"));
    let _ = fs::remove_dir_all(&dir);
}
