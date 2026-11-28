//! Equivalent writings of a Word document give the same result: open,
//! validate, story texts, Markdown, save, an edit per story, compare, accept
//! and reject (`specs/2026-11-25-equivalent-xml-writings.md`). Pull requests
//! run the producer shapes and two copies; `OXOFFICE_WORKFLOWS=full` adds the
//! small thesis and every copy but the long and thesis ones.

use oxoffice_docx::{Document, StoryKind, StoryParagraph, validate_document_bytes};
use oxoffice_xml_test::harness::{Input, Outcome, attempt, check_writings};
use oxoffice_xml_test::{Writing, canonical, package_parts, rewrite_package, write_package};

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
/// footnote and a header; and the leaves #664 reads in both writings: a
/// heading style, run toggles, tabs, breaks and hyphens in the body, the
/// footnote and the header, an empty text, a table style region kept as
/// Word writes it (`w:firstRow` beside `w:val`).
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
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Delivery terms</w:t></w:r></w:p>"#,
        r#"<w:p><w:r><w:rPr><w:b/><w:i/></w:rPr><w:t>Bold italic</w:t></w:r>"#,
        r#"<w:r><w:rPr><w:strike/></w:rPr><w:t xml:space="preserve"> struck</w:t></w:r>"#,
        r#"<w:r><w:rPr><w:vanish/></w:rPr><w:t xml:space="preserve"> hidden</w:t></w:r>"#,
        r#"<w:r><w:rPr><w:rtl/></w:rPr><w:t xml:space="preserve"> right</w:t></w:r></w:p>"#,
        r#"<w:p><w:r><w:t>Tab</w:t><w:tab/><w:t>break</w:t><w:br/><w:t>return</w:t><w:cr/>"#,
        r#"<w:t>non</w:t><w:noBreakHyphen/><w:t>breaking</w:t>"#,
        r#"<w:ptab w:relativeTo="margin" w:alignment="right" w:leader="none"/><w:t>end</w:t></w:r>"#,
        r#"<w:r><w:t/></w:r></w:p>"#,
        r#"<w:p><w:pPr><w:cnfStyle w:val="100000000000" w:firstRow="1"/></w:pPr><w:r><w:t>First row style</w:t></w:r></w:p>"#,
    ));
    let id = doc.add_footnote("Footnote sentence about suppliers.");
    doc.add_paragraph("Closing paragraph with a reference.")
        .add_footnote_ref(id);
    doc.set_header("Running header of the contract");
    let bytes = doc.to_bytes().expect("the producer shapes save");
    let split = |data: &[u8], words: &str, leaf: &str| {
        let text = String::from_utf8(data.to_vec()).expect("a part is UTF-8");
        let (first, second) = words.split_once(' ').expect("two words");
        let leaf = format!(r#"{first}</w:t>{leaf}<w:t xml:space="preserve">{second}"#);
        assert!(text.contains(words), "{words}");
        text.replace(words, &leaf).into_bytes()
    };
    let parts = package_parts(&bytes).expect("the producer shapes read");
    let parts = parts.into_iter().map(|(name, data)| {
        let data = match name.as_str() {
            "word/footnotes.xml" => split(&data, "sentence about", "<w:tab/>"),
            name if name.starts_with("word/header") => split(&data, "header of", "<w:br/>"),
            _ => data,
        };
        (name, data)
    });
    write_package(&parts.collect::<Vec<_>>())
}

fn inputs() -> Vec<Input> {
    let mut inputs = vec![("built/producer-shapes.docx".into(), producer_shapes())];
    // The small thesis costs a third of the walk: the full checks run it, a
    // pull request does not (its landmarks are the corpus thesis copies').
    if every_copy() {
        inputs.push(("built/small-thesis.docx".into(), small_thesis()));
    }
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
// attributes are read with the prefixes the document binds (`ox0:history`),
// and restored under those its element binds.
#[test]
fn a_linked_paragraph_added_or_removed_whole_compares_under_other_prefixes() {
    let body = |link: &str, other_prefixes: bool| {
        let document = docx_with_body(&format!(
            r#"<w:p><w:r><w:t>Before.</w:t></w:r></w:p>{link}<w:p><w:r><w:t>After.</w:t></w:r></w:p>"#
        ));
        match other_prefixes {
            true => {
                rewrite_package(&document, Writing::OtherPrefixes)
                    .unwrap()
                    .0
            }
            false => document,
        }
    };
    let link = r#"<w:p><w:hyperlink w:anchor="Target" w:history="1"><w:r><w:t>See the target.</w:t></w:r></w:hyperlink></w:p>"#;
    // (original, edited): both under other prefixes, or the link added in
    // a copy written with them to one written with `w`.
    let (other, plain) = (true, false);
    for (from, to) in [
        ((other, ""), (other, link)),
        ((other, link), (other, "")),
        ((plain, ""), (other, link)),
    ] {
        assert_compares(
            &body(from.1, from.0),
            &body(to.1, to.0),
            &format!("{from:?} {to:?}"),
        );
    }
}

// The canonical form proves a rewrite says the same only if it tells apart
// what does not; and a rewrite that would change what a part says is refused.
#[test]
fn the_canonical_form_tells_apart_two_parts_that_say_different_things() {
    let w = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;
    let mc = r#"xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006""#;
    for (left, right) in [
        (r#"<w:b w:val="1"/>"#, r#"<w:b w:val="0"/>"#),
        ("<w:t>a</w:t>", "<w:t>a </w:t>"),
        ("<w:t> </w:t>", "<w:t></w:t>"),
        (
            r#"<w:r xml:space="preserve"><w:t/> </w:r>"#,
            r#"<w:r xml:space="preserve"><w:t/></w:r>"#,
        ),
        (r#"<a:x xmlns:a="urn:1"/>"#, r#"<a:x xmlns:a="urn:2"/>"#),
        ("<p><a/><b/></p>", "<p><b/><a/></p>"),
        (
            r#"<r xmlns:x="urn:1" mc:Ignorable="x"/>"#,
            r#"<r xmlns:x="urn:2" mc:Ignorable="x"/>"#,
        ),
    ] {
        let read = |xml: &str| canonical(format!("<root {w} {mc}>{xml}</root>").as_bytes());
        assert_ne!(read(left), read(right), "{left} {right}");
    }
    // A compatibility list naming a main prefix cannot follow its renaming.
    let parts = package_parts(&docx_with_body("<w:p/>"))
        .unwrap()
        .into_iter()
        .map(|(name, data)| {
            let data = match name == "word/document.xml" {
                true => String::from_utf8(data).unwrap().replacen(
                    "<w:body>",
                    r#"<w:body mc:Ignorable="w">"#,
                    1,
                ),
                false => return (name, data),
            };
            (name, data.into_bytes())
        });
    let refused = rewrite_package(
        &write_package(&parts.collect::<Vec<_>>()),
        Writing::OtherPrefixes,
    );
    assert!(refused.is_err_and(|error| error.contains("names a renamed prefix")));
}

// A table paragraph's `w:cnfStyle`, or a toggle's carrier in a paragraph or
// its mark, is kept as written: written again in another attribute order,
// or under another prefix, without one (the default namespace), or with
// another spelling of its value (`1` or `on` saved as no value, `0` as
// `false`) after an edit, it is no change; with another region or value it
// is one.
#[test]
fn a_kept_paragraph_property_counts_only_when_what_it_says_changes() {
    let cell = |cnf: &str, text: &str| {
        docx_with_body(&format!(
            r#"<w:tbl><w:tblPr><w:tblW w:w="2000" w:type="dxa"/></w:tblPr><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:tcW w:w="2000" w:type="dxa"/></w:tcPr><w:p><w:pPr><w:cnfStyle {cnf}/></w:pPr><w:r><w:t>{text} totals.</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p/>"#
        ))
    };
    let original = cell(r#"w:val="100000000000" w:firstRow="1""#, "Quarterly");
    for (edited, same) in [
        (r#"w:firstRow="1" w:val="100000000000""#, true),
        (r#"w:val="100000000000" w:firstRow="0""#, false),
    ] {
        let found =
            open(&original)
                .unwrap()
                .compare(&open(&cell(edited, "Yearly")).unwrap(), AUTHOR, DATE);
        assert_eq!(found.unwrap().is_empty(), same, "{edited}");
    }
    // As written, under other prefixes, with the default namespace, and with
    // the default namespace over other prefixes: elements without a prefix,
    // attributes under one that is neither `w` nor the element's.
    let writings: [&[Writing]; 4] = [
        &[],
        &[Writing::OtherPrefixes],
        &[Writing::DefaultNamespace],
        &[Writing::OtherPrefixes, Writing::DefaultNamespace],
    ];
    let paragraph = |ppr: &str| {
        docx_with_body(&format!(
            r#"<w:p><w:pPr>{ppr}</w:pPr><w:r><w:t>Quarterly totals.</w:t></w:r></w:p>"#
        ))
    };
    // A toggle's carrier: a modelled toggle with another attribute, in the
    // mark (`w:rtl`) or the paragraph (`w:bidi`), whose text is bookmarked
    // or not.
    let carrier = |element: &str, val: &str, extra: &str, bookmarked: bool| {
        let toggle = format!(r#"<w:{element} w:val="{val}" w:extra="{extra}"/>"#);
        let ppr = match element {
            "rtl" => format!("<w:rPr>{toggle}</w:rPr>"),
            _ => toggle,
        };
        let text = r#"<w:r><w:t>Quarterly totals.</w:t></w:r>"#;
        let text = if bookmarked {
            format!(r#"<w:bookmarkStart w:id="0" w:name="totals"/>{text}<w:bookmarkEnd w:id="0"/>"#)
        } else {
            text.to_owned()
        };
        docx_with_body(&format!(r#"<w:p><w:pPr>{ppr}</w:pPr>{text}</w:p>"#))
    };
    let mark = |val: &str, extra: &str| carrier("rtl", val, extra, false);
    for (row, (original, edited, same)) in [
        (
            &original,
            cell(r#"w:val="100000000000" w:firstRow="1""#, "Quarterly"),
            true,
        ),
        (
            &original,
            cell(r#"w:val="100000000000" w:firstRow="0""#, "Quarterly"),
            false,
        ),
        (&mark("false", "x"), mark("false", "x"), true),
        (&mark("false", "x"), mark("false", "y"), false),
        (&mark("1", "x"), mark("1", "x"), true),
        (&mark("0", "x"), mark("0", "x"), true),
        (&mark("1", "x"), mark("0", "x"), false),
        (
            &carrier("bidi", "on", "x", false),
            carrier("bidi", "on", "x", false),
            true,
        ),
        (
            &carrier("bidi", "on", "x", true),
            carrier("bidi", "on", "x", true),
            true,
        ),
        (
            &carrier("bidi", "on", "x", false),
            carrier("bidi", "off", "x", false),
            false,
        ),
        (
            &paragraph(r#"<w:tabs><w:tab w:val="left" w:pos="720"/></w:tabs>"#),
            paragraph(r#"<w:tabs><w:tab w:pos="720" w:val="left"/></w:tabs>"#),
            true,
        ),
        (
            &paragraph(r#"<w:pBdr><w:top w:val="single" w:sz="4"/></w:pBdr>"#),
            paragraph(r#"<w:pBdr><w:top w:sz="4" w:val="single"/></w:pBdr>"#),
            true,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        for writing in writings {
            let written = |bytes: Vec<u8>| {
                writing.iter().fold(bytes, |bytes, writing| {
                    rewrite_package(&bytes, *writing).unwrap().0
                })
            };
            let mut doc = open(&written(edited.clone())).unwrap();
            assert_eq!(doc.replace_text("Quarterly", "Yearly"), 1);
            let saved = open(&doc.to_bytes().unwrap()).unwrap();
            let mut redline = open(&written(original.clone())).unwrap();
            let found = redline
                .compare(&saved, AUTHOR, DATE)
                .unwrap_or_else(|error| panic!("{writing:?}, row {row}: {error}"));
            // A kept property that says the same is no property change.
            let tracked = package_parts(&redline.to_bytes().unwrap())
                .unwrap()
                .into_iter()
                .any(|(name, xml)| {
                    name.ends_with("document.xml") && xml.windows(8).any(|w| w == b"PrChange")
                });
            assert_eq!(found.is_empty() && !tracked, same, "{writing:?}, row {row}");
        }
    }
}

// A header written with WordprocessingML as its default namespace keeps the
// elements its model holds raw (a bookmark around its paragraph) in that
// namespace when an edit writes the header again.
#[test]
fn a_header_written_again_keeps_its_default_namespace() {
    let mut doc = Document::new();
    doc.set_header("Quarterly totals");
    let parts = package_parts(&doc.to_bytes().unwrap()).unwrap();
    let parts: Vec<_> = parts
        .into_iter()
        .map(|(name, xml)| match name.starts_with("word/header") {
            true => {
                let xml = String::from_utf8(xml).unwrap();
                let xml = xml
                    .replacen(
                        "<w:p>",
                        r#"<w:bookmarkStart w:id="0" w:name="head"/><w:p>"#,
                        1,
                    )
                    .replacen("</w:hdr>", r#"<w:bookmarkEnd w:id="0"/></w:hdr>"#, 1);
                (name, xml.into_bytes())
            }
            false => (name, xml),
        })
        .collect();
    let original = write_package(&parts);
    let header = |bytes: Vec<u8>| {
        let mut doc = open(&bytes).unwrap();
        assert_eq!(doc.replace_text("Quarterly", "Yearly"), 1);
        let saved = package_parts(&doc.to_bytes().unwrap()).unwrap();
        let (_, xml) = saved
            .iter()
            .find(|(name, _)| name.starts_with("word/header"))
            .unwrap();
        canonical(xml).unwrap()
    };
    let written = rewrite_package(&original, Writing::DefaultNamespace)
        .unwrap()
        .0;
    assert_eq!(header(written), header(original));
}

// WordprocessingML declared as the default namespace below the root, its
// elements unprefixed there: a paragraph, a run, a row, a cell or a link is
// edited as any writing; the body, a table or a block control (no known
// producer writes one there) is refused with a plain message.
#[test]
fn a_default_namespace_below_the_root_is_edited_or_refused() {
    let w = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
    let cell = r#"<w:tbl><w:tblPr><w:tblW w:w="2000" w:type="dxa"/></w:tblPr><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>Cell.</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
    let control = r#"<w:sdt><w:sdtPr><w:id w:val="5"/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>Control.</w:t></w:r></w:p></w:sdtContent></w:sdt>"#;
    let link = r#"<w:p><w:hyperlink w:anchor="b"><w:r><w:t>Link.</w:t></w:r></w:hyperlink></w:p>"#;
    let lead = r#"<w:p><w:r><w:t>Quarterly totals.</w:t></w:r></w:p>"#;
    for (owner, shape, refused) in [
        ("body", lead, true),
        ("tbl", cell, true),
        ("sdt", control, true),
        ("r", lead, false),
        ("tc", cell, false),
        ("hyperlink", link, false),
    ] {
        // The owner's elements unprefixed, the default namespace declared on it.
        let body = format!("{lead}{shape}");
        let (start, end) = match owner {
            "body" => (0, body.len()),
            _ => {
                let tag = |end: &str| body.rfind(&format!("<w:{owner}{end}"));
                let start = tag(">").or_else(|| tag(" ")).unwrap();
                let close = format!("</w:{owner}>");
                (start, body.rfind(&close).unwrap() + close.len())
            }
        };
        let inner = body[start..end].replace("<w:", "<").replace("</w:", "</");
        let inner = match owner {
            "body" => inner,
            _ => inner.replacen(&format!("<{owner}"), &format!(r#"<{owner} xmlns="{w}""#), 1),
        };
        let mut bytes = docx_with_body(&format!("{}{inner}{}", &body[..start], &body[end..]));
        if owner == "body" {
            let parts = package_parts(&bytes)
                .unwrap()
                .into_iter()
                .map(|(name, xml)| {
                    let xml = match name == "word/document.xml" {
                        true => String::from_utf8(xml)
                            .unwrap()
                            .replacen("<w:body>", &format!(r#"<w:body xmlns="{w}">"#), 1)
                            .replace("<w:sectPr", "<sectPr")
                            .replace("</w:sectPr>", "</sectPr>")
                            .replace("<w:pgSz", "<pgSz")
                            .replace("<w:pgMar", "<pgMar")
                            .into_bytes(),
                        false => xml,
                    };
                    (name, xml)
                });
            bytes = write_package(&parts.collect::<Vec<_>>());
        }
        let mut doc = open(&bytes).unwrap();
        let saved = doc
            .try_replace_text("Quarterly", "Yearly")
            .and_then(|_| doc.to_bytes());
        match refused {
            true => assert!(
                saved.is_err_and(|error| {
                    let message = error.to_string();
                    message.contains("declares a default namespace below its root")
                        && !message.contains("limitation")
                }),
                "{owner}"
            ),
            false => {
                let texts = story_texts(&open(&saved.unwrap()).unwrap()).unwrap();
                assert!(texts.contains("Yearly totals."), "{owner}: {texts}");
            }
        }
    }
}
