//! Every PowerPoint deck of the corpus (`tests/corpus/files`) opens, saves
//! without an edit, reopens, and keeps every package part it had.

use std::collections::BTreeSet;
use std::fs;
use std::io::Cursor;

use oxoffice_opc::OpcPackage;
use oxoffice_pptx::Presentation;

use crate::support::corpus_files;

fn part_names(bytes: &[u8]) -> BTreeSet<String> {
    let package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    package
        .zip_entries()
        .expect("package serialises")
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

#[test]
fn every_deck_round_trips_with_all_its_parts() {
    let mut failures = Vec::new();
    for path in corpus_files("pptx") {
        let name = path.display().to_string();
        let original = fs::read(&path).expect("corpus file is readable");
        let saved = match Presentation::from_bytes(&original).and_then(|deck| deck.to_bytes()) {
            Ok(saved) => saved,
            Err(error) => {
                failures.push(format!("{name}: open or save failed: {error}"));
                continue;
            }
        };
        if let Err(error) = Presentation::from_bytes(&saved) {
            failures.push(format!("{name}: the saved file does not reopen: {error}"));
            continue;
        }
        let lost: Vec<_> = part_names(&original)
            .difference(&part_names(&saved))
            .cloned()
            .collect();
        if !lost.is_empty() {
            failures.push(format!("{name}: parts lost on save: {lost:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
