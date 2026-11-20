//! Every PowerPoint deck of the corpus (`tests/corpus/files`) opens, saves
//! without an edit, reopens, and keeps every package part it had.

use std::collections::BTreeSet;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use oxoffice_opc::OpcPackage;
use oxoffice_pptx::Presentation;

fn corpus_files(extension: &str) -> Vec<PathBuf> {
    fn walk(dir: &Path, extension: &str, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("corpus folder is readable") {
            let path = entry.expect("corpus entry is readable").path();
            if path.is_dir() {
                walk(&path, extension, out);
            } else if path.extension().is_some_and(|ext| ext == extension) {
                out.push(path);
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/files");
    let mut files = Vec::new();
    walk(&root, extension, &mut files);
    files.sort();
    assert!(
        !files.is_empty(),
        "no .{extension} file in {}",
        root.display()
    );
    files
}

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
