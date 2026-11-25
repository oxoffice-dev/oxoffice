//! Equivalent writings of a deck give the same result: open, validate, slide
//! texts and notes, Markdown, save, a word replaced per slide
//! (`specs/2026-11-25-equivalent-xml-writings.md`).

use oxoffice_core::units::Emu;
use oxoffice_pptx::Presentation;
use oxoffice_xml_test::harness::{Input, Outcome, attempt, check_writings};

use crate::support::{corpus_files, corpus_root, new_deck};

fn every_copy() -> bool {
    std::env::var("OXOFFICE_WORKFLOWS").is_ok_and(|value| value == "full")
}

/// A deck from the default template: a text box outside ASCII, and notes.
fn template_deck() -> Vec<u8> {
    let mut prs = new_deck();
    let blank = (0..prs.layout_count())
        .find(|&i| prs.layout_name(i) == Some("Blank"))
        .unwrap();
    prs.add_slide(blank).unwrap();
    let mut slide = prs.slide_mut(0).unwrap();
    let text_box = slide.add_textbox(Emu(914_400), Emu(914_400), Emu(5_486_400), Emu(914_400));
    text_box
        .unwrap()
        .set_text("Quarterly results for the Zürich office")
        .unwrap();
    prs.set_notes_text(0, "Speaker notes about the café opening")
        .unwrap();
    prs.to_bytes().unwrap()
}

fn inputs() -> Vec<Input> {
    let mut inputs = vec![("built/template-deck.pptx".into(), template_deck())];
    for file in corpus_files("pptx") {
        if every_copy() || file.path.ends_with("/deck.pptx") {
            inputs.push((file.path.clone(), file.bytes()));
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

fn texts(prs: &Presentation) -> String {
    let slides = prs
        .slides()
        .map(|slide| format!("{:?} {:?}", slide.text(), slide.notes_text()));
    slides.collect::<Vec<_>>().join("\n")
}

/// `bytes` after `change`, saved, and its texts read again.
fn changed(
    bytes: &[u8],
    change: impl FnOnce(&mut Presentation) -> Result<(), String>,
) -> Result<(Vec<u8>, String), String> {
    attempt(|| {
        let mut prs = open(bytes)?;
        change(&mut prs)?;
        let saved = prs.to_bytes().map_err(|error| error.to_string())?;
        Ok((saved.clone(), texts(&open(&saved)?)))
    })
}

fn run(bytes: &[u8]) -> Outcome {
    let mut outcome = Outcome::default();
    let opened = attempt(|| open(bytes));
    outcome.step(
        "open",
        opened.as_ref().map(|_| String::new()).map_err(Clone::clone),
    );
    let Ok(prs) = opened else { return outcome };
    outcome.step("validate", Ok(format!("{:?}", prs.validate())));
    outcome.step("text", Ok(texts(&prs)));
    let markdown = prs
        .to_markdown()
        .map(|export| format!("{}\n{:?}", export.text, export.diagnostics));
    outcome.step("markdown", markdown.map_err(|error| error.to_string()));
    outcome.saving("save", &changed(bytes, |_| Ok(())));
    // A word of each slide, found once in the deck, in upper case.
    let edited = changed(bytes, |prs| {
        let deck = texts(prs);
        let words: Vec<String> = prs
            .slides()
            .filter_map(|slide| {
                let text = slide.text();
                let mut words = text.split(|c: char| !c.is_alphabetic());
                words
                    .find(|w| w.chars().count() >= 4 && deck.matches(w).count() == 1)
                    .map(str::to_owned)
            })
            .collect();
        for word in words {
            prs.try_replace_text(&word, &word.to_uppercase())
                .map_err(|error| format!("{word}: {error}"))?;
        }
        Ok(())
    });
    outcome.saving("edit", &edited);
    outcome
}

#[test]
fn every_equivalent_writing_of_a_deck_gives_the_same_result() {
    check_writings(
        &corpus_root().join("writings-known.toml"),
        inputs(),
        every_copy(),
        &run,
        &save,
    );
}
