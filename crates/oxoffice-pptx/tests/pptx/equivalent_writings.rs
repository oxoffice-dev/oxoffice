//! Equivalent writings of a deck give the same result
//! (`specs/2026-11-25-equivalent-xml-writings.md`): each input written again
//! the way another producer may write the same XML, every rewritten part
//! proved to say what the original says, then open, validate (the live deck
//! and the stored file), read the text and notes of every slide, save without
//! an edit, and replace a word of each slide's text. Each gives what it gives
//! on the original, and a save without an edit keeps the writing of every
//! part it keeps byte for byte on the original. A difference listed in
//! `tests/corpus/writings-known.toml` must happen, with its message.
//!
//! Pull requests run the deck built here and one copy per producer; the full
//! checks (`OXOFFICE_WORKFLOWS=full`) run every deck of the corpus.
//! `OXOFFICE_WRITINGS_ONLY` narrows a run as for the Word documents.

use std::sync::atomic::{AtomicUsize, Ordering};

use oxoffice_core::units::Emu;
use oxoffice_pptx::{Presentation, validate_presentation_file};
use oxoffice_xml_test::harness::{
    Input, Outcome, Scope, assert_known, attempt, only, selected_inputs, selected_writings,
};

use crate::support::{corpus_files, corpus_root, new_deck};

/// The corpus copies a pull request runs: the deck of each producer.
const SAMPLE: [&str; 4] = [
    "powerpoint/deck.pptx",
    "google/deck.pptx",
    "libreoffice/deck.pptx",
    "authored/deck.pptx",
];

/// Whether the full set runs (`OXOFFICE_WORKFLOWS=full`, as the docx
/// workflow walks).
fn every_copy() -> bool {
    std::env::var("OXOFFICE_WORKFLOWS").is_ok_and(|value| value == "full")
}

/// A deck from the default template: a text box with text outside ASCII and
/// notes.
fn template_deck() -> Vec<u8> {
    let mut prs = new_deck();
    let blank = (0..prs.layout_count())
        .find(|&index| prs.layout_name(index) == Some("Blank"))
        .expect("the default template has a Blank layout");
    prs.add_slide(blank).unwrap();
    prs.slide_mut(0)
        .unwrap()
        .add_textbox(Emu(914_400), Emu(914_400), Emu(5_486_400), Emu(914_400))
        .unwrap()
        .set_text("Quarterly results for the Zürich office")
        .unwrap();
    prs.set_notes_text(0, "Speaker notes about the café opening")
        .unwrap();
    prs.to_bytes().expect("the template deck saves")
}

fn inputs() -> Vec<Input> {
    let mut inputs = vec![Input {
        name: "built/template-deck.pptx".to_owned(),
        bytes: template_deck(),
    }];
    let full = every_copy();
    for file in corpus_files("pptx") {
        if full || SAMPLE.contains(&file.path.as_str()) {
            inputs.push(Input {
                bytes: file.bytes(),
                name: file.path,
            });
        }
    }
    inputs
}

fn open(bytes: &[u8]) -> Result<Presentation, String> {
    Presentation::from_bytes(bytes).map_err(|error| error.to_string())
}

fn save(bytes: &[u8]) -> Result<Vec<u8>, String> {
    open(bytes)?.to_bytes().map_err(|error| error.to_string())
}

/// The text and the notes of every slide.
fn slide_texts(prs: &Presentation) -> String {
    prs.slides()
        .map(|slide| format!("{:?} {:?}", slide.text(), slide.notes_text()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The validation of the stored file, read from a file of its own.
fn validate_stored(bytes: &[u8]) -> Result<String, String> {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "oxoffice-pptx-writings-{}-{}.pptx",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, bytes).map_err(|error| error.to_string())?;
    let issues = validate_presentation_file(&path);
    std::fs::remove_file(&path).map_err(|error| error.to_string())?;
    Ok(format!("{:?}", issues.map_err(|error| error.to_string())?))
}

/// A word of a slide's text to replace: four letters or more, found once in
/// the whole deck's text.
fn word_to_edit(text: &str, deck: &str) -> Option<String> {
    text.split(|c: char| !c.is_alphabetic())
        .find(|word| word.chars().count() >= 4 && deck.matches(word).count() == 1)
        .map(str::to_owned)
}

fn run(bytes: &[u8]) -> Outcome {
    let mut outcome = Outcome::default();
    let opened = attempt(|| open(bytes));
    outcome.step(
        "open",
        opened.as_ref().map(|_| String::new()).map_err(Clone::clone),
    );
    let Ok(prs) = opened else {
        return outcome;
    };
    outcome.step(
        "validate",
        attempt(|| Ok(format!("{:?}\n{}", prs.validate(), validate_stored(bytes)?))),
    );
    outcome.step("text", attempt(|| Ok(slide_texts(&prs))));
    outcome.step(
        "markdown",
        attempt(|| {
            prs.to_markdown()
                .map(|export| format!("{}\n{:?}", export.text, export.diagnostics))
                .map_err(|error| error.to_string())
        }),
    );
    let saved = attempt(|| {
        let saved = save(bytes)?;
        Ok((slide_texts(&open(&saved)?), saved))
    });
    outcome.step(
        "save",
        saved
            .as_ref()
            .map(|(texts, _)| texts.clone())
            .map_err(Clone::clone),
    );
    if let Ok((_, saved)) = &saved {
        outcome.saved("save", saved);
    }
    // A word of each slide's text replaced, saved and opened again.
    let edited = attempt(|| {
        let mut copy = open(bytes)?;
        let deck = slide_texts(&copy);
        let words: Vec<String> = copy
            .slides()
            .filter_map(|slide| word_to_edit(&slide.text(), &deck))
            .collect();
        let mut done = Vec::new();
        for word in words {
            let count = copy
                .try_replace_text(&word, &word.to_uppercase())
                .map_err(|error| format!("{word}: {error}"))?;
            done.push(format!("{word} {count}"));
        }
        let saved = copy.to_bytes().map_err(|error| error.to_string())?;
        Ok((
            format!("{}\n{}", done.join(", "), slide_texts(&open(&saved)?)),
            saved,
        ))
    });
    outcome.step(
        "edit",
        edited
            .as_ref()
            .map(|(shown, _)| shown.clone())
            .map_err(Clone::clone),
    );
    if let Ok((_, saved)) = &edited {
        outcome.saved("edit", saved);
    }
    outcome
}

#[test]
fn every_equivalent_writing_of_a_deck_gives_the_same_result() {
    let only = only();
    let scope = match (&only, every_copy()) {
        (Some(_), _) => Scope::Narrowed,
        (None, true) => Scope::Full,
        (None, false) => Scope::Sample,
    };
    assert_known(
        &corpus_root().join("writings-known.toml"),
        &selected_inputs(inputs(), only.as_deref()),
        &selected_writings(only.as_deref()),
        scope,
        &run,
        &save,
    );
}
