//! Equivalent writings of a Word document give the same result: open,
//! validate, story texts, Markdown, save, an edit per story, compare, accept
//! and reject (`specs/2026-11-25-equivalent-xml-writings.md`). Pull requests
//! run the built documents and two copies; `OXOFFICE_WORKFLOWS=full` all but
//! the long and thesis copies.

use oxoffice_docx::{Document, StoryKind, StoryParagraph, validate_document_bytes};
use oxoffice_xml_test::harness::{Input, Outcome, attempt, check_writings};
use oxoffice_xml_test::{Writing, rewrite_package};

use crate::support::{
    corpus_files, corpus_root, doc_with_body, docx_with_body, every_copy, small_thesis,
};

const AUTHOR: &str = "Writings";
const DATE: &str = "2026-11-25T00:00:00Z";

/// The corpus copies a pull request runs: Google's and LibreOffice's, whose
/// writings differ most from Word's.
const SAMPLE: [&str; 2] = ["google/form.docx", "libreoffice/contract.docx"];

/// The shapes behind #656 and #413 in a few lines, written as Word would: an
/// empty inline content control (Google Docs leaves them in paragraphs), a
/// page field with empty run properties (LibreOffice), text outside ASCII, a
/// footnote and a header.
fn producer_shapes() -> Vec<u8> {
    let mut doc = doc_with_body(concat!(
        r#"<w:p><w:r><w:t xml:space="preserve">Alpha beta </w:t></w:r>"#,
        r#"<w:sdt><w:sdtPr><w:tag w:val="goog_rdk_1"/><w:id w:val="1"/></w:sdtPr><w:sdtContent></w:sdtContent></w:sdt>"#,
        r#"<w:r><w:t>gamma.</w:t></w:r></w:p><w:p><w:r><w:rPr></w:rPr><w:t xml:space="preserve">Page </w:t></w:r>"#,
        r#"<w:r><w:rPr></w:rPr><w:fldChar w:fldCharType="begin"></w:fldChar></w:r>"#,
        r#"<w:r><w:rPr></w:rPr><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r>"#,
        r#"<w:r><w:rPr></w:rPr><w:fldChar w:fldCharType="separate"></w:fldChar></w:r><w:r><w:t>1</w:t></w:r>"#,
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

fn inputs() -> Vec<Input> {
    let mut inputs = vec![
        ("built/producer-shapes.docx".into(), producer_shapes()),
        ("built/small-thesis.docx".into(), small_thesis()),
    ];
    for file in corpus_files("docx") {
        // A long copy repeats its family at minutes a writing; the small
        // thesis keeps the landmarks of the thesis copies, a fifth of the rest.
        let costly = file.path.contains("/long-") || file.path.ends_with("/thesis.docx");
        if (every_copy() && !costly) || SAMPLE.contains(&file.path.as_str()) {
            inputs.push((file.path.clone(), file.bytes()));
        }
    }
    inputs
}

fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn open(bytes: &[u8]) -> Result<Document, String> {
    Document::from_bytes(bytes).map_err(message)
}

fn save(bytes: &[u8]) -> Result<Vec<u8>, String> {
    open(bytes)?.to_bytes().map_err(message)
}

/// The paragraphs of every story as a reader sees them.
fn story_texts(doc: &Document) -> Result<String, String> {
    let paragraphs = doc.story_paragraphs().map_err(message)?.into_iter();
    let lines = paragraphs.map(|p| format!("{:?} {:?} {:?}", p.kind, p.style, p.text));
    Ok(lines.collect::<Vec<_>>().join("\n"))
}

/// `bytes` after `change`, saved, and its texts read again.
type Changed = Result<(Vec<u8>, String), String>;
fn changed(bytes: &[u8], change: impl FnOnce(&mut Document) -> Result<(), String>) -> Changed {
    attempt(|| {
        let mut doc = open(bytes)?;
        change(&mut doc)?;
        let saved = doc.to_bytes().map_err(message)?;
        let texts = story_texts(&open(&saved)?)?;
        Ok((saved, texts))
    })
}

/// Every change of a document resolved, accepted or rejected.
fn resolved(bytes: &[u8], accept: bool) -> Changed {
    changed(bytes, |doc| {
        let resolved = if accept {
            doc.accept_all()
        } else {
            doc.reject_all()
        };
        resolved.map(drop).map_err(message)
    })
}

/// The paragraphs one edit changes, by index in [`Document::story_paragraphs`],
/// with a word of four letters or more found once in them: the first of each
/// kind of story, and the first after each quarter of the body.
fn edit_targets(paragraphs: &[StoryParagraph]) -> Vec<(usize, String)> {
    let body = paragraphs
        .iter()
        .filter(|p| p.kind == StoryKind::Body)
        .count();
    let mut quarters: Vec<usize> = (1..4).map(|quarter| body * quarter / 4).collect();
    let (mut kinds, mut targets, mut seen_body) = (Vec::new(), Vec::new(), 0);
    for (index, paragraph) in paragraphs.iter().enumerate() {
        let is_body = paragraph.kind == StoryKind::Body;
        seen_body += usize::from(is_body);
        let text = &paragraph.text;
        let mut words = text.split(|c: char| !c.is_alphabetic());
        let Some(word) = words.find(|w| w.chars().count() >= 4 && text.matches(w).count() == 1)
        else {
            continue;
        };
        let after_quarter = is_body && quarters.first().is_some_and(|at| seen_body > *at);
        if after_quarter {
            quarters.remove(0);
        }
        if !kinds.contains(&paragraph.kind) || after_quarter {
            kinds.push(paragraph.kind);
            targets.push((index, word.to_owned()));
        }
    }
    targets
}

/// Every operation on one writing, each step after the one it needs.
fn run(bytes: &[u8]) -> Outcome {
    let mut outcome = Outcome::default();
    let opened = attempt(|| open(bytes));
    outcome.step(
        "open",
        opened.as_ref().map(|_| String::new()).map_err(message),
    );
    let Ok(doc) = opened else { return outcome };
    let validated = attempt(|| {
        let stored = validate_document_bytes(bytes).map_err(message)?;
        Ok(format!(
            "{:?}\n{:?}",
            stored.findings,
            doc.validate().map_err(message)?.findings
        ))
    });
    outcome.step("validate", validated);
    outcome.step("text", attempt(|| story_texts(&doc)));
    outcome.step("markdown", attempt(|| Ok(doc.to_markdown())));
    outcome.saving("save", &changed(bytes, |_| Ok(())));
    let edited = changed(bytes, |doc| {
        for (index, word) in edit_targets(&doc.story_paragraphs().map_err(message)?) {
            // An edit makes the locations read before it stale.
            let location = doc.story_paragraphs().map_err(message)?[index]
                .location
                .clone();
            let lower = word.chars().any(char::is_lowercase);
            let new = if lower {
                word.to_uppercase()
            } else {
                word.to_lowercase()
            };
            let count = doc.try_replace_text_at(&location, &word, &new, Some(1));
            count
                .map_err(message)?
                .map_err(|mismatch| format!("{word}: {mismatch:?}"))?;
        }
        Ok(())
    });
    outcome.saving("edit", &edited);
    let Ok((edited, edited_texts)) = edited else {
        return outcome;
    };
    let mut diagnostics = String::new();
    let redline = changed(bytes, |doc| {
        let found = doc
            .compare(&open(&edited)?, AUTHOR, DATE)
            .map_err(message)?;
        diagnostics = format!("{found:?}\n");
        Ok(())
    });
    let redline = redline.map(|(saved, texts)| (saved, diagnostics + &texts));
    outcome.saving("compare", &redline);
    let Ok((redline, _)) = redline else {
        return outcome;
    };
    // Accepting gives the edited version back; rejecting, the original with
    // the changes it tracked itself rejected too.
    let versions = [
        (true, Ok(edited_texts)),
        (false, resolved(bytes, false).map(|(_, texts)| texts)),
    ];
    for (accept, version) in versions {
        let result =
            resolved(&redline, accept).and_then(|(saved, texts)| match version == Ok(texts) {
                true => Ok((saved, String::new())),
                false => Err("gives another text than its version".to_owned()),
            });
        outcome.saving(if accept { "accept" } else { "reject" }, &result);
    }
    outcome
}

#[test]
fn every_equivalent_writing_of_a_document_gives_the_same_result() {
    check_writings(
        &corpus_root().join("writings-known.toml"),
        inputs(),
        every_copy(),
        &run,
        &save,
    );
}

/// `bytes` compared with `edited`, then accepted and rejected: each gives its
/// version back (with their own tracked changes resolved alike).
fn assert_compares(bytes: &[u8], edited: &[u8], what: &str) {
    let compared = changed(bytes, |doc| {
        doc.compare(&open(edited)?, AUTHOR, DATE)
            .map(drop)
            .map_err(message)
    });
    let (redline, _) = compared.unwrap_or_else(|error| panic!("{what}: {error}"));
    for (accept, version) in [(true, edited), (false, bytes)] {
        let texts = |bytes: &[u8]| resolved(bytes, accept).map(|(_, texts)| texts);
        assert_eq!(texts(&redline), texts(version), "{what} {accept}");
    }
}

// A link of a paragraph added or removed whole becomes a HYPERLINK field; its
// attributes are read with the prefixes the document binds (`ox0:history`).
#[test]
fn a_linked_paragraph_added_or_removed_whole_compares_under_other_prefixes() {
    let body = |link: &str| {
        let document = docx_with_body(&format!(
            r#"<w:p><w:r><w:t>Before.</w:t></w:r></w:p>{link}<w:p><w:bookmarkStart w:id="0" w:name="Target"/><w:r><w:t>Target.</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>"#
        ));
        rewrite_package(&document, Writing::OtherPrefixes)
            .unwrap()
            .0
    };
    let (with, without) = (
        body(
            r#"<w:p><w:hyperlink w:anchor="Target" w:history="1"><w:r><w:t>See the target.</w:t></w:r></w:hyperlink></w:p>"#,
        ),
        body(""),
    );
    assert_compares(&with, &without, "removed");
    assert_compares(&without, &with, "added");
}
