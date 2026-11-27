//! LibreOffice opens the workbooks this crate makes and edits: it names
//! their sheets, shows their cells as the source did, and reads back the
//! widths, heights and panes written (checked in the workbook it saves).
//!
//! It runs where `soffice` is installed and says so when it is not; the
//! pull request that adds it was checked on a machine that has it. Each run
//! has its own profile, so no other LibreOffice is disturbed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use oxoffice_xlsx::{SheetKind, Workbook};

use crate::support::{child_of, corpus_bytes, package_parts, row_of};

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
    // producer's `review` and the Strict copy, as it was and as edited.
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
        let name = copy.replace('/', "-");
        let original = corpus_bytes(&format!("{copy}.xlsx"));
        files.push(Made {
            sheets: names(&Workbook::from_bytes(&original).unwrap()),
            edited: None,
            bytes: original.clone(),
            name: name.clone(),
        });
        files.push(edited(&name, &original));
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

    // The cells are as they were: each edited workbook exports the CSV of
    // its original, sheet by sheet.
    for made in files.iter().filter(|made| made.name.ends_with("-edited")) {
        let original = made.name.trim_end_matches("-edited");
        for sheet in made.sheets.iter().filter(|sheet| *sheet != "Added") {
            let read = |name: &str| fs::read(csv.join(format!("{name}-{sheet}.csv"))).unwrap();
            assert_eq!(read(&made.name), read(original), "{}: {sheet}", made.name);
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
