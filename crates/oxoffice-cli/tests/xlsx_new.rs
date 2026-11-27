//! `oxoffice xlsx new` makes the same workbook every time, never replaces a
//! file unless asked, and `oxoffice xlsx edit` applies an edit map in order,
//! all or nothing, refusing with the entry it cannot make.

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
