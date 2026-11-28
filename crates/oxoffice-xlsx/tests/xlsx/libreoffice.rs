//! LibreOffice opens the workbooks this crate makes and edits: it names
//! their sheets, shows their cells as the source did, and reads back the
//! widths, heights and panes written (checked in the workbook it saves);
//! every corpus copy with its cells, a style, formulas and a date written
//! opens, and, saved with `drop_cached_values`, LibreOffice calculates the
//! total an edited value feeds, which it does not do on its own.
//!
//! It runs where `soffice` is installed and says so when it is not; the
//! pull request that adds it was checked on a machine that has it. Each run
//! has its own profile, so no other LibreOffice is disturbed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use oxoffice_xlsx::{
    DateValue, FormulaOptions, SaveOptions, SheetKind, StylePatch, Value, Workbook,
};

use crate::support::{child_of, corpus_bytes, corpus_workbooks, package_parts, row_of};

fn soffice() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join("soffice"))
        .find(|candidate| candidate.is_file())
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("oxoffice-xlsx-lo-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch folder");
    dir
}

/// Converts `files` with one LibreOffice, into `out`.
fn convert(soffice: &Path, profile: &Path, filter: &str, files: &[PathBuf], out: &Path) {
    let status = Command::new(soffice)
        .arg(format!(
            "-env:UserInstallation=file://{}",
            profile.display()
        ))
        .args(["--headless", "--convert-to", filter, "--outdir"])
        .arg(out)
        .args(files)
        .output()
        .expect("soffice runs");
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
}

/// A workbook given to LibreOffice, the sheets it has, and the position of
/// the one whose widths, heights and panes were set.
struct Made {
    name: String,
    bytes: Vec<u8>,
    sheets: Vec<String>,
    edited: Option<usize>,
}

fn names(workbook: &Workbook) -> Vec<String> {
    workbook
        .sheets()
        .map(|sheet| sheet.name().to_owned())
        .collect()
}

/// `bytes` with a sheet added, and the first worksheet set up as a user
/// would.
fn edited(name: &str, bytes: &[u8]) -> Made {
    let mut workbook = Workbook::from_bytes(bytes).unwrap();
    let position = workbook
        .sheets()
        .find(|sheet| sheet.kind() == SheetKind::Worksheet)
        .unwrap()
        .position();
    workbook.add_sheet("Added").unwrap();
    let mut sheet = workbook.sheet_at_mut(position).unwrap();
    sheet.set_column_width("B:C", 20.0).unwrap();
    sheet.set_row_height("2:3", 30.0).unwrap();
    sheet.freeze_panes(Some("B2")).unwrap();
    Made {
        name: format!("{name}-edited"),
        bytes: workbook.to_bytes().unwrap(),
        sheets: names(&workbook),
        edited: Some(position),
    }
}

#[test]
fn libreoffice_opens_what_is_made_and_edited_and_reads_the_widths_heights_and_panes() {
    let Some(soffice) = soffice() else {
        eprintln!("skipped: soffice is not on the PATH");
        return;
    };
    let dir = scratch("convert");
    let (profile, input, saved, csv) = (
        dir.join("profile"),
        dir.join("in"),
        dir.join("saved"),
        dir.join("csv"),
    );
    for folder in [&input, &saved, &csv] {
        fs::create_dir_all(folder).unwrap();
    }

    // What LibreOffice is given: new workbooks, and a copy of each
    // producer's `review` and the Strict copy, edited.
    let new = Workbook::new();
    let mut named = Workbook::with_sheet_names(&["Data", "Summary"]).unwrap();
    named.add_sheet("Notes").unwrap();
    let mut data = named.sheet_mut("Data").unwrap();
    data.set_column_width("B:C", 20.0).unwrap();
    data.set_row_height("2:3", 30.0).unwrap();
    data.freeze_panes(Some("B2")).unwrap();
    let mut files = vec![
        Made {
            name: "new".into(),
            bytes: new.to_bytes().unwrap(),
            sheets: names(&new),
            edited: None,
        },
        Made {
            name: "named".into(),
            bytes: named.to_bytes().unwrap(),
            sheets: names(&named),
            edited: Some(0),
        },
    ];
    for copy in [
        "authored/review",
        "excel/review",
        "google/review",
        "libreoffice/review",
        "excel/strict",
    ] {
        let original = corpus_bytes(&format!("{copy}.xlsx"));
        files.push(edited(&copy.replace('/', "-"), &original));
    }
    let paths: Vec<PathBuf> = files
        .iter()
        .map(|made| {
            let path = input.join(format!("{}.xlsx", made.name));
            fs::write(&path, &made.bytes).unwrap();
            path
        })
        .collect();

    convert(
        &soffice,
        &profile,
        "xlsx:Calc MS Excel 2007 XML",
        &paths,
        &saved,
    );
    convert(
        &soffice,
        &profile,
        "csv:Text - txt - csv (StarCalc):44,34,76,1,,0,false,true,false,false,false,-1",
        &paths,
        &csv,
    );

    for made in &files {
        // LibreOffice names the sheets, and exports each.
        let reread = Workbook::open(saved.join(format!("{}.xlsx", made.name))).unwrap();
        assert_eq!(names(&reread), made.sheets, "{}", made.name);
        for sheet in &made.sheets {
            let file = csv.join(format!("{}-{sheet}.csv", made.name));
            assert!(file.is_file(), "{}: no CSV of {sheet}", made.name);
        }
    }

    // The widths, heights and panes come back in LibreOffice's own file.
    for made in &files {
        let Some(position) = made.edited else {
            continue;
        };
        let name = &made.name;
        let parts = package_parts(&fs::read(saved.join(format!("{name}.xlsx"))).unwrap());
        let part = &parts[&format!("xl/worksheets/sheet{}.xml", position + 1)];
        let cols =
            child_of(part, "cols").unwrap_or_else(|| panic!("{name}: LibreOffice reads no widths"));
        let width = |column: u32| {
            cols.elements_named("col")
                .find(|col| {
                    (col.tag.u32("min").unwrap()..=col.tag.u32("max").unwrap()).contains(&column)
                })
                .and_then(|col| col.tag.f64("width"))
        };
        for column in [2, 3] {
            let width = width(column).unwrap_or_else(|| panic!("{name}: column {column}"));
            assert!(
                (width - 20.71).abs() < 0.05,
                "{name}: column {column} is {width}"
            );
        }
        for row in [1, 2] {
            let height = row_of(part, row).and_then(|row| row.ht());
            assert!(
                height.is_some_and(|height| (height - 30.0).abs() < 0.05),
                "{name}: row {} is {height:?}",
                row + 1
            );
        }
        let views = child_of(part, "sheetViews").unwrap();
        let pane = views
            .elements_named("sheetView")
            .next()
            .and_then(|view| view.child("pane"))
            .unwrap_or_else(|| panic!("{name}: LibreOffice reads no panes"));
        for (attribute, expected) in [
            ("xSplit", "1"),
            ("ySplit", "1"),
            ("topLeftCell", "B2"),
            ("state", "frozen"),
        ] {
            assert_eq!(
                pane.tag.get(attribute).as_deref(),
                Some(expected),
                "{name}: {attribute}"
            );
        }
    }
    let _ = fs::remove_dir_all(&dir);
}

/// A value that feeds a formula in each family: the sheet of the value, the
/// cell written, the sheet and cell of the formula that adds it to others.
fn feeds(family: &str) -> (&'static str, &'static str, &'static str, &'static str) {
    match family {
        "budget" => ("Budget", "B4", "Budget", "F4"),
        "multi" => ("Sales", "C2", "Summary", "B4"),
        "dates1904" => ("Dates", "B2", "Dates", "C2"),
        "stream" => ("Log", "E2", "Log", "B30"),
        _ => ("Orders", "D2", "Orders", "D38"),
    }
}

fn number(workbook: &Workbook, sheet: &str, cell: &str) -> f64 {
    match workbook.sheet(sheet).unwrap().cell(cell).unwrap().value {
        Value::Number(number) => number,
        other => panic!("{sheet}!{cell} holds {other:?}"),
    }
}

#[test]
fn libreoffice_opens_every_edited_copy_and_calculates_what_an_edit_feeds_once_cached_values_drop() {
    let Some(soffice) = soffice() else {
        eprintln!("skipped: soffice is not on the PATH");
        return;
    };
    let dir = scratch("dropped");
    let (profile, input, saved) = (dir.join("profile"), dir.join("in"), dir.join("saved"));
    for folder in [&input, &saved] {
        fs::create_dir_all(folder).unwrap();
    }
    let options = SaveOptions {
        drop_cached_values: true,
        ..SaveOptions::default()
    };
    let bold = StylePatch {
        bold: Some(true),
        fill: Some("FFF2CC".to_owned()),
        border: Some("thin".to_owned()),
        ..StylePatch::default()
    };
    let mut expected = Vec::new();
    let mut paths = Vec::new();
    for file in corpus_workbooks() {
        let name = file
            .path
            .rsplit('/')
            .next()
            .unwrap()
            .trim_end_matches(".xlsx");
        let family = match name {
            "threads" | "strict" => "review",
            name => name,
        };
        let (sheet, written, total_sheet, total) = feeds(family);
        let mut workbook = Workbook::from_bytes(&file.bytes()).unwrap();
        let before = number(&workbook, sheet, written);
        let total_before = number(&workbook, total_sheet, total);
        let mut edited = workbook.sheet_mut(sheet).unwrap();
        edited.set_value(written, before + 1000.0).unwrap();
        edited.set_style("A1:B1", &bold).unwrap();
        edited.set_number_format(written, "0.0").unwrap();
        edited
            .set_formula("Z1", "1+1", FormulaOptions::default())
            .unwrap();
        edited
            .set_formula("Z2", "SUM(Z1:Z1)*3", FormulaOptions::default())
            .unwrap();
        edited
            .set_value(
                "Z3",
                DateValue::Date {
                    year: 2026,
                    month: 1,
                    day: 5,
                },
            )
            .unwrap();
        let copy = file.path.replace('/', "-");
        let path = input.join(&copy);
        fs::write(&path, workbook.to_bytes_with_options(options).unwrap()).unwrap();
        paths.push(path);
        expected.push((copy, sheet, total_sheet, total, total_before + 1000.0));
    }
    convert(
        &soffice,
        &profile,
        "xlsx:Calc MS Excel 2007 XML",
        &paths,
        &saved,
    );
    for (copy, sheet, total_sheet, total, value) in expected {
        let reread = Workbook::open(saved.join(&copy))
            .unwrap_or_else(|error| panic!("{copy}: LibreOffice saved no workbook: {error}"));
        let read = number(&reread, total_sheet, total);
        assert!(
            (read - value).abs() < 1e-6,
            "{copy}: LibreOffice reads {total} as {read}, not {value}"
        );
        assert_eq!(number(&reread, sheet, "Z2"), 6.0, "{copy}");
    }
    let _ = fs::remove_dir_all(&dir);
}
