//! Equivalent writings of a workbook give the same result: open, the
//! sheets, their page setup, a save, and every typed part written again by
//! its writers (`specs/2026-11-25-equivalent-xml-writings.md`).

use oxoffice_xlsx::Workbook;
use oxoffice_xml_test::harness::{Input, Outcome, attempt, check_writings};

use crate::support::{corpus_root, corpus_workbooks, package_through_writers};

fn every_copy() -> bool {
    std::env::var("OXOFFICE_WORKFLOWS").is_ok_and(|value| value == "full")
}

/// Every producer's `review`, and the Strict copy; every workbook with
/// `OXOFFICE_WORKFLOWS=full`.
fn inputs() -> Vec<Input> {
    corpus_workbooks()
        .into_iter()
        .filter(|file| {
            every_copy()
                || file.path.ends_with("/review.xlsx")
                || file.path.ends_with("/strict.xlsx")
        })
        .map(|file| (file.path.clone(), file.bytes()))
        .collect()
}

fn open(bytes: &[u8]) -> Result<Workbook, String> {
    Workbook::from_bytes(bytes).map_err(|error| error.to_string())
}

fn save(bytes: &[u8]) -> Result<Vec<u8>, String> {
    open(bytes)?.to_bytes().map_err(|error| error.to_string())
}

fn run(bytes: &[u8]) -> Outcome {
    let mut outcome = Outcome::default();
    let opened = attempt(|| open(bytes));
    outcome.step(
        "open",
        opened.as_ref().map(|_| String::new()).map_err(Clone::clone),
    );
    let Ok(workbook) = opened else { return outcome };
    let sheets = workbook
        .sheets()
        .map(|sheet| format!("{} {:?} {:?}", sheet.name(), sheet.state(), sheet.kind()));
    outcome.step(
        "sheets",
        Ok(format!(
            "{:?} {:?}",
            workbook.date_system(),
            sheets.collect::<Vec<_>>()
        )),
    );
    let setups = workbook
        .sheets()
        .map(|sheet| sheet.page_setup().map(|setup| format!("{setup:?}")))
        .collect::<Result<Vec<_>, _>>();
    outcome.step(
        "page setup",
        setups
            .map(|setups| setups.join("\n"))
            .map_err(|e| e.to_string()),
    );
    outcome.saving(
        "save",
        &attempt(|| save(bytes).map(|saved| (saved, String::new()))),
    );
    let written =
        attempt(|| package_through_writers(bytes).map(|(saved, count)| (saved, count.to_string())));
    outcome.saving("writers", &written);
    outcome
}

#[test]
fn every_equivalent_writing_of_a_workbook_gives_the_same_result() {
    check_writings(
        &corpus_root().join("writings-known.toml"),
        inputs(),
        every_copy(),
        &run,
        &save,
    );
}
