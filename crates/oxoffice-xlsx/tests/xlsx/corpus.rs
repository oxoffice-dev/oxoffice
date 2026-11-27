//! Every workbook of the corpus (`tests/corpus/files`) opens, saves without
//! an edit and reopens with every package part it had, each byte for byte;
//! and every typed part written again by its writers reads the same model
//! and says the same, value spellings folded (`true` and `1`).

use std::collections::BTreeMap;
use std::io::Cursor;

use oxoffice_opc::OpcPackage;
use oxoffice_xlsx::Workbook;
use oxoffice_xlsx_oxml::worksheet::fixed_attributes;
use oxoffice_xml_test::{canonical_folded, first_difference};

use crate::support::{
    corpus_workbooks, map_in_parallel, package_parts, part_model, through_writers, typed_parts,
};

/// What went wrong with a file: broken (it does not open, save or reopen,
/// or loses a part), or only some parts rewritten.
enum Failure {
    Broken(String),
    Rewritten(Vec<String>),
}

fn round_trip(original: &[u8]) -> Result<(), Failure> {
    let workbook = Workbook::from_bytes(original)
        .map_err(|error| Failure::Broken(format!("open failed: {error}")))?;
    if workbook.sheets().len() == 0 {
        return Err(Failure::Broken("no sheet read".to_owned()));
    }
    let saved = workbook
        .to_bytes()
        .map_err(|error| Failure::Broken(format!("save failed: {error}")))?;
    Workbook::from_bytes(&saved)
        .map_err(|error| Failure::Broken(format!("the saved file does not reopen: {error}")))?;
    let saved = package_parts(&saved);
    let (mut lost, mut changed) = (Vec::new(), Vec::new());
    for (name, bytes) in package_parts(original) {
        match saved.get(&name) {
            None => lost.push(name),
            Some(saved) if *saved != bytes => changed.push(name),
            Some(_) => {}
        }
    }
    if !lost.is_empty() {
        Err(Failure::Broken(format!("parts lost on save: {lost:?}")))
    } else if !changed.is_empty() {
        Err(Failure::Rewritten(changed))
    } else {
        Ok(())
    }
}

fn same_parts(found: &[String], listed: &[String]) -> bool {
    let (mut found, mut listed) = (found.to_vec(), listed.to_vec());
    found.sort();
    listed.sort();
    found == listed
}

#[test]
fn every_workbook_saves_every_part_unchanged() {
    let files = corpus_workbooks();
    let outcomes = map_in_parallel(&files, |file| round_trip(&file.bytes()));
    let mut failures = Vec::new();
    for (file, outcome) in files.iter().zip(outcomes) {
        let outcome = outcome.unwrap_or_else(|| Err(Failure::Broken("panicked".to_owned())));
        match (outcome, file.known_failure) {
            (Ok(()), None) => {}
            (Err(Failure::Rewritten(parts)), Some(issue)) if !same_parts(&parts, &file.rewritten) => {
                failures.push(format!(
                    "{}: a save rewrites {parts:?}, #{issue} allows only {:?}",
                    file.path, file.rewritten
                ));
            }
            (Err(Failure::Rewritten(_)), Some(_)) => {}
            (Err(Failure::Broken(error)), _) => failures.push(format!("{}: {error}", file.path)),
            (Err(Failure::Rewritten(parts)), None) => failures.push(format!(
                "{}: parts rewritten by a save without an edit: {parts:?}",
                file.path
            )),
            (Ok(()), Some(issue)) => failures.push(format!(
                "{}: saves unchanged now, remove its known_failure (#{issue}) and rewritten lines from the manifest",
                file.path
            )),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Each typed part of `original` written again by its writers: the model
/// read from it is the model read from the source, and the two say the same
/// once value spellings are folded. Gives how many elements were written.
fn written_back(original: &[u8]) -> Result<usize, String> {
    let package =
        OpcPackage::from_reader(Cursor::new(original)).map_err(|error| error.to_string())?;
    let parts: BTreeMap<String, Vec<u8>> = package_parts(original);
    let mut written = 0;
    for (name, typed) in typed_parts(&package) {
        let source = &parts[&name];
        let (rewritten, count) =
            through_writers(typed, source).map_err(|e| format!("{name}: {e}"))?;
        written += count;
        let (before, after) = (part_model(typed, source)?, part_model(typed, &rewritten)?);
        if let Some(at) = first_difference(&before, &after) {
            return Err(format!("{name}: the model read back differs, {at}"));
        }
        // The attributes a row, a cell and a formula read in their fixed
        // writing, and only those, fold.
        let typed = |element: &str, attribute: &str| {
            fixed_attributes(element)
                .iter()
                .any(|(name, _)| *name == attribute)
        };
        let before = canonical_folded(source, &typed)?;
        let after = canonical_folded(&rewritten, &typed)?;
        if let Some(at) = first_difference(&before, &after) {
            return Err(format!("{name}: the part written says otherwise, {at}"));
        }
    }
    Ok(written)
}

#[test]
fn every_typed_part_reads_the_same_written_by_its_writers() {
    let files = corpus_workbooks();
    let outcomes = map_in_parallel(&files, |file| written_back(&file.bytes()));
    let mut failures = Vec::new();
    for (file, outcome) in files.iter().zip(outcomes) {
        match outcome.unwrap_or_else(|| Err("panicked".to_owned())) {
            Ok(0) => failures.push(format!("{}: no element written", file.path)),
            Ok(_) => {}
            Err(error) => failures.push(format!("{}: {error}", file.path)),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
