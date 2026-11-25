//! Equivalent writings of a Word document give the same result
//! (`specs/2026-11-25-equivalent-xml-writings.md`).
//!
//! Each input is written again the way another producer may write the same
//! XML ([`Writing`]: an empty element with an end tag or self-closed, other
//! prefixes, the default namespace, indentation, attribute order, quotes and
//! spacing, character references), every rewritten part proved to say what
//! the original says by its canonical form. The operations then run on the
//! original and on each rewrite: open, validate, read the text of every
//! story, save without an edit, edit one paragraph in each story, compare the
//! edited copy with the original, and accept and reject that redline. Each
//! gives what it gives on the original, the XML it saves read in its
//! canonical form, and a save without an edit keeps the writing of every part
//! it keeps byte for byte on the original. A difference listed in
//! `tests/corpus/writings-known.toml` must happen, with its message, so that
//! a fix shows.
//!
//! Pull requests run the documents built here and two corpus copies; the
//! full checks (`OXOFFICE_WORKFLOWS=full`) run every Word document of the
//! corpus but the long ones. `OXOFFICE_WRITINGS_ONLY="word/report.docx
//! indented"` runs the inputs and writings it names (all of a kind when it
//! names none of it) and checks no stale entry.

use oxoffice_docx::{Document, StoryKind, StoryParagraph, validate_document_bytes};
use oxoffice_xml_test::harness::{
    Input, Outcome, Scope, assert_known, attempt, only, selected_inputs, selected_writings,
};
use oxoffice_xml_test::{Writing, canonical, rewrite};

use crate::support::{corpus_files, corpus_root, doc_with_body, every_copy, small_thesis};

const AUTHOR: &str = "Writings";
const DATE: &str = "2026-11-25T00:00:00Z";

/// The corpus copies a pull request runs: Google's and LibreOffice's, whose
/// writings differ most from Word's, from families with comments, notes,
/// tracked changes, fields and controls.
const SAMPLE: [&str; 2] = ["google/form.docx", "libreoffice/contract.docx"];

/// A paragraph holding an empty inline content control between two runs,
/// as Google Docs leaves them in ordinary paragraphs, a page field whose runs
/// have empty properties, as LibreOffice writes them, text outside ASCII, a
/// footnote and a header: the shapes behind #656 and #413 in a few lines,
/// written with end tags as Word would write the control.
fn producer_shapes() -> Vec<u8> {
    let mut doc = doc_with_body(concat!(
        r#"<w:p><w:r><w:t xml:space="preserve">Alpha beta </w:t></w:r>"#,
        r#"<w:sdt><w:sdtPr><w:tag w:val="goog_rdk_1"/><w:id w:val="1"/></w:sdtPr><w:sdtContent></w:sdtContent></w:sdt>"#,
        r#"<w:r><w:t>gamma.</w:t></w:r></w:p>"#,
        r#"<w:p><w:r><w:rPr></w:rPr><w:t xml:space="preserve">Page </w:t></w:r>"#,
        r#"<w:r><w:rPr></w:rPr><w:fldChar w:fldCharType="begin"></w:fldChar></w:r>"#,
        r#"<w:r><w:rPr></w:rPr><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r>"#,
        r#"<w:r><w:rPr></w:rPr><w:fldChar w:fldCharType="separate"></w:fldChar></w:r>"#,
        r#"<w:r><w:rPr></w:rPr><w:t>1</w:t></w:r>"#,
        r#"<w:r><w:rPr></w:rPr><w:fldChar w:fldCharType="end"></w:fldChar></w:r>"#,
        r#"<w:r><w:t xml:space="preserve"> of the delivery schedule.</w:t></w:r></w:p>"#,
        r#"<w:p><w:r><w:t>Ångström café naïve résumé.</w:t></w:r></w:p>"#,
    ));
    let id = doc.add_footnote("Footnote sentence about suppliers.");
    doc.add_paragraph("Closing paragraph with a reference.")
        .add_footnote_ref(id);
    doc.set_header("Running header of the contract");
    doc.to_bytes().expect("the producer shapes save")
}

/// The documents built here and the corpus copies this run takes.
fn inputs() -> Vec<Input> {
    let mut inputs = vec![
        Input {
            name: "built/producer-shapes.docx".to_owned(),
            bytes: producer_shapes(),
        },
        Input {
            name: "built/small-thesis.docx".to_owned(),
            bytes: small_thesis(),
        },
    ];
    let full = every_copy();
    for file in corpus_files("docx") {
        // A long copy repeats the markup of its family at minutes a writing.
        let long = file.path.contains("/long-");
        if (full && !long) || SAMPLE.contains(&file.path.as_str()) {
            inputs.push(Input {
                bytes: file.bytes(),
                name: file.path,
            });
        }
    }
    inputs
}

/// The paragraphs of every story as a reader sees them.
fn story_texts(doc: &Document) -> Result<String, String> {
    Ok(doc
        .story_paragraphs()
        .map_err(|error| error.to_string())?
        .iter()
        .map(|p| format!("{:?} {:?} {:?}", p.kind, p.style, p.text))
        .collect::<Vec<_>>()
        .join("\n"))
}

/// A word of a paragraph to edit: four letters or more, found once in it.
fn word_to_edit(text: &str) -> Option<String> {
    text.split(|c: char| !c.is_alphabetic())
        .find(|word| word.chars().count() >= 4 && text.matches(word).count() == 1)
        .map(str::to_owned)
}

/// The paragraphs one edit changes, by their index in
/// [`Document::story_paragraphs`], with the word to change: the first of each
/// kind of story with a word to edit, and the first after each quarter of the
/// body.
fn edit_targets(paragraphs: &[StoryParagraph]) -> Vec<(usize, String)> {
    let body: Vec<usize> = paragraphs
        .iter()
        .enumerate()
        .filter(|(_, p)| p.kind == StoryKind::Body)
        .map(|(index, _)| index)
        .collect();
    let mut quarters: Vec<usize> = [1, 2, 3]
        .iter()
        .filter_map(|quarter| body.get(body.len() * quarter / 4).copied())
        .collect();
    let mut kinds = Vec::new();
    let mut targets = Vec::new();
    for (index, paragraph) in paragraphs.iter().enumerate() {
        let Some(word) = word_to_edit(&paragraph.text) else {
            continue;
        };
        let first_of_kind = !kinds.contains(&paragraph.kind);
        let after_quarter =
            paragraph.kind == StoryKind::Body && quarters.first().is_some_and(|at| index >= *at);
        if after_quarter {
            quarters.remove(0);
        }
        if first_of_kind || after_quarter {
            kinds.push(paragraph.kind);
            targets.push((index, word));
        }
    }
    targets
}

/// The word in another case: the text changes, its length does not.
fn edited_word(word: &str) -> String {
    if word.chars().any(char::is_lowercase) {
        word.to_uppercase()
    } else {
        word.to_lowercase()
    }
}

fn open(bytes: &[u8]) -> Result<Document, String> {
    Document::from_bytes(bytes).map_err(|error| error.to_string())
}

fn save(bytes: &[u8]) -> Result<Vec<u8>, String> {
    open(bytes)?.to_bytes().map_err(|error| error.to_string())
}

/// Every operation on one writing, each step after the one it needs.
fn run(bytes: &[u8]) -> Outcome {
    let mut outcome = Outcome::default();
    let opened = attempt(|| open(bytes));
    outcome.step(
        "open",
        opened.as_ref().map(|_| String::new()).map_err(Clone::clone),
    );
    let Ok(doc) = opened else {
        return outcome;
    };
    outcome.step(
        "validate",
        attempt(|| {
            let stored = validate_document_bytes(bytes).map_err(|error| error.to_string())?;
            let live = doc.validate().map_err(|error| error.to_string())?;
            Ok(format!("{:?}\n{:?}", stored.findings, live.findings))
        }),
    );
    let original_texts = attempt(|| story_texts(&doc));
    outcome.step("text", original_texts.clone());
    // The model as Markdown: headings, lists, tables, bold and italic.
    outcome.step("markdown", attempt(|| Ok(doc.to_markdown())));

    let saved = attempt(|| {
        let saved = save(bytes)?;
        Ok((story_texts(&open(&saved)?)?, saved))
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

    // One paragraph of each story and three more of the body edited, saved
    // and opened again.
    let edited = attempt(|| {
        let mut copy = open(bytes)?;
        let paragraphs = copy.story_paragraphs().map_err(|error| error.to_string())?;
        let mut done = Vec::new();
        for (index, word) in edit_targets(&paragraphs) {
            // An edit makes the locations read before it stale.
            let paragraphs = copy.story_paragraphs().map_err(|error| error.to_string())?;
            let count = copy
                .try_replace_text_at(
                    &paragraphs[index].location,
                    &word,
                    &edited_word(&word),
                    Some(1),
                )
                .map_err(|error| format!("{word}: {error}"))?
                .map_err(|mismatch| format!("{word}: {mismatch:?}"))?;
            done.push(format!("{word} {count}"));
        }
        let saved = copy.to_bytes().map_err(|error| error.to_string())?;
        let texts = story_texts(&open(&saved)?)?;
        Ok((format!("{}\n{texts}", done.join(", ")), texts, saved))
    });
    outcome.step(
        "edit",
        edited
            .as_ref()
            .map(|(shown, ..)| shown.clone())
            .map_err(Clone::clone),
    );
    let Ok((_, edited_texts, edited_bytes)) = edited else {
        return outcome;
    };
    outcome.saved("edit", &edited_bytes);

    // The original compared with the edited copy, the redline saved, then
    // every change accepted, and every change rejected, on copies of it.
    let redline = attempt(|| {
        let mut original = open(bytes)?;
        let diagnostics = original
            .compare(&open(&edited_bytes)?, AUTHOR, DATE)
            .map_err(|error| error.to_string())?;
        let saved = original.to_bytes().map_err(|error| error.to_string())?;
        let texts = story_texts(&open(&saved)?)?;
        Ok((format!("{diagnostics:?}\n{texts}"), saved))
    });
    outcome.step(
        "compare",
        redline
            .as_ref()
            .map(|(shown, _)| shown.clone())
            .map_err(Clone::clone),
    );
    let Ok((_, redline_bytes)) = redline else {
        return outcome;
    };
    outcome.saved("compare", &redline_bytes);
    for (name, accept, version) in [
        ("accept", true, Ok(edited_texts)),
        ("reject", false, original_texts),
    ] {
        let resolved = attempt(|| {
            let mut copy = open(&redline_bytes)?;
            if accept {
                copy.accept_all()
            } else {
                copy.reject_all()
            }
            .map_err(|error| error.to_string())?;
            let saved = copy.to_bytes().map_err(|error| error.to_string())?;
            // Each writing gives its own version back.
            if version.as_ref().ok() != Some(&story_texts(&open(&saved)?)?) {
                return Err(format!(
                    "gives another text than the {} version",
                    if accept { "edited" } else { "original" }
                ));
            }
            Ok(saved)
        });
        outcome.step(
            name,
            resolved
                .as_ref()
                .map(|_| String::new())
                .map_err(Clone::clone),
        );
        if let Ok(saved) = resolved {
            outcome.saved(name, &saved);
        }
    }
    outcome
}

#[test]
fn every_equivalent_writing_of_a_document_gives_the_same_result() {
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

// The rewrites the test above relies on: each changes what it names, copies
// the rest, and says what the original says.
#[test]
fn each_writing_changes_what_it_names_and_says_the_same() {
    let xml = concat!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
        r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" "#,
        r#"xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" mc:Ignorable="w"><w:body>"#,
        r#"<w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:t xml:space="preserve">Café > </w:t><w:tab/></w:r>"#,
        r#"<w:sdt><w:sdtContent></w:sdtContent></w:sdt></w:p></w:body></w:document>"#,
    );
    let expected = [
        (
            Writing::EmptyWithEndTag,
            r#"<w:jc w:val="center"></w:jc></w:pPr><w:r><w:t xml:space="preserve">Café > </w:t><w:tab></w:tab></w:r>"#,
        ),
        (
            Writing::EmptySelfClosed,
            r#"<w:sdt><w:sdtContent/></w:sdt>"#,
        ),
        (
            Writing::OtherPrefixes,
            r#"<ox0:document xmlns:ox0="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" mc:Ignorable="ox0"><ox0:body>"#,
        ),
        (
            Writing::DefaultNamespace,
            r#"<document xmlns="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:w="#,
        ),
        (
            Writing::Indented,
            "<w:body>\n    <w:p>\n      <w:pPr>\n        <w:jc w:val=\"center\"/>\n      </w:pPr>",
        ),
        (
            Writing::AttributeOrder,
            r#"<w:document mc:Ignorable="w" xmlns:mc="#,
        ),
        (Writing::SingleQuotes, r#"<w:jc w:val='center'/>"#),
        (
            Writing::TagSpacing,
            "<w:jc\n\tw:val = \"center\" /></w:pPr\n>",
        ),
        (
            Writing::CharacterReferences,
            r#"<w:t xml:space="preserve">Caf&#xE9; &gt; </w:t>"#,
        ),
        (
            Writing::LeadingCharacterReference,
            r#"<w:jc w:val="&#99;enter"/></w:pPr><w:r><w:t xml:space="preserve">&#67;afé > </w:t>"#,
        ),
    ];
    for (writing, holds) in expected {
        let rewritten = rewrite(xml.as_bytes(), writing).expect("the part rewrites");
        let text = String::from_utf8(rewritten.xml).unwrap();
        assert!(text.contains(holds), "{}: {text}", writing.name());
        assert!(rewritten.changes > 0, "{}", writing.name());
        assert_eq!(
            canonical(text.as_bytes()),
            canonical(xml.as_bytes()),
            "{}",
            writing.name()
        );
    }
    // A text inside `xml:space="preserve"` or of a leaf is never indented.
    let indented =
        String::from_utf8(rewrite(xml.as_bytes(), Writing::Indented).unwrap().xml).unwrap();
    assert!(indented.contains(r#"<w:t xml:space="preserve">Café > </w:t>"#));
}
