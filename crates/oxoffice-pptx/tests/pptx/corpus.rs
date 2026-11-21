//! Every PowerPoint deck of the corpus (`tests/corpus/files`) opens, saves
//! without an edit and reopens, with every package part it had, each byte for
//! byte: what the facade does not model is kept verbatim. A file the manifest
//! marks as a known failure may rewrite exactly the parts it lists, and must,
//! until its issue is fixed.

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};

use oxoffice_pptx::{Presentation, ValidationIssue, validate_presentation_file};

use crate::support::{corpus_files, corpus_root, map_in_parallel};

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

/// The corpus deck `deck` with `character` put right after the first
/// `marker` of `part`, and the one-based line and column it lands at.
fn deck_with_a_character(
    deck: &str,
    part: &str,
    marker: &str,
    character: char,
) -> (Vec<u8>, usize, usize) {
    let bytes = std::fs::read(corpus_root().join("files").join(deck)).unwrap();
    let mut source = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let mut place = None;
    for index in 0..source.len() {
        let mut entry = source.by_index(index).unwrap();
        let name = entry.name().unwrap().into_owned();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        if name == part {
            let xml = String::from_utf8(bytes).unwrap();
            let at = xml.find(marker).unwrap_or_else(|| panic!("{deck} {part}")) + marker.len();
            let before = xml[..at].replace("\r\n", "\n").replace('\r', "\n");
            let line = before.matches('\n').count() + 1;
            let column = before.rsplit('\n').next().unwrap().chars().count() + 1;
            place = Some((line, column));
            bytes = format!("{}{character}{}", &xml[..at], &xml[at..]).into_bytes();
        }
        out.start_file(name, zip::write::SimpleFileOptions::DEFAULT)
            .unwrap();
        out.write_all(&bytes).unwrap();
    }
    let (line, column) = place.unwrap_or_else(|| panic!("{deck} has {part}"));
    (out.finish().unwrap().into_inner(), line, column)
}

// PowerPoint (for Mac 16.115) cannot read such a part either: it repairs the
// deck and drops what the part holds, the slide's shapes, the notes or every
// document property. Refusing the open says where the character is instead.
#[test]
fn a_part_holding_a_control_character_is_refused_at_open_with_where_it_is() {
    let cases = [
        (
            "powerpoint/deck.pptx",
            "ppt/slides/slide1.xml",
            "<a:t>",
            '\u{1}',
        ),
        (
            "google/deck.pptx",
            "ppt/slides/slide1.xml",
            "<a:t>",
            '\u{1}',
        ),
        (
            "libreoffice/deck.pptx",
            "ppt/slides/slide1.xml",
            "<a:t>",
            '\u{1}',
        ),
        (
            "powerpoint/deck.pptx",
            "ppt/notesSlides/notesSlide1.xml",
            "<a:t>",
            '\u{1}',
        ),
        (
            "powerpoint/deck.pptx",
            "docProps/core.xml",
            "<dc:title>",
            '\u{b}',
        ),
    ];
    let file = std::env::temp_dir().join(format!(
        "oxoffice-pptx-control-character-{}.pptx",
        std::process::id()
    ));
    for (deck, part, marker, character) in cases {
        let (bytes, line, column) = deck_with_a_character(deck, part, marker, character);
        let code = u32::from(character);
        let error = match Presentation::from_bytes(&bytes) {
            Ok(_) => panic!("{deck} {part}: opens"),
            Err(error) => error.to_string(),
        };
        assert_eq!(
            error,
            format!(
                "/{part} holds U+{code:04X} at line {line}, column {column}, a character XML 1.0 cannot carry, so no XML reader accepts that part: remove the character from the part, or from the text written into it"
            ),
            "{deck}"
        );

        std::fs::write(&file, &bytes).unwrap();
        let issues = validate_presentation_file(&file);
        std::fs::remove_file(&file).unwrap();
        assert_eq!(
            issues.unwrap(),
            [ValidationIssue::InvalidXmlChild {
                part: format!("/{part}"),
                path: "/".to_owned(),
                message: format!(
                    "holds U+{code:04X} at line {line}, column {column}, a character XML 1.0 cannot carry: PowerPoint cannot read that part either"
                ),
            }],
            "{deck} {part}"
        );
    }
}
