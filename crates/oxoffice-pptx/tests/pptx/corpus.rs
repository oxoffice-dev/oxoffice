//! Every PowerPoint deck of the corpus (`tests/corpus/files`) opens, saves
//! without an edit and reopens, with every package part it had, each byte for
//! byte: what the facade does not model is kept verbatim. A file the manifest
//! marks as a known failure may rewrite exactly the parts it lists, and must,
//! until its issue is fixed.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};

use oxoffice_pptx::Presentation;

use crate::support::{corpus_files, map_in_parallel};

/// Every ZIP entry of a package, by name, as stored.
fn entries(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("package is a ZIP");
    (0..zip.len())
        .map(|index| {
            let mut entry = zip.by_index(index).expect("ZIP entry reads");
            let mut data = Vec::new();
            entry.read_to_end(&mut data).expect("ZIP entry inflates");
            (entry.name().expect("ZIP entry name").into_owned(), data)
        })
        .collect()
}

/// What went wrong with a file: broken (it does not open, save or reopen,
/// or loses a part), or only some parts rewritten.
enum Failure {
    Broken(String),
    Rewritten(Vec<String>),
}

/// Opens, saves without an edit and reopens: every part is still there,
/// with the same bytes.
fn round_trip(original: &[u8]) -> Result<(), Failure> {
    let saved = Presentation::from_bytes(original)
        .and_then(|deck| deck.to_bytes())
        .map_err(|error| Failure::Broken(format!("open or save failed: {error}")))?;
    Presentation::from_bytes(&saved)
        .map_err(|error| Failure::Broken(format!("the saved file does not reopen: {error}")))?;
    let saved = entries(&saved);
    let (mut lost, mut changed) = (Vec::new(), Vec::new());
    for (name, bytes) in entries(original) {
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

/// The same parts, whatever the order they are listed in.
fn same_parts(found: &[String], listed: &[String]) -> bool {
    let (mut found, mut listed) = (found.to_vec(), listed.to_vec());
    found.sort();
    listed.sort();
    found == listed
}

#[test]
fn every_deck_saves_every_part_unchanged() {
    let files = corpus_files("pptx");
    let outcomes = map_in_parallel(&files, |file| round_trip(&file.bytes()));
    let mut failures = Vec::new();
    for (file, outcome) in files.iter().zip(outcomes) {
        let outcome = outcome.unwrap_or_else(|| Err(Failure::Broken("panicked".to_owned())));
        // A known failure may only rewrite the parts the manifest lists.
        match (outcome, file.known_failure) {
            (Ok(()), None) => {}
            (Err(Failure::Rewritten(parts)), Some(issue)) if !same_parts(&parts, &file.rewritten) => failures
                .push(format!(
                    "{}: a save rewrites {parts:?}, #{issue} allows only {:?}",
                    file.path, file.rewritten
                )),
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
