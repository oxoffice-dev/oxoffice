//! Every Word document of the corpus (`tests/corpus/files`) opens, saves
//! without an edit and reopens, with every package part it had, each byte for
//! byte: what the facade does not model is kept verbatim. A file the manifest
//! marks as a known failure may rewrite exactly the parts it lists, and must,
//! until its issue is fixed.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::io::{Cursor, Read};

use oxoffice_docx::Document;

use crate::support::{corpus_files, corpus_manifest, corpus_root, map_in_parallel};

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
    let saved = Document::from_bytes(original)
        .and_then(|mut doc| doc.to_bytes())
        .map_err(|error| Failure::Broken(format!("open or save failed: {error}")))?;
    Document::from_bytes(&saved)
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
fn every_document_saves_every_part_unchanged() {
    let files = corpus_files("docx");
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

/// Every docx and pptx file under `tests/corpus/files` has its manifest
/// entry and the other way round, within the budget of the test suite spec:
/// 1 MiB a file, 20 MiB in all.
#[test]
fn the_manifest_lists_every_corpus_file_within_the_budget() {
    let files = corpus_root().join("files");
    let mut on_disk = BTreeSet::new();
    let mut dirs = vec![files.clone()];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(&dir).expect("corpus folder is readable") {
            let path = entry.expect("corpus entry is readable").path();
            if path.is_dir() {
                dirs.push(path);
            } else if path
                .extension()
                .is_some_and(|ext| ext == "docx" || ext == "pptx")
            {
                let relative = path.strip_prefix(&files).unwrap();
                on_disk.insert(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let listed: BTreeSet<String> = corpus_manifest()
        .into_iter()
        .map(|file| file.path)
        .collect();
    let unlisted: Vec<_> = on_disk.difference(&listed).collect();
    let missing: Vec<_> = listed.difference(&on_disk).collect();
    assert!(
        unlisted.is_empty() && missing.is_empty(),
        "run tests/corpus/generate/manifest.py: files without an entry {unlisted:?}, entries without a file {missing:?}"
    );

    let mut total = 0;
    for path in &on_disk {
        let size = fs::metadata(files.join(path)).unwrap().len();
        assert!(size <= 1024 * 1024, "{path} is {size} bytes, over 1 MiB");
        total += size;
    }
    assert!(
        total <= 20 * 1024 * 1024,
        "the corpus is {total} bytes, over 20 MiB"
    );
}
